use std::time::Duration;

use corra_hub::hub::{HubRef, RunnerConn};
use corra_proto::*;

fn text(s: &str) -> Vec<InputPart> {
    vec![InputPart::Text { text: s.into() }]
}

fn setup(dir: &std::path::Path) -> (HubRef, ThreadId, MachineId) {
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
    let machine = new_id();
    let t = Thread {
        id: new_id(),
        project: p.id,
        title: "t".into(),
        agent: AgentKind::Claude,
        machine: Some(machine),
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
    (hub, t.id, machine)
}

fn connect(
    hub: &HubRef,
    machine: MachineId,
    live: &[ThreadId],
) -> tokio::sync::mpsc::UnboundedReceiver<corra_proto::runner::HubMsg> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    hub.runners.lock().unwrap().insert(
        machine,
        RunnerConn {
            tx,
            live: live.iter().copied().collect(),
            prepared: Default::default(),
            conn_id: 1,
            takes: false,
        },
    );
    rx
}

fn started(hub: &HubRef, thread: ThreadId, turn: &str) {
    hub.append(
        thread,
        None,
        now_ms(),
        Event::TurnStarted {
            turn: turn.into(),
            input: text("go"),
            settings: AgentSettings::default(),
            cause: None,
        },
    )
    .unwrap();
}

fn status(hub: &HubRef, thread: ThreadId) -> ThreadStatus {
    hub.db.thread(thread).unwrap().unwrap().status
}

fn later() -> i64 {
    now_ms() + 120_000
}

fn last_outcome(hub: &HubRef, thread: ThreadId) -> Option<TurnOutcome> {
    hub.db.last_outcome(thread).unwrap()
}

fn notices(hub: &HubRef, thread: ThreadId) -> usize {
    hub.db
        .events_after(thread, 0, 1000)
        .unwrap()
        .iter()
        .filter(|e| matches!(e.event, Event::Notice { .. }))
        .count()
}

#[tokio::test]
async fn a_turn_whose_session_is_gone_is_ended_once_it_has_been_quiet_a_minute() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, thread, machine) = setup(dir.path());
    let _rx = connect(&hub, machine, &[]);
    started(&hub, thread, "a");

    hub.settle_strays(now_ms()).unwrap();
    assert_eq!(
        status(&hub, thread),
        ThreadStatus::Running,
        "ended too soon"
    );

    hub.settle_strays(later()).unwrap();
    assert_eq!(status(&hub, thread), ThreadStatus::Idle);
    assert_eq!(last_outcome(&hub, thread), Some(TurnOutcome::Interrupted));
    assert_eq!(notices(&hub, thread), 1);
}

#[tokio::test]
async fn a_live_session_keeps_its_turn_however_quiet() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, thread, machine) = setup(dir.path());
    let _rx = connect(&hub, machine, &[thread]);
    started(&hub, thread, "a");
    hub.settle_strays(later()).unwrap();
    assert_eq!(status(&hub, thread), ThreadStatus::Running);
    assert!(hub.db.open_turn(thread).unwrap().is_some());
}

#[tokio::test]
async fn a_push_waiting_between_turns_keeps_the_thread_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, thread, machine) = setup(dir.path());
    let _rx = connect(&hub, machine, &[]);
    started(&hub, thread, "a");
    hub.append(
        thread,
        None,
        now_ms(),
        Event::TurnCompleted {
            turn: "a".into(),
            outcome: TurnOutcome::Completed,
            usage: None,
            error: None,
            anchor: None,
            resets_at: None,
        },
    )
    .unwrap();
    let asking = {
        let hub = hub.clone();
        tokio::spawn(async move {
            hub.ask(thread, "a", "git.push", &serde_json::json!({}))
                .await
                .unwrap()
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(status(&hub, thread), ThreadStatus::Waiting);

    hub.settle_strays(later()).unwrap();
    hub.runner_status(thread, ThreadStatus::Idle).unwrap();
    assert_eq!(status(&hub, thread), ThreadStatus::Waiting);
    assert_eq!(hub.db.open_requests(thread).unwrap().len(), 1);
    assert!(!asking.is_finished());
    assert_eq!(notices(&hub, thread), 0);
}

#[tokio::test]
async fn a_runner_that_says_idle_ends_a_turn_the_hub_still_shows_open() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, thread, machine) = setup(dir.path());
    let _rx = connect(&hub, machine, &[thread]);
    started(&hub, thread, "a");
    hub.append(
        thread,
        Some(1),
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
    hub.runner_status(thread, ThreadStatus::Idle).unwrap();
    assert_eq!(status(&hub, thread), ThreadStatus::Idle);
    assert!(hub.db.open_turn(thread).unwrap().is_none());
    assert!(hub.db.open_requests(thread).unwrap().is_empty());
    assert_eq!(notices(&hub, thread), 1);
}

#[tokio::test]
async fn an_idle_thread_with_a_queue_sends_the_next_message() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, thread, machine) = setup(dir.path());
    let mut rx = connect(&hub, machine, &[thread]);
    started(&hub, thread, "a");
    hub.send(thread, text("waiting"), None, true).await.unwrap();
    hub.set_thread_status(thread, ThreadStatus::Idle).unwrap();

    hub.settle_strays(later()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("nothing went to the machine")
        .unwrap();
    assert!(hub.queued(thread).is_empty());
}

#[tokio::test]
async fn a_thread_left_starting_with_nothing_on_the_way_goes_idle() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, thread, _machine) = setup(dir.path());
    hub.set_thread_status(thread, ThreadStatus::Starting)
        .unwrap();
    hub.settle_strays(later()).unwrap();
    assert_eq!(status(&hub, thread), ThreadStatus::Idle);
}
