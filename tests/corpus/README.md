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
| `dataset/pixls-decode.json` | Decode fingerprints of the test dataset's raw.pixls.us CC0 files (see [the pixls dataset](#the-pixls-dataset)). |
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
- `photos_match_accepted_renders`: every photo and `photos` case against RAWmakase's last accepted render; files LibRaw can't open are listed as skipped until they have an accepted render. `RAWMAKASE_PHOTO_SOURCE_EDGE=<pixels>` renders from the photo reduced to that long edge instead of full size (an accepted render made at another size fails); `RAWMAKASE_REPORT=<file>` writes every photo's outcome as JSON; `RAWMAKASE_BLESS=new` records only photos without an accepted render.

`RAWMAKASE_PROFILES` is read only when set, file by file; DCP file names must contain the camera model (Lightroom's `CameraProfiles` folder works). Adobe profiles and numbers derived from them never go into the repository.

## Scripts

All run from the repository root with a Python that has numpy (`/opt/homebrew/bin/python3.12` here; the dataset gate scripts below also need tifffile, which the dataset's `~/.cache/rawmakase-vlm-venv` has). Photoshop scripts refuse to start while Photoshop has documents open, open only RAW or DNG copies (never TIFFs, which block Photoshop with a dialog), and delete every render once it is reduced.

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
- `scripts/corpus/gate.py`: photo gates against frozen Camera Raw references (issue #420's level 3, the scene tone stage's gates): `survey`, `split` and `reference` make and freeze the references through Photoshop (`camera_raw.py`); `render` develops each photo once with every case's sidecar through `rawmakase render-batch <raw> <jobs.json>` (`--data-dir` for a library with Adobe Standard); `score` measures median ΔE00, ΔL* per brightness band and clipping; `accept` applies the acceptance rules to a candidate's scores against a baseline's: by default (`--rule regression`) no setting's median more than 0.1 ΔE00 worse per brightness stratum, no photo worse by more than a limit (tone 1.0 or 2.0; colour families their own, derived from `main`'s baseline with `color-limits`), no more clipping, and no photo that stops rendering; `--rule switch` is the scene tone engine's rule, which also asks far-off tone settings to improve; `--exclude <file>` leaves listed photos (SHA-256 prefixes) out of the verdict. Photos, references and renders stay outside the repository.
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

## The pixls dataset

A classified copy of the whole raw.pixls.us archive, for choosing test photos by camera, exposure, measured tone and scene, and frozen Camera Raw references for the scene-tone gate, lives in the [rawmakase-dataset](https://github.com/pch/rawmakase-dataset) repository (private for now) with the scripts that build it. Its `manifest --link` puts chosen photos where the photo tests above read them (`$RAWMAKASE_CORPUS/raws/pixls`), and its reference sets are made with this repository's `scripts/corpus/gate.py`.

### Decode fingerprints

`rawmakase fingerprint <files…>` prints one JSON line per file (format version 2) with what opening and unpacking it yields, read as development reads it: the file's SHA-256, the step that failed and its error, and three sections. `header` is what opening says before any sensor data is read: the decoder's own facts (the visible area's margins, its default and aspect-ratio crops in raw coordinates, the canonical make and model, the DNG version) and the photo's `Metadata` after `photo::open`, every field as it is saved (sizes, crops, orientation, white balance and presets, matrices, exposure, lens, Highlight Tone Priority, Fujifilm facts, small-RAW flag, Adobe names). `preview` is the embedded JPEG thumbnails use (size and SHA-256), or why there is none. `sensor` is what unpacking yields: the hashed area's origin and size, black levels per channel and their pattern, white levels, CFA pattern (`g` is LibRaw's second green) and a SHA-256 of the unpacked raw data in the visible area (LibRaw leaves the borders of some files unwritten). Nothing is demosaiced, so the values are the decoder's and RAWmakase's opening code's alone.

`rawmakase decode-dump <files…>` prints, per file, what each decode mode makes of it as the renderer receives it, with no decode cache: for the full and the half-size decode, the size, scale factor and integer-clipped count, a SHA-256 of the pixels (f32) and of the image's metadata, and per-channel means; `--floats <dir>` also writes each full decode that is not a mosaic (small RAWs, converted from YCbCr in floating point) as raw floats. `scripts/corpus/dataset-develop.py` records and checks these for the whole dataset as `dataset-fingerprints.py` does below (baselines in `$RAWMAKASE_DATASET/baselines/develop/`, local only, as the hashes depend on the platform's floating point).

A decoding difference made on purpose is approved in [`decode-deltas.toml`](decode-deltas.toml): the file's SHA-256, the field's dotted name (`header.metadata.wb`, `sensor.white`, `full.pixels_sha256`), its value before and after, the evidence and the approval. Both scripts then skip exactly that change; any other value is still reported.

`scripts/corpus/dataset-fingerprints.py` runs it on every file of the dataset (2,944 files, 56 GB: about 4½ minutes from the USB SSD on an M1 Pro) and compares the result with the baselines:

```
export RAWMAKASE_DATASET=<dataset checkout>
cargo build --release
python3 scripts/corpus/dataset-fingerprints.py check                 # every file; exits 1 on any difference
python3 scripts/corpus/dataset-fingerprints.py check --manifest m.json   # a dataset.py manifest's files
python3 scripts/corpus/dataset-fingerprints.py record --reason "…"   # write the baselines
```

The report lists files that start or stop opening and every changed field, grouped by make and format, and fails on files missing from the dataset, files whose bytes changed, files without a baseline and batches that crashed (whose files are then run one by one, so a crash is pinned to its file). `--public` names only raw.pixls.us CC0 files and counts the rest; `--report` writes the report as JSON.

- **Baselines:** the raw.pixls.us CC0 files' are committed in `dataset/pixls-decode.json`; every other file's stay in `$RAWMAKASE_DATASET/baselines/decode/<source>.json` on the dataset's machine. Record a baseline only in a commit that says why it changed, never just to make the check pass.
- **When:** on every LibRaw update (`packaging/native-deps.sh`), and before releases.
- **Same build, same machine:** a baseline records the LibRaw version, the commit `native-deps.sh` pins, the platform and the RAWmakase commit, and `check` notes when they differ. LibRaw decodes some formats with floating-point steps, and white balance is computed in floating point, so compare builds linked against `native-deps.sh`'s LibRaw on the platform that recorded the baseline (macOS arm64).

### Render snapshots

`photos_match_accepted_renders` over every photo the dataset measured (`raw_ok AND rawmakase_ok`: 1,818, of which 1,797 in formats the test reads), in a corpus of their own on the dataset's machine, with accepted renders recorded from `main`:

```
export RAWMAKASE_DATASET=<dataset checkout> RAWMAKASE_CORPUS=$RAWMAKASE_DATASET/baselines/corpus
(cd $RAWMAKASE_DATASET && python3 -I scripts/dataset.py manifest --where "raw_ok AND rawmakase_ok" \
    --out $RAWMAKASE_CORPUS/manifest.json --link)
python3 scripts/corpus/dataset-renders.py run --bless    # on main: record the accepted renders
python3 scripts/corpus/dataset-renders.py run --bless new   # only photos without one (added, or an interrupted run)
python3 scripts/corpus/dataset-renders.py run            # on a branch: exits 1 on any change
```

`scripts/corpus/dataset-renders.py` runs the test with `RAWMAKASE_REPORT` and groups the photos that changed by the dataset's classifications (make, CFA, brightness key, scene, light, setting, contrast, skin, sky), read from `dataset.sqlite` by SHA-256, so a change reads as "X-Trans, low-key scenes" rather than a list of photos. `report <file>` groups an earlier report; `--public` names only raw.pixls.us CC0 photos; `--out` writes the grouped report as JSON.

It renders from the photo reduced to 2,400 pixels (`--source-edge`, 0 for full size): the 48-block comparison sees nothing the reduction loses, and a run of all 1,797 photos takes about 2½ hours on an M1 Pro instead of about 15. Run it before merging rendering-engine changes. The accepted renders stay in `$RAWMAKASE_DATASET/baselines/corpus/accepted/`; re-record them only from `main`, in a change that says why. The current ones are main a1be2c1a's (1,800 photos), recorded with the gate baseline of the same commit.

### Camera Raw parity

The dataset's frozen Camera Raw 18.7 references (`references/camera-raw-18.7*/<control>-<split>/`, 48 validation and 48 holdout sets over raw.pixls.us, Signature Edits and the owner's photos) are rendered with a build and scored against Camera Raw by `scripts/corpus/gate.py`; `scripts/corpus/dataset-gate.py` does it for every set of a split and judges one run against another:

```
lib=<Adobe profile library>     # a folder whose camera-profiles/ holds Adobe Standard and the Adobe Raw looks
run=$RAWMAKASE_DATASET/baselines/gate
python3 scripts/corpus/dataset-gate.py run --rawmakase target/release/rawmakase --data-dir $lib --out $run/<name>
python3 scripts/corpus/dataset-gate.py accept --candidate $run/<name> --baseline $run/main-<commit>
python3 scripts/corpus/dataset-gate.py report $run/<name>     # --split holdout for holdout
```

- **run** renders and scores each set (two at a time) and deletes its renders once scored: the SSD cannot hold them all. It renders with a copy of the build taken when the run starts (`<run>/rawmakase`), so rebuilding meanwhile changes nothing. Scores, a log per set and `run.json` (the build) stay in the run folder; an interrupted run resumes. The validation split takes about 2½ hours on an M1 Pro.
- **accept** applies `gate.py accept` to every set of a split (`--split`, validation by default), which both runs must have scored. Default `regression` protects the baseline; `--rule switch` adds the scene tone engine's improvement rule; `--rule parity` adds absolute per-photo requirements (median ΔE00 ≤ 2, regional mean ≤ 5, clipping disagreement ≤ 0.5%). Every mode checks corresponding spatial/tonal regions against baseline +2 ΔE00 and per-photo clipping against baseline +0.5 percentage points. These are engineering budgets, not a claim of perceptual equivalence. Missing strata, unscored references and strata with fewer than three photos **fail**, including when the same gap exists in both runs.
- **report** distinguishes case-weighted scores from a deduplicated, equal-photo-weight summary, prints unique photo counts and repeated defaults, and reports errors by control, stratum and source. It is descriptive, never an acceptance verdict.
- **Default parity:** `gate.py default-parity --survey $RAWMAKASE_DATASET/references/<family>/survey --rawmakase … --data-dir $lib --out <folder>` scores default renders against the survey's Camera Raw renders, leaving holdout photos out.
- **Baselines** are runs of `main`, named by its commit; record a new one when `main`'s rendering changes. The current one is `main-a1be2c1a-metric2` (metric policy 2, validation and holdout): the scene tone engine with the camera calibration rows (#493, #497), frame fixes (#494, #499), Highlight Tone Priority from Canon's custom functions (#512), Sony extended low ISO (#514), Canon sRAW baselines (#515) and the clipped-channel floor (#513), recorded with LibRaw from `packaging/native-deps.sh`. The X-Trans demosaic (#518) and the Canon ColorData12 black (#517) came after it and have not been judged against it yet. `main-9ef177b8-metric2` is the scene tone engine as first merged (#416) and `main-418d0ca7` the engine before it. Judge on validation; score holdout once, at the decision: `run --split holdout` into the same run folder, then `accept --split holdout --exclude $run/holdout-excluded.txt` (holdout photos already looked at). A run folder holds one build with one profile library; another build needs its own.

Metric policy 2 requires new scores for **both** builds. Existing score files lack
the spatial/tonal measurements and are refused for acceptance/resumption. Use
new run folders with the original pinned binaries; frozen Adobe references and
old scores stay unchanged. Retained renders can instead be rescored with
`gate.py score`. Do not bless an engine snapshot to satisfy Adobe parity.

The dataset's `references --suite stress` creates a separate reference family
from genuinely clipped, dark or noisy captures, retaining the existing splits,
license rules and scene deduplication. It does not alter the general suite.
Generate it only when Photoshop and baseline generation are idle; new reference
sets then participate in `dataset-gate.py` automatically. Slider variants cannot
substitute for sensor clipping or capture underexposure. Missing stress strata
must be supplied with additional captures, not waived by the script.

Synthetic gate regressions run in CI without RAWs or Photoshop:
`python3 -m unittest discover -s scripts/corpus` (numpy and Pillow required).

### Testing a rendering-engine change

On the machine with the dataset, from the branch's checkout, built against `packaging/native-deps.sh`'s LibRaw (`PKG_CONFIG_PATH=<its prefix>/lib/pkgconfig cargo build --release`; `otool -L target/release/rawmakase` names that prefix, not Homebrew's LibRaw):

| Step | Command | Time | Expect |
|---|---|---|---|
| 1. Decoding | `dataset-fingerprints.py check` | 5 min | No difference: an engine change does not change decoding. |
| 2. Render snapshots | `dataset-renders.py run` | 2½ h | Every photo the change touches moves; read the grouping (make, CFA, key, scene, light) for where. Nothing is accepted here: re-record (`--bless`) on `main` after merging. |
| 3. Camera Raw parity | `dataset-gate.py run … --out $run/<branch>`, then `accept` against `main`'s baseline, then `report` | 2½ h | The verdict per set, and how far the branch is from Camera Raw next to `main`'s report. |
| 4. Default parity | `gate.py default-parity` per survey | about 1 h | Default renders against Camera Raw's. |

Steps 2 and 3 compare different things: render snapshots compare a build with RAWmakase's own earlier renders (whether anything moved), Camera Raw parity with Camera Raw (whether it moved closer). Run them one after the other; both use every core.

## TODO

Known limitations, not yet addressed:

- **Photo parity baseline not recorded.** `camera-raw-photos/baseline.json` does not exist yet; run `photos_camera_raw_parity_does_not_regress` once with `RAWMAKASE_BLESS=1` (about an hour) before relying on that test.
- **Run in CI on Linux.** The public tests pass on macOS (Apple Silicon). CI builds on Arch Linux x86_64, where floating-point results may differ slightly (tolerances not yet specifically calibrated for Linux float variance).
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
