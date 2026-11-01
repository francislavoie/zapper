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
        // Never connected at all: these come from opening the socket.
        const UNREACHED: &[&str] = &[
            "connection refused",
            "network is unreachable",
            "no route to host",
            "host is unreachable",
            "failed to resolve",
            "name or service not known",
            "temporary failure in name resolution",
            "could not resolve",
        ];
        // The network, but perhaps after the provider had answered: a reset
        // or a timeout can come mid-stream, and a TLS failure from a
        // connection that was made.
        const NETWORK: &[&str] = &[
            "connection timed out",
            "operation timed out",
            "connection reset",
            "tls: ",
            "tls handshake",
            "ssl routines",
            "certificate verify failed",
        ];
        if UNREACHED.iter().any(|n| lower.contains(n)) {
            self.network = true;
            self.unreached = true;
        } else if NETWORK.iter().any(|n| lower.contains(n)) {
            self.network = true;
        }
    }
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
        EndingKind::Other => Some(error.unwrap_or("it could not be played").to_owned()),
        // Not tried at all: the account was full.
        EndingKind::Dropped | EndingKind::ServerError | EndingKind::Network | EndingKind::InUse => None,
    }
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
            if self.end.is_none_or(|last| (end - last).abs() > 0.01) {
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
/// - Its name did not resolve, the connection was refused, or there was no
///   route: certain. No request reached the provider.
/// - A timeout while connecting, a reset while connecting, a TLS failure:
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
        let end = |kind, status, limit| Ending { kind, status, limit, slots: None };
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
        ] {
            let c = clues(&[line]);
            assert!(c.network && c.unreached, "{line}");
        }
        for line in [
            "tcp: Connection to tcp://10.0.0.1:80 failed: Connection timed out",
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
    fn a_stall_whose_end_was_never_seen_read_is_measured_to_now() {
        assert!(flow(30.0, f64::INFINITY, 37.0, true).stalled());
        assert!(!flow(30.0, f64::INFINITY, 32.0, true).stalled());
        assert!(!Flow::default().stalled(), "no data at all is no stall");
    }

    #[cfg(feature = "gui")]
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
        let e = |kind, status| Ending { kind, status, limit: false, slots: None };
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
        ] {
            assert!(!worth_reconnecting(&e(kind, None), true), "{kind:?}");
        }
        assert!(!worth_reconnecting(&e(EndingKind::Network, Some(503)), true));
    }

    #[test]
    fn error_codes_read_as_words() {
        assert_eq!(error_text(-13), "the stream could not be opened");
        assert!(error_text(-99).contains("-99"));
    }

    #[test]
    fn the_dto_speaks_the_interfaces_names() {
        let e = Ending { kind: EndingKind::NotVideo, status: Some(509), limit: true, slots: None };
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"kind":"notVideo","status":509,"limit":true,"slots":null}"#
        );
        let e = Ending {
            kind: EndingKind::InUse,
            status: None,
            limit: true,
            slots: Some(crate::connections::Slots { max: 1, active: 1 }),
        };
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"kind":"inUse","status":null,"limit":true,"slots":{"max":1,"active":1}}"#
        );
    }
}
