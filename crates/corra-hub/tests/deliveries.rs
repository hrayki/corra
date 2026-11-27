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
        HubMsg::Send { delivery, .. } | HubMsg::Start { delivery, .. } => delivery.clone(),
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
