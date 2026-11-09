//! What the player controller asks of a player, and the one installed.
//!
//! The controller (`ipc::player_cmds`) and the title preview worker
//! (`surface::title_thumbs`) reach the player only through
//! [`PlayerBackend`], from whichever thread they are on: a Tauri command's
//! worker, the heartbeat's own thread, the preview worker. A backend is
//! `Send + Sync` and every method takes `&self`, so it is shared as an
//! `Arc` rather than kept in a thread's locals.
//!
//! mpv-shaped for now: the methods are the ones the controller already
//! called on mpv, with mpv's property names. Where a picture is drawn stays
//! the platform surface's business, and on its own thread.

use std::path::Path;
use std::sync::{Arc, PoisonError, RwLock};

use anyhow::Result;

/// The diagnostics graphs' four numbers, as [`PlayerBackend::health`] reads
/// them.
pub use crate::ipc::health::Reading as Health;

/// What a backend can do beyond playing, for a caller deciding whether to
/// offer it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendCaps {
    /// Holds what has played, so a live channel can be rewound.
    pub rewind_buffer: bool,
    /// Writes what it holds to a file ([`PlayerBackend::dump_cache`]).
    pub cache_dump: bool,
    /// Takes audio filters (`af`).
    pub audio_filters: bool,
    /// Reports where on its grid a closed caption sits (`sub-text/ass`).
    pub caption_geometry: bool,
}

/// The player, as the controller drives it. Callable from any thread.
pub trait PlayerBackend: Send + Sync {
    /// What this backend can do beyond playing.
    fn capabilities(&self) -> BackendCaps;

    /// Starts playback of `url`, or holds it until the surface can show it.
    fn load(&self, url: &str) -> Result<()>;

    fn set_property(&self, key: &str, value: &str) -> Result<()>;

    /// Sends a raw command, as written in mpv's `input.conf`.
    fn command(&self, name: &str, args: &[&str]) -> Result<()>;

    fn set_flag(&self, key: &str, value: bool) -> Result<()>;

    fn set_f64(&self, key: &str, value: f64) -> Result<()>;

    /// `None` rather than `Err` throughout the getters: almost every property
    /// is legitimately unavailable before the first frame, and a caller
    /// polling for it does not want an error per tick.
    fn get_f64(&self, key: &str) -> Option<f64>;

    fn get_i64(&self, key: &str) -> Option<i64>;

    fn get_bool(&self, key: &str) -> Option<bool>;

    /// `None` for an empty string as well as a missing property.
    fn get_string(&self, key: &str) -> Option<String>;

    /// What happened since the last pump. Never blocks.
    fn pump(&self) -> Vec<Signal>;

    /// Everything the state feed needs, in one visit.
    fn probe(&self) -> Probe;

    /// Everything the diagnostics panel shows.
    fn stats(&self) -> Stats;

    /// What the channel list remembers about a channel.
    fn reading(&self) -> crate::catalog::store::Reading;

    /// Just the numbers the diagnostics graphs plot.
    fn health(&self) -> Health;

    /// Writes what the cache holds from `start` to `end` (the stream's time)
    /// to `path`. Blocks until the file is written, so never on a thread that
    /// draws or answers the interface.
    fn dump_cache(&self, start: f64, end: f64, path: &Path) -> Result<()>;
}

/// The installed backend. A read lock is all a caller takes, and only long
/// enough to clone the `Arc`: nothing is called on the backend while it is
/// held.
static INSTALLED: RwLock<Option<Arc<dyn PlayerBackend>>> = RwLock::new(None);

/// Makes `b` the player every caller reaches. Called by the platform's
/// surface once it has created the player; a second call replaces the first.
pub fn install(b: Arc<dyn PlayerBackend>) {
    *INSTALLED.write().unwrap_or_else(PoisonError::into_inner) = Some(b);
}

/// The installed player, from any thread. `None` before [`install`] - never
/// a wait, so a command that arrives early fails fast instead of hanging.
pub fn backend() -> Option<Arc<dyn PlayerBackend>> {
    INSTALLED
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// Forgets the installed player; [`backend`] answers `None` after it.
pub fn uninstall() {
    *INSTALLED.write().unwrap_or_else(PoisonError::into_inner) = None;
}

/// What the player reported since the last pump. Plain data: it crosses
/// back to the command thread, where the surface's types may not go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    StartFile,
    FileLoaded,
    /// First frame of a new size - the honest "playing" signal. `file-loaded`
    /// fires before a single frame is decoded, so a stream that connects and
    /// then stalls would otherwise look healthy.
    VideoReconfig,
    /// Playback of a file ended.
    EndFile {
        /// mpv's error code, when it called it a failure rather than an
        /// ending.
        error: Option<i32>,
        /// True when it ended only because something else was loaded over it.
        /// The obituary of the *previous* stream arrives after the next one
        /// has already opened, and acting on it would knock a healthy channel
        /// change back to stopped.
        replaced: bool,
    },
    /// mpv's `track-list`, as JSON, after it changed.
    Tracks(String),
    /// mpv's `sub-text/ass`: the subtitle on screen, with its ASS tags,
    /// after it changed. Never logged: it is what is being said.
    SubText(String),
    /// A warning or error mpv logged. Only ever read for the clues in
    /// `ipc::ending`, and never logged or shown: the text can carry the
    /// stream URL, and with it the account.
    Log(String),
}

