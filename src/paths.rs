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

/// Machine-local data (log, state, image cache): `%LOCALAPPDATA%\ntfy-toast`.
pub fn data_dir() -> PathBuf {
    env_dir("LOCALAPPDATA", ".local/share")
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
