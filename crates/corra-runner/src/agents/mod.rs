pub mod claude;
pub mod import;
mod record;
#[cfg(test)]
mod replay;

use corra_proto::*;
use tokio::sync::mpsc;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionCmd {
    Send {
        input: Vec<InputPart>,
        settings: Option<AgentSettings>,
    },
    Interrupt,
    StopTask {
        tool: String,
    },
    Respond {
        request: String,
        response: RequestResponse,
    },
    Settings(AgentSettings),
    Stop,
}

#[derive(Debug)]
pub enum AgentOut {
    Event(Event),
    Delta {
        turn: String,
        item: String,
        parent: Option<String>,
        delta: Delta,
    },
    Status(ThreadStatus),
    Exited {
        error: Option<String>,
    },
}

pub struct HubCall {
    pub thread: ThreadId,
    pub turn: String,
    pub action: String,
    pub input: serde_json::Value,
    pub reply: tokio::sync::oneshot::Sender<Result<serde_json::Value, String>>,
}

pub struct SessionCtx {
    pub browser_cdp: Option<String>,
    pub baseline: std::sync::Arc<tokio::sync::Mutex<Vec<(String, String)>>>,
    pub hub_url: String,
    pub hub_token: String,
    pub hub: mpsc::UnboundedSender<HubCall>,
    pub sync: Option<crate::sync::SyncRef>,
    pub spec: StartSpec,
    pub cwd: std::path::PathBuf,
    pub extra_dirs: Vec<std::path::PathBuf>,
    pub out: mpsc::UnboundedSender<AgentOut>,
    pub cmds: mpsc::UnboundedReceiver<SessionCmd>,
    pub initial: Option<Vec<InputPart>>,
}

pub fn spawn(ctx: SessionCtx) {
    match ctx.spec.agent {
        AgentKind::Claude => {
            tokio::spawn(async move {
                for p in crate::git::ensure_repos(
                    &ctx.cwd,
                    &ctx.hub_url,
                    &ctx.hub_token,
                    &ctx.spec.repos,
                )
                .await
                {
                    let _ = ctx.out.send(AgentOut::Event(Event::Notice {
                        level: NoticeLevel::Warning,
                        text: p,
                    }));
                }
                if let Some(sync) = ctx.sync.clone() {
                    restore(&ctx, &sync).await;
                }
                {
                    let mut b = ctx.baseline.lock().await;
                    for (name, path) in crate::git::repos(&ctx.cwd) {
                        if let Ok(sha) = crate::git::snapshot(&path).await {
                            b.push((name, sha));
                        }
                    }
                }
                claude::run(ctx).await
            });
        }
    }
}

fn notice(ctx: &SessionCtx, level: NoticeLevel, text: String) {
    let _ = ctx.out.send(AgentOut::Event(Event::Notice { level, text }));
}

fn folders(r: &[String]) -> String {
    r.iter()
        .map(|x| {
            if x == "." {
                "the project folder".to_string()
            } else {
                x.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

async fn restore(ctx: &SessionCtx, sync: &crate::sync::Syncer) {
    let thread = ctx.spec.thread;
    if ctx.spec.moved_from.is_some() {
        match crate::git::restore_work(sync, thread, &ctx.cwd).await {
            Ok(r) if !r.is_empty() => notice(
                ctx,
                NoticeLevel::Info,
                format!(
                    "Brought this thread's uncommitted work in {} along.",
                    folders(&r)
                ),
            ),
            Ok(_) => {}
            Err(e) => notice(
                ctx,
                NoticeLevel::Warning,
                format!("Couldn't bring the uncommitted work along: {e:#}"),
            ),
        }
    }
    let base = claude::session_base(&ctx.cwd);
    let mut sources = vec![];
    if ctx.spec.resume_session.is_some() || ctx.spec.fork_of.is_some() {
        sources.push(thread);
    }
    if let Some(parent) = ctx.spec.fork_of.as_ref().and_then(|f| f.thread) {
        sources.push(parent);
    }
    for t in sources {
        match sync.restore(t, &base).await {
            Ok(n) if n > 0 => tracing::info!(%thread, "restored {n} session files from the hub"),
            Ok(_) => {}
            Err(e) => tracing::warn!(%thread, "couldn't restore session files: {e:#}"),
        }
    }
}

pub fn session_files(
    agent: AgentKind,
    cwd: &std::path::Path,
    session_id: &str,
) -> (std::path::PathBuf, Vec<std::path::PathBuf>) {
    match agent {
        AgentKind::Claude => (
            claude::session_base(cwd),
            claude::session_files(cwd, session_id),
        ),
    }
}
