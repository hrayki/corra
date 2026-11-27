use corra_hub::hub::{HubRef, NewProject};
use corra_proto::app::{AppCall, Found};
use corra_proto::*;
use serde_json::{Value, json};

fn hub(dir: &std::path::Path) -> HubRef {
    corra_hub::build(corra_hub::Options {
        data: dir.to_path_buf(),
        public_url: "http://127.0.0.1:1".into(),
        dev_no_auth: true,
    })
    .unwrap()
    .state
    .hub
}

struct Log<'a> {
    hub: &'a HubRef,
    thread: ThreadId,
    n: u32,
}

impl<'a> Log<'a> {
    fn new(hub: &'a HubRef) -> Self {
        let p = hub
            .create_project(NewProject {
                name: format!("p{}", new_id().simple()),
                ..Default::default()
            })
            .unwrap();
        let t = Thread {
            id: new_id(),
            project: p.id,
            title: "find".into(),
            agent: AgentKind::Claude,
            machine: None,
            session_id: Some("s".into()),
            cwd: "~".into(),
            settings: AgentSettings::default(),
            status: ThreadStatus::Idle,
            last_seq: 0,
            created_at: 0,
            updated_at: 0,
            pinned_at: None,
            settled_at: None,
            hidden_at: None,
            started_by: None,
        };
        hub.db.insert_thread(&t).unwrap();
        let log = Log {
            hub,
            thread: t.id,
            n: 0,
        };
        log.add(json!({ "kind": "session", "session_id": "s", "cwd": "~", "model": "opus" }));
        log
    }

    fn add(&self, e: Value) -> i64 {
        let e: Event = serde_json::from_value(e).unwrap();
        self.hub.append(self.thread, None, 1, e).unwrap();
        self.hub.db.thread(self.thread).unwrap().unwrap().last_seq
    }

    fn id(&mut self, prefix: &str) -> String {
        self.n += 1;
        format!("{prefix}_01ChMg{:06}", self.n)
    }

    fn ask(&mut self, turn: &str, text: &str) -> i64 {
        let seq = self.add(json!({
            "kind": "turn_started", "turn": turn, "settings": { "model": "opus" },
            "input": [{ "type": "text", "text": text }],
        }));
        self.add(json!({
            "kind": "item_completed", "turn": turn,
            "item": { "id": format!("{turn}:input"), "type": "user_message", "content": [{ "type": "text", "text": text }] },
        }));
        seq
    }

    fn think(&mut self, turn: &str, text: &str) -> i64 {
        let id = format!("{}:0", self.id("msg"));
        self.add(json!({
            "kind": "item_completed", "turn": turn,
            "item": { "id": id, "type": "reasoning", "text": text },
        }))
    }

    fn say(&mut self, turn: &str, text: &str) -> i64 {
        let id = format!("{}:0", self.id("msg"));
        self.say_as(turn, &id, None, text)
    }

    fn say_as(&mut self, turn: &str, id: &str, parent: Option<&str>, text: &str) -> i64 {
        self.add(json!({
            "kind": "item_completed", "turn": turn,
            "item": { "id": id, "parent": parent, "type": "agent_message", "text": text },
        }))
    }

    fn tool(&mut self, turn: &str, name: &str, input: Value, output: &str) -> String {
        let id = self.id("toolu");
        self.add(json!({
            "kind": "item_started", "turn": turn,
            "item": { "id": id, "type": "tool_call", "name": name, "input": input, "status": "running" },
        }));
        self.add(json!({
            "kind": "item_completed", "turn": turn,
            "item": {
                "id": id, "type": "tool_call", "name": name, "input": input, "status": "completed",
                "output": { "text": output, "is_error": false },
            },
        }));
        id
    }

    fn bash(&mut self, turn: &str, command: &str, output: &str) -> String {
        self.tool(
            turn,
            "Bash",
            json!({ "command": command, "description": "Run it" }),
            output,
        )
    }

    fn done(&mut self, turn: &str) {
        self.add(json!({
            "kind": "turn_completed", "turn": turn, "outcome": "completed",
            "usage": { "input_tokens": 10, "output_tokens": 20 },
        }));
    }

    fn turn(&mut self, turn: &str, ask: &str, interim: &str, answer: &str) -> (i64, i64, i64) {
        let asked = self.ask(turn, ask);
        self.think(turn, "Let me look at the repo first.");
        self.bash(turn, "ls -la", "Cargo.toml\nsrc\n");
        let interim = self.say(turn, interim);
        self.bash(turn, "cargo test", "test result: ok. 12 passed");
        let answer = self.say(turn, answer);
        self.done(turn);
        (asked, interim, answer)
    }
}

async fn find(hub: &HubRef, thread: ThreadId, query: &str) -> Found {
    let v = corra_hub::ws::exec_call(
        hub,
        AppCall::Find {
            thread,
            query: query.into(),
        },
    )
    .await
    .unwrap();
    serde_json::from_value(v).unwrap()
}

