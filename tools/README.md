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
connection with 20 s of video at once, and `--gap-every 4` goes quiet every
four seconds and then sends the backlog in a burst (playing right at the live
edge survives an even feed and stalls on that one).

### Driving the app itself at a fixture

`play()` resolves stream ids through the configured source, so env vars redirect
it:

```sh
tools/fake_stream.py &
ZAPPER_FIXTURE_BASE=http://127.0.0.1:8788 \
ZAPPER_FIXTURE_NAME=sport720p60.ts \
ZAPPER_STREAM_ID=1 cargo run --bin player_check
```

Unset, they are inert and the configured source is used. Play, seek back,
return to live and stop all work this way with no account at all, so nothing
about the surface, player or interface needs a provider to test.

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
tools/check_all.sh            # tests, lint, build, the local mirror — no windows
tools/check_all.sh --windowed # also GL compositing and playback
```

The local-mirror check reads the catalogue database a configured copy of
zapper has synced (`~/.local/share/zapper`). The windowed harnesses open real
windows and play video, which interrupts whoever is at the machine. Run them
when surface, mpv or rect behaviour changed — not on every edit.

## Other tools

- `install-desktop.sh`: a per-user desktop entry and icons for a build run
  from this tree (README-install.md explains when, and when not, to use it).
- `fetch-libmpv.ps1`: libmpv and its import library for Windows builds
  (docs/windows.md).
- `licenses.mjs`: the Vite plugin that writes the interface's third-party
  notices (docs/licensing.md).
