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
