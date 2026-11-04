use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::alert::Detail;

pub const APP: &str = "noroshi";
pub const ENV_PREFIX: &str = "NOROSHI_";
pub const DEFAULT_NTFY_SERVER: &str = "https://ntfy.sh";
pub const DISCORD_HOSTS: [&str; 2] = ["discord.com", "discordapp.com"];

/// Merged from, lowest to highest: built-in defaults, `/etc/noroshi/noroshi.yaml`,
/// `~/.config/noroshi/noroshi.yaml` (rendered by blackmatter-claude), every
/// `.noroshi.yaml` from `/` down to the session's working directory, then
/// `NOROSHI_*` environment variables (`__` nests: `NOROSHI_RENDER__DETAIL=context`).
///
/// Secrets are never values here, only paths read at run time. Every per-event
/// entry is kept raw and resolved on use, so one bad entry refuses that entry and
/// leaves the rest of the file working.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub host: String,
    pub mention: String,
    pub webhook_url_file: Option<PathBuf>,
    pub webhook_hosts: Vec<String>,
    pub events: BTreeMap<String, String>,
    pub ntfy: Option<NtfyConfig>,
    pub render: Render,
    #[serde(skip)]
    pub ignored: Vec<String>,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Render {
    pub detail: String,
    pub markdown: bool,
    pub said_max: usize,
    pub project: bool,
    pub branch: bool,
    pub session: bool,
    pub quote_claude: bool,
    pub headlines: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
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
    #[serde(default)]
    pub click: Option<String>,
}

fn default_server() -> String {
    DEFAULT_NTFY_SERVER.to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: String::new(),
            mention: String::new(),
            webhook_url_file: None,
            webhook_hosts: DISCORD_HOSTS.map(String::from).to_vec(),
            events: BTreeMap::from([
                ("Stop".into(), "signal".into()),
                ("Notification".into(), "signal".into()),
            ]),
            ntfy: None,
            render: Render::default(),
            ignored: Vec::new(),
        }
    }
}

impl Default for Render {
    fn default() -> Self {
        Self {
            detail: "minimal".into(),
            markdown: true,
            said_max: 280,
            project: true,
            branch: true,
            session: true,
            quote_claude: true,
            headlines: BTreeMap::from([
                ("Stop".into(), "Claude finished".into()),
                ("Notification".into(), "Claude is waiting on you".into()),
                (
                    "Notification:permission_prompt".into(),
                    "Claude needs your approval".into(),
                ),
                (
                    "Notification:idle_prompt".into(),
                    "Claude is idle, waiting on you".into(),
                ),
                ("SubagentStop".into(), "A subagent finished".into()),
                ("SessionEnd".into(), "Claude session ended".into()),
            ]),
        }
    }
}

impl Render {
    #[must_use]
    pub fn detail(&self) -> Detail {
        match self.detail.as_str() {
            "context" => Detail::Context,
            _ => Detail::Minimal,
        }
    }

    #[must_use]
    pub fn headline(&self, event: &str, kind: Option<&str>) -> String {
        kind.and_then(|k| self.headlines.get(&format!("{event}:{k}")))
            .or_else(|| self.headlines.get(event))
            .cloned()
            .unwrap_or_else(|| format!("Claude {event}"))
    }
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
    #[must_use]
    pub fn layers(start_dir: Option<&Path>) -> Vec<PathBuf> {
        let mut discovery = shikumi::ConfigDiscovery::new(APP)
            .formats(&[shikumi::Format::Yaml])
            .hierarchical();
        if let Some(dir) = start_dir {
            discovery = discovery.start_dir(dir);
        }
        discovery.discover_all().unwrap_or_default()
    }

