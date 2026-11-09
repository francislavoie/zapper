//! Landing the provider's movie list in the mirror.
//!
//! Like channels, movies are upserted and never deleted: a title the server
//! stops listing is soft-deleted, so its favourite and resume point are still
//! there if it returns. One transaction per write, so a download that fails
//! halfway changes nothing.

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashSet;

use crate::catalog::sync::{clean_name, SyncStats};
use crate::m3u::ids;
use crate::xtream::{Category, VodStream};

/// A movie as the mirror stores it, whichever kind of source listed it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NewMovie {
    pub movie_id: i64,
    pub category_id: Option<String>,
    pub name: String,
    pub year: Option<i32>,
    pub rating: Option<f64>,
    pub poster_url: Option<String>,
    /// Unix seconds the provider added it, when it says.
    pub added: Option<i64>,
    pub container: Option<String>,
    pub tmdb_id: Option<i64>,
    /// Where a playlist's entry plays from; None for an Xtream server's,
    /// which builds the URL from the account.
    pub stream_url: Option<String>,
    pub user_agent: Option<String>,
    pub referrer: Option<String>,
}

impl From<&VodStream> for NewMovie {
    fn from(v: &VodStream) -> Self {
        Self {
            movie_id: v.stream_id,
            category_id: v.category_id.map(|c| c.to_string()),
            name: v.name.clone(),
            year: v
                .year
                .and_then(|y| i32::try_from(y).ok())
                .filter(plausible_year),
            rating: v.rating10(),
            poster_url: v.stream_icon.clone(),
            added: v.added.filter(|a| *a > 0),
            container: v.container_extension.clone(),
            tmdb_id: v.tmdb,
            ..Self::default()
        }
    }
}

/// The catalogue the mirror's movies were last landed from, in `meta`.
const MOVIES_ORIGIN: &str = "movies_origin";

/// Which catalogue the mirror's movies are from, for the interface: a hash
/// of the origin (itself a hash, never an address or an account), empty
/// before any movies were landed. When it changes, the titles are another
/// account's, whatever ids they share with the ones shown so far.
pub fn movies_origin(conn: &Connection) -> Result<String> {
    landed_hash(conn, MOVIES_ORIGIN)
}

/// The catalogue a kind of title was last landed from, as `meta` records it
/// under `key`.
fn landed(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
        .optional()?)
}

/// [`landed`] hashed for the interface, empty before any landed.
pub(super) fn landed_hash(conn: &Connection, key: &str) -> Result<String> {
    Ok(landed(conn, key)?.map_or_else(String::new, |o| {
        format!("{:016x}", ids::fnv1a(o.as_bytes()))
    }))
}

/// The catalogue being written now, and whether the titles recorded under
/// `key` are another one's.
///
/// Each kind keeps its own record apart from the channels', because the
/// channels are written first and will already have recorded the new one.
pub(super) fn current_origin(conn: &Connection, key: &str) -> Result<(String, bool)> {
    let origin = crate::catalog::origin::recorded(conn)?.unwrap_or_default();
    let switched = landed(conn, key)?.is_some_and(|l| l != origin);
    Ok((origin, switched))
}

/// Notes under `key` that the titles now came from `origin`.
pub(super) fn record_origin(conn: &Connection, key: &str, origin: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, origin],
    )?;
    Ok(())
}

/// A kind's categories in the order they are shown: a source's own list,
/// or a playlist's groups in order of first appearance.
#[derive(Default)]
pub(super) struct CategoryList {
    /// `(id, name)`.
    list: Vec<(String, String)>,
    ids: HashSet<String>,
}

impl CategoryList {
    pub(super) fn extend(&mut self, cats: &[Category]) {
        for c in cats {
            if self.ids.insert(c.category_id.clone()) {
                self.list
                    .push((c.category_id.clone(), c.category_name.clone()));
            }
        }
    }

    /// The category a playlist group becomes.
    pub(super) fn group(&mut self, g: &str) -> String {
        let id = ids::category_id(g);
        if self.ids.insert(id.clone()) {
            self.list.push((id.clone(), g.to_owned()));
        }
        id
    }

