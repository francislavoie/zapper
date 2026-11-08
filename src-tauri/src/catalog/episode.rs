//! Season and episode numbers from a guide's `<episode-num>`.
//!
//! XMLTV says them in several ways, and the two that matter are:
//!
//! - `xmltv_ns`: `season.episode.part`, each **zero-based** and each
//!   optionally `n/total`, any of them empty: `0.4.` is the first season's
//!   fifth episode, `2.9/10.0/1` the third season's tenth of ten. Grabbers
//!   that mean a year by the season (`2025.11.`) make it ambiguous, so a
//!   season that size is not believed.
//! - `onscreen`: what a listing would print. Only the clean forms are read —
//!   `S03E05`, `S3 E5`, `3x05`, `Season 3 Episode 5`, `E05`, `Ep. 5` — and
//!   anything else (`S03E05-06`, `Part 2`, a title) says nothing, so a
//!   double episode or an odd format never names a file wrongly.
//!
//! A guide that gives both is believed in `xmltv_ns`, the machine-readable
//! one. Two onscreen numbers that disagree say nothing.
//!
//! An episode without a season is kept (naming writes `E05`); a season
//! without an episode is not, since it names nothing on its own.
//!
//! A playlist entry's name carries its number among other words (`A Show
//! S01E02 - Title`), and [`find_in_name`] finds it there, with the same
//! numbers and limits. It reads more than a guide's whole field may say, as
//! media servers do: `S01E02` with `[ ._x-]` between the two, `1x02` (only
//! after a show's name and with the episode zero-padded, so `4x4` and `Film
//! 16x9` stay films; the guide's whole-field `3x5` is read as before),
//! `Season 1 Episode 2`,
//! and ranges straight after the episode (`-E02`, `-02`, `E02`, `-02-03`,
//! `-S01E02`, `x02`; a spaced ` - ` only into a whole code, so `S01E02 -
//! Title 3` is no range). The first from the left wins; it must not start
//! in the middle of a word or number, nor end running into one. Dates,
//! `Part 2`, absolute numbers (`- 123`) and resolutions (`1920x1080`) are
//! not numbers here: too easily a channel's name or a year.

/// A programme's place in its series. Both one-based, as people count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Episode {
    pub season: Option<u32>,
    pub episode: u32,
}

/// Seasons at or above this are years, not seasons.
const MAX_SEASON: u32 = 1000;
/// Nothing runs to ten thousand episodes; a number that size is something
/// else (a production code, a date).
const MAX_EPISODE: u32 = 9999;

fn season_ok(season: u32) -> bool {
    season < MAX_SEASON
}

fn episode_ok(episode: u32) -> bool {
    (1..=MAX_EPISODE).contains(&episode)
}

/// One `<episode-num>`, by its `system` attribute (XMLTV's default when it
/// is absent is `onscreen`). Systems that are identifiers rather than
/// numbers, such as `dd_progid`, say nothing.
pub fn parse(system: Option<&str>, text: &str) -> Option<Episode> {
    match system.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("xmltv_ns") => from_xmltv_ns(text),
        None | Some("onscreen") | Some("sxxexx") | Some("common") | Some("") => {
            from_onscreen(text)
        }
        _ => None,
    }
}

/// What a programme's `<episode-num>` elements add up to: `xmltv_ns` when
/// it says something, otherwise the onscreen numbers if they agree.
#[derive(Debug, Default, Clone)]
pub struct Candidates {
    ns: Option<Episode>,
    onscreen: Option<Episode>,
    disagree: bool,
}

impl Candidates {
    pub fn add(&mut self, system: Option<&str>, text: &str) {
        let is_ns = system.is_some_and(|s| s.trim().eq_ignore_ascii_case("xmltv_ns"));
        let Some(e) = parse(system, text) else { return };
        if is_ns {
            self.ns.get_or_insert(e);
        } else {
            match self.onscreen {
                None => self.onscreen = Some(e),
                Some(o) if o != e => self.disagree = true,
                Some(_) => {}
            }
        }
    }

    pub fn result(&self) -> Option<Episode> {
        self.ns.or(if self.disagree { None } else { self.onscreen })
    }
}

/// `season.episode.part`, zero-based, each part `n` or `n/total`.
pub fn from_xmltv_ns(text: &str) -> Option<Episode> {
    let parts: Vec<&str> = text.split('.').collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    let season = ns_part(parts[0])?;
    let episode = ns_part(parts[1])??;
    // The part (`/total` or not) is checked for sense but not kept.
    if let Some(p) = parts.get(2) {
        ns_part(p)?;
    }
    let episode = episode.checked_add(1).filter(|&e| e <= MAX_EPISODE)?;
    let season = match season {
        Some(s) if s + 1 >= MAX_SEASON => return None,
        Some(s) => Some(s + 1),
        None => None,
    };
    Some(Episode { season, episode })
}

