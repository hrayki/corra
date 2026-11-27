use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};

use crate::state::valid_version;
use crate::store::{Blobs, Cond};

pub const FILES: [&str; 3] = ["corra-hub", "corra-runner", "corra-modal"];
pub const SUMS: &str = "SHA256SUMS";
pub const VERSION_FILE: &str = "VERSION";

pub trait Releases: Send + Sync {
    fn newest(&self) -> Result<String>;
    fn get(&self, version: &str, name: &str) -> Result<Vec<u8>>;
}

pub struct Http {
    base: String,
    agent: ureq::Agent,
}

impl Http {
    pub fn new(base: &str) -> Http {
        Http {
            base: base.trim_end_matches('/').to_string(),
            agent: crate::http_agent(),
        }
    }

    fn fetch(&self, url: &str) -> Result<Vec<u8>> {
        let mut r = self
            .agent
            .get(url)
            .call()
            .with_context(|| format!("fetching {url}"))?;
        if !r.status().is_success() {
            bail!("{url} answered {}", r.status());
        }
        Ok(r.body_mut().with_config().limit(1 << 30).read_to_vec()?)
    }
}

impl Releases for Http {
    fn newest(&self) -> Result<String> {
        let body = self.fetch(&format!("{}/latest/download/{VERSION_FILE}", self.base))?;
        let v = String::from_utf8_lossy(&body).trim().to_string();
        if !valid_version(&v) {
            bail!("the newest release names itself {v:?}, which isn't a version");
        }
        Ok(v)
    }

    fn get(&self, version: &str, name: &str) -> Result<Vec<u8>> {
        self.fetch(&format!("{}/download/{version}/{name}", self.base))
    }
}

pub fn parse_sums(text: &str) -> Result<HashMap<String, String>> {
    let mut out = HashMap::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let (hash, name) = line
            .split_once(char::is_whitespace)
            .ok_or_else(|| anyhow!("{SUMS} has a line that isn't a checksum: {line:?}"))?;
        if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            bail!("{SUMS} has a line that isn't a checksum: {line:?}");
        }
        let name = name.trim().trim_start_matches('*');
        out.insert(name.to_string(), hash.to_ascii_lowercase());
    }
    Ok(out)
}

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn check(sums: &HashMap<String, String>, name: &str, bytes: &[u8]) -> Result<()> {
    let want = sums
        .get(name)
        .ok_or_else(|| anyhow!("{SUMS} doesn't list {name}"))?;
    if *want != hex(bytes) {
        bail!("{name} doesn't match its checksum in {SUMS}");
    }
    Ok(())
}

