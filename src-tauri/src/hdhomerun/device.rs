//! Asking a tuner what it is and what it can tune, and turning its lineup
//! into playlist entries.
//!
//! **Ids.** A lineup entry's id is derived from the tuner's `DeviceID` and
//! the entry's `GuideNumber`, nothing else. Those two are what make it the
//! same channel next week: a broadcaster renaming `KXYZ-HD` to `KXYZ` is the
//! same channel on the same number, and history, favourites and quality
//! measurements should follow it. They hash into the same range as a
//! playlist's ids (`m3u::ids`, above anything an Xtream server issues), so
//! switching between account kinds never mistakes one for another. A rescan
//! that moves a station to a new number does make it a new channel — the
//! number is the only identity the lineup offers that a rename cannot touch.
//!
//! **Numbers.** Over the air, channels are `5.1`, `5.2`: a major number and
//! a subchannel. The mirror's `num` is an integer the listing sorts by, so a
//! lineup with any subchannel in it numbers every entry `major * 1000 +
//! minor` and keeps the written form beside it (`m3u::parse::channel_number`).
//! A cable lineup — plain `702` throughout — keeps plain numbers.
//!
//! **Categories.** A tuner has no channel groups. What it does have, and what
//! is worth sorting by, is the owner's favourites (starred in SiliconDust's
//! own app, `Favorite: 1`) and whether a channel is HD. So: `Favourites`
//! first when there are any, then `HD`, then `SD`, each channel in exactly
//! one. Two or three groups are the right size for a lineup of a few dozen
//! channels; by-network or by-genre would need the guide, and the guide is
//! optional.
//!
//! **Copy protection.** Entries marked `DRM: 1` — premium cable on a
//! CableCARD tuner, some ATSC 3.0 stations — stream encrypted, and mpv
//! cannot decrypt them. They are left out rather than listed: a channel that
//! fails every time it is opened is worse than a missing one, and nothing in
//! zapper can make it play. The count is reported by the connection test and
//! the sync, so they are not silently missing.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::fmt;
use std::time::Duration;

use crate::m3u::parse::{channel_number, MINOR_SPAN};
use crate::m3u::Entry;

use super::{flag, text};

/// The categories a lineup can produce, in the order the rail shows them.
pub const FAVOURITES: &str = "Favourites";
pub const HD: &str = "HD";
pub const SD: &str = "SD";

/// The tuner's root, from whatever was typed: a bare address gets `http://`,
/// and a pasted `…/discover.json` or trailing slash is taken off.
pub fn base_url(device: &str) -> Result<String> {
    let t = device.trim();
    if t.is_empty() {
        return Err(anyhow!("an HDHomeRun address is required"));
    }
    if t.chars().any(char::is_whitespace) {
        return Err(anyhow!("the HDHomeRun address has a space in it"));
    }
    let lower = t.to_ascii_lowercase();
    let mut base = if lower.starts_with("http://") || lower.starts_with("https://") {
        t.to_owned()
    } else if lower.contains("://") {
        return Err(anyhow!("an HDHomeRun is reached over http://"));
    } else {
        format!("http://{t}")
    };
    for tail in ["/discover.json", "/lineup.json"] {
        if base.to_ascii_lowercase().ends_with(tail) {
            base.truncate(base.len() - tail.len());
        }
    }
    Ok(base.trim_end_matches('/').to_owned())
}

/// What `/discover.json` says. Every field defaults: firmware from 2014 and
/// firmware from last month disagree about which ones exist.
#[derive(Clone, Default, Deserialize)]
pub struct Discover {
    #[serde(rename = "FriendlyName", default)]
    pub friendly_name: String,
    #[serde(rename = "ModelNumber", default)]
    pub model_number: String,
    #[serde(rename = "DeviceID", default)]
    pub device_id: String,
    /// The token SiliconDust's guide service knows this tuner by. It rotates,
    /// so it is read fresh each sync and never stored; and it is a
    /// credential, so it never reaches a log ([`fmt::Debug`] below).
    #[serde(rename = "DeviceAuth", default)]
    pub device_auth: String,
    #[serde(rename = "LineupURL", default)]
    pub lineup_url: String,
    #[serde(rename = "TunerCount", default)]
    pub tuner_count: Option<serde_json::Value>,
}

impl fmt::Debug for Discover {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Discover")
            .field("friendly_name", &self.friendly_name)
            .field("model_number", &self.model_number)
            .field("device_id", &self.device_id)
            .field("device_auth", &"<redacted>")
            .field("lineup_url", &self.lineup_url)
            .field("tuner_count", &self.tuners())
            .finish()
    }
}

impl Discover {
    pub fn tuners(&self) -> Option<u32> {
        match &self.tuner_count {
            Some(serde_json::Value::Number(n)) => n.as_u64().and_then(|n| u32::try_from(n).ok()),
            Some(serde_json::Value::String(s)) => s.trim().parse().ok(),
            _ => None,
        }
    }

