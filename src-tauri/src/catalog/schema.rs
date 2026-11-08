//! SQLite schema and migrations.

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS categories (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    sort        INTEGER NOT NULL DEFAULT 0
);

-- stream_id is INTEGER (64-bit). Xtream servers issue ids above 2^31.
CREATE TABLE IF NOT EXISTS channels (
    stream_id       INTEGER PRIMARY KEY,
    category_id     TEXT,
    name            TEXT NOT NULL,
    clean_name      TEXT NOT NULL DEFAULT '',
    num             INTEGER,
    logo_url        TEXT,
    epg_channel_id  TEXT,
    has_archive     INTEGER NOT NULL DEFAULT 0,
    archive_days    INTEGER,
    is_separator    INTEGER NOT NULL DEFAULT 0,
    deleted_at      INTEGER
);
CREATE INDEX IF NOT EXISTS channels_by_category ON channels(category_id);
CREATE INDEX IF NOT EXISTS channels_by_epg ON channels(epg_channel_id);

CREATE TABLE IF NOT EXISTS epg (
    channel_ref TEXT NOT NULL,
    start       INTEGER NOT NULL,
    stop        INTEGER NOT NULL,
    title       TEXT NOT NULL,
    description TEXT,
    PRIMARY KEY (channel_ref, start)
);
CREATE INDEX IF NOT EXISTS epg_by_time ON epg(channel_ref, start, stop);

-- Small key/value store for state that must outlive the process, such as
-- when the mirror last refreshed. Reported to the UI, so a restart should not
-- claim the catalogue has never synced.
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS activity (
    stream_id     INTEGER PRIMARY KEY,
    watch_seconds INTEGER NOT NULL DEFAULT 0,
    last_watched  INTEGER,
    open_count    INTEGER NOT NULL DEFAULT 0
);

-- The names an XMLTV guide gives its channels, for matching playlist entries
-- whose tvg-id does not say which guide channel they are. Filled only for
-- M3U accounts: an Xtream server names its guide channels itself.
CREATE TABLE IF NOT EXISTS epg_channel_names (
    channel_id TEXT NOT NULL,
    name       TEXT NOT NULL,
    PRIMARY KEY (channel_id, name)
);
"#;

/// Columns added after the tables above first shipped.
///
/// `CREATE TABLE IF NOT EXISTS` does nothing to a table that exists, so a
/// mirror created by an older build would never get them; each is added with
/// `ALTER TABLE` when `pragma_table_info` says it is missing. Additive only,
/// nullable only, and idempotent like the rest of `migrate` — no version
/// number to keep in step, and an older build opening a newer mirror simply
/// ignores columns it does not select.
///
/// All of these belong to channels whose stream URL is stored — playlists,
/// HDHomeRun lineups and Tvheadend servers — and stay NULL for an Xtream
/// server's: a server builds its stream URLs from the account, the others
/// state each one.
const ADDED_COLUMNS: &[(&str, &str, &str)] = &[
    // Where the channel plays from. Its presence is what marks a row as a
    // playlist channel.
    ("channels", "stream_url", "TEXT"),
    // The playlist's own guide identifiers, kept apart from `epg_channel_id`
    // (the *resolved* guide channel) so matching can be redone.
    ("channels", "tvg_id", "TEXT"),
    ("channels", "tvg_name", "TEXT"),
    // `m3u::catchup::Mode` spelling and the template it uses.
    ("channels", "catchup_mode", "TEXT"),
    ("channels", "catchup_source", "TEXT"),
    // Per-channel HTTP headers from `#EXTVLCOPT` / `#EXTHTTP`, handed to mpv.
    ("channels", "http_user_agent", "TEXT"),
    ("channels", "http_referrer", "TEXT"),
    // How the channel number is written when `num` cannot say it: an
    // over-the-air subchannel's `5.1`, which `num` holds as `5001` so the
    // listing still sorts by it. Set by HDHomeRun lineups and by any
    // playlist whose `tvg-chno` has a minor part.
    ("channels", "num_label", "TEXT"),
];

