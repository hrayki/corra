mod control;
mod fetch;
mod state;
mod store;

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use control::{Ask, Event};
use fetch::{Http, Releases};
use state::{Run, State, Verdict, valid_version};
use store::{Blobs, BucketEnv, Disk, S3};

#[macro_export]
macro_rules! log {
    ($($t:tt)*) => { eprintln!("launcher: {}", format!($($t)*)) };
}

pub const RELEASES: &str = "https://github.com/hrayki/corra/releases";
const STOP_WAIT: Duration = Duration::from_secs(45);
const TICK: Duration = Duration::from_millis(200);
const TAIL: usize = 40;

pub fn http_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(Duration::from_secs(600)))
        .user_agent("corra-launcher")
        .build()
        .into()
}

struct Config {
    data: PathBuf,
    local: PathBuf,
    releases: String,
    bucket: Option<BucketEnv>,
    probe: SocketAddr,
    socket: PathBuf,
    litestream: Option<PathBuf>,
}

fn env(k: &str) -> Option<String> {
    std::env::var(k)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn probe_addr() -> Result<SocketAddr> {
    let mut a: SocketAddr = match (env("CORRA_LISTEN"), env("PORT")) {
        (Some(l), _) => l
            .parse()
            .context("CORRA_LISTEN must be an address like 0.0.0.0:8080")?,
        (None, Some(p)) => SocketAddr::from((
            [127, 0, 0, 1],
            p.parse().context("PORT must be a port number")?,
        )),
        (None, None) => SocketAddr::from(([127, 0, 0, 1], 7810)),
    };
    if a.ip().is_unspecified() {
        a.set_ip(if a.is_ipv4() {
            std::net::Ipv4Addr::LOCALHOST.into()
        } else {
            std::net::Ipv6Addr::LOCALHOST.into()
        });
    }
    Ok(a)
}

impl Config {
    fn from_env() -> Result<Config> {
        let data = PathBuf::from(env("CORRA_DATA").unwrap_or_else(|| "./data".into()));
        std::fs::create_dir_all(&data).with_context(|| format!("creating {}", data.display()))?;
        let data = data.canonicalize()?;
        let names = [
            "CORRA_STORAGE_ENDPOINT",
            "CORRA_STORAGE_BUCKET",
            "CORRA_STORAGE_ACCESS_KEY",
            "CORRA_STORAGE_SECRET_KEY",
        ];
        let vals: Vec<Option<String>> = names.iter().map(|n| env(n)).collect();
        let bucket = if vals.iter().all(Option::is_none) {
            None
        } else if vals.iter().any(Option::is_none) {
            bail!(
                "set all of {} for remote storage, or none",
                names.join(", ")
            );
        } else {
            let v: Vec<String> = vals.into_iter().flatten().collect();
            Some(BucketEnv {
                endpoint: v[0].clone(),
                bucket: v[1].clone(),
                access_key: v[2].clone(),
                secret_key: v[3].clone(),
                region: env("CORRA_STORAGE_REGION").unwrap_or_else(|| "auto".into()),
                prefix: env("CORRA_STORAGE_PREFIX").unwrap_or_else(|| "corra".into()),
            })
        };
        let litestream = env("CORRA_LITESTREAM").map(PathBuf::from).or_else(|| {
            let beside = std::env::current_exe().ok()?.parent()?.join("litestream");
            beside.exists().then_some(beside)
        });
        Ok(Config {
            local: data.join("launcher"),
            data,
            releases: env("CORRA_RELEASES").unwrap_or_else(|| RELEASES.into()),
            bucket,
            probe: probe_addr()?,
            socket: std::env::temp_dir()
                .join(format!("corra-launcher-{}.sock", std::process::id())),
            litestream,
        })
    }
}

fn block_signals() -> libc::sigset_t {
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for s in [
            libc::SIGTERM,
            libc::SIGINT,
            libc::SIGQUIT,
            libc::SIGHUP,
            libc::SIGCHLD,
        ] {
            libc::sigaddset(&mut set, s);
        }
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        set
    }
}

