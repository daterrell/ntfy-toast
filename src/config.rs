use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::Path;

pub const SAMPLE: &str = r#"# ntfy-toast configuration
#
# Changes are picked up automatically while ntfy-toast is running.
#
# Server used by subscriptions that don't set their own `server`.
default_server = "https://ntfy.sh"

# Name shown as the notification source in Windows (Settings > System > Notifications).
app_display_name = "ntfy"

# Clicking a notification without its own click URL opens the topic in the browser.
click_opens_topic = true

# On startup, fetch messages published while the daemon wasn't running
# (resumes from the last message seen; the server keeps ~12h of cache by default).
catch_up_on_start = false

[[subscription]]
topic = "change-me-to-your-topic"

# Self-update. ntfy-toast checks for a new release at startup and whenever the release
# topic below announces one, then shows a notification with an Install button.
[update]
# "notify" (ask first), "auto" (install right away), or "off".
mode = "notify"
# Where releases are published: a Forgejo/Gitea server, or https://github.com.
server = "https://forgejo.terrell.ltd"
repo = "dave/ntfy-toast"
# Forgejo access token with read access to the repo (needed if the server requires
# sign-in). Without one, the public GitHub releases below are used instead.
# token = "..."
github_repo = "daterrell/ntfy-toast"
# ntfy topic that CI posts to when a release is published, for an instant check.
# Set to "" to rely on the startup check only.
topic = "https://ntfy.terrell.ltd/ntfy-toast-releases"

# Self-hosted server with access control, several topics, and a priority filter.
# A topic can be just its id, or `{ id = "...", name = "..." }` to show a friendlier
# name on notifications (handy for random-looking topic ids):
# [[subscription]]
# server = "https://ntfy.example.com"
# topics = [
#   "alerts",
#   { id = "Xk29fQpL0aZr", name = "Backups" },
# ]
# token = "tk_xxxxxxxxxxxxxxxxxxxxxxxxxxxxx"   # or: username = "..." + password = "..."
# min_priority = 3                             # 1=min 2=low 3=default 4=high 5=urgent
# name = "Home server"                         # shown as the attribution line
"#;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_server")]
    pub default_server: String,
    #[serde(default = "default_app_id")]
    pub app_id: String,
    #[serde(default = "default_display_name")]
    pub app_display_name: String,
    #[serde(default = "yes")]
    pub click_opens_topic: bool,
    #[serde(default)]
    pub catch_up_on_start: bool,
    #[serde(default, rename = "subscription")]
    pub subscriptions: Vec<Subscription>,
    #[serde(default)]
    pub update: UpdateConfig,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct UpdateConfig {
    pub mode: UpdateMode,
    /// Forgejo/Gitea base URL, or `https://github.com`.
    pub server: String,
    /// `owner/name` on `server`.
    pub repo: String,
    /// Access token for `server` (Forgejo needs one when it requires sign-in).
    pub token: Option<String>,
    /// GitHub `owner/name` to use when `server` is Forgejo and no token is set; "" disables.
    pub github_repo: String,
    /// ntfy topic URL that announces new releases; "" disables push checks.
    pub topic: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateMode {
    /// Show a notification with an Install button.
    #[default]
    Notify,
    /// Install as soon as a release is found.
    Auto,
    Off,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        UpdateConfig {
            mode: UpdateMode::Notify,
            server: "https://forgejo.terrell.ltd".into(),
            repo: "dave/ntfy-toast".into(),
            token: None,
            github_repo: "daterrell/ntfy-toast".into(),
            topic: "https://ntfy.terrell.ltd/ntfy-toast-releases".into(),
        }
    }
}

impl UpdateConfig {
    /// The release topic split into (server, topic id), if push checks are enabled.
    pub fn topic_parts(&self) -> Option<(String, String)> {
        if self.mode == UpdateMode::Off || self.topic.is_empty() {
            return None;
        }
        let (server, topic) = self.topic.rsplit_once('/')?;
        Some((server.to_string(), topic.to_string()))
    }
}

fn valid_topic_id(t: &str) -> bool {
    !t.is_empty()
        && t.len() <= 64
        && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subscription {
    pub topic: Option<Topic>,
    #[serde(default)]
    pub topics: Vec<Topic>,
    pub server: Option<String>,
    pub token: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    #[serde(default = "one")]
    pub min_priority: u8,
    pub name: Option<String>,
}

/// A topic id, optionally with a display name: `"alerts"` or `{ id = "alerts", name = "Alerts" }`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(
    untagged,
    expecting = "a topic id like \"alerts\", or a table like { id = \"alerts\", name = \"Alerts\" }"
)]
pub enum Topic {
    Id(String),
    Named(NamedTopic),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamedTopic {
    pub id: String,
    pub name: String,
}

impl Topic {
    pub fn id(&self) -> &str {
        match self {
            Topic::Id(id) => id,
            Topic::Named(t) => &t.id,
        }
    }

