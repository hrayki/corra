use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use corra_proto::runner::RunnerMsg;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};

const SCROLLBACK: usize = 256 * 1024;
const MAX_INFLIGHT_CHUNKS: usize = 64;
const DRAIN_AFTER_EXIT: Duration = Duration::from_millis(500);

pub type OutItem = (RunnerMsg, Option<OwnedSemaphorePermit>);

enum Cmd {
    Input(Vec<u8>),
    Resize(u16, u16),
    Close,
}

struct Term {
    cmds: mpsc::UnboundedSender<Cmd>,
    scrollback: Arc<Mutex<VecDeque<u8>>>,
}

#[derive(Clone)]
pub struct Terms {
    map: Arc<Mutex<HashMap<String, Term>>>,
    permits: Arc<Semaphore>,
}

impl Default for Terms {
    fn default() -> Self {
        Terms {
            map: Default::default(),
            permits: Arc::new(Semaphore::new(MAX_INFLIGHT_CHUNKS)),
        }
    }
}

impl Terms {
    pub fn is_empty(&self) -> bool {
        self.map.lock().unwrap().is_empty()
    }

    pub fn open(
        &self,
        id: String,
        cwd: &Path,
        cols: u16,
        rows: u16,
        out: mpsc::UnboundedSender<OutItem>,
    ) -> Result<()> {
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "bash".into());
        self.open_command(id, &shell, &["-l"], cwd, (cols, rows), out)
    }

    pub fn open_command(
        &self,
        id: String,
        program: &str,
        args: &[&str],
        cwd: &Path,
        (cols, rows): (u16, u16),
        out: mpsc::UnboundedSender<OutItem>,
    ) -> Result<()> {
        if self.map.lock().unwrap().contains_key(&id) {
            return Ok(());
        }
        let (pty, pts) = pty_process::open()?;
        pty.resize(pty_process::Size::new(rows.max(2), cols.max(2)))?;
        let mut child = pty_process::Command::new(program)
            .args(args)
            .current_dir(cwd)
            .env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor")
            .env_remove("CORRA_TOKEN")
            .env_remove("CORRA_TOKEN_FILE")
            .kill_on_drop(true)
            .spawn(pts)?;
        let (mut reader, mut writer) = pty.into_split();
        let (tx, mut rx) = mpsc::unbounded_channel::<Cmd>();
        let scrollback: Arc<Mutex<VecDeque<u8>>> = Default::default();
        self.map.lock().unwrap().insert(
            id.clone(),
            Term {
                cmds: tx,
                scrollback: scrollback.clone(),
            },
        );
        let sb = scrollback;
        let out2 = out.clone();
        let id2 = id.clone();
        let permits = self.permits.clone();
        let mut reader_task = tokio::spawn(async move {
            let mut buf = vec![0u8; 16 * 1024];
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        {
                            let mut s = sb.lock().unwrap();
                            s.extend(&buf[..n]);
                            let over = s.len().saturating_sub(SCROLLBACK);
                            s.drain(..over);
                        }
                        let Ok(permit) = permits.clone().acquire_owned().await else {
                            break;
                        };
                        if out2
                            .send((
                                RunnerMsg::TermOutput {
                                    term: id2.clone(),
                                    data: STANDARD.encode(&buf[..n]),
                                },
                                Some(permit),
                            ))
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        });
        let map = self.map.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    cmd = rx.recv() => match cmd {
                        Some(Cmd::Input(d)) => { if writer.write_all(&d).await.is_err() { break } }
                        Some(Cmd::Resize(c, r)) => { let _ = writer.resize(pty_process::Size::new(r.max(2), c.max(2))); }
                        Some(Cmd::Close) | None => { let _ = child.kill().await; break }
                    },
                    status = child.wait() => {
                        let _ = status;
                        break;
                    }
                }
            }
            drop(writer);
            if tokio::time::timeout(DRAIN_AFTER_EXIT, &mut reader_task)
                .await
                .is_err()
            {
                reader_task.abort();
            }
            map.lock().unwrap().remove(&id);
            let _ = out.send((RunnerMsg::TermExit { term: id }, None));
        });
        Ok(())
    }

    pub fn scrollback(&self, id: &str) -> Result<String> {
        let map = self.map.lock().unwrap();
        let t = map
            .get(id)
            .ok_or_else(|| anyhow!("that terminal has closed"))?;
        let s = t.scrollback.lock().unwrap();
        let (a, b) = s.as_slices();
        let mut v = Vec::with_capacity(a.len() + b.len());
        v.extend_from_slice(a);
        v.extend_from_slice(b);
        Ok(STANDARD.encode(v))
    }

    pub fn input(&self, id: &str, data: &str) {
        if let Some(t) = self.map.lock().unwrap().get(id) {
            let _ = t.cmds.send(Cmd::Input(data.as_bytes().to_vec()));
        }
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) {
        if let Some(t) = self.map.lock().unwrap().get(id) {
            let _ = t.cmds.send(Cmd::Resize(cols, rows));
        }
    }

    pub fn close(&self, id: &str) {
        if let Some(t) = self.map.lock().unwrap().get(id) {
            let _ = t.cmds.send(Cmd::Close);
        }
    }
}
