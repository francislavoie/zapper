//! A minimal PNG encoder, so neither verification nor the scrubber needs an
//! image dependency.
//!
//! Portable on purpose: it began inside the GTK capture code, but the
//! scrollback thumbnails are encoded with it on every platform, and an
//! encoder has no business knowing which toolkit drew the pixels.
//!
//! Stored (uncompressed) deflate blocks: larger than a real encoder's output,
//! which does not matter for a 160x90 thumbnail or a debug capture.

use anyhow::{Context, Result};
use std::path::Path;

/// Writes RGBA rows, top-down, to `path` as a PNG.
pub fn write(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<()> {
    use std::io::Write;
    let png = png_bytes(w, h, rgba);
    let mut f =
        std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    f.write_all(&png).context("writing the capture")?;
    Ok(())
}

/// A PNG in memory, as RGBA rows top-down.
///
/// Separate from [`write`] because the scrollback thumbnails need the
/// bytes rather than a file: they live in memory for as long as the buffer
/// they describe, and most of them are never looked at.
pub fn png_bytes(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
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
    png
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_the_png_specs_check_value() {
        // The CRC-32 check value from the PNG specification's sample code.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn adler_matches_the_zlib_reference() {
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn a_png_has_its_signature_and_three_chunks() {
        let png = png_bytes(2, 1, &[255, 0, 0, 255, 0, 255, 0, 255]);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
    }
}
