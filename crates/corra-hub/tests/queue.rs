use std::time::Duration;

use corra_hub::hub::HubRef;
use corra_proto::*;

fn text(s: &str) -> Vec<InputPart> {
    vec![InputPart::Text { text: s.into() }]
}

fn setup(dir: &std::path::Path) -> (HubRef, ThreadId) {
    let hub = corra_hub::build(corra_hub::Options {
        data: dir.to_path_buf(),
        public_url: "http://127.0.0.1:1".into(),
        dev_no_auth: false,
    })
    .unwrap()
    .state
    .hub;
    let p = hub
        .create_project(corra_hub::hub::NewProject {
            name: "p".into(),
            provider: Some("existing".into()),
            ..Default::default()
        })
        .unwrap();
    let t = Thread {
        id: new_id(),
        project: p.id,
        title: "t".into(),
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
    };
    hub.db.insert_thread(&t).unwrap();
    (hub, t.id)
}

fn turn(hub: &HubRef, thread: ThreadId, id: &str, outcome: Option<TurnOutcome>) {
    let event = match outcome {
        None => Event::TurnStarted {
            turn: id.into(),
            input: vec![],
            settings: AgentSettings::default(),
        },
        Some(outcome) => Event::TurnCompleted {
            turn: id.into(),
            outcome,
            usage: None,
            error: None,
            anchor: None,
            resets_at: None,
        },
    };
    hub.append(thread, None, now_ms(), event).unwrap();
}

fn waiting(hub: &HubRef, thread: ThreadId) -> Vec<String> {
    hub.queued(thread)
        .into_iter()
        .map(|q| match &q.input[0] {
            InputPart::Text { text } => text.clone(),
            _ => String::new(),
        })
        .collect()
}

async fn tried_to_send(hub: &HubRef, thread: ThreadId) -> usize {
    tokio::time::sleep(Duration::from_millis(100)).await;
    hub.db
        .events_after(thread, 0, 1000)
        .unwrap()
        .iter()
        .filter(
            |e| matches!(&e.event, Event::Notice { text, .. } if text.starts_with("Couldn't send")),
        )
        .count()
}

#[tokio::test]
async fn queued_messages_wait_for_the_turn_to_end_and_go_one_at_a_time() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, thread) = setup(dir.path());

    hub.send(thread, text("while idle"), None, true)
        .await
        .unwrap();
    assert!(waiting(&hub, thread).is_empty(), "queued while idle");
    assert_eq!(tried_to_send(&hub, thread).await, 1);

    turn(&hub, thread, "a", None);
    hub.send(thread, text("first"), None, true).await.unwrap();
    hub.send(thread, text("second"), None, true).await.unwrap();
    hub.send(thread, text("third"), None, true).await.unwrap();
    assert_eq!(waiting(&hub, thread), ["first", "second", "third"]);
    assert_eq!(tried_to_send(&hub, thread).await, 1, "sent mid-turn");

    let second = hub.queued(thread)[1].id.clone();
    assert!(hub.unqueue(thread, &second).is_some());
    assert!(hub.unqueue(thread, &second).is_none());

    turn(&hub, thread, "a", Some(TurnOutcome::Interrupted));
    assert_eq!(
        waiting(&hub, thread),
        ["first", "third"],
        "sent after a stop"
    );

    turn(&hub, thread, "b", None);
    turn(&hub, thread, "b", Some(TurnOutcome::Completed));
    assert_eq!(waiting(&hub, thread), ["third"]);
    assert_eq!(tried_to_send(&hub, thread).await, 2);

    turn(&hub, thread, "c", None);
    turn(&hub, thread, "c", Some(TurnOutcome::Completed));
    assert!(waiting(&hub, thread).is_empty());
    assert_eq!(tried_to_send(&hub, thread).await, 3);
}

#[tokio::test]
async fn a_queue_survives_a_hub_restart() {
    let dir = tempfile::tempdir().unwrap();
    let thread = {
        let (hub, thread) = setup(dir.path());
        turn(&hub, thread, "a", None);
        hub.send(thread, text("later"), None, true).await.unwrap();
        thread
    };
    let hub = corra_hub::build(corra_hub::Options {
        data: dir.path().to_path_buf(),
        public_url: "http://127.0.0.1:1".into(),
        dev_no_auth: false,
    })
    .unwrap()
    .state
    .hub;
    assert_eq!(waiting(&hub, thread), ["later"]);
}

