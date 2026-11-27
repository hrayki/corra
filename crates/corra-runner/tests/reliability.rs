use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use corra_proto::runner::{RunnerMsg, RunnerRequest};
use corra_proto::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

type StoreMap = Arc<Mutex<HashMap<(String, String), Vec<u8>>>>;
type PutLog = Arc<Mutex<Vec<(String, String, u64, bool)>>>;

#[tokio::test]
async fn sync_replaces_mutable_same_size_files_and_appends_jsonl_without_blocking_other_threads() {
    let store: StoreMap = Default::default();
    let put_log: PutLog = Default::default();
    let slow_started = Arc::new(tokio::sync::Notify::new());
    let release_slow = Arc::new(tokio::sync::Notify::new());

    let t_slow = new_id();
    let t_fast = new_id();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = {
        let store = store.clone();
        let put_log = put_log.clone();
        let slow_started = slow_started.clone();
        let release_slow = release_slow.clone();
        let slow_id = t_slow.to_string();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let store = store.clone();
                let put_log = put_log.clone();
                let slow_started = slow_started.clone();
                let release_slow = release_slow.clone();
                let slow_id = slow_id.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 64 * 1024];
                    let mut got = Vec::new();
                    let header_end = loop {
                        let n = sock.read(&mut buf).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        got.extend_from_slice(&buf[..n]);
                        if let Some(pos) = got.windows(4).position(|w| w == b"\r\n\r\n") {
                            break pos + 4;
                        }
                    };
                    let head = String::from_utf8_lossy(&got[..header_end]).to_string();
                    let content_len: usize = head
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case("content-length")
                                .then(|| v.trim().parse().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    while got.len() - header_end < content_len {
                        let n = sock.read(&mut buf).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        got.extend_from_slice(&buf[..n]);
                    }
                    let body = got[header_end..header_end + content_len].to_vec();
                    let first = head.lines().next().unwrap_or("");
                    let mut parts = first.split_whitespace();
                    let method = parts.next().unwrap_or("");
                    let target = parts.next().unwrap_or("");
                    let (path, query) = target.split_once('?').unwrap_or((target, ""));
                    let params: HashMap<&str, &str> = query
                        .split('&')
                        .filter_map(|kv| kv.split_once('='))
                        .collect();

                    if method == "GET" && path.starts_with("/runner/files/") {
                        let tid = path.trim_start_matches("/runner/files/");
                        let list: Vec<serde_json::Value> = store
                            .lock()
                            .unwrap()
                            .iter()
                            .filter(|((t, _), _)| t == tid)
                            .map(|((_, p), b)| serde_json::json!({ "path": p, "size": b.len() }))
                            .collect();
                        let resp_body = serde_json::to_vec(&list).unwrap();
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            resp_body.len()
                        );
                        let _ = sock.write_all(resp.as_bytes()).await;
                        let _ = sock.write_all(&resp_body).await;
                    } else if method == "PUT" && path.starts_with("/runner/file/") {
                        let tid = path.trim_start_matches("/runner/file/").to_string();
                        let rel = params.get("path").copied().unwrap_or("").to_string();
                        let offset: u64 = params
                            .get("offset")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(0);
                        let replace = params.get("replace").copied() == Some("true");
                        if tid == slow_id && rel == "slow.db" {
                            slow_started.notify_one();
                            release_slow.notified().await;
                        }
                        put_log
                            .lock()
                            .unwrap()
                            .push((tid.clone(), rel.clone(), offset, replace));
                        {
                            let mut st = store.lock().unwrap();
                            let entry = st.entry((tid, rel)).or_default();
                            if replace {
                                *entry = body;
                            } else {
                                entry.extend_from_slice(&body);
                            }
                        }
                        let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
                        let _ = sock.write_all(resp.as_bytes()).await;
                    } else if method == "GET" && path.starts_with("/runner/file/") {
                        let tid = path.trim_start_matches("/runner/file/").to_string();
                        let rel = params.get("path").copied().unwrap_or("").to_string();
                        let data = store.lock().unwrap().get(&(tid, rel)).cloned();
                        if let Some(data) = data {
                            let resp = format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                data.len()
                            );
                            let _ = sock.write_all(resp.as_bytes()).await;
                            let _ = sock.write_all(&data).await;
                        } else {
                            let _ = sock
                                .write_all(
                                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                                )
                                .await;
                        }
                    }
                });
            }
        })
    };

    let sync = corra_runner::sync::Syncer::new(&format!("http://{addr}"), "tok");
    let dir = tempfile::tempdir().unwrap();
    let db_file = dir.path().join("state.db");
    let log_file = dir.path().join("session.jsonl");
    tokio::fs::write(&db_file, b"AAAA1111").await.unwrap();
    tokio::fs::write(&log_file, b"line1\n").await.unwrap();

    sync.upload(t_fast, dir.path(), vec![db_file.clone(), log_file.clone()])
        .await
        .unwrap();
    assert_eq!(
        store
            .lock()
            .unwrap()
            .get(&(t_fast.to_string(), "state.db".into()))
            .unwrap(),
        b"AAAA1111"
    );

    // Mutate state.db in place with the EXACT SAME byte size (SQLite page overwrite!)
    tokio::fs::write(&db_file, b"BBBB2222").await.unwrap();
    // Append to .jsonl
    tokio::fs::write(&log_file, b"line1\nline2\n")
        .await
        .unwrap();
    sync.upload(t_fast, dir.path(), vec![db_file.clone(), log_file.clone()])
        .await
        .unwrap();

    assert_eq!(
        store
            .lock()
            .unwrap()
            .get(&(t_fast.to_string(), "state.db".into()))
            .unwrap(),
        b"BBBB2222",
        "in-place same-size mutable file edit was skipped or corrupted"
    );
    assert_eq!(
        store
            .lock()
            .unwrap()
            .get(&(t_fast.to_string(), "session.jsonl".into()))
            .unwrap(),
        b"line1\nline2\n"
    );
    {
        let log = put_log.lock().unwrap();
        let db_puts: Vec<_> = log.iter().filter(|(_, p, _, _)| p == "state.db").collect();
        assert_eq!(db_puts.len(), 2);
        assert!(
            db_puts
                .iter()
                .all(|(_, _, offset, replace)| *offset == 0 && *replace)
        );
        let jsonl_puts: Vec<_> = log
            .iter()
            .filter(|(_, p, _, _)| p == "session.jsonl")
            .collect();
        assert_eq!(jsonl_puts.len(), 2);
        assert_eq!(jsonl_puts[1].2, 6);
        assert!(!jsonl_puts[1].3);
    }

    // Uploading again without changing state.db should be a no-op (hash matched)
    sync.upload(t_fast, dir.path(), vec![db_file.clone()])
        .await
        .unwrap();
    assert_eq!(
        put_log
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, p, _, _)| p == "state.db")
            .count(),
        2
    );

    // Restore: force_mutable=false preserves existing mutable file, force_mutable=true overwrites it
    tokio::fs::write(&db_file, b"STALE000").await.unwrap();
    assert_eq!(sync.restore(t_fast, dir.path(), false).await.unwrap(), 0);
    assert_eq!(tokio::fs::read(&db_file).await.unwrap(), b"STALE000");
    assert_eq!(sync.restore(t_fast, dir.path(), true).await.unwrap(), 1);
    assert_eq!(tokio::fs::read(&db_file).await.unwrap(), b"BBBB2222");

    // Concurrency: slow upload on t_slow MUST NOT block upload on t_fast!
    let slow_file = dir.path().join("slow.db");
    let fast2 = dir.path().join("other.db");
    tokio::fs::write(&slow_file, b"slow").await.unwrap();
    tokio::fs::write(&fast2, b"fast").await.unwrap();
    let sync2 = sync.clone();
    let dir_path = dir.path().to_path_buf();
    let slow_task =
        tokio::spawn(async move { sync2.upload(t_slow, &dir_path, vec![slow_file]).await });
    slow_started.notified().await;
    tokio::time::timeout(
        Duration::from_secs(2),
        sync.upload(t_fast, dir.path(), vec![fast2]),
    )
    .await
    .expect("upload on t_fast was blocked by t_slow")
    .unwrap();
    release_slow.notify_one();
    slow_task.await.unwrap().unwrap();
    server.abort();
}

