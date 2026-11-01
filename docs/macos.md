# zapper on macOS

zapper builds, packages and plays on macOS on Apple Silicon (arm64), as checked on GitHub's
macOS runners by [.github/workflows/macos.yml](../.github/workflows/macos.yml). It has not been
tried on a real Mac yet and is not released: there is no published macOS libmpv in
`packaging/libmpv/manifest.json`, and release.yml builds macOS only when asked to
([releasing.md](releasing.md#windows-and-macos)). Intel Macs are not built.

## How video is drawn

The same arrangement as on Linux, in AppKit's terms
([src-tauri/src/surface/macos/mod.rs](../src-tauri/src/surface/macos/mod.rs)):

- a view of zapper's own sits beneath the WKWebView in the window, and holds a view whose frame
  is the rect the page asks for (the same `VideoRect` as on the other platforms);
- that view hosts a `CAOpenGLLayer`. Core Animation calls it to draw, on the main thread, with
  its OpenGL context current, and mpv renders into it through the OpenGL render API
  (`vo=libmpv`); mpv's update callback is what asks for the draw;
- the webview is transparent and the window black, so the video shows wherever the page has
  no background of its own, and every click lands on the page.

mpv has no `wid` embedding on macOS, which is why it is not the Windows arrangement. Decoding is
VideoToolbox's (`hwdec=auto-safe`), handed to OpenGL without a copy. Scrollback thumbnails are
blitted down from the frame just drawn, as on Linux.

## Building

You need Xcode (its Swift compiler builds part of libmpv), Rust, Node.js, and from Homebrew a
`bash` newer than macOS's 3.2 (first on `PATH`), `meson`, `ninja` and `pkgconf` for libmpv. Until
a macOS libmpv is published, build it here (a few minutes on an M1) and unpack that:

```sh
brew install bash meson ninja pkgconf
just build-libmpv macos-arm64
just fetch-libmpv --archive target-libmpv/out/libmpv-macos-arm64.tar.xz
npm ci
just dev          # a development build
just test         # Rust, interface and release-script checks
```

The app bundle and the `.dmg` (into `src-tauri/target/release/bundle/`), with the license
notices (`cargo-about` 0.9.2), and the bundle's checks:

```sh
scripts/licenses.sh
npx tauri build --config packaging/macos/dmg.conf.json
scripts/check-macos-app.sh src-tauri/target/release/bundle/macos/zapper.app
```

`just package-macos` does the same once the manifest names a published macOS libmpv. The bundle
carries libmpv and FFmpeg in `Contents/Frameworks` ([licensing.md](licensing.md#mpv-and-ffmpeg)),
is ad-hoc signed and not notarized, and is built for macOS 11 or later (only macOS 15, the
runners', has run it).

## What CI checks

On every pull request, `macos.yml`:

- builds the LGPL libmpv for macos-arm64 with `scripts/build-libmpv.sh`, cached until the script
  or `packaging/libmpv/sources.txt` changes;
- runs Rust's unit tests and clippy (the harnesses too), builds zapper-server and the interface,
  and runs the interface's checks;
- builds `zapper.app` and the `.dmg`, and checks the bundle with `scripts/check-macos-app.sh`:
  every binary links only macOS's own libraries and the bundled dylibs, nothing in it is GPL, the
  notices and install marker are there, and the signature verifies. The `.dmg` is kept as an
  artifact;
- plays a local test stream (`tools/fake_stream.py`, no sound) through `surface_check`, which
  fails unless the layer draws, mpv paints, the centre of the picture is not black, the docked
  rect moves the view and thumbnails are taken; then plays one through the bundled app itself.
  Screenshots of both are kept as the `macos-screenshots` artifact.

The runners have no GPU: OpenGL there is Apple's software renderer, at a few frames a second.
CI shows the pieces are wired together correctly, and says nothing about how smoothly video
plays.

## What needs a real Mac

- **Playback on a GPU.** Smoothness, frame pacing, CPU use, and the hardware OpenGL 3.2 pixel
  format the layer asks for first (the runners only ever reach the software fallback). Retina
  sharpness, and a window moved between screens of different scales.
- **Sound.** Every run on CI has `ao=null`; Core Audio output has never been heard.
- **The window.** It is undecorated (`decorations: false`) and draws the same title bar and
  window buttons as on Linux and Windows: no traffic lights, and moving, full screen and Mission
  Control are untried. Shortcuts are named with Ctrl, where a Mac user expects ⌘.
- **First launch.** Ad-hoc signed and not notarized, so Gatekeeper asks before it opens
  (Control-click, Open), and quarantine applies to a downloaded `.dmg`.
- **Updates.** The updater's `.dmg` install (mount, swap the bundle) has tests but has never run.
- **Media keys.** The desktop media controls (MPRIS, Windows' SMTC) have no macOS counterpart
  yet.
- **HDR.** The layer is SDR; HDR video is tone-mapped by mpv.
- **zapper-server** builds on macOS but is not in the bundle and has not been run there.
