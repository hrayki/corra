use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use corra_proto::runner::RunnerMsg;
use corra_proto::{PhoneInput, PhoneOp, TouchAction};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

const SCRCPY: &str = "5.0.1";
const SCRCPY_SHA256: &str = "764eb6f79811d5211fe9df341120882ba9994c7a61b897d7bf3fb662e53bc536";
const VIDEO_MAX: u32 = 1200;
const SHOT_MAX: u32 = 1280;
const H264: u32 = 0x6832_3634;

#[derive(Clone)]
pub struct Phone {
    inner: Arc<Inner>,
    tunnels: crate::tunnel::Tunnels,
}

struct Inner {
    out: mpsc::UnboundedSender<RunnerMsg>,
    booted: tokio::sync::OnceCell<()>,
    size: tokio::sync::OnceCell<(u32, u32)>,
    stream: Mutex<Option<Stream>>,
    starting: tokio::sync::Mutex<()>,
}

struct Stream {
    control: mpsc::UnboundedSender<Vec<u8>>,
    task: tokio::task::JoinHandle<()>,
    port: u16,
}

fn adb_path() -> PathBuf {
    std::env::var_os("CORRA_ADB")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/opt/sdk/platform-tools/adb".into())
}

fn adb_cmd(args: &[&str]) -> tokio::process::Command {
    let mut c = tokio::process::Command::new(adb_path());
    c.arg("-e")
        .args(args)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    c
}

async fn adb(args: &[&str], secs: u64) -> Result<Vec<u8>> {
    let out = tokio::time::timeout(Duration::from_secs(secs), adb_cmd(args).output())
        .await
        .map_err(|_| anyhow!("the phone didn't answer in time"))?
        .context("running adb")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = if err.trim().is_empty() {
            String::from_utf8_lossy(&out.stdout).into_owned()
        } else {
            err.into_owned()
        };
        bail!("{}", err.trim());
    }
    Ok(out.stdout)
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn keycode(key: &str) -> Option<u32> {
    Some(match key.to_lowercase().as_str() {
        "home" => 3,
        "back" => 4,
        "up" | "arrowup" => 19,
        "down" | "arrowdown" => 20,
        "left" | "arrowleft" => 21,
        "right" | "arrowright" => 22,
        "power" => 26,
        "tab" => 61,
        "enter" => 66,
        "backspace" => 67,
        "menu" => 82,
        "escape" => 111,
        "delete" => 112,
        "recents" => 187,
        other => return other.parse().ok(),
    })
}

struct Node {
    x: u32,
    y: u32,
    tappable: bool,
    kind: String,
    text: String,
}

fn unescape(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#10;", " ")
        .replace("&#13;", " ")
        .replace("&amp;", "&")
}

fn attr<'a>(tag: &'a str, name: &str) -> &'a str {
    let key = format!(" {name}=\"");
    match tag.find(&key) {
        Some(i) => {
            let rest = &tag[i + key.len()..];
            &rest[..rest.find('"').unwrap_or(rest.len())]
        }
        None => "",
    }
}

fn nodes(xml: &str, div: u32) -> Vec<Node> {
    let mut out = vec![];
    for tag in xml.split("<node").skip(1) {
        let tag = &tag[..tag.find('>').unwrap_or(tag.len())];
        let b: Vec<u32> = attr(tag, "bounds")
            .split(|c: char| !c.is_ascii_digit())
            .filter_map(|n| n.parse().ok())
            .collect();
        if b.len() != 4 {
            continue;
        }
        let text = ["text", "content-desc", "hint"]
            .iter()
            .map(|a| attr(tag, a))
            .find(|v| !v.is_empty())
            .unwrap_or("");
        let text = unescape(text)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let class = attr(tag, "class").rsplit('.').next().unwrap_or("");
        let id = attr(tag, "resource-id").rsplit('/').next().unwrap_or("");
        out.push(Node {
            x: (b[0] + b[2]) / 2 / div,
            y: (b[1] + b[3]) / 2 / div,
            tappable: attr(tag, "clickable") == "true",
            kind: if id.is_empty() {
                class.to_string()
            } else {
                format!("{class}#{id}")
            },
            text,
        });
    }
    out
}

fn touch(action: TouchAction, x: i32, y: i32, width: u16, height: u16) -> Vec<u8> {
    let mut m = vec![
        2,
        match action {
            TouchAction::Down => 0,
            TouchAction::Up => 1,
            TouchAction::Move => 2,
        },
    ];
    m.extend((-2i64).to_be_bytes());
    m.extend(x.to_be_bytes());
    m.extend(y.to_be_bytes());
    m.extend(width.to_be_bytes());
    m.extend(height.to_be_bytes());
    m.extend(
        if action == TouchAction::Up {
            0u16
        } else {
            0xffff
        }
        .to_be_bytes(),
    );
    m.extend(0u32.to_be_bytes());
    m.extend(0u32.to_be_bytes());
    m
}

