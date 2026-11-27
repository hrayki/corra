use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use corra_proto::*;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

use super::{AgentOut, SessionCmd, SessionCtx, mcp};

const MANIFEST: &str =
    "https://antigravity-cli-auto-updater-974169037036.us-central1.run.app/manifests";
const STDERR_LINES: usize = 15;
const WAKE_QUIET: Duration = Duration::from_secs(5);

pub async fn run(mut ctx: SessionCtx) {
    let error = drive(&mut ctx).await.err().map(|e| format!("{e:#}"));
    if let Some(e) = &error {
        emit(
            &ctx,
            Event::Notice {
                level: NoticeLevel::Error,
                text: format!("Antigravity stopped: {e}"),
            },
        );
    }
    let _ = ctx.out.send(AgentOut::Exited { error });
}

fn emit(ctx: &SessionCtx, e: Event) {
    let _ = ctx.out.send(AgentOut::Event(e));
}

pub fn installed() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CORRA_AGY_BIN") {
        return Some(PathBuf::from(p));
    }
    let own = crate::files::home().join(".corra/agy/agy");
    if own.exists() {
        return Some(own);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join("agy"))
        .find(|p| p.is_file())
}

async fn install(ctx: &SessionCtx) -> Result<PathBuf> {
    if let Some(p) = installed() {
        return Ok(p);
    }
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        a => bail!("Antigravity CLI has no build for {a}"),
    };
    let client = reqwest::Client::new();
    let manifest: Value = client
        .get(format!("{MANIFEST}/linux_{arch}.json"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
        .context("reading the Antigravity CLI release manifest")?;
    let (Some(url), Some(sha)) = (manifest["url"].as_str(), manifest["sha512"].as_str()) else {
        bail!("the Antigravity CLI release manifest has no download");
    };
    emit(
        ctx,
        Event::Notice {
            level: NoticeLevel::Info,
            text: format!(
                "Installing Antigravity CLI {} on this machine.",
                manifest["version"].as_str().unwrap_or("")
            ),
        },
    );
    let bytes = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    use sha2::Digest;
    let got: String = sha2::Sha512::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if !got.eq_ignore_ascii_case(sha) {
        bail!("the Antigravity CLI download doesn't match its checksum");
    }
    let dir = crate::files::home().join(".corra/agy");
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join("agy.new");
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(&bytes[..]));
    let mut found = false;
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry
            .path()?
            .file_name()
            .is_some_and(|n| n == "antigravity")
        {
            entry.unpack(&tmp)?;
            found = true;
            break;
        }
    }
    if !found {
        bail!("the Antigravity CLI download has no antigravity binary");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    let bin = dir.join("agy");
    std::fs::rename(&tmp, &bin)?;
    Ok(bin)
}

fn env_for(cmd: &mut Command, ctx: &SessionCtx) {
    cmd.env_remove("CORRA_TOKEN")
        .env_remove("CORRA_TOKEN_FILE")
        .env_remove("GEMINI_API_KEY")
        .env("AGY_ACP_FORCE_FILE_STORAGE", "1")
        .env("AGY_CLI_DISABLE_AUTO_UPDATE", "true");
    for (k, v) in &ctx.spec.agent_env {
        cmd.env(k, v);
    }
}

fn has_key(ctx: &SessionCtx) -> bool {
    ctx.spec
        .agent_env
        .iter()
        .any(|(k, v)| k == "GEMINI_API_KEY" && !v.is_empty())
}

type Reader = Box<dyn tokio::io::AsyncRead + Unpin + Send>;
type Writer = Box<dyn tokio::io::AsyncWrite + Unpin + Send>;

struct Acp {
    child: Option<Child>,
    stdin: Writer,
    lines: tokio::io::Lines<BufReader<Reader>>,
    next: u64,
    stderr: std::sync::Arc<std::sync::Mutex<VecDeque<String>>>,
    early: VecDeque<Value>,
    rec: Option<super::record::Recorder>,
}

