# zapper on Windows

zapper builds and runs on Windows 10 and 11 (x64), and each release carries
two installers for it. The interface is the same WebView2 page as on Linux;
video is drawn by libmpv into a window of its own beneath the page. This
covers the installers, what a Windows build needs, how to run it, and what is
not there yet.

## The installers

Built by the release workflow on every tag (docs/releasing.md), from one
build:

| File | Installs |
|---|---|
| `zapper_<version>_x64-setup.exe` | NSIS: for the current user, no administrator, into `%LOCALAPPDATA%\zapper` |
| `zapper_<version>_x64_en-US.msi` | for every user, as an administrator, into `Program Files\zapper` |

Both put `zapper.exe`, `zapper-server.exe` (the recording service), libmpv
and FFmpeg's DLLs side by side, the license notices in `licenses\`, and an
`install-kind` file (`nsis` or `msi`) the updater reads to download the same
kind next time. They are not signed yet: SmartScreen warns on the first run
(**More info** → **Run anyway**).

`scripts/check-windows.ps1` checks a pair of them, and the release workflow
runs it on every build: the files above, nothing imported from outside the
installer but Windows' own DLLs (no Visual C++ redistributable: both
programs link its runtime in), FFmpeg's libraries and libmpv asked for their
licenses (LGPL, `-Dgpl=false`), and `zapper-server.exe --version` answering.
With `-Install` it really installs the NSIS setup, silently, to see its hook
write `nsis`, and uninstalls it; leave that off on a computer you use.

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\check-windows.ps1 -Dir out -Version 0.2.0
```

## What you need

- **Rust**, stable, MSVC toolchain (`rustup default stable-x86_64-pc-windows-msvc`).
- **Visual Studio 2022 Build Tools** with "Desktop development with C++"
  (the linker, and `lib.exe` for the step below).
- **Node.js and npm** for the interface.
- **WebView2 runtime**: part of Windows 11, and of Windows 10 since 2021.
- **libmpv**, fetched by a script (next section). Not in the repository.

## libmpv and FFmpeg

zapper links libmpv dynamically, and zapper-server (the recording service,
docs/recording.md) FFmpeg's libavformat, libavcodec and libavutil: zapper's
own LGPL build of both (docs/licensing.md, "mpv and FFmpeg"), published as a
pre-release of this repository and named in `packaging/libmpv/manifest.json`.
libmpv2-sys asks the linker for `mpv`, and the remuxer for `avformat` and
the rest, which MSVC resolves to *import libraries* (`mpv.lib`,
`avformat.lib`, ...) for the DLLs. None of it is committed (`src-tauri/lib/`
is ignored).

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools\fetch-libmpv.ps1
```

That downloads the manifest's `libmpv-windows-x86_64.zip`, checks it against
the manifest's SHA-256, and leaves in `src-tauri\lib\windows-x86_64\`:

