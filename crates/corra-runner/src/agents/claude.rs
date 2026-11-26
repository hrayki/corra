use std::collections::{HashMap, VecDeque};
use std::process::Stdio;

use anyhow::{Context, Result, anyhow};
use corra_proto::*;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

use super::{AgentOut, SessionCmd, SessionCtx};

pub async fn run(mut ctx: SessionCtx) {
    let result = drive(&mut ctx).await;
    finish(&ctx, result);
}

fn finish(ctx: &SessionCtx, result: Result<()>) {
    let error = result.err().map(|e| format!("{e:#}"));
    if let Some(e) = &error {
        let _ = ctx.out.send(AgentOut::Event(Event::Notice {
            level: NoticeLevel::Error,
            text: format!("Claude Code stopped: {e}"),
        }));
    }
    let _ = ctx.out.send(AgentOut::Exited { error });
}

type Reader = Box<dyn tokio::io::AsyncRead + Unpin + Send>;
type Writer = Box<dyn tokio::io::AsyncWrite + Unpin + Send>;

struct Proc {
    child: Option<Child>,
    stdin: Writer,
    lines: tokio::io::Lines<BufReader<Reader>>,
    next_req: u64,
    stderr: std::sync::Arc<std::sync::Mutex<VecDeque<String>>>,
    stderr_reader: Option<tokio::task::JoinHandle<()>>,
    rec: Option<super::record::Recorder>,
}

const STDERR_LINES: usize = 15;
const STDERR_LINE_CHARS: usize = 400;

fn exit_text(status: std::process::ExitStatus, tail: &str) -> String {
    with_output(format!("claude exited ({status})"), tail)
}

fn with_output(what: String, tail: &str) -> String {
    if tail.is_empty() || what.contains(tail) {
        what
    } else {
        format!("{what}. The end of its output:\n\n```\n{tail}\n```")
    }
}

impl Proc {
    #[cfg(test)]
    fn piped(reader: Reader, writer: Writer) -> Proc {
        Proc {
            child: None,
            stdin: writer,
            lines: BufReader::new(reader).lines(),
            next_req: 0,
            stderr: Default::default(),
            stderr_reader: None,
            rec: None,
        }
    }

    async fn wait(&mut self) -> Result<std::process::ExitStatus> {
        match &mut self.child {
            Some(c) => Ok(c.wait().await?),
            None => {
                use std::os::unix::process::ExitStatusExt;
                Ok(std::process::ExitStatus::from_raw(0))
            }
        }
    }

    async fn kill(&mut self) {
        if let Some(c) = &mut self.child {
            let _ = c.kill().await;
        }
    }

    async fn write(&mut self, v: &Value) -> Result<()> {
        if let Some(r) = &mut self.rec {
            r.runner(v);
        }
        let mut line = serde_json::to_vec(v)?;
        line.push(b'\n');
        self.stdin.write_all(&line).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn stderr_tail(&mut self) -> String {
        if let Some(reader) = self.stderr_reader.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_millis(500), reader).await;
        }
        let lines = self.stderr.lock().unwrap();
        lines.iter().cloned().collect::<Vec<_>>().join("\n")
    }

    async fn control(&mut self, request: Value) -> Result<String> {
        self.next_req += 1;
        let id = format!("corra-{}", self.next_req);
        self.write(&json!({ "type": "control_request", "request_id": id, "request": request }))
            .await?;
        Ok(id)
    }
}

fn spawn_claude(ctx: &SessionCtx, settings: &AgentSettings, session: &SessionRef) -> Result<Proc> {
    let mut cmd = Command::new(claude_bin());
    cmd.arg("-p")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--input-format")
        .arg("stream-json")
        .arg("--verbose")
        .arg("--include-partial-messages")
        .arg("--permission-prompt-tool")
        .arg("stdio")
        .arg("--thinking")
        .arg("adaptive")
        .arg("--thinking-display")
        .arg("summarized");
    match session {
        SessionRef::New(id) => {
            cmd.arg("--session-id").arg(id);
        }
        SessionRef::Resume(id) => {
            cmd.arg("--resume").arg(id);
            if let Some(at) = &ctx.spec.resume_at {
                cmd.arg("--resume-session-at").arg(at);
            }
        }
        SessionRef::Fork(f) => {
            cmd.arg("--resume").arg(&f.session).arg("--fork-session");
            if let Some(at) = &f.at_message {
                cmd.arg("--resume-session-at").arg(at);
            }
        }
    }
    if let Some(m) = &settings.model {
        cmd.arg("--model").arg(m);
    }
    if let Some(e) = &settings.effort {
        cmd.arg("--effort").arg(e);
    }
    let mode = settings.permission_mode.as_deref().unwrap_or("default");
    cmd.arg("--permission-mode").arg(mode);
    if mode == "bypassPermissions" {
        cmd.arg("--allow-dangerously-skip-permissions");
    }
    for d in &ctx.extra_dirs {
        cmd.arg("--add-dir").arg(d);
    }
    let mut servers = serde_json::Map::new();
    let mut allowed = vec![];
    if !ctx.spec.tools.is_empty() {
        servers.insert(
            "corra".into(),
            json!({ "type": "sdk", "name": "corra", "alwaysLoad": true }),
        );
        allowed.push("mcp__corra__*");
    }
    if let Some(cdp) = ctx.browser_cdp.as_ref().filter(|_| ctx.spec.browser) {
        let server = if which("playwright-mcp") {
            Some(json!({ "command": "playwright-mcp", "args": ["--cdp-endpoint", cdp] }))
        } else if which("npx") {
            Some(
                json!({ "command": "npx", "args": ["-y", "@playwright/mcp@latest", "--cdp-endpoint", cdp] }),
            )
        } else {
            None
        };
        if let Some(server) = server {
            servers.insert("browser".into(), server);
            allowed.push("mcp__browser__*");
        }
    }
    let shared_mcp = crate::files::home().join(".corra/kit/shared/mcp.json");
    if !servers.is_empty() || shared_mcp.exists() {
        cmd.arg("--mcp-config")
            .arg(json!({ "mcpServers": servers }).to_string());
        if shared_mcp.exists() {
            cmd.arg(&shared_mcp);
        }
        cmd.arg("--allowedTools").arg(allowed.join(","));
    }
    cmd.current_dir(&ctx.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env_remove("CLAUDECODE")
        .env_remove("NODE_OPTIONS")
        .env_remove("CORRA_TOKEN")
        .env_remove("CORRA_TOKEN_FILE")
        .env("CLAUDE_CODE_ENTRYPOINT", "sdk-ts")
        .env("CLAUDE_CODE_FORWARD_SUBAGENT_TEXT", "1")
        .env("CLAUDE_CODE_DISABLE_TERMINAL_TITLE", "1")
        .env("CLAUDE_CODE_ENABLE_TODO_TOOLS", "1");
    for (k, v) in &ctx.spec.agent_env {
        cmd.env(k, v);
    }
    #[cfg(target_os = "linux")]
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            Ok(())
        });
    }
    let mut child = cmd.spawn().context("starting claude")?;
    let stdin: Writer = Box::new(child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?);
    let stdout: Reader = Box::new(child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?);
    let tail = std::sync::Arc::new(std::sync::Mutex::new(VecDeque::new()));
    let stderr_reader = child.stderr.take().map(|stderr| {
        let tail = tail.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                tracing::debug!(target: "corra_runner::claude", "stderr: {l}");
                if l.trim().is_empty() {
                    continue;
                }
                let mut tail = tail.lock().unwrap();
                if tail.len() == STDERR_LINES {
                    tail.pop_front();
                }
                tail.push_back(l.chars().take(STDERR_LINE_CHARS).collect());
            }
        })
    });
    let rec = super::record::Recorder::open(
        ctx.spec.thread,
        json!({
            "session": match session {
                SessionRef::New(_) => "new",
                SessionRef::Resume(_) => "resume",
                SessionRef::Fork(_) => "fork",
            },
            "settings": settings,
            "tools": ctx.spec.tools,
        }),
    );
    Ok(Proc {
        child: Some(child),
        stdin,
        lines: BufReader::new(stdout).lines(),
        next_req: 0,
        stderr: tail,
        stderr_reader,
        rec,
    })
}

