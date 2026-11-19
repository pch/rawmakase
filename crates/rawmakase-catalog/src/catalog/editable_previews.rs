//! Each master photo's editable preview (docs/editable-previews.md): a reduced
//! copy of its original, built while the original was online, which Develop
//! edits the photo from while the original is offline. This module records
//! which photos have one, the file's name in the catalog's copies folder
//! ([`folder`]), its format, the original's [`Identity`] it was built from and
//! whether it is out of date. It records files and never decodes them: whether
//! a file can be read is `rawmakase_decode::editable_preview`'s to say, and any
//! file that can't be is treated as out of date ([`EditablePreview::usable`]).
//!
//! The row is the master's, so virtual copies share it. Set Copy as Master
//! moves it with the checksum (`copies`). An older release's promotion, which
//! doesn't know the table, leaves the row on what is then a virtual copy, where
//! nothing looks for it: the family has no copy, and Build makes a new one while
//! the original is online. Removing a photo drops its row through
//! `ON DELETE CASCADE`, whichever release removes it.
//!
//! The row is the one place the original's identity is kept: the saving rule,
//! the out-of-date flag and Build's check all read it, and the copy's file holds
//! none. The file's name is made from that identity ([`file_name`]) when the
//! row is recorded, so a row and its file can't disagree about which original
//! the copy is of.
//!
//! Copies are RAWmakase's own derived files, not a user's photos: their owner
//! deletes them outright. A file is deleted after the transaction that drops its
//! row commits, by whoever ran it ([`Catalog::discard_editable_previews`] and
//! [`Catalog::record_editable_preview`] return the files that went); a file no
//! row names, such as one left by a removal or by an older release, is listed
//! by [`Catalog::editable_preview_orphans`] for the app to delete once the
//! removal has committed.
//!
//! A copy goes out of date when the catalog learns its photo's original is
//! another file than the one it was built from: [`Catalog::note_original`],
//! relinks and cleared locations ([`read_relocated`] before the change's
//! transaction, [`relocated`] in it), and the moves
//! Synchronize and Find on Disk apply ([`note_found`]).
use super::db::{Reads, Write, sql};
use super::folder_sync::LocationRows;
use super::value::row;
use super::{Catalog, CatalogLocation, PhotoId};
use crate::storage::Identity;
use anyhow::{Context, Result, ensure};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The format of the copies this release builds: a linear DNG with
/// RAWmakase's record in its DNGPrivateData. An importer of other copies
/// records its own.
pub const RAWMAKASE_DNG: &str = "rawmakase-dng-1";

/// A master photo's editable preview, as the catalog records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditablePreview {
    /// The master photo it belongs to, which its virtual copies share.
    pub master: PhotoId,
    /// The file's name in the catalog's copies folder ([`folder`]).
    pub file: String,
    /// What kind of copy it is, such as [`RAWMAKASE_DNG`].
    pub format: String,
    /// The original's identity when the copy was built.
    pub original: Identity,
    /// The original came back as another file than the one the copy was
    /// built from: the copy is never opened, and Build replaces it.
    pub out_of_date: bool,
    /// When the copy was recorded, "YYYY-MM-DD HH:MM:SS" UTC.
    pub built_at: String,
}

impl EditablePreview {
    /// The copy's file in `folder`, the catalog's copies folder; `None` when
    /// the recorded name isn't a plain file name, so a damaged or hostile row
    /// never names a file elsewhere.
    pub fn path_in(&self, folder: &Path) -> Option<PathBuf> {
        is_plain_name(&self.file).then(|| folder.join(&self.file))
    }
    /// The copy's file in `folder` when it may be opened: not out of date, and
    /// `opens` reads it without error. Anything else, a missing or damaged file
    /// or a record of a version the reader doesn't know among them, makes the
    /// copy out of date, which is never a reason to fail.
    pub fn usable(
        &self,
        folder: &Path,
        opens: impl FnOnce(&Path) -> Result<()>,
    ) -> Option<PathBuf> {
        let path = self.path_in(folder).filter(|_| !self.out_of_date)?;
        opens(&path).is_ok().then_some(path)
    }
}

