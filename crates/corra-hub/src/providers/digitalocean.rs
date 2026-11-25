use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use serde_json::{Value, json};

use super::{
    BASE_DOCKERFILE, CreateRequest, Credential, Description, Kind, Provider, ProviderStatus,
};
use crate::hub::Hub;

const API: &str = "https://api.digitalocean.com/v2";
const DEFAULT_REGION: &str = "sfo3";
const SIZES: &[(u32, u32, &str)] = &[
    (2, 4, "s-2vcpu-4gb-amd"),
    (4, 8, "s-4vcpu-8gb-amd"),
    (4, 16, "s-4vcpu-16gb-amd"),
    (8, 16, "s-8vcpu-16gb-amd"),
];
const KIND_SIZES: &[(u32, u32)] = &[(2, 4), (4, 8), (4, 16), (8, 16)];

pub struct DigitalOcean {
    api: String,
    poll: Duration,
    building: tokio::sync::Mutex<()>,
}

impl Default for DigitalOcean {
    fn default() -> Self {
        Self::new()
    }
}

impl DigitalOcean {
    pub fn new() -> Self {
        Self::with_api(API, Duration::from_secs(10))
    }

    pub fn with_api(api: &str, poll: Duration) -> Self {
        DigitalOcean {
            api: api.trim_end_matches('/').to_string(),
            poll,
            building: tokio::sync::Mutex::new(()),
        }
    }

    async fn call(
        &self,
        hub: &Hub,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let token: String = hub
            .secrets
            .get("digitalocean.token")?
            .map(|k| k.split_whitespace().collect())
            .ok_or_else(|| anyhow!("add a DigitalOcean token in Settings first"))?;
        let mut rb = hub
            .http
            .request(method, format!("{}{path}", self.api))
            .bearer_auth(token);
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        let r = rb.send().await?;
        let status = r.status();
        let text = r.text().await?;
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if !status.is_success() {
            let msg = v["message"].as_str().unwrap_or(text.as_str()).to_string();
            bail!("DigitalOcean answered {status}: {msg}");
        }
        Ok(v)
    }

    async fn action(&self, hub: &Hub, droplet: &str, body: Value, what: &str) -> Result<()> {
        let a = self
            .call(
                hub,
                reqwest::Method::POST,
                &format!("/droplets/{droplet}/actions"),
                Some(body),
            )
            .await?;
        let id = a["action"]["id"]
            .as_u64()
            .ok_or_else(|| anyhow!("DigitalOcean didn't start the {what}"))?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60 * 60);
        loop {
            let a = self
                .call(hub, reqwest::Method::GET, &format!("/actions/{id}"), None)
                .await?;
            match a["action"]["status"].as_str() {
                Some("completed") => return Ok(()),
                Some("errored") => bail!("DigitalOcean couldn't finish the {what}"),
                _ => {}
            }
            if tokio::time::Instant::now() > deadline {
                bail!("the {what} took longer than an hour");
            }
            tokio::time::sleep(self.poll).await;
        }
    }

    async fn droplet_status(&self, hub: &Hub, id: &str) -> Result<Option<String>> {
        match self
            .call(hub, reqwest::Method::GET, &format!("/droplets/{id}"), None)
            .await
        {
            Ok(v) => Ok(v["droplet"]["status"].as_str().map(String::from)),
            Err(e) if e.to_string().contains("404") => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn wait_off(&self, hub: &Hub, id: &str, limit: Duration, what: &str) -> Result<()> {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            match self.droplet_status(hub, id).await?.as_deref() {
                Some("off") => return Ok(()),
                None => bail!("the droplet disappeared during the {what}"),
                _ => {}
            }
            if tokio::time::Instant::now() > deadline {
                bail!(
                    "the {what} took longer than {} minutes",
                    limit.as_secs() / 60
                );
            }
            tokio::time::sleep(self.poll).await;
        }
    }

    async fn snapshot_named(&self, hub: &Hub, name: &str) -> Result<Option<Value>> {
        let v = self
            .call(
                hub,
                reqwest::Method::GET,
                "/snapshots?resource_type=droplet&per_page=200",
                None,
            )
            .await?;
        Ok(v["snapshots"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|s| s["name"] == name)
            .cloned())
    }

    async fn create_droplet(
        &self,
        hub: &Hub,
        name: &str,
        size: &str,
        image: Value,
        user_data: &str,
        tags: &[String],
    ) -> Result<String> {
        let v = self
            .call(
                hub,
                reqwest::Method::POST,
                "/droplets",
                Some(json!({
                    "name": name,
                    "region": region(hub),
                    "size": size,
                    "image": image,
                    "user_data": user_data,
                    "tags": tags,
                })),
            )
            .await?;
        v["droplet"]["id"]
            .as_u64()
            .map(|id| id.to_string())
            .ok_or_else(|| anyhow!("DigitalOcean didn't return a droplet"))
    }

    async fn delete_droplet(&self, hub: &Hub, id: &str) -> Result<()> {
        match self
            .call(
                hub,
                reqwest::Method::DELETE,
                &format!("/droplets/{id}"),
                None,
            )
            .await
        {
            Err(e) if !e.to_string().contains("404") => Err(e),
            _ => Ok(()),
        }
    }

    async fn build_base(&self, hub: &Hub, name: &str) -> Result<String> {
        tracing::info!("making the DigitalOcean base snapshot {name}");
        let id = self
            .create_droplet(
                hub,
                &format!("{name}-build"),
                SIZES[0].2,
                json!(base_os()),
                &image_script(BASE_DOCKERFILE),
                &[build_tag(hub)],
            )
            .await?;
        let result = async {
            self.wait_off(hub, &id, Duration::from_secs(40 * 60), "base image build")
                .await?;
            self.action(
                hub,
                &id,
                json!({ "type": "snapshot", "name": name }),
                "base snapshot",
            )
            .await?;
            self.snapshot_named(hub, name)
                .await?
                .and_then(|s| s["id"].as_str().map(String::from))
                .ok_or_else(|| anyhow!("the base snapshot isn't listed after it was made"))
        }
        .await;
        let _ = self.delete_droplet(hub, &id).await;
        result
    }

    async fn start(&self, hub: &Hub, image: &str, req: CreateRequest) -> Result<String> {
        let size = slug(req.size.cpu, req.size.memory_gb);
        self.create_droplet(
            hub,
            &format!("{}-{}", req.name, super::hub_tag(hub)),
            size,
            image_value(image),
            &boot_script(&req.env),
            &[tag(hub)],
        )
        .await
    }
}

