//! A movie's file at the provider, read for a browser: a response at a time
//! for the browser's own player ([`HttpUpstream::open`],
//! [`HttpUpstream::chunk`]), or as a file with seeks for the remuxer
//! ([`vodmux::Upstream`]).
//!
//! One response is open at a time, and moving elsewhere in the file drops
//! the one being read *before* the next is asked for: the account sees one
//! connection per viewer, never two for a moment, which a provider allowing
//! one stream would refuse, or settle by ending the older.
//!
//! The file is asked for with the movie's own user agent and referrer, as
//! the app's player would ([`Playable`]). Errors are [`Failure`]s, whose
//! words never carry the address (an Xtream address carries the account).

use std::time::Duration;

use bytes::{Buf, Bytes};
use reqwest::header;
use tokio::runtime::Handle;

use super::fallback::{Failure, STALL_SECS};
use super::hub;
use super::vodmux;
use crate::source::Playable;

/// How long the provider has to answer a request for the file.
const ANSWER_WAIT: Duration = Duration::from_secs(20);
/// A seek forward by no more than this, on a response already open, reads
/// its way there rather than asking again.
const SKIP_BY_READING: u64 = 256 << 10;

/// What a response from the provider holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// The provider answered with a part of the file (206); otherwise the
    /// whole of it (200), wherever it was asked from.
    pub partial: bool,
    /// The range asked for lies past the end of the file (416).
    pub beyond: bool,
    /// Where in the file the response starts.
    pub first: u64,
    /// The response's length, when said.
    pub length: Option<u64>,
    /// The file's, when said.
    pub total: Option<u64>,
}

/// A movie's file at the provider; see the module.
pub struct HttpUpstream {
    rt: Handle,
    http: reqwest::Client,
    playable: Playable,
    resp: Option<reqwest::Response>,
    /// Read from `resp` and not handed on yet; starts at `pos`.
    left: Bytes,
    /// Where in the file the next byte from `resp` (or `left`) is.
    pos: u64,
    /// Where [`vodmux::Upstream::read`] reads from next.
    want: u64,
    size: Option<u64>,
    /// The open response, while nothing has been read from it, and the
    /// range it was asked for: asking for the same again takes it as it is.
    fresh: Option<(u64, Option<u64>, Span)>,
}

impl HttpUpstream {
    /// Within the runtime, whose handle drives the blocking reads.
    pub fn new(playable: Playable) -> anyhow::Result<Self> {
        Ok(Self {
            rt: Handle::current(),
            http: crate::m3u::fetch::http_client(playable.user_agent.as_deref())?,
            playable,
            resp: None,
            left: Bytes::new(),
            pos: 0,
            want: 0,
            size: None,
            fresh: None,
        })
    }

    /// The file's length, once a response has said it.
    pub fn size(&self) -> Option<u64> {
        self.size
    }

    /// Lets the open response go, if there is one.
    pub fn close(&mut self) {
        self.resp = None;
        self.left = Bytes::new();
        self.fresh = None;
    }

    /// Asks for the file from byte `from` (to byte `to`, inclusive), the
    /// open response dropped first.
    pub async fn open(&mut self, from: u64, to: Option<u64>) -> Result<Span, Failure> {
        if let Some((f, t, span)) = self.fresh {
            if self.resp.is_some() && (f, t) == (from, to) {
                return Ok(span);
            }
        }
        self.close();
        let range = match to {
            Some(to) => format!("bytes={from}-{to}"),
            None => format!("bytes={from}-"),
        };
        let mut req = self
            .http
            .get(&self.playable.url)
            .header(header::RANGE, range);
        if let Some(r) = &self.playable.referrer {
            req = req.header(header::REFERER, r);
        }
        let resp = match tokio::time::timeout(ANSWER_WAIT, req.send()).await {
            Ok(Ok(r)) => r,
            _ => return Err(Failure::Unreachable),
        };
        let status = resp.status().as_u16();
        let range = content_range(resp.headers().get(header::CONTENT_RANGE));
        let length = resp
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let span = match status {
            206 => {
                let Some((first, _, total)) = range else {
                    return Err(Failure::NotVideo);
                };
                Span {
                    partial: true,
                    beyond: false,
                    first,
                    length,
                    total,
                }
            }
            200 => Span {
                partial: false,
                beyond: false,
                first: 0,
                length,
                total: length,
            },
            416 => {
                self.size = range.and_then(|(_, _, t)| t).or(self.size);
                return Ok(Span {
                    partial: false,
                    beyond: true,
                    first: from,
                    length: Some(0),
                    total: self.size,
                });
            }
            _ => {
                let body = tokio::time::timeout(Duration::from_secs(3), resp.text())
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .unwrap_or_default();
                let body: String = body.chars().take(512).collect();
                return Err(hub::refusal(status, &body));
            }
        };
        // A provider refusing politely answers with a page of text.
        let text = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|t| t.starts_with("text/"));
        if text {
            return Err(Failure::NotVideo);
        }
        if span.total.is_some() {
            self.size = span.total;
        }
        self.pos = span.first;
        self.resp = Some(resp);
        self.fresh = Some((from, to, span));
        Ok(span)
    }

    /// The next piece of the open response; `None` at its end, or with none
    /// open.
    pub async fn chunk(&mut self) -> Result<Option<Bytes>, Failure> {
        let b = if self.left.is_empty() {
            self.next().await?
        } else {
            Some(std::mem::take(&mut self.left))
        };
        if let Some(b) = &b {
            self.pos += b.len() as u64;
            self.want = self.pos;
        }
        Ok(b)
    }

    /// The response's next piece, as it comes.
    async fn next(&mut self) -> Result<Option<Bytes>, Failure> {
        let Some(resp) = self.resp.as_mut() else {
            return Ok(None);
        };
        self.fresh = None;
        let stall = Duration::from_secs(STALL_SECS as u64);
        match tokio::time::timeout(stall, resp.chunk()).await {
            Ok(Ok(Some(b))) => Ok(Some(b)),
            Ok(Ok(None)) => {
                self.resp = None;
                Ok(None)
            }
            Ok(Err(_)) => {
                self.resp = None;
                Err(Failure::Unreachable)
            }
            Err(_) => {
                self.resp = None;
                Err(Failure::Stalled)
            }
        }
    }
}

