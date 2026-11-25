use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use corra_hub::db::NewMachine;
use corra_hub::hub::{Hub, HubRef, NewProject};
use corra_hub::providers::{CreateRequest, Provider, ProviderStatus};
use corra_proto::*;

struct Fake {
    status: Mutex<ProviderStatus>,
    release: tokio::sync::Notify,
}

#[async_trait]
impl Provider for Fake {
    fn name(&self) -> &'static str {
        "fake"
    }
    fn creates_machines(&self) -> bool {
        true
    }
    fn sleeps(&self) -> bool {
        true
    }
    async fn create(&self, _hub: &Hub, _req: CreateRequest) -> Result<String> {
        Ok("box-1".into())
    }
    async fn wake(&self, _hub: &Hub, _sleep_ref: &str, _req: CreateRequest) -> Result<String> {
        self.release.notified().await;
        Ok("box-2".into())
    }
    async fn status(&self, _hub: &Hub, _provider_ref: &str) -> Result<ProviderStatus> {
        Ok(*self.status.lock().unwrap())
    }
}

fn setup(dir: &std::path::Path, status: ProviderStatus) -> (HubRef, Arc<Fake>, Project) {
    let hub = corra_hub::build(corra_hub::Options {
        data: dir.to_path_buf(),
        public_url: "http://127.0.0.1:1".into(),
        dev_no_auth: false,
    })
    .unwrap()
    .state
    .hub;
    let fake = Arc::new(Fake {
        status: Mutex::new(status),
        release: tokio::sync::Notify::new(),
    });
    hub.providers.add(fake.clone());
    let p = hub
        .create_project(NewProject {
            name: "p".into(),
            provider: Some("fake".into()),
            ..Default::default()
        })
        .unwrap();
    (hub, fake, p)
}

fn status(hub: &HubRef, m: MachineId) -> MachineStatus {
    hub.db.machine(m).unwrap().unwrap().status
}

#[tokio::test]
async fn the_sweep_leaves_a_machine_alone_while_it_wakes() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, fake, p) = setup(dir.path(), ProviderStatus::Running);
    let m = hub
        .db
        .insert_machine(NewMachine {
            name: "old".into(),
            provider: "fake".into(),
            project: Some(p.id),
            size: "4x8".into(),
            token_hash: "h".into(),
            provider_ref: None,
            status: MachineStatus::Asleep,
        })
        .unwrap();
    hub.db.set_machine_sleep_ref(m.id, Some("snap")).unwrap();
    let day_ago = now_ms() - 24 * 3600 * 1000;
    rusqlite::Connection::open(dir.path().join("corra.db"))
        .unwrap()
        .execute(
            "UPDATE machines SET created_at = ?1 WHERE id = ?2",
            rusqlite::params![day_ago, m.id.to_string()],
        )
        .unwrap();

    let waking = {
        let hub = hub.clone();
        let p = p.clone();
        tokio::spawn(async move { hub.providers.start_for_project(&hub, &p).await })
    };
    for _ in 0..200 {
        if status(&hub, m.id) == MachineStatus::Creating {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(status(&hub, m.id), MachineStatus::Creating);

    corra_hub::lifecycle::sweep(&hub).await.unwrap();
    assert_eq!(status(&hub, m.id), MachineStatus::Creating);

    *fake.status.lock().unwrap() = ProviderStatus::Gone;
    fake.release.notify_one();
    let err = tokio::time::timeout(Duration::from_secs(15), waking)
        .await
        .expect("the wake should fail fast once the machine is gone")
        .unwrap()
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("stopped before its runner connected"),
        "{err}"
    );

    corra_hub::lifecycle::sweep(&hub).await.unwrap();
    assert_eq!(status(&hub, m.id), MachineStatus::Failed);
}

#[tokio::test]
async fn a_new_machine_that_dies_during_boot_fails_in_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, _fake, p) = setup(dir.path(), ProviderStatus::Gone);
    let began = std::time::Instant::now();
    let err = tokio::time::timeout(
        Duration::from_secs(15),
        hub.providers.start_for_project(&hub, &p),
    )
    .await
    .expect("the start should fail fast once the machine is gone")
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("stopped before its runner connected"),
        "{err}"
    );
    assert!(began.elapsed() < Duration::from_secs(10));
}
