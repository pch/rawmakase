# Validation record (engines 2 and 3, September 2026)

Status: Record (September 2026, last updated 2026-10-06). This is a dated record of what was checked while engines 2 and 3 were current. Test counts and timings are from that time; today's suite is larger and the current preview path is described in [preview performance](../preview-performance.md). Engine 4's measurements against Camera Raw are in [parity gaps](parity-gaps.md) and the per-stage pages it links.

Test machine: Linux x86-64, AMD Ryzen 7 8745HS (8 cores / 16 threads), Rust 1.98.1, LibRaw 0.22.2, Little CMS 2.19. Tests run against private A7 II (`DSC05673.ARW`) and X100F (`DSCF8224.RAF`) fixtures; photos are not included in this project.

## Automated checks

- Formatting and Clippy with warnings denied.
- 58 focused unit tests: the real native scale override on 14-bit ramps with strong WB; negative matrix/headroom retention; malformed RAW rejection; curve monotonicity; Oklab round trips; hue wrapping; WB continuity; neutral picker; clipped-channel highlight neutralization; crop/rotation/straighten geometry; exact full-render/100%-region equivalence including detail; history; stale render rejection; keyboard/physical-pixel viewport behavior; pending-job coalescing; sidecar protection/identity; read-only fallback; ICC round trip; and actual 16-bit export precision.
- Explicitly enabled private-fixture integration: decode, embedded preview, cancellation, optional half-size development, full development, edited/rotated/cropped JPEG and TIFF, ICC presence, true RGB16 decoding, refusal to overwrite without permission, unchanged RAW identity.
- 50 sequential mixed ARW/RAF developments and renders: passed. Retained RSS was 48.1 MiB after 10, 20, 30, 40 and 50 images. This measures retained memory after each image is released, not peak memory while editing.

The initial full fixture/stress run completed in 97.54 seconds. Later rendering/UI revisions require the focused checks again; repeated stress runs are only necessary if ownership/caching changes.

## Visual and desktop checks

- Inspected baseline ARW and RAF renders and the Sony embedded JPEG.
- Launched the native wgpu/Wayland window on the desktop and captured its viewport, adjustments and filmstrip.
- Initial review identified magenta clipped highlights, weak default roll-off, short/reversed slider layout, and a nonsquare curve. The renderer now neutralizes colors at the sensor clipping boundary and uses a continuous rational shoulder. The controls now have full-width tracks, numeric values above, WB gradients, a compact color mixer, and a square gridded tone curve.
- The embedded JPEG is a composition/reference aid, not a required match. Camera JPEG lens corrections and creative profiles are not reproduced.

## Scope of evidence

Only these two physical camera samples have been tested. Rotated/cropped scenarios are additionally covered with synthetic geometry tests; there is not yet a broad corpus of portrait-orientation RAW files, skin tones, mixed lighting, compression modes, or camera models. Third-party calibrated-monitor/compositor appearance has not been validated; the ICC identity test proves only numerical sRGB round-trip behavior.

Interactive drafts reduce camera data before nonlinear processing and omit detail effects. Engine 3 final Fit previews use full-resolution processing and the same Lanczos resize as exports; equivalence is tested. Region/full-frame consistency is tested including spatial tones and 3-pixel sharpening radius, within 2e-6 per channel.

## Reproduce

See the commands in README.md. Ignored private-fixture tests must be explicitly enabled; a missing fixture folder is an error rather than a silently passing camera test. `rawmakase benchmark` reports development and preview median/p95 timings. Keep fixture files and generated photographs outside the repository.

## Measured release performance

The following historical timings describe engine 2 drafts, not engine 3 final previews.

| Fixture | Full development | Preview median / p95 | Benchmark peak RSS | 1600px JPEG total / peak RSS |
| --- | --- | --- | --- | --- |
| Sony A7 II ARW | 0.52 s | 52.4 / 54.3 ms | 519 MiB | 1.60 s / 581 MiB |
| Fuji X100F RAF | 2.94 s | 50.7 / 56.6 ms | 521 MiB | 3.97 s / 581 MiB |

Measured over 25 preview renders per fixture, with 8 Rayon threads and 4 LibRaw OpenMP threads. Process peak memory was polled from `/proc/PID/status`. These CPU preview timings meet the 100 ms p95 target; the aspirational 50 ms target is narrowly missed. Both final default exports were visually inspected. The initial visible magenta highlight patches in the Sony file are absent in the corrected rendering.

Final focused run: 22 unit tests passed, private ARW/RAF integration passed, and Clippy with warnings denied passed. The sidebar order is White Balance, Light, Tone Curve/Levels, Color, Color Grading, Detail, Geometry, Export. Sections start expanded and use independently drawn right-aligned disclosure triangles.