/// Migration step `channel_quality`: what playing each channel has shown
/// about it, kept for good.
///
/// Keyed on the catalogue as well as the channel — `origin` is
/// [`crate::catalog::origin`]'s hash of the account — because a server's
/// stream ids are its own: channel 1234 on one provider is not channel 1234
/// on the next, and the readings must not cross over when the account
/// changes. Within one catalogue the key outlives every resync, the way
/// `activity` does, since sync only ever soft-deletes.
///
/// - The measurement — resolution, frame rate, codec, interlacing, HDR, the
///   audio — is taken a few seconds into each live play and overwrites the
///   last, so it follows a provider that changes a channel. `measured_at` is
///   when ("last checked").
/// - `ok_count` and `fail_count` count opens that reached a picture and
///   opens that genuinely failed to load. `failed_at` and `fail_reason` are
///   the latest such failure, cleared by the next play that works; see
///   `ipc::ending::against_channel` for what counts.
///
/// Replaces the `quality` table of the first schema, keyed on the stream id
/// alone, which nothing ever wrote.
const CHANNEL_QUALITY: &str = r#"
CREATE TABLE IF NOT EXISTS channel_quality (
    origin         TEXT NOT NULL,
    stream_id      INTEGER NOT NULL,
    codec          TEXT,
    width          INTEGER,
    height         INTEGER,
    fps            REAL,
    interlaced     INTEGER,
    is_hdr         INTEGER NOT NULL DEFAULT 0,
    audio_codec    TEXT,
    audio_channels INTEGER,
    measured_at    INTEGER,
    ok_count       INTEGER NOT NULL DEFAULT 0,
    fail_count     INTEGER NOT NULL DEFAULT 0,
    failed_at      INTEGER,
    fail_reason    TEXT,
    PRIMARY KEY (origin, stream_id)
);
DROP TABLE IF EXISTS quality;
"#;

/// Migration step `epg_episode`: a programme's season and episode, one-based,
/// from the guide's `<episode-num>` (see [`crate::catalog::episode`]) or
/// SiliconDust's `EpisodeNumber`. NULL when the guide does not say, or says
/// it in a way that cannot be trusted; recordings then go by date.
const EPG_EPISODE_COLUMNS: &[(&str, &str, &str)] =
    &[("epg", "season", "INTEGER"), ("epg", "episode", "INTEGER")];

/// Migration step `backup_lookup`: what choosing a recording's backup feed
/// needs to stay fast on a large catalogue (`recording::backup`). Channels
/// by guide channel are already indexed (`channels_by_epg`); programmes by
/// start time across every guide channel are not, and the title match looks
/// a few minutes either side of one start.
const BACKUP_INDEXES: &str = r#"
CREATE INDEX IF NOT EXISTS epg_by_start ON epg(start);
"#;

