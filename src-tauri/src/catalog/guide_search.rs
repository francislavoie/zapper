//! Finding programmes in the guide by title.
//!
//! Channel search is fuzzy matching over a resident list of names (see
//! `search.rs`), because a picker has to find "NORTHWIND ONE" from
//! "northwindone". The guide is a different shape. A week of listings across a
//! large catalogue is a few hundred thousand rows, far too many to hold
//! resident and score on every keystroke, and a programme title is words that
//! people type as words. So this is SQLite FTS5 over `epg.title`: every word
//! typed must begin a word of the title, in any order, with case and accents
//! folded by the `unicode61` tokenizer — "cafe" finds "Café Society", as the
//! channel filter's smart normalisation would.
//!
//! The index is an external-content table over `epg`, addressed by `epg`'s
//! rowid, and it is brought up to date in bulk by whoever writes the guide:
//! [`index_changes`], inside the writer's own transaction, so a guide that
//! lands is a guide search finds. The XMLTV parser and HDHomeRun's guide both
//! call it; opening the mirror calls it too, so rows from any writer that did
//! not are searchable from the next start rather than never.
//!
//! Not row-by-row triggers, which is the textbook way to keep such an index:
//! measured on a 240,000-programme guide, an insert trigger writing the index
//! took landing it from 0.7s to 8s — every statement that fires a trigger
//! opens a savepoint, and FTS5 flushes its pending terms at each one — and a
//! trigger writing merely to a plain queue table still cost 3.5s. So inserts,
//! nearly all of what a sync writes, fire nothing: new rows are the ones above
//! a rowid watermark, indexed in one statement, which costs about half a
//! second for that whole guide. Only the rare changes the watermark cannot
//! see — a changed title, a deleted row — fire a trigger, which queues the
//! row with the title it was indexed under, because FTS5 can only remove an
//! entry when told exactly what was indexed. Resyncing an unchanged guide
//! fires nothing.
//!
//! Titles only, not descriptions. Results are ordered by when they air rather
//! than by relevance, so description matches would scatter every programme
//! whose blurb mentions a word among the airings of the show being looked
//! for; and descriptions are most of a guide's text, so indexing them would
//! multiply both the index and the time every guide write spends on it.
//!
//! `epg`'s rowid is implicit: upserts keep it, but `VACUUM` may renumber it.
//! Nothing here vacuums; anything that ever does must follow it with
//! [`rebuild`].

use anyhow::{Context, Result};
use rusqlite::Connection;

use crate::catalog::search::alternatives;
use crate::catalog::store::{channel_from_row, CHANNEL_COLUMNS, CHANNEL_COLUMN_COUNT, QUALITY_JOIN};
use crate::ipc::types::{AiringDto, ProgrammeDto, ProgrammeHitDto};

/// The index and what keeps it, created together or not at all.
///
/// - `epg_fts_mark.upto`: every `epg` row at or below it is indexed, under
///   the title in `epg_fts_stale` if it has an entry there, else its own.
///   Rows above it are not indexed yet.
/// - `epg_fts_stale`: indexed rows whose title has changed or which have
///   been deleted since, with the title they were indexed under. `OR IGNORE`
///   keeps the first, which is the indexed one.
///
/// A delete also lowers the mark to the highest rowid left. SQLite gives a
/// new row the highest rowid plus one, so deleting the top rows would
/// otherwise hand their rowids to new rows already beneath the mark, and
/// those would never be indexed.
///
/// Prefix indexes for two and three characters, because results arrive while
/// the first word is still being typed, and an unindexed short prefix has to
/// merge the postings of every term it begins.
const DDL: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS epg_fts USING fts5(
    title,
    content = 'epg',
    tokenize = 'unicode61 remove_diacritics 2',
    prefix = '2 3'
);
CREATE TABLE IF NOT EXISTS epg_fts_mark (
    id   INTEGER PRIMARY KEY CHECK (id = 0),
    upto INTEGER NOT NULL
);
INSERT OR IGNORE INTO epg_fts_mark (id, upto) VALUES (0, 0);
CREATE TABLE IF NOT EXISTS epg_fts_stale (
    id    INTEGER PRIMARY KEY,
    title TEXT NOT NULL
);
CREATE TRIGGER IF NOT EXISTS epg_fts_retitle AFTER UPDATE OF title ON epg
WHEN old.title IS NOT new.title
 AND old.rowid <= (SELECT upto FROM epg_fts_mark WHERE id = 0)
