//! The process-wide fuzzy index.
//!
//! One `nucleo` index serves both the channel filter and the command palette,
//! so it lives here rather than being rebuilt per query: tens of thousands of
//! names cost a few MB resident and a full rebuild is far too slow to do per
//! keystroke.

use anyhow::Result;
use rusqlite::Connection;
use std::sync::Mutex;

use crate::catalog::{store, vod_store, SearchIndex};

/// A `Mutex` and not the `RwLock` a read-mostly index would suggest:
/// `SearchIndex` holds its matcher in a `RefCell` and scoring mutates it, so
/// the type is `Send` but not `Sync` and concurrent readers are not sound.
pub struct Slot(Mutex<Option<SearchIndex>>);

impl Slot {
    pub const fn new() -> Self {
        Self(Mutex::new(None))
    }

    /// Repopulates from the catalogue, returning how many names are indexed.
    pub fn rebuild(&self, conn: &Connection) -> Result<usize> {
        Ok(self.rebuild_from(store::channel_names(conn)?))
    }

    /// Repopulates from names already in hand, returning how many.
    pub fn rebuild_from(&self, names: Vec<(i64, String)>) -> usize {
        let n = names.len();
        // A poisoned lock means a previous query panicked mid-scoring. The
        // index is derived state, so rebuilding over it is the recovery.
        let mut guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(SearchIndex::new(names));
        n
    }

    /// Ranked best-first. Empty before the first rebuild: search that has not
    /// loaded yet finds nothing, it does not fail.
    pub fn query(&self, needle: &str, limit: usize) -> Vec<i64> {
        let guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match guard.as_ref() {
            Some(idx) => idx.query(needle, limit),
            None => Vec::new(),
        }
    }

    pub fn is_built(&self) -> bool {
        let guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        guard.is_some()
    }
}

impl Default for Slot {
    fn default() -> Self {
        Self::new()
    }
}

static INDEX: Slot = Slot::new();
/// Movies get an index of their own: a hundred thousand titles must not slow
/// the channel filter and the palette, which search the same way.
static MOVIES: Slot = Slot::new();

pub fn rebuild(conn: &Connection) -> Result<usize> {
    INDEX.rebuild(conn)
}

pub fn query(needle: &str, limit: usize) -> Vec<i64> {
    INDEX.query(needle, limit)
}

pub fn is_built() -> bool {
    INDEX.is_built()
}

pub fn rebuild_movies(conn: &Connection) -> Result<usize> {
    Ok(MOVIES.rebuild_from(vod_store::movie_names(conn)?))
}

pub fn query_movies(needle: &str, limit: usize) -> Vec<i64> {
    MOVIES.query(needle, limit)
}

pub fn movies_built() -> bool {
    MOVIES.is_built()
}

/// Shows get theirs, for the same reason as movies.
static SERIES: Slot = Slot::new();

/// The live shows' names, as the series index holds them.
fn series_names(conn: &Connection) -> Result<Vec<(i64, String)>> {
    let mut stmt =
        conn.prepare("SELECT series_id, clean_name FROM series WHERE deleted_at IS NULL")?;
    let names = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(names)
}

pub fn rebuild_series(conn: &Connection) -> Result<usize> {
    Ok(SERIES.rebuild_from(series_names(conn)?))
}

pub fn query_series(needle: &str, limit: usize) -> Vec<i64> {
    SERIES.query(needle, limit)
}

pub fn series_built() -> bool {
    SERIES.is_built()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::open_in_memory;

    fn db() -> Connection {
        let c = open_in_memory().unwrap();
        for (id, name, sep) in [
            (1, "UK: NORTHWIND ONE LONDON", 0),
            (2, "CA: PRISM 1", 0),
            (3, "###### SPORTS ######", 1),
        ] {
            c.execute(
                "INSERT INTO channels (stream_id, name, clean_name, is_separator)
                 VALUES (?1,?2,?2,?3)",
                rusqlite::params![id, name, sep],
            )
            .unwrap();
        }
        c
    }

    #[test]
    fn query_before_rebuild_is_empty_not_a_panic() {
        let slot = Slot::new();
        assert!(!slot.is_built());
        assert!(slot.query("northwind", 10).is_empty());
    }

    #[test]
    fn rebuild_then_query_finds_channels() {
        let slot = Slot::new();
        // The separator is excluded by `channel_names`, so two, not three.
        assert_eq!(slot.rebuild(&db()).unwrap(), 2);
        assert_eq!(slot.query("northwindone", 10), vec![1]);
    }

    #[test]
    fn rebuild_replaces_rather_than_appends() {
        let slot = Slot::new();
        let c = db();
        slot.rebuild(&c).unwrap();
        c.execute("UPDATE channels SET deleted_at = 1 WHERE stream_id = 1", [])
            .unwrap();
        assert_eq!(slot.rebuild(&c).unwrap(), 1);
        assert!(slot.query("northwindone", 10).is_empty());
    }

    #[test]
    fn movie_search_is_its_own_index() {
        let channels = Slot::new();
        channels.rebuild(&db()).unwrap();
        let slot = Slot::new();
        let n = slot.rebuild_from(vec![(10, "First Film".into()), (11, "Second Film".into())]);
        assert_eq!(n, 2);
        let mut found = slot.query("film", 10);
        found.sort();
        assert_eq!(found, vec![10, 11]);
        assert_eq!(channels.query("northwindone", 10), vec![1]);
        assert!(channels.query("film", 10).is_empty());
    }

    #[test]
    fn series_search_is_its_own_index() {
        use crate::catalog::series_sync::{land_series, NewSeries};
        let c = db();
        c.execute(
            "INSERT INTO movies (movie_id, name, clean_name, added) VALUES (20, 'Quillmoor Film', 'Quillmoor Film', 1)",
            [],
        )
        .unwrap();
        let shows = [
            NewSeries {
                series_id: 30,
                name: "Quillmoor Harbour".into(),
                ..NewSeries::default()
            },
            NewSeries {
                series_id: 31,
                name: "Brackenfold".into(),
                ..NewSeries::default()
            },
        ];
        land_series(&c, &[], &shows, 1).unwrap();
        // Local slots, built as the process-wide ones are: those are shared
        // with every other test that searches.
        let series = Slot::new();
        assert_eq!(series.rebuild_from(series_names(&c).unwrap()), 2);
        let movies = Slot::new();
        movies.rebuild_from(vod_store::movie_names(&c).unwrap());
        let channels = Slot::new();
        channels.rebuild(&c).unwrap();
        // Each finds its own kind under the same word, and only it.
        assert_eq!(series.query("quillmoor", 10), vec![30]);
        assert_eq!(movies.query("quillmoor", 10), vec![20]);
        assert!(channels.query("quillmoor", 10).is_empty());
        assert!(series.query("northwindone", 10).is_empty());
        assert_eq!(channels.query("northwindone", 10), vec![1]);
    }

    // The free functions over `INDEX` are covered end-to-end by the `search`
    // command's tests instead. Rebuilding the one process-wide index from a
    // second test would race whichever test searched it.
}
