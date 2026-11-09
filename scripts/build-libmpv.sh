#!/usr/bin/env bash
# Usage: scripts/build-libmpv.sh <platform> <workdir> <outdir>
#
# Builds the libmpv zapper ships: mpv and FFmpeg under the LGPL alone (mpv -Dgpl=false, FFmpeg
# without --enable-gpl, --enable-version3 or --enable-nonfree), from the sources pinned in
# packaging/libmpv/sources.txt, each checked against its SHA-256 before it is unpacked.
# .github/workflows/libmpv.yml runs it once per platform and publishes the archives; nothing
# else needs to (`just fetch-libmpv` downloads what it published).
#
#   platform  linux-x86_64    native, on Linux
#             windows-x86_64  cross-compiled on Linux with MinGW-w64 (Debian's -posix gcc), or
#                             native in an MSYS2 MINGW64 shell on Windows
#             macos-arm64     native, on an Apple Silicon Mac
#             macos-x86_64    native, on an Intel Mac
#   workdir   sources/ (the downloads, kept between runs), and the build trees and install
#             prefix, removed once the archive is made unless LIBMPV_KEEP_BUILD=1
#   outdir    receives libmpv-<platform>.tar.xz (.zip for Windows), and sources/ with the
#             exact tarballs this platform was built from
#
# FFmpeg's libraries are shared, because two programs use them: libmpv, and zapper-server's
# remuxer (crates/zapper/src/recording/remux.c). Every other dependency (libplacebo, libass, dav1d,
# the text stack, the shader compilers) is a static library linked into the one that needs it,
# with every symbol but that library's API hidden, so nothing can collide with a same-named
# library the system has already loaded into the process (GTK's FreeType). What the libraries
# still need from the system is ordinary, ABI-stable desktop libraries; docs/licensing.md lists
# them.
#
# The archive holds runtime/ (exactly the libraries that ship: libmpv and FFmpeg's), link/ (what
# a link against them needs: the unversioned names, or on Windows the export lists MSVC's import
# libraries are made from), include/ (mpv's and FFmpeg's headers), the notices and BUILDINFO.txt.
#
# LIBMPV_JOBS sets the build's parallelism (default 4: builds here share the machine with
# video playback). LIBMPV_SOURCE_URL is where the archive's notices say the sources are
# published (the workflow passes the release's address). LIBMPV_ARCHIVE_ONLY=1 makes only the
# archive again, from what a run with LIBMPV_KEEP_BUILD=1 left.
set -euo pipefail

platform=${1:?usage: build-libmpv.sh <platform> <workdir> <outdir>}
work=${2:?usage: build-libmpv.sh <platform> <workdir> <outdir>}
out=${3:?usage: build-libmpv.sh <platform> <workdir> <outdir>}
jobs=${LIBMPV_JOBS:-4}
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/.." && pwd)
sources_txt=$repo/packaging/libmpv/sources.txt

case $platform in
  linux-x86_64) family=linux arch=x86_64 ;;
  windows-x86_64) family=windows arch=x86_64 ;;
  macos-arm64) family=macos arch=arm64 ;;
  macos-x86_64) family=macos arch=x86_64 ;;
  *) echo "build-libmpv: unknown platform $platform" >&2; exit 2 ;;
esac

mkdir -p "$work" "$out"
work=$(cd "$work" && pwd)
out=$(cd "$out" && pwd)
dl=$work/sources
src=$work/src-$platform
build=$work/build-$platform
prefix=$work/prefix-$platform
tools=$work/tools-$(uname -s)-$(uname -m)
stage_name=libmpv-$platform
stage=$work/$stage_name
mkdir -p "$dl"
archive_only=${LIBMPV_ARCHIVE_ONLY:-0}
if [ "$archive_only" = 1 ]; then
  [ -d "$prefix/include/mpv" ] || { echo "build-libmpv: nothing built in $prefix to archive" >&2; exit 1; }
  rm -rf "$stage"
else
  rm -rf "$src" "$build" "$prefix" "$stage"
fi
mkdir -p "$src" "$build" "$prefix/lib/pkgconfig" "$prefix/include" "$stage"

log() { printf '\n\033[1m== build-libmpv %s: %s\033[0m\n' "$platform" "$*"; }
die() { echo "build-libmpv: $*" >&2; exit 1; }

sha256() {
  if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

# --- sources -----------------------------------------------------------------------------------

# The sources this platform builds from, as parallel arrays: name, version, file, SHA-256, URL.
s_name=() s_version=() s_file=() s_sum=() s_url=()
while read -r name version platforms sum url; do
  case $name in ''|'#'*) continue ;; esac
  case ",$platforms," in *",all,"*|*",$family,"*) ;; *) continue ;; esac
  case ${url##*/} in
    *.tar.gz) ext=tar.gz ;; *.tar.xz) ext=tar.xz ;; *.tar.bz2) ext=tar.bz2 ;;
    *) die "$name: unknown archive type: $url" ;;
  esac
  s_name+=("$name") s_version+=("$version") s_file+=("$name-$version.$ext") s_sum+=("$sum") s_url+=("$url")
done < "$sources_txt"

index_of() {
  local i
  for i in "${!s_name[@]}"; do [ "${s_name[$i]}" = "$1" ] && { echo "$i"; return; }; done
  die "$1 isn't in sources.txt for $family"
}
version_of() { local i; i=$(index_of "$1"); echo "${s_version[$i]}"; }
file_of() { local i; i=$(index_of "$1"); echo "${s_file[$i]}"; }

log "sources"
for i in "${!s_name[@]}"; do
  f=$dl/${s_file[$i]}
  if [ ! -f "$f" ] || [ "$(sha256 "$f")" != "${s_sum[$i]}" ]; then
    echo "fetching ${s_file[$i]}"
    curl -fsSL --retry 4 --retry-all-errors -o "$f.part" "${s_url[$i]}"
    mv "$f.part" "$f"
  fi
  got=$(sha256 "$f")
  [ "$got" = "${s_sum[$i]}" ] || die "${s_file[$i]}: SHA-256 is $got, sources.txt says ${s_sum[$i]}"
  echo "ok  $got  ${s_file[$i]}"
