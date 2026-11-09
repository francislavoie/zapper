//! SubRip files, as fetched subtitles come: read whatever their encoding,
//! told apart from the error page a download link can hand out instead, and
//! turned into WebVTT, which both the browser's `<track>` and mpv read.

/// The largest subtitle file zapper takes. A feature's subtitles are well
/// under a megabyte.
pub const SUBTITLE_MAX_BYTES: usize = 2 << 20;

/// The file's text: UTF-8 (with or without its byte-order mark), UTF-16 with
/// one, and anything else as Windows-1252, the Latin-1 superset older
/// subtitle files are written in.
pub fn decode(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(b"\xef\xbb\xbf") {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if let Some(rest) = bytes.strip_prefix(b"\xff\xfe") {
        return utf16(rest, u16::from_le_bytes);
    }
    if let Some(rest) = bytes.strip_prefix(b"\xfe\xff") {
        return utf16(rest, u16::from_be_bytes);
    }
    if let Some(unit) = utf16_without_bom(bytes) {
        return utf16(bytes, unit);
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => bytes.iter().map(|&b| windows_1252(b)).collect(),
    }
}

/// UTF-16 without a byte-order mark, told by its NULs: a subtitle file is
/// mostly Latin text, whose every other byte is then 0 (the second of each
/// pair little-endian, the first big-endian). Judged on the first 512 bytes.
fn utf16_without_bom(bytes: &[u8]) -> Option<fn([u8; 2]) -> u16> {
    let (pairs, _) = bytes[..bytes.len().min(512)].as_chunks::<2>();
    if pairs.len() < 4 {
        return None;
    }
    let first = pairs.iter().filter(|p| p[0] == 0).count();
    let second = pairs.iter().filter(|p| p[1] == 0).count();
    // Most pairs have their NUL on one side, and almost none on the other.
    let most = |n: usize| n * 10 >= pairs.len() * 7;
    let few = |n: usize| n * 10 <= pairs.len();
    if most(second) && few(first) {
        Some(u16::from_le_bytes)
    } else if most(first) && few(second) {
        Some(u16::from_be_bytes)
    } else {
        None
    }
}

fn utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> String {
    let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|&c| unit(c)).collect();
    String::from_utf16_lossy(&units)
}

/// Latin-1, but for the 0x80–0x9F block, where Windows-1252 has its quotes
/// and dashes instead of control codes.
fn windows_1252(b: u8) -> char {
    const HIGH: [u16; 32] = [
        0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160,
        0x2039, 0x0152, 0x008D, 0x017D, 0x008F, 0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022,
        0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x017E, 0x0178,
    ];
    match b {
        0x80..=0x9F => char::from_u32(u32::from(HIGH[usize::from(b - 0x80)])).unwrap_or('\u{fffd}'),
        _ => char::from(b),
    }
}

/// Whether `text` is a SubRip file at all: it has a cue timing, and is not a
/// web page (a link that has expired answers with one).
pub fn looks_like_srt(text: &str) -> bool {
    let head = text.trim_start_matches('\u{feff}').trim_start();
    !head.starts_with('<') && text.lines().any(|l| timing(l).is_some())
}

/// SubRip to WebVTT: the `WEBVTT` header, `.` for `,` in the times, the cue
/// numbers and ASS `{\…}` overrides dropped, `<i>`, `<b>` and `<u>` kept and
/// every other tag dropped, a `-->` inside the text made `→` (it would end
/// the cue's text in WebVTT), cues left with no text dropped and blocks
/// without a readable timing skipped.
pub fn to_vtt(srt: &str) -> String {
    let text = srt
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let mut out = String::from("WEBVTT\n");
    let mut block: Vec<&str> = Vec::new();
    for line in text.lines().chain(std::iter::once("")) {
        if line.trim().is_empty() {
            if !block.is_empty() {
                push_cue(&block, &mut out);
                block.clear();
            }
        } else {
            block.push(line);
        }
    }
    out
}

