//! Telling a playlist's on-demand entries from its channels, and its
//! episodes from its films.
//!
//! An M3U has no field for "this is a film": a provider's export lists
//! channels, movies and series episodes in one flat list, and the only
//! evidence is the address. The path says it when the provider builds it
//! (`/movie/…`, `/series/…`); otherwise a file extension does, since live
//! streams are `.ts`, `.m3u8` or nothing at all. An on-demand entry is an
//! episode when its name carries a season and episode number
//! ([`episode_name`]), and episodes are browsed as shows, gathered by group
//! and by the name before the number. One without a number stays a movie,
//! even under a series path: better offered as a film than hidden. A channel
//! is never an episode, whatever it is called.

use super::Entry;
use crate::catalog::episode::find_in_name;

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
        if episode_name(&e.name).is_some() {
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

/// The most episodes past its first that one entry is believed to hold. A
/// longer "range" is more likely a number that means something else, and
/// the entry is then the single episode at its first number.
pub const MAX_EPISODE_SPAN: i32 = 10;

/// An episode entry's name, taken apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpisodeName {
    /// The name before the number. Empty when the name starts with it; the
    /// writer then names the show after the entry's group.
    pub show: String,
    pub season: i32,
    pub episode: i32,
    /// A multi-episode entry's last number (`S01E01-E02`).
    pub episode_end: Option<i32>,
    /// What follows the number, if anything.
    pub title: Option<String>,
}

/// What is trimmed off both ends of a show's name and an episode's title.
fn is_separator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '-' | '–' | '—' | ':' | '|' | '.' | '_' | ',')
}

