//! Scrollback thumbnails: what the picture looked like, a while ago.
//!
//! Scrubbing a live buffer by clock time alone is guesswork — "two minutes
//! back" does not tell you whether that is before or after the thing you are
//! looking for. These are the small images the scrubber shows under the
//! cursor so it does.
//!
//! **Nothing decodes anything for these.** That is the entire reason they are
//! affordable. A thumbnail is a copy of a frame that has already been decoded
//! for the screen. There is no second decoder, no second connection to the
//! server — which matters, because many accounts allow only one — and no
//! re-reading of the demuxer cache, which mpv does not expose anyway.
//!
//! This file is the portable half: the store, the clock that says when the
//! next one is due, and the encoding the scrubber receives. Taking the copy
//! is the platform's job, because it depends on how the frame was drawn:
//!
//! - Linux (`surface::linux::thumbs`): the frame is on the GPU in GTK's GL
//!   framebuffer, so it is blitted down to 160x90 there and only that is read
//!   back — 57KB, where reading a fullscreen surface back to shrink it on the
//!   CPU would move about 14MB and stall the pipeline.
//! - Windows (`surface::windows`): mpv draws into its own window, so there is
//!   no framebuffer of ours to read. mpv's `screenshot-raw` hands over the
//!   current frame instead, off the main thread, and [`shrink_bgr0`] reduces
//!   it here.
//!
//! The store is a plain mutex rather than a thread-local, because the capture
//! happens on one thread and the lookup answers an IPC command on another.
//! The data is bytes, so it can simply move; a write every few seconds
//! contends with nothing.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;


/// Thumbnail size. Small on purpose: it answers "roughly what was on screen",
/// and at scrubber size nothing larger is legible anyway. 16:9 because that
/// is what essentially every channel is, and a non-matching source is letter-
/// boxed by mpv before it ever reaches the surface.
pub const THUMB_W: i32 = 160;
pub const THUMB_H: i32 = 90;

/// Seconds of stream between thumbnails.
///
/// Four is a compromise measured against the thing being looked for: a goal,
/// a wicket, the start of an item. Closer spacing costs memory linearly for a
/// precision the scrubber cannot show, and wider starts missing events.
const INTERVAL_S: f64 = 4.0;

/// How far off a request may land before it is not worth answering. Half the
/// interval: beyond that there is a nearer thumbnail that simply has not been
/// taken, and showing the wrong moment confidently is worse than showing none.
const TOLERANCE_S: f64 = INTERVAL_S;

/// Ceiling on the number kept, so a long rewind budget cannot grow without
/// bound. At 160x90 RGBA one is 57KB, so this is about 13MB — the same order
/// as the demuxer's own back buffer, which is what these describe.
const MAX_THUMBS: usize = 240;

struct Thumb {
    /// mpv's `playback-time` when it was taken. Stream time, not wall clock:
    /// it is the only clock that means the same thing to a later seek.
    at: f64,
    /// RGBA, top-down, `THUMB_W * THUMB_H * 4` bytes.
    rgba: Vec<u8>,
}

#[derive(Default)]
struct Store {
    /// Sorted by `at`, because that is how it is searched and how it is
    /// pruned, and keeping it sorted on insert is cheaper than either.
    thumbs: Vec<Thumb>,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);

/// Bumped by every [`clear`], under the store's lock.
///
/// A capture that runs off the main thread (Windows takes its screenshot on
/// a worker) can finish after the channel has changed, and would otherwise
/// file the previous channel's picture into the new channel's buffer. It
/// carries the generation it started in and is dropped if that has passed.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Wall-clock gate for the capture, so the common path costs one comparison.
///
/// Reading mpv's clock is cheap but not free, and the render handler runs
/// sixty times a second; this keeps the property read down to the handful of
/// frames that might actually produce a thumbnail.
fn due(last: &mut Option<Instant>) -> bool {
    let now = Instant::now();
    match last {
        Some(t) if now.duration_since(*t).as_secs_f64() < INTERVAL_S => false,
        _ => {
            *last = Some(now);
            true
        }
    }
}

