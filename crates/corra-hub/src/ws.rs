use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use corra_proto::app::{AppCall, AppRequest, HubToApp};
use corra_proto::runner::{HubMsg, RunnerMsg, RunnerRequest};
use corra_proto::*;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};

use crate::auth::AppState;
use crate::hub::{HubRef, Outbound, RunnerConn, hash_token};

pub async fn runner_ws(
    State(AppState { hub, .. }): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    let auth = match hub.db.machine_by_token(&hash_token(token)) {
        Ok(Some(a)) if a.status != MachineStatus::Destroyed => a,
        _ => return StatusCode::UNAUTHORIZED.into_response(),
    };
    if hub
        .runners
        .lock()
        .unwrap()
        .get(&auth.id)
        .is_some_and(|c| !c.tx.is_closed())
    {
        tracing::warn!(machine = %auth.id, "refused a second connection with a token that's already connected");
        return (StatusCode::CONFLICT, "this machine is already connected").into_response();
    }
    ws.max_message_size(64 << 20)
        .on_upgrade(move |socket| async move {
            if let Err(e) = runner_loop(hub, auth.id, socket).await {
                tracing::warn!(machine = %auth.id, "runner connection ended: {e:#}");
            }
        })
}

async fn runner_loop(hub: HubRef, machine: MachineId, socket: WebSocket) -> Result<()> {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<HubMsg>();
    let conn_id = hub.next_id();

    let writer = tokio::spawn(async move {
        let mut ping = tokio::time::interval(Duration::from_secs(20));
        loop {
            tokio::select! {
                msg = rx.recv() => {
                    let Some(msg) = msg else { break };
                    let text = match serde_json::to_string(&msg) { Ok(t) => t, Err(_) => continue };
                    if sink.send(Message::Text(text.into())).await.is_err() { break }
                }
                _ = ping.tick() => {
                    if sink.send(Message::Ping(Default::default())).await.is_err() { break }
                }
            }
        }
    });

    let first = tokio::time::timeout(Duration::from_secs(15), stream.next())
        .await
        .map_err(|_| anyhow!("no hello"))?;
    let Some(Ok(Message::Text(text))) = first else {
        return Err(anyhow!("expected hello"));
    };
    let RunnerMsg::Hello {
        protocol,
        info,
        threads,
    } = serde_json::from_str(&text)?
    else {
        return Err(anyhow!("expected hello"));
    };
    if protocol != PROTOCOL_VERSION {
        return Err(anyhow!(
            "runner speaks protocol {protocol}, hub {PROTOCOL_VERSION}"
        ));
    }
    hub.db.set_machine_info(machine, &info)?;
    hub.db
        .set_machine_status(machine, MachineStatus::Connected)?;

    let threads: Vec<_> = threads
        .into_iter()
        .filter(|t| owns(&hub, machine, t.thread))
        .collect();
    let live: HashSet<ThreadId> = threads.iter().map(|t| t.thread).collect();
    let mut acks = vec![];
    for t in &threads {
        acks.push((t.thread, hub.db.runner_seq(t.thread)?));
    }
    for t in hub.db.threads_on_machine(machine)? {
        if !live.contains(&t.id) {
            hub.background_gone(t.id);
        }
        if !live.contains(&t.id)
            && matches!(
                t.status,
                ThreadStatus::Running | ThreadStatus::Waiting | ThreadStatus::Starting
            )
        {
            hub.hub_notice(
                t.id,
                NoticeLevel::Warning,
                "The machine restarted; the agent stopped mid-turn.",
            );
            if !hub.close_open_turn(t.id)? {
                hub.tell_follower_text(t.id, "stopped mid-turn because its machine restarted");
            }
            hub.set_thread_status(t.id, ThreadStatus::Idle)?;
        }
    }
    let old = hub.runners.lock().unwrap().insert(
        machine,
        RunnerConn {
            tx: tx.clone(),
            live,
            prepared: HashSet::new(),
            conn_id,
        },
    );
    drop(old);
    let _ = tx.send(HubMsg::Welcome {
        machine,
        acks,
        runner: crate::files::runner_build(),
    });
    hub.touch_machine(machine);
    hub.publish_machine(machine);
    tracing::info!(%machine, host = %info.hostname, "runner connected");

    let result: Result<()> = async {
        loop {
            let msg = match tokio::time::timeout(Duration::from_secs(60), stream.next()).await {
                Ok(Some(m)) => m,
                Ok(None) => break,
                Err(_) => return Err(anyhow!("the runner went silent")),
            };
            let text = match msg? {
                Message::Text(t) => t,
                Message::Close(_) => break,
                _ => continue,
            };
            let msg: RunnerMsg = match serde_json::from_str(&text) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!("bad runner message: {e}");
                    continue;
                }
            };
            handle_runner_msg(&hub, machine, &tx, msg).await?;
        }
        Ok(())
    }
    .await;

    {
        let mut runners = hub.runners.lock().unwrap();
        if runners.get(&machine).map(|c| c.conn_id) == Some(conn_id) {
            runners.remove(&machine);
        }
    }
    if !hub.runner_connected(machine) {
        if let Ok(Some(m)) = hub.db.machine(machine)
            && m.status == MachineStatus::Connected
        {
            hub.db.set_machine_status(machine, MachineStatus::Offline)?;
        }
        hub.publish_machine(machine);
    }
    writer.abort();
    tracing::info!(%machine, "runner disconnected");
    result
}

