//! zapper — a fast Xtream live-TV client: the core every front end shares,
//! and the recording service (`zapper-server`, its own crate) builds on.
//! No native dependency: libmpv is zapper-mpv's, FFmpeg's remuxers
//! zapper-server's.
//!
//! Module boundaries are fixed here deliberately: `xtream` owns the server's
//! wire protocol and `m3u` the playlist/XMLTV one, `source` chooses between
//! them for the account in use, `catalog` owns persistence, `paths` says
//! where on disk each platform keeps it, `player` drives playback through
//! one backend (zapper-mpv's, or a stand-in), `updates` owns zapper's own
//! releases (checking, downloading, installing), `events` is the bus
//! whatever listens hears from, and `api` is the contract the interface
//! speaks. Nothing else may reach across those lines. docs/architecture.md
//! has the picture.

pub mod api;
pub mod catalog;
pub mod connections;
pub mod events;
#[cfg(any(test, feature = "harness"))]
pub mod harness;
pub mod hdhomerun;
pub mod localtime;
pub mod logfile;
pub mod m3u;
pub mod online;
pub mod paths;
pub mod player;
pub mod recording;
pub mod source;
#[cfg(any(test, feature = "test-support"))]
pub mod test_http;
pub mod tvheadend;
pub mod updates;
pub mod xtream;
