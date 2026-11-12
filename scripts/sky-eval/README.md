# Select Sky evaluation

Compares Select Sky masks of this checkout (candidate) with a baseline commit and with
Lightroom's Sky mask, on a fixed photo list. It writes metrics (IoU, boundary F against
Lightroom), blind review panels, a review by Claude models and an HTML report. Written for
pch/rawmakase#801 ("IoU or boundary F-score against hand-drawn or Lightroom masks").

```bash
scripts/sky-eval/run.sh
```

Requirements: `run.sh` runs on Windows under Git Bash (it uses `cygpath`,
`.venv/Scripts/python` and defaults under `$LOCALAPPDATA`), with Python 3 and cargo on the path.

| Variable | Default | |
|---|---|---|
| `BASELINE_REF` | `main` | commit the candidate is compared with |
| `BASELINE_EDGE` / `CANDIDATE_EDGE` | `1280` / `1280` | selection input long edge, as each ships (the app renders every mask generator's input at 1280) |
| `PHOTOS` | `scripts/sky-eval/photos.txt` | photo list, relative to `../rawmakase-samples/sky`; `photos-all.txt` = all 138 photos with Lightroom TIFFs |
| `REVIEWERS` | `opus,sonnet` | Claude models via the `claude` CLI; Haiku is refused |
| `SKIP_REVIEW` | unset | `1` = metrics, panels and report only, no model calls |
| `FINDINGS` | unset | hand-written HTML inserted into the report |
| `REMEASURE` | unset | `1` = compute cached masks again to time both sides in this run |
| `RAWMAKASE_SKY_MODEL` | `$LOCALAPPDATA/RAWmakase/models/upernet-convnext-base-v1` | the installed sky model's folder |
| `RAWMAKASE_ORT_LIB` | `$LOCALAPPDATA/Programs/RAWmakase/onnxruntime.dll` | the ONNX Runtime library |
| `SAMPLES` | `../rawmakase-samples/sky` | photo folder, a sibling of the repository |
| `LR_DIR` | `$SAMPLES/LR-mask-exports` | Lightroom exports, see below |
| `DATA` | `../rawmakase-testdata/sky-eval` | everything the run writes, see below |

The photo lists name the author's private set (Wikimedia Commons and Unsplash downloads plus
NEFs), which is not in the repository. To use your own, write a list of paths relative to
`SAMPLES`, one per line, each photo with its Lightroom exports in `LR_DIR`, and pass it as
`PHOTOS`.

Everything it produces lives outside the repository, in `../rawmakase-testdata/sky-eval/`:

- `masks/<sha>-<edge>/` — coverage PNGs per commit and edge, reused by later runs (a checkout
  with uncommitted changes under `crates`, `src` or `tests` keeps its masks in the run);
- `runs/<date>-<candidate>-vs-<baseline>/` — `photos.txt`, `run.json`, `timings.txt`,
  `metrics.json`, `key.json` (which letter is which source), `review/` (panels),
  `review_<model>.json` (+ raw replies), `summary.json`, `report/` (`index.html` + `img/`);
- `worktree-baseline/`, `target-baseline/` — the baseline checkout and its build;
- `.venv/` — numpy, pillow, scipy, tifffile.

Steps, each runnable on its own: `tests/sky_eval_dump.rs` (masks; copied into the baseline
worktree, with `rawmakase-selection` added to its test dependencies for the run; it needs a
baseline that has `rawmakase-selection`, which holds the selection models since they moved out of
`rawmakase-inference`), `evaluate.py` (Lightroom mask, metrics, panels), `review.py` (blind review),
`aggregate.py` (decode, composite score), `report.py` (HTML).

`tests/sky_samples.rs` is the earlier, simpler Lightroom comparison that `evaluate.py`
supersedes. It reconstructs Lightroom's mask with a different formula, so its numbers are not
comparable with this tool's.

## Lightroom reference exports

