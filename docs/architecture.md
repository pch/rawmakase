# Architecture

RAWmakase is the app crate with explicit domain modules, on top of
`crates/rawmakase-model`: what an edit is, as values (the recipe and its settings,
camera and lens profiles, colour primitives, DNG hints, output buffers, storage helpers), which
Cargo builds without LibRaw, the renderer, wgpu or the GUI, and
`crates/rawmakase-interop`: the file formats and presets over it (XMP, Lightroom's
Develop settings, presets, raw defaults, EXIF and JPEG, export settings),
built the same way, and `crates/rawmakase-catalog`: the SQLite catalog, Lightroom
catalog import and which edit a photo develops with. Above the model,
`crates/rawmakase-engine` is the renderer (`develop`, CPU and the GPU preview port),
built without LibRaw, the catalog or the GUI; `crates/rawmakase-native` is the only
crate that compiles and links LibRaw and Little CMS (`raw`, `demosaic`, `photo`,
and the full-size decode in `decode` and `decode_cache`, which uses the engine's
highlight recovery); `crates/rawmakase-export` develops and writes exports with
their watermarks over both; `crates/rawmakase-inference` is the one leaf that runs
the subject selection model through a lazily loaded ONNX Runtime (pixels in,
coverage out; it knows no other workspace crate, and no value, catalog or
rendering crate can reach it, so saved masks render without any inference). The app crate re-exports these crates' modules at its
root (`crate::model`, `crate::xmp`, `crate::catalog`, `crate::develop`…), so paths
read the same on either side. The desktop app and CLI compose these APIs;
parsing, persistence and rendering implementations do not import the desktop UI.
Use the domain paths below for new work.

For file-by-file navigation, runtime flows, storage locations and feature entry
points, see the [code map](code-map.md). This guide describes the boundaries those
files should preserve.

## Module ownership

