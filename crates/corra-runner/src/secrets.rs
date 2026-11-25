use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, bail};

pub const KEPT_FOR: Duration = Duration::from_secs(24 * 3600);

pub fn dir() -> PathBuf {
    crate::files::home().join(".corra/secrets")
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

pub fn write(dir: &Path, name: &str, value: &str) -> Result<PathBuf> {
    if !valid_name(name) {
        bail!("a secret's name may use letters, digits, '_', '-' and '.'");
    }
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    let path = dir.join(name);
    let _ = std::fs::remove_file(&path);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    f.write_all(value.as_bytes())?;
    Ok(path)
}

pub fn sweep(dir: &Path, kept_for: Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age >= kept_for);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_lands_in_a_file_only_its_owner_reads_and_is_swept_when_old() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("secrets");
        let path = write(&dir, "API_KEY", "first").unwrap();
        let path = write(&dir, "API_KEY", "s3cret\n").and(Ok(path)).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "s3cret\n");
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&dir), 0o700);

        for bad in ["", "../x", "a/b", ".hidden", "sp ace", &"x".repeat(65)] {
            assert!(write(&dir, bad, "v").is_err(), "{bad:?} was taken");
        }
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);

        sweep(&dir, Duration::from_secs(3600));
        assert!(path.exists());
        sweep(&dir, Duration::ZERO);
        assert!(!path.exists());
    }
}
