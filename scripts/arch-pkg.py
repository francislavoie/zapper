#!/usr/bin/env python3
"""Helpers for scripts/package-arch.sh (the Arch package built from the .deb, without makepkg).

Subcommands (each prints to stdout; errors go to stderr with exit status 2):
  pkgver <deb-version>          the Debian version as a pacman pkgver: the '~' goes and '+'
                                becomes '.' (0.2.0~rc.1 -> 0.2.0rc.1). vercmp puts a leftover
                                part that starts with a letter before the release, but one that
                                starts with '.' after it: 0.2.0.rc.1 would be newer than 0.2.0.
  depends <Depends-field> [payload-dir]
                                the Arch dependencies, one per line, sorted and de-duplicated
                                (DEB_TO_ARCH). A dependency on a library the payload bundles
                                is dropped, with a note on stderr.
  bundled <payload-dir>         the Debian package names of the shared libraries the payload
                                carries, one per line (what `depends` drops)
  escape <path>                 a path in mtree's escaping (tests)
  mtree <dir> <epoch> <out.gz>  writes the gzip-compressed .MTREE for everything under <dir>
  size <dir>                    installed size in bytes (apparent size, as makepkg reports it)
  licenses <doc-dir>            the package's licenses (SPDX), one per line: zapper's MIT, then
                                those the third-party notices' summaries list
  verify <src.tar> <pkg.tar> [edited...]
                                the package's payload matches the .deb's data member: same
                                paths, types, modes, sizes and link targets (plus ARCH_EXTRA;
                                the edited files' sizes may differ), every entry root:root
"""
import fnmatch
import glob
import gzip
import hashlib
import os
import re
import stat
import sys
import tarfile

# Debian package (glob) -> Arch packages. Every pattern is tried; a Debian name that matches none
# fails the build, so a new dependency is never dropped by accident. An empty list says the
# dependency has no counterpart worth naming on Arch (part of the base system there).
#
# A dependency on a library the package bundles needs no row: `depends` drops it by itself (see
# bundled_names), so moving a library into the package only means removing it from the .deb's
# Depends, and a row left behind here for it is harmless.
DEB_TO_ARCH = [
    # What tauri.conf.json's deb.depends names, and what Tauri adds to it.
    ("libwebkit2gtk-4.1-0", ["webkit2gtk-4.1"]),
    ("libjavascriptcoregtk-4.1-0", ["webkit2gtk-4.1"]),
    ("libgtk-3-0*", ["gtk3"]),
    # Arch's mpv package ships libmpv.so.2.
    ("libmpv2", ["mpv"]),
    ("libmpv1", ["mpv"]),
    # What a bundled library, or a future Depends, is likely to bring in.
    ("libc6", ["glibc"]),
    ("libgcc-s1", ["gcc-libs"]),
    ("libstdc++6", ["gcc-libs"]),
    ("libglib2.0-0*", ["glib2"]),
    ("libcairo2", ["cairo"]),
    ("libpango-1.0-0", ["pango"]),
    ("libgdk-pixbuf-2.0-0", ["gdk-pixbuf2"]),
    ("libsoup-3.0-0", ["libsoup3"]),
    ("libayatana-appindicator3-1", ["libayatana-appindicator"]),
    ("libepoxy0", ["libepoxy"]),
    ("libasound2*", ["alsa-lib"]),
    ("libpulse0", ["libpulse"]),
    ("libpipewire-0.3-0*", ["libpipewire"]),
    ("libva2", ["libva"]),
    ("libva-drm2", ["libva"]),
    ("libva-x11-2", ["libva"]),
    ("libva-wayland2", ["libva"]),
    ("libvdpau1", ["libvdpau"]),
    ("libdrm2", ["libdrm"]),
    ("libgbm1", ["mesa"]),
    ("libegl1", ["libglvnd"]),
    ("libgl1", ["libglvnd"]),
    ("libx11-6", ["libx11"]),
    ("libxext6", ["libxext"]),
    ("libxrandr2", ["libxrandr"]),
    ("libxss1", ["libxss"]),
    ("libxpresent1", ["libxpresent"]),
    ("libxkbcommon0", ["libxkbcommon"]),
    ("libwayland-client0", ["wayland"]),
    ("libwayland-cursor0", ["wayland"]),
    ("libwayland-egl1", ["wayland"]),
    ("libvulkan1", ["vulkan-icd-loader"]),
    ("libfreetype6", ["freetype2"]),
    ("libfontconfig1", ["fontconfig"]),
    ("libfribidi0", ["fribidi"]),
    ("libharfbuzz0b", ["harfbuzz"]),
    ("libass9", ["libass"]),
    ("libdbus-1-3", ["dbus"]),
    ("libssl3*", ["openssl"]),
    ("zlib1g", ["zlib"]),
    ("liblzma5", ["xz"]),
    ("libbz2-1.0", ["bzip2"]),
]


