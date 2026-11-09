//! Why a stream stopped, told apart as well as mpv lets us.
//!
//! Many providers allow a limited number of simultaneous connections, and
//! when another device opens a stream on the same account they end one of the
//! others. From here that looks like nothing in particular: the HTTP response
//! simply finishes, mid-stream, and mpv plays out what it has buffered and
//! reports an ordinary end of file. Tuning in again then fails, with a `403`,
//! a `509`, a reason phrase like "Max Connections Reached", or a `200` whose
//! body is a line of text rather than video.
//!
//! None of that is an error mpv would put into words, so the evidence is
//! gathered here: how the file ended (an end of file or a failure, and
//! whether a frame had ever been shown), mpv's error code, and the warnings
//! mpv and FFmpeg log while opening — FFmpeg logs a refused request as
//! `HTTP error 509 Max Connections Reached`, which is the only place the
//! status ever appears. [`classify`] turns that into an [`Ending`], which the
//! interface words.
//!
//! **No retrying after the provider, ever.** Reconnecting on the viewer's
//! behalf after a provider ended the stream would end the other device's
//! stream in turn, and the two would take the connection from each other
//! until somebody gave up. So an ending the provider could have caused is
//! only explained; the interface offers Retry, and only a person presses it.
//! For the same reason nothing asks the server again to read its status:
//! that request would itself be a connection. The status comes from the log
//! of the attempt that failed.
//!
//! The one exception is [`EndingKind::Network`], which is only ever the
//! network's doing — see [`worth_reconnecting`] for how sure each kind of
//! evidence is, and `connections` for the check on the account's free
//! connections that comes before any reconnect.
//!
//! Pure, so every case is a unit test. Log text can carry a stream URL, and
//! with it the account, so nothing here keeps a line: only the status and
//! a handful of flags survive [`Clues::observe`].

use serde::{Deserialize, Serialize};

/// What the interface is told about a stream that stopped by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ending {
    pub kind: EndingKind,
    /// The HTTP status the server answered with, when it said one.
    pub status: Option<u16>,
    /// The server said in words that it was a connection limit ("Max
    /// Connections Reached", "too many connections"), rather than merely
    /// refusing. Lets the interface say so plainly instead of asking.
    pub limit: bool,
    /// How many of the account's connections were in use, and how many it
    /// has, when that was asked: for [`EndingKind::InUse`].
    pub slots: Option<crate::connections::Slots>,
    /// How long the placeholder clip was, in whole seconds: for
    /// [`EndingKind::OffAir`]. Defaulted when read, so an ending written
    /// before it was a field still reads.
    #[serde(default)]
    pub clip_secs: Option<u32>,
}

