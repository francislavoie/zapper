<#
.SYNOPSIS
    Fetches zapper's libmpv and FFmpeg for Windows and makes the import
    libraries zapper and zapper-server link against.

.DESCRIPTION
    zapper links libmpv dynamically (libmpv2-sys emits `rustc-link-lib=mpv`),
    and zapper-server's remuxer FFmpeg's avformat, avcodec and avutil, so an
    MSVC build needs what no Windows machine has by default:

      * runtime\       - libmpv-2.dll and FFmpeg's DLLs, beside zapper.exe at
                         run time;
      * link\*.lib     - the import libraries the linker resolves `mpv`,
                         `avformat` and the rest to, made here;
      * include\       - mpv's and FFmpeg's headers, for the remuxer.

    They are zapper's own LGPL build of libmpv and FFmpeg
    (scripts/build-libmpv.sh, published by .github/workflows/libmpv.yml): the
    archive packaging/libmpv/manifest.json names for windows-x86_64, checked
    against the manifest's SHA-256. This script unpacks it into
    crates/zapper/lib/windows-x86_64/ (ignored by git), which build.rs adds to the
    link search path and tauri.windows.conf.json bundles from, and makes an
    import library for each export list in link\ (mpv.def, avformat.def, ...)
    with the MSVC `lib` tool, found through vswhere. Does nothing when that
    archive is already unpacked there.

.PARAMETER Archive
    A libmpv-windows-x86_64.zip to use instead of the manifest's download (one
    you built with scripts/build-libmpv.sh).

.PARAMETER Dest
    Where to put the result. Defaults to crates/zapper/lib/windows-x86_64.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File tools\fetch-libmpv.ps1
#>
param(
    [string]$Archive = "",
    [string]$Dest = (Join-Path $PSScriptRoot "..\crates\zapper\lib\windows-x86_64")
)

$ErrorActionPreference = "Stop"
# Windows PowerShell 5 redraws its progress bar per chunk, which makes a 30 MB
# download take minutes instead of seconds.
$ProgressPreference = "SilentlyContinue"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$platform = "windows-x86_64"
New-Item -ItemType Directory -Force -Path $Dest | Out-Null
$Dest = (Resolve-Path $Dest).Path
$lib = Split-Path -Parent $Dest
$cache = Join-Path $lib ".cache"
New-Item -ItemType Directory -Force -Path $cache | Out-Null

if ($Archive) {
    $Archive = (Resolve-Path $Archive).Path
    $want = (Get-FileHash -Algorithm SHA256 $Archive).Hash.ToLowerInvariant()
    Write-Host "Using $Archive (not the manifest's published build)"
} else {
    $manifestPath = Join-Path $PSScriptRoot "..\packaging\libmpv\manifest.json"
    $manifest = Get-Content -Raw $manifestPath | ConvertFrom-Json
    $entry = $manifest.platforms.$platform
    if (-not $entry -or -not $entry.sha256) {
        throw "packaging/libmpv/manifest.json has no published libmpv for $platform (tag $($manifest.tag)); build one with scripts/build-libmpv.sh and pass it with -Archive (docs/licensing.md)"
    }
    $want = $entry.sha256.ToLowerInvariant()
    if ($want -notmatch '^[0-9a-f]{64}$' -or -not $entry.url.StartsWith("https://")) {
        throw "packaging/libmpv/manifest.json: malformed entry for $platform"
    }
    $stamp = Join-Path $Dest ".archive-sha256"
    if ((Test-Path $stamp) -and ((Get-Content -Raw $stamp).Trim() -eq $want) -and (Test-Path (Join-Path $Dest "link\mpv.lib"))) {
        Write-Host "libmpv for $platform is up to date in $Dest"
        exit 0
    }
    $Archive = Join-Path $cache ($entry.url.Split("/")[-1])
    if (-not (Test-Path $Archive) -or (Get-FileHash -Algorithm SHA256 $Archive).Hash.ToLowerInvariant() -ne $want) {
        Write-Host "Downloading $($entry.url)"
        Invoke-WebRequest -Uri $entry.url -OutFile "$Archive.part" -UseBasicParsing
        Move-Item -Force "$Archive.part" $Archive
    }
    $got = (Get-FileHash -Algorithm SHA256 $Archive).Hash.ToLowerInvariant()
    if ($got -ne $want) {
        Remove-Item -Force $Archive
        throw "SHA-256 mismatch for $Archive`: got $got, the manifest says $want"
    }
    Write-Host "SHA-256 verified"
}

# Unpacked beside the old copy, then swapped in, so an interrupted run never
# leaves half of one.
$work = Join-Path $lib (".unpack-" + [guid]::NewGuid().ToString("N"))
Expand-Archive -Path $Archive -DestinationPath $work
$top = Join-Path $work "libmpv-$platform"
foreach ($f in @("runtime\libmpv-2.dll", "link\mpv.def", "link\avformat.def", "include\libavformat\avformat.h")) {
    if (-not (Test-Path (Join-Path $top $f))) { throw "the archive has no libmpv-$platform/$f" }
}

# The MSVC tools, wherever Visual Studio or the Build Tools put them.
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (-not (Test-Path $vswhere)) { throw "vswhere.exe not found; install the Visual Studio 2022 Build Tools" }
$vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vs) { throw "no Visual Studio install with the x64 C++ tools" }
$bin = Get-ChildItem -Path "$vs\VC\Tools\MSVC\*\bin\Hostx64\x64" -Directory |
    Sort-Object FullName -Descending | Select-Object -First 1
if (-not $bin) { throw "no MSVC x64 toolset under $vs" }
$libExe = Join-Path $bin.FullName "lib.exe"

# Each .def names its DLL (LIBRARY) and lists its exports.
foreach ($def in Get-ChildItem -Path (Join-Path $top "link") -Filter "*.def") {
    $out = [IO.Path]::ChangeExtension($def.FullName, ".lib")
    & $libExe /nologo "/def:$($def.FullName)" "/out:$out" /machine:x64 | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "lib.exe failed on $($def.Name) with exit code $LASTEXITCODE" }
    # lib.exe leaves an export file beside the import library; nothing uses it.
    Remove-Item -Force -ErrorAction SilentlyContinue ([IO.Path]::ChangeExtension($def.FullName, ".exp"))
}

Set-Content -Path (Join-Path $top ".archive-sha256") -Value $want -Encoding ASCII
if (Test-Path $Dest) { Remove-Item -Recurse -Force $Dest }
Move-Item $top $Dest
Remove-Item -Recurse -Force $work
Write-Host "libmpv for $platform ready in $Dest"
Get-Content (Join-Path $Dest "BUILDINFO.txt") | Where-Object { $_ -match '^source (mpv|ffmpeg) ' }
