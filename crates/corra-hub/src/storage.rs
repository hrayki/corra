use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use corra_proto::now_ms;
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};
use serde::{Deserialize, Serialize};

pub const CHILD: &str = "CORRA_STORAGE_CHILD";
const HOLDER: &str = "CORRA_STORAGE_HOLDER";
const SOCKET: &str = "/tmp/corra-litestream.sock";

static RELEASED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub struct Storage {
    endpoint: String,
    bucket_name: String,
    prefix: String,
    region: String,
    bucket: Bucket,
    creds: Credentials,
    http: reqwest::Client,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lease {
    pub holder: String,
    pub at: i64,
    pub released: bool,
}

#[derive(Clone, Debug)]
pub enum Cond {
    Absent,
    Match(String),
}

#[derive(Debug, PartialEq)]
pub enum Put {
    Done,
    Conflict,
}

#[async_trait::async_trait]
pub trait LeaseStore: Send + Sync {
    async fn get(&self) -> Result<Option<(Lease, String)>>;
    async fn put(&self, lease: &Lease, cond: Cond) -> Result<Put>;
}

#[derive(Clone, Copy)]
pub struct Timing {
    pub renew: Duration,
    pub give_up: Duration,
    pub stale_ms: i64,
    pub poll: Duration,
}

pub const TIMING: Timing = Timing {
    renew: Duration::from_secs(5),
    give_up: Duration::from_secs(15),
    stale_ms: 30_000,
    poll: Duration::from_secs(2),
};

#[derive(Debug, PartialEq)]
pub enum Lost {
    TakenBy(String),
    Unreachable,
}

pub async fn take(st: &dyn LeaseStore, holder: &str, t: Timing) -> Result<()> {
    let mut said = false;
    loop {
        let cond = match st.get().await? {
            None => Some(Cond::Absent),
            Some((l, tag)) if l.released => Some(Cond::Match(tag)),
            Some((l, tag)) if now_ms() - l.at > t.stale_ms => {
                tracing::warn!(
                    "the last hub stopped without handing over; carrying on from its last copy"
                );
                Some(Cond::Match(tag))
            }
            Some(_) => None,
        };
        if let Some(cond) = cond {
            let lease = Lease {
                holder: holder.to_string(),
                at: now_ms(),
                released: false,
            };
            if st.put(&lease, cond).await? == Put::Done
                && st.get().await?.is_some_and(|(l, _)| l.holder == holder)
            {
                return Ok(());
            }
            tokio::time::sleep(t.poll).await;
            continue;
        }
        if !said {
            tracing::info!("waiting for the running hub to hand over its data");
            said = true;
        }
        tokio::time::sleep(t.poll).await;
    }
}

async fn renew(st: &dyn LeaseStore, holder: &str) -> Result<std::result::Result<(), String>> {
    let Some((l, tag)) = st.get().await? else {
        return Ok(Err("nobody".into()));
    };
    if l.holder != holder {
        return Ok(Err(l.holder));
    }
    let lease = Lease {
        holder: holder.to_string(),
        at: now_ms(),
        released: false,
    };
    match st.put(&lease, Cond::Match(tag)).await? {
        Put::Done => Ok(Ok(())),
        Put::Conflict => bail!("the lease changed while it was being renewed"),
    }
}

pub async fn keep(
    st: &dyn LeaseStore,
    holder: &str,
    t: Timing,
    released: &std::sync::atomic::AtomicBool,
) -> Option<Lost> {
    let mut last_ok = std::time::Instant::now();
    loop {
        if released.load(std::sync::atomic::Ordering::SeqCst) {
            return None;
        }
        let attempt = tokio::time::timeout(t.give_up, renew(st, holder)).await;
        if released.load(std::sync::atomic::Ordering::SeqCst) {
            return None;
        }
        match attempt {
            Ok(Ok(Ok(()))) => last_ok = std::time::Instant::now(),
            Ok(Ok(Err(other))) => return Some(Lost::TakenBy(other)),
            Ok(Err(e)) => tracing::warn!("couldn't renew the storage lease: {e:#}"),
            Err(_) => tracing::warn!("renewing the storage lease timed out"),
        }
        if last_ok.elapsed() >= t.give_up {
            return Some(Lost::Unreachable);
        }
        tokio::time::sleep(t.renew).await;
    }
}

pub async fn release(st: &dyn LeaseStore, holder: &str) -> Result<()> {
    let Some((l, tag)) = st.get().await? else {
        return Ok(());
    };
    if l.holder != holder {
        bail!("another hub holds the lease");
    }
    let lease = Lease {
        holder: holder.to_string(),
        at: now_ms(),
        released: true,
    };
    match st.put(&lease, Cond::Match(tag)).await? {
        Put::Done => Ok(()),
        Put::Conflict => bail!("the lease changed while it was being released"),
    }
}

pub fn from_env() -> Result<Option<Arc<Storage>>> {
    let get = |k: &str| {
        std::env::var(k)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let names = [
        "CORRA_STORAGE_ENDPOINT",
        "CORRA_STORAGE_BUCKET",
        "CORRA_STORAGE_ACCESS_KEY",
        "CORRA_STORAGE_SECRET_KEY",
    ];
    let vals: Vec<Option<String>> = names.iter().map(|n| get(n)).collect();
    if vals.iter().all(Option::is_none) {
        return Ok(None);
    }
    if vals.iter().any(Option::is_none) {
        bail!(
            "set all of {} for remote storage, or none",
            names.join(", ")
        );
    }
    for k in ["CORRA_MASTER_KEY", "CORRA_ACCESS_KEY"] {
        if get(k).is_none() {
            bail!("with remote storage, set {k}: the hub's disk doesn't last");
        }
    }
    let endpoint = vals[0].clone().unwrap();
    let endpoint = if endpoint.starts_with("http") {
        endpoint
    } else {
        format!("https://{endpoint}")
    };
    let bucket_name = vals[1].clone().unwrap();
    let region = get("CORRA_STORAGE_REGION").unwrap_or_else(|| "auto".into());
    let bucket = Bucket::new(
        endpoint.parse()?,
        UrlStyle::Path,
        bucket_name.clone(),
        region.clone(),
    )
    .map_err(|e| anyhow!("storage bucket: {e}"))?;
    Ok(Some(Arc::new(Storage {
        endpoint,
        bucket_name,
        prefix: get("CORRA_STORAGE_PREFIX").unwrap_or_else(|| "corra".into()),
        region,
        bucket,
        creds: Credentials::new(vals[2].clone().unwrap(), vals[3].clone().unwrap()),
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()?,
    })))
}

impl Storage {
    fn lease_key(&self) -> String {
        format!("{}/lease.json", self.prefix)
    }

    fn litestream_config(&self, db: &Path) -> String {
        format!(
            "socket:\n  enabled: true\n  path: {SOCKET}\ndbs:\n  - path: {}\n    replica:\n      type: s3\n      bucket: {}\n      path: {}/db\n      endpoint: {}\n      region: {}\n      force-path-style: true\n      access-key-id: ${{CORRA_STORAGE_ACCESS_KEY}}\n      secret-access-key: ${{CORRA_STORAGE_SECRET_KEY}}\n",
            db.display(),
            self.bucket_name,
            self.prefix,
            self.endpoint,
            self.region
        )
    }
}

#[async_trait::async_trait]
impl LeaseStore for Storage {
    async fn get(&self) -> Result<Option<(Lease, String)>> {
        let url = self
            .bucket
            .get_object(Some(&self.creds), &self.lease_key())
            .sign(Duration::from_secs(300));
        let r = self.http.get(url).send().await?;
        if r.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !r.status().is_success() {
            bail!("the storage bucket answered {}", r.status());
        }
        let tag = r
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| anyhow!("the storage bucket gave the lease no ETag"))?
            .to_string();
        let bytes = r.bytes().await?;
        let lease = serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            tracing::warn!("the storage lease file is unreadable ({e}); replacing it");
            Lease {
                holder: String::new(),
                at: 0,
                released: true,
            }
        });
        Ok(Some((lease, tag)))
    }

    async fn put(&self, lease: &Lease, cond: Cond) -> Result<Put> {
        let url = self
            .bucket
            .put_object(Some(&self.creds), &self.lease_key())
            .sign(Duration::from_secs(300));
        let req = self.http.put(url).body(serde_json::to_vec(lease)?);
        let req = match cond {
            Cond::Absent => req.header("if-none-match", "*"),
            Cond::Match(tag) => req.header("if-match", tag),
        };
        let r = req.send().await?;
        if r.status() == reqwest::StatusCode::PRECONDITION_FAILED
            || r.status() == reqwest::StatusCode::CONFLICT
        {
            return Ok(Put::Conflict);
        }
        if !r.status().is_success() {
            bail!("the storage bucket answered {}", r.status());
        }
        Ok(Put::Done)
    }
}

