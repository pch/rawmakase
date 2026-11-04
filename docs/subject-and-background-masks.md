# Subject and Background masks

Implementation plan, 2026-10-08. Based on source revision `5e62b17`, the current
[architecture](architecture.md), [code map](code-map.md), [masking](masking.md),
[catalog format](catalogs.md) and [shutdown contract](shutdown.md). This describes
the plan; see the implementation notes at the end for what was built and where it differs.

## Outcome and scope

Add **Select Subject** and **Select Background** to the existing Masking tool.
Selection runs locally, produces a soft raster mask, and supports the same local
adjustments, composition, overlay, history and export as existing masks.
Background is the complement of the detected subject coverage. Users can correct
either selection with Add/Subtract brushes and intersect it with existing ranges
or gradients. Saved results work offline without a model or inference runtime.

Ship this complete workflow first. Implementing all the work in
the preliminary research (never committed) would not simplify it: removal,
depth, denoise and tagging each need different models, saved representations and
rendering contracts. Share the raster and inference foundations, then extend them
in the order described at the end of this plan.

The first release includes one evaluated automatic-subject model, explicit model
installation, CPU inference, regeneration, manual cleanup and packaged runtime
validation. General grow/shrink and feather controls are a follow-up; usable soft
edges are a first-release requirement. Do not ship extra controls to compensate
for a model that fails the photographic acceptance gate.

## Research and current implementation

Computer inspection of the installed applications confirmed that RAWmakase has a
Masking drawer beneath the histogram, a five-tool Create row and Show Overlay.
Lightroom's Masking entry presents Subject and Background as named creation
actions alongside manual tools. These observations inform placement and naming;
they do not establish model-quality or local-adjustment parity. No reference
selection was generated and no private photo is included in this plan.

The old research contains useful constraints but predates the current crate
split and catalog-only editing:

| Current owner | Existing behavior | Required extension |
| --- | --- | --- |
| `crates/rawmakase-model/src/model/masks.rs` | Five shapes; 16 groups, 32 components per group; no persistent mask IDs | Validated raster reference and generation provenance |
| `crates/rawmakase-engine/src/develop/masks/eval.rs` | Geometry-aware sampling and ordered Add/Subtract/Intersect | Raster sampling using the same image-space mapping |
| `crates/rawmakase-engine/src/develop/stage_cache.rs` | Region weights reusable when local sliders change | Include raster identity and sampling version in keys |
| `crates/rawmakase-model/src/storage/bitmaps.rs` | Bounded compressed rasters; two FNV hashes form the content ID | Checked arithmetic, validated mask format and immutable decoded assets |
| `crates/rawmakase-catalog/src/catalog/edits.rs` | Bitmap insertion; retrieval is test-only | Production batch retrieval and asset-aware saves |
| `crates/rawmakase-catalog/src/catalog/edit_rows.rs` | Authoritative edit/history writes | Save raster dependencies with references in one transaction |
| `src/edit_session/mod.rs` | Edits, history and dirty state coordinated by session operations | Atomic installation of generated masks and their in-memory assets |
| `src/app/develop/tools/mask_tool.rs` | Creation, composition, corrections, sliders, overlay | Generation controls and result application |
| `src/app/task.rs`, `src/app/worker/latest.rs` | Cancellation, generations and bounded worker patterns | Dedicated inference lifecycle using these patterns |
| `crates/rawmakase-export/src/export/job.rs` | Captured image and recipe for export | Capture referenced asset IDs and unsaved leases; resolve before rendering |

Direct-file companion writing is no longer a feature prerequisite. Keep legacy
sidecar ingestion and its preservation rules working; do not revive a second
authoritative save path. Keep the existing local-adjustment math and packed GPU
weight transport. Neither requires a new color shader for a raster selection.
Internal/test and legacy paths still support a shown photo without a catalog.
Disable generated-mask creation and regeneration there, both in the UI and the
session/command boundary, with **Add this photo to a catalog to use AI masks**.
Do not download a model or retain session-only generated masks through that path.

## User workflow

1. Open Masking with Shift+W. Show two labeled actions, **Select Subject** and
   **Select Background**, above the existing manual-tool row. Give these actions
   accessible names, keyboard focus and visible disabled-state explanations.
2. When the user requests a feature whose model is absent, show its verified
   download size, disk requirement and **Download** / **Import model…** actions.
   Download only after the user chooses Download. State that processing happens
   on this device. No model download starts at installation, startup, catalog
   opening, ordinary editing or merely opening the Masking drawer. Dismissing the
   prompt leaves the feature uninstalled and the photo unchanged.
3. Once installed, clicking an action shows **Selecting subject…** or
   **Selecting background…** and **Cancel** in the drawer. Keep zoom, pan and
   navigation responsive. Pending work is UI state, not a saved empty mask.
4. Success creates and selects one named group and shows its overlay. It is one
   history step, with neutral local adjustments. Local sliders behave as usual.
   Do not steal focus if the user has left the tool.
5. Include Subject and Background in the component Add/Subtract/Intersect menus.
   Check group/component limits at submission and again before committing.
6. **Regenerate** replaces only the selected generated component's raster and
   provenance. Preserve its inversion, opacity, operation and the group's other
   components and adjustments. A failed regeneration leaves the old result intact.
7. Cleanup uses ordinary Add or Subtract brush components. Brush Erase continues
   to erase brush strokes; it does not mutate the generated raster.

Keep errors local to the drawer with a retry action: model missing, installation
failure, runtime unavailable, insufficient resources, invalid output and stale
request. Cancellation/failure records no edit. Do not show a fabricated percentage
during a runtime call that provides no progress.

Subject means the model's salient foreground, possibly several disconnected
subjects; it is not a guaranteed selection of one person. Handle an empty result
as **No subject found**, without creating a Background that affects the entire
photo. Exact all-zero coverage is empty. Soft nonzero output needs a calibrated
no-subject policy: phase 0 may evaluate coverage/area thresholds, but must measure
false rejection of small subjects and cannot label raw alpha as confidence.
Do not ship an unvalidated arbitrary threshold. Keep this policy model-versioned.

## Ownership and APIs

Keep the model and renderer usable without inference installed:

