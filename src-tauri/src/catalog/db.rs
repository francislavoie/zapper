//! The application database: where it lives, and the one connection to it.

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();
static WRITER: OnceLock<Mutex<Connection>> = OnceLock::new();

fn db_path_under(home: &Path) -> PathBuf {
    home.join(".local/share/zapper/zapper.db")
}

/// `$HOME`-relative rather than via a `dirs` crate: one environment variable
/// is not worth a dependency.
pub fn db_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    db_path_under(Path::new(&home))
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
    if cfg!(test) {
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
#[cfg(test)]
pub fn install_test_db() {
    DB.get_or_init(|| {
        Mutex::new(crate::catalog::open_in_memory().expect("in-memory test database"))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_path_lands_under_the_xdg_data_dir() {
        let p = db_path();
        assert!(
            p.ends_with(".local/share/zapper/zapper.db"),
            "unexpected database path: {}",
            p.display()
        );
    }

    #[test]
    fn db_path_is_built_from_home() {
        let p = db_path_under(Path::new("/tmp/fake-home"));
        assert_eq!(
            p,
            PathBuf::from("/tmp/fake-home/.local/share/zapper/zapper.db")
        );
    }

    #[test]
    fn open_creates_missing_parent_directories() {
        // Deliberately not the real path: a unit test must not write into $HOME.
        let root = std::env::temp_dir().join(format!("zapper-db-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let path = db_path_under(&root);

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
