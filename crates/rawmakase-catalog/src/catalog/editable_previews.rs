//! The catalog's record of editable previews (`editable_previews`): which photos
//! have one, its file, and the original it was built from.
//!
//! An editable preview is a smaller copy of a photo that RAWmakase built while the
//! original was online, so the photo can be edited while the original is
//! offline. It is not a cache: once the original is offline, it can't be built
//! again, so the catalog keeps track of it for as long as its photo exists. A
//! row is removed only with its photo (Remove from Catalog, Move to Trash),
//! when the user discards it, or when a newer build replaces it. Each of those
//! deletes the file only after the transaction that stops naming it commits
//! ([`Write::delete_after_commit`]).
//!
//! Rows are keyed by master photo, so virtual copies share their master's,
//! and moving or renaming the photo changes nothing here. The files are in the
//! catalog's store folder ([`Catalog::editable_preview_folder`]). The catalog
//! only records their names and what their headers say. It never reads or
//! writes their contents: the format belongs to `rawmakase-decode`, which the
//! catalog can't reach.
//!
//! Editable preview files are RAWmakase's own derived files, not a user's photos,
//! so they are deleted outright, as the preview cache's are.
use super::db::{Reads, Write, sql};
use super::value::row;
use super::{Catalog, CatalogLocation, PhotoId};
use crate::storage::Identity;
use anyhow::{Context, Result, ensure};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The extension of an editable preview file.
const EXTENSION: &str = "rmep";

/// An editable preview just built, as its file's header describes it, to record
/// with [`Catalog::record_editable_preview`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltEditablePreview {
    /// Its name in the store folder ([`editable_preview_file_name`]).
    pub file: String,
    /// The version of the file's format.
    pub format_version: u32,
    /// The original it was built from, as read when the build began.
    pub original: Identity,
    /// The original's width and height, in pixels.
    pub original_size: (u32, u32),
    /// The editable preview's width and height, in pixels.
    pub size: (u32, u32),
}

/// A photo's editable preview, as the catalog records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditablePreview {
    /// The master photo it is of.
    pub photo: PhotoId,
    /// Its file, in the store folder.
    pub path: PathBuf,
    pub format_version: u32,
    /// The original it was built from.
    pub original: Identity,
    /// The original's width and height, in pixels.
    pub original_size: (u32, u32),
    /// Its width and height, in pixels.
    pub size: (u32, u32),
    /// The file's length when it was recorded.
    pub bytes: u64,
    /// When it was recorded, in UTC, as catalogs store times
    /// ([`rawmakase_model::time::utc_text`]).
    pub built_at: String,
}

impl EditablePreview {
    /// Whether the original, seen online as `seen`, is no longer the photo
    /// this was built from ([`Identity::same_photo`]). A stale editable preview
    /// is kept: the caller decides what to do with it.
    pub fn is_stale(&self, seen: &Identity) -> bool {
        !self.original.same_photo(seen)
    }
}

/// Whether a photo's editable preview can stand in for its offline original
/// ([`Catalog::offline_editable_preview`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OfflinePreview {
    /// It has none.
    None,
    /// Its file is not in the store folder.
    Missing(EditablePreview),
    /// Its saved edit was made for another version of the photo than the one
    /// the editable preview was built from.
    Stale(EditablePreview),
    Usable(EditablePreview),
}

/// The name of the file of an editable preview of `master` built from
/// `original`: `<photo id>-<identity key>.rmep`, unique per photo and version
/// of its original. The builder writes it under a temporary name, then
/// renames it to this without replacing anything.
pub fn editable_preview_file_name(master: PhotoId, original: &Identity) -> String {
    format!("{}-{}.{EXTENSION}", master.0, original.key())
}

