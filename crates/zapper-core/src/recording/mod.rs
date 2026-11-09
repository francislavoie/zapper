//! Recording: what the service (`zapper-server`, its own crate) and a front
//! end share, and what a front end needs to talk to it. See
//! docs/recording.md.
//!
//! The pure parts, unit-tested on their own:
//! [`naming`] (file names from a pattern), [`backup`] (which channel a
//! recording falls back to), [`budget`] (whether the
//! account has a connection to spare), [`sharing`] (how recordings share
//! the account's connections, and the conflicts that follow), [`fallback`] (when to switch
//! between a channel's main and backup feeds), [`schedule`] (what to start,
//! stop or give up on at a given moment), [`clock`] (the wall-clock subtitle
//! track), [`lifecycle`] (when the service stops or restarts itself).
//! [`api`] is the HTTP contract shared by the service and the app's
//! [`client`]; [`config`] is where both find the port and the token;
//! [`service`] installs the service as a systemd user unit; [`store`] is its
//! store of recordings; [`local`] is the service on this computer as a front
//! end sees it, and [`handover`] decides Record on the channel playing.
//!
//! The service itself is `zapper_server::recording`.

pub mod api;
pub mod backup;
pub mod budget;
pub mod client;
pub mod clock;
pub mod config;
pub mod fallback;
pub mod handover;
pub mod lifecycle;
pub mod local;
pub mod naming;
pub mod schedule;
pub mod sharing;
pub mod service;
pub mod store;

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
