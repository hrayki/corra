use std::io::Write;
use std::time::Instant;

use corra_proto::{ThreadId, now_ms};
use serde_json::{Value, json};

const PRIVATE: &[&str] = &[
    "isUsingOverage",
    "overageStatus",
    "overageDisabledReason",
    "overageResetsAt",
    "memory_paths",
    "messaging_socket_path",
    "email",
    "account",
];

pub fn scrub(v: &mut Value) {
    match v {
        Value::Object(o) => {
            o.retain(|k, _| !PRIVATE.contains(&k.as_str()));
            for (k, x) in o.iter_mut() {
                if k == "utilization" && x.is_number() {
                    *x = json!(0);
                } else {
                    scrub(x);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(scrub),
        _ => {}
    }
}

pub fn scrub_file(text: &str) -> String {
    text.lines()
        .map(|l| match serde_json::from_str::<Value>(l) {
            Ok(mut v) => {
                scrub(&mut v);
                v.to_string()
            }
            Err(_) => l.to_string(),
        })
        .map(|l| l + "\n")
        .collect()
}

pub fn dir() -> std::path::PathBuf {
    crate::files::home().join(".corra/recordings")
}

pub struct Recorder {
    file: std::fs::File,
    start: Instant,
}

impl Recorder {
    pub fn open(thread: ThreadId, start: Value) -> Option<Recorder> {
        let dir = dir();
        if !dir.is_dir() {
            return None;
        }
        let path = dir.join(format!("{thread}-{}.ndjson", now_ms()));
        let mut opts = std::fs::OpenOptions::new();
        opts.create_new(true).append(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let file = match opts.open(&path) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!("couldn't record to {}: {e}", path.display());
                return None;
            }
        };
        tracing::info!("recording Claude Code to {}", path.display());
        let mut r = Recorder {
            file,
            start: Instant::now(),
        };
        r.entry("start", start);
        Some(r)
    }

    fn entry(&mut self, from: &str, body: Value) {
        let mut v = json!({ "t": self.start.elapsed().as_millis() as u64, "from": from });
        if let (Some(o), Value::Object(b)) = (v.as_object_mut(), body) {
            o.extend(b);
        }
        let mut line = v.to_string();
        line.push('\n');
        let _ = self.file.write_all(line.as_bytes());
    }

    pub fn claude(&mut self, line: &str) {
        match serde_json::from_str::<Value>(line) {
            Ok(msg) => self.entry("claude", json!({ "msg": msg })),
            Err(_) => self.entry("claude", json!({ "line": line })),
        }
    }

    pub fn runner(&mut self, msg: &Value) {
        self.entry("runner", json!({ "msg": msg }));
    }

    pub fn app(&mut self, cmd: &super::SessionCmd, initial: bool) {
        let mut body = json!({ "cmd": cmd });
        if initial {
            body["initial"] = json!(true);
        }
        self.entry("app", body);
    }

    pub fn exit(&mut self, code: Option<i32>) {
        self.entry("exit", json!({ "code": code }));
    }
}
