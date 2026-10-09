# Package releases

The release workflow builds stable `vMAJOR.MINOR.PATCH` tags already on `main`.
The tag must match `Cargo.toml`. All CI, package installation checks and Apple
notarization must pass before anything is published. No AUR pushes occur.

## Downloads

| Platform | Artifact | Requirements |
| --- | --- | --- |
| macOS Apple Silicon | `rawmakase-vVERSION-macos-arm64.dmg` | macOS 15+ |
| macOS Intel | `rawmakase-vVERSION-macos-x86_64.dmg` | macOS 15+ |
| Debian / Ubuntu x86_64 and arm64 | `.deb` | Ubuntu 24.04+ or Debian 13+ |
| Fedora x86_64 and aarch64 | `.rpm` | Fedora 43+ |
| Arch x86_64 | `.pkg.tar.zst` | Current Arch system dependencies |
| Linux archive | `rawmakase-VERSION-x86_64-linux.tar.gz`, `rawmakase-VERSION-aarch64-linux.tar.gz` | Same system baseline as DEB/RPM; not a universal static binary |
| Windows x86_64 installer | `rawmakase-vVERSION-x86_64-pc-windows-msvc-setup.exe` | Windows 10+; per-user install, no administrator rights |
| Windows x86_64 archive | `rawmakase-vVERSION-x86_64-pc-windows-msvc.zip` | Windows 10+ |
| Windows ARM64 installer | `rawmakase-vVERSION-aarch64-pc-windows-msvc-setup.exe` | Windows 11 on ARM64; per-user install, no administrator rights |
| Windows ARM64 archive | `rawmakase-vVERSION-aarch64-pc-windows-msvc.zip` | Windows 11 on ARM64 |

The Mac, DEB and RPM packages contain private imaging libraries. Mac users do
not need Homebrew. The Linux tarball contains the same `/usr` layout, including
private libraries; run the extracted `usr/bin/rawmakase` or install the whole
tree under `/usr`. Do not copy just its executable. Linux still needs system
Vulkan/graphics drivers, window-system libraries and a working file-dialog portal.

Each Windows architecture's installer and archive hold the same folder: `rawmakase.exe`, with
LibRaw, Little CMS and the C runtime linked in statically (so no Visual C++
Redistributable is needed), and the licenses. Neither is code-signed yet, so SmartScreen warns on first run;
signing can be added later as an `after_package` hook on the `windows-amd64`
and `windows-arm64` targets, before checksums are recorded. Both installers
share one `AppId`, so installing either replaces the other.

The current local `packaging/macos/app.sh` remains a development helper using
Homebrew dependencies. It does not produce the standalone release app.

## Apple credentials

Set these **repository Actions secrets**:

- `APPLE_CERTIFICATE_P12`: base64 `.p12` with the Developer ID Application certificate and private key.
- `APPLE_CERTIFICATE_PASSWORD`: the `.p12` export password.
- `APPLE_SIGNING_IDENTITY`: full `Developer ID Application: Name (TEAMID)` identity.
- `APPLE_ID`: the Apple Account email used for notarization.
- `APPLE_TEAM_ID`: that developer team's ID.
- `APPLE_APP_PASSWORD`: the Apple Account app-specific password.

The release step explicitly requires all six. `native-packages` 0.7.0 signs
nested code and the app in a temporary keychain, creates and signs the DMG,
submits it with `notarytool`, waits for acceptance, and staples the ticket.
The subsequent check mounts the actual DMG, verifies its ticket, Gatekeeper
acceptance, architecture and library paths, and runs the bundled CLI.
Checksums are generated after signing and stapling.

No App Store listing or Developer ID Installer certificate is needed for DMGs.
Never put the export, its base64 contents or passwords into the repository.

## Update signatures

