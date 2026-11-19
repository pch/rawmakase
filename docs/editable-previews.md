# Editable previews: plan

Status: Built (2026-11-18) in four parts, one after another on this branch, not
yet merged. Scope agreed in #1022. This page answers the questions asked there
and the review of the first plan (#1143); the notes under each answer say
where the build differs from the plan.

## Goal

A photo whose RAW is offline can be edited in Develop from a smaller copy that
RAWmakase built while the RAW was online: an *editable preview*, 2560 pixels on
the long edge. The edit is the photo's own edit, saved in the catalog for the
original, and it renders from the original again once the original is back.

The change has four parts:

1. **Build Editable Previews** and **Discard Editable Previews** on the
   Library selection. One fixed quality, lossless. Builds run in the background
   at a fixed pace and can be cancelled from the status line.
2. **Develop opens an offline photo from its copy.** The view is labelled as a
   preview. AI masks, AI Denoise and Fit Camera Look are turned off, and so
   is AI Remove, added to `main` since. Export of an offline photo stays
   refused.
3. **One rule for saving offline**, in the save path every edit goes through
   (below).
4. **The copies stay with their photos.** They're removed with the photo,
   carried by Export as Catalog, marked out of date when the original comes back
   changed, and shown in the Library.

Left out: a Lossless/Smaller choice, automation, Sync, Paste Settings and
presets on offline photos from the Library, and full-pace build tuning. Copies
are built for RAWs only. Lightroom's Smart Previews are not imported, but the
storage leaves room for an importer to add them later.

## Size

Measured on the finished branch against `main` (e963437f), counting Rust, SQL,
TOML and shell sources and leaving out Markdown, `Cargo.lock`, test files
(`tests.rs`, `*_tests.rs`) and inline `#[cfg(test)]` items, which are removed
before the files are compared. The first two reviews' changes (deleting a removed
photo's copy at once, marking copies on relink, Synchronize and Find on Disk
with a relink's files read before its transaction, and a plain-file check in
Export as Catalog) are added in, counted the same way:

| | Added | Deleted |
| --- | --- | --- |
| New files (9) | 2,886 lines (2,183 of code, the rest comments and blank lines) | 0 |
| Changes to existing files (53) | 1,213 lines (921 of code) | 196 lines (169 of code) |
| All | 4,099 lines (3,104 of code) | 196 lines (169 of code) |

By part (below), measured the same way between its commits:

| Part | Added | Deleted | Of the added, in new files |
| --- | --- | --- | --- |
| 1. The catalog and the clears check | 723 (514 of code) | 59 | 448 |
| 2. Writing and reading the copy | 1,622 (1,248 of code) | 7 | 1,598 |
| 3. Opening offline in Develop | 791 (582 of code) | 58 | 62 |
| 4. Build and Discard, with the progress-bar refactor | 971 (761 of code) | 80 | 734 |

About 70 percent is in new files. The largest are the build worker
(`app/library/editable_previews.rs`, 734), the record's format and reader
(`rawmakase-decode`, `editable_preview/record.rs`, 481), the DNG writer
(`rawmakase-export`, `editable_preview/dng.rs`, 348), the catalog table
(`catalog/editable_previews.rs`, 448), the record's writer (274) and the
lossless JPEG encoder (`editable_preview/ljpeg.rs`, 257). The record's
two halves are long because every fact is named field by field, both ways, so
a fact added to `Metadata` doesn't compile until the record says what happens
to it.