/// The folder a catalog at `location` keeps its editable previews in: beside a
/// catalog file, named after its whole file name (`Photos.rawmakase` keeps
/// them in `Photos.rawmakase-editable-previews`), so two catalogs differing only
/// in extension never share one. It is beside the file's real path, through
/// any link, as the file operation lock is, so every path a catalog is opened
/// by finds the same folder. A catalog that isn't a file would keep them in
/// RAWmakase's data folder (`storage::data_dir`).
pub(super) fn folder(location: &CatalogLocation) -> PathBuf {
    match location {
        CatalogLocation::File(path) => {
            // A catalog not made yet (an export's destination) by its
            // folder's real path.
            let real = std::fs::canonicalize(path)
                .or_else(|_| {
                    std::fs::canonicalize(crate::storage::parent_dir(path))
                        .map(|dir| dir.join(path.file_name().unwrap_or_default()))
                })
                .unwrap_or_else(|_| path.clone());
            let mut name = real.file_name().unwrap_or_default().to_os_string();
            name.push("-editable-previews");
            real.with_file_name(name)
        }
    }
}

/// Whether `file` is an editable preview's name, as [`editable_preview_file_name`]
/// makes them: digits, lowercase hex letters and hyphens, then the
/// extension. The same on every system, so a name a catalog holds can't lead
/// out of the store folder wherever the catalog is opened (no separator,
/// drive or `..` of any system).
fn is_file_name(file: &str) -> bool {
    file.strip_suffix(EXTENSION)
        .and_then(|stem| stem.strip_suffix('.'))
        .is_some_and(|stem| {
            !stem.is_empty()
                && stem
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b) || b == b'-')
        })
}

/// Whether `source` names the file at `at`, the photo's path as the catalog
/// has it: the same path, or another spelling of it through links (a folder
/// is kept by its real path, and a build may have been given the path through
/// a link, as macOS's `/var` is `/private/var`). `real_source` is `source`'s
/// real path, read before the transaction, so a slow volume holds no lock
/// for it.
fn is_same_file(at: &Path, source: &Path, real_source: Option<&Path>) -> bool {
    at == source
        || real_source.is_some_and(|real| std::fs::canonicalize(at).ok().as_deref() == Some(real))
}

/// The path of editable preview `file` in `folder`. Refused for a name that
/// isn't an editable preview's, whatever a catalog holds.
pub(super) fn path_in(folder: &Path, file: &str) -> Result<PathBuf> {
    ensure!(
        is_file_name(file),
        "Not an editable preview's file name: {file}"
    );
    Ok(folder.join(file))
}

row! {
    struct Stored {
        photo: PhotoId,
        file: String,
        identity: String,
        original_width: i64,
        original_height: i64,
        width: i64,
        height: i64,
        bytes: i64,
        version: i64,
        built_at: String,
    }
}

row! {
    /// A row's photo and file name.
    struct Named {
        photo: PhotoId,
        file: String,
    }
}

impl Stored {
    fn read(self, folder: &Path) -> Result<EditablePreview> {
        Ok(EditablePreview {
            photo: self.photo,
            path: path_in(folder, &self.file)?,
            format_version: u32::try_from(self.version)?,
            original: serde_json::from_str(&self.identity)
                .context("An editable preview's original can't be read")?,
            original_size: (
                u32::try_from(self.original_width)?,
                u32::try_from(self.original_height)?,
            ),
            size: (u32::try_from(self.width)?, u32::try_from(self.height)?),
            bytes: u64::try_from(self.bytes)?,
            built_at: self.built_at,
        })
    }
}

/// The master of `photo`, if it is in the catalog.
fn master_of(db: &impl Reads, photo: PhotoId) -> Result<Option<PhotoId>> {
    db.read_optional(
        sql!("SELECT COALESCE(master_id, id) FROM photos WHERE id=?"),
        &[&photo],
    )
}

/// Removes the rows of `master`'s family, keyed by the master or by one of
/// its copies (a release before this one may have made a master a copy),
/// and deletes their files after the commit, except `keep`. Returns whether
/// there were any.
fn forget_family(w: &mut Write<'_>, master: PhotoId, keep: Option<&str>) -> Result<bool> {
    let rows = family_rows(w, master)?;
    let folder = folder(w.location());
    for row in &rows {
        w.execute(
            sql!("DELETE FROM editable_previews WHERE photo=?"),
            &[&row.photo],
        )?;
        if keep != Some(row.file.as_str()) {
            discard(w, &folder, &row.file);
        }
    }
    Ok(!rows.is_empty())
}