| Module | Owns | Main extension points |
| --- | --- | --- |
| `camera_data` | A RAW's metadata, the demosaic and decode choices and the decoded camera-space image, as values with no native code | `camera_data.rs` |
| `raw` | LibRaw/Little CMS boundary: reading metadata, decoding into `camera_data` images, oriented embedded thumbnails | RAW decoding and native color management |
| `photo` | Opening a photo: LibRaw's facts from `raw`, then the camera's lens tables, a DNG's profile and hints, and the imported lens profiles that fit | `photo.rs` |
| `demosaic` | RAWmakase's own Bayer and X-Trans demosaicing of the unpacked sensor data | `demosaic.rs` |
| `dng`, `tiff` | A DNG's rendering hints (embedded profile, baseline exposure, crop, opcodes) and the bounded TIFF reader behind them | `dng.rs`, `tiff.rs` |
| `decode` | A photo's full-size image, made one way for Develop, prefetch, Reference View and export: the cache's copy, else a decode with highlights recovered and stored, as a `DecodePolicy` asks | `decode.rs` |
| `decode_cache` | Disk cache of developed camera images, keyed by file identity, demosaic setting and build | `decode_cache.rs` |
| `camera_profiles` | DCP parsing and validation, camera transforms, RAWmakase's own profiles, profile library and camera matching, DNG temperature/tint | `dcp.rs`, `library.rs`, `open.rs`, `reference.rs` |
| `optics` | The lens correction model the renderer evaluates (vignetting, distortion and lateral CA as radial functions), and Adobe lens profiles (LCP) as data; depends only on `xml` | `mod.rs`, `lcp.rs` |
| `lens` | Readers that fill the `optics` model: the tables cameras embed in their RAWs, imported Adobe LCPs and lateral CA measurement, plus profile selection | `embedded.rs`, `lcp.rs`, `auto_ca.rs`, `choice.rs` |
| `rendered` | Developed pixels as values: an output image, its histogram and clipping overlay, which the renderer produces and export and watermarks use; depends on nothing | `rendered.rs` |
| `model` | What an edit is, as values shared by the renderer, the catalog and file formats: the `Recipe` with its validation (`ValidRecipe`), saved versions and migration, panel switches, and the settings it is made of: Effects, Heal and Clone, Red Eye, Point Color, masks, Transform with Upright, white balance from metadata, the image space positions are kept in (`image_frame.rs`), the sliders as parameters, the rules an edit follows and the setting groups Copy Settings, Sync and presets use. Depends on camera profiles, lenses and optics, never on rendering | `recipe.rs`, `valid.rs`, `saved_format.rs`, `panels.rs`, `operators.rs`, `effects.rs`, `masks.rs`, `transform.rs` |
| `develop` | Rendering a recipe: geometry, color processing, the scene tone stage, curves, effects, local adjustments, detail rendering and the GPU port | `pipeline/`, `scene_tone/`, `quality/`, `geometry.rs`, `gpu/` |
| `xmp` | Namespace-aware Adobe settings parsing and application to recipes. Settings that ask for Auto measure the photo through `PhotoMeasures`, which `develop::Measures` provides, and packets take the crop as rendered from the caller, so XMP needs no rendering code | `parse.rs`, `apply.rs` |
| `raw_defaults` | Lightroom's Raw Defaults: the master and per-camera choices and a photo's starting settings. Above `presets`, whose library it reads | `raw_defaults.rs` |
| `presets` | Native JSON recipe presets, installed XMP collections, favorites and preset import | `native.rs`, `library.rs` |
| `storage` | RAW identity checks, application paths and atomic JSON writes. Saved-recipe versions are `model::saved_format`, the desktop session is `app::session` and legacy sidecar import is `catalog::legacy_sidecar` | `identity.rs`, `files.rs` |
| `export_settings` | Export choices as values: the Export dialog's settings and a photo's saved `ExportOptions` | `export_settings.rs` |
| `export` | JPEG/16-bit TIFF encoding, selected EXIF, sRGB ICC embedding, atomic output publication | `mod.rs`, `metadata.rs` |
| `ids` | Typed catalog row ids (`PhotoId`, `FolderId`, `RootId`, `CollectionId`), stored and serialized as their integers; a leaf, so edit resolution and export use them below the catalog | `ids.rs` |
| `metadata` | Photo metadata as values: descriptive fields (title, caption, copyright, creator, capture, location), keywords and camera settings; depends on nothing else | `metadata.rs` |
| `edit_session` | The edit of the photo open in Develop apart from the window: its bounded History (named steps, one per gesture, saved with the edit) and save state; no egui | `history.rs`, `save_state.rs` |
| `edits` | Which edit a photo develops with (saved, Lightroom or raw defaults), shared by Develop, Sync and Export; the catalog only reads the stored records | `edits.rs` |
| `catalog` | RAWmakase SQLite database, schema, photo/folder/collection models, edits, relinking and disposable preview cache | `schema.sql`, `models.rs`, `mod.rs`, `preview_cache.rs` |
| `catalog::db` | The catalog's one connection boundary: an opaque `Db` with checked reads, `snapshot` for consistent reads, `write` as the only way to write, and `with_lightroom` for import; statements are `Sql` (portable) or `SqliteSql` (Lightroom), shape-checked at compile time; values go through the catalog's own `ToValue`/`FromRow` (`catalog::value`) | `mod.rs`, `sql.rs` |
| `catalog::lightroom` | Read-only Lightroom snapshot import | `mod.rs`, `history.rs` |
| `catalog_session` | The open catalog and the lists read from it, with no window: opening it, reading it again, writes that must keep those lists in step (ratings, descriptive metadata, collections, virtual copies and their names), and the background reads that fill in capture times and photo info for photos added from folders. Above `photo` and `raw`, as reading a RAW's info opens it; `app::library` decides which photos are online and shows the lists | `mod.rs`, `backfill.rs`, `background.rs` |
| `lr_develop` | Best-effort conversion of Lightroom's serialized Develop settings into a recipe, through XMP; below the catalog, so edit resolution can use it | `lr_develop.rs` |
| `app` | Desktop editor state, UI, dialogs, background task coordination and presentation | Components described below |
| `platform` | OS integration: the Linux GVFS filesystem bridge, drives, the file manager, the browser and the desktop's text size | `network.rs`, `volume.rs`, `reveal.rs`, `web.rs`, `text_scale.rs` |
| `updates` | Release checks and self-update through fastframe-update; the notice itself is in `app` | `updates.rs` |
| `comparison` | Reproducible reference-image comparisons using the same develop APIs | `comparison.rs` |

