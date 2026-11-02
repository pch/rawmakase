# AI Denoise

Implementation plan, 2026-11-02, for issues
[#737](https://github.com/pch/rawmakase/issues/737) and
[#753](https://github.com/pch/rawmakase/issues/753). Based on `79a7d44d`, the current
[architecture](architecture.md), [code map](code-map.md),
[demosaicing](demosaic.md), [RAW decoding](raw-decoding.md),
[rendering quality](rendering-quality.md), the model lifecycle of
[Subject and Background masks](subject-and-background-masks.md), the
[catalog format](catalogs.md) and the [shutdown contract](shutdown.md). Reviewed in six
Codex rounds and approved on 2026-11-02. This is a plan; nothing here is built yet.

## Outcome and scope

A Detail-panel **AI Denoise** switch with an **Amount** slider (0–100, default 50, as
Lightroom), for Bayer RAWs. **RawNIND UtNet2** (Benoit Brummer, UCLouvain) denoises and
demosaics the mosaic in one pass, locally, through the ONNX Runtime RAWmakase already
ships. The result is computed once per photo, kept in a disk cache and blended with the
ordinary demosaic by Amount. Everything after the camera image (manual noise reduction,
retouching, tone, colour, sharpening, masks) is unchanged.

First release:

- Bayer mosaics with a 2 × 2 colour period. X-Trans joins if phase 0 shows the RGGB
  rebuild (below) holds on our Fuji files; otherwise it, Quad Bayer, four-colour,
  monochrome, linear DNGs, small RAWs and non-RAW files show the switch disabled with
  the reason. Half-size drafts stay plain.
- One model, opt-in download, CPU inference everywhere; a GPU provider only where
  measurement shows finite output matching the CPU's.
- Lightroom's in-place Denoise imports as AI Denoise once its XMP encoding is captured
  from Camera Raw (step 3).

Not in the first release: RawNIND's linear-RGB model, remote inference, a choice
between models in the UI, writing Enhance filters back to XMP, AI Denoise inside
presets.

## What other tools do (research 2026-11-02)

Lightroom/Camera Raw (in place since Camera Raw 17.4, June 2025), DxO DeepPRIME, Topaz
RAW Denoise, ON1 NoNoise and Capture One denoise the mosaic jointly with demosaicing,
run the model once and keep the result; none shows a live full-resolution result.
darktable 5.6 (`neural restore`, PR #20523) runs RawNIND and writes a new DNG;
LightCraft (Rust) and RapidRAW keep a cached denoised base inside the edit and blend it
by Amount. LightCraft never exports the plain image in place of a missing result.
RapidRAW's RGB-domain NIND model shows pattern artefacts in shadows (RapidRAW #2047).

## Model and licence

### RawNIND UtNet2, under CC BY 4.0

- **Model:** UtNet2 Bayer denoiser, checkpoint
  `DenoiserTrainingBayerToProfiledRGB_4ch_2024-02-21-bayer_ms-ssim_mgout_notrans_valeither_-4`
  (iteration 4350000), from [rawnind_jddc](https://github.com/trougnouf/rawnind_jddc)
  (also on [GitLab](https://gitlab.com/trougnouf/rawnind_jddc)), code pinned at
  `e80fb7b14dc440bba38c01f312fd3cc02deb566c`.
- **Licence, settled:** the weights are dual-licensed, GPL-3.0 or CC BY 4.0, at the
  recipient's choice. The README says "The weights are dual licensed GPL3 + CC-BY-4.0";
  the author, Benoit Brummer (`trougnouf`), stated publicly on 2026-10-14 that this means
  they can be shipped "in any software with no licensing issue so long as you mention
  the author" ([storytold/lightcraft#1760](https://github.com/storytold/lightcraft/issues/1760)).
  **RAWmakase takes them under CC BY 4.0** and redistributes them under CC BY 4.0. It
  does not take the GPL-3.0 option, so the GPL places no obligation on RAWmakase (MIT).
- **Training data:** the RawNIND dataset (UCLouvain Dataverse,
  [doi:10.14428/DVN/DEQCIM](https://doi.org/10.14428/DVN/DEQCIM)), CC BY-SA 4.0 per its
  Dataverse record. Its licensor is the same author who licensed the weights under
  CC BY 4.0. RAWmakase distributes no part of the dataset, only the author's
  weights, and credits the dataset alongside them.
- **Conversion:** we export ONNX ourselves from the upstream checkpoint (file SHA-256
  recorded) with darktable-ai's `models/rawdenoise-nind/convert.py` at a pinned
  darktable-ai commit (`--in-channels 4 --funit 32 --activation LeakyReLU --opset 20
  --height 512 --width 512 --static`; the script defaults to PReLU, so the activation
  is always passed). Running GPL tooling does not change the licence of its output;
  the weights stay the author's under CC BY 4.0. The export loads the checkpoint
  without retraining; "weights unchanged" is checked on the final artifact by
  comparing its initializers with the checkpoint's state dict, not by byte equality of
  ONNX files. Conversion is a format change, which CC BY 4.0 requires us to say we
  made. Any notices accompanying the checkpoint download are checked and carried in
  `NOTICE`.
- **Done 2026-11-02 (scratch, `~/Work/scratch/ai-denoise`):** checkpoint SHA-256
  `c1f7de3de24cb12f4803ee65e11cfb467e9a5b80c3cb7126746ef026106c15e0` (31,059,270 bytes),
  darktable-ai `76513a1e379b4265007bae768825b9a581944ff8`; our export (31,057,086 bytes)
  gives output identical to darktable's published `model_bayer.onnx` (max difference 0
  on a 512 tile).

### CC BY 4.0 obligations and where each is met

CC BY 4.0 §3(a) requires: the creator's identification, a copyright notice if
supplied, a notice referring to the licence, a disclaimer notice, a link to the
material, an indication of changes, and a link to the licence. No endorsement may be
implied, and no technical measure may restrict recipients' rights (§2(a)(5)(B); SHA-256
checks only verify bytes and restrict nothing).

The **credit text**, kept once in the manifest (`rawmakase-inference`'s
`RAWNIND.attribution`) and shown from there everywhere:

> AI Denoise uses **RawNIND** by **Benoit Brummer** (UCLouvain), model UtNet2 (Bayer),
> licensed under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/). Source:
> https://github.com/trougnouf/rawnind_jddc. Paper: Benoit Brummer and Christophe De
> Vleeschouwer, *Joint Denoising, Demosaicing, and Compression: A Recommended Paradigm
> for Learning-Based Image Workflows* (EUVIP 2026); preprint *Learning Joint Denoising,
> Demosaicing, and Compression from the Raw Natural Image Noise Dataset*,
> [arXiv:2501.08924](https://arxiv.org/abs/2501.08924). Trained on the RawNIND dataset
> ([doi:10.14428/DVN/DEQCIM](https://doi.org/10.14428/DVN/DEQCIM)). Converted by
> RAWmakase from the original PyTorch checkpoint to ONNX; the weights are unchanged.
> Provided as is, without warranties. RAWmakase is not endorsed by the authors.

Shown or shipped in:

1. **Before the download:** the install dialog shows the full credit text and links;
   nothing downloads until the user accepts.
2. **With the files:** the installed model folder holds `LICENSE` (the CC BY 4.0 text),
   `NOTICE` (the credit text) and `MODEL_CARD.md`. The installer verifies them like the
   model, so an installed model always carries its credit.
3. **In the app:** Detail panel AI Denoise info popover and the About / third-party
   notices list, both from the manifest's text.
4. **On the mirror:** `pch/rawmakase-models` gets a `rawnind-utnet2-bayer/` folder with
   the ONNX file, `LICENSE`, `NOTICE`, and a model card: upstream repository and commit,
   checkpoint name and SHA-256, darktable-ai commit and conversion command, input/output
   contract, the licence choice with both sources linked, phase 0 results, SHA-256 of
   every file.
5. **In the repository:** [dependencies](dependencies.md) lists the model, its licence
   choice and sources; release notes of the release that adds it credit RawNIND and its
   authors.
6. **In exports:** nothing; output images are the user's and carry no attribution
   requirement.

A test checks the credit is the manifest's single text, that the installer refuses a
model folder without its `LICENSE` and `NOTICE`, and that the About list includes it.

### Comparison only

LightCraft Denoise (Bayer) 0.1.0 (MIT OR Apache-2.0, CC0 training data,
[storytold/lightcraft#2929](https://github.com/storytold/lightcraft/pull/2929)) is run
in phase 0 beside RawNIND on the same test set, as a check on our measurements. It is
not shipped in this plan. RAWmakase's own model is a later option ([below](#later-our-own-model)).

### The contract (`bayerToRgb`)

- Input `[1, 4, 512, 512]`: packed `[R, G1, G2, B]` half-resolution planes,
  `(raw − black) / (white − black)` per channel, clipped to 0–1, **not** white balanced.
- RGGB alignment: the authoritative preprocessing is rawnind_jddc's
  `src/rawnind/libs/raw.py` `mono_any_to_mono_rggb` at the pinned commit, which crops
  one row and/or column on each side for GBRG, GRBG and BGGR. darktable-ai's
  `demo.py` `pack_rggb` instead stacks the planes at their original offsets, which is a
  spatial shift of one colour pair; we follow the original. To keep the full frame, we
  reflect-pad one row and/or column (which yields RGGB with the same interior samples
  as upstream's crop), run, and drop the padded output row/column. The padded image's
  origin is one packed pixel before upstream's, so tile and pooling grids differ: UtNet2
  pools 2 × 2 four times and is not shift-invariant, so outputs on the same interior
  differ slightly. That difference is measured, not required to be zero.
- Output `[1, 3, 1024, 1024]`: unbalanced camera RGB at sensor resolution (the author
  confirms camera RGB in #1760), at an arbitrary learned scale, matched by gain.
- Tiles 512, overlap and mirror padding as darktable (64 packed pixels; phase 0 checks
  32), only each tile's core kept.
- Providers: darktable pins this model to CoreML's CPU units because its activations
  overflow fp16 on the Neural Engine/GPU. Allowed providers are a manifest field.
  Measured 2026-11-02 with ONNX Runtime 1.31 (Python) on an M1 Pro: CPU 0.33 s per
  tile; CoreML ML Program on CPU-and-GPU 0.03 s per tile, finite, within 7 × 10⁻⁷
  (relative) of the CPU on three real files at ISO 12800–32000 (Sony FX3, Canon R10,
  Sony A7C), timings labelled by the configured compute units; "All" (which allows the
  Neural Engine) showed no speed advantage over CPU-and-GPU, and actual device
  placement was not verified. So
  a 24 MP photo takes about 16 s on CPU and 1.5 s on the GPU. Phase 0 repeats this with
  the app's ONNX Runtime 1.23.2 on the whole test set before CoreML is allowed.
- Linux and Windows: the bundled ONNX Runtime is the CPU build, so AI Denoise runs on
  CPU there in the first release. GPU options for later, not in this plan: the CUDA
  build of ONNX Runtime as an optional runtime (NVIDIA, CUDA and cuDNN installed by the
  user; `Provider::Cuda` exists), DirectML on Windows, or our own wgpu implementation of
  UtNet2's few operators (Vulkan, DX12, Metal; any GPU), checked against the CPU.

## Design

### Where it runs

The model replaces our demosaic for this photo. `CameraImage` (white-balanced camera RGB,
full resolution) stays the interface. Crate direction today: `rawmakase-decode` depends
on `rawmakase-engine`; `rawmakase-export` on both; the app on all.

1. `rawmakase-decode` exposes the mosaic in the model's input domain, `PackedBayer`
   (four planes, the alignment padding, the clipped-sample mask development already
   counts), and builds a `CameraImage` from model output: drop the padding, apply the
   gain, apply `Metadata::wb` exactly as `develop_mosaic` does before demosaicing,
   keep `metadata` and `scale_clipped`. No model knowledge in decode.
2. `rawmakase-inference` gains a `denoise` module: the `RAWNIND` manifest entry (files,
   tensors, tile, overlap, scale mode, allowed providers, adapter version, credit
   text), the tiler (mirror padding, core stitching, the runtime's existing run
   cancellation plus checks between tiles, non-finite check) and the gain fit. It keeps
   depending on no workspace crate.
3. `rawmakase-decode` defines `AiBase`, the narrow trait export and the app use to get
   a complete AI source: `fn get(&self, key: &AiKey, cancel: &AtomicBool) ->
   Result<Arc<CameraImage>>`. The app implements it with the inference crate.
4. **The engine never sees `AiBase`.** It receives a resolved, immutable
   `Arc<CameraImage>` snapshot inside the captured `Denoise` value, from the Develop
   session or from export. So no crate depends upward or in a cycle.
5. The app owns the service behind `AiBase`: the job queue, the model install and the
   cache.

### Highlights and the order of sources

Today `quality/samples.rs` recovers highlights on the decoded image (`recovered`, a full
image cached in the decode's `OnceLock`) and then applies `Denoise`. Highlight
reconstruction (`quality/highlights.rs`) pulls every pixel with a channel above 0.25 of
sensor white, not only clipped ones, so it depends on the pixel's own values.

- **The AI source is recovered on its own**, with the same `recovered` function, into
  its own `OnceLock`. It never reuses the plain image's recovery.
- The model's input is clipped at white, so within radius `R` of a clipped sample the
  AI source takes the plain *unrecovered* demosaic, feathered over a further `R`,
  before its own recovery (phase 0 picks `R` on clipped highlights).
- Below black: development keeps negative samples ([demosaicing](demosaic.md)); the
  model's input is clipped at zero, which raises dark means. Phase 0 measures that bias;
  if visible, the deep shadows blend toward the plain image.

### Gain

Fitted after the last tile in the **unbalanced** domain: the plain reference is our
half-size demosaic divided by `Metadata::wb` (both sides camera RGB before white
balance), recovery excluded, on 2 × 2-averaged blocks without clipped or near-black
samples. Per channel first: phase 0 requires the three gains to agree (the author says
the output is camera RGB). The stored gain is one finite, positive scalar; a degenerate
fit (no usable blocks, non-finite, non-positive) fails the job with a reason. While
tiles are still running, a provisional gain from the tiles done is used; the final gain
only rescales.

### Recipe, history and saved format

- `AiDenoise { enabled: bool, amount: f32 }` in the Detail settings, default
  `{ false, 0.5 }`. Amount survives switching off, saving and reopening. Toggling or
  moving Amount is an ordinary edit: one history step, dirty, undoable. A result
  arriving is not an edit.
- Serialised only when not default (`Effects` denies unknown fields, so older readers
  must never meet it unannounced). Any recipe with a non-default value, in the edit,
  its History or its snapshots, needs the new format in both stores:
  - saved recipe envelope: schema 13 (`saved_format::schema_for`, above Lensfun's 12);
  - catalog: version 5 (`catalog::format_upgrade::Stores` gains `AiDenoise`, above
    `Lensfun`, with the same backup-and-upgrade path; History and snapshot writes
    already go through `Stores::of`), so releases that open version 4 refuse it
    instead of rendering without denoise.
- The recipe does not name a model: every edit renders with the installed model
  ([single engine](rendering-quality.md)); a model change invalidates cached results.
- Copy/Sync and Reset: AI Denoise belongs to the Detail group.

### Rendering and identity

The captured `develop::recipe::Denoise` gains the AI source snapshot and Amount. The
base is `plain_recovered + amount × (ai_recovered − plain_recovered)` (at Amount 100,
`ai_recovered` itself; at 0, or off, the plain path unchanged), then colour and luminance
noise reduction as today. Every publication (partial, final) is a new `Arc` with its
own generation; render jobs capture the snapshot they start with; the retouch cache and
the GPU resident upload (keyed by source pointer) see a new source; the session drops a
result for a photo no longer open before publishing it.

Without a ready result (computing, model not installed, runtime missing, failed), the
preview renders the plain image, with the state shown beside the switch. Export is
stricter (below).

Whether Camera Raw changes manual noise reduction when Denoise is on is measured on
synthetic DNGs in phase 0 and followed.

### The job

- `AiKey`, captured once: the decode cache's file identity, the model file's SHA-256,
  the adapter version, the provider and its options. It runs the job and names the
  cache entry.
- One service, one queue: Develop, prefetch and export ask the same service; a request
  for a key already running waits for that job (coalesced). One job runs at a time, on
  its own thread, with a bounded ONNX thread count, below rendering in priority. A
  queued Develop request for a photo that no longer has AI Denoise on is dropped; a
  running job finishes and stores.
- Viewport first: at 100% the tiles covering the visible region run first; one partial
  snapshot (computed tiles over the plain image) is published after them, then the
  final one. At most two publications per job.
- **GPU failure:** a non-finite tile on a GPU provider discards the whole attempt. The
  job restarts on CPU under a new `AiKey` with the CPU provider; coalesced waiters move
  to it; a published partial snapshot is superseded by the CPU run's; the GPU provider
  is not used again in the session. A cached result therefore always comes from one
  provider. Test: failure after several good tiles, then a cache hit under the CPU key.
- A result is rejected if the file's identity changed (replaced, deleted) or the model
  was removed or changed during the job. Failure, panic, cancellation and provider
  error clear the busy state and report.
- Shutdown follows [the contract](shutdown.md): the stop flag and the runtime's run
  cancellation are raised, the worker is handed to `task::Stopping` and joined only
  under the shared deadline, then detached; the thread owns its ONNX session, never
  unloaded under a running call. Cancellation latency (normally under one tile) is
  measured and documented separately from that hard bound. A test uses a fake model
  run that never returns and checks quitting is not held past the deadline.

### Cache

Its own payload, versioned, in its own namespace: header (format version, `AiKey`,
width, height, alignment padding, the gain applied, as f32) and the model's output
**with the gain applied**, in unbalanced camera RGB before white balance (the same
scale as the normalised mosaic, sensor white at 1), at half precision (6 bytes/pixel:
about 150 MB at 24 MP, 370 MB at 61 MP). The raw model output cannot be stored in half
precision: its scale is arbitrary and measured medians reach about 7.7 × 10⁴ (Canon
R10) and 1.3 × 10⁵ (Sony A7C), beyond half precision's 65,504. Every value is checked
finite after conversion when storing (a non-finite value fails the job) and on load.
Round-trip tests use those measured ranges. Loading builds the `CameraImage`
with the decode crate's assembly. It reuses the decode cache's size limit, eviction,
purge and relocation code but not its entry format, under its own subfolder in both
the default and a chosen cache folder (a separate `Dir` name, so relocation keeps the
namespaces apart) and its own limit (default 10 GB), so ordinary decodes never evict
results that took a minute. Cache-hit and fresh results differ only by half-precision
rounding; tests bound that difference. Saved edits never need the cache.

### Memory and interactivity bounds

Resident per open photo today: plain and its recovered image (24 bytes/pixel). With AI
Denoise: plus the AI source's recovered image and, for 0 < Amount < 100, the blend
(the unrecovered AI source is released once recovered; it can be rebuilt from the
cache): 48 bytes/pixel, about +0.6 GB at 24 MP and +1.5 GB at 61 MP before manual noise
reduction and retouching. Step 1 measures peak memory and, on an M1 Pro, the time from
an Amount change to the updated Fit preview and 100% region at 24 and 61 MP, and the
time to the first partial result. Acceptance bounds before step 2: Amount to updated
preview ≤ 300 ms at 24 MP, ≤ 1 s at 61 MP; peak memory ≤ current + 0.8 GB at 24 MP and
+ 2 GB at 61 MP. If missed, Amount blends the preview pyramid while dragging and the
full image on release, or the recovered AI source is held at half precision; the plan
is revised before step 2.

### Export

Batch, single-photo and headless exports (`rawmakase-export`'s `batch::render` and
`job`) receive the app's `AiBase`, get a complete (never partial) AI source for a photo
with AI Denoise on (cached, coalesced with a running job, or computed in the export's
cancellable progress), and pass the snapshot into the engine. If it cannot be had
(model or runtime missing, inference failed, unsupported sensor), that photo's export
fails with the reason; it never writes the plain image instead. A batch reports such
photos and carries on. Callers without a service pass one that refuses, so the rule
holds.

### Model installation

A third model pack, `Pack::Denoise`, in the existing installer (`subject_mask/models.rs`):
explicit download from the pinned `pch/rawmakase-models` commit (fallback: the same
bytes as a release asset of `pch/rawmakase`) with size, progress, cancel, retry,
verify-then-publish and remove; offline import of the pinned files. The credit and
licence are shown and accepted first; `LICENSE` and `NOTICE` install with the model. It
never fetches the selection models or the other way round.

### UI

Detail panel, above manual noise reduction: **AI Denoise** switch, **Amount**, an ⓘ
popover with the credit, and a status line (Install model…, Denoising 40%, Ready, the
error, or why the photo can't use it). The existing before/after and 100% view are the
comparison #753 asks for.

## Phases

### Phase 0 — measure (scratch, first step of the PR's work)

Python with onnxruntime on our RawNIND export (and darktable's, to compare), LightCraft
0.1.0 alongside; then the Rust adapter checked against the Python reference. File
lists fixed and recorded before measuring: CC0 high-ISO Bayer files from the pixls
archive (45 at ISO > 1600 across 28 cameras, plus the ISO 401–1600 set), CC0 X-Trans
files, synthetic DNGs with a noise profile (Camera Raw Denoise renders for comparison),
and RawNIND's held-out test pairs (Bayer and X-Trans).

Quality metric domain (item 5), as in #1760's X-Trans comparison: output to linear
Rec.2020, a per-channel least-squares gain onto the reference, clip, sRGB encode,
MS-SSIM (5 scales) and PSNR on a fixed interior crop; reported as mean and median over
the set and per camera.

**Comparison transform** (items 1 and 3), one fixed transform shared by both outputs of
a pair, never fitted per output: the reference output of the pair (upstream-cropped
alignment; overlap 64) sets one scalar gain (its unbalanced gain fit) and one exposure
(`0.18 / median green` after white balance); both outputs get that gain, the file's
`Metadata::wb`, the camera-to-sRGB matrix and that exposure, then sRGB encoding as
floating point clipped to [0, 1]. Both are compared on the same sensor coordinates: the
frame minus 64 sensor pixels at every edge. Statistics per pair: mean absolute
difference over all channels (MAE) and the 99.9th percentile of absolute difference.

1. Contract.
   - Exact: packing, channel order and the aligned interior samples equal
     rawnind_jddc's preprocessing bit for bit, for all four phases.
   - Identical tensors: Rust (the app's ONNX Runtime 1.23.2, CPU) and Python
     (`onnxruntime==1.23.2`, CPU) on the same input tile give
     `max |a − b| ≤ 1e-5 × max |b|` and mean `|a − b| ≤ 1e-6 × max |b|` per tile;
     stitched images from the same tile origins likewise.
   - Our ONNX export equals darktable's (done: maximum difference 0 on a 512 tile);
     its initializers equal the checkpoint's state dict exactly.
   - Measured with tolerances, each pair separately, with the comparison transform:
     padded vs upstream-cropped alignment for each of GRBG, GBRG and BGGR on every
     phase 0 Bayer file of that pattern (the network is not shift-invariant, and the
     grids differ by one packed pixel); and, for one synthetic scene mosaicked in each
     of the four phases, each phase against RGGB (each phase also samples different
     colours). Accept a pair at MAE ≤ 0.005 and 99.9th percentile ≤ 0.05. Visual
     inspection at 100% for structure (grids, seams, colour blotches) is a separate
     check that must also pass.
2. Output space and gain in the unbalanced domain; a synthetic case with white balance
   (2, 1, 1.5) and a known gain. First real files: per-channel gains agree within 1.5–2.4%
   on two Sony files but 14% apart on a Canon R10 (ISO 32000); explain before fixing the
   gain rule (one scalar vs per channel).
3. Seams: overlap 32 against overlap 64 with the comparison transform; the MAE in a
   4-pixel band across tile borders against the MAE elsewhere. Overlap 32 is used only
   if the band's MAE is ≤ 0.002 and ≤ 1.5 × elsewhere, and no seam is visible at 100%.
   (The first scratch numbers, 20.3 vs 3.5, are raw model-output units, not this
   measure.)
4. Highlights (choice of `R`) and dark-area bias.
5. Quality: metrics on the RawNIND test pairs; 100% side-by-sides against our manual
   noise reduction and Camera Raw's Denoise; false colour on fine detail.
6. X-Trans: rebuild the mosaic as RGGB of the same size, then the Bayer model
   (LightCraft #1828's approach, implemented ourselves); metrics on RawNIND's X-Trans
   pairs and side-by-sides on CC0 Fuji files; maze or grid artefacts disqualify it.
7. Providers: CPU, CoreML (CPU-and-GPU), with the app's ONNX Runtime 1.23.2: finite
   output on every test file, difference to CPU, time per tile, 24 and 61 MP totals,
   peak memory.

Go/no-go: no non-finite output on CPU; seams invisible at 100%; contract and gain
understood; on the RawNIND test pairs, MS-SSIM above manual noise reduction's on at
least 80% of images; preferred over manual noise reduction in the maintainer's blind
side-by-sides on most of the CC0 high-ISO files; a 24 MP photo in under about 2
minutes on CPU on an M1 Pro. X-Trans is in the first release only if it also passes
item 6. Otherwise stop and report.

### One PR, built in this order

Phase 0's results are recorded in this file before the steps that depend on them
(`R`, the gain rule, overlap, CoreML). The whole feature lands as **one PR**; the steps
below are its order of work and its commits, and the memory and interactivity bounds
are measured after step 1, before step 2 starts.

**Step 1 — adapter, service, cache and the rendering path.** `PackedBayer`, assembly
and `AiBase` in decode; `denoise` and the `RAWNIND` manifest in inference; the service,
cache and `Pack::Denoise` with `LICENSE`/`NOTICE` in the app; the mirror folder with
model card; a dev command `rawmakase denoise <file>`. Enough of the rendering path to
measure it as it will run: the AI snapshot and Amount in the captured `Denoise`, the
blend, snapshot identity in the retouch cache and GPU upload, and viewport-first
publication, driven by a development-only switch (no saved setting yet). Then the
memory and interactivity measurements above.

**Step 2 — recipe, export and UI.** `AiDenoise` with schema 13 and catalog version 5
(replacing the development switch), export through `AiBase`, the Detail panel with the
credit popover and status line, the About entry, docs.

**Step 3 — Lightroom import and Sync.**

First capture fixtures: apply Denoise in Camera Raw to synthetic DNGs (alone, with Raw
Details, with Super Resolution, at several amounts, then off again) and save their XMP;
record what `crs:Denoise`, `DenoiseLumaAmount`, `Details` and manual noise reduction
hold. Then map: Denoise on → `AiDenoise { enabled, amount }` per the fixtures; Raw
Details or Super Resolution, a missing or malformed amount, or an unsupported sensor
stay blockers. Copy/Sync and batch queueing across many photos.

### Later: our own model

Optional, after the first release, for speed (an fp16-safe network that can use
reduced precision on CoreML's GPU and Neural Engine) and a noise-level input. Data on hand
(2026-11-02, `dataset.sqlite`): 1,374 CC0 Bayer files at ISO ≤ 400 from 712 camera
models (14.4 gigapixels) for clean targets, 62 CC0 X-Trans; only 45 CC0 Bayer files above
ISO 1600, enough to calibrate synthetic noise and test, not to train on real noise.
The author of RawNIND warns that models trained only on synthetic noise often fail on
real photos, so this starts only with real noisy/clean tripod pairs the maintainer
shoots and owns. Acceptance is the phase 0 protocol with fixed, disjoint training,
calibration, validation and test file lists, plus: mean MS-SSIM no lower than RawNIND's
by more than 0.001, no camera lower by more than 0.005, preferred or tied in the
maintainer's blind side-by-sides, and fp16 checked on the real providers.

## Tests

Download-free unit and integration tests; real-model tests run separately (as the
selection models' are).

- Packing and alignment padding for all four Bayer phases on synthetic mosaics with
  known values; the aligned interior samples equal upstream's crop exactly; round trip
  through assembly; white balance applied once. (Model output across phases is a
  measured quality check, not a unit test.)
- Tiler with a fake model (nearest-neighbour upsample): stitched output equals the
  whole-image run; edges and non-multiple sizes; cancel during and between tiles.
- Gain: unequal white-balance multipliers with a known gain; provisional then final
  gain changes only the scale; degenerate fits fail with a reason.
- Highlights: the AI source is recovered on its own; an unclipped pixel with unequal
  channels above 0.25 of white (for example plain 0.8/0.5/0.3, AI 0.78/0.48/0.32) gets
  its own recovery, not the plain one's; within `R` of clipped samples the AI source is
  the plain unrecovered image.
- Cache: identity changes with file identity, model digest, adapter version, provider;
  payload round trip within half-precision bounds; non-finite or truncated entries
  rejected; relocation keeps namespaces apart; decodes never evict it.
- Lifecycle: replaced/deleted file and removed model reject the result; failure and
  panic clear busy state; coalesced requests run one job; GPU non-finite after good
  tiles restarts on CPU under the CPU key and later hits that key; a non-returning fake
  run does not hold quitting past the deadline; a queued request is dropped when the
  switch goes off; a result for a photo no longer open is not published; partial →
  final publication invalidates the retouch cache and GPU upload.
- Recipe: switch and Amount are history steps, a result is not; Amount survives off,
  save and reopen; schema 13 and catalog version 5 only with non-default values,
  including History and snapshots; the previous readers refuse them.
- Export: cached result used; running job coalesced; missing model or runtime fails
  that photo with the reason, the batch continues; never the plain or a partial image;
  the refusing service fails closed.
- Credit: one manifest text; install refuses a folder without `LICENSE` and `NOTICE`;
  the About list and the popover show it.
- XMP (step 3): the captured fixtures.
- Rendering: AI Denoise off renders bit-identically to today (no reference changes);
  Amount 0 equals off.

Before the PR: the CI checks, the packaged-runtime smoke test extended to this model,
and quick dataset crops (AI Denoise off cannot change the gate).

## Docs to update when built

[architecture](architecture.md) and [code map](code-map.md) (`AiBase`, the service, the
snapshot in `Denoise`), [catalog format](catalogs.md) (version 5),
[rendering quality](rendering-quality.md), [parity gaps](parity-gaps.md),
[xmp presets](xmp-presets.md), [shutdown](shutdown.md),
[dependencies](dependencies.md) (RawNIND, CC BY 4.0, credit and sources).
