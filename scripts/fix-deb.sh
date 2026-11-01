#!/usr/bin/env bash
# Usage: scripts/fix-deb.sh <package.deb>
#
# Tauri writes the SemVer version into the .deb's control file as it is, and dpkg reads
# 0.2.0-rc.1 as upstream 0.2.0, Debian revision rc.1 — which sorts *after* 0.2.0. An rc
# installed through the updater would then refuse the real 0.2.0 as a downgrade. This rewrites
# only the Version field into Debian's form (scripts/version.py deb: 0.2.0~rc.1, which sorts
# before 0.2.0) and leaves everything else, the data member included, byte for byte. A release
# version without a pre-release part is already right, and is left alone.
#
# The .rpm needs nothing: Tauri already writes 0.2.0~rc.1 there.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
deb=$(realpath "${1:?usage: fix-deb.sh <package.deb>}")
work=$(mktemp -d); trap 'rm -rf "$work"' EXIT
cd "$work"
ar x "$deb"
[ -f debian-binary ] || { echo "fix-deb: not a .deb: $deb" >&2; exit 1; }
ctrl=$(ls control.tar.*)
data=$(ls data.tar.*)
mkdir control
tar -xf "$ctrl" -C control
old=$(sed -n 's/^Version: //p' control/control)
new=$(python3 "$here/version.py" deb "$old")
if [ "$old" = "$new" ]; then
  echo "fix-deb: Version $old is already Debian's form"
  exit 0
fi
sed -i "s/^Version: .*/Version: $new/" control/control
case "$ctrl" in
  *.gz) comp=(-z) ;;
  *.xz) comp=(-J) ;;
  *.zst) comp=(--zstd) ;;
  *.tar) comp=() ;;
  *) echo "fix-deb: unknown control compression: $ctrl" >&2; exit 1 ;;
esac
(cd control && tar --owner=0 --group=0 --numeric-owner --sort=name "${comp[@]}" -cf "../$ctrl.new" .)
mv "$ctrl.new" "$ctrl"
rm -f "$deb.tmp"
ar rc "$deb.tmp" debian-binary "$ctrl" "$data" # member order matters to dpkg
mv "$deb.tmp" "$deb"
echo "fix-deb: Version $old -> $new"
