use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use corra_proto::runner::RunnerMsg;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

pub const CDP_PORT: u16 = 9222;

pub enum Cmd {
    Open {
        width: u32,
        height: u32,
        reply: oneshot::Sender<Result<Value, String>>,
    },
    Ensure {
        reply: oneshot::Sender<Result<u16, String>>,
    },
    Input {
        method: String,
        params: Value,
    },
    Stop,
}

#[derive(Clone)]
pub struct Browser {
    tx: mpsc::UnboundedSender<Cmd>,
}

fn glob_first(dir: &std::path::Path, prefix: &str, tail: &[&str]) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
        .map(|e| e.path())
        .collect();
    found.sort();
    for base in found.into_iter().rev() {
        for t in tail {
            let p = base.join(t);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

pub fn find_binary() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CORRA_CHROMIUM")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
    {
        return Some(p);
    }
    let mut roots: Vec<PathBuf> = vec![];
    if let Some(p) = std::env::var_os("PLAYWRIGHT_BROWSERS_PATH") {
        roots.push(p.into());
    }
    roots.push("/opt/ms-playwright".into());
    roots.push(crate::files::home().join(".cache/ms-playwright"));
    for r in roots {
        if let Some(p) = glob_first(
            &r,
            "chromium-",
            &["chrome-linux64/chrome", "chrome-linux/chrome"],
        ) {
            return Some(p);
        }
    }
    let path = std::env::var("PATH").unwrap_or_default();
    for name in [
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
    ] {
        for dir in path.split(':').filter(|d| !d.is_empty()) {
            let p = PathBuf::from(dir).join(name);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

impl Browser {
    pub fn start(out: mpsc::UnboundedSender<RunnerMsg>, with_proxy: bool) -> Browser {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(run(rx, out));
        let b = Browser { tx };
        if !with_proxy {
            return b;
        }
        let proxy = b.clone();
        tokio::spawn(async move {
            let Ok(listener) = tokio::net::TcpListener::bind(("127.0.0.1", CDP_PORT)).await else {
                return;
            };
            loop {
                let Ok((mut inbound, _)) = listener.accept().await else {
                    continue;
                };
                let b = proxy.clone();
                tokio::spawn(async move {
                    let Ok(port) = b.ensure().await else { return };
                    if let Ok(mut outbound) =
                        tokio::net::TcpStream::connect(("127.0.0.1", port)).await
                    {
                        let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                    }
                });
            }
        });
        b
    }

    pub async fn ensure(&self) -> Result<u16> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::Ensure { reply })
            .map_err(|_| anyhow!("browser stopped"))?;
        rx.await
            .map_err(|_| anyhow!("browser stopped"))?
            .map_err(|e| anyhow!(e))
    }

    pub async fn open(&self, width: u32, height: u32) -> Result<Value> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::Open {
                width,
                height,
                reply,
            })
            .map_err(|_| anyhow!("browser stopped"))?;
        rx.await
            .map_err(|_| anyhow!("browser stopped"))?
            .map_err(|e| anyhow!(e))
    }

    pub fn input(&self, method: String, params: Value) {
        let _ = self.tx.send(Cmd::Input { method, params });
    }

    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
    }
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Live {
    _child: tokio::process::Child,
    port: u16,
    sink: futures_util::stream::SplitSink<Ws, Message>,
    next_id: u64,
    pages: HashMap<String, Page>,
    current: Option<String>,
    casting: bool,
    size: (u32, u32),
    last_frame: Instant,
    pending: Option<Pending>,
    frames: mpsc::UnboundedSender<Frame>,
}

struct Pending {
    data: String,
    width: u32,
    height: u32,
    ack: Value,
    session: Option<String>,
}

struct Frame {
    data: String,
    width: u32,
    height: u32,
    url: String,
    title: String,
}

struct Page {
    session: Option<String>,
    url: String,
    title: String,
}

