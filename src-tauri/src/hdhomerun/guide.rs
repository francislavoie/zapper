//! SiliconDust's guide for HDHomeRun owners.
//!
//! `https://api.hdhomerun.com/api/guide?DeviceAuth=<token>` answers with the
//! listings for the tuner's own lineup, as JSON: one object per channel
//! (`GuideNumber`, `GuideName`, `Guide: [{StartTime, EndTime, Title,
//! EpisodeTitle, Synopsis, …}]`), times in Unix seconds. It is free for
//! anyone who owns a tuner — the token is how the service knows that — and it
//! is what SiliconDust's own apps show, so it is the default; an XMLTV guide
//! in the account replaces it for people who already run a grabber.
//!
//! Each answer covers a window starting at `Start` (now, when absent), and
//! the window is a few hours, not days. So a sync asks again from the end of
//! the last answer until it has a day ahead, the service stops returning
//! anything new, or [`MAX_WINDOWS`] requests have gone out — whichever comes
//! first. A day is what the guide grid shows without scrolling far, and a
//! handful of requests every few hours is a polite load on a free service.
//!
//! Programmes are filed under the channel's `GuideNumber`
//! ([`super::device::guide_ref`]), which is also what each lineup entry's
//! guide channel is, so the two meet without any name matching.
//!
//! **The token is a credential.** It is in the query string of every
//! request; requests drop their URL from errors, and `DeviceAuth=` is among
//! the keys redaction knows (`m3u::redact`).

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

pub const GUIDE_API: &str = "https://api.hdhomerun.com";

/// How far ahead a sync tries to fill.
pub const AHEAD_SECS: i64 = 24 * 3600;