fn seqs(f: &Found) -> Vec<i64> {
    f.hits.iter().map(|h| h.seq).collect()
}

#[tokio::test]
async fn find_searches_the_whole_thread_and_earlier_loads_down_to_a_match() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub(dir.path());
    let mut log = Log::new(&hub);
    let mut spots = vec![];
    for i in 1..=30 {
        let turn = new_id().to_string();
        let (ask, interim, answer) = match i {
            3 => (
                "Where does the Lighthouse config live?",
                "Checking.",
                "In `lighthouse.toml`.",
            ),
            17 => (
                "Rename the flag",
                "The lighthouse check still passes.",
                "Done.",
            ),
            29 => (
                "Thanks",
                "Looking.",
                "LIGHTHOUSE is set up and LightHouse runs nightly.",
            ),
            _ => ("Keep going", "Reading the code.", "Done with this step."),
        };
        spots.push(log.turn(&turn, ask, interim, answer));
    }

    let window = hub.history_before(log.thread, i64::MAX, 4).unwrap();
    let f = find(&hub, log.thread, "lighthouse").await;
    assert_eq!(
        seqs(&f),
        [spots[2].0, spots[2].2, spots[16].1, spots[28].2],
        "find missed a message or matched one twice"
    );
    assert_eq!(f.hits.iter().map(|h| h.n).collect::<Vec<_>>(), [1, 1, 1, 2]);
    assert!(!f.capped);
    assert!(
        f.hits[0].seq < window.from_seq,
        "the oldest match should be outside the window the thread opens on"
    );

    let mut have = window.events.clone();
    let before = window.from_seq;
    let v = corra_hub::ws::exec_call(
        &hub,
        AppCall::Earlier {
            thread: log.thread,
            before_seq: before,
            to_seq: Some(f.hits[0].seq),
        },
    )
    .await
    .unwrap();
    let earlier: Vec<StoredEvent> = serde_json::from_value(v["events"].clone()).unwrap();
    let from = v["from_seq"].as_i64().unwrap();
    assert!(from <= f.hits[0].seq, "earlier stopped short of the match");
    assert!(
        v["earlier"].as_bool().unwrap(),
        "it loaded the whole thread instead of down to the match"
    );
    assert_eq!(
        earlier.first().map(|e| e.seq),
        Some(from),
        "the loaded turns don't start at a turn"
    );
    assert!(matches!(earlier[0].event, Event::TurnStarted { .. }));
    assert_eq!(
        earlier.last().unwrap().seq,
        before - 1,
        "a gap before the window"
    );
    have.splice(0..1, earlier);
    assert!(
        have.windows(2).all(|w| w[1].seq == w[0].seq + 1),
        "the timeline isn't contiguous"
    );
    let starts = have
        .iter()
        .filter(|e| matches!(e.event, Event::TurnStarted { .. }))
        .count();
    assert_eq!(starts, 28, "it should load down to the turn of the match");

    let v = corra_hub::ws::exec_call(
        &hub,
        AppCall::Earlier {
            thread: log.thread,
            before_seq: before,
            to_seq: None,
        },
    )
    .await
    .unwrap();
    let paged: Vec<StoredEvent> = serde_json::from_value(v["events"].clone()).unwrap();
    assert_eq!(
        paged
            .iter()
            .filter(|e| matches!(e.event, Event::TurnStarted { .. }))
            .count(),
        6,
        "scrolling up should still page six turns at a time"
    );
}

#[tokio::test]
async fn find_matches_messages_only() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub(dir.path());
    let mut log = Log::new(&hub);
    let turn = new_id().to_string();
    let asked = log.ask(&turn, "Fix the parser");
    log.think(&turn, "The zebra parser might be wrong.");
    log.bash(&turn, "grep -rn zebra src/", "src/zebra.rs:1: fn zebra()");
    let agent = log.tool(
        &turn,
        "Agent",
        json!({ "description": "Find zebra", "prompt": "Look for zebra in src/" }),
        "zebra is defined in src/zebra.rs",
    );
    log.say_as(
        &turn,
        "msg_01Sub:0",
        Some(&agent),
        "zebra found in the subagent",
    );
    log.add(json!({
        "kind": "item_completed", "turn": turn,
        "item": { "id": "toolu_todo", "type": "todos", "todos": [{ "content": "Fix zebra", "status": "pending" }] },
    }));
    log.add(json!({ "kind": "notice", "level": "info", "text": "zebra notice" }));
    let interim = log.say(&turn, "Zebra is the culprit, fixing it now.");
    log.bash(&turn, "cargo test", "zebra ok");
    let answer = log.say(&turn, "Fixed.");
    log.done(&turn);

    let f = find(&hub, log.thread, "zebra").await;
    assert_eq!(
        seqs(&f),
        [interim],
        "only the agent's message in the folded work should match"
    );
    assert_eq!(seqs(&find(&hub, log.thread, "fixed.").await), [answer]);
    assert_eq!(seqs(&find(&hub, log.thread, "PARSER").await), [asked]);
    assert!(find(&hub, log.thread, "").await.hits.is_empty());
}

