//! The movie catalogue over HTTP, for the web interface: what the app asks
//! its own Rust side for through Tauri's commands ([`crate::ipc::vod_cmds`]),
//! answered from the same reads ([`crate::catalog::reads`]), behind the same
//! sign-in as [`super::browse`] (docs/recording.md, "API").
//!
//! Provider addresses do not leave the service, as with logos: a poster is
//! given as `/api/poster/movie/{id}` and a backdrop as
//! `/api/backdrop/movie/{id}/{n}`, which fetch the picture ([`poster`],
//! [`backdrop`]), so the page never sees where it came from (a provider's
//! pictures may sit behind the account's login).

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::Deserialize;

use crate::catalog::{reads, vod_details};
use crate::ipc::types::{
    MovieDetailsDto, MovieDto, MovieListKind, MovieQueryDto, MovieSort,
};

use super::browse::{bad_request, proxy_image, read, Failed};

/// The most movies one page or one lookup returns.
pub(super) const PAGE_MAX: u32 = 1000;
/// The most a poster or a backdrop may weigh.
pub(super) const POSTER_MAX_BYTES: usize = 4 << 20;

pub fn poster_path(id: i64) -> String {
    format!("/api/poster/movie/{id}")
}

pub fn backdrop_path(id: i64, n: usize) -> String {
    format!("/api/backdrop/movie/{id}/{n}")
}

/// Swaps a movie's provider poster for this service's address for it.
pub(super) fn local_movie(m: &mut MovieDto) {
    if m.poster_url.is_some() {
        m.poster_url = Some(poster_path(m.movie_id));
    }
}

fn local_movies(mut list: Vec<MovieDto>) -> Vec<MovieDto> {
    list.iter_mut().for_each(local_movie);
    list
}

