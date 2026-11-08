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
//!
//! **A movie or an episode** keeps its frames differently, because its bar is
//! the whole title rather than a window behind a live edge. The store is told
//! which title it is holding ([`open_title`]); frames are then filed by the
//! title's own time into slots a fixed spacing apart ([`title_spacing`]), at
//! most one per slot, and kept for as long as that title is open: reopening
//! the same title keeps them, anything else clears them. Two things fill the
//! slots: the frames drawn while it plays, exactly as for a channel, and
//! `surface::title_thumbs`, which reads what mpv has already buffered ahead
//! and behind and decodes one picture per empty slot. That second path is the
//! one place a frame is decoded twice, which is why it is paced, one at a
//! time, and gives way whenever playback needs the machine.

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
    img: Img,
}

/// A thumbnail's pixels, as they were taken or as they are kept.
enum Img {
    /// RGBA, top-down, `THUMB_W * THUMB_H * 4` bytes: a frame as it was
    /// drawn. A channel's stay like this (there are at most [`MAX_THUMBS`]).
    Rgba(Vec<u8>),
    /// A JPEG of the same picture. A title can hold up to
    /// [`TITLE_MAX_SLOTS`], so its frames are compressed off the main thread
    /// (`surface::title_thumbs`): a few kilobytes each instead of 57KB.
    Jpeg(Vec<u8>),
}

/// The title the store is holding frames for, and how far apart they are.
struct Title {
    key: String,
    spacing: f64,
}

#[derive(Default)]
struct Store {
    /// Sorted by `at`, because that is how it is searched and how it is
    /// pruned, and keeping it sorted on insert is cheaper than either.
    thumbs: Vec<Thumb>,
    /// `Some` while a movie or an episode is open: the frames are then the
    /// title's, filed by slot ([`Store::insert_title`]), and never pruned.
    title: Option<Title>,
}

// ---------------------------------------------------------------------------
// On-demand titles: slots and the plan for filling them. Pure, so tested.
// ---------------------------------------------------------------------------

/// Seconds of a title between two slots. Ten is fine enough to find a scene
/// on a bar a few hundred pixels wide and coarse enough that filling what is
/// buffered stays a few dozen decodes.
pub const TITLE_SPACING_S: f64 = 10.0;

/// The most slots a title gets: a three-hour film at [`TITLE_SPACING_S`] is
/// 1080. A longer one is spaced further apart instead (see
/// [`title_spacing`]). At a few kilobytes a JPEG, about 6MB at most.
pub const TITLE_MAX_SLOTS: usize = 1100;

/// How much of the cache one slot asks for. mpv writes whole groups of
/// pictures, from the keyframe at or before the start, so a tenth of a
/// second is one group (two at a keyframe): the keyframe the preview is.
pub const DUMP_SPAN_S: f64 = 0.1;

/// The spacing for a title of `duration` seconds: [`TITLE_SPACING_S`], or a
/// multiple of five seconds wide enough to stay within [`TITLE_MAX_SLOTS`].
pub fn title_spacing(duration: Option<f64>) -> f64 {
    match duration {
        Some(d) if d.is_finite() && d > TITLE_SPACING_S * TITLE_MAX_SLOTS as f64 => {
            (d / TITLE_MAX_SLOTS as f64 / 5.0).ceil() * 5.0
        }
        _ => TITLE_SPACING_S,
    }
}

/// The slot a moment of a title belongs to: the nearest multiple of
/// `spacing`.
pub fn slot_of(at: f64, spacing: f64) -> i64 {
    (at / spacing).round() as i64
}

