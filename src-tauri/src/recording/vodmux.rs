//! A movie file remuxed for a browser from any point in it, through
//! `vodmux.c`: the video copied, one audio stream copied or encoded to AAC,
//! MPEG-TS out starting at zero, and the file's text subtitles handed over
//! as WebVTT cues from the same read. The C side reads the file through
//! [`Upstream`] (with seeks, so playing from the middle does not read what
//! comes before) and writes through a channel, on a thread of its own.

use std::ffi::{c_char, c_int, c_void, CStr};

use bytes::Bytes;
use tokio::sync::{mpsc, oneshot};

/// Where the movie's bytes come from: a provider connection in service, a
/// buffer in the tests. Called on the remuxer's thread, so it may block.
pub trait Upstream: Send + 'static {
    /// Reads from the current position; `Ok(0)` is the end of the file.
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize>;
    /// Moves the position to `pos` bytes from the start.
    fn seek(&mut self, pos: u64) -> std::io::Result<()>;
    /// The file's length, when known.
    fn size(&self) -> Option<u64>;
}

/// An audio stream of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioStream {
    /// Its number in the file.
    pub index: usize,
    /// FFmpeg's name for the codec: `ac3`, `aac`, `dts`, …
    pub codec: String,
    /// As the file declares it (ISO 639-2 mostly).
    pub lang: Option<String>,
    pub channels: Option<u32>,
}

/// A subtitle track of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtitleStream {
    /// Its number in the file; [`Cue::track`] refers to it.
    pub index: usize,
    /// FFmpeg's name for the codec: `subrip`, `ass`, `hdmv_pgs_subtitle`, …
    pub codec: String,
    pub lang: Option<String>,
    /// Its name in the file ("Forced", "SDH", …), when it has one.
    pub title: Option<String>,
    /// Text (`subrip`, `ass`, `ssa`, `mov_text`, `webvtt`), which comes out as
    /// cues; a bitmap track (DVD, Blu-ray, DVB) does not.
    pub text: bool,
}

/// What is in the file.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaInfo {
    /// In seconds, when the file says.
    pub duration: Option<f64>,
    pub video_codec: Option<String>,
    pub audio: Vec<AudioStream>,
    pub subtitles: Vec<SubtitleStream>,
}

/// What to do with it: the audio stream to keep (its [`AudioStream::index`];
/// `None` for no sound) and whether to encode it to AAC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VodPlan {
    pub audio: Option<usize>,
    pub encode: bool,
}

/// One line of subtitles, on the output's timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    /// The subtitle track ([`SubtitleStream::index`]).
    pub track: usize,
    /// Seconds from the output's zero.
    pub start: f64,
    pub end: f64,
    /// A WebVTT cue's payload: lines separated by `\n`, `&`, `<` and `>`
    /// escaped, bold, italic and underline as `<b>`, `<i>`, `<u>`.
    pub text: String,
}

impl Cue {
    /// The cue as a block of a WebVTT file, blank line after.
    pub fn to_webvtt(&self) -> String {
        format!(
            "{} --> {}\n{}\n\n",
            timestamp(self.start),
            timestamp(self.end),
            self.text
        )
    }
}

