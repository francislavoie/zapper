//! The Tauri command boundary.
//!
//! Split by owner so parallel work never edits one file: `catalog_cmds` reads
//! and syncs the local mirror, `player_cmds` drives the surface. `types` is
//! the shared contract and is not edited by either.

pub mod catalog_cmds;
pub mod player_cmds;
pub mod types;

pub use types::*;
