//! zapper — a fast Xtream live-TV client: the core every front end shares,
//! and the recording service (`zapper-server`, the `server` feature) that
//! serves the browser interface.
//!
//! Module boundaries are fixed here deliberately: `xtream` owns the server's
//! wire protocol and `m3u` the playlist/XMLTV one, `source` chooses between
//! them for the account in use, `catalog` owns persistence, `paths` says
//! where on disk each platform keeps it, `player` drives playback through
//! one backend, `surface` is that backend in mpv (the `mpv` feature),
//! `updates` owns zapper's own releases (checking, downloading,
//! installing), `events` is the bus whatever listens hears from, and `ipc`
//! is the contract the interface speaks. Nothing else may reach across those
//! lines. docs/architecture.md has the picture.

pub mod catalog;
pub mod connections;
pub mod events;
pub mod harness;
pub mod hdhomerun;
pub mod ipc;
pub mod localtime;
pub mod logfile;
pub mod m3u;
pub mod online;
pub mod paths;
pub mod player;
pub mod recording;
pub mod source;
#[cfg(feature = "mpv")]
pub mod surface;
#[cfg(test)]
pub(crate) mod test_http;
pub mod tvheadend;
pub mod updates;
pub mod xtream;