/// The next slot to fill from what mpv holds: `ranges` are its seekable
/// ranges, in the title's time. Ahead of the playhead first, nearest first,
/// because that is where the viewer is about to be; then behind it, nearest
/// first. Only a slot whose excerpt lies wholly inside a range, and not past
/// the title's end; `skip` says which already have a frame or were tried.
pub fn next_slot(
    ranges: &[(f64, f64)],
    playhead: f64,
    spacing: f64,
    duration: Option<f64>,
    skip: impl Fn(i64) -> bool,
) -> Option<i64> {
    if !spacing.is_finite() || spacing <= 0.0 || !playhead.is_finite() {
        return None;
    }
    let end_of_title = duration.filter(|d| d.is_finite() && *d > 0.0);
    // A slot is readable when its excerpt is inside the range and the
    // moment is inside the title.
    let readable = |k: i64, (start, end): (f64, f64)| {
        let t = k as f64 * spacing;
        k >= 0 && t >= start && t + DUMP_SPAN_S <= end && end_of_title.is_none_or(|d| t <= d)
    };
    let mut sorted: Vec<(f64, f64)> = ranges
        .iter()
        .copied()
        .filter(|(s, e)| s.is_finite() && e.is_finite() && e > s)
        .collect();
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0));

    // Ahead: from the first slot at or after the playhead, range by range.
    for &r in &sorted {
        let from = (r.0.max(playhead) / spacing).ceil() as i64;
        let to = ((r.1 - DUMP_SPAN_S) / spacing).floor() as i64;
        if let Some(k) = (from..=to).find(|&k| readable(k, r) && !skip(k)) {
            return Some(k);
        }
    }
    // Behind: from the last slot before the playhead, ranges latest first.
    for &r in sorted.iter().rev() {
        let below = (playhead / spacing).ceil() as i64 - 1;
        let to = (((r.1 - DUMP_SPAN_S) / spacing).floor() as i64).min(below);
        let from = (r.0 / spacing).ceil() as i64;
        if let Some(k) = (from..=to).rev().find(|&k| readable(k, r) && !skip(k)) {
            return Some(k);
        }
    }
    None
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
    insert(generation, Thumb { at, img: Img::Rgba(rgba) });
}

fn insert(generation: u64, thumb: Thumb) {
    let Ok(mut guard) = STORE.lock() else { return };
    if GENERATION.load(Ordering::Acquire) != generation {
        return;
    }
    guard.get_or_insert_with(Store::default).insert(thumb);
}

impl Store {
    fn insert(&mut self, thumb: Thumb) {
        if self.title.is_some() {
            self.insert_title(thumb);
            return;
        }
        // Rewatching a stretch that is already covered must not stack duplicates
        // on top of it, and a seek backwards arrives out of order.
        let pos = self.thumbs.partition_point(|t| t.at < thumb.at);
        let near = |i: usize| {
            self.thumbs
                .get(i)
                .is_some_and(|t| (t.at - thumb.at).abs() < INTERVAL_S / 2.0)
        };
        if (pos > 0 && near(pos - 1)) || near(pos) {
            return;
        }
        self.thumbs.insert(pos, thumb);

        // Oldest first: the buffer they describe is pruned from that end too.
        if self.thumbs.len() > MAX_THUMBS {
            let excess = self.thumbs.len() - MAX_THUMBS;
            self.thumbs.drain(..excess);
        }
    }

