use corra_hub::db::NewMachine;
use corra_hub::hub::{HubRef, ImportProject, ImportThread, NewProject};
use corra_proto::*;

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

fn machine(hub: &HubRef, name: &str) -> MachineId {
    hub.db
        .insert_machine(NewMachine {
            name: name.into(),
            provider: "existing".into(),
            project: None,
            size: "existing".into(),
            token_hash: new_id().to_string(),
            provider_ref: None,
            status: MachineStatus::Connected,
        })
        .unwrap()
        .id
}

fn thread(title: &str, status: ThreadStatus) -> Thread {
    Thread {
        id: new_id(),
        project: new_id(),
        title: title.into(),
        agent: AgentKind::Claude,
        machine: Some(new_id()),
        session_id: Some("s".into()),
        cwd: "/home/me/notes".into(),
        settings: AgentSettings::default(),
        status,
        last_seq: 0,
        created_at: 1,
        updated_at: 1,
        pinned_at: None,
        settled_at: None,
        hidden_at: None,
    }
}

fn events(n: i64) -> Vec<StoredEvent> {
    (1..=n)
        .map(|seq| StoredEvent {
            seq,
            ts: seq,
            event: Event::Notice {
                level: NoticeLevel::Info,
                text: format!("event {seq}"),
            },
        })
        .collect()
}

fn item(t: &Thread, name: &str, path: Option<&str>, n: i64) -> ImportThread {
    ImportThread {
        thread: t.clone(),
        project: ImportProject {
            name: name.into(),
            path: path.map(String::from),
            settings: AgentSettings::default(),
        },
        events: events(n),
    }
}

#[tokio::test]
async fn a_hand_over_lands_in_the_right_project_once_and_fills_gaps() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub(dir.path());
    let laptop = machine(&hub, "laptop");
    let other = machine(&hub, "box");
    let notes = hub
        .create_project(NewProject {
            name: "notes".into(),
            path: Some("/home/me/notes".into()),
            machine: Some(laptop),
            provider: Some("existing".into()),
            ..Default::default()
        })
        .unwrap();

    let t = thread("written offline", ThreadStatus::Running);
    hub.import_threads(
        laptop,
        vec![item(&t, "renamed-offline", Some("/home/me/notes"), 3)],
    )
    .unwrap();
    let got = hub.db.thread(t.id).unwrap().unwrap();
    assert_eq!(got.project, notes.id, "matched by folder, not by name");
    assert_eq!(got.machine, Some(laptop));
    assert_eq!(
        got.status,
        ThreadStatus::Idle,
        "a turn the hub can't see stayed open"
    );
    assert_eq!(hub.db.events_after(t.id, 0, 100).unwrap().len(), 3);

    hub.import_threads(laptop, vec![item(&t, "notes", Some("/home/me/notes"), 5)])
        .unwrap();
    let all = hub.db.events_after(t.id, 0, 100).unwrap();
    assert_eq!(
        all.iter().map(|e| e.seq).collect::<Vec<_>>(),
        [1, 2, 3, 4, 5],
        "a second hand-over doubled or lost events"
    );

    let fresh = thread("in a new folder", ThreadStatus::Idle);
    hub.import_threads(
        laptop,
        vec![item(&fresh, "notes", Some("/home/me/scratch"), 1)],
    )
    .unwrap();
    let p = hub
        .db
        .project(hub.db.thread(fresh.id).unwrap().unwrap().project)
        .unwrap()
        .unwrap();
    assert_eq!(p.path.as_deref(), Some("/home/me/scratch"));
    assert_eq!(p.default_machine, Some(laptop));
    assert_eq!(p.provider, "existing");
    assert_ne!(p.name, "notes", "two projects got the same name");

    let theirs = thread("on another machine", ThreadStatus::Idle);
    hub.import_threads(other, vec![item(&theirs, "x", None, 1)])
        .unwrap();
    hub.import_threads(laptop, vec![item(&theirs, "x", None, 4)])
        .unwrap();
    assert_eq!(
        hub.db.thread(theirs.id).unwrap().unwrap().machine,
        Some(other),
        "one machine took over another's thread"
    );
    assert_eq!(hub.db.events_after(theirs.id, 0, 100).unwrap().len(), 1);
}
