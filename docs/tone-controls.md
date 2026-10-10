# Basic panel tone controls

Exposure, Whites, Blacks, Highlights, Shadows, Clarity, Texture and Dehaze render on scene values, before the camera profile's look and tone curve, where Camera Raw 18.7 applies them. Their operators are fitted on linear-profile Camera Raw renders of probes, synthetic scenes and training photos; the [scene tone stage](scene-tone-stage.md) describes the stage, what it measures from each photo and the gates a change to it must pass. Contrast, the parametric and point curves and Levels render after the profile curve, as below.

## Measurement

References are Camera Raw 18.6 renders from Photoshop 2026, the same engine and process version (2012, PV 15.4) as Lightroom Classic 15.5. `scripts/camera-raw-sweep.py` writes a Photoshop script that renders each slider at several positions with Adobe Standard, no lens profile and As Shot white balance. `scripts/lightroom-scorecard.py` re-renders every reference in RAWmakase from the settings embedded in it and reports the error. The 2026-09-26 set is three Fujifilm X100F and two Sony A7 II photos, 8 sliders × 6 positions, 2000 px. It is private and not in the repository.

Each adjusted render was compared with the default render of the same photo. For Contrast, Blacks and Whites, a single curve applied to the brightest and darkest channel in ProPhoto primaries with the sRGB transfer function (DNG RGBTone) explains the change to within 0.0015 MAE. That is the 8-bit quantization level of the comparison. Contrast pivots at a photo-dependent point (0.41–0.51). Highlights, Shadows, Clarity and Dehaze leave spatial residuals and are local operators. Only Contrast still renders from these after-the-profile measurements; the others are measured on scene values ([scene tone stage](scene-tone-stage.md#execution-table)).

## Black point

Camera Raw's default render has a black point with a toe below about 0.02 (scene-linear), which does not follow the camera's baseline exposure. It is part of the scene tone stage's measured curves ([what the default render does](scene-tone-stage.md#what-the-default-render-does-in-s3)); a profile whose DefaultBlackRender is None renders without it. The earlier DNG SDK exposure ramp, with its black at 0.0015 × 2^exposure fitted to three photos, scaled with the baseline and crushed the deep shadows of files with a large one. It is kept only as the basis Auto measures ([Auto](#auto)).

## Implementation

The scene tone stage is `crates/rawmakase-engine/src/develop/scene_tone/` (the white point, Whites and Blacks, Dehaze, and the photo's measures), with `local_tone.rs` (Shadows and Highlights) and `clarity.rs`; `texture.rs` runs before it, on the camera image. `basic_tone.rs` applies Contrast as a curve after the profile's look table and tone curve, before Levels and the user's curves, interpolated between the measured slider positions in `basic_tone_data.rs` with 0 as the identity.

### Contrast

Camera Raw's Contrast is one curve whose pivot (the level it leaves alone) depends on the photo, not on the user's Exposure, and it comes after Whites and Blacks:

- On the synthetic chart rendered at Exposure −1.5, −0.75, 0 and +0.75 the pivot stays at 0.56–0.57 (encoded), so it is measured on the photo before the user's Exposure.
- Rendered together, Blacks then Contrast and Whites then Contrast explain Camera Raw to 0.05–0.17/255 on the gray ramp; RAWmakase's earlier order, Contrast first, was 0.8–3.4/255 off. Whites comes before Blacks (0.02–0.09/255).
- The chart's Contrast curve, moved to another pivot by a power warp of gamma-2.2 encoded values, explains every photo's Camera Raw Contrast at all six measured amounts with one pivot per photo (0.002–0.01 MAE on 33 photos, block means). The pivots range from 0.36 to 0.63.
- The pivot follows the photo's default rendering: it rises with its mean encoded luminance and falls with the middle of its range (halfway between the 1st and 99th percentiles of 48-across block means): pivot = 0.577 + 0.568 · mean − 0.689 · middle. In leave-one-out tests on the 33 photos and the chart this predicts the pivot to 0.031, against a spread of 0.058 (`scripts/corpus/contrast-curve.py`).

RAWmakase renders this: the chart's measured Contrast (`CONTRAST_CHART`, Camera Raw 18.7) moved to the pivot measured on the photo's reduced copy, as its profile, white balance and calibration render it at the camera's exposure, after the scene tone stage (Whites and Blacks included) and the profile. Masks' Contrast uses the same pivot. Extra MAE over the default render on the private photos, compared with RAWmakase's own default render, falls from 0.0046 to 0.0033 on the five sweep photos (Contrast ±25, ±50, ±100) and from 0.0095 to 0.0066 on 14 sample photos (Contrast +50). On the chart, `contrast±*` cases went from 1.6–3.8 to 0.8–2.1 mean ΔE00 and Contrast with Blacks or Whites from 1.8–3.4 to 1.0–2.2 (measured before the scene tone stage).

### Whites and Blacks

Whites and Blacks are curves of the scene tone stage, measured on neutral ramps (`scene_tone/global.rs`, tables `white3.bin`, `white.bin`, `default_black.bin` and `blacks.bin`, written by `scripts/corpus/scene-tone-tables.py`). The default render's white point rolls off toward the sensor's white or twice the photo's maximum; Whites is one curve with it, applied DNG RGBTone fashion, and positive Whites stretches toward the photo's own maximum. Camera Raw's default black, which deepens as the photo's darkest level (its 0.1th percentile) rises toward the sensor's white, and then Blacks follow on each channel, over that level. A mask's Whites and Blacks are Camera Raw's separate curves after the global ones (`masks.bin`). See [Whites and Blacks](scene-tone-stage.md#whites-and-blacks).

### Shadows and Highlights

Both are luminance gains from an edge-aware base level, in the scene tone stage (`local_tone.rs`): a guided filter of log2 scene luminance (radius 3.2% of the long edge, ε = 0.5) on the photo's 512-pixel measurement copy, so tiles, 100% regions, previews and exports agree. The gain is a measured table (`local_tone_data.rs`) of the base level relative to a key: the photo's mean luminance for Shadows, halfway between the 1st and 75th percentiles of log luminance for Highlights. The tables are fitted on Camera Raw's scene-value renders, Shadows on the training photos and Highlights on synthetic scenes (`scripts/corpus/scene-tone-local.py`). See [local operators](scene-tone-stage.md#local-operators).

### Dehaze

Positive Dehaze removes the photo's haze in the scene tone stage (`scene_tone/haze.rs`): a dark-channel model measured on the photo's 512-pixel measurement copy at Exposure 0 (the airlight from its haziest 1%, and a guided-filtered dark channel relative to it), kept with the photo's measures, so Dehaze edits do not measure it again. Each pixel moves away from the airlight by its transmission, 1 − ω·density, where ω grows with the amount (0.3 at +20, 0.45 at +40, 0.9 at +100). Fitted on the 22 training photos against Camera Raw 18.7, it halves the median display ΔE00 at +100 (5.97 → 3.06). Negative Dehaze works on each channel, by its level relative to the photo's 99th percentile of luminance, with a measured log2 gain per amount (`DEHAZE` in `local_tone_data.rs`). A mask's Dehaze adds to the slider; the sum picks the model. See [Dehaze](scene-tone-stage.md#dehaze).

### Clarity

Clarity, of both signs, is a log2 gain of detail at four scales in the scene tone stage (`clarity.rs`): on the measurement copy's grid, the log scene luminance minus an edge-aware blur (bilateral, range σ 2 EV, spatial σ 0.4%, 1.5%, 5% and 15% of the long edge), each scale weighted by the local base level relative to the photo's brightest levels (0.25·p99.9 + 0.75·p99 of log luminance). The weights are least-squares fits to Camera Raw 18.7's scene-value renders of synthetic scenes at −100, −50, +50 and +100; values in between are interpolated, and below ±50 the ±50 fit is scaled. A mask's Clarity reads the same detail at the mask's amount plus the global one.

### Texture

Camera Raw 18.7's Texture, measured on synthetic charts (sine gratings of 0.004 to 0.25 cycles per pixel at ±0.1 to ±2 EV, large flats and edges, Texture −100 to +100), is a local contrast of log values over scales of a few to about thirty pixels of the full-resolution photo: an image twice the size renders the same per pixel. It leaves large flats alone, boosts faint detail most (×1.78 at ±0.1 EV and +100) and strong contrast hardly at all (×1.05 at ±2 EV), and works on each colour channel, so colour edges gain chroma (up to +19 beside a saturated red).

RAWmakase renders Texture with that model (`crates/rawmakase-engine/src/develop/texture.rs`): a six-level Laplacian pyramid of the log of each camera channel, each level compressed where it is strong and weighted, scaled by a strength that Texture sets, made once per render resolution and cached by amount. It is fitted to the gratings within 0.04 RMS (gain ratio) and to the edges' halos within 2.4% of the step in one dimension; on the chart's 2D edges the halo right at the edge is still about 1.5 times Camera Raw's. On the `texture±50/±100` corpus cases the mean ΔE00 to Camera Raw dropped from 1.46–2.16 to 1.10–1.57 (the default render was 0.66). It runs on the camera image before the scene tone stage; the textured image is made on the CPU and cached. A mask's Texture scales the same detail by its own strength on top of the global slider's, and previews with a mask's Texture or Clarity develop on the CPU.

## Parametric curve

The Tone Curve's region sliders (Shadows, Darks, Lights, Highlights) and their three splits were measured with Camera Raw 18.7 on the synthetic chart: about 380 settings, each region at ±25, ±50, ±75 and ±100 with moved splits, and Darks with Lights together (`scripts/corpus/parametric-curve.py`). What the renders show:

- It is one curve, applied DNG RGBTone fashion in ProPhoto RGB (brightest and darkest channel curved, the middle one keeping its place), between the Basic panel's tone and the point curve. A curve read from the gray ramp predicts the chart's colors to mean ΔE00 0.06–0.34; applied to each channel instead, as RAWmakase did, 0.2–0.8.
- The curve is smooth in gamma-2.2 encoded ProPhoto RGB: a cubic spline with knots at the splits fits every single-region render to 0.05/255 there, and to 0.4/255 in the sRGB encoding the point curves use.
- Shadows acts only below the midtone split and Highlights only above it; Shadows depends on the shadow and midtone splits, Highlights on the midtone and highlight splits, and both scale with the span they act on. Darks and Lights act over the whole range and depend on the midtone split only.
- Shadows then Darks, and Highlights then Lights, compose exactly. Darks and Lights together are not the sum of each (up to 8/255 off at ±100), so they were measured together.
- Negative and positive settings are not mirror images: Shadows −50 takes the bottom of the curve to black, while +50 lifts it by about 0.7 EV.

RAWmakase renders it from these measurements (`crates/rawmakase-engine/src/develop/parametric.rs`, tables in `parametric.bin`): Shadows over 0 to the midtone split and Highlights above it, per amount and split ratio, then Darks and Lights, per amount and midtone split (together, on a grid at the default splits, with a moved midtone split changing each as it does alone). Values between the measured ones are interpolated linearly. On all the fitting renders, the curve applied to Camera Raw's own default render is within mean ΔE00 0.29 of Camera Raw (worst 3.3, Lights +100 at a midtone split of 20); the RAWmakase chart cases are in [parity gaps](parity-gaps.md#tone).

Every recipe renders with the measured curve; the earlier approximation, applied to each channel, is no longer rendered. A look's own parametric curve (at its Profile Amount, with its own splits) is a second curve after the user's, as in Camera Raw: on the chart, Darks +30 over a look with Darks −15, Lights −20 and Highlights −33 (`look-parametric-darks+30`) goes from 1.41 to 0.93 mean ΔE00 against adding the look's regions to the user's. At Amount 200 a look's curve does about 1.6 times what it does at 100, not twice; RAWmakase still scales it by the Amount.

## Refine Saturation

Lightroom's Refine Saturation (`crs:CurveRefineSaturation`, default 100) sets how much of the saturation change a master point curve makes is kept. It was measured on the synthetic chart with Camera Raw 18.7, through an S curve, a strong S, a lift and a fade, at 0, 50, 100, 150 and 200:

- It changes only the master point curve. Channel curves, the parametric curve and Contrast render the same at every value.
- At 0 a colour keeps its channel differences from before the curve (in encoded ProPhoto RGB, where the curve runs) and takes its luma (Rec. 601 weights) from the curved colour. Where that would leave 0–1, the differences are scaled down just enough to fit.
- Other values blend linearly between 0 and 100 (exact to 0.0002 at 50). 150 and 200 render exactly as 100.

`curve::refine_saturation` implements it after the master curve on the CPU and the GPU. It is imported and written back with the edit. Tone Curve's point mode has a Refine › Saturation slider (0–100) under the RGB curve; an imported value above 100 shows as 100 and is kept until the slider is moved.

## Point Curve menu

Under the point curve, the **Point Curve** menu lists Lightroom's Linear, Medium Contrast and Strong Contrast, then the curves saved here, then **Save…**. The menu shows the name of the curve the photo has, or Custom.

- The built-in curves set the RGB curve only and leave the Red, Green and Blue curves as they are. Their points (0–255) are those Lightroom Classic 15 stores in its catalog for edits naming each curve: Medium Contrast 0,0 · 32,22 · 64,56 · 128,128 · 192,196 · 255,255 and Strong Contrast 0,0 · 32,16 · 64,50 · 128,128 · 192,202 · 255,255. The Curve presets in the Presets panel use the same points. Whether Lightroom's menu also resets the colour curves was not checked.
- **Save…** asks for a name and saves the RGB, Red, Green and Blue curves as an XMP file in the data directory's `curves/` folder, laid out as the curve files Lightroom and Camera Raw save in their Curves folder (so either can read them). Choosing a saved curve sets all four curves; a file with only the RGB curve sets the colour curves linear. A name already saved is refused rather than replaced. RAWmakase does not read Camera Raw's own Curves folder. A curve file holds points only, which Lightroom and RAWmakase both read as natural cubic curves, so a curve saved from an edit that predates Reference tone curves (with the older interpolation) loads with Lightroom's.
- Choosing a curve is one History step ("Point Curve", with the curve's name) and turns the Tone Curve panel on, even when the photo already had that curve; choosing the curve it has with the panel on changes nothing.

## Targeted adjustment

The Tone Curve's target button (top left of the panel) or Cmd+Option+Shift+T opens Lightroom's Targeted Adjustment Tool for the parametric curve, and shows the parametric view. A drag up or down on the photo moves the region (Shadows, Darks, Lights or Highlights) that the pixel's curve input falls in, between the split points, by 100 per 250 points of travel. The curve and the region's slider are highlighted while it moves. The input is sampled where the parametric curve sees it: a 5×5 average of the photo as shown (masks included), after Contrast and Levels, as the Rec. 601 luma of the three channels the curve runs on. Each drag is one History step ("Region Lights +15"); see [the color mixer](color-mixer.md#targeted-adjustment) for how a drag works. In point mode, Lightroom's tool adds and drags a point on the curve; here it always moves the parametric regions.

## Auto

The Basic panel's **Auto** (the button at the top of the Basic panel, above Treatment, or Cmd/Ctrl+Shift+U) sets the six Tone sliders, Vibrance and Saturation, and keeps white balance, including a manual one, as Lightroom's does; **Auto** in the WB menu sets white balance alone, and the menu shows Auto while the photo keeps that result. `rawmakase render --auto` applies Auto tone from the command line (it sets Exposure, so it does not combine with `--exposure`), and `--auto-wb` applies Auto white balance first. The implementation is `crates/rawmakase-engine/src/develop/auto.rs`; it keeps every other setting, and the app runs it off the UI thread and records one History step. Like Lightroom's, Auto tone measures the photo before its adjustments: as the profile, white balance, calibration, lens corrections and crop render it, without the tone sliders, curves and Levels, presence, color mixer, B&W, grading, detail, effects, spots and masks. So a film-look curve that lifts the blacks, or a color edit, does not change what Auto chooses. In a Lightroom catalog's history, photos whose point curve lifted black to 30/255 or more still got ordinary Auto Blacks (median −10, as against −17 without such a curve), which measuring through the curve could not give. Like Lightroom's, it also sets Vibrance and Saturation (below). Auto is greyed out (and its shortcut does nothing) while running it again would change nothing: the six Tone sliders, Vibrance and Saturation are as Auto set them and nothing Auto measures (profile, white balance, calibration, lens corrections, crop) has changed. Moving a Tone slider, Vibrance or Saturation, changing one of those, undoing Auto or opening another photo turns it back on; adjustments Auto ignores, such as a curve, Clarity or the color mixer, leave it greyed out.

- **White balance** follows Lightroom's Auto as measured: gray world (the average of the camera pixels in the crop made neutral; pixels near clipping or in the noise floor are ignored), then 23 mired warmer and 3 Tint greener, limited to 2850–7500 K and Tint 0 to +30, the range Lightroom's Auto keeps to. The camera pixels are the decoded ones, before highlight recovery invents colour. XMP presets and settings with `WhiteBalance="Auto"` and no resolved Temperature/Tint use the same estimate. On 133 photos from three cameras with Lightroom's or Camera Raw 18.6's Auto values (A7 II from a Lightroom catalog, A7CR and X100F from Camera Raw, all with Adobe Standard), the estimate is within a median of 2.7 mired of Adobe's (90% within 5.5, worst 27, on an A7CR neon night scene) and 1 Tint; the earlier near-neutral search was 24 mired off (worst 183) and averaged 20 mired cooler. The tuning photos are private.
- **Tone** is predicted from one render of the photo before its adjustments, reduced so the crop's long edge is about 1024 px, with every tone slider at 0. Lightroom's Auto behaves like a learned estimate rather than a target it solves for: it lifts a dark photo only part of the way to middle gray, holds Exposure back for bright highlights, and nearly always pulls Highlights down (median −65) and opens Shadows (+47). So each slider is a linear fit, to Lightroom Classic's own Auto values, of the one or two display-encoded percentiles of luminance (L), of the brightest channel (P) or of chroma (C, a pixel's channel spread relative to its brightest channel, 0 for gray and pixels darker than 2%) that predicted it best on held-out photos:

  | Slider | Fit |
  |---|---|
  | Exposure | 2.22 − 2.13 L40 − 1.52 L99 (EV) |
  | Contrast | +6 |
  | Highlights | −36 − 51.9 L90 + 26.9 P25 |
  | Shadows | 54.4 − 41.5 P10 |
  | Whites | 67.5 − 52.2 P99.8 |
  | Blacks | −38 − 1.86 log2(linear L1) |
  | Vibrance | +15 |
  | Saturation | 2.07 − 10.2 C35 |

  Lightroom changed its Auto in mid-2019, and the catalog's history shows both versions. The earlier one gave Contrast about −17, Saturation +4 and Vibrance +17 under process version 10; the current one gives every photo Contrast +5 to +7 (so a constant +6 is within 1 of it), Vibrance +15 (+14 or +15 for four photos in five) and a Saturation mostly from −4 to +3 that follows how colourful the photo's duller part is: a gray photo gets +2, a colourful one less or a negative value. It also sets Blacks about 5 lower and Shadows about 3 higher. The sliders' dependence on the photo is the same in both, so the slopes are fitted on every photo and the levels, Contrast, Vibrance and Saturation on the current Auto alone. Lightroom's Saturation is overwritten by its Auto, so Auto here sets it too.

  The fit uses 452 Auto Settings steps on 439 photos from a Lightroom Classic catalog (mostly two cameras, process versions 10 and 11; 124 photos are from the current Auto), comparing each step's result with Auto run on the settings just before it. Lightroom records Auto's values in the step after it, so steps followed by a reset, paste or sync are left out, as is any slider the next step changed; after a preset, only the sliders that preset is never seen to change in the catalog are used. Every fifth photo was held out of the fit. Absolute error on the held-out photos from the current Auto (18 to 25 per slider), median and 90th percentile, before (the fit that treated both versions as one, which left Saturation as it was) and after; Auto run on a photo gives exactly these values:

  | Slider | Before median | Before p90 | After median | After p90 |
  |---|---|---|---|---|
  | Exposure | 0.21 EV | 0.46 EV | 0.21 EV | 0.46 EV |
  | Contrast | 15.5 | 16 | 1 | 1 |
  | Highlights | 6 | 14.6 | 7 | 15.8 |
  | Shadows | 8 | 25.3 | 6.5 | 23.3 |
  | Whites | 8 | 29.8 | 8 | 29.8 |
  | Blacks | 2.5 | 11 | 4 | 8 |
  | Vibrance | 0 | 5 | 0 | 5 |
  | Saturation | 2 | 3.4 | 1 | 2.4 |

  The averaged error falls from 15 to 0.7 on Contrast, 10.3 to 9.3 on Shadows, 7.7 to 7.2 on Highlights and 2.1 to 1.2 on Saturation, and the bias of Blacks (+3.4) and Shadows (−8.9, −2.2 on the fitted photos) goes. On the 60 to 99 current-Auto photos it was fitted to, the medians and 90th percentiles are 0.21 and 0.55 EV, 1 and 1, 5 and 14.5, 7 and 18, 10 and 33, 4 and 11, 0 and 2, 1 and 3, so the fit carries over to photos it has not seen. Whites is predicted only somewhat. Fits of three percentiles, or of colourfulness, gained at most 0.02 EV on Exposure and 1 on Whites and Vibrance in cross-validation on the current Auto's photos, too little to trust on this few. The earlier Auto's photos are now further off on Contrast (median 24) and Saturation, as intended. The tuning photos are private.

A photo takes one render of the reduced copy, after highlight recovery and the reduction itself. That render keeps the basis the fits were measured on: the DNG exposure ramp ([black point](#black-point)) in place of the scene tone stage (`PixelOutput::AutoBasis`), so Auto gives the same values as before the stage.

## Remaining

- How Camera Raw adapts Shadows, Highlights, Dehaze and positive Whites to each photo is fitted (keys from the photo's statistics, tables over its measures), not known; see [scene tone stage](scene-tone-stage.md#local-operators) for what the keys leave.
- Negative Dehaze's spatial part, and Clarity's residual, are not modelled.
- Contrast's pivot is predicted from two statistics of the photo to about 0.03; what Camera Raw measures exactly is unknown.
- Auto's Whites: Lightroom's choice follows the brightest percentiles only loosely (90th percentile error about 30).
