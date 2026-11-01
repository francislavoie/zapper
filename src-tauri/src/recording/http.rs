//! The service's HTTP API (docs/recording.md), on 127.0.0.1 only.
//!
//! Every route but `/api/version` wants `Authorization: Bearer <token>`,
//! the token in `server-token` beside the service's settings: on one machine
//! the app reads it from there, and nothing else on the machine can, the file
//! being the user's alone.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{Path, Query, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use super::api::{ErrorDto, RecordingSettingsDto, ScheduleRequest, VersionDto, API_VERSION};
use super::config::token_matches;
use super::server::Server;

type Shared = Arc<Server>;

struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(ErrorDto { error: self.1 })).into_response()
    }
}

fn bad(e: anyhow::Error) -> ApiError {
    let text = format!("{e:#}");
    let status = if text.contains("no such recording") {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::BAD_REQUEST
    };
    ApiError(status, text)
}

async fn auth(State(s): State<Shared>, req: Request, next: Next) -> Response {
    let given = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if !token_matches(&s.token, given.trim()) {
        return ApiError(StatusCode::UNAUTHORIZED, "a valid token is required".into())
            .into_response();
    }
    next.run(req).await
}

async fn version() -> Json<VersionDto> {
    Json(VersionDto {
        version: env!("CARGO_PKG_VERSION").to_string(),
        api_version: API_VERSION,
    })
}

async fn status(State(s): State<Shared>) -> impl IntoResponse {
    Json(s.status())
}

async fn list(State(s): State<Shared>) -> Result<impl IntoResponse, ApiError> {
    s.list().map(Json).map_err(bad)
}

async fn one(State(s): State<Shared>, Path(id): Path<i64>) -> Result<impl IntoResponse, ApiError> {
    s.get(id).map(Json).map_err(bad)
}

async fn schedule(
    State(s): State<Shared>,
    Json(req): Json<ScheduleRequest>,
) -> Result<impl IntoResponse, ApiError> {
    s.schedule(req)
        .map(|r| (StatusCode::CREATED, Json(r)))
        .map_err(bad)
}

async fn cancel(State(s): State<Shared>, Path(id): Path<i64>) -> Result<Response, ApiError> {
    match s.cancel(id).map_err(bad)? {
        Some(r) => Ok(Json(r).into_response()),
        // A scheduled recording cancelled is gone; say what it was.
        None => Ok(Json(serde_json::json!({ "id": id, "cancelled": true })).into_response()),
    }
}

#[derive(Deserialize)]
struct DeleteQuery {
    #[serde(default)]
    files: bool,
}

async fn delete(
    State(s): State<Shared>,
    Path(id): Path<i64>,
    Query(q): Query<DeleteQuery>,
) -> Result<impl IntoResponse, ApiError> {
    s.delete(id, q.files).map_err(bad)?;
    Ok(Json(serde_json::json!({ "id": id, "deleted": true })))
}

async fn settings(State(s): State<Shared>) -> impl IntoResponse {
    Json(s.settings())
}

async fn set_settings(
    State(s): State<Shared>,
    Json(new): Json<RecordingSettingsDto>,
) -> Result<impl IntoResponse, ApiError> {
    s.set_settings(new).map(Json).map_err(bad)
}

pub fn router(server: Shared) -> Router {
    let authed = Router::new()
        .route("/api/status", get(status))
        .route("/api/recordings", get(list).post(schedule))
        .route("/api/recordings/{id}", get(one).delete(delete))
        .route("/api/recordings/{id}/cancel", post(cancel))
        .route("/api/settings", get(settings).put(set_settings))
        .route_layer(middleware::from_fn_with_state(server.clone(), auth));
    Router::new()
        .route("/api/version", get(version))
        .merge(authed)
        .with_state(server)
}

/// Serves the API on 127.0.0.1:`port` until the process ends.
pub async fn serve(server: Shared, port: u16) -> anyhow::Result<()> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| anyhow::anyhow!("cannot listen on {addr}: {e}"))?;
    tracing::info!("recording service API on http://{addr}");
    axum::serve(listener, router(server)).await?;
    Ok(())
}
