//! What a channel's stream carries, read from its own tables, and what a
//! browser needs done to it.
//!
//! An MPEG transport stream says what is in it twice a second or so: the
//! programme association table (PID 0) names the programme map table, and
//! that lists each elementary stream with its type and descriptors (its
//! language, whether it is AC-3). [`Tables`] reads those from the first
//! packets of a feed, which costs nothing but the bytes the viewer would
//! get anyway, so whether a channel can play in a browser is learnt from
//! the stream rather than guessed from its name.
//!
//! [`plan`] then decides, for one browser's abilities ([`Can`]):
//!
//! - **Video** is never re-encoded. H.264 plays everywhere Media Source
//!   Extensions do; HEVC where the browser says it can; MPEG-2 (most
//!   broadcast SD, and ATSC and many DVB feeds) nowhere, so such a channel
//!   is refused with a reason rather than shown as a black picture.
//! - **Audio** a browser plays as it comes (AAC) is passed on untouched.
//!   AC-3 and E-AC-3 are passed on where the browser decodes them; anything
//!   else (MPEG-1 Layer II above all) is encoded to AAC by the service
//!   (`livemux`), and so is AC-3 where the browser lacks it.
//! - The player in the page plays the first audio stream it finds, so any
//!   other choice of track goes through the remuxer too, which keeps only
//!   the chosen one.
//!
//! Pure: bytes in, decisions out.

use serde::Serialize;

const PACKET: usize = 188;

/// A video codec, as the stream type and descriptors say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Video {
    Mpeg2,
    H264,
    Hevc,
    Other,
}

impl Video {
    pub fn name(self) -> &'static str {
        match self {
            Video::Mpeg2 => "MPEG-2",
            Video::H264 => "H.264",
            Video::Hevc => "HEVC (H.265)",
            Video::Other => "an unusual format",
        }
    }

    /// Why a browser can't play this video, as a refusal says it; `record`
    /// when recording it is the other way to have it (a channel).
    pub fn unplayable(self, record: bool) -> String {
        format!(
            "Its video is {}, which {} can’t play. Watch it in the zapper app{}.",
            self.name(),
            if self == Video::Hevc {
                "this browser"
            } else {
                "browsers"
            },
            if record { ", or record it" } else { "" }
        )
    }
}

/// An audio codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Audio {
    Aac,
    /// AAC in LATM/LOAS framing (DVB-T2 in places).
    Latm,
    Ac3,
    Eac3,
    /// MPEG-1/2 audio, Layer II on nearly every broadcast.
    Mpeg,
    Other,
}

impl Audio {
    pub fn name(self) -> &'static str {
        match self {
            Audio::Aac | Audio::Latm => "aac",
            Audio::Ac3 => "ac3",
            Audio::Eac3 => "eac3",
            Audio::Mpeg => "mp2",
            Audio::Other => "other",
        }
    }
}

/// One audio stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AudioStream {
    pub pid: u16,
    pub codec: Audio,
    /// ISO 639-2, as the stream declares it.
    pub lang: Option<String>,
    /// Said to be audio description.
    pub described: bool,
    /// Said to be for the hard of hearing.
    pub impaired: bool,
    /// ATSC's stream types (0x81, 0x87) rather than DVB's private stream
    /// with a descriptor: the player in the page recognises only these.
    atsc: bool,
}

/// What the programme map table lists.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StreamInfo {
    pub video: Option<(u16, Video)>,
    pub audio: Vec<AudioStream>,
}

/// Reads PAT and PMT out of packets as they come.
#[derive(Default)]
pub struct Tables {
    /// Bytes left over from the last call: less than a packet.
    rest: Vec<u8>,
    pmt_pid: Option<u16>,
    /// The PMT section being put together across packets.
    section: Vec<u8>,
    found: Option<StreamInfo>,
}

