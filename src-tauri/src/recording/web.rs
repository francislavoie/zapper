//! The web interface: zapper's own interface in a browser, served by the
//! service on the same origin as its API (docs/recording.md, "Web access").
//!
//! - **The files** are the frontend's build, compiled into the binary
//!   (`build.rs`): hashed assets under `/assets/` cached for a year, the rest
//!   revalidated each time, and any other path that is not the API answered
//!   with `index.html` so the interface's own addresses load it.
//! - **Signing in** gives the browser its device token as a cookie that page
//!   scripts cannot read ([`session_cookie`]), sent back on every request to
//!   this origin and no other.
//! - **Cross-site requests** ([`csrf`]): a cookie is sent whoever asks, so
//!   every request that changes something must show it came from this
//!   origin's own page.
//! - **Headers** ([`headers`]) on every answer: a policy that lets the page
//!   run only this origin's scripts and styles and be framed by nobody, and
//!   the rest that keep a browser from guessing types or leaking addresses.

use axum::extract::Request;
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::access::Peer;

/// `(path under dist/, contents)`, sorted by path.
static ASSETS: &[(&str, &[u8])] = include!(concat!(env!("OUT_DIR"), "/web_assets.rs"));

/// The cookie that carries a browser's device token.
pub const COOKIE: &str = "zapper_session";

/// The header the interface sends with every request that changes
/// something. A page on another site cannot add it to a request to this one
/// without the browser first asking this service (CORS), which never agrees.
pub const CSRF_HEADER: &str = "x-zapper";

/// How long a browser keeps its sign-in: a year, the most browsers allow
/// without shortening it. Revoking the device ends it sooner.
const COOKIE_MAX_AGE: u64 = 365 * 24 * 3600;

/// Only this origin's scripts, styles, images and requests; no plugins, no
/// `<base>` elsewhere, forms only to here, and no framing. Media also from
/// `blob:`: the page's player hands the video element a Media Source by a
/// `blob:` address it makes itself, filled from this origin's streams.
pub const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; \
    img-src 'self'; connect-src 'self'; font-src 'self'; media-src 'self' blob:; \
    object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

/// A channel logo, fetched from the provider and served from here: drawn as
/// an image it needs nothing, and opened on its own (an SVG can carry
/// script) it runs nothing.
pub const LOGO_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; sandbox";

// ------------------------------------------------------------------ files

fn asset(path: &str) -> Option<&'static [u8]> {
    ASSETS
        .binary_search_by(|(p, _)| p.cmp(&path))
        .ok()
        .map(|i| ASSETS[i].1)
}

/// Whether the interface was built into this binary.
pub fn built() -> bool {
    asset("index.html").is_some()
}

fn content_type(path: &str) -> &'static str {
    let ext = path.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    match ext.to_ascii_lowercase().as_str() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

/// Vite names everything under `assets/` by its contents' hash, so a file
/// there never changes and may be kept as long as a browser likes. The rest
/// (`index.html` above all, which names the current hashes) is asked about
/// again every time.
fn cache_control(path: &str) -> &'static str {
    if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    }
}

/// What a path that is not a file gets: the interface's own addresses load
/// it, and a missing script or image is a plain 404 rather than a page of
/// HTML a browser would try to run as one.
fn spa_fallback(path: &str) -> bool {
    let last = path.rsplit('/').next().unwrap_or("");
    !path.starts_with("assets/") && !last.contains('.')
}

const NOT_BUILT: &str = "<!doctype html><meta charset=utf-8><title>zapper</title>\
    <p>This zapper-server was built without its web interface. Build the frontend \
    (<code>npm run build</code>) and then zapper-server again.</p>";

/// Everything that is not the API: a file of the interface, or the
/// interface itself for one of its own addresses.
pub async fn files(method: Method, uri: axum::http::Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path == "api" || path.starts_with("api/") {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(super::api::ErrorDto {
                error: "no such API request".into(),
            }),
        )
            .into_response();
    }
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let path = if path.is_empty() { "index.html" } else { path };
    let (path, body) = match asset(path) {
        Some(b) => (path, b),
        None if spa_fallback(path) => match asset("index.html") {
            Some(b) => ("index.html", b),
            None => {
                return (
                    [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                    NOT_BUILT,
                )
                    .into_response()
            }
        },
        None => return StatusCode::NOT_FOUND.into_response(),
    };
    (
        [
            (header::CONTENT_TYPE, content_type(path)),
            (header::CACHE_CONTROL, cache_control(path)),
        ],
        body,
    )
        .into_response()
}

// ---------------------------------------------------------------- headers

