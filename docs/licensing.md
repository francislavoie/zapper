# Licensing and third-party notices

> **This line is the rewrite:** it ships `zapper-server` (a static Linux binary and a Windows zip).
> The 0.4.x desktop app's packages (`.deb`, `.rpm`, Arch, the Windows installers, the macOS
> `.dmg`) and how they carried these notices are on the
> [`main-tauri`](https://github.com/francislavoie/zapper/tree/main-tauri) branch.

zapper is MIT licensed (`LICENSE`, © 2026 Francis Lavoie). Its packages also contain other
people's code: the Rust crates the binary links, the npm packages bundled into the interface, and
libmpv, zapper's own LGPL build of mpv and FFmpeg (below). Their licenses require their notices
to ship with the binary, so every release carries them, and **Settings > About > Open source
licenses** shows them in the interface.

## What ships, and where

| File | Contents |
| --- | --- |
| `LICENSE` | zapper's own license |
| `THIRD-PARTY-NOTICES-rust.txt` | Every crate zapper links, grouped by license, then where the LGPL crates' source is |
| `THIRD-PARTY-NOTICES-ui.txt` | Every npm package in the interface bundle, grouped by license, then any notices they ship |
| `MPV-FFMPEG-NOTICE.txt` | mpv and FFmpeg: the bundled library, its license, where its source is and how to replace it (`packaging/licenses/`) |
| `THIRD-PARTY-NOTICES-libmpv.txt` | What is built into that library, version and license each, with every license's text (the LGPL's among them), from the libmpv archive |