impl Tables {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds bytes (whole packets from a sync byte on, as a hub hands them
    /// out); the stream's contents once a complete PMT has gone past.
    pub fn feed(&mut self, data: &[u8]) -> Option<&StreamInfo> {
        if self.found.is_some() {
            return self.found.as_ref();
        }
        let mut buf = std::mem::take(&mut self.rest);
        buf.extend_from_slice(data);
        let mut at = 0;
        while at + PACKET <= buf.len() {
            if buf[at] != 0x47 {
                // Lost sync: look for the next packet start.
                at += 1;
                continue;
            }
            self.packet(&buf[at..at + PACKET]);
            at += PACKET;
            if self.found.is_some() {
                return self.found.as_ref();
            }
        }
        self.rest = buf[at..].to_vec();
        None
    }

    fn packet(&mut self, p: &[u8]) {
        let pid = (u16::from(p[1] & 0x1f) << 8) | u16::from(p[2]);
        let start = p[1] & 0x40 != 0;
        let afc = (p[3] >> 4) & 3;
        if afc & 1 == 0 {
            return;
        }
        let mut off = 4;
        if afc == 3 {
            off += 1 + usize::from(p[4]);
        }
        if off >= PACKET {
            return;
        }
        let payload = &p[off..];
        if pid == 0 {
            if start {
                if let Some(s) = section_after_pointer(payload) {
                    self.pmt_pid = pat_pmt_pid(s).or(self.pmt_pid);
                }
            }
            return;
        }
        if Some(pid) != self.pmt_pid {
            return;
        }
        if start {
            self.section.clear();
            match section_after_pointer(payload) {
                Some(s) => self.section.extend_from_slice(s),
                None => return,
            }
        } else if !self.section.is_empty() {
            self.section.extend_from_slice(payload);
        } else {
            return;
        }
        if self.section.len() < 3 {
            return;
        }
        let len = 3 + (usize::from(self.section[1] & 0x0f) << 8 | usize::from(self.section[2]));
        if self.section.len() >= len {
            self.found = parse_pmt(&self.section[..len]);
            self.section.clear();
        }
    }
}

/// The stream time a transport stream's packets carry, read as they pass:
/// what `hub` measures a placeholder clip's length by (see
/// `crate::ipc::ending::off_air`), as mpv's cache span measures it for the
/// app.
///
/// Each PCR of the first PID that carries one, or, in a stream with no PCR,
/// each PES header's PTS on the first PID that has one: either is the
/// programme's clock, at 90 kHz. They wrap after about 26.5 hours, and are
/// unwrapped here so a wrap reads as time going on rather than going back.
#[derive(Debug, Default, Clone)]
pub struct StreamClock {
    pcr_pid: Option<u16>,
    pts_pid: Option<u16>,
    /// The last value, in 90 kHz ticks, and what has been added for wraps.
    last: Option<u64>,
    wraps: u64,
}

/// The 33-bit clock's range, in its ticks.
const CLOCK_RANGE: u64 = 1 << 33;

impl StreamClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds whole packets (from a sync byte on, as a hub hands them out);
    /// `each` is given every timestamp in them, in seconds, in order.
    pub fn feed(&mut self, data: &[u8], mut each: impl FnMut(f64)) {
        for p in data.as_chunks::<PACKET>().0 {
            if p[0] != 0x47 {
                continue;
            }
            if let Some(t) = self.packet(p) {
                each(self.unwrap(t));
            }
        }
    }