class Fail(Exception):
    pass


def pkgver(debver):
    v = debver.strip().replace("~", "").replace("+", ".")
    if not v or re.search(r"[-:/\s]", v):
        raise Fail(f"can't map the .deb version {debver!r} to a pkgver (no '-', ':', '/' or spaces allowed)")
    if not re.fullmatch(r"[A-Za-z0-9._]+", v):
        raise Fail(f"unexpected characters in the .deb version {debver!r}")
    return v


# libfoo.so.2.3.0 -> soname libfoo.so.2.
SONAME = re.compile(r"^(lib[^/]+?)\.so\.(\d+)(?:\.[0-9.]+)?$")


def debian_name(soname_stem, major):
    """Debian policy 8.1: the package of libfoo.so.2 is libfoo2, of libfoo-1.0.so.0 libfoo-1.0-0
    (a hyphen when the name ends in a digit), lowercased."""
    sep = "-" if soname_stem[-1].isdigit() else ""
    return f"{soname_stem}{sep}{major}".lower()


def bundled_names(root):
    """The Debian package names of the shared libraries under root, wherever they are."""
    names = set()
    if not root:
        return names
    for dirpath, _, filenames in os.walk(root):
        for n in filenames:
            m = SONAME.match(n)
            if m:
                names.add(debian_name(m.group(1), m.group(2)))
    return names


def without_t64(name):
    # Debian's 64-bit time_t transition renamed libfoo2 to libfoo2t64 without changing the library.
    return name[:-3] if name.endswith("t64") else name


def depends(field, root=None, note=lambda msg: None):
    bundled = bundled_names(root)
    out = set()
    unknown = []
    for item in field.split(","):
        item = item.strip()
        if not item:
            continue
        names = [re.split(r"[\s(:]", alt.strip(), maxsplit=1)[0] for alt in item.split("|")]
        if any(without_t64(n) in bundled for n in names):
            note(f"dropped {item!r}: the package bundles it")
            continue
        # Alternatives (libmpv2 | libmpv1): pacman has none, so the first one the table knows.
        for name in names:
            hits = [arch for pat, arch in DEB_TO_ARCH if fnmatch.fnmatchcase(name, pat)]
            if hits:
                for arch in hits:
                    out.update(arch)
                break
        else:
            unknown.append(item)
    if unknown:
        raise Fail("unknown Debian dependencies (add them to DEB_TO_ARCH in scripts/arch-pkg.py): " + ", ".join(unknown))
    return sorted(out)


def escape(path):
    """libarchive's mtree_quote: every byte outside '!'..'~', plus '#', '=' and '\\', becomes \\ooo."""
    b = path.encode("utf-8", "surrogateescape") if isinstance(path, str) else path
    return "".join(chr(c) if 0x21 <= c <= 0x7E and c not in b"#=\\" else "\\%03o" % c for c in b)


def walk(root):
    """Every path under root (relative, without './'), sorted bytewise like makepkg's LC_COLLATE=C glob."""
    found = []
    for dirpath, dirnames, filenames in os.walk(root):
        for n in dirnames + filenames:
            found.append(os.path.relpath(os.path.join(dirpath, n), root))
    return sorted(found, key=lambda p: os.fsencode(p))


def digests(path):
    md5, sha = hashlib.md5(), hashlib.sha256()
    with open(path, "rb") as f:
        while chunk := f.read(1 << 20):
            md5.update(chunk)
            sha.update(chunk)
    return md5.hexdigest(), sha.hexdigest()


def mtree_lines(root, epoch):
    """pacman's .MTREE (bsdtar --format=mtree with makepkg's options): one /set, then per-entry overrides."""
    lines = ["#mtree", "/set type=file uid=0 gid=0 mode=644"]
    for rel in walk(root):
        if rel == ".MTREE":
            continue
        full = os.path.join(root, rel)
        st = os.lstat(full)
        attrs = [f"time={epoch}.0"]
        if stat.S_ISLNK(st.st_mode):
            attrs += ["mode=777", "type=link", "link=" + escape(os.readlink(full))]
        elif stat.S_ISDIR(st.st_mode):
            mode = stat.S_IMODE(st.st_mode)
            if mode != 0o644:
                attrs.append("mode=%o" % mode)
            attrs.append("type=dir")
        elif stat.S_ISREG(st.st_mode):
            mode = stat.S_IMODE(st.st_mode)
            if mode != 0o644:
                attrs.append("mode=%o" % mode)
            md5, sha = digests(full)
            attrs += [f"size={st.st_size}", f"md5digest={md5}", f"sha256digest={sha}"]
        else:
            raise Fail(f"unsupported file type in the payload: {rel}")
        lines.append("./" + escape(rel) + " " + " ".join(attrs))
    return lines


