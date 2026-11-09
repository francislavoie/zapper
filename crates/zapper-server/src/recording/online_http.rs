//! Online services over HTTP, for the web interface: Settings › Online
//! services (OpenSubtitles on or off, its languages, the sign-in) and a
//! movie's or an episode's subtitles found on OpenSubtitles: searched,
//! downloaded, kept and read back as WebVTT. The app's own answers
//! ([`zapper_core::online::subtitles`]), from the process's own context.
//!
//! What reaches a browser is those answers and the kept files' text: never
//! the app's key, the OpenSubtitles password or token, or a download link.
//! The password comes in a request's JSON body alone, and is not logged. A
//! failure is answered with a fixed sentence; its detail goes to the
//! service's log, scrubbed.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::de::DeserializeOwned;
use serde::Deserialize;

use super::browse::{bad_request, Failed};
use zapper_core::catalog::sync_engine::scrub;
use zapper_core::catalog::vod_store::VodItem;
use zapper_core::online::http::OnlineError;
use zapper_core::online::subtitles::{self, Context};

const SEARCH_FAILED: &str = "OpenSubtitles could not be searched just now. Try again later.";
const DOWNLOAD_FAILED: &str = "The subtitle could not be downloaded just now. Try again later.";
const SIGN_IN_FAILED: &str = "OpenSubtitles could not be reached to sign in. Try again later.";
const SAVE_FAILED: &str = "The Online services settings could not be saved.";
const READ_FAILED: &str = "The Online services settings could not be read.";
const KEPT_FAILED: &str = "The subtitles kept for this title could not be read.";
const FILE_FAILED: &str = "The subtitle file could not be read.";

const BAD_KIND: &str = "kind: movie or episode";
const BAD_ID: &str = "id: a number";
const BAD_FILE_ID: &str = "fileId: an OpenSubtitles file number";
const BAD_BODY: &str = "The request's body is not what this asks for.";

/// The context the routes use: the process's own, or a test's.
#[derive(Clone, Default)]
struct Online(Option<Arc<Context>>);

impl Online {
    fn ctx(&self) -> &Arc<Context> {
        match &self.0 {
            Some(ctx) => ctx,
            None => Context::shared(),
        }
    }
}

/// The sentences [`subtitles`] answers with that hold nothing but
/// themselves, each with its status. Anything else is a detail for the log.
fn known(text: &str) -> Option<StatusCode> {
    let is = |e: OnlineError| text == e.to_string();
    if is(OnlineError::Off) {
        return Some(StatusCode::CONFLICT);
    }
    if is(OnlineError::SignInRefused { email: false })
        || is(OnlineError::SignInRefused { email: true })
    {
        return Some(StatusCode::BAD_REQUEST);
    }
    if is(OnlineError::SignInNeeded)
        || is(OnlineError::Quota { reset_at: None })
        || is(OnlineError::Busy { retry_at: 0 })
        || is(OnlineError::AnswerLost)
    {
        return Some(StatusCode::BAD_GATEWAY);
    }
    match text {
        subtitles::ENTER_SIGN_IN | subtitles::NOT_A_FILE => Some(StatusCode::BAD_REQUEST),
        subtitles::NOT_IN_CATALOGUE => Some(StatusCode::NOT_FOUND),
        subtitles::NOT_SUBTITLES | subtitles::DOWNLOAD_STOPPED => Some(StatusCode::BAD_GATEWAY),
        _ => None,
    }
}

/// `detail` as a browser is answered: one of [`known`]'s sentences as it
/// is; else `fallback` with `status`, and the detail logged scrubbed.
fn failed(what: &str, detail: String, status: StatusCode, fallback: &str) -> Failed {
    if let Some(known) = known(&detail) {
        return Failed(known, detail);
    }
    tracing::warn!("{what} for a browser: {}", scrub(&detail));
    Failed(status, fallback.to_owned())
}

