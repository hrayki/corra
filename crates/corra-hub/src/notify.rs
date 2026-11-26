use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use corra_proto::app::HubToApp;
use corra_proto::*;
use p256::SecretKey;
use serde_json::{Value, json};

use crate::hub::{Hub, Outbound};

const VAPID: &str = "push.vapid";
const LOOKING_FOR: Duration = Duration::from_secs(150);
const BODY_CHARS: usize = 160;

#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub kind: &'static str,
    pub thread: ThreadId,
    pub title: String,
    pub body: String,
    pub resets_at: Option<i64>,
}

impl Notice {
    pub fn payload(&self) -> Value {
        json!({
            "kind": self.kind,
            "thread": self.thread,
            "title": self.title,
            "body": self.body,
            "resets_at": self.resets_at,
        })
    }
}

pub struct Facts<'a> {
    pub last_message: Option<&'a str>,
    pub queued: bool,
    pub quiet: bool,
    pub followed: bool,
}

fn clip(text: &str) -> String {
    let flat = text
        .lines()
        .map(|l| l.trim().trim_start_matches(['#', '>', '-', '*']).trim())
        .filter(|l| !l.is_empty() && !l.starts_with("```"))
        .collect::<Vec<_>>()
        .join(" ")
        .replace("**", "")
        .replace('`', "");
    if flat.chars().count() > BODY_CHARS {
        let mut s: String = flat.chars().take(BODY_CHARS - 1).collect();
        s.push('…');
        s
    } else {
        flat
    }
}

fn asks(request: &AgentRequest) -> String {
    match request {
        AgentRequest::ToolApproval { tool, input, .. } => match (tool.as_str(), input) {
            ("Bash", i) if i["command"].is_string() => {
                format!(
                    "Wants to run {}",
                    clip(i["command"].as_str().unwrap_or_default())
                )
            }
            ("Edit" | "Write" | "MultiEdit" | "NotebookEdit", i) if i["file_path"].is_string() => {
                let path = i["file_path"].as_str().unwrap_or_default();
                format!("Wants to edit {}", path.rsplit('/').next().unwrap_or(path))
            }
            _ => format!("Wants to use {tool}"),
        },
        AgentRequest::Question { questions } => match questions.first() {
            Some(q) => clip(&q.question),
            None => "Has a question".into(),
        },
        AgentRequest::PlanApproval { .. } => "Has a plan for you to approve".into(),
        AgentRequest::HubAction { action, input } if action == "secret.request" => {
            format!(
                "Asks for a secret, {}",
                input["name"].as_str().unwrap_or("unnamed")
            )
        }
        AgentRequest::HubAction { action, input } if action == "git.push" => {
            format!(
                "Wants to push to {}",
                input["repo"].as_str().unwrap_or("a repo")
            )
        }
        AgentRequest::HubAction { action, .. } => format!("Wants your yes for {action}"),
    }
}

pub fn notice(t: &Thread, event: &Event, facts: &Facts) -> Option<Notice> {
    if t.hidden_at.is_some() {
        return None;
    }
    let (kind, body, resets_at) = match event {
        Event::Request { request, .. } => ("needs", asks(request), None),
        Event::TurnCompleted {
            outcome: TurnOutcome::Completed,
            ..
        } => {
            if facts.queued || facts.quiet || facts.followed {
                return None;
            }
            let body = facts
                .last_message
                .map(clip)
                .filter(|b| !b.is_empty())
                .unwrap_or_else(|| "Finished".into());
            ("done", body, None)
        }
        Event::TurnCompleted {
            outcome: TurnOutcome::Failed,
            error,
            resets_at,
            ..
        } => {
            if facts.quiet {
                return None;
            }
            let body = match (resets_at, error) {
                (Some(_), _) => "Hit the usage limit".to_string(),
                (None, Some(e)) => clip(e),
                (None, None) => "Stopped with an error".into(),
            };
            ("failed", body, *resets_at)
        }
        _ => return None,
    };
    Some(Notice {
        kind,
        thread: t.id,
        title: if t.title.trim().is_empty() {
            "A thread".into()
        } else {
            t.title.clone()
        },
        body,
        resets_at,
    })
}

pub fn enabled(hub: &Hub, kind: &str) -> bool {
    hub.settings()["notify"][kind].as_bool().unwrap_or(true)
}

pub fn set_presence(hub: &Hub, conn: u64, focused: bool) {
    let mut p = hub.presence.lock().unwrap();
    if focused {
        p.insert(conn, Instant::now());
    } else {
        p.remove(&conn);
    }
}

pub fn someone_looking(hub: &Hub) -> bool {
    let mut p = hub.presence.lock().unwrap();
    p.retain(|_, at| at.elapsed() < LOOKING_FOR);
    !p.is_empty()
}

