mod projects;
mod runner;
mod threads;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use anyhow::Result;
use axum::http::HeaderMap;
use corra_proto::app::HubToApp;
use corra_proto::runner::HubMsg;
use corra_proto::*;
use serde_json::Value;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use crate::db::{Db, MachineAuth};
use crate::providers::Providers;

pub enum Outbound {
    All(HubToApp),
    Thread(ThreadId, HubToApp),
    Session(String, HubToApp),
}

pub struct RunnerConn {
    pub tx: mpsc::UnboundedSender<HubMsg>,
    pub live: HashSet<ThreadId>,
    pub prepared: HashSet<ProjectId>,
    pub conn_id: u64,
    pub takes: bool,
    pub prompt: bool,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Delivery {
    pub thread: ThreadId,
    pub input: Vec<InputPart>,
    #[serde(default)]
    pub settings: Option<AgentSettings>,
    pub at: i64,
}

type Pending = Mutex<HashMap<u64, (MachineId, oneshot::Sender<Result<Value, String>>)>>;
pub type GitPendingMap = Mutex<HashMap<String, watch::Receiver<Option<Result<(), String>>>>>;

pub struct Hub {
    pub data: std::path::PathBuf,
    pub db: Db,
    pub runners: Mutex<HashMap<MachineId, RunnerConn>>,
    pub apps: broadcast::Sender<Arc<Outbound>>,
    pub pending: Pending,
    pub providers: Providers,
    pub secrets: crate::secrets::Secrets,
    pub http: reqwest::Client,
    pub live_status: Mutex<HashMap<ThreadId, serde_json::Map<String, Value>>>,
    pub hub_requests: Mutex<HashMap<String, oneshot::Sender<RequestResponse>>>,
    pub secret_requests: Mutex<HashMap<String, oneshot::Sender<Option<String>>>>,
    answered: Mutex<HashMap<String, Instant>>,
    pub quiet_turns: Mutex<HashMap<ThreadId, Instant>>,
    context_seen: Mutex<HashMap<ThreadId, (u64, i64)>>,
    pub starting: Mutex<HashMap<ProjectId, Arc<tokio::sync::Notify>>>,
    pub activity: Mutex<HashMap<MachineId, i64>>,
    pub compact_woke: Mutex<HashSet<MachineId>>,
    pub watching: Mutex<HashMap<ThreadId, usize>>,
    pub terms: Mutex<HashMap<String, (ThreadId, MachineId)>>,
    pub quiet_stops: Mutex<HashMap<ThreadId, Instant>>,
    pub delivering: Mutex<HashMap<ThreadId, usize>>,
    pub presence: Mutex<HashMap<u64, Instant>>,
    pub github_api: Mutex<String>,
    pub kit_cache: Mutex<Option<(Instant, Arc<crate::kit::Kit>)>>,
    pub git_pending: GitPendingMap,
    pub project_stages: Mutex<HashMap<ProjectId, String>>,
    shutdown: watch::Sender<bool>,
    me: Weak<Hub>,
    next_id: AtomicU64,
}

pub type HubRef = Arc<Hub>;

pub const TRUSTED_ENV: &[(&str, &str)] = &[
    ("github.token", "GH_TOKEN"),
    ("modal.token_id", "MODAL_TOKEN_ID"),
    ("modal.token_secret", "MODAL_TOKEN_SECRET"),
    ("railway.token", "RAILWAY_API_TOKEN"),
    ("cloudflare.token", "CLOUDFLARE_API_TOKEN"),
    ("cloudflare.account_id", "CLOUDFLARE_ACCOUNT_ID"),
];

#[derive(Default)]
pub struct NewProject {
    pub name: String,
    pub repos: Vec<String>,
    pub machine: Option<MachineId>,
    pub path: Option<String>,
    pub provider: Option<String>,
    pub kind: Option<String>,
    pub size: Option<String>,
    pub setup: Option<String>,
    pub env: Vec<String>,
    pub tools: Option<Vec<String>>,
    pub permission_mode: Option<String>,
    pub allow_main: bool,
    pub trusted: bool,
}

pub struct History {
    pub events: Vec<StoredEvent>,
    pub from_seq: i64,
    pub earlier: bool,
}

impl Hub {
    pub fn new(
        db: Db,
        providers: Providers,
        secrets: crate::secrets::Secrets,
        data: std::path::PathBuf,
    ) -> HubRef {
        let (apps, _) = broadcast::channel(4096);
        let (shutdown, _) = watch::channel(false);
        Arc::new_cyclic(|me| Hub {
            me: me.clone(),
            quiet_stops: Mutex::new(HashMap::new()),
            delivering: Mutex::new(HashMap::new()),
            presence: Mutex::new(HashMap::new()),
            github_api: Mutex::new("https://api.github.com".into()),
            kit_cache: Mutex::new(None),
            git_pending: Mutex::new(HashMap::new()),
            project_stages: Mutex::new(HashMap::new()),
            shutdown,
            data,
            db,
            runners: Mutex::new(HashMap::new()),
            apps,
            pending: Mutex::new(HashMap::new()),
            providers,
            secrets,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(600))
                .build()
                .expect("http client"),
            live_status: Mutex::new(HashMap::new()),
            hub_requests: Mutex::new(HashMap::new()),
            secret_requests: Mutex::new(HashMap::new()),
            answered: Mutex::new(HashMap::new()),
            quiet_turns: Mutex::new(HashMap::new()),
            context_seen: Mutex::new(HashMap::new()),
            starting: Mutex::new(HashMap::new()),
            activity: Mutex::new(HashMap::new()),
            compact_woke: Mutex::new(HashSet::new()),
            watching: Mutex::new(HashMap::new()),
            terms: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        })
    }

