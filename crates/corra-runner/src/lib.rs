pub mod agents;
pub mod files;
pub mod git;
pub mod kit;
#[cfg(target_os = "linux")]
pub mod reaper;
pub mod secrets;
pub mod sync;
pub mod term;
mod update;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, anyhow};
use corra_proto::runner::{HubMsg, RunnerMsg, RunningThread};
use corra_proto::*;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

use crate::agents::{AgentOut, HubCall, SessionCmd, SessionCtx};

pub struct Options {
    pub hub: String,
    pub token: String,
    pub sync: bool,
    pub update: bool,
    pub connected: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

#[derive(Clone)]
struct OutTx(mpsc::UnboundedSender<term::OutItem>);

impl OutTx {
    fn send(&self, msg: RunnerMsg) {
        let _ = self.0.send((msg, None));
    }

    fn send_item(&self, item: term::OutItem) {
        let _ = self.0.send(item);
    }
}

struct Session {
    cmds: mpsc::UnboundedSender<SessionCmd>,
    status: ThreadStatus,
    next_seq: i64,
    agent: AgentKind,
    cwd: PathBuf,
    session_id: Option<String>,
    baseline: std::sync::Arc<tokio::sync::Mutex<Vec<(String, String)>>>,
    spec: StartSpec,
    idle_since: Option<std::time::Instant>,
    parking: bool,
    stopping: bool,
    held: Vec<(Vec<InputPart>, Option<AgentSettings>, Option<String>)>,
    restart: Option<StartSpec>,
}

fn park_after() -> Duration {
    let minutes = std::env::var("CORRA_PARK_MINUTES")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(30);
    Duration::from_secs(minutes * 60)
}

fn spec_mark(tools: &[ToolDef], env: &[(String, String)]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(serde_json::to_vec(&(tools, env)).unwrap_or_default());
    h.finalize().into()
}

fn spec_changed(cur: &StartSpec, next: &StartSpec) -> bool {
    cur.agent != next.agent
        || files::expand(&cur.cwd) != files::expand(&next.cwd)
        || next.resume_at.is_some()
        || next.moved_from.is_some()
        || spec_mark(&cur.tools, &cur.agent_env) != spec_mark(&next.tools, &next.agent_env)
}

#[derive(Default)]
struct Outbox {
    pending: BTreeMap<ThreadId, VecDeque<(i64, i64, Event)>>,
}

impl Outbox {
    fn push(&mut self, thread: ThreadId, seq: i64, ts: i64, event: Event) {
        self.pending
            .entry(thread)
            .or_default()
            .push_back((seq, ts, event));
    }
    fn ack(&mut self, thread: ThreadId, seq: i64) {
        if let Some(q) = self.pending.get_mut(&thread) {
            while q.front().is_some_and(|(s, _, _)| *s <= seq) {
                q.pop_front();
            }
            if q.is_empty() {
                self.pending.remove(&thread);
            }
        }
    }
}

struct Runner {
    sessions: HashMap<ThreadId, Session>,
    outbox: Outbox,
    ended_while_offline: HashMap<ThreadId, Option<String>>,
    stop_waiters: HashMap<ThreadId, Vec<oneshot::Sender<()>>>,
    agent_tx: mpsc::UnboundedSender<(ThreadId, AgentOut)>,
    hub_tx: mpsc::UnboundedSender<HubCall>,
    calls: HashMap<u64, oneshot::Sender<Result<serde_json::Value, String>>>,
    next_call: u64,
    sync: Option<sync::SyncRef>,
    hub_url: String,
    token: String,
    git_token: Option<String>,
    cached_agents: Option<Vec<AgentAvailability>>,
    terms: term::Terms,
    bg_tx: mpsc::UnboundedSender<term::OutItem>,
    parked: HashMap<ThreadId, StartSpec>,
    own_build: Option<String>,
    newer: Option<String>,
    fetching: Option<tokio::task::JoinHandle<Result<std::path::PathBuf>>>,
    fetched: Option<std::path::PathBuf>,
    switch_at: Option<tokio::time::Instant>,
    switch_soon: bool,
    took: VecDeque<String>,
    connected: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

pub async fn run(cli: Options) -> Result<()> {
    let token = cli.token.clone();
    let url = runner_url(&cli.hub);

    let (agent_tx, mut agent_rx) = mpsc::unbounded_channel();
    let (hub_tx, mut hub_rx) = mpsc::unbounded_channel::<HubCall>();
    let (bg_tx, mut bg_rx) = mpsc::unbounded_channel::<term::OutItem>();
    let mut runner = Runner {
        sessions: HashMap::new(),
        outbox: Outbox::default(),
        ended_while_offline: HashMap::new(),
        stop_waiters: HashMap::new(),
        agent_tx,
        hub_tx,
        calls: HashMap::new(),
        next_call: 1,
        sync: cli.sync.then(|| sync::Syncer::new(&cli.hub, &token)),
        hub_url: cli.hub.clone(),
        token: token.clone(),
        git_token: None,
        cached_agents: None,
        terms: term::Terms::default(),
        parked: HashMap::new(),
        own_build: if cli.update {
            update::own_build()
        } else {
            None
        },
        newer: None,
        fetching: None,
        fetched: None,
        switch_at: None,
        switch_soon: false,
        took: Default::default(),
        connected: cli.connected.clone(),
        bg_tx,
    };
    let mut sync_tick = tokio::time::interval(Duration::from_secs(120));
    let mut backoff = Duration::from_millis(500);

    loop {
        match connect(&url, &token).await {
            Ok(ws) => {
                backoff = Duration::from_millis(500);
                tracing::info!("connected to {url}");
                if let Err(e) = run_connection(
                    &mut runner,
                    ws,
                    &mut agent_rx,
                    &mut hub_rx,
                    &mut sync_tick,
                    &mut bg_rx,
                )
                .await
                {
                    tracing::warn!("connection lost: {e:#}");
                }
                runner.set_connected(false);
            }
            Err(e) => tracing::warn!("can't reach the hub: {e:#}"),
        }
        let wait = tokio::time::sleep(backoff);
        tokio::pin!(wait);
        loop {
            tokio::select! {
                _ = &mut wait => break,
                Some((thread, out)) = agent_rx.recv() => {
                    runner.on_agent(thread, out, None);
                }
                Some(c) = hub_rx.recv() => {
                    let _ = c.reply.send(Err("corra's hub is unreachable right now".into()));
                }
                Some(_) = bg_rx.recv() => {}
            }
        }
        backoff = (backoff * 2).min(Duration::from_secs(15));
    }
}

fn runner_url(hub: &str) -> String {
    let base = hub.trim_end_matches('/');
    let base = if let Some(r) = base.strip_prefix("https://") {
        format!("wss://{r}")
    } else if let Some(r) = base.strip_prefix("http://") {
        format!("ws://{r}")
    } else {
        base.to_string()
    };
    format!("{base}/runner")
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(url: &str, token: &str) -> Result<Ws> {
    let mut req = url.into_client_request()?;
    req.headers_mut()
        .insert("authorization", format!("Bearer {token}").parse()?);
    let (ws, _) = tokio::time::timeout(
        Duration::from_secs(15),
        tokio_tungstenite::connect_async(req),
    )
    .await
    .map_err(|_| anyhow!("timed out"))??;
    Ok(ws)
}

pub fn machine_info() -> MachineInfo {
    machine_info_with_cache(None, update::own_build())
}

fn machine_info_with_cache(
    cached: Option<Vec<AgentAvailability>>,
    runner_build: Option<String>,
) -> MachineInfo {
    let hostname = std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let os = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| {
            s.lines().find_map(|l| {
                l.strip_prefix("PRETTY_NAME=")
                    .map(|v| v.trim_matches('"').to_string())
            })
        })
        .unwrap_or_else(|| std::env::consts::OS.to_string());
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1);
    let memory_mb = memory_limit_mb();
    let agents = detect_agents(cached.as_deref());
    MachineInfo {
        hostname,
        os,
        cpus,
        memory_mb,
        runner_version: env!("CARGO_PKG_VERSION").to_string(),
        runner_build,
        home: files::home().to_string_lossy().to_string(),
        agents,
        disk_used_mb: disk_used_mb(),
        kvm: kvm(),
    }
}

fn detect_agents(cached: Option<&[AgentAvailability]>) -> Vec<AgentAvailability> {
    let cached_version = |kind: AgentKind| -> Option<String> {
        cached
            .into_iter()
            .flatten()
            .find(|a| a.kind == kind)
            .and_then(|a| a.version.clone())
    };
    let mut agents = vec![];
    let claude_v =
        cached_version(AgentKind::Claude).or_else(|| command_version(&agents::claude::find_bin()));
    if let Some(v) = claude_v {
        agents.push(AgentAvailability {
            kind: AgentKind::Claude,
            version: Some(v),
        });
    }
    let agy_v = match agents::antigravity::installed() {
        Some(p) => {
            cached_version(AgentKind::Antigravity).or_else(|| command_version(&p.to_string_lossy()))
        }
        None => None,
    };
    agents.push(AgentAvailability {
        kind: AgentKind::Antigravity,
        version: agy_v,
    });
    agents
}

fn kvm() -> bool {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/kvm")
        .is_ok()
}

fn disk_used_mb() -> u64 {
    let Ok(path) = std::ffi::CString::new("/") else {
        return 0;
    };
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut st) } != 0 {
        return 0;
    }
    ((st.f_blocks - st.f_bfree) as u64 * st.f_frsize as u64) / (1 << 20)
}

