//! The long-running daemon: starts subscriptions, and applies config file changes live.

use crate::config::{self, Config, Subscription, Topic};
use crate::toast::SharedConfig;
use crate::{paths, register, show_direct, subscriber, system, toast, updater, write_sample_config};
use anyhow::Result;
use log::{debug, error, info, warn};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, RwLock};
use std::time::{Duration, SystemTime};

const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Editors often write a file in several steps; wait for it to stop changing.
const SETTLE_TIME: Duration = Duration::from_millis(500);
const PLACEHOLDER_TOPIC: &str = "change-me-to-your-topic";
/// Give the network a moment after login before the first update check.
const FIRST_UPDATE_CHECK: Duration = Duration::from_secs(30);

/// A running subscription is identified by its full settings plus its resolved server,
/// so changing anything about it (or `default_server`) restarts it.
type SubKey = (String, Subscription);

fn sub_key(cfg: &Config, sub: &Subscription) -> SubKey {
    (sub.server(cfg).to_string(), sub.clone())
}

fn is_placeholder(sub: &Subscription) -> bool {
    sub.all_topics().iter().any(|t| t == PLACEHOLDER_TOPIC)
}

/// What a reload has to do to get from `running` to `cfg`.
#[derive(Debug, Default, PartialEq)]
struct Plan {
    stop: Vec<SubKey>,
    /// Subscriptions to start, and whether each resumes from its saved message id.
    start: Vec<(Subscription, bool)>,
}

fn plan<'a>(running: impl IntoIterator<Item = &'a SubKey>, cfg: &Config) -> Plan {
    let wanted: HashSet<SubKey> = cfg
        .subscriptions
        .iter()
        .filter(|s| !is_placeholder(s))
        .map(|s| sub_key(cfg, s))
        .collect();
    let running: HashSet<&SubKey> = running.into_iter().collect();

    let stop: Vec<SubKey> = running
        .iter()
        .filter(|k| !wanted.contains(**k))
        .map(|k| (*k).clone())
        .collect();
    // Only resume when the same server+topics were being followed until just now;
    // an old saved id for a newly added topic set could replay hours of cached messages.
    let stopped_streams: HashSet<String> = stop
        .iter()
        .map(|(server, sub)| format!("{server}/{}", sub.all_topics().join(",")))
        .collect();

    let mut start = Vec::new();
    for s in &cfg.subscriptions {
        let key = sub_key(cfg, s);
        if is_placeholder(s) || running.contains(&key) || start.iter().any(|(x, _)| x == s) {
            continue;
        }
        let resume = stopped_streams.contains(&s.state_key(cfg));
        start.push((s.clone(), resume));
    }
    Plan { stop, start }
}

struct Supervisor {
    shared: SharedConfig,
    state: Arc<subscriber::State>,
    tx: mpsc::Sender<subscriber::Notice>,
    running: HashMap<SubKey, Arc<AtomicBool>>,
    /// Messages on the release topic go here (they trigger an update check, not a toast).
    update_tx: mpsc::Sender<subscriber::Notice>,
    update_sub: Option<((String, String), Arc<AtomicBool>)>,
}

impl Supervisor {
    fn apply(&mut self, cfg: Config) -> Result<()> {
        let old = self.shared.read().unwrap().clone();
        if old.app_id != cfg.app_id {
            let _ = system::unregister_app_id(&old.app_id);
        }
        register(&cfg.app_id, &cfg.app_display_name);

        let cfg = Arc::new(cfg);
        *self.shared.write().unwrap() = cfg.clone();

        let plan = plan(self.running.keys(), &cfg);
        for key in &plan.stop {
            if let Some(flag) = self.running.remove(key) {
                flag.store(true, Ordering::Relaxed);
            }
        }
        for (sub, resume) in plan.start {
            let stop = Arc::new(AtomicBool::new(false));
            self.running.insert(sub_key(&cfg, &sub), stop.clone());
            let (cfg, state, tx) = (cfg.clone(), self.state.clone(), self.tx.clone());
            std::thread::Builder::new()
                .name("subscriber".into())
                .spawn(move || subscriber::run(sub, cfg, state, tx, stop, resume))?;
        }
        self.apply_update_topic(&cfg)
    }

