use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, bail};
use async_trait::async_trait;
use corra_hub::db::NewMachine;
use corra_hub::hub::RunnerConn;
use corra_hub::hub::{Hub, HubRef, NewProject};
use corra_hub::providers::{CreateRequest, Description, Kind, Provider, ProviderStatus};
use corra_proto::runner::{HubMsg, RunnerRequest};
use corra_proto::*;
use tokio::sync::mpsc;

#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<String>>,
    broken: Mutex<bool>,
    snapshotting: tokio::sync::Notify,
    snapshotted: tokio::sync::Notify,
}

#[async_trait]
impl Provider for Fake {
    fn name(&self) -> &'static str {
        "fake"
    }
    fn describe(&self) -> Description {
        Description {
            label: "Fake",
            about: "",
            credentials: &[],
            kinds: vec![
                Kind {
                    name: "vm",
                    label: "VM",
                    about: "",
                    sizes: &[(4, 8), (8, 16)],
                    sleeps: "",
                    max_hours: None,
                    kvm: true,
                }
                .into(),
                Kind {
                    name: "box",
                    label: "Box",
                    about: "",
                    sizes: &[],
                    sleeps: "",
                    max_hours: None,
                    kvm: false,
                }
                .into(),
            ],
            listed: true,
        }
    }
    fn creates_machines(&self) -> bool {
        true
    }
    fn sleeps(&self) -> bool {
        true
    }
    async fn create(&self, _hub: &Hub, _req: CreateRequest) -> Result<String> {
        self.calls.lock().unwrap().push("create".into());
        Ok("droplet-1".into())
    }
    async fn sleep(&self, hub: &Hub, r: &str) -> Result<String> {
        self.calls.lock().unwrap().push(format!("sleep {r}"));
        hub.runners.lock().unwrap().clear();
        self.snapshotting.notify_one();
        self.snapshotted.notified().await;
        Ok("snap-1".into())
    }
    async fn wake(&self, _hub: &Hub, s: &str, _req: CreateRequest) -> Result<String> {
        self.calls.lock().unwrap().push(format!("wake {s}"));
        Ok("droplet-2".into())
    }
    async fn running(&self, _hub: &Hub) -> Result<Vec<String>> {
        Ok(vec!["droplet-2".into()])
    }
    async fn status(&self, _hub: &Hub, r: &str) -> Result<ProviderStatus> {
        Ok(if r == "droplet-1" {
            ProviderStatus::Gone
        } else {
            ProviderStatus::Running
        })
    }
    async fn destroy(&self, _hub: &Hub, r: &str) -> Result<()> {
        self.calls.lock().unwrap().push(format!("destroy {r}"));
        if *self.broken.lock().unwrap() {
            bail!("the provider is down");
        }
        Ok(())
    }
    async fn forget(&self, _hub: &Hub, r: &str) -> Result<()> {
        self.calls.lock().unwrap().push(format!("forget {r}"));
        if *self.broken.lock().unwrap() {
            bail!("the provider is down");
        }
        Ok(())
    }
}

fn setup(dir: &std::path::Path) -> (HubRef, Arc<Fake>) {
    let hub = corra_hub::build(corra_hub::Options {
        data: dir.to_path_buf(),
        public_url: "http://127.0.0.1:1".into(),
        dev_no_auth: false,
    })
    .unwrap()
    .state
    .hub;
    let fake = Arc::new(Fake::default());
    hub.providers.add(fake.clone());
    (hub, fake)
}

fn host(kind: &str) -> PhoneHost {
    PhoneHost {
        provider: "fake".into(),
        kind: kind.into(),
        size: "6x12".into(),
    }
}

fn running_phone(hub: &HubRef) -> MachineId {
    let m = hub
        .db
        .insert_machine(NewMachine {
            name: "corra-phone-x".into(),
            provider: "fake".into(),
            project: None,
            size: "8x16".into(),
            token_hash: "h".into(),
            provider_ref: Some("droplet-1".into()),
            status: MachineStatus::Connected,
            kind: Some("vm".into()),
        })
        .unwrap();
    hub.db.kv_set("phone.machine", &m.id.to_string()).unwrap();
    hub.db
        .kv_set(
            "phone.template",
            r#"{"host":{"provider":"fake","kind":"vm","size":"8x16"},"reference":"snap-1"}"#,
        )
        .unwrap();
    m.id
}

