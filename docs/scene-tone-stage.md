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
| Dehaze | S3 | before | before | per channel |
| Texture | before the profile curve (full-resolution detail) | before | before | per channel |
| Contrast | S5, after the profile curve and looks | after | — | RGBTone |

### What the default render does in S3

Camera Raw's default render is not the identity in S3. It maps a white point W* to
output 1, with a shoulder above about 0.25 (scene-linear), and leaves lower values
alone (identity when W* = 1):

- W* is the sensor's white in scene units after Exposure (the clip level of a neutral,
  times 2^(baseline exposure + Exposure)), limited to twice the photo's maximum but
  not below 1. When the sensor's white is below 1 (negative Exposure), W* is that
  white, but not below 0.5: Camera Raw expands highlights by at most one stop.
- The photo's maximum is that of its brightest channel on a reduced copy: on a
  920-pixel probe a bright spot counts fully from about 8 pixels across and not at
  2 pixels, and twice as large on a probe twice the size.
- The curve family is measured on neutral ramps at quarter stops of W*; above W* = 4
  it repeats in two-stop steps. Predicted from the table and this rule, 60 probes with
  different sensor whites, image maxima, sizes of bright areas and Exposure settings
  are within 0.00026 mean (0.0015 worst) encoded.

Engine 4 clipped at 1 instead: a probe whose sensor white is at scene 4 rendered
scene 1.0 at 1.0, where Camera Raw renders 0.69.

### Measurement-input table

| Measure | Read from | Used by |
|---|---|---|
| Sensor white in scene units | metadata, white balance, baseline exposure, Exposure | white point |
| Photo maximum (brightest channel) | reduced copy, S2 before Exposure | white point |
| Photo minimum | reduced copy, S2 before Exposure | negative Blacks |
| Local keys | reduced copy, S2 | Highlights, Shadows, Clarity |

## Testing

- **Tier 1 (stage probes, CI)**: committed probe DNGs and their Camera Raw
  measurements.
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
Dehaze +40 with Clarity +40, a gradient mask with Exposure +0.5 and Shadows +50; and,
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