Of the changes to existing files, the largest are the Library's choice of what
Develop opens (`app/library/mod.rs`, 155), the thumbnail menu and badge
(`app/library/cell.rs`, 115), the loader's opening of a copy
(`app/worker/loader.rs`, 104) and a mechanical refactor that lets Build
Editable Previews draw Build Previews' progress bar (`preview_build.rs`, +111
−72, a commit of its own). The rest are small: the saving rule (`edits.rs` and
the catalog's edit rows), Develop's tools turned off for a copy, and one-line
changes where a new field is passed.

The tests add about 3,270 lines more (2,527 in test files, the rest in
inline test modules), most of them in the catalog's, the copy's, the build's
and Develop's own tests.

## The parts

Each part is one or more commits, and each leaves the app working:

1. **The catalog**: the `editable_previews` table, removal, Set Copy as Master,
   Export as Catalog, and clears checked as saves are
   (`edits::Original`, `Catalog::check_edit`, with the Sync Undo and Redo
   callers).
2. **The copy's file**: export writes it, decode opens it and reads its record,
   with the render check.
3. **Develop opens an offline photo from its copy** under the saving rule: the
   Library's choice (`Refusal`, `develop_from`), the loader, autosave and the
   flush passing the recorded identity, the preview label, AI masks, AI Denoise,
   AI Remove and Fit Camera Look off, Upright analysing the copy, thumbnails kept under the copy's path, and the
   Library's badge. Copies that exist open; none are built yet.
4. **Build and Discard**: the worker, the menu, its row in `app::work`, cancel,
   shutdown and the cleanup of files no row names.

## The copy: a linear DNG

The copy is a DNG linear raw (PhotometricInterpretation 34892): the
demosaiced camera image with highlights already recovered, reduced to 2560 px,
in lossless 16-bit JPEG tiles (Compression 7, ITU T.81 process 14), DNG's
lossless coding for 16-bit integer raw data since its first version.
DNGBackwardVersion is 1.1, as Adobe's own writer gives it. WhiteLevel is 16383,
so samples keep 2 stops above white. `rawmakase-decode`'s DNG reader opens it
unchanged. Integer linear raw develops as `(sample − black) / (white − black)`
with no clamp at 1.

Measured on 13 CC0 RAWs, 9 makes (#1022):

- Renders differ from a render of the reduced image itself, with no file in
  between, by at most 0.013 ΔE2000 on average, under the default recipe, with
  detail off, and at +4 and −3 EV.
- The files average 16.1 MB (12.8–20.8 MB). In JPEG XL tiles they averaged
  13.0 MB, and in Deflate tiles 17.3 MB; neither is used (below).

**Why not JPEG XL or Deflate.** The plan first used DNG 1.7's lossless JPEG XL
tiles (Compression 52546) with `zune-jpegxl`, the lossless encoder export
already uses. Its 16-bit lossless output can't be decoded, by jxl-oxide or by
libjxl, when a gradient runs into a flat area, as a clipped sky does: a copy of
such a photo would have been written and then never opened. zune-jpegxl 0.5.2
is its latest release. Deflate tiles (Compression 8, with Predictor 2) came
next, but DNG allows Deflate only for floating-point, 32-bit integer,
transparency mask and depth map data (DNG 1.7.1, p. 21, and the DNG SDK's
checks), not for 16-bit integer samples. Lossless JPEG is DNG's own lossless
coding for them, and RAWmakase's DNG reader decodes it already. JPEG XL tiles
can replace lossless JPEG's once the encoder is fixed; the reader opens both.

The standard tags carry the pixels, the colour matrix, white balance,
orientation, EXIF and Adobe's original-size tags (51089–51091). Another DNG
reader can open the file: LibRaw 0.22 reads a copy's samples exactly, except
that it places the rows of a copy narrower than one tile (512 px) wrongly.

**Export writes the copy; decode opens it.** `rawmakase-export`
(`export::editable_preview`) makes it from the photo's full-size decode, as an
export is made, encoding its tiles with a lossless JPEG encoder of its own
(`ljpeg.rs`), which needs no dependency. `rawmakase-decode`
(`editable_preview`) owns the record's format and reads it, and opens the copy
as the photo again. Decode gains no encoder:
`scripts/crate-closures.sh` now fails if `rawmakase-decode` reaches
`zune-jpegxl` or `jixel`. The app's build path calls export's writer through
its one seam to a copy's file (`app::worker::editable_preview`).

## Answers to the questions in #1022

### 1. Where the facts record lives: in the DNG

The facts the standard tags can't carry go in **DNGPrivateData** (tag 50740), as
one small versioned record. The catalog stores only a row per photo.

The record starts with `RAWmakase\0`, as DNGPrivateData records begin with
their maker's name, followed by a format version (u16) and the fields. Each
field is written by name, with its default stated, not as a serialised internal
struct. It holds:

- the original's full size, and the copy's;
- the RAW's own lens corrections, camera white balance presets, camera look,
  baseline exposure, and Fujifilm's dynamic range and exposure shift;
- the Auto CA measured on the original;
- for a camera RAWmakase has no colour data for, the default Camera Look fit;
- the original's other facts as its decode had them (the make and model as
  RAWmakase names them, the colour matrices, white balance, crop and
  orientation among them). The standard tags carry the same facts for other DNG
  readers, but rounded to rationals, or as another reader names them, so
  RAWmakase opens the copy with the record's.

Without these, 5 of the 13 files rendered visibly differently from the
original.

**The record holds no identity of the original.** Which file the copy was built
from is kept in one place, the catalog row (question 3 says how a copy and its
row are kept consistent). The format hasn't been released, so this is
version 1 as it stands; nothing is bumped.

A DNG with a transparency mask (a Lightroom panorama) isn't given a copy yet:
the mask is at the original's size and version 1 has no place for it.

Why in the file rather than a catalog column:

- **One thing to carry.** A copy and its facts can't come apart in Export as
  Catalog, a backup or a moved catalog folder.
- **The catalog row stays the same for any copy.** A Smart Preview imported
  later is a linear DNG with no record. Its facts come from its tags alone, as
  for any DNG.

A record whose version this release doesn't know makes the copy out of date. It
is not read. While the original is online, Build makes a new copy. While it is
offline, Develop doesn't open the photo and says the copy needs rebuilding. A
copy with no record, or a damaged one, is treated the same way. There is no
migration code.

The catalog gains one table, added the way `photo_checksums` was: no migration,
and nothing an older release can't open or edit in. It is still a change an
older release notices. This release creates the empty table in every catalog it
opens, and from then on the previous release's Export as Catalog refuses that
catalog ("A later release of RAWmakase added to this catalog"), as it refuses
one with `photo_checksums` and the other recent tables. In `schema.sql`:

```sql
CREATE TABLE IF NOT EXISTS editable_previews (
    photo INTEGER PRIMARY KEY REFERENCES photos(id) ON DELETE CASCADE, -- the master
    file TEXT NOT NULL,            -- file name in the catalog's copies folder
    format TEXT NOT NULL,          -- 'rawmakase-dng-1'; an importer adds its own
    original TEXT NOT NULL,        -- the original's Identity JSON, as recorded
    out_of_date INTEGER NOT NULL DEFAULT 0,
    built_at TEXT NOT NULL
);
```

`schema.postgres.sql` has the same table, with `BIGINT` where photo ids and
integers are `BIGINT` there, as in `photo_checksums`.

The row is the master's, so virtual copies share it:

- **Set Copy as Master** moves the row to the new master in its own
  transaction, as it moves the checksum. A row already on the new master (left
  by an older release, below) is replaced, and its file is then one no row
  names.
- **An older release** may promote a copy without knowing the table. The row
  then sits on what is now a virtual copy, where nothing looks for it: the
  family has no copy. Nothing repairs it when the catalog opens. It is treated
  as the harder case already was, an older release promoting a copy and then
  removing the former master: the copy is lost, and Build makes a new one while
  the original is online. Offline, the photo can't be edited until its original
  is back. The old row goes with the photo it sits on, through the cascade, and
  its file is then deleted as any removed photo's is (below). Opening a catalog
  is unchanged.
- **Removing a photo** removes its row through the cascade, whichever release
  removes it. Removing a virtual copy leaves the master's row alone.

Copies live beside the catalog, in `<catalog name>.rawmakase-editable-previews/`,
named `<photo id>-<identity key>.dng`. For a catalog that isn't a file, they
live in the data folder. A file is deleted after the transaction that drops its
row commits.

As built, Discard and a rebuild hand back the files whose rows they dropped,
for the caller to delete after the commit. Removing a photo drops its row
through the cascade alone, in every removal path; its file is then one no row
names, which `Catalog::editable_preview_orphans` lists. The app deletes those
files on the frame after the removal commits: every workflow that may remove
photos (Remove from Catalog, Move to Trash, removing a folder, Synchronize)
ends with the catalog session starting a new generation, and the build poll,
which runs every frame, deletes the files no row names once per generation.
While a build is under way, or its file not yet recorded, that waits until
nothing being built could still record one. The same sweep deletes a file an
older release left when it removed a photo, and one whose deletion failed;
Build and Discard also run it. Only a file catalog exists today, so the
data-folder case has nothing to decide yet.

### 2. Caches

Nothing rendered from a copy is stored where the original's renders would find
it:

- **Decode cache.** It is keyed by the identity of the file decoded. A copy's
  decode is keyed by the copy file, so it is never the original's.
- **Edited thumbnails.** The preview cache keeps these by source file path,
  with the edit's tag (`EditSource::preview_tag`). A write is checked against
  the file's stamp (`PreviewCache::store_tagged`), which is why Develop's
  thumbnail of an offline photo isn't cached today. A thumbnail rendered from a
  copy is stored under the **copy's** path and stamp, and the same check holds
  for it.
  - The Library looks under the copy's path only for a photo that is offline
    and has an up-to-date copy, so it shows the offline edit.
  - Online, it looks under the original's path alone, so it never finds a
    thumbnail rendered from a copy.
  - Once the original is back, the thumbnail stored there carries the older
    edit's tag. It misses, and the thumbnail is rendered again from the
    original.
- **Standard and 1:1 previews, AI Denoise results and Camera Look fits** are
  never made from a copy.
- **Export** of an offline photo stays refused (`availability_refusal`), and
  automation's `export` is refused too.

A test edits a photo offline, brings the original back, and checks that the
Library and Develop render from the original rather than the copy.

### 3. The saving rule

The rule: **an edit may be saved against the recorded identity only while
nothing is at the original's path.** That means `path.try_exists() == Ok(false)`.
A permission error, a share that stops answering, or any other read error still
refuses the save, as it does today.

**The recorded identity is the catalog row's**, the only place it is kept. The
saving rule, the out-of-date flag, Build's check that a copy is already of the
original as it is, and the loader all use it.

**When a copy goes out of date.** Whenever the Library learns which file a
photo's original is, it compares that file with the recorded identity
(`Identity::same_photo`), and a different photo marks the copy out of date:
Develop opening the original (`Catalog::note_original`), relinking a folder or
root and clearing a location (each master with a copy that the change will find
at another path is read there before the change's transaction, so no file is
opened under the catalog's write lock; in the transaction a copy is marked only
where the change put its original at the path read, and the change and its
marks commit together), and the moves Synchronize and Find on Disk apply (in the move's
own transaction, with the identity the scan read). A location with no file at
it tells nothing, since the photo may only be offline. A file replaced in place
isn't seen by any of these, as the Library doesn't read files it isn't asked
to; Develop opening it, or Build, marks the copy then, and until then a save
made from the copy while the original is offline is protected once the file
is back, by the saving rule.

**How a copy and its row are kept consistent.** The catalog makes the copy's
file name from the identity it records (`<photo id>-<identity key>.dng`, where
the key is the whole identity), in the same call that records the row
(`Catalog::record_editable_preview`): a caller can't name a file. A build reads
the original's identity before anything else reads the file, writes a copy of
that file under that name, reads the identity again at the end, and drops the
copy if it changed. So a file a row names is always a copy of that row's
original:

- A rebuild of another file has another name. Its row replaces the old one in
  one transaction, and the old file is deleted after the commit.
- A rebuild of the same file has the same name, and the file is replaced whole
  (written under a staging name and renamed), so it is a copy of the same
  original either way.
- When Develop opens the photo, the Library reads the row and hands the loader
  the file and the identity together (`EditablePreviewSource`). If the copy is
  discarded, or rebuilt for another file, before the loader opens it, the file
  is gone: opening fails with the reason, and the photo can be opened again. The
  loader doesn't need to compare the copy with the row, and the copy has nothing
  to compare it with.

The rest of the rule:

- **A file deleted or renamed on a drive that is still connected** counts as
  absent, and saving works. To the catalog, a renamed file is a missing file,
  and the Library already shows it as offline. The edit stays bound to the
  recorded identity. Find on Disk to the renamed file brings it back with its
  edit, because the identity matches.
- **Opening** follows the same rule, in the same function. Develop opens a
  photo from its copy only when its original is absent and its copy is up to
  date. The saved edit is then checked against the recorded identity
  (`EditRecord::saved_for`) instead of reading the file. In every other case
  the photo opens from its original, or fails as it does today.
- **An offline edit when the original comes back changed.** "Changed" means a
  different size or first bytes (`Identity::same_photo`); a new modification
  time alone is not a change. Such an edit is treated exactly like an edit made
  online before its file was replaced: it is protected. Develop opens the photo
  with a notice and doesn't save over the edit, and the copy is marked out of
  date. This isn't a new kind of stuck edit. It is the one that exists today
  whenever a file is replaced under its edit, and it has the same cause.
  Letting the user apply a protected edit to the file now at its path would
  help both cases, so it belongs in a change of its own, not in this one.

### 4. The cut features

The save path only uses the recorded identity when the caller passes one in.
The new type `edits::Original` holds a path and an optional recorded identity,
and only Develop's saves of a photo opened from its copy pass one (autosave,
and the flush before navigating or closing). Everything else passes the path
alone, and the save reads the file as it does now. So:

