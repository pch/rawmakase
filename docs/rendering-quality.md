# Rendering engine

RAWmakase renders every edit with one engine, whichever release saved it. Exposure and the Basic panel's tone controls (Whites, Blacks, Highlights, Shadows, Clarity, Texture and Dehaze) render on scene values before the camera profile's look and tone curve, where Camera Raw 18.7 applies them: the [scene tone stage](scene-tone-stage.md). Contrast, the parametric curve, the color mixer, Saturation/Vibrance, color grading, calibration, sharpening, colour noise reduction, grain and manual lens vignetting are responses measured against Camera Raw too: see [tone controls](tone-controls.md), [color mixer and grading](color-mixer.md) and [lens corrections](lens-corrections.md). Earlier engines and the earlier operators of each control are no longer rendered: an edit saved with them renders with the current engine, and the settings that chose them are dropped when it is read ([color pipeline](color-pipeline.md)). Saved recipes are schema/pipeline 11 today (see [imported Lightroom profiles](lightroom-profiles.md) for the embedded profile data). Older binaries reject newer files rather than misreading them.

See [preview performance](preview-performance.md) for how previews are rendered today, timing measurements and supported-hardware validation.

## Preview and detail

Every change renders the real pipeline at viewport size in physical pixels (including display scale), from a resolution pyramid for Fit and zoomed-out views. When a render takes more than a moment, an edit publishes a reduced render first, labeled **Draft • refining**, and the full render replaces it: at 100% a reduced render of the region (also when panning), and for the whole photo one with at most a megapixel. Fast renders, and views still cached, go straight to full resolution. While an edit continues, the running render finishes and shows before the next one starts; a new photo, view or size cancels it, and an older result never replaces a newer one ([architecture](architecture.md#previews)).

Sharpening (`develop/sharpening.rs`) is applied before resizing. New edits and Lightroom imports start at Lightroom Classic's raw defaults (Amount 40, Radius 1.0, Detail 25, Masking 0; every raw and DNG import in the catalogue sampled, process versions 2012 to 6) and render with an unsharp mask fitted to Camera Raw 18.7 on a synthetic chart (slanted edges, sine gratings of 0.04 to 0.3 cycles per pixel, texture, noisy flats; 45 renders). It is about 2.4 times the strength of RAWmakase's earlier operator (a luminance-only Gaussian unsharp mask, no longer rendered) on fine detail per Amount, shapes the high-pass as `d / (1 + (|d|/halo)³)` so strong edges get small halos, makes dark halos 0.57 of light ones, scales the strength with Detail (0.3× at 0, 2.1× at 100) and fades out detail below a Masking threshold. The Detail panel's reset gives the Lightroom defaults. Camera Raw's ringing at Detail above 50 and its gentler sharpening of noise in deep shadows are not modelled. Full-resolution region rendering includes the required sharpening halo, and its pixels match the full render. No separate Fit-only sharpening changes the result. Luminance noise reduction is described below.

Color noise reduction (`develop/color_noise.rs`) starts at Lightroom's raw default of 25 (Detail and Smoothness 50) and follows Camera Raw 18.7 as it renders photos, whose noise it knows from the camera's noise profile. The chroma of the camera image, in square-root values (where shot noise is about the same at every brightness), is split into a Laplacian pyramid. At each level the colour noise is measured in the photo itself: the RMS of the level's detail in its smoothest 10% of 8×8 blocks, leaving out blocks near the sensor's clipping and, unless too few others remain (a dark, noisy photo), blocks with black pixels; a coarser level never counts more noise than the finer one, and a level with fewer than 16 such blocks takes 0.6 of the finer level's. Each level's detail is then kept by the sixth power of its size over a threshold, a multiple of that noise set by Amount, Detail and Smoothness, so noise goes and strong colour edges stay sharp. On synthetic RGGB charts with the noise of a Nikon D7100 at ISO 2000 (from issue 465) and 1/16, 1/4 and 4 times it, each carrying its DNG noise profile, Camera Raw keeps the same share of the noise at each scale whatever its level, and at Amount 25 takes out about 95% of it at every scale; it keeps a little more of a clean photo's coarse colour, which the thresholds follow as a power of the finest level's noise. Fitted to those renders (Amount 10–100, Detail and Smoothness 0–100), the share of noise kept at each of four scales differs from Camera Raw's by a mean |log ratio| of 0.13–0.24 over the 80 cases, against 1.4–2.0 for the operator fitted earlier to charts without a noise profile, where Camera Raw assumes little noise (`scripts/corpus/color-noise.py`). On the D7100 photo the noise measured at the finest level is 0.0086, against 0.0083 predicted from the noise profile Adobe's DNG Converter writes for it. It runs once on the camera image before everything else, as spot removal does. The Detail panel's reset gives Lightroom's default. Edits from earlier releases render with it too; the operators they were saved with are not kept.

Luminance noise reduction (`develop/luminance_noise.rs`) runs on the full camera
image after Color noise reduction, before retouching, the preview pyramid and tone
mapping. A five-level Laplacian pyramid of square-root luminance separates fine
grain from multi-pixel blotches. Each band estimates its noise from the tenth
percentile of 8×8 block energies, excluding near-clipped and mostly black blocks;
small levels fall back to the finer band's estimate. Amount raises a soft threshold,
Detail protects fine structure and Contrast protects coarser structure. A band's
3×3 neighborhood energy distinguishes coherent detail from isolated noise specks. Color Detail
does not control luminance reduction. The result preserves square-root chroma.
Both amounts at zero bypass the stage; saved slider values and XMP units are
unchanged. The preview cache keys the source and all noise settings, and CPU and
GPU rendering sample the same denoised image. This is an approximation, not Adobe's
algorithm: texture-only scenes can overestimate noise, and high amounts can lose
faint detail.

### Noise reduction on a reduced decode

A reduced decode (such as the half-size draft) keeps its photo's
metadata over fewer pixels (`CameraImage::photo_scale`). Both noise reductions treat
its pyramid as the photo's from level `−log₂(photo_scale)` on: level 1 for a
half-size draft, a little under 1 for one of a photo with an odd side
(`demosaic::half` rounds up), 1.2 for 2560 pixels of a 6000-pixel photo. Colour
noise reduction takes the photo's thresholds there (geometrically between two
levels, the noise exponent linearly), luminance noise reduction its Detail and
Contrast weights. Between two of the photo's levels a level's settings are
interpolated. A level that straddles the start of the photo's unfiltered residual
takes the last level's settings and is filtered in proportion to its part before the
residual: of each detail, it removes that share of what a whole level would. A level
that starts at the residual, or past it, is left alone. So noise reduction changes
continuously with the decode's scale. Filtered whole or not at all, the straddling
level changed it in a step where it reached the residual (offset 1 for a half-size
decode): 1.3 to 2.1% of what noise reduction changes, 44 and 88 times the median
step for colour across half-size decodes of photos 1003 to 1043 pixels wide, against
at most 1.14 times now. The noise each level is measured with is the decode's own. A
full decode, whatever its metadata says, filters exactly as before.

Measured on a synthetic photo (an RGGB mosaic of flats and texture with shot noise,
demosaiced bilinearly) against the photo denoised and rendered at the reduced size,
as the share of the noise each keeps at 1, 2, 4 and 8 output pixels relative to the
share the photo keeps: for its half-size decode and for it reduced to 0.43 by area,
the worst |log ratio| over six settings fell from 1.02 to 0.71 (mean 0.37 to 0.26);
for colour at its default 25, from 0.47 to 0.13. At 2 output pixels and coarser the
worst fell from 0.63 to 0.43 (mean 0.28 to 0.15), every setting closer than before
but two reduced to 0.43, a little further than before: colour Amount 10 (0.43
against 0.40) and colour 25 with Detail and Smoothness 80 (0.25 against 0.20). At 1
pixel a reduced decode also holds noise aliased from the photo's finer scales, which
the photo's finest level took out: at colour Amount 10 a decode keeps about twice
the photo's share there, and with Luminance Detail 20 and Contrast 50, 1.4 times.
Starting the decode's levels half a level finer instead helped luminance and made
low colour Amounts worse. Texture keeps its detail within 5% of the photo's (the
test allows 10%). The half-size decode of a photo with an odd side denoises within
0.03% of what noise reduction changes of an even photo's. AI Denoise refuses reduced
decodes.

