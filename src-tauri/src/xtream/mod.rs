//! Xtream Codes protocol client.

pub mod client;
pub mod models;

pub use client::{Credentials, XtreamClient};
pub use models::{Category, Channel, UserInfo};
