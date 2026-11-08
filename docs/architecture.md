# Architecture

Status: Current. How RAWmakase is divided, which way dependencies point, how work
moves between threads, and the rules and contracts every change keeps. For
file-by-file navigation, runtime flows, storage locations and feature entry points,
see the [code map](code-map.md); for the words used here, the
[glossary](glossary.md). Feature details live in each module's `//!` docs.

## Crates and dependency direction

RAWmakase is a Cargo workspace. The app crate (`src/`) is the desktop editor and
the command line; everything it composes is a crate below it, and dependencies
point only down:

```mermaid
flowchart TD
    app["rawmakase (app, CLI)"]
    export[rawmakase-export]
    decode[rawmakase-decode]
    catalog[rawmakase-catalog]
    engine[rawmakase-engine]
    interop[rawmakase-interop]
    model[rawmakase-model]
    raw[rawmakase-raw]
    tiff[rawmakase-tiff]
    cancel[rawmakase-cancel]
    fs[rawmakase-fs]
    protocol[rawmakase-protocol]
    ctl[rawmakase-ctl]
    remote[rawmakase-remote]
    inference[rawmakase-inference]

    app --> export & decode & catalog & engine & interop & model
    app --> ctl & remote & inference & fs
    export --> decode & engine & interop
    decode --> engine & raw
    catalog --> interop & fs
    engine --> model
    interop --> model
    model --> tiff & protocol
    raw --> tiff & cancel
    ctl --> protocol
    remote --> inference
```

(Edges implied by others are left out: every crate above the model also depends
on it directly.)

| Crate | Owns | Builds without |
| --- | --- | --- |
| `rawmakase-model` | What an edit is, as values: the recipe and its settings, camera and lens profiles, colour primitives, DNG hints, output buffers, file identity and storage helpers | The RAW decoders, the renderer, wgpu, the GUI |
| `rawmakase-interop` | The file formats and presets over the model: XMP, Lightroom's Develop settings, presets, raw defaults, EXIF and JPEG, export settings, and which edit a photo develops with (`edits`) | The same |
| `rawmakase-catalog` | The SQLite catalog, Lightroom catalog import, file operations, and the stored edit records `edits` resolves | The same |
| `rawmakase-engine` | The renderer (`develop`): the CPU pipeline, which is the reference, and the GPU preview port | The RAW decoders, the catalog, the GUI |
| `rawmakase-raw` | RAWmakase's own RAW decoders ([RAW decoding](raw-decoding.md)) | Everything but `rawmakase-tiff` and `rawmakase-cancel` |
| `rawmakase-tiff` | The bounded TIFF reader DNGs and RAW containers use; no RAW knowledge | Everything |
| `rawmakase-fs` | What moving a user's file needs of the file system, policy-free: exclusive renames, file ids, exact listings, name comparison, containment, volume facts and a copy's steps, one file per system, and whether a program has a SQLite database open, with the unsafe system calls the model used to hold. The catalog's file operations and the app's Trash use it | Every other workspace crate; the value, file-format, rendering, decoding and export crates never reach it |
| `rawmakase-cancel` | `Cancel`, the borrowed request to stop long-running work, which its owner's flag sets: every crate that does such work takes one, and owners keep their flags | Everything |
| `rawmakase-decode` | Opening and decoding photos: the only crate that reaches `rawmakase-raw`; colour management, demosaicing, rendered files (JPEG … HEIF), the full-size decode and its cache, AI Denoise's packing and assembly, Camera Look fitting | C imaging libraries, the catalog, export, the GUI |
| `rawmakase-export` | Developing and writing exports, with their watermarks | The app, a window or GPU backend |
| `rawmakase-inference` | Running the selection models and AI Denoise's model through a lazily loaded ONNX Runtime: pixels in, coverage or denoised camera RGB out | Every other workspace crate |
| `rawmakase-remote` | The optional worker binary that runs the same models on another computer, and the app's client for it, over a bounded TCP protocol | Everything but inference |
| `rawmakase-protocol` | What the app and its control clients agree on: protocol version, `control.json` endpoint, data folder | Everything |
| `rawmakase-ctl` (`tools/`) | The control client, used by `rawmakase control`, `rawmakase mcp` and the standalone `rawmakase-ctl` | The app |

