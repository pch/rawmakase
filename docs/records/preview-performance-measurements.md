# Preview performance measurements

Status: Record (2026-09-26 to 2026-11-18). Measurements moved from
[preview performance](../preview-performance.md), which describes the current
preview path. Numbers are from the code and machines of their dates; the
"interactive draft" of 2026-09-26 has since been replaced by rendering the real
pipeline at Fit size.

## Memory while a photo opens — 2026-11-18

What is alive at the peak of opening a photo, and of rendering Library previews
side by side (#1173). Release build, Apple M1 Pro with 32 GB, macOS 27.2, an
isolated data folder, 60 CC0 RAWs of 24 to 61 megapixels. The footprint is
`phys_footprint` sampled every 50 ms; what is alive was read from allocation
stacks (`MallocStackLogging=lite`, `malloc_history -allBySize`) with the app
stopped as its footprint crossed a level. The machine was under heavy load from
other work throughout, so the times below compare runs made minutes apart and
are not the app's speed.

Opening a 61-megapixel ARW with nothing cached peaked at 7.2 and 7.5 GB (two
runs). Alive on the way there:

| What | Size |
| --- | ---: |
| The photo's decode and its highlight-recovered copy (they stay) | 1.4 GB |
| Color noise reduction of the first full-size render: the chroma, the mask, the level's blur, the coarser levels' result, the output, the luminance | up to 2.6 GB |
| The neighbour's prefetch, started as the full-size image arrived: its file, mosaic, green plane and output, then its recovered copy | 0.7 to 1.6 GB |
| The decode cache's list of pixels recovery changed, while the photo is stored | 0.26 GB |
| Freed blocks the allocator still held (macOS keeps its last 64 large ones) | up to 1.1 GB at the start of the render; 1.6 GB left afterwards |

The half-size draft and what was made from it were gone by then. So the peak
was three bursts at once on top of the photo: the render's noise reduction,
the neighbour's decode and what the stage before had freed.

Launching into the folder with every photo edited and no preview cached peaked
at 5.0 GB, with 4.1 GB of it in use: four half-size renders of 61-megapixel
photos side by side, each about 1 GB (its half-size image and recovered copy,
0.35 GB, and the same noise reduction planes, 0.65 GB), and 0.6 GB of embedded
previews being decoded for the grid at the same moment. Freed blocks were
0.1 to 0.2 GB there.

Changed for it:

- Color noise reduction filters in place: 26 bytes a pixel at its peak instead
  of 44 (1.5 GB instead of 2.6 GB of a 61-megapixel photo), every value as it
  was.
- A RAW's file is freed before its mosaic is demosaiced.
- The neighbour's prefetch starts once the open photo's first full-size render
  is done, not as its full-size image arrives.
- The Develop loader gives freed blocks back before the first full-size render
  and before the prefetch (`platform::memory::release_freed`), on macOS.

## Library edited previews — 2026-10-29

The Library's 640-pixel edited previews render with `develop::render_fitted`,
Develop's Fit path from the resolution pyramid, on the CPU. They used to render
the half-size decode whole, as an export does, and reduce the result. Measured
with a throwaway harness on an Apple M5 Pro (18 cores), 24 Nikon Z 6 NEF and DNG
files, four threads, medians of one run on a machine that was not idle:

| Recipe | Half-size decode | Whole render | Fit, CPU | Fit, GPU |
| --- | ---: | ---: | ---: | ---: |
| Defaults | 90 ms (NEF), 220 ms (DNG) | 824 ms | 69 ms | 50 ms |
| Exposure, Shadows, Highlights, Clarity | | 943 ms | 129 ms | 111 ms |
| The same and luminance noise reduction | | 976 ms | 170 ms | 147 ms |

The mean difference from the whole render was about 0.01 (0–1 scale) on the NEFs
and 0.002–0.006 on the DNGs; only the mean was measured. The GPU added too little
to share Develop's device for, so previews stay on the CPU. Previews cached
before are keyed by `EditSource::preview_tag` and made again.

With larger RAWs (eight CC0 raw.pixls.us samples, 46-102 MP, same machine, the
real workers, two runs each), eight visible edited previews took 2.0-2.3 s against
24.9-25.3 s rendered one at a time, but peaked at 4.7-5.25 GB against 3.2-3.7 GB;
the 102 MP GFX100S alone peaked at 2.9 GB either way. Full pace therefore renders
one photo per 4 GB of memory side by side (`platform::memory`): two on an 8 GB
computer, four from 16 GB.

## Decoder speed — 2026-10-18

RAWmakase's own decoders against LibRaw, measured at the switch to them on 45
dataset files (three per format), the median of RAWmakase's time over LibRaw's:
0.79 to open and decode, 0.80 to open and develop at full and half size. No
format is slower beyond noise; the largest gains are compressed RAF (0.25),
IIQ and 3FR (0.45) and PEF (0.53).

## 1:1 previews — 2026-10-08

Issue #213 left open what 1:1 previews should be used for: zooming an offline photo
to 100% in the Loupe, or standing in at 100% while the full-resolution image or
region is not ready yet. Measured with `examples/preview_benchmark` on the X100F
DSCF7853 of the [stored Standard previews](../preview-performance.md#stored-standard-previews--2026-10-08) measurement (same machine and load), once the full-resolution image is in memory a
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
disk when a row goes), with Lightroom's automatic discard; see [catalogs](../catalogs.md).

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
not accelerate RAW decoding or the complete color pipeline. High-resolution
color processing, local effects and memory transfers remain targets for future
profiling. Cancellation now interrupts recovery, local-tone and sharpening CPU
work; an already-submitted GPU command must finish, after which a stale result is
discarded. The interactive draft remains an approximation of the final render.

