//! The seek bar's previews of a title, decoded: `thumbframe.c`, against the
//! FFmpeg libmpv already loads (see build.rs), as the
//! [`zapper_core::player::title_thumbs::Decoder`] the core's preview worker
//! calls. [`install`] puts it in place; the host does, with the player.
//!
//! Without FFmpeg's development files (build.rs says so with
//! `zapper_no_thumbframe`) nothing is installed, and the frames drawn while
//! a title plays stay as they were taken.

#[cfg(not(zapper_no_thumbframe))]
use std::ffi::{c_char, c_int, CString};
use std::path::Path;

use zapper_core::player::title_thumbs::{self, Decoder};
#[cfg(not(zapper_no_thumbframe))]
use zapper_core::player::trickplay::{THUMB_H, THUMB_W};

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

/// FFmpeg's decoder and JPEG encoder, through `thumbframe.c`.
pub struct Ffmpeg;

/// Installs [`Ffmpeg`] as the process's preview decoder, when this build has
/// it. Returns whether it did.
pub fn install() -> bool {
    if cfg!(zapper_no_thumbframe) {
        return false;
    }
    title_thumbs::install_decoder(&Ffmpeg);
    true
}

#[cfg(not(zapper_no_thumbframe))]
impl Decoder for Ffmpeg {
    fn first_picture(&self, file: &Path) -> Option<(Vec<u8>, u64)> {
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
        (r == 0).then_some((rgba, key))
    }

    fn jpeg(&self, rgba: &[u8]) -> Option<Vec<u8>> {
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
}

// A build without FFmpeg's development files: nothing is decoded or
// compressed. Never installed (see [`install`]); here so the type is the
// same either way.
#[cfg(zapper_no_thumbframe)]
impl Decoder for Ffmpeg {
    fn first_picture(&self, _: &Path) -> Option<(Vec<u8>, u64)> {
        None
    }

    fn jpeg(&self, _: &[u8]) -> Option<Vec<u8>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zapper_core::player::trickplay::{THUMB_H, THUMB_W};

    #[test]
    #[cfg_attr(zapper_no_thumbframe, ignore = "built without FFmpeg's development files")]
    fn a_preview_compresses_to_a_few_kilobytes() {
        // A gradient, which is about as busy as a frame at this size.
        let mut rgba = vec![0u8; (THUMB_W * THUMB_H * 4) as usize];
        for (i, px) in rgba.chunks_mut(4).enumerate() {
            let (x, y) = (i % THUMB_W as usize, i / THUMB_W as usize);
            px.copy_from_slice(&[(x * 255 / 160) as u8, (y * 255 / 90) as u8, 128, 255]);
        }
        let j = Ffmpeg.jpeg(&rgba).expect("FFmpeg's JPEG encoder");
        assert_eq!(&j[..2], &[0xFF, 0xD8], "a JPEG's start of image");
        assert!(j.len() < 12_000, "{} bytes", j.len());
        assert!(Ffmpeg.jpeg(&[0u8; 4]).is_none());
    }

    #[test]
    fn a_file_that_is_not_video_gives_no_picture() {
        let p = std::env::temp_dir().join(format!("zapper-preview-test-{}.mkv", std::process::id()));
        std::fs::write(&p, b"not a video").unwrap();
        assert!(Ffmpeg.first_picture(&p).is_none());
        let _ = std::fs::remove_file(&p);
        assert!(Ffmpeg.first_picture(Path::new("/nonexistent/zapper-preview.mkv")).is_none());
    }
}