    /// The thumbnail nearest `at` within [`TOLERANCE_S`]: a channel's.
    fn nearest(&self, at: f64) -> Option<&Thumb> {
        if self.thumbs.is_empty() || !at.is_finite() {
            return None;
        }
        let pos = self.thumbs.partition_point(|t| t.at < at);
        let best = [pos.checked_sub(1), Some(pos)]
            .into_iter()
            .flatten()
            .filter_map(|i| self.thumbs.get(i))
            .min_by(|a, b| {
                (a.at - at)
                    .abs()
                    .partial_cmp(&(b.at - at).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })?;
        ((best.at - at).abs() <= TOLERANCE_S).then_some(best)
    }

    /// Files a title's frame in its slot unless the slot has one already:
    /// the first frame of a slot stays, whichever path brought it. Nothing
    /// is pruned, since the slots are bounded by the title's length; the cap
    /// is for a title whose length was never known.
    fn insert_title(&mut self, thumb: Thumb) -> bool {
        let Some(spacing) = self.title.as_ref().map(|t| t.spacing) else {
            return false;
        };
        if !thumb.at.is_finite() || thumb.at < 0.0 || self.thumbs.len() >= TITLE_MAX_SLOTS {
            return false;
        }
        let slot = slot_of(thumb.at, spacing);
        let pos = self.thumbs.partition_point(|t| t.at < thumb.at);
        // A slot is one stretch of time, so a frame already in it is a
        // neighbour of where this one would go.
        let taken = |i: usize| {
            self.thumbs
                .get(i)
                .is_some_and(|t| slot_of(t.at, spacing) == slot)
        };
        if (pos > 0 && taken(pos - 1)) || taken(pos) {
            return false;
        }
        self.thumbs.insert(pos, thumb);
        true
    }

    /// The frame filed in the slot nearest `at`, if any: within half a
    /// spacing of it, counted in slots.
    fn title_lookup(&self, at: f64) -> Option<&Thumb> {
        let spacing = self.title.as_ref()?.spacing;
        if !at.is_finite() {
            return None;
        }
        let slot = slot_of(at, spacing);
        let from = self
            .thumbs
            .partition_point(|t| slot_of(t.at, spacing) < slot);
        self.thumbs
            .get(from)
            .filter(|t| slot_of(t.at, spacing) == slot)
    }

    /// Changes a title's spacing once its length is known, keeping the
    /// first frame of each new slot.
    fn set_spacing(&mut self, spacing: f64) {
        let Some(title) = self.title.as_mut() else {
            return;
        };
        if !spacing.is_finite() || spacing <= 0.0 || title.spacing == spacing {
            return;
        }
        title.spacing = spacing;
        let mut last: Option<i64> = None;
        self.thumbs.retain(|t| {
            let slot = slot_of(t.at, spacing);
            let keep = last != Some(slot);
            last = Some(slot);
            keep
        });
    }

    /// The slots that have a frame.
    fn filled(&self) -> std::collections::HashSet<i64> {
        let Some(spacing) = self.title.as_ref().map(|t| t.spacing) else {
            return Default::default();
        };
        self.thumbs.iter().map(|t| slot_of(t.at, spacing)).collect()
    }
}

/// The thumbnail nearest `at` in stream time, as PNG bytes, or `None` when
/// nothing was captured close enough to be honest about.
pub fn nearest_png(at: f64) -> Option<(f64, Vec<u8>)> {
    let guard = STORE.lock().ok()?;
    let best = guard.as_ref()?.nearest(at)?;
    Some((best.at, encoded(&best.img).1))
}

/// A thumbnail as an image file's bytes, with its media type.
fn encoded(img: &Img) -> (&'static str, Vec<u8>) {
    match img {
        Img::Rgba(rgba) => (
            "image/png",
            crate::surface::png::png_bytes(THUMB_W as u32, THUMB_H as u32, rgba),
        ),
        Img::Jpeg(jpeg) => ("image/jpeg", jpeg.clone()),
    }
}

/// Starts holding frames for an on-demand title, `key` naming it. The same
/// title again (a reopen, a reconnect, a change of sound) keeps what it has
/// and returns `true`; anything else starts empty, like [`clear`].
pub fn open_title(key: &str) -> bool {
    let Ok(mut guard) = STORE.lock() else { return false };
    if guard
        .as_ref()
        .and_then(|s| s.title.as_ref())
        .is_some_and(|t| t.key == key)
    {
        return true;
    }
    GENERATION.fetch_add(1, Ordering::AcqRel);
    *guard = Some(Store {
        thumbs: Vec::new(),
        title: Some(Title {
            key: key.to_owned(),
            spacing: TITLE_SPACING_S,
        }),
    });
    drop(guard);
    LAST.with(|l| l.set(None));
    false
}

/// The open title's spacing, now that its length is known; `None` when no
/// title is open.
pub fn settle_title_spacing(duration: Option<f64>) -> Option<f64> {
    let mut guard = STORE.lock().ok()?;
    let store = guard.as_mut()?;
    store.title.as_ref()?;
    store.set_spacing(title_spacing(duration));
    store.title.as_ref().map(|t| t.spacing)
}

/// The open title's filled slots, for planning; `None` when no title is
/// open or the store has moved on from `generation`.
pub fn title_slots(generation: u64) -> Option<std::collections::HashSet<i64>> {
    let guard = STORE.lock().ok()?;
    if GENERATION.load(Ordering::Acquire) != generation {
        return None;
    }
    let store = guard.as_ref()?;
    store.title.as_ref()?;
    Some(store.filled())
}

/// Files a decoded frame of the open title, as a JPEG, unless the store has
/// moved on from `generation`. `false` when it was not kept.
pub fn record_title(generation: u64, at: f64, jpeg: Vec<u8>) -> bool {
    let Ok(mut guard) = STORE.lock() else { return false };
    if GENERATION.load(Ordering::Acquire) != generation || !at.is_finite() || jpeg.is_empty() {
        return false;
    }
    guard.as_mut().is_some_and(|s| {
        s.title.is_some() && s.insert_title(Thumb { at, img: Img::Jpeg(jpeg) })
    })
}

/// A drawn frame of the open title still kept as RGBA, for compressing.
pub fn raw_title_frame() -> Option<(u64, f64, Vec<u8>)> {
    let guard = STORE.lock().ok()?;
    let store = guard.as_ref()?;
    store.title.as_ref()?;
    store.thumbs.iter().find_map(|t| match &t.img {
        Img::Rgba(rgba) => Some((generation(), t.at, rgba.clone())),
        Img::Jpeg(_) => None,
    })
}

/// Swaps the RGBA frame at `at` for its JPEG ([`raw_title_frame`]).
pub fn compress_title_frame(generation: u64, at: f64, jpeg: Vec<u8>) {
    let Ok(mut guard) = STORE.lock() else { return };
    if GENERATION.load(Ordering::Acquire) != generation || jpeg.is_empty() {
        return;
    }
    if let Some(t) = guard
        .as_mut()
        .and_then(|s| s.thumbs.iter_mut().find(|t| t.at == at))
    {
        t.img = Img::Jpeg(jpeg);
    }
}

/// The open title's frame for the slot nearest `at`, as an image file's
/// bytes with its media type, and the moment it shows. `None` when no title
/// is open or that slot has no frame.
pub fn title_frame(at: f64) -> Option<(f64, &'static str, Vec<u8>)> {
    let guard = STORE.lock().ok()?;
    let thumb = guard.as_ref()?.title_lookup(at)?;
    let (mime, bytes) = encoded(&thumb.img);
    Some((thumb.at, mime, bytes))
}

/// The moments the open title has frames for, in order. For the harness.
pub fn title_times() -> Vec<f64> {
    STORE
        .lock()
        .ok()
        .and_then(|g| {
            g.as_ref()
                .filter(|s| s.title.is_some())
                .map(|s| s.thumbs.iter().map(|t| t.at).collect())
        })
        .unwrap_or_default()
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

    // ----- on-demand titles

    fn title_store(spacing: f64) -> Store {
        Store {
            thumbs: Vec::new(),
            title: Some(Title {
                key: "t".into(),
                spacing,
            }),
        }
    }

    fn jpeg(at: f64) -> Thumb {
        Thumb {
            at,
            img: Img::Jpeg(vec![1]),
        }
    }

    fn times(s: &Store) -> Vec<f64> {
        s.thumbs.iter().map(|t| t.at).collect()
    }

    #[test]
    fn a_title_is_ten_seconds_a_slot_unless_that_is_too_many() {
        assert_eq!(title_spacing(None), 10.0);
        assert_eq!(title_spacing(Some(f64::NAN)), 10.0);
        assert_eq!(title_spacing(Some(90.0 * 60.0)), 10.0);
        // Three hours is 1080 slots: still ten.
        assert_eq!(title_spacing(Some(3.0 * 3600.0)), 10.0);
        // Four hours would be 1440: spaced out to a multiple of five.
        let four = title_spacing(Some(4.0 * 3600.0));
        assert_eq!(four, 15.0);
        assert!(4.0 * 3600.0 / four <= TITLE_MAX_SLOTS as f64);
        // Ten hours stays inside the cap too.
        let ten = title_spacing(Some(10.0 * 3600.0));
        assert_eq!(ten % 5.0, 0.0);
        assert!(10.0 * 3600.0 / ten <= TITLE_MAX_SLOTS as f64);
    }

    #[test]
    fn a_moment_belongs_to_the_nearest_slot() {
        assert_eq!(slot_of(0.0, 10.0), 0);
        assert_eq!(slot_of(4.9, 10.0), 0);
        assert_eq!(slot_of(5.1, 10.0), 1);
        assert_eq!(slot_of(48.1, 10.0), 5);
        assert_eq!(slot_of(54.0, 10.0), 5);
    }

    #[test]
    fn filling_starts_just_ahead_of_the_playhead_and_walks_forward() {
        let ranges = [(0.0, 95.0)];
        let none = |_| false;
        assert_eq!(next_slot(&ranges, 33.0, 10.0, Some(600.0), none), Some(4));
        // The ones it has are skipped.
        let have = |k| k == 4 || k == 5;
        assert_eq!(next_slot(&ranges, 33.0, 10.0, Some(600.0), have), Some(6));
    }

    #[test]
    fn behind_the_playhead_comes_after_everything_ahead() {
        let ranges = [(0.0, 52.0)];
        // Ahead: 40 and 50 (50.1 is inside 52). Then behind, nearest first.
        let have = |k| k == 4 || k == 5;
        assert_eq!(next_slot(&ranges, 33.0, 10.0, None, have), Some(3));
        let have = |k| (3..=5).contains(&k);
        assert_eq!(next_slot(&ranges, 33.0, 10.0, None, have), Some(2));
        let have = |k| (0..=5).contains(&k);
        assert_eq!(next_slot(&ranges, 33.0, 10.0, None, have), None);
    }

    #[test]
    fn nothing_is_planned_past_the_buffered_end() {
        // 50.0 + the excerpt would end past 50.05: not yet readable.
        let ranges = [(20.0, 50.05)];
        let have = |k| (2..=4).contains(&k);
        assert_eq!(next_slot(&ranges, 21.0, 10.0, None, have), None);
        // Nor before a range's start, which mpv cannot read from either.
        let ranges = [(25.0, 50.0)];
        let have = |k| (3..=4).contains(&k);
        assert_eq!(next_slot(&ranges, 26.0, 10.0, None, have), None);
    }

    #[test]
    fn nothing_is_planned_past_the_titles_end() {
        let ranges = [(0.0, 125.0)];
        let have = |k| k <= 11;
        assert_eq!(next_slot(&ranges, 100.0, 10.0, Some(118.0), have), None);
    }

    #[test]
    fn every_buffered_range_is_walked_ahead_first() {
        // After a seek: what was read before it, and a range from the seek.
        let ranges = [(0.0, 31.0), (200.0, 241.0)];
        let mut have = std::collections::HashSet::new();
        let mut order = Vec::new();
        while let Some(k) = next_slot(&ranges, 205.0, 10.0, None, |k| have.contains(&k)) {
            order.push(k);
            have.insert(k);
        }
        assert_eq!(order, vec![21, 22, 23, 24, 20, 3, 2, 1, 0]);
    }

    #[test]
    fn a_title_keeps_one_frame_per_slot_whichever_comes_first() {
        let mut s = title_store(10.0);
        assert!(s.insert_title(jpeg(48.1)));
        assert!(!s.insert_title(jpeg(50.0)), "slot 5 already has one");
        assert!(!s.insert_title(jpeg(54.9)), "slot 5 already has one");
        assert!(s.insert_title(jpeg(55.1)), "slot 6 is free");
        assert!(s.insert_title(jpeg(20.0)), "out of order is fine");
        assert_eq!(times(&s), vec![20.0, 48.1, 55.1]);
    }

    #[test]
    fn a_title_answers_from_the_slot_under_the_pointer_only() {
        let mut s = title_store(10.0);
        s.insert_title(jpeg(48.1));
        assert_eq!(s.title_lookup(54.0).map(|t| t.at), Some(48.1));
        assert_eq!(s.title_lookup(45.0).map(|t| t.at), Some(48.1));
        assert!(s.title_lookup(55.5).is_none(), "slot 6 has nothing");
        assert!(s.title_lookup(40.0).is_none(), "slot 4 has nothing");
        assert!(s.title_lookup(f64::NAN).is_none());
        assert!(title_store(10.0).title_lookup(0.0).is_none());
    }

    #[test]
    fn a_titles_frames_are_capped() {
        let mut s = title_store(10.0);
        for k in 0..TITLE_MAX_SLOTS + 50 {
            s.insert_title(jpeg(k as f64 * 10.0));
        }
        assert_eq!(s.thumbs.len(), TITLE_MAX_SLOTS);
    }

    #[test]
    fn a_wider_spacing_keeps_the_first_frame_of_each_new_slot() {
        let mut s = title_store(10.0);
        for at in [0.0, 10.0, 20.0, 30.0, 40.0] {
            s.insert_title(jpeg(at));
        }
        s.set_spacing(20.0);
        // Slots of 20: 0 ← 0, 10 (round 0.5 = 1) …: one each.
        let slots: Vec<i64> = s.thumbs.iter().map(|t| slot_of(t.at, 20.0)).collect();
        let mut dedup = slots.clone();
        dedup.dedup();
        assert_eq!(slots, dedup, "one frame per slot");
        assert_eq!(s.filled(), slots.into_iter().collect());
    }

    #[test]
    fn a_channel_store_is_not_slotted() {
        // The live store dedups at half its interval and nothing else.
        let mut s = Store::default();
        s.insert(Thumb {
            at: 48.1,
            img: Img::Rgba(vec![]),
        });
        s.insert(Thumb {
            at: 52.0,
            img: Img::Rgba(vec![]),
        });
        assert_eq!(times(&s), vec![48.1, 52.0]);
        assert!(s.title_lookup(50.0).is_none());
    }
}