pub fn on_event(hub: &Hub, thread: ThreadId, event: &Event) {
    if !matches!(event, Event::Request { .. } | Event::TurnCompleted { .. }) {
        return;
    }
    let Ok(Some(t)) = hub.db.thread(thread) else {
        return;
    };
    let last = match event {
        Event::TurnCompleted {
            turn,
            outcome: TurnOutcome::Completed,
            ..
        } => hub
            .db
            .events_tail(thread, 200)
            .ok()
            .and_then(|tail| crate::follow::last_message(&tail, turn)),
        _ => None,
    };
    let facts = Facts {
        last_message: last.as_deref(),
        queued: !hub.queued(thread).is_empty(),
        quiet: crate::schedule::is_quiet_turn(hub, thread),
        followed: hub.follower(thread).is_some_and(|f| f != thread),
    };
    let Some(n) = notice(&t, event, &facts) else {
        return;
    };
    if !enabled(hub, n.kind) || someone_looking(hub) {
        return;
    }
    deliver(hub, &n, None);
}

pub fn deliver(hub: &Hub, n: &Notice, session: Option<&str>) {
    let msg = HubToApp::Notify {
        notice: n.payload(),
    };
    hub.publish(match session {
        Some(s) => Outbound::Session(s.to_string(), msg),
        None => Outbound::All(msg),
    });
    let Some(hub) = hub.me() else {
        return;
    };
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let payload = n.payload().to_string();
    let urgent = n.kind == "needs";
    let topic = n.thread.simple().to_string();
    let only = session.map(String::from);
    rt.spawn(async move {
        for (s, sub) in subscriptions(&hub) {
            if only.as_ref().is_some_and(|o| *o != s) {
                continue;
            }
            if let Err(e) = send(&hub, &s, &sub, &payload, urgent, &topic).await {
                tracing::warn!("couldn't send a notification: {e:#}");
            }
        }
    });
}

fn vapid_key(hub: &Hub) -> Result<SecretKey> {
    if let Some(saved) = hub.secrets.get(VAPID)? {
        return SecretKey::from_slice(&crate::webpush::unb64(&saved)?)
            .context("the saved push key is broken");
    }
    let key = crate::webpush::new_key();
    hub.secrets
        .set(VAPID, &crate::webpush::b64(&key.to_bytes()))?;
    Ok(key)
}

pub fn public_key(hub: &Hub) -> Result<String> {
    Ok(crate::webpush::b64(&crate::webpush::public_bytes(
        &vapid_key(hub)?,
    )))
}

fn sub_key(session: &str) -> String {
    format!("push:{session}")
}

pub fn subscribe(hub: &Hub, session: &str, endpoint: &str, p256dh: &str, auth: &str) -> Result<()> {
    let url = reqwest::Url::parse(endpoint).context("the push endpoint isn't a URL")?;
    if url.scheme() != "https" {
        bail!("the push endpoint must be https");
    }
    if crate::webpush::unb64(p256dh)?.len() != 65 || crate::webpush::unb64(auth)?.len() != 16 {
        bail!("the browser's push keys have the wrong length");
    }
    hub.db.kv_set(
        &sub_key(session),
        &json!({ "endpoint": endpoint, "p256dh": p256dh, "auth": auth, "at": now_ms() })
            .to_string(),
    )
}

pub fn unsubscribe(hub: &Hub, session: &str) -> Result<()> {
    hub.db.kv_delete(&sub_key(session))
}

pub fn subscribed(hub: &Hub, session: &str) -> bool {
    matches!(hub.db.kv_get(&sub_key(session)), Ok(Some(_)))
}

fn subscriptions(hub: &Hub) -> Vec<(String, Value)> {
    let Ok(keys) = hub.db.kv_keys("push:") else {
        return vec![];
    };
    keys.into_iter()
        .filter_map(|k| {
            let session = k.strip_prefix("push:")?.to_string();
            let v: Value = serde_json::from_str(&hub.db.kv_get(&k).ok()??).ok()?;
            Some((session, v))
        })
        .collect()
}

async fn send(
    hub: &Hub,
    session: &str,
    sub: &Value,
    payload: &str,
    urgent: bool,
    topic: &str,
) -> Result<()> {
    let endpoint = sub["endpoint"].as_str().context("no endpoint")?;
    let ua = crate::webpush::unb64(sub["p256dh"].as_str().unwrap_or_default())?;
    let auth = crate::webpush::unb64(sub["auth"].as_str().unwrap_or_default())?;
    let key = vapid_key(hub)?;
    let body = crate::webpush::encrypt(
        &ua,
        &auth,
        payload.as_bytes(),
        &crate::webpush::new_key(),
        rand::random(),
    )?;
    let subject = hub.providers.hub_url.clone();
    let auth_header = crate::webpush::vapid(&key, endpoint, &subject, now_ms() / 1000)?;
    let res = hub
        .http
        .post(endpoint)
        .header("authorization", auth_header)
        .header("content-encoding", "aes128gcm")
        .header("content-type", "application/octet-stream")
        .header("ttl", "86400")
        .header("urgency", if urgent { "high" } else { "normal" })
        .header("topic", topic)
        .body(body)
        .timeout(Duration::from_secs(20))
        .send()
        .await?;
    let status = res.status();
    if status.as_u16() == 404 || status.as_u16() == 410 {
        tracing::info!("a browser's push subscription has ended; dropping it");
        let _ = unsubscribe(hub, session);
        return Ok(());
    }
    if !status.is_success() {
        let text = res.text().await.unwrap_or_default();
        bail!(
            "the push service said {status}: {}",
            text.chars().take(200).collect::<String>()
        );
    }
    Ok(())
}

