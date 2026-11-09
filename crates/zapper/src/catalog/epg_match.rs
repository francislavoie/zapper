//! Which guide channel a playlist entry is.
//!
//! An Xtream server says so outright (`epg_channel_id`). A playlist says so
//! only when its author filled in `tvg-id` *and* it agrees with the guide's
//! own ids, which in practice is the case for perhaps half the entries in a
//! hand-assembled playlist. So the guide is matched in three steps, best
//! evidence first, and the answer stored as the channel's `epg_channel_id` —
//! after which every guide query in the app works on it unchanged:
//!
//! 1. `tvg-id` equal to a guide channel's id (case-insensitively, since
//!    `NW1.uk` and `nw1.uk` are the same author being inconsistent).
//! 2. `tvg-name`, normalised, equal to one of a guide channel's normalised
//!    display names.
//! 3. The entry's own name, normalised, likewise.
//!
//! Failing all three, the `tvg-id` is kept as written: a guide with
//! programmes but no `<channel>` elements still matches on it, and an id that
//! matches nothing costs nothing.
//!
//! **Normalising** is what makes step 2 and 3 work at all: `UK: Northwind One
//! FHD`, `Northwind One [HD]` and the guide's `Northwind One` must meet. Decorations
//! (`|UK|`, brackets, parentheses) go first via [`clean_name`], then a short
//! country prefix (`UK:`), then case and punctuation, then quality words
//! (HD, FHD, UHD, 4K, 1080p, HEVC…). `+` becomes "plus" first, so `Lumen+`
//! and `Lumen` stay different channels.
//!
//! When two guide channels normalise to the same name — regional variants,
//! usually — the first one the guide listed wins. That is a guess, but a
//! stable one, and a `tvg-id` in the playlist overrides it.

use std::collections::{HashMap, HashSet};

use crate::catalog::sync::clean_name;

/// Words that describe the picture rather than the channel.
const QUALITY_WORDS: &[&str] = &[
    "sd", "hd", "fhd", "uhd", "qhd", "hq", "lq", "4k", "8k", "hdr", "hevc", "h264", "h265", "x264",
    "x265", "480p", "576p", "720p", "1080p", "1080i", "2160p", "50fps", "60fps", "backup", "raw",
    "multi",
];

/// Reduces a channel name to what identifies the channel.
pub fn normalise(raw: &str) -> String {
    let cleaned = clean_name(raw).to_lowercase();
    // `UK: Northwind One`, `FR: Lumen` — a two or three letter country code and a
    // colon. Longer prefixes are too likely to be part of the name.
    let body = match cleaned.split_once(':') {
        Some((pre, rest))
            if (2..=3).contains(&pre.trim().len())
                && pre.trim().chars().all(|c| c.is_alphabetic()) =>
        {
            rest
        }
        _ => cleaned.as_str(),
    };
    let body = body.replace('+', " plus ").replace('&', " and ");
    let tokens: Vec<&str> = body
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    let kept: String = tokens
        .iter()
        .filter(|t| !QUALITY_WORDS.contains(t))
        .copied()
        .collect();
    if kept.is_empty() {
        // A name that is nothing but quality words ("HD") is still a name.
        tokens.concat()
    } else {
        kept
    }
}

/// The guide's channels, indexed for the three lookups.
#[derive(Default)]
pub struct Matcher {
    ids: HashSet<String>,
    ids_lower: HashMap<String, String>,
    names: HashMap<String, String>,
}

impl Matcher {
    /// `ids` are every channel id the guide knows (declared or with
    /// programmes); `names` are `(channel id, display name)` in guide order.
    pub fn new(
        ids: impl IntoIterator<Item = String>,
        names: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        let mut m = Matcher::default();
        for id in ids {
            m.add_id(id);
        }
        for (id, name) in names {
            m.add_id(id.clone());
            let n = normalise(&name);
            if !n.is_empty() {
                m.names.entry(n).or_insert(id);
            }
        }
        m
    }

    fn add_id(&mut self, id: String) {
        self.ids_lower
            .entry(id.to_lowercase())
            .or_insert_with(|| id.clone());
        self.ids.insert(id);
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// The guide channel for an entry, or `None` if nothing matched.
    pub fn resolve(
        &self,
        tvg_id: Option<&str>,
        tvg_name: Option<&str>,
        name: &str,
    ) -> Option<String> {
        if let Some(id) = tvg_id.map(str::trim).filter(|s| !s.is_empty()) {
            if self.ids.contains(id) {
                return Some(id.to_owned());
            }
            if let Some(real) = self.ids_lower.get(&id.to_lowercase()) {
                return Some(real.clone());
            }
        }
        for candidate in [tvg_name, Some(name)].into_iter().flatten() {
            let n = normalise(candidate);
            if let Some(id) = self.names.get(&n) {
                return Some(id.clone());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decorations_quality_and_country_prefixes_are_ignored() {
        for raw in [
            "Northwind One",
            "UK: Northwind One FHD",
            "Northwind One [HD]",
            "|UK| NORTHWIND ONE 1080p",
            "northwind-one hevc",
        ] {
            assert_eq!(normalise(raw), "northwindone", "{raw}");
        }
    }

    #[test]
    fn plus_and_ampersand_are_words_not_punctuation() {
        assert_ne!(normalise("Lumen+"), normalise("Lumen"));
        assert_eq!(normalise("Lumen+ HD"), normalise("lumen plus"));
        assert_eq!(normalise("Bay & Bridge Sports"), normalise("Bay and Bridge Sports"));
    }

    #[test]
    fn a_name_of_only_quality_words_survives() {
        assert_eq!(normalise("HD"), "hd");
    }

    #[test]
    fn longer_prefixes_are_part_of_the_name() {
        // "Harbour: Extra" is not a country code.
        assert_eq!(normalise("Harbour: Extra"), "harbourextra");
    }

    #[test]
    fn non_latin_names_keep_their_letters() {
        assert_eq!(normalise("Северный ветер HD"), "северныйветер");
    }

    fn matcher() -> Matcher {
        Matcher::new(
            ["orphan.id".to_owned()],
            [
                ("NW1.uk".to_owned(), "Northwind One".to_owned()),
                ("nw1.london".to_owned(), "Northwind One".to_owned()),
                ("m24.fr".to_owned(), "Meridian 24".to_owned()),
            ],
        )
    }

    #[test]
    fn tvg_id_wins_then_tvg_name_then_name() {
        let m = matcher();
        assert_eq!(
            m.resolve(Some("m24.fr"), Some("Northwind One"), "x").as_deref(),
            Some("m24.fr")
        );
        // Same id, different case.
        assert_eq!(
            m.resolve(Some("nw1.UK"), None, "x").as_deref(),
            Some("NW1.uk")
        );
        // An id the guide declares only through programmes.
        assert_eq!(
            m.resolve(Some("orphan.id"), None, "x").as_deref(),
            Some("orphan.id")
        );
        assert_eq!(
            m.resolve(Some("nope"), Some("Meridian 24 HD"), "x")
                .as_deref(),
            Some("m24.fr")
        );
        assert_eq!(
            m.resolve(None, None, "FR: Meridian 24 [FHD]").as_deref(),
            Some("m24.fr")
        );
        assert_eq!(m.resolve(Some("nope"), None, "Unknown Channel"), None);
    }

    #[test]
    fn the_first_guide_channel_wins_a_shared_name() {
        assert_eq!(
            matcher().resolve(None, None, "Northwind One HD").as_deref(),
            Some("NW1.uk")
        );
    }
}
