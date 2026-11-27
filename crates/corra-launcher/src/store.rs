use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use rusty_s3::actions::ListObjectsV2;
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};

use crate::state::State;

pub const STATE: &str = "state.json";

#[derive(Clone, Debug)]
pub enum Cond {
    Any,
    Absent,
    Match(String),
}

#[derive(Debug, PartialEq)]
pub enum Put {
    Done,
    Conflict,
}

pub trait Blobs: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<(Vec<u8>, Option<String>)>>;
    fn put(&self, key: &str, body: &[u8], cond: Cond) -> Result<Put>;
    fn delete(&self, key: &str) -> Result<()>;
    fn dirs(&self, prefix: &str) -> Result<Vec<String>>;
}

pub struct S3 {
    bucket: Bucket,
    creds: Credentials,
    prefix: String,
    agent: ureq::Agent,
}

pub struct BucketEnv {
    pub endpoint: String,
    pub bucket: String,
    pub access_key: String,
    pub secret_key: String,
    pub region: String,
    pub prefix: String,
}

impl S3 {
    pub fn new(e: &BucketEnv) -> Result<S3> {
        let endpoint = if e.endpoint.starts_with("http") {
            e.endpoint.clone()
        } else {
            format!("https://{}", e.endpoint)
        };
        let bucket = Bucket::new(
            endpoint.parse().context("CORRA_STORAGE_ENDPOINT")?,
            UrlStyle::Path,
            e.bucket.clone(),
            e.region.clone(),
        )
        .map_err(|err| anyhow!("storage bucket: {err}"))?;
        Ok(S3 {
            bucket,
            creds: Credentials::new(e.access_key.clone(), e.secret_key.clone()),
            prefix: format!("{}/launcher", e.prefix.trim_end_matches('/')),
            agent: crate::http_agent(),
        })
    }

    fn key(&self, k: &str) -> String {
        format!("{}/{k}", self.prefix)
    }
}

const SIGNED_FOR: Duration = Duration::from_secs(900);

impl Blobs for S3 {
    fn get(&self, key: &str) -> Result<Option<(Vec<u8>, Option<String>)>> {
        let url = self
            .bucket
            .get_object(Some(&self.creds), &self.key(key))
            .sign(SIGNED_FOR);
        let mut r = self.agent.get(url.as_str()).call()?;
        if r.status() == 404 {
            return Ok(None);
        }
        if !r.status().is_success() {
            bail!("the storage bucket answered {} for {key}", r.status());
        }
        let tag = r
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let body = r.body_mut().with_config().limit(1 << 30).read_to_vec()?;
        Ok(Some((body, tag)))
    }

    fn put(&self, key: &str, body: &[u8], cond: Cond) -> Result<Put> {
        let url = self
            .bucket
            .put_object(Some(&self.creds), &self.key(key))
            .sign(SIGNED_FOR);
        let req = self.agent.put(url.as_str());
        let req = match cond {
            Cond::Any => req,
            Cond::Absent => req.header("if-none-match", "*"),
            Cond::Match(tag) => req.header("if-match", tag),
        };
        let r = req.send(body)?;
        if r.status() == 412 || r.status() == 409 {
            return Ok(Put::Conflict);
        }
        if !r.status().is_success() {
            bail!("the storage bucket answered {} for {key}", r.status());
        }
        Ok(Put::Done)
    }

    fn delete(&self, key: &str) -> Result<()> {
        let url = self
            .bucket
            .delete_object(Some(&self.creds), &self.key(key))
            .sign(SIGNED_FOR);
        let r = self.agent.delete(url.as_str()).call()?;
        if !r.status().is_success() && r.status() != 404 {
            bail!("the storage bucket answered {} for {key}", r.status());
        }
        Ok(())
    }

