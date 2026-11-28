use corra_hub::hub::{HubRef, NewProject};
use corra_proto::*;
use serde_json::{Value, json};

fn ev(v: Value) -> Event {
    serde_json::from_value(v).unwrap()
}

fn long_thread(hub: &HubRef) -> Thread {
    let p = hub
        .create_project(NewProject {
            name: "p".into(),
            ..Default::default()
        })
        .unwrap();
    let t = Thread {
        id: new_id(),
        project: p.id,
        title: "long".into(),
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
    let add = |e: Event| hub.append(t.id, None, 1, e).unwrap();
    for i in 1..=4 {
        let turn = format!("t{i}");
        add(ev(json!({
            "kind": "turn_started", "turn": turn, "settings": {},
            "input": [{ "type": "text", "text": format!("ask {i}") }],
        })));
        for n in 0..400 {
            add(ev(json!({
                "kind": "item_completed", "turn": turn,
                "item": { "id": format!("{turn}-{n}"), "type": "agent_message", "text": "working" },
            })));
        }
        if i < 4 {
            add(ev(json!({
                "kind": "turn_completed", "turn": turn, "outcome": "completed", "anchor": format!("a{i}"),
            })));
            for _ in 0..50 {
                add(ev(
                    json!({ "kind": "notice", "level": "info", "text": "between" }),
                ));
            }
        }
    }
    hub.db.thread(t.id).unwrap().unwrap()
}

fn hub(dir: &std::path::Path) -> HubRef {
    corra_hub::build(corra_hub::Options {
        data: dir.to_path_buf(),
        public_url: "http://127.0.0.1:1".into(),
        dev_no_auth: false,
    })
    .unwrap()
    .state
    .hub
}

fn completed_turn(hub: &HubRef, t: ThreadId) -> Option<String> {
    let all = hub.db.events_after(t, 0, i64::MAX).unwrap();
    match &all.last()?.event {
        Event::TurnCompleted { turn, .. } => Some(turn.clone()),
        _ => None,
    }
}

#[tokio::test]
async fn a_fork_ends_at_the_turn_it_was_made_from() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub(dir.path());
    let t = long_thread(&hub);

    let f = hub.fork_thread(t.id, Some("t2".into())).await.unwrap();
    assert_eq!(completed_turn(&hub, f.id).as_deref(), Some("t2"));
    let fork_of: ForkSource =
        serde_json::from_str(&hub.thread_kv(f.id, "fork_of").unwrap()).unwrap();
    assert_eq!(fork_of.at_message.as_deref(), Some("a2"));

    let f = hub.fork_thread(t.id, None).await.unwrap();
    assert_eq!(completed_turn(&hub, f.id).as_deref(), Some("t3"));
}

#[tokio::test]
async fn a_rewind_cuts_before_its_turn_and_names_every_turn_after() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub(dir.path());
    let t = long_thread(&hub);
    let all = hub.db.events_after(t.id, 0, i64::MAX).unwrap();
    let start = |name: &str| {
        all.iter()
            .find(|e| matches!(&e.event, Event::TurnStarted { turn, .. } if turn == name))
            .unwrap()
            .seq
    };

    let (seq, anchor, turns) = hub.rewind_point(t.id, "t3").unwrap();
    assert_eq!(seq, start("t3") - 1);
    assert_eq!(anchor.as_deref(), Some("a2"));
    assert_eq!(turns, ["t3", "t4"]);

    let (seq, anchor, turns) = hub.rewind_point(t.id, "t1").unwrap();
    assert_eq!(seq, start("t1") - 1);
    assert_eq!(anchor, None);
    assert_eq!(turns, ["t1", "t2", "t3", "t4"]);

    assert!(hub.rewind_point(t.id, "nope").is_err());
}

#[tokio::test]
async fn reading_a_thread_gives_its_last_turns_and_a_push_finds_the_newest() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub(dir.path());
    let t = long_thread(&hub);

    let read = corra_hub::manage::run(
        &hub,
        &t,
        "threads.read",
        json!({ "thread": t.id.to_string(), "last": 2 }),
    )
    .await
    .unwrap();
    let turns = read["turns"].as_array().unwrap();
    let asked: Vec<&str> = turns.iter().map(|t| t["asked"].as_str().unwrap()).collect();
    assert_eq!(asked, ["ask 3", "ask 4"]);
    assert_eq!(turns[0]["outcome"], "completed");
    assert!(turns[1].get("outcome").is_none());
    assert_eq!(turns[0]["steps"].as_array().unwrap().len(), 400);

    let newest = hub.db.last_turns_started(t.id, 1).unwrap();
    assert!(
        matches!(&newest[..], [e] if matches!(&e.event, Event::TurnStarted { turn, .. } if turn == "t4"))
    );
}

