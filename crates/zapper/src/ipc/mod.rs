//! The Tauri command boundary.
//!
//! Split by owner so parallel work never edits one file: `catalog_cmds` reads
//! and syncs the local mirror, `player_cmds` hands the player's commands to
//! `crate::player`, `update_cmds` fronts the updater, `handover` records what
//! the player shows (moving it onto the recording service), and
//! `backup_cmds` exports and imports Settings › Data's backups. `types` is
//! the shared contract and is not edited by any of them.

//!
//! The recording service's build has `types` and the pure `ending` and
//! `health` it refers to: plain data the catalogue speaks too. The commands
//! are the desktop app's.

#[cfg(feature = "gui")]
pub mod backup_cmds;
#[cfg(feature = "gui")]
pub mod catalog_cmds;
#[cfg(feature = "gui")]
pub mod continue_cmds;
pub mod ending;
pub mod health;
#[cfg(feature = "gui")]
pub mod handover;
#[cfg(feature = "gui")]
pub mod online_cmds;
#[cfg(feature = "gui")]
pub mod player_cmds;
#[cfg(feature = "gui")]
pub mod recording_cmds;
#[cfg(feature = "gui")]
pub mod series_cmds;
pub mod types;
#[cfg(feature = "gui")]
pub mod update_cmds;
#[cfg(feature = "gui")]
pub mod vod_cmds;

pub use types::*;
