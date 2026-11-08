//! Landing the provider's shows, and their episodes, in the mirror.
//!
//! Shows follow the movies' rules ([`crate::catalog::vod_sync`]): upserted,
//! only ever soft-deleted, one transaction per write. Episodes differ by
//! source. An Xtream server lists only its shows; a show's episodes come
//! from its details, fetched when it is opened, and land through
//! [`land_series_details`]. A playlist lists every episode, so its writer
//! [owns them](SeriesWriter::own_episodes) and the episodes it pushes are
//! the whole list.

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{BTreeSet, HashSet};

use crate::catalog::series_store;
use crate::catalog::sync::{clean_name, SyncStats};
use crate::catalog::vod_sync::{self, year_from_name, CategoryList};
use crate::xtream::{Category, SeriesInfo, SeriesStream};

/// A show as the mirror stores it, whichever kind of source listed it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NewSeries {
    pub series_id: i64,
    pub category_id: Option<String>,
    pub name: String,
    pub year: Option<i32>,
    pub rating: Option<f64>,
    pub poster_url: Option<String>,
    /// Unix seconds the provider last changed it, when it says.
    pub added: Option<i64>,
    pub tmdb_id: Option<i64>,
}

impl From<&SeriesStream> for NewSeries {
    fn from(s: &SeriesStream) -> Self {
        Self {
            series_id: s.series_id,
            category_id: s.category_id.map(|c| c.to_string()),
            name: s.name.clone(),
            year: s
                .year
                .and_then(|y| i32::try_from(y).ok())
                .filter(vod_sync::plausible_year)
                .or_else(|| year_from_name(&s.name)),
            rating: s.rating10(),
            poster_url: s.cover.clone(),
            added: s.last_modified.filter(|a| *a > 0),
            tmdb_id: s.tmdb,
        }
    }
}

/// An episode as the mirror stores it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NewEpisode {
    pub episode_id: i64,
    pub series_id: i64,
    /// 0 for specials.
    pub season: i32,
    pub episode: i32,
    /// A multi-episode entry's last number; always None from Xtream.
    pub episode_end: Option<i32>,
    pub title: Option<String>,
    pub plot: Option<String>,
    pub duration_secs: Option<i64>,
    pub still_url: Option<String>,
    pub air_date: Option<String>,
    pub container: Option<String>,
    /// Unix seconds the provider added it, when it says.
    pub added: Option<i64>,
    /// Where a playlist's entry plays from; None for an Xtream server's,
    /// which builds the URL from the account.
    pub stream_url: Option<String>,
    pub user_agent: Option<String>,
    pub referrer: Option<String>,
}

/// The catalogue the mirror's shows were last landed from, in `meta`.
const SERIES_ORIGIN: &str = "series_origin";

/// Which catalogue the mirror's shows are from, for the interface, as
/// [`vod_sync::movies_origin`] is for movies.
pub fn series_origin(conn: &Connection) -> Result<String> {
    vod_sync::landed_hash(conn, SERIES_ORIGIN)
}

fn is_title_separator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '-' | '–' | '—' | ':' | '|' | '.' | '_' | ',')
}

/// The rest of a title that starts with the show's name.
fn strip_show<'a>(title: &'a str, show: &str) -> Option<&'a str> {
    let show = show.trim();
    if show.is_empty() {
        return None;
    }
    let head = title.get(..show.len())?;
    let rest = &title[show.len()..];
    (head.eq_ignore_ascii_case(show) && !rest.starts_with(char::is_alphanumeric)).then_some(rest)
}

/// How long `SxxEyy` for this season and episode is at the start of `s`,
/// in any case and with or without zero padding. Another episode's code,
/// or one that runs into a letter or a digit, is not it.
fn code_len(s: &str, season: i32, episode: i32) -> Option<usize> {
    fn number(b: &[u8], from: usize) -> Option<(i64, usize)> {
        let n = b[from..].iter().take_while(|c| c.is_ascii_digit()).count();
        if n == 0 || n > 4 {
            return None;
        }
        let v = std::str::from_utf8(&b[from..from + n]).ok()?.parse().ok()?;
        Some((v, from + n))
    }
    let b = s.as_bytes();
    if !matches!(b.first(), Some(b's' | b'S')) {
        return None;
    }
    let (s_num, i) = number(b, 1)?;
    if !matches!(b.get(i), Some(b'e' | b'E')) {
        return None;
    }
    let (e_num, end) = number(b, i + 1)?;
    let alone = !b.get(end).is_some_and(u8::is_ascii_alphanumeric);
    (s_num == i64::from(season) && e_num == i64::from(episode) && alone).then_some(end)
}

/// What is before this episode's code when the code ends the title.
fn before_trailing_code(t: &str, season: i32, episode: i32) -> Option<&str> {
    t.char_indices()
        .rfind(|&(i, c)| {
            matches!(c, 's' | 'S')
                && !t[..i].ends_with(char::is_alphanumeric)
                && code_len(&t[i..], season, episode) == Some(t.len() - i)
        })
        .map(|(i, _)| &t[..i])
}

