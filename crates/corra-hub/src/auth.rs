use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use axum::Json;
use axum::extract::{ConnectInfo, State};
use axum::http::{Extensions, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::{Cookie, SameSite};
use corra_proto::now_ms;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::hub::{HubRef, hash_token, new_token};

const COOKIE: &str = "corra_session";
const SESSION_DAYS: i64 = 365;
const WRONG_TRIES: u32 = 10;
const WRONG_WINDOW: Duration = Duration::from_secs(60);

pub struct Auth {
    secure_cookie: bool,
    pub dev_no_auth: bool,
    key_hash: Mutex<Option<String>>,
    trust_ip_header: Option<String>,
    failures: Mutex<HashMap<String, (Instant, u32)>>,
    pairing: Mutex<HashMap<String, Instant>>,
}

pub type AuthRef = Arc<Auth>;

impl Auth {
    pub fn new(public_url: &str, dev_no_auth: bool) -> AuthRef {
        Arc::new(Auth {
            secure_cookie: public_url.starts_with("https://"),
            dev_no_auth,
            key_hash: Mutex::new(None),
            trust_ip_header: std::env::var("CORRA_TRUST_IP_HEADER")
                .ok()
                .map(|h| h.trim().to_ascii_lowercase())
                .filter(|h| !h.is_empty()),
            failures: Mutex::new(HashMap::new()),
            pairing: Mutex::new(HashMap::new()),
        })
    }
}

impl Auth {
    fn client(&self, headers: &HeaderMap, ext: &Extensions) -> String {
        let named = self
            .trust_ip_header
            .as_deref()
            .and_then(|h| headers.get(h))
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit(',').map(str::trim).find(|p| !p.is_empty()));
        if let Some(ip) = named {
            return ip.to_string();
        }
        ext.get::<ConnectInfo<std::net::SocketAddr>>()
            .map(|c| c.0.ip().to_string())
            .unwrap_or_default()
    }

    fn locked_out(&self, client: &str) -> bool {
        let mut f = self.failures.lock().unwrap();
        f.retain(|_, (since, _)| since.elapsed() < WRONG_WINDOW);
        f.get(client).is_some_and(|(_, n)| *n >= WRONG_TRIES)
    }

    fn failed(&self, client: &str) {
        let mut f = self.failures.lock().unwrap();
        let e = f.entry(client.to_string()).or_insert((Instant::now(), 0));
        e.1 += 1;
    }
}

fn too_many() -> Response {
    err(
        StatusCode::TOO_MANY_REQUESTS,
        "Too many wrong tries from here; try again in a minute.",
    )
}

fn err(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

pub fn load_access_key(hub: &HubRef, auth: &Auth, data: &std::path::Path) -> Result<()> {
    let from_env = std::env::var("CORRA_ACCESS_KEY")
        .ok()
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty());
    let key = match from_env {
        Some(k) => {
            if k.len() < 16 {
                anyhow::bail!("CORRA_ACCESS_KEY must be at least 16 characters");
            }
            k
        }
        None => {
            let path = data.join("access-key");
            match std::fs::read_to_string(&path) {
                Ok(k) if !k.trim().is_empty() => k.trim().to_string(),
                _ => {
                    use std::io::Write;
                    use std::os::unix::fs::OpenOptionsExt;
                    let k = new_token("key");
                    let mut f = std::fs::OpenOptions::new()
                        .write(true)
                        .create(true)
                        .truncate(true)
                        .mode(0o600)
                        .open(&path)?;
                    f.write_all(k.as_bytes())?;
                    tracing::warn!(
                        "no CORRA_ACCESS_KEY: sign in with the key in {}",
                        path.display()
                    );
                    k
                }
            }
        }
    };
    let hash = hash_token(&key);
    if hub.db.kv_get("access_key")?.as_deref() != Some(hash.as_str()) {
        for k in hub.db.kv_keys("session:")? {
            let _ = hub.db.kv_delete(&k);
        }
        for k in hub.db.kv_keys("push:")? {
            let _ = hub.db.kv_delete(&k);
        }
        hub.db.kv_set("access_key", &hash)?;
        tracing::info!("the access key is new, so every earlier sign-in was signed out");
    }
    *auth.key_hash.lock().unwrap() = Some(hash);
    Ok(())
}