| Path | What it is |
|---|---|
| `runtime\` | `libmpv-2.dll` and FFmpeg's DLLs (`avcodec-62.dll`, `avformat-62.dll`, ...): what ships |
| `link\*.lib` | the import libraries the linker uses, made by the script from the `.def` export lists beside them |
| `link\*.dll.a` | MinGW's import libraries, unused by MSVC |
| `include\` | mpv's and FFmpeg's C headers (the remuxer compiles against FFmpeg's) |
| `THIRD-PARTY-NOTICES-libmpv.txt` | what is inside the DLLs, and every license's text |
| `BUILDINFO.txt` | the sources and configuration they were built from |

It finds `lib.exe` through `vswhere`. `-Archive <zip>` takes a zip you built
yourself instead of downloading (`scripts/build-libmpv.sh windows-x86_64`,
cross-compiled on Linux with MinGW-w64, or natively in an MSYS2 MINGW64
shell).

To make an import library by hand instead, from a "x64 Native Tools Command
Prompt" in `link\` (each `.def` names its DLL):

```bat
lib /def:mpv.def /out:mpv.lib /machine:x64
```

`build.rs` adds `link\` to the link path and copies `runtime\` beside the
executable it builds, so `cargo run`, `npm run app` and a debug
`zapper-server.exe` find the DLLs without PATH changes. Installers carry
them as bundle resources (`src-tauri/tauri.windows.conf.json`), installed
beside `zapper.exe` with `zapper-server.exe`
(`packaging/windows/installers.conf.json`), and their notices in
`licenses\`.

The DLLs are built with Direct3D 11 output (shaderc and SPIRV-Cross linked
into libmpv), D3D11VA and DXVA2 decoding and SChannel for HTTPS, and need
nothing but Windows' own DLLs and each other. Checked so far headless, on
Windows: playback through libmpv (D3D11VA copy-back, HLS over HTTPS, the
compressor's filters) and a recording by zapper-server through the same
FFmpeg. Not yet checked by a person: video on screen through Direct3D 11.

## Building and running

```powershell
npm ci
powershell -NoProfile -ExecutionPolicy Bypass -File tools\fetch-libmpv.ps1
npm run app            # tauri dev: the Vite dev server plus a debug build
npx tauri build        # release: an NSIS installer and an MSI in src-tauri\target\release\bundle
just package-windows   # the same, plus the install-kind markers the updater reads
```

`src-tauri/tauri.windows.conf.json` is what every Windows build needs (the
installer targets, the `.ico`, libmpv as a resource); Tauri merges it in by
itself. `packaging/windows/installers.conf.json` adds only what a release
needs on top (docs/releasing.md).

A debug build opens a console beside the window, with zapper's log in it.

`zapper-server.exe` has no window and no console of its own, so that the
sign-in task that runs it opens nothing. Started from a terminal it prints to
that terminal, but the terminal does not wait for it: `start /wait /b
zapper-server.exe` in `cmd`, or `zapper-server.exe | Out-Host` in
PowerShell, keeps it in the foreground. Started by the task, it logs to
`%LOCALAPPDATA%\zapper\zapper-server.log`.

### The recording service

Settings → Recording → Install registers a scheduled task for the signed-in
user, `zapper-server-<user>`, through Task Scheduler's COM interface: at that
user's sign-in, run `zapper-server.exe` from beside `zapper.exe`, unelevated,
with no stored password, with no time limit and on battery too
(`recording::service`, `task_xml`). Install also starts it, and Remove stops
and deletes it. It runs while the user is signed in. Not offered: recording
with nobody signed in (the task would need the account's password), and
waking the computer to record (a task per recording, with a wake timer).

A running `zapper-server.exe` cannot be replaced, so whatever replaces it
stops it first, cleanly (what it is recording is finished into playable files
and carries on in a new part when it runs again; docs/recording.md,
"Stopping and updating"):

- zapper's updater asks the service first; if it is recording, the update
  dialog offers to install when the recording finishes, or now. It then
  stops the task, starts the installer, and zapper starts the task again
  when it reopens.
- The setup (`packaging/windows/installer-hooks.nsh`) asks the installed
  `zapper-server.exe` whether it is recording (`--busy`); run by hand, it
  then asks whether to stop it, and cancels if not; silent, it does not ask.
  It stops it (`--stop-for-setup`), and once installed starts the task
  again.
- The MSI (`packaging/windows/service-cleanup.wxs`) runs
  `zapper-server.exe --stop-for-setup` as the installing user before any
  file changes, with `--ask` in full UI. An EXE custom action cannot cancel
  an MSI cleanly (only fail it, with a generic error), so a "no" leaves the
  service running and Windows Installer's files-in-use dialog follows,
  which can cancel. Other users' running services are files in use.

Uninstalling removes the service:

- the setup's uninstaller, on an uninstall, stops this user's service as
  above and deletes its task and `zapper-server.log`. Tauri's template also
  runs the uninstaller when a newer setup replaces this one ("uninstall
  before installing"), in place from the install folder, while an
  uninstall from Settings → Apps runs a copy from a temporary folder: that,
  and `/UPDATE`, is how the hook tells the two apart and keeps the task
  through an upgrade;
- the MSI, on a full removal only (not the removal half of an upgrade),
  runs an action as SYSTEM, which sees every user's tasks, that stops and
  deletes every `zapper-server-*` task whose author is zapper.

Recordings, the service's settings and its list of recordings stay.

`scripts/check-windows.ps1 -Install` checks this on the release runner: a
task made by `zapper-server --install-service` survives installing the setup
over itself and the in-place uninstall an upgrade runs, and goes with a real
uninstall of the setup and with the MSI's removal.

`service_check` installs it, checks it answers, and removes it, on a scratch
home, with no window:

```powershell
cargo build --manifest-path src-tauri\Cargo.toml --features harness,server --bin zapper-server --bin service_check
src-tauri\target\debug\service_check.exe
```

### The harnesses on Windows

`ending_check` (with the GUI features), `recording_check` and
`service_check` run on Windows as on Linux, headless and silent. They need the
fixtures (copy them from a Linux `tools/make_fixtures.sh` run and point
`ZAPPER_FIXTURES` at the folder) and Python, ffprobe and ffmpeg on `PATH`, or
named by `ZAPPER_PYTHON3`, `ZAPPER_FFPROBE` and `ZAPPER_FFMPEG`. A scoop shim
on `PATH` is fine: the harnesses start the program it points at.

### Trying it without sound

`ZAPPER_MUTE=1` opens no audio device at all (`ao=null`, which no volume or
mute control can undo), for looking at the picture where the speakers must
stay quiet:

```powershell
$env:ZAPPER_MUTE = "1"; npm run app
```

`ZAPPER_AUTOPLAY=<stream id>` also starts a channel three seconds after
launch, and implies the same silence.

Both are unattended runs, so they get a scratch account and catalogue rather
than the ones on the machine: point them at the fixture server with
`ZAPPER_FIXTURE_BASE`, or set `ZAPPER_ALLOW_REAL_ACCOUNT=1` to use the real
account on purpose (tools/README.md, "Never the real account").

## How the video gets under the page

On Linux, zapper renders mpv's frames itself into a GL surface slid beneath
WebKitGTK's webview. On Windows the arrangement is simpler
(`src-tauri/src/surface/windows`):

1. A child window, `ZAPPER_VIDEO`, is created inside the main window and kept
   at the **bottom** of the main window's children. wry puts WebView2's
   container at the top, so the page is always above the video and every
   click reaches the page. The video window is disabled and transparent to
   hit tests: it never takes input.
2. mpv draws into it by itself: `wid=<that window>`, `vo=gpu-next`,
   `gpu-api=d3d11`, `hwdec=auto-safe` (D3D11VA where the GPU can). mpv's own
   keyboard and mouse handling is off.
3. The webview's background is transparent (WebView2's
   `DefaultBackgroundColor`, alpha 0), so the video shows wherever the page
   paints nothing. The window under both is painted black.
4. Docking the picture into a corner moves the child window
   (`SetWindowPos`, physical pixels). A subclass of the main window reapplies
   the position on every resize, DPI changes included.

## What the visual test should show

- The channel list and guide over a black window before anything plays.
- Pick a channel: the picture fills the window behind the player controls,
  which float over it with their translucent backgrounds intact.
- Browse while it plays: the picture docks into the corner, smoothly, and
  returns to full size on leaving.
- Resize, maximise, drag between monitors with different scaling: the picture
  follows the window.
- Clicks and keys all go to the interface; nothing reaches mpv (space does not
  pause twice, double-click does not toggle mpv's own fullscreen).
- Scrub back in the buffer: thumbnails appear under the cursor after the
  first few seconds of playback.

If the page is opaque (no picture, but audio and the state feed report
playing), WebView2 did not accept the transparent background; the log line
`video surface: mpv window embedded beneath the webview` says the surface
itself was installed.

## Not there yet

- **Scrollback thumbnails** come from mpv's `screenshot-raw` on a worker
  thread every four seconds of playback, shrunk on the CPU, rather than the
  GPU copy Linux uses. Same result, more CPU (a few milliseconds a shot).
- **`ZAPPER_CAPTURE`** (the self-screenshot used for unattended verification)
  is Linux-only: it reconstructs GTK's layers. On Windows it says so.
- **The GTK verification harnesses** (`player_check`, `shot_check`,
  `surface_check`) are Linux-only and print so on Windows.
- **No escape hatch if the interface fails to load.** Linux adds a title bar
  and a dialog; on Windows, Alt+F4 still closes the undecorated window.
- **Updates**: only the release build (`just package-windows`, which adds
  `packaging/windows/installers.conf.json`) carries the `install-kind` marker
  the updater reads. A plain `npx tauri build` installer has none and counts
  as a build from source, pointed at the release page. Updating one
  installed copy to the next has not been tried end to end yet: the first
  release with Windows installers is the first that can be updated from.
- **Signing**: the installers are unsigned (SmartScreen warns once).

## Where things are kept

| | Location |
|---|---|
| Account, window size | `%APPDATA%\zapper` |
| Catalogue database | `%LOCALAPPDATA%\zapper\zapper.db` |
| Downloaded updates | `%LOCALAPPDATA%\zapper\updates` |
| WebView2 profile | `%LOCALAPPDATA%\dev.lavoie.zapper` (Tauri's default) |

See `src-tauri/src/paths.rs` for every platform's rules.