/// The folder a catalog at `location` keeps its editable previews in:
/// `<catalog name>.rawmakase-editable-previews` beside a catalog file.
pub fn folder(location: &CatalogLocation) -> PathBuf {
    match location {
        CatalogLocation::File(path) => {
            let mut name = path.file_stem().unwrap_or_default().to_os_string();
            name.push(".rawmakase-editable-previews");
            crate::storage::parent_dir(path).join(name)
        }
    }
}

/// The name a copy of `master` built from the file `original` names takes in
/// the copies folder: `<photo id>-<identity key>.dng`.
pub fn file_name(master: PhotoId, original: &Identity) -> String {
    format!("{}-{}.dng", master.0, original.key())
}

/// Whether `name` is a single plain file name: not empty, not `.` or `..`,
/// without a separator of any system, and not hidden.
pub(super) fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains(['/', '\\', ':', '\0'])
        && Path::new(name).file_name() == Some(name.as_ref())
}

row! {
    struct Row {
        master: PhotoId,
        file: String,
        format: String,
        original: String,
        out_of_date: bool,
        built_at: String,
    }
}
impl Row {
    fn into_preview(self) -> Result<EditablePreview> {
        Ok(EditablePreview {
            master: self.master,
            original: serde_json::from_str(&self.original)
                .context("An editable preview's original identity can't be read")?,
            file: self.file,
            format: self.format,
            out_of_date: self.out_of_date,
            built_at: self.built_at,
        })
    }
}

