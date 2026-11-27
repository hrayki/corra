use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use corra_proto::{HubFailure, HubVersion, now_ms};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::hub::{Hub, HubRef};

pub const VERSION: &str = match option_env!("CORRA_VERSION") {
    Some(v) => v,
    None => "dev",
};

const RELEASES: &str = "https://github.com/hrayki/corra/releases";
const EVERY: Duration = Duration::from_secs(3600);
const FRESH_MS: i64 = 60_000;

struct Latest {
    version: Option<String>,
    at: Option<i64>,
    error: Option<String>,
}

static LATEST: Mutex<Latest> = Mutex::new(Latest {
    version: None,
    at: None,
    error: None,
});

fn launcher() -> Option<PathBuf> {
    std::env::var_os("CORRA_LAUNCHER")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

fn releases() -> String {
    std::env::var("CORRA_RELEASES")
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| RELEASES.into())
}

pub fn valid_version(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 64
        && v.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

pub fn newer(latest: &str, current: &str) -> bool {
    current == "dev" || latest > current
}

pub fn spawn(hub: HubRef) {
    if launcher().is_none() {
        return;
    }
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        loop {
            check(&hub).await;
            tokio::time::sleep(EVERY).await;
        }
    });
}

async fn newest(hub: &Hub) -> Result<String> {
    let url = format!("{}/latest/download/VERSION", releases());
    let r = hub
        .http
        .get(&url)
        .timeout(Duration::from_secs(20))
        .send()
        .await?;
    if !r.status().is_success() {
        bail!("{url} answered {}", r.status());
    }
    let v = r.text().await?.trim().to_string();
    if !valid_version(&v) {
        bail!("the newest release names itself {v:?}, which isn't a version");
    }
    Ok(v)
}

async fn check(hub: &Hub) {
    let r = newest(hub).await;
    let mut l = LATEST.lock().unwrap();
    l.at = Some(now_ms());
    match r {
        Ok(v) => {
            l.version = Some(v);
            l.error = None;
        }
        Err(e) => {
            tracing::warn!("couldn't check for a newer hub: {e:#}");
            l.error = Some(format!("{e:#}"));
        }
    }
}

async fn ask(req: Value, wait: Duration) -> Result<Value> {
    let path = launcher().ok_or_else(|| anyhow!("this hub isn't run by corra's launcher"))?;
    let talk = async {
        let mut s = tokio::net::UnixStream::connect(&path).await?;
        s.write_all(format!("{req}\n").as_bytes()).await?;
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line).await?;
        anyhow::Ok(line)
    };
    let line = tokio::time::timeout(wait, talk)
        .await
        .map_err(|_| anyhow!("the launcher didn't answer"))??;
    let v: Value = serde_json::from_str(line.trim())
        .map_err(|_| anyhow!("the launcher's answer wasn't readable"))?;
    if let Some(e) = v["error"].as_str() {
        bail!("{e}");
    }
    Ok(v)
}

fn commit(version: &str) -> Option<&str> {
    let sha = version.rsplit('-').next()?;
    (sha.len() >= 7
        && sha.len() <= 40
        && sha.chars().all(|c| c.is_ascii_hexdigit())
        && sha != version)
        .then_some(sha)
}

pub fn changes(releases: &str, from: &str, to: &str) -> Option<String> {
    let repo = releases.strip_suffix("/releases")?;
    if !repo.starts_with("https://github.com/") {
        return None;
    }
    Some(format!(
        "{repo}/compare/{}...{}",
        commit(from)?,
        commit(to)?
    ))
}

pub async fn status(hub: &Hub, check_now: bool) -> HubVersion {
    let mut out = HubVersion {
        version: VERSION.to_string(),
        managed: launcher().is_some(),
        latest: None,
        update: None,
        changes: None,
        checked_at: None,
        check_error: None,
        previous: None,
        trial: false,
        failed: None,
        launcher_error: None,
    };
    if !out.managed {
        return out;
    }
    let stale = LATEST
        .lock()
        .unwrap()
        .at
        .is_none_or(|at| check_now || now_ms() - at > FRESH_MS);
    if stale {
        let _ = tokio::time::timeout(Duration::from_secs(10), check(hub)).await;
    }
    {
        let l = LATEST.lock().unwrap();
        out.latest = l.version.clone();
        out.checked_at = l.at;
        out.check_error = l.error.clone();
        out.update = l.version.clone().filter(|v| newer(v, VERSION));
        out.changes = out
            .update
            .as_deref()
            .and_then(|to| changes(&releases(), VERSION, to));
    }
    match ask(json!({ "op": "status" }), Duration::from_secs(10)).await {
        Ok(v) => {
            let st = &v["state"];
            out.previous = st["previous"].as_str().map(str::to_string);
            out.trial =
                st["trial"].as_bool().unwrap_or(false) && st["current"].as_str() == Some(VERSION);
            out.failed = serde_json::from_value::<HubFailure>(st["failed"].clone()).ok();
        }
        Err(e) => out.launcher_error = Some(format!("{e:#}")),
    }
    out
}

pub async fn switch(version: &str) -> Result<()> {
    let version = version.trim();
    if !valid_version(version) {
        bail!("{version:?} isn't a version");
    }
    if version == VERSION {
        bail!("the hub already runs {version}");
    }
    tracing::info!("switching the hub to {version}");
    ask(
        json!({ "op": "switch", "version": version }),
        Duration::from_secs(900),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_later_release_is_an_update() {
        assert!(newer("2026.11.27-0900-bbbbbbb", "2026.11.26-1432-aaaaaaa"));
        assert!(!newer("2026.11.26-1432-aaaaaaa", "2026.11.26-1432-aaaaaaa"));
        assert!(!newer("2026.11.25-2359-ccccccc", "2026.11.26-0000-aaaaaaa"));
        assert!(newer("2026.11.25-2359-ccccccc", "dev"));
    }

    #[test]
    fn what_changed_links_to_the_two_commits() {
        let gh = "https://github.com/hrayki/corra/releases";
        assert_eq!(
            changes(gh, "2026.11.26-143205-6793d3e", "2026.11.27-090000-abc1234").as_deref(),
            Some("https://github.com/hrayki/corra/compare/6793d3e...abc1234")
        );
        assert_eq!(changes(gh, "dev", "2026.11.27-090000-abc1234"), None);
        assert_eq!(
            changes("http://127.0.0.1:7862/releases", "a-6793d3e", "b-abc1234"),
            None
        );
    }

    #[test]
    fn versions_are_plain_names() {
        assert!(valid_version("2026.11.26-1432-6793d3e"));
        assert!(!valid_version("../x"));
        assert!(!valid_version(""));
    }
}
