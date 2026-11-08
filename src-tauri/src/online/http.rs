//! How zapper asks an outside service anything: one HTTP client shape, a
//! pacer per service, and one way of sending that keeps to the service's
//! rate limit and keeps URLs out of errors and logs.
//!
//! [`send`] waits its turn on the service's [`Pacer`], and answers
//! [`OnlineError::Busy`] at once rather than wait longer than [`MAX_WAIT`]:
//! somebody is looking at a menu. A 429 blocks the pacer until its
//! `Retry-After`, so nothing else is sent to that service before then; a
//! short one is waited out and retried once. A 502 or 503 is retried once
//! after half a second. Each exchange is logged as its method, host, path,
//! status and the service's `x-ratelimit-remaining`, and nothing else: no
//! query, no header, no body.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reqwest::header::{HeaderMap, RETRY_AFTER};

use crate::catalog::sync_engine::scrub;

/// What every request says it is.
pub const USER_AGENT: &str = concat!("zapper v", env!("CARGO_PKG_VERSION"));

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The whole exchange, body included.
pub const TIMEOUT: Duration = Duration::from_secs(20);
/// The longest [`send`] waits for its turn; past it, the answer is
/// [`OnlineError::Busy`].
pub const MAX_WAIT: Duration = Duration::from_secs(5);
/// Redirects followed, at most.
pub const MAX_HOPS: usize = 3;
/// A 429 that does not say how long to wait.
const DEFAULT_RETRY: Duration = Duration::from_secs(10);
/// The longest a `Retry-After` is honoured for.
const MAX_RETRY: Duration = Duration::from_secs(3600);
/// Before asking again after a 502 or 503.
const UNAVAILABLE_PAUSE: Duration = Duration::from_millis(500);

/// The HTTP client for an outside service: [`USER_AGENT`], 10 s to connect
/// and 20 s for the whole exchange, and redirects followed at most
/// [`MAX_HOPS`] times, each only to an OpenSubtitles host (or the stand-in
/// named by [`super::opensubtitles::API_OVERRIDE`]), where the download link
/// leads. A redirect elsewhere is answered as it is, unfollowed.
pub fn client() -> reqwest::Client {
    let base = super::opensubtitles::api_base();
    client_following(move |url| super::opensubtitles::file_link_allowed(url, &base))
}

/// [`client`], with redirects followed only to the URLs `allowed` accepts.
pub fn client_following(allowed: impl Fn(&str) -> bool + Send + Sync + 'static) -> reqwest::Client {
    let policy = reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() > MAX_HOPS || !allowed(attempt.url().as_str()) {
            attempt.stop()
        } else {
            attempt.follow()
        }
    });
    let built = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TIMEOUT)
        // The page a redirect came from is nobody else's business.
        .referer(false)
        .redirect(policy)
        .build();
    built.unwrap_or_else(|e| {
        tracing::warn!("building the online services' client: {}", e.without_url());
        reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default()
    })
}

/// What [`Pacer::admit`] decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admit {
    /// Send now; the request is counted.
    Go,
    /// Ask again after this long (at most [`MAX_WAIT`]).
    Wait(Duration),
    /// Not before `until`, which is further off than [`MAX_WAIT`].
    Busy { until: Instant },
}

/// A service's rate limit, kept on this side: so many requests a second, and
/// so many in a longer window; and, after a 429, none at all until the
/// service said.
pub struct Pacer {
    per_second: u32,
    per_window: u32,
    window: Duration,
    state: Mutex<Paced>,
}

struct Paced {
    /// When each request still inside the window was let through, oldest
    /// first.
    sent: VecDeque<Instant>,
    blocked: Option<Instant>,
}

impl Pacer {
    /// A limit of 0 is no limit.
    pub const fn new(per_second: u32, per_window: u32, window: Duration) -> Self {
        Self {
            per_second,
            per_window,
            window,
            state: Mutex::new(Paced {
                sent: VecDeque::new(),
                blocked: None,
            }),
        }
    }

