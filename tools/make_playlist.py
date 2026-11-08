#!/usr/bin/env python3
"""Write an M3U of made-up live channels, movies and episodes for
tools/fake_stream.py, to stdout.

  tools/make_playlist.py --base http://127.0.0.1:8788 --channels 3 \\
      --movie 2 --episode "A Show S01E01=100111" > test.m3u

Channels are /live/u/p/<n>.ts; a movie is /movie/u/p/<id>.<ext> (odd id mkv,
even mp4); an episode is /series/u/p/<id>.<ext> (odd last digit mkv, even mp4).
"""
import argparse
import sys


def ext(n):
    return "mkv" if n % 2 else "mp4"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", required=True, metavar="URL")
    ap.add_argument("--channels", type=int, default=0, metavar="N")
    ap.add_argument("--movie", type=int, action="append", default=[], metavar="ID")
    ap.add_argument("--episode", action="append", default=[], metavar="NAME=ID")
    ap.add_argument("--group", default="Test", metavar="G")
    a = ap.parse_args()
    base = a.base.rstrip("/")
    out = ["#EXTM3U"]

    def entry(name, url):
        out.append(f'#EXTINF:-1 group-title="{a.group}",{name}')
        out.append(url)

    for n in range(1, a.channels + 1):
        entry(f"Channel {n}", f"{base}/live/u/p/{n}.ts")
    for mid in a.movie:
        entry(f"Film {mid:04d}", f"{base}/movie/u/p/{mid}.{ext(mid)}")
    for spec in a.episode:
        name, sep, eid = spec.rpartition("=")
        if not sep or not eid.isdigit():
            ap.error(f"an episode is NAME=ID, not {spec!r}")
        entry(name, f"{base}/series/u/p/{eid}.{ext(int(eid))}")
    print("\n".join(out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