impl Catalog {
    /// The folder this catalog keeps its editable previews in ([`folder`]).
    pub fn editable_previews_folder(&self) -> PathBuf {
        folder(&self.location)
    }
    /// The editable preview photo `id` opens from: its master's, which a
    /// virtual copy shares. `None` without one, or for a photo not in the
    /// catalog.
    pub fn editable_preview(&self, id: PhotoId) -> Result<Option<EditablePreview>> {
        let row: Option<Row> = self.db.read_optional(
            sql!(
                "SELECT e.photo, e.file, e.format, e.original, e.out_of_date, e.built_at
                 FROM photos p JOIN editable_previews e ON e.photo = COALESCE(p.master_id, p.id)
                 WHERE p.id=?"
            ),
            &[&id],
        )?;
        row.map(Row::into_preview).transpose()
    }
    /// Every editable preview the catalog records, by master.
    pub fn editable_previews(&self) -> Result<Vec<EditablePreview>> {
        let rows: Vec<Row> = self.db.read(
            sql!(
                "SELECT photo, file, format, original, out_of_date, built_at
                 FROM editable_previews ORDER BY photo"
            ),
            &[],
        )?;
        rows.into_iter().map(Row::into_preview).collect()
    }
    /// Records RAWmakase's copy ([`RAWMAKASE_DNG`]) of master photo `id`'s
    /// original as the file `original` names, in place of any copy recorded
    /// before. The copy's file is the one [`file_name`] names in the copies
    /// folder: the name is made here from the identity recorded with it, never
    /// taken from the caller, so a row naming a file always names a copy of its
    /// own original. Returns the earlier copy's file when it had another name,
    /// for the caller to delete once this returns; never a name that isn't a
    /// plain file name, so a damaged or hostile row can't send a deletion
    /// outside the copies folder. Refused for a photo no longer in the catalog
    /// or no longer a master (Set Copy as Master ran since its copy was built
    /// under its id).
    pub fn record_editable_preview(
        &mut self,
        id: PhotoId,
        original: &Identity,
    ) -> Result<Option<String>> {
        let file = file_name(id, original);
        let identity = serde_json::to_string(original)?;
        let built_at = rawmakase_model::time::now_text();
        self.db.write(|w| {
            let master: PhotoId = w
                .read_optional(
                    sql!("SELECT COALESCE(master_id, id) FROM photos WHERE id=?"),
                    &[&id],
                )?
                .context("The photo is no longer in the catalog")?;
            ensure!(
                master == id,
                "The photo is no longer a master photo: its editable preview is its master's"
            );
            let earlier: Option<String> = w.read_optional(
                sql!("SELECT file FROM editable_previews WHERE photo=?"),
                &[&id],
            )?;
            w.execute(
                sql!(
                    "INSERT INTO editable_previews(photo, file, format, original, out_of_date, built_at)
                     VALUES (?, ?, ?, ?, 0, ?)
                     ON CONFLICT(photo) DO UPDATE SET file=excluded.file, format=excluded.format,
                         original=excluded.original, out_of_date=0, built_at=excluded.built_at"
                ),
                &[&id, &file, &RAWMAKASE_DNG, &identity, &built_at],
            )?;
            Ok(earlier.filter(|earlier| *earlier != file && is_plain_name(earlier)))
        })
    }
    /// Notes that photo `id`'s original is now the file `now` names: when that
    /// is not the photo its editable preview was built from
    /// ([`Identity::same_photo`]; a new modification time alone is no change),
    /// the copy is marked out of date. Returns the copy as it is then, `None`
    /// without one. Writes only when the copy goes out of date.
    pub fn note_original(
        &mut self,
        id: PhotoId,
        now: &Identity,
    ) -> Result<Option<EditablePreview>> {
        let Some(mut preview) = self.editable_preview(id)? else {
            return Ok(None);
        };
        if !preview.out_of_date && !preview.original.same_photo(now) {
            self.mark_editable_preview_out_of_date(id)?;
            preview.out_of_date = true;
        }
        Ok(Some(preview))
    }
    /// Marks photo `id`'s editable preview, its master's, out of date. Returns
    /// whether there was one to mark.
    pub fn mark_editable_preview_out_of_date(&mut self, id: PhotoId) -> Result<bool> {
        let changed = self.db.write(|w| {
            w.execute(
                sql!(
                    "UPDATE editable_previews SET out_of_date=1
                     WHERE photo=(SELECT COALESCE(master_id, id) FROM photos WHERE id=?)"
                ),
                &[&id],
            )
        })?;
        Ok(changed > 0)
    }
    /// Forgets the editable previews of photos `ids` (their masters', so a
    /// virtual copy's discards its master's) in one transaction. Returns their
    /// files, for the caller to delete once this returns; a recorded name that
    /// isn't a plain file name is forgotten but never returned, as for
    /// [`Catalog::record_editable_preview`].
    pub fn discard_editable_previews(&mut self, ids: &[PhotoId]) -> Result<Vec<String>> {
        self.db.write(|w| {
            let mut files = Vec::new();
            for id in ids {
                let master: Option<PhotoId> = w.read_optional(
                    sql!("SELECT COALESCE(master_id, id) FROM photos WHERE id=?"),
                    &[id],
                )?;
                let Some(master) = master else { continue };
                let file: Option<String> = w.read_optional(
                    sql!("SELECT file FROM editable_previews WHERE photo=?"),
                    &[&master],
                )?;
                if let Some(file) = file {
                    w.execute(
                        sql!("DELETE FROM editable_previews WHERE photo=?"),
                        &[&master],
                    )?;
                    if is_plain_name(&file) {
                        files.push(file);
                    }
                }
            }
            Ok(files)
        })
    }
    /// The files in the copies folder no row names: copies of photos removed
    /// since (by this release, or an older one that doesn't know the table),
    /// replaced or discarded copies whose deletion failed. Hidden files, files
    /// still being written (`storage::staging_file`'s `.partial` ones) and
    /// folders are left out. Empty when there is
    /// no copies folder.
    pub fn editable_preview_orphans(&self) -> Result<Vec<PathBuf>> {
        let folder = self.editable_previews_folder();
        let entries = match std::fs::read_dir(&folder) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let named: std::collections::HashSet<String> = self
            .db
            .read::<String>(sql!("SELECT file FROM editable_previews"), &[])?
            .into_iter()
            .collect();
        let mut orphans = Vec::new();
        for entry in entries {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if is_plain_name(&name)
                && !name.ends_with(".partial")
                && !named.contains(&name)
                && entry.file_type()?.is_file()
            {
                orphans.push(entry.path());
            }
        }
        orphans.sort();
        Ok(orphans)
    }
}

/// The originals a location change will find at other files, read before its
/// transaction so no file is opened while the catalog's write lock is held: for
/// each master with an editable preview whose original `computer` would find
/// at another path were its location rows changed by `edit` (as the change
/// will write them), that path and the identity of the file there. A file that
/// isn't there or can't be read tells nothing, since the photo may only be
/// offline, and is left out.
pub(super) fn read_relocated(
    db: &impl Reads,
    computer: &str,
    edit: impl FnOnce(&mut LocationRows),
) -> Result<HashMap<PhotoId, (PathBuf, Identity)>> {
    let before = originals_located(db, computer, |_| {})?;
    let mut found = HashMap::new();
    for (master, path) in originals_located(db, computer, edit)? {
        if before.get(&master) == Some(&path) {
            continue;
        }
        if let Ok(now) = Identity::read(&path) {
            found.insert(master, (path, now));
        }
    }
    Ok(found)
}