fn timestamp(seconds: f64) -> String {
    let ms = (seconds.max(0.0) * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

/// A remuxer running.
pub struct Vodmux {
    /// MPEG-TS, from the output's zero. Dropping it makes the remuxer stop
    /// at its next write.
    pub output: mpsc::Receiver<Bytes>,
    /// Where in the file, in seconds, the output's zero is: the keyframe at or
    /// before the requested time (up to a GOP early), which the output starts
    /// on. Sent before the first output byte; the page's clock, seek bar and
    /// resume position add it to the output's time. It never comes (the
    /// receiver errors) if the remuxer ends before the output starts.
    pub start: oneshot::Receiver<f64>,
    /// The file's text subtitles as the file is read: every track at once,
    /// each cue sent as it is decoded (a track's cues come in order, a few
    /// seconds ahead of the picture they belong to). Closes when the remuxer
    /// ends.
    pub cues: mpsc::UnboundedReceiver<Cue>,
}

type Plan = Box<dyn FnOnce(&MediaInfo) -> Result<VodPlan, String> + Send>;

type ReadFn = extern "C" fn(*mut c_void, *mut u8, c_int) -> c_int;
type WriteFn = extern "C" fn(*mut c_void, *const u8, c_int) -> c_int;
type SeekFn = extern "C" fn(*mut c_void, i64) -> i64;
type SizeFn = extern "C" fn(*mut c_void) -> i64;
type StreamFn = extern "C" fn(
    *mut c_void,
    c_int,
    c_int,
    *const c_char,
    *const c_char,
    *const c_char,
    c_int,
);
type PlanFn = extern "C" fn(*mut c_void, f64, *const c_char, *mut c_int, *mut c_int) -> c_int;
type StartFn = extern "C" fn(*mut c_void, f64);
type CueFn = extern "C" fn(*mut c_void, c_int, i64, i64, *const c_char);

/// `zv_run`'s result for a plan that said no.
const REFUSED: c_int = 1;

extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn zv_run(
        read: ReadFn,
        seek: SeekFn,
        size: SizeFn,
        write: WriteFn,
        stream: StreamFn,
        plan: PlanFn,
        start: StartFn,
        cue: CueFn,
        opaque: *mut c_void,
        from: f64,
        err: *mut c_char,
        errlen: usize,
    ) -> c_int;
}

/// Chunks queued out of the remuxer: enough to ride out a slow reader, few
/// enough that a reader who leaves is noticed soon.
const QUEUE: usize = 64;

struct State {
    input: Box<dyn Upstream>,
    output: mpsc::Sender<Bytes>,
    cues: mpsc::UnboundedSender<Cue>,
    plan: Option<Plan>,
    info: Option<oneshot::Sender<Result<MediaInfo, String>>>,
    start: Option<oneshot::Sender<f64>>,
    audio: Vec<AudioStream>,
    subtitles: Vec<SubtitleStream>,
}

/// # Safety
/// `opaque` is the `State` `spawn`'s thread owns for the whole of `zv_run`,
/// used by that thread alone.
unsafe fn state<'a>(opaque: *mut c_void) -> &'a mut State {
    &mut *(opaque as *mut State)
}

/// Runs a callback's Rust side, answering `or` should it panic: a panic must
/// not unwind into C (and out of an `extern "C"` function, which aborts the
/// whole service).
fn guard<T>(or: T, f: impl FnOnce() -> T) -> T {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|_| {
        tracing::warn!("a movie remuxer's callback panicked");
        or
    })
}

