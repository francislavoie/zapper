//! A show's details and episodes, fetched as it is opened.
//!
//! An Xtream server's series list carries a name, a poster and a rating; the
//! plot, the cast, the seasons and every episode are one `get_series_info`
//! call per show. So a show is asked about when someone opens it, as a
//! movie is ([`crate::catalog::vod_details`]), and the answer is kept for a
//! week, or until the server says the show changed (its `last_modified`,
//! the row's `added`, is past the fetch). A provider that cannot be reached
//! shows what was kept, and is not asked about that show again for
//! [`FAILED_RETRY_SECS`].
//!
//! The episodes land only for a show that is still in the catalogue, and
//! only while the catalogue is still the account's that was asked: an
//! account switched, or the show dropped, while the answer was on its way
//! must not leave another catalogue's episodes behind.
//!
//! A playlist lists every episode itself, so its shows answer from what is
//! here and nothing is asked.
//!
//! Every error here is scrubbed before it leaves: `reqwest` puts the URL in
//! its message, and an Xtream URL carries the account's username and
//! password.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use anyhow::{anyhow, Result};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::catalog::series_reads::{episode_dto, series_dto, series_name};
use crate::catalog::sync_engine::scrub;
use crate::catalog::vod_details::{split, DETAILS_FRESH_SECS, FAILED_RETRY_SECS};
use crate::catalog::{db, origin, series_store, series_sync};
use crate::api::types::{SeasonDto, SeriesDetailsDto};
use crate::source::Source;
use crate::xtream::{SeriesInfo, XtreamClient};

/// Shows whose last fetch failed: when, and the error it gave (scrubbed).
fn failures() -> std::sync::MutexGuard<'static, HashMap<i64, (i64, String)>> {
    static FAILED: OnceLock<Mutex<HashMap<i64, (i64, String)>>> = OnceLock::new();
    FAILED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// What is kept of a show's `get_series_info` besides its episodes, as the
/// JSON in `series_details.info`. Every field has a default so that a copy
/// written by another build still reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SeriesInfoKept {
    pub plot: Option<String>,
    pub genres: Vec<String>,
    pub director: Option<String>,
    pub cast: Vec<String>,
    pub release_date: Option<String>,
    /// The provider's addresses; the interface is never given them as they
    /// are (see `series_reads::series_backdrop_url`).
    pub backdrops: Vec<String>,
    pub tmdb_id: Option<i64>,
    /// The seasons as the details name them; blank names are left blank.
    pub seasons: Vec<(i32, String)>,
    /// The show's `added` when this was fetched: the provider's own clock,
    /// so a change is seen by comparing it with the row's, never with ours.
    /// `None` in a copy kept before it was written down.
    pub modified: Option<i64>,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn info_from_xtream(v: &SeriesInfo) -> SeriesInfoKept {
    let i = &v.info;
    SeriesInfoKept {
        plot: i.plot.clone(),
        genres: split(i.genre.as_deref(), &[',', '/']),
        director: i.director.clone(),
        cast: split(i.cast.as_deref(), &[',']),
        release_date: i.release_date.clone(),
        backdrops: i.backdrop_path.clone(),
        tmdb_id: i.tmdb,
        seasons: v
            .seasons
            .iter()
            .map(|s| (s.season_number, s.name.clone().unwrap_or_default()))
            .collect(),
        modified: None,
    }
}

fn gone() -> anyhow::Error {
    anyhow!("that show is not in the catalogue any more")
}

/// What is kept of a show's details and when it was fetched, if any.
type Kept = Option<(i64, SeriesInfoKept)>;

/// The kept copy and when it was fetched, if there is one that reads.
fn kept_in(c: &Connection, id: i64) -> Result<Kept> {
    Ok(series_store::cached_series_details(c, id)?
        .and_then(|(at, json)| serde_json::from_str(&json).ok().map(|info| (at, info))))
}

/// A live show's `added` (when the provider last changed it), with the
/// kept copy; `None` for a show that is gone.
fn state(id: i64) -> Result<Option<(i64, Kept)>> {
    db::with_db(|c| {
        let added: Option<i64> = c
            .query_row(
                "SELECT added FROM series WHERE series_id = ?1 AND deleted_at IS NULL",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        match added {
            Some(a) => Ok(Some((a, kept_in(c, id)?))),
            None => Ok(None),
        }
    })
}

/// Why an answer that arrived was not landed.
enum Landing {
    Landed,
    Gone,
    Renamed,
    Switched,
}

/// What a fetch was asked under: the show's name (which the episode titles
/// were cleaned with) and the catalogue its shows were landed from.
struct Asked<'a> {
    name: &'a str,
    from: &'a str,
}

