//! What the last sync is doing, for anyone who missed the events.
//!
//! The frontend learns about sync from `zapper://sync-status` events, but a
//! window opened mid-sync — or reloaded — has to be able to ask.

use std::sync::Mutex;

use crate::catalog::db;
use crate::ipc::types::{SyncPhase, SyncStatusDto};

struct State {
    phase: SyncPhase,
    message: String,
    last_synced: Option<i64>,
}

static STATE: Mutex<State> = Mutex::new(State {
    phase: SyncPhase::Idle,
    message: String::new(),
    last_synced: None,
});

fn lock() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Remembers what was just emitted, so a poll and an event never disagree.
pub fn record(s: &SyncStatusDto) {
    let mut st = lock();
    st.phase = s.phase;
    st.message = s.message.clone();
    if s.last_synced.is_some() {
        st.last_synced = s.last_synced;
    }
}

const LAST_SYNCED_KEY: &str = "last_synced";

/// Records that a full pass finished at `at` (Unix seconds).
///
/// Persisted as well as cached: a restart that reported "never synced" over a
/// fully populated database would be a lie, and would push the UI toward
/// refetching a guide it already has.
pub fn mark_synced(at: i64) {
    lock().last_synced = Some(at);
    let _ = db::with_write_db(|c| {
        c.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![LAST_SYNCED_KEY, at.to_string()],
        )?;
        Ok(())
    });
}

/// Cached value if this process has synced, otherwise the persisted one.
pub fn last_synced() -> Option<i64> {
    if let Some(t) = lock().last_synced {
        return Some(t);
    }
    db::with_db(|c| {
        Ok(c.query_row(
            "SELECT value FROM meta WHERE key = ?1",
            [LAST_SYNCED_KEY],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .and_then(|v| v.parse::<i64>().ok()))
    })
    .ok()
    .flatten()
}

/// Live channels, excluding separators and soft-deleted rows — the number the
/// UI shows. A database that will not open reports zero rather than failing:
/// this is a status line, not a query.
pub fn channel_total() -> i64 {
    db::with_db(|c| {
        Ok(c.query_row(
            "SELECT COUNT(*) FROM channels WHERE is_separator = 0 AND deleted_at IS NULL",
            [],
            |r| r.get(0),
        )?)
    })
    .unwrap_or(0)
}

pub fn snapshot() -> SyncStatusDto {
    // Read through the accessor, not the field: after a restart the cache is
    // empty while the database remembers, and reporting "never synced" over a
    // populated mirror would push the UI into refetching a guide it already
    // has.
    // Taken before the lock, since it may itself query.
    let last = last_synced();
    let total = channel_total();
    let s = lock();
    SyncStatusDto {
        phase: s.phase,
        message: s.message.clone(),
        // Recounted rather than remembered: a sync that landed while nobody
        // was listening still shows up here.
        channels_total: total,
        last_synced: last,
    }
}

/// Serialises the tests that assert on this process-wide state. Without it,
/// two parallel tests setting a phase would each read the other's.
#[cfg(test)]
pub fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_was_emitted_is_what_a_later_poll_reports() {
        let _guard = test_lock();
        db::install_test_db();
        record(&SyncStatusDto {
            phase: SyncPhase::Channels,
            message: "60000 channels".into(),
            channels_total: 0,
            last_synced: None,
        });
        mark_synced(1_757_000_000);

        let snap = snapshot();
        assert_eq!(snap.phase, SyncPhase::Channels);
        assert_eq!(snap.message, "60000 channels");
        assert_eq!(snap.last_synced, Some(1_757_000_000));
        assert!(snap.channels_total >= 0);
    }

    #[test]
    fn mark_synced_persists_beyond_the_process() {
        // Both tests mutate the same process-wide status; test_lock keeps
        // them from interleaving under cargo's parallel runner.
        let _guard = test_lock();
        db::install_test_db();
        mark_synced(1_788_800_000);
        // Read the row directly rather than clearing the in-memory cache:
        // tests share that static and run in parallel, so mutating it here
        // would race whichever other test asserts on it.
        let stored: Option<String> = db::with_db(|c| {
            Ok(c.query_row(
                "SELECT value FROM meta WHERE key = ?1",
                [LAST_SYNCED_KEY],
                |r| r.get(0),
            )
            .ok())
        })
        .unwrap();
        assert_eq!(stored.as_deref(), Some("1788800000"));
    }
}