/// `{kind}/{id}`: a movie or an episode.
fn item(kind: &str, id: &str) -> Result<VodItem, Failed> {
    let make = match kind {
        "movie" => VodItem::movie,
        "episode" => VodItem::episode,
        _ => return Err(bad_request(BAD_KIND)),
    };
    id.parse().map(make).map_err(|_| bad_request(BAD_ID))
}

fn file_id(text: &str) -> Result<i64, Failed> {
    text.parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| bad_request(BAD_FILE_ID))
}

/// A JSON body. Any refusal is a 400 with a fixed text, since serde's would
/// quote what was sent.
fn body<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, Failed> {
    serde_json::from_slice(bytes).map_err(|_| bad_request(BAD_BODY))
}

/// Runs a file or catalogue read off the async threads.
async fn blocking<T: Send + 'static>(
    what: &'static str,
    fallback: &'static str,
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, Failed> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r)
        .map_err(|e| failed(what, e, StatusCode::INTERNAL_SERVER_ERROR, fallback))
}

async fn status(State(o): State<Online>) -> Result<Response, Failed> {
    let ctx = Arc::clone(o.ctx());
    let dto = blocking("reading the online services", READ_FAILED, move || {
        Ok(subtitles::status_with(&ctx))
    })
    .await?;
    Ok(Json(dto).into_response())
}

/// An absent field is left as it is.
#[derive(Deserialize)]
struct SetOpenSubtitles {
    enabled: Option<bool>,
    languages: Option<Vec<String>>,
}

async fn set_opensubtitles(State(o): State<Online>, bytes: Bytes) -> Result<Response, Failed> {
    let SetOpenSubtitles { enabled, languages } = body(&bytes)?;
    let ctx = Arc::clone(o.ctx());
    let dto = blocking("saving the online services", SAVE_FAILED, move || {
        subtitles::set_opensubtitles_with(&ctx, enabled, languages)
    })
    .await?;
    Ok(Json(dto).into_response())
}

/// The sign-in a browser sends: never logged, never answered back, and no
/// `Debug`.
#[derive(Deserialize)]
struct SignIn {
    username: String,
    password: String,
}

async fn sign_in(State(o): State<Online>, bytes: Bytes) -> Result<Response, Failed> {
    let SignIn { username, password } = body(&bytes)?;
    let dto = subtitles::sign_in_with(o.ctx(), username, password)
        .await
        .map_err(|e| {
            let what = "signing in to OpenSubtitles";
            failed(what, e, StatusCode::BAD_GATEWAY, SIGN_IN_FAILED)
        })?;
    Ok(Json(dto).into_response())
}

async fn sign_out(State(o): State<Online>) -> Result<Response, Failed> {
    let dto = subtitles::sign_out_with(o.ctx()).await.map_err(|e| {
        let what = "signing out of OpenSubtitles";
        failed(what, e, StatusCode::INTERNAL_SERVER_ERROR, SAVE_FAILED)
    })?;
    Ok(Json(dto).into_response())
}

#[derive(Deserialize)]
struct LanguagesQuery {
    /// `en,fr`; none is the settings' languages.
    #[serde(default)]
    languages: String,
}

