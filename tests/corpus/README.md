# Color test corpus

Tests that RAWmakase's colors don't change unnoticed, and how far they are from Camera Raw. The code is in `tests/color/`; the scripts that talk to Photoshop are in `scripts/corpus/`.

## What is here (public, committed)

| Path | What it is |
| --- | --- |
| `charts/*.dng` | Synthetic chart DNGs (970×742 RGGB mosaic, lossless JPEG, about 0.27 MB each), written by the generator in `tests/color/chart.rs` and `dng.rs`. |
| `charts/layout.json` | The patch areas every chart shares: 24-step gray ramp (−8 to +3.5 EV), 24 hues × 3 lightness × 3 chroma, a wide-gamut row, ColorChecker, skin tones, near-neutrals, two sweeps, and colors on black and white surrounds. |
| `cases.json` | 360 settings cases (sliders one at a time and at their extremes, every color mixer band at ±50 and ±100 and in combinations, Clarity, Texture, Dehaze, detail, grain, lens vignetting and Transform sliders, pairs (Contrast, Whites and Blacks together among them), parametric curve regions with moved splits and together, one combined look, Profile Amounts, RGB-table looks, settings inside looks, Point Color swatches) as Camera Raw XMP attributes. A case's `look` names a file in `looks/` and its Amount; its `curves` are written as XMP sequences, which also carry `PointColors` and `ColorVariance`. |
| `looks/*.xmp` | Synthetic look profiles with Profile Amount, RGB tables and develop settings, written by `scripts/corpus/synthetic-looks.py` from simple formulas (no Adobe data). |
| `snapshots/*.json` | RAWmakase's own render of every chart and case. |
| `camera-raw/*.json` | Camera Raw 18.7 renders of the synthetic charts with their embedded profile (no Adobe files involved). |
| `camera-raw/baseline.json` | RAWmakase's accepted distance from those renders, per case. |
| `camera-raw/scene-probes.json` | Camera Raw 18.7 renders of the scene tone stage's probes (linear-profile ramps and synthetic scenes, read at the scene stage): the ramps' patch values and layout, and per case the settings and Camera Raw's linear ProPhoto patch means or block luminances. Written by `scripts/corpus/scene-probes.py`; never re-blessed. |
| `camera-raw/scene-probes-baseline.json` | RAWmakase's accepted distance from those renders, per probe case. |
| `scene-probes/*.dng` | Three synthetic scenes (`scripts/corpus/probe_scenes.py` seeds 1, 3, 4) reduced to 384×256, as uncompressed probe DNGs (`probe_dng.py`, about 0.2 MB each). |
| `cameras.json` | LibRaw color matrices of the cameras that get their own chart. |
| `pixls.json` | CC0 sample RAWs from raw.pixls.us (URL, SHA-256, size). The files themselves are not committed. |

Patch files hold the mean encoded-sRGB value (16-bit) of each patch, one case per line.

### Charts

- `synthetic-d65`, `-d50`, `-a`, `-f2`: an invented camera (no manufacturer or Adobe data) under daylight, D50, tungsten and fluorescent light, with an embedded named profile (color and forward matrices), as Adobe's DNG Converter writes.
- `synthetic-d65-matrix-only`: the same scene with color matrices only, as some third-party DNGs are.
- `<camera>-d65`: one chart per camera in `cameras.json`, claiming that camera's make and model with its LibRaw matrix. With the camera's Adobe Standard DCP (private tier) this tests the per-camera profile path without a photo from that camera.

Mosaic charts are used because RAWmakase rejects three-channel `LinearRaw` DNGs. Flat patch interiors demosaic exactly.

## Tests

`cargo test --test color` runs in CI and takes seconds:

- `charts_match_their_generator`: the committed charts are exactly what the generator writes.
- `charts_decode_to_their_camera_values`: LibRaw returns the patch values that were written.
- `colors_match_snapshots`: every chart and case against the snapshots. Fails on any patch moving more than ΔE00 0.5, or a case's mean moving more than 0.1, and names the patches with their lightness, chroma and hue change.
- `exports_match_the_render`: 16-bit TIFF and JPEG exports carry the render's colors.
- `preview_size_matches_full_size`: a downscaled render keeps the colors of the full render.
- `scene_probes_do_not_regress`: the scene tone stage's Tier 1 probes (docs/scene-tone-stage.md#testing): ramps (rebuilt by `dng::write_probe`) and the committed scenes rendered to the stage's output and compared with `camera-raw/scene-probes.json`. Fails when a case's encoded mean or largest error, or a scene setting's effect error in EV, exceeds `camera-raw/scene-probes-baseline.json` by more than 5% plus 0.0005, 0.003 or 0.005 EV. With `--nocapture` it prints every case against its baseline.
- `camera_raw_parity_does_not_regress`: fails when a case gets further from Camera Raw than its baseline (mean +0.1 or p95 +0.3 ΔE00). With `--nocapture` it prints, per case, ΔE00, the tone offset on the gray ramp, the contrast (slope) ratio, the hue error and the chroma difference.

