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
use super::now;
use super::server::Server;
use super::{account, browse, web};

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
        .route("/api/recordings/{id}", get(one).delete(delete))
        .route("/api/recordings/{id}/cancel", post(cancel))
        .route("/api/settings", get(settings).put(set_settings))
        .route("/api/logout", post(logout))
        .merge(browse::routes())
        .merge(account::routes())
        .route_layer(middleware::from_fn_with_state(server.clone(), auth));
    let app = Router::new()
        .route("/api/network", get(network).put(set_network))
        .route("/api/network/password", put(set_password))
        .route("/api/devices", get(devices))
        .route("/api/devices/{id}", delete_route(revoke))
        .route("/api/shutdown", post(shutdown))
        .route_layer(middleware::from_fn_with_state(server.clone(), app_only));
    Router::new()
        .route("/api/version", get(version))
        .route("/api/login", post(login))
        .route("/api/session", get(session))
        .merge(authed)
        .merge(app)
        .fallback(web::files)
        .layer(middleware::from_fn(web::csrf))
        .layer(middleware::from_fn_with_state(server.clone(), gate))
        .layer(middleware::from_fn(web::headers))
        .with_state(server)
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use axum::body::Body;
    use tower::ServiceExt;

    use super::*;
    use crate::recording::budget::{Budget, Slots};
    use crate::recording::config::{self, ServerConfig};
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
}
