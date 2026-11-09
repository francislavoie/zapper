//! Previews of what a movie or an episode has buffered, for its seek bar.
//!
//! The frames drawn while a title plays cover what has been watched
//! ([`super::trickplay`]). This covers what mpv has read but not shown: the
//! stretch ahead of the playhead, and anything behind it that was buffered
//! and skipped over. One worker thread walks those ranges a slot at a time
//! ([`trickplay::next_slot`]); for each empty slot it asks mpv for a tenth
//! of a second of its own cache (`dump-cache`, no new connection, no new
//! read of the provider's file), decodes the first picture of that excerpt
//! (`thumbframe.c`) and files it, compressed, under the moment it shows.
//!
//! It gives way to playback rather than competing with it: one slot at a
//! time with a pause between, at a lower priority, and nothing at all while
//! mpv is seeking, waiting for data, short of buffered data ahead, or has
//! just dropped a frame. Each excerpt is a temporary file, deleted as soon
//! as it is read.

use std::collections::HashSet;
use std::ffi::{c_char, c_int, CString};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use libmpv2::Mpv;

use super::trickplay::{self, DUMP_SPAN_S, THUMB_H, THUMB_W};

#[cfg(not(zapper_no_thumbframe))]
extern "C" {
    fn zapper_thumb_decode(
        path: *const c_char,
        tw: c_int,
        th: c_int,
        rgba: *mut u8,
        key: *mut u64,
        span: *mut f64,
    ) -> c_int;
    fn zapper_thumb_jpeg(rgba: *const u8, w: c_int, h: c_int, out: *mut u8, cap: c_int) -> c_int;
}

// A build without FFmpeg's development files (build.rs says so): nothing is
// decoded or compressed, and the frames drawn while a title plays stay as
// they were taken.
#[cfg(zapper_no_thumbframe)]
unsafe fn zapper_thumb_decode(
    _: *const c_char,
    _: c_int,
    _: c_int,
    _: *mut u8,
    _: *mut u64,
    _: *mut f64,
) -> c_int {
    -1
}
#[cfg(zapper_no_thumbframe)]
unsafe fn zapper_thumb_jpeg(_: *const u8, _: c_int, _: c_int, _: *mut u8, _: c_int) -> c_int {
    -1
}

/// The pause between two slots. A 1080p H.264 keyframe takes 10–15ms to
/// decode on one thread, so this keeps the worker to a few percent of one
/// core while it has anything to do.
const PACE: Duration = Duration::from_millis(250);

/// The pause when there is nothing to fill, or playback needs the machine.
const IDLE: Duration = Duration::from_secs(1);

/// The pause after playback dropped a frame: whatever caused it, this is
/// not the moment to add work.
const BACK_OFF: Duration = Duration::from_secs(3);

/// Less than this buffered ahead of the playhead (and the file not all
/// read) is a cache that is starving: the network needs every chance.
const MIN_AHEAD_S: f64 = 5.0;

/// What the worker may write, on average: each excerpt is a whole group of
/// pictures, which for a film encoded with long ones at a high bitrate is
/// tens of megabytes, so the pause after one is as long as this budget says.
const BYTES_PER_SEC: f64 = 4.0 * 1024.0 * 1024.0;

/// An excerpt bigger than this makes the worker try fewer slots: one in
/// as many as the excerpt is times this, one in [`MAX_STRIDE`] at most.
const EXCERPT_CAP: u64 = 16 * 1024 * 1024;
const MAX_STRIDE: i64 = 8;

/// The excerpts' folder, under the cache, and what their names begin with.
const EXCERPT_DIR: &str = "previews";
const EXCERPT_PREFIX: &str = "preview-";

/// The pause after a slot whose excerpt was `bytes` and which took `busy`:
/// [`PACE`], or longer to keep to [`BYTES_PER_SEC`], or longer still to keep
/// to [`CPU_SHARE`] of one core, whichever is longest. The time is the
/// slot's own, so a 4K keyframe on a busy machine waits far longer than a
/// 1080p one on an idle machine, whatever the film.
fn pause_after(bytes: u64, busy: Duration) -> Duration {
    let for_bytes = Duration::from_secs_f64(bytes as f64 / BYTES_PER_SEC);
    let for_time = busy.mul_f64((1.0 - CPU_SHARE) / CPU_SHARE);
    PACE.max(for_bytes).max(for_time)
}