/// Deletes editable preview `file` of `folder` once `w` commits. A name that
/// isn't an editable preview's is left alone: the row goes, and nothing outside
/// the store folder is touched.
fn discard(w: &mut Write<'_>, folder: &Path, file: &str) {
    if let Ok(path) = path_in(folder, file) {
        w.delete_after_commit(path);
    }
}

/// The rows of `master`'s family, keyed by the master or one of its copies.
fn family_rows(db: &impl Reads, master: PhotoId) -> Result<Vec<Named>> {
    db.read(
        sql!(
            "SELECT s.photo, s.file FROM editable_previews s JOIN photos p ON p.id = s.photo
             WHERE p.id = ? OR p.master_id = ? ORDER BY s.photo"
        ),
        &[&master, &master],
    )
}

/// The editable preview files `master`'s family's rows name.
pub(super) fn family_files(db: &impl Reads, master: PhotoId) -> Result<Vec<String>> {
    Ok(family_rows(db, master)?
        .into_iter()
        .map(|r| r.file)
        .collect())
}

/// Removes `photo`'s own row, as the photo leaves the catalog, and deletes
/// its file after the commit. A virtual copy normally has none; one that has
/// (a release before this one made its master a copy) passes it to its
/// master, if that is still in the catalog without one, as it is still the
/// family's.
pub(super) fn forget_photo(w: &mut Write<'_>, photo: PhotoId) -> Result<()> {
    let file: Option<String> = w.read_optional(
        sql!("SELECT file FROM editable_previews WHERE photo=?"),
        &[&photo],
    )?;
    if let Some(file) = file {
        let heir: Option<PhotoId> = w.read_optional(
            sql!(
                "SELECT m.id FROM photos c JOIN photos m ON m.id = c.master_id
                 WHERE c.id = ?
                 AND NOT EXISTS (SELECT 1 FROM editable_previews s WHERE s.photo = m.id)"
            ),
            &[&photo],
        )?;
        if let Some(master) = heir {
            w.execute(
                sql!("UPDATE editable_previews SET photo=? WHERE photo=?"),
                &[&master, &photo],
            )?;
            return Ok(());
        }
        w.execute(
            sql!("DELETE FROM editable_previews WHERE photo=?"),
            &[&photo],
        )?;
        let folder = folder(w.location());
        discard(w, &folder, &file);
    }
    Ok(())
}

/// Gives the editable preview of `master` to `now`, its copy that becomes the
/// master (Set Copy as Master). The file keeps its name.
pub(super) fn pass_to(w: &mut Write<'_>, master: PhotoId, now: PhotoId) -> Result<()> {
    // A row of the copy's own, which nothing records, would be its family's
    // second; the master's is the one kept.
    let had: Option<i64> = w.read_optional(
        sql!("SELECT 1 FROM editable_previews WHERE photo=?"),
        &[&master],
    )?;
    if had.is_some() {
        forget_photo(w, now)?;
    }
    w.execute(
        sql!("UPDATE editable_previews SET photo=? WHERE photo=?"),
        &[&now, &master],
    )?;
    Ok(())
}

/// The file names of every editable preview recorded in `db`.
pub(super) fn files(db: &impl Reads) -> Result<Vec<(PhotoId, String)>> {
    let rows: Vec<Named> = db.read(
        sql!("SELECT photo, file FROM editable_previews ORDER BY photo"),
        &[],
    )?;
    Ok(rows.into_iter().map(|r| (r.photo, r.file)).collect())
}

impl Catalog {
    /// The folder this catalog keeps its editable preview files in.
    pub fn editable_preview_folder(&self) -> PathBuf {
        folder(&self.location)
    }

