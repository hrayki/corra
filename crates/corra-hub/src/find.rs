use std::collections::{HashMap, HashSet};

use anyhow::Result;
use corra_proto::app::{FindHit, Found};
use corra_proto::{Event, InputPart, ItemBody, ThreadId};

use crate::hub::Hub;

pub const LIMIT: u32 = 1000;
const NOTE: &str = "[corra] ";

pub fn fold(s: &str) -> String {
    s.chars().flat_map(char::to_lowercase).collect()
}

fn typed(input: &[InputPart]) -> String {
    input
        .iter()
        .filter_map(|p| match p {
            InputPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn turn_of(e: &Event) -> Option<&str> {
    match e {
        Event::TurnStarted { turn, .. }
        | Event::ItemStarted { turn, .. }
        | Event::ItemCompleted { turn, .. }
        | Event::Request { turn, .. }
        | Event::TurnCompleted { turn, .. }
        | Event::Checkpoint { turn, .. } => Some(turn),
        _ => None,
    }
}

struct Shown {
    turn: String,
    start: i64,
    seq: i64,
    text: String,
    gone: bool,
}

impl Hub {
    pub fn find(&self, thread: ThreadId, query: &str) -> Result<Found> {
        let needle = fold(query);
        if needle.is_empty() {
            return Ok(Found::default());
        }
        let mut shown: Vec<Shown> = vec![];
        let mut items: HashMap<String, Vec<usize>> = HashMap::new();
        let mut asked: HashSet<String> = HashSet::new();
        let mut starts: HashMap<String, i64> = HashMap::new();
        for e in self.db.messages(thread)? {
            match e.event {
                Event::TurnStarted { turn, input, .. } => {
                    let mut text = typed(&input);
                    if input.len() == 1
                        && let Some(note) = text.strip_prefix(NOTE)
                    {
                        text = note.to_string();
                    }
                    if !input.is_empty() {
                        asked.insert(turn.clone());
                    }
                    starts.insert(turn.clone(), e.seq);
                    shown.push(Shown {
                        turn,
                        start: e.seq,
                        seq: e.seq,
                        text,
                        gone: false,
                    });
                }
                Event::ItemCompleted { turn, item } if item.parent.is_none() => {
                    let text = match item.body {
                        ItemBody::UserMessage { content } => typed(&content),
                        ItemBody::AgentMessage { text } => text,
                        _ => continue,
                    };
                    let same = items.entry(item.id.clone()).or_default();
                    if item.id == format!("{turn}:input") {
                        for &i in same.iter() {
                            if shown[i].turn != turn {
                                shown[i].gone = true;
                            }
                        }
                        if asked.contains(&turn) {
                            continue;
                        }
                    }
                    match same
                        .iter()
                        .find(|&&i| shown[i].turn == turn && !shown[i].gone)
                    {
                        Some(&i) => shown[i].text = text,
                        None => {
                            same.push(shown.len());
                            shown.push(Shown {
                                start: starts.get(&turn).copied().unwrap_or(e.seq),
                                turn,
                                seq: e.seq,
                                text,
                                gone: false,
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        shown.sort_by_key(|s| (s.start, s.seq));
        let mut found = Found::default();
        let mut total = 0;
        for s in shown.iter().rev().filter(|s| !s.gone) {
            let n = fold(&s.text).matches(&needle).count() as u32;
            if n == 0 {
                continue;
            }
            if total >= LIMIT {
                found.capped = true;
                break;
            }
            total += n;
            found.hits.push(FindHit { seq: s.seq, n });
        }
        found.hits.reverse();
        Ok(found)
    }

    pub fn turns_down_to(&self, thread: ThreadId, before: i64, to: i64) -> Result<i64> {
        let held = self.db.event_at(thread, to)?;
        let start = match held.as_ref().and_then(|e| turn_of(&e.event)) {
            Some(turn) => self.db.turn_start(thread, turn, to)?,
            None => None,
        };
        let start = match start {
            Some(s) => s,
            None => self
                .db
                .turn_starts_before(thread, to + 1, 1)?
                .first()
                .copied()
                .unwrap_or(0),
        };
        self.db.turns_between(thread, start, before)
    }
}
