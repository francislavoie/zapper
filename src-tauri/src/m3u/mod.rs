//! M3U playlists and XMLTV guides: the second way to describe a catalogue.
//!
//! An Xtream server answers questions; a playlist is a file. Everything the
//! server would say over several endpoints — categories, channels, stream
//! URLs, which channels keep an archive — is folded into one text document of
//! `#EXTINF` lines, and the guide is a separate XMLTV document somewhere else
//! (or nowhere). This module owns that wire format the way `xtream` owns the
//! server's: parsing, the ids a playlist does not carry, catch-up URL
//! templates, and opening a location that may be a URL or a local file.
//!
//! Nothing here writes to the database. `catalog::playlist` does that, and
//! `catalog::sync_engine` decides when.

pub mod account;
pub mod catchup;
pub mod fetch;
pub mod ids;
pub mod parse;
pub mod redact;

pub use account::M3uAccount;
pub use parse::{Entry, Header, Parser};
