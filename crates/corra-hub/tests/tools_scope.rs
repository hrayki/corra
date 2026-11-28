use corra_hub::db::{Db, NewMachine};
use corra_hub::hub::{Hub, HubRef, NewProject};
use corra_hub::providers::Providers;
use corra_hub::secrets::Secrets;
use corra_proto::*;
use serde_json::json;

fn test_hub() -> (tempfile::TempDir, HubRef) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("corra.db")).unwrap();
    let secrets = Secrets::open(db.clone(), dir.path()).unwrap();
    let hub = Hub::new(
        db,
        Providers::new("http://127.0.0.1".into()),
        secrets,
        dir.path().to_path_buf(),
    );
    (dir, hub)
}

fn add_thread(hub: &HubRef, project: ProjectId, machine: MachineId, title: &str) -> Thread {
    let now = now_ms();
    let t = Thread {
        id: new_id(),
        project,
        title: title.into(),
        agent: AgentKind::Claude,
        machine: Some(machine),
        session_id: None,
        cwd: "~".into(),
        settings: AgentSettings::default(),
        status: ThreadStatus::Idle,
        last_seq: 0,
        created_at: now,
        updated_at: now,
        pinned_at: None,
        settled_at: None,
        hidden_at: None,
        started_by: None,
    };
    hub.db.insert_thread(&t).unwrap();
    t
}

#[tokio::test]
async fn normal_project_gets_project_scoped_thread_tools_but_not_cross_project_or_manager() {
    let (_dir, hub) = test_hub();
    let alpha = hub
        .create_project(NewProject {
            name: "alpha".into(),
            provider: Some("modal".into()),
            trusted: false,
            ..Default::default()
        })
        .unwrap();
    let beta = hub
        .create_project(NewProject {
            name: "beta".into(),
            provider: Some("modal".into()),
            trusted: false,
            ..Default::default()
        })
        .unwrap();
    let m_alpha = hub
        .db
        .insert_machine(NewMachine {
            name: "m-alpha".into(),
            provider: "modal".into(),
            kind: Some("vm".into()),
            status: MachineStatus::Connected,
            project: Some(alpha.id),
            size: "4x8".into(),
            token_hash: "h1".into(),
            provider_ref: None,
        })
        .unwrap();
    let m_beta = hub
        .db
        .insert_machine(NewMachine {
            name: "m-beta".into(),
            provider: "modal".into(),
            kind: Some("vm".into()),
            status: MachineStatus::Connected,
            project: Some(beta.id),
            size: "4x8".into(),
            token_hash: "h2".into(),
            provider_ref: None,
        })
        .unwrap();

    let defs = corra_hub::tools::defs_for(&hub, &alpha, m_alpha.id);
    let actions: Vec<&str> = defs.iter().map(|d| d.action.as_str()).collect();
    for expected in [
        "threads.list",
        "threads.read",
        "threads.start",
        "threads.send",
        "threads.stop",
        "threads.follow",
    ] {
        assert!(actions.contains(&expected), "missing {expected}");
    }
    for forbidden in [
        "threads.move",
        "projects.list",
        "projects.create",
        "machines.list",
        "credentials.list",
    ] {
        assert!(!actions.contains(&forbidden), "unexpected {forbidden}");
    }

    let caller = add_thread(&hub, alpha.id, m_alpha.id, "alpha caller");
    let peer = add_thread(&hub, alpha.id, m_alpha.id, "alpha peer");
    let foreign = add_thread(&hub, beta.id, m_beta.id, "beta secret");

    hub.append(
        peer.id,
        None,
        now_ms(),
        Event::TurnStarted {
            turn: "t1".into(),
            input: vec![InputPart::Text {
                text: "peer prompt".into(),
            }],
            settings: AgentSettings::default(),
            cause: None,
        },
    )
    .unwrap();
    hub.append(
        foreign.id,
        None,
        now_ms(),
        Event::TurnStarted {
            turn: "t1".into(),
            input: vec![InputPart::Text {
                text: "foreign secret".into(),
            }],
            settings: AgentSettings::default(),
            cause: None,
        },
    )
    .unwrap();

    let list = corra_hub::tools::call(&hub, m_alpha.id, caller.id, "t1", "threads.list", json!({}))
        .await
        .unwrap();
    let ids: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&caller.id.to_string().as_str()));
    assert!(ids.contains(&peer.id.to_string().as_str()));
    assert!(!ids.contains(&foreign.id.to_string().as_str()));

    let read_peer = corra_hub::tools::call(
        &hub,
        m_alpha.id,
        caller.id,
        "t1",
        "threads.read",
        json!({ "thread": peer.id.to_string() }),
    )
    .await
    .unwrap();
    assert_eq!(read_peer["turns"][0]["asked"], "peer prompt");

    let followed = corra_hub::tools::call(
        &hub,
        m_alpha.id,
        caller.id,
        "t1",
        "threads.follow",
        json!({ "thread": peer.id.to_string(), "follow": true }),
    )
    .await
    .unwrap();
    assert_eq!(followed["following"], "alpha peer");
    assert_eq!(hub.follower(peer.id), Some(caller.id));

    for (action, payload) in [
        ("threads.read", json!({ "thread": foreign.id.to_string() })),
        (
            "threads.send",
            json!({ "thread": foreign.id.to_string(), "message": "hi" }),
        ),
        ("threads.stop", json!({ "thread": foreign.id.to_string() })),
        (
            "threads.follow",
            json!({ "thread": foreign.id.to_string(), "follow": true }),
        ),
        ("projects.list", json!({})),
        ("credentials.list", json!({})),
    ] {
        assert!(
            corra_hub::tools::call(&hub, m_alpha.id, caller.id, "t1", action, payload)
                .await
                .is_err(),
            "{action} should be rejected for non-manager / foreign project"
        );
    }
}

#[test]
fn runner_update_pending_reflects_connected_build_mismatch() {
    let base = Machine {
        id: new_id(),
        name: "m1".into(),
        provider: "modal".into(),
        kind: Some("vm".into()),
        status: MachineStatus::Connected,
        project: None,
        size: "4x8".into(),
        info: Some(MachineInfo {
            runner_build: Some("old-sha".into()),
            ..Default::default()
        }),
        created_at: 0,
        last_seen: None,
        update_pending: false,
    };

    assert!(Hub::machine_update_pending(&base, Some("new-sha")));
    assert!(!Hub::machine_update_pending(&base, Some("old-sha")));
    assert!(!Hub::machine_update_pending(&base, None));

    let asleep = Machine {
        status: MachineStatus::Asleep,
        ..base.clone()
    };
    assert!(!Hub::machine_update_pending(&asleep, Some("new-sha")));

    let legacy = Machine {
        info: Some(MachineInfo {
            runner_build: None,
            ..Default::default()
        }),
        ..base
    };
    assert!(Hub::machine_update_pending(&legacy, Some("new-sha")));
}
