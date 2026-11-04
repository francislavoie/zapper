# Changelog

What changed in each release of zapper. `just release <version>` turns the Unreleased section
into the release's section, and the release workflow publishes that section as the release's
notes, which the update dialog shows (docs/releasing.md). Write entries for the person
installing it, not for the code.

The format is [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Changed

- Live TV has a new layout. One rail on the left replaces the Categories and Favourites tabs:
  Favourites, Recently watched, Recordings and All channels, then your pinned categories and the
  rest. Choosing an item lists its channels, as a list or as the guide (`G`), for any of them.
- Channel rows have two lines: the name with the programme on now, and what is next. Where
  channels from several categories mix, the category is named too.
- Recordings is part of Live TV, not a window: pick it in the rail, or from the menu, the
  command palette or the title-bar badge. What you can do with a recording (open its folder,
  cancel, stop, delete, change its backup channel) is beside the picture.
- The title bar shows the catalogue's sync status beside Settings.
- The title bar shows Movies beside Live TV, and `M` opens it. It appears only for an account that
  has movies.

### Added

- Recently watched: the last 20 channels you watched for at least 30 seconds, in the rail.
- Recordings shows how much space is free on the recordings disk.
- Movies: a tab listing an Xtream account's or a playlist's movies by category, as a list or
  posters, sorted by recently added, name, year or rating and limited to a range of years, with
  favourites, watched, search, and a details sheet (plot, cast, pictures, movies like it).
- Movies play on demand, with a seek bar, the time played and left, and 10-second skips; the place
  is kept, a movie played past 92 % counts as watched, and one that stops can be played again from
  where it stopped.
- Movies in the browser through the recording service: played as they are when the browser can,
  otherwise converted on the way without re-encoding the picture. Text subtitles show there;
  subtitles stored as pictures (DVD, Blu-ray, DVB) can't, and the player's menu says so.

## [0.3.0] - 2026-11-02

### Added

- Recordings of a series are named by season and episode (`Show - S02E05 - …`) when the guide
  numbers its episodes, as XMLTV guides (Xtream servers', Tvheadend's, a playlist's) and
  SiliconDust's guide for HDHomeRun often do. The programme menu shows the number too.
- A recording's backup channel (the one it switches to if its own fails) can be chosen by hand:
  right-click a programme on another channel at the same time in the guide and choose *Use as
  backup for “…”*, or use *Change…* under the recording in Recordings, which lists the
  candidates best first. A change while recording applies the next time a feed fails.
- While the recording service records, the title bar says so beside Settings: a red dot and
  "Recording" (or "Recording 2"), with each recording in its tooltip — channel, programme, how
  long so far and until when, and whether it waits for a connection, is on its backup channel
  or has a conflict ahead. A click opens Recordings. In the app and in a browser.
- Watch live TV in the browser. A browser signed in to the recording service plays channels in the
  page, with the picture band, the player, pause and rewind (through the last couple of minutes),
  volume, the compressor, fullscreen and audio tracks. The service streams the channel itself, so
  the provider's address never reaches the browser; watching a channel that is being recorded
  shares the recording's connection; sound a browser can't play (MP2, AC-3) is converted to AAC,
  and a channel whose video it can't play (MPEG-2) says so instead of showing a black picture.
- On a full account the browser says why it can't play: a recording's connection is never taken,
  and "Watch here anyway" is offered only where no recording is at stake. A browser is never cut
  off for a recording either: from two minutes before a recording on another channel needs its
  connection, the page says so and offers to switch to that channel, which shares one connection.
- Linux: `zapper-server-<version>-linux-x86_64`, the recording service alone for a computer without
  a desktop (a home server, a NAS, a container), as one static file that runs on any x86-64
  Linux, with FFmpeg built in. `zapper-server --licenses` prints the licenses of what is in it;
  packaging/server/ has how to run it, a sample systemd unit and a sample Dockerfile.
- `zapper-server --import-account`, `--set-password` and `--network on|off` set up a service with
  no app beside it: the provider account from an `account.json`, the password other devices sign
  in with (read from standard input, never the command line), and network access.
- Windows: Settings → Recording → *Start at boot (Windows service)* runs the recording service as
  a Windows service that starts with the computer and records whether or not anyone is signed in.
  Windows asks once for an administrator's permission; it runs as an account of its own that may
  use only your zapper folders and recordings folder. Switching back to the sign-in task, updating
  and uninstalling ask nothing more.
- macOS: Settings → Recording → Install (and `zapper-server --install-service`) installs the
  recording service as a LaunchAgent, which starts it when you log in and again after a crash.
- Watch a recording in Jellyfin while it records: *Show recordings to other programs while they
  record* (Settings → Recording) has a *Jellyfin (.strm)* choice. While a recording records, a
  `[RECORDING] <name>.strm` beside it points Jellyfin at the recording service, which streams the
  recording from the start and keeps following it live until it ends; then the `.mkv` takes its
  place. Each `.strm` carries a key that opens that one recording's stream and nothing else, until
  it finishes. For a Jellyfin on another computer it needs network access on; the address in the
  files can be set by hand. See "Watching a recording in Jellyfin while it records" in
  docs/recording.md for what Jellyfin shows and doesn't.
- Record what you're watching, from the player: **R**, *Record* in the picture's right-click
  menu, or the *Record* button beside the picture (in the app and in a browser). Once it records,
  the same places say *Recording* and offer *Stop*.
- Recording what you watch works on an account of one connection. When the recording service
  runs on this computer, the player moves onto the service's connection to the channel and the
  recording shares it, instead of the recording waiting until you stop watching; the picture
  pauses for a moment and keeps its volume and tracks. A recording that starts later on the
  channel you're watching does the same. Without the service on this computer, Record says it
  can't rather than leaving the recording waiting.
- The part of the programme you've already watched is kept: what the player has buffered of it,
  from the programme's start or as far back as the buffer reaches, becomes the recording's first
  part, with a clock track reading when it aired. (Jellyfin following a recording through its
  `.strm` sees what the service receives; the buffer is in the finished file.)
- Tuning to a channel the recording service is recording shares its connection instead of
  opening a second one, so watching what records costs nothing on an account of one stream.

### Changed

- The backup channel picked when scheduling is the best of several: one in the same category
  first, then the one that has played better, then the lower number. When no channel shares
  the guide channel, one showing the same programme at the same time is used.

### Fixed

- In a browser, zapper's tab and bookmarks show its icon, and so does a phone's home screen.
- Stopping a recording no longer leaves its red dot up until the next look a while later: it
  turns grey at once ("Saving…") while the recording is saved into its file, and the list is
  followed every two seconds while anything records or saves. An update or a restart of the
  recording service waits for the saving to finish too.
- A provider that stops sending the guide or the playlist halfway without closing the connection
  no longer stalls the refresh for good: after a minute with nothing arriving (or half an hour
  in all) the refresh fails with that reason, keeps the guide it had, and tries again later. The
  provider's channel and category lists have a deadline too.
- Windows: installing the MSI after uninstalling the per-user setup puts zapper in
  `Program Files\zapper`, rather than in the setup's old folder, which the uninstall had left
  remembered in the registry.

## [0.2.1] - 2026-11-01

### Added

- Recordings of the same channel share one connection: back-to-back programmes on one channel
  are both recorded whole, padding included, even on an account of one stream (the second one
  used to lose its first minutes waiting for the first to let go). On another channel, when the
  account is full, the next programme's start takes the connection from the last one's padding
  after — never from a programme still on, nor from someone watching — and the recording that
  ended early says so in its notes. A recording waiting for a connection now takes it the moment
  one frees up.
- Conflicts are shown before recording: a programme's Record menu, and each row in Recordings,
  say when overlapping recordings on other channels need more streams than the account allows,
  and what will be lost ("Overlaps “The Film” on another channel: the last 5m will be cut").
- Settings → Recording → *Show recordings to other programs while they record* (off by default):
  the file being recorded is named `… (recording).ts` rather than hidden, so a media server such
  as Jellyfin can play it as it grows; the `.mkv` replaces it when the recording ends.

## [0.2.0] - 2026-11-01

### Added

- Windows: an installer for you alone (`zapper_<version>_x64-setup.exe`, no administrator needed)
  and an MSI for everyone on the computer, each with its own libmpv and FFmpeg and the recording
  service. They aren't signed yet, so Windows SmartScreen warns the first time: More info, then
  Run anyway. zapper updates itself from them like the Linux packages.
- Recording on Windows: Settings → Recording installs the recording service as a scheduled task
  that starts when you sign in, and removes it again. It records while you are signed in.
- The recording service no longer cuts a recording short when it stops: what it recorded so far
  is saved as a playable file, and it carries on in a new part if it starts again while the
  programme is on. After a package upgrade on Linux it restarts into the new version once
  nothing is recording. On Windows, updating asks first when a recording is in progress:
  install when it finishes, or now. Uninstalling zapper removes the service too.
- Network access for the recording service (Settings → Recording, off by default): other devices
  on your network can reach it over HTTPS with its own certificate, whose fingerprint Settings
  shows, after signing in with a password you set there. Each device is listed and can be
  revoked, and the port can be changed.
- zapper in a browser: with network access on, opening the recording service's address shows
  zapper's interface after signing in with that password. Browse the channels and the guide,
  search channels and programmes, and schedule, cancel and delete recordings; watching in the
  browser is not there yet. Each browser is listed with the other devices and can be signed out
  there or from the page itself.
- In the browser, Refresh data from the provider (Settings → Data, the command palette, the title
  bar's retry) refreshes the catalogue, with its progress in the title bar and the lists updated
  when it is done. Settings → Account is there too, for every account type: a saved password is
  never sent to the browser, and leaving it empty keeps it, as in the app. Over plain HTTP the page
  warns that a password typed there crosses the network unencrypted. The app on the same computer
  picks up an account or catalogue changed this way within a few seconds.
- zapper keeps a log file, `zapper.log` in its data folder (on Windows,
  `%LOCALAPPDATA%\zapper`), beside the recording service's `zapper-server.log`. Each is kept to a
  few megabytes, with the previous one as `.old.log`.

### Changed

- The picture beside the channel list can be made as large as the window allows, keeping enough
  room beside it for the channel's details and below it for the list.
- Settings → Account: Test connection checks what is in the form, saved or not, so an account can
  be tried before saving it.
- With no account set up, the title bar offers "Add an account" instead of "retry".

### Fixed

- An Xtream server's guide download cut off partway no longer replaces part of the guide with what
  had arrived: the guide stays as it was until a download completes, and the failure is logged.
- In the browser, double-clicking a channel says once, not three times, that watching there isn't
  available yet; and Settings → Keyboard lists only the shortcuts that do something there.
- After a sync — the first one, or Refresh data from the provider — the categories, channel list
  and guide now show the new data straight away instead of after a restart.
- Windows: a channel whose server can't be reached, or whose stream stalls, is now recognised as a
  network problem and reconnected like on Linux, instead of being reported as an unknown error
  or a dropped stream.

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