#[tokio::test]
async fn find_folds_case_beyond_ascii_and_counts_each_occurrence() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub(dir.path());
    let mut log = Log::new(&hub);
    let turn = new_id().to_string();
    let asked = log.ask(&turn, "L'ÉCOLE est fermée. ÉCOLE, école, École.");
    let said = log.say(&turn, "Straße · STRASSE · ΣΊΣΥΦΟΣ · aaaa");
    log.done(&turn);

    let f = find(&hub, log.thread, "école").await;
    assert_eq!(seqs(&f), [asked]);
    assert_eq!(f.hits[0].n, 4);
    assert_eq!(find(&hub, log.thread, "ÉCOLE").await.hits[0].n, 4);
    assert_eq!(find(&hub, log.thread, "FERMÉE").await.hits[0].n, 1);
    assert_eq!(seqs(&find(&hub, log.thread, "STRAßE").await), [said]);
    assert_eq!(find(&hub, log.thread, "σίσυφοσ").await.hits[0].n, 1);
    assert_eq!(
        find(&hub, log.thread, "aa").await.hits[0].n,
        2,
        "occurrences shouldn't overlap"
    );
}

#[tokio::test]
async fn a_message_is_found_where_the_thread_shows_it() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub(dir.path());
    let mut log = Log::new(&hub);
    let first = new_id().to_string();
    let read_mid = new_id().to_string();
    let never_read = new_id().to_string();
    log.ask(&first, "Run the build");
    log.bash(&first, "sleep 8", "");
    let mid = log.add(json!({
        "kind": "item_completed", "turn": first,
        "item": { "id": format!("{read_mid}:input"), "type": "user_message", "content": [{ "type": "text", "text": "also walrus" }] },
    }));
    log.add(json!({
        "kind": "item_completed", "turn": first,
        "item": { "id": format!("{never_read}:input"), "type": "user_message", "content": [{ "type": "text", "text": "and walrus again" }] },
    }));
    log.say(&first, "Built.");
    log.done(&first);
    let own = log.ask(&never_read, "and walrus again");
    log.done(&never_read);
    let note = new_id().to_string();
    let noted = log.add(json!({
        "kind": "turn_started", "turn": note, "settings": {},
        "input": [{ "type": "text", "text": "[corra] Thread walrus finished." }],
    }));

    let f = find(&hub, log.thread, "walrus").await;
    assert_eq!(
        seqs(&f),
        [mid, own, noted],
        "a message moved to its own turn should be found once, there"
    );
    assert!(find(&hub, log.thread, "[corra]").await.hits.is_empty());
}

#[tokio::test]
async fn earlier_loads_down_to_the_turn_a_late_message_joined() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub(dir.path());
    let mut log = Log::new(&hub);
    let turns: Vec<String> = (0..12).map(|_| new_id().to_string()).collect();
    let mut asked = vec![];
    for (i, t) in turns.iter().enumerate() {
        let ask = if i == 5 {
            "is the narwhal job done?"
        } else {
            "go on"
        };
        asked.push(log.turn(t, ask, "Working.", "Done.").0);
    }
    let late = log.say(&turns[1], "The narwhal job finished.");

    let window = hub.history_before(log.thread, i64::MAX, 4).unwrap();
    let f = find(&hub, log.thread, "narwhal").await;
    assert_eq!(
        seqs(&f),
        [late, asked[5]],
        "matches should come in the order the thread shows them"
    );
    assert!(f.hits[0].seq > window.from_seq);
    let v = corra_hub::ws::exec_call(
        &hub,
        AppCall::Earlier {
            thread: log.thread,
            before_seq: window.from_seq,
            to_seq: Some(late),
        },
    )
    .await
    .unwrap();
    let earlier: Vec<StoredEvent> = serde_json::from_value(v["events"].clone()).unwrap();
    assert!(
        earlier
            .iter()
            .any(|e| matches!(&e.event, Event::TurnStarted { turn, .. } if *turn == turns[1])),
        "the turn the message belongs to wasn't loaded"
    );
}

#[tokio::test]
async fn find_keeps_the_newest_matches_when_there_are_too_many() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub(dir.path());
    let mut log = Log::new(&hub);
    let turn = new_id().to_string();
    log.ask(&turn, "go");
    let mut said = vec![];
    for _ in 0..120 {
        said.push(log.say(&turn, &"ok ".repeat(10)));
    }
    log.done(&turn);

    let f = find(&hub, log.thread, "ok").await;
    assert!(f.capped);
    assert_eq!(f.hits.iter().map(|h| h.n).sum::<u32>(), 1000);
    assert_eq!(seqs(&f), said[20..]);
}
