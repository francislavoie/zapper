//! A live feed remuxed for a browser as it arrives, through `livemux.c`:
//! the video copied, one audio stream copied or encoded to AAC (see
//! [`super::tsprobe`] for when). The C side reads and writes through two
//! callbacks into this file, on a thread of its own; the async side sends
//! chunks in and reads remuxed chunks out.

use std::ffi::{c_char, c_int, c_void, CStr};
use std::sync::OnceLock;

use bytes::Bytes;
use tokio::sync::mpsc;

type ReadFn = extern "C" fn(*mut c_void, *mut u8, c_int) -> c_int;
type WriteFn = extern "C" fn(*mut c_void, *const u8, c_int) -> c_int;

extern "C" {
    fn zl_can_encode_aac() -> c_int;
    fn zl_run(
        read: ReadFn,
        write: WriteFn,
        opaque: *mut c_void,
        audio_pid: c_int,
        encode: c_int,
        err: *mut c_char,
        errlen: usize,
    ) -> c_int;
}

/// Whether the FFmpeg this was linked with has its AAC encoder. zapper's
/// own builds do from libmpv-…-r2 on; an older bundle or a system FFmpeg
/// built without it cannot convert audio, and a channel whose sound needs
/// converting then plays without it.
pub fn can_encode_aac() -> bool {
    static CAN: OnceLock<bool> = OnceLock::new();
    // SAFETY: a lookup in FFmpeg's static codec table.
    *CAN.get_or_init(|| unsafe { zl_can_encode_aac() } != 0)
}

/// Chunks queued into the remuxer, and out of it.
const QUEUE: usize = 256;

struct Pipe {
    input: mpsc::Receiver<Bytes>,
    pending: Bytes,
    output: mpsc::Sender<Bytes>,
}

extern "C" fn read_cb(opaque: *mut c_void, buf: *mut u8, size: c_int) -> c_int {
    // SAFETY: `opaque` is the `Pipe` `run` owns for the whole of `zl_run`,
    // used by this thread alone; `buf` holds `size` bytes.
    let p = unsafe { &mut *(opaque as *mut Pipe) };
    if p.pending.is_empty() {
        match p.input.blocking_recv() {
            Some(b) => p.pending = b,
            None => return 0,
        }
    }
    let n = p.pending.len().min(size.max(0) as usize);
    let chunk = p.pending.split_to(n);
    // SAFETY: as above; `n` is within both.
    unsafe { std::ptr::copy_nonoverlapping(chunk.as_ptr(), buf, n) };
    n as c_int
}

