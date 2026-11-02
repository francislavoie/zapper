//! Which channel a recording falls back to when its own feed fails
//! (`fallback`), chosen from the catalogue.
//!
//! The candidates, best first:
//!
//! 1. **Channels carrying the same guide channel** (`epg_channel_id`): the
//!    same programme on another feed, which is what a backup is for — the
//!    HD and SD copies of a channel, a provider's spare. Among them, one in
//!    the same category as the main channel (the same provider's line-up
//!    rather than a regional variant filed elsewhere), then the one playing
//!    has shown to be better (`channel_quality`: not failing the last time
//!    it was opened, then the higher resolution, then the higher frame
//!    rate; never opened counts below any reading), then the lower channel
//!    number.
//! 2. Only when no channel shares the guide channel: **channels whose guide
//!    has the same programme**, the same title starting within
//!    [`TITLE_WINDOW_SECS`] of the recording's start — a simulcast on a
//!    channel the provider files under another guide id. Titles are
//!    compared without case and with runs of white space as one space.
//!    A title too short or too common to mean one programme ("News",
//!    "Movie", anything under four letters) matches nothing, and neither
//!    does one airing on more than [`MAX_TITLE_CHANNELS`] guide channels
//!    at once: that is a format, not a programme. Ordered the same way, with
//!    the closer start first after the category.
//!
//! The first is what a recording gets when it is scheduled without one;
//! the whole list is what the Recordings dialog offers to choose from.
//!
//! Fast on a large catalogue: the first is one lookup on `channels_by_epg`;
//! the second reads the programmes starting within ten minutes of one time
//! (`epg_by_start`, a few thousand rows on a large guide) and the channels
//! of the few guide channels that match.

use std::cmp::Ordering;
use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{params_from_iter, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::catalog::origin::current_origin_sql;

/// How far apart two airings' starts may be and still be one programme.
pub const TITLE_WINDOW_SECS: i64 = 5 * 60;
/// More guide channels than this showing a title at once makes it generic.
pub const MAX_TITLE_CHANNELS: usize = 10;
/// The most candidates offered.
pub const MAX_CANDIDATES: usize = 20;

/// Why a channel is a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BackupReason {
    /// It carries the same guide channel.
    Guide,
    /// Its guide has the same programme at the same time.
    Programme,
}

/// One channel a recording could fall back to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupDto {
    pub stream_id: i64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_label: Option<String>,
    pub reason: BackupReason,
    /// The last measured picture height, when it has been played.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<i64>,
    /// The last time it was opened, it failed to load.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub failing: bool,
}

/// `GET /api/recordings/{id}/backups`: the recording's backup now, and what
/// it could be.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupsDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<i64>,
    pub candidates: Vec<BackupDto>,
}

/// `PUT /api/recordings/{id}/backup`: a channel, or `null` for none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetBackupRequest {
    pub stream_id: Option<i64>,
}

/// What the ranking reads about each candidate.
#[derive(Debug, Clone)]
struct Ranked {
    dto: BackupDto,
    same_category: bool,
    fps: f64,
    /// How far its airing starts from the recording's, for a title match.
    gap: i64,
}

fn better(a: &Ranked, b: &Ranked) -> Ordering {
    b.same_category
        .cmp(&a.same_category)
        .then(a.dto.failing.cmp(&b.dto.failing))
        .then(a.gap.cmp(&b.gap))
        .then(b.dto.height.unwrap_or(0).cmp(&a.dto.height.unwrap_or(0)))
        .then(b.fps.partial_cmp(&a.fps).unwrap_or(Ordering::Equal))
        .then(a.dto.num.is_none().cmp(&b.dto.num.is_none()))
        .then(a.dto.num.cmp(&b.dto.num))
        .then(a.dto.stream_id.cmp(&b.dto.stream_id))
}

