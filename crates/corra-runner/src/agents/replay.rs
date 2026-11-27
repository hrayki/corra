use std::collections::{HashMap, HashSet};
use std::time::Duration;

use corra_proto::*;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use super::{AgentOut, HubCall, SessionCmd, SessionCtx};

const WAIT: Duration = Duration::from_secs(5);

fn load(name: &str) -> Vec<Value> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/replays")
        .join(format!("{name}.ndjson"));
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn user_text(msg: &Value) -> String {
    match &msg["message"]["content"] {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

struct Ids(HashMap<String, String>);

impl Ids {
    fn learn(&mut self, recorded: &Value, actual: &Value) {
        if let (Some(a), Some(b)) = (recorded.as_str(), actual.as_str())
            && a != b
        {
            self.0.insert(format!("\"{a}\""), format!("\"{b}\""));
        }
    }

    fn apply(&self, line: String) -> String {
        self.0
            .iter()
            .fold(line, |l, (from, to)| l.replace(from.as_str(), to))
    }

    fn matches(&mut self, want: &Value, got: &Value) -> bool {
        if want["type"] != got["type"] {
            return false;
        }
        match want["type"].as_str().unwrap_or("") {
            "user" => {
                if user_text(want) != user_text(got) {
                    return false;
                }
                self.learn(&want["uuid"], &got["uuid"]);
                true
            }
            "control_request" => {
                if want["request"]["subtype"] != got["request"]["subtype"]
                    || want["request"]["cancel_queued"] != got["request"]["cancel_queued"]
                {
                    return false;
                }
                self.learn(&want["request_id"], &got["request_id"]);
                true
            }
            "control_response" => {
                want["response"]["subtype"] == got["response"]["subtype"]
                    && want["response"]["request_id"] == got["response"]["request_id"]
            }
            _ => true,
        }
    }
}

pub async fn replay(name: &str) -> Vec<AgentOut> {
    replay_entries(name, load(name), Duration::ZERO).await
}

async fn replay_without(name: &str, line: usize, linger: Duration) -> Vec<AgentOut> {
    let mut entries = load(name);
    entries.remove(line);
    replay_entries(name, entries, linger).await
}

async fn replay_until_exit(name: &str, lines: usize) -> Vec<AgentOut> {
    let mut entries = load(name);
    entries.truncate(lines);
    entries.push(json!({ "from": "exit" }));
    replay_entries(name, entries, Duration::ZERO).await
}

async fn replay_entries(name: &str, entries: Vec<Value>, linger: Duration) -> Vec<AgentOut> {
    let start = entries
        .iter()
        .find(|e| e["from"] == "start")
        .cloned()
        .unwrap_or_default();
    let initial = entries
        .iter()
        .find(|e| e["from"] == "app" && e["initial"] == true)
        .and_then(|e| serde_json::from_value::<SessionCmd>(e["cmd"].clone()).ok())
        .and_then(|c| match c {
            SessionCmd::Send { input, .. } => Some(input),
            _ => None,
        });
    let (out_tx, mut out_rx) = mpsc::unbounded_channel();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (hub_tx, mut hub_rx) = mpsc::unbounded_channel::<HubCall>();
    tokio::spawn(async move {
        while let Some(call) = hub_rx.recv().await {
            let _ = call.reply.send(Ok(json!("ok")));
        }
    });
    let spec = StartSpec {
        thread: new_id(),
        agent: AgentKind::Claude,
        cwd: "/tmp".into(),
        extra_dirs: vec![],
        resume_session: (start["session"] == "resume").then(|| "replayed".into()),
        settings: serde_json::from_value(start["settings"].clone()).unwrap_or_default(),
        next_seq: 1,
        tools: serde_json::from_value(start["tools"].clone()).unwrap_or_default(),
        repos: vec![],
        agent_env: vec![],
        moved_from: None,
        resume_at: None,
        fork_of: None,
        browser: false,
    };
    let ctx = SessionCtx {
        browser_cdp: None,
        baseline: Default::default(),
        hub_url: String::new(),
        hub_token: String::new(),
        hub: hub_tx,
        sync: None,
        spec,
        cwd: "/tmp".into(),
        extra_dirs: vec![],
        out: out_tx,
        cmds: cmd_rx,
        initial,
    };
    let (ours, theirs) = tokio::io::duplex(1 << 24);
    let (runner_reads, runner_writes) = tokio::io::split(theirs);
    let (claude_reads, mut claude_writes) = tokio::io::split(ours);
    let session = tokio::spawn(super::claude::replay(
        ctx,
        Box::new(runner_reads),
        Box::new(runner_writes),
    ));
    let mut from_runner = BufReader::new(claude_reads).lines();
    let mut unmatched: Vec<Value> = vec![];
    let mut ids = Ids(HashMap::new());
    let mut exited = false;
    for (n, e) in entries.iter().enumerate() {
        match e["from"].as_str().unwrap_or("") {
            "app" if e["initial"] != true => {
                let cmd: SessionCmd = serde_json::from_value(e["cmd"].clone()).unwrap();
                cmd_tx.send(cmd).unwrap();
            }
            "runner" => {
                let want = &e["msg"];
                if let Some(i) = unmatched.iter().position(|got| ids.matches(want, got)) {
                    unmatched.remove(i);
                    continue;
                }
                loop {
                    let line = tokio::time::timeout(WAIT, from_runner.next_line())
                        .await
                        .unwrap_or_else(|_| {
                            panic!("{name}:{}: the runner never wrote {want}", n + 1)
                        })
                        .unwrap()
                        .unwrap_or_else(|| {
                            panic!("{name}:{}: the runner closed before writing {want}", n + 1)
                        });
                    let got: Value = serde_json::from_str(&line).unwrap();
                    if ids.matches(want, &got) {
                        break;
                    }
                    unmatched.push(got);
                }
            }
            "claude" => {
                let line = match (&e["msg"], e["line"].as_str()) {
                    (Value::Null, Some(l)) => l.to_string(),
                    (msg, _) => ids.apply(msg.to_string()),
                };
                claude_writes.write_all(line.as_bytes()).await.unwrap();
                claude_writes.write_all(b"\n").await.unwrap();
            }
            "exit" => {
                claude_writes.shutdown().await.unwrap();
                exited = true;
                break;
            }
            _ => {}
        }
    }
    if !exited {
        tokio::time::sleep(linger).await;
        drop(cmd_tx);
    }
    tokio::time::timeout(WAIT, session)
        .await
        .unwrap_or_else(|_| panic!("{name}: the session didn't end"))
        .unwrap();
    let mut out = vec![];
    while let Ok(o) = out_rx.try_recv() {
        out.push(o);
    }
    check(name, &out, exited);
    out
}

fn check(name: &str, out: &[AgentOut], exited: bool) {
    let mut started = HashSet::new();
    let mut open: Option<String> = None;
    let mut requests = HashSet::new();
    let mut last_status = None;
    let known = |started: &HashSet<String>, turn: &str, what: &str| {
        assert!(
            started.contains(turn),
            "{name}: {what} in turn {turn}, which never started"
        );
    };
    for o in out {
        match o {
            AgentOut::Event(Event::TurnStarted { turn, .. }) => {
                assert!(
                    open.is_none(),
                    "{name}: turn {turn} started while {open:?} was open"
                );
                assert!(
                    started.insert(turn.clone()),
                    "{name}: turn {turn} started twice"
                );
                open = Some(turn.clone());
            }
            AgentOut::Event(Event::TurnCompleted { turn, .. }) => {
                assert_eq!(
                    open.as_deref(),
                    Some(turn.as_str()),
                    "{name}: turn {turn} completed but {open:?} was the open one"
                );
                open = None;
            }
            AgentOut::Event(Event::ItemStarted { turn, .. })
            | AgentOut::Event(Event::ItemCompleted { turn, .. }) => {
                known(&started, turn, "an item")
            }
            AgentOut::Delta { turn, .. } => known(&started, turn, "a delta"),
            AgentOut::Event(Event::Request { id, turn, .. }) => {
                known(&started, turn, "a request");
                requests.insert(id.clone());
            }
            AgentOut::Event(Event::RequestResolved { id, .. }) => {
                assert!(
                    requests.remove(id),
                    "{name}: request {id} resolved but wasn't open"
                );
            }
            AgentOut::Status(s) => last_status = Some(*s),
            AgentOut::Exited { error: Some(e) } => panic!("{name}: the session failed: {e}"),
            _ => {}
        }
    }
    assert!(open.is_none(), "{name}: turn {open:?} was left open");
    assert!(
        requests.is_empty(),
        "{name}: requests {requests:?} were left open"
    );
    let failed = out.iter().rev().find_map(|o| match o {
        AgentOut::Event(Event::TurnCompleted { outcome, .. }) => {
            Some(*outcome == TurnOutcome::Failed)
        }
        _ => None,
    });
    if let Some(s) = last_status.filter(|_| !exited) {
        let want = if failed == Some(true) {
            ThreadStatus::Error
        } else {
            ThreadStatus::Idle
        };
        assert_eq!(s, want, "{name}: the thread ended {s:?}");
    }
}

#[derive(Default, Debug)]
struct Turn {
    id: String,
    input: String,
    cause: Option<String>,
    items: Vec<Item>,
    requests: Vec<(AgentRequest, Option<RequestResponse>)>,
    outcome: Option<TurnOutcome>,
    error: Option<String>,
    resets_at: Option<i64>,
    after_end: Vec<String>,
}

impl Turn {
    fn texts(&self) -> Vec<&str> {
        self.items
            .iter()
            .filter_map(|i| match &i.body {
                ItemBody::AgentMessage { text } if i.parent.is_none() => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn position(&self, f: impl Fn(&Item) -> bool) -> usize {
        self.items
            .iter()
            .position(f)
            .unwrap_or_else(|| panic!("no such item in turn {}", self.id))
    }
}

fn text_of(parts: &[InputPart]) -> String {
    parts
        .iter()
        .filter_map(|p| match p {
            InputPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn turns(out: &[AgentOut]) -> Vec<Turn> {
    let mut turns: Vec<Turn> = vec![];
    let mut pending: HashMap<String, usize> = HashMap::new();
    for o in out {
        let AgentOut::Event(e) = o else { continue };
        match e {
            Event::TurnStarted {
                turn, input, cause, ..
            } => turns.push(Turn {
                id: turn.clone(),
                input: text_of(input),
                cause: cause.clone(),
                ..Default::default()
            }),
            Event::TurnCompleted {
                turn,
                outcome,
                error,
                resets_at,
                ..
            } => {
                let t = turns.iter_mut().find(|t| &t.id == turn).unwrap();
                t.outcome = Some(*outcome);
                t.error = error.clone();
                t.resets_at = *resets_at;
            }
            Event::ItemCompleted { turn, item } => {
                let t = turns.iter_mut().find(|t| &t.id == turn).unwrap();
                if t.outcome.is_some() {
                    t.after_end.push(item.id.clone());
                }
                match t.items.iter_mut().find(|i| i.id == item.id) {
                    Some(i) => *i = item.clone(),
                    None => t.items.push(item.clone()),
                }
            }
            Event::ItemStarted { turn, item } => {
                let t = turns.iter_mut().find(|t| &t.id == turn).unwrap();
                t.items.push(item.clone());
            }
            Event::Request { id, turn, request } => {
                let n = turns.iter().position(|t| &t.id == turn).unwrap();
                pending.insert(id.clone(), n);
                turns[n].requests.push((request.clone(), None));
            }
            Event::RequestResolved { id, response } => {
                let n = pending[id];
                if let Some(r) = turns[n].requests.iter_mut().find(|r| r.1.is_none()) {
                    r.1 = Some(response.clone());
                }
            }
            _ => {}
        }
    }
    turns
}

fn notices(out: &[AgentOut]) -> Vec<String> {
    out.iter()
        .filter_map(|o| match o {
            AgentOut::Event(Event::Notice { text, .. }) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn tool(i: &Item, name: &str) -> bool {
    matches!(&i.body, ItemBody::ToolCall { name: n, .. } if n == name)
}

#[test]
fn committed_recordings_hold_no_account_details() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/replays");
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let text = std::fs::read_to_string(e.path()).unwrap();
        assert!(
            text == super::record::scrub_file(&text),
            "{} has account details; record it again",
            e.path().display()
        );
    }
}

#[tokio::test]
async fn a_background_agent_joins_its_turn_and_wakes_the_thread_once() {
    let out = replay("background_agent").await;
    let turns = turns(&out);
    assert_eq!(turns.len(), 2, "{turns:#?}");
    let (first, wake) = (&turns[0], &turns[1]);
    assert_eq!(first.texts(), ["STARTED"]);
    assert_eq!(first.outcome, Some(TurnOutcome::Completed));
    let agent = &first.items[first.position(|i| tool(i, "Agent"))].id;
    assert!(
        first.items.iter().any(|i| i.parent.as_ref() == Some(agent)
            && matches!(&i.body, ItemBody::AgentMessage { text } if text == "BG_DONE")),
        "the agent's own steps left the turn that started it"
    );
    assert_eq!(wake.input, "", "the wake looks like a message from me");
    assert_eq!(
        wake.cause.as_deref(),
        Some("Background agent finished: Run delayed echo check"),
        "the wake doesn't say what woke it"
    );
    let tasks: Vec<&Item> = turns
        .iter()
        .flat_map(|t| &t.items)
        .filter(|i| i.id.starts_with("task:"))
        .collect();
    assert_eq!(
        tasks.len(),
        1,
        "a foreground command showed as a background agent: {tasks:#?}"
    );
    assert!(
        matches!(&tasks[0].body, ItemBody::AgentMessage { text } if text == "Background agent completed: BG_DONE")
    );
    assert!(
        first
            .items
            .iter()
            .any(|i| i.id == tasks[0].id && i.parent.as_ref() == Some(agent)),
        "the agent's result left the call that started it"
    );
    assert_eq!(wake.texts(), ["WAKE_DONE"]);
    let last_background = out.iter().rev().find_map(|o| match o {
        AgentOut::Event(Event::Status {
            background: Some(b),
            ..
        }) => Some(*b),
        _ => None,
    });
    assert_eq!(last_background, Some(0), "still says an agent is working");
}

#[tokio::test]
async fn a_background_command_wakes_the_thread_with_its_own_words() {
    let out = replay("background_bash_wake").await;
    let turns = turns(&out);
    assert_eq!(turns.len(), 3, "{turns:#?}");
    let (ran, wake) = (&turns[1], &turns[2]);
    assert_eq!(wake.input, "");
    assert_eq!(
        wake.cause.as_deref(),
        Some("Background command \"Pause for 60 seconds in background\" completed (exit code 0)")
    );
    let bash = &ran.items[ran.position(|i| {
        tool(i, "Bash")
            && matches!(&i.body, ItemBody::ToolCall { input, .. } if input["run_in_background"] == true)
    })]
    .id;
    assert!(
        ran.items
            .iter()
            .any(|i| i.id.starts_with("task:") && i.parent.as_ref() == Some(bash)),
        "the command's result left the call that started it: {ran:#?}"
    );
    assert_eq!(wake.texts(), ["SLEPT"]);
}

#[tokio::test]
async fn a_message_sent_mid_turn_stays_where_it_was_sent() {
    let out = replay("mid_turn_message").await;
    let turns = turns(&out);
    assert_eq!(turns.len(), 1, "{turns:#?}");
    let t = &turns[0];
    let bash = t.position(|i| tool(i, "Bash"));
    let sent = t.position(|i| {
        matches!(&i.body, ItemBody::UserMessage { content } if text_of(content).contains("SECOND_DONE"))
    });
    let answer = t.position(
        |i| matches!(&i.body, ItemBody::AgentMessage { text } if text.contains("SECOND_DONE")),
    );
    assert!(bash < sent && sent < answer, "{t:#?}");
    assert!(
        t.after_end.is_empty(),
        "items after the turn ended: {:?}",
        t.after_end
    );
    assert!(
        !t.items.iter().any(|i| i.id.starts_with("task:")),
        "a foreground command showed as a background agent"
    );
}

fn unread(out: &[AgentOut]) -> Vec<String> {
    out.iter()
        .filter_map(|o| match o {
            AgentOut::Event(Event::Unread { item }) => Some(item.clone()),
            _ => None,
        })
        .collect()
}

fn message_id(turn: &Turn, text: &str) -> String {
    turn.items[turn.position(
        |i| matches!(&i.body, ItemBody::UserMessage { content } if text_of(content).contains(text)),
    )]
    .id
    .clone()
}

fn shown(out: &[AgentOut], text: &str) -> usize {
    out.iter()
        .filter(|o| match o {
            AgentOut::Event(Event::TurnStarted { input, .. }) => text_of(input).contains(text),
            AgentOut::Event(Event::ItemCompleted {
                item:
                    Item {
                        id,
                        body: ItemBody::UserMessage { content },
                        ..
                    },
                turn,
            }) => *id != format!("{turn}:input") && text_of(content).contains(text),
            _ => false,
        })
        .count()
}

#[tokio::test]
async fn a_stop_runs_nothing_after_it_and_marks_a_waiting_message_unread() {
    let out = replay("stop_with_waiting_message").await;
    let turns = turns(&out);
    assert_eq!(
        turns.iter().map(|t| t.outcome).collect::<Vec<_>>(),
        [Some(TurnOutcome::Interrupted)],
        "{turns:#?}"
    );
    let waiting = message_id(&turns[0], "SECOND_DONE");
    assert_eq!(unread(&out), [waiting]);
    assert_eq!(shown(&out, "SECOND_DONE"), 1);
}

#[tokio::test]
async fn an_exit_marks_a_message_claude_code_never_took_and_shows_it_once() {
    let out = replay_until_exit("mid_turn_message", 25).await;
    let turns = turns(&out);
    assert_eq!(turns.len(), 1, "{turns:#?}");
    assert_eq!(turns[0].outcome, Some(TurnOutcome::Failed));
    assert_eq!(unread(&out), [message_id(&turns[0], "SECOND_DONE")]);
    assert_eq!(shown(&out, "SECOND_DONE"), 1);
}

#[tokio::test]
async fn a_turn_whose_result_never_comes_ends_when_claude_code_says_its_command_finished() {
    let out = replay_without("mid_turn_message", 69, Duration::from_secs(5)).await;
    let turns = turns(&out);
    assert_eq!(turns.len(), 1, "{turns:#?}");
    assert_eq!(turns[0].outcome, Some(TurnOutcome::Completed));
    assert!(unread(&out).is_empty(), "{:?}", unread(&out));
    assert_eq!(shown(&out, "SECOND_DONE"), 1);
}

#[tokio::test]
async fn an_exit_after_claude_code_took_a_message_leaves_it_read() {
    let out = replay_until_exit("mid_turn_message", 29).await;
    let turns = turns(&out);
    assert_eq!(turns.len(), 1, "{turns:#?}");
    assert!(unread(&out).is_empty(), "{:?}", unread(&out));
    assert_eq!(shown(&out, "SECOND_DONE"), 1);
}

#[tokio::test]
async fn a_message_sent_as_a_background_agent_finishes_keeps_its_answer() {
    let out = replay("message_at_wake").await;
    let turns = turns(&out);
    let mine = turns
        .iter()
        .find(|t| t.input.contains("MINE"))
        .expect("my message has no turn");
    assert!(
        mine.texts().iter().any(|t| t.contains("MINE")),
        "my turn ended without its answer: {turns:#?}"
    );
    for t in &turns {
        assert!(
            !t.input.is_empty() || t.cause.is_some(),
            "a turn shows no reason it started: {t:#?}"
        );
        assert!(
            t.input.contains("MINE") || !t.texts().iter().any(|x| x.contains("MINE")),
            "my answer landed in another turn: {t:#?}"
        );
        assert!(t.outcome.is_some(), "a turn never ended: {t:#?}");
    }
}

#[tokio::test]
async fn a_background_agent_shows_as_running_until_it_is_stopped() {
    let out = replay("stop_agent").await;
    let turns = turns(&out);
    let first = &turns[0];
    let agent = first.items[first.position(|i| tool(i, "Agent"))].id.clone();
    let lists: Vec<&Vec<String>> = out
        .iter()
        .filter_map(|o| match o {
            AgentOut::Event(Event::Status {
                agents: Some(a), ..
            }) => Some(a),
            _ => None,
        })
        .collect();
    assert!(
        lists.iter().any(|l| **l == [agent.clone()]),
        "the agent never showed as running: {lists:?}"
    );
    assert!(
        lists.last().is_some_and(|l| l.is_empty()),
        "it still shows as running after it was stopped: {lists:?}"
    );
    assert!(
        first.items.iter().any(|i| i.parent.as_ref() == Some(&agent)
            && matches!(&i.body, ItemBody::AgentMessage { text } if text == "Background agent stopped: Wait a minute")),
        "{first:#?}"
    );
    assert_eq!(
        turns.last().unwrap().cause.as_deref(),
        Some("Background agent stopped: Wait a minute")
    );
}

#[tokio::test]
async fn the_session_offers_claude_codes_own_models_and_efforts() {
    let out = replay("approval_and_interrupt").await;
    let models = out
        .iter()
        .find_map(|o| match o {
            AgentOut::Event(Event::Session { models, .. }) => Some(models.clone()),
            _ => None,
        })
        .expect("no session");
    let ids: Vec<(&str, &str)> = models
        .iter()
        .map(|m| (m.id.as_str(), m.name.as_str()))
        .collect();
    assert_eq!(
        ids,
        [
            ("default", "Default (recommended)"),
            ("sonnet", "Sonnet"),
            ("fable", "Fable"),
            ("opus", "Opus"),
            ("haiku", "Haiku")
        ]
    );
    let opus = &models[3];
    assert_eq!(opus.resolved_model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(
        opus.hint.as_deref(),
        Some("Opus 5.5 \u{b7} Best for everyday, complex tasks")
    );
    assert_eq!(opus.supports_auto_mode, Some(true));
    let efforts: Vec<(&str, &str)> = opus
        .efforts
        .iter()
        .flatten()
        .map(|e| (e.id.as_str(), e.name.as_str()))
        .collect();
    assert_eq!(
        efforts,
        [
            ("low", "Low"),
            ("medium", "Medium"),
            ("high", "High"),
            ("xhigh", "xHigh"),
            ("max", "Max")
        ]
    );
}

#[tokio::test]
async fn a_usage_limit_shows_once_with_its_reset_time() {
    let out = replay("usage_limit").await;
    let turns = turns(&out);
    assert_eq!(turns.len(), 2);
    for t in &turns {
        assert_eq!(t.outcome, Some(TurnOutcome::Failed));
        assert_eq!(
            t.error.as_deref(),
            Some("You've hit your limit · resets 1:27pm (UTC)")
        );
        assert_eq!(t.resets_at, Some(1_795_526_865_000));
    }
    assert!(
        notices(&out).is_empty(),
        "the limit showed twice: {:?}",
        notices(&out)
    );
}

#[tokio::test]
async fn an_approval_lets_the_tool_run_and_an_interrupt_ends_the_turn() {
    let out = replay("approval_and_interrupt").await;
    let turns = turns(&out);
    assert_eq!(turns.len(), 2);
    let (write, stop) = (&turns[0], &turns[1]);
    assert!(matches!(
        &write.requests[..],
        [(AgentRequest::ToolApproval { tool, .. }, Some(RequestResponse::Allow { .. }))] if tool == "Write"
    ));
    assert_eq!(write.texts(), ["WRITTEN"]);
    assert_eq!(write.outcome, Some(TurnOutcome::Completed));
    assert!(matches!(
        &stop.requests[..],
        [(AgentRequest::ToolApproval { tool, .. }, Some(RequestResponse::Allow { .. }))] if tool == "Bash"
    ));
    assert_eq!(stop.outcome, Some(TurnOutcome::Interrupted));
    let bash = &stop.items[stop.position(|i| tool(i, "Bash"))];
    assert!(
        !matches!(
            &bash.body,
            ItemBody::ToolCall {
                status: ToolStatus::Running,
                ..
            }
        ),
        "the interrupted command still runs"
    );
}

enum Then {
    StopAgent,
    Send(&'static str),
    SendIn(&'static str, &'static str),
    Allow,
    InterruptIn(u64),
}

struct Step {
    when: fn(&AgentOut) -> bool,
    then: Then,
}

fn tool_started(o: &AgentOut, tool: &str) -> bool {
    matches!(o, AgentOut::Event(Event::ItemStarted { item: Item { body: ItemBody::ToolCall { name, .. }, .. }, .. }) if name == tool)
}

fn idle(o: &AgentOut) -> bool {
    matches!(
        o,
        AgentOut::Status(ThreadStatus::Idle | ThreadStatus::Error)
    )
}

async fn record(name: &str, prompt: &str, settings: AgentSettings, steps: Vec<Step>, quiet: u64) {
    let token = std::fs::read_to_string(std::env::var("CORRA_RECORD_TOKEN_FILE").unwrap())
        .unwrap()
        .trim()
        .to_string();
    record_with(
        name,
        prompt,
        settings,
        steps,
        quiet,
        vec![("CLAUDE_CODE_OAUTH_TOKEN".into(), token)],
    )
    .await
}

async fn record_with(
    name: &str,
    prompt: &str,
    settings: AgentSettings,
    steps: Vec<Step>,
    quiet: u64,
    agent_env: Vec<(String, String)>,
) {
    let home = crate::files::home();
    let dir = super::record::dir();
    std::fs::create_dir_all(&dir).unwrap();
    let cwd = std::path::PathBuf::from(format!("/tmp/corra-replay/{name}"));
    let _ = std::fs::remove_dir_all(&cwd);
    std::fs::create_dir_all(&cwd).unwrap();
    let before: HashSet<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (hub_tx, mut hub_rx) = mpsc::unbounded_channel::<HubCall>();
    tokio::spawn(async move {
        while let Some(call) = hub_rx.recv().await {
            let _ = call.reply.send(Ok(json!("ok")));
        }
    });
    let ctx = SessionCtx {
        browser_cdp: None,
        baseline: Default::default(),
        hub_url: String::new(),
        hub_token: String::new(),
        hub: hub_tx,
        sync: None,
        spec: StartSpec {
            thread: new_id(),
            agent: AgentKind::Claude,
            cwd: cwd.to_string_lossy().into(),
            extra_dirs: vec![],
            resume_session: None,
            settings,
            next_seq: 1,
            tools: vec![],
            repos: vec![],
            agent_env,
            moved_from: None,
            resume_at: None,
            fork_of: None,
            browser: false,
        },
        cwd: cwd.clone(),
        extra_dirs: vec![],
        out: out_tx,
        cmds: cmd_rx,
        initial: Some(vec![InputPart::Text {
            text: prompt.into(),
        }]),
    };
    let session = tokio::spawn(super::claude::run(ctx));
    let mut steps = steps.into_iter();
    let mut step = steps.next();
    let mut last_request = None;
    let mut last_agent: Option<String> = None;
    let mut done_at: Option<tokio::time::Instant> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
    loop {
        let wait = match done_at {
            Some(at) => at + Duration::from_secs(quiet),
            None => deadline,
        };
        let o = match tokio::time::timeout_at(wait, out_rx.recv()).await {
            Ok(Some(o)) => o,
            Ok(None) => break,
            Err(_) if done_at.is_some() => break,
            Err(_) => panic!("{name}: timed out on a step"),
        };
        match &o {
            AgentOut::Delta { .. } => {}
            other => eprintln!("{name}: {other:?}"),
        }
        if let AgentOut::Exited { error: Some(e) } = &o {
            panic!("{name}: Claude Code failed: {e}");
        }
        if let AgentOut::Event(Event::Request { id, .. }) = &o {
            last_request = Some(id.clone());
        }
        if let AgentOut::Event(Event::ItemCompleted { item, .. }) = &o
            && tool(item, "Agent")
        {
            last_agent = Some(item.id.clone());
        }
        if let Some(s) = &step {
            if (s.when)(&o) {
                let cmd = match &s.then {
                    Then::StopAgent => SessionCmd::StopTask {
                        tool: last_agent.clone().unwrap(),
                    },
                    Then::Send(t) => SessionCmd::Send {
                        input: vec![InputPart::Text { text: (*t).into() }],
                        settings: None,
                    },
                    Then::SendIn(mode, t) => SessionCmd::Send {
                        input: vec![InputPart::Text { text: (*t).into() }],
                        settings: Some(AgentSettings {
                            permission_mode: Some((*mode).into()),
                            ..Default::default()
                        }),
                    },
                    Then::Allow => SessionCmd::Respond {
                        request: last_request.clone().unwrap(),
                        response: RequestResponse::Allow { always: false },
                    },
                    Then::InterruptIn(ms) => {
                        let tx = cmd_tx.clone();
                        let ms = *ms;
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_millis(ms)).await;
                            let _ = tx.send(SessionCmd::Interrupt);
                        });
                        step = steps.next();
                        continue;
                    }
                };
                cmd_tx.send(cmd).unwrap();
                step = steps.next();
            }
        } else if idle(&o) || done_at.is_some() {
            done_at = Some(tokio::time::Instant::now());
        }
    }
    drop(cmd_tx);
    let _ = tokio::time::timeout(Duration::from_secs(10), session).await;
    let made: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| !before.contains(p))
        .collect();
    assert_eq!(
        made.len(),
        1,
        "{name}: expected one recording in {}",
        home.display()
    );
    let to = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/replays")
        .join(format!("{name}.ndjson"));
    std::fs::create_dir_all(to.parent().unwrap()).unwrap();
    let text = std::fs::read_to_string(&made[0]).unwrap();
    std::fs::write(&to, super::record::scrub_file(&text)).unwrap();
    eprintln!("{name}: wrote {}", to.display());
}

fn haiku() -> AgentSettings {
    AgentSettings {
        model: Some("haiku".into()),
        permission_mode: Some("bypassPermissions".into()),
        ..Default::default()
    }
}

#[tokio::test]
#[ignore]
async fn record_background_agent() {
    record(
        "background_agent",
        "Use the Agent tool with run_in_background set to true to start one background agent whose only job is to run `sleep 15 && echo BG_DONE` with Bash and reply BG_DONE. Don't wait for it: as soon as it has started, reply with exactly STARTED and end your turn. When you're told it finished, reply with exactly WAKE_DONE.",
        haiku(),
        vec![],
        40,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn record_stop_agent() {
    record(
        "stop_agent",
        "Use the Agent tool with run_in_background set to true to start one background agent, described as \"Wait a minute\", whose only job is to run `sleep 60 && echo BG_DONE` with Bash and reply BG_DONE. Don't wait for it: as soon as it has started, reply with exactly STARTED and end your turn. If you're told it stopped, reply with exactly STOPPED.",
        haiku(),
        vec![Step {
            when: idle,
            then: Then::StopAgent,
        }],
        25,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn record_message_at_wake() {
    record(
        "message_at_wake",
        "Use the Agent tool with run_in_background set to true to start one background agent whose only job is to run `sleep 10 && echo BG_DONE` with Bash and reply BG_DONE. Don't wait for it: as soon as it has started, reply with exactly STARTED and end your turn. When you're told it finished, reply with exactly WAKE_DONE.",
        haiku(),
        vec![Step {
            when: |o| matches!(o, AgentOut::Event(Event::ItemCompleted { item, .. }) if item.id.starts_with("task:")),
            then: Then::Send("Reply with exactly MINE."),
        }],
        30,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn record_mid_turn_message() {
    record(
        "mid_turn_message",
        "Run `sleep 8` with Bash. When it's done, reply with exactly FIRST_DONE.",
        haiku(),
        vec![Step {
            when: |o| tool_started(o, "Bash"),
            then: Then::Send("Also reply with exactly SECOND_DONE after that."),
        }],
        5,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn record_stop_with_waiting_message() {
    record(
        "stop_with_waiting_message",
        "Run this exact command with Bash in the foreground, not in the background: python3 -c \"import time; time.sleep(30)\". Then reply with exactly FIRST_DONE.",
        haiku(),
        vec![
            Step {
                when: |o| tool_started(o, "Bash"),
                then: Then::Send("Also reply with exactly SECOND_DONE after that."),
            },
            Step {
                when: |o| matches!(o, AgentOut::Event(Event::ItemCompleted { item, .. }) if item.id.ends_with(":input")),
                then: Then::InterruptIn(3000),
            },
        ],
        20,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn record_approval_and_interrupt() {
    record(
        "approval_and_interrupt",
        "Use the Write tool to create hello.txt containing hi, then reply with exactly WRITTEN.",
        AgentSettings { model: Some("haiku".into()), permission_mode: Some("default".into()), ..Default::default() },
        vec![
            Step { when: |o| matches!(o, AgentOut::Event(Event::Request { .. })), then: Then::Allow },
            Step { when: idle, then: Then::Send("Run this exact command with Bash in the foreground, not in the background: python3 -c \"import time; time.sleep(45)\". Then reply with exactly SLEPT.") },
            Step { when: |o| matches!(o, AgentOut::Event(Event::Request { .. })), then: Then::Allow },
            Step { when: |o| matches!(o, AgentOut::Event(Event::RequestResolved { .. })), then: Then::InterruptIn(3000) },
        ],
        5,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn record_auto_mode() {
    record(
        "auto_mode",
        "Reply with exactly READY.",
        AgentSettings {
            model: Some("haiku".into()),
            permission_mode: Some("default".into()),
            ..Default::default()
        },
        vec![Step {
            when: idle,
            then: Then::SendIn(
                "auto",
                "Use the Write tool to create hello.txt containing hi, then run `cat hello.txt` with Bash, then reply with exactly AUTO_DONE.",
            ),
        }],
        5,
    )
    .await;
}

#[tokio::test]
async fn switching_a_thread_to_auto_lets_it_edit_and_run_without_asking() {
    let out = replay("auto_mode").await;
    let turns = turns(&out);
    assert_eq!(turns.len(), 2, "{turns:#?}");
    assert!(out.iter().any(|o| matches!(o,
        AgentOut::Event(Event::SettingsChanged { settings }) if settings.permission_mode.as_deref() == Some("auto"))));
    let t = &turns[1];
    assert!(t.requests.is_empty(), "auto mode asked: {:?}", t.requests);
    for name in ["Write", "Bash"] {
        assert!(
            t.items.iter().any(|i| matches!(&i.body,
                ItemBody::ToolCall { name: n, status: ToolStatus::Completed, .. } if n == name)),
            "{name} didn't run: {t:#?}"
        );
    }
    assert_eq!(t.texts(), ["AUTO_DONE"]);
    assert_eq!(t.outcome, Some(TurnOutcome::Completed));
}

async fn limited_api() -> String {
    use tokio::io::AsyncReadExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![];
                let mut chunk = [0u8; 8192];
                let head_end = loop {
                    let n = sock.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..head_end]).to_lowercase();
                let length: usize = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0);
                while buf.len() < head_end + length {
                    let n = sock.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let reset = now_ms() / 1000 + 2 * 3600;
                let response = if head.starts_with("post /v1/messages")
                    && !head.contains("count_tokens")
                {
                    let body = r#"{"type":"error","error":{"type":"rate_limit_error","message":"This request would exceed your account's rate limit. Please try again later."}}"#;
                    format!(
                        "HTTP/1.1 429 Too Many Requests\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\
                         anthropic-ratelimit-unified-status: rejected\r\nanthropic-ratelimit-unified-reset: {reset}\r\n\
                         anthropic-ratelimit-unified-representative-claim: five_hour\r\n\
                         anthropic-ratelimit-unified-5h-status: rejected\r\nanthropic-ratelimit-unified-5h-utilization: 1.0\r\nanthropic-ratelimit-unified-5h-reset: {reset}\r\n\
                         anthropic-ratelimit-unified-7d-status: allowed\r\nanthropic-ratelimit-unified-7d-utilization: 0.14\r\nanthropic-ratelimit-unified-7d-reset: {}\r\n\
                         anthropic-ratelimit-unified-overage-status: rejected\r\nretry-after: 7200\r\n\r\n{body}",
                        body.len(),
                        reset + 4 * 86400
                    )
                } else {
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}".to_string()
                };
                let _ = sock.write_all(response.as_bytes()).await;
            });
        }
    });
    format!("http://{addr}")
}

#[tokio::test]
#[ignore]
async fn record_usage_limit() {
    let api = limited_api().await;
    record_with(
        "usage_limit",
        "Reply with exactly HI.",
        haiku(),
        vec![Step {
            when: idle,
            then: Then::Send("Reply with exactly AGAIN."),
        }],
        3,
        vec![
            ("CLAUDE_CODE_OAUTH_TOKEN".into(), "not-a-real-token".into()),
            ("ANTHROPIC_BASE_URL".into(), api),
        ],
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn dump() {
    let name = std::env::var("REPLAY").unwrap();
    for o in replay(&name).await {
        match o {
            AgentOut::Delta { .. } => {}
            AgentOut::Event(Event::Status {
                context: Some(_), ..
            }) => {}
            AgentOut::Event(Event::Status {
                rate_limit: Some(_),
                ..
            }) => println!("Status(rate_limit)"),
            other => println!(
                "{}",
                format!("{other:?}").chars().take(260).collect::<String>()
            ),
        }
    }
}

#[tokio::test]
#[ignore]
async fn record_compact() {
    record(
        "compact",
        "Reply with exactly READY.",
        haiku(),
        vec![
            Step {
                when: idle,
                then: Then::Send("/compact"),
            },
            Step {
                when: |o| matches!(o, AgentOut::Event(Event::Compacted { .. })),
                then: Then::Send("Reply with exactly AFTER."),
            },
        ],
        5,
    )
    .await;
}

#[tokio::test]
async fn a_compaction_reports_the_smaller_context_and_the_next_message_gets_its_own_turn() {
    let out = replay("compact").await;
    let turns = turns(&out);
    assert_eq!(turns.len(), 3, "{turns:#?}");
    assert_eq!(turns[1].input, "/compact");
    assert_eq!(turns[1].outcome, Some(TurnOutcome::Completed));
    assert_eq!(turns[2].input, "Reply with exactly AFTER.");
    assert_eq!(turns[2].texts(), ["AFTER"]);
    let compacted = out
        .iter()
        .position(|o| matches!(o, AgentOut::Event(Event::Compacted { .. })))
        .expect("no compaction");
    let after = out[..compacted].iter().rev().find_map(|o| match o {
        AgentOut::Event(Event::Status {
            context: Some(c), ..
        }) => Some(c.used_tokens),
        _ => None,
    });
    assert_eq!(
        after,
        Some(3054),
        "the context still shows its size before the summary"
    );
}
