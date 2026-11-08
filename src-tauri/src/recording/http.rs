//! The service's HTTP API (docs/recording.md).
//!
//! Every request first passes [`access::admit`]: from another machine only
//! while network access is on, and over HTTPS unless plain HTTP was chosen.
//! Then every route but `/api/version` and `/api/login` wants
//! `Authorization: Bearer <token>`: the app's token (`server-token`, from
//! this computer only) or a device's, from signing in. Network access, the
//! password and the list of devices are the app's alone.
//!
//! A browser signs in for a cookie instead (`{"cookie": true}`), which
//! stands for the device's token on every request to this origin; requests
//! that rely on it must show they come from this origin's own page
//! ([`web::csrf`]). The web interface itself is served from here too
//! ([`web::files`]), with the catalogue API it reads ([`browse`]).
//!
//! No CORS headers: a page from another origin cannot read any answer.

use std::sync::Arc;

use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete as delete_route, get, post, put};
use axum::{Extension, Json, Router};
use serde::Deserialize;

use super::access::{self, Caller, LoginError, Peer, Refusal};
use super::api::{
    ErrorDto, LoginRequest, NetworkSettingsDto, PasswordRequest, RecordingSettingsDto,
    ScheduleRequest, ShutdownRequest, VersionDto, API_VERSION,
};
use super::backup::SetBackupRequest;
use super::now;
use super::server::Server;
use super::{account, browse, series_browse, strm, vod_browse, vodwatch, watch, web};

type Shared = Arc<Server>;

struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(ErrorDto { error: self.1 })).into_response()
    }
}

impl From<Refusal> for ApiError {
    fn from(r: Refusal) -> Self {
        ApiError(
            StatusCode::from_u16(r.status()).unwrap_or(StatusCode::FORBIDDEN),
            r.message().into(),
        )
    }
}

fn bad(e: anyhow::Error) -> ApiError {
    let text = format!("{e:#}");
    let status = if text.contains("no such recording") || text.contains("no such device") {
        StatusCode::NOT_FOUND
    } else if text.contains("in use by another program") {
        StatusCode::CONFLICT
    } else {
        StatusCode::BAD_REQUEST
    };
    ApiError(status, text)
}

/// The connection's peer. A request without one did not come through the
/// listener, and is treated as from another machine over plain HTTP: the
/// most refused there is.
fn peer(req: &Request) -> Peer {
    req.extensions().get::<Peer>().copied().unwrap_or(Peer {
        addr: std::net::SocketAddr::from(([0, 0, 0, 0], 0)),
        tls: false,
    })
}

