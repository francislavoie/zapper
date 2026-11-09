//! Xtream Codes protocol client.

pub mod client;
pub mod models;

pub use client::{Account, Credentials, XtreamClient};
pub use models::{
    Category, Channel, EpisodeInfo, EpisodeInfoBody, SeasonInfo, SeriesInfo, SeriesInfoBody,
    SeriesStream, ServerInfo, StreamTags, UserInfo, VodInfo, VodInfoBody, VodMovieData, VodStream,
};
