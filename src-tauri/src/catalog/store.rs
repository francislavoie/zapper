//! Read helpers over the catalogue tables.
//!
//! Everything the UI needs at startup is answered from here. No function in
//! this module may touch the network: a week-old mirror must open exactly as
//! fast as a fresh one.

use anyhow::Result;
use rusqlite::{Connection, Row};
use std::collections::HashMap;

use crate::catalog::origin::current_origin_sql;
use crate::ipc::types::{
    ActivityRowDto, CategoryDto, ChannelDto, FavouriteDto, GuideRowDto, ProgrammeDto, QualityDto,
};

/// The channel columns every listing selects, in the order
/// [`channel_from_row`] expects. Kept in one place so the callers cannot
/// drift out of step with the mapper. Selected from `channels ch`
/// [`QUALITY_JOIN`]; a caller's own columns follow, from index
/// [`CHANNEL_COLUMN_COUNT`].
pub(crate) const CHANNEL_COLUMNS: &str = "ch.stream_id, ch.name, ch.clean_name, ch.num, ch.logo_url,
     ch.category_id, ch.has_archive, ch.archive_days, ch.is_separator,
     q.stream_id, q.codec, q.width, q.height, q.fps, q.is_hdr, q.ok_count, q.fail_count,
     ch.num_label,
     q.interlaced, q.audio_codec, q.audio_channels, q.measured_at, q.failed_at, q.fail_reason";

/// How many columns [`CHANNEL_COLUMNS`] is.
pub(crate) const CHANNEL_COLUMN_COUNT: usize = 24;

/// What has been learnt about each channel by playing it, for this
/// catalogue only: the readings are keyed on the account as well as the
/// channel, see `channel_quality` in [`crate::catalog::schema`].
pub(crate) const QUALITY_JOIN: &str = concat!(
    "LEFT JOIN channel_quality q ON q.stream_id = ch.stream_id AND q.origin = ",
    current_origin_sql!()
);

pub(crate) fn channel_from_row(row: &Row) -> rusqlite::Result<ChannelDto> {
    // A missing row is the normal case, not an error: most of a large
    // catalogue will never be opened, and unknown is not the same as bad.
    // The left-joined key is the only honest presence test, since every
    // other column of it is itself nullable.
    let quality = match row.get::<_, Option<i64>>(9)? {
        None => None,
        Some(_) => Some(QualityDto {
            codec: row.get(10)?,
            width: row.get(11)?,
            height: row.get(12)?,
            fps: row.get(13)?,
            interlaced: row.get::<_, Option<i64>>(18)?.map(|v| v != 0),
            is_hdr: row.get::<_, i64>(14)? != 0,
            audio_codec: row.get(19)?,
            audio_channels: row.get(20)?,
            measured_at: row.get(21)?,
            ok_count: row.get(15)?,
            fail_count: row.get(16)?,
            failed_at: row.get(22)?,
            fail_reason: row.get(23)?,
        }),
    };
    Ok(ChannelDto {
        stream_id: row.get(0)?,
        name: row.get(1)?,
        clean_name: row.get(2)?,
        num: row.get(3)?,
        logo_url: row.get(4)?,
        category_id: row.get(5)?,
        has_archive: row.get::<_, i64>(6)? != 0,
        archive_days: row
            .get::<_, Option<i64>>(7)?
            .and_then(|d| u32::try_from(d).ok()),
        is_separator: row.get::<_, i64>(8)? != 0,
        num_label: row.get(17)?,
        quality,
        // Filled in by the caller for the page it returns; joining the guide
        // to every listing would cost more than the UI needs.
        now_playing: None,
    })
}

fn programme_from_row(row: &Row) -> rusqlite::Result<(i64, ProgrammeDto)> {
    Ok((
        row.get(0)?,
        ProgrammeDto {
            start: row.get(1)?,
            stop: row.get(2)?,
            title: row.get(3)?,
            description: row.get(4)?,
        },
    ))
}

/// `?,?,?` for an `IN` list. SQLite has no array binding, and formatting the
/// ids into the SQL would be an injection seam even though they are `i64`.
fn placeholders(n: usize) -> String {
    let mut s = String::with_capacity(n * 2);
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        s.push('?');
    }
    s
}

/// `?3,?4,…` for an `IN` list that shares a statement with numbered
/// parameters. SQLite gives a bare `?` the index after the highest one seen so
/// far, which depends on where in the SQL text it appears — explicit indices
/// take that trap away.
fn numbered_placeholders(n: usize, start: usize) -> String {
    let mut s = String::with_capacity(n * 4);
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!("?{}", start + i));
    }
    s
}

/// Categories with a live channel count each: separators and soft-deleted
/// rows are excluded, so a category of nothing but headers reads as empty.
pub fn categories(conn: &Connection) -> Result<Vec<CategoryDto>> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.name,
                (SELECT COUNT(*) FROM channels ch
                  WHERE ch.category_id = c.id
                    AND ch.is_separator = 0
                    AND ch.deleted_at IS NULL)
         FROM categories c
         ORDER BY c.sort, c.name",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(CategoryDto {
            id: r.get(0)?,
            name: r.get(1)?,
            channel_count: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// One page of a category, or of everything when `category_id` is `None`.
