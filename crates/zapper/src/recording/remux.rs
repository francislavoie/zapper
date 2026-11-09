//! A captured part into Matroska, through `remux.c` and libavformat.
//!
//! Two passes over the capture: [`scan`] reads it to learn where the
//! reference stream's packets land on the output's timeline (for the clock
//! track), then [`remux`] copies every packet into the `.mkv`, with the clock
//! cues as a subtitle stream if wanted. Both are blocking and read the whole
//! file, so they run on a blocking thread.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::Path;

use anyhow::{anyhow, Result};

use super::clock::{Cue, Sample};

#[repr(C)]
struct ZrSample {
    pos: i64,
    out_ms: i64,
    disc: i32,
}

#[repr(C)]
struct ZrCue {
    start_ms: i64,
    end_ms: i64,
    text: *const c_char,
}

extern "C" {
    fn zr_scan(
        path: *const c_char,
        samples: *mut *mut ZrSample,
        count: *mut usize,
        duration_ms: *mut i64,
        err: *mut c_char,
        errlen: usize,
    ) -> c_int;
    fn zr_remux(
        input: *const c_char,
        output: *const c_char,
        ass_header: *const c_char,
        cues: *const ZrCue,
        ncues: usize,
        err: *mut c_char,
        errlen: usize,
    ) -> c_int;
    fn zr_free(p: *mut c_void);
}

fn c_path(p: &Path) -> Result<CString> {
    CString::new(p.to_string_lossy().as_bytes()).map_err(|_| anyhow!("a path with a NUL in it"))
}

fn error(buf: &[c_char]) -> String {
    // SAFETY: the C side always NUL-terminates within the buffer (snprintf),
    // and the buffer was zeroed beforehand.
    unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

/// What [`scan`] learnt about a capture.
pub struct Scan {
    pub samples: Vec<Sample>,
    pub duration_ms: i64,
}

pub fn scan(input: &Path) -> Result<Scan> {
    let path = c_path(input)?;
    let mut err = [0 as c_char; 512];
    let mut ptr: *mut ZrSample = std::ptr::null_mut();
    let mut count = 0usize;
    let mut duration_ms = 0i64;
    // SAFETY: every pointer is valid for the call; on success the C side
    // hands over an array of `count` samples, freed below with its own
    // allocator.
    let r = unsafe {
        zr_scan(
            path.as_ptr(),
            &mut ptr,
            &mut count,
            &mut duration_ms,
            err.as_mut_ptr(),
            err.len(),
        )
    };
    if r < 0 {
        return Err(anyhow!(error(&err)));
    }
    let samples = if ptr.is_null() {
        Vec::new()
    } else {
        // SAFETY: as above.
        let raw = unsafe { std::slice::from_raw_parts(ptr, count) };
        let v = raw
            .iter()
            .map(|s| Sample {
                pos: s.pos.max(0) as u64,
                out_ms: s.out_ms,
                disc: s.disc != 0,
            })
            .collect();
        unsafe { zr_free(ptr.cast()) };
        v
    };
    Ok(Scan {
        samples,
        duration_ms,
    })
}

/// Copies `input` into a Matroska file at `output`, adding the clock track
/// when `clock` is given (its ASS header and cues).
pub fn remux(input: &Path, output: &Path, clock: Option<(&str, &[Cue])>) -> Result<()> {
    let inp = c_path(input)?;
    let out = c_path(output)?;
    let header = match clock {
        Some((h, _)) => Some(CString::new(h).map_err(|_| anyhow!("a NUL in the ASS header"))?),
        None => None,
    };
    let texts: Vec<CString> = clock
        .map(|(_, cues)| {
            cues.iter()
                .map(|c| CString::new(c.text.as_str()).unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    let cues: Vec<ZrCue> = clock
        .map(|(_, cues)| cues)
        .unwrap_or_default()
        .iter()
        .zip(&texts)
        .map(|(c, t)| ZrCue {
            start_ms: c.start_ms,
            end_ms: c.end_ms,
            text: t.as_ptr(),
        })
        .collect();
    let mut err = [0 as c_char; 512];
    // SAFETY: the strings and the cue array outlive the call, which reads
    // them and keeps nothing.
    let r = unsafe {
        zr_remux(
            inp.as_ptr(),
            out.as_ptr(),
            header.as_ref().map_or(std::ptr::null(), |h| h.as_ptr()),
            cues.as_ptr(),
            cues.len(),
            err.as_mut_ptr(),
            err.len(),
        )
    };
    if r < 0 {
        return Err(anyhow!(error(&err)));
    }
    Ok(())
}
