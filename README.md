<p align="center">
  <img src="src-tauri/icons/icon.png" alt="zapper" width="128" height="128">
</p>

<h1 align="center">zapper</h1>

<p align="center">A desktop player for live TV from IPTV services, HDHomeRun tuners and Tvheadend.</p>

---

zapper is a desktop player for live TV from Xtream Codes servers, M3U playlists with XMLTV
guides, HDHomeRun tuners and Tvheadend servers. It keeps a local copy of the channel list and
guide, so browsing and searching don't wait on the network, and the current channel keeps
playing in a corner while you browse.

It's built with a Rust core, a React interface and [Tauri](https://tauri.app/), with
[mpv](https://mpv.io/) (libmpv) for playback.

![zapper's browse view: a football match playing in the picture at the top right, the playing channel's now and next programmes beside it, and below, the category list on the left with the guide's timeline of what's on across the sports channels](docs/images/screenshot.webp)

> **Status:** used day to day on Linux. Windows installers are published from the next release
> on, newer and less tried than the Linux packages. macOS (Apple Silicon) builds and plays in CI,
> but hasn't been tried on a real Mac or released yet.

## Platforms

| Platform | Status |
|---|---|
| Linux | Supported: `.deb`, `.rpm` and Arch Linux packages |
| Windows | Supported (10 and 11, x64): a setup for you alone or an MSI for the whole computer |
| macOS | Apple Silicon: builds, packages (`.dmg`) and plays in CI; not yet tried on a real Mac or released ([docs/macos.md](docs/macos.md)) |

## Installing

Download the package for your system from the
[latest release](https://github.com/francislavoie/zapper/releases/latest):

- **Debian and Ubuntu:** `sudo apt install ./zapper_<version>_amd64.deb`
- **Fedora:** `sudo dnf install ./zapper-<version>-1.x86_64.rpm`
- **Arch Linux:** `sudo pacman -U ./zapper-<version>-1-x86_64.pkg.tar.zst`
- **Windows:** `zapper_<version>_x64-setup.exe` installs it for you alone, no administrator
  needed (in `%LOCALAPPDATA%\zapper`); `zapper_<version>_x64_en-US.msi` installs it for everyone
  on the computer (in `Program Files`, asking for an administrator). Either carries its own
  libmpv and FFmpeg, and needs WebView2, which Windows 11 has and Windows 10 has had since 2021.

  The installers aren't signed yet, so the first run of one gets a SmartScreen warning ("Windows
  protected your PC"): choose **More info**, then **Run anyway**. Check the file against
  `SHA256SUMS` first (`Get-FileHash .\zapper_<version>_x64-setup.exe` in PowerShell) if you'd
  like to know it's the one the release built.

`SHA256SUMS` in each release lists the files' checksums. After that, zapper updates itself (see
[Updates](#updates)).

## Features

### Sources

- Xtream Codes: server URL, username and password. Live channels, categories, guide, and
  catch-up on channels that have an archive.
- M3U playlists from a URL or a file, with one or more XMLTV guides. Catch-up from the playlist's
  `catchup` attributes.
- HDHomeRun tuners by address. Channels from the tuner's lineup, guide from SiliconDust or your
  own XMLTV; subchannels shown as `5.1`.
- Tvheadend servers, with or without a login. Categories from its channel tags, guide from its
  XMLTV export.
- Settings, favourites and category choices can be exported to a file and imported again.

### Browsing

- The channel list and guide are stored locally in SQLite and refreshed in the background.
- One list for Live TV. The rail on the left has Favourites, Recently watched, Recordings and
  All channels, then your pinned categories, then the rest. Each channel row shows the
  programme on now and the one next; where channels from many categories mix (All channels,
  Favourites, Recently watched) the row also names its category.
- Channel search from the filter box, `/` or the command palette, with results grouped by
  category. `a|b` matches either term.
- Programme search: finds airings in the guide that are on now, coming up, or still available on
  catch-up. Opening one tunes in, jumps the guide to it, or plays it from catch-up.
- List and guide views (`G`) for any rail item, Favourites and Recently watched included. The guide scrolls through time with Shift+wheel, its scrollbar, or
  an hour at a time with `←`/`→`; `Home` returns to now.
- Categories can be pinned, hidden, and sorted by the provider's order, name or size.
- Recently watched: the last 20 channels you watched for at least 30 seconds, newest first.
- Recordings sits in the rail too, with its free disk space and, for the selected recording,
  its actions beside the picture.
- Each channel's last measured resolution, frame rate and codec is shown in the list; channels
  that failed to load are dimmed, with the reason.
- The playing channel stays in a docked picture with its own controls while you browse.
- Command palette (`Ctrl+P`; `⌘P` on a Mac) with every action. Shortcuts are listed in
  Settings › Keyboard, with ⌘ for Ctrl on a Mac.

### Playback

![The player filling the window: the channel name and programme at the top left, and along the bottom the transport bar with the scrollback timeline, pause, stop, volume, skip back and forward, and the Live marker](docs/images/screenshot-player.webp)

- libmpv with hardware decoding (VA-API on Linux) and HDR tone-mapping.
- Live rewind buffer, sized in megabytes; Settings shows roughly how many minutes it holds.
  Skip 5 seconds back or forward, `L` to jump to live.
- Catch-up on channels whose source has an archive.
- `P` switches to the previous channel; typing a channel number tunes it.
- Volume up to 200%, on a perceptual scale, remembered between launches.
- A compressor that evens out loud and quiet audio.
- Right-click menu on the picture: audio track, subtitles, aspect ratio, deinterlacing, sleep
  timer.
- Subtitle font, size, colour and backdrop.
- Mouse: wheel for volume, double-click for fullscreen, double-click the edges to skip.
  Theater mode (`T`).
- Stats (`S`): bitrate, buffer, resolution, codec, dropped frames and stalls.
- When a provider ends or refuses a stream (for example because another device took the
  connection), zapper says so and doesn't reconnect on its own.
- Optional media keys and desktop media controls (MPRIS on Linux), off by default.

### Other

- Restores the last channel, window size and volume on launch.
- Interface zoom (`Ctrl` `+`/`−`/`0`, `⌘` on a Mac); 12- or 24-hour clock.
- Updates itself from GitHub releases, verified against the release's checksums.
- Credentials are stored in your config directory, readable only by you, and kept out of logs.
- A log in the data directory (`zapper.log`, and `zapper-server.log` for the recording service),
  kept to a few megabytes, for when a sync or a recording went wrong.
- No telemetry. Apart from your sources, the only request zapper makes is the daily update
  check, which can be turned off.
- MIT licensed.

### Recording

- **Record from the guide:** right-click a programme. A background service, zapper-server, wakes
  to record it with zapper closed, copying the stream into a Matroska file named the way Jellyfin
  and Plex expect (or by channel, by date, or a pattern of your own), with an optional wall-clock
  subtitle track, named by season and episode when the guide numbers them. It switches to a
  backup channel if the channel's feed fails (picked for you, or chosen in the guide), and never
  takes a connection someone is watching on. Back-to-back programmes on one channel share one
  connection, so both are recorded whole on an account of one stream, and the Record menu warns
  when overlaps on other channels need more streams than the account has. Jellyfin can play a
  recording while it records, through a `.strm` file pointing at the service. On Linux and
  Windows; see [docs/recording.md](docs/recording.md).
- **Record what you're watching:** press R, or Record in the picture's menu. On an account of one
  stream the player moves onto the recording service's connection, so the picture and the
  recording share it, and what the player had already buffered of the programme becomes the
  recording's first part.
- **In a browser:** with network access turned on, the recording service serves zapper's own
  interface to other devices on your network, behind a password: browse the channels and the
  guide, search, schedule or cancel recordings, and watch live TV from a phone or another
  computer. The service streams the channel itself (the provider's address never reaches the
  browser), shares the connection with a recording of the same channel, and converts sound a
  browser can't play to AAC.

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

- **Linux (`.deb`, `.rpm`, Arch):** through `pkexec`, so your desktop asks for your password.
  The package is copied somewhere only root can write and checked again before apt, dnf,
  zypper, rpm or pacman sees it. If you installed zapper through an AUR helper, update it there
  instead.
- **Windows:** runs the new setup or MSI (the MSI asks for an administrator), after checking it
  again, and quits so it can replace zapper.
- **Restart** then starts the new version.

A build from source only links to the release page. Settings › Updates turns the automatic check
off.

## Building from source

Requirements: a stable Rust toolchain, Node.js 26 (see `.nvmrc`) and npm,
[`just`](https://github.com/casey/just), and the WebKitGTK development packages. On Debian and
Ubuntu:

```sh
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev \
  libjavascriptcoregtk-4.1-dev librsvg2-dev libxdo-dev libayatana-appindicator3-dev \
  libepoxy-dev pkg-config
```

libmpv isn't one of them: zapper builds against, and its packages carry, its own LGPL build of
libmpv and FFmpeg, which `just fetch-libmpv` downloads into `src-tauri/lib/`
([docs/licensing.md](docs/licensing.md#mpv-and-ffmpeg)). Without it a development build falls
back to the system's (`libmpv-dev`).

Packaging and the release-script checks also want `jq`, `dpkg-dev`, `zstd` (the Arch package)
and `desktop-file-utils`;
the license notices want [`cargo-about`](https://github.com/EmbarkStudios/cargo-about) 0.9.2 and
Python 3; the test streams in `tools/` want Python 3 and `ffmpeg`.

Then:

```sh
npm ci
just fetch-libmpv  # the bundled libmpv, into src-tauri/lib/
just dev           # run a development build
just test          # Rust, interface and release-script checks
just package       # build the .deb and .rpm
just licenses      # regenerate the third-party license notices
```

[docs/windows.md](docs/windows.md) covers building on Windows, [docs/macos.md](docs/macos.md)
on macOS, and [docs/releasing.md](docs/releasing.md) cutting a release.

## Documentation

- [docs/recording.md](docs/recording.md): the recording service, its settings and its API
- [docs/releasing.md](docs/releasing.md): versions, packages, releases and the updater
- [docs/windows.md](docs/windows.md): building and running on Windows
- [docs/macos.md](docs/macos.md): building on macOS, what CI checks there, and what still needs a
  real Mac
- [docs/licensing.md](docs/licensing.md): how third-party licenses are collected and shipped
- [tools/README.md](tools/README.md): test fixtures and development tools

## License

[MIT](LICENSE). zapper's packages carry libmpv and, inside it, FFmpeg, built without their GPL
parts and so under the LGPL; see [docs/licensing.md](docs/licensing.md) for what is in it and
where its source is.

zapper is a player. It doesn't provide any channels or content: bring your own tuner, server or
playlist, and use it with content you're entitled to watch.
