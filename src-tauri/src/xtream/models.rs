//! Wire types for the Xtream `player_api.php` protocol.
//!
//! Xtream servers are inconsistent: the same field arrives as a string in one
//! payload and a number in the next, and optional fields are sometimes `""`
//! rather than absent. Every type here is deliberately forgiving, because a
//! single strict field takes down the entire catalogue load.

use serde::{Deserialize, Deserializer};

/// Accepts `42`, `"42"`, `null`, and `""`.
fn flexible_i64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Int(i64),
        Float(f64),
        Str(String),
        Null,
    }
    Ok(match Option::<Raw>::deserialize(d)? {
        None | Some(Raw::Null) => None,
        Some(Raw::Int(i)) => Some(i),
        Some(Raw::Float(f)) => Some(f as i64),
        Some(Raw::Str(s)) => {
            let t = s.trim();
            if t.is_empty() {
                None
            } else {
                t.parse::<i64>().ok()
            }
        }
    })
}

fn required_i64<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    flexible_i64(d)?.ok_or_else(|| serde::de::Error::custom("missing required integer"))
}

/// Accepts `1`, `0`, `true`, `false`, `"1"`, `"0"`, and absence.
fn flexible_bool<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Bool(bool),
        Int(i64),
        Str(String),
        Null,
    }
    Ok(match Option::<Raw>::deserialize(d)? {
        None | Some(Raw::Null) => false,
        Some(Raw::Bool(b)) => b,
        Some(Raw::Int(i)) => i != 0,
        Some(Raw::Str(s)) => matches!(s.trim(), "1" | "true" | "yes"),
    })
}

/// Empty strings become `None`; the server uses `""` for "no value".
fn nonempty_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(Option::<String>::deserialize(d)?
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty()))
}

#[derive(Debug, Clone, Deserialize)]
pub struct Channel {
    #[serde(deserialize_with = "required_i64")]
    pub stream_id: i64,
    pub name: String,
    #[serde(default, deserialize_with = "flexible_i64")]
    pub category_id: Option<i64>,
    #[serde(default, deserialize_with = "flexible_i64")]
    pub num: Option<i64>,
    #[serde(default, deserialize_with = "nonempty_string")]
    pub stream_icon: Option<String>,
    #[serde(default, deserialize_with = "nonempty_string")]
    pub epg_channel_id: Option<String>,
    #[serde(default, deserialize_with = "flexible_bool")]
    pub tv_archive: bool,
    /// Kept raw because `deserialize_with` cannot cleanly narrow mixed input
    /// to `u32`; read it through [`Channel::tv_archive_duration`].
    #[serde(
        rename = "tv_archive_duration",
        default,
        deserialize_with = "flexible_i64"
    )]
    pub tv_archive_duration_raw: Option<i64>,
}

impl Channel {
    pub fn tv_archive_duration(&self) -> Option<u32> {
        self.tv_archive_duration_raw
            .and_then(|v| u32::try_from(v).ok())
    }

