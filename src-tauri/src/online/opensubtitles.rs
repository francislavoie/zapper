//! OpenSubtitles (opensubtitles.com's REST API, v1).
//!
//! - `GET /subtitles?…`: the search, with the app's key and no sign-in. It
//!   is sent ids (a TMDB id, or a name and year), season and episode, and
//!   languages. The query is built sorted by name with lower-case values
//!   ([`search_query`]): OpenSubtitles answers any other with a redirect to
//!   that one, which costs a round trip and a request against the limit.
//! - `POST /login`: the user's own username and password, for a token and
//!   the API base to use it on (VIP accounts have their own host).
//! - `POST /download` `{"file_id", "sub_format": "srt"}`, with the token: a
//!   temporary link to the file, and the downloads left. Never without a
//!   token: the app key's anonymous downloads are off, and would be shared
//!   by every zapper user anyway.
//! - The link itself, fetched with neither the key nor the token.
//! - `DELETE /logout`, with the token, on signing out.
//!
//! The token goes only to an OpenSubtitles host ([`token_host_allowed`]),
//! and the file only comes from one ([`file_link_allowed`]); in tests and
//! end-to-end runs, the stand-in at [`API_OVERRIDE`] counts as one.

use std::time::Duration;

use reqwest::header::ACCEPT;
use reqwest::Url;
use serde::Deserialize;

use super::http::{self, OnlineError, Pacer};
use super::srt;

/// zapper's OpenSubtitles API key. It names the app, not a user, as
/// Jellyfin's does, and is public the same way. Anonymous downloads are off
/// for it: every download is made with the user's own sign-in.
pub const API_KEY: &str = "mVss5g5eeciNKar76SpCH6gem699VER4";

pub const DEFAULT_API: &str = "https://api.opensubtitles.com/api/v1";

/// Points zapper at a stand-in OpenSubtitles (`tools/fake_opensubtitles.py`):
/// its API base, `http://127.0.0.1:<port>/api/v1`.
pub const API_OVERRIDE: &str = "ZAPPER_OPENSUBTITLES_URL";

/// Four requests a second and 30 in ten seconds, under what OpenSubtitles
/// allows an address. Used only by [`Client::new`]; tests make their own.
pub static PACER: Pacer = Pacer::new(4, 30, Duration::from_secs(10));

/// The API base: [`API_OVERRIDE`]'s, else [`DEFAULT_API`].
pub fn api_base() -> String {
    std::env::var(API_OVERRIDE)
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_API.to_owned())
}

/// What a title is looked up by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    /// The film's TMDB id, or the show's for an episode.
    Tmdb(i64),
    Name {
        query: String,
        year: Option<i32>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchParams {
    pub lookup: Lookup,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    /// Language codes (`en`, `pt-BR`); none means every language.
    pub languages: Vec<String>,
}

/// The search's query string: parameters sorted by name, values lower-case
/// and percent-encoded, and only those that say something (no defaults).
pub fn search_query(p: &SearchParams) -> String {
    let episodic = p.season.is_some() || p.episode.is_some();
    let mut pairs: Vec<(&str, String)> = Vec::new();
    match &p.lookup {
        Lookup::Tmdb(id) => {
            let key = if episodic {
                "parent_tmdb_id"
            } else {
                "tmdb_id"
            };
            pairs.push((key, id.to_string()));
        }
        Lookup::Name { query, year } => {
            let words: Vec<&str> = query.split_whitespace().collect();
            pairs.push(("query", encode(&words.join(" ").to_lowercase())));
            if let Some(y) = year {
                pairs.push(("year", y.to_string()));
            }
        }
    }
    if let Some(s) = p.season {
        pairs.push(("season_number", s.to_string()));
    }
    if let Some(e) = p.episode {
        pairs.push(("episode_number", e.to_string()));
    }
    pairs.push((
        "type",
        if episodic { "episode" } else { "movie" }.to_owned(),
    ));
    let mut languages: Vec<String> = p
        .languages
        .iter()
        .map(|l| l.trim().to_lowercase())
        .filter(|l| !l.is_empty())
        .collect();
    languages.sort();
    languages.dedup();
    if !languages.is_empty() {
        let codes: Vec<String> = languages.iter().map(|l| encode(l)).collect();
        pairs.push(("languages", codes.join(",")));
    }
    pairs.sort_by(|a, b| a.0.cmp(b.0));
    let parts: Vec<String> = pairs.iter().map(|(k, v)| format!("{k}={v}")).collect();
    parts.join("&")
}

/// Form encoding, with lower-case escapes so the value stays lower-case.
fn encode(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(b))
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02x}")),
        }
    }
    out
}

/// One subtitle file found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub file_id: i64,
    pub language: String,
    pub release: String,
    pub hearing_impaired: bool,
    /// Only the parts in another language (`foreign_parts_only`).
    pub forced: bool,
    /// From a trusted uploader.
    pub trusted: bool,
    pub downloads: u64,
    /// Machine- or AI-translated.
    pub translated: bool,
    pub feature: Feature,
}

/// What OpenSubtitles says the subtitle is for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Feature {
    /// `movie`, `episode` or `tvshow`, lower-case.
    pub kind: Option<String>,
    pub tmdb_id: Option<i64>,
    pub parent_tmdb_id: Option<i64>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
}

