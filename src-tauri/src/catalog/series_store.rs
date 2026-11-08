//! Reading and marking the shows mirror: pages of shows, a show's episodes
//! and seasons, which episode is next up, and what was watched.
//!
//! The lists are the movies' ([`crate::catalog::vod_store`]): paged, served
//! by the `series_live_by_*` indexes (a test holds them to it), each row with
//! the current catalogue's marks beside it. Marks are `vod_marks` rows of kind
//! `series` (★ and watched) and `episode` (watched and how far in).
//!
//! Watched has two levels. An episode is watched as a movie is: marked, or
//! played past the line. A multi-episode entry that becomes watched takes
//! the episodes it covers with it. A show is watched when every one of its
//! known episodes, specials aside, is; that is kept on the show's own mark,
//! so the Watched list stays a cheap query, and refreshed whenever an
//! episode's marks move or a show's episodes land. Every helper here that
//! writes runs inside a transaction its caller opened (`_in_tx`), so marking
//! an episode is one transaction however far it reaches.

use anyhow::Result;
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};
use std::collections::BTreeMap;

use crate::catalog::origin::current_origin_sql;
use crate::catalog::title_search::searched_name_sql;
use crate::catalog::vod_progress::is_started;
use crate::catalog::vod_store::{
    self, ItemKind, Marks, MovieList, MovieQuery, MovieSort, RECENT_DAYS,
};