## AI Denoise

AI Denoise replaces the demosaic for a photo with RawNIND
UtNet2 (Bayer), which denoises and demosaics the mosaic in one pass. The photo's
visible mosaic is packed as RGGB planes (an X-Trans mosaic is demosaiced, its white
balance taken off and sampled again as RGGB), run in 512-pixel packed tiles with 64
pixels of mirrored or real context, and brought to the mosaic's scale by one gain
fitted on 8×8 packed blocks without clipped samples or dark channels. Fed a mosaic
clipped at black, the model lifts the deep shadows; their channels are scaled back
to the mosaic's own mean over 16-pixel packed blocks, in full below 0.01 of white and
not at all above 0.03. The result is white balanced as development balances the
mosaic; within 8 pixels of a clipped sample, feathered over 8 more, it is the plain
demosaic, which the model never saw unclipped. Highlights are then recovered on this
image of its own, never with the plain image's recovery, since recovery depends on
each pixel's values.

Rendering blends the two recovered images by Amount before colour and luminance noise
reduction, which then apply as without it: the AI source's change in luminance (the
channels' mean) and in colour each by a weight, the AI source itself at 100%. The
weights are Camera Raw 18.7's: its Denoise records the blend it applies (`BlendParams`)
as 0.4375 and 0.5 at Amount 25, 0.70 and 0.75 at 50 and 1 at 100. That they are
luminance's and colour's, the channels' mean as luminance, and linear interpolation
between them (from 0 at 0) are RAWmakase's reading, not established Camera Raw
behaviour. Switching AI Denoise on sets manual Luminance and Color noise reduction to
0, and switching it off brings Color's defaults back, as Camera Raw writes them from
its defaults (how it treats manual noise reduction the user changed is not measured); everything after is unchanged. Half-size drafts and a photo
without a ready result (computing, model not installed, failed) render the plain
image, the state shown beside the switch; export never does, and fails the photo
instead. The recipe names no model: as with every other setting, an edit renders
with the installed model, and results are cached under a key holding the model's
SHA-256, the adapter version, the provider and the build, so a changed model or
adapter recomputes them. Cached and fresh results differ only by half-precision
rounding. Measured on an M1 Pro, an Amount change updates the Fit preview in
155 ms at 24 MP and 357 ms at 61 MP, and Develop's resident memory while dragging
grows by 0.80 GB (24 MP) and 0.90 GB (61 MP).