///
/// Separators are returned so the UI can draw section headers; soft-deleted
/// rows never are. `now_playing` is left `None` — see [`now_playing`].
pub fn channels_in(
    conn: &Connection,
    category_id: Option<&str>,
    offset: u32,
    limit: u32,
) -> Result<Vec<ChannelDto>> {
    let sql = format!(
        "SELECT {CHANNEL_COLUMNS}
         FROM channels ch {QUALITY_JOIN}
         WHERE ch.deleted_at IS NULL AND (?1 IS NULL OR ch.category_id = ?1)
         ORDER BY ch.num IS NULL, ch.num, ch.name
         LIMIT ?2 OFFSET ?3"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params![category_id, limit as i64, offset as i64],
        channel_from_row,
    )?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Every channel number in the catalogue, as it is written, beside the
/// channel it tunes: `(label, stream_id)`, in the order the "All channels"
/// list draws them.
///
/// For tuning by number, remote-control style. The label is `num_label` when
/// there is one — an over-the-air subchannel's `5.1`, held in `num` as `5001`
/// — and `num` written out otherwise, because the label is what the viewer
/// sees beside the channel and so what they will type. Separators, deleted
/// rows and channels without a number are left out: none can be tuned by one.
///
/// Two channels can share a number (playlists do not promise otherwise); both
/// are returned, in list order, and the caller takes the first — the one the
/// list shows first, so typing the number agrees with looking it up.
pub fn channel_numbers(conn: &Connection) -> Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT COALESCE(NULLIF(TRIM(num_label), ''), CAST(num AS TEXT)), stream_id
         FROM channels
         WHERE deleted_at IS NULL AND is_separator = 0 AND num IS NOT NULL
         ORDER BY num, name",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Where a channel sits in [`channels_in`]'s listing of `category_id`: how
/// many rows come before it, separators included, since the list draws them.
/// `None` when the channel is not in that listing.
///
/// For taking a list or the guide straight to one channel without paging
/// through everything above it. The comparison spells out the listing's
/// `ORDER BY num IS NULL, num, name` term by term; channels that tie on all
/// three are in no defined order there, and this places a channel at the
/// first of its ties.
pub fn channel_index(
    conn: &Connection,
    category_id: Option<&str>,
    stream_id: i64,
) -> Result<Option<u32>> {
    let found = conn.query_row(
        "SELECT (SELECT COUNT(*) FROM channels ch
                  WHERE ch.deleted_at IS NULL
                    AND (?1 IS NULL OR ch.category_id = ?1)
                    AND ((ch.num IS NULL) < (t.num IS NULL)
                         OR ((ch.num IS NULL) = (t.num IS NULL)
                             AND (ch.num < t.num
                                  OR (ch.num IS t.num AND ch.name < t.name)))))
         FROM channels t
         WHERE t.stream_id = ?2
           AND t.deleted_at IS NULL
           AND (?1 IS NULL OR t.category_id = ?1)",
        rusqlite::params![category_id, stream_id],
        |r| r.get::<_, i64>(0),
    );
    match found {
        Ok(n) => Ok(u32::try_from(n).ok()),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Channels in the order the ids were given, because search hands back ranked
/// ids and SQLite would return them in whatever order it pleases. Ids with no
/// row are skipped rather than treated as an error.
pub fn channels_by_ids(conn: &Connection, ids: &[i64]) -> Result<Vec<ChannelDto>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT {CHANNEL_COLUMNS}
         FROM channels ch {QUALITY_JOIN}
         WHERE ch.stream_id IN ({})",
        placeholders(ids.len())
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(ids), channel_from_row)?;
    let mut by_id: HashMap<i64, ChannelDto> = HashMap::with_capacity(ids.len());
    for ch in rows {
        let ch = ch?;
        by_id.insert(ch.stream_id, ch);
    }
    Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
}

/// Programmes overlapping `[from, to)` for each channel, in the order the ids
/// were given. A channel with no guide yields an empty vec rather than
/// vanishing: only 15% of the catalogue carries EPG, and the guide grid still
/// has to line those rows up.
pub fn programmes(
    conn: &Connection,
    ids: &[i64],
    from: i64,
    to: i64,
) -> Result<Vec<(i64, Vec<ProgrammeDto>)>> {
    let mut buckets: HashMap<i64, Vec<ProgrammeDto>> = HashMap::with_capacity(ids.len());
    if !ids.is_empty() {
        let sql = format!(
            "SELECT ch.stream_id, e.start, e.stop, e.title, e.description
             FROM channels ch JOIN epg e ON e.channel_ref = ch.epg_channel_id
             WHERE ch.stream_id IN ({})
               AND e.start < ?{} AND e.stop > ?{}
             ORDER BY e.start",
            placeholders(ids.len()),
            ids.len() + 1,
            ids.len() + 2,
        );
        let mut stmt = conn.prepare(&sql)?;
        let bounds = [to, from];
        let args = rusqlite::params_from_iter(ids.iter().chain(bounds.iter()));
        for row in stmt.query_map(args, programme_from_row)? {
            let (id, p) = row?;
            buckets.entry(id).or_default().push(p);
        }
    }
    Ok(ids
        .iter()
        .map(|id| (*id, buckets.get(id).cloned().unwrap_or_default()))
        .collect())
}

/// The programme containing `at`, per channel. A gap in the guide yields no
/// entry at all — the nearest programme is not what is on.
pub fn now_playing(conn: &Connection, ids: &[i64], at: i64) -> Result<HashMap<i64, ProgrammeDto>> {
    let mut out = HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let sql = format!(
        "SELECT ch.stream_id, e.start, e.stop, e.title, e.description
         FROM channels ch JOIN epg e ON e.channel_ref = ch.epg_channel_id
         WHERE ch.stream_id IN ({})
           AND e.start <= ?{} AND e.stop > ?{}
         ORDER BY e.start",
        placeholders(ids.len()),
        ids.len() + 1,
        ids.len() + 2,
    );
    let mut stmt = conn.prepare(&sql)?;
    let bounds = [at, at];
    let args = rusqlite::params_from_iter(ids.iter().chain(bounds.iter()));
    for row in stmt.query_map(args, programme_from_row)? {
        let (id, p) = row?;
        // Overlapping guide entries happen; the one that started last is the
        // one on air.
        out.insert(id, p);
    }
    Ok(out)
}