fn push_cue(block: &[&str], out: &mut String) {
    let numbered = block[0].trim().bytes().all(|b| b.is_ascii_digit());
    let at = usize::from(numbered);
    let Some((start, end)) = block.get(at).and_then(|l| timing(l)) else {
        return;
    };
    let lines: Vec<String> = block[at + 1..]
        .iter()
        .map(|l| clean(l))
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return;
    }
    out.push_str(&format!("\n{start} --> {end}\n{}\n", lines.join("\n")));
}

/// `00:00:01,000 --> 00:00:02,500` (and whatever position follows it), as
/// WebVTT's two times.
fn timing(line: &str) -> Option<(String, String)> {
    let (start, rest) = line.split_once("-->")?;
    let end = rest.split_whitespace().next()?;
    Some((time(start.trim())?, time(end)?))
}

/// `H:MM:SS,mmm` (or with a `.`) to `HH:MM:SS.mmm`.
fn time(t: &str) -> Option<String> {
    let mut parts = t.split(':');
    let (h, m, s) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let (s, ms) = s.split_once([',', '.']).unwrap_or((s, "0"));
    let digits = |x: &str, min: usize, max: usize| {
        (min..=max).contains(&x.len()) && x.bytes().all(|b| b.is_ascii_digit())
    };
    if !digits(h, 1, 3) || !digits(m, 2, 2) || !digits(s, 2, 2) || !digits(ms, 1, 3) {
        return None;
    }
    let (h, m, s): (u32, u32, u32) = (h.parse().ok()?, m.parse().ok()?, s.parse().ok()?);
    if m > 59 || s > 59 {
        return None;
    }
    let ms: u32 = format!("{ms:0<3}").parse().ok()?;
    Some(format!("{h:02}:{m:02}:{s:02}.{ms:03}"))
}

/// A line of cue text, its markup reduced to what WebVTT and zapper show.
///
/// Byte by byte: everything it acts on is ASCII, so every index it cuts at
/// is a character boundary. Each search for a closer stops at the next
/// opener of its kind, so no byte is looked at more than a few times,
/// however hostile the line.
fn clean(line: &str) -> String {
    let b = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    // Text since the last markup, copied in one go.
    let mut text_from = 0;
    let mut i = 0;
    while i < b.len() {
        let (put, next) = match b[i] {
            b'{' if b.get(i + 1) == Some(&b'\\') => match closer(b, i, b'{', b'}') {
                Some(end) => ("", end + 1),
                None => {
                    i += 1;
                    continue;
                }
            },
            b'-' if b[i..].starts_with(b"-->") => ("→", i + 3),
            b'<' => match closer(b, i, b'<', b'>') {
                Some(end) if is_tag(&line[i + 1..end]) => {
                    (kept_tag(&line[i + 1..end]).unwrap_or(""), end + 1)
                }
                // A `<` that starts no tag would start one in WebVTT.
                _ => ("&lt;", i + 1),
            },
            // As text, not the start of a character reference.
            b'&' => ("&amp;", i + 1),
            _ => {
                i += 1;
                continue;
            }
        };
        out.push_str(&line[text_from..i]);
        out.push_str(put);
        i = next;
        text_from = next;
    }
    out.push_str(&line[text_from..]);
    out.trim().to_owned()
}

/// Where the `close` for the `open` at `at` is, if it comes before another
/// `open`.
fn closer(b: &[u8], at: usize, open: u8, close: u8) -> Option<usize> {
    let hit = at + 1 + b[at + 1..].iter().position(|&c| c == open || c == close)?;
    (b[hit] == close).then_some(hit)
}

fn is_tag(inside: &str) -> bool {
    let name = inside.strip_prefix('/').unwrap_or(inside);
    name.starts_with(|c: char| c.is_ascii_alphabetic())
}

