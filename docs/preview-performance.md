# Preview performance and GPU support

Fit and zoomed-out views render from a resolution pyramid of the photo (see
[below](#resolution-pyramid)); 100% regions and export use the full-resolution
image. The desktop runs the per-pixel color and tone stage on the GPU (see
[GPU develop stage](#gpu-develop-stage)) and uses GPU compute for sharpening and
Lanczos3 resizing of full resolution renders, and finishes previews (sharpening,
grain, vignettes, clipping overlay, monitor profile) straight into the texture the
viewport draws (see [presenting on the UI's GPU](#presenting-on-the-uis-gpu)). RAW
decoding, geometry sampling, local tones and export stay on the CPU. There is no
separate draft pipeline: every slider change renders the real pipeline at Fit size,
and where that takes more than 40 ms a reduced render of the photo, or at 100% of the
region, comes first (see [slider responsiveness](#slider-responsiveness) and
[slow GPUs](#slow-gpus--2026-10-09)). The status line shows `GPU` for
presented frames and `GPU finish` when only finishing used compute.

The shaders are portable WGSL through wgpu, with no CUDA or Metal-specific code.
Metal is used on macOS and DirectX 12 on Windows; Linux NVIDIA/AMD devices can
use Vulkan with a working compatible driver and sufficient compute/storage
limits. Unsupported adapters, oversized images and GPU failures fall back to CPU. That is intended platform
support, not evidence of testing on every driver. **Hardware tested here: Apple
M1 Pro only. Linux/NVIDIA/AMD runtime validation remains outstanding.**

See [architecture](architecture.md#previews) for scheduling, fallback and
cancellation, and [`develop::gpu`](../crates/rawmakase-engine/src/develop/gpu/mod.rs)'s
module docs for backends, resource limits and the division between CPU and GPU work.

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
pixel), Texture radii follow the level size, Clarity's are fractions of the
Shadows/Highlights map whatever the level, and grain keeps its full-resolution
pattern with the amplitude a resized export would have. Fit is
therefore an approximation of the export resized, not identical to it; a unit test
bounds the mean difference on a textured image with sharpening, local and spatial
effects, and the benchmark checks each photo. The pyramid leaves 100% regions and
exports as they were.

"Full-resolution" is the photo's. A reduced decode (the half-size draft, which keeps
the photo's metadata; `CameraImage::photo_scale`) sharpens and grains as its photo
resized, in Fit, whole renders and 100% regions alike. Before 2026-11-07 it was
sharpened as if it were the photo, at about twice the radius, with grain about 2.4
times too strong. Exports never render one: they decode the photo in full.

Its noise reduction works at the photo's scales too. Each pyramid level of a reduced
decode shows the photo's level `−log₂(photo_scale)` further on (about one for a
half-size draft, 1.2 for 2560 pixels of a 6000-pixel photo): it takes the photo's
settings there, interpolated between two levels, and a level that straddles the
start of the photo's unfiltered residual is filtered in proportion to its part
before it. The thresholds still follow the noise measured in the decode itself.
Before 2026-11-07 a half-size draft was filtered as if its pixels were the photo's:
color noise reduction gave the photo's second level the thresholds of its finest,
and both filtered one level coarser than the photo's last. A unit test compares the
share of noise kept at each scale with the photo's denoised and resized ([rendering
quality](rendering-quality.md#noise-reduction-on-a-reduced-decode)).

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
  render takes the scene tone stage's measures and maps (docs/scene-tone-stage.md),
  and with red eye corrections or spot removal, its copy without them (for the
  darkest level the default black follows);
- the scene tone stage's measures of that copy, keyed by the copies, the profile
  matrix, white balance, Temperature and Tint, the profile and its amount, the
  camera's exposure and Camera Calibration (the measures are taken at Exposure 0);
  with them, what renders measure on the same scene values with one more setting:
  the Contrast pivot by Dehaze, the copy's camera color, and the
  Shadows/Highlights map's base (local Laplacian base and lazily computed
  negative-Clarity detail) by Exposure, so the Basic sliders only rebuild the
  map's curves;
- the textured image, when Texture is on;
- samples: each output pixel's camera value after geometry and lens correction,
  and its source position.

Color and luminance noise reduction are cached together on the full-resolution
source, before retouching and the preview pyramid. Changing any active noise
setting rebuilds that source and invalidates the pyramid and dependent stages;
panning or tone edits reuse it. Both CPU and GPU sample the same denoised source.

Exposure, curve, HSL, grading and Shadows/Highlights edits therefore rerun
only the per-pixel stage; Clarity and Texture edits reuse the blurs. Two entries are
kept per stage (Fit and a 100% view) within 512 MB per stage; larger results are
computed and not kept. Export uses no cache. A unit test checks that cached renders
equal uncached ones after each kind of edit.

An entry is keyed by the image it was made from, so once that image is gone (the
photo was left, or an edit replaced it) nothing can ask for the entry again. Such
entries are dropped after the next render, with what the device keeps of the image.
Before 2026-11-18 they stayed until newer entries of their own stage pushed them
out: a 61-megapixel photo's Texture detail (526 MB) outlived it for as long as the
photos opened next had no Texture, and each pane's last frame held the photo it
showed (1.4 GB of that photo, its decode and recovered copy).

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
job, so while a slider moves the view follows the reduced previews, and the sharp
region appears when it stops. A newer edit of the same view lets the running render
finish rather than cancelling it (see [slow GPUs](#slow-gpus--2026-10-09)); a new
photo or view cancels it at the next cancellation check (per row in blurs and per
pixel in the color stage; a GPU command already submitted finishes first).

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

## Slow GPUs — 2026-10-09

On an Intel UHD Graphics 620 laptop (i5-8365U, Linux, Vulkan) at display scale 2, a
Fit render took 235–314 ms by the status line, and dragging a slider showed no frame
until it stopped. Each changed value cancelled the running render, and with renders
slower than the slider's changes none ever finished. Two changes address it:

- An edit of the same document and view no longer cancels the running render; it
  finishes and shows with the settings it was asked for, and the latest change is
  rendered next. A new photo, view or size still cancels it, and a result older than
  the one shown is dropped.
- While the whole photo is edited, a full render slower than 40 ms is preceded by a
  reduced one, its long edge halved until it has at most a megapixel (701 pixels
  for that laptop's 934 × 1402 Fit), as 100% regions already were.

`examples/preview_benchmark` on that laptop, release build, a Nikon Z f DNG
(6064×4040), Adobe Standard, 1600-pixel Fit, median of three; "local" adds Shadows,
Highlights and Clarity as above. Presented times, until the frame can be drawn:

| Render | Neutral | Local |
| --- | ---: | ---: |
| Fit | 63 ms | 449 ms |
| 100% preview (reduced region) | 14 ms | 256 ms |

Without local adjustments a reduced render is several times faster. With them, the
CPU work per render that does not depend on the output size (the Shadows/Highlights
map, the contrast measure, the histogram readback) dominates, and a reduced render
saves less; that work is the next target on slow machines.

`examples/slider_benchmark` times what the desktop does while each Basic slider is
dragged at Fit: a frame presented for each new value, from a new photo's recipe with
Contrast −76 and Highlights −3, on the 934 × 1402 Fit and its half-size draft. On the
same laptop and photo, per change, the CPU and GPU work that did not depend on the
slider dominated:

| Per render | Before | After |
| --- | ---: | ---: |
| Contrast pivot, measured on the CPU | 88 ms | kept with the measures, by Dehaze |
| Shadows/Highlights map base on the CPU | 65 ms | kept by Exposure; the copy's camera color kept |
| Develop inputs created and uploaded (mostly the map's grids) | 25 ms | grids kept on the device |
| Histogram with global atomics | about 20 ms | counted per workgroup, 64 × 64 tiles |

The develop pass is also compiled for the features a render uses (masks, color
stages, the map, Dehaze: about 7,500 instructions became 2,600 without masks and
color), and keeps each sample set's camera color (white balance, the profile's
matrix and HueSatMap, calibration), which no Basic slider nor Exposure changes, from
the second render with the same settings. A hardware test checks that a warm
renderer presents the frames of a fresh one across these edits.

Per change, median of 8, two runs:

| Slider | Fit before | Fit after | Draft before | Draft after |
| --- | ---: | ---: | ---: | ---: |
| Exposure | 241–244 ms | 27–28 ms | 202–208 ms | 30–42 ms |
| Contrast | 244–247 ms | 35 ms | 205–207 ms | 19 ms |
| Highlights | 241–242 ms | 35 ms | 203–207 ms | 19 ms |
| Shadows | 331–333 ms | 47–59 ms | 410–415 ms | 34–46 ms |
| Whites | 242–243 ms | 35–39 ms | 204–207 ms | 19 ms |
| Blacks | 228–231 ms | 33–34 ms | 191–193 ms | 18 ms |

Positive Shadows still measures its black response per value (the median gain over
the copy), and Exposure builds the map's guided filter per value, on the CPU.

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
adapter render on the CPU; a GPU failure disables the GPU for the session. A region
beyond the device's buffer limits or the develop memory budget is not a failure: it
renders on the CPU and the next region uses the GPU again. Export
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
profile with a new color transform, draw the clipping overlay and compute the
histogram; the UI thread then built an egui image, reduced it for the Navigator and
uploaded it again. The GPU stage now runs on eframe's own wgpu device, and
`gpu/present.wgsl` finishes the developed pixels on the device into a texture the
viewport draws: sharpening, vignettes and grain, the clipping overlay, the monitor
profile (a lattice sampled once per profile) and 8-bit encoding, with the
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
cache (`crates/rawmakase-decode/src/decode_cache.rs`), together with the pixels highlight recovery changed
(0.7% of pixels on DSCF7853, 46 pixels on the A7CR file), so the recovered image is
restored without recomputing it. The loader recovers highlights itself before
announcing the full image, which the first render would otherwise do. A later open
of the same photo reads the cache and skips both the half-size and the full decode.
After a photo opens, the loader develops its neighbour in the direction of travel
into the cache on a two-thread pool, leaving the other cores to rendering; opening
another photo cancels the prefetch. The prefetch starts once the photo's first
full-size render is done (the app holds it until then, `worker::NeighbourPrefetch`):
started as the full-size image arrived, as it was before 2026-11-18, the
neighbour's decode ran beside that render's noise reduction, the two largest
bursts of memory of an open at once
([measurements](records/preview-performance-measurements.md#memory-while-a-photo-opens--2026-11-18)). A JPEG, PNG or TIFF neighbour is never cached:
the prefetch opens it into memory instead, and the next load takes it if it is that
file, unchanged in size and modification time, and drops it otherwise.

Entries are keyed by the RAW file's identity (size, modification time and a hash of
its first 64 KB), the demosaic setting and the running executable's size and time,
so a rebuilt or updated app never uses another build's pixels. The cache lives in
`decoded/` under the per-user cache directory (`~/Library/Caches/RAWmakase` on macOS,
`%LOCALAPPDATA%\RAWmakase\Cache` on Windows, `$XDG_CACHE_HOME/rawmakase` or
`~/.cache/rawmakase` on Linux, `RAWMAKASE_CACHE_DIR` to override), defaults to a 4 GiB cap and drops the least recently used entries first.
Preferences > Catalog > Decoded images sets the cap from 1 GiB to 1 PiB (1,048,576 GiB), saved
between launches. Changes reach Develop, neighbour prefetch and Reference View
without restarting; a reduced cap is enforced on the next cache write.
Preferences > Performance > Choose Folder… moves the cache to `RAWmakase Cache/` in
a folder of the user's choice, such as another drive, and Use Default moves it back.
Moving removes the old location's entries, and an entry being written during the move
removes itself, so the old drive is freed. The folder must be writable when chosen;
its subfolder is created only while the folder exists, so an unplugged drive is never
recreated on the system disk, and photos are just not cached until it returns.
Develop, prefetch, Reference View, export and preview builds share the app's cache,
so they all follow the choice without restarting.
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

## One buffer for a decode and its recovered image

A decoded photo used to hold its pixels twice: as decoded, and with highlights
recovered (`CameraImage::recovered`), 12 bytes a pixel each. That is 277 MB twice for
a 24-megapixel photo and 698 MB twice for a 61-megapixel one, for the photo open in
Develop, the neighbour being prefetched, each export and each Library preview
rendered from a full-size decode. Recovery changes few pixels: 0% to 25% over nine
cached decodes, 3% in the median, and none of a rendered file (JPEG, TIFF…), whose
recovered image is the file as it is.

Since 2026-11-18 the two images hold one buffer between them
(`camera_data::Pixels`, #1173). The buffer is the recovered image's. The decode
keeps its own value only for each pixel recovery changed (12 bytes), with one bit a
pixel that says which those are and a count per 64 pixels that finds a value without
a search (0.25 bytes a pixel together). A rendered file and its recovered image, and
a RAW recovery changed nowhere, are the same buffer outright.

The buffer is the recovered image's because that is the one read whole and fast:
the pyramid's first level, the input of noise reduction and retouching, the image
the device keeps and every sample of a render come from it, as one slice. The
decode's own values are read less often, but they are read, long after recovery:
Auto white balance, the Tone Curve's histogram, mask and HDR previews (each reduces
the decode, then recovers the reduced copy), Upright's analysis, Auto black & white,
AI Denoise's assembly, Auto CA's measurement, the decode cache's store and the
camera-input stage. So the decode can't be dropped, and three simpler designs
were set aside:

- **Share the buffer only when recovery changed nothing.** It covers rendered files
  and one RAW in nine of those measured.
- **Keep the decode whole and the recovered image as its changes.** Every render
  would then read each pixel through the list of changes.
- **Keep the recovered image and let its pixels stand for the decode's.** Each
  reader above would silently measure other pixels; nothing in a type says which
  image a reader means.

The decode still reads as any image does, so no reader can get the wrong pixels.
One pixel (`pixels[i]`) or a range (`Pixels::range`, a row at a time) is read where
it is kept and costs no copy; every reader listed above does one or the other. Auto
CA's measurement takes the whole decode as a range, which is a copy for the
measurement alone. Reading a decode as one slice (`&pixels[..]`, `iter()`) is still
right: it copies the whole image out the first time and keeps the copy, which spends
the memory again. New code that reads a whole decode should use `range` or an index.

The pair is made where a decode is owned, before it is shared:

- `develop::quality::with_recovered` recovers in the decode's own buffer. Recovery is
  each pixel's own, so the pixels are those a second buffer would hold, bit for bit
  (tested). The loader's full-size decode, the prefetch, Reference View, exports
  and the Library previews built from a full-size decode (`export::job::decode_full`)
  go through it, so decoding no longer allocates the second buffer at all.
- The decode cache loads an entry as a pair. Its format is unchanged: the pixels as
  decoded, then the changed pixels' recovered values.
- A rendered file opens with pixels its recovered image shares (`Pixels::share`).

A decode that is only borrowed when it is first rendered is recovered into a second
buffer as before: the half-size image shown while the full size decodes, a Library
preview's half-size decode, the full-size decode of a Library preview with AI
Denoise, the full-size decode batch Sync measures on, and the `render` command's.
They are small or short-lived; each could use `with_recovered` too.

`CameraImage::recovered` is the same `Arc` for a decode as before, so what is keyed
by it (the pyramid's source, the retouch cache, the stage cache and the device's
image) is unchanged.

Measured on an M1 Pro (32 GB, macOS 27.2), release builds: one photo open at Fit in
Develop with default settings, the app launched for that photo alone, live heap from
`heap` once work had gone quiet. Each photo went from three full-size buffers (the
decode, its recovered image, its noise-reduced image) to two and its changed pixels.

| Photo | Pixels recovery changed | Decoded, before | after | From the cache, before | after |
| --- | ---: | ---: | ---: | ---: | ---: |
| 61 MP ARW | 28.3% | 2,367 MB | 1,916 MB | 2,437 MB | 1,991 MB |
| 44 MP CR3 | 0.15% | 1,723 MB | 1,222 MB | 1,723 MB | 1,222 MB |
| 33 MP ARW | 4.3% | 1,316 MB | 965 MB | 1,316 MB | 964 MB |
| 29 MP DNG | 4.2% | 1,150 MB | 835 MB | 1,179 MB | 863 MB |
| 24 MP CR2 | 10.5% | 1,232 MB | 992 MB | 1,244 MB | 1,004 MB |
| 24 MP CR3 | 0.07% | 930 MB | 660 MB | 952 MB | 684 MB |
| 24 MP JPEG | none | 664 MB | 386 MB | 661 MB | 386 MB |

The 61-megapixel photo's 17 million changed pixels take 197 MB; the block that held
them was 233 MB in these runs.

Exports of the seven photos (16-bit ProPhoto TIFF, full size, with Highlights −100,
Shadows +60 and Exposure +0.7) hold the same samples from both builds, decoded and
from the decode cache.

Times, on the same machine while other work loaded it (load average about 150, so
steps this change does not touch moved by a third either way between builds):
opening from the decode cache took 104 ms before and 63 ms after for the
61-megapixel photo, 45 and 41 ms for the 24-megapixel CR2 (the best of 15 interleaved
runs of `preview_benchmark`); the first Fit after opening showed no difference
beyond that noise. Recovering in the decode's own buffer against into a second one,
the best of 12 runs in one process: 416 against 354 ms for the 61-megapixel photo,
88 against 102 ms for the 24-megapixel CR2 and 80 against 52 ms for the 33-megapixel
one, with medians within 25 ms of each other for all three. Where recovery changes
much of a photo it now keeps what the changed pixels were, which the second buffer
did not need.

## Library edited previews

The Library's 640-pixel edited previews render with `develop::render_fitted`,
Develop's Fit path from the resolution pyramid, on the CPU; the GPU added too little
to share Develop's device for. Previews cached by an earlier renderer are keyed by
`EditSource::preview_tag` and made again. Full pace renders one photo per 4 GB of
memory side by side (`platform::memory`): two on an 8 GB computer, four from 16 GB.
The measurements behind these choices are in the
[record](records/preview-performance-measurements.md#library-edited-previews--2026-10-29).

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

These were measured before half-size builds sharpened, grained and reduced noise at
their photo's scale (2026-11-07), and have not been measured again since.

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
