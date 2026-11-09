//! The interface's contract: `types`, the data every front end and the
//! recording service's web API speak, and the pure `ending`, which says why
//! a stream ended in the interface's words.

pub mod ending;
pub mod types;

pub use types::*;