done

# unpack <name> [dir]: the source's tarball, fresh, into $src/<name> (or the dir given).
unpack() {
  local dir=${2:-$src/$1}
  rm -rf "$dir"; mkdir -p "$dir"
  tar -xf "$dl/$(file_of "$1")" -C "$dir" --strip-components=1
}

# --- toolchain ---------------------------------------------------------------------------------

export PKG_CONFIG_PATH=$prefix/lib/pkgconfig
cross=()       # meson's --cross-file, for Windows
cmake_args=(-GNinja -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$prefix" -DCMAKE_INSTALL_LIBDIR=lib
  -DCMAKE_PREFIX_PATH="$prefix" -DCMAKE_POSITION_INDEPENDENT_CODE=ON)
ffmpeg_target=()
case $family in
  linux)
    export CC=${CC:-gcc} CXX=${CXX:-g++}
    export CFLAGS="-O2 -fPIC" CXXFLAGS="-O2 -fPIC"
    cxx_runtime=-lstdc++
    ;;
  windows)
    triple=x86_64-w64-mingw32
    case $(uname -s) in
      MINGW64*)
        # Native, in MSYS2's MINGW64 environment: its gcc is already the posix-threads one.
        native_windows=1 tp=""
        export CC=${MINGW_CC:-gcc} CXX=${MINGW_CXX:-g++} BUILD_CC=gcc
        ;;
      *)
        # Debian's and Ubuntu's MinGW-w64 come in a win32-threads and a posix-threads flavour;
        # the C++ in shaderc and glslang needs std::thread, which is the posix one.
        native_windows=0 tp=$triple-
        export CC=${MINGW_CC:-$triple-gcc-posix} CXX=${MINGW_CXX:-$triple-g++-posix}
        ;;
    esac
    export AR=${tp}ar RANLIB=${tp}ranlib STRIP=${tp}strip NM=${tp}nm
    export CFLAGS="-O2" CXXFLAGS="-O2"
    # Only this build's own prefix: never the build machine's .pc files.
    export PKG_CONFIG_LIBDIR=$prefix/lib/pkgconfig
    # The C++ runtime by path: meson turns a -lstdc++ into libstdc++.dll.a, the import library,
    # whatever -static says, and the DLL would need libstdc++-6.dll beside it.
    cxx_runtime=$("$CXX" -print-file-name=libstdc++.a)
    [ -f "$cxx_runtime" ] || die "$CXX has no static libstdc++.a"
    export LIBMPV_STATIC_CXX=$cxx_runtime
    if [ "$native_windows" = 0 ]; then
    cat > "$build/cross.ini" <<EOF
[binaries]
c = '$CC'
cpp = '$CXX'
ar = '$AR'
strip = '$STRIP'
windres = '$triple-windres'
dlltool = '$triple-dlltool'
pkg-config = 'pkg-config'
nasm = 'nasm'
[built-in options]
pkg_config_path = '$prefix/lib/pkgconfig'
[host_machine]
system = 'windows'
cpu_family = 'x86_64'
cpu = 'x86_64'
endian = 'little'
EOF
    cross=(--cross-file "$build/cross.ini")
    cmake_args+=(-DCMAKE_SYSTEM_NAME=Windows -DCMAKE_SYSTEM_PROCESSOR=x86_64
      -DCMAKE_C_COMPILER="$CC" -DCMAKE_CXX_COMPILER="$CXX" -DCMAKE_RC_COMPILER="$triple-windres"
      -DCMAKE_FIND_ROOT_PATH="$prefix" -DCMAKE_FIND_ROOT_PATH_MODE_PROGRAM=NEVER
      -DCMAKE_FIND_ROOT_PATH_MODE_LIBRARY=ONLY -DCMAKE_FIND_ROOT_PATH_MODE_INCLUDE=ONLY)
    ffmpeg_target=(--target-os=mingw32 --arch=x86_64 --enable-cross-compile --cross-prefix=$triple-)
    else
      ffmpeg_target=(--target-os=mingw32 --arch=x86_64)
    fi
    ;;
  macos)
    [ "$(uname -s)" = Darwin ] || die "$platform builds on macOS"
    [ "$(uname -m)" = "$arch" ] || die "$platform builds on an $arch Mac (this one is $(uname -m))"
    export MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-11.0}
    export CC=clang CXX=clang++
    export CFLAGS="-O2" CXXFLAGS="-O2"
    cxx_runtime=-lc++
    cmake_args+=(-DCMAKE_OSX_DEPLOYMENT_TARGET="$MACOSX_DEPLOYMENT_TARGET")
    ;;
esac

# Static, position-independent, release: the shape every dependency is built in.
meson_common=(--prefix="$prefix" --libdir=lib --buildtype=release --default-library=static
  -Db_staticpic=true -Db_ndebug=true --wrap-mode=nodownload "${cross[@]}")

meson_build() { # meson_build <name> <source dir> [meson options...]
  local name=$1 dir=$2; shift 2
  log "$name"
  meson setup "$build/$name" "$dir" "${meson_common[@]}" "$@"
  ninja -C "$build/$name" -j "$jobs"
  meson install -C "$build/$name" --quiet
}

cmake_build() { # cmake_build <name> <source dir> [cmake options...]
  local name=$1 dir=$2; shift 2
  log "$name"
  cmake -S "$dir" -B "$build/$name" "${cmake_args[@]}" "$@"
  ninja -C "$build/$name" -j "$jobs"
  cmake --install "$build/$name" >/dev/null
}