/// Swaps the pictures of an opened movie for this service's addresses.
fn local_details(d: &mut MovieDetailsDto) {
    local_movie(&mut d.movie);
    let id = d.movie.movie_id;
    for (n, url) in d.backdrop_urls.iter_mut().enumerate() {
        *url = backdrop_path(id, n);
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ListParams {
    list: MovieListKind,
    category: Option<String>,
    sort: Option<MovieSort>,
    from: Option<i32>,
    to: Option<i32>,
    /// A search within the list: only the titles whose name matches.
    q: Option<String>,
    #[serde(default)]
    pub(super) offset: u32,
    pub(super) limit: u32,
}

#[derive(Deserialize)]
struct LimitParams {
    limit: u32,
}

#[derive(Deserialize)]
struct MarksBody {
    favourite: Option<bool>,
    watched: Option<bool>,
}

#[derive(Deserialize)]
struct ProgressBody {
    position: f64,
    duration: Option<f64>,
}

/// The query the words ask for. A category list without its category is the
/// caller's mistake, said before the catalogue is asked.
pub(super) fn movie_query(p: &ListParams) -> Result<MovieQueryDto, Failed> {
    if p.list == MovieListKind::Category && p.category.is_none() {
        return Err(bad_request("category: a category list needs its category"));
    }
    Ok(MovieQueryDto {
        list: p.list,
        category_id: p.category.clone(),
        sort: p.sort.unwrap_or(MovieSort::Added),
        year_from: p.from,
        year_to: p.to,
        search: p.q.clone(),
    })
}

/// A movie that is in the catalogue, or a 404.
async fn present(id: i64) -> Result<(), Failed> {
    if read(move || reads::movies_by_ids(&[id])).await?.is_empty() {
        return Err(Failed(
            StatusCode::NOT_FOUND,
            "that movie is not in the catalogue".into(),
        ));
    }
    Ok(())
}

async fn summary() -> Result<Response, Failed> {
    Ok(Json(read(reads::movies_summary).await?).into_response())
}

async fn categories() -> Result<Response, Failed> {
    Ok(Json(read(reads::movie_categories).await?).into_response())
}

async fn movies(Query(p): Query<ListParams>) -> Result<Response, Failed> {
    let q = movie_query(&p)?;
    let limit = p.limit.min(PAGE_MAX);
    let mut page = read(move || reads::movies(&q, p.offset, limit)).await?;
    page.items.iter_mut().for_each(local_movie);
    Ok(Json(page).into_response())
}

async fn details(Path(id): Path<i64>) -> Result<Response, Failed> {
    present(id).await?;
    // The one read here that may ask the provider, the first time a title is
    // opened in a week; its errors are scrubbed of the account already.
    // The failure is logged where it happens; what a transport error says
    // carries the provider's address, so the page is told only that it failed.
    let mut d = vod_details::movie_details(id).await.map_err(|_| {
        Failed(
            StatusCode::BAD_GATEWAY,
            "the provider did not answer for this movie".into(),
        )
    })?;
    local_details(&mut d);
    Ok(Json(d).into_response())
}

async fn similar(Path(id): Path<i64>, Query(p): Query<LimitParams>) -> Result<Response, Failed> {
    present(id).await?;
    let limit = p.limit.min(PAGE_MAX);
    let list = read(move || reads::similar_movies(id, limit)).await?;
    Ok(Json(local_movies(list)).into_response())
}

async fn set_marks(Path(id): Path<i64>, Json(b): Json<MarksBody>) -> Result<Response, Failed> {
    present(id).await?;
    let marks = read(move || reads::set_movie_marks(id, b.favourite, b.watched)).await?;
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
    present(id).await?;
    let marks = read(move || reads::save_movie_progress(id, b.position, b.duration)).await?;
    Ok(Json(marks).into_response())
}

/// A movie's poster, fetched from where the catalogue says it is; cached as
/// a logo is ([`super::browse`]).
async fn poster(Path(id): Path<i64>) -> Response {
    let url = read(move || reads::movie_poster_url(id)).await.ok().flatten();
    proxy_image(url, POSTER_MAX_BYTES).await
}

/// The `n`th backdrop of the details already kept for a movie.
async fn backdrop(Path((id, n)): Path<(i64, usize)>) -> Response {
    let url = read(move || reads::movie_backdrop_url(id, n))
        .await
        .ok()
        .flatten();
    proxy_image(url, POSTER_MAX_BYTES).await
}

/// The routes, for the app or a signed-in device (`recording::http` puts
/// them behind its sign-in check).
pub fn routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/api/vod/summary", get(summary))
        .route("/api/vod/categories", get(categories))
        .route("/api/vod/movies", get(movies))
        .route("/api/vod/movies/{id}", get(details))
        .route("/api/vod/movies/{id}/similar", get(similar))
        .route(
            "/api/vod/movies/{id}/marks",
            get(super::continue_browse::movie_marks).put(set_marks),
        )
        .route("/api/vod/movies/{id}/progress", put(save_progress))
        .route("/api/poster/movie/{id}", get(poster))
        .route("/api/backdrop/movie/{id}/{n}", get(backdrop))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Uri;

    fn params(query: &str) -> ListParams {
        Query::<ListParams>::try_from_uri(&format!("/api/vod/movies?{query}").parse::<Uri>().unwrap())
            .ok()
            .unwrap()
            .0
    }

    #[test]
    fn a_category_list_needs_its_category() {
        let p = params("list=category&limit=10");
        assert!(movie_query(&p).is_err());
        let p = params("list=category&category=7&limit=10");
        assert_eq!(movie_query(&p).ok().unwrap().category_id.as_deref(), Some("7"));
    }

    #[test]
    fn query_words_map_onto_the_query() {
        let p = params("list=recent&sort=rating&from=1990&to=1999&limit=50");
        assert_eq!(
            movie_query(&p).ok().unwrap(),
            MovieQueryDto {
                list: MovieListKind::Recent,
                category_id: None,
                sort: MovieSort::Rating,
                year_from: Some(1990),
                year_to: Some(1999),
                search: None,
            }
        );
        let p = params("list=all&sort=added&q=EN%20-&limit=50");
        assert_eq!(
            movie_query(&p).ok().unwrap(),
            MovieQueryDto {
                list: MovieListKind::All,
                category_id: None,
                sort: MovieSort::Added,
                year_from: None,
                year_to: None,
                search: Some("EN -".into()),
            }
        );
    }
}
