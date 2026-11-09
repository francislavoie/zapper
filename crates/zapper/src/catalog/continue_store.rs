//! Continue watching: one list of the movies left partway and, for each
//! show being watched, its next-up episode, most recently watched first.
//!
//! Built from the current catalogue's marks alone (`vod_marks`, keyed by
//! origin), never from the provider: building it opens no connection and
//! asks nothing, so a show whose newer episodes have not landed yet (its
//! details not opened since) cannot be listed for them.
//!
//! - A movie is listed while it is in progress: its place between
//!   [`STARTED_FRACTION`] and [`WATCHED_FRACTION`] of its length (the
//!   player's measured `duration`, else the provider's `duration_secs`; an
//!   unknown length never is) and not marked watched.
//! - A show is listed once at least one of its episodes, specials aside, is
//!   watched (played past the line or marked) or in progress, and it has a
//!   next-up episode ([`next_up`]) to show ([`show_entry`]). A show started
//!   over (every episode unmarked) or with every known episode watched is
//!   not listed.
//! - Removing an entry ([`remove_from_continue`]) hides it until it is
//!   watched again and keeps the place: `vod_marks.dismissed_at` on the
//!   movie's or the show's row, the entry showing while its last activity
//!   is later than that. A movie's last activity is its `played_at`; a
//!   show's is its episodes' latest `played_at` or `watched_at`, so playing
//!   or marking any episode brings the show back.
//! - The list holds the [`CONTINUE_MAX`] most recent entries.
//!
//! [`STARTED_FRACTION`]: crate::catalog::vod_progress::STARTED_FRACTION
//! [`WATCHED_FRACTION`]: crate::catalog::vod_progress::WATCHED_FRACTION

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use crate::catalog::origin::current_origin_sql;
use crate::catalog::series_store::{
    next_up, series_by_ids, series_episodes, EpisodeRow, SeriesRow,
};
use crate::catalog::vod_progress::resume_point;
use crate::catalog::vod_store::{self, ItemKind, MovieRow, VodItem};

/// The most entries the list holds.
pub const CONTINUE_MAX: u32 = 100;

/// What an entry is removed by: the movie's id, or the show's (never an
/// episode's: a show is one entry, whichever episode it shows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinueTarget {
    Movie(i64),
    Series(i64),
}

/// One entry: a movie in progress, or a show's next-up episode (`up_next`
/// when that episode is not itself in progress, so it starts from the
/// beginning). `last_watched` is the entry's last activity, in seconds.
#[derive(Debug, Clone, PartialEq)]
pub enum ContinueRow {
    Movie {
        movie: MovieRow,
        last_watched: i64,
    },
    Episode {
        series: SeriesRow,
        episode: EpisodeRow,
        up_next: bool,
        last_watched: i64,
    },
}

impl ContinueRow {
    pub fn last_watched(&self) -> i64 {
        match self {
            Self::Movie { last_watched, .. } | Self::Episode { last_watched, .. } => *last_watched,
        }
    }

    /// The order of two entries last watched at the same second: by id,
    /// a movie before a show with the same.
    fn tie(&self) -> (i64, u8) {
        match self {
            Self::Movie { movie, .. } => (movie.movie_id, 0),
            Self::Episode { series, .. } => (series.series_id, 1),
        }
    }
}

/// Whether an episode is in progress: it has a place to resume.
fn in_progress(e: &EpisodeRow) -> bool {
    resume_point(e.position, e.duration, e.duration_secs, e.watched).is_some()
}

/// A show's entry, from its live episodes: the next-up episode and whether
/// it is up next (not in progress). None unless at least one episode,
/// specials aside, is watched or in progress, and none with nothing next
/// (every episode watched). So a show never started, or started over, is
/// not an entry.
pub fn show_entry(eps: &[EpisodeRow]) -> Option<(i64, bool)> {
    let started = eps
        .iter()
        .any(|e| e.season != 0 && (e.watched || in_progress(e)));
    if !started {
        return None;
    }
    let id = next_up(eps)?;
    let episode = eps.iter().find(|e| e.episode_id == id)?;
    Some((id, !in_progress(episode)))
}

/// How many shows' episodes, at most, one build reads per entry it may
/// list: a backstop. The candidates are already only those
/// [`show_entry`] would most likely list (see [`SHOWS_SQL`]), so only a
/// show whose places are all under [`STARTED_FRACTION`] or of unknown
/// length can be read and not listed.
///
/// [`STARTED_FRACTION`]: crate::catalog::vod_progress::STARTED_FRACTION
const CANDIDATES_PER_ENTRY: u32 = 4;

