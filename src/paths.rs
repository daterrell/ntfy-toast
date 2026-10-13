use std::path::PathBuf;

const APP_DIR: &str = "ntfy-toast";

fn env_dir(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(fallback)))
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_DIR)
}

/// Roaming settings: `%APPDATA%\ntfy-toast`.
pub fn config_dir() -> PathBuf {
    env_dir("APPDATA", ".config")
}

/// Overrides the data directory, e.g. to run a second, isolated copy for testing.
pub const DATA_DIR_ENV: &str = "NTFY_TOAST_DATA_DIR";

/// Machine-local data (log, state, image cache): `%LOCALAPPDATA%\ntfy-toast`,
/// or `$NTFY_TOAST_DATA_DIR` if set.
pub fn data_dir() -> PathBuf {
    match std::env::var_os(DATA_DIR_ENV) {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => env_dir("LOCALAPPDATA", ".local/share"),
    }
}

/// Name of the single-instance lock: one daemon per data directory.
pub fn instance_name() -> String {
    use std::hash::{Hash, Hasher};
    match std::env::var_os(DATA_DIR_ENV) {
        Some(dir) if !dir.is_empty() => {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            dir.hash(&mut h);
            format!(r"Local\ntfy-toast-daemon-{:016x}", h.finish())
        }
        _ => r"Local\ntfy-toast-daemon".to_string(),
    }
}

pub fn default_config_file() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn log_file() -> PathBuf {
    data_dir().join("ntfy-toast.log")
}

pub fn state_file() -> PathBuf {
    data_dir().join("state.json")
}

pub fn cache_dir() -> PathBuf {
    data_dir().join("cache")
}

pub fn icon_file() -> PathBuf {
    data_dir().join("icon.png")
}
