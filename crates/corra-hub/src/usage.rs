use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Result;
use corra_proto::app::HubToApp;
use corra_proto::runner::RunnerRequest;
use corra_proto::{AgentKind, AgentUsage, MachineId, UsageWindow, now_ms};
use serde_json::Value;

use crate::hub::{Hub, HubRef, Outbound};

const MANUAL_REFRESH_COOLDOWN_MS: i64 = 10 * 1000;
const RUNNER_USAGE_TIMEOUT: Duration = Duration::from_secs(22);
const SAME_WINDOW_TOLERANCE_MS: i64 = 5 * 60 * 1000;

#[derive(Default)]
pub struct UsageTracker {
    inner: Mutex<HashMap<AgentKind, RefreshMeta>>,
}

#[derive(Default, Clone, Copy)]
struct RefreshMeta {
    in_flight: bool,
    last_attempt_ms: i64,
}

impl UsageTracker {
    pub fn reset(&self, agent: AgentKind) {
        self.inner.lock().unwrap().remove(&agent);
    }
}

struct InFlightGuard<'a> {
    tracker: &'a UsageTracker,
    agent: AgentKind,
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        if let Some(m) = self.tracker.inner.lock().unwrap().get_mut(&self.agent) {
            m.in_flight = false;
        }
    }
}

fn kv_key(agent: AgentKind) -> String {
    format!("usage:{}", agent.as_str())
}

fn agy_runner(hub: &Hub) -> Option<MachineId> {
    let connected: Vec<MachineId> = hub.runners.lock().unwrap().keys().copied().collect();
    if connected.is_empty() {
        return None;
    }
    let machines = hub.db.machines().ok()?;
    for id in &connected {
        if let Some(m) = machines.iter().find(|m| m.id == *id)
            && m.info.as_ref().is_some_and(|info| {
                info.agents
                    .iter()
                    .any(|a| a.kind == AgentKind::Antigravity && a.version.is_some())
            })
        {
            return Some(*id);
        }
    }
    None
}

pub fn can_refresh(hub: &Hub, agent: AgentKind) -> bool {
    match agent {
        AgentKind::Claude => false,
        AgentKind::Antigravity => agy_runner(hub).is_some(),
    }
}

fn load_stored(hub: &Hub, agent: AgentKind) -> Option<AgentUsage> {
    if let Some(raw) = hub.db.kv_get(&kv_key(agent)).ok().flatten()
        && let Ok(mut u) = serde_json::from_str::<AgentUsage>(&raw)
    {
        if agent == AgentKind::Claude {
            u.error = None;
        }
        return Some(u);
    }
    if agent == AgentKind::Claude
        && let Some(raw) = hub.db.kv_get("usage").ok().flatten()
        && let Ok(v) = serde_json::from_str::<Value>(&raw)
    {
        let at = v["at"].as_i64().unwrap_or(0);
        let windows = parse_claude_rate_limit_event(&v["limits"]);
        if !windows.is_empty() {
            return Some(AgentUsage {
                agent: AgentKind::Claude,
                updated_at: at,
                windows,
                error: None,
                refreshable: false,
            });
        }
    }
    None
}

fn save_stored(hub: &Hub, usage: &AgentUsage) {
    if let Ok(s) = serde_json::to_string(usage) {
        let _ = hub.db.kv_set(&kv_key(usage.agent), &s);
    }
}

pub fn load(hub: &Hub, agent: AgentKind, now: i64) -> Option<AgentUsage> {
    let refreshable = can_refresh(hub, agent);
    if let Some(mut u) = load_stored(hub, agent) {
        prune_expired(&mut u.windows, now);
        u.refreshable = refreshable;
        if u.windows.is_empty() && u.error.is_none() && !u.refreshable {
            return None;
        }
        return Some(u);
    }
    if refreshable {
        return Some(AgentUsage {
            agent,
            updated_at: 0,
            windows: vec![],
            error: None,
            refreshable: true,
        });
    }
    None
}

pub fn all_messages(hub: &Hub) -> Vec<HubToApp> {
    let now = now_ms();
    AgentKind::ALL
        .into_iter()
        .filter_map(|a| load(hub, a, now))
        .map(|usage| HubToApp::Usage { usage })
        .collect()
}