    /// May a request go at `now`? A [`Admit::Go`] counts it.
    pub fn admit(&self, now: Instant) -> Admit {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let second = Duration::from_secs(1);
        let kept = self.window.max(second);
        while s
            .sent
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= kept)
        {
            s.sent.pop_front();
        }
        let mut wait = s
            .blocked
            .map_or(Duration::ZERO, |until| until.saturating_duration_since(now));
        // The n-th newest request decides when one more fits in n.
        let nth_newest = |n: u32| -> Option<Instant> {
            let n = n as usize;
            (n > 0 && s.sent.len() >= n).then(|| s.sent[s.sent.len() - n])
        };
        if let Some(t) = nth_newest(self.per_second) {
            wait = wait.max((t + second).saturating_duration_since(now));
        }
        if let Some(t) = nth_newest(self.per_window) {
            wait = wait.max((t + self.window).saturating_duration_since(now));
        }
        if wait.is_zero() {
            s.sent.push_back(now);
            Admit::Go
        } else if wait > MAX_WAIT {
            Admit::Busy { until: now + wait }
        } else {
            Admit::Wait(wait)
        }
    }

    /// Nothing goes before `until` (a later block already in place stays).
    pub fn block_until(&self, until: Instant) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.blocked = Some(s.blocked.map_or(until, |b| b.max(until)));
    }
}

/// A `Retry-After` in seconds, capped at an hour. `None` when there is none
/// or it is a date; a 429 then waits 10 s.
pub fn retry_after(h: &HeaderMap) -> Option<Duration> {
    let secs: u64 = h.get(RETRY_AFTER)?.to_str().ok()?.trim().parse().ok()?;
    Some(Duration::from_secs(secs).min(MAX_RETRY))
}

/// Why an outside service gave nothing. `Display` is the sentence the
/// interface shows; the texts carried are scrubbed and hold no URL.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OnlineError {
    #[error("This service is turned off in Settings › Online services.")]
    Off,
    #[error("Sign in to OpenSubtitles to download subtitles.")]
    SignInNeeded,
    #[error("{}", refused_text(.email))]
    SignInRefused { email: bool },
    /// Unix seconds when downloads are allowed again, when the service said.
    #[error("That is all of today's OpenSubtitles downloads for this account.")]
    Quota { reset_at: Option<i64> },
    /// Unix seconds.
    #[error("The service is busy. Try again in a moment.")]
    Busy { retry_at: i64 },
    #[error("The service refused the request (HTTP {0}).")]
    Refused(u16),
    #[error("The service could not be reached: {0}")]
    Unreachable(String),
    #[error("The service's answer could not be read: {0}")]
    Invalid(String),
}

fn refused_text(email: &bool) -> &'static str {
    if *email {
        "OpenSubtitles signs in with your username, not your email address."
    } else {
        "OpenSubtitles did not accept that username and password."
    }
}

impl OnlineError {
    /// A transport failure, without its URL and scrubbed.
    pub fn unreachable(e: reqwest::Error) -> Self {
        Self::Unreachable(scrub(&describe(e)))
    }

    pub fn invalid(text: impl AsRef<str>) -> Self {
        Self::Invalid(scrub(text.as_ref()))
    }
}

