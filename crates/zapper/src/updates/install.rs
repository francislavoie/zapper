//! How this copy of zapper was installed, and the commands that install an
//! update over it.
//!
//! One build goes into every package of a platform (the `.deb`, the `.rpm`
//! and the Arch package hold the same binary), so the binary cannot know which
//! one it arrived in. Each package says so instead, in a one-word file the app reads
//! at run time, `install-kind`:
//!
//! - Linux: `/usr/lib/zapper/install-kind`, shipped by tauri.conf.json's
//!   `bundle.linux.deb.files` (`deb`) and `bundle.linux.rpm.files` (`rpm`);
//!   `scripts/package-arch.sh` makes the Arch package from the `.deb` and
//!   writes `arch` there (packaging/install-kind-arch).
//!   Not beside the binary, which is `/usr/bin/zapper`: a stray file in
//!   `/usr/bin` would be a command.
//! - Windows (not built yet): beside `zapper.exe`. The MSI carries a file
//!   saying `msi` as a bundle resource, and the NSIS installer's post-install
//!   hook overwrites it with `nsis` (packaging/windows/).
//! - macOS (not built yet): `zapper.app/Contents/Resources/install-kind`,
//!   `dmg`, added only by the release build (packaging/macos/), so a `.app`
//!   bundled locally counts as a build from source.
//!
//! No file: a build from source, which is only ever offered the release page.
//!
//! The commands are argument lists, never shell lines, and are run by an
//! `UpdateRunner` (the app's runs them; tests only record them).

use std::path::{Path, PathBuf};

/// The marker's file name.
pub const INSTALL_KIND_FILE: &str = "install-kind";

/// The bundle the `.dmg` holds at its top level, and the name an update
/// replaces. Tauri names it after `productName`.
pub const BUNDLE_NAME: &str = "zapper.app";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallKind {
    Deb,
    /// Fedora, RHEL, openSUSE: dnf, else zypper, else plain rpm installs it.
    Rpm,
    /// Arch Linux and its derivatives: pacman installs it.
    Arch,
    /// The per-user NSIS installer.
    Nsis,
    /// The per-machine MSI.
    Msi,
    /// zapper.app from the `.dmg`: an update replaces the bundle in place.
    Dmg,
    /// A build from source: nothing to update, only the release page.
    Unpackaged,
}

impl InstallKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "deb" => Some(Self::Deb),
            "rpm" => Some(Self::Rpm),
            "arch" => Some(Self::Arch),
            "nsis" => Some(Self::Nsis),
            "msi" => Some(Self::Msi),
            "dmg" => Some(Self::Dmg),
            "none" => Some(Self::Unpackaged),
            _ => None,
        }
    }

    /// The word the frontend sees, the same as the marker's.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deb => "deb",
            Self::Rpm => "rpm",
            Self::Arch => "arch",
            Self::Nsis => "nsis",
            Self::Msi => "msi",
            Self::Dmg => "dmg",
            Self::Unpackaged => "none",
        }
    }

    /// Windows installers replace the running app's files, which Windows does
    /// not allow while they are open, so zapper quits once one has started.
    pub fn quits_to_install(self) -> bool {
        matches!(self, Self::Nsis | Self::Msi)
    }

    /// The Linux package managers run as root through pkexec, which has its
    /// own failure modes worth naming (cancelled prompt, no polkit agent).
    pub fn uses_pkexec(self) -> bool {
        matches!(self, Self::Deb | Self::Rpm | Self::Arch)
    }
}

/// Where a package may have put the marker, given the running binary. Every
/// platform's place is tried on every platform: each is specific enough that
/// it cannot exist by accident, and one list is easier to test than three
/// `cfg` branches.
pub fn marker_dirs(exe: &Path) -> Vec<PathBuf> {
    let Some(dir) = exe.parent() else { return Vec::new() };
    vec![
        // /usr/bin/zapper → /usr/lib/zapper
        dir.join("../lib/zapper"),
        // zapper.app/Contents/MacOS/zapper → Contents/Resources
        dir.join("../Resources"),
        // C:\…\zapper\zapper.exe → beside it
        dir.to_path_buf(),
    ]
}

