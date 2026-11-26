use anyhow::{Result, anyhow, bail};
use corra_proto::*;
use serde_json::{Value, json};

use crate::hub::HubRef;

const ASK_UNLESS_MAIN_ALLOWED: &[&str] = &["github.pr.merge"];

fn tool(action: &str, description: &str, props: Value, required: &[&str]) -> ToolDef {
    ToolDef {
        action: action.into(),
        description: description.into(),
        input_schema: json!({ "type": "object", "properties": props, "required": required }),
    }
}

const REPO: &str = "Repository as owner/name or just name. Optional when the project has one repo.";

fn all_defs() -> Vec<ToolDef> {
    let repo = json!({ "type": "string", "description": REPO });
    let number = json!({ "type": "integer", "description": "The pull request or issue number." });
    vec![
        tool(
            "threads.list",
            "List this project's threads with their titles and status.",
            json!({}),
            &[],
        ),
        tool(
            "threads.start",
            "Start a new thread in this project with its own agent, which works on the prompt independently. Returns the new thread's id.",
            json!({
                "prompt": { "type": "string", "description": "The task for the new agent." },
                "model": { "type": "string", "description": "Optional model id." }
            }),
            &["prompt"],
        ),
        tool(
            "secret.request",
            "Ask the user for a secret only they have, like an API key or a password, instead of asking in the chat. They type it into a card in this thread, and it's written to a file on this machine that only you can read; the value never appears in the conversation. Returns the file's path: use it from a command without printing it, like --token \"$(cat PATH)\". The file is deleted after a day.",
            json!({
                "name": { "type": "string", "description": "A short name for it, like STRIPE_TEST_KEY: letters, digits, '_', '-' and '.'." },
                "why": { "type": "string", "description": "One line the user sees on the card: what the secret is and what you'll do with it." }
            }),
            &["name", "why"],
        ),
        tool(
            "github.pr.list",
            "List pull requests in one of the project's GitHub repos.",
            json!({ "repo": repo, "state": { "type": "string", "enum": ["open", "closed", "all"] } }),
            &[],
        ),
        tool(
            "github.pr.view",
            "Show a pull request: description, state, mergeability, checks, reviews and comments.",
            json!({ "repo": repo, "number": number }),
            &["number"],
        ),
        tool(
            "github.pr.create",
            "Open a pull request from a branch you pushed. Push the branch with git first.",
            json!({
                "repo": repo,
                "head": { "type": "string", "description": "The branch with your changes." },
                "base": { "type": "string", "description": "The branch to merge into. Defaults to the repo's default branch." },
                "title": { "type": "string" },
                "body": { "type": "string", "description": "Markdown description." },
                "draft": { "type": "boolean" }
            }),
            &["head", "title"],
        ),
        tool(
            "github.pr.update",
            "Change a pull request's title, description, base branch, or close it (state: closed).",
            json!({
                "repo": repo, "number": number,
                "title": { "type": "string" }, "body": { "type": "string" },
                "base": { "type": "string" }, "state": { "type": "string", "enum": ["open", "closed"] }
            }),
            &["number"],
        ),
        tool(
            "github.pr.comment",
            "Comment on a pull request.",
            json!({ "repo": repo, "number": number, "body": { "type": "string" } }),
            &["number", "body"],
        ),
        tool(
            "github.pr.merge",
            "Merge a pull request. This is the only way to change the default branch.",
            json!({
                "repo": repo, "number": number,
                "method": { "type": "string", "enum": ["squash", "merge", "rebase"], "description": "Defaults to squash." },
                "title": { "type": "string", "description": "Optional commit title." }
            }),
            &["number"],
        ),
        tool(
            "github.checks",
            "Show the CI check runs for a branch, tag or commit, with how many are failing or pending.",
            json!({ "repo": repo, "ref": { "type": "string" } }),
            &["ref"],
        ),
        tool(
            "github.run.logs",
            "Read the end of the logs of a GitHub Actions run's failed jobs. Get run_id from github.checks.",
            json!({
                "repo": repo,
                "run_id": { "type": "integer" },
                "all": { "type": "boolean", "description": "Include jobs that didn't fail." }
            }),
            &["run_id"],
        ),
        tool(
            "github.issue.list",
            "List issues in one of the project's GitHub repos.",
            json!({ "repo": repo, "state": { "type": "string", "enum": ["open", "closed", "all"] } }),
            &[],
        ),
        tool(
            "github.issue.view",
            "Show an issue and its comments.",
            json!({ "repo": repo, "number": number }),
            &["number"],
        ),
        tool(
            "github.issue.create",
            "Open an issue.",
            json!({ "repo": repo, "title": { "type": "string" }, "body": { "type": "string" } }),
            &["title"],
        ),
        tool(
            "github.issue.comment",
            "Comment on an issue.",
            json!({ "repo": repo, "number": number, "body": { "type": "string" } }),
            &["number", "body"],
        ),
    ]
}

