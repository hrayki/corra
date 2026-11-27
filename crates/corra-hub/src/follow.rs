use corra_proto::*;

const LAST_MESSAGE_CHARS: usize = 2000;

pub fn line(t: &Thread, project: &str, what: &str) -> String {
    let end = if what.ends_with("```") { "" } else { "." };
    format!(
        "[corra] The thread \"{}\" in {project} ({}) {what}{end}",
        t.title, t.id
    )
}

pub fn note(t: &Thread, project: &str, event: &Event, last: Option<&str>) -> Option<String> {
    match event {
        Event::TurnCompleted { outcome, error, .. } => {
            let what = match (outcome, error) {
                (TurnOutcome::Completed, _) => "finished its turn".to_string(),
                (TurnOutcome::Interrupted, _) => "was stopped mid-turn".to_string(),
                (TurnOutcome::Failed, Some(e)) => format!("stopped with an error: {e}"),
                (TurnOutcome::Failed, None) => "stopped with an error".to_string(),
            };
            let mut text = line(t, project, &what);
            if let Some(last) = last.map(str::trim).filter(|l| !l.is_empty()) {
                text.push_str("\n\nIts last message:\n\n");
                text.extend(last.chars().take(LAST_MESSAGE_CHARS));
                if last.chars().count() > LAST_MESSAGE_CHARS {
                    text.push('…');
                }
            }
            Some(text)
        }
        Event::Request { request, .. } => {
            let what = match request {
                AgentRequest::ToolApproval { tool, .. } => format!("it wants to use {tool}"),
                AgentRequest::Question { questions } => match questions.as_slice() {
                    [] => "it has a question".to_string(),
                    [q] => format!("it asks \"{}\"", q.question),
                    [q, rest @ ..] => {
                        format!("it asks \"{}\" and {} more", q.question, rest.len())
                    }
                },
                AgentRequest::PlanApproval { .. } => "it has a plan to approve".to_string(),
                AgentRequest::HubAction { action, input } if action == "secret.request" => {
                    format!(
                        "it asks for a secret, {}",
                        input["name"].as_str().unwrap_or("unnamed")
                    )
                }
                AgentRequest::HubAction { action, .. } => format!("it asks to run {action}"),
            };
            Some(line(
                t,
                project,
                &format!("is waiting for the user: {what}"),
            ))
        }
        _ => None,
    }
}

pub fn last_message(tail: &[StoredEvent], turn: &str) -> Option<String> {
    tail.iter().rev().find_map(|e| match &e.event {
        Event::ItemCompleted { turn: t, item } if t == turn && item.parent.is_none() => {
            match &item.body {
                ItemBody::AgentMessage { text } => Some(text.clone()),
                _ => None,
            }
        }
        _ => None,
    })
}

pub fn pick<'a>(threads: &'a [Thread], key: &str) -> Result<&'a Thread, String> {
    let key = key.trim().to_lowercase();
    if let Some(t) = threads.iter().find(|t| t.id.to_string() == key) {
        return Ok(t);
    }
    if key.len() < 6 {
        return Err("give the thread's whole id, or at least its last 6 characters".into());
    }
    let mut matches = threads.iter().filter(|t| t.id.to_string().ends_with(&key));
    match (matches.next(), matches.next()) {
        (Some(t), None) => Ok(t),
        (None, _) => Err(format!("no thread has an id ending in {key}")),
        (Some(_), Some(_)) => Err(format!(
            "several threads have an id ending in {key}; give more of it"
        )),
    }
}

pub fn spec_mark(tools: &[ToolDef], env: &[(String, String)]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(serde_json::to_vec(&(tools, env)).unwrap_or_default());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread(id: &str) -> Thread {
        Thread {
            id: id.parse().unwrap(),
            project: uuid::Uuid::nil(),
            title: String::new(),
            agent: AgentKind::Claude,
            machine: None,
            session_id: None,
            cwd: String::new(),
            settings: AgentSettings::default(),
            status: ThreadStatus::Idle,
            last_seq: 0,
            created_at: 0,
            updated_at: 0,
            pinned_at: None,
            settled_at: None,
            hidden_at: None,
            started_by: None,
        }
    }

    #[test]
    fn threads_made_in_the_same_hours_are_told_apart_by_their_endings() {
        let threads = [
            thread("01a207a8-f241-718b-9c4b-155018099340"),
            thread("01a207a8-bf2c-71db-84ec-e28e37eec2cc"),
            thread("01a2049c-0000-7000-8000-000000eec2cc"),
        ];
        let id = |r: Result<&Thread, String>| r.map(|t| t.id.to_string());
        assert_eq!(
            id(pick(&threads, "099340")).unwrap(),
            "01a207a8-f241-718b-9c4b-155018099340"
        );
        assert_eq!(
            id(pick(&threads, " 01A207A8-BF2C-71DB-84EC-E28E37EEC2CC ")).unwrap(),
            "01a207a8-bf2c-71db-84ec-e28e37eec2cc"
        );
        assert!(pick(&threads, "01a207a8").is_err());
        assert!(pick(&threads, "eec2cc").unwrap_err().contains("several"));
        assert!(pick(&threads, "9340").is_err());
    }
}
