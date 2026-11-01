//! The Tauri command boundary.
//!
//! Split by owner so parallel work never edits one file: `catalog_cmds` reads
//! and syncs the local mirror, `player_cmds` drives the surface,
//! `update_cmds` fronts the updater, `backup_cmds` exports and imports
//! Settings › Data's backups, and `tracks` is the pure half of the picture
//! menu's commands. `types` is the shared contract and is not edited by any
//! of them.

pub mod backup_cmds;
pub mod catalog_cmds;
pub mod health;
pub mod player_cmds;
pub mod tracks;
pub mod types;
pub mod update_cmds;

pub use types::*;
