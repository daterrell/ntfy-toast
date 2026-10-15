use serde::Deserialize;
use std::collections::BTreeMap;

/// One line of ntfy's `/<topics>/json` stream.
/// See https://docs.ntfy.sh/subscribe/api/#json-message-format
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Message {
    pub id: String,
    /// Set when the publisher may later update, clear or delete this notification.
    pub sequence_id: Option<String>,
    #[serde(default)]
    pub time: i64,
    pub event: String,
    #[serde(default)]
    pub topic: String,
    pub title: Option<String>,
    pub message: Option<String>,
    pub priority: Option<u8>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub click: Option<String>,
    pub icon: Option<String>,
    #[serde(default)]
    pub actions: Vec<Action>,
    pub attachment: Option<Attachment>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Action {
    pub action: String,
    pub label: String,
    pub url: Option<String>,
    /// `http` actions only.
    pub method: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    pub body: Option<String>,
    #[serde(default)]
    pub clear: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Attachment {
    pub name: String,
    #[serde(rename = "type")]
    pub mime: Option<String>,
    pub size: Option<u64>,
    pub url: String,
}

impl Message {
    pub fn priority(&self) -> u8 {
        self.priority.unwrap_or(3).clamp(1, 5)
    }

    /// Identifies the toast; later updates, clears and deletes share the sequence id.
    pub fn toast_tag(&self) -> &str {
        self.sequence_id.as_deref().unwrap_or(&self.id)
    }
}

/// Tags that ntfy renders as emoji (subset of the shortcodes most used in practice).
/// Anything not listed here is shown as a plain-text tag.
pub fn tag_emoji(tag: &str) -> Option<&'static str> {
    Some(match tag {
        "+1" | "thumbsup" => "👍",
        "-1" | "thumbsdown" => "👎",
        "warning" => "⚠️",
        "rotating_light" => "🚨",
        "triangular_flag_on_post" => "🚩",
        "skull" => "💀",
        "x" => "❌",
        "no_entry" => "⛔",
        "no_entry_sign" => "🚫",
        "white_check_mark" => "✅",
        "heavy_check_mark" => "✔️",
        "ballot_box_with_check" => "☑️",
        "tada" => "🎉",
        "partying_face" => "🥳",
        "loudspeaker" => "📢",
        "mega" => "📣",
        "bell" => "🔔",
        "fire" => "🔥",
        "boom" => "💥",
        "zap" => "⚡",
        "bug" => "🐛",
        "computer" => "💻",
        "desktop_computer" => "🖥️",
        "floppy_disk" => "💾",
        "cd" => "💿",
        "package" => "📦",
        "lock" => "🔒",
        "unlock" => "🔓",
        "key" => "🔑",
        "email" | "e-mail" => "📧",
        "envelope" => "✉️",
        "calendar" => "📅",
        "clock" | "clock3" => "🕒",
        "hourglass" => "⌛",
        "alarm_clock" => "⏰",
        "chart_with_upwards_trend" => "📈",
        "chart_with_downwards_trend" => "📉",
        "moneybag" => "💰",
        "house" => "🏠",
        "door" => "🚪",
        "bulb" => "💡",
        "electric_plug" => "🔌",
        "battery" => "🔋",
        "sunny" => "☀️",
        "cloud" => "☁️",
        "umbrella" => "☔",
        "snowflake" => "❄️",
        "thermometer" => "🌡️",
        "robot" => "🤖",
        "rocket" => "🚀",
        "star" => "⭐",
        "heart" => "❤️",
        "eyes" => "👀",
        "wave" => "👋",
        "cat" => "🐱",
        "dog" => "🐶",
        "information_source" => "ℹ️",
        "question" => "❓",
        "exclamation" => "❗",
        "grey_exclamation" => "❕",
        "red_circle" => "🔴",
        "green_circle" => "🟢",
        "yellow_circle" => "🟡",
        "large_blue_circle" | "blue_circle" => "🔵",
        "cd_rom" => "💿",
        "hammer_and_wrench" => "🛠️",
        "wrench" => "🔧",
        "gear" => "⚙️",
        "arrows_counterclockwise" => "🔄",
        "inbox_tray" => "📥",
        "outbox_tray" => "📤",
        "phone" | "telephone_receiver" => "📞",
        "iphone" => "📱",
        "car" => "🚗",
        "truck" => "🚚",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_message() {
        let line = r#"{"id":"sPs71M8A2T","time":1643935928,"expires":1643936928,"event":"message","topic":"mytopic","priority":5,"tags":["warning","skull"],"click":"https://homecam.mynet.lan/incident/1234","title":"Unauthorized access detected","message":"Movement detected in the yard","actions":[{"id":"x","action":"view","label":"Open portal","url":"https://home.nest.com/","clear":true},{"action":"http","label":"Turn off","url":"https://api.example.com"}],"attachment":{"name":"cam.jpg","type":"image/jpeg","size":1234,"expires":1,"url":"https://ntfy.sh/file/abc.jpg"}}"#;
        let m: Message = serde_json::from_str(line).unwrap();
        assert_eq!(m.priority(), 5);
        assert_eq!(m.actions.len(), 2);
        assert!(m.actions[0].clear);
        assert_eq!(m.actions[1].method, None);
        assert_eq!(m.toast_tag(), "sPs71M8A2T");
        assert_eq!(m.attachment.unwrap().mime.as_deref(), Some("image/jpeg"));
    }

    #[test]
    fn parses_http_action_and_sequence_id() {
        let line = r#"{"id":"a","time":1,"event":"message","topic":"t","sequence_id":"req1","actions":[{"action":"http","label":"Allow","url":"https://x/reply","method":"PUT","headers":{"X-A":"1"},"body":"req1 allow","clear":true}]}"#;
        let m: Message = serde_json::from_str(line).unwrap();
        assert_eq!(m.toast_tag(), "req1");
        let a = &m.actions[0];
        assert_eq!(a.method.as_deref(), Some("PUT"));
        assert_eq!(a.headers["X-A"], "1");
        assert_eq!(a.body.as_deref(), Some("req1 allow"));
        assert!(a.clear);
    }

    #[test]
    fn parses_keepalive() {
        let m: Message =
            serde_json::from_str(r#"{"id":"a","time":1,"event":"keepalive","topic":"t"}"#)
                .unwrap();
        assert_eq!(m.event, "keepalive");
        assert_eq!(m.priority(), 3);
    }
}
