use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use corra_hub::db::Db;
use corra_hub::hub::{Hub, HubRef, NewProject, Outbound};
use corra_hub::providers::Providers;
use corra_hub::secrets::Secrets;
use corra_hub::usage;
use corra_proto::app::HubToApp;
use corra_proto::{AgentKind, Event, Thread, ThreadStatus, TurnOutcome, new_id, now_ms};
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

async fn mock_usage_server(
    mode: Arc<AtomicUsize>,
    hits: Arc<AtomicUsize>,
    seen_beta: Arc<std::sync::Mutex<Option<String>>>,
    seen_auth: Arc<std::sync::Mutex<Option<String>>>,
) -> String {
    let app = Router::new().route(
        "/api/oauth/usage",
        get(move |headers: HeaderMap| {
            let mode = mode.clone();
            let hits = hits.clone();
            let seen_beta = seen_beta.clone();
            let seen_auth = seen_auth.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                *seen_beta.lock().unwrap() = headers
                    .get("anthropic-beta")
                    .and_then(|v| v.to_str().ok())
                    .map(String::from);
                *seen_auth.lock().unwrap() = headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .map(String::from);
                match mode.load(Ordering::SeqCst) {
                    0 => (
                        StatusCode::OK,
                        axum::Json(json!({
                            "five_hour": {
                                "utilization": 42.5,
                                "resets_at": "2099-04-11T07:00:00+00:00"
                            },
                            "seven_day": {
                                "utilization": 18.0,
                                "resets_at": "2099-04-17T01:00:00+00:00"
                            },
                            "seven_day_opus": null,
                            "seven_day_sonnet": {
                                "utilization": 1.0,
                                "resets_at": "2099-04-16T03:00:00+00:00"
                            },
                            "extra_usage": {
                                "is_enabled": false,
                                "monthly_limit": null,
                                "used_credits": null,
                                "utilization": null
                            }
                        })),
                    )
                        .into_response(),
                    _ => (
                        StatusCode::TOO_MANY_REQUESTS,
                        [("retry-after", "180")],
                        "rate limited",
                    )
                        .into_response(),
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}/api/oauth/usage")
}

#[tokio::test]
async fn claude_oauth_refresh_fetches_caches_and_backs_off_on_429() {
    let (_dir, hub) = test_hub();
    hub.secrets
        .set("claude.token", "sk-ant-oat01-test")
        .unwrap();

    let mode = Arc::new(AtomicUsize::new(0));
    let hits = Arc::new(AtomicUsize::new(0));
    let seen_beta = Arc::new(std::sync::Mutex::new(None));
    let seen_auth = Arc::new(std::sync::Mutex::new(None));
    let url = mock_usage_server(
        mode.clone(),
        hits.clone(),
        seen_beta.clone(),
        seen_auth.clone(),
    )
    .await;
    *hub.claude_usage_url.lock().unwrap() = url;

    let mut rx = hub.apps.subscribe();
    let u = usage::refresh(&hub, AgentKind::Claude, true).await.unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        seen_beta.lock().unwrap().as_deref(),
        Some("oauth-2025-04-20")
    );
    assert_eq!(
        seen_auth.lock().unwrap().as_deref(),
        Some("Bearer sk-ant-oat01-test")
    );
    assert!(u.refreshable);
    assert_eq!(u.error, None);
    assert_eq!(u.windows.len(), 3);
    assert_eq!(u.windows[0].key, "five_hour");
    assert_eq!(u.windows[0].used_pct, Some(42.5));
    assert_eq!(u.windows[2].key, "seven_day_sonnet");
    assert_eq!(u.windows[2].used_pct, Some(1.0));

    let msg = rx.try_recv().unwrap();
    assert!(matches!(
        &*msg,
        Outbound::All(HubToApp::Usage { usage }) if usage.agent == AgentKind::Claude && usage.windows.len() == 3
    ));

    let cached = usage::refresh(&hub, AgentKind::Claude, false)
        .await
        .unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(cached.windows[0].used_pct, Some(42.5));

    hub.usage.reset(AgentKind::Claude);
    mode.store(1, Ordering::SeqCst);
    let limited = usage::refresh(&hub, AgentKind::Claude, true).await.unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    assert_eq!(limited.windows.len(), 3);
    assert_eq!(limited.windows[0].used_pct, Some(42.5));
    assert!(
        limited
            .error
            .as_deref()
            .unwrap_or("")
            .contains("rate-limited")
    );

    let during_backoff = usage::refresh(&hub, AgentKind::Claude, true).await.unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    assert_eq!(during_backoff.windows.len(), 3);
    assert!(during_backoff.error.is_some());
}

#[tokio::test]
async fn legacy_usage_kv_loads_as_claude_and_agents_stay_isolated() {
    let (_dir, hub) = test_hub();
    let future_sec = (now_ms() / 1000) + 3600;
    hub.db
        .kv_set(
            "usage",
            &json!({
                "at": now_ms(),
                "limits": {
                    "rate_limit_info": {
                        "rateLimitType": "five_hour",
                        "resetsAt": future_sec,
                        "status": "rejected",
                        "unifiedWindows": {
                            "five_hour": { "resetsAt": future_sec, "status": "rejected", "utilization": 0 },
                            "seven_day": { "resetsAt": future_sec + 86400, "status": "allowed", "utilization": 0.3 }
                        }
                    }
                }
            })
            .to_string(),
        )
        .unwrap();

    let claude = usage::load(&hub, AgentKind::Claude, now_ms()).unwrap();
    assert_eq!(claude.windows.len(), 2);
    assert!(claude.windows[0].blocked);
    assert_eq!(claude.windows[0].used_pct, Some(100.0));
    assert_eq!(claude.windows[1].used_pct, Some(30.0));
    assert!(usage::load(&hub, AgentKind::Antigravity, now_ms()).is_none());

    let p = hub
        .create_project(NewProject {
            name: "demo".into(),
            provider: Some("modal".into()),
            ..Default::default()
        })
        .unwrap();
    let t = Thread {
        id: new_id(),
        project: p.id,
        title: "agy thread".into(),
        agent: AgentKind::Antigravity,
        machine: None,
        session_id: None,
        cwd: "/tmp".into(),
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

    hub.append(
        t.id,
        None,
        now_ms(),
        Event::TurnCompleted {
            turn: "t1".into(),
            outcome: TurnOutcome::Failed,
            usage: None,
            error: Some("Antigravity refused or ran out of quota for this request.".into()),
            anchor: None,
            resets_at: None,
        },
    )
    .unwrap();

    let agy = usage::load(&hub, AgentKind::Antigravity, now_ms()).unwrap();
    assert_eq!(agy.windows.len(), 1);
    assert!(agy.windows[0].blocked);

    hub.append(
        t.id,
        None,
        now_ms(),
        Event::TurnCompleted {
            turn: "t2".into(),
            outcome: TurnOutcome::Completed,
            usage: None,
            error: None,
            anchor: None,
            resets_at: None,
        },
    )
    .unwrap();

    assert!(usage::load(&hub, AgentKind::Antigravity, now_ms()).is_none());
    let claude_after = usage::load(&hub, AgentKind::Claude, now_ms()).unwrap();
    assert!(claude_after.windows[0].blocked);
}
