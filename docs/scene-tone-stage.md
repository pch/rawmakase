# Scene tone stage

RAWmakase renders the Basic panel's tone controls where Camera Raw does: on scene
values, before the camera profile's look table and tone curve. This page is the
contract for that stage: how it is compared with Camera Raw, what the survey found
about Camera Raw's stage order, what the stage measures from each photo, and the
gates a change to it must pass. Tracking issue: #374 (from #354).

## Pipeline

| # | Stage | Domain |
|---|---|---|
| S0 | Decode, demosaic, highlight recovery of sensor-clipped channels | camera RGB |
| S1 | White balance, the profile's matrix and HueSatMap, Camera Calibration | linear, sRGB primaries |
| S2 | Exposure and the DNG exposure ramp (black point) | linear Rec.2020 |
| S3 | **Scene tone**: Highlights, Shadows, Clarity, Dehaze, the white point and Whites, Blacks | linear ProPhoto, unclamped |
| S4 | Profile LookTable, enhanced look's HSV table, ProfileToneCurve, look curve | as before |
| S5 | Contrast, Levels, parametric and point curves, colour mixer, Point Color, RGB-table looks, grading, gamut, output | as before |

Each stage boundary has a `PixelOutput` tap, so tests can read a render between
stages through the same sampling, masks and caches as the display.

## Comparison contract

All Camera Raw comparisons go through one boundary: 16-bit ProPhoto RGB (gamma 1.8)
TIFFs from Photoshop 2026's Camera Raw (`scripts/corpus/camera_raw.py`, which opens
copies of the source with ProPhoto/16-bit open options), decoded to linear ProPhoto.

- **Stage probes** are synthetic DNGs (`scripts/corpus/probe_dng.py`): RGGB mosaics
  whose camera RGB is linear ProPhoto, with an embedded profile that maps it to XYZ
  D50 unchanged, As Shot white balance neutral, and either a linear ProfileToneCurve
  or a curved one. With the linear curve and no look, Camera Raw's ProPhoto output is
  its scene tone stage's output.
- **Photos at the scene stage**: `scripts/corpus/linear_profile.py` converts a RAW with
  Adobe DNG Converter and rewrites its embedded profile to a linear tone curve without
  a LookTable (keeping the camera's matrices, HueSatMap and baseline exposure), so a
  real photo can be read at the same boundary.
- **Final renders** (photo gates) compare Camera Raw's ProPhoto export, converted to
  sRGB by clipping (relative colorimetric, Bradford D50→D65), with RAWmakase's sRGB
  render of the settings Camera Raw embedded in its export.

For fitting the scene stage, samples Camera Raw clipped, or within 1% of the encoding
limits, are excluded and counted; final-render scores include them. Camera Raw
references are never re-blessed; RAWmakase snapshots and accepted-distance baselines
are re-blessed only per case with a reason.

## Survey (Camera Raw 18.7, Process Version 2012)

### Execution table

Measured with the probes above: each control rendered with a linear and with a curved
profile tone curve; "before the curve" means applying the curve to the linear render
predicts the curved render to the noise floor (encoded MAE 0.0003–0.003), "after"
that the control's curve measured on the linear probe predicts it when applied after
the profile curve. Look order: the same with a DCP LookTable, an XMP HSV look and an
XMP RGB-table look.

| Control | Stage | vs LookTable / HSV look | vs RGB-table look | Colour handling |
|---|---|---|---|---|
| Exposure | S2 (scale), then S3's white point | before | before | RGBTone (white-point rolloff) |
| White point (default) | S3 | before | before | RGBTone |
| Whites | S3, with the white point | before | before | RGBTone |
| Blacks | S3, after Whites (exact composition) | before | before | per channel (negative) |
| Highlights, Shadows | S3 | before | before | luminance gain |
| Clarity | S3 | before | before | luminance gain |
| Dehaze | S3 | before | before | per channel (positive: from the airlight) |
| Texture | before the profile curve (full-resolution detail) | before | before | per channel |
| Contrast | S5, after the profile curve and looks | after | — | RGBTone |

### What the default render does in S3

Camera Raw's default render is not the identity in S3. It maps a white point W* to
output 1, with a shoulder above about 0.25 (scene-linear), and leaves lower values
alone (identity when W* = 1), with the toe of its black point below about 0.02:

- W* is the sensor's white in scene units after Exposure, limited to twice the photo's
  maximum but not below 1. When the sensor's white is below 1 (negative Exposure), W*
  is that white, but not below 0.5. The sensor's white is the level at which a
  neutral's *last* channel clips, times 2^(baseline exposure + Exposure): with the
  first channel's clip the synthetic chart, whose white balance is not neutral, was
  0.02–0.04 too bright at +2 and +3 EV; with the last channel's, its gray ramp is within
  0.001 at Exposure 0 and −1.
- The photo's maximum is that of its brightest channel on a reduced copy (128 pixels
  on the long edge): on a 920-pixel probe a bright spot counts fully from about 8 pixels
  across and not at 2, and twice as large on a probe twice the size. A red highlight
  counts like a white one.
