use std::collections::VecDeque;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

use super::record::Recorder;

pub type Reader = Box<dyn tokio::io::AsyncRead + Unpin + Send>;
pub type Writer = Box<dyn tokio::io::AsyncWrite + Unpin + Send>;

const STDERR_LINES: usize = 15;
const STDERR_LINE_CHARS: usize = 400;

pub struct JsonlChild {
    child: Option<Child>,
    stdin: Writer,
    lines: tokio::io::Lines<BufReader<Reader>>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    stderr_reader: Option<tokio::task::JoinHandle<()>>,
    pub rec: Option<Recorder>,
}

impl JsonlChild {
    pub fn spawn(
        mut cmd: Command,
        label: &str,
        log_target: &'static str,
        rec: Option<Recorder>,
    ) -> Result<Self> {
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(target_os = "linux")]
        unsafe {
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                Ok(())
            });
        }
        let mut child = cmd.spawn().with_context(|| format!("starting {label}"))?;
        let stdin: Writer = Box::new(child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?);
        let stdout: Reader = Box::new(child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?);
        let tail = Arc::new(Mutex::new(VecDeque::new()));
        let stderr_reader = child.stderr.take().map(|stderr| {
            let tail = tail.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    tracing::debug!(target: "corra_runner::agent", agent = log_target, "stderr: {l}");
                    if l.trim().is_empty() {
                        continue;
                    }
                    let mut t = tail.lock().unwrap();
                    if t.len() == STDERR_LINES {
                        t.pop_front();
                    }
                    t.push_back(l.chars().take(STDERR_LINE_CHARS).collect());
                }
            })
        });
        Ok(JsonlChild {
            child: Some(child),
            stdin,
            lines: BufReader::new(stdout).lines(),
            stderr: tail,
            stderr_reader,
            rec,
        })
    }

    #[cfg(test)]
    pub fn piped(reader: Reader, writer: Writer) -> Self {
        JsonlChild {
            child: None,
            stdin: writer,
            lines: BufReader::new(reader).lines(),
            stderr: Default::default(),
            stderr_reader: None,
            rec: None,
        }
    }

    pub async fn next_line(&mut self) -> Result<Option<String>> {
        Ok(self.lines.next_line().await?)
    }

    pub async fn write_json(&mut self, v: &Value) -> Result<()> {
        if let Some(r) = &mut self.rec {
            r.runner(v);
        }
        let mut line = serde_json::to_vec(v)?;
        line.push(b'\n');
        self.stdin.write_all(&line).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    pub async fn wait(&mut self) -> Result<std::process::ExitStatus> {
        match &mut self.child {
            Some(c) => Ok(c.wait().await?),
            None => {
                use std::os::unix::process::ExitStatusExt;
                Ok(std::process::ExitStatus::from_raw(0))
            }
        }
    }

    pub async fn kill(&mut self) {
        if let Some(c) = &mut self.child {
            let _ = c.kill().await;
        }
    }

    pub async fn stderr_tail(&mut self) -> String {
        if let Some(reader) = self.stderr_reader.take() {
            let _ = tokio::time::timeout(Duration::from_millis(500), reader).await;
        }
        self.stderr
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub fn with_output(what: String, tail: &str) -> String {
    if tail.is_empty() || what.contains(tail) {
        what
    } else {
        format!("{what}. The end of its output:\n\n```\n{tail}\n```")
    }
}
