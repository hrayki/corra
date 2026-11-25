use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use corra_proto::runner::RunnerMsg;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

type Conns = Arc<Mutex<HashMap<String, mpsc::UnboundedSender<Option<Vec<u8>>>>>>;

#[derive(Clone)]
pub struct Tunnels {
    conns: Conns,
    listening: Arc<Mutex<HashSet<u16>>>,
    out: mpsc::UnboundedSender<RunnerMsg>,
}

impl Tunnels {
    pub fn new(out: mpsc::UnboundedSender<RunnerMsg>) -> Self {
        Tunnels {
            conns: Default::default(),
            listening: Default::default(),
            out,
        }
    }

    pub async fn listen(&self, port: u16) -> Result<()> {
        if !self.listening.lock().unwrap().insert(port) {
            return Ok(());
        }
        let listener = match TcpListener::bind(("127.0.0.1", port)).await {
            Ok(l) => l,
            Err(e) => {
                self.listening.lock().unwrap().remove(&port);
                return Err(e.into());
            }
        };
        let me = self.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let id = uuid::Uuid::now_v7().simple().to_string();
                let _ = me.out.send(RunnerMsg::TunnelOpen {
                    id: id.clone(),
                    port,
                });
                me.pump(id, stream);
            }
        });
        Ok(())
    }

    pub fn open(&self, id: String, port: u16) {
        let me = self.clone();
        tokio::spawn(async move {
            match TcpStream::connect(("127.0.0.1", port)).await {
                Ok(stream) => me.pump(id, stream),
                Err(_) => {
                    let _ = me.out.send(RunnerMsg::TunnelClose { id });
                }
            }
        });
    }

    pub fn data(&self, id: &str, data: &str) {
        let Ok(bytes) = STANDARD.decode(data) else {
            return;
        };
        if let Some(tx) = self.conns.lock().unwrap().get(id) {
            let _ = tx.send(Some(bytes));
        }
    }

    pub fn close(&self, id: &str) {
        if let Some(tx) = self.conns.lock().unwrap().remove(id) {
            let _ = tx.send(None);
        }
    }

    fn pump(&self, id: String, stream: TcpStream) {
        let (tx, mut rx) = mpsc::unbounded_channel::<Option<Vec<u8>>>();
        self.conns.lock().unwrap().insert(id.clone(), tx);
        let (mut reader, mut writer) = stream.into_split();
        tokio::spawn(async move {
            while let Some(Some(bytes)) = rx.recv().await {
                if writer.write_all(&bytes).await.is_err() {
                    break;
                }
            }
            let _ = writer.shutdown().await;
        });
        let me = self.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 32 * 1024];
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let _ = me.out.send(RunnerMsg::TunnelData {
                            id: id.clone(),
                            data: STANDARD.encode(&buf[..n]),
                        });
                    }
                }
            }
            let _ = me.out.send(RunnerMsg::TunnelClose { id: id.clone() });
            me.close(&id);
        });
    }
}