- **Library Sync, Paste Settings, presets on the selection and Quick Develop**
  read each target's file (`Identity::read` in `library::sync`) before they
  write, and fail on an offline photo before reaching the save. They stay
  refused offline, unchanged.
- **An extension's proposal** to a photo that isn't open is checked the same
  way, against its file.
- **In Develop, on the photo open from its copy,** Paste Settings and a preset
  change the open photo's recipe as one History step
  (`settings_transfer::apply_settings`), as a slider does, and are saved the
  same way. Sync from Develop to other photos takes the Library's path above,
  so offline targets are refused.

**Clears go through the same check.** Before this change, `EditChange::Clear`
read no file. Undo of a Sync or preset therefore cleared a photo that had no
edit before it, even while the photo was offline. If it stayed that way, Undo
of an earlier Sync could clear an edit made offline. So a clear carries the
photo's `Original` and the identity it is meant for, as a save does, and is
checked the same way:

- It is refused while the file can't be read, unless the caller passes the
  recorded identity, and the Library's Undo doesn't pass one.
- Undo of a Sync over an offline photo is then refused whether or not the photo
  had an edit before. Before, only an edited one was refused. It fails with the
  reason, as a save there does.

Tests:

- Sync, Paste Settings and a preset over a selection that includes an offline
  photo with a copy each leave its edit unchanged and report it.
