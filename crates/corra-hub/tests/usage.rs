use corra_hub::db::Db;
use corra_hub::hub::{Hub, HubRef, NewProject, Outbound};
use corra_hub::providers::Providers;
use corra_hub::secrets::Secrets;
use corra_hub::usage;
use corra_proto::app::HubToApp;
use corra_proto::{
    AgentKind, AgentUsage, Event, Thread, ThreadStatus, TurnOutcome, UsageWindow, new_id, now_ms,
};
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

#[tokio::test]
async fn runner_reported_usage_is_persisted_broadcast_and_clears_stale_claude_error() {
    let (_dir, hub) = test_hub();
    let now = now_ms();
    let reset_5h = now + 3_600_000;
    let reset_7d = now + 5 * 86_400_000;

    hub.db
        .kv_set(
            "usage:claude",
            &serde_json::to_string(&AgentUsage {
                agent: AgentKind::Claude,
                updated_at: now - 60_000,
                windows: vec![],
                error: Some("Claude token wasn't accepted by the usage endpoint".into()),
                refreshable: true,
            })
            .unwrap(),
        )
        .unwrap();
    assert!(usage::load(&hub, AgentKind::Claude, now).is_none());

    let mut rx = hub.apps.subscribe();
    let ingested = usage::ingest_usage(
        &hub,
        AgentUsage {
            agent: AgentKind::Claude,
            updated_at: now,
            windows: vec![
                UsageWindow {
                    key: "five_hour".into(),
                    label: "5-hour".into(),
                    used_pct: Some(34.0),
                    resets_at: Some(reset_5h),
                    blocked: false,
                    detail: None,
                },
                UsageWindow {
                    key: "seven_day".into(),
                    label: "Weekly".into(),
                    used_pct: Some(77.0),
                    resets_at: Some(reset_7d),
                    blocked: false,
                    detail: None,
                },
            ],
            error: None,
            refreshable: false,
        },
    );
    assert!(!ingested.refreshable);
    assert_eq!(ingested.error, None);
    assert_eq!(ingested.windows.len(), 2);
    assert_eq!(ingested.windows[0].used_pct, Some(34.0));
    assert_eq!(ingested.windows[1].used_pct, Some(77.0));

    let msg = rx.try_recv().unwrap();
    assert!(matches!(
        &*msg,
        Outbound::All(HubToApp::Usage { usage }) if usage.agent == AgentKind::Claude && usage.windows.len() == 2
    ));

    let loaded = usage::load(&hub, AgentKind::Claude, now).unwrap();
    assert_eq!(loaded.windows[0].used_pct, Some(34.0));
    assert_eq!(loaded.windows[1].used_pct, Some(77.0));

    let agy_without_runner = usage::refresh(&hub, AgentKind::Antigravity, true)
        .await
        .unwrap();
    assert!(!agy_without_runner.refreshable);
    assert!(
        agy_without_runner
            .error
            .as_deref()
            .unwrap_or("")
            .contains("No awake machine")
    );
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
