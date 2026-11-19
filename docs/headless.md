# Headless rendering

Status: Current. How to render photos without the desktop app's window: on a
server, in a container or from a script.

`rawmakase render` and `rawmakase render-batch` develop a photo with an edit and
write the result without opening a window. They need no display server
(X11 or Wayland) and no GPU: exports render on the CPU pipeline, which is the
engine's reference. The GUI libraries are loaded only when a window opens, so a
render runs with an empty environment. For a running desktop app driven by
scripts instead, see [Automation](automation.md).

```sh
rawmakase render photo.ARW out.jpg --xmp preset.xmp --max-edge 2400 --quality 85
```

See the [command line](../README.md#command-line) for every edit source and
option, and [XMP presets](xmp-presets.md) for what a preset carries.

## What a server needs

- **The binary.** The Linux tarball (`rawmakase-<version>-<arch>-linux.tar.gz`),
  or the DEB or RPM package. In the tarball, `usr/bin/rawmakase` is a launcher
  for `usr/lib/rawmakase/rawmakase`; keep the folder together, as ONNX Runtime
  (`libonnxruntime.so`, for AI Denoise) sits beside the binary.
- **A new enough system.** The Linux build needs glibc 2.39 or newer: Ubuntu
  24.04+, Debian 13+, Fedora 43+. AWS Lambda's own runtimes (Amazon Linux 2023,
  glibc 2.34) are too old: use a container-image function on one of those
  bases.
- **Memory.** A full-size render of a 24–60 MP RAW needs several gigabytes;
  `--max-edge` reduces the render, not the decode.
- **No GPU, no display server, no desktop session.**

## Folders

| What | Where on Linux | Moved by |
| --- | --- | --- |
| Profiles, lens profiles, Lensfun, presets, preferences (raw defaults) | `$XDG_DATA_HOME/rawmakase` (default `~/.local/share/rawmakase`) | `RAWMAKASE_DATA_DIR` |
| AI Denoise's model (`models/`) and an ONNX Runtime the app downloaded (`runtime/`) | `$XDG_DATA_HOME/rawmakase` | Not `RAWMAKASE_DATA_DIR`: this computer's own data folder only |
| Caches (decoded photos, AI Denoise results, Camera Look fits) | `$XDG_CACHE_HOME/rawmakase` (default `~/.cache/rawmakase`) | `RAWMAKASE_CACHE_DIR` |

Set `RAWMAKASE_DATA_DIR` and `RAWMAKASE_CACHE_DIR` explicitly in a container,
and make the cache folder writable. A render that starts from no saved edit
uses the raw defaults chosen in Preferences; with no preferences in the data
folder, the defaults a fresh install has. Pass `--recipe` for a render that
depends on nothing saved.

## Camera profiles

No Adobe profiles are bundled. A photo without an imported profile renders with
RAWmakase Color, and a preset that names a profile not imported is refused
(`Missing or unsupported enhanced profile`). Import them once, when the image
is built:

```sh
export RAWMAKASE_DATA_DIR=/opt/rawmakase/data
rawmakase import-profiles /path/to/profiles/        # DCPs, look XMPs, .cube and HaldCLUT .png LUTs
rawmakase import-lens-profiles /path/to/lcp/        # Adobe .lcp lens profiles
rawmakase lensfun install                           # Lensfun's community lens database
```

Import only profiles you are licensed to use, and don't publish an image that
contains Adobe's.

Two kinds of XMP file look alike:

- A **develop preset** (sliders, curves, and optionally a `crs:Look` naming a
  profile) goes to `render --xmp`.
- A **look profile** (an XMP with an embedded `RGBTable` or `LookTable`) is a
  profile: import it with `import-profiles`. Passed to `--xmp`, it is refused
  with `Unsupported preset settings: RGBTable`.

## AI Denoise

An edit with AI Denoise on needs its model, else the render fails (`AI Denoise
is on but its model is not installed`). The command line can't install it:
install it from the app on a computer, then copy that computer's
`~/.local/share/rawmakase/models` folder, its `verified.txt` receipts
included, to `$XDG_DATA_HOME/rawmakase/models` in the image. The tarball's
bundled ONNX Runtime is found beside the binary.

## Writing to S3 and other storage

RAWmakase writes local files only. Render to a local path, then copy it with a
tool built for it, such as `rclone` or the AWS CLI, which handle multipart
uploads, checksums, retries and credentials for S3-compatible storage (AWS,
Cloudflare R2, Backblaze B2, MinIO and others):

```sh
rawmakase render "$in" /tmp/out.jpg --xmp look.xmp --max-edge 2400 --overwrite
rclone copyto /tmp/out.jpg "remote:bucket/renders/$(basename "$in" .ARW).jpg"
```

## A container

```dockerfile
FROM ubuntu:24.04
ARG VERSION
ADD rawmakase-${VERSION}-x86_64-linux.tar.gz /opt/rawmakase/
ENV PATH=/opt/rawmakase/usr/bin:$PATH \
    RAWMAKASE_DATA_DIR=/opt/rawmakase/data \
    RAWMAKASE_CACHE_DIR=/tmp/rawmakase-cache
COPY profiles/ /tmp/profiles/
RUN rawmakase import-profiles /tmp/profiles && rm -rf /tmp/profiles
```

Each render is its own process, so a queue worker runs one command per photo;
starting the process costs little next to developing a RAW. `render-batch`
develops a photo once for several presets or sizes.

## Limits

- One process per render: there is no server mode or HTTP API.
- No S3 output: copy the files with another tool (above).
- The Linux build's glibc 2.39 floor rules out older distributions and
  Lambda's own runtimes.
- The binary is the desktop app's; there is no separate build without the GUI
  code, which it carries but doesn't load.

What it would take to go further (a headless binary, server mode, S3 output)
is in the [headless plan](headless-plan.md).
