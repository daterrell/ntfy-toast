use crate::action::HttpAction;
use crate::config::Config;
use crate::message::{tag_emoji, Message};
use crate::paths;
use crate::subscriber::{error_chain, Http, Notice};
use log::{debug, info, warn};
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, RwLock};

/// Windows refuses toast images larger than this.
const MAX_IMAGE_BYTES: u64 = 3 * 1024 * 1024;
const MAX_BUTTONS: usize = 5;

/// Everything that goes into one toast, after images have been fetched to local files.
pub struct Toast {
    pub xml: String,
    pub fallback_xml: String,
    pub tag: String,
    pub group: String,
    /// Priority 1 ("min") goes straight to the notification center without a popup.
    pub silent_popup: bool,
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // Control characters other than tab/newline are invalid in XML 1.0.
            c if (c as u32) < 0x20 && c != '\n' && c != '\t' && c != '\r' => {}
            c => out.push(c),
        }
    }
    out
}

fn file_uri(p: &Path) -> String {
    format!("file:///{}", p.display().to_string().replace('\\', "/"))
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Turn an ntfy message into toast XML. `icon` / `image` are local files already downloaded.
pub fn build(n: &Notice, cfg: &Config, icon: Option<&Path>, image: Option<&Path>) -> Toast {
    let m = &n.msg;
    let p = m.priority();

    let emoji: String = m.tags.iter().filter_map(|t| tag_emoji(t)).collect();
    let plain_tags: Vec<&str> = m
        .tags
        .iter()
        .filter(|t| tag_emoji(t).is_none())
        .map(String::as_str)
        .collect();

    // A configured topic name stands in for random-looking topic ids.
    let topic = n.topic_name.as_deref().unwrap_or(&m.topic);
    let title_text = m.title.clone().unwrap_or_else(|| topic.to_string());
    let title = if emoji.is_empty() {
        title_text
    } else {
        format!("{emoji} {title_text}")
    };
    let mut body = m.message.clone().unwrap_or_default();
    if body.is_empty() {
        if let Some(a) = &m.attachment {
            body = format!("Attachment: {}", a.name);
        }
    }
    if !plain_tags.is_empty() {
        if !body.is_empty() {
            body.push('\n');
        }
        let _ = write!(body, "Tags: {}", plain_tags.join(", "));
    }

    let host = n
        .server
        .split("://")
        .nth(1)
        .unwrap_or(&n.server)
        .to_string();
    let attribution = match &n.source_name {
        Some(name) => format!("{name} · {topic}"),
        None => format!("{topic} · {host}"),
    };

    let click = m.click.clone().or_else(|| {
        cfg.click_opens_topic
            .then(|| format!("{}/{}", n.server, m.topic))
    });

    let tag = truncate(m.toast_tag(), 64);
    let group = truncate(&m.topic, 64);

    let mut buttons: Vec<(String, String)> = Vec::new();
    for a in &m.actions {
        match (a.action.as_str(), &a.url) {
            ("view", Some(url)) => buttons.push((a.label.clone(), url.clone())),
            ("http", Some(url)) => {
                let uri = HttpAction::from_message(a, url, &tag, &group).to_uri();
                buttons.push((a.label.clone(), uri));
            }
            (kind, _) => debug!("action {:?} ({kind}) not supported on Windows", a.label),
        }
    }
    if let Some(a) = &m.attachment {
        if image.is_none() {
            buttons.push((format!("Open {}", truncate(&a.name, 30)), a.url.clone()));
        }
    }
    buttons.truncate(MAX_BUTTONS);

    let mut toast_attrs = String::new();
    if let Some(url) = &click {
        let _ = write!(
            toast_attrs,
            r#" activationType="protocol" launch="{}""#,
            esc(url)
        );
    }
    if p >= 4 {
        toast_attrs.push_str(r#" duration="long""#);
    }
    if p == 5 {
        toast_attrs.push_str(r#" scenario="urgent""#);
    }

    let audio = match p {
        1 | 2 => r#"<audio silent="true"/>"#,
        3 => r#"<audio src="ms-winsoundevent:Notification.Default"/>"#,
        _ => r#"<audio src="ms-winsoundevent:Notification.Reminder"/>"#,
    };

    let mut visual = String::new();
    let _ = write!(visual, "<text>{}</text>", esc(&title));
    if !body.is_empty() {
        let _ = write!(visual, "<text>{}</text>", esc(&body));
    }
    let _ = write!(
        visual,
        r#"<text placement="attribution">{}</text>"#,
        esc(&attribution)
    );
    let mut images = String::new();
    if let Some(i) = icon {
        let _ = write!(
            images,
            r#"<image placement="appLogoOverride" hint-crop="circle" src="{}"/>"#,
            esc(&file_uri(i))
        );
    }
    if let Some(i) = image {
        let _ = write!(images, r#"<image src="{}"/>"#, esc(&file_uri(i)));
    }

    let mut actions = String::new();
    if !buttons.is_empty() {
        actions.push_str("<actions>");
        for (label, url) in &buttons {
            let _ = write!(
                actions,
                r#"<action content="{}" activationType="protocol" arguments="{}"/>"#,
                esc(&truncate(label, 40)),
                esc(url)
            );
        }
        actions.push_str("</actions>");
    }

    let xml = format!(
        r#"<toast{toast_attrs}><visual><binding template="ToastGeneric">{visual}{images}</binding></visual>{actions}{audio}</toast>"#
    );
    // Used if Windows rejects the full payload (e.g. an image it can't decode).
    let fallback_xml = format!(
        r#"<toast><visual><binding template="ToastGeneric"><text>{}</text><text>{}</text></binding></visual></toast>"#,
        esc(&title),
        esc(&body)
    );

    Toast {
        xml,
        fallback_xml,
        tag,
        group,
        silent_popup: p == 1,
    }
}

/// Download an image to the local cache (toasts from unpackaged apps can't use http images).
fn fetch_image(http: &Http, url: &str, n: &Notice) -> Option<PathBuf> {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    let lower = url.to_ascii_lowercase();
    let ext = [".png", ".jpg", ".jpeg", ".gif"]
        .into_iter()
        .find(|e| lower.split('?').next().unwrap_or("").ends_with(e))
        .unwrap_or(".img");
    let path = paths::cache_dir().join(format!("{:016x}{ext}", h.finish()));
    if path.exists() {
        return Some(path);
    }

    let mut req = http.get(url);
    if let Some(a) = &n.auth {
        if url.starts_with(&n.server) {
            req = req.header("Authorization", a);
        }
    }
    let resp = match req.send().and_then(|r| r.error_for_status()) {
        Ok(r) => r,
        Err(e) => {
            warn!("could not fetch image {url}: {}", error_chain(&e));
            return None;
        }
    };
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    if !content_type.starts_with("image/") {
        warn!("not an image ({content_type}): {url}");
        return None;
    }
    let mut buf = Vec::new();
    if let Err(e) = resp.take(MAX_IMAGE_BYTES + 1).read_to_end(&mut buf) {
        warn!("could not fetch image {url}: {e}");
        return None;
    }
    if buf.len() as u64 > MAX_IMAGE_BYTES {
        info!("image too large for a toast, skipping: {url}");
        return None;
    }
    let _ = std::fs::create_dir_all(paths::cache_dir());
    std::fs::write(&path, buf).ok()?;
    Some(path)
}

fn prepare(http: &Http, n: &Notice, cfg: &Config) -> Toast {
    let m: &Message = &n.msg;
    let icon = m.icon.as_deref().and_then(|u| fetch_image(http, u, n));
    let image = m.attachment.as_ref().and_then(|a| {
        let is_image = a
            .mime
            .as_deref()
            .is_some_and(|t| t.starts_with("image/") && t != "image/svg+xml");
        let small = a.size.is_none_or(|s| s <= MAX_IMAGE_BYTES);
        (is_image && small)
            .then(|| fetch_image(http, &a.url, n))
            .flatten()
    });
    build(n, cfg, icon.as_deref(), image.as_deref())
}

/// Delete cached images older than a day.
pub fn prune_cache() {
    let Ok(entries) = std::fs::read_dir(paths::cache_dir()) else {
        return;
    };
    let day = std::time::Duration::from_secs(24 * 3600);
    for e in entries.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > day);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// The live config, swapped on reload; readers clone the inner `Arc` and drop the lock.
pub type SharedConfig = Arc<RwLock<Arc<Config>>>;

/// Toast thread: owns the WinRT notifier and displays notices as they arrive.
pub fn run(rx: Receiver<Notice>, shared: SharedConfig) {
    let http = crate::subscriber::http();
    let mut notifier: Option<(String, platform::Notifier)> = None;
    for n in rx {
        let cfg = shared.read().unwrap().clone();
        if notifier.as_ref().is_none_or(|(id, _)| *id != cfg.app_id) {
            match platform::Notifier::new(&cfg.app_id) {
                Ok(x) => notifier = Some((cfg.app_id.clone(), x)),
                Err(e) => {
                    log::error!("cannot create toast notifier: {e:#}");
                    continue;
                }
            }
        }
        let Some((_, notifier)) = &notifier else {
            continue;
        };
        if matches!(n.msg.event.as_str(), "message_clear" | "message_delete") {
            let (tag, group) = (truncate(n.msg.toast_tag(), 64), truncate(&n.msg.topic, 64));
            if let Err(e) = platform::remove(&cfg.app_id, &tag, &group) {
                debug!("could not remove toast {group}/{tag}: {e:#}");
            }
            continue;
        }
        let toast = prepare(&http, &n, &cfg);
        if let Err(e) = notifier.show(&toast, &toast.xml) {
            warn!("toast rejected ({e:#}); retrying without extras");
            if let Err(e) = notifier.show(&toast, &toast.fallback_xml) {
                log::error!("could not show toast: {e:#}");
            }
        }
    }
}

#[cfg(windows)]
pub mod platform {
    use super::Toast;
    use anyhow::{Context, Result};
    use windows::core::HSTRING;
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager, ToastNotifier};
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

    pub struct Notifier(ToastNotifier);

    impl Notifier {
        pub fn new(app_id: &str) -> Result<Notifier> {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            let n = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(app_id))
                .context("CreateToastNotifierWithId")?;
            Ok(Notifier(n))
        }

        pub fn show(&self, t: &Toast, xml: &str) -> Result<()> {
            let doc = XmlDocument::new()?;
            doc.LoadXml(&HSTRING::from(xml)).context("invalid toast XML")?;
            let toast = ToastNotification::CreateToastNotification(&doc)?;
            toast.SetTag(&HSTRING::from(t.tag.as_str()))?;
            toast.SetGroup(&HSTRING::from(t.group.as_str()))?;
            if t.silent_popup {
                toast.SetSuppressPopup(true)?;
            }
            self.0.Show(&toast).context("ToastNotifier::Show")?;
            Ok(())
        }
    }

    /// Remove a toast from the screen and the notification center.
    pub fn remove(app_id: &str, tag: &str, group: &str) -> Result<()> {
        ToastNotificationManager::History()?
            .RemoveGroupedTagWithId(
                &HSTRING::from(tag),
                &HSTRING::from(group),
                &HSTRING::from(app_id),
            )
            .context("ToastNotificationHistory::Remove")?;
        Ok(())
    }
}

/// Non-Windows builds print toasts instead, so the daemon can be developed and tested anywhere.
#[cfg(not(windows))]
pub mod platform {
    use super::Toast;
    use anyhow::Result;

    pub struct Notifier;

    impl Notifier {
        pub fn new(_app_id: &str) -> Result<Notifier> {
            Ok(Notifier)
        }

        pub fn show(&self, t: &Toast, xml: &str) -> Result<()> {
            println!("TOAST [{}/{} quiet={}] {xml}", t.group, t.tag, t.silent_popup);
            Ok(())
        }
    }

    pub fn remove(_app_id: &str, tag: &str, group: &str) -> Result<()> {
        println!("REMOVE [{group}/{tag}]");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{Action, Attachment};

    fn cfg() -> Config {
        Config::parse("[[subscription]]\ntopic = \"t\"").unwrap()
    }

    fn notice(msg: Message) -> Notice {
        Notice {
            msg,
            server: "https://ntfy.sh".into(),
            auth: None,
            source_name: None,
            topic_name: None,
        }
    }

    #[test]
    fn minimal_message_opens_topic() {
        let t = build(
            &notice(Message {
                id: "abc".into(),
                event: "message".into(),
                topic: "alerts".into(),
                message: Some("hi <there> & you".into()),
                ..Default::default()
            }),
            &cfg(),
            None,
            None,
        );
        assert!(t.xml.contains(r#"launch="https://ntfy.sh/alerts""#));
        assert!(t.xml.contains("<text>alerts</text>"));
        assert!(t.xml.contains("hi &lt;there&gt; &amp; you"));
        assert!(t.xml.contains("Notification.Default"));
        assert!(!t.silent_popup);
    }

    #[test]
    fn rich_message() {
        let t = build(
            &notice(Message {
                id: "abc".into(),
                event: "message".into(),
                topic: "cam".into(),
                title: Some("Motion".into()),
                priority: Some(5),
                tags: vec!["warning".into(), "backyard".into()],
                click: Some("https://cam/1?a=1&b=2".into()),
                actions: vec![
                    Action {
                        action: "view".into(),
                        label: "Open".into(),
                        url: Some("https://cam".into()),
                        ..Default::default()
                    },
                    Action {
                        action: "http".into(),
                        label: "Disarm".into(),
                        url: Some("https://api".into()),
                        body: Some("off".into()),
                        clear: true,
                        ..Default::default()
                    },
                ],
                attachment: Some(Attachment {
                    name: "log.txt".into(),
                    mime: Some("text/plain".into()),
                    size: Some(10),
                    url: "https://ntfy.sh/file/x.txt".into(),
                }),
                ..Default::default()
            }),
            &cfg(),
            None,
            None,
        );
        assert!(t.xml.contains("<text>⚠️ Motion</text>"));
        assert!(t.xml.contains("Tags: backyard"));
        assert!(t.xml.contains(r#"launch="https://cam/1?a=1&amp;b=2""#));
        assert!(t.xml.contains(r#"scenario="urgent""#));
        assert!(t.xml.contains(r#"content="Open""#));
        assert!(t.xml.contains(r#"content="Disarm" activationType="protocol" arguments="ntfy-toast-action:"#));
        assert!(t.xml.contains(r#"content="Open log.txt""#));
    }

    #[test]
    fn http_button_targets_sequence_tag() {
        let t = build(
            &notice(Message {
                id: "m1".into(),
                sequence_id: Some("req1".into()),
                event: "message".into(),
                topic: "Claude".into(),
                actions: vec![Action {
                    action: "http".into(),
                    label: "Allow".into(),
                    url: Some("https://ntfy/reply".into()),
                    body: Some("req1 allow".into()),
                    clear: true,
                    ..Default::default()
                }],
                ..Default::default()
            }),
            &cfg(),
            None,
            None,
        );
        assert_eq!(t.tag, "req1");
        let uri = t.xml.split(r#"arguments=""#).nth(1).unwrap().split('"').next().unwrap();
        let a = HttpAction::from_uri(uri).unwrap();
        assert_eq!((a.method.as_str(), a.url.as_str()), ("POST", "https://ntfy/reply"));
        assert_eq!((a.body.as_str(), a.tag.as_str(), a.group.as_str()), ("req1 allow", "req1", "Claude"));
        assert!(a.clear);
    }

    #[test]
    fn topic_name_replaces_id_in_display() {
        let mut n = notice(Message {
            id: "abc".into(),
            event: "message".into(),
            topic: "CgbRePrzudWiLmVl".into(),
            message: Some("person detected".into()),
            ..Default::default()
        });
        n.topic_name = Some("Frigate".into());
        let t = build(&n, &cfg(), None, None);
        assert!(t.xml.contains("<text>Frigate</text>"), "{}", t.xml);
        assert!(t.xml.contains("Frigate · ntfy.sh"));
        // The click URL and grouping still use the real topic id.
        assert!(t.xml.contains(r#"launch="https://ntfy.sh/CgbRePrzudWiLmVl""#));
        assert_eq!(t.group, "CgbRePrzudWiLmVl");
    }

    #[test]
    fn min_priority_is_quiet() {
        let t = build(
            &notice(Message {
                id: "abc".into(),
                event: "message".into(),
                topic: "t".into(),
                priority: Some(1),
                ..Default::default()
            }),
            &cfg(),
            Some(Path::new(r"C:\Users\x\icon.png")),
            None,
        );
        assert!(t.silent_popup);
        assert!(t.xml.contains(r#"silent="true""#));
        assert!(t.xml.contains(r#"src="file:///C:/Users/x/icon.png""#));
    }
}
