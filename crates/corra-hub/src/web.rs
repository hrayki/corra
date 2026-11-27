use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "../../web/dist"]
#[allow_missing = true]
struct Assets;

pub fn csp() -> String {
    [
        "default-src 'self'".to_string(),
        "script-src 'self' 'wasm-unsafe-eval'".into(),
        "style-src 'self' 'unsafe-inline'".into(),
        "img-src 'self' data: blob:".into(),
        "media-src 'self' blob:".into(),
        "font-src 'self' data:".into(),
        "connect-src 'self' data:".into(),
        "worker-src 'self' blob:".into(),
        "object-src 'none'".into(),
        "base-uri 'none'".into(),
        "form-action 'self'".into(),
        "frame-ancestors 'none'".into(),
    ]
    .join("; ")
}

pub fn build() -> Option<String> {
    static BUILD: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    BUILD
        .get_or_init(|| {
            let page = Assets::get("index.html")?;
            let page = std::str::from_utf8(&page.data).ok()?;
            let assets: Vec<&str> = page
                .split('"')
                .filter(|x| x.starts_with("/assets/"))
                .collect();
            (!assets.is_empty()).then(|| assets.join(" "))
        })
        .clone()
}

pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    if let Some(file) = Assets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        let cache = if path.starts_with("assets/") {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        return (
            [
                (header::CONTENT_TYPE, mime.as_ref().to_string()),
                (header::CACHE_CONTROL, cache.to_string()),
            ],
            file.data,
        )
            .into_response();
    }
    let api = ["dist/", "git/", "runner", "auth/", "app"];
    if path.contains('.') || api.iter().any(|p| path.starts_with(p)) {
        return StatusCode::NOT_FOUND.into_response();
    }
    match Assets::get("index.html") {
        Some(file) => (
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()),
                (header::CACHE_CONTROL, "no-cache".to_string()),
            ],
            file.data,
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            "the web app isn't built: run pnpm build in web/",
        )
            .into_response(),
    }
}