- Undo and Redo of such a Sync, for a target edited before it and one that
  wasn't, each leave an offline photo's edit unchanged.
- The same saves and clears through the save path with no recorded identity
  are refused.

### 5. Tests

**Catalog (`rawmakase-catalog`):**

- Removing the photo (Remove from Catalog, Move to Trash) removes its row in
  the same transaction; the app deletes the file after commit (below). A failed
  removal keeps both.
- Removing a virtual copy keeps the master's copy.
- Virtual copies open from their master's copy, and only a master records one.
- The file a row names is the one its identity gives.
- Set Copy as Master moves the row. After that, the family opens offline from
  the copy, and removing the former master (now a virtual copy) keeps it. Over
  a row an older release left on the new master, the family's row takes its
  place.
- A row an older release's promotion left on a virtual copy is not found for
  the family. Recording again makes a new one for the new master, and the old
  row and file go with the former master.
- Removing a photo with SQL alone, as an older release would, drops the row
  through the cascade. The app's sweep deletes the file left behind.
- Relinking a folder or root, clearing a location, Find on Disk and
  Synchronize, each through its own catalog call, and Develop opening the
  original:
  - to the same file, the copy stays up to date;
  - to a location with no file, it stays as it is;
  - to a different file, the copy is out of date and is never opened.
