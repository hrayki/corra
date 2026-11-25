use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;

use super::{
    BASE_DOCKERFILE, CreateRequest, Credential, Description, Kind, Provider, ProviderStatus,
    boot_command,
};
use crate::hub::Hub;

pub struct Modal {
    helper: std::path::PathBuf,
}

impl Modal {
    pub fn new() -> Self {
        let helper = std::env::var_os("CORRA_MODAL_HELPER")
            .map(Into::into)
            .or_else(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|e| e.parent().map(|d| d.join("corra-modal")))
            })
            .unwrap_or_else(|| "corra-modal".into());
        Modal { helper }
    }

    async fn call(&self, hub: &Hub, req: Value) -> Result<Value> {
        let id = hub
            .secrets
            .get("modal.token_id")?
            .ok_or_else(|| anyhow!("add modal.token_id in Settings → Credentials"))?;
        let secret = hub
            .secrets
            .get("modal.token_secret")?
            .ok_or_else(|| anyhow!("add modal.token_secret in Settings → Credentials"))?;
        let mut child = tokio::process::Command::new(&self.helper)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env(
                "HOME",
                std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()),
            )
            .env("MODAL_TOKEN_ID", id)
            .env("MODAL_TOKEN_SECRET", secret)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("starting {}", self.helper.display()))?;
        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(serde_json::to_vec(&req)?.as_slice())
            .await?;
        drop(stdin);
        let out = tokio::time::timeout(Duration::from_secs(45 * 60), child.wait_with_output())
            .await
            .map_err(|_| anyhow!("Modal took too long"))??;
        let v: Value = serde_json::from_slice(&out.stdout).map_err(|_| {
            anyhow!(
                "Modal helper failed: {}",
                String::from_utf8_lossy(&out.stderr)
                    .lines()
                    .last()
                    .unwrap_or("")
            )
        })?;
        if let Some(e) = v["error"].as_str() {
            bail!("Modal: {e}");
        }
        Ok(v)
    }
}

fn dockerfile_commands() -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for line in BASE_DOCKERFILE.lines() {
        let l = line.trim_end();
        if l.is_empty() || l.starts_with("FROM ") || l.starts_with('#') {
            continue;
        }
        match out.last_mut() {
            Some(prev) if prev.ends_with('\\') => {
                prev.push('\n');
                prev.push_str(l);
            }
            _ => out.push(l.to_string()),
        }
    }
    out
}

fn base_registry() -> String {
    BASE_DOCKERFILE
        .lines()
        .find_map(|l| l.strip_prefix("FROM "))
        .unwrap_or("ubuntu:24.04")
        .trim()
        .to_string()
}

impl Modal {
    async fn start(&self, hub: &Hub, image: &str, req: CreateRequest) -> Result<String> {
        let env: serde_json::Map<String, Value> =
            req.env.into_iter().map(|(k, v)| (k, json!(v))).collect();
        let v = self
            .call(
                hub,
                json!({
                    "op": "create",
                    "image_id": image,
                    "vm": true,
                    "cpu": 0.25,
                    "cpu_limit": req.size.cpu as f64 / 2.0,
                    "memory_mib": 1024,
                    "memory_limit_mib": req.size.memory_gb * 1024,
                    "timeout_s": 24 * 3600,
                    "env": env,
                    "command": boot_command(),
                    "tags": { "corra-hub": super::hub_id(hub), "corra-machine": req.machine.to_string() },
                }),
            )
            .await?;
        Ok(v["sandbox_id"]
            .as_str()
            .ok_or_else(|| anyhow!("no sandbox id"))?
            .to_string())
    }
}

#[async_trait]
impl Provider for Modal {
    fn name(&self) -> &'static str {
        "modal"
    }

    fn describe(&self) -> Description {
        Description {
            label: "Modal",
            about: "Cloud machines billed on the CPU and memory they use.",
            credentials: &[
                Credential {
                    name: "modal.token_id",
                    label: "Token ID",
                    hint: "Modal, Settings, API Tokens.",
                },
                Credential {
                    name: "modal.token_secret",
                    label: "Token secret",
                    hint: "The secret half of the same token.",
                },
            ],
            kinds: vec![
                Kind {
                    name: "vm",
                    label: "VM sandbox",
                    about: "Starts small and bursts up to the project's size, billed on what it uses.",
                    sizes: &[],
                    sleeps: "Its disk is snapshotted and the machine shut down.",
                    max_hours: Some(24),
                    kvm: false,
                }
                .into(),
            ],
            listed: true,
        }
    }

    fn creates_machines(&self) -> bool {
        true
    }

    fn max_lifetime(&self) -> Option<Duration> {
        Some(Duration::from_secs(24 * 3600))
    }

    async fn base_image(&self, hub: &Hub) -> Result<String> {
        let key = format!(
            "modal.base_image:{}",
            crate::hub::hash_token(BASE_DOCKERFILE)
        );
        if let Some(id) = hub.db.kv_get(&key)? {
            return Ok(id);
        }
        let v = self
            .call(hub, json!({ "op": "build_image", "registry": base_registry(), "commands": dockerfile_commands() }))
            .await?;
        let id = v["image_id"]
            .as_str()
            .ok_or_else(|| anyhow!("no image id"))?
            .to_string();
        hub.db.kv_set(&key, &id)?;
        Ok(id)
    }

    async fn create(&self, hub: &Hub, req: CreateRequest) -> Result<String> {
        let image = self.base_image(hub).await?;
        self.start(hub, &image, req).await
    }

    fn sleeps(&self) -> bool {
        true
    }

    async fn sleep(&self, hub: &Hub, provider_ref: &str) -> Result<String> {
        let v = self
            .call(hub, json!({ "op": "snapshot", "sandbox_id": provider_ref }))
            .await?;
        let image = v["image_id"]
            .as_str()
            .ok_or_else(|| anyhow!("no image id"))?
            .to_string();
        self.destroy(hub, provider_ref).await?;
        Ok(image)
    }

    async fn wake(&self, hub: &Hub, sleep_ref: &str, req: CreateRequest) -> Result<String> {
        self.start(hub, sleep_ref, req).await
    }

    async fn forget(&self, hub: &Hub, sleep_ref: &str) -> Result<()> {
        self.call(hub, json!({ "op": "delete_image", "image_id": sleep_ref }))
            .await?;
        Ok(())
    }

    async fn running(&self, hub: &Hub) -> Result<Vec<String>> {
        let v = self
            .call(
                hub,
                json!({ "op": "list", "tags": { "corra-hub": super::hub_id(hub) } }),
            )
            .await?;
        Ok(v["sandbox_ids"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn destroy(&self, hub: &Hub, provider_ref: &str) -> Result<()> {
        self.call(
            hub,
            json!({ "op": "terminate", "sandbox_id": provider_ref }),
        )
        .await?;
        Ok(())
    }

    async fn status(&self, hub: &Hub, provider_ref: &str) -> Result<ProviderStatus> {
        let v = self
            .call(hub, json!({ "op": "status", "sandbox_id": provider_ref }))
            .await?;
        Ok(if v["status"] == "running" {
            ProviderStatus::Running
        } else {
            ProviderStatus::Gone
        })
    }
}
