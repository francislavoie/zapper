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

/// Accepts `7.1`, `"7.1"`, `null`, and `""`; anything unparsable is `None`.
fn flexible_f64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Num(f64),
        Str(String),
        Null,
    }
    Ok(match Option::<Raw>::deserialize(d)? {
        None | Some(Raw::Null) => None,
        Some(Raw::Num(f)) => Some(f).filter(|f| f.is_finite()),
        Some(Raw::Str(s)) => s.trim().parse::<f64>().ok().filter(|f| f.is_finite()),
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

/// One movie in `get_vod_streams`. Forgiving like [`Channel`]; a catalogue
/// holds well over a hundred thousand of these.
#[derive(Debug, Clone, Deserialize)]
#[serde(from = "RawVodStream")]
pub struct VodStream {
    pub stream_id: i64,
    pub name: String,
    pub category_id: Option<i64>,
    pub stream_icon: Option<String>,
    /// The ten-point rating, as the server sent it.
    pub rating: Option<f64>,
    pub rating_5based: Option<f64>,
    /// Unix seconds the provider added it.
    pub added: Option<i64>,
    pub container_extension: Option<String>,
    pub year: Option<i64>,
    pub tmdb: Option<i64>,
}

/// The wire shape of [`VodStream`]. Servers disagree on `tmdb` against
/// `tmdb_id`, and some send both; an alias would make that a duplicate-field
/// error, so both are read and merged.
#[derive(Deserialize)]
struct RawVodStream {
    #[serde(deserialize_with = "required_i64")]
    stream_id: i64,
    name: String,
    #[serde(default, deserialize_with = "flexible_i64")]
    category_id: Option<i64>,
    #[serde(default, deserialize_with = "nonempty_string")]
    stream_icon: Option<String>,
    #[serde(default, deserialize_with = "flexible_f64")]
    rating: Option<f64>,
    #[serde(default, deserialize_with = "flexible_f64")]
    rating_5based: Option<f64>,
    #[serde(default, deserialize_with = "flexible_i64")]
    added: Option<i64>,
    #[serde(default, deserialize_with = "nonempty_string")]
    container_extension: Option<String>,
    #[serde(default, deserialize_with = "flexible_i64")]
    year: Option<i64>,
    #[serde(default, deserialize_with = "flexible_i64")]
    tmdb: Option<i64>,
    #[serde(default, deserialize_with = "flexible_i64")]
    tmdb_id: Option<i64>,
}

impl From<RawVodStream> for VodStream {
    fn from(r: RawVodStream) -> Self {
        Self {
            stream_id: r.stream_id,
            name: r.name,
            category_id: r.category_id,
            stream_icon: r.stream_icon,
            rating: r.rating,
            rating_5based: r.rating_5based,
            added: r.added,
            container_extension: r.container_extension,
            year: r.year,
            tmdb: r.tmdb.or(r.tmdb_id),
        }
    }
}

impl VodStream {
    /// Divider rows, as in the live list: not playable.
    pub fn is_separator(&self) -> bool {
        let t = self.name.trim();
        t.starts_with("##") && t.ends_with("##")
    }

    /// A rating out of ten: the ten-point one, else the five-point one
    /// doubled. Servers send `0` for "unrated", so zero and anything outside
    /// 0 to 10 count as no rating.
    pub fn rating10(&self) -> Option<f64> {
        let ok = |r: f64| (r > 0.0 && r <= 10.0).then_some(r);
        self.rating
            .and_then(ok)
            .or_else(|| self.rating_5based.and_then(|r| ok(r * 2.0)))
    }
}

/// `get_vod_info`. `movie_data` says how the file is packaged.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct VodInfo {
    /// Servers send `[]` rather than `{}` when they have nothing for a
    /// title, so an array reads as no details instead of failing the call.
    #[serde(default, deserialize_with = "info_body")]
    pub info: VodInfoBody,
    #[serde(default, deserialize_with = "lenient")]
    pub movie_data: Option<VodMovieData>,
}

#[derive(Debug, Clone, Default)]
pub struct VodInfoBody {
    pub movie_image: Option<String>,
    /// A string or an array on the wire; always a list here.
    pub backdrop_path: Vec<String>,
    pub plot: Option<String>,
    pub description: Option<String>,
    pub genre: Option<String>,
    pub director: Option<String>,
    pub cast: Option<String>,
    pub actors: Option<String>,
    pub releasedate: Option<String>,
    pub duration_secs: Option<i64>,
    /// `"01:49:00"`.
    pub duration: Option<String>,
    pub rating: Option<f64>,
    pub tmdb_id: Option<i64>,
    pub video: Option<StreamTags>,
    pub audio: Option<StreamTags>,
}

#[derive(Debug, Clone, Default)]
pub struct StreamTags {
    pub codec_name: Option<String>,
    /// From `tags.language`.
    pub language: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct VodMovieData {
    #[serde(default, deserialize_with = "nonempty_string")]
    pub container_extension: Option<String>,
    #[serde(default, deserialize_with = "flexible_i64")]
    pub added: Option<i64>,
}

impl VodInfoBody {
    /// The runtime in seconds: the numeric field, else `duration` parsed as
    /// `HH:MM:SS`.
    pub fn runtime_secs(&self) -> Option<i64> {
        if let Some(s) = self.duration_secs.filter(|s| *s > 0) {
            return Some(s);
        }
        let parts: Option<Vec<i64>> = self
            .duration
            .as_deref()?
            .trim()
            .split(':')
            .map(|p| p.trim().parse::<i64>().ok().filter(|n| *n >= 0))
            .collect();
        let parts = parts?;
        if parts.len() != 3 {
            return None;
        }
        Some(parts[0] * 3600 + parts[1] * 60 + parts[2]).filter(|s| *s > 0)
    }
}

/// Only a JSON object counts: a derived struct would also accept `[]` as an
/// empty sequence of fields, which is exactly the "nothing" being filtered out.
fn lenient<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let v = serde_json::Value::deserialize(d)?;
    Ok(if v.is_object() {
        serde_json::from_value(v).ok()
    } else {
        None
    })
}

fn info_body<'de, D: Deserializer<'de>>(d: D) -> Result<VodInfoBody, D::Error> {
    Ok(lenient::<_, VodInfoBody>(d)?.unwrap_or_default())
}

/// A string, an array of strings, or nothing; empty entries are dropped.
fn string_list<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        One(String),
        Many(Vec<serde_json::Value>),
        Other(serde::de::IgnoredAny),
    }
    let all = match Option::<Raw>::deserialize(d)? {
        Some(Raw::One(s)) => vec![s],
        Some(Raw::Many(v)) => v
            .into_iter()
            .filter_map(|x| x.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    };
    Ok(all
        .into_iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect())
}

