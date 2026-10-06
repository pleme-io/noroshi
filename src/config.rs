use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const DEFAULT_NTFY_SERVER: &str = "https://ntfy.sh";

/// `~/.config/noroshi/noroshi.yaml`, rendered by blackmatter-claude.
///
/// Secrets are never values here, only paths read at run time. Every per-event
/// entry is kept raw and resolved on use, so one bad entry refuses that entry and
/// leaves the rest of the file working.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub host: String,
    pub mention: String,
    pub webhook_url_file: Option<PathBuf>,
    pub events: BTreeMap<String, String>,
    pub ntfy: Option<NtfyConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NtfyConfig {
    #[serde(default = "default_server")]
    pub server: String,
    pub topic_file: PathBuf,
    #[serde(default)]
    pub priorities: BTreeMap<String, i64>,
    #[serde(default)]
    pub tags: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub titles: BTreeMap<String, String>,
}

fn default_server() -> String {
    DEFAULT_NTFY_SERVER.to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    Signal,
    Silent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Priority(u8);

impl Priority {
    pub const DEFAULT: Self = Self(3);

    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<i64> for Priority {
    type Error = i64;

    fn try_from(v: i64) -> Result<Self, i64> {
        u8::try_from(v)
            .ok()
            .filter(|p| (1..=5).contains(p))
            .map(Self)
            .ok_or(v)
    }
}

impl Config {
    /// # Errors
    ///
    /// Returns an error when the file exists but is not valid YAML/JSON of this shape.
    pub fn load(path: &Path) -> anyhow::Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(path)?;
        Ok(Some(serde_yaml::from_str(&text)?))
    }

    /// An event with no entry, or an entry that is not `signal`, is silent.
    #[must_use]
    pub fn policy(&self, event: &str) -> Policy {
        match self.events.get(event).map(String::as_str) {
            Some("signal") => Policy::Signal,
            _ => Policy::Silent,
        }
    }

    /// Every entry that will be ignored, one line each.
    #[must_use]
    pub fn problems(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .events
            .iter()
            .filter(|(_, p)| !matches!(p.as_str(), "signal" | "silent"))
            .map(|(e, p)| {
                format!("events.{e}: `{p}` is neither signal nor silent; treated as silent")
            })
            .collect();
        if let Some(n) = &self.ntfy {
            out.extend(
                n.priorities
                    .iter()
                    .filter(|(_, p)| Priority::try_from(**p).is_err())
                    .map(|(e, p)| {
                        format!("ntfy.priorities.{e}: {p} is outside 1..=5; the default 3 is used")
                    }),
            );
        }
        out
    }
}

impl NtfyConfig {
    #[must_use]
    pub fn priority(&self, event: &str) -> Priority {
        self.priorities
            .get(event)
            .and_then(|p| Priority::try_from(*p).ok())
            .unwrap_or(Priority::DEFAULT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priorities_outside_one_to_five_are_refused_per_entry() {
        let cfg: Config = serde_yaml::from_str(
            "events: {Stop: signal, Notification: loud}\nntfy:\n  topic_file: /t\n  priorities: {Stop: 5, Notification: 9}\n",
        )
        .unwrap();
        let n = cfg.ntfy.as_ref().unwrap();
        assert_eq!(n.priority("Stop").get(), 5);
        assert_eq!(n.priority("Notification"), Priority::DEFAULT);
        assert_eq!(n.server, DEFAULT_NTFY_SERVER);
        assert_eq!(cfg.policy("Stop"), Policy::Signal);
        assert_eq!(cfg.policy("Notification"), Policy::Silent);
        assert_eq!(cfg.policy("SessionEnd"), Policy::Silent);
        assert_eq!(cfg.problems().len(), 2);
    }

    #[test]
    fn priority_bounds() {
        assert!(Priority::try_from(0).is_err());
        assert!(Priority::try_from(6).is_err());
        assert!(Priority::try_from(-1).is_err());
        assert_eq!(Priority::try_from(1).unwrap().get(), 1);
        assert_eq!(Priority::try_from(5).unwrap().get(), 5);
    }

    #[test]
    fn the_blackmatter_rendering_parses() {
        let cfg: Config = serde_json::from_str(
            r#"{"mention":"","host":"ryn","events":{"Stop":"signal","SessionEnd":"silent"},"webhook_url_file":"/w","ntfy":{"server":"https://ntfy.sh","topic_file":"/t","priorities":{"Stop":5},"tags":{},"titles":{}}}"#,
        )
        .unwrap();
        assert_eq!(cfg.host, "ryn");
        assert!(cfg.problems().is_empty());
    }
}