pub fn session_valid(hub: &HubRef, token: &str) -> bool {
    let key = format!("session:{}", hash_token(token));
    match hub.db.kv_get(&key) {
        Ok(Some(v)) => {
            let v: Value = serde_json::from_str(&v).unwrap_or_default();
            v["expires"].as_i64().unwrap_or(0) > now_ms()
        }
        _ => false,
    }
}

pub fn list_sessions(hub: &HubRef) -> Result<Value> {
    let mut out = vec![];
    for key in hub.db.kv_keys("session:")? {
        let Some(v) = hub.db.kv_get(&key)? else {
            continue;
        };
        let v: Value = serde_json::from_str(&v).unwrap_or_default();
        let id = key.trim_start_matches("session:");
        if v["expires"].as_i64().unwrap_or(0) <= now_ms() {
            let _ = hub.db.kv_delete(&key);
            let _ = crate::notify::unsubscribe(hub, id);
            continue;
        }
        out.push(json!({
            "id": id,
            "push": crate::notify::subscribed(hub, id),
            "kind": v["kind"],
            "name": v["name"],
            "created": v["created"],
            "expires": v["expires"],
        }));
    }
    out.sort_by_key(|v| -v["created"].as_i64().unwrap_or(0));
    Ok(json!(out))
}

pub fn revoke_session(hub: &HubRef, id: &str) -> Result<()> {
    if id.len() != 64 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
        anyhow::bail!("no such session");
    }
    let _ = crate::notify::unsubscribe(hub, id);
    hub.db.kv_delete(&format!("session:{id}"))
}

pub fn create_device_session(db: &crate::db::Db, name: &str) -> Result<String> {
    let token = new_token("cs");
    let expires = now_ms() + SESSION_DAYS * 86_400_000;
    db.kv_set(
        &format!("session:{}", hash_token(&token)),
        &json!({ "kind": "device", "name": name, "created": now_ms(), "expires": expires })
            .to_string(),
    )?;
    Ok(token)
}

fn create_session(hub: &HubRef, kind: &str, name: &str) -> Result<String> {
    let name: String = name
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(80)
        .collect();
    let token = new_token("cs");
    let expires = now_ms() + SESSION_DAYS * 86_400_000;
    hub.db.kv_set(
        &format!("session:{}", hash_token(&token)),
        &json!({ "kind": kind, "name": name, "created": now_ms(), "expires": expires }).to_string(),
    )?;
    Ok(token)
}

fn session_cookie(auth: &Auth, token: String) -> Cookie<'static> {
    Cookie::build((COOKIE, token))
        .http_only(true)
        .secure(auth.secure_cookie)
        .same_site(SameSite::Strict)
        .path("/")
        .max_age(time::Duration::days(SESSION_DAYS))
        .build()
}

pub fn request_authorized(
    hub: &HubRef,
    auth: &Auth,
    jar: &CookieJar,
    headers: &HeaderMap,
    query_token: Option<&str>,
) -> bool {
    request_session(hub, auth, jar, headers, query_token).is_some()
}

pub fn request_session(
    hub: &HubRef,
    auth: &Auth,
    jar: &CookieJar,
    headers: &HeaderMap,
    query_token: Option<&str>,
) -> Option<String> {
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let found = [jar.get(COOKIE).map(|c| c.value()), bearer, query_token]
        .into_iter()
        .flatten()
        .find(|t| session_valid(hub, t))
        .map(hash_token);
    match found {
        Some(s) => Some(s),
        None if auth.dev_no_auth => Some("dev".into()),
        None => None,
    }
}

#[derive(Clone)]
pub struct AppState {
    pub hub: HubRef,
    pub auth: AuthRef,
    pub data: std::path::PathBuf,
}

