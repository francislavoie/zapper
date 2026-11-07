//! Keeping the guide to what can still be watched.
//!
//! Every guide sync upserts what the provider sends, and nothing ever took a
//! programme out again: a mirror synced every six hours for a year held a
//! year of listings, every row of it in the programme-search index, for a
//! guide that shows three hours and a catch-up archive that reaches back a
//! week at most.
//!
//! A finished programme is worth keeping only while something can be done
//! with it. On a channel with catch-up that is as long as its archive
//! reaches; search offers it as a replay, and the guide plays it. Anywhere
//! else it is only "what was just on", for which a day is plenty. So
//! [`prune_guide`] gives each guide channel the longest window any live
//! channel using it needs — the HD and 4K copies of a network share one
//! guide channel, and the one with an archive decides — and drops the
//! finished programmes older than that. Nothing still airing or still to
//! come is touched, whatever its start.
//!
//! Deletions go through `epg`'s own delete trigger, which queues what the
//! search index has to forget (see `guide_search`), and the index is caught
//! up inside the same transaction: a pruned programme stops being a search
//! hit the moment the prune commits, never a moment later.
//!
//! **The file.** SQLite does not give freed pages back to the filesystem; it
//! reuses them. Pruning on every sync means each sync frees roughly what the
//! next one writes, so the file stops growing and stays at the size of the
//! window — no vacuum needed for that. What does leave a lot of free space
//! is the first prune of a mirror that was never pruned (a year of guide,
//! gone at once) or a switch to a smaller account. For those, [`maintain`]
//! runs a full `VACUUM` at startup, only when most of the file is free
//! space, and at most once a week. See [`worth_vacuuming`].
//!
//! Not `auto_vacuum = INCREMENTAL`. It would hand pages back without a full
//! rewrite, but turning it on for an existing mirror *takes* a full `VACUUM`
//! anyway, it leaves the file fragmented in exactly the table that is
//! scanned most, and the steady state frees nothing it would hand back. A
//! rare full vacuum fits how the space is actually freed: once, in bulk.

use anyhow::{Context, Result};
use rusqlite::Connection;

/// The furthest back anything is kept. Catch-up archives longer than two
/// weeks are rare, and the guide's own search would otherwise carry every
/// programme a server with no stated limit has ever sent.
pub const MAX_KEEP_DAYS: i64 = 14;

/// What is kept of a channel with no catch-up: enough for "what was just
/// on", the programme that finished while the viewer was away.
pub const NO_ARCHIVE_KEEP_DAYS: i64 = 1;

const DAY: i64 = 86_400;

/// How many days of finished programmes one channel needs.
///
/// An archive with no stated length is treated as the longest kept, which
/// is also what search assumes of it (it offers such an archive's every
/// programme). One of zero days is still an archive, and gets a day.
pub fn keep_days(has_archive: bool, archive_days: Option<i64>) -> i64 {
    if !has_archive {
        return NO_ARCHIVE_KEEP_DAYS;
    }
    archive_days
        .unwrap_or(MAX_KEEP_DAYS)
        .clamp(NO_ARCHIVE_KEEP_DAYS, MAX_KEEP_DAYS)
}

/// What one prune did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Pruned {
    /// Programmes deleted.
    pub programmes: usize,
    /// Guide channels looked at.
    pub channels: usize,
}

/// Deletes the finished programmes no channel can use any more, relative to
/// the instant `now` (Unix seconds), and brings the search index up to date.
///
/// One transaction — a savepoint, so it nests inside a caller's — so a
/// prune that fails leaves the guide and its index exactly as they were.
///
/// The per-channel windows are worked out once into a temporary table
/// (the `CASE` is [`keep_days`] in SQL), and the delete then walks `epg`'s
/// primary key, `(channel_ref, start)`, one range per guide channel.
/// "Finished" is the guide's own test: `stop` has passed, and a missing
/// `stop` (stored as 0) counts as finished once the programme has started,
/// as `guide_search` reads it.
pub fn prune_guide(conn: &Connection, now: i64) -> Result<Pruned> {
    conn.execute_batch("SAVEPOINT prune_guide")?;
    match prune_inside(conn, now) {
        Ok(p) => {
            conn.execute_batch("RELEASE prune_guide")?;
            Ok(p)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO prune_guide; RELEASE prune_guide");
            Err(e).context("pruning the guide")
        }
    }
}