#[tokio::test]
async fn a_thread_an_agent_starts_knows_which_thread_started_it() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub(dir.path());
    let parent = long_thread(&hub);
    let project = hub.db.project(parent.project).unwrap().unwrap();
    let started = corra_hub::manage::run(
        &hub,
        &parent,
        "threads.start",
        json!({ "project": project.name, "prompt": "check the logs", "follow": false }),
    )
    .await
    .unwrap();
    let child: ThreadId = started["thread"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        hub.db.thread(child).unwrap().unwrap().started_by,
        Some(parent.id)
    );
    assert!(
        hub.db
            .threads()
            .unwrap()
            .iter()
            .any(|t| t.id == child && t.started_by == Some(parent.id)),
        "the thread list lost which thread started it"
    );
    assert_eq!(hub.db.thread(parent.id).unwrap().unwrap().started_by, None);
}

#[tokio::test]
async fn slim_events_defers_large_tool_outputs_but_keeps_summary_and_agent_calls() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub(dir.path());
    let p = hub
        .create_project(NewProject {
            name: "slim".into(),
            ..Default::default()
        })
        .unwrap();
    let t = Thread {
        id: new_id(),
        project: p.id,
        title: "slim".into(),
        agent: AgentKind::Claude,
        machine: None,
        session_id: None,
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
    let big_text = "x".repeat(4000);
    let big_content = "y".repeat(4000);
    hub.append(
        t.id,
        None,
        10,
        Event::ItemCompleted {
            turn: "t1".into(),
            item: Item {
                id: "i-read".into(),
                parent: None,
                body: ItemBody::ToolCall {
                    name: "Write".into(),
                    input: json!({ "file_path": "src/main.rs", "content": big_content }),
                    output: Some(ToolOutput {
                        text: big_text.clone(),
                        is_error: false,
                        images: vec![],
                        deferred: None,
                    }),
                    status: ToolStatus::Completed,
                },
            },
        },
    )
    .unwrap();
    hub.append(
        t.id,
        None,
        11,
        Event::ItemCompleted {
            turn: "t1".into(),
            item: Item {
                id: "i-agent".into(),
                parent: None,
                body: ItemBody::ToolCall {
                    name: "Agent".into(),
                    input: json!({ "prompt": "check" }),
                    output: Some(ToolOutput {
                        text: big_text.clone(),
                        is_error: false,
                        images: vec![],
                        deferred: None,
                    }),
                    status: ToolStatus::Completed,
                },
            },
        },
    )
    .unwrap();

    let mut events = hub.db.events_after(t.id, 0, 10).unwrap();
    corra_hub::hub::threads::slim_events(&mut events);

    match &events[0].event {
        Event::ItemCompleted { item, .. } => match &item.body {
            ItemBody::ToolCall { input, output, .. } => {
                assert_eq!(input["file_path"], "src/main.rs");
                assert!(input.get("content").is_none());
                let out = output.as_ref().unwrap();
                assert!(out.text.is_empty());
                assert_eq!(out.deferred, Some(true));
            }
            other => panic!("unexpected body {other:?}"),
        },
        other => panic!("unexpected event {other:?}"),
    }

    match &events[1].event {
        Event::ItemCompleted { item, .. } => match &item.body {
            ItemBody::ToolCall { output, .. } => {
                let out = output.as_ref().unwrap();
                assert_eq!(out.text.len(), 4000);
                assert_eq!(out.deferred, None);
            }
            other => panic!("unexpected body {other:?}"),
        },
        other => panic!("unexpected event {other:?}"),
    }

    let detail = corra_hub::ws::exec_call(
        &hub,
        corra_proto::app::AppCall::EventDetail {
            thread: t.id,
            seq: events[0].seq,
        },
    )
    .await
    .unwrap();
    let full: StoredEvent = serde_json::from_value(detail).unwrap();
    match full.event {
        Event::ItemCompleted { item, .. } => match item.body {
            ItemBody::ToolCall { input, output, .. } => {
                assert_eq!(input["content"].as_str().unwrap().len(), 4000);
                assert_eq!(output.unwrap().text.len(), 4000);
            }
            other => panic!("unexpected body {other:?}"),
        },
        other => panic!("unexpected event {other:?}"),
    }
}
