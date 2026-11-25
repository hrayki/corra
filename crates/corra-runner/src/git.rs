use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use corra_proto::{FileChange, RepoCheckpoint, ThreadId};
use tokio::process::Command;

async fn git(repo: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<String> {
    let mut c = Command::new("git");
    c.arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0");
    for (k, v) in env {
        c.env(k, v);
    }
    let out = c.output().await?;
    if !out.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

pub fn repos(root: &Path) -> Vec<(String, PathBuf)> {
    if root.join(".git").exists() {
        return vec![(".".into(), root.to_path_buf())];
    }
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path();
            if p.join(".git").exists() {
                out.push((e.file_name().to_string_lossy().to_string(), p));
            }
        }
    }
    out.sort();
    out
}

const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

pub async fn uncommitted(root: &Path) -> serde_json::Value {
    let mut out = vec![];
    for (name, path) in repos(root) {
        let base = match git(&path, &["rev-parse", "--verify", "-q", "HEAD"], &[]).await {
            Ok(h) if !h.is_empty() => h,
            _ => EMPTY_TREE.to_string(),
        };
        let mut files = vec![];
        if let Ok(stat) = git(&path, &["diff", "--numstat", "--find-renames", &base], &[]).await {
            for line in stat.lines() {
                let mut it = line.splitn(3, '\t');
                let (a, r, p) = (it.next(), it.next(), it.next());
                let Some(p) = p else { continue };
                files.push(serde_json::json!({
                    "path": p,
                    "added": a.and_then(|x| x.parse::<u64>().ok()).unwrap_or(0),
                    "removed": r.and_then(|x| x.parse::<u64>().ok()).unwrap_or(0),
                    "binary": a == Some("-"),
                    "untracked": false,
                }));
            }
        }
        if let Ok(list) = git(&path, &["ls-files", "--others", "--exclude-standard"], &[]).await {
            for p in list.lines().filter(|l| !l.is_empty()).take(500) {
                let (lines, binary) = match std::fs::read(path.join(p)) {
                    Ok(bytes) if bytes.len() > 4 << 20 || bytes.contains(&0) => (0, true),
                    Ok(bytes) => (line_count(&bytes), false),
                    Err(_) => (0, false),
                };
                files.push(serde_json::json!({
                    "path": p,
                    "added": lines,
                    "removed": 0,
                    "binary": binary,
                    "untracked": true,
                }));
            }
        }
        out.push(serde_json::json!({ "repo": name, "base": base, "files": files }));
    }
    serde_json::Value::Array(out)
}

fn line_count(bytes: &[u8]) -> u64 {
    let n = bytes.iter().filter(|b| **b == b'\n').count() as u64;
    if bytes.last().is_some_and(|b| *b != b'\n') {
        n + 1
    } else {
        n
    }
}

pub async fn snapshot(repo: &Path) -> Result<String> {
    let head = git(repo, &["rev-parse", "--verify", "-q", "HEAD"], &[])
        .await
        .ok();
    let dir = tempdir();
    std::fs::create_dir_all(&dir)?;
    let index = dir.join(format!("index-{}", uuid::Uuid::now_v7()));
    let index_s = index.to_string_lossy().to_string();
    let env = [("GIT_INDEX_FILE", index_s.as_str())];
    let result = async {
        if head.is_some() {
            git(repo, &["read-tree", "HEAD"], &env).await?;
        }
        git(repo, &["add", "-A", "--", "."], &env).await?;
        let tree = git(repo, &["write-tree"], &env).await?;
        let mut args = vec!["commit-tree", tree.as_str(), "-m", "corra checkpoint"];
        if let Some(h) = &head {
            args.push("-p");
            args.push(h);
        }
        git(
            repo,
            &args,
            &[
                ("GIT_AUTHOR_NAME", "corra"),
                ("GIT_AUTHOR_EMAIL", "corra@localhost"),
                ("GIT_COMMITTER_NAME", "corra"),
                ("GIT_COMMITTER_EMAIL", "corra@localhost"),
            ],
        )
        .await
    }
    .await;
    let _ = std::fs::remove_file(&index);
    result
}

fn tempdir() -> PathBuf {
    std::env::temp_dir().join("corra-git")
}