async fn search(
    State(o): State<Online>,
    Path((kind, id)): Path<(String, String)>,
    Query(q): Query<LanguagesQuery>,
) -> Result<Response, Failed> {
    let item = item(&kind, &id)?;
    let languages = q
        .languages
        .split(',')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect();
    let dto = subtitles::search_with(o.ctx(), item, languages)
        .await
        .map_err(|e| {
            failed(
                "searching OpenSubtitles",
                e,
                StatusCode::BAD_GATEWAY,
                SEARCH_FAILED,
            )
        })?;
    Ok(Json(dto).into_response())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Download {
    file_id: i64,
}

async fn download(
    State(o): State<Online>,
    Path((kind, id)): Path<(String, String)>,
    bytes: Bytes,
) -> Result<Response, Failed> {
    let item = item(&kind, &id)?;
    let Download { file_id } = body(&bytes)?;
    if file_id <= 0 {
        return Err(bad_request(BAD_FILE_ID));
    }
    let dto = subtitles::download_with(o.ctx(), item, file_id)
        .await
        .map_err(|e| {
            let what = "downloading a subtitle";
            failed(what, e, StatusCode::BAD_GATEWAY, DOWNLOAD_FAILED)
        })?;
    Ok(Json(dto).into_response())
}

async fn kept(Path((kind, id)): Path<(String, String)>) -> Result<Response, Failed> {
    let item = item(&kind, &id)?;
    let rows = blocking("reading a title's kept subtitles", KEPT_FAILED, move || {
        subtitles::kept(item)
    })
    .await?;
    Ok(Json(rows).into_response())
}

/// A kept file as WebVTT, for the page's `<track>`; a 404 unless the title
/// keeps it.
async fn file(
    State(o): State<Online>,
    Path((kind, id, file)): Path<(String, String, String)>,
) -> Result<Response, Failed> {
    let item = item(&kind, &id)?;
    let file_id = file_id(&file)?;
    let ctx = Arc::clone(o.ctx());
    let vtt = blocking("reading a kept subtitle file", FILE_FAILED, move || {
        subtitles::vtt_with(&ctx, item, file_id)
    })
    .await?;
    Ok(match vtt {
        Some(text) => (
            [
                (header::CONTENT_TYPE, "text/vtt; charset=utf-8"),
                (header::CACHE_CONTROL, "private, max-age=3600"),
            ],
            text,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    })
}

/// The routes, for the app or a signed-in device (`recording::http` puts
/// them behind its sign-in check), on the process's own context.
pub fn routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    router(Online::default())
}

/// The routes on a test's own context.
#[cfg(test)]
fn routes_with<S: Clone + Send + Sync + 'static>(ctx: Arc<Context>) -> Router<S> {
    router(Online(Some(ctx)))
}

