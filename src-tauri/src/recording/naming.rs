//! What recordings are called on disk.
//!
//! A recording's place inside the output folder comes from a pattern: text
//! with tokens such as `{show}` and `{date}`, where `/` makes a folder. The
//! default lays files out the way Jellyfin (and Plex, and Kodi) read a TV
//! library:
//!
//! ```text
//! <output>/<Show>/<Show> - 2026-11-03 20.00.mkv
//! <output>/<Show>/<Show> - S02E05 - 2026-11-03 20.00.mkv     (when the guide says)
//! <output>/<Show>/<Show> - 2026-11-03 20.00 - part2.mkv      (after a feed switch)
//! ```
//!
//! The date is the programme's start in this machine's time, which is what
//! a person looking for "Tuesday's news" expects, and it keeps two airings of
//! the same show apart. The extension, the part number (only when there is
//! more than one) and a ` (2)` for a name already taken are always the
//! service's to add, never the pattern's.
//!
//! A token with nothing to say (no season, no episode title) takes the
//! separator before it along: `{show} - {se} - {date}` is `News - 2026-11-03`
//! for a programme without an episode number, never `News -  - 2026-11-03`.
//! The precise rule is on [`collapse`].
//!
//! Whatever the pattern and whatever the guide's titles, the result stays
//! inside the output folder and is a name every file system the service may
//! write to accepts. `src/logic/naming.ts` is the same logic for the app's
//! preview; `naming_cases.json` holds cases both are checked against.

use serde::{Deserialize, Serialize};

use crate::localtime::Civil;

/// The choice in Settings › Recording › File names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Naming {
    /// A folder per show, as media servers want it.
    #[default]
    MediaServer,
    ByChannel,
    ByDate,
    /// Everything straight in the output folder.
    Flat,
    /// The person's own pattern.
    Custom,
}

pub const MEDIA_SERVER: &str = "{show}/{show} - {se} - {date} {time}";

impl Naming {
    /// The preset's pattern; `None` for a custom one.
    pub fn preset(self) -> Option<&'static str> {
        Some(match self {
            Self::MediaServer => MEDIA_SERVER,
            Self::ByChannel => "{channel}/{show} - {date} {time}",
            Self::ByDate => "{date}/{time} {channel} - {show}",
            Self::Flat => "{show} - {channel} - {date} {time}",
            Self::Custom => return None,
        })
    }
}

/// The pattern recordings are named by: the preset's, or the custom one.
/// A custom pattern that is not valid (only possible by editing
/// `server.json` by hand, since the API refuses one) falls back to the
/// default rather than failing a recording.
pub fn pattern_for(naming: Naming, custom: &str) -> &str {
    match naming.preset() {
        Some(p) => p,
        None if validate(custom).is_ok() => custom,
        None => MEDIA_SERVER,
    }
}

pub const TOKENS: [&str; 9] = [
    "show", "title", "channel", "date", "time", "year", "season", "episode", "se",
];

/// At most this many characters in a pattern.
pub const PATTERN_MAX: usize = 250;
/// A token's value, in bytes: a title this long is still recognisable, and
/// the date after it survives.
const VALUE_MAX: usize = 100;
/// A folder's or file's name, in bytes. File systems allow 255; the service
/// adds up to ` - part12 (99).mkv`, and a leading `.` with `.partial` to the
/// file it writes first.
const SEGMENT_MAX: usize = 180;
/// Where a token with no value was, while separators are collapsed. Never in
/// a name: control characters are taken out of everything first.
const GAP: char = '\0';
/// Used for the file name when the pattern's comes out empty (`{title}` for
/// a programme without an episode title).
const FALLBACK_FILE: &str = "{show} - {date} {time}";

/// What the guide says about the programme.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Programme<'a> {
    pub title: &'a str,
    pub episode_title: Option<&'a str>,
    pub channel: &'a str,
    pub season: Option<u32>,
    pub episode: Option<u32>,
}

/// Where a recording goes, relative to the output folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    /// Folders inside the output folder, outermost first; none for a flat
    /// layout.
    pub folders: Vec<String>,
    /// The file name without part number or extension.
    pub base: String,
}