fn owns(hub: &HubRef, machine: MachineId, thread: ThreadId) -> bool {
    matches!(hub.db.thread(thread), Ok(Some(t)) if t.machine == Some(machine))
}

async fn handle_runner_msg(
    hub: &HubRef,
    machine: MachineId,
    tx: &mpsc::UnboundedSender<HubMsg>,
    msg: RunnerMsg,
) -> Result<()> {
    match msg {
        RunnerMsg::Hello { .. } => {}
        RunnerMsg::TunnelOpen { id, port } => {
            let is_phone = crate::phone::is_machine(hub, machine);
            let target = hub.phone.forwards.lock().unwrap().get(&port).copied();
            match target.filter(|t| is_phone && hub.runner_connected(*t)) {
                Some(t) => {
                    hub.phone
                        .tunnels
                        .lock()
                        .unwrap()
                        .insert(id.clone(), (machine, t));
                    hub.touch_machine(t);
                    let _ = hub.send_runner_pub(t, HubMsg::TunnelOpen { id, port });
                }
                None => {
                    let _ = tx.send(HubMsg::TunnelClose { id });
                }
            }
        }
        RunnerMsg::TunnelData { id, data } => {
            let other = hub
                .phone
                .tunnels
                .lock()
                .unwrap()
                .get(&id)
                .and_then(|(a, b)| {
                    if *a == machine {
                        Some(*b)
                    } else if *b == machine {
                        Some(*a)
                    } else {
                        None
                    }
                });
            if let Some(o) = other {
                hub.touch_machine(machine);
                let _ = hub.send_runner_pub(o, HubMsg::TunnelData { id, data });
            }
        }
        RunnerMsg::TunnelClose { id } => {
            let pair = {
                let mut t = hub.phone.tunnels.lock().unwrap();
                match t.get(&id) {
                    Some((a, b)) if *a == machine || *b == machine => t.remove(&id),
                    _ => None,
                }
            };
            if let Some((a, b)) = pair {
                let o = if a == machine { b } else { a };
                let _ = hub.send_runner_pub(o, HubMsg::TunnelClose { id });
            }
        }
        RunnerMsg::Event {
            thread,
            seq,
            ts,
            event,
        } => {
            if !owns(hub, machine, thread) {
                return Ok(());
            }
            hub.append(thread, Some(seq), ts, event)?;
            let _ = tx.send(HubMsg::Ack { thread, seq });
        }
        RunnerMsg::Delta {
            thread,
            turn,
            item,
            parent,
            delta,
        } => {
            if !owns(hub, machine, thread) {
                return Ok(());
            }
            hub.publish(Outbound::Thread(
                thread,
                HubToApp::Delta {
                    delta: LiveDelta {
                        thread,
                        turn,
                        item,
                        parent,
                        delta,
                    },
                },
            ));
        }
        RunnerMsg::ThreadStatus { thread, status } => {
            if !owns(hub, machine, thread) {
                return Ok(());
            }
            hub.runner_status(thread, status)?;
        }
        RunnerMsg::Status { thread, status } => {
            if owns(hub, machine, thread) {
                hub.set_live_status(thread, status);
            }
        }
        RunnerMsg::TermOutput { term, data } => {
            let owner = hub.terms.lock().unwrap().get(&term).copied();
            if let Some((thread, m)) = owner
                && m == machine
            {
                hub.touch_machine(machine);
                hub.publish(Outbound::Thread(
                    thread,
                    HubToApp::TermOutput { thread, term, data },
                ));
            }
        }
        RunnerMsg::TermExit { term } => {
            let owner = hub.terms.lock().unwrap().remove(&term);
            if let Some((thread, m)) = owner
                && m == machine
            {
                hub.publish(Outbound::Thread(
                    thread,
                    HubToApp::TermExit { thread, term },
                ));
            }
        }
        RunnerMsg::BrowserFrame {
            data,
            width,
            height,
            url,
            title,
        } => {
            hub.touch_machine(machine);
            hub.publish(Outbound::Machine(
                machine,
                HubToApp::BrowserFrame {
                    machine,
                    data,
                    width,
                    height,
                    url,
                    title,
                },
            ));
        }
        RunnerMsg::PhoneVideo { width, height } => {
            hub.publish(Outbound::Machine(
                machine,
                HubToApp::PhoneVideo {
                    machine,
                    width,
                    height,
                },
            ));
        }
        RunnerMsg::PhoneFrame { data, config, key } => {
            hub.publish(Outbound::Machine(
                machine,
                HubToApp::PhoneFrame {
                    machine,
                    data,
                    config,
                    key,
                },
            ));
        }
        RunnerMsg::ThreadEnded { thread, error } => {
            if let Some(c) = hub.runners.lock().unwrap().get_mut(&machine) {
                c.live.remove(&thread);
            }
            if owns(hub, machine, thread) {
                let status = if error.is_some() {
                    ThreadStatus::Error
                } else {
                    ThreadStatus::Idle
                };
                hub.runner_status(thread, status)?;
            }
        }
        RunnerMsg::Reply { id, ok, err } => {
            hub.resolve_pending(
                machine,
                id,
                match err {
                    Some(e) => Err(e),
                    None => Ok(ok.unwrap_or(Value::Null)),
                },
            );
        }
        RunnerMsg::Call {
            id,
            thread,
            turn,
            action,
            input,
        } => {
            if !owns(hub, machine, thread) {
                let _ = tx.send(HubMsg::CallResult {
                    id,
                    ok: None,
                    err: Some("not your thread".into()),
                });
                return Ok(());
            }
            let hub = hub.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                let r = crate::tools::call(&hub, machine, thread, &turn, &action, input).await;
                let (ok, err) = match r {
                    Ok(v) => (Some(v), None),
                    Err(e) => (None, Some(format!("{e:#}"))),
                };
                let _ = tx.send(HubMsg::CallResult { id, ok, err });
            });
        }
    }
    Ok(())
}

