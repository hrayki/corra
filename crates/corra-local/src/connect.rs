use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::StateRef;

const PAGE: &str = include_str!("connect.html");
const SCRIPT: &str = include_str!("connect.js");

pub fn router(state: StateRef) -> Router {
    Router::new()
        .route("/desktop/connect", get(page).post(connect))
        .route(
            "/desktop/connect.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    SCRIPT,
                )
            }),
        )
        .with_state(state)
}

#[derive(Deserialize)]
struct Key {
    k: String,
}

async fn page(State(s): State<StateRef>, Query(q): Query<Key>) -> Response {
    if q.k != s.enter_key {
        return (StatusCode::FORBIDDEN, "Open corra from the app.").into_response();
    }
    let current = s.cfg().map(|c| c.url).unwrap_or_default();
    let page = PAGE
        .replace("{{current}}", &html_escape(&current))
        .replace("{{host}}", &html_escape(&crate::hostname()));
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        page,
    )
        .into_response()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[derive(Deserialize)]
struct Connect {
    url: String,
    #[serde(default)]
    key: String,
    #[serde(default)]
    code: String,
}

async fn connect(
    State(s): State<StateRef>,
    Query(q): Query<Key>,
    Json(body): Json<Connect>,
) -> Response {
    if q.k != s.enter_key {
        return StatusCode::FORBIDDEN.into_response();
    }
    match crate::desktop::sign_in(&s, &body.url, &body.key, &body.code).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("{e:#}") })),
        )
            .into_response(),
    }
}
