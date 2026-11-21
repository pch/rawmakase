# Lightroom extension: plan

Status: step 1 built; steps 2 to 4 planned.

## Goal

- Importing a Lightroom Classic catalog is the work of a bundled extension,
  `rawmakase.lightroom`, built on a new `importer` contribution.
- Every Lightroom and Camera Raw path on disk (catalogs, camera profiles, looks,
  lens profiles, presets) is known only to that extension.
- Our catalog is canonical. A `.lrcat` is a data source read once, during an
  import. Nothing in a `.rawmakase` file depends on the `.lrcat` it came from,
  and no release reads that `.lrcat` again to add to a catalog.

## Decisions

| | Decision |
| --- | --- |
| L1 | An import always creates a new catalog, as today. Adding to an open catalog, and importing the same source twice, are not offered. |
| L2 | The `.rawmakase` file no longer embeds the `.lrcat` (`sources.original_catalog`). New imports don't write it; existing catalogs lose it once. |
| L3 | The five backfills that read the embedded `.lrcat` (History, snapshots, photo info, descriptive metadata, keyword export) are removed. A catalog imported by an old release keeps what it has. |
| L4 | Lightroom's develop settings, History steps and snapshots are stored in our catalog as Lightroom's develop text, as today, and the core converts them when they are used, as today (`lr_develop`, `edits::resolve`, the History and Snapshots panels, Sync). Turning the extension off changes nothing for imported photos. |
| L5 | The extension knows where Lightroom and Camera Raw keep things and what Adobe's folder layout means. The core decides what fits a photo or a catalog, shows it and imports it, with its existing importers and their existing rules. |
| L6 | The extension is bundled and on by default. With it off, the app has no Lightroom import and no offers from Adobe's folders; everything already imported keeps working. |
| L7 | The importer's program writes its plan to a file; the core reads, checks and commits it. Pixels and large records never go over the JSON-lines channel. |

## What the core keeps

- Converting Lightroom develop text (`lr_develop`, including `develop_fields`),
  **Apply compatible Lightroom edits**, Lightroom History ("From Lightroom") and
  Lightroom snapshots.
- Decoding compressed Lightroom History text (`decode_history_text`): catalogs
  imported by earlier releases store some History and snapshot text compressed,
  and `snapshots.rs` decodes it when it is read. New imports store plain text.
- `CollectionKind`, and the Adobe `creationId` strings the `collections.kind`
  column holds today (`Catalog::collections`, `quick_collection`). The plan's
  collection kinds are written as those strings, so the stored form doesn't
  change.
- Reading descriptive metadata from XMP (`descriptive::read`), which sidecars
  use too.
- The profile, look, LUT, lens-profile and preset importers, with their rules:
  folder traversal, relative grouping, the skipped `Defaults`, `GPU` and
  `cr_defaults` folders, the `cr_settings` rule (`bulk_import.rs`), and the
  difference between chosen and discovered files.