async fn launch() -> Result<(tokio::process::Child, u16, String)> {
    let bin = find_binary().ok_or_else(|| anyhow!("there's no Chromium on this machine"))?;
    let dir = crate::files::home().join(".corra/browser");
    tokio::fs::create_dir_all(&dir).await?;
    let port_file = dir.join("DevToolsActivePort");
    let _ = tokio::fs::remove_file(&port_file).await;
    let mut cmd = tokio::process::Command::new(&bin);
    cmd.arg("--headless=new")
        .arg("--no-sandbox")
        .arg("--disable-gpu")
        .arg("--disable-dev-shm-usage")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--remote-debugging-port=0")
        .arg(format!("--user-data-dir={}", dir.display()))
        .arg("--window-size=1280,800")
        .arg("about:blank")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(target_os = "linux")]
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            Ok(())
        });
    }
    let child = cmd
        .spawn()
        .with_context(|| format!("starting {}", bin.display()))?;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(s) = tokio::fs::read_to_string(&port_file).await {
            let mut lines = s.lines();
            if let (Some(port), Some(path)) = (
                lines.next().and_then(|p| p.trim().parse::<u16>().ok()),
                lines.next(),
            ) {
                return Ok((child, port, format!("ws://127.0.0.1:{port}{}", path.trim())));
            }
        }
        if Instant::now() > deadline {
            bail!("Chromium didn't start");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

impl Live {
    async fn send(&mut self, method: &str, params: Value, session: Option<&str>) -> u64 {
        self.next_id += 1;
        let mut m = json!({ "id": self.next_id, "method": method, "params": params });
        if let Some(s) = session {
            m["sessionId"] = json!(s);
        }
        let _ = self.sink.send(Message::Text(m.to_string().into())).await;
        self.next_id
    }

    fn session(&self) -> Option<String> {
        self.pages.get(self.current.as_ref()?)?.session.clone()
    }

    async fn cast(&mut self) {
        let Some(s) = self.session() else { return };
        let (w, h) = self.size;
        self.send("Page.enable", json!({}), Some(&s)).await;
        self.send(
            "Emulation.setDeviceMetricsOverride",
            json!({ "width": w, "height": h, "deviceScaleFactor": 1, "mobile": false }),
            Some(&s),
        )
        .await;
        self.send(
            "Page.startScreencast",
            json!({ "format": "png", "maxWidth": w, "maxHeight": h, "everyNthFrame": 1 }),
            Some(&s),
        )
        .await;
    }

    async fn switch_to(&mut self, target: String) {
        if self.current.as_ref() == Some(&target) {
            return;
        }
        if self.casting
            && let Some(s) = self.session()
        {
            self.send("Page.stopScreencast", json!({}), Some(&s)).await;
        }
        self.current = Some(target);
        self.pending = None;
        if self.casting {
            self.cast().await;
        }
    }
}

async fn run(mut rx: mpsc::UnboundedReceiver<Cmd>, out: mpsc::UnboundedSender<RunnerMsg>) {
    let mut live: Option<Live> = None;
    let mut stream: Option<futures_util::stream::SplitStream<Ws>> = None;
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let (frames, frames_rx) = mpsc::unbounded_channel();
    tokio::spawn(encode(frames_rx, out));
    loop {
        tokio::select! {
            _ = tick.tick() => {
                if let Some(l) = live.as_mut() {
                    flush(l).await;
                }
            }
            cmd = rx.recv() => {
                let Some(cmd) = cmd else { return };
                if live.is_none() && !matches!(cmd, Cmd::Stop | Cmd::Input { .. }) {
                    match start(frames.clone()).await {
                        Ok((l, s)) => { live = Some(l); stream = Some(s); }
                        Err(e) => {
                            let msg = format!("{e:#}");
                            match cmd {
                                Cmd::Open { reply, .. } => { let _ = reply.send(Err(msg)); }
                                Cmd::Ensure { reply } => { let _ = reply.send(Err(msg)); }
                                _ => {}
                            }
                            continue;
                        }
                    }
                }
                let Some(l) = live.as_mut() else { continue };
                match cmd {
                    Cmd::Ensure { reply } => { let _ = reply.send(Ok(l.port)); }
                    Cmd::Open { width, height, reply } => {
                        l.size = (width.clamp(320, 2560), height.clamp(240, 1600));
                        l.casting = true;
                        l.cast().await;
                        let (url, title) = l.current.as_ref().and_then(|c| l.pages.get(c)).map(|p| (p.url.clone(), p.title.clone())).unwrap_or_default();
                        let _ = reply.send(Ok(json!({ "url": url, "title": title })));
                    }
                    Cmd::Input { method, params } => {
                        let allowed = ["Input.dispatchMouseEvent", "Input.dispatchKeyEvent", "Input.insertText", "Page.navigate", "Page.reload"];
                        let s = l.session();
                        if method == "back" || method == "forward" {
                            let js = if method == "back" { "history.back()" } else { "history.forward()" };
                            l.send("Runtime.evaluate", json!({ "expression": js }), s.as_deref()).await;
                        } else if allowed.contains(&method.as_str()) {
                            l.send(&method, params, s.as_deref()).await;
                        }
                    }
                    Cmd::Stop => {
                        l.pending = None;
                        if l.casting {
                            l.casting = false;
                            if let Some(s) = l.session() {
                                l.send("Page.stopScreencast", json!({}), Some(&s)).await;
                            }
                        }
                    }
                }
            }
            msg = async { match stream.as_mut() { Some(s) => s.next().await, None => std::future::pending().await } } => {
                let text = match msg {
                    Some(Ok(Message::Text(t))) => t,
                    Some(Ok(_)) => continue,
                    _ => { live = None; stream = None; continue; }
                };
                let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
                let Some(l) = live.as_mut() else { continue };
                on_event(l, &v).await;
            }
        }
    }
}

async fn flush(l: &mut Live) {
    if l.pending.is_none() || l.last_frame.elapsed() < Duration::from_millis(90) {
        return;
    }
    let Some(p) = l.pending.take() else { return };
    l.last_frame = Instant::now();
    l.send(
        "Page.screencastFrameAck",
        json!({ "sessionId": p.ack }),
        p.session.as_deref(),
    )
    .await;
    let (url, title) = l
        .current
        .as_ref()
        .and_then(|c| l.pages.get(c))
        .map(|p| (p.url.clone(), p.title.clone()))
        .unwrap_or_default();
    let _ = l.frames.send(Frame {
        data: p.data,
        width: p.width,
        height: p.height,
        url,
        title,
    });
}

async fn encode(mut rx: mpsc::UnboundedReceiver<Frame>, out: mpsc::UnboundedSender<RunnerMsg>) {
    while let Some(mut f) = rx.recv().await {
        while let Ok(newer) = rx.try_recv() {
            f = newer;
        }
        let Frame {
            data,
            width,
            height,
            url,
            title,
        } = f;
        match tokio::task::spawn_blocking(move || to_jpeg(&data)).await {
            Ok(Ok(data)) => {
                if out
                    .send(RunnerMsg::BrowserFrame {
                        data,
                        width,
                        height,
                        url,
                        title,
                    })
                    .is_err()
                {
                    return;
                }
            }
            Ok(Err(e)) => tracing::warn!("could not encode a browser frame: {e:#}"),
            Err(_) => {}
        }
    }
}

// Chromium's own JPEG encoder writes scrambled images on some virtual machines (seen on Modal), so frames
// leave it as PNG and are encoded here.
fn to_jpeg(png: &str) -> Result<String> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;
    Ok(b64.encode(crate::image::decode_png(&b64.decode(png)?)?.jpeg(75)?))
}