The Library's edited previews, Compare, Survey and the Loupe show AI Denoise only
from a finished result in the cache for the photo's file and the installed model, on
any provider. The previews then decode the photo at full size, as Develop does, and
assemble the result over it. Without a finished result they render the plain image,
from the half-size decode, as before. The Loupe never runs the model, and doesn't show
a partial result. Once any job stores a result, previews of that file shown plain are
made again. A preview records what it shows in its cache tag: no AI Denoise (`-fit`), the
plain image of an edit with it (`-fit-noai`), or the finished result of a named model
(`-fit-ai-<model>`). The plain and AI previews never replace each other, the plain one is
not used once a result is finished, and a changed model makes the preview again. Develop
gives its render to the Library only once its result is final; a render with a partial
result is shown in Develop only. Previews that earlier releases kept for edits with
AI Denoise, under `-fit` with or without the result, are not shown again. An offline photo
keeps whichever preview it has. Only RAWmakase's own edits are read this way. An imported
Lightroom edit with Denoise previews plain in the Library until it is edited in Develop.

The choices above were measured on 2026-11-02 (M1 Pro, ONNX Runtime 1.23.2; the 190
CC0 photos above ISO 400 in the pixls archive, RawNIND's held-out test pairs). Tiles
start at rawnind_jddc's grid origin: with the padded image's own grid, 9 of 59
non-RGGB photos differed from upstream's alignment by up to MAE 0.009 and p99.9 0.17
after the comparison transform, against at most 2 × 10⁻⁵ and 0.0017 on upstream's grid.
One gain serves all three channels: on mid-tone blocks they agree within 3.2%; the
Canon R10's 14% spread at ISO 32,000 came from lifted dark blocks, which the fit now
leaves out. Overlap 32 left a band at tile borders 5–7 times the difference elsewhere,
so the overlap is 64. Deep shadows were lifted up to 2.5 EV at ISO 204,800 before the
correction. Against the scene's low-ISO frame (both in linear Rec.2020, a
per-channel least-squares gain onto the reference, clipped and sRGB-encoded, then
MS-SSIM over 5 scales), AI Denoise beat the best of manual Luminance 0, 25, 50 and 75 on
82 of RawNIND's 85 noisy Bayer frames (mean 0.942 against 0.779) and on 19 of 27 X-Trans
frames (0.955 against 0.904; its losses at ISO 2500 or below, by at most 0.01), with no
maze or grid at 100%. No tile was non-finite on the CPU or CoreML, which stays within
1.2 × 10⁻⁶ (relative) of the CPU, so CoreML's GPU (an ML Program in single precision) is used on
Apple Silicon: a 24 MP photo takes about 3.5 s there and 20 s on the CPU. Against Camera Raw 18.7's
Denoise on five of those CC0 photos at Amount 50 and 100 (100% crops, side by side):
similar at ISO 12,800–20,000 (Camera Raw keeps slightly more fine texture); behind at
ISO 32,000 and above, where RawNIND leaves colour blotches in dark areas and Camera Raw
keeps more detail, and on X-Trans, where the RGGB rebuild leaves the result softer and
mottled beside Camera Raw's own X-Trans model.