thread_local! {
    static LAST: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// Files a thumbnail taken at `at` (mpv's `playback-time`). `rgba` is
/// [`THUMB_W`] x [`THUMB_H`], top-down; anything else is dropped rather than
/// encoded wrong.
pub fn record(at: f64, rgba: Vec<u8>) {
    record_for(generation(), at, rgba);
}

/// The current generation, for a capture that will finish later. See
/// [`GENERATION`].
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

/// [`record`], for a capture begun in `generation`: dropped if the store has
/// been cleared since.
pub fn record_for(generation: u64, at: f64, rgba: Vec<u8>) {
    if !at.is_finite() || rgba.len() != (THUMB_W * THUMB_H * 4) as usize {
        return;
    }
    insert(generation, Thumb { at, rgba });
}

fn insert(generation: u64, thumb: Thumb) {
    let Ok(mut guard) = STORE.lock() else { return };
    if GENERATION.load(Ordering::Acquire) != generation {
        return;
    }
    let store = guard.get_or_insert_with(Store::default);

    // Rewatching a stretch that is already covered must not stack duplicates
    // on top of it, and a seek backwards arrives out of order.
    let pos = store
        .thumbs
        .partition_point(|t| t.at < thumb.at);
    let near = |i: usize| {
        store
            .thumbs
            .get(i)
            .is_some_and(|t| (t.at - thumb.at).abs() < INTERVAL_S / 2.0)
    };
    if (pos > 0 && near(pos - 1)) || near(pos) {
        return;
    }
    store.thumbs.insert(pos, thumb);

    // Oldest first: the buffer they describe is pruned from that end too.
    if store.thumbs.len() > MAX_THUMBS {
        let excess = store.thumbs.len() - MAX_THUMBS;
        store.thumbs.drain(..excess);
    }
}

/// The thumbnail nearest `at` in stream time, as PNG bytes, or `None` when
/// nothing was captured close enough to be honest about.
pub fn nearest_png(at: f64) -> Option<(f64, Vec<u8>)> {
    let guard = STORE.lock().ok()?;
    let store = guard.as_ref()?;
    if store.thumbs.is_empty() || !at.is_finite() {
        return None;
    }
    let pos = store.thumbs.partition_point(|t| t.at < at);
    let best = [pos.checked_sub(1), Some(pos)]
        .into_iter()
        .flatten()
        .filter_map(|i| store.thumbs.get(i))
        .min_by(|a, b| {
            (a.at - at)
                .abs()
                .partial_cmp(&(b.at - at).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })?;
    if (best.at - at).abs() > TOLERANCE_S {
        return None;
    }
    Some((
        best.at,
        crate::surface::png::png_bytes(THUMB_W as u32, THUMB_H as u32, &best.rgba),
    ))
}

/// Drops everything. Called when the stream changes: the thumbnails describe
/// a buffer that no longer exists, and showing the previous channel's picture
/// under the cursor would be worse than showing none.
pub fn clear() {
    if let Ok(mut guard) = STORE.lock() {
        GENERATION.fetch_add(1, Ordering::AcqRel);
        *guard = None;
    }
    LAST.with(|l| l.set(None));
}

/// How many are held. For the harness, which otherwise cannot tell a working
/// capture from one that silently never runs.
pub fn count() -> usize {
    STORE
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|s| s.thumbs.len()))
        .unwrap_or(0)
}

/// Whether a capture is due. Public so the render handler can skip reading
/// mpv's clock on the frames that cannot produce one.
pub fn is_due() -> bool {
    LAST.with(|l| {
        let mut last = l.get();
        let yes = due(&mut last);
        l.set(last);
        yes
    })
}