pub fn test(hub: &Hub, session: &str) {
    deliver(
        hub,
        &Notice {
            kind: "test",
            thread: ThreadId::nil(),
            title: "Corra".into(),
            body: "Notifications work on this device.".into(),
            resets_at: None,
        },
        Some(session),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread() -> Thread {
        Thread {
            id: ThreadId::nil(),
            project: uuid::Uuid::nil(),
            title: "Fix the login".into(),
            agent: AgentKind::Claude,
            machine: None,
            session_id: None,
            cwd: String::new(),
            settings: AgentSettings::default(),
            status: ThreadStatus::Idle,
            last_seq: 0,
            created_at: 0,
            updated_at: 0,
            pinned_at: None,
            settled_at: None,
            hidden_at: None,
            started_by: None,
        }
    }

    fn facts() -> Facts<'static> {
        Facts {
            last_message: None,
            queued: false,
            quiet: false,
            followed: false,
        }
    }

    fn done(outcome: TurnOutcome, error: Option<&str>, resets_at: Option<i64>) -> Event {
        Event::TurnCompleted {
            turn: "t1".into(),
            outcome,
            usage: None,
            error: error.map(String::from),
            anchor: None,
            resets_at,
        }
    }

    #[test]
    fn a_finished_turn_says_its_last_message_in_one_short_line() {
        let f = Facts {
            last_message: Some("## Done\n\nI fixed **the login** in `auth.rs`.\n\n```\ncode\n```"),
            ..facts()
        };
        let n = notice(&thread(), &done(TurnOutcome::Completed, None, None), &f).unwrap();
        assert_eq!(n.kind, "done");
        assert_eq!(n.title, "Fix the login");
        assert_eq!(n.body, "Done I fixed the login in auth.rs. code");
    }

    #[test]
    fn no_finish_while_the_queue_has_more_or_for_compaction_or_a_followed_thread() {
        let e = done(TurnOutcome::Completed, None, None);
        for f in [
            Facts {
                queued: true,
                ..facts()
            },
            Facts {
                quiet: true,
                ..facts()
            },
            Facts {
                followed: true,
                ..facts()
            },
        ] {
            assert_eq!(notice(&thread(), &e, &f), None);
        }
    }

    #[test]
    fn a_stopped_turn_and_a_trashed_thread_say_nothing() {
        assert_eq!(
            notice(
                &thread(),
                &done(TurnOutcome::Interrupted, None, None),
                &facts()
            ),
            None
        );
        let mut t = thread();
        t.hidden_at = Some(1);
        let ask = Event::Request {
            id: "r".into(),
            turn: "t1".into(),
            request: AgentRequest::PlanApproval { plan: "p".into() },
        };
        assert_eq!(notice(&t, &ask, &facts()), None);
    }

    #[test]
    fn a_usage_limit_carries_its_reset_time() {
        let n = notice(
            &thread(),
            &done(
                TurnOutcome::Failed,
                Some("You've hit your limit"),
                Some(1_795_526_865_000),
            ),
            &facts(),
        )
        .unwrap();
        assert_eq!(
            (n.kind, n.body.as_str(), n.resets_at),
            ("failed", "Hit the usage limit", Some(1_795_526_865_000))
        );
    }

    #[test]
    fn requests_say_what_they_want() {
        let ask = |request| Event::Request {
            id: "r".into(),
            turn: "t1".into(),
            request,
        };
        let body = |request| notice(&thread(), &ask(request), &facts()).unwrap().body;
        assert_eq!(
            body(AgentRequest::ToolApproval {
                tool: "Bash".into(),
                input: json!({ "command": "rm -rf build\nls" }),
                reason: None,
                tool_call: None,
            }),
            "Wants to run rm -rf build ls"
        );
        assert_eq!(
            body(AgentRequest::ToolApproval {
                tool: "Edit".into(),
                input: json!({ "file_path": "/src/auth.rs" }),
                reason: None,
                tool_call: None,
            }),
            "Wants to edit auth.rs"
        );
        assert_eq!(
            body(AgentRequest::HubAction {
                action: "git.push".into(),
                input: json!({ "repo": "acme/app" }),
            }),
            "Wants to push to acme/app"
        );
        assert_eq!(
            body(AgentRequest::HubAction {
                action: "secret.request".into(),
                input: json!({ "name": "STRIPE_KEY" }),
            }),
            "Asks for a secret, STRIPE_KEY"
        );
    }
}