    pub(super) fn len(&self) -> usize {
        self.list.len()
    }

    /// Replaces `kind`'s categories wholesale. Derived from what the source
    /// says each time, so one it dropped is not a category any more.
    pub(super) fn replace(&self, conn: &Connection, kind: &str) -> Result<()> {
        conn.execute("DELETE FROM vod_categories WHERE kind = ?1", [kind])?;
        let mut insert = conn.prepare_cached(
            "INSERT INTO vod_categories (kind, id, name, sort) VALUES (?1, ?2, ?3, ?4)",
        )?;
        for (i, (id, name)) in self.list.iter().enumerate() {
            insert.execute(params![kind, id, name, i as i64])?;
        }
        Ok(())
    }
}

pub(super) fn plausible_year(y: &i32) -> bool {
    (1900..=2099).contains(y)
}

/// The year a title's name carries, for lists that do not give one: a
/// trailing `(2021)`, or a trailing ` - 1999`. A bare number in the name
/// (`Room 2049`) is not one.
pub fn year_from_name(name: &str) -> Option<i32> {
    let t = name.trim();
    let tail = if let Some(rest) = t.strip_suffix(')') {
        rest.rsplit_once('(')?.1
    } else {
        t.rsplit_once(" - ")?.1
    };
    let tail = tail.trim();
    if tail.len() != 4 || !tail.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    tail.parse().ok().filter(plausible_year)
}

pub(super) fn is_separator(name: &str) -> bool {
    let t = name.trim();
    t.starts_with("##") && t.ends_with("##")
}

/// One open write of the movie list inside someone else's transaction, which
/// also commits it. The playlist writer drives this as it parses entries;
/// [`land_movies`] drives it for an Xtream list. Dropping the transaction
/// without committing undoes everything.
pub struct MovieWriter<'t> {
    conn: &'t Connection,
    existing: HashSet<i64>,
    seen: HashSet<i64>,
    categories: CategoryList,
    stats: SyncStats,
    /// Whether an empty list is believed. See [`Self::accept_empty`].
    empty_ok: bool,
}

impl<'t> MovieWriter<'t> {
    pub fn begin(tx: &'t rusqlite::Transaction<'_>) -> Result<Self> {
        Self::begin_on(tx)
    }

    /// As [`begin`](Self::begin), for a writer that holds the transaction's
    /// connection rather than the transaction itself: everything written
    /// through it is inside the transaction all the same.
    pub fn begin_on(conn: &'t Connection) -> Result<Self> {
        let existing: HashSet<i64> = {
            let mut s = conn.prepare("SELECT movie_id FROM movies WHERE deleted_at IS NULL")?;
            let ids = s.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
            ids
        };
        Ok(Self {
            conn,
            existing,
            seen: HashSet::new(),
            categories: CategoryList::default(),
            stats: SyncStats::default(),
            empty_ok: false,
        })
    }

    /// How many distinct movies have been written so far.
    pub fn seen(&self) -> usize {
        self.seen.len()
    }

    /// Believes an empty list. A playlist that still has its channels but no
    /// movies any more is a provider that dropped them, where a server that
    /// answers with an empty movie list is more likely having a bad minute.
    pub fn accept_empty(&mut self) {
        self.empty_ok = true;
    }

    /// Names the categories as the source lists them, for a source that has
    /// a list of its own rather than groups.
    fn set_categories(&mut self, cats: &[Category]) {
        self.categories.extend(cats);
    }