    pub fn name(&self) -> Option<&str> {
        match self {
            Topic::Id(_) => None,
            Topic::Named(t) => Some(&t.name),
        }
    }
}

fn default_server() -> String {
    "https://ntfy.sh".into()
}
fn default_app_id() -> String {
    "ntfy-toast".into()
}
fn default_display_name() -> String {
    "ntfy".into()
}
fn yes() -> bool {
    true
}
fn one() -> u8 {
    1
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("in config {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Config> {
        let mut cfg: Config = toml::from_str(text)?;
        cfg.default_server = normalize_server(&cfg.default_server)?;
        let u = &mut cfg.update;
        u.server = normalize_server(&u.server).context("in [update] server")?;
        for (key, repo) in [("repo", &u.repo), ("github_repo", &u.github_repo)] {
            let ok = repo.split('/').filter(|p| !p.is_empty()).count() == 2;
            if !ok && !(key == "github_repo" && repo.is_empty()) {
                bail!("[update] {key} must look like \"owner/name\", not {repo:?}");
            }
        }
        if u.token.as_deref().is_some_and(|t| t.trim().is_empty()) {
            u.token = None;
        }
        if !u.topic.is_empty() {
            let topic = u.topic.trim().trim_end_matches('/').to_string();
            let parts = topic
                .split_once("://")
                .and_then(|(_, rest)| rest.split_once('/'))
                .filter(|(host, id)| !host.is_empty() && valid_topic_id(id));
            if !(topic.starts_with("https://") || topic.starts_with("http://")) || parts.is_none() {
                bail!("[update] topic must be a URL like \"https://ntfy.sh/my-releases\", not {topic:?}");
            }
            u.topic = topic;
        }
        if cfg.subscriptions.is_empty() {
            bail!("no [[subscription]] entries");
        }
        for sub in &mut cfg.subscriptions {
            if let Some(s) = &sub.server {
                sub.server = Some(normalize_server(s)?);
            }
            let topics = sub.all_topics();
            if topics.is_empty() {
                bail!("subscription has no `topic` or `topics`");
            }
            for t in &topics {
                if t.is_empty()
                    || t.len() > 64
                    || !t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                {
                    bail!("invalid topic id {t:?} (allowed: A-Z a-z 0-9 _ -, max 64 chars)");
                }
            }
            for t in sub.topic.iter().chain(&sub.topics) {
                if t.name().is_some_and(|n| n.trim().is_empty()) {
                    bail!("topic {:?} has an empty `name`", t.id());
                }
            }
            if sub.token.is_some() && (sub.username.is_some() || sub.password.is_some()) {
                bail!("subscription for {topics:?} sets both `token` and `username`/`password`");
            }
            if sub.username.is_some() != sub.password.is_some() {
                bail!("subscription for {topics:?} needs both `username` and `password`");
            }
            if !(1..=5).contains(&sub.min_priority) {
                bail!("min_priority must be 1-5");
            }
        }
        Ok(cfg)
    }
}

impl Subscription {
    /// Topic ids, in config order, without duplicates.
    pub fn all_topics(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for t in self.topic.iter().chain(&self.topics) {
            if !v.iter().any(|x| x == t.id()) {
                v.push(t.id().to_string());
            }
        }
        v
    }

    /// The configured display name for a topic id, if any.
    pub fn topic_name(&self, id: &str) -> Option<&str> {
        self.topic
            .iter()
            .chain(&self.topics)
            .find(|t| t.id() == id && t.name().is_some())
            .and_then(Topic::name)
            .map(str::trim)
    }

    pub fn server<'a>(&'a self, cfg: &'a Config) -> &'a str {
        self.server.as_deref().unwrap_or(&cfg.default_server)
    }

    /// Value for the HTTP `Authorization` header, if credentials are configured.
    pub fn auth_header(&self) -> Option<String> {
        use base64::Engine;
        if let Some(t) = &self.token {
            return Some(format!("Bearer {t}"));
        }
        match (&self.username, &self.password) {
            (Some(u), Some(p)) => Some(format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode(format!("{u}:{p}"))
            )),
            _ => None,
        }
    }

    /// Stable identifier used to remember the last-seen message across restarts.
    pub fn state_key(&self, cfg: &Config) -> String {
        format!("{}/{}", self.server(cfg), self.all_topics().join(","))
    }
}