fn router<S: Clone + Send + Sync + 'static>(online: Online) -> Router<S> {
    Router::new()
        .route("/api/online", get(status))
        .route("/api/online/opensubtitles", put(set_opensubtitles))
        .route("/api/online/opensubtitles/sign-in", post(sign_in))
        .route("/api/online/opensubtitles/sign-out", post(sign_out))
        .route("/api/subtitles/{kind}/{id}", get(search))
        .route("/api/subtitles/{kind}/{id}/download", post(download))
        .route("/api/subtitles/{kind}/{id}/kept", get(kept))
        .route("/api/subtitles/{kind}/{id}/file/{file_id}", get(file))
        .with_state(online)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    use axum::body::Body;
    use axum::http::{header, HeaderMap, Request, StatusCode};
    use axum::Router;
    use serde_json::{json, Value};
    use tower::ServiceExt;

    use super::*;
    use zapper_core::catalog::vod_store::VodItem;
    use zapper_core::catalog::{db, origin};
    use zapper_core::online::config;
    use zapper_core::online::http::Pacer;
    use zapper_core::online::opensubtitles::{Client, API_KEY};
    use zapper_core::online::subtitles::{self, Context};
    use zapper_core::test_http::{serve, Reply, Request as Seen, Server};

    const ORIGIN: &str = "xtream:online-http";
    const FILM_1: i64 = 960_001;
    const FILM_2: i64 = 960_002;
    const USERNAME: &str = "tester";
    const PASSWORD: &str = "fake-os-pass-960";

    /// Film 0001 (TMDB 900001) and Film 0002, under the test's account.
    fn seed() {
        db::install_test_db();
        db::with_db(|c| {
            c.execute_batch(&format!(
                "INSERT OR REPLACE INTO movies (movie_id, name, clean_name, year, added, tmdb_id)
                   VALUES ({FILM_1}, 'Film 0001', 'Film 0001', 2021, 0, 900001),
                          ({FILM_2}, 'Film 0002', 'Film 0002', 2019, 0, NULL);
                 DELETE FROM subtitle_downloads WHERE item_id IN ({FILM_1}, {FILM_2});"
            ))?;
            origin::record(c, ORIGIN)
        })
        .unwrap();
    }

    fn row(file_id: i64, lang: &str, release: &str, extra: &str) -> String {
        format!(
            r#"{{"id":"{file_id}","type":"subtitle","attributes":{{"language":"{lang}","download_count":10,"hearing_impaired":false,"foreign_parts_only":false,"from_trusted":false,"ai_translated":false,"machine_translated":false,"release":"{release}","feature_details":{{"feature_type":"Movie","tmdb_id":900001,"season_number":null,"episode_number":null}},"files":[{{"file_id":{file_id},"cd_number":1}}]{extra}}}}}"#
        )
    }

    /// Film 0001's rows: two in English, one machine-translated, one in
    /// French.
    fn film_1_page() -> String {
        let rows = [
            row(7000001, "en", "Film.0001.2021.1080p.TEST", "")
                .replace(r#""from_trusted":false"#, r#""from_trusted":true"#),
            row(7000002, "en", "Film.0001.2021.1080p.TEST.SDH", "")
                .replace(r#""hearing_impaired":false"#, r#""hearing_impaired":true"#),
            row(7000003, "en", "Film.0001.2021.AI", "")
                .replace(r#""ai_translated":false"#, r#""ai_translated":true"#),
            row(7000004, "fr", "Film.0001.2021.1080p.TEST", ""),
        ];
        format!(
            r#"{{"total_pages":1,"page":1,"data":[{}]}}"#,
            rows.join(",")
        )
    }

    fn srt(file_id: i64) -> String {
        format!(
            "1\n00:00:01,000 --> 00:00:03,000\nTest line 1 of file {file_id}\n\n\
             2\n00:00:03,000 --> 00:00:05,000\nTest line 2 of file {file_id}\n"
        )
    }

    /// OpenSubtitles as the tests have it: Film 0001 found by its id,
    /// `tester` signed in, and file 7000001 for every download.
    fn stand_in() -> Server {
        serve(|req: &Seen, base: &str| match req.route() {
            "/api/v1/subtitles" if req.query("tmdb_id") == Some("900001") => {
                Reply::ok(film_1_page())
            }
            "/api/v1/subtitles" => Reply::ok(r#"{"total_pages":1,"page":1,"data":[]}"#),
            "/api/v1/login" => Reply::ok(format!(
                r#"{{"token":"fake-os-token-960","base_url":"{}","user":{{"allowed_downloads":20}},"status":200}}"#,
                base.trim_start_matches("http://")
            )),
            "/api/v1/logout" => Reply::ok(r#"{"message":"token successfully destroyed"}"#),
            "/api/v1/download" => Reply::ok(format!(
                r#"{{"link":"{base}/files/7000001.srt","remaining":4,"reset_time_utc":"2036-11-08T12:00:00.000Z"}}"#
            )),
            "/files/7000001.srt" => Reply::ok(srt(7000001)),
            _ => Reply::new(404, ""),
        })
    }

    /// A test's scratch directory, removed when the test ends.
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The routes on a context of the test's own: its own pacer, search
    /// cache, settings file and files, at `server`.
    fn app(server: &Server, name: &str) -> (Router, Arc<Context>, Scratch) {
        let dir =
            std::env::temp_dir().join(format!("zapper-online-http-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = Arc::new(Context::new(
            Client::at_with(
                &format!("{}/api/v1", server.base),
                Pacer::new(4, 30, Duration::from_secs(10)),
            ),
            dir.join(config::FILE),
            dir.join("subtitles"),
        ));
        (routes_with(Arc::clone(&ctx)), ctx, Scratch(dir))
    }

    async fn send(
        app: &Router,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, HeaderMap, String) {
        let mut r = Request::builder().method(method).uri(uri);
        let body = match body {
            Some(b) => {
                r = r.header(header::CONTENT_TYPE, "application/json");
                Body::from(b.to_string())
            }
            None => Body::empty(),
        };
        let resp = app.clone().oneshot(r.body(body).unwrap()).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, headers, String::from_utf8(bytes.to_vec()).unwrap())
    }

    async fn send_json(
        app: &Router,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let (st, _, text) = send(app, method, uri, body).await;
        (st, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    fn sign_in_body() -> Option<Value> {
        Some(json!({ "username": USERNAME, "password": PASSWORD }))
    }

    fn film(id: i64, rest: &str) -> String {
        format!("/api/subtitles/movie/{id}{rest}")
    }

    #[tokio::test]
    async fn a_search_and_a_download_answer_as_the_app_does() {
        let _catalogue = db::movie_catalogue();
        seed();
        let server = stand_in();
        let (app, ctx, _scratch) = app(&server, "search");

        let (st, body) = send_json(&app, "GET", &film(FILM_1, "?languages=en"), None).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        let own = subtitles::search_with(&ctx, VodItem::movie(FILM_1), vec!["en".into()])
            .await
            .unwrap();
        assert_eq!(body, serde_json::to_value(&own).unwrap());
        assert_eq!(body["kind"], "results");
        let ids: Vec<i64> = body["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["fileId"].as_i64().unwrap())
            .collect();
        assert_eq!(ids, [7000001, 7000002], "the machine translation dropped");
        // Two languages, in their order.
        let (_, body) = send_json(&app, "GET", &film(FILM_1, "?languages=fr,en"), None).await;
        assert_eq!(body["languages"], json!(["fr", "en"]));

        let download = || Some(json!({ "fileId": 7000001 }));
        let (st, body) = send_json(&app, "POST", &film(FILM_1, "/download"), download()).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(
            body,
            json!({ "kind": "signIn", "refused": false, "email": false })
        );
        assert!(
            !server
                .requests()
                .iter()
                .any(|r| r.route() == "/api/v1/download"),
            "no download without a sign-in"
        );

        let (st, body) = send_json(
            &app,
            "POST",
            "/api/online/opensubtitles/sign-in",
            sign_in_body(),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["openSubtitles"]["signedInAs"], USERNAME);
        let (st, body) = send_json(&app, "POST", &film(FILM_1, "/download"), download()).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["kind"], "downloaded");
        assert_eq!(body["subtitle"]["fileId"], 7000001);
        assert_eq!(body["downloadsLeft"], 4);

        // Settings: what the app's own commands answer.
        let (st, body) = send_json(&app, "GET", "/api/online", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(
            body,
            serde_json::to_value(subtitles::status_with(&ctx)).unwrap()
        );
        let (st, body) = send_json(
            &app,
            "PUT",
            "/api/online/opensubtitles",
            Some(json!({ "languages": ["FR", "en"] })),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["openSubtitles"]["languages"], json!(["fr", "en"]));
        assert_eq!(body["openSubtitles"]["enabled"], true, "left as it was");
        let (_, body) = send_json(
            &app,
            "PUT",
            "/api/online/opensubtitles",
            Some(json!({ "enabled": false })),
        )
        .await;
        assert_eq!(body["openSubtitles"]["enabled"], false);
        assert_eq!(body["openSubtitles"]["languages"], json!(["fr", "en"]));
        let (_, body) = send_json(&app, "GET", &film(FILM_1, "?languages=en"), None).await;
        assert_eq!(body, json!({ "kind": "off" }));

        let (st, body) = send_json(&app, "POST", "/api/online/opensubtitles/sign-out", None).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["openSubtitles"]["signedInAs"], Value::Null);
    }

    #[tokio::test]
    async fn the_kept_file_is_webvtt_and_only_for_its_title() {
        let _catalogue = db::movie_catalogue();
        seed();
        let server = stand_in();
        let (app, _ctx, _scratch) = app(&server, "kept");
        let (_, body) = send_json(&app, "GET", &film(FILM_1, "/kept"), None).await;
        assert_eq!(body, json!([]));
        send_json(
            &app,
            "POST",
            "/api/online/opensubtitles/sign-in",
            sign_in_body(),
        )
        .await;
        // Found first, as the app does: the search describes the file kept.
        let (_, body) = send_json(&app, "GET", &film(FILM_1, "?languages=en"), None).await;
        assert_eq!(body["kind"], "results", "{body}");
        let (_, body) = send_json(
            &app,
            "POST",
            &film(FILM_1, "/download"),
            Some(json!({ "fileId": 7000001 })),
        )
        .await;
        assert_eq!(body["kind"], "downloaded", "{body}");

        let (st, body) = send_json(&app, "GET", &film(FILM_1, "/kept"), None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body.as_array().unwrap().len(), 1, "{body}");
        assert_eq!(body[0]["fileId"], 7000001);
        assert_eq!(body[0]["language"], "en");

        let (st, h, text) = send(&app, "GET", &film(FILM_1, "/file/7000001"), None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(h[header::CONTENT_TYPE], "text/vtt; charset=utf-8");
        assert_eq!(h[header::CACHE_CONTROL], "private, max-age=3600");
        assert!(text.starts_with("WEBVTT"), "{text}");
        assert!(text.contains("00:00:01.000 --> 00:00:03.000"), "{text}");
        assert!(text.contains("Test line 2 of file 7000001"), "{text}");

        // Another title, a file not kept, and a kind that is not this one.
        for uri in [
            film(FILM_2, "/file/7000001"),
            film(FILM_1, "/file/7000002"),
            format!("/api/subtitles/episode/{FILM_1}/file/7000001"),
        ] {
            let (st, _, _) = send(&app, "GET", &uri, None).await;
            assert_eq!(st, StatusCode::NOT_FOUND, "{uri}");
        }
        let (_, body) = send_json(&app, "GET", &film(FILM_2, "/kept"), None).await;
        assert_eq!(body, json!([]));
    }

    #[tokio::test]
    async fn a_bad_kind_or_id_is_400() {
        let _catalogue = db::movie_catalogue();
        seed();
        let server = stand_in();
        let (app, _ctx, _scratch) = app(&server, "bad");
        for (method, uri, body) in [
            ("GET", "/api/subtitles/film/1".to_owned(), None),
            ("GET", "/api/subtitles/series/1".to_owned(), None),
            ("GET", "/api/subtitles/movie/abc".to_owned(), None),
            ("GET", "/api/subtitles/film/1/kept".to_owned(), None),
            ("GET", "/api/subtitles/movie/abc/kept".to_owned(), None),
            (
                "POST",
                film(FILM_1, "/download"),
                Some(json!({ "fileId": "x" })),
            ),
            ("POST", film(FILM_1, "/download"), Some(json!({}))),
            ("POST", film(FILM_1, "/download"), None),
            (
                "POST",
                film(FILM_1, "/download"),
                Some(json!({ "fileId": 0 })),
            ),
            (
                "POST",
                "/api/subtitles/film/1/download".to_owned(),
                Some(json!({ "fileId": 7000001 })),
            ),
            ("GET", film(FILM_1, "/file/abc"), None),
            ("GET", film(FILM_1, "/file/-1"), None),
            ("GET", "/api/subtitles/film/1/file/7000001".to_owned(), None),
            (
                "PUT",
                "/api/online/opensubtitles".to_owned(),
                Some(json!({ "enabled": "yes" })),
            ),
            (
                "POST",
                "/api/online/opensubtitles/sign-in".to_owned(),
                Some(json!({ "username": USERNAME })),
            ),
        ] {
            let (st, body) = send_json(&app, method, &uri, body).await;
            assert_eq!(st, StatusCode::BAD_REQUEST, "{method} {uri}: {body}");
            assert!(body["error"].is_string(), "{method} {uri}: {body}");
        }
        assert!(server.requests().is_empty(), "{:?}", server.requests());
    }

    #[tokio::test]
    async fn errors_are_fixed_texts() {
        let _catalogue = db::movie_catalogue();
        seed();
        // OpenSubtitles failing, and refusing the sign-in.
        let server = serve(|req: &Seen, _: &str| match req.route() {
            "/api/v1/login" => Reply::new(401, r#"{"message":"Error, invalid username/password"}"#),
            _ => Reply::new(500, "detail 127.0.0.1 fake-os-pass-960"),
        });
        let (app, _ctx, _scratch) = app(&server, "errors");
        let (st, body) = send_json(&app, "GET", &film(FILM_1, "?languages=en"), None).await;
        assert_eq!(st, StatusCode::BAD_GATEWAY);
        assert_eq!(body, json!({ "error": SEARCH_FAILED }));
        let (st, body) = send_json(&app, "GET", &film(940_999_999, ""), None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], subtitles::NOT_IN_CATALOGUE);

        let (st, body) = send_json(
            &app,
            "POST",
            "/api/online/opensubtitles/sign-in",
            sign_in_body(),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert_eq!(
            body["error"],
            "OpenSubtitles did not accept that username and password."
        );
        let (st, body) = send_json(
            &app,
            "POST",
            "/api/online/opensubtitles/sign-in",
            Some(json!({ "username": " ", "password": PASSWORD })),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], subtitles::ENTER_SIGN_IN);

        // OpenSubtitles out of reach: the route's own sentence.
        let nowhere = Server {
            base: "http://127.0.0.1:9".into(),
            seen: Default::default(),
        };
        let (app, _ctx, _scratch) = self::app(&nowhere, "errors-unreachable");
        let (st, body) = send_json(
            &app,
            "POST",
            "/api/online/opensubtitles/sign-in",
            sign_in_body(),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_GATEWAY);
        assert_eq!(body, json!({ "error": SIGN_IN_FAILED }));
    }

    #[tokio::test]
    async fn no_online_answer_carries_a_secret() {
        let _catalogue = db::movie_catalogue();
        seed();
        let server = stand_in();
        let (app, _ctx, _scratch) = app(&server, "secrets");
        let mut seen = Vec::new();
        for (method, uri, body) in [
            (
                "POST",
                "/api/online/opensubtitles/sign-in".to_owned(),
                sign_in_body(),
            ),
            ("GET", "/api/online".to_owned(), None),
            (
                "PUT",
                "/api/online/opensubtitles".to_owned(),
                Some(json!({ "languages": ["en"] })),
            ),
            ("GET", film(FILM_1, "?languages=en"), None),
            (
                "POST",
                film(FILM_1, "/download"),
                Some(json!({ "fileId": 7000001 })),
            ),
            ("GET", film(FILM_1, "/kept"), None),
            ("GET", film(FILM_1, "/file/7000001"), None),
            (
                "POST",
                "/api/online/opensubtitles/sign-out".to_owned(),
                None,
            ),
        ] {
            let (st, h, text) = send(&app, method, &uri, body).await;
            assert!(st.is_success(), "{method} {uri}: {st} {text}");
            seen.push(format!("{method} {uri}: {h:?} {text}"));
        }
        assert!(
            server
                .requests()
                .iter()
                .any(|r| r.route() == "/files/7000001.srt"),
            "the file was fetched"
        );
        for answer in seen {
            for secret in ["fake-os-token", PASSWORD, API_KEY, "/files/", &server.base] {
                assert!(!answer.contains(secret), "{secret} in {answer}");
            }
        }
    }
}
