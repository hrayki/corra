use std::path::{Component, Path, PathBuf};

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use corra_proto::ThreadId;
use serde::Deserialize;
use serde_json::json;

use crate::auth::AppState;

fn machine_of(s: &AppState, headers: &HeaderMap) -> Option<corra_proto::MachineId> {
    s.hub.auth_machine_bearer(headers)
}

fn allowed(s: &AppState, headers: &HeaderMap, thread: ThreadId) -> bool {
    let Some(m) = machine_of(s, headers) else {
        return false;
    };
    matches!(s.hub.db.thread(thread), Ok(Some(t)) if t.machine == Some(m))
}

fn may_read(s: &AppState, headers: &HeaderMap, thread: ThreadId) -> bool {
    let Some(m) = machine_of(s, headers) else {
        return false;
    };
    let Ok(Some(t)) = s.hub.db.thread(thread) else {
        return false;
    };
    if t.machine == Some(m) {
        return true;
    }
    matches!(s.hub.db.machine(m), Ok(Some(mm)) if mm.project == Some(t.project))
}

fn safe_rel(p: &str) -> Option<PathBuf> {
    let path = Path::new(p);
    if p.is_empty() || path.is_absolute() {
        return None;
    }
    for c in path.components() {
        if !matches!(c, Component::Normal(_)) {
            return None;
        }
    }
    Some(path.to_path_buf())
}

pub async fn list(
    State(s): State<AppState>,
    headers: HeaderMap,
    UrlPath(thread): UrlPath<ThreadId>,
) -> Response {
    if !may_read(&s, &headers, thread) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Ok(files) = s.hub.db.file_sizes(thread) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    Json(json!(
        files
            .into_iter()
            .map(|(p, n)| json!({ "path": p, "size": n }))
            .collect::<Vec<_>>()
    ))
    .into_response()
}

#[derive(Deserialize)]
pub struct FileQuery {
    path: String,
    #[serde(default)]
    offset: u64,
    #[serde(default)]
    replace: bool,
}

pub async fn put(
    State(s): State<AppState>,
    headers: HeaderMap,
    UrlPath(thread): UrlPath<ThreadId>,
    Query(q): Query<FileQuery>,
    body: Bytes,
) -> Response {
    if !allowed(&s, &headers, thread) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(rel) = safe_rel(&q.path) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let rel = rel.to_string_lossy().to_string();
    if q.replace {
        return match s.hub.db.file_replace(thread, &rel, &body) {
            Ok(()) => Json(json!({ "size": body.len() })).into_response(),
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
    }
    match s.hub.db.file_append(thread, &rel, q.offset, &body) {
        Ok(Ok(size)) => Json(json!({ "size": size })).into_response(),
        Ok(Err(size)) => (StatusCode::CONFLICT, Json(json!({ "size": size }))).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn get(
    State(s): State<AppState>,
    headers: HeaderMap,
    UrlPath(thread): UrlPath<ThreadId>,
    Query(q): Query<FileQuery>,
) -> Response {
    if !may_read(&s, &headers, thread) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(rel) = safe_rel(&q.path) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    match s.hub.db.file_read(thread, &rel.to_string_lossy()) {
        Ok(Some(b)) => b.into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn runner_path() -> Option<PathBuf> {
    std::env::var_os("CORRA_RUNNER_BIN")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|e| e.parent().map(|d| d.join("corra-runner")))
        })
}

pub fn runner_build() -> Option<String> {
    use sha2::{Digest, Sha256};
    static BUILD: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    BUILD
        .get_or_init(|| {
            let bytes = std::fs::read(runner_path()?).ok()?;
            Some(
                Sha256::digest(&bytes)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect(),
            )
        })
        .clone()
}

pub async fn runner_binary(State(s): State<AppState>, headers: HeaderMap) -> Response {
    if machine_of(&s, &headers).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(p) = runner_path() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(file) = tokio::fs::File::open(&p).await else {
        return (StatusCode::NOT_FOUND, "the hub has no runner binary").into_response();
    };
    let len = file.metadata().await.ok().map(|m| m.len());
    let stream = tokio_util::io::ReaderStream::new(file);
    let mut builder = Response::builder().header("content-type", "application/octet-stream");
    if let Some(len) = len {
        builder = builder.header("content-length", len);
    }
    builder
        .body(axum::body::Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
