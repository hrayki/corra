use anyhow::{Result, anyhow, bail};
use corra_proto::app::AppCall;
use corra_proto::*;
use serde_json::{Value, json};

use crate::hub::HubRef;
use crate::ws::exec_call;

fn tool(action: &str, description: &str, props: Value, required: &[&str]) -> ToolDef {
    ToolDef {
        action: action.into(),
        description: description.into(),
        input_schema: json!({ "type": "object", "properties": props, "required": required }),
    }
}

fn kinds_line(providers: &[ProviderInfo]) -> String {
    let each: Vec<String> = providers
        .iter()
        .filter(|p| !p.kinds.is_empty())
        .map(|p| {
            let kinds: Vec<String> = p.kinds.iter().map(|k| k.name.clone()).collect();
            format!("{} has {}", p.name, kinds.join(" and "))
        })
        .collect();
    format!(
        "The kind of machine its provider makes: {}. Defaults to the first.",
        each.join("; ")
    )
}

pub fn defs(hub: &HubRef) -> Vec<ToolDef> {
    let providers = hub.providers.info(hub);
    let makers: Vec<&str> = providers
        .iter()
        .filter(|p| p.creates_machines)
        .map(|p| p.name.as_str())
        .collect();
    let all_providers: Vec<&str> = providers.iter().map(|p| p.name.as_str()).collect();
    let kind = json!({ "type": "string", "description": kinds_line(&providers) });
    let thread = json!({ "type": "string", "description": "A thread's id from threads.list, or the last characters of it." });
    let project_settings = |mut props: Value| {
        let more = json!({
            "size": { "type": "string", "description": "Cores x GB of memory, like 4x8." },
            "repos": { "type": "array", "items": { "type": "string" }, "description": "GitHub repos as owner/name. Replaces the list." },
            "setup": { "type": "string", "description": "The setup from the setup repo's corra/setups/ to run on its machine; empty for none." },
            "env": { "type": "array", "items": { "type": "string" }, "description": "Names of the env vars from Settings it gets. Replaces the list." },
            "allow_main": { "type": "boolean", "description": "Whether its machines push to the default branch and tags, and merge, without asking." }
        });
        if let (Some(p), Value::Object(m)) = (props.as_object_mut(), more) {
            p.extend(m);
        }
        props
    };
    let all = vec![
        tool(
            "threads.list",
            "List corra threads on every machine: id, title, project, machine, status, and the model, effort and permission mode each runs with.",
            json!({
                "project": { "type": "string", "description": "Only this project's threads." },
                "status": { "type": "string", "enum": ["idle", "starting", "running", "waiting", "error"] }
            }),
            &[],
        ),
        tool(
            "threads.read",
            "Read the end of a thread: its model, effort and permission mode, what the user asked, what the agent answered and which tools it ran.",
            json!({ "thread": thread, "last": { "type": "integer", "description": "How many turns to read. Defaults to 3." } }),
            &["thread"],
        ),
        tool(
            "threads.start",
            "Start a new thread with its own agent working on a prompt. Returns the new thread's id. You follow the new thread, as threads.follow describes, unless follow is false (defaults to false when your own thread can't receive automated messages).",
            json!({
                "project": { "type": "string", "description": "The project's name." },
                "prompt": { "type": "string", "description": "The task for the new agent." },
                "agent": { "type": "string", "enum": ["claude", "antigravity"], "description": "Which agent runs the new thread. Defaults to this thread's agent." },
                "on": { "type": "string", "description": "A machine's name. Leave out for the project's usual machine." },
                "model": { "type": "string", "description": "Optional model id." },
                "effort": { "type": "string", "enum": ["low", "medium", "high", "xhigh", "max"], "description": "Optional effort." },
                "follow": { "type": "boolean", "description": "False to start it without following it." }
            }),
            &["project", "prompt"],
        ),
        tool(
            "threads.send",
            "Send a message to a thread's agent. If it's mid-turn, it reads the message at its next step; set queue to hold the message until that turn ends.",
            json!({ "thread": thread, "message": { "type": "string" }, "queue": { "type": "boolean", "description": "True to hold the message until the agent's turn ends." } }),
            &["thread", "message"],
        ),
        tool(
            "threads.stop",
            "Stop a thread's agent mid-turn.",
            json!({ "thread": thread }),
            &["thread"],
        ),
        tool(
            "threads.follow",
            "Follow a thread, or stop following it. While you follow a thread you get a message when it finishes a turn, stops with an error or waits on the user. A thread has one follower, so following it takes it over from any other.",
            json!({ "thread": thread, "follow": { "type": "boolean", "description": "False to stop following. Defaults to true." } }),
            &["thread"],
        ),
        tool(
            "threads.move",
            "Move an idle thread to another machine, with its conversation and uncommitted work.",
            json!({ "thread": thread, "to": { "type": "string", "description": "A machine's name. Leave out for the project's own remote machine." } }),
            &["thread"],
        ),
        tool(
            "projects.list",
            "List corra projects: name, where they run, size, repos and settings.",
            json!({}),
            &[],
        ),
        tool(
            "projects.create",
            "Create a project that runs on remote machines.",
            project_settings(json!({
                "name": { "type": "string" },
                "provider": { "type": "string", "enum": makers, "description": "Who makes its machines." },
                "kind": kind
            })),
            &["name"],
        ),
        tool(
            "projects.update",
            "Change a project's provider, kind of machine, machine size, repos, setup, env vars, permission mode or what it may push without asking.",
            project_settings(json!({
                "project": { "type": "string", "description": "The project's name." },
                "provider": { "type": "string", "enum": all_providers },
                "kind": kind
            })),
            &["project"],
        ),
        tool(
            "machines.list",
            "List corra's machines: name, provider, status, project and size.",
            json!({}),
            &[],
        ),
        tool(
            "machines.destroy",
            "Destroy a machine corra created. Its threads continue on a new machine at their next message.",
            json!({ "machine": { "type": "string", "description": "The machine's name." } }),
            &["machine"],
        ),
        tool(
            "credentials.list",
            "List the names of the credentials the hub holds. Values can't be read.",
            json!({}),
            &[],
        ),
        tool(
            "credentials.set",
            "Store a credential in the hub, replacing any with that name.",
            json!({ "name": { "type": "string", "description": "Like github.token or modal.token_id." }, "value": { "type": "string" } }),
            &["name", "value"],
        ),
        tool(
            "credentials.remove",
            "Remove a credential from the hub.",
            json!({ "name": { "type": "string" } }),
            &["name"],
        ),
    ];
    all
}

