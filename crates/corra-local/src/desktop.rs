use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use corra_proto::*;
use serde_json::{Value, json};

use crate::{HubCfg, StateRef};

const OFFLINE_AFTER: Duration = Duration::from_secs(60);
const AT_START: Duration = Duration::from_secs(10);
const CHECK_EVERY: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Starting,
    Connect(String),
    Hub(String),
    Offline(String),
}

impl Target {
    pub fn url(&self) -> Option<&str> {
        match self {
            Target::Starting => None,
            Target::Connect(u) | Target::Hub(u) | Target::Offline(u) => Some(u),
        }
    }
}

fn fragment(v: Value) -> String {
    let s = v.to_string();
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn hub_target(s: &StateRef, c: &HubCfg) -> Target {
    let f = fragment(json!({ "token": c.token, "machine": c.machine, "connect": s.connect_url() }));
    Target::Hub(format!("{}/#desktop={f}", c.url))
}

fn offline_target(s: &StateRef, hub: &str) -> Target {
    let f = fragment(json!({
        "token": s.session,
        "machine": s.machine,
        "connect": s.connect_url(),
        "offline": hub,
    }));
    Target::Offline(format!("{}/#desktop={f}", s.base))
}

fn set(s: &StateRef, t: Target) {
    s.target.send_if_modified(|cur| {
        if *cur == t {
            false
        } else {
            tracing::info!("the window shows {}", t.url().unwrap_or("nothing"));
            *cur = t;
            true
        }
    });
}

pub fn normalize(url: &str) -> Result<String> {
    let u = url.trim().trim_end_matches('/');
    if u.is_empty() {
        bail!("enter the hub's address");
    }
    Ok(if u.starts_with("http://") || u.starts_with("https://") {
        u.to_string()
    } else {
        format!("https://{u}")
    })
}

async fn post(s: &StateRef, url: &str, token: Option<&str>, body: Value) -> Result<Value> {
    let mut req = s.http.post(url).json(&body);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let r = req
        .send()
        .await
        .map_err(|e| anyhow!("can't reach the hub: {}", e.without_url()))?;
    let status = r.status();
    let v: Value = r.json().await.unwrap_or_default();
    if status == reqwest::StatusCode::UNAUTHORIZED && token.is_some() {
        bail!("signed out");
    }
    if !status.is_success() {
        match v["error"].as_str() {
            Some(e) => bail!("{e}"),
            None => bail!("that doesn't look like a corra hub (it answered {status})"),
        }
    }
    Ok(v)
}

async fn register(s: &StateRef, url: &str, token: &str) -> Result<HubCfg> {
    let v = post(
        s,
        &format!("{url}/api/machine"),
        Some(token),
        json!({ "install_id": s.install_id(), "name": crate::hostname() }),
    )
    .await?;
    Ok(HubCfg {
        url: url.to_string(),
        token: token.to_string(),
        machine: v["machine"]
            .as_str()
            .and_then(|x| x.parse().ok())
            .ok_or_else(|| anyhow!("the hub gave a bad answer"))?,
        machine_token: v["token"]
            .as_str()
            .ok_or_else(|| anyhow!("the hub gave a bad answer"))?
            .to_string(),
    })
}

pub async fn sign_in(s: &StateRef, url: &str, key: &str, code: &str) -> Result<()> {
    let url = normalize(url)?;
    let v = if !code.trim().is_empty() {
        post(
            s,
            &format!("{url}/auth/pair/finish"),
            None,
            json!({ "code": code.trim(), "name": crate::hostname() }),
        )
        .await?
    } else if !key.trim().is_empty() {
        post(
            s,
            &format!("{url}/auth/key"),
            None,
            json!({ "key": key.trim(), "device": crate::hostname() }),
        )
        .await?
    } else {
        bail!("enter the hub's key or a pairing code");
    };
    let token = v["token"]
        .as_str()
        .ok_or_else(|| anyhow!("that doesn't look like a corra hub"))?;
    let cfg = register(s, &url, token).await?;
    s.set_cfg(Some(&cfg));
    Ok(())
}

async fn reachable(s: &StateRef, c: &HubCfg) -> Result<bool> {
    let ok = s
        .http
        .get(format!("{}/healthz", c.url))
        .timeout(Duration::from_secs(8))
        .send()
        .await
        .is_ok_and(|r| r.status().is_success());
    if !ok {
        return Ok(false);
    }
    let r = s
        .http
        .get(format!("{}/auth/state", c.url))
        .bearer_auth(&c.token)
        .timeout(Duration::from_secs(8))
        .send()
        .await?;
    let v: Value = r.json().await.unwrap_or_default();
    if v["signed_in"] == false {
        bail!("signed out");
    }
    Ok(true)
}

fn run_against(s: &StateRef, c: &HubCfg) {
    let mut slot = s.runner.lock().unwrap();
    if slot
        .as_ref()
        .is_some_and(|(cfg, h)| cfg == c && !h.is_finished())
    {
        return;
    }
    if let Some((_, old)) = slot.take() {
        old.abort();
    }
    s.runner_up
        .store(false, std::sync::atomic::Ordering::Relaxed);
    let opts = corra_runner::Options {
        hub: c.url.clone(),
        token: c.machine_token.clone(),
        sync: true,
        update: false,
        connected: Some(s.runner_up.clone()),
    };
    let handle = tokio::spawn(async move {
        if let Err(e) = corra_runner::run(opts).await {
            tracing::error!("the runner stopped: {e:#}");
        }
    });
    *slot = Some((c.clone(), handle));
}

fn stop_runner(s: &StateRef) {
    if let Some((_, h)) = s.runner.lock().unwrap().take() {
        h.abort();
    }
}

pub fn spawn(s: StateRef) {
    tokio::spawn(async move {
        let mut registered: Option<String> = None;
        let fresh = || {
            Instant::now()
                .checked_sub(OFFLINE_AFTER - AT_START)
                .unwrap_or_else(Instant::now)
        };
        let mut last_up = fresh();
        loop {
            let Some(mut c) = s.cfg() else {
                stop_runner(&s);
                registered = None;
                last_up = fresh();
                set(&s, Target::Connect(s.connect_url()));
                s.wake.notified().await;
                continue;
            };
            let up = match reachable(&s, &c).await {
                Ok(up) => up,
                Err(e) if e.to_string() == "signed out" => {
                    tracing::warn!("this computer was signed out of {}", c.url);
                    s.set_cfg(None);
                    continue;
                }
                Err(_) => false,
            };
            if up && registered.as_deref() != Some(c.url.as_str()) {
                match register(&s, &c.url, &c.token).await {
                    Ok(fresh) => {
                        s.set_cfg(Some(&fresh));
                        c = fresh;
                        registered = Some(c.url.clone());
                    }
                    Err(e) if e.to_string() == "signed out" => {
                        s.set_cfg(None);
                        continue;
                    }
                    Err(e) => tracing::warn!("couldn't register this computer: {e:#}"),
                }
            }
            if registered.is_some() {
                run_against(&s, &c);
            }
            let runner_up = s.runner_up.load(std::sync::atomic::Ordering::Relaxed);
            if up && registered.is_some() && runner_up {
                last_up = Instant::now();
                if let Err(e) = hand_over(&s, &c).await {
                    tracing::warn!("couldn't hand the offline threads over: {e:#}");
                }
                if waiting(&s) == 0 {
                    set(&s, hub_target(&s, &c));
                }
            } else if last_up.elapsed() >= OFFLINE_AFTER {
                set(&s, offline_target(&s, &c.url));
            } else if up && registered.is_some() {
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
            tokio::select! {
                _ = tokio::time::sleep(CHECK_EVERY) => {}
                _ = s.wake.notified() => {}
            }
        }
    });
}

pub(crate) fn waiting(s: &StateRef) -> usize {
    s.hub.db.threads().map(|t| t.len()).unwrap_or(0)
}

fn busy(t: &Thread) -> bool {
    matches!(
        t.status,
        ThreadStatus::Running | ThreadStatus::Waiting | ThreadStatus::Starting
    )
}

pub async fn hand_over(s: &StateRef, c: &HubCfg) -> Result<usize> {
    let hub = &s.hub;
    let threads: Vec<Thread> = hub.db.threads()?.into_iter().filter(|t| !busy(t)).collect();
    if threads.is_empty() {
        return Ok(0);
    }
    let mut batch = vec![];
    for t in &threads {
        hub.stop_live(t);
        let p = hub.db.project(t.project)?;
        let mut events = vec![];
        loop {
            let after = events.last().map(|e: &StoredEvent| e.seq).unwrap_or(0);
            let more = hub.db.events_after(t.id, after, 2000)?;
            if more.is_empty() {
                break;
            }
            events.extend(more);
        }
        batch.push(json!({
            "thread": t,
            "project": {
                "name": p.as_ref().map(|p| p.name.clone()).unwrap_or_else(|| "local".into()),
                "path": p.as_ref().and_then(|p| p.path.clone()).or_else(|| Some(t.cwd.clone())),
                "settings": p.as_ref().map(|p| p.settings.clone()).unwrap_or_default(),
            },
            "events": events,
        }));
    }
    post(
        s,
        &format!("{}/runner/import", c.url),
        Some(&c.machine_token),
        json!(batch),
    )
    .await?;
    let sync = corra_runner::sync::Syncer::new(&c.url, &c.machine_token);
    for t in &threads {
        if let Some(sid) = &t.session_id {
            let cwd = corra_runner::files::expand(&t.cwd);
            let (base, files) = corra_runner::agents::session_files(t.agent, &cwd, sid);
            if !files.is_empty()
                && let Err(e) = sync.upload(t.id, &base, files).await
            {
                tracing::warn!(thread = %t.id, "couldn't copy the session to the hub: {e:#}");
            }
        }
        hub.remove_thread(t.id)?;
    }
    tracing::info!("handed {} threads over to {}", threads.len(), c.url);
    Ok(threads.len())
}
