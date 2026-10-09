# Basic panel tone controls

## Measurement

References are Camera Raw 18.6 renders from Photoshop 2026, the same engine and process version (2012, PV 15.4) as Lightroom Classic 15.5. `scripts/camera-raw-sweep.py` writes a Photoshop script that renders each slider at several positions with Adobe Standard, no lens profile and As Shot white balance. `scripts/lightroom-scorecard.py` re-renders every reference in RAWmakase from the settings embedded in it and reports the error. The 2026-09-26 set is three Fujifilm X100F and two Sony A7 II photos, 8 sliders × 6 positions, 2000 px. It is private and not in the repository.

Each adjusted render was compared with the default render of the same photo. For Contrast, Blacks and Whites, a single curve applied to the brightest and darkest channel in ProPhoto primaries with the sRGB transfer function (DNG RGBTone) explains the change to within 0.0015 MAE. That is the 8-bit quantization level of the comparison, so these sliders are global tone curves in that domain. The same curve fits every photo for Blacks and negative Whites. Contrast pivots at a photo-dependent point (0.41–0.51), and positive Whites stretches further on photos whose highlights are dim. Highlights, Shadows, Clarity and Dehaze leave spatial residuals and are local operators.

## Black point

Adobe's rendering subtracts a small black level before the tone curve: the DNG SDK exposure ramp, whose default Shadows setting maps to a black point with a quadratic toe. Without it, RAWmakase's deep shadows (display luminance 0.02–0.08) were 0.4–0.55 EV brighter than Camera Raw on every camera. RAWmakase applies the ramp after exposure, with the black at 0.0015 × 2^exposure in scene-linear units. That value was fitted to the Camera Raw defaults of three photos; the SDK's nominal 0.005 crushes shadows by about 1.2 EV here. Scorecards: X100F Lightroom references 0.0120 → 0.0096 MAE, Sony Camera Raw references 0.0088 → 0.0065.

## Implementation

`crates/rawmakase-engine/src/develop/basic_tone.rs` applies Whites → Blacks → Contrast as one composed curve after the camera profile's tone curve and before the user's point curve. The curves are the measured averages in `basic_tone_data.rs`, interpolated between slider positions with 0 as the identity. They replace the earlier power-S contrast and luminance-weighted Whites/Blacks, which are no longer rendered.

Extra MAE over the default render, averaged across the five photos (previous operators in parentheses):

| Slider | −100 | −50 | −25 | +25 | +50 | +100 |
|---|---:|---:|---:|---:|---:|---:|
| Contrast | −0.0009 | −0.0004 (+0.0107) | −0.0001 | +0.0001 | +0.0002 (+0.0061) | +0.0002 |
| Blacks | −0.0008 | +0.0007 (+0.0189) | +0.0007 | −0.0005 | −0.0004 (+0.0009) | +0.0005 |
| Whites | −0.0011 | −0.0010 (+0.0031) | −0.0005 | +0.0019 | +0.0091 (+0.0152) | +0.0577 |

### Contrast

Camera Raw's Contrast is one curve whose pivot (the level it leaves alone) depends on the photo, not on the user's Exposure, and it comes after Whites and Blacks:

- On the synthetic chart rendered at Exposure −1.5, −0.75, 0 and +0.75 the pivot stays at 0.56–0.57 (encoded), so it is measured on the photo before the user's Exposure.
- Rendered together, Blacks then Contrast and Whites then Contrast explain Camera Raw to 0.05–0.17/255 on the gray ramp; RAWmakase's earlier order, Contrast first, was 0.8–3.4/255 off. Whites comes before Blacks (0.02–0.09/255).
- The chart's Contrast curve, moved to another pivot by a power warp of gamma-2.2 encoded values, explains every photo's Camera Raw Contrast at all six measured amounts with one pivot per photo (0.002–0.01 MAE on 33 photos, block means). The pivots range from 0.36 to 0.63.
- The pivot follows the photo's default rendering: it rises with its mean encoded luminance and falls with the middle of its range (halfway between the 1st and 99th percentiles of 48-across block means): pivot = 0.577 + 0.568 · mean − 0.689 · middle. In leave-one-out tests on the 33 photos and the chart this predicts the pivot to 0.031, against a spread of 0.058 (`scripts/corpus/contrast-curve.py`).

