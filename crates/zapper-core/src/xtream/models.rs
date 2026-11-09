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

/// One row of a long list, which may not read: a provider's list of a
/// hundred thousand movies must not fail for one malformed row.
#[derive(Deserialize)]
#[serde(untagged)]
enum Row<T> {
    Read(T),
    Unreadable(serde::de::IgnoredAny),
}

/// The rows of a list that read, and how many did not and were left out.
#[derive(Debug)]
pub struct Rows<T> {
    pub read: Vec<T>,
    pub skipped: usize,
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Rows<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let rows = Vec::<Row<T>>::deserialize(d)?;
        let mut read = Vec::with_capacity(rows.len());
        let mut skipped = 0;
        for row in rows {
            match row {
                Row::Read(t) => read.push(t),
                Row::Unreadable(_) => skipped += 1,
            }
        }
        Ok(Self { read, skipped })
    }
}

/// One movie in `get_vod_streams`. Forgiving like [`Channel`]; a catalogue
/// holds well over a hundred thousand of these. A row without a usable id
/// or a name does not read (and is left out of the list: [`Rows`]).
#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "RawVodStream")]
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
    #[serde(default, deserialize_with = "flexible_i64")]
    stream_id: Option<i64>,
    #[serde(default)]
    name: Option<String>,
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

impl TryFrom<RawVodStream> for VodStream {
    type Error = &'static str;

    fn try_from(r: RawVodStream) -> Result<Self, Self::Error> {
        let stream_id = r
            .stream_id
            .filter(|&id| id > 0)
            .ok_or("a movie without an id")?;
        let name = r.name.unwrap_or_default();
        if name.trim().is_empty() {
            return Err("a movie without a name");
        }
        // `0` is a server's "none", and must not hide the other spelling.
        let tmdb = |t: Option<i64>| t.filter(|&t| t > 0);
        Ok(Self {
            stream_id,
            name,
            category_id: r.category_id,
            stream_icon: r.stream_icon,
            rating: r.rating,
            rating_5based: r.rating_5based,
            added: r.added,
            container_extension: r.container_extension,
            year: r.year,
            tmdb: tmdb(r.tmdb).or(tmdb(r.tmdb_id)),
        })
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
        hms_secs(self.duration.as_deref()?)
    }
}

/// `HH:MM:SS` as seconds; zero and anything else is `None`.
fn hms_secs(s: &str) -> Option<i64> {
    let parts: Option<Vec<i64>> = s
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
            tmdb_id: r.tmdb_id.filter(|&t| t > 0),
            video: r.video.map(Into::into),
            audio: r.audio.map(Into::into),
        })
    }
}

/// One show in `get_series`. Forgiving like [`VodStream`]; a row without a
/// usable id or a name does not read (and is left out: [`Rows`]).
#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "RawSeriesStream")]
pub struct SeriesStream {
    pub series_id: i64,
    pub name: String,
    pub category_id: Option<i64>,
    pub cover: Option<String>,
    /// The ten-point rating, as the server sent it.
    pub rating: Option<f64>,
    pub rating_5based: Option<f64>,
    /// Unix seconds the provider last changed it.
    pub last_modified: Option<i64>,
    /// The first four digits of the release date, else `year`.
    pub year: Option<i64>,
    pub tmdb: Option<i64>,
}

/// The wire shape of [`SeriesStream`]: both spellings of the release date
/// and of the TMDB id are read and merged, never aliased (an alias turns a
/// server that sends both into a duplicate-field error).
#[derive(Deserialize)]
struct RawSeriesStream {
    #[serde(default, deserialize_with = "flexible_i64")]
    series_id: Option<i64>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default, deserialize_with = "flexible_i64")]
    category_id: Option<i64>,
    #[serde(default, deserialize_with = "nonempty_string")]
    cover: Option<String>,
    #[serde(default, deserialize_with = "flexible_f64")]
    rating: Option<f64>,
    #[serde(default, deserialize_with = "flexible_f64")]
    rating_5based: Option<f64>,
    #[serde(default, deserialize_with = "flexible_i64")]
    last_modified: Option<i64>,
    #[serde(default, deserialize_with = "nonempty_string", rename = "releaseDate")]
    release_date_camel: Option<String>,
    #[serde(default, deserialize_with = "nonempty_string")]
    release_date: Option<String>,
    #[serde(default, deserialize_with = "flexible_i64")]
    year: Option<i64>,
    #[serde(default, deserialize_with = "flexible_i64")]
    tmdb: Option<i64>,
    #[serde(default, deserialize_with = "flexible_i64")]
    tmdb_id: Option<i64>,
}

