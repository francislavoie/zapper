//! Scrollback thumbnails on Linux: a copy of the frame GTK just drew.
//!
//! The frame is already on the GPU, in GtkGLArea's framebuffer, because it
//! has just been drawn. It is blitted down to 160x90 there and only that is
//! read back: 57KB, where reading a fullscreen surface back to shrink it on
//! the CPU would move about 14MB and stall the pipeline waiting on the GPU.
//! The result is filed with [`crate::surface::trickplay::record`].

use super::gl;
use crate::surface::trickplay::{record, THUMB_H, THUMB_W};

thread_local! {
    /// The offscreen target, built once on the GTK thread and kept. `None`
    /// once the driver has told us it cannot do this, so a missing entry
    /// point is asked about once rather than every four seconds.
    static TARGET: std::cell::RefCell<Option<Option<Target>>> =
        const { std::cell::RefCell::new(None) };
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
/// time — see [`crate::surface::trickplay::is_due`].
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
    record(at, rgba);
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

#[cfg(test)]
mod tests {
    use super::*;

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
