use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::config::Render;

/// What an alert may carry. At `Detail::Minimal` that is the machine, the event
/// and a short session label — no prompt, no message text, no path, no code.
/// `Detail::Context` adds what a human needs to act on it: the project, the
/// branch, what Claude asked for, and the start of Claude's last reply.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Alert {
    pub event: String,
    pub host: String,
    pub session: String,
    pub context: Option<Context>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Context {
    pub project: Option<String>,
    pub branch: Option<String>,
    pub kind: Option<String>,
    pub said: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Detail {
    #[default]
    Minimal,
    Context,
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
            context: None,
        })
    }

    #[must_use]
    pub fn with_detail(mut self, payload: &Value, render: &Render) -> Self {
        if render.detail() == Detail::Context {
            self.context = Some(Context::from_payload(payload, &self.event, render));
        }
        self
    }

    #[must_use]
    pub fn headline(&self, render: &Render) -> String {
        let kind = self.context.as_ref().and_then(|c| c.kind.as_deref());
        render.headline(&self.event, kind)
    }

    #[must_use]
    pub fn title(&self, render: &Render) -> String {
        match self.context.as_ref().and_then(|c| c.project.as_deref()) {
            Some(project) => format!("{project} — {}", self.headline(render)),
            None => self.headline(render),
        }
    }

    #[must_use]
    pub fn line(&self, render: &Render) -> String {
        let mut parts = Vec::new();
        if !self.host.is_empty() {
            parts.push(self.host.clone());
        }
        if let Some(c) = &self.context {
            match (&c.project, &c.branch) {
                (Some(p), Some(b)) => parts.push(format!("{p} @ {b}")),
                (Some(p), None) => parts.push(p.clone()),
                (None, Some(b)) => parts.push(format!("@ {b}")),
                (None, None) => {}
            }
        }
        parts.push(self.event.clone());
        if render.session {
            parts.push(format!("session {}", self.session));
        }
        parts.join(" · ")
    }

    #[must_use]
    pub fn body(&self, render: &Render) -> String {
        let said = self.context.as_ref().and_then(|c| c.said.as_deref());
        let line = self.line(render);
        match said {
            Some(s) if render.markdown => format!("{}\n\n`{line}`", quote(s)),
            Some(s) => format!("{s}\n— {line}"),
            None => line,
        }
    }
}