#[tokio::test]
async fn a_message_sent_twice_goes_once_and_a_lost_turn_is_closed() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, thread) = setup(dir.path());

    turn(&hub, thread, "a", None);
    for _ in 0..3 {
        hub.send_once(thread, text("retry me"), None, true, Some("same-id".into()))
            .await
            .unwrap();
    }
    assert_eq!(waiting(&hub, thread), ["retry me"]);
    assert_eq!(hub.queued(thread)[0].id, "same-id");

    hub.append(
        thread,
        None,
        now_ms(),
        Event::Request {
            id: "perm-1".into(),
            turn: "a".into(),
            request: AgentRequest::ToolApproval {
                tool: "Bash".into(),
                input: serde_json::json!({}),
                reason: None,
                tool_call: None,
            },
        },
    )
    .unwrap();
    assert!(hub.close_open_turn(thread).unwrap());
    assert!(!hub.close_open_turn(thread).unwrap(), "closed twice");
    let events = hub.db.events_after(thread, 0, 100).unwrap();
    let n = events.len();
    assert!(matches!(
        &events[n - 2].event,
        Event::RequestResolved { id, response: RequestResponse::Cancelled } if id == "perm-1"
    ));
    assert!(matches!(
        &events[n - 1].event,
        Event::TurnCompleted { turn, outcome: TurnOutcome::Interrupted, .. } if turn == "a"
    ));
    assert_eq!(
        hub.db.thread(thread).unwrap().unwrap().status,
        ThreadStatus::Idle
    );
    assert_eq!(
        waiting(&hub, thread),
        ["retry me"],
        "a lost turn sent the queue"
    );
}

fn limited(hub: &HubRef, thread: ThreadId, id: &str, resets_at: i64) {
    turn(hub, thread, id, None);
    hub.append(
        thread,
        None,
        now_ms(),
        Event::TurnCompleted {
            turn: id.into(),
            outcome: TurnOutcome::Failed,
            usage: None,
            error: Some("You've hit your limit · resets 1pm (UTC)".into()),
            anchor: None,
            resets_at: Some(resets_at),
        },
    )
    .unwrap();
}

#[tokio::test]
async fn a_limit_continues_once_at_its_reset_then_the_queue_drains() {
    use corra_hub::schedule::{continue_limited, planned_continue};
    let dir = tempfile::tempdir().unwrap();
    let (hub, thread) = setup(dir.path());
    let now = now_ms();
    let reset = now + 10 * 60 * 1000;

    turn(&hub, thread, "a", None);
    hub.send(thread, text("later"), None, true).await.unwrap();
    hub.append(
        thread,
        None,
        now_ms(),
        Event::TurnCompleted {
            turn: "a".into(),
            outcome: TurnOutcome::Failed,
            usage: None,
            error: Some("You've hit your limit".into()),
            anchor: None,
            resets_at: Some(reset),
        },
    )
    .unwrap();
    assert_eq!(planned_continue(&hub, thread), Some(reset));
    assert_eq!(
        waiting(&hub, thread),
        ["later"],
        "a failed turn sent the queue"
    );

    continue_limited(&hub, reset).await.unwrap();
    assert_eq!(
        tried_to_send(&hub, thread).await,
        0,
        "continued before the reset"
    );
    continue_limited(&hub, reset + 61_000).await.unwrap();
    continue_limited(&hub, reset + 120_000).await.unwrap();
    assert_eq!(
        tried_to_send(&hub, thread).await,
        1,
        "continued other than once"
    );
    assert_eq!(planned_continue(&hub, thread), None);

    turn(&hub, thread, "b", None);
    turn(&hub, thread, "b", Some(TurnOutcome::Completed));
    assert!(waiting(&hub, thread).is_empty());
    assert_eq!(
        tried_to_send(&hub, thread).await,
        2,
        "the queue didn't drain"
    );
}

#[tokio::test]
async fn a_limit_doesnt_continue_a_trashed_waiting_or_resumed_thread_or_loop() {
    use corra_hub::schedule::{continue_limited, planned_continue};
    let dir = tempfile::tempdir().unwrap();
    let (hub, thread) = setup(dir.path());
    let reset = now_ms() + 60_000;
    let later = reset + 61_000;

    limited(&hub, thread, "a", reset);
    turn(&hub, thread, "b", None);
    assert_eq!(
        planned_continue(&hub, thread),
        None,
        "a new turn kept the plan"
    );
    turn(&hub, thread, "b", Some(TurnOutcome::Completed));

    limited(&hub, thread, "c", reset);
    hub.set_thread_status(thread, ThreadStatus::Waiting)
        .unwrap();
    continue_limited(&hub, later).await.unwrap();
    assert_eq!(
        tried_to_send(&hub, thread).await,
        0,
        "continued a thread waiting on me"
    );
    hub.set_thread_status(thread, ThreadStatus::Error).unwrap();

    limited(&hub, thread, "d", reset);
    let mut t = hub.db.thread(thread).unwrap().unwrap();
    t.hidden_at = Some(now_ms());
    hub.db.update_thread(&t).unwrap();
    continue_limited(&hub, later).await.unwrap();
    assert_eq!(
        tried_to_send(&hub, thread).await,
        0,
        "continued a trashed thread"
    );
    t.hidden_at = None;
    hub.db.update_thread(&t).unwrap();

    limited(&hub, thread, "e", reset);
    continue_limited(&hub, later).await.unwrap();
    limited(&hub, thread, "f", reset);
    continue_limited(&hub, later).await.unwrap();
    assert_eq!(tried_to_send(&hub, thread).await, 2);
    limited(&hub, thread, "g", reset);
    assert_eq!(
        planned_continue(&hub, thread),
        None,
        "kept continuing into the limit"
    );
}

