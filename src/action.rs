//! ntfy `http` action buttons. A toast button can only launch a URI, so the request is
//! packed into an `ntfy-toast-action:` URI, which Windows hands to `ntfy-toast action <uri>`.

use crate::message;
use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

pub const SCHEME: &str = "ntfy-toast-action";

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct HttpAction {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: String,
    /// Remove the toast once the request succeeds.
    #[serde(default)]
    pub clear: bool,
    pub tag: String,
    pub group: String,
}

impl HttpAction {
    pub fn from_message(a: &message::Action, url: &str, tag: &str, group: &str) -> HttpAction {
        HttpAction {
            // ntfy's default for http actions.
            method: a.method.clone().unwrap_or_else(|| "POST".into()),
            url: url.into(),
            headers: a.headers.clone(),
            body: a.body.clone().unwrap_or_default(),
            clear: a.clear,
            tag: tag.into(),
            group: group.into(),
        }
    }

    pub fn to_uri(&self) -> String {
        let json = serde_json::to_vec(self).expect("serializable");
        format!("{SCHEME}:{}", URL_SAFE_NO_PAD.encode(json))
    }

    pub fn from_uri(uri: &str) -> Result<HttpAction> {
        let Some(data) = uri.strip_prefix(SCHEME).and_then(|r| r.strip_prefix(':')) else {
            bail!("not an {SCHEME}: URI");
        };
        // Some launchers append a slash to URIs.
        let json = URL_SAFE_NO_PAD
            .decode(data.trim_end_matches('/'))
            .context("bad action encoding")?;
        serde_json::from_slice(&json).context("bad action payload")
    }

    pub fn send(&self) -> Result<()> {
        let method = reqwest::Method::from_bytes(self.method.to_ascii_uppercase().as_bytes())
            .with_context(|| format!("bad HTTP method {:?}", self.method))?;
        let mut req = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()?
            .request(method, &self.url)
            .body(self.body.clone());
        for (k, v) in &self.headers {
            req = req.header(k, v);
        }
        req.send()
            .and_then(|r| r.error_for_status())
            .with_context(|| format!("{} {}", self.method, self.url))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> HttpAction {
        HttpAction {
            method: "POST".into(),
            url: "https://ntfy.example.com/reply".into(),
            headers: BTreeMap::from([("Authorization".into(), "Bearer x".into())]),
            body: "abc123 allow".into(),
            clear: true,
            tag: "abc123".into(),
            group: "Claude".into(),
        }
    }

    #[test]
    fn uri_round_trip() {
        let a = sample();
        let uri = a.to_uri();
        assert!(uri.starts_with("ntfy-toast-action:"));
        assert!(!uri.contains(['&', '"', ' ', '/', '+', '=']));
        assert_eq!(HttpAction::from_uri(&uri).unwrap(), a);
        assert_eq!(HttpAction::from_uri(&format!("{uri}/")).unwrap(), a);
    }

    #[test]
    fn rejects_other_uris() {
        assert!(HttpAction::from_uri("https://example.com").is_err());
        assert!(HttpAction::from_uri("ntfy-toast-action:%%%").is_err());
    }

    #[test]
    fn defaults_to_post() {
        let m: message::Action = serde_json::from_str(
            r#"{"action":"http","label":"Go","url":"https://x/y","body":"b"}"#,
        )
        .unwrap();
        let a = HttpAction::from_message(&m, "https://x/y", "t", "g");
        assert_eq!(a.method, "POST");
        assert!(!a.clear);
    }
}
