use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use corra_hub::hub::HubRef;
use corra_hub::providers::digitalocean::DigitalOcean;
use corra_hub::providers::{CreateRequest, Provider, ProviderStatus, Size};
use corra_proto::*;
use serde_json::{Value, json};

#[derive(Default)]
struct Cloud {
    next: u64,
    droplets: HashMap<u64, Value>,
    snapshots: HashMap<String, Value>,
    log: Vec<String>,
}

type S = State<Arc<Mutex<Cloud>>>;

async fn create(State(c): S, Json(b): Json<Value>) -> Json<Value> {
    let mut c = c.lock().unwrap();
    c.next += 1;
    let id = c.next;
    c.log.push(format!(
        "create {} size={} image={} tags={}",
        b["name"].as_str().unwrap(),
        b["size"].as_str().unwrap(),
        b["image"],
        b["tags"]
    ));
    let mut d = b.clone();
    d["id"] = json!(id);
    d["status"] = json!("active");
    c.droplets.insert(id, d.clone());
    Json(json!({ "droplet": d }))
}

async fn droplet(State(c): S, Path(id): Path<u64>) -> Result<Json<Value>, StatusCode> {
    let mut c = c.lock().unwrap();
    let d = c.droplets.get_mut(&id).ok_or(StatusCode::NOT_FOUND)?;
    if d["name"].as_str().unwrap().ends_with("-build") {
        d["status"] = json!("off");
    }
    Ok(Json(json!({ "droplet": d })))
}

async fn delete_droplet(State(c): S, Path(id): Path<u64>) -> StatusCode {
    let mut c = c.lock().unwrap();
    c.log.push(format!("delete droplet {id}"));
    match c.droplets.remove(&id) {
        Some(_) => StatusCode::NO_CONTENT,
        None => StatusCode::NOT_FOUND,
    }
}

async fn list(State(c): S, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    let c = c.lock().unwrap();
    let tag = q.get("tag_name").cloned().unwrap_or_default();
    let ds: Vec<&Value> = c
        .droplets
        .values()
        .filter(|d| d["tags"].as_array().unwrap().iter().any(|t| t == &tag))
        .collect();
    Json(json!({ "droplets": ds }))
}

async fn action(State(c): S, Path(id): Path<u64>, Json(b): Json<Value>) -> Json<Value> {
    let mut c = c.lock().unwrap();
    let kind = b["type"].as_str().unwrap().to_string();
    c.log.push(format!("{kind} {id}"));
    match kind.as_str() {
        "shutdown" | "power_off" => {
            c.droplets.get_mut(&id).unwrap()["status"] = json!("off");
        }
        "snapshot" => {
            assert_eq!(
                c.droplets[&id]["status"], "off",
                "snapshot of a running droplet"
            );
            c.next += 1;
            let sid = (1000 + c.next).to_string();
            let disk = if c.droplets[&id]["size"] == "s-4vcpu-16gb-amd" {
                200
            } else {
                80
            };
            c.snapshots.insert(
                sid.clone(),
                json!({ "id": sid, "name": b["name"], "min_disk_size": disk }),
            );
        }
        _ => {}
    }
    Json(json!({ "action": { "id": 7, "status": "in-progress" } }))
}

async fn action_status() -> Json<Value> {
    Json(json!({ "action": { "id": 7, "status": "completed" } }))
}

async fn snapshots(State(c): S) -> Json<Value> {
    let c = c.lock().unwrap();
    Json(json!({ "snapshots": c.snapshots.values().collect::<Vec<_>>() }))
}

async fn snapshot(State(c): S, Path(id): Path<String>) -> Result<Json<Value>, StatusCode> {
    let c = c.lock().unwrap();
    let s = c.snapshots.get(&id).ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(json!({ "snapshot": s })))
}

async fn delete_snapshot(State(c): S, Path(id): Path<String>) -> StatusCode {
    let mut c = c.lock().unwrap();
    c.log.push(format!("delete snapshot {id}"));
    match c.snapshots.remove(&id) {
        Some(_) => StatusCode::NO_CONTENT,
        None => StatusCode::NOT_FOUND,
    }
}