/// The install kind: the first marker found, else a build from source. An
/// unknown word counts as none rather than guessing.
pub fn detect_install_kind(exe: Option<&Path>) -> InstallKind {
    let Some(exe) = exe else { return InstallKind::Unpackaged };
    marker_dirs(exe)
        .iter()
        .find_map(|d| std::fs::read_to_string(d.join(INSTALL_KIND_FILE)).ok())
        .as_deref()
        .and_then(InstallKind::parse)
        .unwrap_or(InstallKind::Unpackaged)
}

/// What installs an RPM here: the first of dnf (Fedora, RHEL) and zypper
/// (openSUSE) that is installed, else rpm itself, which cannot fetch missing
/// dependencies but is always there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpmTool {
    Dnf,
    Zypper,
    Rpm,
}

impl RpmTool {
    /// Looked up on PATH and in the system's bin folders, because pkexec runs
    /// the tool with its own, minimal PATH.
    pub fn detect() -> Self {
        let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        dirs.extend(["/usr/bin", "/usr/sbin", "/bin", "/sbin"].map(PathBuf::from));
        Self::pick(|name| dirs.iter().any(|d| d.join(name).is_file()))
    }

    fn pick(installed: impl Fn(&str) -> bool) -> Self {
        if installed("dnf") {
            Self::Dnf
        } else if installed("zypper") {
            Self::Zypper
        } else {
            Self::Rpm
        }
    }

    /// The install as root, the package's path to follow. zypper refuses an
    /// unsigned package unless told; the download has been checked against
    /// the release's SHA256SUMS already, which is the assurance a signature
    /// would have given.
    fn install_args_without_file(self) -> Vec<&'static str> {
        match self {
            Self::Dnf => vec!["dnf", "install", "-y"],
            Self::Zypper => vec!["zypper", "--non-interactive", "install", "--allow-unsigned-rpm"],
            Self::Rpm => vec!["rpm", "-U"],
        }
    }

    /// The same, for a person to run with sudo; `file` is already quoted.
    fn manual(self, file: &str) -> String {
        match self {
            Self::Dnf => format!("sudo dnf install {file}"),
            Self::Zypper => format!("sudo zypper install --allow-unsigned-rpm {file}"),
            Self::Rpm => format!("sudo rpm -U {file}"),
        }
    }
}