fn hub_command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.process_group(0);
    unsafe {
        cmd.pre_exec(|| {
            let mut none: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut none);
            libc::pthread_sigmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
            Ok(())
        });
    }
    cmd
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn describe(status: i32) -> String {
    if libc::WIFEXITED(status) {
        format!("exited with status {}", libc::WEXITSTATUS(status))
    } else if libc::WIFSIGNALED(status) {
        format!("was killed by signal {}", libc::WTERMSIG(status))
    } else {
        format!("stopped ({status})")
    }
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

type Tail = Arc<Mutex<VecDeque<String>>>;

fn tee(from: impl Read + Send + 'static, mut to: impl Write + Send + 'static, tail: Tail) {
    std::thread::spawn(move || {
        let mut r = BufReader::new(from);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match r.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => return,
                Ok(_) => {
                    let _ = to.write_all(&buf);
                    let _ = to.flush();
                    let line = strip_ansi(String::from_utf8_lossy(&buf).trim_end());
                    let line: String = line.chars().take(400).collect();
                    let mut t = tail.lock().unwrap();
                    if t.len() == TAIL {
                        t.pop_front();
                    }
                    t.push_back(line);
                }
            }
        }
    });
}

fn probe(addr: SocketAddr) -> Option<Option<String>> {
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(1)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    s.set_write_timeout(Some(Duration::from_secs(2))).ok()?;
    s.write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .ok()?;
    let mut got = Vec::new();
    let mut buf = [0u8; 1024];
    while got.len() < 16 << 10 && !got.windows(4).any(|w| w == b"\r\n\r\n") {
        match s.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => got.extend_from_slice(&buf[..n]),
        }
    }
    parse_health(&String::from_utf8_lossy(&got))
}

fn parse_health(text: &str) -> Option<Option<String>> {
    let head = text.split("\r\n\r\n").next()?;
    let mut lines = head.split("\r\n");
    let status = lines.next()?.split_whitespace().nth(1)?;
    if status != "200" {
        return None;
    }
    Some(lines.find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case("x-corra-version")
            .then(|| v.trim().to_string())
    }))
}

fn backoff(n: u32) -> Duration {
    Duration::from_secs((2u64 << n.min(5)).min(60))
}

struct Child {
    pid: i32,
    tail: Tail,
}

enum Got {
    Term,
    Exit(i32, i32),
    Event(Event),
    Nothing,
}

enum Outcome {
    Stopped,
    Switched,
    Failed(String, Vec<String>),
    Exited(Duration),
}

struct Launcher {
    cfg: Config,
    sigs: libc::sigset_t,
    rx: Receiver<Event>,
    tx: Sender<Event>,
    releases: Arc<dyn Releases>,
    bucket: Option<Arc<dyn Blobs>>,
    state: Arc<dyn Blobs>,
    st: State,
    term: bool,
    exits: VecDeque<(i32, i32)>,
    fetching: Option<(String, Vec<Sender<Value>>)>,
}

impl Launcher {
    fn next(&mut self, timeout: Duration) -> Got {
        let deadline = Instant::now() + timeout;
        loop {
            loop {
                let mut status = 0;
                let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
                if pid <= 0 {
                    break;
                }
                self.exits.push_back((pid, status));
            }
            if let Some((pid, status)) = self.exits.pop_front() {
                return Got::Exit(pid, status);
            }
            if let Ok(e) = self.rx.try_recv() {
                return Got::Event(e);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Got::Nothing;
            }
            let wait = left.min(TICK);
            let ts = libc::timespec {
                tv_sec: wait.as_secs() as libc::time_t,
                tv_nsec: wait.subsec_nanos() as libc::c_long,
            };
            let sig = unsafe { libc::sigtimedwait(&self.sigs, std::ptr::null_mut(), &ts) };
            if matches!(sig, libc::SIGTERM | libc::SIGINT | libc::SIGQUIT) && !self.term {
                self.term = true;
                log!("stopping");
                return Got::Term;
            }
        }
    }

    fn status(&self, running: Option<&str>) -> Value {
        json!({ "running": running, "state": self.st })
    }

    fn refuse(&mut self, e: Event, why: &str) {
        match e {
            Event::Ask(Ask::Status, reply) => {
                let _ = reply.send(self.status(None));
            }
            Event::Ask(Ask::Switch { .. }, reply) => {
                let _ = reply.send(json!({ "error": why }));
            }
            Event::Fetched(v, r) => {
                self.fetched(v, r, false);
            }
        }
    }

    fn pause(&mut self, d: Duration) -> bool {
        let until = Instant::now() + d;
        while Instant::now() < until {
            match self.next(until.saturating_duration_since(Instant::now())) {
                Got::Term => return false,
                Got::Event(e) => self.refuse(e, "the hub is starting; try again in a moment"),
                Got::Exit(..) | Got::Nothing => {}
            }
        }
        !self.term
    }

