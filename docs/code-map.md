# Code map

Use this page to find an implementation or decide where a change belongs. Read
[architecture.md](architecture.md) for ownership rules, concurrency invariants and
compatibility constraints. Paths below are relative to this document and clickable.
Module roots (`mod.rs`) define their public API; implementation helpers generally
remain private to their domain. Besides the app (`src/`) and the domain crates
described in [architecture.md](architecture.md) (model, interop, catalog, engine,
native and export, under `crates/`), the workspace has
[`crates/rawmakase-protocol`](../crates/rawmakase-protocol/src/lib.rs)
(what the app and its control clients agree on: protocol version, `control.json`
endpoint, data folder) and [`tools/rawmakase-ctl`](../tools/rawmakase-ctl/src/lib.rs)
(the control client, used by `rawmakase control`, `rawmakase mcp` and the
standalone `rawmakase-ctl`). The last two build without the app.

## Where to start a change

| Work | Start here | Related work |
| --- | --- | --- |
| Add a develop adjustment | [Recipe](../crates/rawmakase-model/src/model/recipe.rs), [pipeline](../crates/rawmakase-engine/src/develop/pipeline/mod.rs) | Inspector, XMP application, format migration and rendering regressions |
| Change preview quality or detail | [Quality rendering](../crates/rawmakase-engine/src/develop/quality/mod.rs) | Worker renderer, region/fit/export consistency tests |
| Support another XMP setting | [Parser](../crates/rawmakase-interop/src/xmp/parse.rs), [application stages](../crates/rawmakase-interop/src/xmp/apply.rs) | Recipe validation and XMP tests; library discovery stays in presets |
| Change preset discovery/import | [Preset library](../crates/rawmakase-interop/src/presets/library.rs) | Preset browser UI and shared asset paths |
| Add DCP support | [DCP reader](../crates/rawmakase-model/src/camera_profiles/dcp.rs), [profile model](../crates/rawmakase-model/src/camera_profiles/mod.rs) | Camera matching, validation, reference rendering |
| Change JPEG/TIFF output | [Export](../crates/rawmakase-export/src/export/mod.rs), [metadata](../crates/rawmakase-export/src/export/metadata.rs) | Export tests; UI captures a recipe before starting |
| Change native catalog behavior | [Catalog API](../crates/rawmakase-catalog/src/catalog/mod.rs), [schema](../crates/rawmakase-catalog/src/catalog/schema.sql) | Models, catalog tests, library UI |
| Improve Lightroom import | [Importer](../crates/rawmakase-catalog/src/catalog/lightroom/mod.rs), [Develop translation](../crates/rawmakase-interop/src/lr_develop.rs) | Preservation tests and unsupported-setting reporting |
| Change autosave or saved formats | [Save policy](../src/edit_session/save_state.rs), [background saver](../src/app/autosave.rs), [legacy sidecars](../crates/rawmakase-catalog/src/catalog/legacy_sidecar.rs), [format migration](../crates/rawmakase-model/src/model/saved_format.rs) | Catalog edits, native presets and persistence tests |
| Change navigation or async behavior | [Workflow](../src/app/workflow.rs), [events](../src/app/events.rs), [task lifecycle](../src/app/task.rs) | History, state reset and app regression tests |
| Add a command-line operation | [CLI](../src/main.rs) | Call domain APIs directly; keep the operation usable without an editor |

## Entry points and native boundary