#[derive(Deserialize)]
struct RawVodInfoBody {
    #[serde(default, deserialize_with = "nonempty_string")]
    movie_image: Option<String>,
    #[serde(default, deserialize_with = "string_list")]
    backdrop_path: Vec<String>,
    #[serde(default, deserialize_with = "nonempty_string")]
    plot: Option<String>,
    #[serde(default, deserialize_with = "nonempty_string")]
    description: Option<String>,
    #[serde(default, deserialize_with = "nonempty_string")]
    genre: Option<String>,
    #[serde(default, deserialize_with = "nonempty_string")]
    director: Option<String>,
    #[serde(default, deserialize_with = "nonempty_string")]
    cast: Option<String>,
    #[serde(default, deserialize_with = "nonempty_string")]
    actors: Option<String>,
    #[serde(default, deserialize_with = "nonempty_string")]
    releasedate: Option<String>,
    // Some servers spell it with an underscore, and some send both.
    #[serde(default, deserialize_with = "nonempty_string")]
    release_date: Option<String>,
    #[serde(default, deserialize_with = "flexible_i64")]
    duration_secs: Option<i64>,
    #[serde(default, deserialize_with = "nonempty_string")]
    duration: Option<String>,
    #[serde(default, deserialize_with = "flexible_f64")]
    rating: Option<f64>,
    #[serde(default, deserialize_with = "flexible_i64")]
    tmdb_id: Option<i64>,
    #[serde(default, deserialize_with = "lenient")]
    video: Option<RawStreamTags>,
    #[serde(default, deserialize_with = "lenient")]
    audio: Option<RawStreamTags>,
}

#[derive(Deserialize)]
struct RawStreamTags {
    #[serde(default, deserialize_with = "nonempty_string")]
    codec_name: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    tags: Option<RawLanguage>,
}

#[derive(Deserialize)]
struct RawLanguage {
    #[serde(default, deserialize_with = "nonempty_string")]
    language: Option<String>,
}

impl From<RawStreamTags> for StreamTags {
    fn from(r: RawStreamTags) -> Self {
        Self {
            codec_name: r.codec_name,
            language: r.tags.and_then(|t| t.language),
        }
    }
}