fn region(hub: &Hub) -> String {
    hub.setting_str("digitalocean.region")
        .unwrap_or_else(|| DEFAULT_REGION.into())
}

fn tag(hub: &Hub) -> String {
    format!("corra-{}", super::hub_tag(hub))
}

fn build_tag(hub: &Hub) -> String {
    format!("corra-build-{}", super::hub_tag(hub))
}

fn image_value(id: &str) -> Value {
    id.parse::<u64>().map(Value::from).unwrap_or(json!(id))
}

pub fn slug(cpu: u32, memory_gb: u32) -> &'static str {
    SIZES
        .iter()
        .find(|(c, m, _)| *c >= cpu && *m >= memory_gb)
        .unwrap_or(&SIZES[SIZES.len() - 1])
        .2
}

fn base_os() -> String {
    let from = BASE_DOCKERFILE
        .lines()
        .find_map(|l| l.trim().strip_prefix("FROM "))
        .unwrap_or("ubuntu:24.04");
    let version = from.trim().strip_prefix("ubuntu:").unwrap_or("24.04");
    format!("ubuntu-{}-x64", version.replace('.', "-"))
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn env_pairs(rest: &str) -> Vec<(String, String)> {
    let mut out = vec![];
    let mut chars = rest.trim().chars().peekable();
    while chars.peek().is_some() {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let key: String = chars.by_ref().take_while(|c| *c != '=').collect();
        if key.is_empty() {
            break;
        }
        let mut value = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                value.push(c);
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                value.push(c);
                chars.next();
            }
        }
        out.push((key, value));
    }
    out
}

pub fn image_script(dockerfile: &str) -> String {
    let mut env: Vec<(String, String)> = vec![];
    let mut steps: Vec<String> = vec![];
    let mut logical = String::new();
    for line in dockerfile.lines() {
        let line = line.trim_end();
        if let Some(cont) = line.strip_suffix('\\') {
            logical.push_str(cont);
            logical.push(' ');
            continue;
        }
        logical.push_str(line);
        let l = std::mem::take(&mut logical);
        let l = l.trim();
        if let Some(rest) = l.strip_prefix("ENV ") {
            for (k, v) in env_pairs(rest) {
                steps.push(format!("export {k}={}", shell_quote(&v)));
                env.push((k, v));
            }
        } else if let Some(rest) = l.strip_prefix("RUN ") {
            steps.push(rest.to_string());
        } else if let Some(rest) = l.strip_prefix("WORKDIR ") {
            steps.push(format!("mkdir -p {0} && cd {0}", rest.trim()));
        }
    }
    let profile: String = env
        .iter()
        .map(|(k, v)| format!("export {k}={}\n", shell_quote(v)))
        .collect();
    format!(
        "#!/bin/bash\nset -euxo pipefail\nexport HOME=/root\ncd /root\necho 'DPkg::Lock::Timeout \"900\";' > /etc/apt/apt.conf.d/90corra-lock\ncat > /etc/profile.d/corra.sh <<'CORRA_ENV'\n{profile}CORRA_ENV\n{}\ncloud-init clean --logs || true\n/usr/sbin/poweroff\n",
        steps.join("\n")
    )
}