fn key_event(code: u32, up: bool) -> Vec<u8> {
    let mut m = vec![0, up as u8];
    m.extend(code.to_be_bytes());
    m.extend(0u32.to_be_bytes());
    m.extend(0u32.to_be_bytes());
    m
}

fn control_msgs(input: PhoneInput) -> Vec<Vec<u8>> {
    match input {
        PhoneInput::Touch {
            action,
            x,
            y,
            width,
            height,
        } => vec![touch(action, x, y, width, height)],
        PhoneInput::Scroll {
            x,
            y,
            width,
            height,
            dx,
            dy,
        } => {
            let fixed =
                |v: f32| ((v / 16.0 * 32768.0).clamp(-32768.0, 32767.0) as i16).to_be_bytes();
            let mut m = vec![3];
            m.extend(x.to_be_bytes());
            m.extend(y.to_be_bytes());
            m.extend(width.to_be_bytes());
            m.extend(height.to_be_bytes());
            m.extend(fixed(dx));
            m.extend(fixed(dy));
            m.extend(0u32.to_be_bytes());
            vec![m]
        }
        PhoneInput::Text { text } => {
            let mut m = vec![1];
            m.extend((text.len() as u32).to_be_bytes());
            m.extend(text.as_bytes());
            vec![m]
        }
        PhoneInput::Key { key } => match keycode(&key) {
            Some(code) => vec![key_event(code, false), key_event(code, true)],
            None => vec![],
        },
    }
}

impl Phone {
    pub fn new(out: mpsc::UnboundedSender<RunnerMsg>, tunnels: crate::tunnel::Tunnels) -> Self {
        Phone {
            tunnels,
            inner: Arc::new(Inner {
                out,
                booted: Default::default(),
                size: Default::default(),
                stream: Mutex::new(None),
                starting: Default::default(),
            }),
        }
    }

    async fn boot(&self) -> Result<()> {
        self.inner
            .booted
            .get_or_try_init(|| async {
                if !adb_path().is_file() {
                    bail!("this machine is not a phone");
                }
                let out = tokio::process::Command::new("/usr/local/bin/phone-boot")
                    .stdin(Stdio::null())
                    .output()
                    .await
                    .context("starting the phone")?;
                if !out.status.success() {
                    bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
                }
                Ok(())
            })
            .await
            .map(|_| ())
    }

    async fn size(&self) -> Result<(u32, u32)> {
        self.inner
            .size
            .get_or_try_init(|| async {
                let out =
                    String::from_utf8_lossy(&adb(&["shell", "wm", "size"], 20).await?).into_owned();
                let dims = out
                    .lines()
                    .last()
                    .and_then(|l| l.rsplit(' ').next())
                    .and_then(|d| d.trim().split_once('x'))
                    .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
                dims.ok_or_else(|| anyhow!("couldn't read the phone's screen size"))
            })
            .await
            .copied()
    }

    async fn div(&self) -> Result<u32> {
        let (w, h) = self.size().await?;
        Ok(w.max(h).div_ceil(SHOT_MAX).max(1))
    }

    async fn input(&self, args: &[String]) -> Result<Value> {
        let mut all = vec!["shell", "input"];
        all.extend(args.iter().map(String::as_str));
        adb(&all, 30).await?;
        Ok(Value::Null)
    }

    async fn ui(&self) -> Result<Vec<Node>> {
        let xml = adb(
            &[
                "exec-out",
                "uiautomator dump /sdcard/ui.xml >/dev/null && cat /sdcard/ui.xml",
            ],
            40,
        )
        .await?;
        Ok(nodes(&String::from_utf8_lossy(&xml), self.div().await?))
    }

