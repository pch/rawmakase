# Lightroom (cloud) import: plan

Status: planning. Nothing here is built. It follows the
[Lightroom extension plan](lightroom-extension-plan.md), which builds the
`importer` contribution, the plan file (a SQLite file of a published schema,
whose rows this page calls records) and `files.list`; read that first.

## Goal

Someone moving from Lightroom (the cloud version, not Classic) can bring their
library into RAWmakase, as #935 asked and #936 and #940 built in the core.
Here it is an extension that RAWmakase doesn't write or vouch for: its author
(ljbergmann) maintains it in their own repository, from #936's and #940's
code. This repository builds only the core contracts it needs, each proven by
a test extension in `extensions/example`, as the extensions plan requires.

What #936 and #940 do, and where each part goes:

| #936 / #940 | Here |
| --- | --- |
| Reading `Managed Catalog.mcat` (SQLite of MessagePack), `settings/`, `previews.db` | The extension |
| Whether Lightroom has its library open (`sqlite_in_use`) | The extension |
| Finding originals in the folders given (name, size, SHA-256; `name-2`) | The extension |
| Photos, edits, ratings, picks, albums | The plan file (built), plus an XMP edit (C1) |
| Photos still in Adobe's cloud, placed where Lightroom will download them | Expected originals (C3) |
| Lightroom's preview of a cloud photo, shown until its original arrives | Stand-in previews (C5) |
| Looking for arrived originals; reordering `name-2`; edits downloaded later | The core accepts arrived originals itself (C3); a `source` (built) relinks reordered ones and hands over late edits (C4) |
| Album picker; marking albums Store Album Locally; the "Downloaded by RAWmakase" album; the room check; opening Lightroom | The extension: a command and a window panel (C6). Every write into Lightroom's files is the extension's |
| `lightroom_library_photos` | Not built. Its parts become general core facts (C3, C4, C5) |

**Not carried, as in #936:** keywords and descriptive metadata from the
library, people, stacks, edit history, versions and videos. #936 had no
Lightroom (cloud) History or snapshots to import, so only a photo's edit needs
the XMP form.

Decisions from the Lightroom extension plan hold: our catalog is canonical,
an import creates a new catalog, the core converts Lightroom edits at runtime
and never writes another program's files.

## Storage

Everything new is a new table; no existing table changes (`schema.sql`:
tables are only ever added).

| Table | Holds |
| --- | --- |
| `photo_lightroom_xmp` | A photo's Lightroom edit as Camera Raw XMP (C1) |
| `photo_expected` | A master's expected original: size, SHA-256 when known, a note (C3) |
| `photo_pending_edit` | A photo whose Lightroom edit is still to come (C3, C4) |
| `photo_import_keys` | The importer's own key for a photo, opaque to the core (C4) |

**Format version 8** may hold expected photos, as version 4 may hold Lensfun
lens choices: a release before it would take a file at an expected photo's
path unchecked, and could develop or export another photo's original under
this photo's edit, so it must refuse the catalog rather than ignore the table.
Catalogs are upgraded to version 8 when they open, with the usual verified
backup, as each version is ([catalogs](catalogs.md)), and new catalogs are
created at it. `photo_lightroom_xmp` and `photo_import_keys` alone would need
no version; they come with it.

Each row goes with its photo, through the same deletes that remove a photo's
other rows. `photo_expected` is a fact about the file, so it belongs to the
master: a virtual copy has no file of its own and follows its master, and Set
Copy as Master moves the row, as it moves the master's other file facts.
`photo_pending_edit` and `photo_import_keys` belong to the photo the importer
made and never move: a pending edit arrives for that photo, whichever is
master by then. Export as Catalog copies the tables with their photos.

## Contracts

### C1. Camera Raw XMP as an edit

