# tools

> **On this line** (the rewrite) there is no desktop app. The checks and tools that drove the Tauri
> app's window (`surface_check`, `shot_check`, `ipc_check`, `quit_check.py`, `shot.py`,
> `install-desktop.sh`) are on the [`main-tauri`](https://github.com/francislavoie/zapper/tree/main-tauri)
> branch. `player_check` and `catchup_check` run on a headless mpv host (see Checking everything),
> with the same flags.

## Test against a local stream

Real streams take seconds to open, their content changes minute to minute,
and many providers allow a single connection at a time. The harnesses play a
short synthetic stream from a local server instead:

```sh
tools/make_fixtures.sh                    # once: generates the fixtures with ffmpeg
tools/fake_stream.py &                    # serves them on 127.0.0.1:8788
ZAPPER_STREAM_URL=http://127.0.0.1:8788/sample1080.ts \
  cargo run -p zapper --features harness,mpv --bin seek_bench
```

Fixtures live in `~/.cache/zapper/fixtures` (`ZAPPER_FIXTURES` overrides it),
outside the repository: they are generated, and tens of megabytes each.

| Fixture | Format | Why |
|---|---|---|
| `sample1080.ts` | h264 1920x1080@50, aac | The test pattern with little motion |
| `motion1080p.ts` | h264 1920x1080@25, aac | 1080p with the whole frame moving |
| `sport720p60.ts` | h264 1280x720@60, **ac3** | 60 fps, high motion, and the only AC-3 audio |
| `mpeg2sd.ts` | **mpeg2** 720x576@25, **mp2** | Video no browser plays (watching in a browser says so) |
| `placeholder.ts` | h264 1920x1080@30, aac, 600 s | Black and silence: what a provider sends for a channel that is off the air (`--placeholder`) |
| `radio.ts` | no video, aac | A radio channel: sound and nothing else, so no frame ever comes |

Between them: 720p and 1080p, 25/50/60 fps, aac, ac3 and mp2, low and high
motion. All but `mpeg2sd.ts` and `radio.ts` are H.264 8-bit 4:2:0 in MPEG-TS with a keyframe
every two seconds, the shape most live IPTV feeds have. Any other `.ts` file dropped into the
fixtures directory is served too.

`player_check` can zap after its run: `ZAPPER_ZAP_TO=2,3` opens ids 2 and 3 in turn from the
channel playing, each from its own fixture (`2.ts`, `3.ts`: a fixtures directory of links, say
`2.ts` to `radio.ts`), and checks each still plays past the first-frame deadline.

`fake_stream.py` sends a fixture as an endless chunked response with no
length, as a live feed does, paced to its bitrate. Flags imitate how real
servers deliver: `--rate 2` bursts at twice realtime, `--burst 20` opens each
connection with 20 s of video at once (`--burst-rate 4` sends it at four times
realtime instead), and `--gap-every 4` goes quiet every
four seconds and then sends the backlog in a burst (playing right at the live
edge survives an even feed and stalls on that one).

Others imitate a provider that allows one connection and hands it to another
device: `--end-after 30` ends each stream after 30 s (the response just
finishes, as when the provider drops it), `--after-end 509` answers every
stream after that with a 509 (or any status, or `body` for a `200` carrying a
line of text instead of video), `--refuse 403` refuses every stream from the
start, and `--reason "Max Connections Reached"` sets the reason phrase a
refusal is sent with.

And a provider answering a channel that is off the air with a placeholder clip
instead of the channel: `--placeholder` answers every stream with
`placeholder.ts` (600 s of black and silence, 1080p at 30 fps and under
100 kbit/s, which `make_fixtures.sh` writes too), as one chunked response
with no length, all at once, and then ends it and closes the connection.
`--burst-rate 200` paces it to about three seconds, as the provider measured
did. The app should stop it as off air within `ending::OFF_AIR_WINDOW` of the
open, without skipping to the clip's end or reconnecting:

```sh
tools/fake_stream.py --placeholder &
ZAPPER_LIVE_START=wait cargo run --features harness --bin player_check -- --ending offAir
```

`player_check --ending <kind>` plays a channel that should stop by itself and
checks the kind it ends as (`offAir`, `dropped`, …); for `offAir` also that it
came within the window, carried the clip's length, never jumped ahead or
reconnected, and was written to the channel's record as off air.
`ZAPPER_LIVE_START` (`wait`, `catchUp`, `stay`) picks how the channel starts.
Against `--burst 20 --end-after 8`, `--ending dropped` checks a takeover still
ends as one.

`--placeholder-while FILE` sends the clip only while FILE exists: delete it and
the channel comes on the air, as a broadcast that starts. `offair_check` uses
it to check the recording service end to end (docs/recording.md, "Checking
it"):

```sh
cd crates/zapper
cargo build --no-default-features --features harness,server --bin zapper-server --bin offair_check
target/debug/offair_check 19700
```

And the network failing instead: `--drop-after 5` resets the connection after
5 s with no end of stream, `--stall-after 5` stops sending and leaves it open,
and `--fault-once` applies either (or `--end-after`) to the first stream only,
so a reconnect plays. `/player_api.php` answers as an Xtream account does,
with `max_connections` from `--max-connections` (default 1) and `active_cons`
the streams being served plus `--other-devices`.

It is an archive too. `--panel-utc-offset 1` puts the server's clock an hour
east of UTC, which `player_api.php`'s `server_info` reports (`time_now`,
`timestamp_now`) as a real server does; `/streaming/timeshift.php` reads its
`start` in that zone, logs the moment it names, and lists every catch-up and
`/live/<user>/<pass>/<id>.ts` request as JSON at `/requests`. A start older
than `--archive-days` (default 3) is a 404.

`cargo run --features harness --bin catchup_check` plays that archive through
a scratch account: "five minutes ago" must reach the server as five minutes
ago, Live must open the channel again rather than seek through the
programme, a request at the archive's far edge must be moved inside it, and
one older than the archive must say so. Run it with this computer in another
zone from the server's:

```sh
tools/fake_stream.py --panel-utc-offset 1 &
TZ=America/New_York cargo run --features harness --bin catchup_check
```

### Movies

`make_fixtures.sh` also writes two 120 s "films" (640x360@25, a keyframe every
2 s, made-up subtitle text):

| Fixture | Format | Why |
|---|---|---|
| `movie.mp4` | h264, aac, `-movflags +faststart`, one `mov_text` subtitle track | The common MP4 |
| `movie.mkv` | h264, **ac3**, a `subrip` (`eng`) and an `ass` (`fra`) subtitle track | Matroska, AC-3 audio, text subtitles to extract |

`tools/fake_stream.py --movies 500` lists 500 made-up movies (ids 1..500, over
three categories; odd ids are `mkv`, even `mp4`) through `player_api.php`:
`get_vod_categories`, `get_vod_streams` (`&category_id=` filters) and
`get_vod_info&vod_id=`. `/movie/<user>/<pass>/<id>.<ext>` serves the matching
fixture as a file, with `Accept-Ranges: bytes` and `206` plus `Content-Range`
for a `Range` header. A movie being sent counts in `active_cons` like a live
stream, and `--refuse` applies to it too.

`--series N` lists N made-up shows ("Show 0001"...) over three categories:
`get_series_categories`, `get_series` (`&category_id=` filters; `last_modified`
spread over 60 days, every tenth show within the last day) and
`get_series_info&series_id=`. Show `i` has `1 + i % 3` seasons of 4 episodes,
episode id `100000 + i*100 + season*10 + n` (odd `n` `mkv`, even `mp4`). Show 2
answers its episodes as an array of arrays, show 3 adds two season-0 specials,
show 4 answers an empty `info` and `seasons`. `/series/<user>/<pass>/<id>.<ext>`
serves an episode from `movie.mp4` / `movie.mkv` exactly like a movie, and covers
point at `/placeholder.png`. `--vod-cut-at 0.8` makes every movie and episode
file announce its full length and then close the connection once 80 % of it has
been sent (a range starting past that closes at once), so a reader meets an
early end whatever its speed.

`tools/make_playlist.py --base URL [--channels N] [--movie ID]...
[--episode "NAME=ID"]... [--group G]` prints an M3U pointing at those channels,
movies and episodes of the fake server.

`cargo run --features harness --bin live_start_bench [fixture]` plays the
server under several kinds of connect burst, starting each channel both ways
that reach live — from the first frame then jumping to live ("Play right away,
then catch up to live"), and held on the first frame until the burst is in
("Wait and start at live") — and prints when the picture first
moved, how far behind live it was, and any jump after. Give it a fixture
longer than the burst plus twenty seconds (`ffmpeg -stream_loop 5 -i
motion1080p.ts -c copy long.ts`, served from a `ZAPPER_FIXTURES` directory):
the server loops its file, and the timestamps going back at each loop are a
discontinuity no live channel has.

`cargo run --features harness --bin ending_check` runs those cases end to end:
it starts the server with each set of flags, plays it in a headless, silent
mpv set up as the app's, and checks how the ending is explained (taken over,
refused with its status, not video, the network, a stall). Where the app would
reconnect by itself it does too, through the app's own decisions and the
server's `player_api.php`, with the back-off scaled to a tenth; and it checks
the server saw exactly the stream requests it should — nothing after a
takeover or a refusal, and nothing while another device holds the account's
one connection. Given a local URL as its argument, it plays that once and
prints how it ended. `ZAPPER_ENDING_ONLY=stall` runs only the scenarios with
that in their name; `ZAPPER_ENDING_DEBUG=1` prints mpv's warnings, and
`ZAPPER_ENDING_LOG=debug` asks mpv for more of its log than the app does.

### Driving the player at a fixture

`play()` resolves stream ids through the configured source, so env vars redirect
it:

```sh
tools/fake_stream.py &
ZAPPER_FIXTURE_BASE=http://127.0.0.1:8788 \
ZAPPER_FIXTURE_NAME=sport720p60.ts \
ZAPPER_STREAM_ID=1 cargo run --bin player_check
```

`player_check` sets the first two itself when they are unset (to the server
above and `sample1080.ts`). Unset, they are inert, and the configured source
is used. Play, seek back, return to live and stop all work this way with no
account at all, so nothing about the player needs a provider to test.

## OpenSubtitles, without OpenSubtitles

`tools/fake_opensubtitles.py --port N` stands in for the OpenSubtitles REST
API with made-up data, so tests never reach the real service. Point zapper at
it with `ZAPPER_OPENSUBTITLES_URL=http://127.0.0.1:N`. It needs an `Api-Key`
and a `User-Agent` starting `zapper v` (else 403), signs in `tester` /
`test-pass` (tokens are always `fake-os-token-<n>`), redirects an unsorted or
mixed-case search query as the real one does, answers a few fixed titles
(`tmdb_id=900001`, `parent_tmdb_id=800001` with season 1 episode 2,
`query=film 0002`, `query=show 0002`), and serves downloads only with a
bearer token, up to `--quota` per user (then 406). It also serves
`DELETE /api/v1/logout` and `GET /api/v1/infos/user`.

Failure switches: `--busy-after N` (429 with `Retry-After` after N requests
in 10 s, `--retry-after S`), `--down` (503), `--token-ttl S` (a token stops
working on `/download` after S seconds). `/files/<file_id>.srt` serves 20 cues
(`7000099`: Latin-1 with CRLF; `7000098`: an HTML page; `7000097`: empty).
`GET /_stats` returns counters (logins, logouts, infos, searches, downloads,
files, refused, `authorized_files` — file fetches that carried an `Authorization`
header, which a client should never send to the link's host — and the API hosts
that received one) and never a token or password. `/download` without a
`file_id` is a 400 and costs no download.

`fake_stream.py` gives odd movies a `tmdb_id` (900000 + id) and odd shows a
`tmdb` (800000 + id) in `get_vod_streams`/`get_vod_info` and
`get_series`/`get_series_info`; even ones have none, which is the search-by-name
path.

## Never the real account

Many providers allow one connection per account, and end the older one when
another device opens a stream. A test that plays through the account on this
machine takes the stream from whoever is watching on it. So no harness uses
the real account or catalogue unless told to in so many words:

```sh
ZAPPER_ALLOW_REAL_ACCOUNT=1     # exactly 1; nothing else counts
```

Without it:

- `player_check` plays the fixture server, on a scratch account.
- `open_bench`, `seek_bench` and `filter_bench` default to `http://127.0.0.1:8788/sample1080.ts`, and refuse
  a URL that is not on this machine (loopback or a file).
- `tools/check_all.sh` points everything at a scratch `ZAPPER_HOME`, and
  unsets the opt-in even if the calling shell exported it.

`ZAPPER_HOME=<dir>` moves config, data and cache to `<dir>/config`,
`<dir>/data` and `<dir>/cache` for any run. Unit tests (`cargo test`) never
see the real directories at all, whatever the environment says: under test
they resolve into a sandbox in the system's temporary directory
(`crates/zapper/src/paths.rs`).

## Screenshots

For interface work, drive the frontend in a browser: `npm run dev` with the
mock backend, then a headless Playwright script. That gives true screenshots
of everything except video.

## Checking everything

```sh
tools/check_all.sh          # tests, lint, build, the recording and service checks
tools/check_all.sh --host   # also the player checks, on a headless mpv host
```

`--host` runs `player_check` (live, `--movie`, `--held`, `--episode`, `--up-next`,
`--subtitles`, `--find-subtitles`, `--captions`, `--trickplay-vod` and the rest), `catchup_check`,
`ending_check` and `handover_check` against the fixture servers. mpv opens its own window, so the
ones that need one run inside `tools/headless_host.sh`: a headless gnome-shell with a virtual
monitor, a scratch HOME and XDG_RUNTIME_DIR, no sound and no core dumps. Nothing opens on the
display of whoever is at the machine, but the run plays video for a few minutes: run it when the
player or the mpv backend changed, not on every edit.

## Other tools

- `fetch-libmpv.ps1`: the bundled libmpv and its import library for Windows
  builds (docs/windows.md); `scripts/fetch-libmpv.sh` elsewhere.
- `licenses.mjs`: the Vite plugin that writes the interface's third-party
  notices (docs/licensing.md).