/// One part of `xmltv_ns`: `Some(None)` when empty, `Some(Some(n))` for a
/// zero-based number, `None` when it is not one (or exceeds its total).
fn ns_part(p: &str) -> Option<Option<u32>> {
    let p: String = p.chars().filter(|c| !c.is_whitespace()).collect();
    if p.is_empty() {
        return Some(None);
    }
    let (n, total) = match p.split_once('/') {
        Some((n, t)) => (n, Some(t)),
        None => (p.as_str(), None),
    };
    if n.is_empty() {
        // `/10`: the total alone.
        return total.and_then(number).map(|_| None);
    }
    let n = number(n)?;
    if let Some(t) = total {
        let t = number(t)?;
        if t > 0 && n >= t {
            return None;
        }
    }
    Some(Some(n))
}

fn number(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 5 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// The clean onscreen forms; see the module.
pub fn from_onscreen(text: &str) -> Option<Episode> {
    let up = text.trim().to_ascii_uppercase();
    // Words to letters, then the separators people put between them away.
    // A dash or a slash stays, and spoils the match: `S03E05-06` is two
    // episodes, and `1/2` is a part.
    let compact: String = up
        .replace("SEASON", "S")
        .replace("SERIES", "S")
        .replace("EPISODE", "E")
        .replace("EP", "E")
        .chars()
        .filter(|c| !matches!(c, ' ' | '\t' | ',' | '.' | '_' | ':'))
        .collect();
    let b = compact.as_bytes();
    let digits = |from: usize| -> (usize, Option<u32>) {
        let end = b[from..]
            .iter()
            .position(|c| !c.is_ascii_digit())
            .map_or(b.len(), |p| from + p);
        (end, number(&compact[from..end]))
    };
    match b.first()? {
        b'S' => {
            let (end, season) = digits(1);
            let season = season?;
            if b.get(end) != Some(&b'E') {
                return None;
            }
            let (end, episode) = digits(end + 1);
            let episode = episode?;
            (end == b.len() && season_ok(season) && episode_ok(episode)).then_some(Episode {
                season: Some(season),
                episode,
            })
        }
        b'E' => {
            let (end, episode) = digits(1);
            let episode = episode?;
            (end == b.len() && episode_ok(episode)).then_some(Episode {
                season: None,
                episode,
            })
        }
        c if c.is_ascii_digit() => {
            let (end, season) = digits(0);
            let season = season?;
            if b.get(end) != Some(&b'X') {
                return None;
            }
            let (end, episode) = digits(end + 1);
            let episode = episode?;
            (end == b.len() && season_ok(season) && episode_ok(episode)).then_some(Episode {
                season: Some(season),
                episode,
            })
        }
        _ => None,
    }
}

/// Where an episode number sits in a name, and what it says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameMatch {
    /// The byte span the number takes, a range's text included.
    pub start: usize,
    pub end: usize,
    pub season: u32,
    pub first: u32,
    /// A range's last number: in the same season and above `first`. How
    /// long a range is believed is the caller's to say.
    pub last: Option<u32>,
}

/// The digits from `from` in `s`: where they end, and their value when it is
/// a number ([`number`]'s rules).
fn digits_at(s: &str, from: usize) -> (usize, Option<u32>) {
    let b = s.as_bytes();
    let end = b[from..]
        .iter()
        .position(|c| !c.is_ascii_digit())
        .map_or(b.len(), |p| from + p);
    (end, number(&s[from..end]))
}

fn skip(b: &[u8], mut i: usize, set: &[u8]) -> usize {
    while b.get(i).is_some_and(|c| set.contains(&c.to_ascii_lowercase())) {
        i += 1;
    }
    i
}

fn word_at(b: &[u8], i: usize, word: &[u8]) -> Option<usize> {
    let end = i + word.len();
    (b.len() >= end && b[i..end].eq_ignore_ascii_case(word)).then_some(end)
}

