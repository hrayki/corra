use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use corra_proto::{AgentKind, AgentUsage, UsageWindow, now_ms};
use tokio::sync::{mpsc, oneshot};

use super::AgentOut;

const DEFAULT_UPSTREAM: &str = "https://api.anthropic.com";

pub struct ClaudeProxy {
    url: String,
    stop: Option<oneshot::Sender<()>>,
}

impl ClaudeProxy {
    pub fn url(&self) -> &str {
        &self.url
    }

    pub async fn start_default(out: mpsc::UnboundedSender<AgentOut>) -> Result<Self> {
        Self::start(DEFAULT_UPSTREAM.to_string(), out).await
    }

    pub async fn start(upstream: String, out: mpsc::UnboundedSender<AgentOut>) -> Result<Self> {
        let client = reqwest::Client::builder()
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .build()?;
        let state = Arc::new(ProxyState {
            upstream: upstream.trim_end_matches('/').to_string(),
            client,
            out,
        });
        let app = Router::new().fallback(forward).with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (tx, rx) = oneshot::channel::<()>();
        tokio::spawn(async move {
            let server = axum::serve(listener, app).with_graceful_shutdown(async {
                let _ = rx.await;
            });
            let _ = server.await;
        });
        Ok(Self {
            url: format!("http://127.0.0.1:{}", addr.port()),
            stop: Some(tx),
        })
    }
}

impl Drop for ClaudeProxy {
    fn drop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
    }
}

struct ProxyState {
    upstream: String,
    client: reqwest::Client,
    out: mpsc::UnboundedSender<AgentOut>,
}

fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name,
        "host"
            | "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailers"
            | "transfer-encoding"
            | "upgrade"
    )
}

async fn forward(State(st): State<Arc<ProxyState>>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");
    let target = format!("{}{}", st.upstream, path_and_query);
    let method = match reqwest::Method::from_bytes(parts.method.as_str().as_bytes()) {
        Ok(m) => m,
        Err(_) => {
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Body::empty())
                .unwrap();
        }
    };
    let mut builder = st.client.request(method, &target);
    for (name, value) in &parts.headers {
        if is_hop_by_hop(name.as_str()) {
            continue;
        }
        builder = builder.header(name.as_str(), value.as_bytes());
    }
    builder = builder.body(reqwest::Body::wrap_stream(body.into_data_stream()));
    let resp = match builder.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("claude proxy request failed: {e}");
            return Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .body(Body::from(format!("upstream error: {e}")))
                .unwrap();
        }
    };
    let windows = parse_rate_limit_headers(resp.headers());
    if !windows.is_empty() {
        let _ = st.out.send(AgentOut::Usage(AgentUsage {
            agent: AgentKind::Claude,
            updated_at: now_ms(),
            windows,
            error: None,
            refreshable: false,
        }));
    }
    let status =
        StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut out = Response::builder().status(status);
    for (name, value) in resp.headers() {
        if is_hop_by_hop(name.as_str()) {
            continue;
        }
        out = out.header(name.as_str(), value.as_bytes());
    }
    out.body(Body::from_stream(resp.bytes_stream()))
        .unwrap_or_else(|_| {
            Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .body(Body::empty())
                .unwrap()
        })
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok().map(str::trim)
}

fn header_pct(headers: &HeaderMap, names: &[&str]) -> Option<f64> {
    for name in names {
        if let Some(v) = header_str(headers, name)
            && let Ok(n) = v.parse::<f64>()
            && n.is_finite()
        {
            let pct = if n <= 1.0 { n * 100.0 } else { n };
            return Some(pct.clamp(0.0, 100.0));
        }
    }
    None
}

fn header_reset_ms(headers: &HeaderMap, name: &str) -> Option<i64> {
    let raw = header_str(headers, name)?.parse::<i64>().ok()?;
    if raw <= 0 {
        return None;
    }
    Some(if raw < 10_000_000_000 {
        raw * 1000
    } else {
        raw
    })
}