#[derive(Debug, Clone, PartialEq)]
pub struct SeriesRow {
    pub series_id: i64,
    pub name: String,
    pub clean_name: String,
    pub year: Option<i32>,
    pub rating: Option<f64>,
    pub poster_url: Option<String>,
    pub added: Option<i64>,
    pub category_id: Option<String>,
    /// Blank until the show's episodes have landed once.
    pub season_count: Option<u32>,
    pub episode_count: Option<u32>,
    pub favourite: bool,
    pub watched: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EpisodeRow {
    pub episode_id: i64,
    pub series_id: i64,
    /// 0 for specials.
    pub season: i32,
    pub episode: i32,
    /// A multi-episode entry's last number.
    pub episode_end: Option<i32>,
    pub title: Option<String>,
    pub plot: Option<String>,
    /// The provider's length.
    pub duration_secs: Option<i64>,
    pub still_url: Option<String>,
    pub air_date: Option<String>,
    pub watched: bool,
    pub watched_at: Option<i64>,
    pub position: Option<f64>,
    /// The length the player measured.
    pub duration: Option<f64>,
    pub played_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeasonRow {
    pub number: i32,
    pub name: String,
    pub episode_count: u32,
    pub watched_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SeriesCounts {
    pub total: u32,
    pub favourites: u32,
    pub recent: u32,
    pub watched: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpisodeSource {
    pub series_id: i64,
    pub container: Option<String>,
    pub stream_url: Option<String>,
    pub user_agent: Option<String>,
    pub referrer: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeriesCategoryRow {
    pub id: String,
    pub name: String,
    pub series_count: u32,
}

const COLUMNS: &str = "s.series_id, s.name, s.clean_name, s.year, s.rating, s.poster_url,
    s.added, s.category_id, s.season_count, s.episode_count,
    k.favourite_at IS NOT NULL, k.watched_at IS NOT NULL";

/// Shows with the current catalogue's marks beside them.
const MARKED: &str = concat!(
    "series s LEFT JOIN vod_marks k ON k.origin = ",
    current_origin_sql!(),
    " AND k.kind = 'series' AND k.item_id = s.series_id"
);

/// The marks first, for the lists that are marks.
const MARKS_FIRST: &str = concat!(
    "vod_marks k JOIN series s ON s.series_id = k.item_id AND k.origin = ",
    current_origin_sql!(),
    " AND k.kind = 'series'"
);

const CATEGORIES: &str = "SELECT c.id, c.name,
        (SELECT COUNT(*) FROM series s
         WHERE s.category_id = c.id AND s.deleted_at IS NULL)
    FROM vod_categories c WHERE c.kind = 'series' ORDER BY c.sort, c.id";

/// Episodes with the current catalogue's marks beside them.
macro_rules! episodes_marked {
    () => {
        concat!(
            "SELECT e.episode_id, e.series_id, e.season, e.episode, e.episode_end,
                    e.title, e.plot, e.duration_secs, e.still_url, e.air_date,
                    k.watched_at IS NOT NULL, k.watched_at, k.position, k.duration,
                    k.played_at
             FROM episodes e LEFT JOIN vod_marks k ON k.origin = ",
            current_origin_sql!(),
            " AND k.kind = 'episode' AND k.item_id = e.episode_id"
        )
    };
}

/// A show's live episodes in the index's order (specials, season 0, are
/// moved last afterwards, which the index cannot do).
const EPISODES: &str = concat!(
    episodes_marked!(),
    " WHERE e.series_id = ?1 AND e.deleted_at IS NULL
      ORDER BY e.season, e.episode, e.episode_id"
);

/// The live episodes that episode `?1` covers when it is a multi-episode
/// entry: the same show and season, numbered within its range, itself
/// aside. None for a single episode.
macro_rules! covered_sql {
    () => {
        "SELECT e.episode_id FROM episodes r JOIN episodes e
           ON e.series_id = r.series_id AND e.season = r.season
          AND e.episode BETWEEN r.episode AND r.episode_end
         WHERE r.episode_id = ?1 AND r.episode_end > r.episode
           AND e.episode_id <> r.episode_id AND e.deleted_at IS NULL"
    };
}

fn series_row(r: &Row) -> rusqlite::Result<SeriesRow> {
    Ok(SeriesRow {
        series_id: r.get(0)?,
        name: r.get(1)?,
        clean_name: r.get(2)?,
        year: r.get(3)?,
        rating: r.get(4)?,
        poster_url: r.get(5)?,
        added: r.get(6)?,
        category_id: r.get(7)?,
        season_count: r.get(8)?,
        episode_count: r.get(9)?,
        favourite: r.get(10)?,
        watched: r.get(11)?,
    })
}

fn episode_row(r: &Row) -> rusqlite::Result<EpisodeRow> {
    Ok(EpisodeRow {
        episode_id: r.get(0)?,
        series_id: r.get(1)?,
        season: r.get(2)?,
        episode: r.get(3)?,
        episode_end: r.get(4)?,
        title: r.get(5)?,
        plot: r.get(6)?,
        duration_secs: r.get(7)?,
        still_url: r.get(8)?,
        air_date: r.get(9)?,
        watched: r.get(10)?,
        watched_at: r.get(11)?,
        position: r.get(12)?,
        duration: r.get(13)?,
        played_at: r.get(14)?,
    })
}

pub fn series_categories(conn: &Connection) -> Result<Vec<SeriesCategoryRow>> {
    let mut stmt = conn.prepare(CATEGORIES)?;
    let rows = stmt.query_map([], |r| {
        Ok(SeriesCategoryRow {
            id: r.get(0)?,
            name: r.get(1)?,
            series_count: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The `FROM`, the `WHERE` conditions and their parameters for a query.
fn scope(q: &MovieQuery, now: i64) -> (&'static str, Vec<String>, Vec<Value>) {
    let mut conds = vec!["s.deleted_at IS NULL".to_string()];
    let mut args = Vec::new();
    let from = match &q.list {
        MovieList::All => MARKED,
        MovieList::Favourites => {
            conds.push("k.favourite_at IS NOT NULL".into());
            MARKS_FIRST
        }
        MovieList::Watched => {
            conds.push("k.watched_at IS NOT NULL".into());
            MARKS_FIRST
        }
        MovieList::Recent => {
            conds.push("s.added >= ?".into());
            args.push(Value::Integer(now - RECENT_DAYS * 86_400));
            MARKED
        }
        MovieList::Category(id) => {
            conds.push("s.category_id = ?".into());
            args.push(Value::Text(id.clone()));
            MARKED
        }
    };
    if let Some(y) = q.year_from {
        conds.push("s.year >= ?".into());
        args.push(Value::Integer(y.into()));
    }
    if let Some(y) = q.year_to {
        conds.push("s.year <= ?".into());
        args.push(Value::Integer(y.into()));
    }
    if let Some(text) = &q.search {
        conds.push(concat!("title_match(", searched_name_sql!("s"), ", ?)").into());
        args.push(Value::Text(text.clone()));
    }
    (from, conds, args)
}

fn order_by(sort: MovieSort) -> &'static str {
    match sort {
        MovieSort::Added => "s.added DESC, s.series_id DESC",
        MovieSort::Az => "s.clean_name COLLATE NOCASE, s.series_id",
        MovieSort::Year => "s.year DESC NULLS LAST, s.clean_name, s.series_id",
        MovieSort::Rating => "s.rating DESC NULLS LAST, s.clean_name, s.series_id",
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

/// As the movies': only a list of marks needs the marks to count, and a
/// search of every show reads the names from their index.
fn count_sql(q: &MovieQuery, now: i64) -> (String, Vec<Value>) {
    let (from, conds, args) = scope(q, now);
    let count_from = match (&q.list, &q.search) {
        (MovieList::Favourites | MovieList::Watched, _) => from,
        (MovieList::All, Some(_)) => "series s INDEXED BY series_live_by_name",
        _ => "series s",
    };
    let sql = format!(
        "SELECT COUNT(*) FROM {count_from} WHERE {}",
        conds.join(" AND ")
    );
    (sql, args)
}

/// One page of shows and how many there are in all, with the movies' lists
/// and sorts.
pub fn series_page(
    conn: &Connection,
    q: &MovieQuery,
    offset: u32,
    limit: u32,
    now: i64,
) -> Result<(u32, Vec<SeriesRow>)> {
    let (sql, args) = count_sql(q, now);
    let total: u32 = conn.query_row(&sql, params_from_iter(args), |r| r.get(0))?;
    // As the movies': past the end there is nothing to read.
    if offset >= total {
        return Ok((total, Vec::new()));
    }
    let (sql, args) = page_sql(q, offset, limit, now);
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params_from_iter(args), series_row)?
        .collect::<rusqlite::Result<_>>()?;
    Ok((total, rows))
}

/// In the order of `ids`; ids that are unknown or deleted are left out.
pub fn series_by_ids(conn: &Connection, ids: &[i64]) -> Result<Vec<SeriesRow>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {COLUMNS} FROM {MARKED} WHERE s.series_id = ?1 AND s.deleted_at IS NULL"
    ))?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(r) = stmt.query_row([id], series_row).optional()? {
            out.push(r);
        }
    }
    Ok(out)
}

pub fn series_counts(conn: &Connection, now: i64) -> Result<SeriesCounts> {
    let total = conn.query_row(
        "SELECT COUNT(*) FROM series WHERE deleted_at IS NULL",
        [],
        |r| r.get(0),
    )?;
    let recent = conn.query_row(
        "SELECT COUNT(*) FROM series WHERE deleted_at IS NULL AND added >= ?1",
        [now - RECENT_DAYS * 86_400],
        |r| r.get(0),
    )?;
    let marked = |column: &str| -> rusqlite::Result<u32> {
        conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM {MARKS_FIRST}
                 WHERE s.deleted_at IS NULL AND k.{column} IS NOT NULL"
            ),
            [],
            |r| r.get(0),
        )
    };
    Ok(SeriesCounts {
        total,
        favourites: marked("favourite_at")?,
        recent,
        watched: marked("watched_at")?,
    })
}

/// What the search index is built from.
pub fn series_names(conn: &Connection) -> Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT series_id, CASE WHEN clean_name <> '' THEN clean_name ELSE name END
         FROM series WHERE deleted_at IS NULL",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// A live show's poster, as the provider gives it.
pub fn series_poster_url(conn: &Connection, id: i64) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT poster_url FROM series WHERE series_id = ?1 AND deleted_at IS NULL",
            [id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

/// The details last fetched for a show: when, and the JSON.
pub fn cached_series_details(conn: &Connection, id: i64) -> Result<Option<(i64, String)>> {
    Ok(conn
        .query_row(
            "SELECT fetched_at, info FROM series_details WHERE series_id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

/// A show's live episodes: by season, specials last, then by (first)
/// number, then by id.
pub fn series_episodes(conn: &Connection, series_id: i64) -> Result<Vec<EpisodeRow>> {
    let mut stmt = conn.prepare_cached(EPISODES)?;
    let rows: Vec<EpisodeRow> = stmt
        .query_map([series_id], episode_row)?
        .collect::<rusqlite::Result<_>>()?;
    let (mut seasons, specials): (Vec<_>, Vec<_>) = rows.into_iter().partition(|e| e.season != 0);
    seasons.extend(specials);
    Ok(seasons)
}

/// What playing an episode needs, or None for one that is unknown or
/// deleted.
pub fn episode_source(conn: &Connection, id: i64) -> Result<Option<EpisodeSource>> {
    Ok(conn
        .query_row(
            "SELECT series_id, container, stream_url, http_user_agent, http_referrer
             FROM episodes WHERE episode_id = ?1 AND deleted_at IS NULL",
            [id],
            |r| {
                Ok(EpisodeSource {
                    series_id: r.get(0)?,
                    container: r.get(1)?,
                    stream_url: r.get(2)?,
                    user_agent: r.get(3)?,
                    referrer: r.get(4)?,
                })
            },
        )
        .optional()?)
}

/// A live episode and its live show.
pub fn episode_by_id(conn: &Connection, id: i64) -> Result<Option<(SeriesRow, EpisodeRow)>> {
    let episode = conn
        .query_row(
            concat!(
                episodes_marked!(),
                " WHERE e.episode_id = ?1 AND e.deleted_at IS NULL"
            ),
            [id],
            episode_row,
        )
        .optional()?;
    let Some(episode) = episode else {
        return Ok(None);
    };
    Ok(series_by_ids(conn, &[episode.series_id])?
        .pop()
        .map(|show| (show, episode)))
}

/// A live episode's still, as the provider gives it.
pub fn episode_still_url(conn: &Connection, id: i64) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT still_url FROM episodes WHERE episode_id = ?1 AND deleted_at IS NULL",
            [id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

/// The seasons that have episodes, in order with the specials last, named
/// as the details list them (`listed`), else "Season N" or "Specials". A
/// multi-episode entry counts as one, and so does each row it covers.
pub fn seasons_of(eps: &[EpisodeRow], listed: &[(i32, String)]) -> Vec<SeasonRow> {
    let mut counts: BTreeMap<(bool, i32), (u32, u32)> = BTreeMap::new();
    for e in eps {
        let c = counts.entry((e.season == 0, e.season)).or_default();
        c.0 += 1;
        c.1 += u32::from(e.watched);
    }
    counts
        .into_iter()
        .map(|((_, number), (episode_count, watched_count))| {
            let name = listed
                .iter()
                .find(|(n, name)| *n == number && !name.trim().is_empty())
                .map(|(_, name)| name.trim().to_owned())
                .unwrap_or_else(|| match number {
                    0 => "Specials".to_owned(),
                    n => format!("Season {n}"),
                });
            SeasonRow {
                number,
                name,
                episode_count,
                watched_count,
            }
        })
        .collect()
}

/// The episode a show's main action plays, from its episodes in any order.
///
/// The rule: the episode in progress, else the next unwatched one after the
/// last watched. Episodes run in order of season, (first) number and id;
/// specials (season 0) never take part; a multi-episode entry is one item,
/// and "after" it means after its last number.
///
/// 1. The episode in progress (past [`STARTED_FRACTION`], short of
///    [`WATCHED_FRACTION`], not marked watched) played most recently. Its
///    length is what the player measured, else the provider's.
/// 2. Else the first unwatched one after the watched one with the latest
///    `watched_at`: in a later season, or in the same season with a first
///    number past that one's last.
/// 3. Else the first unwatched one from the start; with nothing watched or
///    started, that is the first episode.
/// 4. With every one watched, none.
///
/// Tie-breaks: of two in progress played at the same second, or two watched
/// at the same second (as a multi-episode entry and the rows it covers
/// are), the later in order counts.
///
/// [`STARTED_FRACTION`]: crate::catalog::vod_progress::STARTED_FRACTION
/// [`WATCHED_FRACTION`]: crate::catalog::vod_progress::WATCHED_FRACTION
pub fn next_up(eps: &[EpisodeRow]) -> Option<i64> {
    let mut order: Vec<&EpisodeRow> = eps.iter().filter(|e| e.season != 0).collect();
    order.sort_by_key(|e| (e.season, e.episode, e.episode_id));
    let length = |e: &EpisodeRow| e.duration.or(e.duration_secs.map(|s| s as f64));
    let in_progress = order
        .iter()
        .enumerate()
        .filter(|(_, e)| !e.watched && e.position.is_some_and(|p| is_started(p, length(e))))
        .max_by_key(|&(i, e)| (e.played_at.unwrap_or(i64::MIN), i));
    if let Some((_, e)) = in_progress {
        return Some(e.episode_id);
    }
    let last = order
        .iter()
        .enumerate()
        .filter(|(_, e)| e.watched)
        .max_by_key(|&(i, e)| (e.watched_at.unwrap_or(i64::MIN), i))
        .map(|(_, e)| *e);
    let after = last.and_then(|w| {
        let end = w.episode_end.unwrap_or(w.episode).max(w.episode);
        order.iter().find(|e| {
            !e.watched && (e.season > w.season || (e.season == w.season && e.episode > end))
        })
    });
    after
        .or_else(|| order.iter().find(|e| !e.watched))
        .map(|e| e.episode_id)
}

/// A multi-episode entry that became watched marks the live episodes it
/// covers watched too (keeping the time any of them was first).
fn mark_covered_in_tx(tx: &Connection, episode_id: i64, at: i64) -> Result<()> {
    tx.prepare_cached(concat!(
        "INSERT INTO vod_marks (origin, kind, item_id, watched_at)
         SELECT ",
        current_origin_sql!(),
        ", 'episode', c.episode_id, ?2 FROM (",
        covered_sql!(),
        ") c WHERE true
         ON CONFLICT(origin, kind, item_id) DO UPDATE
           SET watched_at = COALESCE(vod_marks.watched_at, excluded.watched_at)"
    ))?
    .execute(params![episode_id, at])?;
    Ok(())
}

/// Mark unwatched on a multi-episode entry starts over the episodes it
/// covers: their watched marks and positions go.
fn uncover_in_tx(tx: &Connection, episode_id: i64) -> Result<()> {
    tx.prepare_cached(concat!(
        "UPDATE vod_marks SET watched_at = NULL, position = NULL
         WHERE origin = ",
        current_origin_sql!(),
        " AND kind = 'episode' AND item_id IN (",
        covered_sql!(),
        ")"
    ))?
    .execute([episode_id])?;
    Ok(())
}

/// After an episode's marks moved (inside the caller's transaction): a
/// watched multi-episode entry takes the episodes it covers with it, one
/// just marked unwatched (left with no watched mark and no position, which
/// is what unmarking does and playing never does) clears them, and the
/// show's own watched mark is refreshed.
pub fn episode_marked_in_tx(tx: &Connection, episode_id: i64, at: i64) -> Result<()> {
    let state: Option<(i64, bool, bool)> = tx
        .prepare_cached(concat!(
            "SELECT e.series_id, k.watched_at IS NOT NULL, k.position IS NULL
             FROM episodes e LEFT JOIN vod_marks k ON k.origin = ",
            current_origin_sql!(),
            " AND k.kind = 'episode' AND k.item_id = e.episode_id
             WHERE e.episode_id = ?1"
        ))?
        .query_row([episode_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?;
    let Some((series_id, watched, no_position)) = state else {
        return Ok(());
    };
    if watched {
        mark_covered_in_tx(tx, episode_id, at)?;
    } else if no_position {
        uncover_in_tx(tx, episode_id)?;
    }
    refresh_series_watched_in_tx(tx, series_id, at)
}

/// After a progress save of an episode (inside the caller's transaction).
/// Only the save that first makes it watched takes a multi-episode entry's
/// covered episodes with it, so replaying a watched entry does not re-mark
/// one the viewer has since unmarked; the show's watched mark is refreshed
/// on every save.
pub fn episode_played_in_tx(
    tx: &Connection,
    episode_id: i64,
    became_watched: bool,
    at: i64,
) -> Result<()> {
    if became_watched {
        return episode_marked_in_tx(tx, episode_id, at);
    }
    let series_id: Option<i64> = tx
        .prepare_cached("SELECT series_id FROM episodes WHERE episode_id = ?1")?
        .query_row([episode_id], |r| r.get(0))
        .optional()?;
    match series_id {
        Some(id) => refresh_series_watched_in_tx(tx, id, at),
        None => Ok(()),
    }
}

/// How many live episodes show `?1` has, specials aside, and how many of
/// them are not watched.
const WATCHED_COUNT: &str = concat!(
    "SELECT COUNT(*), COUNT(*) - COUNT(k.watched_at)
     FROM episodes e LEFT JOIN vod_marks k ON k.origin = ",
    current_origin_sql!(),
    " AND k.kind = 'episode' AND k.item_id = e.episode_id
     WHERE e.series_id = ?1 AND e.deleted_at IS NULL AND e.season <> 0"
);

/// Sets or clears the show's watched mark (inside the caller's
/// transaction): watched when every live episode, specials aside, is, so a
/// watched show that gains an episode is not any more. A show with no
/// episodes known (an Xtream show not opened since its details went) is
/// left as it is: there is nothing to judge it by. The ★ is never touched.
pub fn refresh_series_watched_in_tx(tx: &Connection, series_id: i64, at: i64) -> Result<()> {
    let (known, unwatched): (i64, i64) = tx
        .prepare_cached(WATCHED_COUNT)?
        .query_row([series_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    if known == 0 {
        return Ok(());
    }
    if unwatched == 0 {
        tx.prepare_cached(concat!(
            "INSERT INTO vod_marks (origin, kind, item_id, watched_at) VALUES (",
            current_origin_sql!(),
            ", 'series', ?1, ?2)
             ON CONFLICT(origin, kind, item_id) DO UPDATE
               SET watched_at = COALESCE(vod_marks.watched_at, excluded.watched_at)"
        ))?
        .execute(params![series_id, at])?;
    } else {
        tx.prepare_cached(concat!(
            "UPDATE vod_marks SET watched_at = NULL WHERE origin = ",
            current_origin_sql!(),
            " AND kind = 'series' AND item_id = ?1"
        ))?
        .execute([series_id])?;
    }
    Ok(())
}

/// Sets or clears a show's ★ and watched marks (`None` leaves one alone),
/// in one transaction. Marking it watched marks every live episode watched,
/// specials aside; marking it unwatched starts every episode over, clearing
/// their watched marks and positions. Written here, never through
/// [`vod_store::set_marks`], which would open a transaction per episode.
pub fn set_series_marks(
    conn: &Connection,
    id: i64,
    favourite: Option<bool>,
    watched: Option<bool>,
    at: i64,
) -> Result<Marks> {
    let origin = vod_store::origin(conn)?;
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT OR IGNORE INTO vod_marks (origin, kind, item_id) VALUES (?1, 'series', ?2)",
        params![origin, id],
    )?;
    if let Some(f) = favourite {
        tx.execute(
            "UPDATE vod_marks SET favourite_at = CASE WHEN ?3 THEN COALESCE(favourite_at, ?4)
                                                     ELSE NULL END
             WHERE origin = ?1 AND kind = 'series' AND item_id = ?2",
            params![origin, id, f, at],
        )?;
    }
    match watched {
        Some(true) => {
            tx.execute(
                "INSERT INTO vod_marks (origin, kind, item_id, watched_at)
                 SELECT ?1, 'episode', episode_id, ?3 FROM episodes
                 WHERE series_id = ?2 AND deleted_at IS NULL AND season <> 0
                 ON CONFLICT(origin, kind, item_id) DO UPDATE
                   SET watched_at = COALESCE(vod_marks.watched_at, excluded.watched_at)",
                params![origin, id, at],
            )?;
        }
        Some(false) => {
            tx.execute(
                "UPDATE vod_marks SET watched_at = NULL, position = NULL
                 WHERE origin = ?1 AND kind = 'episode'
                   AND item_id IN (SELECT episode_id FROM episodes WHERE series_id = ?2)",
                params![origin, id],
            )?;
            tx.execute(
                "UPDATE vod_marks SET watched_at = NULL
                 WHERE origin = ?1 AND kind = 'series' AND item_id = ?2",
                params![origin, id],
            )?;
        }
        None => {}
    }
    refresh_series_watched_in_tx(&tx, id, at)?;
    let marks = vod_store::marks_of(&tx, &origin, ItemKind::Series, id)?;
    tx.commit()?;
    Ok(marks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::open_in_memory;
    use crate::catalog::series_sync::{land_series_details, NewEpisode, NewSeries, SeriesWriter};
    use crate::catalog::vod_store::{record_progress, set_marks, ItemKind};

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
            "INSERT INTO series (series_id, category_id, name, clean_name, year, rating, added)
             VALUES (?1, ?2, ?3, ?3, ?4, ?5, ?6)",
            params![id, cat, name, year, rating, added],
        )
        .unwrap();
    }

    /// Episode `id` of show 1.
    fn ep(id: i64, season: i32, n: i32) -> NewEpisode {
        NewEpisode {
            episode_id: id,
            series_id: 1,
            season,
            episode: n,
            ..NewEpisode::default()
        }
    }

    /// One entry holding episodes `first..=last` of show 1.
    fn range(id: i64, season: i32, first: i32, last: i32) -> NewEpisode {
        NewEpisode {
            episode_end: Some(last),
            ..ep(id, season, first)
        }
    }

    /// Show 1 with these episodes, as its details land them.
    fn show_with(c: &Connection, eps: &[NewEpisode]) {
        c.execute(
            "INSERT OR IGNORE INTO series (series_id, category_id, name, clean_name, added)
             VALUES (1, 'c', 'A Show', 'A Show', 1)",
            [],
        )
        .unwrap();
        land_series_details(c, 1, 10, "{}", eps).unwrap();
    }

    fn watch(c: &Connection, id: i64, at: i64) {
        set_marks(c, ItemKind::Episode, id, None, Some(true), at).unwrap();
    }

    fn unwatch(c: &Connection, id: i64, at: i64) {
        set_marks(c, ItemKind::Episode, id, None, Some(false), at).unwrap();
    }

    fn play(c: &Connection, id: i64, position: f64, at: i64) {
        record_progress(c, ItemKind::Episode, id, position, Some(100.0), at).unwrap();
    }

    fn next(c: &Connection) -> Option<i64> {
        next_up(&series_episodes(c, 1).unwrap())
    }

    fn show_watched(c: &Connection, id: i64) -> bool {
        series_by_ids(c, &[id]).unwrap()[0].watched
    }

    fn episode(c: &Connection, id: i64) -> EpisodeRow {
        series_episodes(c, 1)
            .unwrap()
            .into_iter()
            .find(|e| e.episode_id == id)
            .unwrap()
    }

    fn q(list: MovieList, sort: MovieSort) -> MovieQuery {
        MovieQuery {
            list,
            sort,
            year_from: None,
            year_to: None,
            search: None,
        }
    }

    fn search(list: MovieList, sort: MovieSort, text: &str) -> MovieQuery {
        MovieQuery {
            search: Some(text.into()),
            ..q(list, sort)
        }
    }

    fn cat(s: &str) -> MovieList {
        MovieList::Category(s.into())
    }

    fn ids(c: &Connection, q: &MovieQuery) -> Vec<i64> {
        series_page(c, q, 0, 100, NOW)
            .unwrap()
            .1
            .iter()
            .map(|s| s.series_id)
            .collect()
    }

    fn set_origin(c: &Connection, o: &str) {
        c.execute(
            "INSERT INTO meta (key, value) VALUES ('catalogue_origin', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [o],
        )
        .unwrap();
    }

    fn plan(c: &Connection, sql: &str, args: Vec<Value>) -> String {
        let mut stmt = c.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let rows: Vec<String> = stmt
            .query_map(params_from_iter(args), |r| r.get(3))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        rows.join("\n")
    }

    #[test]
    fn a_page_of_shows_counts_them_all_and_sorts_as_named() {
        let c = open_in_memory().unwrap();
        for i in 10..35 {
            add(&c, i, "big", &format!("S{i:02}"), None, None, i);
        }
        let (total, items) = series_page(&c, &q(cat("big"), MovieSort::Az), 20, 10, NOW).unwrap();
        assert_eq!((total, items.len()), (25, 5));
        assert_eq!(items[0].name, "S30");

        add(&c, 1, "c", "bravo", Some(2000), Some(5.0), NOW - 40 * DAY);
        add(&c, 2, "c", "Alpha", None, None, NOW - DAY);
        add(&c, 3, "c", "charlie", Some(2010), Some(8.0), NOW - 2 * DAY);
        assert_eq!(ids(&c, &q(cat("c"), MovieSort::Added)), [2, 3, 1]);
        assert_eq!(ids(&c, &q(cat("c"), MovieSort::Az)), [2, 1, 3]);
        assert_eq!(ids(&c, &q(cat("c"), MovieSort::Year)), [3, 1, 2]);
        assert_eq!(ids(&c, &q(cat("c"), MovieSort::Rating)), [3, 1, 2]);
        let mut years = q(cat("c"), MovieSort::Added);
        years.year_from = Some(2005);
        assert_eq!(ids(&c, &years), [3]);
        assert_eq!(ids(&c, &q(MovieList::Recent, MovieSort::Added)), [2, 3]);
        let counts = series_counts(&c, NOW).unwrap();
        assert_eq!((counts.total, counts.recent), (28, 2));
    }

    #[test]
    fn a_show_search_is_a_filter_on_the_list() {
        let c = open_in_memory().unwrap();
        add(
            &c,
            1,
            "a",
            "EN - Harbour Lights",
            Some(2001),
            Some(6.0),
            NOW - 5 * DAY,
        );
        add(
            &c,
            2,
            "b",
            "Garden - Lantern",
            Some(2003),
            Some(9.0),
            NOW - DAY,
        );
        add(
            &c,
            3,
            "c",
            "EN - Amber Tide",
            Some(1995),
            Some(8.0),
            NOW - 2 * DAY,
        );
        add(
            &c,
            4,
            "a",
            "FR/EN - Copper Bridge",
            Some(2010),
            None,
            NOW - 3 * DAY,
        );
        add(
            &c,
            5,
            "b",
            "L'Été à Brackenfold",
            Some(2005),
            Some(7.0),
            NOW,
        );
        let en = |sort| ids(&c, &search(MovieList::All, sort, "EN -"));
        assert_eq!(en(MovieSort::Added), [3, 4, 1]);
        assert_eq!(en(MovieSort::Az), [3, 1, 4]);
        assert_eq!(en(MovieSort::Year), [4, 1, 3]);
        assert_eq!(en(MovieSort::Rating), [3, 1, 4]);
        assert_eq!(
            ids(&c, &search(MovieList::All, MovieSort::Added, "ete a")),
            [5]
        );
        let mut years = search(MovieList::All, MovieSort::Added, "EN -");
        years.year_from = Some(2000);
        assert_eq!(ids(&c, &years), [4, 1]);
        assert_eq!(ids(&c, &search(cat("a"), MovieSort::Added, "EN -")), [4, 1]);
        let (total, items) = series_page(
            &c,
            &search(MovieList::All, MovieSort::Added, "EN -"),
            1,
            1,
            NOW,
        )
        .unwrap();
        assert_eq!((total, items[0].series_id), (3, 4));
        assert_eq!(
            ids(&c, &q(MovieList::All, MovieSort::Added)),
            [5, 2, 3, 4, 1]
        );
    }

    #[test]
    fn the_show_page_queries_use_an_index() {
        let c = open_in_memory().unwrap();
        let page = |q: &MovieQuery| -> String {
            let (sql, args) = page_sql(q, 0, 50, NOW);
            plan(&c, &sql, args)
        };
        let p = page(&q(cat("c"), MovieSort::Added));
        assert!(p.contains("series_by_category"), "{p}");
        assert!(!p.contains("SCAN"), "{p}");
        assert!(!p.contains("TEMP B-TREE"), "{p}");
        let p = page(&q(MovieList::Recent, MovieSort::Added));
        assert!(p.contains("series_live_by_added"), "{p}");
        assert!(!p.contains("SCAN"), "{p}");
        assert!(!p.contains("TEMP B-TREE"), "{p}");
        let p = page(&q(MovieList::Favourites, MovieSort::Az));
        assert!(!p.contains("SCAN s"), "{p}");
        // As the movies': a search reads the names from the index that gives
        // the list's order, and its count from the names' own.
        for (sort, index) in [
            (MovieSort::Added, "series_live_by_added"),
            (MovieSort::Az, "series_live_by_name"),
            (MovieSort::Year, "series_live_by_year"),
            (MovieSort::Rating, "series_live_by_rating"),
        ] {
            let p = page(&search(MovieList::All, sort, "x"));
            assert!(p.contains(index), "{p}");
            assert!(!p.contains("TEMP B-TREE"), "{p}");
        }
        let (sql, args) = count_sql(&search(MovieList::All, MovieSort::Az, "x"), NOW);
        let p = plan(&c, &sql, args);
        assert!(p.contains("series_live_by_name"), "{p}");
    }

    #[test]
    fn counting_a_category_and_listing_episodes_read_their_indexes() {
        let c = open_in_memory().unwrap();
        // The count `series_page` runs for a category.
        let (_, conds, args) = scope(&q(cat("c"), MovieSort::Added), NOW);
        let p = plan(
            &c,
            &format!(
                "SELECT COUNT(*) FROM series s WHERE {}",
                conds.join(" AND ")
            ),
            args,
        );
        assert!(p.contains("series_live_by_category"), "{p}");
        assert!(!p.contains("SCAN") && !p.contains("TEMP B-TREE"), "{p}");
        let p = plan(&c, CATEGORIES, vec![]);
        assert!(p.contains("series_live_by_category"), "{p}");
        let p = plan(&c, EPISODES, vec![Value::Integer(1)]);
        assert!(p.contains("episodes_by_series"), "{p}");
        assert!(!p.contains("SCAN"), "{p}");
        assert!(!p.contains("TEMP B-TREE"), "{p}");
        // What marking an episode reads: the rows a range covers, and the
        // show's watched count.
        let p = plan(&c, covered_sql!(), vec![Value::Integer(1)]);
        assert!(p.contains("episodes_by_series"), "{p}");
        assert!(!p.contains("SCAN"), "{p}");
        let p = plan(&c, WATCHED_COUNT, vec![Value::Integer(1)]);
        assert!(p.contains("episodes_by_series"), "{p}");
        assert!(!p.contains("SCAN"), "{p}");

        c.execute(
            "INSERT INTO vod_categories (kind, id, name, sort) VALUES ('series','c','Cat',0)",
            [],
        )
        .unwrap();
        for i in 0..5 {
            add(&c, i, "c", "x", None, None, NOW - i);
        }
        c.execute("UPDATE series SET deleted_at = 1 WHERE series_id = 4", [])
            .unwrap();
        assert_eq!(series_categories(&c).unwrap()[0].series_count, 4);
        for list in [cat("c"), MovieList::Recent] {
            let (total, items) = series_page(&c, &q(list, MovieSort::Added), 0, 100, NOW).unwrap();
            assert_eq!((total as usize, items.len()), (4, 4));
        }
    }

    #[test]
    fn deleted_shows_and_episodes_are_left_out_and_order_is_kept() {
        let c = open_in_memory().unwrap();
        show_with(&c, &[ep(11, 1, 1), ep(12, 1, 2)]);
        add(&c, 2, "c", "Two", None, None, 2);
        add(&c, 3, "c", "Three", None, None, 3);
        c.execute(
            "UPDATE episodes SET still_url = 'http://img/11.jpg', container = 'mkv'
             WHERE episode_id = 11",
            [],
        )
        .unwrap();
        let got: Vec<i64> = series_by_ids(&c, &[3, 1, 2])
            .unwrap()
            .iter()
            .map(|s| s.series_id)
            .collect();
        assert_eq!(got, [3, 1, 2]);
        let source = episode_source(&c, 11).unwrap().unwrap();
        assert_eq!(
            (source.series_id, source.container.as_deref()),
            (1, Some("mkv"))
        );
        assert_eq!(
            episode_still_url(&c, 11).unwrap().as_deref(),
            Some("http://img/11.jpg")
        );
        let (show, e) = episode_by_id(&c, 12).unwrap().unwrap();
        assert_eq!((show.series_id, e.episode), (1, 2));

        c.execute("UPDATE series SET deleted_at = 9 WHERE series_id = 3", [])
            .unwrap();
        c.execute(
            "UPDATE episodes SET deleted_at = 9 WHERE episode_id = 12",
            [],
        )
        .unwrap();
        assert_eq!(series_by_ids(&c, &[3, 1]).unwrap().len(), 1);
        assert_eq!(series_names(&c).unwrap().len(), 2);
        assert_eq!(series_counts(&c, NOW).unwrap().total, 2);
        assert_eq!(series_episodes(&c, 1).unwrap().len(), 1);
        assert!(episode_source(&c, 12).unwrap().is_none());
        assert!(episode_by_id(&c, 12).unwrap().is_none());
        assert!(series_poster_url(&c, 3).unwrap().is_none());
        assert_eq!(
            cached_series_details(&c, 1).unwrap(),
            Some((10, "{}".into()))
        );
        assert!(cached_series_details(&c, 2).unwrap().is_none());
    }

    #[test]
    fn episodes_come_in_order_with_the_specials_last() {
        let c = open_in_memory().unwrap();
        show_with(&c, &[ep(1, 0, 1), ep(22, 2, 2), ep(11, 1, 1), ep(21, 2, 1)]);
        let order: Vec<i64> = series_episodes(&c, 1)
            .unwrap()
            .iter()
            .map(|e| e.episode_id)
            .collect();
        assert_eq!(order, [11, 21, 22, 1]);
    }

    #[test]
    fn nothing_watched_means_the_first_episode_is_next() {
        let c = open_in_memory().unwrap();
        show_with(&c, &[ep(1, 0, 1), ep(21, 2, 1), ep(11, 1, 1), ep(12, 1, 2)]);
        assert_eq!(next(&c), Some(11));
    }

    #[test]
    fn the_episode_in_progress_is_next() {
        let c = open_in_memory().unwrap();
        show_with(&c, &[ep(11, 1, 1), ep(12, 1, 2), ep(13, 1, 3)]);
        watch(&c, 11, 1);
        play(&c, 13, 30.0, 2);
        assert_eq!(next(&c), Some(13));
        // Barely begun is not in progress.
        play(&c, 13, 0.5, 3);
        assert_eq!(next(&c), Some(12));
    }

    #[test]
    fn the_latest_in_progress_wins() {
        let c = open_in_memory().unwrap();
        show_with(&c, &[ep(11, 1, 1), ep(12, 1, 2), ep(21, 2, 1)]);
        play(&c, 21, 40.0, 5);
        play(&c, 12, 30.0, 9);
        assert_eq!(next(&c), Some(12));
        play(&c, 21, 41.0, 10);
        assert_eq!(next(&c), Some(21));
    }

    #[test]
    fn after_the_last_watched_comes_the_next_unwatched() {
        let c = open_in_memory().unwrap();
        show_with(
            &c,
            &[
                ep(11, 1, 1),
                ep(12, 1, 2),
                ep(13, 1, 3),
                ep(21, 2, 1),
                ep(22, 2, 2),
            ],
        );
        watch(&c, 11, 1);
        watch(&c, 12, 2);
        watch(&c, 13, 3);
        assert_eq!(next(&c), Some(21));
    }

    #[test]
    fn the_latest_watched_counts_not_the_furthest() {
        let c = open_in_memory().unwrap();
        show_with(
            &c,
            &[ep(11, 1, 1), ep(12, 1, 2), ep(13, 1, 3), ep(21, 2, 1)],
        );
        watch(&c, 21, 1);
        watch(&c, 12, 2);
        assert_eq!(next(&c), Some(13));
    }

    #[test]
    fn with_nothing_after_it_the_first_unwatched_from_the_start() {
        let c = open_in_memory().unwrap();
        show_with(&c, &[ep(11, 1, 1), ep(12, 1, 2), ep(24, 2, 4)]);
        watch(&c, 11, 1);
        watch(&c, 24, 2);
        assert_eq!(next(&c), Some(12));
    }

    #[test]
    fn specials_are_never_next_up() {
        let c = open_in_memory().unwrap();
        show_with(&c, &[ep(1, 0, 1), ep(11, 1, 1), ep(12, 1, 2)]);
        watch(&c, 11, 1);
        watch(&c, 12, 2);
        assert_eq!(next(&c), None);
        // Even in progress.
        play(&c, 1, 50.0, 3);
        assert_eq!(next(&c), None);
        let only_specials = open_in_memory().unwrap();
        show_with(&only_specials, &[ep(1, 0, 1)]);
        assert_eq!(next(&only_specials), None);
    }

    #[test]
    fn all_watched_has_no_next_up() {
        let c = open_in_memory().unwrap();
        show_with(&c, &[ep(11, 1, 1), ep(12, 1, 2)]);
        watch(&c, 12, 1);
        watch(&c, 11, 2);
        assert_eq!(next(&c), None);
        assert_eq!(next_up(&[]), None);
    }

    #[test]
    fn next_up_falls_back_to_the_episodes_own_length() {
        let c = open_in_memory().unwrap();
        let mut long = ep(12, 1, 2);
        long.duration_secs = Some(100);
        show_with(&c, &[ep(11, 1, 1), long]);
        record_progress(&c, ItemKind::Episode, 12, 30.0, None, 1).unwrap();
        assert_eq!(next(&c), Some(12));
    }

    #[test]
    fn seasons_come_from_the_episodes_when_the_list_is_empty() {
        let c = open_in_memory().unwrap();
        show_with(&c, &[ep(1, 0, 1), ep(21, 2, 1), ep(11, 1, 1), ep(12, 1, 2)]);
        watch(&c, 11, 1);
        let eps = series_episodes(&c, 1).unwrap();
        let seasons = seasons_of(&eps, &[]);
        let got: Vec<(i32, &str, u32, u32)> = seasons
            .iter()
            .map(|s| (s.number, s.name.as_str(), s.episode_count, s.watched_count))
            .collect();
        assert_eq!(
            got,
            [
                (1, "Season 1", 2, 1),
                (2, "Season 2", 1, 0),
                (0, "Specials", 1, 0)
            ]
        );
        // Listed names win; a listed season with no episodes is not one.
        let listed = [(2, "The Return".to_string()), (5, "Later".to_string())];
        let names: Vec<String> = seasons_of(&eps, &listed)
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, ["Season 1", "The Return", "Specials"]);
    }

    #[test]
    fn a_show_is_watched_when_every_episode_is() {
        let c = open_in_memory().unwrap();
        show_with(&c, &[ep(1, 0, 1), ep(11, 1, 1), ep(12, 1, 2)]);
        watch(&c, 11, 1);
        assert!(!show_watched(&c, 1));
        watch(&c, 12, 2);
        assert!(show_watched(&c, 1), "the special does not count");
        let counts = series_counts(&c, NOW).unwrap();
        assert_eq!(counts.watched, 1);
        assert_eq!(ids(&c, &q(MovieList::Watched, MovieSort::Added)), [1]);
        unwatch(&c, 12, 3);
        assert!(!show_watched(&c, 1));
        // Watching to the end counts as much as marking.
        play(&c, 12, 95.0, 4);
        assert!(show_watched(&c, 1));
    }

    #[test]
    fn a_watched_show_that_gains_an_episode_is_not_watched_any_more() {
        let c = open_in_memory().unwrap();
        show_with(&c, &[ep(11, 1, 1), ep(12, 1, 2)]);
        watch(&c, 11, 1);
        watch(&c, 12, 2);
        assert!(show_watched(&c, 1));
        land_series_details(&c, 1, 20, "{}", &[ep(11, 1, 1), ep(12, 1, 2), ep(13, 1, 3)]).unwrap();
        assert!(!show_watched(&c, 1));
        // And losing it again makes it watched again.
        land_series_details(&c, 1, 30, "{}", &[ep(11, 1, 1), ep(12, 1, 2)]).unwrap();
        assert!(show_watched(&c, 1));
    }

    #[test]
    fn marking_a_show_watched_marks_its_episodes_and_unwatching_starts_over() {
        let c = open_in_memory().unwrap();
        show_with(&c, &[ep(1, 0, 1), ep(11, 1, 1), ep(12, 1, 2)]);
        set_series_marks(&c, 1, Some(true), None, 1).unwrap();
        play(&c, 12, 40.0, 2);
        let m = set_series_marks(&c, 1, None, Some(true), 3).unwrap();
        assert!(m.watched && m.favourite);
        assert!(episode(&c, 11).watched && episode(&c, 12).watched);
        assert!(!episode(&c, 1).watched, "specials are left alone");
        assert_eq!(next(&c), None);

        play(&c, 1, 30.0, 4);
        let m = set_series_marks(&c, 1, None, Some(false), 5).unwrap();
        assert!(!m.watched && m.favourite);
        for id in [1, 11, 12] {
            let e = episode(&c, id);
            assert!(!e.watched && e.position.is_none(), "{id}");
        }
        assert!(!show_watched(&c, 1));
        assert_eq!(next(&c), Some(11));
        let m = set_series_marks(&c, 1, Some(false), None, 6).unwrap();
        assert!(!m.favourite);
        assert!(!series_by_ids(&c, &[1]).unwrap()[0].favourite);
    }

    #[test]
    fn next_up_continues_after_a_ranges_last_number() {
        let c = open_in_memory().unwrap();
        show_with(&c, &[range(10, 1, 1, 2), ep(12, 1, 2), ep(13, 1, 3)]);
        watch(&c, 10, 1);
        unwatch(&c, 12, 2);
        assert!(!episode(&c, 12).watched);
        assert_eq!(next(&c), Some(13));
    }

    #[test]
    fn watching_a_range_marks_the_episodes_it_covers() {
        let c = open_in_memory().unwrap();
        show_with(
            &c,
            &[range(10, 1, 1, 2), ep(12, 1, 2), ep(13, 1, 3), ep(22, 2, 2)],
        );
        play(&c, 10, 50.0, 1);
        assert!(!episode(&c, 12).watched);
        play(&c, 10, 92.0, 2);
        assert!(episode(&c, 10).watched);
        assert!(episode(&c, 12).watched);
        assert_eq!(episode(&c, 12).watched_at, Some(2));
        assert!(!episode(&c, 13).watched && !episode(&c, 22).watched);
    }

    #[test]
    fn unwatching_a_range_clears_the_episodes_it_covers() {
        let c = open_in_memory().unwrap();
        show_with(&c, &[range(10, 1, 1, 2), ep(12, 1, 2), ep(13, 1, 3)]);
        watch(&c, 10, 1);
        play(&c, 12, 95.0, 2);
        play(&c, 13, 40.0, 3);
        unwatch(&c, 10, 4);
        let (r, covered, other) = (episode(&c, 10), episode(&c, 12), episode(&c, 13));
        assert!(!r.watched && !covered.watched);
        assert_eq!((r.position, covered.position), (None, None));
        assert_eq!(other.position, Some(40.0), "outside the range");
    }

    #[test]
    fn replaying_a_watched_range_leaves_a_row_unmarked_on_its_own_alone() {
        let c = open_in_memory().unwrap();
        show_with(&c, &[range(10, 1, 1, 2), ep(12, 1, 2)]);
        play(&c, 10, 95.0, 1);
        assert!(episode(&c, 12).watched);
        assert!(show_watched(&c, 1));
        unwatch(&c, 12, 2);
        play(&c, 10, 97.0, 3);
        assert!(!episode(&c, 12).watched, "the save did not re-cover it");
        assert!(!show_watched(&c, 1));
    }

    #[test]
    fn a_range_counts_as_one_item_and_covered_rows_count_too() {
        let c = open_in_memory().unwrap();
        let episodes = |c: &Connection| -> (u32, Option<u32>) {
            let eps = series_episodes(c, 1).unwrap();
            (
                seasons_of(&eps, &[])[0].episode_count,
                series_by_ids(c, &[1]).unwrap()[0].episode_count,
            )
        };
        show_with(&c, &[range(10, 1, 1, 2), ep(13, 1, 3)]);
        assert_eq!(episodes(&c), (2, Some(2)));
        land_series_details(
            &c,
            1,
            20,
            "{}",
            &[range(10, 1, 1, 2), ep(12, 1, 2), ep(13, 1, 3)],
        )
        .unwrap();
        assert_eq!(episodes(&c), (3, Some(3)));
    }

    #[test]
    fn marking_inside_a_transaction_opens_none() {
        let c = open_in_memory().unwrap();
        show_with(&c, &[range(10, 1, 1, 2), ep(12, 1, 2), ep(13, 1, 3)]);
        let m = set_marks(&c, ItemKind::Episode, 10, None, Some(true), 1).unwrap();
        assert!(m.watched);
        record_progress(&c, ItemKind::Episode, 13, 99.0, Some(100.0), 2).unwrap();
        assert!(show_watched(&c, 1));
        set_marks(&c, ItemKind::Episode, 10, None, Some(false), 3).unwrap();
        assert!(
            set_series_marks(&c, 1, Some(true), Some(true), 4)
                .unwrap()
                .watched
        );
        assert!(c.is_autocommit(), "no transaction left open");
    }

    #[test]
    fn a_playlist_write_refreshes_the_shows_it_changed() {
        let c = open_in_memory().unwrap();
        let write = |eps: &[NewEpisode], now: i64| {
            let tx = c.unchecked_transaction().unwrap();
            let mut w = SeriesWriter::begin(&tx).unwrap();
            w.own_episodes();
            for e in eps {
                let s = NewSeries {
                    series_id: e.series_id,
                    name: "A Show".into(),
                    ..NewSeries::default()
                };
                w.push_series(&s, Some("Shows")).unwrap();
                w.push_episode(e).unwrap();
            }
            w.finish(now).unwrap();
            tx.commit().unwrap();
        };
        write(&[ep(11, 1, 1), ep(12, 1, 2)], 100);
        watch(&c, 11, 1);
        watch(&c, 12, 2);
        assert!(show_watched(&c, 1));
        write(&[ep(11, 1, 1), ep(12, 1, 2), ep(13, 1, 3)], 200);
        assert!(!show_watched(&c, 1));
        write(&[ep(11, 1, 1), ep(12, 1, 2)], 300);
        assert!(show_watched(&c, 1));
    }

    #[test]
    fn progress_of_an_episode_is_its_own() {
        let c = open_in_memory().unwrap();
        show_with(&c, &[ep(7, 1, 1)]);
        c.execute(
            "INSERT INTO movies (movie_id, name, clean_name, added) VALUES (7, 'Film', 'Film', 1)",
            [],
        )
        .unwrap();
        let m = record_progress(&c, ItemKind::Episode, 7, 95.0, Some(100.0), 1).unwrap();
        assert!(m.watched);
        assert!(episode(&c, 7).watched);
        let movie_rows: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM vod_marks WHERE kind = 'movie'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(movie_rows, 0);
        assert!(!crate::catalog::vod_store::movies_by_ids(&c, &[7]).unwrap()[0].watched);
    }

    #[test]
    fn favourites_and_watched_belong_to_the_catalogue_that_marked_them() {
        let c = open_in_memory().unwrap();
        set_origin(&c, "A");
        show_with(&c, &[ep(11, 1, 1)]);
        add(&c, 2, "c", "Two", None, None, 2);
        set_series_marks(&c, 2, Some(true), None, 5).unwrap();
        watch(&c, 11, 5);
        let favs = q(MovieList::Favourites, MovieSort::Added);
        let seen = q(MovieList::Watched, MovieSort::Added);
        assert_eq!((ids(&c, &favs), ids(&c, &seen)), (vec![2], vec![1]));
        set_origin(&c, "B");
        assert!(ids(&c, &favs).is_empty() && ids(&c, &seen).is_empty());
        let counts = series_counts(&c, NOW).unwrap();
        assert_eq!((counts.favourites, counts.watched), (0, 0));
        assert!(!series_by_ids(&c, &[2]).unwrap()[0].favourite);
        assert!(!episode(&c, 11).watched);
        assert_eq!(next(&c), Some(11));
        set_origin(&c, "A");
        assert_eq!((ids(&c, &favs), ids(&c, &seen)), (vec![2], vec![1]));
        assert!(episode(&c, 11).watched);
        assert_eq!(series_counts(&c, NOW).unwrap().favourites, 1);
    }
}
