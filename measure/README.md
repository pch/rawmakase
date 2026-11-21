# Look channel curves in HDR (#949)

`look-channels-hdr.json` holds Camera Raw 18.7 (2743) renders of looks whose
parameters carry per-channel point curves (`crs:ToneCurvePV2012Red/Green/Blue`). It
covers a gray ramp and colour patches, in HDR and SDR. It is a diagnostic for
implementing those curves. It is not part of the corpus, and no test reads it.

## Inputs

- **Looks**: `tests/corpus/looks/synthetic-channels.xmp`, `synthetic-channels-curve.xmp`
  (the same channel curves with the S master curve), `synthetic-channels-red.xmp`
  (a Red curve only: its Green and Blue elements are absent) and the existing
  `synthetic-curve.xmp` (the S master curve only). They all use the 4×2×1 identity
  HSV table. Each look goes into the sidecar the way Lightroom writes one: the
  `crs:Look` element with its parameters and curves, and `Table_` at the top level
  (`camera_raw.look_element`). Every returned XMP keeps the `crs:Look` element with
  its channel curves.
- **Gray ramp**: the frozen HDR family's probe
  `088785c379d181291aedb1ea714026143cf9b2605a2066776dc9fb38f976943f.dng`
  (`$RAWMAKASE_DATASET/baselines/hdr/family/sources/`). This is the ramp behind the
  "Looks" measurements in docs/records/hdr-stage1-results.md (tags `w4 …`, `lin …`).
  It is a 920×344 `probe_dng.py` DNG: a linear embedded profile (`RAWmakase Probe`,
  ProfileToneCurve 0,0 → 1,1, no look table), and camera RGB that is linear ProPhoto.
  It has BaselineExposure +4, so the sensor white is scene 16. It holds 192 gray
  patches from scene 0.00098 to 16, in steps of about 0.073 stop, on a 0.05
  background.
- **Colour patches**: the family's probe `9e623038454c…dng` (the `colors` /
  hdr-looks `c1` probe; same profile and exposure). It holds 216 patches: twelve hues
  at several saturations, with the largest channel from 0.1 to 8 times white.

## Captures

The settings are `camera_raw.BASE` (Process Version 15.4, As Shot white balance, lens
and detail processing off) plus `HDREditMode=1, HDRMaxValue=8` (HDR) or
`HDREditMode=0` (SDR). The profile is the embedded one. Photoshop 2026 saves HDR as
16-bit P3 PQ and SDR as 16-bit Display P3.

```sh
cd scripts/corpus
~/.cache/rawmakase-vlm-venv/bin/python -B hdr_reference.py capture CONFIG OUT --factor 1
```

The two job lists (`tag`, source, settings, look file and amount) are recorded per
case in the JSON. The first capture had 14 jobs. On the ramp: no look, channels at
amount 1 and 2, channels-curve at 1 and channels-red at 1, each in HDR and SDR. On
the patches: no look and channels-curve at 1, each in HDR and SDR. The second capture
added three HDR jobs: curve-only on the ramp, and channels and curve-only on the
patches. They are there for the per-channel test below. The full-size TIFFs stayed in
a scratch folder and are not kept.

`hdr_reference.py` decodes PQ with the ST 2084 EOTF and divides by 203 cd/m². SDR is
decoded with the sRGB curve. Both give linear Display P3 with graphics white at 1.
The no-look renders (ramp HDR and SDR, patches HDR and SDR) are pixel-identical to the
frozen family's captures 183, 184, 240 and 242.

## Reduction

```sh
/opt/homebrew/bin/python3.12 -B measure/look-channels-hdr.py measure/look-channels-hdr.json OUT1 OUT2
```

- `scene` is the patch's value in the DNG: the sensor value × 2^BaselineExposure,
  in linear ProPhoto RGB, with 1 at graphics white.
- `rgb` is the mean of the render over the patch, 4 px inside its edge. It is
  converted from linear Display P3 (D65) to linear ProPhoto RGB (D50) with
  `M = ProPhoto_from_XYZ(D50) · Bradford(D65→D50) · XYZ_from_P3(D65)`, and graphics
  white stays at 1. The script holds the matrices. Every patch is flat in every
  render (`rgb_sd` is 0).
- `p3_clipped` marks a row where a Display P3 channel of the render is exactly 0.
  There the output's gamut clip hides Camera Raw's pipeline value. This happens in
  the deep shadows of every look that lifts red, because a red lifted alone in
  ProPhoto falls outside P3. It happens up to scene 0.009 for channels at amount 1,
  0.077 at amount 2, 0.020 for channels-curve and 0.004 for channels-red, and on
  saturated patches. Skip these rows when you fit.
