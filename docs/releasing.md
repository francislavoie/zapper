# Releasing zapper

A release is a tag. `just release <version>` prepares the release commit and tag locally;
pushing the tag starts [.github/workflows/release.yml](../.github/workflows/release.yml), which
builds the `.deb` and the `.rpm`, checks them, and uploads them with `SHA256SUMS` to a **draft**
GitHub release. You review the draft and publish it. Nothing is published automatically.

Installed copies of zapper find a published release by themselves (the updater, below), so
publishing is the moment a release reaches people: everything before it is reversible.

Releases carry Linux packages only: no Arch package, no AppImage, no signing. Windows and macOS
are wired in but disabled
([Windows and macOS](#windows-and-macos)).

## One-time setup

Until this is done, `just release` refuses to run, and the updater in any build reports "Updates aren't configured" and never makes a request.

1. **Create the GitHub repository** and push main to it:

   ```sh
   gh repo create <owner>/zapper --private --source . --push
   ```

   (Or create it on github.com and `git remote add origin git@github.com:<owner>/zapper.git &&
   git push -u origin main`.) Public or private both work for releases, but an installed
   zapper checks for updates **without a token**, so on a private repository the releases API
   answers 404 and every copy reports a failed check. Make the repository public before the
   first release that people will install, or accept that updates only work once it is.

2. **Set the releases repository** in one place, `RELEASES_REPO` in
   [src-tauri/src/updates/mod.rs](../src-tauri/src/updates/mod.rs):

   ```rust
   pub const RELEASES_REPO: &str = "<owner>/zapper";
   ```

   Commit it. This is what every build checks for updates and downloads them from, and the only
   repository a download may come from. The release workflow fails a tagged build whose
   `RELEASES_REPO` isn't the repository it runs in (`scripts/releases-repo.sh` reads it), so a
   fork or a renamed repository can't ship installs that look for updates in the wrong place.

3. **Actions settings** (Settings → Actions → General): allow GitHub Actions, and leave the
   default `GITHUB_TOKEN` permissions at read-only — the workflows ask for `contents: write`
   only in the job that drafts the release.

4. **Secrets: none.** The draft is created with the workflow's own `GITHUB_TOKEN`. Optional:
   the repository variable `ZAPPER_ATTEST` = `true` turns on build provenance attestations for a
   private repository on a plan that supports them (a public repository always gets them).

5. **Run a dry run** (Actions → Release → Run workflow, tag empty) to see the build pass on a
   runner before the first real tag.

## Before you start

- **main must be green.** CI (`ci.yml`) runs the Rust tests, clippy, the frontend build and
  checks, and the release scripts' tests on every push. The windowed harnesses (`just check
  --windowed`, which needs a display and mpv) only run locally: run them on the commit you're
  releasing if surface, playback or rect behaviour changed.
- **CHANGELOG.md's `## [Unreleased]` section lists the release's changes.** It becomes the
  release notes, which the update dialog shows to everyone updating. `just release` refuses an
  empty section.

## Steps

1. **Prepare the release** on an up-to-date main, with no uncommitted changes:

   ```sh
   just release 0.2.0
   ```

   It sets the version in `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` and
   `src-tauri/Cargo.lock` (zapper's own entry), and `package.json` and `package-lock.json`. Then
   it turns `## [Unreleased]` into `## [0.2.0] - <today>` under a fresh, empty `## [Unreleased]`,
   commits "Release 0.2.0" and creates the annotated tag `v0.2.0`. It never pushes.

   It refuses to run when `RELEASES_REPO` is still the placeholder, when you're not on main,
   when tracked files have uncommitted changes, when main is behind `origin/main`, or when the
   version is invalid, already tagged, or not newer than every `v*` tag. If a step fails partway,
   it restores the files it edited. To undo a release you haven't pushed:
   `git tag -d v0.2.0 && git reset --hard HEAD~1`.

2. **Push the commit and the tag:**

   ```sh
   git push origin main v0.2.0
   ```

3. **Wait for the workflow:** Actions → Release. It fails fast when the tag doesn't match
   `tauri.conf.json`'s version, when `RELEASES_REPO` isn't this repository, or when CHANGELOG.md
   has no section for the version.

4. **Review the draft** under Releases. It's titled "zapper 0.2.0", has the changelog section as
   its notes, and carries the `.deb`, the `.rpm` and `SHA256SUMS`. A version with a pre-release
   part (`-rc.1`) is marked pre-release. Edit the notes if needed, and install the packages from
   the draft to try them (`sudo apt install ./zapper_0.2.0_amd64.deb`).

5. **Publish** the draft. Installed copies see it at their next check (a minute after they
   start, then daily) or when someone chooses Check for updates.

## The updater

zapper updates itself without Tauri's updater plugin
([src-tauri/src/updates/](../src-tauri/src/updates/)):

1. **Check:** `GET https://api.github.com/repos/<RELEASES_REPO>/releases`, unauthenticated
   (GitHub allows 60 an hour per address; zapper makes about one a day). Nothing about the user
   is sent; the user agent says `zapper/<version>`. Drafts are never offered. A pre-release is
   offered only to a copy that is itself a pre-release. The newest release above the running
   version wins.
2. **Pick the asset** by its fixed name for this install's kind (the table below). A build
   from source has no kind and is only offered the release page.
3. **Download** it into `~/.cache/zapper/updates/` (a `0700` folder, emptied first). The URL
   must be under `https://github.com/<RELEASES_REPO>/releases/download/`, and may only redirect
   to GitHub's asset storage hosts.
4. **Verify** it against the same release's `SHA256SUMS`. Not listed, or a different digest:
   the file is deleted and never run. The digest is checked again just before installing.
5. **Install:**
   - `.deb` and `.rpm`: pkexec runs a short script as root (`install::ROOT_INSTALL`; the
     polkit prompt names `/bin/sh`) that copies the download into a fresh root-owned directory,
     checks the copy against the verified digest, and only then runs `apt install -y` (or dnf,
     else zypper, else `rpm -U`) on the copy. The download sits in a folder this user owns, so
     this is what stops another process running as this user from swapping in a different
     package while the password prompt is up;
   - Windows (not built yet): copies the installer into a folder made for that attempt, checks
     it again, runs it (the NSIS installer, or `msiexec /i` for the MSI) and quits so it can
     replace the files. This narrows the swap window but can't close it: there is no
     privilege boundary to copy across first. Code signing is the real fix;
   - macOS (not built yet): mounts the `.dmg` and swaps its `zapper.app` for the running one,
     when this user can write there; otherwise says to drag it to Applications.

   If pkexec is missing or the password prompt is cancelled, the dialog shows the command to
   run in a terminal (`sudo apt install …`) instead.
6. **Restart:** starts the new binary once this process has exited.

The automatic check is the setting Settings → Updates → "Check for updates automatically", on by
default. Check for updates (menu, command palette, or that pane) always asks, whatever the
setting says.

### Asset names are an API

Installed copies find their update by these names
([src-tauri/src/updates/release.rs](../src-tauri/src/updates/release.rs)). They are Tauri's own
bundle names; keep them exactly, and keep `SHA256SUMS` in every release: a package
`SHA256SUMS` doesn't list is never installed.

| Install | Asset |
|---|---|
| `.deb` | `zapper_<version>_amd64.deb` |
| `.rpm` (Fedora, openSUSE) | `zapper-<version>-1.x86_64.rpm` (SemVer in the name; the package's own Version is `0.2.0~rc.1` for a pre-release) |
| Windows NSIS (not built yet) | `zapper_<version>_x64-setup.exe` |
| Windows MSI (not built yet) | `zapper_<version>_x64_en-US.msi` |
| macOS (not built yet) | `zapper_<version>_aarch64.dmg` (an Intel Mac would look for `_x64.dmg`) |
| all | `SHA256SUMS` (`sha256sum` output listing each package) |

The tag stays `v<version>`, and the release's body (the CHANGELOG section) is what the update
dialog shows.

### How a copy knows how it was installed

The `.deb` and the `.rpm` hold the same binary, so each package carries a one-word marker the
app reads at run time, `install-kind`
([src-tauri/src/updates/install.rs](../src-tauri/src/updates/install.rs)):

- the `.deb` ships `/usr/lib/zapper/install-kind` = `deb` and the `.rpm` the same path = `rpm`,
  from [packaging/](../packaging/) through `bundle.linux.deb.files` and `bundle.linux.rpm.files`
  in tauri.conf.json. Not in `/usr/bin` beside the binary, where it would be a command.
  `scripts/check-deb.sh` fails a `.deb` without it;
- Windows (not built yet): `packaging/windows/installers.conf.json` ships `install-kind` = `msi`
  beside `zapper.exe` as a resource, and the NSIS installer's post-install hook
  (`packaging/windows/installer-hooks.nsh`) overwrites it with `nsis`;
- macOS (not built yet): `packaging/macos/dmg.conf.json` puts `install-kind` = `dmg` in
  `zapper.app/Contents/Resources`.

No marker: a build from source (`npm run app`, or `npx tauri build` on Windows and macOS without
those configs), which is only shown the release page. A `.deb` or `.rpm` you build locally with
`just package` *does* carry the marker, and updates like a released one.

## Versions

A release version is [SemVer](https://semver.org) without build metadata: `X.Y.Z` or
`X.Y.Z-<pre-release>`, where the pre-release is a lowercase word then optional dot-separated
words or numbers: `0.2.0-rc.1`, `1.0.0-beta.2`. The rules are in `scripts/version.py`, and the
updater compares versions the same way
([src-tauri/src/updates/version.rs](../src-tauri/src/updates/version.rs)).

Each package manager gets a version that sorts the same way, pre-releases before the release:

| SemVer (tag `v…`) | `.deb` Version | `.rpm` Version | Files |
|---|---|---|---|
| `0.2.0-rc.1` | `0.2.0~rc.1` | `0.2.0~rc.1` | `zapper_0.2.0-rc.1_amd64.deb`, `zapper-0.2.0-rc.1-1.x86_64.rpm` |
| `0.2.0` | `0.2.0` | `0.2.0` | `zapper_0.2.0_amd64.deb`, `zapper-0.2.0-1.x86_64.rpm` |

Tauri writes the SemVer form into the `.deb`, where dpkg would sort `0.2.0-rc.1` *after*
`0.2.0`; `scripts/fix-deb.sh` rewrites only that field (`just package` runs it). Tauri already
writes the `~` form into the `.rpm`. `scripts/test-scripts.sh` checks the order with
`dpkg --compare-versions`.

## What the workflow does

1. **Plan:** the tag to release (a pushed tag, or Run workflow's input) and its commit; no tag
   is a dry run.
2. **Linux** (`ubuntu-24.04`, read-only): checks the tag against `tauri.conf.json`'s version and
   `RELEASES_REPO` against the repository, extracts the release notes, installs
   the Linux build dependencies (README.md), stable Rust, Node from `.nvmrc` and `just`, then runs
   `just package` with `ZAPPER_RELEASE_VERSION` set: the frontend build, the release binary, the
   `.deb` and `.rpm`, `fix-deb.sh` and `check-deb.sh`. Writes `SHA256SUMS` and keeps everything as
   a workflow artifact for 3 days.
3. **Windows and macOS:** skipped (below).
4. **Release** (`contents: write`): verifies each job's checksums and merges them into one
   `SHA256SUMS`, attests the packages' build provenance (public repositories, or
   `ZAPPER_ATTEST`), and creates the draft release, or updates it on a re-run.

The packages' `Depends` (`.deb`) and `Requires` (`.rpm`) are fixed in tauri.conf.json, not
derived from the runner: `libmpv2 | libmpv1`, `libwebkit2gtk-4.1-0`, `libgtk-3-0` for the
`.deb`; the same libraries by soname for the `.rpm` (`libmpv.so.2()(64bit)` and so on), which
resolves on Fedora and openSUSE alike. The binary itself links the runner's glibc, so the
packages need Ubuntu 24.04, Debian 13, Fedora 40 or later.

## Windows and macOS

Windows builds and links (docs/windows.md: libmpv comes from `tools/fetch-libmpv.ps1`, which
the `windows` job runs), but has not been tested on screen or released; macOS has only a stub
surface. What is in place:

- the updater already knows the NSIS, MSI and `.dmg` installs (asset names, install commands,
  the bundle swap), and its tests cover them;
- `packaging/windows/` and `packaging/macos/` hold the install-kind wiring, and
  `just package-windows` / `just package-macos` build with it;
- `release.yml` has `windows` and `macos` jobs that run only on Run workflow with **Also build
  Windows and macOS** ticked. A tag push skips them. The macOS one fails until that port
  exists, and a failure holds that run's draft back.

Neither Windows installer has been tested end to end, and the macOS packaging is untested.
Things to settle when the port lands:

- an MSI needs a numeric version: a pre-release needs `bundle.windows.wix.version` set
  (for example, every build of `X.Y.Z` mapped to `X.Y.Z.0`);
- the installers are unsigned (SmartScreen warns), and the `.app` is ad-hoc signed and not
  notarized (Gatekeeper asks once): code signing needs a certificate and a signing hook in the
  bundle config;
- turn the jobs on for tags: drop `inputs.windows_macos == true` from their `if:`.

## Dry run

Actions → Release → **Run workflow**, with the tag left empty. It builds and checks the
packages from the branch's version and keeps them as a workflow artifact (3 days). It creates no
release. A missing CHANGELOG section or a placeholder `RELEASES_REPO` is only a warning.

## Re-running and fixing

- **A transient failure** (a download, an apt mirror): "Re-run failed jobs". Re-running only
  the release job reuses the run's artifacts.
- **Re-running for the same tag** updates the draft's title, notes and pre-release flag, and
  replaces its assets (`gh release upload --clobber`). If the release is already published, the
  job fails and leaves it untouched.
- **A broken release that needs a code fix:** delete the draft, delete the tag
  (`git push origin :refs/tags/v0.2.0` and `git tag -d v0.2.0`), fix it on main, and release
  again. Once a version is published, don't reuse it: installed copies may already have it.
  Release the next one.

## Build provenance

When attestations run, anyone can check a package came from this workflow:

```sh
gh attestation verify zapper_0.2.0_amd64.deb --repo <owner>/zapper
```
