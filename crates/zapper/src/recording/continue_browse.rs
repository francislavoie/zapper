//! Continue watching over HTTP, for the web interface: what the app asks its
//! own Rust side for through Tauri's commands ([`crate::ipc::continue_cmds`]),
//! answered from the same reads ([`crate::catalog::continue_reads`]), behind
//! the same sign-in as [`super::browse`] (docs/recording.md, "API").
//!
//! One list for the app and every signed-in browser: it is built from the
//! shared catalogue's marks, so what one plays or removes, the others see.
//! A removal is dated by this service's clock, never by the request.
//!
//! As with movies and shows ([`super::vod_browse`], [`super::series_browse`]),
//! provider addresses do not leave the service: every poster and still in an
//! entry is this service's address for it.

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};
use serde::Deserialize;

use crate::catalog::continue_reads;
use crate::catalog::continue_store::CONTINUE_MAX;
use crate::ipc::types::{ContinueItemDto, ContinueKind};

use super::browse::{bad_request, read, Failed};
use super::series_browse::{local_episode, local_series};
use super::vod_browse::local_movie;

#[derive(Deserialize)]
struct ListParams {
    limit: Option<u32>,
}

/// What a failed read says: words of our own, never the error itself; why
/// goes to the service's log.
fn unreadable(e: Failed) -> Failed {
    tracing::warn!("continue watching: {}", e.1);
    Failed(
        StatusCode::INTERNAL_SERVER_ERROR,
        "the catalogue could not be read".into(),
    )
}

fn not_found() -> Failed {
    Failed(
        StatusCode::NOT_FOUND,
        "that title is not in the catalogue".into(),
    )
}

/// Swaps every picture of an entry for this service's address for it.
fn local_item(item: &mut ContinueItemDto) {
    match item {
        ContinueItemDto::Movie { movie, .. } => local_movie(movie),
        ContinueItemDto::Episode {
            series, episode, ..
        } => {
            local_series(series);
            local_episode(episode);
        }
    }
}

async fn list(Query(p): Query<ListParams>) -> Result<Response, Failed> {
    let limit = p.limit.unwrap_or(CONTINUE_MAX).min(CONTINUE_MAX);
    let mut items = read(move || continue_reads::continue_watching(limit))
        .await
        .map_err(unreadable)?;
    items.iter_mut().for_each(local_item);
    Ok(Json(items).into_response())
}

async fn remove(Path((kind, id)): Path<(String, i64)>) -> Result<Response, Failed> {
    let kind = match kind.as_str() {
        "movie" => ContinueKind::Movie,
        "series" => ContinueKind::Series,
        _ => return Err(bad_request("kind: movie or series")),
    };
    let removed = read(move || continue_reads::remove_from_continue(kind, id))
        .await
        .map_err(unreadable)?;
    if !removed {
        return Err(not_found());
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// A movie's marks as they are now (where it was left and when).
pub(super) async fn movie_marks(Path(id): Path<i64>) -> Result<Response, Failed> {
    let marks = read(move || continue_reads::movie_marks(id))
        .await
        .map_err(unreadable)?
        .ok_or_else(not_found)?;
    Ok(Json(marks).into_response())
}

/// An episode's marks as they are now.
pub(super) async fn episode_marks(Path(id): Path<i64>) -> Result<Response, Failed> {
    let marks = read(move || continue_reads::episode_marks(id))
        .await
        .map_err(unreadable)?
        .ok_or_else(not_found)?;
    Ok(Json(marks).into_response())
}

/// The routes, for the app or a signed-in device (`recording::http` puts
/// them behind its sign-in check). A title's marks are read beside where
/// they are set ([`super::vod_browse`], [`super::series_browse`]).
pub fn routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/api/vod/continue", get(list))
        .route("/api/vod/continue/{kind}/{id}", delete(remove))
}