- **Model:** raster reference, sampling version, provenance values, validation and
  asset-reference enumeration. A proposed `MaskShape::Bitmap` references immutable
  coverage; dimensions/sample format are validated against the resolved asset.
  Provenance is not necessary to render or required to be understood by inference.
  Shared raster-budget and allocation-lease types also live in the model crate:
  pure memory/resource values with no catalog, engine, UI or inference dependency.
- **Engine:** model-input rendering, raster sampling, overlay weights and optional
  future edge operators. Accept an immutable, validated `MaskAssets` value through
  render inputs; never read the catalog or install models from a pixel evaluator.
- **Catalog:** compressed blobs, batch resolution and transactional persistence.
  Resolution checks all references and produces shared decoded data. No renderer
  dependency and no runtime dependency.
- **Edit session:** a single operation accepts validated generated coverage and
  the intended edit, registers assets, validates the resulting recipe and commits
  exactly one history/dirty-state change. Do not coordinate this by mutating three
  independent Editor fields. The session retains unsaved assets under a byte
  budget; saved history holds asset IDs, not every decoded raster. Session APIs
  enforce resolvable references and consume catalog-scoped persistence receipts.
- **Proposed `crates/rawmakase-inference`:** a small concrete ONNX integration with
  model manifest, tensor preprocessing/postprocessing and inference session. It
  accepts owned RGB input and returns coverage or a typed error. Depends on the
  model crate and runtime, never the app, catalog, GUI, RAW decoders or rendering engine.
- **App:** model installation I/O, worker scheduling, status and guarded application
  through the edit session. Compose decode, engine input rendering and inference
  here. Start with one model implementation, not a generic plugin framework.

Capture a recipe and its enumerated asset IDs together. An app/catalog job also
carries its catalog identity/location and any unsaved asset leases. A worker may
resolve committed IDs later on its own catalog connection: immutable blobs are
never replaced or garbage-collected in this release. It must not re-read a newer
recipe. Resolve and validate before invoking the engine; pass only `MaskAssets`
to the engine/export renderer. Preserve catalog replacement/source identity guards
when opening a connection, and fail if captured IDs no longer resolve.

Audit these callers explicitly; replacing `EditSource::Recipe(String)` with an
asset-aware job source is necessary for Library workers:

| Path | Required behavior |
| --- | --- |
| Develop After/Before, fit/regions, overlay and Reference View | Resolve the particular displayed recipe, including historical Before states |
| Library thumbnails and edited previews (`src/app/library/previews/`) | Carry catalog identity and captured IDs; never substitute Defaults on an asset error |
| Library Loupe (`src/app/library/stage.rs`, `src/app/library/screen.rs`) | Preserve typed asset errors through source selection and rendering |
| Build Standard/1:1 Previews (`src/app/library/preview_build.rs`) | Resolve captured saved assets and report per-photo failure without publishing an incomplete preview |
| Snapshot/history restore | Install recipe/IDs and history synchronously; resolve render assets asynchronously under the new revision |
| Foreground/queued export, command outputs and headless CLI | Capture recipe/IDs together, lease unsaved assets and fail before output publication on missing assets |
| Masking color sampler (`src/app/develop/tools/mask_tool.rs::developed_lab`) | Continue clearing masks deliberately; pass an empty asset set and test absence of local-mask feedback |

An asset-free render entry point must reject a recipe with unresolved bitmap
references. Share decoded bytes using budgeted `Arc` leases, not per-worker copies.
Audit all render entry-point callers beyond this table during phase 2.

Introduce a typed missing/corrupt-mask-asset state distinct from the existing
protected-edit fallback. In Develop, preserve recipe/history, block mutation and
autosave, and show an explicit error with Retry. The last valid frame may remain
only with a stale/unavailable indication; on initial open use an unavailable-image
state rather than presenting defaults as the saved edit. Reference/Loupe/Library
follow the same rule, with no successful new preview cache entry. All export paths
refuse that state, even though some existing protected edits can be exported.
Do not broaden this change to unrelated legacy fallback behavior. Propagate the
typed state through `Library::develop_source`, `EditSource` and the load events;
an `Err -> None -> Defaults` conversion must not erase it.

Invalidate the previous render generation immediately when a recipe is installed,
even if its new assets are pending and no replacement render can be submitted yet.
Every rendered frame/thumbnail carries the captured recipe revision and cache tag
(including raster IDs), derived from its actual inputs. `refresh_library_thumbnail`
must consume that captured tag instead of tagging old pixels with the current
document recipe. Reject obsolete results before display or Library/cache publication;
asset-resolution errors cannot leave an old render eligible as the new edit.

Update architecture, code map and dependency/closure checks when these boundaries
are implemented. Keep existing forbidden dependency closures intact and add a
check that model, catalog and renderer cannot reach the inference runtime.

## Raster and rendering contract

Use single-channel unsigned 8-bit coverage, where 0 is unselected and 255 is
selected; keep model output in float until final conversion. Bilinear sampling
returns fractional coverage before composition and the existing GPU weight
quantization. Start with an upper bound of 4096 pixels on either axis and 16 MiB
decoded per selection. Store meaningful output resolution, not an unconditionally
enlarged 4096 image. Phase 0 must compare composited edges against float output;
adopt 16-bit storage only if visible errors justify its extra cost, revising the
format and budgets explicitly before implementation.

For the first variant, the raster covers exactly the camera-oriented default-crop
`ImageFrame`, normalized to `[0,1]²`, before lens distortion correction, Transform,
user crop, straighten, rotation and flips. Do not introduce arbitrary raster
placement until a feature needs it. Store a version for this sampling contract.

Sample bilinearly at pixel centers (`x = u * width - 0.5`, similarly for y), clamp
the interpolation footprint at the raster edge for points inside the frame, and
make the raster component's contribution zero outside that frame, including when
the component is inverted: `inside_frame * (invert ? 1 - sample : sample)` before
opacity. This guard belongs to the raster component, not the whole group. Keep
existing analytic-shape composition and final group inversion unchanged; an
explicit group inversion still complements the composed group under the existing
semantics. Initial Background uses component inversion only. Retain the renderer's
existing out-of-geometry rejection. Test mixed raster/analytic groups and the
difference between out-of-frame raster support and out-of-geometry pixels.