# A static library's .pc file names what it needs only under Libs.private and
# Requires.private, which a plain `pkg-config --libs` leaves out. Folding them into Libs and
# Requires gives every consumer (FFmpeg's configure, mpv's meson) the full link line without
# asking pkg-config for --static, which would drag in the system libraries' private
# dependencies too. `extra` is appended to Libs (the C++ runtime, for a library written in C++
# that a C link will pull in).
flatten_pc() { # flatten_pc <name.pc> [extra libs]
  local pc=$prefix/lib/pkgconfig/$1 extra=${2:-}
  [ -f "$pc" ] || die "flatten_pc: no $pc"
  python3 -I - "$pc" "$extra" <<'PY'
import os, re, sys
path, extra = sys.argv[1], sys.argv[2]
fields, lines = {}, []
for line in open(path).read().splitlines():
    m = re.match(r'^(Libs|Libs\.private|Requires|Requires\.private):\s*(.*)$', line)
    if m:
        fields[m.group(1)] = m.group(2).strip()
    else:
        lines.append(line)
libs = ' '.join(x for x in (fields.get('Libs', ''), fields.get('Libs.private', ''), extra) if x)
static_cxx = os.environ.get('LIBMPV_STATIC_CXX')
if static_cxx:
    libs = ' '.join(static_cxx if t == '-lstdc++' else t for t in libs.split())
reqs = ', '.join(x for x in (fields.get('Requires', ''), fields.get('Requires.private', '')) if x)
lines.append('Libs: ' + libs)
if reqs:
    lines.append('Requires: ' + reqs)
open(path, 'w').write('\n'.join(lines) + '\n')
PY
}

# nasm: dav1d's and FFmpeg's x86 assembly. Built from the pinned source rather than taken from
# the system, so every platform assembles with the same one. Not needed on arm64.
if [ "$arch" = x86_64 ]; then
  if [ ! -x "$tools/bin/nasm" ] || [ "$("$tools/bin/nasm" -v | awk '{print $3}')" != "$(version_of nasm)" ]; then
    log "nasm (build tool)"
    unpack nasm
    (cd "$src/nasm" && CC=${BUILD_CC:-cc} CFLAGS=-O2 ./configure --prefix="$tools" >/dev/null && make -j "$jobs" >/dev/null && make install >/dev/null)
  fi
  export PATH=$tools/bin:$PATH
  nasm -v
fi

if [ "$archive_only" != 1 ]; then

# --- dependencies --------------------------------------------------------------------------------

if [ "$family" = windows ]; then
  log "zlib"
  unpack zlib
  # zlib's MinGW makefile, static only; it ships no .pc for that, so one is written here.
  make -C "$src/zlib" -f win32/Makefile.gcc -j "$jobs" CC="$CC" AR="$AR" RC=${tp}windres \
    STRIP="$STRIP" libz.a >/dev/null
  install -m644 "$src/zlib/libz.a" "$prefix/lib/"
  install -m644 "$src/zlib/zlib.h" "$src/zlib/zconf.h" "$prefix/include/"
  printf 'prefix=%s\nlibdir=${prefix}/lib\nincludedir=${prefix}/include\n\nName: zlib\nDescription: zlib compression library\nVersion: %s\nLibs: -L${libdir} -lz\nCflags: -I${includedir}\n' \
    "$prefix" "$(version_of zlib)" > "$prefix/lib/pkgconfig/zlib.pc"
fi

unpack dav1d
meson_build dav1d "$src/dav1d" -Denable_tools=false -Denable_tests=false -Denable_examples=false -Denable_docs=false
flatten_pc dav1d.pc

if [ "$family" = linux ]; then
  # Its build reads hwdata's pnp.ids to name monitor makers; mpv never asks for the names, so
  # it gets an empty table rather than compiling in a GPL-or-XFree86 data file.
  mkdir -p "$build/hwdata"
  : > "$build/hwdata/pnp.ids"
  printf 'pkgdatadir=%s\n\nName: hwdata\nDescription: empty\nVersion: 0\n' "$build/hwdata" > "$build/hwdata/hwdata.pc"
  unpack libdisplay-info
  PKG_CONFIG_PATH=$PKG_CONFIG_PATH:$build/hwdata meson_build libdisplay-info "$src/libdisplay-info"
  flatten_pc libdisplay-info.pc
fi

if [ "$family" != linux ]; then
  unpack freetype
  zlib_mode=system
  meson_build freetype "$src/freetype" -Dbrotli=disabled -Dbzip2=disabled -Dharfbuzz=disabled \
    -Dpng=disabled -Dzlib=$zlib_mode -Dtests=disabled
  flatten_pc freetype2.pc

  unpack fribidi
  meson_build fribidi "$src/fribidi" -Ddocs=false -Dbin=false -Dtests=false -Ddeprecated=false
  flatten_pc fribidi.pc
  # A static FriBidi's headers must not ask for dllimport.
  [ "$family" = windows ] && sed -i 's/^Cflags: \(.*\)$/Cflags: \1 -DFRIBIDI_LIB_STATIC/' "$prefix/lib/pkgconfig/fribidi.pc"

  unpack harfbuzz
  meson_build harfbuzz "$src/harfbuzz" -Dauto_features=disabled -Dfreetype=enabled \
    -Dtests=disabled -Ddocs=disabled -Dutilities=disabled -Dbenchmark=disabled -Dintrospection=disabled \
    -Dsubset=disabled -Draster=disabled -Dvector=disabled -Dgpu=disabled -Dgpu_demo=disabled
  flatten_pc harfbuzz.pc "$cxx_runtime"
fi

unpack libass
# libass takes iconv whenever its build finds one, and has no option to say no. On Windows that
# depends on the build machine (MSYS2 has a libiconv DLL, MinGW-w64 on Linux none), so it is
# never looked for there, in the one line of its meson.build that would find it.
if [ "$family" = windows ]; then
  sed -i "s/^iconv_dep = dependency('iconv', required: false)$/iconv_dep = dependency('', required: false)/" \
    "$src/libass/meson.build"
  grep -q "^iconv_dep = dependency('', required: false)$" "$src/libass/meson.build" ||
    die "libass's meson.build no longer looks for iconv the way this expects"