/// Guide rows: each channel's header fields beside the programmes overlapping
/// `[from, to)`, in the order the ids were given.
///
/// One query rather than a channel fetch followed by [`programmes`]: the grid
/// asks for a screenful of rows at a time, and two passes over the same ids is
/// work the join already does.
///
/// A channel with no `epg_channel_id`, or none in the window, still gets a row
/// with an empty `programmes` — the left join is the point. Omitting it would
/// slide every row beneath it against the wrong channel in the grid. An id
/// with no channel row at all is skipped, as in [`channels_by_ids`]: there is
/// no name to draw.
pub fn guide_rows(conn: &Connection, ids: &[i64], from: i64, to: i64) -> Result<Vec<GuideRowDto>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    // The window lives in the join condition, not the WHERE clause: as a
    // filter it would drop the channel along with its absent programmes and
    // turn the left join back into an inner one.
    // Numbered throughout, and the bounds first. A bare `?` takes the index
    // after the highest seen so far, so mixing the two forms while the join
    // predicate precedes the `IN` list would silently renumber the ids.
    let sql = format!(
        "SELECT ch.stream_id, ch.name, ch.logo_url, ch.has_archive, ch.archive_days,
                e.start, e.stop, e.title, e.description
         FROM channels ch
         LEFT JOIN epg e
                ON e.channel_ref = ch.epg_channel_id
               AND e.start < ?1 AND e.stop > ?2
         WHERE ch.stream_id IN ({})
         ORDER BY e.start",
        numbered_placeholders(ids.len(), 3),
    );
    let mut stmt = conn.prepare(&sql)?;
    let bounds = [to, from];
    let args = rusqlite::params_from_iter(bounds.iter().chain(ids.iter()));
    let mut by_id: HashMap<i64, GuideRowDto> = HashMap::with_capacity(ids.len());
    let mut rows = stmt.query(args)?;
    while let Some(row) = rows.next()? {
        let stream_id: i64 = row.get(0)?;
        let entry = match by_id.entry(stream_id) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => e.insert(GuideRowDto {
                stream_id,
                name: row.get(1)?,
                logo_url: row.get(2)?,
                has_archive: row.get::<_, i64>(3)? != 0,
                archive_days: row
                    .get::<_, Option<i64>>(4)?
                    .and_then(|d| u32::try_from(d).ok()),
                programmes: Vec::new(),
            }),
        };
        // A NULL start is the left join saying "nothing in the window", which
        // is a row with no programmes rather than a programme.
        if let Some(start) = row.get::<_, Option<i64>>(5)? {
            entry.programmes.push(ProgrammeDto {
                start,
                stop: row.get(6)?,
                title: row.get(7)?,
                description: row.get(8)?,
            });
        }
    }
    Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
}

// ------------------------------------------------------- channel quality

/// What a few seconds of a live channel showed. Every field is optional:
/// mpv answers what it can, and a missing answer keeps the last one on
/// record rather than erasing it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reading {
    pub codec: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub fps: Option<f64>,
    pub interlaced: Option<bool>,
    pub is_hdr: bool,
    pub audio_codec: Option<String>,
    pub audio_channels: Option<i64>,
}

