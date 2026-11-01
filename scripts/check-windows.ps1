<#
.SYNOPSIS
    Checks the Windows installers a release build made: what each one puts
    on disk, and that what it puts there runs on a bare Windows.

.DESCRIPTION
    For the MSI and the NSIS installer of -Version in -Dir
    (zapper_<version>_x64_en-US.msi, zapper_<version>_x64-setup.exe, the names
    the updater downloads by; src-tauri/src/updates/release.rs):

      * zapper.exe and zapper-server.exe side by side, with every DLL
        src-tauri/tauri.windows.conf.json bundles, the same files in both;
      * the install-kind marker the updater reads: msi in the MSI; nsis once
        the NSIS installer has run (its post-install hook writes it,
        packaging/windows/installer-hooks.nsh), msi in it before that;
      * the license notices in licenses\, none empty (docs/licensing.md);
      * every executable and DLL imports only the bundled DLLs and Windows'
        own: nothing from a Visual C++ redistributable, which a fresh
        Windows does not have (dumpbin /dependents);
      * FFmpeg's libraries say they are LGPL, and libmpv was built with
        -Dgpl=false: loaded from the installed folder and asked;
      * zapper-server.exe --version answers with -Version, which it can only
        do with its DLLs found.

    The MSI is unpacked with an administrative install (msiexec /a), which
    installs nothing. The NSIS installer is unpacked with 7-Zip, or with
    -Install really installed (silently, for this user, into a scratch
    folder) and uninstalled again: that is what runs its hook. The release
    workflow uses -Install; on a desktop you care about, leave it off.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\check-windows.ps1 -Dir out -Version 0.2.0
#>
param(
    [Parameter(Mandatory = $true)][string]$Dir,
    [Parameter(Mandatory = $true)][string]$Version,
    [switch]$Install
)

$ErrorActionPreference = "Stop"
$failures = New-Object System.Collections.Generic.List[string]
function Fail([string]$what) { Write-Host "  FAIL $what"; $failures.Add($what) }
function Step([string]$what) { Write-Host "== $what" }

$repo = Split-Path -Parent $PSScriptRoot
$Dir = (Resolve-Path $Dir).Path
$msi = Join-Path $Dir "zapper_${Version}_x64_en-US.msi"
$nsis = Join-Path $Dir "zapper_${Version}_x64-setup.exe"
foreach ($f in @($msi, $nsis)) { if (-not (Test-Path $f)) { throw "Missing $f" } }

# What the installers must carry: the DLLs tauri.windows.conf.json bundles,
# beside the two programs.
$conf = Get-Content -Raw (Join-Path $repo "src-tauri\tauri.windows.conf.json") | ConvertFrom-Json
$dlls = @($conf.bundle.resources.PSObject.Properties | ForEach-Object { $_.Value } | Where-Object { $_ -like "*.dll" })
if ($dlls.Count -lt 2) { throw "tauri.windows.conf.json bundles no DLLs?" }
$programs = @("zapper.exe", "zapper-server.exe")
$notices = @("LICENSE.txt", "MPV-FFMPEG-NOTICE.txt", "THIRD-PARTY-NOTICES-libmpv.txt",
    "THIRD-PARTY-NOTICES-rust.txt", "THIRD-PARTY-NOTICES-ui.txt")

