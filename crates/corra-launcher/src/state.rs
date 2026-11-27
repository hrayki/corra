use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

pub const UP_WITHIN: Duration = Duration::from_secs(180);
pub const STAY_UP: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub current: Option<String>,
    #[serde(default)]
    pub previous: Option<String>,
    #[serde(default)]
    pub trial: bool,
    #[serde(default)]
    pub failed: Option<Failure>,
    #[serde(default)]
    pub schemas: BTreeMap<String, i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Failure {
    pub version: String,
    pub reason: String,
    pub at: i64,
    #[serde(default)]
    pub log: Vec<String>,
    #[serde(default)]
    pub newer_database: bool,
}

pub fn valid_version(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 64
        && v.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

impl State {
    pub fn start_with(&mut self, newest: &str) {
        if self.current.is_none() {
            self.current = Some(newest.to_string());
            self.previous = None;
            self.trial = true;
        }
    }

    pub fn switch(&mut self, to: &str) {
        let from = self.current.take();
        if !self.trial || self.previous.is_none() {
            self.previous = from;
        }
        self.current = Some(to.to_string());
        self.trial = true;
        if self.previous.as_deref() == Some(to) {
            self.previous = None;
        }
    }

    pub fn confirm(&mut self, version: &str) -> bool {
        if self.current.as_deref() != Some(version) || !self.trial {
            return false;
        }
        self.trial = false;
        self.failed = None;
        true
    }

    pub fn fail(&mut self, version: &str, reason: String, log: Vec<String>, at: i64) -> bool {
        if self.current.as_deref() != Some(version) || !self.trial {
            return false;
        }
        self.failed = Some(Failure {
            version: version.to_string(),
            reason,
            at,
            log,
            newer_database: false,
        });
        if let Some(p) = self.previous.take() {
            self.current = Some(p);
        }
        self.trial = false;
        true
    }

    pub fn refused(&mut self, version: &str, reason: String, log: Vec<String>, at: i64) -> bool {
        if self.current.as_deref() != Some(version) {
            return false;
        }
        let left = self
            .failed
            .as_ref()
            .filter(|f| !f.newer_database)
            .map(|f| f.version.clone());
        let back = if self.trial {
            self.previous.take()
        } else {
            left.or(self.previous.take())
        };
        self.failed = Some(Failure {
            version: version.to_string(),
            reason,
            at,
            log,
            newer_database: true,
        });
        self.trial = false;
        self.previous = None;
        match back.filter(|b| b != version) {
            Some(b) => {
                self.current = Some(b);
                true
            }
            None => false,
        }
    }

    pub fn saw_schema(&mut self, version: &str, schema: i64) {
        self.schemas.insert(version.to_string(), schema);
        let keep = self.keep();
        self.schemas.retain(|v, _| keep.contains(v));
    }

    pub fn crosses_schema(&self, from: &str, to: &str) -> Option<(i64, i64)> {
        match (self.schemas.get(to), self.schemas.get(from)) {
            (Some(&a), Some(&b)) if a < b => Some((a, b)),
            _ => None,
        }
    }

    pub fn keep(&self) -> Vec<String> {
        self.current
            .iter()
            .chain(self.previous.iter())
            .cloned()
            .collect()
    }
}

#[derive(Debug, PartialEq)]
pub enum Verdict {
    Wait,
    Good,
    Bad(String),
}

pub struct Run {
    pub started: Instant,
    pub up: Option<Instant>,
    pub answered_as: Option<String>,
}

impl Run {
    pub fn new(now: Instant) -> Run {
        Run {
            started: now,
            up: None,
            answered_as: None,
        }
    }

    pub fn judge(&self, now: Instant) -> Verdict {
        match self.up {
            Some(u) if now.duration_since(u) >= STAY_UP => Verdict::Good,
            Some(_) => Verdict::Wait,
            None if now.duration_since(self.started) >= UP_WITHIN => {
                let secs = UP_WITHIN.as_secs();
                Verdict::Bad(match self.answered_as.as_deref() {
                    Some("") => format!(
                        "it was still starting, or waiting for the storage lease, after {secs} seconds"
                    ),
                    Some(v) => format!(
                        "it answered /healthz as version {v}, not as itself, for {secs} seconds"
                    ),
                    None => format!("it didn't answer /healthz within {secs} seconds"),
                })
            }
            None => Verdict::Wait,
        }
    }

    pub fn exited(&self, now: Instant, how: &str) -> String {
        match self.up {
            None => format!(
                "it {how} after {} seconds, before it answered /healthz",
                now.duration_since(self.started).as_secs()
            ),
            Some(u) => format!(
                "it {how} {} seconds after it started answering",
                now.duration_since(u).as_secs()
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(current: &str, previous: Option<&str>, trial: bool) -> State {
        State {
            current: Some(current.into()),
            previous: previous.map(Into::into),
            trial,
            ..State::default()
        }
    }

    #[test]
    fn a_first_start_takes_the_newest_on_trial() {
        let mut s = State::default();
        s.start_with("2026.11.26-1200-aaaaaaa");
        assert_eq!(s, st("2026.11.26-1200-aaaaaaa", None, true));
        s.start_with("2026.11.27-1200-bbbbbbb");
        assert_eq!(s.current.as_deref(), Some("2026.11.26-1200-aaaaaaa"));
    }

    #[test]
    fn an_update_remembers_the_version_it_replaced() {
        let mut s = st("a", None, false);
        s.switch("b");
        assert_eq!(s, st("b", Some("a"), true));
        assert!(s.confirm("b"));
        assert_eq!(s, st("b", Some("a"), false));
    }

    #[test]
    fn switching_again_mid_trial_keeps_the_last_good_version() {
        let mut s = st("a", None, false);
        s.switch("b");
        s.switch("c");
        assert_eq!(s, st("c", Some("a"), true));
        s.switch("a");
        assert_eq!(s, st("a", None, true));
    }

    #[test]
    fn a_failed_trial_goes_back_and_says_why() {
        let mut s = st("b", Some("a"), true);
        assert!(s.fail("b", "it exited".into(), vec!["boom".into()], 7));
        assert_eq!(s.current.as_deref(), Some("a"));
        assert_eq!(s.previous, None);
        assert!(!s.trial);
        let f = s.failed.clone().unwrap();
        assert_eq!(
            (f.version.as_str(), f.reason.as_str(), f.at),
            ("b", "it exited", 7)
        );
        assert_eq!(f.log, vec!["boom".to_string()]);
        assert!(!s.fail("a", "again".into(), vec![], 8));
        assert_eq!(s.current.as_deref(), Some("a"));
    }

    #[test]
    fn a_failed_first_version_stays_with_nothing_to_go_back_to() {
        let mut s = st("a", None, true);
        assert!(s.fail("a", "it exited".into(), vec![], 1));
        assert_eq!(s.current.as_deref(), Some("a"));
        assert!(!s.trial);
        assert!(s.failed.is_some());
    }

    #[test]
    fn only_the_version_on_trial_is_confirmed_or_failed() {
        let mut s = st("b", Some("a"), true);
        assert!(!s.confirm("a"));
        assert!(!s.fail("a", "x".into(), vec![], 1));
        assert_eq!(s, st("b", Some("a"), true));
    }

    #[test]
    fn a_confirmed_update_clears_the_old_failure() {
        let mut s = st("a", None, false);
        s.failed = Some(Failure {
            version: "b".into(),
            reason: "x".into(),
            at: 1,
            log: vec![],
            newer_database: false,
        });
        s.switch("c");
        assert!(s.failed.is_some());
        s.confirm("c");
        assert_eq!(s.failed, None);
    }

    #[test]
    fn an_update_during_the_first_trial_can_still_go_back() {
        let mut s = State::default();
        s.start_with("a");
        s.switch("b");
        assert_eq!(s, st("b", Some("a"), true));
        assert!(s.fail("b", "it exited".into(), vec![], 1));
        assert_eq!(s.current.as_deref(), Some("a"));
    }

    #[test]
    fn a_rollback_the_database_refuses_returns_to_the_newer_version() {
        let mut s = st("b", Some("a"), false);
        s.switch("a");
        assert_eq!(s, st("a", Some("b"), true));
        assert!(s.refused("a", "newer database".into(), vec![], 1));
        assert_eq!(
            (s.current.as_deref(), s.previous.as_deref(), s.trial),
            (Some("b"), None, false)
        );
        let f = s.failed.clone().unwrap();
        assert!(f.newer_database && f.version == "a");
    }

    #[test]
    fn an_old_version_left_after_a_failed_update_returns_to_the_update() {
        let mut s = st("b", Some("a"), true);
        assert!(s.fail(
            "b",
            "it exited 30 seconds after it started answering".into(),
            vec![],
            1
        ));
        assert_eq!(s.current.as_deref(), Some("a"));
        assert!(s.refused("a", "newer database".into(), vec![], 2));
        assert_eq!((s.current.as_deref(), s.trial), (Some("b"), false));
        assert!(!s.refused("b", "newer database".into(), vec![], 3));
        assert_eq!(s.current.as_deref(), Some("b"));
        assert!(!s.refused("b", "newer database".into(), vec![], 4));
        assert_eq!(s.current.as_deref(), Some("b"));
    }

    #[test]
    fn a_rollback_across_a_schema_change_is_known_before_it_is_tried() {
        let mut s = st("a", None, false);
        s.saw_schema("a", 13);
        s.switch("b");
        s.saw_schema("b", 14);
        s.confirm("b");
        assert_eq!(s.crosses_schema("b", "a"), Some((13, 14)));
        assert_eq!(s.crosses_schema("a", "b"), None);
        assert_eq!(s.crosses_schema("b", "unknown"), None);
        s.switch("c");
        s.saw_schema("c", 14);
        s.confirm("c");
        assert_eq!(s.schemas.keys().collect::<Vec<_>>(), vec!["b", "c"]);
        assert_eq!(s.crosses_schema("c", "b"), None);
    }

    #[test]
    fn old_or_partial_state_files_still_load() {
        let s: State = serde_json::from_str("{}").unwrap();
        assert_eq!(s, State::default());
        let s: State = serde_json::from_str(r#"{"current":"a","extra":1}"#).unwrap();
        assert_eq!(s.current.as_deref(), Some("a"));
    }

    #[test]
    fn versions_are_plain_names() {
        assert!(valid_version("2026.11.26-1432-6793d3e"));
        for bad in ["", "../x", ".hidden", "a/b", "a b", &"x".repeat(65)] {
            assert!(!valid_version(bad), "{bad}");
        }
    }

    #[test]
    fn the_rollback_rule() {
        let t0 = Instant::now();
        let mut r = Run::new(t0);
        assert_eq!(r.judge(t0 + Duration::from_secs(10)), Verdict::Wait);
        assert!(matches!(r.judge(t0 + UP_WITHIN), Verdict::Bad(m) if m.contains("didn't answer")));
        r.answered_as = Some(String::new());
        assert!(matches!(r.judge(t0 + UP_WITHIN), Verdict::Bad(m) if m.contains("still starting")));
        r.answered_as = Some("other".into());
        assert!(
            matches!(r.judge(t0 + UP_WITHIN), Verdict::Bad(m) if m.contains("as version other"))
        );
        r.up = Some(t0 + Duration::from_secs(20));
        assert_eq!(r.judge(t0 + Duration::from_secs(70)), Verdict::Wait);
        assert_eq!(
            r.judge(t0 + Duration::from_secs(20) + STAY_UP),
            Verdict::Good
        );
        let why = r.exited(t0 + Duration::from_secs(50), "exited with status 1");
        assert_eq!(
            why,
            "it exited with status 1 30 seconds after it started answering"
        );
        let early = Run::new(t0).exited(t0 + Duration::from_secs(2), "was killed by signal 9");
        assert!(early.contains("before it answered"));
    }
}