fi
libass_fonts=(-Dfontconfig=disabled -Ddirectwrite=disabled -Dcoretext=disabled)
case $family in
  linux) libass_fonts=(-Dfontconfig=enabled -Ddirectwrite=disabled -Dcoretext=disabled) ;;
  windows) libass_fonts=(-Dfontconfig=disabled -Ddirectwrite=enabled -Dcoretext=disabled) ;;
  macos) libass_fonts=(-Dfontconfig=disabled -Ddirectwrite=disabled -Dcoretext=enabled) ;;
esac
libass_asm=enabled; [ "$arch" = x86_64 ] || libass_asm=auto
meson_build libass "$src/libass" "${libass_fonts[@]}" -Drequire-system-font-provider=true \
  -Dlibunibreak=disabled -Dasm=$libass_asm -Dtest=disabled -Dcompare=disabled -Dprofile=disabled \
  -Dfuzz=disabled -Dcheckasm=disabled
flatten_pc libass.pc

if [ "$family" = windows ]; then
  # Direct3D 11 output: mpv and libplacebo translate their GLSL to HLSL through shaderc (with
  # glslang, SPIRV-Tools and SPIRV-Headers) and SPIRV-Cross.
  unpack shaderc
  unpack glslang "$src/shaderc/third_party/glslang"
  unpack spirv-tools "$src/shaderc/third_party/spirv-tools"
  unpack spirv-headers "$src/shaderc/third_party/spirv-headers"
  cmake_build shaderc "$src/shaderc" -DSHADERC_SKIP_TESTS=ON -DSHADERC_SKIP_EXAMPLES=ON \
    -DSHADERC_SKIP_COPYRIGHT_CHECK=ON -DSHADERC_SKIP_INSTALL=OFF -DBUILD_SHARED_LIBS=OFF \
    -DSPIRV_SKIP_EXECUTABLES=ON -DSPIRV_SKIP_TESTS=ON -DSPIRV_TOOLS_BUILD_STATIC=ON \
    -DENABLE_GLSLANG_BINARIES=OFF -DENABLE_CTEST=OFF -DSKIP_GLSLANG_INSTALL=ON \
    -DSKIP_SPIRV_TOOLS_INSTALL=ON -DSPIRV_HEADERS_SKIP_INSTALL=ON
  # shaderc_combined is the one archive with glslang and SPIRV-Tools inside; mpv and
  # libplacebo ask for `shaderc`.
  [ -f "$prefix/lib/libshaderc_combined.a" ] || die "shaderc: no libshaderc_combined.a"
  printf 'prefix=%s\nlibdir=${prefix}/lib\nincludedir=${prefix}/include\n\nName: shaderc\nDescription: shaderc, static (shaderc_combined)\nVersion: %s\nLibs: -L${libdir} -lshaderc_combined %s\nCflags: -I${includedir}\n' \
    "$prefix" "$(version_of shaderc)" "$cxx_runtime" > "$prefix/lib/pkgconfig/shaderc.pc"

  unpack spirv-cross
  cmake_build spirv-cross "$src/spirv-cross" -DSPIRV_CROSS_SHARED=OFF -DSPIRV_CROSS_STATIC=ON \
    -DSPIRV_CROSS_CLI=OFF -DSPIRV_CROSS_ENABLE_TESTS=OFF -DSPIRV_CROSS_ENABLE_C_API=ON \
    -DSPIRV_CROSS_SKIP_INSTALL=OFF
  # Both ask for `spirv-cross-c-shared`, the name of the shared build's .pc; this is the static
  # one under that name, every module in link order.
  sc_libs=""
  for m in c hlsl glsl msl cpp reflect util core; do
    [ -f "$prefix/lib/libspirv-cross-$m.a" ] && sc_libs="$sc_libs -lspirv-cross-$m"
  done
  printf 'prefix=%s\nlibdir=${prefix}/lib\nincludedir=${prefix}/include\n\nName: spirv-cross-c-shared\nDescription: SPIRV-Cross C API, static\nVersion: 0.%s\nLibs: -L${libdir}%s %s\nCflags: -I${includedir}/spirv_cross\n' \
    "$prefix" "$(sed -n 's/^Version: *0\.//p' "$prefix/lib/pkgconfig/spirv-cross-c.pc")" "$sc_libs" "$cxx_runtime" \
    > "$prefix/lib/pkgconfig/spirv-cross-c-shared.pc"
fi

unpack libplacebo
unpack vulkan-headers "$src/libplacebo/3rdparty/Vulkan-Headers"
unpack glad "$src/libplacebo/3rdparty/glad"
unpack jinja "$src/libplacebo/3rdparty/jinja"
unpack markupsafe "$src/libplacebo/3rdparty/markupsafe"
unpack fast_float "$src/libplacebo/3rdparty/fast_float"
placebo=(-Dopengl=enabled -Dgl-proc-addr=disabled -Dvulkan=disabled -Dvk-proc-addr=disabled
  -Dglslang=disabled -Dlcms=disabled -Ddovi=enabled -Dlibdovi=disabled -Dunwind=disabled
  -Dxxhash=disabled -Ddemos=false -Dtests=false -Dbench=false -Dfuzz=false)
if [ "$family" = windows ]; then
  placebo+=(-Dd3d11=enabled -Dshaderc=enabled)
  # libplacebo compiles itself with -DPL_EXPORT, static or not, which on Windows marks its API
  # dllexport: libmpv-2.dll would export all of it. A header forced in after the command line's
  # -D takes that back (PL_STATIC: no import or export attribute), without touching its source.
  printf '#undef PL_EXPORT\n#define PL_STATIC 1\n' > "$build/placebo-static.h"
  # MSYS2 translates paths in arguments, not inside CFLAGS: its native gcc needs C:/....
  inc=$build/placebo-static.h
  [ "$native_windows" = 1 ] && inc=$(cygpath -m "$inc")
  CFLAGS="$CFLAGS -include $inc" CXXFLAGS="$CXXFLAGS -include $inc" \
    meson_build libplacebo "$src/libplacebo" "${placebo[@]}"