const MANAGE_ACTIONS: &[&str] = &[
    "threads.list",
    "threads.read",
    "threads.start",
    "threads.send",
    "threads.stop",
    "threads.follow",
    "threads.move",
    "projects.list",
    "projects.create",
    "projects.update",
    "machines.list",
    "machines.destroy",
    "credentials.list",
    "credentials.set",
    "credentials.remove",
];

pub fn is_manage(action: &str) -> bool {
    MANAGE_ACTIONS.contains(&action)
}

pub fn find_thread_scoped(hub: &HubRef, key: &str, project: Option<ProjectId>) -> Result<Thread> {
    let threads: Vec<Thread> = hub
        .db
        .threads()?
        .into_iter()
        .filter(|t| project.is_none_or(|p| t.project == p))
        .collect();
    crate::follow::pick(&threads, key)
        .cloned()
        .map_err(|e| anyhow!(e))
}

fn find_thread(hub: &HubRef, key: &str) -> Result<Thread> {
    find_thread_scoped(hub, key, None)
}

fn find_project(hub: &HubRef, name: &str) -> Result<Project> {
    hub.db
        .project_by_name(name.trim())?
        .ok_or_else(|| anyhow!("no project is named {name}"))
}

fn find_machine(hub: &HubRef, name: &str) -> Result<Machine> {
    let name = name.trim();
    let all = hub.db.machines()?;
    all.iter()
        .find(|m| m.id.to_string() == name)
        .or_else(|| {
            all.iter()
                .find(|m| m.name.eq_ignore_ascii_case(name) && m.status == MachineStatus::Connected)
        })
        .or_else(|| all.iter().find(|m| m.name.eq_ignore_ascii_case(name)))
        .cloned()
        .ok_or_else(|| anyhow!("no machine is named {name}"))
}

