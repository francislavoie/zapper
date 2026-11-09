//! Where the picture is shown: libmpv, the one player ([`mpv`]), and a
//! window for it to present into ([`host`]). The `mpv` feature.
//!
//! [`mpv::Player`] is the process's [`crate::player::PlayerBackend`]; the
//! controller drives it through `crate::player` from any thread, and
//! nothing outside this module names libmpv. The host is mpv in a window it
//! makes itself, as the mpv program does: what the player checks run on.

pub mod host;
pub mod mpv;

// Where the seek bar's previews and their encoder lived before they moved
// next to the controller; the harnesses still reach them here.
pub use crate::player::{png, title_thumbs, trickplay};
