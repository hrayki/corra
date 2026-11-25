use anyhow::{Result, anyhow};
use corra_proto::runner::RunnerRequest;
use corra_proto::*;
use serde_json::{Value, json};

use crate::hub::{HubRef, NewProject};

fn same_folder(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

fn project_at(hub: &HubRef, machine: MachineId, cwd: &str) -> Option<Project> {
    hub.db.projects().ok()?.into_iter().find(|p| {
        p.default_machine == Some(machine) && p.path.as_deref().is_some_and(|x| same_folder(x, cwd))
    })
}

pub async fn sessions(hub: &HubRef, machine: MachineId) -> Result<Value> {
    let known: std::collections::HashSet<String> = hub
        .db
        .threads()?
        .into_iter()
        .filter_map(|t| t.session_id)
        .collect();
    let list = hub
        .runner_request(machine, RunnerRequest::ClaudeSessions)
        .await?;
    let out: Vec<Value> = list
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|x| !x["id"].as_str().is_some_and(|id| known.contains(id)))
        .take(300)
        .map(|mut x| {
            let cwd = x["cwd"].as_str().unwrap_or_default().to_string();
            x["project"] = json!(project_at(hub, machine, &cwd).map(|p| p.id));
            x
        })
        .collect();
    Ok(json!(out))
}

fn folder_name(cwd: &str) -> String {
    std::path::Path::new(cwd)
        .file_name()
        .map(|n| {
            n.to_string_lossy().replace(
                |c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'),
                "-",
            )
        })
        .unwrap_or_else(|| "project".into())
}

pub async fn session(
    hub: &HubRef,
    machine: MachineId,
    session: &str,
    project: Option<ProjectId>,
) -> Result<Value> {
    let chosen = match project {
        Some(id) => Some(
            hub.db
                .project(id)?
                .filter(|p| p.default_machine == Some(machine) && p.path.is_some())
                .ok_or_else(|| anyhow!("that project has no folder on this machine"))?,
        ),
        None => None,
    };
    let v = hub
        .runner_request_timeout(
            machine,
            RunnerRequest::ImportSession {
                session: session.to_string(),
                cwd: chosen.as_ref().and_then(|p| p.path.clone()),
            },
            std::time::Duration::from_secs(120),
        )
        .await?;
    let cwd = v["cwd"].as_str().unwrap_or_default().to_string();
    let p = match chosen {
        Some(p) => p,
        None => match project_at(hub, machine, &cwd) {
            Some(p) => p,
            None => hub.create_project(NewProject {
                name: folder_name(&cwd),
                path: Some(cwd.clone()),
                machine: Some(machine),
                provider: Some("existing".into()),
                ..Default::default()
            })?,
        },
    };
    let history: Vec<(i64, Event)> = serde_json::from_value(v["history"].clone())?;
    let now = now_ms();
    let mut events: Vec<StoredEvent> = history
        .into_iter()
        .enumerate()
        .map(|(i, (ts, event))| StoredEvent {
            seq: i as i64 + 1,
            ts: if ts > 0 { ts } else { now },
            event,
        })
        .collect();
    events.push(StoredEvent {
        seq: events.len() as i64 + 1,
        ts: now,
        event: Event::Notice {
            level: NoticeLevel::Info,
            text: "Brought in from Claude Code. The next message carries on the same session."
                .into(),
        },
    });
    let t = Thread {
        id: new_id(),
        project: p.id,
        title: v["title"]
            .as_str()
            .unwrap_or("Claude Code session")
            .to_string(),
        agent: AgentKind::Claude,
        machine: Some(machine),
        session_id: Some(session.to_string()),
        cwd: crate::hub::Hub::project_cwd(&p),
        settings: AgentSettings {
            permission_mode: None,
            ..p.settings.clone()
        },
        status: ThreadStatus::Idle,
        last_seq: events.len() as i64,
        created_at: events.first().map(|e| e.ts).unwrap_or(now),
        updated_at: now,
        pinned_at: None,
        settled_at: None,
        hidden_at: None,
    };
    hub.db.upsert_imported_thread(&t)?;
    hub.db.insert_imported_events(t.id, &events)?;
    if let Some(stored) = hub.db.thread(t.id)? {
        hub.publish_thread(&stored);
    }
    Ok(json!({ "thread": t.id, "project": p.id }))
}
