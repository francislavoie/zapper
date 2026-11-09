//! The recording service itself. What it shares with a front end (the API,
//! the pure decisions, the store of recordings, the config) is
//! `zapper_core::recording`; see docs/recording.md.
//!
//! The capture loop ([`capture`]), the connections it shares between
//! recordings and browsers watching ([`hub`], [`watch`]), the browsers
//! watching movies on connections of their own ([`vodwatch`], reading the
//! file through [`upstream`]), the remuxers (a capture into Matroska,
//! [`remux`]; a live feed for a browser, [`livemux`], as [`tsprobe`]
//! decides; a movie file from any point in it, [`vodmux`]), the HTTP server
//! ([`server`], [`http`]) with its listeners ([`listen`]), who may use it
//! ([`access`]), the provider account a signed-in browser may change
//! ([`account`]), its HTTPS certificate ([`tls`]), the subtitles it finds
//! online for a browser ([`online_http`]), and the web interface it serves
//! ([`web`]) with the catalogue API that interface reads ([`browse`], for
//! movies [`vod_browse`], for shows [`series_browse`] and for Continue
//! watching [`continue_browse`]), and the live stream of a recording a
//! `.strm` file points Jellyfin at ([`strm`]).

pub mod access;
pub mod account;
pub mod browse;
pub mod capture;
pub mod continue_browse;
pub mod http;
pub mod hub;
pub mod listen;
pub mod livemux;
pub mod online_http;
pub mod remux;
pub mod series_browse;
pub mod server;
pub mod strm;
#[cfg(test)]
pub(crate) mod test_fixtures;
pub mod tls;
pub mod tsprobe;
pub mod upstream;
pub mod vod_browse;
pub mod vodmux;
pub mod vodwatch;
pub mod watch;
pub mod web;
