//! A movie's details, fetched as it is opened.
//!
//! The list call carries a title, a poster and a rating; the plot, cast and
//! backdrops are one `get_vod_info` call per title. Asking for all of them
//! would be a hundred and fifty thousand requests, so a title is asked about
//! when someone opens it, and the answer is kept for a week. A provider that
//! cannot be reached shows what was kept rather than nothing, and is not
//! asked about that title again for [`FAILED_RETRY_SECS`]: reopening it
//! while the provider is down answers at once rather than after another
//! wait for the provider.
//!
//! Every error here is scrubbed before it leaves: `reqwest` puts the URL in
//! its message, and an Xtream URL carries the account's username and
//! password.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::catalog::sync_engine::scrub;
use crate::catalog::{db, reads, vod_store};
use crate::ipc::types::{MovieDetailsDto, MovieDto};
use crate::source::Source;
use crate::xtream::VodInfo;

/// How long fetched details are trusted before the next open asks again.
pub const DETAILS_FRESH_SECS: i64 = 7 * 86_400;

/// How long after a failed fetch a title's details are not asked for again.
pub const FAILED_RETRY_SECS: i64 = 60;

/// Titles whose last fetch failed: when, and the error it gave (scrubbed).
fn failures() -> std::sync::MutexGuard<'static, HashMap<i64, (i64, String)>> {
    static FAILED: OnceLock<Mutex<HashMap<i64, (i64, String)>>> = OnceLock::new();
    FAILED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// What is kept of a title's `get_vod_info`, as the JSON in
/// `movie_details.info`. Every field has a default so that a copy written by
/// another build still reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MovieInfo {
    pub plot: Option<String>,
    pub genres: Vec<String>,
    pub director: Option<String>,
    pub cast: Vec<String>,
    pub release_date: Option<String>,
    /// The provider's addresses; the interface is never given them as they
    /// are (see `reads::movie_backdrop_url`).
    pub backdrops: Vec<String>,
    pub tmdb_id: Option<i64>,
    /// Seconds.
    pub runtime: Option<i64>,
    pub video_codec: Option<String>,
    pub audio_codecs: Vec<String>,
    pub audio_languages: Vec<String>,
    pub subtitle_languages: Vec<String>,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A provider's list in one string (`"Drama, Crime"`), as a list; a show's
/// details split theirs the same way.
pub(crate) fn split(text: Option<&str>, separators: &[char]) -> Vec<String> {
    text.map(|t| {
        t.split(separators)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    })
    .unwrap_or_default()
}

pub fn info_from_xtream(v: &VodInfo) -> MovieInfo {
    let i = &v.info;
    let cast = split(i.cast.as_deref(), &[',']);
    let cast = if cast.is_empty() {
        split(i.actors.as_deref(), &[','])
    } else {
        cast
    };
    MovieInfo {
        plot: i.plot.clone().or_else(|| i.description.clone()),
        genres: split(i.genre.as_deref(), &[',', '/']),
        director: i.director.clone(),
        cast,
        release_date: i.releasedate.clone(),
        backdrops: i.backdrop_path.clone(),
        tmdb_id: i.tmdb_id,
        runtime: i.runtime_secs(),
        video_codec: i.video.as_ref().and_then(|t| t.codec_name.clone()),
        audio_codecs: i
            .audio
            .iter()
            .filter_map(|t| t.codec_name.clone())
            .collect(),
        audio_languages: i.audio.iter().filter_map(|t| t.language.clone()).collect(),
        // `get_vod_info` does not list subtitles.
        subtitle_languages: Vec::new(),
    }
}

/// The kept copy and when it was fetched, if there is one that reads.
fn cached(id: i64) -> Result<Option<(i64, MovieInfo)>> {
    Ok(db::with_db(|c| vod_store::cached_details(c, id))?
        .and_then(|(at, json)| serde_json::from_str(&json).ok().map(|info| (at, info))))
}

