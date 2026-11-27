use std::io::Write;
use std::net::SocketAddr;

use corra_hub::db::NewMachine;
use corra_hub::hub::{NewProject, hash_token, new_token};
use corra_proto::*;
use reqwest::StatusCode;
use serde_json::json;

async fn start_hub(dir: &std::path::Path) -> (String, corra_hub::auth::AppState, String) {
    let built = corra_hub::build(corra_hub::Options {
        data: dir.to_path_buf(),
        public_url: "http://127.0.0.1:1".into(),
        dev_no_auth: false,
    })
    .unwrap();
    let state = built.state.clone();
    let app = corra_hub::finish(built.router);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let key = std::fs::read_to_string(dir.join("access-key"))
        .unwrap()
        .trim()
        .to_string();
    (url, state, key)
}

#[tokio::test]
async fn scoped_git_token_works_only_for_git_and_is_rejected_on_runner_endpoints() {
    let dir = tempfile::tempdir().unwrap();
    let (url, state, _) = start_hub(dir.path()).await;
    let hub = &state.hub;

    let runner_token = new_token("cm");
    let m = hub
        .db
        .insert_machine(NewMachine {
            name: "box".into(),
            provider: "existing".into(),
            project: None,
            size: "existing".into(),
            token_hash: hash_token(&runner_token),
            provider_ref: None,
            status: MachineStatus::Connected,
            kind: None,
        })
        .unwrap();
    let p = hub
        .create_project(NewProject {
            name: "p".into(),
            repos: vec!["acme/app".into()],
            machine: Some(m.id),
            provider: Some("existing".into()),
            ..Default::default()
        })
        .unwrap();
    let thread = new_id();
    hub.db
        .insert_thread(&Thread {
            id: thread,
            project: p.id,
            title: "t".into(),
            agent: AgentKind::Claude,
            machine: Some(m.id),
            session_id: None,
            cwd: "~".into(),
            settings: AgentSettings::default(),
            status: ThreadStatus::Idle,
            last_seq: 0,
            created_at: 0,
            updated_at: 0,
            pinned_at: None,
            settled_at: None,
            hidden_at: None,
            started_by: None,
        })
        .unwrap();

    let git_token = hub.git_token(m.id).expect("git token");
    assert!(git_token.starts_with("cg_"));
    assert_eq!(
        hub.verify_git_token(&git_token).unwrap().map(|a| a.id),
        Some(m.id)
    );
    assert!(
        hub.verify_git_token(&format!("{git_token}0"))
            .unwrap()
            .is_none()
    );

    let client = reqwest::Client::new();
    // Every non-git runner endpoint MUST reject the scoped git token with 401
    for path in [
        format!("/runner/files/{thread}"),
        format!("/runner/file/{thread}?path=a.jsonl"),
        "/runner/kit".to_string(),
        "/dist/runner".to_string(),
    ] {
        let res = client
            .get(format!("{url}{path}"))
            .bearer_auth(&git_token)
            .send()
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            StatusCode::UNAUTHORIZED,
            "cg_ token was accepted on {path}"
        );
    }

    // On /git/*, cg_ token passes authentication (and hits the "no GitHub credential" 403, NOT 401!)
    let git_res = client
        .get(format!(
            "{url}/git/acme/app.git/info/refs?service=git-upload-pack"
        ))
        .bearer_auth(&git_token)
        .send()
        .await
        .unwrap();
    assert_eq!(git_res.status(), StatusCode::FORBIDDEN);
    assert!(
        git_res
            .text()
            .await
            .unwrap()
            .contains("no GitHub credential")
    );

    // Rotating the machine's token immediately invalidates old cg_ tokens
    let new_runner_token = new_token("cm");
    hub.db
        .set_machine_token(m.id, &hash_token(&new_runner_token))
        .unwrap();
    assert!(hub.verify_git_token(&git_token).unwrap().is_none());
    let rotated_git = hub.git_token(m.id).unwrap();
    assert_ne!(rotated_git, git_token);
    assert_eq!(
        hub.verify_git_token(&rotated_git).unwrap().map(|a| a.id),
        Some(m.id)
    );

    // Destroying the machine invalidates cg_ tokens
    hub.db
        .set_machine_status(m.id, MachineStatus::Destroyed)
        .unwrap();
    assert!(hub.verify_git_token(&rotated_git).unwrap().is_none());
}