else
  placebo+=(-Dd3d11=disabled -Dshaderc=disabled)
  meson_build libplacebo "$src/libplacebo" "${placebo[@]}"
fi
flatten_pc libplacebo.pc "$cxx_runtime"

# --- FFmpeg ----------------------------------------------------------------------------------------

unpack ffmpeg
# LGPL-2.1-or-later: no --enable-gpl, --enable-version3 or --enable-nonfree, and FFmpeg's
# configure refuses any GPL-only component without them (it lists its GPL filters as depending
# on "gpl"; the compressor's speechnorm and alimiter are not among them).
#
# --disable-autodetect: nothing comes in because the build machine happens to have it; what is
# wanted is named. The decoders and demuxers are what live TV and VOD libraries carry (MPEG-TS,
# HLS, MP4, Matroska, AVI; H.264, HEVC, MPEG-2, AV1, AAC, AC-3/E-AC-3, MP2/MP3, Opus, DTS; DVB
# and CEA-608 subtitles). DVB teletext needs libzvbi, part of which is GPL-2.0-only, so it is
# not here. Protocols, parsers, bitstream filters and filters are left at FFmpeg's full set,
# which here means its LGPL set. Encoders: PNG and MJPEG for mpv's screenshots, and FFmpeg's own
# AAC encoder (LGPL), which zapper-server uses to turn sound a browser can't play (AC-3, MP2)
# into AAC while a browser watches (crates/zapper/src/recording/livemux.c).
#
# The names are configure's (allcodecs.c's ff_<name>_decoder), not the codecs' own: MP4's text
# subtitles are movtext, though FFmpeg calls the codec mov_text. A name configure doesn't know is
# dropped without an error, so check a new one against allcodecs.c and allformats.c.
decoders=(
  h264 hevc mpeg1video mpeg2video mpeg4 msmpeg4v1 msmpeg4v2 msmpeg4v3 h263 flv vc1 wmv1 wmv2 wmv3
  vp8 vp9 av1 libdav1d mjpeg png rawvideo wrapped_avframe
  aac aac_fixed aac_latm ac3 ac3_fixed eac3 dca truehd mlp mp1 mp1float mp2 mp2float mp3 mp3float
  opus vorbis flac alac wmav1 wmav2 wmapro s302m 'pcm_*'
  dvbsub dvdsub pgssub ccaption subrip srt ass ssa webvtt movtext text xsub
)
demuxers=(
  mpegts mpegtsraw hls mov matroska avi asf flv live_flv mpegps mpegvideo h264 hevc m4v
  aac ac3 eac3 dts truehd mp3 flac ogg wav w64 mpjpeg rtsp rtp sdp webvtt srt ass
)
ffmpeg_args=(
  --prefix="$prefix" --pkg-config=pkg-config --pkg-config-flags=
  --cc="$CC" --cxx="$CXX"
  --extra-cflags="-I$prefix/include" --extra-ldflags="-L$prefix/lib"
  --disable-static --enable-shared
  --disable-programs --disable-doc --disable-debug --disable-autodetect
  --enable-libdav1d
  --disable-encoders --enable-encoder=png,mjpeg,aac
  --disable-muxers --enable-muxer=spdif,mpegts,matroska,mp4,mov
  --disable-decoders --enable-decoder="$(IFS=,; echo "${decoders[*]}")"
  --disable-demuxers --enable-demuxer="$(IFS=,; echo "${demuxers[*]}")"
  --disable-indevs --enable-indev=lavfi --disable-outdevs
  "${ffmpeg_target[@]}"
)
case $family in
  # OpenSSL 3 (Apache-2.0) for https: FFmpeg's configure accepts it in an LGPL build, and every
  # distribution zapper runs on has libssl.so.3.
  linux) ffmpeg_args+=(--enable-pthreads --enable-zlib --enable-iconv --enable-openssl --enable-vaapi --enable-libdrm) ;;
  # Statically linked runtime (libgcc, and libstdc++ for the bits of C++): no MinGW DLLs beside
  # FFmpeg's.
  windows) ffmpeg_args+=(--enable-w32threads --enable-zlib --enable-schannel --enable-d3d11va --enable-dxva2
             --extra-ldflags="-static -static-libgcc -static-libstdc++") ;;
  # iconv is a library of its own on macOS, not part of libc, and FFmpeg's configure finds the
  # header without adding the library to the link.
  macos) ffmpeg_args+=(--enable-pthreads --enable-zlib --enable-iconv --enable-securetransport --enable-videotoolbox
           --extra-libs=-liconv) ;;
esac
log "ffmpeg"
mkdir -p "$build/ffmpeg"
(cd "$build/ffmpeg" && "$src/ffmpeg/configure" "${ffmpeg_args[@]}") > "$build/ffmpeg-configure.log" 2>&1 ||
  { tail -40 "$build/ffmpeg-configure.log"; tail -60 "$build/ffmpeg/ffbuild/config.log" 2>/dev/null; die "FFmpeg's configure failed"; }
grep -E '^License:' "$build/ffmpeg-configure.log"
grep -q '^License: LGPL version 2.1 or later' "$build/ffmpeg-configure.log" || die "FFmpeg isn't configured LGPL-2.1-or-later"
for f in speechnorm alimiter; do
  grep -q "^#define CONFIG_$(echo $f | tr a-z A-Z)_FILTER 1" "$build/ffmpeg/config_components.h" ||
    die "FFmpeg is configured without the $f filter, which the compressor needs"
done
grep -q "^#define CONFIG_AAC_ENCODER 1" "$build/ffmpeg/config_components.h" ||
  die "FFmpeg is configured without its AAC encoder, which zapper-server's browser streams need"
