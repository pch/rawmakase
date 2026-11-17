# Editable previews: plan

Status: Plan (2026-11-17), not built. Scope agreed in #1022; this page answers
the questions asked there before any code is written.

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
   preview. AI masks, AI Denoise and Fit Camera Look are turned off. Export of
   an offline photo stays refused.
3. **One rule for saving offline**, in the save path every edit goes through
   (below).
4. **The copies stay with their photos.** They're removed with the photo,
   carried by Export as Catalog, marked out of date when the original comes back
   changed, and shown in the Library.

Left out: a Lossless/Smaller choice, automation, Sync, Paste Settings and
presets on offline photos from the Library, and full-pace build tuning. Copies
are built for RAWs only. Lightroom's Smart Previews are not imported, but the
storage leaves room for an importer to add them later.

## The copy: a linear DNG

The copy is a DNG 1.7 linear raw (PhotometricInterpretation 34892): the
demosaiced camera image with highlights already recovered, reduced to 2560 px,
in lossless 16-bit JPEG XL tiles (Compression 52546). WhiteLevel is 16383, so
samples keep 2 stops above white. `rawmakase-decode`'s DNG reader opens it
unchanged. Integer linear raw develops as `(sample − black) / (white − black)`
with no clamp at 1.

Measured on 13 CC0 RAWs, 9 makes (#1022):

- Renders differ from a render of the reduced image itself, with no file in
  between, by at most 0.013 ΔE2000 on average, under the default recipe, with
  detail off, and at +4 and −3 EV.
- The files average 13.0 MB.

The standard tags carry the pixels, the colour matrix, white balance,
orientation, EXIF and Adobe's original-size tags (51089–51091). Another DNG
reader can open the file.

## Answers to the questions in #1022

### 1. Where the facts record lives: in the DNG

The facts the standard tags can't carry go in **DNGPrivateData** (tag 50740), as
one small versioned record. The catalog stores only a row per photo.

The record starts with `RAWmakase\0`, as DNGPrivateData records begin with
their maker's name, followed by a format version (u16) and the fields. Each
field is written by name, with its default stated, not as a serialised internal
struct. It holds:

- the original's file identity (size, modification time, first-bytes hash) and
  full size;
- the RAW's own lens corrections, camera white balance presets, camera look,
  baseline exposure, and Fujifilm's dynamic range and exposure shift;
- the Auto CA measured on the original;
- for a camera RAWmakase has no colour data for, the default Camera Look fit.

Without these, 5 of the 13 files rendered visibly differently from the
original.

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

The catalog gains one table, added the way `photo_checksums` was, with no
format change. In `schema.sql`:

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
  transaction, as it moves the checksum.
- **An older release** may promote a copy without knowing the table, and the
  row then sits on what is now a virtual copy. When the catalog opens, a read
  looks for such rows. Only if it finds one does a write transaction check
  again and move the row to its photo's master. A catalog with nothing to
  repair opens without taking a write lock, as it does today (`open_tests`).
  - **One sequence it can't repair.** If an older release promotes a copy *and
    then removes the former master* before this release opens the catalog,
    the cascade removes the family's row with that photo. The copy is then
    rebuilt like any missing one while the original is online. Offline, the
    photo can't be edited until its original is back. A test covers this.
- **Removing a photo** removes its row through the cascade, whichever release
  removes it. Removing a virtual copy leaves the master's row alone.

Copies live beside the catalog, in `<catalog name>.rawmakase-editable-previews/`,
named `<photo id>-<identity key>.dng`. For a catalog that isn't a file, they
live in the data folder. A file is deleted after the transaction that drops its
row commits. A file no row names, such as one left when an older release
removed its photo, is deleted the next time Build or Discard runs on that
catalog.

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

The rule: **an edit may be saved against the copy's recorded identity only
while nothing is at the original's path.** That means
`path.try_exists() == Ok(false)`. A permission error, a share that stops
answering, or any other read error still refuses the save, as it does today.

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

**Clears go through the same check.** Today `EditChange::Clear` reads no file.
Undo of a Sync or preset therefore clears a photo that had no edit before it,
even while the photo is offline. If it stayed that way, Undo of an earlier Sync
could clear an edit made offline. So a clear will carry the photo's `Original`
and the identity it is meant for, as a save does, and will be checked the same
way:

- It is refused while the file can't be read, unless the caller passes the
  recorded identity, and the Library's Undo doesn't pass one.
- Undo of a Sync over an offline photo is then refused whether or not the photo
  had an edit before. Today only an edited one is refused. It fails with the
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
  the same transaction, and the file after commit. A failed removal keeps both.
- Removing a virtual copy keeps the master's copy.
- Virtual copies open from their master's copy.
- Set Copy as Master moves the row. After that, the family opens offline from
  the copy, and removing the former master (now a virtual copy) keeps it.
- A row left on a virtual copy, as an older release's promotion leaves it, is
  moved to the master when the catalog opens.
- A catalog with no such row opens while another connection holds a write
  lock.
- An older release's promotion followed by removal of the former master
  leaves no row. Build makes a new copy, and offline the photo is refused with
  the reason.
- Removing a photo with SQL alone, as an older release would, drops the row
  through the cascade. The next Build deletes the file left behind.
- Replacing the file, Find on Disk and relinking a folder:
  - to the same file, the copy stays up to date;
  - to a different file, the copy is out of date and is never opened.
- Export as Catalog carries the copies of the photos it exports, and only
  those.
- The saving rule: a save with a recorded identity is
  - allowed while the original is absent;
  - refused when the path is unreadable rather than absent (a folder without
    permission);
  - refused once a different file is at the path;
  - checked against the file again once the same file is back.
- An unknown record version, a missing file and a truncated file each make the
  copy out of date, with no panic.

**Builds (`app::library`):**

- A build cancelled, failing, or panicking clears the busy state.
- A build whose photo is removed, relinked or rebuilt meanwhile writes nothing.
- Switching catalogs while a build runs drops its results and leaves no row in
  either catalog.
- Discard while a build runs.

**Develop (`app::develop`):**

- An offline photo opens from its copy, is labelled as a preview, and has AI
  masks, AI Denoise and Fit Camera Look turned off.
- Its edits save as the original's.
- They render from the original once it is back.
- The caches (above) and the cut features (above).

**Rendering:** an automated check in CI, on a small CC0 RAW committed under
`tests/data`. Its copy is written and read back, and its renders are compared
with the original's, reduced, under the default recipe and at +4 and −3 EV. The
bounds are set from the measured differences, not chosen to pass. Separately,
the written DNG is checked to open as an ordinary linear DNG, with the record
ignored.

## Ownership

| Concern | Owner |
| --- | --- |
| Writing the DNG and its record; reading the record; opening a copy as a reduced image of the original's size, with highlights not recovered twice | `rawmakase-decode` (`editable_preview`), beside `decode`. The DNG reader itself is unchanged |
| Which photos have a copy, where it is, out of date, removal, Export as Catalog | `rawmakase-catalog` (`catalog::editable_previews`); it records files and never decodes |
| The saving and opening rule | `edits::Original` in `rawmakase-interop`, used by `Catalog::check_edit` and `EditRecord::saved` |
| Whether Develop opens an offline photo, and what is turned off | `app::library::Refusal` and Develop's tools |
| Build, Discard, progress and cancellation | `app::library::preview_build` and a row in `app::work` |

Copies are RAWmakase's own derived files, not a user's photos. Their owner
deletes them outright, as it does the preview cache's.
`docs/architecture.md` will say so beside the rule that RAWmakase never deletes
a user's file.