fn is_manager(hub: &HubRef, machine: MachineId) -> bool {
    hub.machine_trusted(machine)
}

pub fn defs_for(hub: &HubRef, project: &Project, machine: MachineId) -> Vec<ToolDef> {
    let phone = project.tools.iter().any(|t| t == "phone");
    let mut defs: Vec<ToolDef> = all_defs()
        .into_iter()
        .chain(if phone {
            crate::phone::defs(hub)
        } else {
            vec![]
        })
        .collect();
    if is_manager(hub, machine) {
        let manage = crate::manage::defs(hub);
        defs.retain(|d| !manage.iter().any(|m| m.action == d.action));
        defs.extend(manage);
    }
    defs
}

pub async fn call(
    hub: &HubRef,
    machine: MachineId,
    thread: ThreadId,
    turn: &str,
    action: &str,
    input: Value,
) -> Result<Value> {
    let t = hub
        .db
        .thread(thread)?
        .ok_or_else(|| anyhow!("no such thread"))?;
    let phone = crate::phone::is_phone(action);
    if (crate::manage::is_manage(hub, action) || phone) && is_manager(hub, machine) {
        if phone {
            return crate::phone::run(hub, machine, action, input).await;
        }
        return crate::manage::run(hub, &t, action, input).await;
    }
    let p = hub
        .db
        .project(t.project)?
        .ok_or_else(|| anyhow!("no such project"))?;
    if !phone && !all_defs().iter().any(|d| d.action == action) {
        bail!("unknown tool {action}");
    }
    if phone && !p.tools.iter().any(|t| t == "phone") {
        bail!("the test phone is switched off for this project");
    }
    if action == "secret.request" {
        return secret_request(hub, machine, thread, turn, &input).await;
    }
    let mut input = input;
    if let Some(obj) = input.as_object_mut() {
        obj.remove("sha");
    }
    if ASK_UNLESS_MAIN_ALLOWED.contains(&action) && !p.allow_main && !p.trusted {
        let shown = crate::github::merge_request(hub, &p, &input).await?;
        input["sha"] = shown["sha"].clone();
        match hub.ask(thread, turn, action, &shown).await? {
            RequestResponse::Allow { .. } => {}
            RequestResponse::Deny { message } => {
                bail!(
                    "{}",
                    message.unwrap_or_else(|| "The user declined this.".into())
                )
            }
            _ => bail!("The request was cancelled."),
        }
    }
    if phone {
        return crate::phone::run(hub, machine, action, input).await;
    }
    run(hub, &t, &p, action, input).await
}

pub fn secret_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

