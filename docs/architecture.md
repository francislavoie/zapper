# Architecture

This line is the rewrite: the Tauri desktop app is gone (it lives on the `main-tauri` branch,
0.4.x), and what is left is a toolkit-free core, the libmpv backend, the recording service and the
browser interface. A new desktop app (egui and mpv) will sit on top of the same core.

## Crates

One crate today, `crates/zapper` (package `zapper`, library `zapper_lib`), in a workspace whose
root `Cargo.toml` holds the version (`[workspace.package]`, read by `scripts/version.py current`).
Its features, all off by default, mark the seams where it will be split into three crates:

| Feature | What it adds | Becomes |
|---|---|---|
| none | catalogue, sources (Xtream, M3U, HDHomeRun, Tvheadend), online services, the player controller and its tracks and captions, the event bus, paths, updates | `zapper-core`: no native libraries |
| `mpv` | the libmpv backend, the headless host, the preview frames of a title's seek bar | `zapper-mpv` |
| `server` | the recording service, its HTTP API and the browser interface compiled in, the remuxers (FFmpeg) | `zapper-server`, which does not depend on `zapper-mpv` |
| `harness` | the check programs' helpers (`bin/*_check.rs`, `bin/*_bench.rs`) | each crate's own bins |

## The event bus

Parts of the core that used to call a window toolkit's `emit` send an `Event` (sync status, player
state, player options, channel quality, update) through one `EventBus` (`events.rs`), a tokio
broadcast channel. Anything that wants them subscribes: the server turns them into its
server-sent events, a future app turns them into repaints. A subscriber that falls behind never
stalls the sender. A forwarder that passes every kind of event on (as the Tauri app's webview
forwarder did) uses plain `recv` and carries on past a lag, so it loses only what the channel had
already dropped; a consumer that only wants the newest state uses `recv_latest`, which skips to
the newest event still held.

## The player backend

The controller (`player/`) is what the interface's commands call: play, seek, pause, tracks,
captions, endings, resume points. It calls mpv only through the `PlayerBackend` trait
(`player/backend.rs`), from any thread: libmpv's client API is thread-safe, so there is no main
thread to hop onto. `surface/mpv.rs` implements it over libmpv; `player/fake.rs` implements it for
tests, recording every call and answering with scripted signals. `BackendCaps` says what a backend
can do (rewind buffer, cache dump, audio filters, caption geometry).

## The host

A backend needs somewhere to run. `surface/host.rs` is the headless one: `open_host` makes an mpv
that opens its own window (`force-window`, `vo=gpu-next`), installs it as the backend and feeds
drawn-frame thumbnails to the live trickplay store. The player checks (`player_check`,
`catchup_check`) drive the controller with it, so they need no app; because mpv opens a window,
`tools/headless_host.sh` runs them in a headless gnome-shell (`tools/check_all.sh --host`). The
windowless libmpv checks (`ending_check`, `handover_check`, the benches) need no host.

## The server

`zapper-server` (`bin/zapper_server.rs`) hosts the recording service: it syncs the catalogue,
records, serves the HTTP API and the browser interface (the frontend's build, embedded by
`build.rs`), and installs itself as a service. The browser talks to it as the app did, so the same
interface runs in both. `recording_check`, `offair_check` and `service_check` run it for real on
fixture streams.

## What stays the same

Data folders (`paths.rs`: `~/.config/zapper`, `~/.local/share/zapper`, `~/.cache/zapper`, and the
Windows and macOS equivalents), their `ZAPPER_HOME`, `ZAPPER_CONFIG_DIR`, `ZAPPER_DATA_DIR` and
`ZAPPER_CACHE_DIR` overrides, and the app id `dev.lavoie.zapper` (with `APP_ID = "zapper"`) are
unchanged, so an installed copy of the old app and its successor share one set of files.
