use corra_proto::*;

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let from = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/old");
    for f in ["corra-v12.db", "master.key"] {
        let to = if f == "corra-v12.db" { "corra.db" } else { f };
        std::fs::copy(from.join(f), dir.path().join(to)).unwrap();
    }
    dir
}

fn open(dir: &std::path::Path) -> anyhow::Result<corra_hub::hub::HubRef> {
    Ok(corra_hub::build(corra_hub::Options {
        data: dir.to_path_buf(),
        public_url: "http://127.0.0.1:1".into(),
        dev_no_auth: false,
    })?
    .state
    .hub)
}

#[tokio::test]
async fn a_database_made_by_the_old_hub_opens_and_keeps_working() {
    let dir = fixture();
    let hub = open(dir.path()).unwrap();

    let p = hub.db.project_by_name("old").unwrap().unwrap();
    assert_eq!(p.repos[0].github.as_deref(), Some("o/r"));
    assert_eq!(p.setup.as_deref(), Some("corra"));
    assert_eq!(p.env, ["FOO"]);
    assert!(p.allow_main && p.trusted);
    assert_eq!(p.settings.permission_mode.as_deref(), Some("acceptEdits"));

    let threads = hub.db.threads().unwrap();
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].title, "hello from the old hub");
    assert_eq!(threads[0].status, ThreadStatus::Error);
    let events = hub.db.events_after(threads[0].id, 0, 10).unwrap();
    assert!(matches!(
        &events[0].event,
        Event::Notice { text, .. } if text == "Couldn't start: laptop is offline"
    ));

    assert_eq!(
        hub.secrets.get("github.token").unwrap().as_deref(),
        Some("old-token")
    );
    assert_eq!(hub.secrets.get("env.FOO").unwrap().as_deref(), Some("bar"));
    assert_eq!(hub.db.machines().unwrap()[0].name, "laptop");

    let mut q = hub
        .create_project(corra_hub::hub::NewProject {
            name: "new".into(),
            provider: Some("existing".into()),
            ..Default::default()
        })
        .unwrap();
    q.allow_main = true;
    hub.db.update_project(&q).unwrap();
    hub.db.file_replace(threads[0].id, "s.jsonl", b"x").unwrap();
    hub.remove_thread(threads[0].id).unwrap();
    assert!(hub.db.project_by_name("new").unwrap().unwrap().allow_main);
}

#[tokio::test]
async fn a_new_database_gets_the_whole_schema_and_opens_again() {
    let dir = tempfile::tempdir().unwrap();
    drop(open(dir.path()).unwrap());
    let hub = open(dir.path()).unwrap();
    hub.create_project(corra_hub::hub::NewProject {
        name: "p".into(),
        provider: Some("existing".into()),
        ..Default::default()
    })
    .unwrap();
    let v: i64 = rusqlite::Connection::open(dir.path().join("corra.db"))
        .unwrap()
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(v, 12);
}

#[tokio::test]
async fn a_database_from_before_the_folded_schema_is_refused() {
    let dir = fixture();
    let c = rusqlite::Connection::open(dir.path().join("corra.db")).unwrap();
    c.pragma_update(None, "user_version", 5).unwrap();
    drop(c);
    let err = open(dir.path()).err().unwrap().to_string();
    assert!(err.contains("schema version"), "{err}");
}

#[tokio::test]
async fn the_wrong_master_key_is_caught_at_start() {
    let dir = fixture();
    std::fs::write(
        dir.path().join("master.key"),
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
    )
    .unwrap();
    assert!(open(dir.path()).is_err());
}