/// One episode code starting at `i`: `S01E02` (with `[ ._x-]` between),
/// `Season 1 Episode 2`, or `1x02`. Returns `(season, episode, end)`; `end`
/// is just past the episode's digits, which may still run into something.
fn code_at(s: &str, i: usize) -> Option<(u32, u32, usize)> {
    let b = s.as_bytes();
    let ok = |season: u32, episode: u32| season_ok(season) && episode_ok(episode);
    match b[i].to_ascii_lowercase() {
        b's' => {
            if let Some(after) = word_at(b, i, b"season") {
                let (end, season) = digits_at(s, skip(b, after, b" ._-"));
                let after = word_at(b, skip(b, end, b" ._-"), b"episode")?;
                let (end, episode) = digits_at(s, skip(b, after, b" ._-"));
                let (season, episode) = (season?, episode?);
                return ok(season, episode).then_some((season, episode, end));
            }
            let (end, season) = digits_at(s, i + 1);
            let season = season?;
            let e = skip(b, end, b" ._x-");
            if !b.get(e).is_some_and(|c| c.eq_ignore_ascii_case(&b'e')) {
                return None;
            }
            let (end, episode) = digits_at(s, e + 1);
            let episode = episode?;
            ok(season, episode).then_some((season, episode, end))
        }
        c if c.is_ascii_digit() => {
            let (x, season) = digits_at(s, i);
            if x - i > 2 || !b.get(x).is_some_and(|c| c.eq_ignore_ascii_case(&b'x')) {
                return None;
            }
            // The episode zero-padded, as such codes are written: `1x02`,
            // where `4x4` and `16x9` are names.
            let (end, episode) = digits_at(s, x + 1);
            if !(2..=3).contains(&(end - (x + 1))) {
                return None;
            }
            let (season, episode) = (season?, episode?);
            ok(season, episode).then_some((season, episode, end))
        }
        _ => None,
    }
}

/// What continues a code at `i` into a range: a dash (spaced or not) into a
/// whole code (`-S01E02`, ` - 1x02`), or straight on, `[-_ex]+` and a
/// number (`-E02`, `-02`, `E02`, `x02`). Returns `(season, episode, end)`,
/// the season `None` when only an episode was given.
fn more_at(s: &str, i: usize) -> Option<(Option<u32>, u32, usize)> {
    let b = s.as_bytes();
    let dash = skip(b, i, b" ");
    if b.get(dash) == Some(&b'-') {
        let j = skip(b, dash + 1, b" ");
        if b.get(j).is_some_and(u8::is_ascii_alphanumeric) {
            if let Some((season, episode, end)) = code_at(s, j) {
                return Some((Some(season), episode, end));
            }
        }
    }
    let j = skip(b, i, b"-_ex");
    if j == i {
        return None;
    }
    let (end, episode) = digits_at(s, j);
    let episode = episode.filter(|&e| episode_ok(e))?;
    Some((None, episode, end))
}