fn text(p: *const c_char) -> String {
    // SAFETY: the C side passes NUL-terminated strings.
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

fn some(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

extern "C" fn read_cb(opaque: *mut c_void, buf: *mut u8, size: c_int) -> c_int {
    guard(-1, || {
        // SAFETY: see `state`; `buf` holds `size` bytes.
        let (s, buf) = unsafe {
            (
                state(opaque),
                std::slice::from_raw_parts_mut(buf, size.max(0) as usize),
            )
        };
        match s.input.read(buf) {
            Ok(n) => n as c_int,
            Err(_) => -1,
        }
    })
}

extern "C" fn write_cb(opaque: *mut c_void, buf: *const u8, size: c_int) -> c_int {
    guard(-1, || {
        // SAFETY: see `state`; `buf` holds `size` bytes.
        let (s, data) = unsafe {
            (
                state(opaque),
                std::slice::from_raw_parts(buf, size.max(0) as usize),
            )
        };
        match s.output.blocking_send(Bytes::copy_from_slice(data)) {
            Ok(()) => 0,
            Err(_) => -1,
        }
    })
}

extern "C" fn seek_cb(opaque: *mut c_void, pos: i64) -> i64 {
    guard(-1, || {
        // SAFETY: see `state`.
        let s = unsafe { state(opaque) };
        match u64::try_from(pos).ok().map(|p| s.input.seek(p)) {
            Some(Ok(())) => pos,
            _ => -1,
        }
    })
}

extern "C" fn size_cb(opaque: *mut c_void) -> i64 {
    guard(-1, || {
        // SAFETY: see `state`.
        let s = unsafe { state(opaque) };
        s.input.size().and_then(|n| i64::try_from(n).ok()).unwrap_or(-1)
    })
}

extern "C" fn stream_cb(
    opaque: *mut c_void,
    kind: c_int,
    index: c_int,
    codec: *const c_char,
    lang: *const c_char,
    title: *const c_char,
    n: c_int,
) {
    guard((), || {
        // SAFETY: see `state`.
        let s = unsafe { state(opaque) };
        let (index, codec, lang) = (index as usize, text(codec), some(text(lang)));
        if kind == 0 {
            s.audio.push(AudioStream {
                index,
                codec,
                lang,
                channels: u32::try_from(n).ok().filter(|&n| n > 0),
            });
        } else {
            s.subtitles.push(SubtitleStream {
                index,
                codec,
                lang,
                title: some(text(title)),
                text: kind == 1,
            });
        }
    })
}

extern "C" fn plan_cb(
    opaque: *mut c_void,
    duration: f64,
    video: *const c_char,
    audio: *mut c_int,
    encode: *mut c_int,
) -> c_int {
    // SAFETY: see `state`; `audio` and `encode` are the C side's to be set.
    let s = unsafe { state(opaque) };
    let info = MediaInfo {
        duration: (duration >= 0.0).then_some(duration),
        video_codec: some(text(video)),
        audio: std::mem::take(&mut s.audio),
        subtitles: std::mem::take(&mut s.subtitles),
    };
    let (Some(plan), Some(tx)) = (s.plan.take(), s.info.take()) else {
        return REFUSED;
    };
    let decided = guard(Err("the plan failed".to_string()), || plan(&info));
    match decided {
        Ok(p) => {
            // Before the first byte of output, which only now can start.
            let _ = tx.send(Ok(info));
            // SAFETY: as above.
            unsafe {
                *audio = p.audio.map_or(-1, |a| a as c_int);
                *encode = c_int::from(p.encode);
            }
            0
        }
        Err(why) => {
            let _ = tx.send(Err(why));
            REFUSED
        }
    }
}

extern "C" fn start_cb(opaque: *mut c_void, secs: f64) {
    guard((), || {
        // SAFETY: see `state`.
        let s = unsafe { state(opaque) };
        if let Some(tx) = s.start.take() {
            let _ = tx.send(secs);
        }
    })
}

extern "C" fn cue_cb(opaque: *mut c_void, track: c_int, start: i64, end: i64, body: *const c_char) {
    guard((), || {
        // SAFETY: see `state`.
        let s = unsafe { state(opaque) };
        let _ = s.cues.send(Cue {
            track: track as usize,
            start: start as f64 / 1e6,
            end: end as f64 / 1e6,
            text: text(body),
        });
    })
}

/// Starts the remuxer on a thread of its own. It opens `input`, reports what
/// is in it (or why it could not be read) on the returned channel, asks
/// `plan` what to keep (an `Err` is reported the same way and nothing is
/// written), seeks to `from` seconds when that is past the start, and writes
/// the output from the first keyframe there, timestamps from zero. It runs
/// until the file ends or the output is dropped, and logs why it stopped
/// when that was a failure.
pub fn spawn(
    input: Box<dyn Upstream>,
    from: f64,
    plan: impl FnOnce(&MediaInfo) -> Result<VodPlan, String> + Send + 'static,
) -> (oneshot::Receiver<Result<MediaInfo, String>>, Vodmux) {
    let (out_tx, out_rx) = mpsc::channel(QUEUE);
    let (cue_tx, cue_rx) = mpsc::unbounded_channel();
    let (info_tx, info_rx) = oneshot::channel();
    let (start_tx, start_rx) = oneshot::channel();
    let mut state = State {
        input,
        output: out_tx,
        cues: cue_tx,
        plan: Some(Box::new(plan)),
        info: Some(info_tx),
        start: Some(start_tx),
        audio: Vec::new(),
        subtitles: Vec::new(),
    };
    let spawned = std::thread::Builder::new().name("vodmux".into()).spawn(move || {
        let mut err = [0 as c_char; 512];
        // SAFETY: the callbacks and `state` outlive the call, which uses
        // them on this thread only; the error buffer is zeroed and its
        // length given.
        let r = unsafe {
            zv_run(
                read_cb,
                seek_cb,
                size_cb,
                write_cb,
                stream_cb,
                plan_cb,
                start_cb,
                cue_cb,
                &mut state as *mut State as *mut c_void,
                from.max(0.0),
                err.as_mut_ptr(),
                err.len(),
            )
        };
        if r < 0 {
            // SAFETY: the C side NUL-terminates within the buffer.
            let why = unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy();
            match state.info.take() {
                // It never got as far as the plan.
                Some(tx) => {
                    let _ = tx.send(Err(why.into_owned()));
                }
                None if !state.output.is_closed() => {
                    tracing::warn!("remuxing a movie for a browser stopped: {why}");
                }
                None => {}
            }
        }
    });
    if let Err(e) = spawned {
        // The closure (and with it the channel's sender) is gone: the
        // receiver reports it as closed.
        tracing::warn!("starting the movie remuxer: {e}");
    }
    (
        info_rx,
        Vodmux {
            output: out_rx,
            start: start_rx,
            cues: cue_rx,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::livemux::can_encode_aac;
    use crate::recording::tsprobe::{Audio, Tables, Video};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// A file in memory that remembers where it was seeked to.
    struct Mem {
        data: Arc<Vec<u8>>,
        pos: usize,
        seeks: Arc<Mutex<Vec<u64>>>,
    }

    impl Upstream for Mem {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = buf.len().min(self.data.len().saturating_sub(self.pos));
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
        fn seek(&mut self, pos: u64) -> std::io::Result<()> {
            self.seeks.lock().unwrap().push(pos);
            self.pos = pos as usize;
            Ok(())
        }
        fn size(&self) -> Option<u64> {
            Some(self.data.len() as u64)
        }
    }

    fn mem(data: &[u8]) -> (Box<Mem>, Arc<Mutex<Vec<u64>>>) {
        let seeks = Arc::new(Mutex::new(Vec::new()));
        (
            Box::new(Mem {
                data: Arc::new(data.to_vec()),
                pos: 0,
                seeks: seeks.clone(),
            }),
            seeks,
        )
    }

    const SUBTITLES: &str = "1\n00:00:01,000 --> 00:00:03,000\nHello & 1 < 2\n\n\
                             2\n00:00:12,000 --> 00:00:14,000\n<i>Later</i>\non two lines\n\n";

    /// A 20 s test pattern with a tone and two subtitle cues, in a Matroska
    /// (AC-3 sound, SubRip subtitles) or an MP4 (AAC sound, `mov_text`),
    /// made with the ffmpeg command when there is one. `busy` adds noise so
    /// the video is big enough to outrun the output queue. `None` without
    /// ffmpeg.
    fn fixture(mkv: bool, busy: bool) -> Option<Vec<u8>> {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("zapper-vodmux-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).ok()?;
        let srt = dir.join("subs.srt");
        std::fs::write(&srt, SUBTITLES).ok()?;
        let out = dir.join(if mkv { "movie.mkv" } else { "movie.mp4" });
        let video = if busy {
            "testsrc2=size=640x360:rate=25,noise=alls=80:allf=t"
        } else {
            "testsrc2=size=320x180:rate=25"
        };
        let (audio, sub) = if mkv { ("ac3", "srt") } else { ("aac", "mov_text") };
        let made = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(["-f", "lavfi", "-i", video])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"])
            .arg("-i")
            .arg(&srt)
            .args(["-t", "20", "-map", "0:v", "-map", "1:a", "-map", "2:s"])
            .args(["-c:v", "libx264", "-g", "25", "-pix_fmt", "yuv420p"])
            .args(["-b:v", if busy { "8M" } else { "300k" }])
            .args(["-c:a", audio, "-ac", "6", "-c:s", sub])
            .arg(&out)
            .status()
            .ok()?
            .success();
        let bytes = made.then(|| std::fs::read(&out).ok()).flatten();
        let _ = std::fs::remove_dir_all(&dir);
        bytes
    }

    fn encode_plan(info: &MediaInfo) -> Result<VodPlan, String> {
        Ok(VodPlan {
            audio: info.audio.first().map(|a| a.index),
            encode: true,
        })
    }

    async fn all(output: &mut mpsc::Receiver<Bytes>) -> Vec<u8> {
        let mut out = Vec::new();
        while let Some(b) = tokio::time::timeout(Duration::from_secs(30), output.recv())
            .await
            .expect("the remuxer finishes")
        {
            out.extend_from_slice(&b);
        }
        out
    }

    /// The presentation time, in seconds, of the first picture on `pid`.
    fn first_pts(ts: &[u8], pid: u16) -> Option<f64> {
        for p in ts.chunks_exact(188) {
            if u16::from(p[1] & 0x1f) << 8 | u16::from(p[2]) != pid || p[1] & 0x40 == 0 {
                continue;
            }
            let mut at = 4;
            if p[3] & 0x20 != 0 {
                at += 1 + usize::from(p[4]);
            }
            let pes = p.get(at..at + 14)?;
            if pes[..3] != [0, 0, 1] || pes[7] & 0x80 == 0 {
                continue;
            }
            let b = &pes[9..14];
            let pts = u64::from(b[0] >> 1 & 7) << 30
                | u64::from(b[1]) << 22
                | u64::from(b[2] >> 1) << 15
                | u64::from(b[3]) << 7
                | u64::from(b[4] >> 1);
            return Some(pts as f64 / 90_000.0);
        }
        None
    }

    #[tokio::test]
    async fn an_mkv_with_ac3_comes_out_as_ts_with_aac() {
        let Some(file) = fixture(true, false) else {
            eprintln!("skipped: no ffmpeg command to make a fixture with");
            return;
        };
        if !can_encode_aac() {
            eprintln!("skipped: this FFmpeg has no AAC encoder");
            return;
        }
        let (input, _) = mem(&file);
        let (info, mut vod) = spawn(input, 0.0, encode_plan);
        let info = info.await.unwrap().expect("it opens");
        let duration = info.duration.expect("the file has a duration");
        assert!((duration - 20.0).abs() < 1.0, "{duration} s");
        assert_eq!(info.video_codec.as_deref(), Some("h264"));
        assert_eq!(info.audio.len(), 1);
        assert_eq!(info.audio[0].codec, "ac3");
        assert_eq!(info.audio[0].channels, Some(6));
        assert_eq!(info.subtitles.len(), 1);
        assert_eq!(info.subtitles[0].codec, "subrip");
        assert!(info.subtitles[0].text);

        let out = all(&mut vod.output).await;
        let mut t = Tables::new();
        let got = t.feed(&out).expect("the output has a PMT").clone();
        assert_eq!(got.video.map(|v| v.1), Some(Video::H264));
        assert_eq!(got.audio.len(), 1, "one audio PID, and nothing for subtitles");
        assert_eq!(got.audio[0].codec, Audio::Aac);
        let apid = got.audio[0].pid;
        let aac = out
            .chunks_exact(188)
            .filter(|p| (u16::from(p[1] & 0x1f) << 8 | u16::from(p[2])) == apid)
            .count();
        assert!(aac > 20, "{aac} audio packets");
    }

    #[tokio::test]
    async fn text_subtitles_come_out_as_cues_on_the_outputs_timeline() {
        let Some(file) = fixture(true, false) else {
            eprintln!("skipped: no ffmpeg command to make a fixture with");
            return;
        };
        let (input, _) = mem(&file);
        let (info, mut vod) = spawn(input, 0.0, |_| Ok(VodPlan { audio: None, encode: false }));
        let info = info.await.unwrap().unwrap();
        let track = info.subtitles[0].index;
        let _ = all(&mut vod.output).await;
        let mut cues = Vec::new();
        while let Some(c) = vod.cues.recv().await {
            cues.push(c);
        }
        assert_eq!(cues.len(), 2, "{cues:?}");
        assert_eq!(cues[0].track, track);
        assert!((cues[0].start - 1.0).abs() < 0.1 && (cues[0].end - 3.0).abs() < 0.1);
        assert_eq!(cues[0].text, "Hello &amp; 1 &lt; 2");
        assert_eq!(cues[1].text, "<i>Later</i>\non two lines");
        assert_eq!(
            cues[0].to_webvtt(),
            "00:00:01.000 --> 00:00:03.000\nHello &amp; 1 &lt; 2\n\n"
        );
    }

    #[tokio::test]
    async fn starting_at_ten_seconds_seeks_rather_than_reading_from_the_start() {
        let Some(file) = fixture(false, false) else {
            eprintln!("skipped: no ffmpeg command to make a fixture with");
            return;
        };
        let (input, seeks) = mem(&file);
        // Between keyframes (one a second): the output starts on the one before.
        let (info, mut vod) = spawn(input, 10.5, |info| {
            Ok(VodPlan {
                audio: info.audio.first().map(|a| a.index),
                encode: false,
            })
        });
        let info = info.await.unwrap().expect("it opens");
        assert_eq!(info.audio[0].codec, "aac");
        assert_eq!(info.subtitles[0].codec, "mov_text");
        let start = vod.start.await.expect("the start is reported");
        assert!(start <= 10.5 && start > 9.4, "the output's zero is at {start} s");
        let out = all(&mut vod.output).await;
        let past_half = seeks
            .lock()
            .unwrap()
            .iter()
            .filter(|&&p| p > file.len() as u64 / 2)
            .count();
        assert!(past_half > 0, "seeks: {:?}", seeks.lock().unwrap());

        let mut t = Tables::new();
        let got = t.feed(&out).expect("a PMT").clone();
        let vpid = got.video.expect("video").0;
        // The muxer starts its clock a little after zero; ten seconds in
        // would be nowhere near.
        let pts = first_pts(&out, vpid).expect("a picture");
        assert!(pts < 2.0, "the first picture is at {pts} s");
        // Half the movie is 10 s of 25 fps pictures; allow for the
        // keyframe before.
        let video = out
            .chunks_exact(188)
            .filter(|p| (u16::from(p[1] & 0x1f) << 8 | u16::from(p[2])) == vpid && p[1] & 0x40 != 0)
            .count();
        assert!((240..=290).contains(&video), "{video} pictures");

        // The cue at 12 s is as far into the output as the output's zero is
        // from it.
        let cue = vod.cues.recv().await.expect("the later cue");
        assert!((cue.start - (12.0 - start)).abs() < 0.1, "starts at {}", cue.start);
        assert!(vod.cues.recv().await.is_none(), "the cue before the start is not sent");
    }

    #[tokio::test]
    async fn a_refused_plan_reports_its_reason_and_writes_nothing() {
        let Some(file) = fixture(false, false) else {
            eprintln!("skipped: no ffmpeg command to make a fixture with");
            return;
        };
        let (input, _) = mem(&file);
        let (info, mut vod) = spawn(input, 0.0, |_| Err("no browser here plays this video".into()));
        assert_eq!(
            info.await.unwrap().unwrap_err(),
            "no browser here plays this video"
        );
        assert!(all(&mut vod.output).await.is_empty());
    }

    #[tokio::test]
    async fn something_that_is_not_a_movie_reports_why() {
        let (input, _) = mem(&[7u8; 4096]);
        let (info, mut vod) = spawn(input, 0.0, encode_plan);
        let why = info.await.unwrap().unwrap_err();
        assert!(why.contains("reading the file"), "{why}");
        assert!(all(&mut vod.output).await.is_empty());
    }

    #[tokio::test]
    async fn a_panicking_plan_is_an_error_not_an_abort() {
        let Some(file) = fixture(false, false) else {
            eprintln!("skipped: no ffmpeg command to make a fixture with");
            return;
        };
        let (input, _) = mem(&file);
        let (info, mut vod) = spawn(input, 0.0, |_| panic!("plan bug"));
        assert!(info.await.unwrap().is_err());
        assert!(all(&mut vod.output).await.is_empty());
    }

    #[tokio::test]
    async fn drawing_commands_and_override_blocks_are_not_subtitle_text() {
        static N: AtomicUsize = AtomicUsize::new(1000);
        let dir = std::env::temp_dir().join(format!(
            "zapper-vodmux-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let ass = dir.join("subs.ass");
        let events = [
            r"Dialogue: 0,0:00:01.00,0:00:03.00,Default,,0,0,0,,Seen{\p1}m 0 0 l 10 0 10 10{\p0} here",
            r"Dialogue: 0,0:00:04.00,0:00:06.00,Default,,0,0,0,,{\p1}m 0 0 l 5 5",
            r"Dialogue: 0,0:00:07.00,0:00:09.00,Default,,0,0,0,,{\pos(5,5)\b1}Bold{\b0} and {\i1}plain{\unclosed",
        ];
        std::fs::write(
            &ass,
            format!(
                "[Script Info]\nScriptType: v4.00+\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize\nStyle: Default,Arial,20\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n{}\n",
                events.join("\n")
            ),
        )
        .unwrap();
        let out = dir.join("movie.mkv");
        let made = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(["-f", "lavfi", "-i", "testsrc2=size=160x90:rate=25"])
            .arg("-i")
            .arg(&ass)
            .args(["-t", "10", "-map", "0:v", "-map", "1:s"])
            .args(["-c:v", "libx264", "-g", "25", "-pix_fmt", "yuv420p", "-c:s", "ass"])
            .arg(&out)
            .status();
        let file = made.ok().filter(|s| s.success()).and_then(|_| std::fs::read(&out).ok());
        let _ = std::fs::remove_dir_all(&dir);
        let Some(file) = file else {
            eprintln!("skipped: no ffmpeg command to make a fixture with");
            return;
        };
        let (input, _) = mem(&file);
        let (info, mut vod) = spawn(input, 0.0, |_| Ok(VodPlan { audio: None, encode: false }));
        assert_eq!(info.await.unwrap().unwrap().subtitles[0].codec, "ass");
        let _ = all(&mut vod.output).await;
        let mut texts = Vec::new();
        while let Some(c) = vod.cues.recv().await {
            texts.push(c.text);
        }
        assert_eq!(texts, ["Seen here", "<b>Bold</b> and <i>plain</i>"], "{texts:?}");
    }

    #[tokio::test]
    async fn the_remuxer_stops_when_its_reader_leaves() {
        let Some(file) = fixture(false, true) else {
            eprintln!("skipped: no ffmpeg command to make a fixture with");
            return;
        };
        let (input, _) = mem(&file);
        let (info, mut vod) = spawn(input, 0.0, |_| Ok(VodPlan { audio: None, encode: false }));
        info.await.unwrap().unwrap();
        assert!(vod.output.recv().await.is_some(), "something comes out");
        drop(vod.output);
        // The cues close when the thread's state is dropped, as it ends.
        tokio::time::timeout(Duration::from_secs(5), async {
            while vod.cues.recv().await.is_some() {}
        })
        .await
        .expect("the remuxer stops once nobody reads");
    }
}
