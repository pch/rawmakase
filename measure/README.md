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