/// `bytes first-last/total` (the total may be `*`), or `bytes */total` on
/// a 416.
fn content_range(v: Option<&header::HeaderValue>) -> Option<(u64, u64, Option<u64>)> {
    let v = v?.to_str().ok()?.trim().strip_prefix("bytes")?.trim_start();
    let (span, total) = v.split_once('/')?;
    let total = total.trim().parse::<u64>().ok();
    if span.trim() == "*" {
        return Some((0, 0, total));
    }
    let (first, last) = span.split_once('-')?;
    Some((first.trim().parse().ok()?, last.trim().parse().ok()?, total))
}

fn io(f: Failure) -> std::io::Error {
    std::io::Error::other(f.describe())
}

/// The remuxer's side: called on its own thread, never the runtime's, so
/// the reads may block on the runtime handle.
impl vodmux::Upstream for HttpUpstream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let rt = self.rt.clone();
        loop {
            if self.left.is_empty() {
                if self.resp.is_none() {
                    if self.size.is_some_and(|s| self.want >= s) {
                        return Ok(0);
                    }
                    let want = self.want;
                    if rt.block_on(self.open(want, None)).map_err(io)?.beyond {
                        return Ok(0);
                    }
                }
                match rt.block_on(self.next()).map_err(io)? {
                    Some(b) => self.left = b,
                    None => return Ok(0),
                }
            }
            // A provider that sends the whole file whatever was asked: read
            // up to where the reading is.
            if self.pos < self.want {
                let skip = (self.want - self.pos).min(self.left.len() as u64) as usize;
                self.left.advance(skip);
                self.pos += skip as u64;
                continue;
            }
            let n = buf.len().min(self.left.len());
            buf[..n].copy_from_slice(&self.left[..n]);
            self.left.advance(n);
            self.pos += n as u64;
            self.want = self.pos;
            return Ok(n);
        }
    }

    fn seek(&mut self, pos: u64) -> std::io::Result<()> {
        let open = self.resp.is_some() || !self.left.is_empty();
        if open && pos >= self.pos && pos - self.pos <= SKIP_BY_READING {
            self.want = pos;
            return Ok(());
        }
        // Dropped before the next is asked for, at the next read.
        self.close();
        self.want = pos;
        Ok(())
    }

    fn size(&self) -> Option<u64> {
        self.size
    }
}

/// A provider's file for the tests: 1 MB of a known pattern on
/// 127.0.0.1, served with ranges, slowly enough that a response is still
/// going when the next is asked for, and counting the responses open at
/// once.
#[cfg(test)]
pub(crate) mod fake {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    pub const SIZE: usize = 1_000_000;

    /// The file's byte at `i`.
    pub fn byte(i: usize) -> u8 {
        (i % 251) as u8
    }

    #[derive(Default)]
    pub struct Counts {
        pub requests: AtomicUsize,
        pub open: AtomicUsize,
        pub most_open: AtomicUsize,
    }