fn quote(text: &str) -> String {
    text.lines()
        .map(|l| format!("> {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

impl Context {
    #[must_use]
    pub fn from_payload(payload: &Value, event: &str, render: &Render) -> Self {
        let cwd = payload.get("cwd").and_then(Value::as_str).map(Path::new);
        let root = cwd.and_then(repo_root);
        let project = render
            .project
            .then(|| root.as_deref().or(cwd).and_then(Path::file_name))
            .flatten()
            .map(|n| n.to_string_lossy().into_owned());
        let branch = render
            .branch
            .then(|| root.as_deref().and_then(git_branch))
            .flatten();
        let kind = payload
            .get("notification_type")
            .and_then(Value::as_str)
            .map(str::to_string);
        let said = match event {
            "Notification" => payload
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string),
            _ => payload
                .get("last_assistant_message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    payload
                        .get("transcript_path")
                        .and_then(Value::as_str)
                        .and_then(|p| last_assistant_text(Path::new(p)))
                }),
        }
        .filter(|_| render.quote_claude)
        .map(|s| clip(&s, render.said_max))
        .filter(|s| !s.is_empty());
        Self {
            project,
            branch,
            kind,
            said,
        }
    }
}

fn repo_root(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .find(|d| d.join(".git").exists())
        .map(Path::to_path_buf)
}

fn git_branch(root: &Path) -> Option<String> {
    let git = root.join(".git");
    let head = if git.is_file() {
        let pointer = std::fs::read_to_string(&git).ok()?;
        let dir = pointer.trim().strip_prefix("gitdir: ")?.to_string();
        std::fs::read_to_string(Path::new(&dir).join("HEAD")).ok()?
    } else {
        std::fs::read_to_string(git.join("HEAD")).ok()?
    };
    head.trim()
        .strip_prefix("ref: refs/heads/")
        .map(str::to_string)
}

#[must_use]
pub fn last_assistant_text(transcript: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(transcript).ok()?;
    raw.lines().rev().find_map(|line| {
        let v: Value = serde_json::from_str(line).ok()?;
        if v.get("type")?.as_str()? != "assistant" {
            return None;
        }
        let text: Vec<&str> = v
            .get("message")?
            .get("content")?
            .as_array()?
            .iter()
            .filter(|c| c.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|c| c.get("text").and_then(Value::as_str))
            .collect();
        let joined = text.join("\n");
        (!joined.trim().is_empty()).then_some(joined)
    })
}

#[must_use]
pub fn clip(text: &str, max: usize) -> String {
    let flat: String = text
        .lines()
        .map(|l| {
            l.trim()
                .trim_start_matches(['#', '>', '|', '-', '*'])
                .trim()
        })
        .filter(|l| !l.is_empty() && !l.starts_with("```"))
        .collect::<Vec<_>>()
        .join(" ");
    let flat = flat.replace("**", "").replace('`', "");
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    format!("{cut}…")
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
        let r = Render::default();
        let a = Alert::from_payload(&payload, "ryn")
            .unwrap()
            .with_detail(&payload, &r);
        assert_eq!(a.session, "3f1c9a2e");
        let rendered = format!("{} {} {}", a.title(&r), a.line(&r), a.body(&r));
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
        assert_eq!(a.line(&r), "ryn · Notification · session 3f1c9a2e");
    }

    fn context() -> Render {
        Render {
            detail: "context".into(),
            ..Render::default()
        }
    }

    #[test]
    fn a_payload_without_an_event_is_not_an_alert() {
        assert!(Alert::from_payload(&serde_json::json!({}), "h").is_none());
        let a = Alert::from_payload(&serde_json::json!({"hook_event_name": "Stop"}), "").unwrap();
        assert_eq!(a.line(&Render::default()), "Stop · session -");
    }

    #[test]
    fn context_names_the_project_branch_and_what_claude_asked() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("nix");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let payload = serde_json::json!({
            "hook_event_name": "Notification",
            "session_id": "3f1c9a2e-77aa",
            "notification_type": "permission_prompt",
            "message": "Claude needs your permission to use Bash",
            "cwd": repo.join("docs").to_string_lossy(),
        });
        std::fs::create_dir_all(repo.join("docs")).unwrap();
        let r = context();
        let a = Alert::from_payload(&payload, "ryn")
            .unwrap()
            .with_detail(&payload, &r);
        assert_eq!(a.title(&r), "nix — Claude needs your approval");
        assert_eq!(
            a.line(&r),
            "ryn · nix @ main · Notification · session 3f1c9a2e"
        );
        assert_eq!(
            a.body(&r),
            "> Claude needs your permission to use Bash\n\n`ryn · nix @ main · Notification · session 3f1c9a2e`"
        );

        let quiet = Render {
            branch: false,
            session: false,
            quote_claude: false,
            markdown: false,
            ..context()
        };
        let a = Alert::from_payload(&payload, "ryn")
            .unwrap()
            .with_detail(&payload, &quiet);
        assert_eq!(a.body(&quiet), "ryn · nix · Notification");
    }

    #[test]
    fn stop_reads_the_last_assistant_text_from_the_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("s.jsonl");
        std::fs::write(
            &t,
            [
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"old"}]}}"#,
                &serde_json::json!({"type":"assistant","message":{"content":[{"type":"text","text":"## Done\n\n**Shipped** the `fix`."}]}}).to_string(),
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#,
                r#"{"type":"user","message":{"content":"thanks"}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        let payload = serde_json::json!({
            "hook_event_name": "Stop",
            "transcript_path": t.to_string_lossy(),
            "cwd": dir.path().to_string_lossy(),
        });
        let c = Context::from_payload(&payload, "Stop", &context());
        assert_eq!(c.said.as_deref(), Some("Done Shipped the fix."));
    }

    #[test]
    fn clip_cuts_at_a_word() {
        assert_eq!(clip("alpha beta gamma", 12), "alpha beta…");
        assert_eq!(clip("short", 12), "short");
    }
}
