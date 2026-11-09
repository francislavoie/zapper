//! The picture menu's half of the player: the stream's audio and subtitle
//! tracks, and how the picture is fitted and deinterlaced.
//!
//! Pure. The commands are in `player::controller`; this is what they translate
//! between mpv's vocabulary and the contract's.

use crate::ipc::types::{AspectMode, DeinterlaceMode, TrackDto};

/// Whether mpv's `track-list` has a video track in it: `None` while it is
/// empty or unreadable (the file is not loaded yet). A cover picture
/// (`albumart`) is not video.
pub fn has_video(json: &str) -> Option<bool> {
    let Ok(serde_json::Value::Array(entries)) = serde_json::from_str::<serde_json::Value>(json) else {
        return None;
    };
    if entries.is_empty() {
        return None;
    }
    Some(entries.iter().any(|e| {
        e.get("type").and_then(|v| v.as_str()) == Some("video")
            && !e.get("albumart").and_then(|v| v.as_bool()).unwrap_or(false)
    }))
}

/// The audio and subtitle tracks in mpv's `track-list`, in its order.
///
/// mpv renders the list as JSON when it is read (or observed) as a string,
/// the same trick `Player::cache_window` relies on. Anything unreadable is
/// an empty list rather than an error: the menu then offers no track choice,
/// which is also what a single-track stream gets.
pub fn parse_track_list(json: &str) -> (Vec<TrackDto>, Vec<TrackDto>) {
    let Ok(serde_json::Value::Array(entries)) = serde_json::from_str::<serde_json::Value>(json)
    else {
        return (Vec::new(), Vec::new());
    };
    let mut audio = Vec::new();
    let mut subtitles = Vec::new();
    for e in &entries {
        let Some(id) = e.get("id").and_then(|v| v.as_i64()) else {
            continue;
        };
        let text = |key: &str| {
            e.get(key)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        let flag = |key: &str| e.get(key).and_then(|v| v.as_bool()).unwrap_or(false);
        let track = TrackDto {
            id,
            title: text("title"),
            // "und" is the standard's own word for "not said", and showing
            // it would be showing a code rather than a language.
            lang: text("lang").filter(|l| !matches!(l.as_str(), "und" | "zxx" | "mis")),
            codec: text("codec"),
            channels: e
                .get("demux-channel-count")
                .or_else(|| e.get("audio-channels"))
                .and_then(|v| v.as_i64())
                .filter(|n| *n > 0),
            default: flag("default"),
            forced: flag("forced"),
            hearing_impaired: flag("hearing-impaired"),
            visual_impaired: flag("visual-impaired"),
            selected: flag("selected"),
        };
        match e.get("type").and_then(|v| v.as_str()) {
            Some("audio") => audio.push(track),
            Some("sub") => subtitles.push(TrackDto {
                channels: None,
                ..track
            }),
            _ => {}
        }
    }
    (audio, subtitles)
}

/// The mpv properties that express an aspect mode.
///
/// Three properties, and every mode sets all three, so that leaving a mode
/// undoes it: Fill is `panscan` and Stretch is `keepaspect`, and either one
/// left behind would quietly crop or distort whatever was chosen next.
pub fn aspect_properties(mode: AspectMode) -> [(&'static str, &'static str); 3] {
    let (aspect, panscan, keep) = match mode {
        AspectMode::Auto => ("no", "0", "yes"),
        AspectMode::Wide => ("16:9", "0", "yes"),
        AspectMode::Standard => ("4:3", "0", "yes"),
        AspectMode::Scope => ("2.35:1", "0", "yes"),
        // The stream's own shape, scaled up until the slot is covered; the
        // overhang is cropped rather than letterboxed.
        AspectMode::Fill => ("no", "1", "yes"),
        AspectMode::Stretch => ("no", "0", "no"),
    };
    [
        ("video-aspect-override", aspect),
        ("panscan", panscan),
        ("keepaspect", keep),
    ]
}

/// mpv's `deinterlace` value for a mode.
pub fn deinterlace_value(mode: DeinterlaceMode) -> &'static str {
    match mode {
        DeinterlaceMode::Auto => "auto",
        DeinterlaceMode::On => "yes",
        DeinterlaceMode::Off => "no",
    }
}

/// The mode an mpv `deinterlace` reading means. Unknown values are Off,
/// which is mpv's default and what an unreadable property most likely is.
pub fn deinterlace_mode(value: &str) -> DeinterlaceMode {
    match value {
        "auto" => DeinterlaceMode::Auto,
        "yes" => DeinterlaceMode::On,
        _ => DeinterlaceMode::Off,
    }
}