fn litestream_bin() -> PathBuf {
    std::env::var_os("CORRA_LITESTREAM")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|e| e.parent().map(|d| d.join("litestream")))
        })
        .unwrap_or_else(|| PathBuf::from("litestream"))
}

pub async fn run_under_litestream(
    st: Arc<Storage>,
    data: &Path,
    listen: std::net::SocketAddr,
) -> Result<()> {
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let listener = tokio::net::TcpListener::bind(listen).await?;
    let starting = axum::Router::new()
        .route("/healthz", axum::routing::get(|| async { "ok" }))
        .fallback(|| async {
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "corra is starting",
            )
        });
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, starting)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await;
    });
    let holder = crate::hub::new_token("hold");
    take(st.as_ref(), &holder, TIMING).await?;
    let _ = stop.send(());
    let _ = server.await;
    let db = data.join("corra.db");
    let config = std::env::temp_dir().join("corra-litestream.yml");
    std::fs::write(&config, st.litestream_config(&db))?;
    if let Err(e) = restore(&config, &db).await {
        let _ = release(st.as_ref(), &holder).await;
        return Err(e);
    }
    let exe = std::env::current_exe()?;
    tracing::info!("starting the hub under litestream");
    let err = std::process::Command::new(litestream_bin())
        .arg("replicate")
        .arg("-config")
        .arg(&config)
        .arg("-exec")
        .arg(format!("{} serve", exe.display()))
        .env(CHILD, "1")
        .env(HOLDER, &holder)
        .exec();
    Err(anyhow!("couldn't start litestream: {err}"))
}

