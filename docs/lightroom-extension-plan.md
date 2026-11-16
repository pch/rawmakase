# Lightroom extension: plan

Status: steps 1 and 2 built; 3 and 4 planned.

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
program writes the plan. The program answers `{"photos":n}`, the photo rows
the plan holds, once the plan is complete and closed, or an error.

- **Deadlines.** `import.run` has no fixed deadline. It has an inactivity
  deadline instead: the program sends the protocol's `progress` for the
  request (`{"t":"progress","id":n,"done":…,"text":"Reading photos… 12,000 of
  48,000"}`) at least every 10 seconds, and 60 seconds without one cancels the
  request, which then fails after the usual cancel grace. The session gains
  this as a per-request deadline kind (`Session::request_with_inactivity`):
  `progress` resets it for the request its `id` names, and for no other, and
  progress that arrives after a deadline the poll is about to enforce neither
  revives the request nor is handed over. `hello` keeps its 5 seconds.
- **Cancel** is the protocol's `cancel`; the program stops writing and answers
  `cancelled`. Whatever it wrote is deleted with the folder.
- The program never touches the destination catalog; the app never reads the
  plan before the answer.

### The plan

A SQLite file in a fixed, published staging schema: the protocol crate ships
its DDL as one constant, `plan::SCHEMA`, with `PLAN_VERSION`, and states its
rules (`crates/rawmakase-extension-protocol/src/plan.rs`,
[the protocol's page](extensions/protocol.md#the-plan)). The program writes it
with its own SQLite: it runs `SCHEMA` as it is on the new file, writes the one
`plan` row and the tables' rows, and closes the file. The SDK's `importer`
feature hands the handler the source and the plan's path, and has no SQLite of
its own.

The tables are ordinary rowid tables with text ids the importer chooses:
`plan(version, source_photos)`, one row; `root(id, path)`;
`folder(id, root, path)`; `photo(id, folder, file_name, original_path,
captured, rating, flag, label, format, orientation, master, copy_name,
original_name)`; `info(photo, camera, lens, focal, aperture, exposure, iso,
width, height)`; `metadata(photo, xmp)`; `edit(photo, format, text)` (format
`lightroom-develop` only); `history(photo, position, name, created, text)`;
`snapshot(photo, name, text)`; `collection(id, name, parent, kind)` (kind
`set`, `collection`, `smart`, `quick` or `system`);
`collection_photo(collection, photo, position)`; `stack(id, collapsed)`;
`stack_photo(stack, photo, position)`; `keyword(id, name, parent,
include_on_export, export_containing)`; `photo_keyword(photo, keyword)`.
`source_photos` is how many photos the source holds, counted on their own
(for Lightroom, `COUNT(*) FROM Adobe_images`), not from the rows written. The
DDL has `STRICT` tables, primary keys, `UNIQUE`, `NOT NULL` and `CHECK`s, so an
importer's mistakes fail as it writes; the core relies on none of them.

### The core's side

The core imports a plan the way `catalog::lightroom::import_lightroom` imports
a Lightroom catalog, in the catalog crate (`catalog::plan_import`):

1. **Prepare.** The app saves what it must before a catalog changes
   (`ready_for_catalog()`, as today). `PlanImport::prepare` makes the source
   and destination absolute, once, refuses a destination that exists, and
   creates a folder beside the destination (`rawmakase-import-….partial`).
2. **Program.** The app (or the CLI, below) sends `import.run` with the plan's
   path in that folder and waits for the answer, showing the program's
   progress. The extension's host owns the session; the import owns the
   folder. Once the program has answered, the plan is complete, and the
   program stopping (idle stop, a crash) changes nothing for the import.
   Disabling the extension cancels the import until it publishes, in this
   phase and the next.
3. **Import**, on a worker: `Catalog::create` makes a working catalog in the
   folder; the plan is attached to its connection, hardened first; the plan
   is checked and copied in one transaction, on one snapshot of it; then the
   integrity check.
   - **Hardening** (input hardening for a file another program wrote, not a
     sandbox): `trusted_schema=OFF`, SQLite's defensive mode, no extension
     loading (SQLite's default, left as it is), limits on a value's length
     (64 MiB), a statement's, columns, expression depth and attached
     databases, and the plan attached by a `file:` URI with `mode=ro`. Then
     the plan's `sqlite_master` must equal what `SCHEMA` makes in an empty
     database, entry for entry: no other table, index, view or trigger, no
     column more or less.
   - **Checks**, each one query for the first row that breaks a rule, which
     the message names: storage classes and develop text and XMP under
     16,000,000 bytes by byte length (`length(CAST(x AS BLOB))`); the one plan
     row and its version; ids unique per table; every reference resolving;
     at most one info, metadata and edit row per photo, a History position,
     a collection membership and a keyword link once, a stack position
     once, a photo in one stack; ratings, flags, collection kinds and edit
     formats; the Quick Collection named "quick collection" at the top; sets
     without photos; a stack's photos in one folder; a virtual copy's master
     a master in its folder. The photo rows must equal both `source_photos`
     and the answer's `photos`, so a source photo the program couldn't place
     fails as today's orphaned-image check does ("Catalog has orphaned image
     records (n/m); import rolled back").
   - **Copy**: `INSERT … SELECT` from the plan into the catalog's tables, as
     `copy_tables` does from a `.lrcat`. The catalog is new and empty, so a
     row's catalog id is its row id in the plan, and a reference is resolved
     by joining its text id to that row. XMP is read a row at a time with
     `descriptive::read` and applied with `Merge::FillEmpty`, its keywords,
     rating, label and flag ignored, as `copy_lightroom_metadata` does;
     keyword export options as `copy_keyword_export`; stacks numbered from 1
     in their positions' order, and stacks without photos left out, as
     today. Collection kinds are stored as the Adobe `creationId` strings
     today's catalogs hold (`quick` as the plain collection
     `quick_collection()` finds; `system` as `com.adobe.ag.library.unsaved`).
     The `sources` row has the source's path and size and an empty
     `original_catalog`.
4. **Publish.** The `Gate` is one state, *running*, *cancelled* or
   *publishing*, moved once by compare-and-swap: the import moves it to
   *publishing*, a canceller (Stop Import, disabling the extension, quitting)
   to *cancelled*, and whichever comes first wins. The working catalog is
   closed (its statements finalized, the plan detached) and synced through a
   writable handle (Windows syncs only those) before the import moves the
   gate. Publishing is a hard link, which never replaces a file; on a volume
   without hard links (exFAT), a copy into a destination created only if
   none exists, in 1 MiB blocks that check whether the app is quitting, which
   deletes the file it created when it fails or quits. The folder is deleted
   on every outcome.
5. **Open.** The app opens the new catalog through the ordinary catalog-open
   path, which saves the open edit first; when that save fails, the catalog
   stays published but isn't opened, and the app says where it is.

Cancelling before the gate is seen everywhere: a progress handler on the
importing connection reads the gate every thousand SQLite steps of any
statement (the schema check, the checks, the inserts and the integrity check,
whose steps call it too), and the import reads it between statements and XMP
rows. Busy state ends with every outcome, including a panic and a worker
that couldn't start. Quitting cancels the import in any phase before it
published and waits for its worker at most the quit grace.

There is no import registry and no cleanup at start: a crash or a forced
quit during an import leaves its folder beside the destination, as a crash
during today's import leaves its temporary folder, and one during a copy to
an exFAT volume can leave a partial destination, which fails to open as a
damaged catalog does.

### The CLI

`rawmakase import-catalog --importer EXTENSION/IMPORTER source output` runs
the same import (`catalog_import`): it finds the extension as the app does (a
developer folder over an installed version over a bundled one), starts its
program in a session it polls itself (the host crate's `Session`, without the
app's frame loop), sends `hello` and `import.run`, prints the program's
progress to stderr, stops the program once it answered, and runs
`PlanImport`. Secret settings are empty, as only the app reads the credential
store. With the extension disabled it fails with "‹name› is disabled in
Preferences > Extensions". Without `--importer`, `import-catalog` keeps
today's core Lightroom import until step 4.

*Tests, as built:* every Lightroom import test variation, written into a plan
by test SQL from its `.lrcat`, imports to `import_lightroom`'s rows, ids
compared through the plan's row ids, beside a plan written by hand; each check
fails with its message, on a schema without its constraints; another schema
is refused; cancels before, during and after the gate and racing it;
publishing by copy through a test seam; a 100,000-photo plan. In the app and
the CLI, with the example importer (`extensions/example/src/importer.rs`):
the program's crash, failure and silence, steady progress past the idle
stop, the program stopped after it answered, disabling the extension and
quitting in each phase, a panic, a failed save before opening, relative
paths, the disabled extension's message.

### What the extension does

- Reads the `.lrcat` read-only from a private copy, refusing a catalog with a
  live journal or one that changes while copied, and checks it with
  `PRAGMA quick_check`, as today.
- Writes the plan with its own SQLite.
- Counts the source's photos (`Adobe_images`) on their own for
  `source_photos`, independently of the photo rows it writes.
- Numbers a stack's photos itself (Lightroom's position, then its row id, as
  today's import orders them), and drops what today's import drops: a second
  develop settings row of a photo, memberships, keyword links and stack
  photos naming a photo or collection that isn't imported, and duplicate
  links, since the plan refuses them where today's import ignores them.
- Decodes compressed History and snapshot text and maps `creationId` to a
  collection kind, so the plan holds plain text and kinds (an unsaved
  creation's own `creationId`, such as `…print.unsaved`, isn't kept: it reads
  back as today's kind).

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
  copy deletes the partial destination; a destination another writer took
  meanwhile is left untouched.
- **Plan checks.** The existing orphaned-image fixture fails the import, as
  each of step 2's checks does.
- **Large catalogs.** The extension writes a plan of 100,000 photos with
  History, which imports.

## Documents to update when steps 3 and 4 are built

- `docs/extensions-plan.md`: the Official extensions paragraph on Lightroom
  import and the Designed-not-scheduled row.
- `docs/extensions/protocol.md` (`files.list`), `docs/catalogs.md`,
  `docs/architecture.md`, `docs/code-map.md` (the extension, the core's
  import code removed).

## Open

- Whether `files.list` belongs to the `importer` API version or one of its own.
