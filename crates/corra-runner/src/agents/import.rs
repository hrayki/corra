use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corra_proto::*;
use serde::Serialize;
use serde_json::Value;

use super::claude::{config_dir, session_base};

#[derive(Debug, Clone, Serialize)]
pub struct FoundSession {
    pub id: String,
    pub cwd: String,
    pub title: String,
    pub updated_at: i64,
    pub size: u64,
    #[serde(skip)]
    pub path: PathBuf,
}

fn read_head(path: &Path, n: u64) -> String {
    let mut s = String::new();
    if let Ok(f) = std::fs::File::open(path) {
        let _ = f.take(n).read_to_string(&mut s);
    }
    s
}

fn read_tail(path: &Path, n: u64) -> String {
    let Ok(mut f) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(n)));
    let mut buf = vec![];
    let _ = f.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).to_string()
}

fn prompt_text(v: &Value) -> Option<String> {
    if v["type"] != "user"
        || v["isMeta"] == true
        || v["isSidechain"] == true
        || v["isCompactSummary"] == true
    {
        return None;
    }
    let text = match &v["message"]["content"] {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|p| p["type"] == "text")
            .filter_map(|p| p["text"].as_str())
            .filter(|t| !t.trim_start().starts_with('<'))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    let t = text.trim();
    if t.is_empty() || t.starts_with('<') || t.starts_with("[Request interrupted") {
        return None;
    }
    Some(t.to_string())
}

fn short(s: &str) -> String {
    let line = s.lines().next().unwrap_or("").trim();
    if line.chars().count() > 80 {
        format!("{}…", line.chars().take(79).collect::<String>())
    } else {
        line.to_string()
    }
}

fn describe(path: &Path) -> Option<FoundSession> {
    let id = path.file_stem()?.to_string_lossy().to_string();
    uuid::Uuid::parse_str(&id).ok()?;
    let meta = std::fs::metadata(path).ok()?;
    let mut cwd = String::new();
    let mut first = None;
    for line in read_head(path, 256 * 1024).lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if cwd.is_empty()
            && let Some(c) = v["cwd"].as_str()
        {
            cwd = c.to_string();
        }
        if first.is_none() {
            first = prompt_text(&v);
        }
        if !cwd.is_empty() && first.is_some() {
            break;
        }
    }
    let mut title = None;
    for line in read_tail(path, 256 * 1024).lines().rev() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(t) = v["customTitle"].as_str().or(v["summary"].as_str()) {
            title = Some(t.to_string());
            break;
        }
    }
    let title = title.or(first.map(|f| short(&f)))?;
    let updated_at = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Some(FoundSession {
        id,
        cwd,
        title,
        updated_at,
        size: meta.len(),
        path: path.to_path_buf(),
    })
}

pub fn list_sessions() -> Vec<FoundSession> {
    let mut out = vec![];
    let Ok(dirs) = std::fs::read_dir(config_dir().join("projects")) else {
        return out;
    };
    for d in dirs.flatten() {
        let Ok(files) = std::fs::read_dir(d.path()) else {
            continue;
        };
        for f in files.flatten() {
            let p = f.path();
            if p.extension().is_some_and(|e| e == "jsonl")
                && let Some(s) = describe(&p)
            {
                out.push(s);
            }
        }
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
    out
}

pub fn find_session(id: &str) -> Option<FoundSession> {
    list_sessions().into_iter().find(|s| s.id == id)
}

pub fn copy_into(s: &FoundSession, cwd: &Path) -> Result<()> {
    let from = s.path.parent().context("no session folder")?;
    let to = session_base(cwd);
    if from == to {
        return Ok(());
    }
    std::fs::create_dir_all(&to)?;
    std::fs::copy(&s.path, to.join(format!("{}.jsonl", s.id)))?;
    let extra = from.join(&s.id);
    if extra.is_dir() {
        let mut files = vec![];
        crate::sync::walk(&extra, &mut files);
        for f in files {
            let rel = f.strip_prefix(from)?;
            let dest = to.join(rel);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&f, dest)?;
        }
    }
    Ok(())
}

fn ts_of(v: &Value) -> i64 {
    v["timestamp"].as_str().and_then(utc_ms).unwrap_or(0)
}

