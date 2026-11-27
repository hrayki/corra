use corra_hub::hub::{HubRef, NewProject};
use corra_proto::*;
use serde_json::json;

fn hub(dir: &std::path::Path) -> HubRef {
    corra_hub::build(corra_hub::Options {
        data: dir.into(),
        public_url: "http://x".into(),
        dev_no_auth: true,
    })
    .unwrap()
    .state
    .hub
}

fn thread(hub: &HubRef, agent: AgentKind) -> Thread {
    let p = hub
        .create_project(NewProject {
            name: format!("p-{}", agent.as_str()),
            ..Default::default()
        })
        .unwrap();
    let t = Thread {
        id: new_id(),
        project: p.id,
        title: agent.name().into(),
        agent,
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
        started_by: None,
    };
    hub.db.insert_thread(&t).unwrap();
    t
}

#[tokio::test]
async fn agents_cant_prompt_an_antigravity_thread_signed_in_with_google() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub(dir.path());
    let claude = thread(&hub, AgentKind::Claude);
    let agy = thread(&hub, AgentKind::Antigravity);

    let sent = corra_hub::manage::run(
        &hub,
        &claude,
        "threads.send",
        json!({ "thread": agy.id.to_string(), "message": "do this" }),
    )
    .await;
    assert!(
        sent.unwrap_err().to_string().contains("only from the user"),
        "an agent prompted an Antigravity thread"
    );
    let started = corra_hub::manage::run(
        &hub,
        &agy,
        "threads.start",
        json!({ "project": "p-antigravity", "prompt": "go" }),
    )
    .await;
    assert!(
        started.is_err(),
        "an Antigravity thread started another with its own prompt"
    );
    assert!(hub.db.threads().unwrap().len() == 2);

    hub.secrets.set("gemini.api_key", "test-key").unwrap();
    assert!(
        !hub.hands_off(AgentKind::Antigravity),
        "a Gemini API key should lift the limit"
    );
    assert!(!hub.hands_off(AgentKind::Claude));
}

#[tokio::test]
async fn antigravity_thread_can_spawn_a_claude_thread_without_inheriting_its_model() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub(dir.path());
    let mut agy = thread(&hub, AgentKind::Antigravity);
    agy.settings = AgentSettings {
        model: Some("gemini-4-neon".into()),
        effort: Some("high".into()),
        permission_mode: Some("bypassPermissions".into()),
    };
    hub.db.update_thread(&agy).unwrap();
    let claude = thread(&hub, AgentKind::Claude);

    let bad_follow = corra_hub::manage::run(
        &hub,
        &agy,
        "threads.start",
        json!({ "project": "p-claude", "agent": "claude", "prompt": "work", "follow": true }),
    )
    .await;
    assert!(
        bad_follow.unwrap_err().to_string().contains("threads.read"),
        "hands-off Antigravity caller should not be allowed to follow"
    );

    let res = corra_hub::manage::run(
        &hub,
        &agy,
        "threads.start",
        json!({ "project": "p-claude", "agent": "claude", "model": "opus", "prompt": "do the work" }),
    )
    .await
    .unwrap();
    assert_eq!(res["agent"], "claude");
    assert_eq!(res["following"], false);
    let spawned_id: ThreadId = res["thread"].as_str().unwrap().parse().unwrap();
    let spawned = hub.db.thread(spawned_id).unwrap().unwrap();
    assert_eq!(spawned.agent, AgentKind::Claude);
    assert_eq!(spawned.started_by, Some(agy.id));
    assert_eq!(spawned.settings.model.as_deref(), Some("opus"));
    assert_eq!(spawned.settings.effort.as_deref(), Some("high"));
    assert_eq!(
        spawned.settings.permission_mode.as_deref(),
        Some("bypassPermissions")
    );
    assert_eq!(hub.follower(spawned_id), None);

    let res2 = corra_hub::manage::run(
        &hub,
        &agy,
        "threads.start",
        json!({ "project": "p-claude", "agent": "claude", "prompt": "default model" }),
    )
    .await
    .unwrap();
    let spawned2_id: ThreadId = res2["thread"].as_str().unwrap().parse().unwrap();
    let spawned2 = hub.db.thread(spawned2_id).unwrap().unwrap();
    assert_eq!(spawned2.settings.model, None);

    let claude_to_agy = corra_hub::manage::run(
        &hub,
        &claude,
        "threads.start",
        json!({ "project": "p-antigravity", "agent": "antigravity", "prompt": "hi" }),
    )
    .await;
    assert!(claude_to_agy.is_err());

    hub.secrets.set("gemini.api_key", "test-key").unwrap();
    let ok = corra_hub::manage::run(
        &hub,
        &claude,
        "threads.start",
        json!({ "project": "p-antigravity", "agent": "antigravity", "prompt": "hi" }),
    )
    .await
    .unwrap();
    assert_eq!(ok["agent"], "antigravity");
    assert_eq!(ok["following"], true);
}
