# Changelog

What changed in each release. `just release <version>` turns Unreleased into the release's
section, which becomes the release notes and the update dialog's text (docs/releasing.md).
Write for the person installing zapper: what they get, in a line or two.

The format is [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- **Continue watching** (`W`): pick up movies and shows where you left off, in the app and the browser. Opening one you left partway asks whether to resume or start over, and movies show a progress bar.
- **Settings › Online services**: a switch for OpenSubtitles with a line on what it sends, the
  languages to search in, and signing in or out of your OpenSubtitles account.
- **Movies**: a tab (`M`) for an Xtream account's or a playlist's movies. Browse by category as a
  list or posters, sort, filter by year, search, mark favourites and watched, and open a details
  sheet.
- **Shows**: a tab (`N`) for series. Seasons and episodes are in a pane beside the list, with
  watched marks per episode or for a whole show.
- Movies and episodes play with a seek bar and 10-second skips, resume where you stopped, and count
  as watched past 92 %.
- **Up next**: near an episode's end, a 10-second countdown into the next one, with Play now and
  Cancel. It keeps your audio and subtitle languages.
- Up next plays a next episode you had started from where you left it.
- Movies and episodes play in the browser through the recording service, converted on the fly when
  the browser can't play the file. Text subtitles included.
- Subtitles from OpenSubtitles for movies and episodes, downloaded with your own free OpenSubtitles
  account and kept with the title for next time.
- Off-air channels: when the provider sends a placeholder clip instead of the broadcast, zapper
  says so and suggests channels showing the same programmes. Recordings retry until the broadcast
  starts.
- Recently watched channels, in the rail.
- Recordings shows the free space on the recordings disk.
- Radio channels (sound, no picture) play.
- A subtitles button on the player, remembered across channels and restarts (`C`).
- **Find subtitles…** for movies and episodes, from OpenSubtitles.com, added to the player at once.
- The mouse's back and forward buttons move between screens, and sheets have a Back button.

### Changed

- Live TV has a new layout: one rail for Favourites, Recently watched, Recordings, All channels and
  your categories. Each item shows as a list or as the guide (`G`).
- Channel rows show what's on now and next.
- Recordings moved into Live TV instead of its own window.
- The title bar shows the sync status, and Movies and Shows when the account has them.
- A playlist's episodes (`S01E02`, `1x02`, `Season 1 Episode 2`, ranges like `S01E01-E02`) are
  listed under Shows instead of Live TV.
- Search boxes have dimmer hints, with `/` shown as a key.
- Search in Movies and Shows keeps the list's sort, and matches the start of words (`EN -`).
- The top band shows what you selected and what's playing side by side.
- Selecting a show opens its episodes in a pane beside the list.

### Fixed

- Play starts playing even after something was paused.
- Closed captions on wide video keep their place instead of drifting to the left.
- Choosing another subtitle track during a movie or episode no longer ends it.
- Rewinding into a provider's archive plays the moment you picked, in the provider's time zone,
  including right after an update.
- Live returns to live from a replayed programme.
- A replay older than the provider keeps says so and offers Watch live.
- The guide update no longer fails with "UNIQUE constraint failed: epg_fts_stale.id".
- Linux: quitting no longer triggers a crash report for `WebKitWebProcess`. Settings changed just
  before quitting are saved, and a playing movie resumes where it was.
- Shows and watched marks work even while the channel list fails to sync.
- Tooltips appear only on cut-off text, next to it; sheets open at the top and cover the rail's
  resize handle.

## [0.3.0] - 2026-11-02

### Added

- Recordings of a series are named by season and episode (`Show - S02E05 - …`) when the guide
  numbers episodes.
- Choose a recording's backup channel by hand, from the guide or from Recordings.
- The title bar shows what's recording.
- Watch live TV in the browser through the recording service, with rewind, volume, the compressor
  and audio tracks. The provider's address never reaches the browser.
- The browser explains a full account and never takes a recording's connection.
- Linux: `zapper-server`, the recording service alone as one static file for a headless machine,
  with a sample systemd unit and Dockerfile.
- `zapper-server --import-account`, `--set-password` and `--network on|off` to set up a service
  without the app.
- Windows: run the recording service as a Windows service that starts at boot.
- macOS: install the recording service as a LaunchAgent.
- Watch a recording in Jellyfin while it records, through a `.strm` file.
- Record what you're watching: **R**, the picture's menu, or the Record button.
- Recording what you watch works on a one-connection account: the player and the recording share
  the connection.
- The part of the programme you already watched is kept in the recording.
- Watching a channel that's being recorded shares the recording's connection.

### Changed

- The automatic backup channel prefers the same category, then the channel that has played better.

### Fixed

- The browser tab, bookmarks and phone home screens show zapper's icon.
- A stopped recording turns grey ("Saving…") at once instead of staying red.
- A provider that stalls mid-download no longer stalls the refresh for good.
- Windows: the MSI installs into `Program Files\zapper` after the per-user setup was uninstalled.

## [0.2.1] - 2026-11-01

### Added

- Recordings of the same channel share one connection, so back-to-back programmes both record whole.
- Conflicts are shown before recording, with what would be lost.
- Settings → Recording can expose a recording in progress to media servers such as Jellyfin.

## [0.2.0] - 2026-11-01

### Added

- Windows: a per-user installer and an MSI, with libmpv, FFmpeg and the recording service. They
  aren't signed yet, so SmartScreen warns the first time.
- Recording on Windows, through a scheduled task that starts when you sign in.
- Stopping the recording service no longer cuts a recording short; it resumes in a new part.
- Network access for the recording service over HTTPS, with a password and a list of devices.
- zapper in a browser: browse channels and the guide, search, and manage recordings.
- Refresh data and account settings from the browser.
- Log files: `zapper.log` and `zapper-server.log` in the data folder.

### Changed

- The picture beside the channel list can be resized larger.
- Test connection checks the form as typed, before saving.
- With no account, the title bar offers "Add an account".

### Fixed

- A guide download cut off partway no longer replaces part of the guide.
- After a sync, new data shows at once instead of after a restart.
- Windows: an unreachable or stalled channel reconnects as on Linux.

## [0.1.0] - 2026-11-01

The first release.

### Added

- Sources: Xtream Codes, M3U playlists with XMLTV guides, HDHomeRun and Tvheadend.
- List and guide views, channel and programme search, favourites, and pinned, hidden and sorted
  categories.
- Playback through a bundled libmpv: hardware decoding, HDR tone-mapping, a rewind buffer,
  catch-up, a compressor, track choice, aspect ratio, deinterlacing and a sleep timer.
- `P` for the previous channel, type a number to tune, and a choice of how channels start.
- A stopped stream says why; zapper reconnects only after network failures.
- Recording on Linux through a background service, with file naming presets and a backup feed.
- Settings with per-setting reset, clock format, media keys, and backup and restore.
- Each channel's resolution, frame rate and codec are remembered; failing channels are dimmed.
- Packages for Debian/Ubuntu, Fedora/openSUSE and Arch, with LGPL-only libmpv and FFmpeg.
- Daily update checks, verified against published checksums.
- Settings → About lists third-party licenses.