    /// Writes one movie. `group` is the playlist group it sits in, which
    /// becomes its category (in order of first appearance); a source with a
    /// category list leaves it `None` and the movie's own `category_id` stands.
    pub fn push(&mut self, m: &NewMovie, group: Option<&str>) -> Result<()> {
        if is_separator(&m.name) {
            return Ok(());
        }
        let category = match group {
            Some(g) => Some(self.categories.group(g)),
            None => m.category_id.clone(),
        };
        // As the Xtream conversion does: a zero is "no date", not 1970.
        let added = m.added.filter(|a| *a > 0);
        // What was fetched about a title under another name or film id is
        // not about this one.
        self.conn
            .prepare_cached(
                "DELETE FROM movie_details WHERE movie_id = ?1 AND EXISTS
                   (SELECT 1 FROM movies x WHERE x.movie_id = ?1
                      AND (x.name IS NOT ?2 OR x.tmdb_id IS NOT ?3))",
            )?
            .execute(params![m.movie_id, m.name, m.tmdb_id])?;
        let year = m
            .year
            .filter(plausible_year)
            .or_else(|| year_from_name(&m.name));
        self.conn
            .prepare_cached(
                // A title the provider gave no date for is stored with 0 and
                // stamped with the time of the write in `finish`.
                "INSERT INTO movies
                   (movie_id, category_id, name, clean_name, year, rating, poster_url,
                    added, container, tmdb_id, stream_url, http_user_agent,
                    http_referrer, deleted_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,COALESCE(?8, 0),?9,?10,?11,?12,?13,NULL)
                 ON CONFLICT(movie_id) DO UPDATE SET
                   category_id=excluded.category_id, name=excluded.name,
                   clean_name=excluded.clean_name, year=excluded.year,
                   rating=excluded.rating, poster_url=excluded.poster_url,
                   -- The provider's date wins when given; otherwise the day it
                   -- was first seen stands, so a title does not climb the
                   -- newest-first list every time the list is fetched.
                   added=COALESCE(?8, movies.added),
                   -- The runtime comes from the details and goes with them.
                   duration_secs=CASE WHEN movies.name IS excluded.name
                                       AND movies.tmdb_id IS excluded.tmdb_id
                                      THEN movies.duration_secs END,
                   container=excluded.container, tmdb_id=excluded.tmdb_id,
                   stream_url=excluded.stream_url,
                   http_user_agent=excluded.http_user_agent,
                   http_referrer=excluded.http_referrer, deleted_at=NULL
                 -- A title the provider lists as it was is left alone: a
                 -- write of every row would rewrite each index for nothing.
                 WHERE movies.deleted_at IS NOT NULL
                    OR movies.category_id IS NOT excluded.category_id
                    OR movies.name IS NOT excluded.name
                    OR movies.clean_name IS NOT excluded.clean_name
                    OR movies.year IS NOT excluded.year
                    OR movies.rating IS NOT excluded.rating
                    OR movies.poster_url IS NOT excluded.poster_url
                    OR (?8 IS NOT NULL AND movies.added IS NOT ?8)
                    OR movies.container IS NOT excluded.container
                    OR movies.tmdb_id IS NOT excluded.tmdb_id
                    OR movies.stream_url IS NOT excluded.stream_url
                    OR movies.http_user_agent IS NOT excluded.http_user_agent
                    OR movies.http_referrer IS NOT excluded.http_referrer",
            )?
            .execute(params![
                m.movie_id,
                category,
                m.name,
                clean_name(&m.name),
                year,
                m.rating,
                m.poster_url,
                added,
                m.container,
                m.tmdb_id,
                m.stream_url,
                m.user_agent,
                m.referrer,
            ])?;
        if self.seen.insert(m.movie_id) {
            if self.existing.contains(&m.movie_id) {
                self.stats.updated += 1;
            } else {
                self.stats.inserted += 1;
            }
        }
        Ok(())
    }