impl Ending {
    /// The channel is off air: see [`off_air`]. `clip` is the clip's length
    /// in seconds.
    pub fn off_air(clip: f64) -> Self {
        Self {
            kind: EndingKind::OffAir,
            status: None,
            limit: false,
            slots: None,
            clip_secs: Some(clip.max(0.0).round() as u32),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EndingKind {
    /// It was playing, and the provider ended it: most often because another
    /// device took the account's connection.
    Dropped,
    /// The server refused to open it with a status that means "not you, not
    /// now": 401, 403, 429, 509 and the like.
    Refused,
    /// The server answered, but with something that is not video — some
    /// providers' way of refusing a connection over the limit.
    NotVideo,
    /// The server says the channel is not there (404, 410).
    Unavailable,
    /// The server failed (5xx).
    ServerError,
    /// The server could not be reached, or stopped answering.
    Network,
    /// Not tried: the account's connections are all in use, by other
    /// devices, as the provider counts them. Only ever the answer to a
    /// check made before reconnecting by itself; `slots` says how many.
    InUse,
    /// It connected and never produced a picture before the deadline.
    NoVideo,
    /// Not a broadcast at all: the provider answered the live channel with
    /// a placeholder clip (minutes of black, sent at once, then closed)
    /// because the channel is not on the air. `clip_secs` says how long the
    /// clip was. See [`off_air`].
    OffAir,
    /// A catch-up programme the server refused, or answered with nothing,
    /// before showing a frame: most likely further back than its archive
    /// keeps. Only ever from [`for_catchup`].
    BeyondArchive,
    /// None of the above could be told; `error` carries mpv's words.
    Other,
}

/// What mpv said while a file was opening and playing, boiled down.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Clues {
    /// From `HTTP error 403 Forbidden` or `Server returned 403 …`.
    pub status: Option<u16>,
    /// The reason phrase spoke of a connection limit.
    pub limit: bool,
    /// The connection could not be made or timed out: the network, not the
    /// provider.
    pub network: bool,
    /// Of those, the ones that prove no connection to the provider was ever
    /// made: its name did not resolve, nothing answered, or there was no
    /// route. A provider counts streams it served, so a stream that never
    /// reached it cannot have taken anybody's connection.
    pub unreached: bool,
    /// The data stopped while the connection was still open, long before it
    /// ended: see [`Flow`]. A provider ending a stream closes it; it does not
    /// leave it hanging for the network timeout.
    pub stalled: bool,
}

impl Clues {
    /// Takes one log line. Keeps nothing of the text itself.
    pub fn observe(&mut self, text: &str) {
        let lower = text.to_ascii_lowercase();
        if let Some((status, reason)) = http_error(&lower) {
            self.status = Some(status);
            self.limit |= speaks_of_a_limit(reason);
        } else if let Some(status) = server_returned(&lower) {
            // The summary FFmpeg prints after the line above; it carries no
            // phrase of the server's, and must not undo the one that did.
            self.status.get_or_insert(status);
        }
        if speaks_of_a_limit(&lower) && lower.contains("connection") {
            self.limit = true;
        }
        // Never connected at all: these come from opening the socket. The
        // last four are Windows' words for the same.
        const UNREACHED: &[&str] = &[
            "connection refused",
            "network is unreachable",
            "network is down",
            "no route to host",
            "host is unreachable",
            "failed to resolve",
            "name or service not known",
            "temporary failure in name resolution",
            "could not resolve",
            "actively refused",
            "unreachable network",
            "unreachable host",
            "no such host is known",
        ];
        // The network, but perhaps after the provider had answered: a reset
        // or a timeout can come mid-stream, and a TLS failure from a
        // connection that was made. The last two are Windows' words.
        const NETWORK: &[&str] = &[
            "connection timed out",
            "operation timed out",
            "connection reset",
            "tls: ",
            "tls handshake",
            "ssl routines",
            "certificate verify failed",
            "forcibly closed",
            "did not properly respond",
        ];
        let numbered = socket_error(&lower);
        if UNREACHED.iter().any(|n| lower.contains(n))
            || numbered == Some(Reach::Unreached)
            || failed_to_connect(&lower)
        {
            self.network = true;
            self.unreached = true;
        } else if NETWORK.iter().any(|n| lower.contains(n)) || numbered == Some(Reach::Network) {
            self.network = true;
        }
    }
}

/// What a socket error number says about the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reach {
    /// No connection was made: see [`Clues::unreached`].
    Unreached,
    /// The network failed, perhaps mid-stream.
    Network,
}

/// FFmpeg built for Windows has no words for socket errors and logs their
/// numbers instead: `Error number -138 occurred`. The number is minus either
/// the C runtime's `errno`, in Windows' own numbering (`ETIMEDOUT` is 138
/// there, not Linux's 110), or a Winsock code (`WSAETIMEDOUT`, 10060), which
/// is what a socket's pending error comes back as.
fn socket_error(lower: &str) -> Option<Reach> {
    let rest = &lower[lower.find("error number -")? + "error number -".len()..];
    let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    match rest[..end].parse::<u32>().ok()? {
        // ECONNREFUSED, EHOSTUNREACH, ENETDOWN, ENETUNREACH; WSAENETDOWN,
        // WSAENETUNREACH, WSAECONNREFUSED, WSAEHOSTDOWN, WSAEHOSTUNREACH,
        // WSAHOST_NOT_FOUND, WSATRY_AGAIN.
        107 | 110 | 116 | 118 | 10050 | 10051 | 10061 | 10064 | 10065 | 11001 | 11002 => {
            Some(Reach::Unreached)
        }
        // ECONNABORTED, ECONNRESET, ENETRESET, ETIMEDOUT; WSAENETRESET,
        // WSAECONNABORTED, WSAECONNRESET, WSAETIMEDOUT.
        106 | 108 | 117 | 138 | 10052 | 10053 | 10054 | 10060 => Some(Reach::Network),
        _ => None,
    }
}

/// FFmpeg's `Connection to tcp://host:port failed: …`: the socket never
/// connected, whatever the reason given, and without a connection there was
/// no request, so nothing reached the provider. That includes a timeout, and
/// has to: on Windows a refused connection is never reported as refused;
/// FFmpeg waits out its timeout and says `ETIMEDOUT` (measured with
/// `ending_check`, "nobody there").
fn failed_to_connect(lower: &str) -> bool {
    lower
        .find("connection to tcp://")
        .is_some_and(|at| lower[at..].contains(" failed"))
}

/// `http error 509 max connections reached` → (509, "max connections reached").
fn http_error(lower: &str) -> Option<(u16, &str)> {
    let rest = &lower[lower.find("http error ")? + "http error ".len()..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let status = valid_status(&digits)?;
    Some((status, rest[digits.len()..].trim()))
}

/// `server returned 403 forbidden (access denied)` → 403. FFmpeg's summary
/// of a refused request; "4xx"/"5xx" for the statuses it has no name for,
/// which say nothing exact and are skipped.
fn server_returned(lower: &str) -> Option<u16> {
    let rest = &lower[lower.find("server returned ")? + "server returned ".len()..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    valid_status(&digits)
}

fn valid_status(digits: &str) -> Option<u16> {
    if digits.len() != 3 {
        return None;
    }
    digits.parse().ok().filter(|s| (100..600).contains(s))
}

fn speaks_of_a_limit(lower: &str) -> bool {
    const WORDS: &[&str] = &[
        "max connection",
        "maximum connection",
        "too many",
        "connection limit",
        "limit reached",
        "limit exceeded",
        "concurrent",
        "already in use",
    ];
    WORDS.iter().any(|w| lower.contains(w))
}

/// How the file ended, from mpv's `end-file`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    /// An ordinary end of file.
    Eof,
    /// A failure, with mpv's error code.
    Error(i32),
    /// No end at all: the first frame never came in time.
    Deadline,
}

/// mpv's `MPV_ERROR_UNKNOWN_FORMAT`: what it opened was not media.
pub const UNKNOWN_FORMAT: i32 = -17;

/// Why the stream stopped.
///
/// `shown` is whether a frame had been on screen — the difference between a
/// stream that was taken away and one that never came.
pub fn classify(how: How, shown: bool, clues: Clues) -> Ending {
    let kind = |kind| Ending {
        kind,
        status: clues.status,
        limit: clues.limit,
        slots: None,
        clip_secs: None,
    };
    if how == How::Deadline {
        return kind(EndingKind::NoVideo);
    }
    if let Some(status) = clues.status {
        return kind(match status {
            404 | 410 => EndingKind::Unavailable,
            // A server that names a limit is refusing, whatever number it
            // chose to say it with.
            _ if clues.limit => EndingKind::Refused,
            // 509 is "bandwidth limit exceeded", which several IPTV servers
            // send for a connection over the account's allowance.
            509 => EndingKind::Refused,
            500..=599 => EndingKind::ServerError,
            400..=499 => EndingKind::Refused,
            // A 2xx or 3xx logged as an error is no refusal; fall through to
            // how it ended.
            _ => return classify(how, shown, Clues { status: None, ..clues }),
        });
    }
    if clues.network || clues.stalled {
        return kind(EndingKind::Network);
    }
    if clues.limit {
        return kind(EndingKind::Refused);
    }
    match (how, shown) {
        // Playing, and then it was over. A live stream does not end, so the
        // server ended it.
        (How::Eof | How::Error(_), true) => kind(EndingKind::Dropped),
        // Opened, and what came back was not video, or nothing at all.
        (How::Error(UNKNOWN_FORMAT) | How::Eof, false) => kind(EndingKind::NotVideo),
        (How::Error(_), false) => kind(EndingKind::Other),
        (How::Deadline, _) => kind(EndingKind::NoVideo),
    }
}

/// The same ending, for a catch-up programme.
///
/// A server asked for footage older than it keeps answers 404 (or 410, or
/// 400 for a start it will not read): that is the archive, wherever the
/// request was. It may also answer 401 or 403, or with nothing — but those
/// are also how a provider refuses a connection over the account's limit
/// (a bare 403, a `200` with a line of text), and taking that for the
/// archive would hide "Watch here anyway". So they are put down to the
/// archive only for a request `near_edge` (see `player::controller::CatchUp`).
/// A refusal that names a limit (in words, or as 429 or 509) is always the
/// limit, and anything after a frame was shown is left as it was.
pub fn for_catchup(e: Ending, shown: bool, near_edge: bool) -> Ending {
    if shown {
        return e;
    }
    let beyond = match (e.kind, e.status) {
        (EndingKind::Unavailable, _) => true,
        _ if e.limit => false,
        (EndingKind::Refused, Some(400)) => true,
        (EndingKind::Refused, Some(429 | 509)) => false,
        (EndingKind::Refused, _) | (EndingKind::NotVideo, _) => near_edge,
        _ => false,
    };
    if beyond {
        Ending {
            kind: EndingKind::BeyondArchive,
            ..e
        }
    } else {
        e
    }
}

/// Whether a live channel that never showed a picture failed through a
/// fault of its own — and if so, why, in a few words for the channel list.
///
/// The list dims channels that do not load, so this has to be the channel's
/// fault and not the account's or the network's, or one bad evening would
/// dim half the catalogue:
///
/// - **Counted**: a refusal of this channel (401, 403 and the like), the
///   server saying it is not there (404, 410), an answer that is not video,
///   no picture before the deadline, and a failure mpv could not explain.
/// - **Not counted**: anything that names or implies the connection limit
///   (a phrase about connections, 429, 509) — another device has the
///   account, which says nothing about the channel; the network; a server
///   error (5xx), which is the server's bad minute, and which is also how an
///   HDHomeRun says its tuners are busy; and of course a stream that played
///   and was then taken away.
///
/// Only meaningful for an open that never showed a frame; the caller checks
/// that, since an ending does not record it. `error` is mpv's reason in
/// words, for the unexplained case.
pub fn against_channel(e: &Ending, error: Option<&str>) -> Option<String> {
    let http = |what: &str| match e.status {
        Some(s) => format!("{what} (HTTP {s})"),
        None => what.to_owned(),
    };
    match e.kind {
        EndingKind::Refused if e.limit || matches!(e.status, Some(429 | 509)) => None,
        EndingKind::Refused => Some(http("refused by the provider")),
        EndingKind::Unavailable => Some(http("not on the server")),
        EndingKind::NotVideo => Some("the server answered with something that is not video".to_owned()),
        EndingKind::NoVideo => Some("no picture arrived".to_owned()),
        // Said in two words, because the list shows it as "Off air · 18:26"
        // (src/logic/format.ts reads it back by this text).
        EndingKind::OffAir => Some(OFF_AIR_REASON.to_owned()),
        EndingKind::Other => Some(error.unwrap_or("it could not be played").to_owned()),
        // Not tried at all: the account was full. Or not the channel's
        // doing: a programme older than its archive keeps.
        EndingKind::Dropped
        | EndingKind::ServerError
        | EndingKind::Network
        | EndingKind::InUse
        | EndingKind::BeyondArchive => None,
    }
}

/// The reason a channel that was off air is written down with in its
/// record (`channel_quality.fail_reason`). The interface recognises it by
/// this text, so it is spelled exactly the same in `src/logic/format.ts`.
pub const OFF_AIR_REASON: &str = "off air";

/// How soon after the open the end of the provider's data has to be read for
/// the stream to be a placeholder clip, in seconds.
///
/// The captured case sent its whole ten-minute clip in about three seconds.
/// Twenty leaves room for a link six or seven times slower than that, and is
/// still far too soon for a live stream with ordinary timestamps to have
/// produced [`OFF_AIR_MIN_CLIP`] of video: see there.
pub const OFF_AIR_WINDOW: f64 = 20.0;

/// The least stream time a placeholder clip must have delivered by its end,
/// in seconds.
///
/// A live server can only send what it already holds — its connect burst,
/// at most about sixty seconds on the providers measured — and then what
/// happens, one second per second. So by the end of [`OFF_AIR_WINDOW`] a
/// live channel with continuous timestamps has delivered at most about
/// 60 + 20 = 80 seconds, and a takeover or a dropped connection right after
/// a long burst stays under this, with a margin. A stream whose timestamps
/// jump forward can appear to have delivered more; the other guards in
/// [`off_air`] are for that. The placeholders seen are several minutes long,
/// so none falls short.
pub const OFF_AIR_MIN_CLIP: f64 = 120.0;

/// The most stream time a placeholder clip is believed to span, in seconds.
/// Placeholders are minutes long; a span of hours within seconds of the open
/// is a timestamp jump, not a clip.
pub const OFF_AIR_MAX_CLIP: f64 = 4.0 * 3600.0;

/// The least data a placeholder clip carries per second of its span, in
/// bytes: black and silence still costs something to send (the captured
/// clip about 24 KB/s, the fixture about 11 KB/s). A span that arrived with
/// far fewer bytes than that is timestamps that jumped, not video.
pub const OFF_AIR_MIN_BYTES_PER_SEC: f64 = 2_000.0;

/// What an open showed when the end of its data was read: when, how much
/// stream time had arrived by then, and how many bytes mpv held.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Read {
    /// Seconds since the open.
    pub at: f64,
    /// Stream time received: the newest timestamp less the oldest seen.
    pub span: f64,
    /// `demuxer-cache-state`'s `total-bytes` at the end; `None` when mpv did
    /// not say.
    pub bytes: Option<i64>,
}

/// What else is known about an open whose end was read, for [`off_air`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Context<'a> {
    /// A live channel, not a catch-up programme or a movie.
    pub live: bool,
    /// mpv's `file-format`: the demuxer that read it.
    pub format: Option<&'a str>,
    /// mpv logged the network failing (see [`Clues::network`]).
    pub network: bool,
}

