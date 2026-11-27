use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use corra_proto::{ThreadId, ToolDef};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

use super::HubCall;

pub type CurrentTurn = Arc<Mutex<Option<String>>>;

pub struct McpServer {
    pub path: PathBuf,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_file(&self.path);
    }
}

pub fn serve(
    thread: ThreadId,
    tools: Vec<ToolDef>,
    hub: mpsc::UnboundedSender<HubCall>,
    turn: CurrentTurn,
) -> Result<McpServer> {
    let dir = crate::files::home().join(".corra/mcp");
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let path = dir.join(format!("{thread}.sock"));
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    let tools = Arc::new(tools);
    let task = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(connection(
                stream,
                thread,
                tools.clone(),
                hub.clone(),
                turn.clone(),
            ));
        }
    });
    Ok(McpServer { path, task })
}

async fn connection(
    stream: UnixStream,
    thread: ThreadId,
    tools: Arc<Vec<ToolDef>>,
    hub: mpsc::UnboundedSender<HubCall>,
    turn: CurrentTurn,
) {
    let (read, write) = stream.into_split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        let mut write = write;
        while let Some(v) = rx.recv().await {
            let mut line = v.to_string();
            line.push('\n');
            if write.write_all(line.as_bytes()).await.is_err() {
                break;
            }
        }
    });
    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = msg.get("id").cloned();
        let Some(id) = id.filter(|v| !v.is_null()) else {
            continue;
        };
        let reply = |result: Value| json!({ "jsonrpc": "2.0", "id": id.clone(), "result": result });
        match msg["method"].as_str().unwrap_or("") {
            "initialize" => {
                let version = msg["params"]["protocolVersion"]
                    .as_str()
                    .unwrap_or("2025-06-18");
                let _ = tx.send(reply(json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "corra", "version": env!("CARGO_PKG_VERSION") }
                })));
            }
            "tools/list" => {
                let list: Vec<Value> = tools
                    .iter()
                    .map(|t| json!({ "name": t.mcp_name(), "description": t.description, "inputSchema": t.input_schema }))
                    .collect();
                let _ = tx.send(reply(json!({ "tools": list })));
            }
            "tools/call" => {
                let name = msg["params"]["name"].as_str().unwrap_or_default();
                let args = msg["params"]["arguments"].clone();
                let Some(def) = tools.iter().find(|t| t.mcp_name() == name) else {
                    let _ = tx.send(reply(json!({ "content": [{ "type": "text", "text": format!("unknown tool {name}") }], "isError": true })));
                    continue;
                };
                let (rtx, rrx) = tokio::sync::oneshot::channel();
                let _ = hub.send(HubCall {
                    thread,
                    turn: turn.lock().unwrap().clone().unwrap_or_default(),
                    action: def.action.clone(),
                    input: if args.is_null() { json!({}) } else { args },
                    reply: rtx,
                });
                let tx = tx.clone();
                tokio::spawn(async move {
                    let _ = tx.send(
                        json!({ "jsonrpc": "2.0", "id": id, "result": tool_result(rrx.await) }),
                    );
                });
            }
            "ping" => {
                let _ = tx.send(reply(json!({})));
            }
            other => {
                let _ = tx.send(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("no method {other}") } }));
            }
        }
    }
    drop(tx);
    let _ = writer.await;
}

pub fn tool_result(
    r: Result<Result<Value, String>, tokio::sync::oneshot::error::RecvError>,
) -> Value {
    match r {
        Ok(Ok(v)) => match (v["image"]["data"].as_str(), v["image"]["mime"].as_str()) {
            (Some(data), Some(mime)) => json!({ "content": [
                { "type": "text", "text": v["text"].as_str().unwrap_or_default() },
                { "type": "image", "data": data, "mimeType": mime }
            ] }),
            _ => {
                let text = match &v {
                    Value::String(s) => s.clone(),
                    other => serde_json::to_string_pretty(other).unwrap_or_default(),
                };
                json!({ "content": [{ "type": "text", "text": text }] })
            }
        },
        Ok(Err(e)) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
        Err(_) => {
            json!({ "content": [{ "type": "text", "text": "the call was dropped" }], "isError": true })
        }
    }
}

pub fn shared_mcp_path() -> PathBuf {
    crate::files::home().join(".corra/kit/shared/mcp.json")
}

pub fn acp_shared_servers(path: &std::path::Path) -> Vec<Value> {
    let Ok(bytes) = std::fs::read(path) else {
        return vec![];
    };
    let Ok(v) = serde_json::from_slice::<Value>(&bytes) else {
        return vec![];
    };
    let Some(map) = v["mcpServers"].as_object() else {
        return vec![];
    };
    let mut out = vec![];
    for (name, cfg) in map {
        if name == "corra" {
            continue;
        }
        let Some(command) = cfg["command"].as_str().filter(|s| !s.is_empty()) else {
            continue;
        };
        let args: Vec<Value> = cfg["args"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| a.as_str().map(|s| json!(s)))
            .collect();
        let env: Vec<Value> = cfg["env"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(k, v)| Some(json!({ "name": k, "value": v.as_str()? })))
            .collect();
        out.push(json!({
            "name": name,
            "command": command,
            "args": args,
            "env": env,
        }));
    }
    out
}

pub async fn bridge(path: &str) -> Result<()> {
    let stream = UnixStream::connect(path).await?;
    let (mut read, mut write) = stream.into_split();
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    tokio::select! {
        r = tokio::io::copy(&mut stdin, &mut write) => { r?; }
        r = tokio::io::copy(&mut read, &mut stdout) => { r?; }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acp_shared_servers_converts_claude_format_and_skips_corra() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&json!({
                "mcpServers": {
                    "corra": { "command": "corra-runner", "args": ["mcp"] },
                    "docs": {
                        "command": "npx",
                        "args": ["-y", "docs-mcp"],
                        "env": { "API_KEY": "secret" }
                    },
                    "empty": { "args": [] }
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let servers = acp_shared_servers(&path);
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0]["name"], "docs");
        assert_eq!(servers[0]["command"], "npx");
        assert_eq!(servers[0]["args"], json!(["-y", "docs-mcp"]));
        assert_eq!(
            servers[0]["env"],
            json!([{ "name": "API_KEY", "value": "secret" }])
        );
        assert!(acp_shared_servers(&dir.path().join("missing.json")).is_empty());
    }
}
