use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use corra_proto::runner::RunnerRequest;
use corra_proto::*;
use serde::Serialize;

use crate::auth::AppState;
use crate::hub::Hub;

#[derive(Clone, Serialize)]
pub struct SetupInfo {
    pub name: String,
    pub repos: Vec<String>,
    pub env: Vec<String>,
}

pub struct Kit {
    pub repo: String,
    pub version: String,
    pub tar: Vec<u8>,
    pub setups: Vec<SetupInfo>,
}

static CACHE: Mutex<Option<(Instant, Arc<Kit>)>> = Mutex::new(None);

fn lines(s: &str) -> Vec<String> {
    s.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect()
}

pub async fn current(hub: &Hub) -> Result<Option<Arc<Kit>>> {
    let Some(repo) = hub.setting_str("setup_repo") else {
        return Ok(None);
    };
    let repo = repo.trim_end_matches(".git").to_string();
    let cached = CACHE.lock().unwrap().clone();
    if let Some((at, k)) = &cached
        && k.repo == repo
        && at.elapsed() < Duration::from_secs(60)
    {
        return Ok(Some(k.clone()));
    }
    let Some(token) = hub.secrets.get("github.token")? else {
        bail!("add github.token in Settings → Credentials to use a setup repo");
    };
    let gh = |url: String| {
        hub.http
            .get(url)
            .bearer_auth(&token)
            .header("user-agent", "corra")
            .header("accept", "application/vnd.github+json")
    };
    let commits: serde_json::Value = gh(format!(
        "https://api.github.com/repos/{repo}/commits?path=corra&per_page=1"
    ))
    .send()
    .await?
    .error_for_status()?
    .json()
    .await?;
    let Some(version) = commits[0]["sha"].as_str().map(String::from) else {
        return Ok(None);
    };
    if let Some((_, k)) = cached
        && k.repo == repo
        && k.version == version
    {
        *CACHE.lock().unwrap() = Some((Instant::now(), k.clone()));
        return Ok(Some(k));
    }
    let tarball = gh(format!(
        "https://api.github.com/repos/{repo}/tarball/{version}"
    ))
    .send()
    .await?
    .error_for_status()?
    .bytes()
    .await?;
    let (tar, setups) = tokio::task::spawn_blocking(move || extract(&tarball)).await??;
    let kit = Arc::new(Kit {
        repo,
        version,
        tar,
        setups,
    });
    *CACHE.lock().unwrap() = Some((Instant::now(), kit.clone()));
    Ok(Some(kit))
}

pub fn extract(tarball: &[u8]) -> Result<(Vec<u8>, Vec<SetupInfo>)> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(tarball));
    let out = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut builder = tar::Builder::new(out);
    let mut setups: std::collections::BTreeMap<String, SetupInfo> = Default::default();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        let mut parts = path.components();
        parts.next();
        let rest = parts.as_path();
        let Ok(inside) = rest.strip_prefix("corra") else {
            continue;
        };
        if inside.as_os_str().is_empty() {
            continue;
        }
        let kind = entry.header().entry_type();
        let mut data = vec![];
        if kind.is_file() {
            entry.read_to_end(&mut data)?;
        }
        let names: Vec<String> = inside
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect();
        if names.len() >= 2 && names[0] == "setups" {
            let s = setups.entry(names[1].clone()).or_insert_with(|| SetupInfo {
                name: names[1].clone(),
                repos: vec![],
                env: vec![],
            });
            if names.len() == 3 && kind.is_file() {
                let text = String::from_utf8_lossy(&data);
                match names[2].as_str() {
                    "repos" => s.repos = lines(&text),
                    "env" => s.env = lines(&text),
                    _ => {}
                }
            }
        }
        let mut h = entry.header().clone();
        if kind.is_file() {
            h.set_size(data.len() as u64);
            h.set_cksum();
            builder.append_data(&mut h, inside, data.as_slice())?;
        } else if kind.is_dir() {
            builder.append_data(&mut h, inside, std::io::empty())?;
        }
    }
    let tar = builder.into_inner()?.finish()?;
    Ok((tar, setups.into_values().collect()))
}

