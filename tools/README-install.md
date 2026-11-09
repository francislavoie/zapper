# Installing zapper

## A real install

```sh
just package                        # .deb and .rpm under target/release/bundle/
sudo apt install --reinstall ./target/release/bundle/deb/zapper_*_amd64.deb
```

`--reinstall` because a rebuild of the same version is otherwise "already
installed" and skipped. That puts `zapper` on `PATH`, its desktop entry in
`/usr/share/applications/` and its icons in the hicolor theme, so it appears in
the launcher like any other application.

**Remove any entry `tools/install-desktop.sh` made first.** Both are
`zapper.desktop`, and the per-user one wins: the launcher keeps starting
whatever build it points at in this tree, with the old icon, while the package
sits unused.

```sh
tools/install-desktop.sh --uninstall
```

If GNOME's search still shows nothing afterwards, the shell is holding a stale
view of the id; logging out and back in rebuilds it.

## Running from the tree

`npm run app` builds and runs a development build. With the package installed
its window already takes the package's icon, so nothing more is needed.
Without it, the dock shows a placeholder — `tools/install-desktop.sh` installs
a per-user entry pointing at `target/debug/zapper`, and refuses to
while the package is installed (`--force` overrides).

## Why a desktop entry is needed at all

On Wayland a window carries no icon. The compositor pairs the toplevel's
`app_id` with an installed `.desktop` file and takes the icon from there, so
an application with no entry gets the generic placeholder however many icons
are compiled into it. `APP_ID` in `crates/zapper/src/lib.rs` fixes that id to
`zapper`, and both desktop entries carry `StartupWMClass=zapper` to match.

## Where your account lives

`~/.config/zapper/account.json`, owner-readable only. Set it in
**Settings → Account**.

The catalogue and guide mirror is separate, at
`~/.local/share/zapper/zapper.db`. That one is a cache: deleting it costs a
resync, not your account.