Per photo, two exports in `LR-mask-exports/` that differ only by a Sky mask with Exposure −4:
`<stem>_original.<ext>` and `<stem>_sky-exp_-4.<ext>`. Export at full size, no lens or
profile correction, no output sharpening, no noise reduction.

**Prefer 16-bit TIFF** (`.tif`, sRGB, uncompressed or ZIP) for **both** exports, the unmasked
one included: `evaluate.py` takes a TIFF over a JPEG of the same stem and pairs them by stem.
This helps even when the source photo is a JPEG: its own artefacts are the same in both exports
and cancel in the ratio; the noise comes from rounding and compressing each export.
Noise reduction and sharpening only need to match between the two exports. With 8-bit JPEGs Lightroom's tone curve squeezes −4 EV to one to three
levels in a dark sky, so JPEG noise shows up as mottled coverage and false blobs in dark
areas; 16-bit has 256 times finer levels and no compression artefacts.

**Frames.** Lightroom always applies a Nikon Z lens's built-in distortion profile; RAWmakase's
render for the selection is uncorrected, so for those NEFs the frames differ by a few per cent
of scale plus distortion. `run.sh` saves the model's input next to each mask (`<stem>.input.jpg`)
and `evaluate.py` fits a radial distortion model between that render and the Lightroom export,
then warps the masks into Lightroom's frame (`registration` in metrics.json; residual < 1 px).

**Star trackers.** Night photos taken with a star tracker have a sharp sky over a blurred or
blended foreground, so the edge is soft in the photo itself; every mask does poorly there.

The coverage is the darkening ratio divided by the ratio sure sky of the same original
brightness gets (calibrated per photo, per luma band). It is a reference shape, not ground
truth: Lightroom itself errs (it leaves bare crowns out, selects parts of signs).

## Scores

- **IoU** and **boundary F** at 50 % coverage, 2048 px long edge, boundary tolerance 0.75 % of
  the diagonal (DAVIS convention); only pixels where Lightroom's mask can be read count.
- **Panel**: each model scores every mask 0–10 for bleed, missed sky, edges, sky objects,
  reflections and overall, blind (masks shuffled per photo as X/Y/Z).
- **Composite** = 50 % panel overall × 10 + 25 % IoU × 100 + 25 % boundary F × 100.

Blind means the panels carry only letters, shuffled per photo with a fixed seed; the reviewer
is asked not to open other files and runs with the panels' folder as its working directory,
but nothing stops a reviewer from reading `../key.json`, and Lightroom's reconstructed mask and
main's coarser 1280 px staircase can be told apart by eye. In the `--prompts-only` flow
blindness rests on the answering agents being fresh.

## Large runs

- `review.py` reviews in batches of 8 photos (`--batch`), resumes skipped batches, and merges
  into `review_<model>.json`. If the `claude` CLI is not signed in, `--prompts-only` writes
  `prompts/batchNN.md`; give each to an Opus or Sonnet agent that saves its JSON as
  `prompts/batchNN.<model>.json`, then run `review.py --merge`.
- `report.py --compact --full 2000` writes two images per photo (overview, stacked crops),
  used inline and full screen: 138 photos = 276 files, ~106 MB, which fits an Artifact in
  two publishes (≤ 255 files and 64 MB each).
- `tuned.txt` lists what the modes were tuned on; the report scores holdout and tuned apart.
  The holdout is not untouched: the 138-photo run at `89e0bac7` showed the first design losing
  to main, and the design that ships (`d781bb6a`, main's edge as the base, modes only for
  crowns and sure lattices, water a veto) was chosen from those results. The holdout numbers
  therefore show how that design does on photos its thresholds were not tuned on, not on
  photos nobody looked at.

## Refining

- Add photos to `photos.txt` (with their Lightroom exports); keep holdout photos apart from
  the ones modes are tuned on.
- Write each run's conclusions to `runs/<run>/findings.html` and rebuild the report with
  `FINDINGS=… scripts/sky-eval/run.sh` or `report.py --findings`.
- Check reviewer claims against the panels before acting on them; Haiku inverted one photo
  in the first run, Sonnet favours smooth masks over fine twig selections.