#[tokio::test]
async fn x_corra_thread_header_selects_the_exact_thread_and_rejects_foreign_machines() {
    let dir = tempfile::tempdir().unwrap();
    let (_, state, _) = start_hub(dir.path()).await;
    let hub = &state.hub;

    let m1 = new_id();
    let m2 = new_id();
    let p = hub
        .create_project(NewProject {
            name: "p".into(),
            provider: Some("existing".into()),
            ..Default::default()
        })
        .unwrap();

    let mk_thread = |machine: MachineId, updated: i64, turn: &str| -> ThreadId {
        let id = new_id();
        hub.db
            .insert_thread(&Thread {
                id,
                project: p.id,
                title: turn.into(),
                agent: AgentKind::Claude,
                machine: Some(machine),
                session_id: None,
                cwd: "~".into(),
                settings: AgentSettings::default(),
                status: ThreadStatus::Running,
                last_seq: 0,
                created_at: 0,
                updated_at: updated,
                pinned_at: None,
                settled_at: None,
                hidden_at: None,
                started_by: None,
            })
            .unwrap();
        hub.append(
            id,
            None,
            updated,
            Event::TurnStarted {
                turn: turn.into(),
                input: vec![],
                settings: AgentSettings::default(),
                cause: None,
            },
        )
        .unwrap();
        id
    };

    let t_older = mk_thread(m1, 1000, "turn-old");
    let t_newer = mk_thread(m1, 2000, "turn-new");
    let t_other_machine = mk_thread(m2, 3000, "turn-other");

    // Without header, falls back to most recently updated running thread
    let empty = axum::http::HeaderMap::new();
    assert_eq!(
        corra_hub::gitproxy::asking_thread(&state, m1, &empty),
        Some((t_newer, "turn-new".into()))
    );

    // With X-Corra-Thread pointing to t_older, picks t_older even though t_newer is newer
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-corra-thread", t_older.to_string().parse().unwrap());
    assert_eq!(
        corra_hub::gitproxy::asking_thread(&state, m1, &headers),
        Some((t_older, "turn-old".into()))
    );

    // Forging a thread from another machine falls back to m1's own thread
    let mut foreign = axum::http::HeaderMap::new();
    foreign.insert(
        "x-corra-thread",
        t_other_machine.to_string().parse().unwrap(),
    );
    assert_eq!(
        corra_hub::gitproxy::asking_thread(&state, m1, &foreign),
        Some((t_newer, "turn-new".into()))
    );
}

