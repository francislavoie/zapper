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

CREATE TABLE IF NOT EXISTS quality (
    stream_id   INTEGER PRIMARY KEY,
    codec       TEXT,
    width       INTEGER,
    height      INTEGER,
    fps         REAL,
    pix_fmt     TEXT,
    is_hdr      INTEGER NOT NULL DEFAULT 0,
    measured_at INTEGER,
    ok_count    INTEGER NOT NULL DEFAULT 0,
    fail_count  INTEGER NOT NULL DEFAULT 0,
    last_error  TEXT
);

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

fn add_missing_columns(conn: &Connection) -> Result<()> {
    for (table, column, decl) in ADDED_COLUMNS {
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
    add_missing_columns(conn)
}

fn tune(conn: &Connection) -> Result<()> {
    // WAL keeps background sync from blocking UI reads.
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
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
        for t in ["activity", "categories", "channels", "epg", "epg_channel_names", "quality"] {
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
