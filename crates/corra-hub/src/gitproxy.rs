use std::io::{BufReader, Read, Seek, SeekFrom, Write};

use anyhow::{Result, anyhow};
use axum::body::Body;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;

use crate::auth::AppState;

fn machine_token(headers: &HeaderMap) -> Option<String> {
    let v = headers.get("authorization")?.to_str().ok()?;
    if let Some(t) = v.strip_prefix("Bearer ") {
        return Some(t.to_string());
    }
    let b = v.strip_prefix("Basic ")?;
    let decoded = base64::engine::general_purpose::STANDARD.decode(b).ok()?;
    let s = String::from_utf8(decoded).ok()?;
    Some(s.split_once(':')?.1.to_string())
}

fn denied(msg: &str) -> Response {
    (StatusCode::FORBIDDEN, format!("corra: {msg}\n")).into_response()
}

fn unauthorized() -> Response {
    let mut r = (
        StatusCode::UNAUTHORIZED,
        "corra: sign in with the machine's token\n",
    )
        .into_response();
    r.headers_mut().insert(
        "www-authenticate",
        HeaderValue::from_static("Basic realm=\"corra\""),
    );
    r
}

pub async fn proxy(
    State(s): State<AppState>,
    method: Method,
    headers: HeaderMap,
    Path(path): Path<String>,
    RawQuery(query): RawQuery,
    body: Body,
) -> Response {
    match handle(s, method, headers, path, query, body).await {
        Ok(r) => r,
        Err(e) => (StatusCode::BAD_GATEWAY, format!("corra: {e:#}\n")).into_response(),
    }
}

enum SpoolError {
    TooLarge,
    BadGzip,
    Io(anyhow::Error),
}

struct SpooledPush {
    file: std::fs::File,
    len: u64,
    header: Vec<u8>,
}

const MAX_HEADER_BYTES: usize = 4 << 20;

fn read_pkt_header(reader: &mut impl Read) -> std::io::Result<Vec<u8>> {
    let mut header = Vec::new();
    loop {
        let mut len_buf = [0u8; 4];
        match reader.read_exact(&mut len_buf) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(header),
            Err(e) => return Err(e),
        }
        header.extend_from_slice(&len_buf);
        let Ok(len_s) = std::str::from_utf8(&len_buf) else {
            return Ok(header);
        };
        let Ok(len) = usize::from_str_radix(len_s, 16) else {
            return Ok(header);
        };
        if len == 0 {
            return Ok(header);
        }
        if len < 4 || header.len() + (len - 4) > MAX_HEADER_BYTES {
            return Ok(header);
        }
        let start = header.len();
        header.resize(start + len - 4, 0);
        match reader.read_exact(&mut header[start..]) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                header.truncate(start);
                return Ok(header);
            }
            Err(e) => return Err(e),
        }
    }
}

async fn spool_receive_pack(
    body: Body,
    gzipped: bool,
    limit: usize,
) -> std::result::Result<SpooledPush, SpoolError> {
    let std_file = tempfile::tempfile().map_err(|e| SpoolError::Io(e.into()))?;
    let mut file = tokio::fs::File::from_std(std_file);
    let mut stream = body.into_data_stream();
    let mut written: usize = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| SpoolError::Io(anyhow!("reading push body: {e}")))?;
        written = written.saturating_add(chunk.len());
        if written > limit {
            return Err(SpoolError::TooLarge);
        }
        file.write_all(&chunk)
            .await
            .map_err(|e| SpoolError::Io(e.into()))?;
    }
    file.flush().await.map_err(|e| SpoolError::Io(e.into()))?;
    let raw_file = file.into_std().await;
    tokio::task::spawn_blocking(move || inspect_spooled(raw_file, written as u64, gzipped, limit))
        .await
        .map_err(|e| SpoolError::Io(e.into()))?
}