    fn packet(&mut self, p: &[u8]) -> Option<u64> {
        let pid = (u16::from(p[1] & 0x1f) << 8) | u16::from(p[2]);
        let afc = (p[3] >> 4) & 3;
        let mut payload = 4;
        if afc & 2 != 0 {
            let len = usize::from(p[4]);
            payload = 5 + len;
            // The PCR: the adaptation field's flags, then 33 bits of base
            // (90 kHz) and 9 of extension (27 MHz), which this ignores.
            if len >= 7 && p[5] & 0x10 != 0 && self.pcr_pid.is_none_or(|w| w == pid) {
                self.pcr_pid = Some(pid);
                let b = &p[6..11];
                return Some(
                    (u64::from(b[0]) << 25)
                        | (u64::from(b[1]) << 17)
                        | (u64::from(b[2]) << 9)
                        | (u64::from(b[3]) << 1)
                        | (u64::from(b[4]) >> 7),
                );
            }
        }
        if self.pcr_pid.is_some() || afc & 1 == 0 || p[1] & 0x40 == 0 {
            return None;
        }
        // A PES header starting here, with a PTS.
        let h = p.get(payload..)?;
        if h.len() < 14 || h[..3] != [0, 0, 1] || h[7] & 0x80 == 0 {
            return None;
        }
        if self.pts_pid.is_some_and(|w| w != pid) {
            return None;
        }
        self.pts_pid = Some(pid);
        let t = &h[9..14];
        Some(
            (u64::from((t[0] >> 1) & 0x07) << 30)
                | (u64::from(t[1]) << 22)
                | (u64::from(t[2] >> 1) << 15)
                | (u64::from(t[3]) << 7)
                | (u64::from(t[4]) >> 1),
        )
    }

    /// Seconds, carried across the clock's wrap: a value more than half
    /// the range below the last one is the clock starting over.
    fn unwrap(&mut self, ticks: u64) -> f64 {
        if let Some(last) = self.last {
            if ticks + CLOCK_RANGE / 2 < last {
                self.wraps += CLOCK_RANGE;
            }
        }
        self.last = Some(ticks);
        (ticks + self.wraps) as f64 / 90_000.0
    }
}

/// The section a payload unit starts, past its pointer field.
fn section_after_pointer(payload: &[u8]) -> Option<&[u8]> {
    let pointer = usize::from(*payload.first()?);
    payload.get(1 + pointer..)
}

/// The PID of the first programme's map table.
fn pat_pmt_pid(s: &[u8]) -> Option<u16> {
    if s.len() < 8 || s[0] != 0x00 {
        return None;
    }
    let len = 3 + (usize::from(s[1] & 0x0f) << 8 | usize::from(s[2]));
    let end = len.min(s.len()).checked_sub(4)?;
    let mut i = 8;
    while i + 4 <= end {
        let program = u16::from(s[i]) << 8 | u16::from(s[i + 1]);
        let pid = (u16::from(s[i + 2] & 0x1f) << 8) | u16::from(s[i + 3]);
        if program != 0 {
            return Some(pid);
        }
        i += 4;
    }
    None
}

fn parse_pmt(s: &[u8]) -> Option<StreamInfo> {
    if s.len() < 12 || s[0] != 0x02 {
        return None;
    }
    let end = s.len().checked_sub(4)?;
    let info_len = usize::from(s[10] & 0x0f) << 8 | usize::from(s[11]);
    let mut i = 12 + info_len;
    let mut out = StreamInfo::default();
    while i + 5 <= end {
        let kind = s[i];
        let pid = (u16::from(s[i + 1] & 0x1f) << 8) | u16::from(s[i + 2]);
        let es_len = usize::from(s[i + 3] & 0x0f) << 8 | usize::from(s[i + 4]);
        let desc = s.get(i + 5..(i + 5 + es_len).min(end)).unwrap_or(&[]);
        i += 5 + es_len;
        let d = Descriptors::read(desc);
        let video = match kind {
            0x01 | 0x02 => Some(Video::Mpeg2),
            0x1b => Some(Video::H264),
            0x24 => Some(Video::Hevc),
            0x10 | 0xea | 0x42 | 0xd1 | 0x33 => Some(Video::Other),
            0x06 if d.registration == Some(*b"HEVC") => Some(Video::Hevc),
            _ => None,
        };
        if let Some(v) = video {
            if out.video.is_none() {
                out.video = Some((pid, v));
            }
            continue;
        }
        let audio = match kind {
            0x03 | 0x04 => Some((Audio::Mpeg, false)),
            0x0f => Some((Audio::Aac, false)),
            0x11 => Some((Audio::Latm, false)),
            0x81 => Some((Audio::Ac3, true)),
            0x87 => Some((Audio::Eac3, true)),
            0x06 if d.ac3 || d.registration == Some(*b"AC-3") => Some((Audio::Ac3, false)),
            0x06 if d.eac3 || d.registration == Some(*b"EAC3") => Some((Audio::Eac3, false)),
            0x06 if d.aac => Some((Audio::Aac, false)),
            0x06 if d.registration == Some(*b"Opus") || d.registration == Some(*b"DTS1") => {
                Some((Audio::Other, false))
            }
            _ => None,
        };
        if let Some((codec, atsc)) = audio {
            out.audio.push(AudioStream {
                pid,
                codec,
                lang: d.lang,
                described: d.audio_type == 3,
                impaired: d.audio_type == 2,
                atsc,
            });
        }
    }
    Some(out)
}

