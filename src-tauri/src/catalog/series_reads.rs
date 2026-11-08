//! The shows' reads and marks, as the interface asks for them: one place
//! for the app's commands (`ipc::series_cmds`) and the recording service's
//! web API to answer from, as [`crate::catalog::reads`] is for channels and
//! movies.
//!
//! Every read answers from SQLite alone. The one thing here that may ask
//! the provider is marking a show watched whose episodes are not here yet:
//! an Xtream server's arrive with the show's details, and a show is watched
//! when its episodes are, so they are fetched first (once, and kept; see
//! [`series_details`]).

use std::sync::Mutex;

use anyhow::{anyhow, Result};
use rusqlite::OptionalExtension;

use crate::catalog::credentials::Kind;
use crate::catalog::reads::{marks_dto, store_query};
use crate::catalog::series_details::{self, SeriesInfoKept};
use crate::catalog::series_store::{self, EpisodeRow, SeriesRow};
use crate::catalog::vod_store::{self, ItemKind};
use crate::catalog::{db, index, series_sync, status};
use crate::ipc::types::{
    EpisodeDto, MarksDto, PlayingEpisodeDto, SeriesCategoryDto, SeriesDto, SeriesPageDto,
    SeriesQueryDto, ShowsSummaryDto,
};
use crate::source::Source;

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub(crate) fn series_dto(r: SeriesRow) -> SeriesDto {
    SeriesDto {
        series_id: r.series_id,
        name: r.name,
        clean_name: r.clean_name,
        year: r.year,
        rating: r.rating,
        poster_url: r.poster_url,
        added: r.added,
        category_id: r.category_id,
        season_count: r.season_count,
        episode_count: r.episode_count,
        favourite: r.favourite,
        watched: r.watched,
    }
}

pub(crate) fn episode_dto(r: EpisodeRow) -> EpisodeDto {
    EpisodeDto {
        episode_id: r.episode_id,
        series_id: r.series_id,
        season: r.season,
        episode: r.episode,
        episode_end: r.episode_end,
        title: r.title,
        plot: r.plot,
        duration_secs: r.duration_secs,
        still_url: r.still_url,
        air_date: r.air_date,
        watched: r.watched,
        position: r.position,
        duration: r.duration,
    }
}

/// Why the account has no shows, in words, or `None` when it has some.
pub fn shows_note(kind: Option<Kind>, total: u32) -> Option<String> {
    if total > 0 {
        return None;
    }
    Some(
        match kind {
            None => "No account yet. Set one in Settings \u{203a} Account.",
            Some(Kind::HdHomeRun) => "A tuner carries live channels only: it has no shows.",
            Some(Kind::Tvheadend) => "Tvheadend serves live channels: it has no shows.",
            Some(Kind::M3u) => {
                "This playlist has no shows. Entries whose name has a season and episode (S01E02) and whose address is a video file or contains /series/ appear here."
            }
            Some(Kind::Xtream) => "No shows yet. They arrive with the catalogue's next refresh.",
        }
        .to_owned(),
    )
}

fn account_kind() -> Option<Kind> {
    crate::catalog::credentials::load_account_stored()
        .ok()
        .flatten()
        .map(|a| a.kind())
}

/// How many shows there are, and whether the interface should offer them.
pub fn shows_summary() -> Result<ShowsSummaryDto> {
    let (counts, origin) = db::with_db(|c| {
        Ok((
            series_store::series_counts(c, now())?,
            series_sync::series_origin(c)?,
        ))
    })?;
    let note = shows_note(account_kind(), counts.total);
    Ok(ShowsSummaryDto {
        available: note.is_none(),
        note,
        total: counts.total,
        favourites: counts.favourites,
        recent: counts.recent,
        watched: counts.watched,
        origin,
    })
}

pub fn series_categories() -> Result<Vec<SeriesCategoryDto>> {
    db::with_db(|c| {
        Ok(series_store::series_categories(c)?
            .into_iter()
            .map(|r| SeriesCategoryDto {
                id: r.id,
                name: r.name,
                series_count: r.series_count.into(),
            })
            .collect())
    })
}

/// One page of a list, and how many shows the list has in all.
pub fn series(q: &SeriesQueryDto, offset: u32, limit: u32) -> Result<SeriesPageDto> {
    let query = store_query(
        q.list,
        q.category_id.as_deref(),
        q.sort,
        q.year_from,
        q.year_to,
    )?;
    let (total, rows) =
        db::with_db(|c| series_store::series_page(c, &query, offset, limit, now()))?;
    Ok(SeriesPageDto {
        total,
        items: rows.into_iter().map(series_dto).collect(),
    })
}

