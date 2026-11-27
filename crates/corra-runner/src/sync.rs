use std::collections::HashMap;
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
        for file in files {
            let Ok(rel) = file.strip_prefix(base) else {
                continue;
            };
            let rel = rel.to_string_lossy().to_string();
            let Ok(meta) = tokio::fs::metadata(&file).await else {
                continue;
            };
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
        let mut n = 0;
        for (rel, size) in remote {
            if rel.starts_with("work/") {
                continue;
            }
            let path = base.join(&rel);
            let append_only = is_append_only(&rel);
            if append_only {
                let local = tokio::fs::metadata(&path)
                    .await
                    .map(|m| m.len())
                    .unwrap_or(0);
                if local >= size {
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