The app updates itself from the Apple Silicon DMG through
[fastframe-update](https://github.com/crmne/fastframe/tree/main/crates/fastframe-update).
Before downloading a package it verifies `checksums.txt.sig`, a raw Ed25519
signature over `checksums.txt`, against the public key embedded from
`assets/update-public-key.hex`. There is no unsigned fallback.

The publish job signs with `RAWMAKASE_UPDATE_SIGNING_KEY` (the PKCS#8 PEM
private key), a secret of the `release-signing` environment, which permits
`v*` tags and manual rebuilds from `main`. Both require a maintainer's approval;
administrator bypass is disabled. Keep a backup of the private
key outside GitHub, which never shows a secret again. Installed apps trust only
the key they were built with: losing it means asking users to download the next
release by hand, and a new key must first ship alongside the old one (see
fastframe-update's notes on rotating the publisher key).

The updater looks for `rawmakase-vVERSION-macos-arm64.dmg` and
`rawmakase-vVERSION-<x86_64|aarch64>-pc-windows-msvc-setup.exe` by name, for
the architecture of the running executable. A copy the Windows
installer set up (it writes `rawmakase-installer.txt` beside the executable)
updates by running the next release's setup program silently. Intel Macs, Linux
installs and the Windows archive are shown the release page instead. Never
rename these assets.

## Publishing and rehearsal

1. Finish and review the release commit, including the version files and
   `packaging/release-notes/vX.Y.Z.md` (see below).
2. Push that commit to `main`, then push its matching `vX.Y.Z` tag.
3. The Release workflow publishes after every required job succeeds.

For a rehearsal without a new publication, manually run **Release**, select
`main` as the workflow branch, enter an existing stable **tag** (such as
`v0.1.1`), and uncheck **publish** (enabled by default). The workflow validates and builds
the tag’s exact source commit using packaging tools from the selected workflow
branch, so tags created before this workflow can also be tested. Tags from
before Windows support (v0.1.8 and earlier) skip the Windows build.
This still builds, signs, notarizes and verifies packages, then retains them as
Actions artifacts. It does not replace any existing release assets. A normal
tag push proceeds to publication after maintainer approval. To repair packaging
for an existing tag without moving it, run **Release** from `main` with that
tag and **publish** enabled, then approve `release-signing` after all checks pass.
Publishing to an existing release attaches
the generated packages and refreshes assets with matching names, including
`SHA256SUMS`; the reviewed notes replace the release description and unrelated
assets are preserved. AUR publication
remains disabled independently of GitHub publication.

The workflow supports stable versions only. Do not push a prerelease tag with
this workflow expecting a published release.

## Writing release notes

Release notes are authored and reviewed Markdown, not generated commit lists.
Write `packaging/release-notes/vX.Y.Z.md` before tagging. The workflow fails early
if the file is missing, empty or whitespace-only, including during rehearsals.
It publishes the file verbatim for both new and existing releases. Automated
checks enforce presence; the maintainer still reviews accuracy and writing.

For a normal tag push, notes come from the tagged commit. A manual rebuild uses
notes from the selected workflow branch while building the exact tag's source.
This allows notes to be backfilled for older releases without moving their tags.
Review those notes against that tag, not against the latest application code.

1. Inspect the changes from the previous stable tag to the release commit. Read
   relevant diffs and issue/PR context so reverted or partial work is not announced
   as a shipped feature. For the first release, describe the capabilities shipped.
2. Open with a short summary of the main changes and their practical effect.
   Focus on what users can now do, what feels better, and which problems are fixed.
3. Group items under **New**, **Improved**, **Fixed**, **Changed**, or **Removed**,
   choosing only sections that have meaningful content. Use a bold result followed
   by a concise explanation; mention UI paths or upgrade actions where useful.
   Omit routine refactors and build plumbing. Packaging changes belong when they
   change installation or supported systems. Do not turn notes into a setup guide.
4. Add clearly labelled direct download links for the shipped platforms and a
   link to the full comparison with the previous tag. State important requirements,
   breaking changes, migration steps or known limitations when relevant. Credit
   implementers and reporters accurately, with PR/issue links where available;
   include a Thanks section only when there is someone specific to acknowledge.
5. For visible features, add useful screenshots or short recordings using
   synthetic or explicitly approved content. Upload them as release assets and
   use their final asset URLs in the notes. Never commit private photographs,
   catalogs, filesystem paths or credentials. Omit media when it adds no value.
6. Review every claim against shipped code and verification results. Qualify
   measured speedups with the tested conditions; do not promise universal gains.
   Verify notes, media and downloads on the published release page.

Use `packaging/release-notes/v0.1.2.md` as an example of structure, not a source
of claims to copy into later releases. No external project's release history
is required. Keep the length proportional to the changes.

To correct only a published description, edit and commit its notes file, then
publish that exact file without rebuilding or replacing packages:

```sh
gh release edit vX.Y.Z --notes-file packaging/release-notes/vX.Y.Z.md
```

## Dependency maintenance

`packaging/native-deps.sh` pins Little CMS 2.19.1 and a LibRaw master commit
(newer than 0.22.2, for the Sony A7 V) by SHA-256 and builds them into a
private prefix. A git archive has no configure script, so the build runs
`autoreconf` and needs autoconf, automake and libtool; move back to a release
tarball once LibRaw publishes one with those cameras. Update versions and
hashes together after testing. Keep JPEG/zlib support enabled so compressed DNG decoding is retained.
The app's native wrapper retains OpenMP acceleration.
Release builds define `CMS_NO_REGISTER_KEYWORD` for compatibility between the
Little CMS headers and the wrapper's C++17 compiler.

Mac bundling follows transitive dependencies, rewrites library paths, preserves
native notices and Homebrew source/version metadata, and fails on unresolved
paths or conflicting library names. JPEG and OpenMP come from the runner's
Homebrew installation; they are recorded in the app's license directory.
The layered Liquid Glass icon is compiled once with Xcode 26.3 on macOS 26;
its asset runtime crashes on the macOS 15 runners. Both Mac builds download
that `Assets.car` and pass it to `bundle.py --icon-assets`. The executables and
libraries still build on macOS 15, and each app keeps the flat icon for older
systems. The finished DMG check requires both icons and the layered icon's
Info.plist entry before accepting the package.
Linux bundles imaging dependencies, preserves their notices, and leaves core
OS/C++/OpenMP/zlib libraries to the host. The native source archives and build
script are published alongside packages; application source is also attached.

Windows builds with MSVC. `packaging/windows/deps.ps1` builds the same LibRaw
and Little CMS versions (plus libjpeg-turbo, zlib and JasPer, which LibRaw
needs) as static libraries with vcpkg, pinned to one vcpkg commit; update that
commit when the versions above change. vcpkg's own LibRaw port follows releases
only, so LibRaw comes from the overlay port in `packaging/windows/vcpkg-ports/libraw`
(vcpkg's port, MIT, with the commit and SHA-512 of the git archive changed);
delete the overlay once vcpkg's port reaches a release with the same cameras. `build.rs` finds them through vcpkg's
pkg-config files. Windows builds link the C runtime statically
(`.cargo/config.toml`) and compile the wrapper without OpenMP, so
`rawmakase.exe` imports only Windows' own DLLs: the updater runs a copy of it
alone as its helper. `packaging/windows/stage.ps1` copies the executable and
licenses and fails if the executable imports any other DLL. `deps.ps1`, `stage.ps1`
and `setup.ps1` build natively for the runner's architecture: x64 on `windows-2025`, ARM64 on
`windows-11-arm`. On ARM64, ring assembles with clang, so `deps.ps1` puts LLVM on
PATH. The `windows-amd64` and `windows-arm64` targets in `native-packages.yaml` run
`packaging/windows/setup.ps1`, which compiles `packaging/windows/rawmakase.iss`
with Inno Setup. Never change that script's `AppId`: it is how Windows tells an
update from a second installation.

Packaging uses pinned `native-packages` 0.7.0, configured in
`native-packages.yaml`. The Linux archives are built natively on x86_64 and
arm64 runners; `packaging/linux/bundle.py` stages each with its private
libraries. The release then calls `.github/workflows/packaging.yml`, which runs
native-packages' shared packaging workflow on those archives and both DMGs: it
installs the nFPM version `tool.nfpm` names, builds the DEB and RPM for each
architecture (mapping each host library to its distribution package) and
renders `packaging/homebrew/rawmakase.rb.in` with the notarized DMGs'
checksums. The release carries the resulting `rawmakase.rb` cask.

### Homebrew tap

After GitHub publication, `.github/workflows/homebrew.yml` updates
[`pch/homebrew-tap`](https://github.com/pch/homebrew-tap). Users install with
`brew install --cask pch/tap/rawmakase` and update with
`brew upgrade --cask rawmakase`.

The workflow prepares only the recipe from the published release, verifies both
DMG checksums and renders the checked-in cask template with those hashes. It
runs Homebrew style and online audits, installs the app on macOS, checks its CLI,
signature and Gatekeeper acceptance, and uninstalls it before publishing. The
`homebrew` destination in `native-packages.yaml` maps the recipe to
`Casks/rawmakase.rb`. AUR publication remains disabled.

The application repository's `HOMEBREW_TAP_SSH_KEY` Actions secret holds an SSH
private key whose public key is a write-enabled deploy key on the tap alone.
The normal `GITHUB_TOKEN` cannot push to a separate repository. No broad personal
access token or newer shared packaging workflow is needed for this publisher.

To bootstrap or retry a tap update without rebuilding or replacing release
assets, run **Publish Homebrew cask** from Actions on `main`, supplying the latest
published stable tag. Older versions and prereleases are rejected; updates are
serialized and native-packages also refuses downgrades. A release is complete
only after this workflow succeeds and the tap contains the expected version.

## AUR pause and updates

GitHub includes the Arch binary package and a recipe archive containing a
checksum-filled `PKGBUILD` and `.SRCINFO`. Install the binary with:

```sh
sudo pacman -U ./rawmakase-VERSION-1-x86_64.pkg.tar.zst
```

Or extract the recipe into an empty folder and run `makepkg -si` as a normal
user. No AUR account is required. When AUR pushes resume, add a separate opt-in
publisher with the maintainer's SSH key and verified host keys; it must not be
a prerequisite for GitHub releases.

These packages do not add in-app updates or an apt/dnf/pacman repository.
Users download new releases and install them over the previous version. Their
photo library/settings remain outside package-owned directories.

## Validation

Run `actionlint .github/workflows/release.yml`, `shellcheck packaging/*.sh packaging/macos/*.sh packaging/linux/*.sh`
and `native-packages validate` before changing the workflow. A Mac bundle can
be tested without Apple credentials using the bundler and `dmg.rb`; public
releases always require signing.

Release CI installs/removes DEB/RPM packages in clean Ubuntu 24.04, Debian 13,
Fedora 43 and Fedora 44 containers on both x86_64 and arm64 runners; checks linked and dynamically loaded GUI
libraries; and verifies removal preserves user data. Arch builds its exact
tagged source recipe, installs it, runs the CLI and loads the bundled ONNX Runtime. Windows installs the finished
setup program silently, checks the installed app's `--version` and marker, and
uninstalls it (`packaging/windows/verify.ps1`); pull requests that touch packaging
run the same build and check and keep the installer as an Actions artifact. Existing CI covers Rust
tests and dependency audits.

CLI and container checks do not validate a real desktop, Metal/Vulkan driver,
or photo development. Before announcing the first packaged release, test a
downloaded DMG on a Mac without Homebrew and the Linux packages on real desktops:
add a photo folder to the Library, preview and edit, import a catalog/profile, export JPEG
and TIFF, and upgrade while preserving settings. Test both Mac architectures,
Linux Wayland/X11, and both Windows installers and an in-app update on real x64 and ARM64 PCs. Do not claim older OS compatibility without testing the
executable and every bundled library against that baseline.