No value, catalog or rendering crate can reach `rawmakase-inference`, so saved masks
render without any inference installed and a photo with AI Denoise renders from a
result handed to it. The app crate re-exports these crates' modules at its root
(`crate::model`, `crate::xmp`, `crate::catalog`, `crate::develop`…), so paths read
the same on either side; parsing, persistence and rendering never import the
desktop UI. Inside the app crate, `scripts/deps.py` checks the same direction
between top-level modules (see [boundaries](#boundaries-to-preserve)).

## Module ownership

| Module | Owns | Main extension points |
| --- | --- | --- |
| `camera_data` | A RAW's metadata, the demosaic and decode choices and the decoded camera-space image, as values with no native code | `camera_data.rs` |
| `raw` | RAW files: reading metadata, decoding into `camera_data` images, oriented embedded thumbnails, with RAWmakase's own decoders (`crates/rawmakase-raw`, see [RAW decoding](raw-decoding.md)) | RAW decoding |
| `color_management` | The profile each export colour space embeds (`output_profile`: sRGB's, and the CC0 Adobe RGB and ProPhoto profiles) and the transform of 8-bit sRGB to a monitor profile, in pure Rust (moxcms reads the profiles) | `color_management.rs` |
| `photo` | Opening a photo: `open_source` is the one entry point, sending a RAW to `raw` and a JPEG, PNG, TIFF, WebP, AVIF, HEIF or JPEG XL to `raster`; a RAW's decoder facts from `raw`, then the camera's lens tables, a DNG's profile and hints, and the imported lens profiles that fit | `photo.rs` |
| `raster` | Opening a JPEG, PNG, TIFF, WebP, AVIF, HEIF or JPEG XL: decoded upright and converted from its embedded ICC profile or CICP colour to 8-bit sRGB for previews, or to linear ProPhoto f32 for Develop (pure Rust: `image`, `jxl-oxide`, `rav1d` without assembly, `moxcms`; a HEIC's pixels come from the system's ImageIO on macOS, Windows Imaging Component on Windows, and elsewhere the system's libheif, opened at run time; without it a HEIC is listed and decoding says what to install) | `raster.rs`, `raster/` |
| `demosaic` | RAWmakase's own Bayer and X-Trans demosaicing of the unpacked sensor data | `demosaic.rs` |
| `dng`, `tiff` | A DNG's rendering hints (embedded profile, baseline exposure, crop, opcodes) and the bounded TIFF reader behind them (`crates/rawmakase-tiff`, a leaf crate with no RAW knowledge, re-exported as `model::tiff`) | `dng.rs`, `rawmakase-tiff/src/lib.rs` |
| `decode` | A photo's full-size image, made one way for Develop, prefetch, Reference View and export: the cache's copy, else a decode with highlights recovered and stored, as a `DecodePolicy` asks | `decode.rs` |
| `decode_cache` | Disk cache of developed camera images, keyed by file identity, demosaic setting and build; the app shares one live size limit and folder across Develop, prefetch, Reference View, export and preview builds, persisted in its session | `decode_cache.rs` |
| `rawmakase_decode::ai::denoise` | AI Denoise's side of decoding, with no model knowledge: the visible mosaic packed as the model takes it (`PackedBayer`, RGGB; X-Trans rebuilt as RGGB by `Raw::packed_bayer`), the camera image assembled from the model's output (`assemble`), and `AiBase`, the trait export and the app ask for a photo's complete AI source through (`AiService`; `Refusing` for callers without one) | decode's `ai/denoise.rs` |
| `ai_cache` | Disk cache of AI Denoise results: its own subfolder, extension and size cap (10 GB by default) over the decode cache's trim, purge and relocation (`decode_cache::Namespace`); half-precision payload with the gain applied, checked finite when stored and loaded | decode's `ai/cache.rs` |
| `camera_profiles` | DCP parsing and validation, camera transforms, RAWmakase's own profiles, profile library and camera matching, the community profile catalogue (`data/camera-profiles`: parsing, validation, matching, byte and identity verification, and its generator), `.cube` LUTs imported as look profiles, fitted camera looks as profiles (built only; fitting is `camera_look`'s), DNG temperature/tint | `dcp.rs`, `library.rs`, `catalogue.rs`, `open.rs`, `cube_look.rs`, `fitted.rs`, `reference.rs` |
| `camera_look` | Fitting Camera Look (fitted) to a RAW's embedded JPEG: preview choice, pairing with the photo's render, solvers and acceptance on held-out tiles, and the default fit of photos without colour data through its fit cache. In `rawmakase-decode`, as it opens RAWs and decodes their previews; the model only builds and checks what it finds, and the catalog, `xmp` and presets never fit (the app and export pass the default in with the photo's profiles) | `camera_look/` |
| `optics` | The lens correction model the renderer evaluates (vignetting, distortion and lateral CA as radial functions), Adobe lens profiles (LCP) as data, and Lensfun calibrations as data with their evaluation for a photo; depends only on `xml` | `mod.rs`, `lcp.rs`, `lensfun.rs` |
| `lens` | Readers that fill the `optics` model: imported Adobe LCPs, Lensfun's database (read, matched, installed) and lateral CA measurement, plus profile selection; an edit's Lensfun lens carries its own copy of the calibration, so rendering never reads the database. The tables cameras embed in their RAWs are read next to the RAW containers, by the decode crate's `raw::lens` | `lcp.rs`, `lensfun/`, `auto_ca.rs`, `choice.rs` |
| `rendered` | Developed pixels as values: an output image, its histogram and clipping overlay, which the renderer produces and export and watermarks use; depends on nothing | `rendered.rs` |
| `model` | What an edit is, as values shared by the renderer, the catalog and file formats: the `Recipe` with its validation (`ValidRecipe`), saved versions and migration, panel switches, and the settings it is made of: Effects, Heal and Clone, Red Eye, Point Color, masks, Transform with Upright, white balance from metadata, the image space positions are kept in (`image_frame.rs`), the sliders as parameters, the rules an edit follows, the setting groups Copy Settings, Sync and presets use, and Camera Raw's keys for the sliders (`camera_raw.rs`). Depends on camera profiles, lenses and optics, never on rendering | `recipe.rs`, `valid.rs`, `saved_format.rs`, `panels.rs`, `operators.rs`, `effects.rs`, `masks.rs`, `transform.rs` |
| `develop` | Rendering a recipe: geometry, color processing, the scene tone stage, curves, effects, local adjustments, detail rendering and the GPU port | `pipeline/`, `scene_tone/`, `quality/`, `geometry.rs`, `gpu/` |
| `xmp` | Namespace-aware Adobe settings parsing and application to recipes. Settings that ask for Auto measure the photo through the model's `PhotoMeasures` (`model::measures`), which `develop::Measures` provides, and packets take the crop as rendered from the caller, so XMP needs no rendering code | `parse.rs`, `apply.rs` |
| `raw_defaults` | Lightroom's Raw Defaults: the master and per-camera choices and a photo's starting settings. Above `presets`, whose library it reads | `raw_defaults.rs` |
| `presets` | Native JSON recipe presets, installed XMP collections, favorites and preset import, and a preset's application to one photo, which Develop and the Library's batch share | `native.rs`, `library.rs`, `photo.rs` |
| `storage` | RAW identity checks, which files are photos and the rendered formats' facts (`RasterFormat`: extensions, MIME types, names; the codecs are `raster`'s and `export`'s), application paths and atomic JSON writes. The file system primitives moving a user's files needs are `rawmakase-fs`'s. Saved-recipe versions are `model::saved_format`, the desktop session is `app::preferences::session` and legacy sidecar import is `catalog::legacy_sidecar` | `identity.rs`, `files.rs`, `formats.rs` |
| `export_settings` | Export choices as values: the Export dialog's settings, Rename Photos' remembered choices (`RenameSettings`), the user's saved file-name templates (`SavedTemplates`), a photo's saved `ExportOptions`, and the naming templates (`NameTemplate`: Lightroom's presets and Filename Template Editor templates, `TokenTemplate`) exports and renames name files by, from the `NameFields` taken once per export | `export_settings.rs`, `export_settings/tokens.rs`, `export_settings/rename.rs`, `export_settings/saved.rs` |
| `export` | JPEG, 16-bit TIFF and PNG encoding, selected EXIF with the colour space's ColorSpace and interoperability index, ICC embedding of the pixels' colour space (taken from the `ExportImage`, never from a second setting), the sRGB conversion of watermark colours, atomic output publication | `mod.rs`, `metadata.rs` |
| `ids` | Typed catalog row ids (`PhotoId`, `FolderId`, `RootId`, `CollectionId`), stored and serialized as their integers; a leaf, so edit resolution and export use them below the catalog | `ids.rs` |
| `metadata` | Photo metadata as values: descriptive fields (title, caption, copyright, creator, capture, location), keywords and camera settings; depends on nothing else | `metadata.rs` |
| `edit_session` | The edit of the photo open in Develop apart from the window: its bounded History (named steps, one per gesture, saved with the edit) and save state; no egui | `history.rs`, `save_state.rs` |
| `edits` | Which edit a photo develops with (saved, Lightroom or raw defaults), shared by Develop, Library previews, Sync and Export: `resolve` at once, or a `Start` chosen once and resolved later; callers keep their own error and warning policies. The catalog only reads the stored records. In `rawmakase-interop`, so export resolves edits without the catalog | `edits.rs` |
| `catalog` | RAWmakase SQLite database, schema, photo/folder/collection models, edits, relinking, the default Creator and Copyright of photos added from folders (below the camera's own, which the app's reader finds, as the catalog can't reach the RAW decoders), Synchronize Folder's and Find on Disk's engine (`folder_sync`: scan, matching, apply, family digest; named apart from `app/library/sync.rs`, which is Sync Settings) and disposable preview cache | `schema.sql`, `models.rs`, `mod.rs`, `folder_sync.rs`, `preview_cache.rs` |
| `catalog::db` | The catalog's one connection boundary: an opaque `Db` with checked reads, `snapshot` for consistent reads, `write` as the only way to write, and `with_lightroom` for import; statements are `Sql` (portable) or `SqliteSql` (Lightroom), shape-checked at compile time; values go through the catalog's own `ToValue`/`FromRow` (`catalog::value`) | `mod.rs`, `sql.rs` |
| `catalog::lightroom` | Lightroom snapshot import, of a Lightroom Classic catalog or a Lightroom library (its photos in Adobe's cloud added where Lightroom downloads their originals), and the one write to another program's files: Store Album Locally marked in a Lightroom library's catalog, as Lightroom marks it, and an album made for photos in no album, as Lightroom makes one, for Lightroom to download those originals | `mod.rs`, `history.rs`, `library.rs`, `library_db.rs`, `downloads.rs` |
| `catalog::file_ops` | The one owner of changing where a photo or folder is or what it is named, on disk and in the catalog (#370, #556): planning against a snapshot, the refusals, destination conflicts, names, which companion files go with a photo (by `catalog::sidecar`'s candidate rule), the journal, the operation lock, execution and recovery. Names photos from `interop::export_settings::NameTemplate`, the one owner of naming templates. Reaches the file system through its `Disk` seam over `rawmakase-fs`, the Trash through a `Trash` the app passes in | `plan.rs`, `execute.rs`, `recover.rs`, `journal.rs`, `rename.rs`, `tree.rs` |
| `catalog_session` | The open catalog and the lists read from it, with no window: opening it, reading it again, writes that must keep those lists in step (ratings, descriptive metadata, collections, virtual copies and their names, stacks), behind private fields: the lists and catalog are read through accessors, and the writes that change no list (edits, History, snapshots, camera sequences) are methods of their own, the background reads that fill in capture times and photo info for photos added from folders, and the throttled checksum reader, whose results a structural generation guards. Above `photo` and `raw`, as reading a RAW's info opens it; `app::library` decides which photos are online and shows the lists | `mod.rs`, `backfill.rs`, `background.rs`, `checksums.rs`, `stacks.rs` |
| `lr_develop` | Best-effort conversion of Lightroom's serialized Develop settings, Lightroom Classic's Lua or a Lightroom library's XMP, into a recipe, through XMP; below the catalog, so edit resolution can use it | `lr_develop.rs` |
| `lr_library` | A Lightroom (not Classic) library's documents, read as data for the catalog's import, with its own MessagePack decoder, and a document's local annotation marked Store Album Locally with every other byte kept | `lr_library/` |
| `app` | Desktop editor state, UI, dialogs, background task coordination and presentation | Components described below |
| `platform` | OS integration: the Linux GVFS filesystem bridge, drives, the file manager, the browser, the system Trash file operations get (`trash::SystemTrash`, failing closed, and whether a drive has one; no policy), and the display's color management (macOS surface tagging and EDR, the Windows display profile), and whether a RAW on a network volume is read for its embedded preview alone (`preview_reads`) | `network.rs`, `volume.rs`, `reveal.rs`, `web.rs`, `trash.rs`, `display.rs`, `preview_reads.rs` |
| `diagnostics` | The system report a bug report asks for (Report an Issue…, `rawmakase system-info`): this build, the system probed once per process and what the window knew, as plain snapshots, with no path, user, file, catalog or profile name | `diagnostics.rs` |
| `updates` | Release checks and self-update through fastframe-update; the notice itself is in `app` | `updates.rs` |
| `ai_denoise` | AI Denoise's service, shared by the desktop app and the command line: the queue, the worker and its ONNX session, job keys, the finished-result lookup, `AiBase` for export; `CommandLine` for `render` and `render-batch`, and the `rawmakase denoise` development command. No egui and no `app`: the open photo's result and the Loupe's loader are `app::develop::ai_denoise`'s, and the app passes the user's cache in | `mod.rs`, `command_line.rs`, `bench.rs` |
| `model_install` | Installing the model packs (Select Subject's, Sky's, AI Denoise's): their folders, the receipt of checked files, downloads that resume, the lock across instances, and the runtime folder; the threads and the drawer are `app`'s | `mod.rs` |
| `lensfun_install` | Downloading, checking, publishing and removing Lensfun's lens database on request (never bundled, never fetched unasked); reading it is `lens::lensfun::installed`'s, the panels and the job thread are `app`'s | `lensfun_install.rs` |
| `comparison` | Reproducible reference-image comparisons using the same develop APIs | `comparison.rs` |

`color` holds shared numeric primitives (matrices, sRGB transfer, HSV), the export colour
spaces (`color::output::OutputSpace`: each space's matrix from linear sRGB, transfer curve
and luminance, the one owner of those facts for the engine, export settings and export)
and the tone-curve model (`color::curve`). It depends on nothing else, so camera profiles, XMP and the
interface use it without depending on recipe or render orchestration. Presets and
camera profiles independently use storage's asset-directory policy; neither
library discovers its folders through the other.

## Threads and jobs

The UI thread owns the `Editor` and draws every frame. Long work (decoding,
rendering, opening or importing a catalog, file operations, autosave) runs on
worker threads, which report back, mostly through named `worker::Event`s and
otherwise through a channel of the service's own (autosave, the catalog session's
background readers), and ask for a repaint; the UI thread applies a result only if
it is still wanted. Some short catalog work stays on the UI thread on purpose: the
synchronous save before navigating to another photo (`Editor::flush`, after
waiting for the autosave in flight) and the catalog session writing what its
background readers found (`catalog_session::backfill`). Closing and quitting save
the Develop edit on the autosave thread instead and wait only until a deadline
(`Editor::flush_by`), so a stalled catalog cannot hold up closing on it. Two writes
before it still run on the UI thread without a deadline: a Library Copy Name or
metadata field being typed, and the session file ([shutdown](shutdown.md#where-the-code-stands)). One table, `app::work`, says for each kind of work whether closing waits for it, whether the status line lists it (and where its progress shows otherwise), whether it slows the Library's edited previews or pauses the checksum reader, and its step of the exit sequence; the close guard, the status line, preview pacing and quitting read it, each from the state that owns the work. Four kinds of worker exist:

- **One-off tasks** (`app::task::spawn`): opening a catalog, an import, a file
  operation, a fit, and Develop's analyses of the open photo (Auto, Upright, Auto
  Straighten, the targeted adjustment and Point Color samples). A panic still
  reaches the UI as a failure. The state that owns
  the work says it is running. Fits and downloads are owned by a `Task`, whose
  generation and cancel flag reject stale results. Catalog dialogs, opening and
  import, folder changes, batch edits and file operations hold
  `app::activity::Activity`, which allows one foreground operation at a time and
  keeps the busy state; how each of those checks that its result still applies is
  its own (a catalog location, the catalog session's structural generation).
- **Latest-only workers** (`app::worker::Latest`): the Develop loader, its full-size
  stage and prefetch, the renderer (with lanes for the edit and Before), Reference
  View, stand-ins, the Loupe and its zoom. A single-slot mailbox replaces pending
  work with the newest request; a panicking job does not end the thread.
- **Owned services with their own queue**: autosave (its own thread and catalog
  connection), AI Denoise (`ai_denoise::Service`: one queue and one worker for
  Develop, export and the CLI), the Library's edited-preview scheduler and thumbnail
  workers, and the catalog session's background reads (capture times, photo info,
  checksums).
- **Rayon pools** inside a job, for pixel work; background work such as the
  neighbour's prefetch uses a small pool at background priority
  (`raw::background_pool`).

Every worker, what it blocks on, how it stops and whether quitting waits for it is
in the [shutdown contract](shutdown.md); a change that adds a worker or changes
what one waits on updates it.

The rules every job must follow (known gaps are tracked in #808):

- **Inputs are captured once**, when the job is made, and the same values give its
  cache identity. A job never reads editor state while it runs.
- **Results are checked against the current state.** A generation (the task's, the
  load's or the catalog session's structural one) rejects cancelled, superseded and
  duplicate completions, and a file's `Identity` rejects a result for a file replaced
  meanwhile.
- **Every ending clears busy state**: success, failure, panic, cancellation and a
  worker that disconnects.
- **No unbounded waits.** Quitting signals every worker, then waits with a
  deadline (`task::wait_for`); nothing joins in `Drop`.

Each load/render task owns its generation, cancellation token and lifecycle.
The single-slot mailbox replaces pending work. The app discards obsolete results,
and a failure only finishes its owning task. Worker messages use named fields;
render stages are enums, independent of user-facing status text. Exports capture
their recipe before starting, and overwrite confirmation holds the current photo.
Choosing what to export commits Library typing and reads every chosen photo's
name fields from the catalog in one snapshot; what only the files have (the
camera's make and model, and a photo without a catalog's capture time and info)
is read by one background read per scope, started only when the file-name
template needs it. The dialog's Example and the planned names both come from
these fields; a read of a replaced or cancelled scope is dropped, and Export
waits for, or says it goes on without, what its names need.

Document reset clears its edit history and decoded images together; a catalog
photo's History then comes back from the catalog with its edit. A frame's
history transaction is bound to the load generation, so navigation during drawing
cannot record the previous photo's edits against the new photo. A control names
the History step it makes in the editor's `StepName`, passed to it explicitly
with the recipe; the frame hands the last name given to History as it finishes,
after ending a wheel scroll or control-surface turn still going as steps of
their own. Preset-browser
preferences survive navigation; hover previews and compatibility results do not.
Session preference writes have an explicit destination; UI tests disable them or
inject a temporary file, without changing the process-wide environment.

Folder-drop workers resolve the imported folder tree keys against actual local
locations (including mapped symlinks). The source keys travel with the reopened
Library result; the UI applies them after restoring layout and filters, so the
result's source cannot be replaced by the old view. Filesystem resolution stays
on the worker.

### Previews

How preview renders are scheduled; what runs on the GPU, its backends, budgets and
textures are described in [`develop::gpu`](../crates/rawmakase-engine/src/develop/gpu/mod.rs)'s
module docs.

There is no separate draft pipeline. Fit renders the current pipeline from the
smallest pyramid level at or above the physical viewport size on every change. While
an edit is slower to present than 40 ms, a reduced render from the pyramid is
published first as a draft stage: at 100% the region at half resolution or less, and
for the whole photo at an edge halved until it has at most a megapixel. The
single-slot mailbox keeps only the latest pending job. An edit of the same document
and view (`Editor::schedule_edit`, `Task::supersede`) lets the running render finish,
and its frame shows with the settings it was asked for, so a GPU slower than the
slider still shows frames during a drag; any other render, and the next view or
photo, cancels the running ones, and a result older than the one shown is dropped.

`develop::PreviewRenderer` owns the optional GPU processor and fallback state.
The desktop renderer creates it on its worker thread. Headless UI tests and the
legacy `worker::renderer` entry point remain CPU-only. Domain color processing,
RAW development and exports use the existing CPU implementation, which is the
reference: the GPU port covers recipes resolved with a camera profile, and
everything else renders on the CPU. Any GPU failure falls back to the CPU and turns
the failing path off for the session. GPU work already submitted cannot be
interrupted, so obsolete results are discarded before they are published;
full-quality CPU stages check cancellation within pixel, row and column work.

### AI Denoise

AI Denoise ([rendering quality](rendering-quality.md#ai-denoise)) has one service, `ai_denoise::Service` (a root module, so the command line uses it without the app): one
queue and one worker thread for Develop, export and the CLI (one service per
command, so `render-batch` loads the model once, with the user's cache in the folder
Preferences chose, as the app keeps it). A job's
inputs are captured once as its key (the file's identity, the model's SHA-256, the
adapter version, the provider and the build), which also names its cache entry; a
request for a key already queued or running waits for that job. Develop's requests
run the tiles covering the 100% view first and publish one partial source after
them, then the final one; each publication is a new `AiSource`, so the retouch
cache, the pyramid and the GPU upload see a new source. A non-finite tile on the GPU
discards the attempt, which reruns on the CPU under the CPU's key with every job
queued behind it, and the GPU is not used again in the session. A request carries the
file's identity when its plain image was decoded (`AiBase::get` takes it too), and
Develop's request is keyed by it without reading the file on the UI thread; the
worker reads the file before and after the job, and a request joined to a job of
the same file under another path (a hard link) is checked against its own file as
each result is handed to it: a file changed since, before or during the job, cached
result or not, gets no result, so a photo is never denoised from another file. A result is rejected when the model
changed during the job, and an update for a plain image that is no longer the open
photo's is dropped. A panic in a job's run fails that job; one while handing a result
to a listener fails that listener alone; and if the worker thread dies anyway, the job
it held and every queued one fail, so no Develop request or export waits for it, and
the next request starts a new worker. The Detail panel's switch and Amount are ordinary edits (one
History step each); a result arriving is not one. `app::develop::panels::inspector::ai_denoise` draws
the panel's controls, credit and install offer; the model installs as
`model_install::Pack::Denoise`, as the selection models do.

## Data flow

The [code map](code-map.md#how-an-operation-moves-through-the-app) follows each
operation step by step; in terms of the crates:

- **Opening a photo.** The Library (`app::library`) decides whether Develop may
  open it (below). Its starting edit is resolved from the catalog's record (`edits::resolve`:
  saved edit, Lightroom edit or raw defaults); the loader opens it through
  `rawmakase-decode` (`photo::open_source`, then the embedded preview, the half-size
  and the full-size decode through the decode cache) and the renderer develops it
  with `rawmakase-engine`.
- **Editing.** A panel changes the recipe inside the editor's frame
  (`edit_session`): the frame records one History step and marks the edit for
  saving. The renderer gets the captured recipe and view; autosave writes the edit
  through `catalog::edit_rows` on its own thread.
- **Exporting.** The export job captures each photo's recipe and options, decodes
  with `DecodePolicy::Export`, renders on the CPU, and `rawmakase-export` encodes and
  publishes the file atomically. With AI Denoise on it asks the service for the
  photo's complete AI source first.
- **Settings in and out.** XMP and presets enter through `rawmakase-interop`
  (parse, then apply to a recipe, which is validated); exports' XMP and Lightroom
  presets leave through it. A Lightroom catalog is imported by
  `rawmakase-catalog` (`catalog::lightroom`, inside `Db::with_lightroom`); its
  Develop settings are converted by interop's `lr_develop`.
- **Library previews.** Thumbnails come from the files' embedded previews
  (`raw`, `raster`); edited previews are rendered with `develop::render_fitted` on the CPU and
  stored in the catalog's preview cache.

Whether Develop opens a catalog photo has one owner, `app::library::Refusal`:
`develop_refusal` refuses an offline file, and a JPEG, PNG, TIFF, WebP, AVIF, HEIF or JPEG XL whose header
`raster::probe` finds Develop can't edit (`raster::Unsupported`, such as a CMYK
JPEG, an HDR or animated file, or a tiled AVIF), before anything opens it. The probe reads the file, so only work on one
photo uses it (opening it, its dialog, Quick Develop, the context menu,
automation's `open`); export and preview builds over many photos use
`availability_refusal` and meet such a file as that photo's failure; `Refusal::opening` turns the same error, should
opening find it, into the reason the loader reports. Whether the Library shows a photo developed or as its file is
`Library::develops`: a RAW always, any other format once it has an edit, since
its defaults reproduce the file. Preview builds and stored previews follow the
same rule from the photo's edit record.

## Desktop composition

`app/mod.rs` owns the `Editor`, initialization and lifecycle. Its fields remain
private. UI and workflow methods have visibility limited to the app module, one
file per tool, workflow or policy. Each Develop adjustment panel has its own file
in `inspector/panels/`: it takes the recipe, the view state and a read-only view
of what it shows, edits the recipe during the edit frame and returns the actions
it can't carry out itself (Auto, imports, dialogs), which `Editor::controls`
carries out after every panel is drawn. The [code map](code-map.md#desktop-application) lists them.

`Editor` deliberately remains the coordinator for state shared across panels.
A panel should call the domain API responsible for an operation, rather than
implementing file formats, SQL or pixel processing itself. Thumbnail decoding
belongs to `raw`; library browsing does not call into the preview renderer
(`PreviewRenderer` and its state). Its edited previews use `develop::render_fitted`,
Develop's Fit path on the CPU, keeping nothing between photos.

Some features span several owners; each module's docs describe its part:

- `platform::menu` owns the macOS menu objects and queues typed actions, waking the
  UI without reaching editor state; `app::shell::native_menu` owns availability and
  revalidates each action before calling the existing editor operations. Close and
  Quit keep the normal save guard. `platform::titlebar` owns the macOS header's
  drag area and double-click.
- The community camera-profile catalogue: the model's `camera_profiles::catalogue`
  owns the data, its validation, matching and verification and never reaches the
  network; `app::develop::profile_download` owns the request, the task and the offer, and
  `app::develop::panels::inspector::profile_offer` draws it.
- `app::shell::messages` owns what the app says to the user, and the running-work line
  is read each frame from the state that owns the work, so nothing starts or ends it
  by hand.
- `app::preferences::monitor` resolves the Monitor profile into the profile each render job
  captures; `platform::display` owns the OS queries. No choice changes the
  renderer's output colour space or export profiles.
- An export's colour space travels in its `ExportSettings` (export.json), captured
  with the job, never in a recipe, `ExportOptions` or the catalog.
  `ExportSettings::output_space` is the one check of space and format before
  rendering. Only `develop::render_export` makes an `ExportImage` in a space other
  than sRGB, on the CPU; the preview, its GPU stages and caches stay sRGB, and
  `render_base` refuses another space with them.

## Boundaries to preserve

- Dependencies between top-level modules are listed in
  `scripts/deps-allowed.txt`, and `scripts/deps.py check` (CI and `make check`)
  fails on a new one or on a cycle; add a line only when the new dependency
  points down the intended layering. `scripts/deps-closures.txt` lists what some
  modules must never reach, even through others: the edit session never reaches
  the RAW decoders, the renderer or the app, and the renderer never reaches the app or
  export. The check prints the chain that breaks a rule.
- `scripts/deps-crates.txt` lists the workspace crates each crate must never
  depend on, directly or through others: the model never on file formats, the
  catalog, rendering or decoding; the RAW decoders on no edit; the protocol and
  inference on no other crate. `scripts/deps.py check` reads the rules against
  `cargo metadata --no-deps`, so an edge that points up the layering fails
  without a build.
- The model, file formats, catalog and control protocol are crates, so Cargo
  rules out a dependency back on the app. `scripts/crate-closures.sh` (CI and
  `make check`) fails when one of them gains the app, a GPU or GUI crate, or a
  C RAW library or Little CMS binding, and when any workspace crate depends on
  one or a build script links one (`rawmakase-decode` has no build script). The
  only C compiled comes with Rust crates' own sources (the catalog's bundled
  SQLite, ring, Wayland's backend on Linux); HEIC pixels come from the operating
  system's decoders, opened at run time ([dependencies](dependencies.md)).
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
  recipe. Collection discovery and favorites belong in `presets`. Auto settings in
  XMP are interpreted in `xmp` and measured by the engine through the model's `PhotoMeasures`,
  so a preset's Auto is the app's Auto. A preset applied to a photo (Develop's
  click, a batch on the Library's selection) goes through
  `presets::apply_to_photo`, and the rule that turns on a switched-off panel a
  change edits is `model::edit::turn_on_edited_panel`, applied by Develop's edit
  session and by batch edits alike.
- Validate recipe changes at domain boundaries. Saved format versions and
  migrations are shared by native presets, catalog edits and legacy sidecars. Every saved recipe renders
  with the one engine, and fields it lacks take the recipe defaults, so changes to
  recipe defaults must account for older saved edits.
- Never change an original's contents or a Lightroom source. A photo's file is
  moved, renamed or put in the Trash only by a file operation the user
  confirmed (`catalog::file_ops`); nothing copies over, replaces or deletes
  one. A move to another volume copies it, verified, and puts the original in
  the Trash only once the catalog follows the copy. Preserve unsupported source data, legacy sidecars, no-clobber
  publication, ICC/EXIF handling and temporary-file write behavior.
- Rendering math stays in `develop` and `camera_profiles`. Fit previews, regions
  and exports must continue to share the relevant processing paths. What the
  [scene tone stage](scene-tone-stage.md) measures of a photo comes from one
  measurement copy of the full-resolution photo (`Toned::measured`, cached per
  image), never from the region or preview size being rendered; the darkest level
  the default black follows comes from the same copy without red eye corrections
  and spot removal (`Toned::unretouched`, the same copy when there are none).
- AI Denoise keeps the crate direction: `rawmakase-inference` runs the model and
  knows no workspace crate but the cancellation token; `rawmakase-decode` packs the mosaic and assembles the
  result without knowing the model and defines `AiBase`; the app implements
  `AiBase` with inference (`ai_denoise::Service`) and owns the queue, the model
  install (`model_install`) and the cache. **The engine never sees `AiBase`:** it receives a resolved,
  immutable `develop::AiSource` (highlights recovered on its own, equal only to
  itself) inside the captured `Denoise`, from the Develop session
  (`PreviewRenderer::set_ai`) or from export (`render_with_ai`). An export of a
  photo with AI Denoise on gets the complete source or fails that photo with the
  reason; it never renders the plain image in its place, and callers without the
  service pass `AiService::refusing`.
- Keep OS integration in `platform`, and decoding and colour management in
  `rawmakase-decode`. Use the existing asset-path policy instead of duplicating environment
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
- File operations change catalog rows only on positive evidence: rows follow a
  file proven at its new path (the same file object, by its exact name in the
  folder's listing), and go only with a Trash receipt and the file gone from
  its path; a file's absence alone authorises nothing. A file leaves its path
  only by an exclusive rename on the same volume (never a link and unlink) or
  by the system Trash, which fails closed: a volume without one is refused,
  never deleted from. To another volume a file is copied (into a hidden
  partial copy, hashed as read, synced, read back, then renamed exclusively),
  the catalog follows only proven copies, and the original goes to the Trash
  only after that, while it is still the file copied and its extras
  (extended attributes, resource forks, alternate data streams) are still
  its copy's; a drive that can't keep them leaves it where it is; the one
  file ever unlinked is a partial copy the run made and never named. Such copies are
  made only between volumes whose file ids stay (local APFS, HFS+, ext,
  XFS, Btrfs or NTFS ones) or FAT and exFAT ones, where a copy is proven by
  its exact name and SHA-256 instead and no file id is journalled (#573);
  and a name held by another file object, or not proven, is never taken for
  a copy gone. A folder moved to another volume is copied whole into one
  hidden folder there, then named by one exclusive rename; its originals go
  to the Trash whole only when the folder holds exactly what was copied, and
  the only folders ever removed are empty ones that copy made. On FAT and
  exFAT both are proven by names, as photos are (#573). A folder put in the
  Trash goes in one Trash call, only while its whole tree is exactly as the
  user was asked about (`file_ops::tree_trash`), and its subtree's rows go
  only with the receipt, the folder gone from its path and, where it can be
  read, the trashed folder proven the one planned. A root folder moved into
  another root moves as a folder does, and leaves the catalog's roots, with
  its locations on every computer, in the transaction that re-keys its
  folders; it is journalled as its own kind (`move_root`), which earlier
  releases fail closed on. Every disk step is journalled, after its folders
  are synced, before the item's next, and recovery decides whatever a crash left from
  the journal and the disk, putting back only what it proves. Items moved or
  renamed within a drive go up to 100 at a time (#692), each step in one
  commit and each folder synced once for all of them, so every item still
  steps, syncs and journals its files one after another. One process at a
  time changes a catalog's files, under an OS lock on a file beside the
  catalog named after its canonical path; catalogs on network volumes or with
  several hard links are refused. Each item's preconditions (its family's
  placement and user-state digests, its folders' resolved paths) are captured
  once in the plan and checked again in the transaction that journals it, the
  one that completes it, and by recovery.
- A community profile download captures its profile, camera metadata and
  destination once and survives navigation. It applies only to the photo it was
  asked for, while that photo is still open (same load), its file still has the
  `storage::Identity` it was opened with (checked when the download starts and at
  completion), it still has the profile it had and no camera-look fit was asked for
  since, as one History step; otherwise the profile is only installed. What applies
  is the installed file read once and verified as the download was. Cancellation
  and publication claim one atomic state ([`app::develop::profile_download`](../src/app/develop/profile_download.rs)).

## Compatibility

The library API is internal: the RAWmakase binary, its examples and its tests are
its only clients, so modules and functions change freely with them and nothing is
kept for outside callers. `src/lib.rs` exports only the modules they use; the rest
(`catalog_session`, `decode`, `demosaic`, `edit_session`, `platform`, `time`)
are `pub(crate)`, and the desktop app exposes only `app::run`, so the
compiler reports what in them nothing uses. The compatibility surface is the saved data: recipe and
preset envelopes are versioned and migrated by `model::saved_format` (written as
version 11, which releases before the one engine refuse, 12 for a recipe naming a
Lensfun lens, which releases before Lensfun refuse, or 13 for one with AI Denoise
not at its default, which releases before AI Denoise refuse); a catalog opens at
versions 1 to 5 (others are refused, the file left unchanged), is upgraded to 3
before a recipe is stored in it, to 4 before a Lensfun choice is and to 5 before an
AI Denoise setting is, and its schema only ever gains tables, applied idempotently
on open; see [catalogs](catalogs.md#sqlite-format-versions-1-to-5). An AI Denoise
result is never part of the saved data: the recipe names no model, and results are
cached under a key holding the model's SHA-256 and the adapter version.

## Validation

`make check` runs the standard suite (see the [README](../README.md#development)).
Photographic checks and measured results live in the per-stage pages and in dated
[records](records/); see the [documentation index](README.md).

## Live application control

`app::shell::commands` defines transport-independent application operations, target
checks, parameter units and explicit results. `Editor` executes each mutation
inside its generation-bound edit transaction, then answers that request.
`app::shell::automation` owns the input queue and adapter lifecycles. Its MIDI device
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
combined with the per-file white-balance presets the native adapter reads.
Camera profiles apply them after signature matching. XMP carries Temperature
and Tint; it does not implement calibration rules.
See [camera parity](camera-parity.md) for the offline Adobe audit and references.