impl Place {
    /// `folder/folder/base`, for showing and testing.
    pub fn path(&self) -> String {
        let mut p = self.folders.join("/");
        if !p.is_empty() {
            p.push('/');
        }
        p.push_str(&self.base);
        p
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Piece<'a> {
    Text(&'a str),
    Token(&'a str),
}

/// The pattern as folder and file segments, each a run of text and tokens.
/// `/` and `\` both make a folder, so a pattern typed on Windows reads the
/// same.
fn segments(pattern: &str) -> Result<Vec<Vec<Piece<'_>>>, String> {
    fn text<'a>(segs: &mut Vec<Vec<Piece<'a>>>, t: &'a str) {
        for (i, part) in t.split(['/', '\\']).enumerate() {
            if i > 0 {
                segs.push(Vec::new());
            }
            if !part.is_empty() {
                segs.last_mut()
                    .expect("never empty")
                    .push(Piece::Text(part));
            }
        }
    }
    let mut segs = vec![Vec::new()];
    let mut rest = pattern;
    while let Some(i) = rest.find(['{', '}']) {
        if rest[i..].starts_with('}') {
            return Err("a } has no { before it".into());
        }
        text(&mut segs, &rest[..i]);
        let after = &rest[i + 1..];
        let end = after
            .find(['{', '}'])
            .filter(|&j| after[j..].starts_with('}'))
            .ok_or("a { is not closed with }")?;
        let name = &after[..end];
        if !TOKENS.contains(&name) {
            return Err(format!(
                "{{{name}}} is not a token; the tokens are {}",
                TOKENS.map(|t| format!("{{{t}}}")).join(" ")
            ));
        }
        segs.last_mut()
            .expect("never empty")
            .push(Piece::Token(name));
        rest = &after[end + 1..];
    }
    text(&mut segs, rest);
    Ok(segs)
}

/// Why a pattern cannot be used, if it cannot. The service refuses such a
/// pattern; the app says the same before sending it.
pub fn validate(pattern: &str) -> Result<(), String> {
    let p = pattern.trim();
    if p.is_empty() {
        return Err("the pattern is empty".into());
    }
    if p.chars().count() > PATTERN_MAX {
        return Err(format!(
            "the pattern is longer than {PATTERN_MAX} characters"
        ));
    }
    if p.starts_with(['/', '\\']) {
        return Err(
            "the pattern cannot start with /: it names a place inside the output folder".into(),
        );
    }
    let b = p.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return Err(
            "the pattern cannot start with a drive: it names a place inside the output folder"
                .into(),
        );
    }
    if p.chars().any(char::is_control) {
        return Err("the pattern cannot contain control characters".into());
    }
    if let Some(c) = p.chars().find(|c| "<>:\"|?*".contains(*c)) {
        return Err(format!(
            "names cannot contain {c} (Windows does not allow it)"
        ));
    }
    let segs = segments(p)?;
    for seg in &segs {
        let fixed: String = seg
            .iter()
            .map(|piece| match piece {
                Piece::Text(t) => *t,
                Piece::Token(_) => "",
            })
            .collect();
        let has_token = seg.iter().any(|piece| matches!(piece, Piece::Token(_)));
        if !has_token {
            let fixed = fixed.trim();
            if fixed.is_empty() {
                return Err("a folder name is empty (two / in a row, or one at the end)".into());
            }
            if fixed.chars().all(|c| c == '.') {
                return Err(format!(
                    "“{fixed}” cannot be a folder: recordings stay inside the output folder"
                ));
            }
            if reserved(stem(fixed)) {
                return Err(format!("“{fixed}” is a reserved name on Windows"));
            }
        }
    }
    let file = segs.last().expect("never empty");
    if !file.iter().any(|piece| matches!(piece, Piece::Token(_))) {
        return Err(
            "the file name needs a token, such as {date} {time}, or every recording gets the same name"
                .into(),
        );
    }
    Ok(())
}