fn memory_limit_mb() -> u64 {
    let cg = std::fs::read_to_string("/sys/fs/cgroup/memory.max")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok());
    let total = std::fs::read_to_string("/proc/meminfo").ok().and_then(|s| {
        s.lines()
            .find_map(|l| l.strip_prefix("MemTotal:"))
            .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
            .map(|kb| kb * 1024)
    });
    match (cg, total) {
        (Some(c), Some(t)) => c.min(t) / (1 << 20),
        (Some(c), None) => c / (1 << 20),
        (None, Some(t)) => t / (1 << 20),
        _ => 0,
    }
}

fn command_version(cmd: &str) -> Option<String> {
    let mut child = std::process::Command::new(cmd)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(None) if start.elapsed() < std::time::Duration::from_secs(5) => {
                std::thread::sleep(std::time::Duration::from_millis(50))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    std::io::Read::read_to_string(child.stdout.as_mut()?, &mut out).ok()?;
    Some(out.trim().to_string())
}

async fn run_connection(
    runner: &mut Runner,
    ws: Ws,
    agent_rx: &mut mpsc::UnboundedReceiver<(ThreadId, AgentOut)>,
    hub_rx: &mut mpsc::UnboundedReceiver<HubCall>,
    sync_tick: &mut tokio::time::Interval,
    bg_rx: &mut mpsc::UnboundedReceiver<term::OutItem>,
) -> Result<()> {
    let (mut sink, mut stream) = ws.split();
    let (raw_tx, mut out_rx) = mpsc::unbounded_channel::<term::OutItem>();
    let out_tx = OutTx(raw_tx);

    let cached = runner.cached_agents.clone();
    let own_build = runner.own_build.clone().or_else(update::own_build);
    let info = tokio::task::spawn_blocking(move || machine_info_with_cache(cached, own_build))
        .await
        .map_err(|e| anyhow!("machine info failed: {e}"))?;
    runner.cached_agents = Some(info.agents.clone());

    let hello = RunnerMsg::Hello {
        protocol: PROTOCOL_VERSION,
        info,
        threads: runner.hello_threads(),
        takes: true,
        prompt: true,
    };
    sink.send(Message::Text(serde_json::to_string(&hello)?.into()))
        .await?;

    let writer = tokio::spawn(async move {
        while let Some((msg, _permit)) = out_rx.recv().await {
            let Ok(text) = serde_json::to_string(&msg) else {
                continue;
            };
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    let mut welcomed = false;
    let result: Result<()> = loop {
        tokio::select! {
            msg = stream.next() => {
                let Some(msg) = msg else { break Ok(()) };
                let text = match msg {
                    Ok(Message::Text(t)) => t,
                    Ok(Message::Close(_)) => break Ok(()),
                    Ok(_) => continue,
                    Err(e) => break Err(e.into()),
                };
                let msg: HubMsg = match serde_json::from_str(&text) {
                    Ok(m) => m,
                    Err(e) => { tracing::warn!("bad hub message: {e}"); continue; }
                };
                if let HubMsg::Welcome { acks, runner: build, git_token, .. } = &msg {
                    runner.git_token = git_token.clone();
                    runner.newer = build
                        .clone()
                        .filter(|b| runner.own_build.as_ref().is_some_and(|own| own != b));
                    for (t, s) in acks { runner.outbox.ack(*t, *s); }
                    for (thread, q) in &runner.outbox.pending {
                        for (seq, ts, event) in q {
                            out_tx.send(RunnerMsg::Event { thread: *thread, seq: *seq, ts: *ts, event: event.clone() });
                        }
                    }
                    for (id, s) in &runner.sessions {
                        out_tx.send(RunnerMsg::ThreadStatus { thread: *id, status: s.status });
                    }
                    let ended: Vec<(ThreadId, Option<String>)> = runner
                        .ended_while_offline
                        .drain()
                        .filter(|(id, _)| !runner.sessions.contains_key(id))
                        .collect();
                    for (thread, error) in ended {
                        out_tx.send(RunnerMsg::ThreadEnded { thread, error });
                    }
                    welcomed = true;
                    runner.set_connected(true);
                    continue;
                }
                runner.on_hub(msg, &out_tx);
            }
            Some((thread, out)) = agent_rx.recv() => {
                runner.on_agent(thread, out, welcomed.then_some(&out_tx));
            }
            Some(item) = bg_rx.recv() => {
                if welcomed { out_tx.send_item(item); }
            }
            _ = sync_tick.tick() => {
                let busy: Vec<ThreadId> = runner
                    .sessions
                    .iter()
                    .filter(|(_, s)| matches!(s.status, ThreadStatus::Running | ThreadStatus::Waiting))
                    .map(|(id, _)| *id)
                    .collect();
                for t in busy { runner.sync_thread(t); }
                if welcomed {
                    for (id, s) in &runner.sessions {
                        out_tx.send(RunnerMsg::ThreadStatus { thread: *id, status: s.status });
                    }
                }
                runner.park_idle();
                runner.update();
            }
            fetched = async { runner.fetching.as_mut().unwrap().await }, if runner.fetching.is_some() => {
                runner.fetching = None;
                match fetched {
                    Ok(Ok(next)) => {
                        runner.fetched = Some(next);
                        runner.update();
                    }
                    Ok(Err(e)) => {
                        tracing::warn!("couldn't update the runner: {e:#}");
                        runner.newer = None;
                        runner.switch_soon = false;
                    }
                    Err(e) => {
                        tracing::warn!("the runner's update stopped: {e}");
                        runner.switch_soon = false;
                    }
                }
            }
            _ = async { tokio::time::sleep_until(runner.switch_at.unwrap()).await }, if runner.switch_at.is_some() => {
                runner.switch();
            }
            Some(c) = hub_rx.recv() => {
                let id = runner.next_call;
                runner.next_call += 1;
                runner.calls.insert(id, c.reply);
                out_tx.send(RunnerMsg::Call { id, thread: c.thread, turn: c.turn, action: c.action, input: c.input });
            }
        }
        if writer.is_finished() {
            break Err(anyhow!("writer stopped"));
        }
    };
    writer.abort();
    for (_, reply) in runner.calls.drain() {
        let _ = reply.send(Err("lost the connection to corra's hub".into()));
    }
    result
}

impl Runner {
    fn git_token(&self) -> String {
        self.git_token.clone().unwrap_or_else(|| self.token.clone())
    }

    fn hello_threads(&self) -> Vec<RunningThread> {
        let mut seen: HashSet<ThreadId> = HashSet::new();
        let mut threads: Vec<RunningThread> = self
            .sessions
            .iter()
            .map(|(id, s)| {
                seen.insert(*id);
                RunningThread {
                    thread: *id,
                    last_seq: s.next_seq - 1,
                    status: s.status,
                }
            })
            .collect();
        for thread in self
            .outbox
            .pending
            .keys()
            .chain(self.ended_while_offline.keys())
        {
            if !seen.insert(*thread) {
                continue;
            }
            let last_seq = self
                .outbox
                .pending
                .get(thread)
                .and_then(|q| q.back().map(|(s, _, _)| *s))
                .unwrap_or(0);
            threads.push(RunningThread {
                thread: *thread,
                last_seq,
                status: ThreadStatus::Idle,
            });
        }
        threads
    }

    fn held_contains(&self, thread: ThreadId, delivery: &Option<String>) -> bool {
        let Some(d) = delivery else {
            return false;
        };
        self.sessions
            .get(&thread)
            .is_some_and(|s| s.held.iter().any(|(_, _, h)| h.as_deref() == Some(d)))
    }

    fn on_hub(&mut self, msg: HubMsg, out: &OutTx) {
        match msg {
            HubMsg::Welcome { .. } => {}
            HubMsg::Ack { thread, seq } => self.outbox.ack(thread, seq),
            HubMsg::Prompt {
                spec,
                input,
                settings,
                delivery,
            } => {
                self.switch_at = None;
                if self.seen(&delivery, out) {
                    return;
                }
                let thread = spec.thread;
                self.parked.remove(&thread);
                if self.held_contains(thread, &delivery) {
                    return;
                }
                match self.sessions.get_mut(&thread) {
                    Some(s) if s.parking || s.stopping => {
                        s.held.push((input, settings, delivery));
                        s.restart = Some(*spec);
                    }
                    Some(s)
                        if matches!(s.status, ThreadStatus::Idle | ThreadStatus::Error)
                            && spec_changed(&s.spec, &spec) =>
                    {
                        s.stopping = true;
                        s.parking = false;
                        s.held.push((input, settings, delivery));
                        s.restart = Some(*spec);
                        let _ = s.cmds.send(SessionCmd::Stop);
                    }
                    Some(s) => {
                        s.idle_since = None;
                        let _ = s.cmds.send(SessionCmd::Send { input, settings });
                        self.took(delivery, true, Some(out));
                    }
                    None => {
                        self.start(*spec, Some(input), out);
                        self.took(delivery, true, Some(out));
                    }
                }
            }
            HubMsg::Start {
                spec,
                input,
                delivery,
            } => {
                self.switch_at = None;
                if self.seen(&delivery, out) {
                    return;
                }
                let thread = spec.thread;
                self.parked.remove(&thread);
                if self.held_contains(thread, &delivery) {
                    return;
                }
                if let Some(s) = self.sessions.get_mut(&thread)
                    && (s.parking || s.stopping)
                {
                    if let Some(input) = input {
                        s.held.push((input, Some(spec.settings.clone()), delivery));
                    }
                    s.restart = Some(*spec);
                    return;
                }
                self.start(*spec, input, out);
                self.took(delivery, true, Some(out));
            }
            HubMsg::Send {
                thread,
                input,
                settings,
                delivery,
            } => {
                self.switch_at = None;
                if self.seen(&delivery, out) || self.held_contains(thread, &delivery) {
                    return;
                }
                match self.sessions.get_mut(&thread) {
                    Some(s) if s.parking || s.stopping => s.held.push((input, settings, delivery)),
                    Some(s) => {
                        s.idle_since = None;
                        let _ = s.cmds.send(SessionCmd::Send { input, settings });
                        self.took(delivery, true, Some(out));
                    }
                    None => match self.parked.remove(&thread) {
                        Some(mut spec) => {
                            if let Some(settings) = settings {
                                spec.settings = settings;
                            }
                            self.start(spec, Some(input), out);
                            self.took(delivery, true, Some(out));
                        }
                        None => self.took(delivery, false, Some(out)),
                    },
                }
            }
            HubMsg::Interrupt { thread } => self.cmd(thread, SessionCmd::Interrupt),
            HubMsg::StopTask { thread, tool } => self.cmd(thread, SessionCmd::StopTask { tool }),
            HubMsg::SignIn {
                thread,
                request,
                code,
            } => self.cmd(thread, SessionCmd::SignIn { request, code }),
            HubMsg::Respond {
                thread,
                request,
                response,
            } => self.cmd(thread, SessionCmd::Respond { request, response }),
            HubMsg::Settings { thread, settings } => {
                self.cmd(thread, SessionCmd::Settings(settings))
            }
            HubMsg::Stop { thread } => {
                if let Some(s) = self.sessions.get_mut(&thread) {
                    s.stopping = true;
                    s.parking = false;
                }
                self.cmd(thread, SessionCmd::Stop)
            }
            HubMsg::TermInput { term, data } => self.terms.input(&term, &data),
            HubMsg::TermResize { term, cols, rows } => self.terms.resize(&term, cols, rows),
            HubMsg::TermClose { term } => self.terms.close(&term),
            HubMsg::Request {
                id,
                body:
                    corra_proto::runner::RunnerRequest::TermOpen {
                        term,
                        cwd,
                        cols,
                        rows,
                    },
            } => {
                let r = self.terms.open(
                    term.clone(),
                    &files::expand(&cwd),
                    cols,
                    rows,
                    self.bg_tx.clone(),
                );
                out.send(match r {
                    Ok(()) => RunnerMsg::Reply {
                        id,
                        ok: Some(serde_json::json!({ "term": term })),
                        err: None,
                    },
                    Err(e) => RunnerMsg::Reply {
                        id,
                        ok: None,
                        err: Some(format!("{e:#}")),
                    },
                });
            }
            HubMsg::Request {
                id,
                body: corra_proto::runner::RunnerRequest::TermAttach { term },
            } => {
                out.send(match self.terms.scrollback(&term) {
                    Ok(d) => RunnerMsg::Reply {
                        id,
                        ok: Some(serde_json::json!({ "data": d })),
                        err: None,
                    },
                    Err(e) => RunnerMsg::Reply {
                        id,
                        ok: None,
                        err: Some(format!("{e:#}")),
                    },
                });
            }
            HubMsg::Request {
                id,
                body: corra_proto::runner::RunnerRequest::Stop { thread },
            } => {
                self.parked.remove(&thread);
                if let Some(s) = self.sessions.get_mut(&thread) {
                    s.stopping = true;
                    s.parking = false;
                    let _ = s.cmds.send(SessionCmd::Stop);
                    let (tx, rx) = oneshot::channel();
                    self.stop_waiters.entry(thread).or_default().push(tx);
                    let out = out.clone();
                    tokio::spawn(async move {
                        let _ = tokio::time::timeout(Duration::from_secs(10), rx).await;
                        out.send(RunnerMsg::Reply {
                            id,
                            ok: Some(serde_json::json!({})),
                            err: None,
                        });
                    });
                } else {
                    out.send(RunnerMsg::Reply {
                        id,
                        ok: Some(serde_json::json!({})),
                        err: None,
                    });
                }
            }
            HubMsg::Request {
                id,
                body: corra_proto::runner::RunnerRequest::UpdateRunner,
            } => {
                if self.newer.is_none() {
                    out.send(RunnerMsg::Reply {
                        id,
                        ok: Some(serde_json::json!({ "status": "up_to_date" })),
                        err: None,
                    });
                } else if self.sessions.values().any(|s| {
                    matches!(
                        s.status,
                        ThreadStatus::Running | ThreadStatus::Waiting | ThreadStatus::Starting
                    )
                }) {
                    out.send(RunnerMsg::Reply {
                        id,
                        ok: None,
                        err: Some(
                            "A thread is active on this machine; wait for it to finish before updating the runner."
                                .into(),
                        ),
                    });
                } else {
                    self.switch_soon = true;
                    self.update();
                    out.send(RunnerMsg::Reply {
                        id,
                        ok: Some(serde_json::json!({ "status": "updating" })),
                        err: None,
                    });
                }
            }
            HubMsg::Request { id, body } => {
                let out = out.clone();
                let (hub_url, token, git_token, sync) = (
                    self.hub_url.clone(),
                    self.token.clone(),
                    self.git_token(),
                    self.sync.clone(),
                );
                tokio::spawn(async move {
                    let msg = match files::handle(body, &hub_url, &token, &git_token, sync).await {
                        Ok(v) => RunnerMsg::Reply {
                            id,
                            ok: Some(v),
                            err: None,
                        },
                        Err(e) => RunnerMsg::Reply {
                            id,
                            ok: None,
                            err: Some(format!("{e:#}")),
                        },
                    };
                    out.send(msg);
                });
            }
            HubMsg::CallResult { id, ok, err } => {
                if let Some(reply) = self.calls.remove(&id) {
                    let _ = reply.send(match err {
                        Some(e) => Err(e),
                        None => Ok(ok.unwrap_or(serde_json::Value::Null)),
                    });
                }
            }
        }
    }

    fn checkpoint(&self, thread: ThreadId, turn: String) {
        let Some(s) = self.sessions.get(&thread) else {
            return;
        };
        let baseline = s.baseline.clone();
        let root = s.cwd.clone();
        let tx = self.agent_tx.clone();
        tokio::spawn(async move {
            let mut b = baseline.lock().await;
            let repos = git::checkpoint_all(&root, &thread.to_string(), &turn, &b).await;
            if repos.is_empty() {
                return;
            }
            *b = repos
                .iter()
                .map(|r| (r.repo.clone(), r.after.clone()))
                .collect();
            if repos.iter().any(|r| !r.files.is_empty()) {
                let _ = tx.send((thread, AgentOut::Event(Event::Checkpoint { turn, repos })));
            }
        });
    }

    fn sync_thread(&self, thread: ThreadId) -> Option<tokio::task::JoinHandle<()>> {
        let s = self.sessions.get(&thread)?;
        let sid = s.session_id.clone()?;
        let (base, files) = agents::session_files(s.agent, &s.cwd, &sid);
        if files.is_empty() {
            return None;
        }
        let sync = self.sync.clone()?;
        Some(tokio::spawn(async move {
            if let Err(e) = sync.upload(thread, &base, files).await {
                tracing::warn!(%thread, "couldn't save session files to the hub: {e:#}");
            }
        }))
    }

    fn start(&mut self, spec: StartSpec, input: Option<Vec<InputPart>>, out: &OutTx) {
        let thread = spec.thread;
        self.ended_while_offline.remove(&thread);
        if let Some(s) = self.sessions.get(&thread) {
            if let Some(input) = input {
                let _ = s.cmds.send(SessionCmd::Send {
                    input,
                    settings: Some(spec.settings.clone()),
                });
            }
            return;
        }
        let cwd = files::expand(&spec.cwd);
        if let Err(e) = std::fs::create_dir_all(&cwd) {
            tracing::warn!("can't create {}: {e}", cwd.display());
        }
        let extra_dirs = spec.extra_dirs.iter().map(|d| files::expand(d)).collect();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
        let agent_tx = self.agent_tx.clone();
        tokio::spawn(async move {
            while let Some(o) = ev_rx.recv().await {
                if agent_tx.send((thread, o)).is_err() {
                    break;
                }
            }
        });
        self.sessions.insert(
            thread,
            Session {
                cmds: cmd_tx,
                status: ThreadStatus::Starting,
                next_seq: spec.next_seq,
                agent: spec.agent,
                cwd: cwd.clone(),
                session_id: spec.resume_session.clone(),
                baseline: Default::default(),
                spec: spec.clone(),
                idle_since: None,
                parking: false,
                stopping: false,
                held: vec![],
                restart: None,
            },
        );
        out.send(RunnerMsg::ThreadStatus {
            thread,
            status: ThreadStatus::Starting,
        });
        agents::spawn(SessionCtx {
            baseline: self.sessions[&thread].baseline.clone(),
            hub_url: self.hub_url.clone(),
            git_token: self.git_token(),
            hub: self.hub_tx.clone(),
            sync: self.sync.clone(),
            spec,
            cwd,
            extra_dirs,
            out: ev_tx,
            cmds: cmd_rx,
            initial: input,
        });
    }

    fn ready_to_update(&self) -> bool {
        if self.switch_soon {
            let busy = self.sessions.values().any(|s| {
                matches!(
                    s.status,
                    ThreadStatus::Running | ThreadStatus::Waiting | ThreadStatus::Starting
                )
            });
            return self.newer.is_some() && !busy;
        }
        self.newer.is_some()
            && self.terms.is_empty()
            && self.sessions.values().all(|s| {
                s.parking
                    || s.idle_since
                        .is_some_and(|t| t.elapsed() >= Duration::from_secs(120))
            })
    }

    fn update(&mut self) {
        if self.switch_at.is_some() || !self.ready_to_update() {
            return;
        }
        let Some(want) = self.newer.clone() else {
            return;
        };
        if self.fetched.is_none() {
            if self.fetching.is_none() {
                let (hub, token) = (self.hub_url.clone(), self.token.clone());
                self.fetching = Some(tokio::spawn(async move {
                    update::fetch(&hub, &token, &want).await
                }));
            }
            return;
        }
        for s in self.sessions.values_mut() {
            s.parking = true;
            let _ = s.cmds.send(SessionCmd::Stop);
        }
        let delay = if self.switch_soon {
            Duration::from_millis(600)
        } else {
            Duration::from_secs(2)
        };
        self.switch_at = Some(tokio::time::Instant::now() + delay);
    }

    fn switch(&mut self) {
        self.switch_at = None;
        self.switch_soon = false;
        let Some(next) = self.fetched.take() else {
            return;
        };
        let e = update::switch(next);
        tracing::warn!("couldn't restart into the newer runner: {e:#}");
        self.newer = None;
    }

    fn seen(&mut self, delivery: &Option<String>, out: &OutTx) -> bool {
        let Some(d) = delivery else {
            return false;
        };
        if !self.took.contains(d) {
            return false;
        }
        out.send(RunnerMsg::Took {
            delivery: d.clone(),
            taken: true,
        });
        true
    }

    fn took(&mut self, delivery: Option<String>, taken: bool, out: Option<&OutTx>) {
        let Some(d) = delivery else {
            return;
        };
        if taken {
            if self.took.len() >= 500 {
                self.took.pop_front();
            }
            self.took.push_back(d.clone());
        }
        if let Some(out) = out {
            out.send(RunnerMsg::Took { delivery: d, taken });
        }
    }

    fn set_connected(&self, on: bool) {
        if let Some(c) = &self.connected {
            c.store(on, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn park_idle(&mut self) {
        secrets::sweep(&secrets::dir(), secrets::KEPT_FOR);
        let limit = park_after();
        for s in self.sessions.values_mut() {
            if !s.parking && s.idle_since.is_some_and(|t| t.elapsed() >= limit) {
                s.parking = true;
                let _ = s.cmds.send(SessionCmd::Stop);
            }
        }
    }

    fn cmd(&self, thread: ThreadId, cmd: SessionCmd) {
        if let Some(s) = self.sessions.get(&thread) {
            let _ = s.cmds.send(cmd);
        }
    }

    fn on_agent(&mut self, thread: ThreadId, out: AgentOut, tx: Option<&OutTx>) {
        match out {
            AgentOut::Event(event) if event.is_transient() => {
                if let Some(tx) = tx
                    && let Ok(status) = serde_json::to_value(&event)
                {
                    tx.send(RunnerMsg::Status { thread, status });
                }
            }
            AgentOut::Event(event) => {
                let Some(s) = self.sessions.get_mut(&thread) else {
                    return;
                };
                if let Event::Session { session_id, .. } = &event {
                    s.session_id = Some(session_id.clone());
                }
                let turn_done = match &event {
                    Event::TurnCompleted { turn, .. } => Some(turn.clone()),
                    _ => None,
                };
                let seq = s.next_seq;
                s.next_seq += 1;
                let ts = now_ms();
                self.outbox.push(thread, seq, ts, event.clone());
                if let Some(tx) = tx {
                    tx.send(RunnerMsg::Event {
                        thread,
                        seq,
                        ts,
                        event,
                    });
                }
                if let Some(turn) = turn_done {
                    self.sync_thread(thread);
                    self.checkpoint(thread, turn);
                }
            }
            AgentOut::Delta {
                turn,
                item,
                parent,
                delta,
            } => {
                if let Some(tx) = tx {
                    tx.send(RunnerMsg::Delta {
                        thread,
                        turn,
                        item,
                        parent,
                        delta,
                    });
                }
            }
            AgentOut::Status(status) => {
                if let Some(s) = self.sessions.get_mut(&thread) {
                    s.status = status;
                    s.idle_since = matches!(status, ThreadStatus::Idle | ThreadStatus::Error)
                        .then(std::time::Instant::now);
                }
                if let Some(tx) = tx {
                    tx.send(RunnerMsg::ThreadStatus { thread, status });
                }
            }
            AgentOut::Usage(usage) => {
                if let Some(tx) = tx {
                    tx.send(RunnerMsg::AgentUsage { usage });
                }
            }
            AgentOut::Exited { error } => {
                if let Some(e) = &error {
                    tracing::warn!(%thread, "agent exited: {e}");
                }
                let sync_task = self.sync_thread(thread);
                let waiters = self.stop_waiters.remove(&thread).unwrap_or_default();
                if !waiters.is_empty() {
                    if let Some(handle) = sync_task {
                        tokio::spawn(async move {
                            let _ = tokio::time::timeout(Duration::from_secs(8), handle).await;
                            for w in waiters {
                                let _ = w.send(());
                            }
                        });
                    } else {
                        for w in waiters {
                            let _ = w.send(());
                        }
                    }
                }
                let ended = self.sessions.remove(&thread);
                if let Some(s) = ended.filter(|s| {
                    s.restart.is_some() || !s.held.is_empty() || (s.parking && error.is_none())
                }) && self.switch_at.is_none()
                {
                    let mut spec = match s.restart {
                        Some(mut next) => {
                            if next.resume_session.is_none() {
                                next.resume_session = s.session_id;
                            }
                            next.next_seq = next.next_seq.max(s.next_seq);
                            next
                        }
                        None => {
                            let mut spec = s.spec;
                            spec.resume_session = s.session_id;
                            spec.next_seq = s.next_seq;
                            spec.fork_of = None;
                            spec.resume_at = None;
                            spec.moved_from = None;
                            spec
                        }
                    };
                    let mut held = s.held.into_iter();
                    if let Some((input, settings, delivery)) = held.next() {
                        if let Some(settings) = settings {
                            spec.settings = settings;
                        }
                        let out = OutTx(self.bg_tx.clone());
                        self.start(spec, Some(input), &out);
                        self.took(delivery, true, tx);
                        for (input, settings, delivery) in held {
                            self.cmd(thread, SessionCmd::Send { input, settings });
                            self.took(delivery, true, tx);
                        }
                        return;
                    }
                    self.parked.insert(thread, spec);
                }
                if let Some(tx) = tx {
                    tx.send(RunnerMsg::ThreadEnded { thread, error });
                } else {
                    self.ended_while_offline.insert(thread, error);
                }
            }
        }
    }
}
