use std::net::{IpAddr, SocketAddr};

use reqwest::StatusCode;
use serde_json::json;

async fn serve(dir: &std::path::Path) -> (String, String) {
    let built = corra_hub::build(corra_hub::Options {
        data: dir.to_path_buf(),
        public_url: "http://127.0.0.1:1".into(),
        dev_no_auth: false,
    })
    .unwrap();
    let app = corra_hub::finish(built.router);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap()
    });
    let key = std::fs::read_to_string(dir.join("access-key")).unwrap();
    (url, key.trim().to_string())
}

fn from(ip: &str) -> reqwest::Client {
    reqwest::Client::builder()
        .local_address(ip.parse::<IpAddr>().unwrap())
        .build()
        .unwrap()
}

async fn sign_in(c: &reqwest::Client, url: &str, key: &str, real_ip: Option<&str>) -> StatusCode {
    let mut r = c
        .post(format!("{url}/auth/key"))
        .json(&json!({ "key": key }));
    if let Some(ip) = real_ip {
        r = r.header("x-real-ip", ip);
    }
    r.send().await.unwrap().status()
}

async fn pair(c: &reqwest::Client, url: &str, code: &str) -> StatusCode {
    c.post(format!("{url}/auth/pair/finish"))
        .json(&json!({ "code": code, "name": "phone" }))
        .send()
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn wrong_tries_lock_out_only_the_address_they_came_from() {
    let dir = tempfile::tempdir().unwrap();
    let (url, key) = serve(dir.path()).await;
    let (attacker, owner, pairer) = (from("127.0.0.2"), from("127.0.0.1"), from("127.0.0.3"));

    for _ in 0..10 {
        assert_eq!(
            sign_in(&attacker, &url, "wrong", None).await,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        sign_in(&attacker, &url, &key, None).await,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        sign_in(&attacker, &url, "wrong", Some("198.51.100.9")).await,
        StatusCode::TOO_MANY_REQUESTS,
        "a header the hub wasn't told to trust got around the limit"
    );
    assert_eq!(sign_in(&owner, &url, &key, None).await, StatusCode::OK);

    for _ in 0..10 {
        assert_eq!(
            pair(&pairer, &url, "WRONGCODE").await,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        pair(&pairer, &url, "WRONGCODE").await,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        sign_in(&pairer, &url, &key, None).await,
        StatusCode::TOO_MANY_REQUESTS
    );

    let dir = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("CORRA_TRUST_IP_HEADER", "X-Real-IP") };
    let (url, key) = serve(dir.path()).await;
    unsafe { std::env::remove_var("CORRA_TRUST_IP_HEADER") };
    let proxy = from("127.0.0.1");
    for _ in 0..10 {
        assert_eq!(
            sign_in(&proxy, &url, "wrong", Some("203.0.113.7")).await,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        sign_in(&proxy, &url, &key, Some("203.0.113.7")).await,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        sign_in(&proxy, &url, &key, Some("198.51.100.9")).await,
        StatusCode::OK
    );
    assert_eq!(sign_in(&proxy, &url, &key, None).await, StatusCode::OK);
}
