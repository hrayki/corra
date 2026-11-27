use std::time::Duration;

use corra_hub::hub::{HubRef, RunnerConn};
use corra_proto::runner::HubMsg;
use corra_proto::*;
use tokio::sync::mpsc;

fn setup(dir: &std::path::Path) -> (HubRef, ThreadId, MachineId) {
    let hub = open(dir);
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
        status: ThreadStatus::Running,
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

fn open(dir: &std::path::Path) -> HubRef {
    corra_hub::build(corra_hub::Options {
        data: dir.to_path_buf(),
        public_url: "http://127.0.0.1:1".into(),
        dev_no_auth: false,
    })
    .unwrap()
    .state
    .hub
}

fn connect(
    hub: &HubRef,
    machine: MachineId,
    live: &[ThreadId],
    takes: bool,
) -> mpsc::UnboundedReceiver<HubMsg> {
    let (tx, rx) = mpsc::unbounded_channel();
    hub.runners.lock().unwrap().insert(
        machine,
        RunnerConn {
            tx,
            live: live.iter().copied().collect(),
            prepared: Default::default(),
            conn_id: 1,
            takes,
            prompt: false,
        },
    );
    rx
}

async fn got(rx: &mut mpsc::UnboundedReceiver<HubMsg>) -> HubMsg {
    tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("nothing reached the runner")
        .unwrap()
}

fn delivery_of(m: &HubMsg) -> Option<String> {
    match m {
        HubMsg::Send { delivery, .. }
        | HubMsg::Start { delivery, .. }
        | HubMsg::Prompt { delivery, .. } => delivery.clone(),
        _ => None,
    }
}

fn text(s: &str) -> Vec<InputPart> {
    vec![InputPart::Text { text: s.into() }]
}

#[tokio::test]
async fn a_message_the_runner_hasnt_taken_goes_again_when_it_reconnects() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, thread, machine) = setup(dir.path());
    let mut rx = connect(&hub, machine, &[thread], true);
    hub.send(thread, text("hello"), None, false).await.unwrap();
    let first = got(&mut rx).await;
    assert!(matches!(first, HubMsg::Send { .. }), "{first:?}");
    let id = delivery_of(&first).expect("no delivery id");
    assert!(hub.delivering(thread));

    let mut rx = connect(&hub, machine, &[thread], true);
    hub.redeliver_to(machine);
    let again = got(&mut rx).await;
    assert_eq!(delivery_of(&again).as_deref(), Some(id.as_str()));

    hub.took(machine, id, true);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(hub.deliveries().is_empty());
    assert!(!hub.delivering(thread));
}

#[tokio::test]
async fn a_message_waiting_for_its_runner_survives_a_hub_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (thread, machine, id) = {
        let (hub, thread, machine) = setup(dir.path());
        let mut rx = connect(&hub, machine, &[thread], true);
        hub.send(thread, text("hello"), None, false).await.unwrap();
        let id = delivery_of(&got(&mut rx).await).unwrap();
        (thread, machine, id)
    };
    let hub = open(dir.path());
    assert!(hub.delivering(thread));
    let mut rx = connect(&hub, machine, &[thread], true);
    hub.redeliver_to(machine);
    let again = got(&mut rx).await;
    assert_eq!(delivery_of(&again), Some(id));
    assert!(matches!(again, HubMsg::Send { input, .. } if input == text("hello")));
}

#[tokio::test]
async fn a_runner_without_the_session_gets_the_message_as_a_fresh_start() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, thread, machine) = setup(dir.path());
    let mut rx = connect(&hub, machine, &[thread], true);
    hub.send(thread, text("hello"), None, false).await.unwrap();
    let id = delivery_of(&got(&mut rx).await).unwrap();
    hub.took(machine, id.clone(), false);
    let again = got(&mut rx).await;
    assert!(matches!(again, HubMsg::Start { .. }), "{again:?}");
    assert_eq!(delivery_of(&again), Some(id));
}

#[tokio::test]
async fn an_older_runner_gets_no_delivery_ids_and_nothing_is_kept_for_it() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, thread, machine) = setup(dir.path());
    let mut rx = connect(&hub, machine, &[thread], false);
    hub.send(thread, text("hello"), None, false).await.unwrap();
    let m = got(&mut rx).await;
    assert_eq!(delivery_of(&m), None);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(hub.deliveries().is_empty());
}

#[tokio::test]
async fn a_prompt_capable_runner_always_gets_idempotent_prompt_and_keeps_moved_from_until_took() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, thread, machine) = setup(dir.path());
    hub.set_thread_kv(thread, "moved_from", "~/old");
    hub.set_thread_kv(thread, "resume_at", "msg-1");
    let (tx, mut rx) = mpsc::unbounded_channel();
    hub.runners.lock().unwrap().insert(
        machine,
        RunnerConn {
            tx,
            live: [thread].into_iter().collect(),
            prepared: Default::default(),
            conn_id: 1,
            takes: true,
            prompt: true,
        },
    );
    hub.send(thread, text("hello"), None, false).await.unwrap();
    let first = got(&mut rx).await;
    let id = match &first {
        HubMsg::Prompt { spec, delivery, .. } => {
            assert_eq!(spec.moved_from.as_deref(), Some("~/old"));
            assert_eq!(spec.resume_at.as_deref(), Some("msg-1"));
            delivery.clone().expect("delivery id")
        }
        other => panic!("expected HubMsg::Prompt, got {other:?}"),
    };
    // Before Took, moved_from and resume_at remain persisted for redelivery!
    assert_eq!(
        hub.thread_kv(thread, "moved_from").as_deref(),
        Some("~/old")
    );
    assert_eq!(hub.thread_kv(thread, "resume_at").as_deref(), Some("msg-1"));

    hub.took(machine, id, true);
    assert!(hub.thread_kv(thread, "moved_from").is_none());
    assert!(hub.thread_kv(thread, "resume_at").is_none());
}

#[tokio::test]
async fn stop_session_awaits_runner_stop_request_without_sleeping() {
    use corra_proto::runner::RunnerRequest;
    let dir = tempfile::tempdir().unwrap();
    let (hub, thread, machine) = setup(dir.path());
    let (tx, mut rx) = mpsc::unbounded_channel();
    hub.runners.lock().unwrap().insert(
        machine,
        RunnerConn {
            tx,
            live: [thread].into_iter().collect(),
            prepared: Default::default(),
            conn_id: 1,
            takes: true,
            prompt: true,
        },
    );
    let hub2 = hub.clone();
    let responder = tokio::spawn(async move {
        if let Some(HubMsg::Request {
            id,
            body: RunnerRequest::Stop { thread: t },
        }) = rx.recv().await
        {
            assert_eq!(t, thread);
            hub2.resolve_pending(machine, id, Ok(serde_json::json!({})));
        }
    });
    let began = std::time::Instant::now();
    hub.stop_session(machine, thread).await;
    responder.await.unwrap();
    assert!(began.elapsed() < Duration::from_secs(1));
}