    fn retry<T>(&mut self, what: &str, mut f: impl FnMut(&mut Self) -> Result<T>) -> Option<T> {
        let mut n = 0;
        loop {
            if self.term {
                return None;
            }
            match f(self) {
                Ok(v) => return Some(v),
                Err(e) => {
                    let wait = backoff(n);
                    log!(
                        "couldn't {what}: {e:#}; trying again in {}s",
                        wait.as_secs()
                    );
                    n += 1;
                    if !self.pause(wait) {
                        return None;
                    }
                }
            }
        }
    }

    fn spawn(&self, version: &str, dir: &Path) -> Result<Child> {
        let mut cmd = hub_command(&dir.join("corra-hub"));
        cmd.arg("serve")
            .env("CORRA_DATA", &self.cfg.data)
            .env("CORRA_RUNNER_BIN", dir.join("corra-runner"))
            .env("CORRA_MODAL_HELPER", dir.join("corra-modal"))
            .env("CORRA_LAUNCHER", &self.cfg.socket)
            .env("CORRA_RELEASES", &self.cfg.releases)
            .env_remove("CORRA_STORAGE_CHILD")
            .env_remove("CORRA_STORAGE_HOLDER")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(l) = &self.cfg.litestream {
            cmd.env("CORRA_LITESTREAM", l);
        }
        let mut child = cmd.spawn().with_context(|| format!("starting {version}"))?;
        let tail: Tail = Arc::new(Mutex::new(VecDeque::new()));
        if let Some(o) = child.stdout.take() {
            tee(o, std::io::stdout(), tail.clone());
        }
        if let Some(e) = child.stderr.take() {
            tee(e, std::io::stderr(), tail.clone());
        }
        log!("started hub {version} (pid {})", child.id());
        Ok(Child {
            pid: child.id() as i32,
            tail,
        })
    }

    fn stop(&mut self, child: &Child) -> i32 {
        unsafe { libc::kill(child.pid, libc::SIGTERM) };
        let mut until = Instant::now() + STOP_WAIT;
        let mut killed = false;
        loop {
            match self.next(TICK) {
                Got::Exit(pid, status) if pid == child.pid => {
                    unsafe { libc::kill(-child.pid, libc::SIGKILL) };
                    return status;
                }
                Got::Event(e) => self.refuse(e, "the hub is stopping"),
                _ => {}
            }
            if Instant::now() >= until {
                if killed {
                    log!("hub pid {} won't go; leaving it", child.pid);
                    return -1;
                }
                log!(
                    "the hub didn't stop within {}s, so it's killed",
                    STOP_WAIT.as_secs()
                );
                unsafe { libc::kill(-child.pid, libc::SIGKILL) };
                killed = true;
                until = Instant::now() + Duration::from_secs(10);
            }
        }
    }

    fn fetched(&mut self, version: String, r: std::result::Result<(), String>, live: bool) -> bool {
        let Some((v, replies)) = self.fetching.take() else {
            return false;
        };
        if v != version {
            self.fetching = Some((v, replies));
            return false;
        }
        let answer = match r {
            Err(e) => Err(e),
            Ok(()) if !live => Err("the hub stopped while the new version was downloading".into()),
            Ok(()) => store::update(&*self.state, |s| s.switch(&version))
                .map(|(st, _)| self.st = st)
                .map_err(|e| format!("couldn't record the new version: {e:#}")),
        };
        let ok = answer.is_ok();
        let msg = match answer {
            Ok(()) => json!({ "ok": true }),
            Err(e) => json!({ "error": e }),
        };
        for r in replies {
            let _ = r.send(msg.clone());
        }
        ok
    }