#[derive(Deserialize)]
struct SearchPage {
    data: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct Row {
    attributes: Attributes,
}

#[derive(Deserialize)]
struct Attributes {
    language: Option<String>,
    download_count: Option<u64>,
    hearing_impaired: Option<bool>,
    foreign_parts_only: Option<bool>,
    from_trusted: Option<bool>,
    ai_translated: Option<bool>,
    machine_translated: Option<bool>,
    release: Option<String>,
    feature_details: Option<Details>,
    files: Option<Vec<FileRow>>,
}

#[derive(Deserialize)]
struct Details {
    feature_type: Option<String>,
    tmdb_id: Option<i64>,
    parent_tmdb_id: Option<i64>,
    season_number: Option<i32>,
    episode_number: Option<i32>,
}

#[derive(Deserialize)]
struct FileRow {
    file_id: Option<i64>,
}

/// A search answer's rows, each one file. A row in several files (one per
/// CD), without a file id, or not readable is left out.
pub fn parse_search(body: &str) -> anyhow::Result<Vec<Found>> {
    let page: SearchPage = serde_json::from_str(body)?;
    Ok(page.data.into_iter().filter_map(found).collect())
}

fn found(row: serde_json::Value) -> Option<Found> {
    let a = serde_json::from_value::<Row>(row).ok()?.attributes;
    let file_id = match a.files.as_deref() {
        Some([one]) => one.file_id?,
        _ => return None,
    };
    let feature = a
        .feature_details
        .map_or_else(Feature::default, |d| Feature {
            kind: d.feature_type.map(|k| k.to_lowercase()),
            tmdb_id: d.tmdb_id,
            parent_tmdb_id: d.parent_tmdb_id,
            season: d.season_number,
            episode: d.episode_number,
        });
    Some(Found {
        file_id,
        language: a.language.unwrap_or_default(),
        release: a.release.unwrap_or_default(),
        hearing_impaired: a.hearing_impaired.unwrap_or(false),
        forced: a.foreign_parts_only.unwrap_or(false),
        trusted: a.from_trusted.unwrap_or(false),
        downloads: a.download_count.unwrap_or(0),
        translated: a.ai_translated.unwrap_or(false) || a.machine_translated.unwrap_or(false),
        feature,
    })
}

/// A sign-in. No `Debug` of its own beyond `allowed`: the token is the
/// user's.
pub struct Login {
    pub token: String,
    /// The API base to use the token on.
    pub api_base: String,
    /// Downloads a day for this account.
    pub allowed: Option<u32>,
}

impl std::fmt::Debug for Login {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Login")
            .field("allowed", &self.allowed)
            .finish_non_exhaustive()
    }
}

/// A download's temporary link, and what is left of the allowance.
pub struct Link {
    /// The file, for a few hours; not to be logged or shown.
    pub url: String,
    pub remaining: Option<u32>,
    /// Unix seconds.
    pub reset_at: Option<i64>,
}

impl std::fmt::Debug for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Link")
            .field("remaining", &self.remaining)
            .field("reset_at", &self.reset_at)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct DownloadAnswer {
    link: Option<String>,
    remaining: Option<i64>,
    reset_time_utc: Option<String>,
}

/// The download answer. Its errors never quote it: it holds the link.
pub fn parse_download(body: &str) -> anyhow::Result<Link> {
    let a: DownloadAnswer = serde_json::from_str(body)
        .map_err(|e| anyhow::anyhow!("the download answer is not JSON ({:?})", e.classify()))?;
    let url = a
        .link
        .filter(|l| !l.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("the download answer has no link"))?;
    Ok(Link {
        url,
        remaining: a
            .remaining
            .map(|n| u32::try_from(n.max(0)).unwrap_or(u32::MAX)),
        reset_at: a.reset_time_utc.as_deref().and_then(utc_seconds),
    })
}

/// When a 406 says the downloads come back.
fn quota_reset(body: &str) -> Option<i64> {
    let a: DownloadAnswer = serde_json::from_str(body).ok()?;
    a.reset_time_utc.as_deref().and_then(utc_seconds)
}

/// `2026-11-08T12:00:00.000Z` (or with an offset, or a space for the `T`)
/// as Unix seconds.
pub(crate) fn utc_seconds(s: &str) -> Option<i64> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    if b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> {
        let part = s.get(from..to)?;
        part.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    let (year, month, day) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hour, minute, second) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut rest = &s[19..];
    if let Some(fraction) = rest.strip_prefix('.') {
        rest = fraction.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    let offset = match rest {
        "" | "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            // `+HH:MM`, `+HHMM` or `+HH`, and nothing longer.
            let digits = rest[1..].replacen(':', "", 1);
            if !matches!(digits.len(), 2 | 4) || !digits.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let h: i64 = digits[..2].parse().ok()?;
            let m: i64 = if digits.len() == 4 {
                digits[2..].parse().ok()?
            } else {
                0
            };
            if h > 23 || m > 59 {
                return None;
            }
            sign * (h * 3600 + m * 60)
        }
    };
    let days = crate::localtime::days_from_civil(year, month as u32, day as u32);
    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

fn is_api_host(host: &str) -> bool {
    host == "opensubtitles.com" || host.ends_with(".opensubtitles.com")
}

fn is_file_host(host: &str) -> bool {
    is_api_host(host) || host == "opensubtitles.org" || host.ends_with(".opensubtitles.org")
}

/// `url` is on HTTPS's own port at a host `ok` accepts, or is on `default`'s
/// origin (the stand-in's, in tests).
fn allowed_at(url: &str, default: &str, ok: fn(&str) -> bool) -> bool {
    let Ok(u) = Url::parse(url) else {
        return false;
    };
    let official = u.scheme() == "https"
        && u.port().is_none()
        && u.host_str().is_some_and(|h| ok(&h.to_ascii_lowercase()));
    official || Url::parse(default).is_ok_and(|d| d.origin() == u.origin())
}

