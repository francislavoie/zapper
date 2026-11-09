//! The player, as the rest of the app sees it: one [`PlayerBackend`],
//! installed by the platform's surface and reachable from any thread
//! through [`backend()`], and the controller that drives it
//! ([`controller`]), whose commands are [`api`].
//!
//! mpv is the backend in the app (`surface::mpv`); [`fake::FakeBackend`]
//! stands in for it in tests and harnesses. `captions`, `tracks` and
//! `live_start` are the controller's pure parts.

#[cfg(feature = "gui")]
pub mod api;
pub mod backend;
pub mod captions;
#[cfg(feature = "gui")]
pub mod controller;
#[cfg(any(test, feature = "harness"))]
pub mod fake;
pub mod live_start;
pub mod tracks;

pub use backend::{
    backend, install, uninstall, BackendCaps, Health, PlayerBackend, Probe, Signal, Stats,
};

/// Starts the controller on `rt`: the tasks it spawns run there, and its
/// heartbeat starts, every quarter of a second on a thread of its own.
/// Idempotent.
#[cfg(feature = "gui")]
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
