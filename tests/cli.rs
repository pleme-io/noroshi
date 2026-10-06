use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

struct Captured {
    request_line: String,
    body: serde_json::Value,
}

fn mock_server(requests: usize) -> (String, mpsc::Receiver<Captured>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for stream in listener.incoming().take(requests) {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line.trim().is_empty() {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}")
                .unwrap();
            tx.send(Captured {
                request_line: request_line.trim().to_string(),
                body: serde_json::from_slice(&body).unwrap(),
            })
            .unwrap();
        }
    });
    (url, rx)
}

fn setup(server: &str, events: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("topic"), "unit-test-topic_42\n").unwrap();
    let config = serde_json::json!({
        "host": "testhost",
        "mention": "",
        "events": serde_json::from_str::<serde_json::Value>(events).unwrap(),
        "ntfy": {
            "server": server,
            "topic_file": dir.path().join("topic"),
            "priorities": { "Stop": 5, "Notification": 4 },
            "tags": { "Stop": ["white_check_mark"] },
            "titles": {}
        }
    });
    std::fs::write(dir.path().join("noroshi.yaml"), config.to_string()).unwrap();
    dir
}

fn noroshi(dir: &TempDir, args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("noroshi").unwrap();
    cmd.arg("--config")
        .arg(dir.path().join("noroshi.yaml"))
        .args(args);
    cmd
}

#[test]
fn hook_posts_a_signalled_event_to_ntfy_and_prints_nothing() {
    let (url, rx) = mock_server(1);
    let dir = setup(&url, r#"{"Stop":"signal","SessionEnd":"silent"}"#);
    noroshi(&dir, &["hook"])
        .write_stdin(r#"{"hook_event_name":"Stop","session_id":"9a8b7c6d-0000","last_assistant_message":"secret code"}"#)
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
    let got = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert_eq!(got.request_line, "POST / HTTP/1.1");
    assert_eq!(got.body["topic"], "unit-test-topic_42");
    assert_eq!(got.body["priority"], 5);
    assert_eq!(got.body["message"], "testhost · Stop · session 9a8b7c6d");
    assert!(!got.body.to_string().contains("secret code"));
}

#[test]
fn hook_is_silent_for_a_silent_or_unlisted_event() {
    let (url, rx) = mock_server(1);
    let dir = setup(&url, r#"{"Stop":"signal","SessionEnd":"silent"}"#);
    for event in ["SessionEnd", "SubagentStop"] {
        noroshi(&dir, &["hook"])
            .write_stdin(format!(
                r#"{{"hook_event_name":"{event}","session_id":"s"}}"#
            ))
            .assert()
            .success()
            .stdout(predicate::str::is_empty());
    }
    assert!(
        rx.recv_timeout(std::time::Duration::from_millis(300))
            .is_err()
    );
}

#[test]
fn hook_never_fails_the_session_even_when_delivery_fails() {
    let dir = setup("http://127.0.0.1:9", r#"{"Stop":"signal"}"#);
    noroshi(&dir, &["hook"])
        .write_stdin(r#"{"hook_event_name":"Stop"}"#)
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
    noroshi(&dir, &["hook"])
        .write_stdin("not json")
        .assert()
        .success();
}

#[test]
fn test_subcommand_sends_one_alert_with_the_event_priority() {
    let (url, rx) = mock_server(1);
    let dir = setup(&url, r#"{"Notification":"signal"}"#);
    noroshi(&dir, &["test", "--event", "Notification"])
        .assert()
        .success()
        .stdout(predicate::str::contains("backend ntfy: sent (HTTP 200)"));
    let got = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert_eq!(got.body["priority"], 4);
    assert_eq!(
        got.body["message"],
        "testhost · Notification · session test"
    );
}

#[test]
fn check_reports_readiness_without_printing_the_topic() {
    let dir = setup("https://ntfy.sh", r#"{"Stop":"signal"}"#);
    noroshi(&dir, &["check"])
        .assert()
        .success()
        .stdout(predicate::str::contains("backend ntfy: ready"))
        .stdout(predicate::str::contains("unit-test-topic").not());
    std::fs::remove_file(dir.path().join("topic")).unwrap();
    noroshi(&dir, &["check"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("is missing"));
}