    /// Stamps new undated titles, soft-deletes what was not seen and replaces
    /// the movie categories, without committing. Returns the stats and how
    /// many categories there now are.
    ///
    /// An empty list while movies are mirrored is refused: an empty answer
    /// is far more likely a provider's bad minute than a library emptied, and
    /// accepting it would hide every title.
    pub fn finish(self, now: i64) -> Result<(SyncStats, usize)> {
        let Self {
            conn,
            existing,
            seen,
            categories,
            mut stats,
            empty_ok,
            ..
        } = self;
        let (origin, switched) = current_origin(conn, MOVIES_ORIGIN)?;
        // A different account's list may well be empty, and then the old
        // one's titles must go; the same account's empty answer is a bad
        // minute and changes nothing.
        if seen.is_empty() && !existing.is_empty() && !switched && !empty_ok {
            return Err(anyhow!(
                "the movie list was empty; keeping the {} already here",
                existing.len()
            ));
        }
        if switched {
            // Details and runtimes are the old account's, and a title the
            // new one reuses an id for is not the same film.
            conn.execute("DELETE FROM movie_details", [])?;
            conn.execute("UPDATE movies SET duration_secs = NULL", [])?;
        }
        record_origin(conn, MOVIES_ORIGIN, &origin)?;
        conn.execute(
            "UPDATE movies SET added = ?1 WHERE added = 0 AND deleted_at IS NULL",
            [now],
        )?;
        let mut soft_delete =
            conn.prepare_cached("UPDATE movies SET deleted_at=?1 WHERE movie_id=?2")?;
        for gone in existing.difference(&seen) {
            soft_delete.execute(params![now, gone])?;
            stats.deleted += 1;
        }
        categories.replace(conn, "movie")?;
        Ok((stats, categories.len()))
    }
}

