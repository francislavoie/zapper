# Installing zapper

## A real install

```sh
npx tauri build                     # writes a .deb under src-tauri/target/release/bundle/deb/
sudo dpkg -i src-tauri/target/release/bundle/deb/zapper_*_amd64.deb
```

That puts `zapper` on `PATH`, its desktop entry in `/usr/share/applications/`
and its icons in the hicolor theme, so it appears in the launcher like any
other application. `sudo apt -f install` afterwards if dpkg reports a missing
dependency.

If you previously ran `tools/install-desktop.sh`, remove its entry first or
you will have two:

```sh
tools/install-desktop.sh --uninstall
```

## Running from the tree

`npm run app` builds and runs a development build. It has no desktop entry of
its own, so the dock shows a placeholder icon — `tools/install-desktop.sh`
installs one pointing at `src-tauri/target/debug/zapper`.

## Why a desktop entry is needed at all

On Wayland a window carries no icon. The compositor pairs the toplevel's
`app_id` with an installed `.desktop` file and takes the icon from there, so
an application with no entry gets the generic placeholder however many icons
are compiled into it. `APP_ID` in `src-tauri/src/lib.rs` fixes that id to
`zapper`, and both desktop entries carry `StartupWMClass=zapper` to match.

## Where your account lives

`~/.config/zapper/account.json`, owner-readable only. Set it in
**Settings → Account**.

The catalogue and guide mirror is separate, at
`~/.local/share/zapper/zapper.db`. That one is a cache: deleting it costs a
resync, not your account.
