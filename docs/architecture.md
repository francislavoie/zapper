# Architecture

This line is the rewrite: the Tauri desktop app is gone (it lives on the `main-tauri` branch,
0.4.x), and what is left is a toolkit-free core, the libmpv backend, the recording service and the
browser interface. A new desktop app (egui and mpv) will sit on top of the same core.

## Crates

Three crates in one workspace, whose root `Cargo.toml` holds the version (`[workspace.package]`,
read by `scripts/version.py current`, bumped with each crate's `Cargo.lock` entry by
`scripts/set-version.py`):

| Crate | What it holds | Depends on |
|---|---|---|
| `zapper-core` (`zapper_core`) | catalogue, sources (Xtream, M3U, HDHomeRun, Tvheadend), online services, the player controller and its tracks, captions, trickplay store and title-preview worker, the event bus, paths, updates, the interface's contract (`api`), and what the recording service shares with a front end (`recording`: its API, client, config, store and pure decisions) | no libmpv, no FFmpeg, no build script of its own |
| `zapper-mpv` (`zapper_mpv`) | the libmpv backend (`backend`), the headless host (`host`), the title previews' decoder (`thumbframe`, a C file against the FFmpeg libmpv loads); the player checks and benches | `zapper-core` |
| `zapper-server` (`zapper_server`, bin `zapper-server`) | the recording service: capture, shared connections, its HTTP API and the browser interface compiled in, the remuxers (FFmpeg); its checks | `zapper-core` only, never `zapper-mpv` |

Features, all off by default: `harness` (in each crate; the check programs and, in the core, the
`harness` module and the player's stand-in `player::fake`), and the core's `test-support`, which
only the other crates' dev-dependencies turn on, for their unit tests: `paths` keeps every
directory under a test root there as it does in the core's own tests, and the in-memory catalogue
and the HTTP stand-ins are reachable.

`zapper-core` builds without a C compiler of its own, but two of its dependencies compile C:
SQLite (rusqlite's `bundled`) and aws-lc (rustls, under reqwest). `cargo tree -p zapper-core`
lists `cc` and `pkg-config` only as their build dependencies; libmpv2 and axum never.

The libmpv bundle (`just fetch-libmpv`) unpacks into `lib/<platform>/` at the top of the
workspace; `zapper-mpv` and `zapper-server` find it through the same code
(`packaging/libmpv/bundle.rs`, included by both build scripts).

## The event bus

Parts of the core that used to call a window toolkit's `emit` send an `Event` (sync status, player
state, player options, channel quality, update) through one `EventBus` (`events.rs`), a tokio
broadcast channel. Anything that wants them subscribes: a future app turns them into repaints.
The server does not listen to it; browsers poll its HTTP API for what they show. A subscriber
that falls behind never stalls the sender. A forwarder that passes every kind of event on (as the
Tauri app's webview forwarder did) uses plain `recv` and carries on past a lag, so it loses only
what the channel had already dropped; a consumer that only wants the player's state uses
`recv_latest_player_state`, which skips every other kind of event and, after a lag, goes to the
newest player state still held, so an event of another kind after it cannot hide it.

## The player backend

The controller (`player/`) is what the interface's commands call: play, seek, pause, tracks,
captions, endings, resume points. It calls mpv only through the `PlayerBackend` trait
(`player/backend.rs`), from any thread: libmpv's client API is thread-safe, so there is no main
thread to hop onto. `zapper_mpv::backend` implements it over libmpv; `player/fake.rs` implements it
for tests, recording every call and answering with scripted signals. `BackendCaps` says what a
backend can do (rewind buffer, cache dump, audio filters, caption geometry).

The seek bar's previews of a movie or an episode are filled by a worker in the core
(`player::title_thumbs`), which decides which moments to fill and when playback can spare the
machine; decoding a picture and compressing it is FFmpeg's, so it calls a `Decoder` that
`zapper_mpv::thumbframe` installs with the player. That keeps the core free of C, and the worker's
scheduling with the controller it serves; a front end with no decoder installed (a stand-in
player, a build without FFmpeg's development files) keeps the frames drawn while a title plays.

A front end opens the core with `player::startup` once its backend is installed: the saved
settings applied, the controller started on its runtime, the catalogue's periodic sync and its
watch for changes made elsewhere, and the recording service resumed after an update (Windows).
The player checks call it with the player alone (`Startup::PLAYER_ONLY`).

## The host

A backend needs somewhere to run. `zapper_mpv::host` is the headless one: `open_host` makes an mpv
that opens its own window (`force-window`, `vo=gpu-next`), installs it as the backend and feeds
drawn-frame thumbnails to the live trickplay store. The player checks (`player_check`,
`catchup_check`) drive the controller with it, so they need no app; because mpv opens a window,
`tools/headless_host.sh` runs them in a headless gnome-shell (`tools/check_all.sh --host`). The
windowless libmpv checks (`ending_check`, `handover_check`, the benches) need no host.

## The server

`zapper-server` (`crates/zapper-server`, `src/bin/zapper_server.rs`) hosts the recording service:
it syncs the catalogue, records, serves the HTTP API and the browser interface (the frontend's
build, embedded by its `build.rs`), and installs itself as a service. The browser talks to it as
the app did, so the same interface runs in both. `recording_check`, `offair_check` and
`service_check` run it for real on fixture streams.

## Notes for the desktop app

- The egui app will need mpv's load gate again for a render-API `vo` (the picture drawn into the
  app's own surface rather than a window mpv makes): a load has to wait until the render context
  exists. The headless host does not need it, so this line has none.
- `player::handover::recordings()` and `noticed()` have no caller until a front end fetches the
  service's recordings through them, so tuning to a channel the service records joins its
  connection (`player::captured`) only once one does: dormant on this line.
- At quit, call `player::shutdown()` (as the player checks do at exit): it writes where a movie
  or an episode got to, uninstalls the backend and marks the player stopped, then stops it,
  waiting at most two seconds. The old app's quit had a watchdog for the same reason.
- `updates::start` (the updater's state and its periodic check) has no caller on this line.
- What went with the Tauri app and is the desktop app's to restore: the media session
  (souvlaki: media keys and the desktop's now-playing), remembering the window's size, and
  quitting in order on SIGTERM, SIGINT and SIGHUP.
- The release zip's licence notices are checked on the Windows build (`--licenses` prints them);
  the node checks of the app package's notices went with the Tauri app. A server-side check of
  the zip's notices against `scripts/licenses.sh`'s output is still to write.

## What stays the same

Data folders (`paths.rs`: `~/.config/zapper`, `~/.local/share/zapper`, `~/.cache/zapper`, and the
Windows and macOS equivalents), their `ZAPPER_HOME`, `ZAPPER_CONFIG_DIR`, `ZAPPER_DATA_DIR` and
`ZAPPER_CACHE_DIR` overrides, and the app id `dev.lavoie.zapper` (with `APP_ID = "zapper"`) are
unchanged, so an installed copy of the old app and its successor share one set of files.