/// The shows that may be entries, each with its last activity, newest
/// first, at most `?1`. A live show with a live episode played or marked,
/// and:
///
/// - played or marked since it was last removed (`dismissed_at`);
/// - not watched: the show's own mark, kept current by
///   [`refresh_series_watched_in_tx`], is set exactly when every live
///   episode, specials aside, is, which is when there is no next-up
///   episode;
/// - with an episode, specials aside, watched or holding a place: a show
///   started over keeps its episodes' `played_at` but neither of those.
///
/// What is left is exact but for the place's fraction, which
/// [`show_entry`] judges from the episodes.
///
/// [`refresh_series_watched_in_tx`]: crate::catalog::series_store::refresh_series_watched_in_tx
const SHOWS_SQL: &str = concat!(
    "SELECT e.series_id,
            MAX(MAX(COALESCE(k.played_at, k.watched_at),
                    COALESCE(k.watched_at, k.played_at))) AS last
     FROM vod_marks k
     JOIN episodes e ON e.episode_id = k.item_id AND e.deleted_at IS NULL
     JOIN series s ON s.series_id = e.series_id AND s.deleted_at IS NULL
     LEFT JOIN vod_marks d ON d.origin = k.origin AND d.kind = 'series'
                          AND d.item_id = e.series_id
     WHERE k.origin = ",
    current_origin_sql!(),
    " AND k.kind = 'episode'
     GROUP BY e.series_id
     HAVING last IS NOT NULL AND last > COALESCE(MAX(d.dismissed_at), -1)
        AND MAX(d.watched_at) IS NULL
        AND MAX(e.season <> 0 AND (k.watched_at IS NOT NULL OR k.position IS NOT NULL))
     ORDER BY last DESC, e.series_id LIMIT ?1"
);