    pub async fn handle(&self, op: PhoneOp) -> Result<Value> {
        self.boot().await?;
        match op {
            PhoneOp::Boot => Ok(Value::Null),
            PhoneOp::Stream => {
                self.stream().await?;
                Ok(Value::Null)
            }
            PhoneOp::Shot => {
                let png = adb(&["exec-out", "screencap", "-p"], 30).await?;
                let div = self.div().await?;
                let (data, width, height) = tokio::task::spawn_blocking(move || {
                    let small = crate::image::decode_png(&png)?.shrink(div);
                    anyhow::Ok((small.jpeg(80)?, small.width, small.height))
                })
                .await??;
                Ok(json!({
                    "text": format!("The phone's screen, {width}x{height}."),
                    "image": { "data": base64::engine::general_purpose::STANDARD.encode(data), "mime": "image/jpeg" },
                    "width": width,
                    "height": height,
                }))
            }
            PhoneOp::Ui => {
                let lines: Vec<String> = self
                    .ui()
                    .await?
                    .into_iter()
                    .filter(|n| !n.text.is_empty() || (n.tappable && n.kind.contains('#')))
                    .map(|n| {
                        format!(
                            "{} {},{} [{}] {}",
                            if n.tappable { "*" } else { " " },
                            n.x,
                            n.y,
                            n.kind,
                            n.text
                        )
                    })
                    .collect();
                Ok(json!(if lines.is_empty() {
                    "Nothing is listed on this screen. Take a screenshot and tap by point."
                        .to_string()
                } else {
                    lines.join("\n")
                }))
            }
            PhoneOp::Tap { x, y } => {
                let d = self.div().await?;
                self.input(&["tap".into(), (x * d).to_string(), (y * d).to_string()])
                    .await
            }
            PhoneOp::TapText { text, index } => {
                let all = self.ui().await?;
                let want = text.to_lowercase();
                let mut hits: Vec<&Node> = all.iter().filter(|n| n.text == text).collect();
                if hits.is_empty() {
                    hits = all
                        .iter()
                        .filter(|n| n.text.to_lowercase().contains(&want))
                        .collect();
                }
                let Some(n) = hits.get(index as usize) else {
                    bail!("no '{text}' on screen");
                };
                let d = self.div().await?;
                self.input(&["tap".into(), (n.x * d).to_string(), (n.y * d).to_string()])
                    .await?;
                Ok(json!(format!("Tapped '{}' at {},{}.", n.text, n.x, n.y)))
            }
            PhoneOp::Swipe { x1, y1, x2, y2, ms } => {
                let d = self.div().await?;
                let mut args: Vec<String> = vec!["swipe".into()];
                args.extend([x1, y1, x2, y2].map(|v| (v * d).to_string()));
                args.push(ms.unwrap_or(300).to_string());
                self.input(&args).await
            }
            PhoneOp::Text { text } => {
                self.input(&["text".into(), quote(&text.replace(' ', "%s"))])
                    .await
            }
            PhoneOp::Key { key } => {
                let code = keycode(&key).ok_or_else(|| anyhow!("unknown key {key}"))?;
                self.input(&["keyevent".into(), code.to_string()]).await
            }
            PhoneOp::Shell { command } => {
                let out = tokio::time::timeout(
                    Duration::from_secs(60),
                    adb_cmd(&["shell", &command]).output(),
                )
                .await
                .map_err(|_| anyhow!("the command ran for more than a minute"))??;
                let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
                text.push_str(&String::from_utf8_lossy(&out.stderr));
                if text.len() > 20_000 {
                    let cut = text.len() - 20_000;
                    let cut = (cut..text.len())
                        .find(|&i| text.is_char_boundary(i))
                        .unwrap_or(cut);
                    text = format!("[the first {cut} bytes were dropped]\n{}", &text[cut..]);
                }
                Ok(json!(text))
            }
            PhoneOp::Install { urls } => self.install(urls).await,
            PhoneOp::Forward { port } => {
                self.tunnels.listen(port).await?;
                adb(
                    &["reverse", &format!("tcp:{port}"), &format!("tcp:{port}")],
                    20,
                )
                .await?;
                Ok(Value::Null)
            }
        }
    }