/// Specific shows, in the order asked; ones that are gone are left out.
pub fn series_by_ids(ids: &[i64]) -> Result<Vec<SeriesDto>> {
    db::with_db(|c| {
        Ok(series_store::series_by_ids(c, ids)?
            .into_iter()
            .map(series_dto)
            .collect())
    })
}

/// The catalogue's last full pass when the shows' index was last built
/// here, as the channels' and the movies' are (see `reads`).
static SERIES_INDEXED_FOR: Mutex<Option<Option<i64>>> = Mutex::new(None);

/// Fuzzy match over every show's name, best first. Blank finds nothing.
pub fn search_series(query: &str, limit: u32) -> Result<Vec<SeriesDto>> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let synced = status::last_synced();
    db::with_db(|conn| {
        let mut stamp = SERIES_INDEXED_FOR.lock().unwrap_or_else(|e| e.into_inner());
        if !index::series_built() || stamp.is_some_and(|s| s != synced) {
            index::rebuild_series(conn)?;
        }
        *stamp = Some(synced);
        drop(stamp);
        let ids = index::query_series(query, limit as usize);
        Ok(series_store::series_by_ids(conn, &ids)?
            .into_iter()
            .map(series_dto)
            .collect())
    })
}

/// Whether a live show has no live episodes here.
fn has_no_episodes(id: i64) -> Result<bool> {
    db::with_db(|c| {
        Ok(c.query_row(
            "SELECT NOT EXISTS (SELECT 1 FROM episodes
                                WHERE series_id = ?1 AND deleted_at IS NULL)",
            [id],
            |r| r.get(0),
        )?)
    })
}

fn mark_series(id: i64, favourite: Option<bool>, watched: Option<bool>) -> Result<MarksDto> {
    db::with_db(|c| {
        let m = series_store::set_series_marks(c, id, favourite, watched, now())?;
        Ok(marks_dto(id, m))
    })
}

/// Sets or clears a show's ★ and watched marks; `None` leaves one alone.
/// Watched reaches every episode, specials aside; an Xtream show whose
/// episodes are not here yet has them fetched first, since a show with no
/// episodes known cannot be watched. When they cannot be fetched, nothing
/// is changed and the error (scrubbed) says why.
pub async fn set_series_marks(
    id: i64,
    favourite: Option<bool>,
    watched: Option<bool>,
) -> Result<MarksDto> {
    let source = Source::load().ok();
    marks_with(source.as_ref(), id, favourite, watched, now()).await
}

/// [`set_series_marks`] with the account's source, if it loads.
async fn marks_with(
    source: Option<&Source>,
    id: i64,
    favourite: Option<bool>,
    watched: Option<bool>,
    now: i64,
) -> Result<MarksDto> {
    if let Some(source @ Source::Xtream(_)) = source {
        if watched == Some(true) && blocking(move || has_no_episodes(id)).await? {
            series_details::details_with(source, id, now).await?;
        }
    }
    blocking(move || mark_series(id, favourite, watched)).await
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| anyhow!("the write did not finish: {e}"))?
}

/// Marks an episode watched, or starts it over. A multi-episode entry takes
/// the episodes it covers with it, and the show's own mark follows.
pub fn set_episode_marks(id: i64, watched: bool) -> Result<MarksDto> {
    db::with_db(|c| {
        let m = vod_store::set_marks(c, ItemKind::Episode, id, None, Some(watched), now())?;
        Ok(marks_dto(id, m))
    })
}

/// Writes down how far into an episode the player is.
pub fn save_episode_progress(id: i64, position: f64, duration: Option<f64>) -> Result<MarksDto> {
    db::with_db(|c| {
        let m = vod_store::record_progress(c, ItemKind::Episode, id, position, duration, now())?;
        Ok(marks_dto(id, m))
    })
}

/// The episode playing and its show, for the player's title.
pub fn playing_episode(id: i64) -> Result<PlayingEpisodeDto> {
    let (series, episode) = db::with_db(|c| series_store::episode_by_id(c, id))?
        .ok_or_else(|| anyhow!("that episode is not in the catalogue any more"))?;
    Ok(PlayingEpisodeDto {
        series: series_dto(series),
        episode: episode_dto(episode),
    })
}

/// The poster the catalogue has for a show, as the provider gave it.
pub fn series_poster_url(id: i64) -> Result<Option<String>> {
    db::with_db(|c| series_store::series_poster_url(c, id))
}

/// The `n`th backdrop in the details already kept. Never fetches: an image
/// request must not cost the provider a call.
pub fn series_backdrop_url(id: i64, n: usize) -> Result<Option<String>> {
    let kept = db::with_db(|c| series_store::cached_series_details(c, id))?;
    Ok(kept
        .and_then(|(_, json)| serde_json::from_str::<SeriesInfoKept>(&json).ok())
        .and_then(|i| i.backdrops.into_iter().nth(n)))
}