/// `name` split around its episode number: the show before it, the title
/// after it. `None` when it has none ([`find_in_name`] says what counts).
///
/// A name without spaces is read with its dots as spaces, as a file's name
/// would be: `A.Show.Name.S01E02.720p` is `A Show Name`'s, titled `720p`.
pub fn episode_name(name: &str) -> Option<EpisodeName> {
    let m = find_in_name(name)?;
    let dotted = !name.contains(char::is_whitespace);
    let tidy = |part: &str| -> String {
        let part = if dotted {
            part.replace('.', " ")
        } else {
            part.to_owned()
        };
        part.trim_matches(is_separator).to_owned()
    };
    let season = i32::try_from(m.season).ok()?;
    let episode = i32::try_from(m.first).ok()?;
    let title = tidy(&name[m.end..]);
    Some(EpisodeName {
        show: tidy(&name[..m.start]),
        season,
        episode,
        episode_end: m
            .last
            .and_then(|l| i32::try_from(l).ok())
            .filter(|&l| l > episode && l - episode <= MAX_EPISODE_SPAN),
        title: (!title.is_empty()).then_some(title),
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
            k("http://h/series/u/p/77.mp4", "A Show 1x02"),
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
            assert_eq!(k(url, "News S01E02"), EntryKind::Channel);
        }
    }

    #[test]
    fn an_episode_number_needs_a_season_and_an_episode() {
        assert!(episode_name("Show s1e10").is_some());
        assert!(episode_name("Show S01E02 - Title").is_some());
        assert!(episode_name("Show Season 2 Episode 3").is_some());
        assert!(episode_name("Season 2").is_none());
        assert!(episode_name("Sunset E5").is_none());
        assert!(episode_name("S01").is_none());
        assert!(episode_name("Show S01E0").is_none());
    }

    type Parts = (&'static str, i32, i32, Option<i32>, Option<&'static str>);

    /// Names with an episode number, and what they split into.
    const EPISODES: &[(&str, Parts)] = &[
        ("A Show S01E02", ("A Show", 1, 2, None, None)),
        ("A Show s01.e02 Title", ("A Show", 1, 2, None, Some("Title"))),
        ("A Show S01_E02", ("A Show", 1, 2, None, None)),
        ("A Show S01 E02", ("A Show", 1, 2, None, None)),
        (
            "A Show S01 - E02 - The Title",
            ("A Show", 1, 2, None, Some("The Title")),
        ),
        ("A Show S01xE02", ("A Show", 1, 2, None, None)),
        ("A Show S1 E2", ("A Show", 1, 2, None, None)),
        ("A Show 1x02 Title", ("A Show", 1, 2, None, Some("Title"))),
        ("A Show Season 1 Episode 2", ("A Show", 1, 2, None, None)),
        ("A Show S01E01-E02", ("A Show", 1, 1, Some(2), None)),
        ("A Show S01E01-02", ("A Show", 1, 1, Some(2), None)),
        ("A Show S01E01E02", ("A Show", 1, 1, Some(2), None)),
        ("A Show S01E01-02-03", ("A Show", 1, 1, Some(3), None)),
        ("A Show S01E01-S01E02", ("A Show", 1, 1, Some(2), None)),
        ("A Show 1x01x02", ("A Show", 1, 1, Some(2), None)),
        ("A Show 1x01-02", ("A Show", 1, 1, Some(2), None)),
        ("A Show 1x01 - 1x02", ("A Show", 1, 1, Some(2), None)),
        // A span too long, a descending one, one across two seasons: the
        // first episode alone, and the range text never becomes the title.
        ("A Show S01E01-E15", ("A Show", 1, 1, None, None)),
        ("A Show S01E03-E02", ("A Show", 1, 3, None, None)),
        ("A Show S01E01-S02E01", ("A Show", 1, 1, None, None)),
        ("A Show S01E02 - Title 3", ("A Show", 1, 2, None, Some("Title 3"))),
        ("A Show S01E02-720p", ("A Show", 1, 2, None, Some("720p"))),
        (
            "A.Show.Name.S01E02.720p",
            ("A Show Name", 1, 2, None, Some("720p")),
        ),
        ("S01E01 Pilot", ("", 1, 1, None, Some("Pilot"))),
    ];

    /// Names that only look like one: a year for a season, channel names,
    /// dates, parts, absolute numbers, a resolution, a film.
    const NOT_EPISODES: &[&str] = &[
        "A Show S2024E01",
        "SPORT 1",
        "Film 2019",
        "BBC 1x",
        "S1",
        "News 2024.11.07",
        "07.11.2024",
        "A Show Part 2",
        "A Show - 123",
        "103",
        "Film 1920x1080",
        "4x4",
        "4x4 Adventures",
        "A Film (2021)",
    ];

    #[test]
    fn episode_names_split_into_show_season_episode_and_title() {
        for &(name, (show, season, episode, episode_end, title)) in EPISODES {
            assert_eq!(
                episode_name(name),
                Some(EpisodeName {
                    show: show.into(),
                    season,
                    episode,
                    episode_end,
                    title: title.map(Into::into),
                }),
                "{name:?}"
            );
        }
        for &name in NOT_EPISODES {
            assert_eq!(episode_name(name), None, "{name:?}");
        }
    }

    #[test]
    fn every_name_classified_as_an_episode_parses() {
        let k = |url: &str, name: &str| {
            classify(&Entry {
                url: url.into(),
                name: name.into(),
                ..Default::default()
            })
        };
        for url in ["http://h/series/u/p/7.mp4", "http://h/vod/a-show.mkv"] {
            for &(name, _) in EPISODES {
                assert_eq!(k(url, name), EntryKind::Episode, "{name:?} at {url}");
            }
            for &name in NOT_EPISODES {
                assert_eq!(k(url, name), EntryKind::Movie, "{name:?} at {url}");
            }
        }
        // A `/movie/` address is always a film.
        assert_eq!(
            k("http://h/movie/u/p/7.mkv", "A Show S01E02"),
            EntryKind::Movie
        );
        for url in [
            "http://h/a-show.ts",
            "http://h/a-show/index.m3u8",
            "http://h/live/u/p/7.mkv",
        ] {
            assert_eq!(k(url, "A Show S01E02"), EntryKind::Channel, "{url}");
        }
    }
}
