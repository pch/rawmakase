//! Lightroom's virtual copies: photos of the same file with their own edit,
//! metadata and name.
use super::db::{Reads, Write, sql};
use super::{Catalog, PhotoId};
use anyhow::{Context, Result, ensure};

impl Catalog {
    /// Lightroom's Create Virtual Copy: a new photo of the same file with the
    /// edit, rating, flag, label, keywords and descriptive metadata of `id`, named "Copy N" after
    /// its master's other copies, stacked right after `id` (see `stacks`).
    /// Returns the copy's id.
    pub fn create_virtual_copy(&mut self, id: PhotoId) -> Result<PhotoId> {
        let master: PhotoId = self
            .db
            .read_optional(
                sql!("SELECT COALESCE(master_id, id) FROM photos WHERE id=?"),
                &[&id],
            )?
            .context("Unknown photo")?;
        let name = self.unused_copy_name(master)?;
        self.db.write(|w| {
            let copy: PhotoId = w.insert_returning_id(
                sql!(
                    "INSERT INTO photos(folder,filename,original_path,captured,rating,flag,label,format,
                        copy_name,master_id,orientation,lightroom_develop)
                     SELECT folder,filename,original_path,captured,rating,flag,label,format,
                        ?,?,orientation,lightroom_develop
                     FROM photos WHERE id=?
                     RETURNING id"
                ),
                &[&name, &master, &id],
            )?;
            super::edit_rows::copy_edit(w, id, copy)?;
            w.execute(
                sql!(
                    "INSERT INTO photo_keywords(photo,keyword)
                     SELECT ?,keyword FROM photo_keywords WHERE photo=?"
                ),
                &[&copy, &id],
            )?;
            super::descriptive::copy_rows(w, id, copy)?;
            super::stacks::stack_copy(w, id, copy)?;
            Ok(copy)
        })
    }
    /// The first "Copy N" none of `master`'s copies is named.
    fn unused_copy_name(&self, master: PhotoId) -> Result<String> {
        let names: std::collections::HashSet<String> = self
            .db
            .read::<String>(
                sql!("SELECT copy_name FROM photos WHERE master_id=?"),
                &[&master],
            )?
            .into_iter()
            .collect();
        Ok((1..)
            .map(|n| format!("Copy {n}"))
            .find(|name| !names.contains(name))
            .unwrap())
    }
    fn master_of(&self, id: PhotoId) -> Result<Option<PhotoId>> {
        self.db
            .read_optional(sql!("SELECT master_id FROM photos WHERE id=?"), &[&id])?
            .context("Unknown photo")
    }
    /// Lightroom's Set Copy as Master: the copy becomes the master, and the
    /// former master and the other copies become its copies.
    pub fn set_copy_as_master(&mut self, id: PhotoId) -> Result<()> {
        let master = self
            .master_of(id)?
            .context("This photo is already the master")?;
        let name: String = self
            .db
            .read_one(sql!("SELECT copy_name FROM photos WHERE id=?"), &[&id])?;
        let name = if name.is_empty() {
            self.unused_copy_name(master)?
        } else {
            name
        };
        self.db.write(|w| {
            w.execute(
                sql!("UPDATE photos SET master_id=?1 WHERE master_id=?2 AND id<>?1"),
                &[&id, &master],
            )?;
            w.execute(
                sql!("UPDATE photos SET master_id=?, copy_name=? WHERE id=?"),
                &[&id, &name, &master],
            )?;
            // Photo info is kept by master; the new one takes it over.
            w.execute(
                sql!(
                    "INSERT INTO photo_info
                     SELECT ?1, camera, lens, focal, aperture, exposure, iso, width, height
                     FROM photo_info WHERE photo=?2
                     ON CONFLICT(photo) DO UPDATE SET camera=excluded.camera, lens=excluded.lens,
                         focal=excluded.focal, aperture=excluded.aperture, exposure=excluded.exposure,
                         iso=excluded.iso, width=excluded.width, height=excluded.height"
                ),
                &[&id, &master],
            )?;
            // So is its file's checksum.
            w.execute(
                sql!("DELETE FROM photo_checksums WHERE photo=?"),
                &[&id],
            )?;
            w.execute(
                sql!("UPDATE photo_checksums SET photo=? WHERE photo=?"),
                &[&id, &master],
            )?;
            // And what it was when it entered the catalog.
            w.execute(
                sql!("DELETE FROM photo_import_facts WHERE photo=?"),
                &[&id],
            )?;
            w.execute(
                sql!("UPDATE photo_import_facts SET photo=? WHERE photo=?"),
                &[&id, &master],
            )?;
            // And what its camera numbered it.
            w.execute(
                sql!("DELETE FROM photo_sequences WHERE photo=?"),
                &[&id],
            )?;
            w.execute(
                sql!("UPDATE photo_sequences SET photo=? WHERE photo=?"),
                &[&id, &master],
            )?;
            w.execute(
                sql!("UPDATE photos SET master_id=NULL, copy_name='' WHERE id=?"),
                &[&id],
            )?;
            Ok(())
        })
    }
    pub fn set_copy_name(&mut self, id: PhotoId, name: &str) -> Result<()> {
        ensure!(
            self.master_of(id)?.is_some(),
            "Only virtual copies have a copy name"
        );
        self.db.write(|w| {
            w.execute(
                sql!("UPDATE photos SET copy_name=? WHERE id=?"),
                &[&name.trim(), &id],
            )
        })?;
        Ok(())
    }
    /// Removes a virtual copy, with its edit and metadata, from the catalog.
    /// The file and the other photos of it are untouched.
    pub fn remove_virtual_copy(&mut self, id: PhotoId) -> Result<()> {
        ensure!(
            self.master_of(id)?.is_some(),
            "Only virtual copies can be removed"
        );
        self.db.write(|w| delete_photo(w, id))
    }
}
/// Deletes photo `id` from the catalog with its edit, history, snapshots and
/// metadata, and its place in collections. Its file is untouched.
pub(super) fn delete_photo(w: &mut Write<'_>, id: PhotoId) -> Result<()> {
    super::edit_rows::delete_edits(w, id)?;
    for delete in [
        sql!("DELETE FROM lightroom_history WHERE photo=?"),
        sql!("DELETE FROM photo_keywords WHERE photo=?"),
        sql!("DELETE FROM collection_photos WHERE photo=?"),
        sql!("DELETE FROM photo_info WHERE photo=?"),
        sql!("DELETE FROM photo_checksums WHERE photo=?"),
        sql!("DELETE FROM photo_sequences WHERE photo=?"),
        sql!("DELETE FROM photo_import_facts WHERE photo=?"),
        sql!("DELETE FROM photo_imports WHERE photo=?"),
        sql!("DELETE FROM stack_photos WHERE photo=?"),
    ]
    .into_iter()
    .chain(super::descriptive::DELETE_ROWS)
    {
        w.execute(delete, &[&id])?;
    }
    w.execute(sql!("DELETE FROM photos WHERE id=?"), &[&id])?;
    Ok(())
}
