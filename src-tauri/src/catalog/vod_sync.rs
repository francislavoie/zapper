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

fn plausible_year(y: &i32) -> bool {
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

fn is_separator(name: &str) -> bool {
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
    /// `(id, name)` in the order they are shown.
    categories: Vec<(String, String)>,
    category_ids: HashSet<String>,
    stats: SyncStats,
}

impl<'t> MovieWriter<'t> {
    pub fn begin(tx: &'t rusqlite::Transaction<'_>) -> Result<Self> {
        let conn: &Connection = tx;
        let existing: HashSet<i64> = {
            let mut s = conn.prepare("SELECT movie_id FROM movies WHERE deleted_at IS NULL")?;
            let ids = s.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
            ids
        };
        Ok(Self {
            conn,
            existing,
            seen: HashSet::new(),
            categories: Vec::new(),
            category_ids: HashSet::new(),
            stats: SyncStats::default(),
        })
    }

    /// Names the categories as the source lists them, for a source that has
    /// a list of its own rather than groups.
    fn set_categories(&mut self, cats: &[Category]) {
        for c in cats {
            if self.category_ids.insert(c.category_id.clone()) {
                self.categories
                    .push((c.category_id.clone(), c.category_name.clone()));
            }
        }
    }

    /// Writes one movie. `group` is the playlist group it sits in, which
    /// becomes its category (in order of first appearance); a source with a
    /// category list leaves it `None` and the movie's own `category_id` stands.
    pub fn push(&mut self, m: &NewMovie, group: Option<&str>) -> Result<()> {
        if is_separator(&m.name) {
            return Ok(());
        }
        let category = match group {
            Some(g) => {
                let id = ids::category_id(g);
                if self.category_ids.insert(id.clone()) {
                    self.categories.push((id.clone(), g.to_owned()));
                }
                Some(id)
            }
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
                   http_referrer=excluded.http_referrer, deleted_at=NULL",
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
            ..
        } = self;
        // Which catalogue the mirror's movies came from is kept apart from
        // the channels' record, because the channels are written first and
        // will already have recorded the new one.
        let origin = crate::catalog::origin::recorded(conn)?.unwrap_or_default();
        let landed: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                [MOVIES_ORIGIN],
                |r| r.get(0),
            )
            .optional()?;
        let switched = landed.as_deref().is_some_and(|l| l != origin);
        // A different account's list may well be empty, and then the old
        // one's titles must go; the same account's empty answer is a bad
        // minute and changes nothing.
        if seen.is_empty() && !existing.is_empty() && !switched {
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
        conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![MOVIES_ORIGIN, origin],
        )?;
        conn.execute("UPDATE movies SET added = ?1 WHERE added = 0", [now])?;
        let mut soft_delete =
            conn.prepare_cached("UPDATE movies SET deleted_at=?1 WHERE movie_id=?2")?;
        for gone in existing.difference(&seen) {
            soft_delete.execute(params![now, gone])?;
            stats.deleted += 1;
        }
        // Derived from what the source says each time, so one it dropped is
        // not a category any more.
        conn.execute("DELETE FROM vod_categories WHERE kind = 'movie'", [])?;
        let mut insert = conn.prepare_cached(
            "INSERT INTO vod_categories (kind, id, name, sort) VALUES ('movie', ?1, ?2, ?3)",
        )?;
        for (i, (id, name)) in categories.iter().enumerate() {
            insert.execute(params![id, name, i as i64])?;
        }
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
