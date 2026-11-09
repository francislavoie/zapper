//! The outside services zapper asks about on-demand titles.
//!
//! Each is on unless turned off in Settings › Online services
//! ([`config`], `online-services.json`); off, nothing at all is sent to it.
//! What each is sent is the title's ids, name, year, season and episode,
//! and languages: never a stream URL, the provider's name or address, or
//! the provider account.
//!
//! - **OpenSubtitles** ([`opensubtitles`]): subtitle search by TMDB id (or
//!   name and year), season, episode and languages, with zapper's app key
//!   and no sign-in; downloads with the user's own OpenSubtitles sign-in,
//!   which is kept in the settings file (readable by this user alone) and
//!   sent to OpenSubtitles alone. Fetched files are SubRip ([`srt`]),
//!   turned into WebVTT. [`subtitles`] finds a title's, downloads them and
//!   keeps them with it.
//!
//! Every service is asked through [`http`]: one client shape (zapper's
//! `User-Agent`, short timeouts, redirects only where they are expected),
//! and one [`http::Pacer`] per service, a `static` beside its client, which
//! keeps to the service's rate limit and honours its 429s. Errors are
//! [`http::OnlineError`]s, whose texts carry no URL and pass
//! [`crate::catalog::sync_engine::scrub`].
//!
//! TMDB (posters, plots, cast) and TheIntroDB (intro and credits times) slot
//! in the same way: a module each with its client and pacer, a section each
//! in [`config::OnlineConfig`], and a line each in the status the settings
//! show.

pub mod config;
pub mod http;
pub mod opensubtitles;
pub mod srt;
pub mod subtitles;