async fn start(
    frames: mpsc::UnboundedSender<Frame>,
) -> Result<(Live, futures_util::stream::SplitStream<Ws>)> {
    let (child, port, ws_url) = launch().await?;
    let (ws, _) = tokio_tungstenite::connect_async(ws_url).await?;
    let (sink, stream) = ws.split();
    let mut l = Live {
        _child: child,
        port,
        sink,
        next_id: 0,
        pages: HashMap::new(),
        current: None,
        casting: false,
        size: (1280, 800),
        last_frame: Instant::now(),
        pending: None,
        frames,
    };
    let downloads = crate::files::home().join(".corra/browser-downloads");
    let _ = tokio::fs::create_dir_all(&downloads).await;
    l.send(
        "Browser.setDownloadBehavior",
        json!({ "behavior": "allow", "downloadPath": downloads.to_string_lossy() }),
        None,
    )
    .await;
    l.send(
        "Target.setDiscoverTargets",
        json!({ "discover": true }),
        None,
    )
    .await;
    Ok((l, stream))
}

async fn on_event(l: &mut Live, v: &Value) {
    let method = v["method"].as_str().unwrap_or("");
    let p = &v["params"];
    if method != "Page.screencastFrame" {
        tracing::debug!(
            "cdp: {}",
            v.to_string().chars().take(300).collect::<String>()
        );
    }
    match method {
        "Target.targetCreated" | "Target.targetInfoChanged" => {
            let info = &p["targetInfo"];
            if info["type"] != "page" {
                return;
            }
            let id = info["targetId"].as_str().unwrap_or_default().to_string();
            let url = info["url"].as_str().unwrap_or_default().to_string();
            let title = info["title"].as_str().unwrap_or_default().to_string();
            let known = l.pages.contains_key(&id);
            let page = l.pages.entry(id.clone()).or_insert(Page {
                session: None,
                url: String::new(),
                title: String::new(),
            });
            page.url = url;
            page.title = title;
            if !known {
                l.send(
                    "Target.attachToTarget",
                    json!({ "targetId": id, "flatten": true }),
                    None,
                )
                .await;
            }
        }
        "Target.attachedToTarget" => {
            let id = p["targetInfo"]["targetId"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let session = p["sessionId"].as_str().map(String::from);
            if let Some(page) = l.pages.get_mut(&id) {
                page.session = session;
                l.current = None;
                l.switch_to(id).await;
            }
        }
        "Target.targetDestroyed" => {
            let id = p["targetId"].as_str().unwrap_or_default().to_string();
            l.pages.remove(&id);
            if l.current.as_ref() == Some(&id) {
                l.current = None;
                if let Some(next) = l.pages.keys().next().cloned() {
                    l.switch_to(next).await;
                }
            }
        }
        "Page.screencastFrame" => {
            let session = v["sessionId"].as_str().map(String::from);
            if !l.casting || session != l.session() {
                l.send(
                    "Page.screencastFrameAck",
                    json!({ "sessionId": p["sessionId"] }),
                    session.as_deref(),
                )
                .await;
                return;
            }
            let frame = Pending {
                data: p["data"].as_str().unwrap_or_default().to_string(),
                width: p["metadata"]["deviceWidth"].as_f64().unwrap_or(0.0) as u32,
                height: p["metadata"]["deviceHeight"].as_f64().unwrap_or(0.0) as u32,
                ack: p["sessionId"].clone(),
                session,
            };
            if let Some(old) = l.pending.replace(frame) {
                l.send(
                    "Page.screencastFrameAck",
                    json!({ "sessionId": old.ack }),
                    old.session.as_deref(),
                )
                .await;
            }
            flush(l).await;
        }
        _ => {}
    }
}
