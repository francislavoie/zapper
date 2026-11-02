//! Opening a playlist or a guide, wherever it is.
//!
//! Either may be a URL or a file on this machine, and either may be
//! gzipped. Whether it is gzipped is decided by looking at the first two
//! bytes (`1f 8b`), never by the name or the `Content-Type`: providers serve
//! `guide.xml` gzipped, `epg.xml.gz` plain, and `application/octet-stream`
//! for both, and the magic number is the only thing that does not lie.
//!
//! **Every HTTP error drops its URL** (`reqwest::Error::without_url`) before
//! it leaves this module. Playlist URLs routinely carry the account, and not
//! always anywhere [`super::redact`] can recognise — an Xtream export puts the
//! password in the path. The status code survives, which is what the sync's
//! refusal handling reads.

use anyhow::{Context, Result};
use std::io::{self, BufRead, BufReader, Read};
use std::path::PathBuf;
use std::time::Duration;

/// How long a download may go without a byte before it is given up on.
///
/// A server that stops sending without closing the connection would
/// otherwise hold a sync pass forever: TCP keeps an idle connection open as
/// long as nobody gives up on it. A minute is far longer than any pause a
/// working server takes mid-body, and short enough that the next pass is
/// not far off.
pub const STALL: Duration = Duration::from_secs(60);

/// How long a server may take to start answering. Longer than [`STALL`]:
/// some panels build the whole guide before they send its first byte.
pub const FIRST_ANSWER: Duration = Duration::from_secs(120);

/// Sent when the account does not name an agent. The same VLC string the
/// Xtream client and mpv use: providers that filter agents at all let VLC
/// through, and one agent everywhere is one less thing to debug.
pub const DEFAULT_USER_AGENT: &str = "VLC/3.0.20 LibVLC/3.0.20";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    Http(String),
    File(PathBuf),
}

/// `http(s)://…` is fetched, `file://…` and anything else is a path. A
/// leading `~/` is the home directory, because that is what people type.
pub fn locate(s: &str) -> Location {
    let t = s.trim();
    let lower = t.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Location::Http(t.to_owned());
    }
    let path = file_url_path(t);
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        // HOME everywhere it is set; Windows keeps it in USERPROFILE.
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            return Location::File(PathBuf::from(home).join(rest));
        }
    }
    Location::File(PathBuf::from(path))
}

/// The path part of a `file://` URL, or the text unchanged.
///
/// `file:///C:/tv.m3u` is a Windows path with a slash in front of the drive
/// letter, which Windows reads as "the root of the current drive, then a
/// folder called C:". The slash goes.
fn file_url_path(t: &str) -> &str {
    let Some(rest) = t.strip_prefix("file://") else {
        return t;
    };
    let b = rest.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        &rest[1..]
    } else {
        rest
    }
}

#[cfg(test)]
mod locate_tests {
    use super::*;

    #[test]
    fn file_urls_keep_unix_paths_and_drop_the_slash_before_a_drive() {
        assert_eq!(file_url_path("file:///home/me/tv.m3u"), "/home/me/tv.m3u");
        assert_eq!(file_url_path("file:///C:/tv/list.m3u"), "C:/tv/list.m3u");
        assert_eq!(file_url_path("/plain/path.m3u"), "/plain/path.m3u");
    }
}

pub fn http_client(user_agent: Option<&str>) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(user_agent.unwrap_or(DEFAULT_USER_AGENT))
        .redirect(reqwest::redirect::Policy::limited(5))
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .context("building http client")
}

/// An opened location: a response still streaming, or a file.
pub enum Opened {
    Http(reqwest::Response),
    File(std::fs::File),
}

/// Opens `location` for reading. `what` names it in the error ("the
/// playlist", "the guide"), since the URL itself may not be shown.
pub async fn open(http: &reqwest::Client, location: &str, what: &str) -> Result<Opened> {
    match locate(location) {
        Location::Http(url) => {
            let resp = answer(http.get(&url).send(), what)
                .await?
                .map_err(reqwest::Error::without_url)
                .with_context(|| format!("fetching {what}"))?
                .error_for_status()
                .map_err(reqwest::Error::without_url)
                .with_context(|| format!("the server refused {what}"))?;
            Ok(Opened::Http(resp))
        }
        Location::File(path) => {
            let f = tokio::task::spawn_blocking({
                let path = path.clone();
                move || std::fs::File::open(path)
            })
            .await
            .context("joining a file open")?
            .with_context(|| format!("opening {what} at {}", path.display()))?;
            Ok(Opened::File(f))
        }
    }
}

/// Waits at most [`FIRST_ANSWER`] for a request's response to begin.
pub async fn answer<T>(send: impl std::future::Future<Output = T>, what: &str) -> Result<T> {
    tokio::time::timeout(FIRST_ANSWER, send).await.map_err(|_| {
        anyhow::anyhow!(
            "the server did not start sending {what} within {} s",
            FIRST_ANSWER.as_secs()
        )
    })
}

/// Reads the next chunk of a response, dropping the URL from any error.
/// Owned bytes, because every caller hands them to another thread anyway.
/// A server that sends nothing for [`STALL`] is an error.
pub async fn next_chunk(resp: &mut reqwest::Response, what: &str) -> Result<Option<Vec<u8>>> {
    next_chunk_within(resp, what, STALL).await
}

