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
  tools/fake_stream.py --burst 20 --burst-rate 4   # ...delivered at 4x rather than at once

Imitating a provider that allows one connection and ends it when another
device takes it:

  tools/fake_stream.py --end-after 30                   # each stream ends after 30s
  tools/fake_stream.py --end-after 30 --after-end 509   # ...and then refuses with 509
  tools/fake_stream.py --refuse 403                     # refuses every stream
  tools/fake_stream.py --refuse body                    # 200 with a short text body, no video

Imitating the network failing rather than the provider:

  tools/fake_stream.py --drop-after 5        # reset the connection after 5s, no end of stream
  tools/fake_stream.py --stall-after 5       # stop sending after 5s, connection left open
  tools/fake_stream.py --drop-after 5 --fault-once   # only the first stream; later ones play

An Xtream account's own report of its connections, at /player_api.php (an API
call, not a stream): max_connections from --max-connections, active_cons the
streams open right now plus --other-devices.

  tools/fake_stream.py --max-connections 1 --other-devices 1   # the one slot is taken elsewhere

And its channels and archive: /live/<user>/<pass>/<id>.ts plays --live-fixture,
and /streaming/timeshift.php reads `start` in the server's time zone, which
player_api.php's server_info reports (time_now, timestamp_now). Each request's
real moment is logged, and listed as JSON at /requests; one older than
--archive-days is a 404.

  tools/fake_stream.py --panel-utc-offset 1 --archive-days 3   # a server an hour east of UTC

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
    burst_rate = 0.0
    end_after = 0.0
    drop_after = 0.0
    stall_after = 0.0
    fault_once = False
    # Stream connections served so far, for --fault-once.
    served = 0
    # Streams being sent right now, and the account's report of them.
    open_streams = 0
    max_connections = 1
    other_devices = 0
    lock = threading.Lock()
    refuse = None
    after_end = None
    reason = ""
    # Set once a stream has been ended by --end-after, so --after-end knows
    # the slot is now "taken".
    ended = threading.Event()
    # The server's time zone, seconds east of UTC (--panel-utc-offset), and
    # how far back its archive reaches (--archive-days).
    panel_offset = None
    archive_days = 3.0
    # What /live/<user>/<pass>/<id>.ts and a catch-up play.
    live_fixture = "sample1080.ts"
    # Every live and catch-up request, for /requests.
    requests = []

    def log_message(self, fmt, *a):
        if not Handler.quiet:
            # With the client's name: telling mpv's own request from a helper
            # it starts (yt-dlp, say) is the point of reading this log.
            headers = getattr(self, "headers", None)
            agent = headers.get("User-Agent", "-") if headers else "-"
            super().log_message(fmt + " [%s]", *a, agent)

    def do_GET(self):
        name = self.path.lstrip("/").split("?")[0]
        if name == "player_api.php":
            self._user_info()
            return
        if name == "requests":
            self._json(Handler.requests)
            return
        if name == "streaming/timeshift.php":
            self._timeshift()
            return
        live = _live_path(name)
        if live is not None:
            Handler.requests.append({"kind": "live", "stream": live, "at": int(time.time())})
            print(f"live: stream {live}", flush=True)
            name = Handler.live_fixture
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

        with Handler.lock:
            Handler.served += 1
            first = Handler.served == 1
        # The faults below apply to every stream, or only to the first.
        faulty = first or not Handler.fault_once

        data = path.read_bytes()
        with Handler.lock:
            Handler.open_streams += 1
        try:
            self._send(name, path, data, faulty)
        finally:
            with Handler.lock:
                Handler.open_streams -= 1

    def _json(self, value):
        import json
        body = json.dumps(value).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _timeshift(self):
        """A catch-up request, as an Xtream server reads it: `start` is
        `YYYY-MM-DD:HH-MM` in the server's own time zone (--panel-utc-offset),
        not the viewer's. The moment it names is logged and kept for
        /requests, so a test can check what was really asked for; a moment
        older than --archive-days is refused with a 404, as a server that
        has dropped it does."""
        from urllib.parse import parse_qs, urlsplit
        q = parse_qs(urlsplit(self.path).query)
        stamp = (q.get("start") or [""])[0]
        stream = (q.get("stream") or [""])[0]
        now = int(time.time())
        start = _stamp_unix(stamp, Handler.panel_offset or 0)
        entry = {"kind": "timeshift", "stream": stream, "stamp": stamp,
                 "duration": (q.get("duration") or [""])[0], "at": now,
                 "start": start, "ago": None if start is None else now - start}
        oldest = now - int(Handler.archive_days * 86400)
        if start is None or start < oldest or start > now:
            entry["status"] = 404
            Handler.requests.append(entry)
            print(f"timeshift: stream {stream} start {stamp!r} is not in the archive; 404", flush=True)
            self.send_error(404, "not in the archive")
            return
        entry["status"] = 200
        Handler.requests.append(entry)
        print(f"timeshift: stream {stream} start {stamp} = "
              f"{time.strftime('%Y-%m-%d %H:%M:%SZ', time.gmtime(start))}, {now - start}s ago",
              flush=True)
        path = FIXTURES / Handler.live_fixture
        self._send(f"timeshift {stream}", path, path.read_bytes(), False)

    def _user_info(self):
        """The account as an Xtream server reports it: connections allowed
        and open, which is what a client asks before reconnecting, and with
        --panel-utc-offset, the server's clock in its own time zone."""
        import json
        with Handler.lock:
            active = Handler.open_streams + Handler.other_devices
        server_info = {}
        if Handler.panel_offset is not None:
            now = int(time.time())
            wall = time.gmtime(now + int(Handler.panel_offset))
            server_info = {
                "timezone": "Etc/Test",
                "time_now": time.strftime("%Y-%m-%d %H:%M:%S", wall),
                "timestamp_now": now,
            }
        body = json.dumps({
            "user_info": {
                "auth": 1,
                "status": "Active",
                # Strings, as many servers send them.
                "max_connections": str(Handler.max_connections),
                "active_cons": str(active),
                "exp_date": "1893456000",
                "is_trial": "0",
            },
            "server_info": server_info,
        }).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
        print(f"user_info: {active} of {Handler.max_connections}", flush=True)

    def _send(self, name, path, data, faulty):
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
        ends_at = time.monotonic() + Handler.end_after if Handler.end_after and faulty else None
        # The network failing rather than the provider: the connection is
        # reset with no end of stream, or simply goes quiet and stays open.
        drops_at = time.monotonic() + Handler.drop_after if Handler.drop_after and faulty else None
        stalls_at = time.monotonic() + Handler.stall_after if Handler.stall_after and faulty else None

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
                    if drops_at is not None and time.monotonic() >= drops_at:
                        self._reset()
                        Handler.ended.set()
                        print(f"reset {name} after {Handler.drop_after:g}s", flush=True)
                        return
                    if stalls_at is not None and time.monotonic() >= stalls_at:
                        self.wfile.flush()
                        Handler.ended.set()
                        print(f"stalled {name} after {Handler.stall_after:g}s", flush=True)
                        # Silent until the client gives up and hangs up.
                        self.connection.settimeout(None)
                        while self.connection.recv(1024):
                            pass
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
                        # A server's backlog arrives as fast as the link
                        # allows, or at a rate of its own (--burst-rate).
                        if Handler.burst_rate:
                            time.sleep(delay / Handler.burst_rate)
                    else:
                        time.sleep(delay)
        except (BrokenPipeError, ConnectionResetError):
            pass  # client went away; normal

    def _reset(self):
        """Drops the connection the way a failing network does: a reset,
        with no end-of-stream chunk and no orderly close."""
        import socket
        import struct
        try:
            self.wfile.flush()
        except OSError:
            pass
        self.connection.setsockopt(
            socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))
        self.connection.close()
        self.close_connection = True

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


