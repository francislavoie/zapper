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

/// Parses a stored blob, or `None` if nothing usable can be made of it.
fn parse(raw: &str) -> Option<SettingsDto> {
    let stored: serde_json::Value = serde_json::from_str(raw).ok()?;
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
        assert_eq!(s.rewind_minutes, SettingsDto::default().rewind_minutes);
        assert_eq!(s.guide_hours, 3);
        assert!(s.hardware_decode);
    }

    #[test]
    fn settings_round_trip() {
        let c = open_in_memory().unwrap();
        let want = SettingsDto {
            rewind_minutes: 12.5,
            hardware_decode: false,
            tone_map_hdr: false,
            guide_hours: 6,
            sync_interval_hours: 24,
            subtitle: SubtitleStyleDto {
                font: "Inter".into(),
                size: 52,
                color: "#ffee00".into(),
                backdrop: SubtitleBackdrop::Outline,
                backdrop_opacity: 0.75,
                outline_size: 2.5,
                shadow_offset: 1.5,
            },
        };
        save(&c, &want).unwrap();

        let got = load(&c);
        assert_eq!(got.rewind_minutes, 12.5);
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
        write_raw(&c, r#"{"rewindMinutes":9.0,"guideHours":5}"#);
        let s = load(&c);
        assert_eq!(s.rewind_minutes, 9.0);
        assert_eq!(s.guide_hours, 5);
        assert_eq!(
            s.sync_interval_hours,
            SettingsDto::default().sync_interval_hours
        );
        assert_eq!(s.subtitle.font, SubtitleStyleDto::default().font);
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