/// Watches an open for the end of the provider's data, so that a
/// placeholder clip can be told from a broadcast.
///
/// Some providers, when a channel is not on the air, answer its live URL
/// with a clip instead — minutes of black, sent as fast as the link allows,
/// and then the response ends. Played as a live stream it reads as minutes
/// of "data in", jumps to the clip's end, and stops as if another device had
/// taken the connection. The tell is the end: read within seconds of the
/// open, after more stream time than any live server could have sent by
/// then (see [`off_air`]).
///
/// Fed mpv's `demuxer-cache-state` once a heartbeat. Without it nothing is
/// ever read, and the ending is whatever it would have been.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Placeholder {
    /// Seconds since the open.
    t: f64,
    /// The oldest timestamp seen since the timeline began.
    first: Option<f64>,
    /// The newest packet's timestamp, last heartbeat.
    end: Option<f64>,
    /// The end has been read and judged; once per open.
    done: bool,
    /// `total-bytes` at the last reading.
    bytes: Option<i64>,
}

impl Placeholder {
    /// One heartbeat: `dt` seconds since the last, the start of mpv's cache
    /// (or the playhead, failing that), its end, both in stream time,
    /// whether the demuxer has read the end of the stream, and the bytes it
    /// holds. `Some` once, on the heartbeat the end is first read.
    pub fn observe(
        &mut self,
        dt: f64,
        start: Option<f64>,
        cache_end: Option<f64>,
        eof: bool,
        bytes: Option<i64>,
    ) -> Option<Read> {
        self.t += dt;
        if self.done {
            return None;
        }
        if bytes.is_some() {
            self.bytes = bytes;
        }
        if let Some(end) = cache_end.filter(|e| e.is_finite()) {
            // Timestamps that went back started a new timeline (a reset, a
            // splice): the jump is not stream time received.
            if self.end.is_some_and(|last| end < last - 1.0) {
                self.first = None;
            }
            self.end = Some(end);
        }
        for seen in [start, cache_end].into_iter().flatten().filter(|v| v.is_finite()) {
            // Never past the end: a start from before the reset is not this
            // timeline's.
            if self.end.is_none_or(|e| seen <= e) {
                self.first = Some(self.first.map_or(seen, |f| f.min(seen)));
            }
        }
        if !eof {
            return None;
        }
        self.done = true;
        Some(Read { at: self.t, span: self.span(), bytes: self.bytes })
    }