fn which(name: &str) -> bool {
    std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .any(|d| !d.is_empty() && std::path::Path::new(d).join(name).is_file())
}

pub fn find_bin() -> String {
    claude_bin()
}

fn claude_bin() -> String {
    if let Ok(b) = std::env::var("CORRA_CLAUDE_BIN") {
        return b;
    }
    let path = std::env::var("PATH").unwrap_or_default();
    for dir in path.split(':').filter(|d| !d.is_empty()) {
        let c = std::path::Path::new(dir).join("claude");
        if c.is_file() {
            return c.to_string_lossy().to_string();
        }
    }
    for c in [
        crate::files::home().join(".local/bin/claude"),
        "/root/.local/bin/claude".into(),
        "/usr/local/bin/claude".into(),
    ] {
        if c.is_file() {
            return c.to_string_lossy().to_string();
        }
    }
    "claude".into()
}

enum SessionRef {
    New(String),
    Resume(String),
    Fork(ForkSource),
}

struct PendingPermission {
    request_id: String,
    tool: String,
    input: Value,
    suggestions: Value,
}

struct ToolInfo {
    name: String,
    input: Value,
    parent: Option<String>,
}

#[derive(Default)]
struct StreamState {
    message_id: Option<String>,
    parent: Option<String>,
    blocks: HashMap<u64, (String, String)>,
    block_count: usize,
}

struct BackgroundTask {
    id: String,
    turn: String,
    kind: String,
    description: String,
}

struct State {
    turn: Option<String>,
    last_turn: Option<String>,
    user_turn: bool,
    turn_began: bool,
    knows_users: bool,
    background_said: Option<std::time::Instant>,
    background_tasks: HashMap<String, BackgroundTask>,
    wake_causes: Vec<String>,
    queued_turns: VecDeque<(String, Vec<InputPart>)>,
    tasks: Vec<(String, Todo)>,
    context_window: Option<u64>,
    deferred: Option<tokio::sync::mpsc::UnboundedSender<Value>>,
    anchor: Option<String>,
    moved_note: Option<String>,
    permissions: HashMap<String, PendingPermission>,
    tools: HashMap<String, ToolInfo>,
    streams: HashMap<Option<String>, StreamState>,
    message_blocks: HashMap<String, usize>,
    settings: AgentSettings,
    session_id: String,
    session_announced: bool,
    turn_counter: u64,
    api_error: Option<(String, String)>,
    limit_resets: Option<i64>,
    foreground_tasks: std::collections::HashSet<String>,
    task_ids: HashMap<String, String>,
    running_tasks: Vec<String>,
}

impl State {
    fn new_turn_id(&mut self) -> String {
        self.turn_counter += 1;
        format!("t{}-{}", now_ms(), self.turn_counter)
    }
}

fn new_state(ctx: &SessionCtx) -> (State, SessionRef) {
    let mut st = State {
        turn: None,
        last_turn: None,
        user_turn: false,
        turn_began: false,
        knows_users: false,
        background_said: None,
        background_tasks: HashMap::new(),
        wake_causes: vec![],
        queued_turns: VecDeque::new(),
        tasks: Vec::new(),
        context_window: None,
        deferred: None,
        anchor: None,
        moved_note: None,
        permissions: HashMap::new(),
        tools: HashMap::new(),
        streams: HashMap::new(),
        message_blocks: HashMap::new(),
        settings: ctx.spec.settings.clone(),
        session_id: String::new(),
        session_announced: false,
        turn_counter: 0,
        api_error: None,
        limit_resets: None,
        foreground_tasks: Default::default(),
        task_ids: HashMap::new(),
        running_tasks: vec![],
    };
    let session = match (&ctx.spec.resume_session, &ctx.spec.fork_of) {
        (Some(id), _) => SessionRef::Resume(id.clone()),
        (None, Some(f)) => SessionRef::Fork(f.clone()),
        (None, None) => SessionRef::New(uuid::Uuid::now_v7().to_string()),
    };
    st.session_id = match &session {
        SessionRef::New(id) | SessionRef::Resume(id) => id.clone(),
        SessionRef::Fork(_) => String::new(),
    };
    st.moved_note = ctx.spec.moved_from.as_ref().map(|old| {
        format!(
            "[corra: this thread moved to another machine. The project folder is now {}; it was {old}.]",
            ctx.cwd.display()
        )
    });
    (st, session)
}

async fn drive(ctx: &mut SessionCtx) -> Result<()> {
    let (mut st, session) = new_state(ctx);
    let mut proc = spawn_claude(ctx, &st.settings, &session)?;
    talk(ctx, &mut st, &mut proc).await
}

#[cfg(test)]
pub(super) async fn replay(mut ctx: SessionCtx, reader: Reader, writer: Writer) {
    let (mut st, _) = new_state(&ctx);
    let mut proc = Proc::piped(reader, writer);
    let result = talk(&mut ctx, &mut st, &mut proc).await;
    finish(&ctx, result);
}

async fn talk(ctx: &mut SessionCtx, st: &mut State, proc: &mut Proc) -> Result<()> {
    match converse(ctx, st, proc).await {
        Ok(()) => Ok(()),
        Err(e) => {
            let tail = proc.stderr_tail().await;
            Err(anyhow!(with_output(format!("{e:#}"), &tail)))
        }
    }
}