fn bearer(req: &Request) -> Option<&str> {
    req.headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

/// Every request: where it may come from.
///
/// A browser that asks for the web interface over plain HTTP while the
/// service wants HTTPS is sent to the same address over HTTPS rather than
/// shown an error; nothing is answered over plain HTTP either way.
async fn gate(State(s): State<Shared>, req: Request, next: Next) -> Response {
    let p = peer(&req);
    if let Err(r) = access::admit(&p, &s.config().network) {
        if r == Refusal::NeedsHttps {
            if let Some(to) = https_redirect(&req) {
                return to;
            }
        }
        return ApiError::from(r).into_response();
    }
    next.run(req).await
}

/// `308` to the HTTPS address of a page of the web interface, for a `GET`
/// outside the API that names its host.
fn https_redirect(req: &Request) -> Option<Response> {
    let path = req.uri().path();
    if req.method() != axum::http::Method::GET || path == "/api" || path.starts_with("/api/") {
        return None;
    }
    let host = req.headers().get(header::HOST)?.to_str().ok()?;
    if host.is_empty() || host.contains(['/', '\\', '@', ' ']) {
        return None;
    }
    let to = HeaderValue::from_str(&format!("https://{host}{path}")).ok()?;
    Some((StatusCode::PERMANENT_REDIRECT, [(header::LOCATION, to)]).into_response())
}

/// Who it is: the app, or a signed-in device.
///
/// A bearer token is the app's or a device's. A cookie can only ever be a
/// device's: the service sets one only from a sign-in, and the app's token
/// in one (put there by hand, or by a page that somehow learnt it) is
/// refused rather than trusted, so nothing a browser sends by itself is
/// worth more than a device.
fn caller(s: &Server, req: &Request) -> Result<Caller, Refusal> {
    let p = peer(req);
    if let Some(token) = bearer(req) {
        return s
            .access
            .devices(|d| access::identify(&p, Some(token), &s.token, d, now()));
    }
    let token = web::cookie_token(req.headers());
    if token.is_some_and(|t| super::config::token_matches(&s.token, t)) {
        return Err(Refusal::BadToken);
    }
    s.access
        .devices(|d| access::identify(&p, token, &s.token, d, now()))
}

/// The app or a signed-in device.
async fn auth(State(s): State<Shared>, mut req: Request, next: Next) -> Response {
    match caller(&s, &req) {
        Ok(c) => {
            req.extensions_mut().insert(c);
            next.run(req).await
        }
        Err(r) => ApiError::from(r).into_response(),
    }
}

/// The app on this computer only.
async fn app_only(State(s): State<Shared>, req: Request, next: Next) -> Response {
    match caller(&s, &req) {
        Ok(Caller::App) => next.run(req).await,
        Ok(Caller::Device(_)) => ApiError::from(Refusal::AppOnly).into_response(),
        Err(r) => ApiError::from(r).into_response(),
    }
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
    let mut list = s.list().map_err(bad)?;
    s.add_conflicts(&mut list, s.account_max().await);
    Ok(Json(list))
}

async fn one(State(s): State<Shared>, Path(id): Path<i64>) -> Result<impl IntoResponse, ApiError> {
    let mut r = s.get(id).map_err(bad)?;
    s.add_conflicts(std::slice::from_mut(&mut r), s.account_max().await);
    Ok(Json(r))
}

async fn schedule(
    State(s): State<Shared>,
    Json(req): Json<ScheduleRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let mut r = s.schedule(req).map_err(bad)?;
    s.add_conflicts(std::slice::from_mut(&mut r), s.account_max().await);
    Ok((StatusCode::CREATED, Json(r)))
}

/// The conflicts a recording would bring, before it is scheduled.
async fn preview(
    State(s): State<Shared>,
    Json(req): Json<ScheduleRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let max = s.account_max().await;
    s.preview(&req, max).map(Json).map_err(bad)
}

async fn cancel(State(s): State<Shared>, Path(id): Path<i64>) -> Result<Response, ApiError> {
    match s.cancel(id).map_err(bad)? {
        Some(r) => Ok(Json(r).into_response()),
        // A scheduled recording cancelled is gone; say what it was.
        None => Ok(Json(serde_json::json!({ "id": id, "cancelled": true })).into_response()),
    }
}

/// The channels a recording could fall back to, best first.
async fn backups(State(s): State<Shared>, Path(id): Path<i64>) -> Result<impl IntoResponse, ApiError> {
    let s2 = s.clone();
    // The catalogue read can take a moment on a large guide: off the async
    // threads, like the browse API's.
    tokio::task::spawn_blocking(move || s2.backups(id))
        .await
        .map_err(|e| bad(anyhow::anyhow!("{e}")))?
        .map(Json)
        .map_err(bad)
}

/// Chooses a recording's backup feed, or none (`{"streamId": null}`).
async fn set_backup(
    State(s): State<Shared>,
    Path(id): Path<i64>,
    Json(req): Json<SetBackupRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let mut r = s.set_backup(id, req.stream_id).map_err(bad)?;
    s.add_conflicts(std::slice::from_mut(&mut r), s.account_max().await);
    Ok(Json(r))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreludeQuery {
    /// When the first packet arrived, Unix milliseconds.
    first_ms: i64,
}

/// What the app's player had buffered of the programme before the recording
/// began, as its first part (capture::add_prelude). The app's alone: only
/// its player has such a buffer, and the body is written to disk as it
/// comes.
async fn prelude(
    State(s): State<Shared>,
    Path(id): Path<i64>,
    Query(q): Query<PreludeQuery>,
    req: Request,
) -> Result<impl IntoResponse, ApiError> {
    let body = req.into_body().into_data_stream();
    let part = super::capture::add_prelude(&s, id, q.first_ms, body)
        .await
        .map_err(|e| {
            let text = format!("{e:#}");
            if text.contains("no longer being recorded") {
                ApiError(StatusCode::CONFLICT, text)
            } else {
                bad(e)
            }
        })?;
    Ok((StatusCode::CREATED, Json(part)))
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

/// Stop or restart, now or once idle (lifecycle). It answers first, with
/// its status; the app's alone, like everything that changes the service
/// itself.
async fn shutdown(State(s): State<Shared>, Json(r): Json<ShutdownRequest>) -> impl IntoResponse {
    s.request_shutdown(r);
    Json(s.status())
}

// ------------------------------------------------------------- network

async fn network(State(s): State<Shared>) -> impl IntoResponse {
    Json(s.network())
}

async fn set_network(
    State(s): State<Shared>,
    Json(new): Json<NetworkSettingsDto>,
) -> Result<impl IntoResponse, ApiError> {
    s.set_network(new).await.map(Json).map_err(bad)
}

async fn set_password(
    State(s): State<Shared>,
    Json(req): Json<PasswordRequest>,
) -> Result<impl IntoResponse, ApiError> {
    s.set_password(&req.password).await.map(Json).map_err(bad)
}

async fn devices(State(s): State<Shared>) -> impl IntoResponse {
    Json(s.access.devices(|d| d.list()))
}

async fn revoke(
    State(s): State<Shared>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    s.revoke_device(&id).map(Json).map_err(bad)
}

async fn login(
    State(s): State<Shared>,
    p: Option<Extension<Peer>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Response {
    // Always there through the listener; see `peer` for the fallback.
    let p = p.map(|Extension(p)| p).unwrap_or(Peer {
        addr: std::net::SocketAddr::from(([0, 0, 0, 0], 0)),
        tls: false,
    });
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    // A cookie is something a browser sends by itself from then on, so only
    // the interface's own page may ask for one (see `web::csrf`).
    if req.cookie && !web::has_csrf_header(&headers) {
        return ApiError(
            StatusCode::FORBIDDEN,
            "refused: the request did not come from zapper's own page".into(),
        )
        .into_response();
    }
    match s
        .access
        .login(p.addr.ip(), &req.password, req.name.as_deref(), ua, now())
        .await
    {
        // The token goes where the page cannot read it, and nowhere else.
        Ok(out) if req.cookie => (
            StatusCode::CREATED,
            [(header::SET_COOKIE, web::session_cookie(&out.token, p.tls))],
            Json(serde_json::json!({ "device": out.device })),
        )
            .into_response(),
        Ok(out) => (StatusCode::CREATED, Json(out)).into_response(),
        Err(LoginError::Wait(d)) => {
            let secs = d.as_secs_f64().ceil().max(1.0) as u64;
            let mut r = ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                format!("too many wrong passwords; try again in {secs} s"),
            )
            .into_response();
            if let Ok(v) = HeaderValue::from_str(&secs.to_string()) {
                r.headers_mut().insert(header::RETRY_AFTER, v);
            }
            r
        }
        Err(LoginError::NoPassword) => ApiError(
            StatusCode::FORBIDDEN,
            "signing in is not set up: set a password in zapper's Settings › Recording".into(),
        )
        .into_response(),
        Err(LoginError::Wrong) => {
            ApiError(StatusCode::UNAUTHORIZED, "wrong password".into()).into_response()
        }
        Err(LoginError::Other(e)) => bad(e).into_response(),
    }
}

/// Signs the calling device out, and a browser's cookie with it.
async fn logout(
    State(s): State<Shared>,
    Extension(c): Extension<Caller>,
    p: Option<Extension<Peer>>,
) -> Result<impl IntoResponse, ApiError> {
    let Caller::Device(id) = c else {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "the app is not signed in; there is nothing to sign out of".into(),
        ));
    };
    s.access.devices(|d| d.revoke(&id)).map_err(bad)?;
    let tls = p.is_some_and(|Extension(p)| p.tls);
    Ok((
        [(header::SET_COOKIE, web::cleared_cookie(tls))],
        Json(serde_json::json!({ "signedOut": true })),
    ))
}

/// Who is asking: `{device}` for a signed-in device, `{app: true}` for the
/// app, `{signedIn: false}` for anyone else. The web interface asks first,
/// to know whether to show its sign-in, so not being signed in is an answer
/// here rather than a 401.
async fn session(State(s): State<Shared>, req: Request) -> Response {
    let device = match caller(&s, &req) {
        Ok(Caller::App) => return Json(serde_json::json!({ "app": true })).into_response(),
        Ok(Caller::Device(id)) => s
            .access
            .devices(|d| d.list())
            .into_iter()
            .find(|d| d.id == id),
        Err(_) => None,
    };
    match device {
        Some(d) => Json(serde_json::json!({ "signedIn": true, "device": d })).into_response(),
        None => Json(serde_json::json!({ "signedIn": false })).into_response(),
    }
}

pub fn router(server: Shared) -> Router {
    let authed = Router::new()
        .route("/api/status", get(status))
        .route("/api/recordings", get(list).post(schedule))
        .route("/api/recordings/preview", post(preview))
        .route("/api/recordings/{id}", get(one).delete(delete))
        .route("/api/recordings/{id}/cancel", post(cancel))
        .route("/api/recordings/{id}/backups", get(backups))
        .route("/api/recordings/{id}/backup", put(set_backup))
        .route("/api/settings", get(settings).put(set_settings))
        .route("/api/logout", post(logout))
        .merge(browse::routes())
        .merge(vod_browse::routes())
        .merge(series_browse::routes())
        .merge(account::routes())
        .merge(watch::routes())
        .merge(vodwatch::routes())
        .route_layer(middleware::from_fn_with_state(server.clone(), auth));
    let app = Router::new()
        .route("/api/network", get(network).put(set_network))
        .route("/api/network/password", put(set_password))
        .route("/api/devices", get(devices))
        .route("/api/devices/{id}", delete_route(revoke))
        .route("/api/shutdown", post(shutdown))
        .route("/api/recordings/{id}/prelude", put(prelude))
        .route_layer(middleware::from_fn_with_state(server.clone(), app_only));
    Router::new()
        .route("/api/version", get(version))
        .route("/api/login", post(login))
        .route("/api/session", get(session))
        // Its own key in the address stands in for signing in (`strm`).
        .merge(strm::routes())
        .merge(authed)
        .merge(app)
        .fallback(web::files)
        .layer(middleware::from_fn(web::csrf))
        .layer(middleware::from_fn_with_state(server.clone(), gate))
        .layer(middleware::from_fn(web::headers))
        .with_state(server)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::net::SocketAddr;
    use std::time::Duration;

    use axum::body::Body;
    use tower::ServiceExt;

    use super::*;
    use crate::catalog::db::movie_catalogue;
    use crate::catalog::vod_store::VodItem;
    use crate::recording::budget::{Budget, Slots};
    use crate::recording::config::{self, ServerConfig};
    use crate::recording::api::RecordingState;
    use crate::recording::store;

    struct Free;
    impl Budget for Free {
        fn check(
            &self,
            _own: usize,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Slots> + Send + '_>> {
            Box::pin(async { Slots::Free })
        }
    }

    const APP: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    pub(crate) fn server(name: &str) -> Shared {
        let d = std::env::temp_dir().join(format!("zapper-http-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let cfg = ServerConfig::default();
        let path = d.join("server.json");
        config::save_to(&path, &cfg).unwrap();
        Server::new(
            store::open_in_memory().unwrap(),
            cfg,
            path,
            APP.into(),
            Box::new(Free),
        )
    }

    fn req(method: &str, uri: &str, from: &str, tls: bool, token: Option<&str>) -> Request {
        req_body(method, uri, from, tls, token, None)
    }

    fn req_body(
        method: &str,
        uri: &str,
        from: &str,
        tls: bool,
        token: Option<&str>,
        body: Option<serde_json::Value>,
    ) -> Request {
        let mut b = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        let mut r = match body {
            Some(j) => b
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(j.to_string()))
                .unwrap(),
            None => b.body(Body::empty()).unwrap(),
        };
        r.extensions_mut().insert(Peer {
            addr: SocketAddr::new(from.parse().unwrap(), 40000),
            tls,
        });
        r
    }

    async fn call(s: &Shared, r: Request) -> (StatusCode, serde_json::Value) {
        let resp = router(s.clone()).oneshot(r).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    const LAN: &str = "192.168.1.30";
    const HOME: &str = "127.0.0.1";

    async fn enable(s: &Shared, https: bool) {
        s.access.set_password("correct horse").await.unwrap();
        let mut c = s.config();
        c.network.enabled = true;
        c.network.https = https;
        s.replace_config(c);
    }

    #[tokio::test]
    async fn the_app_token_works_here_and_nowhere_else() {
        let s = server("app");
        enable(&s, true).await;
        assert_eq!(
            call(&s, req("GET", "/api/status", HOME, false, Some(APP)))
                .await
                .0,
            StatusCode::OK
        );
        let (st, body) = call(&s, req("GET", "/api/status", LAN, true, Some(APP))).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        assert!(body["error"]
            .as_str()
            .unwrap()
            .contains("not accepted from another device"));
        assert_eq!(
            call(&s, req("GET", "/api/status", HOME, false, None))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn only_the_app_here_may_stop_the_service() {
        let s = server("shutdown");
        enable(&s, true).await;
        let body = || Some(serde_json::json!({"when": "idle", "restart": true}));
        let (st, _) = call(&s, req_body("POST", "/api/shutdown", HOME, false, None, body())).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        let (st, _) = call(&s, req_body("POST", "/api/shutdown", LAN, true, Some(APP), body())).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        assert_eq!(s.requested_shutdown(), None);
        let (st, reply) = call(&s, req_body("POST", "/api/shutdown", HOME, false, Some(APP), body())).await;
        assert_eq!(st, StatusCode::OK);
        // It answers with its status: idle, with nothing scheduled.
        assert_eq!(reply["idle"], true);
        let r = s.requested_shutdown().unwrap();
        assert_eq!((r.when, r.restart), (crate::recording::api::ShutdownWhen::Idle, true));
    }

    #[tokio::test]
    async fn another_machine_gets_nothing_while_network_access_is_off() {
        let s = server("off");
        s.access.set_password("correct horse").await.unwrap();
        for path in ["/api/version", "/api/status"] {
            let (st, body) = call(&s, req("GET", path, LAN, true, Some(APP))).await;
            assert_eq!(st, StatusCode::FORBIDDEN, "{path}");
            assert!(body["error"].as_str().unwrap().contains("off"));
        }
        let (st, _) = call(
            &s,
            req_body(
                "POST",
                "/api/login",
                LAN,
                true,
                None,
                Some(serde_json::json!({"password": "correct horse"})),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn plain_http_from_another_machine_is_refused_unless_chosen() {
        let s = server("plain");
        enable(&s, true).await;
        let (st, body) = call(&s, req("GET", "/api/version", LAN, false, None)).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        assert!(body["error"].as_str().unwrap().contains("https://"));
        assert_eq!(
            call(&s, req("GET", "/api/version", LAN, true, None))
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(
            call(&s, req("GET", "/api/version", HOME, false, None))
                .await
                .0,
            StatusCode::OK
        );
        enable(&s, false).await;
        assert_eq!(
            call(&s, req("GET", "/api/version", LAN, false, None))
                .await
                .0,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn a_device_signs_in_uses_the_api_and_is_revoked() {
        let s = server("device");
        enable(&s, true).await;
        let login = |pw: &str| {
            let mut r = req_body(
                "POST",
                "/api/login",
                LAN,
                true,
                None,
                Some(serde_json::json!({ "password": pw })),
            );
            r.headers_mut().insert(
                header::USER_AGENT,
                HeaderValue::from_static("Mozilla/5.0 (Android 14) Firefox/131.0"),
            );
            r
        };
        let (st, body) = call(&s, login("wrong horse")).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"], "wrong password");
        let (st, body) = call(&s, login("correct horse")).await;
        assert_eq!(st, StatusCode::CREATED);
        let token = body["token"].as_str().unwrap().to_string();
        let id = body["device"]["id"].as_str().unwrap().to_string();
        assert_eq!(body["device"]["name"], "Firefox on Android");

        assert_eq!(
            call(&s, req("GET", "/api/recordings", LAN, true, None))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(&s, req("GET", "/api/recordings", LAN, true, Some(&token)))
                .await
                .0,
            StatusCode::OK
        );
        // Network access, the password and the devices are the app's.
        for (m, path) in [("GET", "/api/network"), ("GET", "/api/devices")] {
            let (st, _) = call(&s, req(m, path, LAN, true, Some(&token))).await;
            assert_eq!(st, StatusCode::FORBIDDEN, "{path}");
        }
        let (st, _) = call(
            &s,
            req_body(
                "PUT",
                "/api/network",
                LAN,
                true,
                Some(&token),
                Some(serde_json::json!({"enabled": true, "port": 8796, "https": false})),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        let (st, _) = call(
            &s,
            req_body(
                "PUT",
                "/api/network/password",
                LAN,
                true,
                Some(&token),
                Some(serde_json::json!({"password": "taken over"})),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN);

        // The app lists the device and revokes it.
        let (st, body) = call(&s, req("GET", "/api/network", HOME, false, Some(APP))).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["devices"][0]["id"], id.as_str());
        assert_eq!(body["devices"][0]["address"], LAN);
        assert_eq!(body["passwordSet"], true);
        assert!(body.get("password").is_none());
        let (st, body) = call(
            &s,
            req(
                "DELETE",
                &format!("/api/devices/{id}"),
                HOME,
                false,
                Some(APP),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["devices"].as_array().unwrap().len(), 0);
        assert_eq!(
            call(&s, req("GET", "/api/recordings", LAN, true, Some(&token)))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(
                &s,
                req(
                    "DELETE",
                    &format!("/api/devices/{id}"),
                    HOME,
                    false,
                    Some(APP)
                )
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn a_device_can_sign_itself_out() {
        let s = server("logout");
        enable(&s, true).await;
        let (_, body) = call(
            &s,
            req_body(
                "POST",
                "/api/login",
                LAN,
                true,
                None,
                Some(serde_json::json!({"password": "correct horse", "name": "Kitchen"})),
            ),
        )
        .await;
        let token = body["token"].as_str().unwrap().to_string();
        assert_eq!(body["device"]["name"], "Kitchen");
        assert_eq!(
            call(&s, req("POST", "/api/logout", LAN, true, Some(&token)))
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(
            call(&s, req("GET", "/api/status", LAN, true, Some(&token)))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(&s, req("POST", "/api/logout", HOME, false, Some(APP)))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn wrong_passwords_are_slowed_down_per_address() {
        let s = server("limit");
        enable(&s, true).await;
        let try_from = |from: &str, pw: &str| {
            req_body(
                "POST",
                "/api/login",
                from,
                true,
                None,
                Some(serde_json::json!({ "password": pw })),
            )
        };
        for _ in 0..access::FREE_TRIES - 1 {
            assert_eq!(
                call(&s, try_from(LAN, "nope nope")).await.0,
                StatusCode::UNAUTHORIZED
            );
        }
        let resp = router(s.clone())
            .oneshot(try_from(LAN, "nope nope"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(resp.headers()[header::RETRY_AFTER], "1");
        assert_eq!(
            call(&s, try_from(LAN, "correct horse")).await.0,
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            call(&s, try_from("192.168.1.31", "correct horse")).await.0,
            StatusCode::CREATED
        );
    }

    #[tokio::test]
    async fn network_settings_are_checked_before_anything_changes() {
        let s = server("netset");
        let put = |body: serde_json::Value| {
            req_body("PUT", "/api/network", HOME, false, Some(APP), Some(body))
        };
        let (st, body) = call(
            &s,
            put(serde_json::json!({"enabled": true, "port": 8796, "https": true})),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(
            body["error"].as_str().unwrap().contains("password"),
            "{body}"
        );
        let (st, body) = call(
            &s,
            put(serde_json::json!({"enabled": false, "port": 80, "https": true})),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(body["error"].as_str().unwrap().contains("1024"), "{body}");
        let (st, body) = call(
            &s,
            req_body(
                "PUT",
                "/api/network/password",
                HOME,
                false,
                Some(APP),
                Some(serde_json::json!({"password": "short"})),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(
            body["error"].as_str().unwrap().contains("8 characters"),
            "{body}"
        );
        assert_eq!(s.config(), ServerConfig::default(), "nothing changed");
        let (st, body) = call(
            &s,
            req_body(
                "PUT",
                "/api/network/password",
                HOME,
                false,
                Some(APP),
                Some(serde_json::json!({"password": "correct horse"})),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["passwordSet"], true);
    }

    /// A port nothing listens on now, for tests that must name one (Settings
    /// refuses port 0).
    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    /// A fresh connection each time: a settings change closes the ones
    /// before it, which a pooled client would trip over.
    fn client() -> reqwest::Client {
        reqwest::Client::builder()
            .no_proxy()
            .pool_max_idle_per_host(0)
            // The service's own certificate, self-signed: checked by
            // fingerprint below rather than by an authority.
            .danger_accept_invalid_certs(true)
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap()
    }

    async fn get(url: &str, token: Option<&str>) -> Result<u16, reqwest::Error> {
        let mut r = client().get(url);
        if let Some(t) = token {
            r = r.bearer_auth(t);
        }
        Ok(r.send().await?.status().as_u16())
    }

    async fn put_network(port: u16, body: serde_json::Value) -> (u16, serde_json::Value) {
        let r = client()
            .put(format!("http://127.0.0.1:{port}/api/network"))
            .bearer_auth(APP)
            .json(&body)
            .send()
            .await
            .unwrap();
        (r.status().as_u16(), r.json().await.unwrap())
    }

    /// The whole path over real sockets: the listener, plain HTTP and TLS on
    /// one port, the peer's address, and changing where it listens while it
    /// runs. Other devices are stood for by this computer's own network
    /// address, when it has one: a connection to it comes from it, which is
    /// not a loopback address.
    #[tokio::test]
    async fn the_service_over_real_sockets() {
        let s = server("sockets");
        let a = free_port();
        let mut cfg = s.config();
        cfg.port = a;
        s.replace_config(cfg);
        s.listen().await.unwrap();
        let home = |port: u16, path: &str| format!("http://127.0.0.1:{port}{path}");
        assert_eq!(get(&home(a, "/api/version"), None).await.unwrap(), 200);
        assert_eq!(get(&home(a, "/api/status"), Some(APP)).await.unwrap(), 200);
        assert_eq!(get(&home(a, "/api/status"), None).await.unwrap(), 401);

        // Not without a password.
        let (st, body) = put_network(
            a,
            serde_json::json!({"enabled": true, "port": a, "https": true}),
        )
        .await;
        assert_eq!(st, 400, "{body}");
        let st = client()
            .put(home(a, "/api/network/password"))
            .bearer_auth(APP)
            .json(&serde_json::json!({"password": "correct horse"}))
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(st, 200);

        // A port in use is refused, and nothing changes.
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let c = taken.local_addr().unwrap().port();
        let (st, body) = put_network(
            a,
            serde_json::json!({"enabled": false, "port": c, "https": true}),
        )
        .await;
        assert_eq!(st, 409, "{body}");
        assert_eq!(
            body["error"],
            format!("port {c} is in use by another program")
        );
        assert_eq!(get(&home(a, "/api/version"), None).await.unwrap(), 200);
        assert_eq!(s.config().port, a);

        // Another port: the old one closes, the new one answers, and the
        // app finds it in server.json.
        let b = free_port();
        let (st, body) = put_network(
            a,
            serde_json::json!({"enabled": false, "port": b, "https": true}),
        )
        .await;
        assert_eq!(st, 200, "{body}");
        assert_eq!(body["port"], b);
        assert!(
            get(&home(a, "/api/version"), None).await.is_err(),
            "{a} is closed"
        );
        assert_eq!(get(&home(b, "/api/version"), None).await.unwrap(), 200);
        let saved = config::load_from(&s.dir_for_test().join("server.json"));
        assert_eq!(saved.port, b);

        // Network access on, on the same port: every interface, HTTPS for
        // other devices, plain HTTP still for this computer's app.
        let (st, body) = put_network(
            b,
            serde_json::json!({"enabled": true, "port": b, "https": true}),
        )
        .await;
        assert_eq!(st, 200, "{body}");
        let fp = body["fingerprint"].as_str().unwrap().to_string();
        assert_eq!(fp.len(), 95, "{fp}");
        assert!(saved_has_no_secret(&s));
        assert_eq!(get(&home(b, "/api/status"), Some(APP)).await.unwrap(), 200);
        assert_eq!(
            get(&format!("https://127.0.0.1:{b}/api/status"), Some(APP))
                .await
                .unwrap(),
            200
        );
        let on: Vec<std::net::SocketAddr> = s.listening_on().await;
        assert!(on.iter().any(|a| a.ip().is_unspecified()), "{on:?}");

        let lan = crate::recording::listen::interface_ips()
            .into_iter()
            .find(|ip| ip.is_ipv4());
        if let Some(lan) = lan {
            let url = |scheme: &str, path: &str| format!("{scheme}://{lan}:{b}{path}");
            // Plain HTTP from another device: refused, with the way in.
            let r = client()
                .get(url("http", "/api/version"))
                .send()
                .await
                .unwrap();
            assert_eq!(r.status().as_u16(), 403);
            assert!(r.text().await.unwrap().contains("https://"));
            assert_eq!(get(&url("https", "/api/version"), None).await.unwrap(), 200);
            assert_eq!(get(&url("https", "/api/status"), None).await.unwrap(), 401);
            assert_eq!(
                get(&url("https", "/api/status"), Some(APP)).await.unwrap(),
                401,
                "the app's token is refused from the network"
            );
            let r = client()
                .post(url("https", "/api/login"))
                .header(
                    "User-Agent",
                    "Mozilla/5.0 (X11; Linux x86_64) Firefox/131.0",
                )
                .json(&serde_json::json!({"password": "correct horse"}))
                .send()
                .await
                .unwrap();
            assert_eq!(r.status().as_u16(), 201);
            let login: serde_json::Value = r.json().await.unwrap();
            let token = login["token"].as_str().unwrap().to_string();
            assert_eq!(login["device"]["address"], lan.to_string());
            assert_eq!(
                get(&url("https", "/api/status"), Some(&token))
                    .await
                    .unwrap(),
                200
            );
            assert_eq!(
                get(&url("https", "/api/network"), Some(&token))
                    .await
                    .unwrap(),
                403,
                "the app's alone"
            );
            assert_eq!(
                cert_fingerprint(lan, b).await,
                fp,
                "the certificate served is the one Settings shows"
            );

            // Off again: another device cannot connect at all.
            let (st, _) = put_network(
                b,
                serde_json::json!({"enabled": false, "port": b, "https": true}),
            )
            .await;
            assert_eq!(st, 200);
            assert!(get(&url("https", "/api/version"), None).await.is_err());
            assert_eq!(get(&home(b, "/api/status"), Some(APP)).await.unwrap(), 200);
        } else {
            eprintln!("no network address here: the requests from another device were not tried");
        }
    }

    fn saved_has_no_secret(s: &Server) -> bool {
        let text = std::fs::read_to_string(s.dir_for_test().join("server.json")).unwrap();
        !text.contains("argon2") && !text.contains("correct horse") && !text.contains("PRIVATE KEY")
    }

    /// The SHA-256 of the certificate the service presents at `ip:port`.
    async fn cert_fingerprint(ip: std::net::IpAddr, port: u16) -> String {
        use rustls::client::danger::{
            HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
        };
        use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
        use rustls::{DigitallySignedStruct, SignatureScheme};

        #[derive(Debug)]
        struct Any;
        impl ServerCertVerifier for Any {
            fn verify_server_cert(
                &self,
                _: &CertificateDer<'_>,
                _: &[CertificateDer<'_>],
                _: &ServerName<'_>,
                _: &[u8],
                _: UnixTime,
            ) -> Result<ServerCertVerified, rustls::Error> {
                Ok(ServerCertVerified::assertion())
            }
            fn verify_tls12_signature(
                &self,
                _: &[u8],
                _: &CertificateDer<'_>,
                _: &DigitallySignedStruct,
            ) -> Result<HandshakeSignatureValid, rustls::Error> {
                Ok(HandshakeSignatureValid::assertion())
            }
            fn verify_tls13_signature(
                &self,
                _: &[u8],
                _: &CertificateDer<'_>,
                _: &DigitallySignedStruct,
            ) -> Result<HandshakeSignatureValid, rustls::Error> {
                Ok(HandshakeSignatureValid::assertion())
            }
            fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
                rustls::crypto::aws_lc_rs::default_provider()
                    .signature_verification_algorithms
                    .supported_schemes()
            }
        }
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Any))
        .with_no_client_auth();
        let tcp = tokio::net::TcpStream::connect((ip, port)).await.unwrap();
        let tls = tokio_rustls::TlsConnector::from(Arc::new(config))
            .connect(ServerName::IpAddress(ip.into()), tcp)
            .await
            .unwrap();
        let (_, conn) = tls.get_ref();
        crate::recording::tls::fingerprint(&conn.peer_certificates().unwrap()[0])
    }

    #[tokio::test]
    async fn no_cors_headers_are_sent() {
        let s = server("cors");
        let mut r = req("GET", "/api/version", HOME, false, None);
        r.headers_mut().insert(
            header::ORIGIN,
            HeaderValue::from_static("http://evil.example"),
        );
        let resp = router(s.clone()).oneshot(r).await.unwrap();
        assert!(resp
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none());
    }

    // ------------------------------------------------------------ browsers

    const LAN_HOST: &str = "192.168.1.20:8796";
    const LAN_ORIGIN: &str = "https://192.168.1.20:8796";

    /// A request as the web interface's own page sends it: the cookie, the
    /// interface's header and this origin.
    fn from_page(
        method: &str,
        uri: &str,
        cookie: Option<&str>,
        body: Option<serde_json::Value>,
    ) -> Request {
        let mut r = req_body(method, uri, LAN, true, None, body);
        let h = r.headers_mut();
        h.insert(header::HOST, HeaderValue::from_static(LAN_HOST));
        h.insert(header::ORIGIN, HeaderValue::from_static(LAN_ORIGIN));
        h.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
        h.insert("x-zapper", HeaderValue::from_static("1"));
        h.insert(
            header::USER_AGENT,
            HeaderValue::from_static("Mozilla/5.0 (X11; Linux x86_64) Firefox/131.0"),
        );
        if let Some(c) = cookie {
            h.insert(
                header::COOKIE,
                HeaderValue::from_str(&format!("{}={c}", web::COOKIE)).unwrap(),
            );
        }
        r
    }

    async fn call_raw(s: &Shared, r: Request) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let resp = router(s.clone()).oneshot(r).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 24)
            .await
            .unwrap();
        (status, headers, bytes.to_vec())
    }

    /// Signs in as the web interface does; the cookie's token.
    async fn browser_login(s: &Shared) -> String {
        let (st, h, body) = call_raw(
            s,
            from_page(
                "POST",
                "/api/login",
                None,
                Some(serde_json::json!({"password": "correct horse", "cookie": true})),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(body.get("token").is_none(), "the page never sees the token: {body}");
        assert_eq!(body["device"]["name"], "Firefox on Linux");
        let set = h[header::SET_COOKIE].to_str().unwrap().to_string();
        set.strip_prefix(&format!("{}=", web::COOKIE))
            .and_then(|r| r.split(';').next())
            .unwrap()
            .to_string()
    }

    #[tokio::test]
    async fn a_browser_signs_in_for_a_cookie_its_page_cannot_read() {
        let s = server("cookie");
        enable(&s, true).await;
        let (st, h, body) = call_raw(
            &s,
            from_page(
                "POST",
                "/api/login",
                None,
                Some(serde_json::json!({"password": "correct horse", "cookie": true})),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);
        let set = h[header::SET_COOKIE].to_str().unwrap();
        for part in ["HttpOnly", "SameSite=Strict", "Path=/", "; Secure"] {
            assert!(set.contains(part), "{part} in {set}");
        }
        let body = String::from_utf8(body).unwrap();
        let token = set.split(';').next().unwrap().split_once('=').unwrap().1;
        assert!(!body.contains(token), "the token stays out of the answer");

        // Signed in: the session says which device, and the API answers.
        let (st, body) = call(&s, from_page("GET", "/api/session", Some(token), None)).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["device"]["name"], "Firefox on Linux");
        assert_eq!(
            call(&s, from_page("GET", "/api/recordings", Some(token), None))
                .await
                .0,
            StatusCode::OK
        );
        // The browser is listed like any other device.
        let (_, net) = call(&s, req("GET", "/api/network", HOME, false, Some(APP))).await;
        assert_eq!(net["devices"][0]["name"], "Firefox on Linux");
        // And, like any other device, cannot touch network access.
        assert_eq!(
            call(&s, from_page("GET", "/api/network", Some(token), None))
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        // A wrong one is no session at all, which the session says.
        let (st, body) = call(&s, from_page("GET", "/api/session", Some("nope"), None)).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body, serde_json::json!({ "signedIn": false }));
        assert_eq!(
            call(&s, from_page("GET", "/api/recordings", Some("nope"), None))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn the_cookie_is_secure_only_over_https() {
        let s = server("cookie-plain");
        s.access.set_password("correct horse").await.unwrap();
        // The app's own computer, plain HTTP on 127.0.0.1: a Secure cookie
        // would never come back.
        let mut r = req_body(
            "POST",
            "/api/login",
            HOME,
            false,
            None,
            Some(serde_json::json!({"password": "correct horse", "cookie": true})),
        );
        r.headers_mut().insert("x-zapper", HeaderValue::from_static("1"));
        let (st, h, _) = call_raw(&s, r).await;
        assert_eq!(st, StatusCode::CREATED);
        let set = h[header::SET_COOKIE].to_str().unwrap();
        assert!(set.contains("HttpOnly") && set.contains("SameSite=Strict"), "{set}");
        assert!(!set.contains("Secure"), "{set}");
    }

    #[tokio::test]
    async fn a_cookie_sign_in_must_come_from_the_page() {
        let s = server("cookie-csrf");
        enable(&s, true).await;
        let body = serde_json::json!({"password": "correct horse", "cookie": true});
        let mut r = from_page("POST", "/api/login", None, Some(body.clone()));
        r.headers_mut().remove("x-zapper");
        let (st, h, _) = call_raw(&s, r).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        assert!(h.get(header::SET_COOKIE).is_none());
        let mut r = from_page("POST", "/api/login", None, Some(body));
        r.headers_mut()
            .insert(header::ORIGIN, HeaderValue::from_static("https://evil.example"));
        assert_eq!(call(&s, r).await.0, StatusCode::FORBIDDEN);
        assert!(s.access.devices(|d| d.is_empty()), "nobody signed in");
        // A program signing in for a token is as it was.
        let r = req_body(
            "POST",
            "/api/login",
            LAN,
            true,
            None,
            Some(serde_json::json!({"password": "correct horse"})),
        );
        let (st, body) = call(&s, r).await;
        assert_eq!(st, StatusCode::CREATED);
        assert!(body["token"].is_string());
    }

    #[tokio::test]
    async fn changes_by_cookie_need_the_header_and_this_origin() {
        let s = server("csrf");
        enable(&s, true).await;
        let token = browser_login(&s).await;
        let soon = now() + 3600;
        let schedule = || {
            Some(serde_json::json!({
                "streamId": 7, "title": "Evening News", "start": soon, "stop": soon + 1800,
                "channelName": "One"
            }))
        };
        // No header: refused, and nothing scheduled.
        let mut r = from_page("POST", "/api/recordings", Some(&token), schedule());
        r.headers_mut().remove("x-zapper");
        let (st, body) = call(&s, r).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
        // From another site, with the header.
        let mut r = from_page("POST", "/api/recordings", Some(&token), schedule());
        r.headers_mut()
            .insert(header::ORIGIN, HeaderValue::from_static("https://evil.example"));
        assert_eq!(call(&s, r).await.0, StatusCode::FORBIDDEN);
        let mut r = from_page("POST", "/api/recordings", Some(&token), schedule());
        r.headers_mut()
            .insert("sec-fetch-site", HeaderValue::from_static("cross-site"));
        assert_eq!(call(&s, r).await.0, StatusCode::FORBIDDEN);
        assert!(s.list().unwrap().is_empty());

        // From the page: scheduled, then cancelled.
        let (st, rec) = call(&s, from_page("POST", "/api/recordings", Some(&token), schedule())).await;
        assert_eq!(st, StatusCode::CREATED, "{rec}");
        let id = rec["id"].as_i64().unwrap();
        let mut r = from_page("POST", &format!("/api/recordings/{id}/cancel"), Some(&token), None);
        r.headers_mut().remove("x-zapper");
        assert_eq!(call(&s, r).await.0, StatusCode::FORBIDDEN);
        let (st, body) = call(
            &s,
            from_page("POST", &format!("/api/recordings/{id}/cancel"), Some(&token), None),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["cancelled"], true);
    }

    #[tokio::test]
    async fn a_backup_is_chosen_by_hand_through_the_page_and_checked() {
        use crate::catalog::db;
        let s = server("backup");
        enable(&s, true).await;
        // Its own channels, apart from the other tests' in the shared
        // catalogue: 94001 recorded, 94002 and 94003 on the same guide
        // channel (94003 measured better), 94004 elsewhere, 94005 a divider.
        db::install_test_db();
        db::with_db(|c| {
            for (id, name, epg, num) in [
                (94001, "Bay One", "bay.test", 1),
                (94002, "Bay One SD", "bay.test", 2),
                (94003, "Bay One HD", "bay.test", 3),
                (94004, "Cove", "cove.test", 4),
                (94005, "--- Divider ---", "none.test", 5),
            ] {
                c.execute(
                    "INSERT INTO channels (stream_id, category_id, name, clean_name, num, epg_channel_id)
                     VALUES (?1, 'bay', ?2, ?2, ?3, ?4)",
                    rusqlite::params![id, name, num, epg],
                )?;
            }
            c.execute("UPDATE channels SET is_separator = 1 WHERE stream_id = 94005", [])?;
            c.execute(
                &format!(
                    "INSERT INTO channel_quality (origin, stream_id, height) VALUES ({}, 94003, 1080)",
                    crate::catalog::origin::current_origin_sql!()
                ),
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let token = browser_login(&s).await;
        let soon = now() + 3600;
        let (st, rec) = call(
            &s,
            from_page(
                "POST",
                "/api/recordings",
                Some(&token),
                Some(serde_json::json!({
                    "streamId": 94001, "title": "Bay Report", "start": soon, "stop": soon + 1800
                })),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{rec}");
        // Chosen when scheduled: the better of the two on its guide channel.
        assert_eq!(rec["backupStreamId"], 94003);
        assert_eq!(rec["backupName"], "Bay One HD");
        let id = rec["id"].as_i64().unwrap();

        let (st, b) = call(&s, from_page("GET", &format!("/api/recordings/{id}/backups"), Some(&token), None)).await;
        assert_eq!(st, StatusCode::OK, "{b}");
        assert_eq!(b["current"], 94003);
        let ids: Vec<i64> = b["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["streamId"].as_i64().unwrap())
            .collect();
        assert_eq!(ids, [94003, 94002]);
        assert_eq!(b["candidates"][0]["reason"], "guide");
        assert_eq!(b["candidates"][0]["height"], 1080);

        let put = |body: serde_json::Value| from_page("PUT", &format!("/api/recordings/{id}/backup"), Some(&token), Some(body));
        // Without the page's header, or from another site: refused.
        let mut r = put(serde_json::json!({"streamId": 94004}));
        r.headers_mut().remove("x-zapper");
        assert_eq!(call(&s, r).await.0, StatusCode::FORBIDDEN);
        let mut r = put(serde_json::json!({"streamId": 94004}));
        r.headers_mut()
            .insert(header::ORIGIN, HeaderValue::from_static("https://evil.example"));
        assert_eq!(call(&s, r).await.0, StatusCode::FORBIDDEN);
        // Without a token at all.
        let r = req_body(
            "PUT",
            &format!("/api/recordings/{id}/backup"),
            HOME,
            false,
            None,
            Some(serde_json::json!({"streamId": 94004})),
        );
        assert_eq!(call(&s, r).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(s.get(id).unwrap().backup_stream_id, Some(94003), "nothing changed");

        // Its own channel, a divider, a channel that is not there: refused.
        for (bad_id, words) in [
            (94001, "another channel"),
            (94005, "no channel 94005"),
            (99_999_999, "no channel 99999999"),
        ] {
            let (st, e) = call(&s, put(serde_json::json!({"streamId": bad_id}))).await;
            assert_eq!(st, StatusCode::BAD_REQUEST, "{bad_id}: {e}");
            assert!(e["error"].as_str().unwrap().contains(words), "{e}");
        }

        // Any listed channel will do, candidate or not; null is none.
        let (st, r) = call(&s, put(serde_json::json!({"streamId": 94004}))).await;
        assert_eq!(st, StatusCode::OK, "{r}");
        assert_eq!((r["backupStreamId"].as_i64(), r["backupName"].as_str()), (Some(94004), Some("Cove")));
        // The app, by its token, the same.
        let (st, r) = call(
            &s,
            req_body(
                "PUT",
                &format!("/api/recordings/{id}/backup"),
                HOME,
                false,
                Some(APP),
                Some(serde_json::json!({"streamId": null})),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{r}");
        assert!(r["backupStreamId"].is_null());
        assert!(r.get("backupName").is_none());

        // No such recording; a finished one.
        let (st, _) = call(&s, from_page("PUT", "/api/recordings/987654/backup", Some(&token), Some(serde_json::json!({"streamId": 94002})))).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        s.db(|c| store::set_state(c, id, RecordingState::Done, None))
            .unwrap();
        let (st, e) = call(&s, put(serde_json::json!({"streamId": 94002}))).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(e["error"].as_str().unwrap().contains("finished"), "{e}");
    }

    #[tokio::test]
    async fn a_stopped_recording_says_finishing_at_once_and_keeps_the_service_busy() {
        let s = server("finishing");
        let t = now();
        // One being captured: in the store as recording, with its capture
        // registered the way `spawn_capture` does.
        let id = s
            .db(|c| {
                let req = ScheduleRequest {
                    stream_id: 7,
                    backup_stream_id: None,
                    channel_name: Some("One".into()),
                    title: "Long Film".into(),
                    episode_title: None,
                    season: None,
                    episode: None,
                    start: t - 600,
                    stop: t + 3600,
                    pad_before: None,
                    pad_after: None,
                };
                let id = store::insert(
                    c,
                    &store::NewRecording {
                        req: &req,
                        backup_stream_id: None,
                        channel_name: "One",
                        pad_before: 60,
                        pad_after: 300,
                        folder: "/v",
                        base: "Long Film",
                        clock_track: false,
                        now: t,
                    },
                )?;
                store::set_state(c, id, RecordingState::Recording, None)?;
                Ok(id)
            })
            .unwrap();
        let (tx, mut stop) = tokio::sync::watch::channel(false);
        s.live(|m| {
            m.insert(
                id,
                crate::recording::server::Live {
                    title: "Long Film".into(),
                    stream_id: 7,
                    bytes: 5_000_000_000,
                    since: t - 600,
                    waiting: None,
                    retry_at: None,
                    until: t + 3900,
                    finishing: false,
                    stop: tx,
                },
            )
        });
        let get = |uri: &str| req("GET", uri, HOME, false, Some(APP));
        let (_, st) = call(&s, get("/api/status")).await;
        assert_eq!((st["idle"].as_bool(), st["active"][0].get("finishing")), (Some(false), None));

        // Stopping answers at once with what it is now: being finished.
        let (code, r) = call(&s, req("POST", &format!("/api/recordings/{id}/cancel"), HOME, false, Some(APP))).await;
        assert_eq!(code, StatusCode::OK, "{r}");
        assert_eq!(r["state"], "finishing");
        assert!(*stop.borrow_and_update(), "the capture was told to stop");
        let (_, list) = call(&s, get("/api/recordings")).await;
        assert_eq!(list[0]["state"], "finishing");
        // Busy while it is finished: an installer or a restart waits.
        let (_, st) = call(&s, get("/api/status")).await;
        assert_eq!(st["active"][0]["finishing"], true);
        assert_eq!(st["idle"], false);
        assert!(s.busy().idle_at.is_some());
        // Asking again changes nothing, and nothing can be deleted meanwhile.
        let (_, r) = call(&s, req("POST", &format!("/api/recordings/{id}/cancel"), HOME, false, Some(APP))).await;
        assert_eq!(r["state"], "finishing");
        assert!(s.delete(id, false).is_err());

        // Finished: done, and idle once the capture is gone.
        s.db(|c| store::set_state(c, id, RecordingState::Done, None)).unwrap();
        s.live(|m| m.remove(&id));
        let (_, st) = call(&s, get("/api/status")).await;
        assert_eq!(st["idle"], true);
        assert!(st["active"].as_array().unwrap().is_empty());
    }

    /// An account of one stream, which nothing else uses.
    struct OneStream;
    impl Budget for OneStream {
        fn check(
            &self,
            _own: usize,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Slots> + Send + '_>> {
            Box::pin(async { Slots::Free })
        }
        fn max(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<u32>> + Send + '_>> {
            Box::pin(async { Some(1) })
        }
    }

    #[tokio::test]
    async fn conflicts_on_another_channel_are_shown_before_and_after_scheduling() {
        let d = std::env::temp_dir().join(format!("zapper-http-{}-conflicts", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let cfg = ServerConfig::default();
        let path = d.join("server.json");
        config::save_to(&path, &cfg).unwrap();
        let s = Server::new(store::open_in_memory().unwrap(), cfg, path, APP.into(), Box::new(OneStream));
        // Padding of 1 min before and 5 after, the defaults.
        let eight = now() + 3 * 3600;
        let body = |stream: i64, title: &str, start: i64, stop: i64| {
            Some(serde_json::json!({
                "streamId": stream, "title": title, "start": start, "stop": stop,
                "channelName": format!("Channel {stream}"), "backupStreamId": stream
            }))
        };
        let post = |uri: &str, b| req_body("POST", uri, HOME, false, Some(APP), b);
        let (st, _) = call(&s, post("/api/recordings", body(7, "Film", eight, eight + 3600))).await;
        assert_eq!(st, StatusCode::CREATED);

        // The next programme on the same channel shares the connection: no
        // conflict.
        let (st, c) = call(
            &s,
            post("/api/recordings/preview", body(7, "Talk", eight + 3600, eight + 7200)),
        )
        .await;
        assert_eq!((st, c), (StatusCode::OK, serde_json::json!([])));

        // On another channel it would take the film's padding after, and
        // lose its own padding before.
        let (_, c) = call(
            &s,
            post("/api/recordings/preview", body(8, "Match", eight + 3600, eight + 7200)),
        )
        .await;
        let kinds: Vec<(&str, &str, &str, i64)> = c
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                (
                    c["title"].as_str().unwrap(),
                    c["kind"].as_str().unwrap(),
                    c["with"].as_str().unwrap(),
                    c["secs"].as_i64().unwrap(),
                )
            })
            .collect();
        assert!(kinds.contains(&("Film", "cutEnd", "Match", 300)), "{c}");
        assert!(kinds.contains(&("Match", "lateStart", "Film", 60)), "{c}");
        assert!(c.as_array().unwrap().iter().all(|c| c["streams"] == 1));

        // Scheduled, the list says the same on each row.
        let (st, m) = call(&s, post("/api/recordings", body(8, "Match", eight + 3600, eight + 7200))).await;
        assert_eq!(st, StatusCode::CREATED);
        assert_eq!(m["conflicts"][0]["kind"], "lateStart");
        let (_, list) = call(&s, req("GET", "/api/recordings", HOME, false, Some(APP))).await;
        let film = list.as_array().unwrap().iter().find(|r| r["title"] == "Film").unwrap();
        assert_eq!(film["conflicts"][0]["kind"], "cutEnd");
        assert_eq!(film["conflicts"][0]["with"], "Match");

        // Inside the film altogether: it cannot be recorded.
        let (_, c) = call(
            &s,
            post("/api/recordings/preview", body(9, "Short", eight + 600, eight + 1200)),
        )
        .await;
        assert_eq!(c[0]["title"], "Short");
        assert_eq!(c[0]["kind"], "missed");
    }

    #[tokio::test]
    async fn the_apps_token_is_no_cookie() {
        let s = server("app-cookie");
        let mut r = req("GET", "/api/status", HOME, false, None);
        r.headers_mut().insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("{}={APP}", web::COOKIE)).unwrap(),
        );
        assert_eq!(call(&s, r).await.0, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn signing_out_revokes_the_device_and_forgets_the_cookie() {
        let s = server("web-logout");
        enable(&s, true).await;
        let token = browser_login(&s).await;
        let (st, h, _) = call_raw(&s, from_page("POST", "/api/logout", Some(&token), None)).await;
        assert_eq!(st, StatusCode::OK);
        let set = h[header::SET_COOKIE].to_str().unwrap();
        assert!(set.starts_with("zapper_session=;") && set.contains("Max-Age=0"), "{set}");
        let (_, body) = call(&s, from_page("GET", "/api/session", Some(&token), None)).await;
        assert_eq!(body["signedIn"], false);
        assert_eq!(
            call(&s, from_page("GET", "/api/status", Some(&token), None))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert!(s.access.devices(|d| d.is_empty()));
    }

    #[tokio::test]
    async fn every_answer_carries_the_security_headers() {
        let s = server("headers");
        enable(&s, true).await;
        for (uri, want) in [
            ("/", StatusCode::OK),
            ("/api/version", StatusCode::OK),
            ("/api/status", StatusCode::UNAUTHORIZED),
        ] {
            let (st, h, _) = call_raw(&s, from_page("GET", uri, None, None)).await;
            assert_eq!(st, want, "{uri}");
            assert_eq!(h[header::CONTENT_SECURITY_POLICY], web::CSP, "{uri}");
            let csp = h[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
            assert!(csp.contains("script-src 'self'") && !csp.contains("unsafe"), "{csp}");
            assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
            assert_eq!(h[header::X_CONTENT_TYPE_OPTIONS], "nosniff", "{uri}");
            assert_eq!(h[header::REFERRER_POLICY], "no-referrer", "{uri}");
            assert!(h.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none());
        }
        // Even what the gate turns away.
        let (st, h, _) = call_raw(&s, req("GET", "/api/version", LAN, false, None)).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        assert_eq!(h[header::CONTENT_SECURITY_POLICY], web::CSP);
    }

    #[tokio::test]
    async fn the_interface_is_served_with_its_own_addresses_falling_back_to_it() {
        let s = server("files");
        enable(&s, true).await;
        let (st, h, index) = call_raw(&s, from_page("GET", "/", None, None)).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(h[header::CONTENT_TYPE], "text/html; charset=utf-8");
        assert_eq!(h[header::CACHE_CONTROL], "no-cache");
        let index = String::from_utf8(index).unwrap();
        if !web::built() {
            eprintln!("dist/ was not built: the interface's files were not checked");
            assert!(index.contains("npm run build"));
            return;
        }
        // An address of the interface's own is the interface.
        let (st, _, page) = call_raw(&s, from_page("GET", "/guide", None, None)).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(String::from_utf8(page).unwrap(), index);
        // No inline script: the policy would refuse it.
        assert!(!index.contains("<script>"), "{index}");
        assert!(!index.contains("<style>"), "{index}");
        // A hashed asset, kept for a year.
        let asset = index
            .split('"')
            .find(|p| p.starts_with("/assets/") && p.ends_with(".js"))
            .expect("index.html names its script")
            .to_string();
        let (st, h, js) = call_raw(&s, from_page("GET", &asset, None, None)).await;
        assert_eq!(st, StatusCode::OK);
        assert!(!js.is_empty());
        assert_eq!(h[header::CONTENT_TYPE], "text/javascript; charset=utf-8");
        assert_eq!(h[header::CACHE_CONTROL], "public, max-age=31536000, immutable");
        // A missing file is missing, not the page.
        assert_eq!(
            call_raw(&s, from_page("GET", "/assets/gone-1234.js", None, None))
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        let (st, body) = call(&s, from_page("GET", "/api/nothing-here", None, None)).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        assert!(body["error"].is_string());
        // Files are only read.
        let mut r = from_page("POST", "/guide", None, None);
        r.headers_mut().remove(header::COOKIE);
        assert_eq!(call(&s, r).await.0, StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn a_browser_on_plain_http_is_sent_to_https() {
        let s = server("redirect");
        enable(&s, true).await;
        let mut r = req("GET", "/guide", LAN, false, None);
        r.headers_mut()
            .insert(header::HOST, HeaderValue::from_static(LAN_HOST));
        let (st, h, _) = call_raw(&s, r).await;
        assert_eq!(st, StatusCode::PERMANENT_REDIRECT);
        assert_eq!(h[header::LOCATION], "https://192.168.1.20:8796/guide");
        // The API is refused, not redirected.
        let mut r = req("GET", "/api/version", LAN, false, None);
        r.headers_mut()
            .insert(header::HOST, HeaderValue::from_static(LAN_HOST));
        assert_eq!(call(&s, r).await.0, StatusCode::FORBIDDEN);
        // While network access is off, nothing at all.
        let mut c = s.config();
        c.network.enabled = false;
        s.replace_config(c);
        let mut r = req("GET", "/guide", LAN, false, None);
        r.headers_mut()
            .insert(header::HOST, HeaderValue::from_static(LAN_HOST));
        assert_eq!(call(&s, r).await.0, StatusCode::FORBIDDEN);
    }

    /// Two channels, `base + 1` and `base + 2`, in category `web-<base>`:
    /// each test its own, since the tests share one in-memory catalogue.
    fn seed_catalogue(base: i64, logo: &str) {
        use crate::catalog::db;
        db::install_test_db();
        db::with_db(|c| {
            c.execute(
                "INSERT INTO categories (id, name, sort) VALUES (?1, 'Web News', 0)",
                [format!("web-{base}")],
            )?;
            for (id, name) in [(base + 1, "Harbour One"), (base + 2, "Harbour Two")] {
                c.execute(
                    "INSERT INTO channels (stream_id, category_id, name, clean_name, num,
                                           epg_channel_id, logo_url)
                     VALUES (?1, ?2, ?3, ?3, ?1, ?4, ?5)",
                    rusqlite::params![id, format!("web-{base}"), name, format!("web-{id}"), logo],
                )?;
            }
            let t = now();
            c.execute(
                "INSERT INTO epg (channel_ref, start, stop, title, description)
                 VALUES (?1, ?2, ?3, 'Harbour Report', NULL)",
                rusqlite::params![format!("web-{}", base + 1), t - 600, t + 1200],
            )?;
            crate::catalog::guide_search::index_changes(c)?;
            Ok(())
        })
        .unwrap();
    }

    #[tokio::test]
    async fn the_catalogue_is_read_without_its_provider_addresses() {
        let s = server("browse");
        enable(&s, true).await;
        let secret_logo = "http://provider.example/u/secret-user/p/secret-pass/logo.png";
        seed_catalogue(91000, secret_logo);
        let token = browser_login(&s).await;
        let get = |uri: &str| from_page("GET", uri, Some(&token), None);

        let (st, cats) = call(&s, get("/api/categories")).await;
        assert_eq!(st, StatusCode::OK);
        assert!(cats
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == "web-91000" && c["channelCount"] == 2));

        let (st, page) = call(&s, get("/api/channels?category=web-91000&offset=0&limit=10")).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(page[0]["streamId"], 91001);
        assert_eq!(page[0]["logoUrl"], "/api/logo/91001");
        assert_eq!(page[0]["nowPlaying"]["title"], "Harbour Report");

        let (_, rows) = call(&s, get("/api/channels/by-id?ids=91002,91001")).await;
        assert_eq!(rows.as_array().unwrap().len(), 2);

        let (st, hits) = call(&s, get("/api/channels/search?q=harbour%20two&limit=5")).await;
        assert_eq!(st, StatusCode::OK);
        assert!(hits.as_array().unwrap().iter().any(|h| h["streamId"] == 91002));
        let (_, either) = call(&s, get("/api/channels/search?q=harbour%20one%7Charbour%20two&limit=5")).await;
        let found: Vec<i64> = either
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|h| h["streamId"].as_i64())
            .collect();
        assert!(found.contains(&91001) && found.contains(&91002), "{either}");

        let (st, hits) = call(&s, get("/api/programmes/search?q=harbour%20rep&offset=0&limit=10")).await;
        assert_eq!(st, StatusCode::OK);
        let hit = hits
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["channel"]["streamId"] == 91001)
            .expect("the programme is found");
        assert_eq!(hit["programme"]["title"], "Harbour Report");
        assert_eq!(hit["channel"]["logoUrl"], "/api/logo/91001");

        let t = now();
        let (st, rows) = call(
            &s,
            get(&format!("/api/guide/rows?ids=91001,91002&from={}&to={}", t - 3600, t + 3600)),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(rows[0]["programmes"][0]["title"], "Harbour Report");
        assert_eq!(rows[1]["logoUrl"], "/api/logo/91002");
        let (st, _) = call(&s, get(&format!("/api/guide?ids=91001&from={}&to={}", t - 3600, t + 3600))).await;
        assert_eq!(st, StatusCode::OK);
        let (st, at) = call(&s, get("/api/channels/91002/index?category=web-91000")).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(at, 1);
        let (st, _) = call(&s, get("/api/channels/numbers")).await;
        assert_eq!(st, StatusCode::OK);
        let (st, _) = call(&s, get("/api/favourites?limit=10")).await;
        assert_eq!(st, StatusCode::OK);
        let (st, _) = call(&s, get("/api/recent?limit=10")).await;
        assert_eq!(st, StatusCode::OK);
        let (st, sync) = call(&s, get("/api/sync")).await;
        assert_eq!(st, StatusCode::OK);
        assert!(sync["phase"].is_string());

        // Nothing of the provider's address in any of it.
        for uri in [
            "/api/channels?category=web-91000&offset=0&limit=10",
            "/api/channels/search?q=harbour&limit=5",
            "/api/programmes/search?q=harbour&offset=0&limit=10",
        ] {
            let (_, _, body) = call_raw(&s, get(uri)).await;
            assert!(!String::from_utf8(body).unwrap().contains("secret"), "{uri}");
        }

        // Not without signing in, and a bad request says why.
        assert_eq!(
            call(&s, from_page("GET", "/api/categories", None, None)).await.0,
            StatusCode::UNAUTHORIZED
        );
        let (st, body) = call(&s, get("/api/guide/rows?ids=1,x&from=0&to=1")).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(body["error"].as_str().unwrap().contains("ids"));
    }

    #[tokio::test]
    async fn a_logo_is_fetched_by_the_service_and_runs_nothing() {
        let provider = crate::test_http::serve(|r, _| {
            if r.route() == "/logo.png" {
                crate::test_http::Reply::ok(b"\x89PNG fake".to_vec()).header("Content-Type", "image/png")
            } else {
                crate::test_http::Reply::ok("<html>not an image</html>").header("Content-Type", "text/html")
            }
        });
        let s = server("logo");
        enable(&s, true).await;
        let token = browser_login(&s).await;
        seed_catalogue(92000, &format!("{}/logo.png", provider.base));
        let (st, h, body) = call_raw(&s, from_page("GET", "/api/logo/92001", Some(&token), None)).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(h[header::CONTENT_TYPE], "image/png");
        assert_eq!(body, b"\x89PNG fake");
        assert_eq!(h[header::CONTENT_SECURITY_POLICY], web::LOGO_CSP);
        assert!(h[header::CACHE_CONTROL].to_str().unwrap().contains("max-age"));
        assert_eq!(
            call_raw(&s, from_page("GET", "/api/logo/92001", None, None)).await.0,
            StatusCode::UNAUTHORIZED
        );

        // Something that is not an image is not served as one.
        crate::catalog::db::with_db(|c| {
            c.execute(
                "UPDATE channels SET logo_url = ?1 WHERE stream_id = 92002",
                [format!("{}/page", provider.base)],
            )?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            call_raw(&s, from_page("GET", "/api/logo/92002", Some(&token), None)).await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            call_raw(&s, from_page("GET", "/api/logo/1", Some(&token), None)).await.0,
            StatusCode::NOT_FOUND
        );
    }

    /// Movies 9_100_001.. in one category; the first has the poster at
    /// `poster`, and (in its kept details) one backdrop at `backdrop`. The
    /// second has kept details of a playlist's kind: nothing but a record.
    fn seed_movies(poster: &str, backdrop: &str) {
        use crate::catalog::{db, vod_store, vod_sync};
        db::install_test_db();
        let movie = |id: i64, name: &str, poster: Option<&str>| vod_sync::NewMovie {
            movie_id: id,
            category_id: Some("vod-9100".into()),
            name: name.into(),
            poster_url: poster.map(str::to_owned),
            ..Default::default()
        };
        db::with_db(|c| {
            vod_sync::land_movies(
                c,
                &[crate::xtream::Category {
                    category_id: "vod-9100".into(),
                    category_name: "Web Films".into(),
                }],
                &[
                    movie(9_100_001, "Harbour Lights", Some(poster)),
                    movie(9_100_002, "Harbour Dusk", None),
                ],
                now(),
            )?;
            let info = serde_json::json!({ "backdrops": [backdrop], "genres": ["Drama"] });
            vod_store::store_details(c, 9_100_001, now(), &info.to_string(), None)?;
            vod_store::store_details(c, 9_100_002, now(), "{}", None)?;
            Ok(())
        })
        .unwrap();
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn the_movie_catalogue_is_read_and_marked_without_provider_addresses() {
        let _catalogue = movie_catalogue();
        let s = server("vod");
        enable(&s, true).await;
        let poster = "http://provider.example/u/secret-user/p/secret-pass/poster.jpg";
        let backdrop = "http://provider.example/u/secret-user/p/secret-pass/back.jpg";
        seed_movies(poster, backdrop);
        let token = browser_login(&s).await;
        let get = |uri: &str| from_page("GET", uri, Some(&token), None);
        let put = |uri: &str, body: serde_json::Value| from_page("PUT", uri, Some(&token), Some(body));

        let (st, sum) = call(&s, get("/api/vod/summary")).await;
        assert_eq!(st, StatusCode::OK);
        assert!(sum["total"].as_u64().unwrap() >= 2, "{sum}");
        // Which catalogue the movies are from, as a hash the page compares.
        assert!(sum["origin"].is_string(), "{sum}");
        let (_, cats) = call(&s, get("/api/vod/categories")).await;
        assert!(cats.as_array().unwrap().iter().any(|c| c["id"] == "vod-9100"));

        let page_uri = "/api/vod/movies?list=category&category=vod-9100&sort=az&offset=0&limit=10";
        let (st, page) = call(&s, get(page_uri)).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(page["total"], 2);
        let posters: Vec<&str> = page["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["posterUrl"].as_str())
            .collect();
        assert_eq!(posters, ["/api/poster/movie/9100001"]);
        assert_eq!(
            call(&s, get("/api/vod/movies?list=category&limit=10")).await.0,
            StatusCode::BAD_REQUEST
        );

        let (_, hits) = call(&s, get("/api/vod/movies/search?q=harbour%20lights&limit=5")).await;
        assert_eq!(hits[0]["movieId"], 9_100_001);
        let (_, like) = call(&s, get("/api/vod/movies/9100001/similar?limit=5")).await;
        assert!(like.is_array());

        // Opened, a movie's pictures are this service's; a playlist's has
        // nothing more than the list said.
        let (st, d) = call(&s, get("/api/vod/movies/9100001")).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(d["backdropUrls"][0], "/api/backdrop/movie/9100001/0");
        assert_eq!(d["movie"]["posterUrl"], "/api/poster/movie/9100001");
        assert_eq!(d["genres"][0], "Drama");
        let (st, bare) = call(&s, get("/api/vod/movies/9100002")).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(bare["genres"], serde_json::json!([]));
        assert_eq!(
            call(&s, get("/api/vod/movies/9199999")).await.0,
            StatusCode::NOT_FOUND
        );

        let (st, m) = call(&s, put("/api/vod/movies/9100001/marks", serde_json::json!({"favourite": true}))).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(m["favourite"], true);
        let (_, favs) = call(&s, get("/api/vod/movies?list=favourites&limit=1000")).await;
        assert!(favs["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["movieId"] == 9_100_001));
        let (st, m) = call(
            &s,
            put("/api/vod/movies/9100002/progress", serde_json::json!({"position": 95, "duration": 100})),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(m["watched"], true);
        assert_eq!(
            call(&s, put("/api/vod/movies/9199999/marks", serde_json::json!({"watched": true}))).await.0,
            StatusCode::NOT_FOUND
        );

        // Nothing of the provider's address in any answer.
        for uri in [
            page_uri,
            "/api/vod/movies/search?q=harbour&limit=5",
            "/api/vod/movies/9100001",
            "/api/vod/movies/9100002/similar?limit=5",
            "/api/vod/movies?list=favourites&limit=10",
        ] {
            let (_, _, body) = call_raw(&s, get(uri)).await;
            let text = String::from_utf8(body).unwrap();
            assert!(!text.contains("provider.example") && !text.contains("secret"), "{uri}: {text}");
        }

        // Not without signing in.
        for uri in ["/api/vod/summary", "/api/poster/movie/9100001", "/api/backdrop/movie/9100001/0"] {
            assert_eq!(
                call(&s, from_page("GET", uri, None, None)).await.0,
                StatusCode::UNAUTHORIZED,
                "{uri}"
            );
        }
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn posters_and_backdrops_are_fetched_by_the_service() {
        let _catalogue = movie_catalogue();
        let provider = crate::test_http::serve(|r, _| match r.route() {
            "/poster.jpg" | "/back.jpg" => {
                crate::test_http::Reply::ok(b"\xff\xd8 fake".to_vec()).header("Content-Type", "image/jpeg")
            }
            _ => crate::test_http::Reply::ok("<html>no</html>").header("Content-Type", "text/html"),
        });
        let s = server("vod-images");
        enable(&s, true).await;
        let token = browser_login(&s).await;
        seed_movies(
            &format!("{}/poster.jpg", provider.base),
            &format!("{}/back.jpg", provider.base),
        );
        for uri in ["/api/poster/movie/9100001", "/api/backdrop/movie/9100001/0"] {
            let (st, h, body) = call_raw(&s, from_page("GET", uri, Some(&token), None)).await;
            assert_eq!(st, StatusCode::OK, "{uri}");
            assert_eq!(h[header::CONTENT_TYPE], "image/jpeg");
            assert_eq!(h[header::CONTENT_SECURITY_POLICY], web::LOGO_CSP);
            assert_eq!(h[header::CACHE_CONTROL], "private, max-age=86400");
            assert_eq!(body, b"\xff\xd8 fake");
        }
        // No picture, no such backdrop, no such movie: a 404 kept for an hour.
        for uri in [
            "/api/poster/movie/9100002",
            "/api/backdrop/movie/9100001/1",
            "/api/poster/movie/9199999",
        ] {
            let (st, h, _) = call_raw(&s, from_page("GET", uri, Some(&token), None)).await;
            assert_eq!(st, StatusCode::NOT_FOUND, "{uri}");
            assert_eq!(h[header::CACHE_CONTROL], "private, max-age=3600");
        }
    }

    #[test]
    fn a_failed_details_fetch_does_not_tell_the_page_where_the_provider_is() {
        use crate::catalog::{db, vod_sync};
        let _catalogue = movie_catalogue();
        let provider = crate::test_http::serve(|_, _| crate::test_http::Reply::new(500, "nope"));
        with_account("vodfail", async {
            crate::catalog::credentials::save(&crate::xtream::Credentials {
                host: provider.base.clone(),
                username: "viewer".into(),
                password: "vod-secret-41".into(),
            })
            .unwrap();
            db::install_test_db();
            db::with_db(|c| {
                vod_sync::land_movies(
                    c,
                    &[crate::xtream::Category {
                        category_id: "vod-9101".into(),
                        category_name: "Web Films Two".into(),
                    }],
                    &[vod_sync::NewMovie {
                        movie_id: 9_101_001,
                        category_id: Some("vod-9101".into()),
                        name: "Harbour Fog".into(),
                        ..Default::default()
                    }],
                    now(),
                )?;
                Ok(())
            })
            .unwrap();
            let s = server("vod-fail");
            enable(&s, true).await;
            let token = browser_login(&s).await;
            let get = |uri: &str| from_page("GET", uri, Some(&token), None);
            let (st, _, body) = call_raw(&s, get("/api/vod/movies/9101001")).await;
            assert_eq!(st, StatusCode::BAD_GATEWAY);
            let text = String::from_utf8(body).unwrap();
            for leak in [provider.base.as_str(), "127.0.0.1", "player_api", "vod-secret-41"] {
                assert!(!text.contains(leak), "{leak} reached the browser: {text}");
            }
            // An unknown id is a 404 for every read of one, and a bad duration is refused.
            let (st, _) = call(&s, get("/api/vod/movies/9199999/similar?limit=5")).await;
            assert_eq!(st, StatusCode::NOT_FOUND);
            let bad = serde_json::json!({"position": 5, "duration": 0});
            let (st, _) = call(
                &s,
                from_page("PUT", "/api/vod/movies/9101001/progress", Some(&token), Some(bad)),
            )
            .await;
            assert_eq!(st, StatusCode::BAD_REQUEST);
        });
    }

    /// The provider's own date for the shows below, which their kept
    /// details were fetched under (so they are fresh, and never asked for).
    const SHOWS_ADDED: i64 = 1_700_000_000;

    /// Shows 9_200_001.. in one category. The first has the poster at
    /// `poster`, kept details with one backdrop at `backdrop`, and two
    /// episodes (9_200_101, 9_200_102) with the still at `still`; the second
    /// has nothing more than its name.
    fn seed_series(poster: &str, backdrop: &str, still: &str) {
        use crate::catalog::{db, series_sync};
        db::install_test_db();
        let show = |id: i64, name: &str, poster: Option<&str>| series_sync::NewSeries {
            series_id: id,
            category_id: Some("ser-9200".into()),
            name: name.into(),
            poster_url: poster.map(str::to_owned),
            added: Some(SHOWS_ADDED),
            ..Default::default()
        };
        let episode = |id: i64, n: i32| series_sync::NewEpisode {
            episode_id: id,
            series_id: 9_200_001,
            season: 1,
            episode: n,
            title: Some(format!("Night Shift {n}")),
            still_url: Some(still.into()),
            container: Some("mkv".into()),
            ..Default::default()
        };
        db::with_db(|c| {
            series_sync::land_series(
                c,
                &[crate::xtream::Category {
                    category_id: "ser-9200".into(),
                    category_name: "Web Shows".into(),
                }],
                &[
                    show(9_200_001, "Lantern Bay", Some(poster)),
                    show(9_200_002, "Lantern Point", None),
                ],
                now(),
            )?;
            let info = serde_json::json!({
                "backdrops": [backdrop],
                "genres": ["Drama"],
                "modified": SHOWS_ADDED,
            });
            series_sync::land_series_details(
                c,
                9_200_001,
                now(),
                &info.to_string(),
                &[episode(9_200_101, 1), episode(9_200_102, 2)],
            )?;
            Ok(())
        })
        .unwrap();
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn the_series_catalogue_is_read_and_marked_without_provider_addresses() {
        let _catalogue = movie_catalogue();
        let s = server("series");
        enable(&s, true).await;
        let poster = "http://provider.example/u/secret-user/p/secret-pass/poster.jpg";
        let backdrop = "http://provider.example/u/secret-user/p/secret-pass/back.jpg";
        let still = "http://provider.example/u/secret-user/p/secret-pass/still.jpg";
        seed_series(poster, backdrop, still);
        let token = browser_login(&s).await;
        let get = |uri: &str| from_page("GET", uri, Some(&token), None);
        let put = |uri: &str, body: serde_json::Value| from_page("PUT", uri, Some(&token), Some(body));

        let (st, sum) = call(&s, get("/api/vod/shows/summary")).await;
        assert_eq!(st, StatusCode::OK);
        assert!(sum["total"].as_u64().unwrap() >= 2, "{sum}");
        assert!(sum["origin"].is_string(), "{sum}");
        let (st, cats) = call(&s, get("/api/vod/series/categories")).await;
        assert_eq!(st, StatusCode::OK);
        assert!(cats.as_array().unwrap().iter().any(|c| c["id"] == "ser-9200"));

        let page_uri = "/api/vod/series?list=category&category=ser-9200&sort=az&offset=0&limit=10";
        let (st, page) = call(&s, get(page_uri)).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(page["total"], 2);
        let posters: Vec<&str> = page["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["posterUrl"].as_str())
            .collect();
        assert_eq!(posters, ["/api/poster/series/9200001"]);
        assert!(posters.iter().all(|p| p.starts_with("/api/poster/series/") && !p.starts_with("http")));
        assert_eq!(
            call(&s, get("/api/vod/series?list=category&limit=10")).await.0,
            StatusCode::BAD_REQUEST
        );

        let (st, hits) = call(&s, get("/api/vod/series/search?q=lantern%20bay&limit=5")).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(hits[0]["seriesId"], 9_200_001);
        assert_eq!(hits[0]["posterUrl"], "/api/poster/series/9200001");

        // Opened, a show's pictures are this service's, every episode's too.
        let (st, d) = call(&s, get("/api/vod/series/9200001")).await;
        assert_eq!(st, StatusCode::OK, "{d}");
        assert_eq!(d["series"]["posterUrl"], "/api/poster/series/9200001");
        assert_eq!(d["backdropUrls"], serde_json::json!(["/api/backdrop/series/9200001/0"]));
        assert_eq!(d["genres"][0], "Drama");
        let eps = d["episodes"].as_array().unwrap();
        assert_eq!(eps.len(), 2);
        for e in eps {
            assert!(e["stillUrl"].as_str().unwrap().starts_with("/api/"), "{e}");
        }
        assert_eq!(eps[0]["stillUrl"], "/api/still/episode/9200101");
        assert_eq!(
            call(&s, get("/api/vod/series/9299999")).await.0,
            StatusCode::NOT_FOUND
        );

        // The episode playing, for the player's title.
        let (st, p) = call(&s, get("/api/vod/episodes/9200102")).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(p["series"]["posterUrl"], "/api/poster/series/9200001");
        assert_eq!(p["episode"]["stillUrl"], "/api/still/episode/9200102");
        assert_eq!(
            call(&s, get("/api/vod/episodes/9299999")).await.0,
            StatusCode::NOT_FOUND
        );

        let (st, m) = call(&s, put("/api/vod/series/9200001/marks", serde_json::json!({"favourite": true}))).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(m["favourite"], true);
        let (_, favs) = call(&s, get("/api/vod/series?list=favourites&limit=1000")).await;
        assert!(favs["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["seriesId"] == 9_200_001));
        assert_eq!(
            call(&s, put("/api/vod/series/9299999/marks", serde_json::json!({"favourite": true}))).await.0,
            StatusCode::NOT_FOUND
        );

        // Played past most of its length, an episode is watched; the show
        // is once every episode is.
        let (st, m) = call(
            &s,
            put("/api/vod/episodes/9200101/progress", serde_json::json!({"position": 95, "duration": 100})),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(m["watched"], true);
        let (_, d) = call(&s, get("/api/vod/series/9200001")).await;
        assert_eq!(d["series"]["watched"], false, "one episode is still to watch");
        let (st, m) = call(
            &s,
            put("/api/vod/episodes/9200102/progress", serde_json::json!({"position": 95, "duration": 100})),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(m["watched"], true);
        let (_, d) = call(&s, get("/api/vod/series/9200001")).await;
        assert_eq!(d["series"]["watched"], true, "{d}");
        let (st, m) = call(&s, put("/api/vod/episodes/9200102/marks", serde_json::json!({"watched": false}))).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(m["watched"], false);
        assert_eq!(
            call(&s, put("/api/vod/episodes/9200101/progress", serde_json::json!({"position": 5, "duration": 0}))).await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            call(&s, put("/api/vod/episodes/9299999/marks", serde_json::json!({"watched": true}))).await.0,
            StatusCode::NOT_FOUND
        );

        // Nothing of the provider's address in any answer.
        for uri in [
            page_uri,
            "/api/vod/series/search?q=lantern&limit=5",
            "/api/vod/series/9200001",
            "/api/vod/series?list=favourites&limit=10",
            "/api/vod/episodes/9200101",
        ] {
            let (_, _, body) = call_raw(&s, get(uri)).await;
            let text = String::from_utf8(body).unwrap();
            assert!(!text.contains("provider.example") && !text.contains("secret"), "{uri}: {text}");
        }

        // Not without signing in.
        for uri in [
            "/api/vod/shows/summary",
            "/api/vod/series/9200001",
            "/api/vod/episodes/9200101",
            "/api/poster/series/9200001",
            "/api/backdrop/series/9200001/0",
            "/api/still/episode/9200101",
        ] {
            assert_eq!(
                call(&s, from_page("GET", uri, None, None)).await.0,
                StatusCode::UNAUTHORIZED,
                "{uri}"
            );
        }
        let mark = serde_json::json!({"favourite": false});
        assert_eq!(
            call(&s, from_page("PUT", "/api/vod/series/9200001/marks", None, Some(mark))).await.0,
            StatusCode::UNAUTHORIZED
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn series_posters_backdrops_and_stills_are_fetched_by_the_service() {
        let _catalogue = movie_catalogue();
        let provider = crate::test_http::serve(|r, _| match r.route() {
            "/poster.jpg" | "/back.jpg" | "/still.jpg" => {
                crate::test_http::Reply::ok(b"\xff\xd8 fake".to_vec()).header("Content-Type", "image/jpeg")
            }
            _ => crate::test_http::Reply::ok("<html>no</html>").header("Content-Type", "text/html"),
        });
        let s = server("series-images");
        enable(&s, true).await;
        let token = browser_login(&s).await;
        seed_series(
            &format!("{}/poster.jpg", provider.base),
            &format!("{}/back.jpg", provider.base),
            &format!("{}/still.jpg", provider.base),
        );
        for uri in [
            "/api/poster/series/9200001",
            "/api/backdrop/series/9200001/0",
            "/api/still/episode/9200101",
        ] {
            let (st, h, body) = call_raw(&s, from_page("GET", uri, Some(&token), None)).await;
            assert_eq!(st, StatusCode::OK, "{uri}");
            assert_eq!(h[header::CONTENT_TYPE], "image/jpeg");
            assert_eq!(h[header::CONTENT_SECURITY_POLICY], web::LOGO_CSP);
            assert_eq!(h[header::CACHE_CONTROL], "private, max-age=86400");
            assert_eq!(body, b"\xff\xd8 fake");
        }
        // No picture, no such backdrop, no such show or episode.
        for uri in [
            "/api/poster/series/9200002",
            "/api/backdrop/series/9200001/1",
            "/api/backdrop/series/9200002/0",
            "/api/poster/series/9299999",
            "/api/still/episode/9299999",
        ] {
            let (st, h, _) = call_raw(&s, from_page("GET", uri, Some(&token), None)).await;
            assert_eq!(st, StatusCode::NOT_FOUND, "{uri}");
            assert_eq!(h[header::CACHE_CONTROL], "private, max-age=3600");
        }
    }

    #[test]
    fn a_failed_series_fetch_says_why_without_where_the_provider_is() {
        use crate::catalog::{db, origin, series_sync};
        let _catalogue = movie_catalogue();
        let provider = crate::test_http::serve(|_, _| crate::test_http::Reply::new(500, "nope"));
        with_account("seriesfail", async {
            let creds = crate::xtream::Credentials {
                host: provider.base.clone(),
                username: "viewer".into(),
                password: "series-secret-42".into(),
            };
            crate::catalog::credentials::save(&creds).unwrap();
            db::install_test_db();
            db::with_db(|c| {
                // The shows are this account's, so it is asked about them.
                origin::record(c, &origin::of_xtream(&creds))?;
                series_sync::land_series(
                    c,
                    &[crate::xtream::Category {
                        category_id: "ser-9201".into(),
                        category_name: "Web Shows Two".into(),
                    }],
                    &[series_sync::NewSeries {
                        series_id: 9_201_001,
                        category_id: Some("ser-9201".into()),
                        name: "Lantern Fog".into(),
                        ..Default::default()
                    }],
                    now(),
                )?;
                Ok(())
            })
            .unwrap();
            let s = server("series-fail");
            enable(&s, true).await;
            let token = browser_login(&s).await;
            let (st, _, body) =
                call_raw(&s, from_page("GET", "/api/vod/series/9201001", Some(&token), None)).await;
            assert_eq!(st, StatusCode::BAD_GATEWAY);
            let text = String::from_utf8(body).unwrap();
            assert_eq!(text, r#"{"error":"the show's episodes could not be fetched"}"#);
            for leak in [provider.base.as_str(), "127.0.0.1", "player_api", "series-secret-42", "viewer"] {
                assert!(!text.contains(leak), "{leak} reached the browser: {text}");
            }
            // Its ★, or unwatching it, asks nothing of the provider. (Marking
            // it watched reads the account on another thread, which this
            // test's account directory does not reach; `series_browse`'s
            // tests hold that a failure then is the provider's.)
            let put = |body: serde_json::Value| {
                from_page("PUT", "/api/vod/series/9201001/marks", Some(&token), Some(body))
            };
            for body in [serde_json::json!({"favourite": true}), serde_json::json!({"watched": false})] {
                let (st, _) = call(&s, put(body.clone())).await;
                assert_eq!(st, StatusCode::OK, "{body}");
            }
        });
    }

    // ------------------------------------------------------------- account

    /// Runs `f` on a current-thread runtime in a config directory of its
    /// own (`paths::with_test_root` is per thread, so every await must stay
    /// on this one), with an account file no other test can see.
    fn with_account<T>(name: &str, f: impl std::future::Future<Output = T>) -> T {
        let dir = std::env::temp_dir().join(format!(
            "zapper-http-account-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = crate::paths::with_test_root(&dir, || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(f)
        });
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    /// An account file with a secret in every kind that has one.
    fn save_every_kind() {
        use crate::catalog::credentials;
        credentials::save_tvheadend(&crate::tvheadend::TvhAccount {
            url: "http://tvh.invalid:9981".into(),
            username: "viewer".into(),
            password: "tvh-secret-31".into(),
        })
        .unwrap();
        credentials::save_m3u(&crate::m3u::M3uAccount {
            playlist: "http://p.invalid/get.php?username=viewer&password=m3u-secret-32".into(),
            epg_urls: vec![
                "http://p.invalid/xmltv.php?username=viewer&password=m3u-secret-32".into(),
            ],
            user_agent: None,
        })
        .unwrap();
        credentials::save(&crate::xtream::Credentials {
            host: "http://saved.invalid".into(),
            username: "viewer".into(),
            password: "saved-secret-33".into(),
        })
        .unwrap();
    }

    const SECRETS: [&str; 3] = ["tvh-secret-31", "m3u-secret-32", "saved-secret-33"];

    fn on_file() -> crate::catalog::credentials::OnFile {
        crate::catalog::credentials::on_file()
    }

    #[test]
    fn a_device_sees_the_account_without_its_secrets() {
        with_account("view", async {
            save_every_kind();
            let s = server("acct-view");
            enable(&s, true).await;
            let token = browser_login(&s).await;
            let (st, _, body) =
                call_raw(&s, from_page("GET", "/api/account", Some(&token), None)).await;
            assert_eq!(st, StatusCode::OK);
            let text = String::from_utf8(body).unwrap();
            for secret in SECRETS {
                assert!(!text.contains(secret), "{secret} reached the browser: {text}");
            }
            let a: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(a["kind"], "xtream");
            assert_eq!(a["host"], "http://saved.invalid");
            assert_eq!(a["hasPassword"], true);
            assert_eq!(a["tvheadendHasPassword"], true);
            assert_eq!(a["playlistRedacted"], true);
            assert_eq!(a["epgRedacted"], true);
            // A path on the service's computer is not the browser's business.
            assert_eq!(a["path"], "");
            // Signed out, nothing.
            assert_eq!(
                call_raw(&s, from_page("GET", "/api/account", None, None)).await.0,
                StatusCode::UNAUTHORIZED
            );
            // The app may ask too.
            let (st, a) = call(&s, req("GET", "/api/account", HOME, false, Some(APP))).await;
            assert_eq!(st, StatusCode::OK);
            assert_eq!(a["username"], "viewer");
        });
    }

    #[test]
    fn a_device_saves_and_a_blank_password_keeps_the_saved_one() {
        with_account("save", async {
            save_every_kind();
            let s = server("acct-save");
            enable(&s, true).await;
            let token = browser_login(&s).await;
            let form = serde_json::json!({
                "kind": "xtream", "host": " http://new.invalid ", "username": "viewer",
                "password": ""
            });
            let (st, _, body) =
                call_raw(&s, from_page("PUT", "/api/account", Some(&token), Some(form))).await;
            assert_eq!(st, StatusCode::OK);
            let text = String::from_utf8(body).unwrap();
            assert!(!SECRETS.iter().any(|x| text.contains(x)), "{text}");
            let saved = on_file().xtream.unwrap();
            assert_eq!(saved.host, "http://new.invalid");
            assert_eq!(saved.password, "saved-secret-33");

            // Switching kinds: a playlist sent back as shown (null) is kept,
            // and becomes the account in use; the Xtream login stays on file.
            let form = serde_json::json!({
                "kind": "m3u", "playlist": null, "epgUrls": null, "userAgent": ""
            });
            let (st, a) = call(&s, from_page("PUT", "/api/account", Some(&token), Some(form))).await;
            assert_eq!(st, StatusCode::OK, "{a}");
            assert_eq!(a["kind"], "m3u");
            let f = on_file();
            assert_eq!(f.kind, crate::catalog::credentials::Kind::M3u);
            assert!(f.m3u.unwrap().playlist.ends_with("password=m3u-secret-32"));
            assert_eq!(f.xtream.unwrap().password, "saved-secret-33");

            // A Tvheadend password typed in replaces the saved one.
            let form = serde_json::json!({
                "kind": "tvheadend", "url": "http://tvh.invalid:9981", "username": "viewer",
                "password": "typed-secret-34"
            });
            let (st, _) = call(&s, from_page("PUT", "/api/account", Some(&token), Some(form))).await;
            assert_eq!(st, StatusCode::OK);
            assert_eq!(on_file().tvheadend.unwrap().password, "typed-secret-34");

            // An HDHomeRun, which has no secret at all.
            let form = serde_json::json!({"kind": "hdhomerun", "device": "10.0.0.7", "epgUrls": []});
            let (st, a) = call(&s, from_page("PUT", "/api/account", Some(&token), Some(form))).await;
            assert_eq!(st, StatusCode::OK);
            assert_eq!(a["hdhomerunDevice"], "10.0.0.7");
        });
    }

    #[test]
    fn account_changes_by_cookie_need_the_header_and_this_origin() {
        with_account("csrf", async {
            save_every_kind();
            let s = server("acct-csrf");
            enable(&s, true).await;
            let token = browser_login(&s).await;
            let form = || {
                Some(serde_json::json!({
                    "kind": "xtream", "host": "http://evil.invalid", "username": "x",
                    "password": "evil-secret"
                }))
            };
            let mut r = from_page("PUT", "/api/account", Some(&token), form());
            r.headers_mut().remove("x-zapper");
            assert_eq!(call(&s, r).await.0, StatusCode::FORBIDDEN);
            let mut r = from_page("PUT", "/api/account", Some(&token), form());
            r.headers_mut()
                .insert(header::ORIGIN, HeaderValue::from_static("https://evil.example"));
            assert_eq!(call(&s, r).await.0, StatusCode::FORBIDDEN);
            let mut r = from_page("POST", "/api/account/test", Some(&token), form());
            r.headers_mut()
                .insert("sec-fetch-site", HeaderValue::from_static("cross-site"));
            assert_eq!(call(&s, r).await.0, StatusCode::FORBIDDEN);
            assert_eq!(on_file().xtream.unwrap().host, "http://saved.invalid");
        });
    }

    #[test]
    fn a_test_writes_nothing_and_never_repeats_the_password() {
        use crate::test_http::{serve, Reply};
        with_account("test", async {
            save_every_kind();
            let before = std::fs::read(crate::catalog::credentials::account_path()).unwrap();
            let s = server("acct-test");
            enable(&s, true).await;
            let token = browser_login(&s).await;
            // A server that knows the saved password, and fails on demand.
            let provider = serve(|r, _| {
                if r.route() != "/player_api.php" {
                    return Reply::new(404, "");
                }
                if r.query("username") == Some("broken") {
                    return Reply::new(500, "");
                }
                let ok = r.query("password") == Some("saved-secret-33");
                Reply::ok(if ok {
                    r#"{"user_info":{"auth":1,"status":"Active"}}"#
                } else {
                    r#"{"user_info":{"auth":0}}"#
                })
                .header("Content-Type", "application/json")
            });
            let form = |user: &str, password: &str| {
                Some(serde_json::json!({
                    "kind": "xtream", "host": provider.base, "username": user,
                    "password": password
                }))
            };
            // Blank: the saved password is the one tried.
            let r = from_page("POST", "/api/account/test", Some(&token), form("viewer", ""));
            let (st, body) = call(&s, r).await;
            assert_eq!(st, StatusCode::OK, "{body}");
            assert_eq!(body["result"], "Active");
            // Typed and wrong: refused, without the typed password.
            let r = from_page(
                "POST",
                "/api/account/test",
                Some(&token),
                form("viewer", "typed-secret-35"),
            );
            let (st, body) = call(&s, r).await;
            assert_eq!(st, StatusCode::BAD_REQUEST);
            assert!(body["error"].as_str().unwrap().contains("rejected"), "{body}");
            // A failing server: reqwest quotes the URL, and the URL has it.
            let r = from_page(
                "POST",
                "/api/account/test",
                Some(&token),
                form("broken", "typed-secret-35"),
            );
            let (st, body) = call(&s, r).await;
            assert_eq!(st, StatusCode::BAD_REQUEST);
            assert!(!body.to_string().contains("typed-secret-35"), "{body}");
            assert!(body["error"].as_str().unwrap().contains("500"), "{body}");
            // A form that does not parse is not quoted back either.
            let garbled = serde_json::json!({"kind": "xtream", "password": ["typed-secret-35"]});
            let r = from_page("POST", "/api/account/test", Some(&token), Some(garbled));
            let (st, body) = call(&s, r).await;
            assert_eq!(st, StatusCode::BAD_REQUEST);
            assert!(!body.to_string().contains("typed-secret-35"), "{body}");
            let garbled = serde_json::json!({"kind": "typed-secret-35"});
            let r = from_page("PUT", "/api/account", Some(&token), Some(garbled));
            let (st, body) = call(&s, r).await;
            assert_eq!(st, StatusCode::BAD_REQUEST);
            assert!(!body.to_string().contains("typed-secret-35"), "{body}");
            // None of that wrote anything.
            let after = std::fs::read(crate::catalog::credentials::account_path()).unwrap();
            assert_eq!(before, after);
            // No body: the account on file, which points nowhere real here.
            let r = from_page("POST", "/api/account/test", Some(&token), None);
            let (st, body) = call(&s, r).await;
            assert_eq!(st, StatusCode::BAD_REQUEST);
            assert!(!body.to_string().contains("saved-secret-33"), "{body}");
        });
    }

    #[test]
    fn a_save_that_fails_says_why_without_the_password() {
        with_account("save-bad", async {
            let s = server("acct-save-bad");
            enable(&s, true).await;
            let token = browser_login(&s).await;
            let form = serde_json::json!({
                "kind": "tvheadend", "url": "http://typed-secret-36@tvh.invalid",
                "username": "viewer", "password": "typed-secret-36"
            });
            let r = from_page("PUT", "/api/account", Some(&token), Some(form));
            let (st, body) = call(&s, r).await;
            assert_eq!(st, StatusCode::BAD_REQUEST);
            assert!(!body.to_string().contains("typed-secret-36"), "{body}");
            assert!(!crate::catalog::credentials::account_path().exists());
        });
    }

    // ------------------------------------------------------------ watching

    /// An account whose connections are all in use, as Xtream counts them.
    struct Full;
    impl Budget for Full {
        fn check(
            &self,
            _own: usize,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Slots> + Send + '_>> {
            Box::pin(async { Slots::Full { active: 1, max: 1 } })
        }
    }

    fn server_with(name: &str, budget: Box<dyn Budget>) -> Shared {
        let d = std::env::temp_dir().join(format!("zapper-http-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let cfg = ServerConfig::default();
        let path = d.join("server.json");
        config::save_to(&path, &cfg).unwrap();
        Server::new(store::open_in_memory().unwrap(), cfg, path, APP.into(), budget)
    }

    /// A live feed on 127.0.0.1 that repeats its tables (H.264 and AAC on
    /// `/h264.ts`, MPEG-2 and MP2 on `/mpeg2.ts`) and padding for as long as
    /// the client stays. Counts the connections it took.
    async fn feed_server() -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        use crate::recording::tsprobe::tests::{pat, pmt};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let count = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&count);
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = l.accept().await {
                c.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut req = [0u8; 2048];
                    let n = sock.read(&mut req).await.unwrap_or(0);
                    let mpeg2 = String::from_utf8_lossy(&req[..n]).contains("/mpeg2.ts");
                    let tables = if mpeg2 {
                        pmt(0x100, &[(0x02, 0x101, vec![]), (0x04, 0x102, vec![])])
                    } else {
                        pmt(0x100, &[(0x1b, 0x101, vec![]), (0x0f, 0x102, vec![])])
                    };
                    let head =
                        "HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nConnection: close\r\n\r\n";
                    if sock.write_all(head.as_bytes()).await.is_err() {
                        return;
                    }
                    let mut null = vec![0xffu8; 188];
                    null[..4].copy_from_slice(&[0x47, 0x1f, 0xff, 0x10]);
                    loop {
                        let mut tick = pat(0x100);
                        tick.extend_from_slice(&tables);
                        for _ in 0..40 {
                            tick.extend_from_slice(&null);
                        }
                        if sock.write_all(&tick).await.is_err() {
                            return;
                        }
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                });
            }
        });
        (base, count)
    }

    /// Channel 66 is MPEG-2; every other one H.264.
    fn feeds_from(s: &Shared, base: &str) {
        let base = base.to_string();
        s.set_playables(Arc::new(move |id| {
            let path = if id == 66 { "mpeg2.ts" } else { "h264.ts" };
            Ok(crate::source::Playable::plain(format!("{base}/{path}")))
        }));
    }

    fn watch_body(stream_id: i64, anyway: bool) -> Option<serde_json::Value> {
        Some(serde_json::json!({"streamId": stream_id, "anyway": anyway}))
    }

    /// The first `n` bytes of a live answer, without waiting for its end.
    async fn read_some(s: &Shared, r: Request, n: usize) -> (StatusCode, Vec<u8>) {
        use futures_core::Stream;
        let resp = router(s.clone()).oneshot(r).await.unwrap();
        let status = resp.status();
        let mut body = resp.into_body().into_data_stream();
        let mut got = Vec::new();
        while got.len() < n {
            let next = tokio::time::timeout(
                Duration::from_secs(10),
                std::future::poll_fn(|cx| std::pin::Pin::new(&mut body).poll_next(cx)),
            )
            .await
            .expect("stream data in time");
            match next {
                Some(Ok(b)) => got.extend_from_slice(&b),
                _ => break,
            }
        }
        (status, got)
    }

    async fn until(what: &str, mut f: impl FnMut() -> bool) {
        let t = std::time::Instant::now();
        while !f() {
            assert!(t.elapsed() < Duration::from_secs(10), "{what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn viewer_id(body: &serde_json::Value) -> String {
        body["viewer"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn watching_needs_a_sign_in_and_a_stream_is_its_owners_alone() {
        let s = server("watch-auth");
        enable(&s, true).await;
        let (base, _) = feed_server().await;
        feeds_from(&s, &base);
        let (st, _) = call(&s, from_page("POST", "/api/watch", None, watch_body(5, false))).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        let (st, _) = call(&s, from_page("GET", "/api/stream/abc", None, None)).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);

        // Starting one changes something: the page's header is wanted.
        let cookie = browser_login(&s).await;
        let mut r = from_page("POST", "/api/watch", Some(&cookie), watch_body(5, false));
        r.headers_mut().remove("x-zapper");
        assert_eq!(call(&s, r).await.0, StatusCode::FORBIDDEN);

        let (st, body) =
            call(&s, from_page("POST", "/api/watch", Some(&cookie), watch_body(5, false))).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        let stream = body["stream"].as_str().unwrap().to_string();
        assert!(!body.to_string().contains("127.0.0.1"), "no provider address: {body}");
        assert_eq!(body["video"], "H.264");
        assert_eq!(body["audio"][0]["codec"], "aac");
        assert_eq!(body["converting"], false);

        // Another device (the app's token) does not get this one's stream.
        let (st, _) = call(&s, req("GET", &stream, HOME, false, Some(APP))).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        // Nor does a page on another site make the browser fetch it.
        let mut r = from_page("GET", &stream, Some(&cookie), None);
        r.headers_mut()
            .insert("sec-fetch-site", HeaderValue::from_static("cross-site"));
        assert_eq!(call(&s, r).await.0, StatusCode::FORBIDDEN);

        // Its own page reads it: a transport stream, from a sync byte.
        let (st, bytes) =
            read_some(&s, from_page("GET", &stream, Some(&cookie), None), 188 * 50).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(bytes[0], 0x47);
        assert_eq!(bytes[188], 0x47);
    }

    #[tokio::test]
    async fn a_viewer_rides_a_recordings_connection_and_costs_no_other() {
        let s = server_with("watch-share", Box::new(Full));
        enable(&s, true).await;
        let (base, connections) = feed_server().await;
        feeds_from(&s, &base);
        // A recording of channel 7 has the account's only connection.
        let rec = s.hubs.open(
            7,
            crate::source::Playable::plain(format!("{base}/h264.ts")),
            crate::recording::sharing::Rider {
                id: 1,
                stop: now() + 3600,
                until: now() + 3900,
            },
        );
        let cookie = browser_login(&s).await;

        // Watching what is recorded joins its connection.
        let (st, body) =
            call(&s, from_page("POST", "/api/watch", Some(&cookie), watch_body(7, false))).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["shared"], true);
        let stream = body["stream"].as_str().unwrap().to_string();
        let (st, bytes) =
            read_some(&s, from_page("GET", &stream, Some(&cookie), None), 188 * 20).await;
        assert_eq!(st, StatusCode::OK);
        assert!(bytes.len() >= 188 * 20);
        assert_eq!(s.hubs.open_count(), 1);
        assert_eq!(s.hubs.conns()[0].riders.len(), 2, "the recording and the viewer");
        assert_eq!(
            connections.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "one upstream connection for both"
        );

        // Another channel: the account is full and a recording holds it, so
        // even "anyway" is refused, and the channel that could be watched
        // instead is named.
        for anyway in [false, true] {
            let (st, body) =
                call(&s, from_page("POST", "/api/watch", Some(&cookie), watch_body(9, anyway))).await;
            assert_eq!(st, StatusCode::CONFLICT, "{body}");
            assert_eq!(body["reason"], "recording");
            assert_eq!(body["anyway"], false);
            assert_eq!(body["watchInstead"], serde_json::json!([7]));
        }
        assert_eq!(connections.load(std::sync::atomic::Ordering::SeqCst), 1);

        // The viewer leaves: the recording carries on alone.
        let id = viewer_id(&body);
        let (st, _) =
            call(&s, from_page("DELETE", &format!("/api/watch/{id}"), Some(&cookie), None)).await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        until("the viewer leaves", || s.hubs.conns()[0].riders.len() == 1).await;
        assert_eq!(s.hubs.open_count(), 1);
        drop(rec);
        until("the connection closes", || s.hubs.open_count() == 0).await;
    }

    #[tokio::test]
    async fn a_viewer_alone_opens_a_connection_the_rules_never_close() {
        let s = server("watch-own");
        enable(&s, true).await;
        let (base, connections) = feed_server().await;
        feeds_from(&s, &base);
        let cookie = browser_login(&s).await;
        let (st, body) =
            call(&s, from_page("POST", "/api/watch", Some(&cookie), watch_body(5, false))).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["shared"], false);
        assert_eq!(s.hubs.open_count(), 1);
        let rider = s.hubs.conns()[0].riders[0];
        assert_eq!(rider.stop, i64::MAX, "a viewer, whose programme never ends");
        assert!(
            s.hubs.doom(5, now() + 365 * 86400).is_none(),
            "the padding rules never close a viewer's connection"
        );
        let id = viewer_id(&body);
        call(&s, from_page("DELETE", &format!("/api/watch/{id}"), Some(&cookie), None)).await;
        until("the connection closes", || s.hubs.open_count() == 0).await;
        assert_eq!(connections.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn another_device_on_a_full_account_is_said_and_anyway_is_offered() {
        let s = server_with("watch-full", Box::new(Full));
        enable(&s, true).await;
        let (base, connections) = feed_server().await;
        feeds_from(&s, &base);
        let cookie = browser_login(&s).await;
        let (st, body) =
            call(&s, from_page("POST", "/api/watch", Some(&cookie), watch_body(5, false))).await;
        assert_eq!(st, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["reason"], "inUse");
        assert_eq!(body["anyway"], true);
        assert_eq!(body["slots"]["max"], 1);
        assert_eq!(
            connections.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "nothing was opened"
        );
        // Pressed: it connects, as the app's player would.
        let (st, body) =
            call(&s, from_page("POST", "/api/watch", Some(&cookie), watch_body(5, true))).await;
        assert_eq!(st, StatusCode::OK, "{body}");

        // Another device is told a browser watches, and may take it over.
        let other = || {
            let mut r = req_body("POST", "/api/watch", HOME, false, Some(APP), watch_body(8, false));
            r.headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
            r
        };
        let (st, body) = call(&s, other()).await;
        assert_eq!(st, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["reason"], "viewers");
        assert_eq!(body["anyway"], true);

        // The same browser (a reloaded page) is not asked: its newer viewer
        // takes the connection from its older one.
        let (st, body) =
            call(&s, from_page("POST", "/api/watch", Some(&cookie), watch_body(6, false))).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        until("one connection again", || s.hubs.open_count() == 1).await;
        assert_eq!(s.hubs.conns()[0].feed, 6);
    }

    #[tokio::test]
    async fn mpeg2_video_is_refused_from_the_stream_and_remembered() {
        let s = server("watch-mpeg2");
        enable(&s, true).await;
        let (base, connections) = feed_server().await;
        feeds_from(&s, &base);
        let cookie = browser_login(&s).await;
        let (st, body) =
            call(&s, from_page("POST", "/api/watch", Some(&cookie), watch_body(66, false))).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["reason"], "notPlayable");
        assert!(body["error"].as_str().unwrap().contains("MPEG-2"), "{body}");
        until("the connection closes", || s.hubs.open_count() == 0).await;
        // Known now: refused again without connecting.
        let (st, _) =
            call(&s, from_page("POST", "/api/watch", Some(&cookie), watch_body(66, false))).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(connections.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_device_may_hold_only_so_many_viewers() {
        let s = server("watch-limit");
        enable(&s, true).await;
        let (base, _) = feed_server().await;
        feeds_from(&s, &base);
        let cookie = browser_login(&s).await;
        for ch in 0..crate::recording::watch::PER_DEVICE as i64 {
            let (st, body) = call(
                &s,
                from_page("POST", "/api/watch", Some(&cookie), watch_body(ch + 1, false)),
            )
            .await;
            assert_eq!(st, StatusCode::OK, "{body}");
        }
        let (st, body) =
            call(&s, from_page("POST", "/api/watch", Some(&cookie), watch_body(99, false))).await;
        assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "{body}");
        assert_eq!(body["reason"], "limit");
    }

    // ------------------------------------------------------- watching movies

    /// Movie `id` in the catalogue as `container`, its kept details saying
    /// H.264 video and AAC sound and a 90-minute runtime (which a watch does
    /// not go by: the file says what it holds). The caller holds
    /// [`movie_catalogue`];
    /// written afresh before each use all the same, as a sync elsewhere in
    /// the tests may land a catalogue without its movies.
    fn seed_movie(id: i64, container: &str) {
        use crate::catalog::{db, vod_store};
        db::install_test_db();
        db::with_db(|c| {
            c.execute(
                "INSERT INTO movies (movie_id, category_id, name, clean_name, added, container)
                 VALUES (?1, 'vod-9120', 'Quay Lights', 'Quay Lights', 1, ?2)
                 ON CONFLICT(movie_id) DO UPDATE SET deleted_at = NULL, container = excluded.container",
                rusqlite::params![id, container],
            )?;
            let info = serde_json::json!({
                "video_codec": "h264", "audio_codecs": ["aac"], "audio_languages": ["eng"],
                "runtime": 5400
            });
            vod_store::store_details(c, id, now(), &info.to_string(), Some(5400))
        })
        .unwrap();
    }

    /// A 10 s MP4 of about a megabyte, its index first: H.264 video (a
    /// keyframe a second) and English AAC sound, and, `subtitled`, English
    /// `mov_text` subtitles with a cue from 1 to 3 s. Made once each, with
    /// the ffmpeg command; `None` without one.
    fn mp4(subtitled: bool) -> Option<Arc<Vec<u8>>> {
        static PLAIN: std::sync::OnceLock<Option<Arc<Vec<u8>>>> = std::sync::OnceLock::new();
        static SUBTITLED: std::sync::OnceLock<Option<Arc<Vec<u8>>>> = std::sync::OnceLock::new();
        let made = if subtitled { &SUBTITLED } else { &PLAIN };
        made.get_or_init(|| {
            let dir = std::env::temp_dir().join(format!(
                "zapper-mp4-{}-{subtitled}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).ok()?;
            let srt = dir.join("subs.srt");
            std::fs::write(&srt, "1\n00:00:01,000 --> 00:00:03,000\nAt the quay\n\n").ok()?;
            let out = dir.join("film.mp4");
            let mut ffmpeg = std::process::Command::new("ffmpeg");
            ffmpeg
                .args(["-hide_banner", "-loglevel", "error", "-y"])
                .args(["-f", "lavfi", "-i", "testsrc2=size=320x180:rate=25"])
                .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"]);
            if subtitled {
                ffmpeg.arg("-i").arg(&srt);
            }
            ffmpeg.args(["-t", "10", "-map", "0:v", "-map", "1:a"]);
            if subtitled {
                ffmpeg
                    .args(["-map", "2:s", "-c:s", "mov_text"])
                    .args(["-metadata:s:s:0", "language=eng"]);
            }
            let made = ffmpeg
                .args(["-c:v", "libx264", "-g", "25", "-pix_fmt", "yuv420p", "-b:v", "800k"])
                .args(["-c:a", "aac", "-metadata:s:a:0", "language=eng"])
                .args(["-movflags", "+faststart"])
                .arg(&out)
                .status()
                .ok()?
                .success();
            let bytes = made.then(|| std::fs::read(&out).ok()).flatten();
            let _ = std::fs::remove_dir_all(&dir);
            bytes.map(Arc::new)
        })
        .clone()
    }

    /// Every movie's file is a plain [`mp4`] at the fake provider; the
    /// provider's address and counts, and the file. `None` without the
    /// ffmpeg command, said so.
    async fn mp4s_from(
        s: &Shared,
    ) -> Option<(String, Arc<crate::recording::upstream::fake::Counts>, Arc<Vec<u8>>)> {
        let Some(film) = mp4(false) else {
            eprintln!("skipped: no ffmpeg command to make a fixture with");
            return None;
        };
        let (base, counts) = films_from(s, &film, "").await;
        Some((base, counts, film))
    }

    fn movie_body(id: i64) -> Option<serde_json::Value> {
        Some(serde_json::json!({"movieId": id}))
    }

    /// The next piece of a streaming answer, or `None` at its end.
    async fn next_piece(body: &mut axum::body::BodyDataStream) -> Option<bytes::Bytes> {
        use futures_core::Stream;
        tokio::time::timeout(
            Duration::from_secs(10),
            std::future::poll_fn(|cx| std::pin::Pin::new(&mut *body).poll_next(cx)),
        )
        .await
        .expect("stream data in time")
        .and_then(Result::ok)
    }

    fn with_range(mut r: Request, range: &str) -> Request {
        r.headers_mut()
            .insert(header::RANGE, HeaderValue::from_str(range).unwrap());
        r
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_new_request_ends_the_one_before() {
        use crate::recording::upstream::fake;
        use std::sync::atomic::Ordering;
        let _catalogue = movie_catalogue();
        let s = server("vod-watch");
        enable(&s, true).await;
        let Some((base, counts, film)) = mp4s_from(&s).await else {
            return;
        };
        // Movie 9_120_002 is no film at all: a megabyte of a pattern.
        let (junk, junk_counts) = fake::serve().await;
        let film_url = format!("{base}/movie/secret-user/secret-pass/1.mp4");
        let junk_url = format!("{junk}/movie/secret-user/secret-pass/2.mkv");
        s.vod.set_playables(Arc::new(move |item: VodItem| {
            let url = if item == VodItem::movie(9_120_002) { &junk_url } else { &film_url };
            Ok(crate::source::Playable::plain(url.clone()))
        }));
        let size = film.len();
        let cookie = browser_login(&s).await;
        let (st, _) = call(&s, from_page("POST", "/api/vod/watch", None, movie_body(9_120_001))).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);

        seed_movie(9_120_001, "mp4");
        let (st, body) =
            call(&s, from_page("POST", "/api/vod/watch", Some(&cookie), movie_body(9_120_001))).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        // An MP4 a browser plays, without subtitles: as it is, as its file
        // (read at the watch) says, not as the details do.
        assert_eq!(body["mode"], "direct");
        assert_eq!(body["duration"], 10);
        assert_eq!(body["audio"][0]["codec"], "aac");
        assert_eq!(body["audio"][0]["lang"], "eng");
        assert_eq!(body["audioIndex"], 0);
        assert_eq!(body["converting"], false);
        assert_eq!(body["subtitles"], serde_json::json!([]));
        let asked = counts.requests.load(Ordering::SeqCst);
        for leak in ["127.0.0.1", "secret-user", "secret-pass", base.as_str()] {
            assert!(!body.to_string().contains(leak), "{leak} reached the browser: {body}");
        }
        let stream = body["stream"].as_str().unwrap().to_string();
        assert!(stream.starts_with("/api/vod/stream/"), "{stream}");
        assert_eq!(s.own_conns().len(), 1, "the viewer holds a connection");

        // Its own page alone reads it.
        let (st, _) = call(&s, req("GET", &stream, HOME, false, Some(APP))).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        let mut r = from_page("GET", &stream, Some(&cookie), None);
        r.headers_mut()
            .insert("sec-fetch-site", HeaderValue::from_static("cross-site"));
        assert_eq!(call(&s, r).await.0, StatusCode::FORBIDDEN);
        // A HEAD asks the provider nothing (counted below).
        let (st, h, _) = call_raw(&s, from_page("HEAD", &stream, Some(&cookie), None)).await;
        assert_eq!(st, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(h[header::ALLOW], "GET");

        // The browser's player reads from the start...
        let get = |range| with_range(from_page("GET", &stream, Some(&cookie), None), range);
        let first = router(s.clone()).oneshot(get("bytes=0-")).await.unwrap();
        assert_eq!(first.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            first.headers()[header::CONTENT_RANGE],
            format!("bytes 0-{}/{size}", size - 1).as_str()
        );
        assert_eq!(first.headers()[header::CONTENT_LENGTH], size.to_string().as_str());
        assert_eq!(first.headers()[header::ACCEPT_RANGES], "bytes");
        assert_eq!(first.headers()[header::CONTENT_TYPE], "video/mp4");
        let mut first = first.into_body().into_data_stream();
        let piece = next_piece(&mut first).await.unwrap();
        assert_eq!(piece[..100], film[..100]);

        // ...and, while that one streams, seeks.
        let half = size / 2;
        let second = router(s.clone())
            .oneshot(with_range(
                from_page("GET", &stream, Some(&cookie), None),
                &format!("bytes={half}-"),
            ))
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            second.headers()[header::CONTENT_RANGE],
            format!("bytes {half}-{}/{size}", size - 1).as_str()
        );
        assert_eq!(second.headers()[header::CONTENT_LENGTH], (size - half).to_string().as_str());
        // The first has ended, short of its length.
        let mut had = piece.len();
        while let Some(p) = next_piece(&mut first).await {
            had += p.len();
        }
        assert!(had < size, "the first reading stopped");
        let mut second = second.into_body().into_data_stream();
        let mut got = Vec::new();
        while let Some(p) = next_piece(&mut second).await {
            got.extend_from_slice(&p);
        }
        assert_eq!(got[..], film[half..]);
        assert_eq!(
            counts.most_open.load(Ordering::SeqCst),
            1,
            "never two responses open at once"
        );
        assert_eq!(
            counts.requests.load(Ordering::SeqCst),
            asked + 2,
            "one request a reading"
        );

        // A request a newer one supersedes while the provider is still
        // answering it is given up at once, not answered late.
        let (older, newer) = tokio::join!(router(s.clone()).oneshot(get("bytes=100000-")), async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            router(s.clone()).oneshot(get("bytes=200000-")).await
        });
        assert_eq!(older.unwrap().status(), StatusCode::CONFLICT);
        let newer = newer.unwrap();
        assert_eq!(newer.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            newer.headers()[header::CONTENT_RANGE],
            format!("bytes 200000-{}/{size}", size - 1).as_str()
        );
        drop(newer);

        // Every file is read at the watch, to learn what it holds: this one
        // is no film, and says so. It replaced the first viewer, which let
        // its file go.
        seed_movie(9_120_002, "mp4");
        let viewer = viewer_id(&body);
        let replace = Some(serde_json::json!({"movieId": 9_120_002, "replaces": viewer}));
        let (st, body) = call(&s, from_page("POST", "/api/vod/watch", Some(&cookie), replace)).await;
        assert_eq!(st, StatusCode::BAD_GATEWAY, "{body}");
        assert_eq!(body["reason"], "feed");
        for leak in ["127.0.0.1", "secret-user", "secret-pass", base.as_str()] {
            assert!(!body.to_string().contains(leak), "{leak} reached the browser: {body}");
        }
        let (st, _) = call(&s, from_page("GET", &format!("/api/vod/watch/{viewer}"), Some(&cookie), None)).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "the first viewer is gone");
        assert!(s.own_conns().is_empty());
        until("the provider sees no connection", || {
            counts.open.load(Ordering::SeqCst) == 0 && junk_counts.open.load(Ordering::SeqCst) == 0
        })
        .await;
        assert_eq!(junk_counts.most_open.load(Ordering::SeqCst), 1);

        // Stopped: its connection is given up.
        let (st, body) =
            call(&s, from_page("POST", "/api/vod/watch", Some(&cookie), movie_body(9_120_001))).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(s.own_conns().len(), 1);
        let (st, _) = call(
            &s,
            from_page("DELETE", &format!("/api/vod/watch/{}", viewer_id(&body)), Some(&cookie), None),
        )
        .await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        assert!(s.own_conns().is_empty());
        until("the provider sees no connection", || {
            counts.open.load(Ordering::SeqCst) == 0
        })
        .await;
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_browser_cannot_watch_more_than_its_share() {
        let _catalogue = movie_catalogue();
        let s = server("vod-share");
        enable(&s, true).await;
        let (feeds, _) = feed_server().await;
        feeds_from(&s, &feeds);
        if mp4s_from(&s).await.is_none() {
            return;
        }
        let cookie = browser_login(&s).await;
        let post = |uri: &str, body| from_page("POST", uri, Some(&cookie), body);
        for ch in 1..crate::recording::watch::PER_DEVICE as i64 {
            let (st, body) = call(&s, post("/api/watch", watch_body(ch, false))).await;
            assert_eq!(st, StatusCode::OK, "{body}");
        }
        seed_movie(9_120_011, "mp4");
        let (st, body) = call(&s, post("/api/vod/watch", movie_body(9_120_011))).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        let movie = viewer_id(&body);
        // Channels and movies count together, whichever comes next.
        let (st, body) = call(&s, post("/api/watch", watch_body(9, false))).await;
        assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "{body}");
        assert_eq!(body["reason"], "limit");
        seed_movie(9_120_012, "mp4");
        let (st, body) = call(&s, post("/api/vod/watch", movie_body(9_120_012))).await;
        assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "{body}");
        assert_eq!(body["reason"], "limit");
        // The movie stopped, a channel may start.
        let (st, _) =
            call(&s, from_page("DELETE", &format!("/api/vod/watch/{movie}"), Some(&cookie), None)).await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        let (st, body) = call(&s, post("/api/watch", watch_body(9, false))).await;
        assert_eq!(st, StatusCode::OK, "{body}");
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_movie_viewer_holds_one_of_the_accounts_connections_and_anyway_takes_it() {
        let _catalogue = movie_catalogue();
        let s = server_with("vod-full", Box::new(Full));
        enable(&s, true).await;
        let (feeds, _) = feed_server().await;
        feeds_from(&s, &feeds);
        if mp4s_from(&s).await.is_none() {
            return;
        }
        let cookie = browser_login(&s).await;
        seed_movie(9_120_021, "mp4");
        // Another device holds the account's one connection.
        let (st, body) =
            call(&s, from_page("POST", "/api/vod/watch", Some(&cookie), movie_body(9_120_021))).await;
        assert_eq!(st, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["reason"], "inUse");
        assert_eq!(body["anyway"], true);
        let anyway = Some(serde_json::json!({"movieId": 9_120_021, "anyway": true}));
        let (st, body) = call(&s, from_page("POST", "/api/vod/watch", Some(&cookie), anyway)).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        let movie = viewer_id(&body);

        // The app asking for a channel finds a browser watching a movie,
        // named, with nothing to watch instead.
        let app = |anyway| {
            let mut r =
                req_body("POST", "/api/watch", HOME, false, Some(APP), watch_body(8, anyway));
            r.headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
            r
        };
        let (st, body) = call(&s, app(false)).await;
        assert_eq!(st, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["reason"], "viewers");
        assert_eq!(body["watchInstead"], serde_json::json!([]));
        assert!(body["error"].as_str().unwrap().contains("Quay Lights"), "{body}");
        // Anyway: the movie's viewer gives its place up, and is told why.
        let (st, body) = call(&s, app(true)).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        let (st, status) =
            call(&s, from_page("GET", &format!("/api/vod/watch/{movie}"), Some(&cookie), None)).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(status["takenOver"], true);
        assert_eq!(s.own_conns().len(), 1);
        assert_eq!(s.own_conns()[0].feed, 8);
    }

    /// An account with room until `full` is set, then full with two
    /// connections in use.
    struct FullWhen(Arc<std::sync::atomic::AtomicBool>);
    impl Budget for FullWhen {
        fn check(
            &self,
            _own: usize,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Slots> + Send + '_>> {
            let full = self.0.load(std::sync::atomic::Ordering::SeqCst);
            Box::pin(async move {
                if full {
                    Slots::Full { active: 2, max: 2 }
                } else {
                    Slots::Free
                }
            })
        }
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_device_making_room_for_itself_never_ends_another_devices_viewer_of_the_same_movie() {
        let _catalogue = movie_catalogue();
        let full = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let s = server_with("vod-same", Box::new(FullWhen(Arc::clone(&full))));
        enable(&s, true).await;
        if mp4s_from(&s).await.is_none() {
            return;
        }
        let cookie = browser_login(&s).await;
        let app = |body| {
            let mut r = req_body("POST", "/api/vod/watch", HOME, false, Some(APP), body);
            r.headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
            r
        };
        let status = |id: &str, page: bool| {
            let uri = format!("/api/vod/watch/{id}");
            if page {
                from_page("GET", &uri, Some(&cookie), None)
            } else {
                req("GET", &uri, HOME, false, Some(APP))
            }
        };

        // The app watches movie 9 first, then this browser does: two
        // connections on the movie's one feed, the app's the older.
        seed_movie(9_120_031, "mp4");
        let (st, body) = call(&s, app(movie_body(9_120_031))).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        let other = viewer_id(&body);
        let (st, body) =
            call(&s, from_page("POST", "/api/vod/watch", Some(&cookie), movie_body(9_120_031))).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        let mine = viewer_id(&body);
        assert_eq!(s.own_conns().len(), 2);
        full.store(true, std::sync::atomic::Ordering::SeqCst);

        // The browser asks again, its old viewer not named: its own is the
        // one that makes room, never the app's.
        let (st, body) =
            call(&s, from_page("POST", "/api/vod/watch", Some(&cookie), movie_body(9_120_031))).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        let newer = viewer_id(&body);
        let (st, st_other) = call(&s, status(&other, false)).await;
        assert_eq!(st, StatusCode::OK, "{st_other}");
        assert_eq!(st_other["ended"], serde_json::Value::Null, "{st_other}");
        let (_, st_mine) = call(&s, status(&mine, true)).await;
        assert_eq!(st_mine["takenOver"], true, "{st_mine}");

        // And replacing it by name ends only it.
        let replace = Some(serde_json::json!({"movieId": 9_120_031, "replaces": newer}));
        let (st, body) = call(&s, from_page("POST", "/api/vod/watch", Some(&cookie), replace)).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        let (st, st_other) = call(&s, status(&other, false)).await;
        assert_eq!(st, StatusCode::OK, "{st_other}");
        assert_eq!(st_other["ended"], serde_json::Value::Null, "{st_other}");
        let (st, _) = call(&s, status(&newer, true)).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "the replaced viewer is gone");
        assert_eq!(s.own_conns().len(), 2);
    }

    // ------------------------------------------------- converting movies

    /// A 20 s Matroska film for the remuxer: H.264 video (a keyframe a
    /// second), AAC sound in English and AC-3 in French, and English SubRip
    /// subtitles named "Forced", with a cue from 1 to 3 s and one from 12 to
    /// 14 s. Made once, with the ffmpeg command; `None` without one.
    fn film() -> Option<Arc<Vec<u8>>> {
        static FILM: std::sync::OnceLock<Option<Arc<Vec<u8>>>> = std::sync::OnceLock::new();
        FILM.get_or_init(|| {
            let dir = std::env::temp_dir().join(format!("zapper-film-{}", std::process::id()));
            std::fs::create_dir_all(&dir).ok()?;
            let srt = dir.join("subs.srt");
            std::fs::write(
                &srt,
                "1\n00:00:01,000 --> 00:00:03,000\nHello & 1 < 2\n\n\
                 2\n00:00:12,000 --> 00:00:14,000\n<i>Later</i>\non two lines\n\n",
            )
            .ok()?;
            let out = dir.join("film.mkv");
            let made = std::process::Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-y"])
                .args(["-f", "lavfi", "-i", "testsrc2=size=320x180:rate=25"])
                .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"])
                .args(["-f", "lavfi", "-i", "sine=frequency=880:sample_rate=48000"])
                .arg("-i")
                .arg(&srt)
                .args(["-t", "20", "-map", "0:v", "-map", "1:a", "-map", "2:a", "-map", "3:s"])
                .args(["-c:v", "libx264", "-g", "25", "-pix_fmt", "yuv420p", "-b:v", "300k"])
                .args(["-c:a:0", "aac", "-c:a:1", "ac3", "-c:s", "srt"])
                .args(["-metadata:s:a:0", "language=eng", "-metadata:s:a:1", "language=fre"])
                .args(["-metadata:s:s:0", "language=eng", "-metadata:s:s:0", "title=Forced"])
                .arg(&out)
                .status()
                .ok()?
                .success();
            let bytes = made.then(|| std::fs::read(&out).ok()).flatten();
            let _ = std::fs::remove_dir_all(&dir);
            bytes.map(Arc::new)
        })
        .clone()
    }

    /// Every movie's file is `film`, at the fake provider, under `path`; the
    /// provider's counts.
    async fn films_from(
        s: &Shared,
        film: &Arc<Vec<u8>>,
        path: &str,
    ) -> (String, Arc<crate::recording::upstream::fake::Counts>) {
        let (base, counts) = crate::recording::upstream::fake::serve_file(Arc::clone(film)).await;
        let url = format!("{base}{path}/movie/secret-user/secret-pass/2.mkv");
        s.vod.set_playables(Arc::new(move |_: VodItem| {
            Ok(crate::source::Playable::plain(url.clone()))
        }));
        (base, counts)
    }

    /// The whole of a streaming answer.
    async fn read_all(body: Body) -> Vec<u8> {
        let mut body = body.into_data_stream();
        let mut got = Vec::new();
        while let Some(p) = next_piece(&mut body).await {
            got.extend_from_slice(&p);
        }
        got
    }

    fn x_start(r: &Response) -> f64 {
        r.headers()["x-start"].to_str().unwrap().parse().unwrap()
    }

    fn is_ts(b: &[u8]) -> bool {
        !b.is_empty() && b.len() % 188 == 0 && b.chunks(188).all(|p| p[0] == 0x47)
    }

    /// A WebVTT file's cues: start, end, text.
    fn vtt_cues(doc: &str) -> Vec<(f64, f64, String)> {
        let secs = |t: &str| {
            let p: Vec<f64> = t.trim().split(':').map(|x| x.parse().unwrap()).collect();
            p[0] * 3600.0 + p[1] * 60.0 + p[2]
        };
        assert!(doc.starts_with("WEBVTT\n\n"), "{doc}");
        doc["WEBVTT\n\n".len()..]
            .split("\n\n")
            .filter(|b| !b.is_empty())
            .map(|b| {
                let (times, text) = b.split_once('\n').unwrap();
                let (a, z) = times.split_once(" --> ").unwrap();
                (secs(a), secs(z), text.to_string())
            })
            .collect()
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_seek_reopens_from_the_new_time() {
        use std::sync::atomic::Ordering;
        let Some(film) = film() else {
            eprintln!("skipped: no ffmpeg command to make a fixture with");
            return;
        };
        let _catalogue = movie_catalogue();
        let s = server("vod-remux");
        enable(&s, true).await;
        // Slow enough that the first remux still reads when the seek comes.
        let (_, counts) = films_from(&s, &film, "/slow").await;
        let cookie = browser_login(&s).await;
        seed_movie(9_120_041, "mkv");
        let (st, body) =
            call(&s, from_page("POST", "/api/vod/watch", Some(&cookie), movie_body(9_120_041))).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["mode"], "remux");
        let duration = body["duration"].as_i64().unwrap();
        assert!((19..=21).contains(&duration), "the file's length, not the details': {duration}");
        let stream = body["stream"].as_str().unwrap().to_string();
        let get = |q: &str| from_page("GET", &format!("{stream}{q}"), Some(&cookie), None);
        assert_eq!(call(&s, get("?from=-1")).await.0, StatusCode::BAD_REQUEST);
        assert_eq!(call(&s, get("?from=soon")).await.0, StatusCode::BAD_REQUEST);

        let first = router(s.clone()).oneshot(get("?from=0")).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(first.headers()[header::CONTENT_TYPE], "video/mp2t");
        assert!(first.headers().get(header::CONTENT_LENGTH).is_none());
        assert!(x_start(&first) < 0.5, "starts at {}", x_start(&first));
        let mut first = first.into_body().into_data_stream();
        let piece = next_piece(&mut first).await.unwrap();
        assert_eq!(piece[0], 0x47, "MPEG-TS");
        let asked = counts.firsts.lock().unwrap().len();

        // Seeking past what the browser holds: from the keyframe at or
        // before 12 s, said so.
        let second = router(s.clone()).oneshot(get("?from=12")).await.unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        let at = x_start(&second);
        assert!(at <= 12.0 && at > 10.9, "starts at {at}");
        // The first has ended.
        let mut had = piece.len();
        while let Some(p) = next_piece(&mut first).await {
            had += p.len();
        }
        let got = read_all(second.into_body()).await;
        assert!(is_ts(&got), "{} bytes", got.len());
        assert!(had > 0);
        let firsts = counts.firsts.lock().unwrap().clone();
        assert!(
            firsts[asked..].iter().any(|&f| f > film.len() / 2),
            "the second remux reads from the middle: {firsts:?} of {}",
            film.len()
        );
        assert_eq!(
            counts.most_open.load(Ordering::SeqCst),
            1,
            "never two responses open at once"
        );
        assert_eq!(s.own_conns().len(), 1);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn the_watch_answer_lists_the_films_audio() {
        let Some(film) = film() else {
            eprintln!("skipped: no ffmpeg command to make a fixture with");
            return;
        };
        let _catalogue = movie_catalogue();
        let s = server("vod-tracks");
        enable(&s, true).await;
        let (base, _) = films_from(&s, &film, "").await;
        let cookie = browser_login(&s).await;
        seed_movie(9_120_051, "mkv");
        let body = Some(serde_json::json!({"movieId": 9_120_051, "audio": 1}));
        let (st, body) = call(&s, from_page("POST", "/api/vod/watch", Some(&cookie), body)).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        for leak in ["127.0.0.1", "secret-user", "secret-pass", base.as_str()] {
            assert!(!body.to_string().contains(leak), "{leak} reached the browser: {body}");
        }
        let audio: Vec<_> = body["audio"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| (t["index"].as_u64().unwrap(), t["codec"].clone(), t["lang"].clone()))
            .collect();
        assert_eq!(
            audio,
            [(0, "aac".into(), "eng".into()), (1, "ac3".into(), "fre".into())],
            "the file's, not the details'"
        );
        if crate::recording::livemux::can_encode_aac() {
            assert_eq!(body["audioIndex"], 1);
            assert_eq!(body["converting"], true, "no AC-3 in this browser");
        }
        assert_eq!(
            body["subtitles"],
            serde_json::json!([{"index": 0, "language": "eng", "title": "Forced", "text": true}])
        );

        // The first sound, unasked, plays as it is.
        let replace = Some(serde_json::json!({"movieId": 9_120_051, "replaces": viewer_id(&body)}));
        let (st, body) = call(&s, from_page("POST", "/api/vod/watch", Some(&cookie), replace)).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["audioIndex"], 0);
        assert_eq!(body["converting"], false);
        assert_eq!(s.own_conns().len(), 1);

        // A browser that plays AC-3 is sent it as it is; a sound the file
        // does not have is refused; one chosen stays chosen.
        let replace = Some(serde_json::json!({
            "movieId": 9_120_051, "replaces": viewer_id(&body), "can": {"ac3": true}
        }));
        let (st, body) = call(&s, from_page("POST", "/api/vod/watch", Some(&cookie), replace)).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        let stream = body["stream"].as_str().unwrap().to_string();
        let get = |q: &str| from_page("GET", &format!("{stream}{q}"), Some(&cookie), None);
        let (st, body) = call(&s, get("?audio=2")).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        let sound = |ts: &[u8]| {
            let mut t = crate::recording::tsprobe::Tables::new();
            t.feed(ts).expect("a PMT").audio[0].codec
        };
        use crate::recording::tsprobe::Audio;
        let r = router(s.clone()).oneshot(get("?from=15&audio=1")).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(sound(&read_all(r.into_body()).await), Audio::Ac3);
        let r = router(s.clone()).oneshot(get("?from=15")).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(sound(&read_all(r.into_body()).await), Audio::Ac3, "still the second sound");
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn an_mp4_with_text_subtitles_is_remuxed_to_show_them() {
        use std::sync::atomic::Ordering;
        let Some(film) = mp4(true) else {
            eprintln!("skipped: no ffmpeg command to make a fixture with");
            return;
        };
        let _catalogue = movie_catalogue();
        let s = server("vod-mp4-subs");
        enable(&s, true).await;
        let (_, counts) = films_from(&s, &film, "").await;
        let cookie = browser_login(&s).await;
        // Its details, like every provider's, list no subtitles: the file
        // does.
        seed_movie(9_120_071, "mp4");
        let (st, body) =
            call(&s, from_page("POST", "/api/vod/watch", Some(&cookie), movie_body(9_120_071))).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["mode"], "remux", "{body}");
        assert_eq!(body["converting"], false, "AAC as it is");
        assert_eq!(body["duration"], 10);
        assert_eq!(
            body["subtitles"],
            serde_json::json!([{"index": 0, "language": "eng", "title": null, "text": true}])
        );
        let viewer = viewer_id(&body);
        let stream = body["stream"].as_str().unwrap().to_string();
        let r = router(s.clone())
            .oneshot(from_page("GET", &format!("{stream}?from=0"), Some(&cookie), None))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert!(is_ts(&read_all(r.into_body()).await));
        let subs = || {
            from_page("GET", &format!("/api/vod/subtitles/{viewer}/0"), Some(&cookie), None)
        };
        let mut cues = Vec::new();
        for _ in 0..50 {
            let (st, _, doc) = call_raw(&s, subs()).await;
            assert_eq!(st, StatusCode::OK);
            cues = vtt_cues(&String::from_utf8(doc).unwrap());
            if !cues.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(cues.len(), 1, "{cues:?}");
        assert!((cues[0].0 - 1.0).abs() < 0.1 && (cues[0].1 - 3.0).abs() < 0.1, "{cues:?}");
        assert_eq!(cues[0].2, "At the quay");
        assert_eq!(
            counts.most_open.load(Ordering::SeqCst),
            1,
            "the file read once at a time, on one connection"
        );
        assert_eq!(s.own_conns().len(), 1);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn subtitles_come_from_the_same_read_in_the_films_time() {
        use std::sync::atomic::Ordering;
        let Some(film) = film() else {
            eprintln!("skipped: no ffmpeg command to make a fixture with");
            return;
        };
        let _catalogue = movie_catalogue();
        let s = server("vod-subs");
        enable(&s, true).await;
        let (_, counts) = films_from(&s, &film, "").await;
        let cookie = browser_login(&s).await;
        seed_movie(9_120_061, "mkv");
        let (st, body) =
            call(&s, from_page("POST", "/api/vod/watch", Some(&cookie), movie_body(9_120_061))).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        let viewer = viewer_id(&body);
        let stream = body["stream"].as_str().unwrap().to_string();
        let subs = |q: &str| {
            from_page("GET", &format!("/api/vod/subtitles/{viewer}/0{q}"), Some(&cookie), None)
        };
        // Cues come as the remux reads them, and are kept as they come.
        async fn cues_once(
            s: &Shared,
            r: impl Fn() -> Request,
            want: usize,
        ) -> Vec<(f64, f64, String)> {
            for _ in 0..50 {
                let (st, h, doc) = call_raw(s, r()).await;
                assert_eq!(st, StatusCode::OK);
                assert_eq!(h[header::CONTENT_TYPE], "text/vtt; charset=utf-8");
                let cues = vtt_cues(&String::from_utf8(doc).unwrap());
                if cues.len() >= want {
                    return cues;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            panic!("no cues came");
        }
        let cues_once = |want, q: &'static str| cues_once(&s, move || subs(q), want);

        // Nothing read yet, nothing to show.
        assert!(cues_once(0, "").await.is_empty());
        let r = router(s.clone())
            .oneshot(from_page("GET", &format!("{stream}?from=0"), Some(&cookie), None))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert!(is_ts(&read_all(r.into_body()).await));
        let cues = cues_once(2, "").await;
        assert_eq!(cues.len(), 2, "{cues:?}");
        let close = |(a, z): (f64, f64), (want_a, want_z): (f64, f64)| {
            (a - want_a).abs() < 0.1 && (z - want_z).abs() < 0.1
        };
        assert!(close((cues[0].0, cues[0].1), (1.0, 3.0)), "{cues:?}");
        assert_eq!(cues[0].2, "Hello &amp; 1 &lt; 2");
        assert!(close((cues[1].0, cues[1].1), (12.0, 14.0)), "{cues:?}");
        assert_eq!(cues[1].2, "<i>Later</i>\non two lines");
        let later = cues_once(1, "?after=5").await;
        assert_eq!(later.len(), 1, "{later:?}");
        assert_eq!(later[0].2, cues[1].2);

        // A remux from 12 s: its cues alone, in the film's time.
        let r = router(s.clone())
            .oneshot(from_page("GET", &format!("{stream}?from=12"), Some(&cookie), None))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        // On the keyframe at or before 12 s: the film has one each second, so it may be 12 s
        // itself (FFmpeg 8.1 lands there; 8.0 on the one before).
        assert!(x_start(&r) <= 12.0, "starts at {}", x_start(&r));
        assert!(is_ts(&read_all(r.into_body()).await));
        let cues = cues_once(1, "").await;
        assert_eq!(cues.len(), 1, "{cues:?}");
        assert!(close((cues[0].0, cues[0].1), (12.0, 14.0)), "{cues:?}");
        assert_eq!(cues[0].2, "<i>Later</i>\non two lines");

        // Its own page's alone, and only the tracks there are.
        assert_eq!(
            call(&s, req("GET", &format!("/api/vod/subtitles/{viewer}/0"), HOME, false, Some(APP)))
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            call(&s, from_page("GET", &format!("/api/vod/subtitles/{viewer}/1"), Some(&cookie), None))
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(call(&s, subs("?after=later")).await.0, StatusCode::BAD_REQUEST);
        assert_eq!(
            call(&s, from_page("GET", &format!("/api/vod/subtitles/{viewer}/0"), None, None))
                .await
                .0,
            StatusCode::UNAUTHORIZED,
            "signed in only"
        );
        // Subtitles that are pictures are listed, and never served.
        let pictures = s.vod.add_for_test("app", VodItem::movie(9_120_061), -61);
        s.vod.set_subtitles_for_test(
            &pictures,
            vec![crate::recording::vodmux::SubtitleStream {
                index: 3,
                codec: "hdmv_pgs_subtitle".into(),
                lang: Some("eng".into()),
                title: None,
                text: false,
            }],
        );
        let (st, body) = call(
            &s,
            req("GET", &format!("/api/vod/subtitles/{pictures}/0"), HOME, false, Some(APP)),
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{body}");
        assert!(body["error"].as_str().unwrap().contains("pictures"), "{body}");
        let mut r = subs("");
        r.headers_mut()
            .insert("sec-fetch-site", HeaderValue::from_static("cross-site"));
        assert_eq!(call(&s, r).await.0, StatusCode::FORBIDDEN);
        assert_eq!(counts.most_open.load(Ordering::SeqCst), 1);
    }

    // ------------------------------------------------------ watching episodes

    /// Episode `id` of the made-up "A Show" (9_130_000), season 2 episode
    /// `number`, in the catalogue as `container`, its listed length 45
    /// minutes (which a watch does not go by: the file says). The caller
    /// holds [`movie_catalogue`].
    fn seed_episode(id: i64, number: i32, container: &str) {
        use crate::catalog::db;
        db::install_test_db();
        db::with_db(|c| {
            c.execute(
                "INSERT INTO series (series_id, name, clean_name, added)
                 VALUES (9130000, 'A Show', 'A Show', 1)
                 ON CONFLICT(series_id) DO UPDATE SET deleted_at = NULL",
                [],
            )?;
            c.execute(
                "INSERT INTO episodes
                   (episode_id, series_id, season, episode, title, duration_secs, container)
                 VALUES (?1, 9130000, 2, ?2, 'Night Shift', 2700, ?3)
                 ON CONFLICT(episode_id) DO UPDATE SET deleted_at = NULL,
                   container = excluded.container",
                rusqlite::params![id, number, container],
            )?;
            Ok(())
        })
        .unwrap();
    }

    fn episode_body(id: i64) -> Option<serde_json::Value> {
        Some(serde_json::json!({"episodeId": id}))
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_browser_watches_an_episode_direct_and_remuxed() {
        use crate::recording::upstream::fake;
        use crate::recording::vodwatch::vod_feed;
        use std::sync::atomic::Ordering;
        let (Some(plain), Some(film)) = (mp4(false), film()) else {
            eprintln!("skipped: no ffmpeg command to make a fixture with");
            return;
        };
        let _catalogue = movie_catalogue();
        let s = server("vod-episode");
        enable(&s, true).await;
        let (mp4_base, mp4_counts) = fake::serve_file(Arc::clone(&plain)).await;
        // Slow enough that the first remux still reads when the seek comes.
        let (mkv_base, mkv_counts) = fake::serve_file(Arc::clone(&film)).await;
        let mp4_url = format!("{mp4_base}/series/secret-user/secret-pass/9130004.mp4");
        let mkv_url = format!("{mkv_base}/slow/series/secret-user/secret-pass/9130005.mkv");
        // Episode 9_130_004 and movie 9_130_004 share an id; only the
        // episode has a file here.
        s.vod.set_playables(Arc::new(move |item: VodItem| {
            let url = if item == VodItem::episode(9_130_004) {
                mp4_url.clone()
            } else if item == VodItem::episode(9_130_005) {
                mkv_url.clone()
            } else if item == VodItem::episode(9_130_006) {
                "http://127.0.0.1:1/series/secret-user/secret-pass/9130006.mp4".into()
            } else {
                anyhow::bail!("that episode is not in the catalogue any more: {item:?}");
            };
            Ok(crate::source::Playable::plain(url))
        }));
        let cookie = browser_login(&s).await;
        let post = |body| from_page("POST", "/api/vod/watch", Some(&cookie), body);
        seed_episode(9_130_004, 4, "mp4");
        seed_episode(9_130_005, 5, "mkv");
        seed_episode(9_130_006, 6, "mp4");

        // An MP4 a browser plays: as it is, by ranges passed on.
        let (st, body) = call(&s, post(episode_body(9_130_004))).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["mode"], "direct");
        assert_eq!(body["duration"], 10, "the file's length, not the listing's");
        assert_eq!(body["audio"][0]["codec"], "aac");
        for leak in ["127.0.0.1", "secret-user", "secret-pass", "/series/", mp4_base.as_str()] {
            assert!(!body.to_string().contains(leak), "{leak} reached the browser: {body}");
        }
        let own = s.own_conns();
        assert_eq!(own.len(), 1, "the viewer holds a connection");
        assert_eq!(own[0].feed, vod_feed(VodItem::episode(9_130_004)));
        let stream = body["stream"].as_str().unwrap().to_string();
        let size = plain.len();
        let half = size / 2;
        let r = router(s.clone())
            .oneshot(with_range(
                from_page("GET", &stream, Some(&cookie), None),
                &format!("bytes={half}-"),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            r.headers()[header::CONTENT_RANGE],
            format!("bytes {half}-{}/{size}", size - 1).as_str()
        );
        assert_eq!(read_all(r.into_body()).await[..], plain[half..]);
        assert!(mp4_counts.most_open.load(Ordering::SeqCst) <= 1);

        // A Matroska file goes through the remuxer, its text subtitles
        // listed; it replaces the first viewer.
        let replace = Some(serde_json::json!({"episodeId": 9_130_005, "replaces": viewer_id(&body)}));
        let (st, body) = call(&s, post(replace)).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["mode"], "remux");
        let duration = body["duration"].as_i64().unwrap();
        assert!((19..=21).contains(&duration), "{duration}");
        assert_eq!(body["subtitles"][0]["text"], true, "{body}");
        let own = s.own_conns();
        assert_eq!(own.len(), 1, "the first viewer let its connection go");
        assert_eq!(own[0].feed, vod_feed(VodItem::episode(9_130_005)));
        until("the first file let go", || mp4_counts.open.load(Ordering::SeqCst) == 0).await;
        let viewer = viewer_id(&body);
        let stream = body["stream"].as_str().unwrap().to_string();
        let get = |q: &str| from_page("GET", &format!("{stream}{q}"), Some(&cookie), None);
        let first = router(s.clone()).oneshot(get("?from=0")).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(first.headers()[header::CONTENT_TYPE], "video/mp2t");
        let mut first = first.into_body().into_data_stream();
        assert_eq!(next_piece(&mut first).await.unwrap()[0], 0x47, "MPEG-TS");
        let asked = mkv_counts.firsts.lock().unwrap().len();
        // A seek past the middle reopens the file there.
        let second = router(s.clone()).oneshot(get("?from=12")).await.unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        let at = x_start(&second);
        assert!(at <= 12.0 && at > 10.9, "starts at {at}");
        while next_piece(&mut first).await.is_some() {}
        assert!(is_ts(&read_all(second.into_body()).await));
        let firsts = mkv_counts.firsts.lock().unwrap().clone();
        assert!(
            firsts[asked..].iter().any(|&f| f > film.len() / 2),
            "the second remux reads from the middle: {firsts:?} of {}",
            film.len()
        );
        assert_eq!(mkv_counts.most_open.load(Ordering::SeqCst), 1);
        // Its subtitles, as WebVTT in the episode's time.
        let mut cues = Vec::new();
        for _ in 0..50 {
            let uri = format!("/api/vod/subtitles/{viewer}/0");
            let (st, _, doc) = call_raw(&s, from_page("GET", &uri, Some(&cookie), None)).await;
            assert_eq!(st, StatusCode::OK);
            cues = vtt_cues(&String::from_utf8(doc).unwrap());
            if !cues.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(cues.iter().any(|c| (c.0 - 12.0).abs() < 0.1), "{cues:?}");
        let (st, status) =
            call(&s, from_page("GET", &format!("/api/vod/watch/{viewer}"), Some(&cookie), None)).await;
        assert_eq!(st, StatusCode::OK, "{status}");
        assert_eq!(status["ended"], serde_json::Value::Null);
        let (st, _) =
            call(&s, from_page("DELETE", &format!("/api/vod/watch/{viewer}"), Some(&cookie), None)).await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        assert!(s.own_conns().is_empty());

        // Refusals name the episode, in words of their own.
        let (st, body) = call(&s, post(episode_body(9_130_006))).await;
        assert_eq!(st, StatusCode::BAD_GATEWAY, "{body}");
        let said = body["error"].as_str().unwrap();
        assert!(said.starts_with("The episode didn't start: "), "{said}");
        let (st, body) = call(&s, post(episode_body(9_139_999))).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{body}");
        assert_eq!(body["error"], "That episode is not in the catalogue any more.");
        // A movie with an episode's id is no episode.
        let (st, body) = call(&s, post(movie_body(9_130_004))).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{body}");
        assert_eq!(body["error"], "That movie is not in the catalogue any more.");
        // The account's own words reach the page, never what follows them.
        seed_episode(9_130_007, 7, "mp4");
        let (st, body) = call(&s, post(episode_body(9_130_007))).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{body}");
        assert_eq!(
            body["error"],
            "Can't play this episode: that episode is not in the catalogue any more"
        );
        assert!(s.own_conns().is_empty());
    }

    #[tokio::test]
    async fn a_watch_names_one_title() {
        let s = server("vod-one-title");
        enable(&s, true).await;
        let cookie = browser_login(&s).await;
        for body in [
            serde_json::json!({"movieId": 9_130_001, "episodeId": 9_130_004}),
            serde_json::json!({}),
            serde_json::json!({"episodeId": "four"}),
        ] {
            let (st, said) =
                call(&s, from_page("POST", "/api/vod/watch", Some(&cookie), Some(body.clone()))).await;
            assert_eq!(st, StatusCode::BAD_REQUEST, "{body}: {said}");
            assert_eq!(said["error"], "Not a request to watch a movie or an episode.", "{body}");
        }
        assert!(s.own_conns().is_empty());
    }

    #[tokio::test]
    async fn an_id_no_catalogue_holds_is_not_looked_for() {
        let s = server("vod-out-of-range");
        enable(&s, true).await;
        let cookie = browser_login(&s).await;
        let base = crate::recording::vodwatch::EPISODE_FEED_BASE;
        for id in [0, -1, base] {
            let body = serde_json::json!({"episodeId": id});
            let (st, said) =
                call(&s, from_page("POST", "/api/vod/watch", Some(&cookie), Some(body.clone()))).await;
            assert_eq!(st, StatusCode::NOT_FOUND, "{body}: {said}");
            assert_eq!(said["error"], "That episode is not in the catalogue any more.", "{body}");
            assert!(s.own_conns().is_empty(), "{body}");
        }
    }

    // ------------------------------------------------- the app's own player

    fn player_body(stream_id: i64, only_shared: bool) -> Option<serde_json::Value> {
        Some(serde_json::json!({
            "streamId": stream_id, "player": true, "onlyShared": only_shared, "anyway": true
        }))
    }

    #[tokio::test]
    async fn the_apps_player_joins_a_recordings_connection_and_never_opens_one_when_told_not_to() {
        let s = server_with("watch-player", Box::new(Full));
        enable(&s, true).await;
        let (base, connections) = feed_server().await;
        feeds_from(&s, &base);
        let app = |body| req_body("POST", "/api/watch", HOME, false, Some(APP), body);

        // Nothing recorded on 66 (MPEG-2, which no browser plays): "only a
        // connection you have" opens none.
        let (st, body) = call(&s, app(player_body(66, true))).await;
        assert_eq!(st, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["reason"], "notOpen");
        assert_eq!(s.hubs.open_count(), 0);
        assert_eq!(connections.load(std::sync::atomic::Ordering::SeqCst), 0);

        // A recording of it: the player rides its connection, and is given
        // the stream as it comes, MPEG-2 and all.
        let rec = s.hubs.open(
            66,
            crate::source::Playable::plain(format!("{base}/mpeg2.ts")),
            crate::recording::sharing::Rider {
                id: 1,
                stop: now() + 3600,
                until: now() + 3900,
            },
        );
        let (st, body) = call(&s, app(player_body(66, true))).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["shared"], true);
        assert_eq!(body["converting"], false);
        let stream = body["stream"].as_str().unwrap().to_string();
        let (st, bytes) = read_some(&s, req("GET", &stream, HOME, false, Some(APP)), 188 * 20).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(bytes[0], 0x47);
        assert_eq!(s.hubs.conns()[0].riders.len(), 2);
        assert_eq!(connections.load(std::sync::atomic::Ordering::SeqCst), 1);

        // Letting go frees its place at once.
        let id = viewer_id(&body);
        let (st, _) = call(&s, req("DELETE", &format!("/api/watch/{id}"), HOME, false, Some(APP))).await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        until("the player leaves", || s.hubs.conns()[0].riders.len() == 1).await;
        drop(rec);

        // A browser is not the app's player.
        let cookie = browser_login(&s).await;
        let (st, _) = call(&s, from_page("POST", "/api/watch", Some(&cookie), player_body(5, false))).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
    }

    /// A few seconds of test pattern and tone as MPEG-TS, made with the
    /// ffmpeg command when there is one; `None` without it.
    fn ts_fixture(dir: &std::path::Path) -> Option<Vec<u8>> {
        let out = dir.join("pattern.ts");
        let made = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(["-f", "lavfi", "-i", "testsrc2=size=320x180:rate=25"])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"])
            .args(["-t", "4", "-map", "0:v", "-map", "1:a"])
            .args(["-c:v", "mpeg2video", "-g", "25", "-b:v", "500k", "-c:a", "mp2", "-f", "mpegts"])
            .arg(&out)
            .status()
            .ok()?
            .success();
        let bytes = made.then(|| std::fs::read(&out).ok()).flatten();
        let _ = std::fs::remove_file(&out);
        bytes
    }

    fn put_prelude(id: i64, first_ms: i64, token: &str, body: Vec<u8>) -> Request {
        let mut r = Request::builder()
            .method("PUT")
            .uri(format!("/api/recordings/{id}/prelude?firstMs={first_ms}"))
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "video/mp2t")
            .body(Body::from(body))
            .unwrap();
        r.extensions_mut().insert(Peer {
            addr: SocketAddr::new(HOME.parse().unwrap(), 40000),
            tls: false,
        });
        r
    }

    #[tokio::test]
    async fn a_players_buffer_becomes_the_first_part_and_is_finished_with_the_rest() {
        let s = server("prelude");
        let folder = s.dir_for_test().join("Recorded");
        std::fs::create_dir_all(&folder).unwrap();
        let Some(ts) = ts_fixture(&folder) else {
            eprintln!("skipped: no ffmpeg command to make a fixture with");
            return;
        };
        let t = now();
        let id = s
            .db(|c| {
                let req = ScheduleRequest {
                    stream_id: 7,
                    backup_stream_id: None,
                    channel_name: Some("One".into()),
                    title: "News".into(),
                    episode_title: None,
                    season: None,
                    episode: None,
                    start: t - 600,
                    stop: t + 3600,
                    pad_before: None,
                    pad_after: None,
                };
                store::insert(
                    c,
                    &store::NewRecording {
                        req: &req,
                        backup_stream_id: None,
                        channel_name: "One",
                        pad_before: 60,
                        pad_after: 300,
                        folder: &folder.to_string_lossy(),
                        base: "News",
                        clock_track: true,
                        now: t,
                    },
                )
            })
            .unwrap();
        // What the capture has received since, as it writes it.
        let live = crate::recording::api::PartDto {
            path: folder.join(".News - part1.ts").to_string_lossy().into_owned(),
            stream_id: 7,
            bytes: ts.len() as u64,
            started: t,
            ended: Some(t + 4),
            handed_over: false,
        };
        std::fs::write(&live.path, &ts).unwrap();
        s.db(|c| store::set_parts(c, id, std::slice::from_ref(&live))).unwrap();

        // Not a transport stream, or not from the app: refused, nothing kept.
        let (st, _) = call(&s, put_prelude(id, (t - 120) * 1000, APP, b"hello".repeat(100))).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        let cookie = {
            enable(&s, true).await;
            browser_login(&s).await
        };
        let mut r = from_page("PUT", &format!("/api/recordings/{id}/prelude?firstMs=1"), Some(&cookie), None);
        *r.body_mut() = Body::from(ts.clone());
        assert_eq!(call(&s, r).await.0, StatusCode::FORBIDDEN);
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 1, "nothing left behind");
        let (st, _) = call(&s, put_prelude(99, 0, APP, ts.clone())).await;
        assert_eq!(st, StatusCode::NOT_FOUND);

        // The app's: in front of what the capture received, with a clock log
        // placing it when it arrived.
        let first_ms = (t - 120) * 1000;
        let (st, part) = call(&s, put_prelude(id, first_ms, APP, ts.clone())).await;
        assert_eq!(st, StatusCode::CREATED, "{part}");
        let rec = s.get(id).unwrap();
        assert_eq!(rec.parts.len(), 2);
        assert_eq!(rec.parts[0].started, t - 120);
        assert!(rec.parts[0].handed_over, "kept out of what is followed while it records");
        assert!(
            std::path::Path::new(&rec.parts[0].path)
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.')),
            "hidden whatever the setting"
        );
        assert_eq!(rec.parts[0].bytes, ts.len() as u64);
        assert_eq!(rec.parts[1], live);
        let log = std::fs::read_to_string(crate::recording::capture::log_path(
            std::path::Path::new(&rec.parts[0].path),
        ))
        .unwrap();
        let arrivals = crate::recording::clock::parse_log(&log);
        assert!(arrivals.len() > 5, "{log}");
        assert!(arrivals[0].wall_ms >= first_ms && arrivals[0].wall_ms < first_ms + 1000);
        assert!(rec.notes.iter().any(|n| n.contains("already received")), "{:?}", rec.notes);

        // Finished, both are files of their own, in order.
        crate::recording::capture::finish(&s, id).await;
        let rec = s.get(id).unwrap();
        assert_eq!(rec.state, RecordingState::Done, "{:?}", rec.notes);
        let names: Vec<String> = rec
            .parts
            .iter()
            .map(|p| std::path::Path::new(&p.path).file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["News - part1.mkv", "News - part2.mkv"]);
        let left: Vec<String> = std::fs::read_dir(&folder)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with('.'))
            .collect();
        assert!(left.is_empty(), "{left:?}");

        // Once it is saved there is nothing to put a part in front of.
        let (st, _) = call(&s, put_prelude(id, first_ms, APP, ts)).await;
        assert_eq!(st, StatusCode::CONFLICT);
    }

    // ------------------------------------------------------------ off air

    /// A provider answering every stream as one does for a channel off the
    /// air: the tables (H.264 and AAC), a pause long enough for a page to be
    /// given its viewer, then ten minutes of stream at once, and the end.
    async fn placeholder_server() -> String {
        use crate::recording::hub::tests::stream_of;
        use crate::recording::tsprobe::tests::{pat, pmt};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = l.accept().await {
                tokio::spawn(async move {
                    let mut req = [0u8; 2048];
                    let _ = sock.read(&mut req).await;
                    let head =
                        "HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nConnection: close\r\n\r\n";
                    let mut first = pat(0x100);
                    first.extend(pmt(0x100, &[(0x1b, 0x101, vec![]), (0x0f, 0x102, vec![])]));
                    if sock.write_all(head.as_bytes()).await.is_err()
                        || sock.write_all(&first).await.is_err()
                    {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(400)).await;
                    let _ = sock.write_all(&stream_of(0.0, 600.0)).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        base
    }

    /// What the status route says of viewer `id` once it has ended.
    async fn ended_status(s: &Shared, cookie: &str, id: &str) -> serde_json::Value {
        let uri = format!("/api/watch/{id}");
        let t = std::time::Instant::now();
        loop {
            let (st, said) = call(s, from_page("GET", &uri, Some(cookie), None)).await;
            assert_eq!(st, StatusCode::OK, "{said}");
            if !said["ended"].is_null() {
                return said;
            }
            assert!(t.elapsed() < Duration::from_secs(10), "it never ended: {said}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn fail_reason(stream_id: i64) -> Option<String> {
        crate::catalog::db::with_db(|c| {
            Ok(c.query_row(
                "SELECT fail_reason FROM channel_quality WHERE stream_id = ?1",
                [stream_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten())
        })
        .unwrap()
    }

    #[tokio::test]
    async fn a_recording_waiting_to_ask_an_off_air_channel_again_is_in_no_browsers_way() {
        let s = server("off-air-retry-notice");
        enable(&s, true).await;
        let (base, _) = feed_server().await;
        feeds_from(&s, &base);
        let t = now();
        // A recording of channel 9, running, between its off-air tries.
        let id = s
            .db(|c| {
                let req = ScheduleRequest {
                    stream_id: 9,
                    backup_stream_id: None,
                    channel_name: Some("Nine".into()),
                    title: "Late Show".into(),
                    episode_title: None,
                    season: None,
                    episode: None,
                    start: t - 60,
                    stop: t + 3600,
                    pad_before: None,
                    pad_after: None,
                };
                let id = store::insert(
                    c,
                    &store::NewRecording {
                        req: &req,
                        backup_stream_id: None,
                        channel_name: "Nine",
                        pad_before: 0,
                        pad_after: 0,
                        folder: "/v",
                        base: "Late Show",
                        clock_track: false,
                        now: t,
                    },
                )?;
                store::set_state(c, id, RecordingState::Recording, None)?;
                Ok(id)
            })
            .unwrap();
        let (tx, _stop) = tokio::sync::watch::channel(false);
        s.live(|m| {
            m.insert(
                id,
                crate::recording::server::Live {
                    title: "Late Show".into(),
                    stream_id: 9,
                    bytes: 0,
                    since: t - 60,
                    waiting: None,
                    retry_at: Some(t + 120),
                    until: t + 3600,
                    finishing: false,
                    stop: tx,
                },
            )
        });
        let cookie = browser_login(&s).await;
        let (st, body) =
            call(&s, from_page("POST", "/api/watch", Some(&cookie), watch_body(5, false))).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert!(body["notice"].is_null(), "{body}");
        let status = format!("/api/watch/{}", viewer_id(&body));
        let (_, said) = call(&s, from_page("GET", &status, Some(&cookie), None)).await;
        assert!(said["notice"].is_null(), "no recording waits for this one: {said}");
        // The recordings list shows the retry, and no wait for a connection.
        let (_, list) = call(&s, req("GET", "/api/recordings", HOME, false, Some(APP))).await;
        assert_eq!(list[0]["retryAt"], t + 120);
        assert!(list[0].get("waiting").is_none(), "{list}");

        // The same recording waiting for a connection is: the test sees it.
        s.update_live(id, |l| {
            l.retry_at = None;
            l.waiting = Some("waiting for a free connection (1 of 1 in use)".into());
        });
        let (_, said) = call(&s, from_page("GET", &status, Some(&cookie), None)).await;
        assert_eq!(said["notice"]["recordingId"], id, "{said}");
    }

    #[tokio::test]
    async fn a_browser_watching_a_channel_off_air_is_told_so_and_so_is_everyone_on_it() {
        crate::catalog::db::install_test_db();
        let s = server("watch-off-air");
        enable(&s, true).await;
        let base = placeholder_server().await;
        s.set_playables(Arc::new(move |_| {
            Ok(crate::source::Playable::plain(format!("{base}/live.ts")))
        }));
        let cookie = browser_login(&s).await;
        let (st, body) =
            call(&s, from_page("POST", "/api/watch", Some(&cookie), watch_body(93_001, false))).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        let id = viewer_id(&body);
        // A recording on the same feed meanwhile: one connection, and it
        // learns the same.
        let (_, mut rec) = s
            .hubs
            .join(&[93_001], crate::recording::sharing::Rider { id: 1, stop: now() + 600, until: now() + 900 })
            .expect("the viewer's connection");
        assert_eq!(s.hubs.open_count(), 1);

        let said = ended_status(&s, &cookie, &id).await;
        assert_eq!(said["ended"]["kind"], "offAir", "{said}");
        assert_eq!(said["ended"]["clipSecs"], 600, "{said}");
        assert_eq!(said["takenOver"], false);
        // Its stream says it has ended rather than playing the clip.
        let (st, _) = call(&s, from_page("GET", &format!("/api/stream/{id}"), Some(&cookie), None)).await;
        assert_eq!(st, StatusCode::GONE);

        assert!(rec.recv().await.is_none(), "the recording reads none of the clip");
        assert!(matches!(
            rec.ended(),
            Some((crate::recording::fallback::Failure::OffAir { clip_secs: 600 }, _))
        ));
        until("the ledger says off air", || fail_reason(93_001).is_some()).await;
        assert_eq!(fail_reason(93_001).as_deref(), Some("off air"));

        // Asked again, it does not start: the page shows why all the same.
        let (st, body) =
            call(&s, from_page("POST", "/api/watch", Some(&cookie), watch_body(93_001, false))).await;
        if st == StatusCode::OK {
            // The tables came before the clip ended: the viewer ends as above.
            let said = ended_status(&s, &cookie, &viewer_id(&body)).await;
            assert_eq!(said["ended"]["kind"], "offAir");
        } else {
            assert_eq!(st, StatusCode::BAD_GATEWAY, "{body}");
            assert_eq!(body["ending"]["kind"], "offAir");
            assert_eq!(body["anyway"], false, "no other device is involved");
        }
    }

    #[tokio::test]
    async fn other_sources_for_a_channel_are_offered_without_provider_addresses() {
        use crate::catalog::db;
        let s = server("alternatives");
        enable(&s, true).await;
        db::install_test_db();
        let secret = "http://provider.example/u/secret-user/p/secret-pass";
        db::with_db(|c| {
            for (id, name, num) in [(94_101, "Lantern One", 1), (94_102, "Lantern One HD", 2), (94_103, "Lantern One East", 3)] {
                c.execute(
                    "INSERT INTO channels (stream_id, category_id, name, clean_name, num,
                                           epg_channel_id, logo_url, stream_url)
                     VALUES (?1, 'alt-94100', ?2, ?2, ?3, 'lantern.one', ?4, ?5)",
                    rusqlite::params![id, name, num, format!("{secret}/logo.png"), format!("{secret}/{id}.ts")],
                )?;
            }
            Ok(())
        })
        .unwrap();
        let cookie = browser_login(&s).await;
        let uri = "/api/channels/94101/alternatives";

        assert_eq!(
            call(&s, from_page("GET", uri, None, None)).await.0,
            StatusCode::UNAUTHORIZED,
            "behind the sign-in"
        );
        let (st, body) = call(&s, from_page("GET", uri, Some(&cookie), None)).await;
        assert_eq!(st, StatusCode::OK);
        let ids: Vec<i64> = body
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["streamId"].as_i64().unwrap())
            .collect();
        assert_eq!(ids, [94_102, 94_103], "the others with its guide id, by number");
        // The app's own shape: a channel as every channel route gives it.
        let first = &body[0];
        assert_eq!(first["name"], "Lantern One HD");
        assert_eq!(first["num"], 2);
        assert_eq!(first["logoUrl"], "/api/logo/94102");
        for key in ["cleanName", "categoryId", "hasArchive", "isSeparator", "quality", "nowPlaying"] {
            assert!(first.get(key).is_some(), "{key} in {first}");
        }
        let (_, _, raw) = call_raw(&s, from_page("GET", uri, Some(&cookie), None)).await;
        let raw = String::from_utf8(raw).unwrap();
        assert!(!raw.contains("secret") && !raw.contains("provider.example"), "{raw}");
        assert!(!raw.to_ascii_lowercase().contains("url\":\"http"), "{raw}");

        // An unknown channel has none; not a number is refused.
        let (st, none) = call(&s, from_page("GET", "/api/channels/94999/alternatives", Some(&cookie), None)).await;
        assert_eq!((st, none), (StatusCode::OK, serde_json::json!([])));
        let (st, _) = call(&s, from_page("GET", "/api/channels/x/alternatives", Some(&cookie), None)).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }
}
