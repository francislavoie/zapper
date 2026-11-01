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

- Live TV from an Xtream Codes panel: the whole catalogue mirrored locally, so browsing and
  search never wait on the network.
- Playback through mpv: HDR tone-mapping, hardware decoding, live rewind, and an audio
  compressor for loud adverts.
- A programme guide, catch-up on channels that archive, and favourites earned from what you
  actually watch.
- Updates: zapper checks for a new release once a day (Settings → Updates turns it off), and
  installs it from the menu's Check for updates, after checking it against the release's
  published checksums.