    /// Whether the end has been read, and judged, already.
    pub fn judged(&self) -> bool {
        self.done
    }

    /// Stream time received so far: the newest timestamp less the oldest
    /// seen, zero before any. A clip-shaped buffer is one this deep within
    /// seconds of the open.
    pub fn span(&self) -> f64 {
        match (self.first, self.end) {
            (Some(f), Some(e)) => (e - f).max(0.0),
            _ => 0.0,
        }
    }
}

/// Whether mpv's `file-format` is one a live channel arrives in: MPEG-TS,
/// or HLS. A file in a container made for files (MP4, Matroska) is an
/// episode or a film, however it was listed, and being sent fast and then
/// ending is what a file does.
pub fn live_container(format: &str) -> bool {
    format
        .split(',')
        .any(|f| matches!(f.trim(), "mpegts" | "mpegtsraw" | "hls" | "applehttp"))
}

/// Whether an open whose data ended as `read` says was a placeholder clip,
/// and if so its length in seconds.
///
/// - Only a live channel: a catch-up programme and a movie are files, sent
///   as fast as the link allows and then ended, which is exactly this shape.
///   And only in a live container ([`live_container`]): a playlist's
///   episodes are listed among its channels.
/// - The end within [`OFF_AIR_WINDOW`] of the open, after between
///   [`OFF_AIR_MIN_CLIP`] and [`OFF_AIR_MAX_CLIP`] of stream time, carried by
///   at least [`OFF_AIR_MIN_BYTES_PER_SEC`]: a span with too few bytes behind
///   it, or one of hours, is timestamps that jumped.
/// - Never when mpv logged the network failing: that ending is the network's.
pub fn off_air(read: Read, cx: Context<'_>) -> Option<f64> {
    if !cx.live || cx.network || !cx.format.is_some_and(live_container) {
        return None;
    }
    let dense = read
        .bytes
        .is_some_and(|b| b as f64 >= OFF_AIR_MIN_BYTES_PER_SEC * read.span);
    (read.at <= OFF_AIR_WINDOW
        && (OFF_AIR_MIN_CLIP..=OFF_AIR_MAX_CLIP).contains(&read.span)
        && dense)
        .then_some(read.span)
}

/// How long the data must have stopped before the stream ended for the
/// ending to count as a stall. mpv gives up on a silent connection after its
/// `network-timeout`, eight seconds; a provider that ends a stream closes it
/// while data is still flowing, so its gap is a fraction of a second.
pub const STALL_GAP: f64 = 5.0;

/// mpv's forward budget, `demuxer-max-bytes`, as `surface::mpv` sets it.
/// Here rather than read from there because this module is also the
/// recording service's, which has no player.
pub const DEMUXER_MAX_BYTES: i64 = 256 * 1024 * 1024;

/// A cache this full stops reading on purpose, which is not the network
/// stalling.
pub const FORWARD_FULL: i64 = DEMUXER_MAX_BYTES * 9 / 10;

/// When data last arrived, and when the stream's end was read: the evidence
/// that tells a stall from a stream the provider ended.
///
/// Both end the same way — mpv reads to the end of what it has and reports
/// an end of file — and the log says nothing about either. What differs is
/// the gap. A provider closes the response mid-flow, so the end is read the
/// moment the data stops. A stalled connection goes quiet and stays open
/// until mpv's `network-timeout` gives up on it, eight seconds later.
///
/// Measured against the end of the *data* (the demuxer reading its end),
/// never against `end-file`: playback carries on through whatever is
/// buffered first, which is seconds at live and minutes for a viewer who
/// rewound, and would make every ending look like a stall.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Flow {
    /// Seconds since the stream opened.
    t: f64,
    /// The newest packet's timestamp, last time it moved.
    end: Option<f64>,
    /// When it last moved.
    grew_at: Option<f64>,
    /// Whether the cache had room then, so that the quiet was not mpv
    /// choosing to stop reading.
    room: bool,
    /// When the demuxer first said it had read the end.
    eof_at: Option<f64>,
}

impl Flow {
    /// One heartbeat: `dt` seconds since the last, the end of mpv's cache in
    /// stream time, whether the demuxer has read the end of the stream, and
    /// the bytes buffered ahead.
    pub fn observe(&mut self, dt: f64, cache_end: Option<f64>, eof: bool, fw_bytes: Option<i64>) {
        self.t += dt;
        if let Some(end) = cache_end {
            // Any change counts, backwards too: a timestamp wrap is data.
            // Except one seen with the end: that is the demuxer letting go
            // of the packet it held back when the read failed, not data
            // arriving (a stall's end on Windows moves the cache's end by a
            // frame or two, measured with `ending_check`).
            let flushed = eof && self.eof_at.is_none() && self.end.is_some();
            if !flushed && self.end.is_none_or(|last| (end - last).abs() > 0.01) {
                self.end = Some(end);
                self.grew_at = Some(self.t);
                self.room = fw_bytes.is_none_or(|b| b < FORWARD_FULL);
            }
        }
        if eof && self.eof_at.is_none() {
            self.eof_at = Some(self.t);
        }
    }

    /// Whether the stream went quiet with its connection open and was then
    /// given up on, rather than being closed. Read when it has ended, so a
    /// stream whose end was never seen read is taken to end now.
    pub fn stalled(&self) -> bool {
        let Some(grew) = self.grew_at else {
            return false;
        };
        self.room && self.eof_at.unwrap_or(self.t) - grew >= STALL_GAP
    }
}

/// Whether a stream that ended this way may be reconnected without asking.
///
/// Only [`EndingKind::Network`] — and only for a live channel, where
/// reconnecting is picking up where the viewer was; a catch-up programme
/// would start again from its beginning. How sure that is, by evidence:
///
/// - Its name did not resolve, the connection was refused, there was no
///   route, or the connection was otherwise never made (a timeout while
///   connecting, which is also how Windows reports a refusal): certain. No
///   request reached the provider.
/// - A reset or a timeout on a connection that was made, a TLS failure:
///   near certain. These come from the socket before any stream was served.
/// - A connection reset *mid-play* is not here at all, because it cannot be
///   seen: FFmpeg hands mpv a reset as an ordinary end of the stream, and
///   nothing about it reaches mpv's log at any level (measured with
///   `ending_check`). It reads as [`EndingKind::Dropped`], like a takeover,
///   and is left to a person. That is the safe way to be wrong.
/// - A stall (see [`Flow`]): near certain. The data stopped with the
///   connection open and mpv gave up eight seconds later.
///
/// Never: a clean end mid-play (most often another device taking the
/// connection), any HTTP status (a refusal, a missing channel, a broken
/// server — none of them better on a second try, and a refusal at the limit
/// would take the connection back from whoever has it), or anything else.
pub fn worth_reconnecting(e: &Ending, live: bool) -> bool {
    live && e.kind == EndingKind::Network && e.status.is_none()
}

