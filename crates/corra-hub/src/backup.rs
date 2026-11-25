use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use corra_proto::now_ms;
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};
use serde_json::{Value, json};

use crate::hub::Hub;

const EVERY: Duration = Duration::from_secs(6 * 3600);

struct Target {
    bucket: Bucket,
    creds: Credentials,
    prefix: String,
}

fn target(hub: &Hub) -> Result<Option<Target>> {
    let get = |k: &str| {
        hub.secrets
            .get(k)
            .ok()
            .flatten()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let (Some(endpoint), Some(bucket), Some(key), Some(secret)) = (
        get("backup.endpoint"),
        get("backup.bucket"),
        get("backup.access_key"),
        get("backup.secret_key"),
    ) else {
        return Ok(None);
    };
    let endpoint = if endpoint.starts_with("http") {
        endpoint
    } else {
        format!("https://{endpoint}")
    };
    let region = get("backup.region").unwrap_or_else(|| "auto".into());
    let bucket = Bucket::new(endpoint.parse()?, UrlStyle::Path, bucket, region)
        .map_err(|e| anyhow!("backup bucket: {e}"))?;
    Ok(Some(Target {
        bucket,
        creds: Credentials::new(key, secret),
        prefix: get("backup.prefix").unwrap_or_else(|| "corra".into()),
    }))
}

async fn put(hub: &Hub, t: &Target, key: &str, file: &Path) -> Result<u64> {
    let url = t
        .bucket
        .put_object(Some(&t.creds), key)
        .sign(Duration::from_secs(3600));
    let f = tokio::fs::File::open(file).await?;
    let size = f.metadata().await?.len();
    let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(f));
    let r = hub
        .http
        .put(url)
        .header("content-length", size)
        .body(body)
        .send()
        .await?;
    if !r.status().is_success() {
        bail!("the backup store answered {} for {key}", r.status());
    }
    Ok(size)
}

async fn copy(hub: &Hub, t: &Target, from: &str, to: &str) -> Result<()> {
    let mut action = t.bucket.put_object(Some(&t.creds), to);
    action
        .headers_mut()
        .insert("x-amz-copy-source", format!("/{}/{from}", t.bucket.name()));
    let url = action.sign(Duration::from_secs(3600));
    let r = hub
        .http
        .put(url)
        .header("x-amz-copy-source", format!("/{}/{from}", t.bucket.name()))
        .header("content-length", 0)
        .send()
        .await?;
    let status = r.status();
    let text = r.text().await.unwrap_or_default();
    if !status.is_success() || text.contains("<Error>") {
        bail!("the backup store answered {status} copying {from} to {to}");
    }
    Ok(())
}

pub async fn run(hub: &Hub, data: &Path) -> Result<Value> {
    let Some(t) = target(hub)? else {
        bail!(
            "add backup.endpoint, backup.bucket, backup.access_key and backup.secret_key in Settings first"
        )
    };
    let snap: PathBuf = data.join("backup-snapshot.db");
    let _ = std::fs::remove_file(&snap);
    let result = async {
        let db = hub.db.clone();
        let s2 = snap.clone();
        tokio::task::spawn_blocking(move || db.snapshot_to(&s2)).await??;
        let day = now_ms() / 86_400_000;
        let latest = format!("{}/latest/corra.db", t.prefix);
        let daily = format!("{}/daily/{}/corra.db", t.prefix, day % 7);
        let db_len = put(hub, &t, &latest, &snap).await?;
        if let Err(e) = copy(hub, &t, &latest, &daily).await {
            tracing::warn!("{e:#}; uploading the daily copy instead");
            put(hub, &t, &daily, &snap).await?;
        }
        anyhow::Ok(db_len)
    }
    .await;
    let _ = tokio::fs::remove_file(&snap).await;
    let db_len = result?;
    let result = json!({ "at": now_ms(), "db_bytes": db_len });
    hub.db.kv_set("backup_last", &result.to_string())?;
    Ok(result)
}

pub fn status(hub: &Hub) -> Value {
    let last: Value = hub
        .db
        .kv_get("backup_last")
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    let error = hub.db.kv_get("backup_error").ok().flatten();
    json!({
        "configured": matches!(target(hub), Ok(Some(_))),
        "last": last,
        "error": error,
        "db_bytes": hub.db.size_bytes().ok(),
    })
}

pub fn spawn(hub: Arc<Hub>, data: PathBuf) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(120)).await;
        loop {
            if matches!(target(&hub), Ok(Some(_))) {
                let last = status(&hub)["last"]["at"].as_i64().unwrap_or(0);
                if now_ms() - last > EVERY.as_millis() as i64 {
                    match run(&hub, &data).await {
                        Ok(_) => {
                            let _ = hub.db.kv_delete("backup_error");
                        }
                        Err(e) => {
                            tracing::warn!("backup failed: {e:#}");
                            let _ = hub.db.kv_set("backup_error", &format!("{e:#}"));
                        }
                    }
                }
            }
            let swept = hub
                .db
                .kv_get("retention_last")
                .ok()
                .flatten()
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(0);
            if now_ms() - swept > 86_400_000 {
                match hub.sweep_hidden(now_ms()) {
                    Ok(n) => {
                        if n > 0 {
                            tracing::info!(
                                "deleted {n} threads hidden longer than the retention setting"
                            );
                        }
                        let _ = hub.db.kv_set("retention_last", &now_ms().to_string());
                    }
                    Err(e) => tracing::warn!("couldn't delete old hidden threads: {e:#}"),
                }
            }
            tokio::time::sleep(Duration::from_secs(600)).await;
        }
    });
}