- `tests` compares models of the colour patches against Camera Raw's HDR renders.
  Each model predicts a patch from the no-look render, through each look's gray-ramp
  map (no-look gray → look gray, per channel, log-log interpolation). It uses only
  patches that stay inside P3 (every P3 channel ≥ 0.003) in the renders compared and
  in the prediction. Errors are |log2| per channel. RGBTone here is the DNG SDK's
  form: the curve on the largest and smallest channel, with the middle one
  interpolated in linear values.

## Findings

- The channel curves alone apply **per channel**: on the patches that stay in P3,
  the median is 0.0001–0.0002 stop and the maximum 0.0016, below and above white.
- With the S master curve, the order is **the master curve first, then the channel
  curves**. Channels first and then the master misses by a median of 0.074 stop
  below white (p95 0.30). Master first misses by 0.0006 (p95 0.04–0.05). On these
  patches, the master in RGBTone form and the master per channel can't be told
  apart (p95 0.037 against 0.050 below white, 0.013 against 0.008 above).
- A look with only a Red curve leaves Green and Blue as they are without a look,
  wherever P3 doesn't clip them. Camera Raw applies it and doesn't refuse it.
- In HDR, Red and Green at amount 1 and 2 end at 16.00 at scene 16 (the no-look
  render reaches 16.04). Blue's curve ends below white (255 → 230), and it renders
  10.71 at amount 1 and 7.25 at amount 2.

## Round 2: the user's settings against the look's channel curves

`look-channels-hdr-2.json` asks where a look's channel curves sit relative to the
user's own settings. It uses the same ramp probe (`088785…`), settings, capture command
and reduction as above, in the same format. `tests` is empty, and `p3_clipped` is marked
as before.

- **Cases** (all on the gray ramp, with `synthetic-channels.xmp` at amount 1 where a
  look is named):
  - the look plus the user's red curve (master, green and blue identity; red
    0,0 / 128,100 / 255,255), in HDR L8 and SDR;
  - the look plus the user's S master curve (0,0 / 64,48 / 128,128 / 192,208 /
    255,255; channels identity), in HDR L8 and SDR;
  - the look plus Contrast2012 +50, in HDR L8;
  - the controls without a look, in HDR L8: the user's red curve, the user's S curve
    and Contrast +50.
- **Checks**: the user-red control is pixel-identical to the frozen family's capture
  1004 (`R:dark L8`), and the Contrast +50 control to capture 189 (`top16 lin C50 L8`).
  A repeat of round 1's `ramp channels a1 L8` reproduced its pixels exactly.
- **The chart**: `tests/corpus/cases.json` gained `look-channels-user-s`,
  `look-channels-contrast+50`, `look-channels-exposure+1` and `look-curve-user-s` (the
  S master look plus the user's S master curve), rendered into
  `tests/corpus/camera-raw/synthetic-d65.json` with `camera-raw-charts.py`.

### Order

Each prediction composes two of the ramp's gray maps per channel (no-look gray →
setting alone, no-look gray → look alone; the same log-log interpolation as `mapping`
in the script), in both orders, and compares them with the combined render. Rows that
P3 clips, rows below scene 0.01 and values at the 16 ceiling are skipped. The errors
below are |log2| per channel.

| HDR L8 | setting first, then the look's channels | the look's channels first |
| --- | --- | --- |
| user red curve (red), below / above white | median 0.0001 / 0.0003, max 0.006 / 0.013 | median 0.136 / 0.167 |
| user S master, below / above white | median 0.0002 / 0.0001, max 0.0012 / 0.0006 | median 0.083 / 0.145 |
| Contrast +50, below white | median 0.0002, max 0.0027 | median 0.093 |

- The user's point curves, master and per channel, and Contrast all apply **before**
  the look's channel curves, in HDR as in SDR (Exposure measured in SDR only). Round 1
  put the look's master curve before its channel curves. These captures don't place the
  look's master curve relative to the user's settings: the chart has no case of
  `synthetic-curve.xmp` alone at amount 1 to compose with `look-curve-user-s`.
- Contrast +50 lifts blue above the ramp's top (18.8 at scene 16). Above scene 13.7 the
  look's blue map is clamped at the ramp's end there, so the composed prediction falls
  short by up to 0.24 stop. Camera Raw goes on along the look's blue curve: 12.63 at
  scene 16, against 10.71 without Contrast. Below that point, Contrast first matches
  above white too (median 0.0002).
- On the chart (SDR, encoded sRGB, the 31 near-neutral patches) the same composition of
  `curve-s`, `contrast+50` or `exposure+1.00` with `look-channels-100` gives a median
  error of 4–8 / 65535 with the setting first, against 547–1161 with the look first.
