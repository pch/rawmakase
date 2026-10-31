<h1><img src="packaging/icons/rawmakase.svg" width="48" height="48" align="top" alt=""> RAWmakase</h1>

*/raw·muh·KAH·say/, RAW + omakase*

RAWmakase is a fast, non-destructive RAW photo developer for Linux, macOS and Windows, written in Rust. It develops RAW, JPEG, PNG, TIFF, WebP, AVIF, HEIF/HEIC and JPEG XL files with a Lightroom-style set of controls and exports JPEG, 16-bit TIFF or PNG (exporting to WebP, AVIF, HEIF and JPEG XL is not available yet). It can also import a Lightroom Classic catalog with its ratings, flags, labels, keywords and compatible develop settings, without ever writing to the original catalog or your photos.

![RAWmakase Develop view with presets, the photo, and the Basic and Tone Curve panels](docs/images/screenshot-2026-10-13-1111.png)

<p align="center">
  <a href="https://github.com/pch/rawmakase/releases/latest"><strong>⬇ Download the latest version</strong></a><br>
  <sub>macOS (Apple Silicon, Intel) · Linux (.deb, .rpm, Arch) · Windows (x86_64, ARM64) · <a href="#install">install notes</a></sub>
</p>

It is a personal project in active development. It develops photos from almost every current camera, including monochrome DNGs (such as the Leica Monochrom's), linear DNGs (such as iPhone ProRAW) and Lightroom's HDR and panorama merges; the exceptions are Sigma's Foveon cameras, GoPro's GPR files, the Pentax K-3 Mark III Monochrome's PEFs (its DNGs open), and Adobe's older lossy DNGs (JPEG-compressed, as Adobe DNG Converter's "lossy compression" writes them). Rendering aims for close, not exact, Lightroom parity; see [parity gaps](docs/parity-gaps.md).

> [!WARNING]
> **RAWmakase is early software in active development.** The rendering engine may change from one version to the next, so photos you have already edited can look different after an upgrade. Until 1.0, backwards compatibility is limited and not guaranteed. The contents of your originals are never modified, but keep exports of finished work.

## Features

- **Develop**: white balance and picker, one-click Auto tone and Auto white balance, exposure and tone, Shadows/Highlights, Clarity, Dehaze, point curves and levels, HSL color mixer, three-way color grading, detail (denoise and sharpening), crop, straighten and Transform, lens corrections, effects and calibration.
- **Spot removal and masks (experimental, early)**: Heal and Clone spots and brushed areas with automatic sources, and brush, gradient and range masks with local adjustments, also imported from Lightroom. Not yet measured against Lightroom.
- **Library**: SQLite catalogs, folders, ratings, flags, color labels, filtering, and non-destructive Lightroom `.lrcat` import with folder relinking.
- **Presets and profiles**: 26 built-in presets and your Lightroom XMP presets; RAWmakase's own Standard and Color profiles for every camera with a usable color matrix, plus DCP and XMP camera profiles you import yourself, and `.cube` LUTs imported as looks, as Camera Raw makes them.
- **Non-destructive**: the contents of your originals are never modified. Edits live in the catalog, and all writes are atomic.
- **Fast previews**: every change renders at viewport size, with the color and tone stage on the GPU (Metal on macOS, Vulkan on Linux, DirectX 12 on Windows) and a CPU fallback.
- **Command line**: inspect, render, export thumbnails, import catalogs and profiles, and benchmark without the GUI.
- **External control**: [MIDI controllers](docs/midi.md), [scripts](docs/automation.md), and a built-in [MCP server](docs/mcp.md) for agents to adjust the running editor, inspect previews, save and export.
- **Updates**: the app checks GitHub for new releases; the Apple Silicon and Windows installer builds update themselves, other installs are pointed at the release page.

## Coming soon

- AI object removal
- Agentic features: e.g. culling assistance

## Install

Download the package for your system from [GitHub Releases](https://github.com/pch/rawmakase/releases). Older releases may have only the original Arch-built Linux archive; use the requirements in that release's notes.

### Let your agent install it

Point your coding agent to [GitHub Releases](https://github.com/pch/rawmakase/releases) and tell it to install the latest version for your operating system, or give it this prompt:

```text
Install the latest release of RAWmakase for my operating system from https://github.com/pch/rawmakase/releases. Pick the right package for my OS and CPU architecture, verify it against the release's checksums, install it, and tell me how to launch it.
```

### Omarchy

Install `rawmakase-bin` from **Omarchy menu → Install → Package**, or from a terminal:

```sh
omarchy pkg add rawmakase-bin
```

It updates with the rest of the system.

Omarchy makes windows slightly transparent by default, which changes how photos look. To keep RAWmakase fully opaque, add this to your Hyprland `looknfeel.lua`:

```lua
-- Keep RAWmakase fully opaque (opt out of Omarchy's default window opacity)
o.window("^RAWmakase$", { tag = "-default-opacity", opacity = "1 1" })
```

### macOS (15 or newer)

Install with Homebrew:

```sh
brew install --cask pch/tap/rawmakase
```

Update with `brew upgrade --cask rawmakase`.

Or install the DMG directly:

Choose `rawmakase-v<version>-macos-arm64.dmg` for Apple Silicon or `rawmakase-v<version>-macos-x86_64.dmg` for Intel. Open the DMG, drag **RAWmakase** into **Applications**, and launch it there. Release DMGs are signed and notarized, and include their imaging libraries; Homebrew is not required.

### Linux (x86_64 and aarch64)

Download the package for your architecture (`amd64`/`x86_64` or `arm64`/`aarch64`), then run the matching command from its folder, replacing the filename with the one you downloaded:

| System | Package | Install |
| --- | --- | --- |
| Ubuntu 24.04+ / Debian 13+ | `.deb` | `sudo apt install ./rawmakase_<version>_amd64.deb` |
| Fedora 43+ | `.rpm` | `sudo dnf install ./rawmakase-<version>-1.x86_64.rpm` |
| Arch Linux (x86_64) | `.pkg.tar.zst` | `sudo pacman -U ./rawmakase-<version>-1-x86_64.pkg.tar.zst` |

Arch packages use system dependencies. A working graphics driver is required; install your desktop's `xdg-desktop-portal` backend for native file dialogs.

**AUR publication is paused.** Install the Arch package directly, or (on Arch Linux ARM too) download and extract `rawmakase-<version>-arch-recipe.tar.gz` into an empty folder and build as a normal user:

```sh
makepkg -si
```

The development recipe is in [packaging/arch/rawmakase-git](packaging/arch/rawmakase-git/PKGBUILD).

The `rawmakase-<version>-<arch>-linux.tar.gz` download contains the same bundled imaging libraries as the DEB/RPM packages. Extract it and run `./usr/bin/rawmakase` from the extracted folder. Keep the whole directory together. It requires the same OS/runtime baseline as the packages above; it is not a fully static build.

The `rawmakase-<version>-<arch>.AppImage` download is the same build as a single file: make it executable (`chmod +x`) and run it. It needs the same OS baseline as the packages above (Ubuntu 24.04+, Debian 13+, Fedora 43+; glibc 2.39 or newer), so it does not run on older distributions. On systems without FUSE 2, run it with `APPIMAGE_EXTRACT_AND_RUN=1`. It carries update information, so AppImageUpdate, Gear Lever and similar tools can update it to the newest release, downloading only what changed.

### Windows (10 or newer on x86_64, 11 on ARM64)

Run `rawmakase-v<version>-x86_64-pc-windows-msvc-setup.exe`, or `rawmakase-v<version>-aarch64-pc-windows-msvc-setup.exe` on an ARM64 PC. It installs for your user account only, without administrator rights, and adds RAWmakase to the Start menu. The installer is not code-signed yet, so Microsoft Defender SmartScreen may warn about an unrecognized app: choose **More info → Run anyway**. For a copy without installing, extract `rawmakase-v<version>-<arch>-pc-windows-msvc.zip` and run `rawmakase.exe` from the extracted folder.

### Updates and verification

RAWmakase checks GitHub for a newer release after launch and once an hour, and shows a notice when one is out (turn the check off in Preferences). The Apple Silicon DMG and the Windows installer download and install the update themselves; Intel Macs, the Linux tarball and AppImage, the Windows archive and package-managed installs (Homebrew, DEB, RPM, Arch) are pointed at the release page or their package manager. Settings and catalogs are kept separately from the installed application, so reinstalling over an old version loses nothing.

Each new packaged release includes `SHA256SUMS`. After downloading it beside your package, verify downloaded files on Linux with `sha256sum --ignore-missing -c SHA256SUMS`. On macOS, use `shasum -a 256 <downloaded-file>` and compare the result with that file's entry in `SHA256SUMS`.

### From source (Linux and macOS)

You need Rust 1.98 or newer and a C compiler (a few Rust crates, such as the catalog's bundled SQLite, compile C); no RAW or colour-management library is needed.

- Arch Linux: `sudo pacman -S rust base-devel`
- macOS: the Xcode command-line tools (`xcode-select --install`). Use a native rustup toolchain (`aarch64-apple-darwin` on Apple Silicon).

```sh
make                                  # cargo build --release --locked
make install PREFIX="$HOME/.local"    # binary, desktop entry, icon and licenses
```

`make install` never builds, so `make && sudo make install PREFIX=/usr` does not compile as root. `make uninstall` removes the installed files.

HEIC photos are decoded by RAWmakase's own HEVC decoder. HEVC is patented, and some distributions leave software HEVC decoders out: `make CARGO_FLAGS=--no-default-features` builds without it, and HEIF photos are then listed, with their metadata, but not shown or edited (see [dependencies](docs/dependencies.md)).

On macOS, `packaging/macos/app.sh` builds `target/release/RAWmakase.app`, which you can open from Finder. It uses the Homebrew libraries installed on your Mac and is not a signed, self-contained distribution.

## Usage

```sh
rawmakase                     # reopen the last catalog
rawmakase Photos.rawmakase    # open a catalog
rawmakase photo.dng           # add the photo's folder to the last catalog and edit it
```

Photos are edited through the Library. The first launch starts a catalog named Photos in RAWmakase's data folder and asks for a folder of photos; to keep your Lightroom Classic folders, ratings and edits, import its catalog instead. Both, and other catalogs, are available later from the **Catalog** menu. Folders dropped onto the window, one or several, are added to the open catalog; right-click a folder in the Library's Folders panel to remove it and its photos from the catalog (its files stay on disk), or select photos and press Delete to move their files to the Trash or just remove them from the catalog. Drag photos onto a folder, or use Move to Folder… in their menu, to move their files there; New Folder Inside… in a folder's menu makes a folder and can move the selection into it. F2 renames a photo, or several by a template in the order they were taken; Rename Folder… renames a folder, and a folder dragged onto another (or Move Folder…) moves there. A photo dropped there or passed on the command line has its folder added, then opens in Develop; edits it got in earlier releases (its `photo.rawmakase.json`) come along. You can also drop a catalog onto the window. The camera's embedded JPEG shows immediately while the RAW develops. A JPEG, PNG, TIFF, WebP, AVIF, HEIF or JPEG XL opens in Develop like a RAW: untouched it looks as the file does, and its white balance is Lightroom's relative Temp and Tint (−100 to 100) rather than Kelvin. WebP, AVIF, HEIF and JPEG XL are read only for now, and Develop does not edit HDR (PQ or HLG), animated, tiled AVIF or 4:2:2 and monochrome HEIF files; HDR and animated ones still show in the Library.

Useful shortcuts:

| Key | Action |
| --- | --- |
| Left / Right | Previous / next photo |
| F / Z | Fit / toggle Fit and 100% |
| 0–5, 6–9 | Rating; red, yellow, green, blue label |
| P / X / U | Pick / reject / clear flag |
| Shift + rating, label or flag key | Apply and advance |
| R or C | Crop |
| J | Shadow and highlight clipping warnings (click a histogram triangle for one) |
| Backslash | Before alone |
| Shift+Backslash | Camera thumbnail: the embedded JPEG, labelled, in place of the photo |
| Y / Option+Y / Shift+Y | Before and after: left and right / top and bottom / split |
| I | Photo info over the photo: Info 1, Info 2, off |
| Shift+R | Reference View: another photo beside the one you edit (drag it from the filmstrip) |
| Tab / Shift+Tab | Hide or show the side panels / also the filmstrip and status bar |
| F7 / F8 / F6 | Hide or show the left panel / right panel / filmstrip |
| Cmd/Ctrl+Z, Cmd/Ctrl+Shift+Z | Undo / redo |
| Cmd/Ctrl+Shift+U | Auto tone |

Double-click a slider or a color grading wheel to reset it, type its value for precision, or hover a slider and press Up or Down (Shift for ten steps). As in Lightroom, `.` and `,` select the next or previous Basic slider (or click its name), and `=` and `-` step it by Lightroom's amounts (Shift for more, Option for less), even with the panel folded. On a grading wheel, Shift keeps a drag to hue or saturation and Cmd/Ctrl moves it finely. Drag sideways in the histogram to move Blacks, Shadows, Exposure, Highlights or Whites, whichever region you start in.

Tab hides the side panels to give the photo the window, and Shift+Tab the filmstrip too; pressed again, each brings back the panels it hid. The panel buttons at either end of the top bar hide or show the left panel, the right panel or the filmstrip alone.

The switch in a panel's header turns the panel off without losing its settings, as in Lightroom; right-click a header for Solo Mode, where opening one panel closes the others on that side.

### Camera profiles

Every camera with a usable color matrix gets two profiles of RAWmakase's own: **RAWmakase Standard** (the camera's color matrix with the DNG default tone curve) and **RAWmakase Color** (a mild look on top of it). A new photo uses a compatible imported Adobe Color, then Adobe Standard, then a DNG's own profile, and RAWmakase Color when it has none of those. No Adobe profiles are bundled. For Lightroom's color, import DCP and XMP profiles you are licensed to use (for example from your own Lightroom or Camera Raw installation, or published third-party DCPs such as RawTherapee's) from the **Profile** menu in Develop, from **Preferences**, or with `rawmakase import-profiles`. The same import takes `.cube` LUTs, which become look profiles with a Profile Amount, as Camera Raw's Create Profile makes them, for LUTs made for sRGB, Adobe RGB, Display P3, ProPhoto RGB or Rec. 2020. On a Mac or PC, the Profile menu also looks in Camera Raw's profile folder and, when it finds profiles filed under the camera's name, offers to import them in one click. Imported profiles are copied into RAWmakase's own data directory. See [Lightroom profiles](docs/lightroom-profiles.md).

### Command line

```sh
rawmakase inspect photo.dng
rawmakase thumbnail photo.dng embedded.jpg
rawmakase render photo.dng edited.jpg --exposure 0.7 --max-edge 2400
rawmakase render photo.dng edited.tiff --xmp preset.xmp
rawmakase render photo.dng edited.jpg --auto
rawmakase import-catalog Lightroom.lrcat Photos.rawmakase
rawmakase fingerprint *.NEF        # decode facts per file as JSON lines, for regression checks
rawmakase help
```

`render` starts from the photo's default look, or from an edit a release before 0.1.8 saved as a sidecar (`photo.dng.rawmakase.json` beside the photo, or in the data directory's `sidecars/` folder, whichever is newer). `--recipe` replaces that starting point with a saved RAWmakase preset; `--xmp` applies an XMP preset on top of it, so settings the XMP leaves out keep the starting point's values. For a render that depends on nothing saved, pass `--recipe`. It does not read edits from a catalog yet, so exporting Develop edits is done from the app. It needs `--overwrite` to replace an existing file.

### Where data lives

- Catalogs, with every edit: the `.rawmakase` file you choose.
- Edits saved beside photos by releases before 0.1.8 (`photo.dng.rawmakase.json`, with spots and masks in `photo.dng.rawmakase-local.json`, or in the data directory's `sidecars/` folder for read-only locations) are brought into the catalog when you add their folder, and left as they are.
- Profiles, presets, previews and session state: `~/Library/Application Support/RAWmakase` on macOS, `$XDG_DATA_HOME/rawmakase` (default `~/.local/share/rawmakase`) on Linux, `%APPDATA%\RAWmakase` on Windows. `RAWMAKASE_DATA_DIR` overrides it.

Exports are always sRGB. The display defaults to sRGB; pick a monitor ICC profile under **More** only if your compositor does not already manage color.

## Development

Start with the [code map](docs/code-map.md) and the [architecture guide](docs/architecture.md).
The [website source and deployment guide](website/README.md) live in `website/`.
The [usage stats service](stats/README.md) in `stats/` still counts reports from older releases; the app no longer sends them.

```sh
make check    # cargo fmt --check, clippy -D warnings, cargo test
```

The repository contains no RAW photos, Lightroom catalogs or camera profiles, so tests that need them are ignored by default. Run them with your own files:

| Environment variable | Test target | Needs |
| --- | --- | --- |
| `RAWMAKASE_FIXTURES` | `--test raw_fixtures` | A folder of RAW files (the test currently expects both a Bayer and an X-Trans file) |
| `RAWMAKASE_PROFILES` | `--test private_profiles` | A folder of DCP files |
| `RAWMAKASE_TEST_DCP` | `--lib camera_profiles` | A DCP file (the assertions currently match RawTherapee's `SONY ILCE-7M2.dcp`) |
| `RAWMAKASE_LRCAT` | `--lib catalog` | A Lightroom catalog |
| `RAWMAKASE_CORPUS` | `--test color` | The private tier of the [color corpus](tests/corpus/README.md): your own RAWs and their Camera Raw renders |

```sh
RAWMAKASE_FIXTURES=~/raw-fixtures cargo test --release --test raw_fixtures -- --ignored --nocapture
```

The public part of the color corpus runs with every `cargo test`: synthetic chart DNGs rendered on the CPU and compared with committed snapshots and with Camera Raw's renders of the same charts, so a color change beyond its tolerances (ΔE00 0.5 per patch, 0.1 on a case's mean) is noticed. The GPU preview path is covered by the hardware tests below, not by this suite. When a change is intended, `RAWMAKASE_BLESS=1 cargo test --test color` rewrites the snapshots, and also the charts and the Camera Raw baseline (`tests/corpus/camera-raw/baseline.json`). Read the baseline diff on its own before committing: it records how far renders are from Camera Raw, and accepting a larger distance should be a decision, not a side effect. Commit the files with the reason.

GPU tests are ignored as well; run them with `cargo test -p rawmakase-engine --lib gpu -- --ignored` on a machine with a compute adapter.

Application CI runs `make check`, a release build, an Arch package build and a `cargo deny` license and advisory audit. Website-only pushes and pull requests run the Hugo build instead, and stats-only changes run the stats service's tests; edits to either workflow also run the workflow linter. A stable `vX.Y.Z` tag on `main` matching `Cargo.toml` builds both macOS DMGs, the Linux packages and the Windows installer. Publication waits for Apple notarization and package checks, then refreshes the website's download links. See [packaging/RELEASING.md](packaging/RELEASING.md) for credentials, rehearsal runs, supported systems and the AUR pause.

## License

RAWmakase is released under the [MIT License](LICENSE). The DNG default tone curve and temperature table come from the Adobe DNG SDK, under the license in [licenses/Adobe-DNG-SDK.txt](licenses/Adobe-DNG-SDK.txt). The interface font, Inter, is under the SIL Open Font License ([licenses/Inter-OFL.txt](licenses/Inter-OFL.txt)), and the icons are Lucide's, under the ISC License ([licenses/Lucide-ISC.txt](licenses/Lucide-ISC.txt)). egui's bundled fallback fonts are compiled into the binary as well: Hack ([licenses/Hack-MIT-BitstreamVera.txt](licenses/Hack-MIT-BitstreamVera.txt)), Noto Emoji ([licenses/NotoEmoji-OFL.txt](licenses/NotoEmoji-OFL.txt)), Ubuntu Light ([licenses/Ubuntu-UFL.txt](licenses/Ubuntu-UFL.txt)) and the emoji icon font ([licenses/emoji-icon-font-MIT.txt](licenses/emoji-icon-font-MIT.txt)). The Rust dependencies and ONNX Runtime keep their own licenses; see [dependencies](docs/dependencies.md).

Adobe, Lightroom and Camera Raw are trademarks of Adobe Inc. RAWmakase is not affiliated with or endorsed by Adobe.