pub fn boot_script(env: &[(String, String)]) -> String {
    let exports: String = env
        .iter()
        .map(|(k, v)| format!("export {k}={}\n", shell_quote(v)))
        .collect();
    format!(
        "#!/bin/bash\nexport HOME=/root\n. /etc/profile.d/corra.sh\n{exports}curl -fsSL --retry 8 --retry-all-errors -H \"Authorization: Bearer $CORRA_TOKEN\" \"$CORRA_HUB/dist/runner\" -o /usr/local/bin/corra-runner\nchmod +x /usr/local/bin/corra-runner\ncd /root\nnohup /usr/local/bin/corra-runner > /var/log/corra-runner.log 2>&1 &\n"
    )
}

#[async_trait]
impl Provider for DigitalOcean {
    fn name(&self) -> &'static str {
        "digitalocean"
    }

    fn describe(&self) -> Description {
        Description {
            label: "DigitalOcean",
            about: "Droplets, billed by the hour while they exist.",
            credentials: &[Credential {
                name: "digitalocean.token",
                label: "Token",
                hint: "DigitalOcean, API, Tokens: a token that can make and delete droplets and snapshots.",
            }],
            kinds: vec![
                Kind {
                    name: "droplet",
                    label: "Droplet",
                    about: "A virtual machine with KVM, so it can run the Android phone.",
                    sizes: KIND_SIZES,
                    sleeps: "It's shut down, snapshotted and deleted, so only the snapshot is billed.",
                    max_hours: None,
                    kvm: true,
                }
                .into(),
            ],
            listed: true,
        }
    }

    fn creates_machines(&self) -> bool {
        true
    }

    async fn base_image(&self, hub: &Hub) -> Result<String> {
        let hash = crate::hub::hash_token(BASE_DOCKERFILE);
        let key = format!("digitalocean.base_image:{hash}");
        if let Some(id) = hub.db.kv_get(&key)? {
            return Ok(id);
        }
        let _guard = self.building.lock().await;
        if let Some(id) = hub.db.kv_get(&key)? {
            return Ok(id);
        }
        let name = format!("corra-base-{}-{}", &hash[..12], super::hub_tag(hub));
        let id = match self
            .snapshot_named(hub, &name)
            .await?
            .and_then(|s| s["id"].as_str().map(String::from))
        {
            Some(id) => id,
            None => self.build_base(hub, &name).await?,
        };
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
        if self.droplet_status(hub, provider_ref).await?.as_deref() != Some("off") {
            self.action(hub, provider_ref, json!({ "type": "shutdown" }), "shutdown")
                .await
                .ok();
            if self
                .wait_off(hub, provider_ref, Duration::from_secs(120), "shutdown")
                .await
                .is_err()
            {
                self.action(
                    hub,
                    provider_ref,
                    json!({ "type": "power_off" }),
                    "power off",
                )
                .await?;
            }
        }
        let name = format!("corra-sleep-{provider_ref}-{}", corra_proto::now_ms());
        self.action(
            hub,
            provider_ref,
            json!({ "type": "snapshot", "name": name }),
            "snapshot",
        )
        .await?;
        let snap = self
            .snapshot_named(hub, &name)
            .await?
            .and_then(|s| s["id"].as_str().map(String::from))
            .ok_or_else(|| anyhow!("the sleep snapshot isn't listed after it was made"))?;
        self.delete_droplet(hub, provider_ref).await?;
        Ok(snap)
    }

    async fn wake(&self, hub: &Hub, sleep_ref: &str, req: CreateRequest) -> Result<String> {
        let snap = self
            .call(
                hub,
                reqwest::Method::GET,
                &format!("/snapshots/{sleep_ref}"),
                None,
            )
            .await?;
        let need = snap["snapshot"]["min_disk_size"].as_u64().unwrap_or(0);
        let size = slug(req.size.cpu, req.size.memory_gb);
        let disk = SIZE_DISKS
            .iter()
            .find(|(s, _)| *s == size)
            .map(|(_, d)| *d)
            .unwrap_or(0);
        if need > disk {
            bail!(
                "this machine's disk is {need} GB, more than the {disk} GB a {}-core, {} GB droplet has; pick a larger size, or reset the machine",
                req.size.cpu,
                req.size.memory_gb
            );
        }
        self.start(hub, sleep_ref, req).await
    }

    async fn forget(&self, hub: &Hub, sleep_ref: &str) -> Result<()> {
        match self
            .call(
                hub,
                reqwest::Method::DELETE,
                &format!("/snapshots/{sleep_ref}"),
                None,
            )
            .await
        {
            Err(e) if !e.to_string().contains("404") => Err(e),
            _ => Ok(()),
        }
    }

    async fn running(&self, hub: &Hub) -> Result<Vec<String>> {
        let mut ids = vec![];
        for t in [tag(hub), build_tag(hub)] {
            let v = self
                .call(
                    hub,
                    reqwest::Method::GET,
                    &format!("/droplets?tag_name={t}&per_page=200"),
                    None,
                )
                .await?;
            ids.extend(
                v["droplets"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|d| d["id"].as_u64().map(|i| i.to_string())),
            );
        }
        Ok(ids)
    }

    async fn destroy(&self, hub: &Hub, provider_ref: &str) -> Result<()> {
        self.delete_droplet(hub, provider_ref).await
    }

    async fn status(&self, hub: &Hub, provider_ref: &str) -> Result<ProviderStatus> {
        Ok(match self.droplet_status(hub, provider_ref).await? {
            None => ProviderStatus::Gone,
            Some(s) if s == "off" || s == "archive" => ProviderStatus::Gone,
            Some(_) => ProviderStatus::Running,
        })
    }
}