    /// Follow the release topic from `[update]`, restarting the subscription if it changed.
    fn apply_update_topic(&mut self, cfg: &Arc<Config>) -> Result<()> {
        let wanted = cfg.update.topic_parts();
        if self.update_sub.as_ref().map(|(k, _)| k) == wanted.as_ref() {
            return Ok(());
        }
        if let Some((_, stop)) = self.update_sub.take() {
            stop.store(true, Ordering::Relaxed);
        }
        let Some((server, topic)) = wanted else {
            return Ok(());
        };
        let sub = Subscription {
            topic: Some(Topic::Id(topic.clone())),
            topics: Vec::new(),
            server: Some(server.clone()),
            token: None,
            username: None,
            password: None,
            min_priority: 1,
            name: None,
        };
        // Releases missed while offline are found by the startup check instead.
        let mut sub_cfg = (**cfg).clone();
        sub_cfg.catch_up_on_start = false;
        let sub_cfg = Arc::new(sub_cfg);
        let stop = Arc::new(AtomicBool::new(false));
        self.update_sub = Some(((server, topic), stop.clone()));
        let (state, tx) = (self.state.clone(), self.update_tx.clone());
        std::thread::Builder::new()
            .name("release-topic".into())
            .spawn(move || subscriber::run(sub, sub_cfg, state, tx, stop, false))?;
        Ok(())
    }
}

fn file_stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

fn warn_placeholder(cfg: &Config, path: &Path) {
    if cfg.subscriptions.iter().any(is_placeholder) {
        let _ = show_direct(
            cfg,
            "ntfy-toast needs a topic",
            &format!("Edit {} — changes apply automatically.", path.display()),
            4,
            "ntfy-toast",
        );
    }
}

fn report_config_error(cfg: &Config, path: &Path, e: &anyhow::Error, keeping_old: bool) {
    error!("{e:#}");
    let what = if keeping_old {
        "Keeping the previous settings until it's fixed."
    } else {
        "Waiting for a valid config."
    };
    let _ = show_direct(
        cfg,
        "ntfy-toast: config problem",
        &format!("{e:#}\n\n{what}\n{}", path.display()),
        4,
        "ntfy-toast",
    );
}

