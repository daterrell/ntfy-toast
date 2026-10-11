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

# Self-hosted server with access control, several topics, and a priority filter:
# [[subscription]]
# server = "https://ntfy.example.com"
# topics = ["alerts", "backups"]
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
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subscription {
    pub topic: Option<String>,
    #[serde(default)]
    pub topics: Vec<String>,
    pub server: Option<String>,
    pub token: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    #[serde(default = "one")]
    pub min_priority: u8,
    pub name: Option<String>,
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
                    bail!("invalid topic name {t:?} (allowed: A-Z a-z 0-9 _ -, max 64 chars)");
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
    pub fn all_topics(&self) -> Vec<String> {
        let mut v: Vec<String> = self.topic.iter().cloned().collect();
        for t in &self.topics {
            if !v.contains(t) {
                v.push(t.clone());
            }
        }
        v
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
}
