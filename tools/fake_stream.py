#!/usr/bin/env python3
"""Serve a short MPEG-TS segment as an endless live stream, for testing.

Real streams are slow to open, change from minute to minute, and many
providers allow a single connection at a time. This replays a local fixture
(tools/make_fixtures.sh generates them) over HTTP forever, so tests exercise
the same path — HTTP, MPEG-TS, mpv's cache — with none of those problems.

  tools/fake_stream.py                       # serves every fixture on :8788
  tools/fake_stream.py --port 9000 --rate 2  # burst at 2x realtime
  tools/fake_stream.py --gap-every 4         # 1.5s gaps, then a burst
  tools/fake_stream.py --burst 20            # open with 20s at once, as many servers do

Imitating a provider that allows one connection and ends it when another
device takes it:

  tools/fake_stream.py --end-after 30                   # each stream ends after 30s
  tools/fake_stream.py --end-after 30 --after-end 509   # ...and then refuses with 509
  tools/fake_stream.py --refuse 403                     # refuses every stream
  tools/fake_stream.py --refuse body                    # 200 with a short text body, no video

Then point the player at:  http://127.0.0.1:8788/sample1080.ts
"""
import argparse
import os
import pathlib
import socketserver
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler

FIXTURES = pathlib.Path(
    os.environ.get("ZAPPER_FIXTURES") or "~/.cache/zapper/fixtures").expanduser()


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    rate = 1.0
    quiet = True
    gap_every = 0.0
    gap_for = 0.0
    burst = 0.0
    end_after = 0.0
    refuse = None
    after_end = None
    reason = ""
    # Set once a stream has been ended by --end-after, so --after-end knows
    # the slot is now "taken".
    ended = threading.Event()

    def log_message(self, fmt, *a):
        if not Handler.quiet:
            # With the client's name: telling mpv's own request from a helper
            # it starts (yt-dlp, say) is the point of reading this log.
            headers = getattr(self, "headers", None)
            agent = headers.get("User-Agent", "-") if headers else "-"
            super().log_message(fmt + " [%s]", *a, agent)

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

        refusal = Handler.refuse
        if refusal is None and Handler.after_end is not None and Handler.ended.is_set():
            refusal = Handler.after_end
        if refusal is not None:
            self._refuse(refusal)
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
        # Chunks to send without pacing: a gap's backlog, and the connect
        # burst many servers open with — what they already hold, at once.
        owed = int(Handler.burst / delay) if Handler.burst else 0
        # A provider that hands the account's one connection to another
        # device ends this one: the response simply finishes, mid-stream.
        ends_at = time.monotonic() + Handler.end_after if Handler.end_after else None

        try:
            while True:  # loop the segment forever
                for i in range(0, len(data), chunk):
                    if ends_at is not None and time.monotonic() >= ends_at:
                        self.wfile.write(b"0\r\n\r\n")
                        self.wfile.flush()
                        self.close_connection = True
                        Handler.ended.set()
                        print(f"ended {name} after {Handler.end_after:g}s", flush=True)
                        return
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

    def _refuse(self, how):
        """Answers as a provider at its connection limit does: a status, or a
        `200` whose body is a line of text rather than video."""
        if how == "body":
            data = b"Max connections reached\n"
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
        else:
            code = int(how)
            data = f"{code} refused\n".encode()
            self.send_response(code, Handler.reason or None)
            self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
        print(f"refused {self.path} with {how}", flush=True)


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

    def handle_error(self, request, client_address):
        # A client hanging up on a kept-alive connection is how a player
        # leaves; not worth a traceback.
        if isinstance(sys.exc_info()[1], (ConnectionResetError, BrokenPipeError)):
            return
        super().handle_error(request, client_address)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8788)
    ap.add_argument("--rate", type=float, default=1.0,
                    help="playback rate multiplier; 2.0 mimics a server's burst")
    ap.add_argument("--gap-every", type=float, default=0.0, metavar="SECONDS",
                    help="go quiet every SECONDS of delivery, like a real server")
    ap.add_argument("--gap-for", type=float, default=1.5, metavar="SECONDS",
                    help="how long each gap lasts; the backlog follows in a burst")
    ap.add_argument("--burst", type=float, default=0.0, metavar="SECONDS",
                    help="open each connection with SECONDS of video at once, like a server")
    ap.add_argument("--end-after", type=float, default=0.0, metavar="SECONDS",
                    help="end each stream after SECONDS, as a provider does when "
                         "another device takes the connection")
    ap.add_argument("--after-end", metavar="STATUS",
                    help="once a stream has been ended, refuse later ones: an HTTP "
                         "status such as 403 or 509, or 'body' for a 200 with text")
    ap.add_argument("--refuse", metavar="STATUS",
                    help="refuse every stream: an HTTP status, or 'body'")
    ap.add_argument("--reason", default="", metavar="TEXT",
                    help="the reason phrase sent with a refusal, e.g. 'Max Connections Reached'")
    ap.add_argument("--verbose", action="store_true")
    a = ap.parse_args()
    Handler.burst = a.burst

    Handler.rate = a.rate
    Handler.gap_every = a.gap_every
    Handler.gap_for = a.gap_for
    Handler.quiet = not a.verbose
    Handler.end_after = a.end_after
    Handler.after_end = a.after_end
    Handler.refuse = a.refuse
    Handler.reason = a.reason
    for how in (a.after_end, a.refuse):
        if how is not None and how != "body" and not how.isdigit():
            ap.error(f"a refusal is an HTTP status or 'body', not {how!r}")

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