- Export as Catalog carries the copies of the photos it exports, and only
  those, and only plain files: a link named as a copy isn't followed.
- The saving rule: a save with a recorded identity is
  - allowed while the original is absent;
  - refused when the path is unreadable rather than absent (a folder without
    permission);
  - refused once a different file is at the path;
  - checked against the file again once the same file is back.
- An unknown record version, a missing file and a truncated file each make the
  copy out of date, with no panic.

**Builds (`app::library::editable_previews`):**

- A build cancelled, failing, or panicking clears the busy state.
- A build whose photo is removed, relinked or rebuilt meanwhile writes nothing.
- Removing photos deletes their copies' files on the next frame, with no Build
  or Discard after it; a removal while a build is under way deletes them once
  it finishes, its own file included.
- Switching catalogs while a build runs drops its results and leaves no row in
  either catalog.
- Discard while a build runs.

**Develop (`app::develop`):**

- An offline photo opens from its copy, is labelled as a preview, and has AI
  masks, AI Denoise, AI Remove and Fit Camera Look turned off, each saying to
  connect the original.
- Upright analyses the copy, which is final as it opens: no full-size image
  follows it, as one follows an original's half-size draft.
- Its edits save as the original's.
- They render from the original once it is back.
- The caches (above) and the cut features (above).

