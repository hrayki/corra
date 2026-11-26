use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use corra_proto::*;

use crate::hub::Hub;

fn idle_ms() -> i64 {
    std::env::var("CORRA_IDLE_MINUTES")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(15)
        * 60
        * 1000
}
const LOST_AFTER_MS: i64 = 90 * 1000;
const WATCHED_IDLE_MS: i64 = 60 * 60 * 1000;

pub fn spawn(hub: Arc<Hub>) {
    if let Ok(machines) = hub.db.machines() {
        for m in machines {
            if m.status == MachineStatus::Connected && !hub.runner_connected(m.id) {
                let _ = hub.db.set_machine_status(m.id, MachineStatus::Offline);
            }
        }
    }
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        let mut rounds: u64 = 0;
        loop {
            tick.tick().await;
            if let Err(e) = sweep(&hub).await {
                tracing::warn!("machine sweep failed: {e:#}");
            }
            if let Err(e) = crate::schedule::continue_limited(&hub, now_ms()).await {
                tracing::warn!("continuing limited threads failed: {e:#}");
            }
            if let Err(e) = crate::schedule::compact_idle(&hub, now_ms()).await {
                tracing::warn!("compacting idle threads failed: {e:#}");
            }
            if rounds % 20 == 1 {
                orphans(&hub).await;
            }
            rounds += 1;
        }
    });
}

pub async fn sweep(hub: &Arc<Hub>) -> Result<()> {
    let now = now_ms();
    let threads = hub.db.threads()?;
    for m in hub.db.machines()? {
        let Some(provider) = hub.providers.get(&m.provider) else {
            continue;
        };
        if !provider.creates_machines() {
            continue;
        }
        let on_machine: Vec<&Thread> = threads.iter().filter(|t| t.machine == Some(m.id)).collect();
        let running = on_machine
            .iter()
            .any(|t| matches!(t.status, ThreadStatus::Running | ThreadStatus::Starting));
        let waiting = on_machine.iter().any(|t| t.status == ThreadStatus::Waiting);
        let watched = {
            let w = hub.watching.lock().unwrap();
            on_machine
                .iter()
                .any(|t| w.get(&t.id).copied().unwrap_or(0) > 0)
        };
        let watched = watched
            || hub
                .browser_watch
                .lock()
                .unwrap()
                .get(&m.id)
                .is_some_and(|n| *n > 0);
        if running {
            hub.touch_machine(m.id);
        }
        let last = hub
            .activity
            .lock()
            .unwrap()
            .get(&m.id)
            .copied()
            .unwrap_or(m.created_at);
        let limit = if watched || waiting {
            WATCHED_IDLE_MS
        } else {
            idle_ms()
        };
        match m.status {
            MachineStatus::Connected if !hub.runner_connected(m.id) => {
                hub.db.set_machine_status(m.id, MachineStatus::Offline)?;
            }
            MachineStatus::Connected if crate::phone::is_machine(hub, m.id) => {
                if !crate::phone::busy(hub) && now - last > limit {
                    tracing::info!(machine = %m.name, "nobody used the phone for a while");
                    if let Err(e) = crate::phone::down(hub).await {
                        tracing::warn!("couldn't stop the phone: {e:#}");
                    }
                }
            }
            MachineStatus::Connected => {
                let since = hub
                    .db
                    .kv_get(&format!("awake_since:{}", m.id))?
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(m.created_at);
                let age = Duration::from_millis((now - since).max(0) as u64);
                let ends_within =
                    |d: Duration| provider.max_lifetime().is_some_and(|max| age + d > max);
                if ends_within(Duration::from_secs(5 * 60)) {
                    tracing::info!(machine = %m.name, "the machine's time is nearly up");
                    rest(hub, &m, Some("This machine reached its provider's time limit, so corra put it to sleep. Send a message to wake it.")).await;
                } else if !running
                    && (now - last > limit || ends_within(Duration::from_secs(30 * 60)))
                {
                    tracing::info!(machine = %m.name, "an idle machine goes to sleep");
                    rest(hub, &m, waiting.then_some("A request waited an hour for an answer, so corra put the machine to sleep. Send a message to wake it.")).await;
                }
            }
            MachineStatus::Offline => {
                if now - m.last_seen.unwrap_or(m.created_at) < LOST_AFTER_MS {
                    continue;
                }
                let gone = match hub.db.machine_provider_ref(m.id)? {
                    Some(r) => match provider.status(hub, &r).await {
                        Ok(crate::providers::ProviderStatus::Stopped) => {
                            let _ = hub.db.set_machine_sleep_ref(m.id, Some(&r));
                            true
                        }
                        Ok(crate::providers::ProviderStatus::Gone) => true,
                        _ => false,
                    },
                    None => true,
                };
                if gone || now - m.last_seen.unwrap_or(m.created_at) > 10 * 60 * 1000 {
                    tracing::warn!(machine = %m.name, "a machine was lost");
                    lost(hub, &m).await;
                }
            }
            MachineStatus::Creating | MachineStatus::Failed => {
                if hub.providers.is_starting(m.id) {
                    continue;
                }
                let since = hub
                    .db
                    .kv_get(&crate::providers::starting_key(m.id))?
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(m.created_at);
                let minutes = if crate::phone::is_machine(hub, m.id) {
                    crate::phone::SETUP_MINUTES
                } else {
                    15
                };
                if now - since > minutes * 60 * 1000 && !hub.runner_connected(m.id) {
                    lost(hub, &m).await;
                }
            }
            MachineStatus::Destroying => {
                retire(hub, m.id, None).await;
            }
            _ => {}
        }
    }
    Ok(())
}