/// Whether the token may be sent to `url`: an OpenSubtitles API host over
/// HTTPS, or `default`'s own origin.
pub fn token_host_allowed(url: &str, default: &str) -> bool {
    allowed_at(url, default, is_api_host)
}

/// Whether a download link may be fetched (or a redirect followed to it):
/// an OpenSubtitles host over HTTPS, or `default`'s own origin.
pub fn file_link_allowed(link: &str, default: &str) -> bool {
    allowed_at(link, default, is_file_host)
}

/// The API base a sign-in names (`base_url`, a bare host), when it is
/// OpenSubtitles' or `default`'s own; otherwise `default`.
pub fn login_base(base_url: Option<&str>, default: &str) -> String {
    let Some(given) = base_url.map(str::trim).filter(|s| !s.is_empty()) else {
        return default.to_owned();
    };
    let with_scheme = if given.contains("://") {
        given.to_owned()
    } else {
        format!("https://{given}")
    };
    let Ok(u) = Url::parse(&with_scheme) else {
        return default.to_owned();
    };
    let Some(host) = u.host_str().map(str::to_ascii_lowercase) else {
        return default.to_owned();
    };
    if is_api_host(&host) {
        return format!("https://{host}/api/v1");
    }
    let same = Url::parse(default)
        .is_ok_and(|d| d.host_str() == Some(host.as_str()) && d.port() == u.port());
    if !same {
        tracing::warn!("OpenSubtitles named an API host that is not its own; using the usual one");
    }
    default.to_owned()
}

#[derive(Deserialize)]
struct LoginAnswer {
    token: Option<String>,
    base_url: Option<String>,
    user: Option<LoginUser>,
}

#[derive(Deserialize)]
struct LoginUser {
    allowed_downloads: Option<u32>,
}

enum Pace {
    Shared(&'static Pacer),
    Own(Pacer),
}

/// OpenSubtitles, through its own pacer.
pub struct Client {
    http: reqwest::Client,
    base: String,
    pace: Pace,
}

/// A JSON answer's text, at most [`http::ANSWER_MAX_BYTES`].
async fn answer_text(resp: reqwest::Response) -> Result<String, OnlineError> {
    let bytes = http::read_capped(resp, http::ANSWER_MAX_BYTES, || {
        OnlineError::invalid("the answer is too large")
    })
    .await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    /// The real service (or the stand-in at [`API_OVERRIDE`]), on the shared
    /// [`PACER`].
    pub fn new() -> Self {
        Self {
            http: http::client(),
            base: api_base(),
            pace: Pace::Shared(&PACER),
        }
    }

    /// The service at `base`, on a pacer of its own (so no two tests share
    /// one).
    pub fn at(base: &str) -> Self {
        Self::at_with(base, Pacer::new(4, 30, Duration::from_secs(10)))
    }

    /// The service at `base`, on `pacer`.
    pub fn at_with(base: &str, pacer: Pacer) -> Self {
        let base = base.trim_end_matches('/').to_owned();
        let own = base.clone();
        Self {
            http: http::client_following(move |url| file_link_allowed(url, &own)),
            base,
            pace: Pace::Own(pacer),
        }
    }

    /// The API base this client asks.
    pub fn base(&self) -> &str {
        &self.base
    }

    fn pacer(&self) -> &Pacer {
        match &self.pace {
            Pace::Shared(p) => p,
            Pace::Own(p) => p,
        }
    }

    fn api(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.header("Api-Key", API_KEY)
            .header(ACCEPT, "application/json")
    }

    /// Where the token may go: `api_base` when it is OpenSubtitles' (or
    /// this client's own), this client's base otherwise.
    fn token_base<'a>(&'a self, api_base: &'a str) -> &'a str {
        if token_host_allowed(api_base, &self.base) {
            api_base.trim_end_matches('/')
        } else {
            tracing::warn!(
                "an OpenSubtitles sign-in named a host that is not its own; using the usual one"
            );
            &self.base
        }
    }

    pub async fn login(&self, user: &str, pass: &str) -> Result<Login, OnlineError> {
        let body = serde_json::json!({ "username": user, "password": pass });
        let req = self
            .api(self.http.post(format!("{}/login", self.base)))
            .json(&body);
        let resp = http::send(self.pacer(), req).await?;
        match resp.status().as_u16() {
            200..=299 => {}
            // A 400 is what an email address in place of the username gets.
            400 | 401 => {
                return Err(OnlineError::SignInRefused {
                    email: user.contains('@'),
                })
            }
            s => return Err(OnlineError::Refused(s)),
        }
        let text = answer_text(resp).await?;
        // Never quoted in an error: it holds the token.
        let a: LoginAnswer = serde_json::from_str(&text)
            .map_err(|_| OnlineError::invalid("the sign-in answer is not JSON"))?;
        let token = a
            .token
            .filter(|t| !t.is_empty())
            .ok_or_else(|| OnlineError::invalid("the sign-in answer has no token"))?;
        Ok(Login {
            token,
            api_base: login_base(a.base_url.as_deref(), &self.base),
            allowed: a.user.and_then(|u| u.allowed_downloads),
        })
    }

    /// Searches without a sign-in.
    pub async fn search(&self, p: &SearchParams) -> Result<Vec<Found>, OnlineError> {
        let url = format!("{}/subtitles?{}", self.base, search_query(p));
        let resp = http::send(self.pacer(), self.api(self.http.get(url))).await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(OnlineError::Refused(status.as_u16()));
        }
        let text = answer_text(resp).await?;
        parse_search(&text).map_err(|e| OnlineError::invalid(format!("{e:#}")))
    }

