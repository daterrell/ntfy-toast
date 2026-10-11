use crate::config::{Config, Subscription};
use crate::message::Message;
use log::{debug, error, info, warn};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// ntfy sends a keepalive every 45s; anything quieter than this means the link is dead.
const READ_TIMEOUT: Duration = Duration::from_secs(100);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// A message to display, plus what's needed to fetch its images.
pub struct Notice {
    pub msg: Message,
    pub server: String,
    pub auth: Option<String>,
    pub source_name: Option<String>,
}

/// Last-seen message id per subscription, persisted so `catch_up_on_start` can resume.
pub struct State {
    path: PathBuf,
    ids: Mutex<HashMap<String, String>>,
}

impl State {
    pub fn load(path: PathBuf) -> State {
        let ids = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        State {
            path,
            ids: Mutex::new(ids),
        }
    }

    fn get(&self, key: &str) -> Option<String> {
        self.ids.lock().unwrap().get(key).cloned()
    }

    fn set(&self, key: &str, id: &str) {
        let mut ids = self.ids.lock().unwrap();
        ids.insert(key.to_string(), id.to_string());
        if let Ok(json) = serde_json::to_string_pretty(&*ids) {
            let tmp = self.path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }
}

pub fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(READ_TIMEOUT)
        .user_agent(concat!("ntfy-toast/", env!("CARGO_PKG_VERSION")))
        .build()
}

fn stream_url(server: &str, topics: &[String], since: Option<&str>) -> String {
    let mut url = format!("{server}/{}/json", topics.join(","));
    if let Some(s) = since {
        url.push_str("?since=");
        url.push_str(s);
    }
    url
}

/// Runs forever: keeps one streaming connection open for `sub`, reconnecting with backoff.
pub fn run(sub: Subscription, cfg: Arc<Config>, state: Arc<State>, tx: Sender<Notice>) {
    let server = sub.server(&cfg).to_string();
    let topics = sub.all_topics();
    let key = sub.state_key(&cfg);
    let auth = sub.auth_header();
    let agent = agent();
    let label = format!("{server}/{}", topics.join(","));

    // `since` cursor: a message id, or a unix timestamp before the first message arrives.
    let mut since: Option<String> = if cfg.catch_up_on_start {
        state.get(&key)
    } else {
        None
    };
    let mut recent: VecDeque<String> = VecDeque::new();
    let mut backoff = Duration::from_secs(1);

    loop {
        let url = stream_url(&server, &topics, since.as_deref());
        debug!("[{label}] connecting: {url}");
        let mut req = agent.get(&url);
        if let Some(a) = &auth {
            req = req.set("Authorization", a);
        }
        let started = Instant::now();
        match req.call() {
            Ok(resp) => {
                for line in BufReader::new(resp.into_reader()).lines() {
                    let line = match line {
                        Ok(l) => l,
                        Err(e) => {
                            warn!("[{label}] connection lost: {e}");
                            break;
                        }
                    };
                    if line.trim().is_empty() {
                        continue;
                    }
                    let msg: Message = match serde_json::from_str(&line) {
                        Ok(m) => m,
                        Err(e) => {
                            warn!("[{label}] unparseable line ({e}): {line}");
                            continue;
                        }
                    };
                    match msg.event.as_str() {
                        "open" => {
                            info!("[{label}] subscribed");
                            backoff = Duration::from_secs(1);
                            if since.is_none() {
                                since = Some(msg.time.to_string());
                            }
                        }
                        "keepalive" => {
                            if since.as_deref().is_some_and(|s| s.parse::<i64>().is_ok()) {
                                since = Some(msg.time.to_string());
                            }
                        }
                        "message" => {
                            since = Some(msg.id.clone());
                            state.set(&key, &msg.id);
                            // A timestamp cursor can replay messages from the same second.
                            if recent.contains(&msg.id) {
                                continue;
                            }
                            recent.push_back(msg.id.clone());
                            if recent.len() > 100 {
                                recent.pop_front();
                            }
                            if msg.priority() < sub.min_priority {
                                debug!("[{label}] skipped low-priority message {}", msg.id);
                                continue;
                            }
                            info!("[{label}] message {} on {}", msg.id, msg.topic);
                            let notice = Notice {
                                msg,
                                server: server.clone(),
                                auth: auth.clone(),
                                source_name: sub.name.clone(),
                            };
                            if tx.send(notice).is_err() {
                                return;
                            }
                        }
                        other => debug!("[{label}] ignoring event {other}"),
                    }
                }
                if started.elapsed() > Duration::from_secs(30) {
                    backoff = Duration::from_secs(1);
                }
            }
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                error!("[{label}] server returned HTTP {code}: {}", body.trim());
                if code == 401 || code == 403 {
                    backoff = MAX_BACKOFF;
                }
            }
            Err(e) => warn!("[{label}] connect failed: {e}"),
        }
        debug!("[{label}] reconnecting in {backoff:?}");
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_stream_url() {
        let t = vec!["a".to_string(), "b".to_string()];
        assert_eq!(stream_url("https://x", &t, None), "https://x/a,b/json");
        assert_eq!(
            stream_url("https://x", &t, Some("abc")),
            "https://x/a,b/json?since=abc"
        );
    }
}