/// Lands an Xtream server's movie categories and movies in one transaction.
pub fn land_movies(
    conn: &Connection,
    categories: &[Category],
    movies: &[NewMovie],
    now: i64,
) -> Result<SyncStats> {
    let tx = conn.unchecked_transaction()?;
    let mut w = MovieWriter::begin(&tx)?;
    w.set_categories(categories);
    for m in movies {
        w.push(m, None)?;
    }
    let (stats, _) = w.finish(now)?;
    tx.commit()?;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::open_in_memory;

    fn cat(id: &str, name: &str) -> Category {
        Category {
            category_id: id.into(),
            category_name: name.into(),
        }
    }

    fn movie(id: i64, cat: &str, name: &str) -> NewMovie {
        NewMovie {
            movie_id: id,
            category_id: Some(cat.into()),
            name: name.into(),
            ..NewMovie::default()
        }
    }

    fn three() -> (Vec<Category>, Vec<NewMovie>) {
        (
            vec![cat("a", "First"), cat("b", "Second")],
            vec![
                movie(1, "a", "One"),
                movie(2, "a", "Two"),
                movie(3, "b", "Three"),
            ],
        )
    }

    #[test]
    fn the_year_comes_from_the_name_when_the_list_has_none() {
        assert_eq!(year_from_name("A Film (2021)"), Some(2021));
        assert_eq!(year_from_name("A Film - 1999"), Some(1999));
        assert_eq!(year_from_name("Room 2049"), None);
        assert_eq!(year_from_name("A Film (1850)"), None);
        assert_eq!(year_from_name("A Film (Director's Cut)"), None);
    }

    #[test]
    fn movies_land_with_their_categories_and_resync_without_duplicates() {
        let c = open_in_memory().unwrap();
        let (cats, movies) = three();
        let s = land_movies(&c, &cats, &movies, 100).unwrap();
        assert_eq!((s.inserted, s.updated, s.deleted), (3, 0, 0));
        let s = land_movies(&c, &cats, &movies, 200).unwrap();
        assert_eq!((s.inserted, s.updated, s.deleted), (0, 3, 0));
        let n: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM vod_categories WHERE kind = 'movie'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn a_title_listed_as_it_was_is_not_written_again() {
        let c = open_in_memory().unwrap();
        let (cats, mut movies) = three();
        movies[0].added = Some(50);
        land_movies(&c, &cats, &movies, 100).unwrap();
        c.execute("UPDATE movies SET deleted_at = 150 WHERE movie_id = 3", [])
            .unwrap();
        // What each upsert changed, the one statement it ends with.
        let upserts = |movies: &[NewMovie]| -> (Vec<u64>, SyncStats) {
            let tx = c.unchecked_transaction().unwrap();
            let mut w = MovieWriter::begin(&tx).unwrap();
            w.set_categories(&cats);
            let mut changed = Vec::new();
            for m in movies {
                w.push(m, None).unwrap();
                changed.push(tx.changes());
            }
            let (stats, _) = w.finish(200).unwrap();
            tx.commit().unwrap();
            (changed, stats)
        };
        // The deleted one comes back; the other two are as they were.
        let (changed, s) = upserts(&movies);
        assert_eq!(changed, [0, 0, 1]);
        assert_eq!((s.inserted, s.updated, s.deleted), (1, 2, 0));
        // An identical list writes nothing, and still counts every title.
        let (changed, s) = upserts(&movies);
        assert_eq!(changed, [0, 0, 0]);
        assert_eq!((s.inserted, s.updated, s.deleted), (0, 3, 0));
        // A change to any column is written.
        movies[1].rating = Some(7.5);
        movies[0].added = Some(60);
        let (changed, s) = upserts(&movies);
        assert_eq!(changed, [1, 1, 0]);
        assert_eq!((s.inserted, s.updated, s.deleted), (0, 3, 0));
        let added: i64 = c
            .query_row("SELECT added FROM movies WHERE movie_id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(added, 60);
    }

    #[test]
    fn a_movie_the_server_stopped_listing_is_soft_deleted_and_its_marks_kept() {
        let c = open_in_memory().unwrap();
        let (cats, movies) = three();
        land_movies(&c, &cats, &movies, 100).unwrap();
        c.execute(
            "INSERT INTO vod_marks (origin, kind, item_id, favourite_at)
             VALUES ('', 'movie', 2, 5)",
            [],
        )
        .unwrap();
        let s = land_movies(&c, &cats, &[movies[0].clone(), movies[2].clone()], 200).unwrap();
        assert_eq!(s.deleted, 1);
        let gone: Option<i64> = c
            .query_row(
                "SELECT deleted_at FROM movies WHERE movie_id = 2",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(gone, Some(200));
        let marks: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM vod_marks WHERE item_id = 2",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(marks, 1);
    }

    #[test]
    fn an_empty_movie_list_keeps_the_movies_already_here() {
        let c = open_in_memory().unwrap();
        let (cats, movies) = three();
        land_movies(&c, &cats, &movies, 100).unwrap();
        let err = land_movies(&c, &cats, &[], 200).unwrap_err().to_string();
        assert!(err.contains("keeping the 3"), "{err}");
        let live: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM movies WHERE deleted_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(live, 3);

        let fresh = open_in_memory().unwrap();
        let s = land_movies(&fresh, &[], &[], 200).unwrap();
        assert_eq!((s.inserted, s.updated, s.deleted), (0, 0, 0));
    }

    fn set_origin(c: &Connection, o: &str) {
        c.execute(
            "INSERT INTO meta (key, value) VALUES ('catalogue_origin', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [o],
        )
        .unwrap();
    }

    fn live(c: &Connection) -> i64 {
        c.query_row(
            "SELECT COUNT(*) FROM movies WHERE deleted_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn a_new_account_with_no_movies_empties_the_old_ones() {
        let c = open_in_memory().unwrap();
        let (cats, movies) = three();
        set_origin(&c, "A");
        land_movies(&c, &cats, &movies, 100).unwrap();
        // The same account answering nothing is refused...
        assert!(land_movies(&c, &cats, &[], 150).is_err());
        assert_eq!(live(&c), 3);
        // ...a different one's is believed.
        set_origin(&c, "B");
        let s = land_movies(&c, &[], &[], 200).unwrap();
        assert_eq!(s.deleted, 3);
        assert_eq!(live(&c), 0);
        let cats: i64 = c
            .query_row("SELECT COUNT(*) FROM vod_categories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cats, 0);
    }

    #[test]
    fn details_and_runtimes_do_not_cross_accounts_or_renamed_titles() {
        let c = open_in_memory().unwrap();
        let (cats, movies) = three();
        set_origin(&c, "A");
        land_movies(&c, &cats, &movies, 100).unwrap();
        let details = |c: &Connection| -> i64 {
            c.query_row("SELECT COUNT(*) FROM movie_details", [], |r| r.get(0))
                .unwrap()
        };
        let runtime = |c: &Connection, id: i64| -> Option<i64> {
            c.query_row(
                "SELECT duration_secs FROM movies WHERE movie_id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap()
        };
        for id in 1..=3 {
            c.execute("INSERT INTO movie_details VALUES (?1, 1, '{}')", [id])
                .unwrap();
        }
        c.execute("UPDATE movies SET duration_secs = 60", [])
            .unwrap();
        // Unchanged titles keep theirs; a renamed one starts over.
        let mut again = movies.clone();
        again[0].name = "Another".into();
        land_movies(&c, &cats, &again, 150).unwrap();
        assert_eq!(
            (details(&c), runtime(&c, 1), runtime(&c, 2)),
            (2, None, Some(60))
        );
        // A new account clears the rest.
        set_origin(&c, "B");
        land_movies(&c, &cats, &movies, 200).unwrap();
        assert_eq!((details(&c), runtime(&c, 2)), (0, None));
    }

    #[test]
    fn the_interface_is_told_when_the_movies_are_another_accounts() {
        let c = open_in_memory().unwrap();
        assert_eq!(movies_origin(&c).unwrap(), "", "none landed yet");
        let (cats, movies) = three();
        set_origin(&c, "xtream:00000000000000aa");
        land_movies(&c, &cats, &movies, 100).unwrap();
        let a = movies_origin(&c).unwrap();
        assert_eq!(a.len(), 16);
        assert!(!a.contains("xtream") && !a.contains("aa"), "{a}");
        land_movies(&c, &cats, &movies, 150).unwrap();
        assert_eq!(movies_origin(&c).unwrap(), a, "the same account's sync");
        set_origin(&c, "xtream:00000000000000bb");
        land_movies(&c, &cats, &movies, 200).unwrap();
        assert_ne!(movies_origin(&c).unwrap(), a);
    }

    #[test]
    fn a_zero_date_from_a_playlist_is_no_date() {
        let c = open_in_memory().unwrap();
        let (cats, mut movies) = three();
        movies[0].added = Some(0);
        land_movies(&c, &cats, &movies, 100).unwrap();
        land_movies(&c, &cats, &movies, 200).unwrap();
        let added: i64 = c
            .query_row("SELECT added FROM movies WHERE movie_id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(added, 100);
    }

    #[test]
    fn added_is_the_providers_or_when_first_seen_and_never_moves() {
        let c = open_in_memory().unwrap();
        let (cats, mut movies) = three();
        movies[1].added = Some(50);
        land_movies(&c, &cats, &movies, 100).unwrap();
        land_movies(&c, &cats, &movies, 200).unwrap();
        let added = |id: i64| -> i64 {
            c.query_row("SELECT added FROM movies WHERE movie_id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap()
        };
        assert_eq!((added(1), added(2)), (100, 50));
    }

    #[test]
    fn separators_are_skipped_and_the_name_supplies_a_missing_year() {
        let c = open_in_memory().unwrap();
        let movies = [
            movie(1, "a", "###### ROW ######"),
            movie(2, "a", "Film (2020)"),
        ];
        land_movies(&c, &[cat("a", "A")], &movies, 1).unwrap();
        let rows: Vec<(i64, Option<i32>)> = c
            .prepare("SELECT movie_id, year FROM movies")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(rows, vec![(2, Some(2020))]);
    }

    #[test]
    fn a_writer_makes_categories_of_the_groups_in_order_of_appearance() {
        let c = open_in_memory().unwrap();
        let tx = c.unchecked_transaction().unwrap();
        let mut w = MovieWriter::begin(&tx).unwrap();
        w.push(&movie(1, "", "One"), Some("Zeta")).unwrap();
        w.push(&movie(2, "", "Two"), Some("Alpha")).unwrap();
        w.push(&movie(3, "", "Three"), Some("Zeta")).unwrap();
        let (stats, cats) = w.finish(10).unwrap();
        tx.commit().unwrap();
        assert_eq!((stats.inserted, cats), (3, 2));
        let names: Vec<String> = c
            .prepare("SELECT name FROM vod_categories ORDER BY sort")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(names, ["Zeta", "Alpha"]);
    }
}
