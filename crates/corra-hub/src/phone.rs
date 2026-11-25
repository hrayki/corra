use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use corra_proto::runner::{HubMsg, RunnerRequest};
use corra_proto::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::auth::AppState;
use crate::db::NewMachine;
use crate::hub::{Hub, HubRef, hash_token, new_token};
use crate::leftovers::clean;
use crate::providers::{CreateRequest, Size};

const MACHINE_KEY: &str = "phone.machine";
const TEMPLATE_KEY: &str = "phone.template";
pub const SETUP_MINUTES: i64 = 90;

#[derive(Default)]
pub struct PhoneState {
    starting: tokio::sync::Mutex<()>,
    blobs: std::sync::Mutex<HashSet<String>>,
    pub forwards: std::sync::Mutex<std::collections::HashMap<u16, MachineId>>,
    pub tunnels: std::sync::Mutex<std::collections::HashMap<String, (MachineId, MachineId)>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Template {
    host: PhoneHost,
    reference: String,
}

pub fn host(hub: &Hub) -> Option<PhoneHost> {
    match hub.settings().get("phone") {
        Some(v) if v.is_object() => serde_json::from_value(v.clone()).ok(),
        Some(_) => None,
        None => hub
            .secrets
            .get("digitalocean.token")
            .ok()
            .flatten()
            .map(|_| PhoneHost {
                provider: "digitalocean".into(),
                kind: "droplet".into(),
                size: "8x16".into(),
            }),
    }
}

pub fn check_host(hub: &Hub, h: &PhoneHost) -> Result<PhoneHost> {
    let p = hub
        .providers
        .info(hub)
        .into_iter()
        .find(|p| p.name == h.provider && p.creates_machines)
        .ok_or_else(|| anyhow!("{} can't make machines for the phone", h.provider))?;
    if !p.connected {
        bail!("connect {} in Settings first", p.label);
    }
    let k = p
        .kinds
        .iter()
        .find(|k| k.name == h.kind)
        .ok_or_else(|| anyhow!("{} has no kind called {}", p.label, h.kind))?;
    if !k.kvm {
        bail!(
            "a {} {} has no KVM, which the Android emulator needs",
            p.label,
            k.label
        );
    }
    Ok(PhoneHost {
        provider: h.provider.clone(),
        kind: h.kind.clone(),
        size: if k.sizes.is_empty() {
            h.size.clone()
        } else {
            crate::providers::fit(&k.sizes, &h.size)
        },
    })
}

fn template(hub: &Hub) -> Option<Template> {
    hub.db
        .kv_get(TEMPLATE_KEY)
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
}

pub fn machine(hub: &Hub) -> Option<Machine> {
    let id: MachineId = hub.db.kv_get(MACHINE_KEY).ok().flatten()?.parse().ok()?;
    hub.db.machine(id).ok().flatten().filter(|m| {
        !matches!(
            m.status,
            MachineStatus::Destroyed | MachineStatus::Destroying
        )
    })
}

pub fn busy(hub: &Hub) -> bool {
    hub.phone.starting.try_lock().is_err()
}

pub fn is_machine(hub: &Hub, id: MachineId) -> bool {
    hub.db
        .kv_get(MACHINE_KEY)
        .ok()
        .flatten()
        .is_some_and(|v| v == id.to_string())
}

fn ready(hub: &Hub) -> Option<MachineId> {
    machine(hub)
        .filter(|m| hub.runner_connected(m.id))
        .map(|m| m.id)
}

async fn connect(hub: &HubRef, m: &Machine, r: &str) -> Result<()> {
    let provider = hub
        .providers
        .get(&m.provider)
        .ok_or_else(|| anyhow!("unknown provider {}", m.provider))?;
    hub.db.set_machine_provider_ref(m.id, r)?;
    if let Err(e) = crate::providers::await_runner(hub, provider.as_ref(), m.id, r).await {
        hub.db.set_machine_status(m.id, MachineStatus::Failed)?;
        hub.publish_machine(m.id);
        bail!(e);
    }
    Ok(())
}

fn request(hub: &Hub, m: &Machine, h: &PhoneHost) -> Result<CreateRequest> {
    Ok(CreateRequest {
        name: m.name.clone(),
        machine: m.id,
        kind: h.kind.clone(),
        size: Size::parse(&h.size),
        env: hub.providers.machine_env(hub, m.id)?,
    })
}

pub async fn up(hub: &HubRef) -> Result<MachineId> {
    if let Some(m) = ready(hub) {
        boot(hub, m).await?;
        return Ok(m);
    }
    let _guard = hub.phone.starting.lock().await;
    if let Some(m) = ready(hub) {
        boot(hub, m).await?;
        return Ok(m);
    }
    let h = host(hub).ok_or_else(|| anyhow!("the phone is off: pick where it runs in Settings"))?;
    let h = check_host(hub, &h)?;
    let provider = hub
        .providers
        .get(&h.provider)
        .ok_or_else(|| anyhow!("unknown provider {}", h.provider))?;

    if let Some(m) = machine(hub) {
        let sleep_ref = hub.db.machine_sleep_ref(m.id)?;
        if m.status == MachineStatus::Asleep
            && let Some(s) = sleep_ref
        {
            hub.db.set_machine_status(m.id, MachineStatus::Creating)?;
            hub.publish_machine(m.id);
            let r = provider.wake(hub, &s, request(hub, &m, &h)?).await?;
            connect(hub, &m, &r).await?;
            boot(hub, m.id).await?;
            return Ok(m.id);
        }
        if let Some(r) = hub.db.machine_provider_ref(m.id)? {
            clean(
                hub,
                "the phone's machine",
                &m.provider,
                LeftoverThing::Machine,
                &r,
            )
            .await;
        }
        hub.db.set_machine_status(m.id, MachineStatus::Destroyed)?;
        hub.publish_machine(m.id);
        let _ = hub.db.kv_delete(MACHINE_KEY);
    }

    let tpl = template(hub).filter(|t| t.host == h);
    let m = hub.db.insert_machine(NewMachine {
        name: format!("corra-phone-{}", &new_id().simple().to_string()[26..]),
        provider: h.provider.clone(),
        project: None,
        size: h.size.clone(),
        token_hash: hash_token(&new_token("cm")),
        provider_ref: None,
        status: MachineStatus::Creating,
        kind: Some(h.kind.clone()),
    })?;
    hub.db.kv_set(MACHINE_KEY, &m.id.to_string())?;
    hub.db
        .kv_set(&crate::providers::starting_key(m.id), &now_ms().to_string())?;
    hub.publish_machine(m.id);
    let fail = |e: anyhow::Error| {
        let _ = hub.db.set_machine_status(m.id, MachineStatus::Failed);
        hub.publish_machine(m.id);
        e
    };

    if let Some(t) = tpl {
        let r = provider
            .wake(hub, &t.reference, request(hub, &m, &h)?)
            .await
            .map_err(fail)?;
        connect(hub, &m, &r).await?;
        boot(hub, m.id).await?;
        return Ok(m.id);
    }

    let r = provider
        .create(hub, request(hub, &m, &h)?)
        .await
        .map_err(fail)?;
    connect(hub, &m, &r).await?;
    hub.runner_request_timeout(
        m.id,
        RunnerRequest::Phone {
            action: PhoneOp::Setup,
        },
        Duration::from_secs(50 * 60),
    )
    .await
    .map_err(fail)?;
    if !provider.sleep_keeps_machine() && provider.sleeps() {
        hub.db
            .kv_set(&crate::providers::starting_key(m.id), &now_ms().to_string())?;
        hub.db.set_machine_status(m.id, MachineStatus::Creating)?;
        hub.publish_machine(m.id);
        let snap = provider.sleep(hub, &r).await.map_err(fail)?;
        hub.db.kv_set(
            TEMPLATE_KEY,
            &serde_json::to_string(&Template {
                host: h.clone(),
                reference: snap.clone(),
            })?,
        )?;
        let r = provider
            .wake(hub, &snap, request(hub, &m, &h)?)
            .await
            .map_err(fail)?;
        connect(hub, &m, &r).await?;
    }
    boot(hub, m.id).await?;
    Ok(m.id)
}

async fn boot(hub: &Hub, m: MachineId) -> Result<()> {
    hub.touch_machine(m);
    hub.runner_request_timeout(
        m,
        RunnerRequest::Phone {
            action: PhoneOp::Boot,
        },
        Duration::from_secs(330),
    )
    .await
    .map(|_| ())
}

pub async fn down(hub: &HubRef) -> Result<()> {
    hub.phone.forwards.lock().unwrap().clear();
    hub.phone.tunnels.lock().unwrap().clear();
    let Some(m) = machine(hub) else {
        return Ok(());
    };
    if m.status == MachineStatus::Asleep {
        return Ok(());
    }
    let provider = hub.providers.get(&m.provider);
    let r = hub.db.machine_provider_ref(m.id)?;
    if let (Some(p), Some(r)) = (&provider, &r)
        && p.sleep_keeps_machine()
        && hub.runner_connected(m.id)
    {
        let _ = hub.send_runner_pub(m.id, HubMsg::PhoneStop);
        let s = p.sleep(hub, r).await?;
        hub.db.set_machine_sleep_ref(m.id, Some(&s))?;
        hub.db.set_machine_status(m.id, MachineStatus::Asleep)?;
        hub.publish_machine(m.id);
        return Ok(());
    }
    if let Some(r) = &r {
        clean(
            hub,
            "the phone's machine",
            &m.provider,
            LeftoverThing::Machine,
            r,
        )
        .await;
    }
    hub.db.set_machine_status(m.id, MachineStatus::Destroyed)?;
    hub.publish_machine(m.id);
    let _ = hub.db.kv_delete(MACHINE_KEY);
    Ok(())
}

pub async fn switch(hub: &HubRef, new: Option<PhoneHost>) -> Result<Option<PhoneHost>> {
    let new = match new {
        Some(h) => Some(check_host(hub, &h)?),
        None => None,
    };
    let _guard = hub.phone.starting.lock().await;
    if host(hub) != new {
        hub.phone.forwards.lock().unwrap().clear();
        hub.phone.tunnels.lock().unwrap().clear();
        if let Some(m) = machine(hub) {
            if let Some(r) = hub.db.machine_provider_ref(m.id)? {
                clean(
                    hub,
                    "the phone's machine",
                    &m.provider,
                    LeftoverThing::Machine,
                    &r,
                )
                .await;
            }
            if let Some(s) = hub.db.machine_sleep_ref(m.id)?
                && Some(&s) != hub.db.machine_provider_ref(m.id)?.as_ref()
            {
                clean(
                    hub,
                    "the phone's saved machine",
                    &m.provider,
                    LeftoverThing::Image,
                    &s,
                )
                .await;
            }
            hub.db.set_machine_status(m.id, MachineStatus::Destroyed)?;
            hub.publish_machine(m.id);
        }
        if let Some(t) = template(hub) {
            clean(
                hub,
                "the phone's template",
                &t.host.provider,
                LeftoverThing::Image,
                &t.reference,
            )
            .await;
        }
        let _ = hub.db.kv_delete(MACHINE_KEY);
        let _ = hub.db.kv_delete(TEMPLATE_KEY);
    }
    hub.set_setting(
        "phone",
        match &new {
            Some(h) => serde_json::to_value(h)?,
            None => json!("off"),
        },
    )?;
    Ok(new)
}

pub async fn act(hub: &Hub, action: PhoneOp) -> Result<Value> {
    let m = ready(hub).ok_or_else(|| anyhow!("no phone is running: start it first"))?;
    hub.touch_machine(m);
    let secs = match action {
        PhoneOp::Boot | PhoneOp::Stream | PhoneOp::Install { .. } => 330,
        _ => 90,
    };
    hub.runner_request_timeout(
        m,
        RunnerRequest::Phone { action },
        Duration::from_secs(secs),
    )
    .await
}

fn tool(action: &str, description: &str, props: Value, required: &[&str]) -> ToolDef {
    ToolDef {
        action: action.into(),
        description: description.into(),
        input_schema: json!({ "type": "object", "properties": props, "required": required }),
    }
}

pub fn defs(hub: &Hub) -> Vec<ToolDef> {
    if host(hub).is_none() {
        return vec![];
    }
    let x = json!({ "type": "integer", "description": "In screenshot pixels." });
    vec![
        tool(
            "phone.start",
            "Start the Android test phone, an emulator in the cloud with Google Play, and wait until it's ready. Every thread shares the one phone. It takes about two minutes, and is destroyed once nobody has used it for a while.",
            json!({}),
            &[],
        ),
        tool(
            "phone.stop",
            "Destroy the test phone. Its apps and data go with it.",
            json!({}),
            &[],
        ),
        tool(
            "phone.screenshot",
            "Take a screenshot of the phone. Points in the other phone tools are in this image's pixels.",
            json!({}),
            &[],
        ),
        tool(
            "phone.ui",
            "List what's on the phone's screen: each element's centre, kind and text, with * for the tappable ones. Compose and Flutter screens often list little; take a screenshot then.",
            json!({}),
            &[],
        ),
        tool(
            "phone.tap",
            "Tap the phone's screen: a point, or the element whose text is, or else contains, the text given.",
            json!({
                "x": x, "y": x,
                "text": { "type": "string", "description": "Tap the element with this text instead of a point." },
                "index": { "type": "integer", "description": "Which match to tap when several have the text, from 0." }
            }),
            &[],
        ),
        tool(
            "phone.swipe",
            "Swipe on the phone from one point to another. Swiping up scrolls down.",
            json!({
                "x1": x, "y1": x, "x2": x, "y2": x,
                "ms": { "type": "integer", "description": "How long the swipe takes. Defaults to 300." }
            }),
            &["x1", "y1", "x2", "y2"],
        ),
        tool(
            "phone.type",
            "Type text into the field that has focus on the phone.",
            json!({ "text": { "type": "string" } }),
            &["text"],
        ),
        tool(
            "phone.key",
            "Press a key on the phone.",
            json!({ "key": { "type": "string", "enum": ["back", "home", "recents", "enter", "backspace", "tab", "escape", "up", "down", "left", "right", "menu", "power"] } }),
            &["key"],
        ),
        tool(
            "phone.install",
            "Install an Android app on the phone from APK files on this machine.",
            json!({ "paths": { "type": "array", "items": { "type": "string" }, "description": "The APK's path; several paths for a split app." } }),
            &["paths"],
        ),
        tool(
            "phone.forward",
            "Make the phone's localhost:PORT reach this machine's localhost:PORT, so an app on the phone can use a dev server running here. It lasts until the phone is stopped; forwarding the port again points it at the machine that asked last.",
            json!({ "port": { "type": "integer", "description": "The port, the same on both sides." } }),
            &["port"],
        ),
        tool(
            "phone.shell",
            "Run a shell command on the phone, as adb shell would: am start, pm list packages, logcat -d -t 200. Its input commands take the phone's own pixels, which are not screenshot pixels.",
            json!({ "command": { "type": "string" } }),
            &["command"],
        ),
    ]
}

pub fn is_phone(action: &str) -> bool {
    action.starts_with("phone.")
}

fn int(input: &Value, key: &str) -> Result<u32> {
    input[key]
        .as_u64()
        .map(|v| v as u32)
        .ok_or_else(|| anyhow!("{key} is required"))
}

pub async fn run(hub: &HubRef, caller: MachineId, action: &str, input: Value) -> Result<Value> {
    match action {
        "phone.start" => {
            let starting = hub.clone();
            let started = tokio::spawn(async move { up(&starting).await });
            match tokio::time::timeout(Duration::from_secs(240), started).await {
                Ok(r) => {
                    r.map_err(|_| anyhow!("starting the phone stopped"))??;
                    Ok(json!("The phone is ready."))
                }
                Err(_) => Ok(json!(
                    "The phone is still starting. The first start makes its image and takes about fifteen minutes. Call phone.start again to wait for it."
                )),
            }
        }
        "phone.stop" => {
            down(hub).await?;
            Ok(json!("The phone is destroyed."))
        }
        "phone.screenshot" => act(hub, PhoneOp::Shot).await,
        "phone.ui" => act(hub, PhoneOp::Ui).await,
        "phone.tap" => {
            let op = match input["text"].as_str() {
                Some(text) => PhoneOp::TapText {
                    text: text.to_string(),
                    index: input["index"].as_u64().unwrap_or(0) as u32,
                },
                None => PhoneOp::Tap {
                    x: int(&input, "x")?,
                    y: int(&input, "y")?,
                },
            };
            let v = act(hub, op).await?;
            Ok(if v.is_null() { json!("Tapped.") } else { v })
        }
        "phone.swipe" => {
            act(
                hub,
                PhoneOp::Swipe {
                    x1: int(&input, "x1")?,
                    y1: int(&input, "y1")?,
                    x2: int(&input, "x2")?,
                    y2: int(&input, "y2")?,
                    ms: input["ms"].as_u64().map(|v| v as u32),
                },
            )
            .await?;
            Ok(json!("Swiped."))
        }
        "phone.type" => {
            let text = input["text"]
                .as_str()
                .ok_or_else(|| anyhow!("text is required"))?;
            act(
                hub,
                PhoneOp::Text {
                    text: text.to_string(),
                },
            )
            .await?;
            Ok(json!("Typed."))
        }
        "phone.key" => {
            let key = input["key"]
                .as_str()
                .ok_or_else(|| anyhow!("key is required"))?;
            act(
                hub,
                PhoneOp::Key {
                    key: key.to_string(),
                },
            )
            .await?;
            Ok(json!("Pressed."))
        }
        "phone.forward" => {
            let port = input["port"]
                .as_u64()
                .filter(|p| (1024..=65535).contains(p) && *p != 5037)
                .ok_or_else(|| {
                    anyhow!("port must be a number from 1024 to 65535, other than 5037")
                })? as u16;
            act(hub, PhoneOp::Forward { port }).await?;
            hub.phone.forwards.lock().unwrap().insert(port, caller);
            Ok(json!(format!(
                "The phone's localhost:{port} now reaches this machine's localhost:{port}."
            )))
        }
        "phone.shell" => {
            let command = input["command"]
                .as_str()
                .ok_or_else(|| anyhow!("command is required"))?;
            act(
                hub,
                PhoneOp::Shell {
                    command: command.to_string(),
                },
            )
            .await
        }
        "phone.install" => {
            let paths: Vec<String> = input["paths"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p.as_str().map(String::from))
                .collect();
            if paths.is_empty() {
                bail!("paths is required");
            }
            ready(hub).ok_or_else(|| anyhow!("no phone is running: start it first"))?;
            let mut ids = vec![];
            let result = async {
                let mut urls = vec![];
                for path in paths {
                    let id = new_token("bl");
                    hub.phone.blobs.lock().unwrap().insert(id.clone());
                    let url = format!("{}/blob/{id}", hub.providers.hub_url);
                    ids.push(id);
                    hub.runner_request_timeout(
                        caller,
                        RunnerRequest::Upload {
                            path,
                            url: url.clone(),
                        },
                        Duration::from_secs(300),
                    )
                    .await?;
                    urls.push(url);
                }
                act(hub, PhoneOp::Install { urls }).await
            }
            .await;
            for id in ids {
                hub.phone.blobs.lock().unwrap().remove(&id);
                let _ = tokio::fs::remove_file(blob_path(hub, &id)).await;
            }
            result
        }
        _ => bail!("unknown tool {action}"),
    }
}

fn blob_path(hub: &Hub, id: &str) -> PathBuf {
    hub.data.join("blobs").join(id)
}

fn blob_known(hub: &Hub, id: &str) -> bool {
    hub.phone.blobs.lock().unwrap().contains(id)
}

const BLOB_MAX: u64 = 1 << 30;

pub async fn blob_put(
    State(s): State<AppState>,
    UrlPath(id): UrlPath<String>,
    body: axum::body::Body,
) -> Response {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;
    if !blob_known(&s.hub, &id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let path = blob_path(&s.hub, &id);
    if let Some(dir) = path.parent() {
        let _ = tokio::fs::create_dir_all(dir).await;
    }
    let Ok(mut file) = tokio::fs::File::create(&path).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let mut stream = body.into_data_stream();
    let mut size = 0u64;
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        size += chunk.len() as u64;
        if size > BLOB_MAX {
            return StatusCode::PAYLOAD_TOO_LARGE.into_response();
        }
        if file.write_all(&chunk).await.is_err() {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    match file.flush().await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn blob_get(State(s): State<AppState>, UrlPath(id): UrlPath<String>) -> Response {
    if !blob_known(&s.hub, &id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    match tokio::fs::File::open(blob_path(&s.hub, &id)).await {
        Ok(f) => (
            [("content-type", "application/octet-stream")],
            axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(f)),
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}