RAWmakase renders this: the chart's measured Contrast (`CONTRAST_CHART`, Camera Raw 18.7) moved to the pivot measured on the photo's reduced copy, as its profile, white balance and calibration render it at the camera's exposure, after Whites and Blacks. Masks' Contrast uses the same pivot. Extra MAE over the default render on the private photos, compared with RAWmakase's own default render, falls from 0.0046 to 0.0033 on the five sweep photos (Contrast ±25, ±50, ±100) and from 0.0095 to 0.0066 on 14 sample photos (Contrast +50). On the chart, `contrast±*` cases go from 1.6–3.8 to 0.8–2.1 mean ΔE00 and Contrast with Blacks or Whites from 1.8–3.4 to 1.0–2.2.

### Whites

Positive Whites follows the photo's highlights in Camera Raw, after its Exposure: on the synthetic chart rendered at Exposure −2 to +1 (13 steps), Whites +100 stretches the highlights to white from about 0.65 (encoded) at −2 EV and 0.9 at 0 EV, and leaves them nearly alone at +1 EV. The chart's curves at these exposures form one family:

- One member of it explains each of the five sweep photos' Camera Raw Whites at +25, +50 and +100 together (0.0003–0.0045 MAE; the median curve was 0.0004–0.21).
- Which member follows the 98th percentile of the photo's encoded luminance: the chart exposure with the same highlights, plus 0.24 EV (photos behave a little brighter than the chart), predicts the best member to 0.15 EV (`scripts/corpus/whites-curve.py`).

RAWmakase renders this: the curve for the photo's highlights, measured on its reduced copy as the recipe renders it before the Basic tone sliders (Exposure included), interpolated between the chart's exposures. Negative Whites, which is the same on every photo, keeps the median curve, and so do masks' Whites with the photo's table. Extra MAE over the default render on the five photos (Whites +25, +50, +100) falls from 0.024 to 0.0068. On the chart, Whites with Exposure −1.5 or +0.75 goes from 4.1–16.3 to 1.0–3.1 mean ΔE00, while the chart at its own exposure moves from 0.91–3.12 to 0.98–3.06 (+50: 1.37 to 1.52): photos and the chart differ by the 0.24 EV above, and the offset follows the photos.

### Shadows and Highlights

Offline fits on the sweeps show both are local operators whose effect is best explained in log luminance of the toned image. The base level is a guided filter (radius 3.2% of the long edge, ε = 1.5 in log2 units squared), with the gain measured as a function of that base level relative to an image key. The key is the 99th luminance percentile for Shadows and the median for Highlights. `crates/rawmakase-engine/src/develop/local_tone.rs` computes the base level on a 512 px copy of the photo, so tiles, 100% regions and previews agree, and applies the measured tables in `local_tone_data.rs` after the profile tone curve.

| Slider | −100 | −60 | −30 | +30 | +60 | +100 |
|---|---:|---:|---:|---:|---:|---:|
| Shadows | +0.0025 | +0.0014 (+0.0343) | +0.0007 | +0.0006 | +0.0037 (+0.0389) | +0.0102 |
| Highlights | +0.0060 | +0.0025 (+0.0014) | +0.0000 | +0.0014 | +0.0032 (+0.0054) | — |

### Dehaze

Dehaze is mostly a per-photo tone curve with a spatial residual. A single curve per photo explains ±40 to 0.011–0.019 MAE (from 0.04–0.11 unchanged). RAWmakase applies the curve averaged across photos, before Contrast in the same composed curve. Extra MAE over the default render: +0.0078 / +0.0135 at +40 / −40 (previously +0.037 / +0.062), +0.0021 / +0.0045 at ±20, and +0.036 / +0.044 at ±100, where the per-photo adaptation dominates.

### Clarity

Camera Raw's positive Clarity is a local contrast operator whose strength depends on the scale of the detail and on how bright its surroundings are relative to the photo's highlights. Measured on synthetic gray scenes, it stops at edges: both sides of a step change keep their own level up to a few pixels from the edge, and a uniform gray only darkens by about 0.1 EV at +100. Its effect scales with the photo's size, so a 3000-pixel copy of a scene responds like a 1500-pixel one (correlation 0.92).

