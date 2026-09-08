//! Local mirror of the provider catalogue.
//!
//! Startup reads only from here, never the network; sync refreshes in the
//! background and the UI updates as each task lands.

pub mod credentials;
pub mod db;
pub mod epg;
pub mod index;
pub mod schema;
pub mod search;
pub mod settings;
pub mod status;
pub mod store;
pub mod sync;
pub mod sync_engine;

pub use schema::{migrate, open_at, open_in_memory};
pub use search::SearchIndex;
pub use sync::{clean_name, sync_categories, sync_channels, SyncStats};