pub fn machine_setups(hub: &Hub, machine: MachineId) -> Vec<String> {
    let project = hub
        .db
        .machine(machine)
        .ok()
        .flatten()
        .and_then(|m| m.project);
    let mut names: Vec<String> = hub
        .db
        .projects()
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.default_machine == Some(machine) || project == Some(p.id))
        .filter_map(|p| p.setup)
        .collect();
    names.sort();
    names.dedup();
    names
}

pub fn version_for(kit: &Kit, setups: &[String]) -> String {
    format!("{}+{}", kit.version, setups.join(","))
}

pub fn only(tar: &[u8], setups: &[String]) -> Result<Vec<u8>> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(tar));
    let out = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut builder = tar::Builder::new(out);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        let names: Vec<String> = path
            .components()
            .filter(|c| !matches!(c, std::path::Component::CurDir))
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect();
        let keep = match names.as_slice() {
            [top, ..] if top == "shared" => true,
            [top] if top == "setups" => true,
            [top, name, ..] if top == "setups" => setups.contains(name),
            _ => false,
        };
        if !keep {
            continue;
        }
        let mut h = entry.header().clone();
        let mut data = vec![];
        entry.read_to_end(&mut data)?;
        builder.append_data(&mut h, &path, data.as_slice())?;
    }
    Ok(builder.into_inner()?.finish()?)
}

pub async fn serve(State(s): State<AppState>, headers: HeaderMap) -> Response {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let machine = token.and_then(
        |t| match s.hub.db.machine_by_token(&crate::hub::hash_token(t)) {
            Ok(Some(m)) if m.status != MachineStatus::Destroyed => Some(m.id),
            _ => None,
        },
    );
    let Some(machine) = machine else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match current(&s.hub).await {
        Ok(Some(k)) => {
            let setups = machine_setups(&s.hub, machine);
            match only(&k.tar, &setups) {
                Ok(tar) => (
                    [
                        ("content-type", "application/gzip".to_string()),
                        ("x-corra-kit", version_for(&k, &setups)),
                    ],
                    tar,
                )
                    .into_response(),
                Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
            }
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, format!("{e:#}")).into_response(),
    }
}

pub async fn prepare(hub: &Hub, p: &Project, machine: MachineId) -> Result<()> {
    let wants_kit = hub.db.kv_get(&format!("machine:{machine}:kit"))?.as_deref() != Some("off");
    let kit = match current(hub).await {
        Ok(k) if wants_kit => k.map(|k| version_for(&k, &machine_setups(hub, machine))),
        Ok(_) => None,
        Err(e) => {
            tracing::warn!(project = %p.name, "couldn't read the setup repo: {e:#}");
            None
        }
    };
    let result = hub
        .runner_request_timeout(
            machine,
            RunnerRequest::Prepare {
                cwd: Hub::project_cwd(p),
                repos: p.repos.clone(),
                setup: p.setup.clone(),
                kit,
                env: hub.project_env(p),
            },
            Duration::from_secs(45 * 60),
        )
        .await;
    let failed = match &result {
        Ok(v) if v["ok"] == true => None,
        Ok(v) => Some(v["log"].as_str().unwrap_or_default().to_string()),
        Err(e) => Some(format!("setup didn't run: {e:#}")),
    };
    if let Some(log) = failed {
        tracing::warn!(project = %p.name, "setup failed: {log}");
        let tail: Vec<&str> = log.lines().collect();
        let tail = tail[tail.len().saturating_sub(30)..].join("\n");
        hub.db.kv_set(&format!("setup_failed:{}", p.id), &tail)?;
    }
    Ok(())
}

pub async fn setups(hub: &Hub) -> Result<Vec<SetupInfo>> {
    Ok(current(hub)
        .await?
        .map(|k| k.setups.clone())
        .unwrap_or_default())
}

pub fn match_setup(setups: &[SetupInfo], repo: &str) -> Option<String> {
    let short = repo.rsplit('/').next().unwrap_or(repo).to_lowercase();
    setups
        .iter()
        .find(|s| s.repos.iter().any(|r| r.eq_ignore_ascii_case(repo)))
        .or_else(|| setups.iter().find(|s| s.name.to_lowercase() == short))
        .map(|s| s.name.clone())
}
