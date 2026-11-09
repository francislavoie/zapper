//! The application database: where it lives, and the one connection to it.

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();
static WRITER: OnceLock<Mutex<Connection>> = OnceLock::new();

/// `~/.local/share/zapper/zapper.db` on Linux, `%LOCALAPPDATA%\zapper` on
/// Windows; see [`crate::paths`].
pub fn db_path() -> PathBuf {
    crate::paths::data_dir().join("zapper.db")
}

/// Opens (and migrates) a database at an explicit path, creating the
/// directories above it.
pub fn open_path(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    crate::catalog::open_at(path)
}

pub fn open() -> Result<Connection> {
    open_path(&db_path())
}

/// `rusqlite::Connection` is `Send` but not `Sync`, so the process shares one
/// behind a mutex rather than handing out references. Reads are microseconds
/// and sync writes in one transaction, so the contention is not worth a pool.
fn shared() -> Result<&'static Mutex<Connection>> {
    if let Some(db) = DB.get() {
        return Ok(db);
    }
    // A racing thread may win the `set`; its connection is as good as ours,
    // and the loser's is simply dropped.
    let _ = DB.set(Mutex::new(open()?));
    Ok(DB.get().expect("connection was just set"))
}

/// A second connection used only by the background sync.
///
/// Sharing one connection meant the EPG phase - a single transaction over
/// ~223,000 programmes - held the lock for its whole duration, freezing every
/// catalogue read behind it. The database runs in WAL mode, which permits one
/// writer concurrently with readers, so the writer gets its own handle and the
/// UI stays responsive while the guide refreshes.
fn writer() -> Result<&'static Mutex<Connection>> {
    if let Some(db) = WRITER.get() {
        return Ok(db);
    }
    // In tests the shared connection is in-memory, and a second connection
    // would open a *different* empty database. Fall back to the shared one.
    if cfg!(any(test, feature = "test-support")) {
        return shared();
    }
    let _ = WRITER.set(Mutex::new(open()?));
    Ok(WRITER.get().expect("writer was just set"))
}

/// Runs a write against the dedicated writer connection. Use for sync only;
/// everything the UI calls should go through [`with_db`].
pub fn with_write_db<R>(f: impl FnOnce(&Connection) -> Result<R>) -> Result<R> {
    let db = writer()?;
    let conn = db.lock().unwrap_or_else(|e| e.into_inner());
    f(&conn)
}

pub fn with_db<R>(f: impl FnOnce(&Connection) -> Result<R>) -> Result<R> {
    let db = shared()?;
    // A poisoned lock means a previous caller panicked holding the
    // connection. SQLite itself is unharmed, so carry on rather than taking
    // the whole catalogue down with it.
    let conn = db.lock().unwrap_or_else(|e| e.into_inner());
    f(&conn)
}

/// Installs an in-memory database as the process connection, so unit tests
/// never touch `$HOME`. Idempotent: the first caller wins and the rest share
/// it, which is why tests written against it must not assume empty tables.
#[cfg(any(test, feature = "test-support"))]
pub fn install_test_db() {
    DB.get_or_init(|| {
        Mutex::new(crate::catalog::open_in_memory().expect("in-memory test database"))
    });
}

/// Held for the whole of every test that lands or seeds movies: the test
/// catalogue is one for the process, and a landing sets aside every movie
/// it does not list, another test's included.
#[cfg(any(test, feature = "test-support"))]
pub fn movie_catalogue() -> std::sync::MutexGuard<'static, ()> {
    static MOVIE_CATALOGUE: Mutex<()> = Mutex::new(());
    MOVIE_CATALOGUE.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_path_lands_under_the_data_dir() {
        // Where the data directory is on each platform is `paths`' to test;
        // under test it is a sandbox, which is the point: were any test to
        // reach `with_db` before `install_test_db`, it would open a scratch
        // file rather than the catalogue on the machine running the tests.
        assert_eq!(db_path(), crate::paths::data_dir().join("zapper.db"));
        assert!(db_path().starts_with(std::env::temp_dir()));
    }

    #[test]
    fn open_creates_missing_parent_directories() {
        // Deliberately not the real path: a unit test must not write into $HOME.
        let root = std::env::temp_dir().join(format!("zapper-db-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let path = root.join("zapper/zapper.db");

        let conn = open_path(&path).unwrap();
        assert!(path.exists(), "database file was not created");
        // A migrated database, not merely a file.
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM channels", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);

        drop(conn);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn with_db_hands_out_a_migrated_connection() {
        install_test_db();
        // Row counts would race with every other test sharing this database;
        // the schema will not.
        let n: i64 = with_db(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='channels'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
        assert_eq!(n, 1);
    }
}