pub fn parse_rate_limit_headers(headers: &HeaderMap) -> Vec<UsageWindow> {
    let specs: &[(&str, &str, &[&str], &str, &str)] = &[
        (
            "five_hour",
            "5-hour",
            &[
                "anthropic-ratelimit-unified-5h-utilization",
                "anthropic-ratelimit-unified-grace-5h-utilization",
            ],
            "anthropic-ratelimit-unified-5h-reset",
            "anthropic-ratelimit-unified-5h-status",
        ),
        (
            "seven_day",
            "Weekly",
            &[
                "anthropic-ratelimit-unified-7d-utilization",
                "anthropic-ratelimit-unified-grace-7d-utilization",
            ],
            "anthropic-ratelimit-unified-7d-reset",
            "anthropic-ratelimit-unified-7d-status",
        ),
    ];
    let overall_rejected =
        header_str(headers, "anthropic-ratelimit-unified-status") == Some("rejected");
    let claim = header_str(headers, "anthropic-ratelimit-unified-representative-claim");
    let mut out = Vec::new();
    for &(key, label, util_names, reset_name, status_name) in specs {
        let mut used_pct = header_pct(headers, util_names);
        let resets_at = header_reset_ms(headers, reset_name);
        let status = header_str(headers, status_name);
        let blocked = status == Some("rejected")
            || (overall_rejected
                && (claim == Some(key)
                    || (claim == Some("5h") && key == "five_hour")
                    || (claim == Some("7d") && key == "seven_day")));
        if blocked && used_pct.is_none_or(|p| p == 0.0) {
            used_pct = Some(100.0);
        }
        if used_pct.is_none() && resets_at.is_none() && !blocked {
            continue;
        }
        out.push(UsageWindow {
            key: key.to_string(),
            label: label.to_string(),
            used_pct,
            resets_at,
            blocked,
            detail: None,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::post;

    #[tokio::test]
    async fn proxy_streams_response_and_extracts_unified_rate_limit_headers() {
        let mock = Router::new().route(
            "/v1/messages",
            post(|| async {
                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "text/event-stream")
                    .header("anthropic-ratelimit-unified-5h-utilization", "0.34")
                    .header("anthropic-ratelimit-unified-5h-reset", "1796000000")
                    .header("anthropic-ratelimit-unified-5h-status", "allowed")
                    .header("anthropic-ratelimit-unified-7d-utilization", "0.77")
                    .header("anthropic-ratelimit-unified-7d-reset", "1796400000")
                    .header("anthropic-ratelimit-unified-7d-status", "allowed_warning")
                    .body(Body::from("data: hello\n\n"))
                    .unwrap()
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mock_addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, mock).await;
        });

        let (tx, mut rx) = mpsc::unbounded_channel();
        let proxy = ClaudeProxy::start(format!("http://127.0.0.1:{}", mock_addr.port()), tx)
            .await
            .unwrap();

        let resp = reqwest::Client::new()
            .post(format!("{}/v1/messages", proxy.url()))
            .header("x-api-key", "test-key")
            .body(r#"{"model":"claude"}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await.unwrap(), "data: hello\n\n");

        let msg = rx.recv().await.unwrap();
        match msg {
            AgentOut::Usage(u) => {
                assert_eq!(u.agent, AgentKind::Claude);
                assert_eq!(u.windows.len(), 2);
                assert_eq!(u.windows[0].key, "five_hour");
                assert_eq!(u.windows[0].used_pct, Some(34.0));
                assert_eq!(u.windows[0].resets_at, Some(1_796_000_000_000));
                assert!(!u.windows[0].blocked);
                assert_eq!(u.windows[1].key, "seven_day");
                assert_eq!(u.windows[1].used_pct, Some(77.0));
                assert_eq!(u.windows[1].resets_at, Some(1_796_400_000_000));
            }
            other => panic!("unexpected AgentOut: {other:?}"),
        }
    }
}
