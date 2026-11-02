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
    let episode_only = |e: u32| (1..=MAX_EPISODE).contains(&e);
    match b.first()? {
        b'S' => {
            let (end, season) = digits(1);
            let season = season?;
            if b.get(end) != Some(&b'E') {
                return None;
            }
            let (end, episode) = digits(end + 1);
            let episode = episode?;
            (end == b.len() && season < MAX_SEASON && episode_only(episode)).then_some(Episode {
                season: Some(season),
                episode,
            })
        }
        b'E' => {
            let (end, episode) = digits(1);
            let episode = episode?;
            (end == b.len() && episode_only(episode)).then_some(Episode {
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
            (end == b.len() && season < MAX_SEASON && episode_only(episode)).then_some(Episode {
                season: Some(season),
                episode,
            })
        }
        _ => None,
    }
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