extern "C" fn write_cb(opaque: *mut c_void, buf: *const u8, size: c_int) -> c_int {
    // SAFETY: as in `read_cb`; `buf` holds `size` bytes.
    let p = unsafe { &mut *(opaque as *mut Pipe) };
    let data = unsafe { std::slice::from_raw_parts(buf, size.max(0) as usize) };
    match p.output.blocking_send(Bytes::copy_from_slice(data)) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

/// A remuxer running.
pub struct Remuxer {
    /// Chunks of the feed in. Dropping it ends the remuxer's input.
    pub input: mpsc::Sender<Bytes>,
    /// What comes out. Dropping it makes the remuxer stop at its next write.
    pub output: mpsc::Receiver<Bytes>,
}

/// Starts the remuxer on a thread of its own, keeping audio stream
/// `audio_pid` (copied, or with `encode` encoded to AAC; none with `None`).
/// It runs until its input ends or its output is dropped, and logs why it
/// stopped when that was a failure.
pub fn start(audio_pid: Option<u16>, encode: bool) -> std::io::Result<Remuxer> {
    let (in_tx, in_rx) = mpsc::channel(QUEUE);
    let (out_tx, out_rx) = mpsc::channel(QUEUE);
    std::thread::Builder::new()
        .name("livemux".into())
        .spawn(move || {
            let mut pipe = Pipe {
                input: in_rx,
                pending: Bytes::new(),
                output: out_tx,
            };
            let mut err = [0 as c_char; 512];
            // SAFETY: the callbacks and `pipe` outlive the call, which uses
            // them on this thread only; the error buffer is zeroed and its
            // length given.
            let r = unsafe {
                zl_run(
                    read_cb,
                    write_cb,
                    &mut pipe as *mut Pipe as *mut c_void,
                    audio_pid.map_or(-1, c_int::from),
                    c_int::from(encode),
                    err.as_mut_ptr(),
                    err.len(),
                )
            };
            if r < 0 && !pipe.output.is_closed() {
                // SAFETY: the C side NUL-terminates within the buffer.
                let why = unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy();
                tracing::warn!("remuxing for a browser stopped: {why}");
            }
        })?;
    Ok(Remuxer {
        input: in_tx,
        output: out_rx,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::tsprobe::{Audio, Tables, Video};

    /// A few seconds of test pattern with AC-3 sound, made with the ffmpeg
    /// command when there is one (its output is ours to use: a pattern and
    /// a tone). `None` without it.
    fn ac3_fixture() -> Option<Vec<u8>> {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("zapper-livemux-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).ok()?;
        let out = dir.join("ac3.ts");
        let made = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(["-f", "lavfi", "-i", "testsrc2=size=320x180:rate=25"])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"])
            .args(["-t", "4", "-map", "0:v", "-map", "1:a"])
            .args(["-c:v", "mpeg2video", "-g", "25", "-b:v", "500k"])
            .args(["-c:a", "ac3", "-ac", "6", "-b:a", "384k", "-f", "mpegts"])
            .arg(&out)
            .status()
            .ok()?
            .success();
        let bytes = made.then(|| std::fs::read(&out).ok()).flatten();
        let _ = std::fs::remove_dir_all(&dir);
        bytes
    }

    #[tokio::test]
    async fn ac3_sound_comes_out_as_aac_and_the_picture_as_it_was() {
        let Some(ts) = ac3_fixture() else {
            eprintln!("skipped: no ffmpeg command to make an AC-3 fixture with");
            return;
        };
        if !can_encode_aac() {
            eprintln!("skipped: this FFmpeg has no AAC encoder");
            return;
        }
        let mut src = Tables::new();
        let info = src.feed(&ts).expect("the fixture has a PMT").clone();
        assert_eq!(info.audio[0].codec, Audio::Ac3);
        let pid = info.audio[0].pid;

        let Remuxer { input, mut output } = start(Some(pid), true).unwrap();
        let feeder = tokio::spawn(async move {
            for c in ts.chunks(188 * 50) {
                if input.send(Bytes::copy_from_slice(c)).await.is_err() {
                    return;
                }
            }
        });
        let mut out = Vec::new();
        while let Some(b) = tokio::time::timeout(std::time::Duration::from_secs(20), output.recv())
            .await
            .expect("the remuxer finishes")
        {
            out.extend_from_slice(&b);
        }
        feeder.await.unwrap();
        assert!(out.len() > 50_000, "{} bytes out", out.len());
        let mut t = Tables::new();
        let got = t.feed(&out).expect("the output has a PMT").clone();
        assert_eq!(
            got.video.map(|v| v.1),
            Some(Video::Mpeg2),
            "the video is copied"
        );
        assert_eq!(got.audio.len(), 1);
        assert_eq!(got.audio[0].codec, Audio::Aac, "the sound is AAC");
        // ADTS frames (0xFFF sync) on the audio PID: really encoded, not
        // just relabelled.
        let apid = got.audio[0].pid;
        let adts = out
            .chunks(188)
            .filter(|p| p.len() == 188 && (u16::from(p[1] & 0x1f) << 8 | u16::from(p[2])) == apid)
            .count();
        assert!(adts > 20, "{adts} audio packets");
    }

    #[tokio::test]
    async fn the_remuxer_stops_when_its_reader_leaves() {
        let Some(ts) = ac3_fixture() else {
            eprintln!("skipped: no ffmpeg command to make a fixture with");
            return;
        };
        let Remuxer { input, mut output } = start(None, false).unwrap();
        // Feed it forever, until it stops taking.
        let feeder = tokio::spawn(async move {
            loop {
                for c in ts.chunks(188 * 50) {
                    if input.send(Bytes::copy_from_slice(c)).await.is_err() {
                        return;
                    }
                }
            }
        });
        assert!(output.recv().await.is_some(), "something comes out");
        drop(output);
        tokio::time::timeout(std::time::Duration::from_secs(20), feeder)
            .await
            .expect("the remuxer stops taking input once nobody reads")
            .unwrap();
    }
}