- The black point's toe does not follow the camera's baseline exposure. Engine 4 scaled
  it with the baseline (0.0015 × 2^(baseline + Exposure)), which crushed the deep
  shadows of files with a large baseline.
- The curve family is measured on neutral ramps at quarter stops of W*; above W* = 4
  it repeats in two-stop steps. Predicted from the table and this rule, 60 probes with
  different sensor whites, image maxima, sizes of bright areas and Exposure settings
  are within 0.00026 mean (0.0015 worst) encoded.

Engine 4 clipped at 1 instead: a probe whose sensor white is at scene 4 rendered
scene 1.0 at 1.0, where Camera Raw renders 0.69.

### Whites and Blacks

- **Whites** is a curve family over scene values, applied RGBTone. Negative Whites
  depends on the white point only; positive Whites stretches toward the photo's own
  maximum: a photo 4 stops below its sensor's white gets most of those stops back at
  +100 (a probe whose maximum is 0.25 maps 0.0625 to white). Two probes with the same
  white point and maximum but different sensor whites differ too, so the table is
  measured over sensor white × the photo's maximum below it × Whites (`white3.bin`).
  On photos the level it stretches toward is not their maximum: a specular highlight
  at the sensor's white does not stop it. For each of 22 training photos the level
  that best reproduces Camera Raw's Whites +100 through the table is, within 0.1 stops
  on average (median 0.06), twice the 99th percentile of the photo's luminance, at most
  its sensor's white; its maximum misses by 1.0 stops on average, the 99.9th
  percentile by 0.6. RAWmakase stretches toward that level; the white point keeps the
  maximum.
- **Blacks** composes after the white point and Whites exactly (encoded error 0.0000:
  Blacks maps their output). Negative Blacks maps the photo's darkest level to black
  when it lies above a fixed black point (about 0.016 scene); even a 4-pixel dark spot
  counts, so the minimum is measured at higher resolution than the maximum. Positive
  Blacks does not depend on it. Negative Blacks works on each channel.
- **The default black** adapts to the photo's darkest level relative to the sensor's
  white: the 0.1th percentile of the measurement copy's luminance less the sensor's
  white (log2, both at the render's Exposure, so Exposure does not move it: letting
  Exposure move the darkest level against the sensor's white at Exposure 0 does worse
  on Camera Raw's references, the exposure ramp at +1 0.0013 → 0.0027 mean error and
  the chart's Exposure +1, +2 and +3 0.32, 0.31, 0.20 → 0.35, 0.36, 0.26 ΔE00, with
  −2 unchanged at 0.0045). It is
  measured on the photo before red eye corrections and spot removal, so a local
  repair does not move the whole photo's black, and a few dark pixels do not move
  the percentile; on the probes the darkest patch covers far more than 0.1% of the
  pixels, so it is the darkest patch. (Blacks keeps the minimum.) The black ramps
  (darkest patch 2^-8 … 2^-2 at a sensor white of 4, so −10 … −4 relative to it)
  show a default black that depends on the level; the white curves, measured on
  white ramps whose darkest patch is 2^-14 of their white, carry almost none.
  `default_black.bin` maps the white ramp's default output to the black ramp's at the
  same scene value, per key; with a profile's default black (DefaultBlackRender
  Auto) it applies to each channel after the white curve and before Blacks. Between
  keys it is interpolated linearly, above −4 the last key holds, and below −10 it
  fades linearly to none at −11.5 and below. The anchor is set by the synthetic
  chart: it sits at −11.6 and Camera Raw renders it with no extra black (the white
  ramps, at −14, stay without one too). Below a key's darkest patch the curve keeps
  the ratio of its first sample. On the training photos' deep shadows against Camera
  Raw's renders with a linear profile, the ratio of RAWmakase's level to Camera Raw's
  in the bands 2^-9, 2^-8, 2^-7 and 2^-6 was 1.77, 1.44, 1.24 and 1.13 without it and
  is 1.18, 1.06, 1.01 and 1.02 with it (fading to none at −14 instead: 1.17, 1.04,
  1.00 and 1.01; with the minimum in place of the percentile: 1.20, 1.09, 1.03 and
  1.03).