#[derive(Default)]
struct Descriptors {
    lang: Option<String>,
    audio_type: u8,
    ac3: bool,
    eac3: bool,
    aac: bool,
    registration: Option<[u8; 4]>,
}

impl Descriptors {
    fn read(mut d: &[u8]) -> Self {
        let mut out = Self::default();
        while d.len() >= 2 {
            let (tag, len) = (d[0], usize::from(d[1]));
            let Some(body) = d.get(2..2 + len) else {
                break;
            };
            match tag {
                0x0a if len >= 4 => {
                    let lang: String = body[..3]
                        .iter()
                        .filter(|b| b.is_ascii_alphabetic())
                        .map(|&b| char::from(b.to_ascii_lowercase()))
                        .collect();
                    if lang.len() == 3 {
                        out.lang = Some(lang);
                    }
                    out.audio_type = body[3];
                }
                0x6a => out.ac3 = true,
                0x7a => out.eac3 = true,
                0x7c => out.aac = true,
                0x05 if len >= 4 => out.registration = Some([body[0], body[1], body[2], body[3]]),
                _ => {}
            }
            d = &d[2 + len..];
        }
        out
    }
}

// ---------------------------------------------------------------- plans

/// What a browser can decode, as it says (`MediaSource.isTypeSupported`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Can {
    pub hevc: bool,
    pub ac3: bool,
    pub eac3: bool,
}

/// How the stream is handed to the browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    /// As it comes.
    Pass,
    /// Through the remuxer: the video copied, and `audio` (a PID) copied
    /// or, with `encode`, encoded to AAC; no audio at all with `None`.
    Remux { audio: Option<u16>, encode: bool },
}

/// What the browser is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    Play {
        how: How,
        /// The audio stream played, an index into [`StreamInfo::audio`].
        audio: Option<usize>,
        /// Its sound is dropped: nothing here can turn it into something
        /// the browser plays. Says why.
        silent: Option<String>,
    },
    /// Why it can't play in this browser.
    Refuse(String),
}

fn needs_encoding(a: &AudioStream, can: Can) -> bool {
    match a.codec {
        Audio::Aac | Audio::Latm => false,
        Audio::Ac3 => !(can.ac3 && a.atsc),
        Audio::Eac3 => !(can.eac3 && a.atsc),
        Audio::Mpeg | Audio::Other => true,
    }
}

