# Rendering pipeline versions

Every recipe, whichever release saved it, renders with the one current engine: the stages, camera profiles and preview behavior in [rendering-quality.md](rendering-quality.md), the [scene tone stage](scene-tone-stage.md), and the measured controls in [tone controls](tone-controls.md), [color mixer](color-mixer.md), [lens corrections](lens-corrections.md) and [transform](transform.md). Earlier engines and their per-control operators are no longer rendered, and edits saved with them render differently now. The settings that chose them (`model::saved_format::OBSOLETE_SETTINGS`) are dropped when a recipe is read; slider values and other unknown settings are kept. Recipes are saved as schema/pipeline 11, which releases before the one engine refuse.

Without an imported or bundled camera profile, the photo renders like a DNG that carries only a ColorMatrix: the decoder's XYZ-to-camera matrix (Adobe's) becomes a forward matrix adapted to D50, the ACR 3 default tone curve is applied with hue-preserving RGB tone, and white balance temperature/tint are solved through that matrix. The camera exposure baseline applies whether or not a profile is present. On DSCF7853 (X100F) against Lightroom's Adobe Standard export this changed the out-of-the-box result (measured before the scene tone stage) from engine 3's −1.1 EV / MAE 0.140 (LibRaw's sRGB matrix and a generic scene shoulder) to −0.01 EV / MAE 0.030, the same as with the imported Adobe Standard DCP.

The native development boundary, white balance, out-of-gamut and display sections below still apply. The numbered [Rust pipeline](#rust-pipeline) and the saved-version notes at the end are the historical pipeline-1 contract, kept as a record; that pipeline is no longer rendered.

## Rendered files (JPEG, PNG, TIFF, WebP, AVIF, HEIF, JPEG XL)

Develop edits a JPEG, PNG, TIFF, WebP, AVIF, HEIF or JPEG XL from its own pixels, as Lightroom does, through the same engine as a RAW.

- **Input space.** `raster::open` decodes the file upright and converts it through its embedded ICC profile (sRGB when it has none) to linear ProPhoto RGB (D50), f32, in a `CameraImage` marked `Source::Rendered`, with identity white balance and matrix. RGB and grey profiles are read; CMYK and other colour spaces are refused: `raster::probe` reads only the header and profile, so the Library greys out Open in Develop and says "RAWmakase can't edit CMYK images" before anything is decoded. There is no RAW decoding, half-size stage or decode cache: the file opens at full size at once.
- **WebP, AVIF and JPEG XL.** Decoded in pure Rust (`image`'s WebP decoder, `jxl-oxide`, `rav1d` built without assembly) and chosen by content, not extension. An AVIF's colour comes from its `colr` box: an ICC profile, or one built from its CICP code points; without a `colr` box it is taken as sRGB, so an HDR AVIF signalled only in its AV1 header is not refused. AVIF matrix and range follow the file; identity-matrix files with limited range and YCgCo or ICtCp matrices may show slightly wrong colours. One in BT.2020 constant luminance can't be opened. Alpha is dropped. HDR (CICP transfer PQ or HLG, or a JPEG XL with an HDR type) and animated files (a WebP animation, an AVIF sequence, a JPEG XL animation) show in the Library as a plain preview of the first frame (an HDR one tone mapped, its reference white of 203 nits shown as white and brighter light rolled off into it), while Develop refuses them ("RAWmakase can't edit HDR images", "… animated images"); an animated AVIF without a primary still image can't be shown. A tiled (`grid`) AVIF is refused everywhere ("… tiled AVIF images"). So is one whose image needs a property RAWmakase does not read, such as a clean-aperture crop (`clap`), rather than shown uncropped. A JPEG XL is opened as jxl-oxide renders it, upright by its codestream, with no second EXIF rotation. Export to these formats is not available yet.
- **HEIF (HEIC).** A HEIF of HEVC (`.heic`, `.heif`, `.hif`), as phones and some cameras write, is chosen by content and decoded by `rust_h265` (pure Rust), the same on every platform (the `software-hevc` build feature; [dependencies](dependencies.md)). A phone's grid of tiles is laid into one picture before its conversion to RGB, and a clean aperture (`clap`) crops it, as libheif does; a file whose pictures' data isn't all there is refused before any of it is decoded. Colour comes from the `colr` boxes of the image, or of its first tile when the image has none of that kind (an iPhone writes its ICC profile on the grid and its `nclx` on the tiles); without an `nclx` the matrix and range are taken as BT.601 and full range, what phones and libheif write, and the HEVC header's own are not read. 4:2:0 chroma is interpolated bilinearly, as an AVIF's (above 8 bits, by RAWmakase itself with `yuv`'s weights, for both formats: `yuv` 0.8.19's own 10- and 12-bit 4:2:0 conversion tints every other pixel on ARM); libheif repeats each sample, so the two differ along sharp colour edges and otherwise agree within one level. 8- and 10-bit 4:2:0 and 4:4:4 pictures are read (12-bit is accepted, but rust_h265 calls it untested); 4:2:2 and monochrome HEVC, which some cameras write as `.hif`, are refused from the header ("RAWmakase can't edit 4:2:2 HEIF images") and can't be shown either, as a tiled AVIF can't; alpha, depth maps and thumbnails are not read. A grid's tiles may not be cropped or turned themselves, and every tile is decoded whole, so the tiles, not only the image, are held to 150 MP; each tile's coded data is read only up to what a picture its size can take, and a picture coded wider or taller than the header says, in whole 64-pixel blocks, is refused before the decoder reads its sequence parameter set (the decoder itself holds only the area to a limit, then allocates whole blocks). An iPhone's HDR gain map is not applied: the photo shows and edits as its standard image. HDR (PQ or HLG) and image sequences follow the AVIF rules above.
- **Embedded profile.** The only profile a rendered file has, named as Lightroom names it. Its forward matrix maps ProPhoto to itself and its tone curve is ACR3, Camera Raw's default; it has no hue or look tables, baseline exposure or default black. `Recipe::for_rendered` gives the defaults: no sharpening or colour noise reduction, Kelvin 6500 K and 0, unit gains. A saved Embedded profile gets this tone curve when the recipe loads.
- **Identity contract.** The file is converted to a pseudo-scene once, when it opens: the inverse of Camera Raw's default rendering (`develop::rendered`, `photo::open_source`; [scene tone stage](scene-tone-stage.md#rendered-files)). At the defaults the render reproduces the file within ±1/255 (finer in 16 bits), on the CPU and the GPU. Exposure and the tone sliders act on that scene as they do on a RAW's, so Lightroom's highlight roll-off applies. Because an unedited file renders as it is, the Library and Loupe show it from the file, and develop only an edited one; preview builds skip unedited ones.
- **White balance of rendered files.** Temp and Tint are Lightroom's relative values, −100 to 100, saved as `incremental_temperature` and `incremental_tint` (Lightroom's `IncrementalTemperature` and `IncrementalTint`). `model::white_balance::rendered_white_balance` reads them as the DNG SDK would with a camera whose RGB is linear ProPhoto and no forward matrix: measured tables (`RENDERED_TEMPERATURE`, `RENDERED_TINT`) name a white as a DNG reciprocal temperature and tint (`xy_reciprocal`), with the file's D50 white (199.9 mired, tint +9.6) at 0 and 0; Temp moves only the temperature and Tint only the tint, and the two combine. The white balance is a 3×3 matrix, not channel gains: Bradford adaptation from that white to D50 (each cone's scaling held to 0.1–10, as the SDK's `MapWhiteMatrix` holds it), normalised as the SDK normalises a camera neutral, so warming and cooling both brighten the photo as in Lightroom; before it each channel is held at the white in camera RGB, which in the cool magenta corner (strong cooling with strong magenta, where the white's ProPhoto blue is negative) takes blue to 0, as Lightroom does. Where that clip sits against the scene (four times the camera white) is measured, not the SDK's. The pipeline's camera stage applies both (`develop::camera_stage`, on the CPU and the GPU), and a rendered file's `wb` gains stay 1: Temp and Tint are its white balance. Against Lightroom Classic 15.6 (Camera Raw 18.7) on synthetic charts, greys are within about 2/255 on average (mostly within 1) from Temp −35 to +100 and Tint −75 to +75, alone and combined, and 3–7 at Temp −50 to −100 and Tint +100; colours' 95th percentile is a median 8/255 over the white balance files (24 with channel gains), and on 32 photos at Temp +35 the mean is 1.1/255 (3.1 with gains; `scripts/corpus/rendered-white-balance.py`). Known gaps: in the cool magenta corner red is a few levels high, as the matrix leaves ProPhoto blue below 0, which the tone stage carries; at Temp −50 with Tint +50 greys are about 10/255 off; and at Temp −25 with Tint +100, where the white's blue cone is about 0, Lightroom warms the photo instead (about 40/255 off). `Recipe::pick_white_balance` turns the eyedropper's and Auto's gains into the nearest Temp and Tint (`rendered_controls`: the pair whose matrix renders the picked colour most neutral, cross-channel terms included). Kelvin `temperature` and `tint` stay 6500 and 0, so releases without relative white balance still read the recipe. The WB menu offers As Shot (0 and 0), Auto and Custom, and MIDI and the control socket move the same relative values (`ParameterId::for_photo`); Lightroom's Kelvin presets are for RAW files. The eyedropper's and Auto's gains (Auto is gray world, without the warmer shift measured on RAWs) become the nearest relative Temp and Tint, so the next slider move does not jump; Auto records that pair in `auto_white_balance` to show Auto in the menu. Between a RAW and a rendered file, Copy and Sync keep the target's white balance and profile.

# Color and rendering contract — pipeline version 1

## Native development boundary

RAW files are decoded by RAWmakase's own decoders (`crates/rawmakase-raw`, see [RAW decoding](raw-decoding.md)) and developed in `rawmakase-decode`'s `raw` module: each sample of the visible area becomes (raw − black) / (white − black) in floating point, with the decoder's per-channel black and black pattern, then is white balanced by the as-shot multipliers and demosaiced ([demosaicing](demosaic.md)); nothing is scaled to integers, so nothing saturates before the renderer, and values above one are kept. Half-size drafts are 2 × 2 blocks of the mosaic. Small RAWs (Canon sRAW and mRAW, Nikon's and Sony's) are converted to camera RGB by the decoder and develop without demosaicing, at full size in both modes. Monochrome DNGs develop as grey, R = G = B, through a neutral matrix and without white balance (Temp and Tint do not apply; see [RAW decoding](raw-decoding.md)). Only three-colour Bayer and X-Trans mosaics, those small RAWs, linear raw DNGs and monochrome DNGs develop; any other file fails with an error that names what it is. Orientation is handled by Rust exactly once. LibRaw, which developed photos before, is linked only as the oracle of the decoder-comparison commands until it is removed.

## White balance

Development is cached at as-shot WB. Relative camera-space multipliers are applied before the camera matrix in Rust, without rerunning demosaicing. Metadata records both as-shot and daylight multipliers. A Planckian-locus CIE xy approximation, transformed through the inverse camera matrix and daylight normalization, estimates the as-shot temperature. Temperature/tint changes are anchored to this reference so touching a slider does not jump from camera WB to a different model. Stored relative multipliers are authoritative; Kelvin is approximate.

The neutral picker samples a 5 × 5 region in camera space via the same inverse geometry mapping used for rendering. It derives relative gains from that patch. Extreme WB changes can differ from WB-aware re-demosaicing; there is no claim of exact equivalence.

## Rust pipeline

Historical: pipeline version 1, no longer rendered.

1. Cache demosaiced as-shot camera RGB (`CameraImage`, f32). Keep metadata, source orientation, default inset and development diagnostics with it.
2. Apply relative WB. Use the decoder's camera-to-linear-sRGB matrix, then a fixed linear-sRGB-to-Rec.2020 matrix. Exposure multiplies by `2^EV`. Negative matrix results and values above one survive this boundary.
3. Basic edge-aware luma/chroma averaging uses a 3 × 3 camera-space neighborhood at original-image scale. The implementation performs this before WB/color conversion. Fit previews omit it; full exports and 100% regions include it.
4. Shadows/highlights/whites/blacks modify luminance with smooth range weights. Version 1 uses global mappings, not spatial/local contrast operators.
5. A luminance-preserving `Y (2.2Y + 0.05) / (Y (2.2Y + 0.6) + 0.1)` tone mapper brings scene luminance into display range. Tone-mapped linear sRGB is transformed to Oklab for saturation, vibrance, smoothly overlapping hue bands and grading. Near the normalized sensor ceiling, chroma is reduced to suppress false clipped-channel colors. This is neutralization, not highlight reconstruction.
6. Bring out-of-sRGB colors into sRGB ([out-of-gamut colors](#out-of-gamut-colors)). Apply the sRGB transfer function, then levels, contrast and an editable shape-preserving cubic master curve in encoded output space. Output is bounded to [0,1].
7. Geometry uses one inverse map: camera default inset, metadata orientation plus quarter-turn, flips, straighten with an inscribed auto-fit, and normalized crop in the oriented/straightened canvas. Rendering samples through this map; global color operators remain shared between full frames and tiles.
8. Resized exports first render the full edited image, then area-average output samples and apply output sharpening. Full-size exports and 100% region rendering apply identical 3 × 3 sharpening with matching boundary halos. The region/full-frame equivalence test includes denoise, sharpening and rotated geometry.
9. Quantize to 8-bit JPEG or PNG, or 16-bit TIFF, at export. Embed RAWmakase's sRGB profile (`assets/srgb.icc`, the one exports have always carried, first written by Little CMS 2.19), plus camera make/model, orientation 1, ISO, exposure, aperture and focal length. Old thumbnails and maker-note offsets are not copied.

The fit preview area-averages camera data to a maximum 1600-pixel edge before the shared color operations, trading exact spatial/nonlinear equivalence for responsive interaction. It omits detail processing. The histogram describes the whole photo, as that preview, also at 100%; clipping overlays describe what is shown: that preview or the visible 100% region. Fit and export can differ at fine edges; use 100% for detail decisions.

## Out-of-gamut colors

Colors the edit pushes outside sRGB, such as a saturated orange brightened past white or a strong white-balance shift, reach sRGB by clipping (`finish_color` in `develop/pipeline/pixel.rs`, and `develop.wgsl` on the GPU): each linear channel is clipped to 0–1 on its own, as Camera Raw's conversion to sRGB does. A color keeps its in-gamut channels, so a too-bright orange turns toward yellow rather than toward gray. Earlier releases compressed instead: chroma moved toward the neutral of the same Oklab lightness until every channel fit, which kept the hue but desaturated; edits saved with compression now clip too.

On the corpus chart (Camera Raw 18.7, 369 cases), clipping lowered the mean ΔE00 from 1.62 to 1.29: the default render 0.91 → 0.66, white balance at 8000 K under illuminant A 4.22 → 0.89 and at 5000 K 2.45 → 0.69, the parametric curve cases 0.76–2.95 → 0.48–1.15, Color Grading 0.75–2.02 → 0.56–1.43, Point Color 0.83–1.82 → 0.69–1.11 and Camera Calibration 0.97–2.21 → 0.71–2.02. Positive Highlights moved further off (+60: 5.27 → 5.89, +100: 7.98 → 8.66), and Dehaze and Shadows +100 by up to 0.15. These figures predate the scene tone stage.

## Display

The renderer returns encoded sRGB. `egui::ColorImage::from_rgb` supplies those bytes to egui's own texture/compositing path; there is no extra application gamma pass. The viewport measures logical-point dimensions times `pixels_per_point` when asking for a 100% region, then paints the result at one texel per physical pixel.

Clipping is judged on the rendered encoded-sRGB values (0–1), before any monitor profile, so the display never changes it. A channel clips in the highlights at 0.999 or above and in the shadows at 0.001 or below (`HIGHLIGHT_CLIP`, `SHADOW_CLIP` in `develop/rendered.rs`; the GPU's `present.wgsl` uses the same values and counts clipped pixels per channel while it counts the histogram). The highlight warning paints red where any channel clips, the shadow warning blue where all three do. Each histogram triangle lights when more than 0.1% of the pixels clip in a channel, in the colours of the clipping channels (white when all three).

Develop's RGB readout under the histogram (and the White Balance loupe) reads these rendered values, never the screen, and shows them as Lightroom does in Melissa RGB: linear sRGB to ProPhoto primaries (the D65-to-D50 matrix the camera profiles use), then the sRGB tone curve.

Preferences > Monitor profile follows the system by default, as Lightroom does.
On macOS that explicitly tags the Metal surface as sRGB so
Core Animation matches it to the current monitor, including wide-gamut displays.
On Windows it converts to the ICC profile Windows assigns to the display the
window is on (`GetICMProfileW` on that display, checked again when the window
reaches another display and every two seconds), unless the display is in
advanced color (automatic color management or HDR), where Windows converts sRGB
itself. Without that conversion, Windows shows sRGB pixels as device RGB, which
looks oversaturated on a wide-gamut display (issue 550). Linux has no system
profile yet and shows sRGB as it is. "Use sRGB" shows sRGB pixels as device RGB
on every platform, for a display set to sRGB; it is saved as `monitor_srgb`
beside no profile, which versions before it read as their own "Use sRGB". A
session from before it with no profile follows the system.
`app::display` schedules this declaration in the paint callback, after wgpu can
reconfigure the surface; `platform::display` finds the current Metal layer and
sets its color space. This works around wgpu-hal 30.0.1 leaving sRGB surfaces
untagged ([upstream fix](https://github.com/gfx-rs/wgpu/pull/10286)). No additional
gamma or pixel conversion is applied by this workaround.

Optional monitor ICC conversion maps encoded sRGB bytes to device RGB
(`color_management::display_transform`), relative colorimetric with black-point
compensation, in pure Rust: profiles are read with moxcms; a matrix/TRC monitor
profile is applied by its colorants and tone curves (inverted exactly, so a raised
black stays right). So is a LUT-based profile whose lut8/lut16 tables connect
through XYZ, such as DisplayCAL's default "XYZ LUT + matrix" profiles, evaluated
as Little CMS does: the table's matrix, input curves, tetrahedral CLUT and output
curves. moxcms ignores that matrix and miscolours such profiles (green, in issue
548). Any other LUT-based profile goes through moxcms's transform from sRGB. Its
perceptual tables stand in for colorimetric ones it lacks, as ICC asks.
Black-point compensation scales XYZ per channel so sRGB's black lands on the
monitor's (Adobe's published method); the monitor's black is its device black
through the colorimetric transform (the A2B tables when the profile has A2B1/B2A1
or matrix/TRC tags, as in Little CMS), and none for a LUT profile with only
perceptual tables (whose black is the reference medium's). A profile that cannot
be read or used is an error and leaves the pixels as they were.

Measured against Little CMS 2.19, which RAWmakase used before (relative
colorimetric, black-point compensation, 8-bit), over the dense grid 0, 5 … 255 in
each channel. The goal, agreement with its 8-bit result within one code value
everywhere, is not met in full: matrix/TRC profiles (synthetic Display P3, Adobe RGB and a raised
black, and this machine's four monitor profiles) agree within one code value
everywhere, or within Little CMS's own rounding where its 8-bit result is more than
one from its floating-point result. Synthetic LUT profiles with raised blacks miss
that in 0.01–0.05% of the grid, saturated shadows where moxcms's resampling of the
profile's table differs by up to 6.5 code values (Little CMS's own 8-bit result is up
to 7 from its floating-point one there). Synthetic DisplayCAL-style profiles (XYZ
lut16 tables with a matrix or a CLUT, matrix/TRC tags, raised blacks) agree with
Little CMS's 8-bit result within one code value everywhere. `color_management`'s tests hold the
committed fixtures (`crates/rawmakase-decode/tests/data/monitor`) to this; an
ignored test runs the dense grid over a folder of profiles and Little CMS results.
Choosing this manual override, or sRGB, on macOS leaves the Metal surface
unmanaged to avoid applying a second conversion; choosing the system profile
restores system color management. Failure of the manual transform still shows the sRGB bytes with a
visible error, but on macOS the surface stays unmanaged while a custom profile
is selected, so that fallback is not matched to the display and can look
oversaturated on wide-gamut monitors. The Linux display profile (colord, X11's
`_ICC_PROFILE`) and HDR output are outside this release. Native tests check
the actual Metal layer tag, surface reset/replacement and switching manual
conversion on and off; a test proves an sRGB-profile round trip within one
byte. Visual matching with other applications still requires
validation on the chosen display.

## Invalidation and ownership

WB/tone/color/crop changes reuse the as-shot development. A single replaceable pending render request coalesces slider activity; results carry generation IDs and obsolete results are discarded. Full-resolution loading is serialized and cancelable inside the decoders' loops. Thumbnails are limited to 32 neighbors. Export holds an immutable recipe and the active development; navigation waits for it to finish. No texture contains a full 24 MP image just to display a fit preview.

Historical (pipeline 2): sidecars recorded schema 2 and pipeline 2. Version 1 curves load with their original linear interpolation; editing a curve switches it to smooth interpolation. Unknown versions and changed source identities are preserved and disable autosave for that photo. Legacy sidecars and presets migrate on save; their curve shape remains unchanged until edited.