    fn ask_switch(&mut self, version: String, running: &str, reply: Sender<Value>) {
        if !valid_version(&version) {
            let _ = reply.send(json!({ "error": format!("{version:?} isn't a version") }));
            return;
        }
        if version == running {
            let _ = reply.send(json!({ "error": format!("the hub already runs {version}") }));
            return;
        }
        match &mut self.fetching {
            Some((v, replies)) if *v == version => {
                replies.push(reply);
                return;
            }
            Some((v, _)) => {
                let _ = reply.send(json!({ "error": format!("the hub is already getting {v}") }));
                return;
            }
            None => {}
        }
        self.fetching = Some((version.clone(), vec![reply]));
        let (local, bucket, releases, tx) = (
            self.cfg.local.clone(),
            self.bucket.clone(),
            self.releases.clone(),
            self.tx.clone(),
        );
        log!("getting {version}");
        std::thread::spawn(move || {
            let r = fetch::ensure(&version, &local, bucket.as_deref(), &*releases)
                .map(|_| ())
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Event::Fetched(version, r));
        });
    }

    fn run_one(&mut self, version: &str, dir: &Path) -> Outcome {
        let child = match self.spawn(version, dir) {
            Ok(c) => c,
            Err(e) => return Outcome::Failed(format!("{e:#}"), Vec::new()),
        };
        let trial = self.st.trial && self.st.current.as_deref() == Some(version);
        let mut run = Run::new(Instant::now());
        let mut confirmed = !trial;
        let mut next_probe = Instant::now();
        let mut next_confirm = Instant::now();
        let mut switch_at: Option<Instant> = None;
        let tail = |c: &Child| -> Vec<String> {
            let t = c.tail.lock().unwrap();
            t.iter().skip(t.len().saturating_sub(20)).cloned().collect()
        };
        loop {
            match self.next(TICK) {
                Got::Term => {
                    self.stop(&child);
                    return Outcome::Stopped;
                }
                Got::Exit(pid, status) if pid == child.pid => {
                    unsafe { libc::kill(-child.pid, libc::SIGKILL) };
                    let how = describe(status);
                    log!("hub {version} {how}");
                    let up_for = run.up.map(|u| u.elapsed()).unwrap_or_default();
                    if !confirmed {
                        return Outcome::Failed(run.exited(Instant::now(), &how), tail(&child));
                    }
                    return Outcome::Exited(up_for);
                }
                Got::Exit(..) | Got::Nothing => {}
                Got::Event(Event::Ask(Ask::Status, reply)) => {
                    let _ = reply.send(self.status(Some(version)));
                }
                Got::Event(Event::Ask(Ask::Switch { version: to }, reply)) => {
                    if switch_at.is_some() {
                        let _ = reply.send(json!({ "error": "the hub is already switching" }));
                    } else {
                        self.ask_switch(to, version, reply);
                    }
                }
                Got::Event(Event::Fetched(v, r)) => {
                    let to = v.clone();
                    if self.fetched(v, r, true) {
                        log!("switching from {version} to {to}");
                        switch_at = Some(Instant::now() + Duration::from_secs(1));
                    }
                }
            }
            let now = Instant::now();
            if switch_at.is_some_and(|at| now >= at) {
                self.stop(&child);
                return if self.term {
                    Outcome::Stopped
                } else {
                    Outcome::Switched
                };
            }
            if run.up.is_none() && now >= next_probe {
                next_probe = now + Duration::from_secs(1);
                match probe(self.cfg.probe) {
                    Some(Some(v)) if v == version => {
                        run.up = Some(now);
                        log!("hub {version} is up");
                    }
                    Some(v) => run.answered_as = Some(v.unwrap_or_default()),
                    None => {}
                }
            }
            if !confirmed && switch_at.is_none() {
                match run.judge(now) {
                    Verdict::Wait => {}
                    Verdict::Good if now >= next_confirm => {
                        match store::update(&*self.state, |s| s.confirm(version)) {
                            Ok((st, _)) => {
                                self.st = st;
                                confirmed = true;
                                log!("hub {version} stayed up, so it's kept");
                                fetch::prune(
                                    &self.cfg.local,
                                    self.bucket.as_deref(),
                                    &self.st.keep(),
                                );
                            }
                            Err(e) => {
                                log!("couldn't record that {version} is good: {e:#}");
                                next_confirm = now + Duration::from_secs(5);
                            }
                        }
                    }
                    Verdict::Good => {}
                    Verdict::Bad(why) => {
                        log!("hub {version} is no good: {why}");
                        let lines = tail(&child);
                        self.stop(&child);
                        return Outcome::Failed(why, lines);
                    }
                }
            }
        }
    }

    fn run(&mut self) -> i32 {
        let mut quick = 0u32;
        loop {
            let state = self.state.clone();
            let Some(st) = self.retry("read the launcher's state", |_| Ok(store::load(&*state)?.0))
            else {
                return 0;
            };
            self.st = st;
            let version = match self.st.current.clone() {
                Some(v) => v,
                None => {
                    let releases = self.releases.clone();
                    let Some(newest) = self.retry("find the newest release", |_| releases.newest())
                    else {
                        return 0;
                    };
                    log!("no version recorded yet, so it starts with the newest, {newest}");
                    let Some((st, _)) = self.retry("record the version", |_| {
                        store::update(&*state, |s| s.start_with(&newest))
                    }) else {
                        return 0;
                    };
                    self.st = st;
                    match self.st.current.clone() {
                        Some(v) => v,
                        None => continue,
                    }
                }
            };
            let trial = self.st.trial;
            let got = fetch::ensure(
                &version,
                &self.cfg.local,
                self.bucket.as_deref(),
                &*self.releases,
            );
            let outcome = match got {
                Ok((dir, from)) => {
                    log!(
                        "hub {version} is from {}",
                        match from {
                            fetch::From::Disk => "the data folder",
                            fetch::From::Bucket => "the bucket",
                            fetch::From::Release => "its release",
                        }
                    );
                    self.run_one(&version, &dir)
                }
                Err(e) => Outcome::Failed(format!("couldn't get it: {e:#}"), Vec::new()),
            };
            match outcome {
                Outcome::Stopped => return 0,
                Outcome::Switched => quick = 0,
                Outcome::Failed(why, lines) if trial => {
                    log!("{version} failed its first start: {why}");
                    let r = self.retry("record the failure", |_| {
                        store::update(&*state, |s| {
                            s.fail(&version, why.clone(), lines.clone(), now_ms())
                        })
                    });
                    match r {
                        None => return 0,
                        Some((st, _)) if st.current.as_deref() != Some(version.as_str()) => {
                            log!("going back to {}", st.current.as_deref().unwrap_or("?"));
                        }
                        Some(_) => {
                            if !self.pause(backoff(quick)) {
                                return 0;
                            }
                            quick += 1;
                        }
                    }
                }
                Outcome::Failed(why, _) => {
                    log!("hub {version}: {why}");
                    if !self.pause(backoff(quick)) {
                        return 0;
                    }
                    quick += 1;
                }
                Outcome::Exited(up_for) => {
                    if up_for > Duration::from_secs(300) {
                        quick = 0;
                    }
                    if !self.pause(backoff(quick)) {
                        return 0;
                    }
                    quick += 1;
                }
            }
        }
    }
}

