//! Recording: the headless service (`zapper-server`) and what the app needs
//! to talk to it. See docs/recording.md.
//!
//! The pure parts are in every build and unit-tested on their own:
//! [`naming`] (file names from a pattern), [`backup`] (which channel a
//! recording falls back to), [`budget`] (whether the
//! account has a connection to spare), [`sharing`] (how recordings share
//! the account's connections, and the conflicts that follow), [`fallback`] (when to switch
//! between a channel's main and backup feeds), [`schedule`] (what to start,
//! stop or give up on at a given moment), [`clock`] (the wall-clock subtitle
//! track), [`lifecycle`] (when the service stops or restarts itself).
//! [`api`] is the HTTP contract shared by the service and the app's
//! [`client`]; [`config`] is where both find the port and the token;
//! [`service`] installs the service as a systemd user unit.
//!
//! The service itself — the store of recordings, the capture loop, the
//! connections it shares between recordings and browsers watching ([`hub`],
//! [`watch`]), the browsers watching movies on connections of their own
//! ([`vodwatch`], reading the file through [`upstream`]), the remuxers (a
//! capture into Matroska; a live feed for a browser, [`livemux`], as
//! [`tsprobe`] decides; a movie file from any point in it, [`vodmux`]), the
//! HTTP server with its listeners ([`listen`]), who may use it
//! ([`access`]), the provider account a signed-in browser may change
//! ([`account`]), its HTTPS certificate ([`tls`]), and the web interface it
//! serves ([`web`]) with the catalogue API that interface reads ([`browse`],
//! for movies [`vod_browse`], for shows [`series_browse`] and for Continue
//! watching [`continue_browse`]),
//! and the live stream of a recording a `.strm` file points Jellyfin at
//! ([`strm`]) — is in the `server` feature, so the app links none of it.

pub mod api;
pub mod backup;
pub mod budget;
pub mod client;
pub mod clock;
pub mod config;
pub mod fallback;
pub mod handover;
pub mod lifecycle;
pub mod naming;
pub mod schedule;
pub mod sharing;
pub mod service;
pub mod store;

#[cfg(feature = "server")]
pub mod access;
#[cfg(feature = "server")]
pub mod account;
#[cfg(feature = "server")]
pub mod browse;
#[cfg(feature = "server")]
pub mod capture;
#[cfg(feature = "server")]
pub mod http;
#[cfg(feature = "server")]
pub mod hub;
#[cfg(feature = "server")]
pub mod listen;
#[cfg(feature = "server")]
pub mod livemux;
#[cfg(all(test, feature = "server"))]
pub(crate) mod test_fixtures;
#[cfg(feature = "server")]
pub mod remux;
#[cfg(feature = "server")]
pub mod server;
#[cfg(feature = "server")]
pub mod strm;
#[cfg(feature = "server")]
pub mod tls;
#[cfg(feature = "server")]
pub mod tsprobe;
#[cfg(feature = "server")]
pub mod upstream;
#[cfg(feature = "server")]
pub mod vodmux;
#[cfg(feature = "server")]
pub mod vodwatch;
#[cfg(feature = "server")]
pub mod watch;
#[cfg(feature = "server")]
pub mod vod_browse;
#[cfg(feature = "server")]
pub mod series_browse;
#[cfg(feature = "server")]
pub mod continue_browse;
#[cfg(feature = "server")]
pub mod web;

/// Unix seconds now.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Unix milliseconds now.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