fn inspect_spooled(
    mut raw_file: std::fs::File,
    raw_len: u64,
    gzipped: bool,
    limit: usize,
) -> std::result::Result<SpooledPush, SpoolError> {
    raw_file
        .seek(SeekFrom::Start(0))
        .map_err(|e| SpoolError::Io(e.into()))?;
    let (mut out_file, len) = if gzipped {
        let mut unpacked = tempfile::tempfile().map_err(|e| SpoolError::Io(e.into()))?;
        let mut dec = flate2::read::GzDecoder::new(BufReader::new(raw_file));
        let mut buf = [0u8; 64 * 1024];
        let mut total: usize = 0;
        loop {
            let n = dec.read(&mut buf).map_err(|_| SpoolError::BadGzip)?;
            if n == 0 {
                break;
            }
            total = total.saturating_add(n);
            if total > limit {
                return Err(SpoolError::TooLarge);
            }
            unpacked
                .write_all(&buf[..n])
                .map_err(|e| SpoolError::Io(e.into()))?;
        }
        unpacked.flush().map_err(|e| SpoolError::Io(e.into()))?;
        unpacked
            .seek(SeekFrom::Start(0))
            .map_err(|e| SpoolError::Io(e.into()))?;
        (unpacked, total as u64)
    } else {
        (raw_file, raw_len)
    };
    let header = read_pkt_header(&mut out_file).map_err(|e| SpoolError::Io(e.into()))?;
    out_file
        .seek(SeekFrom::Start(0))
        .map_err(|e| SpoolError::Io(e.into()))?;
    Ok(SpooledPush {
        file: out_file,
        len,
        header,
    })
}