When a color change is intended, run the tests with `RAWMAKASE_BLESS=1` and commit the diff of `snapshots/` (and `camera-raw/baseline.json` or `camera-raw/scene-probes-baseline.json` if parity moved), saying why in the commit message. Charts are regenerated the same way after a generator change.

## Private tier

Kept outside the repository in `RAWMAKASE_CORPUS` (Piotr: `~/RAWmakase Corpus`, 7 GB cap):

```
raws/own/…              symlinks to your own RAWs (nothing is copied or written next to them)
raws/pixls/…            downloaded CC0 samples (scripts/corpus/pixls.py download)
camera-raw-adobe/       Camera Raw renders of the camera charts with Adobe Standard, plus baseline.json
camera-raw-photos/      Camera Raw renders of the photos as 48-across block averages, plus baseline.json
accepted/               RAWmakase's last accepted photo renders, as block averages
```

Run with `--release` (the dev profile is slow on 24 MP files):

```
RAWMAKASE_CORPUS=… RAWMAKASE_PROFILES=<folder of DCPs> cargo test --release --test color -- --ignored --nocapture
```

- `adobe_profile_parity_does_not_regress`: camera charts with each camera's Adobe Standard DCP against Camera Raw.
- `photos_camera_raw_parity_does_not_regress`: every photo reference against Camera Raw. `RAWMAKASE_PHOTO_FILTER` selects a subset by path.
- `photos_match_accepted_renders`: every photo and `photos` case against RAWmakase's last accepted render; files LibRaw can't open are listed as skipped until they have an accepted render.

`RAWMAKASE_PROFILES` is read only when set, file by file; DCP file names must contain the camera model (Lightroom's `CameraProfiles` folder works). Adobe profiles and numbers derived from them never go into the repository.

## Scripts

All run from the repository root with a Python that has numpy (`/opt/homebrew/bin/python3.12` here). Photoshop scripts refuse to start while Photoshop has documents open, open only RAW or DNG copies (never TIFFs, which block Photoshop with a dialog), and delete every render once it is reduced.

- `scripts/corpus/synthetic-looks.py`: writes the synthetic look profiles in `looks/`. Camera Raw reads them from the sidecar (`crs:Look` with its parameters and the `Table_` attribute), so nothing is installed.
- `scripts/corpus/camera-raw-charts.py`: Camera Raw renders of the synthetic charts into `camera-raw/`; with `--adobe`, of the camera charts with Adobe Standard into the private corpus.
- `scripts/corpus/camera-raw-photos.py`: Camera Raw renders of every corpus photo for the `photos` cases.
- `scripts/corpus/contrast-curve.py`: renders Contrast on `synthetic-d65`, prints the chart's Contrast table for `basic_tone_data.rs`, and with the private photo references fits the photo's Contrast pivot.
- `scripts/corpus/color-grading.py`: renders Color Grading's fitting cases (about 740: every region at twelve hues and four saturations, Shadows, Midtones and Highlights over a grid of Blending and Balance, the Luminance sliders, and held-out checks) on `synthetic-d65` as 16-bit ProPhoto RGB and fits `crates/rawmakase-engine/src/develop/color_grade_curves.bin` from them. The renders' patch means stay outside the repository.
- `scripts/corpus/scene-probes.py`: `references ROOT` writes `camera-raw/scene-probes.json` and `scene-probes/` from the scene tone survey's existing Camera Raw renders under ROOT (`render ROOT` renders missing ramp cases; the scenes come from `scene-tone-local.py synth`).
- `scripts/corpus/scene-tone-tables.py`: renders neutral-ramp probe DNGs (`probe_dng.py`) in Camera Raw (`camera_raw.py`) and writes the scene tone stage's global tables (white point and Whites, Blacks, masks' Whites and Blacks) in `crates/rawmakase-engine/src/develop/scene_tone/`.
- `scripts/corpus/scene-tone-local.py`: renders synthetic scenes (`probe_scenes.py`) and prepares training photos with a linear profile (`linear_profile.py`), then fits the local operators' tables (`local_tone_data.rs`, `clarity_data.rs`).
- `scripts/corpus/gate.py`: the scene tone stage's photo gates (docs/scene-tone-stage.md#photo-gates): Camera Raw references, RAWmakase renders, scores and the acceptance rule; photos and renders stay outside the repository.
- `scripts/corpus/parametric-curve.py`: renders the parametric curve's fitting cases (about 380 region and split settings) on `synthetic-d65` and fits `crates/rawmakase-engine/src/develop/parametric.bin` from them. The renders' patch means stay outside the repository.
- `scripts/corpus/pixls.py`: `manifest` (rebuild `pixls.json`), `download` (checks hashes and the budget), `cameras` (rebuild `cameras.json` from the corpus RAWs).
- `scripts/corpus/migrate-references.py`: reduce existing reference TIFFs (sweeps, Lightroom exports) to block files.
- `scripts/corpus/parity-report.py`: groups the Camera Raw comparison by Develop control (mean and p95 ΔE00, distance above the default render, worst cases and patches) into `report.json` and a self-contained `report.html`; `--previous` marks changes against an earlier report. It reads RAWmakase's patch values from the parity test:

  ```
  RAWMAKASE_PARITY_DUMP=<dir> cargo test --release --test color camera_raw_parity -- --nocapture
  python3 scripts/corpus/parity-report.py <dir> --out <report dir>
  ```

  `--photos` adds the default exposure per camera: how much brighter Camera Raw renders each unedited corpus photo with Adobe Standard (median log2 luminance ratio over midtone blocks, in EV) and its mean ΔE00, listed by camera only. To repeat it from scratch:

  ```
  python3 scripts/corpus/pixls.py download                                  # CC0 samples listed in pixls.json, hash-checked
  python3 scripts/corpus/camera-raw-photos.py --raws pixls --cases default  # Camera Raw renders, needs Photoshop
  RAWMAKASE_PHOTO_FILTER=/default RAWMAKASE_PARITY_DUMP=<dir> cargo test --release --test color photos_camera_raw -- --ignored
  python3 scripts/corpus/parity-report.py <dir> --out <report dir> --photos
  ```

  with `RAWMAKASE_CORPUS` and `RAWMAKASE_PROFILES` set as for the private tier.