#[tokio::test]
async fn the_phone_needs_a_connected_kind_with_kvm() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, _) = setup(dir.path());
    let err = corra_hub::phone::switch(&hub, Some(host("box")))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("no KVM"), "{err}");
    let err = corra_hub::phone::switch(
        &hub,
        Some(PhoneHost {
            provider: "agent37".into(),
            kind: "vm".into(),
            size: "4x8".into(),
        }),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(err.contains("connect Agent37"), "{err}");
    let set = corra_hub::phone::switch(&hub, Some(host("vm")))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(set.size, "8x16");
    assert_eq!(corra_hub::phone::host(&hub), Some(set));
}

#[tokio::test]
async fn a_hub_with_a_digitalocean_key_and_no_choice_keeps_its_phone_there() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, _) = setup(dir.path());
    assert_eq!(corra_hub::phone::host(&hub), None);
    hub.secrets.set("digitalocean.token", "t").unwrap();
    assert_eq!(
        corra_hub::phone::host(&hub).map(|h| (h.provider, h.kind)),
        Some(("digitalocean".into(), "droplet".into()))
    );
    corra_hub::phone::switch(&hub, None).await.unwrap();
    assert_eq!(corra_hub::phone::host(&hub), None);
}

#[tokio::test]
async fn switching_the_phone_deletes_its_machine_and_template_and_forgets_them() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, fake) = setup(dir.path());
    corra_hub::phone::switch(&hub, Some(host("vm")))
        .await
        .unwrap();
    let m = running_phone(&hub);
    hub.phone.forwards.lock().unwrap().insert(3000, m);

    corra_hub::phone::switch(&hub, None).await.unwrap();
    assert_eq!(
        *fake.calls.lock().unwrap(),
        ["destroy droplet-1", "forget snap-1"]
    );
    assert_eq!(
        hub.db.machine(m).unwrap().unwrap().status,
        MachineStatus::Destroyed
    );
    assert!(corra_hub::phone::machine(&hub).is_none());
    assert!(hub.db.kv_get("phone.template").unwrap().is_none());
    assert!(hub.phone.forwards.lock().unwrap().is_empty());
    assert!(corra_hub::leftovers::list(&hub).is_empty());
}

#[tokio::test]
async fn what_couldnt_be_deleted_is_kept_as_a_leftover_until_a_retry_works() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, fake) = setup(dir.path());
    corra_hub::phone::switch(&hub, Some(host("vm")))
        .await
        .unwrap();
    running_phone(&hub);
    *fake.broken.lock().unwrap() = true;

    corra_hub::phone::switch(&hub, None).await.unwrap();
    let left = corra_hub::leftovers::list(&hub);
    let refs: Vec<(&str, LeftoverThing)> = left
        .iter()
        .map(|l| (l.reference.as_str(), l.thing))
        .collect();
    assert_eq!(
        refs,
        [
            ("droplet-1", LeftoverThing::Machine),
            ("snap-1", LeftoverThing::Image)
        ]
    );
    assert!(
        left.iter()
            .all(|l| l.provider == "fake" && l.error.contains("down"))
    );
    assert!(corra_hub::phone::machine(&hub).is_none());

    assert_eq!(corra_hub::leftovers::retry(&hub).await.len(), 2);
    *fake.broken.lock().unwrap() = false;
    assert!(corra_hub::leftovers::retry(&hub).await.is_empty());
    assert!(corra_hub::leftovers::list(&hub).is_empty());
}

#[tokio::test]
async fn a_project_moved_to_another_kind_lets_go_of_its_old_machine() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, fake) = setup(dir.path());
    let mut p = hub
        .create_project(NewProject {
            name: "p".into(),
            provider: Some("fake".into()),
            kind: Some("vm".into()),
            ..Default::default()
        })
        .unwrap();
    let old = hub
        .db
        .insert_machine(NewMachine {
            name: "corra-p-old".into(),
            provider: "fake".into(),
            project: Some(p.id),
            size: "4x8".into(),
            token_hash: "h".into(),
            provider_ref: Some("box-1".into()),
            status: MachineStatus::Asleep,
            kind: Some("vm".into()),
        })
        .unwrap();
    hub.db
        .set_machine_sleep_ref(old.id, Some("snap-old"))
        .unwrap();

    corra_hub::lifecycle::retire_moved(&hub, p.id);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        fake.calls.lock().unwrap().is_empty(),
        "the current machine was touched"
    );

    hub.place_project(&mut p, None, Some("box".into()), None)
        .unwrap();
    hub.db.update_project(&p).unwrap();
    corra_hub::lifecycle::retire_moved(&hub, p.id);
    for _ in 0..100 {
        if hub.db.machine(old.id).unwrap().unwrap().status == MachineStatus::Destroyed {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        hub.db.machine(old.id).unwrap().unwrap().status,
        MachineStatus::Destroyed
    );
    assert_eq!(
        *fake.calls.lock().unwrap(),
        ["destroy box-1", "forget snap-old"]
    );
}

