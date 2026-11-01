# Changelog

What changed in each release of zapper. `just release <version>` turns the Unreleased section
into the release's section, and the release workflow publishes that section as the release's
notes, which the update dialog shows (docs/releasing.md). Write entries for the person
installing it, not for the code.

The format is [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

The first release.

### Added

- Four kinds of source: an Xtream Codes server, an M3U playlist with an XMLTV guide, an
  HDHomeRun tuner, or a Tvheadend server. The whole catalogue is mirrored locally, so browsing
  and search never wait on the network.
- Playback through libmpv: hardware decoding, HDR tone-mapping, a rewind buffer for live TV, and
  an audio compressor for loud adverts.
- A programme guide, catch-up on channels that archive, favourites, and categories that can be
  pinned, sorted or hidden.
- A picture that keeps playing in a corner while you browse, with its own transport controls.
- Keyboard control throughout, a command palette, and a stats overlay for drops and stalls.
- Updates: zapper checks for a new release once a day (Settings → Updates turns it off), and
  installs it from the menu's Check for updates, after checking it against the release's
  published checksums.
- Settings → About lists the third-party licenses zapper ships under.
- When a stream stops by itself, the picture says why: your provider ended it (often because
  another device is using your connection), refused it, sent something other than video, or
  could not be reached. Retry tunes it again; nothing reconnects without you asking, so a
  second device is never knocked off in return.