async fn restore(config: &Path, db: &Path) -> Result<()> {
    tracing::info!("restoring the hub's data if needed");
    let out = tokio::process::Command::new(litestream_bin())
        .arg("restore")
        .arg("-config")
        .arg(config)
        .args(["-if-db-not-exists", "-if-replica-exists"])
        .arg(db)
        .output()
        .await
        .map_err(|e| anyhow!("couldn't start litestream: {e}"))?;
    if !out.status.success() {
        bail!(
            "litestream couldn't restore the hub's data: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    if let Some(v) = crate::db::schema_of(db)?
        && v > crate::db::SCHEMA_VERSION
    {
        return Err(crate::db::NewerDatabase(v).into());
    }
    Ok(())
}

pub fn hold(st: Arc<Storage>) -> String {
    let holder = std::env::var(HOLDER).unwrap_or_default();
    let h = holder.clone();
    tokio::spawn(async move {
        if let Some(lost) = keep(st.as_ref(), &h, TIMING, &RELEASED).await {
            match lost {
                Lost::TakenBy(other) => tracing::error!(
                    "another hub ({other}) holds the storage lease now, so this one stops"
                ),
                Lost::Unreachable => tracing::error!(
                    "the storage lease couldn't be renewed for {}s, so another hub may take over; this one stops",
                    TIMING.give_up.as_secs()
                ),
            }
            std::process::exit(1);
        }
    });
    holder
}

pub async fn hand_over(st: &Storage, holder: &str, data: &Path) {
    RELEASED.store(true, std::sync::atomic::Ordering::SeqCst);
    let synced = tokio::process::Command::new(litestream_bin())
        .args(["sync", "-wait", "-timeout", "20", "-socket", SOCKET])
        .arg(data.join("corra.db"))
        .output()
        .await;
    match synced {
        Ok(o) if o.status.success() => tracing::info!("the hub's last changes are copied"),
        Ok(o) => tracing::warn!(
            "litestream couldn't sync: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => tracing::warn!("litestream couldn't sync: {e}"),
    }
    for attempt in 1..=3 {
        match release(st, holder).await {
            Ok(()) => return,
            Err(e) if attempt == 3 => {
                tracing::warn!("couldn't hand over the storage lease: {e:#}")
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    }
}