#[tokio::test]
async fn a_machine_whose_provider_is_gone_can_be_forgotten() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, fake) = setup(dir.path());
    let m = hub
        .db
        .insert_machine(NewMachine {
            name: "corra-phone".into(),
            provider: "phone".into(),
            project: None,
            size: "8x16".into(),
            token_hash: "h".into(),
            provider_ref: Some("77".into()),
            status: MachineStatus::Connected,
            kind: None,
        })
        .unwrap();
    hub.providers.destroy_machine(&hub, m.id).await.unwrap();
    assert_eq!(
        hub.db.machine(m.id).unwrap().unwrap().status,
        MachineStatus::Destroyed
    );
    assert!(fake.calls.lock().unwrap().is_empty());
}

fn attach_runner(hub: &HubRef, m: MachineId) -> mpsc::UnboundedReceiver<HubMsg> {
    let (tx, rx) = mpsc::unbounded_channel();
    hub.runners.lock().unwrap().insert(
        m,
        RunnerConn {
            tx,
            live: Default::default(),
            prepared: Default::default(),
            conn_id: 0,
        },
    );
    hub.db
        .set_machine_status(m, MachineStatus::Connected)
        .unwrap();
    rx
}

async fn answer(hub: &HubRef, m: MachineId, rx: &mut mpsc::UnboundedReceiver<HubMsg>) -> PhoneOp {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("the hub should ask the phone's machine something")
            .unwrap();
        if let HubMsg::Request {
            id,
            body: RunnerRequest::Phone { action },
        } = msg
        {
            hub.resolve_pending(m, id, Ok(serde_json::json!({})));
            return action;
        }
    }
}

#[tokio::test]
async fn the_sweep_leaves_the_phone_alone_while_its_first_start_makes_a_template() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, fake) = setup(dir.path());
    corra_hub::phone::switch(&hub, Some(host("vm")))
        .await
        .unwrap();
    let up = {
        let hub = hub.clone();
        tokio::spawn(async move { corra_hub::phone::up(&hub).await })
    };
    let m = loop {
        if let Some(m) = corra_hub::phone::machine(&hub) {
            break m.id;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let mut rx = attach_runner(&hub, m);
    assert!(matches!(answer(&hub, m, &mut rx).await, PhoneOp::Setup));

    tokio::time::timeout(Duration::from_secs(10), fake.snapshotting.notified())
        .await
        .unwrap();
    corra_hub::lifecycle::sweep(&hub).await.unwrap();
    let day_ago = now_ms() - 24 * 3600 * 1000;
    rusqlite::Connection::open(dir.path().join("corra.db"))
        .unwrap()
        .execute(
            "UPDATE machines SET created_at = ?1, last_seen = ?1 WHERE id = ?2",
            rusqlite::params![day_ago, m.to_string()],
        )
        .unwrap();
    corra_hub::lifecycle::sweep(&hub).await.unwrap();
    fake.snapshotted.notify_one();

    for _ in 0..500 {
        if hub.db.machine_provider_ref(m).unwrap().as_deref() == Some("droplet-2") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    corra_hub::lifecycle::sweep(&hub).await.unwrap();
    corra_hub::lifecycle::orphans(&hub).await;
    let mut rx = attach_runner(&hub, m);
    assert!(matches!(answer(&hub, m, &mut rx).await, PhoneOp::Boot));

    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), up)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        m
    );
    assert_eq!(
        *fake.calls.lock().unwrap(),
        ["create", "sleep droplet-1", "wake snap-1"]
    );
    assert_eq!(corra_hub::phone::machine(&hub).map(|m| m.id), Some(m));
    assert_eq!(
        hub.db.machine(m).unwrap().unwrap().status,
        MachineStatus::Connected
    );
}
