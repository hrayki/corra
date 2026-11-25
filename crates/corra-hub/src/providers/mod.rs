mod agent37;
mod existing;
mod modal;
pub mod phone;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use corra_proto::*;

use crate::db::NewMachine;
use crate::hub::{Hub, hash_token, new_token};

pub const BASE_DOCKERFILE: &str = include_str!("../../../../machines/base.Dockerfile");

pub struct CreateRequest {
    pub name: String,
    pub machine: MachineId,
    pub size: Size,
    pub env: Vec<(String, String)>,
}

pub fn hub_id(hub: &Hub) -> String {
    if let Ok(Some(id)) = hub.db.kv_get("hub_id") {
        return id;
    }
    let id = new_id().simple().to_string();
    let _ = hub.db.kv_set("hub_id", &id);
    id
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    pub cpu: u32,
    pub memory_gb: u32,
}

impl Size {
    pub fn parse(s: &str) -> Size {
        let d = Size {
            cpu: 4,
            memory_gb: 8,
        };
        let Some((c, m)) = s.split_once('x') else {
            return d;
        };
        match (
            c.trim().parse(),
            m.trim()
                .trim_end_matches("gb")
                .trim_end_matches("GB")
                .parse(),
        ) {
            (Ok(cpu), Ok(memory_gb)) if cpu > 0 && memory_gb > 0 => Size { cpu, memory_gb },
            _ => d,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderStatus {
    Running,
    Stopped,
    Gone,
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &'static str;
    fn creates_machines(&self) -> bool;
    fn max_lifetime(&self) -> Option<Duration> {
        None
    }
    async fn base_image(&self, _hub: &Hub) -> Result<String> {
        bail!("{} has no images", self.name())
    }
    async fn create(&self, _hub: &Hub, _req: CreateRequest) -> Result<String> {
        bail!("{} can't create machines", self.name())
    }
    fn sleeps(&self) -> bool {
        false
    }
    async fn sleep(&self, _hub: &Hub, _provider_ref: &str) -> Result<String> {
        bail!("{} can't put machines to sleep", self.name())
    }
    async fn wake(&self, _hub: &Hub, _sleep_ref: &str, _req: CreateRequest) -> Result<String> {
        bail!("{} can't wake machines", self.name())
    }
    async fn forget(&self, _hub: &Hub, _sleep_ref: &str) -> Result<()> {
        Ok(())
    }
    async fn running(&self, _hub: &Hub) -> Result<Vec<String>> {
        Ok(vec![])
    }
    async fn destroy(&self, _hub: &Hub, _provider_ref: &str) -> Result<()> {
        Ok(())
    }
    async fn status(&self, _hub: &Hub, _provider_ref: &str) -> Result<ProviderStatus> {
        Ok(ProviderStatus::Running)
    }
}

pub struct Providers {
    by_name: RwLock<HashMap<&'static str, Arc<dyn Provider>>>,
    starting: Arc<Mutex<HashSet<MachineId>>>,
    pub hub_url: String,
}

struct Starting(Arc<Mutex<HashSet<MachineId>>>, MachineId);

impl Drop for Starting {
    fn drop(&mut self) {
        self.0.lock().unwrap().remove(&self.1);
    }
}

pub fn starting_key(m: MachineId) -> String {
    format!("starting_since:{m}")
}

const RUNNER_WAIT: Duration = Duration::from_secs(300);
const STATUS_EVERY: Duration = Duration::from_secs(5);
const STOPPED_GRACE: Duration = Duration::from_secs(60);

pub fn boot_command() -> Vec<String> {
    vec![
        "bash".into(),
        "-c".into(),
        "set -e; curl -fsSL --retry 5 --retry-all-errors -H \"Authorization: Bearer $CORRA_TOKEN\" \"$CORRA_HUB/dist/runner\" -o /usr/local/bin/corra-runner; chmod +x /usr/local/bin/corra-runner; exec /usr/local/bin/corra-runner".into(),
    ]
}

impl Providers {
    pub fn new(hub_url: String) -> Self {
        let mut by_name: HashMap<&'static str, Arc<dyn Provider>> = HashMap::new();
        let all: Vec<Arc<dyn Provider>> = vec![
            Arc::new(existing::Existing),
            Arc::new(modal::Modal::new()),
            Arc::new(agent37::Agent37),
            Arc::new(phone::Phone),
        ];
        for p in all {
            by_name.insert(p.name(), p);
        }
        Providers {
            by_name: RwLock::new(by_name),
            starting: Arc::default(),
            hub_url,
        }
    }

    pub fn add(&self, p: Arc<dyn Provider>) {
        self.by_name.write().unwrap().insert(p.name(), p);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Provider>> {
        self.by_name.read().unwrap().get(name).cloned()
    }

    pub fn is_starting(&self, m: MachineId) -> bool {
        self.starting.lock().unwrap().contains(&m)
    }

    pub fn names(&self) -> Vec<&'static str> {
        let mut v: Vec<_> = self.by_name.read().unwrap().keys().copied().collect();
        v.sort();
        v
    }

    pub fn default_name(&self) -> &'static str {
        "existing"
    }

    pub async fn start_for_project(&self, hub: &Arc<Hub>, p: &Project) -> Result<MachineId> {
        if let Some(m) = p.default_machine
            && hub.runner_connected(m)
        {
            return Ok(m);
        }
        for m in hub.db.machines()? {
            if m.project == Some(p.id) && hub.runner_connected(m.id) {
                return Ok(m.id);
            }
        }
        let starting = {
            let mut s = hub.starting.lock().unwrap();
            match s.get(&p.id) {
                Some(n) => Some(n.clone()),
                None => {
                    let n = Arc::new(tokio::sync::Notify::new());
                    s.insert(p.id, n);
                    None
                }
            }
        };
        if let Some(n) = starting {
            let _ = tokio::time::timeout(Duration::from_secs(900), n.notified()).await;
            for m in hub.db.machines()? {
                if m.project == Some(p.id) && hub.runner_connected(m.id) {
                    return Ok(m.id);
                }
            }
            bail!("the project's machine didn't start");
        }
        let result = self.wake_or_create(hub, p).await;
        if let Some(n) = hub.starting.lock().unwrap().remove(&p.id) {
            n.notify_waiters();
        }
        result
    }

    fn machine_env(&self, hub: &Hub, machine: MachineId) -> Result<Vec<(String, String)>> {
        let token = match hub.secrets.get(&format!("machine_token.{machine}"))? {
            Some(t) => t,
            None => {
                let t = new_token("cm");
                hub.db.set_machine_token(machine, &hash_token(&t))?;
                hub.secrets.set(&format!("machine_token.{machine}"), &t)?;
                t
            }
        };
        Ok(vec![
            ("CORRA_HUB".into(), self.hub_url.clone()),
            ("CORRA_TOKEN".into(), token),
        ])
    }

    async fn wake_or_create(&self, hub: &Arc<Hub>, p: &Project) -> Result<MachineId> {
        let provider = self
            .get(&p.provider)
            .ok_or_else(|| anyhow!("unknown provider {}", p.provider))?;
        if !provider.creates_machines() {
            bail!(
                "project {} has no running machine; start its runner",
                p.name
            );
        }
        let asleep = hub.db.machines()?.into_iter().find(|m| {
            m.project == Some(p.id) && m.provider == p.provider && m.status == MachineStatus::Asleep
        });
        let (m, woke) = match asleep {
            Some(m) => (m, true),
            None => {
                let m = hub.db.insert_machine(NewMachine {
                    name: format!("corra-{}-{}", p.name, &new_id().simple().to_string()[26..]),
                    provider: p.provider.clone(),
                    project: Some(p.id),
                    size: p.size.clone(),
                    token_hash: hash_token(&new_token("cm")),
                    provider_ref: None,
                    status: MachineStatus::Creating,
                })?;
                (m, false)
            }
        };
        self.starting.lock().unwrap().insert(m.id);
        let _starting = Starting(self.starting.clone(), m.id);
        hub.db.kv_set(&starting_key(m.id), &now_ms().to_string())?;
        hub.db.set_machine_status(m.id, MachineStatus::Creating)?;
        hub.publish_machine(m.id);
        let req = CreateRequest {
            name: m.name.clone(),
            machine: m.id,
            size: Size::parse(&p.size),
            env: self.machine_env(hub, m.id)?,
        };
        let started = match (woke, hub.db.machine_sleep_ref(m.id)?) {
            (true, Some(sleep_ref)) => provider.wake(hub, &sleep_ref, req).await,
            _ => provider.create(hub, req).await,
        };
        let r = match started {
            Ok(r) => r,
            Err(e) => {
                hub.db.set_machine_status(
                    m.id,
                    if woke {
                        MachineStatus::Asleep
                    } else {
                        MachineStatus::Failed
                    },
                )?;
                hub.publish_machine(m.id);
                return Err(e);
            }
        };
        hub.db.set_machine_provider_ref(m.id, &r)?;
        hub.db
            .kv_set(&format!("awake_since:{}", m.id), &now_ms().to_string())?;
        let began = tokio::time::Instant::now();
        let mut checked = began;
        let failure = loop {
            if hub.runner_connected(m.id) {
                crate::kit::prepare(hub, p, m.id).await?;
                return Ok(m.id);
            }
            if began.elapsed() >= RUNNER_WAIT {
                break "the machine didn't connect within 5 minutes";
            }
            if checked.elapsed() >= STATUS_EVERY {
                checked = tokio::time::Instant::now();
                match provider.status(hub, &r).await {
                    Ok(ProviderStatus::Gone) => {
                        break "the machine stopped before its runner connected";
                    }
                    Ok(ProviderStatus::Stopped) if began.elapsed() >= STOPPED_GRACE => {
                        break "the machine stopped before its runner connected";
                    }
                    _ => {}
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        };
        hub.db.set_machine_status(m.id, MachineStatus::Failed)?;
        hub.publish_machine(m.id);
        let _ = provider.destroy(hub, &r).await;
        bail!(failure)
    }

    pub async fn destroy_machine(&self, hub: &Hub, id: MachineId) -> Result<()> {
        let m = hub
            .db
            .machine(id)?
            .ok_or_else(|| anyhow!("no such machine"))?;
        let provider = self
            .get(&m.provider)
            .ok_or_else(|| anyhow!("unknown provider"))?;
        if !provider.creates_machines() {
            bail!(
                "{} is an existing machine; corra doesn't destroy those",
                m.name
            );
        }
        hub.db.set_machine_status(id, MachineStatus::Destroying)?;
        hub.publish_machine(id);
        if let Some(r) = hub.db.machine_provider_ref(id)? {
            provider.destroy(hub, &r).await?;
        }
        hub.db.set_machine_status(id, MachineStatus::Destroyed)?;
        hub.publish_machine(id);
        Ok(())
    }
}

pub fn hub_tag(hub: &Hub) -> String {
    let id = hub_id(hub);
    id[id.len().saturating_sub(8)..].to_string()
}