/// The share of one core the worker keeps to, on average: each slot's time
/// is followed by a pause nineteen times as long.
const CPU_SHARE: f64 = 0.05;

/// How many slots apart to try, after an excerpt of `bytes`.
fn stride_after(stride: i64, bytes: u64) -> i64 {
    // As many slots apart as the excerpt is times the cap, rounded up.
    let wanted = bytes.div_ceil(EXCERPT_CAP).min(MAX_STRIDE as u64) as i64;
    stride.max(wanted).clamp(1, MAX_STRIDE)
}

/// Removes excerpts older than `age` from `dir`: left by a run that ended
/// mid-way. A live worker's excerpt exists for milliseconds. Returns how
/// many went.
fn sweep(dir: &Path, age: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let now = std::time::SystemTime::now();
    entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(EXCERPT_PREFIX))
        .filter(|e| {
            e.metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| now.duration_since(t).unwrap_or_default() >= age)
        })
        .filter(|e| std::fs::remove_file(e.path()).is_ok())
        .count()
}

/// A name for an excerpt that another program cannot guess: 64 bits from
/// the standard library's randomly keyed hasher, which the system seeds.
fn excerpt_name() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(Instant::now().elapsed().as_nanos());
    format!("{EXCERPT_PREFIX}{:016x}", h.finish())
}

/// Bumped by every [`start`] and [`stop`]; a worker runs while it is its own.
static EPOCH: AtomicU64 = AtomicU64::new(0);

/// Off only for the harness, which measures playback with and without it.
static ENABLED: AtomicBool = AtomicBool::new(true);

/// What the worker has done since the process started, for the harness.
static FILED: AtomicU64 = AtomicU64::new(0);
static FAILED: AtomicU64 = AtomicU64::new(0);
static REPEATS: AtomicU64 = AtomicU64::new(0);
static BUSY_US: AtomicU64 = AtomicU64::new(0);
static MAX_US: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
static MAX_BYTES: AtomicU64 = AtomicU64::new(0);
static CPU_US: AtomicU64 = AtomicU64::new(0);
static STRIDE: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);

/// Whether this build decodes previews at all: one built without FFmpeg's
/// development files (see build.rs) keeps the frames drawn while a title
/// plays, and nothing else.
pub const DECODES: bool = !cfg!(zapper_no_thumbframe);

/// Starts filling the open title's previews, ending any worker before it.
/// Called when a movie or an episode opens, after [`trickplay::open_title`].
pub fn start() {
    let epoch = EPOCH.fetch_add(1, Ordering::AcqRel) + 1;
    if !DECODES {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("title-previews".into())
        .spawn(move || run(epoch));
    if let Err(e) = spawned {
        tracing::warn!("no previews of what is buffered: {e}");
    }
}

/// Ends the worker at its next step. Called when anything else opens, or
/// playback stops.
pub fn stop() {
    EPOCH.fetch_add(1, Ordering::AcqRel);
}

/// Pauses or resumes the worker (it keeps its place). For the harness.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Release);
}

/// What the worker has done since the process started. For the harness.
#[derive(Debug, Clone, Copy, Default)]
pub struct Stats {
    /// Slots given a picture.
    pub filed: u64,
    /// Slots that gave none (no excerpt, or nothing to decode).
    pub failed: u64,
    /// Slots whose excerpt began at a keyframe already filed.
    pub repeats: u64,
    /// Time spent on slots (excerpt, decode, compression), and the longest.
    pub busy_us: u64,
    pub longest_us: u64,
    /// Excerpt bytes written, and the largest one.
    pub bytes: u64,
    pub largest: u64,
    /// The worker threads' processor time (Linux; 0 elsewhere).
    pub cpu_us: u64,
    /// Every how many slots the latest worker tries.
    pub stride: i64,
}

pub fn stats() -> Stats {
    let get = |a: &AtomicU64| a.load(Ordering::Acquire);
    Stats {
        filed: get(&FILED),
        failed: get(&FAILED),
        repeats: get(&REPEATS),
        busy_us: get(&BUSY_US),
        longest_us: get(&MAX_US),
        bytes: get(&BYTES),
        largest: get(&MAX_BYTES),
        cpu_us: get(&CPU_US),
        stride: STRIDE.load(Ordering::Acquire),
    }
}