    pub fn subscribe_shutdown(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    pub fn begin_shutdown(&self) {
        let _ = self.shutdown.send(true);
    }

    pub fn auth_machine_token(&self, token: &str) -> Result<Option<MachineAuth>> {
        if token.is_empty() {
            return Ok(None);
        }
        let Some(auth) = self.db.machine_by_token(&hash_token(token))? else {
            return Ok(None);
        };
        if matches!(
            auth.status,
            MachineStatus::Destroyed | MachineStatus::Destroying
        ) {
            return Ok(None);
        }
        Ok(Some(auth))
    }

    pub fn auth_machine_bearer(&self, headers: &HeaderMap) -> Option<MachineId> {
        let token = headers
            .get("authorization")?
            .to_str()
            .ok()?
            .strip_prefix("Bearer ")?;
        self.auth_machine_token(token).ok()?.map(|a| a.id)
    }

    pub fn git_token(&self, machine: MachineId) -> Option<String> {
        let (status, token_hash) = self.db.machine_auth(machine).ok()??;
        if matches!(status, MachineStatus::Destroyed | MachineStatus::Destroying) {
            return None;
        }
        let sig = self.secrets.derive_hex(
            "corra-git-token-v1",
            format!("{machine}:{token_hash}").as_bytes(),
        );
        Some(format!("cg_{machine}_{sig}"))
    }

    pub fn verify_git_token(&self, token: &str) -> Result<Option<MachineAuth>> {
        let Some(rest) = token.strip_prefix("cg_") else {
            return Ok(None);
        };
        let Some((id_str, sig)) = rest.split_once('_') else {
            return Ok(None);
        };
        let Ok(id) = id_str.parse::<MachineId>() else {
            return Ok(None);
        };
        let Some((status, token_hash)) = self.db.machine_auth(id)? else {
            return Ok(None);
        };
        if matches!(status, MachineStatus::Destroyed | MachineStatus::Destroying) {
            return Ok(None);
        }
        let expected = self.secrets.derive_hex(
            "corra-git-token-v1",
            format!("{id}:{token_hash}").as_bytes(),
        );
        if !const_eq(sig.as_bytes(), expected.as_bytes()) {
            return Ok(None);
        }
        Ok(Some(MachineAuth { id, status }))
    }

    pub fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn me(&self) -> Option<HubRef> {
        self.me.upgrade()
    }

    pub fn publish(&self, out: Outbound) {
        let _ = self.apps.send(Arc::new(out));
    }

    pub fn publish_thread(&self, t: &Thread) {
        self.publish(Outbound::All(HubToApp::Thread { thread: t.clone() }));
    }

    pub fn publish_machine(&self, id: MachineId) {
        if let Ok(Some(m)) = self.db.machine(id) {
            self.publish(Outbound::All(HubToApp::Machine { machine: m }));
        }
    }

    pub fn projects(&self) -> Result<Vec<Project>> {
        self.db.projects()
    }

    pub fn publish_project(&self, p: &Project) {
        self.publish(Outbound::All(HubToApp::Project { project: p.clone() }));
    }

    pub fn settings(&self) -> Value {
        self.db
            .kv_get("settings")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| serde_json::json!({}))
    }

