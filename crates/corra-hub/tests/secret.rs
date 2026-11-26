use std::time::Duration;

use corra_hub::hub::HubRef;
use corra_proto::*;

const VALUE: &str = "s3cr3t-value-7f3a9c";

fn setup(dir: &std::path::Path) -> (HubRef, ThreadId) {
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
    let t = Thread {
        id: new_id(),
        project: p.id,
        title: "t".into(),
        agent: AgentKind::Claude,
        machine: None,
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
    (hub, t.id)
}

fn ask(hub: &HubRef, thread: ThreadId) -> tokio::task::JoinHandle<Option<String>> {
    let hub = hub.clone();
    tokio::spawn(async move {
        hub.ask_secret(thread, "turn", "API_KEY", "to call the API")
            .await
            .unwrap()
    })
}

async fn open_request(hub: &HubRef, thread: ThreadId, nth: usize) -> String {
    for _ in 0..100 {
        let ids: Vec<String> = hub
            .db
            .events_after(thread, 0, 1000)
            .unwrap()
            .into_iter()
            .filter_map(|e| match e.event {
                Event::Request { id, .. } => Some(id),
                _ => None,
            })
            .collect();
        if let Some(id) = ids.get(nth) {
            return id.clone();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no request appeared");
}

#[tokio::test]
async fn a_secret_reaches_the_asker_and_is_never_stored() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, thread) = setup(dir.path());
    let asked = ask(&hub, thread);
    let id = open_request(&hub, thread, 0).await;

    assert!(
        hub.answer_secret(thread, id.clone(), String::new())
            .is_err()
    );
    hub.answer_secret(thread, id.clone(), VALUE.into()).unwrap();
    assert_eq!(asked.await.unwrap().as_deref(), Some(VALUE));
    assert!(hub.answer_secret(thread, id, VALUE.into()).is_err());

    let events = hub.db.events_after(thread, 0, 1000).unwrap();
    assert!(matches!(
        &events[0].event,
        Event::Request { request: AgentRequest::HubAction { action, input }, .. }
            if action == "secret.request" && input["name"] == "API_KEY" && input["why"] == "to call the API"
    ));
    assert!(matches!(
        &events[1].event,
        Event::RequestResolved {
            response: RequestResponse::Allow { .. },
            ..
        }
    ));
    hub.db.snapshot_to(&dir.path().join("snapshot.db")).unwrap();
    for f in std::fs::read_dir(dir.path()).unwrap().flatten() {
        let bytes = std::fs::read(f.path()).unwrap();
        let found = bytes.windows(VALUE.len()).any(|w| w == VALUE.as_bytes());
        assert!(!found, "the secret is in {}", f.path().display());
    }
}

#[tokio::test]
async fn declining_or_an_old_apps_allow_gives_the_agent_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, thread) = setup(dir.path());

    let asked = ask(&hub, thread);
    let id = open_request(&hub, thread, 0).await;
    hub.respond(thread, id, RequestResponse::Deny { message: None })
        .unwrap();
    assert_eq!(asked.await.unwrap(), None);

    let asked = ask(&hub, thread);
    let id = open_request(&hub, thread, 1).await;
    hub.respond(thread, id, RequestResponse::Allow { always: false })
        .unwrap();
    assert_eq!(asked.await.unwrap(), None);
    let last = hub.db.events_after(thread, 0, 1000).unwrap().pop().unwrap();
    assert!(matches!(
        last.event,
        Event::RequestResolved {
            response: RequestResponse::Cancelled,
            ..
        }
    ));
}

#[tokio::test]
async fn hub_internal_keys_are_out_of_reach_of_credential_calls() {
    use corra_hub::ws::exec_call;
    use corra_proto::app::AppCall;
    let dir = tempfile::tempdir().unwrap();
    let (hub, _) = setup(dir.path());
    hub.secrets.set("push.vapid", "k").unwrap();
    hub.secrets.set("machine_token.abc", "t").unwrap();
    hub.secrets.set("github.token", "g").unwrap();
    let listed = exec_call(&hub, AppCall::ListSecrets).await.unwrap();
    let listed: Vec<String> = serde_json::from_value(listed).unwrap();
    assert!(listed.contains(&"github.token".to_string()));
    assert!(
        !listed
            .iter()
            .any(|n| n.starts_with("push.") || n.starts_with("machine_token.") || n == "key-check")
    );
    for name in ["push.vapid", "machine_token.abc", "key-check", "env.X"] {
        assert!(
            exec_call(&hub, AppCall::DeleteSecret { name: name.into() })
                .await
                .is_err(),
            "{name} could be deleted"
        );
        assert!(
            exec_call(
                &hub,
                AppCall::SetSecret {
                    name: name.into(),
                    value: "x".into()
                }
            )
            .await
            .is_err(),
            "{name} could be set"
        );
    }
    assert_eq!(hub.secrets.get("push.vapid").unwrap().as_deref(), Some("k"));
    assert_eq!(
        hub.secrets.get("machine_token.abc").unwrap().as_deref(),
        Some("t")
    );
}