- The `sources` table: the path a catalog was imported from, its size, and an
  empty `original_catalog` (the schema's `NOT NULL` columns stay).

## What leaves the core

- `catalog::lightroom`'s import: the private copy of the source, its checks,
  and `copy_tables` with its `ATTACH` and `INSERT … SELECT` statements; the
  copies of info, metadata, keyword export, History and snapshots that read
  `lr.*` tables (`info.rs`, `sidecar.rs`, `snapshots.rs`,
  `lightroom/history.rs`).
- `backfill_once`, the five backfills and their `meta` keys, and their calls in
  `catalog_session/mod.rs` and `export_catalog.rs`.
- The Adobe paths and layout: `shared_camera_raw`, `user_camera_raw`,
  `lightroom_folder`, `lightroom_catalogs` and the folder scan in
  `app/catalog/onboarding.rs`; `camera_profiles::adobe_installed`
  (`crates/rawmakase-model/src/camera_profiles/library.rs`); the Adobe path in
  the Lens Corrections import tooltip, which becomes generic text.

## Step 1: stop embedding the `.lrcat` (core only)

This step needs no extension and lands first.

- New imports write their `sources` row with the path and size and an empty
  `original_catalog`.
- The backfills, `backfill_once` and their calls go.
- **Removing existing archives.** When a catalog opens and any `sources` row
  has a non-empty `original_catalog`, one transaction sets it to empty and
  records `lightroom_archive_removed` in `meta`. There is no format version
  change: the schema is unchanged, and an older release opening the catalog
  finds an empty archive, which its backfills already treat as nothing to read
  (`length(original_catalog) > 0`). No backup is written first; the archive is
  a copy of the user's `.lrcat`, and a backup as large as the catalog would
  defeat the purpose.
- **Compacting.** Emptying the blob leaves its pages free inside the file, so
  the catalog is compacted (`VACUUM`) once, after that transaction committed,
  as a visible step of opening ("Making the catalog smaller…"). `VACUUM` needs
  free space up to the catalog's size and leaves the file as it was when it
  fails. A failure is not an error for opening: the catalog opens, the status
  line says the catalog couldn't be made smaller and why, and `meta` keeps
  `compact_pending` so a later open tries again.
- Export as Catalog stops running backfills; it copies what the catalog holds.
- *As built:* the removal and compaction are
  `Catalog::remove_lightroom_archive`, called where a catalog is opened for
  use (the app's catalog session and the CLI's `open_catalog`), not by every
  connection, so a background connection never compacts. Opening reports the
  step through `OpeningProgress::Compacting` (formerly `UpgradeProgress`).
- `docs/catalogs.md` drops the archive from "Import and preservation" and says
  what an import does not keep.

**What users lose.** Data the importer never interpreted was kept only in the
archive: faces, smart collection rules, publish services, GPS and IPTC fields
the metadata copy doesn't read, and anything Lightroom adds later. After this
step it is not in the `.rawmakase` file; it stays in the user's `.lrcat`. This
follows from L2 and is the intended behaviour, stated in the release notes.

Tests: a catalog with an archive opens without it, keeps every photo, edit,
History step and snapshot (including compressed ones), and is smaller; a
failed compaction leaves the catalog opening and retries on the next open; an
older release's reading of the emptied catalog is covered by a stored fixture
read through the old backfill query; Export as Catalog of an emptied catalog
equals one of the same catalog without backfills.

## Step 2: the `importer` contribution

### Manifest

```toml
[[importer]]
id = "catalog"
name = "Lightroom Classic catalog"
extensions = ["lrcat"]   # what the file chooser offers
```

A new `api` version knows the table. An importer needs a program; it may be
`on-demand`.

### Protocol

One request, `import.run` (app to program):
`{"importer":id,"source":"…","plan":"…"}`. `source` is the file the user
chose; `plan` is a path in a folder the app created for this import, where the
program writes the plan file. The program answers `{"records":n}` once the plan
file is complete and closed, or an error.

- **Deadlines.** `import.run` has no fixed deadline. It has an inactivity
  deadline instead: the program sends the protocol's `progress` for the
  request (`{"t":"progress","id":n,"done":…,"text":"Reading photos… 12,000 of
  48,000"}`) at least every 10 seconds, and 60 seconds without one cancels the
  request, which then fails after the usual cancel grace. The session gains
  this as a per-request deadline kind: `progress` resets it for the request its
  `id` names, and for no other. `hello` keeps its 5 seconds.
- **Cancel** is the protocol's `cancel`; the program stops writing and answers
  `cancelled`. Whatever it wrote is deleted with the folder.
- The program never touches the destination catalog; the app never reads the
  plan file before the answer.

### The plan file

JSON lines, written by the SDK's `PlanWriter` (feature `importer`) and read by
the core, which owns the format and its version. The first line names the
version, the record count the program states, and `source_photos`: how many
photos the source holds, counted on their own (for Lightroom,
`COUNT(*) FROM Adobe_images`), not from the records written. A record per line
follows,
each `{"type":…, …}`. The file has no line-length limit of the channel's kind;
each field has the limit the core already applies to it: develop text and XMP
packets up to 16,000,000 bytes (`lr_develop.rs`, `descriptive.rs`), and a line
at most 64 MiB. Records:

- **`root`**: id, the absolute path it had in the source.
- **`folder`**: id, root, path relative to the root.
- **`photo`**: id, folder, file name, original path, capture time, rating,
  flag, color label, format, orientation; for a virtual copy, its master and
  copy name; the file's name at its original import, when known.
- **`info`**: photo, camera, lens, focal length, aperture, exposure time, ISO,
  dimensions (already turned for orientation), each optional.
- **`metadata`**: photo, an XMP packet, read with `descriptive::read` and
  applied only to empty fields, as today; its keywords, rating, label and flag
  are ignored, since the plan has its own.
- **`edit`**: photo, `format` (`lightroom-develop`, the only one now), text.
- **`history`**: photo, position, name, creation time, text.
- **`snapshot`**: photo, name, text.
- **`collection`**: id, name, parent, kind (`set`, `collection`, `smart`,
  `quick`, `system`); **`collection-photo`**: collection, photo, position.
- **`stack`**: id, collapsed; **`stack-photo`**: stack, photo, position (top
  first).
- **`keyword`**: id, name, parent, include on export, export containing
  keywords. Lightroom's nameless root keyword is a keyword like any other, with
  an empty name and no parent; **`photo-keyword`**: photo, keyword.

Ids are strings chosen by the importer, unique per record type. Records may
come in any order; a reference to a record not yet read is resolved when the
whole file has been read.

### The core's side

An import is one job in the app (and in the CLI, below), owned by a new
non-UI module, `catalog_import`, that the app and the CLI share. Its phases:

1. **Prepare.** The app saves what it must before a catalog changes
   (`ready_for_catalog()`, as today) and starts the job. The job creates a
   folder beside the destination holding the plan file and the working
   catalog, and records it in the import registry (below).
2. **Program.** The job sends `import.run` and waits for the answer, showing
   the program's progress. The extension's host owns the session; the job owns
   the folder. This is the only phase that needs the program: once it has
   answered, the plan file is complete, and the program stopping (idle stop,
   a crash) changes nothing for the import. Disabling the extension still
   cancels the import until the publish gate.
3. **Read.** The job reads the plan file record by record into the working
   catalog: catalog ids are assigned there, and each plan id's catalog id is
   kept in a temporary table of the working catalog, so memory doesn't grow
   with the catalog. References are resolved in a final pass in SQL. Collection
   kinds are written as the `creationId` strings stored today (`quick` as
   Lightroom's Quick Collection is stored). Ids are the core's own; the
   Lightroom ids are not kept.
4. **Check.** Every reference resolves; ids are unique per type; the record
   count matches the stated one; the photos written equal both the photo
   records read and `source_photos`, so a source photo the program couldn't
   place (today's orphaned-image check: an image whose file row is missing)
   fails the import instead of being left out; a collection set holds no photos; a
   stack's photos share a folder; field limits hold. Then the integrity check.
5. **Publish.** The gate is one state the job and every canceller change
   under one lock, as the profile download's is: the job moves it from
   *running* to *publishing*, or a canceller moves it to *cancelled*, and
   whichever comes first wins. Before the gate, every failure or cancel (by
   the user, by disabling the extension, by quitting) deletes the folder and
   publishes nothing. Publishing is `rename_noclobber`: a hard link, which is
   atomic; on a volume without hard links (exFAT), a copy into a destination
   file it creates only if none exists. Only once that exclusive create has
   succeeded does the registry record the destination as *claimed* by this
   import (synced), and only then does the copy start. A copy that fails, or
   is cancelled by quitting, deletes the destination file the job claimed.
   The import has succeeded once the destination is complete and synced and
   its registry entry is retired (removed and synced); success is reported
   only after that, and stands whatever arrives later.
6. **Open.** The app opens the new catalog through the ordinary catalog-open
   path, which saves the open edit first; when that save fails, the catalog
   stays published but isn't opened, and the app says where it is.

Busy state ends with every outcome of the job, including a panic. Quitting
cancels the job in any phase before success; its worker stops at the next
record or copied block and deletes what it made, and quitting waits for it at
most the quit grace. Phases 3 to 5 run on a worker, never the UI thread.

**The import registry** is a file in the app's local data folder listing each
import under way: its folder, its destination once publishing by copy has
begun, and a lock the running job holds. On start (app or CLI), an entry whose
lock no process holds is cleaned up: its folder is deleted, and its
destination only when the entry says this import *claimed* it, since only then
is that file this import's unfinished copy. An entry that never claimed its
destination deletes nothing there, whatever file has that name. A crash
between the exclusive create and recording the claim leaves an incomplete
file that no entry names; it is left alone, and opening it fails as a damaged
catalog does. An entry on a volume that isn't there is kept and tried on a
later start. An entry whose lock is held belongs to another running import
and is left alone.

### The CLI

`rawmakase import-catalog input.lrcat Photos.rawmakase` keeps its explicit
destination. It runs the same `catalog_import` job: it starts the bundled
extension's program in a session it polls itself (the host crate's
`Session`, without the app's frame loop), sends `hello` and `import.run`,
prints the program's status, and reads, checks and publishes the plan as the
app does. With the extension disabled it fails with "The Lightroom extension
is disabled in Preferences > Extensions".

### What the extension does

- Reads the `.lrcat` read-only from a private copy, refusing a catalog with a
  live journal or one that changes while copied, and checks it with
  `PRAGMA quick_check`, as today.
- Counts the source's photos (`Adobe_images`) on their own for
  `source_photos`, and fails before answering when it wrote fewer photo
  records, as today's importer does.
- Decodes compressed History and snapshot text and maps `creationId` to a
  collection kind, so the plan holds plain text and kinds.
- Uses its own SQLite.

## Step 3: Adobe's folders in the extension

### `files.list` (app to program)

The app asks the program what is installed. The answer comes in pages
(`{"cursor":…}` in, `{"entries":[…],"next":…}` out), each within the channel's
line limit. An entry is:

| Field | Meaning |
| --- | --- |
| `kind` | `camera-profile`, `look`, `lens-profile`, `presets` or `catalog` |
| `path` | A file, or for `presets` a folder to import as a folder |
| `origin` | `shared` (Camera Raw's folder for all users) or `user` |
| `camera` | For a camera profile: the camera it is for, as Adobe's layout says (the Adobe Standard file name, or the Camera Matching folder name) |
| `group` | For a lens profile: the maker folder it is in, when it is in one |
| `used` | For a catalog: when it was last changed |

The program lists folders and reads no file. It knows Adobe's layout; the core
never parses an Adobe path. A listing belongs to the program's session: a
listing from a session that ended is dropped.

### Matching and offering, in the core

- **Profile menu.** The core lists once per session and caches the answer, as
  it caches its scan per camera today, and lists again after it imports
  files and when the extension is turned on. For the open photo it keeps the
  `camera-profile` entries whose `camera` fits (`camera_fits`, phone modules)
  and that aren't imported yet, and shows **Import Adobe profiles for this
  camera** when some are left, as `adobe_installed` does now. Rendered files
  get no offer, as today. The item is shown only while the extension is
  enabled; on a photo opened before the listing arrives, it appears when the
  listing does.
- **Onboarding.** Opening the first-launch view lists again, as it scans
  again today. It keeps today's rules: shared camera profiles for the
  catalog's cameras, then the looks; shared lens profiles in the cameras'
  makers' and the usual third-party makers' groups; every user lens profile;
  user camera profiles for the catalog's cameras (all of them without a
  catalog); preset folders. The `catalog` entries, newest first, are the
  catalogs it offers to import.
- **Importing.** Choosing an offer runs the core's existing importers with the
  paths the entries give, exactly as onboarding passes them today: files for
  profiles, looks and lens profiles; folders for presets, so traversal,
  grouping, skipped folders and discovered-file rules apply. Profiles are
  imported before looks, which need their base profile in the same import.
  The program never writes into the app's libraries.

Tests: for the existing fixtures, the Profile menu offers the same files as
`adobe_installed`, and onboarding the same files and folders as
`find_on_disk`; an onboarding import through the extension gives the same
library as today's (preset folder layout included); the item appears and
disappears with the extension; a listing from an ended session is dropped.

## Step 4: move the Lightroom import into the extension

- `extensions/lightroom` with the SDK, bundled as MIDI and MCP are (D1), on by
  default.
- **Catalog › Import › Lightroom Classic catalog…** (one item per enabled
  importer), the onboarding rows and `rawmakase import-catalog` go through the
  importer. With the extension off, the item and rows are gone.
- The core's import code (above) is removed.

Tests:

- **Parity.** Every existing import test and the private ones run through the
  extension and give the same catalog as today's importer, apart from the
  archive and with ids compared through the mapping.
- **Extension off.** An imported photo renders the same; a Lightroom History
  step and a snapshot apply; Copy to Before works; Apply compatible Lightroom
  edits works; the same for a catalog imported by an earlier release, with
  compressed History.
- **Lifecycle.** Cancel in each phase, cancel racing the publish gate,
  disabling and enabling the extension during an import, a program crash
  before it answers, the program idle-stopped after it answered (the import
  goes on), a program that stops sending progress, an extraction longer than
  the idle stop with steady progress, quitting during an import, a late
  answer from an ended session, and a failed save before opening the result.
  Before the gate, nothing is published and the folder is gone; after it, the
  catalog is there.
- **Publishing by copy.** On a volume without hard links: quitting during the
  copy deletes the partial destination; a crash during it leaves a registry
  entry that the next start cleans up, destination included; a crash before
  the claim, or a destination another import already took, leaves that file
  untouched; a completed import leaves no entry; another running import's
  entry is left alone.
- **Plan checks.** The existing orphaned-image fixture fails the import. A
  dangling reference, a duplicate id, a short count, a
  field over its limit and a line over 64 MiB each fail the import; a 2 MB
  develop text imports.
- **Large catalogs.** A synthetic plan of 100,000 photos with History imports
  with memory independent of its size.

## Documents to update when built

- `docs/extensions-plan.md`: the Official extensions paragraph on Lightroom
  import, the Importer plans paragraph (edits as Lightroom develop text,
  no archive, the plan file instead of one transaction), the Designed-not-
  scheduled row and the Still open bullet on importers.
- `docs/extensions/manifest.md`, `protocol.md`, `overview.md`, `publishing.md`
  (the SDK's `PlanWriter`).
- `docs/catalogs.md`, `docs/architecture.md`, `docs/code-map.md` (the
  `catalog_import` module, the extension), `docs/shutdown.md` (an import
  during quit), `scripts/deps-crates.txt` if the SDK's feature adds crates.

## Open

- Whether `files.list` belongs to the `importer` API version or one of its own.