## Tone and highlights

Profile-based recipes also store a camera exposure baseline independently of the Exposure slider. The verified X100F DR100 reference baseline is +0.15 EV; unverified camera/DR combinations use zero. Point curves use a natural cubic spline and the sRGB transfer function in ProPhoto primaries, with a hue-preserving master pass followed by independent RGB curves. Clipped input endpoints and 0–255 point editing are supported. Contrast preserves endpoints. See the comparison report for reference-derived metadata and limits.

Every resolved recipe has a camera profile (without an imported or embedded one, the camera matrix with the DNG default tone curve) and uses the profile tone curve. Before it, the scene tone stage keeps scene values above 1 and rolls them off to a white point measured from the sensor's white and the photo's maximum, as Camera Raw's default render does; Shadows and Highlights are a measured local operator there ([tone controls](tone-controls.md#shadows-and-highlights)).

Highlight reconstruction follows Camera Raw 18.7's measured rule in white-balanced camera space, before color conversion ([partially clipped highlights](#partially-clipped-highlights)): every channel is pulled up towards the mean of the other two by a weight that rises with its own sensor level, so a clipped channel becomes that mean and a channel near the sensor's white mostly follows the others. It cannot recreate fully clipped texture. The recovered image is lazily cached per decoded image. Extreme WB changes after demosaicing still have limitations.