/// mpv's error codes in words, for the line under the message. mpv's own
/// rendering of these through libmpv2 is `Raw(-13)`.
pub fn error_text(code: i32) -> String {
    match code {
        -13 => "the stream could not be opened".to_owned(),
        -14 => "the audio output could not be opened".to_owned(),
        -15 => "the video output could not be opened".to_owned(),
        -16 => "the stream has nothing to play".to_owned(),
        UNKNOWN_FORMAT => "the stream is not in a format that can be played".to_owned(),
        -18 => "the stream uses something that is not supported".to_owned(),
        other => format!("playback failed (mpv error {other})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clues(lines: &[&str]) -> Clues {
        let mut c = Clues::default();
        for l in lines {
            c.observe(l);
        }
        c
    }

    #[test]
    fn the_status_and_its_phrase_are_read_from_ffmpegs_warning() {
        let c = clues(&["http: HTTP error 509 Max Connections Reached\n"]);
        assert_eq!(c.status, Some(509));
        assert!(c.limit);
        let c = clues(&["http: HTTP error 403 Forbidden"]);
        assert_eq!(c.status, Some(403));
        assert!(!c.limit);
    }

    #[test]
    fn the_summary_line_supplies_a_status_but_never_undoes_one() {
        assert_eq!(clues(&["Server returned 401 Unauthorized (authorization failed)"]).status, Some(401));
        let c = clues(&["HTTP error 509 Max Connections Reached", "Server returned 5XX Server Error reply"]);
        assert_eq!(c.status, Some(509));
        assert!(c.limit);
        assert_eq!(clues(&["Server returned 4XX Client Error, but not one of 40{0,1,3,4}"]).status, None);
    }

    /// A catch-up that never showed a frame, ended with `lines` logged.
    fn catchup(lines: &[&str], how: How, near_edge: bool) -> Ending {
        for_catchup(classify(how, false, clues(lines)), false, near_edge)
    }

    #[test]
    fn missing_footage_is_beyond_the_archive_wherever_it_was_asked_for() {
        for near in [true, false] {
            let e = catchup(&["HTTP error 404 Not Found"], How::Error(-13), near);
            assert_eq!((e.kind, e.status), (EndingKind::BeyondArchive, Some(404)), "{near}");
            let e = catchup(&["HTTP error 410 Gone"], How::Error(-13), near);
            assert_eq!(e.kind, EndingKind::BeyondArchive, "{near}");
            let e = catchup(&["Server returned 400 Bad Request"], How::Error(-13), near);
            assert_eq!(e.kind, EndingKind::BeyondArchive, "{near}");
        }
    }

    #[test]
    fn a_bare_refusal_or_an_empty_answer_is_the_archive_only_near_its_edge() {
        // Near the far end (or of an archive of unknown depth): the archive.
        let near = |lines: &[&str], how| catchup(lines, how, true).kind;
        assert_eq!(near(&["HTTP error 403 Forbidden"], How::Error(-13)), EndingKind::BeyondArchive);
        assert_eq!(near(&["HTTP error 401 Unauthorized"], How::Error(-13)), EndingKind::BeyondArchive);
        assert_eq!(near(&[], How::Eof), EndingKind::BeyondArchive);
        assert_eq!(near(&[], How::Error(UNKNOWN_FORMAT)), EndingKind::BeyondArchive);
        // Well inside it: how providers refuse over the connection limit, and
        // "Watch here anyway" is kept for it.
        let far = |lines: &[&str], how| catchup(lines, how, false).kind;
        assert_eq!(far(&["HTTP error 403 Forbidden"], How::Error(-13)), EndingKind::Refused);
        assert_eq!(far(&["HTTP error 401 Unauthorized"], How::Error(-13)), EndingKind::Refused);
        assert_eq!(far(&[], How::Eof), EndingKind::NotVideo);
        assert_eq!(far(&[], How::Error(UNKNOWN_FORMAT)), EndingKind::NotVideo);
    }

    #[test]
    fn a_catch_up_refused_for_the_limit_or_the_network_says_so() {
        for near in [true, false] {
            let e = catchup(&["HTTP error 509 Max Connections Reached"], How::Error(-13), near);
            assert_eq!((e.kind, e.limit), (EndingKind::Refused, true));
            let kind = |lines: &[&str], how| catchup(lines, how, near).kind;
            assert_eq!(kind(&["HTTP error 429 Too Many Requests"], How::Error(-13)), EndingKind::Refused);
            assert_eq!(kind(&["HTTP error 403 Max connections reached"], How::Error(-13)), EndingKind::Refused);
            assert_eq!(kind(&["HTTP error 502 Bad Gateway"], How::Error(-13)), EndingKind::ServerError);
            assert_eq!(kind(&["Connection refused"], How::Error(-13)), EndingKind::Network);
            assert_eq!(kind(&[], How::Deadline), EndingKind::NoVideo);
            // Once it had played, an ending is about the stream, not the archive.
            let played = for_catchup(classify(How::Eof, true, Clues::default()), true, near);
            assert_eq!(played.kind, EndingKind::Dropped);
        }
        // And it is never the channel's fault.
        let e = catchup(&["HTTP error 404 Not Found"], How::Error(-13), false);
        assert_eq!(against_channel(&e, None), None);
        assert!(!worth_reconnecting(&e, false));
    }

    #[test]
    fn nonsense_numbers_are_not_statuses() {
        assert_eq!(clues(&["HTTP error 4031 x"]).status, None);
        assert_eq!(clues(&["HTTP error 42 x"]).status, None);
        assert_eq!(clues(&["HTTP error 999 x"]).status, None);
    }

    #[test]
    fn network_trouble_is_recognised() {
        for line in [
            "tcp: Connection to tcp://127.0.0.1:1?timeout=8000000 failed: Connection refused",
            "tcp: Failed to resolve hostname nowhere.invalid: Name or service not known",
            "tcp: Connection to tcp://10.0.0.1:80 failed: Connection timed out",
            "Network is unreachable",
        ] {
            assert!(clues(&[line]).network, "{line}");
        }
        assert!(!clues(&["h264: decode_slice_header error"]).network);
    }

    #[test]
    fn a_line_with_a_url_leaves_nothing_of_it_behind() {
        // Clues is plain data: a status and three flags. The type is the
        // guarantee; this pins that a URL line yields no status from its
        // digits either.
        let c = clues(&["Failed to open http://host:8080/live/user/pass/509.ts."]);
        assert_eq!(c, Clues::default());
    }

    #[test]
    fn a_stream_that_ended_while_playing_was_dropped() {
        let e = classify(How::Eof, true, Clues::default());
        assert_eq!(e.kind, EndingKind::Dropped);
        assert_eq!(e.status, None);
    }

    #[test]
    fn refusals_are_told_apart_by_status() {
        for s in [401, 403, 429, 458, 509] {
            let e = classify(How::Error(-13), false, Clues { status: Some(s), ..Default::default() });
            assert_eq!(e.kind, EndingKind::Refused, "{s}");
            assert_eq!(e.status, Some(s));
        }
        let gone = classify(How::Error(-13), false, Clues { status: Some(404), ..Default::default() });
        assert_eq!(gone.kind, EndingKind::Unavailable);
        let broken = classify(How::Error(-13), false, Clues { status: Some(502), ..Default::default() });
        assert_eq!(broken.kind, EndingKind::ServerError);
    }

    #[test]
    fn a_limit_in_words_is_a_refusal_whatever_the_number() {
        let c = clues(&["HTTP error 503 Too many connections for this account"]);
        let e = classify(How::Error(-13), false, c);
        assert_eq!(e.kind, EndingKind::Refused);
        assert!(e.limit);
    }

    #[test]
    fn something_other_than_video_is_its_own_answer() {
        assert_eq!(classify(How::Error(UNKNOWN_FORMAT), false, Clues::default()).kind, EndingKind::NotVideo);
        // Connected, and the response ended before a single frame.
        assert_eq!(classify(How::Eof, false, Clues::default()).kind, EndingKind::NotVideo);
    }

    #[test]
    fn the_network_is_not_blamed_on_the_provider() {
        let c = clues(&["Connection to tcp://127.0.0.1:1 failed: Connection refused"]);
        assert_eq!(classify(How::Error(-13), false, c).kind, EndingKind::Network);
        // Mid-play as well: a timeout while playing is the connection, not a
        // takeover.
        let c = clues(&["Connection timed out"]);
        assert_eq!(classify(How::Eof, true, c).kind, EndingKind::Network);
    }

    #[test]
    fn a_refusal_is_reported_even_mid_play() {
        // A status logged while playing is a reconnect inside FFmpeg that
        // was refused; the status is the better explanation than "dropped".
        let c = Clues { status: Some(403), ..Default::default() };
        assert_eq!(classify(How::Eof, true, c).kind, EndingKind::Refused);
    }

    #[test]
    fn the_deadline_is_no_video() {
        assert_eq!(classify(How::Deadline, false, Clues::default()).kind, EndingKind::NoVideo);
    }

    #[test]
    fn an_unexplained_failure_to_open_is_other() {
        assert_eq!(classify(How::Error(-13), false, Clues::default()).kind, EndingKind::Other);
    }

    #[test]
    fn a_success_status_is_not_a_refusal() {
        let c = Clues { status: Some(200), ..Default::default() };
        assert_eq!(classify(How::Eof, true, c).kind, EndingKind::Dropped);
    }

    #[test]
    fn only_the_channels_own_failures_count_against_it() {
        let end = |kind, status, limit| Ending { kind, status, limit, slots: None, clip_secs: None };
        let counted = |e: Ending| against_channel(&e, Some("the stream could not be opened"));
        assert_eq!(
            counted(end(EndingKind::Refused, Some(403), false)).as_deref(),
            Some("refused by the provider (HTTP 403)")
        );
        assert_eq!(
            counted(end(EndingKind::Unavailable, Some(404), false)).as_deref(),
            Some("not on the server (HTTP 404)")
        );
        assert!(counted(end(EndingKind::NotVideo, None, false)).is_some());
        assert!(counted(end(EndingKind::NoVideo, None, false)).is_some());
        assert_eq!(
            counted(end(EndingKind::Other, None, false)).as_deref(),
            Some("the stream could not be opened")
        );
        // The account's limit, however it is said, is not the channel.
        assert_eq!(counted(end(EndingKind::Refused, Some(403), true)), None);
        assert_eq!(counted(end(EndingKind::Refused, Some(509), false)), None);
        assert_eq!(counted(end(EndingKind::Refused, Some(429), false)), None);
        assert_eq!(counted(end(EndingKind::Refused, None, true)), None);
        // Nor the network, a server's bad minute, or a takeover mid-play.
        assert_eq!(counted(end(EndingKind::Network, None, false)), None);
        assert_eq!(counted(end(EndingKind::ServerError, Some(503), false)), None);
        assert_eq!(counted(end(EndingKind::Dropped, None, false)), None);
    }

    #[test]
    fn what_proves_no_connection_was_made_is_told_from_what_does_not() {
        for line in [
            "tcp: Failed to resolve hostname nowhere.invalid: Name or service not known",
            "tcp: Connection to tcp://127.0.0.1:1?timeout=8000000 failed: Connection refused",
            "Network is unreachable",
            // Never connected, whatever the reason: no request was made.
            "tcp: Connection to tcp://10.0.0.1:80 failed: Connection timed out",
        ] {
            let c = clues(&[line]);
            assert!(c.network && c.unreached, "{line}");
        }
        for line in [
            "Connection timed out",
            "Connection reset by peer",
            "tls: Error in the pull function.",
            "error:0A00010B:SSL routines::wrong version number",
        ] {
            let c = clues(&[line]);
            assert!(c.network && !c.unreached, "{line}");
        }
        // A URL with "https" in it is not a TLS failure.
        assert!(!clues(&["Opening https://host/live/1.ts"]).network);
    }

    #[test]
    fn windows_socket_errors_are_read_by_number_and_by_name() {
        // A refused connection on Windows, as FFmpeg logs it there: its wait
        // for the connection runs out (ETIMEDOUT is 138 in Windows' C
        // runtime).
        let c = clues(&["tcp: Connection to tcp://127.0.0.1:59807 failed: Error number -138 occurred"]);
        assert!(c.network && c.unreached);
        for line in [
            "Error number -107 occurred",   // ECONNREFUSED
            "Error number -118 occurred",   // ENETUNREACH
            "Error number -10061 occurred", // WSAECONNREFUSED
            "Error number -10065 occurred", // WSAEHOSTUNREACH
            "No connection could be made because the target machine actively refused it.",
            "A socket operation was attempted to an unreachable host.",
            "tcp: Failed to resolve hostname nowhere.invalid: No such host is known.",
        ] {
            let c = clues(&[line]);
            assert!(c.network && c.unreached, "{line}");
        }
        for line in [
            "Error number -138 occurred",   // ETIMEDOUT, once connected
            "Error number -108 occurred",   // ECONNRESET
            "Error number -10054 occurred", // WSAECONNRESET
            "An existing connection was forcibly closed by the remote host.",
        ] {
            let c = clues(&[line]);
            assert!(c.network && !c.unreached, "{line}");
        }
        // Other numbers, and numbers that only begin like one, are not the
        // network.
        for line in ["Error number -5 occurred", "Error number -1380 occurred", "Error number -1 occurred"] {
            assert!(!clues(&[line]).network, "{line}");
        }
    }

    /// A stream's flow, a heartbeat at a time: data for `data` seconds, then
    /// quiet, then the end read at `eof`, then watched until `until`.
    fn flow(data: f64, eof: f64, until: f64, room: bool) -> Flow {
        let mut f = Flow::default();
        let mut t = 0.0;
        while t < until {
            t += 0.25;
            let end = t.min(data);
            let bytes = if room { 1_000_000 } else { FORWARD_FULL + 1 };
            f.observe(0.25, Some(end), t >= eof, Some(bytes));
        }
        f
    }

    #[test]
    fn a_stall_is_told_from_a_stream_the_provider_closed() {
        // Data until 30s, then nothing until mpv's timeout reads the end.
        assert!(flow(30.0, 38.0, 40.0, true).stalled());
        // The provider closed it at 30s; playback ran on through a minute
        // of rewound buffer before end-file. Not a stall.
        assert!(!flow(30.0, 30.25, 90.0, true).stalled());
        // A full cache stops reading on purpose; quiet then is not a stall.
        assert!(!flow(30.0, 38.0, 40.0, false).stalled());
    }

    #[test]
    fn the_packet_flushed_with_the_end_is_not_new_data() {
        // A stall as Windows reports it: quiet from 1.92 for seven seconds,
        // then the end read and the cache's end nudged forward with it.
        let mut f = Flow::default();
        for i in 0..20 {
            f.observe(0.25, Some(1.0 + 0.05 * i as f64), false, Some(1));
        }
        for _ in 0..28 {
            f.observe(0.25, Some(1.95), false, Some(0));
        }
        f.observe(0.25, Some(1.982), true, Some(0));
        assert!(f.stalled());
        // A provider closing a flowing stream still reads as closed.
        let mut f = Flow::default();
        for i in 0..20 {
            f.observe(0.25, Some(1.0 + 0.05 * i as f64), false, Some(1));
        }
        f.observe(0.25, Some(2.1), true, Some(0));
        assert!(!f.stalled());
    }

    #[test]
    fn a_stall_whose_end_was_never_seen_read_is_measured_to_now() {
        assert!(flow(30.0, f64::INFINITY, 37.0, true).stalled());
        assert!(!flow(30.0, f64::INFINITY, 32.0, true).stalled());
        assert!(!Flow::default().stalled(), "no data at all is no stall");
    }

    #[cfg(feature = "mpv")]
    #[test]
    fn the_forward_budget_is_the_players() {
        assert_eq!(DEMUXER_MAX_BYTES, crate::surface::mpv::DEMUXER_MAX_BYTES);
    }

    #[test]
    fn a_stall_is_the_network() {
        let c = Clues { stalled: true, ..Default::default() };
        assert_eq!(classify(How::Eof, true, c).kind, EndingKind::Network);
        // But a status still wins: whatever stalled, the server said no.
        let c = Clues { stalled: true, status: Some(509), ..Default::default() };
        assert_eq!(classify(How::Eof, true, c).kind, EndingKind::Refused);
    }

    #[test]
    fn only_the_network_is_reconnected_by_itself() {
        let e = |kind, status| Ending { kind, status, limit: false, slots: None, clip_secs: None };
        assert!(worth_reconnecting(&e(EndingKind::Network, None), true));
        // A catch-up programme would start again from its beginning.
        assert!(!worth_reconnecting(&e(EndingKind::Network, None), false));
        for kind in [
            EndingKind::Dropped,
            EndingKind::Refused,
            EndingKind::NotVideo,
            EndingKind::Unavailable,
            EndingKind::ServerError,
            EndingKind::NoVideo,
            EndingKind::Other,
            EndingKind::InUse,
            EndingKind::OffAir,
            EndingKind::BeyondArchive,
        ] {
            assert!(!worth_reconnecting(&e(kind, None), true), "{kind:?}");
        }
        assert!(!worth_reconnecting(&e(EndingKind::Network, Some(503)), true));
    }

    /// An open watched a heartbeat at a time: data whose newest timestamp
    /// is `end_at(t)`, the start of the cache at `start`, and the end of the
    /// stream read at `eof` seconds (never, for infinity). Returns what the
    /// watch saw when the end was read, if it was.
    fn placeholder_run(start: Option<f64>, end_at: impl Fn(f64) -> Option<f64>, eof: f64, until: f64) -> Option<Read> {
        let mut p = Placeholder::default();
        let mut t = 0.0;
        let mut seen = None;
        while t < until {
            t += 0.25;
            let end = end_at(t);
            // The fixture's rate: about 11 KB a second of stream.
            let bytes = end.map(|e| ((e - start.unwrap_or(0.0)).max(0.0) * 11_000.0) as i64);
            seen = seen.or(p.observe(0.25, start, end, t >= eof, bytes));
        }
        seen
    }

    /// A live channel in MPEG-TS, with nothing from the network.
    const TS: Context<'static> = Context { live: true, format: Some("mpegts"), network: false };

    fn ts(read: Read) -> Option<f64> {
        off_air(read, TS)
    }

    #[test]
    fn a_placeholder_clip_is_off_air() {
        // The measured case: ten minutes of stream, all of it in three
        // seconds, then the end.
        let read = placeholder_run(Some(1.4), |t| Some(1.4 + (t * 200.0).min(600.0)), 3.0, 10.0)
            .expect("the end was read");
        assert!((read.at - 3.0).abs() < 0.01, "{read:?}");
        assert!((read.span - 600.0).abs() < 0.01, "{read:?}");
        assert_eq!(ts(read), Some(600.0));
        let e = Ending::off_air(600.4);
        assert_eq!((e.kind, e.clip_secs, e.status, e.limit), (EndingKind::OffAir, Some(600), None, false));
    }

    #[test]
    fn a_clip_whose_whole_length_arrived_before_the_first_heartbeat_is_measured_from_its_start() {
        // On a fast link the whole file is in before anything is read: the
        // span is the cache's start to its end, not the first reading's.
        let read = placeholder_run(Some(0.0), |_| Some(598.0), 0.25, 1.0).unwrap();
        assert_eq!(ts(read), Some(598.0));
        // No seekable range reported: the playhead's first reading stands in.
        let read = placeholder_run(None, |_| Some(598.0), 0.25, 1.0).unwrap();
        assert_eq!(read.span, 0.0, "nothing earlier than the end was ever seen");
    }

    #[test]
    fn a_normal_burst_then_an_end_is_not_off_air() {
        // The longest connect bursts seen, about a minute, then the provider
        // ending it (a takeover) or the network dropping it seconds later.
        for (burst, eof) in [(60.0, 4.0), (30.0, 2.0), (60.0, 19.0), (20.0, 12.0)] {
            let read = placeholder_run(Some(0.0), |t| Some(burst + t), eof, eof + 1.0).unwrap();
            assert_eq!(ts(read), None, "burst {burst}s, ended at {eof}s");
        }
        // And whatever ended it is classified exactly as before.
        assert_eq!(classify(How::Eof, true, Clues::default()).kind, EndingKind::Dropped);
        let c = Clues { network: true, ..Default::default() };
        assert_eq!(classify(How::Eof, true, c).kind, EndingKind::Network);
    }

    #[test]
    fn an_end_after_the_window_is_not_off_air() {
        let read = placeholder_run(Some(0.0), |_| Some(600.0), OFF_AIR_WINDOW + 0.5, 30.0).unwrap();
        assert_eq!(ts(read), None);
        // At the window's edge it still counts.
        let read = placeholder_run(Some(0.0), |_| Some(600.0), OFF_AIR_WINDOW, 30.0).unwrap();
        assert_eq!(ts(read), Some(600.0));
    }

    #[test]
    fn a_clip_shorter_than_the_minimum_is_not_off_air() {
        let read = placeholder_run(Some(0.0), |_| Some(OFF_AIR_MIN_CLIP - 1.0), 2.0, 5.0).unwrap();
        assert_eq!(ts(read), None);
        let read = placeholder_run(Some(0.0), |_| Some(OFF_AIR_MIN_CLIP), 2.0, 5.0).unwrap();
        assert_eq!(ts(read), Some(OFF_AIR_MIN_CLIP));
    }

    #[test]
    fn catch_up_and_movies_are_never_off_air() {
        // A programme from the archive, or a film, arrives this way by
        // design: as fast as the link allows, and then it ends.
        let read = placeholder_run(Some(0.0), |_| Some(600.0), 3.0, 5.0).unwrap();
        assert_eq!(off_air(read, Context { live: false, ..TS }), None);
    }

    #[test]
    fn without_the_cache_state_nothing_is_decided() {
        // mpv would not hand back `demuxer-cache-state`: no end is ever read
        // and no span known, so the ending is whatever it was before.
        assert_eq!(placeholder_run(None, |_| None, f64::INFINITY, 30.0), None);
        // An end read with no timestamps at all says nothing either.
        let read = placeholder_run(None, |_| None, 2.0, 5.0).unwrap();
        assert_eq!(ts(read), None);
    }

    #[test]
    fn a_file_container_is_an_episode_not_a_placeholder() {
        // A playlist's episodes are listed among its channels: an MP4 or a
        // Matroska file sent fast and then ended is a file doing what files
        // do.
        let read = placeholder_run(Some(0.0), |_| Some(600.0), 2.0, 5.0).unwrap();
        assert_eq!(off_air(read, Context { format: Some("mov,mp4,m4a,3gp,3g2,mj2"), ..TS }), None);
        assert_eq!(off_air(read, Context { format: Some("matroska,webm"), ..TS }), None);
        assert_eq!(off_air(read, Context { format: None, ..TS }), None, "unknown: no verdict");
        assert_eq!(off_air(read, Context { format: Some("hls"), ..TS }), Some(600.0));
        assert!(live_container("mpegts") && !live_container("mp3"));
    }

    #[test]
    fn a_timestamp_jump_is_not_a_clip() {
        // A live stream whose timestamps jump forward by hours, then ends
        // cleanly: a span of hours, with a few seconds' bytes behind it.
        let read = Read { at: 6.0, span: 26_000.0, bytes: Some(3_000_000) };
        assert_eq!(ts(read), None, "longer than any placeholder");
        // A smaller jump: ten minutes of span carried by the bytes of a
        // few seconds of video.
        let read = Read { at: 6.0, span: 600.0, bytes: Some(600_000) };
        assert_eq!(ts(read), None, "1 KB a second of stream is not video");
        let read = Read { at: 6.0, span: 600.0, bytes: None };
        assert_eq!(ts(read), None, "no byte count: no verdict");
        let read = Read { at: 6.0, span: OFF_AIR_MAX_CLIP, bytes: Some(i64::MAX) };
        assert_eq!(ts(read), Some(OFF_AIR_MAX_CLIP));
    }

    #[test]
    fn the_network_failing_is_never_off_air() {
        let read = placeholder_run(Some(0.0), |_| Some(600.0), 2.0, 5.0).unwrap();
        assert_eq!(off_air(read, Context { network: true, ..TS }), None);
    }

    #[test]
    fn the_end_is_judged_once() {
        let mut p = Placeholder::default();
        assert!(p.observe(0.25, Some(0.0), Some(600.0), true, Some(6_600_000)).is_some());
        assert!(p.observe(0.25, Some(0.0), Some(600.0), true, Some(6_600_000)).is_none());
    }

    #[test]
    fn timestamps_that_go_back_start_the_span_again() {
        // A live stream whose timestamps reset (a splice) and which then
        // ends: the jump is not stream time received.
        let mut p = Placeholder::default();
        p.observe(0.25, Some(95_000.0), Some(95_010.0), false, None);
        p.observe(0.25, Some(95_000.0), Some(95_020.0), false, None);
        let read = p.observe(0.25, Some(0.0), Some(8.0), true, None).unwrap();
        assert_eq!(read.span, 8.0);
    }

    #[test]
    fn off_air_is_held_against_the_channel_and_never_reconnected() {
        let e = Ending::off_air(600.0);
        assert_eq!(against_channel(&e, None).as_deref(), Some(OFF_AIR_REASON));
        assert!(!worth_reconnecting(&e, true));
    }

    #[test]
    fn error_codes_read_as_words() {
        assert_eq!(error_text(-13), "the stream could not be opened");
        assert!(error_text(-99).contains("-99"));
    }

    #[test]
    fn the_dto_speaks_the_interfaces_names() {
        let e = Ending { kind: EndingKind::NotVideo, status: Some(509), limit: true, slots: None, clip_secs: None };
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"kind":"notVideo","status":509,"limit":true,"slots":null,"clipSecs":null}"#
        );
        let e = Ending {
            kind: EndingKind::InUse,
            status: None,
            limit: true,
            slots: Some(crate::connections::Slots { max: 1, active: 1 }),
            clip_secs: None,
        };
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"kind":"inUse","status":null,"limit":true,"slots":{"max":1,"active":1},"clipSecs":null}"#
        );
        assert_eq!(
            serde_json::to_string(&Ending::off_air(600.0)).unwrap(),
            r#"{"kind":"offAir","status":null,"limit":false,"slots":null,"clipSecs":600}"#
        );
        // An ending written before the clip's length was a field still reads.
        let old: Ending = serde_json::from_str(r#"{"kind":"dropped","status":null,"limit":false,"slots":null}"#).unwrap();
        assert_eq!((old.kind, old.clip_secs), (EndingKind::Dropped, None));
    }
}
