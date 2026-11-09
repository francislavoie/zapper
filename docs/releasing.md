# Releasing zapper

A release is a tag. `just release <version>` prepares the release commit and tag locally;
pushing the tag starts [.github/workflows/release.yml](../.github/workflows/release.yml), which
builds `zapper-server` (the static Linux binary and a Windows zip), checks them, and uploads them
with `SHA256SUMS` to a **draft** GitHub release. You review the draft and publish it. Nothing is
published automatically.

This line ships `zapper-server` and the browser interface it serves: the desktop app is being
rebuilt, so there are no installers or distribution packages here. The Tauri app's releases (0.4.x:
`.deb`, `.rpm`, the Arch package, the NSIS setup and the MSI) are made from the
[`main-tauri`](https://github.com/francislavoie/zapper/tree/main-tauri) branch, which keeps its own
copy of this document and workflow.

The release is one version, the `version` in `[workspace.package]` in the root `Cargo.toml`
(`python3 scripts/version.py current` prints it); the tag, the binaries and the CHANGELOG section
all use it.

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
   [crates/zapper-core/src/updates/mod.rs](../crates/zapper-core/src/updates/mod.rs):

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

- **main must be green.** CI (`ci.yml`) runs the core, mpv and server jobs (Rust tests and
  clippy, the frontend build and checks, and the release scripts' tests) on every push. The
  player checks (`just check --host`, which needs gnome-shell and mpv) only run locally: run them on
  the commit you're releasing if the player or the mpv backend changed.
- **CHANGELOG.md's `## [Unreleased]` section lists the release's changes.** It becomes the
  release notes, which the update dialog shows to everyone updating. `just release` refuses an
  empty section.

## Steps

1. **Prepare the release** on an up-to-date main, with no uncommitted changes:

   ```sh
   just release 0.2.0
   ```

   It sets the version in `Cargo.toml` (the workspace's) and `Cargo.lock` (zapper's own entry),
   and `package.json` and `package-lock.json`. Then
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
   the workspace version, when `RELEASES_REPO` isn't this repository, or when CHANGELOG.md
   has no section for the version.

4. **Review the draft** under Releases. It's titled "zapper 0.2.0", has the changelog section as
   its notes, and carries `zapper-server-0.2.0-linux-x86_64`, `zapper-server-0.2.0-windows-x86_64.zip`
   and `SHA256SUMS`. A version with a pre-release part (`-rc.1`) is marked pre-release. Edit the
   notes if needed, and try the binary from the draft (`./zapper-server-0.2.0-linux-x86_64
   --version`; packaging/server/README.md).

5. **Publish** the draft.

## The updater

The update code (`crates/zapper-core/src/updates/`) is kept for the app that comes back: it reads
`GET https://api.github.com/repos/<RELEASES_REPO>/releases`, picks the asset by a fixed name,
downloads it, and refuses anything `SHA256SUMS` doesn't list. `zapper-server` doesn't update
itself, so nothing installed from this line looks for these releases yet; a new binary replaces
the old one by hand (packaging/server/README.md).

The release still keeps what that code needs, so the next app can use it unchanged:

- `RELEASES_REPO` in [crates/zapper-core/src/updates/mod.rs](../crates/zapper-core/src/updates/mod.rs) is the
  one repository releases come from, and the workflow fails a tagged build whose `RELEASES_REPO`
  isn't the repository it runs in (`scripts/releases-repo.sh` reads it);
- every release has `SHA256SUMS`, merged from each job's, and the tag stays `v<version>`;
- the asset names are an API: `zapper-server-<version>-linux-x86_64` (one static executable) and
  `zapper-server-<version>-windows-x86_64.zip` (`zapper-server.exe`, the libmpv and FFmpeg DLLs it
  loads, `licenses\` and `LICENSE`, in a folder of that name).

## Versions

A release version is [SemVer](https://semver.org) without build metadata: `X.Y.Z` or
`X.Y.Z-<pre-release>`, where the pre-release is a lowercase word then optional dot-separated
words or numbers: `0.2.0-rc.1`, `1.0.0-beta.2`. The rules are in `scripts/version.py`, and the
updater compares versions the same way
([crates/zapper-core/src/updates/version.rs](../crates/zapper-core/src/updates/version.rs)).

The version rules are SemVer's order everywhere: `scripts/version.py cmp` compares two versions,
`prerelease` says whether one is a pre-release, and `deb` writes the Debian form (`0.2.0~rc.1`),
which `scripts/test-scripts.sh` checks against `dpkg --compare-versions` so that the order stays
the one a package manager would use.

## What the workflow does

1. **Plan:** the tag to release (a pushed tag, or Run workflow's input) and its commit; no tag
   is a dry run.
2. **Server** (`ubuntu-24.04`, read-only): checks the tag against the workspace version
   (`scripts/version.py current`) and `RELEASES_REPO` against the repository, extracts the release
   notes, writes the Rust notices and builds the interface, then
   `scripts/build-server-static.sh --with-checks` in a pinned Rust image on Alpine: the minimal
   static FFmpeg (`build-ffmpeg-static.sh`, cached on `sources.txt` and the script), then
   zapper-server for musl with it and its notices built in, the browser feed's tests against that
   FFmpeg (an AC-3 fixture transcoded to AAC), and `recording_check` for the same target.
   `scripts/check-server-binary.sh` then checks it is static, runs it in bare `debian:stable-slim`
   and `alpine` containers and from scratch, builds and runs the sample `Dockerfile`, and records
   the fixture streams through it (docs/recording.md, "Without the app"). Writes `SHA256SUMS` and
   keeps it, with the notes, as a workflow artifact for 3 days.
3. **Windows** (`windows-latest`, read-only, beside the Server job): checks the tag against the
   workspace version, fetches libmpv (`tools/fetch-libmpv.ps1`), writes the Rust license notices,
   builds the interface and `zapper-server.exe`, and zips it with the DLLs it loads and the
   notices. It unzips that and runs it: `--version` must say the release's version, `--licenses`
   must print notices, and the libmpv and FFmpeg DLLs must be beside it. Writes `SHA256SUMS` and
   keeps the zip as a workflow artifact for 3 days. The release waits for it.
4. **Release** (`contents: write`): verifies each job's checksums and merges them into one
   `SHA256SUMS`, attests the files' build provenance (public repositories, or `ZAPPER_ATTEST`),
   and creates the draft release, or updates it on a re-run.

`zapper-server` takes its FFmpeg from zapper's own LGPL build (docs/licensing.md, "mpv and
FFmpeg"): linked in statically on Linux, the DLLs beside it on Windows. The Linux binary needs
nothing installed; the Windows zip needs only Windows.

## Windows and macOS

Windows is released as `zapper-server-<version>-windows-x86_64.zip`: every tag and every dry run
builds and checks it, and a release is drafted only when it passes (docs/windows.md). It is
unsigned, so SmartScreen warns on its first run; signing needs a certificate and a signing step in
the `windows` job.

macOS has no release job. `.github/workflows/macos.yml` (weekly and on pull requests that touch
macOS; docs/macos.md) builds libmpv for macos-arm64, runs the core and server tests, and installs
and removes `zapper-server` as a LaunchAgent, so the port keeps working for when an app returns.

## Dry run

Actions → Release → **Run workflow**, with the tag left empty (or
`gh workflow run release.yml --ref <branch>`). It builds and checks the static server and the
Windows zip from the branch's version and keeps them as workflow artifacts (3 days),
`zapper-<version>-server` and `zapper-<version>-windows`. It creates no release. A missing CHANGELOG section or a placeholder `RELEASES_REPO` is only a warning.

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

When attestations run, anyone can check a file came from this workflow:

```sh
gh attestation verify zapper-server-0.2.0-linux-x86_64 --repo <owner>/zapper
```