make -C "$build/ffmpeg" -j "$jobs" >/dev/null
make -C "$build/ffmpeg" install >/dev/null

# --- mpv ---------------------------------------------------------------------------------------------

unpack mpv
# Everything off but what is named (-Dauto_features=disabled; Lua's option is a choice, not a
# feature, so it is turned off by name), and -Dgpl=false: mpv's own GPL-only parts (X11 among
# them) are left out and the library is LGPL-2.1-or-later.
mpv_opts=(-Dgpl=false -Dlibmpv=true -Dcplayer=false -Dtests=false -Dfuzzers=false -Dlua=disabled
  -Dbuild-date=false -Dauto_features=disabled -Dmanpage-build=disabled -Dhtml-build=disabled
  -Dpdf-build=disabled -Dgl=enabled -Dplain-gl=enabled -Dzlib=enabled -Dlibavdevice=enabled
  -Dvector=enabled)
mpv_ldflags=""
case $family in
  linux)
    # Wayland and DRM for VA-API (interop on Wayland, copy-back anywhere), the three audio
    # outputs a desktop has, EGL for the GL interop.
    mpv_opts+=(-Diconv=enabled -Dalsa=enabled -Dpulse=enabled -Dpipewire=enabled -Dwayland=enabled
      -Degl=enabled -Degl-wayland=enabled -Ddrm=enabled -Dvaapi=enabled -Dvaapi-drm=enabled
      -Dvaapi-wayland=enabled)
    # The static dependencies' symbols stay inside libmpv.
    mpv_ldflags="-Wl,--exclude-libs,ALL"
    ;;
  windows)
    mpv_opts+=(-Dwin32-threads=enabled -Dwasapi=enabled -Dd3d11=enabled -Dshaderc=enabled
      -Dspirv-cross=enabled -Dd3d-hwaccel=enabled -Dd3d9-hwaccel=enabled -Dgl-win32=enabled)
    # One DLL with no MinGW runtime DLLs beside it.
    mpv_ldflags="-static -static-libgcc -static-libstdc++ -Wl,--exclude-libs,ALL"
    ;;
  macos)
    mpv_opts+=(-Diconv=enabled -Dcocoa=enabled -Dswift-build=enabled -Dcoreaudio=enabled
      -Davfoundation=enabled -Dgl-cocoa=enabled -Dvideotoolbox-gl=enabled
      -Dmacos-10-15-4-features=enabled -Dmacos-11-features=enabled -Dmacos-11-3-features=enabled
      -Dmacos-12-features=enabled
      -Dswift-flags="-target $arch-apple-macos$MACOSX_DEPLOYMENT_TARGET")
    # mpv's API alone is exported. Its Swift parts are compiled by mpv's own rule, which knows
    # nothing of MACOSX_DEPLOYMENT_TARGET: without the target above they are built for the
    # build machine's macOS and the library needs that version or newer.
    mpv_ldflags="-Wl,-exported_symbol,_mpv_*"
    ;;
esac
log "mpv"
meson setup "$build/mpv" "$src/mpv" --prefix="$prefix" --libdir=lib --buildtype=release \
  --default-library=shared -Db_ndebug=true --wrap-mode=nodownload "${cross[@]}" \
  -Dc_link_args="$mpv_ldflags" "${mpv_opts[@]}"
ninja -C "$build/mpv" -j "$jobs"
meson install -C "$build/mpv" --quiet

fi # archive_only

# --- the archive -----------------------------------------------------------------------------------

log "archive"
mkdir -p "$stage/runtime" "$stage/link" "$stage/include"
for h in mpv libavcodec libavformat libavutil libavfilter libavdevice libswscale libswresample; do
  cp -R "$prefix/include/$h" "$stage/include/"
