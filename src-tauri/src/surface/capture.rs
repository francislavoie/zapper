//! Reconstructs what the window is showing, for verification without eyes.
//!
//! GNOME refuses D-Bus screenshots to unauthorised callers, Wayland-native
//! windows capture black, and under XWayland there is no shared framebuffer
//! for x11grab to read. So the window is reconstructed from its two layers
//! instead:
//!
//! * the video, read back from the GL framebuffer with `glReadPixels`,
//! * the interface, rendered through Cairo, which preserves per-pixel alpha
//!   exactly (a CSS 0.55 arrives as 140/255).
//!
//! Compositing them offline is not a true screenshot - it cannot prove the
//! compositor puts the result on screen - but it catches the failures that
//! actually happen: chrome painting opaque over video, a wrong video rect,
//! an inverted picture, a black surface.

use anyhow::{anyhow, Context, Result};
use gtk::prelude::*;
use std::cell::RefCell;
use std::path::PathBuf;

use crate::surface::gl;
use crate::surface::overlay::Overlay;

const RGBA: u32 = 0x1908;
const UNSIGNED_BYTE: u32 = 0x1401;

thread_local! {
    /// Set from outside the render loop; consumed inside it, where a GL
    /// context is current. Holds the video layer once read back.
    static REQUEST: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    static VIDEO: RefCell<Option<(i32, i32, Vec<u8>)>> = const { RefCell::new(None) };
}

/// Asks for a capture on the next frame. Returns immediately.
pub fn request(path: impl Into<PathBuf>) {
    REQUEST.with(|r| *r.borrow_mut() = Some(path.into()));
}

/// Reads the framebuffer if a capture was asked for. Call from inside the
/// GLArea render signal, after mpv has drawn, where the context is current.
pub fn maybe_grab(width: i32, height: i32) {
    let wanted = REQUEST.with(|r| r.borrow().is_some());
    if !wanted || width < 2 || height < 2 {
        return;
    }
    let mut buf = vec![0u8; (width as usize) * (height as usize) * 4];
    unsafe {
        // Rows are tightly packed; the default alignment of 4 already suits
        // RGBA, so no pack-alignment change is needed.
        gl::read_pixels(0, 0, width, height, RGBA, UNSIGNED_BYTE, buf.as_mut_ptr());
    }
    VIDEO.with(|v| *v.borrow_mut() = Some((width, height, buf)));
}

