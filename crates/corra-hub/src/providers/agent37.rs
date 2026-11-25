use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use serde_json::{Value, json};

use super::{BASE_DOCKERFILE, CreateRequest, Provider, ProviderStatus, boot_command};
use crate::hub::Hub;

const API: &str = "https://api.agent37.com/v1";

pub struct Agent37;

fn key(hub: &Hub) -> Result<String> {
    hub.secrets
        .get("agent37.key")?
        .map(|k| k.split_whitespace().collect())
        .ok_or_else(|| anyhow!("add agent37.key in Settings → Credentials"))
}

async fn send(rb: reqwest::RequestBuilder) -> Result<Value> {
    let r = rb.send().await?;
    let status = r.status();
    let text = r.text().await?;
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::String(text.clone()));
    if !status.is_success() {
        let msg = v["error"]["message"]
            .as_str()
            .or(v["message"].as_str())
            .unwrap_or(text.as_str())
            .to_string();
        bail!("Agent37 answered {status}: {msg}");
    }
    Ok(v)
}

impl Agent37 {
    async fn api(
        &self,
        hub: &Hub,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let mut rb = hub
            .http
            .request(method, format!("{API}{path}"))
            .bearer_auth(key(hub)?);
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        send(rb).await
    }
}

fn dockerfile() -> String {
    let cmd = serde_json::to_string(&boot_command()).unwrap_or_default();
    format!("{}\nCMD {cmd}\n", BASE_DOCKERFILE.trim_end())
}

fn context_tar() -> Result<Vec<u8>> {
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut tar = tar::Builder::new(gz);
    let df = dockerfile();
    let mut h = tar::Header::new_gnu();
    h.set_size(df.len() as u64);
    h.set_mode(0o644);
    h.set_cksum();
    tar.append_data(&mut h, "Dockerfile", df.as_bytes())?;
    Ok(tar.into_inner()?.finish()?)
}

#[async_trait]
impl Provider for Agent37 {
    fn name(&self) -> &'static str {
        "agent37"
    }

    fn creates_machines(&self) -> bool {
        true
    }

    async fn base_image(&self, hub: &Hub) -> Result<String> {
        let hash = crate::hub::hash_token(&dockerfile());
        let kv = format!("agent37.base_image:{hash}");
        if let Some(t) = hub.db.kv_get(&kv)? {
            return Ok(t);
        }
        let name = format!("corra-base-{}", &hash[..10]);
        let ctx = context_tar()?;
        let b = self
            .api(
                hub,
                reqwest::Method::POST,
                "/template-builds",
                Some(json!({ "name": name, "size_bytes": ctx.len(), "description": "corra machine base" })),
            )
            .await?;
        let id = b["id"]
            .as_str()
            .ok_or_else(|| anyhow!("no build id"))?
            .to_string();
        let url = b["upload_url"]
            .as_str()
            .ok_or_else(|| anyhow!("no upload url"))?;
        let r = hub.http.put(url).body(ctx).send().await?;
        if !r.status().is_success() {
            bail!("uploading the build context failed: {}", r.status());
        }
        self.api(
            hub,
            reqwest::Method::POST,
            &format!("/template-builds/{id}/start"),
            Some(json!({})),
        )
        .await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(50 * 60);
        loop {
            let v = self
                .api(
                    hub,
                    reqwest::Method::GET,
                    &format!("/template-builds/{id}"),
                    None,
                )
                .await?;
            match v["status"].as_str().unwrap_or("") {
                "succeeded" => break,
                "failed" => bail!(
                    "the Agent37 image build failed: {}",
                    v["error"]["message"].as_str().unwrap_or("unknown")
                ),
                _ => {}
            }
            if tokio::time::Instant::now() > deadline {
                bail!("the Agent37 image build took too long");
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        let image = format!("template:{name}");
        hub.db.kv_set(&kv, &image)?;
        Ok(image)
    }

    async fn create(&self, hub: &Hub, req: CreateRequest) -> Result<String> {
        let image = self.base_image(hub).await?;
        let template = image
            .strip_prefix("template:")
            .ok_or_else(|| anyhow!("unknown Agent37 image {image}"))?;
        let env: serde_json::Map<String, Value> =
            req.env.into_iter().map(|(k, v)| (k, json!(v))).collect();
        let resources = json!({ "cpu": req.size.cpu, "memory": req.size.memory_gb, "disk": 40 });
        let v = self
            .api(
                hub,
                reqwest::Method::POST,
                "/instances",
                Some(json!({ "template": template, "type": "vm", "resources": resources, "env": env, "name": format!("{}-{}", req.name, super::hub_tag(hub)) })),
            )
            .await?;
        Ok(v["id"]
            .as_str()
            .ok_or_else(|| anyhow!("no instance id"))?
            .to_string())
    }

    fn sleeps(&self) -> bool {
        true
    }

    async fn sleep(&self, hub: &Hub, provider_ref: &str) -> Result<String> {
        self.api(
            hub,
            reqwest::Method::POST,
            &format!("/instances/{provider_ref}/stop"),
            Some(json!({})),
        )
        .await?;
        Ok(provider_ref.to_string())
    }

    async fn wake(&self, hub: &Hub, sleep_ref: &str, _req: CreateRequest) -> Result<String> {
        self.api(
            hub,
            reqwest::Method::POST,
            &format!("/instances/{sleep_ref}/start"),
            Some(json!({})),
        )
        .await?;
        Ok(sleep_ref.to_string())
    }

    async fn forget(&self, hub: &Hub, sleep_ref: &str) -> Result<()> {
        self.destroy(hub, sleep_ref).await
    }

    async fn running(&self, hub: &Hub) -> Result<Vec<String>> {
        let v = self
            .api(hub, reqwest::Method::GET, "/instances", None)
            .await?;
        let list = if v.is_array() {
            v
        } else {
            v.get("data")
                .or_else(|| v.get("instances"))
                .cloned()
                .unwrap_or_default()
        };
        Ok(list
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|i| {
                        i["name"].as_str().is_some_and(|n| {
                            n.starts_with("corra-")
                                && n.ends_with(&format!("-{}", super::hub_tag(hub)))
                        })
                    })
                    .filter(|i| !matches!(i["status"].as_str(), Some("deleted" | "deleting")))
                    .filter_map(|i| i["id"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn destroy(&self, hub: &Hub, provider_ref: &str) -> Result<()> {
        match self
            .api(
                hub,
                reqwest::Method::DELETE,
                &format!("/instances/{provider_ref}"),
                None,
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(e) if e.to_string().contains("404") => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn status(&self, hub: &Hub, provider_ref: &str) -> Result<ProviderStatus> {
        match self
            .api(
                hub,
                reqwest::Method::GET,
                &format!("/instances/{provider_ref}"),
                None,
            )
            .await
        {
            Ok(v) => Ok(match v["status"].as_str().unwrap_or("") {
                "stopped" => ProviderStatus::Stopped,
                "deleted" | "deleting" | "failed" => ProviderStatus::Gone,
                _ => ProviderStatus::Running,
            }),
            Err(e) if e.to_string().contains("404") => Ok(ProviderStatus::Gone),
            Err(e) => Err(e),
        }
    }
}
