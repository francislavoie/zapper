#!/usr/bin/env python3
"""A stand-in for the OpenSubtitles REST API (v1), for tests.

Nothing here talks to the real service: every answer is made-up data, so
zapper's tests and end-to-end runs exercise search, sign-in, download quota,
rate limiting and subtitle files with no account, no key, no network.

  tools/fake_opensubtitles.py --port 19641
  tools/fake_opensubtitles.py --port 19641 --quota 2          # 406 after two downloads
  tools/fake_opensubtitles.py --port 19641 --busy-after 3     # 429 after 3 requests in 10 s
  tools/fake_opensubtitles.py --port 19641 --down             # 503 on every API call
  tools/fake_opensubtitles.py --port 19641 --token-ttl 5      # a token expires after 5 s

Point zapper at it with ZAPPER_OPENSUBTITLES_URL=http://127.0.0.1:19641.

What it imitates: the Api-Key / User-Agent requirement, login (`tester` /
`test-pass`), the redirect to a sorted, lower-cased query, search by TMDB id,
by parent TMDB id with season and episode, and by name, download links and a
per-user daily quota, logout and the user-info call, and the odd files a
real download can hand back (Latin-1 text, an HTML page, an empty body).

What it does not: moviehash search, pagination (one page, always), the
real ranking, or any answer beyond the handful of fixed titles below.

`GET /_stats` answers counters for assertions; it never prints a token or a
password.
"""
import argparse
import json
import sys
import threading
import time
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qsl, quote, urlsplit

TOKEN_PREFIX = "fake-os-token-"
USERNAME = "tester"
PASSWORD = "test-pass"


def _row(file_id, lang, release, feature, downloads=10, sdh=False, ai=False, trusted=False):
    return {
        "id": str(file_id - 6000000),
        "type": "subtitle",
        "attributes": {
            "subtitle_id": str(file_id - 6000000),
            "language": lang,
            "download_count": downloads,
            "hearing_impaired": sdh,
            "ai_translated": ai,
            "machine_translated": False,
            "from_trusted": trusted,
            "release": release,
            "feature_details": feature,
            "files": [{"file_id": file_id, "cd_number": 1, "file_name": f"{release}.srt"}],
        },
    }


def _film(n, year=2021):
    return {"feature_type": "Movie", "title": f"Film {n:04d}", "year": year,
            "movie_name": f"Film {n:04d} ({year})", "tmdb_id": 900000 + n}


def _ep(n, season, episode):
    return {"feature_type": "Episode", "title": f"Show {n:04d} episode {episode}",
            "season_number": season, "episode_number": episode,
            "parent_title": f"Show {n:04d}", "parent_tmdb_id": 800000 + n}


def search_rows(q):
    """The made-up rows for a (lower-cased) query dict."""
    if q.get("tmdb_id") == "900001":
        f = _film(1)
        return [
            _row(7000001, "en", "Film.0001.2021.1080p.TEST", f, downloads=1200, trusted=True),
            _row(7000002, "en", "Film.0001.2021.1080p.TEST.SDH", f, sdh=True),
            _row(7000003, "en", "Film.0001.2021.AI", f, ai=True),
            _row(7000004, "fr", "Film.0001.2021.1080p.TEST", f),
        ]
    if (q.get("parent_tmdb_id") == "800001" and q.get("season_number") == "1"
            and q.get("episode_number") == "2"):
        return [
            _row(7000011, "en", "Show.0001.S01E02.TEST", _ep(1, 1, 2)),
            _row(7000012, "en", "Show.0001.S01E03.TEST", _ep(1, 1, 3)),
        ]
    if q.get("query") == "film 0002":
        return [_row(7000021, "en", "Film.0002.2021.720p.TEST", _film(2))]
    if q.get("query") == "show 0002" and q.get("season_number") and q.get("episode_number"):
        try:
            season, episode = int(q["season_number"]), int(q["episode_number"])
        except ValueError:
            return []
        return [_row(7000031, "en", "Show.0002.S01E01.TEST", _ep(2, season, episode))]
    return []


def _ts(s):
    return f"{s // 3600:02d}:{s // 60 % 60:02d}:{s % 60:02d},000"


def srt(file_id):
    """20 cues of 2 s, from 00:00:01."""
    cues = []
    for k in range(1, 21):
        start = k * 2 - 1
        cues.append(f"{k}\n{_ts(start)} --> {_ts(start + 2)}\nTest line {k} of file {file_id}\n")
    return "\n".join(cues).encode()