impl Acp {
    fn spawn(bin: &PathBuf, ctx: &SessionCtx) -> Result<Acp> {
        let mut cmd = Command::new(bin);
        cmd.arg("acp")
            .current_dir(&ctx.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        env_for(&mut cmd, ctx);
        #[cfg(target_os = "linux")]
        unsafe {
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                Ok(())
            });
        }
        let mut child = cmd.spawn().context("starting agy acp")?;
        let stdin: Writer = Box::new(child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?);
        let stdout: Reader = Box::new(child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?);
        let tail = std::sync::Arc::new(std::sync::Mutex::new(VecDeque::new()));
        if let Some(stderr) = child.stderr.take() {
            let tail = tail.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    tracing::debug!(target: "corra_runner::antigravity", "stderr: {l}");
                    if l.trim().is_empty() {
                        continue;
                    }
                    let mut t = tail.lock().unwrap();
                    if t.len() == STDERR_LINES {
                        t.pop_front();
                    }
                    t.push_back(l.chars().take(400).collect());
                }
            });
        }
        let rec = super::record::Recorder::open(
            ctx.spec.thread,
            json!({ "agent": "antigravity", "settings": ctx.spec.settings }),
        );
        Ok(Acp {
            child: Some(child),
            stdin,
            lines: BufReader::new(stdout).lines(),
            next: 0,
            stderr: tail,
            early: VecDeque::new(),
            rec,
        })
    }

    #[cfg(test)]
    fn piped(reader: Reader, writer: Writer) -> Acp {
        Acp {
            child: None,
            stdin: writer,
            lines: BufReader::new(reader).lines(),
            next: 0,
            stderr: Default::default(),
            early: VecDeque::new(),
            rec: None,
        }
    }

    async fn write(&mut self, v: &Value) -> Result<()> {
        if let Some(r) = &mut self.rec {
            r.runner(v);
        }
        let mut line = v.to_string();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<u64> {
        self.next += 1;
        let id = self.next;
        self.write(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await?;
        Ok(id)
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.write(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
            .await
    }

    async fn reply(&mut self, id: &Value, result: Value) -> Result<()> {
        self.write(&json!({ "jsonrpc": "2.0", "id": id, "result": result }))
            .await
    }

    async fn read(&mut self) -> Result<Option<Value>> {
        loop {
            let Some(line) = self.lines.next_line().await? else {
                return Ok(None);
            };
            if line.trim().is_empty() {
                continue;
            }
            if let Some(r) = &mut self.rec {
                r.claude(&line);
            }
            match serde_json::from_str::<Value>(&line) {
                Ok(v) => return Ok(Some(v)),
                Err(_) => tracing::debug!("non-json from agy: {line}"),
            }
        }
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Result<Value, (i64, String)>> {
        let id = self.request(method, params).await?;
        loop {
            let Some(msg) = self.read().await? else {
                bail!("agy closed before answering {method}");
            };
            if msg["id"].as_u64() == Some(id) && msg.get("method").is_none() {
                if let Some(e) = msg.get("error") {
                    return Ok(Err((
                        e["code"].as_i64().unwrap_or(0),
                        e["message"].as_str().unwrap_or("").to_string(),
                    )));
                }
                return Ok(Ok(msg["result"].clone()));
            }
            self.early.push_back(msg);
        }
    }

    fn tail(&self) -> String {
        self.stderr
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    async fn kill(&mut self) {
        if let Some(c) = &mut self.child {
            let _ = c.kill().await;
        }
    }
}

#[derive(Default)]
struct Choices {
    model: Option<(String, Vec<Choice>, String)>,
    effort: Option<(String, Vec<Choice>, String)>,
    mode: Option<(String, Vec<Choice>, String)>,
    modes: Vec<Choice>,
    mode_now: Option<String>,
    variants: Vec<Choice>,
    effort_names: HashMap<String, String>,
}

fn flat(options: &Value) -> Vec<Choice> {
    let mut out = vec![];
    for o in options.as_array().into_iter().flatten() {
        if let Some(inner) = o.get("options") {
            out.extend(flat(inner));
        } else if let Some(v) = o["value"].as_str().or(o["id"].as_str()) {
            out.push(Choice {
                id: v.to_string(),
                name: o["name"].as_str().unwrap_or(v).to_string(),
                hint: o["description"].as_str().map(String::from),
                ..Default::default()
            });
        }
    }
    out
}

impl Choices {
    fn read(&mut self, result: &Value) {
        if let Some(opts) = result["configOptions"].as_array() {
            for o in opts {
                let id = o["id"].as_str().unwrap_or_default().to_string();
                let cat = o["category"].as_str().unwrap_or_default();
                let entry = (
                    id.clone(),
                    flat(&o["options"]),
                    o["currentValue"].as_str().unwrap_or_default().to_string(),
                );
                if id == "model" || cat == "model" {
                    self.model = Some(entry);
                } else if cat == "thought_level"
                    || id.contains("thought")
                    || id.contains("thinking")
                {
                    for c in &entry.1 {
                        self.effort_names.insert(c.id.clone(), c.name.clone());
                    }
                    self.effort = Some(entry);
                } else if cat == "mode" {
                    self.mode = Some(entry);
                }
            }
        }
        if let Some(modes) = result["modes"]["availableModes"].as_array() {
            self.modes = modes
                .iter()
                .filter_map(|m| {
                    Some(Choice {
                        id: m["id"].as_str()?.to_string(),
                        name: m["name"].as_str().unwrap_or_default().to_string(),
                        hint: m["description"].as_str().map(String::from),
                        ..Default::default()
                    })
                })
                .collect();
            self.mode_now = result["modes"]["currentModeId"].as_str().map(String::from);
        }
        if let Some(list) = result["models"]["availableModels"].as_array() {
            self.variants = list
                .iter()
                .filter_map(|m| {
                    Some(Choice {
                        id: m["modelId"].as_str()?.to_string(),
                        name: m["name"].as_str().unwrap_or_default().to_string(),
                        ..Default::default()
                    })
                })
                .collect();
        }
    }

    fn efforts_of(&self, model: &str) -> Vec<Choice> {
        if let (Some((_, _, now)), Some((_, list, _))) = (&self.model, &self.effort)
            && now == model
        {
            return list.clone();
        }
        let Some(name) = self
            .model
            .as_ref()
            .and_then(|(_, l, _)| l.iter().find(|c| c.id == model))
            .map(|c| c.name.clone())
        else {
            return vec![];
        };
        let mut out: Vec<Choice> = self
            .variants
            .iter()
            .filter_map(|v| {
                let id = v.id.strip_prefix(model)?.strip_prefix('-')?;
                let label = v
                    .name
                    .strip_prefix(name.as_str())?
                    .strip_prefix(" (")?
                    .strip_suffix(')')?;
                Some(Choice {
                    id: id.to_string(),
                    name: self
                        .effort_names
                        .get(id)
                        .cloned()
                        .unwrap_or_else(|| label.to_string()),
                    ..Default::default()
                })
            })
            .collect();
        out.sort_by_key(|c| rank(&c.id).unwrap_or(usize::MAX));
        out
    }

    fn mode_list(&self) -> Vec<Choice> {
        match &self.mode {
            Some((_, list, _)) => list.clone(),
            None => self.modes.clone(),
        }
    }

    fn mode_is(&mut self, now: Option<String>) -> bool {
        if let (Some(m), Some((_, _, cur))) = (&now, self.mode.as_mut()) {
            *cur = m.clone();
        }
        let changed = now != self.mode_now;
        self.mode_now = now;
        changed
    }

    fn mode_current(&self) -> Option<String> {
        match &self.mode {
            Some((_, _, now)) if !now.is_empty() => Some(now.clone()),
            _ => self.mode_now.clone(),
        }
    }

    fn mode_ids(&self) -> Vec<String> {
        match &self.mode {
            Some((_, c, _)) => c.iter().map(|c| c.id.clone()).collect(),
            None => self.modes.iter().map(|c| c.id.clone()).collect(),
        }
    }
}

fn mode_for(permission: &str, available: &[String]) -> Option<String> {
    available.iter().find(|a| *a == permission).cloned()
}

const RANKS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

fn rank(id: &str) -> Option<usize> {
    RANKS.iter().position(|r| r.eq_ignore_ascii_case(id))
}

fn effort_for(effort: &str, available: &[Choice]) -> Option<String> {
    if let Some(c) = available
        .iter()
        .find(|c| c.id.eq_ignore_ascii_case(effort) || c.name.eq_ignore_ascii_case(effort))
    {
        return Some(c.id.clone());
    }
    let want = rank(effort)?;
    let ranked: Vec<(usize, &Choice)> = available
        .iter()
        .filter_map(|c| Some((rank(&c.id)?, c)))
        .collect();
    ranked
        .iter()
        .filter(|(r, _)| *r <= want)
        .max_by_key(|(r, _)| *r)
        .or_else(|| ranked.iter().min_by_key(|(r, _)| *r))
        .map(|(_, c)| c.id.clone())
}

struct Tool {
    name: String,
    input: Value,
    output: String,
}

struct Ask {
    rpc: Value,
    options: Vec<Value>,
    question: bool,
}

struct State {
    session: String,
    settings: AgentSettings,
    choices: Choices,
    turn: Option<String>,
    last_turn: Option<String>,
    prompts: Vec<u64>,
    stop: Option<String>,
    failed: Option<String>,
    text: Option<(String, String)>,
    thought: Option<(String, String)>,
    n: u64,
    tools: HashMap<String, Tool>,
    asks: HashMap<String, Ask>,
    pending: HashMap<u64, Value>,
    held: VecDeque<(String, Vec<InputPart>)>,
    usage: Option<ContextUse>,
    wake_until: Option<tokio::time::Instant>,
    slash: Vec<String>,
}

pub fn store_dir() -> PathBuf {
    crate::files::home().join(".gemini/antigravity-cli")
}

pub fn session_files(session: &str) -> Vec<PathBuf> {
    let base = store_dir();
    let mut out = vec![];
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let p = base.join(format!("conversations/{session}.db{suffix}"));
        if p.exists() {
            out.push(p);
        }
    }
    let brain = base.join("brain").join(session);
    let mut stack = vec![brain];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}

async fn drive(ctx: &mut SessionCtx) -> Result<()> {
    let bin = install(ctx).await?;
    let turn = mcp::CurrentTurn::default();
    let server = if ctx.spec.tools.is_empty() {
        None
    } else {
        Some(mcp::serve(
            ctx.spec.thread,
            ctx.spec.tools.clone(),
            ctx.hub.clone(),
            turn.clone(),
        )?)
    };
    let mut servers = vec![];
    if let Some(s) = &server {
        servers.push(json!({
            "name": "corra",
            "command": std::env::current_exe()?.to_string_lossy(),
            "args": ["mcp-bridge", s.path.to_string_lossy()],
            "env": [],
        }));
    }
    let mut acp = Acp::spawn(&bin, ctx)?;
    let result = talk(ctx, &mut acp, &bin, servers, turn).await;
    acp.kill().await;
    drop(server);
    result.map_err(|e| {
        let tail = acp.tail();
        if tail.is_empty() {
            e
        } else {
            anyhow!("{e:#}. The end of its output:\n\n```\n{tail}\n```")
        }
    })
}

async fn talk(
    ctx: &mut SessionCtx,
    acp: &mut Acp,
    bin: &PathBuf,
    servers: Vec<Value>,
    current: mcp::CurrentTurn,
) -> Result<()> {
    let init = acp
        .call(
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false },
                "clientInfo": { "name": "corra", "version": env!("CARGO_PKG_VERSION") },
            }),
        )
        .await?
        .map_err(|(_, m)| anyhow!("agy didn't start: {m}"))?;
    tracing::info!(
        "agy {}",
        init["agentInfo"]["version"].as_str().unwrap_or("?")
    );
    let mut st = State {
        session: String::new(),
        settings: ctx.spec.settings.clone(),
        choices: Choices::default(),
        turn: None,
        last_turn: None,
        prompts: vec![],
        stop: None,
        failed: None,
        text: None,
        thought: None,
        n: 0,
        tools: HashMap::new(),
        asks: HashMap::new(),
        pending: HashMap::new(),
        held: VecDeque::new(),
        usage: None,
        wake_until: None,
        slash: vec![],
    };
    let initial = ctx.initial.take();
    if let Some(input) = &initial {
        start_turn(
            ctx,
            &mut st,
            uuid::Uuid::now_v7().to_string(),
            input.clone(),
        );
        *current.lock().unwrap() = st.turn.clone();
    } else {
        let _ = ctx.out.send(AgentOut::Status(ThreadStatus::Idle));
    }
    authenticate(ctx, acp, bin, &mut st).await?;
    open_session(ctx, acp, &mut st, servers).await?;
    apply_settings(ctx, acp, &mut st, ctx.spec.settings.clone(), true).await?;
    while let Some(msg) = acp.early.pop_front() {
        on_message(ctx, acp, &mut st, msg).await?;
    }
    if let Some(input) = initial {
        prompt(acp, &mut st, &input).await?;
    } else if st.turn.is_none()
        && let Some((id, input)) = st.held.pop_front()
    {
        start_turn(ctx, &mut st, id, input.clone());
        prompt(acp, &mut st, &input).await?;
    }
    loop {
        *current.lock().unwrap() = st.turn.clone().or_else(|| st.last_turn.clone());
        let quiet = st.wake_until;
        tokio::select! {
            biased;
            msg = acp.read() => {
                let Some(msg) = msg? else {
                    let code = match &mut acp.child { Some(c) => c.wait().await.ok().and_then(|s| s.code()), None => None };
                    drop_held(ctx, &mut st);
                    finish_turn(ctx, &mut st, TurnOutcome::Failed, Some(format!("agy exited ({})", code.map(|c| c.to_string()).unwrap_or_else(|| "killed".into()))));
                    bail!("agy exited");
                };
                on_message(ctx, acp, &mut st, msg).await?;
            }
            cmd = ctx.cmds.recv() => {
                let Some(cmd) = cmd else { return Ok(()); };
                if let Some(r) = &mut acp.rec {
                    r.app(&cmd, false);
                }
                if !on_command(ctx, acp, &mut st, cmd).await? {
                    return Ok(());
                }
            }
            _ = async { tokio::time::sleep_until(quiet.unwrap()).await }, if quiet.is_some() => {
                st.wake_until = None;
                if st.prompts.is_empty() && st.turn.is_some() {
                    finish_turn(ctx, &mut st, TurnOutcome::Completed, None);
                    idle(ctx, &st);
                }
            }
        }
    }
}