    /// # Errors
    ///
    /// Returns an error when a layer exists but is not valid YAML of this shape.
    pub fn load_layers(layers: &[PathBuf]) -> anyhow::Result<Self> {
        let mut config = Self::merged(layers)?;
        let operator: Vec<PathBuf> = layers
            .iter()
            .filter(|l| !is_repo_layer(l))
            .cloned()
            .collect();
        let hosts = Self::merged(&operator)?.webhook_hosts;
        if config.webhook_hosts != hosts {
            config.ignored.push(
                "webhook_hosts: a .noroshi.yaml in the working tree cannot set this; \
                 use /etc, the user config, --config or the environment"
                    .to_string(),
            );
            config.webhook_hosts = hosts;
        }
        Ok(config)
    }

    fn merged(layers: &[PathBuf]) -> anyhow::Result<Self> {
        Ok(layers
            .iter()
            .fold(
                shikumi::ProviderChain::new().with_defaults(&Self::default()),
                |chain, path| chain.with_file(path),
            )
            .with_env(ENV_PREFIX)
            .extract()?)
    }

    /// An explicit `--config` file replaces the user layer; system and repo
    /// layers still merge around it.
    ///
    /// # Errors
    ///
    /// Returns an error when a layer exists but is not valid YAML of this shape.
    pub fn load(explicit: Option<&Path>, start_dir: Option<&Path>) -> anyhow::Result<Self> {
        let mut layers = Self::layers(start_dir);
        if let Some(path) = explicit {
            let user = Self::user_layer();
            layers.retain(|l| *l != user);
            let at = layers
                .iter()
                .position(|l| !l.starts_with("/etc"))
                .unwrap_or(layers.len());
            layers.insert(at, path.to_path_buf());
        }
        Self::load_layers(&layers)
    }

    fn user_layer() -> PathBuf {
        okiba::Okiba::for_app(APP)
            .path(okiba::Tier::Config, "noroshi.yaml")
            .into_path_buf()
    }

    /// An event with no entry, or an entry that is not `signal`, is silent.
    #[must_use]
    pub fn policy(&self, event: &str) -> Policy {
        match self.events.get(event).map(String::as_str) {
            Some("signal") => Policy::Signal,
            _ => Policy::Silent,
        }
    }