| File | Responsibility |
| --- | --- |
| [src/main.rs](../src/main.rs) | CLI argument parsing and command dispatch; starts the desktop application when no subcommand is selected. |
| [src/lib.rs](../src/lib.rs) | The module list. The library serves the binary, examples and tests; it is not a stable public API. |
| [crates/rawmakase-native/src/photo.rs](../crates/rawmakase-native/src/photo.rs) | Opens a photo for developing: `Raw::open_file`'s facts, then embedded lens tables, a DNG's profile, baseline exposure, colour matrix and crop, and the imported lens profiles that fit. |
| [crates/rawmakase-model/src/ids.rs](../crates/rawmakase-model/src/ids.rs) | Typed catalog row ids: photo, folder, root and collection, each stored and serialized as its integer. |
| [crates/rawmakase-catalog/src/edits.rs](../crates/rawmakase-catalog/src/edits.rs) | The edit a photo develops with: its saved edit, else its Lightroom edit, else the raw defaults. Develop, Sync and Export resolve through it; [catalog/edit_records.rs](../crates/rawmakase-catalog/src/catalog/edit_records.rs) reads the stored records. |
| [crates/rawmakase-native/src/decode.rs](../crates/rawmakase-native/src/decode.rs) | A photo's full-size image: the decode cache's copy, else a decode with highlights recovered and stored; Develop, prefetch, Reference View and export differ only in their `DecodePolicy`. |
| [crates/rawmakase-native/src/decode_cache.rs](../crates/rawmakase-native/src/decode_cache.rs) | Disk cache of developed camera images and their highlight recovery, keyed by file identity, demosaic setting and build. |
| [crates/rawmakase-model/src/model/recipe.rs](../crates/rawmakase-model/src/model/recipe.rs) | A photo's develop settings as saved: defaults, validation, rendering-engine compatibility, profile selection and white balance controls, and the local edits saved beside them. [valid.rs](../crates/rawmakase-model/src/model/valid.rs) is a recipe known to be valid, which render entry points take; [panels.rs](../crates/rawmakase-model/src/model/panels.rs) the per-panel switches. |
| [crates/rawmakase-model/src/model/params.rs](../crates/rawmakase-model/src/model/params.rs), [edit.rs](../crates/rawmakase-model/src/model/edit.rs), [settings_groups.rs](../crates/rawmakase-model/src/model/settings_groups.rs) | The sliders as parameters (ids, ranges, formatting), the rules an edit follows (which operator a changed setting takes, which panel it turns on), and the setting groups Copy Settings, Sync and presets move between photos. |
| [crates/rawmakase-model/src/model/effects.rs](../crates/rawmakase-model/src/model/effects.rs) | The Effects, Detail and Calibration settings a recipe keeps (curves, grading, grain, vignettes, Defringe, noise reduction), their defaults, validation and the Effects panel's reset. Rendering them is in `develop/effects.rs`. |
| [crates/rawmakase-model/src/model/point_color.rs](../crates/rawmakase-model/src/model/point_color.rs) | Point Color swatches as a recipe stores them, in Camera Raw's units: the sampled color, shifts, Variance and ranges, which swatches Camera Raw accepts, and their `crs:PointColors` text form. Selecting and changing colors, and the dropper, are in `develop/point_color.rs`. |
| [crates/rawmakase-model/src/model/red_eye.rs](../crates/rawmakase-model/src/model/red_eye.rs) | Red Eye and Pet Eye corrections as a recipe stores them: the ellipse, Pupil Size, Darken and the catchlight, with their limits, the ellipse geometry the tool draws and edits, and the list that keeps corrections from a later release in place. Rendering and pupil detection are in `develop/red_eye/`. |
| [crates/rawmakase-model/src/model/retouch.rs](../crates/rawmakase-model/src/model/retouch.rs) | Heal and Clone operations as a recipe stores them: mode, spot or brushed shape, feather, opacity and source offset, with their limits and edits (move, resize). Rendering and source search are in `develop/retouch/`. |
| [crates/rawmakase-model/src/model/image_frame.rs](../crates/rawmakase-model/src/model/image_frame.rs) | Image space: positions normalised to the oriented photo before lens correction, Transform and crop, where masks, spots and red eye corrections are kept, and how they map to decoded pixels. |
| [crates/rawmakase-model/src/storage/mask_assets.rs](../crates/rawmakase-model/src/storage/mask_assets.rs) | The store of raster mask coverage by content ID: unsaved rasters, the catalog-backed loader, limits and eviction of saved ones. |
| [crates/rawmakase-catalog/src/catalog/mask_assets.rs](../crates/rawmakase-catalog/src/catalog/mask_assets.rs) | Storing a save's or snapshot's rasters with its rows, the loader over the `bitmaps` table, and the version 2 upgrade with its backup. |
| [crates/rawmakase-inference/](../crates/rawmakase-inference/src/lib.rs) | The selection model's pinned contract (`manifest.rs`), prompt and tensor mappings (`process.rs`), automatic Subject and Sky from the models' proposals (`auto.rs`), reading the panoptic model's answer (`panoptic.rs`), edge refinement (`refine.rs`) and the lazily loaded ONNX Runtime session of the three networks (`runtime.rs`). |
| [src/app/subject_mask/](../src/app/subject_mask/mod.rs) | Select Subject / Sky / Background: requests and their guards (`mod.rs`), the Select tiles, setup card, click-refining overlay (`ui.rs`), model download and import (`models.rs`) and the inference thread with its per-photo analysis (`worker.rs`). |
| [crates/rawmakase-model/src/model/masks.rs](../crates/rawmakase-model/src/model/masks.rs) | Masks as a recipe stores them: groups of brush, gradient and range components with their local adjustment, Amount and visibility, and their limits. Rendering their weights is in `develop/masks/`. |
| [crates/rawmakase-model/src/model/transform.rs](../crates/rawmakase-model/src/model/transform.rs) | The Transform panel's settings as a recipe stores them: the manual sliders, Upright's mode with its analysed corrections and Guided guides, and Lightroom's guides read from older edits. Rendering, analysis and solving are in `develop/geometry.rs`, `upright.rs` and `guided.rs`. |
| [crates/rawmakase-model/src/model/white_balance.rs](../crates/rawmakase-model/src/model/white_balance.rs) | Fallback illuminant and as-shot temperature estimation from a RAW's metadata, and Lightroom's named white balance presets. |
| [crates/rawmakase-model/src/model/operators.rs](../crates/rawmakase-model/src/model/operators.rs) | The operator versions a recipe records (Texture, Clarity, Sharpening, the color mixer, Gamut and the rest): stored names and oldest defaults that keep saved edits rendering as they did, and the Sharpening sliders' defaults for each. The renderer picks its operator from them. |
| [crates/rawmakase-model/src/camera_data.rs](../crates/rawmakase-model/src/camera_data.rs) | What a camera captured, as values: a RAW's metadata as RAWmakase keeps it, the demosaic and decode choices, and the decoded camera-space image. No native code, so modules above it need not link LibRaw. |
| [crates/rawmakase-native/src/raw/mod.rs](../crates/rawmakase-native/src/raw/mod.rs) | RAW files read through LibRaw: their metadata (with DNG, RAF and lens details read on top), development into camera-space images, oriented embedded thumbnails. No unsafe code. |
| [crates/rawmakase-native/src/raw/ffi.rs](../crates/rawmakase-native/src/raw/ffi.rs) | The C ABI of the native bridge: declarations, the mirrored metadata struct with its layout check, and one safe wrapper per entry point with its safety contract. |
| [crates/rawmakase-native/native/raw.cpp](../crates/rawmakase-native/native/raw.cpp) | C ABI bridge to LibRaw and Little CMS, including native image development and color management. |
| [crates/rawmakase-model/src/color/mod.rs](../crates/rawmakase-model/src/color/mod.rs) | Shared matrix and sRGB transfer primitives. |
| [src/comparison.rs](../src/comparison.rs) | Reference-image comparisons and reproducible resolved-recipe output using the normal development APIs. |
| [crates/rawmakase-native/src/demosaic.rs](../crates/rawmakase-native/src/demosaic.rs) | RAWmakase's own demosaicing of the unpacked sensor data (Bayer and X-Trans); LibRaw's is the fallback. See [demosaicing](demosaic.md). |
| [crates/rawmakase-model/src/cameras.rs](../crates/rawmakase-model/src/cameras.rs) | The camera table, [data/cameras.toml](../data/cameras.toml): per-model baseline exposure, with the same-make fallback. See [camera table](cameras.md). |
| [src/dng.rs](../src/dng.rs) | The rendering hints a DNG carries: embedded camera profile, baseline exposure, default crop and opcode lens corrections. |
| [crates/rawmakase-model/src/tiff.rs](../crates/rawmakase-model/src/tiff.rs) | Minimal bounded TIFF directory reader for RAW containers (ARW, DNG, the TIFF inside RAF), and the TIFF field types. |
| [crates/rawmakase-interop/src/jpeg.rs](../crates/rawmakase-interop/src/jpeg.rs) | Walks a JPEG's marker segments up to the image data: embedded XMP and EXIF, and where an export inserts its XMP. |
| [crates/rawmakase-interop/src/exif.rs](../crates/rawmakase-interop/src/exif.rs) | The camera's own EXIF read from a RAW, JPEG or TIFF (for exports, capture times and photo info), and the names of the EXIF, TIFF and GPS tags RAWmakase uses; maker notes and offsets into the RAW are left out. |
| [src/stats.rs](../src/stats.rs) | The opt-in weekly usage report: what it holds, how the install channel and platform are found, and sending it at most once a week ([usage-stats.md](usage-stats.md)). Built only with the default `telemetry` feature. |
| [crates/rawmakase-model/src/time.rs](../crates/rawmakase-model/src/time.rs) | Calendar dates and ISO weeks from Unix time, without a date library. |
| [src/updates.rs](../src/updates.rs) | Release checks against GitHub, whether this install may replace itself, and the signed download and install (through fastframe-update). |
| [src/platform/mod.rs](../src/platform/mod.rs), [network.rs](../src/platform/network.rs), [volume.rs](../src/platform/volume.rs), [reveal.rs](../src/platform/reveal.rs), [web.rs](../src/platform/web.rs), [text_scale.rs](../src/platform/text_scale.rs) | OS integration: Linux GVFS/FUSE path bridge, which drive a path is on, showing a file in the file manager, opening web pages, following the desktop's text size (GNOME/Omarchy `text-scaling-factor`) as the interface zoom under Wayland. |

## Development and rendering

