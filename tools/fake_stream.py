#!/usr/bin/env python3
"""Serve a recorded segment as an endless live stream, for testing.

Many providers allow a single connection, which concurrent runs wouldfight over, and real streams are slow to open and non-deterministic. This
replays a recorded fixture over HTTP forever, so tests exercise the same
path — HTTP, MPEG-TS, mpv's cache — with none of those problems.

  tools/fake_stream.py                       # serves every fixture on :8788
  tools/fake_stream.py --port 9000 --rate 2  # burst at 2x realtime
  tools/fake_stream.py --gap-every 4         # 1.5s gaps, then a burst

Then point the player at:  http://127.0.0.1:8788/sample1080.ts
"""
import argparse
import pathlib
import socketserver
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler

FIXTURES = pathlib.Path("~/.cache/zapper/fixtures").expanduser()


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    rate = 1.0
    quiet = True
    gap_every = 0.0
    gap_for = 0.0

    def log_message(self, *a):
        if not Handler.quiet:
            super().log_message(*a)

    def do_GET(self):
        name = self.path.lstrip("/").split("?")[0]
        if name in ("", "index"):
            body = "\n".join(p.name for p in sorted(FIXTURES.glob("*.ts")))
            data = (body + "\n").encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return

        path = FIXTURES / name
        if not path.is_file() or path.suffix != ".ts":
            self.send_error(404, "no such fixture")
            return

        data = path.read_bytes()
        # Chunked with no length: a live feed never declares one, and mpv must
        # treat this as unseekable-at-source exactly as it does a real server.
        self.send_response(200)
        self.send_header("Content-Type", "video/mp2t")
        self.send_header("Transfer-Encoding", "chunked")
        self.send_header("Cache-Control", "no-cache")
        self.end_headers()

        # Pace to roughly the source bitrate so the client sees a live-ish
        # feed rather than a firehose. Real servers often burst at about 2x
        # realtime (--rate).
        seconds = _duration(path)
        bps = len(data) / max(seconds, 0.001) * Handler.rate
        chunk = 64 * 1024
        delay = chunk / bps

        # Delivery gaps, off unless asked for. Real servers often do not deliver
        # evenly: it goes quiet for a second or two and then sends what it owes
        # in a burst. Playing right at the live edge survives an even feed and
        # stalls on that one, so testing the edge needs the gaps.
        next_gap = time.monotonic() + Handler.gap_every if Handler.gap_every else None
        owed = 0  # chunks to send without pacing, to catch up after a gap

        try:
            while True:  # loop the segment forever
                for i in range(0, len(data), chunk):
                    part = data[i : i + chunk]
                    self.wfile.write(f"{len(part):X}\r\n".encode())
                    self.wfile.write(part)
                    self.wfile.write(b"\r\n")
                    if next_gap is not None and time.monotonic() >= next_gap:
                        self.wfile.flush()
                        time.sleep(Handler.gap_for)
                        owed += int(Handler.gap_for / delay)
                        next_gap = time.monotonic() + Handler.gap_every
                    if owed > 0:
                        owed -= 1
                    else:
                        time.sleep(delay)
        except (BrokenPipeError, ConnectionResetError):
            pass  # client went away; normal


_durations: dict[pathlib.Path, float] = {}


def _duration(path: pathlib.Path) -> float:
    if path not in _durations:
        import subprocess

        r = subprocess.run(
            ["ffprobe", "-v", "error", "-show_entries", "format=duration",
             "-of", "csv=p=0", str(path)],
            capture_output=True, text=True)
        try:
            _durations[path] = float(r.stdout.strip())
        except ValueError:
            _durations[path] = 30.0
    return _durations[path]


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8788)
    ap.add_argument("--rate", type=float, default=1.0,
                    help="playback rate multiplier; 2.0 mimics the panel's burst")
    ap.add_argument("--gap-every", type=float, default=0.0, metavar="SECONDS",
                    help="go quiet every SECONDS of delivery, like a live server")
    ap.add_argument("--gap-for", type=float, default=1.5, metavar="SECONDS",
                    help="how long each gap lasts; the backlog follows in a burst")
    ap.add_argument("--verbose", action="store_true")
    a = ap.parse_args()

    Handler.rate = a.rate
    Handler.gap_every = a.gap_every
    Handler.gap_for = a.gap_for
    Handler.quiet = not a.verbose

    fixtures = sorted(FIXTURES.glob("*.ts"))
    if not fixtures:
        print(f"no fixtures in {FIXTURES}; run tools/make_fixtures.sh first",
              file=sys.stderr)
        return 1

    with Server(("127.0.0.1", a.port), Handler) as srv:
        print(f"serving {len(fixtures)} fixture(s) on http://127.0.0.1:{a.port}/")
        for f in fixtures:
            print(f"  http://127.0.0.1:{a.port}/{f.name}")
        sys.stdout.flush()
        srv.serve_forever()
    return 0


if __name__ == "__main__":
    sys.exit(main())