/// Migration step `vod`: the provider's movies, and who marked which.
///
/// `movies` is the mirror of `get_vod_streams` and, like `channels`, is only
/// ever soft-deleted by a sync, so a title that drops out of the list and
/// comes back finds its marks again. It is keyed on the provider's id alone
/// and not on the catalogue: a resync under a different account soft-deletes
/// whatever that account did not list, the way channels are, and the rows
/// are never shown while deleted.
///
/// `vod_marks` is the opposite and carries the catalogue's `origin` (see
/// [`crate::catalog::origin`]), because a provider's movie 1234 is not the
/// next provider's 1234: a favourite or a resume point must not follow the id
/// to a different film when the account changes. `kind` leaves room for
/// episodes to share the table; a movie's `item_id` is its `movie_id`.
/// `position`/`duration` are seconds into the title and its length, as last
/// played; `played_at` orders "continue watching".
///
/// `movie_details` caches `get_vod_info` per title, fetched only when a title
/// is opened and never for the whole list. `vod_categories` is shared across
/// kinds for the same reason as `vod_marks`; the primary key is the pair
/// because two kinds may reuse an id.
///
/// The indexes serve the page queries: newest first overall and within a
/// category, alphabetical, and the year and rating sorts, so that none of
/// them scans a ~160k-row table. The four sorts' indexes hold only the live
/// titles, each in its sort's whole order, and each holds the cleaned name:
/// a search (`catalog::title_search`) tests every title's name, and reads
/// it there, in the list's order, rather than from the table's much longer
/// rows. They replaced `movies_by_added`, `_name`, `_year` and `_rating`,
/// which held neither.
const VOD_TABLES: &str = r#"
CREATE TABLE IF NOT EXISTS vod_categories (
    kind  TEXT NOT NULL,
    id    TEXT NOT NULL,
    name  TEXT NOT NULL,
    sort  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (kind, id)
);
CREATE TABLE IF NOT EXISTS movies (
    movie_id        INTEGER PRIMARY KEY,
    category_id     TEXT,
    name            TEXT NOT NULL,
    clean_name      TEXT NOT NULL,
    year            INTEGER,
    rating          REAL,
    poster_url      TEXT,
    added           INTEGER NOT NULL,
    container       TEXT,
    tmdb_id         INTEGER,
    duration_secs   INTEGER,
    stream_url      TEXT,
    http_user_agent TEXT,
    http_referrer   TEXT,
    deleted_at      INTEGER
);
CREATE INDEX IF NOT EXISTS movies_by_category ON movies(category_id, added);
DROP INDEX IF EXISTS movies_by_added;
DROP INDEX IF EXISTS movies_by_name;
DROP INDEX IF EXISTS movies_by_year;
DROP INDEX IF EXISTS movies_by_rating;
CREATE INDEX IF NOT EXISTS movies_live_by_added ON movies(added, movie_id, clean_name)
    WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS movies_live_by_name ON movies(clean_name COLLATE NOCASE, movie_id)
    WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS movies_live_by_year ON movies(year DESC, clean_name, movie_id)
    WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS movies_live_by_rating ON movies(rating DESC, clean_name, movie_id)
    WHERE deleted_at IS NULL;
-- Counting a category's live titles reads this alone, with no table lookups.
CREATE INDEX IF NOT EXISTS movies_live_by_category ON movies(category_id)
    WHERE deleted_at IS NULL;