pub fn publish_current(hub: &Hub, agent: AgentKind) {
    let now = now_ms();
    let usage = load(hub, agent, now).unwrap_or(AgentUsage {
        agent,
        updated_at: 0,
        windows: vec![],
        error: None,
        refreshable: can_refresh(hub, agent),
    });
    hub.publish(Outbound::All(HubToApp::Usage { usage }));
}

pub fn ingest_usage(hub: &Hub, incoming: AgentUsage) -> AgentUsage {
    let now = now_ms();
    let agent = incoming.agent;
    let mut current = load_stored(hub, agent).unwrap_or(AgentUsage {
        agent,
        updated_at: now,
        windows: vec![],
        error: None,
        refreshable: can_refresh(hub, agent),
    });
    merge_windows(&mut current.windows, incoming.windows, true, now);
    current.updated_at = if incoming.updated_at > 0 {
        incoming.updated_at
    } else {
        now
    };
    current.error = None;
    current.refreshable = can_refresh(hub, agent);
    save_stored(hub, &current);
    hub.publish(Outbound::All(HubToApp::Usage {
        usage: current.clone(),
    }));
    current
}

pub fn ingest_rate_limit(hub: &Hub, agent: AgentKind, raw: &Value) -> Option<AgentUsage> {
    let incoming = match agent {
        AgentKind::Claude => parse_claude_rate_limit_event(raw),
        AgentKind::Antigravity => parse_generic_rate_limit(raw),
    };
    if incoming.is_empty() {
        return None;
    }
    let now = now_ms();
    let mut current = load_stored(hub, agent).unwrap_or(AgentUsage {
        agent,
        updated_at: now,
        windows: vec![],
        error: None,
        refreshable: can_refresh(hub, agent),
    });
    merge_windows(&mut current.windows, incoming, false, now);
    current.updated_at = now;
    current.error = None;
    current.refreshable = can_refresh(hub, agent);
    save_stored(hub, &current);
    hub.publish(Outbound::All(HubToApp::Usage {
        usage: current.clone(),
    }));
    Some(current)
}

pub fn record_limit_hit(hub: &Hub, agent: AgentKind, resets_at: Option<i64>, error: Option<&str>) {
    let now = now_ms();
    let resets_at = resets_at.map(normalize_epoch_ms).filter(|r| *r > now);
    let mut current = load_stored(hub, agent).unwrap_or(AgentUsage {
        agent,
        updated_at: now,
        windows: vec![],
        error: None,
        refreshable: can_refresh(hub, agent),
    });
    prune_expired(&mut current.windows, now);
    let key = match agent {
        AgentKind::Claude => {
            if let Some(r) = resets_at
                && let Some(matched) = current.windows.iter().find(|w| {
                    w.resets_at
                        .is_some_and(|wr| (wr - r).abs() <= SAME_WINDOW_TOLERANCE_MS)
                })
            {
                matched.key.clone()
            } else if current.windows.iter().any(|w| w.key == "five_hour") {
                "five_hour".to_string()
            } else {
                "limit".to_string()
            }
        }
        AgentKind::Antigravity => "quota".to_string(),
    };
    let detail = error
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| truncate_chars(s, 140));
    let incoming = vec![UsageWindow {
        label: window_label(&key),
        key,
        used_pct: Some(100.0),
        resets_at,
        blocked: true,
        detail,
    }];
    merge_windows(&mut current.windows, incoming, false, now);
    current.updated_at = now;
    current.refreshable = can_refresh(hub, agent);
    save_stored(hub, &current);
    hub.publish(Outbound::All(HubToApp::Usage { usage: current }));
}

pub fn note_turn_completed(hub: &Hub, agent: AgentKind) {
    let Some(mut current) = load_stored(hub, agent) else {
        return;
    };
    let before = current.windows.len();
    current.windows.retain(|w| {
        !((w.key == "quota" || w.key == "limit") && w.blocked && w.resets_at.is_none())
    });
    if current.windows.len() == before {
        return;
    }
    current.updated_at = now_ms();
    current.refreshable = can_refresh(hub, agent);
    save_stored(hub, &current);
    hub.publish(Outbound::All(HubToApp::Usage { usage: current }));
}