async fn secret_request(
    hub: &HubRef,
    machine: MachineId,
    thread: ThreadId,
    turn: &str,
    input: &Value,
) -> Result<Value> {
    let name = input["name"].as_str().unwrap_or_default().trim();
    let why = input["why"].as_str().unwrap_or_default().trim();
    if !secret_name(name) {
        bail!("name is required, and may use letters, digits, '_', '-' and '.'");
    }
    if why.is_empty() {
        bail!("why is required: say what the secret is and what you'll do with it");
    }
    let Some(value) = hub.ask_secret(thread, turn, name, why).await? else {
        bail!("The user declined to give {name}.");
    };
    let sent = hub
        .runner_request_timeout(
            machine,
            corra_proto::runner::RunnerRequest::Secret {
                name: name.to_string(),
                value,
            },
            std::time::Duration::from_secs(15),
        )
        .await;
    match sent {
        Ok(v) => Ok(json!({ "file": v["path"] })),
        Err(e) if e.to_string().contains("did not answer in time") => bail!(
            "This machine's runner is older than secret requests, so the secret wasn't stored. The runner updates when the machine next wakes from sleep; ask the user again after that."
        ),
        Err(e) => Err(e),
    }
}

async fn run(hub: &HubRef, t: &Thread, p: &Project, action: &str, input: Value) -> Result<Value> {
    match action {
        "threads.list" => {
            let list: Vec<Value> = hub
                .db
                .threads()?
                .into_iter()
                .filter(|x| x.project == p.id && x.hidden_at.is_none())
                .map(|x| json!({ "id": x.id, "title": x.title, "status": x.status, "updated_at": x.updated_at }))
                .collect();
            Ok(json!(list))
        }
        "threads.start" => {
            let prompt = input["prompt"]
                .as_str()
                .ok_or_else(|| anyhow!("prompt is required"))?;
            let mut settings = t.settings.clone();
            if let Some(m) = input["model"].as_str() {
                settings.model = Some(m.to_string());
            }
            let n = hub
                .create_thread(
                    p.id,
                    t.agent,
                    t.machine,
                    settings,
                    vec![InputPart::Text {
                        text: prompt.to_string(),
                    }],
                )
                .await?;
            hub.mark_started_by(n.id, t.id);
            Ok(json!({ "thread": n.id, "title": n.title }))
        }
        a if a.starts_with("github.") => crate::github::run(hub, p, a, &input).await,
        _ => bail!("unknown tool {action}"),
    }
}

pub fn info(hub: &HubRef) -> Vec<ToolInfo> {
    let has = |k: &str| hub.secrets.get(k).ok().flatten().is_some();
    let tool = |name: &str, label: &str, about: &str, needs: &[&str], host: Option<&str>| {
        let missing: Vec<&str> = needs.iter().copied().filter(|k| !has(k)).collect();
        ToolInfo {
            name: name.into(),
            label: label.into(),
            about: about.into(),
            needs: needs.iter().map(|s| s.to_string()).collect(),
            host: host.map(String::from),
            ready: missing.is_empty(),
            missing: (!missing.is_empty()).then(|| format!("Needs a {label} key")),
        }
    };
    let phone_host = crate::phone::host(hub);
    let phone_ready = phone_host
        .as_ref()
        .is_some_and(|h| crate::phone::check_host(hub, h).is_ok());
    vec![
        tool(
            "github",
            "GitHub",
            "Clone, push, and work with pull requests, issues and checks. The hub holds the key.",
            &["github.token"],
            None,
        ),
        tool(
            "browser",
            "Browser",
            "A Chromium on the project's machine that agents drive and you can watch.",
            &[],
            None,
        ),
        ToolInfo {
            ready: phone_ready,
            missing: (!phone_ready).then(|| {
                if phone_host.is_some() {
                    "Its machine's provider isn't connected".to_string()
                } else {
                    "Off: pick where it runs".to_string()
                }
            }),
            ..tool(
                "phone",
                "Android phone",
                "A test phone in the cloud that agents drive and you can watch. Its apps and data go when it stops.",
                &[],
                Some("kvm"),
            )
        },
        tool(
            "threads",
            "Other threads",
            "List the project's threads and start new ones.",
            &[],
            None,
        ),
        tool(
            "manage",
            "Managing corra",
            "Create and change projects, read and message any thread, and manage machines and keys. Only full-access projects get it.",
            &[],
            None,
        ),
    ]
}
