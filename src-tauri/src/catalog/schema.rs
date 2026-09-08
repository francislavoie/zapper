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

-- stream_id is INTEGER (64-bit). Panels issue ids above 2^31.
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
"#;

pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA).context("applying schema")
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
        for t in ["activity", "categories", "channels", "epg", "quality"] {
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
