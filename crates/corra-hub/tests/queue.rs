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
        started_by: None,
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
            cause: None,
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

    turn(&hub, thread, "a", Some(TurnOutcome::Completed));
    assert_eq!(waiting(&hub, thread), ["third"]);
    assert_eq!(tried_to_send(&hub, thread).await, 2);

    turn(&hub, thread, "b", None);
    turn(&hub, thread, "b", Some(TurnOutcome::Interrupted));
    assert!(waiting(&hub, thread).is_empty(), "kept after a stop");
    assert_eq!(tried_to_send(&hub, thread).await, 3);
}

#[tokio::test]
async fn a_failed_turn_holds_the_queue_until_a_turn_completes() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, thread) = setup(dir.path());

    turn(&hub, thread, "a", None);
    hub.send(thread, text("first"), None, true).await.unwrap();
    hub.send(thread, text("second"), None, true).await.unwrap();
    turn(&hub, thread, "a", Some(TurnOutcome::Failed));
    assert_eq!(waiting(&hub, thread), ["first", "second"]);
    assert_eq!(tried_to_send(&hub, thread).await, 0, "sent after a failure");

    turn(&hub, thread, "b", None);
    turn(&hub, thread, "b", Some(TurnOutcome::Completed));
    assert_eq!(waiting(&hub, thread), ["second"]);
    assert_eq!(tried_to_send(&hub, thread).await, 1);
}

#[tokio::test]
async fn a_stop_hands_the_queue_back_and_nothing_is_sent_after_it() {
    use corra_hub::hub::RunnerConn;
    use corra_proto::runner::HubMsg;
    let dir = tempfile::tempdir().unwrap();
    let (hub, thread) = setup(dir.path());
    let machine = new_id();
    let mut t = hub.db.thread(thread).unwrap().unwrap();
    t.machine = Some(machine);
    hub.db.update_thread(&t).unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    hub.runners.lock().unwrap().insert(
        machine,
        RunnerConn {
            tx,
            live: Default::default(),
            prepared: Default::default(),
            conn_id: 1,
            takes: false,
            prompt: false,
        },
    );

    turn(&hub, thread, "a", None);
    hub.send(thread, text("first"), None, true).await.unwrap();
    hub.send(thread, text("second"), None, true).await.unwrap();
    let unsent = hub.interrupt(thread).unwrap();
    assert_eq!(
        unsent.iter().map(|q| q.input.clone()).collect::<Vec<_>>(),
        [text("first"), text("second")]
    );
    assert!(matches!(rx.try_recv(), Ok(HubMsg::Interrupt { .. })));
    assert!(waiting(&hub, thread).is_empty());

    turn(&hub, thread, "a", Some(TurnOutcome::Interrupted));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(rx.try_recv().is_err(), "something was sent after the stop");
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
    assert!(
        waiting(&hub, thread).is_empty(),
        "a lost turn kept the queue"
    );
    assert_eq!(tried_to_send(&hub, thread).await, 1);
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
            r#"{"compact_idle":"never","compact_over":"100000"}"#,
        )
        .unwrap();
    note_cache_use(&hub, thread, 150_000, now_ms());
    note_cache_use(&hub, small, 90_000, now_ms());
    compact_idle(&hub, minutes(52)).await.unwrap();
    assert_eq!(
        tried_to_send(&hub, thread).await,
        0,
        "compacted when disabled"
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

    // 5-minute API-key TTL uses a 90s lead window (3m30s..4m45s) and marks offline machines compact_woke
    hub.secrets.set("anthropic.api_key", "sk-ant-test").unwrap();
    let offline_machine = new_id();
    let mut t = hub.db.thread(thread).unwrap().unwrap();
    t.machine = Some(offline_machine);
    t.status = ThreadStatus::Idle;
    hub.db.update_thread(&t).unwrap();
    let t0 = now_ms();
    note_cache_use(&hub, thread, 150_000, t0);
    compact_idle(&hub, t0 + 180_000).await.unwrap();
    assert_eq!(tried_to_send(&hub, thread).await, 1, "compacted at 3m00s");
    compact_idle(&hub, t0 + 225_000).await.unwrap();
    assert_eq!(
        tried_to_send(&hub, thread).await,
        2,
        "didn't compact inside 5m TTL 3m30s..4m45s window"
    );
    assert!(hub.compact_woke.lock().unwrap().contains(&offline_machine));
    // Real user message clears compact_woke
    hub.send(thread, text("real prompt"), None, false)
        .await
        .unwrap();
    assert!(!hub.compact_woke.lock().unwrap().contains(&offline_machine));
}

#[tokio::test]
async fn a_send_says_whether_it_queued_and_its_id_outlives_a_hub_restart() {
    let dir = tempfile::tempdir().unwrap();
    let thread = {
        let (hub, thread) = setup(dir.path());
        assert!(
            !hub.send_once(thread, text("now"), None, true, Some("idle-id".into()))
                .await
                .unwrap()
        );
        turn(&hub, thread, "a", None);
        assert!(
            hub.send_once(thread, text("later"), None, true, Some("busy-id".into()))
                .await
                .unwrap()
        );
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
    assert!(
        !hub.send_once(thread, text("now"), None, false, Some("idle-id".into()))
            .await
            .unwrap()
    );
    assert!(
        hub.send_once(thread, text("later"), None, true, Some("busy-id".into()))
            .await
            .unwrap()
    );
    assert_eq!(waiting(&hub, thread), ["later"]);
    assert_eq!(tried_to_send(&hub, thread).await, 1, "sent twice");
}

#[tokio::test]
async fn a_send_that_cant_be_delivered_tells_the_app_which_one() {
    use corra_hub::hub::Outbound;
    let dir = tempfile::tempdir().unwrap();
    let (hub, thread) = setup(dir.path());
    let mut apps = hub.apps.subscribe();
    hub.send_once(thread, text("hello"), None, false, Some("mine".into()))
        .await
        .unwrap();
    let failed = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Outbound::Thread(t, corra_proto::app::HubToApp::NotSent { client_id, .. }) =
                &*apps.recv().await.unwrap()
                && *t == thread
            {
                return client_id.clone();
            }
        }
    })
    .await
    .expect("the app was never told");
    assert_eq!(failed, "mine");
}

#[tokio::test]
async fn a_long_turn_is_closed_and_its_old_request_answered_however_many_events_followed() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, thread) = setup(dir.path());
    turn(&hub, thread, "a", None);
    let asking = {
        let hub = hub.clone();
        tokio::spawn(async move {
            hub.ask(thread, "a", "github.pr_merge", &serde_json::json!({}))
                .await
                .unwrap()
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    let ask = hub.db.open_requests(thread).unwrap();
    let Event::Request { id, .. } = &ask[0].event else {
        panic!()
    };
    for i in 0..2100 {
        hub.append(
            thread,
            None,
            now_ms(),
            Event::ItemCompleted {
                turn: "a".into(),
                item: Item {
                    id: format!("i{i}"),
                    parent: None,
                    body: ItemBody::AgentMessage { text: ".".into() },
                },
            },
        )
        .unwrap();
    }
    hub.respond(thread, id.clone(), RequestResponse::Allow { always: false })
        .unwrap();
    assert!(matches!(
        asking.await.unwrap(),
        RequestResponse::Allow { .. }
    ));
    assert!(
        hub.close_open_turn(thread).unwrap(),
        "the turn was left open"
    );
    assert!(hub.db.open_turn(thread).unwrap().is_none());
}
