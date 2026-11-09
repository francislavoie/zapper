//! GitHub's releases as the update check reads them: which one is an update,
//! which of its assets this install takes, and the SHA256SUMS check.
//!
//! The asset names are an API (docs/releasing.md): an installed zapper finds
//! its package in a later release by these exact names, so changing one
//! strands every install that expects the old one.

use super::install::InstallKind;
use super::version::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

/// The checksums asset every release carries, in `sha256sum`'s format.
pub const SUMS_ASSET: &str = "SHA256SUMS";

/// One release from `GET /repos/{owner}/{repo}/releases`: the fields read.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GhRelease {
    pub tag_name: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub assets: Vec<GhAsset>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GhAsset {
    pub name: String,
    #[serde(default)]
    pub size: u64,
    pub browser_download_url: String,
}

impl GhRelease {
    pub fn asset(&self, name: &str) -> Option<&GhAsset> {
        self.assets.iter().find(|a| a.name == name)
    }
}

/// The newest release newer than `current`, with its version.
///
/// Drafts never count (they are not visible without a token anyway, but a
/// test server or a future token must not change that). A pre-release counts
/// only when `current` is one: someone running an rc wants the next rc, and
/// someone on a release does not want to be moved onto one. GitHub's flag and
/// the tag's own `-rc.1` each make a release a pre-release. A tag that is not
/// a version is skipped rather than failing the check.
pub fn newest_update<'a>(releases: &'a [GhRelease], current: &Version) -> Option<(&'a GhRelease, Version)> {
    let pre_ok = current.is_prerelease();
    releases
        .iter()
        .filter(|r| !r.draft)
        .filter_map(|r| Some((r, Version::parse(&r.tag_name)?)))
        .filter(|(r, v)| pre_ok || !(r.prerelease || v.is_prerelease()))
        .filter(|(_, v)| v > current)
        .max_by(|a, b| a.1.cmp(&b.1))
}

/// The Debian architecture of this build, as the `.deb` is named.
const DEB_ARCH: &str = if cfg!(target_arch = "aarch64") { "arm64" } else { "amd64" };
/// The RPM architecture, likewise.
const RPM_ARCH: &str = if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86_64" };
/// pacman's, likewise (Arch Linux ARM names it as the RPM does).
const PACMAN_ARCH: &str = RPM_ARCH;
/// The `.dmg`'s: Apple Silicon or Intel.
const DMG_ARCH: &str = if cfg!(target_arch = "aarch64") { "aarch64" } else { "x64" };

/// The asset a `kind` install takes from the release of `version`. These are
/// the names each package has always had, kept as they are:
///
/// - deb: `zapper_<version>_amd64.deb`;
/// - rpm: `zapper-<version>-1.x86_64.rpm` (the SemVer form in the file name,
///   even though the package's own Version is `0.3.0~rc.1`);
/// - arch: `zapper-<pkgver>-1-x86_64.pkg.tar.zst`, named after the package's
///   own version as pacman's files are: the SemVer one without its `-`
///   (`0.3.0rc.1`), which pacman sorts before `0.3.0` and cannot hold a `-`
///   anyway (scripts/arch-pkg.py, pkgver);
/// - nsis: `zapper_<version>_x64-setup.exe`;
/// - msi: `zapper_<version>_x64_en-US.msi`;
/// - dmg: `zapper_<version>_aarch64.dmg` (or `_x64` on an Intel Mac);
/// - none for a build from source.
pub fn asset_name(kind: InstallKind, version: &Version) -> Option<String> {
    let v = version.without_build();
    match kind {
        InstallKind::Deb => Some(format!("zapper_{v}_{DEB_ARCH}.deb")),
        InstallKind::Rpm => Some(format!("zapper-{v}-1.{RPM_ARCH}.rpm")),
        InstallKind::Arch => Some(format!("zapper-{}-1-{PACMAN_ARCH}.pkg.tar.zst", v.replace('-', ""))),
        InstallKind::Nsis => Some(format!("zapper_{v}_x64-setup.exe")),
        InstallKind::Msi => Some(format!("zapper_{v}_x64_en-US.msi")),
        InstallKind::Dmg => Some(format!("zapper_{v}_{DMG_ARCH}.dmg")),
        InstallKind::Unpackaged => None,
    }
}

