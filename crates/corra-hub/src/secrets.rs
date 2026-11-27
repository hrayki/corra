use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};

use crate::db::Db;

pub struct Secrets {
    cipher: XChaCha20Poly1305,
    key: [u8; 32],
    db: Db,
}

impl Secrets {
    pub fn open(db: Db, data: &Path) -> Result<Self> {
        let key = match std::env::var("CORRA_MASTER_KEY") {
            Ok(k) => STANDARD
                .decode(k.trim())
                .context("CORRA_MASTER_KEY must be base64")?,
            Err(_) => {
                let path = data.join("master.key");
                if path.exists() {
                    STANDARD.decode(std::fs::read_to_string(&path)?.trim())?
                } else {
                    use rand::RngExt;
                    let mut k = [0u8; 32];
                    rand::rng().fill(&mut k);
                    use std::io::Write;
                    use std::os::unix::fs::OpenOptionsExt;
                    let mut f = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(&path)?;
                    f.write_all(STANDARD.encode(k).as_bytes())?;
                    tracing::warn!(
                        "no CORRA_MASTER_KEY set; made one in {}. Keep it out of backups of the database.",
                        path.display()
                    );
                    k.to_vec()
                }
            }
        };
        let key_arr: [u8; 32] = key
            .try_into()
            .map_err(|_| anyhow!("the master key must be 32 bytes"))?;
        let s = Secrets {
            cipher: XChaCha20Poly1305::new_from_slice(&key_arr).map_err(|_| anyhow!("bad key"))?,
            key: key_arr,
            db,
        };
        s.check_key()?;
        Ok(s)
    }

    pub fn derive_hex(&self, label: &str, data: &[u8]) -> String {
        let hk = hkdf::Hkdf::<sha2::Sha256>::new(Some(&self.key), data);
        let mut out = [0u8; 32];
        hk.expand(label.as_bytes(), &mut out)
            .expect("32-byte HKDF expand");
        out.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn check_key(&self) -> Result<()> {
        const CHECK: &str = "key-check";
        match self.get(CHECK) {
            Ok(Some(_)) => Ok(()),
            Ok(None) => {
                if let Some(first) = self.names()?.into_iter().next() {
                    self.get(&first).map_err(|_| {
                        anyhow!("the master key doesn't open the saved credentials: CORRA_MASTER_KEY and master.key differ, or the key changed")
                    })?;
                }
                self.set(CHECK, "corra")
            }
            Err(_) => bail!(
                "the master key doesn't open the saved credentials: CORRA_MASTER_KEY and master.key differ, or the key changed"
            ),
        }
    }

    pub fn set(&self, name: &str, value: &str) -> Result<()> {
        use rand::RngExt;
        let mut nonce = [0u8; 24];
        rand::rng().fill(&mut nonce);
        let ct = self
            .cipher
            .encrypt(&XNonce::from(nonce), value.as_bytes())
            .map_err(|_| anyhow!("encryption failed"))?;
        self.db.kv_set(
            &format!("secret:{name}"),
            &format!("{}.{}", STANDARD.encode(nonce), STANDARD.encode(ct)),
        )
    }

    pub fn get(&self, name: &str) -> Result<Option<String>> {
        let Some(v) = self.db.kv_get(&format!("secret:{name}"))? else {
            return Ok(None);
        };
        let (n, c) = v
            .split_once('.')
            .ok_or_else(|| anyhow!("bad secret record"))?;
        let nonce = XNonce::try_from(STANDARD.decode(n)?.as_slice())
            .map_err(|_| anyhow!("bad secret record"))?;
        let pt = self
            .cipher
            .decrypt(&nonce, STANDARD.decode(c)?.as_ref())
            .map_err(|_| anyhow!("can't decrypt {name}: wrong master key?"))?;
        Ok(Some(String::from_utf8(pt)?))
    }

    pub fn delete(&self, name: &str) -> Result<()> {
        self.db.kv_delete(&format!("secret:{name}"))
    }

    pub fn names(&self) -> Result<Vec<String>> {
        Ok(self
            .db
            .kv_keys("secret:")?
            .into_iter()
            .map(|k| k.trim_start_matches("secret:").to_string())
            .filter(|n| n != "key-check")
            .collect())
    }
}