/// Composites the captured video with the interface and writes a PNG.
/// Call after `maybe_grab` has run at least once.
pub fn finish(ov: &Overlay) -> Result<PathBuf> {
    let path = REQUEST
        .with(|r| r.borrow_mut().take())
        .ok_or_else(|| anyhow!("no capture was requested"))?;
    let (vw, vh, video) = VIDEO
        .with(|v| v.borrow_mut().take())
        .ok_or_else(|| anyhow!("the render loop never grabbed a frame"))?;

    let (w, h) = (ov.overlay.allocated_width(), ov.overlay.allocated_height());
    if w < 2 || h < 2 {
        return Err(anyhow!("overlay has no allocation"));
    }

    // The interface, with its alpha intact.
    let mut ui = gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, w, h)
        .context("allocating the interface surface")?;
    {
        let cr = gtk::cairo::Context::new(&ui).context("cairo context")?;
        ov.overlay.draw(&cr);
    }
    let ui_stride = ui.stride() as usize;
    let ui_data = ui.data().context("locking the interface surface")?;

    // Where the video sits inside the window, so a docked player composites
    // in the right place rather than filling the frame.
    let (vx, vy) = video_origin(ov);

    let mut out = vec![0u8; (w as usize) * (h as usize) * 4];
    for y in 0..h as usize {
        for x in 0..w as usize {
            let o = (y * w as usize + x) * 4;

            // Video underneath. glReadPixels is bottom-left origin.
            let (mut r, mut g, mut b) = (16u32, 16u32, 16u32);
            let (lx, ly) = (x as i32 - vx, y as i32 - vy);
            if lx >= 0 && ly >= 0 && lx < vw && ly < vh {
                let src = (((vh - 1 - ly) as usize) * vw as usize + lx as usize) * 4;
                r = video[src] as u32;
                g = video[src + 1] as u32;
                b = video[src + 2] as u32;
            }

            // Interface on top. Cairo ARGB32 is premultiplied, native-endian,
            // which on x86 means BGRA in memory.
            let s = y * ui_stride + x * 4;
            let (ub, ug, ur, ua) = (
                ui_data[s] as u32,
                ui_data[s + 1] as u32,
                ui_data[s + 2] as u32,
                ui_data[s + 3] as u32,
            );
            let inv = 255 - ua;
            out[o] = (ur + r * inv / 255).min(255) as u8;
            out[o + 1] = (ug + g * inv / 255).min(255) as u8;
            out[o + 2] = (ub + b * inv / 255).min(255) as u8;
            out[o + 3] = 255;
        }
    }

    // Report how much of the interface layer actually carried alpha. Zero
    // means the webview drew nothing into Cairo, which is a different failure
    // from the composite being wrong, and the two look identical in the PNG.
    let mut ui_pixels = 0usize;
    for y in 0..h as usize {
        for x in 0..w as usize {
            if ui_data[y * ui_stride + x * 4 + 3] > 0 {
                ui_pixels += 1;
            }
        }
    }
    tracing::info!(
        "capture: interface covered {ui_pixels} of {} pixels ({:.1}%)",
        w as usize * h as usize,
        100.0 * ui_pixels as f64 / (w as f64 * h as f64),
    );

    write_png(&path, w as u32, h as u32, &out)?;
    Ok(path)
}

fn video_origin(ov: &Overlay) -> (i32, i32) {
    let a = ov.gl_area.allocation();
    (a.x(), a.y())
}

/// A minimal PNG writer, so verification needs no image dependency.
fn write_png(path: &PathBuf, w: u32, h: u32, rgba: &[u8]) -> Result<()> {
    use std::io::Write;

    let mut raw = Vec::with_capacity((w as usize * 4 + 1) * h as usize);
    for y in 0..h as usize {
        raw.push(0); // filter: none
        let row = y * w as usize * 4;
        raw.extend_from_slice(&rgba[row..row + w as usize * 4]);
    }

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let chunk = |tag: &[u8], data: &[u8], out: &mut Vec<u8>| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(tag);
        out.extend_from_slice(data);
        let mut crc = crc32(tag);
        crc = crc32_continue(crc, data);
        out.extend_from_slice(&crc.to_be_bytes());
    };

    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(b"IHDR", &ihdr, &mut png);
    chunk(b"IDAT", &deflate_stored(&raw), &mut png);
    chunk(b"IEND", &[], &mut png);

    let mut f = std::fs::File::create(path)
        .with_context(|| format!("creating {}", path.display()))?;
    f.write_all(&png).context("writing the capture")?;
    Ok(())
}

/// zlib with stored (uncompressed) blocks: larger on disk, but avoids pulling
/// a compression crate into the binary for a debug aid.
fn deflate_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    for (i, block) in data.chunks(65_535).enumerate() {
        let last = (i + 1) * 65_535 >= data.len();
        out.push(if last { 1 } else { 0 });
        out.extend_from_slice(&(block.len() as u16).to_le_bytes());
        out.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        out.extend_from_slice(block);
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[n] = c;
        n += 1;
    }
    t
}

/// PNG's CRC-32 over one buffer, from the standard initial state.
fn crc32(data: &[u8]) -> u32 {
    crc32_continue(0, data)
}

fn crc32_continue(prev: u32, data: &[u8]) -> u32 {
    let t = crc_table();
    let mut c = prev ^ 0xFFFF_FFFF;
    for &b in data {
        c = t[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}
