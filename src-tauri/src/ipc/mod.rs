//! The Tauri command boundary.
//!
//! Split by owner so parallel work never edits one file: `catalog_cmds` reads
//! and syncs the local mirror, `player_cmds` drives the surface,
//! `update_cmds` fronts the updater. `types` is the shared contract and is
//! not edited by any of them.

pub mod catalog_cmds;
pub mod health;
pub mod player_cmds;
pub mod types;
pub mod update_cmds;

pub use types::*;