fn text(input: &Value, key: &str) -> Result<String> {
    input[key]
        .as_str()
        .map(String::from)
        .ok_or_else(|| anyhow!("{key} is required"))
}

fn repos_from(v: &Value) -> Option<Vec<Repo>> {
    v.as_array().map(|a| {
        a.iter()
            .filter_map(|x| x.as_str())
            .map(|r| {
                let r = r.trim().trim_end_matches(".git");
                let name = r.rsplit('/').next().unwrap_or(r).to_string();
                Repo {
                    name,
                    github: r.contains('/').then(|| r.to_string()),
                }
            })
            .collect()
    })
}

fn digest(events: &[StoredEvent], last: usize) -> Value {
    let mut turns: Vec<Value> = vec![];
    for e in events {
        match &e.event {
            Event::TurnStarted { input, .. } => {
                let asked: Vec<&str> = input
                    .iter()
                    .filter_map(|p| {
                        if let InputPart::Text { text } = p {
                            Some(text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect();
                turns.push(json!({ "asked": asked.join("\n"), "steps": [] }));
            }
            Event::ItemCompleted { item, .. } if item.parent.is_none() => {
                let Some(t) = turns.last_mut() else { continue };
                let steps = t["steps"].as_array_mut().unwrap();
                match &item.body {
                    ItemBody::AgentMessage { text } => steps.push(json!({ "agent": text })),
                    ItemBody::ToolCall {
                        name,
                        input,
                        status,
                        ..
                    } => {
                        let brief = input["description"]
                            .as_str()
                            .or(input["command"].as_str())
                            .or(input["file_path"].as_str())
                            .unwrap_or("");
                        steps.push(json!({ "tool": name, "what": brief.chars().take(200).collect::<String>(), "status": status }));
                    }
                    _ => {}
                }
            }
            Event::Request { request, .. } => {
                if let Some(t) = turns.last_mut() {
                    t["waiting_on_user"] = serde_json::to_value(request).unwrap_or(Value::Null);
                }
            }
            Event::RequestResolved { .. } => {
                if let Some(t) = turns.last_mut() {
                    t.as_object_mut().unwrap().remove("waiting_on_user");
                }
            }
            Event::TurnCompleted { outcome, error, .. } => {
                if let Some(t) = turns.last_mut() {
                    t["outcome"] = json!(outcome);
                    if let Some(e) = error {
                        t["error"] = json!(e);
                    }
                }
            }
            _ => {}
        }
    }
    let skip = turns.len().saturating_sub(last.max(1));
    json!(turns.into_iter().skip(skip).collect::<Vec<_>>())
}

pub fn read_thread(
    hub: &HubRef,
    key: &str,
    last: usize,
    project: Option<ProjectId>,
) -> Result<Value> {
    let t = find_thread_scoped(hub, key, project)?;
    let last = last.max(1);
    let from = hub
        .db
        .last_turns_started(t.id, last as i64)?
        .last()
        .map_or(0, |e| e.seq - 1);
    let events = hub.db.events_after(t.id, from, 1_000_000)?;
    Ok(json!({
        "id": t.id,
        "title": t.title,
        "agent": t.agent,
        "status": t.status,
        "model": t.settings.model,
        "effort": t.settings.effort,
        "permission_mode": t.settings.permission_mode,
        "turns": digest(&events, last),
    }))
}

pub fn follow_thread(
    hub: &HubRef,
    caller: &Thread,
    key: &str,
    follow: bool,
    project: Option<ProjectId>,
) -> Result<Value> {
    let t = find_thread_scoped(hub, key, project)?;
    if t.id == caller.id {
        bail!("that's this thread");
    }
    if follow {
        if hub.hands_off(caller.agent) {
            bail!(
                "Antigravity threads signed in with Google can't receive automated follower messages. Use threads.read to check progress instead."
            );
        }
        let mut up = Some(caller.id);
        for _ in 0..64 {
            up = up.and_then(|id| hub.follower(id));
            if up == Some(t.id) {
                bail!("that thread follows this one, so the two would wake each other without end");
            }
        }
        hub.set_follower(t.id, Some(caller.id));
        Ok(json!({ "following": t.title, "status": t.status }))
    } else {
        if hub.follower(t.id) == Some(caller.id) {
            hub.set_follower(t.id, None);
        }
        Ok(json!({ "stopped_following": t.title }))
    }
}

pub async fn send_thread(
    hub: &HubRef,
    caller: &Thread,
    key: &str,
    message: String,
    queue: bool,
    project: Option<ProjectId>,
) -> Result<Value> {
    let t = find_thread_scoped(hub, key, project)?;
    if t.id == caller.id {
        bail!("that's this thread");
    }
    if hub.hands_off(t.agent) {
        bail!(
            "Antigravity threads signed in with Google take messages only from the user. Ask the user to send it, or to add a Gemini API key in Settings."
        );
    }
    exec_call(
        hub,
        AppCall::Send {
            thread: t.id,
            input: vec![InputPart::Text { text: message }],
            settings: None,
            queue,
            client_id: None,
        },
    )
    .await?;
    Ok(json!({ "sent_to": t.title }))
}

pub async fn stop_thread(
    hub: &HubRef,
    caller: &Thread,
    key: &str,
    project: Option<ProjectId>,
) -> Result<Value> {
    let t = find_thread_scoped(hub, key, project)?;
    let quiet = hub.follower(t.id) == Some(caller.id)
        && matches!(t.status, ThreadStatus::Running | ThreadStatus::Waiting);
    if quiet {
        hub.quiet_stops
            .lock()
            .unwrap()
            .insert(t.id, std::time::Instant::now());
    }
    let unsent = match exec_call(hub, AppCall::Interrupt { thread: t.id }).await {
        Ok(v) => v["unsent"].clone(),
        Err(e) => {
            hub.quiet_stops.lock().unwrap().remove(&t.id);
            return Err(e);
        }
    };
    let texts: Vec<String> = serde_json::from_value::<Vec<Queued>>(unsent)
        .unwrap_or_default()
        .iter()
        .map(|q| crate::hub::input_text(&q.input))
        .collect();
    if texts.is_empty() {
        Ok(json!({ "stopped": t.title }))
    } else {
        Ok(json!({ "stopped": t.title, "unsent": texts }))
    }
}

pub async fn run(hub: &HubRef, caller: &Thread, action: &str, input: Value) -> Result<Value> {
    match action {
        "threads.list" => {
            let projects = hub.db.projects()?;
            let machines = hub.db.machines()?;
            let want_project = match input["project"].as_str() {
                Some(n) => Some(find_project(hub, n)?.id),
                None => None,
            };
            let want_status = input["status"].as_str();
            let list: Vec<Value> = hub
                .db
                .threads()?
                .into_iter()
                .filter(|t| t.hidden_at.is_none())
                .filter(|t| want_project.is_none_or(|p| t.project == p))
                .filter(|t| want_status.is_none_or(|s| t.status.as_str() == s))
                .take(200)
                .map(|t| {
                    json!({
                        "id": t.id,
                        "title": t.title,
                        "agent": t.agent,
                        "project": projects.iter().find(|p| p.id == t.project).map(|p| p.name.clone()),
                        "machine": t.machine.and_then(|m| machines.iter().find(|x| x.id == m)).map(|m| m.name.clone()),
                        "status": t.status,
                        "model": t.settings.model,
                        "effort": t.settings.effort,
                        "permission_mode": t.settings.permission_mode,
                        "this_thread": t.id == caller.id,
                        "following": hub.follower(t.id) == Some(caller.id),
                    })
                })
                .collect();
            Ok(json!(list))
        }
        "threads.read" => {
            let last = input["last"].as_u64().unwrap_or(3) as usize;
            read_thread(hub, &text(&input, "thread")?, last, None)
        }
        "threads.start" => {
            let target_agent = match input["agent"].as_str() {
                None => caller.agent,
                Some("claude") => AgentKind::Claude,
                Some("antigravity") => AgentKind::Antigravity,
                Some(other) => bail!("unknown agent {other}; choose claude or antigravity"),
            };
            if hub.hands_off(target_agent) {
                bail!(
                    "Antigravity threads signed in with Google take messages only from the user. Pass agent: \"claude\", ask the user to send it, or add a Gemini API key in Settings."
                );
            }
            let p = find_project(hub, &text(&input, "project")?)?;
            let machine = match input["on"].as_str() {
                Some(n) => Some(find_machine(hub, n)?.id),
                None => None,
            };
            let base = if target_agent == caller.agent {
                caller.settings.clone()
            } else {
                AgentSettings {
                    model: None,
                    effort: caller.settings.effort.clone(),
                    permission_mode: caller.settings.permission_mode.clone(),
                }
            };
            let settings = AgentSettings {
                model: input["model"].as_str().map(String::from).or(base.model),
                effort: input["effort"].as_str().map(String::from).or(base.effort),
                ..base
            };
            let caller_hands_off = hub.hands_off(caller.agent);
            let follow = match input["follow"].as_bool() {
                Some(true) if caller_hands_off => bail!(
                    "Antigravity threads signed in with Google can't receive automated follower messages. Omit follow or set follow: false, and check progress with threads.read."
                ),
                Some(v) => v,
                None => !caller_hands_off,
            };
            let v = exec_call(
                hub,
                AppCall::CreateThread {
                    project: p.id,
                    agent: target_agent,
                    machine,
                    settings,
                    input: vec![InputPart::Text {
                        text: text(&input, "prompt")?,
                    }],
                },
            )
            .await?;
            if let Some(id) = v["id"].as_str().and_then(|s| s.parse::<ThreadId>().ok()) {
                hub.mark_started_by(id, caller.id);
                if follow {
                    hub.set_follower(id, Some(caller.id));
                }
            }
            Ok(
                json!({ "thread": v["id"], "title": v["title"], "agent": target_agent, "following": follow }),
            )
        }
        "threads.follow" => follow_thread(
            hub,
            caller,
            &text(&input, "thread")?,
            input["follow"].as_bool().unwrap_or(true),
            None,
        ),
        "threads.send" => {
            send_thread(
                hub,
                caller,
                &text(&input, "thread")?,
                text(&input, "message")?,
                input["queue"].as_bool().unwrap_or(false),
                None,
            )
            .await
        }
        "threads.stop" => stop_thread(hub, caller, &text(&input, "thread")?, None).await,
        "threads.move" => {
            let t = find_thread(hub, &text(&input, "thread")?)?;
            let machine = match input["to"].as_str() {
                Some(n) => Some(find_machine(hub, n)?.id),
                None => None,
            };
            let moved = hub.move_thread(t.id, machine).await?;
            let name = moved
                .machine
                .and_then(|m| hub.db.machine(m).ok().flatten())
                .map(|m| m.name);
            Ok(
                json!({ "moved": moved.title, "to": name.unwrap_or_else(|| "the project's machine, made at its next message".into()) }),
            )
        }
        "projects.list" => {
            let machines = hub.db.machines()?;
            Ok(json!(hub
                .projects()?
                .into_iter()
                .map(|p| json!({
                    "name": p.name,
                    "runs_on": if !hub.providers.get(&p.provider).is_some_and(|x| x.creates_machines()) {
                        p.default_machine.and_then(|m| machines.iter().find(|x| x.id == m)).map(|m| m.name.clone()).unwrap_or_else(|| p.provider.clone())
                    } else {
                        p.provider.clone()
                    },
                    "kind": p.kind,
                    "size": p.size,
                    "folder": p.path,
                    "repos": p.repos.iter().map(|r| r.github.clone().unwrap_or_else(|| r.name.clone())).collect::<Vec<_>>(),
                    "setup": p.setup,
                    "env": p.env,
                    "tools": p.tools,
                    "allow_main": p.allow_main,
                    "full_access": p.trusted,
                }))
                .collect::<Vec<_>>()))
        }
        "projects.create" => {
            let p = hub.create_project(crate::hub::NewProject {
                name: text(&input, "name")?,
                provider: Some(
                    input["provider"]
                        .as_str()
                        .map(String::from)
                        .unwrap_or_else(|| hub.default_provider()),
                ),
                ..Default::default()
            })?;
            let v = update_project(hub, p, &input)?;
            Ok(v)
        }
        "projects.update" => {
            let p = find_project(hub, &text(&input, "project")?)?;
            update_project(hub, p, &input)
        }
        "machines.list" => Ok(json!(hub
            .db
            .machines()?
            .into_iter()
            .map(|m| json!({
                "name": m.name,
                "provider": m.provider,
                "status": m.status,
                "size": m.size,
                "project": m.project.and_then(|p| hub.db.project(p).ok().flatten()).map(|p| p.name),
            }))
            .collect::<Vec<_>>())),
        "machines.destroy" => {
            let m = find_machine(hub, &text(&input, "machine")?)?;
            exec_call(hub, AppCall::DestroyMachine { machine: m.id }).await?;
            Ok(json!({ "destroyed": m.name }))
        }
        "credentials.list" => exec_call(hub, AppCall::ListSecrets).await,
        "credentials.set" => {
            exec_call(
                hub,
                AppCall::SetSecret {
                    name: text(&input, "name")?,
                    value: text(&input, "value")?,
                },
            )
            .await?;
            Ok(json!({ "saved": input["name"] }))
        }
        "credentials.remove" => {
            exec_call(
                hub,
                AppCall::DeleteSecret {
                    name: text(&input, "name")?,
                },
            )
            .await?;
            Ok(json!({ "removed": input["name"] }))
        }
        _ => bail!("unknown tool {action}"),
    }
}

fn update_project(hub: &HubRef, mut p: Project, input: &Value) -> Result<Value> {
    let opt = |k: &str| input[k].as_str().map(String::from);
    hub.place_project(&mut p, opt("provider"), opt("kind"), opt("size"))?;
    let moved = p.id;
    if let Some(r) = repos_from(&input["repos"]) {
        p.repos = r;
    }
    if let Some(setup) = input["setup"].as_str() {
        p.setup = Some(setup.trim().to_string()).filter(|s| !s.is_empty());
    }
    if let Some(env) = input["env"].as_array() {
        p.env = env
            .iter()
            .filter_map(|e| e.as_str().map(|s| s.trim().to_string()))
            .filter(|e| !e.is_empty())
            .collect();
    }
    if let Some(allow) = input["allow_main"].as_bool() {
        p.allow_main = allow;
    }
    hub.db.update_project(&p)?;
    crate::lifecycle::retire_moved(hub, moved);
    hub.publish_project(&p);
    Ok(json!({
        "name": p.name,
        "provider": p.provider,
        "kind": p.kind,
        "size": p.size,
        "repos": p.repos.iter().map(|r| r.github.clone().unwrap_or_else(|| r.name.clone())).collect::<Vec<_>>(),
        "setup": p.setup,
        "env": p.env,
        "allow_main": p.allow_main,
    }))
}