fn prune_inside(conn: &Connection, now: i64) -> Result<Pruned> {
    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS guide_keep (
             ref    TEXT PRIMARY KEY,
             cutoff INTEGER NOT NULL
         ) WITHOUT ROWID;
         DELETE FROM temp.guide_keep;",
    )?;
    // `MAX` over every live channel sharing the guide channel; a guide
    // channel nothing points at (or only deleted channels do) gets the
    // shortest window.
    let channels = conn.execute(
        &format!(
            "INSERT INTO temp.guide_keep (ref, cutoff)
             SELECT r.channel_ref,
                    ?1 - {DAY} * COALESCE(MAX(CASE
                        WHEN ch.has_archive <> 0
                        THEN MIN(MAX(COALESCE(ch.archive_days, {MAX_KEEP_DAYS}), {NO_ARCHIVE_KEEP_DAYS}), {MAX_KEEP_DAYS})
                        ELSE {NO_ARCHIVE_KEEP_DAYS} END), {NO_ARCHIVE_KEEP_DAYS})
             FROM (SELECT DISTINCT channel_ref FROM epg) r
             LEFT JOIN channels ch
                    ON ch.epg_channel_id = r.channel_ref AND ch.deleted_at IS NULL
             GROUP BY r.channel_ref"
        ),
        [now],
    )?;
    let programmes = conn.execute(
        "DELETE FROM epg WHERE rowid IN (
             SELECT e.rowid
             FROM temp.guide_keep k
             JOIN epg e ON e.channel_ref = k.ref AND e.start < k.cutoff
             WHERE e.stop <= ?1
         )",
        [now],
    )?;
    conn.execute("DELETE FROM temp.guide_keep", [])?;
    if programmes > 0 {
        crate::catalog::guide_search::index_changes(conn)?;
    }
    Ok(Pruned {
        programmes,
        channels,
    })
}

/// Free space below which a vacuum is not worth the rewrite.
const VACUUM_MIN_FREE_BYTES: i64 = 64 * 1024 * 1024;
/// The share of the file that has to be free.
const VACUUM_MIN_FREE_SHARE: f64 = 0.4;
/// The least time between two vacuums.
const VACUUM_EVERY: i64 = 7 * DAY;

/// Where the last vacuum is remembered, in `meta`.
const VACUUMED_AT: &str = "guide_vacuumed_at";

/// Whether the file has enough free space in it to be worth rewriting.
///
/// All three, because a vacuum rewrites the whole file and blocks the
/// catalogue while it does: a lot of space in absolute terms (a small
/// mirror's free pages are not worth anybody's second), most of the file
/// (a big mirror with a little slack will reuse it on the next sync), and
/// not again within a week (a mirror that keeps freeing that much is
/// growing and shrinking with its provider, and rewriting it each time
/// would only churn the disk).
pub fn worth_vacuuming(
    page_count: i64,
    freelist_count: i64,
    page_size: i64,
    last_vacuum: Option<i64>,
    now: i64,
) -> bool {
    if page_count <= 0 || freelist_count <= 0 || page_size <= 0 {
        return false;
    }
    let free = freelist_count.saturating_mul(page_size);
    let share = freelist_count as f64 / page_count as f64;
    let due = last_vacuum.is_none_or(|t| now - t >= VACUUM_EVERY);
    free >= VACUUM_MIN_FREE_BYTES && share >= VACUUM_MIN_FREE_SHARE && due
}

/// Rewrites the file if [`worth_vacuuming`] says so. Returns whether it did.
///
/// `VACUUM` may renumber `epg`'s implicit rowids, which the search index is
/// keyed by, so the index is rebuilt straight after. `conn` must be the
/// connection the interface reads through: it is held for both steps, so
/// no search can run between them and find the index pointing at the wrong
/// programmes. A vacuum cannot run inside a transaction, and needs the
/// database to itself; if another connection is writing it fails busy,
/// which is logged by the caller and tried again at the next start.
pub fn vacuum_if_worthwhile(conn: &Connection, now: i64) -> Result<bool> {
    let pragma = |name: &str| -> Result<i64> {
        Ok(conn.query_row(&format!("PRAGMA {name}"), [], |r| r.get(0))?)
    };
    let (pages, free, size) = (pragma("page_count")?, pragma("freelist_count")?, pragma("page_size")?);
    let last: Option<i64> = conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [VACUUMED_AT], |r| {
            r.get::<_, String>(0)
        })
        .ok()
        .and_then(|v| v.parse().ok());
    if !worth_vacuuming(pages, free, size, last, now) {
        return Ok(false);
    }
    tracing::info!(
        "the mirror is {}% free space ({} MB); vacuuming",
        free * 100 / pages,
        free * size / 1_000_000
    );
    conn.execute_batch("VACUUM").context("vacuuming the mirror")?;
    crate::catalog::guide_search::rebuild(conn)
        .context("rebuilding the programme search after a vacuum")?;
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![VACUUMED_AT, now.to_string()],
    )?;
    Ok(true)
}