/// The headers every answer carries, API and files alike. A handler that set
/// a stricter policy of its own (a logo's) keeps it.
pub async fn headers(req: Request, next: Next) -> Response {
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    let set = |h: &mut HeaderMap, name: HeaderName, value: &'static str| {
        h.entry(name).or_insert(HeaderValue::from_static(value));
    };
    set(h, header::CONTENT_SECURITY_POLICY, CSP);
    set(h, header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    set(h, header::REFERRER_POLICY, "no-referrer");
    set(h, header::X_FRAME_OPTIONS, "DENY");
    set(
        h,
        HeaderName::from_static("cross-origin-opener-policy"),
        "same-origin",
    );
    set(
        h,
        HeaderName::from_static("cross-origin-resource-policy"),
        "same-origin",
    );
    resp
}

// ---------------------------------------------------------------- cookies

/// The `Set-Cookie` that signs a browser in: not readable by scripts, sent
/// only to this origin and only from its own pages, and over HTTPS only when
/// the sign-in came over HTTPS (the app's own computer is plain HTTP on
/// 127.0.0.1, where a `Secure` cookie would never be sent back).
pub fn session_cookie(token: &str, tls: bool) -> String {
    format!(
        "{COOKIE}={token}; Path=/; Max-Age={COOKIE_MAX_AGE}; HttpOnly; SameSite=Strict{}",
        if tls { "; Secure" } else { "" }
    )
}

/// The `Set-Cookie` that forgets it.
pub fn cleared_cookie(tls: bool) -> String {
    format!(
        "{COOKIE}=; Path=/; Max-Age=0; HttpOnly; SameSite=Strict{}",
        if tls { "; Secure" } else { "" }
    )
}

/// The device token in the request's cookie, if it has one.
pub fn cookie_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE)
        .map(|(_, value)| value.trim())
        .filter(|v| !v.is_empty())
}

// ------------------------------------------------------------------- CSRF

/// Whether `origin` (an `Origin` header) is the origin the request was made
/// to: the scheme of the connection and the `Host` it named.
pub fn same_origin(origin: &str, host: Option<&str>, tls: bool) -> bool {
    let Some(host) = host.map(str::trim).filter(|h| !h.is_empty()) else {
        return false;
    };
    let scheme = if tls { "https" } else { "http" };
    let Some(rest) = origin.strip_prefix(scheme).and_then(|r| r.strip_prefix("://")) else {
        return false;
    };
    rest.eq_ignore_ascii_case(host)
}

/// Why a request that changes something is refused, or `None`.
///
/// Reading (`GET`, `HEAD`) changes nothing and passes. Otherwise a browser
/// says where the request came from: an `Origin` must be this one, and a
/// `Sec-Fetch-Site` must say the same origin (or `none`, the person's own
/// doing). Programs that are not browsers send neither, and use a bearer
/// token no page can attach for them; a request that relies on the cookie,
/// which a browser attaches by itself, must also carry [`CSRF_HEADER`].
pub fn csrf_refusal(method: &Method, headers: &HeaderMap, tls: bool) -> Option<&'static str> {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return None;
    }
    let text = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    if let Some(origin) = headers.get(header::ORIGIN) {
        let host = text(header::HOST.as_str());
        if !origin
            .to_str()
            .is_ok_and(|o| same_origin(o, host, tls))
        {
            return Some("refused: the request came from another site");
        }
    }
    if let Some(site) = text("sec-fetch-site") {
        if site != "same-origin" && site != "none" {
            return Some("refused: the request came from another site");
        }
    }
    let bearer = headers.contains_key(header::AUTHORIZATION);
    if !bearer && cookie_token(headers).is_some() && !has_csrf_header(headers) {
        return Some("refused: the request did not come from zapper's own page");
    }
    None
}

/// Whether the request carries [`CSRF_HEADER`] as the interface sends it.
pub fn has_csrf_header(headers: &HeaderMap) -> bool {
    headers
        .get(CSRF_HEADER)
        .is_some_and(|v| v.as_bytes() == b"1")
}

