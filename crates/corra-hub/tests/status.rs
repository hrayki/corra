use std::time::Duration;

use corra_hub::hub::HubRef;
use corra_proto::*;

fn text(s: &str) -> Vec<InputPart> {
    vec![InputPart::Text { text: s.into() }]
}

fn setup(dir: &std::path::Path) -> (HubRef, ThreadId) {
    let hub = corra_hub::build(corra_hub::Options {
        data: dir.to_path_buf(),
        public_url: "http://127.0.0.1:1".into(),
        dev_no_auth: false,
    })
    .unwrap()
    .state
    .hub;
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
        status: ThreadStatus::Idle,
        last_seq: 0,
        created_at: 0,
        updated_at: 0,
        pinned_at: None,
        settled_at: None,
        hidden_at: None,
        started_by: None,
    };
    hub.db.insert_thread(&t).unwrap();
    (hub, t.id)
}

fn turn(hub: &HubRef, thread: ThreadId, id: &str, outcome: Option<TurnOutcome>) {
    let event = match outcome {
        None => Event::TurnStarted {
            turn: id.into(),
            input: vec![],
            settings: AgentSettings::default(),
            cause: None,
        },
        Some(outcome) => Event::TurnCompleted {
            turn: id.into(),
            outcome,
            usage: None,
            error: None,
            anchor: None,
            resets_at: None,
        },
    };
    hub.append(thread, None, now_ms(), event).unwrap();
}

fn status(hub: &HubRef, thread: ThreadId) -> ThreadStatus {
    hub.db.thread(thread).unwrap().unwrap().status
}

fn ask(
    hub: &HubRef,
    thread: ThreadId,
    turn: &str,
    action: &str,
) -> tokio::task::JoinHandle<RequestResponse> {
    let hub = hub.clone();
    let (turn, action) = (turn.to_string(), action.to_string());
    tokio::spawn(async move {
        hub.ask(thread, &turn, &action, &serde_json::json!({}))
            .await
            .unwrap()
    })
}

async fn open(hub: &HubRef, thread: ThreadId, n: usize) -> Vec<String> {
    for _ in 0..100 {
        let ids: Vec<String> = hub
            .db
            .open_requests(thread)
            .unwrap()
            .into_iter()
            .filter_map(|e| match e.event {
                Event::Request { id, .. } => Some(id),
                _ => None,
            })
            .collect();
        if ids.len() >= n {
            return ids;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("fewer than {n} requests opened");
}

fn allow() -> RequestResponse {
    RequestResponse::Allow { always: false }
}

async fn tried_to_send(hub: &HubRef, thread: ThreadId) -> usize {
    tokio::time::sleep(Duration::from_millis(100)).await;
    hub.db
        .events_after(thread, 0, 1000)
        .unwrap()
        .iter()
        .filter(
            |e| matches!(&e.event, Event::Notice { text, .. } if text.starts_with("Couldn't send")),
        )
        .count()
}

#[tokio::test]
async fn a_yes_given_after_its_turn_ended_leaves_the_thread_idle_and_sends_what_waited() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, thread) = setup(dir.path());
    turn(&hub, thread, "a", None);
    turn(&hub, thread, "a", Some(TurnOutcome::Completed));

    let push = ask(&hub, thread, "a", "git.push");
    let id = open(&hub, thread, 1).await.remove(0);
    assert_eq!(status(&hub, thread), ThreadStatus::Waiting);
    hub.send(thread, text("next"), None, true).await.unwrap();
    assert_eq!(hub.queued(thread).len(), 1);

    hub.respond(thread, id, allow()).unwrap();
    assert!(matches!(push.await.unwrap(), RequestResponse::Allow { .. }));
    assert_eq!(
        tried_to_send(&hub, thread).await,
        1,
        "the queue didn't move"
    );
    assert!(hub.queued(thread).is_empty());
}

#[tokio::test]
async fn a_yes_after_a_failed_turn_shows_the_error_and_keeps_the_queue() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, thread) = setup(dir.path());
    turn(&hub, thread, "a", None);
    turn(&hub, thread, "a", Some(TurnOutcome::Failed));

    let _push = ask(&hub, thread, "a", "git.push");
    let id = open(&hub, thread, 1).await.remove(0);
    hub.send(thread, text("next"), None, true).await.unwrap();
    hub.respond(thread, id, allow()).unwrap();
    assert_eq!(status(&hub, thread), ThreadStatus::Error);
    assert_eq!(tried_to_send(&hub, thread).await, 0);
    assert_eq!(hub.queued(thread).len(), 1);
}

#[tokio::test]
async fn a_yes_mid_turn_shows_running_unless_another_request_still_waits() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, thread) = setup(dir.path());
    turn(&hub, thread, "a", None);
    let _one = ask(&hub, thread, "a", "github.pr_merge");
    let _two = ask(&hub, thread, "a", "github.pr_merge");
    let ids = open(&hub, thread, 2).await;

    hub.respond(thread, ids[0].clone(), allow()).unwrap();
    assert_eq!(status(&hub, thread), ThreadStatus::Waiting);
    hub.respond(thread, ids[1].clone(), allow()).unwrap();
    assert_eq!(status(&hub, thread), ThreadStatus::Running);
}

#[tokio::test]
async fn stopping_a_turn_cancels_its_merge_and_secret_requests_but_not_a_push() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, thread) = setup(dir.path());
    turn(&hub, thread, "a", None);
    let merge = ask(&hub, thread, "a", "github.pr_merge");
    let push = ask(&hub, thread, "a", "git.push");
    let secret = {
        let hub = hub.clone();
        tokio::spawn(async move { hub.ask_secret(thread, "a", "KEY", "why").await.unwrap() })
    };
    open(&hub, thread, 3).await;

    turn(&hub, thread, "a", Some(TurnOutcome::Interrupted));
    let soon = Duration::from_secs(2);
    assert_eq!(
        tokio::time::timeout(soon, merge)
            .await
            .expect("the merge still waits")
            .unwrap(),
        RequestResponse::Cancelled
    );
    assert_eq!(
        tokio::time::timeout(soon, secret)
            .await
            .expect("the secret still waits")
            .unwrap(),
        None
    );
    let left = open(&hub, thread, 1).await;
    assert_eq!(left.len(), 1, "{left:?}");
    assert!(!push.is_finished());

    hub.respond(thread, left[0].clone(), allow()).unwrap();
    assert!(matches!(push.await.unwrap(), RequestResponse::Allow { .. }));
    assert_eq!(status(&hub, thread), ThreadStatus::Idle);
}