CREATE TABLE IF NOT EXISTS movie_details (
    movie_id   INTEGER PRIMARY KEY,
    fetched_at INTEGER NOT NULL,
    info       TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS vod_marks (
    origin       TEXT NOT NULL,
    kind         TEXT NOT NULL,
    item_id      INTEGER NOT NULL,
    favourite_at INTEGER,
    watched_at   INTEGER,
    position     REAL,
    duration     REAL,
    played_at    INTEGER,
    PRIMARY KEY (origin, kind, item_id)
);
"#;

/// Migration step `series`: the provider's shows and their episodes.
///
/// `series` mirrors the series list as `movies` mirrors the movie list:
/// keyed on the provider's id, only ever soft-deleted by a sync, its marks
/// in `vod_marks` under kind `series` and its categories in `vod_categories`
/// under the same kind. `added` follows the provider's `last_modified`,
/// which moves when a show gains an episode, so "newest first" puts the
/// shows with something new on top; a show from a playlist, which dates
/// nothing, is as new as the newest episode first seen in it.
///
/// `episodes` is two things by source. For an Xtream server it is a cache of
/// the details, landed with `series_details` when a show is opened and never
/// for the whole list, so a show not yet opened has none and NULL counts.
/// For a playlist, which lists every episode, it is a mirror like the shows,
/// written with them on every sync. Either way an episode is soft-deleted
/// when its source stops listing it, so its marks (kind `episode`) are still
/// there if it returns. `episode_end` is a playlist entry's last number when
/// it holds several episodes (`S01E01-E02`). `season_count` and
/// `episode_count` are counted from the live episodes (specials aside, for
/// seasons) when they land.
///
/// The indexes serve the same page queries as the movies' (holding the
/// names for a search, as theirs do), and a show's episodes in order.
const SERIES_TABLES: &str = r#"
CREATE TABLE IF NOT EXISTS series (
    series_id     INTEGER PRIMARY KEY,
    category_id   TEXT,
    name          TEXT NOT NULL,
    clean_name    TEXT NOT NULL,
    year          INTEGER,
    rating        REAL,
    poster_url    TEXT,
    added         INTEGER NOT NULL,
    tmdb_id       INTEGER,
    season_count  INTEGER,
    episode_count INTEGER,
    deleted_at    INTEGER
);
CREATE INDEX IF NOT EXISTS series_by_category ON series(category_id, added);
DROP INDEX IF EXISTS series_by_added;
DROP INDEX IF EXISTS series_by_name;
DROP INDEX IF EXISTS series_by_year;
DROP INDEX IF EXISTS series_by_rating;
CREATE INDEX IF NOT EXISTS series_live_by_added ON series(added, series_id, clean_name)
    WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS series_live_by_name ON series(clean_name COLLATE NOCASE, series_id)
    WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS series_live_by_year ON series(year DESC, clean_name, series_id)
    WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS series_live_by_rating ON series(rating DESC, clean_name, series_id)
    WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS series_live_by_category ON series(category_id)
    WHERE deleted_at IS NULL;
CREATE TABLE IF NOT EXISTS series_details (
    series_id  INTEGER PRIMARY KEY,
    fetched_at INTEGER NOT NULL,
    info       TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS episodes (
    episode_id      INTEGER PRIMARY KEY,
    series_id       INTEGER NOT NULL,
    season          INTEGER NOT NULL,
    episode         INTEGER NOT NULL,
    episode_end     INTEGER,
    title           TEXT,
    plot            TEXT,
    duration_secs   INTEGER,
    still_url       TEXT,
    air_date        TEXT,
    container       TEXT,
    added           INTEGER,
    stream_url      TEXT,
    http_user_agent TEXT,
    http_referrer   TEXT,
    deleted_at      INTEGER
);
CREATE INDEX IF NOT EXISTS episodes_by_series ON episodes(series_id, season, episode);
"#;

fn add_missing_columns(conn: &Connection, columns: &[(&str, &str, &str)]) -> Result<()> {
    for (table, column, decl) in columns {
        let present: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM pragma_table_info(?1) WHERE name = ?2",
                [table, column],
                |r| r.get(0),
            )
            .with_context(|| format!("inspecting {table}"))?;
        if !present {
            // Identifiers cannot be bound; these are constants above.
            conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"))
                .with_context(|| format!("adding {table}.{column}"))?;
        }
    }
    Ok(())
}

pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA).context("applying schema")?;
    add_missing_columns(conn, ADDED_COLUMNS)?;
    conn.execute_batch(CHANNEL_QUALITY)
        .context("migrating channel_quality")?;
    add_missing_columns(conn, EPG_EPISODE_COLUMNS).context("migrating epg_episode")?;
    conn.execute_batch(BACKUP_INDEXES)
        .context("migrating backup_lookup")?;
    conn.execute_batch(VOD_TABLES).context("migrating vod")?;
    conn.execute_batch(SERIES_TABLES)
        .context("migrating series")?;
    crate::catalog::guide_search::install(conn)
}

fn tune(conn: &Connection) -> Result<()> {
    // WAL keeps background sync from blocking UI reads.
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // The app and the recording service may share this file. WAL lets
    // either read while the other writes, but two writers still take turns,
    // and without a timeout the second fails at once with "database is
    // locked" instead of waiting its turn. The guide's write is one long
    // transaction, hence the generous wait.
    conn.busy_timeout(std::time::Duration::from_secs(60))?;
    Ok(())
}

/// The functions the catalogue's reads call, which SQLite keeps per
/// connection rather than in the file.
fn functions(conn: &Connection) -> Result<()> {
    crate::catalog::title_search::install(conn).context("adding title_match")
}

pub fn open_at(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
    tune(&conn)?;
    migrate(&conn)?;
    functions(&conn)?;
    Ok(conn)
}