$work = Join-Path ([IO.Path]::GetTempPath()) ("zapper-check-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force $work | Out-Null

# The MSVC tools, for dumpbin, wherever Visual Studio or the Build Tools put them.
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
$bin = Get-ChildItem -Path "$vs\VC\Tools\MSVC\*\bin\Hostx64\x64" -Directory | Sort-Object FullName -Descending | Select-Object -First 1
$dumpbin = Join-Path $bin.FullName "dumpbin.exe"
if (-not (Test-Path $dumpbin)) { throw "dumpbin.exe not found (Visual Studio's C++ tools)" }

# Windows' own DLLs, which every copy of Windows 10 and 11 has: those in
# System32 (and the API sets, which resolve to them), but never the Visual
# C++ runtime's, which only an installed redistributable puts there.
$system32 = Join-Path $env:SystemRoot "System32"
function Is-Redistributable([string]$name) {
    $name -match '^(vcruntime|msvcp|vcomp|concrt|vccorlib|ucrtbased|mfc)\d*.*\.dll$'
}

function Imports([string]$file) {
    $out = & $dumpbin /nologo /dependents $file
    if ($LASTEXITCODE -ne 0) { throw "dumpbin failed on $file" }
    $out | Where-Object { $_ -match '^\s+\S+\.dll\s*$' } | ForEach-Object { $_.Trim() }
}

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class ZapperProbe {
    [DllImport("kernel32", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool SetDllDirectory(string path);
    [DllImport("kernel32", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr LoadLibrary(string path);
    [DllImport("kernel32", CharSet = CharSet.Ansi, SetLastError = true)]
    static extern IntPtr GetProcAddress(IntPtr module, string name);

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate IntPtr NoArgs();
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate int SetOption(IntPtr h, string name, string value);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate int Handle(IntPtr h);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate IntPtr GetString(IntPtr h, string name);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate void Destroy(IntPtr h);

    static IntPtr Load(string dir, string dll) {
        SetDllDirectory(dir);
        IntPtr m = LoadLibrary(System.IO.Path.Combine(dir, dll));
        if (m == IntPtr.Zero) throw new Exception("could not load " + dll + ": error " + Marshal.GetLastWin32Error());
        return m;
    }
    static T Fn<T>(IntPtr m, string name) {
        IntPtr p = GetProcAddress(m, name);
        if (p == IntPtr.Zero) throw new Exception("no " + name);
        return (T)(object)Marshal.GetDelegateForFunctionPointer(p, typeof(T));
    }
    // FFmpeg's avutil_license() and the like: "LGPL version 2.1 or later".
    public static string License(string dir, string dll, string fn) {
        return Marshal.PtrToStringAnsi(Fn<NoArgs>(Load(dir, dll), fn)());
    }
    // libmpv's build options, from a player that opens no window and no
    // audio device.
    public static string MpvConfiguration(string dir, string dll) {
        IntPtr m = Load(dir, dll);
        IntPtr h = Fn<NoArgs>(m, "mpv_create")();
        SetOption set = Fn<SetOption>(m, "mpv_set_option_string");
        set(h, "vo", "null"); set(h, "ao", "null"); set(h, "config", "no"); set(h, "terminal", "no");
        if (Fn<Handle>(m, "mpv_initialize")(h) < 0) throw new Exception("mpv_initialize failed");
        IntPtr s = Fn<GetString>(m, "mpv_get_property_string")(h, "mpv-configuration");
        string text = s == IntPtr.Zero ? "" : Marshal.PtrToStringAnsi(s);
        Fn<Destroy>(m, "mpv_terminate_destroy")(h);
        return text;
    }
}
'@

function Check-Installed([string]$label, [string]$root, [string]$kind) {
    Step "$label`: $root"
    $exe = Get-ChildItem -Path $root -Recurse -Filter "zapper.exe" | Select-Object -First 1
    if (-not $exe) { Fail "$label`: no zapper.exe"; return $null }
    $at = $exe.DirectoryName
    foreach ($f in $programs + $dlls) {
        if (-not (Test-Path (Join-Path $at $f))) { Fail "$label`: no $f beside zapper.exe" }
    }

    $marker = Join-Path $at "install-kind"
    $found = if (Test-Path $marker) { (Get-Content -Raw $marker).Trim() } else { "(none)" }
    if ($found -ne $kind) { Fail "$label`: install-kind is '$found', not '$kind'" } else { Write-Host "  install-kind = $found" }

    foreach ($n in $notices) {
        $p = Join-Path $at "licenses\$n"
        if (-not (Test-Path $p) -or (Get-Item $p).Length -eq 0) { Fail "$label`: licenses\$n missing or empty" }
    }

    $bundled = @($dlls | ForEach-Object { $_.ToLowerInvariant() })
    foreach ($f in Get-ChildItem -Path $at -File | Where-Object { $_.Extension -in ".exe", ".dll" }) {
        if ($f.Name -like "uninstall*") { continue }
        foreach ($d in Imports $f.FullName) {
            $l = $d.ToLowerInvariant()
            if ($bundled -contains $l) { continue }
            if (Is-Redistributable $l) { Fail "$label`: $($f.Name) imports $d, from a Visual C++ redistributable"; continue }
            if ($l -like "api-ms-win-*" -or $l -like "ext-ms-*") { continue }
            if (-not (Test-Path (Join-Path $system32 $d))) { Fail "$label`: $($f.Name) imports $d, which is neither bundled nor Windows'" }
        }
    }

    $server = Join-Path $at "zapper-server.exe"
    if (Test-Path $server) {
        $out = (& $server --version 2>&1 | Out-String).Trim()
        if ($out -ne "zapper-server $Version") { Fail "$label`: zapper-server.exe --version said '$out'" } else { Write-Host "  $out" }
    }
    return $at
}

# The MSI, unpacked.
$msiDir = Join-Path $work "msi"
$p = Start-Process -Wait -PassThru msiexec.exe -ArgumentList "/a", "`"$msi`"", "/qn", "TARGETDIR=`"$msiDir`""
if ($p.ExitCode -ne 0) { throw "msiexec /a failed with $($p.ExitCode)" }
$fromMsi = Check-Installed "MSI" $msiDir "msi"

# The NSIS installer, installed and later removed, or unpacked.
$nsisDir = Join-Path $work "nsis"
if ($Install) {
    # /D must come last and unquoted.
    $p = Start-Process -Wait -PassThru $nsis -ArgumentList "/S", "/D=$nsisDir"
    if ($p.ExitCode -ne 0) { throw "the NSIS installer failed with $($p.ExitCode)" }
    $fromNsis = Check-Installed "NSIS (installed)" $nsisDir "nsis"
} else {
    $7z = "$env:ProgramFiles\7-Zip\7z.exe"
    if (-not (Test-Path $7z)) { $7z = (Get-Command 7z.exe -ErrorAction Stop).Source }
    & $7z x -y "-o$nsisDir" $nsis | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "7-Zip could not unpack $nsis" }
    $fromNsis = Check-Installed "NSIS (unpacked)" $nsisDir "msi"
}

if ($fromMsi -and $fromNsis) {
    # Not zapper.exe: Tauri patches each installer's copy with its bundle type.
    Step "the same zapper-server and DLLs in both"
    foreach ($f in @("zapper-server.exe") + $dlls) {
        $a = Join-Path $fromMsi $f; $b = Join-Path $fromNsis $f
        if ((Test-Path $a) -and (Test-Path $b) -and (Get-FileHash $a).Hash -ne (Get-FileHash $b).Hash) { Fail "$f differs between the MSI and the NSIS installer" }
    }
}

if ($fromMsi) {
    Step "LGPL: FFmpeg's libraries and libmpv, as built"
    foreach ($d in $dlls) {
        if (-not (Test-Path (Join-Path $fromMsi $d))) { continue }
        $base = ($d -split '-')[0]
        if ($base -eq "libmpv") {
            $config = [ZapperProbe]::MpvConfiguration($fromMsi, $d)
            if ($config -notmatch '(^|\s)-Dgpl=false(\s|$)') { Fail "$d was not built with -Dgpl=false: $config" } else { Write-Host "  $d`: -Dgpl=false" }
        } else {
            $license = [ZapperProbe]::License($fromMsi, $d, "${base}_license")
            if ($license -notmatch '^LGPL') { Fail "$d says its license is '$license'" } else { Write-Host "  $d`: $license" }
        }
    }
}

if ($Install) {
    Step "uninstall the NSIS install"
    $p = Start-Process -Wait -PassThru (Join-Path $nsisDir "uninstall.exe") -ArgumentList "/S"
    # The uninstaller copies itself elsewhere and returns at once; wait for it.
    $deadline = (Get-Date).AddSeconds(120)
    while ((Test-Path (Join-Path $nsisDir "zapper.exe")) -and (Get-Date) -lt $deadline) { Start-Sleep 1 }
    if (Test-Path (Join-Path $nsisDir "zapper.exe")) { Fail "the NSIS uninstaller left zapper.exe behind" }
}

Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
if ($failures.Count -gt 0) {
    Write-Host "FAIL: $($failures.Count) problem(s)"
    exit 1
}
Write-Host "PASS: both installers carry zapper and zapper-server, their DLLs, the notices and the install-kind marker, and need nothing Windows lacks"
