//! The shows over HTTP, for the web interface: what the app asks its own
//! Rust side for through Tauri's commands ([`crate::ipc::series_cmds`]),
//! answered from the same reads ([`crate::catalog::series_reads`],
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

use crate::catalog::{db, series_details, series_reads, series_store};
use crate::ipc::types::{EpisodeDto, SeriesDetailsDto, SeriesDto, SeriesQueryDto};

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
fn local_series(s: &mut SeriesDto) {
    if s.poster_url.is_some() {
        s.poster_url = Some(poster_path(s.series_id));
    }
}

/// Swaps an episode's provider still for this service's address for it.
fn local_episode(e: &mut EpisodeDto) {
    if e.still_url.is_some() {
        e.still_url = Some(still_path(e.episode_id));
    }
}

fn local_list(mut list: Vec<SeriesDto>) -> Vec<SeriesDto> {
    list.iter_mut().for_each(local_series);
    list
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

/// A provider's failure, for the page: `502`, with why (scrubbed already)
/// and no address.
fn provider_failed(e: anyhow::Error) -> Failed {
    Failed(
        StatusCode::BAD_GATEWAY,
        without_addresses(&format!("{e:#}")),
    )
}

#[derive(Deserialize)]
struct SearchParams {
    #[serde(default)]
    q: String,
    limit: u32,
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

async fn search(Query(p): Query<SearchParams>) -> Result<Response, Failed> {
    let limit = p.limit.min(PAGE_MAX);
    let hits = read(move || series_reads::search_series(&p.q, limit)).await?;
    Ok(Json(local_list(hits)).into_response())
}

async fn details(Path(id): Path<i64>) -> Result<Response, Failed> {
    present(id).await?;
    // The one read here that may ask the provider, the first time a show is
    // opened in a week; it logs its failure where it happens.
    let mut d = series_details::series_details(id)
        .await
        .map_err(provider_failed)?;
    local_series_details(&mut d);
    Ok(Json(d).into_response())
}

async fn set_marks(Path(id): Path<i64>, Json(b): Json<MarksBody>) -> Result<Response, Failed> {
    present(id).await?;
    // Marking watched a show whose episodes are not here yet asks for them.
    let marks = series_reads::set_series_marks(id, b.favourite, b.watched)
        .await
        .map_err(provider_failed)?;
    Ok(Json(marks).into_response())
}

async fn episode(Path(id): Path<i64>) -> Result<Response, Failed> {
    episode_present(id).await?;
    let mut playing = read(move || series_reads::playing_episode(id)).await?;
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
        .route("/api/vod/series/search", get(search))
        .route("/api/vod/series/{id}", get(details))
        .route("/api/vod/series/{id}/marks", put(set_marks))
        .route("/api/vod/episodes/{id}", get(episode))
        .route("/api/vod/episodes/{id}/marks", put(set_episode_marks))
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
}
