//! The service's HTTP API (docs/recording.md).
//!
//! Every request first passes [`access::admit`]: from another machine only
//! while network access is on, and over HTTPS unless plain HTTP was chosen.
//! Then every route but `/api/version` and `/api/login` wants
//! `Authorization: Bearer <token>`: the app's token (`server-token`, from
//! this computer only) or a device's, from signing in. Network access, the
//! password and the list of devices are the app's alone.
//!
//! No CORS headers: a page from another origin cannot read any answer. The
//! web client will be served from here, on the same origin.

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
async fn gate(State(s): State<Shared>, req: Request, next: Next) -> Response {
    let p = peer(&req);
    if let Err(r) = access::admit(&p, &s.config().network) {
        return ApiError::from(r).into_response();
    }
    next.run(req).await
}

/// Who it is: the app, or a signed-in device.
fn caller(s: &Server, req: &Request) -> Result<Caller, Refusal> {
    let p = peer(req);
    let token = bearer(req);
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
    match s
        .access
        .login(p.addr.ip(), &req.password, req.name.as_deref(), ua, now())
        .await
    {
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

/// Signs the calling device out.
async fn logout(
    State(s): State<Shared>,
    Extension(c): Extension<Caller>,
) -> Result<impl IntoResponse, ApiError> {
    let Caller::Device(id) = c else {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "the app is not signed in; there is nothing to sign out of".into(),
        ));
    };
    s.access.devices(|d| d.revoke(&id)).map_err(bad)?;
    Ok(Json(serde_json::json!({ "signedOut": true })))
}

pub fn router(server: Shared) -> Router {
    let authed = Router::new()
        .route("/api/status", get(status))
        .route("/api/recordings", get(list).post(schedule))
        .route("/api/recordings/{id}", get(one).delete(delete))
        .route("/api/recordings/{id}/cancel", post(cancel))
        .route("/api/settings", get(settings).put(set_settings))
        .route("/api/logout", post(logout))
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
        .merge(authed)
        .merge(app)
        .layer(middleware::from_fn_with_state(server.clone(), gate))
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
}