#[derive(serde::Deserialize)]
pub struct AppQuery {
    token: Option<String>,
}

pub async fn app_ws(
    State(AppState { hub, auth, .. }): State<AppState>,
    jar: axum_extra::extract::CookieJar,
    headers: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<AppQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let offered = headers
        .get("sec-websocket-protocol")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let header_token = offered
        .split(',')
        .map(str::trim)
        .find(|p| p.starts_with("cs_"))
        .map(String::from);
    let token = header_token.as_deref().or(q.token.as_deref());
    let Some(session) = crate::auth::request_session(&hub, &auth, &jar, &headers, token) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    ws.protocols(["corra.v1"])
        .max_message_size(64 << 20)
        .on_upgrade(move |socket| async move {
            if let Err(e) = app_loop(hub, socket, session).await {
                tracing::debug!("app connection ended: {e:#}");
            }
        })
}

pub struct Client {
    pub id: u64,
    pub session: String,
}

static NEXT_CLIENT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

async fn app_loop(hub: HubRef, socket: WebSocket, session: String) -> Result<()> {
    let client = Arc::new(Client {
        id: NEXT_CLIENT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        session,
    });
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let mut sub = hub.apps.subscribe();
    let subs: Arc<std::sync::Mutex<HashSet<ThreadId>>> = Default::default();
    let browsers: Arc<std::sync::Mutex<HashSet<MachineId>>> = Default::default();
    let queued = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let w_queued = queued.clone();

    let writer = tokio::spawn(async move {
        let mut ping = tokio::time::interval(Duration::from_secs(25));
        loop {
            tokio::select! {
                m = rx.recv() => {
                    let Some(m) = m else { break };
                    let sent = sink.send(Message::Text(m.into())).await;
                    let _ = w_queued.try_update(
                        std::sync::atomic::Ordering::Relaxed,
                        std::sync::atomic::Ordering::Relaxed,
                        |v| Some(v.saturating_sub(1)),
                    );
                    if sent.is_err() { break }
                }
                _ = ping.tick() => {
                    if sink.send(Message::Ping(Default::default())).await.is_err() { break }
                }
            }
        }
    });

    let fwd_tx = tx.clone();
    let fwd_queued = queued.clone();
    let fwd_subs = subs.clone();
    let fwd_browsers = browsers.clone();
    let fwd_client = client.clone();
    let forwarder = tokio::spawn(async move {
        let mut skipping = false;
        loop {
            match sub.recv().await {
                Ok(out) => {
                    let msg = match &*out {
                        Outbound::All(m) => m,
                        Outbound::Session(s, m) => {
                            if *s != fwd_client.session {
                                continue;
                            }
                            m
                        }
                        Outbound::Thread(t, m) => {
                            if !fwd_subs.lock().unwrap().contains(t) {
                                continue;
                            }
                            m
                        }
                        Outbound::Machine(mid, m) => {
                            if !fwd_browsers.lock().unwrap().contains(mid) {
                                continue;
                            }
                            let behind = fwd_queued.load(std::sync::atomic::Ordering::Relaxed) > 32;
                            match m {
                                HubToApp::BrowserFrame { .. } if behind => continue,
                                HubToApp::PhoneFrame {
                                    config: false,
                                    key: false,
                                    ..
                                } if behind || skipping => {
                                    skipping = true;
                                    continue;
                                }
                                HubToApp::PhoneFrame { key: true, .. } => skipping = false,
                                _ => {}
                            }
                            m
                        }
                    };
                    if let Ok(s) = serde_json::to_string(msg) {
                        fwd_queued.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if fwd_tx.send(s).is_err() {
                            break;
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => break,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let result: Result<()> = async {
        send_json(&tx, &hub.snapshot()?);
        if let Some(usage) = hub.usage() {
            send_json(&tx, &usage);
        }

        while let Some(msg) = stream.next().await {
            let text = match msg? {
                Message::Text(t) => t,
                Message::Close(_) => break,
                _ => continue,
            };
            let req: AppRequest = match serde_json::from_str(&text) {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("bad app message: {e}");
                    continue;
                }
            };
            let id = req.id;
            let hub = hub.clone();
            let tx2 = tx.clone();
            let subs = subs.clone();
            let browsers = browsers.clone();
            let client = client.clone();
            tokio::spawn(async move {
                let reply =
                    match handle_app_call(&hub, &tx2, &subs, &browsers, &client, req.body).await {
                        Ok(v) => HubToApp::Reply {
                            id,
                            ok: Some(v),
                            err: None,
                        },
                        Err(e) => HubToApp::Reply {
                            id,
                            ok: None,
                            err: Some(format!("{e:#}")),
                        },
                    };
                send_json(&tx2, &reply);
            });
            if forwarder.is_finished() {
                break;
            }
        }
        Ok(())
    }
    .await;
    forwarder.abort();
    writer.abort();
    for t in subs.lock().unwrap().drain() {
        hub.watch(t, false);
    }
    for m in browsers.lock().unwrap().drain() {
        hub.browser_unwatch(m);
    }
    crate::notify::set_presence(&hub, client.id, false);
    result
}

fn send_json(tx: &mpsc::UnboundedSender<String>, msg: &HubToApp) {
    if let Ok(s) = serde_json::to_string(msg) {
        let _ = tx.send(s);
    }
}

async fn handle_app_call(
    hub: &HubRef,
    tx: &mpsc::UnboundedSender<String>,
    subs: &Arc<std::sync::Mutex<HashSet<ThreadId>>>,
    browsers: &Arc<std::sync::Mutex<HashSet<MachineId>>>,
    client: &Client,
    call: AppCall,
) -> Result<Value> {
    Ok(match call {
        AppCall::Presence { focused } => {
            crate::notify::set_presence(hub, client.id, focused);
            Value::Null
        }
        AppCall::PushKey => json!({
            "key": crate::notify::public_key(hub)?,
            "on": crate::notify::subscribed(hub, &client.session),
        }),
        AppCall::PushSubscribe {
            endpoint,
            p256dh,
            auth,
        } => {
            crate::notify::subscribe(hub, &client.session, &endpoint, &p256dh, &auth)?;
            Value::Null
        }
        AppCall::PushUnsubscribe => {
            crate::notify::unsubscribe(hub, &client.session)?;
            Value::Null
        }
        AppCall::PushTest => {
            crate::notify::test(hub, &client.session);
            Value::Null
        }
        AppCall::BrowserOpen {
            thread,
            width,
            height,
        } => {
            let (_, m) = hub.machine_for_thread(thread).await?;
            let v = hub
                .runner_request(m, RunnerRequest::BrowserOpen { width, height })
                .await?;
            if browsers.lock().unwrap().insert(m) {
                *hub.browser_watch.lock().unwrap().entry(m).or_insert(0) += 1;
            }
            v
        }
        AppCall::BrowserInput {
            thread,
            method,
            params,
        } => {
            let (_, m) = hub.machine_for_thread(thread).await?;
            hub.send_runner_pub(m, HubMsg::BrowserInput { method, params })?;
            Value::Null
        }
        AppCall::BrowserClose { thread } => {
            if let Ok((_, m)) = hub.machine_for_thread(thread).await
                && browsers.lock().unwrap().remove(&m)
            {
                hub.browser_unwatch(m);
            }
            Value::Null
        }
        AppCall::PhoneUp => {
            if crate::phone::host(hub).is_none() {
                return Err(anyhow!(
                    "the phone is off: pick where it runs in Settings, under Tools"
                ));
            }
            let hub = hub.clone();
            tokio::spawn(async move {
                if let Err(e) = crate::phone::up(&hub).await {
                    tracing::warn!("the phone didn't start: {e:#}");
                }
            });
            Value::Null
        }
        AppCall::PhoneDown => {
            crate::phone::down(hub).await?;
            Value::Null
        }
        AppCall::PhoneOpen => {
            let m = crate::phone::machine(hub)
                .map(|m| m.id)
                .ok_or_else(|| anyhow!("no phone is running"))?;
            if browsers.lock().unwrap().insert(m) {
                *hub.browser_watch.lock().unwrap().entry(m).or_insert(0) += 1;
            }
            crate::phone::act(hub, PhoneOp::Stream).await?
        }
        AppCall::PhoneInput { input } => {
            if let Some(m) = crate::phone::machine(hub) {
                hub.touch_machine(m.id);
                hub.send_runner_pub(m.id, HubMsg::PhoneInput { input })?;
            }
            Value::Null
        }
        AppCall::PhoneClose => {
            if let Some(m) = crate::phone::machine(hub)
                && browsers.lock().unwrap().remove(&m.id)
            {
                hub.browser_unwatch(m.id);
            }
            Value::Null
        }
        AppCall::Phone { action } => crate::phone::act(hub, action).await?,
        AppCall::Subscribe {
            thread,
            after_seq,
            tail,
        } => {
            if subs.lock().unwrap().insert(thread) {
                hub.watch(thread, true);
            }
            let mut after = after_seq;
            let last = hub.db.thread(thread)?.map(|t| t.last_seq).unwrap_or(0);
            if after > last {
                send_json(tx, &HubToApp::Reset { thread });
                after = 0;
            }
            if let Some(turns) = tail.filter(|n| *n > 0) {
                let h = hub.history_before(thread, i64::MAX, turns as i64)?;
                if h.earlier && after < h.from_seq - 1 {
                    after = h.events.last().map(|e| e.seq).unwrap_or(after);
                    send_json(
                        tx,
                        &HubToApp::Window {
                            thread,
                            events: h.events,
                            from_seq: h.from_seq,
                            earlier: true,
                        },
                    );
                }
            }
            loop {
                let events = hub.db.events_after(thread, after, 500)?;
                let n = events.len();
                if let Some(last) = events.last() {
                    after = last.seq;
                }
                if n > 0 {
                    send_json(tx, &HubToApp::Events { thread, events });
                }
                if n < 500 {
                    break;
                }
            }
            if let Some(status) = hub.live_status(thread) {
                send_json(tx, &HubToApp::Status { thread, status });
            }
            send_json(
                tx,
                &HubToApp::Queue {
                    thread,
                    items: hub.queued(thread),
                },
            );
            json!({ "last_seq": after })
        }
        AppCall::Unsubscribe { thread } => {
            if subs.lock().unwrap().remove(&thread) {
                hub.watch(thread, false);
            }
            Value::Null
        }
        call @ (AppCall::CreateThread { .. } | AppCall::ForkThread { .. }) => {
            let v = exec_call(hub, call).await?;
            if let Some(id) = v["id"].as_str().and_then(|s| s.parse::<ThreadId>().ok())
                && subs.lock().unwrap().insert(id)
            {
                hub.watch(id, true);
            }
            v
        }
        other => exec_call(hub, other).await?,
    })
}

const EARLIER_TURNS: i64 = 6;

pub fn is_credential(name: &str) -> bool {
    !(name.starts_with("env.")
        || name.starts_with("machine_token.")
        || name.starts_with("push.")
        || name == "key-check")
}

pub fn exec_call<'a>(
    hub: &'a HubRef,
    call: AppCall,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value>> + Send + 'a>> {
    Box::pin(exec_call_inner(hub, call))
}

async fn exec_call_inner(hub: &HubRef, call: AppCall) -> Result<Value> {
    Ok(match call {
        AppCall::Earlier { thread, before_seq } => {
            let h = hub.history_before(thread, before_seq, EARLIER_TURNS)?;
            json!({ "events": h.events, "from_seq": h.from_seq, "earlier": h.earlier })
        }
        AppCall::Subscribe { .. }
        | AppCall::Unsubscribe { .. }
        | AppCall::BrowserOpen { .. }
        | AppCall::BrowserInput { .. }
        | AppCall::BrowserClose { .. }
        | AppCall::PhoneUp
        | AppCall::PhoneDown
        | AppCall::PhoneOpen
        | AppCall::PhoneInput { .. }
        | AppCall::PhoneClose
        | AppCall::Phone { .. }
        | AppCall::Presence { .. }
        | AppCall::PushKey
        | AppCall::PushSubscribe { .. }
        | AppCall::PushUnsubscribe
        | AppCall::PushTest => Value::Null,
        AppCall::CreateProject {
            name,
            repos,
            machine,
            path,
            provider,
            kind,
            size,
            setup,
            env,
            tools,
            permission_mode,
            allow_main,
            trusted,
        } => serde_json::to_value(hub.create_project(crate::hub::NewProject {
            name,
            repos,
            machine,
            path,
            provider,
            kind,
            size,
            setup,
            env,
            tools,
            permission_mode,
            allow_main: allow_main.unwrap_or(false),
            trusted: trusted.unwrap_or(false),
        })?)?,
        AppCall::AddRepo { project, github } => {
            let mut p = hub
                .db
                .project(project)?
                .ok_or_else(|| anyhow!("no such project"))?;
            let full = github.trim().trim_end_matches(".git").to_string();
            let short = full.rsplit('/').next().unwrap_or(&full).to_string();
            if !full.contains('/') {
                anyhow::bail!("give the repo as owner/name");
            }
            if !p.repos.iter().any(|r| r.name == short) {
                p.repos.push(Repo {
                    name: short,
                    github: Some(full),
                });
                hub.db.update_project(&p)?;
                hub.publish_project(&p);
            }
            let awake = hub
                .db
                .machines()?
                .into_iter()
                .find(|m| m.project == Some(p.id) && hub.runner_connected(m.id));
            if let Some(m) = awake {
                let hub = hub.clone();
                tokio::spawn(async move {
                    let _ = crate::kit::prepare(&hub, &p, m.id).await;
                });
            }
            Value::Null
        }
        AppCall::ResetMachine { project } => {
            crate::lifecycle::reset_project(hub, project).await?;
            Value::Null
        }
        AppCall::ListRepos => crate::github::list_repos(hub).await?,
        AppCall::ListSetups => json!(crate::kit::setups(hub).await?),
        AppCall::ListEnv => json!(hub.env_names()?),
        AppCall::SetEnv { name, value } => {
            let name = name.trim();
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                anyhow::bail!("an env var name may use letters, digits and '_'");
            }
            hub.secrets.set(&format!("env.{name}"), &value)?;
            Value::Null
        }
        AppCall::DeleteEnv { name } => {
            hub.secrets.delete(&format!("env.{}", name.trim()))?;
            Value::Null
        }
        AppCall::CreateThread {
            project,
            agent,
            machine,
            settings,
            input,
        } => serde_json::to_value(
            hub.create_thread(project, agent, machine, settings, input)
                .await?,
        )?,
        AppCall::Send {
            thread,
            input,
            settings,
            queue,
            client_id,
            compact,
        } => {
            let queued = if compact == Some(true) {
                crate::schedule::compact_then_send(hub, thread, input, settings, client_id).await?;
                true
            } else {
                hub.send_once(thread, input, settings, queue, client_id)
                    .await?
            };
            json!({ "queued": queued })
        }
        AppCall::DeleteThread { thread } => {
            let t = hub
                .db
                .thread(thread)?
                .ok_or_else(|| anyhow!("no such thread"))?;
            if t.hidden_at.is_none() {
                anyhow::bail!("move the thread to the trash first");
            }
            hub.stop_live(&t);
            hub.remove_thread(thread)?;
            Value::Null
        }
        AppCall::Unqueue { thread, id, send } => {
            let item = hub
                .unqueue(thread, &id)
                .ok_or_else(|| anyhow!("that message already went"))?;
            if send {
                hub.send(thread, item.input, None, false).await?;
            }
            Value::Null
        }
        AppCall::Interrupt { thread } => json!({ "unsent": hub.interrupt(thread)? }),
        AppCall::StopAgent { thread, tool } => {
            hub.stop_task(thread, tool)?;
            Value::Null
        }
        AppCall::Respond {
            thread,
            request,
            response,
        } => {
            hub.respond(thread, request, response)?;
            Value::Null
        }
        AppCall::AnswerSecret {
            thread,
            request,
            value,
        } => {
            hub.answer_secret(thread, request, value)?;
            Value::Null
        }
        AppCall::UpdateThread {
            thread,
            title,
            pinned,
            settled,
            hidden,
            settings,
        } => {
            let mut t = hub
                .db
                .thread(thread)?
                .ok_or_else(|| anyhow!("no such thread"))?;
            let now = now_ms();
            if let Some(title) = title {
                t.title = title;
            }
            if let Some(p) = pinned {
                t.pinned_at = p.then_some(now);
            }
            if let Some(s) = settled {
                t.settled_at = s.then_some(now);
            }
            if let Some(h) = hidden {
                t.hidden_at = h.then_some(now);
            }
            hub.db.update_thread(&t)?;
            hub.publish_thread(&t);
            if let Some(s) = settings {
                hub.update_settings(thread, s)?;
            }
            Value::Null
        }
        AppCall::MoveThread { thread, machine } => {
            serde_json::to_value(hub.move_thread(thread, machine).await?)?
        }
        AppCall::ForkThread { thread, turn } => {
            serde_json::to_value(hub.fork_thread(thread, turn).await?)?
        }
        AppCall::RewindThread { thread, turn } => {
            hub.rewind_thread(thread, turn).await?;
            Value::Null
        }
        AppCall::UpdateMachine { machine, name } => {
            hub.db
                .machine(machine)?
                .ok_or_else(|| anyhow!("no such machine"))?;
            if let Some(n) = name {
                let n = n.trim();
                if !n.is_empty() {
                    hub.db.set_machine_name(machine, n)?;
                }
            }
            hub.publish_machine(machine);
            Value::Null
        }
        AppCall::BackupNow => bail_local(hub, crate::backup::run(hub, &hub.data).await)?,
        AppCall::BackupStatus => crate::backup::status(hub),
        AppCall::Sessions => crate::auth::list_sessions(hub)?,
        AppCall::RevokeSession { session } => {
            crate::auth::revoke_session(hub, &session)?;
            Value::Null
        }
        AppCall::GetSettings => hub.settings(),
        AppCall::SetSettings { settings } => {
            let mut cur = hub.settings();
            if let (Some(c), Some(n)) = (cur.as_object_mut(), settings.as_object()) {
                for (k, v) in n {
                    c.insert(k.clone(), v.clone());
                }
            }
            hub.db.kv_set("settings", &cur.to_string())?;
            cur
        }
        AppCall::SetSecret { name, value } => {
            let name = name.trim();
            if name.is_empty()
                || !is_credential(name)
                || !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
            {
                anyhow::bail!("a credential name may use letters, digits, '.', '_' and '-'");
            }
            hub.secrets.set(name, value.trim())?;
            Value::Null
        }
        AppCall::DeleteSecret { name } => {
            if !is_credential(&name) {
                anyhow::bail!("there's no credential called {name}");
            }
            hub.secrets.delete(&name)?;
            Value::Null
        }
        AppCall::PhoneHost => json!({
            "host": crate::phone::host(hub),
            "machine": crate::phone::machine(hub).map(|m| m.id),
        }),
        AppCall::SetPhoneHost { host } => {
            serde_json::to_value(crate::phone::switch(hub, host).await?)?
        }
        AppCall::Leftovers => serde_json::to_value(crate::leftovers::list(hub))?,
        AppCall::RetryLeftovers => serde_json::to_value(crate::leftovers::retry(hub).await)?,
        AppCall::ListProviders => serde_json::to_value(hub.providers.info(hub))?,
        AppCall::ListTools => serde_json::to_value(crate::tools::info(hub))?,
        AppCall::ListSecrets => json!(
            hub.secrets
                .names()?
                .into_iter()
                .filter(|n| is_credential(n))
                .collect::<Vec<_>>()
        ),
        AppCall::DestroyMachine { machine } => {
            hub.providers.destroy_machine(hub, machine).await?;
            for t in hub.db.threads_on_machine(machine)? {
                if matches!(
                    t.status,
                    ThreadStatus::Running | ThreadStatus::Waiting | ThreadStatus::Starting
                ) {
                    hub.close_open_turn(t.id)?;
                    hub.set_thread_status(t.id, ThreadStatus::Idle)?;
                }
            }
            Value::Null
        }
        AppCall::UpdateProject {
            project,
            repos,
            default_machine,
            provider,
            kind,
            size,
            settings,
            setup,
            env,
            tools,
            allow_main,
            trusted,
        } => {
            let mut p = hub
                .db
                .project(project)?
                .ok_or_else(|| anyhow!("no such project"))?;
            if let Some(r) = repos {
                p.repos = r;
            }
            if let Some(s) = setup {
                p.setup = Some(s.trim().to_string()).filter(|s| !s.is_empty());
            }
            if let Some(e) = env {
                p.env = e;
            }
            if let Some(t) = tools {
                p.tools = t;
            }
            if let Some(a) = allow_main {
                p.allow_main = a;
            }
            if let Some(t) = trusted {
                p.trusted = t;
            }
            if let Some(m) = default_machine {
                p.default_machine = Some(m);
            }
            if let Some(s) = settings {
                p.settings = s;
            }
            hub.place_project(&mut p, provider, kind, size)?;
            hub.db.update_project(&p)?;
            crate::lifecycle::retire_moved(hub, p.id);
            hub.publish_project(&p);
            serde_json::to_value(p)?
        }
        AppCall::TermOpen { thread, cols, rows } => {
            let (t, m) = hub.machine_for_thread(thread).await?;
            let term = format!("t{}", new_id().simple());
            hub.terms.lock().unwrap().insert(term.clone(), (thread, m));
            if let Err(e) = hub
                .runner_request(
                    m,
                    RunnerRequest::TermOpen {
                        term: term.clone(),
                        cwd: t.cwd.clone(),
                        cols,
                        rows,
                    },
                )
                .await
            {
                hub.terms.lock().unwrap().remove(&term);
                return Err(e);
            }
            json!({ "term": term })
        }
        AppCall::TermAttach { thread, term } => {
            let m = term_machine(hub, thread, &term)?;
            hub.runner_request(m, RunnerRequest::TermAttach { term })
                .await?
        }
        AppCall::TermInput { thread, term, data } => {
            let m = term_machine(hub, thread, &term)?;
            hub.send_runner_pub(m, HubMsg::TermInput { term, data })?;
            Value::Null
        }
        AppCall::TermResize {
            thread,
            term,
            cols,
            rows,
        } => {
            let m = term_machine(hub, thread, &term)?;
            hub.send_runner_pub(m, HubMsg::TermResize { term, cols, rows })?;
            Value::Null
        }
        AppCall::TermClose { thread, term } => {
            let m = term_machine(hub, thread, &term)?;
            hub.send_runner_pub(m, HubMsg::TermClose { term })?;
            Value::Null
        }
        AppCall::Changes { thread } => {
            let (t, m) = hub.machine_for_thread(thread).await?;
            hub.runner_request(m, RunnerRequest::Changes { cwd: t.cwd })
                .await?
        }
        AppCall::ListFolders { machine, path } => {
            hub.runner_request(machine, RunnerRequest::Folders { path })
                .await?
        }
        AppCall::ClaudeSessions { machine } => crate::import::sessions(hub, machine).await?,
        AppCall::ImportSession {
            machine,
            session,
            project,
        } => crate::import::session(hub, machine, &session, project).await?,
        AppCall::ListDir { thread, path } => {
            let (t, m) = hub.machine_for_thread(thread).await?;
            hub.runner_request(
                m,
                RunnerRequest::ListDir {
                    path: resolve(&t.cwd, &path),
                },
            )
            .await?
        }
        AppCall::ReadFile { thread, path } => {
            let (t, m) = hub.machine_for_thread(thread).await?;
            hub.runner_request(
                m,
                RunnerRequest::ReadFile {
                    path: resolve(&t.cwd, &path),
                    max_bytes: 4 << 20,
                },
            )
            .await?
        }
        AppCall::Git { thread, repo, args } => {
            let (t, m) = hub.machine_for_thread(thread).await?;
            let cwd = match repo.as_deref() {
                None | Some(".") | Some("") => t.cwd.clone(),
                Some(r) if !r.contains("..") && !r.starts_with('/') => format!("{}/{}", t.cwd, r),
                Some(_) => anyhow::bail!("bad repo name"),
            };
            hub.runner_request(m, RunnerRequest::Git { cwd, args })
                .await?
        }
    })
}

fn term_machine(hub: &HubRef, thread: ThreadId, term: &str) -> Result<MachineId> {
    match hub.terms.lock().unwrap().get(term) {
        Some((t, m)) if *t == thread => Ok(*m),
        _ => Err(anyhow!("that terminal has closed")),
    }
}

fn resolve(cwd: &str, path: &str) -> String {
    if path.starts_with('/') || path.starts_with('~') {
        path.to_string()
    } else if path.is_empty() || path == "." {
        cwd.to_string()
    } else {
        format!("{}/{}", cwd.trim_end_matches('/'), path)
    }
}

fn bail_local(hub: &HubRef, r: Result<Value>) -> Result<Value> {
    match r {
        Ok(v) => {
            let _ = hub.db.kv_delete("backup_error");
            Ok(v)
        }
        Err(e) => Err(e),
    }
}
