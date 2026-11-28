use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use corra_proto::app::HubToApp;
use corra_proto::runner::{HubMsg, RunnerRequest};
use corra_proto::*;

use super::{Delivery, History, Hub, Outbound, title_from};

const STRAY_AFTER_MS: i64 = 60_000;

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

    pub fn tail_start(&self, thread: ThreadId, turns: i64) -> Result<(i64, bool)> {
        let starts = self.db.turn_starts_before(thread, i64::MAX, turns + 1)?;
        Ok(match starts.get(turns as usize) {
            Some(_) => (starts[turns as usize - 1], true),
            None => (0, false),
        })
    }

    pub fn follower(&self, thread: ThreadId) -> Option<ThreadId> {
        self.db.follower(thread).ok().flatten()
    }

    pub fn set_follower(&self, thread: ThreadId, follower: Option<ThreadId>) {
        let _ = self.db.set_follower(thread, follower);
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
            Ok(Some(f)) if f.hidden_at.is_none() && !self.hands_off(f.agent) => {}
            _ => {
                self.set_follower(thread, None);
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
            };
            if let Err(e) = crate::ws::exec_call(&hub, call).await {
                tracing::warn!(%thread, %follower, "couldn't tell the follower: {e:#}");
            }
        });
    }

    pub fn delivering(&self, thread: ThreadId) -> bool {
        self.delivering.lock().unwrap().contains_key(&thread)
            || self.deliveries().iter().any(|(_, d)| d.thread == thread)
    }

    pub fn deliveries(&self) -> Vec<(String, Delivery)> {
        self.db
            .kv_keys("delivery:")
            .unwrap_or_default()
            .into_iter()
            .filter_map(|k| {
                let d = serde_json::from_str(&self.db.kv_get(&k).ok()??).ok()?;
                Some((k.trim_start_matches("delivery:").to_string(), d))
            })
            .collect()
    }

    pub fn took(self: &Arc<Self>, machine: MachineId, delivery: String, taken: bool) {
        let key = format!("delivery:{delivery}");
        let d = self
            .db
            .kv_get(&key)
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str::<Delivery>(&s).ok());
        if taken {
            let _ = self.db.kv_delete(&key);
            if let Some(d) = d {
                let _ = self
                    .db
                    .kv_delete(&format!("thread:{}:moved_from", d.thread));
                let _ = self.db.kv_delete(&format!("thread:{}:resume_at", d.thread));
            }
            return;
        }
        let Some(d) = d else {
            return;
        };
        if let Some(c) = self.runners.lock().unwrap().get_mut(&machine) {
            c.live.remove(&d.thread);
        }
        self.redeliver(delivery, d);
    }

    pub fn redeliver_to(self: &Arc<Self>, machine: MachineId) {
        for (id, d) in self.deliveries() {
            if self.machine_of(d.thread) == Some(machine) {
                self.redeliver(id, d);
            }
        }
    }

    pub fn redeliver_unconnected(self: &Arc<Self>) {
        for (id, d) in self.deliveries() {
            if !self
                .machine_of(d.thread)
                .is_some_and(|m| self.runner_connected(m))
            {
                self.redeliver(id, d);
            }
        }
    }

    pub(crate) fn machine_of(&self, thread: ThreadId) -> Option<MachineId> {
        self.db
            .thread(thread)
            .ok()
            .flatten()
            .and_then(|t| t.machine)
    }

    fn redeliver(self: &Arc<Self>, id: String, d: Delivery) {
        let hub = self.clone();
        tokio::spawn(async move {
            let thread = d.thread;
            if let Err(e) = hub.deliver_tracked(id, d).await {
                hub.hub_notice(thread, NoticeLevel::Error, format!("Couldn't send: {e:#}"));
                let _ = hub.set_thread_status(thread, ThreadStatus::Error);
            }
        });
    }

    pub fn forget_deliveries_before(&self, at: i64) {
        for (id, d) in self.deliveries() {
            if d.at < at {
                let _ = self.db.kv_delete(&format!("delivery:{id}"));
            }
        }
    }

    pub fn settle_strays(&self, now: i64) -> Result<()> {
        for t in self.db.threads()? {
            if self.delivering(t.id) || now - t.updated_at < STRAY_AFTER_MS {
                continue;
            }
            match t.status {
                ThreadStatus::Running | ThreadStatus::Waiting | ThreadStatus::Starting => {
                    let live = t.machine.and_then(|m| {
                        self.runners
                            .lock()
                            .unwrap()
                            .get(&m)
                            .map(|c| c.live.contains(&t.id))
                    });
                    let gone = match live {
                        Some(live) => !live,
                        None => t.status == ThreadStatus::Starting,
                    };
                    if gone {
                        self.runner_status(t.id, ThreadStatus::Idle)?;
                    }
                }
                ThreadStatus::Idle
                    if t.hidden_at.is_none()
                        && !self.queued(t.id).is_empty()
                        && self.db.last_outcome(t.id)? != Some(TurnOutcome::Failed) =>
                {
                    self.send_next_queued(t.id);
                }
                _ => {}
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

    pub(crate) fn set_queued(&self, thread: ThreadId, items: &[Queued]) {
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
        self.db.first_send(id, now_ms()).unwrap_or(true)
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

    pub(crate) fn send_next_queued(&self, thread: ThreadId) {
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
        self.send_once(thread, input, settings, queue, None)
            .await
            .map(|_| ())
    }

    pub async fn send_once(
        self: &Arc<Self>,
        thread: ThreadId,
        input: Vec<InputPart>,
        settings: Option<AgentSettings>,
        queue: bool,
        client_id: Option<String>,
    ) -> Result<bool> {
        if let Some(id) = &client_id
            && !self.first_send(id)
        {
            return Ok(self.queued(thread).iter().any(|q| q.id == *id));
        }
        if !matches!(&input[..], [InputPart::Text { text }] if text == crate::schedule::COMPACT)
            && let Some(m) = self.machine_of(thread)
        {
            self.compact_woke.lock().unwrap().remove(&m);
        }
        if queue && self.busy(thread) {
            self.enqueue(thread, input, client_id);
            return Ok(true);
        }
        let hub = self.clone();
        tokio::spawn(async move {
            if let Err(e) = hub.deliver(thread, input, settings).await {
                hub.hub_notice(thread, NoticeLevel::Error, format!("Couldn't send: {e:#}"));
                let _ = hub.set_thread_status(thread, ThreadStatus::Error);
                hub.tell_follower_text(thread, &format!("couldn't take a message: {e:#}"));
                if let Some(client_id) = client_id {
                    hub.publish(Outbound::Thread(
                        thread,
                        HubToApp::NotSent { thread, client_id },
                    ));
                }
            }
        });
        Ok(false)
    }

    async fn deliver(
        self: &Arc<Self>,
        thread: ThreadId,
        input: Vec<InputPart>,
        settings: Option<AgentSettings>,
    ) -> Result<()> {
        let id = new_id().to_string();
        let d = Delivery {
            thread,
            input,
            settings,
            at: now_ms(),
        };
        if let Ok(s) = serde_json::to_string(&d) {
            self.db.kv_set(&format!("delivery:{id}"), &s)?;
        }
        self.deliver_tracked(id, d).await
    }

    async fn deliver_tracked(self: &Arc<Self>, id: String, d: Delivery) -> Result<()> {
        let thread = d.thread;
        *self.delivering.lock().unwrap().entry(thread).or_default() += 1;
        let r = self
            .deliver_now(thread, d.input, d.settings, Some(id.clone()))
            .await;
        if r.is_err() {
            let _ = self.db.kv_delete(&format!("delivery:{id}"));
        }
        let mut d = self.delivering.lock().unwrap();
        if let Some(n) = d.get_mut(&thread) {
            *n -= 1;
            if *n == 0 {
                d.remove(&thread);
            }
        }
        r
    }

    async fn deliver_now(
        self: &Arc<Self>,
        thread: ThreadId,
        input: Vec<InputPart>,
        settings: Option<AgentSettings>,
        delivery: Option<String>,
    ) -> Result<()> {
        let mut t = self
            .db
            .thread(thread)?
            .ok_or_else(|| anyhow!("no such thread"))?;
        let mut dirty = false;
        if t.settled_at.is_some() {
            t.settled_at = None;
            dirty = true;
        }
        if let Some(s) = &settings {
            t.settings.merge(s);
            dirty = true;
        }
        if dirty {
            self.db.update_thread(&t)?;
            self.publish_thread(&t);
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
        let mark = crate::follow::spec_mark(&tools, &agent_env);
        let (takes, prompt) = self
            .runners
            .lock()
            .unwrap()
            .get(&machine)
            .map(|c| (c.takes, c.prompt))
            .unwrap_or((false, false));
        if !takes && let Some(d) = &delivery {
            let _ = self.db.kv_delete(&format!("delivery:{d}"));
        }
        let delivery = delivery.filter(|_| takes);
        if prompt {
            let moved_from = if takes {
                self.thread_kv(thread, "moved_from")
            } else {
                self.take_thread_kv(thread, "moved_from")
            };
            let resume_at = if takes {
                self.thread_kv(thread, "resume_at")
            } else {
                self.take_thread_kv(thread, "resume_at")
            };
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
                moved_from,
                fork_of: self
                    .thread_kv(thread, "fork_of")
                    .and_then(|s| serde_json::from_str(&s).ok()),
                resume_at,
            };
            self.send_runner(
                machine,
                HubMsg::Prompt {
                    spec: Box::new(spec),
                    input,
                    settings,
                    delivery,
                },
            )?;
            self.set_thread_kv(thread, "spec", &mark);
            if let Some(c) = self.runners.lock().unwrap().get_mut(&machine) {
                c.live.insert(thread);
            }
            return Ok(());
        }
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
                    delivery,
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
            };
            self.send_runner(
                machine,
                HubMsg::Start {
                    spec: Box::new(spec),
                    input: Some(input),
                    delivery,
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

    pub(crate) async fn ensure_machine(self: &Arc<Self>, t: &mut Thread) -> Result<MachineId> {
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

    pub(crate) async fn prepare_paired(
        &self,
        project: ProjectId,
        machine: MachineId,
    ) -> Result<()> {
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

    pub fn mark_started_by(&self, thread: ThreadId, parent: ThreadId) {
        let _ = self.db.set_started_by(thread, parent);
        if let Ok(Some(t)) = self.db.thread(thread) {
            self.publish_thread(&t);
        }
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

    pub fn auto_settle(&self, now: i64) -> Result<usize> {
        let Some(days) = self.auto_settle_days() else {
            return Ok(0);
        };
        let cutoff = days * 86_400_000;
        let mut n = 0;
        for mut t in self.db.threads()? {
            if t.hidden_at.is_some()
                || t.settled_at.is_some()
                || t.pinned_at.is_some()
                || t.status != ThreadStatus::Idle
                || self.delivering(t.id)
                || !self.queued(t.id).is_empty()
            {
                continue;
            }
            if now - t.updated_at >= cutoff {
                t.settled_at = Some(now);
                self.db.update_thread(&t)?;
                self.publish_thread(&t);
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
            if self.runner_supports_prompt(m) {
                self.stop_session(m, thread).await;
            } else {
                self.stop_live(&t);
                tokio::time::sleep(Duration::from_millis(1500)).await;
            }
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
            started_by: None,
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
        self.stop_session(machine, thread).await;
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

const SLIM_OUTPUT_BYTES: usize = 1024;
const SLIM_INPUT_BYTES: usize = 2048;
const SLIM_FIELD_BYTES: usize = 512;
const SUMMARY_INPUT_KEYS: &[&str] = &[
    "file_path",
    "path",
    "Path",
    "AbsolutePath",
    "TargetFile",
    "command",
    "CommandLine",
    "pattern",
    "query",
    "Query",
    "description",
    "title",
    "url",
    "Url",
    "glob",
    "name",
    "subagent_type",
];

fn keep_tool_verbatim(name: &str) -> bool {
    matches!(name, "Agent" | "Task" | "threads.start")
        || name.ends_with("__threads_start")
        || name.ends_with(".threads.start")
}

fn truncate_utf8(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

fn slim_tool_input(input: &mut serde_json::Value) -> bool {
    let serialized_len = serde_json::to_string(input).map(|s| s.len()).unwrap_or(0);
    if serialized_len <= SLIM_INPUT_BYTES {
        return false;
    }
    match input {
        serde_json::Value::Object(map) => {
            let mut slim = serde_json::Map::new();
            for &key in SUMMARY_INPUT_KEYS {
                if let Some(v) = map.get(key) {
                    let kept = match v {
                        serde_json::Value::String(s) => {
                            serde_json::Value::String(truncate_utf8(s, SLIM_FIELD_BYTES))
                        }
                        other => other.clone(),
                    };
                    slim.insert(key.to_string(), kept);
                }
            }
            *map = slim;
            true
        }
        _ => {
            *input = serde_json::Value::Null;
            true
        }
    }
}

pub fn slim_events(events: &mut [StoredEvent]) {
    for ev in events {
        match &mut ev.event {
            Event::ItemStarted { item, .. } => {
                if let ItemBody::ToolCall { name, input, .. } = &mut item.body
                    && !keep_tool_verbatim(name)
                {
                    slim_tool_input(input);
                }
            }
            Event::ItemCompleted { item, .. } => {
                if let ItemBody::ToolCall {
                    name,
                    input,
                    output,
                    ..
                } = &mut item.body
                    && !keep_tool_verbatim(name)
                {
                    let trimmed_input = slim_tool_input(input);
                    let heavy_output = output
                        .as_ref()
                        .is_some_and(|o| o.text.len() > SLIM_OUTPUT_BYTES || !o.images.is_empty());
                    if trimmed_input || heavy_output {
                        match output {
                            Some(out) => {
                                if heavy_output {
                                    out.text.clear();
                                    out.images.clear();
                                }
                                out.deferred = Some(true);
                            }
                            None => {
                                *output = Some(ToolOutput {
                                    text: String::new(),
                                    is_error: false,
                                    images: vec![],
                                    deferred: Some(true),
                                });
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}