/// The year a release date (`2021-03-01`) starts with.
fn year_of(date: &str) -> Option<i64> {
    let d = date.trim().get(..4)?;
    d.bytes()
        .all(|b| b.is_ascii_digit())
        .then(|| d.parse().ok())
        .flatten()
}

impl TryFrom<RawSeriesStream> for SeriesStream {
    type Error = &'static str;

    fn try_from(r: RawSeriesStream) -> Result<Self, Self::Error> {
        let series_id = r
            .series_id
            .filter(|&id| id > 0)
            .ok_or("a show without an id")?;
        let name = r.name.unwrap_or_default();
        if name.trim().is_empty() {
            return Err("a show without a name");
        }
        let tmdb = |t: Option<i64>| t.filter(|&t| t > 0);
        let year = r
            .release_date_camel
            .as_deref()
            .and_then(year_of)
            .or_else(|| r.release_date.as_deref().and_then(year_of))
            .or(r.year);
        Ok(Self {
            series_id,
            name,
            category_id: r.category_id,
            cover: r.cover,
            rating: r.rating,
            rating_5based: r.rating_5based,
            last_modified: r.last_modified,
            year,
            tmdb: tmdb(r.tmdb).or(tmdb(r.tmdb_id)),
        })
    }
}

impl SeriesStream {
    /// Divider rows, as in the live list: nothing to open.
    pub fn is_separator(&self) -> bool {
        let t = self.name.trim();
        t.starts_with("##") && t.ends_with("##")
    }

    /// A rating out of ten, by the movies' rule ([`VodStream::rating10`]).
    pub fn rating10(&self) -> Option<f64> {
        let ok = |r: f64| (r > 0.0 && r <= 10.0).then_some(r);
        self.rating
            .and_then(ok)
            .or_else(|| self.rating_5based.and_then(|r| ok(r * 2.0)))
    }
}

/// `get_series_info`: the show, its seasons and every episode.
#[derive(Debug, Clone, Default)]
pub struct SeriesInfo {
    pub info: SeriesInfoBody,
    pub seasons: Vec<SeasonInfo>,
    pub episodes: Vec<EpisodeInfo>,
    /// Episode rows that did not read and were left out.
    pub skipped: usize,
}