/// The details as of `now`: the kept copy while it is fresh, else the
/// provider's answer (kept), else, when the provider fails, the stale copy.
/// A playlist's entries carry no more than the list gave, so there is
/// nothing to ask for.
async fn info_at(source: &Source, id: i64, now: i64) -> Result<Option<(i64, MovieInfo)>> {
    let kept = cached(id)?;
    if kept
        .as_ref()
        .is_some_and(|(at, _)| now - at < DETAILS_FRESH_SECS)
    {
        return Ok(kept);
    }
    let Source::Xtream(client) = source else {
        return Ok(kept);
    };
    let failed = |why: String| match kept.clone() {
        Some(k) => Ok(Some(k)),
        None => Err(anyhow!("the details could not be fetched: {why}")),
    };
    // Failed a moment ago: the same answer, without asking again.
    let recent = failures()
        .get(&id)
        .filter(|(at, _)| (0..FAILED_RETRY_SECS).contains(&(now - at)))
        .map(|(_, why)| why.clone());
    if let Some(why) = recent {
        return failed(why);
    }
    match client.vod_info(id).await {
        Ok(v) => {
            failures().remove(&id);
            let info = info_from_xtream(&v);
            let json = serde_json::to_string(&info)?;
            db::with_db(|c| vod_store::store_details(c, id, now, &json, info.runtime))?;
            Ok(Some((now, info)))
        }
        Err(e) => {
            let why = scrub(&format!("{e:#}"));
            tracing::warn!("movie details for {id} could not be fetched: {why}");
            failures().insert(id, (now, why.clone()));
            failed(why)
        }
    }
}

/// An account that cannot be loaded answers from the kept copy alone.
async fn info_stamped(id: i64) -> Result<Option<(i64, MovieInfo)>> {
    match Source::load() {
        Ok(source) => info_at(&source, id, now()).await,
        Err(e) => match cached(id)? {
            Some(kept) => Ok(Some(kept)),
            None => Err(anyhow!(scrub(&format!("{e:#}")))),
        },
    }
}

/// A title's details, from the kept copy or the provider; see [`info_at`].
/// `None` for a playlist.
pub async fn movie_info(id: i64) -> Result<Option<MovieInfo>> {
    Ok(info_stamped(id).await?.map(|(_, i)| i))
}

fn row(id: i64) -> Result<MovieDto> {
    reads::movies_by_ids(&[id])?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("that movie is not in the catalogue any more"))
}

fn dto(movie: MovieDto, info: Option<(i64, MovieInfo)>) -> MovieDetailsDto {
    let (fetched_at, i) = match info {
        Some((at, i)) => (Some(at), i),
        None => (None, MovieInfo::default()),
    };
    MovieDetailsDto {
        movie,
        plot: i.plot,
        genres: i.genres,
        director: i.director,
        cast: i.cast,
        release_date: i.release_date,
        backdrop_urls: i.backdrops,
        tmdb_id: i.tmdb_id,
        audio_languages: i.audio_languages,
        subtitle_languages: i.subtitle_languages,
        fetched_at,
    }
}

/// What the details pane shows for a title.
pub async fn movie_details(id: i64) -> Result<MovieDetailsDto> {
    // A title that is gone is said so before the provider is asked about it.
    row(id)?;
    let info = info_stamped(id).await?;
    // Read again: a fetch stores the runtime in the row.
    Ok(dto(row(id)?, info))
}