async fn handle(
    s: AppState,
    method: Method,
    headers: HeaderMap,
    path: String,
    query: Option<String>,
    body: Body,
) -> Result<Response> {
    let Some(token) = machine_token(&headers) else {
        return Ok(unauthorized());
    };
    let m = match s.hub.verify_git_token(&token)? {
        Some(m) => m,
        None => match s.hub.auth_machine_token(&token)? {
            Some(m) => m,
            None => return Ok(unauthorized()),
        },
    };
    let mut parts = path.splitn(3, '/');
    let (Some(owner), Some(repo), rest) = (parts.next(), parts.next(), parts.next().unwrap_or(""))
    else {
        return Ok(denied("bad repository path"));
    };
    let repo = repo.trim_end_matches(".git");
    let full = format!("{owner}/{repo}");
    let service = match rest {
        "info/refs" => query
            .as_deref()
            .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("service=")))
            .unwrap_or("")
            .to_string(),
        "git-upload-pack" => "git-upload-pack".into(),
        "git-receive-pack" => "git-receive-pack".into(),
        _ => return Ok(denied("only git's smart HTTP protocol is served here")),
    };
    if service != "git-upload-pack" && service != "git-receive-pack" {
        return Ok(denied("unsupported git service"));
    }

    let projects = s.hub.db.projects()?;
    let machine_projects: Vec<_> = projects
        .iter()
        .filter(|p| {
            p.default_machine == Some(m.id)
                || s.hub
                    .db
                    .machine(m.id)
                    .ok()
                    .flatten()
                    .and_then(|x| x.project)
                    == Some(p.id)
        })
        .collect();
    let trusted = machine_projects.iter().any(|p| p.trusted);
    let allowed = trusted
        || machine_projects.iter().any(|p| {
            p.repos.iter().any(|r| {
                r.github
                    .as_deref()
                    .map(|g| g.eq_ignore_ascii_case(&full))
                    .unwrap_or(false)
            })
        });
    if !allowed {
        return Ok(denied(&format!(
            "{full} isn't one of this machine's project repos"
        )));
    }

    let Some(gh) = s.hub.secrets.get("github.token")? else {
        return Ok(denied("the hub has no GitHub credential yet"));
    };

    let push = if rest == "git-receive-pack" && !trusted {
        let gzipped = headers
            .get("content-encoding")
            .and_then(|v| v.to_str().ok())
            == Some("gzip");
        let push = match spool_receive_pack(body, gzipped, MAX_PUSH_BYTES).await {
            Ok(p) => p,
            Err(SpoolError::TooLarge) => {
                return Ok(denied(&format!(
                    "the push is larger than {} MB unpacked",
                    MAX_PUSH_BYTES >> 20
                )));
            }
            Err(SpoolError::BadGzip) => return Ok(denied("couldn't unpack the push")),
            Err(SpoolError::Io(e)) => return Err(e),
        };
        let commands = match push_commands(&push.header) {
            Some(c) if !c.is_empty() => c,
            _ => return Ok(denied("couldn't read the push")),
        };
        let default = match default_branch(&s, &gh, &full).await {
            Ok(b) => b,
            Err(e) => {
                return Ok(denied(&format!(
                    "couldn't look up {full}'s default branch, so the push was refused: {e:#}"
                )));
            }
        };
        let sideband = first_caps(&push.header)
            .split(' ')
            .any(|c| c == "side-band-64k" || c == "side-band");
        let refuse_all = |why: &dyn Fn(&str) -> Option<String>| -> Response {
            let refs: Vec<(String, String)> = commands
                .iter()
                .map(|(_, _, r)| {
                    let w = why(r).unwrap_or_else(|| {
                        "not pushed, since another ref in this push was refused".into()
                    });
                    (r.clone(), w)
                })
                .collect();
            rejected(&refs, sideband)
        };
        if commands.iter().any(|(_, _, r)| odd_ref(r)) {
            return Ok(refuse_all(&|r: &str| {
                odd_ref(r)
                    .then(|| format!("machines push branches and tags only, so {r} wasn't pushed"))
            }));
        }
        let guarded: Vec<String> = commands
            .iter()
            .filter(|(_, _, r)| is_guarded(r, &default))
            .map(|(_, new, r)| describe(new, r))
            .collect();
        let project = machine_projects.iter().find(|p| {
            p.repos.iter().any(|r| {
                r.github
                    .as_deref()
                    .is_some_and(|g| g.eq_ignore_ascii_case(&full))
            })
        });
        if !guarded.is_empty() && !project.is_some_and(|p| p.allow_main) {
            let key = approval_key(&full, &commands, &default);
            if !granted(&s, &key) {
                let waiting = s.hub.git_pending.lock().unwrap().get(&key).cloned();
                let mut rx = match waiting {
                    Some(rx) => rx,
                    None => {
                        let Some((thread, turn)) = asking_thread(&s, m.id, &headers) else {
                            let why = format!(
                                "a push to {default} or a tag waits for a yes in one of this machine's threads, and it has none"
                            );
                            return Ok(refuse_all(&|r: &str| {
                                is_guarded(r, &default).then(|| why.clone())
                            }));
                        };
                        let (tx, rx) = tokio::sync::watch::channel(None);
                        s.hub
                            .git_pending
                            .lock()
                            .unwrap()
                            .insert(key.clone(), rx.clone());
                        let hub = s.hub.clone();
                        let input = serde_json::json!({ "repo": full, "push": guarded });
                        let k = key.clone();
                        let branch = default.clone();
                        tokio::spawn(async move {
                            let answer = match hub.ask(thread, &turn, "git.push", &input).await {
                                Ok(corra_proto::RequestResponse::Allow { .. }) => {
                                    let _ = hub.db.kv_set(
                                        &format!("push_ok:{k}"),
                                        &corra_proto::now_ms().to_string(),
                                    );
                                    Ok(())
                                }
                                Ok(corra_proto::RequestResponse::Deny { message }) => Err(message
                                    .unwrap_or_else(|| {
                                        format!("the push to {branch} or a tag was declined")
                                    })),
                                _ => Err("the push was cancelled".to_string()),
                            };
                            let _ = tx.send(Some(answer));
                            hub.git_pending.lock().unwrap().remove(&k);
                        });
                        rx
                    }
                };
                let mut shutdown = s.hub.subscribe_shutdown();
                let answer = tokio::time::timeout(std::time::Duration::from_secs(240), async {
                    loop {
                        if let Some(a) = rx.borrow().clone() {
                            return a;
                        }
                        tokio::select! {
                            _ = shutdown.changed() => return Err("the hub is restarting; run the push again".to_string()),
                            res = rx.changed() => {
                                if res.is_err() {
                                    return Err("the push was cancelled".to_string());
                                }
                            }
                        }
                    }
                })
                .await
                .unwrap_or_else(|_| {
                    Err("still waiting for a yes in the app; run the same push again once it's allowed".to_string())
                });
                if let Err(why) = answer {
                    return Ok(refuse_all(&|r: &str| {
                        is_guarded(r, &default).then(|| why.clone())
                    }));
                }
            }
        }
        push
    } else {
        // Streaming pass-through for info/refs, git-upload-pack, or trusted repos.
        let url = format!(
            "https://github.com/{full}.git/{rest}{}",
            query.map(|q| format!("?{q}")).unwrap_or_default()
        );
        let client = &s.hub.http;
        let mut req = client
            .request(method.clone(), &url)
            .basic_auth("x-access-token", Some(gh));
        if method != Method::GET {
            req = req.body(reqwest::Body::wrap_stream(body.into_data_stream()));
        }
        for h in ["content-type", "accept", "git-protocol", "content-encoding"] {
            if let Some(v) = headers.get(h) {
                req = req.header(h, v.clone());
            }
        }
        req = req.header("user-agent", "git/2 corra");
        let resp = req.send().await?;
        let status = StatusCode::from_u16(resp.status().as_u16())?;
        let mut out = Response::builder().status(status);
        for h in ["content-type", "cache-control", "expires", "pragma"] {
            if let Some(v) = resp.headers().get(h) {
                out = out.header(h, v.as_bytes());
            }
        }
        return Ok(out.body(Body::from_stream(resp.bytes_stream()))?);
    };

    let url = format!(
        "https://github.com/{full}.git/{rest}{}",
        query.map(|q| format!("?{q}")).unwrap_or_default()
    );
    let client = &s.hub.http;
    let file = tokio::fs::File::from_std(push.file);
    let mut req = client
        .request(method, &url)
        .basic_auth("x-access-token", Some(gh))
        .header("content-length", push.len)
        .body(reqwest::Body::wrap_stream(ReaderStream::new(file)));
    for h in ["content-type", "accept", "git-protocol"] {
        if let Some(v) = headers.get(h) {
            req = req.header(h, v.clone());
        }
    }
    req = req.header("user-agent", "git/2 corra");
    let resp = req.send().await?;
    let status = StatusCode::from_u16(resp.status().as_u16())?;
    let mut out = Response::builder().status(status);
    for h in ["content-type", "cache-control", "expires", "pragma"] {
        if let Some(v) = resp.headers().get(h) {
            out = out.header(h, v.as_bytes());
        }
    }
    Ok(out.body(Body::from_stream(resp.bytes_stream()))?)
}