pub fn is_quota_error(error: Option<&str>) -> bool {
    let Some(e) = error else {
        return false;
    };
    let lower = e.to_ascii_lowercase();
    [
        "quota",
        "rate limit",
        "rate_limit",
        "resource_exhausted",
        "too many requests",
        "usage limit",
        "hit your limit",
        "429",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

pub async fn refresh_for(hub: &HubRef, agent: Option<AgentKind>) -> Result<Value> {
    match agent {
        Some(a) => Ok(serde_json::to_value(refresh(hub, a, true).await?)?),
        None => {
            let mut out = Vec::new();
            for a in AgentKind::ALL {
                if can_refresh(hub, a) {
                    out.push(refresh(hub, a, true).await?);
                } else if let Some(u) = load(hub, a, now_ms()) {
                    out.push(u);
                }
            }
            Ok(serde_json::to_value(out)?)
        }
    }
}

pub async fn refresh(hub: &HubRef, agent: AgentKind, manual: bool) -> Result<AgentUsage> {
    let now = now_ms();
    if agent != AgentKind::Antigravity {
        return Ok(load(hub, agent, now).unwrap_or(AgentUsage {
            agent,
            updated_at: 0,
            windows: vec![],
            error: None,
            refreshable: false,
        }));
    }
    let Some(machine) = agy_runner(hub) else {
        let mut current = load(hub, agent, now).unwrap_or(AgentUsage {
            agent,
            updated_at: 0,
            windows: vec![],
            error: None,
            refreshable: false,
        });
        current.refreshable = false;
        if manual {
            current.error = Some("No awake machine with Antigravity to check usage".into());
        }
        return Ok(current);
    };

    {
        let mut map = hub.usage.inner.lock().unwrap();
        let meta = map.entry(agent).or_default();
        if meta.in_flight {
            return Ok(load(hub, agent, now).unwrap_or(AgentUsage {
                agent,
                updated_at: 0,
                windows: vec![],
                error: None,
                refreshable: true,
            }));
        }
        if manual
            && meta.last_attempt_ms > 0
            && now - meta.last_attempt_ms < MANUAL_REFRESH_COOLDOWN_MS
        {
            return Ok(load(hub, agent, now).unwrap_or(AgentUsage {
                agent,
                updated_at: 0,
                windows: vec![],
                error: None,
                refreshable: true,
            }));
        }
        meta.in_flight = true;
        meta.last_attempt_ms = now;
    }
    let _guard = InFlightGuard {
        tracker: &hub.usage,
        agent,
    };

    let resp = hub
        .runner_request_timeout(
            machine,
            RunnerRequest::AgentUsage {
                agent: AgentKind::Antigravity,
            },
            RUNNER_USAGE_TIMEOUT,
        )
        .await;

    let done = now_ms();
    match resp.and_then(|v| serde_json::from_value::<AgentUsage>(v).map_err(Into::into)) {
        Ok(u) => Ok(ingest_usage(hub, u)),
        Err(e) => {
            let mut current = load_stored(hub, agent).unwrap_or(AgentUsage {
                agent,
                updated_at: 0,
                windows: vec![],
                error: None,
                refreshable: can_refresh(hub, agent),
            });
            prune_expired(&mut current.windows, done);
            current.error = Some(truncate_chars(&format!("{e:#}"), 140));
            current.refreshable = can_refresh(hub, agent);
            save_stored(hub, &current);
            hub.publish(Outbound::All(HubToApp::Usage {
                usage: current.clone(),
            }));
            Ok(current)
        }
    }
}

pub fn parse_claude_rate_limit_event(raw: &Value) -> Vec<UsageWindow> {
    let info = raw.get("rate_limit_info").unwrap_or(raw);
    let Some(obj) = info.as_object() else {
        return vec![];
    };
    let mut out = Vec::new();
    if let Some(many) = obj.get("unifiedWindows").and_then(Value::as_object)
        && !many.is_empty()
    {
        for (k, v) in many {
            if let Some(w) = parse_claude_event_window(k, v) {
                out.push(w);
            }
        }
    } else if obj.contains_key("status")
        || obj.contains_key("resetsAt")
        || obj.contains_key("resets_at")
        || obj.contains_key("utilization")
    {
        let key = obj
            .get("rateLimitType")
            .or_else(|| obj.get("rate_limit_type"))
            .and_then(Value::as_str)
            .unwrap_or("limit");
        if let Some(w) = parse_claude_event_window(key, info) {
            out.push(w);
        }
    }
    sort_windows(&mut out);
    out
}

fn parse_claude_event_window(key: &str, v: &Value) -> Option<UsageWindow> {
    let obj = v.as_object()?;
    let blocked = obj.get("status").and_then(Value::as_str) == Some("rejected");
    let mut used_pct = obj
        .get("utilization")
        .and_then(Value::as_f64)
        .filter(|u| u.is_finite())
        .map(|u| (if u <= 1.0 { u * 100.0 } else { u }).clamp(0.0, 100.0));
    if blocked && used_pct.is_none_or(|p| p == 0.0) {
        used_pct = Some(100.0);
    }
    let resets_at = obj
        .get("resetsAt")
        .or_else(|| obj.get("resets_at"))
        .and_then(parse_timestamp_value);
    if used_pct.is_none() && resets_at.is_none() && !blocked {
        return None;
    }
    Some(UsageWindow {
        key: key.to_string(),
        label: window_label(key),
        used_pct,
        resets_at,
        blocked,
        detail: None,
    })
}

fn parse_generic_rate_limit(raw: &Value) -> Vec<UsageWindow> {
    let Some(obj) = raw.as_object() else {
        return vec![];
    };
    let key = obj
        .get("key")
        .and_then(Value::as_str)
        .unwrap_or("quota")
        .to_string();
    let label = obj
        .get("label")
        .and_then(Value::as_str)
        .map(String::from)
        .unwrap_or_else(|| window_label(&key));
    let used_pct = obj
        .get("used_pct")
        .and_then(Value::as_f64)
        .filter(|u| u.is_finite())
        .map(|u| u.clamp(0.0, 100.0));
    let resets_at = obj.get("resets_at").and_then(parse_timestamp_value);
    let blocked = obj.get("blocked").and_then(Value::as_bool).unwrap_or(false);
    let detail = obj
        .get("detail")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| truncate_chars(s, 140));
    if used_pct.is_none() && resets_at.is_none() && !blocked {
        return vec![];
    }
    vec![UsageWindow {
        key,
        label,
        used_pct,
        resets_at,
        blocked,
        detail,
    }]
}

pub fn prune_expired(windows: &mut Vec<UsageWindow>, now: i64) {
    windows.retain_mut(|w| {
        if let Some(r) = w.resets_at
            && r <= now
        {
            if w.key == "quota" || w.key == "limit" {
                return false;
            }
            w.used_pct = Some(0.0);
            w.blocked = false;
            w.resets_at = None;
            w.detail = None;
        }
        true
    });
}

pub fn merge_windows(
    existing: &mut Vec<UsageWindow>,
    incoming: Vec<UsageWindow>,
    authoritative: bool,
    now: i64,
) {
    prune_expired(existing, now);
    if authoritative {
        let mut next = incoming;
        prune_expired(&mut next, now);
        sort_windows(&mut next);
        *existing = next;
        return;
    }
    let has_named_allowed = incoming
        .iter()
        .any(|w| w.key != "limit" && w.key != "quota" && !w.blocked);
    for mut w in incoming {
        if w.resets_at.is_some_and(|r| r <= now) {
            continue;
        }
        if w.blocked && w.used_pct.is_none_or(|p| p == 0.0) {
            w.used_pct = Some(100.0);
        }
        if let Some(cur) = existing.iter_mut().find(|e| e.key == w.key) {
            let same_period = match (cur.resets_at, w.resets_at) {
                (Some(a), Some(b)) => (a - b).abs() <= SAME_WINDOW_TOLERANCE_MS,
                _ => true,
            };
            let preserve_pct = !w.blocked
                && w.used_pct == Some(0.0)
                && same_period
                && cur.used_pct.is_some_and(|p| p > 0.0);
            cur.label = w.label;
            if !preserve_pct && w.used_pct.is_some() {
                cur.used_pct = w.used_pct;
            }
            if w.resets_at.is_some() {
                cur.resets_at = w.resets_at;
            }
            cur.blocked = w.blocked;
            if w.detail.is_some() {
                cur.detail = w.detail;
            } else if !w.blocked {
                cur.detail = None;
            }
        } else {
            existing.push(w);
        }
    }
    if has_named_allowed {
        existing.retain(|w| w.key != "limit");
    }
    prune_expired(existing, now);
    sort_windows(existing);
}

fn sort_windows(windows: &mut [UsageWindow]) {
    windows.sort_by(|a, b| window_order(&a.key).cmp(&window_order(&b.key)));
}

fn window_order(key: &str) -> (u8, &str) {
    let rank = match key {
        "five_hour" => 0,
        "seven_day" => 1,
        "seven_day_sonnet" => 2,
        "seven_day_opus" => 3,
        "seven_day_oauth_apps" => 4,
        "extra_usage" => 5,
        "gemini-5h" => 6,
        "gemini-weekly" => 7,
        "3p-5h" => 8,
        "3p-weekly" => 9,
        "quota" | "limit" => 10,
        _ => 11,
    };
    (rank, key)
}

pub fn window_label(key: &str) -> String {
    match key {
        "five_hour" => "5-hour".into(),
        "seven_day" => "Weekly".into(),
        "seven_day_sonnet" => "Weekly (Sonnet)".into(),
        "seven_day_opus" => "Weekly (Opus)".into(),
        "seven_day_oauth_apps" => "OAuth apps".into(),
        "extra_usage" => "Extra usage".into(),
        "gemini-5h" => "Gemini · 5-hour".into(),
        "gemini-weekly" => "Gemini · Weekly".into(),
        "3p-5h" => "Claude/GPT · 5-hour".into(),
        "3p-weekly" => "Claude/GPT · Weekly".into(),
        "quota" => "Quota".into(),
        "limit" => "Limit".into(),
        other => {
            let spaced = other.replace('_', " ");
            let mut chars = spaced.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => "Limit".into(),
            }
        }
    }
}