/// What mpv says about playback, read once per step.
struct State {
    ranges: Vec<(f64, f64)>,
    position: f64,
    duration: Option<f64>,
    /// Playback can spare the machine: not seeking, not waiting for data,
    /// and enough buffered ahead (or the whole file read).
    calm: bool,
    drops: i64,
}

impl State {
    /// The title's length, or as much of it as is known: a stream that does
    /// not say how long it is has at least what has been buffered.
    fn length(&self) -> f64 {
        self.duration.unwrap_or_else(|| {
            self.ranges
                .iter()
                .map(|r| r.1)
                .fold(self.position, f64::max)
        })
    }
}

fn read_state(mpv: &Mpv) -> Option<State> {
    let position = mpv.get_property::<f64>("time-pos").ok()?;
    let raw = mpv.get_property::<String>("demuxer-cache-state").ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let ranges: Vec<(f64, f64)> = v
        .get("seekable-ranges")
        .and_then(|r| r.as_array())
        .map(|rs| {
            rs.iter()
                .filter_map(|r| Some((r.get("start")?.as_f64()?, r.get("end")?.as_f64()?)))
                .collect()
        })
        .unwrap_or_default();
    let eof = v.get("eof").and_then(|x| x.as_bool()).unwrap_or(false);
    let ahead = v
        .get("cache-end")
        .and_then(|x| x.as_f64())
        .map_or(0.0, |end| end - position);
    let flag = |k: &str| mpv.get_property::<bool>(k).unwrap_or(false);
    let calm = !flag("seeking") && !flag("paused-for-cache") && (eof || ahead >= MIN_AHEAD_S);
    let count = |k: &str| mpv.get_property::<i64>(k).unwrap_or(0);
    Some(State {
        ranges,
        position,
        duration: mpv.get_property::<f64>("duration").ok().filter(|d| *d > 0.0),
        calm,
        drops: count("frame-drop-count") + count("decoder-frame-drop-count"),
    })
}

/// Where the excerpts go, under the cache rather than the shared temporary
/// directory. Once a process, excerpts an earlier run left there go.
fn excerpt_dir() -> std::path::PathBuf {
    static SWEPT: std::sync::Once = std::sync::Once::new();
    let dir = crate::paths::cache_dir().join(EXCERPT_DIR);
    SWEPT.call_once(|| {
        let _ = std::fs::create_dir_all(&dir);
        let gone = sweep(&dir, Duration::from_secs(60));
        if gone > 0 {
            tracing::info!("removed {gone} preview excerpts an earlier run left");
        }
    });
    dir
}

/// What the worker knows of the title it is filling, reset when the store
/// starts afresh.
struct Walk {
    generation: u64,
    spacing: f64,
    /// Slots tried at this spacing, filed or not: one that gave no picture
    /// is not asked for again and again.
    tried: HashSet<i64>,
    /// Keyframes already filed: a slot whose excerpt starts at one of them
    /// (a group of pictures longer than a slot) adds nothing.
    seen: HashSet<u64>,
    /// Every how many slots are tried ([`stride_after`]).
    stride: i64,
}

impl Walk {
    fn new(generation: u64) -> Self {
        Self {
            generation,
            spacing: 0.0,
            tried: HashSet::new(),
            seen: HashSet::new(),
            stride: 1,
        }
    }
}

