//! The player, as the rest of the app sees it: one [`PlayerBackend`],
//! installed by the platform's surface and reachable from any thread
//! through [`backend()`].
//!
//! mpv is the backend in the app (`surface::mpv`); [`fake::FakeBackend`]
//! stands in for it in tests and harnesses.

pub mod backend;
#[cfg(any(test, feature = "harness"))]
pub mod fake;

pub use backend::{
    backend, install, uninstall, BackendCaps, Health, PlayerBackend, Probe, Signal, Stats,
};
