//! User settings, persisted as one JSON blob in `meta`.
//!
//! Settings must never be the reason the app will not open. Every read path
//! here degrades to [`SettingsDto::default`]: a missing row, unreadable JSON,
//! a value of the wrong type, even a database that will not open all mean
//! "defaults", not "error".
//!
//! One blob rather than a row per field because the shape is owned by
//! `ipc::types` and changes there should not need a migration here.

use anyhow::Result;
use rusqlite::Connection;

use crate::catalog::db;
use crate::ipc::types::SettingsDto;

const KEY: &str = "settings";

/// Merges stored values over defaults so a blob written by an older build —
/// one missing a field added since — still loads.
///
/// `SettingsDto` is the frontend contract and carries no `#[serde(default)]`,
/// so `from_str` on a partial blob would fail outright. Filling the gaps from
/// the default before deserialising gives the same result without touching
/// the contract.
fn overlay(base: &mut serde_json::Value, stored: &serde_json::Value) {
    match (base, stored) {
        (serde_json::Value::Object(b), serde_json::Value::Object(s)) => {
            // An unknown key is a field this build has dropped, or one a
            // newer build added. Neither is worth failing over.
            for (k, v) in s {
                if let Some(slot) = b.get_mut(k) {
                    overlay(slot, v);
                }
            }
        }
        (b, s) => *b = s.clone(),
    }
}

/// The bitrate the old minutes setting was converted at (18.2 Mbps).
const LEGACY_REWIND_MB_PER_MINUTE: f64 = 60.0 * 18_200_000.0 / 8.0 / 1_000_000.0;

/// Carries a rewind saved in minutes over to megabytes.
///
/// Converted at the rate the old setting was applied at, so the budget mpv
/// holds is exactly what it was before the upgrade: someone who had chosen
/// four minutes keeps the 546 MB they actually had, rather than a new
/// default. A stored `rewindMb` always wins.
fn migrate_rewind(stored: &mut serde_json::Value) {
    let Some(map) = stored.as_object_mut() else {
        return;
    };
    if map.contains_key("rewindMb") {
        return;
    }
    if let Some(minutes) = map.get("rewindMinutes").and_then(|v| v.as_f64()) {
        if minutes.is_finite() && minutes > 0.0 {
            let mb = (minutes * LEGACY_REWIND_MB_PER_MINUTE).round() as u64;
            map.insert("rewindMb".into(), serde_json::json!(mb));
        }
    }
}

/// Parses a stored blob, or `None` if nothing usable can be made of it.
fn parse(raw: &str) -> Option<SettingsDto> {
    let mut stored: serde_json::Value = serde_json::from_str(raw).ok()?;
    migrate_rewind(&mut stored);
    let mut merged = serde_json::to_value(SettingsDto::default()).ok()?;
    overlay(&mut merged, &stored);
    serde_json::from_value(merged).ok()
}

/// Settings as stored on this connection. Defaults for anything unusable.
pub fn load(conn: &Connection) -> SettingsDto {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", [KEY], |r| {
        r.get::<_, String>(0)
    })
    .ok()
    .as_deref()
    .and_then(parse)
    .unwrap_or_default()
}

pub fn save(conn: &Connection, settings: &SettingsDto) -> Result<()> {
    let blob = serde_json::to_string(settings)?;
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![KEY, blob],
    )?;
    Ok(())
}

/// Settings from the process database. A database that will not open reports
/// defaults rather than failing — the same contract as every other read here.
pub fn load_or_default() -> SettingsDto {
    db::with_db(|c| Ok(load(c))).unwrap_or_default()
}