#[tokio::test]
async fn logout_clears_push_subscription_and_sets_root_cookie_path_and_xff_uses_rightmost_ip() {
    let dir = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("CORRA_TRUST_IP_HEADER", "X-Forwarded-For") };
    let (url, state, key) = start_hub(dir.path()).await;
    unsafe { std::env::remove_var("CORRA_TRUST_IP_HEADER") };

    let client = reqwest::Client::new();
    // Spoofing the leftmost X-Forwarded-For IP cannot dodge rate limiting on the rightmost IP
    for i in 0..10 {
        let r = client
            .post(format!("{url}/auth/key"))
            .header("x-forwarded-for", format!("198.51.100.{i}, 203.0.113.42"))
            .json(&json!({ "key": "wrong" }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    }
    let locked = client
        .post(format!("{url}/auth/key"))
        .header("x-forwarded-for", "192.0.2.99, 203.0.113.42")
        .json(&json!({ "key": key }))
        .send()
        .await
        .unwrap();
    assert_eq!(locked.status(), StatusCode::TOO_MANY_REQUESTS);

    // Real sign-in from a different rightmost IP succeeds
    let login = client
        .post(format!("{url}/auth/key"))
        .header("x-forwarded-for", "198.51.100.1, 203.0.113.77")
        .json(&json!({ "key": key }))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let cookie = login
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let session_keys = state.hub.db.kv_keys("session:").unwrap();
    assert_eq!(session_keys.len(), 1);
    let sid = session_keys[0].trim_start_matches("session:").to_string();
    state
        .hub
        .db
        .kv_set(
            &format!("push:{sid}"),
            r#"{"endpoint":"https://example.com"}"#,
        )
        .unwrap();
    assert!(corra_hub::notify::subscribed(&state.hub, &sid));

    // Logout must delete BOTH session and push subscription AND expire cookie with Path=/
    let out = client
        .post(format!("{url}/auth/logout"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(out.status(), StatusCode::OK);
    let set_cookie = out
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        set_cookie.contains("Path=/"),
        "logout Set-Cookie missing Path=/: {set_cookie}"
    );
    assert!(state.hub.db.kv_keys("session:").unwrap().is_empty());
    assert!(!corra_hub::notify::subscribed(&state.hub, &sid));

    // Rotating CORRA_ACCESS_KEY clears all push:* subscriptions too
    state
        .hub
        .db
        .kv_set(
            "push:stale-session",
            r#"{"endpoint":"https://example.com"}"#,
        )
        .unwrap();
    unsafe {
        std::env::set_var(
            "CORRA_ACCESS_KEY",
            "a-brand-new-access-key-for-rotation-test",
        )
    };
    corra_hub::auth::load_access_key(&state.hub, &state.auth, dir.path()).unwrap();
    unsafe { std::env::remove_var("CORRA_ACCESS_KEY") };
    assert!(state.hub.db.kv_keys("push:").unwrap().is_empty());
}

#[tokio::test]
async fn body_limits_and_compression_and_streaming_push_guard() {
    let dir = tempfile::tempdir().unwrap();
    let (url, state, _) = start_hub(dir.path()).await;
    let hub = &state.hub;

    let client = reqwest::Client::new();
    // Normal JSON routes reject > 2 MB bodies with 413 Payload Too Large
    let big = vec![b'x'; 3 << 20];
    let res = client
        .post(format!("{url}/auth/key"))
        .header("content-type", "application/json")
        .body(big)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);

    // /dist/runner streams binary without gzip re-compression even with Accept-Encoding: gzip
    let runner_token = new_token("cm");
    let m = hub
        .db
        .insert_machine(NewMachine {
            name: "box".into(),
            provider: "existing".into(),
            project: None,
            size: "existing".into(),
            token_hash: hash_token(&runner_token),
            provider_ref: None,
            status: MachineStatus::Connected,
            kind: None,
        })
        .unwrap();
    let p = hub
        .create_project(NewProject {
            name: "p".into(),
            repos: vec!["acme/app".into()],
            machine: Some(m.id),
            provider: Some("existing".into()),
            ..Default::default()
        })
        .unwrap();
    let fake_bin = dir.path().join("fake-runner");
    let bin_bytes = vec![0x7f; 64 * 1024];
    std::fs::write(&fake_bin, &bin_bytes).unwrap();
    unsafe { std::env::set_var("CORRA_RUNNER_BIN", &fake_bin) };
    let bin_res = client
        .get(format!("{url}/dist/runner"))
        .bearer_auth(&runner_token)
        .header("accept-encoding", "gzip")
        .send()
        .await
        .unwrap();
    unsafe { std::env::remove_var("CORRA_RUNNER_BIN") };
    assert_eq!(bin_res.status(), StatusCode::OK);
    assert!(
        bin_res.headers().get("content-encoding").is_none(),
        "/dist/runner should not be re-compressed"
    );
    assert_eq!(
        bin_res.bytes().await.unwrap().as_ref(),
        bin_bytes.as_slice()
    );

    // Streaming git-receive-pack rejects bad gzip cleanly via the HTTP route
    hub.secrets.set("github.token", "ghp_fake").unwrap();
    let git_token = hub.git_token(m.id).unwrap();
    let bad_gz = client
        .post(format!("{url}/git/acme/app.git/git-receive-pack"))
        .bearer_auth(&git_token)
        .header("content-encoding", "gzip")
        .body(b"not valid gzip".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(bad_gz.status(), StatusCode::FORBIDDEN);
    assert!(bad_gz.text().await.unwrap().contains("couldn't unpack"));

    // Streaming gzipped push to refs/heads/main is parsed and guarded (rejected cleanly without a thread)
    hub.db
        .kv_set(
            "default_branch:acme/app",
            &json!({ "branch": "main", "at": now_ms() }).to_string(),
        )
        .unwrap();
    let raw_main = std::fs::read(format!(
        "{}/tests/pushes/main.bin",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&raw_main).unwrap();
    let packed = gz.finish().unwrap();
    let main_res = client
        .post(format!("{url}/git/acme/app.git/git-receive-pack"))
        .bearer_auth(&git_token)
        .header("content-encoding", "gzip")
        .body(packed)
        .send()
        .await
        .unwrap();
    assert_eq!(main_res.status(), StatusCode::OK);
    let body_text = String::from_utf8_lossy(&main_res.bytes().await.unwrap()).to_string();
    assert!(
        body_text.contains("ng refs/heads/main"),
        "expected guarded push refusal, got: {body_text:?}"
    );
    let _ = p;
}