    pub fn set_setting(&self, key: &str, value: Value) -> Result<()> {
        let mut cur = self.settings();
        if let Some(c) = cur.as_object_mut() {
            c.insert(key.to_string(), value);
        }
        self.db.kv_set("settings", &cur.to_string())
    }

    pub fn setting_str(&self, key: &str) -> Option<String> {
        self.settings()[key]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    }

    pub fn env_names(&self) -> Result<Vec<String>> {
        let mut names: Vec<String> = self
            .secrets
            .names()?
            .into_iter()
            .filter_map(|n| n.strip_prefix("env.").map(String::from))
            .collect();
        names.sort();
        Ok(names)
    }

    pub fn trusted_env(&self) -> Vec<(String, String)> {
        TRUSTED_ENV
            .iter()
            .filter_map(|(secret, var)| {
                let v = self.secrets.get(secret).ok().flatten()?;
                let v = v.trim().to_string();
                (!v.is_empty()).then(|| (var.to_string(), v))
            })
            .collect()
    }

    pub fn machine_trusted(&self, machine: MachineId) -> bool {
        let Ok(Some(m)) = self.db.machine(machine) else {
            return false;
        };
        self.db
            .projects()
            .unwrap_or_default()
            .iter()
            .any(|p| p.trusted && (m.project == Some(p.id) || p.default_machine == Some(machine)))
    }

    pub fn project_env(&self, p: &Project) -> Vec<(String, String)> {
        p.env
            .iter()
            .filter_map(|name| {
                self.secrets
                    .get(&format!("env.{name}"))
                    .ok()
                    .flatten()
                    .map(|v| (name.clone(), v))
            })
            .collect()
    }

    pub fn snapshot(&self) -> Result<HubToApp> {
        Ok(HubToApp::Snapshot {
            projects: self.projects()?,
            machines: self.db.machines()?,
            threads: self.db.threads()?,
            build: crate::web::build(),
        })
    }

    pub fn touch_machine(&self, machine: MachineId) {
        self.activity.lock().unwrap().insert(machine, now_ms());
    }

    pub fn watch(&self, thread: ThreadId, on: bool) {
        let mut w = self.watching.lock().unwrap();
        let n = w.entry(thread).or_insert(0);
        if on {
            *n += 1;
        } else {
            *n = n.saturating_sub(1);
            if *n == 0 {
                w.remove(&thread);
            }
        }
    }

    pub fn usage(&self) -> Option<HubToApp> {
        let v: Value = serde_json::from_str(&self.db.kv_get("usage").ok()??).ok()?;
        Some(HubToApp::Usage {
            at: v["at"].as_i64()?,
            limits: v["limits"].clone(),
        })
    }

