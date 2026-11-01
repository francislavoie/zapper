//! Recording: the headless service (`zapper-server`) and what the app needs
//! to talk to it. See docs/recording.md.
//!
//! The pure parts are in every build and unit-tested on their own:
//! [`naming`] (file names from a pattern), [`budget`] (whether the
//! account has a connection to spare), [`fallback`] (when to switch
//! between a channel's main and backup feeds), [`schedule`] (what to start,
//! stop or give up on at a given moment), [`clock`] (the wall-clock subtitle
//! track). [`api`] is the HTTP contract shared by the service and the app's
//! [`client`]; [`config`] is where both find the port and the token;
//! [`service`] installs the service as a systemd user unit.
//!
//! The service itself — the store of recordings, the capture loop, the
//! remuxer and the HTTP server — is in the `server` feature, so the app links
//! none of it.

pub mod api;
pub mod budget;
pub mod client;
pub mod clock;
pub mod config;
pub mod fallback;
pub mod naming;
pub mod schedule;
pub mod service;
pub mod store;

#[cfg(feature = "server")]
pub mod capture;
#[cfg(feature = "server")]
pub mod http;
#[cfg(feature = "server")]
pub mod remux;
#[cfg(feature = "server")]
pub mod server;

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
