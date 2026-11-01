//! Scrollback thumbnails taken with mpv's `screenshot-raw`, for the surfaces
//! that have no framebuffer of their own to copy from.
//!
//! Linux renders every frame itself and shrinks the one it has just drawn
//! (`linux::thumbs`). On Windows mpv draws into its own window, and on macOS
//! into a Core Animation layer's framebuffer that is current only while Core
//! Animation is drawing it; either way the frame is asked of mpv instead, on
//! a worker thread, and shrunk on the CPU (see
//! [`crate::surface::trickplay::shrink_bgr0`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::surface::{mpv, trickplay};

/// How often the thumbnail clock is looked at. The clock itself decides
/// when one is due (every few seconds of stream); this only bounds how late
/// that can be noticed.
pub(super) const THUMB_POLL: Duration = Duration::from_secs(1);

/// One screenshot at a time. `screenshot-raw` blocks its caller until mpv
/// has the frame; a slow one must not queue another behind it.
static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// Starts a thumbnail if one is due. On the main thread, from the timer.
pub(super) fn thumbnail_tick() {
    // Nothing loaded, nothing to photograph. Read before the clock, so that
    // an idle player does not use up the due time.
    let Some(Some(at)) = mpv::with_player(|p| p.get_f64("playback-time")) else {
        return;
    };
    if !trickplay::is_due() || IN_FLIGHT.swap(true, Ordering::AcqRel) {
        return;
    }
    let generation = trickplay::generation();
    // Off the main thread: the screenshot waits on mpv's core, and the
    // interface must not wait with it.
    let spawned = std::thread::Builder::new()
        .name("zapper-thumb".into())
        .spawn(move || {
            if let Some(rgba) = screenshot_thumbnail() {
                trickplay::record_for(generation, at, rgba);
            }
            IN_FLIGHT.store(false, Ordering::Release);
        });
    if spawned.is_err() {
        IN_FLIGHT.store(false, Ordering::Release);
    }
}

/// mpv's current video frame, shrunk to a thumbnail. `None` when mpv has no
/// frame (between channels) or hands back a format this does not read.
fn screenshot_thumbnail() -> Option<Vec<u8>> {
    use libmpv2_sys::{
        mpv_command_ret, mpv_format_MPV_FORMAT_NODE_MAP, mpv_free_node_contents, mpv_node,
    };

    let handle = mpv::handle()?;
    // `video`: the decoded picture at its own size, without subtitles or
    // OSD, which is what a thumbnail is of. The format is left to mpv's
    // default, `bgr0`, which every version that has `screenshot-raw` speaks.
    let mut args = [
        c"screenshot-raw".as_ptr(),
        c"video".as_ptr(),
        std::ptr::null(),
    ];
    let mut node: mpv_node = unsafe { std::mem::zeroed() };
    let rc = unsafe { mpv_command_ret(handle.ctx.as_ptr(), args.as_mut_ptr(), &mut node) };
    if rc < 0 {
        return None;
    }
    let out = if node.format == mpv_format_MPV_FORMAT_NODE_MAP {
        unsafe { shrink_node(&node) }
    } else {
        None
    };
    unsafe { mpv_free_node_contents(&mut node) };
    out
}

/// Reads `screenshot-raw`'s map — `w`, `h`, `stride`, `format`, `data` — and
/// shrinks the frame it describes.
///
/// # Safety
/// `node` must be a `MPV_FORMAT_NODE_MAP` returned by mpv and not yet freed.
unsafe fn shrink_node(node: &libmpv2_sys::mpv_node) -> Option<Vec<u8>> {
    use libmpv2_sys::{
        mpv_format_MPV_FORMAT_BYTE_ARRAY, mpv_format_MPV_FORMAT_INT64, mpv_format_MPV_FORMAT_STRING,
    };
    use std::ffi::CStr;

    let list = unsafe { node.u.list.as_ref() }?;
    let (mut w, mut h, mut stride) = (None, None, None);
    let (mut format, mut data) = (None, None);
    for i in 0..usize::try_from(list.num).ok()? {
        let key = unsafe { CStr::from_ptr(*list.keys.add(i)) }.to_bytes();
        let value = unsafe { &*list.values.add(i) };
        match (key, value.format) {
            (b"w", f) if f == mpv_format_MPV_FORMAT_INT64 => w = Some(unsafe { value.u.int64 }),
            (b"h", f) if f == mpv_format_MPV_FORMAT_INT64 => h = Some(unsafe { value.u.int64 }),
            (b"stride", f) if f == mpv_format_MPV_FORMAT_INT64 => {
                stride = Some(unsafe { value.u.int64 })
            }
            (b"format", f) if f == mpv_format_MPV_FORMAT_STRING => {
                format = Some(
                    unsafe { CStr::from_ptr(value.u.string) }
                        .to_bytes()
                        .to_vec(),
                )
            }
            (b"data", f) if f == mpv_format_MPV_FORMAT_BYTE_ARRAY => {
                data = unsafe { value.u.ba.as_ref() }
            }
            _ => {}
        }
    }
    if format.as_deref() != Some(b"bgr0".as_slice()) {
        return None;
    }
    let (w, h, stride) = (
        usize::try_from(w?).ok()?,
        usize::try_from(h?).ok()?,
        usize::try_from(stride?).ok()?,
    );
    let data = data?;
    if data.data.is_null() {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data.data as *const u8, data.size) };
    trickplay::shrink_bgr0(bytes, w, h, stride)
}
