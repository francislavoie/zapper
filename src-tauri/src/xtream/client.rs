//! HTTP access to an Xtream server.
//!
//! Two things are load-bearing and easy to get wrong: some servers reject
//! unknown user agents, and timeshift URLs answer 302, so redirects must be
//! followed rather than treated as failures.

use anyhow::{Context, Result};
use std::fmt;

use crate::xtream::{Category, Channel, UserInfo};

/// Some servers reject default agents; VLC's is widely accepted.
const USER_AGENT: &str = "VLC/3.0.20 LibVLC/3.0.20";

#[derive(Clone)]
pub struct Credentials {
    pub host: String,
    pub username: String,
    pub password: String,
}

impl fmt::Debug for Credentials {
    /// Credentials appear in every stream URL; never let them reach a log.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("host", &self.host)
            .field("username", &"<redacted>")
            .field("password", &"<redacted>")
            .finish()
    }
}

/// No byte from the server for this long fails a request. Longer than
/// `m3u::fetch::FIRST_ANSWER`, so the guide download says why in its own
/// words first.
const API_STALL: std::time::Duration = std::time::Duration::from_secs(150);
/// The most a catalogue call may take in all: a list of 100,000 channels is
/// tens of megabytes, a few minutes on a slow line.
const API_TOTAL: std::time::Duration = std::time::Duration::from_secs(10 * 60);

pub struct XtreamClient {
    creds: Credentials,
    http: reqwest::Client,
}

impl XtreamClient {
    pub fn new(mut creds: Credentials) -> Result<Self> {
        creds.host = creds.host.trim_end_matches('/').to_owned();
        // A server that stops answering must not hold a sync pass forever.
        // The guide's download has its own, clearer deadlines
        // (`m3u::fetch::STALL`); this one, a little longer, is for the
        // catalogue's API calls, whose bodies are read whole by `json()`.
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::limited(5))
            .connect_timeout(std::time::Duration::from_secs(15))
            .read_timeout(API_STALL)
            .build()
            .context("building http client")?;
        Ok(Self { creds, http })
    }

    fn api_url(&self, action: Option<&str>) -> String {
        let mut u = format!(
            "{}/player_api.php?username={}&password={}",
            self.creds.host,
            urlencoding(&self.creds.username),
            urlencoding(&self.creds.password),
        );
        if let Some(a) = action {
            u.push_str("&action=");
            u.push_str(a);
        }
        u
    }

    pub fn live_url(&self, stream_id: i64) -> String {
        format!(
            "{}/live/{}/{}/{}.ts",
            self.creds.host, self.creds.username, self.creds.password, stream_id
        )
    }

    /// `start` is `YYYY-MM-DD:HH-MM`; `duration` is minutes.
    pub fn timeshift_url(&self, stream_id: i64, start: &str, duration: u32) -> String {
        format!(
            "{}/streaming/timeshift.php?username={}&password={}&stream={}&start={}&duration={}",
            self.creds.host,
            urlencoding(&self.creds.username),
            urlencoding(&self.creds.password),
            stream_id,
            urlencoding(start),
            duration,
        )
    }

    pub fn xmltv_url(&self) -> String {
        format!(
            "{}/xmltv.php?username={}&password={}",
            self.creds.host,
            urlencoding(&self.creds.username),
            urlencoding(&self.creds.password),
        )
    }

    /// Opens the XMLTV guide as a stream.
    ///
    /// Returns the response rather than the bytes: a guide can run to tens
    /// of megabytes and must be parsed incrementally, never buffered whole.
    /// Lives here so callers do not rebuild a client and re-guess the user
    /// agent some servers require.
    pub async fn xmltv_stream(&self) -> Result<reqwest::Response> {
        self.http
            .get(self.xmltv_url())
            .send()
            .await
            .context("fetching the guide")?
            .error_for_status()
            .context("the server refused the guide")
    }

    pub async fn login(&self) -> Result<UserInfo> {
        #[derive(serde::Deserialize)]
        struct Envelope {
            user_info: UserInfo,
        }
        let e: Envelope = self
            .http
            .get(self.api_url(None))
            // Asked before reconnecting and from Settings: a server that
            // does not answer promptly is as good as one that cannot.
            .timeout(std::time::Duration::from_secs(8))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .context("parsing login")?;
        Ok(e.user_info)
    }

    pub async fn categories(&self) -> Result<Vec<Category>> {
        self.http
            .get(self.api_url(Some("get_live_categories")))
            .timeout(API_TOTAL)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .context("parsing categories")
    }

    pub async fn channels(&self) -> Result<Vec<Channel>> {
        self.http
            .get(self.api_url(Some("get_live_streams")))
            .timeout(API_TOTAL)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .context("parsing channels")
    }
}

/// Minimal percent-encoding for query values; avoids a dependency.
fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creds() -> Credentials {
        Credentials {
            host: "http://example.com".into(),
            username: "u".into(),
            password: "p".into(),
        }
    }

    #[test]
    fn live_url_is_ts() {
        let c = XtreamClient::new(creds()).unwrap();
        assert_eq!(c.live_url(498338), "http://example.com/live/u/p/498338.ts");
    }

    #[test]
    fn trailing_slash_on_host_is_normalised() {
        let mut cr = creds();
        cr.host = "http://example.com/".into();
        let c = XtreamClient::new(cr).unwrap();
        assert_eq!(c.live_url(1), "http://example.com/live/u/p/1.ts");
    }

    #[test]
    fn timeshift_uses_query_form_and_escapes() {
        // Servers accept both URL shapes, but the query form avoids
        // path-escaping credentials.
        let c = XtreamClient::new(creds()).unwrap();
        let u = c.timeshift_url(42, "2026-09-08:01-30", 5);
        assert!(u.starts_with("http://example.com/streaming/timeshift.php?"));
        assert!(u.contains("stream=42"));
        assert!(u.contains("duration=5"));
        assert!(u.contains("start=2026-09-08%3A01-30"));
    }

    #[test]
    fn credentials_are_redacted_in_debug() {
        let c = creds();
        let s = format!("{c:?}");
        // A bare `contains('p')` cannot work: the field names carry a `p`.
        assert!(
            !s.contains("password: \"p\""),
            "password leaked into Debug output: {s}"
        );
        assert!(s.contains("<redacted>"), "redaction marker missing: {s}");
    }
}