**Rendering:** an automated check in CI. Its copy is written and read back, and
its renders are compared with the original's, reduced, under the default recipe
and at +4 and −3 EV. The bounds are set from the measured differences, not
chosen to pass. Separately, the written DNG is checked to open as an ordinary
linear DNG, with the record ignored.

As built, **the check runs on two of the synthetic chart DNGs already committed
under `tests/corpus/charts`, not on a CC0 camera RAW**: the repository has no
CC0 camera RAW small enough to commit, and these charts are 970 × 742 mosaics
made by the repository's own generator, so no one else's rights are involved.
The 13 CC0 RAWs of #1022 were measured by hand and are not in CI. The charts'
copies are reduced to 480 px instead of 2560, so they are reduced decodes as a
camera's 2560 px copies are. Each render is compared with the reduced image
itself, with no file in between (at most 0.11 mean, 1.3 p99 ΔE2000, at +4 EV),
and with the original rendered at the copy's size, where the copy may add little
to what the reduction alone costs. The chart's flat, noiseless shadows show the
16-bit samples' steps at +4 EV more than a photo's noise lets them show, so its
gap is larger than the 13 RAWs'. The check is export's test, as export writes
the copy (`export::editable_preview`); decode's own tests read records and
refuse files that aren't copies, without writing one.

## Ownership

| Concern | Owner |
| --- | --- |
| Writing the DNG and its record from the full-size decode | `rawmakase-export` (`export::editable_preview`), its tiles encoded by its own lossless JPEG encoder |
| The record's format and reading it; opening a copy as a reduced image of the original's size, with highlights not recovered twice | `rawmakase-decode` (`editable_preview`). The DNG reader itself is unchanged |
| Which photos have a copy, where it is, the original's identity, out of date, removal, Export as Catalog | `rawmakase-catalog` (`catalog::editable_previews`); it records files and never decodes them, and names each file after the identity it records |
| The saving and opening rule | `edits::Original` in `rawmakase-interop`, used by `Catalog::check_edit` and `EditRecord::saved` |
| Whether Develop opens an offline photo, and what is turned off | `app::library::Refusal` and Develop's tools |
| Build, Discard, progress and cancellation | `app::library::editable_previews` and a row in `app::work` |

**Why Build Editable Previews has its own worker.** Build Standard-Sized
Previews (`app::library::preview_build`, #213) was considered first, as the
review asked. Its worker is a preview-cache worker: its thread holds the cache's
connection, and between builds it runs the cache's upkeep (requests recorded,
moved and forgotten, Discard and Clear, expiry, a refresh after each save) and
gives back the space discarded previews leave. Its queue is keyed by photo and
preview kind, and Cancel, Discard and Clear filter by kind and bump a
generation that drops refreshes. A copy needs none of that: no cache, no kind,
no refresh, and its results are recorded in the catalog rather than shown in
the grid. It needs what the preview worker lacks: a discard counter per photo,
so a result for a photo discarded since is dropped, and a count of finished but
unrecorded builds, so cleanup never deletes one. Putting both in one queue
would make every item, outcome and filter of the preview worker a choice between
the two. So Build Editable Previews is its own module,
`app::library::editable_previews`, with the same pattern: inputs captured once
per request, one photo at a time on two background threads, cancel between
photos and inside the build, and joined at exit within the shutdown deadline.
It shares what fits unchanged: Build Previews' `Progress` and its progress bar,
which a mechanical commit lets both draw. It is not under `preview_build`.

Copies are RAWmakase's own derived files, not a user's photos. Their owner
deletes them outright, as it does the preview cache's.
`docs/architecture.md` says so beside the rule that RAWmakase never deletes
a user's file.
