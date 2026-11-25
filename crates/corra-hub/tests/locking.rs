use std::time::Duration;

use corra_hub::hub::{HubRef, NewProject};
use corra_proto::*;
use serde_json::json;

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

fn thread(hub: &HubRef) -> Thread {
    let p = hub
        .create_project(NewProject {
            name: "p".into(),
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
    t
}

fn hold_write_lock(dir: &std::path::Path, for_ms: u64) -> std::thread::JoinHandle<()> {
    let other = rusqlite::Connection::open(dir.join("corra.db")).unwrap();
    other.execute_batch("BEGIN IMMEDIATE").unwrap();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(for_ms));
        other.execute_batch("ROLLBACK").unwrap();
    })
}

#[tokio::test]
async fn writes_wait_while_the_backup_process_holds_the_write_lock() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub(dir.path());
    let t = thread(&hub);
    let event: Event =
        serde_json::from_value(json!({ "kind": "notice", "level": "info", "text": "hi" })).unwrap();

    let held = hold_write_lock(dir.path(), 300);
    assert!(
        hub.db
            .append_event(t.id, Some(1), 1, &event)
            .unwrap()
            .is_some()
    );
    held.join().unwrap();

    let held = hold_write_lock(dir.path(), 300);
    assert_eq!(hub.db.file_append(t.id, "log", 0, b"ab").unwrap(), Ok(2));
    held.join().unwrap();

    let held = hold_write_lock(dir.path(), 300);
    hub.db.file_replace(t.id, "log", b"c").unwrap();
    held.join().unwrap();

    assert_eq!(hub.db.thread(t.id).unwrap().unwrap().last_seq, 1);
    assert_eq!(
        hub.db.file_read(t.id, "log").unwrap().as_deref(),
        Some(&b"c"[..])
    );
}