    /// The server injects `###### NAME ######` rows as visual dividers. They
    /// carry no video and must never be treated as playable.
    pub fn is_separator(&self) -> bool {
        let t = self.name.trim();
        t.starts_with("##") && t.ends_with("##")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Category {
    pub category_id: String,
    pub category_name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UserInfo {
    #[serde(default, deserialize_with = "flexible_bool")]
    pub auth: bool,
    #[serde(default)]
    pub status: String,
    #[serde(default, deserialize_with = "flexible_i64")]
    pub max_connections: Option<i64>,
    #[serde(default, deserialize_with = "flexible_i64")]
    pub active_cons: Option<i64>,
    /// When the account expires, Unix seconds; `null` for one that does not.
    #[serde(default, deserialize_with = "flexible_i64")]
    pub exp_date: Option<i64>,
}

/// The server's half of `player_api.php`'s answer: its clock.
///
/// A server reads a catch-up request's start (`YYYY-MM-DD:HH-MM`) as wall
/// time in its own time zone, which need not be the viewer's — so the
/// offset is worked out from what it says its clock reads.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ServerInfo {
    /// The server's wall clock, `2026-11-07 21:17:17`, in its own zone.
    #[serde(default, deserialize_with = "nonempty_string")]
    pub time_now: Option<String>,
    /// The same moment in Unix seconds.
    #[serde(default, deserialize_with = "flexible_i64")]
    pub timestamp_now: Option<i64>,
}

impl ServerInfo {
    /// Seconds east of UTC the server's clock reads; see [`panel_utc_offset`].
    pub fn utc_offset(&self) -> Option<i64> {
        panel_utc_offset(self.time_now.as_deref()?, self.timestamp_now?)
    }
}

/// The time zones there are run in quarter hours (UTC+5:45, UTC+8:45), so
/// an offset is rounded to one: the server's two readings of its clock can
/// straddle a second, and its wall clock can be a little off its own Unix
/// time.
const OFFSET_STEP: i64 = 15 * 60;

/// Seconds east of UTC, from a server's wall clock (`time_now`, read as if
/// it were UTC) and the same moment in Unix seconds (`timestamp_now`),
/// rounded to the nearest quarter hour. `None` for a reading that is not a
/// time, or an offset no time zone has (beyond UTC−12 and UTC+14).
pub fn panel_utc_offset(time_now: &str, timestamp_now: i64) -> Option<i64> {
    let wall = naive_unix(time_now)?;
    let raw = wall.checked_sub(timestamp_now)?;
    let offset = (raw as f64 / OFFSET_STEP as f64).round() as i64 * OFFSET_STEP;
    (-12 * 3600..=14 * 3600).contains(&offset).then_some(offset)
}

/// `2026-11-07 21:17:17` (or with a `T`, or without seconds) as Unix
/// seconds, as if the wall clock were UTC.
fn naive_unix(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date, time) = s.split_once([' ', 'T'])?;
    let mut d = date.split('-').map(|p| p.trim().parse::<i64>().ok());
    let (y, mo, day) = (d.next()??, d.next()??, d.next()??);
    if d.next().is_some() {
        return None;
    }
    let mut t = time.trim().split(':').map(|p| p.trim().parse::<i64>().ok());
    let (h, mi) = (t.next()??, t.next()??);
    let sec = t.next().unwrap_or(Some(0))?;
    if t.next().is_some()
        || !(1..=12).contains(&mo)
        || !(1..=31).contains(&day)
        || !(0..24).contains(&h)
        || !(0..60).contains(&mi)
        || !(0..=60).contains(&sec)
    {
        return None;
    }
    let days = crate::localtime::days_from_civil(y, mo as u32, day as u32);
    Some(days * 86_400 + h * 3600 + mi * 60 + sec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_id_survives_beyond_i32() {
        let json = r#"{"stream_id": 4294967295, "name": "Big", "category_id": "7"}"#;
        let c: Channel = serde_json::from_str(json).unwrap();
        assert_eq!(c.stream_id, 4_294_967_295);
    }

    #[test]
    fn user_info_carries_connections_and_expiry_as_strings_or_numbers() {
        let u: UserInfo = serde_json::from_str(
            r#"{"auth":1,"status":"Active","max_connections":"1","active_cons":"1",
                "exp_date":"1893456000","is_trial":"0"}"#,
        )
        .unwrap();
        assert_eq!((u.max_connections, u.active_cons, u.exp_date), (Some(1), Some(1), Some(1_893_456_000)));
        let u: UserInfo =
            serde_json::from_str(r#"{"auth":1,"max_connections":2,"active_cons":0,"exp_date":null}"#).unwrap();
        assert_eq!((u.max_connections, u.active_cons, u.exp_date), (Some(2), Some(0), None));
    }

    /// A moment as a server reported it: 20:17:17 UTC on 2026-11-07.
    const NOW: i64 = 1_794_082_637;

    #[test]
    fn the_panel_offset_is_its_wall_clock_less_its_unix_time() {
        // As measured: a server an hour east of UTC.
        assert_eq!(panel_utc_offset("2026-11-07 21:17:17", NOW), Some(3600));
        // Five hours west, the day before in UTC terms or not.
        assert_eq!(panel_utc_offset("2026-11-07 15:17:17", NOW), Some(-5 * 3600));
        // Half and quarter hours, across midnight.
        assert_eq!(panel_utc_offset("2026-11-08 01:47:17", NOW), Some(5 * 3600 + 1800));
        assert_eq!(panel_utc_offset("2026-11-08 06:02:17", NOW), Some(9 * 3600 + 2700));
        assert_eq!(panel_utc_offset("2026-11-07 20:17:17", NOW), Some(0));
        // `T` between the two, and no seconds.
        assert_eq!(panel_utc_offset("2026-11-07T21:17:17", NOW), Some(3600));
        assert_eq!(panel_utc_offset("2026-11-07 21:17", NOW), Some(3600));
    }

    #[test]
    fn a_clock_a_little_off_rounds_to_its_zone() {
        // A minute fast or slow, or a reading that straddled a second.
        assert_eq!(panel_utc_offset("2026-11-07 21:18:17", NOW), Some(3600));
        assert_eq!(panel_utc_offset("2026-11-07 21:16:17", NOW), Some(3600));
        assert_eq!(panel_utc_offset("2026-11-07 21:17:18", NOW), Some(3600));
        assert_eq!(panel_utc_offset("2026-11-07 15:16:20", NOW), Some(-5 * 3600));
        assert_eq!(panel_utc_offset("2026-11-08 01:48:40", NOW), Some(5 * 3600 + 1800));
    }

    #[test]
    fn a_reading_that_is_not_a_time_is_no_offset() {
        for bad in ["", "now", "2026-11-07", "21:17:17", "2026-13-07 21:17:17",
                    "2026-11-07 25:17:17", "2026-11-07 21:17:17:01", "2026/11/07 21:17:17"] {
            assert_eq!(panel_utc_offset(bad, NOW), None, "{bad:?}");
        }
        // An offset no time zone has: a clock that is simply wrong.
        assert_eq!(panel_utc_offset("2026-11-08 12:17:17", NOW), None);
        assert_eq!(panel_utc_offset("2026-11-07 21:17:17", 0), None);
    }

    #[test]
    fn server_info_reads_its_clock_or_says_nothing() {
        let s: ServerInfo = serde_json::from_str(
            r#"{"url":"x","timezone":"Europe/Somewhere","timestamp_now":1794082637,
                "time_now":"2026-11-07 21:17:17"}"#,
        )
        .unwrap();
        assert_eq!(s.utc_offset(), Some(3600));
        let s: ServerInfo =
            serde_json::from_str(r#"{"timestamp_now":"1794082637","time_now":"2026-11-07 15:17:17"}"#).unwrap();
        assert_eq!(s.utc_offset(), Some(-5 * 3600));
        for partial in [r#"{}"#, r#"{"time_now":"2026-11-07 21:17:17"}"#, r#"{"timestamp_now":1794082637}"#,
                        r#"{"time_now":"","timestamp_now":""}"#, r#"{"time_now":null,"timestamp_now":"soon"}"#] {
            let s: ServerInfo = serde_json::from_str(partial).unwrap();
            assert_eq!(s.utc_offset(), None, "{partial}");
        }
    }

    #[test]
    fn numeric_fields_accept_strings() {
        // Servers return "123" and 123 interchangeably, sometimes in one
        // payload.
        let json = r#"{"stream_id": "42", "name": "S", "category_id": 9, "num": "7"}"#;
        let c: Channel = serde_json::from_str(json).unwrap();
        assert_eq!(c.stream_id, 42);
        assert_eq!(c.category_id, Some(9));
        assert_eq!(c.num, Some(7));
    }

    #[test]
    fn archive_flag_accepts_int_and_bool() {
        let a: Channel = serde_json::from_str(
            r#"{"stream_id":1,"name":"a","tv_archive":1,"tv_archive_duration":"3"}"#,
        )
        .unwrap();
        assert!(a.tv_archive);
        assert_eq!(a.tv_archive_duration(), Some(3));
        let b: Channel =
            serde_json::from_str(r#"{"stream_id":2,"name":"b","tv_archive":0}"#).unwrap();
        assert!(!b.tv_archive);
    }

    #[test]
    fn missing_optional_fields_are_none_not_errors() {
        let c: Channel = serde_json::from_str(r#"{"stream_id":5,"name":"bare"}"#).unwrap();
        assert_eq!(c.category_id, None);
        assert_eq!(c.epg_channel_id, None);
        assert!(!c.tv_archive);
    }

    #[test]
    fn empty_string_epg_id_is_none() {
        // Servers commonly send "" rather than omitting the field.
        let c: Channel =
            serde_json::from_str(r#"{"stream_id":5,"name":"x","epg_channel_id":""}"#).unwrap();
        assert_eq!(c.epg_channel_id, None);
    }

    #[test]
    fn separator_rows_are_detected() {
        let s: Channel =
            // Not a raw string: `"######` would close any `r#…"` delimiter.
            serde_json::from_str("{\"stream_id\":1,\"name\":\"###### DOCUMENTARY ######\"}")
                .unwrap();
        assert!(s.is_separator());
        let n: Channel = serde_json::from_str(r#"{"stream_id":2,"name":"CA: PRISM 1"}"#).unwrap();
        assert!(!n.is_separator());
    }
}