/// A program and its arguments, never a shell line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl InstallCommand {
    fn new(program: &str, args: &[&str]) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(|a| a.to_string()).collect(),
        }
    }

    /// The command as a shell line, for the log.
    pub fn display(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .map(quote)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// `s` quoted for a POSIX shell when it needs it.
pub fn quote(s: &str) -> String {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_./=+:@%,".contains(&b)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// What installs `file`, a download already checked against SHA256SUMS
/// with the digest `sha256`:
///
/// - deb, rpm, arch: pkexec runs [`ROOT_INSTALL`], which copies the file
///   somewhere only root can write, checks the copy against `sha256` once
///   more, and only then hands *the copy* to `apt install -y`, dnf/zypper/rpm,
///   or `pacman -U --noconfirm`. The
///   download sits in a folder this user owns, and the polkit prompt can stay
///   up for as long as the person takes to type: checking the user's file
///   and then installing it by path would let any process running as this
///   user swap in another package in between, and have root install it.
///   The path must be absolute: pkexec does not keep the working directory;
/// - nsis: the installer itself, with its own UI; msi: `msiexec /i <file>`;
///   the caller stages a fresh, re-verified copy first (`Updates::install`);
/// - dmg: [`DMG_INSTALL`] over `bundle`, the running `.app` (none outside one);
/// - none for a build from source, or a digest that is not one.
pub fn install_command(kind: InstallKind, file: &Path, sha256: &str, bundle: Option<&Path>) -> Option<InstallCommand> {
    file.file_name()?;
    match kind {
        InstallKind::Deb | InstallKind::Rpm | InstallKind::Arch if !file.is_absolute() || !is_sha256(sha256) => None,
        InstallKind::Deb => Some(root_install(file, sha256, "deb", &["apt", "install", "-y"])),
        InstallKind::Rpm => Some(rpm_install_command(file, sha256, RpmTool::detect())),
        // pacman fetches the dependencies the package names from the sync
        // repositories, as `apt install` does, and installs unsigned local
        // files (LocalFileSigLevel = Optional, the default).
        InstallKind::Arch => Some(root_install(file, sha256, "pkg.tar.zst", &["pacman", "-U", "--noconfirm"])),
        InstallKind::Nsis => Some(InstallCommand::new(&text(file), &[])),
        InstallKind::Msi => Some(InstallCommand::new("msiexec", &["/i", &text(file)])),
        InstallKind::Dmg => Some(InstallCommand::new(
            "/bin/sh",
            &["-c", DMG_INSTALL, "zapper-update", &text(file), &text(bundle?)],
        )),
        InstallKind::Unpackaged => None,
    }
}

/// 64 lowercase hex digits, as `release::sha256_file` writes them. Checked
/// before it becomes an argument, though it is never part of a script's text.
fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn rpm_install_command(file: &Path, sha256: &str, tool: RpmTool) -> InstallCommand {
    root_install(file, sha256, "rpm", &tool.install_args_without_file())
}

fn root_install(file: &Path, sha256: &str, ext: &str, tool: &[&str]) -> InstallCommand {
    let file = text(file);
    let mut args = vec!["/bin/sh", "-c", ROOT_INSTALL, "zapper-install", &file, sha256, ext];
    args.extend_from_slice(tool);
    InstallCommand::new("pkexec", &args)
}

/// The privileged half of a Linux install, run by pkexec as root. Every value
/// arrives as an argument — the user's file, the digest zapper verified, the
/// extension, then the package manager's command — and none is spliced into
/// the text. It copies the file into a fresh directory of root's (`install
/// -o root`, so nothing this user runs can touch the copy), checks the copy
/// against the digest and stops if it differs, installs the copy, and always
/// removes the directory. The directory is 0755 so apt's unprivileged
/// download sandbox can read the file instead of warning about it.
pub const ROOT_INSTALL: &str = r#"set -eu
src=$1 sum=$2 ext=$3
shift 3
d=$(mktemp -d)
trap 'rm -rf "$d"' EXIT
chmod 0755 "$d"
pkg="$d/zapper-update.$ext"
install -m 0644 -o root -g root "$src" "$pkg"
printf '%s  %s\n' "$sum" "$pkg" | sha256sum -c --status - || { echo "The package changed after zapper checked it, so it was not installed." >&2; exit 3; }
"$@" "$pkg""#;

/// A `.dmg` update, as a script of its two arguments (never spliced into it):
/// the verified image and the running bundle. It mounts the image read-only
/// and unbrowsed (no Finder window), copies its zapper.app with `ditto`
/// beside the bundle under a hidden name, swaps the two by renaming within one
/// folder, and detaches. A failed swap puts the old bundle back. The running
/// process keeps its open files; the new bundle takes effect on restart.
pub const DMG_INSTALL: &str = r#"set -eu
dmg=$1 app=${2%/}
dir=${app%/*} name=${app##*/}
new="$dir/.$name.new.$$" old="$dir/.$name.old.$$"
mnt=$(mktemp -d "${TMPDIR:-/tmp}/zapper-update.XXXXXX")
finish() { rm -rf "$new"; hdiutil detach -quiet "$mnt" 2>/dev/null || hdiutil detach -quiet -force "$mnt" 2>/dev/null || true; rmdir "$mnt" 2>/dev/null || true; }
trap finish EXIT
hdiutil attach -nobrowse -readonly -noautoopen -mountpoint "$mnt" "$dmg" >/dev/null
[ -d "$mnt/zapper.app" ] || { echo "The disk image has no zapper.app." >&2; exit 1; }
ditto "$mnt/zapper.app" "$new"
mv "$app" "$old"
mv "$new" "$app" || { mv "$old" "$app"; exit 1; }
rm -rf "$old""#;

/// The `.app` bundle `exe` runs from (`<bundle>/Contents/MacOS/<exe>`), if any.
pub fn app_bundle(exe: &Path) -> Option<PathBuf> {
    let contents = exe
        .parent()
        .filter(|d| d.file_name().is_some_and(|n| n == "MacOS"))?
        .parent()?;
    let bundle = contents
        .parent()
        .filter(|_| contents.file_name().is_some_and(|n| n == "Contents"))?;
    bundle
        .extension()
        .is_some_and(|e| e == "app")
        .then(|| bundle.to_path_buf())
}

/// Whether this user can replace `bundle`: write into it and into its folder.
/// Probed by writing rather than by reading permission bits, which say nothing
/// about ACLs, a read-only mount or a translocated copy.
pub fn bundle_replaceable(bundle: &Path) -> bool {
    let probe = |dir: &Path| {
        let p = dir.join(format!(".zapper-update-probe-{}", std::process::id()));
        let ok = std::fs::write(&p, b"").is_ok();
        let _ = std::fs::remove_file(&p);
        ok
    };
    bundle.is_dir() && probe(bundle) && bundle.parent().is_some_and(probe)
}

/// The command for a person to run when the privileged one could not run (no
/// pkexec, the password prompt was cancelled): `sudo apt install <file>`,
/// the RPM's with this system's tool, `sudo pacman -U <file>`, or
/// `open <file>` for a `.dmg`, which
/// shows the image with its Applications link to drag to.
pub fn manual_command(kind: InstallKind, file: &Path) -> Option<String> {
    let f = quote(&text(file));
    match kind {
        InstallKind::Deb => Some(format!("sudo apt install {f}")),
        InstallKind::Rpm => Some(RpmTool::detect().manual(&f)),
        InstallKind::Arch => Some(format!("sudo pacman -U {f}")),
        InstallKind::Dmg => Some(format!("open {f}")),
        _ => None,
    }
}

/// Runs `argv` once process `pid` (this one) has exited, so the new version
/// does not start while the old one still holds the window, the database and
/// mpv. `argv` is the binary, or `open <bundle>` on macOS, where the binary
/// run directly would get no Dock icon. Everything is an argument, never part
/// of the script.
pub fn relaunch_command(pid: u32, argv: &[&Path]) -> InstallCommand {
    let pid = pid.to_string();
    let argv: Vec<String> = argv.iter().map(|p| text(p)).collect();
    let mut args = vec![
        "-c",
        r#"p=$1; shift; while kill -0 "$p" 2>/dev/null; do sleep 0.2; done; exec "$@""#,
        "zapper-relaunch",
        &pid,
    ];
    args.extend(argv.iter().map(String::as_str));
    InstallCommand::new("/bin/sh", &args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "zapper-install-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn reads_the_kind_from_where_each_package_puts_it() {
        let root = tempdir();
        // A build from source: nothing anywhere.
        let dev = root.join("target/debug/zapper");
        std::fs::create_dir_all(dev.parent().unwrap()).unwrap();
        assert_eq!(detect_install_kind(Some(&dev)), InstallKind::Unpackaged);
        assert_eq!(detect_install_kind(None), InstallKind::Unpackaged);

        // The Linux packages: /usr/bin/zapper with the marker in /usr/lib/zapper.
        let usr = root.join("usr");
        std::fs::create_dir_all(usr.join("bin")).unwrap();
        std::fs::create_dir_all(usr.join("lib/zapper")).unwrap();
        let exe = usr.join("bin/zapper");
        for (text, kind) in [
            ("deb\n", InstallKind::Deb),
            ("rpm\n", InstallKind::Rpm),
            ("arch\n", InstallKind::Arch),
            ("nsis\r\n", InstallKind::Nsis),
            ("msi", InstallKind::Msi),
            ("dmg\n", InstallKind::Dmg),
            ("snap", InstallKind::Unpackaged),
            ("", InstallKind::Unpackaged),
        ] {
            std::fs::write(usr.join("lib/zapper").join(INSTALL_KIND_FILE), text).unwrap();
            assert_eq!(detect_install_kind(Some(&exe)), kind, "{text:?}");
        }

        // macOS: inside the bundle's Resources.
        let app = root.join("Applications/zapper.app/Contents");
        std::fs::create_dir_all(app.join("MacOS")).unwrap();
        std::fs::create_dir_all(app.join("Resources")).unwrap();
        std::fs::write(app.join("Resources").join(INSTALL_KIND_FILE), "dmg\n").unwrap();
        assert_eq!(detect_install_kind(Some(&app.join("MacOS/zapper"))), InstallKind::Dmg);

        // Windows: beside the executable.
        let win = root.join("Programs/zapper");
        std::fs::create_dir_all(&win).unwrap();
        std::fs::write(win.join(INSTALL_KIND_FILE), "nsis\r\n").unwrap();
        assert_eq!(detect_install_kind(Some(&win.join("zapper.exe"))), InstallKind::Nsis);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_markers_in_the_repository_say_what_their_names_say() {
        let packaging = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging");
        for (file, kind) in [
            ("install-kind-deb", InstallKind::Deb),
            ("install-kind-rpm", InstallKind::Rpm),
            ("install-kind-arch", InstallKind::Arch),
            ("install-kind-msi", InstallKind::Msi),
            ("install-kind-dmg", InstallKind::Dmg),
        ] {
            let text = std::fs::read_to_string(packaging.join(file)).unwrap();
            assert_eq!(InstallKind::parse(&text), Some(kind), "{file}");
            assert_eq!(kind.as_str(), text.trim());
        }
    }

    /// sha256("hello")
    const SUM: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    #[test]
    fn builds_each_kinds_install_command() {
        // Absolute on every platform: `/home/…` is not, on Windows, and the
        // Linux kinds refuse a relative path.
        let deb_buf = std::env::temp_dir().join("zapper").join("updates").join("zapper_0.3.0_amd64.deb");
        let deb = deb_buf.as_path();
        let c = install_command(InstallKind::Deb, deb, SUM, None).unwrap();
        assert_eq!(c.program, "pkexec");
        assert_eq!(c.args[..3], ["/bin/sh".to_string(), "-c".into(), ROOT_INSTALL.into()]);
        assert_eq!(
            c.args[3..],
            ["zapper-install", &deb.display().to_string(), SUM, "deb", "apt", "install", "-y"],
            "the file, the digest and the tool are arguments, never part of the script"
        );
        assert!(
            install_command(InstallKind::Deb, Path::new("zapper_0.3.0_amd64.deb"), SUM, None).is_none(),
            "pkexec does not keep the working directory"
        );
        for bad in ["", "abc", &SUM.to_uppercase(), &format!("{}; reboot", &SUM[..56])] {
            assert!(install_command(InstallKind::Deb, deb, bad, None).is_none(), "{bad:?}");
        }
        assert_eq!(
            manual_command(InstallKind::Deb, deb).unwrap(),
            format!("sudo apt install {}", quote(&deb.display().to_string()))
        );
        if cfg!(unix) {
            // The ordinary case, spelled out: a plain path needs no quotes.
            let plain = Path::new("/home/u/.cache/zapper/updates/zapper_0.3.0_amd64.deb");
            assert_eq!(
                manual_command(InstallKind::Deb, plain).unwrap(),
                "sudo apt install /home/u/.cache/zapper/updates/zapper_0.3.0_amd64.deb"
            );
        }

        let exe = Path::new(r"C:\Users\u\AppData\Local\zapper\updates\zapper_0.3.0_x64-setup.exe");
        let c = install_command(InstallKind::Nsis, exe, SUM, None).unwrap();
        assert_eq!((c.program.as_str(), c.args.len()), (exe.to_str().unwrap(), 0));
        let msi = Path::new(r"C:\Users\u\AppData\Local\zapper\updates\zapper_0.3.0_x64_en-US.msi");
        let c = install_command(InstallKind::Msi, msi, SUM, None).unwrap();
        assert_eq!(c.args, vec!["/i".to_string(), msi.to_str().unwrap().into()]);
        assert_eq!(manual_command(InstallKind::Msi, msi), None);
        assert_eq!(install_command(InstallKind::Unpackaged, deb, SUM, None), None);

        let dmg = Path::new("/Users/u/Library/Caches/zapper/updates/zapper_0.3.0_aarch64.dmg");
        let app = Path::new("/Applications/zapper.app");
        let c = install_command(InstallKind::Dmg, dmg, SUM, Some(app)).unwrap();
        assert_eq!((c.program.as_str(), c.args[1].as_str()), ("/bin/sh", DMG_INSTALL));
        assert_eq!(
            c.args[2..],
            ["zapper-update".to_string(), dmg.display().to_string(), "/Applications/zapper.app".into()],
            "the paths are arguments, never part of the script"
        );
        assert_eq!(install_command(InstallKind::Dmg, dmg, SUM, None), None, "not running from a bundle");
    }

    /// [`ROOT_INSTALL`] itself, run as this user with `install` stood in for
    /// (the real one's `-o root` needs root) and `sh -c 'cat …'` as the
    /// package manager: the copy is what gets installed, a changed file is
    /// refused before the tool runs, and the root directory is always gone.
    #[cfg(unix)]
    #[test]
    fn the_root_side_installs_a_verified_copy_and_refuses_a_swapped_file() {
        let root = tempdir();
        let bin = root.join("bin");
        std::fs::create_dir(&bin).unwrap();
        // The last two arguments are the source and the destination.
        let fake_install = "#!/bin/sh\nfor a in \"$@\"; do prev=$last; last=$a; done\ncp \"$prev\" \"$last\"\n";
        std::fs::write(bin.join("install"), fake_install).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(bin.join("install"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
        let log = root.join("installed");
        // "The package manager": records the path it was given and the content there.
        let tool = format!(r#"echo "$1" > '{0}'; cat "$1" >> '{0}'"#, log.display());
        let run = |file: &Path| {
            std::process::Command::new("/bin/sh")
                .args(["-c", ROOT_INSTALL, "zapper-install"])
                .arg(file)
                .args([SUM, "deb", "/bin/sh", "-c", &tool, "pm"])
                .env("PATH", &path)
                .env("TMPDIR", &root)
                .output()
                .unwrap()
        };

        let good = root.join("good.deb");
        std::fs::write(&good, b"hello").unwrap();
        let out = run(&good);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let logged = std::fs::read_to_string(&log).unwrap();
        let (installed, content) = logged.split_once('\n').unwrap();
        assert_ne!(installed, good.display().to_string(), "the copy is installed, not the user's file");
        assert!(installed.ends_with("/zapper-update.deb"), "{installed}");
        assert_eq!(content, "hello");
        assert!(!Path::new(installed).exists(), "the private directory is removed");

        std::fs::remove_file(&log).unwrap();
        let swapped = root.join("swapped.deb");
        std::fs::write(&swapped, b"evil").unwrap();
        let out = run(&swapped);
        assert_eq!(out.status.code(), Some(3));
        assert!(String::from_utf8_lossy(&out.stderr).contains("changed after zapper checked it"));
        assert!(!log.exists(), "the package manager never ran");
        let left: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("tmp."))
            .collect();
        assert!(left.is_empty(), "no temporary directory left behind: {left:?}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_rpm_installs_with_dnf_else_zypper_else_rpm() {
        let rpm = Path::new("/home/u/.cache/zapper/updates/zapper-0.3.0-1.x86_64.rpm");
        let f = rpm.display().to_string();
        assert_eq!(RpmTool::pick(|t| t == "dnf" || t == "zypper"), RpmTool::Dnf);
        assert_eq!(RpmTool::pick(|t| t == "zypper"), RpmTool::Zypper);
        assert_eq!(RpmTool::pick(|_| false), RpmTool::Rpm);
        for (tool, args, manual) in [
            (RpmTool::Dnf, vec!["dnf", "install", "-y"], format!("sudo dnf install {f}")),
            (
                RpmTool::Zypper,
                vec!["zypper", "--non-interactive", "install", "--allow-unsigned-rpm"],
                format!("sudo zypper install --allow-unsigned-rpm {f}"),
            ),
            (RpmTool::Rpm, vec!["rpm", "-U"], format!("sudo rpm -U {f}")),
        ] {
            let c = rpm_install_command(rpm, SUM, tool);
            assert_eq!(c.program, "pkexec");
            assert_eq!(c.args[2], ROOT_INSTALL);
            assert_eq!(c.args[4..7], [f.clone(), SUM.into(), "rpm".into()]);
            assert_eq!(c.args[7..], args.iter().map(|a| a.to_string()).collect::<Vec<_>>(), "{tool:?}");
            assert_eq!(tool.manual(&quote(&f)), manual);
        }
        assert!(install_command(InstallKind::Rpm, Path::new("zapper-0.3.0-1.x86_64.rpm"), SUM, None).is_none());
    }

    #[test]
    fn an_arch_package_installs_with_pacman_through_the_verified_copy() {
        let pkg_buf = std::env::temp_dir().join("zapper").join("updates").join("zapper-0.3.0-1-x86_64.pkg.tar.zst");
        let pkg = pkg_buf.as_path();
        let f = pkg.display().to_string();
        let c = install_command(InstallKind::Arch, pkg, SUM, None).unwrap();
        assert_eq!(c.program, "pkexec");
        assert_eq!(c.args[..3], ["/bin/sh".to_string(), "-c".into(), ROOT_INSTALL.into()]);
        assert_eq!(
            c.args[3..],
            ["zapper-install", &f, SUM, "pkg.tar.zst", "pacman", "-U", "--noconfirm"],
            "the file, the digest and the tool are arguments, never part of the script"
        );
        assert!(InstallKind::Arch.uses_pkexec());
        assert!(!InstallKind::Arch.quits_to_install());
        assert!(
            install_command(InstallKind::Arch, Path::new("zapper-0.3.0-1-x86_64.pkg.tar.zst"), SUM, None).is_none(),
            "pkexec does not keep the working directory"
        );
        assert!(install_command(InstallKind::Arch, pkg, "abc", None).is_none());
        assert_eq!(manual_command(InstallKind::Arch, pkg).unwrap(), format!("sudo pacman -U {}", quote(&f)));
        if cfg!(unix) {
            let plain = Path::new("/home/u/.cache/zapper/updates/zapper-0.3.0rc.1-1-x86_64.pkg.tar.zst");
            assert_eq!(
                manual_command(InstallKind::Arch, plain).unwrap(),
                "sudo pacman -U /home/u/.cache/zapper/updates/zapper-0.3.0rc.1-1-x86_64.pkg.tar.zst"
            );
        }
    }

    #[test]
    fn finds_the_bundle_the_binary_runs_from() {
        assert_eq!(
            app_bundle(Path::new("/Applications/zapper.app/Contents/MacOS/zapper")),
            Some(PathBuf::from("/Applications/zapper.app"))
        );
        for not in ["/usr/bin/zapper", "/x/target/debug/zapper", "/x/zapper/Contents/MacOS/zapper"] {
            assert_eq!(app_bundle(Path::new(not)), None, "{not}");
        }
    }

    #[test]
    fn a_bundle_is_replaceable_only_where_it_exists_and_can_be_written() {
        let root = tempdir();
        let app = root.join("zapper.app");
        assert!(!bundle_replaceable(&app), "no bundle");
        std::fs::create_dir(&app).unwrap();
        assert!(bundle_replaceable(&app));
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1, "the probes are cleaned up");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_path_with_spaces_or_quotes_is_quoted_for_the_shell() {
        let f = Path::new("/home/o'neil/my cache/zapper_0.3.0_amd64.deb");
        assert_eq!(
            manual_command(InstallKind::Deb, f).unwrap(),
            r"sudo apt install '/home/o'\''neil/my cache/zapper_0.3.0_amd64.deb'"
        );
    }

    #[cfg(unix)]
    #[test]
    fn relaunch_waits_for_this_process_then_starts_the_binary() {
        let c = relaunch_command(4242, &[Path::new("/usr/bin/zapper")]);
        assert_eq!(c.program, "/bin/sh");
        assert_eq!(
            c.args[2..],
            ["zapper-relaunch".to_string(), "4242".into(), "/usr/bin/zapper".into()]
        );
        let mac = relaunch_command(1, &[Path::new("/usr/bin/open"), Path::new("/Applications/zapper.app")]);
        assert_eq!(mac.args[4..], ["/usr/bin/open".to_string(), "/Applications/zapper.app".into()]);
        // A pid that is not running: the wait ends at once and the whole argv
        // runs, arguments and all.
        let out = std::process::Command::new(&c.program)
            .args(&c.args[..2])
            .args(["x", "999999999", "echo", "it's", "two words"])
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout), "it's two words\n");
    }
}
