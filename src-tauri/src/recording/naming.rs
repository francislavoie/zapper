//! What recordings are called on disk.
//!
//! Laid out the way Jellyfin (and Plex, and Kodi) read a TV library:
//!
//! ```text
//! <output>/<Show>/<Show> - 2026-11-03 20.00.mkv
//! <output>/<Show>/<Show> - S02E05 - 2026-11-03 20.00.mkv     (when the guide says)
//! <output>/<Show>/<Show> - 2026-11-03 20.00 - part2.mkv      (after a feed switch)
//! ```
//!
//! The date is the programme's start in this machine's time, which is what
//! a person looking for "Tuesday's news" expects, and it keeps two airings of
//! the same show apart. Parts are numbered only when there is more than one.

use crate::localtime::Civil;

/// A name that every file system the service may write to accepts.
///
/// Windows forbids `<>:"/\|?*` and control characters and drops trailing
/// dots and spaces; Linux forbids only `/` and NUL, but the folder may be a
/// Samba share. So the strictest rules apply everywhere. Never empty, never
/// `.` or `..`, and short enough to leave room for the rest of the name.
pub fn sanitize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut space = false;
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.next() {
        let c = match c {
            '<' | '>' | '"' | '|' | '?' | '*' => continue,
            // "News: Late Edition" reads best as "News - Late Edition",
            // "24:7" as "24-7".
            ':' if chars.peek().is_some_and(|n| n.is_whitespace()) => {
                space = !out.is_empty();
                '-'
            }
            ':' | '/' | '\\' => '-',
            c if c.is_control() => ' ',
            c => c,
        };
        if c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
    }
    let mut out: String = out.chars().take(120).collect();
    while out.ends_with(['.', ' ']) {
        out.pop();
    }
    if out.is_empty() || out.chars().all(|c| c == '.') {
        return "Recording".to_string();
    }
    out
}

/// What the guide says about the programme.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Programme<'a> {
    pub title: &'a str,
    pub season: Option<u32>,
    pub episode: Option<u32>,
}

/// The show's folder, inside the output folder.
pub fn folder(p: &Programme<'_>) -> String {
    sanitize(p.title)
}

/// The file name without part number or extension.
pub fn base(p: &Programme<'_>, start: Civil) -> String {
    let show = sanitize(p.title);
    let when = format!(
        "{:04}-{:02}-{:02} {:02}.{:02}",
        start.year, start.month, start.day, start.hour, start.minute
    );
    match (p.season, p.episode) {
        (Some(s), Some(e)) => format!("{show} - S{s:02}E{e:02} - {when}"),
        (None, Some(e)) => format!("{show} - E{e:02} - {when}"),
        _ => format!("{show} - {when}"),
    }
}

/// The finished file's name: `part` is 1-based, out of `parts`.
pub fn file(base: &str, part: usize, parts: usize, ext: &str) -> String {
    if parts > 1 {
        format!("{base} - part{part}.{ext}")
    } else {
        format!("{base}.{ext}")
    }
}

/// The capture file a part is written to while recording: hidden, so a
/// library scan does not pick up a file that is still growing.
pub fn capture_file(base: &str, part: usize) -> String {
    format!(".{base} - part{part}.ts")
}

/// `name`, or `name (2)`, `name (3)`... before the extension: the first that
/// `exists` says is free. A rerun of the same airing never overwrites the
/// first recording.
pub fn unique(name: &str, exists: impl Fn(&str) -> bool) -> String {
    if !exists(name) {
        return name.to_string();
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s, format!(".{e}")),
        None => (name, String::new()),
    };
    (2..)
        .map(|n| format!("{stem} ({n}){ext}"))
        .find(|candidate| !exists(candidate))
        .expect("an unbounded range finds a free name")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(h: u32, m: u32) -> Civil {
        Civil {
            year: 2026,
            month: 11,
            day: 3,
            hour: h,
            minute: m,
            second: 0,
        }
    }

    #[test]
    fn a_plain_programme_is_named_by_show_and_start() {
        let p = Programme {
            title: "The News",
            ..Default::default()
        };
        assert_eq!(folder(&p), "The News");
        assert_eq!(base(&p, at(20, 0)), "The News - 2026-11-03 20.00");
        assert_eq!(
            file(&base(&p, at(20, 0)), 1, 1, "mkv"),
            "The News - 2026-11-03 20.00.mkv"
        );
    }

    #[test]
    fn season_and_episode_come_first_when_the_guide_has_them() {
        let p = Programme {
            title: "Harbour Lights",
            season: Some(2),
            episode: Some(5),
        };
        assert_eq!(
            base(&p, at(19, 5)),
            "Harbour Lights - S02E05 - 2026-11-03 19.05"
        );
        let p = Programme {
            title: "Serial",
            season: None,
            episode: Some(12),
        };
        assert_eq!(base(&p, at(9, 30)), "Serial - E12 - 2026-11-03 09.30");
    }

    #[test]
    fn parts_are_numbered_only_when_there_are_several() {
        assert_eq!(
            file("X - 2026-11-03 20.00", 1, 2, "mkv"),
            "X - 2026-11-03 20.00 - part1.mkv"
        );
        assert_eq!(
            file("X - 2026-11-03 20.00", 2, 2, "mkv"),
            "X - 2026-11-03 20.00 - part2.mkv"
        );
        assert_eq!(capture_file("X", 3), ".X - part3.ts");
    }

    #[test]
    fn names_are_safe_on_every_file_system() {
        assert_eq!(sanitize("News: 24/7"), "News - 24-7");
        assert_eq!(sanitize("At 12:30"), "At 12-30");
        assert_eq!(sanitize("What?  <Live>  "), "What Live");
        assert_eq!(sanitize("a\tb\nc"), "a b c");
        assert_eq!(sanitize("Trailing..."), "Trailing");
        assert_eq!(sanitize(""), "Recording");
        assert_eq!(sanitize(".."), "Recording");
        assert_eq!(sanitize("???"), "Recording");
        assert_eq!(sanitize(&"x".repeat(500)).len(), 120);
    }

    #[test]
    fn a_taken_name_gets_a_number() {
        let taken = ["a.mkv", "a (2).mkv"];
        assert_eq!(unique("b.mkv", |n| taken.contains(&n)), "b.mkv");
        assert_eq!(unique("a.mkv", |n| taken.contains(&n)), "a (3).mkv");
        assert_eq!(unique("noext", |n| n == "noext"), "noext (2)");
    }
}
