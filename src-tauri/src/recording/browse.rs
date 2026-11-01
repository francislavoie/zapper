//! The catalogue over HTTP, for the web interface: categories, channel
//! pages, search, the guide, favourites and the sync's status — what the app
//! asks its own Rust side for through Tauri's commands, answered from the
//! same reads ([`crate::catalog::reads`]) out of the catalogue the service
//! keeps (docs/recording.md, "API").
//!
//! Provider addresses do not leave the service: a channel's logo is given as
//! this service's `/api/logo/{streamId}`, which fetches it ([`logo`]), so a
//! browser never sees the provider's URL (a playlist's may carry its login)
//! and the page needs nothing but its own origin.

use std::time::Duration;

use axum::extract::{Path, Query};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};
use serde::Deserialize;

use crate::catalog::{reads, status, sync_engine};
use crate::ipc::types::ChannelDto;

use super::api::ErrorDto;
use super::web::LOGO_CSP;

/// The most channels one page or one lookup returns.
const PAGE_MAX: u32 = 1000;
/// The most channels one guide request covers.
const IDS_MAX: usize = 500;
/// The most a logo may weigh.
const LOGO_MAX_BYTES: usize = 1 << 20;

struct Failed(StatusCode, String);

impl IntoResponse for Failed {
    fn into_response(self) -> Response {
        (self.0, Json(ErrorDto { error: self.1 })).into_response()
    }
}

fn bad_request(text: impl Into<String>) -> Failed {
    Failed(StatusCode::BAD_REQUEST, text.into())
}

/// Runs a catalogue read off the async threads: a guide search over a week
/// of programmes is not instant, and the service's other requests should not
/// wait behind it.
async fn read<T: Send + 'static>(
    f: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> Result<T, Failed> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| Failed(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| Failed(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))
}

/// Where a channel's logo is served from here, in place of the provider's.
pub fn logo_path(stream_id: i64) -> String {
    format!("/api/logo/{stream_id}")
}

/// Swaps a channel's provider logo for this service's address for it.
fn local_logo(ch: &mut ChannelDto) {
    if ch.logo_url.is_some() {
        ch.logo_url = Some(logo_path(ch.stream_id));
    }
}

fn local_logos(mut list: Vec<ChannelDto>) -> Vec<ChannelDto> {
    list.iter_mut().for_each(local_logo);
    list
}

/// `1,2,3`: stream ids, at most [`IDS_MAX`].
fn parse_ids(text: &str) -> Result<Vec<i64>, Failed> {
    let ids: Vec<i64> = text
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| t.parse::<i64>())
        .collect::<Result<_, _>>()
        .map_err(|_| bad_request("ids: stream ids separated by commas"))?;
    if ids.len() > IDS_MAX {
        return Err(bad_request(format!("ids: at most {IDS_MAX} at once")));
    }
    Ok(ids)
}

#[derive(Deserialize)]
struct PageQuery {
    /// Absent: every channel.
    category: Option<String>,
    #[serde(default)]
    offset: u32,
    limit: u32,
}

#[derive(Deserialize)]
struct IdsQuery {
    #[serde(default)]
    ids: String,
}

#[derive(Deserialize)]
struct SearchQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    offset: u32,
    limit: u32,
}

#[derive(Deserialize)]
struct IndexQuery {
    category: Option<String>,
}

#[derive(Deserialize)]
struct GuideQuery {
    #[serde(default)]
    ids: String,
    from: i64,
    to: i64,
}

#[derive(Deserialize)]
struct LimitQuery {
    limit: u32,
}

async fn categories() -> Result<Response, Failed> {
    Ok(Json(read(reads::categories).await?).into_response())
}

async fn channels(Query(q): Query<PageQuery>) -> Result<Response, Failed> {
    let limit = q.limit.min(PAGE_MAX);
    let page = read(move || reads::channels(q.category.as_deref(), q.offset, limit)).await?;
    Ok(Json(local_logos(page)).into_response())
}

async fn channels_by_ids(Query(q): Query<IdsQuery>) -> Result<Response, Failed> {
    let ids = parse_ids(&q.ids)?;
    let rows = read(move || reads::channels_by_ids(&ids)).await?;
    Ok(Json(local_logos(rows)).into_response())
}

async fn channel_numbers() -> Result<Response, Failed> {
    Ok(Json(read(reads::channel_numbers).await?).into_response())
}

async fn search(Query(q): Query<SearchQuery>) -> Result<Response, Failed> {
    let limit = q.limit.min(PAGE_MAX);
    let hits = read(move || reads::search(&q.q, limit)).await?;
    Ok(Json(local_logos(hits)).into_response())
}

async fn channel_index(
    Path(id): Path<i64>,
    Query(q): Query<IndexQuery>,
) -> Result<Response, Failed> {
    let at = read(move || reads::channel_index(q.category.as_deref(), id)).await?;
    Ok(Json(at).into_response())
}

