//! Scrollback thumbnails: what the picture looked like, a while ago.
//!
//! Scrubbing a live buffer by clock time alone is guesswork — "two minutes
//! back" does not tell you whether that is before or after the thing you are
//! looking for. These are the small images the scrubber shows under the
//! cursor so it does.
//!
//! **Nothing here decodes anything.** That is the entire reason this is
//! affordable. The frames have already been decoded and are already on the
//! GPU, because they have just been drawn; a thumbnail is a copy of one that
//! was going to be thrown away. There is no second decoder, no second
//! connection to the panel — which matters, because the account permits
//! exactly one — and no re-reading of the demuxer cache, which mpv does not
//! expose anyway.
//!
//! The cost is a hardware downscale and a small readback, a few times a
//! minute. The downscale is the important half: reading a fullscreen surface
//! back to shrink it on the CPU would move about 14MB and stall the pipeline
//! waiting on the GPU, where blitting to 160x90 first and reading *that* back
//! moves 57KB.
//!
//! The store is a plain mutex rather than the thread-local the rest of this
//! module uses, because the capture happens on the GTK thread and the lookup
//! answers an IPC command on another. The data is bytes, so it can simply
//! move; a write every few seconds contends with nothing.

use std::sync::Mutex;
use std::time::Instant;

use crate::surface::gl;

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
    /// The offscreen target, built once on the GTK thread and kept. `None`
    /// once the driver has told us it cannot do this, so a missing entry
    /// point is asked about once rather than every four seconds.
    static TARGET: std::cell::RefCell<Option<Option<Target>>> =
        const { std::cell::RefCell::new(None) };
    static LAST: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

struct Target {
    fbo: u32,
}

/// Builds the small offscreen framebuffer. `None` when the driver lacks the
/// framebuffer-object entry points, which is a reason to do without
/// thumbnails and not a reason to fail.
///
/// # Safety
/// Requires a current GL context.
unsafe fn make_target() -> Option<Target> {
    let fbo = gl::gen_framebuffer();
    let rb = gl::gen_renderbuffer();
    if fbo == 0 || rb == 0 {
        return None;
    }
    gl::bind_renderbuffer(rb);
    gl::renderbuffer_storage(gl::RGBA8, THUMB_W, THUMB_H);
    gl::bind_renderbuffer(0);

    let prev = gl::get_integer(gl::DRAW_FRAMEBUFFER_BINDING);
    gl::bind_framebuffer(gl::DRAW_FRAMEBUFFER, fbo as i32);
    gl::framebuffer_renderbuffer(gl::DRAW_FRAMEBUFFER, rb);
    let status = gl::check_framebuffer_status(gl::DRAW_FRAMEBUFFER);
    gl::bind_framebuffer(gl::DRAW_FRAMEBUFFER, prev);

    if status != gl::FRAMEBUFFER_COMPLETE {
        tracing::warn!("thumbnail framebuffer incomplete (0x{status:x}); no scrollback previews");
        return None;
    }
    Some(Target { fbo })
}

/// Takes a thumbnail of the frame just drawn, if one is due.
///
/// Called from the render handler, where the GLArea's framebuffer is bound
/// and a context is current. `surface` is the size of that framebuffer, and
/// `playback_time` is read lazily by the caller only when this says it is
/// time — see [`due`].
///
/// # Safety
/// Requires a current GL context, with the surface's framebuffer bound for
/// drawing, exactly as inside GTK's `render` signal.
pub unsafe fn capture(surface_w: i32, surface_h: i32, at: f64) {
    if surface_w < 16 || surface_h < 16 || !at.is_finite() {
        return;
    }

    let fbo = TARGET.with(|t| {
        let mut slot = t.borrow_mut();
        let built = slot.get_or_insert_with(|| make_target());
        built.as_ref().map(|x| x.fbo)
    });
    let Some(fbo) = fbo else { return };

    // GTK has its own framebuffer bound for drawing and expects it still
    // bound when this returns; mpv has already caught us out once by not
    // restoring it.
    let prev_draw = gl::get_integer(gl::DRAW_FRAMEBUFFER_BINDING);
    let prev_read = gl::get_integer(gl::READ_FRAMEBUFFER_BINDING);

    gl::bind_framebuffer(gl::READ_FRAMEBUFFER, prev_draw);
    gl::bind_framebuffer(gl::DRAW_FRAMEBUFFER, fbo as i32);
    let blitted = gl::blit_framebuffer(
        0, 0, surface_w, surface_h, //
        0, 0, THUMB_W, THUMB_H,
    );

    let mut rgba = vec![0u8; (THUMB_W * THUMB_H * 4) as usize];
    let read = if blitted {
        gl::bind_framebuffer(gl::READ_FRAMEBUFFER, fbo as i32);
        gl::read_pixels_rgba(0, 0, THUMB_W, THUMB_H, &mut rgba)
    } else {
        false
    };

    gl::bind_framebuffer(gl::READ_FRAMEBUFFER, prev_read);
    gl::bind_framebuffer(gl::DRAW_FRAMEBUFFER, prev_draw);

    if !blitted {
        // No blit entry point: record the refusal once by dropping the target
        // so nothing retries every four seconds.
        TARGET.with(|t| *t.borrow_mut() = Some(None));
        tracing::warn!("glBlitFramebuffer is unavailable; no scrollback previews");
        return;
    }
    if !read {
        return;
    }

    // GL's origin is bottom-left, so the rows come back upside down.
    flip_rows(&mut rgba, THUMB_W as usize, THUMB_H as usize);
    insert(Thumb { at, rgba });
}

/// Turns a bottom-up readback into a top-down image.
fn flip_rows(rgba: &mut [u8], w: usize, h: usize) {
    let stride = w * 4;
    for y in 0..h / 2 {
        let (top, bottom) = rgba.split_at_mut((y + 1) * stride);
        let a = &mut top[y * stride..];
        let b = &mut bottom[(h - 2 * y - 2) * stride..][..stride];
        a[..stride].swap_with_slice(b);
    }
}

fn insert(thumb: Thumb) {
    let Ok(mut guard) = STORE.lock() else { return };
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
        crate::surface::capture::png_bytes(THUMB_W as u32, THUMB_H as u32, &best.rgba),
    ))
}

/// Drops everything. Called when the stream changes: the thumbnails describe
/// a buffer that no longer exists, and showing the previous channel's picture
/// under the cursor would be worse than showing none.
pub fn clear() {
    if let Ok(mut guard) = STORE.lock() {
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
    fn flipping_rows_turns_a_readback_the_right_way_up() {
        // Two rows of one pixel: GL hands back the bottom one first.
        let mut px = vec![1, 1, 1, 255, 2, 2, 2, 255];
        flip_rows(&mut px, 1, 2);
        assert_eq!(px, vec![2, 2, 2, 255, 1, 1, 1, 255]);
        // An odd height leaves the middle row alone and is not an error.
        let mut three = vec![1, 1, 1, 255, 9, 9, 9, 255, 3, 3, 3, 255];
        flip_rows(&mut three, 1, 3);
        assert_eq!(three, vec![3, 3, 3, 255, 9, 9, 9, 255, 1, 1, 1, 255]);
    }
}
