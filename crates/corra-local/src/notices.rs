use std::time::Duration;

use corra_hub::hub::Outbound;
use corra_proto::app::HubToApp;
use futures_util::StreamExt;
use serde_json::Value;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest, http::HeaderValue};

use crate::StateRef;

pub(crate) fn spawn(s: StateRef, tx: broadcast::Sender<Value>) {
    let mine = corra_hub::hub::hash_token(&s.session);
    let mut local = s.hub.apps.subscribe();
    let ltx = tx.clone();
    tokio::spawn(async move {
        loop {
            match local.recv().await {
                Ok(out) => match &*out {
                    Outbound::All(HubToApp::Notify { notice }) => {
                        let _ = ltx.send(notice.clone());
                    }
                    Outbound::Session(sess, HubToApp::Notify { notice }) if *sess == mine => {
                        let _ = ltx.send(notice.clone());
                    }
                    _ => {}
                },
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    tokio::spawn(async move {
        loop {
            match s.cfg() {
                Some(cfg) => {
                    if let Err(e) = listen(&cfg.url, &cfg.token, &tx).await {
                        tracing::debug!("not listening to the hub for notifications: {e:#}");
                    }
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
                None => {
                    tokio::select! {
                        _ = s.wake.notified() => {}
                        _ = tokio::time::sleep(Duration::from_secs(30)) => {}
                    }
                }
            }
        }
    });
}

async fn listen(url: &str, token: &str, tx: &broadcast::Sender<Value>) -> anyhow::Result<()> {
    let ws = format!("{}/app", url.trim_end_matches('/'))
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1);
    let mut req = ws.into_client_request()?;
    req.headers_mut().insert(
        "sec-websocket-protocol",
        HeaderValue::from_str(&format!("corra.v1, {token}"))?,
    );
    let (mut stream, _) = tokio_tungstenite::connect_async(req).await?;
    while let Some(msg) = stream.next().await {
        let text = match msg? {
            tungstenite::Message::Text(t) => t,
            tungstenite::Message::Close(_) => break,
            _ => continue,
        };
        if !text.contains("\"notify\"") {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(&text)
            && v["t"] == "notify"
        {
            let _ = tx.send(v["notice"].clone());
        }
    }
    Ok(())
}