    fn dirs(&self, prefix: &str) -> Result<Vec<String>> {
        let full = format!("{}/", self.key(prefix.trim_end_matches('/')));
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut action = self.bucket.list_objects_v2(Some(&self.creds));
            action.with_prefix(full.as_str());
            action.with_delimiter("/");
            if let Some(t) = &token {
                action.with_continuation_token(t.as_str());
            }
            let url = action.sign(SIGNED_FOR);
            let mut r = self.agent.get(url.as_str()).call()?;
            if !r.status().is_success() {
                bail!("the storage bucket answered {} for a listing", r.status());
            }
            let text = r
                .body_mut()
                .with_config()
                .limit(16 << 20)
                .read_to_string()?;
            let page = ListObjectsV2::parse_response(&text)
                .map_err(|e| anyhow!("the storage bucket's listing: {e}"))?;
            for p in page.common_prefixes {
                if let Some(name) = p.prefix.strip_prefix(&full) {
                    let name = name.trim_end_matches('/');
                    if !name.is_empty() {
                        out.push(name.to_string());
                    }
                }
            }
            match page.next_continuation_token {
                Some(t) => token = Some(t),
                None => return Ok(out),
            }
        }
    }
}

pub struct Disk {
    root: PathBuf,
}

impl Disk {
    pub fn new(root: &Path) -> Disk {
        Disk {
            root: root.to_path_buf(),
        }
    }
}

impl Blobs for Disk {
    fn get(&self, key: &str) -> Result<Option<(Vec<u8>, Option<String>)>> {
        match std::fs::read(self.root.join(key)) {
            Ok(b) => Ok(Some((b, None))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {key}")),
        }
    }

    fn put(&self, key: &str, body: &[u8], _cond: Cond) -> Result<Put> {
        let path = self.root.join(key);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("partial");
        std::fs::write(&tmp, body).with_context(|| format!("writing {key}"))?;
        std::fs::File::open(&tmp)?.sync_all()?;
        std::fs::rename(&tmp, &path)?;
        Ok(Put::Done)
    }

    fn delete(&self, key: &str) -> Result<()> {
        match std::fs::remove_file(self.root.join(key)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    fn dirs(&self, prefix: &str) -> Result<Vec<String>> {
        let Ok(rd) = std::fs::read_dir(self.root.join(prefix)) else {
            return Ok(Vec::new());
        };
        Ok(rd
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect())
    }
}

pub fn load(b: &dyn Blobs) -> Result<(State, Cond)> {
    match b.get(STATE)? {
        None => Ok((State::default(), Cond::Absent)),
        Some((bytes, tag)) => {
            let st = serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                crate::log!("the launcher's state couldn't be read ({e}), so it starts over");
                State::default()
            });
            Ok((st, tag.map(Cond::Match).unwrap_or(Cond::Any)))
        }
    }
}

pub fn update<T>(b: &dyn Blobs, f: impl Fn(&mut State) -> T) -> Result<(State, T)> {
    for _ in 0..5 {
        let (mut st, cond) = load(b)?;
        let before = st.clone();
        let out = f(&mut st);
        if st == before {
            return Ok((st, out));
        }
        if b.put(STATE, &serde_json::to_vec_pretty(&st)?, cond)? == Put::Done {
            return Ok((st, out));
        }
    }
    bail!("the launcher's state kept changing under it")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_with_no_state_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("corra.db"), b"old data").unwrap();
        let d = Disk::new(&dir.path().join("launcher"));
        let (st, _) = load(&d).unwrap();
        assert_eq!(st, State::default());
        let (st, _) = update(&d, |s| s.start_with("v1")).unwrap();
        assert_eq!(st.current.as_deref(), Some("v1"));
        assert_eq!(load(&d).unwrap().0, st);
        assert_eq!(
            std::fs::read(dir.path().join("corra.db")).unwrap(),
            b"old data"
        );
    }

    #[test]
    fn a_broken_state_file_starts_over() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(STATE), b"{not json").unwrap();
        assert_eq!(load(&Disk::new(dir.path())).unwrap().0, State::default());
    }
}
