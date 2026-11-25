use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use corra_proto::runner::RunnerMsg;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

const SCROLLBACK: usize = 256 * 1024;

enum Cmd {
    Input(Vec<u8>),
    Resize(u16, u16),
    Close,
}

struct Term {
    cmds: mpsc::UnboundedSender<Cmd>,
    scrollback: Arc<Mutex<VecDeque<u8>>>,
}

#[derive(Clone, Default)]
pub struct Terms {
    map: Arc<Mutex<HashMap<String, Term>>>,
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
        out: mpsc::UnboundedSender<RunnerMsg>,
    ) -> Result<()> {
        if self.map.lock().unwrap().contains_key(&id) {
            return Ok(());
        }
        let (pty, pts) = pty_process::open()?;
        pty.resize(pty_process::Size::new(rows.max(2), cols.max(2)))?;
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "bash".into());
        let mut child = pty_process::Command::new(&shell)
            .arg("-l")
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
        let sb = scrollback.clone();
        let out2 = out.clone();
        let id2 = id.clone();
        tokio::spawn(async move {
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
                        let _ = out2.send(RunnerMsg::TermOutput {
                            term: id2.clone(),
                            data: STANDARD.encode(&buf[..n]),
                        });
                    }
                }
            }
        });
        let map = self.map.clone();
        let id3 = id.clone();
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
            map.lock().unwrap().remove(&id3);
            let _ = out.send(RunnerMsg::TermExit { term: id3 });
        });
        self.map.lock().unwrap().insert(
            id,
            Term {
                cmds: tx,
                scrollback,
            },
        );
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
