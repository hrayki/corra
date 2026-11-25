use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use corra_hub::db::NewMachine;
use corra_hub::hub::{Hub, HubRef, NewProject};
use corra_hub::providers::{CreateRequest, Description, Kind, Provider, ProviderStatus};
use corra_proto::*;

#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<String>>,
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
                    name: "big",
                    label: "Big",
                    about: "",
                    sizes: &[(2, 4), (4, 8), (8, 64)],
                    sleeps: "",
                    max_hours: None,
                    kvm: true,
                }
                .into(),
                Kind {
                    name: "small",
                    label: "Small",
                    about: "",
                    sizes: &[(2, 4), (4, 8)],
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
    async fn create(&self, _hub: &Hub, req: CreateRequest) -> Result<String> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("create {}", req.kind));
        Ok("new".into())
    }
    async fn wake(&self, _hub: &Hub, sleep_ref: &str, req: CreateRequest) -> Result<String> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("wake {sleep_ref} {}", req.kind));
        Ok(sleep_ref.into())
    }
    async fn status(&self, _hub: &Hub, _provider_ref: &str) -> Result<ProviderStatus> {
        Ok(ProviderStatus::Gone)
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

#[tokio::test]
async fn a_project_gets_a_kind_and_a_size_the_kind_offers() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, _) = setup(dir.path());
    let mut p = hub
        .create_project(NewProject {
            name: "p".into(),
            provider: Some("fake".into()),
            size: Some("8x64".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(p.kind.as_deref(), Some("big"));
    assert_eq!(p.size, "8x64");

    let err = hub
        .place_project(&mut p, None, Some("huge".into()), None)
        .unwrap_err()
        .to_string();
    assert!(err.contains("big, small"), "{err}");

    p.default_machine = Some(new_id());
    hub.place_project(&mut p, None, Some("small".into()), None)
        .unwrap();
    assert_eq!(p.kind.as_deref(), Some("small"));
    assert_eq!(p.size, "4x8");
    assert_eq!(p.default_machine, None);

    hub.place_project(&mut p, Some("modal".into()), None, Some("6x12".into()))
        .unwrap();
    assert_eq!(
        (p.provider.as_str(), p.kind.as_deref()),
        ("modal", Some("vm"))
    );
    assert_eq!(p.size, "6x12");
}

#[tokio::test]
async fn a_machine_asleep_as_another_kind_is_not_woken() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, fake) = setup(dir.path());
    let p = hub
        .create_project(NewProject {
            name: "p".into(),
            provider: Some("fake".into()),
            kind: Some("small".into()),
            ..Default::default()
        })
        .unwrap();
    for kind in [None, Some("big")] {
        let m = hub
            .db
            .insert_machine(NewMachine {
                name: format!("old-{kind:?}"),
                provider: "fake".into(),
                project: Some(p.id),
                size: "4x8".into(),
                token_hash: format!("h{kind:?}"),
                provider_ref: None,
                status: MachineStatus::Asleep,
                kind: kind.map(String::from),
            })
            .unwrap();
        hub.db.set_machine_sleep_ref(m.id, Some("snap")).unwrap();
    }
    let _ = hub.providers.start_for_project(&hub, &p).await;
    assert_eq!(*fake.calls.lock().unwrap(), ["create small"]);

    let mut p = p;
    hub.place_project(&mut p, None, Some("big".into()), None)
        .unwrap();
    fake.calls.lock().unwrap().clear();
    let _ = hub.providers.start_for_project(&hub, &p).await;
    assert_eq!(*fake.calls.lock().unwrap(), ["wake snap big"]);
}
