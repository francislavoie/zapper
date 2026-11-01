#!/usr/bin/env bash
# Gives the app an icon in the dock and the launcher, for a build run from
# this tree rather than installed from a package.
#
# For an actual install use `just package` and the .deb it produces — that
# puts the binary on PATH and its own desktop entry in /usr/share.
#
# The two must never coexist. Both are `zapper.desktop`, because that name
# has to match the window's Wayland app_id, and a per-user entry always
# shadows the system one: the launcher would keep starting whatever build the
# per-user entry points at while the installed package sat unused. So this
# refuses to install while the package is present (a development build's
# window takes its icon from the package's entry anyway, so nothing is lost),
# and --uninstall is how to clear one left over from before.
#
# Wayland windows carry no icon. The compositor pairs the toplevel's app_id
# with an installed `.desktop` file and takes the icon from there, so a
# development build launched with `npm run app` shows the generic
# placeholder however many PNGs are bundled into it. This installs the
# desktop entry and the icons into the per-user XDG directories, which needs
# no root and is undone by `--uninstall`.
#
#   tools/install-desktop.sh              # install, pointing at target/debug
#   tools/install-desktop.sh --release    # point at target/release instead
#   tools/install-desktop.sh --uninstall
#   tools/install-desktop.sh --force      # install even over the package
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
apps="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
icons="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor"
entry="$apps/zapper.desktop"
profile=debug
force=0

# Where a packaged zapper announces itself: the updater's install-kind marker
# (.deb and .rpm both ship it), or the system desktop entry.
packaged() {
  [[ -e /usr/lib/zapper/install-kind || -e /usr/share/applications/zapper.desktop ]]
}

for arg in "$@"; do
  case "$arg" in
    --release) profile=release ;;
    --force) force=1 ;;
    --uninstall)
      rm -f "$entry"
      rm -f "$icons"/*/apps/zapper.png "$icons/scalable/apps/zapper.svg"
      update-desktop-database "$apps" 2>/dev/null || true
      gtk-update-icon-cache -f -t "$icons" 2>/dev/null || true
      echo "removed zapper.desktop and its icons"
      exit 0
      ;;
    *) echo "usage: install-desktop.sh [--release] [--force] [--uninstall]" >&2; exit 2 ;;
  esac
done

if packaged && [[ $force -eq 0 ]]; then
  echo "zapper is installed from a package; not adding a per-user entry." >&2
  echo "It would shadow the package's and launch $profile builds from this tree" >&2
  echo "instead. Development windows already get their icon from the package." >&2
  if [[ -e "$entry" ]]; then
    echo >&2
    echo "A per-user entry is shadowing the package now:" >&2
    echo "  $entry -> $(sed -n 's/^Exec=//p' "$entry")" >&2
    echo "Remove it with: tools/install-desktop.sh --uninstall" >&2
  fi
  echo "(--force installs anyway.)" >&2
  exit 1
fi

binary="$here/src-tauri/target/$profile/zapper"
if [[ ! -x "$binary" ]]; then
  echo "no binary at $binary — build it first (npm run app, or cargo build)" >&2
  exit 1
fi

mkdir -p "$apps"
# The sizes the theme looks for, from the same PNGs the bundle uses.
install -Dm644 "$here/src-tauri/icons/32x32.png"     "$icons/32x32/apps/zapper.png"
install -Dm644 "$here/src-tauri/icons/128x128.png"   "$icons/128x128/apps/zapper.png"
install -Dm644 "$here/src-tauri/icons/128x128@2x.png" "$icons/256x256/apps/zapper.png"
install -Dm644 "$here/src-tauri/icons/icon.svg"      "$icons/scalable/apps/zapper.svg"

# Exec has to be the absolute path: nothing named `zapper` is on PATH when
# it is being run out of a build tree.
sed "s|^Exec=zapper$|Exec=$binary|" "$here/packaging/zapper.desktop" > "$entry"
chmod 644 "$entry"

update-desktop-database "$apps" 2>/dev/null || true
gtk-update-icon-cache -f -t "$icons" 2>/dev/null || true

echo "installed $entry"
echo "  icon:   $icons/128x128/apps/zapper.png"
echo "  exec:   $binary"
echo
echo "Restart the app so the compositor sees its app_id against the new entry."