#[derive(Debug, Clone, Default)]
pub struct SeriesInfoBody {
    pub name: Option<String>,
    pub cover: Option<String>,
    pub plot: Option<String>,
    pub cast: Option<String>,
    pub director: Option<String>,
    pub genre: Option<String>,
    pub release_date: Option<String>,
    /// A string or an array on the wire; always a list here.
    pub backdrop_path: Vec<String>,
    pub rating: Option<f64>,
    pub tmdb: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeasonInfo {
    #[serde(deserialize_with = "required_i32")]
    pub season_number: i32,
    #[serde(default, deserialize_with = "nonempty_string")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "nonempty_string")]
    pub cover: Option<String>,
    #[serde(default, deserialize_with = "nonempty_string")]
    pub air_date: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EpisodeInfo {
    pub id: i64,
    pub season: i32,
    pub episode_num: i32,
    pub title: Option<String>,
    pub container_extension: Option<String>,
    /// Unix seconds the provider added it.
    pub added: Option<i64>,
    pub info: EpisodeInfoBody,
}

#[derive(Debug, Clone, Default)]
pub struct EpisodeInfoBody {
    /// The episode's still.
    pub movie_image: Option<String>,
    pub plot: Option<String>,
    /// `releasedate`, `release_date` or `air_date`, whichever the server used.
    pub air_date: Option<String>,
    pub duration_secs: Option<i64>,
    /// `"00:52:00"`.
    pub duration: Option<String>,
    pub rating: Option<f64>,
    pub video: Option<StreamTags>,
    pub audio: Option<StreamTags>,
}

impl EpisodeInfoBody {
    /// The runtime in seconds: the numeric field, else `duration` parsed as
    /// `HH:MM:SS`.
    pub fn runtime_secs(&self) -> Option<i64> {
        if let Some(s) = self.duration_secs.filter(|s| *s > 0) {
            return Some(s);
        }
        hms_secs(self.duration.as_deref()?)
    }
}

fn required_i32<'de, D: Deserializer<'de>>(d: D) -> Result<i32, D::Error> {
    flexible_i64(d)?
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| serde::de::Error::custom("missing required integer"))
}

fn flexible_i32<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i32>, D::Error> {
    Ok(flexible_i64(d)?.and_then(|n| i32::try_from(n).ok()))
}

#[derive(Deserialize)]
struct RawSeriesInfoBody {
    #[serde(default, deserialize_with = "nonempty_string")]
    name: Option<String>,
    #[serde(default, deserialize_with = "nonempty_string")]
    cover: Option<String>,
    #[serde(default, deserialize_with = "nonempty_string")]
    plot: Option<String>,
    #[serde(default, deserialize_with = "nonempty_string")]
    cast: Option<String>,
    #[serde(default, deserialize_with = "nonempty_string")]
    director: Option<String>,
    #[serde(default, deserialize_with = "nonempty_string")]
    genre: Option<String>,
    #[serde(default, deserialize_with = "nonempty_string", rename = "releaseDate")]
    release_date_camel: Option<String>,
    #[serde(default, deserialize_with = "nonempty_string")]
    release_date: Option<String>,
    #[serde(default, deserialize_with = "nonempty_string")]
    releasedate: Option<String>,
    #[serde(default, deserialize_with = "string_list")]
    backdrop_path: Vec<String>,
    #[serde(default, deserialize_with = "flexible_f64")]
    rating: Option<f64>,
    #[serde(default, deserialize_with = "flexible_i64")]
    tmdb: Option<i64>,
    #[serde(default, deserialize_with = "flexible_i64")]
    tmdb_id: Option<i64>,
}

impl<'de> Deserialize<'de> for SeriesInfoBody {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let r = RawSeriesInfoBody::deserialize(d)?;
        let tmdb = |t: Option<i64>| t.filter(|&t| t > 0);
        Ok(Self {
            name: r.name,
            cover: r.cover,
            plot: r.plot,
            cast: r.cast,
            director: r.director,
            genre: r.genre,
            release_date: r
                .release_date_camel
                .or(r.release_date)
                .or(r.releasedate),
            backdrop_path: r.backdrop_path,
            rating: r.rating,
            tmdb: tmdb(r.tmdb).or(tmdb(r.tmdb_id)),
        })
    }
}

#[derive(Deserialize)]
struct RawEpisodeInfoBody {
    #[serde(default, deserialize_with = "nonempty_string")]
    movie_image: Option<String>,
    #[serde(default, deserialize_with = "nonempty_string")]
    plot: Option<String>,
    #[serde(default, deserialize_with = "nonempty_string")]
    releasedate: Option<String>,
    #[serde(default, deserialize_with = "nonempty_string")]
    release_date: Option<String>,
    #[serde(default, deserialize_with = "nonempty_string")]
    air_date: Option<String>,
    #[serde(default, deserialize_with = "flexible_i64")]
    duration_secs: Option<i64>,
    #[serde(default, deserialize_with = "nonempty_string")]
    duration: Option<String>,
    #[serde(default, deserialize_with = "flexible_f64")]
    rating: Option<f64>,
    #[serde(default, deserialize_with = "lenient")]
    video: Option<RawStreamTags>,
    #[serde(default, deserialize_with = "lenient")]
    audio: Option<RawStreamTags>,
}

impl<'de> Deserialize<'de> for EpisodeInfoBody {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let r = RawEpisodeInfoBody::deserialize(d)?;
        Ok(Self {
            movie_image: r.movie_image,
            plot: r.plot,
            air_date: r.releasedate.or(r.release_date).or(r.air_date),
            duration_secs: r.duration_secs,
            duration: r.duration,
            rating: r.rating,
            video: r.video.map(Into::into),
            audio: r.audio.map(Into::into),
        })
    }
}