/// What to send a browser that `can` this much, playing audio stream
/// `want` (an index; the first when `None` or out of range). `encoder`:
/// whether this build can encode AAC.
pub fn plan(info: &StreamInfo, can: Can, want: Option<usize>, encoder: bool) -> Plan {
    match info.video {
        Some((_, Video::H264)) => {}
        Some((_, Video::Hevc)) if can.hevc => {}
        Some((_, v)) => return Plan::Refuse(v.unplayable(true)),
        None if info.audio.is_empty() => {
            return Plan::Refuse(
                "Its stream has neither video nor sound a browser can play.".into(),
            )
        }
        None => {}
    }
    if info.audio.is_empty() {
        return Plan::Play {
            how: How::Pass,
            audio: None,
            silent: None,
        };
    }
    let i = want.filter(|&i| i < info.audio.len()).unwrap_or(0);
    let a = &info.audio[i];
    let encode = needs_encoding(a, can);
    if encode && !encoder {
        return Plan::Play {
            how: How::Remux {
                audio: None,
                encode: false,
            },
            audio: None,
            silent: Some(format!(
                "Its sound ({}) can’t be played in this browser, and this zapper-server can’t convert it.",
                a.codec.name().to_uppercase()
            )),
        };
    }
    let how = if i == 0 && !encode {
        How::Pass
    } else {
        How::Remux {
            audio: Some(a.pid),
            encode,
        }
    };
    Plan::Play {
        how,
        audio: Some(i),
        silent: None,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A packet of `pid` carrying a section that starts in it.
    fn section_packet(pid: u16, section: &[u8]) -> Vec<u8> {
        let mut p = vec![0xffu8; PACKET];
        p[0] = 0x47;
        p[1] = 0x40 | (pid >> 8) as u8;
        p[2] = pid as u8;
        p[3] = 0x10;
        p[4] = 0; // pointer field
        p[5..5 + section.len()].copy_from_slice(section);
        p
    }

    fn with_length(mut s: Vec<u8>) -> Vec<u8> {
        // Section length counts from after the length field, CRC included.
        let len = s.len() - 3 + 4;
        s[1] = 0xb0 | (len >> 8) as u8;
        s[2] = len as u8;
        s.extend_from_slice(&[0, 0, 0, 0]);
        s
    }

    pub(crate) fn pat(pmt: u16) -> Vec<u8> {
        section_packet(
            0,
            &with_length(vec![
                0x00,
                0,
                0,
                0,
                1,
                0xc1,
                0,
                0,
                0,
                1,
                0xe0 | (pmt >> 8) as u8,
                pmt as u8,
            ]),
        )
    }

    /// `(stream type, pid, descriptors)`.
    pub(crate) fn pmt(pid: u16, streams: &[(u8, u16, Vec<u8>)]) -> Vec<u8> {
        let mut s = vec![0x02, 0, 0, 0, 1, 0xc1, 0, 0, 0xe1, 0x00, 0xf0, 0];
        for (kind, es, desc) in streams {
            s.extend_from_slice(&[
                *kind,
                0xe0 | (es >> 8) as u8,
                *es as u8,
                0xf0 | (desc.len() >> 8) as u8,
                desc.len() as u8,
            ]);
            s.extend_from_slice(desc);
        }
        section_packet(pid, &with_length(s))
    }

    fn lang(code: &str, kind: u8) -> Vec<u8> {
        let mut d = vec![0x0a, 4];
        d.extend_from_slice(code.as_bytes());
        d.push(kind);
        d
    }

    fn read(packets: &[Vec<u8>]) -> Option<StreamInfo> {
        let mut t = Tables::new();
        let all: Vec<u8> = packets.concat();
        // Fed in awkward pieces, as a hub's chunks come.
        for piece in all.chunks(100) {
            if let Some(i) = t.feed(piece) {
                return Some(i.clone());
            }
        }
        None
    }

    #[test]
    fn the_tables_say_what_a_stream_carries() {
        let mut dvb_ac3 = lang("fre", 0);
        dvb_ac3.extend_from_slice(&[0x6a, 1, 0]);
        let info = read(&[
            vec![0x47, 0x1f, 0xff, 0x10]
                .into_iter()
                .chain([0xff; 184])
                .collect(),
            pat(0x100),
            pmt(
                0x100,
                &[
                    (0x1b, 0x101, vec![]),
                    (0x0f, 0x102, lang("eng", 0)),
                    (0x06, 0x103, dvb_ac3),
                    (0x04, 0x104, lang("eng", 3)),
                    (0x06, 0x105, vec![0x59, 0]),
                ],
            ),
        ])
        .expect("the PMT is read");
        assert_eq!(info.video, Some((0x101, Video::H264)));
        let codecs: Vec<_> = info.audio.iter().map(|a| (a.pid, a.codec)).collect();
        assert_eq!(
            codecs,
            vec![
                (0x102, Audio::Aac),
                (0x103, Audio::Ac3),
                (0x104, Audio::Mpeg)
            ],
            "subtitles are not audio"
        );
        assert_eq!(info.audio[1].lang.as_deref(), Some("fre"));
        assert!(info.audio[2].described);
        assert!(!info.audio[1].atsc, "a DVB AC-3 stream is a private one");
    }

    #[test]
    fn nothing_is_said_before_a_whole_pmt() {
        let mut t = Tables::new();
        assert!(t.feed(&pat(0x100)).is_none());
        assert!(
            t.feed(&[0x47, 0x01, 0x00]).is_none(),
            "a partial packet waits"
        );
        assert!(t.feed(b"not a stream at all, just words").is_none());
    }

    fn info(video: Video, audio: &[(Audio, bool)]) -> StreamInfo {
        StreamInfo {
            video: Some((0x100, video)),
            audio: audio
                .iter()
                .enumerate()
                .map(|(i, &(codec, atsc))| AudioStream {
                    pid: 0x101 + i as u16,
                    codec,
                    lang: None,
                    described: false,
                    impaired: false,
                    atsc,
                })
                .collect(),
        }
    }

    const CHROME: Can = Can {
        hevc: false,
        ac3: false,
        eac3: false,
    };

    #[test]
    fn h264_with_aac_goes_as_it_comes() {
        let p = plan(
            &info(Video::H264, &[(Audio::Aac, false)]),
            CHROME,
            None,
            true,
        );
        assert_eq!(
            p,
            Plan::Play {
                how: How::Pass,
                audio: Some(0),
                silent: None
            }
        );
    }

    #[test]
    fn mpeg2_video_is_refused_with_a_reason() {
        let Plan::Refuse(why) = plan(
            &info(Video::Mpeg2, &[(Audio::Mpeg, false)]),
            CHROME,
            None,
            true,
        ) else {
            panic!("MPEG-2 plays nowhere in a browser");
        };
        assert!(why.contains("MPEG-2"), "{why}");
    }

    #[test]
    fn hevc_plays_only_where_the_browser_says_it_can() {
        let i = info(Video::Hevc, &[(Audio::Aac, false)]);
        assert!(matches!(plan(&i, CHROME, None, true), Plan::Refuse(_)));
        let safari = Can {
            hevc: true,
            ..CHROME
        };
        assert!(matches!(
            plan(&i, safari, None, true),
            Plan::Play { how: How::Pass, .. }
        ));
    }

    #[test]
    fn audio_the_browser_lacks_is_encoded() {
        let i = info(Video::H264, &[(Audio::Ac3, true), (Audio::Aac, false)]);
        assert_eq!(
            plan(&i, CHROME, None, true),
            Plan::Play {
                how: How::Remux {
                    audio: Some(0x101),
                    encode: true
                },
                audio: Some(0),
                silent: None
            }
        );
        let edge = Can {
            ac3: true,
            ..CHROME
        };
        assert!(
            matches!(
                plan(&i, edge, None, true),
                Plan::Play { how: How::Pass, .. }
            ),
            "ATSC AC-3 goes as it is where the browser decodes it"
        );
        let dvb = info(Video::H264, &[(Audio::Ac3, false)]);
        assert!(
            matches!(
                plan(&dvb, edge, None, true),
                Plan::Play {
                    how: How::Remux { encode: true, .. },
                    ..
                }
            ),
            "the page's player does not know a DVB AC-3 stream"
        );
        assert!(matches!(
            plan(
                &info(Video::H264, &[(Audio::Mpeg, false)]),
                edge,
                None,
                true
            ),
            Plan::Play {
                how: How::Remux { encode: true, .. },
                ..
            }
        ));
    }

    #[test]
    fn another_track_goes_through_the_remuxer_alone() {
        let i = info(Video::H264, &[(Audio::Aac, false), (Audio::Aac, false)]);
        assert_eq!(
            plan(&i, CHROME, Some(1), true),
            Plan::Play {
                how: How::Remux {
                    audio: Some(0x102),
                    encode: false
                },
                audio: Some(1),
                silent: None
            }
        );
        assert_eq!(
            plan(&i, CHROME, Some(9), true),
            plan(&i, CHROME, None, true),
            "a track that is not there is the first"
        );
    }

    #[test]
    fn without_an_encoder_the_picture_plays_without_its_sound() {
        let Plan::Play { how, audio, silent } = plan(
            &info(Video::H264, &[(Audio::Ac3, true)]),
            CHROME,
            None,
            false,
        ) else {
            panic!("the picture still plays");
        };
        assert_eq!(
            how,
            How::Remux {
                audio: None,
                encode: false
            }
        );
        assert_eq!(audio, None);
        assert!(silent.unwrap().contains("AC3"));
    }

    /// A packet of `pid` with nothing but an adaptation field carrying a
    /// PCR of `secs`.
    pub(crate) fn pcr_packet(pid: u16, secs: f64) -> Vec<u8> {
        let base = (secs * 90_000.0).round() as u64 % CLOCK_RANGE;
        let mut p = vec![0xffu8; PACKET];
        p[0] = 0x47;
        p[1] = (pid >> 8) as u8 & 0x1f;
        p[2] = pid as u8;
        p[3] = 0x20; // adaptation field only
        p[4] = 183;
        p[5] = 0x10; // PCR present
        p[6] = (base >> 25) as u8;
        p[7] = (base >> 17) as u8;
        p[8] = (base >> 9) as u8;
        p[9] = (base >> 1) as u8;
        p[10] = ((base & 1) << 7) as u8 | 0x7e;
        p[11] = 0;
        p
    }

    /// A packet of `pid` starting a PES with a PTS of `secs`.
    pub(crate) fn pes_packet(pid: u16, secs: f64) -> Vec<u8> {
        let t = (secs * 90_000.0).round() as u64 % CLOCK_RANGE;
        let mut p = vec![0xffu8; PACKET];
        p[0] = 0x47;
        p[1] = 0x40 | ((pid >> 8) as u8 & 0x1f);
        p[2] = pid as u8;
        p[3] = 0x10;
        p[4..13].copy_from_slice(&[0, 0, 1, 0xe0, 0, 0, 0x80, 0x80, 5]);
        p[13] = 0x21 | (((t >> 30) as u8 & 0x07) << 1);
        p[14] = (t >> 22) as u8;
        p[15] = (((t >> 15) as u8) << 1) | 1;
        p[16] = (t >> 7) as u8;
        p[17] = ((t as u8) << 1) | 1;
        p
    }

    fn clock_of(packets: &[Vec<u8>]) -> Vec<f64> {
        let mut c = StreamClock::new();
        let mut seen = Vec::new();
        c.feed(&packets.concat(), |t| seen.push((t * 1000.0).round() / 1000.0));
        seen
    }

    #[test]
    fn the_clock_is_read_from_the_pcr_of_one_pid() {
        let seen = clock_of(&[
            pcr_packet(0x100, 10.0),
            pes_packet(0x101, 10.5),
            pcr_packet(0x200, 900.0),
            pcr_packet(0x100, 10.1),
        ]);
        assert_eq!(seen, vec![10.0, 10.1], "the PTS and another PID's PCR are not read");
    }

    #[test]
    fn without_a_pcr_the_clock_is_read_from_the_pts() {
        let seen = clock_of(&[pes_packet(0x101, 3.0), pes_packet(0x102, 50.0), pes_packet(0x101, 3.04)]);
        assert_eq!(seen, vec![3.0, 3.04]);
    }

    #[test]
    fn the_clock_carries_on_across_its_wrap() {
        let range = CLOCK_RANGE as f64 / 90_000.0;
        let seen = clock_of(&[pcr_packet(0x100, range - 1.0), pcr_packet(0x100, 1.0)]);
        assert_eq!(seen.len(), 2);
        assert!((seen[1] - seen[0] - 2.0).abs() < 0.01, "{seen:?}");
    }
}