| File | Responsibility |
| --- | --- |
| [develop/mod.rs](../crates/rawmakase-engine/src/develop/mod.rs) | Public rendering API and exports of `Recipe`, `Geometry` and `Rendered`. |
| [recipe.rs](../crates/rawmakase-engine/src/develop/recipe.rs) | What rendering makes of a recipe: the measured manual Vignetting and Color noise reduction run on the camera image. |
| [raw_defaults.rs](../crates/rawmakase-interop/src/raw_defaults.rs) | Raw defaults: the master and per-camera choices (Adobe Default, Camera Settings, RAWmakase Default or a preset), and resolving a photo's starting settings with a fallback note. See [raw defaults](xmp-presets.md#raw-defaults). |
| [geometry.rs](../crates/rawmakase-engine/src/develop/geometry.rs) | Crop, orientation, rotation, flips, straighten, output sizing and coordinate mapping. |
| [orientation.rs](../crates/rawmakase-engine/src/develop/orientation.rs) | Rotate and Flip on the photo as shown, keeping the crop and straightening on the same part of the photo. |
| [image_space.rs](../crates/rawmakase-engine/src/develop/image_space.rs) | Image space's mapping to and from the view, including the lens distortion inverse. The frame itself is `model/image_frame.rs`. |
| [retouch/mod.rs](../crates/rawmakase-engine/src/develop/retouch/mod.rs) | Heal and Clone operations (spots and brushed areas), validation and Visualize Spots. |
| [retouch/heal.rs](../crates/rawmakase-engine/src/develop/retouch/heal.rs) | Rendering one operation on linear camera pixels: feathered coverage, Clone, and Heal's multigrid membrane solve in log values. |
| [retouch/layer.rs](../crates/rawmakase-engine/src/develop/retouch/layer.rs) | The retouched image (red eye corrections, then Heal and Clone): built at once for exports, updated in dirty 256-pixel tiles for previews. |
| [red_eye/mod.rs](../crates/rawmakase-engine/src/develop/red_eye/mod.rs) | Red eye corrections (Lightroom's ellipse with semi-axes and correlation), validation, and reading saved ones leniently. |
| [red_eye/detect.rs](../crates/rawmakase-engine/src/develop/red_eye/detect.rs) | Finding the red (or, for Pet Eye, glowing) pupil inside the circle dragged over an eye. |
| [red_eye/render.rs](../crates/rawmakase-engine/src/develop/red_eye/render.rs) | Rendering one correction on linear camera pixels (Red Eye's dark neutral, Pet Eye's black and catchlight), fitted to Camera Raw 18.7. |
| [retouch/search.rs](../crates/rawmakase-engine/src/develop/retouch/search.rs) | Automatic source selection on a reduced neighbourhood: border match, texture, clipping and overlap scores. |
| [masks/mod.rs](../crates/rawmakase-engine/src/develop/masks/mod.rs) | Mask groups, components (brush, gradients, ranges), local adjustments, validation and the overlay weights. |
| [masks/eval.rs](../crates/rawmakase-engine/src/develop/masks/eval.rs) | Mask weights for a rendered region: tracing pixels to image space, combining components, caching brush rasters. |
| [masks/brush.rs](../crates/rawmakase-engine/src/develop/masks/brush.rs) | Brush strokes rasterised in image space (flow, density, erase, Auto Mask). |
| [masks/range.rs](../crates/rawmakase-engine/src/develop/masks/range.rs) | Color Range and Luminance Range weights from developed Oklab colours. |
| [masks/local.rs](../crates/rawmakase-engine/src/develop/masks/local.rs) | A mask's sliders as per-pixel deltas, and where each acts in the pipeline. |
| [pipeline/](../crates/rawmakase-engine/src/develop/pipeline/mod.rs) | The CPU pipeline, one module per seam: [colour conversions](../crates/rawmakase-engine/src/develop/pipeline/color.rs), [per-pixel stages](../crates/rawmakase-engine/src/develop/pipeline/pixel.rs), [tone preparation](../crates/rawmakase-engine/src/develop/pipeline/tone.rs), [sampling](../crates/rawmakase-engine/src/develop/pipeline/sampling.rs), [the fringe and neutral pickers](../crates/rawmakase-engine/src/develop/pipeline/pickers.rs), [image fields](../crates/rawmakase-engine/src/develop/pipeline/fields.rs) (exposure ramp, vignetting, lens warp), [GPU parameters](../crates/rawmakase-engine/src/develop/pipeline/gpu_params.rs), [render entry points](../crates/rawmakase-engine/src/develop/pipeline/render.rs) and [legacy engines](../crates/rawmakase-engine/src/develop/pipeline/legacy.rs). |
| [basic_tone.rs](../crates/rawmakase-engine/src/develop/basic_tone.rs), [basic_tone_data.rs](../crates/rawmakase-engine/src/develop/basic_tone_data.rs) | Engine 4 Contrast, Whites, Blacks and Dehaze as measured Camera Raw curves, and the measured tables. See [tone controls](tone-controls.md). |
| [parametric.rs](../crates/rawmakase-engine/src/develop/parametric.rs), [parametric.bin](../crates/rawmakase-engine/src/develop/parametric.bin) | Engine 4 parametric tone curve (Shadows, Darks, Lights, Highlights and the splits) as measured Camera Raw curves, and the measured tables. See [tone controls](tone-controls.md#parametric-curve). |
| [local_tone.rs](../crates/rawmakase-engine/src/develop/local_tone.rs), [local_tone_data.rs](../crates/rawmakase-engine/src/develop/local_tone_data.rs) | Engine 4 Shadows and Highlights: an edge-aware local operator fitted to Camera Raw, and its tables. |
| [color_mixer.rs](../crates/rawmakase-engine/src/develop/color_mixer.rs), [color_mixer.bin](../crates/rawmakase-engine/src/develop/color_mixer.bin) | Engine 4 HSL mixer, Saturation and Vibrance as measured hue/saturation/value lookups. See [color mixer](color-mixer.md). |
| [point_color.rs](../crates/rawmakase-engine/src/develop/point_color.rs) | Point Color swatches as Camera Raw stores them, their validation and text form for XMP and catalogs, and the fitted operator in HSV of linear ProPhoto RGB (`develop.wgsl`'s `point_colors` on the GPU). See [color mixer](color-mixer.md#point-color). |
| [color_grade.rs](../crates/rawmakase-engine/src/develop/color_grade.rs), [color_grade_curves.rs](../crates/rawmakase-engine/src/develop/color_grade_curves.rs), [color_grade_data.rs](../crates/rawmakase-engine/src/develop/color_grade_data.rs) | Engine 4 color grading: Camera Raw 18.7's per-channel curves (`color_grade_curves.bin`) for current recipes, and the earlier per-luminance gains that older recipes keep. |
| [upright.rs](../crates/rawmakase-engine/src/develop/upright.rs) | Upright analysis: vanishing points from straight lines, giving Level, Vertical, Full and Auto, and the Crop panel's Auto straighten angle. See [transform](transform.md). |
| [guided.rs](../crates/rawmakase-engine/src/develop/guided.rs) | Guided Upright: solving two to four guides into a correction, and what to say when they can't. See [transform](transform.md#guided-upright). |
| [quality/](../crates/rawmakase-engine/src/develop/quality/mod.rs) | Full-quality rendering, one module per seam: sizing and resizing (`mod.rs`), [detail](../crates/rawmakase-engine/src/develop/quality/detail.rs) (sharpening, mask noise), [highlight recovery](../crates/rawmakase-engine/src/develop/quality/highlights.rs), [local tone](../crates/rawmakase-engine/src/develop/quality/local.rs) (the blurs and gain behind Clarity, Texture and Dehaze), [samples](../crates/rawmakase-engine/src/develop/quality/samples.rs) (Point Color, Targeted Adjustment, retouching) and the cancellable fit/region/export [render](../crates/rawmakase-engine/src/develop/quality/render.rs). |
| [targeted.rs](../crates/rawmakase-engine/src/develop/targeted.rs) | The Targeted Adjustment Tool's targets, and how a drag is shared among the sliders for a sampled color. |
| [preview_renderer.rs](../crates/rawmakase-engine/src/develop/preview_renderer.rs) | Stateful preview backend selection, the photo's resolution pyramid, GPU diagnostics and CPU fallback. |
| [pyramid.rs](../crates/rawmakase-engine/src/develop/pyramid.rs) | Resolution pyramid of the recovered (and retouched) camera image for Fit and zoomed-out previews; patched where spot removal changed. |
| [stage_cache.rs](../crates/rawmakase-engine/src/develop/stage_cache.rs) | Preview cache of local-tone blurs, local-tone images, geometry samples, mask weights and brush rasters, keyed by the recipe fields each stage reads. |
| [gpu/mod.rs](../crates/rawmakase-engine/src/develop/gpu/mod.rs) | Optional compute device, bounded/reused buffers, command submission and readback for preview finishing. |
| [gpu/finish.wgsl](../crates/rawmakase-engine/src/develop/gpu/finish.wgsl) | Portable sharpening and separable Lanczos resize compute kernels. |
| [gpu/develop.rs](../crates/rawmakase-engine/src/develop/gpu/develop.rs) | GPU per-pixel color and tone stage: sample buffers kept per stage-cache entry, dispatch and readback. |
| [gpu/develop.wgsl](../crates/rawmakase-engine/src/develop/gpu/develop.wgsl) | WGSL port of the engine 4 per-pixel pipeline (profile tables, tone, curves, mixer, grading). |
| [pipeline/pixel_params.rs](../crates/rawmakase-engine/src/develop/pipeline/pixel_params.rs) | Which recipes the GPU stage covers, and its parameters and tables. |
| [gpu/resident.rs](../crates/rawmakase-engine/src/develop/gpu/resident.rs), [gpu/logs.wgsl](../crates/rawmakase-engine/src/develop/gpu/logs.wgsl), [gpu/local.wgsl](../crates/rawmakase-engine/src/develop/gpu/local.wgsl) | The stages before the per-pixel stage on the device: the photo kept there, local-tone blurs and gain, region sampling through geometry, lens correction and noise reduction. |
| [gpu/sampling.rs](../crates/rawmakase-engine/src/develop/gpu/sampling.rs) | The sampling pass's parameter header: named slots for the Rust side and the `S_*` offsets generated for `local.wgsl`. |
| [gpu/uniforms.rs](../crates/rawmakase-engine/src/develop/gpu/uniforms.rs) | The `present.wgsl` and `finish.wgsl` parameter blocks as Rust structs; a naga test checks their fields and offsets against the shaders. |
| [gpu/present.rs](../crates/rawmakase-engine/src/develop/gpu/present.rs), [gpu/present.wgsl](../crates/rawmakase-engine/src/develop/gpu/present.wgsl), [gpu/reduce.wgsl](../crates/rawmakase-engine/src/develop/gpu/reduce.wgsl) | Finishing developed pixels straight into the viewport texture (sharpening, effects, clipping overlay, monitor profile, histogram) and box-reducing it for the Navigator and thumbnails. |
| [gpu/weights.rs](../crates/rawmakase-engine/src/develop/gpu/weights.rs) | CPU-generated resampling coefficients matching reference boundaries and normalization. |
| [rendered.rs](../crates/rawmakase-model/src/rendered.rs) | Float RGB output buffers, integer pixel conversion, histogram generation with per-channel clipping counts, the clipping thresholds and the clipping overlay. |
| [color/curve.rs](../crates/rawmakase-model/src/color/curve.rs) | Tone-curve points, validation, interpolation and lookup tables. |
| [effects.rs](../crates/rawmakase-engine/src/develop/effects.rs) | Additional recipe controls used by XMP and spatial finishing such as grain and vignette. |
| [color.rs](../crates/rawmakase-engine/src/develop/color.rs) | Reference color behavior, including vibrance and grading math. |
| [calibration.rs](../crates/rawmakase-engine/src/develop/calibration.rs) | Camera-primary calibration and shadow tint. |
| [black_white.rs](../crates/rawmakase-engine/src/develop/black_white.rs) | Treatment (Color or Black & White, kept with black & white profiles) and the Auto black & white mix, fitted to Camera Raw's Auto. See [color mixer](color-mixer.md#black--white). |
| [auto.rs](../crates/rawmakase-engine/src/develop/auto.rs) | Auto: the Basic tone sliders, Vibrance and Saturation predicted from a reduced render of the photo by fits to Lightroom's Auto values, and white balance from gray world. |

## Camera profiles

| File | Responsibility |
| --- | --- |
| [camera_profiles/mod.rs](../crates/rawmakase-model/src/camera_profiles/mod.rs) | Profile/table models, validation, camera transforms and profile tone behavior. |
| [dcp.rs](../crates/rawmakase-model/src/camera_profiles/dcp.rs) | Bounded, endian-aware TIFF/DCP tag decoding. |
| [library.rs](../crates/rawmakase-model/src/camera_profiles/library.rs) | Explicit profile imports, RAWmakase-library loading and camera matching; lists a camera's Adobe profiles on this computer for the one-click import, and reads nothing else from there. |
| [enhanced.rs](../crates/rawmakase-model/src/camera_profiles/enhanced.rs) | Bounded XMP HSV big-table decoding, profile curves and internal adjustments; camera and creative look files (`LookFile`) and Profile Amount (`Enhanced::at_amount`). |
| [look_settings.rs](../crates/rawmakase-model/src/camera_profiles/look_settings.rs) | Exposure, Saturation, colour mixer, parametric curve, split toning and vignette settings inside looks; `Recipe::with_profile_adjustments` renders them with the user's. |
| [rgb_table.rs](../crates/rawmakase-model/src/camera_profiles/rgb_table.rs) | Adobe RGB tables (1D and 3D, in their own primaries and encoding) of creative and camera-matching looks, with tetrahedral interpolation; the colour stage applies them after the colour mixer (`develop.wgsl`'s `rgb_table` on the GPU). |
| [temperature.rs](../crates/rawmakase-model/src/camera_profiles/temperature.rs) | DNG temperature/tint and chromaticity conversion. |
| [reference.rs](../crates/rawmakase-model/src/camera_profiles/reference.rs) | A photo's baseline exposure (DNG tag or camera table) and verified neutral calibration data. |
| [dng_tone.rs](../crates/rawmakase-model/src/camera_profiles/dng_tone.rs) | Adobe DNG default tone-curve data. |
| [open.rs](../crates/rawmakase-model/src/camera_profiles/open.rs) | RAWmakase Standard and Color, our own profiles for every camera with a colour matrix. |
| [film.rs](../crates/rawmakase-model/src/camera_profiles/film.rs) | The film looks over RAWmakase Standard, embedded from `assets/looks`, which `scripts/film/` makes from Kodak's datasheets. See [film looks](lightroom-profiles.md#film-looks). |

## Lens corrections

| File | Responsibility |
| --- | --- |
| [optics/mod.rs](../crates/rawmakase-model/src/optics/mod.rs) | Radial correction model: vignetting gain, distortion and lateral CA scales, fill scale. Shared by the lens readers, DNG and the renderer. |
| [optics/lcp.rs](../crates/rawmakase-model/src/optics/lcp.rs) | Adobe lens profiles as data: parsed LCP entries, an imported profile, and the profiles a photo can choose from. |
| [lens/auto_ca.rs](../crates/rawmakase-model/src/lens/auto_ca.rs) | Remove Chromatic Aberration: lateral CA measured from the decoded image as red and blue radial scales. |
| [lens/embedded.rs](../crates/rawmakase-model/src/lens/embedded.rs) | Bounded reader for Fujifilm and Sony built-in correction tables in the RAW container. See [lens corrections](lens-corrections.md). |
| [lens/lcp.rs](../crates/rawmakase-model/src/lens/lcp.rs) | Adobe lens profiles (LCP), imported explicitly into `lens-profiles`, cached, listed for the photo's camera and matched to its lens; their data is in `optics::lcp`. |
| [lens/choice.rs](../crates/rawmakase-model/src/lens/choice.rs) | Lightroom's lens profile Setup (Default, Auto, Custom), the profile an edit names, which one renders, and the Make/Model/Profile menus. |

## XMP and presets

XMP translates Adobe settings into a validated recipe. Presets manage reusable
recipes and the installed preset collection; they do not own the renderer.

| File | Responsibility |
| --- | --- |
| [xmp/mod.rs](../crates/rawmakase-interop/src/xmp/mod.rs) | Parsed preset/settings model and XMP API. |
| [parse.rs](../crates/rawmakase-interop/src/xmp/parse.rs) | Namespace-aware XML parsing, curves, provenance and unsupported-setting notes. |
| [apply.rs](../crates/rawmakase-interop/src/xmp/apply.rs) | Named application stages for profiles, basic controls, WB, color, curves, grading, effects and crop; checks consumed settings and validates before returning a recipe. |
| [write.rs](../crates/rawmakase-interop/src/xmp/write.rs) | Writes the Camera Raw-compatible subset of a recipe as `crs:` settings, the XMP packet exports embed; the keys mirror `apply`. Not a round trip: spots and masks, Levels, quarter-turn rotation and flips, and built-in lens corrections are not written. |
| [xml/ns.rs](../crates/rawmakase-model/src/xml/ns.rs), [xml/mod.rs](../crates/rawmakase-model/src/xml/mod.rs) | XMP namespace URIs and JPEG XMP headers; XML escaping and the packet wrapper RAWmakase writes. |
| [local.rs](../crates/rawmakase-interop/src/xmp/local.rs) | Lightroom's spot removal, red eye and masks (`RetouchAreas`, legacy `RetouchInfo`, `RedEyeInfo`, mask correction lists) from XMP or a catalog, as retouch operations, red eye corrections and masks; import only. |
| [presets/mod.rs](../crates/rawmakase-interop/src/presets/mod.rs) | Public preset API. |
| [native.rs](../crates/rawmakase-interop/src/presets/native.rs) | Native JSON recipe preset load/save and shared migration handling. |
| [amount.rs](../crates/rawmakase-interop/src/presets/amount.rs) | Lightroom's preset Amount: which presets offer one, and the settings at an Amount from those before the preset and the preset's result. |
| [curves.rs](../crates/rawmakase-interop/src/presets/curves.rs) | The Point Curve menu's curves: Lightroom's built-in point curves, the curves saved in the data directory's `curves/` folder, and the name the menu shows. |
| [builtin.rs](../crates/rawmakase-interop/src/presets/builtin.rs) | Built-in presets embedded from `assets/presets`, their group order and ids. |
| [library.rs](../crates/rawmakase-interop/src/presets/library.rs) | XMP collection discovery, import without overwriting existing files, display names and favorites. |

## Persistence, catalog and export

| File | Responsibility |
| --- | --- |
| [storage/mod.rs](../crates/rawmakase-model/src/storage/mod.rs) | Shared persistence and path API. |
| [files.rs](../crates/rawmakase-model/src/storage/files.rs) | Application/asset directories, atomic JSON writes, relative parent paths and RAW enumeration. |
| [model/saved_history.rs](../crates/rawmakase-model/src/model/saved_history.rs) | A Develop History as kept between sessions, as plain values: Develop's History restores from it and the catalog stores it, neither depending on the other. |
| [model/saved_format.rs](../crates/rawmakase-model/src/model/saved_format.rs) | Saved schema/pipeline versions, envelope validation and legacy recipe migration. Recipes keep unknown fields from newer releases. |
| [bitmaps.rs](../crates/rawmakase-model/src/storage/bitmaps.rs) | Compressed raster data referenced by hash from recipes (future AI masks and patches): catalog `bitmaps` table, sidecar `bitmaps` map. |
| [identity.rs](../crates/rawmakase-model/src/storage/identity.rs) | RAW fingerprints (size, modification time and a hash of the first bytes) that tie edits and cached previews to a file. |
| [catalog/legacy_sidecar.rs](../crates/rawmakase-catalog/src/catalog/legacy_sidecar.rs) | Edits saved beside photos before editing moved into the Library: validated and imported into the catalog, with their spots and masks from the companion `*.rawmakase-local.json`, when their folder is added; also read by the CLI's `render`. The writer stays for the persistence tests. |
| [app/session.rs](../src/app/session.rs) | Last-opened path, monitor profile, raw defaults, the panels each module shows and other preferences. |
| [catalog_session/mod.rs](../src/catalog_session/mod.rs) | The open catalog and its photos, folders, collections and roots as read, with no window; writes that change those lists (ratings, the Quick Collection, refreshed fields and keywords) keep them in step. The Library shows them. |
| [catalog_session/descriptive.rs](../src/catalog_session/descriptive.rs) | Descriptive metadata edits, their undo, Read Metadata from Files, collection membership, and virtual copies with their Copy Names, written to the catalog and read back into the session's lists. |
| [catalog_session/backfill.rs](../src/catalog_session/backfill.rs) | Capture times and photo info read from files added from folders, in the background, saved in the catalog and the session's photos. The Library says which photos are online and re-sorts. |
| [catalog_session/background.rs](../src/catalog_session/background.rs) | Reading many photos' files a batch at a time on a thread, waking whoever shows them after each batch; also used by Read Metadata from Files. |
| [catalog/mod.rs](../crates/rawmakase-catalog/src/catalog/mod.rs) | Catalog lifecycle, browsing queries (photos, folders, collections, roots), metadata and relinking. |
| [catalog/db/mod.rs](../crates/rawmakase-catalog/src/catalog/db/mod.rs) | Owns the SQLite connection, behind an opaque `Db`: reads that refuse to write, consistent snapshots, writes and savepoints, the format check and schema, and Lightroom attachment for import. The only catalog module that names `rusqlite`. |
| [catalog/db/sql.rs](../crates/rawmakase-catalog/src/catalog/db/sql.rs) | `Sql` and `SqliteSql`: one `SELECT`, `WITH`, `INSERT`, `UPDATE` or `DELETE` statement each, checked at compile time by `sql!` and `sqlite_sql!`. |
| [catalog/value.rs](../crates/rawmakase-catalog/src/catalog/value.rs) | The values catalog queries bind and read (`ToValue`, `FromValue`, `FromRow`, `TextOrBlob`) and `row!` for named row structs, decoding as rusqlite always did. |
| [catalog/edits.rs](../crates/rawmakase-catalog/src/catalog/edits.rs) | A photo's saved edit: recipe and export options, the spots and masks kept beside them, and bitmaps by hash. |
| [catalog/location.rs](../crates/rawmakase-catalog/src/catalog/location.rs) | `CatalogLocation`: where a catalog is, as the app names and reopens it. A file is the only kind; file-only behaviour (its size, revealing it, `session.json`) matches on `File`. |
| [catalog/edit_rows.rs](../crates/rawmakase-catalog/src/catalog/edit_rows.rs) | The only writes to a photo's edit rows: a checked save or clear, an exact copy for virtual copies, and removal with a copy. A test fails on any other SQL that writes them. |
| [catalog/develop_history.rs](../crates/rawmakase-catalog/src/catalog/develop_history.rs) | A photo's Develop History (`model::saved_history`), stored in the same transaction as its edit; large settings are stored once per History. |
| [catalog/copies.rs](../crates/rawmakase-catalog/src/catalog/copies.rs) | Virtual copies: create, set as master, rename, remove; deleting a photo's rows, which removing a folder shares. |
| [catalog/ingest.rs](../crates/rawmakase-catalog/src/catalog/ingest.rs) | Adding a folder of photos, with the edits earlier releases saved beside them; each folder found is matched with this computer's locations. Removing folders with their photos (files untouched). |
| [catalog/locations.rs](../crates/rawmakase-catalog/src/catalog/locations.rs) | Folder locations per computer: the computer id, logical folder paths, adopting legacy mappings on open, resolving, relinking and clearing. |
| [models.rs](../crates/rawmakase-catalog/src/catalog/models.rs) | Folder, photo, collection and saved-edit records crossing the catalog API. The metadata values they carry are in [metadata.rs](../crates/rawmakase-model/src/metadata.rs). |
| [schema.postgres.sql](../crates/rawmakase-catalog/src/catalog/schema.postgres.sql) | The same tables in Postgres types, kept in step with `schema.sql` by a test; CI's "Portable catalog SQL" job prepares every portable statement against it. No release opens a Postgres catalog. |
| [schema.sql](../crates/rawmakase-catalog/src/catalog/schema.sql) | Every catalog table, idempotent: run on creation and on every open, so older catalogs gain tables added since. |
| [preview_cache.rs](../crates/rawmakase-catalog/src/catalog/preview_cache.rs) | Separate, disposable SQLite JPEG cache with identity checks, offline hits and a size budget: Library thumbnails, and Standard and 1:1 previews with the catalog photos they were asked for. |
| [lightroom/mod.rs](../crates/rawmakase-catalog/src/catalog/lightroom/mod.rs) | Read-only Lightroom snapshot import, source preservation, relational transfer and atomic destination publication. |
| [lr_develop.rs](../crates/rawmakase-interop/src/lr_develop.rs) | Parses Lightroom's serialized Lua settings as data, translates supported controls through XMP, and reports unsupported settings. Never executes Lua. |
| [lightroom/history.rs](../crates/rawmakase-catalog/src/catalog/lightroom/history.rs) | Lightroom's develop history per photo, and its recovery from the preserved .lrcat for catalogs imported before it was kept. |
| [export/mod.rs](../crates/rawmakase-export/src/export/mod.rs) | Export option validation, original-file protection, overwrite policy and atomic publication. |
| [export/encode.rs](../crates/rawmakase-export/src/export/encode.rs) | JPEG and 16-bit TIFF encoding with the ICC profile, EXIF directories and XMP. |
| [export/metadata.rs](../crates/rawmakase-export/src/export/metadata.rs), [export/exif.rs](../crates/rawmakase-export/src/export/exif.rs) | The EXIF directories an export writes (the camera's, with the export's size, orientation, resolution and software), as a JPEG's TIFF block. |
| [export/job.rs](../crates/rawmakase-export/src/export/job.rs) | One photo's export from start to finish: decode when needed, render, metadata, file. |
| [export_settings.rs](../crates/rawmakase-interop/src/export_settings.rs) | The Export dialog's choices (destination, name, format, size, metadata), saved as `export.json` for the next export, and a photo's own `ExportOptions`. Below the catalog and export, which both use them. |

## Desktop application

The desktop owns interaction and task coordination. Panels use the domain APIs
above rather than implementing SQL, file formats or pixel processing.

### State and operation policy

| File | Responsibility |
| --- | --- |
| [app/mod.rs](../src/app/mod.rs) | `Editor`, initialization, lifecycle, worker connections and explicit session-write destination. |
| [state.rs](../src/app/state.rs) | Separate document, decoded-image pair, preview, viewport and preset-browser state; centralized document reset. |
| [edit_session/history.rs](../src/edit_session/history.rs) | Bounded undo/redo, redo-branch invalidation, one transaction per editing gesture, and saving and restoring History with the edit. |
| [editing.rs](../src/app/editing.rs) | Before/after frame snapshots bound to a document generation so navigation cannot mix histories. |
| [activity.rs](../src/app/activity.rs) | Mutually exclusive foreground states: file choice, overwrite confirmation and export. |
| [task.rs](../src/app/task.rs) | Load/render generations, cancellation and completion ownership. |
| [edit_session/save_state.rs](../src/edit_session/save_state.rs) | Clean, pending, saving, failed and protected edits; debounce and retry policy. |
| [autosave.rs](../src/app/autosave.rs) | The background saver thread and its catalog connection. |
| [workflow.rs](../src/app/workflow.rs) | Opening/navigating photos, flushing edits, scheduling previews, publishing textures and launching exports. |
| [events.rs](../src/app/events.rs) | Receives worker messages, rejects stale generations and applies accepted results to editor state. |
| [dialogs.rs](../src/app/dialogs.rs) | Typed dialog intents and native file/folder choosers. |
| [treatment.rs](../src/app/treatment.rs) | The Basic panel's Treatment (and V), the B&W panel's Auto and the "Apply auto mix when first converting" preference, each change one History step. |
| [auto.rs](../src/app/auto.rs) | Runs Auto (the Basic panel's Auto button, the WB menu, Ctrl/Cmd+Shift+U) off the UI thread and applies the estimate as one History step. |
| [catalog.rs](../src/app/catalog.rs) | UI workflows for native catalogs (including the default one in the data folder), Lightroom import, folder addition, relinking and applying imported edits. |
| [folder_locations.rs](../src/app/folder_locations.rs) | Preferences › Catalog › Folder locations, and the questions changing a root or adding a folder can raise. |
| [bulk_import.rs](../src/app/bulk_import.rs) | Importing camera profiles, lens profiles and presets from chosen files or whole folders, reporting what could not be imported. |
| [upright.rs](../src/app/upright.rs) | Runs the Transform panel's Upright analysis off the UI thread. |
| [stats.rs](../src/app/stats.rs) | The one-time question about sharing usage stats and its Preferences row; the report itself is in `src/stats.rs`. |
| [updates.rs](../src/app/updates.rs) | The update notice under the toolbar and the About rows in Preferences; the checks and downloads themselves are in `src/updates.rs`. |

### Panels and interaction

| File | Responsibility |
| --- | --- |
| [workspace.rs](../src/app/workspace.rs) | Frame composition, workspace switching, shortcuts, filmstrip, status, pending work, autosave and close handling. |
| [toolbar.rs](../src/app/toolbar.rs) | Develop toolbar and menus. |
| [settings_transfer.rs](../src/app/settings_transfer.rs) | Copy Settings and its dialog, Paste Settings and Paste from Previous, through `model::settings_groups`. |
| [sync.rs](../src/app/sync.rs) | Sync Settings: the open photo's chosen groups onto the other selected photos, off the UI thread, saved in one transaction with a History step each, undone as one command. |
| [export/mod.rs](../src/app/export/mod.rs), [export/dialog.rs](../src/app/export/dialog.rs) | Export dialog, remembered export settings, background exports and their progress. |
| [stand_in.rs](../src/app/stand_in.rs) | The stored Standard preview Develop shows while a photo opens, read for the photo and its neighbour on a worker each; an offline RAW's for the Loupe, which zooms into its 1:1 preview (`library/zoom.rs`). |
| [preview_build.rs](../src/app/preview_build.rs) | Build Standard-Sized Previews: the selection's previews rendered as export renders them, on a background worker, into the preview cache; their identity, progress, Discard and Clear. |
| [preferences.rs](../src/app/preferences.rs) | Preferences window: app, catalog, profile, cache and display settings. |
| [raw_defaults.rs](../src/app/raw_defaults.rs) | Preferences' Raw Defaults block, and keeping the open unedited photo and the Library's previews in step with the defaults. |
| [onboarding.rs](../src/app/onboarding.rs) | First-run setup: photos added to the default catalog (or a Lightroom catalog imported), then optional Lightroom profiles and presets; and the Library's start while it has no photos or no catalog. |
| [theme.rs](../src/app/theme.rs), [icons.rs](../src/app/icons.rs) | Interface colors (Lightroom's neutral grays, with fastframe-theme's palettes) and the Lucide icon set. |
| [inspector.rs](../src/app/inspector.rs) | Histogram, adjustment controls and export settings. |
| [inspector/lens_profile.rs](../src/app/inspector/lens_profile.rs) | Lens Corrections' Setup, Make, Model and Profile menus over the imported lens profiles. |
| [color_grading.rs](../src/app/color_grading.rs) | The Color Grading panel: 3-Way and single-wheel views, hue/saturation wheels (Shift constrains, Cmd fine, double-click resets), Luminance, Blending and Balance, over the recipe's grading values. |
| [targeted_tool.rs](../src/app/targeted_tool.rs) | The Targeted Adjustment Tool over the photo: its drag, the sample off the UI thread, the panels' target buttons and shortcuts. |
| [point_color_panel.rs](../src/app/point_color_panel.rs) | The Color Mixer's Point Color tab: the dropper, swatches, shifts, Variance, Range, the range handles and Visualize Range. |
| [tone_drag.rs](../src/app/tone_drag.rs) | Dragging in the histogram: its five regions, the slider each drives, and one History step per drag. |
| [clipping.rs](../src/app/clipping.rs) | The histogram's clipping triangles: independent shadow and highlight warnings, hover preview, J, and the triangles' channel colours. |
| [viewport.rs](../src/app/viewport.rs) | Photo canvas, fit/100%, pan, crop (with its guide overlay and Straighten ruler) and white-balance picking; hands the pointer to the active tool. |
| [before_after.rs](../src/app/before_after.rs) | Before/After views (Before alone, side by side, split), their panes and shared zoom, the open photo's Before settings (set from History or a snapshot, copied or swapped with the edit), and the render of the side beside the edit (Before, or the reference photo). |
| [readout.rs](../src/app/readout.rs) | The RGB readout under the histogram: the rendered pixel under the pointer in Melissa RGB, and asking renders to keep their pixels only while the pointer is over the photo. |
| [reference.rs](../src/app/reference.rs) | Reference View: the reference photo (set from the filmstrip's menu or by dragging it onto the Reference side), its lock, layout and own Fit/100% zoom, developing it with its saved edit, and keeping it while moving between photos. |
| [crop_tool.rs](../src/app/crop_tool.rs) | The Crop tool's guide overlays (O, Shift+O), Straighten ruler, portrait/landscape swap (X) and Auto straighten. See [transform](transform.md#crop-and-straighten). |
| [guided_tool.rs](../src/app/guided_tool.rs) | The Guided Upright tool (Shift+T): drawing, moving, selecting and deleting guides, its loupe and grid. See [transform](transform.md#guided-upright). |
| [overlay.rs](../src/app/overlay.rs) | The active tool's drawing over the photo (pins, circles, brush cursor, handles) and pointer ownership. |
| [retouch_tool.rs](../src/app/retouch_tool.rs) | Remove tool (Q): spots, brushed areas, source dragging, keys and its drawer. |
| [red_eye_tool.rs](../src/app/red_eye_tool.rs) | Red Eye tool: Red Eye and Pet Eye, finding a pupil from a dragged circle or a click, moving, Delete, Pupil Size, Darken and the pet eye's catchlight. |
| [mask_tool.rs](../src/app/mask_tool.rs) | Masking tool (Shift+W): mask list, components, brushes and gradients on the photo, and the local adjustment sliders. |
| [panels.rs](../src/app/panels.rs) | Which side panels and filmstrip each module shows (Tab, Shift+Tab, F6–F8), kept in the session; a panel being hidden first saves or ends what it was doing. |
| [presets.rs](../src/app/presets.rs) | Preset search, groups, favorites, compatibility, application, the Amount slider and temporary hover previews. |
| [snapshots.rs](../src/app/snapshots.rs) | Develop's Snapshots panel: named states of the open photo's edit, kept per photo in the catalog (`catalog/snapshots.rs`, including Lightroom's imported snapshots). |
| [curve_menu.rs](../src/app/curve_menu.rs) | The Tone Curve panel's Point Curve menu: choosing a curve as one History step, and the Save Point Curve window. |
| [user_presets.rs](../src/app/user_presets.rs) | New Develop Preset, and Update, Rename and Delete for presets made here (`presets/user.rs`, written by `xmp/preset_write.rs`). |
| [photo_metadata.rs](../src/app/photo_metadata.rs) | Rating, color label and pick/reject controls and shortcuts. |
| [widgets.rs](../src/app/widgets.rs) | Shared buttons, adjustment sections (with Lightroom's panel switches and per-side Solo Mode), sliders (Up/Down over a hovered slider), curve editor and workspace tabs. |
| [library/mod.rs](../src/app/library/mod.rs) | The Library: composes the owners below, hands metadata and virtual-copy changes to `catalog_session` and records them for undo, and draws the sidebar, grid, filmstrip and info panel. |
| [library/filter.rs](../src/app/library/filter.rs) | The source (folder scope or collection), the filter bar's search, flag, rating and label, the offline filter and sort order; computes what is shown. |
| [library/availability.rs](../src/app/library/availability.rs) | Which originals are online, found out in the background while the Library already shows them. |
| [library/volumes.rs](../src/app/library/volumes.rs) | Whether each drive is attached and its free space, probed off the UI thread, and the volume header row. |
| [library/textures.rs](../src/app/library/textures.rs) | The bounded embedded and edited preview textures, their requests to the preview workers and the tickets that drop stale results. |
| [library/copy_name.rs](../src/app/library/copy_name.rs) | The Copy Name field: the name being typed, saved on commit and kept across selection changes and failed saves. |
| [library/tree.rs](../src/app/library/tree.rs) | Folder/collection hierarchy, rows and tree actions. |
| [library/cell.rs](../src/app/library/cell.rs) | Individual photo grid cells. |
| [library/thumbnails.rs](../src/app/library/thumbnails.rs) | Batched file availability and bounded thumbnail work using the RAW API and preview cache. |
| [library/previews.rs](../src/app/library/previews.rs) | The two preview workers (embedded thumbnails, and each photo's edited preview) with their disk cache and progress. |

### Background work

What each long-lived worker blocks on, how it stops and whether exit may wait for it: [the shutdown contract](shutdown.md).

| File | Responsibility |
| --- | --- |
| [worker/mod.rs](../src/app/worker/mod.rs) | Named event payloads, load/render jobs, task kinds, render stages and repaint notification. |
| [worker/latest.rs](../src/app/worker/latest.rs) | Single-slot mailbox: submitting a new job replaces pending work rather than growing a queue; optional lanes each keep their own latest job. A panicking job does not end the thread. |
| [worker/loader.rs](../src/app/worker/loader.rs) | RAW metadata and profile loading, embedded preview, the half-size then full decoded image and neighboring thumbnails. |
| [worker/reference.rs](../src/app/worker/reference.rs) | Develops Reference View's photo with its catalog edit on its own thread: half size first for Fit, then in full (through the decode cache) for 100%. |
| [worker/renderer.rs](../src/app/worker/renderer.rs) | Fit previews, reduced-then-full 100% regions, cancellation, monitor conversion and clipping overlays, for the edit and, in its own lane with its own caches and textures, Before beside it. After a panic it reports the job failed and rebuilds all of its state. |

## How an operation moves through the app

1. **Open a photo:** workspace/dialog/catalog actions enter `app::workflow`. It
   flushes the old document, resets document state, starts a load generation and
   submits a loader job. The loader uses `raw`, `camera_profiles` and `storage`.
   `app::events` accepts only current results. A saved catalog recipe, when present,
   replaces the loader's resolved defaults for a catalog photo.
2. **Edit and preview:** panels change the recipe. `app::editing` and `history`
   group the change; save policy marks it pending. `workflow` submits the effective
   recipe and viewport to the renderer. The worker calls `develop`; with a usable
   GPU and a recipe the GPU stage covers (engine 4 with the reference flags, see
   `pipeline/pixel_params.rs`), the color and tone stage and the finishing run on
   the device and the result is drawn straight from its texture; otherwise the
   worker renders on the CPU and prepares the display bytes. Accepted results
   become the preview. At 100%, an edit or pan whose region is not already cached
   and takes more than a moment publishes a reduced draft first, then the
   full-resolution region.
3. **Save edits:** workspace autosave and navigation/close flushing call
   `workflow`; autosave writes in the background (`autosave`), and flushing
   waits for it before saving synchronously. Photos are edited only through the
   Library, so edits always save through `catalog`. An edit the catalog cannot load
   is protected and cannot autosave, and failed writes remain pending for retry. Presets are separately saved reusable recipes.
4. **Export:** foreground activity coordinates destination/overwrite handling.
   The export job captures the recipe, renders through the development API, then
   calls `export` to encode and publish. Output embeds sRGB independently of the
   monitor profile. Navigation is held during export.
5. **Apply a preset:** `presets` discovers assets, `xmp` parses/translates Adobe
   settings, and the app handles compatibility and hover state. Hover is temporary;
   application commits a recipe through the editing/history flow.
6. **Import Lightroom:** the app or CLI calls `catalog::lightroom`. It reads a
   source snapshot, preserves source data, transfers relationships, and publishes
   a new native catalog. Develop translation is best effort and reports unsupported
   settings; it does not promise Lightroom rendering parity.

## Where data lives

`storage::data_dir()` selects `RAWMAKASE_DATA_DIR` when set. Otherwise it uses
`~/Library/Application Support/RAWmakase` on macOS and `$XDG_DATA_HOME/rawmakase`
(default `~/.local/share/rawmakase`) on Linux.

| Data | Location and lifetime |
| --- | --- |
| Original RAW and Lightroom catalog | User-selected source files; treated as read-only. |
| Legacy sidecar edits | Adjacent `photo.ARW.rawmakase.json` / `photo.RAF.rawmakase.json`, plus `photo.ARW.rawmakase-local.json` for spots and masks, or identity-keyed JSON under the data directory's `sidecars/`, saved by releases before 0.1.8. Imported into the catalog when their folder is added and left on disk unchanged. |
| Native catalog | User-selected `.rawmakase` SQLite file; authoritative catalog metadata, edits and preserved import data. Spots, red eye corrections and masks are in the `local_edits` table, Develop History in `develop_history`, raster data in `bitmaps`. |
| Native preset / exported photo | User-selected JSON / JPEG / TIFF destination. |
| Session preferences | `session.json` in the data directory; last path and monitor ICC path. UI tests inject a temporary destination or disable writes. |
| Preset favorites | `preset-favorites.json` in the data directory. |
| Saved point curves | `curves/` in the data directory: one XMP file per curve, named by the curve. |
| Installed assets | `xmp-presets/` and `camera-profiles/` under asset roots; shared discovery also checks the legacy XDG/Linux data location. Imported XMP files go to `xmp-presets/Imported/` under the current data directory. |
| Library previews | `previews.sqlite3` in the data directory; disposable cache, not a source of edits. |
| Decoded photos | `decoded/` in the cache directory (`~/Library/Caches/RAWmakase` on macOS, `$XDG_CACHE_HOME/rawmakase`, default `~/.cache/rawmakase`, on Linux; `RAWMAKASE_CACHE_DIR` overrides it): developed camera images of opened and prefetched photos, capped at 4 GB, least recently used removed first. Disposable. |
| Build output | `target/`; generated binaries, documentation and local macOS bundle. |

## Tests and reference tools

Most unit tests live alongside their implementation, either inline or in a
sibling `tests.rs`. Keep regressions with the domain that owns the behavior.

| Location | Coverage / use |
| --- | --- |
| [app/tests.rs](../src/app/tests.rs), [library/tests.rs](../src/app/library/tests.rs) | Editor interactions, state transitions, worker results, navigation, library trees and metadata. Small state owners also contain inline tests. |
| [develop/pipeline/tests.rs](../crates/rawmakase-engine/src/develop/pipeline/tests.rs) | Rendering, geometry and reference regressions; numeric helpers also have inline tests. |
| [camera_profiles/tests.rs](../crates/rawmakase-model/src/camera_profiles/tests.rs) | Profile parsing and validation. |
| [xmp/tests.rs](../crates/rawmakase-interop/src/xmp/tests.rs), [presets/tests.rs](../crates/rawmakase-interop/src/presets/tests.rs) | Settings parsing/application and native preset compatibility. |
| [catalog/legacy_sidecar/tests.rs](../crates/rawmakase-catalog/src/catalog/legacy_sidecar/tests.rs) | Migration, source identity, conflict protection and fallback persistence. |
| [catalog/tests.rs](../crates/rawmakase-catalog/src/catalog/tests.rs) | Catalog, import and relinking behavior; preview-cache tests live in its module. |
| [catalog/locations_tests.rs](../crates/rawmakase-catalog/src/catalog/locations_tests.rs) | One catalog on several computers: adoption, per-computer relinking and clearing, import matching and legacy paths. |
| [export/tests.rs](../crates/rawmakase-export/src/export/tests.rs) | JPEG/TIFF precision, ICC and EXIF output. |
| [develop/gpu/tests.rs](../crates/rawmakase-engine/src/develop/gpu/tests.rs) | Explicit hardware tests for CPU/GPU agreement, borders, buffer reuse, crop/region handling, effects and fallback. |
| [tests/color/](../tests/color/main.rs), [tests/corpus/README.md](../tests/corpus/README.md) | The color corpus: synthetic chart DNGs rendered and compared with committed snapshots and Camera Raw renders on every `cargo test`; private photo and Adobe-profile tiers behind `RAWMAKASE_CORPUS`. |
| [examples/preview_benchmark.rs](../examples/preview_benchmark.rs) | Read-only release benchmark of first Fit, slider Fit, 100% region and export renders on a supplied RAW; checks Fit against the resized export. |
| [tests/persistence.rs](../tests/persistence.rs) | Public API regressions for relative paths, malformed legacy recipes and invalid export defaults; isolates process-wide path settings in a child process. |
| [tests/raw_fixtures.rs](../tests/raw_fixtures.rs) | Ignored private RAW development/export and repeated-navigation memory tests (`RAWMAKASE_FIXTURES`). |
| [tests/private_profiles.rs](../tests/private_profiles.rs) | Ignored installed/private DCP coverage (`RAWMAKASE_PROFILES`). |
| [tests/xmp_presets.rs](../tests/xmp_presets.rs) | Ignored installed XMP collection audit. |
| [catalog/private_tests.rs](../crates/rawmakase-catalog/src/catalog/private_tests.rs) | Ignored private Lightroom catalog validation (`RAWMAKASE_LRCAT`). |
| [tests/data/README.md](../tests/data/README.md), [curve samples](../tests/data/lightroom-point-curves.json) | Small checked-in Lightroom point-curve reference data and its provenance. |
| [scripts/make-curve-fixtures.py](../scripts/make-curve-fixtures.py) | Generates synthetic TIFF ramps for manual Lightroom curve comparisons. |
| [scripts/compare-preview.py](../scripts/compare-preview.py) | Compares resized sRGB previews without exposure/color fitting; distinct from the Rust comparison command. |
| [scripts/film/](../scripts/film/README.md) | Makes the film looks in `assets/looks` and their presets from Kodak's datasheets, downloaded on every run. |

Use the [README's development section](../README.md#development) for the standard
suite. Private fixture checks require external data and are explicitly ignored by
default. A passing unit suite does not replace the photographic/manual checks in
the validation docs.

## Build, packaging and documentation

| Location | Responsibility |
| --- | --- |
| [Cargo.toml](../Cargo.toml), [Cargo.lock](../Cargo.lock) | Package/toolchain requirements, dependencies and locked resolution. |
| [build.rs](../build.rs) | Locates LibRaw/Little CMS, compiles the C++ bridge and configures platform OpenMP linking. |
| [Makefile](../Makefile) | Build/check and local install/uninstall shortcuts. |
| [packaging/macos/app.sh](../packaging/macos/app.sh), [Info.plist](../packaging/macos/Info.plist) | Local Finder-launchable macOS bundle and its metadata. |
| [rawmakase.desktop](../packaging/applications/rawmakase.desktop), [rawmakase.svg](../packaging/icons/rawmakase.svg) | Linux launcher and application icon. |
| [LICENSE](../LICENSE), [Adobe notice](../licenses/Adobe-DNG-SDK.txt) | Project licensing and third-party DNG attribution. |
| [README](../README.md) | Build/run instructions, controls, CLI examples and supported scope. |
| [Architecture](architecture.md) | Module ownership, state/thread invariants and compatibility rules. |
| [Catalogs](catalogs.md) | Catalog use, import preservation, relinking, schema and preview cache. |
| [XMP presets](xmp-presets.md) | Supported settings and compatibility limitations. |
| [Preview performance](preview-performance.md) | GPU/CPU responsibilities, hardware support limits, measured timings and reproduction. |
| [Rendering quality](rendering-quality.md) | Engine 3 behavior, profile support and reference comparisons. |
| [Color pipeline](color-pipeline.md) | Versioned rendering contract, including the historical pipeline-1 description. |
| [Dependencies](dependencies.md) | Native dependency and linking notes. |
| [Validation](validation.md) | Recorded checks, evidence limits, reproduction and measured performance. |
| [macOS / Lightroom validation](macos-lightroom-validation.md) | Dated photographic comparisons and platform validation results. |
| [Parity gaps](parity-gaps.md) | Known differences and work still needed for Lightroom parity. |
| [Tone controls](tone-controls.md), [color mixer](color-mixer.md), [lens corrections](lens-corrections.md), [transform](transform.md), [demosaic](demosaic.md) | How each engine 4 stage was measured against Camera Raw and what it does. |
| [Retouching](retouching.md), [masking](masking.md) | The Remove, Red Eye and Masking tools: use, rendering, Camera Raw measurements and what is not implemented. |
| [Lightroom profiles](lightroom-profiles.md) | RAWmakase's own profiles, importing Adobe and third-party profiles, and supported profile features. |

When adding or moving a module, update its entry here. Put API contracts in Rust
doc comments, ownership decisions in the architecture guide, and measured results
in the validation documents so each has a clear home.

### Camera parity diagnostics

`scripts/cameras/check-calibration.py` audits converter metadata against the camera
table; `data/camera-calibration-facts.json` holds public-sample and anonymous local numerical evidence.
`scripts/cameras/parity.py` captures and verifies Camera Raw renders and compares
frozen references with RAWmakase. See [camera parity](camera-parity.md).