fn started(hub: &HubRef, thread: ThreadId, id: &str, input: &str) {
    hub.append(
        thread,
        None,
        now_ms(),
        Event::TurnStarted {
            turn: id.into(),
            input: text(input),
            settings: AgentSettings::default(),
        },
    )
    .unwrap();
}

#[tokio::test]
async fn compacting_first_sends_the_message_after_the_summary_and_tells_no_follower() {
    use corra_hub::schedule::compact_then_send;
    let dir = tempfile::tempdir().unwrap();
    let (hub, thread) = setup(dir.path());
    let follower = {
        let mut t = hub.db.thread(thread).unwrap().unwrap();
        t.id = new_id();
        hub.db.insert_thread(&t).unwrap();
        t.id
    };
    hub.set_thread_kv(thread, "follower", &follower.to_string());

    compact_then_send(&hub, thread, text("my question"), None, Some("c1".into()))
        .await
        .unwrap();
    compact_then_send(&hub, thread, text("my question"), None, Some("c1".into()))
        .await
        .unwrap();
    assert_eq!(waiting(&hub, thread), ["my question"]);
    assert_eq!(
        tried_to_send(&hub, thread).await,
        1,
        "the summary wasn't asked for once"
    );
    let told = tried_to_send(&hub, follower).await;

    started(&hub, thread, "c", "/compact");
    turn(&hub, thread, "c", Some(TurnOutcome::Completed));
    assert!(waiting(&hub, thread).is_empty());
    assert_eq!(
        tried_to_send(&hub, thread).await,
        2,
        "the message didn't follow"
    );
    assert_eq!(
        tried_to_send(&hub, follower).await,
        told + 1,
        "the follower heard about the summary, not just the failed send"
    );

    started(&hub, thread, "m", "my question");
    turn(&hub, thread, "m", Some(TurnOutcome::Completed));
    assert_eq!(
        tried_to_send(&hub, follower).await,
        told + 2,
        "the follower missed a real turn"
    );
}

#[tokio::test]
async fn an_idle_thread_compacts_once_before_its_cache_expires() {
    use corra_hub::schedule::{compact_idle, note_cache_use};
    let dir = tempfile::tempdir().unwrap();
    let (hub, thread) = setup(dir.path());
    let small = {
        let mut t = hub.db.thread(thread).unwrap().unwrap();
        t.id = new_id();
        hub.db.insert_thread(&t).unwrap();
        t.id
    };
    let minutes = |m: i64| now_ms() + m * 60 * 1000;
    hub.db
        .kv_set(
            "settings",
            r#"{"compact_idle":"return","compact_over":"100000"}"#,
        )
        .unwrap();
    note_cache_use(&hub, thread, 150_000, now_ms());
    note_cache_use(&hub, small, 90_000, now_ms());
    compact_idle(&hub, minutes(52)).await.unwrap();
    assert_eq!(
        tried_to_send(&hub, thread).await,
        0,
        "compacted when set to wait for me"
    );

    hub.db
        .kv_set(
            "settings",
            r#"{"compact_idle":"before","compact_over":"100000"}"#,
        )
        .unwrap();
    compact_idle(&hub, minutes(30)).await.unwrap();
    assert_eq!(
        tried_to_send(&hub, thread).await,
        0,
        "compacted while the cache was fresh"
    );
    compact_idle(&hub, minutes(52)).await.unwrap();
    compact_idle(&hub, minutes(53)).await.unwrap();
    assert_eq!(
        tried_to_send(&hub, thread).await,
        1,
        "compacted other than once"
    );
    assert_eq!(
        tried_to_send(&hub, small).await,
        0,
        "compacted a small thread"
    );

    hub.set_thread_status(thread, ThreadStatus::Idle).unwrap();
    note_cache_use(&hub, thread, 150_000, now_ms());
    compact_idle(&hub, minutes(65)).await.unwrap();
    assert_eq!(
        tried_to_send(&hub, thread).await,
        1,
        "compacted after the cache expired"
    );
}