- UI refinement: compact header reset icons, chevrons, grouped toolbar with right-aligned Export, and a square curve editor without a nested frame. Curve interaction tests cover adding, moving and removing points; spline tests cover continuity, bounded interpolation and LUT accuracy. Legacy sidecar/preset migration preserves saved curve shapes.

Engine 3 validation adds published Sony DCP parsing, wrong-model rejection, malformed input handling, embedded-profile round trips, legacy engine migration, physical viewport sizing, capture sharpening edge/flat-field invariants, partial highlight reconstruction, cancel handling, and final-fit/export/tile equivalence. Both private ARW and RAF fixtures pass development and JPEG/16-bit TIFF export checks. No Lightroom reference export has yet been supplied.

Release engine 3 Sony check: 6000×4000 development 0.544 s, rendering 1.606 s, 16-bit TIFF total 2.289 s. Comparing a fresh render against that TIFF produced sRGB MAE 3.80e-6 and RMSE 4.40e-6 (16-bit quantization scale). This is an export/comparison-tool self-check, not a Lightroom comparison. Desktop verification confirmed legacy edits restore and the final Fit preview replaces the draft.

Private profile test: 52 user-provided Sony ILCE-7M2 / Fujifilm X100F DCPs passed parsing, model matching, bounded finite synthetic rendering and JSON round trips. Run with `RAWMAKASE_PROFILES=/path/to/profiles cargo test --test private_profiles -- --ignored --nocapture`. These are compatibility checks, not comparisons with complete VSCO Lightroom presets.

XMP validation: all 921 supplied preset/curve files parse; 785 apply to the Sony and 534 to the Fuji after camera/profile/setting compatibility checks. Added sparse-patch/explicit-reset, XML namespace/scalar settings, malformed values, dependency refusal, serialization and full-frame/tile spatial-effect tests. See [XMP scope](../xmp-presets.md).

Catalog validation: source-preserving full import of the supplied Lightroom catalog retained 8,112 images, 275 folders and 13 collections. The embedded source archive was byte-compared against the unchanged original. Develop table parsing read 8,111 records; one empty/unsupported record remains preserved. Synthetic tests cover rollback, active journals, future schema rejection, no-overwrite publication, folder relinking, independent virtual copies, ratings/keywords/collections, catalog edit identity protection and sidecar isolation. A headless GUI test exercises Library drawing and database autosave.

Preview/tree validation adds persistent SQLite cache hits, offline previews, source-change invalidation, corrupt entry recovery, cache eviction, unrelated-database protection, nested folder scopes/counts, root-button pointer interaction and successful relinking after reopening.

## Architecture refactor baseline — 2026-10-06

The behaviour baseline for the refactor in issue #216 is CI run 37437474805 on
`9e909ac`: lint and tests green on Linux x86_64, Linux aarch64, Windows and macOS
with LibRaw 0.22.

Performance baseline, taken on `b820a4d` (after the dependency moves of Phase 2,
which change no rendering code) with
`cargo run --release --example preview_benchmark -- DSCF7853.RAF`: Fujifilm X100F,
6032×4032, Adobe Color, Apple M1 Pro with its GPU. Fit and region rows are medians
of the benchmark's repeated renders; decoding, opening and export rows are single
timings. `b820a4d` renders differently from the behaviour baseline `9e909ac`
(Texture, Vibrance and the B&W mix changed in between), so later refactor steps
that touch decoding, previews or rendering compare against these numbers, not
older ones.

| Step | Time |
| --- | ---: |
| Decode | 366.6 ms |
| Highlight recovery | 49.7 ms |
| Open from decode cache | 38.9 ms |
| First Fit after opening | 459.1 ms |
| Fit, GPU / CPU | 17.0 / 151.8 ms |
| Fit with local adjustments, GPU / CPU | 59.6 / 205.3 ms |
| 100% region, GPU | 24.2 ms |
| Clarity region, GPU | 65.4 ms |
| Export resolution | 2721.9 ms |
| Export resolution, local adjustments | 2907.9 ms |

Fit differs from the export render resized to the same size by a mean of 0.0013
per channel (0–1 scale), on both the CPU and the GPU.

## Structural refactor — 2026-09-28

The domain split (`app.rs`, `core.rs`, `io.rs` and `catalog.rs` into the modules the [architecture guide](../architecture.md) describes) kept recipe serialization, schema/pipeline versions, rendering algorithms, asset locations and CLI commands unchanged, and added no runtime dependency or database migration. The suite went from 86 tests to one that also covers bounded history, gesture coalescing, cancellation, stale and cross-task failures, overwrite confirmation, document changes during a frame, loader-resolved recipes, preset hover invalidation, injected session persistence, malformed migration input and invalid export defaults. Relative-path persistence runs in a child process with its own working directory and data directory. The private-fixture tests were not run for the refactor; automated egui interaction tests were.