async fn stop_sessions(hub: &Arc<Hub>, machine: MachineId) {
    if !hub.runner_connected(machine) {
        return;
    }
    let Ok(threads) = hub.db.threads_on_machine(machine) else {
        return;
    };
    for t in &threads {
        hub.stop_live(t);
    }
    if !threads.is_empty() {
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

fn settle_threads(hub: &Arc<Hub>, machine: MachineId, notice: Option<&str>) {
    if let Ok(threads) = hub.db.threads_on_machine(machine) {
        for t in threads {
            hub.background_gone(t.id);
            if matches!(
                t.status,
                ThreadStatus::Running | ThreadStatus::Waiting | ThreadStatus::Starting
            ) {
                if let Some(n) = notice {
                    hub.hub_notice(t.id, NoticeLevel::Warning, n);
                }
                let _ = hub.close_open_turn(t.id);
                let _ = hub.set_thread_status(t.id, ThreadStatus::Idle);
            }
        }
    }
}

async fn rest(hub: &Arc<Hub>, m: &Machine, notice: Option<&str>) {
    let Some(provider) = hub.providers.get(&m.provider) else {
        return;
    };
    if !provider.sleeps() {
        retire(hub, m.id, notice).await;
        return;
    }
    let Ok(Some(r)) = hub.db.machine_provider_ref(m.id) else {
        retire(hub, m.id, notice).await;
        return;
    };
    stop_sessions(hub, m.id).await;
    let old = hub.db.machine_sleep_ref(m.id).ok().flatten();
    match provider.sleep(hub, &r).await {
        Ok(new) => {
            let _ = hub.db.set_machine_sleep_ref(m.id, Some(&new));
            let _ = hub.db.set_machine_status(m.id, MachineStatus::Asleep);
            let _ = hub.db.kv_delete(&format!("awake_since:{}", m.id));
            hub.publish_machine(m.id);
            settle_threads(hub, m.id, notice);
            hub.activity.lock().unwrap().remove(&m.id);
            if let Some(old) = old.filter(|o| *o != new)
                && let Err(e) = provider.forget(hub, &old).await
            {
                tracing::warn!(machine = %m.name, "couldn't delete the previous sleep: {e:#}");
            }
        }
        Err(e) => {
            tracing::warn!(machine = %m.name, "couldn't put the machine to sleep, will try again: {e:#}")
        }
    }
}

async fn lost(hub: &Arc<Hub>, m: &Machine) {
    let sleep_ref = hub.db.machine_sleep_ref(m.id).ok().flatten();
    if let Some(s) = &sleep_ref {
        if let (Ok(Some(r)), Some(provider)) = (
            hub.db.machine_provider_ref(m.id),
            hub.providers.get(&m.provider),
        ) && r != *s
            && let Err(e) = provider.destroy(hub, &r).await
        {
            tracing::warn!(machine = %m.name, "couldn't delete what was left of a lost machine: {e:#}");
        }
        let _ = hub.db.set_machine_status(m.id, MachineStatus::Asleep);
        hub.publish_machine(m.id);
        settle_threads(
            hub,
            m.id,
            Some(
                "The machine running this thread stopped. Send a message to wake it as it was when it last slept.",
            ),
        );
        hub.activity.lock().unwrap().remove(&m.id);
        return;
    }
    retire(
        hub,
        m.id,
        Some(
            "The machine running this thread stopped. Send a message to carry on on a new machine.",
        ),
    )
    .await;
}

async fn retire(hub: &Arc<Hub>, machine: MachineId, notice: Option<&str>) {
    stop_sessions(hub, machine).await;
    if let Err(e) = hub.providers.destroy_machine(hub, machine).await {
        tracing::warn!(%machine, "couldn't destroy a machine, will try again: {e:#}");
        return;
    }
    if let (Ok(Some(m)), Ok(Some(s))) = (hub.db.machine(machine), hub.db.machine_sleep_ref(machine))
    {
        if let Some(provider) = hub.providers.get(&m.provider)
            && let Err(e) = provider.forget(hub, &s).await
        {
            tracing::warn!(%machine, "couldn't delete the machine's sleep: {e:#}");
            crate::leftovers::add(
                hub,
                &format!("{}'s saved disk", m.name),
                &m.provider,
                LeftoverThing::Image,
                &s,
                &format!("{e:#}"),
            );
        }
        let _ = hub.db.set_machine_sleep_ref(machine, None);
    }
    let _ = hub.secrets.delete(&format!("machine_token.{machine}"));
    let _ = hub.db.kv_delete(&format!("awake_since:{machine}"));
    let _ = hub.db.kv_delete(&crate::providers::starting_key(machine));
    settle_threads(hub, machine, notice);
    hub.activity.lock().unwrap().remove(&machine);
}

pub async fn reset_project(hub: &Arc<Hub>, project: ProjectId) -> Result<()> {
    let machines: Vec<Machine> = hub
        .db
        .machines()?
        .into_iter()
        .filter(|m| m.project == Some(project) && m.status != MachineStatus::Destroyed)
        .collect();
    for m in machines {
        let provider = hub
            .providers
            .get(&m.provider)
            .ok_or_else(|| anyhow!("unknown provider {}", m.provider))?;
        if !provider.creates_machines() {
            continue;
        }
        stop_sessions(hub, m.id).await;
        if let Some(r) = hub.db.machine_provider_ref(m.id)?
            && m.status != MachineStatus::Asleep
        {
            provider.destroy(hub, &r).await?;
        }
        if let Some(s) = hub.db.machine_sleep_ref(m.id)? {
            provider.forget(hub, &s).await?;
        }
        hub.db.set_machine_sleep_ref(m.id, None)?;
        hub.db.set_machine_status(m.id, MachineStatus::Destroyed)?;
        let _ = hub.secrets.delete(&format!("machine_token.{}", m.id));
        hub.publish_machine(m.id);
        settle_threads(hub, m.id, None);
    }
    Ok(())
}

pub async fn orphans(hub: &Arc<Hub>) {
    for name in hub.providers.names() {
        let Some(provider) = hub.providers.get(name) else {
            continue;
        };
        if !provider.creates_machines() {
            continue;
        }
        let running = match provider.running(hub).await {
            Ok(r) => r,
            Err(_) => continue,
        };
        let Ok(machines) = hub.db.machines() else {
            return;
        };
        let mine: Vec<&Machine> = machines.iter().filter(|m| m.provider == name).collect();
        if mine.iter().any(|m| m.status == MachineStatus::Creating) {
            continue;
        }
        let mut known: HashSet<String> = HashSet::new();
        for m in &mine {
            if m.status == MachineStatus::Destroyed {
                continue;
            }
            if let Ok(Some(r)) = hub.db.machine_provider_ref(m.id) {
                known.insert(r);
            }
            if let Ok(Some(s)) = hub.db.machine_sleep_ref(m.id) {
                known.insert(s);
            }
        }
        for r in running {
            if known.contains(&r) {
                continue;
            }
            tracing::warn!(provider = name, machine = %r, "stopping a machine corra had lost track of");
            if let Err(e) = provider.destroy(hub, &r).await {
                tracing::warn!(provider = name, machine = %r, "couldn't stop it: {e:#}");
            }
        }
    }
}

pub fn retire_moved(hub: &Arc<Hub>, project: ProjectId) {
    let Ok(Some(p)) = hub.db.project(project) else {
        return;
    };
    let current = hub.providers.kind_name(&p.provider, p.kind.as_deref());
    let old: Vec<Machine> = hub
        .db
        .machines()
        .unwrap_or_default()
        .into_iter()
        .filter(|m| {
            m.project == Some(project)
                && !matches!(
                    m.status,
                    MachineStatus::Destroyed | MachineStatus::Destroying
                )
                && hub
                    .providers
                    .get(&m.provider)
                    .is_some_and(|x| x.creates_machines())
                && (m.provider != p.provider
                    || hub.providers.kind_name(&m.provider, m.kind.as_deref()) != current)
        })
        .collect();
    for m in old {
        let hub = hub.clone();
        let p = p.clone();
        tokio::spawn(async move {
            if let Err(e) = move_off(&hub, &p, &m).await {
                tracing::warn!(machine = %m.name, "couldn't move the threads off an old machine: {e:#}");
                for (thing, r) in [
                    (
                        LeftoverThing::Machine,
                        hub.db.machine_provider_ref(m.id).ok().flatten(),
                    ),
                    (
                        LeftoverThing::Image,
                        hub.db.machine_sleep_ref(m.id).ok().flatten(),
                    ),
                ] {
                    if let Some(r) = r {
                        crate::leftovers::add(
                            &hub,
                            &format!(
                                "{}'s old machine, kept because its threads' work couldn't be saved",
                                p.name
                            ),
                            &m.provider,
                            thing,
                            &r,
                            &format!("{e:#}"),
                        );
                    }
                }
            }
        });
    }
}

async fn move_off(hub: &Arc<Hub>, p: &Project, m: &Machine) -> Result<()> {
    let provider = hub
        .providers
        .get(&m.provider)
        .ok_or_else(|| anyhow::anyhow!("unknown provider {}", m.provider))?;
    let threads = hub.db.threads_on_machine(m.id)?;
    if !threads.is_empty() && !hub.runner_connected(m.id) {
        let sleep_ref = hub.db.machine_sleep_ref(m.id)?.ok_or_else(|| {
            anyhow::anyhow!("the machine isn't running and has no sleep to wake from")
        })?;
        let req = crate::providers::CreateRequest {
            name: m.name.clone(),
            machine: m.id,
            kind: hub.providers.kind_name(&m.provider, m.kind.as_deref()),
            size: crate::providers::Size::parse(&m.size),
            env: hub.providers.machine_env(hub, m.id)?,
        };
        hub.db.set_machine_status(m.id, MachineStatus::Creating)?;
        hub.publish_machine(m.id);
        let r = provider.wake(hub, &sleep_ref, req).await?;
        hub.db.set_machine_provider_ref(m.id, &r)?;
        crate::providers::await_runner(hub, provider.as_ref(), m.id, &r)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
    }
    for t in threads {
        if hub.runner_connected(m.id) {
            hub.stop_live(&t);
            hub.runner_request_timeout(
                m.id,
                corra_proto::runner::RunnerRequest::SaveWork {
                    cwd: t.cwd.clone(),
                    thread: t.id,
                },
                Duration::from_secs(120),
            )
            .await?;
        }
        if let Ok(Some(mut t)) = hub.db.thread(t.id) {
            let from = t.cwd.clone();
            t.machine = None;
            if matches!(
                t.status,
                ThreadStatus::Running | ThreadStatus::Waiting | ThreadStatus::Starting
            ) {
                t.status = ThreadStatus::Idle;
            }
            t.cwd = format!("~/projects/{}", p.name);
            t.updated_at = now_ms();
            hub.db.reset_runner_seq(t.id)?;
            hub.db.update_thread(&t)?;
            hub.publish_thread(&t);
            if from != t.cwd {
                hub.set_thread_kv(t.id, "moved_from", &from);
            }
        }
    }
    retire(hub, m.id, None).await;
    Ok(())
}
