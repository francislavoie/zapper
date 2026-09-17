#!/usr/bin/env bash
# Gives the app an icon in the dock and the launcher, for a build run from
# this tree rather than installed from a package.
#
# For an actual install use `npx tauri build` and the .deb it produces —
# that puts the binary on PATH and its own desktop entry in /usr/share, and
# this script's entry should be removed first with --uninstall so there are
# not two.
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
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
apps="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
icons="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor"
entry="$apps/zapper.desktop"
profile=debug

for arg in "$@"; do
  case "$arg" in
    --release) profile=release ;;
    --uninstall)
      rm -f "$entry"
      rm -f "$icons"/*/apps/zapper.png "$icons/scalable/apps/zapper.svg"
      update-desktop-database "$apps" 2>/dev/null || true
      gtk-update-icon-cache -f -t "$icons" 2>/dev/null || true
      echo "removed zapper.desktop and its icons"
      exit 0
      ;;
    *) echo "usage: install-desktop.sh [--release] [--uninstall]" >&2; exit 2 ;;
  esac
done

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
