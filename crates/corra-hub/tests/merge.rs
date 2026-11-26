use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, put};
use corra_hub::hub::HubRef;
use corra_proto::*;
use serde_json::{Value, json};

#[derive(Default)]
struct Fake {
    head: String,
    merged_with: Option<Value>,
}

type Shared = Arc<Mutex<Fake>>;

async fn pull(State(f): State<Shared>) -> Json<Value> {
    let head = f.lock().unwrap().head.clone();
    Json(json!({
        "number": 7,
        "state": "open",
        "merged_at": null,
        "title": "Add the thing",
        "head": { "ref": "feature", "sha": head },
        "base": { "ref": "main" }
    }))
}

async fn merge(State(f): State<Shared>, Json(body): Json<Value>) -> (StatusCode, Json<Value>) {
    let mut f = f.lock().unwrap();
    if body["sha"].as_str().is_some_and(|s| s != f.head) {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "message": "Head branch was modified. Review and try the merge again." })),
        );
    }
    f.merged_with = Some(body);
    (
        StatusCode::OK,
        Json(
            json!({ "merged": true, "sha": "m".repeat(40), "message": "Pull Request successfully merged" }),
        ),
    )
}

async fn setup() -> (tempfile::TempDir, HubRef, Shared, Thread) {
    let fake: Shared = Arc::new(Mutex::new(Fake {
        head: "a".repeat(40),
        merged_with: None,
    }));
    let app = Router::new()
        .route("/repos/o/r/pulls/7", get(pull))
        .route("/repos/o/r/pulls/7/merge", put(merge))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let dir = tempfile::tempdir().unwrap();
    let built = corra_hub::build(corra_hub::Options {
        data: dir.path().to_path_buf(),
        public_url: "http://127.0.0.1:1".into(),
        dev_no_auth: false,
    })
    .unwrap();
    let hub = built.state.hub.clone();
    *hub.github_api.lock().unwrap() = format!("http://{addr}");
    hub.secrets.set("github.token", "fake").unwrap();
    let p = hub
        .create_project(corra_hub::hub::NewProject {
            name: "p".into(),
            repos: vec!["o/r".into()],
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
        cwd: "~/projects/p".into(),
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
    (dir, hub, fake, t)
}

async fn shown_request(hub: &HubRef, thread: ThreadId) -> (String, Value) {
    for _ in 0..100 {
        for e in hub.db.events_after(thread, 0, 1000).unwrap() {
            if let Event::Request {
                id,
                request: AgentRequest::HubAction { input, .. },
                ..
            } = e.event
            {
                return (id, input);
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the merge never asked");
}

async fn merge_after_yes(move_branch: bool) -> (Result<Value, String>, Value, Shared) {
    let (_dir, hub, fake, t) = setup().await;
    let call = {
        let hub = hub.clone();
        tokio::spawn(async move {
            corra_hub::tools::call(
                &hub,
                new_id(),
                t.id,
                "turn",
                "github.pr.merge",
                json!({ "number": 7, "sha": "b".repeat(40) }),
            )
            .await
            .map_err(|e| format!("{e:#}"))
        })
    };
    let (request, shown) = shown_request(&hub, t.id).await;
    if move_branch {
        fake.lock().unwrap().head = "c".repeat(40);
    }
    hub.respond(t.id, request, RequestResponse::Allow { always: false })
        .unwrap();
    (call.await.unwrap(), shown, fake)
}

#[tokio::test]
async fn a_merge_shows_the_commit_and_merges_only_that_commit() {
    let (result, shown, fake) = merge_after_yes(false).await;
    assert_eq!(shown["sha"], "a".repeat(40));
    assert_eq!(shown["title"], "Add the thing");
    assert_eq!(shown["head"], "feature");
    assert_eq!(shown["base"], "main");
    assert_eq!(result.unwrap()["merged"], true);
    let sent = fake.lock().unwrap().merged_with.clone().unwrap();
    assert_eq!(sent["sha"], "a".repeat(40));
}

#[tokio::test]
async fn a_branch_that_moved_after_the_yes_is_not_merged() {
    let (result, _, fake) = merge_after_yes(true).await;
    let err = result.unwrap_err();
    assert!(err.contains("new commits since the user allowed"), "{err}");
    assert!(fake.lock().unwrap().merged_with.is_none());
}