Compose coverage in the existing order: component inversion, component opacity,
Add=max / Subtract=saturating subtraction / Intersect=min, then group inversion.
Initial Background is an inverted Subject **component**, not an inverted group.
Thus adding a brush to Background still adds coverage in the expected way.

Subject and Background generated from the same input may share one asset;
regeneration changes only the requested component. They are independent edits,
not live-linked groups. Before manual corrections, their coverage sums to one
within the valid frame, within the quantization tolerance.

Cache identity includes immutable asset ID, sampling version, shape/composition,
frame and output mapping. Adjustment-only edits reuse weights. Replacing the
bitmap invalidates weights even if dimensions do not change. Keep render-cache
and inference-cache identities distinct. Never regenerate during rendering.

## Persistence and compatibility

Extend checked catalog saves with an optional batch of **new, unconfirmed assets**,
not decoded copies of all current/history assets. Validate/hash/compress each new
asset once off the UI thread; share the prepared immutable payload across retries.
Generation completes validation, hashing and compression before the session accepts
its result; a later synchronous flush/snapshot write never prepares pixel payloads.
In one `Db::write`, insert that batch, compare content on an unexpected existing-ID
conflict, verify that every ID in the new current edit and replacement history
exists, and write edit/history rows through `edit_rows`. Already committed IDs need
only batched existence checks, not decompression and rehashing on every autosave.
`HistoryUpdate::Keep` leaves retained history untouched; do not parse/rewrite an
unknown retained format just to save a slider. With no bitmap GC, its references
remain retained. Full blob validation happens on resolution, not every save.

Sync without Masking, Sync Undo and Library catalog Undo can pass no new assets
when they only reference blobs already committed in that catalog. Check reference
sets for each batch member within the same transaction and preserve all-or-none
save behavior. Receipts identify catalog instance, committed IDs and saved edit
revision: an old acknowledgement may mark those exact blobs durable, but cannot
clear a newer dirty edit or mark blobs durable in a different/replaced catalog.

Snapshot add/update has a separate write path in `catalog/snapshots.rs`. Give it
the same new-asset batch and reference-check helper, atomically with the snapshot
row; it does not route recipe writes through `edit_rows`. Keep add/update synchronous
through the existing snapshot action for this release, using the prepared payload
and the catalog's existing busy timeout. Start rename only after a successful insert
returns its row ID, and display failure before accepting navigation/quit input.
This retains existing database-I/O blocking behavior; moving it off-thread later
requires a separate lifecycle change covering pending rows, scoped completions and
navigation/close flushing. Snapshot failure leaves the previous row
intact and unsaved assets available for retry. Snapshot success confirms only
its blobs/row, not that the current edit/history was saved. An immediate snapshot
after generation must survive even if the subsequent autosave fails.

Use namespaced SHA-256 IDs for newly generated raster assets, hashing a canonical
header (dimensions/channels/sample depth) and uncompressed samples. Continue
reading and preserving existing FNV IDs and imported blobs; do not rename them.
The reference type/validator accepts the defined legacy and new ID formats and
retrieval validates with the appropriate algorithm. No existing saved mask shape
needs conversion. Hashes for model files remain independent artifact SHA-256s.

Production retrieval must bound compressed/decompressed sizes, use checked
dimension multiplication, validate channels/sample depth and verify content IDs.
Validate both shape metadata and blob metadata. Return explicit missing/corrupt
asset errors before rendering, including for inverted components. A known missing
or corrupt asset protects the edit from autosave and blocks exports that would
silently omit the mask. A rendering memory shortage is a separate resource error,
not evidence of damaged persistence. Do not substitute all-zero coverage or a
default recipe.

Save failure keeps the complete in-memory edit and assets pending for retry.
Autosave acknowledgements clear only the revision they saved. Navigation and
close must wait for the asset-aware save under the existing deadline and failure
policy. Successful inference is not a successful save.

**Downgrade protection:** use catalog format version 2 for raster-capable catalogs.
Newly created/imported catalogs use version 2; opening an existing version 1
catalog leaves it at 1 until the user enables this feature. Before generation,
offer **Upgrade catalog…**, explaining older-release incompatibility, required
backup space and that the catalog must be closed in other app instances/computers.
This follows the existing one-computer-at-a-time catalog contract.

Flush pending writes successfully, pause catalog-mutating jobs/commands, create
and verify a consistent version 1 backup through `catalog::db`, then upgrade the
original catalog in place in an immediate transaction. Use SQLite's backup API,
not a live filesystem copy; the primary catalog currently uses rollback journaling.
Publish the backup without clobbering any existing file before committing the
upgrade. Bound worker I/O waits using the existing task/close rules. Backup/upgrade
failure starts no generation and preserves version 1; if the upgrade committed
but its acknowledgement was lost, re-read its version and treat it as upgraded.
Release paused jobs only after the outcome is known. Keep the catalog path and
photo IDs stable so preview requests, folder locations and Library undo survive.

Require compatible app versions on every computer using the upgraded catalog.
An old process with a connection opened before upgrading cannot be retroactively
protected by a version check or a temporary SQLite write lock. The UI must state
the close-other-instances precondition; do not claim mixed-version concurrent
access is safe. New-version connections recheck supported format at writes.
Keep the backup clearly named as a pre-upgrade recovery copy, not a second active
catalog; its size can be substantial because imported source archives are included.

Version 2 remains after mask deletion to protect history-only and snapshot-only
references. Older apps refuse it on open. Test prior-release refusal, backup
failure, transaction rollback, preserved path-based preview requests and fresh
catalog creation. No automatic downgrade or catalog switch is part of enabling
masking. Keep migration/version control inside `catalog::db` and update the
catalog-format documentation with this behavior when implemented.
Document the version 2 default for newly created/imported catalogs in the creation
UI, catalog guide and release notes, explicitly replacing the older-version
compatibility promise for those catalogs even if no AI mask is ever created.

Keep old shape serialization unchanged and preserve recipe unknown fields and
unreadable source records. Native preset/legacy envelopes use their existing
version machinery, while catalog records currently store bare recipe/local JSON;
a `saved_format::SCHEMA` bump alone cannot protect catalog data. Native presets
containing raster selections are rejected in this first release, before writing
a file, with guidance to exclude Masking. Do not write a preset with dangling IDs.

