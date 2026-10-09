# Preview performance and GPU support

Fit and zoomed-out views render from a resolution pyramid of the photo (see
[below](#resolution-pyramid)); 100% regions and export use the full-resolution
image. The desktop runs the per-pixel color and tone stage on the GPU (see
[GPU develop stage](#gpu-develop-stage)) and uses GPU compute for sharpening and
Lanczos3 resizing of full resolution renders, and finishes previews (sharpening,
grain, vignettes, clipping overlay, monitor profile) straight into the texture the
viewport draws (see [presenting on the UI's GPU](#presenting-on-the-uis-gpu)). RAW
decoding, geometry sampling, local tones and export stay on the CPU. There is no
separate draft: every slider change renders the real pipeline at Fit size, and at
100% a half-resolution preview of the region comes first (see
[slider responsiveness](#slider-responsiveness)). The status line shows `GPU` for
presented frames and `GPU finish` when only finishing used compute.

The shaders are portable WGSL through wgpu, with no CUDA or Metal-specific code.
Metal is used on macOS; Linux NVIDIA/AMD devices can use Vulkan with a working
compatible driver and sufficient compute/storage limits. Unsupported adapters,
oversized images and GPU failures fall back to CPU. That is intended platform
support, not evidence of testing on every driver. **Hardware tested here: Apple
M1 Pro only. Linux/NVIDIA/AMD runtime validation remains outstanding.**

See [architecture](architecture.md#preview-compute-backend) for resource limits,
fallback behavior, cancellation and the division between CPU and GPU work.

## Resolution pyramid

Opening a photo recovers highlights once at full resolution. Fit and zoomed-out
renders then use a pyramid of that image: each level halves the previous one with a
2×2 box average in linear camera space, and levels are built on first use and kept
while the photo is open. A render takes the smallest level with at least one pixel
per output pixel and develops each output pixel exactly once, averaging four
bilinear taps over the pixel's footprint. Before, Fit developed a camera image
reduced to twice the output size (four times the pixels) and resized it afterwards.

Radius-based effects are scaled to the output: sharpening uses the full-resolution
radius times the output scale (a three-tap kernel of the same variance below half a
pixel), Clarity and Texture radii follow the level size, and grain keeps its
full-resolution pattern with the amplitude a resized export would have. Fit is
therefore an approximation of the export resized, not identical to it; a unit test
bounds the mean difference on a textured image with sharpening, local and spatial
effects, and the benchmark checks each photo. 100% regions and exports are
unchanged.

`examples/preview_benchmark`, release build, Apple M1 Pro, 1600-pixel Fit, median of
three exposure changes after a warm-up. "Local" adds Shadows +40, Highlights −30
and Clarity +20. Error is the mean absolute channel difference from the export
render resized to the same size (0–1 scale).

| Photo | Render | Before | After |
| --- | --- | ---: | ---: |
| X100F DSCF7853, 6032×4032, Adobe Color DCP | First Fit after opening | 1407 ms | 508 ms |
| | Fit | 1098 ms | 283 ms |
| | Fit, local | 1429 ms | 498 ms |
| | Fit error vs export | 0.0023 | 0.0016 |
| Sony A7CR, 9564×6376, no DCP | First Fit after opening | 640 ms | 379 ms |
| | Fit | 565 ms | 249 ms |
| | Fit, local | 731 ms | 278 ms |
| | Fit error vs export | 0.0047 | 0.0059 |

The remaining Fit time is the per-pixel color pipeline (DCP tables, profile tone
curve, `powf`), about 1.7 megapixels per render. 100% regions with local
adjustments (about 1.1 s on the X100F and 1.5 s on the A7CR for a 1600×1000
region) still recompute Clarity over the full image on every change.

## Stage caching

The desktop renderer keeps the results of the stages before the per-pixel color
pipeline (`crates/rawmakase-engine/src/develop/stage_cache.rs`), each keyed by the recipe fields it reads:

- the measurement copy: the full photo reduced to 512 pixels, from which every
  render takes the scene tone stage's measures and maps (docs/scene-tone-stage.md);
- the scene tone stage's measures of that copy, keyed by the copy, the profile
  matrix, white balance, Temperature and Tint, the profile and its amount, the
  camera's exposure and Camera Calibration (the measures are taken at Exposure 0);
- the textured image, when Texture is on;
- samples: each output pixel's camera value after geometry, lens correction and
  noise reduction, and its source position.

Exposure, curve, HSL, grading and Shadows/Highlights edits therefore rerun
only the per-pixel stage; Clarity and Texture edits reuse the blurs. Two entries are
kept per stage (Fit and a 100% view) within 512 MB per stage; larger results are
computed and not kept. Export uses no cache. A unit test checks that cached renders
equal uncached ones after each kind of edit.

Measured as above, but as the best of three interleaved runs of the before and after
builds, because other work was loading the machine (load average about 30 on 10
cores; unchanged export timings varied by up to 2×). "Clarity" changes Clarity on
every render instead of exposure.

| Photo | Render | Pyramid only | With stage cache |
| --- | --- | ---: | ---: |
| X100F | Fit | 358 ms | 264 ms |
| | Fit, local | 435 ms | 251 ms |
| | Fit, Clarity edits | 688 ms | 405 ms |
| | 100% region, local | 1429 ms | 366 ms |
| | 100% region, Clarity edits | 1296 ms | 546 ms |
| A7CR | Fit | 141 ms | 135 ms |
| | Fit, local | 187 ms | 120 ms |
| | 100% region, local | 1140 ms | 1386 ms |

The A7CR's full-resolution local-tone stages (61 megapixels) exceed the cache
budget, so its 100% view with Clarity is not faster yet; local tones on pyramid
levels are the next step.

## Slider responsiveness

The render worker used to answer each change with a 1024-pixel draft from the
earlier (engine 2) pipeline, wait 150 ms, then render the full-quality Fit, which
took one to two seconds. With the pyramid and stage cache, the worker renders the
current engine at Fit size straight away, with no draft and no wait, so every
frame shown while dragging is the real rendering.

At 100%, each change first renders the visible region from the pyramid at half
resolution or less (at most 0.6 megapixels), which the viewport stretches over the
region, then the full-resolution region. The worker's mailbox keeps only the latest
job and a newer job cancels the running one, so while a slider moves the view
follows the reduced previews, and the sharp region appears when it stops. Pending
region or quality work never delays a newer slider job beyond the next cancellation
check (per row in blurs and per pixel in the color stage; a GPU command already
submitted finishes first).

Renderer times per change, same conditions as the stage cache table, but with the
machine even busier (load average 40–58), so these are upper bounds. Before this
change the first update was the 50 ms legacy draft, and the real rendering came
150 ms plus 0.8–1.8 s later.

| Photo | View | Real rendering per change |
| --- | --- | ---: |
| X100F | Fit | 170 ms |
| | Fit, local | 228 ms |
| | 100% preview (half resolution) | 62 ms |
| | 100% preview, local | 67 ms |
| | 100% preview, Clarity edits | 104 ms |
| | 100% full region, local | 221 ms |
| | 100% full region, Clarity edits | 325 ms |
| A7CR | Fit | 184 ms |
| | Fit, local | 223 ms |
| | 100% preview (half resolution) | 34 ms |
| | 100% preview, local | 41 ms |
| | 100% preview, Clarity edits | 116 ms |
| | 100% full region, local | 1469 ms |

## GPU develop stage

`crates/rawmakase-engine/src/develop/gpu/develop.wgsl` ports the per-pixel stage (`process_pixel`) of the
current engine: white balance, camera matrix, DCP HueSatMap (with its two-illuminant
blend), calibration, exposure and the DNG exposure ramp, LookTable, enhanced-look
table and curve, the profile tone curve, the Shadows/Highlights map,
measured Basic curves, levels, parametric and point curves, color mixer, color
grading, Oklab Defringe/Monochrome and per-channel gamut clipping. It runs on the samples in
the stage cache, which stay on the device while only the recipe changes; parameters
and tables (`pixel_params.rs`) are uploaded per render and the result is read back
for sharpening and spatial effects on the CPU (readback callers such as the benchmark
and tests), or finished on the device (the desktop).

The port covers every recipe resolved with a camera profile, which is every photo
once it has been resolved (`pixel_params::supported`). Machines without a usable
adapter render on the CPU; a GPU failure disables the GPU for the session. Export
always uses the CPU, which remains the reference.

Profile tables are read with explicit trilinear interpolation from a storage buffer
rather than a hardware-filtered 3D texture, whose reduced-precision filter weights
would not match the CPU. A hardware test compares the two stages on 4000 samples over
seven recipes that exercise every table and branch: the 99.9th-percentile channel
difference is at most 0.0003 and the mean at most 0.00001 (0–1 scale). With
Monochrome, a few near-neutral pixels whose Oklab hue is unstable can land in
another band (largest difference 0.01).

The result is no longer read back on the desktop; see
[presenting on the UI's GPU](#presenting-on-the-uis-gpu).

Best of two interleaved runs against the previous commit, load average 24–35:

| Photo | Render, per exposure change | CPU color stage | GPU color stage |
| --- | --- | ---: | ---: |
| X100F | Fit | 166 ms | 14 ms |
| | Fit, local | 437 ms | 39 ms |
| | Fit, Clarity edits | 461 ms | 149 ms |
| | 100% preview | 51 ms | 10 ms |
| | 100% region | 188 ms | 24 ms |
| | 100% region, local | 381 ms | 53 ms |
| | First Fit after opening | 392 ms | 198 ms |
| A7CR | Fit | 95 ms | 11 ms |
| | Fit, local | 149 ms | 26 ms |
| | 100% preview | 25 ms | 6 ms |
| | 100% region | 106 ms | 20 ms |
| | 100% region, local | 1327 ms | 1359 ms |

Fit differs from the resized export exactly as much as the CPU Fit (0.0016 X100F,
0.0059 A7CR). Clarity edits and the A7CR's 100% view with local adjustments were then dominated
by the full-resolution local-tone blurs on the CPU (see the next section).

## Presenting on the UI's GPU

The render worker used to read every GPU result back with a blocking wait, sharpen
and apply grain and vignettes on the CPU, convert to 8 bits, apply the monitor
profile with a new lcms transform, draw the clipping overlay and compute the
histogram; the UI thread then built an egui image, reduced it for the Navigator and
uploaded it again. The GPU stage now runs on eframe's own wgpu device, and
`gpu/present.wgsl` finishes the developed pixels on the device into a texture the
viewport draws: sharpening, vignettes and grain, the clipping overlay, the monitor
profile (a lattice sampled from lcms once per profile) and 8-bit encoding, with the
histogram counted in the same pass and the Navigator copy reduced on the GPU. Only
the histogram (3 KB) and, for catalog photos, the library thumbnail come back.
When the GPU does not render a recipe, the worker prepares the display bytes,
histogram and reduced copies, so the UI thread only uploads.

A hardware test compares presented frames with the CPU renderer's 8-bit output for
Fit, full size and a 100% region, with sharpening, grain, vignettes, Clarity and the
clipping overlay: at most one level differs, in at most 9 of 48,513 values.

Also in this change: the local-tone gain and the samples are keyed by what they are
computed from rather than by the address of the blurs, so a 61-megapixel photo,
whose blurs exceed the cache budget, no longer recomputes Clarity on every exposure
change at 100%; and the reduced image the Shadows/Highlights map is built
from is kept in the stage cache instead of being reduced from the full image on
every render.

Time until a frame is ready to draw, per exposure (or Clarity) change: before, the
render plus the worker's and UI thread's conversions (`ready to draw` in
`preview_benchmark`); after, the presented frame including the histogram readback
(`presented`). Best of two interleaved runs of the previous commit and this change,
release build, Apple M1 Pro, machine loaded by other work (load average about 15–20):

| Photo | Render | Before | After |
| --- | --- | ---: | ---: |
| X100F | Fit | 31.9 ms | 8.8 ms |
| | Fit, local | 59.1 ms | 39.1 ms |
| | Fit, Clarity edits | 158.5 ms | 118.9 ms |
| | 100% preview | 10.0 ms | 3.9 ms |
| | 100% region | 33.8 ms | 7.3 ms |
| | 100% region, local | 66.7 ms | 26.3 ms |
| | 100% region, Clarity edits | 173.1 ms | 134.6 ms |
| A7CR | Fit | 24.3 ms | 4.1 ms |
| | Fit, local | 40.7 ms | 14.4 ms |
| | Fit, Clarity edits | 86.7 ms | 56.4 ms |
| | 100% preview | 6.7 ms | 1.3 ms |
| | 100% region | 24.6 ms | 4.1 ms |
| | 100% region, local | 1026.8 ms | 12.8 ms |
| | 100% region, Clarity edits | 725.9 ms | 743.7 ms |

Fit differs from the resized export as before (0.0016 X100F, 0.0059 A7CR). Clarity
edits at 100% on the A7CR then still recomputed the full-resolution blurs and gain
on the CPU (about 110 ms for the log luminance, 150–190 ms per box blur and 150 ms for
the gain under this load); see the next section.

## Photo kept on the GPU

(The log luminance, box blurs and Clarity/Texture gain this section describes were
removed with the scene tone stage, whose Clarity reads the 512-pixel map; the photo
is still kept on the device and sampled there.)

The stages before the per-pixel stage now run on the device too (`gpu/logs.wgsl`,
`gpu/local.wgsl`, `gpu/resident.rs`). The photo, or the pyramid level a Fit renders
from, is uploaded once and kept; the log luminance and the box blurs (running sums
per row and column, as the CPU computes them) are computed and kept there, keyed as
the stage cache keys them. The Clarity/Texture gain is computed only for the photo
pixels a region samples (found by mapping the region's edges through geometry and
lens correction), so a Clarity edit touches the whole photo only when the
Shadows/Highlights map needs its reduced input: that reduction computes the gain as
it goes, in two passes (row sums over each box's columns, then over its rows) and is
read back (512 pixels on the long edge). Each region is sampled through geometry,
Transform, lens correction and noise reduction straight into the develop stage's
input. Only the
Shadows/Highlights map itself (guided filter on 512 pixels) stays on the CPU. Photos
up to 2 GB of pixels where the adapter allows (a 61-megapixel photo is 735 MB) take
this path; a failure here, for example for lack of device memory, turns only this
path off and the CPU stages run as before.

The hardware test above now also covers lens correction (distortion, lateral
chromatic aberration, vignetting), straighten and crop, Transform, noise reduction,
Clarity and Texture: at most one 8-bit level differs from the CPU render.

Per Clarity edit, until the frame is ready to draw, median of three after a warm-up,
release build, Apple M1 Pro, load average about 20:

| Photo | Render | Step 1 | Photo on the GPU |
| --- | --- | ---: | ---: |
| X100F | Fit, Clarity edits | 118.9 ms | 42.4 ms |
| | 100% region, Clarity edits | 134.6 ms | 49.4 ms |
| A7CR | Fit, Clarity edits | 56.4 ms | 22.4 ms |
| | 100% region, Clarity edits | 743.7 ms | 72.1 ms |

With the gain computed only where it is read and the two-pass reduction (load average
about 35): A7CR 100% region, Clarity edits with Shadows and Highlights, 59 ms; with
Clarity alone, 7 ms (650 ms on the CPU path); Fit, Clarity edits, 19 ms.

Exposure edits are unchanged (the gain is kept either way). A new photo's first
Clarity render uploads the photo and computes the blurs once.

Further per-render work removed afterwards:

- The Shadows/Highlights map's tone pass (the tone stage over the 512-pixel reduced
  photo, about 15 ms on the CPU per exposure change) runs on the GPU; the guided
  filter then runs on the CPU from the luminance, with its blurs in parallel (same
  arithmetic, so exports are unchanged).
- The device keeps the state of two images, the pyramid level Fit renders and the
  full photo 100% renders, so switching views with Clarity on does not upload the
  photo or blur again: 15–18 ms per switch with an exposure edit.
- At 100%, the reduced preview before the full region is skipped while full regions
  are presented within 40 ms, so a slider drag shows only sharp frames.

Presented per exposure change with Shadows +40, Highlights −30 and Clarity +20 (the
"local" rows), X100F: Fit 28 → 16 ms, 100% region 29 → 16 ms, 100% preview 24 → 10 ms.

## Local-tone gain

Clarity and Texture compare each pixel's log
luminance with full-resolution box blurs of it (16 and 64 pixels on a 6000-pixel long
edge). The stage result is kept as a per-pixel gain that the sampling stage applies
(4 bytes per pixel), instead of a modified copy of the camera image (12 bytes per
pixel), so a 61-megapixel A7CR's local-tone stage fits the stage cache. Applying the
gain while sampling gives the same values as the modified copy.

Blurring on reduced levels was tried and removed: it changed exports with these
controls (mean 0.0002, at most 0.016), and the develop math is to stay unchanged.
Exports without Clarity, Texture or older Shadows/Highlights are bit-identical to
the renderer before this work; with them, computing the blurs before exposure (so
exposure edits reuse them) changes results only by float rounding: mean 0.000001, at
most 0.00013 on the X100F.

Measured with reduced-level blurs, before they were removed (best of two interleaved
runs, load average 38–47); the gain storage accounts for the A7CR's 100% gain:

| Photo | Render | Before | Gain storage |
| --- | --- | ---: | ---: |
| A7CR | 100% region, local, exposure edits | 1119 ms | 66 ms |
| | 100% region, Clarity edits | 1010 ms | 476 ms |
| X100F | 100% region, local, exposure edits | 50 ms | 49 ms |

Clarity edits recompute the gain for the whole image and resample the view;
computing the gain only under the visible region would be the next step for 100%
Clarity drags.

## Decode cache and prefetch

Opening a photo stores the developed camera image, exactly as decoded, in a disk
cache (`crates/rawmakase-native/src/decode_cache.rs`), together with the pixels highlight recovery changed
(0.7% of pixels on DSCF7853, 46 pixels on the A7CR file), so the recovered image is
restored without recomputing it. The loader recovers highlights itself before
announcing the full image, which the first render would otherwise do. A later open
of the same photo reads the cache and skips both the half-size and the full decode.
After a photo opens, the loader develops the next and previous photos of the folder
into the cache on a two-thread pool, leaving the other cores to rendering; opening
another photo cancels the prefetch.

Entries are keyed by the RAW file's identity (size, modification time and a hash of
its first 64 KB), the demosaic setting and the running executable's size and time,
so a rebuilt or updated app never uses another build's pixels. The cache lives in
`decoded/` under the per-user cache directory (`~/Library/Caches/RAWmakase` on macOS,
`$XDG_CACHE_HOME/rawmakase` or `~/.cache/rawmakase` on Linux, `RAWMAKASE_CACHE_DIR`
to override), is capped at 4 GB and drops the least recently used entries first.
An entry is the image's size in 32-bit floats: 278 MB for the X100F, 697 MB for the
A7CR. RAW files, sidecars and catalogs are not touched.

Measured with `preview_benchmark` (store, then open from a warm cache) on the loaded
machine:

| Photo | Decode | Highlight recovery | Store | Open from cache |
| --- | ---: | ---: | ---: | ---: |
| X100F | 434–534 ms | 51–105 ms | 83–246 ms | 58–232 ms |
| A7CR | 1037–2836 ms | 135–200 ms | 286–529 ms | 587–648 ms |

Through the real loader, in a folder of three X100F photos: the first open showed
the half-size image at 166 ms and the full image at 884 ms; reopening it showed the
full image at 140 ms, and the two prefetched neighbours opened in 190 and 300 ms.

## Stored Standard previews — 2026-10-08

A photo with a Standard preview built (Library › Previews, see
[catalogs](catalogs.md)) shows it as soon as it starts opening, until the first live
render replaces it; the neighbour's is read ahead, so moving on shows it in the
first frame. Without one, Develop shows the Library's 640 px thumbnail at once and
then the camera's embedded JPEG.

`examples/stand_in_benchmark`, release build, Apple M1 Pro with other work loading
the machine (load average 17–33 on 10 cores), the default edit, a 1600-pixel Fit,
best of two runs. The handoff difference is the mean absolute difference between
the stored preview scaled to the Fit and the live Fit, in 8-bit sRGB on a 0–1 scale.

| | X100F DSCF7853, 6032×4032 | Sony A7CR, 9504×6336 |
| --- | ---: | ---: |
| Embedded JPEG read (before its resize to 2560 px) | 17 ms, 1920×1280 | 193 ms, 9504×6336 |
| Stored Standard preview read, decoded and made a texture image | 14 ms | 19 ms |
| First live Fit from the decode cache | 336–372 ms | 695–883 ms |
| First live Fit after a decode | 575–609 ms | 1995–2150 ms |
| Building the Standard preview (2048 px, half-size decode) | 1.08 s | 2.75 s |
| Building it from the full decode instead | 3.9 s | 8.5 s |
| Handoff difference, half-size build | 0.0038 | 0.0173 |
| Handoff difference, full-decode build | 0.0036 | 0.0164 |

The stored preview is therefore on screen within about 20 ms on both cameras,
against 0.3–2 s for the first render. On the A7CR it also arrives well before the
embedded JPEG, which is full size and is resized before it shows. The handoff
difference comes from the stored preview being an export render (sharpened at its
own size, JPEG at quality 85, then scaled by the viewport) where the live Fit is the
pyramid approximation (see [Resolution pyramid](#resolution-pyramid)); building
from the full decode barely changes it, so builds keep the half-size decode when it
is large enough. On the A7CR the difference is about four 8-bit levels on average,
a visible but small shift when the live render lands.

A build running in the background (two threads at background priority) did not
measurably slow the first live Fit: from the decode cache, 330–343 ms against
347–356 ms with a build running on the X100F, and 695–852 ms against 727–912 ms
on the A7CR, within the run-to-run spread on this loaded machine. Builds are
therefore not paused while Develop loads or renders.

## 1:1 previews — 2026-10-08

Issue #213 left open what 1:1 previews should be used for: zooming an offline photo
to 100% in the Loupe, or standing in at 100% while the full-resolution image or
region is not ready yet. Measured with `examples/preview_benchmark` on the X100F
file above (same machine and load), once the full-resolution image is in memory a
100% region is presented in 24 ms (73 ms with Shadows, Highlights and Clarity), and
the reduced region shown while a slider moves in 5–55 ms. What a 100% view waits
for is therefore the full-size image itself: 45–80 ms from the decode cache, or
0.36 s (X100F) and 1.4 s (A7CR) for a decode, the same wait as the first Fit, which
the Standard preview already covers. A 1:1 stand-in would save at most that decode,
only when zooming in within the first second of opening a photo, for tens of
megabytes per photo; the benchmark's A7CR run stopped at an unrelated Fit-against-
export check, so its region times are not recorded here.

So 1:1 previews ship for the offline Loupe only, stored as rows of the preview cache
like Standard previews (one store, one staleness check, one budget, nothing left on
disk when a row goes), with Lightroom's automatic discard; see [catalogs](catalogs.md).

## GPU finishing measurements — 2026-09-26

Release build on Apple M1 Pro, private Fujifilm X100F RAW (6032×4032), installed
Adobe Standard DCP, 1600-pixel final fit. Each number is the median of three
exposure changes after one warm-up. Decoding, first highlight recovery, GPU device
initialization, the worker's 150 ms refinement debounce and UI presentation are
outside these timings. GPU timings include uploads, processing and readback.
These are renderer measurements, not end-to-end slider latency or a broad camera
benchmark. No source RAW, edit or export file was changed by the benchmark.

| Render | Before | After | Change |
| --- | ---: | ---: | ---: |
| Interactive draft | 134.4 ms | 49.2 ms | 63% less time |
| Full-quality fit, neutral local controls | 2713.7 ms | 1800.8 ms | 34% less time |
| Full-quality fit, local tone adjustments | 3518.3 ms | 2455.6 ms | 30% less time |

The draft comparison intentionally includes the reduction from 1600 to 1024
pixels; it is a responsiveness/temporary-detail tradeoff. Final-quality sizes and
processing remain the same. Local adjustments in this benchmark are shadows
+0.4, highlights −0.3 and clarity +0.2.

For the current code alone, CPU versus GPU quality-fit medians were 2473.0 versus
1800.8 ms with neutral local controls and 2906.2 versus 2455.6 ms with local tones.
The rest of the improvement comes from CPU work: parallel column processing in
local-tone blurs and skipping hue calculations for inactive color controls.
Exposure gain is prepared once per recipe rather than calculated per pixel.

Maximum CPU/GPU channel differences on the final benchmark images were
0.00000036 (neutral local controls) and 0.00000030 (local tones), in normalized
float RGB. GPU tests enforce a 0.00002 tolerance and cover different sharpening
radii, image edges, one-pixel dimensions, resizing, repeated buffer reuse, rotated
crops, regions, spatial effects, cancellation and fallback. The neutral-color CPU
fast path is also checked against its general processing path.

Full-quality rendering still takes seconds on this photograph. The GPU work does
not accelerate LibRaw decoding or the complete color pipeline. High-resolution
color processing, local effects and memory transfers remain targets for future
profiling. Cancellation now interrupts recovery, local-tone and sharpening CPU
work; an already-submitted GPU command must finish, after which a stale result is
discarded. The interactive draft remains an approximation of the final render.

## Reproduce

Standard tests do not require a GPU. Run hardware checks explicitly:

```sh
cargo test --locked -p rawmakase-engine develop::gpu::tests -- --ignored --nocapture
cargo run --release --locked --example preview_benchmark -- /path/to/photo.RAF 3
```

The benchmark prints the actual adapter and profile, and fails if a Fit render
differs from the resized export by a mean of 0.01 or more. It times the decode cache
in a temporary directory that it removes. It reads the supplied RAW and
installed profiles and writes no photographic files. Use the same RAW, profile,
release build and machine conditions for comparisons. Keep private photos outside
the repository.
