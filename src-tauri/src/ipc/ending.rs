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
//! **No retrying, ever, from here or anywhere.** Reconnecting on the
//! viewer's behalf after a provider ended the stream would end the other
//! device's stream in turn, and the two would take the connection from each
//! other until somebody gave up. So this only explains; the interface offers
//! Retry, and only a person presses it. For the same reason nothing asks the
//! server again to read its status: that request would itself be a
//! connection. The status comes from the log of the attempt that failed.
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
        const NETWORK: &[&str] = &[
            "connection refused",
            "connection timed out",
            "operation timed out",
            "network is unreachable",
            "no route to host",
            "host is unreachable",
            "failed to resolve",
            "name or service not known",
            "temporary failure in name resolution",
            "could not resolve",
        ];
        if NETWORK.iter().any(|n| lower.contains(n)) {
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
    if clues.network {
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
    fn error_codes_read_as_words() {
        assert_eq!(error_text(-13), "the stream could not be opened");
        assert!(error_text(-99).contains("-99"));
    }

    #[test]
    fn the_dto_speaks_the_interfaces_names() {
        let e = Ending { kind: EndingKind::NotVideo, status: Some(509), limit: true };
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"kind":"notVideo","status":509,"limit":true}"#
        );
    }
}