Retain blobs referenced by current edits, undo/redo, persisted history, snapshots,
virtual copies and in-flight save/render/export jobs. First release performs no
automatic catalog bitmap garbage collection. This deliberately trades disk growth
for recoverability; show it as a known limitation. A future explicit compaction
must trace every root and abort on formats it cannot understand. Bound decoded
caches by bytes independently of disk retention.

### Raster memory ownership and limits

Initial application limits (phase 0 may revise them before shipping):

| Resource | Limit and behavior |
| --- | --- |
| One raster | 4096 pixels per axis, 16 MiB decoded for 8-bit coverage |
| Generation/regeneration or transfer introducing raster references | 256 MiB of distinct referenced rasters in the resulting edit, counting hidden/neutral masks; shared IDs count once. This admission limit never blocks catalog saves or restoration of existing reference sets |
| Unsaved assets retained by the session | 256 MiB across current edit, history and temporary Before states; refuse another generated edit when this cannot fit, preserving the previous state and offering save/retry |
| Process-wide raster allocations | 512 MiB, including cache, unsaved data, staging buffers and live render/export leases; reserve before allocation, evict unleased cache entries first, then report a resource-limit error if the request cannot fit |

Instantiate one shared model-crate raster allocation budget in the app and pass
it to catalog resolution, preview workers and the export queue. The headless CLI
does the same. `rawmakase-export::export::batch` receives that handle and catalog
asset source with its captured `Edit::Catalog` jobs; it can resolve IDs when each
photo runs without reaching into app state or reversing dependency direction.
Pass leases to workers rather than independent per-worker cache allowances.
Compression/decompression temporary buffers count too. Headless callers initialize
an equivalent budget. Reserve the full additional working set needed for a job
before starting so workers cannot deadlock holding partial reservations. A failed
reservation is a visible, retryable resource error; never wait indefinitely for
document/history leases to disappear. Library jobs may retry after a budget-release
notification, without a busy loop; exports report failure for the affected photo.
Inference tensors, decoded RAWs, GPU buffers and region weights are separate
budgets; phase 0 measures their combined peak, not just mask storage.

After a catalog receipt, release the session's unsaved ownership of those blobs;
current renders may retain bounded leases, but history/snapshot metadata keeps
IDs and resolves lazily on restore. Unsaved history assets cannot be evicted before
they are durable. Discard unreachable unsaved assets only after checking session
history, temporary Before, snapshot/save jobs and output leases. A save failure
must not trigger eviction of the only copy.

Loading metadata does not reject or rewrite an existing edit merely because its
raster working set exceeds the local budget. Preserve it and expose a resource
error if it cannot be rendered within the budget. Do not silently omit masks or
resolve every historical state eagerly. Preserve the existing synchronous Undo log:
Undo/Redo, History clicks and Apply Snapshot install recipe/IDs and move their
history/log cursors together through existing session APIs. A successful command
means the edit state changed, not that its new pixels have rendered or saved.
Then resolve assets off-thread under the new document/render revision. Rapid Undo/
Redo supersedes only asset/render work, never a half-applied history command.

Gate saves/snapshot writes on persistence, not on rendering: every referenced ID
must be supplied by a validated prepared unsaved asset or pass the in-transaction
catalog existence check. Pending render resolution and raster-memory shortages
do not block ordinary edits, saving, snapshots or navigation/close. A permanently
over-budget edit can still be saved with its assets intact. Navigation/close uses
the ordinary save/deadline policy without waiting for raster decoding.

If resolution reports a current missing/corrupt asset, retain the installed
recipe/history and enter the protected unavailable state; block ordinary edits
and saves, allow Retry or a further Undo/Redo to a healthy state, and never roll
back just one cursor. A resource-limit error instead affects rendering/export
only and offers Retry when resources become available. Ignore failures from
superseded revisions. This keeps automation's synchronous history-result contract
unchanged while preventing render-memory pressure from becoming a save failure.

### Copy, Sync, snapshots and import

| Operation | First-release policy |
| --- | --- |
| Duplicate group / virtual copy of the same photo | Share immutable assets; edits and regeneration remain independent |
| Snapshot add/update | Commit newly needed assets and the row together, independently of pending autosave |
| Snapshot and history restore | Install recipe/IDs and history synchronously; resolve assets lazily; report current failure without desynchronizing the Undo log |
| Copy/Paste or Sync Masking to a different original | Reject raster-bearing Masking transfer with an explicit explanation; permit other selected settings only after the user excludes Masking |
| Preset save/update/import/application | Enforce the raster restriction at the domain boundary, not only the checkbox UI; preserve unreadable imported files |
| Legacy sidecar ingestion | Preserve source files and validate/import supported referenced assets transactionally; unsupported edits remain protected |
| Lightroom mask import | Existing support remains; this feature does not claim translation of proprietary AI mask payloads |

The low-level settings-group copy currently clones `masks`. Add a checked transfer
operation that knows source/target identity and rejects the unsupported case.
For this release, raster transfer is allowed only within the same catalog and the
same original-photo lineage (master and its virtual copies), with matching source
identity and image frame. A different `PhotoId` alone is not a different original;
two independently imported files with equal names/metadata are not the same one.
Other originals or catalogs require future adaptive transfer or an asset package.
Audit Paste, Sync, preset paths and their automation entry points so none bypass
it. Adaptive cross-photo regeneration is a later feature with per-photo jobs and
its own save/partial-failure policy.

## Model choice and canonical input

### Alternatives to evaluate

No model is selected yet. Compare these pretrained alternatives on the same
photographic corpus and CPU/resource gates; custom training is not required for
the initial feature.