/// An episode's own title, without the show's name and the `SxxEyy` that
/// servers repeat in it: `A Show - S02E04 - Night Shift` is
/// `Night Shift`. None when nothing else is left.
pub fn episode_title(raw: &str, show: &str, season: i32, episode: i32) -> Option<String> {
    let mut t = raw.trim();
    // A show listed with its year is often named without it in its episodes.
    let bare = year_from_name(show).and_then(|_| {
        let s = show.trim();
        match s.strip_suffix(')') {
            Some(rest) => rest.rsplit_once('(').map(|(name, _)| name),
            None => s.rsplit_once(" - ").map(|(name, _)| name),
        }
    });
    if let Some(rest) = strip_show(t, show).or_else(|| bare.and_then(|b| strip_show(t, b))) {
        t = rest.trim_start_matches(is_title_separator);
    }
    if let Some(n) = code_len(t, season, episode) {
        t = t[n..].trim_start_matches(is_title_separator);
    } else if let Some(rest) = before_trailing_code(t, season, episode) {
        t = rest.trim_end_matches(is_title_separator);
    }
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

/// A show's episodes as its details list them. `show` is the show's name,
/// which the episode titles lose when they repeat it.
pub fn episodes_from_info(series_id: i64, show: &str, info: &SeriesInfo) -> Vec<NewEpisode> {
    info.episodes
        .iter()
        .map(|e| NewEpisode {
            episode_id: e.id,
            series_id,
            season: e.season,
            episode: e.episode_num,
            episode_end: None,
            title: e
                .title
                .as_deref()
                .and_then(|t| episode_title(t, show, e.season, e.episode_num)),
            plot: e.info.plot.clone(),
            duration_secs: e.info.runtime_secs(),
            still_url: e.info.movie_image.clone(),
            air_date: e.info.air_date.clone(),
            container: e.container_extension.clone(),
            added: e.added.filter(|a| *a > 0),
            stream_url: None,
            user_agent: None,
            referrer: None,
        })
        .collect()
}

/// Writes one episode under `series_id`. A new one the source gave no date
/// for is stored with 0, for the caller to stamp; an existing one keeps the
/// date it has.
fn upsert_episode(conn: &Connection, series_id: i64, e: &NewEpisode) -> Result<()> {
    conn.prepare_cached(
        "INSERT INTO episodes
           (episode_id, series_id, season, episode, episode_end, title, plot,
            duration_secs, still_url, air_date, container, added, stream_url,
            http_user_agent, http_referrer, deleted_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,COALESCE(?12, 0),?13,?14,?15,NULL)
         ON CONFLICT(episode_id) DO UPDATE SET
           series_id=excluded.series_id, season=excluded.season,
           episode=excluded.episode, episode_end=excluded.episode_end,
           title=excluded.title, plot=excluded.plot,
           duration_secs=excluded.duration_secs, still_url=excluded.still_url,
           air_date=excluded.air_date, container=excluded.container,
           added=COALESCE(?12, episodes.added, 0), stream_url=excluded.stream_url,
           http_user_agent=excluded.http_user_agent,
           http_referrer=excluded.http_referrer, deleted_at=NULL",
    )?
    .execute(params![
        e.episode_id,
        series_id,
        e.season,
        e.episode,
        e.episode_end,
        e.title,
        e.plot,
        e.duration_secs,
        e.still_url,
        e.air_date,
        e.container,
        e.added.filter(|a| *a > 0),
        e.stream_url,
        e.user_agent,
        e.referrer,
    ])?;
    Ok(())
}

/// A show's seasons (specials aside) and episodes, counted from its live
/// episodes.
const COUNTS: &str = "
    season_count = (SELECT COUNT(DISTINCT e.season) FROM episodes e
                     WHERE e.series_id = series.series_id
                       AND e.deleted_at IS NULL AND e.season > 0),
    episode_count = (SELECT COUNT(*) FROM episodes e
                      WHERE e.series_id = series.series_id AND e.deleted_at IS NULL)";

/// One open write of the series list inside someone else's transaction,
/// which also commits it, as [`vod_sync::MovieWriter`] is for movies. The
/// playlist writer drives this as it parses entries; [`land_series`] drives
/// it for an Xtream list.
pub struct SeriesWriter<'t> {
    conn: &'t Connection,
    existing: HashSet<i64>,
    seen: HashSet<i64>,
    categories: CategoryList,
    stats: SyncStats,
    /// Whether an empty list is believed. See [`Self::accept_empty`].
    empty_ok: bool,
    /// See [`Self::own_episodes`].
    owns_episodes: bool,
    /// The episodes pushed.
    episodes: HashSet<i64>,
    /// New episodes the source gave no date for, stamped in `finish`.
    undated: Vec<i64>,
    /// Shows whose episodes changed.
    changed: BTreeSet<i64>,
    /// Shows listed under another name or TMDB id than before.
    renamed: BTreeSet<i64>,
}

impl<'t> SeriesWriter<'t> {
    pub fn begin(tx: &'t rusqlite::Transaction<'_>) -> Result<Self> {
        Self::begin_on(tx)
    }