#[cfg(test)]
async fn details_with(source: &Source, id: i64, now: i64) -> Result<MovieDetailsDto> {
    row(id)?;
    let info = info_at(source, id, now).await?;
    Ok(dto(row(id)?, info))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_http::{serve, Reply};
    use crate::xtream::Credentials;
    use crate::xtream::XtreamClient;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    const SECRET: &str = "hunter2-secret";

    fn seed(id: i64) {
        db::install_test_db();
        db::with_db(|c| {
            c.execute(
                "INSERT INTO movies (movie_id, category_id, name, clean_name, added)
                 VALUES (?1, 'c', 'Title', 'Title', 0)",
                [id],
            )?;
            Ok(())
        })
        .unwrap();
    }

    fn source(base: &str) -> Source {
        Source::Xtream(
            XtreamClient::new(Credentials {
                host: base.into(),
                username: "someone".into(),
                password: SECRET.into(),
            })
            .unwrap(),
        )
    }

    fn vod(v: &str) -> VodInfo {
        serde_json::from_str(v).unwrap()
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn details_are_read_once_and_kept_for_a_week() {
        let _catalogue = db::movie_catalogue();
        seed(9601);
        let calls = Arc::new(AtomicUsize::new(0));
        let n = calls.clone();
        let s = serve(move |_, _| {
            n.fetch_add(1, Ordering::SeqCst);
            Reply::ok(r#"{"info":{"plot":"A plot","duration_secs":"5400"}}"#)
        });
        let src = source(&s.base);
        let t = 1_000_000;
        let a = details_with(&src, 9601, t).await.unwrap();
        assert_eq!(a.plot.as_deref(), Some("A plot"));
        assert_eq!(a.fetched_at, Some(t));
        // The runtime the details give is what the lists then show.
        assert_eq!(a.movie.duration_secs, Some(5400));
        details_with(&src, 9601, t + 86_400).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        details_with(&src, 9601, t + 8 * 86_400).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_failed_refetch_shows_what_was_kept() {
        let _catalogue = db::movie_catalogue();
        seed(9602);
        let calls = Arc::new(AtomicUsize::new(0));
        let n = calls.clone();
        let s = serve(move |_, _| {
            if n.fetch_add(1, Ordering::SeqCst) == 0 {
                Reply::ok(r#"{"info":{"plot":"Kept plot"}}"#)
            } else {
                Reply::new(500, "boom")
            }
        });
        let src = source(&s.base);
        details_with(&src, 9602, 1_000_000).await.unwrap();
        let d = details_with(&src, 9602, 1_000_000 + 8 * 86_400)
            .await
            .unwrap();
        assert_eq!(d.plot.as_deref(), Some("Kept plot"));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_failed_first_fetch_names_no_password() {
        let _catalogue = db::movie_catalogue();
        seed(9603);
        let s = serve(|_, _| Reply::new(500, "boom"));
        let src = source(&s.base);
        let err = details_with(&src, 9603, 1_000_000).await.unwrap_err();
        let text = format!("{err:#}");
        assert!(!text.contains(SECRET), "{text}");
        assert!(text.contains("could not be fetched"), "{text}");
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_failed_fetch_is_not_asked_again_for_a_minute() {
        let _catalogue = db::movie_catalogue();
        seed(9605);
        let calls = Arc::new(AtomicUsize::new(0));
        let n = calls.clone();
        let s = serve(move |_, _| {
            if n.fetch_add(1, Ordering::SeqCst) < 2 {
                Reply::new(500, "boom")
            } else {
                Reply::ok(r#"{"info":{"plot":"Back"}}"#)
            }
        });
        let src = source(&s.base);
        let t = 2_000_000;
        let first = format!("{:#}", details_with(&src, 9605, t).await.unwrap_err());
        // Reopened while the provider is down: the same answer, at once.
        let again = format!("{:#}", details_with(&src, 9605, t + 30).await.unwrap_err());
        assert_eq!(again, first);
        assert!(!again.contains(SECRET), "{again}");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // A minute on, it is asked again; and once it answers, kept.
        details_with(&src, 9605, t + FAILED_RETRY_SECS).await.unwrap_err();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let d = details_with(&src, 9605, t + 2 * FAILED_RETRY_SECS).await.unwrap();
        assert_eq!(d.plot.as_deref(), Some("Back"));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_playlist_movie_has_no_details_to_fetch() {
        let _catalogue = db::movie_catalogue();
        seed(9604);
        let src =
            Source::M3u(crate::source::M3uSource::new(crate::m3u::M3uAccount::default()).unwrap());
        let d = details_with(&src, 9604, 1_000_000).await.unwrap();
        assert_eq!(d.fetched_at, None);
        assert!(d.plot.is_none());
    }

    #[test]
    fn xtream_details_are_split_into_lists() {
        let i = info_from_xtream(&vod(
            r#"{"info":{"genre":"Drama, Thriller / Crime","cast":"","actors":"A, B ,"}}"#,
        ));
        assert_eq!(i.genres, ["Drama", "Thriller", "Crime"]);
        assert_eq!(i.cast, ["A", "B"]);
        let j = info_from_xtream(&vod(r#"{"info":{"cast":"C","actors":"A, B"}}"#));
        assert_eq!(j.cast, ["C"]);
    }
}