/// `sha256sum`'s output: file name to lowercase hex digest. Binary-mode names
/// (`*name`) count too; any other line is ignored rather than failing, so a
/// comment or a blank line cannot break every update.
pub fn parse_sums(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let (hash, name) = line.trim_end().split_once(char::is_whitespace)?;
            let name = name.trim_start();
            let name = name.strip_prefix('*').unwrap_or(name);
            (hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) && !name.is_empty())
                .then(|| (name.to_string(), hash.to_ascii_lowercase()))
        })
        .collect()
}

/// The file's SHA-256, lowercase hex.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SumCheck {
    /// It matches; the digest.
    Good(String),
    /// SHA256SUMS does not list it.
    Missing,
    /// It lists a different digest.
    Mismatch,
}

/// Checks `path`, the download of asset `name`, against the release's sums.
pub fn check_sum(path: &Path, name: &str, sums: &str) -> std::io::Result<SumCheck> {
    let Some(want) = parse_sums(sums).remove(name) else {
        return Ok(SumCheck::Missing);
    };
    let got = sha256_file(path)?;
    Ok(if got == want {
        SumCheck::Good(got)
    } else {
        SumCheck::Mismatch
    })
}

#[cfg(test)]
pub(crate) fn release(tag: &str, pre: bool, draft: bool) -> GhRelease {
    GhRelease {
        tag_name: tag.into(),
        name: Some(format!("zapper {}", tag.trim_start_matches('v'))),
        body: None,
        draft,
        prerelease: pre,
        html_url: format!("https://github.com/OWNER/zapper/releases/tag/{tag}"),
        published_at: None,
        assets: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    fn pick(list: &[GhRelease], current: &str) -> Option<String> {
        newest_update(list, &v(current)).map(|(r, _)| r.tag_name.clone())
    }

    #[test]
    fn offers_the_newest_release_newer_than_this_one() {
        let list = [release("v0.2.0", false, false), release("v0.3.0", false, false), release("v0.2.1", false, false)];
        assert_eq!(pick(&list, "0.2.0").as_deref(), Some("v0.3.0"));
        assert_eq!(pick(&list, "0.3.0"), None);
        assert_eq!(pick(&list, "0.4.0"), None, "a build newer than every release");
    }

    #[test]
    fn a_local_build_is_not_offered_its_own_version() {
        let list = [release("v0.2.0", false, false)];
        assert_eq!(pick(&list, "0.2.0+202610072046.d1d4d7d"), None);
        assert_eq!(pick(&list, "0.1.9+202610072046.d1d4d7d").as_deref(), Some("v0.2.0"));
    }

    #[test]
    fn drafts_and_tags_that_are_not_versions_are_skipped() {
        let list = [release("v0.9.0", false, true), release("nightly", false, false), release("v0.3.0", false, false)];
        assert_eq!(pick(&list, "0.2.0").as_deref(), Some("v0.3.0"));
    }

    #[test]
    fn the_libmpv_release_is_never_an_update() {
        // .github/workflows/libmpv.yml publishes the bundled libmpv in the same repository, as
        // a pre-release tagged libmpv-…: not a version, so not even an rc build takes it.
        let list = [release("libmpv-0.41.0-ffmpeg-8.1.3-r1", true, false), release("v0.3.0", false, false)];
        assert_eq!(pick(&list, "0.2.0").as_deref(), Some("v0.3.0"));
        assert_eq!(pick(&list, "0.4.0-rc.1"), None);
        assert_eq!(pick(&[release("libmpv-0.41.0-ffmpeg-8.1.3-r1", false, false)], "0.0.1-rc.1"), None);
    }

    #[test]
    fn pre_releases_only_reach_pre_release_builds() {
        let list = [release("v0.3.0", false, false), release("v0.4.0-rc.1", true, false)];
        assert_eq!(pick(&list, "0.2.0").as_deref(), Some("v0.3.0"));
        assert_eq!(pick(&list, "0.4.0-alpha.1").as_deref(), Some("v0.4.0-rc.1"));
        // GitHub's flag counts even on a release-looking tag, and the tag
        // counts when the flag was forgotten.
        let odd = [release("v0.5.0", true, false), release("v0.6.0-beta", false, false)];
        assert_eq!(pick(&odd, "0.2.0"), None);
        // The release of an rc's version is newer than the rc.
        assert_eq!(pick(&[release("v0.4.0", false, false)], "0.4.0-rc.1").as_deref(), Some("v0.4.0"));
    }

    #[test]
    fn each_install_kind_takes_its_package() {
        let rel = v("0.3.0");
        let rc = v("0.3.0-rc.1");
        let (deb, rpm, dmg) = if cfg!(target_arch = "aarch64") {
            ("arm64", "aarch64", "aarch64")
        } else {
            ("amd64", "x86_64", "x64")
        };
        assert_eq!(asset_name(InstallKind::Deb, &rel), Some(format!("zapper_0.3.0_{deb}.deb")));
        assert_eq!(asset_name(InstallKind::Deb, &rc), Some(format!("zapper_0.3.0-rc.1_{deb}.deb")));
        assert_eq!(asset_name(InstallKind::Rpm, &rel), Some(format!("zapper-0.3.0-1.{rpm}.rpm")));
        assert_eq!(asset_name(InstallKind::Rpm, &rc), Some(format!("zapper-0.3.0-rc.1-1.{rpm}.rpm")));
        assert_eq!(asset_name(InstallKind::Arch, &rel), Some(format!("zapper-0.3.0-1-{rpm}.pkg.tar.zst")));
        assert_eq!(asset_name(InstallKind::Arch, &rc), Some(format!("zapper-0.3.0rc.1-1-{rpm}.pkg.tar.zst")));
        assert_eq!(
            asset_name(InstallKind::Arch, &v("0.3.0-beta.2+20261031.abc")),
            Some(format!("zapper-0.3.0beta.2-1-{rpm}.pkg.tar.zst"))
        );
        assert_eq!(asset_name(InstallKind::Nsis, &rel).as_deref(), Some("zapper_0.3.0_x64-setup.exe"));
        assert_eq!(asset_name(InstallKind::Msi, &rel).as_deref(), Some("zapper_0.3.0_x64_en-US.msi"));
        assert_eq!(asset_name(InstallKind::Dmg, &rc), Some(format!("zapper_0.3.0-rc.1_{dmg}.dmg")));
        assert_eq!(asset_name(InstallKind::Unpackaged, &rel), None);
        // A stamped build asks for the release's plain name.
        assert_eq!(
            asset_name(InstallKind::Deb, &v("0.3.0+20261031.abc")),
            Some(format!("zapper_0.3.0_{deb}.deb"))
        );
    }

    #[test]
    fn reads_sha256sum_output() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let sums = parse_sums(&format!(
            "{a}  zapper_0.3.0_amd64.deb\n{b} *zapper-0.3.0-1.x86_64.rpm\nnot a line\n\n{}  short\n",
            "c".repeat(10)
        ));
        assert_eq!(sums.len(), 2);
        assert_eq!(sums["zapper_0.3.0_amd64.deb"], a);
        assert_eq!(sums["zapper-0.3.0-1.x86_64.rpm"], "b".repeat(64), "lowercased");
        // CRLF, as a Windows job would write it.
        let crlf = parse_sums(&format!("{a}  zapper_0.3.0_x64-setup.exe\r\n"));
        assert_eq!(crlf["zapper_0.3.0_x64-setup.exe"], a);
    }

    #[test]
    fn checks_a_download_against_the_sums() {
        let dir = std::env::temp_dir().join(format!("zapper-sums-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("zapper_0.3.0_amd64.deb");
        std::fs::write(&file, b"hello").unwrap();
        // sha256("hello")
        let good = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        let name = "zapper_0.3.0_amd64.deb";
        assert_eq!(check_sum(&file, name, &format!("{good}  {name}\n")).unwrap(), SumCheck::Good(good.into()));
        assert_eq!(check_sum(&file, name, &format!("{}  {name}\n", "0".repeat(64))).unwrap(), SumCheck::Mismatch);
        assert_eq!(check_sum(&file, name, &format!("{good}  zapper-0.3.0-1.x86_64.rpm\n")).unwrap(), SumCheck::Missing);
        let _ = std::fs::remove_dir_all(dir);
    }
}