/// A one-shot reading for the diagnostics panel.
///
/// Every field is optional on purpose: mpv drops most of these between
/// files, and half of them never exist for a raw MPEG-TS multiplex. A
/// missing reading is a dash on screen, not a failure.
#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub container: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub hwdec: Option<String>,
    pub pixel_format: Option<String>,
    pub primaries: Option<String>,
    pub gamma: Option<String>,
    pub video_bitrate: Option<f64>,
    pub audio_bitrate: Option<f64>,
    pub measured_fps: Option<f64>,
    pub container_fps: Option<f64>,
    pub audio_samplerate: Option<i64>,
    pub audio_channels: Option<i64>,
    pub cache_duration: Option<f64>,
    pub cache_speed: Option<f64>,
    pub demuxer_bytes: Option<i64>,
    pub buffering: Option<bool>,
    pub cache_buffering_percent: Option<f64>,
    pub dropped_frames: Option<i64>,
    pub decoder_dropped_frames: Option<i64>,
    pub avsync: Option<f64>,
}

/// A snapshot of the properties the UI shows. Plain data, same reason.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Probe {
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub fps: Option<f64>,
    pub codec: Option<String>,
    /// Seconds between the newest demuxed packet and where playback is.
    pub behind_live: f64,
    /// Seconds of scrollback the demuxer is still holding.
    pub buffered_back: f64,
    pub paused: bool,
    /// The end of mpv's seekable window, in stream time - the furthest
    /// forward a seek can actually land.
    pub live_edge: Option<f64>,
    pub time_pos: Option<f64>,
    /// The newest packet mpv holds, in stream time. Unlike `live_edge` it
    /// moves with every packet read, so it is what tells how fast data is
    /// arriving — and it moves while playback is paused.
    pub cache_end: Option<f64>,
    /// The oldest packet mpv still holds, in stream time: the start of its
    /// seekable window. With `cache_end`, how much stream time has arrived.
    pub cache_start: Option<f64>,
    /// The demuxer has read the end of the stream: the server finished the
    /// response, or the connection failed. Playback carries on through
    /// what is buffered, so this comes well before `end-file`.
    pub demuxer_eof: bool,
    /// Bytes buffered ahead of the playhead.
    pub fw_bytes: Option<i64>,
    /// Bytes the demuxer holds in all, behind the playhead and ahead.
    pub cache_bytes: Option<i64>,
    /// The length of the file, when it has one: a movie's, not a live
    /// stream's, which mpv reports as unknown.
    pub duration: Option<f64>,
}

/// Serialises the tests, here and elsewhere, that install a backend: there
/// is one registry per process and the test harness runs tests in parallel.
#[cfg(test)]
pub(crate) static TEST_REGISTRY: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::fake::FakeBackend;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    fn registry() -> std::sync::MutexGuard<'static, ()> {
        TEST_REGISTRY.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[test]
    fn install_then_backend_returns_it() {
        let _turn = registry();
        let fake = Arc::new(FakeBackend::new());
        install(fake.clone());
        let got = backend().expect("a backend after install");
        got.set_property("volume", "50").unwrap();
        assert_eq!(fake.calls(), vec!["set_property volume=50".to_owned()]);
        uninstall();
    }

    #[test]
    fn uninstall_clears_it() {
        let _turn = registry();
        install(Arc::new(FakeBackend::new()));
        assert!(backend().is_some());
        uninstall();
        assert!(backend().is_none());
    }

    #[test]
    fn concurrent_commands_with_a_ticking_backend() {
        const THREADS: usize = 8;
        const SETS: usize = 1000;
        let _turn = registry();
        let fake = Arc::new(FakeBackend::new());
        install(fake.clone());
        let began = Instant::now();
        let done = Arc::new(AtomicBool::new(false));
        let ticker = {
            let done = done.clone();
            std::thread::spawn(move || {
                let mut pumps = 0;
                while !done.load(Ordering::Acquire) {
                    let p = backend().expect("installed");
                    p.pump();
                    p.probe();
                    pumps += 1;
                    std::thread::sleep(Duration::from_millis(1));
                }
                pumps
            })
        };
        let setters: Vec<_> = (0..THREADS)
            .map(|t| {
                std::thread::spawn(move || {
                    for i in 0..SETS {
                        backend()
                            .expect("installed")
                            .set_property("volume", &format!("{t}-{i}"))
                            .unwrap();
                    }
                })
            })
            .collect();
        for s in setters {
            s.join().expect("a setter panicked");
        }
        done.store(true, Ordering::Release);
        let pumps = ticker.join().expect("the ticker panicked");
        let took = began.elapsed();
        uninstall();

        assert!(took < Duration::from_secs(2), "took {took:?}");
        assert!(pumps > 0);
        let calls = fake.calls();
        let sets = calls.iter().filter(|c| c.starts_with("set_property ")).count();
        assert_eq!(sets, THREADS * SETS);
        assert_eq!(calls.iter().filter(|c| *c == "pump").count(), pumps);
        // Every thread's sets are recorded, each in the order it made them.
        for t in 0..THREADS {
            let mine: Vec<&String> = calls
                .iter()
                .filter(|c| c.starts_with(&format!("set_property volume={t}-")))
                .collect();
            let expected: Vec<String> =
                (0..SETS).map(|i| format!("set_property volume={t}-{i}")).collect();
            assert_eq!(mine.len(), SETS);
            assert!(mine.iter().zip(&expected).all(|(a, b)| *a == b));
        }
    }
}
