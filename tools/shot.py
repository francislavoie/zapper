#!/usr/bin/env python3
"""Screenshot a window by title, for scripted checks that cannot see the screen.

GNOME refuses D-Bus screenshots to unauthorised callers and blocks Wayland
capture, but an app launched with GDK_BACKEND=x11 is an ordinary XWayland
client: X can then report its geometry and ffmpeg's x11grab can read the
composited pixels, GL surface included.

  tools/shot.py --title zapper --out /tmp/z.png
  tools/shot.py --list
"""
import argparse
import subprocess
import sys

from Xlib import display as xdisplay
from Xlib import X


def windows(dpy):
    root = dpy.screen().root
    out = []

    def walk(win, depth=0):
        try:
            children = win.query_tree().children
        except Exception:
            return
        for c in children:
            try:
                name = c.get_wm_name()
                g = c.get_geometry()
                attrs = c.get_attributes()
                if name and attrs.map_state == X.IsViewable and g.width > 80:
                    # Translate to root coordinates; nested clients report
                    # geometry relative to their parent.
                    t = c.translate_coords(root, 0, 0)
                    out.append((name, -t.x, -t.y, g.width, g.height))
            except Exception:
                pass
            walk(c, depth + 1)

    walk(root)
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--title", help="substring of the window title")
    ap.add_argument("--out", default="/tmp/shot.png")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--display", default=":0")
    a = ap.parse_args()

    dpy = xdisplay.Display(a.display)
    found = windows(dpy)

    if a.list or not a.title:
        for n, x, y, w, h in found:
            print(f"{w}x{h}+{x}+{y}\t{n}")
        return 0

    match = [f for f in found if a.title.lower() in f[0].lower()]
    if not match:
        print(f"no window matching {a.title!r}; use --list", file=sys.stderr)
        return 1
    # Largest match: title substrings can hit menus and tooltips too.
    name, x, y, w, h = max(match, key=lambda f: f[3] * f[4])

    # Clamp to the screen: a window may extend past the edge (or sit under a
    # panel), and x11grab refuses any region that does.
    scr = dpy.screen()
    sw, sh = scr.width_in_pixels, scr.height_in_pixels
    x, y = max(0, x), max(0, y)
    w, h = min(w, sw - x), min(h, sh - y)
    # x11grab wants even dimensions for most pixel formats.
    w, h = w - (w % 2), h - (h % 2)
    if w < 2 or h < 2:
        print(f"window {name!r} is offscreen", file=sys.stderr)
        return 1

    cmd = [
        "ffmpeg", "-hide_banner", "-loglevel", "error",
        "-f", "x11grab", "-video_size", f"{w}x{h}",
        "-i", f"{a.display}+{x},{y}", "-frames:v", "1", "-y", a.out,
    ]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        print(r.stderr.strip()[:300], file=sys.stderr)
        return 1
    print(f"{a.out}  ({w}x{h} from {name!r})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