pub const MAX_PUSH_BYTES: usize = 256 << 20;

pub fn gunzip(body: &[u8], limit: usize) -> std::io::Result<Option<Vec<u8>>> {
    let mut out = vec![];
    flate2::read::GzDecoder::new(body)
        .take(limit as u64 + 1)
        .read_to_end(&mut out)?;
    Ok((out.len() <= limit).then_some(out))
}

pub fn approval_key(full: &str, commands: &[(String, String, String)], default: &str) -> String {
    let mut exact: Vec<String> = commands
        .iter()
        .filter(|(_, _, r)| is_guarded(r, default))
        .map(|(old, new, r)| format!("{r} {old} {new}"))
        .collect();
    exact.sort();
    format!("{}|{}", full.to_lowercase(), exact.join("|"))
}

fn pkt(out: &mut Vec<u8>, data: &[u8]) {
    out.extend(format!("{:04x}", data.len() + 4).as_bytes());
    out.extend(data);
}

fn first_caps(body: &[u8]) -> String {
    let Some(len) = body
        .get(0..4)
        .and_then(|l| std::str::from_utf8(l).ok())
        .and_then(|l| usize::from_str_radix(l, 16).ok())
    else {
        return String::new();
    };
    let line = body.get(4..len).unwrap_or_default();
    match line.iter().position(|b| *b == 0) {
        Some(i) => String::from_utf8_lossy(&line[i + 1..]).trim().to_string(),
        None => String::new(),
    }
}

fn rejected(refs: &[(String, String)], sideband: bool) -> Response {
    let mut report = vec![];
    pkt(&mut report, b"unpack ok\n");
    for (r, why) in refs {
        let why = why.replace('\n', " ");
        pkt(&mut report, format!("ng {r} {why}\n").as_bytes());
    }
    report.extend(b"0000");
    let body = if sideband {
        let mut out = vec![];
        for chunk in report.chunks(65000) {
            let mut d = vec![1u8];
            d.extend(chunk);
            pkt(&mut out, &d);
        }
        out.extend(b"0000");
        out
    } else {
        report
    };
    (
        StatusCode::OK,
        [("content-type", "application/x-git-receive-pack-result")],
        body,
    )
        .into_response()
}

fn granted(s: &AppState, key: &str) -> bool {
    s.hub
        .db
        .kv_get(&format!("push_ok:{key}"))
        .ok()
        .flatten()
        .and_then(|v| v.parse::<i64>().ok())
        .is_some_and(|at| corra_proto::now_ms() - at < 3_600_000)
}