def write_mtree(root, epoch, out):
    data = ("\n".join(mtree_lines(root, int(epoch))) + "\n").encode()
    with open(out, "wb") as raw, gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0, compresslevel=9) as gz:
        gz.write(data)


def installed_size(root):
    total = 0
    for rel in walk(root):
        if rel.startswith("."):
            continue
        st = os.lstat(os.path.join(root, rel))
        if stat.S_ISREG(st.st_mode) or stat.S_ISLNK(st.st_mode):
            total += st.st_size
    return total


def entries(tarpath, skip_meta):
    got = {}
    with tarfile.open(tarpath, "r|") as t:
        for m in t:
            name = m.name[2:] if m.name.startswith("./") else m.name
            name = name.rstrip("/")
            if name in ("", ".") or (skip_meta and name in (".PKGINFO", ".MTREE")):
                continue
            kind = "dir" if m.isdir() else "link" if m.issym() else "hardlink" if m.islnk() else "file" if m.isfile() else "other"
            # A symlink's mode means nothing on Linux (tar records 755 or 777 depending on the tool).
            got[name] = (kind, 0 if m.issym() else m.mode & 0o7777, m.size if m.isfile() else 0, m.linkname if (m.issym() or m.islnk()) else "", m.uid, m.gid, m.uname, m.gname)
    return got


# What package-arch.sh adds to the .deb's payload: Arch's license directory, a symlink to the
# .deb's /usr/share/doc/zapper (and its parent directory, which the .deb doesn't have).
ARCH_EXTRA = {"usr/share/licenses": ("dir", 0o755, 0, ""), "usr/share/licenses/zapper": ("link", 0, 0, "../doc/zapper")}


# Each third-party notice (docs/licensing.md) starts with a summary of the licenses it lists.
NOTICES = "THIRD-PARTY-NOTICES-*.txt"
SUMMARY = re.compile(r"^Summary\n-+\n((?:  \S+ +\d+\n)+)", re.M)


def licenses(doc):
    files = sorted(glob.glob(os.path.join(doc, NOTICES)))
    if not files:
        raise Fail(f"no {NOTICES} in {doc}")
    found = set()
    for path in files:
        with open(path, encoding="utf-8") as f:
            m = SUMMARY.search(f.read())
        if not m:
            raise Fail(f"{os.path.basename(path)} has no license summary")
        found.update(line.split()[0] for line in m.group(1).splitlines())
    found.discard("MIT")
    return ["MIT"] + sorted(found)


def verify(src, pkg, edited=()):
    a = entries(src, False)
    b = entries(pkg, True)
    problems = []
    for name in sorted(set(a) | set(b)):
        if name not in b:
            problems.append(f"missing from the package: {name}")
        elif name not in a:
            if ARCH_EXTRA.get(name) != b[name][:4]:
                problems.append(f"not in the .deb: {name}")
        elif a[name][:4] != b[name][:4] and not (name in edited and a[name][:2] + a[name][3:4] == b[name][:2] + b[name][3:4]):
            problems.append(f"differs: {name}: .deb {a[name][:4]} vs package {b[name][:4]}")
    for name, e in b.items():
        if e[4:] != (0, 0, "root", "root"):
            problems.append(f"not root:root: {name} {e[4:]}")
    if problems:
        raise Fail("payload check failed:\n  " + "\n  ".join(problems[:50]))
    return len(b)


def main(argv):
    cmd, args = argv[1], argv[2:]
    if cmd == "pkgver":
        print(pkgver(args[0]))
    elif cmd == "depends":
        note = lambda msg: print(f"package-arch: {msg}", file=sys.stderr)
        print("\n".join(depends(args[0], args[1] if len(args) > 1 else None, note)))
    elif cmd == "bundled":
        print("\n".join(sorted(bundled_names(args[0]))))
    elif cmd == "escape":
        print(escape(args[0]))
    elif cmd == "mtree":
        write_mtree(args[0], args[1], args[2])
    elif cmd == "size":
        print(installed_size(args[0]))
    elif cmd == "licenses":
        print("\n".join(licenses(args[0])))
    elif cmd == "verify":
        print(verify(args[0], args[1], set(args[2:])))
    else:
        raise Fail(f"unknown subcommand {cmd}")


if __name__ == "__main__":
    try:
        main(sys.argv)
    except Fail as e:
        print(f"package-arch: {e}", file=sys.stderr)
        sys.exit(2)
