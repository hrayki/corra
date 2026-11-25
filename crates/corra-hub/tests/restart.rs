use corra_proto::*;
use serde_json::json;

fn open(dir: &std::path::Path) -> corra_hub::hub::HubRef {
    corra_hub::build(corra_hub::Options {
        data: dir.to_path_buf(),
        public_url: "http://127.0.0.1:1".into(),
        dev_no_auth: false,
    })
    .unwrap()
    .state
    .hub
}

fn request(id: &str) -> Event {
    Event::Request {
        id: id.into(),
        turn: "turn".into(),
        request: AgentRequest::HubAction {
            action: "github.pr.merge".into(),
            input: json!({}),
        },
    }
}

#[tokio::test]
async fn an_approval_left_open_by_a_restart_is_closed_with_a_notice() {
    let dir = tempfile::tempdir().unwrap();
    let thread = {
        let hub = open(dir.path());
        let p = hub
            .create_project(corra_hub::hub::NewProject {
                name: "p".into(),
                provider: Some("existing".into()),
                ..Default::default()
            })
            .unwrap();
        let t = Thread {
            id: new_id(),
            project: p.id,
            title: "t".into(),
            agent: AgentKind::Claude,
            machine: None,
            session_id: None,
            cwd: "~".into(),
            settings: AgentSettings::default(),
            status: ThreadStatus::Running,
            last_seq: 0,
            created_at: 0,
            updated_at: 0,
            pinned_at: None,
            settled_at: None,
            hidden_at: None,
        };
        hub.db.insert_thread(&t).unwrap();
        for e in [
            request("hub-answered"),
            Event::RequestResolved {
                id: "hub-answered".into(),
                response: RequestResponse::Allow { always: false },
            },
            request("perm-from-claude"),
            request("hub-open"),
        ] {
            hub.append(t.id, None, now_ms(), e).unwrap();
        }
        assert_eq!(
            hub.db.thread(t.id).unwrap().unwrap().status,
            ThreadStatus::Waiting
        );
        t.id
    };

    let hub = open(dir.path());
    let events = hub.db.events_after(thread, 0, 100).unwrap();
    let closed: Vec<&str> = events
        .iter()
        .filter_map(|e| match &e.event {
            Event::RequestResolved {
                id,
                response: RequestResponse::Cancelled,
            } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(closed, ["hub-open"]);
    assert!(matches!(
        &events.last().unwrap().event,
        Event::Notice { text, .. } if text.contains("hub restarted")
    ));
}
