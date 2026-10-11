// No console window when started from Explorer or the Run key.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod config;
mod daemon;
mod message;
mod paths;
mod subscriber;
mod system;
mod toast;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use config::Config;
use log::{error, LevelFilter};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

#[derive(Parser)]
#[command(version, about = "Subscribe to ntfy topics and show them as Windows toast notifications")]
struct Cli {
    /// Config file [default: %APPDATA%\ntfy-toast\config.toml]
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,

    /// Log debug details (connection attempts, skipped messages)
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the daemon (the default when no command is given); config edits apply live
    Run,
    /// Write a sample config file
    Init {
        /// Overwrite an existing config
        #[arg(long)]
        force: bool,
    },
    /// Start automatically at login, and start now
    Install,
    /// Stop starting at login and remove the notification registration
    Uninstall,
    /// Show a test notification
    Test {
        #[arg(long, default_value = "ntfy-toast works")]
        title: String,
        #[arg(long, default_value = "Notifications from your ntfy topics will look like this.")]
        message: String,
        /// 1 (min) to 5 (urgent)
        #[arg(long, default_value_t = 3)]
        priority: u8,
    },
    /// Print config, log and data locations
    Paths,
}

struct Logger {
    file: Option<Mutex<std::fs::File>>,
    level: LevelFilter,
}

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= self.level && m.target().starts_with("ntfy_toast")
    }
    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let line = format!("{} {:<5} {}\n", fmt_utc(secs), r.level(), r.args());
        eprint!("{line}");
        if let Some(f) = &self.file {
            let _ = f.lock().unwrap().write_all(line.as_bytes());
        }
    }
    fn flush(&self) {}
}

/// `YYYY-MM-DD HH:MM:SSZ` without pulling in a date crate.
fn fmt_utc(secs: u64) -> String {
    let (days, rem) = ((secs / 86400) as i64, secs % 86400);
    // Civil-from-days (Howard Hinnant).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

fn init_logging(verbose: bool, to_file: bool) {
    let file = to_file.then(|| {
        let path = paths::log_file();
        let _ = std::fs::create_dir_all(paths::data_dir());
        // Keep the log from growing without bound.
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1024 * 1024) {
            let _ = std::fs::rename(&path, path.with_extension("log.old"));
        }
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()
            .map(Mutex::new)
    });
    let level = if verbose {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };
    let logger = Logger {
        file: file.flatten(),
        level,
    };
    if log::set_logger(Box::leak(Box::new(logger))).is_ok() {
        log::set_max_level(level);
    }
}

fn write_sample_config(path: &PathBuf, force: bool) -> Result<bool> {
    if path.exists() && !force {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, config::SAMPLE)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(true)
}

/// Write the icon and register the toast identity. Failures here only cost cosmetics.
fn register(app_id: &str, display_name: &str) {
    let icon = paths::icon_file();
    let _ = std::fs::create_dir_all(paths::data_dir());
    if let Err(e) = std::fs::write(&icon, ICON_PNG) {
        error!("could not write icon: {e}");
    }
    if let Err(e) = system::register_app_id(app_id, display_name, &icon) {
        error!("could not register notification identity: {e:#}");
    }
}

/// Show one toast synchronously (used by `test` and for startup problems).
fn show_direct(cfg: &Config, title: &str, message: &str, priority: u8, topic: &str) -> Result<()> {
    let n = subscriber::Notice {
        msg: message::Message {
            id: format!("local-{}", std::process::id()),
            event: "message".into(),
            topic: topic.into(),
            title: Some(title.into()),
            message: Some(message.into()),
            priority: Some(priority),
            ..Default::default()
        },
        server: cfg.default_server.clone(),
        auth: None,
        source_name: Some("ntfy-toast".into()),
    };
    let mut c = cfg.clone();
    c.click_opens_topic = false;
    let t = toast::build(&n, &c, None, None);
    toast::platform::Notifier::new(&cfg.app_id)?.show(&t, &t.xml)
}

fn main() {
    system::attach_console();
    let cli = Cli::parse();
    let config_path = cli.config.clone().unwrap_or_else(paths::default_config_file);
    let command = cli.command.unwrap_or(Command::Run);
    init_logging(cli.verbose, matches!(command, Command::Run));

    let result = match command {
        Command::Run => daemon::run(config_path),
        Command::Init { force } => write_sample_config(&config_path, force).map(|wrote| {
            if wrote {
                println!("Wrote {}", config_path.display());
            } else {
                println!(
                    "{} already exists (use --force to overwrite)",
                    config_path.display()
                );
            }
        }),
        Command::Install => (|| {
            system::set_autostart(true)?;
            let cfg = Config::load(&config_path).or_else(|_| Config::parse(config::SAMPLE))?;
            register(&cfg.app_id, &cfg.app_display_name);
            let mut cmd = std::process::Command::new(std::env::current_exe()?);
            if let Some(c) = &cli.config {
                cmd.arg("--config").arg(c);
            }
            cmd.arg("run").spawn()?;
            println!("Installed: ntfy-toast will start at login, and is running now.");
            println!("Config: {}", config_path.display());
            Ok(())
        })(),
        Command::Uninstall => (|| {
            system::set_autostart(false)?;
            let cfg = Config::load(&config_path).or_else(|_| Config::parse(config::SAMPLE))?;
            system::unregister_app_id(&cfg.app_id)?;
            println!("Removed autostart and notification registration.");
            println!("A running instance keeps going until you sign out or run:");
            println!("  taskkill /im ntfy-toast.exe");
            Ok(())
        })(),
        Command::Test {
            title,
            message,
            priority,
        } => (|| {
            let cfg = Config::load(&config_path).or_else(|_| Config::parse(config::SAMPLE))?;
            register(&cfg.app_id, &cfg.app_display_name);
            show_direct(&cfg, &title, &message, priority.clamp(1, 5), "test")?;
            println!("Test notification sent.");
            Ok(())
        })(),
        Command::Paths => {
            println!("config: {}", config_path.display());
            println!("log:    {}", paths::log_file().display());
            println!("state:  {}", paths::state_file().display());
            println!("cache:  {}", paths::cache_dir().display());
            Ok(())
        }
    };

    if let Err(e) = result {
        error!("{e:#}");
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn formats_utc() {
        assert_eq!(super::fmt_utc(0), "1970-01-01 00:00:00Z");
        assert_eq!(super::fmt_utc(1_760_183_999), "2025-10-11 11:59:59Z");
    }
}
