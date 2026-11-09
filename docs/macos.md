# zapper on macOS

> **This line is the rewrite:** it has no desktop app. The Tauri app (0.4.x), how it drew video
> under its page on macOS, its window and keyboard, its `.app` and `.dmg` and their checks are on
> the [`main-tauri`](https://github.com/francislavoie/zapper/tree/main-tauri) branch.

Here macOS on Apple Silicon (arm64) builds the core, the libmpv backend and `zapper-server`, as
checked on GitHub's macOS runners by [.github/workflows/macos.yml](../.github/workflows/macos.yml).
It is not released: there is no published macOS libmpv in `packaging/libmpv/manifest.json`.
Intel Macs are not built.

## Building

You need Xcode (its Swift compiler builds part of libmpv), Rust, Node.js, and from Homebrew a
`bash` newer than macOS's 3.2 (first on `PATH`), `meson`, `ninja` and `pkgconf` for libmpv. Until
a macOS libmpv is published, build it here (a few minutes on an M1) and unpack that:

```sh
brew install bash meson ninja pkgconf
just build-libmpv macos-arm64
just fetch-libmpv --archive target-libmpv/out/libmpv-macos-arm64.tar.xz
npm ci
just test         # Rust, interface and release-script checks
```

## What CI checks

`macos.yml` runs on the Mac runners every Monday and on Run workflow for main, and on a pull
request only when it can change what macOS gets: `paths.rs`, the recording service's installer
and `zapper_server.rs`, any Rust file with `target_os = "macos"`, the libmpv build, Cargo config,
or the workflow itself; or when the pull request carries the `macos` label. A small Linux job,
*macOS needed?*, decides in seconds. When it runs, it:

- builds the LGPL libmpv for macos-arm64 with `scripts/build-libmpv.sh`, cached until the script
  or `packaging/libmpv/sources.txt` changes;
- runs Rust's unit tests (zapper-core, zapper-mpv, zapper-server) and clippy (the
  harnesses too), builds zapper-server and the interface, and runs the interface's checks;
- installs the built zapper-server as a LaunchAgent (`zapper-server --install-service`), on a
  scratch home and port, checks the plist with `plutil -lint`, that `launchctl print` says it
  runs and that it answers (`--busy` says idle), then removes it (`--remove-service`) and checks
  that launchd has let it go and that it stopped cleanly.

## What needs a real Mac

- **zapper-server** as a LaunchAgent on a real login, and the browser interface against it.
- **libmpv** on a GPU: the runners have none, and the player checks' host
  (`tools/headless_host.sh`) is Linux-only.