pub fn run(config_path: PathBuf) -> Result<()> {
    // Set when the previous version restarted us after installing this one.
    let updated_from = std::env::var(updater::UPDATED_FROM_ENV).ok();
    std::env::remove_var(updater::UPDATED_FROM_ENV);
    let wait = if updated_from.is_some() {
        Duration::from_secs(30)
    } else {
        Duration::ZERO
    };
    if !system::acquire_single_instance(&paths::instance_name(), wait) {
        info!("already running; exiting");
        return Ok(());
    }
    info!("ntfy-toast {} starting", crate::VERSION);
    updater::cleanup_old();

    if write_sample_config(&config_path, false)? {
        info!("created sample config at {}", config_path.display());
    }
    toast::prune_cache();

    // Settings with no subscriptions, used until a valid config has been loaded.
    let mut idle = Config::parse(config::SAMPLE)?;
    idle.subscriptions.clear();

    let (tx, rx) = mpsc::channel();
    let shared: SharedConfig = Arc::new(RwLock::new(Arc::new(idle.clone())));
    let (update_tx, update_rx) = mpsc::channel::<subscriber::Notice>();
    let (event_tx, event_rx) = mpsc::channel();
    {
        let (shared, toasts) = (shared.clone(), tx.clone());
        std::thread::Builder::new()
            .name("updater".into())
            .spawn(move || updater::run(event_rx, shared, toasts))?;
    }
    {
        let (events, shared) = (event_tx.clone(), shared.clone());
        std::thread::Builder::new()
            .name("release-announcements".into())
            .spawn(move || {
                for n in update_rx {
                    // CI tags development-build announcements "dev"; only that channel wants them.
                    let channel = shared.read().unwrap().update.channel;
                    if channel == crate::config::Channel::Stable && n.msg.tags.iter().any(|t| t == "dev") {
                        debug!("release topic posted a dev build ({}); ignored on the stable channel", n.msg.id);
                        continue;
                    }
                    info!("release topic posted ({}); checking for updates", n.msg.id);
                    let _ = events.send(updater::Event::Check);
                }
            })?;
    }
    {
        let events = event_tx.clone();
        std::thread::Builder::new()
            .name("install-requests".into())
            .spawn(move || {
                system::wait_for_signals(&paths::update_event_name(), || {
                    info!("install requested");
                    let _ = events.send(updater::Event::Install);
                })
            })?;
    }
    std::thread::Builder::new()
        .name("first-update-check".into())
        .spawn(move || {
            std::thread::sleep(FIRST_UPDATE_CHECK);
            let _ = event_tx.send(updater::Event::Check);
        })?;

    let mut sup = Supervisor {
        shared: shared.clone(),
        state: Arc::new(subscriber::State::load(paths::state_file())),
        tx,
        running: HashMap::new(),
        update_tx,
        update_sub: None,
    };
    std::thread::Builder::new()
        .name("toast".into())
        .spawn(move || toast::run(rx, shared))?;

    let mut stamp = file_stamp(&config_path);
    let mut loaded = match Config::load(&config_path) {
        Ok(cfg) => {
            warn_placeholder(&cfg, &config_path);
            sup.apply(cfg.clone())?;
            if let Some(from) = &updated_from {
                info!("updated from {from}");
                let _ = show_direct(
                    &cfg,
                    &format!("ntfy-toast updated to {}", crate::VERSION),
                    &format!("Previously {from}."),
                    2,
                    "ntfy-toast",
                );
            }
            Some(cfg)
        }
        Err(e) => {
            register(&idle.app_id, &idle.app_display_name);
            report_config_error(&idle, &config_path, &e, false);
            None
        }
    };
    info!("watching {} for changes", config_path.display());

    loop {
        std::thread::sleep(POLL_INTERVAL);
        let now = file_stamp(&config_path);
        if now == stamp {
            continue;
        }
        std::thread::sleep(SETTLE_TIME);
        if file_stamp(&config_path) != now {
            continue; // still being written; look again next tick
        }
        stamp = now;
        if stamp.is_none() {
            warn!("config file is gone; keeping current settings");
            continue;
        }

        match Config::load(&config_path) {
            Ok(cfg) if loaded.as_ref() == Some(&cfg) => {}
            Ok(cfg) => {
                let n = cfg.subscriptions.len();
                info!("config changed; reloading ({n} subscription(s))");
                warn_placeholder(&cfg, &config_path);
                sup.apply(cfg.clone())?;
                let _ = show_direct(
                    &cfg,
                    "ntfy-toast settings updated",
                    &format!("Now following {n} subscription(s)."),
                    2,
                    "ntfy-toast",
                );
                loaded = Some(cfg);
            }
            Err(e) => {
                let cfg = sup.shared.read().unwrap().clone();
                report_config_error(&cfg, &config_path, &e, loaded.is_some());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(text: &str) -> Config {
        Config::parse(text).unwrap()
    }

    fn keys(c: &Config) -> Vec<SubKey> {
        c.subscriptions.iter().map(|s| sub_key(c, s)).collect()
    }

    #[test]
    fn initial_start_skips_placeholder() {
        let c = cfg(config::SAMPLE);
        assert_eq!(plan(&[], &c), Plan::default());
    }

    #[test]
    fn unchanged_subscriptions_keep_running() {
        let c = cfg("[[subscription]]\ntopic = \"a\"\n[[subscription]]\ntopic = \"b\"");
        let p = plan(&keys(&c), &c);
        assert_eq!(p, Plan::default());
    }

    #[test]
    fn changed_settings_restart_and_resume() {
        let old = cfg("[[subscription]]\ntopic = \"a\"\n[[subscription]]\ntopic = \"b\"");
        let new = cfg("[[subscription]]\ntopic = \"a\"\nmin_priority = 4\n[[subscription]]\ntopic = \"c\"");
        let p = plan(&keys(&old), &new);
        let mut stopped: Vec<_> = p.stop.iter().map(|(_, s)| s.all_topics()).collect();
        stopped.sort();
        assert_eq!(stopped, vec![vec!["a".to_string()], vec!["b".to_string()]]);
        let started: Vec<_> = p
            .start
            .iter()
            .map(|(s, r)| (s.all_topics()[0].clone(), *r))
            .collect();
        // `a` changed only its filter: resume. `c` is new: start fresh.
        assert_eq!(started, vec![("a".into(), true), ("c".into(), false)]);
    }

    #[test]
    fn default_server_change_restarts_without_resume() {
        let old = cfg("[[subscription]]\ntopic = \"a\"");
        let new = cfg("default_server = \"https://other.example\"\n[[subscription]]\ntopic = \"a\"");
        let p = plan(&keys(&old), &new);
        assert_eq!(p.stop.len(), 1);
        assert_eq!(p.start.len(), 1);
        assert!(!p.start[0].1);
    }

    #[test]
    fn duplicate_entries_start_once() {
        let c = cfg("[[subscription]]\ntopic = \"a\"\n[[subscription]]\ntopic = \"a\"");
        assert_eq!(plan(&[], &c).start.len(), 1);
    }
}