async fn converse(ctx: &mut SessionCtx, st: &mut State, proc: &mut Proc) -> Result<()> {
    let mut init = json!({ "subtype": "initialize", "perTaskStopAffordance": true });
    if !ctx.spec.tools.is_empty() {
        init["sdkMcpServers"] = json!(["corra"]);
    }
    proc.control(init).await?;
    emit(
        ctx,
        Event::Status {
            context: None,
            rate_limit: None,
            background: Some(0),
            agents: Some(vec![]),
        },
    );
    let (deferred_tx, mut deferred_rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    st.deferred = Some(deferred_tx);

    if let Some(input) = ctx.initial.take() {
        if let Some(r) = &mut proc.rec {
            r.app(
                &SessionCmd::Send {
                    input: input.clone(),
                    settings: None,
                },
                true,
            );
        }
        send_user(ctx, st, proc, input).await?;
    } else {
        let _ = ctx.out.send(AgentOut::Status(ThreadStatus::Idle));
    }

    loop {
        tokio::select! {
            biased;
            line = proc.lines.next_line() => {
                let Some(line) = line? else {
                    let status = proc.wait().await?;
                    if let Some(r) = &mut proc.rec {
                        r.exit(status.code());
                    }
                    let why = exit_text(status, &proc.stderr_tail().await);
                    finish_open_turn(ctx, st, TurnOutcome::Failed, Some(why.clone()));
                    if status.success() { return Ok(()); }
                    return Err(anyhow!(why));
                };
                if line.trim().is_empty() { continue; }
                if let Some(r) = &mut proc.rec {
                    r.claude(&line);
                }
                let msg: Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => { tracing::debug!("non-json from claude: {line}"); continue; }
                };
                on_message(ctx, st, proc, msg).await?;
            }
            Some(v) = deferred_rx.recv() => {
                proc.write(&v).await?;
            }
            cmd = ctx.cmds.recv() => {
                let Some(cmd) = cmd else { proc.kill().await; return Ok(()); };
                if let Some(r) = &mut proc.rec {
                    r.app(&cmd, false);
                }
                match cmd {
                    SessionCmd::Send { input, settings } => {
                        if let Some(s) = settings {
                            apply_settings(ctx, st, proc, s).await?;
                        }
                        send_user(ctx, st, proc, input).await?;
                    }
                    SessionCmd::StopTask { tool } => {
                        if let Some(id) = st.task_ids.get(&tool) {
                            proc.control(json!({ "subtype": "stop_task", "task_id": id })).await?;
                        }
                    }
                    SessionCmd::Interrupt => {
                        proc.control(json!({ "subtype": "interrupt", "cancel_queued": true })).await?;
                        let ids: Vec<String> = st.permissions.keys().cloned().collect();
                        for id in ids {
                            resolve_permission(ctx, st, proc, &id, RequestResponse::Cancelled).await?;
                        }
                    }
                    SessionCmd::Respond { request, response } => {
                        resolve_permission(ctx, st, proc, &request, response).await?;
                    }
                    SessionCmd::Settings(s) => apply_settings(ctx, st, proc, s).await?,
                    SessionCmd::Stop => {
                        proc.kill().await;
                        finish_open_turn(ctx, st, TurnOutcome::Interrupted, None);
                        return Ok(());
                    }
                }
            }
        }
    }
}

async fn apply_settings(
    ctx: &SessionCtx,
    st: &mut State,
    proc: &mut Proc,
    s: AgentSettings,
) -> Result<()> {
    let mut changed = AgentSettings::default();
    if s.model.is_some() && s.model != st.settings.model {
        proc.control(json!({ "subtype": "set_model", "model": s.model }))
            .await?;
        changed.model = s.model.clone();
    }
    if s.permission_mode.is_some() && s.permission_mode != st.settings.permission_mode {
        proc.control(json!({ "subtype": "set_permission_mode", "mode": s.permission_mode }))
            .await?;
        changed.permission_mode = s.permission_mode.clone();
    }
    if s.effort.is_some() && s.effort != st.settings.effort {
        proc.control(
            json!({ "subtype": "apply_flag_settings", "settings": { "effortLevel": s.effort } }),
        )
        .await?;
        changed.effort = s.effort.clone();
    }
    if changed != AgentSettings::default() {
        st.settings.merge(&changed);
        emit(ctx, Event::SettingsChanged { settings: changed });
    }
    Ok(())
}

fn emit(ctx: &SessionCtx, e: Event) {
    let _ = ctx.out.send(AgentOut::Event(e));
}

async fn send_user(
    ctx: &SessionCtx,
    st: &mut State,
    proc: &mut Proc,
    input: Vec<InputPart>,
) -> Result<()> {
    let mut content: Vec<Value> = input
        .iter()
        .map(|p| match p {
            InputPart::Text { text } => json!({ "type": "text", "text": text }),
            InputPart::Image { media_type, data } => json!({
                "type": "image",
                "source": { "type": "base64", "media_type": media_type, "data": data }
            }),
        })
        .collect();
    if let Some(note) = st.moved_note.take() {
        content.push(json!({ "type": "text", "text": note }));
    }
    let turn = uuid::Uuid::now_v7().to_string();
    proc.write(&json!({
        "type": "user",
        "uuid": turn,
        "message": { "role": "user", "content": content },
        "parent_tool_use_id": null,
        "session_id": st.session_id,
    }))
    .await?;
    match st.turn.clone() {
        None => start_turn(ctx, st, turn, input),
        Some(running) => {
            emit(
                ctx,
                Event::ItemCompleted {
                    turn: running,
                    item: Item {
                        id: format!("{turn}:input"),
                        parent: None,
                        body: ItemBody::UserMessage {
                            content: input.clone(),
                        },
                    },
                },
            );
            st.queued_turns.push_back((turn, input));
        }
    }
    Ok(())
}

fn start_turn(ctx: &SessionCtx, st: &mut State, turn: String, input: Vec<InputPart>) {
    emit(
        ctx,
        Event::TurnStarted {
            turn: turn.clone(),
            input: input.clone(),
            settings: st.settings.clone(),
            cause: None,
        },
    );
    emit(
        ctx,
        Event::ItemCompleted {
            turn: turn.clone(),
            item: Item {
                id: format!("{turn}:input"),
                parent: None,
                body: ItemBody::UserMessage { content: input },
            },
        },
    );
    st.last_turn = Some(turn.clone());
    st.turn = Some(turn);
    st.user_turn = true;
    st.turn_began = false;
    st.wake_causes.clear();
    st.api_error = None;
    let _ = ctx.out.send(AgentOut::Status(ThreadStatus::Running));
}

