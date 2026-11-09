//! The provider account over HTTP: Settings → Account for a browser signed
//! in to the service (docs/recording.md, "API"), on the same rules as the
//! app's own pane ([`account_form`]).
//!
//! - `GET /api/account`: the account without its secrets. Whether a password
//!   is on file, never the password; a playlist or guide address with its
//!   login redacted. Not where the file is kept either: that is a path on
//!   this computer, and the browser may be on another.
//! - `PUT /api/account`: saves a form. A blank password, or `null` for a
//!   redacted address, keeps what is on file. Then the catalogue syncs,
//!   which also notices a change of catalogue (`catalog::origin`). The app
//!   on this computer reads the same file, and notices on its own.
//! - `POST /api/account/test`: tests a form as typed, writing nothing; with
//!   no body, the account on file.
//!
//! Behind the same sign-in and cross-site checks as the rest of the API
//! (`recording::http`): the app or a signed-in device. A body is never
//! logged, and an answer never repeats one: a form that does not parse gets
//! a fixed sentence rather than the parser's, which quotes what it read, and
//! an error has the typed password masked out on top of the usual scrub.

use axum::body::Bytes;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};

use zapper_core::catalog::{account_form, sync_engine};
use zapper_core::api::types::{AccountCandidateDto, AccountDto};

use super::access::Caller;
use zapper_core::recording::api::ErrorDto;

fn refused(status: StatusCode, text: String) -> Response {
    (status, Json(ErrorDto { error: text })).into_response()
}

/// What the form looks like from here: [`account_form::view`] without the
/// file's path.
fn view() -> AccountDto {
    AccountDto {
        path: String::new(),
        ..account_form::view()
    }
}

/// The form in a request's body, if it is one.
fn form(body: &[u8]) -> Option<AccountCandidateDto> {
    serde_json::from_slice(body).ok()
}

/// The answer to a body that is not a form: never the parser's words, which
/// quote what they could not read.
fn not_understood() -> Response {
    refused(
        StatusCode::BAD_REQUEST,
        "the account form was not understood".into(),
    )
}

/// What went wrong, with the typed secret taken out as well as the scrub's.
fn said(text: &str, secret: &str) -> String {
    account_form::mask_secret(&sync_engine::scrub(text), secret)
}

async fn account() -> Response {
    Json(view()).into_response()
}

async fn save(caller: Option<Extension<Caller>>, body: Bytes) -> Response {
    let Some(candidate) = form(&body) else {
        return not_understood();
    };
    let secret = account_form::typed_secret(&candidate).to_owned();
    match account_form::save(candidate) {
        Ok(kind) => {
            let who = match caller {
                Some(Extension(Caller::Device(id))) => format!("device {id}"),
                _ => "the app".to_owned(),
            };
            tracing::info!(
                "the account was changed by {who} ({}); syncing",
                kind.label()
            );
            // What the service knew of the old account's connections is not
            // this one's.
            zapper_core::connections::forget();
            sync_engine::request_now();
            Json(view()).into_response()
        }
        Err(e) => refused(StatusCode::BAD_REQUEST, said(&e, &secret)),
    }
}

async fn test(body: Bytes) -> Response {
    let answer = if body.iter().all(u8::is_ascii_whitespace) {
        account_form::test_saved().await
    } else {
        let Some(candidate) = form(&body) else {
            return not_understood();
        };
        let secret = account_form::typed_secret(&candidate).to_owned();
        account_form::test(candidate)
            .await
            .map_err(|e| said(&e, &secret))
    };
    match answer {
        Ok(text) => Json(serde_json::json!({ "result": text })).into_response(),
        // A form that cannot be tested, or an account that said no: either
        // way the page shows it as the test's failure.
        Err(e) => refused(StatusCode::BAD_REQUEST, e),
    }
}

/// The routes, for the app or a signed-in device (`recording::http` puts
/// them behind its sign-in check).
pub fn routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/api/account", get(account).put(save))
        .route("/api/account/test", post(test))
}