fn normalize_server(s: &str) -> Result<String> {
    let s = s.trim().trim_end_matches('/');
    let s = if s.contains("://") {
        s.to_string()
    } else {
        format!("https://{s}")
    };
    if !(s.starts_with("https://") || s.starts_with("http://")) {
        bail!("server must be an http(s) URL: {s}");
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_parses() {
        let cfg = Config::parse(SAMPLE).unwrap();
        assert_eq!(cfg.subscriptions.len(), 1);
        assert_eq!(cfg.app_id, "ntfy-toast");
    }

    #[test]
    fn normalizes_and_validates() {
        let cfg = Config::parse(
            r#"
            default_server = "ntfy.example.com/"
            [[subscription]]
            topics = ["a", "b_c"]
            username = "u"
            password = "p"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.default_server, "https://ntfy.example.com");
        let s = &cfg.subscriptions[0];
        assert_eq!(s.state_key(&cfg), "https://ntfy.example.com/a,b_c");
        assert_eq!(s.auth_header().unwrap(), "Basic dTpw");

        assert!(Config::parse("[[subscription]]\ntopic = \"bad topic\"").is_err());
        assert!(Config::parse("[[subscription]]\ntopic = \"a\"\nusername = \"u\"").is_err());
        assert!(Config::parse("").is_err());
    }

    #[test]
    fn update_defaults_and_overrides() {
        let cfg = Config::parse("[[subscription]]\ntopic = \"a\"").unwrap();
        assert_eq!(cfg.update, UpdateConfig::default());
        assert_eq!(
            cfg.update.topic_parts(),
            Some(("https://ntfy.terrell.ltd".into(), "ntfy-toast-releases".into()))
        );

        let cfg = Config::parse(
            r#"
            [[subscription]]
            topic = "a"
            [update]
            mode = "auto"
            server = "github.com/"
            repo = "someone/ntfy-toast"
            token = " "
            topic = ""
            "#,
        )
        .unwrap();
        assert_eq!(cfg.update.mode, UpdateMode::Auto);
        assert_eq!(cfg.update.server, "https://github.com");
        assert_eq!(cfg.update.token, None);
        assert_eq!(cfg.update.topic_parts(), None);

        let off = Config::parse("[[subscription]]\ntopic = \"a\"\n[update]\nmode = \"off\"").unwrap();
        assert_eq!(off.update.topic_parts(), None);

        for bad in [
            "mode = \"sometimes\"",
            "repo = \"just-a-name\"",
            "topic = \"ntfy.sh/x\"",
            "topic = \"https://ntfy.sh/bad topic\"",
            "colour = \"teal\"",
        ] {
            let text = format!("[[subscription]]\ntopic = \"a\"\n[update]\n{bad}");
            assert!(Config::parse(&text).is_err(), "{bad}");
        }
    }

    #[test]
    fn topics_can_have_names() {
        // Mixed plain ids and named tables, spread over lines with comments.
        let cfg = Config::parse(
            r#"
            [[subscription]]
            topic = { id = "solo", name = "Solo" }
            topics = [
              { id = "CgbRePrzudWiLmVl", name = "Frigate" }, # cameras
              "HomeAssistant",
              { id = "dAr9b0rpwA9kOS40", name = "VORON" },
              ]
            "#,
        )
        .unwrap();
        let s = &cfg.subscriptions[0];
        assert_eq!(
            s.all_topics(),
            ["solo", "CgbRePrzudWiLmVl", "HomeAssistant", "dAr9b0rpwA9kOS40"]
        );
        assert_eq!(s.topic_name("CgbRePrzudWiLmVl"), Some("Frigate"));
        assert_eq!(s.topic_name("solo"), Some("Solo"));
        assert_eq!(s.topic_name("HomeAssistant"), None);
        // Names don't change which messages a saved position belongs to.
        assert_eq!(
            s.state_key(&cfg),
            "https://ntfy.sh/solo,CgbRePrzudWiLmVl,HomeAssistant,dAr9b0rpwA9kOS40"
        );
    }

    #[test]
    fn bad_topic_entries_are_explained() {
        let err = Config::parse("[[subscription]]\ntopics = [{ id = \"a\", nmae = \"A\" }]")
            .unwrap_err()
            .to_string();
        assert!(err.contains("a topic id like"), "{err}");
        assert!(Config::parse("[[subscription]]\ntopics = [{ id = \"a\", name = \" \" }]").is_err());
        assert!(
            Config::parse("[[subscription]]\ntopics = [{ id = \"bad id\", name = \"x\" }]").is_err()
        );
    }
}