    /// "HDHomeRun FLEX 4K (HDFX-4K)", or as much of it as the tuner gave.
    pub fn title(&self) -> String {
        match (self.friendly_name.trim(), self.model_number.trim()) {
            ("", "") => "HDHomeRun".to_owned(),
            (name, "") => name.to_owned(),
            ("", model) => format!("HDHomeRun ({model})"),
            (name, model) => format!("{name} ({model})"),
        }
    }
}

/// One `/lineup.json` entry. The flags are raw JSON because firmware writes
/// them as numbers, strings or not at all; see [`flag`].
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LineupEntry {
    #[serde(rename = "GuideNumber", default)]
    pub guide_number: Option<serde_json::Value>,
    #[serde(rename = "GuideName", default)]
    pub guide_name: String,
    #[serde(rename = "URL", default)]
    pub url: String,
    #[serde(rename = "HD", default)]
    pub hd: Option<serde_json::Value>,
    #[serde(rename = "Favorite", default)]
    pub favorite: Option<serde_json::Value>,
    #[serde(rename = "DRM", default)]
    pub drm: Option<serde_json::Value>,
}

impl LineupEntry {
    pub fn number(&self) -> String {
        text(&self.guide_number)
    }

    pub fn protected(&self) -> bool {
        flag(&self.drm)
    }
}

/// A lineup as playlist entries, each with the identity its id is derived
/// from, plus what was left out.
#[derive(Debug, Default)]
pub struct Lineup {
    pub entries: Vec<(Entry, String)>,
    /// Copy-protected entries, not listed (see the module notes).
    pub protected: usize,
}

/// The guide channel a lineup entry is filed under: the same key the
/// SiliconDust guide is written with ([`super::guide`]), so the two meet
/// without any matching.
pub fn guide_ref(guide_number: &str) -> String {
    format!("hdhr:{guide_number}")
}

/// Turns a lineup into entries. `device_id` scopes the ids to this tuner.
pub fn entries(device_id: &str, lineup: &[LineupEntry]) -> Lineup {
    let device = device_id.trim().to_ascii_uppercase();
    // Subchannel numbering is all or nothing: `702` beside `5.1` would sort
    // as 702 against 5001, so one minor number anywhere puts every entry on
    // the `major * 1000 + minor` scale.
    let any_minor = lineup
        .iter()
        .filter_map(|e| channel_number(&e.number()))
        .any(|(_, label)| label.is_some());
    let mut out = Lineup::default();
    for l in lineup {
        if l.protected() {
            out.protected += 1;
            continue;
        }
        let number = l.number();
        if l.url.trim().is_empty() {
            continue;
        }
        let (chno, chno_label) = match channel_number(&number) {
            Some((n, None)) if any_minor => (Some(n * MINOR_SPAN), Some(number.clone())),
            Some((n, label)) => (Some(n), label),
            None => (None, None),
        };
        let name = if l.guide_name.trim().is_empty() {
            number.clone()
        } else {
            l.guide_name.trim().to_owned()
        };
        let group = if flag(&l.favorite) {
            FAVOURITES
        } else if flag(&l.hd) {
            HD
        } else {
            SD
        };
        let entry = Entry {
            name: name.clone(),
            named: true,
            url: l.url.trim().to_owned(),
            tvg_id: Some(guide_ref(&number)),
            // The call sign, for matching an XMLTV guide by name when one is
            // configured in place of SiliconDust's.
            tvg_name: Some(name),
            group: Some(group.to_owned()),
            chno,
            chno_label,
            ..Entry::default()
        };
        out.entries.push((entry, format!("hdhr\u{0}{device}\u{0}{number}")));
    }
    out
}

/// The order the categories are shown in.
pub fn group_order() -> Vec<String> {
    [FAVOURITES, HD, SD].map(str::to_owned).to_vec()
}

/// A tuner on the network.
pub struct Device {
    base: String,
    http: reqwest::Client,
}

impl Device {
    pub fn new(address: &str) -> Result<Self> {
        let base = base_url(address)?;
        let http = reqwest::Client::builder()
            .user_agent(concat!("zapper/", env!("CARGO_PKG_VERSION")))
            // On the LAN: a tuner that has not answered in five seconds is
            // off, or not at this address.
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .build()
            .context("building http client")?;
        Ok(Self { base, http })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str, what: &str) -> Result<T> {
        let resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .with_context(|| format!("could not reach the HDHomeRun at {} ({what})", self.base))?
            .error_for_status()
            .map_err(reqwest::Error::without_url)
            .with_context(|| format!("the HDHomeRun refused {what}"))?;
        let body = resp
            .bytes()
            .await
            .map_err(reqwest::Error::without_url)
            .with_context(|| format!("reading {what}"))?;
        serde_json::from_slice(&body)
            .with_context(|| format!("{what} from {} is not what an HDHomeRun sends", self.base))
    }

    pub async fn discover(&self) -> Result<Discover> {
        self.get_json(&format!("{}/discover.json", self.base), "discover.json")
            .await
    }

