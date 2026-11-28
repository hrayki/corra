use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use corra_proto::runner::RunnerRequest;
use serde_json::{Value, json};

pub fn expand(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        return home().join(rest);
    }
    if path == "~" {
        return home();
    }
    PathBuf::from(path)
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub async fn handle(
    req: RunnerRequest,
    hub_url: &str,
    token: &str,
    git_token: &str,
    sync: Option<crate::sync::SyncRef>,
) -> Result<Value> {
    match req {
        RunnerRequest::ListDir { path } => list_dir(&expand(&path)).await,
        RunnerRequest::ReadFile { path, max_bytes } => read_file(&expand(&path), max_bytes).await,
        RunnerRequest::Git { cwd, args } => git(&expand(&cwd), &args).await,
        RunnerRequest::Prepare {
            cwd,
            repos,
            setup,
            kit,
            env,
            force,
        } => {
            crate::kit::prepare(
                &expand(&cwd),
                &repos,
                crate::kit::HubAuth {
                    url: hub_url,
                    token,
                    git_token,
                },
                setup.as_deref(),
                kit.as_deref(),
                &env,
                force,
            )
            .await
        }
        RunnerRequest::Rewind {
            cwd,
            thread,
            turn,
            turns,
        } => {
            let root = expand(&cwd);
            let done = crate::git::rewind_all(&root, &thread.to_string(), &turn, &turns).await?;
            Ok(json!({ "repos": done }))
        }
        RunnerRequest::SaveWork { cwd, thread } => match sync {
            Some(s) => {
                Ok(json!({ "repos": crate::git::save_current(&s, thread, &expand(&cwd)).await? }))
            }
            None => Ok(json!({ "repos": 0 })),
        },
        RunnerRequest::Changes { cwd } => Ok(crate::git::uncommitted(&expand(&cwd)).await),
        RunnerRequest::Folders { path } => {
            tokio::task::spawn_blocking(move || folders(&path)).await?
        }
        RunnerRequest::ClaudeSessions => {
            let list = tokio::task::spawn_blocking(crate::agents::import::list_sessions).await?;
            Ok(json!(list))
        }
        RunnerRequest::ImportSession { session, cwd } => {
            tokio::task::spawn_blocking(move || import_session(&session, &cwd)).await?
        }
        RunnerRequest::Secret { name, value } => {
            let dir = crate::secrets::dir();
            crate::secrets::sweep(&dir, crate::secrets::KEPT_FOR);
            let path = crate::secrets::write(&dir, &name, &value)?;
            Ok(json!({ "path": path.to_string_lossy() }))
        }
        RunnerRequest::AgentUsage { agent } => match agent {
            corra_proto::AgentKind::Antigravity => {
                let u = crate::agents::antigravity::fetch_usage().await?;
                Ok(serde_json::to_value(u)?)
            }
            corra_proto::AgentKind::Claude => {
                bail!("Claude usage updates automatically during turns")
            }
        },
        RunnerRequest::TermOpen { .. }
        | RunnerRequest::TermAttach { .. }
        | RunnerRequest::Stop { .. } => {
            bail!("handled elsewhere")
        }
    }
}

fn folders(path: &str) -> Result<Value> {
    let home = home();
    let path: PathBuf = if path.trim().is_empty() {
        home.clone()
    } else {
        expand(path.trim())
    };
    let (dir, prefix) = if path.is_dir() {
        (path.clone(), String::new())
    } else {
        (
            path.parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| home.clone()),
            path.file_name()
                .map(|f| f.to_string_lossy().to_lowercase())
                .unwrap_or_default(),
        )
    };
    let mut entries: Vec<Value> = vec![];
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !e.path().is_dir() || (name.starts_with('.') && !prefix.starts_with('.')) {
                continue;
            }
            if !prefix.is_empty() && !name.to_lowercase().starts_with(&prefix) {
                continue;
            }
            entries.push(json!({ "name": name, "path": e.path().to_string_lossy(), "git": e.path().join(".git").exists() }));
        }
    }
    entries.sort_by_key(|e| e["name"].as_str().unwrap_or("").to_lowercase());
    entries.truncate(200);
    Ok(json!({
        "path": dir.to_string_lossy(),
        "exists": path.is_dir(),
        "git": path.join(".git").exists(),
        "parent": dir.parent().map(|p| p.to_string_lossy().to_string()),
        "home": home.to_string_lossy(),
        "entries": entries,
    }))
}