def _live_path(name):
    """`live/<user>/<pass>/<id>.ts`, an Xtream channel's URL, to its id."""
    parts = name.split("/")
    if len(parts) == 4 and parts[0] == "live" and parts[3].endswith(".ts"):
        return parts[3][:-3]
    return None


def _stamp_unix(stamp, offset):
    """`YYYY-MM-DD:HH-MM`, wall time `offset` seconds east of UTC, to Unix
    seconds; None if it is not one."""
    import calendar
    try:
        t = time.strptime(stamp, "%Y-%m-%d:%H-%M")
    except ValueError:
        return None
    return calendar.timegm(t) - int(offset)


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
    ap.add_argument("--burst-rate", type=float, default=0.0, metavar="RATE",
                    help="deliver the --burst at RATE times realtime rather than at once")
    ap.add_argument("--drop-after", type=float, default=0.0, metavar="SECONDS",
                    help="reset each stream's connection after SECONDS, with no end of stream")
    ap.add_argument("--stall-after", type=float, default=0.0, metavar="SECONDS",
                    help="stop sending after SECONDS and leave the connection open")
    ap.add_argument("--fault-once", action="store_true",
                    help="--end-after/--drop-after/--stall-after only the first stream")
    ap.add_argument("--max-connections", type=int, default=1, metavar="N",
                    help="the account's connection limit, as /player_api.php reports it")
    ap.add_argument("--other-devices", type=int, default=0, metavar="N",
                    help="streams open on other devices, counted in active_cons")
    ap.add_argument("--end-after", type=float, default=0.0, metavar="SECONDS",
                    help="end each stream after SECONDS, as a provider does when "
                         "another device takes the connection")
    ap.add_argument("--after-end", metavar="STATUS",
                    help="once a stream has been ended (or dropped, or stalled), refuse later ones: an HTTP "
                         "status such as 403 or 509, or 'body' for a 200 with text")
    ap.add_argument("--refuse", metavar="STATUS",
                    help="refuse every stream: an HTTP status, or 'body'")
    ap.add_argument("--reason", default="", metavar="TEXT",
                    help="the reason phrase sent with a refusal, e.g. 'Max Connections Reached'")
    ap.add_argument("--panel-utc-offset", type=float, default=None, metavar="HOURS",
                    help="the server's time zone, hours east of UTC (1, -5, 5.5): reported "
                         "in player_api.php's server_info and used to read catch-up starts")
    ap.add_argument("--archive-days", type=float, default=3.0, metavar="DAYS",
                    help="how far back /streaming/timeshift.php serves; older is a 404")
    ap.add_argument("--live-fixture", default=None, metavar="NAME",
                    help="the fixture /live/<user>/<pass>/<id>.ts and catch-up serve "
                         "(default sample1080.ts, or the first there is)")
    ap.add_argument("--verbose", action="store_true")
    a = ap.parse_args()
    if a.panel_utc_offset is not None:
        Handler.panel_offset = round(a.panel_utc_offset * 3600)
    Handler.archive_days = a.archive_days
    Handler.burst = a.burst
    Handler.burst_rate = a.burst_rate
    Handler.drop_after = a.drop_after
    Handler.stall_after = a.stall_after
    Handler.fault_once = a.fault_once
    Handler.max_connections = a.max_connections
    Handler.other_devices = a.other_devices

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
    names = [f.name for f in fixtures]
    if a.live_fixture is not None:
        if a.live_fixture not in names:
            ap.error(f"no fixture named {a.live_fixture!r}")
        Handler.live_fixture = a.live_fixture
    elif Handler.live_fixture not in names:
        Handler.live_fixture = names[0]

    with Server(("127.0.0.1", a.port), Handler) as srv:
        print(f"serving {len(fixtures)} fixture(s) on http://127.0.0.1:{a.port}/")
        for f in fixtures:
            print(f"  http://127.0.0.1:{a.port}/{f.name}")
        sys.stdout.flush()
        srv.serve_forever()
    return 0


if __name__ == "__main__":
    sys.exit(main())