/// The shows that may be entries, for a list of `limit`: (show, last
/// activity), newest first.
fn show_candidates(conn: &Connection, limit: u32) -> Result<Vec<(i64, i64)>> {
    let mut stmt = conn.prepare_cached(SHOWS_SQL)?;
    let rows = stmt
        .query_map([limit * CANDIDATES_PER_ENTRY], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

fn movies_in_progress(conn: &Connection, limit: u32) -> Result<Vec<ContinueRow>> {
    Ok(vod_store::movies_in_progress(conn, limit)?
        .into_iter()
        .map(|movie| ContinueRow::Movie {
            last_watched: movie.played_at.unwrap_or_default(),
            movie,
        })
        .collect())
}

fn shows_in_progress(conn: &Connection, limit: u32) -> Result<Vec<ContinueRow>> {
    let mut out = Vec::new();
    for (series_id, last_watched) in show_candidates(conn, limit)? {
        if out.len() >= limit as usize {
            break;
        }
        let Some(series) = series_by_ids(conn, &[series_id])?.pop() else {
            continue;
        };
        let mut eps = series_episodes(conn, series_id)?;
        let Some((id, up_next)) = show_entry(&eps) else {
            continue;
        };
        let Some(at) = eps.iter().position(|e| e.episode_id == id) else {
            continue;
        };
        out.push(ContinueRow::Episode {
            series,
            episode: eps.swap_remove(at),
            up_next,
            last_watched,
        });
    }
    Ok(out)
}

/// The current catalogue's Continue watching, most recently watched first
/// (ties by id), at most `limit` entries (clamped to `1..=`[`CONTINUE_MAX`]).
/// Never touches the network.
pub fn continue_watching(conn: &Connection, limit: u32) -> Result<Vec<ContinueRow>> {
    let limit = limit.clamp(1, CONTINUE_MAX);
    let mut rows = movies_in_progress(conn, limit)?;
    rows.extend(shows_in_progress(conn, limit)?);
    rows.sort_by(|a, b| {
        b.last_watched()
            .cmp(&a.last_watched())
            .then_with(|| a.tie().cmp(&b.tie()))
    });
    rows.truncate(limit as usize);
    Ok(rows)
}

/// Removes a movie or a show from Continue watching as of `at`: hidden
/// until it is watched again, its place kept (see the module notes). False,
/// with nothing written, for a title not live in the catalogue.
pub fn remove_from_continue(conn: &Connection, target: ContinueTarget, at: i64) -> Result<bool> {
    let (kind, live, id) = match target {
        ContinueTarget::Movie(id) => (
            ItemKind::Movie,
            "SELECT 1 FROM movies WHERE movie_id = ?1 AND deleted_at IS NULL",
            id,
        ),
        ContinueTarget::Series(id) => (
            ItemKind::Series,
            "SELECT 1 FROM series WHERE series_id = ?1 AND deleted_at IS NULL",
            id,
        ),
    };
    if conn.query_row(live, [id], |_| Ok(())).optional()?.is_none() {
        return Ok(false);
    }
    let origin = vod_store::origin(conn)?;
    conn.execute(
        "INSERT INTO vod_marks (origin, kind, item_id, dismissed_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(origin, kind, item_id) DO UPDATE SET dismissed_at = excluded.dismissed_at",
        params![origin, kind.as_str(), id, at],
    )?;
    Ok(true)
}

/// Where to resume a movie or an episode, from its current marks and its
/// listed length; None to play it from the start.
pub fn resume_point_of(conn: &Connection, item: VodItem) -> Result<Option<f64>> {
    let listed = match item.kind {
        ItemKind::Movie => "SELECT duration_secs FROM movies WHERE movie_id = ?1",
        ItemKind::Episode => "SELECT duration_secs FROM episodes WHERE episode_id = ?1",
        ItemKind::Series => return Ok(None),
    };
    let listed: Option<i64> = conn
        .query_row(listed, [item.id], |r| r.get(0))
        .optional()?
        .flatten();
    let origin = vod_store::origin(conn)?;
    let marks = vod_store::marks_of(conn, &origin, item.kind, item.id)?;
    Ok(resume_point(
        marks.position,
        marks.duration,
        listed,
        marks.watched,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::open_in_memory;
    use crate::catalog::series_store::set_series_marks;
    use crate::catalog::series_sync::{land_series, land_series_details, NewEpisode, NewSeries};
    use crate::catalog::vod_progress::{STARTED_FRACTION, WATCHED_FRACTION};
    use crate::catalog::vod_store::{marks_of, origin, record_progress, set_marks, ItemKind};
    use rusqlite::params;

    /// Movie `id`, with the provider's length when it says one.
    fn movie(c: &Connection, id: i64, listed: Option<i64>) {
        c.execute(
            "INSERT INTO movies (movie_id, name, clean_name, added, duration_secs)
             VALUES (?1, 'A Film', 'A Film', 1, ?2)",
            params![id, listed],
        )
        .unwrap();
    }

    fn play_movie(c: &Connection, id: i64, position: f64, measured: Option<f64>, at: i64) {
        record_progress(c, ItemKind::Movie, id, position, measured, at).unwrap();
    }

    /// These shows, as one sync lists them.
    fn shows(c: &Connection, ids: &[i64]) {
        let list: Vec<NewSeries> = ids
            .iter()
            .map(|&series_id| NewSeries {
                series_id,
                name: "A Show".into(),
                ..NewSeries::default()
            })
            .collect();
        land_series(c, &[], &list, 1).unwrap();
    }

    /// Episode `id` of show `series`.
    fn ep(series: i64, id: i64, season: i32, n: i32) -> NewEpisode {
        NewEpisode {
            episode_id: id,
            series_id: series,
            season,
            episode: n,
            ..NewEpisode::default()
        }
    }

    /// Show `series`'s episodes, as its details land them.
    fn episodes(c: &Connection, series: i64, eps: &[NewEpisode]) {
        land_series_details(c, series, 1, "{}", eps).unwrap();
    }

    /// Show 1 with S1E1–E3 (episodes 11, 12, 13).
    fn a_show(c: &Connection) {
        shows(c, &[1]);
        episodes(c, 1, &[ep(1, 11, 1, 1), ep(1, 12, 1, 2), ep(1, 13, 1, 3)]);
    }

    fn play_episode(c: &Connection, id: i64, position: f64, at: i64) {
        record_progress(c, ItemKind::Episode, id, position, Some(100.0), at).unwrap();
    }

    fn watch_episode(c: &Connection, id: i64, at: i64) {
        set_marks(c, ItemKind::Episode, id, None, Some(true), at).unwrap();
    }

    fn set_origin(c: &Connection, o: &str) {
        c.execute(
            "INSERT INTO meta (key, value) VALUES ('catalogue_origin', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [o],
        )
        .unwrap();
    }

    /// The list, as `movie 2` and `show 1: 12` (`… next` when up next).
    fn listed(c: &Connection) -> Vec<String> {
        continue_watching(c, CONTINUE_MAX)
            .unwrap()
            .iter()
            .map(|r| match r {
                ContinueRow::Movie { movie, .. } => format!("movie {}", movie.movie_id),
                ContinueRow::Episode {
                    series,
                    episode,
                    up_next,
                    ..
                } => format!(
                    "show {}: {}{}",
                    series.series_id,
                    episode.episode_id,
                    if *up_next { " next" } else { "" }
                ),
            })
            .collect()
    }

    #[test]
    fn a_movie_in_progress_is_listed_newest_first() {
        let c = open_in_memory().unwrap();
        movie(&c, 1, None);
        movie(&c, 2, None);
        play_movie(&c, 1, 30.0, Some(100.0), 10);
        play_movie(&c, 2, 50.0, Some(100.0), 20);
        assert_eq!(listed(&c), ["movie 2", "movie 1"]);
        let rows = continue_watching(&c, CONTINUE_MAX).unwrap();
        let ContinueRow::Movie {
            movie,
            last_watched,
        } = &rows[1]
        else {
            panic!("{rows:?}");
        };
        assert_eq!(
            (
                movie.position,
                movie.duration,
                movie.played_at,
                *last_watched
            ),
            (Some(30.0), Some(100.0), Some(10), 10)
        );
        assert_eq!(rows[0].last_watched(), 20);
    }

    #[test]
    fn under_one_percent_or_from_ninety_two_percent_is_not_in_progress() {
        let c = open_in_memory().unwrap();
        movie(&c, 1, None);
        movie(&c, 2, None);
        play_movie(&c, 1, 0.5, Some(100.0), 10);
        play_movie(&c, 2, 92.0, Some(100.0), 10);
        assert!(listed(&c).is_empty());
        // At 92 % even with no watched mark (one taken off by hand keeps no
        // place, but a place written by an older build might).
        c.execute("UPDATE vod_marks SET watched_at = NULL", [])
            .unwrap();
        assert!(listed(&c).is_empty());
        play_movie(&c, 1, 1.0, Some(100.0), 11);
        assert_eq!(listed(&c), ["movie 1"]);
    }

    #[test]
    fn a_title_of_unknown_length_is_never_in_progress() {
        let c = open_in_memory().unwrap();
        movie(&c, 1, None);
        play_movie(&c, 1, 30.0, None, 10);
        assert!(listed(&c).is_empty());
        c.execute(
            "UPDATE movies SET duration_secs = 100 WHERE movie_id = 1",
            [],
        )
        .unwrap();
        assert_eq!(listed(&c), ["movie 1"]);
    }

    #[test]
    fn the_measured_length_wins_over_the_providers() {
        let c = open_in_memory().unwrap();
        movie(&c, 1, Some(6000));
        play_movie(&c, 1, 95.0, Some(100.0), 10);
        assert!(marks_of(&c, "", ItemKind::Movie, 1).unwrap().watched);
        assert!(listed(&c).is_empty());
        c.execute("UPDATE vod_marks SET watched_at = NULL", [])
            .unwrap();
        assert!(
            listed(&c).is_empty(),
            "95 of a measured 100 is past the line"
        );
        // And the other way: a listed 100 that the player measured as 6000.
        movie(&c, 2, Some(100));
        play_movie(&c, 2, 95.0, Some(6000.0), 20);
        assert_eq!(listed(&c), ["movie 2"]);
    }

    #[test]
    fn a_watched_movie_leaves_the_list() {
        let c = open_in_memory().unwrap();
        movie(&c, 1, None);
        play_movie(&c, 1, 30.0, Some(100.0), 10);
        set_marks(&c, ItemKind::Movie, 1, None, Some(true), 20).unwrap();
        assert!(listed(&c).is_empty());
    }

    #[test]
    fn a_shows_entry_is_its_episode_in_progress() {
        let c = open_in_memory().unwrap();
        a_show(&c);
        watch_episode(&c, 11, 5);
        play_episode(&c, 12, 40.0, 10);
        assert_eq!(listed(&c), ["show 1: 12"]);
        let rows = continue_watching(&c, CONTINUE_MAX).unwrap();
        let ContinueRow::Episode {
            series,
            episode,
            last_watched,
            ..
        } = &rows[0]
        else {
            panic!("{rows:?}");
        };
        assert_eq!(series.name, "A Show");
        assert_eq!((episode.season, episode.episode), (1, 2));
        assert_eq!((episode.position, *last_watched), (Some(40.0), 10));
    }

    #[test]
    fn after_an_episode_is_watched_the_entry_is_the_next_one() {
        let c = open_in_memory().unwrap();
        a_show(&c);
        play_episode(&c, 11, 95.0, 10);
        assert_eq!(listed(&c), ["show 1: 12 next"]);
    }

    #[test]
    fn after_a_range_the_entry_is_past_its_end() {
        let c = open_in_memory().unwrap();
        shows(&c, &[1]);
        let range = NewEpisode {
            episode_end: Some(2),
            ..ep(1, 11, 1, 1)
        };
        episodes(&c, 1, &[range, ep(1, 13, 1, 3)]);
        watch_episode(&c, 11, 5);
        assert_eq!(listed(&c), ["show 1: 13 next"]);
    }

    #[test]
    fn a_finished_show_is_not_listed() {
        let c = open_in_memory().unwrap();
        shows(&c, &[1]);
        episodes(&c, 1, &[ep(1, 10, 0, 1), ep(1, 11, 1, 1), ep(1, 12, 1, 2)]);
        watch_episode(&c, 11, 5);
        play_episode(&c, 12, 99.0, 6);
        assert!(
            listed(&c).is_empty(),
            "the special left unwatched does not count"
        );
    }

    #[test]
    fn a_show_started_over_is_not_listed() {
        let c = open_in_memory().unwrap();
        a_show(&c);
        play_episode(&c, 11, 95.0, 10);
        play_episode(&c, 12, 40.0, 11);
        set_series_marks(&c, 1, None, Some(false), 20).unwrap();
        assert!(listed(&c).is_empty());
    }

    #[test]
    fn an_episode_marked_by_hand_lists_its_show() {
        let c = open_in_memory().unwrap();
        a_show(&c);
        watch_episode(&c, 11, 5);
        assert_eq!(listed(&c), ["show 1: 12 next"]);
        assert_eq!(continue_watching(&c, 1).unwrap()[0].last_watched(), 5);
    }

    #[test]
    fn movies_and_shows_mix_by_last_watched() {
        let c = open_in_memory().unwrap();
        movie(&c, 1, None);
        movie(&c, 2, None);
        a_show(&c);
        play_movie(&c, 1, 30.0, Some(100.0), 10);
        play_episode(&c, 11, 40.0, 30);
        play_movie(&c, 2, 30.0, Some(100.0), 20);
        assert_eq!(listed(&c), ["show 1: 11", "movie 2", "movie 1"]);
    }

    #[test]
    fn removed_stays_removed_until_played_again() {
        let c = open_in_memory().unwrap();
        movie(&c, 1, None);
        play_movie(&c, 1, 30.0, Some(100.0), 10);
        assert!(remove_from_continue(&c, ContinueTarget::Movie(1), 50).unwrap());
        assert!(listed(&c).is_empty());
        play_movie(&c, 1, 31.0, Some(100.0), 60);
        assert_eq!(listed(&c), ["movie 1"]);
    }

    #[test]
    fn removing_keeps_the_place() {
        let c = open_in_memory().unwrap();
        movie(&c, 1, None);
        play_movie(&c, 1, 30.0, Some(100.0), 10);
        remove_from_continue(&c, ContinueTarget::Movie(1), 20).unwrap();
        let marks = marks_of(&c, &origin(&c).unwrap(), ItemKind::Movie, 1).unwrap();
        assert_eq!((marks.position, marks.played_at), (Some(30.0), Some(10)));
        assert_eq!(resume_point_of(&c, VodItem::movie(1)).unwrap(), Some(30.0));
    }

    #[test]
    fn removing_a_show_hides_it_until_an_episode_plays() {
        let c = open_in_memory().unwrap();
        a_show(&c);
        play_episode(&c, 11, 40.0, 10);
        assert!(remove_from_continue(&c, ContinueTarget::Series(1), 20).unwrap());
        assert!(listed(&c).is_empty());
        play_episode(&c, 12, 40.0, 30);
        assert_eq!(listed(&c), ["show 1: 12"]);
        // Marking one watched brings it back too.
        remove_from_continue(&c, ContinueTarget::Series(1), 40).unwrap();
        assert!(listed(&c).is_empty());
        watch_episode(&c, 11, 50);
        assert_eq!(listed(&c), ["show 1: 12"]);
    }

    #[test]
    fn movie_7_and_show_7_are_removed_apart() {
        let c = open_in_memory().unwrap();
        movie(&c, 7, None);
        shows(&c, &[7]);
        episodes(&c, 7, &[ep(7, 7, 1, 1), ep(7, 8, 1, 2)]);
        play_movie(&c, 7, 30.0, Some(100.0), 10);
        play_episode(&c, 7, 40.0, 20);
        assert_eq!(listed(&c), ["show 7: 7", "movie 7"]);
        remove_from_continue(&c, ContinueTarget::Series(7), 30).unwrap();
        assert_eq!(listed(&c), ["movie 7"]);
        let o = origin(&c).unwrap();
        assert_eq!(
            marks_of(&c, &o, ItemKind::Episode, 7).unwrap().position,
            Some(40.0)
        );
        remove_from_continue(&c, ContinueTarget::Movie(7), 30).unwrap();
        play_episode(&c, 7, 41.0, 40);
        assert_eq!(listed(&c), ["show 7: 7"]);
    }

    #[test]
    fn removing_an_unknown_title_writes_nothing() {
        let c = open_in_memory().unwrap();
        movie(&c, 1, None);
        c.execute("UPDATE movies SET deleted_at = 9 WHERE movie_id = 1", [])
            .unwrap();
        assert!(!remove_from_continue(&c, ContinueTarget::Movie(999), 5).unwrap());
        assert!(!remove_from_continue(&c, ContinueTarget::Series(999), 5).unwrap());
        assert!(!remove_from_continue(&c, ContinueTarget::Movie(1), 5).unwrap());
        let n: i64 = c
            .query_row("SELECT COUNT(*) FROM vod_marks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn continue_watching_belongs_to_the_catalogue() {
        let c = open_in_memory().unwrap();
        movie(&c, 1, None);
        a_show(&c);
        set_origin(&c, "A");
        play_movie(&c, 1, 30.0, Some(100.0), 10);
        play_episode(&c, 11, 40.0, 20);
        set_origin(&c, "B");
        assert!(listed(&c).is_empty());
        assert_eq!(resume_point_of(&c, VodItem::movie(1)).unwrap(), None);
        // Removing here is this catalogue's, and leaves A's alone.
        remove_from_continue(&c, ContinueTarget::Movie(1), 30).unwrap();
        set_origin(&c, "A");
        assert_eq!(listed(&c), ["show 1: 11", "movie 1"]);
    }

    #[test]
    fn a_deleted_title_is_not_listed() {
        let c = open_in_memory().unwrap();
        movie(&c, 1, None);
        movie(&c, 2, None);
        a_show(&c);
        play_movie(&c, 1, 30.0, Some(100.0), 10);
        play_movie(&c, 2, 30.0, Some(100.0), 20);
        watch_episode(&c, 11, 30);
        play_episode(&c, 12, 40.0, 40);
        c.execute("UPDATE movies SET deleted_at = 9 WHERE movie_id = 1", [])
            .unwrap();
        assert_eq!(listed(&c), ["show 1: 12", "movie 2"]);
        c.execute(
            "UPDATE episodes SET deleted_at = 9 WHERE episode_id = 12",
            [],
        )
        .unwrap();
        assert_eq!(listed(&c), ["show 1: 13 next", "movie 2"]);
        c.execute("UPDATE series SET deleted_at = 9 WHERE series_id = 1", [])
            .unwrap();
        assert_eq!(listed(&c), ["movie 2"]);
    }

    #[test]
    fn the_list_is_capped() {
        let c = open_in_memory().unwrap();
        for i in 0..120 {
            movie(&c, i, None);
            play_movie(&c, i, 30.0, Some(100.0), i);
        }
        let rows = continue_watching(&c, 100).unwrap();
        assert_eq!(rows.len(), 100);
        assert_eq!(rows[0].last_watched(), 119);
        assert_eq!(continue_watching(&c, 0).unwrap().len(), 1);
        assert_eq!(continue_watching(&c, 500).unwrap().len(), 100);
    }

    fn plan(c: &Connection, sql: &str, args: impl rusqlite::Params) -> String {
        let mut stmt = c.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let rows: Vec<String> = stmt
            .query_map(args, |r| r.get(3))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        rows.join("\n")
    }

    #[test]
    fn the_continue_query_uses_an_index() {
        let c = open_in_memory().unwrap();
        let p = plan(
            &c,
            vod_store::movies_in_progress_sql(),
            params![STARTED_FRACTION, WATCHED_FRACTION, CONTINUE_MAX],
        );
        assert!(p.contains("vod_marks_by_played"), "{p}");
        assert!(!p.contains("SCAN m"), "{p}");
        assert!(!p.contains("SCAN movies"), "{p}");
        let p = plan(&c, SHOWS_SQL, [CONTINUE_MAX * CANDIDATES_PER_ENTRY]);
        for full in [
            "SCAN k",
            "SCAN d",
            "SCAN e",
            "SCAN s",
            "SCAN episodes",
            "SCAN series",
        ] {
            assert!(!p.contains(full), "{full}: {p}");
        }
    }

    #[test]
    fn finished_and_started_over_shows_are_never_read() {
        let c = open_in_memory().unwrap();
        shows(&c, &[1, 2, 3]);
        episodes(&c, 1, &[ep(1, 11, 1, 1), ep(1, 12, 1, 2)]);
        episodes(&c, 2, &[ep(2, 21, 1, 1), ep(2, 22, 1, 2)]);
        episodes(&c, 3, &[ep(3, 31, 1, 1), ep(3, 32, 1, 2)]);
        // Show 1 finished; show 2 played, then started over; show 3 going.
        watch_episode(&c, 11, 10);
        play_episode(&c, 12, 99.0, 11);
        play_episode(&c, 21, 95.0, 20);
        play_episode(&c, 22, 40.0, 21);
        set_series_marks(&c, 2, None, Some(false), 22).unwrap();
        play_episode(&c, 31, 40.0, 5);
        assert_eq!(show_candidates(&c, CONTINUE_MAX).unwrap(), [(3, 5)]);
        assert_eq!(listed(&c), ["show 3: 31"]);
        // A finished show that gains an episode is a candidate again.
        episodes(&c, 1, &[ep(1, 11, 1, 1), ep(1, 12, 1, 2), ep(1, 13, 1, 3)]);
        assert_eq!(
            show_candidates(&c, CONTINUE_MAX).unwrap(),
            [(1, 11), (3, 5)]
        );
        assert_eq!(listed(&c), ["show 1: 13 next", "show 3: 31"]);
    }

    #[test]
    fn the_show_candidates_are_capped() {
        let c = open_in_memory().unwrap();
        let ids: Vec<i64> = (1..=10).collect();
        shows(&c, &ids);
        for &s in &ids {
            episodes(&c, s, &[ep(s, s * 10, 1, 1), ep(s, s * 10 + 1, 1, 2)]);
            // Barely started: a candidate, but never an entry.
            play_episode(&c, s * 10, 0.5, s);
        }
        assert_eq!(show_candidates(&c, 2).unwrap().len(), 8);
        assert!(listed(&c).is_empty());
    }

    #[test]
    fn a_movie_and_a_show_watched_in_the_same_second_go_by_id() {
        let c = open_in_memory().unwrap();
        movie(&c, 1, None);
        movie(&c, 2, None);
        a_show(&c);
        play_movie(&c, 2, 30.0, Some(100.0), 10);
        play_episode(&c, 11, 40.0, 10);
        play_movie(&c, 1, 30.0, Some(100.0), 10);
        assert_eq!(listed(&c), ["movie 1", "show 1: 11", "movie 2"]);
    }

    #[test]
    fn just_under_ninety_two_percent_is_in_progress() {
        let c = open_in_memory().unwrap();
        movie(&c, 1, None);
        play_movie(&c, 1, 91.9, Some(100.0), 10);
        assert_eq!(listed(&c), ["movie 1"]);
    }

    #[test]
    fn a_show_needs_an_episode_watched_or_in_progress() {
        let c = open_in_memory().unwrap();
        a_show(&c);
        play_episode(&c, 11, 0.5, 10);
        assert!(listed(&c).is_empty(), "barely started is not started");
        let eps = series_episodes(&c, 1).unwrap();
        assert_eq!(show_entry(&eps), None);
    }
}