async fn authenticate(
    ctx: &mut SessionCtx,
    acp: &mut Acp,
    bin: &PathBuf,
    st: &mut State,
) -> Result<()> {
    let method = if has_key(ctx) {
        "gemini-api-key"
    } else {
        "oauth-personal"
    };
    loop {
        match acp
            .call("authenticate", json!({ "methodId": method }))
            .await?
        {
            Ok(_) => return Ok(()),
            Err((code, msg)) if method == "gemini-api-key" => {
                bail!("Antigravity didn't take the Gemini API key ({code}): {msg}")
            }
            Err(_) => {
                if !sign_in(ctx, bin, st).await? {
                    bail!("Antigravity needs a Google sign-in on this machine");
                }
            }
        }
    }
}

async fn sign_in(ctx: &mut SessionCtx, bin: &PathBuf, st: &mut State) -> Result<bool> {
    let mut cmd = Command::new(bin);
    cmd.args(["acp", "--login"])
        .current_dir(&ctx.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    env_for(&mut cmd, ctx);
    let mut child = cmd.spawn().context("starting agy acp --login")?;
    let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
    let mut out = BufReader::new(child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?).lines();
    let url = tokio::time::timeout(Duration::from_secs(60), async {
        while let Ok(Some(l)) = out.next_line().await {
            if let Some(u) = l.split_whitespace().find(|w| w.starts_with("https://")) {
                return Some(u.to_string());
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
    .ok_or_else(|| anyhow!("agy didn't print a sign-in link"))?;
    let turn = match &st.turn {
        Some(t) => t.clone(),
        None => {
            start_wake(ctx, st, Some("Antigravity needs you to sign in.".into()));
            st.turn.clone().unwrap()
        }
    };
    let request = format!("signin-{}", uuid::Uuid::now_v7());
    emit(
        ctx,
        Event::Request {
            id: request.clone(),
            turn,
            request: AgentRequest::HubAction {
                action: "agent.sign_in".into(),
                input: json!({ "agent": "Antigravity", "url": url }),
            },
        },
    );
    let _ = ctx.out.send(AgentOut::Status(ThreadStatus::Waiting));
    loop {
        let Some(cmd) = ctx.cmds.recv().await else {
            return Ok(false);
        };
        match cmd {
            SessionCmd::SignIn { request: r, code } if r == request => {
                stdin
                    .write_all(format!("{}\n", code.trim()).as_bytes())
                    .await?;
                stdin.flush().await?;
                let status = tokio::time::timeout(Duration::from_secs(90), child.wait()).await;
                let _ = ctx.out.send(AgentOut::Status(ThreadStatus::Running));
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    let _ =
                        tokio::time::timeout(Duration::from_secs(2), e.read_to_string(&mut err))
                            .await;
                }
                return match status {
                    Ok(Ok(s)) if s.success() => {
                        emit(
                            ctx,
                            Event::Notice {
                                level: NoticeLevel::Info,
                                text: "Signed in to Antigravity on this machine.".into(),
                            },
                        );
                        Ok(true)
                    }
                    _ => {
                        let _ = child.kill().await;
                        emit(
                            ctx,
                            Event::Notice {
                                level: NoticeLevel::Warning,
                                text: format!(
                                    "That sign-in didn't work{}. Try again with a new link.",
                                    err.lines()
                                        .last()
                                        .map(|l| l.trim().trim_end_matches('.'))
                                        .filter(|l| !l.is_empty())
                                        .map(|l| format!(": {l}"))
                                        .unwrap_or_default()
                                ),
                            },
                        );
                        Box::pin(sign_in(ctx, bin, st)).await
                    }
                };
            }
            SessionCmd::Interrupt | SessionCmd::Stop => {
                let _ = child.kill().await;
                emit(
                    ctx,
                    Event::RequestResolved {
                        id: request,
                        response: RequestResponse::Cancelled,
                    },
                );
                drop_held(ctx, st);
                finish_turn(ctx, st, TurnOutcome::Interrupted, None);
                return Ok(false);
            }
            SessionCmd::Send { input, .. } => hold(ctx, st, input),
            SessionCmd::Settings(s) => st.settings.merge(&s),
            _ => {}
        }
    }
}

async fn open_session(
    ctx: &SessionCtx,
    acp: &mut Acp,
    st: &mut State,
    servers: Vec<Value>,
) -> Result<()> {
    let dirs: Vec<String> = ctx
        .extra_dirs
        .iter()
        .map(|d| d.to_string_lossy().to_string())
        .collect();
    let cwd = ctx.cwd.to_string_lossy().to_string();
    if let Some(id) = &ctx.spec.resume_session {
        let params = json!({ "sessionId": id, "cwd": cwd, "mcpServers": servers, "additionalDirectories": dirs });
        match acp.call("session/resume", params).await? {
            Ok(r) => {
                st.session = id.clone();
                st.choices.read(&r);
                announce(ctx, st);
                return Ok(());
            }
            Err((_, m)) => emit(
                ctx,
                Event::Notice {
                    level: NoticeLevel::Warning,
                    text: format!(
                        "Antigravity couldn't reopen this conversation ({m}), so it starts a new one."
                    ),
                },
            ),
        }
    }
    let r = acp
        .call(
            "session/new",
            json!({ "cwd": cwd, "mcpServers": servers, "additionalDirectories": dirs }),
        )
        .await?
        .map_err(|(_, m)| anyhow!("Antigravity didn't open a conversation: {m}"))?;
    st.session = r["sessionId"].as_str().unwrap_or_default().to_string();
    if st.session.is_empty() {
        bail!("Antigravity opened a conversation without an id");
    }
    st.choices.read(&r);
    announce(ctx, st);
    Ok(())
}

fn announce(ctx: &SessionCtx, st: &State) {
    let pick = |c: &Option<(String, Vec<Choice>, String)>| {
        c.as_ref().map(|(_, l, _)| l.clone()).unwrap_or_default()
    };
    let models = pick(&st.choices.model)
        .into_iter()
        .map(|c| {
            let efforts = st.choices.efforts_of(&c.id);
            Choice {
                efforts: (!efforts.is_empty()).then_some(efforts),
                ..c
            }
        })
        .collect();
    emit(
        ctx,
        Event::Session {
            session_id: st.session.clone(),
            model: st
                .choices
                .model
                .as_ref()
                .map(|(_, _, v)| v.clone())
                .filter(|v| !v.is_empty()),
            cwd: ctx.cwd.to_string_lossy().to_string(),
            tools: vec![],
            slash_commands: st.slash.clone(),
            permission_mode: st.choices.mode_current(),
            models,
            efforts: pick(&st.choices.effort),
            modes: st.choices.mode_list(),
        },
    );
}

async fn set_option(acp: &mut Acp, st: &mut State, config: &str, value: &str) -> Result<()> {
    let id = acp
        .request(
            "session/set_config_option",
            json!({ "sessionId": st.session, "configId": config, "value": value }),
        )
        .await?;
    st.pending.insert(id, json!({ "config": config }));
    Ok(())
}

async fn apply_settings(
    ctx: &SessionCtx,
    acp: &mut Acp,
    st: &mut State,
    s: AgentSettings,
    first: bool,
) -> Result<()> {
    let mut changed = AgentSettings::default();
    let mut switched = None;
    if let (Some(m), Some((id, list, now))) = (&s.model, st.choices.model.clone())
        && (first || Some(m) != st.settings.model.as_ref())
        && m != &now
        && list.iter().any(|c| &c.id == m)
    {
        set_option(acp, st, &id, m).await?;
        changed.model = Some(m.clone());
        switched = Some(m.clone());
    }
    if let (Some(e), Some((id, list, now))) = (&s.effort, st.choices.effort.clone()) {
        let list = match &switched {
            Some(m) => st.choices.efforts_of(m),
            None => list,
        };
        if (first || switched.is_some() || Some(e) != st.settings.effort.as_ref())
            && let Some(v) = effort_for(e, &list)
            && (switched.is_some() || v != now)
        {
            set_option(acp, st, &id, &v).await?;
            changed.effort = Some(e.clone());
        }
    }
    if let Some(p) = &s.permission_mode
        && (first || Some(p) != st.settings.permission_mode.as_ref())
        && let Some(mode) = mode_for(p, &st.choices.mode_ids())
    {
        match st.choices.mode.clone() {
            Some((id, _, now)) if now != mode => set_option(acp, st, &id, &mode).await?,
            None if st.choices.mode_now.as_deref() != Some(&mode) => {
                let rid = acp
                    .request(
                        "session/set_mode",
                        json!({ "sessionId": st.session, "modeId": mode }),
                    )
                    .await?;
                st.pending.insert(rid, json!({ "mode": mode }));
            }
            _ => {}
        }
        changed.permission_mode = Some(p.clone());
    }
    st.settings.merge(&s);
    if !first && changed != AgentSettings::default() {
        emit(ctx, Event::SettingsChanged { settings: changed });
    }
    Ok(())
}

fn content(input: &[InputPart]) -> Vec<Value> {
    input
        .iter()
        .map(|p| match p {
            InputPart::Text { text } => json!({ "type": "text", "text": text }),
            InputPart::Image { media_type, data } => {
                json!({ "type": "image", "mimeType": media_type, "data": data })
            }
        })
        .collect()
}

async fn prompt(acp: &mut Acp, st: &mut State, input: &[InputPart]) -> Result<()> {
    let id = acp
        .request(
            "session/prompt",
            json!({ "sessionId": st.session, "prompt": content(input) }),
        )
        .await?;
    st.prompts.push(id);
    st.wake_until = None;
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
    st.stop = None;
    st.failed = None;
    let _ = ctx.out.send(AgentOut::Status(ThreadStatus::Running));
}

fn start_wake(ctx: &SessionCtx, st: &mut State, cause: Option<String>) {
    let turn = format!("t{}-{}", now_ms(), st.n);
    st.n += 1;
    emit(
        ctx,
        Event::TurnStarted {
            turn: turn.clone(),
            input: vec![],
            settings: st.settings.clone(),
            cause,
        },
    );
    st.last_turn = Some(turn.clone());
    st.turn = Some(turn);
    st.stop = None;
    st.failed = None;
    let _ = ctx.out.send(AgentOut::Status(ThreadStatus::Running));
}

fn current(ctx: &SessionCtx, st: &mut State) -> String {
    if st.turn.is_none() {
        start_wake(ctx, st, Some("Continued on its own.".into()));
    }
    if st.prompts.is_empty() {
        st.wake_until = Some(tokio::time::Instant::now() + WAKE_QUIET);
    }
    st.turn.clone().unwrap()
}

fn close_text(ctx: &SessionCtx, st: &mut State) {
    close_slot(ctx, st, false);
    close_slot(ctx, st, true);
}

fn close_slot(ctx: &SessionCtx, st: &mut State, thought: bool) {
    let Some(turn) = st.turn.clone().or_else(|| st.last_turn.clone()) else {
        return;
    };
    let slot = if thought {
        &mut st.thought
    } else {
        &mut st.text
    };
    let Some((id, text)) = slot.take() else {
        return;
    };
    let body = if thought {
        ItemBody::Reasoning { text }
    } else {
        ItemBody::AgentMessage { text }
    };
    emit(
        ctx,
        Event::ItemCompleted {
            turn,
            item: Item {
                id,
                parent: None,
                body,
            },
        },
    );
}

fn finish_turn(ctx: &SessionCtx, st: &mut State, outcome: TurnOutcome, error: Option<String>) {
    close_text(ctx, st);
    let Some(turn) = st.turn.take() else {
        return;
    };
    for (id, t) in st.tools.drain() {
        emit(
            ctx,
            Event::ItemCompleted {
                turn: turn.clone(),
                item: Item {
                    id,
                    parent: None,
                    body: ItemBody::ToolCall {
                        name: t.name,
                        input: t.input,
                        output: (!t.output.is_empty()).then(|| ToolOutput {
                            text: t.output,
                            is_error: false,
                            images: vec![],
                        }),
                        status: if outcome == TurnOutcome::Completed {
                            ToolStatus::Completed
                        } else {
                            ToolStatus::Failed
                        },
                    },
                },
            },
        );
    }
    emit(
        ctx,
        Event::TurnCompleted {
            turn,
            outcome,
            usage: None,
            error,
            anchor: None,
            resets_at: None,
        },
    );
    st.wake_until = None;
}

fn show_message(ctx: &SessionCtx, turn: &str, id: &str, input: &[InputPart]) {
    emit(
        ctx,
        Event::ItemCompleted {
            turn: turn.to_string(),
            item: Item {
                id: format!("{id}:input"),
                parent: None,
                body: ItemBody::UserMessage {
                    content: input.to_vec(),
                },
            },
        },
    );
}

fn hold(ctx: &SessionCtx, st: &mut State, input: Vec<InputPart>) {
    let id = uuid::Uuid::now_v7().to_string();
    if let Some(turn) = st.turn.clone() {
        close_text(ctx, st);
        show_message(ctx, &turn, &id, &input);
    }
    st.held.push_back((id, input));
}

fn drop_held(ctx: &SessionCtx, st: &mut State) {
    for (id, _) in st.held.drain(..) {
        emit(
            ctx,
            Event::Unread {
                item: format!("{id}:input"),
            },
        );
    }
}

fn idle(ctx: &SessionCtx, st: &State) {
    let status = if st.failed.is_some() {
        ThreadStatus::Error
    } else {
        ThreadStatus::Idle
    };
    let _ = ctx.out.send(AgentOut::Status(status));
}

async fn on_command(
    ctx: &SessionCtx,
    acp: &mut Acp,
    st: &mut State,
    cmd: SessionCmd,
) -> Result<bool> {
    match cmd {
        SessionCmd::Send { input, settings } => {
            if let Some(s) = settings {
                apply_settings(ctx, acp, st, s, false).await?;
            }
            match st.turn.clone() {
                None => {
                    start_turn(ctx, st, uuid::Uuid::now_v7().to_string(), input.clone());
                    prompt(acp, st, &input).await?;
                }
                Some(_) if !st.asks.is_empty() => hold(ctx, st, input),
                Some(running) => {
                    close_text(ctx, st);
                    show_message(ctx, &running, &uuid::Uuid::now_v7().to_string(), &input);
                    prompt(acp, st, &input).await?;
                }
            }
        }
        SessionCmd::Interrupt => {
            acp.notify("session/cancel", json!({ "sessionId": st.session }))
                .await?;
            drop_held(ctx, st);
            let open: Vec<String> = st.asks.keys().cloned().collect();
            for id in open {
                answer(ctx, acp, st, &id, RequestResponse::Cancelled).await?;
            }
            if st.prompts.is_empty() && st.turn.is_some() {
                finish_turn(ctx, st, TurnOutcome::Interrupted, None);
                idle(ctx, st);
            }
        }
        SessionCmd::Respond { request, response } => {
            answer(ctx, acp, st, &request, response).await?
        }
        SessionCmd::Settings(s) => apply_settings(ctx, acp, st, s, false).await?,
        SessionCmd::Stop => {
            drop_held(ctx, st);
            finish_turn(ctx, st, TurnOutcome::Interrupted, None);
            return Ok(false);
        }
        SessionCmd::StopTask { .. } | SessionCmd::SignIn { .. } => {}
    }
    Ok(true)
}

async fn answer(
    ctx: &SessionCtx,
    acp: &mut Acp,
    st: &mut State,
    request: &str,
    response: RequestResponse,
) -> Result<()> {
    let Some(ask) = st.asks.remove(request) else {
        return Ok(());
    };
    let pick = |kinds: &[&str]| {
        kinds.iter().find_map(|k| {
            ask.options
                .iter()
                .find(|o| o["kind"] == *k)
                .and_then(|o| o["optionId"].as_str())
                .map(String::from)
        })
    };
    let chosen = match &response {
        RequestResponse::Allow { always } if *always => pick(&["allow_always", "allow_once"]),
        RequestResponse::Allow { .. } => pick(&["allow_once", "allow_always"]),
        RequestResponse::Deny { .. } => pick(&["reject_once", "reject_always"]),
        RequestResponse::Answers { answers } if ask.question => {
            answers.first().and_then(|a| a.first()).and_then(|label| {
                ask.options
                    .iter()
                    .find(|o| o["name"].as_str() == Some(label.as_str()))
                    .and_then(|o| o["optionId"].as_str())
                    .map(String::from)
            })
        }
        _ => None,
    };
    let outcome = match chosen {
        Some(id) => json!({ "outcome": { "outcome": "selected", "optionId": id } }),
        None => json!({ "outcome": { "outcome": "cancelled" } }),
    };
    acp.reply(&ask.rpc, outcome).await?;
    emit(
        ctx,
        Event::RequestResolved {
            id: request.to_string(),
            response,
        },
    );
    if st.asks.is_empty() {
        if st.turn.is_some() {
            let _ = ctx.out.send(AgentOut::Status(ThreadStatus::Running));
        }
        while let Some((_, input)) = st.held.pop_front() {
            prompt(acp, st, &input).await?;
        }
    }
    Ok(())
}

fn tool_name(kind: &str) -> String {
    format!("acp:{}", if kind.is_empty() { "other" } else { kind })
}

fn tool_input(call: &Value) -> Value {
    let mut input = match &call["rawInput"] {
        Value::Object(o) => Value::Object(o.clone()),
        _ => json!({}),
    };
    if let Some(t) = call["title"].as_str() {
        input["title"] = json!(t);
    }
    if let Some(p) = call["locations"][0]["path"].as_str() {
        input["path"] = json!(p);
    }
    input
}

fn output_text(update: &Value) -> String {
    let mut out = vec![];
    for c in update["content"].as_array().into_iter().flatten() {
        match c["type"].as_str() {
            Some("content") => {
                if let Some(t) = c["content"]["text"].as_str() {
                    out.push(t.to_string());
                }
            }
            Some("diff") => out.push(format!(
                "Changed {}",
                c["path"].as_str().unwrap_or("a file")
            )),
            _ => {}
        }
    }
    if out.is_empty()
        && let Some(raw) = update.get("rawOutput").filter(|v| !v.is_null())
    {
        out.push(match raw {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        });
    }
    out.join("\n")
}

async fn on_message(ctx: &SessionCtx, acp: &mut Acp, st: &mut State, msg: Value) -> Result<()> {
    let method = msg["method"].as_str().map(String::from);
    match (method.as_deref(), msg.get("id").filter(|v| !v.is_null())) {
        (Some("session/update"), _) => on_update(ctx, st, &msg["params"]["update"]),
        (Some("session/request_permission"), Some(id)) => {
            on_permission(ctx, st, id.clone(), &msg["params"])
        }
        (Some("elicitation/create"), Some(id)) => {
            acp.reply(id, json!({ "action": "decline" })).await?;
            emit(
                ctx,
                Event::Notice {
                    level: NoticeLevel::Info,
                    text: format!(
                        "Antigravity asked something corra can't show yet: {}",
                        msg["params"]["message"].as_str().unwrap_or("")
                    ),
                },
            );
        }
        (Some(other), Some(id)) => {
            acp.write(&json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("corra doesn't handle {other}") } }))
                .await?;
        }
        (Some(_), None) => {}
        (None, Some(id)) => {
            let Some(id) = id.as_u64() else {
                return Ok(());
            };
            if let Some(pos) = st.prompts.iter().position(|p| *p == id) {
                st.prompts.remove(pos);
                if let Some(e) = msg.get("error") {
                    let text = e["message"]
                        .as_str()
                        .unwrap_or("the prompt failed")
                        .to_string();
                    if st.prompts.is_empty() {
                        st.failed = Some(text);
                    } else {
                        emit(
                            ctx,
                            Event::Notice {
                                level: NoticeLevel::Warning,
                                text: format!("Antigravity didn't take a message: {text}"),
                            },
                        );
                    }
                } else if let Some(r) = msg["result"]["stopReason"].as_str() {
                    st.stop = Some(r.to_string());
                }
                if st.prompts.is_empty() && st.asks.is_empty() {
                    let (outcome, error) = match (st.stop.as_deref(), st.failed.clone()) {
                        (_, Some(e)) => (TurnOutcome::Failed, Some(e)),
                        (Some("cancelled"), _) => (TurnOutcome::Interrupted, None),
                        (Some("refusal"), _) => (
                            TurnOutcome::Failed,
                            Some(
                                "Antigravity refused or ran out of quota for this request.".into(),
                            ),
                        ),
                        (Some("max_tokens"), _) => (
                            TurnOutcome::Failed,
                            Some("Antigravity hit its output limit.".into()),
                        ),
                        _ => (TurnOutcome::Completed, None),
                    };
                    finish_turn(ctx, st, outcome, error);
                    idle(ctx, st);
                    st.failed = None;
                    if let Some((id, next)) = st.held.pop_front() {
                        start_turn(ctx, st, id, next.clone());
                        prompt(acp, st, &next).await?;
                    }
                }
            } else if let Some(what) = st.pending.remove(&id) {
                if let Some(e) = msg.get("error") {
                    emit(
                        ctx,
                        Event::Notice {
                            level: NoticeLevel::Warning,
                            text: format!(
                                "Antigravity didn't change {what}: {}",
                                e["message"].as_str().unwrap_or("")
                            ),
                        },
                    );
                } else {
                    st.choices.read(&msg["result"]);
                    if let Some(m) = what["mode"].as_str() {
                        st.choices.mode_is(Some(m.to_string()));
                    }
                }
            }
        }
        (None, None) => {}
    }
    Ok(())
}

fn on_update(ctx: &SessionCtx, st: &mut State, u: &Value) {
    match u["sessionUpdate"].as_str().unwrap_or("") {
        "agent_message_chunk" | "agent_thought_chunk" => {
            let Some(text) = u["content"]["text"].as_str() else {
                return;
            };
            let thought = u["sessionUpdate"] == "agent_thought_chunk";
            close_slot(ctx, st, !thought);
            let turn = current(ctx, st);
            let slot = if thought {
                &mut st.thought
            } else {
                &mut st.text
            };
            if slot.is_none() {
                st.n += 1;
                let id = format!("{turn}:{}{}", if thought { "r" } else { "m" }, st.n);
                let slot = if thought {
                    &mut st.thought
                } else {
                    &mut st.text
                };
                *slot = Some((id, String::new()));
            }
            let slot = if thought {
                &mut st.thought
            } else {
                &mut st.text
            };
            let (id, acc) = slot.as_mut().unwrap();
            acc.push_str(text);
            let _ = ctx.out.send(AgentOut::Delta {
                turn,
                item: id.clone(),
                parent: None,
                delta: if thought {
                    Delta::Reasoning {
                        text: text.to_string(),
                    }
                } else {
                    Delta::Text {
                        text: text.to_string(),
                    }
                },
            });
        }
        "tool_call" => {
            let Some(id) = u["toolCallId"].as_str() else {
                return;
            };
            close_text(ctx, st);
            let turn = current(ctx, st);
            let name = tool_name(u["kind"].as_str().unwrap_or(""));
            let input = tool_input(u);
            let status = u["status"].as_str().unwrap_or("pending");
            st.tools.insert(
                id.to_string(),
                Tool {
                    name: name.clone(),
                    input: input.clone(),
                    output: output_text(u),
                },
            );
            emit(
                ctx,
                Event::ItemStarted {
                    turn: turn.clone(),
                    item: Item {
                        id: id.to_string(),
                        parent: None,
                        body: ItemBody::ToolCall {
                            name,
                            input,
                            output: None,
                            status: ToolStatus::Running,
                        },
                    },
                },
            );
            if matches!(status, "completed" | "failed") {
                complete_tool(ctx, st, &turn, id, status == "failed");
            }
        }
        "tool_call_update" => {
            let Some(id) = u["toolCallId"].as_str() else {
                return;
            };
            let turn = current(ctx, st);
            let Some(t) = st.tools.get_mut(id) else {
                return;
            };
            if let Value::Object(more) = tool_input(u)
                && let Value::Object(have) = &mut t.input
            {
                for (k, v) in more {
                    have.insert(k, v);
                }
            }
            let out = output_text(u);
            if !out.is_empty() {
                t.output = out;
            }
            match u["status"].as_str() {
                Some("completed") => complete_tool(ctx, st, &turn, id, false),
                Some("failed") => complete_tool(ctx, st, &turn, id, true),
                _ => {}
            }
        }
        "plan" => {
            let turn = current(ctx, st);
            let todos = u["entries"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|e| Todo {
                    content: e["content"].as_str().unwrap_or_default().to_string(),
                    status: e["status"].as_str().unwrap_or("pending").to_string(),
                    active_form: None,
                })
                .collect();
            emit(
                ctx,
                Event::ItemCompleted {
                    turn: turn.clone(),
                    item: Item {
                        id: format!("{turn}:todos"),
                        parent: None,
                        body: ItemBody::Todos { todos },
                    },
                },
            );
        }
        "usage_update" => {
            if let Some(used) = u["used"].as_u64() {
                st.usage = Some(ContextUse {
                    used_tokens: used,
                    window_tokens: u["size"].as_u64(),
                });
                emit(
                    ctx,
                    Event::Status {
                        context: st.usage.clone(),
                        rate_limit: None,
                        background: None,
                        agents: None,
                    },
                );
            }
        }
        "available_commands_update" => {
            st.slash = u["availableCommands"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| c["name"].as_str().map(String::from))
                .collect();
            announce(ctx, st);
        }
        "current_mode_update" => {
            if st
                .choices
                .mode_is(u["currentModeId"].as_str().map(String::from))
            {
                announce(ctx, st);
            }
        }
        "config_option_update" | "config_options_update" => {
            st.choices.read(u);
            announce(ctx, st);
        }
        _ => {}
    }
}

fn complete_tool(ctx: &SessionCtx, st: &mut State, turn: &str, id: &str, failed: bool) {
    let Some(t) = st.tools.remove(id) else {
        return;
    };
    emit(
        ctx,
        Event::ItemCompleted {
            turn: turn.to_string(),
            item: Item {
                id: id.to_string(),
                parent: None,
                body: ItemBody::ToolCall {
                    name: t.name,
                    input: t.input,
                    output: Some(ToolOutput {
                        text: t.output,
                        is_error: failed,
                        images: vec![],
                    }),
                    status: if failed {
                        ToolStatus::Failed
                    } else {
                        ToolStatus::Completed
                    },
                },
            },
        },
    );
}

fn on_permission(ctx: &SessionCtx, st: &mut State, rpc: Value, p: &Value) {
    close_text(ctx, st);
    let call = &p["toolCall"];
    let tool_call = call["toolCallId"].as_str().unwrap_or_default().to_string();
    let options: Vec<Value> = p["options"].as_array().cloned().unwrap_or_default();
    let question = tool_call.starts_with("interaction_");
    let kind = call["kind"].as_str().unwrap_or("");
    let turn = current(ctx, st);
    st.wake_until = None;
    let id = format!("acp-{}", uuid::Uuid::now_v7());
    let request = if question {
        AgentRequest::Question {
            questions: vec![Question {
                question: call["title"]
                    .as_str()
                    .unwrap_or("Antigravity has a question")
                    .to_string(),
                header: None,
                options: options
                    .iter()
                    .filter_map(|o| {
                        Some(QuestionOption {
                            label: o["name"].as_str()?.to_string(),
                            description: None,
                        })
                    })
                    .collect(),
                multi_select: false,
            }],
        }
    } else {
        AgentRequest::ToolApproval {
            tool: tool_name(kind),
            input: tool_input(call),
            reason: p["options"][0]["_meta"]["agy.security.warning"]["message"]
                .as_str()
                .map(String::from),
            tool_call: Some(tool_call),
        }
    };
    st.asks.insert(
        id.clone(),
        Ask {
            rpc,
            options,
            question,
        },
    );
    emit(ctx, Event::Request { id, turn, request });
    let _ = ctx.out.send(AgentOut::Status(ThreadStatus::Waiting));
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::sync::mpsc;

    fn real_session() -> Value {
        json!({
            "sessionId": "s",
            "models": { "currentModelId": "gemini-3.1-pro-low", "availableModels": [
                { "modelId": "gemini-4-neon-high", "name": "Gemini 4 Neon (High)" },
                { "modelId": "gemini-4-neon-xhigh", "name": "Gemini 4 Neon (XHigh)" },
                { "modelId": "gemini-4-neon-medium", "name": "Gemini 4 Neon (Medium)" },
                { "modelId": "gemini-4-neon-low", "name": "Gemini 4 Neon (Low)" },
                { "modelId": "gemini-3.1-pro-high", "name": "Gemini 3.1 Pro (High)" },
                { "modelId": "gemini-3.1-pro-low", "name": "Gemini 3.1 Pro (Low)" }
            ]},
            "modes": { "currentModeId": "default", "availableModes": [
                { "id": "default", "name": "Default", "description": "Ask before edits and commands, per your permission settings." },
                { "id": "accept-edits", "name": "Accept edits", "description": "Approve file edits in the workspace without asking." },
                { "id": "plan", "name": "Plan", "description": "Plan carefully before executing a task." }
            ]},
            "configOptions": [
                { "id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "gemini-3.1-pro", "options": [
                    { "value": "gemini-4-neon", "name": "Gemini 4 Neon" },
                    { "value": "gemini-3.1-pro", "name": "Gemini 3.1 Pro" },
                    { "value": "gpt-oss-120b-medium", "name": "GPT-OSS 120B (Medium)" }
                ]},
                { "id": "thought_level", "name": "Thinking Level", "category": "thought_level", "type": "select", "currentValue": "low", "options": [
                    { "value": "low", "name": "Low" }, { "value": "high", "name": "High" }
                ]},
                { "id": "mode", "name": "Mode", "category": "mode", "type": "select", "currentValue": "default", "options": [
                    { "value": "default", "name": "Default", "description": "Ask before edits and commands, per your permission settings." },
                    { "value": "accept-edits", "name": "Accept edits", "description": "Approve file edits in the workspace without asking." },
                    { "value": "plan", "name": "Plan", "description": "Plan carefully before executing a task." }
                ]}
            ]
        })
    }

    #[test]
    fn each_model_gets_the_efforts_agy_lists_for_it() {
        let mut c = Choices::default();
        c.read(&real_session());
        let ids = |l: Vec<Choice>| l.into_iter().map(|c| (c.id, c.name)).collect::<Vec<_>>();
        assert_eq!(
            ids(c.efforts_of("gemini-4-neon")),
            [
                ("low", "Low"),
                ("medium", "Medium"),
                ("high", "High"),
                ("xhigh", "XHigh")
            ]
            .map(|(a, b)| (a.to_string(), b.to_string()))
        );
        assert_eq!(ids(c.efforts_of("gemini-3.1-pro")).len(), 2);
        assert!(c.efforts_of("gpt-oss-120b-medium").is_empty());
        let modes = c.mode_list();
        assert_eq!(modes[1].id, "accept-edits");
        assert_eq!(
            modes[1].hint.as_deref(),
            Some("Approve file edits in the workspace without asking.")
        );
        let pro = c.efforts_of("gemini-3.1-pro");
        assert_eq!(effort_for("xhigh", &pro).as_deref(), Some("high"));
        assert_eq!(effort_for("medium", &pro).as_deref(), Some("low"));
        assert_eq!(
            mode_for("accept-edits", &c.mode_ids()).as_deref(),
            Some("accept-edits")
        );
        assert_eq!(mode_for("acceptEdits", &c.mode_ids()), None);
        assert_eq!(mode_for("bypassPermissions", &c.mode_ids()), None);
    }

    struct Agy {
        lines: tokio::io::Lines<BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>>,
        out: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    }

    impl Agy {
        async fn next(&mut self) -> Value {
            let l = tokio::time::timeout(Duration::from_secs(5), self.lines.next_line())
                .await
                .expect("the runner went quiet")
                .unwrap()
                .expect("the runner closed");
            serde_json::from_str(&l).unwrap()
        }
        async fn expect(&mut self, method: &str) -> Value {
            loop {
                let m = self.next().await;
                if m["method"] == method {
                    return m;
                }
            }
        }
        async fn send(&mut self, v: Value) {
            let mut l = v.to_string();
            l.push('\n');
            self.out.write_all(l.as_bytes()).await.unwrap();
        }
        async fn answer(&mut self, req: &Value, result: Value) {
            self.send(json!({ "jsonrpc": "2.0", "id": req["id"], "result": result }))
                .await;
        }
        async fn update(&mut self, u: Value) {
            self.send(json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": "s1", "update": u } })).await;
        }
    }

    fn start(
        settings: AgentSettings,
    ) -> (
        Agy,
        mpsc::UnboundedReceiver<AgentOut>,
        mpsc::UnboundedSender<SessionCmd>,
    ) {
        let (ours, theirs) = tokio::io::duplex(1 << 20);
        let (our_r, our_w) = tokio::io::split(ours);
        let (their_r, their_w) = tokio::io::split(theirs);
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (hub_tx, _hub_rx) = mpsc::unbounded_channel();
        let mut ctx = SessionCtx {
            baseline: Default::default(),
            hub_url: String::new(),
            hub_token: String::new(),
            hub: hub_tx,
            sync: None,
            spec: StartSpec {
                thread: new_id(),
                agent: AgentKind::Antigravity,
                cwd: "/tmp".into(),
                extra_dirs: vec![],
                resume_session: None,
                settings,
                next_seq: 1,
                tools: vec![],
                repos: vec![],
                agent_env: vec![],
                moved_from: None,
                resume_at: None,
                fork_of: None,
            },
            cwd: "/tmp".into(),
            extra_dirs: vec![],
            out: out_tx,
            cmds: cmd_rx,
            initial: Some(vec![InputPart::Text {
                text: "fix the build".into(),
            }]),
        };
        tokio::spawn(async move {
            let mut acp = Acp::piped(Box::new(our_r), Box::new(our_w));
            let _ = talk(
                &mut ctx,
                &mut acp,
                &PathBuf::from("agy"),
                vec![],
                Default::default(),
            )
            .await;
        });
        (
            Agy {
                lines: BufReader::new(their_r).lines(),
                out: their_w,
            },
            out_rx,
            cmd_tx,
        )
    }

    async fn open(agy: &mut Agy) {
        let init = agy.expect("initialize").await;
        agy.answer(&init, json!({ "protocolVersion": 1, "agentInfo": { "name": "Antigravity ACP Server", "version": "1.5.18" } })).await;
        let auth = agy.expect("authenticate").await;
        assert_eq!(auth["params"]["methodId"], "oauth-personal");
        agy.answer(&auth, json!({})).await;
        let new = agy.expect("session/new").await;
        agy.answer(&new, json!({
            "sessionId": "s1",
            "models": { "currentModelId": "gemini-4-neon-high", "availableModels": [
                { "modelId": "gemini-4-neon-high", "name": "Gemini 4 Neon (High)" },
                { "modelId": "gemini-4-neon-xhigh", "name": "Gemini 4 Neon (XHigh)" },
                { "modelId": "gemini-4-neon-low", "name": "Gemini 4 Neon (Low)" },
                { "modelId": "gemini-4-carbon-high", "name": "Gemini 4 Carbon (High)" },
                { "modelId": "gemini-4-carbon-medium", "name": "Gemini 4 Carbon (Medium)" },
                { "modelId": "gemini-4-carbon-low", "name": "Gemini 4 Carbon (Low)" } ] },
            "configOptions": [
                { "id": "model", "name": "Model", "category": "model", "currentValue": "gemini-4-neon", "options": [
                    { "value": "gemini-4-neon", "name": "Gemini 4 Neon" }, { "value": "gemini-4-carbon", "name": "Gemini 4 Carbon" } ] },
                { "id": "thought_level", "name": "Thinking", "category": "thought_level", "currentValue": "high", "options": [
                    { "value": "low", "name": "Low" }, { "value": "high", "name": "High" }, { "value": "xhigh", "name": "Extra" } ] }
            ]
        })).await;
    }

    async fn drain(
        rx: &mut mpsc::UnboundedReceiver<AgentOut>,
        until: impl Fn(&AgentOut) -> bool,
    ) -> Vec<AgentOut> {
        let mut seen = vec![];
        loop {
            let o = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .expect("the driver went quiet")
                .expect("the driver stopped");
            let done = until(&o);
            seen.push(o);
            if done {
                return seen;
            }
        }
    }

    fn ended(o: &AgentOut) -> bool {
        matches!(o, AgentOut::Event(Event::TurnCompleted { .. }))
    }

    #[tokio::test]
    async fn a_turn_streams_text_tools_and_a_plan_and_reports_the_models_on_offer() {
        let (mut agy, mut rx, _cmd) = start(AgentSettings {
            model: Some("gemini-4-carbon".into()),
            effort: Some("xhigh".into()),
            ..Default::default()
        });
        open(&mut agy).await;
        let set1 = agy.expect("session/set_config_option").await;
        assert_eq!(
            (
                set1["params"]["configId"].as_str(),
                set1["params"]["value"].as_str()
            ),
            (Some("model"), Some("gemini-4-carbon"))
        );
        agy.answer(&set1, json!({})).await;
        let set2 = agy.expect("session/set_config_option").await;
        assert_eq!(
            (
                set2["params"]["configId"].as_str(),
                set2["params"]["value"].as_str()
            ),
            (Some("thought_level"), Some("high"))
        );
        agy.answer(&set2, json!({})).await;
        let p = agy.expect("session/prompt").await;
        assert_eq!(p["params"]["prompt"][0]["text"], "fix the build");
        agy.update(json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "Looking " } })).await;
        agy.update(json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "at it." } })).await;
        agy.update(json!({ "sessionUpdate": "tool_call", "toolCallId": "c1", "title": "cargo build", "kind": "execute", "status": "pending", "rawInput": { "command": "cargo build" } })).await;
        agy.update(json!({ "sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "completed", "content": [{ "type": "content", "content": { "type": "text", "text": "Finished" } }] })).await;
        agy.update(json!({ "sessionUpdate": "plan", "entries": [{ "content": "Fix the build", "priority": "high", "status": "completed" }] })).await;
        agy.update(json!({ "sessionUpdate": "usage_update", "used": 1200, "size": 1048576 }))
            .await;
        agy.update(json!({ "sessionUpdate": "agent_thought_chunk", "content": { "type": "text", "text": "Done, say so." } })).await;
        agy.update(json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "Fixed." } })).await;
        agy.answer(&p, json!({ "stopReason": "end_turn" })).await;
        let out = drain(&mut rx, ended).await;
        let events: Vec<&Event> = out
            .iter()
            .filter_map(|o| match o {
                AgentOut::Event(e) => Some(e),
                _ => None,
            })
            .collect();
        let models = events.iter().find_map(|e| match e {
            Event::Session {
                models, efforts, ..
            } => Some((models.len(), efforts.len())),
            _ => None,
        });
        assert_eq!(models, Some((2, 3)));
        let done: Vec<&Item> = events
            .iter()
            .filter_map(|e| match e {
                Event::ItemCompleted { item, .. } => Some(item),
                _ => None,
            })
            .collect();
        let texts: Vec<&str> = done
            .iter()
            .filter_map(|i| match &i.body {
                ItemBody::AgentMessage { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, ["Looking at it.", "Fixed."]);
        let order: Vec<&str> = done
            .iter()
            .filter_map(|i| match &i.body {
                ItemBody::Reasoning { .. } => Some("thought"),
                ItemBody::AgentMessage { .. } => Some("text"),
                _ => None,
            })
            .collect();
        assert_eq!(order, ["text", "thought", "text"]);
        assert!(done.iter().any(|i| matches!(&i.body, ItemBody::ToolCall { name, status: ToolStatus::Completed, output: Some(o), input, .. }
            if name == "acp:execute" && o.text == "Finished" && input["title"] == "cargo build")));
        assert!(done.iter().any(
            |i| matches!(&i.body, ItemBody::Todos { todos } if todos[0].status == "completed")
        ));
        assert!(matches!(
            events.last(),
            Some(Event::TurnCompleted {
                outcome: TurnOutcome::Completed,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn an_approval_goes_to_the_app_and_its_answer_back_to_antigravity() {
        let (mut agy, mut rx, cmd) = start(AgentSettings::default());
        open(&mut agy).await;
        let p = agy.expect("session/prompt").await;
        agy.send(json!({ "jsonrpc": "2.0", "id": 77, "method": "session/request_permission", "params": {
            "sessionId": "s1",
            "toolCall": { "toolCallId": "c9", "title": "rm -rf target", "kind": "execute", "rawInput": { "command": "rm -rf target" } },
            "options": [{ "optionId": "a1", "name": "Allow", "kind": "allow_once" }, { "optionId": "a2", "name": "Always", "kind": "allow_always" }, { "optionId": "r1", "name": "Deny", "kind": "reject_once" }]
        } })).await;
        let out = drain(&mut rx, |o| {
            matches!(o, AgentOut::Event(Event::Request { .. }))
        })
        .await;
        let Some(AgentOut::Event(Event::Request {
            id,
            request: AgentRequest::ToolApproval { tool, .. },
            ..
        })) = out.last()
        else {
            panic!()
        };
        assert_eq!(tool, "acp:execute");
        cmd.send(SessionCmd::Respond {
            request: id.clone(),
            response: RequestResponse::Allow { always: true },
        })
        .unwrap();
        let reply = agy.next().await;
        assert_eq!(
            (
                reply["id"].as_u64(),
                reply["result"]["outcome"]["optionId"].as_str()
            ),
            (Some(77), Some("a2"))
        );
        agy.answer(&p, json!({ "stopReason": "end_turn" })).await;
        drain(&mut rx, ended).await;
    }

    #[tokio::test]
    async fn a_message_held_behind_an_approval_is_marked_unread_when_stopped() {
        let (mut agy, mut rx, cmd) = start(AgentSettings::default());
        open(&mut agy).await;
        let p = agy.expect("session/prompt").await;
        agy.send(json!({ "jsonrpc": "2.0", "id": 78, "method": "session/request_permission", "params": {
            "sessionId": "s1",
            "toolCall": { "toolCallId": "c1", "title": "ls", "kind": "execute", "rawInput": { "command": "ls" } },
            "options": [{ "optionId": "a1", "name": "Allow", "kind": "allow_once" }, { "optionId": "r1", "name": "Deny", "kind": "reject_once" }]
        } })).await;
        drain(&mut rx, |o| {
            matches!(o, AgentOut::Event(Event::Request { .. }))
        })
        .await;
        cmd.send(SessionCmd::Send {
            input: vec![InputPart::Text {
                text: "and this".into(),
            }],
            settings: None,
        })
        .unwrap();
        cmd.send(SessionCmd::Interrupt).unwrap();
        agy.expect("session/cancel").await;
        agy.answer(&p, json!({ "stopReason": "cancelled" })).await;
        let out = drain(&mut rx, ended).await;
        let shown: Vec<&str> = out
            .iter()
            .filter_map(|o| match o {
                AgentOut::Event(Event::ItemCompleted {
                    item:
                        Item {
                            id,
                            body: ItemBody::UserMessage { .. },
                            ..
                        },
                    ..
                }) => Some(id.as_str()),
                _ => None,
            })
            .collect();
        let unread: Vec<&str> = out
            .iter()
            .filter_map(|o| match o {
                AgentOut::Event(Event::Unread { item }) => Some(item.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(shown.len(), 1, "{out:?}");
        assert_eq!(unread, shown);
    }

    #[tokio::test]
    async fn every_request_goes_to_the_app_and_a_mode_agy_lacks_leaves_it_as_it_is() {
        for (stored, set) in [
            ("bypassPermissions", None),
            ("accept-edits", Some("accept-edits")),
        ] {
            let (mut agy, mut rx, _cmd) = start(AgentSettings {
                permission_mode: Some(stored.into()),
                ..Default::default()
            });
            let init = agy.expect("initialize").await;
            agy.answer(&init, json!({ "protocolVersion": 1 })).await;
            let auth = agy.expect("authenticate").await;
            agy.answer(&auth, json!({})).await;
            let new = agy.expect("session/new").await;
            agy.answer(&new, real_session()).await;
            let next = agy.next().await;
            let p = match set {
                Some(mode) => {
                    assert_eq!(
                        (
                            next["method"].as_str(),
                            next["params"]["configId"].as_str(),
                            next["params"]["value"].as_str()
                        ),
                        (Some("session/set_config_option"), Some("mode"), Some(mode))
                    );
                    agy.answer(&next, json!({})).await;
                    agy.expect("session/prompt").await
                }
                None => {
                    assert_eq!(
                        next["method"], "session/prompt",
                        "agy was asked to change its mode"
                    );
                    next
                }
            };
            for (n, kind) in ["edit", "read", "execute"].into_iter().enumerate() {
                agy.send(json!({ "jsonrpc": "2.0", "id": 80 + n, "method": "session/request_permission", "params": {
                    "sessionId": "s",
                    "toolCall": { "toolCallId": format!("c{n}"), "title": kind, "kind": kind },
                    "options": [{ "optionId": "a1", "name": "Allow", "kind": "allow_once" }, { "optionId": "r1", "name": "Deny", "kind": "reject_once" }]
                } })).await;
            }
            let out = drain(&mut rx, |o| {
                matches!(o, AgentOut::Event(Event::Request { request: AgentRequest::ToolApproval { tool, .. }, .. }) if tool == "acp:execute")
            })
            .await;
            let asked: Vec<&str> = out
                .iter()
                .filter_map(|o| match o {
                    AgentOut::Event(Event::Request {
                        request: AgentRequest::ToolApproval { tool, .. },
                        ..
                    }) => Some(tool.as_str()),
                    _ => None,
                })
                .collect();
            assert_eq!(
                asked,
                ["acp:edit", "acp:read", "acp:execute"],
                "in {stored}"
            );
            let shown = out.iter().find_map(|o| match o {
                AgentOut::Event(Event::Session {
                    permission_mode, ..
                }) => Some(permission_mode.clone()),
                _ => None,
            });
            if set.is_none() {
                assert_eq!(shown, Some(Some("default".to_string())));
            }
            agy.answer(&p, json!({ "stopReason": "cancelled" })).await;
        }
    }

    #[tokio::test]
    async fn a_message_sent_mid_turn_steers_it_and_the_turn_ends_once_after_both_prompts() {
        let (mut agy, mut rx, cmd) = start(AgentSettings::default());
        open(&mut agy).await;
        let first = agy.expect("session/prompt").await;
        agy.update(json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "Working on it." } })).await;
        drain(&mut rx, |o| matches!(o, AgentOut::Delta { .. })).await;
        cmd.send(SessionCmd::Send {
            input: vec![InputPart::Text {
                text: "also update the docs".into(),
            }],
            settings: None,
        })
        .unwrap();
        let second = agy.expect("session/prompt").await;
        assert_eq!(
            second["params"]["prompt"][0]["text"],
            "also update the docs"
        );
        agy.answer(&first, json!({ "stopReason": "end_turn" }))
            .await;
        agy.update(json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "Docs too." } })).await;
        agy.answer(&second, json!({ "stopReason": "end_turn" }))
            .await;
        let out = drain(&mut rx, ended).await;
        let turns: Vec<&String> = out
            .iter()
            .filter_map(|o| match o {
                AgentOut::Event(Event::TurnStarted { turn, .. }) => Some(turn),
                _ => None,
            })
            .collect();
        assert!(turns.is_empty(), "the steer started a turn of its own");
        let said: Vec<&str> = out
            .iter()
            .filter_map(|o| match o {
                AgentOut::Event(Event::ItemCompleted {
                    item:
                        Item {
                            body: ItemBody::AgentMessage { text },
                            ..
                        },
                    ..
                }) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            said,
            ["Working on it.", "Docs too."],
            "the turn ended before the second prompt finished"
        );
    }
}
