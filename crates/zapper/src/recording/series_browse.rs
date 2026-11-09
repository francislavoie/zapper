//! The shows over HTTP, for the web interface, answered from the same reads ([`crate::catalog::series_reads`],
//! [`crate::catalog::series_details`]), behind the same sign-in as
//! [`super::browse`] (docs/recording.md, "API").
//!
//! As with movies ([`super::vod_browse`]), provider addresses do not leave
//! the service: a show's poster is `/api/poster/series/{id}`, its backdrops
//! `/api/backdrop/series/{id}/{n}` and an episode's still
//! `/api/still/episode/{id}`, which fetch the picture, so the page never
//! sees where it came from.

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::Deserialize;

use crate::catalog::sync_engine::scrub;
use crate::catalog::{db, series_details, series_reads, series_store};
use crate::ipc::types::{
    EpisodeDto, PlayingEpisodeDto, SeriesDetailsDto, SeriesDto, SeriesQueryDto,
};

use super::browse::{bad_request, proxy_image, read, Failed};
use super::vod_browse::{movie_query, ListParams, PAGE_MAX, POSTER_MAX_BYTES};

pub fn poster_path(id: i64) -> String {
    format!("/api/poster/series/{id}")
}

pub fn backdrop_path(id: i64, n: usize) -> String {
    format!("/api/backdrop/series/{id}/{n}")
}

pub fn still_path(id: i64) -> String {
    format!("/api/still/episode/{id}")
}

/// Swaps a show's provider poster for this service's address for it.
pub(super) fn local_series(s: &mut SeriesDto) {
    if s.poster_url.is_some() {
        s.poster_url = Some(poster_path(s.series_id));
    }
}

/// Swaps an episode's provider still for this service's address for it.
pub(super) fn local_episode(e: &mut EpisodeDto) {
    if e.still_url.is_some() {
        e.still_url = Some(still_path(e.episode_id));
    }
}

/// Swaps every picture of an opened show for this service's addresses.
fn local_series_details(d: &mut SeriesDetailsDto) {
    local_series(&mut d.series);
    let id = d.series.series_id;
    for (n, url) in d.backdrop_urls.iter_mut().enumerate() {
        *url = backdrop_path(id, n);
    }
    d.episodes.iter_mut().for_each(local_episode);
}

