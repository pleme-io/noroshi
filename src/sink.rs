use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;

use crate::alert::Alert;
use crate::config::{Config, NtfyConfig, Render};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub url: String,
    pub content_type: &'static str,
    pub body: String,
}

/// Why a sink could not deliver. Never carries a secret's contents.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SinkError {
    #[error("secret file {0} is missing")]
    Missing(PathBuf),
    #[error("secret file {0} is unreadable: {1}")]
    Unreadable(PathBuf, String),
    #[error("secret file {0} is empty")]
    Empty(PathBuf),
    #[error("secret file {0} does not hold {1}")]
    Malformed(PathBuf, &'static str),
    #[error("{0} is not an http(s) URL")]
    BadServer(String),
    #[error("delivery failed: {0}")]
    Transport(String),
    #[error("server answered HTTP {0}")]
    Status(u16),
}

pub trait Transport {
    /// # Errors
    ///
    /// Returns the transport failure; an HTTP status is a success at this layer.
    fn send(&self, request: &HttpRequest) -> Result<u16, String>;
}

pub struct Ureq {
    agent: ureq::Agent,
}

impl Ureq {
    #[must_use]
    pub fn new(timeout: Duration) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .build();
        Self {
            agent: config.into(),
        }
    }
}

impl Transport for Ureq {
    fn send(&self, request: &HttpRequest) -> Result<u16, String> {
        self.agent
            .post(&request.url)
            .header("Content-Type", request.content_type)
            .send(request.body.as_str())
            .map(|r| r.status().as_u16())
            .map_err(|e| e.to_string())
    }
}

pub trait Sink {
    fn name(&self) -> &'static str;

    /// # Errors
    ///
    /// Returns why this sink cannot deliver right now.
    fn request(&self, alert: &Alert) -> Result<HttpRequest, SinkError>;
}

fn read_secret(path: &Path) -> Result<String, SinkError> {
    let text = std::fs::read_to_string(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => SinkError::Missing(path.to_path_buf()),
        _ => SinkError::Unreadable(path.to_path_buf(), e.kind().to_string()),
    })?;
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err(SinkError::Empty(path.to_path_buf()));
    }
    Ok(text)
}

pub struct Discord {
    pub webhook_url_file: PathBuf,
    pub mention: String,
    pub render: Render,
}

impl Sink for Discord {
    fn name(&self) -> &'static str {
        "discord"
    }

    fn request(&self, alert: &Alert) -> Result<HttpRequest, SinkError> {
        let url = read_secret(&self.webhook_url_file)?;
        if !url.starts_with("https://") {
            return Err(SinkError::Malformed(
                self.webhook_url_file.clone(),
                "an https webhook URL",
            ));
        }
        let r = &self.render;
        let mut content = if alert.context.is_some() {
            format!("**{}**\n{}", alert.title(r), alert.body(r))
        } else {
            format!("**{}** — {}", alert.headline(r), alert.line(r))
        };
        if !self.mention.is_empty() {
            content = format!("{} {content}", self.mention);
        }
        Ok(HttpRequest {
            url,
            content_type: "application/json",
            body: json!({ "content": content, "allowed_mentions": { "parse": ["users"] } })
                .to_string(),
        })
    }
}

pub struct Ntfy {
    pub config: NtfyConfig,
    pub render: Render,
}

const TOPIC_MAX: usize = 64;

fn valid_topic(topic: &str) -> bool {
    !topic.is_empty()
        && topic.len() <= TOPIC_MAX
        && topic
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl Sink for Ntfy {
    fn name(&self) -> &'static str {
        "ntfy"
    }

    fn request(&self, alert: &Alert) -> Result<HttpRequest, SinkError> {
        let server = self.config.server.trim_end_matches('/');
        if !(server.starts_with("https://") || server.starts_with("http://")) {
            return Err(SinkError::BadServer(server.to_string()));
        }
        let topic = read_secret(&self.config.topic_file)?;
        if !valid_topic(&topic) {
            return Err(SinkError::Malformed(
                self.config.topic_file.clone(),
                "an ntfy topic (1-64 of A-Z a-z 0-9 - _)",
            ));
        }
        let title = self
            .config
            .titles
            .get(&alert.event)
            .cloned()
            .unwrap_or_else(|| alert.title(&self.render));
        let tags = self
            .config
            .tags
            .get(&alert.event)
            .cloned()
            .unwrap_or_default();
        let mut body = json!({
            "topic": topic,
            "title": title,
            "message": alert.body(&self.render),
            "markdown": self.render.markdown,
            "priority": self.config.priority(&alert.event).get(),
            "tags": tags,
        });
        if let Some(click) = &self.config.click {
            body["click"] = json!(click);
        }
        Ok(HttpRequest {
            url: format!("{server}/"),
            content_type: "application/json",
            body: body.to_string(),
        })
    }
}

/// Every backend the config names. More than one may be configured; each
/// delivers on its own.
#[must_use]
pub fn sinks(config: &Config) -> Vec<Box<dyn Sink>> {
    let mut out: Vec<Box<dyn Sink>> = Vec::new();
    if let Some(path) = &config.webhook_url_file {
        out.push(Box::new(Discord {
            webhook_url_file: path.clone(),
            mention: config.mention.clone(),
            render: config.render.clone(),
        }));
    }
    if let Some(ntfy) = &config.ntfy {
        out.push(Box::new(Ntfy {
            config: ntfy.clone(),
            render: config.render.clone(),
        }));
    }
    out
}

