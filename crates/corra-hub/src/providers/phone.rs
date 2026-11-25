use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use serde_json::{Value, json};

use super::{CreateRequest, Provider, ProviderStatus};
use crate::hub::Hub;

const API: &str = "https://api.digitalocean.com/v2";
const IMAGE_SCRIPT: &str = include_str!("../../../../machines/phone-image.sh");
const SIZE: &str = "s-8vcpu-16gb-amd";
fn tag(hub: &Hub) -> String {
    format!("corra-phone-{}", super::hub_tag(hub))
}

pub struct Phone;

fn var(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.into())
}

fn image_name() -> String {
    var("CORRA_PHONE_IMAGE", "corra-phone-image")
}

fn region() -> String {
    var("CORRA_PHONE_REGION", "sfo3")
}

pub fn configured(hub: &Hub) -> bool {
    matches!(hub.secrets.get("digitalocean.token"), Ok(Some(_)))
}

async fn api(hub: &Hub, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value> {
    let token: String = hub
        .secrets
        .get("digitalocean.token")?
        .map(|k| k.split_whitespace().collect())
        .ok_or_else(|| {
            anyhow!("add digitalocean.token in Settings → Credentials to use a test phone")
        })?;
    let mut rb = hub
        .http
        .request(method, format!("{API}{path}"))
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

async fn snapshot_id(hub: &Hub, name: &str) -> Result<Option<String>> {
    let v = api(
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
        .and_then(|s| s["id"].as_str().map(String::from)))
}

async fn ssh_keys(hub: &Hub) -> Result<Vec<Value>> {
    let v = api(
        hub,
        reqwest::Method::GET,
        "/account/keys?per_page=200",
        None,
    )
    .await?;
    Ok(v["ssh_keys"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|k| k["id"].clone())
        .collect())
}

async fn create_droplet(
    hub: &Hub,
    name: &str,
    image: Value,
    user_data: &str,
    tag: &str,
) -> Result<String> {
    let v = api(
        hub,
        reqwest::Method::POST,
        "/droplets",
        Some(json!({
            "name": name,
            "region": region(),
            "size": SIZE,
            "image": image,
            "ssh_keys": ssh_keys(hub).await?,
            "user_data": user_data,
            "tags": [tag],
        })),
    )
    .await?;
    v["droplet"]["id"]
        .as_u64()
        .map(|id| id.to_string())
        .ok_or_else(|| anyhow!("DigitalOcean didn't return a droplet"))
}

async fn droplet_status(hub: &Hub, id: &str) -> Result<Option<String>> {
    match api(hub, reqwest::Method::GET, &format!("/droplets/{id}"), None).await {
        Ok(v) => Ok(v["droplet"]["status"].as_str().map(String::from)),
        Err(e) if e.to_string().contains("404") => Ok(None),
        Err(e) => Err(e),
    }
}

async fn delete_droplet(hub: &Hub, id: &str) -> Result<()> {
    match api(
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

async fn newest_ubuntu(hub: &Hub) -> Result<String> {
    let v = api(
        hub,
        reqwest::Method::GET,
        "/images?type=distribution&per_page=200",
        None,
    )
    .await?;
    let mut best: Option<(u32, String)> = None;
    for i in v["images"].as_array().into_iter().flatten() {
        let Some(slug) = i["slug"].as_str() else {
            continue;
        };
        let Some(year) = slug
            .strip_prefix("ubuntu-")
            .and_then(|s| s.strip_suffix("-04-x64"))
            .and_then(|y| y.parse::<u32>().ok())
        else {
            continue;
        };
        if year % 2 == 0 && best.as_ref().is_none_or(|(b, _)| year > *b) {
            best = Some((year, slug.to_string()));
        }
    }
    best.map(|(_, s)| s)
        .ok_or_else(|| anyhow!("DigitalOcean lists no Ubuntu image"))
}

async fn build_image(hub: &Hub, name: &str) -> Result<String> {
    tracing::info!("making the phone's image; this takes about fifteen minutes");
    let os = newest_ubuntu(hub).await?;
    let id = create_droplet(
        hub,
        &format!("{name}-build"),
        json!(os),
        IMAGE_SCRIPT,
        &format!("{}-build", tag(hub)),
    )
    .await?;
    let result = async {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(40 * 60);
        loop {
            tokio::time::sleep(Duration::from_secs(15)).await;
            match droplet_status(hub, &id).await?.as_deref() {
                Some("off") => break,
                None => bail!("the droplet building the phone's image disappeared"),
                _ => {}
            }
            if tokio::time::Instant::now() > deadline {
                bail!("the phone's image didn't finish building in 40 minutes");
            }
        }
        let action = api(
            hub,
            reqwest::Method::POST,
            &format!("/droplets/{id}/actions"),
            Some(json!({ "type": "snapshot", "name": name })),
        )
        .await?;
        let action_id = action["action"]["id"]
            .as_u64()
            .ok_or_else(|| anyhow!("DigitalOcean didn't start the snapshot"))?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30 * 60);
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            let a = api(
                hub,
                reqwest::Method::GET,
                &format!("/actions/{action_id}"),
                None,
            )
            .await?;
            match a["action"]["status"].as_str() {
                Some("completed") => break,
                Some("errored") => bail!("DigitalOcean couldn't snapshot the phone's image"),
                _ => {}
            }
            if tokio::time::Instant::now() > deadline {
                bail!("the snapshot of the phone's image didn't finish in 30 minutes");
            }
        }
        snapshot_id(hub, name)
            .await?
            .ok_or_else(|| anyhow!("the phone's image isn't listed after its snapshot"))
    }
    .await;
    let _ = delete_droplet(hub, &id).await;
    result
}

fn boot_script(env: &[(String, String)]) -> String {
    let exports: String = env
        .iter()
        .map(|(k, v)| format!("export {k}='{}'\n", v.replace('\'', "'\\''")))
        .collect();
    format!(
        "#!/bin/bash\nexport HOME=/root\n{exports}curl -fsSL --retry 8 --retry-all-errors -H \"Authorization: Bearer $CORRA_TOKEN\" \"$CORRA_HUB/dist/runner\" -o /usr/local/bin/corra-runner\nchmod +x /usr/local/bin/corra-runner\ncd /root\nnohup /usr/local/bin/corra-runner > /var/log/corra-runner.log 2>&1 &\n"
    )
}

#[async_trait]
impl Provider for Phone {
    fn name(&self) -> &'static str {
        "phone"
    }

    fn creates_machines(&self) -> bool {
        true
    }

    async fn create(&self, hub: &Hub, req: CreateRequest) -> Result<String> {
        let name = image_name();
        let image = match snapshot_id(hub, &name).await? {
            Some(id) => id,
            None => build_image(hub, &name).await?,
        };
        let image: Value = image
            .parse::<u64>()
            .map(Value::from)
            .unwrap_or(json!(image));
        create_droplet(hub, &req.name, image, &boot_script(&req.env), &tag(hub)).await
    }

    async fn destroy(&self, hub: &Hub, provider_ref: &str) -> Result<()> {
        delete_droplet(hub, provider_ref).await
    }

    async fn running(&self, hub: &Hub) -> Result<Vec<String>> {
        let mut ids = vec![];
        for tag in [tag(hub), format!("{}-build", tag(hub))] {
            let v = api(
                hub,
                reqwest::Method::GET,
                &format!("/droplets?tag_name={tag}&per_page=200"),
                None,
            )
            .await?;
            if let Some(a) = v["droplets"].as_array() {
                ids.extend(
                    a.iter()
                        .filter_map(|d| d["id"].as_u64().map(|i| i.to_string())),
                );
            }
        }
        Ok(ids)
    }

    async fn status(&self, hub: &Hub, provider_ref: &str) -> Result<ProviderStatus> {
        Ok(match droplet_status(hub, provider_ref).await? {
            None => ProviderStatus::Gone,
            Some(s) if s == "off" || s == "archive" => ProviderStatus::Gone,
            Some(_) => ProviderStatus::Running,
        })
    }
}
