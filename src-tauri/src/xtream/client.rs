//! HTTP access to an Xtream server.
//!
//! Two things are load-bearing and easy to get wrong: some servers reject
//! unknown user agents, and timeshift URLs answer 302, so redirects must be
//! followed rather than treated as failures.

use anyhow::{Context, Result};
use std::fmt;

use crate::xtream::models::Rows;
use crate::xtream::{
    Category, Channel, SeriesInfo, SeriesStream, ServerInfo, UserInfo, VodInfo, VodStream,
};

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

/// `player_api.php`'s answer with no action: the account, and the server.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Account {
    pub user_info: UserInfo,
    /// Missing or malformed on some servers; it only matters for its clock.
    #[serde(default, deserialize_with = "lenient_server_info")]
    pub server_info: ServerInfo,
}

/// A `server_info` that is not an object (some servers send `[]`) is an
/// empty one, not a failed login.
fn lenient_server_info<'de, D: serde::Deserializer<'de>>(d: D) -> Result<ServerInfo, D::Error> {
    use serde::Deserialize;
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(v).unwrap_or_default())
}

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

    /// Where a movie plays. Carries the credentials like [`live_url`]
    /// does, so it is never logged.
    ///
    /// [`live_url`]: XtreamClient::live_url
    pub fn movie_url(&self, stream_id: i64, ext: &str) -> String {
        format!(
            "{}/movie/{}/{}/{}.{}",
            self.creds.host, self.creds.username, self.creds.password, stream_id, ext
        )
    }

    /// Where an episode plays. Carries the credentials like [`live_url`]
    /// does, so it is never logged.
    ///
    /// [`live_url`]: XtreamClient::live_url
    pub fn series_url(&self, episode_id: i64, ext: &str) -> String {
        format!(
            "{}/series/{}/{}/{}.{}",
            self.creds.host, self.creds.username, self.creds.password, episode_id, ext
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
        Ok(self.account().await?.user_info)
    }

    /// The account and the server's clock, in one call: what `login` asks.
    pub async fn account(&self) -> Result<Account> {
        let e: Account = self
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
        Ok(e)
    }

    /// The account these calls are made as. For keying what is learnt about
    /// the server (`catalog::origin`), never for logging.
    pub fn credentials(&self) -> &Credentials {
        &self.creds
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

    /// The movie categories; a row that does not read is left out.
    pub async fn vod_categories(&self) -> Result<Vec<Category>> {
        let rows: Rows<Category> = self
            .http
            .get(self.api_url(Some("get_vod_categories")))
            .timeout(API_TOTAL)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .context("parsing movie categories")?;
        Ok(readable(rows, "movie categories"))
    }

    /// Every movie, in one call: the list is tens of megabytes on a big
    /// catalogue, hence the long deadline. Details come per title through
    /// [`vod_info`](Self::vod_info), never in bulk.
    ///
    /// A row that does not read (no id, no name, or not a movie at all) is
    /// left out, never failing the rest.
    pub async fn vod_streams(&self) -> Result<Vec<VodStream>> {
        let rows: Rows<VodStream> = self
            .http
            .get(self.api_url(Some("get_vod_streams")))
            .timeout(API_TOTAL)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .context("parsing movies")?;
        Ok(readable(rows, "movies"))
    }

    pub async fn vod_info(&self, vod_id: i64) -> Result<VodInfo> {
        self.http
            .get(format!(
                "{}&vod_id={vod_id}",
                self.api_url(Some("get_vod_info"))
            ))
            // One title's details, asked while someone waits.
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .context("parsing movie details")
    }

    /// The show categories; a row that does not read is left out.
    pub async fn series_categories(&self) -> Result<Vec<Category>> {
        let rows: Rows<Category> = self
            .http
            .get(self.api_url(Some("get_series_categories")))
            .timeout(API_TOTAL)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .context("parsing show categories")?;
        Ok(readable(rows, "show categories"))
    }

    /// Every show, in one call (no episodes: those come per show through
    /// [`series_info`](Self::series_info)). A row that does not read is
    /// left out, never failing the rest.
    pub async fn series(&self) -> Result<Vec<SeriesStream>> {
        let rows: Rows<SeriesStream> = self
            .http
            .get(self.api_url(Some("get_series")))
            .timeout(API_TOTAL)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .context("parsing shows")?;
        Ok(readable(rows, "shows"))
    }

    pub async fn series_info(&self, series_id: i64) -> Result<SeriesInfo> {
        let info: SeriesInfo = self
            .http
            .get(format!(
                "{}&series_id={series_id}",
                self.api_url(Some("get_series_info"))
            ))
            // One show's seasons and episodes, asked while someone waits.
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .context("parsing show details")?;
        if info.skipped > 0 {
            tracing::warn!(
                "{} of a show's episodes could not be read and were left out",
                info.skipped
            );
        }
        Ok(info)
    }
}

/// The rows of a list that read, those that did not counted in the log
/// (as a number: a row may carry anything).
fn readable<T>(rows: Rows<T>, what: &str) -> Vec<T> {
    if rows.skipped > 0 {
        tracing::warn!(
            "{} of the provider's {what} could not be read and were left out",
            rows.skipped
        );
    }
    rows.read
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

    #[test]
    fn the_account_answer_carries_the_server_clock_when_it_has_one() {
        let a: Account = serde_json::from_str(
            r#"{"user_info":{"auth":1,"max_connections":"1"},
                "server_info":{"timezone":"Europe/Somewhere","time_now":"2026-11-07 21:17:17",
                               "timestamp_now":1794082637}}"#,
        )
        .unwrap();
        assert_eq!(a.user_info.max_connections, Some(1));
        assert_eq!(a.server_info.utc_offset(), Some(3600));
        // Missing, or not an object: still a login, with no clock.
        for body in [r#"{"user_info":{"auth":1}}"#, r#"{"user_info":{"auth":1},"server_info":[]}"#,
                     r#"{"user_info":{"auth":1},"server_info":null}"#] {
            let a: Account = serde_json::from_str(body).unwrap();
            assert!(a.user_info.auth, "{body}");
            assert_eq!(a.server_info.utc_offset(), None, "{body}");
        }
    }

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

    #[test]
    fn movie_url_names_the_container() {
        let c = XtreamClient::new(creds()).unwrap();
        assert_eq!(
            c.movie_url(77, "mkv"),
            "http://example.com/movie/u/p/77.mkv"
        );
    }

    #[test]
    fn series_url_names_the_container() {
        let c = XtreamClient::new(creds()).unwrap();
        assert_eq!(
            c.series_url(77, "mkv"),
            "http://example.com/series/u/p/77.mkv"
        );
    }

    #[tokio::test]
    async fn series_calls_ask_the_right_actions() {
        let s = crate::test_http::serve(|_, _| crate::test_http::Reply::ok("[]"));
        let mut cr = creds();
        cr.host = s.base.clone();
        let c = XtreamClient::new(cr).unwrap();
        assert!(c.series_categories().await.unwrap().is_empty());
        assert!(c.series().await.unwrap().is_empty());
        // `[]` is a server's "nothing" for a show: no episodes, no details.
        assert!(c.series_info(5).await.unwrap().episodes.is_empty());
        let seen = s.requests();
        let actions: Vec<_> = seen.iter().map(|r| r.query("action").unwrap()).collect();
        assert_eq!(
            actions,
            ["get_series_categories", "get_series", "get_series_info"]
        );
        assert_eq!(seen[2].query("series_id"), Some("5"));
    }

    #[tokio::test]
    async fn vod_calls_ask_the_right_actions() {
        let s = crate::test_http::serve(|_, _| crate::test_http::Reply::ok("[]"));
        let mut cr = creds();
        cr.host = s.base.clone();
        let c = XtreamClient::new(cr).unwrap();
        assert!(c.vod_categories().await.unwrap().is_empty());
        assert!(c.vod_streams().await.unwrap().is_empty());
        // `[]` is a server's "nothing" for a title.
        assert!(c.vod_info(5).await.unwrap().info.plot.is_none());
        let seen = s.requests();
        let actions: Vec<_> = seen.iter().map(|r| r.query("action").unwrap()).collect();
        assert_eq!(
            actions,
            ["get_vod_categories", "get_vod_streams", "get_vod_info"]
        );
        assert_eq!(seen[2].query("vod_id"), Some("5"));
    }

    #[tokio::test]
    async fn one_malformed_movie_does_not_fail_the_list() {
        let s = crate::test_http::serve(|_, _| {
            crate::test_http::Reply::ok(
                r#"[{"stream_id":1,"name":"One"},{"stream_id":"","name":null},{"stream_id":2,"name":"Two"}]"#,
            )
        });
        let mut cr = creds();
        cr.host = s.base.clone();
        let c = XtreamClient::new(cr).unwrap();
        let ids: Vec<_> = c.vod_streams().await.unwrap().iter().map(|v| v.stream_id).collect();
        assert_eq!(ids, [1, 2]);
    }
}