    /// The temporary link to `file_id` as SubRip, on the user's token.
    pub async fn download_link(
        &self,
        api_base: &str,
        token: &str,
        file_id: i64,
    ) -> Result<Link, OnlineError> {
        // Without a token the answer is known: nothing is sent.
        if token.trim().is_empty() {
            return Err(OnlineError::SignInNeeded);
        }
        let base = self.token_base(api_base);
        let body = serde_json::json!({ "file_id": file_id, "sub_format": "srt" });
        let req = self
            .api(self.http.post(format!("{base}/download")))
            .bearer_auth(token)
            .json(&body);
        let resp = http::send(self.pacer(), req).await?;
        let status = resp.status().as_u16();
        let text = answer_text(resp).await;
        match status {
            // Counted however its answer reads: one that cannot be read is
            // AnswerLost, so that the caller does not pay for it again.
            200..=299 => text
                .and_then(|t| {
                    parse_download(&t).map_err(|e| OnlineError::invalid(format!("{e:#}")))
                })
                .map_err(|e| {
                    tracing::warn!("OpenSubtitles counted a download whose answer was lost: {e}");
                    OnlineError::AnswerLost
                }),
            401 => Err(OnlineError::SignInNeeded),
            406 => Err(OnlineError::Quota {
                reset_at: text.ok().and_then(|t| quota_reset(&t)),
            }),
            s => Err(OnlineError::Refused(s)),
        }
    }