/// Prunes the guide through `conn` and says so in the log. Never fails:
/// a guide that could not be pruned is a slightly larger guide, not a
/// reason to fail the sync that just landed it.
pub fn prune_and_log(conn: &Connection, now: i64) {
    let started = std::time::Instant::now();
    match prune_guide(conn, now) {
        Ok(p) if p.programmes > 0 => tracing::info!(
            "pruned {} past programmes across {} guide channels in {:.0?}",
            p.programmes,
            p.channels,
            started.elapsed()
        ),
        Ok(_) => {}
        Err(e) => tracing::warn!("could not prune the guide: {e:#}"),
    }
}

/// Startup housekeeping: the prune a sync would do, for a mirror that may
/// not sync for hours, then the rare vacuum. Blocking; run it off the main
/// thread, before the first sync, so the vacuum does not meet a sync's
/// writes.
pub fn maintain(now: i64) {
    if let Err(e) = crate::catalog::db::with_write_db(|c| {
        prune_and_log(c, now);
        Ok(())
    }) {
        tracing::warn!("could not open the mirror to prune it: {e:#}");
        return;
    }
    // Through the interface's connection, so searches wait for the rebuild.
    match crate::catalog::db::with_db(|c| vacuum_if_worthwhile(c, now)) {
        Ok(true) => tracing::info!("the mirror was vacuumed"),
        Ok(false) => {}
        Err(e) => tracing::warn!("the mirror was not vacuumed: {e:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::guide_search;
    use crate::catalog::open_in_memory;

    /// 2026-09-08 12:00 UTC.
    const NOW: i64 = 1_788_868_800;
    const HOUR: i64 = 3600;

    fn chan(c: &Connection, id: i64, epg: &str, has_archive: bool, days: Option<i64>) {
        c.execute(
            "INSERT INTO channels (stream_id, category_id, name, clean_name, num, epg_channel_id,
                                   has_archive, archive_days)
             VALUES (?1, 'cat', ?2, ?2, ?1, ?3, ?4, ?5)",
            rusqlite::params![id, format!("ch{id}"), epg, has_archive, days],
        )
        .unwrap();
    }

    fn prog(c: &Connection, r: &str, start: i64, stop: i64, title: &str) {
        c.execute(
            "INSERT INTO epg (channel_ref, start, stop, title) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![r, start, stop, title],
        )
        .unwrap();
    }

    /// An hour-long programme that started `days_ago` days back.
    fn past(c: &Connection, r: &str, days_ago: f64) {
        let start = NOW - (days_ago * DAY as f64) as i64;
        prog(c, r, start, start + HOUR, &format!("{r} {days_ago}"));
    }

    fn starts(c: &Connection, r: &str) -> Vec<i64> {
        let mut s = c
            .prepare("SELECT start FROM epg WHERE channel_ref = ?1 ORDER BY start")
            .unwrap();
        s.query_map([r], |row| row.get(0)).unwrap().map(Result::unwrap).collect()
    }

    fn days_kept(c: &Connection, r: &str) -> Vec<i64> {
        starts(c, r).into_iter().map(|s| (NOW - s + DAY / 2) / DAY).collect()
    }

    fn with_index_after_writes(c: &Connection) {
        guide_search::index_changes(c).unwrap();
    }

    #[test]
    fn the_window_follows_the_archive() {
        assert_eq!(keep_days(false, None), 1);
        assert_eq!(keep_days(false, Some(7)), 1, "no archive, whatever the length says");
        assert_eq!(keep_days(true, Some(3)), 3);
        assert_eq!(keep_days(true, Some(7)), 7);
        assert_eq!(keep_days(true, Some(30)), MAX_KEEP_DAYS, "capped");
        assert_eq!(keep_days(true, None), MAX_KEEP_DAYS, "unstated is the longest kept");
        assert_eq!(keep_days(true, Some(0)), 1, "a zero-day archive is still a day");
    }

    /// One programme each 0.5, 1.5, 2.5 … 20.5 days back on `r`.
    fn history(c: &Connection, r: &str) {
        for d in 0..=20 {
            past(c, r, f64::from(d) + 0.5);
        }
    }

    #[test]
    fn each_guide_channel_keeps_what_its_channels_can_replay() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "three", true, Some(3));
        chan(&c, 2, "unstated", true, None);
        chan(&c, 3, "none", false, None);
        chan(&c, 4, "month", true, Some(30));
        for r in ["three", "unstated", "none", "month", "nobody"] {
            history(&c, r);
        }
        with_index_after_writes(&c);
        let p = prune_guide(&c, NOW).unwrap();
        assert_eq!(p.channels, 5);

        let upto = |n: i64| (0..n).map(|d| d + 1).rev().collect::<Vec<_>>();
        // Rounded day counts of what is left: d + 0.5 days back rounds to d + 1.
        assert_eq!(days_kept(&c, "three"), upto(3), "a three-day archive");
        assert_eq!(days_kept(&c, "unstated"), upto(14), "an archive of no stated length");
        assert_eq!(days_kept(&c, "month"), upto(14), "a long archive, capped");
        assert_eq!(days_kept(&c, "none"), upto(1), "no archive: what was just on");
        assert_eq!(days_kept(&c, "nobody"), upto(1), "a guide channel nothing uses");
    }

    #[test]
    fn the_longest_archive_on_a_shared_guide_channel_decides() {
        let c = open_in_memory().unwrap();
        // The HD copy has no archive, the 4K copy a week's.
        chan(&c, 1, "network", false, None);
        chan(&c, 2, "network", true, Some(7));
        // A deleted channel's archive does not count.
        chan(&c, 3, "gone", true, Some(7));
        c.execute("UPDATE channels SET deleted_at = 1 WHERE stream_id = 3", []).unwrap();
        history(&c, "network");
        history(&c, "gone");
        prune_guide(&c, NOW).unwrap();
        assert_eq!(starts(&c, "network").len(), 7);
        assert_eq!(starts(&c, "gone").len(), 1);
    }

    #[test]
    fn nothing_airing_or_to_come_is_touched() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "a", false, None);
        // Started three days ago and still going: a marathon, or a guide
        // that lists a whole day as one block.
        prog(&c, "a", NOW - 3 * DAY, NOW + HOUR, "still on");
        prog(&c, "a", NOW + HOUR, NOW + 2 * HOUR, "next");
        prog(&c, "a", NOW + 6 * DAY, NOW + 6 * DAY + HOUR, "next week");
        // No stop, and long started: finished, as the guide reads it.
        prog(&c, "a", NOW - 3 * DAY - HOUR, 0, "no stop");
        // No stop, and not started yet: to come.
        prog(&c, "a", NOW + 3 * HOUR, 0, "no stop, later");
        prune_guide(&c, NOW).unwrap();
        let titles: Vec<String> = c
            .prepare("SELECT title FROM epg ORDER BY start")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(titles, ["still on", "next", "no stop, later", "next week"]);
    }

    #[test]
    fn exactly_the_archive_is_kept_as_search_counts_it() {
        // The archive holds what started since `now - days * 86400` (search
        // offers it from `guide_search::ARCHIVE_MARGIN` inside that); the
        // prune keeps every one of those and nothing earlier.
        let c = open_in_memory().unwrap();
        chan(&c, 1, "a", true, Some(2));
        prog(&c, "a", NOW - 2 * DAY, NOW - 2 * DAY + HOUR, "on the line");
        prog(&c, "a", NOW - 2 * DAY - 1, NOW - 2 * DAY + HOUR - 1, "a second older");
        prune_guide(&c, NOW).unwrap();
        assert_eq!(starts(&c, "a"), [NOW - 2 * DAY]);
    }

    #[test]
    fn search_forgets_what_was_pruned_and_finds_what_was_kept() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "a", true, Some(3));
        prog(&c, "a", NOW - 10 * DAY, NOW - 10 * DAY + HOUR, "Harbour Lights old");
        prog(&c, "a", NOW - DAY, NOW - DAY + HOUR, "Harbour Lights recent");
        prog(&c, "a", NOW + HOUR, NOW + 2 * HOUR, "Harbour Lights next");
        with_index_after_writes(&c);
        // Indexed before the prune: the old one is a search term too.
        let n: i64 = c
            .query_row("SELECT COUNT(*) FROM epg_fts WHERE epg_fts MATCH 'old'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);

        let p = prune_guide(&c, NOW).unwrap();
        assert_eq!(p.programmes, 1);
        let stale: i64 = c.query_row("SELECT COUNT(*) FROM epg_fts_stale", [], |r| r.get(0)).unwrap();
        assert_eq!(stale, 0, "the queue is drained inside the prune");
        let n: i64 = c
            .query_row("SELECT COUNT(*) FROM epg_fts WHERE epg_fts MATCH 'old'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "the pruned title is gone from the index");
        let titles: Vec<String> = guide_search::search(&c, "harbour", NOW, 0, 10)
            .unwrap()
            .into_iter()
            .map(|h| h.programme.title)
            .collect();
        assert_eq!(titles, ["Harbour Lights next", "Harbour Lights recent"]);
        // The index agrees with the table, term for term.
        c.execute_batch("INSERT INTO epg_fts (epg_fts, rank) VALUES ('integrity-check', 1)")
            .unwrap();
    }

    #[test]
    fn rows_written_after_a_prune_are_still_indexed() {
        // Deleting the newest rows lowers the index's watermark; a row that
        // reuses a freed rowid must still be found.
        let c = open_in_memory().unwrap();
        chan(&c, 1, "a", false, None);
        prog(&c, "a", NOW - 5 * DAY, NOW - 5 * DAY + HOUR, "Gone");
        with_index_after_writes(&c);
        prune_guide(&c, NOW).unwrap();
        prog(&c, "a", NOW + HOUR, NOW + 2 * HOUR, "Fresh");
        with_index_after_writes(&c);
        assert_eq!(guide_search::search(&c, "fresh", NOW, 0, 10).unwrap().len(), 1);
        assert!(guide_search::search(&c, "gone", NOW, 0, 10).unwrap().is_empty());
        c.execute_batch("INSERT INTO epg_fts (epg_fts, rank) VALUES ('integrity-check', 1)")
            .unwrap();
    }

    #[test]
    fn pruning_twice_is_pruning_once() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "a", true, Some(3));
        history(&c, "a");
        let first = prune_guide(&c, NOW).unwrap();
        assert!(first.programmes > 0);
        let again = prune_guide(&c, NOW).unwrap();
        assert_eq!(again.programmes, 0);
    }

    #[test]
    fn a_prune_nests_inside_a_callers_transaction() {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "a", false, None);
        history(&c, "a");
        c.execute_batch("BEGIN").unwrap();
        prune_guide(&c, NOW).unwrap();
        c.execute_batch("ROLLBACK").unwrap();
        assert_eq!(starts(&c, "a").len(), 21, "rolled back with the caller");
    }

    #[test]
    fn an_empty_guide_prunes_nothing() {
        let c = open_in_memory().unwrap();
        assert_eq!(prune_guide(&c, NOW).unwrap(), Pruned::default());
    }

    const MB: i64 = 1024 * 1024;

    #[test]
    fn a_vacuum_needs_a_lot_of_free_space_most_of_the_file_and_a_week() {
        let page = 4096;
        let pages = |mb: i64| mb * MB / page;
        // 300 MB file, 200 MB free, never vacuumed: yes.
        assert!(worth_vacuuming(pages(300), pages(200), page, None, NOW));
        // The same, vacuumed yesterday: not again so soon.
        assert!(!worth_vacuuming(pages(300), pages(200), page, Some(NOW - DAY), NOW));
        assert!(worth_vacuuming(pages(300), pages(200), page, Some(NOW - 8 * DAY), NOW));
        // Mostly free but small: not worth anybody's second.
        assert!(!worth_vacuuming(pages(40), pages(30), page, None, NOW));
        // A lot free but a small share of a big file: the next sync reuses it.
        assert!(!worth_vacuuming(pages(1000), pages(100), page, None, NOW));
        // Nothing free, or nonsense.
        assert!(!worth_vacuuming(pages(300), 0, page, None, NOW));
        assert!(!worth_vacuuming(0, 0, 0, None, NOW));
    }

    #[test]
    fn a_vacuum_leaves_search_pointing_at_the_right_programmes() {
        // On a file, as the app's is: VACUUM renumbers `epg`'s rowids when
        // rows before the end were deleted, and only the rebuild after it
        // keeps the index honest.
        let dir = std::env::temp_dir().join(format!("zapper-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let c = crate::catalog::open_at(&dir.join("m.db")).unwrap();
        chan(&c, 1, "a", false, None);
        c.execute_batch("BEGIN").unwrap();
        for i in 0..20_000 {
            let start = NOW - 30 * DAY + i * 60;
            prog(&c, "a", start, start + 60, &format!("Filler {i}"));
        }
        prog(&c, "a", NOW + HOUR, NOW + 2 * HOUR, "Harbour Lights");
        c.execute_batch("COMMIT").unwrap();
        with_index_after_writes(&c);
        let p = prune_guide(&c, NOW).unwrap();
        assert!(p.programmes >= 18_000);
        // Small, so the size threshold would refuse it; vacuum directly
        // through the same steps the threshold guards.
        c.execute_batch("VACUUM").unwrap();
        guide_search::rebuild(&c).unwrap();
        let hits = guide_search::search(&c, "harbour", NOW, 0, 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].programme.title, "Harbour Lights");
        c.execute_batch("INSERT INTO epg_fts (epg_fts, rank) VALUES ('integrity-check', 1)")
            .unwrap();
        // And the gate itself says no to a file this small.
        assert!(!vacuum_if_worthwhile(&c, NOW).unwrap());
        drop(c);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Timings on a guide the size of a large provider's, printed rather
    /// than asserted: `cargo test --lib prune::tests::bench -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench() {
        for (label, channels, back_days, ahead_days) in [
            ("steady: 2000 ch, 14d back + 7d ahead", 2000, 14, 7),
            ("first prune: 2000 ch, 60d back + 7d ahead", 2000, 60, 7),
        ] {
            let dir = std::env::temp_dir().join(format!("zapper-prune-bench-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let c = crate::catalog::open_at(&dir.join("m.db")).unwrap();
            // A third with a week's archive, a third with three days, a
            // third with none.
            for id in 0..channels {
                let (arch, days) = match id % 3 {
                    0 => (true, Some(7)),
                    1 => (true, Some(3)),
                    _ => (false, None),
                };
                chan(&c, id, &format!("g{id}"), arch, days);
            }
            // A programme every 2h: 12 a day per channel.
            let t = std::time::Instant::now();
            c.execute_batch("BEGIN").unwrap();
            {
                let mut s = c
                    .prepare("INSERT INTO epg (channel_ref, start, stop, title) VALUES (?1, ?2, ?3, ?4)")
                    .unwrap();
                for id in 0..channels {
                    let r = format!("g{id}");
                    let mut start = NOW - back_days * DAY;
                    while start < NOW + ahead_days * DAY {
                        s.execute(rusqlite::params![r, start, start + 2 * HOUR, format!("Show {} {}", id, start / DAY)])
                            .unwrap();
                        start += 2 * HOUR;
                    }
                }
            }
            guide_search::index_changes(&c).unwrap();
            c.execute_batch("COMMIT").unwrap();
            let rows: i64 = c.query_row("SELECT COUNT(*) FROM epg", [], |r| r.get(0)).unwrap();
            let landed = t.elapsed();
            let t = std::time::Instant::now();
            let p = prune_guide(&c, NOW).unwrap();
            let pruned = t.elapsed();
            let t = std::time::Instant::now();
            let again = prune_guide(&c, NOW).unwrap();
            let idle = t.elapsed();
            // The next sync, six hours on: what a pass prunes from then on.
            let t = std::time::Instant::now();
            let next = prune_guide(&c, NOW + 6 * HOUR).unwrap();
            let later = t.elapsed();
            let pragma = |n: &str| -> i64 { c.query_row(&format!("PRAGMA {n}"), [], |r| r.get(0)).unwrap() };
            let (pages, free, size) = (pragma("page_count"), pragma("freelist_count"), pragma("page_size"));
            let t = std::time::Instant::now();
            c.execute_batch("VACUUM").unwrap();
            guide_search::rebuild(&c).unwrap();
            let vac = t.elapsed();
            println!(
                "{label}: {rows} rows (landed in {landed:.2?}); pruned {} in {pruned:.2?}; \
                 second prune {} in {idle:.2?}; six hours on {} in {later:.2?}; file {} MB, {} MB free; vacuum+rebuild {vac:.2?}, now {} MB",
                p.programmes,
                again.programmes,
                next.programmes,
                pages * size / 1_000_000,
                free * size / 1_000_000,
                pragma("page_count") * size / 1_000_000,
            );
            drop(c);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
