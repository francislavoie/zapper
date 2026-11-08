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

extern "C" {
    fn zapper_thumb_decode(path: *const c_char, tw: c_int, th: c_int, rgba: *mut u8) -> c_int;
    fn zapper_thumb_jpeg(rgba: *const u8, w: c_int, h: c_int, out: *mut u8, cap: c_int) -> c_int;
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

/// Bumped by every [`start`] and [`stop`]; a worker runs while it is its own.
static EPOCH: AtomicU64 = AtomicU64::new(0);

/// Off only for the harness, which measures playback with and without it.
static ENABLED: AtomicBool = AtomicBool::new(true);

/// Slots decoded and filed, slots tried and not, and the time it took, for
/// the harness.
static DONE: AtomicU64 = AtomicU64::new(0);
static FAILED: AtomicU64 = AtomicU64::new(0);
static BUSY_US: AtomicU64 = AtomicU64::new(0);
static MAX_US: AtomicU64 = AtomicU64::new(0);

/// Starts filling the open title's previews, ending any worker before it.
/// Called when a movie or an episode opens, after [`trickplay::open_title`].
pub fn start() {
    let epoch = EPOCH.fetch_add(1, Ordering::AcqRel) + 1;
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

/// What the worker has done since the process started: slots filed, slots
/// that gave no picture, and the total and longest time one took
/// (excerpt, decode and compression), in microseconds. For the harness.
pub fn stats() -> (u64, u64, u64, u64) {
    (
        DONE.load(Ordering::Acquire),
        FAILED.load(Ordering::Acquire),
        BUSY_US.load(Ordering::Acquire),
        MAX_US.load(Ordering::Acquire),
    )
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

fn run(epoch: u64) {
    lower_priority();
    let base = std::env::temp_dir().join(format!("zapper-preview-{}-{epoch}", std::process::id()));
    let mut container = 0;
    // Slots tried in this title's store, filed or not: one that gave no
    // picture is not asked for again and again.
    let mut tried: HashSet<i64> = HashSet::new();
    let mut tried_in = trickplay::generation();
    let mut drops: Option<i64> = None;
    let mut wait = PACE;
    loop {
        std::thread::sleep(wait);
        wait = PACE;
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
        if generation != tried_in {
            tried.clear();
            tried_in = generation;
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
        let Some(spacing) = trickplay::settle_title_spacing(state.duration) else {
            // No title open any more: whatever opened next said so.
            break;
        };
        let Some(filled) = trickplay::title_slots(generation) else {
            wait = IDLE;
            continue;
        };
        let next = trickplay::next_slot(
            &state.ranges,
            state.position,
            spacing,
            state.duration,
            |k| filled.contains(&k) || tried.contains(&k),
        );
        let Some(slot) = next else {
            wait = IDLE;
            continue;
        };
        tried.insert(slot);
        fill(mpv, generation, slot as f64 * spacing, &base, &mut container);
    }
    for ext in CONTAINERS {
        let _ = std::fs::remove_file(base.with_extension(ext));
    }
}

/// The excerpt's containers, in the order they are tried: see [`fill`].
const CONTAINERS: [&str; 2] = ["mkv", "ts"];

/// Reads, decodes and files the picture for the slot at `t`. The excerpt
/// is `base` with the extension of `CONTAINERS[*container]`, or a later one
/// when mpv cannot write that.
fn fill(mpv: &Mpv, generation: u64, t: f64, base: &Path, container: &mut usize) {
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
    let filed = dumped
        .as_deref()
        .and_then(decode)
        .and_then(|rgba| jpeg(&rgba))
        // Filed under the slot's moment. The picture is the keyframe at or
        // before it, so up to one group of pictures earlier (two to five
        // seconds in most films): mpv writes whole groups, and the excerpt
        // does not say where in its group the moment was.
        .is_some_and(|jpeg| trickplay::record_title(generation, t, jpeg));
    if let Some(file) = &dumped {
        let _ = std::fs::remove_file(file);
    }
    let us = began.elapsed().as_micros() as u64;
    tracing::debug!("previews: {t}s {} in {}us", if filed { "filed" } else { "not filed" }, us);
    BUSY_US.fetch_add(us, Ordering::AcqRel);
    MAX_US.fetch_max(us, Ordering::AcqRel);
    if filed {
        DONE.fetch_add(1, Ordering::AcqRel);
    } else {
        FAILED.fetch_add(1, Ordering::AcqRel);
    }
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

/// The first picture of the video in `file`, as a preview's RGBA.
fn decode(file: &Path) -> Option<Vec<u8>> {
    let path = CString::new(file.to_string_lossy().as_bytes()).ok()?;
    let mut rgba = vec![0u8; (THUMB_W * THUMB_H * 4) as usize];
    // SAFETY: `path` is NUL-terminated and outlives the call; `rgba` holds
    // exactly THUMB_W x THUMB_H x 4 bytes, which is what the C side writes.
    let r = unsafe { zapper_thumb_decode(path.as_ptr(), THUMB_W, THUMB_H, rgba.as_mut_ptr()) };
    (r == 0).then_some(rgba)
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
