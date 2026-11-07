//! One upstream connection per feed, shared.
//!
//! A hub holds the service's connection to one feed (one channel's stream)
//! and hands every chunk of it to each of its subscribers. A recording asks
//! [`Hubs::join`] first: when a hub for one of its feeds is open, it rides
//! that one and costs the account nothing. Only when none is open does it
//! need a connection of its own ([`super::sharing`]'s rules) and
//! [`Hubs::open`] one, which others then join. A subscriber leaves by
//! dropping its [`Subscription`]; the hub closes its connection when the
//! last one has left. When the feed fails (it stalls, ends, refuses) the hub
//! closes and tells each subscriber why, and each decides for itself what
//! to do next (its own `fallback`): the same feed again is a new hub, which
//! the others may join; its backup is another hub.
//!
//! The bytes are handed on in whole transport-stream packets (188 bytes),
//! from the first sync byte the provider sent, so whoever joins part way
//! starts on a packet boundary. That is all a subscriber needs to make a
//! playable file of what it gets, and all a browser watching the channel
//! needs too: a viewer (`super::watch`) is one more subscriber, a rider
//! whose programme never ends (`sharing::Rider::viewer`), so the rules never
//! close its connection for a recording.
//!
//! A subscriber that falls behind (a disk that stalls) does not hold the
//! others up: past a queue of [`QUEUE`] chunks, what it misses is counted
//! ([`Subscription::lost`]) and dropped.
//!
//! **Off air.** Some providers answer a channel that is not broadcasting
//! with a placeholder clip: minutes of black, sent as fast as the link
//! allows, and then the response ends. For its first [`OFF_AIR_WINDOW`] a
//! hub reads the stream time its packets carry ([`OffAirWatch`]) and judges
//! a clean end by the app's own rule (`crate::ipc::ending::off_air`). A clip
//! ends the hub as [`Failure::OffAir`]: every subscriber is told at once,
//! what was queued for it is dropped rather than read
//! ([`Subscription::recv`]), and the channel's record says "off air", as
//! the app writes it. Until the window has gone by, a subscriber may hold
//! back what it receives ([`Subscription::judged`]): a recording does, so
//! that a clip never reaches its file. A browser watching does not: it would
//! wait twenty seconds for every channel.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::{Bytes, BytesMut};
use tokio::sync::{mpsc, watch, Notify};

use super::fallback::{Failure, STALL_SECS};
use super::sharing::{Conn, Rider};
use super::tsprobe::StreamClock;
use crate::ipc::ending::{self, Placeholder, OFF_AIR_WINDOW};
use crate::source::Playable;

/// A transport stream's packet.
const PACKET: usize = 188;

/// Chunks queued for one subscriber before what it misses is dropped:
/// several seconds of any stream.
pub const QUEUE: usize = 1024;

/// How long after the service closes one of its own connections a provider
/// that counts connections is given to stop counting it. Meanwhile a full
/// account is looked at again every [`SETTLE_LOOK_SECS`] instead of backing
/// off, and no other connection is closed for the same need.
pub const SETTLE_SECS: u64 = 20;
pub const SETTLE_LOOK_SECS: i64 = 3;

/// Every hub the service has open, by feed.
pub struct Hubs {
    map: Mutex<HashMap<i64, Arc<Hub>>>,
    /// Upstream connections open now (each hub's, from its connecting to its
    /// closing).
    open: AtomicUsize,
    /// Bumped whenever a hub opens or closes: whoever waits for a
    /// connection, or for a feed to ride, looks again.
    changes: watch::Sender<u64>,
    /// When connections closed, the last [`SETTLE_SECS`]' worth.
    released: Mutex<VecDeque<Instant>>,
}

struct Hub {
    feed: i64,
    state: Mutex<HubState>,
    /// Told when a subscriber leaves.
    left: Notify,
    /// Set once the stream is known not to be a placeholder clip: its first
    /// [`OFF_AIR_WINDOW`] went by without the end of one. Never set by an
    /// end, so a subscriber holding data back reads the end first.
    judged: watch::Sender<bool>,
}

struct HubState {
    subs: Vec<Sub>,
    next_id: u64,
    /// Closed, or closing: nobody joins it any more.
    doomed: bool,
}

struct Sub {
    id: u64,
    rider: Rider,
    tx: mpsc::Sender<Bytes>,
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    lost: AtomicU64,
    /// Why the hub closed, set before the subscriber's queue ends.
    ended: Mutex<Option<(Failure, i64)>>,
    /// What is still queued is not to be read: the feed was a placeholder
    /// clip. Set with `ended`.
    cut: AtomicBool,
}

