//! Tvheadend servers: the fourth way to describe a catalogue.
//!
//! Tvheadend is a TV server — tuners on one machine, clients anywhere — and
//! it already speaks the two formats zapper reads for playlists: an M3U of
//! its channels and an XMLTV guide of their programmes. So its channels go
//! through the M3U parser and the playlist writer, and its guide through the
//! XMLTV parser, exactly as a playlist account's do. Two things are its own:
//! logging in, which is HTTP Digest ([`digest`]) and decides which playlist
//! to ask for ([`client`]); and categories, which come from its channel tags
//! rather than from the playlist.
//!
//! Guide channels need no matching: the playlist's `tvg-id` is the channel's
//! UUID, and so is the guide's `<channel id>` (in Tvheadend's default XMLTV
//! format; the "basic, no hash" format uses the channel name, which the
//! name matcher then finds).
//!
//! Channel ids come from that UUID too, so renaming or retagging a channel
//! on the server keeps its history here.
//!
//! Not supported: recordings and timeshift (Tvheadend's DVR is its own
//! subsystem, and out of scope), and HTSP, its native protocol — mpv speaks
//! HTTP, and the HTTP streams are the same streams.

pub mod account;
pub mod client;
pub mod digest;
pub mod source;

pub use account::TvhAccount;
pub use source::TvhSource;

/// The identity a Tvheadend channel's id is derived from: its UUID.
pub fn identity(uuid: &str) -> String {
    format!("tvh\u{0}{}", uuid.trim().to_ascii_lowercase())
}