    pub fn set_live_status(&self, thread: ThreadId, status: Value) {
        if let Some(n) = status["context"]["used_tokens"].as_u64().filter(|n| *n > 0) {
            self.context_seen
                .lock()
                .unwrap()
                .insert(thread, (n, now_ms()));
        }
        if !status["rate_limit"].is_null() {
            let limits = status["rate_limit"].clone();
            tracing::info!("usage reported: {limits}");
            let at = now_ms();
            let _ = self.db.kv_set(
                "usage",
                &serde_json::json!({ "at": at, "limits": limits }).to_string(),
            );
            self.publish(Outbound::All(HubToApp::Usage { at, limits }));
        }
        let merged = {
            let mut all = self.live_status.lock().unwrap();
            let entry = all.entry(thread).or_default();
            if let Value::Object(o) = status {
                for (k, v) in o {
                    if k != "kind" && !v.is_null() {
                        entry.insert(k, v);
                    }
                }
            }
            Value::Object(entry.clone())
        };
        self.publish(Outbound::Thread(
            thread,
            HubToApp::Status {
                thread,
                status: merged,
            },
        ));
    }

    pub fn live_status(&self, thread: ThreadId) -> Option<Value> {
        let mut status = self.live_status.lock().unwrap().get(&thread).cloned();
        if let Some(at) = crate::schedule::planned_continue(self, thread) {
            status.get_or_insert_default().insert(
                "continue_at".into(),
                crate::schedule::continue_time(at).into(),
            );
        }
        status.map(Value::Object)
    }

    pub fn hands_off(&self, agent: AgentKind) -> bool {
        agent == AgentKind::Antigravity
            && !self
                .secrets
                .get("gemini.api_key")
                .ok()
                .flatten()
                .is_some_and(|k| !k.trim().is_empty())
    }

    pub(crate) fn agent_env(&self, agent: AgentKind) -> Result<Vec<(String, String)>> {
        let mut env = vec![];
        if agent == AgentKind::Antigravity
            && let Some(k) = self.secrets.get("gemini.api_key")?
            && !k.trim().is_empty()
        {
            env.push(("GEMINI_API_KEY".to_string(), k.trim().to_string()));
        }
        if agent == AgentKind::Claude {
            let clean = |s: String| s.split_whitespace().collect::<String>();
            if let Some(t) = self.secrets.get("claude.token")? {
                env.push(("CLAUDE_CODE_OAUTH_TOKEN".to_string(), clean(t)));
            } else if let Some(k) = self.secrets.get("anthropic.api_key")? {
                env.push(("ANTHROPIC_API_KEY".to_string(), clean(k)));
            }
        }
        Ok(env)
    }

    pub fn thread_kv(&self, thread: ThreadId, key: &str) -> Option<String> {
        self.db
            .kv_get(&format!("thread:{thread}:{key}"))
            .ok()
            .flatten()
    }

    pub fn set_thread_kv(&self, thread: ThreadId, key: &str, value: &str) {
        let _ = self.db.kv_set(&format!("thread:{thread}:{key}"), value);
    }

    pub fn take_thread_kv(&self, thread: ThreadId, key: &str) -> Option<String> {
        let k = format!("thread:{thread}:{key}");
        let v = self.db.kv_get(&k).ok().flatten();
        if v.is_some() {
            let _ = self.db.kv_delete(&k);
        }
        v
    }

    pub fn retention_days(&self) -> Option<i64> {
        let v = &self.settings()["retention_days"];
        v.as_i64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            .filter(|d| *d > 0)
    }
}

pub fn input_text(input: &[InputPart]) -> String {
    input
        .iter()
        .filter_map(|p| match p {
            InputPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn title_from(input: &[InputPart]) -> String {
    let text = input
        .iter()
        .find_map(|p| match p {
            InputPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .unwrap_or("New thread");
    let line = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("New thread")
        .trim();
    let mut out: String = line.chars().take(80).collect();
    if line.chars().count() > 80 {
        out.push('…');
    }
    out
}

pub fn hash_token(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(token.as_bytes());
    d.iter().map(|b| format!("{b:02x}")).collect()
}

fn const_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

pub fn new_token(prefix: &str) -> String {
    use rand::RngExt;
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    use base64::Engine;
    format!(
        "{prefix}_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    )
}