/// Where a programme starting at `start` goes under `pattern`. A pattern
/// that is not valid is read as the default one.
pub fn place(pattern: &str, p: &Programme<'_>, start: Civil) -> Place {
    let segs = match validate(pattern) {
        Ok(()) => segments(pattern.trim()),
        Err(_) => segments(MEDIA_SERVER),
    }
    .expect("a valid pattern parses");
    let value = |token: &str| -> String {
        match token {
            "show" => match clean(p.title, VALUE_MAX) {
                s if s.is_empty() => "Recording".into(),
                s => s,
            },
            "title" => clean(p.episode_title.unwrap_or(""), VALUE_MAX),
            "channel" => clean(p.channel, VALUE_MAX),
            "date" => format!("{:04}-{:02}-{:02}", start.year, start.month, start.day),
            "time" => format!("{:02}.{:02}", start.hour, start.minute),
            "year" => format!("{:04}", start.year),
            "season" => p.season.map(|s| format!("{s:02}")).unwrap_or_default(),
            "episode" => p.episode.map(|e| format!("{e:02}")).unwrap_or_default(),
            "se" => match (p.season, p.episode) {
                (Some(s), Some(e)) => format!("S{s:02}E{e:02}"),
                (None, Some(e)) => format!("E{e:02}"),
                _ => String::new(),
            },
            _ => String::new(),
        }
    };
    let mut names: Vec<String> = segs.iter().map(|seg| segment(seg, &value)).collect();
    let base = names.pop().filter(|b| !b.is_empty()).unwrap_or_else(|| {
        let fallback = segments(FALLBACK_FILE).expect("the fallback parses");
        segment(&fallback[0], &value)
    });
    Place {
        folders: names.into_iter().filter(|n| !n.is_empty()).collect(),
        base,
    }
}

/// One folder or file name: its tokens filled in, the separators around
/// empty ones collapsed, and made safe. Empty when nothing is left.
fn segment(pieces: &[Piece<'_>], value: &impl Fn(&str) -> String) -> String {
    let mut s: Vec<char> = Vec::new();
    for piece in pieces {
        match piece {
            Piece::Text(t) => s.extend(t.chars().map(|c| if c.is_control() { ' ' } else { c })),
            Piece::Token(name) => match value(name) {
                v if v.is_empty() => s.push(GAP),
                v => s.extend(v.chars()),
            },
        }
    }
    collapse(&mut s);
    clean(&s.into_iter().collect::<String>(), SEGMENT_MAX)
}

fn is_separator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '-' | '_' | '.' | ',' | '–' | '—' | '·')
}

/// Takes out each empty token (a [`GAP`]) and what only made sense beside
/// it:
///
/// 1. Brackets right around it: `{show} ({year})` with no year is `{show} `.
/// 2. The run of separators (spaces, `-`, `_`, `.`, `,`, dashes, `·`)
///    right before it, when there is other text before it in the name;
///    otherwise, at the name's start, the run right after it.
///
/// Gaps are taken from the left, so `{show} - {season} - {episode} - {date}`
/// with neither number is `{show} - {date}`. Letters and digits are never
/// taken: `{season}x{episode}` without a season is `x05`.
fn collapse(s: &mut Vec<char>) {
    let mut i = 0;
    while i < s.len() {
        if s[i] == GAP
            && i > 0
            && i + 1 < s.len()
            && matches!((s[i - 1], s[i + 1]), ('(', ')') | ('[', ']'))
        {
            s.remove(i + 1);
            s.remove(i - 1);
            i -= 1;
            continue;
        }
        i += 1;
    }
    while let Some(i) = s.iter().position(|&c| c == GAP) {
        let mut j = i;
        while j > 0 && is_separator(s[j - 1]) {
            j -= 1;
        }
        if j > 0 {
            s.drain(j..=i);
        } else {
            let mut k = i + 1;
            while k < s.len() && is_separator(s[k]) {
                k += 1;
            }
            s.drain(i..k);
        }
    }
}

