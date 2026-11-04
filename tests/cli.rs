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

#[test]
fn send_delivers_a_composed_message_and_refuses_a_bad_one() {
    let (url, rx) = mock_server(1);
    let dir = setup(&url, r#"{}"#);
    noroshi(&dir, &["send"])
        .write_stdin(r#"{"title":"plantao: rio sui failed","message":"origin down","priority":5,"tags":["rotating_light"],"click":"https://rio.example/x"}"#)
        .assert()
        .success()
        .stdout(predicate::str::contains("unit-test-topic").not());
    let got = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert_eq!(got.body["title"], "plantao: rio sui failed");
    assert_eq!(got.body["priority"], 5);
    assert_eq!(got.body["click"], "https://rio.example/x");
    noroshi(&dir, &["send"])
        .write_stdin(r#"{"title":"x","message":"y","priority":0}"#)
        .assert()
        .failure();
}

fn webhook_setup(url: &str, hosts: Option<&[&str]>) -> TempDir {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("hook"), format!("{url}\n")).unwrap();
    let mut config = serde_json::json!({
        "host": "testhost",
        "mention": "",
        "webhook_url_file": dir.path().join("hook"),
    });
    if let Some(hosts) = hosts {
        config["webhook_hosts"] = serde_json::json!(hosts);
    }
    std::fs::write(dir.path().join("noroshi.yaml"), config.to_string()).unwrap();
    dir
}

const REFUSED_URL: &str = "https://evil.example/collect-zq7xv";

#[test]
fn check_refuses_a_webhook_host_outside_the_list_without_echoing_the_secret() {
    let dir = webhook_setup(REFUSED_URL, None);
    noroshi(&dir, &["check"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("backend discord: cannot deliver"))
        .stdout(predicate::str::contains("webhook_hosts"))
        .stdout(predicate::str::contains("zq7xv").not())
        .stdout(predicate::str::contains("evil.example").not());
}

#[test]
fn hook_refuses_a_webhook_host_outside_the_list_and_still_exits_zero() {
    let dir = webhook_setup(REFUSED_URL, None);
    noroshi(&dir, &["hook"])
        .write_stdin(r#"{"hook_event_name":"Stop"}"#)
        .assert()
        .success()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("webhook_hosts"))
        .stderr(predicate::str::contains("zq7xv").not());
}

#[test]
fn send_and_test_refuse_a_webhook_host_outside_the_list() {
    let dir = webhook_setup(REFUSED_URL, None);
    noroshi(&dir, &["send"])
        .write_stdin(r#"{"title":"x","message":"y"}"#)
        .assert()
        .failure()
        .stderr(predicate::str::contains("webhook_hosts"));
    noroshi(&dir, &["test"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("webhook_hosts"));
}

#[test]
fn a_discord_webhook_is_ready_by_default_and_another_host_only_by_name() {
    let dir = webhook_setup("https://discord.com/api/webhooks/1/x", None);
    noroshi(&dir, &["check"])
        .assert()
        .success()
        .stdout(predicate::str::contains("backend discord: ready"));
    let dir = webhook_setup("https://hooks.example/any/path", Some(&["hooks.example"]));
    noroshi(&dir, &["check"])
        .assert()
        .success()
        .stdout(predicate::str::contains("backend discord: ready"));
    let dir = webhook_setup(
        "https://discord.com/api/webhooks/1/x",
        Some(&["hooks.example"]),
    );
    noroshi(&dir, &["check"]).assert().failure();
}

#[test]
fn the_environment_can_allow_another_host() {
    let dir = webhook_setup("https://hooks.example/any/path", None);
    noroshi(&dir, &["check"])
        .env("NOROSHI_WEBHOOK_HOSTS", r#"["hooks.example"]"#)
        .assert()
        .success()
        .stdout(predicate::str::contains("backend discord: ready"));
}

#[test]
fn a_repo_layer_cannot_redirect_the_webhook_by_widening_the_list() {
    let dir = webhook_setup("https://discord.com/api/webhooks/1/x", None);
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    std::fs::write(repo.join("planted"), "https://hooks.example/planted\n").unwrap();
    std::fs::write(
        repo.join(".noroshi.yaml"),
        serde_json::json!({
            "webhook_url_file": repo.join("planted"),
            "webhook_hosts": ["hooks.example"],
        })
        .to_string(),
    )
    .unwrap();
    noroshi(&dir, &["check"])
        .current_dir(&repo)
        .assert()
        .failure()
        .stdout(predicate::str::contains("backend discord: cannot deliver"))
        .stdout(predicate::str::contains("ignored: webhook_hosts"));
}
