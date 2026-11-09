//! Where the picture is shown: libmpv, the one player ([`backend`]), a
//! window for it to present into ([`host`]), and the seek bar's previews
//! decoded with the FFmpeg it loads ([`thumbframe`]).
//!
//! [`backend::Player`] is the process's [`zapper_core::player::PlayerBackend`]; the
//! controller drives it through `zapper_core::player` from any thread, and
//! nothing outside this crate names libmpv. The host is mpv in a window it
//! makes itself, as the mpv program does: what the player checks run on.

pub mod backend;
pub mod host;
pub mod thumbframe;