pub async fn state(State(s): State<AppState>, jar: CookieJar, headers: HeaderMap) -> Response {
    Json(json!({
        "signed_in": request_authorized(&s.hub, &s.auth, &jar, &headers, None),
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct KeyLogin {
    key: String,
    #[serde(default)]
    device: Option<String>,
}

pub async fn key_login(
    State(s): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    ext: Extensions,
    Json(body): Json<KeyLogin>,
) -> Response {
    let client = s.auth.client(&headers, &ext);
    if s.auth.locked_out(&client) {
        return too_many();
    }
    let expected = s.auth.key_hash.lock().unwrap().clone();
    if expected.is_none() || expected.as_deref() != Some(hash_token(body.key.trim()).as_str()) {
        s.auth.failed(&client);
        return err(StatusCode::UNAUTHORIZED, "That isn't the hub's key.");
    }
    if let Some(name) = body.device {
        let name: String = name.chars().take(60).collect();
        return match create_session(&s.hub, "device", &name) {
            Ok(t) => Json(json!({ "token": t })).into_response(),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        };
    }
    let name = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(browser_name)
        .unwrap_or_else(|| "a browser".into());
    match create_session(&s.hub, "browser", &name) {
        Ok(t) => (
            jar.add(session_cookie(&s.auth, t)),
            Json(json!({ "ok": true })),
        )
            .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

fn browser_name(ua: &str) -> String {
    let os = ["Android", "iPhone", "Mac OS", "Windows", "Linux"]
        .into_iter()
        .find(|o| ua.contains(o))
        .unwrap_or("unknown system");
    let app = ["Firefox", "Edg", "Chrome", "Safari"]
        .into_iter()
        .find(|b| ua.contains(b))
        .map(|b| if b == "Edg" { "Edge" } else { b })
        .unwrap_or("a browser");
    format!("{app} on {os}")
}

pub async fn logout(State(s): State<AppState>, jar: CookieJar) -> Response {
    if let Some(c) = jar.get(COOKIE) {
        let id = hash_token(c.value());
        let _ = s.hub.db.kv_delete(&format!("session:{id}"));
        let _ = crate::notify::unsubscribe(&s.hub, &id);
    }
    (
        jar.remove(Cookie::build((COOKIE, "")).path("/").build()),
        Json(json!({ "ok": true })),
    )
        .into_response()
}

pub async fn pair_start(State(s): State<AppState>, jar: CookieJar, headers: HeaderMap) -> Response {
    if !request_authorized(&s.hub, &s.auth, &jar, &headers, None) {
        return err(StatusCode::UNAUTHORIZED, "Sign in first.");
    }
    use rand::RngExt;
    const ALPHA: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::rng();
    let code: String = (0..8)
        .map(|_| ALPHA[rng.random_range(0..ALPHA.len())] as char)
        .collect();
    let mut p = s.auth.pairing.lock().unwrap();
    p.retain(|_, t| t.elapsed() < Duration::from_secs(600));
    p.insert(code.clone(), Instant::now());
    Json(json!({ "code": code, "expires_in": 600 })).into_response()
}

#[derive(Deserialize)]
pub struct PairFinish {
    code: String,
    name: String,
}

pub async fn pair_finish(
    State(s): State<AppState>,
    headers: HeaderMap,
    ext: Extensions,
    Json(body): Json<PairFinish>,
) -> Response {
    let client = s.auth.client(&headers, &ext);
    if s.auth.locked_out(&client) {
        return too_many();
    }
    let code = body.code.trim().to_uppercase().replace(['-', ' '], "");
    let ok = {
        let mut p = s.auth.pairing.lock().unwrap();
        matches!(p.remove(&code), Some(t) if t.elapsed() < Duration::from_secs(600))
    };
    if !ok {
        s.auth.failed(&client);
        return err(
            StatusCode::UNAUTHORIZED,
            "That code isn't valid or has expired.",
        );
    }
    match create_session(&s.hub, "device", &body.name) {
        Ok(t) => Json(json!({ "token": t })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}