done
ffmpeg_libs="avutil avcodec avformat avfilter avdevice swscale swresample"
case $family in
  linux)
    # Each library under its soname, finding the others beside it through a RUNPATH of $ORIGIN,
    # and the unversioned name a link asks for in link/.
    for f in "$prefix"/lib/libmpv.so.2.* $(for l in $ffmpeg_libs; do ls "$prefix"/lib/lib$l.so.*.*.*; done); do
      soname=$(readelf -d "$f" | sed -n 's/.*(SONAME).*\[\(.*\)\]/\1/p')
      [ -n "$soname" ] || die "$f has no SONAME"
      cp "$f" "$stage/runtime/$soname"
      strip --strip-unneeded "$stage/runtime/$soname"
      patchelf --set-rpath '$ORIGIN' "$stage/runtime/$soname"
      ln -s "../runtime/$soname" "$stage/link/${soname%%.so.*}.so"
    done
    lib=$stage/runtime/libmpv.so.2
    exported=$(nm -D --defined-only "$lib" | awk '{print $3}' | grep -v '^mpv_' || true)
    [ -z "$exported" ] || die "libmpv.so.2 exports more than mpv's API: $(echo $exported | head -c 300)"
    # FFmpeg's libraries export their own API (their version scripts), nothing of what is linked
    # into them.
    leaked=$(for f in "$stage"/runtime/lib[as][vw]*; do nm -D --defined-only "$f" | awk '{print $3}'; done |
      grep -E '^(dav1d_|pl_|ass_|FT_|hb_|fribidi_)' || true)
    [ -z "$leaked" ] || die "FFmpeg's libraries export what is linked into them: $(echo $leaked | head -c 300)"
    # What they may take from the system: the libraries the .deb's and .rpm's Depends name
    # (crates/zapper/tauri.conf.json), besides each other. Anything else is a dependency that crept in.
    allowed=" libc.so.6 libm.so.6 libstdc++.so.6 libgcc_s.so.1 libz.so.1 libssl.so.3 libcrypto.so.3
      libva.so.2 libva-drm.so.2 libva-wayland.so.2 libdrm.so.2 libEGL.so.1 libwayland-client.so.0
      libwayland-cursor.so.0 libwayland-egl.so.1 libxkbcommon.so.0 libasound.so.2 libpulse.so.0
      libpipewire-0.3.so.0 libfreetype.so.6 libharfbuzz.so.0 libfribidi.so.0 libfontconfig.so.1
      $(ls "$stage/runtime") "
    allowed=" $(echo $allowed) "
    for f in "$stage"/runtime/*; do
      for needed in $(readelf -d "$f" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p'); do
        case $allowed in *" $needed "*) ;; *) die "${f##*/} needs $needed, which isn't on the list" ;; esac
      done
    done
    ;;
  windows)
    for f in "$prefix"/bin/libmpv-2.dll $(for l in $ffmpeg_libs; do ls "$prefix"/bin/$l-*.dll; done); do
      n=${f##*/}
      cp "$f" "$stage/runtime/"
      $STRIP --strip-unneeded "$stage/runtime/$n"
      # The export list MSVC's lib.exe turns into an import library (tools/fetch-libmpv.ps1):
      # mpv.def for libmpv-2.dll (libmpv2-sys links `mpv`), avformat.def for avformat-62.dll.
      base=${n%%-*}
      { echo "LIBRARY $n"; echo EXPORTS
        ${tp}objdump -p "$stage/runtime/$n" |
          awk '/\[Ordinal\/Name Pointer\] Table/ {t=1; next} t && /^[ \t]*\[/ {print $NF} t && /^$/ {t=0}'
      } > "$stage/link/${base#lib}.def"
      [ "$(grep -vcE '^(LIBRARY|EXPORTS)' "$stage/link/${base#lib}.def")" -gt 0 ] || die "${base#lib}.def lists no exports"
    done
    cp "$prefix"/lib/lib*.dll.a "$stage/link/"
    grep -qvE '^(LIBRARY|EXPORTS|mpv_)' "$stage/link/mpv.def" &&
      die "libmpv-2.dll exports more than mpv's API: $(grep -vE '^(LIBRARY|EXPORTS|mpv_)' "$stage/link/mpv.def" | head -5 | tr '\n' ' ')"
    leaked=$(cat "$stage"/link/*.def | grep -E '^(dav1d_|pl_|ass_|FT_|hb_|fribidi_|shaderc_|spvc_)' || true)
    [ -z "$leaked" ] || die "the DLLs export what is linked into them: $(echo $leaked | head -c 300)"
    [ "$(grep -c '^mpv_' "$stage/link/mpv.def")" -gt 20 ] || die "mpv.def lists too few of mpv's functions"
    for f in "$stage"/runtime/*.dll; do
      deps=$(${tp}objdump -p "$f" | awk '/DLL Name:/ {print $3}' | sort -u)
      echo "${f##*/} imports: $(echo $deps)"
      # Windows' own DLLs, and each other: anything else (a MinGW runtime, a library the build
      # machine happened to have) would be missing on the user's.
      for d in $deps; do
        [ -f "$stage/runtime/$d" ] && continue
        case $(echo "$d" | tr A-Z a-z) in
          lib*|zlib*|*gcc*|*pthread*|*stdc++*) die "${f##*/} needs $d, which is not Windows' and not bundled" ;;
        esac
      done
    done
    lib=$stage/runtime/libmpv-2.dll
    ;;
  macos)
    # Each library as @rpath/<its install name>, finding the others beside it (@loader_path).
    for f in "$prefix"/lib/libmpv.2.dylib $(for l in $ffmpeg_libs; do ls "$prefix"/lib/lib$l.[0-9]*.dylib; done); do
      n=$(basename "$(otool -D "$f" | tail -1)")
      [ -f "$stage/runtime/$n" ] && continue
      cp -L "$f" "$stage/runtime/$n"
      chmod u+w "$stage/runtime/$n"
      install_name_tool -id "@rpath/$n" "$stage/runtime/$n"
      for dep in $(otool -L "$stage/runtime/$n" | awk 'NR > 1 {print $1}' | grep "^$prefix/lib/" || true); do
        install_name_tool -change "$dep" "@rpath/${dep##*/}" "$stage/runtime/$n"
      done
      # mpv's Swift link adds the build machine's Xcode toolchain as an rpath; the Swift runtime
      # it needs is macOS's own, in /usr/lib/swift, which it names directly.
      for rp in $(otool -l "$stage/runtime/$n" | awk '$1 == "cmd" && $2 == "LC_RPATH" { getline; getline; print $2 }'); do
        case $rp in /usr/lib/*|@*) ;; *) install_name_tool -delete_rpath "$rp" "$stage/runtime/$n" ;; esac
      done
      install_name_tool -add_rpath @loader_path "$stage/runtime/$n"
      strip -x "$stage/runtime/$n"
      ln -s "../runtime/$n" "$stage/link/${n%%.*}.dylib"
    done
    for f in "$stage"/runtime/*.dylib; do
      otool -L "$f"
      if otool -L "$f" | tail -n +2 | grep -vE '^\s+(/usr/lib/|/System/Library/|@rpath/lib(mpv|av|sw))'; then
        die "${f##*/} links something outside the system and the bundle"
      fi
    done
    lib=$stage/runtime/libmpv.2.dylib
    ;;
esac
echo "runtime: $(cd "$stage/runtime" && echo *)"