    async fn install(&self, urls: Vec<String>) -> Result<Value> {
        let http = reqwest::Client::new();
        let dir = std::env::temp_dir().join(format!("corra-apk-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await?;
        let mut files = vec![];
        for (i, url) in urls.iter().enumerate() {
            let bytes = http
                .get(url)
                .send()
                .await?
                .error_for_status()?
                .bytes()
                .await?;
            let path = dir.join(format!("{i}.apk"));
            tokio::fs::write(&path, &bytes).await?;
            files.push(path.to_string_lossy().into_owned());
        }
        let mut args = vec![
            if files.len() == 1 {
                "install"
            } else {
                "install-multiple"
            },
            "-r",
        ];
        args.extend(files.iter().map(String::as_str));
        let out = adb_cmd(&args).output().await?;
        let _ = tokio::fs::remove_dir_all(&dir).await;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        if !text.contains("Success") {
            bail!("the phone refused the app: {}", text.trim());
        }
        Ok(json!("Installed."))
    }

    pub fn send(&self, input: PhoneInput) {
        if let Some(s) = self.inner.stream.lock().unwrap().as_ref() {
            for m in control_msgs(input) {
                let _ = s.control.send(m);
            }
        }
    }

    pub fn stop(&self) {
        if let Some(s) = self.inner.stream.lock().unwrap().take() {
            s.task.abort();
            tokio::spawn(async move {
                let _ = adb(&["forward", "--remove", &format!("tcp:{}", s.port)], 10).await;
            });
        }
    }

    async fn stream(&self) -> Result<()> {
        let _guard = self.inner.starting.lock().await;
        if let Some(s) = self.inner.stream.lock().unwrap().as_ref()
            && !s.task.is_finished()
        {
            let _ = s.control.send(vec![17]);
            return Ok(());
        }
        let jar = server_jar().await?;
        adb(
            &[
                "push",
                &jar.to_string_lossy(),
                "/data/local/tmp/scrcpy-server.jar",
            ],
            60,
        )
        .await?;
        let scid = format!("{:08x}", (std::process::id() ^ rand_u32()) & 0x7fff_ffff);
        let forward = adb(
            &["forward", "tcp:0", &format!("localabstract:scrcpy_{scid}")],
            20,
        )
        .await?;
        let port: u16 = String::from_utf8_lossy(&forward)
            .trim()
            .parse()
            .context("reading the forwarded port")?;
        let server = adb_cmd(&[
            "shell",
            &format!(
                "CLASSPATH=/data/local/tmp/scrcpy-server.jar app_process / com.genymobile.scrcpy.Server {SCRCPY} scid={scid} log_level=warn tunnel_forward=true audio=false control=true cleanup=false send_device_meta=false max_size={VIDEO_MAX} max_fps=30 video_bit_rate=2000000"
            ),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("starting the screen stream")?;

        let mut video = None;
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)).await else {
                continue;
            };
            let mut dummy = [0u8; 1];
            match tokio::time::timeout(Duration::from_secs(2), s.read_exact(&mut dummy)).await {
                Ok(Ok(_)) => {
                    video = Some(s);
                    break;
                }
                _ => continue,
            }
        }
        let mut video = video.ok_or_else(|| anyhow!("the phone's screen stream didn't start"))?;
        let control = TcpStream::connect(("127.0.0.1", port)).await?;
        let mut codec = [0u8; 4];
        video.read_exact(&mut codec).await?;
        if u32::from_be_bytes(codec) != H264 {
            bail!("the phone sent an unexpected video format");
        }

        let (control_tx, mut control_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let out = self.inner.out.clone();
        let task = tokio::spawn(async move {
            let _server = server;
            let (mut control_read, mut control_write) = control.into_split();
            let writer = async {
                while let Some(m) = control_rx.recv().await {
                    if control_write.write_all(&m).await.is_err() {
                        break;
                    }
                }
            };
            let drain = async {
                let mut buf = [0u8; 4096];
                while matches!(control_read.read(&mut buf).await, Ok(n) if n > 0) {}
            };
            let reader = async {
                let b64 = base64::engine::general_purpose::STANDARD;
                let mut head = [0u8; 12];
                loop {
                    if video.read_exact(&mut head).await.is_err() {
                        break;
                    }
                    if head[0] & 0x80 != 0 {
                        let width = u32::from_be_bytes(head[4..8].try_into().unwrap());
                        let height = u32::from_be_bytes(head[8..12].try_into().unwrap());
                        let _ = out.send(RunnerMsg::PhoneVideo { width, height });
                        continue;
                    }
                    let size = u32::from_be_bytes(head[8..12].try_into().unwrap()) as usize;
                    if size > 8 << 20 {
                        break;
                    }
                    let mut data = vec![0u8; size];
                    if video.read_exact(&mut data).await.is_err() {
                        break;
                    }
                    let _ = out.send(RunnerMsg::PhoneFrame {
                        data: b64.encode(&data),
                        config: head[0] & 0x40 != 0,
                        key: head[0] & 0x20 != 0,
                    });
                }
            };
            tokio::select! {
                _ = writer => {}
                _ = drain => {}
                _ = reader => {}
            }
        });
        *self.inner.stream.lock().unwrap() = Some(Stream {
            control: control_tx,
            task,
            port,
        });
        Ok(())
    }
}

fn rand_u32() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0)
}

async fn server_jar() -> Result<PathBuf> {
    let path = PathBuf::from(format!("/opt/scrcpy-server-v{SCRCPY}"));
    if path.is_file() {
        return Ok(path);
    }
    let url = format!(
        "https://github.com/Genymobile/scrcpy/releases/download/v{SCRCPY}/scrcpy-server-v{SCRCPY}"
    );
    let bytes = reqwest::get(&url)
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let tmp = path.with_extension("part");
    tokio::fs::write(&tmp, &bytes).await?;
    let sum = tokio::process::Command::new("sha256sum")
        .arg(&tmp)
        .output()
        .await?;
    if !String::from_utf8_lossy(&sum.stdout).starts_with(SCRCPY_SHA256) {
        let _ = tokio::fs::remove_file(&tmp).await;
        bail!("the screen stream's download didn't match its checksum");
    }
    tokio::fs::rename(&tmp, &path).await?;
    Ok(path)
}