/// A name every file system the service may write to accepts, or empty.
///
/// Windows forbids `<>:"/\|?*` and control characters, drops trailing dots
/// and spaces, and reserves `CON`, `NUL`, `COM1` and the like; Linux forbids
/// only `/` and NUL, but the folder may be a Samba share. So the strictest
/// rules apply everywhere. A leading dot would hide the file from a library
/// scan, and `..` would leave the folder, so leading dots go too. At most
/// `max` bytes, cut between characters.
pub fn clean(name: &str, max: usize) -> String {
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
    let mut out = out.trim_start_matches(['.', ' ']).to_string();
    if out.len() > max {
        let mut cut = max;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
    }
    while out.ends_with(['.', ' ']) {
        out.pop();
    }
    let st = stem(&out);
    if reserved(st) {
        out = format!("{st}_{}", &out[st.len()..]);
    }
    out
}

/// The part of a name Windows checks against its reserved names: up to the
/// first dot, without trailing spaces.
fn stem(name: &str) -> &str {
    name.split('.').next().unwrap_or("").trim_end()
}

fn reserved(stem: &str) -> bool {
    let u = stem.to_ascii_uppercase();
    if matches!(u.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    match u.strip_prefix("COM").or_else(|| u.strip_prefix("LPT")) {
        Some(n) => {
            let mut cs = n.chars();
            matches!(
                (cs.next(), cs.next()),
                (Some('0'..='9' | '¹' | '²' | '³'), None)
            )
        }
        None => false,
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

/// What a capture file says to show that it is still being recorded, when
/// other programs are to see it.
pub const RECORDING_MARK: &str = " (recording)";

/// The capture file a part is written to while recording.
///
/// Hidden by default (`.<base> - part1.ts`), so a library scan does not
/// pick up a file that is still growing. With `visible`, for a media server
/// to play while it records: `<base> (recording).ts` for the first part,
/// `<base> - part2 (recording).ts` after. Either way it can never be taken
/// for a finished file ([`is_capture_file`]).
pub fn capture_file(base: &str, part: usize, visible: bool) -> String {
    match (visible, part) {
        (false, _) => format!(".{base} - part{part}.ts"),
        (true, 1) => format!("{base}{RECORDING_MARK}.ts"),
        (true, _) => format!("{base} - part{part}{RECORDING_MARK}.ts"),
    }
}

/// What starts the name of a recording's `.strm` file. Square brackets
/// rather than angle ones, which Windows does not allow in a name.
pub const STRM_MARK: &str = "[RECORDING] ";

/// The `.strm` file that stands for a recording while it records, for
/// Jellyfin (`recording::strm`): `[RECORDING] <base>.strm`, beside where the
/// `.mkv` will be. The mark keeps it apart from the finished file in a
/// library listing, and sorts it with the other recordings in progress.
pub fn strm_file(base: &str) -> String {
    format!("{STRM_MARK}{base}.strm")
}

/// Whether `name` is one of the capture files of a recording named `base`
/// (as [`capture_file`] names them, with or without a [`numbered`] suffix)
/// rather than a finished file: the `.mkv`, or a `.ts` kept because it could
/// not be remuxed, which is named like the `.mkv` and never carries the
/// mark.
pub fn is_capture_file(name: &str, base: &str) -> bool {
    let Some(stem) = name.strip_suffix(".ts") else {
        return false;
    };
    let stem = strip_number(stem);
    let part = |rest: &str| {
        rest.strip_prefix(" - part")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    };
    if let Some(rest) = stem.strip_prefix('.').and_then(|s| s.strip_prefix(base)) {
        return part(rest);
    }
    stem.strip_prefix(base)
        .and_then(|rest| rest.strip_suffix(RECORDING_MARK))
        .is_some_and(|rest| rest.is_empty() || part(rest))
}

/// `stem` without a trailing ` (n)`.
fn strip_number(stem: &str) -> &str {
    stem.strip_suffix(')')
        .and_then(|s| s.rsplit_once(" ("))
        .filter(|(_, n)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        .map_or(stem, |(s, _)| s)
}

/// `name` with ` (n)` before its extension.
pub fn numbered(name: &str, n: u32) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem} ({n}).{ext}"),
        None => format!("{name} ({n})"),
    }
}

/// `name`, or `name (2)`, `name (3)`... before the extension: the first that
/// `exists` says is free. A rerun of the same airing never overwrites the
/// first recording.
pub fn unique(name: &str, exists: impl Fn(&str) -> bool) -> String {
    if !exists(name) {
        return name.to_string();
    }
    (2..)
        .map(|n| numbered(name, n))
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

    fn news() -> Programme<'static> {
        Programme {
            title: "The Evening News",
            channel: "Meridian 24",
            ..Default::default()
        }
    }

    fn harbour() -> Programme<'static> {
        Programme {
            title: "Harbour Lights",
            episode_title: Some("The Lamp Keeper"),
            channel: "Northwind One HD",
            season: Some(2),
            episode: Some(4),
        }
    }

    fn path(pattern: &str, p: &Programme<'_>) -> String {
        place(pattern, p, at(20, 0)).path()
    }

    fn preset(n: Naming) -> &'static str {
        n.preset().unwrap()
    }

    #[test]
    fn the_media_server_preset_is_a_folder_per_show() {
        let p = preset(Naming::MediaServer);
        assert_eq!(
            path(p, &news()),
            "The Evening News/The Evening News - 2026-11-03 20.00"
        );
        assert_eq!(
            path(p, &harbour()),
            "Harbour Lights/Harbour Lights - S02E04 - 2026-11-03 20.00"
        );
        let serial = Programme {
            title: "Serial",
            episode: Some(12),
            ..Default::default()
        };
        assert_eq!(
            place(p, &serial, at(9, 30)).path(),
            "Serial/Serial - E12 - 2026-11-03 09.30"
        );
        assert_eq!(Naming::default(), Naming::MediaServer);
    }

    #[test]
    fn the_other_presets() {
        assert_eq!(
            path(preset(Naming::ByChannel), &harbour()),
            "Northwind One HD/Harbour Lights - 2026-11-03 20.00"
        );
        assert_eq!(
            path(preset(Naming::ByDate), &harbour()),
            "2026-11-03/20.00 Northwind One HD - Harbour Lights"
        );
        let flat = place(preset(Naming::Flat), &news(), at(20, 0));
        assert!(flat.folders.is_empty());
        assert_eq!(
            flat.base,
            "The Evening News - Meridian 24 - 2026-11-03 20.00"
        );
        assert_eq!(Naming::Custom.preset(), None);
        for n in [
            Naming::MediaServer,
            Naming::ByChannel,
            Naming::ByDate,
            Naming::Flat,
        ] {
            assert_eq!(validate(preset(n)), Ok(()), "{n:?}");
        }
    }

    #[test]
    fn every_token() {
        let d = harbour();
        let one = |t: &str| path(&format!("x {{{t}}}"), &d);
        assert_eq!(one("show"), "x Harbour Lights");
        assert_eq!(one("title"), "x The Lamp Keeper");
        assert_eq!(one("channel"), "x Northwind One HD");
        assert_eq!(one("date"), "x 2026-11-03");
        assert_eq!(one("time"), "x 20.00");
        assert_eq!(one("year"), "x 2026");
        assert_eq!(one("season"), "x 02");
        assert_eq!(one("episode"), "x 04");
        assert_eq!(one("se"), "x S02E04");
        assert_eq!(
            place("{time}", &d, at(7, 5)).base,
            "07.05",
            "local time, two digits each"
        );
        assert_eq!(
            path(
                "{se}",
                &Programme {
                    season: Some(3),
                    ..news()
                }
            ),
            "The Evening News - 2026-11-03 20.00",
            "a season alone says nothing"
        );
    }

    #[test]
    fn a_custom_pattern_makes_folders() {
        assert_eq!(
            path("{show}/Season {season}/{show} {se} {title}", &harbour()),
            "Harbour Lights/Season 02/Harbour Lights S02E04 The Lamp Keeper"
        );
        assert_eq!(
            path("{channel}\\{year}\\{show} {date}", &news()),
            "Meridian 24/2026/The Evening News 2026-11-03",
            "a backslash makes a folder too"
        );
        // A folder made only of an empty token is left out.
        assert_eq!(
            path("{show}/Season {season}/{show} - {date}", &news()),
            "The Evening News/Season/The Evening News - 2026-11-03"
        );
        assert_eq!(
            path("{show}/{season}/{show} - {date}", &news()),
            "The Evening News/The Evening News - 2026-11-03"
        );
    }

    #[test]
    fn empty_tokens_take_their_separator_along() {
        let n = news();
        assert_eq!(
            path("{show} - {se} - {date}", &n),
            "The Evening News - 2026-11-03"
        );
        assert_eq!(path("{show} - {title}", &n), "The Evening News");
        assert_eq!(
            path("{se} - {show}", &n),
            "The Evening News",
            "at the start, the one after"
        );
        assert_eq!(
            path("{show} - {season} - {episode} - {date}", &n),
            "The Evening News - 2026-11-03"
        );
        assert_eq!(
            path("{show}_{se}_{date}", &n),
            "The Evening News_2026-11-03"
        );
        assert_eq!(
            path("{show}.{se}.{date}", &n),
            "The Evening News.2026-11-03"
        );
        assert_eq!(
            path("{show}, {title}, {date}", &n),
            "The Evening News, 2026-11-03"
        );
        assert_eq!(
            path("{show} ({title}) {date}", &n),
            "The Evening News 2026-11-03",
            "brackets go too"
        );
        assert_eq!(path("{show} [{se}]", &n), "The Evening News");
        assert_eq!(path("{show}{se}{date}", &n), "The Evening News2026-11-03");
        assert_eq!(
            path("{season}x{episode} {show}", &n),
            "x The Evening News",
            "letters stay"
        );
        // With the values there, nothing moves.
        assert_eq!(
            path("{show} ({year}) [{se}]", &harbour()),
            "Harbour Lights (2026) [S02E04]"
        );
    }

    #[test]
    fn a_file_name_with_nothing_left_falls_back() {
        assert_eq!(
            path("{title}", &news()),
            "The Evening News - 2026-11-03 20.00"
        );
        assert_eq!(
            path("{show}/{se}", &news()),
            "The Evening News/The Evening News - 2026-11-03 20.00"
        );
    }

    #[test]
    fn patterns_are_checked() {
        let bad = |p: &str| validate(p).unwrap_err();
        assert!(bad("").contains("empty"));
        assert!(bad("   ").contains("empty"));
        assert!(bad("{show").contains("not closed"));
        assert!(bad("{show}}").contains("no {"));
        assert!(bad("{sh{ow}").contains("not closed"));
        assert!(bad("{name}").contains("{name} is not a token"));
        assert!(bad("{show}/fixed").contains("needs a token"));
        assert!(bad("{show}/").contains("empty"));
        assert!(bad("{show}//{date}").contains("empty"));
        assert!(bad("{show}: {date}").contains(':'));
        assert!(bad("{show}?").contains('?'));
        assert!(bad("{show}\t{date}").contains("control"));
        assert!(bad(&format!("{{show}}{}", "x".repeat(PATTERN_MAX))).contains("longer"));
        assert_eq!(validate("{show}/{show} - {se} - {date} {time}"), Ok(()));
        assert_eq!(validate(" {show} "), Ok(()));
    }

    #[test]
    fn a_pattern_cannot_leave_the_output_folder() {
        let bad = |p: &str| validate(p).unwrap_err();
        assert!(bad("../{show}").contains("“..”"));
        assert!(bad("{show}/../../{date}").contains("“..”"));
        assert!(bad("./{show}").contains("“.”"));
        assert!(bad("/etc/{show}").contains("cannot start with /"));
        assert!(bad("\\\\server\\{show}").contains("cannot start with /"));
        assert!(bad("C:\\{show}").contains("drive"));
        assert!(bad("c:{show}").contains("drive"));
        assert!(bad("CON/{show}").contains("reserved"));
        assert!(bad("{show}/nul.txt/{date}").contains("reserved"));
        assert!(bad("lpt1/{show}").contains("reserved"));
        assert_eq!(validate("CONTENT/{show}"), Ok(()));
        assert_eq!(validate("COM10/{show}"), Ok(()));

        // Even so, a pattern that got past the check (a hand-edited
        // server.json) is never followed out of the folder.
        for p in ["../{show}", "/etc/{show}", "C:\\{show}", "{show}/../../x"] {
            let got = place(p, &news(), at(20, 0));
            assert_eq!(
                got.path(),
                "The Evening News/The Evening News - 2026-11-03 20.00",
                "{p}"
            );
        }
        // Nor do the guide's titles lead anywhere.
        let evil = Programme {
            title: "../../etc/passwd",
            channel: "..",
            episode_title: Some("C:\\Windows"),
            ..Default::default()
        };
        assert_eq!(
            path("{channel}/{show}/{title} {date}", &evil),
            "-..-etc-passwd/C--Windows 2026-11-03"
        );
        let dots = Programme {
            title: "...",
            channel: "CON",
            ..Default::default()
        };
        assert_eq!(
            path("{channel}/{show} {date}", &dots),
            "CON_/Recording 2026-11-03"
        );
        // A custom pattern only applies when chosen, and only when valid.
        assert_eq!(pattern_for(Naming::Custom, "../x"), MEDIA_SERVER);
        assert_eq!(
            pattern_for(Naming::Custom, "{date}/{show}"),
            "{date}/{show}"
        );
        assert_eq!(
            pattern_for(Naming::Flat, "{date}/{show}"),
            preset(Naming::Flat)
        );
    }

    #[test]
    fn long_names_are_cut_to_fit() {
        let long = "Q".repeat(500);
        let p = Programme {
            title: &long,
            episode_title: Some(&long),
            channel: "C",
            ..Default::default()
        };
        let got = place("{show}/{show} - {title} - {date} {time}", &p, at(20, 0));
        assert_eq!(got.folders[0].len(), VALUE_MAX);
        assert!(got.base.len() <= SEGMENT_MAX, "{}", got.base.len());
        // Each value is cut first, so a long title does not push the date
        // out of a short pattern.
        let got = place("{show} - {date} {time}", &p, at(20, 0));
        assert!(got.base.ends_with(" - 2026-11-03 20.00"), "{}", got.base);
        // In bytes, between characters: 255 bytes is the file systems'
        // limit, and "é" is two of them.
        let accents = "é".repeat(300);
        let p = Programme {
            title: &accents,
            ..Default::default()
        };
        let got = place("{show} {date}", &p, at(20, 0));
        assert!(got.base.len() <= SEGMENT_MAX);
        assert!(got.base.starts_with('é'));
        assert!(file(&got.base, 12, 12, "mkv").len() + ".partial".len() + 1 < 255);
    }

    #[test]
    fn names_are_safe_on_every_file_system() {
        let c = |s: &str| clean(s, 120);
        assert_eq!(c("News: 24/7"), "News - 24-7");
        assert_eq!(c("At 12:30"), "At 12-30");
        assert_eq!(c("What?  <Live>  "), "What Live");
        assert_eq!(c("a\tb\nc"), "a b c");
        assert_eq!(c("Trailing..."), "Trailing");
        assert_eq!(c(".hidden"), "hidden");
        assert_eq!(c(""), "");
        assert_eq!(c(".."), "");
        assert_eq!(c("???"), "");
        assert_eq!(c("con"), "con_");
        assert_eq!(c("Aux.mkv"), "Aux_.mkv");
        assert_eq!(c("COM¹"), "COM¹_");
        assert_eq!(c("Console"), "Console");
        assert_eq!(c(&"x".repeat(500)).len(), 120);
    }

    #[test]
    fn parts_are_numbered_only_when_there_are_several() {
        assert_eq!(
            file("X - 2026-11-03 20.00", 1, 1, "mkv"),
            "X - 2026-11-03 20.00.mkv"
        );
        assert_eq!(
            file("X - 2026-11-03 20.00", 1, 2, "mkv"),
            "X - 2026-11-03 20.00 - part1.mkv"
        );
        assert_eq!(
            file("X - 2026-11-03 20.00", 2, 2, "mkv"),
            "X - 2026-11-03 20.00 - part2.mkv"
        );
        assert_eq!(capture_file("X", 3, false), ".X - part3.ts");
        assert_eq!(capture_file("X", 1, true), "X (recording).ts");
        assert_eq!(capture_file("X", 2, true), "X - part2 (recording).ts");
    }

    #[test]
    fn a_capture_file_is_never_taken_for_a_finished_one() {
        let base = "Harbour Lights - 2026-11-03 20.00";
        for part in 1..=3 {
            for visible in [false, true] {
                let name = capture_file(base, part, visible);
                assert!(is_capture_file(&name, base), "{name}");
                assert!(is_capture_file(&numbered(&name, 2), base), "{name} (2)");
                assert!(!is_capture_file(&name, "Harbour Lights"), "another base");
            }
        }
        // What a recording finishes into, the .ts kept when a remux fails
        // included, whatever the parts and numbers.
        for done in [
            format!("{base}.mkv"),
            format!("{base}.ts"),
            format!("{base} (2).ts"),
            format!("{base} - part2.ts"),
            format!("{base} - part2 (3).ts"),
            format!(".{base}.mkv.partial"),
        ] {
            assert!(!is_capture_file(&done, base), "{done}");
        }
        // A title that looks like the mark, or a number, changes nothing.
        for base in ["Odd (recording)", "Odd (3)", "Odd (recording) (2)"] {
            assert!(is_capture_file(&capture_file(base, 1, true), base));
            assert!(!is_capture_file(&format!("{base}.ts"), base), "{base}");
            assert!(!is_capture_file(
                &unique(&format!("{base}.ts"), |n| n == format!("{base}.ts")),
                base
            ));
        }
    }

    #[test]
    fn a_taken_name_gets_a_number() {
        let taken = ["a.mkv", "a (2).mkv"];
        assert_eq!(unique("b.mkv", |n| taken.contains(&n)), "b.mkv");
        assert_eq!(unique("a.mkv", |n| taken.contains(&n)), "a (3).mkv");
        assert_eq!(unique("noext", |n| n == "noext"), "noext (2)");
    }

    /// The cases the app's preview (src/logic/naming.ts) is checked against
    /// too, so the two cannot drift apart.
    #[test]
    fn the_shared_cases() {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Prog {
            title: String,
            #[serde(default)]
            episode_title: Option<String>,
            #[serde(default)]
            channel: String,
            #[serde(default)]
            season: Option<u32>,
            #[serde(default)]
            episode: Option<u32>,
        }
        #[derive(Deserialize)]
        struct Render {
            pattern: String,
            programme: Prog,
            start: [u32; 5],
            path: String,
        }
        #[derive(Deserialize)]
        struct Check {
            pattern: String,
            error: Option<String>,
        }
        #[derive(Deserialize)]
        struct Strm {
            base: String,
            file: String,
        }
        #[derive(Deserialize)]
        struct Cases {
            render: Vec<Render>,
            validate: Vec<Check>,
            strm: Vec<Strm>,
        }
        let cases: Cases = serde_json::from_str(include_str!("naming_cases.json")).unwrap();
        for c in &cases.render {
            let p = Programme {
                title: &c.programme.title,
                episode_title: c.programme.episode_title.as_deref(),
                channel: &c.programme.channel,
                season: c.programme.season,
                episode: c.programme.episode,
            };
            let [y, mo, d, h, mi] = c.start;
            let start = Civil {
                year: y as i64,
                month: mo,
                day: d,
                hour: h,
                minute: mi,
                second: 0,
            };
            assert_eq!(place(&c.pattern, &p, start).path(), c.path, "{}", c.pattern);
        }
        for c in &cases.validate {
            match (&c.error, validate(&c.pattern)) {
                (None, got) => assert_eq!(got, Ok(()), "{}", c.pattern),
                (Some(want), got) => assert_eq!(got.as_ref().err(), Some(want), "{}", c.pattern),
            }
        }
        assert!(!cases.strm.is_empty());
        for c in &cases.strm {
            assert_eq!(strm_file(&c.base), c.file, "{}", c.base);
            // Whatever the title, a name Windows accepts.
            assert_eq!(clean(&c.file, 255), c.file, "{}", c.file);
        }
    }
}