| Candidate | Role in evaluation | Example ONNX download size | Main question |
| --- | --- | --- | --- |
| BiRefNet general, tiny backbone | Initial candidate for automatic Subject/Background | 224 MB | Do edges and semantic subject choice meet the quality gate within the CPU budget? |
| BiRefNet general, full backbone | Larger alternative for automatic Subject/Background | 973 MB | Does the measured improvement justify the larger download and resource use? |
| IS-Net (`isnet-general-use`) | Alternative automatic foreground segmentation | 179 MB | How does boundary quality and subject choice compare on real photographs? |
| U²-Net (`u2net`) | Established automatic salient-foreground baseline | 176 MB | Is its quality sufficient with a simpler deployment and acceptable latency? |
| U²-Net small (`u2netp`) | Minimal-download candidate | 4.6 MB | Can it meet the same quality gate? Small size alone is not a reason to ship it. |
| SAM / SAM 2 | Later interactive point/box object selection | Pin and measure a complete encoder/decoder export during that feature's spike | Prompted selection is a different interaction; automatic candidate masks alone do not choose the photographic subject. |

Sizes above are decimal MB rounded from published artifact metadata checked on
2026-10-08. They describe specific example exports, exclude runtime libraries,
and are not inference memory estimates or final distribution commitments.
BiRefNet examples are `BiRefNet-general-bb_swin_v1_tiny-epoch_232.onnx` and
`BiRefNet-general-epoch_244.onnx` from its
[upstream release](https://github.com/ZhengPeng7/BiRefNet/releases/tag/v1).
The IS-Net and U²-Net examples are conversions distributed in the
[rembg model release](https://github.com/danielgatis/rembg/releases/tag/v0.0.0);
audit conversion provenance and redistribution rights before adopting or mirroring
them. FP16/quantized alternatives require their own CPU/provider and quality tests;
do not assume a smaller export is numerically interchangeable.

Model references: [BiRefNet](https://github.com/ZhengPeng7/BiRefNet),
[IS-Net / DIS](https://github.com/xuebinqin/DIS),
[U²-Net](https://github.com/xuebinqin/U-2-Net),
[SAM](https://github.com/facebookresearch/segment-anything) and
[SAM 2](https://github.com/facebookresearch/sam2).

Choose one automatic-subject model after evaluation, rather than downloading all
candidates to users' machines. Subject and Background share that one installed
model. If quality tiers are added later, each must be tested and installed only
when explicitly requested.

### Runtime and input contract

Use [the Rust `ort` integration](https://github.com/pykeio/ort) with a pinned,
packaged ONNX Runtime as the proposed runtime. Gate adoption on a working model
export and all release architectures. Prefer a dynamically loaded library from
the application/package's controlled location, loaded lazily when needed; a
missing library must not stop the app or saved-mask rendering. Deliberately bundle
this executable runtime for predictable signing and library loading, while keeping
model weights on demand. Measure and report its installer-size cost in phase 0;
do not imply AI adds zero bytes to the base installer. Pin crate, runtime, opset
and model versions together after the spike, not to unverified latest tags.

The manifest must record upstream revision, checkpoint and conversion provenance,
weight/export redistribution terms, SHA-256, exact byte sizes, tensor names,
shapes, layouts, normalization, output activation and preprocessing version.
Do not infer weight licensing from the repository's code license. Ship a single
default model; do not expose an unevaluated model picker in the first release.

Create a dedicated engine input-rendering entry point with a versioned policy:

- Start from the native full-decode policy and camera-oriented default crop, not
  the embedded JPEG, on-screen texture or an arbitrary Fit pyramid level.
- Render sRGB RGB with a fixed, versioned canonical tone/color policy: camera
  matrix/standard profile and as-shot white balance with the model-input policy's
  baseline tone, independent of user Exposure, color edits, creative profiles,
  presets and mutable raw-default preferences. Capture the resolved policy values,
  rather than resolving current defaults again in a worker. Include the current
  supported retouch/red-eye operations so existing removals are visible to selection.
  Exclude monitor conversion, overlays and local adjustments. Evaluate this neutral
  input on difficult exposures in phase 0 before freezing the policy.
- Bypass user crop, straighten, Transform, rotation/flips and lens geometric
  distortion/CA, preserving exact `ImageFrame` registration. Exclude spatial
  finishing such as output sharpening, grain and post-crop vignette. Centralize
  this derivation; do not repeat a list of disabled fields in UI and worker code.
- Apply the selected model's exact resize/pad/normalization policy and record its
  inverse mapping. Do not assume letterboxing is interchangeable with stretching
  if the checkpoint was trained for a particular preprocessing contract.
- Convert validated output to coverage, undo padding/resize mapping, and store it
  in frame coordinates. Reject NaN, infinity, malformed dimensions and outputs
  inconsistent with the manifest. Use the model's specified output activation;
  do not min/max-normalize every prediction into an artificial foreground.

Capture one immutable `SelectionInput` used both to execute and to derive its
cache key: source identity, decode choices, canonical input recipe, profile data,
frame, input-renderer version, model digest and preprocessing/postprocessing
versions. Hash the same values/pixels actually consumed. Exclude view size and
monitor profile. Include provider/numerical policy if results differ by provider.

Global color/tone adjustments during or after generation do not invalidate it or
regenerate the saved selection: those settings are not model inputs. Commit onto
the current recipe so the user's latest sliders survive. View changes and geometry
excluded from input also remain valid. Source replacement, decode/frame changes or
retouch/red-eye edits during generation do invalidate it; these alter the actual
content or registration being selected. Use a monotonic content-input epoch so
changing and then restoring those inputs cannot accept an obsolete job. Record
captured provenance for explicit regeneration; it is not a live dependency that
blocks rendering an already-saved mask offline.

## Jobs, installation and resource bounds

Use one inference worker with at most one active request and one latest pending
request. Reuse one loaded model session. Budget inference threads separately from
rendering and test simultaneous inference, slider movement and export. Cache at
most the latest successful selection by input key initially; saved assets supply
durability, so a disk inference cache is unnecessary for the first release.

A request captures catalog/photo identity, document load generation, source
identity, task generation, content-input epoch, canonical input key and a target
guard. Do not use a
bare component index as the identity of an asynchronous operation.

For this release, use a session-owned monotonically increasing **mask structure
epoch**, with indices valid only inside that epoch. Any component/group addition,
deletion, duplication, reordering or shape replacement invalidates it; whole-recipe
replacement, undo/redo and snapshot restore also invalidate it, even if resulting
values equal earlier values. Enforce this in the session mutation boundary,
including UI and command edits. Adjustment/name/visibility changes can remain
valid and must survive completion. This avoids adding persistent IDs to all old
masks solely for jobs; a future concurrent mask editor can introduce IDs with its
own migration.

Before commit, check every captured guard, content-input epoch and resource limits.
For an existing component, also verify its expected shape/content reference.
Commit onto the current recipe, changing only the intended component. A manual
shape correction made during regeneration cancels that result rather than being
overwritten. New group creation also captures the structure epoch. Duplicate
clicks coalesce; old successes and failures cannot finish a newer task.

Every success, error, panic, cancellation and channel disconnection clears the
owning busy state. Cancellation invalidates acceptance immediately and asks the
runtime to terminate the run where supported. It may not interrupt a submitted
provider kernel. Do not spawn replacement workers indefinitely while cancelled
native calls remain running; wait for the one worker or report it unavailable.
Navigation discards pending work without waiting for inference.

On exit, signal cancellation, close the mailbox and hand the worker to the shared
shutdown deadline. Keep its runtime/session resources owned by the worker until
it ends; never unload a library under a running native call. No blanket join in
`Drop`. If evaluation shows unacceptable non-interruptible hangs, isolate inference
in a helper process before shipping rather than promising hard thread timeouts.
Document the final worker behavior in `shutdown.md`.

Model download/import has its own bounded worker and task state; do not reuse the
existing update downloader's non-cancellable lifecycle. It can block on HTTP or
file I/O, so configure connect/read/total deadlines, stream in bounded chunks and
check cancellation between chunks and before publication. On exit, cancel, close
its mailbox, and offer its handle to the shared shutdown deadline after dropping
bounded result receivers; detach if the deadline expires. A blocked worker retains
its own file handles and never waits for a UI reply. Clear only its owning busy
state on error, panic or disconnection. Add a separate row to `shutdown.md`.

A downloaded file's temporary file is kept on cancellation/failure and resumed by
the next install, in the same or a later session, with HTTP range requests; an
imported file's is removed. One install runs at a time across instances, holding a
lock file in the model folder; another waits for it, cancellably, then checks the
files it left. Clean only abandoned installer-owned temporary files
(those an earlier version named for its process); never remove published models or
another instance's active temporary file. No cleanup step starts a download. Model removal
is asynchronous: mark it pending, reject new leases, cancel pending use and wait
off the UI thread for active leases to finish. A native run that cannot stop may
leave removal pending until process exit; the UI reports that state without
blocking navigation or waiting indefinitely on quit.

### Hugging Face hosting and on-demand installation

Host approved model artifacts in a public, ungated Hugging Face model repository
under the maintainer's existing account.
The exact namespace/repository is to be supplied when publishing artifacts; no
repository has been created as part of this plan. Use Hugging Face for file
distribution, with all image processing local. Users need no Hugging Face account
or token, and no photo is uploaded to the host.

Publish only checkpoints/conversions whose terms permit this redistribution.
Include the model card, upstream revision, license/attribution, conversion recipe,
input/output contract, measured validation results and checksums. Upload credentials
stay in the maintainer's publishing environment; never embed them in the app.

The app's release-controlled model manifest identifies the repository, immutable
commit revision, exact artifact paths, byte sizes and SHA-256 digests. Fetch only
those files through HTTPS resolve URLs pinned to the commit, never a mutable
`main` URL or a whole-repository snapshot. Follow the host's HTTPS download/CDN
redirects and handle network failures and rate limits with bounded retry and clear
status. Hugging Face documents its file download mechanisms and redirects in
[Downloading models](https://huggingface.co/docs/hub/models-downloading).

Keep model dependencies per feature. Subject/Background requests install only
their shared model; future Objects, Sky, Depth, removal, denoise and tagging install
their own required files only when the user requests those features and accepts
the download. Reuse already verified shared files. Never initialize or download
every AI model together. Ordinary app updates do not fetch model weights; a model
upgrade is a separate explicit download and the current model remains usable.
Retain manifests and compatible input adapters for supported installed versions;
app updates cannot silently discard them. If a version must be retired, explicitly
disable new inference with a migration notice and an optional replacement download;
saved mask rendering remains available without that version. An intentional
retirement is not an automatic model upgrade.
Model weights are not bundled in the standard app installer. The packaged native
runtime is separate from model weights and remains as specified above.

Show download progress, cancellation, retry, installed size and a remove action.
Once installed, generation works offline; rendering already-saved masks needs
neither a download nor an installed model. Keep the validated offline-import route
for users who transfer the pinned files themselves. Uninstalling a model must not
delete saved mask assets or trigger an automatic re-download on the next launch.

Model installation uses the existing application-data directory policy, under a
version/digest-specific `models/` location. Download to a temporary sibling file,
enforce byte limits/timeouts, verify SHA-256, then atomically publish. Import accepts
only the supported pinned artifact, validates it the same way, and executes no
Python or remote model code. Support cancellation, offline retry and disk-full
errors without removing a working installed version. Model removal/update waits
off-thread for session leases under the lifecycle above; existing masks retain
their pixels and provenance.
Upgrading a model never regenerates edits automatically.

Package CPU support for the current release matrix: macOS arm64/x86_64, Linux
arm64/x86_64 and Windows x86_64. Evaluate CoreML on Apple Silicon after CPU
correctness. Provider support is build/model dependent, as documented by
[ONNX Runtime](https://onnxruntime.ai/docs/execution-providers/CoreML-ExecutionProvider.html);
do not assume the preview's wgpu device supplies inference acceleration. Add other
providers only with numerical, memory and packaged-install validation. Retry a
provider failure on CPU at most once and report the fallback.

## Delivery phases and acceptance gates

| Phase | Work and concrete deliverable | Gate |
| --- | --- | --- |
| 0. Model/runtime spike | Reproducible evaluator, candidate report, pinned artifact manifest, canonical-input prototype and package-loading smoke test | Select model, supported numerical policy and measured resource limits before promising shipping quality |
| 1. Raster domain and catalog | Bitmap shape/assets, validation, format upgrade, transactional saves, production retrieval, history/snapshot/transfer policies | Synthetic assets survive save/reopen, crashes and undo without inference |
| 2. Rendering integration | Shared raster inputs through every preview/overlay/export path; geometry and cache tests | Correct alignment and composition; old rendering references unchanged |
| 3. Installation and jobs | Model install/import, bounded worker, input capture, cancellation and session commit | Deterministic fake-inference tests pass for lifecycle and save failures |
| 4. Masking UI | Creation and component menus, progress/cancel/retry, regeneration and correction workflow | One result = one history step; no empty edits or stale application |
| 5. Quality and distribution | Real-model evaluation, CPU fallback, selected acceleration, signed/bundled libraries and clean-machine checks | Release criteria below pass on supported packages |

Phases 0 and 1 are independent work streams; phases 2 and 3 require their
respective foundations, and the final UI joins them. Keep preparatory mechanical
API changes separate from rendering/behavior changes. Do not hide incomplete
persistence behind an apparently finished Subject button.

### Deterministic coverage

- Soft steps, ramps, isolated pixels, disconnected regions and holes; complement,
  opacity, group inversion, every composition operator and padding boundaries.
- Camera orientations/default crops and non-square rasters; lens correction,
  perspective Transform, crop, straighten, rotation/flips; Fit, 100% regions and
  resized/full exports. Test seam/half-pixel alignment explicitly.
- CPU/GPU agreement under established tolerances; slider-only weight reuse,
  same-size raster replacement invalidation and Before/Reference correctness.
- Malformed dimensions, overflow, mismatched IDs, corrupt compression, truncated
  data, missing assets and poorly compressing rasters. Test aggregate decoded
  memory limits across many masks, not only the per-bitmap limit. Exercise hidden
  masks, distinct/shared IDs, 100 regenerations, repeated save failure, simultaneous
  render/export leases and failure when reservations cannot fit. Verify saved
  history keeps IDs rather than pinning all decoded assets.
- Catalog rollback at every asset/reference write boundary, save/reopen, snapshot
  restore, virtual copies, retained history after mask deletion, save failure and
  navigation/quit with autosave in flight. Test that a stale save completion cannot
  mark a later generated mask saved.
- Snapshot add/update immediately after generation, before autosave; subsequent
  autosave failure/crash must not break that snapshot. Snapshot acknowledgements
  must not mark the current edit saved. Sync without Masking and both Sync/Library
  Undo must work with existing destination rasters and no uploaded asset batch;
  instrument repeated slider autosave to prove old blobs are not recompressed.
- Repeated Undo/Redo/History/Snapshot actions while assets resolve: session History,
  shared Undo log and automation results remain consistent; obsolete success/error
  cannot alter the newer state. Pending/over-budget rendering must not block saving,
  snapshots or navigation when references exist; known missing/corrupt assets do
  block saves and in-transaction reference checks reject absent blobs. Exercise
  this with export leases holding the budget and with permanently over-budget
  metadata-only edits. Snapshot insert/update failure is
  reported synchronously and never creates rename state for an uncommitted row.
- Missing/corrupt assets in Develop, Reference, Loupe, thumbnails, Build Previews,
  history/Before and every export route must retain the edit and propagate errors,
  never go through an `Err -> None -> Defaults` fallback. Masking's intentionally
  mask-free color sampler remains usable on healthy documents with bitmap masks.
- An old After render completing while new assets are pending/failed must not be
  displayed or cached under the new recipe. Verify captured cache tags as well as
  task generations, including the Develop-to-Library thumbnail publication path.
- Raster-free legacy recipes retain stored forms and rendering; unknown data and
  legacy sidecars are preserved; previous-release downgrade refusal is exercised.
  Test legacy FNV and new SHA-256 ID validation independently. Verify in-place
  version upgrade/backup failures, lost completion, new version 2 catalogs, preserved
  preview requests and no model download as a side effect of catalog creation.
- Stale completion after navigation, source replacement, deletion/recreation,
  reorder, undo/redo, snapshot restore, superseded jobs and target shape editing.
  Global/local slider edits survive accepted completion; retouch/decode/input-frame
  changes invalidate even after changing back. Panic, disconnect, cancel and
  provider failure clear only the owning busy state.
- No model requests at app startup/update, catalog opening, drawer opening or
  saved-mask rendering. Requesting Subject/Background downloads only their shared
  model after explicit acceptance; declining downloads nothing. Test that future
  unrelated model dependencies are not fetched and that removing a model does
  not cause background reinstallation. Verify public downloads without credentials,
  pinned revisions, CDN redirects, rate limits and unavailable hosting.
- Interrupted import/download, bad checksum, disk full,
  offline mode, unavailable runtime and missing provider. Saved masks still render
  with the model and runtime removed. Export never starts inference.
- Download-worker loss/shutdown, abandoned temporary-file cleanup, and asynchronous
  model removal during an uninterruptible inference call. Old model manifests remain
  usable after an app update unless explicitly retired with a visible migration.
- Copy/Sync/preset restrictions are enforced through both UI and domain entry
  points. Test neutral/hidden masks as well as visibly active ones.
- Non-catalog generation/regeneration is refused before installation or inference;
  existing non-catalog manual editing/export behavior remains unchanged.

### Photographic and performance evaluation

Use a licensed or explicitly approved corpus covering portraits, hair/fur,
animals, products, architecture, multiple/small subjects, thin structures, holes,
clutter, low contrast, transparency and no clear subject. Keep a holdout set not
used to choose thresholds. Record provenance without committing private RAWs or
machine paths. Do not upload photos to evaluation services.

Measure region overlap and boundary accuracy where labels exist; inspect 100%
edges and actual local exposure/color edits at Fit and export. Judge semantic
subject choice separately from boundary quality. Record failure classes and the
manual cleanup required. Compare candidate outputs on identical canonical input;
a display screenshot is not equivalent model input.

Measure cold load, first and warm inference, preprocessing/postprocessing, peak
RSS/device memory, cancellation response and preview responsiveness. Record CPU,
OS, model digest, provider and numerical settings. Phase 0 must publish explicit
go/no-go thresholds and reference machines before model selection; no timings,
memory promises or parity claims are established by this planning work. If no
candidate meets the agreed gates, stop release integration and revisit the model
or runtime rather than silently reducing quality.

### Build and release verification

Run current stable Rust and the required checks before each push:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
python3 scripts/deps.py check
scripts/crate-closures.sh
```

Also run the applicable checks in `.github/workflows/ci.yml`: MSRV, model
all-features, isolated crate builds, portable SQL
preparation, dependency/license audit, platform tests and Arch install validation.
Keep ordinary tests download-free; run pinned actual-model tests separately in CI
and in packaged-app smoke tests. Extend release packaging/verification for runtime
libraries, signing/notarization and third-party notices. Verify inference on a
clean machine and saved-mask export without the runtime on each supported target.
Report private-photo, GPU and unavailable-platform coverage explicitly.

## Later work that reuses the foundation

| Feature | Reuse | Additional contract required |
| --- | --- | --- |
| Grow/shrink and feather | Raster sampling and operator versions | Image-space units, deterministic preview/export behavior, bounded edge processing; preserve base coverage for undo |
| Interactive Objects | Model install, jobs, raster persistence | Prompt coordinates, encoder/decoder compatibility, bounded embedding cache and positive/negative-point UI |
| Adaptive Copy/Sync | Captured inputs and per-photo jobs | Regenerate on each destination, carry manual corrections deliberately, batch cancellation and partial-failure semantics |
| Sky / People | Existing mask groups and workers | Separate evaluated semantics, licensed checkpoints and component choices |
| Depth masks | Asset store and inference | Depth representation/normalization, range evaluation and foreground boundaries |
| Lens blur | Depth input | Occlusion-aware renderer, highlight behavior and export-resolution memory limits |
| Content-aware removal | Stored raster assets and jobs | Generated RGB patch type, color-space/input provenance, placement and ordering with Heal/Clone/red eye |
| Generative replacement | Patch lifecycle | Separate model/service, explicit image-transfer flow when remote, reproducible saved output |
| AI denoise | Model lifecycle | Sensor/RGB input domain, noise assumptions, pipeline placement and tiled output lifecycle |
| Suggested tags | Model installation | Model/tokenizer pair, label vocabulary, review UI and catalog keyword writes |

Resolve the model/checkpoint, exact runtime/provider build and measured budgets in
phase 0. All other first-release behavior above is the proposed implementation
contract. Broader AI work remains independent of shipping Subject and Background.

## Implementation notes

Built as one change, after trying candidates on real photographs. SAM 2 alone (what
RapidRAW uses for subjects; its code in fact downloads SAM 1 ViT-B, with U²-Net for
foreground and sky) draws crisp outlines but does not decide which object matters.
IS-Net picked subjects well but its weights have no license (see dependencies), so it
was dropped; BiRefNet-lite at 1024 px needs 7 GB, and at 512 px shares IS-Net's
DIS5K training-data question. The shipped design combines **DETR panoptic** (Apache-2.0
base weights, COCO), which finds each person, animal and the sky, with **SAM 2.1 Hiera
small** (Apache-2.0, `onnx-community` export at a pinned commit), prompted per instance
with its box and points to draw the outline (as detector-prompted SAM pipelines do), and
a guided filter moving the result onto the photo's edges. Clicks (positive, negative, box) go
through the same SAM 2 decoder, which is how anything DETR does not know is selected.
Measured on a handful of photographs only: the three people of a group portrait, a man
in a forest and the sky of overcast, landscape and dusk scenes come out right, a
landscape has no subject, and a dusk street picks up passers-by. ONNX Runtime is 1.23.2
loaded dynamically (`ort` `load-dynamic`) on every platform. The model files are mirrored,
unmodified, at [`pch/rawmakase-models`](https://huggingface.co/pch/rawmakase-models)
(pinned commit; upstream repositories as fallback).

Deliberate differences from the plan above:

- Rasters are resolved through one process-wide store with a catalog-backed loader,
  not a `MaskAssets` value passed into every job. Rendering fails on an unresolvable
  raster; opening a photo whose raster is missing protects the edit from saving.
- The mask structure epoch is a fingerprint of the masks' structure plus the spots and
  red eye, compared when a result arrives. Raster memory limits are the per-raster
  limit and a 256 MB per-edit admission check, not process-wide leases.
- Transfer of raster masks to another photo skips them with a note instead of
  rejecting the whole transfer; presets reject them. No Library-wide `EditSource`
  rework: the global loader serves workers.
- The models are downloaded file by file. A finished file is kept on retry, and a
  partial one is resumed with a range request, after a dropped connection, a request
  reaching its one-minute body budget, a cancel or a restart of the app; a source
  gives up after three requests in a row bring nothing. The bytes kept are hashed
  again when resumed, and a file whose checksum fails is fetched again from the start
  from the other source.
- Hair and fur edges are SAM 2's 256-pixel mask snapped to the photo by the guided
  filter. Replacing the edge band with a matting model's alpha (MODNet, BiRefNet-lite
  and BiRefNet-lite-matting at 512 and 1024 px on a crop of the subject, ViTMatte-S
  with a trimap) was tried on a group portrait, a man in a cap, a lion's mane and a
  cat: ViTMatte and BiRefNet-matting at 1024 px kept a few more strands of fur, the
  rest matched the current result, and none was judged a meaningful difference for
  100–200 MB more to download and a second more per photo (ViTMatte's weights are
  also trained on Adobe's restricted Composition-1k). Not added.
- Sky was first the DETR sky label refined with SAM 2's outlines, and left blocky gaps
  beside poles and heads, and took the sea for sky. It is now UperNet ConvNeXt base
  (see [masking](masking.md) and [dependencies](dependencies.md)), its own pack and its own
  session, loaded when first asked for. It was looked at on 256 of the owner's photographs, chosen
  by what could break it (blown skies, water, foliage, night, no sky) and at random, with
  no ground truth: its worse cases are in masking's limits. It runs at 1024 px, as tried
  first: 768 px takes half the time and 1.7 GB, gets some hazy windows right and leaves
  holes in others' bright clouds. The horizon cut the old
  outlines needed is gone: on the new model it only removed real sky behind what stands
  in front of it.
- The subject choice is measured on a handful of photographs, not on the plan's
  evaluation corpus; there is no faint-result threshold and no grow/shrink/feather.
  Selections run on the CPU by default; the plan's acceleration option arrived as the
  optional [remote worker](remote-inference.md), which may register CUDA or CoreML
  where the runtime truly supports them, and selection over it is excluded from the
  numbers above.
