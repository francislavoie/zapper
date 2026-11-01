<p align="center">
  <img src="src-tauri/icons/icon.png" alt="zapper" width="128" height="128">
</p>

<h1 align="center">zapper</h1>

<p align="center">A fast, keyboard-friendly live TV player for IPTV, HDHomeRun and Tvheadend.</p>

---

zapper is a desktop player for live television. Point it at an Xtream Codes server, an M3U
playlist, an HDHomeRun tuner or a Tvheadend server, and it mirrors the whole channel list and
guide locally, so browsing tens of thousands of channels is instant and never interrupts what
you're watching.

It's named for what it's meant to be good at: zapping. Changing channel should feel immediate,
browsing should leave the picture playing, and opening the app shouldn't mean waiting on a sync.

It's built with a Rust core, a React interface and [Tauri](https://tauri.app/), with
[mpv](https://mpv.io/) (libmpv) doing the playback, drawn straight beneath the window rather than
through the browser engine.

> **Status:** used day to day on Linux. Windows builds and runs; its installers aren't published
> yet. macOS isn't supported yet.

## Platforms

| Platform | Status |
|---|---|
| Linux | Supported: `.deb` and `.rpm` packages |
| Windows | Builds and runs (10 and 11); installers not published yet |
| macOS | Not supported yet |

## Installing

Download the package for your system from the
[latest release](https://github.com/francislavoie/zapper/releases/latest):

- **Debian and Ubuntu:** `sudo apt install ./zapper_<version>_amd64.deb`
- **Fedora:** `sudo dnf install ./zapper-<version>-1.x86_64.rpm`

zapper uses your distribution's libmpv and WebKitGTK, which the package pulls in. `SHA256SUMS` in
each release lists the files' checksums. After that, zapper updates itself (see
[Updates](#updates)).

## Features

### Sources

- **Xtream Codes** servers: a URL, username and password. Live channels, categories, the guide,
  and catch-up on channels that have an archive.
- **M3U playlists** from a URL or a file, with one or more **XMLTV** guides. Catch-up through the
  playlist's `catchup` attributes.
- **HDHomeRun** tuners, found by address. Channels come from the tuner's lineup, the guide from
  SiliconDust's or your own XMLTV, and subchannels read as `5.1`. A busy tuner says so.
- **Tvheadend** servers, with or without a login. Categories come from its channel tags, the
  guide from its XMLTV export.

### Browsing

- **The whole catalogue, locally.** Every channel and the guide are mirrored into SQLite and
  refreshed in the background. Startup shows the last mirror at once, without waiting on the
  network.
- **Instant search** across every channel name, from the filter box, `/`, or the command palette.
- **List and guide.** Switch with `G`. The guide glides through time with the wheel
  (`Shift`+wheel), a scrollbar, or an hour at a time with `←`/`→`, and `Home` brings it back to
  now.
- **Categories your way.** Pin them to the top, hide the ones you never watch, and sort the rest
  by the provider's order, A–Z or size. The rail folds away when you want the room.
- **Favourites**, in their own tab.
- **Browse without stopping.** The picture stays docked, with its own controls, while you look
  for the next thing to watch.
- **Command palette** (`Ctrl+P`) with every action and channel search; every shortcut is listed in
  Settings › Keyboard.

### Playback

- **libmpv** with hardware decoding (VA-API on Linux, frames kept on the GPU), HEVC, and HDR
  tone-mapped to your display.
- **Quick tune-in.** Playback starts a few seconds behind live, so a stream that hiccups doesn't
  stall, and catches up once the server's opening burst has arrived.
- **Rewind live TV.** A timeshift buffer whose size you set in megabytes (Settings shows roughly
  how many minutes that buys), skip back and forward 5 seconds, and `L` to jump to live.
- **Catch-up** on channels whose source offers an archive.
- **Volume that sounds right.** The slider follows perceived loudness rather than raw gain, goes
  up to 200% for quiet channels, and is remembered between launches.
- **Dynamic range compressor** that evens out loud adverts and quiet dialogue, toggled instantly.
- **Subtitles** styled your way: font, size, colour and backdrop, with a live preview.
- **On the picture:** wheel for volume, double-click the middle for fullscreen, double-click
  either edge to skip. Theater mode (`T`) fills the window with the picture.
- **Stats for nerds** (`S`): bitrate, buffer, resolution, codec, and dropped frames and stalls
  on one graph, with the value under the pointer.

### The app

- **Picks up where you left off:** the last channel, the window's size, the volume.
- **App zoom** (`Ctrl` `+`/`−`/`0`) that scales the whole interface, video placement included.
- **Updates itself** from GitHub releases, each download checked against the release's checksums.
- **Your credentials stay yours.** They're stored in your config directory, readable only by you,
  and redacted from logs.
- **No telemetry.** Apart from your own sources, the only request zapper makes on its own is the
  daily update check, which you can turn off.
- **Free and MIT licensed.**

## Performance

Rough numbers, to give a sense of it. Measured on a Framework 13 laptop (Ryzen 7 7840U,
Radeon 780M graphics) on Ubuntu, with a release build, a synthetic 50,000-channel playlist and a
1080p25 H.264 test stream:

| | |
|---|---|
| Start to window and video surface ready | ~0.5 s |
| First import of 50,000 channels | ~0.9 s (catalogue ready ~1.4 s after launch) |
| Searching 50,000 channel names | 2–3.5 ms per query |
| Tune to first frame | ~0.2–0.5 s when the server bursts on connect, as most do; ~1–1.3 s when it sends in real time |
| CPU while playing 1080p | ~11% of one core (hardware decoding) |
| CPU while idle | ~0.1% of one core |
| Memory, all processes (PSS) | ~210 MB idle with 50,000 channels loaded, ~290 MB playing |
| Package size | 7.6 MB `.deb` (libmpv and WebKitGTK come from the system) |

## Updates

zapper checks its GitHub releases a minute after startup and once a day after that. When there's
a newer version, it offers to download the package for how you installed it, keeps the download
only if the release's `SHA256SUMS` lists it with the same digest, and installs it:

- **Linux (`.deb`, `.rpm`):** through `pkexec`, so your desktop asks for your password. The
  package is copied somewhere only root can write and checked again before apt, dnf, zypper or
  rpm sees it.
- **Restart** then starts the new version.

A build from source only links to the release page. Settings › Updates turns the automatic check
off.

## Building from source

Requirements: a stable Rust toolchain, Node.js 26 (see `.nvmrc`) and npm,
[`just`](https://github.com/casey/just), and the libmpv and WebKitGTK development packages. On
Debian and Ubuntu:

```sh
sudo apt install libwebkit2gtk-4.1-dev libmpv-dev libgtk-3-dev libsoup-3.0-dev \
  libjavascriptcoregtk-4.1-dev librsvg2-dev libxdo-dev libayatana-appindicator3-dev \
  libepoxy-dev pkg-config
```

Packaging and the release-script checks also want `jq`, `dpkg-dev` and `desktop-file-utils`;
the license notices want [`cargo-about`](https://github.com/EmbarkStudios/cargo-about) 0.9.2 and
Python 3; the test streams in `tools/` want Python 3 and `ffmpeg`.

Then:

```sh
npm ci
just dev           # run a development build
just test          # Rust, interface and release-script checks
just package       # build the .deb and .rpm
just licenses      # regenerate the third-party license notices
```

[docs/windows.md](docs/windows.md) covers building on Windows, and
[docs/releasing.md](docs/releasing.md) cutting a release.

## Documentation

- [docs/releasing.md](docs/releasing.md): versions, packages, releases and the updater
- [docs/windows.md](docs/windows.md): building and running on Windows
- [docs/licensing.md](docs/licensing.md): how third-party licenses are collected and shipped
- [tools/README.md](tools/README.md): test fixtures and development tools

## License

[MIT](LICENSE). zapper links libmpv and, through it, FFmpeg; see
[docs/licensing.md](docs/licensing.md) for what that means for each platform's packages.

zapper is a player. It doesn't provide any channels or content: bring your own tuner, server or
playlist, and use it with content you're entitled to watch.