fn import_session(session: &str, cwd: &Option<String>) -> Result<Value> {
    use crate::agents::import;
    let found = import::find_session(session)
        .ok_or_else(|| anyhow!("there's no Claude Code session {session} on this machine"))?;
    let target = match cwd {
        Some(c) => c.clone(),
        None if !found.cwd.is_empty() && Path::new(&found.cwd).is_dir() => found.cwd.clone(),
        None => bail!(
            "the session's folder {} isn't on this machine; pick a project",
            found.cwd
        ),
    };
    import::copy_into(&found, &expand(&target))?;
    let history = import::history(&found)?;
    Ok(json!({ "cwd": target, "title": found.title, "history": history }))
}

async fn list_dir(path: &Path) -> Result<Value> {
    let mut rd = tokio::fs::read_dir(path)
        .await
        .with_context(|| format!("reading {}", path.display()))?;
    let mut entries = vec![];
    while let Some(e) = rd.next_entry().await? {
        let name = e.file_name().to_string_lossy().to_string();
        let ft = e.file_type().await?;
        let meta = e.metadata().await.ok();
        entries.push(json!({
            "name": name,
            "dir": ft.is_dir(),
            "link": ft.is_symlink(),
            "size": meta.as_ref().map(|m| m.len()).unwrap_or(0),
        }));
    }
    entries.sort_by(|a, b| {
        let ad = a["dir"].as_bool().unwrap_or(false);
        let bd = b["dir"].as_bool().unwrap_or(false);
        bd.cmp(&ad).then_with(|| {
            a["name"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .cmp(&b["name"].as_str().unwrap_or("").to_lowercase())
        })
    });
    Ok(json!({ "path": path.to_string_lossy(), "entries": entries }))
}

async fn read_file(path: &Path, max_bytes: u64) -> Result<Value> {
    use tokio::io::AsyncReadExt;
    let file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("reading {}", path.display()))?;
    let meta = file.metadata().await?;
    if meta.is_dir() {
        bail!("{} is a folder", path.display());
    }
    let size = meta.len();
    let cap = size.min(max_bytes).min(16 << 20) as usize;
    let mut data = Vec::with_capacity(cap);
    file.take(max_bytes).read_to_end(&mut data).await?;
    let truncated = size > max_bytes;
    let (text, base64) = match String::from_utf8(data) {
        Ok(s) => (Some(s), None),
        Err(e) => {
            use base64::Engine;
            (
                None,
                Some(base64::engine::general_purpose::STANDARD.encode(e.into_bytes())),
            )
        }
    };
    Ok(
        json!({ "path": path.to_string_lossy(), "size": size, "truncated": truncated, "text": text, "base64": base64 }),
    )
}

const GIT_READ_ONLY: &[&str] = &[
    "status",
    "diff",
    "log",
    "show",
    "branch",
    "rev-parse",
    "ls-files",
    "worktree",
];

async fn git(cwd: &Path, args: &[String]) -> Result<Value> {
    let Some(sub) = args.first() else {
        bail!("no git command")
    };
    if !GIT_READ_ONLY.contains(&sub.as_str()) {
        bail!("git {sub} isn't allowed here");
    }
    let out = tokio::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .arg("--no-pager")
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .await?;
    Ok(json!({
        "code": out.status.code(),
        "stdout": String::from_utf8_lossy(&out.stdout),
        "stderr": String::from_utf8_lossy(&out.stderr),
    }))
}