/// Lands a show's details in one transaction, after checking in it that the
/// show is still live under the name it was asked about, and the shows are
/// still `from`'s. The transaction takes the write lock first (IMMEDIATE),
/// so a sync committing between the checks and the write waits rather than
/// failing the landing.
fn land_if_current(
    c: &Connection,
    id: i64,
    asked: &Asked,
    at: i64,
    json: &str,
    episodes: &[series_sync::NewEpisode],
) -> Result<Landing> {
    let tx = rusqlite::Transaction::new_unchecked(c, rusqlite::TransactionBehavior::Immediate)?;
    let name: Option<String> = tx
        .query_row(
            "SELECT name FROM series WHERE series_id = ?1 AND deleted_at IS NULL",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    match name {
        None => return Ok(Landing::Gone),
        Some(n) if n != asked.name => return Ok(Landing::Renamed),
        Some(_) => {}
    }
    if series_sync::series_landed_from(&tx)?.as_deref() != Some(asked.from) {
        return Ok(Landing::Switched);
    }
    series_sync::land_series_details_in_tx(&tx, id, at, json, episodes)?;
    tx.commit()?;
    Ok(Landing::Landed)
}

/// Asks the server and lands the answer. `Ok(Err(_))` is a refusal to land
/// (the show went, or the account changed), said as it is and not
/// remembered; `Err` is the provider's failure (or the landing's own,
/// such as an emptied episode list), remembered.
async fn fetch(
    client: &XtreamClient,
    id: i64,
    from: &str,
    now: i64,
    added: i64,
) -> Result<std::result::Result<SeriesInfoKept, anyhow::Error>> {
    // The name as the list gave it, which episode titles repeat. Read
    // before asking: a show dropped or renamed meanwhile is the landing's
    // to refuse.
    let Some(name) = series_name(id)? else {
        return Ok(Err(gone()));
    };
    let v = client.series_info(id).await?;
    let mut info = info_from_xtream(&v);
    info.modified = Some(added);
    let json = serde_json::to_string(&info)?;
    let episodes = series_sync::episodes_from_info(id, &name, &v);
    let asked = Asked { name: &name, from };
    Ok(
        match db::with_db(|c| land_if_current(c, id, &asked, now, &json, &episodes))? {
            Landing::Landed => Ok(info),
            Landing::Gone => Err(gone()),
            Landing::Renamed => Err(anyhow!(
                "the show changed while it was being fetched; open it again"
            )),
            Landing::Switched => Err(anyhow!(
                "the catalogue changed while the show was being fetched; open it again"
            )),
        },
    )
}

/// The details as of `now`: the kept copy while it is fresh, else the
/// provider's answer (kept, with the episodes landed), else, when the
/// provider fails, the stale copy.
async fn info_at(source: &Source, id: i64, now: i64) -> Result<Kept> {
    let (added, kept) = state(id)?.ok_or_else(gone)?;
    // Fresh for a week, while the show's `added` is the one it was fetched
    // under: the provider's clock against the provider's, so a server whose
    // clock runs ahead of ours is not asked again on every open. A copy
    // kept before `modified` was written down is asked about once more.
    let fresh = kept
        .as_ref()
        .is_some_and(|(at, k)| now - at < DETAILS_FRESH_SECS && k.modified == Some(added));
    if fresh {
        return Ok(kept);
    }
    let Source::Xtream(client) = source else {
        return Ok(kept);
    };
    // Shows landed from another account (switched, and its first refresh
    // not landed yet): this server's answer is not about them.
    let from = origin::of_xtream(client.credentials());
    if db::with_db(series_sync::series_landed_from)?.as_deref() != Some(from.as_str()) {
        return match kept {
            Some(k) => Ok(Some(k)),
            None => Err(anyhow!(
                "The shows are being refreshed for this account; open it again shortly."
            )),
        };
    }
    let failed = |why: String| match kept.clone() {
        Some(k) => Ok(Some(k)),
        None => Err(anyhow!("the show's episodes could not be fetched: {why}")),
    };
    // Failed a moment ago: the same answer, without asking again.
    let recent = failures()
        .get(&id)
        .filter(|(at, _)| (0..FAILED_RETRY_SECS).contains(&(now - at)))
        .map(|(_, why)| why.clone());
    if let Some(why) = recent {
        return failed(why);
    }
    match fetch(client, id, &from, now, added).await {
        Ok(Ok(info)) => {
            failures().remove(&id);
            Ok(Some((now, info)))
        }
        Ok(Err(refused)) => Err(anyhow!(scrub(&format!("{refused:#}")))),
        Err(e) => {
            let why = scrub(&format!("{e:#}"));
            tracing::warn!("show details for {id} could not be fetched: {why}");
            failures().insert(id, (now, why.clone()));
            failed(why)
        }
    }
}

/// The year a date starts with (`2019-03-01`, `2019`).
fn year_of(date: &str) -> Option<i32> {
    let y = date.trim().get(..4)?;
    if !y.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    y.parse().ok().filter(|y| (1900..=2100).contains(y))
}

/// The answer, from what is here once the details are settled.
fn dto(id: i64, info: Kept) -> Result<SeriesDetailsDto> {
    let (show, eps) = db::with_db(|c| {
        let show = series_store::series_by_ids(c, &[id])?.pop();
        Ok((show, series_store::series_episodes(c, id)?))
    })?;
    let show = show.ok_or_else(gone)?;
    let (fetched_at, i) = match info {
        Some((at, i)) => (Some(at), i),
        None => (None, SeriesInfoKept::default()),
    };
    let years = || {
        eps.iter()
            .filter_map(|e| e.air_date.as_deref().and_then(year_of))
    };
    let seasons = series_store::seasons_of(&eps, &i.seasons)
        .into_iter()
        .map(|s| SeasonDto {
            number: s.number,
            name: s.name,
            episode_count: s.episode_count,
            watched_count: s.watched_count,
        })
        .collect();
    Ok(SeriesDetailsDto {
        first_year: years().min().or(show.year),
        last_year: years().max(),
        next_up: series_store::next_up(&eps),
        series: series_dto(show),
        plot: i.plot,
        genres: i.genres,
        director: i.director,
        cast: i.cast,
        release_date: i.release_date,
        backdrop_urls: i.backdrops,
        tmdb_id: i.tmdb_id,
        seasons,
        episodes: eps.into_iter().map(episode_dto).collect(),
        fetched_at,
    })
}

/// [`series_details`] from `source`, as of `now`.
pub(crate) async fn details_with(source: &Source, id: i64, now: i64) -> Result<SeriesDetailsDto> {
    let info = info_at(source, id, now).await?;
    dto(id, info)
}

/// What opening a show shows: its details, seasons and episodes, and which
/// one is next up. The one read of a show that may ask the provider, and
/// only when what is kept is a week old or older than the show.
pub async fn series_details(id: i64) -> Result<SeriesDetailsDto> {
    match Source::load() {
        Ok(source) => details_with(&source, id, now()).await,
        // An account that cannot be loaded answers from what is here, and
        // says why only when there is nothing.
        Err(e) => {
            let (_, kept) = state(id)?.ok_or_else(gone)?;
            let d = dto(id, kept)?;
            if d.fetched_at.is_none() && d.episodes.is_empty() {
                return Err(anyhow!(scrub(&format!("{e:#}"))));
            }
            Ok(d)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::vod_store::{self, ItemKind};
    use crate::test_http::{serve, Reply};
    use crate::xtream::Credentials;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    const SECRET: &str = "hunter2-secret";

    const INFO: &str = r#"{
        "info":{"name":"A Show","plot":"A plot","genre":"Drama, Mystery / Crime",
                "cast":"Ann Example, Bo Sample","backdrop_path":["http://img/b.jpg"]},
        "seasons":[{"season_number":1,"name":"The First"},{"season_number":2,"name":""}],
        "episodes":{
          "1":[{"id":"1","episode_num":"1","title":"A Show - S01E01 - Night Shift",
                "container_extension":"mkv","info":{"releasedate":"2019-03-01"}},
               {"id":"2","episode_num":"2","info":{"releasedate":"2019-03-08"}}],
          "2":[{"id":"3","episode_num":"1","info":{"releasedate":"2021-01-01"}}]}}"#;

    /// The info above, with its episode ids made the show's own (tests
    /// share one catalogue).
    fn info_for(id: i64) -> String {
        INFO.replace(r#""id":"1""#, &format!(r#""id":"{}""#, id * 100 + 1))
            .replace(r#""id":"2""#, &format!(r#""id":"{}""#, id * 100 + 2))
            .replace(r#""id":"3""#, &format!(r#""id":"{}""#, id * 100 + 3))
    }

    fn creds(base: &str) -> Credentials {
        Credentials {
            host: base.into(),
            username: "someone".into(),
            password: SECRET.into(),
        }
    }

    /// A show of the account at `base`, with nothing fetched about it yet.
    fn seed(id: i64, base: &str) {
        db::install_test_db();
        db::with_db(|c| {
            c.execute(
                "INSERT OR REPLACE INTO series (series_id, category_id, name, clean_name, year, added)
                 VALUES (?1, 'c', 'A Show', 'A Show', 2018, 0)",
                [id],
            )?;
            c.execute("DELETE FROM series_details WHERE series_id = ?1", [id])?;
            c.execute(
                "INSERT INTO meta (key, value) VALUES ('series_origin', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [origin::of_xtream(&creds(base))],
            )?;
            Ok(())
        })
        .unwrap();
    }

    fn source(base: &str) -> Source {
        Source::Xtream(XtreamClient::new(creds(base)).unwrap())
    }

    /// A fake server answering every request with `reply(n)`, `n` counting
    /// from 0, and the count.
    fn server(
        reply: impl Fn(usize) -> Reply + Send + Sync + 'static,
    ) -> (crate::test_http::Server, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let n = calls.clone();
        let s = serve(move |req, _| {
            assert_eq!(req.query("action"), Some("get_series_info"));
            reply(n.fetch_add(1, Ordering::SeqCst))
        });
        (s, calls)
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn details_are_asked_once_and_land_the_episodes() {
        let _catalogue = db::movie_catalogue();
        let (s, calls) = server(|_| Reply::ok(info_for(9901)));
        seed(9901, &s.base);
        let src = source(&s.base);
        let t = 1_000_000;
        let a = details_with(&src, 9901, t).await.unwrap();
        let b = details_with(&src, 9901, t + 60).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(a, b);
        assert_eq!(a.fetched_at, Some(t));
        assert_eq!(a.plot.as_deref(), Some("A plot"));
        assert_eq!(a.genres, ["Drama", "Mystery", "Crime"]);
        assert_eq!(a.cast, ["Ann Example", "Bo Sample"]);
        assert_eq!(a.backdrop_urls, ["http://img/b.jpg"]);
        let ids: Vec<i64> = a.episodes.iter().map(|e| e.episode_id).collect();
        assert_eq!(ids, [990101, 990102, 990103]);
        assert_eq!(a.episodes[0].title.as_deref(), Some("Night Shift"));
        // The counts the list shows come with the episodes.
        assert_eq!(
            (a.series.season_count, a.series.episode_count),
            (Some(2), Some(3))
        );
        assert_eq!((a.first_year, a.last_year), (Some(2019), Some(2021)));
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_show_changed_since_its_details_were_kept_is_asked_again() {
        let _catalogue = db::movie_catalogue();
        let (s, calls) = server(|_| Reply::ok(info_for(9902)));
        seed(9902, &s.base);
        let src = source(&s.base);
        let t = 1_000_000;
        details_with(&src, 9902, t).await.unwrap();
        db::with_db(|c| {
            c.execute(
                "UPDATE series SET added = ?1 WHERE series_id = 9902",
                [t + 10],
            )?;
            Ok(())
        })
        .unwrap();
        details_with(&src, 9902, t + 20).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn details_are_kept_for_a_week() {
        let _catalogue = db::movie_catalogue();
        let (s, calls) = server(|_| Reply::ok(info_for(9903)));
        seed(9903, &s.base);
        let src = source(&s.base);
        let t = 1_000_000;
        details_with(&src, 9903, t).await.unwrap();
        details_with(&src, 9903, t + 6 * 86_400).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let d = details_with(&src, 9903, t + DETAILS_FRESH_SECS)
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(d.fetched_at, Some(t + DETAILS_FRESH_SECS));
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_failed_refetch_shows_what_was_kept() {
        let _catalogue = db::movie_catalogue();
        let (s, calls) = server(|n| match n {
            0 => Reply::ok(info_for(9904)),
            _ => Reply::new(500, "boom"),
        });
        seed(9904, &s.base);
        let src = source(&s.base);
        let t = 1_000_000;
        details_with(&src, 9904, t).await.unwrap();
        let d = details_with(&src, 9904, t + 8 * 86_400).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(d.plot.as_deref(), Some("A plot"));
        assert_eq!(d.fetched_at, Some(t));
        assert_eq!(d.episodes.len(), 3);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_playlist_show_answers_from_its_episodes() {
        let _catalogue = db::movie_catalogue();
        db::install_test_db();
        db::with_db(|c| {
            c.execute_batch(
                "INSERT OR REPLACE INTO series (series_id, name, clean_name, added)
                   VALUES (9905, 'A Show', 'A Show', 0);
                 INSERT OR REPLACE INTO episodes
                   (episode_id, series_id, season, episode, stream_url, air_date)
                 VALUES (990501, 9905, 1, 1, 'http://example.test/a/s01e01.mkv', '2020-05-01'),
                        (990502, 9905, 1, 2, 'http://example.test/a/s01e02.mkv', NULL);",
            )?;
            Ok(())
        })
        .unwrap();
        let src =
            Source::M3u(crate::source::M3uSource::new(crate::m3u::M3uAccount::default()).unwrap());
        let d = details_with(&src, 9905, 1_000_000).await.unwrap();
        assert_eq!(d.fetched_at, None);
        assert!(d.plot.is_none());
        assert_eq!(d.episodes.len(), 2);
        assert_eq!(d.seasons.len(), 1);
        assert_eq!(d.seasons[0].name, "Season 1");
        assert_eq!(d.next_up, Some(990501));
        assert_eq!((d.first_year, d.last_year), (Some(2020), Some(2020)));
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn the_answer_carries_next_up_and_season_counts() {
        let _catalogue = db::movie_catalogue();
        let (s, calls) = server(|_| Reply::ok(info_for(9906)));
        seed(9906, &s.base);
        let src = source(&s.base);
        let t = 1_000_000;
        let before = details_with(&src, 9906, t).await.unwrap();
        assert_eq!(before.next_up, Some(990601));
        db::with_db(|c| {
            vod_store::set_marks(c, ItemKind::Episode, 990601, None, Some(true), t)?;
            Ok(())
        })
        .unwrap();
        let d = details_with(&src, 9906, t + 60).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(d.next_up, Some(990602));
        let seasons: Vec<_> = d
            .seasons
            .iter()
            .map(|s| (s.number, s.name.as_str(), s.episode_count, s.watched_count))
            .collect();
        // Named as the details name them, else by number.
        assert_eq!(seasons, [(1, "The First", 2, 1), (2, "Season 2", 1, 0)]);
        assert!(d.episodes[0].watched);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_failure_is_not_asked_again_for_a_minute() {
        let _catalogue = db::movie_catalogue();
        let (s, calls) = server(|n| match n {
            0 | 1 => Reply::new(500, "boom"),
            _ => Reply::ok(info_for(9907)),
        });
        seed(9907, &s.base);
        let src = source(&s.base);
        let t = 2_000_000;
        let first = format!("{:#}", details_with(&src, 9907, t).await.unwrap_err());
        // Reopened while the provider is down: the same answer, at once.
        let again = format!("{:#}", details_with(&src, 9907, t + 30).await.unwrap_err());
        assert_eq!(again, first);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // A minute on, it is asked again; and once it answers, kept.
        details_with(&src, 9907, t + FAILED_RETRY_SECS)
            .await
            .unwrap_err();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let d = details_with(&src, 9907, t + 2 * FAILED_RETRY_SECS)
            .await
            .unwrap();
        assert_eq!(d.episodes.len(), 3);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn errors_never_carry_the_account() {
        let _catalogue = db::movie_catalogue();
        let (s, _) = server(|_| Reply::new(500, format!("no such user someone:{SECRET}")));
        seed(9908, &s.base);
        let src = source(&s.base);
        let err = format!(
            "{:#}",
            details_with(&src, 9908, 1_000_000).await.unwrap_err()
        );
        assert!(!err.contains(SECRET), "{err}");
        assert!(err.contains("could not be fetched"), "{err}");
        // The remembered failure is the same scrubbed words.
        let again = format!(
            "{:#}",
            details_with(&src, 9908, 1_000_010).await.unwrap_err()
        );
        assert!(!again.contains(SECRET), "{again}");
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_show_dropped_during_the_fetch_lands_nothing() {
        let _catalogue = db::movie_catalogue();
        let (s, _) = server(|_| {
            // The catalogue's refresh drops the show while it is asked about.
            db::with_db(|c| {
                c.execute(
                    "UPDATE series SET deleted_at = 1 WHERE series_id = 9909",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
            Reply::ok(info_for(9909))
        });
        seed(9909, &s.base);
        let src = source(&s.base);
        let err = format!(
            "{:#}",
            details_with(&src, 9909, 1_000_000).await.unwrap_err()
        );
        assert!(err.contains("not in the catalogue"), "{err}");
        let landed: i64 = db::with_db(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM episodes WHERE series_id = 9909",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
        assert_eq!(landed, 0);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn an_account_switched_during_the_fetch_lands_nothing() {
        let _catalogue = db::movie_catalogue();
        let (s, _) = server(|_| {
            db::with_db(|c| {
                c.execute(
                    "UPDATE meta SET value = 'xtream:another' WHERE key = 'series_origin'",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
            Reply::ok(info_for(9910))
        });
        seed(9910, &s.base);
        let src = source(&s.base);
        let err = format!(
            "{:#}",
            details_with(&src, 9910, 1_000_000).await.unwrap_err()
        );
        assert!(err.contains("catalogue changed"), "{err}");
        let (episodes, kept): (i64, i64) = db::with_db(|c| {
            Ok(c.query_row(
                "SELECT (SELECT COUNT(*) FROM episodes WHERE series_id = 9910),
                        (SELECT COUNT(*) FROM series_details WHERE series_id = 9910)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .unwrap();
        assert_eq!((episodes, kept), (0, 0));
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn another_accounts_shows_are_not_asked_about() {
        let _catalogue = db::movie_catalogue();
        let (s, calls) = server(|_| Reply::ok(info_for(9911)));
        seed(9911, "http://another.example.test");
        let err = details_with(&source(&s.base), 9911, 1_000_000)
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            err,
            "The shows are being refreshed for this account; open it again shortly."
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_show_dated_ahead_of_our_clock_is_asked_once() {
        let _catalogue = db::movie_catalogue();
        let (s, calls) = server(|_| Reply::ok(info_for(9912)));
        seed(9912, &s.base);
        let t = 1_000_000;
        // The provider's clock runs an hour ahead of ours.
        db::with_db(|c| {
            c.execute(
                "UPDATE series SET added = ?1 WHERE series_id = 9912",
                [t + 3600],
            )?;
            Ok(())
        })
        .unwrap();
        let src = source(&s.base);
        details_with(&src, 9912, t).await.unwrap();
        details_with(&src, 9912, t + 60).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_copy_kept_without_the_shows_date_is_asked_again_once() {
        let _catalogue = db::movie_catalogue();
        let (s, calls) = server(|_| Reply::ok(info_for(9913)));
        seed(9913, &s.base);
        let t = 1_000_000;
        db::with_db(|c| {
            c.execute(
                "INSERT INTO series_details (series_id, fetched_at, info)
                 VALUES (9913, ?1, '{\"plot\":\"Old\"}')",
                [t],
            )?;
            Ok(())
        })
        .unwrap();
        let src = source(&s.base);
        let d = details_with(&src, 9913, t + 60).await.unwrap();
        assert_eq!(d.plot.as_deref(), Some("A plot"));
        details_with(&src, 9913, t + 120).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_show_renamed_during_the_fetch_lands_nothing() {
        let _catalogue = db::movie_catalogue();
        let (s, _) = server(|_| {
            db::with_db(|c| {
                c.execute(
                    "UPDATE series SET name = 'Another Show' WHERE series_id = 9914",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
            Reply::ok(info_for(9914))
        });
        seed(9914, &s.base);
        let src = source(&s.base);
        let err = format!(
            "{:#}",
            details_with(&src, 9914, 1_000_000).await.unwrap_err()
        );
        assert!(err.contains("show changed"), "{err}");
        let kept: i64 = db::with_db(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM series_details WHERE series_id = 9914",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
        assert_eq!(kept, 0);
    }

    #[test]
    fn years_are_read_from_the_start_of_a_date() {
        assert_eq!(year_of("2019-03-01"), Some(2019));
        assert_eq!(year_of(" 2021"), Some(2021));
        assert_eq!(year_of("03/01/2019"), None);
        assert_eq!(year_of("0000-00-00"), None);
    }
}