fn parse_timestamp_value(v: &Value) -> Option<i64> {
    if let Some(n) = v.as_i64() {
        return Some(normalize_epoch_ms(n));
    }
    if let Some(f) = v.as_f64()
        && f.is_finite()
        && f > 0.0
    {
        return Some(normalize_epoch_ms(f as i64));
    }
    v.as_str().and_then(parse_rfc3339_ms)
}

fn normalize_epoch_ms(ts: i64) -> i64 {
    if ts < 1_000_000_000_000 {
        ts.saturating_mul(1000)
    } else {
        ts
    }
}

pub fn parse_rfc3339_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    let bytes = s.as_bytes();
    if bytes.len() < 20 {
        return None;
    }
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b't' | b' ')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let year: i64 = s[0..4].parse().ok()?;
    let month: u32 = s[5..7].parse().ok()?;
    let day: u32 = s[8..10].parse().ok()?;
    let hour: i64 = s[11..13].parse().ok()?;
    let minute: i64 = s[14..16].parse().ok()?;
    let second: i64 = s[17..19].parse().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=60).contains(&second)
    {
        return None;
    }
    let mut rest = &s[19..];
    let mut ms: i64 = 0;
    if let Some(after_dot) = rest.strip_prefix('.') {
        let digits = after_dot.bytes().take_while(|b| b.is_ascii_digit()).count();
        if digits == 0 {
            return None;
        }
        let frac = &after_dot[..digits];
        let mut scale = 100;
        for b in frac.bytes().take(3) {
            ms += i64::from(b - b'0') * scale;
            scale /= 10;
        }
        rest = &after_dot[digits..];
    }
    let offset_secs: i64 = if rest.eq_ignore_ascii_case("z") {
        0
    } else {
        let sign_byte = rest.as_bytes().first()?;
        let sign = match sign_byte {
            b'+' => 1_i64,
            b'-' => -1_i64,
            _ => return None,
        };
        let tz = &rest[1..];
        let (oh, om) = if tz.len() == 5 && tz.as_bytes()[2] == b':' {
            (tz[0..2].parse::<i64>().ok()?, tz[3..5].parse::<i64>().ok()?)
        } else if tz.len() == 4 {
            (tz[0..2].parse::<i64>().ok()?, tz[2..4].parse::<i64>().ok()?)
        } else {
            return None;
        };
        if !(0..=23).contains(&oh) || !(0..=59).contains(&om) {
            return None;
        }
        sign * (oh * 3600 + om * 60)
    };
    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + hour * 3600 + minute * 60 + second - offset_secs;
    Some(secs * 1000 + ms)
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(month);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn truncate_chars(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rfc3339_timestamps_parse_to_epoch_ms() {
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339_ms("2026-04-11T07:00:00.528743+00:00"),
            Some(1_775_890_800_528)
        );
        assert_eq!(
            parse_rfc3339_ms("2026-04-11T09:00:00+02:00"),
            Some(1_775_890_800_000)
        );
        assert_eq!(
            parse_rfc3339_ms("2026-04-11T02:00:00-05:00"),
            Some(1_775_890_800_000)
        );
        assert_eq!(parse_rfc3339_ms("not-a-timestamp"), None);
    }

    #[test]
    fn passive_zero_utilization_preserves_existing_percentage_in_same_window() {
        let now = 1_775_880_000_000;
        let reset_5h = 1_775_890_800_000;
        let reset_7d = 1_776_400_000_000;
        let mut windows = vec![
            UsageWindow {
                key: "five_hour".into(),
                label: "5-hour".into(),
                used_pct: Some(42.0),
                resets_at: Some(reset_5h),
                blocked: false,
                detail: None,
            },
            UsageWindow {
                key: "seven_day".into(),
                label: "Weekly".into(),
                used_pct: Some(19.0),
                resets_at: Some(reset_7d),
                blocked: false,
                detail: None,
            },
        ];
        let event = json!({
            "rate_limit_info": {
                "rateLimitType": "five_hour",
                "resetsAt": reset_5h / 1000,
                "status": "allowed",
                "unifiedWindows": {
                    "five_hour": { "resetsAt": reset_5h / 1000, "status": "allowed", "utilization": 0 }
                }
            }
        });
        let incoming = parse_claude_rate_limit_event(&event);
        merge_windows(&mut windows, incoming, false, now);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].key, "five_hour");
        assert_eq!(windows[0].used_pct, Some(42.0));
        assert_eq!(windows[1].key, "seven_day");
        assert_eq!(windows[1].used_pct, Some(19.0));
    }

    #[test]
    fn passive_rejected_event_marks_blocked_and_100_pct() {
        let now: i64 = 1_775_880_000_000;
        let reset_5h: i64 = 1_775_890_800_000;
        let mut windows = vec![];
        let event = json!({
            "rate_limit_info": {
                "rateLimitType": "five_hour",
                "resetsAt": reset_5h / 1000,
                "status": "rejected",
                "unifiedWindows": {
                    "five_hour": { "resetsAt": reset_5h / 1000, "status": "rejected", "utilization": 0 },
                    "seven_day": { "resetsAt": (reset_5h / 1000) + 345_600, "status": "allowed", "utilization": 0.25 }
                }
            }
        });
        let incoming = parse_claude_rate_limit_event(&event);
        merge_windows(&mut windows, incoming, false, now);
        assert_eq!(windows[0].key, "five_hour");
        assert!(windows[0].blocked);
        assert_eq!(windows[0].used_pct, Some(100.0));
        assert_eq!(windows[1].key, "seven_day");
        assert!(!windows[1].blocked);
        assert_eq!(windows[1].used_pct, Some(25.0));
    }

    #[test]
    fn expired_windows_auto_clear_blocked_state_and_reset_usage() {
        let past = 1_775_880_000_000;
        let now = past + 60_000;
        let mut windows = vec![
            UsageWindow {
                key: "five_hour".into(),
                label: "5-hour".into(),
                used_pct: Some(100.0),
                resets_at: Some(past),
                blocked: true,
                detail: None,
            },
            UsageWindow {
                key: "quota".into(),
                label: "Quota".into(),
                used_pct: Some(100.0),
                resets_at: Some(past),
                blocked: true,
                detail: Some("Out of quota".into()),
            },
        ];
        prune_expired(&mut windows, now);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].key, "five_hour");
        assert_eq!(windows[0].used_pct, Some(0.0));
        assert!(!windows[0].blocked);
        assert_eq!(windows[0].resets_at, None);
    }
}