    /// As [`begin`](Self::begin), for a writer that holds the transaction's
    /// connection rather than the transaction itself.
    pub fn begin_on(conn: &'t Connection) -> Result<Self> {
        let existing: HashSet<i64> = {
            let mut s = conn.prepare("SELECT series_id FROM series WHERE deleted_at IS NULL")?;
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
            owns_episodes: false,
            episodes: HashSet::new(),
            undated: Vec::new(),
            changed: BTreeSet::new(),
            renamed: BTreeSet::new(),
        })
    }

    /// How many distinct shows have been written so far.
    pub fn seen(&self) -> usize {
        self.seen.len()
    }

    /// Believes an empty list, as [`vod_sync::MovieWriter::accept_empty`].
    pub fn accept_empty(&mut self) {
        self.empty_ok = true;
    }

    /// Makes this a write of every episode as well as every show, as a
    /// playlist's is: the episodes not pushed are gone, and each show's
    /// counts and date come from the ones that were.
    pub fn own_episodes(&mut self) {
        self.owns_episodes = true;
    }

    /// Writes one show. `group` is the playlist group it sits in, which
    /// becomes its category (in order of first appearance); a source with a
    /// category list leaves it `None` and the show's own `category_id`
    /// stands. A show pushed again in the same write is left as first
    /// written, so a playlist can push it with each of its episodes.
    pub fn push_series(&mut self, s: &NewSeries, group: Option<&str>) -> Result<()> {
        if vod_sync::is_separator(&s.name) || self.seen.contains(&s.series_id) {
            return Ok(());
        }
        let category = match group {
            Some(g) => Some(self.categories.group(g)),
            None => s.category_id.clone(),
        };
        let added = s.added.filter(|a| *a > 0);
        // What was fetched about a show under another name or TMDB id is
        // not about this one: its details go now, and its episodes are
        // soft-deleted in `finish` (the counts go in the upsert below).
        let renamed: Option<bool> = self
            .conn
            .prepare_cached(
                "SELECT name IS NOT ?2 OR tmdb_id IS NOT ?3 FROM series WHERE series_id = ?1",
            )?
            .query_row(params![s.series_id, s.name, s.tmdb_id], |r| r.get(0))
            .optional()?;
        if renamed == Some(true) {
            self.conn
                .prepare_cached("DELETE FROM series_details WHERE series_id = ?1")?
                .execute([s.series_id])?;
            self.renamed.insert(s.series_id);
        }
        let year = s
            .year
            .filter(vod_sync::plausible_year)
            .or_else(|| year_from_name(&s.name));
        self.conn
            .prepare_cached(
                // A show the provider gave no date for is stored with 0 and
                // stamped with the time of the write in `finish`.
                "INSERT INTO series
                   (series_id, category_id, name, clean_name, year, rating,
                    poster_url, added, tmdb_id, deleted_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,COALESCE(?8, 0),?9,NULL)
                 ON CONFLICT(series_id) DO UPDATE SET
                   category_id=excluded.category_id, name=excluded.name,
                   clean_name=excluded.clean_name, year=excluded.year,
                   rating=excluded.rating, poster_url=excluded.poster_url,
                   added=COALESCE(?8, series.added),
                   -- The counts come from the details and go with them.
                   season_count=CASE WHEN series.name IS excluded.name
                                      AND series.tmdb_id IS excluded.tmdb_id
                                     THEN series.season_count END,
                   episode_count=CASE WHEN series.name IS excluded.name
                                       AND series.tmdb_id IS excluded.tmdb_id
                                      THEN series.episode_count END,
                   tmdb_id=excluded.tmdb_id, deleted_at=NULL",
            )?
            .execute(params![
                s.series_id,
                category,
                s.name,
                clean_name(&s.name),
                year,
                s.rating,
                s.poster_url,
                added,
                s.tmdb_id,
            ])?;
        self.seen.insert(s.series_id);
        if self.existing.contains(&s.series_id) {
            self.stats.updated += 1;
        } else {
            self.stats.inserted += 1;
        }
        Ok(())
    }

    /// Writes one episode under its show.
    pub fn push_episode(&mut self, e: &NewEpisode) -> Result<()> {
        let before: Option<(i64, Option<i64>)> = self
            .conn
            .prepare_cached("SELECT series_id, deleted_at FROM episodes WHERE episode_id = ?1")?
            .query_row([e.episode_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        match before {
            None => {
                self.changed.insert(e.series_id);
                if e.added.filter(|a| *a > 0).is_none() {
                    self.undated.push(e.episode_id);
                }
            }
            Some((was, deleted)) if was != e.series_id || deleted.is_some() => {
                self.changed.insert(was);
                self.changed.insert(e.series_id);
            }
            Some(_) => {}
        }
        upsert_episode(self.conn, e.series_id, e)?;
        self.episodes.insert(e.episode_id);
        Ok(())
    }

    /// Stamps new undated shows and episodes, soft-deletes what was not seen
    /// and replaces the series categories, without committing.
    ///
    /// An empty list while shows are mirrored is refused, as an empty movie
    /// list is ([`vod_sync::MovieWriter::finish`]).
    pub fn finish(self, now: i64) -> Result<SeriesLanded> {
        let Self {
            conn,
            existing,
            seen,
            categories,
            mut stats,
            empty_ok,
            owns_episodes,
            episodes,
            undated,
            mut changed,
            renamed,
        } = self;
        let (origin, switched) = vod_sync::current_origin(conn, SERIES_ORIGIN)?;
        if seen.is_empty() && !existing.is_empty() && !switched && !empty_ok {
            return Err(anyhow!(
                "the series list was empty; keeping the {} already here",
                existing.len()
            ));
        }
        if switched {
            // Details, episodes and counts are the old account's, and a show
            // the new one reuses an id for is not the same show.
            conn.execute("DELETE FROM series_details", [])?;
            let old: Vec<(i64, i64)> = conn
                .prepare("SELECT episode_id, series_id FROM episodes")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<_, _>>()?;
            let mut delete = conn.prepare_cached("DELETE FROM episodes WHERE episode_id = ?1")?;
            for (id, show) in old.into_iter().filter(|(id, _)| !episodes.contains(id)) {
                delete.execute([id])?;
                changed.insert(show);
            }
            conn.execute(
                "UPDATE series SET season_count = NULL, episode_count = NULL",
                [],
            )?;
        }
        vod_sync::record_origin(conn, SERIES_ORIGIN, &origin)?;
        conn.execute("UPDATE series SET added = ?1 WHERE added = 0", [now])?;
        let mut soft_delete =
            conn.prepare_cached("UPDATE series SET deleted_at=?1 WHERE series_id=?2")?;
        for gone in existing.difference(&seen) {
            soft_delete.execute(params![now, gone])?;
            stats.deleted += 1;
        }
        let mut stamp = conn
            .prepare_cached("UPDATE episodes SET added = ?1 WHERE episode_id = ?2 AND added = 0")?;
        for id in &undated {
            stamp.execute(params![now, id])?;
        }
        let mut soft_delete =
            conn.prepare_cached("UPDATE episodes SET deleted_at = ?1 WHERE episode_id = ?2")?;
        // A renamed show's episodes were another show's, unless this write
        // listed them again.
        let mut of_show = conn.prepare_cached(
            "SELECT episode_id FROM episodes WHERE series_id = ?1 AND deleted_at IS NULL",
        )?;
        for &show in &renamed {
            let live: Vec<i64> = of_show
                .query_map([show], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            for id in live.into_iter().filter(|id| !episodes.contains(id)) {
                soft_delete.execute(params![now, id])?;
                changed.insert(show);
            }
        }
        if owns_episodes {
            let live: Vec<(i64, i64)> = conn
                .prepare("SELECT episode_id, series_id FROM episodes WHERE deleted_at IS NULL")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<_, _>>()?;
            for (id, show) in live.into_iter().filter(|(id, _)| !episodes.contains(id)) {
                soft_delete.execute(params![now, id])?;
                changed.insert(show);
            }
            // Only the shows whose counts may have moved: those whose
            // episodes changed, those a rename cleared, and every one after
            // a switch cleared them all.
            let mut recount: BTreeSet<i64> = changed.union(&renamed).copied().collect();
            if switched {
                recount.extend(&seen);
            }
            // A playlist dates nothing, so a show is as new as the newest
            // episode it gained: one that adds an episode climbs the
            // newest-first list, one that only stays listed does not.
            let mut update = conn.prepare_cached(&format!(
                "UPDATE series SET {COUNTS},
                   added = COALESCE((SELECT MAX(e.added) FROM episodes e
                                      WHERE e.series_id = series.series_id
                                        AND e.deleted_at IS NULL), added)
                 WHERE series_id = ?1 AND deleted_at IS NULL"
            ))?;
            for show in recount {
                update.execute([show])?;
            }
        }
        // A show that gained or lost episodes may be watched now, or not.
        for &show in &changed {
            series_store::refresh_series_watched_in_tx(conn, show, now)?;
        }
        categories.replace(conn, "series")?;
        Ok(SeriesLanded {
            stats,
            categories: categories.len(),
            episodes: episodes.len(),
            changed_shows: changed.into_iter().collect(),
        })
    }
}

/// What a [`SeriesWriter`] landed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SeriesLanded {
    /// The shows'.
    pub stats: SyncStats,
    /// How many categories there now are.
    pub categories: usize,
    /// How many distinct episodes were pushed.
    pub episodes: usize,
    /// The shows whose episodes were added, moved, revived or removed, in
    /// ascending order.
    pub changed_shows: Vec<i64>,
}

/// Lands an Xtream server's series categories and shows in one
/// transaction. Episodes are left alone, except on a change of account.
pub fn land_series(
    conn: &Connection,
    categories: &[Category],
    series: &[NewSeries],
    now: i64,
) -> Result<SyncStats> {
    let tx = conn.unchecked_transaction()?;
    let mut w = SeriesWriter::begin(&tx)?;
    w.categories.extend(categories);
    for s in series {
        w.push_series(s, None)?;
    }
    let landed = w.finish(now)?;
    tx.commit()?;
    Ok(landed.stats)
}

/// Lands one show's details, fetched at `at`, in one transaction: the
/// details as fetched (`info_json`), and `episodes` as the whole of the
/// show's episodes, written under `series_id`. Episodes it no longer lists
/// are soft-deleted, so their marks are still there if they return.
///
/// No episodes for a show that has some is refused, as an empty series list
/// is: a server's bad minute, far more likely than a show emptied.
pub fn land_series_details(
    conn: &Connection,
    series_id: i64,
    at: i64,
    info_json: &str,
    episodes: &[NewEpisode],
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    if episodes.is_empty() {
        let live: i64 = tx.query_row(
            "SELECT COUNT(*) FROM episodes WHERE series_id = ?1 AND deleted_at IS NULL",
            [series_id],
            |r| r.get(0),
        )?;
        if live > 0 {
            return Err(anyhow!(
                "the show's episode list was empty; keeping the {live} already here"
            ));
        }
    }
    tx.execute(
        "INSERT INTO series_details (series_id, fetched_at, info) VALUES (?1, ?2, ?3)
         ON CONFLICT(series_id) DO UPDATE SET
           fetched_at=excluded.fetched_at, info=excluded.info",
        params![series_id, at, info_json],
    )?;
    let mut listed = HashSet::new();
    for e in episodes {
        upsert_episode(&tx, series_id, e)?;
        listed.insert(e.episode_id);
    }
    tx.execute(
        "UPDATE episodes SET added = ?1 WHERE series_id = ?2 AND added = 0",
        params![at, series_id],
    )?;
    let live: Vec<i64> = tx
        .prepare("SELECT episode_id FROM episodes WHERE series_id = ?1 AND deleted_at IS NULL")?
        .query_map([series_id], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    {
        let mut soft_delete =
            tx.prepare_cached("UPDATE episodes SET deleted_at = ?1 WHERE episode_id = ?2")?;
        for id in live.into_iter().filter(|id| !listed.contains(id)) {
            soft_delete.execute(params![at, id])?;
        }
    }
    tx.execute(
        &format!("UPDATE series SET {COUNTS} WHERE series_id = ?1"),
        [series_id],
    )?;
    // A watched show that gained an episode is not watched any more.
    series_store::refresh_series_watched_in_tx(&tx, series_id, at)?;
    tx.commit()?;
    Ok(())
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

    fn show(id: i64, cat: &str, name: &str) -> NewSeries {
        NewSeries {
            series_id: id,
            category_id: Some(cat.into()),
            name: name.into(),
            ..NewSeries::default()
        }
    }

    fn three() -> (Vec<Category>, Vec<NewSeries>) {
        (
            vec![cat("a", "First"), cat("b", "Second")],
            vec![
                show(1, "a", "One"),
                show(2, "a", "Two"),
                show(3, "b", "Three"),
            ],
        )
    }

    fn ep(id: i64, series: i64, season: i32, episode: i32) -> NewEpisode {
        NewEpisode {
            episode_id: id,
            series_id: series,
            season,
            episode,
            ..NewEpisode::default()
        }
    }

    fn set_origin(c: &Connection, o: &str) {
        c.execute(
            "INSERT INTO meta (key, value) VALUES ('catalogue_origin', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [o],
        )
        .unwrap();
    }

    fn count(c: &Connection, sql: &str) -> i64 {
        c.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn added(c: &Connection, id: i64) -> i64 {
        c.query_row("SELECT added FROM series WHERE series_id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap()
    }

    #[test]
    fn series_land_with_their_categories_and_resync_without_duplicates() {
        let c = open_in_memory().unwrap();
        c.execute(
            "INSERT INTO vod_categories (kind, id, name) VALUES ('movie', 'm', 'Films')",
            [],
        )
        .unwrap();
        let (cats, shows) = three();
        let s = land_series(&c, &cats, &shows, 100).unwrap();
        assert_eq!((s.inserted, s.updated, s.deleted), (3, 0, 0));
        let s = land_series(&c, &cats, &shows, 200).unwrap();
        assert_eq!((s.inserted, s.updated, s.deleted), (0, 3, 0));
        assert_eq!(count(&c, "SELECT COUNT(*) FROM series"), 3);
        assert_eq!(
            count(
                &c,
                "SELECT COUNT(*) FROM vod_categories WHERE kind = 'series'"
            ),
            2
        );
        assert_eq!(
            count(
                &c,
                "SELECT COUNT(*) FROM vod_categories WHERE kind = 'movie'"
            ),
            1
        );
    }

    #[test]
    fn a_show_the_server_stopped_listing_is_soft_deleted_and_its_marks_kept() {
        let c = open_in_memory().unwrap();
        let (cats, shows) = three();
        land_series(&c, &cats, &shows, 100).unwrap();
        c.execute(
            "INSERT INTO vod_marks (origin, kind, item_id, favourite_at)
             VALUES ('', 'series', 2, 5)",
            [],
        )
        .unwrap();
        let s = land_series(&c, &cats, &[shows[0].clone(), shows[2].clone()], 200).unwrap();
        assert_eq!(s.deleted, 1);
        let gone: Option<i64> = c
            .query_row(
                "SELECT deleted_at FROM series WHERE series_id = 2",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(gone, Some(200));
        assert_eq!(
            count(&c, "SELECT COUNT(*) FROM vod_marks WHERE item_id = 2"),
            1
        );
        // Back in the list, it is the same show again.
        land_series(&c, &cats, &shows, 300).unwrap();
        assert_eq!(
            count(&c, "SELECT COUNT(*) FROM series WHERE deleted_at IS NULL"),
            3
        );
    }

    #[test]
    fn an_empty_series_list_keeps_the_shows_already_here() {
        let c = open_in_memory().unwrap();
        let (cats, shows) = three();
        land_series(&c, &cats, &shows, 100).unwrap();
        let err = land_series(&c, &cats, &[], 200).unwrap_err().to_string();
        assert!(err.contains("keeping the 3"), "{err}");
        assert!(err.contains("series list was empty"), "{err}");
        assert_eq!(
            count(&c, "SELECT COUNT(*) FROM series WHERE deleted_at IS NULL"),
            3
        );

        let fresh = open_in_memory().unwrap();
        let s = land_series(&fresh, &[], &[], 200).unwrap();
        assert_eq!((s.inserted, s.updated, s.deleted), (0, 0, 0));
    }

    #[test]
    fn added_follows_the_providers_last_change_else_first_seen() {
        let c = open_in_memory().unwrap();
        let (cats, mut shows) = three();
        shows[0].added = Some(500);
        land_series(&c, &cats, &shows, 100).unwrap();
        assert_eq!((added(&c, 1), added(&c, 2)), (500, 100));
        shows[0].added = Some(900);
        land_series(&c, &cats, &shows, 200).unwrap();
        assert_eq!((added(&c, 1), added(&c, 2)), (900, 100));
    }

    #[test]
    fn the_xtream_row_converts_with_its_last_change_and_a_year() {
        let row: SeriesStream = serde_json::from_str(
            r#"{"series_id":7,"name":"A Show (2019)","category_id":"3",
                "last_modified":"0","rating":"0","rating_5based":4}"#,
        )
        .unwrap();
        let s = NewSeries::from(&row);
        assert_eq!(s.series_id, 7);
        assert_eq!(s.category_id.as_deref(), Some("3"));
        assert_eq!(s.added, None, "zero is no date");
        assert_eq!(s.year, Some(2019), "from the name");
        assert_eq!(s.rating, Some(8.0));
        let row: SeriesStream = serde_json::from_str(
            r#"{"series_id":7,"name":"A Show","releaseDate":"2021-03-01","last_modified":"500"}"#,
        )
        .unwrap();
        let s = NewSeries::from(&row);
        assert_eq!((s.year, s.added), (Some(2021), Some(500)));
    }

    #[test]
    fn details_land_the_shows_episodes_and_counts() {
        let c = open_in_memory().unwrap();
        let (cats, shows) = three();
        land_series(&c, &cats, &shows, 100).unwrap();
        let all = [
            ep(11, 1, 1, 1),
            ep(12, 1, 1, 2),
            ep(21, 1, 2, 1),
            ep(1, 1, 0, 1),
        ];
        land_series_details(&c, 1, 150, "{}", &all).unwrap();
        assert_eq!(count(&c, "SELECT COUNT(*) FROM episodes"), 4);
        let counts = |c: &Connection| -> (Option<i64>, Option<i64>) {
            c.query_row(
                "SELECT season_count, episode_count FROM series WHERE series_id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
        };
        assert_eq!(counts(&c), (Some(2), Some(4)));
        assert_eq!(
            count(
                &c,
                "SELECT COUNT(*) FROM series_details WHERE fetched_at = 150"
            ),
            1
        );

        let fewer = [all[0].clone(), all[2].clone(), all[3].clone()];
        land_series_details(&c, 1, 250, "{}", &fewer).unwrap();
        let gone: Option<i64> = c
            .query_row(
                "SELECT deleted_at FROM episodes WHERE episode_id = 12",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(gone, Some(250));
        assert_eq!(
            count(&c, "SELECT COUNT(*) FROM episodes WHERE deleted_at IS NULL"),
            3
        );
        assert_eq!(counts(&c), (Some(2), Some(3)));
        assert_eq!(
            count(
                &c,
                "SELECT fetched_at FROM series_details WHERE series_id = 1"
            ),
            250
        );
        // A later list leaves the episodes alone.
        land_series(&c, &cats, &shows, 300).unwrap();
        assert_eq!(
            count(&c, "SELECT COUNT(*) FROM episodes WHERE deleted_at IS NULL"),
            3
        );
        assert_eq!(counts(&c), (Some(2), Some(3)));
    }

    #[test]
    fn an_episode_title_loses_the_show_and_code_it_repeats() {
        assert_eq!(
            episode_title("A Show - S02E04 - Night Shift", "A Show", 2, 4).as_deref(),
            Some("Night Shift")
        );
        assert_eq!(
            episode_title("s2e4 Night Shift", "A Show", 2, 4).as_deref(),
            Some("Night Shift")
        );
        assert_eq!(episode_title("A Show S02E04", "A Show", 2, 4), None);
        assert_eq!(
            episode_title("Night Shift", "A Show", 2, 4).as_deref(),
            Some("Night Shift")
        );
        // Another episode's code, or a longer word, is part of the title.
        assert_eq!(
            episode_title("S02E05 Night Shift", "A Show", 2, 4).as_deref(),
            Some("S02E05 Night Shift")
        );
        assert_eq!(
            episode_title("A Showcase", "A Show", 2, 4).as_deref(),
            Some("A Showcase")
        );
        assert_eq!(
            episode_title("a show: Night Shift - S02E04", "A Show", 2, 4).as_deref(),
            Some("Night Shift")
        );
        assert_eq!(
            episode_title("A Show - S02E04", "A Show (2019)", 2, 4),
            None
        );
        assert_eq!(episode_title("  ", "A Show", 2, 4), None);
    }

    #[test]
    fn episodes_come_from_the_details_with_their_titles_cleaned() {
        let info: SeriesInfo = serde_json::from_str(
            r#"{"info":{"name":"A Show"},"episodes":{"1":[
                {"id":"11","episode_num":"1","title":"A Show - S01E01 - Pilot",
                 "container_extension":"mkv","added":"400",
                 "info":{"movie_image":"http://img/1.jpg","plot":"It starts.",
                         "duration":"00:52:00","releasedate":"2020-01-01"}},
                {"id":"12","episode_num":"2","title":"A Show - S01E02"}]}}"#,
        )
        .unwrap();
        let eps = episodes_from_info(9, "A Show", &info);
        assert_eq!(eps.len(), 2);
        let e = &eps[0];
        assert_eq!(
            (e.episode_id, e.series_id, e.season, e.episode),
            (11, 9, 1, 1)
        );
        assert_eq!(e.title.as_deref(), Some("Pilot"));
        assert_eq!(e.plot.as_deref(), Some("It starts."));
        assert_eq!(e.duration_secs, Some(52 * 60));
        assert_eq!(e.still_url.as_deref(), Some("http://img/1.jpg"));
        assert_eq!(e.air_date.as_deref(), Some("2020-01-01"));
        assert_eq!(e.container.as_deref(), Some("mkv"));
        assert_eq!(
            (e.added, e.episode_end, e.stream_url.as_deref()),
            (Some(400), None, None)
        );
        assert_eq!(eps[1].title, None);
    }

    #[test]
    fn a_switch_leaves_nothing_of_the_old_accounts_shows() {
        let c = open_in_memory().unwrap();
        let (cats, shows) = three();
        set_origin(&c, "A");
        land_series(&c, &cats, &shows, 100).unwrap();
        land_series_details(&c, 1, 110, "{}", &[ep(11, 1, 1, 1), ep(12, 1, 1, 2)]).unwrap();
        set_origin(&c, "B");
        let b = [show(1, "x", "Other One"), show(5, "x", "Five")];
        let s = land_series(&c, &[cat("x", "Theirs")], &b, 200).unwrap();
        assert_eq!(s.deleted, 2);
        assert_eq!(count(&c, "SELECT COUNT(*) FROM series_details"), 0);
        assert_eq!(count(&c, "SELECT COUNT(*) FROM episodes"), 0);
        let counts: (Option<i64>, Option<i64>) = c
            .query_row(
                "SELECT season_count, episode_count FROM series WHERE series_id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(counts, (None, None));
        let live: Vec<i64> = c
            .prepare("SELECT series_id FROM series WHERE deleted_at IS NULL ORDER BY series_id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(live, [1, 5]);
        // The new account's empty list is believed; its own is not.
        set_origin(&c, "C");
        assert!(land_series(&c, &[], &[], 300).is_ok());
        assert!(land_series(&c, &[], &b, 400).is_ok());
        assert!(land_series(&c, &[], &[], 500).is_err());
    }

    #[test]
    fn the_interface_is_told_when_the_shows_are_another_accounts() {
        let c = open_in_memory().unwrap();
        assert_eq!(series_origin(&c).unwrap(), "", "none landed yet");
        let (cats, shows) = three();
        set_origin(&c, "xtream:00000000000000aa");
        land_series(&c, &cats, &shows, 100).unwrap();
        let a = series_origin(&c).unwrap();
        assert_eq!(a.len(), 16);
        assert!(!a.contains("xtream"), "{a}");
        land_series(&c, &cats, &shows, 150).unwrap();
        assert_eq!(series_origin(&c).unwrap(), a);
        set_origin(&c, "xtream:00000000000000bb");
        land_series(&c, &cats, &shows, 200).unwrap();
        assert_ne!(series_origin(&c).unwrap(), a);
    }

    #[test]
    fn separators_are_skipped_and_a_renamed_show_loses_its_details() {
        let c = open_in_memory().unwrap();
        let shows = [show(1, "a", "## ROW ##"), show(2, "a", "Two")];
        land_series(&c, &[cat("a", "A")], &shows, 1).unwrap();
        assert_eq!(count(&c, "SELECT COUNT(*) FROM series"), 1);
        land_series_details(&c, 2, 5, "{}", &[ep(21, 2, 1, 1)]).unwrap();
        land_series(&c, &[cat("a", "A")], &shows, 10).unwrap();
        assert_eq!(count(&c, "SELECT COUNT(*) FROM series_details"), 1);
        let renamed = [show(2, "a", "Another")];
        land_series(&c, &[cat("a", "A")], &renamed, 20).unwrap();
        assert_eq!(count(&c, "SELECT COUNT(*) FROM series_details"), 0);
        // Its episodes were the other show's.
        assert_eq!(
            count(&c, "SELECT deleted_at FROM episodes WHERE episode_id = 21"),
            20
        );
        let counts: (Option<i64>, Option<i64>) = c
            .query_row(
                "SELECT season_count, episode_count FROM series WHERE series_id = 2",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(counts, (None, None));
    }

    #[test]
    fn an_empty_episode_list_keeps_the_episodes_already_here() {
        let c = open_in_memory().unwrap();
        let (cats, shows) = three();
        land_series(&c, &cats, &shows, 100).unwrap();
        // A show that never had any may have none.
        land_series_details(&c, 2, 110, "{}", &[]).unwrap();
        land_series_details(&c, 1, 120, "{}", &[ep(11, 1, 1, 1), ep(12, 1, 1, 2)]).unwrap();
        let err = land_series_details(&c, 1, 130, "{}", &[])
            .unwrap_err()
            .to_string();
        assert!(err.contains("keeping the 2"), "{err}");
        assert_eq!(
            count(&c, "SELECT COUNT(*) FROM episodes WHERE deleted_at IS NULL"),
            2
        );
        assert_eq!(
            count(
                &c,
                "SELECT fetched_at FROM series_details WHERE series_id = 1"
            ),
            120
        );
    }

    #[test]
    fn details_date_their_undated_episodes_when_first_seen() {
        let c = open_in_memory().unwrap();
        let (cats, shows) = three();
        land_series(&c, &cats, &shows, 100).unwrap();
        let mut dated = ep(12, 1, 1, 2);
        dated.added = Some(50);
        land_series_details(&c, 1, 200, "{}", &[ep(11, 1, 1, 1), dated.clone()]).unwrap();
        land_series_details(&c, 1, 300, "{}", &[ep(11, 1, 1, 1), dated, ep(13, 1, 1, 3)]).unwrap();
        let added: Vec<i64> = c
            .prepare("SELECT added FROM episodes ORDER BY episode_id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(added, [200, 50, 300]);
    }

    #[test]
    fn a_switch_names_the_shows_whose_episodes_went() {
        let c = open_in_memory().unwrap();
        let (cats, shows) = three();
        set_origin(&c, "A");
        land_series(&c, &cats, &shows, 100).unwrap();
        land_series_details(&c, 1, 110, "{}", &[ep(11, 1, 1, 1)]).unwrap();
        land_series_details(&c, 3, 110, "{}", &[ep(31, 3, 1, 1)]).unwrap();
        set_origin(&c, "B");
        let tx = c.unchecked_transaction().unwrap();
        let mut w = SeriesWriter::begin(&tx).unwrap();
        w.push_series(&show(5, "x", "Five"), None).unwrap();
        let landed = w.finish(200).unwrap();
        tx.commit().unwrap();
        assert_eq!(landed.changed_shows, [1, 3]);
        assert_eq!(count(&c, "SELECT COUNT(*) FROM episodes"), 0);
    }

    fn write(c: &Connection, now: i64, eps: &[NewEpisode]) -> SeriesLanded {
        let tx = c.unchecked_transaction().unwrap();
        let mut w = SeriesWriter::begin(&tx).unwrap();
        w.own_episodes();
        w.accept_empty();
        for e in eps {
            let s = show(e.series_id, "", &format!("Show {}", e.series_id));
            w.push_series(&s, Some("Shows")).unwrap();
            w.push_episode(e).unwrap();
        }
        let landed = w.finish(now).unwrap();
        tx.commit().unwrap();
        landed
    }

    #[test]
    fn a_playlist_show_is_as_new_as_its_newest_episode() {
        let c = open_in_memory().unwrap();
        let first = write(&c, 100, &[ep(11, 1, 1, 1)]);
        assert_eq!(added(&c, 1), 100);
        assert_eq!((first.episodes, first.categories), (1, 1));
        assert_eq!(first.changed_shows, [1]);
        let again = write(&c, 500, &[ep(11, 1, 1, 1)]);
        assert_eq!(added(&c, 1), 100, "nothing new, so it does not move");
        assert!(again.changed_shows.is_empty(), "{:?}", again.changed_shows);
        let more = write(&c, 900, &[ep(11, 1, 1, 1), ep(12, 1, 1, 2)]);
        assert_eq!(added(&c, 1), 900);
        assert_eq!(more.changed_shows, [1]);
        assert_eq!(
            count(&c, "SELECT added FROM episodes WHERE episode_id = 11"),
            100
        );
        assert_eq!(
            count(&c, "SELECT episode_count FROM series WHERE series_id = 1"),
            2
        );
    }

    #[test]
    fn a_playlist_write_owns_its_episodes() {
        let c = open_in_memory().unwrap();
        write(
            &c,
            100,
            &[ep(11, 1, 1, 1), ep(12, 1, 1, 2), ep(21, 2, 1, 1)],
        );
        let landed = write(&c, 200, &[ep(11, 1, 1, 1), ep(13, 1, 2, 1)]);
        assert_eq!(landed.changed_shows, [1, 2]);
        assert_eq!(landed.stats.deleted, 1, "show 2");
        assert_eq!(
            count(&c, "SELECT deleted_at FROM episodes WHERE episode_id = 12"),
            200
        );
        let counts: (i64, i64) = c
            .query_row(
                "SELECT season_count, episode_count FROM series WHERE series_id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(counts, (2, 2));
    }

    #[test]
    fn an_episode_that_moves_changes_both_shows() {
        let c = open_in_memory().unwrap();
        write(&c, 100, &[ep(11, 1, 1, 1), ep(12, 1, 1, 2)]);
        let landed = write(&c, 200, &[ep(11, 2, 1, 1), ep(12, 1, 1, 2)]);
        assert_eq!(landed.changed_shows, [1, 2]);
        let episodes = |id: i64| -> i64 {
            c.query_row(
                "SELECT episode_count FROM series WHERE series_id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!((episodes(1), episodes(2)), (1, 1));
        assert_eq!(
            count(&c, "SELECT series_id FROM episodes WHERE episode_id = 11"),
            2
        );
    }

    #[test]
    fn a_playlist_write_recounts_only_the_shows_that_changed() {
        let c = open_in_memory().unwrap();
        write(&c, 100, &[ep(11, 1, 1, 1), ep(21, 2, 1, 1)]);
        // A count no write would set, to see which shows are recounted.
        c.execute("UPDATE series SET episode_count = 99", [])
            .unwrap();
        let landed = write(
            &c,
            200,
            &[ep(11, 1, 1, 1), ep(21, 2, 1, 1), ep(22, 2, 1, 2)],
        );
        assert_eq!(landed.changed_shows, [2]);
        assert_eq!(
            count(&c, "SELECT episode_count FROM series WHERE series_id = 1"),
            99
        );
        assert_eq!(
            count(&c, "SELECT episode_count FROM series WHERE series_id = 2"),
            2
        );
    }
}
