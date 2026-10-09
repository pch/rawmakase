# Rendering engine

RAWmakase renders every edit with one engine, whichever release saved it. It is the former engine 4 (process version 4): engine 3's camera profiles, point curves, highlight reconstruction and detail processing, with the Basic tone sliders, Shadows/Highlights, the parametric curve, the color mixer, Saturation/Vibrance, color grading, calibration, sharpening, colour noise reduction, grain and manual lens vignetting replaced by responses measured against Camera Raw: see [tone controls](tone-controls.md), [color mixer and grading](color-mixer.md) and [lens corrections](lens-corrections.md). Earlier engines (process versions 1–3) and the earlier operators of each control are no longer rendered: an edit saved with them renders with the current operators, and the fields that chose them are kept unchanged in the saved recipe but ignored ([color pipeline](color-pipeline.md)). Saved recipes are schema/pipeline 6 today (see [imported Lightroom profiles](lightroom-profiles.md) for the embedded profile data). Older binaries reject newer files rather than misreading them.

See [preview performance](preview-performance.md) for how previews are rendered today, timing measurements and supported-hardware validation.

## Preview and detail

There is no reduced interactive draft: every change renders the real pipeline at viewport size in physical pixels (including display scale), from a resolution pyramid for Fit. At 100%, an edit or pan whose full-resolution region takes more than a moment publishes a reduced render of the region first, labeled **Draft • refining**, and the full-resolution region replaces it; fast renders and regions still cached go straight to full resolution. A resize schedules a new render; superseded jobs are canceled and stale results cannot replace current edits ([architecture](architecture.md#preview-compute-backend)).

Sharpening (`develop/sharpening.rs`) is applied before resizing. New edits and Lightroom imports start at Lightroom Classic's raw defaults (Amount 40, Radius 1.0, Detail 25, Masking 0; every raw and DNG import in the catalogue sampled, process versions 2012 to 6) and render with an unsharp mask fitted to Camera Raw 18.7 on a synthetic chart (slanted edges, sine gratings of 0.04 to 0.3 cycles per pixel, texture, noisy flats; 45 renders). It is about 2.4 times the strength of RAWmakase's earlier operator (a luminance-only Gaussian unsharp mask, no longer rendered) on fine detail per Amount, shapes the high-pass as `d / (1 + (|d|/halo)³)` so strong edges get small halos, makes dark halos 0.57 of light ones, scales the strength with Detail (0.3× at 0, 2.1× at 100) and fades out detail below a Masking threshold. The Detail panel's reset gives the Lightroom defaults. Camera Raw's ringing at Detail above 50 and its gentler sharpening of noise in deep shadows are not modelled. Full-resolution region rendering includes the required sharpening halo, and its pixels match the full render. No separate Fit-only sharpening changes the result. Luminance noise reduction remains the existing simple edge-aware filter.

Color noise reduction (`develop/color_noise.rs`) starts at Lightroom's raw default of 25 (Detail and Smoothness 50) and follows Camera Raw 18.7, fitted to the frequency response of 36 renders of synthetic noisy charts (Amount 10–100 at two noise levels, Detail and Smoothness 0–100). The chroma of the camera image, in square-root values, is split into a Laplacian pyramid: the finest level keeps a share of its colour noise that Detail sets (a quarter at 50, four fifths at 100), coarser levels are thresholded by Amount, Detail and Smoothness, and detail that stands out from a level's threshold (colour edges) is kept. It runs once on the camera image before everything else, as spot removal does, adding well under a second to a 24 MP export. Over the 36 cases the share of a flat's colour noise kept is within a mean 0.04 of Camera Raw's (Amount 25: 62% against 60%). Camera Raw takes out less colour noise on very noisy images (at twice the noise, Amount 50 keeps 67% against our 55%) and reduces single-pixel colour lines by about a quarter where this keeps them. The Detail panel's reset gives Lightroom's default.

## Tone and highlights

Profile-based recipes also store a camera exposure baseline independently of the Exposure slider. The verified X100F DR100 reference baseline is +0.15 EV; unverified camera/DR combinations use zero. Point curves use a natural cubic spline and the sRGB transfer function in ProPhoto primaries, with a hue-preserving master pass followed by independent RGB curves. Clipped input endpoints and 0–255 point editing are supported. Contrast preserves endpoints. See the comparison report for reference-derived metadata and limits.

Every resolved recipe has a camera profile (without an imported or embedded one, the camera matrix with the DNG default tone curve) and uses the profile tone curve; the scene-luminance shoulder of earlier engines is not applied on top of it, which would compress tones twice. Shadows and Highlights are a measured local operator ([tone controls](tone-controls.md#shadows-and-highlights)).

Highlight reconstruction estimates clipped camera channels from nearby unclipped channel ratios, before color conversion. It retains unclipped channels and blends in recovery near the sensor ceiling. With no usable color evidence, it falls back to neutral; it cannot recreate fully clipped texture. The recovered image is lazily cached per decoded image. Extreme WB changes after demosaicing still have limitations.

Master point curves still have a saturated-color residual. The parametric curve is measured ([tone controls](tone-controls.md#parametric-curve)). Generated-ramp regression data checks neutral master and independent RGB curves separately from RAW/profile differences; see `tests/data/README.md`. Exact Lightroom rendering parity is **not** established.

## Color mixer and grading

The color mixer, Saturation, Vibrance, color grading and Point Color render Camera Raw 18.7's measured responses: see [color mixer and grading](color-mixer.md). Legacy split-tone XMP imports restore blending 100 and clear modern grading controls. The earlier approximations, and the `reference_color` choice between them, are no longer rendered; the [color validation table](macos-lightroom-validation.md) records how they compared.

## Camera calibration

Calibration renders neutral-preserving primary Hue/Saturation matrices plus an asymmetric green/magenta shadow tint. The Calibration panel groups Shadows, Red Primary, Green Primary and Blue Primary, using −100–100 values; its Process row shows the current version, the only one. Calibration does not modify the installed DCP.

The primary sliders render Camera Raw 18.7's measured changes (in `calibration.rs`), replacing the original coefficients, measured on X100F/Adobe Standard. Camera Raw rendered a dense synthetic chart (1,728 colors and a gray ramp) with each primary's Hue and Saturation at −100, −50, +50 and +100. For each position, the six numbers of a neutral-preserving matrix at RAWmakase's calibration step were fitted by rendering the same chart through RAWmakase's own pipeline (Gauss–Newton on CIELAB differences), and positions in between are interpolated. Camera Raw leaves grays alone under every calibration slider, as the matrices do. On the fitting chart the 24 renders went from mean ΔE00 1.0–3.4 to 0.66–0.80, against 0.80 for the default render there; on the corpus chart the `calibration-*` cases went from 1.11–2.02 to 0.62–0.68 (default 0.66), and calibration combined with the color mixer from 1.55 to 0.66. Several sliders add their changes, which was checked only on that pair. Shadow Tint keeps its earlier fit (0.71 and 0.91 on the chart). The chart is one synthetic camera with its embedded profile; other cameras and profiles are not measured.

## Camera profiles

DCP ColorMatrix1/2 now support DNG temperature/tint and camera-neutral inversion. Camera calibration is signature-matched; missing color matrices fall back to the original WB approximation. Profile matrices, signatures and rendering parameters are embedded in saved recipes.

RAWmakase supports a bounded DCP subset: three-channel forward matrices, one or two standard illuminants, reciprocal-temperature interpolation, interpolated HSV calibration tables, linear/sRGB-indexed look tables, profile tone curves and baseline exposure offsets. The profile curve processes the low/high channels and interpolates the middle channel to preserve hue. DCPs without an embedded curve use the Adobe DNG SDK default curve. The profile tone stage clips to the SDR domain. This is an independent implementation, not exact Adobe processing-order parity.

No Adobe profiles are bundled. New edits prefer a matching imported Adobe Color, then Adobe Standard profile, then a DNG's embedded profile; without those, every camera uses RAWmakase Color, our own look over its LibRaw matrix (see [RAWmakase profiles](lightroom-profiles.md#rawmakase-profiles)). Published third-party DCPs, such as RawTherapee's, can be imported like any other profile. Profiles for another model are rejected, never substituted.

The **Profile** selector lists compatible DCPs in the local `camera-profiles` library. **Import profile…** loads a matching DCP file. Profile data and copyright are embedded into JSON recipes, so moving the profile file doesn't change an existing edit. Histories share immutable profile data. Matrix-only DCPs, HDR/triple-illuminant profiles and profiles forbidding embedding are rejected with an explanation. Compatible enhanced XMP look profiles can be explicitly imported together with their base DCP; see [supported look features and comparisons](lightroom-profiles.md). Profiles alone cannot reproduce Lightroom's demosaicing, local tone operators or detail algorithms.

Specification used: Adobe Digital Negative Specification 1.7.1.0, camera profile tags and chapter 6:
https://helpx.adobe.com/content/dam/help/en/photoshop/pdf/DNG_Spec_1_7_1_0.pdf

## Reference comparison

Export the same RAW from Lightroom at full resolution in sRGB, with matching orientation, WB, exposure, crop, and lens corrections. Use 16-bit TIFF for the least quantization loss. Then run:

```
rawmakase compare source.ARW lightroom.tiff /tmp/sony-comparison --origin 2400 1200
```

The output directory must not already exist. `--recipe preset.json` optionally supplies RAWmakase edits; without it the current engine defaults are used, independently of sidecars. The tool writes unscaled 512-pixel crops, a 4× difference visualization, the exact recipe, and full-image sRGB MAE/RMSE/PSNR. These metrics measure differences, not quality, and do not compensate for exposure or registration. A reference with different dimensions is rejected rather than stretched.

Five X100F Lightroom comparisons across three photographs were checked on macOS, including custom white balance, master/RGB point curves and nonzero tone sliders. The corrected profile tone substantially reduced preview error, but full Lightroom parity is not established. See [macOS and Lightroom validation](macos-lightroom-validation.md) for measurements and limitations. The supplied private archives installed 62 matching camera profiles and 603 Settings presets in the user data directory; none are bundled. See [XMP support](xmp-presets.md).

CLI profile selection: `rawmakase render source.RAF output.jpg --profile /path/to/profile.dcp`. Profile libraries are read from `$RAWMAKASE_DATA_DIR/camera-profiles` and the platform RAWmakase data directory (`~/Library/Application Support/RAWmakase` on macOS, XDG on Linux); camera matching is mandatory.
