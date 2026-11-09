# zapper on Windows

> **This line is the rewrite:** it has no desktop app. The Tauri app (0.4.x), its installers
> (NSIS and MSI), how it put the video under its page and how it ran on Windows are described on
> the [`main-tauri`](https://github.com/francislavoie/zapper/tree/main-tauri) branch.

Here, Windows gets `zapper-server-<version>-windows-x86_64.zip`: `zapper-server.exe` (the
recording service, which also serves the browser interface), the libmpv and FFmpeg DLLs it loads
and the license notices, built and checked by the release workflow (docs/releasing.md).

## What you need

- **Rust**, stable, MSVC toolchain (`rustup default stable-x86_64-pc-windows-msvc`).
- **Visual Studio 2022 Build Tools** with "Desktop development with C++"
  (the linker, and `lib.exe` for the step below).
- **Node.js and npm** for the interface zapper-server serves.
- **libmpv and FFmpeg**, fetched by a script (next section). Not in the repository.

## libmpv and FFmpeg

zapper links libmpv dynamically, and zapper-server (the recording service,
docs/recording.md) FFmpeg's libavformat, libavcodec and libavutil: zapper's
own LGPL build of both (docs/licensing.md, "mpv and FFmpeg"), published as a
pre-release of this repository and named in `packaging/libmpv/manifest.json`.
libmpv2-sys asks the linker for `mpv`, and the remuxer for `avformat` and
the rest, which MSVC resolves to *import libraries* (`mpv.lib`,
`avformat.lib`, ...) for the DLLs. None of it is committed (`crates/zapper/lib/`
is ignored).

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools\fetch-libmpv.ps1
```

That downloads the manifest's `libmpv-windows-x86_64.zip`, checks it against
the manifest's SHA-256, and leaves in `crates\zapper\lib\windows-x86_64\`:

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
executable it builds, so a debug `zapper-server.exe` and the harnesses find
the DLLs without PATH changes. The release's zip carries them beside
`zapper-server.exe`, and their notices in `licenses\`.

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
npm run build
cargo build --release -p zapper --features server --bin zapper-server
```

`zapper-server.exe` has no window and no console of its own, so that the
sign-in task that runs it opens nothing. Started from a terminal it prints to
that terminal, but the terminal does not wait for it: `start /wait /b
zapper-server.exe` in `cmd`, or `zapper-server.exe | Out-Host` in
PowerShell, keeps it in the foreground. Started by the task, it logs to
`%LOCALAPPDATA%\zapper\zapper-server.log`.

### The recording service

`zapper-server.exe --install-service` (what the desktop app's Settings →
Recording → Install does) registers a scheduled task for the signed-in
user, `zapper-server-<user>`, through Task Scheduler's COM interface: at that
user's sign-in, run that `zapper-server.exe`, unelevated,
with no stored password, with no time limit and on battery too
(`recording::service`, `task_xml`). Install also starts it, and Remove stops
and deletes it. It runs while the user is signed in. Not offered: waking the
computer to record (a task per recording, with a wake timer).

`--install-service --boot` (the app's **Start at boot** switch) runs it as a
real Windows service of the same name instead, started when the computer
starts whether or not anyone signs in (`recording::service::winsvc`). One or
the other, never both: switching stops the running one cleanly before the
other starts, and either way Remove removes whichever is there.

- **Its account.** Its own virtual account, `NT SERVICE\zapper-server-<user>`:
  no password, no rights but what it is given. Not LocalSystem, because the
  service may answer on the network, and a flaw in it should not be one in
  the most powerful account on the computer; not the user's own account,
  which a service could only use with their password stored in the Service
  Control Manager.
- **Its files: the user's.** The service's `%APPDATA%` is the virtual
  account's, so the user's folders are on its command line:
  `zapper-server.exe --service zapper-server-<user> --config-dir … --data-dir
  … --cache-dir … --recordings-dir …` (or the folders under `ZAPPER_HOME`
  when the app runs on one). It is granted Modify on those and on the
  recordings folder in its settings, and Read & Execute on the folder
  `zapper-server.exe` is in when that is in the user's profile, with
  `icacls`, inherited by what is in them. The user keeps their access, and
  what the service writes inherits it, so the app finds the token in its
  config folder as before; the token and loopback rules are unchanged. A
  recordings folder chosen later is granted by the app, as the user (they
  own it). A network share or mapped drive is out of a service's reach.
- **One UAC prompt.** Creating a service needs an administrator: the app
  runs `zapper-server.exe --install-boot-service …` elevated, with everything
  it does decided beforehand on its command line rather than in a file. It
  creates the service (automatic start; restarted ten seconds after a crash,
  three times a day; a minute's pre-shutdown notice to finish what it
  records), grants the folders, and sets the service's security descriptor so
  that the user who installed it (by SID) may start, stop, query and delete
  it, but not reconfigure it. From then on the app, the updater and the
  uninstaller manage it as that user with no prompt. A standard user's
  administrator can approve it: the plan names the user's folders and SID,
  not the administrator's.
- **Its log** is the same `zapper-server.log` in the user's data folder.
- **Network access** with the service: Windows Firewall asks about a
  program that listens only when a signed-in user's program does, so for
  other devices to reach the service, allow `zapper-server.exe` in for
  private networks (Windows Security → Firewall → Allow an app, or
  `netsh advfirewall firewall add rule name="zapper-server" dir=in
  action=allow program="<path>\zapper-server.exe" profile=private`).

A running `zapper-server.exe` cannot be replaced, so whatever replaces it
stops it first, cleanly (what it is recording is finished into playable files
and carries on in a new part when it runs again; docs/recording.md,
"Stopping and updating"): `zapper-server.exe --busy` says whether it is
recording, `--stop-for-setup` stops it (and, for the boot-time service,
waits for its process to exit), and `--remove-service` deletes the task or
the boot-time service and takes back its folder permissions. Recordings, the
service's settings and its list of recordings stay.

`service_check` installs it, checks it answers, and removes it, on a scratch
home, with no window:

```powershell
cargo build --manifest-path crates\zapper\Cargo.toml --features harness,server --bin zapper-server --bin service_check
target\debug\service_check.exe
```

### The harnesses on Windows

`ending_check` (the `mpv` feature), `recording_check` and
`service_check` run on Windows as on Linux, headless and silent. They need the
fixtures (copy them from a Linux `tools/make_fixtures.sh` run and point
`ZAPPER_FIXTURES` at the folder) and Python, ffprobe and ffmpeg on `PATH`, or
named by `ZAPPER_PYTHON3`, `ZAPPER_FFPROBE` and `ZAPPER_FFMPEG`. A scoop shim
on `PATH` is fine: the harnesses start the program it points at.
`player_check`, `catchup_check` and `host_smoke` play in a window mpv makes
itself (`surface::host`) and are Linux-only for now.

## Where things are kept

| | Location |
|---|---|
| Account | `%APPDATA%\zapper` |
| Catalogue database | `%LOCALAPPDATA%\zapper\zapper.db` |
| The recording service's log | `%LOCALAPPDATA%\zapper\zapper-server.log`, with the one before as `zapper-server.old.log` |

See `crates/zapper/src/paths.rs` for every platform's rules.