/// What a failure says, without any address in it. The reads scrub the
/// account from their errors, but a transport error names the URL it was
/// asking, and its host is the provider's: every word that is an address
/// (a URL, a path, a host and port) is said as "the provider" instead.
fn without_addresses(text: &str) -> String {
    text.split(' ')
        .map(|word| {
            let core = word
                .trim_start_matches(['(', '[', '<', '"', '\''])
                .trim_end_matches([')', ']', '>', '"', '\'', ',', '.', ';', ':']);
            let is_address = core.contains("://")
                || core.starts_with('/')
                || core.contains("player_api")
                || core.parse::<std::net::SocketAddr>().is_ok()
                || core.parse::<std::net::IpAddr>().is_ok();
            if core.is_empty() || !is_address {
                return word.to_owned();
            }
            let at = word.find(core).unwrap_or(0);
            format!("{}the provider{}", &word[..at], &word[at + core.len()..])
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// What a show's details say when they could not be fetched and nothing is
/// kept; `catalog::series_details` logs why when the fetch fails.
const FETCH_FAILED: &str = "the show's episodes could not be fetched";
/// What they say when the show went while it was asked about.
const GONE: &str = "that show is not in the catalogue any more";
/// The words `catalog::series_details` writes itself, which may reach the
/// page as they are: only ever the text before the first ": ", since what
/// follows can be the provider's own words or a transport error naming its
/// host (a URL, or a certificate's `not valid for name "host"`).
const SAID: [&str; 4] = [
    FETCH_FAILED,
    "The shows are being refreshed for this account; open it again shortly.",
    "the show changed while it was being fetched; open it again",
    "the catalogue changed while the show was being fetched; open it again",
];
/// Any other failure, as a movie's is said.
const NO_ANSWER: &str = "the provider did not answer for this show";

/// A failure of a read that may ask the provider, for the page: `502`, with
/// only words of our own, and `404` for a show that went meanwhile. Why in
/// full (scrubbed of the account) goes to the service's log alone.
fn provider_failed(id: i64, e: anyhow::Error) -> Failed {
    let why = scrub(&format!("{e:#}"));
    let lead = why.split(": ").next().unwrap_or_default();
    if lead != FETCH_FAILED {
        tracing::warn!("show {id}: {why}");
    }
    if lead == GONE {
        return not_found("show");
    }
    let said = SAID
        .iter()
        .copied()
        .find(|s| *s == lead)
        .unwrap_or(NO_ANSWER);
    // A second guard, in case a phrase above ever carries an address.
    Failed(StatusCode::BAD_GATEWAY, without_addresses(said))
}

/// A failure to set a show's marks: the provider's (`502`) only when
/// marking watched, which may ask it for the show's episodes; any other is
/// this service's own (`500`), as a movie's is.
fn marks_failed(id: i64, watched: Option<bool>, e: anyhow::Error) -> Failed {
    if watched == Some(true) {
        return provider_failed(id, e);
    }
    Failed(
        StatusCode::INTERNAL_SERVER_ERROR,
        without_addresses(&scrub(&format!("{e:#}"))),
    )
}

#[derive(Deserialize)]
struct MarksBody {
    favourite: Option<bool>,
    watched: Option<bool>,
}

#[derive(Deserialize)]
struct EpisodeMarksBody {
    watched: bool,
}

#[derive(Deserialize)]
struct ProgressBody {
    position: f64,
    duration: Option<f64>,
}

/// The query the words ask for: the movies' words, lists and sorts.
fn series_query(p: &ListParams) -> Result<SeriesQueryDto, Failed> {
    let q = movie_query(p)?;
    Ok(SeriesQueryDto {
        list: q.list,
        category_id: q.category_id,
        sort: q.sort,
        year_from: q.year_from,
        year_to: q.year_to,
        search: q.search,
    })
}

fn not_found(what: &str) -> Failed {
    Failed(
        StatusCode::NOT_FOUND,
        format!("that {what} is not in the catalogue"),
    )
}

/// A show that is in the catalogue, or a 404.
async fn present(id: i64) -> Result<(), Failed> {
    if read(move || series_reads::series_by_ids(&[id]))
        .await?
        .is_empty()
    {
        return Err(not_found("show"));
    }
    Ok(())
}

/// An episode that is in the catalogue, or a 404.
async fn episode_present(id: i64) -> Result<(), Failed> {
    if read(move || db::with_db(|c| series_store::episode_by_id(c, id)))
        .await?
        .is_none()
    {
        return Err(not_found("episode"));
    }
    Ok(())
}

async fn summary() -> Result<Response, Failed> {
    Ok(Json(read(series_reads::shows_summary).await?).into_response())
}

async fn categories() -> Result<Response, Failed> {
    Ok(Json(read(series_reads::series_categories).await?).into_response())
}

async fn series(Query(p): Query<ListParams>) -> Result<Response, Failed> {
    let q = series_query(&p)?;
    let limit = p.limit.min(PAGE_MAX);
    let mut page = read(move || series_reads::series(&q, p.offset, limit)).await?;
    page.items.iter_mut().for_each(local_series);
    Ok(Json(page).into_response())
}

async fn details(Path(id): Path<i64>) -> Result<Response, Failed> {
    present(id).await?;
    // The one read here that may ask the provider, the first time a show is
    // opened in a week. Every failure of it is a 502, as a movie's details'
    // are, its local ones included: the page has the one answer to give.
    let mut d = series_details::series_details(id)
        .await
        .map_err(|e| provider_failed(id, e))?;
    local_series_details(&mut d);
    Ok(Json(d).into_response())
}

async fn set_marks(Path(id): Path<i64>, Json(b): Json<MarksBody>) -> Result<Response, Failed> {
    present(id).await?;
    // Marking watched a show whose episodes are not here yet asks for them.
    let marks = series_reads::set_series_marks(id, b.favourite, b.watched)
        .await
        .map_err(|e| marks_failed(id, b.watched, e))?;
    Ok(Json(marks).into_response())
}

/// The episode and its show, in one read; a 404 for one not here.
async fn episode(Path(id): Path<i64>) -> Result<Response, Failed> {
    let (series, episode) = read(move || db::with_db(|c| series_store::episode_by_id(c, id)))
        .await?
        .ok_or_else(|| not_found("episode"))?;
    let mut playing = PlayingEpisodeDto {
        series: series_reads::series_dto(series),
        episode: series_reads::episode_dto(episode),
    };
    local_series(&mut playing.series);
    local_episode(&mut playing.episode);
    Ok(Json(playing).into_response())
}

async fn set_episode_marks(
    Path(id): Path<i64>,
    Json(b): Json<EpisodeMarksBody>,
) -> Result<Response, Failed> {
    episode_present(id).await?;
    let marks = read(move || series_reads::set_episode_marks(id, b.watched)).await?;
    Ok(Json(marks).into_response())
}

async fn save_progress(
    Path(id): Path<i64>,
    Json(b): Json<ProgressBody>,
) -> Result<Response, Failed> {
    if !b.position.is_finite() || b.position < 0.0 {
        return Err(bad_request("position: seconds from the start"));
    }
    if b.duration.is_some_and(|d| !d.is_finite() || d <= 0.0) {
        return Err(bad_request("duration: seconds, more than zero"));
    }
    episode_present(id).await?;
    let marks =
        read(move || series_reads::save_episode_progress(id, b.position, b.duration)).await?;
    Ok(Json(marks).into_response())
}

/// A show's poster, fetched from where the catalogue says it is; cached as
/// a logo is ([`super::browse`]).
async fn poster(Path(id): Path<i64>) -> Response {
    let url = read(move || series_reads::series_poster_url(id))
        .await
        .ok()
        .flatten();
    proxy_image(url, POSTER_MAX_BYTES).await
}

/// The `n`th backdrop of the details already kept for a show; never asks
/// the provider for the details themselves.
async fn backdrop(Path((id, n)): Path<(i64, usize)>) -> Response {
    let url = read(move || series_reads::series_backdrop_url(id, n))
        .await
        .ok()
        .flatten();
    proxy_image(url, POSTER_MAX_BYTES).await
}

async fn still(Path(id): Path<i64>) -> Response {
    let url = read(move || series_reads::episode_still_url(id))
        .await
        .ok()
        .flatten();
    proxy_image(url, POSTER_MAX_BYTES).await
}

/// The routes, for the app or a signed-in device (`recording::http` puts
/// them behind its sign-in check).
pub fn routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/api/vod/shows/summary", get(summary))
        .route("/api/vod/series/categories", get(categories))
        .route("/api/vod/series", get(series))
        .route("/api/vod/series/{id}", get(details))
        .route("/api/vod/series/{id}/marks", put(set_marks))
        .route("/api/vod/episodes/{id}", get(episode))
        .route(
            "/api/vod/episodes/{id}/marks",
            get(super::continue_browse::episode_marks).put(set_episode_marks),
        )
        .route("/api/vod/episodes/{id}/progress", put(save_progress))
        .route("/api/poster/series/{id}", get(poster))
        .route("/api/backdrop/series/{id}/{n}", get(backdrop))
        .route("/api/still/episode/{id}", get(still))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::types::{MovieListKind, MovieSort};
    use axum::http::Uri;

    fn params(query: &str) -> ListParams {
        Query::<ListParams>::try_from_uri(
            &format!("/api/vod/series?{query}").parse::<Uri>().unwrap(),
        )
        .ok()
        .unwrap()
        .0
    }

    #[test]
    fn a_category_list_of_shows_needs_its_category() {
        let p = params("list=category&limit=10");
        assert!(series_query(&p).is_err());
        let p = params("list=category&category=7&limit=10");
        assert_eq!(
            series_query(&p).ok().unwrap().category_id.as_deref(),
            Some("7")
        );
    }

    #[test]
    fn series_query_words_are_the_movies() {
        let p = params("list=watched&sort=az&from=2001&to=2009&offset=40&limit=20");
        assert_eq!(
            series_query(&p).ok().unwrap(),
            SeriesQueryDto {
                list: MovieListKind::Watched,
                category_id: None,
                sort: MovieSort::Az,
                year_from: Some(2001),
                year_to: Some(2009),
                search: None,
            }
        );
        assert_eq!((p.offset, p.limit), (40, 20));
        assert_eq!(
            series_query(&params("list=recent&limit=5"))
                .ok()
                .unwrap()
                .sort,
            MovieSort::Added
        );
        let found = series_query(&params("list=all&sort=year&q=EN%20-&limit=5"))
            .ok()
            .unwrap();
        assert_eq!(
            (found.list, found.sort, found.search.as_deref()),
            (MovieListKind::All, MovieSort::Year, Some("EN -"))
        );
    }

    fn episode(id: i64, still: Option<&str>) -> EpisodeDto {
        EpisodeDto {
            episode_id: id,
            series_id: 5,
            season: 1,
            episode: 1,
            episode_end: None,
            title: Some("Night Shift".into()),
            plot: None,
            duration_secs: None,
            still_url: still.map(str::to_owned),
            air_date: None,
            watched: false,
            position: None,
            duration: None,
        }
    }

    fn show(poster: Option<&str>) -> SeriesDto {
        SeriesDto {
            series_id: 5,
            name: "Lantern Bay".into(),
            clean_name: "Lantern Bay".into(),
            year: None,
            rating: None,
            poster_url: poster.map(str::to_owned),
            added: None,
            category_id: None,
            season_count: None,
            episode_count: None,
            favourite: false,
            watched: false,
        }
    }

    #[test]
    fn every_picture_of_a_show_is_this_services() {
        let mut d = SeriesDetailsDto {
            series: show(Some("http://p.example/u/p/poster.jpg")),
            plot: None,
            genres: vec![],
            director: None,
            cast: vec![],
            release_date: None,
            backdrop_urls: vec![
                "http://p.example/a.jpg".into(),
                "http://p.example/b.jpg".into(),
            ],
            tmdb_id: None,
            first_year: None,
            last_year: None,
            seasons: vec![],
            episodes: vec![
                episode(51, Some("http://p.example/s.jpg")),
                episode(52, None),
            ],
            next_up: None,
            fetched_at: None,
        };
        local_series_details(&mut d);
        assert_eq!(d.series.poster_url.as_deref(), Some("/api/poster/series/5"));
        assert_eq!(
            d.backdrop_urls,
            ["/api/backdrop/series/5/0", "/api/backdrop/series/5/1"]
        );
        assert_eq!(
            d.episodes[0].still_url.as_deref(),
            Some("/api/still/episode/51")
        );
        assert_eq!(d.episodes[1].still_url, None, "no still stays none");
        let mut bare = show(None);
        local_series(&mut bare);
        assert_eq!(bare.poster_url, None);
    }

    #[test]
    fn a_failure_is_told_without_the_providers_address() {
        let text = "the show's episodes could not be fetched: HTTP status server error \
                    (500 Internal Server Error) for url \
                    (http://192.0.2.7:8080/player_api.php?username=u&password=<redacted>&action=get_series_info): \
                    connect to 192.0.2.7:8080 at /player_api.php";
        let out = without_addresses(text);
        for leak in ["192.0.2.7", "8080", "player_api", "http", "username"] {
            assert!(!out.contains(leak), "{leak}: {out}");
        }
        assert!(out.starts_with(
            "the show's episodes could not be fetched: HTTP status server error (500"
        ));
        assert!(out.contains("for url (the provider):"), "{out}");
        let fixed = "The shows are being refreshed for this account; open it again shortly.";
        assert_eq!(without_addresses(fixed), fixed);
    }

    /// The page's whole answer for a failure, as its body is written.
    async fn body(f: Failed) -> (StatusCode, String) {
        let resp = f.into_response();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 16)
            .await
            .unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn a_failure_never_names_the_provider_even_bare_or_quoted() {
        let failures = [
            // A certificate for another name: bare and quoted hostnames.
            "the show's episodes could not be fetched: error sending request: invalid peer \
             certificate: certificate not valid for name \"iptv-host.example\"; certificate is \
             only valid for DnsName(\"cdn-host.example\")",
            "the show's episodes could not be fetched: tcp connect error to [::1]:8080 at \
             iptv-host.example",
            // Not a phrase of ours: none of it is said.
            "dns error looking up iptv-host.example: no such host",
            "error decoding response body from cdn-host.example [2001:db8::7]:443",
        ];
        for text in failures {
            let (st, out) = body(provider_failed(7, anyhow::anyhow!("{text}"))).await;
            assert_eq!(st, StatusCode::BAD_GATEWAY, "{text}");
            for leak in [
                "iptv-host",
                "cdn-host",
                "example",
                "::1",
                "2001",
                "8080",
                "443",
                "DnsName",
            ] {
                assert!(!out.contains(leak), "{leak} reached the page: {out}");
            }
        }
        let (_, out) = body(provider_failed(7, anyhow::anyhow!("{}", failures[0]))).await;
        assert_eq!(
            out,
            r#"{"error":"the show's episodes could not be fetched"}"#
        );
        let (_, out) = body(provider_failed(7, anyhow::anyhow!("{}", failures[2]))).await;
        assert_eq!(out, format!(r#"{{"error":"{NO_ANSWER}"}}"#));
    }

    #[tokio::test]
    async fn the_details_own_words_reach_the_page_as_they_are() {
        for said in SAID {
            let (st, out) = body(provider_failed(7, anyhow::anyhow!("{said}"))).await;
            assert_eq!(st, StatusCode::BAD_GATEWAY);
            assert_eq!(out, serde_json::json!({ "error": said }).to_string());
        }
        // A show that went while it was asked about is a 404.
        let (st, _) = body(provider_failed(7, anyhow::anyhow!("{GONE}"))).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn only_marking_watched_can_be_the_providers_failure() {
        let e = || anyhow::anyhow!("{FETCH_FAILED}: connect to iptv-host.example");
        let (st, out) = body(marks_failed(7, Some(true), e())).await;
        assert_eq!(st, StatusCode::BAD_GATEWAY);
        assert!(!out.contains("iptv-host"), "{out}");
        for watched in [None, Some(false)] {
            let (st, _) = body(marks_failed(
                7,
                watched,
                anyhow::anyhow!("database is locked"),
            ))
            .await;
            assert_eq!(st, StatusCode::INTERNAL_SERVER_ERROR, "{watched:?}");
        }
    }
}
