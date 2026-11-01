# tools

## Test against a local stream

Real streams take seconds to open, their content changes minute to minute,
and many providers allow a single connection at a time. The harnesses play a
short synthetic stream from a local server instead:

```sh
tools/make_fixtures.sh                    # once: generates the fixtures with ffmpeg
tools/fake_stream.py &                    # serves them on 127.0.0.1:8788
ZAPPER_STREAM_URL=http://127.0.0.1:8788/sample1080.ts cargo run --bin surface_check
```

Fixtures live in `~/.cache/zapper/fixtures` (`ZAPPER_FIXTURES` overrides it),
outside the repository: they are generated, and tens of megabytes each.

| Fixture | Format | Why |
|---|---|---|
| `sample1080.ts` | h264 1920x1080@50, aac | The test pattern with little motion |
| `motion1080p.ts` | h264 1920x1080@25, aac | 1080p with the whole frame moving |
| `sport720p60.ts` | h264 1280x720@60, **ac3** | 60 fps, high motion, and the only AC-3 audio |

Between them: 720p and 1080p, 25/50/60 fps, aac and ac3, low and high motion.
All are H.264 8-bit 4:2:0 in MPEG-TS with a keyframe every two seconds, the
shape most live IPTV feeds have. Any other `.ts` file dropped into the
fixtures directory is served too.

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

And the network failing instead: `--drop-after 5` resets the connection after
5 s with no end of stream, `--stall-after 5` stops sending and leaves it open,
and `--fault-once` applies either (or `--end-after`) to the first stream only,
so a reconnect plays. `/player_api.php` answers as an Xtream account does,
with `max_connections` from `--max-connections` (default 1) and `active_cons`
the streams being served plus `--other-devices`.

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

### Driving the app itself at a fixture

`play()` resolves stream ids through the configured source, so env vars redirect
it:

```sh
tools/fake_stream.py &
ZAPPER_FIXTURE_BASE=http://127.0.0.1:8788 \
ZAPPER_FIXTURE_NAME=sport720p60.ts \
ZAPPER_STREAM_ID=1 cargo run --bin player_check
```

`player_check` sets the first two itself when they are unset (to the server
above and `sample1080.ts`). In the app they are inert when unset, and the
configured source is used. Play, seek back, return to live and stop all work
this way with no account at all, so nothing about the surface, player or
interface needs a provider to test.

The app itself, muted and at a fixture:

```sh
ZAPPER_MUTE=1 ZAPPER_FIXTURE_BASE=http://127.0.0.1:8788 \
ZAPPER_FIXTURE_NAME=sample1080.ts ZAPPER_AUTOPLAY=1 cargo run
```

## Never the real account

Many providers allow one connection per account, and end the older one when
another device opens a stream. A test that plays through the account on this
machine takes the stream from whoever is watching on it. So no harness, and no
unattended run of the app, uses the real account or catalogue unless told to
in so many words:

```sh
ZAPPER_ALLOW_REAL_ACCOUNT=1     # exactly 1; nothing else counts
```

Without it:

- `player_check` plays the fixture server, on a scratch account.
- `surface_check`, `shot_check`, `open_bench`, `seek_bench` and
  `filter_bench` default to `http://127.0.0.1:8788/sample1080.ts`, and refuse
  a URL that is not on this machine (loopback or a file).
- `ipc_check` builds a fixture catalogue (600 categories, 12,000 channels, a
  guide around now) in a scratch directory and checks the commands over that.
- The app run with `ZAPPER_HARNESS`, `ZAPPER_AUTOPLAY`, `ZAPPER_CAPTURE` or
  `ZAPPER_MUTE` set gets a scratch config, data and cache directory: no
  account, so no sync and nothing to play but a fixture.
- `tools/check_all.sh` points everything at a scratch `ZAPPER_HOME`, and
  unsets the opt-in even if the calling shell exported it.

`ZAPPER_HOME=<dir>` moves config, data and cache to `<dir>/config`,
`<dir>/data` and `<dir>/cache` for any run. Unit tests (`cargo test`) never
see the real directories at all, whatever the environment says: under test
they resolve into a sandbox in the system's temporary directory
(`src-tauri/src/paths.rs`).

## Screenshots

`shot.py` finds a window by title and captures it with ffmpeg's x11grab.

It works on an X11 session or against Xvfb, not under a GNOME Wayland
session: Wayland-native windows capture black, and under XWayland there is no
shared framebuffer for x11grab to read, so the result is a flat colour.

For interface work, drive the frontend in a browser instead: `npm run dev`
with the mock backend, then Playwright. That gives true screenshots of
everything except video compositing.

## Seeing the real window

`cargo run --bin shot_check -- out.png [seconds]` reconstructs what the window
is showing and writes a PNG.

It composites two layers rather than screenshotting: the video read back from
the GL framebuffer with `glReadPixels`, and the interface rendered through
Cairo, which preserves per-pixel alpha exactly. That is necessary because
GNOME refuses D-Bus screenshots to unauthorised callers, Wayland-native
windows capture black, and XWayland has no shared framebuffer for x11grab.

It is a reconstruction, not a screenshot — it cannot prove the compositor puts
the result on screen. It does catch the failures that matter: a black surface,
an inverted picture, chrome painting opaque over video, a video rect in the
wrong place.

```sh
tools/fake_stream.py &
ZAPPER_STREAM_URL=http://127.0.0.1:8788/motion1080p.ts \
  cargo run --bin shot_check -- /tmp/shot.png 12
```

## Checking everything

```sh
tools/check_all.sh            # tests, lint, build, a fixture catalogue — no windows
tools/check_all.sh --windowed # also GL compositing and playback
```

The catalogue check builds a fixture mirror rather than reading the one a
configured copy of zapper has synced; `ZAPPER_ALLOW_REAL_ACCOUNT=1 cargo run
--features harness --bin ipc_check` checks the real one (`~/.local/share/zapper`)
when that is what is wanted. The windowed harnesses open real
windows and play video, which interrupts whoever is at the machine. Run them
when surface, mpv or rect behaviour changed — not on every edit.

## Other tools

- `install-desktop.sh`: a per-user desktop entry and icons for a build run
  from this tree (README-install.md explains when, and when not, to use it).
- `fetch-libmpv.ps1`: the bundled libmpv and its import library for Windows
  builds (docs/windows.md); `scripts/fetch-libmpv.sh` elsewhere.
- `licenses.mjs`: the Vite plugin that writes the interface's third-party
  notices (docs/licensing.md).
