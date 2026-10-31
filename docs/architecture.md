# Architecture

RAWmakase is the app crate with explicit domain modules, on top of
`crates/rawmakase-model`: what an edit is, as values (the recipe and its settings,
camera and lens profiles, colour primitives, DNG hints, output buffers, storage helpers), which
Cargo builds without the RAW decoders, the renderer, wgpu or the GUI, and
`crates/rawmakase-interop`: the file formats and presets over it (XMP, Lightroom's
Develop settings, presets, raw defaults, EXIF and JPEG, export settings),
built the same way, and `crates/rawmakase-catalog`: the SQLite catalog, Lightroom
catalog import and which edit a photo develops with. Above the model,
`crates/rawmakase-engine` is the renderer (`develop`, CPU and the GPU preview port),
built without the RAW decoders, the catalog or the GUI; `crates/rawmakase-decode` is the only
crate that reaches RAWmakase's own RAW decoders (`crates/rawmakase-raw`) and does
colour management (`raw`, `demosaic`, `photo`,
and the full-size decode in `decode` and `decode_cache`, which uses the engine's
highlight recovery); `crates/rawmakase-export` develops and writes exports with
their watermarks over both; `crates/rawmakase-inference` is the one leaf that runs
the selection models through a lazily loaded ONNX Runtime (pixels in,
coverage out; it knows no other workspace crate, and no value, catalog or
rendering crate can reach it, so saved masks render without any inference);
`crates/rawmakase-remote` sits directly on inference alone — the optional
worker binary that runs the same models on another computer, and the client the
app uses when configured to, over a bounded TCP protocol that knows no catalog,
renderer or GUI. The app crate re-exports these crates' modules at its
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
| `raw` | RAW files: reading metadata, decoding into `camera_data` images, oriented embedded thumbnails, with RAWmakase's own decoders (`crates/rawmakase-raw`, see [RAW decoding](raw-decoding.md)) | RAW decoding |
| `color_management` | The sRGB profile exports embed and the transform of 8-bit sRGB to a monitor profile, in pure Rust (moxcms reads the profiles) | `color_management.rs` |
| `photo` | Opening a photo: `open_source` is the one entry point, sending a RAW to `raw` and a JPEG, PNG, TIFF, WebP, AVIF, HEIF or JPEG XL to `raster`; a RAW's decoder facts from `raw`, then the camera's lens tables, a DNG's profile and hints, and the imported lens profiles that fit | `photo.rs` |
| `raster` | Opening a JPEG, PNG, TIFF, WebP, AVIF, HEIF or JPEG XL: decoded upright and converted from its embedded ICC profile or CICP colour to 8-bit sRGB for previews, or to linear ProPhoto f32 for Develop (pure Rust: `image`, `jxl-oxide`, `rav1d` without assembly, `rust_h265`, `moxcms`) | `raster.rs`, `raster/` |
| `demosaic` | RAWmakase's own Bayer and X-Trans demosaicing of the unpacked sensor data | `demosaic.rs` |
| `dng`, `tiff` | A DNG's rendering hints (embedded profile, baseline exposure, crop, opcodes) and the bounded TIFF reader behind them (`crates/rawmakase-tiff`, a leaf crate with no RAW knowledge, re-exported as `model::tiff`) | `dng.rs`, `rawmakase-tiff/src/lib.rs` |
| `decode` | A photo's full-size image, made one way for Develop, prefetch, Reference View and export: the cache's copy, else a decode with highlights recovered and stored, as a `DecodePolicy` asks | `decode.rs` |
| `decode_cache` | Disk cache of developed camera images, keyed by file identity, demosaic setting and build; the app shares one live size limit and folder across Develop, prefetch, Reference View, export and preview builds, persisted in its session | `decode_cache.rs` |
| `camera_profiles` | DCP parsing and validation, camera transforms, RAWmakase's own profiles, profile library and camera matching, `.cube` LUTs imported as look profiles, fitted camera looks as profiles (built only; fitting is `camera_look`'s), DNG temperature/tint | `dcp.rs`, `library.rs`, `open.rs`, `cube_look.rs`, `fitted.rs`, `reference.rs` |
| `camera_look` | Fitting Camera Look (fitted) to a RAW's embedded JPEG: preview choice, pairing with the photo's render, solvers and acceptance on held-out tiles, and the default fit of photos without colour data through its fit cache. In `rawmakase-decode`, as it opens RAWs and decodes their previews; the model only builds and checks what it finds, and the catalog, `xmp` and presets never fit (the app and export pass the default in with the photo's profiles) | `camera_look/` |
| `optics` | The lens correction model the renderer evaluates (vignetting, distortion and lateral CA as radial functions), Adobe lens profiles (LCP) as data, and Lensfun calibrations as data with their evaluation for a photo; depends only on `xml` | `mod.rs`, `lcp.rs`, `lensfun.rs` |
| `lens` | Readers that fill the `optics` model: the tables cameras embed in their RAWs, imported Adobe LCPs, Lensfun's database (read, matched, installed) and lateral CA measurement, plus profile selection; an edit's Lensfun lens carries its own copy of the calibration, so rendering never reads the database | `embedded.rs`, `lcp.rs`, `lensfun/`, `auto_ca.rs`, `choice.rs` |
| `rendered` | Developed pixels as values: an output image, its histogram and clipping overlay, which the renderer produces and export and watermarks use; depends on nothing | `rendered.rs` |
| `model` | What an edit is, as values shared by the renderer, the catalog and file formats: the `Recipe` with its validation (`ValidRecipe`), saved versions and migration, panel switches, and the settings it is made of: Effects, Heal and Clone, Red Eye, Point Color, masks, Transform with Upright, white balance from metadata, the image space positions are kept in (`image_frame.rs`), the sliders as parameters, the rules an edit follows and the setting groups Copy Settings, Sync and presets use. Depends on camera profiles, lenses and optics, never on rendering | `recipe.rs`, `valid.rs`, `saved_format.rs`, `panels.rs`, `operators.rs`, `effects.rs`, `masks.rs`, `transform.rs` |
| `develop` | Rendering a recipe: geometry, color processing, the scene tone stage, curves, effects, local adjustments, detail rendering and the GPU port | `pipeline/`, `scene_tone/`, `quality/`, `geometry.rs`, `gpu/` |
| `xmp` | Namespace-aware Adobe settings parsing and application to recipes. Settings that ask for Auto measure the photo through `PhotoMeasures`, which `develop::Measures` provides, and packets take the crop as rendered from the caller, so XMP needs no rendering code | `parse.rs`, `apply.rs` |
| `raw_defaults` | Lightroom's Raw Defaults: the master and per-camera choices and a photo's starting settings. Above `presets`, whose library it reads | `raw_defaults.rs` |
| `presets` | Native JSON recipe presets, installed XMP collections, favorites and preset import, and a preset's application to one photo, which Develop and the Library's batch share | `native.rs`, `library.rs`, `photo.rs` |
| `storage` | RAW identity checks, which files are photos and the rendered formats' facts (`RasterFormat`: extensions, MIME types, names; the codecs are `raster`'s and `export`'s), application paths and atomic JSON writes, and the file system primitives moving a user's files needs (`fsops`: exclusive rename, file ids, exact listings, name comparison, containment, volume facts and free space, and a copy's steps; policy-free, one file per OS). Saved-recipe versions are `model::saved_format`, the desktop session is `app::session` and legacy sidecar import is `catalog::legacy_sidecar` | `identity.rs`, `files.rs`, `formats.rs`, `fsops/` |
| `export_settings` | Export choices as values: the Export dialog's settings, Rename Photos' remembered choices (`RenameSettings`), the user's saved file-name templates (`SavedTemplates`), a photo's saved `ExportOptions`, and the naming templates (`NameTemplate`: Lightroom's presets and Filename Template Editor templates, `TokenTemplate`) exports and renames name files by, from the `NameFields` taken once per export | `export_settings.rs`, `export_settings/tokens.rs`, `export_settings/rename.rs`, `export_settings/saved.rs` |
| `export` | JPEG, 16-bit TIFF and PNG encoding, selected EXIF, sRGB ICC embedding, atomic output publication | `mod.rs`, `metadata.rs` |
| `ids` | Typed catalog row ids (`PhotoId`, `FolderId`, `RootId`, `CollectionId`), stored and serialized as their integers; a leaf, so edit resolution and export use them below the catalog | `ids.rs` |
| `metadata` | Photo metadata as values: descriptive fields (title, caption, copyright, creator, capture, location), keywords and camera settings; depends on nothing else | `metadata.rs` |
| `edit_session` | The edit of the photo open in Develop apart from the window: its bounded History (named steps, one per gesture, saved with the edit) and save state; no egui | `history.rs`, `save_state.rs` |
| `edits` | Which edit a photo develops with (saved, Lightroom or raw defaults), shared by Develop, Sync and Export; the catalog only reads the stored records | `edits.rs` |
| `catalog` | RAWmakase SQLite database, schema, photo/folder/collection models, edits, relinking, the default Creator and Copyright of photos added from folders (below the camera's own, which the app's reader finds, as the catalog can't reach the RAW decoders), Synchronize Folder's and Find on Disk's engine (`folder_sync`: scan, matching, apply, family digest; named apart from `app/sync.rs`, which is Sync Settings) and disposable preview cache | `schema.sql`, `models.rs`, `mod.rs`, `folder_sync.rs`, `preview_cache.rs` |
| `catalog::db` | The catalog's one connection boundary: an opaque `Db` with checked reads, `snapshot` for consistent reads, `write` as the only way to write, and `with_lightroom` for import; statements are `Sql` (portable) or `SqliteSql` (Lightroom), shape-checked at compile time; values go through the catalog's own `ToValue`/`FromRow` (`catalog::value`) | `mod.rs`, `sql.rs` |
| `catalog::lightroom` | Read-only Lightroom snapshot import | `mod.rs`, `history.rs` |
| `catalog::file_ops` | The one owner of changing where a photo or folder is or what it is named, on disk and in the catalog (#370, planned in #556): planning against a snapshot, the refusals, destination conflicts, names, which companion files go with a photo (by `catalog::sidecar`'s candidate rule), the journal, the operation lock, execution and recovery. Names photos from `interop::export_settings::NameTemplate`, the one owner of naming templates. Reaches the file system through its `Disk` seam over `storage::fsops`, the Trash through a `Trash` the app passes in | `plan.rs`, `execute.rs`, `recover.rs`, `journal.rs`, `rename.rs`, `tree.rs` |
| `catalog_session` | The open catalog and the lists read from it, with no window: opening it, reading it again, writes that must keep those lists in step (ratings, descriptive metadata, collections, virtual copies and their names, stacks), the background reads that fill in capture times and photo info for photos added from folders, and the throttled checksum reader, whose results a structural generation guards. Above `photo` and `raw`, as reading a RAW's info opens it; `app::library` decides which photos are online and shows the lists | `mod.rs`, `backfill.rs`, `background.rs`, `checksums.rs`, `stacks.rs` |
| `lr_develop` | Best-effort conversion of Lightroom's serialized Develop settings into a recipe, through XMP; below the catalog, so edit resolution can use it | `lr_develop.rs` |
| `app` | Desktop editor state, UI, dialogs, background task coordination and presentation | Components described below |
| `platform` | OS integration: the Linux GVFS filesystem bridge, drives, the file manager, the browser, the system Trash file operations get (`trash::SystemTrash`, failing closed, and whether a drive has one; no policy), and the display's color management (macOS surface tagging, the Windows display profile) | `network.rs`, `volume.rs`, `reveal.rs`, `web.rs`, `trash.rs`, `display.rs` |
| `diagnostics` | The system report a bug report asks for (Report an Issue…, `rawmakase system-info`): this build, the system probed once per process and what the window knew, as plain snapshots, with no path, user, file, catalog or profile name | `diagnostics.rs` |
| `updates` | Release checks and self-update through fastframe-update; the notice itself is in `app` | `updates.rs` |
| `lensfun_install` | Downloading, checking, publishing and removing Lensfun's lens database on request (never bundled, never fetched unasked); reading it is `lens::lensfun::installed`'s, the panels and the job thread are `app`'s | `lensfun_install.rs` |
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

`platform::menu` owns the macOS menu objects and queues typed actions, waking the
UI without reaching editor state. The AppKit objects stay on the main thread;
`app::native_menu` owns availability and revalidates each action before calling
the existing editor operations. Close and Quit retain the normal save guard.

`platform::titlebar` owns the macOS header's drag widget and double-click
(fastframe's widget is used elsewhere). `configure` registers the native window
for the current viewport. An application-lifetime AppKit event monitor retains
the latest left mouse-down for each registered window; closed windows are pruned.
An accepted egui header press consumes that event once, across repeated passes.
Other windows, controls and modal input cannot spend its event. Native dragging
receives the original mouse-down, never `currentEvent`, which can already be a
trackpad pressure, drag or release event. The second press performs the system's
Zoom or Minimize setting once across the entire header; Fill remains native as
in fastframe. There is no native-strip ownership split or `setMovable` override.
Fullscreen skips the handler; native traffic-light buttons remain available.

`Editor` deliberately remains the coordinator for state shared across panels.
A panel should call the domain API responsible for an operation, rather than
implementing file formats, SQL or pixel processing itself. Thumbnail decoding
belongs to `raw`; library browsing does not call into the preview renderer.

Whether Develop opens a catalog photo has one owner, `app::library::Refusal`:
`develop_refusal` refuses an offline file, and a JPEG, PNG, TIFF, WebP, AVIF, HEIF or JPEG XL whose header
`raster::probe` finds Develop can't edit (`raster::Unsupported`, such as a CMYK
JPEG, an HDR or animated file, a tiled AVIF, or a 4:2:2 or monochrome HEIF), before anything opens it. The probe reads the file, so only work on one
photo uses it (opening it, its dialog, Quick Develop, the context menu,
automation's `open`); export and preview builds over many photos use
`availability_refusal` and meet such a file as that photo's failure; `Refusal::opening` turns the same error, should
opening find it, into the reason the loader reports. Whether the Library shows a photo developed or as its file is
`Library::develops`: a RAW always, any other format once it has an edit, since
its defaults reproduce the file. Preview builds and stored previews follow the
same rule from the photo's edit record.

`app::messages` owns what the app says to the user. `Editor::messages` is the one
way to say it, and the Library's own `Messages` are taken on each frame. Each thing
is said in one place and nothing is drawn over the photo. `note` (progress, small
confirmations) stays in the status line. `done` (a result, briefly), `warn` (a
refusal or partial result, longer; neither goes while the pointer is on it or the
window is away) and `fail` (what the user asked for did not happen, until dismissed)
show in the top bar after the progress, past a hairline: the worst and newest as a lamp, one line, and a cross on hover that dismisses them all (or Shift+Esc), a problem tinted once as it arrives. A click opens every message in full.
They take no keys. Without the bar (the chrome hidden) they show small at the
window's top. A failure a later event ends is posted with a `Topic` (`fail_on`) and
`settle`d by that event, so a retried save neither repeats a dismissed message nor
leaves a stale one.

Work the user started shows in the same slot as a label over a sweeping line, once it
has run for a moment. `Editor::running_work` reads it each frame from the state that
owns the work (a `Task` and the like), so no call starts or ends it: it goes when the
work ends, however it ends. New long work adds one line there. Work with a place of
its own (an export's or scan's progress, a batch's window, a selection's tile, opening
a photo's badge) is not said again.

`app::monitor` owns the Monitor profile choice (System, sRGB or a chosen ICC
profile) and resolves it, with what `platform::display` reports about the
window's display, into the profile each render job captures. `app::display`
installs a paint callback for the macOS surface color declaration;
`platform::display` owns the AppKit/Metal layer access and the Windows display
profile and advanced-color queries. Following the system lets macOS's
compositor match the display, and on Windows converts to the display's assigned
profile unless Windows manages color itself. sRGB and a chosen profile output
device RGB and bypass compositor conversion. No mode changes the renderer's
output color space or export profiles.

Folder-drop workers resolve the imported folder tree keys against actual local
locations (including mapped symlinks). The source keys travel with the reopened
Library result; the UI applies them after restoring layout and filters, so the
result's source cannot be replaced by the old view. Filesystem resolution stays
on the worker.

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
  the RAW decoders, the renderer or the app, and the renderer never reaches the app or
  export. The check prints the chain that breaks a rule.
- The model, file formats, catalog and control protocol are crates, so Cargo
  rules out a dependency back on the app. `scripts/crate-closures.sh` (CI and
  `make check`) fails when one of them gains the app, a GPU or GUI crate, or a
  C RAW library or Little CMS binding, and when any workspace crate depends on
  one or a build script links one (`rawmakase-decode` has no build script). The
  whole workspace is Rust apart from the catalog's bundled SQLite, which is
  intended.
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
  XMP are interpreted in `xmp` and measured by the engine through `PhotoMeasures`,
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

## Compatibility

The library API is internal: the RAWmakase binary, its examples and its tests are
its only clients, so modules and functions change freely with them and nothing is
kept for outside callers. `src/lib.rs` exports only the modules they use; the rest
(`catalog_session`, `decode`, `demosaic`, `edit_session`, `platform`, `time`)
are `pub(crate)`, and the desktop app exposes only `app::run`, so the
compiler reports what in them nothing uses. The compatibility surface is the saved data: recipe and
preset envelopes are versioned and migrated by `model::saved_format` (written as
version 11, which releases before the one engine refuse, or 12 for a recipe naming a
Lensfun lens, which releases before Lensfun refuse); a catalog opens at versions 1
to 4 (others are refused, the file left unchanged), is upgraded to 3 before a recipe
is stored in it and to 4 before a Lensfun choice is, and its schema only ever gains
tables, applied idempotently on open; see [catalogs](catalogs.md#sqlite-format-versions-1-to-4).

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

`develop::gpu` uses portable WGSL compute through wgpu: Metal on macOS, Vulkan
on supported Linux drivers, including NVIDIA and AMD, and DirectX 12 on Windows.
Windows leaves Vulkan out while a DirectX 12 device can be created, because an
Intel Vulkan driver crashed the app at launch where DirectX 12 worked (#623). If
that device fails, it tries Vulkan and then OpenGL before opening the window
(#768); `WGPU_BACKEND` overrides all of this. The UI and a standalone processor
share that choice (`gpu::backends`). On the desktop it shares the
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
all exports use the CPU stage, which is the reference. Each pass is compiled for the
features its parameters use (`gpu::develop::Variant`); the Shadows/Highlights map's
grids stay on the device per map base, and each sample set's camera colour per the
recipe fields it reads (`gpu::develop::Kept`), written once a second render with the
same settings asks for it and read after its pass is submitted.
When the GPU renders a preview, the developed pixels stay on the device and
`gpu/present.wgsl` finishes them into a texture that egui draws directly (registered
as a native texture by the render worker): sharpening, vignettes and grain, the
clipping overlay, the monitor profile (a 52³ lattice sampled from `color_management` once per
profile, interpolated trilinearly) and 8-bit encoding, in the CPU reference's order.
The histogram is counted in the same pass and the minimap's copy is reduced on the
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
is kept there, and regions are sampled there through geometry and lens correction.
Color and luminance noise reduction run on the full camera image before retouching
and before building the preview pyramid. `develop::recipe::Denoise` captures their
settings and execution together; the retouch cache keys that value and the source
image identity. Exports run the same stage without the cache, and the GPU samples
the already denoised image. The Shadows/Highlights map (with Clarity's gain) is built on the
CPU from the photo's measurement copy, and the scene tone stage reads it
in `develop.wgsl`. Texture's image is made on the CPU, and a mask's Clarity or
Texture renders the whole preview on the CPU. A failure in these stages turns off
only this path for the session.

Presented textures are kept per view (the whole photo, or a 100% region) and size,
two of each, so a slider moving alternates between the reduced preview's and the
full render's textures without allocating. Each texture carries a generation that
changes whenever it is written; the worker reuses a finished frame when switching
views only while its texture still holds it.

Buffers are reused by dimensions, limited by the adapter and a 1 GiB aggregate
buffer budget. No compatible adapter, oversized buffers, allocation/validation
errors and readback failures fall back to CPU. After a GPU failure the preview
renderer releases the backend and avoids retrying it for every edit. Restarting
creates a new backend. GPU execution already submitted cannot be interrupted;
obsolete results are discarded before they are published. Full-quality CPU stages check cancellation
within pixel/row/column work. Pending jobs still coalesce in a single-slot mailbox.

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
combined with the per-file white-balance presets the native adapter reads.
Camera profiles apply them after signature matching. XMP carries Temperature
and Tint; it does not implement calibration rules.
See [camera parity](camera-parity.md) for the offline Adobe audit and references.
