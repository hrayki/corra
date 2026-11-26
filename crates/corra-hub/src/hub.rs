use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use corra_proto::app::HubToApp;
use corra_proto::runner::{HubMsg, RunnerRequest};
use corra_proto::*;
use serde_json::Value;
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::db::Db;
use crate::providers::Providers;

pub enum Outbound {
    All(HubToApp),
    Thread(ThreadId, HubToApp),
    Machine(MachineId, HubToApp),
    Session(String, HubToApp),
}

pub struct RunnerConn {
    pub tx: mpsc::UnboundedSender<HubMsg>,
    pub live: HashSet<ThreadId>,
    pub prepared: HashSet<ProjectId>,
    pub conn_id: u64,
}

type Pending = Mutex<HashMap<u64, (MachineId, oneshot::Sender<Result<Value, String>>)>>;

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
    pub sent: Mutex<std::collections::VecDeque<String>>,
    answered: Mutex<HashMap<String, std::time::Instant>>,
    pub quiet_turns: Mutex<HashMap<ThreadId, std::time::Instant>>,
    context_seen: Mutex<HashMap<ThreadId, (u64, i64)>>,
    pub starting: Mutex<HashMap<ProjectId, Arc<tokio::sync::Notify>>>,
    pub activity: Mutex<HashMap<MachineId, i64>>,
    pub watching: Mutex<HashMap<ThreadId, usize>>,
    pub terms: Mutex<HashMap<String, (ThreadId, MachineId)>>,
    pub browser_watch: Mutex<HashMap<MachineId, usize>>,
    pub phone: crate::phone::PhoneState,
    pub quiet_stops: Mutex<HashMap<ThreadId, std::time::Instant>>,
    pub presence: Mutex<HashMap<u64, std::time::Instant>>,
    pub github_api: Mutex<String>,
    me: Weak<Hub>,
    next_id: AtomicU64,
}

pub type HubRef = Arc<Hub>;

pub const TRUSTED_ENV: &[(&str, &str)] = &[
    ("github.token", "GH_TOKEN"),
    ("modal.token_id", "MODAL_TOKEN_ID"),
    ("modal.token_secret", "MODAL_TOKEN_SECRET"),
    ("digitalocean.token", "DIGITALOCEAN_ACCESS_TOKEN"),
    ("agent37.key", "AGENT37_API_KEY"),
    ("railway.token", "RAILWAY_API_TOKEN"),
    ("cloudflare.token", "CLOUDFLARE_API_TOKEN"),
    ("cloudflare.account_id", "CLOUDFLARE_ACCOUNT_ID"),
];

#[derive(serde::Deserialize)]
pub struct ImportProject {
    pub name: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub settings: AgentSettings,
}

