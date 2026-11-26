use corra_hub::hub::{HubRef, NewProject};
use corra_proto::*;
use serde_json::{Value, json};

fn ev(v: Value) -> Event {
    serde_json::from_value(v).unwrap()
}

fn thread(hub: &HubRef, turns: usize) -> ThreadId {
    let p = hub
        .create_project(NewProject {
            name: format!("p{turns}"),
            ..Default::default()
        })
        .unwrap();
    let t = Thread {
        id: new_id(),
        project: p.id,
        title: "day".into(),
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
    let add = |e: Value| hub.append(t.id, None, 1, ev(e)).unwrap();
    add(json!({ "kind": "session", "session_id": "s", "cwd": "~", "model": "opus" }));
    for i in 1..=turns {
        let turn = format!("t{i}");
        add(
            json!({ "kind": "turn_started", "turn": turn, "settings": {}, "input": [{ "type": "text", "text": format!("ask {i}") }] }),
        );
        for k in 0..3 {
            add(
                json!({ "kind": "item_completed", "turn": turn, "item": { "id": format!("{turn}-{k}"), "type": "agent_message", "text": "step" } }),
            );
        }
        add(json!({ "kind": "turn_completed", "turn": turn, "outcome": "completed" }));
    }
    add(
        json!({ "kind": "item_completed", "turn": "t2", "item": { "id": "late", "type": "agent_message", "text": "a background agent of turn 2 finished" } }),
    );
    t.id
}

fn started(events: &[StoredEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            Event::TurnStarted { turn, .. } => Some(turn.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_long_thread_opens_on_its_last_turns_and_pages_back_to_its_first() {
    let dir = tempfile::tempdir().unwrap();
    let hub = corra_hub::build(corra_hub::Options {
        data: dir.path().into(),
        public_url: "http://x".into(),
        dev_no_auth: true,
    })
    .unwrap()
    .state
    .hub;
    let id = thread(&hub, 20);
    let all = hub.db.events_after(id, 0, 100_000).unwrap();

    let w = hub.history_before(id, i64::MAX, 4).unwrap();
    assert!(w.earlier);
    assert!(
        matches!(&w.events[0].event, Event::Session { model: Some(m), .. } if m == "opus"),
        "the window lost the session it runs in"
    );
    assert_eq!(started(&w.events), ["t17", "t18", "t19", "t20"]);
    assert_eq!(w.events.last().unwrap().seq, all.last().unwrap().seq);

    let mut seen: Vec<i64> = w.events[1..].iter().map(|e| e.seq).collect();
    let mut from = w.from_seq;
    let mut pages = 0;
    loop {
        let h = hub.history_before(id, from, 6).unwrap();
        pages += 1;
        assert!(h.events.iter().all(|e| e.seq < from));
        seen.extend(h.events.iter().map(|e| e.seq));
        if !h.earlier {
            break;
        }
        from = h.from_seq;
        assert!(pages < 10, "paging back never reached the first turn");
    }
    seen.sort();
    let want: Vec<i64> = all.iter().map(|e| e.seq).collect();
    assert_eq!(seen, want, "paging back skipped or repeated events");
}

#[tokio::test]
async fn a_short_thread_is_sent_whole() {
    let dir = tempfile::tempdir().unwrap();
    let hub = corra_hub::build(corra_hub::Options {
        data: dir.path().into(),
        public_url: "http://x".into(),
        dev_no_auth: true,
    })
    .unwrap()
    .state
    .hub;
    let id = thread(&hub, 4);
    assert!(!hub.history_before(id, i64::MAX, 4).unwrap().earlier);
}