fn finish_open_turn(ctx: &SessionCtx, st: &mut State, outcome: TurnOutcome, error: Option<String>) {
    if let Some(turn) = st.turn.take() {
        close_running_tools(ctx, st, &turn);
        let (error, resets_at) = match st.api_error.take() {
            Some((kind, text)) if outcome == TurnOutcome::Failed => {
                (Some(text), limit_reset(st, &kind))
            }
            _ => (error, None),
        };
        emit(
            ctx,
            Event::TurnCompleted {
                turn,
                outcome,
                usage: None,
                error,
                anchor: st.anchor.clone(),
                resets_at,
            },
        );
    }
    while let Some((turn, input)) = st.queued_turns.pop_front() {
        emit(
            ctx,
            Event::TurnStarted {
                turn: turn.clone(),
                input,
                settings: st.settings.clone(),
                cause: None,
            },
        );
        emit(
            ctx,
            Event::TurnCompleted {
                turn,
                outcome: TurnOutcome::Interrupted,
                usage: None,
                error: None,
                anchor: None,
                resets_at: None,
            },
        );
    }
}

fn limit_reset(st: &State, kind: &str) -> Option<i64> {
    let limited = matches!(kind, "rate_limit" | "usage_limit_reached");
    st.limit_resets.filter(|_| limited)
}

fn close_running_tools(ctx: &SessionCtx, st: &mut State, turn: &str) {
    for (id, t) in st.tools.drain() {
        emit(
            ctx,
            Event::ItemCompleted {
                turn: turn.to_string(),
                item: Item {
                    id,
                    parent: t.parent,
                    body: ItemBody::ToolCall {
                        name: t.name,
                        input: t.input,
                        output: None,
                        status: ToolStatus::Failed,
                    },
                },
            },
        );
    }
}

fn current_turn(ctx: &SessionCtx, st: &mut State) -> String {
    if st.turn.is_none() {
        let t = st.new_turn_id();
        let causes = std::mem::take(&mut st.wake_causes);
        emit(
            ctx,
            Event::TurnStarted {
                turn: t.clone(),
                input: vec![],
                settings: st.settings.clone(),
                cause: (!causes.is_empty()).then(|| causes.join("\n")),
            },
        );
        st.last_turn = Some(t.clone());
        st.turn = Some(t);
        st.user_turn = false;
        st.turn_began = true;
        let _ = ctx.out.send(AgentOut::Status(ThreadStatus::Running));
    }
    st.turn.clone().unwrap()
}

fn turn_for(ctx: &SessionCtx, st: &mut State, parent: &Option<String>) -> String {
    let home = parent
        .as_ref()
        .and_then(|p| st.background_tasks.get(p))
        .map(|t| t.turn.clone());
    if st.turn.is_none()
        && parent.is_some()
        && let Some(last) = home.clone().or_else(|| st.last_turn.clone())
    {
        if st
            .background_said
            .is_none_or(|at| at.elapsed() >= std::time::Duration::from_secs(5))
        {
            st.background_said = Some(std::time::Instant::now());
            background(ctx, now_ms());
        }
        return last;
    }
    if let Some(home) = home {
        return home;
    }
    current_turn(ctx, st)
}

fn background(ctx: &SessionCtx, at: i64) {
    emit(
        ctx,
        Event::Status {
            context: None,
            rate_limit: None,
            background: Some(at),
            agents: None,
        },
    );
}