/// Reduces a `bgr0` frame (mpv's `screenshot-raw` default: B, G, R, padding)
/// to a [`THUMB_W`] x [`THUMB_H`] RGBA thumbnail, letterboxed to keep its
/// shape.
///
/// Each thumbnail pixel is the average of the source pixels it covers, not a
/// sample of one of them: point sampling a 1080p frame down to 160 columns
/// keeps one pixel in twelve and turns text and fine edges into noise.
/// `None` for a frame too small or a buffer too short to be what it claims.
pub fn shrink_bgr0(src: &[u8], w: usize, h: usize, stride: usize) -> Option<Vec<u8>> {
    if w == 0 || h == 0 || stride < w * 4 || src.len() < stride * (h - 1) + w * 4 {
        return None;
    }
    let (tw, th) = (THUMB_W as usize, THUMB_H as usize);
    // Fit inside the thumbnail, centred, like mpv letterboxes on screen.
    let (fw, fh) = if w * th >= h * tw {
        (tw, (h * tw / w).clamp(1, th))
    } else {
        ((w * th / h).clamp(1, tw), th)
    };
    let (ox, oy) = ((tw - fw) / 2, (th - fh) / 2);

    let mut out = vec![0u8; tw * th * 4];
    for px in out.as_chunks_mut::<4>().0 {
        px[3] = 255;
    }
    for ty in 0..fh {
        let (y0, y1) = (ty * h / fh, ((ty + 1) * h / fh).max(ty * h / fh + 1));
        for tx in 0..fw {
            let (x0, x1) = (tx * w / fw, ((tx + 1) * w / fw).max(tx * w / fw + 1));
            let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
            for y in y0..y1.min(h) {
                let row = &src[y * stride..];
                for x in x0..x1.min(w) {
                    b += u32::from(row[x * 4]);
                    g += u32::from(row[x * 4 + 1]);
                    r += u32::from(row[x * 4 + 2]);
                }
            }
            let n = ((y1.min(h) - y0) * (x1.min(w) - x0)).max(1) as u32;
            let o = ((oy + ty) * tw + ox + tx) * 4;
            out[o] = (r / n) as u8;
            out[o + 1] = (g / n) as u8;
            out[o + 2] = (b / n) as u8;
        }
    }
    Some(out)
}

/// Base64, for a `data:` URL. Written out rather than pulled in: it is
/// sixteen lines, and the alternative is shipping a `Vec<u8>` through JSON as
/// an array of numbers, which costs about four bytes per byte.
pub fn base64(data: &[u8]) -> String {
    const SET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(SET[(n >> 18) as usize & 63] as char);
        out.push(SET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            SET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            SET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_rfc_examples() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn a_frame_shrinks_to_the_thumbnail_size_in_rgba() {
        // A 16:9 frame, pure blue in bgr0: B first.
        let (w, h) = (320usize, 180usize);
        let src: Vec<u8> = (0..w * h).flat_map(|_| [200u8, 0, 0, 0]).collect();
        let out = shrink_bgr0(&src, w, h, w * 4).unwrap();
        assert_eq!(out.len(), (THUMB_W * THUMB_H * 4) as usize);
        assert_eq!(&out[..4], &[0, 0, 200, 255], "bgr0 must come out as rgba");
    }

    #[test]
    fn a_square_frame_is_pillarboxed_rather_than_stretched() {
        let (w, h) = (90usize, 90usize);
        let src = vec![255u8; w * h * 4];
        let out = shrink_bgr0(&src, w, h, w * 4).unwrap();
        let at = |x: usize, y: usize| out[(y * THUMB_W as usize + x) * 4];
        assert_eq!(at(0, 45), 0, "the left bar is black");
        assert_eq!(at(80, 45), 255, "the picture is in the middle");
        assert_eq!(at(159, 45), 0, "the right bar is black");
    }

    #[test]
    fn a_short_buffer_is_refused_rather_than_read_past() {
        assert!(shrink_bgr0(&[0u8; 16], 4, 4, 16).is_none());
        assert!(shrink_bgr0(&[], 0, 0, 0).is_none());
    }

    #[test]
    fn a_capture_from_before_a_clear_is_not_filed() {
        let stale = generation();
        clear();
        record_for(stale, 2.0e9, vec![0u8; (THUMB_W * THUMB_H * 4) as usize]);
        assert!(nearest_png(2.0e9).is_none(), "the previous channel's picture leaked through");
    }

    #[test]
    fn only_a_thumbnail_sized_image_is_recorded() {
        // Wrong size: silently dropped, never filed under a time.
        record(1.0e9, vec![0u8; 4]);
        assert!(nearest_png(1.0e9).is_none());
    }
}
