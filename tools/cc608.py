"""Puts three made-up closed captions into an MPEG-2 video stream.

Usage: python3 -I tools/cc608.py <in.m2v> <out.m2v>

The captions are CEA-608, carried as ATSC A/53 (GA94) user data after each
picture header, the way a North American broadcast carries them; FFmpeg's
caption decoder (mpv's `eia_608` track) reads them. Each is a pop-on caption
on caption channel 1:

  0.2 s  top left, rows 1-2, column 0
  4.0 s  centred at the bottom, rows 14-15 (indents and spaces)
  8.0 s  on the right, row 15, column 20

each cleared about 3.5 s later. tools/make_fixtures.sh makes the input
(no B-frames, so the order pictures are coded in is the order they are
shown) at 29.97 fps, and puts the result in MPEG-TS.
"""
import sys

FPS = 30000 / 1001


def odd_parity(b):
    b &= 0x7F
    return b | (0x80 if bin(b).count("1") % 2 == 0 else 0)


def frame_at(seconds):
    return int(round(seconds * FPS))


def control(hi, lo):
    # Control codes are sent twice, as the standard asks.
    return [(hi, lo), (hi, lo)]


def text(s):
    codes = [ord(c) for c in s]
    if len(codes) % 2:
        codes.append(0x00)
    return [(codes[i], codes[i + 1]) for i in range(0, len(codes), 2)]


RESUME_CAPTION_LOADING = (0x14, 0x20)
ERASE_NON_DISPLAYED = (0x14, 0x2E)
END_OF_CAPTION = (0x14, 0x2F)
ERASE_DISPLAYED = (0x14, 0x2C)
# Preamble address codes on channel 1: (first byte, second byte) per row.
ROW_CODES = {1: (0x11, 0x40), 2: (0x11, 0x60), 3: (0x12, 0x40), 4: (0x12, 0x60),
             5: (0x15, 0x40), 6: (0x15, 0x60), 7: (0x16, 0x40), 8: (0x16, 0x60),
             9: (0x17, 0x40), 10: (0x17, 0x60), 11: (0x10, 0x40), 12: (0x13, 0x40),
             13: (0x13, 0x60), 14: (0x14, 0x40), 15: (0x14, 0x60)}


def preamble(row, indent):
    """Row `row` (1-15), indented by `indent` columns (a multiple of 4)."""
    hi, lo = ROW_CODES[row]
    return (hi, lo | 0x10 | (2 * (indent // 4)))


def pop_on(lines):
    pairs = control(*RESUME_CAPTION_LOADING) + control(*ERASE_NON_DISPLAYED)
    for row, indent, s in lines:
        pairs += control(*preamble(row, indent)) + text(s)
    return pairs + control(*END_OF_CAPTION)


schedule = {}


def at(seconds, pairs):
    for i, pair in enumerate(pairs):
        schedule[frame_at(seconds) + i] = pair


at(0.2, pop_on([(1, 0, "TOP LEFT, ROW ONE"), (2, 0, "AND ROW TWO")]))
at(3.6, control(*ERASE_DISPLAYED))
at(4.0, pop_on([(14, 8, "  CENTRED AT"), (15, 8, "THE BOTTOM ROWS")]))
at(7.6, control(*ERASE_DISPLAYED))
at(8.0, pop_on([(15, 20, "RIGHT SIDE")]))
at(11.4, control(*ERASE_DISPLAYED))


def user_data(pair):
    """One picture's GA94 cc_data: one CEA-608 field-1 pair, or padding."""
    b1, b2 = (odd_parity(pair[0]), odd_parity(pair[1])) if pair else (0x80, 0x80)
    return bytes([0, 0, 1, 0xB2, 0x47, 0x41, 0x39, 0x34, 0x03, 0x40 | 2, 0xFF,
                  0xFC, b1, b2, 0xFD, 0x80, 0x80, 0xFF])


def main(src_path, out_path):
    src = open(src_path, "rb").read()
    out = bytearray()
    pos = 0
    frame = 0
    while True:
        picture = src.find(b"\x00\x00\x01\x00", pos)
        if picture < 0:
            break
        # The user data goes before the picture's first slice.
        s = picture + 4
        while True:
            s = src.find(b"\x00\x00\x01", s)
            if s < 0 or 0x01 <= src[s + 3] <= 0xAF:
                break
            s += 3
        if s < 0:
            break
        out += src[pos:s]
        out += user_data(schedule.get(frame))
        pos = s
        frame += 1
    out += src[pos:]
    open(out_path, "wb").write(out)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