/// A place on a hub. Dropping it leaves.
pub struct Subscription {
    hub: Arc<Hub>,
    id: u64,
    rx: mpsc::Receiver<Bytes>,
    shared: Arc<Shared>,
}

impl Subscription {
    /// The feed it carries.
    pub fn feed(&self) -> i64 {
        self.hub.feed
    }

    /// The next chunk, or `None` once the hub has closed ([`Self::ended`]
    /// says why). After a placeholder clip, `None` at once: the rest of the
    /// clip is never handed on.
    pub async fn recv(&mut self) -> Option<Bytes> {
        if self.shared.cut.load(Ordering::SeqCst) {
            return None;
        }
        let got = self.rx.recv().await;
        got.filter(|_| !self.shared.cut.load(Ordering::SeqCst))
    }

    /// How the feed ended, when it ended as off air: nothing is left to
    /// read, so a viewer nobody reads yet can be told at once.
    pub fn cut_off(&self) -> Option<(Failure, i64)> {
        if self.shared.cut.load(Ordering::SeqCst) {
            self.ended()
        } else {
            None
        }
    }

    /// Whether the stream is known not to be a placeholder clip (see the
    /// module's "Off air"), and changes when it becomes so. Until then a
    /// subscriber that must never keep a clip holds back what it receives.
    /// Never true for a stream that ends first.
    pub fn judged(&self) -> watch::Receiver<bool> {
        self.hub.judged.subscribe()
    }

    /// How the feed failed, and for how many seconds it had delivered.
    pub fn ended(&self) -> Option<(Failure, i64)> {
        *self.shared.ended.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Bytes dropped because this subscriber fell behind.
    pub fn lost(&self) -> u64 {
        self.shared.lost.load(Ordering::Relaxed)
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let mut st = self.hub.state.lock().unwrap_or_else(|e| e.into_inner());
        st.subs.retain(|s| s.id != self.id);
        drop(st);
        self.hub.left.notify_one();
    }
}

impl Default for Hubs {
    fn default() -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
            open: AtomicUsize::new(0),
            changes: watch::channel(0).0,
            released: Mutex::new(VecDeque::new()),
        }
    }
}

impl Hubs {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Upstream connections open now.
    pub fn open_count(&self) -> usize {
        self.open.load(Ordering::SeqCst)
    }