/// An audio or subtitle choice as mpv's `aid`/`sid` want it. `None` is no
/// track at all, which only subtitles offer.
pub fn track_value(id: Option<i64>) -> String {
    match id {
        Some(n) if n > 0 => n.to_string(),
        _ => "no".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_radio_channel_has_no_video_and_a_cover_picture_is_none() {
        assert_eq!(has_video(LIST), Some(true));
        assert_eq!(has_video(r#"[{"id":1,"type":"audio","codec":"aac"}]"#), Some(false));
        assert_eq!(
            has_video(r#"[{"id":1,"type":"video","albumart":true},{"id":1,"type":"audio"}]"#),
            Some(false)
        );
        // Not loaded yet: nothing to say.
        assert_eq!(has_video("[]"), None);
        assert_eq!(has_video("nonsense"), None);
    }

    /// Read from libmpv 0.41 playing a Matroska file with two audio tracks
    /// and one subtitle track, trimmed to the fields that matter.
    const LIST: &str = r#"[
        {"id":1,"type":"video","src-id":1,"selected":true,"codec":"h264"},
        {"id":1,"type":"audio","src-id":2,"lang":"eng","audio-channels":6,"default":true,
         "forced":false,"visual-impaired":false,"hearing-impaired":false,"selected":true,
         "codec":"ac3","demux-channel-count":6,"demux-channels":"5.1(side)"},
        {"id":2,"type":"audio","src-id":3,"title":"Commentary","lang":"fre","audio-channels":2,
         "default":false,"selected":false,"codec":"aac","demux-channel-count":2},
        {"id":1,"type":"sub","src-id":4,"lang":"eng","default":false,"hearing-impaired":true,
         "selected":false,"codec":"subrip"}
    ]"#;

    #[test]
    fn audio_and_subtitles_are_separated_and_video_is_dropped() {
        let (audio, subs) = parse_track_list(LIST);
        assert_eq!(audio.len(), 2);
        assert_eq!(subs.len(), 1);
        assert_eq!(audio[0].id, 1);
        assert_eq!(audio[0].lang.as_deref(), Some("eng"));
        assert_eq!(audio[0].codec.as_deref(), Some("ac3"));
        assert_eq!(audio[0].channels, Some(6));
        assert!(audio[0].default && audio[0].selected);
        assert_eq!(audio[1].title.as_deref(), Some("Commentary"));
        assert!(!audio[1].selected);
        assert_eq!(subs[0].id, 1);
        assert!(subs[0].hearing_impaired);
        assert_eq!(subs[0].channels, None);
    }

    #[test]
    fn an_empty_or_unreadable_list_is_no_tracks() {
        assert_eq!(parse_track_list("[]"), (vec![], vec![]));
        assert_eq!(parse_track_list(""), (vec![], vec![]));
        assert_eq!(parse_track_list("{\"id\":1}"), (vec![], vec![]));
        // An entry without an id cannot be selected, so it is not offered.
        let (audio, _) = parse_track_list(r#"[{"type":"audio","lang":"eng"}]"#);
        assert!(audio.is_empty());
    }

    #[test]
    fn an_undetermined_language_is_no_language() {
        let (audio, _) =
            parse_track_list(r#"[{"id":1,"type":"audio","lang":"und","title":"  "}]"#);
        assert_eq!(audio[0].lang, None);
        assert_eq!(audio[0].title, None);
    }

    #[test]
    fn every_aspect_mode_sets_the_same_three_properties() {
        for mode in [
            AspectMode::Auto,
            AspectMode::Wide,
            AspectMode::Standard,
            AspectMode::Scope,
            AspectMode::Fill,
            AspectMode::Stretch,
        ] {
            let keys: Vec<_> = aspect_properties(mode).iter().map(|(k, _)| *k).collect();
            assert_eq!(keys, ["video-aspect-override", "panscan", "keepaspect"]);
        }
    }

    #[test]
    fn fill_crops_and_stretch_distorts_and_nothing_else_does_either() {
        let get = |mode, key| {
            aspect_properties(mode)
                .into_iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v)
                .unwrap()
        };
        assert_eq!(get(AspectMode::Fill, "panscan"), "1");
        assert_eq!(get(AspectMode::Stretch, "keepaspect"), "no");
        assert_eq!(get(AspectMode::Wide, "video-aspect-override"), "16:9");
        assert_eq!(get(AspectMode::Scope, "video-aspect-override"), "2.35:1");
        for mode in [AspectMode::Auto, AspectMode::Wide, AspectMode::Standard, AspectMode::Scope] {
            assert_eq!(get(mode, "panscan"), "0");
            assert_eq!(get(mode, "keepaspect"), "yes");
        }
    }

    #[test]
    fn deinterlace_round_trips_through_mpvs_words() {
        for mode in [DeinterlaceMode::Auto, DeinterlaceMode::On, DeinterlaceMode::Off] {
            assert_eq!(deinterlace_mode(deinterlace_value(mode)), mode);
        }
        assert_eq!(deinterlace_mode("something new"), DeinterlaceMode::Off);
    }

    #[test]
    fn the_menu_starts_where_the_player_does() {
        // `Player::new` sets `deinterlace=auto`; the picture menu's first
        // answer, before anything is chosen, has to say the same.
        assert_eq!(DeinterlaceMode::default(), DeinterlaceMode::Auto);
        assert_eq!(deinterlace_value(DeinterlaceMode::default()), "auto");
    }

    #[test]
    fn no_track_is_mpvs_no() {
        assert_eq!(track_value(Some(2)), "2");
        assert_eq!(track_value(None), "no");
        assert_eq!(track_value(Some(0)), "no");
    }

    #[test]
    fn the_contract_spells_the_ratios_as_people_do() {
        assert_eq!(serde_json::to_string(&AspectMode::Wide).unwrap(), "\"16:9\"");
        assert_eq!(serde_json::to_string(&AspectMode::Scope).unwrap(), "\"2.35:1\"");
        assert_eq!(serde_json::to_string(&AspectMode::Fill).unwrap(), "\"fill\"");
        assert_eq!(serde_json::to_string(&DeinterlaceMode::Auto).unwrap(), "\"auto\"");
    }
}