## TODO

Known limitations, not yet addressed:

- **Photo parity baseline not recorded.** `camera-raw-photos/baseline.json` does not exist yet; run `photos_camera_raw_parity_does_not_regress` once with `RAWMAKASE_BLESS=1` (about an hour) before relying on that test.
- **Not yet run on CI.** The public tests pass on macOS (Apple Silicon). CI builds on Arch Linux x86_64, where floating-point results may differ slightly. Measure the difference and set the snapshot tolerances from it.
- **Tolerances are not measured.** The snapshot limits (ΔE00 0.5 per patch, 0.1 mean) and parity margins (+0.1 mean, +0.3 p95) are reasonable guesses, not derived from Mac/Linux or CPU/GPU spread. The GPU preview path is not covered at all.
- **Local operators are barely covered by charts.** Clarity, Texture and Dehaze have chart cases, but Shadows, Highlights, Dehaze and Clarity adapt to image content, so flat patches (even with the black/white surrounds) say little about them; only the private real photos test them properly.
- **sRGB only.** RAWmakase outputs sRGB, so the wide-gamut row and very saturated colors are clipped before comparison and saturation errors outside sRGB are invisible. Needs a wide-gamut (ProPhoto or linear) render output in RAWmakase.
- **Private tier is slow.** Photo parity against Camera Raw (1,305 references) takes about an hour and accepted renders about 20 minutes with `--release`. Trim to a representative subset (a few photos per camera, the `photos` cases) for routine runs.
- **Bad sample files are accepted.** Nikon Z5II and Z50II samples decode as "data corrupted" with LibRaw 0.22.0, yet their renders were recorded in `accepted/`. Exclude files LibRaw can't decode cleanly (also Z 8, Z6III and A1 II lossless, which don't open; the Sony A7 V opens since the LibRaw update) until LibRaw supports them.
- **Presets are not used.** The plan included about 40 of Piotr's Lightroom presets as realistic combinations; only 12 hand-picked pairs and one combined look exist.
- **Parity numbers are not in docs/parity-gaps.md.** `scripts/corpus/parity-report.py` summarises them per control; that document still quotes ad-hoc scorecard runs.
- **Known RAWmakase gaps the tests expose** (tests record them as the baseline, or fail on purpose):
  - Matrix-only DNGs (`synthetic-d65-matrix-only`) now render from the file's own D65 colour matrix and sit about ΔE00 0.9 from Camera Raw, the same as the fully-profiled `synthetic-d65` chart.
  - A DNG's embedded profile is rejected when it has no ProfileName ("Invalid profile identity").
  - Three-channel `LinearRaw` DNGs are rejected, which is why charts are mosaics.
  - Nikon Z6III: LibRaw reports model "Z6_3" but Adobe's DCP is named for the "Nikon Z 6 3", so Adobe Standard is not matched (`adobe_profile_parity` fails for `nikon-z6-3-d65`).
  - LibRaw trims 19 rows from Leica SL2 DNGs, so the SL2 has no chart.