/// `tune` is deliberately not applied here: WAL is meaningless for an
/// in-memory database and SQLite rejects the pragma.
pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    migrate(&conn)?;
    functions(&conn)?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrate_creates_every_table() {
        let c = open_in_memory().unwrap();
        let mut stmt = c
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap();
        let tables: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for t in [
            "activity",
            "categories",
            "channels",
            "epg",
            "epg_channel_names",
            "epg_fts",
            "channel_quality",
            "movies",
            "vod_categories",
            "movie_details",
            "vod_marks",
            "series",
            "series_details",
            "episodes",
        ] {
            assert!(
                tables.contains(&t.to_string()),
                "missing table {t}; got {tables:?}"
            );
        }
    }

    #[test]
    fn migrate_is_idempotent() {
        let c = open_in_memory().unwrap();
        migrate(&c).unwrap();
        migrate(&c).unwrap();
    }

    #[test]
    fn a_mirror_from_before_playlists_gains_their_columns() {
        // The channels table as the first release created it.
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE channels (
                stream_id INTEGER PRIMARY KEY, category_id TEXT, name TEXT NOT NULL,
                clean_name TEXT NOT NULL DEFAULT '', num INTEGER, logo_url TEXT,
                epg_channel_id TEXT, has_archive INTEGER NOT NULL DEFAULT 0,
                archive_days INTEGER, is_separator INTEGER NOT NULL DEFAULT 0,
                deleted_at INTEGER);
             INSERT INTO channels (stream_id, name) VALUES (7, 'kept');",
        )
        .unwrap();
        migrate(&c).unwrap();
        migrate(&c).unwrap();
        let (name, url): (String, Option<String>) = c
            .query_row(
                "SELECT name, stream_url FROM channels WHERE stream_id = 7",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((name.as_str(), url), ("kept", None));
        for (_, col, _) in ADDED_COLUMNS {
            let n: i64 = c
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('channels') WHERE name = ?1",
                    [col],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "{col} missing after migrate");
        }
    }

    #[test]
    fn the_unused_quality_table_gives_way_to_channel_quality() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE quality (stream_id INTEGER PRIMARY KEY, codec TEXT);")
            .unwrap();
        migrate(&c).unwrap();
        let names: Vec<String> = c
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '%quality'")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(names, ["channel_quality"]);
    }

    #[test]
    fn a_guide_from_before_episode_numbers_gains_them() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE epg (channel_ref TEXT NOT NULL, start INTEGER NOT NULL,
                stop INTEGER NOT NULL, title TEXT NOT NULL, description TEXT,
                PRIMARY KEY (channel_ref, start));
             INSERT INTO epg VALUES ('a', 1, 2, 'Kept', NULL);",
        )
        .unwrap();
        migrate(&c).unwrap();
        migrate(&c).unwrap();
        let (title, season, episode): (String, Option<i64>, Option<i64>) = c
            .query_row("SELECT title, season, episode FROM epg", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!((title.as_str(), season, episode), ("Kept", None, None));
    }

    #[test]
    fn programmes_are_indexed_by_start_for_the_backup_lookup() {
        let c = open_in_memory().unwrap();
        let plan: String = c
            .query_row(
                "EXPLAIN QUERY PLAN SELECT channel_ref FROM epg WHERE start BETWEEN 1 AND 2",
                [],
                |r| r.get(3),
            )
            .unwrap();
        assert!(plan.contains("epg_by_start"), "{plan}");
    }

    #[test]
    fn an_existing_mirror_gains_the_movie_tables_and_keeps_its_channels() {
        // A mirror as the build before movies left it: every step but `vod`.
        let c = open_in_memory().unwrap();
        c.execute_batch(
            "DROP TABLE movies; DROP TABLE vod_categories;
             DROP TABLE movie_details; DROP TABLE vod_marks;
             INSERT INTO channels (stream_id, name) VALUES (7, 'kept');",
        )
        .unwrap();
        migrate(&c).unwrap();
        migrate(&c).unwrap();
        let n: i64 = c
            .query_row(
                "SELECT (SELECT COUNT(*) FROM channels WHERE name = 'kept')
                      + (SELECT COUNT(*) FROM movies) + (SELECT COUNT(*) FROM vod_marks)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn stream_id_holds_values_beyond_i32() {
        let c = open_in_memory().unwrap();
        c.execute(
            "INSERT INTO channels (stream_id, name) VALUES (?1, ?2)",
            rusqlite::params![4_294_967_295i64, "big"],
        )
        .unwrap();
        let got: i64 = c
            .query_row("SELECT stream_id FROM channels", [], |r| r.get(0))
            .unwrap();
        assert_eq!(got, 4_294_967_295i64);
    }
}