    /// Records the editable preview of `photo` (its master's, for a virtual
    /// copy) built from its original at `source`, in
    /// [`file`](BuiltEditablePreview::file) of the store folder. In one
    /// transaction, it replaces the row of the photo's previous one, whose
    /// file is deleted only after that commits (unless it is the same file,
    /// built again from the same original).
    ///
    /// Refused, recording nothing, when the name isn't an editable preview's, the
    /// file isn't there, or the photo is no longer in the catalog with its
    /// file at `source` on this computer (that path, or one leading to the
    /// same file through links): it left, was moved or renamed, or
    /// its id was given to another photo while it was built. The new file
    /// then stays where it is, for the caller to delete (unless another row
    /// names it, which refuses it too) or record again.
    pub fn record_editable_preview(
        &mut self,
        photo: PhotoId,
        source: &Path,
        built: &BuiltEditablePreview,
    ) -> Result<EditablePreview> {
        let folder = self.editable_preview_folder();
        let path = path_in(&folder, &built.file)?;
        let computer = self.computer.id.clone();
        let real_source = std::fs::canonicalize(source).ok();
        let metadata = std::fs::metadata(&path)
            .with_context(|| format!("The editable preview {} is not there", path.display()))?;
        ensure!(metadata.is_file(), "Not a file: {}", path.display());
        let identity = serde_json::to_string(&built.original)?;
        let bytes = i64::try_from(metadata.len())?;
        let built_at = rawmakase_model::time::now_text();
        let master = self.db.write(|w| {
            let master = master_of(w, photo)?.context("The photo is no longer in the catalog")?;
            row! {
                struct Place {
                    folder: super::FolderId,
                    filename: String,
                }
            }
            let place: Place = w.read_one(
                sql!("SELECT folder, filename FROM photos WHERE id=?"),
                &[&master],
            )?;
            let here = super::locations::folder_path(w, &computer, place.folder)?;
            let at = here.map(|folder| folder.join(&place.filename));
            ensure!(
                at.is_some_and(|at| is_same_file(&at, source, real_source.as_deref())),
                "The photo changed while its editable preview was built"
            );
            forget_family(w, master, Some(&built.file))?;
            w.execute(
                sql!(
                    "INSERT INTO editable_previews
                     (photo, file, identity, original_width, original_height, width, height,
                      bytes, version, built_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
                ),
                &[
                    &master,
                    &built.file,
                    &identity,
                    &i64::from(built.original_size.0),
                    &i64::from(built.original_size.1),
                    &i64::from(built.size.0),
                    &i64::from(built.size.1),
                    &bytes,
                    &i64::from(built.format_version),
                    &built_at,
                ],
            )?;
            Ok(master)
        })?;
        Ok(EditablePreview {
            photo: master,
            path,
            format_version: built.format_version,
            original: built.original.clone(),
            original_size: built.original_size,
            size: built.size,
            bytes: metadata.len(),
            built_at,
        })
    }

    /// The editable preview of `photo`, its master's for a virtual copy. Its
    /// file may be missing (deleted outside RAWmakase, or the catalog moved
    /// without its store folder): the caller checks when it opens it.
    pub fn editable_preview(&self, photo: PhotoId) -> Result<Option<EditablePreview>> {
        let stored: Option<Stored> = self.db.read_optional(
            sql!(
                "SELECT s.photo, s.file, s.identity, s.original_width, s.original_height,
                    s.width, s.height, s.bytes, s.version, s.built_at
                 FROM editable_previews s JOIN photos p ON p.id = s.photo
                 WHERE COALESCE(p.master_id, p.id) =
                     (SELECT COALESCE(q.master_id, q.id) FROM photos q WHERE q.id = ?)
                 ORDER BY CASE WHEN p.master_id IS NULL THEN 0 ELSE 1 END, s.built_at DESC
                 LIMIT 1"
            ),
            &[&photo],
        )?;
        stored
            .map(|s| s.read(&self.editable_preview_folder()))
            .transpose()
    }

    /// Whether photo `id`'s editable preview (its master's, for a virtual copy)
    /// can stand in for its original while that is offline, by what the
    /// catalog knows: a recorded file that is there, of the version of the photo
    /// its saved edit, if any, was made for. Its contents are checked by whoever
    /// opens it.
    pub fn offline_editable_preview(&self, id: PhotoId) -> Result<OfflinePreview> {
        let Some(preview) = self.editable_preview(id)? else {
            return Ok(OfflinePreview::None);
        };
        if !preview.path.is_file() {
            return Ok(OfflinePreview::Missing(preview));
        }
        // An edit saved since for another version of the photo (the original
        // replaced while online, and this preview not rebuilt) says the preview
        // is no longer of the photo being edited.
        let record = self.edit_record(id)?;
        if record.recipe.is_some()
            && let Some(saved) = record.identity.as_deref()
        {
            let saved: Identity = serde_json::from_str(saved)?;
            if preview.is_stale(&saved) {
                return Ok(OfflinePreview::Stale(preview));
            }
        }
        Ok(OfflinePreview::Usable(preview))
    }

    /// Every photo with an editable preview: the masters with one, and their
    /// virtual copies.
    pub fn photos_with_editable_previews(&self) -> Result<BTreeSet<PhotoId>> {
        let photos: Vec<PhotoId> = self.db.read(
            sql!(
                "SELECT p.id FROM photos p
                 WHERE COALESCE(p.master_id, p.id) IN
                     (SELECT COALESCE(q.master_id, q.id)
                      FROM editable_previews s JOIN photos q ON q.id = s.photo)"
            ),
            &[],
        )?;
        Ok(photos.into_iter().collect())
    }

    /// Discards the editable previews of `photos` (their masters', for virtual
    /// copies), in one transaction; their files are deleted after it
    /// commits. Returns the masters whose editable previews were discarded.
    pub fn discard_editable_previews(&mut self, photos: &[PhotoId]) -> Result<Vec<PhotoId>> {
        self.db.write(|w| {
            let mut masters = BTreeSet::new();
            for &photo in photos {
                if let Some(master) = master_of(w, photo)? {
                    masters.insert(master);
                }
            }
            let mut discarded = Vec::new();
            for master in masters {
                if forget_family(w, master, None)? {
                    discarded.push(master);
                }
            }
            Ok(discarded)
        })
    }

    /// Deletes editable preview `file` from the store folder when no row names
    /// it: a build [`record_editable_preview`](Self::record_editable_preview)
    /// refused, or one its builder gave up on after writing it. The file goes
    /// after the transaction commits. The transaction only reads, so it holds
    /// against records made on this connection, which in RAWmakase is the one
    /// worker that records editable previews, but not against another
    /// connection's: two RAWmakase instances building the same photo at once
    /// can leave a row without its file, which callers treat as missing and
    /// build again. Returns whether there was a file to delete. Refused for a
    /// name that isn't an editable preview's.
    pub fn discard_unrecorded_editable_preview(&mut self, file: &str) -> Result<bool> {
        let path = path_in(&self.editable_preview_folder(), file)?;
        self.db.write(|w| {
            let named: Option<PhotoId> = w.read_optional(
                sql!("SELECT photo FROM editable_previews WHERE file=?"),
                &[&file],
            )?;
            let there = named.is_none() && path.is_file();
            if there {
                w.delete_after_commit(path.clone());
            }
            Ok(there)
        })
    }

    /// The bytes every editable preview's file took when it was recorded.
    pub fn editable_previews_size(&self) -> Result<u64> {
        row! {
            struct Total {
                bytes: i64,
            }
        }
        let total: Total = self.db.read_one(
            // Postgres sums BIGINT as NUMERIC.
            sql!("SELECT CAST(COALESCE(sum(bytes), 0) AS BIGINT) FROM editable_previews"),
            &[],
        )?;
        Ok(u64::try_from(total.bytes)?)
    }

    /// The editable preview files in the store folder that no row names: left by
    /// a deletion that failed, a release before editable previews that removed
    /// their photos, or a build whose photo left the catalog. Only RAWmakase's
    /// own files are listed, so their owner may delete them; a file a build
    /// has written but not yet recorded is listed too, so the caller asks only
    /// while no build is running.
    pub fn editable_preview_orphans(&self) -> Result<Vec<PathBuf>> {
        let folder = self.editable_preview_folder();
        let entries = match std::fs::read_dir(&folder) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let named: BTreeSet<String> = files(&self.db)?.into_iter().map(|(_, f)| f).collect();
        let mut orphans = Vec::new();
        for entry in entries {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if is_file_name(&name) && !named.contains(&name) && entry.file_type()?.is_file() {
                orphans.push(entry.path());
            }
        }
        orphans.sort();
        Ok(orphans)
    }
}
