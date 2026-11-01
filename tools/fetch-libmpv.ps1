<#
.SYNOPSIS
    Fetches libmpv for Windows and makes the import library zapper links against.

.DESCRIPTION
    zapper links libmpv dynamically (libmpv2-sys emits `rustc-link-lib=mpv`),
    so an MSVC build needs two things that no Windows machine has by default:

      * mpv.lib      - the import library the linker resolves `mpv` to, and
      * libmpv-2.dll - the library itself, beside zapper.exe at run time.

    Neither is committed (src-tauri/lib/ is ignored): the DLL is ~100 MB of
    somebody else's build, and the .lib is derived from it. This script puts
    both in src-tauri/lib/windows/, which build.rs adds to the link search
    path and tauri.windows.conf.json bundles from.

    The build is shinchiro's mpv-winbuild-cmake `mpv-dev-x86_64`, the one mpv's
    own site links to. Not the `-v3` variant: that one is compiled for
    x86-64-v3 (AVX2) and dies with an illegal instruction on older CPUs, which
    is not a failure a player should have.

    The archive ships `mpv.def`; the import library is made from it with the
    MSVC `lib` tool, found through vswhere. If a future archive stops shipping
    the .def, one is generated from the DLL's export table with `dumpbin`.

.PARAMETER Tag
    A release tag of shinchiro/mpv-winbuild-cmake (e.g. 20261101), or
    "latest".

.PARAMETER Dest
    Where to put the result. Defaults to src-tauri/lib/windows.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File tools\fetch-libmpv.ps1
#>
param(
    [string]$Tag = "latest",
    [string]$Dest = (Join-Path $PSScriptRoot "..\src-tauri\lib\windows")
)

$ErrorActionPreference = "Stop"
# Windows PowerShell 5 redraws its progress bar per chunk, which makes a 30 MB
# download take minutes instead of seconds.
$ProgressPreference = "SilentlyContinue"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$repo = "shinchiro/mpv-winbuild-cmake"
$api = if ($Tag -eq "latest") {
    "https://api.github.com/repos/$repo/releases/latest"
} else {
    "https://api.github.com/repos/$repo/releases/tags/$Tag"
}
$release = Invoke-RestMethod -Uri $api -Headers @{ "User-Agent" = "zapper-fetch-libmpv" }
$asset = $release.assets |
    Where-Object { $_.name -match '^mpv-dev-x86_64-\d{8}-git-[0-9a-f]+\.7z$' } |
    Select-Object -First 1
if (-not $asset) {
    throw "release $($release.tag_name) of $repo has no mpv-dev-x86_64 archive"
}

New-Item -ItemType Directory -Force -Path $Dest | Out-Null
$Dest = (Resolve-Path $Dest).Path
$work = Join-Path $Dest ".download"
if (Test-Path $work) { Remove-Item -Recurse -Force $work }
New-Item -ItemType Directory -Force -Path $work | Out-Null

$archive = Join-Path $work $asset.name
Write-Host "Downloading $($asset.name) ($([math]::Round($asset.size / 1MB, 1)) MB)"
Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $archive -UseBasicParsing

# GitHub records a digest for every uploaded asset. Checked when present, so a
# truncated or substituted download is never turned into a library.
if ($asset.digest -and $asset.digest -like "sha256:*") {
    $want = $asset.digest.Substring(7).ToLowerInvariant()
    $got = (Get-FileHash -Algorithm SHA256 $archive).Hash.ToLowerInvariant()
    if ($got -ne $want) { throw "SHA-256 mismatch for $($asset.name): got $got, expected $want" }
    Write-Host "SHA-256 verified"
}