/// reqwest's error and its causes, without the URL it would name.
fn describe(e: reqwest::Error) -> String {
    let e = e.without_url();
    let mut text = e.to_string();
    let mut source = std::error::Error::source(&e);
    while let Some(s) = source {
        text.push_str(": ");
        text.push_str(&s.to_string());
        source = s.source();
    }
    text
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// `until` as Unix seconds, rounded up.
fn unix_at(until: Instant) -> i64 {
    let left = until.saturating_duration_since(Instant::now());
    unix_now() + left.as_secs_f64().ceil() as i64
}

async fn take_turn(pacer: &Pacer) -> Result<(), OnlineError> {
    loop {
        match pacer.admit(Instant::now()) {
            Admit::Go => return Ok(()),
            Admit::Wait(d) => tokio::time::sleep(d).await,
            Admit::Busy { until } => {
                return Err(OnlineError::Busy {
                    retry_at: unix_at(until),
                })
            }
        }
    }
}

/// Sends `req` on `pacer`'s terms (see the module's notes). Any answer but a
/// 429 is returned as it is; the caller reads its status.
pub async fn send(
    pacer: &Pacer,
    req: reqwest::RequestBuilder,
) -> Result<reqwest::Response, OnlineError> {
    exchange(pacer, req, true).await
}

/// [`send`] for a temporary link, whose path is not logged either.
pub(crate) async fn send_link(
    pacer: &Pacer,
    req: reqwest::RequestBuilder,
) -> Result<reqwest::Response, OnlineError> {
    exchange(pacer, req, false).await
}

async fn exchange(
    pacer: &Pacer,
    req: reqwest::RequestBuilder,
    show_path: bool,
) -> Result<reqwest::Response, OnlineError> {
    let (http, request) = req.build_split();
    let request = request.map_err(|e| OnlineError::invalid(describe(e)))?;
    let method = request.method().clone();
    let host = request.url().host_str().unwrap_or("?").to_owned();
    let path = if show_path {
        request.url().path().to_owned()
    } else {
        " (a download link)".to_owned()
    };
    let mut retried = false;
    loop {
        take_turn(pacer).await?;
        let Some(attempt) = request.try_clone() else {
            return Err(OnlineError::invalid("the request could not be repeated"));
        };
        let resp = match http.execute(attempt).await {
            Ok(r) => r,
            Err(e) => {
                let e = OnlineError::unreachable(e);
                tracing::info!("{method} {host}{path}: {e}");
                return Err(e);
            }
        };
        let status = resp.status().as_u16();
        let remaining = resp
            .headers()
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            .map(|v| format!(" ({v} left)"))
            .unwrap_or_default();
        tracing::info!("{method} {host}{path} → {status}{remaining}");
        match status {
            429 => {
                let wait = retry_after(resp.headers()).unwrap_or(DEFAULT_RETRY);
                let until = Instant::now() + wait;
                pacer.block_until(until);
                if retried || wait > MAX_WAIT {
                    return Err(OnlineError::Busy {
                        retry_at: unix_at(until),
                    });
                }
                retried = true;
            }
            502 | 503 if !retried => {
                retried = true;
                tokio::time::sleep(UNAVAILABLE_PAUSE).await;
            }
            _ => return Ok(resp),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue};
    use std::time::{Duration, Instant};

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn the_pacer_spaces_requests() {
        let p = Pacer::new(4, 30, Duration::from_secs(10));
        let t0 = Instant::now();
        for _ in 0..4 {
            assert_eq!(p.admit(t0), Admit::Go);
        }
        match p.admit(t0 + ms(100)) {
            Admit::Wait(d) => assert!(d <= ms(900) && d > Duration::ZERO, "{d:?}"),
            other => panic!("the fifth in a second should wait: {other:?}"),
        }
        // A second on, the first four are out of the second.
        assert_eq!(p.admit(t0 + ms(1000)), Admit::Go);

        // Thirty in ten seconds, four a second: the window then holds the
        // next one back until the oldest leaves it.
        let p = Pacer::new(4, 30, Duration::from_secs(10));
        for i in 0..30 {
            assert_eq!(p.admit(t0 + ms(250 * i)), Admit::Go, "request {i}");
        }
        match p.admit(t0 + ms(7500)) {
            Admit::Wait(d) => assert_eq!(d, ms(2500)),
            other => panic!("{other:?}"),
        }

        // A wait longer than MAX_WAIT is Busy, until the window frees up.
        let p = Pacer::new(10, 30, Duration::from_secs(10));
        for i in 0..30 {
            assert_eq!(p.admit(t0 + ms(100 * i)), Admit::Go, "request {i}");
        }
        assert_eq!(
            p.admit(t0 + ms(3000)),
            Admit::Busy {
                until: t0 + Duration::from_secs(10)
            }
        );

        // Blocked (after a 429): every request waits, or is Busy past MAX_WAIT.
        let p = Pacer::new(4, 30, Duration::from_secs(10));
        p.block_until(t0 + Duration::from_secs(3));
        assert_eq!(p.admit(t0), Admit::Wait(Duration::from_secs(3)));
        p.block_until(t0 + Duration::from_secs(30));
        assert_eq!(
            p.admit(t0),
            Admit::Busy {
                until: t0 + Duration::from_secs(30)
            }
        );
        // An earlier block does not shorten a later one.
        p.block_until(t0 + Duration::from_secs(1));
        assert!(matches!(p.admit(t0), Admit::Busy { .. }));
        assert_eq!(p.admit(t0 + Duration::from_secs(30)), Admit::Go);
    }

    #[test]
    fn retry_after_reads_seconds_and_is_capped() {
        let mut h = HeaderMap::new();
        assert_eq!(retry_after(&h), None);
        h.insert("retry-after", HeaderValue::from_static("30"));
        assert_eq!(retry_after(&h), Some(Duration::from_secs(30)));
        h.insert("retry-after", HeaderValue::from_static("999999"));
        assert_eq!(retry_after(&h), Some(Duration::from_secs(3600)));
        h.insert(
            "retry-after",
            HeaderValue::from_static("Wed, 21 Oct 2026 07:28:00 GMT"),
        );
        assert_eq!(retry_after(&h), None);
    }
}