pub struct Files {
    sums: Vec<u8>,
    files: Vec<(&'static str, Vec<u8>)>,
}

fn verified(sums: Vec<u8>, get: impl Fn(&str) -> Result<Option<Vec<u8>>>) -> Result<Option<Files>> {
    let table = parse_sums(&String::from_utf8_lossy(&sums))?;
    let mut files = Vec::new();
    for name in FILES {
        let Some(bytes) = get(name)? else {
            return Ok(None);
        };
        check(&table, name, &bytes)?;
        files.push((name, bytes));
    }
    Ok(Some(Files { sums, files }))
}

fn from_blobs(b: &dyn Blobs, version: &str) -> Result<Option<Files>> {
    let Some((sums, _)) = b.get(&format!("releases/{version}/{SUMS}"))? else {
        return Ok(None);
    };
    verified(sums, |name| {
        Ok(b.get(&format!("releases/{version}/{name}"))?
            .map(|(x, _)| x))
    })
}

fn from_releases(r: &dyn Releases, version: &str) -> Result<Files> {
    let sums = r.get(version, SUMS)?;
    verified(sums, |name| r.get(version, name).map(Some))?
        .ok_or_else(|| anyhow!("release {version} is missing a file"))
}

fn upload(b: &dyn Blobs, version: &str, f: &Files) -> Result<()> {
    for (name, bytes) in &f.files {
        b.put(&format!("releases/{version}/{name}"), bytes, Cond::Any)?;
    }
    b.put(&format!("releases/{version}/{SUMS}"), &f.sums, Cond::Any)?;
    Ok(())
}

pub fn local_dir(local: &Path, version: &str) -> PathBuf {
    local.join("releases").join(version)
}

fn verify_dir(dir: &Path) -> Result<()> {
    let sums = std::fs::read(dir.join(SUMS))?;
    verified(sums, |name| Ok(Some(std::fs::read(dir.join(name))?)))?
        .map(|_| ())
        .ok_or_else(|| anyhow!("incomplete"))
}

fn write_dir(local: &Path, version: &str, f: &Files) -> Result<PathBuf> {
    let dir = local_dir(local, version);
    let tmp = local.join("releases").join(format!(".{version}.partial"));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    for (name, bytes) in &f.files {
        let p = tmp.join(name);
        std::fs::write(&p, bytes)?;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::write(tmp.join(SUMS), &f.sums)?;
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&tmp, &dir)?;
    Ok(dir)
}

#[derive(Debug, PartialEq)]
pub enum From {
    Disk,
    Bucket,
    Release,
}

pub fn ensure(
    version: &str,
    local: &Path,
    bucket: Option<&dyn Blobs>,
    releases: &dyn Releases,
) -> Result<(PathBuf, From)> {
    if !valid_version(version) {
        bail!("{version:?} isn't a version");
    }
    let dir = local_dir(local, version);
    if verify_dir(&dir).is_ok() {
        return Ok((dir, From::Disk));
    }
    if let Some(b) = bucket {
        match from_blobs(b, version) {
            Ok(Some(f)) => return Ok((write_dir(local, version, &f)?, From::Bucket)),
            Ok(None) => {}
            Err(e) => crate::log!(
                "the bucket's copy of {version} is no good ({e:#}), so it's fetched again"
            ),
        }
    }
    let f =
        from_releases(releases, version).with_context(|| format!("getting release {version}"))?;
    if let Some(b) = bucket
        && let Err(e) = upload(b, version, &f)
    {
        crate::log!("couldn't keep a copy of {version} in the bucket: {e:#}");
    }
    Ok((write_dir(local, version, &f)?, From::Release))
}

pub fn prune(local: &Path, bucket: Option<&dyn Blobs>, keep: &[String]) {
    let gone = |v: &str| !keep.iter().any(|k| k == v);
    if let Ok(rd) = std::fs::read_dir(local.join("releases")) {
        for e in rd.flatten() {
            let Ok(name) = e.file_name().into_string() else {
                continue;
            };
            let version = name
                .strip_prefix('.')
                .and_then(|n| n.strip_suffix(".partial"))
                .unwrap_or(&name);
            if gone(version) {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    let Some(b) = bucket else { return };
    match b.dirs("releases") {
        Ok(versions) => {
            for v in versions.iter().filter(|v| gone(v)) {
                for name in [SUMS].iter().chain(FILES.iter()) {
                    if let Err(e) = b.delete(&format!("releases/{v}/{name}")) {
                        crate::log!("couldn't delete {v} from the bucket: {e:#}");
                        break;
                    }
                }
            }
        }
        Err(e) => crate::log!("couldn't list the bucket's releases: {e:#}"),
    }
}

#[cfg(test)]
pub mod testing {
    use super::*;
    use std::sync::Mutex;

    pub struct Fake {
        dir: PathBuf,
        calls: Mutex<Vec<String>>,
    }

    impl Fake {
        pub fn new(dir: &Path) -> Fake {
            Fake {
                dir: dir.to_path_buf(),
                calls: Mutex::new(Vec::new()),
            }
        }

        pub fn publish(&self, version: &str, tamper: Option<&str>) {
            let d = self.dir.join(version);
            std::fs::create_dir_all(&d).unwrap();
            let mut sums = String::new();
            for name in FILES {
                let body = format!("#!/bin/sh\necho {name} {version}\n");
                sums.push_str(&format!("{}  {name}\n", hex(body.as_bytes())));
                let body = if tamper == Some(name) {
                    format!("{body}tampered\n")
                } else {
                    body
                };
                std::fs::write(d.join(name), body).unwrap();
            }
            std::fs::write(d.join(SUMS), sums).unwrap();
            std::fs::write(self.dir.join(VERSION_FILE), version).unwrap();
        }

        pub fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Releases for Fake {
        fn newest(&self) -> Result<String> {
            self.calls.lock().unwrap().push("newest".into());
            Ok(std::fs::read_to_string(self.dir.join(VERSION_FILE))?)
        }

        fn get(&self, version: &str, name: &str) -> Result<Vec<u8>> {
            self.calls.lock().unwrap().push(format!("{version}/{name}"));
            Ok(std::fs::read(self.dir.join(version).join(name))?)
        }
    }

    pub struct Gate<B> {
        pub inner: B,
        pub at: String,
        pub reached: Mutex<Option<std::sync::mpsc::Sender<()>>>,
        pub open: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    }

    impl<B: Blobs> Blobs for Gate<B> {
        fn get(&self, key: &str) -> Result<Option<(Vec<u8>, Option<String>)>> {
            self.inner.get(key)
        }

        fn put(&self, key: &str, body: &[u8], cond: Cond) -> Result<crate::store::Put> {
            if key == self.at {
                if let Some(r) = self.reached.lock().unwrap().take() {
                    let _ = r.send(());
                }
                if let Some(o) = self.open.lock().unwrap().take() {
                    let _ = o.recv();
                }
            }
            self.inner.put(key, body, cond)
        }

        fn delete(&self, key: &str) -> Result<()> {
            self.inner.delete(key)
        }

        fn dirs(&self, prefix: &str) -> Result<Vec<String>> {
            self.inner.dirs(prefix)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::Fake;
    use super::*;
    use crate::store::Disk;

    struct Dirs {
        _t: tempfile::TempDir,
        local: PathBuf,
        bucket: Disk,
        releases: Fake,
    }

    fn dirs() -> Dirs {
        let t = tempfile::tempdir().unwrap();
        let local = t.path().join("data/launcher");
        let bucket = Disk::new(&t.path().join("bucket"));
        let releases = Fake::new(&t.path().join("releases"));
        Dirs {
            _t: t,
            local,
            bucket,
            releases,
        }
    }

    #[test]
    fn a_new_version_comes_from_the_release_and_is_kept_in_the_bucket() {
        let d = dirs();
        d.releases.publish("v1", None);
        let (dir, from) = ensure("v1", &d.local, Some(&d.bucket), &d.releases).unwrap();
        assert_eq!(from, From::Release);
        for name in FILES {
            let meta = std::fs::metadata(dir.join(name)).unwrap();
            assert_eq!(meta.permissions().mode() & 0o111, 0o111);
            assert!(
                d.bucket
                    .get(&format!("releases/v1/{name}"))
                    .unwrap()
                    .is_some()
            );
        }
        assert!(d.bucket.get("releases/v1/SHA256SUMS").unwrap().is_some());
    }

    #[test]
    fn a_cached_version_needs_no_release_server() {
        let d = dirs();
        d.releases.publish("v1", None);
        ensure("v1", &d.local, Some(&d.bucket), &d.releases).unwrap();
        let before = d.releases.calls().len();
        assert_eq!(
            ensure("v1", &d.local, Some(&d.bucket), &d.releases)
                .unwrap()
                .1,
            From::Disk
        );
        std::fs::remove_dir_all(&d.local).unwrap();
        assert_eq!(
            ensure("v1", &d.local, Some(&d.bucket), &d.releases)
                .unwrap()
                .1,
            From::Bucket
        );
        assert_eq!(d.releases.calls().len(), before);
    }

    #[test]
    fn without_a_bucket_the_data_folder_is_the_cache() {
        let d = dirs();
        d.releases.publish("v1", None);
        ensure("v1", &d.local, None, &d.releases).unwrap();
        let before = d.releases.calls().len();
        assert_eq!(
            ensure("v1", &d.local, None, &d.releases).unwrap().1,
            From::Disk
        );
        assert_eq!(d.releases.calls().len(), before);
    }

    #[test]
    fn a_checksum_mismatch_is_refused_and_nothing_is_kept() {
        let d = dirs();
        d.releases.publish("v1", Some("corra-runner"));
        let err = ensure("v1", &d.local, Some(&d.bucket), &d.releases).unwrap_err();
        assert!(
            format!("{err:#}").contains("corra-runner doesn't match its checksum"),
            "{err:#}"
        );
        assert!(!local_dir(&d.local, "v1").exists());
        assert!(d.bucket.get("releases/v1/SHA256SUMS").unwrap().is_none());
    }

    #[test]
    fn a_file_missing_from_the_sums_is_refused() {
        let sums = parse_sums(
            "0000000000000000000000000000000000000000000000000000000000000000  corra-hub\n",
        )
        .unwrap();
        assert!(check(&sums, "corra-runner", b"x").is_err());
        assert!(parse_sums("not a checksum line\n").is_err());
    }

    #[test]
    fn a_damaged_copy_on_disk_or_in_the_bucket_is_fetched_again() {
        let d = dirs();
        d.releases.publish("v1", None);
        let (dir, _) = ensure("v1", &d.local, Some(&d.bucket), &d.releases).unwrap();
        std::fs::write(dir.join("corra-hub"), b"half a file").unwrap();
        d.bucket
            .put("releases/v1/corra-hub", b"half a file", Cond::Any)
            .unwrap();
        let (dir, from) = ensure("v1", &d.local, Some(&d.bucket), &d.releases).unwrap();
        assert_eq!(from, From::Release);
        assert!(
            std::fs::read_to_string(dir.join("corra-hub"))
                .unwrap()
                .contains("corra-hub v1")
        );
        let (bytes, _) = d.bucket.get("releases/v1/corra-hub").unwrap().unwrap();
        assert!(String::from_utf8(bytes).unwrap().contains("corra-hub v1"));
    }

    #[test]
    fn prune_keeps_only_what_it_is_told() {
        let d = dirs();
        for v in ["v1", "v2", "v3"] {
            d.releases.publish(v, None);
            ensure(v, &d.local, Some(&d.bucket), &d.releases).unwrap();
        }
        prune(&d.local, Some(&d.bucket), &["v2".into(), "v3".into()]);
        assert!(!local_dir(&d.local, "v1").exists());
        assert!(local_dir(&d.local, "v2").exists());
        let mut left = d.bucket.dirs("releases").unwrap();
        left.sort();
        assert!(d.bucket.get("releases/v1/corra-hub").unwrap().is_none());
        assert!(d.bucket.get("releases/v3/corra-hub").unwrap().is_some());
        assert!(left.contains(&"v2".to_string()));
    }

    #[test]
    fn prune_spares_the_download_of_a_version_it_keeps() {
        let d = dirs();
        let releases = d.local.join("releases");
        std::fs::create_dir_all(releases.join(".v3.partial")).unwrap();
        std::fs::create_dir_all(releases.join(".v0.partial")).unwrap();
        prune(&d.local, None, &["v2".into(), "v3".into()]);
        assert!(releases.join(".v3.partial").exists());
        assert!(!releases.join(".v0.partial").exists());
    }

    #[test]
    fn a_version_that_isnt_a_plain_name_is_refused() {
        let d = dirs();
        assert!(ensure("../../etc", &d.local, None, &d.releases).is_err());
        assert!(d.releases.calls().is_empty());
    }
}