    /// The lineup, from where the tuner says it is, or the usual place if it
    /// does not say.
    pub async fn lineup(&self, d: &Discover) -> Result<Vec<LineupEntry>> {
        let url = if d.lineup_url.trim().is_empty() {
            format!("{}/lineup.json", self.base)
        } else {
            d.lineup_url.trim().to_owned()
        };
        self.get_json(&url, "the channel lineup").await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DISCOVER: &str = include_str!("../../tests/fixtures/hdhomerun/discover.json");
    const LINEUP: &str = include_str!("../../tests/fixtures/hdhomerun/lineup.json");

    fn lineup() -> Vec<LineupEntry> {
        serde_json::from_str(LINEUP).unwrap()
    }

    #[test]
    fn addresses_are_read_however_they_are_typed() {
        assert_eq!(base_url("192.168.1.50").unwrap(), "http://192.168.1.50");
        assert_eq!(base_url(" hdhomerun.local/ ").unwrap(), "http://hdhomerun.local");
        assert_eq!(
            base_url("http://10.0.0.7:8080/discover.json").unwrap(),
            "http://10.0.0.7:8080"
        );
        assert!(base_url("").is_err());
        assert!(base_url("rtsp://10.0.0.7").is_err());
        assert!(base_url("10.0.0.7 5004").is_err());
    }

    #[test]
    fn discover_reads_and_never_shows_its_token() {
        let d: Discover = serde_json::from_str(DISCOVER).unwrap();
        assert_eq!(d.title(), "HDHomeRun FLEX 4K (HDFX-4K)");
        assert_eq!(d.tuners(), Some(4));
        assert_eq!(d.device_id, "10ABCDEF");
        let shown = format!("{d:?}");
        assert!(!shown.contains(&d.device_auth), "{shown}");
        // Strings where numbers belong, and nothing at all, both read.
        let d: Discover = serde_json::from_str(r#"{"TunerCount":"2"}"#).unwrap();
        assert_eq!((d.tuners(), d.title().as_str()), (Some(2), "HDHomeRun"));
    }

    #[test]
    fn a_lineup_becomes_entries_with_stable_ids_and_sortable_numbers() {
        let l = entries("10abcdef", &lineup());
        // Seven entries, one copy-protected.
        assert_eq!((l.entries.len(), l.protected), (6, 1));
        let (first, ident) = &l.entries[0];
        assert_eq!(first.name, "KXMO-HD");
        assert_eq!(first.url, "http://192.168.1.50:5004/auto/v4.1");
        assert_eq!((first.chno, first.chno_label.as_deref()), (Some(4001), Some("4.1")));
        assert_eq!(first.tvg_id.as_deref(), Some("hdhr:4.1"));
        // Upper-cased, so a DeviceID typed either way is one tuner.
        assert_eq!(ident, "hdhr\u{0}10ABCDEF\u{0}4.1");
        // A whole number in a lineup with subchannels is on the same scale.
        let whole = l.entries.iter().find(|(e, _)| e.name == "KXHL").unwrap();
        assert_eq!((whole.0.chno, whole.0.chno_label.as_deref()), (Some(9000), Some("9")));
        // 4.2 before 4.10.
        let num = |n: &str| l.entries.iter().find(|(e, _)| e.name == n).unwrap().0.chno;
        assert!(num("KXMO-D2") < num("KXMO-D10"));
    }

    #[test]
    fn categories_are_favourites_then_hd_then_sd() {
        let l = entries("10ABCDEF", &lineup());
        let group = |n: &str| {
            l.entries
                .iter()
                .find(|(e, _)| e.name == n)
                .unwrap()
                .0
                .group
                .clone()
                .unwrap()
        };
        assert_eq!(group("KXNG-HD"), FAVOURITES, "a favourite HD channel is a favourite");
        assert_eq!(group("KXMO-HD"), HD);
        assert_eq!(group("KXMO-D2"), SD);
    }

    #[test]
    fn a_cable_lineup_keeps_plain_numbers() {
        let cable: Vec<LineupEntry> = serde_json::from_str(
            r#"[{"GuideNumber":"702","GuideName":"XSPN","URL":"http://h:5004/auto/v702"},
                {"GuideNumber":703,"GuideName":"WNN","URL":"http://h:5004/auto/v703"}]"#,
        )
        .unwrap();
        let l = entries("1", &cable);
        let nums: Vec<_> = l.entries.iter().map(|(e, _)| (e.chno, e.chno_label.clone())).collect();
        assert_eq!(nums, [(Some(702), None), (Some(703), None)]);
    }

    #[test]
    fn ids_belong_to_the_device_and_the_number_not_the_name() {
        let renamed: Vec<LineupEntry> = serde_json::from_str(
            r#"[{"GuideNumber":"4.1","GuideName":"KXMO","URL":"http://192.168.1.50:5004/auto/v4.1"}]"#,
        )
        .unwrap();
        let before = &entries("10ABCDEF", &lineup()).entries[0].1;
        let after = &entries("10ABCDEF", &renamed).entries[0].1;
        assert_eq!(before, after);
        let other_tuner = &entries("20000000", &renamed).entries[0].1;
        assert_ne!(before, other_tuner);
    }
}