async fn search_programmes(Query(q): Query<SearchQuery>) -> Result<Response, Failed> {
    let mut hits = read(move || reads::search_programmes(&q.q, q.offset, q.limit)).await?;
    hits.iter_mut().for_each(|h| local_logo(&mut h.channel));
    Ok(Json(hits).into_response())
}

async fn guide(Query(q): Query<GuideQuery>) -> Result<Response, Failed> {
    let ids = parse_ids(&q.ids)?;
    let g = read(move || reads::guide(&ids, q.from, q.to)).await?;
    Ok(Json(g).into_response())
}

async fn guide_rows(Query(q): Query<GuideQuery>) -> Result<Response, Failed> {
    let ids = parse_ids(&q.ids)?;
    let mut rows = read(move || reads::guide_rows(&ids, q.from, q.to)).await?;
    for r in &mut rows {
        if r.logo_url.is_some() {
            r.logo_url = Some(logo_path(r.stream_id));
        }
    }
    Ok(Json(rows).into_response())
}

async fn favourites(Query(q): Query<LimitQuery>) -> Result<Response, Failed> {
    let limit = q.limit.min(PAGE_MAX);
    let mut top = read(move || reads::favourites(limit)).await?;
    top.iter_mut().for_each(|f| local_logo(&mut f.channel));
    Ok(Json(top).into_response())
}

async fn forget(Path(id): Path<i64>) -> Result<Response, Failed> {
    read(move || reads::forget_activity(id)).await?;
    Ok(Json(serde_json::json!({ "streamId": id, "forgotten": true })).into_response())
}

async fn sync_status() -> Result<Response, Failed> {
    Ok(Json(read(|| Ok(status::snapshot())).await?).into_response())
}

/// Asks for a refresh now, through the service's own periodic loop (one
/// schedule, one pass at a time, and the lease shared with the app). With
/// `--no-sync` there is no loop, and the request waits for nothing.
async fn sync_now() -> Response {
    sync_engine::request_now();
    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "requested": true })),
    )
        .into_response()
}

/// A channel's logo, fetched from where the catalogue says it is. Only an
/// image, and not too large; cached by the browser for a day, and a missing
/// one for an hour (provider logos are missing constantly).
async fn logo(Path(id): Path<i64>) -> Response {
    let missing = || {
        (
            StatusCode::NOT_FOUND,
            [(header::CACHE_CONTROL, "private, max-age=3600")],
        )
            .into_response()
    };
    let Ok(Ok(Some(url))) = tokio::task::spawn_blocking(move || reads::logo_url(id)).await else {
        return missing();
    };
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return missing();
    }
    match fetch_logo(&url).await {
        Some((kind, bytes)) => (
            [
                (header::CONTENT_TYPE, kind),
                (header::CACHE_CONTROL, "private, max-age=86400".into()),
                (header::CONTENT_SECURITY_POLICY, LOGO_CSP.into()),
            ],
            bytes,
        )
            .into_response(),
        None => missing(),
    }
}

fn logo_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::limited(3))
            .build()
            .unwrap_or_default()
    })
}

/// The image at `url` and its type, or `None` for anything else: an error,
/// something that is not an image, or more than [`LOGO_MAX_BYTES`].
async fn fetch_logo(url: &str) -> Option<(String, Vec<u8>)> {
    let mut resp = logo_client().get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let kind = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or("").trim().to_ascii_lowercase())
        .filter(|v| v.starts_with("image/"))?;
    if resp.content_length().is_some_and(|n| n > LOGO_MAX_BYTES as u64) {
        return None;
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.ok()? {
        if body.len() + chunk.len() > LOGO_MAX_BYTES {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    Some((kind, body))
}

/// The routes, for the app or a signed-in device (`recording::http` puts
/// them behind its sign-in check).
pub fn routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/api/categories", get(categories))
        .route("/api/channels", get(channels))
        .route("/api/channels/by-id", get(channels_by_ids))
        .route("/api/channels/numbers", get(channel_numbers))
        .route("/api/channels/search", get(search))
        .route("/api/channels/{id}/index", get(channel_index))
        .route("/api/programmes/search", get(search_programmes))
        .route("/api/guide", get(guide))
        .route("/api/guide/rows", get(guide_rows))
        .route("/api/favourites", get(favourites))
        .route("/api/favourites/{id}", delete(forget))
        .route("/api/sync", get(sync_status).post(sync_now))
        .route("/api/logo/{id}", get(logo))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_numbers_separated_by_commas_and_not_too_many() {
        assert_eq!(parse_ids("1, 2,3").ok().unwrap(), vec![1, 2, 3]);
        assert_eq!(parse_ids("").ok().unwrap(), Vec::<i64>::new());
        assert!(parse_ids("1,x").is_err());
        let many = (0..=IDS_MAX).map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        assert!(parse_ids(&many).is_err());
    }
}