BEGIN
    INSERT OR IGNORE INTO epg_fts_stale (id, title) VALUES (old.rowid, old.title);
END;
CREATE TRIGGER IF NOT EXISTS epg_fts_forget AFTER DELETE ON epg BEGIN
    INSERT OR IGNORE INTO epg_fts_stale (id, title)
        SELECT old.rowid, old.title
        WHERE old.rowid <= (SELECT upto FROM epg_fts_mark WHERE id = 0);
    UPDATE epg_fts_mark
       SET upto = MIN(upto, IFNULL((SELECT MAX(rowid) FROM epg), 0))
     WHERE id = 0;
END;
"#;

/// Creates the index if the mirror has none, filling it from the guide
/// already there; otherwise catches it up. Part of `migrate`, so idempotent
/// like the rest of it: a mirror written by an older build gains the index
/// once, on first open.
pub fn install(conn: &Connection) -> Result<()> {
    let present: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'epg_fts'",
        [],
        |r| r.get(0),
    )?;
    if present {
        index_changes(conn)?;
        return Ok(());
    }
    // A savepoint rather than a transaction, so this nests inside one if
    // `migrate` is ever called from a caller's. A half-made index — the table
    // without its triggers — would silently go stale.
    conn.execute_batch("SAVEPOINT epg_fts_install")?;
    let made = conn.execute_batch(DDL).map_err(anyhow::Error::from).and_then(|()| rebuild(conn));
    match made {
        Ok(()) => conn.execute_batch("RELEASE epg_fts_install")?,
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO epg_fts_install; RELEASE epg_fts_install");
            return Err(e).context("creating the programme search index");
        }
    }
    Ok(())
}

/// Brings the index up to date with `epg`: drops what the stale queue says
/// was indexed, re-indexes those rows that still exist under their current
/// titles, and indexes every row above the mark. Every guide writer calls
/// this before it commits. Returns how many rows it indexed.
pub fn index_changes(conn: &Connection) -> Result<usize> {
    let upto: i64 =
        conn.query_row("SELECT upto FROM epg_fts_mark WHERE id = 0", [], |r| r.get(0))?;
    conn.execute(
        "INSERT INTO epg_fts (epg_fts, rowid, title)
         SELECT 'delete', id, title FROM epg_fts_stale",
        [],
    )?;
    // At or below the mark only: a rowid freed by a delete and taken by a new
    // row is above the lowered mark, and indexed with the new rows below.
    let mut n = conn.execute(
        "INSERT INTO epg_fts (rowid, title)
         SELECT e.rowid, e.title
         FROM epg_fts_stale s JOIN epg e ON e.rowid = s.id
         WHERE s.id <= ?1",
        [upto],
    )?;
    conn.execute("DELETE FROM epg_fts_stale", [])?;
    n += conn.execute(
        "INSERT INTO epg_fts (rowid, title)
         SELECT rowid, title FROM epg WHERE rowid > ?1 ORDER BY rowid",
        [upto],
    )?;
    conn.execute(
        "UPDATE epg_fts_mark SET upto = IFNULL((SELECT MAX(rowid) FROM epg), 0) WHERE id = 0",
        [],
    )?;
    Ok(n)
}

/// Recomputes the whole index from `epg`, and the bookkeeping with it.
pub fn rebuild(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "INSERT INTO epg_fts (epg_fts) VALUES ('rebuild');
         DELETE FROM epg_fts_stale;
         UPDATE epg_fts_mark SET upto = IFNULL((SELECT MAX(rowid) FROM epg), 0) WHERE id = 0;",
    )?;
    Ok(())
}

/// What someone typed, as an FTS5 query: each word a quoted prefix term, all
/// of them required — and, where the query offers alternatives (`a|b`, see
/// [`crate::catalog::search`]), each alternative's words so, any one
/// alternative enough. `None` when there is no word in it at all.
///
/// Split on anything that is not a letter or digit, which is how the
/// tokenizer splits titles, so "grey's" asks for "grey" and "s" just as the
/// title "Grey's Anatomy" was indexed. Quoting every term keeps FTS5's own
/// syntax — `AND`, `NEAR`, `-`, `:` — from being read out of a title search;
/// the only operators in the expression are the `OR`s written here, between
/// parenthesised groups of quoted terms.
pub fn match_expression(input: &str) -> Option<String> {
    let groups: Vec<String> = alternatives(input)
        .into_iter()
        .filter_map(|alt| {
            let terms: Vec<String> = alt
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .map(|w| format!("\"{w}\"*"))
                .collect();
            (!terms.is_empty()).then(|| terms.join(" "))
        })
        .collect();
    match groups.len() {
        0 => None,
        1 => groups.into_iter().next(),
        _ => Some(
            groups
                .iter()
                .map(|g| format!("({g})"))
                .collect::<Vec<_>>()
                .join(" OR "),
        ),
    }
}