# 7-Zip if it is installed and works, else the tar that ships with Windows 10
# and later, whose libarchive reads 7z with LZMA2 too. Both are tried rather
# than the first one found, because a half-updated scoop install leaves a
# `7z` shim on PATH that points at nothing.
$unpacked = Join-Path $work "x"
$extractors = @()
$found = Get-Command 7z -ErrorAction SilentlyContinue
if ($found) { $extractors += , @($found.Source, "x", "-y", "-o$unpacked", $archive) }
foreach ($p in @("$env:ProgramFiles\7-Zip\7z.exe", "${env:ProgramFiles(x86)}\7-Zip\7z.exe")) {
    if (Test-Path $p) { $extractors += , @($p, "x", "-y", "-o$unpacked", $archive) }
}
$tar = Join-Path $env:SystemRoot "System32\tar.exe"
if (Test-Path $tar) { $extractors += , @($tar, "-xf", $archive, "-C", $unpacked) }
$extracted = $false
foreach ($cmd in $extractors) {
    if (Test-Path $unpacked) { Remove-Item -Recurse -Force $unpacked }
    New-Item -ItemType Directory -Force -Path $unpacked | Out-Null
    $exe, $cmdArgs = $cmd
    try { & $exe @cmdArgs 2>&1 | Out-Null } catch { continue }
    if ($LASTEXITCODE -eq 0 -and (Get-ChildItem -Recurse -Path $unpacked -Filter "libmpv-2.dll")) {
        $extracted = $true
        break
    }
}
if (-not $extracted) {
    throw "could not unpack the archive; install 7-Zip ('winget install 7zip.7zip') and run again"
}

$dll = Get-ChildItem -Recurse -Path $unpacked -Filter "libmpv-2.dll" | Select-Object -First 1
if (-not $dll) { throw "the archive has no libmpv-2.dll" }
Copy-Item -Force $dll.FullName $Dest
$include = Get-ChildItem -Recurse -Directory -Path $unpacked -Filter "include" | Select-Object -First 1
if ($include) { Copy-Item -Recurse -Force $include.FullName $Dest }

# The MSVC tools, wherever Visual Studio or the Build Tools put them.
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (-not (Test-Path $vswhere)) { throw "vswhere.exe not found; install the Visual Studio 2022 Build Tools" }
$vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vs) { throw "no Visual Studio install with the x64 C++ tools" }
$bin = Get-ChildItem -Path "$vs\VC\Tools\MSVC\*\bin\Hostx64\x64" -Directory |
    Sort-Object FullName -Descending | Select-Object -First 1
if (-not $bin) { throw "no MSVC x64 toolset under $vs" }
$libExe = Join-Path $bin.FullName "lib.exe"

$def = Get-ChildItem -Recurse -Path $unpacked -Filter "mpv.def" | Select-Object -First 1
if ($def) {
    $defPath = $def.FullName
} else {
    Write-Host "No mpv.def in the archive; generating one from the DLL's exports"
    $dumpbin = Join-Path $bin.FullName "dumpbin.exe"
    $names = & $dumpbin /nologo /exports $dll.FullName |
        ForEach-Object { if ($_ -match '^\s+\d+\s+[0-9A-F]+\s+[0-9A-F]+\s+(mpv_\w+)') { $Matches[1] } }
    if (-not $names) { throw "dumpbin found no mpv_ exports in libmpv-2.dll" }
    $defPath = Join-Path $work "mpv.def"
    Set-Content -Path $defPath -Value (@("EXPORTS") + $names) -Encoding ASCII
}
Copy-Item -Force $defPath (Join-Path $Dest "mpv.def")

& $libExe /nologo "/def:$defPath" /name:libmpv-2.dll "/out:$(Join-Path $Dest 'mpv.lib')" /machine:x64 | Out-Null
if ($LASTEXITCODE -ne 0) { throw "lib.exe failed with exit code $LASTEXITCODE" }
# lib.exe leaves an export file beside the import library; nothing uses it.
Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $Dest "mpv.exp")

Set-Content -Path (Join-Path $Dest "VERSION") -Value "$($release.tag_name) $($asset.name)" -Encoding ASCII
Remove-Item -Recurse -Force $work
Write-Host "libmpv $($release.tag_name) ready in $Dest"
Get-ChildItem $Dest | Select-Object Name, Length | Format-Table -AutoSize
