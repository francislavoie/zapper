//! Reading and marking the movie mirror.
//!
//! Every list is a page: the catalogue is ~160k titles and nothing here
//! returns them all. The page queries are shaped to be served by the
//! `movies_live_by_*` and `movies_by_category` indexes (see the `vod`
//! migration), and a test holds them to it.

use anyhow::Result;
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};

use crate::catalog::origin::current_origin_sql;
use crate::catalog::series_store;
use crate::catalog::vod_progress::is_finished;

/// "Recently added" is this many days back.
pub const RECENT_DAYS: i64 = 30;

/// What a mark is on: `vod_marks.kind`. Each kind has its own id space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemKind {
    Movie,
    Series,
    Episode,
}

impl ItemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Series => "series",
            Self::Episode => "episode",
        }
    }
}

/// One title that plays: a movie or an episode, never confused with the
/// other kind's equal id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VodItem {
    pub kind: ItemKind,
    pub id: i64,
}

impl VodItem {
    pub fn movie(id: i64) -> Self {
        Self {
            kind: ItemKind::Movie,
            id,
        }
    }

    pub fn episode(id: i64) -> Self {
        Self {
            kind: ItemKind::Episode,
            id,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct VodCategoryRow {
    pub id: String,
    pub name: String,
    pub movie_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MovieList {
    Favourites,
    Recent,
    Watched,
    Category(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MovieSort {
    Added,
    Az,
    Year,
    Rating,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovieQuery {
    pub list: MovieList,
    pub sort: MovieSort,
    pub year_from: Option<i32>,
    pub year_to: Option<i32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MovieRow {
    pub movie_id: i64,
    pub name: String,
    pub clean_name: String,
    pub year: Option<i32>,
    pub rating: Option<f64>,
    pub poster_url: Option<String>,
    pub added: Option<i64>,
    pub duration_secs: Option<i64>,
    pub category_id: Option<String>,
    pub favourite: bool,
    pub watched: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MovieCounts {
    pub total: u32,
    pub favourites: u32,
    pub recent: u32,
    pub watched: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovieSource {
    pub container: Option<String>,
    pub stream_url: Option<String>,
    pub user_agent: Option<String>,
    pub referrer: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Marks {
    pub favourite: bool,
    pub watched: bool,
    pub position: Option<f64>,
    pub duration: Option<f64>,
}

const COLUMNS: &str = "m.movie_id, m.name, m.clean_name, m.year, m.rating, m.poster_url,
    m.added, m.duration_secs, m.category_id,
    k.favourite_at IS NOT NULL, k.watched_at IS NOT NULL";

/// Movies with the current catalogue's marks beside them.
const MARKED: &str = concat!(
    "movies m LEFT JOIN vod_marks k ON k.origin = ",
    current_origin_sql!(),
    " AND k.kind = 'movie' AND k.item_id = m.movie_id"
);

/// The marks first, for the lists that are marks: a few rows to look up in
/// the movies, rather than every movie to look up a mark for.
const MARKS_FIRST: &str = concat!(
    "vod_marks k JOIN movies m ON m.movie_id = k.item_id AND k.origin = ",
    current_origin_sql!(),
    " AND k.kind = 'movie'"
);

fn row(r: &Row) -> rusqlite::Result<MovieRow> {
    Ok(MovieRow {
        movie_id: r.get(0)?,
        name: r.get(1)?,
        clean_name: r.get(2)?,
        year: r.get(3)?,
        rating: r.get(4)?,
        poster_url: r.get(5)?,
        added: r.get(6)?,
        duration_secs: r.get(7)?,
        category_id: r.get(8)?,
        favourite: r.get(9)?,
        watched: r.get(10)?,
    })
}

pub fn movie_categories(conn: &Connection) -> Result<Vec<VodCategoryRow>> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.name,
                (SELECT COUNT(*) FROM movies m
                 WHERE m.category_id = c.id AND m.deleted_at IS NULL)
         FROM vod_categories c WHERE c.kind = 'movie' ORDER BY c.sort, c.id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(VodCategoryRow {
            id: r.get(0)?,
            name: r.get(1)?,
            movie_count: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The `FROM`, the `WHERE` conditions and their parameters for a query.
fn scope(q: &MovieQuery, now: i64) -> (&'static str, Vec<String>, Vec<Value>) {
    let mut conds = vec!["m.deleted_at IS NULL".to_string()];
    let mut args = Vec::new();
    let from = match &q.list {
        MovieList::Favourites => {
            conds.push("k.favourite_at IS NOT NULL".into());
            MARKS_FIRST
        }
        MovieList::Watched => {
            conds.push("k.watched_at IS NOT NULL".into());
            MARKS_FIRST
        }
        MovieList::Recent => {
            conds.push("m.added >= ?".into());
            args.push(Value::Integer(now - RECENT_DAYS * 86_400));
            MARKED
        }
        MovieList::Category(id) => {
            conds.push("m.category_id = ?".into());
            args.push(Value::Text(id.clone()));
            MARKED
        }
    };
    if let Some(y) = q.year_from {
        conds.push("m.year >= ?".into());
        args.push(Value::Integer(y.into()));
    }
    if let Some(y) = q.year_to {
        conds.push("m.year <= ?".into());
        args.push(Value::Integer(y.into()));
    }
    (from, conds, args)
}

fn order_by(sort: MovieSort) -> &'static str {
    match sort {
        MovieSort::Added => "m.added DESC, m.movie_id DESC",
        MovieSort::Az => "m.clean_name COLLATE NOCASE, m.movie_id",
        MovieSort::Year => "m.year DESC NULLS LAST, m.clean_name",
        MovieSort::Rating => "m.rating DESC NULLS LAST, m.clean_name",
    }
}

fn page_sql(q: &MovieQuery, offset: u32, limit: u32, now: i64) -> (String, Vec<Value>) {
    let (from, conds, mut args) = scope(q, now);
    args.push(Value::Integer(limit.into()));
    args.push(Value::Integer(offset.into()));
    let sql = format!(
        "SELECT {COLUMNS} FROM {from} WHERE {} ORDER BY {} LIMIT ? OFFSET ?",
        conds.join(" AND "),
        order_by(q.sort)
    );
    (sql, args)
}

/// One page and how many there are in all.
pub fn movies_page(
    conn: &Connection,
    q: &MovieQuery,
    offset: u32,
    limit: u32,
    now: i64,
) -> Result<(u32, Vec<MovieRow>)> {
    let (from, conds, args) = scope(q, now);
    // Only a list of marks needs the marks to count; joining them for the
    // others costs several times as much on a large catalogue.
    let count_from = match q.list {
        MovieList::Favourites | MovieList::Watched => from,
        _ => "movies m",
    };
    let total: u32 = conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM {count_from} WHERE {}",
            conds.join(" AND ")
        ),
        params_from_iter(args),
        |r| r.get(0),
    )?;
    let (sql, args) = page_sql(q, offset, limit, now);
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params_from_iter(args), row)?
        .collect::<rusqlite::Result<_>>()?;
    Ok((total, rows))
}

/// In the order of `ids`; ids that are unknown or deleted are left out.
pub fn movies_by_ids(conn: &Connection, ids: &[i64]) -> Result<Vec<MovieRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM {MARKED} WHERE m.movie_id = ?1 AND m.deleted_at IS NULL"
    ))?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(r) = stmt.query_row([id], row).optional()? {
            out.push(r);
        }
    }
    Ok(out)
}

/// Others in the same category, within three years of this one's, best rated
/// first. A title with no year is compared on category alone.
pub fn similar_movies(conn: &Connection, id: i64, limit: u32) -> Result<Vec<MovieRow>> {
    let this: Option<(Option<String>, Option<i32>)> = conn
        .query_row(
            "SELECT category_id, year FROM movies WHERE movie_id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((Some(category), year)) = this else {
        return Ok(Vec::new());
    };
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM {MARKED}
         WHERE m.category_id = ?1 AND m.deleted_at IS NULL AND m.movie_id <> ?2
           AND (?3 IS NULL OR m.year BETWEEN ?3 - 3 AND ?3 + 3)
         ORDER BY m.rating DESC NULLS LAST, m.clean_name LIMIT ?4"
    ))?;
    let rows = stmt
        .query_map(params![category, id, year, limit], row)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

pub fn movie_counts(conn: &Connection, now: i64) -> Result<MovieCounts> {
    let total = conn.query_row(
        "SELECT COUNT(*) FROM movies WHERE deleted_at IS NULL",
        [],
        |r| r.get(0),
    )?;
    let recent = conn.query_row(
        "SELECT COUNT(*) FROM movies WHERE deleted_at IS NULL AND added >= ?1",
        [now - RECENT_DAYS * 86_400],
        |r| r.get(0),
    )?;
    let marked = |column: &str| -> rusqlite::Result<u32> {
        conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM {MARKS_FIRST}
                 WHERE m.deleted_at IS NULL AND k.{column} IS NOT NULL"
            ),
            [],
            |r| r.get(0),
        )
    };
    Ok(MovieCounts {
        total,
        favourites: marked("favourite_at")?,
        recent,
        watched: marked("watched_at")?,
    })
}

/// What the search index is built from.
pub fn movie_names(conn: &Connection) -> Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT movie_id, CASE WHEN clean_name <> '' THEN clean_name ELSE name END
         FROM movies WHERE deleted_at IS NULL",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// What playing a movie needs, or None for one that is unknown or deleted.
pub fn movie_source(conn: &Connection, id: i64) -> Result<Option<MovieSource>> {
    Ok(conn
        .query_row(
            "SELECT container, stream_url, http_user_agent, http_referrer
             FROM movies WHERE movie_id = ?1 AND deleted_at IS NULL",
            [id],
            |r| {
                Ok(MovieSource {
                    container: r.get(0)?,
                    stream_url: r.get(1)?,
                    user_agent: r.get(2)?,
                    referrer: r.get(3)?,
                })
            },
        )
        .optional()?)
}

/// The details last fetched for a title: when, and the JSON.
pub fn cached_details(conn: &Connection, id: i64) -> Result<Option<(i64, String)>> {
    Ok(conn
        .query_row(
            "SELECT fetched_at, info FROM movie_details WHERE movie_id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

/// Caches a title's details, and its runtime in seconds when they say one,
/// which the lists then show.
pub fn store_details(
    conn: &Connection,
    id: i64,
    at: i64,
    info: &str,
    runtime: Option<i64>,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO movie_details (movie_id, fetched_at, info) VALUES (?1, ?2, ?3)
         ON CONFLICT(movie_id) DO UPDATE SET fetched_at=excluded.fetched_at, info=excluded.info",
        params![id, at, info],
    )?;
    if let Some(secs) = runtime {
        tx.execute(
            "UPDATE movies SET duration_secs = ?2 WHERE movie_id = ?1",
            params![id, secs],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub(crate) fn origin(conn: &Connection) -> Result<String> {
    Ok(crate::catalog::origin::recorded(conn)?.unwrap_or_default())
}

pub(crate) fn marks_of(conn: &Connection, origin: &str, kind: ItemKind, id: i64) -> Result<Marks> {
    Ok(conn
        .query_row(
            "SELECT favourite_at IS NOT NULL, watched_at IS NOT NULL, position, duration
             FROM vod_marks WHERE origin = ?1 AND kind = ?2 AND item_id = ?3",
            params![origin, kind.as_str(), id],
            |r| {
                Ok(Marks {
                    favourite: r.get(0)?,
                    watched: r.get(1)?,
                    position: r.get(2)?,
                    duration: r.get(3)?,
                })
            },
        )
        .optional()?
        .unwrap_or_default())
}

/// Sets or clears the favourite and watched marks (`None` leaves one alone).
/// Marking watched keeps the time it was first marked; un-marking it starts
/// the title over, so it clears the resume point too. An episode's watched
/// mark also reaches the episodes a multi-episode entry covers and its
/// show's ([`series_store::episode_marked_in_tx`]), in the same transaction.
pub fn set_marks(
    conn: &Connection,
    kind: ItemKind,
    id: i64,
    favourite: Option<bool>,
    watched: Option<bool>,
    at: i64,
) -> Result<Marks> {
    let origin = origin(conn)?;
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT OR IGNORE INTO vod_marks (origin, kind, item_id) VALUES (?1, ?2, ?3)",
        params![origin, kind.as_str(), id],
    )?;
    if let Some(f) = favourite {
        tx.execute(
            "UPDATE vod_marks SET favourite_at = CASE WHEN ?4 THEN COALESCE(favourite_at, ?5)
                                                     ELSE NULL END
             WHERE origin = ?1 AND kind = ?2 AND item_id = ?3",
            params![origin, kind.as_str(), id, f, at],
        )?;
    }
    match watched {
        Some(true) => {
            tx.execute(
                "UPDATE vod_marks SET watched_at = COALESCE(watched_at, ?4)
                 WHERE origin = ?1 AND kind = ?2 AND item_id = ?3",
                params![origin, kind.as_str(), id, at],
            )?;
        }
        Some(false) => {
            tx.execute(
                "UPDATE vod_marks SET watched_at = NULL, position = NULL
                 WHERE origin = ?1 AND kind = ?2 AND item_id = ?3",
                params![origin, kind.as_str(), id],
            )?;
        }
        None => {}
    }
    if kind == ItemKind::Episode && watched.is_some() {
        series_store::episode_marked_in_tx(&tx, id, at)?;
    }
    let marks = marks_of(&tx, &origin, kind, id)?;
    tx.commit()?;
    Ok(marks)
}

/// Writes down how far into a title the player is. Keeps the longest length
/// ever reported, since a stream's first reports can under-state it, and
/// marks the title watched the first time the position is past the line
/// (an episode's then reaching what it covers and its show, as in
/// [`set_marks`]; see [`series_store::episode_played_in_tx`]).
pub fn record_progress(
    conn: &Connection,
    kind: ItemKind,
    id: i64,
    position: f64,
    duration: Option<f64>,
    at: i64,
) -> Result<Marks> {
    let origin = origin(conn)?;
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT OR IGNORE INTO vod_marks (origin, kind, item_id) VALUES (?1, ?2, ?3)",
        params![origin, kind.as_str(), id],
    )?;
    let before = marks_of(&tx, &origin, kind, id)?;
    let known = before.duration;
    let duration = match (known, duration) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
    let finished = is_finished(position, duration);
    tx.execute(
        "UPDATE vod_marks SET position = ?4, duration = ?5, played_at = ?6,
                              watched_at = CASE WHEN ?7 THEN COALESCE(watched_at, ?6)
                                                ELSE watched_at END
         WHERE origin = ?1 AND kind = ?2 AND item_id = ?3",
        params![origin, kind.as_str(), id, position, duration, at, finished],
    )?;
    if kind == ItemKind::Episode {
        let became_watched = finished && !before.watched;
        series_store::episode_played_in_tx(&tx, id, became_watched, at)?;
    }
    let marks = marks_of(&tx, &origin, kind, id)?;
    tx.commit()?;
    Ok(marks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::open_in_memory;

    const DAY: i64 = 86_400;
    const NOW: i64 = 100 * DAY;

    fn add(
        c: &Connection,
        id: i64,
        cat: &str,
        name: &str,
        year: Option<i32>,
        rating: Option<f64>,
        added: i64,
    ) {
        c.execute(
            "INSERT INTO movies (movie_id, category_id, name, clean_name, year, rating, added)
             VALUES (?1, ?2, ?3, ?3, ?4, ?5, ?6)",
            params![id, cat, name, year, rating, added],
        )
        .unwrap();
    }

    fn q(list: MovieList, sort: MovieSort) -> MovieQuery {
        MovieQuery {
            list,
            sort,
            year_from: None,
            year_to: None,
        }
    }

    fn ids(c: &Connection, q: &MovieQuery) -> Vec<i64> {
        movies_page(c, q, 0, 100, NOW)
            .unwrap()
            .1
            .iter()
            .map(|m| m.movie_id)
            .collect()
    }

    fn cat(s: &str) -> MovieList {
        MovieList::Category(s.into())
    }

    #[test]
    fn a_page_says_how_many_there_are_in_all() {
        let c = open_in_memory().unwrap();
        for i in 0..25 {
            add(&c, i, "c", &format!("M{i:02}"), None, None, i);
        }
        let (total, items) = movies_page(&c, &q(cat("c"), MovieSort::Az), 20, 10, NOW).unwrap();
        assert_eq!((total, items.len()), (25, 5));
        assert_eq!(items[0].name, "M20");
    }

    #[test]
    fn each_sort_orders_as_named() {
        let c = open_in_memory().unwrap();
        add(&c, 1, "c", "bravo", Some(2000), Some(5.0), 10);
        add(&c, 2, "c", "Alpha", None, None, 30);
        add(&c, 3, "c", "charlie", Some(2010), Some(8.0), 20);
        assert_eq!(ids(&c, &q(cat("c"), MovieSort::Added)), [2, 3, 1]);
        assert_eq!(ids(&c, &q(cat("c"), MovieSort::Az)), [2, 1, 3]);
        assert_eq!(ids(&c, &q(cat("c"), MovieSort::Year)), [3, 1, 2]);
        assert_eq!(ids(&c, &q(cat("c"), MovieSort::Rating)), [3, 1, 2]);
    }

    #[test]
    fn the_year_range_bounds_both_ends() {
        let c = open_in_memory().unwrap();
        add(&c, 1, "c", "a", Some(1979), None, 1);
        add(&c, 2, "c", "b", Some(1985), None, 2);
        add(&c, 3, "c", "c", Some(2021), None, 3);
        let mut query = q(cat("c"), MovieSort::Added);
        query.year_from = Some(1980);
        query.year_to = Some(1989);
        assert_eq!(ids(&c, &query), [2]);
    }

    #[test]
    fn recently_added_is_the_last_thirty_days() {
        let c = open_in_memory().unwrap();
        add(&c, 1, "c", "in", None, None, NOW - 29 * DAY);
        add(&c, 2, "c", "out", None, None, NOW - 31 * DAY);
        assert_eq!(ids(&c, &q(MovieList::Recent, MovieSort::Added)), [1]);
        assert_eq!(movie_counts(&c, NOW).unwrap().recent, 1);
    }

    #[test]
    fn favourites_and_watched_belong_to_the_catalogue_that_marked_them() {
        let c = open_in_memory().unwrap();
        add(&c, 1, "c", "a", None, None, 1);
        add(&c, 2, "c", "b", None, None, 2);
        let switch = |o: &str| {
            c.execute(
                "INSERT INTO meta (key, value) VALUES ('catalogue_origin', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [o],
            )
            .unwrap();
        };
        switch("A");
        set_marks(&c, ItemKind::Movie, 1, Some(true), None, 5).unwrap();
        set_marks(&c, ItemKind::Movie, 2, None, Some(true), 5).unwrap();
        let favs = q(MovieList::Favourites, MovieSort::Added);
        let seen = q(MovieList::Watched, MovieSort::Added);
        assert_eq!((ids(&c, &favs), ids(&c, &seen)), (vec![1], vec![2]));
        switch("B");
        assert!(ids(&c, &favs).is_empty() && ids(&c, &seen).is_empty());
        let counts = movie_counts(&c, NOW).unwrap();
        assert_eq!((counts.favourites, counts.watched), (0, 0));
        assert!(!movies_by_ids(&c, &[1]).unwrap()[0].favourite);
        switch("A");
        assert_eq!(ids(&c, &favs), [1]);
        assert!(movies_by_ids(&c, &[1]).unwrap()[0].favourite);
        assert_eq!(movie_counts(&c, NOW).unwrap().favourites, 1);
    }

    #[test]
    fn deleted_movies_are_left_out_everywhere() {
        let c = open_in_memory().unwrap();
        add(&c, 1, "c", "a", None, None, 1);
        add(&c, 2, "c", "b", None, None, 2);
        set_marks(&c, ItemKind::Movie, 2, Some(true), None, 5).unwrap();
        c.execute("UPDATE movies SET deleted_at = 9 WHERE movie_id = 2", [])
            .unwrap();
        assert_eq!(ids(&c, &q(cat("c"), MovieSort::Added)), [1]);
        assert!(ids(&c, &q(MovieList::Favourites, MovieSort::Added)).is_empty());
        assert_eq!(movies_by_ids(&c, &[2, 1]).unwrap().len(), 1);
        assert_eq!(movie_counts(&c, NOW).unwrap().total, 1);
        assert_eq!(movie_names(&c).unwrap().len(), 1);
        assert!(movie_source(&c, 2).unwrap().is_none());
        assert_eq!(movie_categories(&c).unwrap().len(), 0);
    }

    #[test]
    fn by_ids_keeps_the_order_asked_for() {
        let c = open_in_memory().unwrap();
        for i in 1..=3 {
            add(&c, i, "c", "x", None, None, i);
        }
        let got: Vec<i64> = movies_by_ids(&c, &[3, 1, 2])
            .unwrap()
            .iter()
            .map(|m| m.movie_id)
            .collect();
        assert_eq!(got, [3, 1, 2]);
    }

    #[test]
    fn more_like_this_is_the_same_category_near_in_year_best_rated_first() {
        let c = open_in_memory().unwrap();
        add(&c, 1, "c", "self", Some(2000), Some(9.0), 1);
        add(&c, 2, "c", "near", Some(2002), Some(6.0), 2);
        add(&c, 3, "c", "nearer", Some(1999), Some(8.0), 3);
        add(&c, 4, "c", "far", Some(2010), Some(9.5), 4);
        add(&c, 5, "d", "elsewhere", Some(2000), Some(9.9), 5);
        let got: Vec<i64> = similar_movies(&c, 1, 10)
            .unwrap()
            .iter()
            .map(|m| m.movie_id)
            .collect();
        assert_eq!(got, [3, 2]);
        assert_eq!(similar_movies(&c, 1, 1).unwrap().len(), 1);
    }

    #[test]
    fn categories_count_their_live_movies() {
        let c = open_in_memory().unwrap();
        c.execute(
            "INSERT INTO vod_categories (kind, id, name, sort) VALUES ('movie','c','Cat',0)",
            [],
        )
        .unwrap();
        add(&c, 1, "c", "a", None, None, 1);
        add(&c, 2, "c", "b", None, None, 2);
        assert_eq!(movie_categories(&c).unwrap()[0].movie_count, 2);
    }

    #[test]
    fn the_movie_page_queries_use_an_index() {
        let c = open_in_memory().unwrap();
        let plan = |q: &MovieQuery| -> String {
            let (sql, args) = page_sql(q, 0, 50, NOW);
            let mut stmt = c.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
            let rows: Vec<String> = stmt
                .query_map(params_from_iter(args), |r| r.get(3))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            rows.join("\n")
        };
        let p = plan(&q(cat("c"), MovieSort::Added));
        assert!(p.contains("movies_by_category"), "{p}");
        assert!(!p.contains("SCAN m"), "{p}");
        assert!(!p.contains("TEMP B-TREE"), "{p}");
        let p = plan(&q(MovieList::Recent, MovieSort::Added));
        assert!(p.contains("movies_live_by_added"), "{p}");
        assert!(!p.contains("SCAN m"), "{p}");
        assert!(!p.contains("TEMP B-TREE"), "{p}");
        let p = plan(&q(MovieList::Favourites, MovieSort::Az));
        assert!(!p.contains("SCAN m"), "{p}");
    }

    #[test]
    fn counting_a_category_reads_the_live_index_and_matches_the_page() {
        let c = open_in_memory().unwrap();
        c.execute(
            "INSERT INTO vod_categories (kind, id, name, sort) VALUES ('movie','c','Cat',0)",
            [],
        )
        .unwrap();
        for i in 0..5 {
            add(&c, i, "c", "x", None, None, NOW - i);
        }
        c.execute("UPDATE movies SET deleted_at = 1 WHERE movie_id = 4", [])
            .unwrap();
        let plan: String = c
            .query_row(
                "EXPLAIN QUERY PLAN SELECT COUNT(*) FROM movies m
                 WHERE m.category_id = 'c' AND m.deleted_at IS NULL",
                [],
                |r| r.get(3),
            )
            .unwrap();
        assert!(plan.contains("movies_live_by_category"), "{plan}");
        assert_eq!(movie_categories(&c).unwrap()[0].movie_count, 4);
        for list in [cat("c"), MovieList::Recent] {
            let (total, items) = movies_page(&c, &q(list, MovieSort::Added), 0, 100, NOW).unwrap();
            assert_eq!((total as usize, items.len()), (4, 4));
        }
    }

    #[test]
    fn details_are_cached_and_set_the_runtime() {
        let c = open_in_memory().unwrap();
        add(&c, 1, "c", "a", None, None, 1);
        assert!(cached_details(&c, 1).unwrap().is_none());
        store_details(&c, 1, 50, "{}", Some(5400)).unwrap();
        store_details(&c, 1, 60, "{\"a\":1}", None).unwrap();
        assert_eq!(
            cached_details(&c, 1).unwrap(),
            Some((60, "{\"a\":1}".into()))
        );
        assert_eq!(
            movies_by_ids(&c, &[1]).unwrap()[0].duration_secs,
            Some(5400)
        );
    }

    #[test]
    fn progress_marks_watched_once_and_unwatching_starts_over() {
        let c = open_in_memory().unwrap();
        let m = record_progress(&c, ItemKind::Movie, 1, 40.0, Some(100.0), 10).unwrap();
        assert!(!m.watched);
        assert_eq!((m.position, m.duration), (Some(40.0), Some(100.0)));
        // A shorter length reported later does not shrink what is known.
        let m = record_progress(&c, ItemKind::Movie, 1, 95.0, Some(90.0), 20).unwrap();
        assert!(m.watched && m.duration == Some(100.0));
        let first: i64 = c
            .query_row("SELECT watched_at FROM vod_marks", [], |r| r.get(0))
            .unwrap();
        record_progress(&c, ItemKind::Movie, 1, 99.0, Some(100.0), 30).unwrap();
        let again: i64 = c
            .query_row("SELECT watched_at FROM vod_marks", [], |r| r.get(0))
            .unwrap();
        assert_eq!((first, again), (20, 20));
        let played: i64 = c
            .query_row("SELECT played_at FROM vod_marks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(played, 30);

        let m = set_marks(&c, ItemKind::Movie, 1, None, Some(false), 40).unwrap();
        assert!(!m.watched);
        assert_eq!(m.position, None);
    }

    #[test]
    fn favouriting_toggles_and_leaves_progress_alone() {
        let c = open_in_memory().unwrap();
        record_progress(&c, ItemKind::Movie, 1, 10.0, Some(100.0), 1).unwrap();
        let m = set_marks(&c, ItemKind::Movie, 1, Some(true), None, 2).unwrap();
        assert!(m.favourite && m.position == Some(10.0));
        let m = set_marks(&c, ItemKind::Movie, 1, Some(false), None, 3).unwrap();
        assert!(!m.favourite && m.position == Some(10.0));
    }
}
