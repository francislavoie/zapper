# Changelog

What changed in each release of zapper. `just release <version>` turns the Unreleased section
into the release's section, and the release workflow publishes that section as the release's
notes, which the update dialog shows (docs/releasing.md). Write entries for the person
installing it, not for the code.

The format is [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Network access for the recording service (Settings → Recording, off by default): other devices
  on your network can reach it over HTTPS with its own certificate, whose fingerprint Settings
  shows, after signing in with a password you set there. Each device is listed and can be
  revoked, and the port can be changed.

### Changed

- The picture beside the channel list can be made as large as the window allows, keeping enough
  room beside it for the channel's details and below it for the list.

## [0.1.0] - 2026-11-01

The first release.

### Added

- Sources: an Xtream Codes server, an M3U playlist with XMLTV guides, an HDHomeRun tuner, or a
  Tvheadend server. The channel list and guide are kept locally and refreshed in the background.
- Browsing: list and guide views, channel search grouped by category (`a|b` matches either),
  programme search across the guide, favourites, and categories that can be pinned, hidden and
  sorted. The playing channel stays in a corner while you browse.
- Playback through a bundled libmpv: hardware decoding, HDR tone-mapping, a rewind buffer for
  live TV, catch-up, a compressor, audio and subtitle track choice, aspect ratio and
  deinterlacing from the picture's right-click menu, and a sleep timer.
- `P` for the previous channel, typing a number to tune it, and a choice of how a channel opens:
  where the stream starts, right away then catching up to live, or waiting to start at live.
- When a stream stops, the picture says why. zapper reconnects on its own only after network
  failures and only when the account has a connection free; otherwise Retry, or Watch here
  anyway to take the connection from another device.
- Recording (Linux): record from the guide's or the search results' menu. Settings → Recording
  installs the recording service, which records with zapper closed, names files from a preset
  or your own pattern, can add a clock subtitle track, and falls back to another feed of the
  same channel if its own fails.
- Settings: a reset per setting, 12- or 24-hour clock, optional media keys and desktop media
  controls, and backup and restore of settings, categories and favourites.
- Each channel's measured resolution, frame rate and codec are remembered and shown; channels
  that failed to load are dimmed with the reason.
- Packages for Debian and Ubuntu (`.deb`), Fedora and openSUSE (`.rpm`), and Arch Linux
  (`.pkg.tar.zst`), with libmpv and FFmpeg built LGPL-only and bundled.
- Updates: zapper checks for a new release once a day (Settings → Updates turns it off) and
  installs it after checking it against the release's published checksums.
- Settings → About lists the third-party licenses zapper ships under.
