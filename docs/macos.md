# zapper on macOS

> **This line is the rewrite:** it has no desktop app (the Tauri app, 0.4.x, is on the
> [`main-tauri`](https://github.com/francislavoie/zapper/tree/main-tauri) branch), so
> `.github/workflows/macos.yml` here builds libmpv for macos-arm64, runs the core and server tests
> and installs and removes `zapper-server` as a LaunchAgent; it builds no `.app` or `.dmg`. The
> sections below about the window, packaging and `scripts/check-macos-app.sh` describe the 0.4.x app
> and stay on `main-tauri`; what they say about libmpv and the recording service still applies.


zapper builds, packages and plays on macOS on Apple Silicon (arm64), as checked on GitHub's
macOS runners by [.github/workflows/macos.yml](../.github/workflows/macos.yml). It has not been
tried on a real Mac yet and is not released: there is no published macOS libmpv in
`packaging/libmpv/manifest.json`, and release.yml builds macOS only when asked to
([releasing.md](releasing.md#windows-and-macos)). Intel Macs are not built.

## How video is drawn

The same arrangement as on Linux, in AppKit's terms
([crates/zapper/src/surface/macos/mod.rs](../crates/zapper/src/surface/macos/mod.rs)):

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

## The window and the keyboard

On a Mac the window keeps the system's title bar, transparent and under zapper's own
(`titleBarStyle: "Overlay"` in `crates/zapper/tauri.macos.conf.json`; Linux and Windows stay
undecorated). The traffic lights are AppKit's, in the bar's top-left corner, so the bar draws
no window buttons there and starts the hamburger to their right; the rest of it drags the
window, and a double click on it does what System Settings › Desktop & Dock says
(`crates/zapper/src/chrome.rs`). Full screen is the system's, from the green button, View › Enter
Full Screen (⌃⌘F) or zapper's F, and zapper's bar is not drawn there.

Shortcuts use ⌘ where Linux and Windows use Ctrl, and are written the Mac way (⌘P, ⇧⌘F):
`src/logic/keys.ts` decides both, and with Control held a shortcut does nothing, so ⌃P and
⌃F stay the system's and the text fields'. The app menu is a Mac's: zapper (About, Settings…
⌘,, Services, Hide, Quit ⌘Q), Edit (Undo, Redo, Cut, Copy, Paste, Select All, which is what
makes ⌘C, ⌘V and ⌘A work in a text field), View (Enter Full Screen) and Window (Minimise, Zoom).
Quit closes the window, the same way out as its red button.

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

The app bundle and the `.dmg` (into `target/release/bundle/`), with the license
notices (`cargo-about` 0.9.2), and the bundle's checks:

```sh
scripts/licenses.sh
npx tauri build --config packaging/macos/dmg.conf.json
scripts/check-macos-app.sh target/release/bundle/macos/zapper.app
```

`just package-macos` does the same once the manifest names a published macOS libmpv. The bundle
carries libmpv and FFmpeg in `Contents/Frameworks` ([licensing.md](licensing.md#mpv-and-ffmpeg)),
is ad-hoc signed and not notarized, and is built for macOS 11 or later (only macOS 15, the
runners', has run it).

## What CI checks

`macos.yml` runs on the Mac runners every Monday and on Run workflow for main, and on a pull
request only when it can change what macOS gets: a change to the macOS code (`surface/`,
`chrome.rs`, `paths.rs`, the recording service's installer and `zapper_server.rs`, any Rust file
with `target_os = "macos"`), `packaging/macos/`, the
libmpv build, Cargo or Tauri config, or the workflow itself; or when the pull request carries the
`macos` label. A pull request builds the `.app` and `.dmg` only when macOS packaging or the libmpv
build changes. A small Linux job, *macOS needed?*, decides in seconds. When it runs, it:

- builds the LGPL libmpv for macos-arm64 with `scripts/build-libmpv.sh`, cached until the script
  or `packaging/libmpv/sources.txt` changes;
- runs Rust's unit tests and clippy (the harnesses too), builds zapper-server and the interface,
  and runs the interface's checks;
- installs the built zapper-server as a LaunchAgent (`zapper-server --install-service`, what
  Settings → Recording → Install does), on a scratch home and port, checks the plist with `plutil
  -lint`, that `launchctl print` says it runs and that it answers (`--busy` says idle), then
  removes it (`--remove-service`) and checks that launchd has let it go and that it stopped
  cleanly;
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
plays. With it, VideoToolbox's frames (handed to OpenGL as IOSurfaces) have also come out black
for a whole run in one run of several, which is likely the software renderer's doing but which
only a real Mac can settle. So the `surface_check` that gates decodes in software
(`ZAPPER_HWDEC=no`), and the app's own path, VideoToolbox into OpenGL, runs twice after it for
the record (the job summary says how each went) and in the bundled app's screenshot.

## What needs a real Mac

- **Playback on a GPU.** Smoothness, frame pacing, CPU use, and the hardware OpenGL 3.2 pixel
  format the layer asks for first (the runners only ever reach the software fallback).
  That VideoToolbox's frames reach the picture every time (on the runners one run in several
  stayed black). Retina sharpness, and a window moved between screens of different scales.
- **Sound.** Every run on CI has `ao=null`; Core Audio output has never been heard.
- **The window by hand.** Dragging it by the bar, the double click (whichever action System
  Settings names), the traffic lights, resizing from the edges, the green button's tiling
  menu, full screen with a pointer that reveals the menu bar, Mission Control. CI only sees
  screenshots of the bar and of full screen. On the runner, a window resized from code came
  back from full screen at its first size; whether a window resized by hand keeps its size
  through full screen wants checking.
- **The keyboard by hand.** ⌘ for every shortcut and ⌃ letting them be. CI types ⌘V into the
  search box, ⌘, ⌃⌘F and ⌘Q through System Events (the run's summary says whether ⌘Q quit),
  but not ⌘C, ⌘A or anything in Settings' fields.
- **First launch.** Ad-hoc signed and not notarized, so Gatekeeper asks before it opens
  (Control-click, Open), and quarantine applies to a downloaded `.dmg`.
- **Updates.** The updater's `.dmg` install (mount, swap the bundle) has tests but has never run.
- **Media keys.** The desktop media controls (MPRIS, Windows' SMTC) have no macOS counterpart
  yet.
- **HDR.** The layer is SDR; HDR video is tone-mapped by mpv.
- **zapper-server** builds on macOS and runs as a LaunchAgent in CI, but is not in the bundle
  yet: Settings → Recording → Install finds one on `PATH` (or named by `ZAPPER_SERVER_BIN`), and
  `zapper-server --install-service` installs itself.