fn utc_ms(t: &str) -> Option<i64> {
    let n = |a: usize, b: usize| t.get(a..b)?.parse::<i64>().ok();
    let (y, mo, d) = (n(0, 4)?, n(5, 7)?, n(8, 10)?);
    let (h, mi, sec) = (n(11, 13)?, n(14, 16)?, n(17, 19)?);
    let ms = if t.as_bytes().get(19) == Some(&b'.') {
        let frac: String = t[20..].chars().take_while(|c| c.is_ascii_digit()).collect();
        format!("{frac:0<3}")[..3].parse::<i64>().ok()?
    } else {
        0
    };
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(((days * 24 + h) * 60 + mi) * 60_000 + sec * 1000 + ms)
}

pub fn history(s: &FoundSession) -> Result<Vec<(i64, Event)>> {
    let text = std::fs::read_to_string(&s.path)?;
    let lines: Vec<Value> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let mut results: HashMap<String, (String, bool)> = HashMap::new();
    for v in &lines {
        if v["type"] != "user" {
            continue;
        }
        if let Value::Array(parts) = &v["message"]["content"] {
            for p in parts.iter().filter(|p| p["type"] == "tool_result") {
                let out = match &p["content"] {
                    Value::String(s) => s.clone(),
                    Value::Array(a) => a
                        .iter()
                        .filter_map(|x| x["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                    _ => String::new(),
                };
                if let Some(id) = p["tool_use_id"].as_str() {
                    results.insert(id.to_string(), (out, p["is_error"] == true));
                }
            }
        }
    }
    let mut events = vec![];
    let mut turn: Option<String> = None;
    let mut last_ts = 0;
    let end = |events: &mut Vec<(i64, Event)>, turn: &mut Option<String>, ts: i64| {
        if let Some(t) = turn.take() {
            events.push((
                ts,
                Event::TurnCompleted {
                    turn: t,
                    outcome: TurnOutcome::Completed,
                    usage: None,
                    error: None,
                    anchor: None,
                    resets_at: None,
                },
            ));
        }
    };
    for v in &lines {
        if v["isSidechain"] == true {
            continue;
        }
        let ts = ts_of(v);
        if ts > 0 {
            last_ts = ts;
        }
        if v["type"] == "system" && v["subtype"] == "compact_boundary" {
            events.push((
                last_ts,
                Event::Compacted {
                    trigger: None,
                    pre_tokens: None,
                },
            ));
            continue;
        }
        if let Some(prompt) = prompt_text(v) {
            end(&mut events, &mut turn, last_ts);
            let id = uuid::Uuid::now_v7().to_string();
            let input = vec![InputPart::Text { text: prompt }];
            events.push((
                last_ts,
                Event::TurnStarted {
                    turn: id.clone(),
                    input: input.clone(),
                    settings: AgentSettings::default(),
                },
            ));
            events.push((
                last_ts,
                Event::ItemCompleted {
                    turn: id.clone(),
                    item: Item {
                        id: format!("{id}:input"),
                        parent: None,
                        body: ItemBody::UserMessage { content: input },
                    },
                },
            ));
            turn = Some(id);
            continue;
        }
        if v["type"] != "assistant" {
            continue;
        }
        let Some(t) = turn.clone() else {
            continue;
        };
        let Value::Array(parts) = &v["message"]["content"] else {
            continue;
        };
        let base = v["uuid"].as_str().unwrap_or("").to_string();
        for (i, p) in parts.iter().enumerate() {
            let body = match p["type"].as_str() {
                Some("text") => {
                    let text = p["text"].as_str().unwrap_or("").to_string();
                    if text.trim().is_empty() {
                        continue;
                    }
                    ItemBody::AgentMessage { text }
                }
                Some("tool_use") => {
                    let id = p["id"].as_str().unwrap_or("");
                    let output = results.get(id).map(|(text, is_error)| ToolOutput {
                        text: text.clone(),
                        is_error: *is_error,
                        images: vec![],
                    });
                    let status = match &output {
                        Some(o) if o.is_error => ToolStatus::Failed,
                        _ => ToolStatus::Completed,
                    };
                    ItemBody::ToolCall {
                        name: p["name"].as_str().unwrap_or("tool").to_string(),
                        input: p["input"].clone(),
                        output,
                        status,
                    }
                }
                _ => continue,
            };
            let id = match p["id"].as_str() {
                Some(id) => id.to_string(),
                None => format!("{base}:{i}"),
            };
            events.push((
                last_ts,
                Event::ItemCompleted {
                    turn: t.clone(),
                    item: Item {
                        id,
                        parent: None,
                        body,
                    },
                },
            ));
        }
    }
    end(&mut events, &mut turn, last_ts);
    Ok(events)
}