#[derive(Deserialize)]
struct RawEpisode {
    #[serde(default, deserialize_with = "flexible_i64")]
    id: Option<i64>,
    #[serde(default, deserialize_with = "flexible_i32")]
    season: Option<i32>,
    #[serde(default, deserialize_with = "flexible_i32")]
    episode_num: Option<i32>,
    #[serde(default, deserialize_with = "nonempty_string")]
    title: Option<String>,
    #[serde(default, deserialize_with = "nonempty_string")]
    container_extension: Option<String>,
    #[serde(default, deserialize_with = "flexible_i64")]
    added: Option<i64>,
    #[serde(default, deserialize_with = "lenient")]
    info: Option<EpisodeInfoBody>,
}

impl RawEpisode {
    /// `listed_under` is the season its key or position put it in.
    fn read(row: serde_json::Value, listed_under: i32) -> Option<EpisodeInfo> {
        let r: RawEpisode = serde_json::from_value(row).ok()?;
        Some(EpisodeInfo {
            id: r.id.filter(|&id| id > 0)?,
            season: r.season.unwrap_or(listed_under),
            episode_num: r.episode_num.unwrap_or(0),
            title: r.title,
            container_extension: r.container_extension,
            added: r.added,
            info: r.info.unwrap_or_default(),
        })
    }
}

/// The episodes in any of the three shapes servers send: an object of
/// arrays keyed by season (`{"1":[…]}`), an array of arrays (by position,
/// the first being season 1), or one flat array. An episode's season is its
/// own `season` field, else where it was listed (season 1 in a flat array).
/// Rows that do not read are counted.
fn read_episodes(v: serde_json::Value) -> (Vec<EpisodeInfo>, usize) {
    use serde_json::Value;
    let mut out = Vec::new();
    let mut skipped = 0;
    let mut take = |rows: Value, season: i32| match rows {
        Value::Array(rows) => {
            for row in rows {
                match RawEpisode::read(row, season) {
                    Some(e) => out.push(e),
                    None => skipped += 1,
                }
            }
        }
        // A season that is not a list holds no episodes we can name.
        Value::Null => {}
        _ => skipped += 1,
    };
    match v {
        Value::Object(map) => {
            // Keys are season numbers; a key that is not one is left to the
            // episodes' own `season` fields.
            for (key, rows) in map {
                take(rows, key.trim().parse().unwrap_or(1));
            }
        }
        Value::Array(items) if items.iter().any(Value::is_array) => {
            for (i, rows) in items.into_iter().enumerate() {
                take(rows, i as i32 + 1);
            }
        }
        rows @ Value::Array(_) => take(rows, 1),
        _ => {}
    }
    (out, skipped)
}