`color` holds shared numeric primitives (matrices, sRGB transfer, HSV) and the tone-curve
model (`color::curve`). It depends on nothing else, so camera profiles, XMP and the
interface use it without depending on recipe or render orchestration. Presets and
camera profiles independently use storage's asset-directory policy; neither
library discovers its folders through the other.

## Desktop composition

`app/mod.rs` owns the `Editor`, initialization and lifecycle. Its fields remain
private. UI and workflow methods have visibility limited to the app module, one
file per tool, workflow or policy; the Develop adjustment panels currently share
`inspector.rs`. The [code map](code-map.md#desktop-application) lists them.

`Editor` deliberately remains the coordinator for state shared across panels.
A panel should call the domain API responsible for an operation, rather than
implementing file formats, SQL or pixel processing itself. Thumbnail decoding
belongs to `raw`; library browsing does not call into the preview renderer.

Each load/render task owns its generation, cancellation token and lifecycle.
The single-slot mailbox replaces pending work. The app discards obsolete results,
and a failure only finishes its owning task. Worker messages use named fields;
render stages are enums, independent of user-facing status text. Exports capture
their recipe before starting, and overwrite confirmation holds the current photo.

Document reset clears its edit history and decoded images together; a catalog
photo's History then comes back from the catalog with its edit. A frame's
history transaction is bound to the load generation, so navigation during drawing
cannot record the previous photo's edits against the new photo. Preset-browser
preferences survive navigation; hover previews and compatibility results do not.
Session preference writes have an explicit destination; UI tests disable them or
inject a temporary file, without changing the process-wide environment.

## Boundaries to preserve

- Dependencies between top-level modules are listed in
  `scripts/deps-allowed.txt`, and `scripts/deps.py check` (CI and `make check`)
  fails on a new one or on a cycle; add a line only when the new dependency
  points down the intended layering. `scripts/deps-closures.txt` lists what some
  modules must never reach, even through others: the edit session never reaches
  LibRaw, the renderer or the app, and the renderer never reaches the app or
  export. The check prints the chain that breaks a rule.
- The model, file formats, catalog and control protocol are crates, so Cargo
  rules out a dependency back on the app. `scripts/crate-closures.sh` (CI and
  `make check`) fails when one of them gains the app, a GPU or GUI crate, or a
  LibRaw or Little CMS binding; CI also builds and tests them on a machine
  without LibRaw or Little CMS. The catalog's bundled SQLite is intended.
- Keep `eframe`, `egui` and native chooser code in `app`. The CLI must be able to
  use domain operations without creating an editor or UI context. The crate still
  links its existing GUI dependencies; this is module separation, not a separate
  headless build feature.
- The catalog owns its connection, and only `catalog::db` sees it (issue #341).
  Catalog code reads through `Reads` and writes inside `Db::write` in portable
  SQL (`sql!`), with values in the catalog's own types, so a second backend
  could be added behind `Db` without touching domain code; a test fails on
  `rusqlite` anywhere else but the preview cache and the check of a Lightroom
  file before import. Lightroom import runs inside `Db::with_lightroom`, the
  only place `SqliteSql` runs; do not expose the connection publicly or put
  Lightroom-specific queries back into general catalog operations. Portable
  SQL stays portable: CI prepares every `sql!` statement on Postgres against
  `schema.postgres.sql` (no Postgres backend ships), so an `INSERT OR`, a
  reserved word or a SQLite-only function fails CI the day it is
  written.
- The app names and reopens a catalog by its `CatalogLocation`, never by a
  path: sessions, autosave, Sync and folder jobs carry it, and behaviour that
  only makes sense for a file (its size, revealing it, the path `session.json`
  keeps) matches on `CatalogLocation::File`.
- Raster masks (`MaskShape::Bitmap`) hold only a content ID. The pixels live in
  `model::storage::mask_assets`, a process-wide store: a generated raster is
  *unsaved* until a catalog write stores it, saved ones are read back through the
  loader the app installs from the open catalog (`Catalog::mask_asset_loader`) and
  can be evicted. Rendering resolves rasters through it and fails, never renders an
  empty mask, when one cannot be provided. `edit_rows` stores a save's unsaved
  rasters in the same transaction as the rows naming them and rejects an edit
  whose references do not exist; rasters are never collected. Catalogs that keep
  them are format version 2 or later (see [catalogs](catalogs.md)).
- A photo's edit (its recipe, export options, identity and edit time, spots
  and masks, and History) is written only by `catalog::edit_rows`: a checked
  save or clear, an exact copy for a virtual copy, removal with one, and the
  format upgrade's migration of every stored edit.
- The settings earlier releases chose an engine or operator with are
  `model::saved_format::OBSOLETE_SETTINGS`, its one owner. Reading any `Recipe`
  drops them from its unknown settings (and nothing else), so catalog edits,
  History, snapshots, presets, sidecars and the session all migrate on the way
  in; `catalog::format_upgrade` removes them from stored rows after a backup,
  in the same transaction as the first write of a recipe into a catalog of an
  earlier format (`Catalog::write_recipes`, from saving edits and snapshots),
  once that write's checks have passed.
- Parsing XMP produces settings, while application validates and resolves a
  recipe. Collection discovery and favorites belong in `presets`.
- Validate recipe changes at domain boundaries. Saved format versions and
  migrations are shared by native presets, catalog edits and legacy sidecars. Every saved recipe renders
  with the one engine, and fields it lacks take the recipe defaults, so changes to
  recipe defaults must account for older saved edits.
- Keep original RAWs and Lightroom sources read-only. Preserve unsupported source
  data, legacy sidecars, no-clobber publication, ICC/EXIF handling and
  temporary-file write behavior.
- Rendering math stays in `develop` and `camera_profiles`. Fit previews, regions
  and exports must continue to share the relevant processing paths. What the
  [scene tone stage](scene-tone-stage.md) measures of a photo comes from one
  measurement copy of the full-resolution photo (`Toned::measured`, cached per
  image), never from the region or preview size being rendered.
- Keep OS integration in `platform` and native decoding/color management in
  `raw`. Use the existing asset-path policy instead of duplicating environment
  variable handling in feature modules.

## Invariants the code relies on

- Dialog and relink actions are exhaustive enums carrying their target ID, so a
  new menu item cannot silently become another operation through a catch-all
  branch.
- XMP application is a sequence of named profile, basic, white-balance, color,
  curve, grading, effects and geometry stages. Each stage mutates a private
  recipe; unsupported settings and final validation must pass before that recipe
  is returned.
- Bare relative filenames resolve their parent to `.` consistently for JSON
  writes, catalog creation/import, RAW enumeration and image export.
- Saved-recipe migration validates the envelope and recipe object before
  mutation; malformed legacy recipes return errors rather than panicking.
- Export defaults are validated before catalog writes and on restoration.
  An edit the catalog cannot load is protected: it never enters autosave and is
  not replaced. Failed saves remain pending for retry.
- Autosave writes on its own thread and catalog connection, so a slow disk
  cannot stall editing. Saves before navigation and close wait for it, then
  save synchronously.

## Compatibility

The library API is internal: the RAWmakase binary, its examples and its tests are
its only clients, so modules and functions change freely with them and nothing is
kept for outside callers. `src/lib.rs` exports only the modules they use; the rest
(`catalog_session`, `decode`, `demosaic`, `edit_session`, `platform`, `time`)
are `pub(crate)`, and the desktop app exposes only `app::run`, so the
compiler reports what in them nothing uses. The compatibility surface is the saved data: recipe and
preset envelopes are versioned and migrated by `model::saved_format` (written as
version 11, which releases before the one engine refuse); a catalog opens at
versions 1 to 3 (others are refused, the file left unchanged), is upgraded to 3
before a recipe is stored in it, and its schema only ever gains tables, applied
idempotently on open; see [catalogs](catalogs.md#sqlite-format-versions-1-to-3).

## Validation

`make check` runs the standard suite (see the [README](../README.md#development)).
Photographic checks, measured results and dated validation records live in
[validation.md](validation.md), the per-stage measurement pages and
[macOS and Lightroom validation](macos-lightroom-validation.md).

## Preview compute backend

`develop::PreviewRenderer` owns the optional GPU processor and fallback state.
The desktop renderer creates it on its worker thread. Headless UI tests and the
legacy `worker::renderer` entry point remain CPU-only. Domain color processing,
RAW development and exports use the existing CPU implementation.

`develop::gpu` uses portable WGSL compute through wgpu: Metal on macOS and Vulkan
on supported Linux drivers, including NVIDIA and AMD. On the desktop it shares the
UI's wgpu device (eframe's), which the app creates with the storage limits previews
need (`gpu::required_limits`) and a high-performance adapter. On Linux the window's
adapter is the first GPU the system lists instead: the display's GPU, or the discrete
one under `prime-run` or `DRI_PRIME=1`, because a render-offload GPU can claim it
presents to the window and then fail to configure it. OpenGL and software adapters
are not used for compute. Every GPU call runs inside its own validation,
internal and out-of-memory error scopes, so a failure disables the preview GPU
without reaching the UI. Headless callers (tests, the benchmark) create a device of
their own. It uses ordinary 32-bit storage buffers, Rgba8Unorm storage textures and
compute workgroups, without vendor extensions. The current hardware validation is
Apple M1 Pro; Linux GPU vendors require validation on those machines before claiming
equivalent performance or numerical precision.

The per-pixel color and tone stage runs on the GPU for previews
(`gpu/develop.wgsl`, fed by `pipeline/pixel_params.rs`), on the stage cache's
samples, for every recipe resolved with a camera profile; unresolved recipes and
all exports use the CPU stage, which is the reference.
When the GPU renders a preview, the developed pixels stay on the device and
`gpu/present.wgsl` finishes them into a texture that egui draws directly (registered
as a native texture by the render worker): sharpening, vignettes and grain, the
clipping overlay, the monitor profile (a 52³ lattice sampled from lcms once per
profile, interpolated trilinearly) and 8-bit encoding, in the CPU reference's order.
The histogram is counted in the same pass and the Navigator copy is reduced on the
GPU; only the histogram (3 KB) and, for catalog photos, a 640-pixel library
thumbnail are read back. Nothing is converted or uploaded on the UI thread. Recipes
the port does not cover and machines without a usable adapter render
on the CPU, and the worker then prepares the display bytes, histogram and reduced
copies before the UI uploads the texture. Fit and zoomed-out previews render from a
resolution pyramid at output size (see
[preview performance](preview-performance.md#resolution-pyramid)). Full-resolution
regions preserve the sharpening halo before cropping. Lanczos3 resizing of full
renders remains on the GPU for readback callers. Export remains on the CPU. The
status line says `GPU` for presented frames, `GPU finish` when only finishing used
the GPU, and `CPU` otherwise.

For the same recipes, the stages before the per-pixel stage also run on the device
when the photo fits its buffer limits (`gpu/resident.rs`): the photo or pyramid level
is kept there, and regions are sampled there through geometry, lens correction and
noise reduction. Only the reduced input of the Shadows/Highlights map is read back;
the map (with Clarity's gain) is built on the CPU, and the scene tone stage reads it
in `develop.wgsl`. Texture's image is made on the CPU, and a mask's Clarity or
Texture renders the whole preview on the CPU. A failure in these stages turns off
only this path for the session.

Presented textures are kept per view (the whole photo, or a 100% region) and size,
two of each, so a slider moving at 100% alternates between the reduced preview's and
the full region's textures without allocating. Each texture carries a generation that
changes whenever it is written; the worker reuses a finished frame when switching
views only while its texture still holds it.

Buffers are reused by dimensions, limited by the adapter and a 1 GiB aggregate
buffer budget. No compatible adapter, oversized buffers, allocation/validation
errors and readback failures fall back to CPU. After a GPU failure the preview
renderer releases the backend and avoids retrying it for every edit. Restarting
creates a new backend. GPU execution already submitted cannot be interrupted;
obsolete results are discarded before they are published. Full-quality CPU stages check cancellation
within pixel/row/column work. Pending jobs still coalesce in a single-slot mailbox.

There is no separate interactive draft. Fit renders the current pipeline from the
smallest pyramid level at or above the physical viewport size on every change;
at 100% a reduced region from the pyramid is published first (as a draft stage),
then the full-resolution region. The single-slot mailbox and cancellation keep
continuous editing on the latest change.

## Live application control

`app::commands` defines transport-independent application operations, target
checks, parameter units and explicit results. `Editor` executes each mutation
inside its generation-bound edit transaction, then answers that request.
`app::automation` owns the input queue and adapter lifecycles. Its MIDI device
adapter translates controls, while its independent socket adapter authenticates,
bounds and expires requests, each with a separate reply channel. These adapters never write catalog edits or render pixels themselves.
Headless test editors do not start listeners or touch connection/configuration
files; only desktop initialization starts them.

External parameter commands default to global scope. MIDI may resolve the active
mask; explicit mask commands require generation and revision guards because
recipe masks have no persistent IDs. Output jobs capture the photo and recipe
revision and call `export::job`; successful queueing is distinct from published
output. The protocol and CLI are documented in [External control](automation.md).

The workspace's `crates/rawmakase-protocol` holds what the app and its clients
agree on: the protocol version, the `control.json` endpoint and the
data-directory policy. `tools/rawmakase-ctl` is the client library and the
standalone binary; the built-in `rawmakase control` subcommand and the MCP
server depend on it rather than including its source. Neither crate depends on
the app, so the client builds without GUI, GPU or native libraries. The CLI
performs transport and polling only, without creating an Editor or GUI context.

`rawmakase mcp` is a stdio adapter built with the official Rust MCP SDK. It shares
connection discovery and request/reply validation with the command-line client,
and sends the same guarded commands through the local socket. It owns tool
schemas and temporary preview images, but no recipe state or editing logic.
Point-curve validation, history, auto adjustment and target checks remain in the
application command layer. MCP errors retain application error codes; output
jobs retain their captured edit revision. See [MCP](mcp.md) for setup and tools.

### Automation boundaries

Application commands return typed state snapshots and results; the socket adapter
encodes them as protocol JSON. A single action-name table supports parsing,
discovery and device mapping serialization. Gesture ownership belongs to the
application command layer, with separate MIDI and socket scopes.

The automation owner manages the bounded queue and local socket independently
of MIDI. Each MIDI device owns its mapping and connection state, using a shared
profile registry. Further work can route remaining UI shortcuts through these
operations and share parameter descriptors with all editing panels.
The protocol crate can also hold the request types, so MCP schemas reuse
them instead of declaring their own. These follow-ups do not require a new editing engine or MIDI-specific commands.

## Camera white-balance calibration

Calibration facts stay in the model: DNG metadata or exact camera table rows
combined with Sony per-file daylight facts read by the native adapter. Camera
profiles apply them after signature matching. XMP transports the explicit
white-balance conversion version; it does not implement calibration rules.
See [camera parity](camera-parity.md) for the offline Adobe audit and references.