fn run(epoch: u64) {
    lower_priority();
    let base = excerpt_dir().join(excerpt_name());
    let mut container = 0;
    let mut walk = Walk::new(trickplay::generation());
    let mut drops: Option<i64> = None;
    let mut cpu = thread_cpu_us();
    let mut wait = PACE;
    loop {
        std::thread::sleep(wait);
        let now = thread_cpu_us();
        CPU_US.fetch_add(now.saturating_sub(cpu), Ordering::AcqRel);
        cpu = now;
        if EPOCH.load(Ordering::Acquire) != epoch {
            break;
        }
        if !ENABLED.load(Ordering::Acquire) {
            wait = IDLE;
            continue;
        }
        let Some(mpv) = super::mpv::handle() else {
            wait = IDLE;
            continue;
        };
        let generation = trickplay::generation();
        if generation != walk.generation {
            walk = Walk::new(generation);
            STRIDE.store(1, Ordering::Release);
        }
        compress_one();
        let Some(state) = read_state(mpv) else {
            wait = IDLE;
            continue;
        };
        let dropped = drops.is_some_and(|d| state.drops > d);
        drops = Some(state.drops);
        if dropped {
            tracing::trace!("previews: playback dropped a frame; waiting");
            wait = BACK_OFF;
            continue;
        }
        if !state.calm {
            tracing::trace!("previews: playback is busy; waiting");
            wait = IDLE;
            continue;
        }
        let Some(spacing) = trickplay::settle_title_spacing(Some(state.length())) else {
            // No title open any more: whatever opened next said so.
            break;
        };
        if spacing != walk.spacing {
            // Other slots now: what was tried at the old spacing says
            // nothing about them.
            walk.tried.clear();
            walk.spacing = spacing;
        }
        let Some(filled) = trickplay::title_slots(generation) else {
            wait = IDLE;
            continue;
        };
        let next = trickplay::next_slot(
            &state.ranges,
            state.position,
            spacing,
            state.duration,
            |k| filled.contains(&k) || walk.tried.contains(&k) || k % walk.stride != 0,
        );
        let Some(slot) = next else {
            wait = IDLE;
            continue;
        };
        walk.tried.insert(slot);
        let (bytes, busy) = fill(mpv, &mut walk, slot as f64 * spacing, &base, &mut container);
        walk.stride = stride_after(walk.stride, bytes);
        STRIDE.store(walk.stride, Ordering::Release);
        wait = pause_after(bytes, busy);
    }
    for ext in CONTAINERS {
        let _ = std::fs::remove_file(base.with_extension(ext));
    }
}

/// The excerpt's containers, in the order they are tried: see [`fill`].
const CONTAINERS: [&str; 2] = ["mkv", "ts"];

/// Reads, decodes and files the picture for the slot at `t`, and returns
/// how many bytes the excerpt was and how long the slot took. The excerpt
/// is `base` with the extension
/// of `CONTAINERS[*container]`, or a later one when mpv cannot write that.
fn fill(mpv: &Mpv, walk: &mut Walk, t: f64, base: &Path, container: &mut usize) -> (u64, Duration) {
    let began = Instant::now();
    // mpv writes the title's selected tracks, and picks the container from
    // the name. Matroska takes any video, but not every subtitle format (an
    // MP4's own, for one); MPEG-TS takes any track, as private data if need
    // be. Once Matroska is refused, this title's excerpts are MPEG-TS.
    let mut dumped = None;
    for (i, ext) in CONTAINERS.iter().enumerate().skip(*container) {
        let file = base.with_extension(ext);
        match super::mpv::dump_cache(mpv, t, t + DUMP_SPAN_S, &file) {
            Ok(()) => {
                *container = i;
                dumped = Some(file);
                break;
            }
            Err(e) => {
                let _ = std::fs::remove_file(&file);
                tracing::debug!("previews: no .{ext} excerpt at {t}s: {e:#}");
            }
        }
    }
    let bytes = dumped
        .as_deref()
        .and_then(|f| std::fs::metadata(f).ok())
        .map_or(0, |m| m.len());
    let written = began.elapsed().as_micros();
    let picture = dumped.as_deref().and_then(decode);
    if let Some(file) = &dumped {
        let _ = std::fs::remove_file(file);
    }
    let outcome = match picture {
        None => &FAILED,
        Some(p) if walk.seen.contains(&p.key) => &REPEATS,
        Some(p) => {
            // Filed under the slot's moment. The picture is the keyframe at
            // or before it, so up to one group of pictures earlier: mpv
            // writes an excerpt's timestamps from zero, so it cannot say
            // where in its group the moment was.
            let filed = jpeg(&p.rgba)
                .is_some_and(|j| trickplay::record_title(walk.generation, t, j));
            if filed {
                walk.seen.insert(p.key);
                &FILED
            } else {
                &FAILED
            }
        }
    };
    outcome.fetch_add(1, Ordering::AcqRel);
    let us = began.elapsed().as_micros() as u64;
    tracing::debug!("previews: {t}s, {bytes} bytes, {us}us ({written}us of it the excerpt)");
    BUSY_US.fetch_add(us, Ordering::AcqRel);
    MAX_US.fetch_max(us, Ordering::AcqRel);
    BYTES.fetch_add(bytes, Ordering::AcqRel);
    MAX_BYTES.fetch_max(bytes, Ordering::AcqRel);
    (bytes, began.elapsed())
}

