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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Failure {
    pub version: String,
    pub reason: String,
    pub at: i64,
    #[serde(default)]
    pub log: Vec<String>,
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
        if !self.trial {
            self.previous = self.current.take();
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
        });
        if let Some(p) = self.previous.take() {
            self.current = Some(p);
        }
        self.trial = false;
        true
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
            failed: None,
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
        });
        s.switch("c");
        assert!(s.failed.is_some());
        s.confirm("c");
        assert_eq!(s.failed, None);
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