class State:
    def __init__(self, opts):
        self.opts = opts
        self.lock = threading.Lock()
        self.tokens = {}          # token -> issued at (monotonic)
        self.dead = set()
        self.recent = []          # monotonic times of /api/v1 requests
        self.next_token = 1
        self.downloads = 0
        self.stats = {"logins": 0, "logouts": 0, "infos": 0, "searches": 0,
                      "downloads": 0, "files": 0, "authorized_files": 0, "refused": 0}
        self.hosts = set()

    def reset_time(self):
        when = datetime.now(timezone.utc) + timedelta(hours=6)
        return when.strftime("%Y-%m-%dT%H:%M:%S.000Z")


class Handler(BaseHTTPRequestHandler):
    state = None
    server_version = "FakeOpenSubtitles/1"

    def log_message(self, fmt, *a):
        pass  # no request lines: they could carry tokens

    def _send(self, code, body, ctype="application/json", headers=None):
        if not isinstance(body, bytes):
            body = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(body)

    def _refuse(self, code, body, headers=None):
        with self.state.lock:
            self.state.stats["refused"] += 1
        self._send(code, body, headers=headers)

    def _bearer(self):
        auth = self.headers.get("Authorization", "")
        if auth.startswith("Bearer " + TOKEN_PREFIX):
            return auth[len("Bearer "):]
        return None

    def _token_ok(self, token, with_ttl=False):
        st = self.state
        with st.lock:
            if token is None or token not in st.tokens or token in st.dead:
                return False
            ttl = st.opts.token_ttl
            return not (with_ttl and ttl > 0 and time.monotonic() - st.tokens[token] > ttl)

    def _json_body(self):
        try:
            n = int(self.headers.get("Content-Length") or 0)
            body = json.loads(self.rfile.read(n) or b"{}")
        except ValueError:
            return {}
        return body if isinstance(body, dict) else {}

    def _gate(self):
        """Common checks for /api/v1; True when the request may go on."""
        st = self.state
        if self.headers.get("Authorization"):
            with st.lock:
                st.hosts.add(self.headers.get("Host", ""))
        if not self.headers.get("Api-Key") or not (self.headers.get("User-Agent") or "").startswith("zapper v"):
            self._refuse(403, {"message": "You cannot consume this service"})
            return False
        if st.opts.down:
            self._refuse(503, {"message": "Service unavailable"})
            return False
        limit = st.opts.busy_after
        if limit > 0:
            now = time.monotonic()
            with st.lock:
                st.recent = [t for t in st.recent if now - t < 10]
                busy = len(st.recent) >= limit
                if not busy:
                    st.recent.append(now)
            if busy:
                self._refuse(429, {"message": "Too many requests"},
                             {"Retry-After": str(st.opts.retry_after)})
                return False
        return True

    def do_GET(self):
        parts = urlsplit(self.path)
        path = parts.path
        st = self.state
        if path == "/_stats":
            with st.lock:
                out = dict(st.stats, authorized_hosts=sorted(st.hosts))
            return self._send(200, out)
        if path.startswith("/files/") and path.endswith(".srt"):
            return self._file(path[len("/files/"):-len(".srt")])
        if not path.startswith("/api/v1/"):
            return self._send(404, {"message": "not found"})
        if not self._gate():
            return
        if path == "/api/v1/subtitles":
            return self._search(parts.query)
        if path == "/api/v1/infos/user":
            if not self._token_ok(self._bearer()):
                return self._refuse(401, {"message": "Invalid token"})
            with st.lock:
                st.stats["infos"] += 1
                n = st.downloads
            return self._send(200, {"data": {
                "allowed_downloads": st.opts.quota,
                "remaining_downloads": max(st.opts.quota - n, 0),
                "reset_time_utc": st.reset_time()}})
        self._send(404, {"message": "not found"})

    def do_POST(self):
        path = urlsplit(self.path).path
        st = self.state
        if not path.startswith("/api/v1/"):
            return self._send(404, {"message": "not found"})
        if not self._gate():
            return
        body = self._json_body()
        if path == "/api/v1/login":
            user, pw = str(body.get("username", "")), str(body.get("password", ""))
            if "@" in user:
                return self._refuse(400, {"message": "Username, not email"})
            if user != USERNAME or pw != PASSWORD:
                return self._refuse(401, {"message": "Invalid username/password"})
            with st.lock:
                token = f"{TOKEN_PREFIX}{st.next_token}"
                st.next_token += 1
                st.tokens[token] = time.monotonic()
                st.stats["logins"] += 1
            return self._send(200, {
                "token": token, "base_url": f"127.0.0.1:{st.opts.port}",
                "user": {"allowed_downloads": st.opts.quota, "level": "Sub leecher", "vip": False},
                "status": 200})
        if path == "/api/v1/download":
            if not self._token_ok(self._bearer(), with_ttl=True):
                return self._refuse(401, {"message": "Invalid token"})
            try:
                fid = int(body.get("file_id") or 0)
            except (TypeError, ValueError):
                fid = 0
            if fid <= 0:
                # Refused before the allowance is touched, as a missing file costs nothing.
                return self._refuse(400, {"message": "file_id is required"})
            with st.lock:
                over = st.downloads >= st.opts.quota
                if not over:
                    st.downloads += 1
                    st.stats["downloads"] += 1
                n = st.downloads
            if over:
                return self._refuse(406, {
                    "message": f"You have downloaded your allowed {st.opts.quota} subtitles for 24h",
                    "remaining": 0, "reset_time_utc": st.reset_time()})
            return self._send(200, {
                "link": f"http://127.0.0.1:{st.opts.port}/files/{fid}.srt",
                "file_name": f"{fid}.srt", "requests": n,
                "remaining": max(st.opts.quota - n, 0), "message": "",
                "reset_time": "6 hours", "reset_time_utc": st.reset_time()})
        self._send(404, {"message": "not found"})

    def do_DELETE(self):
        path = urlsplit(self.path).path
        st = self.state
        if not path.startswith("/api/v1/"):
            return self._send(404, {"message": "not found"})
        if not self._gate():
            return
        if path == "/api/v1/logout":
            token = self._bearer()
            if not self._token_ok(token):
                return self._refuse(401, {"message": "Invalid token"})
            with st.lock:
                st.dead.add(token)
                st.stats["logouts"] += 1
            return self._send(200, {"message": "token destroyed", "status": 200})
        self._send(404, {"message": "not found"})

    def _search(self, raw_query):
        st = self.state
        pairs = parse_qsl(raw_query, keep_blank_values=True)
        canon = sorted((k.lower(), v.lower()) for k, v in pairs)
        if pairs != canon:
            loc = "/api/v1/subtitles?" + "&".join(
                f"{quote(k, safe='')}={quote(v, safe=',')}" for k, v in canon)
            return self._send(301, b"", headers={"Location": loc})
        q = dict(canon)
        rows = search_rows(q)
        langs = [x for x in q.get("languages", "").split(",") if x]
        if langs:
            rows = [r for r in rows if r["attributes"]["language"] in langs]
        with st.lock:
            st.stats["searches"] += 1
        self._send(200, {"total_pages": 1 if rows else 0, "total_count": len(rows),
                         "per_page": 60, "page": 1, "data": rows})

    def _file(self, name):
        with self.state.lock:
            self.state.stats["files"] += 1
            # A token sent to the link's host is a leak: counted so a check can see it.
            if self.headers.get("Authorization"):
                self.state.stats["authorized_files"] += 1
        try:
            fid = int(name)
        except ValueError:
            return self._send(404, b"", "text/plain")
        if fid == 7000099:
            text = "1\r\n00:00:01,000 --> 00:00:03,000\r\nCafé test line of file 7000099\r\n\r\n"
            return self._send(200, text.encode("latin-1"), "application/x-subrip")
        if fid == 7000098:
            return self._send(200, b"<!DOCTYPE html><html><body>Please log in</body></html>",
                              "text/html")
        if fid == 7000097:
            return self._send(200, b"", "application/x-subrip")
        self._send(200, srt(fid), "application/x-subrip")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--port", type=int, required=True)
    ap.add_argument("--quota", type=int, default=5, help="downloads allowed per user (default 5)")
    ap.add_argument("--busy-after", type=int, default=0,
                    help="429 after this many /api/v1 requests within 10 s (0: never)")
    ap.add_argument("--retry-after", type=int, default=2, help="Retry-After seconds for a 429")
    ap.add_argument("--down", action="store_true", help="503 on every /api/v1 call")
    ap.add_argument("--token-ttl", type=int, default=0,
                    help="seconds before a token stops working on /download (0: never)")
    opts = ap.parse_args()
    Handler.state = State(opts)
    srv = ThreadingHTTPServer(("127.0.0.1", opts.port), Handler)
    srv.daemon_threads = True
    print(f"fake OpenSubtitles on http://127.0.0.1:{opts.port}", file=sys.stderr, flush=True)
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        srv.server_close()


if __name__ == "__main__":
    main()