fn kept_tag(inside: &str) -> Option<&'static str> {
    Some(match inside.trim().to_ascii_lowercase().as_str() {
        "i" => "<i>",
        "/i" => "</i>",
        "b" => "<b>",
        "/b" => "</b>",
        "u" => "<u>",
        "/u" => "</u>",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srt_becomes_webvtt() {
        let srt = "\u{feff}1\r\n00:00:01,000 --> 00:00:02,500\r\n<font color=\"#fff\">Test line 1</font>\r\n\r\n2\r\n00:00:03,000 --> 00:00:04,000\r\n{\\an8}<i>Test line 2</i>\r\n";
        assert_eq!(
            to_vtt(&decode(srt.as_bytes())),
            "WEBVTT\n\n00:00:01.000 --> 00:00:02.500\nTest line 1\n\n00:00:03.000 --> 00:00:04.000\n<i>Test line 2</i>\n"
        );
    }

    #[test]
    fn odd_blocks_are_cleaned_or_skipped() {
        let srt = "1\n00:00:01,000 --> 00:00:02,000\n<B>Test</B> <u>line</u> <span>3</span>\nof --> file\n\n\n\
                   2\n00:00:03,000 --> 00:00:04,000\n<font color=\"red\"></font>\n\n\
                   3\nnot a time\nTest line 5\n\n\
                   00:00:05,000 --> 00:00:06,000 X1:10 X2:20\nTest line 6 < 7\n\n\
                   4\n0:0:7,5 --> 00:00:08,000\nTest line 7\n";
        assert_eq!(
            to_vtt(srt),
            "WEBVTT\n\n\
             00:00:01.000 --> 00:00:02.000\n<b>Test</b> <u>line</u> 3\nof → file\n\n\
             00:00:05.000 --> 00:00:06.000\nTest line 6 &lt; 7\n"
        );
        assert_eq!(to_vtt(""), "WEBVTT\n");
    }

    #[test]
    fn ampersands_and_unclosed_markup_stay_text() {
        let srt = "1\n00:00:01,000 --> 00:00:02,000\nTest &lt; line & 8\n\
                   {\\an8 <i Test {\\i1}line <<i>9</i>\n";
        assert_eq!(
            to_vtt(srt),
            "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nTest &amp;lt; line &amp; 8\n\
             {\\an8 &lt;i Test line &lt;<i>9</i>\n"
        );
    }

    #[test]
    fn a_hostile_line_takes_linear_time() {
        let half = SUBTITLE_MAX_BYTES / 2;
        for line in [
            "<".repeat(SUBTITLE_MAX_BYTES),
            "{\\".repeat(half),
            "<a".repeat(half),
            format!("<{}", "a".repeat(SUBTITLE_MAX_BYTES)),
            format!("{{\\{}", "a".repeat(SUBTITLE_MAX_BYTES)),
        ] {
            let srt = format!("1\n00:00:01,000 --> 00:00:02,000\n{line}\n");
            let started = std::time::Instant::now();
            let vtt = to_vtt(&srt);
            let took = started.elapsed();
            assert!(vtt.len() > SUBTITLE_MAX_BYTES / 2);
            // Linear is milliseconds even unoptimised; quadratic was hours.
            assert!(took < std::time::Duration::from_secs(1), "{took:?}");
        }
    }

    #[test]
    fn latin1_reads_and_html_is_not_a_subtitle() {
        assert_eq!(decode(b"\xe9"), "é");
        assert_eq!(
            decode(b"caf\xe9\r\n\x93Test\x94"),
            "café\r\n\u{201c}Test\u{201d}"
        );
        assert_eq!(decode("café".as_bytes()), "café");
        assert_eq!(decode(b"\xff\xfeT\x00e\x00"), "Te");
        // UTF-16 without its mark, either way round.
        let text = "1\r\n00:00:01,000 --> 00:00:02,000\r\nTest line 1 é\r\n";
        let le: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let be: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
        assert_eq!(decode(&le), text);
        assert_eq!(decode(&be), text);
        assert_eq!(decode(b"Te\x00t"), "Te\u{0}t", "a stray NUL is not UTF-16");
        let srt = "1\n00:00:01,000 --> 00:00:02,000\nTest line 1 of file 7000001\n";
        assert!(looks_like_srt(srt));
        assert!(!looks_like_srt(
            "<html><body>00:00:01,000 --> 00:00:02,000</body></html>"
        ));
        assert!(!looks_like_srt("<!DOCTYPE html>\n<html></html>"));
        assert!(!looks_like_srt(""));
        assert!(!looks_like_srt("Test line 1\nTest line 2\n"));
    }
}