#[tokio::test]
async fn fast_exiting_terminal_flushes_all_output_and_cleans_up_its_entry() {
    let dir = tempfile::tempdir().unwrap();
    let terms = corra_runner::term::Terms::default();
    let (tx, mut rx) = mpsc::unbounded_channel();
    terms
        .open_command(
            "fast-1".into(),
            "sh",
            &["-c", "printf 'FINAL_OUTPUT_MARKER\\n'"],
            dir.path(),
            (80, 24),
            tx,
        )
        .unwrap();

    let mut output = String::new();
    let mut exited = false;
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some((msg, _permit)) = rx.recv().await {
            match msg {
                RunnerMsg::TermOutput { term, data } if term == "fast-1" => {
                    let bytes = STANDARD.decode(data).unwrap();
                    output.push_str(&String::from_utf8_lossy(&bytes));
                }
                RunnerMsg::TermExit { term } if term == "fast-1" => {
                    exited = true;
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("terminal never exited");
    assert!(exited);
    assert!(
        output.contains("FINAL_OUTPUT_MARKER"),
        "final output lost before TermExit: {output:?}"
    );
    assert!(
        terms.is_empty(),
        "fast-exiting terminal leaked its entry in Terms"
    );
}

#[tokio::test]
async fn git_sends_both_scoped_auth_header_and_thread_header_from_env() {
    let dir = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hub_url = format!("http://{addr}");

    let captured = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let mut req = Vec::new();
        loop {
            let n = sock.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            req.extend_from_slice(&buf[..n]);
            if req.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let _ = sock
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await;
        String::from_utf8_lossy(&req).to_string()
    });

    // Init a local git repo and configure the scoped git token via ensure_repos
    let repo_dir = dir.path().join("r");
    let status = tokio::process::Command::new("git")
        .arg("init")
        .arg("-q")
        .arg(&repo_dir)
        .status()
        .await
        .unwrap();
    assert!(status.success());
    let repos = vec![Repo {
        name: "r".into(),
        github: Some("acme/app".into()),
    }];
    corra_runner::git::ensure_repos(dir.path(), &hub_url, "cg_scoped_tok", &repos).await;

    let tid = new_id();
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("-C")
        .arg(&repo_dir)
        .arg("ls-remote")
        .arg(format!("{hub_url}/git/acme/app.git"));
    corra_runner::git::thread_header_env(&mut cmd, &hub_url, tid);
    let _ = cmd.output().await.unwrap();

    let raw_req = tokio::time::timeout(Duration::from_secs(5), captured)
        .await
        .expect("git never connected")
        .unwrap();
    let lower = raw_req.to_ascii_lowercase();
    assert!(
        lower.contains("authorization: bearer cg_scoped_tok"),
        "missing scoped git auth header: {raw_req}"
    );
    assert!(
        lower.contains(&format!("x-corra-thread: {tid}")),
        "missing x-corra-thread header: {raw_req}"
    );
}

#[tokio::test]
async fn read_file_streams_only_up_to_max_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let big = dir.path().join("big.txt");
    tokio::fs::write(&big, vec![b'a'; 100_000]).await.unwrap();
    let v = corra_runner::files::handle(
        RunnerRequest::ReadFile {
            path: big.to_string_lossy().into(),
            max_bytes: 128,
        },
        "",
        "",
        "",
        None,
    )
    .await
    .unwrap();
    assert_eq!(v["size"].as_u64(), Some(100_000));
    assert_eq!(v["truncated"].as_bool(), Some(true));
    assert_eq!(v["text"].as_str().unwrap().len(), 128);
}
