use std::time::Duration;

use corra_proto::runner::{HubMsg, RunnerMsg};
use corra_proto::*;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

async fn next(ws: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) -> RunnerMsg {
    loop {
        let m = tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .expect("the runner went quiet")
            .expect("the runner hung up")
            .unwrap();
        if let Message::Text(t) = m {
            return serde_json::from_str(&t).unwrap();
        }
    }
}

async fn send(ws: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>, m: &HubMsg) {
    ws.send(Message::Text(serde_json::to_string(m).unwrap().into()))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_message_for_a_thread_the_runner_doesnt_have_is_handed_back_not_dropped() {
    let home = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", home.path()) };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let runner = tokio::spawn(corra_runner::run(corra_runner::Options {
        hub: format!("http://{addr}"),
        token: "t".into(),
        sync: false,
        update: false,
        connected: None,
    }));
    let (sock, _) = listener.accept().await.unwrap();
    let mut ws = tokio_tungstenite::accept_async(sock).await.unwrap();
    let RunnerMsg::Hello { takes, prompt, .. } = next(&mut ws).await else {
        panic!("no hello");
    };
    assert!(takes);
    assert!(prompt);
    send(
        &mut ws,
        &HubMsg::Welcome {
            machine: new_id(),
            acks: vec![],
            runner: None,
            git_token: None,
        },
    )
    .await;
    for _ in 0..2 {
        send(
            &mut ws,
            &HubMsg::Send {
                thread: new_id(),
                input: vec![InputPart::Text { text: "hi".into() }],
                settings: None,
                delivery: Some("d1".into()),
            },
        )
        .await;
        let took = loop {
            if let RunnerMsg::Took { delivery, taken } = next(&mut ws).await {
                break (delivery, taken);
            }
        };
        assert_eq!(took, ("d1".to_string(), false));
    }
    send(
        &mut ws,
        &HubMsg::Request {
            id: 42,
            body: corra_proto::runner::RunnerRequest::Stop { thread: new_id() },
        },
    )
    .await;
    let reply_id = loop {
        if let RunnerMsg::Reply { id, err, .. } = next(&mut ws).await {
            assert!(err.is_none(), "stop request returned error: {err:?}");
            break id;
        }
    };
    assert_eq!(reply_id, 42);
    runner.abort();
}
