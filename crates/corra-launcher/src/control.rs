use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Ask {
    Status,
    Switch { version: String },
}

pub enum Event {
    Ask(Ask, Sender<Value>),
    Fetched(String, std::result::Result<(), String>),
}

pub fn serve(path: &Path, tx: Sender<Event>) -> Result<()> {
    let _ = std::fs::remove_file(path);
    let listener =
        UnixListener::bind(path).with_context(|| format!("listening on {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let tx = tx.clone();
            std::thread::spawn(move || answer(conn, tx));
        }
    });
    Ok(())
}

fn answer(conn: UnixStream, tx: Sender<Event>) {
    let _ = conn.set_read_timeout(Some(Duration::from_secs(10)));
    let mut line = String::new();
    let Ok(read) = conn.try_clone() else { return };
    if BufReader::new(read.take(16 << 10))
        .read_line(&mut line)
        .is_err()
    {
        return;
    }
    let reply = match serde_json::from_str::<Ask>(line.trim()) {
        Err(e) => json!({ "error": format!("not a request: {e}") }),
        Ok(ask) => {
            let (rtx, rrx) = mpsc::channel();
            if tx.send(Event::Ask(ask, rtx)).is_err() {
                return;
            }
            rrx.recv_timeout(Duration::from_secs(900))
                .unwrap_or_else(|_| json!({ "error": "the launcher didn't answer" }))
        }
    };
    let mut conn = conn;
    let _ = conn.set_write_timeout(Some(Duration::from_secs(10)));
    let _ = writeln!(conn, "{reply}");
}
