use corra_hub::hub::HubRef;
use corra_proto::*;
use serde_json::json;

const DAY: i64 = 86_400_000;

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

fn thread(hub: &HubRef, project: ProjectId, hidden_days_ago: Option<i64>, now: i64) -> ThreadId {
    let t = Thread {
        id: new_id(),
        project,
        title: "t".into(),
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
        hidden_at: hidden_days_ago.map(|d| now - d * DAY),
    };
    hub.db.insert_thread(&t).unwrap();
    hub.db.update_thread(&t).unwrap();
    hub.append(
        t.id,
        None,
        now,
        Event::Notice {
            level: NoticeLevel::Info,
            text: "x".into(),
        },
    )
    .unwrap();
    hub.db.file_replace(t.id, "session.jsonl", b"{}").unwrap();
    hub.set_thread_kv(t.id, "moved_from", "~/old");
    t.id
}

fn gone(hub: &HubRef, t: ThreadId) -> bool {
    hub.db.thread(t).unwrap().is_none()
        && hub.db.events_after(t, 0, 10).unwrap().is_empty()
        && hub.db.file_sizes(t).unwrap().is_empty()
        && hub.thread_kv(t, "moved_from").is_none()
}

#[tokio::test]
async fn old_hidden_threads_go_with_their_files_and_the_rest_stay() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub(dir.path());
    let p = hub
        .create_project(corra_hub::hub::NewProject {
            name: "p".into(),
            provider: Some("existing".into()),
            ..Default::default()
        })
        .unwrap()
        .id;
    let now = now_ms();
    let old = thread(&hub, p, Some(40), now);
    let recent = thread(&hub, p, Some(5), now);
    let visible = thread(&hub, p, None, now);
    let parent = thread(&hub, p, Some(40), now);
    let fork = thread(&hub, p, None, now);
    let mut f = hub.db.thread(fork).unwrap().unwrap();
    f.session_id = None;
    hub.db.update_thread(&f).unwrap();
    hub.set_thread_kv(
        fork,
        "fork_of",
        &json!({ "session": "s", "thread": parent }).to_string(),
    );

    assert_eq!(hub.sweep_hidden(now).unwrap(), 0, "swept with no setting");

    hub.db
        .kv_set("settings", &json!({ "retention_days": "30" }).to_string())
        .unwrap();
    assert_eq!(hub.sweep_hidden(now).unwrap(), 1);
    assert!(gone(&hub, old));
    for t in [recent, visible, parent, fork] {
        assert!(hub.db.thread(t).unwrap().is_some());
        assert!(!hub.db.file_sizes(t).unwrap().is_empty());
    }

    let mut f = hub.db.thread(fork).unwrap().unwrap();
    f.session_id = Some("forked".into());
    hub.db.update_thread(&f).unwrap();
    assert_eq!(hub.sweep_hidden(now).unwrap(), 1);
    assert!(gone(&hub, parent));
}
