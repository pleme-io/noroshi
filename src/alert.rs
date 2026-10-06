use serde_json::Value;

/// What an alert may carry: the machine, the event and a short session label.
/// Nothing from the payload beyond those reaches a sink — no prompt, no message
/// text, no path, no code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub event: String,
    pub host: String,
    pub session: String,
}

const SESSION_LABEL_LEN: usize = 8;

impl Alert {
    #[must_use]
    pub fn from_payload(payload: &Value, host: &str) -> Option<Self> {
        let event = payload.get("hook_event_name")?.as_str()?.to_string();
        let session = payload
            .get("session_id")
            .and_then(Value::as_str)
            .map_or_else(|| "-".to_string(), session_label);
        Some(Self {
            event,
            host: host.to_string(),
            session,
        })
    }

    #[must_use]
    pub fn headline(&self) -> String {
        match self.event.as_str() {
            "Stop" => "Claude finished".to_string(),
            "Notification" => "Claude is waiting on you".to_string(),
            "SubagentStop" => "A subagent finished".to_string(),
            "SessionEnd" => "Claude session ended".to_string(),
            other => format!("Claude {other}"),
        }
    }

    #[must_use]
    pub fn line(&self) -> String {
        let mut parts = Vec::new();
        if !self.host.is_empty() {
            parts.push(self.host.clone());
        }
        parts.push(self.event.clone());
        parts.push(format!("session {}", self.session));
        parts.join(" · ")
    }
}

#[must_use]
pub fn session_label(id: &str) -> String {
    let label: String = id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(SESSION_LABEL_LEN)
        .collect();
    if label.is_empty() {
        "-".to_string()
    } else {
        label
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_host_event_and_a_short_label_survive() {
        let payload = serde_json::json!({
            "hook_event_name": "Notification",
            "session_id": "3f1c9a2e-77aa-4b1e-9f00-0123456789ab",
            "message": "Claude needs your permission to use Bash: rm -rf secret",
            "transcript_path": "/Users/x/.claude/projects/p/s.jsonl",
            "cwd": "/Users/x/code/private-thing",
        });
        let a = Alert::from_payload(&payload, "ryn").unwrap();
        assert_eq!(a.session, "3f1c9a2e");
        let rendered = format!("{} {}", a.headline(), a.line());
        for leaked in [
            "rm -rf",
            "secret",
            "transcript",
            "private-thing",
            "permission",
        ] {
            assert!(
                !rendered.contains(leaked),
                "{leaked} leaked into {rendered}"
            );
        }
        assert_eq!(a.line(), "ryn · Notification · session 3f1c9a2e");
    }

    #[test]
    fn a_payload_without_an_event_is_not_an_alert() {
        assert!(Alert::from_payload(&serde_json::json!({}), "h").is_none());
        let a = Alert::from_payload(&serde_json::json!({"hook_event_name": "Stop"}), "").unwrap();
        assert_eq!(a.line(), "Stop · session -");
    }
}