Lightroom (cloud) stores each photo's edit as Camera Raw XMP, not Classic's
Lua table. The plan's `edit` table takes `format = 'camera-raw-xmp'` (a new
schema and `PLAN_VERSION`).
The core stores it in `photo_lightroom_xmp`, never in `lightroom_develop`, so
which form a text is is never guessed. `edits::resolve` takes a photo's
Lightroom edit from whichever is there, and `lr_develop::convert_develop`
converts either (#936 did this in about 50 lines), so the photo renders and
takes **Apply compatible Lightroom edits** as a Classic photo does, extension
or not. `history` and `snapshot` records stay Lua only; an XMP form is added
when an importer has them.

### C2. What an importer asks before it runs

- **The source may be a folder.** `.lrlibrary` is a folder (a package on
  macOS). `[[importer]]` gains `source = "file"` (the default) or `"folder"`,
  with `extensions` naming what the chooser offers either way.
- **Questions.** An importer may declare inputs the app asks before
  `import.run`, drawn by the app as settings are: for now one kind,
  `folders` (a list the user adds folders to, possibly none), with a label
  and help. #936 asks where originals are kept (a NAS, say). The answers go in
  `import.run` as `inputs`.
- **Onboarding.** `files.list` entries of kind `catalog` gain `importer`, the
  importer that reads them, so the onboarding row for a `.lrlibrary` runs the
  right one.
- **Photos whose originals are here** get, after the catalog is published,
  the per-file reading Add Folder does (sidecar metadata, filling only empty
  fields; capture time and camera facts from the file), in the background, as
  `photo_import_facts`' EXIF reading runs today.

### C3. Expected originals and edits

A `photo` record may say what is still to come:

- its **original**: not on this computer now, at the path given (for
  Lightroom, where it will download it), with its size, its SHA-256 when
  known, and an optional note ("In Adobe's cloud. Download it with
  Lightroom.");
- its **edit** (`edit_pending`): the source has one, but not here yet. #936
  has these for photos whose original is in the cloud and for some whose
  original was found on a NAS.

The core writes a `photo_expected` row for an expected original and a
`photo_pending_edit` row for a pending edit, and removes each once it has
come. Its capture time, camera and size come
from the plan's `photo` and `info` records, so sorting, filtering and the
Info panel work before it arrives.

**While a master's original is expected, it is offline**, whatever file is
at its path. This is one rule in the core's single answer to "is this photo's
original here", which every consumer asks: the Library's availability
(`library/mod.rs`, `develop_source`), thumbnails, Develop, Export, Sync,
checksums, Synchronize Folder and file operations. Synchronize Folder doesn't
offer to remove an expected photo as missing. Where the app says a photo is
offline, it shows the note.

**Accepting an original.** The core takes the file as the photo's original
when both hold:

- a file is at its path and has the expected size (a download in progress is
  shorter);
- when a SHA-256 is expected, the file's matches. The file is read whole for
  this by the same hashing the checksum reader uses, in a check of its own
  for expected photos only (the checksum reader skips offline photos, and no
  other consumer sees the file before it is accepted); the result becomes the
  photo's first checksum.

A photo whose edit is still pending isn't accepted until the edit has come
(C4), so it never opens without it. A photo found here whose only pending
thing is its edit is online; its edit is taken only while it has no
RAWmakase edit (C4).

A file of the right size whose checksum doesn't match stays offline, with
"The file at this place isn't this photo's original" instead of the note. The
core checks expected photos when the Library reads the catalog, when a folder
is synchronized, and every few seconds while any are expected and the
catalog is open, off the UI thread and only with a stat per photo until a
size matches.

C3 and C4's late edits ship together with the first step that can publish
expected photos (below): no release creates one without these rules, so no
importer has to choose between a photo that never comes online and an edit
that is lost.

### C4. Relinking and late edits, through a source

Lightroom numbers same-named photos of a day in the order it downloads them,
so `IMG.CR3` and `IMG-2.CR3` can arrive the other way round, and a photo's
edit can arrive after the catalog was imported. The extension declares a
`[[source]]` (built, D26–D34) for these; it never adds photos.

- **Keys.** A `photo` record may carry `key`, the importer's own id for the
  photo (for Lightroom, its asset id), stored in `photo_import_keys` with the
  importer's id. The core never reads it.
- **`read.expected`** (in a source's scope): the scope's catalog's photos with
  an expected original or a pending edit, with their key, path, what is
  pending, size and SHA-256, and a target token that doesn't need the
  original: it covers the photo, its pending rows and its path, not the file
  or its recipe
  (today's tokens read both, `targets.rs`). It is paged, in photo id order,
  each page within the channel's line limit (an 11,000-photo library is over
  2 MB); the cursor belongs to the scope and ends with it, and a photo
  accepted between pages simply isn't on a later one.
- **`photo.lightroom-edit`**: the source names an `edit_pending` photo and a
  file holding its edit (for Lightroom, the XMP it downloaded into
  `settings/`), or says it has none. The core reads the file (up to the
  16,000,000-byte limit, as XMP sidecars), stores it in `photo_lightroom_xmp`
  and clears the pending edit, or just clears it. For a photo still offline,
  nothing has it open and it has no edit of its own. For one found here, the
  edit is taken only while the photo has no RAWmakase edit and isn't open in
  Develop (answered `busy` then, and the source tries again); otherwise it
  is refused and the pending edit cleared, so the photo keeps the user's own
  edit. Its previews are rebuilt as after any edit save.
- **`photo.locate`**, a batch: each entry moves a photo whose original is
  expected to a path in a catalog folder. The batch is checked whole and
  applied in one transaction: every token current, every photo's original
  still expected, every destination either free or another expected photo's
  in the same batch, no destination twice. An entry whose destination has a
  file must pass C3's size rule there (its checksum is then checked as for
  any arrival); an entry whose destination has none just moves where the
  photo is expected. So a swap of `IMG.CR3` and `IMG-2.CR3` is one batch of
  two, whether both originals arrived or one. Anything else refuses the
  whole batch (`stale` or the reason).

Source proposals are admitted only in the source's current scope, for that
scope's catalog, with tokens from that scope's `read.expected` (D28's
`not_admitted` for any other). These are `api` 4 additions.

Keys, `read.expected` and `photo.lightroom-edit` ship with C3;
`photo.locate` can follow. Until it does, an expected photo whose original
arrives under another photo's name stays offline (its size or checksum
doesn't match), never shown with the other photo's edit, and comes online
once `photo.locate` moves it.

### C5. Stand-in previews

A `preview` record gives a JPEG (a file in the plan folder) to show for a
photo while its original isn't here: in the grid, the Loupe, Compare and
Survey, marked as a stand-in, never in Develop or Export.

Where the core keeps them is to be settled with #1022 (editable previews),
which needs files attached to a photo too. The requirements:

- **Not inside the `.rawmakase` file.** A library's stand-ins are gigabytes
  (#936's library had 11,278 of them, up to 2048 pixels each); a catalog
  doesn't embed them for the reason it no longer embeds a `.lrcat`.
- **Not in the preview cache**, which can be emptied and rebuilt from
  originals; a stand-in can't be.
- **Kept with the catalog**, deleted with their photo or once its original is
  accepted and its own preview built, carried by Export as Catalog, found
  again when the catalog moves, and named in what users back up.
- **Published after the catalog.** Stand-ins are written by a job that runs
  once the catalog is published, from the plan folder, so the catalog's
  one-file publication stays as it is; a photo shows as offline until its
  stand-in is in.

The design to review with #1022: a folder beside the catalog
(`<name>.rawmakase-attachments/`), files named by content, a table naming
each photo's attachments by kind (`stand-in`, `editable-preview`), its writes
and deletes journalled as file operations are.

### C6. The download picker

**Download from Lightroom…** is the extension's command
(`places = ["file.import"]`, drawn once the place is). It opens the
extension's `window` panel: the albums, and "Not in an album" (#940), each
with how many photos and bytes are in the cloud, to tick, and a button. What
the panel needs from the host:

- `window` panels drawn as windows (accepted in manifests today, drawn on the
  extension's page);
- a **checklist** control: rows of a label, a detail text and a tick, set by
  the program, with a summary line the program updates;
- a button whose click goes to the program, and a confirmation dialog it can
  ask for.

The panel says, before anything is written, that Lightroom's files are
changed and a backup made, and for "Not in an album" that the album it makes
syncs to the user's other devices (#940); it refuses a selection that doesn't
fit on the disk. Marking albums, making that album, the backup, the room check
and opening Lightroom are all the extension's work; the core never writes
Lightroom's files, and photos come online through C3.

## Order

Each step ships with its test extension in `extensions/example` and its docs
(`manifest.md`, `protocol.md`, `overview.md`, `catalogs.md`), after the
Lightroom extension plan's step 2.

1. **C1, C2, C3, and C4's keys, `read.expected` and late edits.** The
   extension imports a library: photos here, and cloud photos as expected
   originals that come online, with their edits, as their originals arrive
   under their own names. Mark albums Store Album Locally in Lightroom to
   download them.
2. **C4's `photo.locate`.** Reordered arrivals.
3. **C5**, designed with #1022 first. Cloud photos show Lightroom's preview.
4. **C6.** The picker.

## Tests

- **Storage.** A version 7 catalog upgrades to 8 with its backup; a stored
  version 7 release refuses a version 8 catalog that has an expected photo
  with a wrong file at its path; Export as Catalog carries the rows; Set Copy
  as Master moves the expected original to the new master and leaves a
  pending edit with the imported photo, which still takes it (the new
  master, edited meanwhile, keeps its edit); Remove from Catalog removes
  them.
- **C1.** A photo with an XMP edit renders and takes Apply compatible
  Lightroom edits as #936's fixtures do, with the extension disabled.
- **C2.** A folder source; a `folders` input with none, one and several
  folders; the onboarding row starting the right importer; found originals
  getting their sidecar metadata.
- **C3.** A short file stays offline everywhere (Library, thumbnails,
  Develop, Export, checksums, Synchronize Folder, file operations); the right
  size and checksum accept the photo; a checksum mismatch stays offline with
  its message; another photo's original at its path is never developed with
  this photo's edit; Synchronize Folder offers no expected photo for removal.
- **C4.** `read.expected` paged over a large catalog, with photos accepted
  between pages; a swap in one batch, with both originals arrived and with
  one; a batch with one stale entry refused whole; a
  destination that is an online photo's refused; a late edit stored and the
  photo then accepted; a late edit for a photo found here, taken while it has
  no edit, `busy` while it is open in Develop, refused once it has an edit; a
  proposal from another scope `not_admitted`; the source turned off and the
  extension disabled mid-batch.
- **C5.** A stand-in survives an emptied preview cache, goes with its photo
  and its acceptance, moves with Export as Catalog and with the catalog, and
  is never exported or developed.
- **C6.** Checklist rows set and ticked, the button reaching the program, the
  confirmation, the window closing with the extension disabled.

## Open

- C5's store, decided with #1022.
- Whether C2's inputs grow beyond `folders` (for Capture One: sessions or a
  catalog); add a kind only with an importer that needs it.
- Whether the Lightroom (cloud) extension is listed as Official
  (`rawmakase_extensions::github::OFFICIAL`) or stays Not reviewed. Listing
  it says RAWmakase vouches for it, which #936's review declined.
