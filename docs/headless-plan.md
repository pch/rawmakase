# Headless binary, server mode and S3 output: plan

Status: Plan (2026-11-16), deferred. Nothing below is scheduled: each part waits
for someone who needs it. What can be done today is in
[Headless rendering](headless.md).

## Goal

A binary without the desktop app's GUI code that renders photos from the command
line and as a server, reads imported camera and lens profiles, and writes to local
files or S3-compatible storage; later, S3 as an Export dialog destination.

## Built

- `render` and `render-batch` run without the app (#1117): they live in
  `render_command`, which `scripts/deps-closures.txt` keeps away from `app`,
  `platform`, `catalog_session` and `edit_session`. The binary passes in what the
  desktop keeps (raw defaults and AI Denoise, as factories), a `Cancel` and where
  to print.
- AI Denoise's service and model pack are the `rawmakase-denoise` crate, given
  their model, runtime and cache folders and a `Cancel` by the caller.
- Watermark fonts no longer come through egui: `rawmakase-export` bundles Inter
  and its own font-folder lookup.
- `--quality` for JPEG output.

## Deferred

The order is the order they build on each other. A design review (Codex,
2026-11-14) found the constraints listed under each; they hold whoever builds it.

### 1. A headless binary

`render_command` and `catalog_command` are modules of the root crate, so anything
linking them still links eframe, winit and rfd.

- Move both into a crate, mechanically, with closure rules that keep it from the
  app, the GUI crates and wgpu.
- A small binary over it (`render`, `render-batch`, `import-profiles`,
  `import-lens-profiles`, `lensfun`, `inspect`, and an `install-model` that
  installs AI Denoise's pack through `rawmakase-model-install`, receipts checked).
- Built for Linux x86_64 and aarch64 in an older-glibc container (2.34: Amazon
  Linux 2023 and Lambda). The pinned ONNX Runtime needs only glibc 2.27. CI checks
  `objdump -T` of the binary and every bundled library, and runs an AI Denoise
  render on the oldest supported image. The release checklist gains its version
  pins.
- wgpu stays linked unless measured to matter. Gating the engine's GPU port means
  first separating the backend-neutral types the CPU export path shares
  (`render_preview` builds `develop::gpu::Finish`; `quality::Output` carries GPU
  frames), then a default-on feature, `default-features = false` on every headless
  edge, and a CI check in that configuration, since `crate-closures.sh` runs with
  `--all-features`.

### 2. Captured jobs

Needed only by a server that queues or caches. A request (paths, presets, options)
is resolved once, before queueing, into an immutable job: the source's identity,
the resolved recipe and its mask assets, the camera and lens profiles, Lensfun and
the raw-default presets by content, AI Denoise's model, the build and the output.
Execution uses only the job; a cache is keyed by its digest. Profiles, LCP lens
profiles (`LENS_PROFILES`, read by `photo::open`), Lensfun (`Handle::Installed`,
read by `Preset::apply`) and the presets raw defaults name are all read through
the snapshot the job captured.

### 3. Server mode

- HTTP and JSON, or a stdin worker mode for queues, one job type for both.
- Raw defaults from the server's configuration, never a desktop session.
- A recipe whose raster masks need pixels it doesn't carry is refused before it is
  queued; carrying mask assets is a separate design.
- Memory is reserved per job from the photo's dimensions and its outputs (HDR and
  gain maps allocate further full-frame buffers; AI Denoise has its own worker),
  with hard limits, because an allocation failure aborts the process. Measure peak
  memory by megapixels and by the expensive cases first.
- Cancellation passes through decoding, rendering, AI Denoise and uploads.
  Disconnect cancellation is an acceptance test for the HTTP library
  (`tiny_http` reports no disconnect while a render runs).
- Local-path inputs open under allow-listed roots with symlinks refused, and the
  opened file's identity is what the job captures.
- SIGTERM drains within a deadline, recorded in [shutdown](shutdown.md).

### 4. S3 output

Until then, `rclone` or the AWS CLI after `render` ([Headless
rendering](headless.md#writing-to-s3-and-other-storage)). A policy-free
`rawmakase-s3` crate (`rusty-s3`, BSD-2-Clause, over the existing `ureq`) would
need:

- Single `PUT` and multipart as separate state machines. A multipart object's
  checksum is the composite of its parts', never compared with the file's
  SHA-256; `CompleteMultipartUpload` can report an error inside a 200; a failed
  upload is aborted.
- No-clobber as `If-None-Match: *` on the `PUT` or the completion, refused at
  configuration where a provider lacks it. A write whose response is lost is
  resolved with a `HEAD` before it is called failed or published.
- Credentials and endpoints are the administrator's profiles; a request names a
  profile. "Rendered" and "uploaded" are reported separately; a pending upload is
  not a finished export.
- Sources read with `If-Match` on their ETag, so one replaced mid-job fails as
  stale.

### 5. S3 in the Export dialog

As the `export-destination` contribution the [extensions
plan](extensions-plan.md#designed-not-scheduled) holds back until a real extension
needs it.

- Uploads outlive the host's 30 s request deadline and 1 s quit grace (D10), so the
  contribution needs a long-running delivery of its own: the app owns the queue
  and the staged files until each upload is confirmed or discarded, captures each
  delivery's settings when the export starts, and keeps deliveries pending through
  an extension's crash, disabling or update. Quitting with uploads pending warns.
- `export.json` gains a destination reference that older releases read safely:
  `ExportSettings::load_from` deserialises the whole file and returns nothing on
  an unknown `Destination` variant, so today's releases would drop every export
  setting. Either a new field, or the `later_*` pattern that already preserves a
  later release's format, colour space, naming and HDR choices, extended to the
  destination before any release writes one.
- Credentials live in the OS keychain, never in `export.json`, presets or the
  catalog.