/// Every request: [`csrf_refusal`].
pub async fn csrf(req: Request, next: Next) -> Response {
    let tls = req.extensions().get::<Peer>().is_some_and(|p| p.tls);
    if let Some(why) = csrf_refusal(req.method(), req.headers(), tls) {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(super::api::ErrorDto { error: why.into() }),
        )
            .into_response();
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(
                HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn the_cookie_is_secure_only_over_tls() {
        let c = session_cookie("abc", true);
        assert_eq!(
            c,
            "zapper_session=abc; Path=/; Max-Age=31536000; HttpOnly; SameSite=Strict; Secure"
        );
        let c = session_cookie("abc", false);
        assert!(c.contains("HttpOnly") && c.contains("SameSite=Strict") && c.contains("Path=/"));
        assert!(!c.contains("Secure"), "{c}");
        assert!(cleared_cookie(true).contains("Max-Age=0"));
        assert!(cleared_cookie(true).ends_with("; Secure"));
    }

    #[test]
    fn the_token_is_read_from_among_other_cookies() {
        let h = headers(&[("cookie", "a=1; zapper_session=t0k; b=2")]);
        assert_eq!(cookie_token(&h), Some("t0k"));
        let h = headers(&[("cookie", "a=1"), ("cookie", "zapper_session=x")]);
        assert_eq!(cookie_token(&h), Some("x"));
        assert_eq!(cookie_token(&headers(&[("cookie", "zapper_session=")])), None);
        assert_eq!(cookie_token(&headers(&[("cookie", "xzapper_session=1")])), None);
        assert_eq!(cookie_token(&HeaderMap::new()), None);
    }

    #[test]
    fn same_origin_is_the_scheme_and_host_asked_for() {
        assert!(same_origin("https://192.168.1.20:8796", Some("192.168.1.20:8796"), true));
        assert!(same_origin("http://127.0.0.1:8796", Some("127.0.0.1:8796"), false));
        assert!(same_origin("https://TV.lan:8796", Some("tv.lan:8796"), true));
        assert!(!same_origin("http://192.168.1.20:8796", Some("192.168.1.20:8796"), true));
        assert!(!same_origin("https://evil.example", Some("192.168.1.20:8796"), true));
        assert!(!same_origin("https://192.168.1.20:8797", Some("192.168.1.20:8796"), true));
        assert!(!same_origin("null", Some("192.168.1.20:8796"), true));
        assert!(!same_origin("https://192.168.1.20:8796", None, true));
        assert!(!same_origin(
            "https://192.168.1.20:8796.evil.example",
            Some("192.168.1.20:8796"),
            true
        ));
    }

    #[test]
    fn changes_need_the_header_and_this_origin() {
        let post = Method::POST;
        let host = ("host", "192.168.1.20:8796");
        let cookie = ("cookie", "zapper_session=t");
        let mine = ("origin", "https://192.168.1.20:8796");
        let ours = ("x-zapper", "1");
        // Reading never needs either.
        assert_eq!(csrf_refusal(&Method::GET, &headers(&[cookie]), true), None);
        // The interface's own request.
        assert_eq!(
            csrf_refusal(&post, &headers(&[host, cookie, mine, ours, ("sec-fetch-site", "same-origin")]), true),
            None
        );
        // Without the header: a form or an <img> elsewhere could have sent it.
        assert!(csrf_refusal(&post, &headers(&[host, cookie, mine]), true).is_some());
        assert!(csrf_refusal(&post, &headers(&[host, cookie, mine, ("x-zapper", "0")]), true).is_some());
        // From another site, header or not.
        for bad in [
            vec![host, cookie, ours, ("origin", "https://evil.example")],
            vec![host, cookie, ours, ("origin", "null")],
            vec![host, cookie, ours, ("sec-fetch-site", "cross-site")],
            vec![host, cookie, ours, ("sec-fetch-site", "same-site")],
            vec![cookie, ours, mine],
        ] {
            assert!(csrf_refusal(&post, &headers(&bad), true).is_some(), "{bad:?}");
        }
        assert!(
            csrf_refusal(&Method::DELETE, &headers(&[host, cookie, ("origin", "https://evil.example")]), true).is_some()
        );
        // A program with a bearer token, which no page can attach.
        assert_eq!(
            csrf_refusal(&post, &headers(&[("authorization", "Bearer t")]), false),
            None
        );
        // Nothing to ride on: no cookie, no token (signing in, say).
        assert_eq!(csrf_refusal(&post, &headers(&[host]), true), None);
    }

    #[test]
    fn hashed_assets_are_kept_and_the_rest_revalidated() {
        assert_eq!(
            cache_control("assets/index-CTu78SET.js"),
            "public, max-age=31536000, immutable"
        );
        assert_eq!(cache_control("index.html"), "no-cache");
        assert_eq!(cache_control("boot.js"), "no-cache");
        assert_eq!(content_type("assets/a.js"), "text/javascript; charset=utf-8");
        assert_eq!(content_type("index.html"), "text/html; charset=utf-8");
        assert_eq!(content_type("licenses/LICENSE.txt"), "text/plain; charset=utf-8");
    }

    #[test]
    fn only_the_interfaces_own_addresses_fall_back_to_it() {
        assert!(spa_fallback("guide"));
        assert!(spa_fallback("recordings/3"));
        assert!(!spa_fallback("assets/gone"));
        assert!(!spa_fallback("assets/index-old.js"));
        assert!(!spa_fallback("favicon.ico"));
    }

    #[test]
    fn the_assets_are_sorted_for_lookup() {
        assert!(ASSETS.windows(2).all(|w| w[0].0 < w[1].0));
    }
}