impl<'de> Deserialize<'de> for VodInfoBody {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let r = RawVodInfoBody::deserialize(d)?;
        Ok(Self {
            movie_image: r.movie_image,
            backdrop_path: r.backdrop_path,
            plot: r.plot,
            description: r.description,
            genre: r.genre,
            director: r.director,
            cast: r.cast,
            actors: r.actors,
            releasedate: r.releasedate.or(r.release_date),
            duration_secs: r.duration_secs,
            duration: r.duration,
            rating: r.rating,
            tmdb_id: r.tmdb_id,
            video: r.video.map(Into::into),
            audio: r.audio.map(Into::into),
        })
    }
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
        assert_eq!(
            (u.max_connections, u.active_cons, u.exp_date),
            (Some(1), Some(1), Some(1_893_456_000))
        );
        let u: UserInfo = serde_json::from_str(
            r#"{"auth":1,"max_connections":2,"active_cons":0,"exp_date":null}"#,
        )
        .unwrap();
        assert_eq!(
            (u.max_connections, u.active_cons, u.exp_date),
            (Some(2), Some(0), None)
        );
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

    #[test]
    fn vod_fields_arrive_as_strings_or_numbers() {
        let v: VodStream = serde_json::from_str(
            r#"{"stream_id":"9","name":"A Film (2021)","category_id":"4",
            "rating":"7.1","added":"1767225600","container_extension":"mkv","tmdb_id":"55"}"#,
        )
        .unwrap();
        assert_eq!(
            (v.stream_id, v.category_id, v.added, v.tmdb),
            (9, Some(4), Some(1_767_225_600), Some(55))
        );
        assert_eq!(v.rating10(), Some(7.1));
    }

    #[test]
    fn both_tmdb_spellings_together_are_not_an_error() {
        let v: VodStream =
            serde_json::from_str(r#"{"stream_id":1,"name":"x","tmdb":"","tmdb_id":"8"}"#).unwrap();
        assert_eq!(v.tmdb, Some(8));
    }

    #[test]
    fn the_five_star_rating_is_doubled_when_the_ten_point_one_is_missing() {
        let v: VodStream =
            serde_json::from_str(r#"{"stream_id":1,"name":"x","rating":"","rating_5based":3.5}"#)
                .unwrap();
        assert_eq!(v.rating10(), Some(7.0));
        let z: VodStream =
            serde_json::from_str(r#"{"stream_id":1,"name":"x","rating":"0"}"#).unwrap();
        assert_eq!(z.rating10(), None);
        let big: VodStream =
            serde_json::from_str(r#"{"stream_id":1,"name":"x","rating":"87"}"#).unwrap();
        assert_eq!(big.rating10(), None);
    }

    #[test]
    fn an_empty_info_array_is_no_details_not_an_error() {
        let i: VodInfo = serde_json::from_str(
            r#"{"info":[],"movie_data":{"stream_id":1,"container_extension":"mp4"}}"#,
        )
        .unwrap();
        assert!(i.info.plot.is_none() && i.info.backdrop_path.is_empty());
        assert_eq!(
            i.movie_data.unwrap().container_extension.as_deref(),
            Some("mp4")
        );
    }

    #[test]
    fn details_read_backdrops_runtime_and_codecs() {
        let i: VodInfo = serde_json::from_str(
            r#"{"info":{"backdrop_path":"http://img/b.jpg","duration":"01:49:00",
            "video":{"codec_name":"h264"},"audio":{"codec_name":"ac3","tags":{"language":"eng"}}}}"#,
        )
        .unwrap();
        assert_eq!(i.info.backdrop_path, vec!["http://img/b.jpg"]);
        assert_eq!(i.info.runtime_secs(), Some(6540));
        assert_eq!(i.info.audio.unwrap().language.as_deref(), Some("eng"));
    }

    #[test]
    fn backdrops_may_be_a_list_and_dates_either_spelling() {
        let i: VodInfo = serde_json::from_str(
            r#"{"info":{"backdrop_path":["a","","b"],"release_date":"2021-05-04",
            "releasedate":"","duration_secs":"90","video":[],"rating":"6.5"}}"#,
        )
        .unwrap();
        assert_eq!(i.info.backdrop_path, vec!["a", "b"]);
        assert_eq!(i.info.releasedate.as_deref(), Some("2021-05-04"));
        assert_eq!(i.info.runtime_secs(), Some(90));
        assert!(i.info.video.is_none());
        assert_eq!(i.info.rating, Some(6.5));
    }
}
