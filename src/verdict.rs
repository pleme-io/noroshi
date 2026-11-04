use serde_json::Value;

use crate::alert::Alert;
use crate::config::{Config, Policy};
use crate::sink;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Signal(Alert),
    Silent(String),
}

#[must_use]
pub fn decide(payload: &Value, config: &Config) -> Verdict {
    let Some(alert) = Alert::from_payload(payload, &config.host) else {
        return Verdict::Silent("the payload has no hook_event_name".to_string());
    };
    if config.policy(&alert.event) == Policy::Silent {
        return Verdict::Silent(config.why_silent(&alert.event));
    }
    if sink::sinks(config).is_empty() {
        return Verdict::Silent("no backend is configured".to_string());
    }
    Verdict::Signal(alert.with_detail(payload, &config.render))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;

    fn config(events: &[(&str, &str)]) -> Config {
        Config {
            host: "ryn".into(),
            webhook_url_file: Some(PathBuf::from("/w")),
            events: events
                .iter()
                .map(|(e, p)| ((*e).to_string(), (*p).to_string()))
                .collect::<BTreeMap<_, _>>(),
            ..Config::default()
        }
    }

    fn silence(payload: &Value, config: &Config) -> String {
        match decide(payload, config) {
            Verdict::Silent(why) => why,
            Verdict::Signal(a) => panic!("expected silence, got a signal for {}", a.event),
        }
    }

    #[test]
    fn a_signalled_event_is_a_signal_carrying_the_alert() {
        let c = config(&[("Stop", "signal")]);
        let Verdict::Signal(a) = decide(&json!({"hook_event_name": "Stop"}), &c) else {
            panic!("expected a signal");
        };
        assert_eq!(a.event, "Stop");
        assert_eq!(a.host, "ryn");
    }

    #[test]
    fn the_signal_carries_context_only_when_the_render_asks() {
        let payload = json!({"hook_event_name": "Stop", "cwd": "/x/project"});
        let mut c = config(&[("Stop", "signal")]);
        let Verdict::Signal(a) = decide(&payload, &c) else {
            panic!("expected a signal");
        };
        assert_eq!(a.context, None);
        c.render.detail = "context".into();
        let Verdict::Signal(a) = decide(&payload, &c) else {
            panic!("expected a signal");
        };
        assert_eq!(a.context.unwrap().project.as_deref(), Some("project"));
    }

    #[test]
    fn each_silence_states_its_own_reason() {
        let c = config(&[("SessionEnd", "silent"), ("Notification", "loud")]);
        let event = |e: &str| json!({"hook_event_name": e});
        assert_eq!(
            silence(&event("SessionEnd"), &c),
            "event SessionEnd is set to silent"
        );
        assert_eq!(
            silence(&event("SubagentStop"), &c),
            "event SubagentStop is not listed in events; an unlisted event is silent"
        );
        assert_eq!(
            silence(&event("Notification"), &c),
            "events.Notification: `loud` is neither signal nor silent; treated as silent"
        );
        assert_eq!(
            silence(&json!({"message": "x"}), &c),
            "the payload has no hook_event_name"
        );
    }

    #[test]
    fn a_machine_with_no_backend_is_silent_and_says_so_after_the_policy() {
        let c = Config {
            webhook_url_file: None,
            ntfy: None,
            ..config(&[("Stop", "signal")])
        };
        assert_eq!(
            silence(&json!({"hook_event_name": "Stop"}), &c),
            "no backend is configured"
        );
        assert_eq!(
            silence(&json!({"hook_event_name": "SessionEnd"}), &c),
            "event SessionEnd is not listed in events; an unlisted event is silent"
        );
    }

    #[test]
    fn no_reason_quotes_the_payload_beyond_the_event_name() {
        let c = config(&[]);
        let why = silence(
            &json!({
                "hook_event_name": "Stop",
                "message": "zq7xv",
                "cwd": "/zq7xv",
                "session_id": "zq7xv",
            }),
            &c,
        );
        assert!(!why.contains("zq7xv"), "{why}");
    }
}