fn odd_ref(r: &str) -> bool {
    !(r.starts_with("refs/heads/") || r.starts_with("refs/tags/"))
}

pub fn is_guarded(r: &str, default: &str) -> bool {
    r.starts_with("refs/tags/") || r.strip_prefix("refs/heads/") == Some(default)
}

fn describe(new: &str, r: &str) -> String {
    let name = r
        .strip_prefix("refs/heads/")
        .or_else(|| r.strip_prefix("refs/tags/"))
        .unwrap_or(r);
    let kind = if r.starts_with("refs/tags/") {
        "tag"
    } else {
        "branch"
    };
    if new.chars().all(|c| c == '0') {
        format!("delete {kind} {name}")
    } else {
        format!("{kind} {name} → {}", &new[..new.len().min(8)])
    }
}

pub fn asking_thread(
    s: &AppState,
    machine: corra_proto::MachineId,
    headers: &HeaderMap,
) -> Option<(corra_proto::ThreadId, String)> {
    let turn_of = |tid: corra_proto::ThreadId| -> Option<String> {
        let events = s.hub.db.last_turns_started(tid, 1).ok()?;
        events.iter().find_map(|e| match &e.event {
            corra_proto::Event::TurnStarted { turn, .. } => Some(turn.clone()),
            _ => None,
        })
    };
    if let Some(tid) = headers
        .get("x-corra-thread")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<corra_proto::ThreadId>().ok())
        && let Ok(Some(t)) = s.hub.db.thread(tid)
        && t.machine == Some(machine)
        && t.hidden_at.is_none()
        && let Some(turn) = turn_of(t.id)
    {
        return Some((t.id, turn));
    }
    let t = s
        .hub
        .db
        .threads()
        .ok()?
        .into_iter()
        .filter(|t| t.machine == Some(machine) && t.hidden_at.is_none())
        .max_by_key(|t| {
            (
                matches!(
                    t.status,
                    corra_proto::ThreadStatus::Running | corra_proto::ThreadStatus::Waiting
                ),
                t.updated_at,
            )
        })?;
    Some((t.id, turn_of(t.id)?))
}

pub fn push_commands(body: &[u8]) -> Option<Vec<(String, String, String)>> {
    let mut out = vec![];
    let mut i = 0;
    loop {
        let len_s = std::str::from_utf8(body.get(i..i + 4)?).ok()?;
        let len = usize::from_str_radix(len_s, 16).ok()?;
        if len == 0 {
            return Some(out);
        }
        if len < 4 || i + len > body.len() {
            return None;
        }
        let line = &body[i + 4..i + len];
        let line = line.split(|b| *b == 0).next().unwrap_or(line);
        let text = String::from_utf8_lossy(line).trim_end().to_string();
        i += len;
        if text.starts_with("shallow ") {
            continue;
        }
        if text == "push-cert" || text.starts_with("push-cert") {
            return None;
        }
        let mut it = text.splitn(3, ' ');
        match (it.next(), it.next(), it.next()) {
            (Some(o), Some(n), Some(r))
                if o.len() >= 40
                    && n.len() >= 40
                    && o.chars().chain(n.chars()).all(|c| c.is_ascii_hexdigit()) =>
            {
                out.push((o.to_string(), n.to_string(), r.to_string()))
            }
            _ => return None,
        }
    }
}

async fn default_branch(s: &AppState, token: &str, full: &str) -> Result<String> {
    let key = format!("default_branch:{full}");
    if let Some(v) = s.hub.db.kv_get(&key)?
        && let Ok(c) = serde_json::from_str::<serde_json::Value>(&v)
        && let (Some(b), Some(at)) = (c["branch"].as_str(), c["at"].as_i64())
        && corra_proto::now_ms() - at < 86_400_000
    {
        return Ok(b.to_string());
    }
    let v: serde_json::Value = s
        .hub
        .http
        .get(format!("https://api.github.com/repos/{full}"))
        .bearer_auth(token)
        .header("user-agent", "corra")
        .header("accept", "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let b = v["default_branch"]
        .as_str()
        .ok_or_else(|| anyhow!("no default branch"))?
        .to_string();
    s.hub.db.kv_set(
        &key,
        &serde_json::json!({ "branch": b, "at": corra_proto::now_ms() }).to_string(),
    )?;
    Ok(b)
}