fn main() {
    let sigs = block_signals();
    unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) };
    let cfg = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            log!("{e:#}");
            std::process::exit(2);
        }
    };
    let (tx, rx) = std::sync::mpsc::channel();
    if let Err(e) = control::serve(&cfg.socket, tx.clone()) {
        log!("{e:#}");
        std::process::exit(2);
    }
    let bucket: Option<Arc<dyn Blobs>> = match cfg.bucket.as_ref().map(S3::new).transpose() {
        Ok(b) => b.map(|b| Arc::new(b) as Arc<dyn Blobs>),
        Err(e) => {
            log!("{e:#}");
            std::process::exit(2);
        }
    };
    let state = bucket
        .clone()
        .unwrap_or_else(|| Arc::new(Disk::new(&cfg.local)) as Arc<dyn Blobs>);
    log!(
        "starting; its state is in {}, releases come from {}",
        if bucket.is_some() {
            "the bucket"
        } else {
            "the data folder"
        },
        cfg.releases
    );
    let mut l = Launcher {
        releases: Arc::new(Http::new(&cfg.releases)),
        cfg,
        sigs,
        rx,
        tx,
        bucket,
        state,
        st: State::default(),
        term: false,
        exits: VecDeque::new(),
        fetching: None,
    };
    let code = l.run();
    let _ = std::fs::remove_file(&l.cfg.socket);
    log!("stopped");
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_answers_are_read_for_the_version() {
        let ok = "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\nX-Corra-Version: 2026.11.26-1432-6793d3e\r\n\r\nok";
        assert_eq!(
            parse_health(ok),
            Some(Some("2026.11.26-1432-6793d3e".into()))
        );
        assert_eq!(parse_health("HTTP/1.1 200 OK\r\n\r\nok"), Some(None));
        assert_eq!(
            parse_health("HTTP/1.1 503 Service Unavailable\r\n\r\n"),
            None
        );
        assert_eq!(parse_health(""), None);
    }

    #[test]
    fn log_lines_lose_their_colours() {
        assert_eq!(
            strip_ansi("\x1b[2m2026\x1b[0m \x1b[31mERROR\x1b[0m boom"),
            "2026 ERROR boom"
        );
    }

    #[test]
    fn the_hub_gets_signals_the_launcher_blocks() {
        block_signals();
        let out = hub_command(Path::new("/usr/bin/grep"))
            .args(["SigBlk", "/proc/self/status"])
            .output()
            .unwrap();
        let line = String::from_utf8(out.stdout).unwrap();
        assert_eq!(
            line.split_whitespace().nth(1),
            Some("0000000000000000"),
            "{line}"
        );
    }

    #[test]
    fn backoff_grows_to_a_minute() {
        assert_eq!(backoff(0), Duration::from_secs(2));
        assert_eq!(backoff(3), Duration::from_secs(16));
        assert_eq!(backoff(20), Duration::from_secs(60));
    }
}
