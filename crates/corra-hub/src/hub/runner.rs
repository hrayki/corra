use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use corra_proto::app::HubToApp;
use corra_proto::runner::{HubMsg, RunnerRequest};
use corra_proto::*;
use serde_json::Value;
use tokio::sync::oneshot;

use super::{Hub, Outbound};

impl Hub {
    pub fn runner_connected(&self, machine: MachineId) -> bool {
        self.runners.lock().unwrap().contains_key(&machine)
    }

    pub fn send_runner_pub(&self, machine: MachineId, msg: HubMsg) -> Result<()> {
        self.send_runner(machine, msg)
    }

    pub(crate) fn send_runner(&self, machine: MachineId, msg: HubMsg) -> Result<()> {
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
        let mut drain = false;
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
                    let (settled, outcome) = self.settled_status(thread)?;
                    drain = settled == ThreadStatus::Idle
                        && matches!(t.status, ThreadStatus::Running | ThreadStatus::Waiting)
                        && outcome != Some(TurnOutcome::Failed);
                    if t.status != ThreadStatus::Starting
                        || !matches!(settled, ThreadStatus::Idle | ThreadStatus::Error)
                    {
                        t.status = settled;
                    }
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
        if let Event::TurnCompleted { turn, .. } = &event {
            self.cancel_cards_of(thread, turn)?;
        }
        if drain {
            self.send_next_queued(thread);
        }
        match &event {
            Event::TurnStarted { input, .. } => {
                crate::schedule::cancel_continue(self, thread);
                if !matches!(&input[..], [InputPart::Text { text }] if text == crate::schedule::COMPACT)
                {
                    self.quiet_turns.lock().unwrap().remove(&thread);
                    if let Some(m) = self.machine_of(thread) {
                        self.compact_woke.lock().unwrap().remove(&m);
                    }
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
                outcome: TurnOutcome::Interrupted,
                ..
            } => {
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

    fn settled_status(&self, thread: ThreadId) -> Result<(ThreadStatus, Option<TurnOutcome>)> {
        let outcome = self.db.last_outcome(thread)?;
        let status = if !self.db.open_requests(thread)?.is_empty() {
            ThreadStatus::Waiting
        } else if self.db.open_turn(thread)?.is_some() {
            ThreadStatus::Running
        } else if outcome == Some(TurnOutcome::Failed) {
            ThreadStatus::Error
        } else {
            ThreadStatus::Idle
        };
        Ok((status, outcome))
    }

    fn cancel_cards_of(&self, thread: ThreadId, turn: &str) -> Result<()> {
        for e in self.db.open_requests(thread)? {
            let Event::Request {
                id,
                turn: of,
                request: AgentRequest::HubAction { action, .. },
            } = e.event
            else {
                continue;
            };
            if of != turn || action == "git.push" {
                continue;
            }
            let secret = self.secret_requests.lock().unwrap().remove(&id);
            let ask = self.hub_requests.lock().unwrap().remove(&id);
            if secret.is_none() && ask.is_none() {
                continue;
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
            if let Some(tx) = secret {
                let _ = tx.send(None);
            }
            if let Some(tx) = ask {
                let _ = tx.send(RequestResponse::Cancelled);
            }
        }
        Ok(())
    }

    fn note_context(&self, thread: ThreadId) {
        let Some((tokens, at)) = self.context_seen.lock().unwrap().remove(&thread) else {
            return;
        };
        crate::schedule::note_cache_use(self, thread, tokens, at);
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
        if request.starts_with("signin-") {
            if !self.request_open(thread, &request)? {
                bail!("that sign-in is no longer waiting");
            }
            let t = self
                .db
                .thread(thread)?
                .ok_or_else(|| anyhow!("no such thread"))?;
            let m = t.machine.ok_or_else(|| anyhow!("thread has no machine"))?;
            self.send_runner(
                m,
                HubMsg::SignIn {
                    thread,
                    request: request.clone(),
                    code: value,
                },
            )?;
            self.append(
                thread,
                None,
                now_ms(),
                Event::RequestResolved {
                    id: request,
                    response: RequestResponse::Allow { always: false },
                },
            )?;
            return Ok(());
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

    fn request_open(&self, thread: ThreadId, request: &str) -> Result<bool> {
        Ok(self
            .db
            .open_requests(thread)?
            .iter()
            .any(|e| matches!(&e.event, Event::Request { id, .. } if id == request)))
    }

    fn open_request_ids(&self, thread: ThreadId) -> Result<Vec<String>> {
        Ok(self
            .db
            .open_requests(thread)?
            .into_iter()
            .filter_map(|e| match e.event {
                Event::Request { id, .. } => Some(id),
                _ => None,
            })
            .collect())
    }

    pub fn close_open_turn(&self, thread: ThreadId) -> Result<bool> {
        let open = self.db.open_turn(thread)?;
        for id in self.open_request_ids(thread)? {
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
            let ids = self.open_request_ids(t.id)?;
            for id in ids.into_iter().filter(|id| id.starts_with("hub-")) {
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

    pub fn runner_status(&self, thread: ThreadId, status: ThreadStatus) -> Result<()> {
        let mut status = status;
        if matches!(status, ThreadStatus::Idle | ThreadStatus::Error) {
            if self.close_stray_turn(thread)? {
                self.hub_notice(
                    thread,
                    NoticeLevel::Warning,
                    "The agent stopped working on this turn without ending it, so corra ended it.",
                );
            }
            if self
                .db
                .open_requests(thread)?
                .iter()
                .any(|e| matches!(&e.event, Event::Request { id, .. } if id.starts_with("hub-")))
            {
                status = ThreadStatus::Waiting;
            }
        }
        self.set_thread_status(thread, status)
    }

    fn close_stray_turn(&self, thread: ThreadId) -> Result<bool> {
        for e in self.db.open_requests(thread)? {
            if let Event::Request { id, .. } = e.event
                && !id.starts_with("hub-")
            {
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
        }
        let Some(turn) = self.db.open_turn(thread)? else {
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

    pub fn interrupt(&self, thread: ThreadId) -> Result<Vec<Queued>> {
        let t = self
            .db
            .thread(thread)?
            .ok_or_else(|| anyhow!("no such thread"))?;
        let m = t.machine.ok_or_else(|| anyhow!("thread has no machine"))?;
        self.send_runner(m, HubMsg::Interrupt { thread })?;
        let unsent = self.queued(thread);
        if !unsent.is_empty() {
            self.set_queued(thread, &[]);
        }
        Ok(unsent)
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
        let waiting = self.request_open(thread, &request)?;
        {
            let mut answered = self.answered.lock().unwrap();
            answered.retain(|_, at| at.elapsed() < Duration::from_secs(60));
            if !waiting || answered.contains_key(&request) {
                bail!("That request is no longer waiting.");
            }
            answered.insert(request.clone(), Instant::now());
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

    pub fn runner_supports_prompt(&self, machine: MachineId) -> bool {
        self.runners
            .lock()
            .unwrap()
            .get(&machine)
            .is_some_and(|c| c.prompt)
    }

    pub async fn stop_session(&self, machine: MachineId, thread: ThreadId) {
        let supports_prompt = {
            let mut runners = self.runners.lock().unwrap();
            let Some(c) = runners.get_mut(&machine) else {
                return;
            };
            c.live.remove(&thread);
            c.prompt
        };
        if supports_prompt {
            let _ = self
                .runner_request_timeout(
                    machine,
                    RunnerRequest::Stop { thread },
                    Duration::from_secs(12),
                )
                .await;
        } else {
            let _ = self.send_runner(machine, HubMsg::Stop { thread });
        }
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
}
