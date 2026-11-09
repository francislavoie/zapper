//! One sync at a time across processes sharing the mirror.
//!
//! The app and the recording service run on one machine as one user and so
//! open the same `zapper.db`. Each keeps the catalogue current on its own
//! schedule, and both read when the last pass finished from the database, so
//! they mostly take turns already; the lease covers the moment both find the
//! mirror due at once, which would fetch a guide of tens of megabytes twice
//! and have two writers queue on each other's long transactions.
//!
//! The lease is a row in `meta` naming the process holding it and when it
//! lapses. It lapses on its own, so a process killed mid-pass holds nothing
//! for long.

use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

use super::db;

const KEY: &str = "sync_lease";

/// Longer than any pass takes; a pass that outlives it merely risks one
/// overlapping pass, not a wrong mirror.
const TTL_SECS: i64 = 30 * 60;

/// Held for the length of a pass; given back when dropped.
pub struct Lease {
    holder: String,
}

impl Drop for Lease {
    fn drop(&mut self) {
        let _ = db::with_write_db(|c| {
            release(c, &self.holder)?;
            Ok(())
        });
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// This process, distinctly enough that a reused pid is not mistaken for it.
fn me() -> String {
    static STARTED: std::sync::OnceLock<u128> = std::sync::OnceLock::new();
    let started = STARTED.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    });
    format!("{}-{started}", std::process::id())
}

/// The lease, unless another process holds it. A database that cannot be
/// written is not a reason to skip the pass: the pass will say what is wrong.
pub fn take() -> Option<Lease> {
    let holder = me();
    match db::with_write_db(|c| try_take(c, &holder, now())) {
        Ok(true) => Some(Lease { holder }),
        Ok(false) => {
            tracing::info!("another process is syncing the catalogue; leaving it to that one");
            None
        }
        Err(e) => {
            tracing::debug!("sync lease unavailable ({e:#}); syncing anyway");
            Some(Lease { holder })
        }
    }
}

/// Takes the lease for `holder` if it is free, lapsed or already theirs.
pub fn try_take(conn: &Connection, holder: &str, now: i64) -> anyhow::Result<bool> {
    // IMMEDIATE takes the write lock up front, so two processes reading a
    // free lease cannot both go on to write it.
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let current: Option<String> = tx
        .query_row("SELECT value FROM meta WHERE key = ?1", [KEY], |r| r.get(0))
        .optional()?;
    if let Some((who, until)) = current.as_deref().and_then(parse) {
        if who != holder && until > now {
            return Ok(false);
        }
    }
    tx.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![KEY, format!("{holder} {}", now + TTL_SECS)],
    )?;
    tx.commit()?;
    Ok(true)
}

/// Gives the lease back, if it is still `holder`'s.
pub fn release(conn: &Connection, holder: &str) -> anyhow::Result<()> {
    conn.execute(
        "DELETE FROM meta WHERE key = ?1 AND value LIKE ?2",
        rusqlite::params![KEY, format!("{holder} %")],
    )?;
    Ok(())
}

fn parse(v: &str) -> Option<(&str, i64)> {
    let (who, until) = v.rsplit_once(' ')?;
    Some((who, until.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_lease_keeps_others_out_until_it_lapses_or_is_given_back() {
        let c = crate::catalog::open_in_memory().unwrap();
        assert!(try_take(&c, "app", 1000).unwrap());
        assert!(!try_take(&c, "server", 1001).unwrap(), "held by the app");
        assert!(try_take(&c, "app", 1002).unwrap(), "the holder may renew");
        assert!(
            try_take(&c, "server", 1002 + TTL_SECS + 1).unwrap(),
            "lapsed"
        );
        release(&c, "server").unwrap();
        assert!(try_take(&c, "app", 5000).unwrap(), "given back");
        release(&c, "someone else").unwrap();
        assert!(
            !try_take(&c, "server", 5001).unwrap(),
            "only the holder gives it back"
        );
    }
}
