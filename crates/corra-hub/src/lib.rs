pub mod auth;
pub mod backup;
pub mod db;
pub mod files;
pub mod follow;
pub mod github;
pub mod gitproxy;
pub mod hub;
pub mod import;
pub mod kit;
pub mod leftovers;
pub mod lifecycle;
pub mod manage;
pub mod notify;
pub mod phone;
pub mod providers;
pub mod schedule;
pub mod secrets;
pub mod storage;
pub mod tools;
pub mod web;
pub mod webpush;
pub mod ws;

use std::path::PathBuf;

use anyhow::Result;
use axum::Router;
use axum::routing::{get, post};

use crate::auth::AppState;
use crate::hub::Hub;
use crate::providers::Providers;

pub struct Options {
    pub data: PathBuf,
    pub public_url: String,
    pub dev_no_auth: bool,
}

pub struct Built {
    pub router: Router,
    pub state: AppState,
}

pub fn build(opts: Options) -> Result<Built> {
    std::fs::create_dir_all(&opts.data)?;
    let db = db::Db::open(&opts.data.join("corra.db"))?;
    let secrets = secrets::Secrets::open(db.clone(), &opts.data)?;
    let hub = Hub::new(
        db,
        Providers::new(opts.public_url.clone()),
        secrets,
        opts.data.clone(),
    );
    let auth = auth::Auth::new(&opts.public_url, opts.dev_no_auth);
    auth::load_access_key(&hub, &auth, &opts.data)?;
    if let Err(e) = hub.close_stale_requests() {
        tracing::warn!("couldn't close the requests left from before the restart: {e:#}");
    }
    lifecycle::spawn(hub.clone());
    backup::spawn(hub.clone(), opts.data.clone());
    let state = AppState {
        hub,
        auth,
        data: opts.data.clone(),
    };
    let router = Router::new()
        .route("/runner", get(ws::runner_ws))
        .route("/app", get(ws::app_ws))
        .route("/auth/state", get(auth::state))
        .route("/auth/logout", post(auth::logout))
        .route("/runner/files/{thread}", get(files::list))
        .route("/runner/file/{thread}", get(files::get).put(files::put))
        .route("/runner/import", post(files::import))
        .route(
            "/healthz",
            get(|| async { ([("access-control-allow-origin", "*")], "ok") }),
        )
        .route("/auth/key", post(auth::key_login))
        .route("/auth/pair/start", post(auth::pair_start))
        .route("/auth/pair/finish", post(auth::pair_finish))
        .route("/api/machine", post(manage::register_machine))
        .route("/git/{*path}", get(gitproxy::proxy).post(gitproxy::proxy))
        .route("/dist/runner", get(files::runner_binary))
        .route("/runner/kit", get(kit::serve))
        .route(
            "/blob/{id}",
            get(phone::blob_get)
                .put(phone::blob_put)
                .layer(axum::extract::DefaultBodyLimit::disable()),
        );
    Ok(Built {
        router: router
            .layer(axum::extract::DefaultBodyLimit::max(256 << 20))
            .with_state(state.clone()),
        state,
    })
}

pub fn finish(router: Router) -> Router {
    use axum::http::{HeaderValue, header};
    use tower_http::set_header::SetResponseHeaderLayer;
    let csp = HeaderValue::from_str(&web::csp()).expect("a valid header");
    router
        .fallback(web::static_handler)
        .layer(SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            csp,
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            HeaderValue::from_static("same-origin"),
        ))
        .layer(tower_http::compression::CompressionLayer::new())
}