/// This thread's processor time so far, in microseconds (Linux only).
fn thread_cpu_us() -> u64 {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: getrusage fills the struct it is given and nothing else.
        let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
        if unsafe { libc::getrusage(libc::RUSAGE_THREAD, &mut ru) } == 0 {
            let us = |t: libc::timeval| t.tv_sec as u64 * 1_000_000 + t.tv_usec as u64;
            return us(ru.ru_utime) + us(ru.ru_stime);
        }
    }
    0
}

/// Compresses one frame drawn while the title played, which the render
/// thread files as it was taken.
fn compress_one() {
    if let Some((generation, at, rgba)) = trickplay::raw_title_frame() {
        if let Some(jpeg) = jpeg(&rgba) {
            trickplay::compress_title_frame(generation, at, jpeg);
        }
    }
}

/// The first picture of an excerpt, and what is known of where it came from.
struct Picture {
    /// The preview's RGBA.
    rgba: Vec<u8>,
    /// The keyframe's fingerprint: the same keyframe in two excerpts is the
    /// same picture, whatever slot asked for it.
    key: u64,
}

/// The first picture of the video in `file`.
fn decode(file: &Path) -> Option<Picture> {
    let path = CString::new(file.to_string_lossy().as_bytes()).ok()?;
    let mut rgba = vec![0u8; (THUMB_W * THUMB_H * 4) as usize];
    let (mut key, mut span) = (0u64, f64::NAN);
    // SAFETY: `path` is NUL-terminated and outlives the call; `rgba` holds
    // exactly THUMB_W x THUMB_H x 4 bytes, which is what the C side writes;
    // `key` and `span` are plain values it writes once each.
    let r = unsafe {
        zapper_thumb_decode(path.as_ptr(), THUMB_W, THUMB_H, rgba.as_mut_ptr(), &mut key, &mut span)
    };
    tracing::trace!("previews: picture {key:016x}, {span:.2}s of video after it");
    (r == 0).then_some(Picture { rgba, key })
}

/// A preview's RGBA as a JPEG.
pub fn jpeg(rgba: &[u8]) -> Option<Vec<u8>> {
    if rgba.len() != (THUMB_W * THUMB_H * 4) as usize {
        return None;
    }
    // A 160x90 JPEG at this quality is a few kilobytes; the RGBA itself is
    // a generous ceiling.
    let mut out = vec![0u8; rgba.len()];
    // SAFETY: `rgba` is THUMB_W x THUMB_H x 4 bytes as checked above, and
    // `out` is `cap` bytes long; the C side writes at most `cap`.
    let n = unsafe {
        zapper_thumb_jpeg(
            rgba.as_ptr(),
            THUMB_W,
            THUMB_H,
            out.as_mut_ptr(),
            out.len() as c_int,
        )
    };
    let n = usize::try_from(n).ok().filter(|n| *n > 0)?;
    out.truncate(n);
    Some(out)
}

