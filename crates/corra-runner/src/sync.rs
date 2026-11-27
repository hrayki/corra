use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Result, bail};
use corra_proto::ThreadId;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SentState {
    Append(u64),
    Full([u8; 32]),
}

pub struct Syncer {
    http: reqwest::Client,
    base: String,
    token: String,
    sent: std::sync::Mutex<HashMap<(ThreadId, String), SentState>>,
    locks: std::sync::Mutex<HashMap<ThreadId, Arc<Mutex<()>>>>,
}

pub type SyncRef = Arc<Syncer>;

fn is_append_only(rel: &str) -> bool {
    rel.ends_with(".jsonl")
}

fn is_sqlite_shm(rel: &str) -> bool {
    rel.ends_with(".db-shm")
}

fn sqlite_sidecar_parent(rel: &str) -> Option<&str> {
    rel.strip_suffix("-wal")
        .or_else(|| rel.strip_suffix("-journal"))
        .filter(|base| base.ends_with(".db"))
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

impl Syncer {
    pub fn new(hub: &str, token: &str) -> SyncRef {
        Arc::new(Syncer {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("http client"),
            base: hub.trim_end_matches('/').to_string(),
            token: token.to_string(),
            sent: std::sync::Mutex::new(HashMap::new()),
            locks: std::sync::Mutex::new(HashMap::new()),
        })
    }

    fn lock(&self, thread: ThreadId) -> Arc<Mutex<()>> {
        self.locks
            .lock()
            .unwrap()
            .entry(thread)
            .or_default()
            .clone()
    }

    fn get_sent(&self, key: &(ThreadId, String)) -> Option<SentState> {
        self.sent.lock().unwrap().get(key).copied()
    }

    fn set_sent(&self, key: (ThreadId, String), state: SentState) {
        self.sent.lock().unwrap().insert(key, state);
    }

    async fn remote_sizes(&self, thread: ThreadId) -> Result<HashMap<String, u64>> {
        let r = self
            .http
            .get(format!("{}/runner/files/{thread}", self.base))
            .bearer_auth(&self.token)
            .send()
            .await?;
        if !r.status().is_success() {
            bail!("hub answered {}", r.status());
        }
        let v: Vec<serde_json::Value> = r.json().await?;
        Ok(v.into_iter()
            .filter_map(|f| Some((f["path"].as_str()?.to_string(), f["size"].as_u64()?)))
            .collect())
    }

    pub async fn upload(&self, thread: ThreadId, base: &Path, files: Vec<PathBuf>) -> Result<()> {
        let lock = self.lock(thread);
        let _g = lock.lock().await;
        let mut remote: Option<HashMap<String, u64>> = None;
        let mut items: Vec<(String, PathBuf)> = files
            .into_iter()
            .filter_map(|file| {
                let rel = file.strip_prefix(base).ok()?.to_string_lossy().to_string();
                (!is_sqlite_shm(&rel)).then_some((rel, file))
            })
            .collect();
        items.sort_by_key(|(rel, _)| {
            if sqlite_sidecar_parent(rel).is_some() {
                0
            } else {
                1
            }
        });
        let present: HashSet<String> = items.iter().map(|(rel, _)| rel.clone()).collect();
        let mut dbs: Vec<String> = vec![];
        for (rel, file) in items {
            let Ok(meta) = tokio::fs::metadata(&file).await else {
                continue;
            };
            if rel.ends_with(".db") {
                dbs.push(rel.clone());
            }
            let key = (thread, rel.clone());
            if is_append_only(&rel) {
                let size = meta.len();
                let known = match self.get_sent(&key) {
                    Some(SentState::Append(n)) => n,
                    _ => {
                        if remote.is_none() {
                            remote = Some(self.remote_sizes(thread).await?);
                        }
                        remote.as_ref().unwrap().get(&rel).copied().unwrap_or(0)
                    }
                };
                if known == size {
                    self.set_sent(key, SentState::Append(size));
                    continue;
                }
                let data = tokio::fs::read(&file).await?;
                let (offset, body, replace) = if (known as usize) < data.len() {
                    (known, data[known as usize..].to_vec(), false)
                } else {
                    (0, data.clone(), true)
                };
                let r = self
                    .http
                    .put(format!("{}/runner/file/{thread}", self.base))
                    .query(&[
                        ("path", rel.as_str()),
                        ("offset", &offset.to_string()),
                        ("replace", if replace { "true" } else { "false" }),
                    ])
                    .bearer_auth(&self.token)
                    .body(body)
                    .send()
                    .await?;
                if r.status() == reqwest::StatusCode::CONFLICT {
                    let r = self
                        .http
                        .put(format!("{}/runner/file/{thread}", self.base))
                        .query(&[("path", rel.as_str()), ("offset", "0"), ("replace", "true")])
                        .bearer_auth(&self.token)
                        .body(data.clone())
                        .send()
                        .await?;
                    if !r.status().is_success() {
                        bail!("hub answered {} for {rel}", r.status());
                    }
                } else if !r.status().is_success() {
                    bail!("hub answered {} for {rel}", r.status());
                }
                self.set_sent(key, SentState::Append(data.len() as u64));
            } else {
                let data = tokio::fs::read(&file).await?;
                let hash = sha256(&data);
                if self.get_sent(&key) == Some(SentState::Full(hash)) {
                    continue;
                }
                let r = self
                    .http
                    .put(format!("{}/runner/file/{thread}", self.base))
                    .query(&[("path", rel.as_str()), ("offset", "0"), ("replace", "true")])
                    .bearer_auth(&self.token)
                    .body(data)
                    .send()
                    .await?;
                if !r.status().is_success() {
                    bail!("hub answered {} for {rel}", r.status());
                }
                self.set_sent(key, SentState::Full(hash));
            }
        }
        let empty_hash = sha256(b"");
        for db in dbs {
            for suffix in ["-wal", "-journal"] {
                let side = format!("{db}{suffix}");
                if present.contains(&side) {
                    continue;
                }
                let key = (thread, side.clone());
                let needs_clear = match self.get_sent(&key) {
                    Some(SentState::Full(h)) => h != empty_hash,
                    Some(SentState::Append(n)) => n > 0,
                    None => {
                        if remote.is_none() {
                            remote = Some(self.remote_sizes(thread).await?);
                        }
                        remote.as_ref().unwrap().get(&side).copied().unwrap_or(0) > 0
                    }
                };
                if needs_clear {
                    let r = self
                        .http
                        .put(format!("{}/runner/file/{thread}", self.base))
                        .query(&[
                            ("path", side.as_str()),
                            ("offset", "0"),
                            ("replace", "true"),
                        ])
                        .bearer_auth(&self.token)
                        .body(Vec::<u8>::new())
                        .send()
                        .await?;
                    if !r.status().is_success() {
                        bail!("hub answered {} for {side}", r.status());
                    }
                    self.set_sent(key, SentState::Full(empty_hash));
                }
            }
        }
        Ok(())
    }

    pub async fn restore(
        &self,
        thread: ThreadId,
        base: &Path,
        force_mutable: bool,
    ) -> Result<usize> {
        let lock = self.lock(thread);
        let _g = lock.lock().await;
        let remote = self.remote_sizes(thread).await?;
        let mut restored_dbs = HashSet::new();
        for rel in remote.keys() {
            if rel.starts_with("work/") || !rel.ends_with(".db") {
                continue;
            }
            let path = base.join(rel);
            if force_mutable || !tokio::fs::try_exists(&path).await.unwrap_or(false) {
                restored_dbs.insert(rel.clone());
            }
        }
        let mut ordered: Vec<(String, u64)> = remote.into_iter().collect();
        ordered.sort_by_key(|(rel, _)| {
            if sqlite_sidecar_parent(rel).is_some() {
                1
            } else {
                0
            }
        });
        let mut n = 0;
        for (rel, size) in ordered {
            if rel.starts_with("work/") || is_sqlite_shm(&rel) {
                continue;
            }
            let path = base.join(&rel);
            let append_only = is_append_only(&rel);
            if let Some(parent_db) = sqlite_sidecar_parent(&rel) {
                if !restored_dbs.contains(parent_db) {
                    continue;
                }
                if size == 0 {
                    let _ = tokio::fs::remove_file(&path).await;
                    self.set_sent((thread, rel), SentState::Full(sha256(b"")));
                    continue;
                }
            } else if append_only {
                let local = tokio::fs::metadata(&path)
                    .await
                    .map(|m| m.len())
                    .unwrap_or(0);
                if local >= size {
                    continue;
                }
            } else if rel.ends_with(".db") {
                if !restored_dbs.contains(&rel) {
                    continue;
                }
            } else if !force_mutable && tokio::fs::try_exists(&path).await.unwrap_or(false) {
                continue;
            }
            let r = self
                .http
                .get(format!("{}/runner/file/{thread}", self.base))
                .query(&[("path", rel.as_str())])
                .bearer_auth(&self.token)
                .send()
                .await?;
            if !r.status().is_success() {
                bail!("hub answered {} for {rel}", r.status());
            }
            let bytes = r.bytes().await?;
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            if rel.ends_with(".db") {
                for suffix in ["-shm", "-wal", "-journal"] {
                    let _ = tokio::fs::remove_file(base.join(format!("{rel}{suffix}"))).await;
                }
            }
            tokio::fs::write(&path, &bytes).await?;
            let state = if append_only {
                SentState::Append(bytes.len() as u64)
            } else {
                SentState::Full(sha256(&bytes))
            };
            self.set_sent((thread, rel), state);
            n += 1;
        }
        Ok(n)
    }
}

impl Syncer {
    pub async fn put_file(&self, thread: ThreadId, rel: &str, data: Vec<u8>) -> Result<()> {
        let r = self
            .http
            .put(format!("{}/runner/file/{thread}", self.base))
            .query(&[("path", rel), ("offset", "0"), ("replace", "true")])
            .bearer_auth(&self.token)
            .body(data)
            .send()
            .await?;
        if !r.status().is_success() {
            bail!("hub answered {} for {rel}", r.status());
        }
        Ok(())
    }

    pub async fn get_file(&self, thread: ThreadId, rel: &str) -> Result<Option<Vec<u8>>> {
        let r = self
            .http
            .get(format!("{}/runner/file/{thread}", self.base))
            .query(&[("path", rel)])
            .bearer_auth(&self.token)
            .send()
            .await?;
        if r.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !r.status().is_success() {
            bail!("hub answered {} for {rel}", r.status());
        }
        Ok(Some(r.bytes().await?.to_vec()))
    }

    pub async fn list_prefix(&self, thread: ThreadId, prefix: &str) -> Result<Vec<String>> {
        Ok(self
            .remote_sizes(thread)
            .await?
            .into_keys()
            .filter(|k| k.starts_with(prefix))
            .collect())
    }
}

pub fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}