    /// The file behind a download link, with neither the key nor the token,
    /// and only from an OpenSubtitles host.
    pub async fn fetch_file(&self, link: &str) -> Result<Vec<u8>, OnlineError> {
        if !file_link_allowed(link, &self.base) {
            return Err(OnlineError::invalid(
                "the download link is not an OpenSubtitles address",
            ));
        }
        let resp = http::send_link(self.pacer(), self.http.get(link)).await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(OnlineError::Refused(status.as_u16()));
        }
        http::read_capped(resp, srt::SUBTITLE_MAX_BYTES, || {
            OnlineError::invalid("the subtitle file is too large")
        })
        .await
    }

    /// Ends the sign-in's token; best effort.
    pub async fn logout(&self, api_base: &str, token: &str) {
        if token.trim().is_empty() {
            return;
        }
        let base = self.token_base(api_base);
        let req = self
            .api(self.http.delete(format!("{base}/logout")))
            .bearer_auth(token);
        match http::send(self.pacer(), req).await {
            Ok(resp) if resp.status().is_success() => {}
            Ok(resp) => tracing::info!(
                "OpenSubtitles' sign-out answered {}",
                resp.status().as_u16()
            ),
            Err(e) => tracing::info!("OpenSubtitles' sign-out: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::online::http::{Admit, OnlineError, Pacer};
    use crate::test_http::{serve, Reply};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    const ROW: &str = r#"{"id":"1","type":"subtitle","attributes":{"language":"en","download_count":1200,"hearing_impaired":false,"foreign_parts_only":false,"from_trusted":true,"ai_translated":false,"machine_translated":false,"release":"Film.0001.2021.1080p.TEST","feature_details":{"feature_type":"Movie","tmdb_id":900001,"parent_tmdb_id":null,"season_number":null,"episode_number":null},"files":[{"file_id":7000001,"cd_number":1,"file_name":"Film.0001.srt"}]}}"#;

    const SRT: &str = "1\n00:00:01,000 --> 00:00:03,000\nTest line 1 of file 7000001\n";

    fn page(rows: &[&str]) -> String {
        format!(
            r#"{{"total_pages":1,"total_count":{},"per_page":60,"page":1,"data":[{}]}}"#,
            rows.len(),
            rows.join(",")
        )
    }

    fn film() -> SearchParams {
        SearchParams {
            lookup: Lookup::Tmdb(900001),
            season: None,
            episode: None,
            languages: vec!["en".into()],
        }
    }

    fn api(base: &str) -> String {
        format!("{base}/api/v1")
    }

    fn unix_now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    #[test]
    fn the_query_is_sorted_lower_case_and_without_defaults() {
        let ep = SearchParams {
            lookup: Lookup::Tmdb(55),
            season: Some(2),
            episode: Some(4),
            languages: vec!["fr".into(), "en".into()],
        };
        assert_eq!(
            search_query(&ep),
            "episode_number=4&languages=en,fr&parent_tmdb_id=55&season_number=2&type=episode"
        );
        let film = SearchParams {
            lookup: Lookup::Name {
                query: "A Made Up Film".into(),
                year: Some(2021),
            },
            season: None,
            episode: None,
            languages: vec!["pt-BR".into()],
        };
        assert_eq!(
            search_query(&film),
            "languages=pt-br&query=a+made+up+film&type=movie&year=2021"
        );
        let odd = SearchParams {
            lookup: Lookup::Name {
                query: " Film & Café ".into(),
                year: None,
            },
            season: None,
            episode: None,
            languages: vec![],
        };
        assert_eq!(search_query(&odd), "query=film+%26+caf%c3%a9&type=movie");
    }

    #[test]
    fn search_rows_read_and_odd_rows_are_left_out() {
        let two_files = ROW.replace(
            r#"[{"file_id":7000001,"cd_number":1,"file_name":"Film.0001.srt"}]"#,
            r#"[{"file_id":7000008,"cd_number":1},{"file_id":7000009,"cd_number":2}]"#,
        );
        let no_file_id = ROW.replace(r#""file_id":7000001,"#, "");
        let no_attributes = r#"{"id":"5","type":"subtitle","attributes":null}"#;
        let ai = ROW
            .replace("7000001", "7000003")
            .replace(r#""ai_translated":false"#, r#""ai_translated":true"#);
        let nulls = r#"{"id":"4","type":"subtitle","attributes":{"language":"fr","download_count":null,"hearing_impaired":null,"foreign_parts_only":true,"from_trusted":null,"ai_translated":null,"machine_translated":true,"release":null,"feature_details":null,"files":[{"file_id":7000004}]}}"#;
        let found = parse_search(&page(&[
            ROW,
            &two_files,
            &no_file_id,
            no_attributes,
            &ai,
            nulls,
        ]))
        .unwrap();
        assert_eq!(found.len(), 3, "{found:?}");
        assert_eq!(
            found[0],
            Found {
                file_id: 7000001,
                language: "en".into(),
                release: "Film.0001.2021.1080p.TEST".into(),
                hearing_impaired: false,
                forced: false,
                trusted: true,
                downloads: 1200,
                translated: false,
                feature: Feature {
                    kind: Some("movie".into()),
                    tmdb_id: Some(900001),
                    parent_tmdb_id: None,
                    season: None,
                    episode: None,
                },
            }
        );
        assert_eq!(found[1].file_id, 7000003);
        assert!(found[1].translated);
        assert_eq!(
            found[2],
            Found {
                file_id: 7000004,
                language: "fr".into(),
                release: String::new(),
                hearing_impaired: false,
                forced: true,
                trusted: false,
                downloads: 0,
                translated: true,
                feature: Feature::default(),
            }
        );
        assert!(parse_search(&page(&[])).unwrap().is_empty());
        assert!(parse_search("<html>busy</html>").is_err());
    }

    #[test]
    fn a_download_answer_reads_its_link_and_allowance() {
        let link = parse_download(
            r#"{"link":"https://www.opensubtitles.com/download/fake/subfile/7000001.srt","file_name":"7000001.srt","requests":1,"remaining":4,"message":"","reset_time":"6 hours","reset_time_utc":"2026-11-08T12:00:00.000Z"}"#,
        )
        .unwrap();
        assert_eq!(
            link.url,
            "https://www.opensubtitles.com/download/fake/subfile/7000001.srt"
        );
        assert_eq!(link.remaining, Some(4));
        assert_eq!(link.reset_at, Some(1_794_139_200));
        let bare = parse_download(r#"{"link":"https://dl.opensubtitles.org/x.srt"}"#).unwrap();
        assert_eq!((bare.remaining, bare.reset_at), (None, None));
        assert!(parse_download(r#"{"message":"no link"}"#).is_err());
        assert_eq!(
            utc_seconds("2026-11-08T14:00:00+02:00"),
            Some(1_794_139_200)
        );
        assert_eq!(utc_seconds("2026-11-08T10:30:00-0130"), Some(1_794_139_200));
        assert_eq!(utc_seconds("2026-11-08T14:00:00+02"), Some(1_794_139_200));
        // An offset that is no offset is refused, never overflowed.
        assert_eq!(
            utc_seconds("2026-11-08T12:00:00+99999999999999999:00"),
            None
        );
        assert_eq!(utc_seconds("2026-11-08T12:00:00+3074457345618258:00"), None);
        assert_eq!(utc_seconds("2026-11-08T12:00:00+24:00"), None);
        assert_eq!(utc_seconds("2026-11-08T12:00:00+02:60"), None);
        assert_eq!(utc_seconds("2026-11-08T12:00:00+"), None);
        assert_eq!(utc_seconds("2026-11-08 12:00:00"), Some(1_794_139_200));
        assert_eq!(utc_seconds("soon"), None);
    }

    #[test]
    fn the_token_goes_only_to_opensubtitles_hosts() {
        assert!(token_host_allowed(
            "https://vip-api.opensubtitles.com/api/v1",
            DEFAULT_API
        ));
        assert!(token_host_allowed(DEFAULT_API, DEFAULT_API));
        assert!(!token_host_allowed(
            "https://api.opensubtitles.com.example.net/api/v1",
            DEFAULT_API
        ));
        assert!(!token_host_allowed(
            "http://api.opensubtitles.com/api/v1",
            DEFAULT_API
        ));
        assert!(!token_host_allowed(
            "https://opensubtitles.org/api/v1",
            DEFAULT_API
        ));
        assert!(token_host_allowed(
            "http://127.0.0.1:9/api/v1",
            "http://127.0.0.1:9/api/v1"
        ));
        assert!(!token_host_allowed(
            "http://127.0.0.1:8/api/v1",
            "http://127.0.0.1:9/api/v1"
        ));
        assert_eq!(login_base(Some("evil.example"), DEFAULT_API), DEFAULT_API);
        assert_eq!(
            login_base(Some("vip-api.opensubtitles.com"), DEFAULT_API),
            "https://vip-api.opensubtitles.com/api/v1"
        );
        assert_eq!(login_base(None, DEFAULT_API), DEFAULT_API);
        assert_eq!(login_base(Some(""), DEFAULT_API), DEFAULT_API);
        assert_eq!(
            login_base(Some("api.opensubtitles.com.example.net"), DEFAULT_API),
            DEFAULT_API
        );
        // The stand-in answers with its own address.
        assert_eq!(
            login_base(Some("127.0.0.1:9"), "http://127.0.0.1:9/api/v1"),
            "http://127.0.0.1:9/api/v1"
        );
    }

    #[test]
    fn the_file_link_must_be_https_on_opensubtitles() {
        assert!(file_link_allowed(
            "https://dl.opensubtitles.org/en/download/file/7000001",
            DEFAULT_API
        ));
        assert!(file_link_allowed(
            "https://www.opensubtitles.com/download/fake/subfile/7000001.srt",
            DEFAULT_API
        ));
        assert!(!file_link_allowed(
            "http://dl.opensubtitles.org/en/download/file/7000001",
            DEFAULT_API
        ));
        assert!(!file_link_allowed(
            "https://files.example.net/7000001.srt",
            DEFAULT_API
        ));
        assert!(!file_link_allowed(
            "https://opensubtitles.org.example.net/7000001.srt",
            DEFAULT_API
        ));
        assert!(!file_link_allowed("not a link", DEFAULT_API));
        let stand_in = "http://127.0.0.1:19642/api/v1";
        assert!(file_link_allowed(
            "http://127.0.0.1:19642/files/7000001.srt",
            stand_in
        ));
        assert!(!file_link_allowed(
            "http://127.0.0.1:19643/files/7000001.srt",
            stand_in
        ));
    }

    #[test]
    fn a_login_and_a_link_debug_nothing_secret() {
        let login = Login {
            token: "fake-os-token-1".into(),
            api_base: DEFAULT_API.into(),
            allowed: Some(20),
        };
        let link = Link {
            url: "http://127.0.0.1:19642/files/7000001.srt".into(),
            remaining: Some(4),
            reset_at: Some(1_794_139_200),
        };
        let printed = format!("{login:?} {link:?} {login:#?} {link:#?}");
        assert!(!printed.contains("fake-os-token-1"));
        assert!(!printed.contains("7000001") && !printed.contains("127.0.0.1"));
        assert!(printed.contains("20") && printed.contains("1794139200"));
    }

    #[tokio::test]
    async fn every_request_names_zapper_and_the_key() {
        let server = serve(|req, base| match req.route() {
            "/api/v1/login" => Reply::ok(format!(
                r#"{{"token":"fake-os-token-1","base_url":"{}","user":{{"allowed_downloads":5}},"status":200}}"#,
                base.trim_start_matches("http://")
            )),
            "/api/v1/subtitles" => Reply::ok(page(&[ROW])),
            "/api/v1/download" => Reply::ok(format!(
                r#"{{"link":"{base}/files/7000001.srt","remaining":4,"reset_time_utc":"2026-11-08T12:00:00.000Z"}}"#
            )),
            "/api/v1/logout" => Reply::ok(r#"{"message":"token destroyed","status":200}"#),
            _ => Reply::new(404, ""),
        });
        let c = Client::at(&api(&server.base));
        let found = c.search(&film()).await.unwrap();
        assert_eq!(found.len(), 1);
        let login = c.login("tester", "test-pass").await.unwrap();
        assert_eq!(login.token, "fake-os-token-1");
        assert_eq!(login.api_base, api(&server.base));
        assert_eq!(login.allowed, Some(5));
        let link = c
            .download_link(&login.api_base, &login.token, 7000001)
            .await
            .unwrap();
        assert_eq!(link.url, format!("{}/files/7000001.srt", server.base));
        assert_eq!(link.remaining, Some(4));
        c.logout(&login.api_base, &login.token).await;

        let seen = server.requests();
        assert_eq!(
            seen.iter()
                .map(|r| (r.method.as_str(), r.path.as_str()))
                .collect::<Vec<_>>(),
            [
                (
                    "GET",
                    "/api/v1/subtitles?languages=en&tmdb_id=900001&type=movie"
                ),
                ("POST", "/api/v1/login"),
                ("POST", "/api/v1/download"),
                ("DELETE", "/api/v1/logout"),
            ]
        );
        for r in &seen {
            let agent = r.header("user-agent").unwrap_or_default();
            assert!(agent.starts_with("zapper v"), "{agent}");
            assert!(
                r.header("api-key") == Some(API_KEY),
                "{} lacks the app's key",
                r.route()
            );
            assert_eq!(
                r.header("accept"),
                Some("application/json"),
                "{}",
                r.route()
            );
        }
        assert_eq!(
            seen[0].header("authorization"),
            None,
            "search needs no sign-in"
        );
        assert_eq!(seen[1].header("authorization"), None);
        assert_eq!(
            seen[2].header("authorization"),
            Some("Bearer fake-os-token-1")
        );
        assert_eq!(
            seen[3].header("authorization"),
            Some("Bearer fake-os-token-1")
        );
    }

    #[tokio::test]
    async fn a_download_answers_sign_in_and_quota() {
        let server = serve(|req, _| match req.header("authorization") {
            Some("Bearer fake-os-token-1") => Reply::new(
                406,
                r#"{"message":"You have downloaded your allowed 5 subtitles for 24h","remaining":0,"reset_time_utc":"2026-11-08T12:00:00.000Z"}"#,
            ),
            _ => Reply::new(401, r#"{"message":"You cannot consume this service"}"#),
        });
        let c = Client::at(&api(&server.base));
        let base = api(&server.base);
        assert!(matches!(
            c.download_link(&base, "fake-os-token-2", 7000001).await,
            Err(OnlineError::SignInNeeded)
        ));
        assert!(matches!(
            c.download_link(&base, "fake-os-token-1", 7000001).await,
            Err(OnlineError::Quota {
                reset_at: Some(1_794_139_200)
            })
        ));
        // A base that is not OpenSubtitles' is not sent the token: the
        // client's own base is used instead.
        let elsewhere = "https://files.example.net/api/v1";
        assert!(!token_host_allowed(elsewhere, &base));
        let _ = c.download_link(elsewhere, "fake-os-token-1", 7000001).await;
        assert_eq!(server.requests().len(), 3);
        assert!(server.requests()[2].path.starts_with("/api/v1/download"));

        // No token, no request.
        for blank in ["", "  "] {
            assert!(matches!(
                c.download_link(&base, blank, 7000001).await,
                Err(OnlineError::SignInNeeded)
            ));
            c.logout(&base, blank).await;
        }
        assert_eq!(server.requests().len(), 3);
    }

    #[tokio::test]
    async fn the_file_link_gets_no_key_and_no_token() {
        let server = serve(|req, _| match req.route() {
            "/files/7000001.srt" => Reply::ok(SRT),
            "/files/7000002.srt" => Reply::ok(vec![b'1'; srt::SUBTITLE_MAX_BYTES + 1]),
            _ => Reply::new(404, ""),
        });
        let c = Client::at(&api(&server.base));
        let bytes = c
            .fetch_file(&format!("{}/files/7000001.srt", server.base))
            .await
            .unwrap();
        assert_eq!(bytes, SRT.as_bytes());
        let r = &server.requests()[0];
        assert!(
            r.header("api-key").is_none(),
            "the file link was sent the app's key"
        );
        assert_eq!(r.header("authorization"), None);
        assert!(r
            .header("user-agent")
            .unwrap_or_default()
            .starts_with("zapper v"));

        assert!(matches!(
            c.fetch_file(&format!("{}/files/7000002.srt", server.base))
                .await,
            Err(OnlineError::Invalid(_))
        ));
        assert!(matches!(
            c.fetch_file(&format!("{}/files/7000003.srt", server.base))
                .await,
            Err(OnlineError::Refused(404))
        ));
        // Refused before any request is built.
        let elsewhere = "https://files.example.net/7000001.srt";
        assert!(!file_link_allowed(elsewhere, c.base()));
        assert!(matches!(
            c.fetch_file(elsewhere).await,
            Err(OnlineError::Invalid(_))
        ));
        assert_eq!(server.requests().len(), 3, "the other host was never asked");
    }

    #[tokio::test]
    async fn a_429_blocks_every_request_until_retry_after() {
        let server = serve(|_, _| {
            Reply::new(429, r#"{"message":"Throttle limit reached"}"#).header("Retry-After", "30")
        });
        let c = Client::at(&api(&server.base));
        let before = unix_now();
        match c.search(&film()).await {
            Err(OnlineError::Busy { retry_at }) => {
                assert!(
                    (before + 29..=before + 32).contains(&retry_at),
                    "{retry_at} vs {before}"
                )
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(server.requests().len(), 1);
        let started = Instant::now();
        assert!(matches!(
            c.search(&film()).await,
            Err(OnlineError::Busy { .. })
        ));
        assert!(matches!(
            c.login("tester", "test-pass").await,
            Err(OnlineError::Busy { .. })
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(server.requests().len(), 1, "nothing more was sent");
    }

    #[tokio::test]
    async fn a_short_429_is_retried_once() {
        let n = AtomicUsize::new(0);
        let server = serve(move |_, _| {
            if n.fetch_add(1, Ordering::SeqCst) == 0 {
                Reply::new(429, "").header("Retry-After", "1")
            } else {
                Reply::ok(page(&[ROW]))
            }
        });
        let c = Client::at(&api(&server.base));
        assert_eq!(c.search(&film()).await.unwrap().len(), 1);
        assert_eq!(server.requests().len(), 2);
    }

    #[tokio::test]
    async fn an_unavailable_answer_is_retried_once() {
        let server = serve(|_, _| Reply::new(503, "down"));
        let c = Client::at(&api(&server.base));
        assert!(matches!(
            c.search(&film()).await,
            Err(OnlineError::Refused(503))
        ));
        assert_eq!(server.requests().len(), 2);
    }

    #[tokio::test]
    async fn a_long_wait_answers_busy_without_waiting() {
        let server = serve(|_, _| Reply::ok(page(&[ROW])));
        let pacer = Pacer::new(30, 30, Duration::from_secs(60));
        let now = Instant::now();
        for _ in 0..30 {
            assert_eq!(pacer.admit(now), Admit::Go);
        }
        let c = Client::at_with(&api(&server.base), pacer);
        let started = Instant::now();
        match c.search(&film()).await {
            Err(OnlineError::Busy { retry_at }) => assert!(retry_at >= unix_now() + 50),
            other => panic!("{other:?}"),
        }
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(server.requests().len(), 0);
    }

    #[tokio::test]
    async fn waits_add_up_to_no_more_than_max_wait() {
        // A 2 s wait for a turn, then a 4 s Retry-After: each fits in
        // MAX_WAIT, both together do not, so the 429 is answered Busy.
        let server = serve(|_, _| Reply::new(429, "").header("Retry-After", "4"));
        let pacer = Pacer::new(10, 1, Duration::from_secs(2));
        assert_eq!(pacer.admit(Instant::now()), Admit::Go);
        let c = Client::at_with(&api(&server.base), pacer);
        let started = Instant::now();
        assert!(matches!(
            c.search(&film()).await,
            Err(OnlineError::Busy { .. })
        ));
        let took = started.elapsed();
        assert!(took >= Duration::from_millis(1900), "{took:?}");
        assert!(took < Duration::from_secs(4), "{took:?}");
        assert_eq!(server.requests().len(), 1, "the 429 was not retried");
    }

    #[tokio::test]
    async fn a_login_with_an_email_says_so() {
        let server = serve(|_, _| Reply::new(400, r#"{"message":"Bad request"}"#));
        let c = Client::at(&api(&server.base));
        assert!(matches!(
            c.login("tester@example.com", "test-pass").await,
            Err(OnlineError::SignInRefused { email: true })
        ));
        let server = serve(|_, _| Reply::new(401, r#"{"message":"Invalid username/password"}"#));
        let c = Client::at(&api(&server.base));
        let e = c.login("tester", "fake-os-pass").await.unwrap_err();
        assert!(matches!(e, OnlineError::SignInRefused { email: false }));
        assert!(!format!("{e} {e:?}").contains("fake-os-pass"));
    }

    #[tokio::test]
    async fn a_transport_error_names_no_url() {
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = closed.local_addr().unwrap().port();
        drop(closed);
        let base = format!("http://127.0.0.1:{port}");
        let c = Client::at(&api(&base));
        let errors = [
            c.search(&film()).await.unwrap_err(),
            c.fetch_file(&format!("{base}/files/7000001.srt"))
                .await
                .unwrap_err(),
        ];
        for e in errors {
            let OnlineError::Unreachable(text) = &e else {
                panic!("{e:?}")
            };
            let shown = format!("{e} {text}");
            assert!(!shown.contains("http"), "{shown}");
            assert!(!shown.contains(&format!("127.0.0.1:{port}")), "{shown}");
            assert!(
                !shown.contains("/api/v1") && !shown.contains("7000001"),
                "{shown}"
            );
        }
    }

    #[tokio::test]
    async fn the_download_link_redirects_only_to_opensubtitles_hosts() {
        let server = serve(|req, base| {
            let route = req.route();
            if let Some(n) = route.strip_prefix("/files/hops/") {
                let n: u32 = n.parse().unwrap();
                return if n == 0 {
                    Reply::ok(SRT)
                } else {
                    Reply::new(302, "").header("Location", &format!("{base}/files/hops/{}", n - 1))
                };
            }
            match route {
                "/files/away" => {
                    Reply::new(302, "").header("Location", "https://evil.example/files/7000001.srt")
                }
                _ => Reply::new(404, ""),
            }
        });
        let c = Client::at(&api(&server.base));
        let got = c
            .fetch_file(&format!("{}/files/hops/3", server.base))
            .await
            .unwrap();
        assert_eq!(
            got,
            SRT.as_bytes(),
            "three hops on an allowed host are followed"
        );
        assert_eq!(server.requests().len(), 4);

        assert!(c
            .fetch_file(&format!("{}/files/away", server.base))
            .await
            .is_err());
        assert_eq!(server.requests().len(), 5);

        assert!(c
            .fetch_file(&format!("{}/files/hops/4", server.base))
            .await
            .is_err());
        assert_eq!(server.requests().len(), 9, "the fourth hop is not followed");
    }

    /// Valid JSON followed by `extra` bytes of spaces.
    fn padded(json: &str, extra: usize) -> String {
        format!("{json}{}", " ".repeat(extra))
    }

    #[tokio::test]
    async fn an_answer_past_a_mebibyte_is_not_read() {
        let over = 1024 * 1024;
        let server = serve(move |req, base| match req.route() {
            "/api/v1/subtitles" => Reply::ok(padded(&page(&[ROW]), over)),
            "/api/v1/login" => Reply::ok(padded(r#"{"token":"fake-os-token-1"}"#, over)),
            "/api/v1/download" => Reply::ok(padded(
                &format!(r#"{{"link":"{base}/files/7000001.srt","remaining":4}}"#),
                over,
            )),
            _ => Reply::new(404, ""),
        });
        let c = Client::at(&api(&server.base));
        let too_large = || OnlineError::invalid("the answer is too large");
        match c.search(&film()).await {
            Err(e) => assert_eq!(e, too_large()),
            Ok(found) => panic!("read in full: {} rows", found.len()),
        }
        match c.login("tester", "test-pass").await {
            Err(e) => assert_eq!(e, too_large()),
            Ok(_) => panic!("read in full"),
        }
        // A download answered is counted, however its answer reads.
        match c
            .download_link(&api(&server.base), "fake-os-token-1", 7000001)
            .await
        {
            Err(e) => assert_eq!(e, OnlineError::AnswerLost),
            Ok(_) => panic!("read in full"),
        }
        // Just under the cap is read.
        let server = serve(move |_, _| {
            let json = page(&[ROW]);
            Reply::ok(padded(&json, over - json.len()))
        });
        let c = Client::at(&api(&server.base));
        assert!(c.search(&film()).await.is_ok_and(|f| f.len() == 1));
    }

    #[tokio::test]
    async fn a_download_answer_that_is_not_json_was_still_counted() {
        let server = serve(|_, _| Reply::ok("<html>not json</html>"));
        let c = Client::at(&api(&server.base));
        match c
            .download_link(&api(&server.base), "fake-os-token-1", 7000001)
            .await
        {
            Err(e) => assert_eq!(e, OnlineError::AnswerLost),
            Ok(_) => panic!("read"),
        }
        // A refusal is not counted: it says what it is.
        let server = serve(|_, _| Reply::new(403, "<html>no</html>"));
        let c = Client::at(&api(&server.base));
        assert!(matches!(
            c.download_link(&api(&server.base), "fake-os-token-1", 7000001)
                .await,
            Err(OnlineError::Refused(403))
        ));
    }
}
