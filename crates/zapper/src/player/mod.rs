//! The player, as the rest of the app sees it: one [`PlayerBackend`],
//! installed by whatever hosts the picture and reachable from any thread
//! through [`backend()`], and the controller that drives it
//! ([`controller`]), whose commands are [`api`].
//!
//! mpv is the backend (`surface::mpv`, the `mpv` feature);
//! [`fake::FakeBackend`] stands in for it in tests and harnesses.
//! `captions`, `tracks` and `live_start` are the controller's pure parts,
//! [`health`] its rolling readings of the stream, [`trickplay`] and
//! [`title_thumbs`] the seek bar's previews, and [`handover`] Record on the
//! channel playing (moving the player onto the recording service).

pub mod api;
pub mod backend;
pub mod captions;
pub mod controller;
#[cfg(any(test, feature = "harness"))]
pub mod fake;
pub mod handover;
pub mod health;
pub mod live_start;
pub mod png;
pub mod title_thumbs;
pub mod tracks;
pub mod trickplay;

pub use backend::{
    backend, install, uninstall, BackendCaps, Health, PlayerBackend, Probe, Signal, Stats,
};

/// Starts the controller on `rt`: the tasks it spawns run there, and its
/// heartbeat starts, every quarter of a second on a thread of its own.
/// Idempotent.
pub fn start(rt: &tokio::runtime::Handle) {
    controller::start(rt);
}

/// How far behind the newest data "live" sits: where `seek_live` lands, the
/// furthest a forward skip goes, and where the automatic catch-up aims.
///
/// Half a second — just enough not to aim past a boundary that moves while
/// it is being read — is not enough: every jump to live stalls a few seconds
/// later. A server does not deliver evenly: it goes quiet for a second
/// or two and then sends what it owes in a burst, so half a second of video
/// ahead of the playhead runs dry in the first quiet spell and mpv stops to
/// refill. Measured with `player_check` against `fake_stream.py --gap-every
/// 4` (1.5s gaps): at 0.5s, one stall in the fifteen seconds after reaching
/// live, the buffer ahead drained to 0.0s; at this value, none.
///
/// Four seconds is a delay nobody watching live TV notices, and comfortably
/// more than the gaps a server leaves.
pub const LIVE_CUSHION: f64 = 4.0;

/// FFmpeg's HTTP options that would open a stream again by themselves.
///
/// mpv turns `reconnect` on for every HTTP stream, and FFmpeg then quietly
/// reconnects when a seekable response — a catch-up programme, say — is cut
/// off. When the provider cut it off because another device took the
/// account's connection, that reconnect ends the other device's stream in
/// turn, and the two take it from each other until somebody gives up. A
/// stream the provider ended stays ended; the interface says so and offers
/// Retry (see `ipc::ending`). The other two are off by default in FFmpeg and
/// are named so that stays true whatever a build's defaults are.
///
/// Measured against a server that cuts a seekable response off a third of
/// the way through: mpv's defaults made 80 requests before giving up; with
/// these, 3 — the open, and the two seeks any seekable file costs.
///
/// `ytdl` is switched off in `surface::mpv::Player::new` for the same reason, and it was
/// the worse of the two. mpv's built-in youtube-dl hook runs whenever a
/// stream fails to open, and where `yt-dlp` is installed it asks the server
/// for the same URL again, as a browser — so every refused open was two
/// requests, the second a fresh connection to a provider that had just said
/// no. Measured with `ending_check`, which counts them. Live TV has no use
/// for the hook in the first place.
pub const NO_RECONNECT: &str = "reconnect=0,reconnect_streamed=0,reconnect_at_eof=0";
