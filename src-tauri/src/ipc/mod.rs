//! The Tauri command boundary.
//!
//! Split by owner so parallel work never edits one file: `catalog_cmds` reads
//! and syncs the local mirror, `player_cmds` drives the surface,
//! `update_cmds` fronts the updater, `backup_cmds` exports and imports
//! Settings › Data's backups, and `tracks` is the pure half of the picture
//! menu's commands. `types` is the shared contract and is not edited by any
//! of them.

//!
//! The recording service's build has `types` and the pure `ending` and
//! `health` it refers to: plain data the catalogue speaks too. The commands
//! are the desktop app's.

#[cfg(feature = "gui")]
pub mod backup_cmds;
#[cfg(feature = "gui")]
pub mod catalog_cmds;
pub mod ending;
pub mod health;
#[cfg(feature = "gui")]
pub mod player_cmds;
#[cfg(feature = "gui")]
pub mod recording_cmds;
#[cfg(feature = "gui")]
pub mod tracks;
pub mod types;
#[cfg(feature = "gui")]
pub mod update_cmds;

pub use types::*;