/// Records a measurement of a channel, as of `at`, for the catalogue on
/// record. Overwrites the last one: a provider that moves a channel to 1080p
/// should be believed the next time it is watched.
pub fn record_reading(conn: &Connection, stream_id: i64, r: &Reading, at: i64) -> Result<()> {
    conn.execute(
        concat!(
            "INSERT INTO channel_quality
                 (origin, stream_id, codec, width, height, fps, interlaced, is_hdr,
                  audio_codec, audio_channels, measured_at)
             VALUES (",
            current_origin_sql!(),
            ", ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(origin, stream_id) DO UPDATE SET
                 codec          = COALESCE(excluded.codec, channel_quality.codec),
                 width          = COALESCE(excluded.width, channel_quality.width),
                 height         = COALESCE(excluded.height, channel_quality.height),
                 fps            = COALESCE(excluded.fps, channel_quality.fps),
                 interlaced     = COALESCE(excluded.interlaced, channel_quality.interlaced),
                 is_hdr         = excluded.is_hdr,
                 audio_codec    = COALESCE(excluded.audio_codec, channel_quality.audio_codec),
                 audio_channels = COALESCE(excluded.audio_channels, channel_quality.audio_channels),
                 measured_at    = excluded.measured_at"
        ),
        rusqlite::params![
            stream_id,
            r.codec,
            r.width,
            r.height,
            r.fps,
            r.interlaced.map(i64::from),
            i64::from(r.is_hdr),
            r.audio_codec,
            r.audio_channels,
            at,
        ],
    )?;
    Ok(())
}

/// A live open of the channel reached a picture: one more success, and
/// whatever failure was on record is over.
pub fn record_loaded(conn: &Connection, stream_id: i64) -> Result<()> {
    conn.execute(
        concat!(
            "INSERT INTO channel_quality (origin, stream_id, ok_count) VALUES (",
            current_origin_sql!(),
            ", ?1, 1)
             ON CONFLICT(origin, stream_id) DO UPDATE SET
                 ok_count    = channel_quality.ok_count + 1,
                 failed_at   = NULL,
                 fail_reason = NULL"
        ),
        [stream_id],
    )?;
    Ok(())
}

/// A live open of the channel genuinely failed to load, for `reason`. What
/// counts is decided by `ipc::ending::against_channel`; this only writes.
pub fn record_failed(conn: &Connection, stream_id: i64, reason: &str, at: i64) -> Result<()> {
    conn.execute(
        concat!(
            "INSERT INTO channel_quality
                 (origin, stream_id, fail_count, failed_at, fail_reason)
             VALUES (",
            current_origin_sql!(),
            ", ?1, 1, ?2, ?3)
             ON CONFLICT(origin, stream_id) DO UPDATE SET
                 fail_count  = channel_quality.fail_count + 1,
                 failed_at   = excluded.failed_at,
                 fail_reason = excluded.fail_reason"
        ),
        rusqlite::params![stream_id, at, reason],
    )?;
    Ok(())
}

// ------------------------------------------------------------------ activity

/// Every channel with watch time against it, unordered.
///
/// Unordered on purpose: the ranking is recency-weighted and belongs in
/// [`crate::catalog::activity`], where it can be tested without a database.
/// This is only the join — and the place where the two rules that must never
/// be broken are enforced, because a favourite is something you *watched*:
/// separators are not playable and soft-deleted channels are not there any
/// more, so neither may appear however much time is against its id.
///
/// The activity row itself survives a sync by design (see
/// [`crate::catalog::sync`]), so a channel that comes back from a soft delete
/// brings its history with it.
pub fn watched(conn: &Connection) -> Result<Vec<FavouriteDto>> {
    let sql = format!(
        "SELECT {CHANNEL_COLUMNS}, a.watch_seconds, a.last_watched, a.open_count
         FROM activity a
         JOIN channels ch ON ch.stream_id = a.stream_id
         {QUALITY_JOIN}
         WHERE ch.deleted_at IS NULL
           AND ch.is_separator = 0
           AND a.watch_seconds > 0"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        Ok(FavouriteDto {
            channel: channel_from_row(row)?,
            watch_seconds: row.get(CHANNEL_COLUMN_COUNT)?,
            last_watched: row.get(CHANNEL_COLUMN_COUNT + 1)?,
            open_count: row.get(CHANNEL_COLUMN_COUNT + 2)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Adds watch time to a channel's running total.
///
/// An upsert rather than a read-modify-write: the player writes this from its
/// heartbeat and must not lose a flush to a race. `last_watched` takes the
/// later of the two values, so a stale write — or a clock that went backwards
/// — cannot make a channel look older than it is.
pub fn record_activity(
    conn: &Connection,
    stream_id: i64,
    seconds: i64,
    open: bool,
    at: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO activity (stream_id, watch_seconds, last_watched, open_count)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(stream_id) DO UPDATE SET
             watch_seconds = activity.watch_seconds + excluded.watch_seconds,
             last_watched  = MAX(COALESCE(activity.last_watched, 0), excluded.last_watched),
             open_count    = activity.open_count + excluded.open_count",
        rusqlite::params![stream_id, seconds.max(0), at, i64::from(open)],
    )?;
    Ok(())
}

/// Forgets one channel's history entirely.
///
/// Deletes rather than zeroing: a row of zeroes is indistinguishable from a
/// channel opened for the first time a moment ago, and would have to be
/// filtered everywhere instead of here. Forgetting something that was never
/// watched is not an error.
pub fn forget_activity(conn: &Connection, stream_id: i64) -> Result<()> {
    conn.execute("DELETE FROM activity WHERE stream_id = ?1", [stream_id])?;
    Ok(())
}

/// Every channel's watch history, for a backup. All of it, not only what
/// ranks as a favourite today: a channel gone from this catalogue, or long
/// decayed, may matter again on the machine the backup is restored to.
pub fn activity_rows(conn: &Connection) -> Result<Vec<ActivityRowDto>> {
    let mut stmt = conn.prepare(
        "SELECT a.stream_id,
                CASE WHEN ch.clean_name <> '' THEN ch.clean_name ELSE ch.name END,
                a.watch_seconds, a.last_watched, a.open_count
         FROM activity a
         LEFT JOIN channels ch ON ch.stream_id = a.stream_id
         ORDER BY a.watch_seconds DESC, a.stream_id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(ActivityRowDto {
            stream_id: r.get(0)?,
            name: r.get(1)?,
            watch_seconds: r.get(2)?,
            last_watched: r.get(3)?,
            open_count: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Folds a backup's watch history into this one. Returns the rows taken.
///
/// Per channel, the larger of each figure rather than their sum: the same
/// backup imported twice must not double anyone's hours, and restoring onto
/// a machine with its own history must not lose any of it. Negative figures,
/// which nothing here writes, are taken as zero. One transaction, so a
/// failure part way leaves the table as it was.
pub fn merge_activity(conn: &Connection, rows: &[ActivityRowDto]) -> Result<usize> {
    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO activity (stream_id, watch_seconds, last_watched, open_count)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(stream_id) DO UPDATE SET
                 watch_seconds = MAX(activity.watch_seconds, excluded.watch_seconds),
                 last_watched  = CASE
                     WHEN excluded.last_watched IS NULL THEN activity.last_watched
                     WHEN activity.last_watched IS NULL THEN excluded.last_watched
                     ELSE MAX(activity.last_watched, excluded.last_watched) END,
                 open_count    = MAX(activity.open_count, excluded.open_count)",
        )?;
        for r in rows {
            stmt.execute(rusqlite::params![
                r.stream_id,
                r.watch_seconds.max(0),
                r.last_watched,
                r.open_count.max(0)
            ])?;
        }
    }
    tx.commit()?;
    Ok(rows.len())
}

/// `(stream_id, name)` for the fuzzy index. The cleaned name is preferred:
/// the picker should not have to match around `|CA|` and `[FHD]`.
pub fn channel_names(conn: &Connection) -> Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT stream_id, CASE WHEN clean_name <> '' THEN clean_name ELSE name END
         FROM channels
         WHERE is_separator = 0 AND deleted_at IS NULL",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::open_in_memory;

    fn row(stream_id: i64, watch: i64, last: Option<i64>, opens: i64) -> ActivityRowDto {
        ActivityRowDto {
            stream_id,
            name: None,
            watch_seconds: watch,
            last_watched: last,
            open_count: opens,
        }
    }

    #[test]
    fn imported_history_merges_by_the_larger_figure_and_twice_is_once() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "One", None, Some(1));
        record_activity(&c, 1, 600, true, 1_000).unwrap();
        let backup = [row(1, 300, Some(5_000), 4), row(2, 90, None, 1), row(3, -5, Some(7), -1)];
        assert_eq!(merge_activity(&c, &backup).unwrap(), 3);
        assert_eq!(merge_activity(&c, &backup).unwrap(), 3);

        let got = activity_rows(&c).unwrap();
        let by = |id| got.iter().find(|r| r.stream_id == id).cloned().unwrap();
        // Kept the local 600 seconds, took the later date and the larger count.
        assert_eq!(by(1), ActivityRowDto { name: Some("One".into()), ..row(1, 600, Some(5_000), 4) });
        // A channel not in this catalogue is kept, nameless, for later.
        assert_eq!(by(2), row(2, 90, None, 1));
        assert_eq!(by(3), row(3, 0, Some(7), 0));
        // Ordered by watch time, the way a reader of the file would want it.
        assert_eq!(got.iter().map(|r| r.stream_id).collect::<Vec<_>>(), [1, 2, 3]);
    }

    #[test]
    fn a_dated_row_keeps_its_date_against_an_undated_import() {
        let c = open_in_memory().unwrap();
        record_activity(&c, 9, 60, true, 4_000).unwrap();
        merge_activity(&c, &[row(9, 10, None, 0)]).unwrap();
        assert_eq!(activity_rows(&c).unwrap()[0].last_watched, Some(4_000));
    }

    /// `num`/`cat` are the fields browse ordering turns on; the rest of the
    /// row is uninteresting to these tests and left at its default.
    fn chan(c: &Connection, id: i64, name: &str, cat: Option<&str>, num: Option<i64>) {
        c.execute(
            "INSERT INTO channels (stream_id, category_id, name, clean_name, num)
             VALUES (?1,?2,?3,?4,?5)",
            rusqlite::params![id, cat, name, name, num],
        )
        .unwrap();
    }

    fn separator(c: &Connection, id: i64, name: &str, cat: Option<&str>) {
        c.execute(
            "INSERT INTO channels (stream_id, category_id, name, clean_name, is_separator)
             VALUES (?1,?2,?3,?3,1)",
            rusqlite::params![id, cat, name],
        )
        .unwrap();
    }

    fn soft_delete(c: &Connection, id: i64) {
        c.execute("UPDATE channels SET deleted_at = 1 WHERE stream_id = ?1", [id])
            .unwrap();
    }

    fn category(c: &Connection, id: &str, name: &str, sort: i64) {
        c.execute(
            "INSERT INTO categories (id, name, sort) VALUES (?1,?2,?3)",
            rusqlite::params![id, name, sort],
        )
        .unwrap();
    }

    #[test]
    fn channel_numbers_are_written_as_the_list_writes_them() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "Twelve", None, Some(12));
        chan(&c, 2, "Subchannel", None, Some(5001));
        c.execute("UPDATE channels SET num_label = '5.1' WHERE stream_id = 2", [])
            .unwrap();
        chan(&c, 3, "Blank label", None, Some(7));
        c.execute("UPDATE channels SET num_label = '  ' WHERE stream_id = 3", [])
            .unwrap();
        chan(&c, 4, "Unnumbered", None, None);
        separator(&c, 5, "##### NEWS #####", None);
        chan(&c, 6, "Gone", None, Some(8));
        soft_delete(&c, 6);
        // A shared number: both come back, the list's first one first.
        chan(&c, 7, "B second", None, Some(12));
        chan(&c, 8, "A first", None, Some(12));

        let got = channel_numbers(&c).unwrap();
        assert_eq!(
            got,
            vec![
                ("7".to_owned(), 3),
                ("12".to_owned(), 8),
                ("12".to_owned(), 7),
                ("12".to_owned(), 1),
                ("5.1".to_owned(), 2),
            ]
        );
    }

    fn epg_row(c: &Connection, chan_ref: &str, start: i64, stop: i64, title: &str) {
        c.execute(
            "INSERT INTO epg (channel_ref, start, stop, title, description)
             VALUES (?1,?2,?3,?4,NULL)",
            rusqlite::params![chan_ref, start, stop, title],
        )
        .unwrap();
    }

    fn with_epg_id(c: &Connection, id: i64, epg_id: &str) {
        c.execute(
            "UPDATE channels SET epg_channel_id = ?2 WHERE stream_id = ?1",
            rusqlite::params![id, epg_id],
        )
        .unwrap();
    }

    #[test]
    fn category_counts_exclude_separators_and_deleted() {
        let c = open_in_memory().unwrap();
        category(&c, "1", "Sports", 0);
        chan(&c, 10, "Alive", Some("1"), Some(1));
        chan(&c, 11, "Gone", Some("1"), Some(2));
        soft_delete(&c, 11);
        separator(&c, 12, "###### SPORTS ######", Some("1"));

        let cats = categories(&c).unwrap();
        assert_eq!(cats.len(), 1);
        assert_eq!(cats[0].id, "1");
        assert_eq!(cats[0].name, "Sports");
        assert_eq!(cats[0].channel_count, 1);
    }

    #[test]
    fn channels_in_orders_by_num_then_name() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "Zed", Some("1"), Some(2));
        chan(&c, 2, "Alpha", Some("1"), Some(1));
        chan(&c, 3, "Beta", Some("1"), None);
        chan(&c, 4, "Aardvark", Some("1"), None);

        let got = channels_in(&c, Some("1"), 0, 10).unwrap();
        let names: Vec<&str> = got.iter().map(|ch| ch.name.as_str()).collect();
        // Numbered channels lead in numeric order; unnumbered fall to the
        // back alphabetically rather than in insertion order.
        assert_eq!(names, ["Alpha", "Zed", "Aardvark", "Beta"]);
    }

    #[test]
    fn channels_in_filters_by_category_and_pages() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "A", Some("1"), Some(1));
        chan(&c, 2, "B", Some("1"), Some(2));
        chan(&c, 3, "C", Some("2"), Some(3));

        assert_eq!(channels_in(&c, None, 0, 10).unwrap().len(), 3);
        assert_eq!(channels_in(&c, Some("1"), 0, 10).unwrap().len(), 2);
        let page = channels_in(&c, Some("1"), 1, 10).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].name, "B");
    }

    #[test]
    fn channels_in_hides_deleted_but_keeps_separators() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "Alive", Some("1"), Some(1));
        chan(&c, 2, "Gone", Some("1"), Some(2));
        soft_delete(&c, 2);
        separator(&c, 3, "###### HEADER ######", Some("1"));

        let got = channels_in(&c, Some("1"), 0, 10).unwrap();
        let names: Vec<&str> = got.iter().map(|ch| ch.name.as_str()).collect();
        assert!(!names.contains(&"Gone"), "deleted channel returned: {names:?}");
        assert!(
            got.iter().any(|ch| ch.is_separator),
            "separator must survive so the UI can draw the header: {names:?}"
        );
    }

    #[test]
    fn channels_by_ids_preserves_requested_order() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "A", Some("1"), Some(1));
        chan(&c, 2, "B", Some("1"), Some(2));
        chan(&c, 3, "C", Some("1"), Some(3));

        // Search hands back ranked ids; database order would throw the
        // ranking away.
        let got = channels_by_ids(&c, &[3, 1, 2]).unwrap();
        let ids: Vec<i64> = got.iter().map(|ch| ch.stream_id).collect();
        assert_eq!(ids, [3, 1, 2]);
    }

    #[test]
    fn channels_by_ids_skips_unknown_ids() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "A", Some("1"), Some(1));
        let got = channels_by_ids(&c, &[9999, 1]).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].stream_id, 1);
    }

    #[test]
    fn channels_by_ids_is_empty_for_no_ids() {
        let c = open_in_memory().unwrap();
        assert!(channels_by_ids(&c, &[]).unwrap().is_empty());
    }

    #[test]
    fn channel_without_quality_row_yields_none_not_an_error() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "Measured", Some("1"), Some(1));
        chan(&c, 2, "Never opened", Some("1"), Some(2));
        c.execute(
            "INSERT INTO channel_quality
                 (origin, stream_id, codec, width, height, fps, is_hdr, ok_count, fail_count)
             VALUES ('',1,'h264',1920,1080,59.94,0,3,1)",
            [],
        )
        .unwrap();

        let got = channels_in(&c, Some("1"), 0, 10).unwrap();
        let q = got[0].quality.as_ref().expect("measured channel has quality");
        assert_eq!(q.codec.as_deref(), Some("h264"));
        assert_eq!((q.width, q.height), (Some(1920), Some(1080)));
        assert_eq!((q.ok_count, q.fail_count), (3, 1));
        assert!(!q.is_hdr);
        // Unknown is not bad: most channels will never be opened.
        assert!(got[1].quality.is_none());
    }

    #[test]
    fn readings_are_remembered_and_follow_the_channel() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "News", Some("1"), Some(1));
        let first = Reading {
            codec: Some("mpeg2video".into()),
            width: Some(720),
            height: Some(576),
            fps: Some(25.0),
            interlaced: Some(true),
            is_hdr: false,
            audio_codec: Some("mp2".into()),
            audio_channels: Some(2),
        };
        record_reading(&c, 1, &first, 100).unwrap();
        let q = channels_by_ids(&c, &[1]).unwrap()[0].quality.clone().unwrap();
        assert_eq!((q.width, q.height, q.interlaced), (Some(720), Some(576), Some(true)));
        assert_eq!(q.audio_codec.as_deref(), Some("mp2"));
        assert_eq!(q.measured_at, Some(100));
        // Later the provider upgrades the channel; what mpv could not say
        // this time is kept from last time.
        let second = Reading {
            codec: Some("h264".into()),
            width: Some(1920),
            height: Some(1080),
            interlaced: Some(false),
            audio_channels: Some(6),
            ..Reading::default()
        };
        record_reading(&c, 1, &second, 200).unwrap();
        let q = channels_by_ids(&c, &[1]).unwrap()[0].quality.clone().unwrap();
        assert_eq!((q.width, q.height, q.interlaced), (Some(1920), Some(1080), Some(false)));
        assert_eq!(q.fps, Some(25.0));
        assert_eq!((q.audio_codec.as_deref(), q.audio_channels), (Some("mp2"), Some(6)));
        assert_eq!(q.measured_at, Some(200));
    }

    #[test]
    fn a_failure_is_remembered_until_a_play_works() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "Dead", Some("1"), Some(1));
        record_failed(&c, 1, "refused (HTTP 403)", 50).unwrap();
        record_failed(&c, 1, "no picture within 8s", 60).unwrap();
        let q = channels_by_ids(&c, &[1]).unwrap()[0].quality.clone().unwrap();
        assert_eq!((q.fail_count, q.ok_count), (2, 0));
        assert_eq!(q.failed_at, Some(60));
        assert_eq!(q.fail_reason.as_deref(), Some("no picture within 8s"));
        // Nothing was measured: a failure alone is not a resolution.
        assert_eq!(q.height, None);
        record_loaded(&c, 1).unwrap();
        let q = channels_by_ids(&c, &[1]).unwrap()[0].quality.clone().unwrap();
        assert_eq!((q.fail_count, q.ok_count), (2, 1));
        assert_eq!((q.failed_at, q.fail_reason), (None, None));
    }

    #[test]
    fn readings_belong_to_the_catalogue_they_were_taken_in() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "Same id, two providers", Some("1"), Some(1));
        crate::catalog::origin::record(&c, "xtream:aaaa").unwrap();
        record_failed(&c, 1, "refused (HTTP 403)", 50).unwrap();
        assert!(channels_by_ids(&c, &[1]).unwrap()[0].quality.is_some());
        // Another account's channel 1 knows nothing of the first's.
        crate::catalog::origin::record(&c, "xtream:bbbb").unwrap();
        assert!(channels_by_ids(&c, &[1]).unwrap()[0].quality.is_none());
        // And the first's comes back with it.
        crate::catalog::origin::record(&c, "xtream:aaaa").unwrap();
        let q = channels_by_ids(&c, &[1]).unwrap()[0].quality.clone().unwrap();
        assert_eq!(q.failed_at, Some(50));
    }

    #[test]
    fn programmes_include_straddling_and_exclude_outside() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "Northwind", Some("1"), Some(1));
        with_epg_id(&c, 1, "northwind.uk");
        epg_row(&c, "northwind.uk", 50, 150, "straddles the start");
        epg_row(&c, "northwind.uk", 150, 250, "inside");
        epg_row(&c, "northwind.uk", 250, 350, "straddles the end");
        epg_row(&c, "northwind.uk", 400, 500, "wholly after");
        epg_row(&c, "northwind.uk", 0, 40, "wholly before");

        let got = programmes(&c, &[1], 100, 300).unwrap();
        assert_eq!(got.len(), 1);
        let titles: Vec<&str> = got[0].1.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(titles, ["straddles the start", "inside", "straddles the end"]);
    }

    #[test]
    fn programmes_returns_an_entry_per_requested_channel() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "With guide", Some("1"), Some(1));
        chan(&c, 2, "No guide", Some("1"), Some(2));
        with_epg_id(&c, 1, "a");
        epg_row(&c, "a", 100, 200, "Show");

        // 85% of channels carry no EPG at all; the caller still wants a row
        // per channel so the guide grid lines up.
        let got = programmes(&c, &[2, 1], 0, 1000).unwrap();
        assert_eq!(got.iter().map(|(id, _)| *id).collect::<Vec<_>>(), [2, 1]);
        assert!(got[0].1.is_empty());
        assert_eq!(got[1].1.len(), 1);
    }

    #[test]
    fn now_playing_picks_the_containing_programme() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "Northwind", Some("1"), Some(1));
        with_epg_id(&c, 1, "northwind.uk");
        epg_row(&c, "northwind.uk", 0, 100, "earlier");
        epg_row(&c, "northwind.uk", 100, 200, "now");
        epg_row(&c, "northwind.uk", 200, 300, "next");

        let np = now_playing(&c, &[1], 150).unwrap();
        assert_eq!(np.get(&1).map(|p| p.title.as_str()), Some("now"));

        // A gap in the guide is not "the nearest programme"; it is nothing.
        let gap = now_playing(&c, &[1], 500).unwrap();
        assert!(gap.is_empty(), "expected no programme in a guide gap");
    }

    #[test]
    fn guide_rows_keep_the_requested_order() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "One", Some("1"), Some(1));
        chan(&c, 2, "Two", Some("1"), Some(2));
        chan(&c, 3, "Three", Some("1"), Some(3));

        let got = guide_rows(&c, &[3, 1, 2], 0, 1000).unwrap();
        assert_eq!(
            got.iter().map(|r| r.stream_id).collect::<Vec<_>>(),
            [3, 1, 2]
        );
        assert_eq!(got[0].name, "Three");
    }

    #[test]
    fn a_channel_with_no_guide_still_gets_a_row() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "With guide", Some("1"), Some(1));
        chan(&c, 2, "No epg id", Some("1"), Some(2));
        chan(&c, 3, "Epg id, nothing in the window", Some("1"), Some(3));
        with_epg_id(&c, 1, "a");
        with_epg_id(&c, 3, "c");
        epg_row(&c, "a", 100, 200, "Show");
        epg_row(&c, "c", 5000, 6000, "Much later");

        // Drop a row and every row beneath it lines up against the wrong
        // channel in the grid.
        let got = guide_rows(&c, &[1, 2, 3], 0, 1000).unwrap();
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].programmes.len(), 1);
        assert!(got[1].programmes.is_empty());
        assert!(got[2].programmes.is_empty());
    }

    #[test]
    fn guide_rows_clip_to_the_window_and_keep_the_straddlers() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "Northwind", Some("1"), Some(1));
        with_epg_id(&c, 1, "northwind.uk");
        epg_row(&c, "northwind.uk", 0, 40, "wholly before");
        epg_row(&c, "northwind.uk", 50, 150, "straddles the start");
        epg_row(&c, "northwind.uk", 150, 250, "inside");
        epg_row(&c, "northwind.uk", 250, 350, "straddles the end");
        epg_row(&c, "northwind.uk", 400, 500, "wholly after");
        // Abutting either edge is outside: the window is half open, and a
        // programme that ends as it opens is not in it.
        epg_row(&c, "northwind.uk", 20, 100, "ends at the start");
        epg_row(&c, "northwind.uk", 300, 400, "begins at the end");

        let got = guide_rows(&c, &[1], 100, 300).unwrap();
        let titles: Vec<&str> = got[0].programmes.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(
            titles,
            ["straddles the start", "inside", "straddles the end"]
        );
    }

    #[test]
    fn guide_rows_carry_the_channels_header_fields() {
        let c = open_in_memory().unwrap();
        c.execute(
            "INSERT INTO channels (stream_id, category_id, name, clean_name, num,
                                   logo_url, has_archive, archive_days)
             VALUES (1,'1','Northwind One','Northwind One',1,'http://logo/1.png',1,7)",
            [],
        )
        .unwrap();

        let got = guide_rows(&c, &[1], 0, 1000).unwrap();
        assert_eq!(got[0].name, "Northwind One");
        assert_eq!(got[0].logo_url.as_deref(), Some("http://logo/1.png"));
        assert!(got[0].has_archive);
        assert_eq!(got[0].archive_days, Some(7));
    }

    #[test]
    fn guide_rows_skip_ids_with_no_channel_and_handle_none() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "Real", Some("1"), Some(1));
        let got = guide_rows(&c, &[9999, 1], 0, 1000).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].stream_id, 1);
        assert!(guide_rows(&c, &[], 0, 1000).unwrap().is_empty());
    }

    #[test]
    fn watched_carries_the_channel_and_its_totals() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "Northwind One", Some("1"), Some(1));
        record_activity(&c, 1, 600, true, 1_000).unwrap();
        record_activity(&c, 1, 300, false, 2_000).unwrap();

        let got = watched(&c).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].channel.name, "Northwind One");
        // Accumulated, not replaced.
        assert_eq!(got[0].watch_seconds, 900);
        assert_eq!(got[0].last_watched, Some(2_000));
        // Counted once, because only one of the two writes was an open.
        assert_eq!(got[0].open_count, 1);
    }

    #[test]
    fn record_activity_never_moves_last_watched_backwards() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "A", Some("1"), Some(1));
        record_activity(&c, 1, 60, true, 5_000).unwrap();
        // A clock that jumped back, or a write that arrived late.
        record_activity(&c, 1, 60, false, 1_000).unwrap();
        let got = watched(&c).unwrap();
        assert_eq!(got[0].last_watched, Some(5_000));
        assert_eq!(got[0].watch_seconds, 120);
    }

    #[test]
    fn watched_never_returns_a_separator_or_a_deleted_channel() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "Alive", Some("1"), Some(1));
        chan(&c, 2, "Gone", Some("1"), Some(2));
        separator(&c, 3, "###### HEADER ######", Some("1"));
        soft_delete(&c, 2);
        // Time against all three, which is exactly the case the filter is for:
        // the row outlives the channel by design.
        for id in [1, 2, 3] {
            record_activity(&c, id, 3600, true, 1_000).unwrap();
        }

        let ids: Vec<i64> = watched(&c)
            .unwrap()
            .iter()
            .map(|f| f.channel.stream_id)
            .collect();
        assert_eq!(ids, [1]);
    }

    #[test]
    fn watched_ignores_a_row_with_no_time_on_it() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "A", Some("1"), Some(1));
        record_activity(&c, 1, 0, false, 1_000).unwrap();
        assert!(watched(&c).unwrap().is_empty());
    }

    #[test]
    fn forget_activity_removes_the_history_and_nothing_else() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "Left playing by accident", Some("1"), Some(1));
        chan(&c, 2, "Real favourite", Some("1"), Some(2));
        record_activity(&c, 1, 40_000, true, 1_000).unwrap();
        record_activity(&c, 2, 600, true, 1_000).unwrap();

        forget_activity(&c, 1).unwrap();
        let ids: Vec<i64> = watched(&c)
            .unwrap()
            .iter()
            .map(|f| f.channel.stream_id)
            .collect();
        assert_eq!(ids, [2]);
        // The channel itself is untouched; only its history is gone.
        assert_eq!(channels_by_ids(&c, &[1]).unwrap().len(), 1);
        // And forgetting something unknown is a no-op, not a failure.
        forget_activity(&c, 9999).unwrap();
    }

    #[test]
    fn channel_index_agrees_with_the_listing() {
        let c = open_in_memory().unwrap();
        // Out of order on purpose, with a numberless tail, a tie on number
        // broken by name, a separator, and a deleted row that must not count.
        chan(&c, 1, "Zed", Some("1"), None);
        chan(&c, 2, "Beta", Some("1"), Some(5));
        chan(&c, 3, "Alpha", Some("1"), Some(5));
        separator(&c, 4, "###### NEWS ######", Some("1"));
        chan(&c, 5, "Gone", Some("1"), Some(1));
        soft_delete(&c, 5);
        chan(&c, 6, "First", Some("1"), Some(2));
        chan(&c, 7, "Elsewhere", Some("2"), Some(1));
        chan(&c, 8, "Able", Some("1"), None);

        for category in [Some("1"), None] {
            let listing = channels_in(&c, category, 0, 100).unwrap();
            for (i, ch) in listing.iter().enumerate() {
                assert_eq!(
                    channel_index(&c, category, ch.stream_id).unwrap(),
                    Some(i as u32),
                    "{} in {category:?}",
                    ch.name
                );
            }
        }
        assert_eq!(channel_index(&c, Some("1"), 7).unwrap(), None);
        assert_eq!(channel_index(&c, Some("1"), 5).unwrap(), None);
        assert_eq!(channel_index(&c, None, 999).unwrap(), None);
    }

    #[test]
    fn channel_names_exclude_separators_and_deleted() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "Alive", Some("1"), Some(1));
        chan(&c, 2, "Gone", Some("1"), Some(2));
        soft_delete(&c, 2);
        separator(&c, 3, "###### HEADER ######", Some("1"));

        let names = channel_names(&c).unwrap();
        assert_eq!(names, vec![(1, "Alive".to_string())]);
    }
}