pub async fn changes(repo: &Path, from: &str, to: &str) -> Result<Vec<FileChange>> {
    let out = git(repo, &["diff", "--numstat", "--no-renames", from, to], &[]).await?;
    Ok(out
        .lines()
        .filter_map(|l| {
            let mut it = l.splitn(3, '\t');
            let a = it.next()?;
            let r = it.next()?;
            let p = it.next()?;
            Some(FileChange {
                path: p.to_string(),
                added: a.parse().unwrap_or(0),
                removed: r.parse().unwrap_or(0),
            })
        })
        .collect())
}

pub async fn checkpoint_all(
    root: &Path,
    thread: &str,
    turn: &str,
    before: &[(String, String)],
) -> Vec<RepoCheckpoint> {
    let mut out = vec![];
    for (name, path) in repos(root) {
        let Ok(after) = snapshot(&path).await else {
            continue;
        };
        let prev = before
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, s)| s.clone());
        let files = match &prev {
            Some(b) if *b != after => changes(&path, b, &after).await.unwrap_or_default(),
            Some(_) => vec![],
            None => vec![],
        };
        let _ = git(
            &path,
            &["update-ref", &format!("refs/corra/{thread}/{turn}"), &after],
            &[],
        )
        .await;
        if let Some(b) = &prev {
            let _ = git(
                &path,
                &[
                    "update-ref",
                    &format!("refs/corra/{thread}/{turn}.before"),
                    b,
                ],
                &[],
            )
            .await;
        }
        let _ = git(
            &path,
            &["update-ref", &format!("refs/corra/{thread}/latest"), &after],
            &[],
        )
        .await;
        let branch = git(&path, &["branch", "--show-current"], &[])
            .await
            .ok()
            .filter(|b| !b.is_empty());
        out.push(RepoCheckpoint {
            repo: name,
            before: prev,
            after,
            branch,
            files,
        });
    }
    out
}

pub async fn ensure_repos(
    root: &Path,
    hub: &str,
    token: &str,
    repos: &[corra_proto::Repo],
) -> Vec<String> {
    let mut problems = vec![];
    let hub = hub.trim_end_matches('/');
    let header = format!("Authorization: Bearer {token}");
    for r in repos {
        let Some(gh) = &r.github else { continue };
        let dir = if r.name == "." {
            root.to_path_buf()
        } else {
            root.join(&r.name)
        };
        let url = format!("{hub}/git/{gh}.git");
        let key = format!("http.{hub}/.extraHeader");
        if dir.join(".git").exists() {
            let _ = git(&dir, &["config", &key, &header], &[]).await;
            continue;
        }
        if let Some(parent) = dir.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        let mut last = String::new();
        let mut done = false;
        for attempt in 0..3 {
            if attempt > 0 {
                let _ = tokio::fs::remove_dir_all(&dir).await;
                tokio::time::sleep(std::time::Duration::from_secs(2 * attempt)).await;
            }
            let out = Command::new("git")
                .arg("-c")
                .arg(format!("{key}={header}"))
                .arg("clone")
                .arg("--quiet")
                .arg(&url)
                .arg(&dir)
                .output()
                .await;
            match out {
                Ok(o) if o.status.success() => {
                    let _ = git(&dir, &["config", &key, &header], &[]).await;
                    done = true;
                    break;
                }
                Ok(o) => last = String::from_utf8_lossy(&o.stderr).trim().to_string(),
                Err(e) => {
                    last = format!("couldn't run git: {e}");
                    break;
                }
            }
        }
        if !done {
            problems.push(format!("couldn't clone {gh}: {last}"));
        }
    }
    problems
}

fn work_name(repo: &str) -> String {
    if repo == "." {
        "_root".into()
    } else {
        repo.replace('/', "_")
    }
}

