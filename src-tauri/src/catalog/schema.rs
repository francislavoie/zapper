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
const EPG_EPISODE_COLUMNS: &[(&str, &str, &str)] = &[
    ("epg", "season", "INTEGER"),
    ("epg", "episode", "INTEGER"),
];

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

pub fn open_at(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
    tune(&conn)?;
    migrate(&conn)?;
    Ok(conn)
}

/// `tune` is deliberately not applied here: WAL is meaningless for an
/// in-memory database and SQLite rejects the pragma.
pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    migrate(&conn)?;
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
            .query_row("SELECT name, stream_url FROM channels WHERE stream_id = 7", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
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
