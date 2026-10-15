//! Self-update: find the latest release, download and verify it, swap the exe, restart.
//!
//! Releases come from a Forgejo/Gitea server or GitHub (same JSON shape for what's used
//! here). The daemon checks at startup and whenever the release topic announces a new
//! version; the topic message only triggers a check, it's never trusted for content.

use crate::config::{Channel, UpdateConfig, UpdateMode};
use crate::message::Message;
use crate::subscriber::{error_chain, Http, Notice};
use crate::toast::SharedConfig;
use anyhow::{bail, ensure, Context, Result};
use log::{debug, info, warn};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};

/// Toast button URI that asks the running daemon to install the latest release.
pub const INSTALL_URI: &str = "ntfy-toast-action:update";
/// Set on the restarted process so it can wait for the old one and say what changed.
pub const UPDATED_FROM_ENV: &str = "NTFY_TOAST_UPDATED_FROM";

const EXE_ASSET: &str = "ntfy-toast.exe";
const SHA_ASSET: &str = "ntfy-toast.exe.sha256";
const MAX_EXE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Version {
    core: (u64, u64, u64),
    pre: Option<String>,
    /// `+build` metadata (e.g. the commit of a dev build): shown, but ignored when comparing.
    build: Option<String>,
}

impl Version {
    /// `1.2.3`, `v1.2.3`, `1.2.3-rc.1` or `1.2.3-dev.4+gabc1234`.
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.trim().trim_start_matches('v');
        let (s, build) = match s.split_once('+') {
            Some((v, b)) => (v, Some(b.to_string())),
            None => (s, None),
        };
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, Some(p.to_string())),
            None => (s, None),
        };
        if pre.as_deref() == Some("") || build.as_deref() == Some("") {
            return None;
        }
        let mut nums = core.split('.').map(|n| n.parse::<u64>().ok());
        let v = (nums.next()??, nums.next()??, nums.next()??);
        if nums.next().is_some() {
            return None;
        }
        Some(Version { core: v, pre, build })
    }

    pub fn current() -> Version {
        Version::parse(crate::VERSION).expect("package version")
    }
}