RAWmakase renders positive Clarity with that model (`crates/rawmakase-engine/src/develop/clarity.rs`). On the Shadows/Highlights map's 512-pixel copy of the toned photo, it takes the detail at four scales, the log luminance minus an edge-aware blur (bilateral, range σ 2 EV, spatial σ 0.4%, 1.5%, 5% and 15% of the long edge). Each scale gets a weight that depends on the local base level (`local_tone.rs`) relative to the photo's 99th luminance percentile. The resulting log2 gain is applied per pixel with Shadows and Highlights. The weights are least-squares fits to the log2 change Camera Raw 18.7 renders at Clarity +50 and +100 on 104 synthetic scenes: 80 random photo-like scenes plus edges, disks, ramps, gratings and textures. Values in between are interpolated, and below +50 the +50 fit is scaled. No photo was used to fit it.

Checked against Camera Raw on the five photos above, Clarity +25 / +50 / +100 change the block averages to within 0.0080 / 0.0156 / 0.0301 of Camera Raw's change, against 0.0118 / 0.0231 / 0.0453 with the earlier operator (doing nothing scores 0.0118 / 0.0232 / 0.0456). Every photo is closer at every amount. Negative Clarity and a mask's Clarity keep the earlier operator (the local detail gain in `quality/local.rs`).

### Texture

Camera Raw 18.7's Texture, measured on synthetic charts (sine gratings of 0.004 to 0.25 cycles per pixel at ±0.1 to ±2 EV, large flats and edges, Texture −100 to +100), is a local contrast of log values over scales of a few to about thirty pixels of the full-resolution photo: an image twice the size renders the same per pixel. It leaves large flats alone, boosts faint detail most (×1.78 at ±0.1 EV and +100) and strong contrast hardly at all (×1.05 at ±2 EV), and works on each colour channel, so colour edges gain chroma (up to +19 beside a saturated red).

RAWmakase renders Texture with that model (`crates/rawmakase-engine/src/develop/texture.rs`): a six-level Laplacian pyramid of the log of each camera channel, each level compressed where it is strong and weighted, scaled by a strength that Texture sets, made once per render resolution and cached by amount. It is fitted to the gratings within 0.04 RMS (gain ratio) and to the edges' halos within 2.4% of the step in one dimension; on the chart's 2D edges the halo right at the edge is still about 1.5 times Camera Raw's. On the `texture±50/±100` corpus cases the mean ΔE00 to Camera Raw drops from 1.46–2.16 to 1.10–1.57 (the default render is 0.66). A mask's Texture keeps the original 3-pixel operator, and previews with the measured Texture develop on the CPU.

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

The Tone Curve's target button (top left of the panel) or Cmd+Option+Shift+T opens Lightroom's Targeted Adjustment Tool for the parametric curve, and shows the parametric view. A drag up or down on the photo moves the region (Shadows, Darks, Lights or Highlights) that the pixel's curve input falls in, between the split points, by 100 per 250 points of travel. The curve and the region's slider are highlighted while it moves. The input is sampled where the parametric curve sees it: a 5×5 average of the photo as shown (masks included), after the basic tone curves and Levels, as the Rec. 601 luma of the three channels the curve runs on. Each drag is one History step ("Region Lights +15"); see [the color mixer](color-mixer.md#targeted-adjustment) for how a drag works. In point mode, Lightroom's tool adds and drags a point on the curve; here it always moves the parametric regions.

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

A photo takes one render of the reduced copy, after highlight recovery and the reduction itself.

## Remaining

- Positive Whites' offset between photos and the chart is fitted on five photos; Lightroom's own measure of the highlights is unknown.
- Contrast's pivot is predicted from two statistics of the photo to about 0.03; what Camera Raw measures exactly is unknown.
- Auto's Whites: Lightroom's choice follows the brightest percentiles only loosely (90th percentile error about 30).
- Positive Clarity at +100 is still 0.030 from Camera Raw on block averages (was 0.045). Negative Clarity and a mask's Clarity and Texture still use the earlier operators. Dehaze at ±100 needs its per-photo adaptation (airlight estimate) and spatial component.
