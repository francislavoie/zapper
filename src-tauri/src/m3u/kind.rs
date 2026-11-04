//! Telling a playlist's on-demand entries from its channels.
//!
//! An M3U has no field for "this is a film": a provider's export lists
//! channels, movies and series episodes in one flat list, and the only
//! evidence is the address. The path says it when the provider builds it
//! (`/movie/…`, `/series/…`); otherwise a file extension does, since live
//! streams are `.ts`, `.m3u8` or nothing at all. A title is an episode only
//! when its name carries a season and episode (`S01E02`): shows are not
//! browsed in this catalogue, and an entry that merely lives under a series
//! path with no such number is better offered as a movie than hidden.

use super::Entry;

/// What an entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Channel,
    Movie,
    Episode,
}

const VIDEO_FILES: [&str; 7] = [".mp4", ".mkv", ".avi", ".m4v", ".mov", ".wmv", ".webm"];

pub fn classify(e: &Entry) -> EntryKind {
    // A query is a token, not a name: `a-film.MP4?token=x` is an `.mp4`.
    let path = e
        .url
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let on_demand = || {
        if has_episode_number(&e.name) {
            EntryKind::Episode
        } else {
            EntryKind::Movie
        }
    };
    // What is plainly live comes first: a channel can sit under a path that
    // happens to be called `movie`, or be a film-looking file on a live path.
    if path.ends_with(".ts") || path.ends_with(".m3u8") || path.contains("/live/") {
        EntryKind::Channel
    } else if path.contains("/movie/") {
        EntryKind::Movie
    } else if path.contains("/series/") || VIDEO_FILES.iter().any(|x| path.ends_with(x)) {
        on_demand()
    } else {
        EntryKind::Channel
    }
}

/// Whether `name` has an `S01E02`, in any case: digits, then an `E`, then
/// digits.
fn has_episode_number(name: &str) -> bool {
    let b = name.as_bytes();
    let digits = |from: usize| b[from..].iter().take_while(|c| c.is_ascii_digit()).count();
    (0..b.len()).any(|i| {
        if !b[i].eq_ignore_ascii_case(&b's') {
            return false;
        }
        let e = i + 1 + digits(i + 1);
        e > i + 1 && b.get(e).is_some_and(|c| c.eq_ignore_ascii_case(&b'e')) && digits(e + 1) > 0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_demand_entries_are_told_from_channels_by_their_address() {
        let k = |url: &str, name: &str| {
            classify(&Entry {
                url: url.into(),
                name: name.into(),
                ..Default::default()
            })
        };
        assert_eq!(k("http://h/movie/u/p/12.mkv", "A Film"), EntryKind::Movie);
        assert_eq!(
            k("http://h/vod/a-film.MP4?token=x", "A Film"),
            EntryKind::Movie
        );
        assert_eq!(
            k("http://h/series/u/p/77.mp4", "A Show S01E02"),
            EntryKind::Episode
        );
        assert_eq!(
            k("http://h/series/u/p/78.mp4", "A Special"),
            EntryKind::Movie
        );
        for url in [
            "http://h/live/u/p/12.ts",
            "http://h/u/p/12",
            "http://h/news.m3u8",
            "http://cdn/live/movie/index.m3u8",
            "http://h/live/u/p/1.mp4",
            "http://h/movie/u/p/1.m3u8",
        ] {
            assert_eq!(k(url, "News"), EntryKind::Channel);
        }
    }

    #[test]
    fn an_episode_number_needs_a_season_and_an_episode() {
        assert!(has_episode_number("Show s1e10"));
        assert!(has_episode_number("Show S01E02 - Title"));
        assert!(!has_episode_number("Season 2"));
        assert!(!has_episode_number("Sunset E5"));
        assert!(!has_episode_number("S01"));
    }
}