# The notices: what is inside, under which license, where its source is, and every license's
# text. Build-only sources (nasm, Jinja, MarkupSafe) are not in the library and not listed.
notices=$stage/THIRD-PARTY-NOTICES-libmpv.txt
source_url=${LIBMPV_SOURCE_URL:-the release this archive was published in}
# <license> is one SPDX id: the one zapper takes it under (Vulkan-Headers is Apache-2.0 or MIT;
# glslang's main license is BSD-3-Clause, and its file lists the others its parts carry).
component() { # component <name> <license> <homepage> <license files...>
  local name=$1 license=$2 home=$3 title; shift 3
  title="$name $(version_of "$name")"
  printf '  %-16s %-12s %-26s %s\n' "$name" "$(version_of "$name" | cut -c1-12)" "$license" "$home" >> "$notices.list"
  {
    printf '\n\n%s\n%s\n\n' "$title" "$(printf '%s' "$title" | sed 's/./-/g')"
    for f in "$@"; do
      # libplacebo's and shaderc's submodules are unpacked inside their trees.
      local dir
      for dir in "$src/$name" "$src/libplacebo/3rdparty/$name" "$src/libplacebo/3rdparty/Vulkan-Headers" \
          "$src/shaderc/third_party/$name"; do
        [ -f "$dir/$f" ] && break
      done
      [ -f "$dir/$f" ] || die "$name: no license file $f"
      printf -- '-- %s --\n\n' "$f"
      cat "$dir/$f"
      printf '\n'
    done
  } >> "$notices.texts"
}
: > "$notices.list"; : > "$notices.texts"
component mpv "LGPL-2.1-or-later" https://mpv.io LICENSE.LGPL Copyright
component ffmpeg "LGPL-2.1-or-later" https://ffmpeg.org COPYING.LGPLv2.1 LICENSE.md
component libplacebo "LGPL-2.1-or-later" https://code.videolan.org/videolan/libplacebo LICENSE
component glad "MIT" https://github.com/Dav1dde/glad LICENSE
component vulkan-headers "MIT" https://github.com/KhronosGroup/Vulkan-Headers LICENSE.md
component fast_float "MIT" https://github.com/fastfloat/fast_float LICENSE-MIT
component dav1d "BSD-2-Clause" https://code.videolan.org/videolan/dav1d COPYING
component libass "ISC" https://github.com/libass/libass COPYING
case $family in
  linux)
    component libdisplay-info "MIT" https://gitlab.freedesktop.org/emersion/libdisplay-info LICENSE
    ;;
  *)
    component freetype "FTL" https://freetype.org LICENSE.TXT docs/FTL.TXT
    component harfbuzz "MIT" https://harfbuzz.github.io COPYING
    component fribidi "LGPL-2.1-or-later" https://github.com/fribidi/fribidi COPYING
    ;;
esac
if [ "$family" = windows ]; then
  component zlib "Zlib" https://zlib.net LICENSE
  component shaderc "Apache-2.0" https://github.com/google/shaderc LICENSE
  component glslang "BSD-3-Clause" https://github.com/KhronosGroup/glslang LICENSE.txt
  component spirv-tools "Apache-2.0" https://github.com/KhronosGroup/SPIRV-Tools LICENSE
  component spirv-headers "MIT" https://github.com/KhronosGroup/SPIRV-Headers LICENSE
  component spirv-cross "Apache-2.0" https://github.com/KhronosGroup/SPIRV-Cross LICENSE
fi
lib_name=${lib##*/}
runtime_list=$(cd "$stage/runtime" && ls | sed 's/^/  /')
{
  cat <<EOF
zapper's libmpv and FFmpeg: what is in them
===========================================

These are libmpv $(version_of mpv) and FFmpeg $(version_of ffmpeg)'s libraries, built for $platform by
zapper's scripts/build-libmpv.sh with nothing under the GPL: mpv configured with -Dgpl=false
and FFmpeg without --enable-gpl, --enable-version3 or --enable-nonfree. Each is under the GNU
Lesser General Public License, version 2.1 or later; its text is below, under mpv.

$runtime_list

zapper plays through $lib_name, and its recording service (zapper-server) remuxes through
FFmpeg's libraries. The components below are linked into them, each under its own license.
The exact source of every one, as built, is published beside this archive:

  $source_url

You may replace any of these libraries with your own build of the same version of its API:
zapper loads whichever ones sit where these do.

  component        version      license                    home
EOF
  cat "$notices.list"
  # The same, counted by license, in the shape the other notices files have (the Arch package
  # reads its license field from these summaries, scripts/arch-pkg.py).
  printf '\n\nSummary\n-------\n'
  awk '{ n[$3]++ } END { for (l in n) printf "  %-20s %3d\n", l, n[l] }' "$notices.list" | sort -k2,2nr -k1,1
  printf '\n'
  cat "$notices.texts"
} > "$notices"
rm -f "$notices.list" "$notices.texts"

# What was built and from what: the sources, FFmpeg's configure line and mpv's options, as each
# recorded them.
{
  echo "platform $platform"
  echo "runtime $(cd "$stage/runtime" && echo *)"
  for i in "${!s_name[@]}"; do echo "source ${s_name[$i]} ${s_version[$i]} ${s_sum[$i]} ${s_url[$i]}"; done
  echo "ffmpeg-configuration $(sed -n 's/^#define FFMPEG_CONFIGURATION "\(.*\)"$/\1/p' "$build/ffmpeg/config.h")"
  meson introspect --buildoptions "$build/mpv" |
    python3 -I -c 'import json,sys; print("mpv-options " + " ".join("-D%s=%s" % (o["name"], o["value"]) for o in json.load(sys.stdin) if o["section"] == "user"))'
} > "$stage/BUILDINFO.txt"

# The archive, and the sources it was built from beside it.
case $family in
  windows) archive=$out/$stage_name.zip
    rm -f "$archive"
    (cd "$work" && python3 -I -m zipfile -c "$archive" "$stage_name") ;;
  *) archive=$out/$stage_name.tar.xz
    rm -f "$archive"
    (cd "$work" && tar -cJf "$archive" "$stage_name") ;;
esac
# Every source, the build-time ones (nasm, Jinja, MarkupSafe) included: a rebuild needs them.
mkdir -p "$out/sources"
for f in "${s_file[@]}"; do cp "$dl/$f" "$out/sources/"; done

echo
echo "$(du -h "$lib" | cut -f1)  $lib_name"
echo "$(du -h "$archive" | cut -f1)  $archive"

if [ "${LIBMPV_KEEP_BUILD:-0}" != 1 ]; then
  rm -rf "$src" "$build" "$prefix"
fi
