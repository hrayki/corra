use corra_hub::hub::{HubRef, RunnerConn};
use corra_proto::runner::HubMsg;
use corra_proto::*;
use tokio::sync::mpsc;

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
        status: ThreadStatus::Waiting,
        last_seq: 0,
        created_at: 0,
        updated_at: 0,
        pinned_at: None,
        settled_at: None,
        hidden_at: None,
    };
    hub.db.insert_thread(&t).unwrap();
    (hub, t.id, machine)
}

fn connect(hub: &HubRef, machine: MachineId) -> mpsc::UnboundedReceiver<HubMsg> {
    let (tx, rx) = mpsc::unbounded_channel();
    hub.runners.lock().unwrap().insert(
        machine,
        RunnerConn {
            tx,
            live: Default::default(),
            prepared: Default::default(),
            conn_id: 1,
        },
    );
    rx
}

fn ask(hub: &HubRef, thread: ThreadId, id: &str) {
    hub.append(
        thread,
        None,
        now_ms(),
        Event::Request {
            id: id.into(),
            turn: "a".into(),
            request: AgentRequest::ToolApproval {
                tool: "Bash".into(),
                input: serde_json::json!({ "command": "ls" }),
                reason: None,
                tool_call: None,
            },
        },
    )
    .unwrap();
}

fn allow() -> RequestResponse {
    RequestResponse::Allow { always: false }
}

fn forwarded(rx: &mut mpsc::UnboundedReceiver<HubMsg>) -> usize {
    std::iter::from_fn(|| rx.try_recv().ok())
        .filter(|m| matches!(m, HubMsg::Respond { .. }))
        .count()
}

#[tokio::test]
async fn an_answer_goes_once_and_only_to_a_request_still_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, thread, machine) = setup(dir.path());
    let mut runner = connect(&hub, machine);

    ask(&hub, thread, "perm-1");
    hub.respond(thread, "perm-1".into(), allow()).unwrap();
    let again = hub.respond(
        thread,
        "perm-1".into(),
        RequestResponse::Deny { message: None },
    );
    assert!(again.is_err(), "a second answer went through");
    assert_eq!(forwarded(&mut runner), 1);

    assert!(hub.respond(thread, "never-asked".into(), allow()).is_err());

    ask(&hub, thread, "perm-2");
    hub.append(
        thread,
        None,
        now_ms(),
        Event::RequestResolved {
            id: "perm-2".into(),
            response: RequestResponse::Cancelled,
        },
    )
    .unwrap();
    let late = hub.respond(thread, "perm-2".into(), allow()).unwrap_err();
    assert_eq!(late.to_string(), "That request is no longer waiting.");
    assert_eq!(forwarded(&mut runner), 0);

    hub.runners.lock().unwrap().remove(&machine);
    ask(&hub, thread, "perm-3");
    assert!(hub.respond(thread, "perm-3".into(), allow()).is_err());
    let mut runner = connect(&hub, machine);
    hub.respond(thread, "perm-3".into(), allow())
        .expect("an answer that never left couldn't be sent again");
    assert_eq!(forwarded(&mut runner), 1);
}