/// An episode's still, as the provider gave it.
pub fn episode_still_url(id: i64) -> Result<Option<String>> {
    db::with_db(|c| series_store::episode_still_url(c, id))
}

/// A live show's name, for the episode titles that repeat it.
pub(crate) fn series_name(id: i64) -> Result<Option<String>> {
    db::with_db(|c| {
        Ok(c.query_row(
            "SELECT name FROM series WHERE series_id = ?1 AND deleted_at IS NULL",
            [id],
            |r| r.get(0),
        )
        .optional()?)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_note_says_why_a_tuner_has_none() {
        let n = shows_note(Some(Kind::HdHomeRun), 0).unwrap();
        assert_eq!(n, "A tuner carries live channels only: it has no shows.");
        assert_eq!(
            shows_note(Some(Kind::Tvheadend), 0).unwrap(),
            "Tvheadend serves live channels: it has no shows."
        );
        assert_eq!(
            shows_note(None, 0).unwrap(),
            "No account yet. Set one in Settings \u{203a} Account."
        );
        let p = shows_note(Some(Kind::M3u), 0).unwrap();
        assert!(p.contains("(S01E02)") && p.contains("/series/"), "{p}");
        assert_eq!(
            shows_note(Some(Kind::Xtream), 0).unwrap(),
            "No shows yet. They arrive with the catalogue's next refresh."
        );
    }

    #[test]
    fn shows_note_is_silent_when_there_are_shows() {
        for k in [
            None,
            Some(Kind::Xtream),
            Some(Kind::M3u),
            Some(Kind::HdHomeRun),
            Some(Kind::Tvheadend),
        ] {
            assert_eq!(shows_note(k, 1), None);
        }
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn marking_an_episode_reaches_its_show() {
        let _catalogue = db::movie_catalogue();
        db::install_test_db();
        db::with_db(|c| {
            c.execute_batch(
                "INSERT OR REPLACE INTO series (series_id, name, clean_name, added)
                   VALUES (9951, 'A Show', 'A Show', 0);
                 INSERT OR REPLACE INTO episodes (episode_id, series_id, season, episode)
                   VALUES (995101, 9951, 1, 1);",
            )?;
            Ok(())
        })
        .unwrap();
        let m = set_episode_marks(995101, true).unwrap();
        assert!(m.watched);
        assert!(series_by_ids(&[9951]).unwrap()[0].watched);
        let p = playing_episode(995101).unwrap();
        assert_eq!((p.series.series_id, p.episode.episode), (9951, 1));
        assert!(p.episode.watched);
        // Unmarking the show starts its episode over.
        let s = set_series_marks(9951, None, Some(false)).await.unwrap();
        assert!(!s.watched);
        assert!(!playing_episode(995101).unwrap().episode.watched);
        assert!(playing_episode(-1).is_err());
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn marking_an_unopened_show_watched_fetches_its_episodes_first() {
        use crate::test_http::{serve, Reply};
        use crate::xtream::{Credentials, XtreamClient};
        let _catalogue = db::movie_catalogue();
        db::install_test_db();
        let s = serve(|_, _| {
            Reply::ok(
                r#"{"info":{"name":"A Show"},"episodes":{"1":[
                    {"id":"995201","episode_num":"1"},{"id":"995202","episode_num":"2"}]}}"#,
            )
        });
        let creds = Credentials {
            host: s.base.clone(),
            username: "someone".into(),
            password: "pw".into(),
        };
        db::with_db(|c| {
            c.execute(
                "INSERT OR REPLACE INTO series (series_id, name, clean_name, added)
                 VALUES (9952, 'A Show', 'A Show', 0)",
                [],
            )?;
            c.execute(
                "INSERT INTO meta (key, value) VALUES ('series_origin', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [crate::catalog::origin::of_xtream(&creds)],
            )?;
            Ok(())
        })
        .unwrap();
        let src = Source::Xtream(XtreamClient::new(creds).unwrap());
        let m = marks_with(Some(&src), 9952, None, Some(true), 1_000_000)
            .await
            .unwrap();
        assert_eq!(s.requests().len(), 1);
        assert!(m.watched);
        let show = &series_by_ids(&[9952]).unwrap()[0];
        assert!(show.watched);
        assert_eq!(show.episode_count, Some(2));
        assert!(playing_episode(995202).unwrap().episode.watched);
        // With its episodes here, marking it again asks nothing.
        marks_with(Some(&src), 9952, None, Some(true), 1_000_000)
            .await
            .unwrap();
        assert_eq!(s.requests().len(), 1);
    }
}