/// The first episode number in a playlist entry's name, from the left; see
/// the playlist rules in the module's notes.
pub fn find_in_name(name: &str) -> Option<NameMatch> {
    let b = name.as_bytes();
    let runs_on = |at: usize| name[at..].chars().next().is_some_and(char::is_alphanumeric);
    (0..b.len()).find_map(|start| {
        // An ASCII byte is always a character's start.
        if !b[start].is_ascii_alphanumeric()
            || name[..start].chars().next_back().is_some_and(char::is_alphanumeric)
        {
            return None;
        }
        // `4x4` is a film: the short form needs a show's name before it.
        if b[start].is_ascii_digit() && !name[..start].contains(char::is_alphanumeric) {
            return None;
        }
        let (season, first, end) = code_at(name, start)?;
        let mut more = Vec::new();
        let mut at = end;
        while let Some(m) = more_at(name, at) {
            at = m.2;
            more.push(m);
        }
        // A continuation that runs into a word (`-720p`) is not one.
        while more.last().is_some_and(|m| runs_on(m.2)) {
            more.pop();
        }
        let end = more.last().map_or(end, |m| m.2);
        if runs_on(end) {
            return None;
        }
        let mut prev = first;
        let range = more.iter().all(|&(s, e, _)| {
            let ascending = s.unwrap_or(season) == season && e > prev;
            prev = e;
            ascending
        });
        Some(NameMatch {
            start,
            end,
            season,
            first,
            last: more.last().filter(|_| range).map(|m| m.1),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn se(season: Option<u32>, episode: u32) -> Option<Episode> {
        Some(Episode { season, episode })
    }

    #[test]
    fn xmltv_ns_is_zero_based_with_optional_totals() {
        let cases: &[(&str, Option<Episode>)] = &[
            ("0.4.", se(Some(1), 5)),
            ("2.9/10.0/1", se(Some(3), 10)),
            ("2 . 9 . 0/2", se(Some(3), 10)),
            (" 11.0.", se(Some(12), 1)),
            ("1/5.3/12.", se(Some(2), 4)),
            (".4.", se(None, 5)),
            ("/4.2.", se(None, 3)),
            ("0.4", se(Some(1), 5)),
            // A season alone names nothing.
            ("2..", None),
            ("..", None),
            // Years in the season: ambiguous.
            ("2024.3.", None),
            ("1999.0.", None),
            // An episode beyond its own total.
            ("0.10/10.", None),
            ("0.12/10.", None),
            // Not numbers.
            ("a.b.c", None),
            ("S01E02", None),
            ("", None),
            ("1.2.3.4", None),
            ("-1.2.", None),
            ("0.99999999.", None),
        ];
        for (text, want) in cases {
            assert_eq!(from_xmltv_ns(text), *want, "{text:?}");
        }
    }

    #[test]
    fn the_clean_onscreen_forms_are_read() {
        let cases: &[(&str, Option<Episode>)] = &[
            ("S03E05", se(Some(3), 5)),
            ("s03e05", se(Some(3), 5)),
            ("S3 E5", se(Some(3), 5)),
            ("S03.E05", se(Some(3), 5)),
            ("3x05", se(Some(3), 5)),
            ("1x02", se(Some(1), 2)),
            ("12X101", se(Some(12), 101)),
            ("Season 3, Episode 5", se(Some(3), 5)),
            ("Series 2 Episode 1", se(Some(2), 1)),
            ("S00E03", se(Some(0), 3)),
            ("E05", se(None, 5)),
            ("Ep. 5", se(None, 5)),
            ("Episode 12", se(None, 12)),
            // Two episodes, parts, junk: nothing.
            ("S03E05-06", None),
            ("S03E05-E06", None),
            ("S03E05E06", None),
            ("Part 2", None),
            ("1/2", None),
            ("S03", None),
            ("E0", None),
            ("S2024E05", None),
            ("Harbour Lights", None),
            ("", None),
            ("S03E05 The Lamp Keeper", None),
            ("x05", None),
            ("3x", None),
        ];
        for (text, want) in cases {
            assert_eq!(from_onscreen(text), *want, "{text:?}");
        }
    }

    #[test]
    fn a_number_in_a_name_is_found_with_its_span() {
        fn m(name: &str) -> Option<(&str, u32, u32, Option<u32>)> {
            find_in_name(name).map(|m| (&name[m.start..m.end], m.season, m.first, m.last))
        }
        assert_eq!(m("A Show S01E01-E02 Title"), Some(("S01E01-E02", 1, 1, Some(2))));
        assert_eq!(m("A Show 1x01 - 1x02"), Some(("1x01 - 1x02", 1, 1, Some(2))));
        assert_eq!(m("A Show S01E03-E02"), Some(("S01E03-E02", 1, 3, None)));
        assert_eq!(m("A Show S01E02-720p"), Some(("S01E02", 1, 2, None)));
        // Inside a word, or running into one: not a number.
        assert_eq!(m("A ShowS01E02"), None);
        assert_eq!(m("A Show S01E02x"), None);
        assert_eq!(m("A Show S01E02nd"), None);
        // The first from the left wins.
        assert_eq!(m("A Show S01E02 S03E04"), Some(("S01E02", 1, 2, None)));
        // A playlist's `NxM` has its episode zero-padded; a guide's field
        // is read as it always was.
        assert_eq!(m("A Show 10x103"), Some(("10x103", 10, 103, None)));
        assert_eq!(m("Desert 4x4"), None);
        assert_eq!(from_onscreen("3x5"), se(Some(3), 5));
    }

    #[test]
    fn the_system_decides_how_it_is_read() {
        assert_eq!(parse(Some("xmltv_ns"), "0.4."), se(Some(1), 5));
        assert_eq!(parse(Some("onscreen"), "0.4."), None);
        assert_eq!(parse(None, "S01E05"), se(Some(1), 5));
        assert_eq!(parse(Some("SxxExx"), "S01E05"), se(Some(1), 5));
        assert_eq!(parse(Some("dd_progid"), "EP01234567.0012"), None);
        assert_eq!(parse(Some("xmltv_ns"), "S01E05"), None);
    }

    #[test]
    fn xmltv_ns_wins_and_disagreeing_onscreen_numbers_say_nothing() {
        let mut c = Candidates::default();
        c.add(Some("onscreen"), "S09E09");
        c.add(Some("xmltv_ns"), "1.4.");
        assert_eq!(c.result(), se(Some(2), 5));

        let mut c = Candidates::default();
        c.add(Some("onscreen"), "S01E02");
        c.add(None, "1x02");
        assert_eq!(c.result(), se(Some(1), 2), "the same number twice agrees");
        c.add(Some("onscreen"), "S01E03");
        assert_eq!(c.result(), None);

        let mut c = Candidates::default();
        c.add(Some("dd_progid"), "EP0123.0004");
        c.add(Some("xmltv_ns"), "garbage");
        assert_eq!(c.result(), None);
    }
}