/// Runs `change` in `w`'s transaction, then marks out of date in the same
/// transaction each copy whose original `found` ([`read_relocated`]) read
/// beforehand is another photo ([`note_found`]): the change and the copies it
/// puts out of date commit together, or neither does. A master the change
/// leaves at another path than the one read is left as it is, as for a
/// location with no file.
pub(super) fn relocated<T>(
    w: &mut Write<'_>,
    computer: &str,
    found: &HashMap<PhotoId, (PathBuf, Identity)>,
    change: impl FnOnce(&mut Write<'_>) -> Result<T>,
) -> Result<T> {
    let changed = change(w)?;
    if !found.is_empty() {
        let now = originals_located(w, computer, |_| {})?;
        for (master, (path, identity)) in found {
            if now.get(master) == Some(path) {
                note_found(w, *master, identity)?;
            }
        }
    }
    Ok(changed)
}

/// Where `computer` finds the original of each master with an editable
/// preview, by the catalog's locations as `db` has them changed by `edit`; a
/// photo whose folder can't be on this computer is left out.
fn originals_located(
    db: &impl Reads,
    computer: &str,
    edit: impl FnOnce(&mut LocationRows),
) -> Result<HashMap<PhotoId, PathBuf>> {
    row! {
        struct Original {
            photo: PhotoId,
            folder: super::FolderId,
            filename: String,
        }
    }
    let rows: Vec<Original> = db.read(
        sql!(
            "SELECT e.photo, p.folder, p.filename
             FROM editable_previews e JOIN photos p ON p.id = e.photo"
        ),
        &[],
    )?;
    if rows.is_empty() {
        return Ok(HashMap::new());
    }
    let folders = super::folder_sync::locate_folders_as(db, computer, edit)?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let path = folders.get(&r.folder)?.path.as_ref()?;
            Some((r.photo, path.join(&r.filename)))
        })
        .collect())
}

/// Notes, in `w`'s transaction, that master photo `master`'s original is now
/// the file `now` names, as [`Catalog::note_original`] does: a file that is not
/// the photo its editable preview was built from puts the copy out of date.
pub(super) fn note_found(w: &mut Write<'_>, master: PhotoId, now: &Identity) -> Result<()> {
    let original: Option<String> = w.read_optional(
        sql!("SELECT original FROM editable_previews WHERE photo=? AND out_of_date=0"),
        &[&master],
    )?;
    let Some(original) = original else {
        return Ok(());
    };
    // A record that can't be read is no copy of this file either.
    let same = serde_json::from_str::<Identity>(&original).is_ok_and(|was| was.same_photo(now));
    if !same {
        w.execute(
            sql!("UPDATE editable_previews SET out_of_date=1 WHERE photo=?"),
            &[&master],
        )?;
    }
    Ok(())
}

/// Copies the files of `copy`'s editable previews, an exported catalog's,
/// from `from`, the exporting catalog's copies folder, into `to`, which is
/// made when there is one to copy. A row whose file isn't in `from` is
/// dropped from `copy`. Returns how many were copied.
pub(super) fn carry(from: &Path, copy: &mut Catalog, to: &Path) -> Result<usize> {
    let mut carried = 0;
    let mut missing = Vec::new();
    for preview in copy.editable_previews()? {
        // Only a plain file: a link named as a copy isn't followed out of the
        // copies folder.
        let plain = |source: &PathBuf| {
            std::fs::symlink_metadata(source).is_ok_and(|m| m.file_type().is_file())
        };
        match preview.path_in(from).filter(plain) {
            Some(source) => {
                std::fs::create_dir_all(to)?;
                std::fs::copy(&source, to.join(&preview.file))?;
                carried += 1;
            }
            None => missing.push(preview.master),
        }
    }
    copy.discard_editable_previews(&missing)?;
    Ok(carried)
}