/// Serialises the tests that write the settings: the process shares one
/// in-memory database, and the settings are one key in it.
#[cfg(test)]
pub fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::open_in_memory;
    use crate::ipc::types::{SubtitleBackdrop, SubtitleStyleDto};

    fn write_raw(c: &Connection, raw: &str) {
        c.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![KEY, raw],
        )
        .unwrap();
    }

    #[test]
    fn an_empty_database_reads_as_defaults() {
        let c = open_in_memory().unwrap();
        let s = load(&c);
        assert_eq!(s.rewind_mb, SettingsDto::default().rewind_mb);
        assert_eq!(s.guide_hours, 3);
        assert!(s.hardware_decode);
    }

    #[test]
    fn settings_round_trip() {
        let c = open_in_memory().unwrap();
        let want = SettingsDto {
            rewind_mb: 1280,
            hardware_decode: false,
            tone_map_hdr: false,
            guide_hours: 6,
            sync_interval_hours: 24,
            language: "fr-CA".into(),
            subtitle: SubtitleStyleDto {
                font: "Inter".into(),
                size: 52,
                color: "#ffee00".into(),
                backdrop: SubtitleBackdrop::Outline,
                backdrop_opacity: 0.75,
                outline_size: 2.5,
                shadow_offset: 1.5,
            },
            check_for_updates: false,
            media_controls: true,
            time_format: "12h".into(),
            live_start: crate::ipc::types::LiveStart::Wait,
            simultaneous_streams: 2,
        };
        save(&c, &want).unwrap();

        let got = load(&c);
        assert_eq!(got.time_format, "12h");
        assert!(!got.check_for_updates);
        assert!(got.media_controls);
        assert_eq!(got.live_start, crate::ipc::types::LiveStart::Wait);
        assert_eq!(got.simultaneous_streams, 2);
        assert_eq!(got.rewind_mb, 1280);
        assert!(!got.hardware_decode);
        assert!(!got.tone_map_hdr);
        assert_eq!(got.guide_hours, 6);
        assert_eq!(got.sync_interval_hours, 24);
        assert_eq!(got.subtitle.font, "Inter");
        assert_eq!(got.subtitle.size, 52);
        assert_eq!(got.subtitle.color, "#ffee00");
        assert_eq!(got.subtitle.backdrop, SubtitleBackdrop::Outline);
        assert_eq!(got.subtitle.backdrop_opacity, 0.75);
        assert_eq!(got.subtitle.outline_size, 2.5);
        assert_eq!(got.subtitle.shadow_offset, 1.5);
    }

    #[test]
    fn a_blob_from_before_the_updater_checks_for_updates() {
        let c = open_in_memory().unwrap();
        write_raw(&c, r#"{"guideHours":5}"#);
        let got = load(&c);
        assert_eq!(got.guide_hours, 5);
        assert!(got.check_for_updates, "on unless someone turned it off");
    }

    #[test]
    fn a_blob_from_before_media_controls_leaves_them_off() {
        // Opt-in: an upgrade must not start answering the media keys.
        let c = open_in_memory().unwrap();
        write_raw(&c, r#"{"guideHours":5,"checkForUpdates":false}"#);
        let got = load(&c);
        assert!(!got.media_controls);
        assert!(!got.check_for_updates);
    }

    #[test]
    fn a_blob_from_before_the_clock_setting_follows_the_system() {
        let c = open_in_memory().unwrap();
        write_raw(&c, r#"{"guideHours":5,"mediaControls":true}"#);
        let got = load(&c);
        assert_eq!(got.time_format, "auto");
        assert!(got.media_controls);
        assert_eq!(got.guide_hours, 5);
    }

    #[test]
    fn a_blob_from_before_live_start_catches_up() {
        let c = open_in_memory().unwrap();
        write_raw(&c, r#"{"guideHours":5}"#);
        assert_eq!(load(&c).live_start, crate::ipc::types::LiveStart::CatchUp);
        write_raw(&c, r#"{"liveStart":"stay"}"#);
        assert_eq!(load(&c).live_start, crate::ipc::types::LiveStart::Stay);
        // A choice from a newer build is the default, and costs nothing else.
        write_raw(&c, r#"{"liveStart":"later","guideHours":4}"#);
        let got = load(&c);
        assert_eq!(got.live_start, crate::ipc::types::LiveStart::CatchUp);
        assert_eq!(got.guide_hours, 4);
    }

    #[test]
    fn saving_twice_replaces_rather_than_conflicts() {
        let c = open_in_memory().unwrap();
        save(
            &c,
            &SettingsDto {
                guide_hours: 4,
                ..Default::default()
            },
        )
        .unwrap();
        save(
            &c,
            &SettingsDto {
                guide_hours: 8,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(load(&c).guide_hours, 8);
        let rows: i64 = c
            .query_row("SELECT COUNT(*) FROM meta WHERE key = ?1", [KEY], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(rows, 1);
    }

    #[test]
    fn a_corrupt_row_reads_as_defaults_rather_than_failing() {
        let c = open_in_memory().unwrap();
        for raw in ["", "{", "not json at all", "[]", "null", "42"] {
            write_raw(&c, raw);
            assert_eq!(
                load(&c).guide_hours,
                SettingsDto::default().guide_hours,
                "corrupt blob {raw:?} should read as defaults"
            );
        }
    }

    #[test]
    fn a_wrongly_typed_field_reads_as_defaults() {
        let c = open_in_memory().unwrap();
        write_raw(&c, r#"{"guideHours":"six"}"#);
        assert_eq!(load(&c).guide_hours, SettingsDto::default().guide_hours);
    }

    #[test]
    fn a_blob_missing_fields_keeps_what_it_has_and_defaults_the_rest() {
        // What a build from before `syncIntervalHours` existed would have
        // written. It must not throw away the settings it does carry.
        let c = open_in_memory().unwrap();
        write_raw(&c, r#"{"rewindMb":900,"guideHours":5}"#);
        let s = load(&c);
        assert_eq!(s.rewind_mb, 900);
        assert_eq!(s.guide_hours, 5);
        assert_eq!(
            s.sync_interval_hours,
            SettingsDto::default().sync_interval_hours
        );
        assert_eq!(s.subtitle.font, SubtitleStyleDto::default().font);
    }

    #[test]
    fn a_rewind_saved_in_minutes_keeps_the_bytes_it_had() {
        // Four minutes was applied as 546 MB; the upgrade must not change
        // what mpv holds.
        let c = open_in_memory().unwrap();
        write_raw(&c, r#"{"rewindMinutes":4.0,"guideHours":5}"#);
        let s = load(&c);
        assert_eq!(s.rewind_mb, 546);
        assert_eq!(s.guide_hours, 5);
    }

    #[test]
    fn a_saved_megabyte_figure_wins_over_a_leftover_minutes_one() {
        let c = open_in_memory().unwrap();
        write_raw(&c, r#"{"rewindMinutes":4.0,"rewindMb":256}"#);
        assert_eq!(load(&c).rewind_mb, 256);
    }

    #[test]
    fn a_partial_subtitle_object_merges_field_by_field() {
        let c = open_in_memory().unwrap();
        write_raw(&c, r#"{"subtitle":{"size":64}}"#);
        let s = load(&c);
        assert_eq!(s.subtitle.size, 64);
        assert_eq!(s.subtitle.font, "sans-serif");
        assert_eq!(s.subtitle.backdrop, SubtitleBackdrop::Background);
        assert_eq!(s.subtitle.backdrop_opacity, 0.25);
    }

    #[test]
    fn an_unknown_field_is_ignored_rather_than_fatal() {
        // A newer build's setting, read by this one.
        let c = open_in_memory().unwrap();
        write_raw(&c, r#"{"guideHours":7,"warpDrive":true}"#);
        assert_eq!(load(&c).guide_hours, 7);
    }
}