#[derive(serde::Deserialize)]
pub struct ImportThread {
    pub thread: Thread,
    pub project: ImportProject,
    #[serde(default)]
    pub events: Vec<StoredEvent>,
}

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
    pub fn history_before(&self, thread: ThreadId, before: i64, turns: i64) -> Result<History> {
        let starts = self.db.turn_starts_before(thread, before, turns + 1)?;
        let (from, earlier) = match starts.get(turns as usize) {
            Some(_) => (starts[turns as usize - 1], true),
            None => (0, false),
        };
        let mut events = self.db.events_between(thread, from, before)?;
        if earlier
            && before == i64::MAX
            && let Some(s) = self.db.last_of_kind_before(thread, "session", from)?
        {
            events.insert(0, s);
        }
        Ok(History {
            events,
            from_seq: from,
            earlier,
        })
    }

    pub fn new(
        db: Db,
        providers: Providers,
        secrets: crate::secrets::Secrets,
        data: std::path::PathBuf,
    ) -> HubRef {
        let (apps, _) = broadcast::channel(4096);
        Arc::new_cyclic(|me| Hub {
            me: me.clone(),
            quiet_stops: Mutex::new(HashMap::new()),
            presence: Mutex::new(HashMap::new()),
            github_api: Mutex::new("https://api.github.com".into()),
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
            sent: Mutex::new(std::collections::VecDeque::new()),
            answered: Mutex::new(HashMap::new()),
            quiet_turns: Mutex::new(HashMap::new()),
            context_seen: Mutex::new(HashMap::new()),
            starting: Mutex::new(HashMap::new()),
            activity: Mutex::new(HashMap::new()),
            watching: Mutex::new(HashMap::new()),
            terms: Mutex::new(HashMap::new()),
            browser_watch: Mutex::new(HashMap::new()),
            phone: Default::default(),
            next_id: AtomicU64::new(1),
        })
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

    pub fn runner_connected(&self, machine: MachineId) -> bool {
        self.runners.lock().unwrap().contains_key(&machine)
    }

    pub fn send_runner_pub(&self, machine: MachineId, msg: HubMsg) -> Result<()> {
        self.send_runner(machine, msg)
    }

    fn send_runner(&self, machine: MachineId, msg: HubMsg) -> Result<()> {
        let runners = self.runners.lock().unwrap();
        let conn = runners
            .get(&machine)
            .ok_or_else(|| anyhow!("machine is not connected"))?;
        conn.tx
            .send(msg)
            .map_err(|_| anyhow!("machine connection closed"))
    }

    pub async fn runner_request(&self, machine: MachineId, body: RunnerRequest) -> Result<Value> {
        self.runner_request_timeout(machine, body, Duration::from_secs(30))
            .await
    }

    pub async fn runner_request_timeout(
        &self,
        machine: MachineId,
        body: RunnerRequest,
        timeout: Duration,
    ) -> Result<Value> {
        let id = self.next_id();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, (machine, tx));
        if let Err(e) = self.send_runner(machine, HubMsg::Request { id, body }) {
            self.pending.lock().unwrap().remove(&id);
            return Err(e);
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(anyhow!(e)),
            Ok(Err(_)) => Err(anyhow!("machine disconnected")),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(anyhow!("machine did not answer in time"))
            }
        }
    }

    pub fn resolve_pending(&self, machine: MachineId, id: u64, result: Result<Value, String>) {
        let tx = {
            let mut p = self.pending.lock().unwrap();
            match p.get(&id) {
                Some((m, _)) if *m == machine => p.remove(&id).map(|(_, tx)| tx),
                Some(_) => {
                    tracing::warn!(%machine, "a machine answered a request that wasn't sent to it");
                    None
                }
                None => None,
            }
        };
        if let Some(tx) = tx {
            let _ = tx.send(result);
        }
    }

    pub fn append(
        &self,
        thread: ThreadId,
        runner_seq: Option<i64>,
        ts: i64,
        event: Event,
    ) -> Result<()> {
        let Some(stored) = self.db.append_event(thread, runner_seq, ts, &event)? else {
            return Ok(());
        };
        if let Ok(Some(t)) = self.db.thread(thread)
            && let Some(m) = t.machine
        {
            self.touch_machine(m);
        }
        let mut changed = false;
        if let Some(mut t) = self.db.thread(thread)? {
            match &event {
                Event::Session {
                    session_id, model, ..
                } => {
                    t.session_id = Some(session_id.clone());
                    if t.settings.model.is_none() {
                        t.settings.model = model.clone();
                    }
                    changed = true;
                }
                Event::SettingsChanged { settings } => {
                    t.settings.merge(settings);
                    changed = true;
                }
                Event::TurnStarted { .. } => {
                    t.status = ThreadStatus::Running;
                    t.settled_at = None;
                    changed = true;
                }
                Event::Request { .. } => {
                    t.status = ThreadStatus::Waiting;
                    changed = true;
                }
                Event::RequestResolved { .. } => {
                    t.status = ThreadStatus::Running;
                    changed = true;
                }
                Event::TurnCompleted { outcome, .. } => {
                    t.status = if *outcome == TurnOutcome::Failed {
                        ThreadStatus::Error
                    } else {
                        ThreadStatus::Idle
                    };
                    changed = true;
                }
                _ => {}
            }
            if changed {
                t.last_seq = stored.seq;
                t.updated_at = now_ms();
                self.db.update_thread(&t)?;
                self.publish_thread(&t);
            }
        }
        self.publish(Outbound::Thread(
            thread,
            HubToApp::Events {
                thread,
                events: vec![stored],
            },
        ));
        crate::notify::on_event(self, thread, &event);
        self.tell_follower(thread, &event);
        match &event {
            Event::TurnStarted { input, .. } => {
                crate::schedule::cancel_continue(self, thread);
                if !matches!(&input[..], [InputPart::Text { text }] if text == crate::schedule::COMPACT)
                {
                    self.quiet_turns.lock().unwrap().remove(&thread);
                }
            }
            Event::TurnCompleted {
                outcome: TurnOutcome::Completed,
                ..
            } => {
                self.take_thread_kv(thread, "continues");
                self.note_context(thread);
                self.send_next_queued(thread);
            }
            Event::TurnCompleted {
                outcome: TurnOutcome::Failed,
                resets_at: Some(at),
                ..
            } => {
                self.note_context(thread);
                crate::schedule::plan_continue(self, thread, *at)
            }
            Event::TurnCompleted { .. } => self.note_context(thread),
            _ => {}
        }
        Ok(())
    }

    fn note_context(&self, thread: ThreadId) {
        let Some((tokens, at)) = self.context_seen.lock().unwrap().remove(&thread) else {
            return;
        };
        crate::schedule::note_cache_use(self, thread, tokens, at);
    }

    pub fn follower(&self, thread: ThreadId) -> Option<ThreadId> {
        self.thread_kv(thread, "follower")?.parse().ok()
    }

    fn followed(&self, thread: ThreadId) -> Option<(ThreadId, Thread, String)> {
        let follower = self.follower(thread).filter(|f| *f != thread)?;
        let t = self.db.thread(thread).ok()??;
        let project = self.db.project(t.project).ok()??.name;
        Some((follower, t, project))
    }

    pub fn tell_follower(&self, thread: ThreadId, event: &Event) {
        if !matches!(event, Event::TurnCompleted { .. } | Event::Request { .. }) {
            return;
        }
        let Some((follower, t, project)) = self.followed(thread) else {
            return;
        };
        let mut last = None;
        if let Event::TurnCompleted { turn, outcome, .. } = event {
            if crate::schedule::take_quiet_turn(self, thread) {
                return;
            }
            let asked = self.quiet_stops.lock().unwrap().remove(&thread);
            if *outcome == TurnOutcome::Interrupted
                && asked.is_some_and(|at| at.elapsed() < Duration::from_secs(60))
            {
                return;
            }
            last = self
                .db
                .events_tail(thread, 200)
                .ok()
                .and_then(|tail| crate::follow::last_message(&tail, turn));
        }
        if let Some(text) = crate::follow::note(&t, &project, event, last.as_deref()) {
            self.say_to(thread, follower, text);
        }
    }

    pub fn tell_follower_text(&self, thread: ThreadId, what: &str) {
        if let Some((follower, t, project)) = self.followed(thread) {
            self.say_to(thread, follower, crate::follow::line(&t, &project, what));
        }
    }

    fn say_to(&self, thread: ThreadId, follower: ThreadId, text: String) {
        match self.db.thread(follower) {
            Ok(Some(f)) if f.hidden_at.is_none() => {}
            _ => {
                let _ = self.db.kv_delete(&format!("thread:{thread}:follower"));
                return;
            }
        }
        let Some(hub) = self.me.upgrade() else {
            return;
        };
        tokio::spawn(async move {
            let call = corra_proto::app::AppCall::Send {
                thread: follower,
                input: vec![InputPart::Text { text }],
                settings: None,
                queue: false,
                client_id: None,
                compact: None,
            };
            if let Err(e) = crate::ws::exec_call(&hub, call).await {
                tracing::warn!(%thread, %follower, "couldn't tell the follower: {e:#}");
            }
        });
    }

    pub fn browser_unwatch(&self, machine: MachineId) {
        let stop = {
            let mut w = self.browser_watch.lock().unwrap();
            let n = w.entry(machine).or_insert(0);
            *n = n.saturating_sub(1);
            if *n == 0 {
                w.remove(&machine);
                true
            } else {
                false
            }
        };
        if stop {
            let phone = crate::phone::is_machine(self, machine);
            let _ = self.send_runner(
                machine,
                if phone {
                    HubMsg::PhoneStop
                } else {
                    HubMsg::BrowserStop
                },
            );
        }
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
        if let Some(c) = crate::schedule::cache_use(self, thread) {
            status
                .get_or_insert_default()
                .insert("cache".into(), crate::schedule::cache_status(self, &c));
        }
        status.map(Value::Object)
    }

    pub async fn ask(
        &self,
        thread: ThreadId,
        turn: &str,
        action: &str,
        input: &Value,
    ) -> Result<RequestResponse> {
        let id = format!("hub-{}", new_id());
        let (tx, rx) = oneshot::channel();
        self.hub_requests.lock().unwrap().insert(id.clone(), tx);
        self.append(
            thread,
            None,
            now_ms(),
            Event::Request {
                id: id.clone(),
                turn: turn.to_string(),
                request: AgentRequest::HubAction {
                    action: action.to_string(),
                    input: input.clone(),
                },
            },
        )?;
        rx.await.map_err(|_| anyhow!("the request was dropped"))
    }

    pub async fn ask_secret(
        &self,
        thread: ThreadId,
        turn: &str,
        name: &str,
        why: &str,
    ) -> Result<Option<String>> {
        let id = format!("hub-{}", new_id());
        let (tx, rx) = oneshot::channel();
        self.secret_requests.lock().unwrap().insert(id.clone(), tx);
        self.append(
            thread,
            None,
            now_ms(),
            Event::Request {
                id,
                turn: turn.to_string(),
                request: AgentRequest::HubAction {
                    action: "secret.request".into(),
                    input: serde_json::json!({ "name": name, "why": why }),
                },
            },
        )?;
        rx.await.map_err(|_| anyhow!("the request was dropped"))
    }

    pub fn answer_secret(&self, thread: ThreadId, request: String, value: String) -> Result<()> {
        if value.is_empty() {
            bail!("the secret is empty");
        }
        let Some(tx) = self.secret_requests.lock().unwrap().remove(&request) else {
            bail!("that request is no longer waiting");
        };
        self.append(
            thread,
            None,
            now_ms(),
            Event::RequestResolved {
                id: request,
                response: RequestResponse::Allow { always: false },
            },
        )?;
        let _ = tx.send(Some(value));
        Ok(())
    }

    pub fn close_open_turn(&self, thread: ThreadId) -> Result<bool> {
        let tail = self.db.events_tail(thread, 2000)?;
        let mut open: Option<String> = None;
        let mut requests: Vec<String> = vec![];
        for e in &tail {
            match &e.event {
                Event::TurnStarted { turn, .. } => open = Some(turn.clone()),
                Event::TurnCompleted { turn, .. } if open.as_deref() == Some(turn.as_str()) => {
                    open = None
                }
                Event::Request { id, .. } => requests.push(id.clone()),
                Event::RequestResolved { id, .. } => requests.retain(|x| x != id),
                _ => {}
            }
        }
        for id in requests {
            if let Some(tx) = self.secret_requests.lock().unwrap().remove(&id) {
                let _ = tx.send(None);
            }
            if let Some(tx) = self.hub_requests.lock().unwrap().remove(&id) {
                let _ = tx.send(RequestResponse::Cancelled);
            }
            self.append(
                thread,
                None,
                now_ms(),
                Event::RequestResolved {
                    id,
                    response: RequestResponse::Cancelled,
                },
            )?;
        }
        let Some(turn) = open else {
            return Ok(false);
        };
        self.append(
            thread,
            None,
            now_ms(),
            Event::TurnCompleted {
                turn,
                outcome: TurnOutcome::Interrupted,
                usage: None,
                error: None,
                anchor: None,
                resets_at: None,
            },
        )?;
        Ok(true)
    }

    pub fn close_stale_requests(&self) -> Result<()> {
        for t in self.db.threads()? {
            if t.status != ThreadStatus::Waiting {
                continue;
            }
            let tail = self.db.events_tail(t.id, 500)?;
            for id in unanswered_hub_requests(&tail) {
                self.append(
                    t.id,
                    None,
                    now_ms(),
                    Event::RequestResolved {
                        id,
                        response: RequestResponse::Cancelled,
                    },
                )?;
                self.hub_notice(
                    t.id,
                    NoticeLevel::Warning,
                    "The hub restarted while this waited for a yes, so the request was cancelled. Ask the agent to try again.",
                );
            }
        }
        Ok(())
    }

    pub fn set_thread_status(&self, thread: ThreadId, status: ThreadStatus) -> Result<()> {
        if let Some(mut t) = self.db.thread(thread)?
            && t.status != status
        {
            t.status = status;
            t.updated_at = now_ms();
            self.db.update_thread(&t)?;
            self.publish_thread(&t);
        }
        Ok(())
    }

    pub fn hub_notice(&self, thread: ThreadId, level: NoticeLevel, text: impl Into<String>) {
        let _ = self.append(
            thread,
            None,
            now_ms(),
            Event::Notice {
                level,
                text: text.into(),
            },
        );
    }

    pub fn default_provider(&self) -> String {
        if let Some(p) = self.setting_str("default_provider") {
            return p;
        }
        let has = |k: &str| self.secrets.get(k).ok().flatten().is_some();
        if has("modal.token_id") {
            "modal".into()
        } else if has("agent37.key") {
            "agent37".into()
        } else {
            self.providers.default_name().to_string()
        }
    }

    pub fn create_project(&self, n: NewProject) -> Result<Project> {
        let machine = n.machine;
        let mut repos: Vec<Repo> = vec![];
        for full in &n.repos {
            let full = full.trim().trim_end_matches(".git").to_string();
            let short = full.rsplit('/').next().unwrap_or(&full).to_string();
            if !full.contains('/') || repos.iter().any(|r| r.name == short) {
                continue;
            }
            repos.push(Repo {
                name: short,
                github: Some(full),
            });
        }
        let base = match n.name.trim() {
            "" => repos
                .first()
                .map(|r| r.name.clone())
                .ok_or_else(|| anyhow!("give the project a name or a repo"))?,
            name => name.to_string(),
        };
        if !base
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            bail!("a project name may use letters, digits, '-', '_' and '.'");
        }
        let mut name = base.clone();
        let mut i = 2;
        while self.db.project_by_name(&name)?.is_some() {
            name = format!("{base}-{i}");
            i += 1;
        }
        let provider = match machine {
            Some(m) => {
                self.db
                    .machine(m)?
                    .ok_or_else(|| anyhow!("no such machine"))?
                    .provider
            }
            None => n.provider.unwrap_or_else(|| self.default_provider()),
        };
        let mut env: Vec<String> = vec![];
        for e in n.env {
            let e = e.trim().to_string();
            if !e.is_empty() && !env.contains(&e) {
                env.push(e);
            }
        }
        let mut p = Project {
            id: new_id(),
            name,
            repos,
            default_machine: machine,
            provider: provider.clone(),
            kind: None,
            size: "4x8".into(),
            settings: AgentSettings {
                permission_mode: n.permission_mode,
                ..Default::default()
            },
            path: n
                .path
                .map(|p| p.trim().trim_end_matches('/').to_string())
                .filter(|p| !p.is_empty()),
            setup: n.setup.filter(|s| !s.trim().is_empty()),
            env,
            tools: n.tools.unwrap_or_else(default_tools),
            allow_main: n.allow_main,
            trusted: n.trusted,
            created_at: now_ms(),
        };
        self.place_project(&mut p, Some(provider), n.kind, n.size)?;
        self.db.insert_project(&p)?;
        self.publish_project(&p);
        Ok(p)
    }

    pub fn place_project(
        &self,
        p: &mut Project,
        provider: Option<String>,
        kind: Option<String>,
        size: Option<String>,
    ) -> Result<()> {
        let before = (
            p.provider.clone(),
            self.providers.kind_name(&p.provider, p.kind.as_deref()),
        );
        if let Some(pr) = provider {
            if self.providers.get(&pr).is_none() {
                bail!("unknown provider {pr}");
            }
            if pr != p.provider {
                p.provider = pr;
                p.kind = None;
            }
        }
        let kinds = self
            .providers
            .get(&p.provider)
            .map(|x| x.describe().kinds)
            .unwrap_or_default();
        if let Some(k) = kind.filter(|k| !k.is_empty()) {
            if !kinds.iter().any(|x| x.name == k) {
                let names: Vec<_> = kinds.iter().map(|x| x.name.as_str()).collect();
                bail!(
                    "{} has no kind called {k}; it has {}",
                    p.provider,
                    if names.is_empty() {
                        "none".to_string()
                    } else {
                        names.join(", ")
                    }
                );
            }
            p.kind = Some(k);
        }
        if p.kind.is_none() && !kinds.is_empty() && p.default_machine.is_none() {
            p.kind = kinds.first().map(|k| k.name.clone());
        }
        if let Some(sz) = size.filter(|s| !s.trim().is_empty()) {
            p.size = sz.trim().to_string();
        }
        if let Some(k) = self.providers.kind(&p.provider, p.kind.as_deref())
            && !k.sizes.is_empty()
        {
            p.size = crate::providers::fit(&k.sizes, &p.size);
        }
        let after = (
            p.provider.clone(),
            self.providers.kind_name(&p.provider, p.kind.as_deref()),
        );
        if after != before
            && self
                .providers
                .get(&p.provider)
                .is_some_and(|x| x.creates_machines())
        {
            p.default_machine = None;
        }
        Ok(())
    }

    pub fn project_cwd(p: &Project) -> String {
        p.path
            .clone()
            .unwrap_or_else(|| format!("~/projects/{}", p.name))
    }

    pub async fn create_thread(
        self: &Arc<Self>,
        project: ProjectId,
        agent: AgentKind,
        machine: Option<MachineId>,
        settings: AgentSettings,
        input: Vec<InputPart>,
    ) -> Result<Thread> {
        let p = self
            .db
            .project(project)?
            .ok_or_else(|| anyhow!("no such project"))?;
        let mut s = p.settings.clone();
        s.permission_mode = None;
        s.merge(&settings);
        let now = now_ms();
        let t = Thread {
            id: new_id(),
            project,
            title: title_from(&input),
            agent,
            machine: machine.or(p.default_machine),
            session_id: None,
            cwd: Self::project_cwd(&p),
            settings: s,
            status: ThreadStatus::Starting,
            last_seq: 0,
            created_at: now,
            updated_at: now,
            pinned_at: None,
            settled_at: None,
            hidden_at: None,
            started_by: None,
        };
        self.db.insert_thread(&t)?;
        self.publish_thread(&t);
        let hub = self.clone();
        let id = t.id;
        tokio::spawn(async move {
            if let Err(e) = hub.deliver(id, input, None).await {
                hub.hub_notice(id, NoticeLevel::Error, format!("Couldn't start: {e:#}"));
                let _ = hub.set_thread_status(id, ThreadStatus::Error);
                hub.tell_follower_text(id, &format!("couldn't start: {e:#}"));
            }
        });
        Ok(t)
    }

    pub fn queued(&self, thread: ThreadId) -> Vec<Queued> {
        self.thread_kv(thread, "queue")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn set_queued(&self, thread: ThreadId, items: &[Queued]) {
        if items.is_empty() {
            let _ = self.db.kv_delete(&format!("thread:{thread}:queue"));
        } else if let Ok(s) = serde_json::to_string(items) {
            self.set_thread_kv(thread, "queue", &s);
        }
        self.publish(Outbound::Thread(
            thread,
            HubToApp::Queue {
                thread,
                items: items.to_vec(),
            },
        ));
    }

    pub fn busy(&self, thread: ThreadId) -> bool {
        matches!(
            self.db.thread(thread),
            Ok(Some(t)) if matches!(
                t.status,
                ThreadStatus::Running | ThreadStatus::Waiting | ThreadStatus::Starting
            )
        )
    }

    pub fn enqueue(&self, thread: ThreadId, input: Vec<InputPart>, id: Option<String>) -> Queued {
        let item = Queued {
            id: id.unwrap_or_else(|| new_id().to_string()),
            input,
            at: now_ms(),
        };
        let mut items = self.queued(thread);
        items.push(item.clone());
        self.set_queued(thread, &items);
        item
    }

    pub fn first_send(&self, id: &str) -> bool {
        let mut sent = self.sent.lock().unwrap();
        if sent.iter().any(|x| x == id) {
            return false;
        }
        if sent.len() >= 1000 {
            sent.pop_front();
        }
        sent.push_back(id.to_string());
        true
    }

    pub fn enqueue_front(&self, thread: ThreadId, input: Vec<InputPart>, id: Option<String>) {
        let item = Queued {
            id: id.unwrap_or_else(|| new_id().to_string()),
            input,
            at: now_ms(),
        };
        let mut items = self.queued(thread);
        items.insert(0, item);
        self.set_queued(thread, &items);
    }

    pub fn unqueue(&self, thread: ThreadId, id: &str) -> Option<Queued> {
        let mut items = self.queued(thread);
        let at = items.iter().position(|q| q.id == id)?;
        let item = items.remove(at);
        self.set_queued(thread, &items);
        Some(item)
    }

    fn send_next_queued(&self, thread: ThreadId) {
        let mut items = self.queued(thread);
        if items.is_empty() {
            return;
        }
        let next = items.remove(0);
        self.set_queued(thread, &items);
        let Some(hub) = self.me.upgrade() else {
            return;
        };
        tokio::spawn(async move {
            let _ = hub.send(thread, next.input, None, false).await;
        });
    }

    pub async fn send(
        self: &Arc<Self>,
        thread: ThreadId,
        input: Vec<InputPart>,
        settings: Option<AgentSettings>,
        queue: bool,
    ) -> Result<()> {
        self.send_once(thread, input, settings, queue, None).await
    }

    pub async fn send_once(
        self: &Arc<Self>,
        thread: ThreadId,
        input: Vec<InputPart>,
        settings: Option<AgentSettings>,
        queue: bool,
        client_id: Option<String>,
    ) -> Result<()> {
        if let Some(id) = &client_id
            && !self.first_send(id)
        {
            return Ok(());
        }
        if queue && self.busy(thread) {
            self.enqueue(thread, input, client_id);
            return Ok(());
        }
        let hub = self.clone();
        tokio::spawn(async move {
            if let Err(e) = hub.deliver(thread, input, settings).await {
                hub.hub_notice(thread, NoticeLevel::Error, format!("Couldn't send: {e:#}"));
                let _ = hub.set_thread_status(thread, ThreadStatus::Error);
                hub.tell_follower_text(thread, &format!("couldn't take a message: {e:#}"));
            }
        });
        Ok(())
    }

    async fn deliver(
        self: &Arc<Self>,
        thread: ThreadId,
        input: Vec<InputPart>,
        settings: Option<AgentSettings>,
    ) -> Result<()> {
        let mut t = self
            .db
            .thread(thread)?
            .ok_or_else(|| anyhow!("no such thread"))?;
        if let Some(s) = &settings {
            t.settings.merge(s);
            self.db.update_thread(&t)?;
        }
        if !t.machine.is_some_and(|m| self.runner_connected(m)) {
            self.set_thread_status(thread, ThreadStatus::Starting)?;
        }
        let machine = self.ensure_machine(&mut t).await?;
        let failed_key = format!("setup_failed:{}", t.project);
        if let Ok(Some(log)) = self.db.kv_get(&failed_key) {
            let _ = self.db.kv_delete(&failed_key);
            self.hub_notice(
                thread,
                NoticeLevel::Warning,
                format!("The machine's setup failed, so it may be missing tools. The end of its log:\n\n```\n{log}\n```"),
            );
        }
        let p = self
            .db
            .project(t.project)?
            .ok_or_else(|| anyhow!("no such project"))?;
        let tools = crate::tools::defs_for(self, &p, machine);
        let agent_env = {
            let mut env = self.agent_env(t.agent)?;
            env.extend(self.project_env(&p));
            if p.trusted {
                env.extend(self.trusted_env());
            }
            env
        };
        let browser = p.tools.iter().any(|t| t == "browser");
        let mark = crate::follow::spec_mark(&tools, &agent_env, browser);
        let mut live = self.thread_live(machine, thread);
        let idle = matches!(t.status, ThreadStatus::Idle | ThreadStatus::Error);
        if live && idle && self.thread_kv(thread, "spec").as_deref() != Some(mark.as_str()) {
            self.send_runner(machine, HubMsg::Stop { thread })?;
            for _ in 0..30 {
                if !self.thread_live(machine, thread) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            live = false;
        }
        if live {
            self.send_runner(
                machine,
                HubMsg::Send {
                    thread,
                    input,
                    settings,
                },
            )?;
        } else {
            let spec = StartSpec {
                thread,
                agent: t.agent,
                cwd: t.cwd.clone(),
                extra_dirs: vec![],
                resume_session: t.session_id.clone(),
                settings: t.settings.clone(),
                next_seq: self.db.runner_seq(thread)? + 1,
                tools,
                repos: p.repos.clone(),
                agent_env,
                moved_from: self.take_thread_kv(thread, "moved_from"),
                fork_of: self
                    .thread_kv(thread, "fork_of")
                    .and_then(|s| serde_json::from_str(&s).ok()),
                resume_at: self.take_thread_kv(thread, "resume_at"),
                browser,
            };
            self.send_runner(
                machine,
                HubMsg::Start {
                    spec: Box::new(spec),
                    input: Some(input),
                },
            )?;
            self.set_thread_kv(thread, "spec", &mark);
            if let Some(c) = self.runners.lock().unwrap().get_mut(&machine) {
                c.live.insert(thread);
            }
        }
        Ok(())
    }

    fn thread_live(&self, machine: MachineId, thread: ThreadId) -> bool {
        self.runners
            .lock()
            .unwrap()
            .get(&machine)
            .is_some_and(|c| c.live.contains(&thread))
    }

    async fn ensure_machine(self: &Arc<Self>, t: &mut Thread) -> Result<MachineId> {
        if let Some(m) = t.machine {
            if self.runner_connected(m) {
                self.prepare_paired(t.project, m).await?;
                return Ok(m);
            }
            let machine = self.db.machine(m)?;
            if let Some(machine) = machine
                && machine.provider == "existing"
            {
                bail!("{} is offline", machine.name);
            }
        }
        let p = self
            .db
            .project(t.project)?
            .ok_or_else(|| anyhow!("no such project"))?;
        let m = self.providers.start_for_project(self, &p).await?;
        self.prepare_paired(p.id, m).await?;
        if t.machine != Some(m) {
            t.machine = Some(m);
            self.db.reset_runner_seq(t.id)?;
            self.db.update_thread(t)?;
            self.publish_thread(t);
        }
        Ok(m)
    }

    async fn prepare_paired(&self, project: ProjectId, machine: MachineId) -> Result<()> {
        if !matches!(self.db.machine(machine)?, Some(m) if m.provider == "existing") {
            return Ok(());
        }
        let done = self
            .runners
            .lock()
            .unwrap()
            .get(&machine)
            .is_some_and(|c| c.prepared.contains(&project));
        if done {
            return Ok(());
        }
        let p = self
            .db
            .project(project)?
            .ok_or_else(|| anyhow!("no such project"))?;
        crate::kit::prepare(self, &p, machine).await?;
        if let Some(c) = self.runners.lock().unwrap().get_mut(&machine) {
            c.prepared.insert(project);
        }
        Ok(())
    }

    fn agent_env(&self, agent: AgentKind) -> Result<Vec<(String, String)>> {
        let mut env = vec![];
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

    pub fn stop_task(&self, thread: ThreadId, tool: String) -> Result<()> {
        let t = self
            .db
            .thread(thread)?
            .ok_or_else(|| anyhow!("no such thread"))?;
        let m = t.machine.ok_or_else(|| anyhow!("thread has no machine"))?;
        self.send_runner(m, HubMsg::StopTask { thread, tool })
    }

    pub fn background_gone(&self, thread: ThreadId) {
        let running = self
            .live_status
            .lock()
            .unwrap()
            .get(&thread)
            .is_some_and(|s| {
                s.get("agents")
                    .and_then(|a| a.as_array())
                    .is_some_and(|a| !a.is_empty())
                    || s.get("background").and_then(|b| b.as_i64()).unwrap_or(0) > 0
            });
        if running {
            self.set_live_status(thread, serde_json::json!({ "agents": [], "background": 0 }));
        }
    }

    pub fn interrupt(&self, thread: ThreadId) -> Result<()> {
        let t = self
            .db
            .thread(thread)?
            .ok_or_else(|| anyhow!("no such thread"))?;
        let m = t.machine.ok_or_else(|| anyhow!("thread has no machine"))?;
        self.send_runner(m, HubMsg::Interrupt { thread })
    }

    pub fn respond(
        &self,
        thread: ThreadId,
        request: String,
        response: RequestResponse,
    ) -> Result<()> {
        if let Some(tx) = self.secret_requests.lock().unwrap().remove(&request) {
            let response = match response {
                RequestResponse::Allow { .. } => RequestResponse::Cancelled,
                other => other,
            };
            self.append(
                thread,
                None,
                now_ms(),
                Event::RequestResolved {
                    id: request,
                    response,
                },
            )?;
            let _ = tx.send(None);
            return Ok(());
        }
        if let Some(tx) = self.hub_requests.lock().unwrap().remove(&request) {
            self.append(
                thread,
                None,
                now_ms(),
                Event::RequestResolved {
                    id: request,
                    response: response.clone(),
                },
            )?;
            let _ = tx.send(response);
            return Ok(());
        }
        let t = self
            .db
            .thread(thread)?
            .ok_or_else(|| anyhow!("no such thread"))?;
        let m = t.machine.ok_or_else(|| anyhow!("thread has no machine"))?;
        let waiting = open_requests(&self.db.events_tail(thread, 2000)?).contains(&request);
        {
            let mut answered = self.answered.lock().unwrap();
            answered.retain(|_, at| at.elapsed() < Duration::from_secs(60));
            if !waiting || answered.contains_key(&request) {
                bail!("That request is no longer waiting.");
            }
            answered.insert(request.clone(), std::time::Instant::now());
        }
        let sent = self.send_runner(
            m,
            HubMsg::Respond {
                thread,
                request: request.clone(),
                response,
            },
        );
        if sent.is_err() {
            self.answered.lock().unwrap().remove(&request);
        }
        sent
    }

    pub fn update_settings(&self, thread: ThreadId, settings: AgentSettings) -> Result<()> {
        let mut t = self
            .db
            .thread(thread)?
            .ok_or_else(|| anyhow!("no such thread"))?;
        t.settings.merge(&settings);
        self.db.update_thread(&t)?;
        self.publish_thread(&t);
        if let Some(m) = t.machine {
            let live = self
                .runners
                .lock()
                .unwrap()
                .get(&m)
                .map(|c| c.live.contains(&thread))
                .unwrap_or(false);
            if live {
                self.send_runner(m, HubMsg::Settings { thread, settings })?;
            }
        }
        Ok(())
    }

    pub async fn machine_for_thread(&self, thread: ThreadId) -> Result<(Thread, MachineId)> {
        let t = self
            .db
            .thread(thread)?
            .ok_or_else(|| anyhow!("no such thread"))?;
        let m = t.machine.ok_or_else(|| anyhow!("thread has no machine"))?;
        if !self.runner_connected(m) {
            bail!("the thread's machine isn't running");
        }
        Ok((t, m))
    }

    pub fn thread_kv(&self, thread: ThreadId, key: &str) -> Option<String> {
        self.db
            .kv_get(&format!("thread:{thread}:{key}"))
            .ok()
            .flatten()
    }

    pub fn mark_started_by(&self, thread: ThreadId, parent: ThreadId) {
        self.set_thread_kv(thread, "started_by", &parent.to_string());
        if let Ok(Some(t)) = self.db.thread(thread) {
            self.publish_thread(&t);
        }
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

    pub fn import_threads(&self, machine: MachineId, batch: Vec<ImportThread>) -> Result<usize> {
        let mut imported = 0;
        for item in batch {
            let mut t = item.thread;
            if let Some(existing) = self.db.thread(t.id)?
                && existing.machine != Some(machine)
            {
                continue;
            }
            let projects = self.db.projects()?;
            let mine = |p: &&Project| p.default_machine == Some(machine);
            let folder =
                |p: &Option<String>| p.as_deref().map(|x| x.trim_end_matches('/').to_string());
            let p = match match folder(&item.project.path) {
                Some(path) => projects
                    .iter()
                    .filter(mine)
                    .find(|p| folder(&p.path).as_deref() == Some(path.as_str())),
                None => projects
                    .iter()
                    .filter(mine)
                    .find(|p| p.name == item.project.name),
            } {
                Some(p) => p.clone(),
                None => {
                    let mut name = item.project.name.clone();
                    let mut n = 2;
                    while projects.iter().any(|p| p.name == name) {
                        name = format!("{}-{n}", item.project.name);
                        n += 1;
                    }
                    let p = Project {
                        id: new_id(),
                        name,
                        repos: vec![],
                        default_machine: Some(machine),
                        provider: "existing".into(),
                        kind: None,
                        size: "existing".into(),
                        settings: item.project.settings.clone(),
                        path: item.project.path.clone(),
                        setup: None,
                        env: vec![],
                        tools: default_tools(),
                        allow_main: false,
                        trusted: false,
                        created_at: now_ms(),
                    };
                    self.db.insert_project(&p)?;
                    self.publish_project(&p);
                    p
                }
            };
            t.project = p.id;
            t.machine = Some(machine);
            if matches!(
                t.status,
                ThreadStatus::Running | ThreadStatus::Waiting | ThreadStatus::Starting
            ) {
                t.status = ThreadStatus::Idle;
            }
            self.db.upsert_imported_thread(&t)?;
            let fresh = self.db.insert_imported_events(t.id, &item.events)?;
            if let Some(stored) = self.db.thread(t.id)? {
                self.publish_thread(&stored);
            }
            if !fresh.is_empty() {
                self.publish(Outbound::Thread(
                    t.id,
                    HubToApp::Events {
                        thread: t.id,
                        events: fresh,
                    },
                ));
            }
            imported += 1;
        }
        Ok(imported)
    }

    pub fn retention_days(&self) -> Option<i64> {
        let v = &self.settings()["retention_days"];
        v.as_i64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            .filter(|d| *d > 0)
    }

    pub fn sweep_hidden(&self, now: i64) -> Result<usize> {
        let Some(days) = self.retention_days() else {
            return Ok(0);
        };
        let threads = self.db.threads()?;
        let needed: HashSet<ThreadId> = threads
            .iter()
            .filter(|t| t.session_id.is_none())
            .filter_map(|t| self.thread_kv(t.id, "fork_of"))
            .filter_map(|s| serde_json::from_str::<ForkSource>(&s).ok()?.thread)
            .collect();
        let mut n = 0;
        for t in threads {
            let old = t.hidden_at.is_some_and(|at| now - at > days * 86_400_000);
            if old && !needed.contains(&t.id) {
                self.stop_live(&t);
                self.remove_thread(t.id)?;
                n += 1;
            }
        }
        Ok(n)
    }

    pub fn remove_thread(&self, thread: ThreadId) -> Result<()> {
        self.db.delete_thread(thread)?;
        self.live_status.lock().unwrap().remove(&thread);
        self.publish(Outbound::All(HubToApp::ThreadRemoved { thread }));
        Ok(())
    }

    pub fn stop_live(&self, t: &Thread) {
        if let Some(m) = t.machine {
            let live = self
                .runners
                .lock()
                .unwrap()
                .get_mut(&m)
                .map(|c| c.live.remove(&t.id))
                .unwrap_or(false);
            if live {
                let _ = self.send_runner(m, HubMsg::Stop { thread: t.id });
            }
        }
    }

    pub async fn move_thread(
        self: &Arc<Self>,
        thread: ThreadId,
        target: Option<MachineId>,
    ) -> Result<Thread> {
        let mut t = self
            .db
            .thread(thread)?
            .ok_or_else(|| anyhow!("no such thread"))?;
        if matches!(
            t.status,
            ThreadStatus::Running | ThreadStatus::Waiting | ThreadStatus::Starting
        ) {
            bail!("the agent is working; stop it or wait for it to finish, then move the thread");
        }
        if target.is_some() && target == t.machine {
            return Ok(t);
        }
        let p = self
            .db
            .project(t.project)?
            .ok_or_else(|| anyhow!("no such project"))?;
        if let Some(m) = target {
            let tm = self
                .db
                .machine(m)?
                .ok_or_else(|| anyhow!("no such machine"))?;
            if !self.runner_connected(m) {
                bail!("{} is offline", tm.name);
            }
        } else {
            let provider = self
                .providers
                .get(&p.provider)
                .ok_or_else(|| anyhow!("unknown provider {}", p.provider))?;
            if !provider.creates_machines() {
                bail!(
                    "project {} has no provider that makes machines; pick one in Settings, or choose a machine",
                    p.name
                );
            }
        }
        let from_cwd = t.cwd.clone();
        if let Some(m) = t.machine
            && self.runner_connected(m)
        {
            self.stop_live(&t);
            tokio::time::sleep(Duration::from_millis(1500)).await;
            self.runner_request_timeout(
                m,
                RunnerRequest::SaveWork {
                    cwd: t.cwd.clone(),
                    thread,
                },
                Duration::from_secs(120),
            )
            .await
            .map_err(|e| {
                anyhow!("couldn't save the thread's work on its machine, so it stayed there: {e:#}")
            })?;
        }
        t = self
            .db
            .thread(thread)?
            .ok_or_else(|| anyhow!("no such thread"))?;
        t.machine = target;
        t.status = ThreadStatus::Idle;
        t.updated_at = now_ms();
        t.cwd = match target {
            Some(m) if p.default_machine == Some(m) => Self::project_cwd(&p),
            _ => format!("~/projects/{}", p.name),
        };
        self.db.reset_runner_seq(thread)?;
        self.db.update_thread(&t)?;
        self.publish_thread(&t);
        if from_cwd != t.cwd {
            self.set_thread_kv(thread, "moved_from", &from_cwd);
        }
        Ok(t)
    }

    pub async fn fork_thread(
        self: &Arc<Self>,
        thread: ThreadId,
        turn: Option<String>,
    ) -> Result<Thread> {
        let t = self
            .db
            .thread(thread)?
            .ok_or_else(|| anyhow!("no such thread"))?;
        let session = t
            .session_id
            .clone()
            .ok_or_else(|| anyhow!("this thread hasn't started a session yet"))?;
        let marks = self.db.turn_marks(thread)?;
        let (seq, anchor) = turn_end(&marks, turn.as_deref())
            .ok_or_else(|| anyhow!("that turn hasn't finished"))?;
        let now = now_ms();
        let n = Thread {
            id: new_id(),
            title: format!("{} (fork)", t.title.trim_end_matches(" (fork)")),
            session_id: None,
            status: ThreadStatus::Idle,
            last_seq: 0,
            created_at: now,
            updated_at: now,
            pinned_at: None,
            settled_at: None,
            hidden_at: None,
            ..t.clone()
        };
        self.db.insert_thread(&n)?;
        self.db.copy_events(thread, n.id, seq)?;
        self.set_thread_kv(
            n.id,
            "fork_of",
            &serde_json::to_string(&ForkSource {
                session,
                at_message: anchor,
                thread: Some(thread),
            })?,
        );
        let n = self
            .db
            .thread(n.id)?
            .ok_or_else(|| anyhow!("fork failed"))?;
        self.publish_thread(&n);
        Ok(n)
    }

    pub fn rewind_point(
        &self,
        thread: ThreadId,
        turn: &str,
    ) -> Result<(i64, Option<String>, Vec<String>)> {
        let marks = self.db.turn_marks(thread)?;
        let (seq, anchor) =
            turn_before(&marks, turn).ok_or_else(|| anyhow!("can't rewind to that turn"))?;
        let turns = marks
            .iter()
            .filter(|e| e.seq > seq)
            .filter_map(|e| match &e.event {
                Event::TurnStarted { turn, .. } => Some(turn.clone()),
                _ => None,
            })
            .collect();
        Ok((seq, anchor, turns))
    }

    pub async fn rewind_thread(self: &Arc<Self>, thread: ThreadId, turn: String) -> Result<()> {
        let t = self
            .db
            .thread(thread)?
            .ok_or_else(|| anyhow!("no such thread"))?;
        if matches!(
            t.status,
            ThreadStatus::Running | ThreadStatus::Waiting | ThreadStatus::Starting
        ) {
            bail!("stop the agent first");
        }
        let (seq, anchor, turns) = self.rewind_point(thread, &turn)?;
        let mut t = t;
        let machine = match t.machine {
            Some(m) if self.runner_connected(m) => m,
            _ => self.ensure_machine(&mut t).await?,
        };
        self.stop_live(&t);
        self.runner_request_timeout(
            machine,
            RunnerRequest::Rewind {
                cwd: t.cwd.clone(),
                thread,
                turn: turn.clone(),
                turns,
            },
            Duration::from_secs(60),
        )
        .await?;
        self.db.delete_events_after(thread, seq)?;
        match anchor {
            Some(a) => self.set_thread_kv(thread, "resume_at", &a),
            None => {
                let mut t2 = self
                    .db
                    .thread(thread)?
                    .ok_or_else(|| anyhow!("no such thread"))?;
                t2.session_id = None;
                self.db.update_thread(&t2)?;
            }
        }
        if let Some(t2) = self.db.thread(thread)? {
            self.publish_thread(&t2);
        }
        self.publish(Outbound::All(HubToApp::Reset { thread }));
        Ok(())
    }
}

pub fn unanswered_hub_requests(events: &[StoredEvent]) -> Vec<String> {
    let mut open = open_requests(events);
    open.retain(|id| id.starts_with("hub-"));
    open
}

pub fn open_requests(events: &[StoredEvent]) -> Vec<String> {
    let mut open: Vec<String> = vec![];
    for e in events {
        match &e.event {
            Event::Request { id, .. } => open.push(id.clone()),
            Event::RequestResolved { id, .. } => open.retain(|x| x != id),
            _ => {}
        }
    }
    open
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

fn turn_end(events: &[StoredEvent], turn: Option<&str>) -> Option<(i64, Option<String>)> {
    events.iter().rev().find_map(|e| match &e.event {
        Event::TurnCompleted {
            turn: t, anchor, ..
        } if turn.is_none() || turn == Some(t.as_str()) => Some((e.seq, anchor.clone())),
        _ => None,
    })
}

fn turn_before(events: &[StoredEvent], turn: &str) -> Option<(i64, Option<String>)> {
    let start = events
        .iter()
        .find(|e| matches!(&e.event, Event::TurnStarted { turn: t, .. } if t == turn))?
        .seq;
    let prev = events.iter().rev().find_map(|e| match &e.event {
        Event::TurnCompleted { anchor, .. } if e.seq < start => Some(anchor.clone()),
        _ => None,
    });
    Some((start - 1, prev.flatten()))
}