/// # Errors
///
/// Returns why the sink did not deliver: unconfigured secret, transport, or non-2xx.
pub fn deliver(
    sink: &dyn Sink,
    alert: &Alert,
    transport: &dyn Transport,
) -> Result<u16, SinkError> {
    let request = sink.request(alert)?;
    let status = transport.send(&request).map_err(SinkError::Transport)?;
    if (200..300).contains(&status) {
        Ok(status)
    } else {
        Err(SinkError::Status(status))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    struct Recording {
        sent: RefCell<Vec<HttpRequest>>,
        status: u16,
    }

    impl Transport for Recording {
        fn send(&self, request: &HttpRequest) -> Result<u16, String> {
            self.sent.borrow_mut().push(request.clone());
            Ok(self.status)
        }
    }

    fn recording(status: u16) -> Recording {
        Recording {
            sent: RefCell::new(Vec::new()),
            status,
        }
    }

    fn alert(event: &str) -> Alert {
        Alert {
            event: event.into(),
            host: "ryn".into(),
            session: "abcd1234".into(),
            context: None,
        }
    }

    fn secret(dir: &tempfile::TempDir, name: &str, value: &str) -> PathBuf {
        let p = dir.path().join(name);
        std::fs::write(&p, value).unwrap();
        p
    }

    fn ntfy(topic_file: PathBuf) -> Ntfy {
        Ntfy {
            config: NtfyConfig {
                server: "https://ntfy.example/".into(),
                topic_file,
                priorities: BTreeMap::from([("Stop".into(), 5), ("Notification".into(), 4)]),
                tags: BTreeMap::from([("Stop".into(), vec!["white_check_mark".into()])]),
                titles: BTreeMap::new(),
                click: None,
            },
            render: Render::default(),
        }
    }

    #[test]
    fn ntfy_request_carries_topic_in_the_body_and_the_event_priority() {
        let dir = tempfile::tempdir().unwrap();
        let sink = ntfy(secret(&dir, "topic", "Abc_def-123\n"));
        let req = sink.request(&alert("Stop")).unwrap();
        assert_eq!(req.url, "https://ntfy.example/");
        assert!(
            !req.url.contains("Abc_def"),
            "the topic stays out of the URL"
        );
        let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
        assert_eq!(body["topic"], "Abc_def-123");
        assert_eq!(body["priority"], 5);
        assert_eq!(body["title"], "Claude finished");
        assert_eq!(body["message"], "ryn · Stop · session abcd1234");
        assert_eq!(body["tags"], serde_json::json!(["white_check_mark"]));

        let body: serde_json::Value =
            serde_json::from_str(&sink.request(&alert("Notification")).unwrap().body).unwrap();
        assert_eq!(body["priority"], 4);
        let body: serde_json::Value =
            serde_json::from_str(&sink.request(&alert("SessionEnd")).unwrap().body).unwrap();
        assert_eq!(body["priority"], 3);
    }

    #[test]
    fn ntfy_refuses_a_missing_or_malformed_topic_without_echoing_it() {
        let dir = tempfile::tempdir().unwrap();
        let missing = ntfy(dir.path().join("absent"));
        assert!(matches!(
            missing.request(&alert("Stop")),
            Err(SinkError::Missing(_))
        ));
        let bad = ntfy(secret(&dir, "topic", "has spaces zq7xv"));
        let err = bad.request(&alert("Stop")).unwrap_err();
        assert!(matches!(err, SinkError::Malformed(..)));
        assert!(!err.to_string().contains("zq7xv"));
        let empty = ntfy(secret(&dir, "empty", "  \n"));
        assert!(matches!(
            empty.request(&alert("Stop")),
            Err(SinkError::Empty(_))
        ));
    }

    #[test]
    fn discord_request_keeps_working() {
        let dir = tempfile::tempdir().unwrap();
        let sink = Discord {
            webhook_url_file: secret(&dir, "hook", "https://discord.com/api/webhooks/1/x\n"),
            mention: "<@42>".into(),
            render: Render::default(),
        };
        let req = sink.request(&alert("Notification")).unwrap();
        assert_eq!(req.url, "https://discord.com/api/webhooks/1/x");
        let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
        assert_eq!(
            body["content"],
            "<@42> **Claude is waiting on you** — ryn · Notification · session abcd1234"
        );
    }

    #[test]
    fn both_backends_deliver_independently() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            webhook_url_file: Some(dir.path().join("no-webhook")),
            ntfy: Some(ntfy(secret(&dir, "topic", "t0p1c")).config),
            ..Config::default()
        };
        let all = sinks(&config);
        assert_eq!(
            all.iter().map(|s| s.name()).collect::<Vec<_>>(),
            ["discord", "ntfy"]
        );
        let t = recording(200);
        let results: Vec<_> = all
            .iter()
            .map(|s| deliver(s.as_ref(), &alert("Stop"), &t))
            .collect();
        assert!(matches!(results[0], Err(SinkError::Missing(_))));
        assert_eq!(results[1], Ok(200));
        assert_eq!(t.sent.borrow().len(), 1);
    }

    #[test]
    fn a_non_2xx_is_a_failure() {
        let dir = tempfile::tempdir().unwrap();
        let sink = ntfy(secret(&dir, "topic", "t"));
        assert_eq!(
            deliver(&sink, &alert("Stop"), &recording(429)),
            Err(SinkError::Status(429))
        );
    }
}