    /// Where it is, and its counts. A request is counted open a moment after
    /// it comes (a connection the client has just dropped is seen closed
    /// first) and answered a while later still, so that a client that asked
    /// before dropping the response it had would be seen with two.
    pub async fn serve() -> (String, Arc<Counts>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let counts = Arc::new(Counts::default());
        let c = Arc::clone(&counts);
        tokio::spawn(async move {
            while let Ok((sock, _)) = l.accept().await {
                let c = Arc::clone(&c);
                tokio::spawn(async move {
                    let (mut rd, mut wr) = sock.into_split();
                    let mut req = Vec::new();
                    let mut buf = [0u8; 2048];
                    while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                        match rd.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => req.extend_from_slice(&buf[..n]),
                        }
                    }
                    c.requests.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    let now = c.open.fetch_add(1, Ordering::SeqCst) + 1;
                    c.most_open.fetch_max(now, Ordering::SeqCst);
                    struct Close(Arc<Counts>);
                    impl Drop for Close {
                        fn drop(&mut self) {
                            self.0.open.fetch_sub(1, Ordering::SeqCst);
                        }
                    }
                    let _close = Close(Arc::clone(&c));
                    tokio::time::sleep(Duration::from_millis(70)).await;

                    let text = String::from_utf8_lossy(&req).to_ascii_lowercase();
                    let range = text
                        .lines()
                        .find_map(|l| l.strip_prefix("range: bytes="))
                        .and_then(|r| r.trim().split_once('-'))
                        .map(|(a, b)| {
                            let a: usize = a.parse().unwrap_or(0);
                            let b: usize = b.parse().unwrap_or(SIZE - 1);
                            (a, b.min(SIZE - 1))
                        });
                    let (first, last) = range.unwrap_or((0, SIZE - 1));
                    let head = match range {
                        Some(_) => format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Type: video/mp4\r\nContent-Length: {}\r\nContent-Range: bytes {first}-{last}/{SIZE}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                            last + 1 - first
                        ),
                        None => format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nContent-Length: {SIZE}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n"
                        ),
                    };
                    if wr.write_all(head.as_bytes()).await.is_err() {
                        return;
                    }
                    let mut at = first;
                    while at <= last {
                        let end = (at + 16 * 1024).min(last + 1);
                        let piece: Vec<u8> = (at..end).map(byte).collect();
                        // The client gone is seen at once, written to or not.
                        tokio::select! {
                            r = wr.write_all(&piece) => if r.is_err() { return },
                            _ = rd.read(&mut buf) => return,
                        }
                        at = end;
                        tokio::time::sleep(Duration::from_millis(2)).await;
                    }
                });
            }
        });
        (base, counts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use vodmux::Upstream;

    #[test]
    fn a_content_range_is_read() {
        let v = |s: &'static str| Some(header::HeaderValue::from_static(s));
        assert_eq!(
            content_range(v("bytes 500-999/1000").as_ref()),
            Some((500, 999, Some(1000)))
        );
        assert_eq!(content_range(v("bytes 0-9/*").as_ref()), Some((0, 9, None)));
        assert_eq!(
            content_range(v("bytes */1000").as_ref()),
            Some((0, 0, Some(1000)))
        );
        assert_eq!(content_range(v("items 0-9/10").as_ref()), None);
        assert_eq!(content_range(None), None);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_remuxers_reads_and_seeks_ask_for_one_response_at_a_time() {
        let (base, counts) = fake::serve().await;
        let mut up = HttpUpstream::new(Playable::plain(format!("{base}/movie.mp4"))).unwrap();
        let span = up.open(0, None).await.unwrap();
        assert!(span.partial);
        assert_eq!(up.size(), Some(fake::SIZE as u64));
        // The remuxer's thread: blocking reads on the runtime's handle.
        let (up, counts) = tokio::task::spawn_blocking(move || {
            let mut up = up;
            let mut buf = [0u8; 1000];
            let n = up.read(&mut buf).unwrap();
            assert!(n > 0);
            assert!((0..n).all(|i| buf[i] == fake::byte(i)));
            // Far ahead: the response is dropped, the next asked from there.
            up.seek(600_000).unwrap();
            let mut got = 0;
            while got < buf.len() {
                let n = up.read(&mut buf[got..]).unwrap();
                assert!(n > 0);
                got += n;
            }
            assert!((0..got).all(|i| buf[i] == fake::byte(600_000 + i)));
            // A little ahead: read through, not asked again.
            let asked = counts.requests.load(Ordering::SeqCst);
            up.seek(601_500).unwrap();
            let n = up.read(&mut buf).unwrap();
            assert!((0..n).all(|i| buf[i] == fake::byte(601_500 + i)));
            assert_eq!(counts.requests.load(Ordering::SeqCst), asked);
            // The end of the file is the end.
            up.seek(fake::SIZE as u64).unwrap();
            assert_eq!(up.read(&mut buf).unwrap(), 0);
            (up, counts)
        })
        .await
        .unwrap();
        drop(up);
        assert_eq!(counts.requests.load(Ordering::SeqCst), 2);
        assert_eq!(counts.most_open.load(Ordering::SeqCst), 1);
    }
}