async fn on_message(ctx: &SessionCtx, st: &mut State, proc: &mut Proc, msg: Value) -> Result<()> {
    let ty = msg["type"].as_str().unwrap_or("");
    if msg["user_message_uuids"].is_array() {
        st.knows_users = true;
    }
    match ty {
        "command_lifecycle" => on_lifecycle(ctx, st, &msg),
        "system" => on_system(ctx, st, &msg),
        "stream_event" => on_stream(ctx, st, &msg),
        "assistant" => on_assistant(ctx, st, &msg),
        "user" => on_user(ctx, st, &msg),
        "result" => on_result(ctx, st, &msg),
        "control_request" => on_control_request(ctx, st, proc, &msg).await?,
        "control_response" => {
            let r = &msg["response"];
            if r["subtype"] == "error"
                && r["request_id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("corra-"))
            {
                tracing::warn!("claude control error: {}", r["error"]);
            }
        }
        "control_cancel_request" => {
            if let Some(id) = msg["request_id"].as_str()
                && st.permissions.remove(id).is_some()
            {
                emit(
                    ctx,
                    Event::RequestResolved {
                        id: id.to_string(),
                        response: RequestResponse::Cancelled,
                    },
                );
            }
        }
        "rate_limit_event" => {
            let info = &msg["rate_limit_info"];
            st.limit_resets = match (info["status"].as_str(), info["resetsAt"].as_i64()) {
                (Some("rejected"), Some(at)) => Some(if at < 1_000_000_000_000 {
                    at * 1000
                } else {
                    at
                }),
                _ => None,
            };
            emit(
                ctx,
                Event::Status {
                    context: None,
                    rate_limit: Some(msg.clone()),
                    background: None,
                    agents: None,
                },
            );
        }
        _ => tracing::debug!("unhandled claude message type {ty}"),
    }
    Ok(())
}

fn on_system(ctx: &SessionCtx, st: &mut State, msg: &Value) {
    match msg["subtype"].as_str().unwrap_or("") {
        "init" => {
            let id = msg["session_id"].as_str().unwrap_or_default().to_string();
            if st.session_announced && id == st.session_id {
                return;
            }
            st.session_announced = true;
            if !id.is_empty() {
                st.session_id = id;
            }
            let strings = |v: &Value| -> Vec<String> {
                v.as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            emit(
                ctx,
                Event::Session {
                    session_id: st.session_id.clone(),
                    model: msg["model"].as_str().map(String::from),
                    cwd: msg["cwd"].as_str().unwrap_or_default().to_string(),
                    tools: strings(&msg["tools"]),
                    slash_commands: strings(&msg["slash_commands"]),
                    permission_mode: msg["permissionMode"].as_str().map(String::from),
                },
            );
        }
        "compact_boundary" => {
            let meta = &msg["compact_metadata"];
            if let Some(after) = meta["post_tokens"].as_u64() {
                emit(
                    ctx,
                    Event::Status {
                        context: Some(ContextUse {
                            used_tokens: after,
                            window_tokens: st.context_window,
                        }),
                        rate_limit: None,
                        background: None,
                        agents: None,
                    },
                );
            }
            emit(
                ctx,
                Event::Compacted {
                    trigger: meta["trigger"].as_str().map(String::from),
                    pre_tokens: meta["pre_tokens"].as_u64(),
                },
            );
        }
        "api_retry" => {
            let attempt = msg["attempt"].as_u64().unwrap_or(0);
            let max = msg["max_retries"].as_u64().unwrap_or(0);
            if attempt >= 2 {
                emit(
                    ctx,
                    Event::Notice {
                        level: NoticeLevel::Warning,
                        text: format!("Anthropic's API failed; retry {attempt} of {max}."),
                    },
                );
            }
        }
        "task_started" => {
            let id = msg["task_id"].as_str().unwrap_or_default().to_string();
            if let Some(tool) = msg["tool_use_id"].as_str() {
                st.task_ids.insert(tool.to_string(), id.clone());
                if st.running_tasks.contains(&id) {
                    running_agents(ctx, st);
                }
            }
            let owner = msg["parent_task_id"].as_str().and_then(|p| {
                st.background_tasks
                    .values()
                    .find(|t| t.id == p)
                    .map(|t| t.turn.clone())
            });
            if msg["is_backgrounded"] == false {
                st.foreground_tasks.insert(id.clone());
            }
            let turn = owner.clone().or_else(|| {
                (msg["is_backgrounded"] != false)
                    .then(|| st.turn.clone().or_else(|| st.last_turn.clone()))
                    .flatten()
            });
            if let (Some(tool), Some(turn)) = (msg["tool_use_id"].as_str(), turn) {
                st.background_tasks.insert(
                    tool.to_string(),
                    BackgroundTask {
                        id,
                        turn,
                        kind: msg["task_type"].as_str().unwrap_or_default().to_string(),
                        description: msg["description"].as_str().unwrap_or_default().to_string(),
                    },
                );
            }
        }
        "background_tasks_changed" => {
            st.running_tasks = msg["tasks"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|t| t["task_id"].as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            if st.running_tasks.is_empty() {
                st.background_said = None;
                background(ctx, 0);
            }
            running_agents(ctx, st);
        }
        "task_notification" => {
            let task = msg["task_id"].as_str().unwrap_or_default();
            let tool = msg["tool_use_id"].as_str().map(String::from);
            let info = tool.as_ref().and_then(|t| st.background_tasks.remove(t));
            if st.foreground_tasks.remove(task) {
                return;
            }
            let summary = msg["summary"].as_str().unwrap_or_default();
            let status = msg["status"].as_str().unwrap_or("completed");
            st.background_said = None;
            background(ctx, 0);
            let item = Item {
                id: format!("task:{task}"),
                parent: tool,
                body: ItemBody::AgentMessage {
                    text: if summary.starts_with("Background ") {
                        summary.to_string()
                    } else if summary.is_empty() {
                        format!("Background agent {status}.")
                    } else {
                        format!("Background agent {status}: {summary}")
                    },
                },
            };
            if st.turn.is_none() {
                st.wake_causes
                    .push(wake_cause(info.as_ref(), summary, status));
            }
            if let Some(turn) = info
                .map(|t| t.turn)
                .or_else(|| st.turn.clone())
                .or_else(|| st.last_turn.clone())
            {
                emit(ctx, Event::ItemCompleted { turn, item });
            }
        }
        _ => {}
    }
}

fn running_agents(ctx: &SessionCtx, st: &State) {
    let tools = st
        .running_tasks
        .iter()
        .filter_map(|id| {
            st.task_ids
                .iter()
                .find(|(_, t)| *t == id)
                .map(|(tool, _)| tool.clone())
        })
        .collect();
    emit(
        ctx,
        Event::Status {
            context: None,
            rate_limit: None,
            background: None,
            agents: Some(tools),
        },
    );
}

fn wake_cause(task: Option<&BackgroundTask>, summary: &str, status: &str) -> String {
    let what = match task {
        Some(t) if t.kind == "local_agent" && !t.description.is_empty() => {
            return format!(
                "Background agent {}: {}",
                match status {
                    "completed" => "finished",
                    "stopped" | "killed" => "stopped",
                    other => other,
                },
                t.description
            );
        }
        Some(t) if t.kind == "local_agent" => "agent",
        Some(t) if t.kind == "local_bash" => "command",
        _ => "task",
    };
    if summary.starts_with("Background ") {
        summary.to_string()
    } else {
        format!("Background {what} {status}.")
    }
}

fn on_lifecycle(ctx: &SessionCtx, st: &mut State, msg: &Value) {
    let ours = st.user_turn && st.turn.as_deref() == msg["command_uuid"].as_str();
    if !ours {
        return;
    }
    match msg["state"].as_str().unwrap_or("") {
        "started" => st.turn_began = true,
        "cancelled" if !st.turn_began => {
            finish_open_turn(ctx, st, TurnOutcome::Interrupted, None);
            let _ = ctx.out.send(AgentOut::Status(ThreadStatus::Idle));
        }
        _ => {}
    }
}

fn parent_of(msg: &Value) -> Option<String> {
    msg["parent_tool_use_id"].as_str().map(String::from)
}

fn on_stream(ctx: &SessionCtx, st: &mut State, msg: &Value) {
    let parent = parent_of(msg);
    let ev = &msg["event"];
    let turn = turn_for(ctx, st, &parent);
    let s = st.streams.entry(parent.clone()).or_default();
    match ev["type"].as_str().unwrap_or("") {
        "message_start" => {
            *s = StreamState {
                message_id: ev["message"]["id"].as_str().map(String::from),
                parent: parent.clone(),
                ..Default::default()
            };
        }
        "content_block_start" => {
            let index = ev["index"].as_u64().unwrap_or(0);
            let block = &ev["content_block"];
            let kind = block["type"].as_str().unwrap_or("").to_string();
            let mid = s.message_id.clone().unwrap_or_default();
            let id = match kind.as_str() {
                "tool_use" | "server_tool_use" => {
                    block["id"].as_str().unwrap_or_default().to_string()
                }
                _ => {
                    let n = s.block_count;
                    s.block_count += 1;
                    format!("{mid}:{n}")
                }
            };
            s.blocks.insert(index, (kind, id));
        }
        "content_block_delta" => {
            let index = ev["index"].as_u64().unwrap_or(0);
            let Some((_, id)) = s.blocks.get(&index) else {
                return;
            };
            let d = &ev["delta"];
            let delta = match d["type"].as_str().unwrap_or("") {
                "text_delta" => Delta::Text {
                    text: d["text"].as_str().unwrap_or_default().to_string(),
                },
                "thinking_delta" => Delta::Reasoning {
                    text: d["thinking"].as_str().unwrap_or_default().to_string(),
                },
                "input_json_delta" => Delta::ToolInput {
                    json: d["partial_json"].as_str().unwrap_or_default().to_string(),
                },
                _ => return,
            };
            let _ = ctx.out.send(AgentOut::Delta {
                turn,
                item: id.clone(),
                parent: s.parent.clone(),
                delta,
            });
        }
        "message_delta" => {
            if let Some(u) = ev["usage"].as_object() {
                let used = [
                    "input_tokens",
                    "cache_read_input_tokens",
                    "cache_creation_input_tokens",
                    "output_tokens",
                ]
                .iter()
                .filter_map(|k| u.get(*k).and_then(|v| v.as_u64()))
                .sum::<u64>();
                if parent.is_none() && used > 0 {
                    emit(
                        ctx,
                        Event::Status {
                            context: Some(ContextUse {
                                used_tokens: used,
                                window_tokens: st.context_window,
                            }),
                            rate_limit: None,
                            background: None,
                            agents: None,
                        },
                    );
                }
            }
        }
        _ => {}
    }
}

fn on_assistant(ctx: &SessionCtx, st: &mut State, msg: &Value) {
    let parent = parent_of(msg);
    let turn = turn_for(ctx, st, &parent);
    let m = &msg["message"];
    let mid = m["id"].as_str().unwrap_or_default().to_string();
    if parent.is_none() {
        if let Some(u) = msg["uuid"].as_str() {
            st.anchor = Some(u.to_string());
        }
        let u = &m["usage"];
        let used = [
            "input_tokens",
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
            "output_tokens",
        ]
        .iter()
        .filter_map(|k| u[*k].as_u64())
        .sum::<u64>();
        if used > 0 {
            emit(
                ctx,
                Event::Status {
                    context: Some(ContextUse {
                        used_tokens: used,
                        window_tokens: st.context_window,
                    }),
                    rate_limit: None,
                    background: None,
                    agents: None,
                },
            );
        }
    }
    if let Some(err) = msg["error"].as_str() {
        let text = m["content"][0]["text"].as_str().unwrap_or(err);
        if parent.is_none() {
            let kind = msg["api_error"].as_str().unwrap_or(err);
            st.api_error = Some((kind.to_string(), text.to_string()));
            return;
        }
        emit(
            ctx,
            Event::Notice {
                level: NoticeLevel::Error,
                text: text.to_string(),
            },
        );
        return;
    }
    let Some(content) = m["content"].as_array() else {
        return;
    };
    for block in content {
        match block["type"].as_str().unwrap_or("") {
            "text" | "thinking" | "redacted_thinking" => {
                let n = st.message_blocks.entry(mid.clone()).or_insert(0);
                let id = format!("{mid}:{n}");
                *n += 1;
                let body = if block["type"] == "text" {
                    ItemBody::AgentMessage {
                        text: block["text"].as_str().unwrap_or_default().to_string(),
                    }
                } else {
                    ItemBody::Reasoning {
                        text: block["thinking"].as_str().unwrap_or_default().to_string(),
                    }
                };
                emit(
                    ctx,
                    Event::ItemCompleted {
                        turn: turn.clone(),
                        item: Item {
                            id,
                            parent: parent.clone(),
                            body,
                        },
                    },
                );
            }
            "tool_use" | "server_tool_use" => {
                let id = block["id"].as_str().unwrap_or_default().to_string();
                let name = block["name"].as_str().unwrap_or_default().to_string();
                let input = block["input"].clone();
                st.tools.insert(
                    id.clone(),
                    ToolInfo {
                        name: name.clone(),
                        input: input.clone(),
                        parent: parent.clone(),
                    },
                );
                emit(
                    ctx,
                    Event::ItemStarted {
                        turn: turn.clone(),
                        item: Item {
                            id,
                            parent: parent.clone(),
                            body: ItemBody::ToolCall {
                                name,
                                input,
                                output: None,
                                status: ToolStatus::Running,
                            },
                        },
                    },
                );
            }
            _ => {}
        }
    }
}

fn parse_todos(input: &Value) -> Option<Vec<Todo>> {
    let arr = input["todos"].as_array()?;
    Some(
        arr.iter()
            .map(|t| Todo {
                content: t["content"].as_str().unwrap_or_default().to_string(),
                status: t["status"].as_str().unwrap_or("pending").to_string(),
                active_form: t["activeForm"].as_str().map(String::from),
            })
            .collect(),
    )
}

fn tool_result_text(content: &Value) -> (String, Vec<InputPart>) {
    match content {
        Value::String(s) => (s.clone(), vec![]),
        Value::Array(parts) => {
            let mut text = String::new();
            let mut images = vec![];
            for p in parts {
                match p["type"].as_str().unwrap_or("") {
                    "text" => {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(p["text"].as_str().unwrap_or_default());
                    }
                    "image" => images.push(InputPart::Image {
                        media_type: p["source"]["media_type"]
                            .as_str()
                            .unwrap_or("image/png")
                            .to_string(),
                        data: p["source"]["data"].as_str().unwrap_or_default().to_string(),
                    }),
                    _ => {}
                }
            }
            (text, images)
        }
        Value::Null => (String::new(), vec![]),
        other => (other.to_string(), vec![]),
    }
}

fn on_user(ctx: &SessionCtx, st: &mut State, msg: &Value) {
    if msg["isReplay"].as_bool().unwrap_or(false) {
        return;
    }
    let Some(content) = msg["message"]["content"].as_array() else {
        return;
    };
    let rejected: Vec<String> = msg["tool_result_meta"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|m| m["permission_decision"]["decision"] == "reject")
                .filter_map(|m| m["id"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    for block in content {
        if block["type"] != "tool_result" {
            continue;
        }
        let turn = turn_for(ctx, st, &parent_of(msg));
        let id = block["tool_use_id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let is_error = block["is_error"].as_bool().unwrap_or(false);
        let (text, images) = tool_result_text(&block["content"]);
        let info = st.tools.remove(&id);
        let (name, input, parent) = match info {
            Some(t) => (t.name, t.input, t.parent),
            None => (String::new(), Value::Null, parent_of(msg)),
        };
        let status = if rejected.contains(&id) {
            ToolStatus::Denied
        } else if is_error {
            ToolStatus::Failed
        } else {
            ToolStatus::Completed
        };
        if !is_error {
            track_tasks(
                ctx,
                st,
                &turn,
                &name,
                &input,
                &msg["tool_use_result"],
                parent.clone(),
            );
        }
        emit(
            ctx,
            Event::ItemCompleted {
                turn: turn.clone(),
                item: Item {
                    id,
                    parent,
                    body: ItemBody::ToolCall {
                        name,
                        input,
                        output: Some(ToolOutput {
                            text,
                            is_error,
                            images,
                        }),
                        status,
                    },
                },
            },
        );
    }
}

fn track_tasks(
    ctx: &SessionCtx,
    st: &mut State,
    turn: &str,
    tool: &str,
    input: &Value,
    result: &Value,
    parent: Option<String>,
) {
    if parent.is_some() {
        return;
    }
    let changed = match tool {
        "TodoWrite" => match parse_todos(input) {
            Some(todos) => {
                st.tasks = todos
                    .into_iter()
                    .enumerate()
                    .map(|(i, t)| (i.to_string(), t))
                    .collect();
                true
            }
            None => false,
        },
        "TaskCreate" => {
            let id = result["task"]["id"]
                .as_str()
                .map(String::from)
                .or_else(|| result["id"].as_str().map(String::from))
                .or_else(|| result["taskId"].as_str().map(String::from))
                .unwrap_or_else(|| (st.tasks.len() + 1).to_string());
            st.tasks.push((
                id,
                Todo {
                    content: input["subject"].as_str().unwrap_or_default().to_string(),
                    status: "pending".into(),
                    active_form: input["activeForm"].as_str().map(String::from),
                },
            ));
            true
        }
        "TaskUpdate" => {
            let id = input["taskId"]
                .as_str()
                .map(String::from)
                .unwrap_or_else(|| input["taskId"].to_string());
            if input["status"] == "deleted" {
                st.tasks.retain(|(i, _)| *i != id);
                true
            } else if let Some((_, t)) = st.tasks.iter_mut().find(|(i, _)| *i == id) {
                if let Some(s) = input["status"].as_str() {
                    t.status = s.to_string();
                }
                if let Some(s) = input["subject"].as_str() {
                    t.content = s.to_string();
                }
                if let Some(s) = input["activeForm"].as_str() {
                    t.active_form = Some(s.to_string());
                }
                true
            } else {
                false
            }
        }
        _ => false,
    };
    if changed {
        emit(
            ctx,
            Event::ItemCompleted {
                turn: turn.to_string(),
                item: Item {
                    id: format!("{turn}:todos"),
                    parent: None,
                    body: ItemBody::Todos {
                        todos: st.tasks.iter().map(|(_, t)| t.clone()).collect(),
                    },
                },
            },
        );
    }
}

fn on_result(ctx: &SessionCtx, st: &mut State, msg: &Value) {
    if let Some(models) = msg["modelUsage"].as_object()
        && let Some(w) = models
            .values()
            .filter_map(|m| m["contextWindow"].as_u64())
            .max()
    {
        st.context_window = Some(w);
    }
    let users: Vec<&str> = msg["user_message_uuids"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    if st.knows_users && st.user_turn && st.turn.as_deref().is_some_and(|t| !users.contains(&t)) {
        st.streams.clear();
        st.message_blocks.clear();
        return;
    }
    let Some(turn) = st.turn.take() else {
        return;
    };
    close_running_tools(ctx, st, &turn);
    st.streams.clear();
    st.message_blocks.clear();
    let u = &msg["usage"];
    let cost = msg["model_usage_since_last_result"].as_object().map(|m| {
        m.values()
            .filter_map(|v| v["costUSD"].as_f64())
            .sum::<f64>()
    });
    let usage = Usage {
        input_tokens: u["input_tokens"].as_u64().unwrap_or(0),
        output_tokens: u["output_tokens"].as_u64().unwrap_or(0),
        cache_read_tokens: u["cache_read_input_tokens"].as_u64().unwrap_or(0),
        cache_write_tokens: u["cache_creation_input_tokens"].as_u64().unwrap_or(0),
        cost_usd: cost,
        duration_ms: msg["duration_ms"].as_u64(),
    };
    let subtype = msg["subtype"].as_str().unwrap_or("");
    let is_error = msg["is_error"].as_bool().unwrap_or(false);
    let terminal = msg["terminal_reason"].as_str().unwrap_or("");
    let api_error = st.api_error.take();
    let mut resets_at = None;
    let (outcome, error) = if subtype == "success" && !is_error {
        (TurnOutcome::Completed, None)
    } else if terminal.starts_with("aborted") {
        (TurnOutcome::Interrupted, None)
    } else {
        let text = msg["result"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(String::from)
            .or_else(|| msg["errors"][0].as_str().map(String::from))
            .or_else(|| api_error.as_ref().map(|(_, t)| t.clone()))
            .unwrap_or_else(|| subtype.to_string());
        let kind = msg["api_error"]
            .as_str()
            .map(String::from)
            .or_else(|| api_error.map(|(k, _)| k))
            .unwrap_or_default();
        resets_at = limit_reset(st, &kind);
        (TurnOutcome::Failed, Some(text))
    };
    emit(
        ctx,
        Event::TurnCompleted {
            turn: turn.clone(),
            outcome,
            usage: Some(usage),
            error,
            anchor: st.anchor.clone(),
            resets_at,
        },
    );
    let merged: Vec<String> = msg["user_message_uuids"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    while st
        .queued_turns
        .front()
        .is_some_and(|(t, _)| merged.contains(t))
    {
        st.queued_turns.pop_front();
    }
    if let Some((turn, input)) = st.queued_turns.pop_front() {
        start_turn(ctx, st, turn, input);
    } else {
        let status = if outcome == TurnOutcome::Failed {
            ThreadStatus::Error
        } else {
            ThreadStatus::Idle
        };
        let _ = ctx.out.send(AgentOut::Status(status));
    }
}

async fn on_control_request(
    ctx: &SessionCtx,
    st: &mut State,
    proc: &mut Proc,
    msg: &Value,
) -> Result<()> {
    let id = msg["request_id"].as_str().unwrap_or_default().to_string();
    let req = &msg["request"];
    match req["subtype"].as_str().unwrap_or("") {
        "can_use_tool" => {
            let tool = req["tool_name"].as_str().unwrap_or_default().to_string();
            let input = req["input"].clone();
            let turn = current_turn(ctx, st);
            let request = match tool.as_str() {
                "AskUserQuestion" => AgentRequest::Question {
                    questions: parse_questions(&input),
                },
                "ExitPlanMode" => AgentRequest::PlanApproval {
                    plan: input["plan"].as_str().unwrap_or_default().to_string(),
                },
                _ => AgentRequest::ToolApproval {
                    tool: tool.clone(),
                    input: input.clone(),
                    reason: req["decision_reason"]
                        .as_str()
                        .or(req["reason"].as_str())
                        .map(String::from),
                    tool_call: req["tool_use_id"].as_str().map(String::from),
                },
            };
            st.permissions.insert(
                id.clone(),
                PendingPermission {
                    request_id: id.clone(),
                    tool,
                    input,
                    suggestions: req["permission_suggestions"].clone(),
                },
            );
            emit(ctx, Event::Request { id, turn, request });
            let _ = ctx.out.send(AgentOut::Status(ThreadStatus::Waiting));
        }
        "mcp_message" if req["server_name"] == "corra" => {
            on_mcp(ctx, st, proc, id, req["message"].clone()).await?;
        }
        other => {
            proc.write(&json!({
                "type": "control_response",
                "response": { "subtype": "error", "request_id": id, "error": format!("corra doesn't handle {other}") }
            }))
            .await?;
        }
    }
    Ok(())
}

fn parse_questions(input: &Value) -> Vec<Question> {
    input["questions"]
        .as_array()
        .map(|qs| {
            qs.iter()
                .map(|q| Question {
                    question: q["question"].as_str().unwrap_or_default().to_string(),
                    header: q["header"].as_str().map(String::from),
                    options: q["options"]
                        .as_array()
                        .map(|os| {
                            os.iter()
                                .map(|o| QuestionOption {
                                    label: o["label"].as_str().unwrap_or_default().to_string(),
                                    description: o["description"].as_str().map(String::from),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    multi_select: q["multiSelect"].as_bool().unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn resolve_permission(
    ctx: &SessionCtx,
    st: &mut State,
    proc: &mut Proc,
    id: &str,
    response: RequestResponse,
) -> Result<()> {
    let Some(p) = st.permissions.remove(id) else {
        return Ok(());
    };
    let body = match &response {
        RequestResponse::Allow { .. } if p.tool == "ExitPlanMode" => json!({
            "behavior": "allow",
            "updatedInput": p.input,
            "updatedPermissions": [{ "type": "setMode", "mode": "acceptEdits", "destination": "session" }],
        }),
        RequestResponse::Allow { always } => {
            let mut r = json!({ "behavior": "allow", "updatedInput": p.input });
            if *always {
                let rules: Vec<Value> = p
                    .suggestions
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter(|x| x["type"] != "setMode")
                            .map(|x| {
                                let mut x = x.clone();
                                x["destination"] = json!("session");
                                x
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                r["updatedPermissions"] = if rules.is_empty() {
                    json!([{ "type": "addRules", "rules": [{ "toolName": p.tool }], "behavior": "allow", "destination": "session" }])
                } else {
                    Value::Array(rules)
                };
            }
            r
        }
        RequestResponse::Answers { answers } => {
            let mut input = p.input.clone();
            let mut map = serde_json::Map::new();
            for (q, a) in parse_questions(&p.input).iter().zip(answers) {
                map.insert(q.question.clone(), Value::String(a.join(", ")));
            }
            input["answers"] = Value::Object(map);
            json!({ "behavior": "allow", "updatedInput": input })
        }
        RequestResponse::Deny { message } => json!({
            "behavior": "deny",
            "message": message.clone().unwrap_or_else(|| "The user declined this.".into()),
        }),
        RequestResponse::Cancelled => json!({
            "behavior": "deny",
            "message": "The request was cancelled.",
            "interrupt": true,
        }),
    };
    proc.write(&json!({
        "type": "control_response",
        "response": { "subtype": "success", "request_id": p.request_id, "response": body }
    }))
    .await?;
    emit(
        ctx,
        Event::RequestResolved {
            id: id.to_string(),
            response,
        },
    );
    if st.permissions.is_empty() && st.turn.is_some() {
        let _ = ctx.out.send(AgentOut::Status(ThreadStatus::Running));
    }
    Ok(())
}

async fn on_mcp(
    ctx: &SessionCtx,
    st: &mut State,
    proc: &mut Proc,
    req_id: String,
    msg: Value,
) -> Result<()> {
    let method = msg["method"].as_str().unwrap_or("");
    let rpc_id = msg["id"].clone();
    let reply = move |result: Value| {
        json!({
            "type": "control_response",
            "response": { "subtype": "success", "request_id": req_id.clone(), "response": {
                "mcp_response": { "jsonrpc": "2.0", "id": if rpc_id.is_null() { json!(0) } else { rpc_id.clone() }, "result": result }
            }}
        })
    };
    match method {
        "initialize" => {
            let version = msg["params"]["protocolVersion"]
                .as_str()
                .unwrap_or("2025-06-18");
            proc.write(&reply(json!({
                "protocolVersion": version,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "corra", "version": env!("CARGO_PKG_VERSION") }
            })))
            .await?;
        }
        "tools/list" => {
            let tools: Vec<Value> = ctx
                .spec
                .tools
                .iter()
                .map(|t| json!({ "name": t.mcp_name(), "description": t.description, "inputSchema": t.input_schema }))
                .collect();
            proc.write(&reply(json!({ "tools": tools }))).await?;
        }
        "tools/call" => {
            let name = msg["params"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let args = msg["params"]["arguments"].clone();
            let Some(def) = ctx.spec.tools.iter().find(|t| t.mcp_name() == name) else {
                proc.write(&reply(json!({ "content": [{ "type": "text", "text": format!("unknown tool {name}") }], "isError": true })))
                    .await?;
                return Ok(());
            };
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = ctx.hub.send(super::HubCall {
                thread: ctx.spec.thread,
                turn: st.turn.clone().unwrap_or_default(),
                action: def.action.clone(),
                input: if args.is_null() { json!({}) } else { args },
                reply: tx,
            });
            let deferred = st.deferred.clone();
            let respond = move |result: Value| {
                if let Some(d) = &deferred {
                    let _ = d.send(reply(result));
                }
            };
            tokio::spawn(async move {
                let result = match rx.await {
                    Ok(Ok(v)) => match (v["image"]["data"].as_str(), v["image"]["mime"].as_str()) {
                        (Some(data), Some(mime)) => json!({ "content": [
                            { "type": "text", "text": v["text"].as_str().unwrap_or_default() },
                            { "type": "image", "data": data, "mimeType": mime }
                        ] }),
                        _ => {
                            let text = match &v {
                                Value::String(s) => s.clone(),
                                other => serde_json::to_string_pretty(other).unwrap_or_default(),
                            };
                            json!({ "content": [{ "type": "text", "text": text }] })
                        }
                    },
                    Ok(Err(e)) => {
                        json!({ "content": [{ "type": "text", "text": e }], "isError": true })
                    }
                    Err(_) => {
                        json!({ "content": [{ "type": "text", "text": "the call was dropped" }], "isError": true })
                    }
                };
                respond(result);
            });
        }
        _ => {
            proc.write(&reply(json!({}))).await?;
        }
    }
    Ok(())
}

pub fn config_dir() -> std::path::PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| crate::files::home().join(".claude"))
}

fn java_hash(s: &str) -> i32 {
    let mut h: i32 = 0;
    for u in s.encode_utf16() {
        h = h.wrapping_shl(5).wrapping_sub(h).wrapping_add(u as i32);
    }
    h
}

fn to_base36(mut n: u64) -> String {
    const D: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".into();
    }
    let mut out = vec![];
    while n > 0 {
        out.push(D[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

pub fn project_slug(cwd: &str) -> String {
    let n: String = cwd
        .encode_utf16()
        .map(|u| {
            if u < 128 && (u as u8).is_ascii_alphanumeric() {
                u as u8 as char
            } else {
                '-'
            }
        })
        .collect();
    if n.len() <= 200 {
        n
    } else {
        format!(
            "{}-{}",
            &n[..200],
            to_base36(java_hash(cwd).unsigned_abs() as u64)
        )
    }
}

pub fn session_base(cwd: &std::path::Path) -> std::path::PathBuf {
    config_dir()
        .join("projects")
        .join(project_slug(&cwd.to_string_lossy()))
}

pub fn session_files(cwd: &std::path::Path, session_id: &str) -> Vec<std::path::PathBuf> {
    let base = session_base(cwd);
    let mut out = vec![];
    let main = base.join(format!("{session_id}.jsonl"));
    if main.exists() {
        out.push(main);
    }
    crate::sync::walk(&base.join(session_id), &mut out);
    out
}