/// The worker below playback in the scheduler's eyes. Linux only: a nice
/// value is per thread there, and elsewhere the pacing does the work.
fn lower_priority() {
    #[cfg(target_os = "linux")]
    // SAFETY: plain syscalls on this thread's own id.
    unsafe {
        let tid = libc::gettid();
        libc::setpriority(libc::PRIO_PROCESS, tid as libc::id_t, 10);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_big_excerpt_is_followed_by_a_longer_pause() {
        let quick = Duration::from_millis(5);
        assert_eq!(pause_after(0, quick), PACE);
        assert_eq!(pause_after(100_000, quick), PACE);
        // 20MB at 4MB a second: five seconds before the next.
        let p = pause_after(20 * 1024 * 1024, quick);
        assert!((p.as_secs_f64() - 5.0).abs() < 0.01, "{p:?}");
    }

    #[test]
    fn a_slow_slot_is_followed_by_a_pause_that_keeps_the_worker_to_a_twentieth_of_a_core() {
        // A small excerpt that took 1.4s to decode (a 4K keyframe on a
        // busy machine): 19 times that before the next, so that slot and
        // its pause together are one part work in twenty.
        let busy = Duration::from_millis(1400);
        let p = pause_after(1024, busy);
        assert!((p.as_secs_f64() - 26.6).abs() < 0.01, "{p:?}");
        let share = busy.as_secs_f64() / (busy + p).as_secs_f64();
        assert!(share <= 0.05 + 1e-9, "{share}");
        // Whichever is longer: the bytes' pause or the time's.
        let p = pause_after(200 * 1024 * 1024, Duration::from_millis(100));
        assert!((p.as_secs_f64() - 50.0).abs() < 0.01, "{p:?}");
    }

    #[test]
    fn an_excerpt_over_the_cap_spaces_out_the_slots_tried() {
        assert_eq!(stride_after(1, 1024), 1);
        assert_eq!(stride_after(1, EXCERPT_CAP + 1), 2);
        // As many times the cap, as many slots apart: the same size again
        // does not widen it further.
        assert_eq!(stride_after(2, EXCERPT_CAP + 1), 2);
        assert_eq!(stride_after(2, EXCERPT_CAP * 3), 3);
        assert_eq!(stride_after(1, EXCERPT_CAP * 100), MAX_STRIDE);
        // A small one after does not narrow it again: the film is the same.
        assert_eq!(stride_after(4, 1024), 4);
    }

    #[test]
    fn excerpts_left_behind_are_swept_and_nothing_else() {
        let dir = std::env::temp_dir().join(format!("zapper-sweep-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{EXCERPT_PREFIX}old.mkv")), b"x").unwrap();
        std::fs::write(dir.join("unrelated.txt"), b"x").unwrap();
        // Everything counts as old with no age at all.
        assert_eq!(sweep(&dir, Duration::ZERO), 1);
        assert!(!dir.join(format!("{EXCERPT_PREFIX}old.mkv")).exists());
        assert!(dir.join("unrelated.txt").exists());
        // A fresh one is a live worker's: kept.
        std::fs::write(dir.join(format!("{EXCERPT_PREFIX}new.mkv")), b"x").unwrap();
        assert_eq!(sweep(&dir, Duration::from_secs(60)), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_excerpts_name_cannot_be_guessed() {
        let (a, b) = (excerpt_name(), excerpt_name());
        assert!(a.starts_with(EXCERPT_PREFIX));
        assert_ne!(a, b);
        assert!(!a.contains(&std::process::id().to_string()), "{a}");
    }

    #[test]
    #[cfg_attr(zapper_no_thumbframe, ignore = "built without FFmpeg's development files")]
    fn a_preview_compresses_to_a_few_kilobytes() {
        // A gradient, which is about as busy as a frame at this size.
        let mut rgba = vec![0u8; (THUMB_W * THUMB_H * 4) as usize];
        for (i, px) in rgba.chunks_mut(4).enumerate() {
            let (x, y) = (i % THUMB_W as usize, i / THUMB_W as usize);
            px.copy_from_slice(&[(x * 255 / 160) as u8, (y * 255 / 90) as u8, 128, 255]);
        }
        let j = jpeg(&rgba).expect("FFmpeg's JPEG encoder");
        assert_eq!(&j[..2], &[0xFF, 0xD8], "a JPEG's start of image");
        assert!(j.len() < 12_000, "{} bytes", j.len());
        assert!(jpeg(&[0u8; 4]).is_none());
    }

    #[test]
    fn a_file_that_is_not_video_gives_no_picture() {
        let p = std::env::temp_dir().join(format!("zapper-preview-test-{}.mkv", std::process::id()));
        std::fs::write(&p, b"not a video").unwrap();
        assert!(decode(&p).is_none());
        let _ = std::fs::remove_file(&p);
        assert!(decode(Path::new("/nonexistent/zapper-preview.mkv")).is_none());
    }
}
