# HDR editing, camera profiles and export

Investigation and proposed implementation, 2026-11-05. Repository inspected at
`4c9eb17e`. Covers [HDR editing (#253)](https://github.com/pch/rawmakase/issues/253)
and [JPEG XL PQ/HLG and gain-map export (#894)](https://github.com/pch/rawmakase/issues/894).
This is a plan, not a claim of implemented support or standards conformance.

## Recommendation

Implement an explicit HDR editing mode with a floating-point, wide-gamut output
boundary, HDR camera-profile evaluation, an independently adjustable SDR rendition,
and display-aware presentation. Build JPEG XL PQ, HLG and gain-map exports on that
boundary. Adding an encoder to the current SDR output would permanently lose the
highlights before encoding.

Keep these concepts separate in the API and UI:

- A source can contain extended-range raw samples, PQ/HLG pixels, or a base image
  and an auxiliary HDR gain map.
- An edit has SDR or HDR intent. Ordinary single-exposure RAWs can be edited in
  HDR; a merged DNG need not have HDR editing enabled.
- A camera profile describes camera colour and rendering intent. An HDR camera
  profile is not a PQ/HLG export colour space.
- A display has a changing gamut and headroom. Those facts change presentation,
  not the saved edit or the brightness of an exported file.
- An export chooses a container, gamut, transfer function and rendition strategy.
  Gain maps are a rendition strategy, not another transfer function.

Bracket alignment and HDR merging are separate work. Reading existing merged DNGs
is in scope. Adobe Adaptive Color/B&W is also separate from HDR DCP support: its
image-dependent processing cannot be reproduced simply by loading a camera DCP.
Adobe describes that distinction in its [Adaptive Profile article](https://blog.adobe.com/en/publish/2024/10/14/the-adobe-adaptive-profile).

## Evidence gathered

Both issues and their comments were read. Neither supplies an HDR sample. The only
open PR at investigation time was unrelated (#897). The standards list in #894
is a request, not evidence that every listed standard or edition applies to this
feature. The implementation needs a precise encoding contract and conformance
fixtures, rather than a blanket promise to implement that list.

The Samsung SSD's dataset and TEST directories were inspected without changing
originals or existing references. The local stress-reference handoff was read.
The dataset includes six Canon EOS 5D Mark III floating-point HDRMerge DNG variants
(16/24/32-bit, compressed/uncompressed), the Lightroom HDR-merge DNG supplied for
#525, and three Nikon Z 8 NEFs. The Nikon files report Standard Picture Control,
sRGB and HDR Off; they are useful ordinary-RAW controls, not confirmed HLG captures.
The inspected directories contained no standalone HIF, HEIC, AVIF or JPEG XL files.
This is not a claim about every directory on the SSD.

ExifTool confirms that the Lightroom merge is linear RAW with JPEG XL compression
and floating-point image data. Its saved edit has `HDREditMode=0` and
`HDRMaxValue=+4.00`. The Canon compressed FP32 example is a floating-point CFA DNG
using Deflate. These need different decode paths despite both being HDR merges.

Five small Camera Raw reference renders were made from temporary copies using
Photoshop scripting, with Camera Raw **18.7 (2743)**:

- Nikon Z 8, Camera Standard, HDR off.
- The same source and profile, HDR on.
- The same source, Camera Standard HDR, HDR on.
- The Lightroom merge with its existing settings, HDR off and on.

The SDR outputs embedded Display P3. HDR outputs embedded P3 PQ and retained
`HDREditMode=1`; the HDR DCP output retained `CameraProfile=Camera Standard HDR`.
PQ decoding of the Nikon TIFF samples confirms values above 203 cd/m². Both an
ordinary camera profile and its HDR variant can therefore produce HDR output;
their rendered pixels differ. The merge also switched to PQ when HDR was enabled.
These are controlled opening/rendering probes, not a complete Adobe export-dialog
or pixel-parity qualification. Existing workflow preferences produced 16-bit PQ
TIFFs; do not assume all Camera Raw-to-Photoshop workflows produce float files.

Installed Adobe Nikon and Panasonic HDR profiles were found. The Nikon Z 8 Camera
Standard HDR DCP has tag 52551 (`ProfileDynamicRange`), TIFF UNDEFINED, eight bytes:
version 1, dynamic range 1, maximum-output hint 0. Its ordinary and HDR profiles
share `ProfileGroupName` (52552), but have different tone curves. A zero hint must
not be interpreted as a zero brightness ceiling.

Adobe's downloaded [DNG SDK](https://www.adobe.com/support/downloads/dng/dng_sdk.html)
(1.7.1 build 2768) confirms the eight-byte layout: two uint16 values followed by a
float32. It validates the version and SDR/HDR enumeration. Its renderer enables
overrange profile processing, extends the tone curve beyond its tabulated domain,
and supports linear floating-point HDR output with 1.0 representing UI white.
Relevant source is `dng_shared.{h,cpp}`, `dng_camera_profile.cpp` and
`dng_render.cpp`. This establishes reference semantics; the SDK is not a proposed
new runtime dependency and is not a substitute for Camera Raw process-version
measurements.

No private image, proprietary profile, or machine-specific dataset path is included
in this document. Temporary scripts and TIFFs remain local to the investigation.

## Lightroom and Camera Raw behavior to match

Lightroom makes HDR a Develop choice, extends its histogram above graphics white
in stops, shows display headroom, and provides an HDR limit and an SDR preview with
separate rendition adjustments. HDR settings participate in Copy, Sync and presets.
HDR export is a separate choice. Its documented HDR gamuts are Rec.709, P3 and
Rec.2020. Library HDR support is view-specific; thumbnails need not be HDR.
See [Lightroom Classic HDR](https://helpx.adobe.com/lightroom-classic/desktop/process-and-develop-photos/hdr-output.html).

Camera Raw also separates editing, display and output. Its documentation describes
HDR interchange with Photoshop and HDR-capable file formats, but workflow settings
must be recorded when generating references, as the local PQ-TIFF probe shows.
See [Camera Raw HDR](https://helpx.adobe.com/camera-raw/desktop/hdr-and-advanced-output/hdr-output.html).

A gain-map file stores a base rendition, a map and metadata so a reader can adapt
between SDR and HDR appearances to available display headroom. Adobe identifies
ISO 21496-1 as the standardized gain-map representation. This is distinct from DNG
lens-shading gain opcodes and camera-profile gain tables already encountered by the
pipeline. See [Adobe gain maps](https://helpx.adobe.com/camera-raw/desktop/hdr-and-advanced-output/gain-map.html).

## Current implementation boundaries

| Owner | Current limitation | Planned responsibility |
| --- | --- | --- |
| model `camera_profiles/dcp.rs:185` | Rejects any tag 52551 | Parse and validate HDR metadata and profile grouping; retain profile intent in saved recipes |
| model `camera_profiles/mod.rs` | Profile operations assume the existing SDR path | Explicit overrange profile evaluation and reproducible identity |
| interop `xmp/apply.rs` | Rejects active `HDREditMode`; ignores HDR/SDR values only when HDR is off | Typed HDR settings, preservation and application through the same edit model |
| decode `raster/jxl.rs`, `avif.rs`, `heic.rs` | Refuses HDR development; previews are SDR; HEIC gain maps are ignored | Preserve HDR source pixels and colour metadata before presentation |
| engine `pipeline/pixel.rs:304` | Final output clamps to sRGB 0–1; earlier operators also assume SDR | Separate scene processing, HDR rendering and destination encoding |
| model `rendered.rs` | Untagged float triples, SDR histogram, clamped integer conversion | Explicit output colour/range contract and HDR measurements |
| engine `gpu/present.rs`, app/platform display | 8-bit presentation and SDR surface handling | HDR photo presentation with SDR UI and correct display transitions |
| export `export/encode.rs` | JPEG/PNG/TIFF quantize existing SDR output | HDR encoders and metadata from one captured export specification |

Follow [architecture](../architecture.md), [code map](../code-map.md),
[colour pipeline](../color-pipeline.md), [profiles](../lightroom-profiles.md) and
[dependencies](../dependencies.md). Keep facts in model, file translation in
interop, decoding in decode, rendering in engine and publication in export.

## Implementation sequence

### 1. Freeze HDR fixtures and resolve the codec/display feasibility risks

Create a separate local HDR reference family; do not repurpose or re-bless the SDR
parity corpus. Record source hash, licence, Adobe version, complete settings,
camera-profile identity, output profile, depth, transfer and reference white.
Record source/output dimensions, crop, resampling method and resampling colour
domain. Freeze full-resolution references or reduce them in documented linear
light, preferably from 32-bit linear output where available. The investigation's
640-pixel bicubic PQ TIFFs are diagnostic probes, not frozen parity references.
Use the discovered merges and Nikon RAWs; add synthetic ramps, saturated highlights,
negative matrix results and values crossing graphics white.

Acquire or generate missing fixtures: actual camera PQ/HLG HEIF, Apple gain-map
HEIC/JPEG, Adobe/ISO gain-map JPEG, and JXL PQ/HLG/gain-map files. Preserve originals.
The current SSD sample set does not qualify these formats.

Measure one control at a time: HDR toggle/limit, Exposure, Highlights, Whites,
curves, masks, SDR rendition sliders, and ordinary versus HDR profiles. Capture XMP
and Lightroom Develop records. Check selection and fallback when toggling HDR,
including unavailable paired profiles; do not infer pairing from filename suffixes.
Record which parts are documented, observed, or still unknown.
Also measure unedited PQ/HLG/gain-map opening defaults (HDR mode, profile and tone),
and whether the default SDR rendition equals an HDR-off render. Set numeric HDR
acceptance tolerances from these references and repeatability measurements before
stage 3 starts; missing thresholds block acceptance, not implementation experiments.

Prototype a float HDR photo surface through the actual eframe/wgpu compositor
before committing to a UI design. Separately prove JXL encoding and `jhgm`
read/write with independent decoders. RAWmakase currently ships no native imaging
library: evaluate a Rust encoder first and treat any native codec proposal as an
explicit dependency/packaging decision, not an incidental addition. Use libjxl as
an interoperability oracle; its [gain-map metadata API](https://libjxl.github.io/libjxl/api_metadata.html)
confirms bundle support, not automatic support in every viewer or Rust decoder.
At the end of this spike, the implementation owner records the encoder decision
and demonstrates PQ, HLG and gain-map output with the chosen candidate. If no Rust
encoder qualifies, present the user with a concrete libjxl proposal covering
crate-closure checks, platforms, packaging and licence notices. Do not weaken
existing dependency checks to make it pass. If that dependency is declined and no
Rust candidate qualifies, continue HDR editing and TIFF interchange, explicitly
leave #894 open, and record JXL as blocked rather than substituting another format.

Acceptance: reference manifests are reproducible, extended values survive the
candidate boundaries, and codec/display feasibility has a concrete result.

### 2. Add the model and persistence contract

Add validated HDR editing settings, an HDR limit, and an SDR rendition settings
group. Default old recipes to their existing SDR behavior. Keep view-only SDR
preview and display headroom outside the recipe. Preserve dormant HDR values when
HDR is disabled; toggling must not discard the user's work.

Extend `saved_format::schema_for` with a new HDR schema above the then-current
`NEWEST` (currently 13), selected whenever HDR editing is active or an embedded
profile requires HDR semantics. Combine feature requirements by taking their
highest schema. Older releases must refuse and preserve these edits before
deserializing their profiles. Omit new SDR-default profile fields on serialization
because `CameraProfile` denies unknown fields. Dormant HDR/SDR values with HDR off
and an SDR-compatible profile keep the otherwise applicable schema. Test catalog,
history, snapshots and preset envelopes against the previous reader's version
check, and preserve unsupported records without rewriting them.
ProfileGroupName (52552) stays library/selection metadata and is never embedded
in saved recipes. Resolve profile pairing from the installed library when toggling
HDR; the selected profile's rendering data still travels with the recipe. Test an
ordinary SDR profile with a non-empty group name, HDR off, through the previous
release's envelope check and profile deserializer.

Introduce explicit types for source encoding and rendered output: primaries,
white point, linear/encoded transfer, reference-white normalization and intended
headroom. Retain unbounded finite f32 scene values. Define rendered HDR 1.0 as
graphics/reference white; keep conversion to absolute nits at output adapters.
Do not impose an arbitrary four-stop storage ceiling because Adobe's UI shows it.

Wire XMP `HDREditMode`, `HDRMaxValue`, `SDRBlend`, `SDRBrightness`, `SDRContrast`,
`SDRHighlights`, `SDRShadows`, `SDRWhites` and `SDRClarity` after measuring exact units
and defaults. Cover Lightroom import, XMP output, native presets, Copy/Sync,
history, reset, virtual copies, autosave and automation. Preserve unknown fields
and continue reporting unsupported active settings rather than dropping them.
Add a distinct **High Dynamic Range** Copy/Sync/preset setting group, owning mode,
limit and SDR rendition controls. Keep profile selection in its existing group;
validate the resulting combination atomically so selecting only one group cannot
install an unsupported profile/mode pair. View-only preview state is never copied.

Acceptance: old edits keep their settings and rendering; HDR settings round-trip
through every supported entry point and make one coherent history/save operation.
Until the activation gate below passes, round-trip means preservation while active
HDR remains refused, not successful application of an HDR edit.

**Release activation rule:** stage 2 adds parsing/preservation and tests, but keeps
active HDR a reported blocker in XMP, Lightroom import, presets, Sync, native
recipe resolution and automation. No UI enables it yet. Lift this single shared
capability guard only after stage 3 supplies the CPU HDR master and authored SDR
rendition and every user-visible caller can consume them. Until GPU HDR math is
qualified, route HDR jobs explicitly to the CPU and display that SDR rendition in
Develop, Reference View, Library previews and thumbnails, with an SDR-preview
status. Never pass an HDR recipe through the old GPU SDR math. Unsupported active
edits stay preserved and refused, not silently downgraded. Test the intermediate
release behavior at each entry point. Stage 5 replaces this deliberate fallback
with qualified HDR presentation; it does not change saved rendering intent.

### 3. Implement HDR camera profiles and the CPU HDR renderer together

Read tag 52551 with byte-order, type, length, enum, version and finite-value checks.
Read 52552 for grouping. An explicit SDR value in tag 52551 must not be rejected
merely because the tag exists. Keep unsupported versions explicit. Preserve embed
policy, copyright, camera matching and profile identity. Include HDR rendering
metadata in schema-gated saved profiles and rendering fingerprints/caches; keep
52552 grouping only in library/selection metadata as specified in stage 2.
Rescan Adobe profile availability and offer the normal explicit re-import action
for profiles previously skipped as HDR. Do not silently copy proprietary assets.
Invalidate cached profile lists/fingerprints when capability changes. Replace the
current HDR-skipping test with valid HDR import tests and a separate unsupported
version test; its existing synthetic tag actually encodes SDR and should be tested
as such rather than merely renamed.

Implement measured overrange behavior for profile matrices, hue/saturation maps,
look tables and tone curves. Audit clamping and lookup domains in both the model
and engine. The SDK's curve extension is evidence to test against, not permission
to extrapolate every LUT identically. Include monochrome and ordinary profiles in
HDR mode, plus HDR-profile behavior when returning to SDR. Do not silently replace
a saved profile with a vaguely matching one.

Initial capability policy for the other profile kinds is explicit:

- RAWmakase Standard/Neutral and camera-matrix fallbacks: qualify their HDR tone
  policy on synthetic and real RAW fixtures before enabling them in HDR.
- Embedded profile for rendered HDR: use the display-referred identity path in
  stage 4, not the bounded inverse-ACR3 path used for SDR rendered files.
- `.cube` looks, fitted Camera Look and enhanced creative looks (including Amount):
  initially not HDR-capable. Refuse activation/import of an active HDR combination
  with a visible explanation; keep the saved profile and edit. Do not silently
  disable the look or replace the camera profile. Later extensions require a
  specified overrange rule and tests for each kind and Amount endpoint.
- DCP `Table` data with the `srgb` flag: support only once encoding, lookup and
  overrange reconstruction match the measured DCP contract. The flag describes
  table encoding, not permission to clip HDR input; otherwise report that profile
  as unsupported in HDR. Test linear and sRGB-encoded tables separately.

Keep capability facts with the profile model and validation, not UI heuristics.
Add a regression for each kind, including toggling HDR on/off without losing the
profile, and Copy/Sync attempts that combine incompatible choices.

Keep one shared scene-development pipeline with explicit SDR/HDR rendering policy.
Audit scene tone, exposure, Auto, point/parametric curves, colour mixer, grading,
Point Color, B&W, masks, detail, effects, healing and watermark composition for
0–1 assumptions. Define each operator's HDR domain and test it. Produce an HDR
master and derive its SDR rendition; SDR rendition controls must not modify the
HDR master. Apply resizing and output sharpening in a documented domain.
Measure and fit HDR-mode default tone, Exposure, Highlights, Whites and HDR limit
against the new HDR reference family. Existing scene-tone/local-tone fits target
SDR and cannot be reused above white merely by removing clamps. Keep this as an
explicit rendering policy in the current engine, not a saved legacy-engine switch.

Acceptance: real HDR profiles render correctly on the selected RAWs; highlights
remain distinguishable above white, controls remain finite, and SDR regression
gates pass. No HDR import button is advertised as supported before rendering works.
HDR tone must meet the numerical tolerances frozen after stage 1 on validation and
then holdout; distinguishable highlights alone are not a parity acceptance test.

### 4. Preserve HDR through rendered-file input

Replace boolean HDR refusal with typed source facts, enabling each format only
once its decoder preserves the required samples. Support PQ and HLG conversion,
ICC/CICP precedence, range and matrix signaling, and float HDR TIFF where valid.
HLG needs its reference viewing/system-gamma contract, not just inverse gamma.
Avoid decoding through an 8-bit image or system tone-mapped bitmap first.

Use a typed display-referred HDR source path: linearize and convert primaries,
then preserve the authored HDR pixels at neutral settings. Do not extrapolate the
SDR inverse ACR3 curve or reapply a raw camera's default tone to this source. Share
adjustment operators where their domains match; use an explicit rendered-HDR
adapter where they do not. Test default render/reopen identity within the stage-1
tolerance, including highlights above white and saturated colours. Library direct
display and neutral Develop must agree for both HDR and the SDR rendition.

Default a newly opened, supported and positively identified PQ/HLG or gain-map
source to HDR intent once the activation gate is satisfied; explicit saved SDR/HDR
intent always wins. Before that gate it remains refused for Develop. An SDR view
of such a file uses the same rendition policy as Develop; gain-map inputs retain
their authored SDR base at neutral settings. Merged RAWs remain a separate default
policy measured in stage 1, not classified solely by float sample depth.

Read gain maps with bounds and metadata validation. Reconstruct the authored HDR
rendition in a display-independent way for editing while retaining the SDR base
and provenance. Never bake the current monitor's headroom into decode-cache data.
Use the same source contract for Develop, preview builds and export. Keep unrelated
animation and unsupported-container refusals intact.

Acceptance: reopen independent PQ/HLG/gain-map fixtures with correct brightness,
colour and orientation; malformed maps fail safely; changing displays cannot
change decoded pixels or exported results.

### 5. Add HDR presentation and editing UI

Port the CPU HDR math to the GPU and compare both in float space. Provide an HDR
photo surface; keep UI graphics at SDR white. On macOS, use an extended-linear
colour space, suitable float surface and EDR opt-in, following [Apple's EDR guidance](https://developer.apple.com/documentation/metal/performing-your-own-tone-mapping).
On Windows, implement the corresponding Advanced Color/scRGB or HDR10 path with
correct SDR-white scaling, following [Microsoft's guidance](https://learn.microsoft.com/en-us/windows/win32/direct3darticles/high-dynamic-range).
Linux requires an explicit compositor/backend capability path; until verified,
provide SDR preview and HDR export with a clear display-status message.

Add HDR toggle, limit, SDR preview/rendition controls, histogram stops, pixel
readouts and visualization of highlights beyond display headroom. Distinguish
source clipping, authored HDR limit and monitor clipping. Start with Develop and
reference/comparison views; deliberately use the authored SDR rendition for
thumbnails until Library HDR previews are implemented.

Handle monitor moves, brightness/headroom changes, HDR disabled by the OS, surface
recreation and GPU loss. CPU fallback must still present a valid SDR rendition.
Display transitions are view changes, not history edits.

### 6. Ship HDR export, including all three #894 workflows

Capture recipe, source identity, profile, rendition settings and output specification
once per job. Encoding must not inspect the active monitor or mutable editor.
Export work may ship before stage 5's native HDR display after stages 1–3 and the
CPU/SDR-preview activation contract pass. Stage 4 is additionally required for
rendered-HDR inputs. Such a release explicitly identifies its preview as SDR;
it does not claim #253 complete. Independent output decoders can qualify RAW-to-JXL
exports before RAWmakase's own JXL reopening support is finished.
If shipped before stage 5's toggle UI, state that HDR edits currently enter through
supported XMP, Lightroom import or presets, and that on-screen preview is SDR.

- **JPEG XL PQ:** explicit P3/Rec.2020/Rec.709 gamut as supported, correct transfer
  and container signalling, adequate depth, and a documented reference-white and
  peak policy. Use 203 cd/m² as a proposed interoperable reference-white default,
  to be verified with golden files rather than inferred from display brightness.
- **JPEG XL HLG:** implement the full BT.2100 conversion, including the declared
  reference display/system gamma and inverse OOTF when converting display-linear
  pixels. Merely replacing the PQ transfer with the HLG OETF is insufficient.
- **JPEG XL gain map:** write the actual `jhgm` bundle, alternate colour encoding,
  map and ISO metadata. Generate it from the final HDR and authored SDR renditions,
  with matched geometry, resizing and watermarks. Validate both base-only decoding
  and headroom-dependent reconstruction.
- **JPEG gain map:** add a compatible SDR-base sharing option with ISO/Adobe metadata
  interoperability tests. This provides a separate compatibility path, not a
  substitute for JPEG XL gain maps requested by #894.
- **HDR TIFF:** add an explicit float-linear interchange output and, if verified,
  tagged PQ output. AVIF/PNG HDR can follow once their codec/metadata paths pass
  equivalent tests; they are not required to delay the JXL milestone.

Keep gamut, transfer and container as separate validated choices; expose useful
presets in the dialog and reject invalid combinations. Embed truthful colour and
HDR metadata, never fabricated mastering-display values. SDR export from an HDR
edit uses the authored SDR rendition. Validate files with an independent decoder,
not only RAWmakase's own reader. Test destination failure, cancellation and atomic
publication through the existing export queue.

## Validation and completion criteria

Use synthetic public fixtures for transfer-function anchors, negative/overrange
handling, profile tags, malformed metadata, gain-map reconstruction and format
round-trips. Add failing behavioral tests before implementation. Use private RAWs
and Adobe profiles only for local references, with a distinct HDR holdout set.

The existing SDR Lab/ΔE00 score cannot establish HDR fidelity. Compare in a declared
linear-light space with luminance/headroom diagnostics and a documented HDR metric
(evaluate ΔEITP); retain SDR gates for SDR output. Set tolerances from measured
CPU/GPU and encode/decode variation, including gamut edges and local adjustment
boundaries. A TIFF's HDR metadata alone does not prove its pixels are HDR.

Cache identities must cover source encoding/gain-map interpretation, edit/profile
semantics and rendition kind. Display-dependent presentation caches additionally
need display-state generations. Test stale completion after edits, HDR toggles,
photo replacement/deletion, cancellation, worker loss and display changes. Keep
busy-state cleanup, bounded shutdown and save-before-navigation invariants.

Before implementation pushes, run `make check`, relevant crate/dependency checks,
SDR rendering regressions, and the separate HDR tests. Run dataset renders and
Camera Raw parity sequentially, following the corpus README and local handoff.
Update architecture/code-map/module docs as boundaries change. Cross-provider
review should cover colour/profile semantics, persistence/cache lifecycle and
codec/display paths before shipping.

#253 is complete only when HDR edits persist, render on supported HDR displays,
have a useful SDR fallback and survive reopening. #894 remains open until JPEG XL
PQ, HLG **and gain-map** outputs pass independent interoperability checks. A
macOS-first display release must state Windows/Linux display coverage explicitly.

## Remaining uncertainties and effort

This investigation verified source metadata, five Camera Raw renders, installed
HDR profile data, the SDK's HDR behavior and the repository's blocking paths. It
did not qualify Adobe's JXL/AVIF/gain-map export dialogs, HLG captures, Windows/Linux
HDR display, every adjustment in HDR, or Adaptive profile parity. The current
dataset needs additional encoded-HDR and gain-map coverage. No full rendering or
CI run was warranted for this documentation-only investigation.

Plan this as several reviewed changes over multiple weeks, not an encoder-sized
patch. The initial reference/codec/display spike should determine a credible
schedule; cross-platform presentation, overrange adjustment parity and the JXL
encoder choice are the largest uncertainties. Keep mechanical output-type changes
separate from behavior changes and retain working SDR behavior at every milestone.
