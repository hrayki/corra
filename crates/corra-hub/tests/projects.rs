use corra_hub::hub::{HubRef, NewProject, Outbound};
use corra_hub::ws::exec_call;
use corra_proto::app::{AppCall, HubToApp};
use corra_proto::*;

fn test_hub(dir: &std::path::Path) -> HubRef {
    corra_hub::build(corra_hub::Options {
        data: dir.to_path_buf(),
        public_url: "http://127.0.0.1:1".into(),
        dev_no_auth: true,
    })
    .unwrap()
    .state
    .hub
}

#[tokio::test]
async fn remove_repo_runtime_info_and_delete_project_clean_up_everything() {
    let dir = tempfile::tempdir().unwrap();
    let hub = test_hub(dir.path());
    hub.secrets.set("env.FOO", "1").unwrap();
    hub.secrets.set("github.token", "ghp_test").unwrap();

    let p = hub
        .create_project(NewProject {
            name: "alpha".into(),
            repos: vec!["acme/one".into(), "acme/two".into()],
            provider: Some("existing".into()),
            env: vec!["FOO".into(), "MISSING".into()],
            ..Default::default()
        })
        .unwrap();

    let info = exec_call(&hub, AppCall::ProjectSetupLog { project: p.id })
        .await
        .unwrap();
    assert_eq!(info["env_injected"], serde_json::json!(["FOO"]));
    assert_eq!(info["manager_env"], serde_json::json!([]));
    assert!(
        !info["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "projects.*")
    );

    let mut p_trusted = p.clone();
    p_trusted.trusted = true;
    hub.db.update_project(&p_trusted).unwrap();
    let info2 = exec_call(&hub, AppCall::ProjectSetupLog { project: p.id })
        .await
        .unwrap();
    assert_eq!(info2["manager_env"], serde_json::json!(["GH_TOKEN"]));
    assert!(
        info2["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "projects.*")
    );

    exec_call(
        &hub,
        AppCall::RemoveRepo {
            project: p.id,
            name: "one".into(),
        },
    )
    .await
    .unwrap();
    let after = hub.db.project(p.id).unwrap().unwrap();
    assert_eq!(after.repos.len(), 1);
    assert_eq!(after.repos[0].name, "two");
    assert!(
        exec_call(
            &hub,
            AppCall::RemoveRepo {
                project: p.id,
                name: "nope".into(),
            },
        )
        .await
        .is_err()
    );

    let t = Thread {
        id: new_id(),
        project: p.id,
        title: "t1".into(),
        agent: AgentKind::Claude,
        machine: None,
        session_id: None,
        cwd: "~/projects/alpha".into(),
        settings: Default::default(),
        status: ThreadStatus::Idle,
        last_seq: 0,
        created_at: now_ms(),
        updated_at: now_ms(),
        pinned_at: None,
        settled_at: None,
        hidden_at: None,
        started_by: None,
    };
    hub.db.insert_thread(&t).unwrap();

    let mut rx = hub.apps.subscribe();
    exec_call(&hub, AppCall::DeleteProject { project: p.id })
        .await
        .unwrap();
    assert!(hub.db.project(p.id).unwrap().is_none());
    assert!(hub.db.thread(t.id).unwrap().is_none());

    let mut saw_thread_removed = false;
    let mut saw_project_removed = false;
    while let Ok(msg) = rx.try_recv() {
        if let Outbound::All(HubToApp::ThreadRemoved { thread }) = &*msg
            && *thread == t.id
        {
            saw_thread_removed = true;
        }
        if let Outbound::All(HubToApp::ProjectRemoved { project }) = &*msg
            && *project == p.id
        {
            saw_project_removed = true;
        }
    }
    assert!(saw_thread_removed);
    assert!(saw_project_removed);
}

#[tokio::test]
async fn project_stage_and_setup_log_stream_and_replay_on_connect() {
    let dir = tempfile::tempdir().unwrap();
    let hub = test_hub(dir.path());
    let p = hub
        .create_project(NewProject {
            name: "beta".into(),
            provider: Some("existing".into()),
            setup: Some("rust".into()),
            ..Default::default()
        })
        .unwrap();

    let mut rx = hub.apps.subscribe();
    hub.set_project_stage(p.id, Some("Waking Modal machine…".into()));
    let log = SetupLog {
        ok: false,
        at: now_ms(),
        setup: Some("rust".into()),
        log: "cargo failed\n".into(),
    };
    hub.save_project_setup_log(p.id, &log);

    let mut saw_stage = false;
    let mut saw_log = false;
    while let Ok(msg) = rx.try_recv() {
        if let Outbound::All(HubToApp::ProjectStatus {
            project,
            stage,
            setup,
        }) = &*msg
            && *project == p.id
        {
            if stage.as_deref() == Some("Waking Modal machine…") {
                saw_stage = true;
            }
            if setup.as_ref() == Some(&log) {
                saw_log = true;
            }
        }
    }
    assert!(saw_stage);
    assert!(saw_log);

    let replayed = hub.project_status_messages();
    assert!(replayed.iter().any(|m| matches!(
        m,
        HubToApp::ProjectStatus { project, stage, setup }
            if *project == p.id
                && stage.as_deref() == Some("Waking Modal machine…")
                && setup.as_ref() == Some(&log)
    )));
}

#[tokio::test]
async fn static_manage_actions_match_defs() {
    let dir = tempfile::tempdir().unwrap();
    let hub = test_hub(dir.path());
    for d in corra_hub::manage::defs(&hub) {
        assert!(
            corra_hub::manage::is_manage(&d.action),
            "missing manage action {}",
            d.action
        );
    }
    assert!(!corra_hub::manage::is_manage("github.pr.list"));
    assert!(!corra_hub::manage::is_manage("unknown.tool"));
}