pub async fn save_work(
    sync: &crate::sync::Syncer,
    thread: corra_proto::ThreadId,
    root: &Path,
    repos: &[RepoCheckpoint],
) -> Result<()> {
    for r in repos {
        let path = if r.repo == "." {
            root.to_path_buf()
        } else {
            root.join(&r.repo)
        };
        let mut refs = thread_refs(&path, &thread.to_string()).await;
        if refs.is_empty() {
            refs.push(format!("refs/corra/{thread}/latest"));
        }
        let has_remote = !git(&path, &["remote"], &[])
            .await
            .unwrap_or_default()
            .trim()
            .is_empty();
        let file = tempdir().join(format!("work-{}.bundle", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(tempdir())?;
        let file_s = file.to_string_lossy().to_string();
        let mut args = vec!["bundle", "create", "-q", file_s.as_str()];
        args.extend(refs.iter().map(String::as_str));
        if has_remote {
            args.extend(["--not", "--remotes"]);
        }
        let made = git(&path, &args, &[]).await;
        let data = match made {
            Ok(_) => tokio::fs::read(&file).await?,
            Err(e) => {
                let _ = std::fs::remove_file(&file);
                return Err(e);
            }
        };
        let _ = std::fs::remove_file(&file);
        let head = git(&path, &["rev-parse", &format!("{}^", r.after)], &[])
            .await
            .ok();
        let name = work_name(&r.repo);
        sync.put_file(thread, &format!("work/{name}.bundle"), data)
            .await?;
        let meta = serde_json::json!({ "repo": r.repo, "branch": r.branch, "head": head, "wip": r.after, "origin": origin_slug(&path).await });
        sync.put_file(
            thread,
            &format!("work/{name}.json"),
            meta.to_string().into_bytes(),
        )
        .await?;
    }
    Ok(())
}

async fn thread_refs(path: &Path, thread: &str) -> Vec<String> {
    git(
        path,
        &[
            "for-each-ref",
            "--format=%(refname)",
            &format!("refs/corra/{thread}/"),
        ],
        &[],
    )
    .await
    .map(|o| {
        o.lines()
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect()
    })
    .unwrap_or_default()
}

async fn repo_for(root: &Path, meta: &serde_json::Value) -> Option<PathBuf> {
    let repo = meta["repo"].as_str().unwrap_or(".");
    let path = if repo == "." {
        root.to_path_buf()
    } else {
        root.join(repo)
    };
    if path.join(".git").exists() {
        return Some(path);
    }
    let want = meta["origin"].as_str()?;
    for (_, candidate) in repos(root) {
        if origin_slug(&candidate).await.as_deref() == Some(want) {
            return Some(candidate);
        }
    }
    None
}

async fn fetch_bundle(path: &Path, bundle: &[u8], thread: ThreadId) -> Result<()> {
    std::fs::create_dir_all(tempdir())?;
    let file = tempdir().join(format!("restore-{}.bundle", uuid::Uuid::now_v7()));
    tokio::fs::write(&file, bundle).await?;
    let file_s = file.to_string_lossy().to_string();
    let _ = git(path, &["fetch", "-q", "origin"], &[]).await;
    let spec = format!("+refs/corra/{thread}/*:refs/corra/{thread}/*");
    let fetched = git(path, &["fetch", "-q", &file_s, &spec], &[]).await;
    let _ = std::fs::remove_file(&file);
    fetched.map(|_| ())
}

pub async fn restore_work(
    sync: &crate::sync::Syncer,
    thread: corra_proto::ThreadId,
    root: &Path,
) -> Result<Vec<String>> {
    restore_work_checked(sync, thread, root, false).await
}

pub async fn origin_slug(repo: &Path) -> Option<String> {
    let url = git(repo, &["remote", "get-url", "origin"], &[])
        .await
        .ok()?;
    let url = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let tail = if let Some(i) = url.find("/git/") {
        &url[i + 5..]
    } else {
        let i = url.find("github.com")?;
        url[i + 10..].trim_start_matches([':', '/'])
    };
    let mut parts = tail.split('/');
    let (o, r) = (parts.next()?, parts.next()?);
    Some(format!("{o}/{r}").to_lowercase())
}

pub async fn save_current(
    sync: &crate::sync::Syncer,
    thread: corra_proto::ThreadId,
    root: &Path,
) -> Result<usize> {
    let mut cps = vec![];
    for (name, path) in repos(root) {
        let Ok(after) = snapshot(&path).await else {
            continue;
        };
        let _ = git(
            &path,
            &["update-ref", &format!("refs/corra/{thread}/latest"), &after],
            &[],
        )
        .await;
        let branch = git(&path, &["branch", "--show-current"], &[])
            .await
            .ok()
            .filter(|b| !b.is_empty());
        cps.push(RepoCheckpoint {
            repo: name,
            before: None,
            after,
            branch,
            files: vec![],
        });
    }
    save_work(sync, thread, root, &cps).await?;
    Ok(cps.len())
}

pub async fn rewind_all(
    root: &Path,
    thread: &str,
    turn: &str,
    turns: &[String],
) -> Result<Vec<String>> {
    let mut done = vec![];
    let turns: Vec<String> = if turns.is_empty() {
        vec![turn.to_string()]
    } else {
        turns.to_vec()
    };
    for (name, path) in repos(root) {
        let rev = |r: String| {
            let path = path.clone();
            async move {
                git(&path, &["rev-parse", "--verify", "-q", &r], &[])
                    .await
                    .ok()
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
            }
        };
        let Some(target) = rev(format!("refs/corra/{thread}/{turn}.before")).await else {
            continue;
        };
        let mut touched = std::collections::BTreeSet::new();
        for t in &turns {
            let (Some(b), Some(a)) = (
                rev(format!("refs/corra/{thread}/{t}.before")).await,
                rev(format!("refs/corra/{thread}/{t}")).await,
            ) else {
                continue;
            };
            let out = git(
                &path,
                &["diff", "--name-only", "--no-renames", "-z", &b, &a],
                &[],
            )
            .await?;
            touched.extend(out.split('\0').filter(|f| !f.is_empty()).map(String::from));
        }
        if touched.is_empty() {
            continue;
        }
        let in_target: std::collections::HashSet<String> =
            git(&path, &["ls-tree", "-r", "--name-only", "-z", &target], &[])
                .await?
                .split('\0')
                .filter(|f| !f.is_empty())
                .map(String::from)
                .collect();
        let (keep, gone): (Vec<&String>, Vec<&String>) =
            touched.iter().partition(|f| in_target.contains(*f));
        for chunk in keep.chunks(200) {
            let source = format!("--source={target}");
            let mut args = vec!["restore", source.as_str(), "--worktree", "--"];
            args.extend(chunk.iter().map(|f| f.as_str()));
            git(&path, &args, &[]).await?;
        }
        for f in gone {
            let _ = tokio::fs::remove_file(path.join(f)).await;
        }
        if let Ok(now) = snapshot(&path).await {
            let _ = git(
                &path,
                &["update-ref", &format!("refs/corra/{thread}/latest"), &now],
                &[],
            )
            .await;
        }
        done.push(name);
    }
    Ok(done)
}

pub async fn restore_work_checked(
    sync: &crate::sync::Syncer,
    thread: corra_proto::ThreadId,
    root: &Path,
    clean_only: bool,
) -> Result<Vec<String>> {
    let mut restored = vec![];
    for rel in sync.list_prefix(thread, "work/").await? {
        let Some(name) = rel
            .strip_prefix("work/")
            .and_then(|n| n.strip_suffix(".json"))
        else {
            continue;
        };
        let Some(meta) = sync.get_file(thread, &rel).await? else {
            continue;
        };
        let meta: serde_json::Value = serde_json::from_slice(&meta)?;
        let repo = meta["repo"].as_str().unwrap_or(".");
        let Some(path) = repo_for(root, &meta).await else {
            continue;
        };
        if clean_only
            && !git(&path, &["status", "--porcelain"], &[])
                .await
                .unwrap_or_default()
                .trim()
                .is_empty()
        {
            let known = git(
                &path,
                &[
                    "rev-parse",
                    "--verify",
                    "-q",
                    &format!("refs/corra/{thread}/latest^{{tree}}"),
                ],
                &[],
            )
            .await
            .ok();
            let now = match snapshot(&path).await {
                Ok(sha) => git(&path, &["rev-parse", &format!("{sha}^{{tree}}")], &[])
                    .await
                    .ok(),
                Err(_) => None,
            };
            if known.is_none() || known != now {
                bail!(
                    "{} has uncommitted changes that aren't this thread's; commit or stash them first",
                    path.display()
                );
            }
        }
        let reference = format!("refs/corra/{thread}/latest");
        let wip_want = meta["wip"].as_str().unwrap_or_default();
        if let Ok(cur) = git(&path, &["rev-parse", "--verify", "-q", &reference], &[]).await
            && cur.trim() == wip_want
        {
            continue;
        }
        let Some(bundle) = sync
            .get_file(thread, &format!("work/{name}.bundle"))
            .await?
        else {
            continue;
        };
        fetch_bundle(&path, &bundle, thread).await?;
        let wip = meta["wip"].as_str().unwrap_or_default().to_string();
        if let (Some(branch), Some(head)) = (meta["branch"].as_str(), meta["head"].as_str()) {
            git(&path, &["checkout", "-q", "-B", branch, head], &[]).await?;
        } else if let Some(head) = meta["head"].as_str() {
            git(&path, &["checkout", "-q", "--detach", head], &[]).await?;
        }
        git(
            &path,
            &[
                "restore",
                &format!("--source={wip}"),
                "--worktree",
                "--",
                ".",
            ],
            &[],
        )
        .await?;
        restored.push(repo.to_string());
    }
    Ok(restored)
}