/// Airings whose title matches `query`, relative to the instant `at`.
///
/// What is on now comes first, the most recently started first since it has
/// the most left to watch; then what is coming, soonest first; then what has
/// finished but can still be played from an archive, most recent first.
/// Finished programmes on channels without an archive, or older than it
/// reaches, are left out: there is nothing to do with them.
///
/// Channels that share a guide channel — the HD, FHD and 4K copies of one
/// network — would each list the same airing. Each airing appears once, on
/// the copy most watched, else the lowest-numbered.
pub fn search(
    conn: &Connection,
    query: &str,
    at: i64,
    offset: u32,
    limit: u32,
) -> Result<Vec<ProgrammeHitDto>> {
    let Some(expr) = match_expression(query) else {
        return Ok(Vec::new());
    };
    // Tier 0 is on now, 1 upcoming, 2 finished. A missing stop (stored as 0)
    // reads as finished once started, matching the guide's own test of
    // `now < max(start, stop)`.
    let sql = format!(
        "WITH placed AS (
             SELECT e.start, e.stop, e.title, e.description, ch.stream_id AS sid,
                    CASE WHEN e.start > ?2 THEN 1 WHEN e.stop > ?2 THEN 0 ELSE 2 END AS tier,
                    ROW_NUMBER() OVER (
                        PARTITION BY e.rowid
                        ORDER BY COALESCE(a.watch_seconds, 0) DESC,
                                 ch.num IS NULL, ch.num, ch.stream_id
                    ) AS pick
             FROM epg_fts
             JOIN epg e ON e.rowid = epg_fts.rowid
             JOIN channels ch ON ch.epg_channel_id = e.channel_ref
             LEFT JOIN activity a ON a.stream_id = ch.stream_id
             WHERE epg_fts MATCH ?1
               AND ch.deleted_at IS NULL
               AND ch.is_separator = 0
               AND (e.stop > ?2
                    OR (ch.has_archive <> 0
                        AND (ch.archive_days IS NULL
                             OR e.start >= ?2 - ch.archive_days * 86400)))
         )
         SELECT {CHANNEL_COLUMNS}, p.start, p.stop, p.title, p.description, p.tier
         FROM placed p
         JOIN channels ch ON ch.stream_id = p.sid
         {QUALITY_JOIN}
         WHERE p.pick = 1
         ORDER BY p.tier,
                  CASE WHEN p.tier = 1 THEN p.start ELSE -p.start END,
                  ch.num IS NULL, ch.num, p.sid
         LIMIT ?3 OFFSET ?4"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params![expr, at, i64::from(limit), i64::from(offset)],
        |row| {
            let mut channel = channel_from_row(row)?;
            let at = CHANNEL_COLUMN_COUNT;
            let programme = ProgrammeDto {
                start: row.get(at)?,
                stop: row.get(at + 1)?,
                title: row.get(at + 2)?,
                description: row.get(at + 3)?,
            };
            let airing = match row.get::<_, i64>(at + 4)? {
                0 => AiringDto::Live,
                1 => AiringDto::Upcoming,
                _ => AiringDto::Catchup,
            };
            // Tuning in from a live hit shows this as what is on, without
            // asking the guide again.
            if airing == AiringDto::Live {
                channel.now_playing = Some(programme.clone());
            }
            Ok(ProgrammeHitDto {
                channel,
                programme,
                airing,
            })
        },
    )?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::epg::parse_xmltv;
    use crate::catalog::open_in_memory;

    /// 2026-09-08 12:00 UTC: the fixture guide below is laid out around it.
    const NOW: i64 = 1_788_868_800;
    const HOUR: i64 = 3600;

    /// A day of a small guide, as XMLTV, so the index is filled the way a
    /// sync fills it — through the parser's upsert, not hand-made rows.
    const GUIDE: &str = r#"<tv>
      <programme start="20260907200000 +0000" stop="20260907210000 +0000" channel="nw1.uk">
        <title>Harbour Lights</title><desc>Last night's episode.</desc></programme>
      <programme start="20260908113000 +0000" stop="20260908123000 +0000" channel="nw1.uk">
        <title>Harbour Lights</title></programme>
      <programme start="20260908200000 +0000" stop="20260908210000 +0000" channel="nw1.uk">
        <title>Harbour Lights</title></programme>
      <programme start="20260908150000 +0000" stop="20260908160000 +0000" channel="m24.fr">
        <title>Harbour Lights</title></programme>
      <programme start="20260908110000 +0000" stop="20260908130000 +0000" channel="m24.fr">
        <title>Le Café des Étoiles</title></programme>
      <programme start="20260907100000 +0000" stop="20260907110000 +0000" channel="m24.fr">
        <title>Harbour Lights</title></programme>
      <programme start="20260830100000 +0000" stop="20260830110000 +0000" channel="nw1.uk">
        <title>Harbour Lights</title></programme>
      <programme start="20260908130000 +0000" stop="20260908140000 +0000" channel="nw1.uk">
        <title>Grey's Harbour</title></programme>
    </tv>"#;

    fn chan(c: &Connection, id: i64, name: &str, num: i64, epg: &str, archive_days: Option<i64>) {
        c.execute(
            "INSERT INTO channels (stream_id, category_id, name, clean_name, num, epg_channel_id,
                                   has_archive, archive_days)
             VALUES (?1, 'cat', ?2, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![id, name, num, epg, archive_days.is_some(), archive_days],
        )
        .unwrap();
    }

    /// Northwind keeps a three-day archive; Meridian has none.
    fn db() -> Connection {
        let c = open_in_memory().unwrap();
        chan(&c, 1, "Northwind One", 1, "nw1.uk", Some(3));
        chan(&c, 2, "Meridian 24", 2, "m24.fr", None);
        parse_xmltv(GUIDE.as_bytes(), &c).unwrap();
        c
    }

    fn hits(c: &Connection, q: &str) -> Vec<(i64, i64, AiringDto)> {
        search(c, q, NOW, 0, 100)
            .unwrap()
            .into_iter()
            .map(|h| (h.channel.stream_id, h.programme.start, h.airing))
            .collect()
    }

    #[test]
    fn match_expression_quotes_every_word_as_a_prefix() {
        assert_eq!(match_expression("harbour li").as_deref(), Some(r#""harbour"* "li"*"#));
        assert_eq!(match_expression("Grey's").as_deref(), Some(r#""Grey"* "s"*"#));
        // FTS5 operators are words like any other, not syntax.
        assert_eq!(
            match_expression(r#"NOT "x" OR y*"#).as_deref(),
            Some(r#""NOT"* "x"* "OR"* "y"*"#)
        );
        assert_eq!(match_expression("  -- ' "), None);
        assert_eq!(match_expression(""), None);
    }

    #[test]
    fn alternatives_become_parenthesised_ors() {
        assert_eq!(
            match_expression("news|weather").as_deref(),
            Some(r#"("news"*) OR ("weather"*)"#)
        );
        assert_eq!(
            match_expression("news | weather uk").as_deref(),
            Some(r#"("news"*) OR ("weather"* "uk"*)"#)
        );
        // An alternative with no word in it is no alternative.
        assert_eq!(match_expression("|news|' |").as_deref(), Some(r#""news"*"#));
        assert_eq!(match_expression("| -- |"), None);
    }

    #[test]
    fn either_title_is_found_and_each_airing_once() {
        let c = db();
        let both = hits(&c, "cafe|grey");
        assert_eq!(both.len(), 2, "{both:?}");
        // Words within one side still all have to match.
        assert_eq!(hits(&c, "cafe nothing|grey").len(), 1);
        // A title matching both sides is one hit, not two.
        assert_eq!(hits(&c, "harbour|lights").len(), hits(&c, "harbour").len());
    }

    #[test]
    fn on_now_then_upcoming_soonest_then_catchup_most_recent() {
        let c = db();
        let got = hits(&c, "harbour lights");
        let at = |h: i64| NOW + h * HOUR;
        assert_eq!(
            got,
            [
                (1, at(0) - HOUR / 2, AiringDto::Live),
                (2, at(3), AiringDto::Upcoming),
                (1, at(8), AiringDto::Upcoming),
                (1, at(-16), AiringDto::Catchup),
            ]
        );
    }

    #[test]
    fn finished_programmes_need_an_archive_that_still_reaches_them() {
        let c = db();
        let got = hits(&c, "harbour lights");
        // Meridian's from yesterday morning: no archive at all.
        assert!(!got.contains(&(2, NOW - 26 * HOUR, AiringDto::Catchup)));
        // Northwind's from nine days ago: older than its three days.
        assert!(got.iter().all(|(_, start, _)| *start > NOW - 3 * 86_400));
    }

    #[test]
    fn an_archive_of_unknown_depth_is_not_a_reason_to_refuse() {
        let c = db();
        c.execute("UPDATE channels SET archive_days = NULL WHERE stream_id = 1", [])
            .unwrap();
        let got = hits(&c, "harbour lights");
        assert_eq!(got.last().map(|h| h.1), Some(NOW - 9 * 86_400 - 2 * HOUR));
    }

    #[test]
    fn matching_folds_case_and_accents_and_takes_prefixes() {
        let c = db();
        assert_eq!(hits(&c, "CAFE etoi").len(), 1);
        assert_eq!(hits(&c, "café").len(), 1);
        // Every word must match, in any order.
        assert_eq!(hits(&c, "lights harb").len(), 4);
        assert!(hits(&c, "harbour nothing").is_empty());
        // An apostrophe splits a word in the title and in the query alike.
        assert_eq!(hits(&c, "grey's").len(), 1);
    }

    #[test]
    fn live_hits_carry_themselves_as_now_playing() {
        let c = db();
        let got = search(&c, "cafe", NOW, 0, 10).unwrap();
        assert_eq!(got[0].airing, AiringDto::Live);
        assert_eq!(
            got[0].channel.now_playing.as_ref().map(|p| p.title.as_str()),
            Some("Le Café des Étoiles")
        );
    }

    #[test]
    fn an_airing_shared_by_copies_of_a_channel_is_listed_once() {
        let c = db();
        chan(&c, 3, "Northwind One HD", 3, "nw1.uk", Some(3));
        let tonight = |c: &Connection| -> Vec<i64> {
            hits(c, "harbour lights")
                .into_iter()
                .filter(|h| h.1 == NOW + 8 * HOUR)
                .map(|h| h.0)
                .collect()
        };
        // The lowest number when nothing has been watched…
        assert_eq!(tonight(&c), [1]);
        // …and the copy actually watched when one has been.
        crate::catalog::store::record_activity(&c, 3, 600, true, NOW).unwrap();
        assert_eq!(tonight(&c), [3]);
    }

    #[test]
    fn deleted_channels_and_unmatched_guide_rows_find_nothing() {
        let c = db();
        c.execute("UPDATE channels SET deleted_at = 1", []).unwrap();
        assert!(hits(&c, "harbour").is_empty());
    }

    #[test]
    fn pages_follow_on_from_each_other() {
        let c = db();
        let all = hits(&c, "harbour");
        let page = |offset| -> Vec<i64> {
            search(&c, "harbour", NOW, offset, 2)
                .unwrap()
                .iter()
                .map(|h| h.programme.start)
                .collect()
        };
        let joined: Vec<i64> = [page(0), page(2), page(4)].concat();
        assert_eq!(joined, all.iter().map(|h| h.1).collect::<Vec<_>>());
    }

    #[test]
    fn the_index_follows_the_guide_as_it_is_rewritten() {
        let c = db();
        // A retitled airing, landed by the same upsert a sync uses.
        let retitled = GUIDE.replace("<title>Grey's Harbour</title>", "<title>Quayside</title>");
        parse_xmltv(retitled.as_bytes(), &c).unwrap();
        assert!(hits(&c, "grey").is_empty());
        assert_eq!(hits(&c, "quayside").len(), 1);
        integrity(&c);
        // A row deleted outright finds nothing at once, since its entry no
        // longer joins to a row, and the entry goes at the next catch-up.
        c.execute("DELETE FROM epg WHERE title = 'Quayside'", []).unwrap();
        assert!(hits(&c, "quayside").is_empty());
        index_changes(&c).unwrap();
        integrity(&c);
    }

    /// The external-content index agrees with its table: nothing indexed
    /// that is not there, nothing there under a title it was not indexed by.
    fn integrity(c: &Connection) {
        c.execute_batch("INSERT INTO epg_fts (epg_fts, rank) VALUES ('integrity-check', 1)")
            .unwrap();
    }

    fn insert(c: &Connection, start: i64, title: &str) {
        c.execute(
            "INSERT INTO epg (channel_ref, start, stop, title) VALUES ('nw1.uk', ?1, ?2, ?3)",
            rusqlite::params![start, start + HOUR, title],
        )
        .unwrap();
    }

    #[test]
    fn a_new_row_is_found_once_its_writer_catches_the_index_up() {
        let c = db();
        insert(&c, NOW + 30 * HOUR, "Quayside");
        assert!(hits(&c, "quayside").is_empty());
        assert_eq!(index_changes(&c).unwrap(), 1);
        assert_eq!(hits(&c, "quayside").len(), 1);
        // Nothing left to do the second time.
        assert_eq!(index_changes(&c).unwrap(), 0);
        integrity(&c);
    }

    #[test]
    fn rows_changed_before_they_were_ever_indexed_need_no_queue() {
        let c = db();
        // One XMLTV file can carry the same slot twice; the second wins.
        insert(&c, NOW + 30 * HOUR, "Quayside");
        c.execute("UPDATE epg SET title = 'Riverside' WHERE title = 'Quayside'", [])
            .unwrap();
        insert(&c, NOW + 32 * HOUR, "Fleeting");
        c.execute("DELETE FROM epg WHERE title = 'Fleeting'", []).unwrap();
        index_changes(&c).unwrap();
        assert!(hits(&c, "quayside").is_empty());
        assert_eq!(hits(&c, "riverside").len(), 1);
        assert!(hits(&c, "fleeting").is_empty());
        integrity(&c);
    }

    #[test]
    fn a_title_changed_twice_is_dropped_under_the_one_it_was_indexed_by() {
        let c = db();
        for title in ["Quayside", "Riverside"] {
            c.execute(
                "UPDATE epg SET title = ?1 WHERE title = 'Grey''s Harbour' OR title = 'Quayside'",
                [title],
            )
            .unwrap();
        }
        index_changes(&c).unwrap();
        assert!(hits(&c, "grey").is_empty());
        assert!(hits(&c, "quayside").is_empty());
        assert_eq!(hits(&c, "riverside").len(), 1);
        integrity(&c);
    }

    #[test]
    fn a_rowid_freed_at_the_top_and_reused_is_indexed() {
        let c = db();
        let top: i64 = c.query_row("SELECT MAX(rowid) FROM epg", [], |r| r.get(0)).unwrap();
        c.execute("DELETE FROM epg WHERE rowid = ?1", [top]).unwrap();
        // SQLite hands the freed rowid straight back out.
        insert(&c, NOW + 30 * HOUR, "Quayside");
        let reused: i64 = c.query_row("SELECT MAX(rowid) FROM epg", [], |r| r.get(0)).unwrap();
        assert_eq!(reused, top);
        index_changes(&c).unwrap();
        assert_eq!(hits(&c, "quayside").len(), 1);
        integrity(&c);
    }

    #[test]
    fn opening_the_mirror_catches_up_what_a_writer_left() {
        let c = db();
        insert(&c, NOW + 30 * HOUR, "Quayside");
        crate::catalog::migrate(&c).unwrap();
        assert_eq!(hits(&c, "quayside").len(), 1);
        integrity(&c);
    }

    #[test]
    fn rebuild_starts_over_from_the_guide() {
        let c = db();
        insert(&c, NOW + 30 * HOUR, "Quayside");
        c.execute("UPDATE epg SET title = 'Riverside' WHERE title = 'Grey''s Harbour'", [])
            .unwrap();
        rebuild(&c).unwrap();
        assert_eq!(hits(&c, "quayside").len(), 1);
        assert_eq!(hits(&c, "riverside").len(), 1);
        // The bookkeeping went with it, or catching up would index twice.
        assert_eq!(index_changes(&c).unwrap(), 0);
        integrity(&c);
    }

    #[test]
    fn a_mirror_with_a_guide_but_no_index_gains_a_full_one() {
        let c = db();
        c.execute_batch(
            "DROP TRIGGER epg_fts_retitle; DROP TRIGGER epg_fts_forget;
             DROP TABLE epg_fts; DROP TABLE epg_fts_mark; DROP TABLE epg_fts_stale;",
        )
        .unwrap();
        crate::catalog::migrate(&c).unwrap();
        assert_eq!(hits(&c, "harbour lights").len(), 4);
        // And a second migrate leaves it be.
        crate::catalog::migrate(&c).unwrap();
        assert_eq!(hits(&c, "harbour lights").len(), 4);
        integrity(&c);
    }
}