impl DigitalOcean {
    pub async fn retire_old_phone(&self, hub: &Hub, droplets: Vec<String>) -> bool {
        use crate::leftovers::clean;
        use corra_proto::LeftoverThing;
        let mut ids = droplets;
        let tag = super::hub_tag(hub);
        for t in [
            format!("corra-phone-{tag}"),
            format!("corra-phone-{tag}-build"),
        ] {
            let Ok(v) = self
                .call(
                    hub,
                    reqwest::Method::GET,
                    &format!("/droplets?tag_name={t}&per_page=200"),
                    None,
                )
                .await
            else {
                return false;
            };
            ids.extend(
                v["droplets"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|d| d["id"].as_u64().map(|i| i.to_string())),
            );
        }
        let name = std::env::var("CORRA_PHONE_IMAGE")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "corra-phone-image".into());
        let Ok(image) = self.snapshot_named(hub, &name).await else {
            return false;
        };
        if let Some(id) = image.as_ref().and_then(|s| s["id"].as_str()) {
            clean(
                hub,
                "the old phone's image",
                "digitalocean",
                LeftoverThing::Image,
                id,
            )
            .await;
        }
        ids.sort();
        ids.dedup();
        for id in ids {
            clean(
                hub,
                "the old phone's droplet",
                "digitalocean",
                LeftoverThing::Machine,
                &id,
            )
            .await;
        }
        true
    }
}

const SIZE_DISKS: &[(&str, u64)] = &[
    ("s-2vcpu-4gb-amd", 80),
    ("s-4vcpu-8gb-amd", 160),
    ("s-4vcpu-16gb-amd", 200),
    ("s-8vcpu-16gb-amd", 320),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_base_dockerfile_becomes_a_script_that_keeps_its_env_and_powers_off() {
        let s = image_script(BASE_DOCKERFILE);
        assert!(s.starts_with("#!/bin/bash\nset -euxo pipefail"));
        assert!(s.contains("export PLAYWRIGHT_BROWSERS_PATH='/opt/ms-playwright'"));
        assert!(s.contains("export DEBIAN_FRONTEND='noninteractive'"));
        assert!(s.contains("apt-get update && apt-get install -y"));
        assert!(s.contains("claude.ai/install.sh"));
        assert!(!s.contains("FROM "));
        assert!(s.trim_end().ends_with("poweroff"));
        let profile = s
            .split("<<'CORRA_ENV'\n")
            .nth(1)
            .unwrap()
            .split("CORRA_ENV")
            .next()
            .unwrap();
        assert!(profile.contains("export PATH='/root/.local/bin:"));
        assert!(profile.contains("export IS_SANDBOX='1'"));
        assert_eq!(base_os(), "ubuntu-24-04-x64");
    }

    #[test]
    fn quoted_env_values_and_continued_lines_survive() {
        let s = image_script(
            "FROM ubuntu:24.04\nENV A=\"x y\" B=it's\nRUN echo one \\\n  && echo two\n",
        );
        assert!(s.contains("export A='x y'"));
        assert!(s.contains("export B='it'\\''s'"));
        assert!(s.contains("echo one    && echo two"));
    }

    #[test]
    fn every_size_has_a_slug_and_a_disk() {
        for &(c, m) in KIND_SIZES {
            let s = slug(c, m);
            assert!(SIZES.iter().any(|x| x == &(c, m, s)), "{c}x{m} maps to {s}");
            assert!(SIZE_DISKS.iter().any(|(x, _)| *x == s));
        }
    }
}