impl<'de> Deserialize<'de> for SeriesInfo {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            /// `[]` when the server has nothing, as in [`VodInfo`].
            #[serde(default, deserialize_with = "lenient")]
            info: Option<SeriesInfoBody>,
            #[serde(default)]
            seasons: serde_json::Value,
            #[serde(default)]
            episodes: serde_json::Value,
        }
        let r = Raw::deserialize(d)?;
        let seasons = match r.seasons {
            serde_json::Value::Array(rows) => rows
                .into_iter()
                .filter_map(|v| serde_json::from_value(v).ok())
                .collect(),
            _ => Vec::new(),
        };
        let (mut episodes, skipped) = read_episodes(r.episodes);
        // Object keys arrive in no useful order.
        episodes.sort_by_key(|e| (e.season, e.episode_num));
        Ok(Self {
            info: r.info.unwrap_or_default(),
            seasons,
            episodes,
            skipped,
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
    fn series_rows_read_strings_numbers_and_both_spellings() {
        let s: SeriesStream = serde_json::from_str(
            r#"{"series_id":"12","name":"A Show","category_id":"3",
            "rating":"","rating_5based":"4","last_modified":"1767225600","releaseDate":"2021-03-01","tmdb_id":"9"}"#,
        )
        .unwrap();
        assert_eq!(
            (s.series_id, s.category_id, s.last_modified, s.year, s.tmdb),
            (12, Some(3), Some(1_767_225_600), Some(2021), Some(9))
        );
        assert_eq!(s.rating10(), Some(8.0));
        // Both spellings, `0` as none, the year field as the last resort.
        let s: SeriesStream = serde_json::from_str(
            r#"{"series_id":1,"name":"B","tmdb":"0","tmdb_id":4,"release_date":"1999-12-31","releaseDate":"","year":"2005"}"#,
        )
        .unwrap();
        assert_eq!((s.year, s.tmdb), (Some(1999), Some(4)));
        let s: SeriesStream =
            serde_json::from_str(r#"{"series_id":1,"name":"B","year":"2005"}"#).unwrap();
        assert_eq!(s.year, Some(2005));
        assert!(serde_json::from_str::<SeriesStream>(r#"{"series_id":0,"name":"B"}"#).is_err());
        assert!(serde_json::from_str::<SeriesStream>(r#"{"series_id":3,"name":" "}"#).is_err());
        let d: SeriesStream =
            serde_json::from_str(r###"{"series_id":3,"name":"## ROW ##"}"###).unwrap();
        assert!(d.is_separator());
    }

    fn episode_keys(info: &SeriesInfo) -> Vec<(i32, i32)> {
        info.episodes.iter().map(|e| (e.season, e.episode_num)).collect()
    }

    #[test]
    fn every_episode_shape_reads() {
        let ep = |id: u32, n: &str| format!(r#"{{"id":"{id}","episode_num":{n},"title":"T{id}"}}"#);
        let (a, b, c) = (ep(1, "1"), ep(2, r#""2""#), ep(3, "1"));
        let shapes = [
            format!(r#"{{"1":[{a},{b}],"2":[{c}]}}"#),
            format!(r#"[[{a},{b}],[{c}]]"#),
            format!(
                r#"[{},{},{}]"#,
                a.replace('{', r#"{"season":1,"#),
                b.replace('{', r#"{"season":"1","#),
                c.replace('{', r#"{"season":2,"#)
            ),
        ];
        for shape in shapes {
            let info: SeriesInfo =
                serde_json::from_str(&format!(r#"{{"episodes":{shape}}}"#)).unwrap();
            assert_eq!(episode_keys(&info), [(1, 1), (1, 2), (2, 1)], "{shape}");
            assert_eq!(info.skipped, 0, "{shape}");
        }
    }

    #[test]
    fn an_episodes_own_season_beats_where_it_was_listed() {
        let info: SeriesInfo = serde_json::from_str(
            r#"{"episodes":{"2":[{"id":1,"season":3,"episode_num":1}]}}"#,
        )
        .unwrap();
        assert_eq!(episode_keys(&info), [(3, 1)]);
    }

    #[test]
    fn episodes_come_out_in_season_order_not_key_order() {
        let info: SeriesInfo = serde_json::from_str(
            r#"{"episodes":{"10":[{"id":1,"episode_num":1}],"2":[{"id":2,"episode_num":1}]}}"#,
        )
        .unwrap();
        assert_eq!(episode_keys(&info), [(2, 1), (10, 1)]);
    }

    #[test]
    fn an_episode_with_no_season_to_go_by_is_in_season_one() {
        for body in [
            r#"{"episodes":[{"id":1,"episode_num":1}]}"#,
            r#"{"episodes":{"extras":[{"id":1,"episode_num":1}]}}"#,
        ] {
            let info: SeriesInfo = serde_json::from_str(body).unwrap();
            assert_eq!(episode_keys(&info), [(1, 1)], "{body}");
        }
    }

    #[test]
    fn an_unreadable_episode_is_left_out() {
        let info: SeriesInfo = serde_json::from_str(
            r#"{"episodes":{"1":[{"id":1,"episode_num":1},{"episode_num":2},{"id":3,"episode_num":3}]}}"#,
        )
        .unwrap();
        assert_eq!(info.episodes.len(), 2);
        assert_eq!(info.skipped, 1);
    }

    #[test]
    fn an_empty_info_and_no_seasons_still_give_the_episodes() {
        let info: SeriesInfo = serde_json::from_str(
            r#"{"info":[],"seasons":[],"episodes":{"1":[{"id":"5","episode_num":1}]}}"#,
        )
        .unwrap();
        assert!(info.info.name.is_none() && info.info.backdrop_path.is_empty());
        assert!(info.seasons.is_empty());
        assert_eq!(info.episodes.len(), 1);
        assert_eq!((info.episodes[0].id, info.episodes[0].season), (5, 1));
    }

    #[test]
    fn a_show_reads_its_body_and_seasons() {
        let info: SeriesInfo = serde_json::from_str(
            r#"{"info":{"name":"A Show","cover":"http://x/c.jpg","releaseDate":"2021-03-01",
                "backdrop_path":["http://x/b.jpg"],"rating":"7.5","tmdb":"0","tmdb_id":"9"},
                "seasons":[{"season_number":"1","name":"Season 1","air_date":"2021-03-01"},{"name":"no number"}],
                "episodes":[]}"#,
        )
        .unwrap();
        assert_eq!(info.info.release_date.as_deref(), Some("2021-03-01"));
        assert_eq!((info.info.tmdb, info.info.rating), (Some(9), Some(7.5)));
        assert_eq!(info.info.backdrop_path, ["http://x/b.jpg"]);
        assert_eq!(info.seasons.len(), 1);
        assert_eq!(info.seasons[0].season_number, 1);
    }

    #[test]
    fn an_episode_reads_its_still_plot_and_runtime() {
        let info: SeriesInfo = serde_json::from_str(
            r#"{"episodes":{"1":[{"id":"7","episode_num":"1","container_extension":"mkv","added":"1700000000",
                "info":{"movie_image":"http://x/s.jpg","plot":"It happens.","air_date":"2021-03-02",
                        "duration":"00:52:00","duration_secs":0,"rating":"8.1",
                        "video":{"codec_name":"h264"},"audio":{"codec_name":"aac","tags":{"language":"eng"}}}}]}}"#,
        )
        .unwrap();
        let e = &info.episodes[0];
        assert_eq!(e.container_extension.as_deref(), Some("mkv"));
        assert_eq!(e.added, Some(1_700_000_000));
        assert_eq!(e.info.movie_image.as_deref(), Some("http://x/s.jpg"));
        assert_eq!(e.info.plot.as_deref(), Some("It happens."));
        assert_eq!(e.info.air_date.as_deref(), Some("2021-03-02"));
        assert_eq!(e.info.runtime_secs(), Some(3120));
        assert_eq!(e.info.rating, Some(8.1));
        assert_eq!(e.info.audio.as_ref().unwrap().language.as_deref(), Some("eng"));
    }

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
        let zero: VodStream =
            serde_json::from_str(r#"{"stream_id":1,"name":"x","tmdb":"0","tmdb_id":"8"}"#).unwrap();
        assert_eq!(zero.tmdb, Some(8), "0 is none");
        let none: VodStream =
            serde_json::from_str(r#"{"stream_id":1,"name":"x","tmdb":0}"#).unwrap();
        assert_eq!(none.tmdb, None);
    }

    #[test]
    fn a_malformed_movie_row_is_left_out_and_the_others_read() {
        let rows: Rows<VodStream> = serde_json::from_str(
            r#"[{"stream_id":1,"name":"First"},
                {"stream_id":2,"name":null},
                {"stream_id":"","name":"No id"},
                {"stream_id":0,"name":"Zero id"},
                {"name":"Missing id"},
                {"stream_id":3,"name":"  "},
                {"stream_id":4,"name":7},
                "not a row",
                {"stream_id":"5","name":"Last","rating":"oops"}]"#,
        )
        .unwrap();
        let ids: Vec<_> = rows.read.iter().map(|v| v.stream_id).collect();
        assert_eq!(ids, [1, 5]);
        assert_eq!(rows.skipped, 7);
        let cats: Rows<Category> = serde_json::from_str(
            r#"[{"category_id":"1","category_name":"A"},{"category_id":null},{"category_id":"2","category_name":"B"}]"#,
        )
        .unwrap();
        assert_eq!((cats.read.len(), cats.skipped), (2, 1));
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