    #[must_use]
    pub fn why_silent(&self, event: &str) -> String {
        match self.events.get(event).map(String::as_str) {
            Some("silent") => format!("event {event} is set to silent"),
            Some(other) => {
                format!("events.{event}: `{other}` is neither signal nor silent; treated as silent")
            }
            None => format!("event {event} is not listed in events; an unlisted event is silent"),
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
        out.extend(self.ignored.iter().cloned());
        out.extend(
            self.webhook_hosts
                .iter()
                .filter(|h| !is_authority(h))
                .map(|h| format!("webhook_hosts: `{h}` is not a host[:port]; it matches nothing")),
        );
        if !matches!(self.render.detail.as_str(), "minimal" | "context") {
            out.push(format!(
                "render.detail: `{}` is neither minimal nor context; treated as minimal",
                self.render.detail
            ));
        }
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

fn is_repo_layer(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with('.'))
}

fn is_authority(host: &str) -> bool {
    format!("https://{host}/")
        .parse::<ureq::http::Uri>()
        .ok()
        .and_then(|u| u.authority().map(|a| a.as_str() == host))
        .unwrap_or(false)
        && !host.contains('@')
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

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn priorities_outside_one_to_five_are_refused_per_entry() {
        let dir = tempfile::tempdir().unwrap();
        let f = write(
            dir.path(),
            "n.yaml",
            "events: {Stop: signal, Notification: loud}\nntfy:\n  topic_file: /t\n  priorities: {Stop: 5, Notification: 9}\n",
        );
        let cfg = Config::load_layers(&[f]).unwrap();
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
        let dir = tempfile::tempdir().unwrap();
        let f = write(
            dir.path(),
            "n.yaml",
            r#"{"mention":"","host":"ryn","events":{"Stop":"signal","SessionEnd":"silent"},"webhook_url_file":"/w","render":{"detail":"context"},"ntfy":{"server":"https://ntfy.sh","topic_file":"/t","priorities":{"Stop":5},"tags":{},"titles":{}}}"#,
        );
        let cfg = Config::load_layers(&[f]).unwrap();
        assert_eq!(cfg.host, "ryn");
        assert_eq!(cfg.render.detail(), Detail::Context);
        assert_eq!(cfg.render.said_max, 280);
        assert!(cfg.problems().is_empty());
    }

    #[test]
    fn a_repo_layer_merges_over_the_user_layer_key_by_key() {
        let dir = tempfile::tempdir().unwrap();
        let user = write(
            dir.path(),
            "user.yaml",
            "host: ryn\nrender:\n  detail: context\n  headlines: {Stop: Done}\n",
        );
        let repo = write(
            dir.path(),
            ".noroshi.yaml",
            "render:\n  branch: false\n  headlines: {Stop: Shipped}\n",
        );
        let cfg = Config::load_layers(&[user, repo]).unwrap();
        assert_eq!(cfg.host, "ryn");
        assert_eq!(cfg.render.detail(), Detail::Context);
        assert!(!cfg.render.branch);
        assert_eq!(cfg.render.headline("Stop", None), "Shipped");
        assert_eq!(
            cfg.render
                .headline("Notification", Some("permission_prompt")),
            "Claude needs your approval"
        );
    }

    #[test]
    fn webhook_hosts_default_to_discord_and_an_operator_layer_replaces_them() {
        assert_eq!(Config::default().webhook_hosts, DISCORD_HOSTS);
        let dir = tempfile::tempdir().unwrap();
        let user = write(
            dir.path(),
            "user.yaml",
            "webhook_hosts: [hooks.example, \"hooks.example:8443\"]\n",
        );
        let cfg = Config::load_layers(&[user]).unwrap();
        assert_eq!(cfg.webhook_hosts, ["hooks.example", "hooks.example:8443"]);
        assert!(cfg.problems().is_empty());
    }

    #[test]
    fn a_dotfile_layer_cannot_change_webhook_hosts_and_the_refusal_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let user = write(dir.path(), "user.yaml", "webhook_hosts: [hooks.example]\n");
        for name in [".noroshi.yaml", ".noroshi-01-extra.yaml"] {
            let repo = write(
                dir.path(),
                name,
                "host: from-repo\nwebhook_hosts: [planted.example]\n",
            );
            let cfg = Config::load_layers(&[user.clone(), repo]).unwrap();
            assert_eq!(cfg.host, "from-repo");
            assert_eq!(cfg.webhook_hosts, ["hooks.example"]);
            assert_eq!(cfg.problems().len(), 1);
            assert!(cfg.problems()[0].starts_with("webhook_hosts:"));
        }
        let repo = write(dir.path(), ".noroshi.yaml", "host: from-repo\n");
        assert!(
            Config::load_layers(&[user, repo])
                .unwrap()
                .problems()
                .is_empty()
        );
    }

    #[test]
    fn a_webhook_hosts_entry_that_cannot_match_is_reported_on_its_own() {
        let cfg = Config {
            webhook_hosts: vec![
                "hooks.example".into(),
                "https://hooks.example".into(),
                "a@hooks.example".into(),
                "hooks.example/path".into(),
                String::new(),
            ],
            ..Config::default()
        };
        assert_eq!(cfg.problems().len(), 4);
    }

    #[test]
    fn hierarchical_discovery_finds_every_repo_layer_down_to_the_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let sub = repo.join("crate");
        std::fs::create_dir_all(&sub).unwrap();
        write(&repo, ".noroshi.yaml", "host: a\n");
        write(&sub, ".noroshi.yaml", "host: b\n");
        let layers = Config::layers(Some(&sub));
        let repo_layers: Vec<_> = layers
            .iter()
            .filter(|l| l.starts_with(dir.path()))
            .collect();
        assert_eq!(repo_layers.len(), 2);
        assert_eq!(Config::load_layers(&layers).unwrap().host, "b");
    }
}