/// [`next_chunk`], giving up after `stall` without a byte.
pub async fn next_chunk_within(
    resp: &mut reqwest::Response,
    what: &str,
    stall: Duration,
) -> Result<Option<Vec<u8>>> {
    let chunk = tokio::time::timeout(stall, resp.chunk())
        .await
        .map_err(|_| stalled(what, stall))?;
    Ok(chunk
        .map_err(reqwest::Error::without_url)
        .with_context(|| format!("reading {what}"))?
        .map(|b| b.to_vec()))
}

/// The error a download that stopped arriving fails with.
pub fn stalled(what: &str, stall: Duration) -> anyhow::Error {
    anyhow::anyhow!(
        "the server stopped sending {what}: nothing arrived for {} s",
        stall.as_secs()
    )
}

/// Wraps `src` in a gzip decoder if it starts with the gzip magic number,
/// and passes it through otherwise.
///
/// Multi-member, because `cat a.gz b.gz` is a valid gzip file and some guide
/// aggregators produce exactly that.
pub fn decompressing<R: Read + Send + 'static>(mut src: R) -> io::Result<Box<dyn BufRead + Send>> {
    // Two bytes, read in a loop: a reader may legitimately hand back one.
    let mut magic = [0u8; 2];
    let mut have = 0;
    while have < 2 {
        let n = src.read(&mut magic[have..])?;
        if n == 0 {
            break;
        }
        have += n;
    }
    let head = io::Cursor::new(magic[..have].to_vec());
    let whole = head.chain(src);
    if have == 2 && magic == [0x1f, 0x8b] {
        Ok(Box::new(BufReader::new(flate2::read::MultiGzDecoder::new(
            whole,
        ))))
    } else {
        Ok(Box::new(BufReader::new(whole)))
    }
}

/// Whether the first bytes of a "playlist" are actually a stream.
///
/// Somebody pasting a single channel's URL into the playlist field is
/// asking to watch that channel, so it becomes a one-entry playlist rather
/// than an error. MPEG-TS opens with the sync byte `0x47`; anything served
/// as `video/*` or `audio/*` is a stream whatever it looks like.
pub fn looks_like_stream(first_bytes: &[u8], content_type: Option<&str>) -> bool {
    let ct = content_type.unwrap_or("").to_ascii_lowercase();
    if ct.starts_with("video/") || ct.starts_with("audio/") {
        return true;
    }
    first_bytes.first() == Some(&0x47)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn gz(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn read_all(r: Box<dyn BufRead + Send>) -> String {
        let mut s = String::new();
        let mut r = r;
        r.read_to_string(&mut s).unwrap();
        s
    }

    #[test]
    fn gzip_is_recognised_by_its_magic_not_its_name() {
        let body = b"<tv><programme/></tv>";
        assert_eq!(
            read_all(decompressing(io::Cursor::new(gz(body))).unwrap()),
            "<tv><programme/></tv>"
        );
        assert_eq!(
            read_all(decompressing(io::Cursor::new(body.to_vec())).unwrap()),
            "<tv><programme/></tv>"
        );
    }

    #[test]
    fn concatenated_gzip_members_are_all_read() {
        let mut both = gz(b"<tv>");
        both.extend(gz(b"</tv>"));
        assert_eq!(
            read_all(decompressing(io::Cursor::new(both)).unwrap()),
            "<tv></tv>"
        );
    }

    /// A reader that hands back one byte at a time, as a chunked response
    /// can at a boundary.
    struct Dribble(io::Cursor<Vec<u8>>);
    impl Read for Dribble {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            let n = out.len().min(1);
            self.0.read(&mut out[..n])
        }
    }

    #[test]
    fn the_magic_is_found_even_when_it_arrives_a_byte_at_a_time() {
        let r = decompressing(Dribble(io::Cursor::new(gz(b"hello")))).unwrap();
        assert_eq!(read_all(r), "hello");
    }

    #[test]
    fn tiny_and_empty_inputs_pass_through() {
        assert_eq!(
            read_all(decompressing(io::Cursor::new(Vec::new())).unwrap()),
            ""
        );
        assert_eq!(
            read_all(decompressing(io::Cursor::new(b"x".to_vec())).unwrap()),
            "x"
        );
    }

    #[test]
    fn locations_are_told_apart() {
        assert_eq!(
            locate(" https://a/b.m3u "),
            Location::Http("https://a/b.m3u".into())
        );
        assert_eq!(locate("HTTP://A/b"), Location::Http("HTTP://A/b".into()));
        assert_eq!(
            locate("file:///tmp/x.m3u"),
            Location::File("/tmp/x.m3u".into())
        );
        assert_eq!(locate("/tmp/x.m3u"), Location::File("/tmp/x.m3u".into()));
    }

    #[test]
    fn a_pasted_stream_is_recognised() {
        assert!(looks_like_stream(&[0x47, 0x40, 0x11], None));
        assert!(looks_like_stream(b"#EXTM3U", Some("video/mp2t")));
        assert!(!looks_like_stream(
            b"#EXTM3U\n",
            Some("application/x-mpegurl")
        ));
        assert!(!looks_like_stream(b"\xef\xbb\xbf#EXTM3U", None));
    }
}
