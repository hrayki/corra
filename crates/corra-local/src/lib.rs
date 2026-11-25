mod connect;
mod desktop;
mod notices;
mod theme;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use corra_hub::db::NewMachine;
use corra_hub::hub::{HubRef, hash_token, new_token};
use corra_proto::{MachineId, MachineStatus};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{Notify, watch};

pub use desktop::Target;

pub struct Options {
    pub data: PathBuf,
    pub port: u16,
}

pub struct Desktop {
    pub target: watch::Receiver<Target>,
    pub theme: watch::Receiver<Value>,
    notices: tokio::sync::broadcast::Sender<Value>,
    state: StateRef,
}

impl Desktop {
    pub fn base(&self) -> &str {
        &self.state.base
    }

    pub fn notices(&self) -> tokio::sync::broadcast::Receiver<Value> {
        self.notices.subscribe()
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct HubCfg {
    pub url: String,
    pub token: String,
    pub machine: MachineId,
    pub machine_token: String,
}

pub(crate) struct State {
    pub hub: HubRef,
    pub base: String,
    pub enter_key: String,
    pub session: String,
    pub machine: MachineId,
    pub http: reqwest::Client,
    pub target: watch::Sender<Target>,
    pub runner: Mutex<Option<(HubCfg, tokio::task::JoinHandle<()>)>>,
    pub runner_up: Arc<std::sync::atomic::AtomicBool>,
    pub wake: Notify,
}

pub(crate) type StateRef = Arc<State>;

impl State {
    pub fn cfg(&self) -> Option<HubCfg> {
        serde_json::from_str(&self.hub.db.kv_get("hub").ok()??).ok()
    }

    pub fn set_cfg(&self, cfg: Option<&HubCfg>) {
        match cfg {
            Some(c) => {
                let _ = self
                    .hub
                    .db
                    .kv_set("hub", &serde_json::to_string(c).unwrap_or_default());
            }
            None => {
                let _ = self.hub.db.kv_delete("hub");
            }
        }
        self.wake.notify_waiters();
    }

    pub fn connect_url(&self) -> String {
        format!("{}/desktop/connect?k={}", self.base, self.enter_key)
    }

    pub fn install_id(&self) -> String {
        self.hub
            .db
            .kv_get("install_id")
            .ok()
            .flatten()
            .unwrap_or_default()
    }
}

pub fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "this computer".into())
}

pub fn default_data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CORRA_LOCAL_DATA").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    if let Some(d) = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(d).join("corra");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/corra")
    } else {
        home.join(".local/share/corra")
    }
}

async fn bind(port: u16) -> Result<tokio::net::TcpListener> {
    match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(l) => Ok(l),
        Err(_) => Ok(tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?),
    }
}

fn this_machine(hub: &HubRef, token: &str) -> Result<MachineId> {
    let known = hub
        .db
        .kv_get("builtin_machine")?
        .and_then(|s| s.parse::<MachineId>().ok())
        .filter(|id| matches!(hub.db.machine(*id), Ok(Some(_))));
    let id = match known {
        Some(id) => id,
        None => {
            let id = match hub
                .db
                .machines()?
                .into_iter()
                .find(|m| m.provider == "existing" && m.status != MachineStatus::Destroyed)
            {
                Some(m) => m.id,
                None => {
                    hub.db
                        .insert_machine(NewMachine {
                            name: hostname(),
                            provider: "existing".into(),
                            project: None,
                            size: "existing".into(),
                            token_hash: hash_token(token),
                            provider_ref: None,
                            status: MachineStatus::Offline,
                        })?
                        .id
                }
            };
            hub.db.kv_set("builtin_machine", &id.to_string())?;
            id
        }
    };
    hub.db.set_machine_token(id, &hash_token(token))?;
    hub.db.set_machine_name(id, &hostname())?;
    hub.db.set_machine_status(id, MachineStatus::Offline)?;
    hub.db.kv_set(&format!("machine:{id}:kit"), "off")?;
    Ok(id)
}

pub async fn start(opts: Options) -> Result<Desktop> {
    let listener = bind(opts.port).await?;
    let port = listener.local_addr()?.port();
    let base = format!("http://127.0.0.1:{port}");
    let built = corra_hub::build(corra_hub::Options {
        data: opts.data.clone(),
        public_url: base.clone(),
        dev_no_auth: false,
    })?;
    let hub = built.state.hub.clone();
    let runner_token = new_token("cm");
    let machine = this_machine(&hub, &runner_token)?;
    for t in hub.db.threads()? {
        if matches!(
            t.status,
            corra_proto::ThreadStatus::Running
                | corra_proto::ThreadStatus::Waiting
                | corra_proto::ThreadStatus::Starting
        ) {
            hub.set_thread_status(t.id, corra_proto::ThreadStatus::Idle)?;
        }
    }
    if let Some(old) = hub.db.kv_get("local_session")? {
        let _ = hub.db.kv_delete(&old);
    }
    let session = corra_hub::auth::create_device_session(&hub.db, "this app")?;
    hub.db.kv_set(
        "local_session",
        &format!("session:{}", hash_token(&session)),
    )?;
    if hub.db.kv_get("install_id")?.is_none() {
        hub.db.kv_set("install_id", &new_token("in"))?;
    }
    let enter_key = new_token("k");
    let (target_tx, target_rx) = watch::channel(Target::Starting);
    let state = Arc::new(State {
        hub: hub.clone(),
        base: base.clone(),
        enter_key,
        session,
        machine,
        http: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()?,
        target: target_tx,
        runner: Mutex::new(None),
        runner_up: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        wake: Notify::new(),
    });
    let router = corra_hub::finish(built.router.merge(connect::router(state.clone())));
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            tracing::error!("the built-in hub stopped: {e}");
        }
    });
    let runner_url = base.clone();
    tokio::spawn(async move {
        if let Err(e) = corra_runner::run(corra_runner::Options {
            hub: runner_url,
            token: runner_token,
            sync: false,
            update: false,
            connected: None,
        })
        .await
        {
            tracing::error!("the built-in hub's runner stopped: {e:#}");
        }
    });
    let theme = theme::watch();
    desktop::spawn(state.clone());
    let (notices, _) = tokio::sync::broadcast::channel(64);
    notices::spawn(state.clone(), notices.clone());
    Ok(Desktop {
        target: target_rx,
        theme,
        notices,
        state,
    })
}