### Local operators

Shadows, Highlights and Clarity are luminance gains from a smooth, edge-aware base
level: on photo-like synthetic scenes their gain is flat within regions whatever their
texture, and stops at edges. Their strength follows the photo: a dark square is lifted
+0.9 EV by Shadows +100 in a mid-gray scene and +1.6 EV in a bright one, and dark areas
of a dark scene are hardly lifted; Highlights −100 pulls the brightest area of a dark
scene down by up to 5.8 EV. With an ideal key per scene, one gain curve of the base
level relative to it explains Shadows +100 to 0.056 EV and Highlights −100 to 0.050 EV
on 40 scenes; keys predicted from the photo's percentiles (Shadows: 0.25·p99.9 +
0.75·p99, Highlights: halfway between p75 and p1 of log luminance) leave 0.17 and 0.11
in cross-validation, against 0.42 and 0.29 for doing nothing. A second dimension (the
pixel against its base) did not help in cross-validation.

On photos the synthetic Shadows tables did not carry over as well as Highlights'. On
22 training photos (cross-validated by photo, luminance gain in EV), Shadows' tables
fitted on the photos themselves against the photo's mean luminance (log2 of the mean
of the map's luminance) leave 0.12 at +100 and −100, against 0.25 and 0.24 for the
synthetic tables and 0.43 and 0.34 for doing nothing; the 90th percentile key does as
well. Highlights' synthetic tables match the photos' luminance gain (0.09–0.10 EV at
±100), but positive Highlights renders closer to Camera Raw with tables fitted on the
photos: rendered with tables fitted on the other half of the photos (two folds, 22
photos), median ΔE00 at +100 is 1.80 against 1.91 for the synthetic tables, at +50 1.57
against 1.69. At −100 the photo tables gain nothing (1.63 against 1.64), so negative
Highlights keeps the synthetic tables. The photo tables are measured through the
engine's own global curves, so they were refitted once the default black (below) was
added: the earlier taps lacked it and the fit had taken Camera Raw's black for Shadows'
gain (median ΔE00 on the training photos, Shadows +100 1.55 → 1.53 and +50 1.16 → 1.11,
Highlights unchanged within 0.02); the cross-validated figures above are from the earlier
taps.

**Open: Shadows +100 in small dark areas.** On the training photos Shadows +100 leaves
the darkest display tones (L* below 15) 5–7 L* too light on average, almost all of it
from photos where they are small areas (0.2–3.5% of the frame) inside brighter
surroundings: Camera Raw lifts them by about 2.3 EV in its linear output, RAWmakase by
about 3 EV, the gain large dark areas get at the same base level (those match within
about 1.5 L*). What was tried, on the training photos: a second table dimension for the
pixel against its base (Camera Raw does lift darker-than-their-base pixels less; in
cross-validation 0.66 → 0.52 EV error in the deep shadows, but the display error only
6.6 → 5.0 L*), including pixels below 2^-10 in the fit (no change), smoother bases
(guided filter ε 0.03–8, radius 1.6–6% of the long edge, refitted: no change), and
applying the default black before the local gain (5.0 L*, but Blacks ±50 and the
default render get worse). None is in the engine. Clarity's weights keep the percentile key they were fitted
with.

### Dehaze