    /// Changes whenever a hub opens or closes.
    pub fn watch(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    fn changed(&self) {
        self.changes.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// A hub open (and not closing) for `feed`.
    pub fn has(&self, feed: i64) -> bool {
        self.lock().get(&feed).is_some_and(|h| !h.state().doomed)
    }

    /// The open connections, as the rules see them. Hubs being closed are
    /// left out: they are already given up.
    pub fn conns(&self) -> Vec<Conn> {
        self.lock()
            .values()
            .filter_map(|h| {
                let st = h.state();
                (!st.doomed).then(|| Conn {
                    feed: h.feed,
                    riders: st.subs.iter().map(|s| s.rider).collect(),
                })
            })
            .collect()
    }

    /// Whether a connection is being closed to make room for a recording.
    pub fn closing(&self) -> bool {
        self.lock().values().any(|h| h.state().doomed)
    }

    /// Whether one of the service's connections closed within
    /// [`SETTLE_SECS`]: a provider's count may still include it.
    pub fn released_lately(&self) -> bool {
        self.released_recently() > 0
    }

    /// How many of the service's connections closed within
    /// [`SETTLE_SECS`]: how many a provider's count may still include.
    pub fn released_recently(&self) -> usize {
        let mut r = self.released.lock().unwrap_or_else(|e| e.into_inner());
        let settle = Duration::from_secs(SETTLE_SECS);
        while r.front().is_some_and(|t| t.elapsed() >= settle) {
            r.pop_front();
        }
        r.len()
    }

    /// Rides a hub already open for one of `feeds`, tried in order: the
    /// index of the feed ridden, and the place.
    pub fn join(&self, feeds: &[i64], rider: Rider) -> Option<(usize, Subscription)> {
        let map = self.lock();
        feeds.iter().enumerate().find_map(|(i, f)| {
            let hub = map.get(f)?;
            subscribe(hub, rider).map(|s| (i, s))
        })
    }

    /// Rides the hub for `feed`, opening it (and its connection, to
    /// `playable`) when none is open. Opening is the caller's decision: the
    /// account must have the connection to spare.
    pub fn open(self: &Arc<Self>, feed: i64, playable: Playable, rider: Rider) -> Subscription {
        let mut map = self.lock();
        if let Some(sub) = map.get(&feed).and_then(|h| subscribe(h, rider)) {
            return sub;
        }
        let hub = Arc::new(Hub {
            feed,
            state: Mutex::new(HubState {
                subs: Vec::new(),
                next_id: 0,
                doomed: false,
            }),
            left: Notify::new(),
            judged: watch::channel(false).0,
        });
        let sub = subscribe(&hub, rider).expect("a new hub takes subscribers");
        map.insert(feed, Arc::clone(&hub));
        drop(map);
        self.open.fetch_add(1, Ordering::SeqCst);
        self.changed();
        tokio::spawn(Arc::clone(self).run(hub, playable));
        sub
    }

    /// Marks the hub for `feed` as closing when, at `now`, everyone on it
    /// is past their programme's end, and says who they are: their
    /// recordings are then to end, which closes it. Checked and marked at
    /// once, so nobody joins it in between.
    pub fn doom(&self, feed: i64, now: i64) -> Option<Vec<Rider>> {
        let map = self.lock();
        let hub = map.get(&feed)?;
        let mut st = hub.state();
        let conn = Conn {
            feed,
            riders: st.subs.iter().map(|s| s.rider).collect(),
        };
        if st.doomed || !conn.only_padding(now) {
            return None;
        }
        st.doomed = true;
        Some(conn.riders)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<i64, Arc<Hub>>> {
        self.map.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The hub's life: connect, hand on what comes, close.
    async fn run(self: Arc<Self>, hub: Arc<Hub>, playable: Playable) {
        let end = self.pump(&hub, &playable).await;
        self.open.fetch_sub(1, Ordering::SeqCst);
        self.released
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(Instant::now());
        let subs = {
            let mut map = self.lock();
            if map.get(&hub.feed).is_some_and(|h| Arc::ptr_eq(h, &hub)) {
                map.remove(&hub.feed);
            }
            let mut st = hub.state();
            st.doomed = true;
            std::mem::take(&mut st.subs)
        };
        let cut = matches!(end, Some((Failure::OffAir { .. }, _)));
        for s in subs {
            *s.shared.ended.lock().unwrap_or_else(|e| e.into_inner()) = end;
            s.shared.cut.store(cut, Ordering::SeqCst);
            // Dropping the sender ends the subscriber's queue once it has
            // read what is in it.
        }
        self.changed();
        if let Some((f, _)) = end {
            tracing::info!("feed {}: closed ({})", hub.feed, f.describe());
        }
        if cut {
            let (feed, at) = (hub.feed, super::now());
            ledger(move |c| {
                crate::catalog::store::record_failed(c, feed, ending::OFF_AIR_REASON, at)
            });
        }
    }

    /// Connects and hands chunks on until the feed fails (how, and for how
    /// long it had delivered) or nobody is left (`None`).
    async fn pump(&self, hub: &Hub, p: &Playable) -> Option<(Failure, i64)> {
        // The off-air window is counted from the open, as the app counts it
        // from loading the stream.
        let opened = Instant::now();
        let judge_at = tokio::time::Instant::from_std(opened + Duration::from_secs_f64(OFF_AIR_WINDOW));
        let mut watch = Some(OffAirWatch::default());
        let connected = tokio::select! {
            r = connect(p) => r,
            _ = self.deserted(hub) => return None,
        };
        let mut resp = match connected {
            Ok(r) => r,
            Err(f) => return Some((f, 0)),
        };
        let stall = Duration::from_secs(STALL_SECS as u64);
        let since = Instant::now();
        let mut first: Option<Instant> = None;
        let delivered = |first: Option<Instant>| first.map_or(0, |f| f.elapsed().as_secs() as i64);
        let mut buf = BytesMut::new();
        loop {
            let chunk = tokio::select! {
                r = tokio::time::timeout(stall, resp.chunk()) => r,
                _ = self.deserted(hub) => return None,
                _ = tokio::time::sleep_until(judge_at), if watch.is_some() => {
                    // The window went by without the end of a clip: a
                    // broadcast, and any mark of "off air" is over.
                    watch = None;
                    hub.judged.send_replace(true);
                    let feed = hub.feed;
                    ledger(move |c| crate::catalog::store::clear_off_air(c, feed));
                    continue;
                }
            };
            match chunk {
                Err(_) => return Some((Failure::Stalled, delivered(first))),
                Ok(Err(_)) => return Some((Failure::Unreachable, delivered(first))),
                Ok(Ok(None)) => {
                    if first.is_none() {
                        let Some(at) = sync_at(&buf) else {
                            return Some((Failure::NotVideo, 0));
                        };
                        let _ = buf.split_to(at);
                    }
                    if !buf.is_empty() {
                        let rest = buf.split().freeze();
                        if let Some(w) = watch.as_mut() {
                            w.feed(opened.elapsed().as_secs_f64(), &rest);
                        }
                        hub.send(rest);
                    }
                    // A clean end, the one way a placeholder ends.
                    let at = opened.elapsed().as_secs_f64();
                    if let Some(clip) = watch.as_mut().and_then(|w| w.ended(at)) {
                        tracing::info!(
                            "feed {}: the provider sent {clip:.0} s of stream and closed it {at:.1} s after the open; a placeholder, not the channel",
                            hub.feed
                        );
                        let clip_secs = clip.round() as u32;
                        return Some((Failure::OffAir { clip_secs }, 0));
                    }
                    return Some((Failure::Ended, delivered(first)));
                }
                Ok(Ok(Some(c))) => {
                    buf.extend_from_slice(&c);
                    if first.is_none() {
                        if buf.len() < 2 * PACKET && since.elapsed() < stall {
                            continue;
                        }
                        let Some(at) = sync_at(&buf) else {
                            return Some((Failure::NotVideo, 0));
                        };
                        let _ = buf.split_to(at);
                        first = Some(Instant::now());
                    }
                    let whole = buf.len() / PACKET * PACKET;
                    if whole > 0 {
                        let packets = buf.split_to(whole).freeze();
                        if let Some(w) = watch.as_mut() {
                            w.feed(opened.elapsed().as_secs_f64(), &packets);
                        }
                        hub.send(packets);
                    }
                }
            }
        }
    }

    /// Resolves once nobody is on `hub`, marking it closing first so
    /// nobody joins it meanwhile.
    async fn deserted(&self, hub: &Hub) {
        loop {
            {
                let _map = self.lock();
                let mut st = hub.state();
                if st.subs.is_empty() {
                    st.doomed = true;
                    return;
                }
            }
            hub.left.notified().await;
        }
    }
}

impl Hub {
    fn state(&self) -> std::sync::MutexGuard<'_, HubState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Hands a chunk to every subscriber.
    fn send(&self, data: Bytes) {
        let st = self.state();
        for s in &st.subs {
            if let Err(mpsc::error::TrySendError::Full(b)) = s.tx.try_send(data.clone()) {
                s.shared.lost.fetch_add(b.len() as u64, Ordering::Relaxed);
            }
        }
    }
}

/// Writes to the channel's record (`channel_quality`) off the async
/// threads. A record that cannot be written costs the list a mark, never the
/// stream, so a failure is only logged.
fn ledger(f: impl FnOnce(&rusqlite::Connection) -> anyhow::Result<()> + Send + 'static) {
    tokio::task::spawn_blocking(move || {
        if let Err(e) = crate::catalog::db::with_db(f) {
            tracing::warn!("could not write the channel's record: {e:#}");
        }
    });
}

/// Watches a connection's first [`OFF_AIR_WINDOW`] for the end of a
/// placeholder clip, by the app's rule ([`ending::off_air`]): the stream
/// time the packets carry ([`StreamClock`]) is measured as the app measures
/// mpv's cache ([`Placeholder`]), with the bytes behind it.
///
/// Pure, with the time since the open passed in, so every case is a test.
#[derive(Debug, Default)]
pub struct OffAirWatch {
    clock: StreamClock,
    span: Placeholder,
    /// Seconds since the open, at the last packets.
    at: f64,
    bytes: i64,
}

impl OffAirWatch {
    /// Whole packets, received `at` seconds after the open.
    pub fn feed(&mut self, at: f64, packets: &[u8]) {
        self.bytes += packets.len() as i64;
        let mut dt = (at - self.at).max(0.0);
        self.at = self.at.max(at);
        let (span, bytes) = (&mut self.span, self.bytes);
        let mut seen = false;
        self.clock.feed(packets, |t| {
            span.observe(dt, None, Some(t), false, Some(bytes));
            dt = 0.0;
            seen = true;
        });
        if !seen {
            span.observe(dt, None, None, false, Some(bytes));
        }
    }

    /// The response ended cleanly `at` seconds after the open: the clip's
    /// length, when what came was a placeholder clip. What a hub reads is
    /// MPEG-TS of a live channel, and a clean end is no network failure;
    /// the rest is the rule's.
    pub fn ended(&mut self, at: f64) -> Option<f64> {
        let dt = (at - self.at).max(0.0);
        let read = self.span.observe(dt, None, None, true, Some(self.bytes))?;
        ending::off_air(
            read,
            ending::Context {
                live: true,
                format: Some("mpegts"),
                network: false,
            },
        )
    }
}

/// A place on `hub`, unless it is closing.
fn subscribe(hub: &Arc<Hub>, rider: Rider) -> Option<Subscription> {
    let mut st = hub.state();
    if st.doomed {
        return None;
    }
    let (tx, rx) = mpsc::channel(QUEUE);
    let shared = Arc::new(Shared::default());
    st.next_id += 1;
    let id = st.next_id;
    st.subs.push(Sub {
        id,
        rider,
        tx,
        shared: Arc::clone(&shared),
    });
    Some(Subscription {
        hub: Arc::clone(hub),
        id,
        rx,
        shared,
    })
}

/// Where a transport stream's packets start: a sync byte, and another one
/// packet later. A provider refusing politely sends a line of text with a
/// success status instead.
pub fn sync_at(head: &[u8]) -> Option<usize> {
    (0..PACKET.min(head.len()))
        .find(|&i| head[i] == 0x47 && head.get(i + PACKET).is_none_or(|&b| b == 0x47))
}

/// Whether the start of a body is an MPEG transport stream.
pub fn looks_like_ts(head: &[u8]) -> bool {
    sync_at(head).is_some()
}

/// A refusal's status and words, as a [`Failure`].
pub(super) fn refusal(status: u16, body: &str) -> Failure {
    let mut clues = crate::ipc::ending::Clues::default();
    clues.observe(&format!(
        "HTTP error {status} {}",
        body.lines().next().unwrap_or("")
    ));
    Failure::Refused {
        status,
        limit: clues.limit,
    }
}

/// Opens the stream the way mpv does when the app plays the channel: the
/// same URL, User-Agent and Referer. A refusal is read for its words (a
/// provider says "max connections" in the body as often as in the status
/// line).
async fn connect(p: &Playable) -> Result<reqwest::Response, Failure> {
    let http = crate::m3u::fetch::http_client(p.user_agent.as_deref())
        .map_err(|_| Failure::Unreachable)?;
    let mut req = http.get(&p.url);
    if let Some(r) = &p.referrer {
        req = req.header(reqwest::header::REFERER, r);
    }
    let resp = req.send().await.map_err(|_| Failure::Unreachable)?;
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = tokio::time::timeout(Duration::from_secs(3), resp.text())
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    let body: String = body.chars().take(512).collect();
    Err(refusal(status.as_u16(), &body))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use std::sync::atomic::AtomicUsize;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn a_transport_stream_is_told_from_a_line_of_text() {
        let mut ts = vec![0u8; 400];
        ts[3] = 0x47;
        ts[3 + 188] = 0x47;
        assert!(looks_like_ts(&ts));
        assert_eq!(sync_at(&ts), Some(3));
        assert!(!looks_like_ts(b"Max connections reached\n"));
        assert!(!looks_like_ts(&[0u8; 400]));
    }

    #[test]
    fn a_refusal_that_speaks_of_a_limit_is_one() {
        assert_eq!(
            refusal(403, "Max Connections Reached"),
            Failure::Refused {
                status: 403,
                limit: true
            }
        );
        assert_eq!(
            refusal(403, "Forbidden"),
            Failure::Refused {
                status: 403,
                limit: false
            }
        );
    }

    /// Packets numbered in their payload, so what a subscriber got can be
    /// checked for order and gaps.
    fn packet(n: u32) -> Vec<u8> {
        let mut p = vec![0xffu8; PACKET];
        p[0] = 0x47;
        p[4..8].copy_from_slice(&n.to_be_bytes());
        p
    }

    /// A stream server on 127.0.0.1 sending numbered packets, 50 a tick, in
    /// odd-sized writes (so chunks never line up with packets), for as long
    /// as the client stays. Counts the connections it took.
    async fn stream_server() -> (String, Arc<AtomicUsize>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/live/1.ts", l.local_addr().unwrap());
        let count = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&count);
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else {
                    return;
                };
                c.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut req = [0u8; 2048];
                    let _ = s.read(&mut req).await;
                    let head =
                        "HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nConnection: close\r\n\r\n";
                    if s.write_all(head.as_bytes()).await.is_err() {
                        return;
                    }
                    // Some junk before the first packet, as some providers send.
                    let _ = s.write_all(&[0u8; 7]).await;
                    let mut n = 0u32;
                    loop {
                        let mut tick = Vec::new();
                        for _ in 0..50 {
                            tick.extend(packet(n));
                            n += 1;
                        }
                        for piece in tick.chunks(1000) {
                            if s.write_all(piece).await.is_err() {
                                return;
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                });
            }
        });
        (url, count)
    }

    fn rider(id: i64) -> Rider {
        Rider {
            id,
            stop: i64::MAX - 1,
            until: i64::MAX - 1,
        }
    }

    /// Reads at least `n` bytes; they must be whole, consecutive packets.
    async fn take(sub: &mut Subscription, n: usize) -> Vec<u32> {
        let mut got = Vec::new();
        let mut bytes = 0;
        while bytes < n {
            let b = tokio::time::timeout(Duration::from_secs(10), sub.recv())
                .await
                .expect("data in time")
                .expect("the hub is open");
            assert_eq!(b.len() % PACKET, 0, "whole packets only");
            bytes += b.len();
            for p in b.chunks(PACKET) {
                assert_eq!(p[0], 0x47, "on a packet boundary");
                got.push(u32::from_be_bytes(p[4..8].try_into().unwrap()));
            }
        }
        assert!(
            got.windows(2).all(|w| w[1] == w[0] + 1),
            "in order, no gaps"
        );
        got
    }

    #[tokio::test]
    async fn two_subscribers_share_one_connection_and_it_closes_when_both_leave() {
        let (url, connections) = stream_server().await;
        let hubs = Hubs::new();
        let mut changes = hubs.watch();
        let mut a = hubs.open(7, Playable::plain(url.clone()), rider(1));
        assert!(changes.has_changed().unwrap(), "opening is a change");
        changes.mark_unchanged();
        let first = take(&mut a, 20 * PACKET).await;
        assert_eq!(
            first[0], 0,
            "from the first packet, the junk before it dropped"
        );

        // B joins part way: one connection still, and from where A is now.
        assert!(hubs.has(7));
        let (i, mut b) = hubs.join(&[9, 7], rider(2)).expect("a hub for feed 7");
        assert_eq!(i, 1);
        let from_a = take(&mut a, 40 * PACKET).await;
        let from_b = take(&mut b, 20 * PACKET).await;
        assert!(
            from_b[0] > first[0],
            "B starts where the stream is, not at its start"
        );
        assert!(
            from_a.contains(&from_b[0]),
            "what both have is the same packets"
        );
        assert_eq!(hubs.open_count(), 1);
        assert_eq!(hubs.conns().len(), 1);
        assert_eq!(hubs.conns()[0].riders.len(), 2);

        // A leaves: B carries on, on the same connection.
        drop(a);
        take(&mut b, 20 * PACKET).await;
        assert_eq!(hubs.open_count(), 1);

        // B leaves: the connection closes.
        drop(b);
        let t = Instant::now();
        while hubs.open_count() > 0 {
            assert!(
                t.elapsed() < Duration::from_secs(5),
                "the hub did not close"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!hubs.has(7));
        assert!(changes.has_changed().unwrap(), "closing is a change");
        assert!(hubs.released_lately(), "a connection closed a moment ago");
        assert_eq!(
            connections.load(Ordering::SeqCst),
            1,
            "one upstream connection in all"
        );
    }

    #[tokio::test]
    async fn a_hub_whose_riders_are_all_in_their_padding_can_be_closed_for_another() {
        let (url, _) = stream_server().await;
        let hubs = Hubs::new();
        let past = Rider {
            id: 1,
            stop: 100,
            until: 400,
        };
        let on = Rider {
            id: 2,
            stop: 300,
            until: 600,
        };
        let a = hubs.open(7, Playable::plain(url), past);
        assert_eq!(hubs.doom(7, 200), Some(vec![past]));
        assert!(
            hubs.join(&[7], on).is_none(),
            "nobody joins a hub being closed"
        );
        assert!(hubs.conns().is_empty(), "a hub being closed is given up");
        assert!(hubs.closing());
        drop(a);
    }

    #[tokio::test]
    async fn a_programme_on_the_hub_keeps_it_open() {
        let (url, _) = stream_server().await;
        let hubs = Hubs::new();
        let a = hubs.open(
            7,
            Playable::plain(url),
            Rider {
                id: 1,
                stop: 100,
                until: 400,
            },
        );
        let (_, b) = hubs
            .join(
                &[7],
                Rider {
                    id: 2,
                    stop: 300,
                    until: 600,
                },
            )
            .unwrap();
        assert_eq!(hubs.doom(7, 200), None);
        assert_eq!(hubs.doom(7, 300).map(|r| r.len()), Some(2));
        drop((a, b));
    }

    #[tokio::test]
    async fn a_feed_that_fails_tells_every_subscriber() {
        // A server that takes the connection, says nothing, and hangs up.
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/x.ts", l.local_addr().unwrap());
        tokio::spawn(async move {
            if let Ok((s, _)) = l.accept().await {
                tokio::time::sleep(Duration::from_millis(500)).await;
                drop(s);
            }
        });
        let hubs = Hubs::new();
        let mut a = hubs.open(7, Playable::plain(url), rider(1));
        let (_, mut b) = hubs
            .join(&[7], rider(2))
            .expect("a hub connecting can be joined");
        assert!(tokio::time::timeout(Duration::from_secs(10), a.recv())
            .await
            .unwrap()
            .is_none());
        assert!(tokio::time::timeout(Duration::from_secs(10), b.recv())
            .await
            .unwrap()
            .is_none());
        assert_eq!(a.ended(), Some((Failure::Unreachable, 0)));
        assert_eq!(b.ended(), Some((Failure::Unreachable, 0)));
        assert_eq!(hubs.open_count(), 0);
        assert!(!hubs.has(7));
    }

    // ------------------------------------------------------------ off air

    use crate::recording::tsprobe::tests::pcr_packet;

    /// `secs` of stream on PID 0x100: a PCR every tenth of a second, each
    /// followed by a packet of nothing, about 3.8 KB a second, as dense as
    /// a black and silent clip.
    pub(crate) fn stream_of(from: f64, secs: f64) -> Vec<u8> {
        let mut out = Vec::new();
        let mut null = vec![0xffu8; PACKET];
        null[..4].copy_from_slice(&[0x47, 0x1f, 0xff, 0x10]);
        let mut t = from;
        while t <= from + secs + 1e-9 {
            out.extend(pcr_packet(0x100, t));
            out.extend_from_slice(&null);
            t += 0.1;
        }
        out
    }

    /// A broadcast as a server sends it: `burst` seconds of stream at once,
    /// then a second of it each second, ended `end` seconds after the open.
    fn broadcast_ended_at(burst: f64, end: f64) -> Option<f64> {
        let mut w = OffAirWatch::default();
        w.feed(0.2, &stream_of(0.0, burst));
        let mut t = 1.0;
        while t <= end {
            w.feed(0.2 + t, &stream_of(burst + t - 0.9, 0.9));
            t += 1.0;
        }
        w.ended(end)
    }

    #[test]
    fn a_clip_sent_at_once_and_closed_is_off_air() {
        let clip = stream_of(1000.0, 600.0);
        let mut w = OffAirWatch::default();
        for (i, p) in clip.chunks(PACKET * 350).enumerate() {
            w.feed(0.1 + i as f64 * 0.01, p);
        }
        let len = w.ended(0.4).expect("a placeholder");
        assert!((len - 600.0).abs() < 0.5, "{len}");
        // Paced over seconds, as a slower link brings it: still one.
        let mut w = OffAirWatch::default();
        for (i, p) in clip.chunks(PACKET * 350).enumerate() {
            w.feed(0.5 + i as f64 * 0.4, p);
        }
        assert!(w.ended(16.0).is_some());
    }

    #[test]
    fn a_broadcast_that_ends_after_its_burst_is_not_off_air() {
        // Up to 60 s at once (the longest connect burst measured), then
        // live, taken away or dropped within the window.
        for burst in [5.0, 30.0, 60.0] {
            for end in [2.0, 10.0, 19.5] {
                assert_eq!(broadcast_ended_at(burst, end), None, "{burst} s, ended at {end}");
            }
        }
    }

    #[test]
    fn the_rules_are_the_apps() {
        let clip = stream_of(0.0, 600.0);
        let at_once = |w: &mut OffAirWatch, data: &[u8]| w.feed(0.3, data);
        // An end after the window.
        let mut w = OffAirWatch::default();
        at_once(&mut w, &clip);
        assert_eq!(w.ended(OFF_AIR_WINDOW + 1.0), None);
        // A clip shorter than the least.
        let mut w = OffAirWatch::default();
        at_once(&mut w, &stream_of(0.0, 90.0));
        assert_eq!(w.ended(1.0), None);
        // A span of hours in a moment: timestamps that jumped.
        let mut w = OffAirWatch::default();
        at_once(&mut w, &stream_of(0.0, 5.0));
        at_once(&mut w, &stream_of(8.0 * 3600.0, 5.0));
        assert_eq!(w.ended(1.0), None);
        // A span with too few bytes behind it: a forward jump.
        let mut w = OffAirWatch::default();
        at_once(&mut w, &pcr_packet(0x100, 0.0));
        at_once(&mut w, &pcr_packet(0x100, 300.0));
        assert_eq!(w.ended(1.0), None);
        // No clock in it at all: nothing to measure, nothing decided.
        let mut w = OffAirWatch::default();
        at_once(&mut w, &vec![0x47u8; PACKET * 10_000]);
        assert_eq!(w.ended(1.0), None);
        // Timestamps that went back start the span again.
        let mut w = OffAirWatch::default();
        at_once(&mut w, &stream_of(500.0, 100.0));
        at_once(&mut w, &stream_of(0.0, 100.0));
        assert_eq!(w.ended(1.0), None, "two 100 s timelines are not a 600 s clip");
    }

    /// A provider answering every stream with `body`, then closing: the
    /// placeholder for a channel off the air. Counts the connections.
    async fn clip_server(body: Vec<u8>) -> (String, Arc<AtomicUsize>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/live/1.ts", l.local_addr().unwrap());
        let count = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&count);
        let body = Arc::new(body);
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                c.fetch_add(1, Ordering::SeqCst);
                let body = Arc::clone(&body);
                tokio::spawn(async move {
                    let mut req = [0u8; 2048];
                    let _ = s.read(&mut req).await;
                    let head = "HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nConnection: close\r\n\r\n";
                    let _ = s.write_all(head.as_bytes()).await;
                    let _ = s.write_all(&body).await;
                    let _ = s.shutdown().await;
                });
            }
        });
        (url, count)
    }

    #[tokio::test]
    async fn every_rider_of_a_clip_learns_it_is_off_air_and_reads_none_of_it() {
        crate::catalog::db::install_test_db();
        let (url, connections) = clip_server(stream_of(0.0, 600.0)).await;
        let hubs = Hubs::new();
        let mut a = hubs.open(77_001, Playable::plain(url), rider(1));
        let (_, mut b) = hubs.join(&[77_001], rider(2)).expect("a hub connecting can be joined");
        let t = Instant::now();
        while hubs.open_count() > 0 {
            assert!(t.elapsed() < Duration::from_secs(10), "the hub did not close");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        for sub in [&mut a, &mut b] {
            let want = Some((Failure::OffAir { clip_secs: 600 }, 0));
            assert_eq!(sub.ended(), want);
            assert_eq!(sub.cut_off(), want);
            assert!(sub.recv().await.is_none(), "what was queued is not read");
            assert!(!*sub.judged().borrow(), "never judged a broadcast");
        }
        assert_eq!(connections.load(Ordering::SeqCst), 1);
        // The channel's record says so, as the app writes it.
        let reason = || {
            crate::catalog::db::with_db(|c| {
                Ok(c.query_row(
                    "SELECT fail_reason FROM channel_quality WHERE stream_id = 77001",
                    [],
                    |r| r.get::<_, Option<String>>(0),
                )
                .ok()
                .flatten())
            })
            .unwrap()
        };
        let t = Instant::now();
        while reason().is_none() {
            assert!(t.elapsed() < Duration::from_secs(5), "nothing was written");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(reason().as_deref(), Some(ending::OFF_AIR_REASON));
    }

    #[tokio::test]
    async fn a_short_stream_that_ends_is_ended_not_off_air() {
        let (url, _) = clip_server(stream_of(0.0, 30.0)).await;
        let hubs = Hubs::new();
        let mut a = hubs.open(77_002, Playable::plain(url), rider(1));
        let mut got = 0;
        while let Some(b) = tokio::time::timeout(Duration::from_secs(10), a.recv()).await.unwrap() {
            got += b.len();
        }
        assert_eq!(got, stream_of(0.0, 30.0).len(), "all of it is read");
        assert_eq!(a.ended().map(|e| e.0), Some(Failure::Ended));
        assert_eq!(a.cut_off(), None);
    }
}