- The Windows zip has them in `licenses\` beside `zapper-server.exe`, as `.txt` (the release
  workflow, docs/releasing.md).
- The static `zapper-server` for Linux, one file with nothing beside it, carries them inside:
  `zapper-server --licenses` prints them ([The static zapper-server](#the-static-zapper-server)).
- In the interface, the UI build copies them all into `dist/licenses/` with an `index.json` listing
  them, which the About pane reads (`src/components/Licenses.tsx`, a lazy chunk; the labels and
  order are `src/logic/licenses.ts`). Under `vite dev`, or a build made before the notices
  existed, the pane says they aren't in this build.

Each notices file lists every package under the license zapper uses it under (for "MIT OR
Apache-2.0", the one earliest in the allow-list, below) with the copyright lines from its license
files. The full text of each license follows its list once; texts that differ in more than their
copyright lines are printed separately, each with the packages that use it.

## How they're generated

`just licenses` generates everything; the release workflow runs the same steps around its build,
so the notices can't go stale.

1. **`scripts/licenses.sh`** writes `target/licenses/`: `cargo about generate`
   (cargo-about 0.9.2) produces JSON for zapper's graph on this platform
   (`x86_64-unknown-linux-gnu`; Windows' in Git for Windows' bash; this Mac's on macOS), without
   build-time, dev-only or harness crates, and `scripts/notices.py` turns it into
   `THIRD-PARTY-NOTICES-rust.txt`. With `--if-changed` it does nothing when `Cargo.lock`,
   `Cargo.toml`, `about.toml`, `LICENSE` and the scripts are as they were for the last run (key
   in `target/licenses.key`); `ZAPPER_REBUILD=1` regenerates. Install cargo-about once:
   `cargo install cargo-about --version 0.9.2 --locked --features cli`. The release workflow
   installs it with `taiki-e/install-action`.
2. **The UI build** (`npm run build`) writes
   `dist/licenses/THIRD-PARTY-NOTICES-ui.txt` through the Vite plugin in `tools/licenses.mjs`. It
   works from the modules actually in the bundle, so devDependencies (Vite, TypeScript) and
   tree-shaken code never appear and nothing bundled is missed; the bundler's own
   helpers (Vite's preload helper, Rolldown's runtime) are credited to their packages. It then
   copies in `LICENSE`, `packaging/licenses/MPV-FFMPEG-NOTICE.txt`, the Rust notices when they
   exist, and libmpv's `THIRD-PARTY-NOTICES-libmpv.txt` from the archive `just fetch-libmpv`
   unpacked into `lib/<platform>/` (below), when it has.
3. **libmpv's notices** are made with the library itself, by `scripts/build-libmpv.sh`, from the
   license files in each source it builds; the builds take them from the fetched archive.

`src/__checks__/licenses.test.mjs` holds the pieces together: the allow-list has no copyleft
license, the build refuses a package outside it, every file the build can write has a label in
the About list, and zapper says MIT everywhere it says what it is.

## The license allow-list

`accepted` in `about.toml` is the list of licenses zapper ships code under: MIT, Apache-2.0,
BSD-2-Clause, BSD-3-Clause, ISC, 0BSD, Zlib, MPL-2.0, EPL-2.0, Unicode, OFL-1.1, CC0-1.0,
BSL-1.0, Unlicense and CDLA-Permissive-2.0. The UI plugin reads the same list. A
crate or package under any other license, or with no recognisable license, fails the build with
its name, so a copyleft dependency is never added unnoticed. Review it, then drop the dependency
or record the decision: for a crate, in `about.toml` (for that crate alone, as below); for an npm
package whose `package.json` declares no license, in `tools/license-exceptions.json`.

- **MPL-2.0** (nucleo, the search, and any other crate under it) is
  file-level copyleft: it binds the covered files, not the MIT code beside them. They ship
  unmodified, their text is in the notices, and their source is on crates.io.

### The libmpv bindings: LGPL-2.1

`libmpv2` and `libmpv2-sys` (the Rust bindings to libmpv) are LGPL-2.1, accepted for those two
crates alone (`[libmpv2]` and `[libmpv2-sys]` in `about.toml`), never in general. They are
compiled into zapper's binary. The LGPL allows that as long as whoever gets the binary can relink
it with a modified copy of them, which zapper's published source (MIT) allows, and their source is
on crates.io: `THIRD-PARTY-NOTICES-rust.txt` ends with both addresses.

## mpv and FFmpeg

zapper uses libmpv and FFmpeg's libraries built by zapper's own
`scripts/build-libmpv.sh` with nothing under the GPL: mpv `-Dgpl=false`, FFmpeg without
`--enable-gpl`, `--enable-version3` or `--enable-nonfree`. They are therefore LGPL-2.1-or-later.
The player (zapper-mpv) plays through libmpv, which uses FFmpeg, and decodes a title's
seek-bar previews through FFmpeg's libavformat, libavcodec, libswscale and libavutil directly
(`crates/zapper-mpv/src/thumbframe.c`); the recording service, zapper-server, remuxes through
FFmpeg's libavformat directly (docs/recording.md). Where they ship as shared libraries they are
separate from the program, and the user can replace them with their own builds
(`packaging/licenses/MPV-FFMPEG-NOTICE.txt` says so, and where):

| Platform | Libraries | Installed as |
| --- | --- | --- |
| Windows (the zapper-server zip) | `libmpv-2.dll`, `avcodec-62.dll`, `avformat-62.dll` and the rest | beside `zapper-server.exe` |
| Linux, macOS (development builds) | `libmpv.so.2` / `libmpv.2.dylib`, FFmpeg's | copied beside cargo's output by `build.rs`, found through an rpath |

The static zapper-server for Linux links its own FFmpeg instead
([below](#the-static-zapper-server)).

The names carry FFmpeg's major versions: moving FFmpeg to a new major changes them (the
archive's `BUILDINFO.txt` lists its `runtime`).

### What is in it

Every other dependency is built from a pinned source as a static library and linked into the
library that needs it (dav1d into libavcodec; libplacebo, libass and the rest into libmpv),
with every symbol but that library's API hidden, so nothing in them can clash with a same-named
library already in the process (GTK's FreeType). FFmpeg's libraries keep their usual sonames;
loaded first, from `/usr/lib/zapper`, they are also what anything else in the process that asks
for the same sonames gets.

| Component | License | Platforms |
| --- | --- | --- |
| mpv 0.41.0 | LGPL-2.1-or-later (`-Dgpl=false`) | all |
| FFmpeg 8.1.3 | LGPL-2.1-or-later | all |
| libplacebo 7.360.1, with glad's generated GL loader and the Vulkan headers | LGPL-2.1-or-later; glad MIT; Vulkan-Headers Apache-2.0 or MIT | all |
| dav1d 1.5.4 | BSD-2-Clause | all |
| libass 0.17.5 | ISC | all |
| libdisplay-info 0.3.0 | MIT | Linux |
| FreeType 2.14.3, HarfBuzz 14.6.0, FriBidi 1.0.18 | FTL, MIT, LGPL-2.1-or-later | Windows, macOS |
| zlib 1.3.1 | Zlib | Windows |
| shaderc 2026.3, glslang, SPIRV-Tools, SPIRV-Headers, SPIRV-Cross | Apache-2.0, BSD-3-Clause and others, Apache-2.0, MIT, Apache-2.0 | Windows |

`THIRD-PARTY-NOTICES-libmpv.txt` (in the archive, then in every release that carries it) lists them with each
license's full text. The versions and SHA-256s are `packaging/libmpv/sources.txt`'s; the
sources are built as they are, but for one line: on Windows, libass's `meson.build` is told not
to look for iconv, which it would otherwise take from whatever the build machine has.

What the configuration includes, besides the software decoders: VA-API on Linux (interop on
Wayland, copy-back through a DRM render node elsewhere), D3D11VA and DXVA2 on Windows,
VideoToolbox on macOS; HTTPS through OpenSSL 3 (the system's, Apache-2.0) on Linux, SChannel on
Windows and Secure Transport on macOS; the decoders and demuxers for live TV and VOD (MPEG-TS,
HLS, MP4, Matroska, AVI; H.264, HEVC, MPEG-2, AV1, AAC, AC-3/E-AC-3, MP2/MP3, Opus, DTS; DVB,
DVD, PGS and CEA-608 subtitles) and FFmpeg's whole LGPL filter set, the compressor's
`speechnorm` and `alimiter` included (FFmpeg's configure marks its GPL-only filters as needing
`gpl`; neither is one, both sources are LGPL-2.1-or-later, and the build fails without them).
The encoders are PNG and MJPEG (mpv's screenshots) and FFmpeg's own AAC encoder (LGPL, not the
non-free libfdk-aac), with which zapper-server turns sound a browser can't play into AAC while a
browser watches (docs/recording.md, "Watching in the browser"); the build fails without it. A
bundle from before it was added (`libmpv-0.41.0-ffmpeg-8.1.3-r1`) has none, and zapper-server
then plays such channels in a browser without their sound; `-r2`, the same versions rebuilt, is
the first with it.

What it leaves out, because it is GPL:

- **DVB teletext.** FFmpeg decodes teletext only through libzvbi, and libzvbi's teletext core
  compiles in files that are GPL-2.0-only (`src/packet-830.c`, `src/pdc.c`). DVB bitmap
  subtitles (`dvbsub`) and CEA-608 captions are FFmpeg's own and are in.
- **mpv's X11 code** (`-Dgpl=false` removes it). zapper draws through the render API, so X11
  sessions still play; what they lose is VA-API interop, and fall back to VA-API copy-back
  through the render node.

On Linux the library also takes a few ordinary system libraries: VA-API (libva, libva-drm,
libva-wayland), libdrm, EGL, Wayland client, cursor and EGL, xkbcommon, ALSA, PulseAudio,
PipeWire, OpenSSL 3, FreeType, HarfBuzz, FriBidi, Fontconfig, zlib and libstdc++.
`scripts/build-libmpv.sh` fails if the library needs anything else. On macOS the libraries take
only what macOS itself has (its frameworks, `libSystem`, `libobjc`, `libz`, `libiconv`, and the
Swift runtime in `/usr/lib/swift` for mpv's Swift parts); the script fails if one links anything
outside `/usr/lib` and `/System/Library` besides the others, and drops the build machine's
toolchain from their rpaths.

### Where the source is

The LGPL asks that whoever receives the library can get its source. Each libmpv build is a
GitHub *pre-release* of this repository, tagged `libmpv-<mpv>-ffmpeg-<FFmpeg>-r<n>`, carrying the
archives and every source tarball they were built from (exactly the files in
`packaging/libmpv/sources.txt`, checked against its SHA-256s), with `SHA256SUMS` and build
provenance attestations. `THIRD-PARTY-NOTICES-libmpv.txt` names that release's address. The tag
is not a version and the release is a pre-release, so the updater never offers it
(`crates/zapper-core/src/updates/release.rs`).

### How builds get it

`packaging/libmpv/manifest.json` names the release, and each platform's archive with its
SHA-256. `just fetch-libmpv` (`scripts/fetch-libmpv.sh`; `tools/fetch-libmpv.ps1` on Windows,
which also makes the import libraries MSVC links with, `mpv.lib`, `avformat.lib` and the rest,
from the archive's export lists) downloads this platform's, checks it and unpacks it into
`lib/<platform>/`: `runtime/` (the libraries that ship), `link/` (what a link needs),
`include/` (mpv's and FFmpeg's headers). zapper-mpv's and zapper-server's build scripts (through
`packaging/libmpv/bundle.rs`) then put `link/` first on the
link path and set the RUNPATH, so libmpv2-sys's `-lmpv` and the remuxer's and previews' `-lavformat` resolve
to the bundle, compiles the remuxer and the previews against `include/`, and copies `runtime/` beside cargo's
output; the release workflow takes the libraries and their notices from there, and fetches it
first. A debug build with nothing fetched links the
system's libmpv and FFmpeg instead, with a warning; a release build refuses
(`ZAPPER_SYSTEM_LIBMPV=1` allows it, for a distribution's own package). CI uses Ubuntu's libmpv
until the manifest names a published build.

### Updating libmpv

1. Move the versions in `packaging/libmpv/sources.txt` (version, URL and SHA-256 together;
   `sha256sum` the downloaded tarball, and check FFmpeg's signature).
2. Build Linux locally to try it: `just build-libmpv` (four jobs; about twenty minutes), then
   `just fetch-libmpv --archive target-libmpv/out/libmpv-linux-x86_64.tar.xz`, `just test`, the
   playback harnesses (`tools/check_all.sh --host`) and the recording one
   (`recording_check`, docs/recording.md). Windows' can be built natively in an MSYS2 MINGW64
   shell (`scripts/build-libmpv.sh windows-x86_64 …`) and tried with
   `tools\fetch-libmpv.ps1 -Archive`.
3. Run the **libmpv** workflow (`.github/workflows/libmpv.yml`, Run workflow) with the next
   revision for those versions. It builds Linux and Windows on Ubuntu and, with **macOS**
   ticked, macOS on the Mac runners, and publishes the pre-release. (The macOS workflow,
   `macos.yml`, builds macOS's from the same script on any pull request that changes it, so a
   change that breaks it shows there first.)
4. Commit that release's `manifest.json` as `packaging/libmpv/manifest.json` (or regenerate it:
   `python3 scripts/libmpv-manifest.py write <tag> francislavoie/zapper SHA256SUMS
   packaging/libmpv/manifest.json` from its `SHA256SUMS`). The next build fetches it.

A published libmpv release must stay: what was built from it points at it for its source.

## The static zapper-server

`zapper-server-<version>-linux-x86_64` (docs/recording.md, "Without the app") is linked
statically, so what is in it travels in it:

- **FFmpeg** (LGPL 2.1 or later): libavformat, libavcodec, libswresample and libavutil, built by
  `scripts/build-ffmpeg-static.sh` from the same FFmpeg tarball as zapper's libmpv
  (`packaging/libmpv/sources.txt`, checked against its SHA-256), configured with nothing GPL or
  nonfree (the script fails unless `configure` says "LGPL version 2.1 or later") and only what the
  service uses.
- **musl**, the C library (MIT): its `COPYRIGHT`, from the musl 1.2.5 source, is
  `packaging/licenses/MUSL-COPYRIGHT.txt`.
- The Rust crates and the interface's npm packages, as everywhere.

The notices go in at build time (`ZAPPER_SERVER_NOTICES`, `build.rs`): zapper's `LICENSE`,
FFmpeg's notice (what was built, the exact `configure` line, where the source is, how to relink)
followed by FFmpeg's `COPYING.LGPLv2.1`, musl's `COPYRIGHT`, `THIRD-PARTY-NOTICES-rust.txt` and
`THIRD-PARTY-NOTICES-ui.txt`. `zapper-server --licenses` prints them; a release build for musl
refuses to build without them, and `scripts/check-server-binary.sh` checks they are there.

Linking the LGPL's libraries statically is allowed when whoever has the binary can relink it
with a modified version of them (LGPL 2.1, section 6). zapper's whole source, including the
scripts that build this binary, is public under the MIT license, so anyone can: build their
FFmpeg as static libraries (`scripts/build-ffmpeg-static.sh` shows how), name its folder in
`ZAPPER_FFMPEG_STATIC`, and build zapper-server for `x86_64-unknown-linux-musl` as
`scripts/build-server-static.sh` does. The FFmpeg source it was built from is published as for
libmpv: the exact tarball is in the libmpv release `packaging/libmpv/manifest.json` names (the
same file, the same SHA-256), and FFmpeg's notice in the binary says so.

## Documentation images

The picture in the README's screenshots, `docs/images/broadcast-still.jpg`, is cropped from
[Championship Soccer Stadium SW View](https://commons.wikimedia.org/wiki/File:Championship-Soccer-Stadium-SW-View-2024-08-03.jpg)
by Haruhi8, released under [CC0 1.0](https://creativecommons.org/publicdomain/zero/1.0/) (public
domain dedication); the advertising boards are blurred. It is a documentation and development
asset only: the mock shows it in place of video under `vite dev`, and it is in no package or
production build.

## Updating

- **New dependency, new license:** the build names it. Check its terms, then extend the
  allow-list or record an exception as above.
- **A new notice file:** add it to `noticeFiles()` in `tools/licenses.mjs`, give it a label in
  `src/logic/licenses.ts`, and add it to what the release workflow packs; `licenses.test.mjs`
  fails until the first two agree.