async fn fake() -> (String, Arc<Mutex<Cloud>>) {
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    let app = axum::Router::new()
        .route("/droplets", post(create).get(list))
        .route("/droplets/{id}", get(droplet).delete(delete_droplet))
        .route("/droplets/{id}/actions", post(action))
        .route("/actions/{id}", get(action_status))
        .route("/snapshots", get(snapshots))
        .route("/snapshots/{id}", get(snapshot).delete(delete_snapshot))
        .with_state(cloud.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, cloud)
}

fn hub(dir: &std::path::Path) -> HubRef {
    let hub = corra_hub::build(corra_hub::Options {
        data: dir.to_path_buf(),
        public_url: "http://127.0.0.1:1".into(),
        dev_no_auth: false,
    })
    .unwrap()
    .state
    .hub;
    hub.secrets.set("digitalocean.token", "t").unwrap();
    hub
}

fn req(size: &str) -> CreateRequest {
    CreateRequest {
        name: "corra-p-abc".into(),
        machine: new_id(),
        kind: "droplet".into(),
        size: Size::parse(size),
        env: vec![("CORRA_TOKEN".into(), "it's-secret".into())],
    }
}

fn take(cloud: &Arc<Mutex<Cloud>>) -> Vec<String> {
    std::mem::take(&mut cloud.lock().unwrap().log)
}

#[tokio::test]
async fn a_droplet_is_made_from_the_base_slept_to_a_snapshot_and_woken() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub(dir.path());
    let (url, cloud) = fake().await;
    let p = DigitalOcean::with_api(&url, Duration::from_millis(5));

    let first = p.create(&hub, req("4x8")).await.unwrap();
    let log = take(&cloud);
    assert!(
        log[0].starts_with("create corra-base-") && log[0].contains("-build"),
        "{log:?}"
    );
    assert!(log[0].contains("\"ubuntu-24-04-x64\""), "{log:?}");
    assert!(log[1].starts_with("snapshot "), "{log:?}");
    assert!(log[2].starts_with("delete droplet"), "{log:?}");
    assert!(log[3].contains("size=s-4vcpu-8gb-amd"), "{log:?}");
    assert_eq!(log.len(), 4, "{log:?}");
    let base_user_data =
        cloud.lock().unwrap().droplets[&first.parse::<u64>().unwrap()]["user_data"]
            .as_str()
            .unwrap()
            .to_string();
    assert!(base_user_data.contains("export CORRA_TOKEN='it'\\''s-secret'"));
    let tag = format!("corra-{}", corra_hub::providers::hub_tag(&hub));
    assert!(log[3].contains(&tag), "{log:?}");

    p.create(&hub, req("2x4")).await.unwrap();
    let log = take(&cloud);
    assert_eq!(log.len(), 1, "the base was built again: {log:?}");
    assert!(log[0].contains("size=s-2vcpu-4gb-amd"));

    let mut running = p.running(&hub).await.unwrap();
    running.sort();
    assert_eq!(running.len(), 2);
    assert!(running.contains(&first));

    let snap = p.sleep(&hub, &first).await.unwrap();
    let log = take(&cloud);
    assert_eq!(
        log,
        [
            format!("shutdown {first}"),
            format!("snapshot {first}"),
            format!("delete droplet {first}")
        ]
    );
    assert_eq!(p.status(&hub, &first).await.unwrap(), ProviderStatus::Gone);

    let woke = p.wake(&hub, &snap, req("4x8")).await.unwrap();
    let log = take(&cloud);
    assert!(log[0].contains(&format!("image={snap}")), "{log:?}");
    assert_eq!(
        p.status(&hub, &woke).await.unwrap(),
        ProviderStatus::Running
    );

    p.forget(&hub, &snap).await.unwrap();
    assert_eq!(take(&cloud), [format!("delete snapshot {snap}")]);
    p.forget(&hub, &snap).await.unwrap();
}

#[tokio::test]
async fn a_machine_cant_wake_on_a_droplet_smaller_than_its_disk() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub(dir.path());
    let (url, _cloud) = fake().await;
    let p = DigitalOcean::with_api(&url, Duration::from_millis(5));
    let big = p.create(&hub, req("4x16")).await.unwrap();
    let snap = p.sleep(&hub, &big).await.unwrap();
    let err = p
        .wake(&hub, &snap, req("2x4"))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("200 GB") && err.contains("80 GB"), "{err}");
    p.wake(&hub, &snap, req("8x16")).await.unwrap();
}