/// A title as compared: lower case, white space collapsed.
pub fn normalise(title: &str) -> String {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Titles that name a kind of programme, not one programme.
const GENERIC: &[&str] = &[
    "news", "the news", "local news", "world news", "news update", "headlines", "weather",
    "the weather", "sport", "sports", "sports news", "movie", "the movie", "film", "feature film",
    "music", "music videos", "paid programming", "paid program", "teleshopping", "shopping",
    "infomercial", "to be announced", "tba", "tbd", "programming", "regular programming",
    "local programming", "off air", "closedown", "close", "station close", "documentary",
    "kids", "cartoons", "children's programmes", "series", "live", "live coverage", "highlights",
    "evening news", "morning news", "late news", "noticias", "nachrichten", "journal",
    "le journal", "info", "information", "telejournal", "notizie", "telegiornale",
];

/// Whether a title is too short or too common to match a programme by.
pub fn is_generic(normalised: &str) -> bool {
    normalised.chars().filter(|c| c.is_alphanumeric()).count() < 4
        || GENERIC.contains(&normalised)
}

/// The channel columns and quality reading each candidate query selects,
/// from `channels b` and the main channel `a`.
const CANDIDATE_COLUMNS: &str = "b.stream_id, b.name, b.num, b.num_label,
    COALESCE(b.category_id = a.category_id, 0), q.failed_at IS NOT NULL, q.height, q.fps";

fn quality_join() -> String {
    format!(
        "LEFT JOIN channel_quality q ON q.stream_id = b.stream_id AND q.origin = {}",
        current_origin_sql!()
    )
}

fn ranked_from_row(r: &rusqlite::Row, reason: BackupReason) -> rusqlite::Result<Ranked> {
    Ok(Ranked {
        dto: BackupDto {
            stream_id: r.get(0)?,
            name: r.get(1)?,
            num: r.get(2)?,
            num_label: r.get(3)?,
            reason,
            height: r.get(6)?,
            failing: r.get(5)?,
        },
        same_category: r.get(4)?,
        fps: r.get::<_, Option<f64>>(7)?.unwrap_or(0.0),
        gap: 0,
    })
}

/// The candidates for a recording of `stream_id`'s `title` at `start`,
/// best first, at most `limit`.
pub fn candidates(
    c: &Connection,
    stream_id: i64,
    title: &str,
    start: i64,
    limit: usize,
) -> Result<Vec<BackupDto>> {
    let mut found = same_guide(c, stream_id)?;
    if found.is_empty() {
        found = same_programme(c, stream_id, title, start)?;
    }
    found.sort_by(better);
    found.truncate(limit);
    Ok(found.into_iter().map(|r| r.dto).collect())
}

/// The best candidate, if there is one.
pub fn best(c: &Connection, stream_id: i64, title: &str, start: i64) -> Result<Option<i64>> {
    Ok(candidates(c, stream_id, title, start, 1)?
        .first()
        .map(|b| b.stream_id))
}

fn same_guide(c: &Connection, stream_id: i64) -> Result<Vec<Ranked>> {
    let sql = format!(
        "SELECT {CANDIDATE_COLUMNS}
         FROM channels a
         JOIN channels b ON b.epg_channel_id = a.epg_channel_id AND b.stream_id != a.stream_id
         {}
         WHERE a.stream_id = ?1 AND a.epg_channel_id IS NOT NULL AND a.epg_channel_id != ''
           AND b.deleted_at IS NULL AND b.is_separator = 0",
        quality_join()
    );
    let mut st = c.prepare(&sql)?;
    let rows = st
        .query_map([stream_id], |r| ranked_from_row(r, BackupReason::Guide))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn same_programme(c: &Connection, stream_id: i64, title: &str, start: i64) -> Result<Vec<Ranked>> {
    let want = normalise(title);
    if is_generic(&want) {
        return Ok(Vec::new());
    }
    let own: Option<String> = c
        .query_row(
            "SELECT epg_channel_id FROM channels WHERE stream_id = ?1",
            [stream_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    // The guide channels airing it, each with its airing closest in time.
    let mut gaps: HashMap<String, i64> = HashMap::new();
    {
        let mut st = c.prepare_cached(
            "SELECT channel_ref, start, title FROM epg WHERE start BETWEEN ?1 AND ?2",
        )?;
        let mut rows = st.query([start - TITLE_WINDOW_SECS, start + TITLE_WINDOW_SECS])?;
        while let Some(r) = rows.next()? {
            let chan: String = r.get(0)?;
            if own.as_deref() == Some(chan.as_str()) {
                continue;
            }
            let t: String = r.get(2)?;
            if normalise(&t) != want {
                continue;
            }
            let gap = (r.get::<_, i64>(1)? - start).abs();
            gaps.entry(chan)
                .and_modify(|g| *g = (*g).min(gap))
                .or_insert(gap);
        }
    }
    if gaps.is_empty() || gaps.len() > MAX_TITLE_CHANNELS {
        return Ok(Vec::new());
    }
    let refs: Vec<String> = gaps.keys().cloned().collect();
    let marks = (0..refs.len())
        .map(|i| format!("?{}", i + 2))
        .collect::<Vec<_>>()
        .join(",");
    // `a` is the main channel, for the category; LEFT so a main channel the
    // catalogue no longer has still finds its programme elsewhere.
    let sql = format!(
        "SELECT {CANDIDATE_COLUMNS}, b.epg_channel_id
         FROM channels b
         LEFT JOIN channels a ON a.stream_id = ?1
         {}
         WHERE b.epg_channel_id IN ({marks}) AND b.stream_id != ?1
           AND b.deleted_at IS NULL AND b.is_separator = 0",
        quality_join()
    );
    let mut st = c.prepare(&sql)?;
    let args: Vec<rusqlite::types::Value> = std::iter::once(stream_id.into())
        .chain(refs.into_iter().map(Into::into))
        .collect();
    let rows = st
        .query_map(params_from_iter(args), |r| {
            let mut ranked = ranked_from_row(r, BackupReason::Programme)?;
            let chan: String = r.get(8)?;
            ranked.gap = gaps.get(&chan).copied().unwrap_or(0);
            Ok(ranked)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Whether `stream_id` is a channel a recording can switch to: one the
/// catalogue lists, not a divider. Its name, if so.
pub fn usable(c: &Connection, stream_id: i64) -> Result<Option<String>> {
    Ok(c.query_row(
        "SELECT name FROM channels WHERE stream_id = ?1 AND deleted_at IS NULL AND is_separator = 0",
        [stream_id],
        |r| r.get(0),
    )
    .optional()?)
}

/// The names of `ids`, those the catalogue has.
pub fn names(c: &Connection, ids: &[i64]) -> Result<HashMap<i64, String>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let marks = vec!["?"; ids.len()].join(",");
    let mut st = c.prepare(&format!(
        "SELECT stream_id, name FROM channels WHERE stream_id IN ({marks})"
    ))?;
    let rows = st
        .query_map(params_from_iter(ids.iter()), |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<HashMap<i64, String>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: &str = "test-origin";

    fn catalogue() -> Connection {
        let c = crate::catalog::open_in_memory().unwrap();
        c.execute(
            "INSERT INTO meta (key, value) VALUES ('catalogue_origin', ?1)",
            [ORIGIN],
        )
        .unwrap();
        c
    }

    fn channel(c: &Connection, id: i64, name: &str, cat: &str, epg: Option<&str>, num: i64) {
        c.execute(
            "INSERT INTO channels (stream_id, name, category_id, epg_channel_id, num)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, name, cat, epg, num],
        )
        .unwrap();
    }

    fn quality(c: &Connection, id: i64, height: i64, fps: f64, failed: bool) {
        c.execute(
            "INSERT INTO channel_quality (origin, stream_id, height, fps, failed_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![ORIGIN, id, height, fps, failed.then_some(1_000i64)],
        )
        .unwrap();
    }

    fn programme(c: &Connection, chan: &str, start: i64, title: &str) {
        c.execute(
            "INSERT INTO epg (channel_ref, start, stop, title) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![chan, start, start + 3600, title],
        )
        .unwrap();
    }

    fn ids(v: &[BackupDto]) -> Vec<i64> {
        v.iter().map(|b| b.stream_id).collect()
    }

    const T: i64 = 1_800_000_000;

    #[test]
    fn the_same_guide_channel_ranks_by_category_then_quality_then_number() {
        let c = catalogue();
        channel(&c, 1, "One HD", "uk", Some("one.uk"), 1);
        channel(&c, 2, "One SD", "uk", Some("one.uk"), 101);
        channel(&c, 3, "One FHD", "uk", Some("one.uk"), 201);
        channel(&c, 4, "One 50", "uk", Some("one.uk"), 301);
        channel(&c, 5, "One (abroad)", "intl", Some("one.uk"), 2);
        channel(&c, 6, "One broken", "uk", Some("one.uk"), 3);
        channel(&c, 7, "One gone", "uk", Some("one.uk"), 4);
        channel(&c, 8, "Two", "uk", Some("two.uk"), 5);
        c.execute("UPDATE channels SET deleted_at = 1 WHERE stream_id = 7", [])
            .unwrap();
        quality(&c, 3, 1080, 25.0, false);
        quality(&c, 4, 1080, 50.0, false);
        quality(&c, 2, 576, 25.0, false);
        quality(&c, 6, 2160, 50.0, true);
        quality(&c, 5, 2160, 50.0, false);
        // Another catalogue's reading of the same id counts for nothing.
        c.execute(
            "INSERT INTO channel_quality (origin, stream_id, height) VALUES ('other', 2, 4320)",
            [],
        )
        .unwrap();

        let got = candidates(&c, 1, "Anything", T, 20).unwrap();
        // Same category first; in it, working before failing, then 1080p50,
        // 1080p25, 576p; the one failing last of its category; then the
        // other category however good; never the deleted one.
        assert_eq!(ids(&got), [4, 3, 2, 6, 5]);
        assert!(got.iter().all(|b| b.reason == BackupReason::Guide));
        assert!(got[3].failing);
        assert_eq!(got[0].height, Some(1080));
        assert_eq!(best(&c, 1, "Anything", T).unwrap(), Some(4));
        assert_eq!(ids(&candidates(&c, 1, "x", T, 2).unwrap()), [4, 3]);
        assert_eq!(best(&c, 8, "Anything", T).unwrap(), None);
    }

    #[test]
    fn unmeasured_channels_go_by_number() {
        let c = catalogue();
        channel(&c, 1, "A", "c", Some("a"), 10);
        channel(&c, 2, "B", "c", Some("a"), 30);
        channel(&c, 3, "C", "c", Some("a"), 20);
        assert_eq!(ids(&candidates(&c, 1, "x", T, 20).unwrap()), [3, 2]);
    }

    #[test]
    fn without_a_shared_guide_channel_the_same_programme_elsewhere_is_found() {
        let c = catalogue();
        channel(&c, 1, "Harbour One", "uk", Some("h1"), 1);
        channel(&c, 2, "Harbour Simulcast", "uk", Some("h-sim"), 50);
        channel(&c, 3, "Harbour Abroad", "intl", Some("h-intl"), 5);
        channel(&c, 4, "Late Copy", "uk", Some("late"), 6);
        channel(&c, 5, "Other Show", "uk", Some("other"), 7);
        channel(&c, 6, "No guide", "uk", None, 8);
        programme(&c, "h1", T, "Harbour  Lights");
        programme(&c, "h-sim", T + 120, "harbour lights");
        programme(&c, "h-intl", T, "HARBOUR LIGHTS");
        programme(&c, "late", T + 3600, "Harbour Lights");
        programme(&c, "other", T, "Quayside");

        let got = candidates(&c, 1, "Harbour Lights", T, 20).unwrap();
        assert_eq!(ids(&got), [2, 3]);
        assert!(got.iter().all(|b| b.reason == BackupReason::Programme));

        // A main channel without a guide id of its own finds it too.
        assert_eq!(best(&c, 6, "Harbour Lights", T).unwrap(), Some(1));
        // Five minutes either side, no more.
        assert_eq!(
            ids(&candidates(&c, 1, "Harbour Lights", T + 2 * 3600, 20).unwrap()),
            Vec::<i64>::new()
        );
    }

    #[test]
    fn a_shared_guide_channel_wins_over_a_title_match() {
        let c = catalogue();
        channel(&c, 1, "One", "c", Some("one"), 1);
        channel(&c, 2, "One again", "c", Some("one"), 2);
        channel(&c, 3, "Simulcast", "c", Some("sim"), 3);
        programme(&c, "one", T, "Harbour Lights");
        programme(&c, "sim", T, "Harbour Lights");
        assert_eq!(ids(&candidates(&c, 1, "Harbour Lights", T, 20).unwrap()), [2]);
    }

    #[test]
    fn generic_and_widespread_titles_match_nothing() {
        let c = catalogue();
        channel(&c, 1, "Main", "c", Some("main"), 1);
        for (i, title) in ["News", "  the   NEWS ", "Film", "Q&A"].iter().enumerate() {
            let chan = format!("g{i}");
            channel(&c, 10 + i as i64, "Elsewhere", "c", Some(&chan), 10 + i as i64);
            programme(&c, &chan, T, title);
            assert!(
                candidates(&c, 1, title, T, 20).unwrap().is_empty(),
                "{title:?} matched"
            );
        }
        // Eleven guide channels at once: a format, not one programme.
        for i in 0..11 {
            let chan = format!("w{i}");
            channel(&c, 100 + i, "Wide", "c", Some(&chan), 100 + i);
            programme(&c, &chan, T, "Countdown to Midnight");
        }
        assert!(candidates(&c, 1, "Countdown to Midnight", T, 20).unwrap().is_empty());
        assert!(is_generic(&normalise("TBA")));
        assert!(!is_generic(&normalise("Harbour Lights")));
    }

    #[test]
    fn usable_channels_and_names() {
        let c = catalogue();
        channel(&c, 1, "One", "c", None, 1);
        channel(&c, 2, "Divider", "c", None, 2);
        c.execute("UPDATE channels SET is_separator = 1 WHERE stream_id = 2", [])
            .unwrap();
        assert_eq!(usable(&c, 1).unwrap().as_deref(), Some("One"));
        assert_eq!(usable(&c, 2).unwrap(), None);
        assert_eq!(usable(&c, 3).unwrap(), None);
        let n = names(&c, &[1, 3]).unwrap();
        assert_eq!((n.get(&1).map(String::as_str), n.len()), (Some("One"), 1));
    }
}
