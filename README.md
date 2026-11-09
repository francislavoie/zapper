<p align="center">
  <img src="packaging/icons/icon.png" alt="zapper" width="128" height="128">
</p>

<h1 align="center">zapper</h1>

<p align="center">A live-TV player and recording service for IPTV services, HDHomeRun tuners and Tvheadend.</p>

---

zapper plays live TV from Xtream Codes servers, M3U playlists with XMLTV guides, HDHomeRun
tuners and Tvheadend servers. It keeps a local copy of the channel list and guide, so browsing and
searching don't wait on the network, and `zapper-server` records from the guide and serves the
interface to any browser on your network.

> **This line is the rewrite.** The desktop app is being rebuilt (Rust, egui and mpv), so this
> line ships `zapper-server` and the browser interface. The Tauri desktop app (the 0.4.x releases:
> `.deb`, `.rpm`, Arch package, Windows installers) lives on the
> [`main-tauri`](https://github.com/francislavoie/zapper/tree/main-tauri) branch. Your settings,
> catalogue and recordings carry over unchanged: the data folders (`~/.config/zapper`,
> `~/.local/share/zapper`, `~/.cache/zapper`; `%APPDATA%\zapper` and `%LOCALAPPDATA%\zapper` on
> Windows; `~/Library/Application Support/zapper` on macOS), their `ZAPPER_*` overrides and the app
> id (`dev.lavoie.zapper`) are kept, so the new app upgrades in place.

![zapper's browse view: a football match playing in the picture at the top right, the playing channel's now and next programmes beside it, and below, the category list on the left with the guide's timeline of what's on across the sports channels](docs/images/screenshot.webp)

## What it ships

| What | Where |
|---|---|
| `zapper-server`, with the interface built in | a release's `zapper-server-<version>-linux-x86_64` (one static file, any x86-64 Linux) and `zapper-server-<version>-windows-x86_64.zip` |
| Playback in a browser | the interface `zapper-server` serves: live TV, movies, shows, the guide and recordings, behind a password |
| The desktop app | being rebuilt; the Tauri one is on `main-tauri` |

macOS: `zapper-server` builds and its tests run in CI ([docs/macos.md](docs/macos.md)); it isn't released.

## Running it

Download `zapper-server` from the [latest release](https://github.com/francislavoie/zapper/releases/latest)
(`SHA256SUMS` lists the checksums), or build it ([below](#building-from-source)), then:

```sh
chmod +x zapper-server-<version>-linux-x86_64
./zapper-server-<version>-linux-x86_64 --import-account account.json   # your provider account
./zapper-server-<version>-linux-x86_64 --set-password                  # the browser sign-in
./zapper-server-<version>-linux-x86_64 --network on                    # serve other devices
./zapper-server-<version>-linux-x86_64
```

It listens on `http://127.0.0.1:8796` (the port is `server.json`'s, or `--port`); open it in a
browser and sign in. `--install-service` keeps it running from then on (a systemd user unit, a
Windows sign-in task or service, a macOS LaunchAgent). [docs/recording.md](docs/recording.md) has
the settings and the API; [packaging/server/](packaging/server/) has a systemd unit and a
`Dockerfile`.

## Features

The short list; [docs/features.md](docs/features.md) has how each part behaves.

- **Sources:** Xtream Codes (live, guide, catch-up, movies and shows), M3U playlists with XMLTV
  guides, HDHomeRun tuners and Tvheadend servers. Export and import of settings, favourites and
  categories.
- **Browsing:** the channel list and guide are kept locally, so search is instant. Live TV as a
  list or a guide, Movies and Shows by category, channel and programme search, pinned, hidden and
  sorted categories, a command palette (`Ctrl+P`).
- **Playback:** live with a rewind buffer and catch-up; movies and episodes with a seek bar,
  resume, Continue watching, Up next and Find subtitles (OpenSubtitles). A refused or ended stream
  says why.
- **Recording:** from the guide, or what you're watching, with files named for Jellyfin and Plex,
  a backup channel if the feed fails, and one-connection accounts shared between recordings and
  the player.
- **In a browser:** with network access on, the service serves the interface behind a password;
  the provider's address never reaches the browser.
- No telemetry: zapper talks only to your sources and the online services you turn on.

## Building from source

Requirements: a stable Rust toolchain, Node.js 26 (see `.nvmrc`) and npm,
[`just`](https://github.com/casey/just), and `pkg-config`. libmpv and FFmpeg are zapper's own LGPL
build, which `just fetch-libmpv` downloads into `crates/zapper/lib/`
([docs/licensing.md](docs/licensing.md#mpv-and-ffmpeg)); without it a development build falls
back to the system's (`libmpv-dev`, `libavformat-dev` and friends). The release-script checks want
`jq`; the license notices want [`cargo-about`](https://github.com/EmbarkStudios/cargo-about)
0.9.2 and Python 3; the test streams in `tools/` want Python 3 and `ffmpeg`.

```sh
npm ci
just fetch-libmpv       # the bundled libmpv and FFmpeg, into crates/zapper/lib/
just server             # the interface, then target/release/zapper-server
target/release/zapper-server
just dev                # the interface alone, against a mock backend, at http://localhost:5180
just test               # Rust, interface and release-script checks
just check              # the above plus lint, the build and the recording checks
just check --host       # also the player checks, on a headless mpv host (needs gnome-shell)
just package-server     # the static Linux binary, in a container (Docker or Podman)
```

The crate has three features, all off by default: `server` (the recording service and its
browser interface), `mpv` (the libmpv backend) and `harness` (the check programs).
[docs/architecture.md](docs/architecture.md) says how the parts fit.
[docs/windows.md](docs/windows.md) covers Windows, [docs/macos.md](docs/macos.md) macOS and
[docs/releasing.md](docs/releasing.md) cutting a release.

## Documentation

- [docs/architecture.md](docs/architecture.md): the crates, the event bus, the player backend and the host
- [docs/features.md](docs/features.md): how each feature behaves, in detail
- [docs/recording.md](docs/recording.md): the recording service, its settings and its API
- [docs/releasing.md](docs/releasing.md): versions, releases and the updater
- [docs/windows.md](docs/windows.md): building and running on Windows
- [docs/macos.md](docs/macos.md): building on macOS and what CI checks there
- [docs/licensing.md](docs/licensing.md): how third-party licenses are collected and shipped
- [tools/README.md](tools/README.md): test fixtures and development tools

## License

[MIT](LICENSE). zapper-server carries libmpv's FFmpeg, built without its GPL parts and so under
the LGPL; see [docs/licensing.md](docs/licensing.md) for what is in it and where its source is.

zapper is a player. It doesn't provide any channels or content: bring your own tuner, server or
playlist, and use it with content you're entitled to watch.
