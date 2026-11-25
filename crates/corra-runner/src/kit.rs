use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

use crate::files::home;

fn kit_dir() -> PathBuf {
    home().join(".corra/kit")
}

fn state_file() -> PathBuf {
    home().join(".corra/kit.applied")
}

pub async fn prepare(
    root: &Path,
    repos: &[corra_proto::Repo],
    hub_url: &str,
    token: &str,
    setup: Option<&str>,
    kit: Option<&str>,
    env: &[(String, String)],
) -> Result<Value> {
    tokio::fs::create_dir_all(root).await?;
    let mut log = String::new();
    let problems = crate::git::ensure_repos(root, hub_url, token, repos).await;
    for p in &problems {
        log.push_str(p);
        log.push('\n');
    }
    let mut ok = problems.is_empty();
    if let Some(version) = kit
        && let Err(e) = apply(root, hub_url, token, version, setup, env, &mut log).await
    {
        ok = false;
        log.push_str(&format!("{e:#}\n"));
    }
    write_note(root, repos, kit.and(setup));
    Ok(json!({ "ok": ok, "log": log }))
}

fn write_note(root: &Path, repos: &[corra_proto::Repo], setup: Option<&str>) {
    if root.join(".git").exists() {
        return;
    }
    let mut note = String::new();
    if repos.len() > 1 {
        let list: Vec<String> = repos
            .iter()
            .map(|r| match &r.github {
                Some(g) => format!("`{}/` ({g})", r.name),
                None => format!("`{}/`", r.name),
            })
            .collect();
        note = format!(
            "This project holds {}. Each has its own AGENTS.md.\n",
            list.join(", ")
        );
    }
    if let Some(rules) = setup.and_then(|name| {
        std::fs::read_to_string(kit_dir().join("setups").join(name).join("AGENTS.md")).ok()
    }) {
        if !note.is_empty() {
            note.push('\n');
        }
        note.push_str(&rules);
    }
    let (agents, claude) = (root.join("AGENTS.md"), root.join("CLAUDE.md"));
    if note.is_empty() {
        let _ = std::fs::remove_file(agents);
        let _ = std::fs::remove_file(claude);
        return;
    }
    let _ = std::fs::write(agents, note);
    let _ = std::fs::write(claude, "@AGENTS.md\n");
}

async fn apply(
    root: &Path,
    hub_url: &str,
    token: &str,
    version: &str,
    setup: Option<&str>,
    env: &[(String, String)],
    log: &mut String,
) -> Result<()> {
    let want = json!({ "version": version, "setup": setup });
    let have: Value = std::fs::read(state_file())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    if have == want {
        return Ok(());
    }
    if have["version"] != want["version"] || !kit_dir().exists() {
        download(hub_url, token).await?;
    }
    let dir = kit_dir();
    let shared = dir.join("shared");
    let project = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut envs: Vec<(String, String)> = env.to_vec();
    envs.push(("CORRA_PROJECT".into(), project));
    envs.push((
        "CORRA_PROJECT_DIR".into(),
        root.to_string_lossy().to_string(),
    ));
    if shared.exists() {
        install_shared(&shared)?;
        let script = shared.join("setup");
        if script.exists() {
            run(&script, &shared, &envs, Duration::from_secs(15 * 60), log).await?;
        }
    }
    if let Some(name) = setup {
        let d = dir.join("setups").join(name);
        let script = d.join("setup");
        if !script.exists() {
            bail!("the setup repo has no corra/setups/{name}/setup");
        }
        run(&script, &d, &envs, Duration::from_secs(40 * 60), log).await?;
    }
    std::fs::write(state_file(), want.to_string())?;
    Ok(())
}

async fn download(hub_url: &str, token: &str) -> Result<()> {
    let r = reqwest::Client::new()
        .get(format!("{}/runner/kit", hub_url.trim_end_matches('/')))
        .bearer_auth(token)
        .timeout(Duration::from_secs(120))
        .send()
        .await?;
    if !r.status().is_success() {
        bail!("the hub answered {} for the setup kit", r.status());
    }
    let bytes = r.bytes().await?;
    let parent = home().join(".corra");
    std::fs::create_dir_all(&parent)?;
    let fresh = parent.join("kit.new");
    let _ = std::fs::remove_dir_all(&fresh);
    std::fs::create_dir_all(&fresh)?;
    tar::Archive::new(flate2::read::GzDecoder::new(&bytes[..])).unpack(&fresh)?;
    let _ = std::fs::remove_dir_all(kit_dir());
    std::fs::rename(&fresh, kit_dir())?;
    Ok(())
}

fn install_shared(shared: &Path) -> Result<()> {
    let claude = home().join(".claude");
    std::fs::create_dir_all(&claude)?;
    let agents = shared.join("AGENTS.md");
    if agents.exists() {
        std::fs::copy(&agents, claude.join("CLAUDE.md"))?;
    }
    let skills = shared.join("skills");
    let dest = claude.join("skills");
    std::fs::create_dir_all(&dest)?;
    if let Ok(rd) = std::fs::read_dir(&dest) {
        for e in rd.flatten() {
            let p = e.path();
            if let Ok(target) = std::fs::read_link(&p)
                && target.starts_with(kit_dir())
                && !target.exists()
            {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
    if let Ok(rd) = std::fs::read_dir(&skills) {
        for e in rd.flatten() {
            if !e.path().is_dir() {
                continue;
            }
            let link = dest.join(e.file_name());
            if link.symlink_metadata().is_ok() {
                if link.read_link().is_err() {
                    continue;
                }
                let _ = std::fs::remove_file(&link);
            }
            std::os::unix::fs::symlink(e.path(), &link)?;
        }
    }
    let path = claude.join("settings.json");
    let mut settings: Value = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| json!({}));
    if let Some(extra) = std::fs::read(shared.join("claude-settings.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
    {
        merge(&mut settings, &extra);
    }
    settings["disableClaudeAiConnectors"] = json!(true);
    std::fs::write(&path, serde_json::to_vec_pretty(&settings)?)?;
    Ok(())
}

fn merge(into: &mut Value, from: &Value) {
    match (into, from) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in b {
                match a.get_mut(k) {
                    Some(existing) => merge(existing, v),
                    None => {
                        a.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            for v in b {
                if !a.contains(v) {
                    a.push(v.clone());
                }
            }
        }
        (slot, v) => *slot = v.clone(),
    }
}

async fn run(
    script: &Path,
    cwd: &Path,
    env: &[(String, String)],
    limit: Duration,
    log: &mut String,
) -> Result<()> {
    log.push_str(&format!("$ {}\n", script.display()));
    let out = tokio::time::timeout(
        limit,
        tokio::process::Command::new("bash")
            .arg(script)
            .current_dir(cwd)
            .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .env_remove("CORRA_TOKEN")
            .env_remove("CORRA_TOKEN_FILE")
            .stdin(std::process::Stdio::null())
            .output(),
    )
    .await
    .map_err(|_| {
        anyhow!(
            "{} took longer than {} minutes",
            script.display(),
            limit.as_secs() / 60
        )
    })??;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let lines: Vec<&str> = text.lines().collect();
    log.push_str(&lines[lines.len().saturating_sub(200)..].join("\n"));
    log.push('\n');
    if !out.status.success() {
        bail!("{} exited with {}", script.display(), out.status);
    }
    Ok(())
}