/// Requests per sync, at most.
pub const MAX_WINDOWS: usize = 8;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct GuideChannel {
    #[serde(rename = "GuideNumber", default)]
    pub guide_number: Option<serde_json::Value>,
    #[serde(rename = "Guide", default)]
    pub guide: Vec<GuideProgramme>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct GuideProgramme {
    #[serde(rename = "StartTime", default)]
    pub start: i64,
    #[serde(rename = "EndTime", default)]
    pub end: i64,
    #[serde(rename = "Title", default)]
    pub title: String,
    #[serde(rename = "EpisodeTitle", default)]
    pub episode_title: Option<String>,
    #[serde(rename = "Synopsis", default)]
    pub synopsis: Option<String>,
}

impl GuideProgramme {
    /// The episode's own title and the synopsis, as one description: the
    /// mirror has one text field beside the title, and "Saturday Morning —
    /// Local news & weather." reads better than either half alone.
    pub fn description(&self) -> Option<String> {
        let ep = self.episode_title.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let syn = self.synopsis.as_deref().map(str::trim).filter(|s| !s.is_empty());
        match (ep, syn) {
            (Some(e), Some(s)) => Some(format!("{e} — {s}")),
            (Some(x), None) | (None, Some(x)) => Some(x.to_owned()),
            (None, None) => None,
        }
    }
}

/// The request for the window starting at `start` (now, if `None`).
pub fn url(api: &str, device_auth: &str, start: Option<i64>) -> String {
    let mut u = format!(
        "{}/api/guide?DeviceAuth={}",
        api.trim_end_matches('/'),
        encode(device_auth)
    );
    if let Some(s) = start {
        u.push_str(&format!("&Start={s}"));
    }
    u
}

/// Percent-encodes what a token might contain that a query cannot.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// One answer. The service says no with a JSON object (`{"Error":…}`) or
/// `null` rather than a list, and that is reported rather than read as an
/// empty guide.
pub fn parse(body: &[u8]) -> Result<Vec<GuideChannel>> {
    let v: serde_json::Value =
        serde_json::from_slice(body).context("the guide service did not answer with JSON")?;
    match v {
        serde_json::Value::Array(_) => {
            serde_json::from_value(v).context("reading the guide service's answer")
        }
        serde_json::Value::Object(o) => {
            let why = o
                .get("Error")
                .and_then(|e| e.as_str())
                .unwrap_or("no listings");
            Err(anyhow!("SiliconDust's guide service said: {why}"))
        }
        _ => Err(anyhow!(
            "SiliconDust's guide service had no listings for this tuner"
        )),
    }
}

/// The latest end time in an answer: where the next window starts.
pub fn latest_end(channels: &[GuideChannel]) -> Option<i64> {
    channels
        .iter()
        .flat_map(|c| c.guide.iter().map(|p| p.end))
        .max()
}

/// Writes an answer's programmes into the mirror's guide, in one
/// transaction. Returns how many were written.
pub fn land(conn: &rusqlite::Connection, channels: &[GuideChannel]) -> Result<usize> {
    let tx = conn.unchecked_transaction()?;
    let mut n = 0;
    {
        // The XMLTV parser's statement: the same table, the same upsert.
        let mut stmt = tx.prepare_cached(
            "INSERT INTO epg (channel_ref, start, stop, title, description)
             VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(channel_ref, start) DO UPDATE SET
               stop=excluded.stop, title=excluded.title, description=excluded.description",
        )?;
        for c in channels {
            let number = super::text(&c.guide_number);
            if number.is_empty() {
                continue;
            }
            let chan = super::device::guide_ref(&number);
            for p in &c.guide {
                if p.end <= p.start || p.title.trim().is_empty() {
                    continue;
                }
                stmt.execute(rusqlite::params![
                    chan,
                    p.start,
                    p.end,
                    p.title.trim(),
                    p.description()
                ])?;
                n += 1;
            }
        }
    }
    // As the XMLTV parser does: searchable from the moment it is committed.
    crate::catalog::guide_search::index_changes(&tx)?;
    tx.commit()?;
    Ok(n)
}

/// Points every lineup channel back at its own guide channel.
///
/// A sync that used an XMLTV guide matched channels to that guide's ids;
/// switching back to SiliconDust's has to undo that, or every channel keeps
/// looking for listings under a name this guide does not use.
pub fn own_guide_channels(conn: &rusqlite::Connection) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE channels SET epg_channel_id = tvg_id
         WHERE stream_url IS NOT NULL AND deleted_at IS NULL
           AND tvg_id LIKE 'hdhr:%' AND epg_channel_id IS NOT tvg_id",
        [],
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUIDE: &[u8] = include_bytes!("../../tests/fixtures/hdhomerun/guide.json");

    #[test]
    fn the_request_carries_the_token_encoded_and_the_window() {
        assert_eq!(
            url("https://api.hdhomerun.com/", "ab+c/=", Some(1_700_000_000)),
            "https://api.hdhomerun.com/api/guide?DeviceAuth=ab%2Bc%2F%3D&Start=1700000000"
        );
        let shown = crate::m3u::redact::redact_url(&url(GUIDE_API, "secretTOKEN", None)).0;
        assert!(!shown.contains("secretTOKEN"), "{shown}");
    }

    #[test]
    fn an_answer_is_read_into_programmes() {
        let g = parse(GUIDE).unwrap();
        assert_eq!(g.len(), 3);
        assert_eq!(g[0].guide.len(), 2);
        assert_eq!(
            g[0].guide[0].description().as_deref(),
            Some("Saturday Morning — Local news & weather.")
        );
        assert_eq!(g[0].guide[1].description(), None);
        assert_eq!(latest_end(&g), Some(1_761_987_600));
    }

    #[test]
    fn a_refusal_is_an_error_not_an_empty_guide() {
        let err = parse(br#"{"Error":"invalid DeviceAuth"}"#).unwrap_err().to_string();
        assert!(err.contains("invalid DeviceAuth"), "{err}");
        assert!(parse(b"null").is_err());
        assert!(parse(b"<html>").is_err());
        assert!(parse(b"[]").unwrap().is_empty());
    }

    #[test]
    fn programmes_land_under_the_lineups_guide_channels() {
        let c = crate::catalog::open_in_memory().unwrap();
        let n = land(&c, &parse(GUIDE).unwrap()).unwrap();
        assert_eq!(n, 4);
        // Twice is an update, not a duplicate.
        land(&c, &parse(GUIDE).unwrap()).unwrap();
        let (count, desc): (i64, Option<String>) = c
            .query_row(
                "SELECT COUNT(*), MAX(description) FROM epg WHERE channel_ref = 'hdhr:5.1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((count, desc.as_deref()), (1, Some("Otters at Herons.")));
    }

    #[test]
    fn landed_programmes_are_searchable_at_once() {
        let c = crate::catalog::open_in_memory().unwrap();
        land(&c, &parse(GUIDE).unwrap()).unwrap();
        let n: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM epg_fts WHERE epg_fts MATCH '\"rowing\"*'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }
}