On synthetic scenes Dehaze works on each channel by its level relative to the photo's
bright end (the 99th percentile of luminance): no change at the top and about −0.5 EV a
few stops below at +40. A curve per amount explains ±40 to 0.10 (doing nothing:
0.36–0.64) and +100 to 0.46 (1.22) in cross-validation; the rest is spatial. On the 22
training photos the same curves fitted on the photos themselves do better at every
amount (cross-validated by photo, per-channel gain in EV): +100 0.54 against 0.69 for
the synthetic curves and 0.90 for doing nothing, +40 0.16 (0.18, 0.30), −40 0.14 (0.16,
0.57), −100 0.34 (0.39, 1.45). **Negative Dehaze** uses these photo-fitted curves.

**Positive Dehaze** is a dark-channel haze removal model (`scene_tone/haze.rs`), on the
scene tone stage's input (linear ProPhoto), measured on the photo's measurement copy (512
pixels on the long edge) at Exposure 0:

1. The dark channel: the minimum of the darkest channel over a (2r+1)² window, clamped
   at the borders, with r = max(1, round(0.005 × the copy's long edge)) (3 at 512). The
   airlight A, per channel, is the mean of the pixels at or above the dark channel's 99th
   percentile (sorted index ⌊(n−1)·0.99⌋).
2. The haze's density: the same window minimum of min_c(I_c / A_c), smoothed by a guided
   filter with itself as the guide (box radius 2r, ε = 0.01).
3. A pixel p at Dehaze s > 0 (the slider plus the masks' Dehaze there) becomes
   J = (p − g·A) / t + g·A, with t = max(1 − ω(s)·density, 0.3), the density read
   bilinearly at the pixel's position and g = 2^Exposure. ω is linear between (0, 0),
   (+20, 0.3), (+40, 0.45) and (+100, 0.9). No channel falls below half of p scaled by
   the luminance's change, so weak channels of bright saturated colours are not driven
   to black (on the training photos: +100 3.06 → 3.04; a floor of 0.7 costs 3.18, 1.0
   costs 4.53). The transmission floor 0.3 costs nothing there (3.06 at 0.1 to 0.3).

The model's few parameters (the window, ε, the transmission floor and ω) were chosen by
grid search on the 22 training photos against Camera Raw 18.7, by display ΔE00. Median
over the photos, against the curves above: +100 5.97 → 3.06, +40 2.73 → 1.68, +20 1.42.
A and the density read exactly what the photo's measures read (white balance, the
profile, calibration, the camera's exposure), so they are kept with them in the stage
cache, measured the first time a render has positive Dehaze somewhere; Exposure scales A
with the scene.

The model is fitted to photos; on synthetic inputs it does less well than the curves.
On the stage probes at +40 the error of the effect grows on all three scenes (0.048 →
0.110, 0.069 → 0.152 and 0.036 → 0.044 EV), and on the synthetic chart it darkens Dehaze
+100 more than Camera Raw (p95 ΔE00 11.3 → 17.1, while the mean improves 6.9 → 5.9):
its wide-gamut cyan patches lose their red, which Camera Raw only reduces, and its dark
gray field darkens strongly.

### Masks

A full-frame mask's Shadows, Highlights, Dehaze, Clarity and Contrast render like the
global sliders in Camera Raw (within 0.006–0.012 encoded, against effects of
0.03–0.12); its Whites and Blacks do not (Blacks +50 differs by 0.07 for an effect of
0.013). Lightroom stores a mask's Exposure normalised: `LocalExposure2012` 1 is +4 EV
(0.8 rendered as +3.2 EV).

## Testing

- **Tier 1 (stage probes, CI)**: `scene_probes_do_not_regress` (`tests/color/scene_probes.rs`)
  renders synthetic probes to the S3 output tap (`develop::quality::render_stage`) and
  compares them with Camera Raw 18.7's renders of the same probes
  (`tests/corpus/camera-raw/scene-probes.json`, from `scripts/corpus/scene-probes.py`):
  white ramps at W* = 1, 4 and 16 (default, Whites ±50 and ±100), Exposure −2, −1 and +1
  on ramps at W* = 1 (clipped) and 4 (overrange), black ramps with darkest levels 2^-4
  and 2^-8 (Blacks ±50 and ±100), a full-frame mask's Whites +50 and Blacks −50, and
  three synthetic scenes (`probe_scenes.py` seeds 1, 3, 4, committed reduced to 384×256)
  at Shadows +100/−50, Highlights −100/+50, Dehaze ±40 and Clarity ±50. Ramps are scored
  by patch means, scenes by 24×16 block luminances; each case records its encoded (gamma
  1.8) mean and largest error and, for scenes, the mean error of the setting's effect in
  EV in `scene-probes-baseline.json`. A case fails when it gets further from Camera Raw
  than its baseline (5% plus 0.0005 mean, 0.003 largest, 0.005 EV effect); intended
  changes are blessed per case with `RAWMAKASE_BLESS=1`.
- **Tier 2 (structural, CI)**: stage placement (the full render equals the downstream
  stages replayed from the S3 tap), CPU/GPU equivalence per stage, cached against fresh
  renders, photo measures equal across preview sizes, regions and export.
- **Tier 3 (photo gates, private)**: below.

## Photo gates

`scripts/corpus/gate.py` holds the frozen metric and the acceptance rule. Photos,
references and renders stay outside the repository.

**Sets.** Train (fitting statistics), validation (accept or reject during
development) and holdout (scored once, at the switch decision), fixed before fitting,
from raw.pixls.us CC0 RAWs and the owner's RAWs. Each is stratified by scene
brightness: the 90th percentile of L* of Camera Raw's default render (Adobe
Standard), below 40, 40–75, above 75. One photo per camera model, so a scene never
appears in two sets.

**Settings.** Each percentage slider (Contrast, Highlights, Shadows, Whites, Blacks,
Texture, Clarity, Dehaze) at ±25, ±50, ±100; Exposure at −2, −1, +1, +2 EV; Exposure −1
with Whites +100, Shadows +50 with Highlights −50, Contrast +50 with Blacks −50,
Dehaze +40 with Clarity +40, a gradient mask with Exposure +2 EV and Shadows +50 (0.5 and 0.5 in Lightroom's normalised local units); and,
with Adobe Color, the default render, Shadows +100, Dehaze +40 and Whites +100 (#354).

**Metric.** Both renders reduced by area to 1024 pixels on the long edge. Per-photo
score: median ΔE00 over pixels (secondary: mean). A setting's score over a set: the
median of the per-photo scores (secondary: mean). Signed ΔL*: mean of RAWmakase −
Camera Raw L* over pixels whose Camera Raw L* falls in a 10-wide band, pooled over a
stratum's photos; bands with less than 1% of the pixels are skipped. Clipping
disagreement: the fraction of pixels where exactly one render is at white (any
channel ≥ 0.995) or at black (all ≤ 0.005). Valid pixels: the common frame less a
3-pixel border. Final-render scores include clipped pixels.

**Acceptance rule** (fixed before fitting). For every setting, on validation and then
holdout, in every brightness stratum:

1. **Material improvement**: the setting's score ≤ the default render's score + 1.0,
   or ≤ 60% of engine 4's score for that setting.
2. **Bands**: |signed ΔL*| ≤ 3 in every band, or ≤ 50% of engine 4's in that band.
3. **No photo regresses**: per-photo score worse than engine 4's by at most 1.0 for
   settings whose largest slider magnitude is ≤ 50 (Exposure ±1 EV counts as 50,
   ±2 EV as 100) and 2.0 above that.
4. **Endpoints**: clipping disagreement ≤ engine 4's + 0.5 percentage points.
5. **Default render** (all sliders 0): score no worse than engine 4's + 0.1.

Settings where engine 4 is already within the default render's score + 0.5 need only
rules 3–5. A stratum with fewer than 3 photos, or a photo with less than 50% valid
pixels, is inconclusive, which fails the gate until data is added. Any exception
needs the owner's explicit, documented approval per setting. A holdout is scored
once; if it fails and its errors are inspected, it becomes validation data and a
fresh holdout is frozen before further fitting. Adaptive statistics are identified
with controlled synthetic probes before fitting; no offsets are fitted to photos.
Engine 4's photo results are frozen first, as the comparison baseline.