/// Semver pre-release order: dot-separated parts, numbers compared as numbers and before
/// words, and more parts sort after fewer (so `dev.10` > `dev.9`, `rc.1.dev.3` > `rc.1`).
fn cmp_pre(a: &str, b: &str) -> Ordering {
    let mut a = a.split('.');
    let mut b = b.split('.');
    loop {
        let ord = match (a.next(), b.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => match (x.parse::<u64>(), y.parse::<u64>()) {
                (Ok(x), Ok(y)) => x.cmp(&y),
                (Ok(_), Err(_)) => Ordering::Less,
                (Err(_), Ok(_)) => Ordering::Greater,
                (Err(_), Err(_)) => x.cmp(y),
            },
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.core.cmp(&other.core).then_with(|| match (&self.pre, &other.pre) {
            (None, None) => Ordering::Equal,
            // 1.2.3 is newer than 1.2.3-rc.1.
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(a), Some(b)) => cmp_pre(a, b),
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Version {}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (a, b, c) = self.core;
        write!(f, "{a}.{b}.{c}")?;
        if let Some(p) = &self.pre {
            write!(f, "-{p}")?;
        }
        if let Some(b) = &self.build {
            write!(f, "+{b}")?;
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
}

/// Where release information comes from, after applying the GitHub fallback.
#[derive(Debug, PartialEq)]
pub struct Source {
    pub latest_url: String,
    /// Set for the dev channel: list all releases (pre-releases included) instead.
    pub list_url: Option<String>,
    pub token: Option<String>,
    pub label: String,
}

impl Source {
    pub fn from_config(u: &UpdateConfig) -> Source {
        // Dev builds are published on Forgejo only, so GitHub always means stable.
        let github = |repo: &str| Source {
            latest_url: format!("https://api.github.com/repos/{repo}/releases/latest"),
            list_url: None,
            token: None,
            label: format!("github.com/{repo}"),
        };
        let host = u.server.split("://").nth(1).unwrap_or(&u.server);
        if host == "github.com" || host == "www.github.com" {
            return github(&u.repo);
        }
        if u.token.is_none() && !u.github_repo.is_empty() {
            return github(&u.github_repo);
        }
        let api = format!("{}/api/v1/repos/{}/releases", u.server, u.repo);
        Source {
            latest_url: format!("{api}/latest"),
            list_url: (u.channel == Channel::Dev).then(|| format!("{api}?draft=false&limit=20")),
            token: u.token.clone(),
            label: format!("{host}/{}", u.repo),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Release {
    pub version: Version,
    pub page: String,
    exe_url: String,
    sha_url: String,
    token: Option<String>,
}

fn get(http: &Http, url: &str, token: Option<&str>) -> Result<reqwest::blocking::Response> {
    let mut req = http.get(url).header("Accept", "application/json");
    if let Some(t) = token {
        req = req.header("Authorization", format!("token {t}"));
    }
    let resp = req.send().map_err(|e| anyhow::anyhow!(error_chain(&e)))?;
    let status = resp.status();
    ensure!(status.is_success(), "HTTP {status} from {url}");
    // Forgejo answers unauthenticated downloads by redirecting to its login page.
    let html = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| t.starts_with("text/html"));
    if html && !url.contains("/api/") {
        bail!("{url} returned a web page instead of a file (does the server need a token?)");
    }
    Ok(resp)
}

impl ApiRelease {
    fn into_release(self, token: Option<String>) -> Result<Release> {
        let version = Version::parse(&self.tag_name)
            .with_context(|| format!("release tag {:?} isn't a version", self.tag_name))?;
        let asset = |name: &str| {
            self.assets
                .iter()
                .find(|a| a.name == name)
                .map(|a| a.browser_download_url.clone())
                .with_context(|| format!("release {} has no {name}", self.tag_name))
        };
        Ok(Release {
            exe_url: asset(EXE_ASSET)?,
            sha_url: asset(SHA_ASSET)?,
            version,
            page: self.html_url,
            token,
        })
    }
}

/// The newest release for the configured channel: the latest formal release, or for
/// `channel = "dev"`, the highest version including the per-merge development builds.
pub fn latest(http: &Http, u: &UpdateConfig) -> Result<Release> {
    let src = Source::from_config(u);
    if u.channel == Channel::Dev && src.list_url.is_none() {
        warn!("channel = \"dev\" needs the Forgejo token (dev builds aren't on GitHub); checking formal releases only");
    }
    debug!("checking {} for updates", src.label);
    let fetch = |url: &str| -> Result<String> {
        get(http, url, src.token.as_deref())
            .and_then(|r| Ok(r.text()?))
            .with_context(|| format!("checking {} for updates", src.label))
    };
    let Some(list_url) = &src.list_url else {
        let rel: ApiRelease = serde_json::from_str(&fetch(&src.latest_url)?)
            .with_context(|| format!("bad release JSON from {}", src.label))?;
        return rel.into_release(src.token);
    };
    let rels: Vec<ApiRelease> = serde_json::from_str(&fetch(list_url)?)
        .with_context(|| format!("bad release list JSON from {}", src.label))?;
    // Skip drafts and releases that aren't finished (no exe yet) or aren't versions.
    rels.into_iter()
        .filter(|r| !r.draft)
        .filter_map(|r| r.into_release(src.token.clone()).ok())
        .max_by(|a, b| a.version.cmp(&b.version))
        .with_context(|| format!("no usable release found on {}", src.label))
}

/// The latest release, if it's newer than this build.
pub fn check(http: &Http, u: &UpdateConfig) -> Result<Option<Release>> {
    let rel = latest(http, u)?;
    Ok((rel.version > Version::current()).then_some(rel))
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// Download the release exe next to `exe` as `<exe>.new`, checking it against the published
/// SHA-256 and that it's a Windows executable.
pub fn download(http: &Http, rel: &Release, exe: &Path) -> Result<PathBuf> {
    let token = rel.token.as_deref();
    let sums = get(http, &rel.sha_url, token)?.text()?;
    let expected = sums
        .split_whitespace()
        .next()
        .filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()))
        .with_context(|| format!("no SHA-256 in {}", rel.sha_url))?
        .to_ascii_lowercase();

    let mut data = Vec::new();
    get(http, &rel.exe_url, token)?
        .take(MAX_EXE_BYTES + 1)
        .read_to_end(&mut data)
        .context("downloading the update")?;
    ensure!(data.len() as u64 <= MAX_EXE_BYTES, "the update is suspiciously large");
    let actual = sha256_hex(&data);
    ensure!(
        actual == expected,
        "checksum mismatch for {} (expected {expected}, got {actual})",
        rel.exe_url
    );
    ensure!(data.starts_with(b"MZ"), "the download isn't a Windows program");

    let new = sibling(exe, "new");
    std::fs::write(&new, &data).with_context(|| format!("writing {}", new.display()))?;
    Ok(new)
}

fn sibling(exe: &Path, ext: &str) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{ext}"));
    exe.with_file_name(name)
}

/// Run the downloaded exe's `--version` and make sure it's the release we expect.
#[cfg(windows)]
fn self_test(new: &Path, version: &Version) -> Result<()> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let mut child = Command::new(new)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("starting the downloaded update")?;
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait()?.is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            bail!("the downloaded update didn't respond");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let mut out = String::new();
    child.stdout.take().unwrap().read_to_string(&mut out)?;
    let reported = out.trim().strip_prefix("ntfy-toast ").and_then(Version::parse);
    ensure!(
        reported.as_ref() == Some(version),
        "the downloaded update reports {:?}, expected {version}",
        out.trim()
    );
    Ok(())
}

#[cfg(not(windows))]
fn self_test(_new: &Path, _version: &Version) -> Result<()> {
    Ok(())
}

/// Put `new` in place of `exe`. A running exe can't be overwritten on Windows, but it
/// can be renamed, so the current one moves to `<exe>.old` (deleted on the next start).
pub fn swap(exe: &Path, new: &Path) -> Result<()> {
    let old = sibling(exe, "old");
    if old.exists() {
        std::fs::remove_file(&old).with_context(|| format!("removing {}", old.display()))?;
    }
    std::fs::rename(exe, &old).with_context(|| format!("moving {} aside", exe.display()))?;
    if let Err(e) = std::fs::rename(new, exe) {
        let _ = std::fs::rename(&old, exe);
        return Err(e).with_context(|| format!("putting the update at {}", exe.display()));
    }
    Ok(())
}

/// Remove the previous exe left behind by an update (it was still running back then).
pub fn cleanup_old() {
    if let Ok(exe) = std::env::current_exe() {
        for ext in ["old", "new"] {
            let p = sibling(&exe, ext);
            if p.exists() {
                match std::fs::remove_file(&p) {
                    Ok(()) => debug!("removed {}", p.display()),
                    Err(e) => debug!("could not remove {}: {e}", p.display()),
                }
            }
        }
    }
}

/// Download, verify and swap in `rel`. Doesn't restart.
pub fn install(http: &Http, rel: &Release) -> Result<()> {
    let exe = std::env::current_exe()?;
    info!("downloading ntfy-toast {} from {}", rel.version, rel.exe_url);
    let new = download(http, rel, &exe)?;
    let result = self_test(&new, &rel.version).and_then(|()| swap(&exe, &new));
    if result.is_err() {
        let _ = std::fs::remove_file(&new);
    }
    result?;
    info!("installed ntfy-toast {} at {}", rel.version, exe.display());
    Ok(())
}

/// Start the (now updated) exe with this process's arguments. The caller exits right
/// after; the new process waits for this one to release the single-instance lock.
pub fn restart() -> Result<()> {
    let exe = std::env::current_exe()?;
    std::process::Command::new(&exe)
        .args(std::env::args_os().skip(1))
        .env(UPDATED_FROM_ENV, Version::current().to_string())
        .spawn()
        .with_context(|| format!("restarting {}", exe.display()))?;
    Ok(())
}

pub enum Event {
    /// Look for a release (at startup, or the release topic posted).
    Check,
    /// Install the latest release now (Install button or `ntfy-toast update`).
    Install,
}

fn notice(id: &str, title: String, body: String, click: Option<&str>, actions: serde_json::Value) -> Notice {
    let msg: Message = serde_json::from_value(serde_json::json!({
        "id": id,
        "event": "message",
        "topic": "ntfy-toast",
        "title": title,
        "message": body,
        "click": click,
        "actions": actions,
    }))
    .expect("valid message");
    Notice {
        msg,
        server: String::new(),
        auth: None,
        source_name: Some("ntfy-toast".into()),
        topic_name: None,
    }
}

fn available_notice(rel: &Release) -> Notice {
    notice(
        "ntfy-toast-update",
        format!("ntfy-toast {} is available", rel.version),
        format!("You have {}.", Version::current()),
        Some(&rel.page),
        serde_json::json!([
            { "action": "view", "label": "Install", "url": INSTALL_URI },
            { "action": "view", "label": "What's new", "url": rel.page },
        ]),
    )
}

fn message_notice(title: &str, body: String) -> Notice {
    notice("ntfy-toast-update", title.into(), body, None, serde_json::json!([]))
}

/// Updater thread: reacts to checks and install requests until the channel closes.
/// A successful install restarts the process.
pub fn run(rx: Receiver<Event>, shared: SharedConfig, toasts: Sender<Notice>) {
    let http = crate::subscriber::http();
    let mut announced: Option<Version> = None;
    for event in rx {
        let cfg = shared.read().unwrap().clone();
        let u = &cfg.update;
        let install_now = match event {
            Event::Check if u.mode == UpdateMode::Off => continue,
            Event::Check => match check(&http, u) {
                Ok(Some(rel)) if u.mode == UpdateMode::Auto => Some(rel),
                Ok(Some(rel)) => {
                    if announced.as_ref() != Some(&rel.version) {
                        info!("ntfy-toast {} is available", rel.version);
                        announced = Some(rel.version.clone());
                        let _ = toasts.send(available_notice(&rel));
                    }
                    None
                }
                Ok(None) => {
                    debug!("up to date ({})", Version::current());
                    None
                }
                Err(e) => {
                    warn!("update check failed: {e:#}");
                    None
                }
            },
            Event::Install => match check(&http, u) {
                Ok(Some(rel)) => Some(rel),
                Ok(None) => {
                    let _ = toasts.send(message_notice(
                        "ntfy-toast is up to date",
                        format!("Version {} is the latest.", Version::current()),
                    ));
                    None
                }
                Err(e) => {
                    warn!("update failed: {e:#}");
                    let _ = toasts.send(message_notice("ntfy-toast update failed", format!("{e:#}")));
                    None
                }
            },
        };
        if let Some(rel) = install_now {
            match install(&http, &rel).and_then(|()| restart()) {
                Ok(()) => {
                    info!("restarting into {}", rel.version);
                    std::process::exit(0);
                }
                Err(e) => {
                    warn!("update failed: {e:#}");
                    let _ = toasts.send(message_notice("ntfy-toast update failed", format!("{e:#}")));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    #[test]
    fn versions_compare() {
        let v = |s| Version::parse(s).unwrap();
        assert!(v("v0.5.0") > v("0.4.9"));
        assert!(v("0.10.0") > v("0.9.0"));
        assert!(v("1.0.0") > v("1.0.0-rc.2"));
        assert!(v("1.0.0-rc.2") > v("1.0.0-rc.1"));
        assert_eq!(v("v1.2.3"), v("1.2.3"));
        assert_eq!(v("1.2.3-rc.1").to_string(), "1.2.3-rc.1");
        // Dev builds: numeric parts compare as numbers, build metadata is ignored.
        assert!(v("0.6.1-dev.10+gabc") > v("0.6.1-dev.9+gdef"));
        assert!(v("0.6.1-dev.1") > v("0.6.0"));
        assert!(v("0.6.1") > v("0.6.1-dev.99"));
        assert!(v("1.0.0-rc.1.dev.3") > v("1.0.0-rc.1"));
        assert!(v("1.0.0-rc.2") > v("1.0.0-rc.1.dev.3"));
        assert_eq!(v("0.6.1-dev.3+gabc1234"), v("v0.6.1-dev.3"));
        assert_eq!(v("0.6.1-dev.3+gabc1234").to_string(), "0.6.1-dev.3+gabc1234");
        for bad in ["Release", "1.2", "1.2.3.4", "v1.x.3", "", "1.2.3-", "1.2.3+"] {
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn source_prefers_forgejo_with_token_else_github() {
        let mut u = UpdateConfig::default();
        let s = Source::from_config(&u);
        assert_eq!(s.latest_url, "https://api.github.com/repos/daterrell/ntfy-toast/releases/latest");

        u.token = Some("t".into());
        let s = Source::from_config(&u);
        assert_eq!(
            s.latest_url,
            "https://forgejo.terrell.ltd/api/v1/repos/dave/ntfy-toast/releases/latest"
        );
        assert_eq!(s.token.as_deref(), Some("t"));

        u.token = None;
        u.github_repo = String::new();
        assert!(Source::from_config(&u).latest_url.starts_with("https://forgejo.terrell.ltd/"));

        u.server = "https://github.com".into();
        u.repo = "x/y".into();
        assert_eq!(Source::from_config(&u).latest_url, "https://api.github.com/repos/x/y/releases/latest");
    }

    /// A tiny HTTP server playing a Forgejo instance that requires a token.
    fn fake_forgejo(exe: Vec<u8>, sha: String, version: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let release = serde_json::json!({
            "tag_name": format!("v{version}"),
            "html_url": format!("{base}/dave/ntfy-toast/releases/tag/v{version}"),
            "assets": [
                { "name": "ntfy-toast.exe", "browser_download_url": format!("{base}/dl/ntfy-toast.exe") },
                { "name": "ntfy-toast.exe.sha256", "browser_download_url": format!("{base}/dl/ntfy-toast.exe.sha256") },
            ],
        })
        .to_string();
        let assets = |v: &str| {
            serde_json::json!([
                { "name": "ntfy-toast.exe", "browser_download_url": format!("{base}/dl/ntfy-toast.exe") },
                { "name": "ntfy-toast.exe.sha256", "browser_download_url": format!("{base}/dl/{v}.sha256") },
            ])
        };
        let dev = format!("{version}-dev.10");
        let list = serde_json::json!([
            { "tag_name": "v0.0.1", "html_url": "x", "assets": assets("0.0.1") },
            { "tag_name": format!("v{version}-dev.12"), "html_url": "x", "assets": [] },
            { "tag_name": format!("v{version}-dev.11"), "html_url": "x", "draft": true, "assets": assets("d") },
            { "tag_name": format!("v{dev}"), "html_url": "x", "assets": assets(&dev) },
            { "tag_name": format!("v{version}-dev.9"), "html_url": "x", "assets": assets("9") },
            { "tag_name": "nightly", "html_url": "x", "assets": assets("n") },
        ])
        .to_string();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                reader.read_line(&mut first).unwrap();
                let mut authed = false;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line.trim().is_empty() {
                        break;
                    }
                    authed |= line.eq_ignore_ascii_case("authorization: token secret\r\n");
                }
                let path = first.split_whitespace().nth(1).unwrap_or("").to_string();
                let (status, ctype, body): (&str, &str, Vec<u8>) = if !authed {
                    ("403 Forbidden", "text/plain", b"no".to_vec())
                } else if path.ends_with("/releases?draft=false&limit=20") {
                    ("200 OK", "application/json", list.clone().into_bytes())
                } else if path.ends_with("/releases/latest") {
                    ("200 OK", "application/json", release.clone().into_bytes())
                } else if path == "/dl/ntfy-toast.exe" {
                    ("200 OK", "application/octet-stream", exe.clone())
                } else if path == "/dl/ntfy-toast.exe.sha256" {
                    ("200 OK", "text/plain", sha.clone().into_bytes())
                } else {
                    ("404 Not Found", "text/plain", b"?".to_vec())
                };
                let mut s = stream;
                let _ = write!(
                    s,
                    "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = s.write_all(&body);
            }
        });
        base
    }

    fn update_config(base: &str) -> UpdateConfig {
        UpdateConfig {
            server: base.into(),
            token: Some("secret".into()),
            ..UpdateConfig::default()
        }
    }

    #[test]
    fn finds_downloads_verifies_and_swaps() {
        let exe_bytes = b"MZ pretend this is ntfy-toast 99.0.0".to_vec();
        let sha = format!("{}  ntfy-toast.exe\n", sha256_hex(&exe_bytes));
        let base = fake_forgejo(exe_bytes.clone(), sha, "99.0.0");
        let http = crate::subscriber::http();

        let rel = check(&http, &update_config(&base)).unwrap().expect("newer release");
        assert_eq!(rel.version.to_string(), "99.0.0");

        let dir = std::env::temp_dir().join(format!("ntfy-toast-update-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("ntfy-toast.exe");
        std::fs::write(&exe, b"MZ old build").unwrap();

        let new = download(&http, &rel, &exe).unwrap();
        assert_eq!(std::fs::read(&new).unwrap(), exe_bytes);
        swap(&exe, &new).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), exe_bytes);
        assert_eq!(std::fs::read(dir.join("ntfy-toast.exe.old")).unwrap(), b"MZ old build");
        assert!(!new.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_bad_checksum_and_missing_token() {
        let exe_bytes = b"MZ tampered".to_vec();
        let base = fake_forgejo(exe_bytes, format!("{}  ntfy-toast.exe", "0".repeat(64)), "99.0.0");
        let http = crate::subscriber::http();

        let rel = latest(&http, &update_config(&base)).unwrap();
        let dir = std::env::temp_dir().join(format!("ntfy-toast-update-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let err = download(&http, &rel, &dir.join("ntfy-toast.exe")).unwrap_err();
        assert!(err.to_string().contains("checksum mismatch"), "{err:#}");
        assert!(!dir.join("ntfy-toast.exe.new").exists());
        std::fs::remove_dir_all(&dir).unwrap();

        let mut no_token = update_config(&base);
        no_token.token = Some("wrong".into());
        let err = latest(&http, &no_token).unwrap_err();
        assert!(format!("{err:#}").contains("403"), "{err:#}");
    }

    #[test]
    fn dev_channel_picks_the_newest_build() {
        let base = fake_forgejo(b"MZ".to_vec(), String::new(), "99.0.0");
        let http = crate::subscriber::http();
        let mut u = update_config(&base);
        assert_eq!(latest(&http, &u).unwrap().version.to_string(), "99.0.0");

        // dev.12 has no exe yet and dev.11 is a draft, so dev.10 wins (numerically over dev.9).
        u.channel = Channel::Dev;
        let rel = latest(&http, &u).unwrap();
        assert_eq!(rel.version.to_string(), "99.0.0-dev.10");
        assert!(rel.sha_url.ends_with("/dl/99.0.0-dev.10.sha256"));

        // Without a token the GitHub fallback is used, which only has formal releases.
        u.token = None;
        assert_eq!(Source::from_config(&u).list_url, None);
    }

    #[test]
    fn older_or_equal_release_is_not_an_update() {
        let base = fake_forgejo(b"MZ".to_vec(), String::new(), crate::VERSION);
        let http = crate::subscriber::http();
        assert!(check(&http, &update_config(&base)).unwrap().is_none());
    }
}