Master point curves still have a saturated-color residual. The parametric curve is measured ([tone controls](tone-controls.md#parametric-curve)). Generated-ramp regression data checks neutral master and independent RGB curves separately from RAW/profile differences; see `tests/data/README.md`. Exact Lightroom rendering parity is **not** established.

## Color mixer and grading

The color mixer, Saturation, Vibrance, color grading and Point Color render Camera Raw 18.7's measured responses: see [color mixer and grading](color-mixer.md). Legacy split-tone XMP imports restore blending 100 and clear modern grading controls. The earlier approximations, and the `reference_color` choice between them, are no longer rendered; the [color validation table](records/macos-lightroom-validation.md) records how they compared.

## Camera calibration

Calibration renders neutral-preserving primary Hue/Saturation matrices plus an asymmetric green/magenta shadow tint. The Calibration panel groups Shadows, Red Primary, Green Primary and Blue Primary, using −100–100 values; its Process row shows the current version, the only one. Calibration does not modify the installed DCP.

The primary sliders render Camera Raw 18.7's measured changes (in `calibration.rs`), replacing the original coefficients, measured on X100F/Adobe Standard. Camera Raw rendered a dense synthetic chart (1,728 colors and a gray ramp) with each primary's Hue and Saturation at −100, −50, +50 and +100. For each position, the six numbers of a neutral-preserving matrix at RAWmakase's calibration step were fitted by rendering the same chart through RAWmakase's own pipeline (Gauss–Newton on CIELAB differences), and positions in between are interpolated. Camera Raw leaves grays alone under every calibration slider, as the matrices do. On the fitting chart the 24 renders went from mean ΔE00 1.0–3.4 to 0.66–0.80, against 0.80 for the default render there; on the corpus chart the `calibration-*` cases went from 1.11–2.02 to 0.62–0.68 (default 0.66), and calibration combined with the color mixer from 1.55 to 0.66. Several sliders add their changes, which was checked only on that pair. Shadow Tint keeps its earlier fit (0.71 and 0.91 on the chart). The chart is one synthetic camera with its embedded profile; other cameras and profiles are not measured.

## Partially clipped highlights

Highlight recovery runs once on the full decoded camera image, before the scene
tone controls, and feeds Fit, regions and export. It is Camera Raw 18.7's rule as
measured on flat patches rendered through a linear profile (`scripts/corpus/probe_dng.py`
with an As Shot neutral, so channels clip at different scene levels): in
white-balanced camera space each channel is pulled up towards the mean of the other
two, never down, by the weight `((s − 0.25) / 0.75)^1.8`, where `s` is the channel's
own fraction of the sensor's white. A clipped channel (`s` = 1) becomes the mean of the
other two outright; a channel at 0.9 of its white follows them by 0.77 of the gap, one
at half by 0.14, one in the lower quarter not at all. The
three pulls are solved together (each channel's target is the others' pulled values,
anchored at what the sensor recorded), so with two channels clipped the lower one lands
between the unclipped channel and the higher clip, and once the unclipped channel passes
the higher clip the pixel is neutral at that level, as Camera Raw renders it. Nothing
reads neighbouring pixels: on flat patches Camera Raw's result is exactly this per-pixel
rule, and a clipped sky keeps whatever gradient its unclipped channels carry.

Measured on 136 patches (six hue directions at two saturations and six levels of the
sensor's white, all unclipped; four hues at eight levels through every clip, under two
As Shot neutrals), the rule reproduces Camera Raw within 0.005 RMS of the level per
channel, the shoulder both renderers apply above white excluded. It replaces the
neighbourhood-ratio estimate and the connected-area colour map of earlier releases: those
reconstructed the patch's true colour (a warm patch with green clipped stayed warm,
where Camera Raw moves it towards neutral and brighter), which is why clipped skies
rendered duller and pinker than Camera Raw's. The rule also changes unclipped colours
whose channels approach the white, as Camera Raw does: a saturated red at 0.9 of its
white loses a third of its chroma. This infers Camera Raw's colour, not missing detail,
and is not Adobe's algorithm. Cancellation discards the recovered image.

## Camera profiles

DCP ColorMatrix1/2 now support DNG temperature/tint and camera-neutral inversion. Camera calibration is signature-matched; missing color matrices fall back to the original WB approximation. Profile matrices, signatures and rendering parameters are embedded in saved recipes.

RAWmakase supports a bounded DCP subset: three-channel forward matrices, one or two standard illuminants, reciprocal-temperature interpolation, interpolated HSV calibration tables, linear/sRGB-indexed look tables, profile tone curves and baseline exposure offsets. The profile curve processes the low/high channels and interpolates the middle channel to preserve hue. DCPs without an embedded curve use the Adobe DNG SDK default curve. The profile tone stage clips to the SDR domain. This is an independent implementation, not exact Adobe processing-order parity.

No Adobe profiles are bundled. New edits prefer a matching imported Adobe Color, then Adobe Standard profile, then a DNG's embedded profile; without those, every camera uses RAWmakase Color, our own look over its camera matrix (see [RAWmakase profiles](lightroom-profiles.md#rawmakase-profiles)). Published third-party DCPs, such as RawTherapee's, can be imported like any other profile. Profiles for another model are rejected, never substituted.

The **Profile** selector lists compatible DCPs in the local `camera-profiles` library. Each photo opened lists them again; the camera each DCP was made for is remembered while the app runs, keyed by the file's size, modification time and (on Unix) status change time, so after the first scan only the photo's camera's profiles are read. **Import profile…** loads a matching DCP file. Profile data and copyright are embedded into JSON recipes, so moving the profile file doesn't change an existing edit. Histories share immutable profile data. Matrix-only DCPs, HDR/triple-illuminant profiles and profiles forbidding embedding are rejected with an explanation. Compatible enhanced XMP look profiles can be explicitly imported together with their base DCP; see [supported look features and comparisons](lightroom-profiles.md). Profiles alone cannot reproduce Lightroom's demosaicing, local tone operators or detail algorithms.

Specification used: Adobe Digital Negative Specification 1.7.1.0, camera profile tags and chapter 6:
https://helpx.adobe.com/content/dam/help/en/photoshop/pdf/DNG_Spec_1_7_1_0.pdf

## Reference comparison

Export the same RAW from Lightroom at full resolution in sRGB, with matching orientation, WB, exposure, crop, and lens corrections. Use 16-bit TIFF for the least quantization loss. Then run:

```
rawmakase compare source.ARW lightroom.tiff /tmp/sony-comparison --origin 2400 1200
```

The output directory must not already exist. `--recipe preset.json` optionally supplies RAWmakase edits; without it the current engine defaults are used, independently of sidecars. The tool writes unscaled 512-pixel crops, a 4× difference visualization, the exact recipe, and full-image sRGB MAE/RMSE/PSNR. These metrics measure differences, not quality, and do not compensate for exposure or registration. A reference with different dimensions is rejected rather than stretched.

Five X100F Lightroom comparisons across three photographs were checked on macOS, including custom white balance, master/RGB point curves and nonzero tone sliders. The corrected profile tone substantially reduced preview error, but full Lightroom parity is not established. See [macOS and Lightroom validation](records/macos-lightroom-validation.md) for measurements and limitations. The supplied private archives installed 62 matching camera profiles and 603 Settings presets in the user data directory; none are bundled. See [XMP support](xmp-presets.md).

CLI profile selection: `rawmakase render source.RAF output.jpg --profile /path/to/profile.dcp`. Profile libraries are read from `$RAWMAKASE_DATA_DIR/camera-profiles` and the platform RAWmakase data directory (`~/Library/Application Support/RAWmakase` on macOS, XDG on Linux); camera matching is mandatory.
