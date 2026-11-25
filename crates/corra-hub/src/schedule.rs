use anyhow::Result;
use corra_proto::*;
use serde_json::json;

use crate::hub::{Hub, HubRef};

pub const CONTINUE: &str = "Your usage limit has reset. Continue where you left off.";
const AFTER_RESET_MS: i64 = 60 * 1000;
const MAX_TRIES: u32 = 2;

fn continue_key(thread: ThreadId) -> String {
    format!("continue:{thread}")
}

pub fn plan_continue(hub: &Hub, thread: ThreadId, resets_at: i64) {
    if resets_at > now_ms() && tries(hub, thread) < MAX_TRIES {
        let _ = hub.db.kv_set(&continue_key(thread), &resets_at.to_string());
        hub.set_live_status(thread, json!({ "continue_at": continue_time(resets_at) }));
    }
}

pub fn cancel_continue(hub: &Hub, thread: ThreadId) {
    if planned_continue(hub, thread).is_some() {
        let _ = hub.db.kv_delete(&continue_key(thread));
        hub.set_live_status(thread, json!({ "continue_at": 0 }));
    }
}

pub fn planned_continue(hub: &Hub, thread: ThreadId) -> Option<i64> {
    hub.db.kv_get(&continue_key(thread)).ok()??.parse().ok()
}

pub fn continue_time(resets_at: i64) -> i64 {
    resets_at + AFTER_RESET_MS
}

fn tries(hub: &Hub, thread: ThreadId) -> u32 {
    hub.thread_kv(thread, "continues")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

pub async fn continue_limited(hub: &HubRef, now: i64) -> Result<()> {
    for key in hub.db.kv_keys("continue:")? {
        let Some(thread) = key
            .strip_prefix("continue:")
            .and_then(|id| id.parse::<ThreadId>().ok())
        else {
            continue;
        };
        let Some(at) = planned_continue(hub, thread) else {
            continue;
        };
        if continue_time(at) > now {
            continue;
        }
        cancel_continue(hub, thread);
        let Some(t) = hub.db.thread(thread)? else {
            continue;
        };
        if t.hidden_at.is_some() || t.status == ThreadStatus::Waiting || hub.busy(thread) {
            continue;
        }
        let n = tries(hub, thread) + 1;
        hub.set_thread_kv(thread, "continues", &n.to_string());
        tracing::info!(%thread, "the usage limit reset, so the thread continues");
        hub.send(
            thread,
            vec![InputPart::Text {
                text: CONTINUE.into(),
            }],
            None,
            false,
        )
        .await?;
    }
    Ok(())
}

pub const COMPACT: &str = "/compact";
const DEFAULT_OVER: u64 = 200_000;
const QUIET_FOR: std::time::Duration = std::time::Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IdleCompaction {
    Never,
    WhenIReturn,
    BeforeExpiry,
}

pub fn idle_compaction(hub: &Hub) -> IdleCompaction {
    match hub.settings()["compact_idle"].as_str() {
        Some("never") => IdleCompaction::Never,
        Some("before") => IdleCompaction::BeforeExpiry,
        _ => IdleCompaction::WhenIReturn,
    }
}

pub fn compact_over(hub: &Hub) -> u64 {
    let v = &hub.settings()["compact_over"];
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_OVER)
}

pub fn cache_ttl(hub: &Hub) -> i64 {
    let has = |k: &str| hub.secrets.get(k).ok().flatten().is_some();
    if !has("claude.token") && has("anthropic.api_key") {
        5 * 60 * 1000
    } else {
        60 * 60 * 1000
    }
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub struct CacheUse {
    pub tokens: u64,
    pub at: i64,
    #[serde(default)]
    pub tried: bool,
}

fn cache_key(thread: ThreadId) -> String {
    format!("cache:{thread}")
}

pub fn cache_use(hub: &Hub, thread: ThreadId) -> Option<CacheUse> {
    serde_json::from_str(&hub.db.kv_get(&cache_key(thread)).ok()??).ok()
}

fn save_cache_use(hub: &Hub, thread: ThreadId, c: CacheUse) {
    if let Ok(s) = serde_json::to_string(&c) {
        let _ = hub.db.kv_set(&cache_key(thread), &s);
    }
}

pub fn cache_status(hub: &Hub, c: &CacheUse) -> serde_json::Value {
    json!({
        "tokens": c.tokens,
        "until": c.at + cache_ttl(hub),
        "compact": idle_compaction(hub) != IdleCompaction::Never && c.tokens >= compact_over(hub),
    })
}

pub fn note_cache_use(hub: &Hub, thread: ThreadId, tokens: u64, at: i64) {
    let c = CacheUse {
        tokens,
        at,
        tried: false,
    };
    save_cache_use(hub, thread, c);
    hub.set_live_status(thread, json!({ "cache": cache_status(hub, &c) }));
}

pub fn quiet_turn(hub: &Hub, thread: ThreadId) {
    hub.quiet_turns
        .lock()
        .unwrap()
        .insert(thread, std::time::Instant::now());
}

pub fn is_quiet_turn(hub: &Hub, thread: ThreadId) -> bool {
    hub.quiet_turns
        .lock()
        .unwrap()
        .get(&thread)
        .is_some_and(|at| at.elapsed() < QUIET_FOR)
}

pub fn take_quiet_turn(hub: &Hub, thread: ThreadId) -> bool {
    hub.quiet_turns
        .lock()
        .unwrap()
        .remove(&thread)
        .is_some_and(|at| at.elapsed() < QUIET_FOR)
}

pub async fn compact_then_send(
    hub: &HubRef,
    thread: ThreadId,
    input: Vec<InputPart>,
    settings: Option<AgentSettings>,
    client_id: Option<String>,
) -> Result<()> {
    if let Some(id) = &client_id
        && !hub.first_send(id)
    {
        return Ok(());
    }
    if hub.busy(thread) {
        hub.enqueue(thread, input, client_id);
        return Ok(());
    }
    hub.enqueue_front(thread, input, client_id);
    quiet_turn(hub, thread);
    hub.send(
        thread,
        vec![InputPart::Text {
            text: COMPACT.into(),
        }],
        settings,
        false,
    )
    .await
}

pub async fn compact_idle(hub: &HubRef, now: i64) -> Result<()> {
    if idle_compaction(hub) != IdleCompaction::BeforeExpiry {
        return Ok(());
    }
    let ttl = cache_ttl(hub);
    let lead = ttl / 6;
    let over = compact_over(hub);
    for key in hub.db.kv_keys("cache:")? {
        let Some(thread) = key
            .strip_prefix("cache:")
            .and_then(|id| id.parse::<ThreadId>().ok())
        else {
            continue;
        };
        let Some(mut c) = cache_use(hub, thread) else {
            continue;
        };
        if c.tried || c.tokens < over || now < c.at + ttl - lead || now > c.at + ttl - 30_000 {
            continue;
        }
        let Some(t) = hub.db.thread(thread)? else {
            let _ = hub.db.kv_delete(&key);
            continue;
        };
        if t.hidden_at.is_some()
            || t.agent != AgentKind::Claude
            || t.status != ThreadStatus::Idle
            || !hub.queued(thread).is_empty()
            || planned_continue(hub, thread).is_some()
        {
            continue;
        }
        c.tried = true;
        save_cache_use(hub, thread, c);
        tracing::info!(%thread, tokens = c.tokens, "compacting an idle thread before its cache expires");
        quiet_turn(hub, thread);
        hub.send(
            thread,
            vec![InputPart::Text {
                text: COMPACT.into(),
            }],
            None,
            false,
        )
        .await?;
    }
    Ok(())
}
