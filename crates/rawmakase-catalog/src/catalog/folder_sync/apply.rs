//! Applying what the user accepted from a [`SyncPlan`], in two phases each
//! consistent on its own: one transaction moves, renames and removes
//! photos and removes folders left empty; then the new files are added
//! through Add Folder's own steps. Every action is checked again first, on
//! disk and in the catalog, and one that no longer holds is skipped and
//! reported, never forced.
use super::digest;
use super::{FileState, Move, NotChecked, PlanKind, SyncPlan, Target, current_master, state_of};
use crate::catalog::copies::delete_photo;
use crate::catalog::db::{Reads, sql};
use crate::catalog::edit_rows::{Refresh, refresh_identity};
use crate::catalog::ingest::{Matched, Matcher, Place, folder_id, known};
use crate::catalog::locations::folder_path;
use crate::catalog::{Catalog, FolderId, NewPhotoDefaults, PhotoId, RootId, SidecarReport};
use crate::storage::Identity;
use anyhow::{Result, ensure};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// What the user accepted from a plan.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Accepted {
    /// Photos to update, each with the file it is now: a move of the plan,
    /// or an option of one of its choices.
    pub moves: Vec<(PhotoId, PathBuf)>,
    /// Files to add.
    pub additions: Vec<PathBuf>,
    /// Missing photos to remove from the catalog, with their copies.
    pub removals: Vec<PhotoId>,
}

impl SyncPlan {
    /// What the review starts with ticked, as Lightroom's: verified moves
    /// and new files; neither probable moves, copies of photos already in
    /// the catalog nor removals. Find on Disk's file picked is ticked too,
    /// verified or not: the user chose it.
    pub fn default_accepted(&self) -> Accepted {
        let picked = self.picked();
        Accepted {
            moves: self
                .moves
                .iter()
                .filter(|m| m.verified || picked == Some(*m))
                .map(|m| (m.photo, m.to.clone()))
                .collect(),
            additions: self
                .additions
                .iter()
                .filter(|a| a.copy_of.is_empty())
                .map(|a| a.file.clone())
                .collect(),
            removals: Vec::new(),
        }
    }
}

/// What applying did.
#[derive(Debug, Default)]
pub struct Applied {
    /// Photos updated to their new file, each with its copies.
    pub moved: Vec<PhotoId>,
    /// Of those, photos whose edit was saved for another version of the
    /// file than the one found, and stays protected.
    pub protected: Vec<PhotoId>,
    /// Photos removed, copies included; their ids may be given to new photos.
    pub removed: Vec<PhotoId>,
    /// Catalog folders removed, left empty.
    pub folders_removed: Vec<FolderId>,
    pub added: usize,
    /// What reading the added photos' sidecars found.
    pub report: SidecarReport,
    /// Actions skipped because something changed since the scan.
    pub skipped: Vec<NotChecked>,
    /// Adding the new files failed; the moves and removals stand.
    pub add_error: Option<anyhow::Error>,
}

impl Catalog {
    /// Applies `accepted` from `plan`. `online` observes volumes as the scan
    /// did; `open` is the photo open in Develop, which can be moved, only by
    /// a verified match, but not removed. Nothing on disk is changed.
    pub fn apply_folder_sync(
        &mut self,
        plan: &SyncPlan,
        accepted: &Accepted,
        online: &dyn Fn(&Path) -> bool,
        open: Option<PhotoId>,
        defaults: &NewPhotoDefaults,
    ) -> Result<Applied> {
        let moves = chosen_moves(plan, accepted)?;
        let finding = matches!(plan.kind, PlanKind::FindOnDisk { .. });
        ensure!(
            !finding || (accepted.additions.is_empty() && accepted.removals.is_empty()),
            "Find on Disk only relinks photos"
        );
        let mut applied = Applied::default();
        let skip = |applied: &mut Applied, path: &Path, reason: &str| {
            applied.skipped.push(NotChecked {
                path: path.to_path_buf(),
                reason: reason.into(),
            })
        };
        // On disk, just before the transaction: the file found is as it
        // was scanned, and the old file still missing by the scan's rules.
        let folders = self.db.snapshot(|s| self.locate_folders(s))?;
        let anchor_of = |id: PhotoId| {
            plan.scanned
                .get(&id)
                .and_then(|(m, _)| folders.get(&m.folder))
                .and_then(|f| f.anchor.clone())
        };
        let still_missing = |id: PhotoId, path: &Path| {
            state_of(Some(path), anchor_of(id).as_deref(), online) == FileState::Missing
        };
        // A move from an offline location, verified: its old file must still
        // not be there, offline or missing.
        let still_absent = |m: &Move| {
            if !m.offline {
                return still_missing(m.photo, &m.from);
            }
            matches!(
                state_of(Some(&m.from), anchor_of(m.photo).as_deref(), online),
                FileState::Missing | FileState::Unavailable
            )
        };
        let matcher = crate::catalog::ingest::Matcher::new(self)?;
        let mut ready: Vec<&Move> = Vec::new();
        for m in moves {
            if Identity::read(&m.to).ok().as_ref() != Some(&m.identity) {
                skip(&mut applied, &m.to, "changed since the scan");
            } else if !still_absent(m) {
                skip(&mut applied, &m.from, "no longer missing");
            } else if !placed_as(&matcher, m) {
                skip(
                    &mut applied,
                    &m.to,
                    "its folder is elsewhere in the catalog now",
                );
            } else {
                ready.push(m);
            }
        }
        let mut removals: Vec<(PhotoId, PathBuf)> = Vec::new();
        for id in &accepted.removals {
            let Some(missing) = plan.missing.iter().find(|m| m.photo == *id) else {
                anyhow::bail!("Not a missing photo of this synchronization");
            };
            if open == Some(*id) {
                skip(&mut applied, &missing.path, "open in Develop");
            } else if !still_missing(*id, &missing.path) {
                skip(&mut applied, &missing.path, "no longer missing");
            } else {
                removals.push((*id, missing.path.clone()));
            }
        }
        // Still gone, by the same evidence as a missing photo: absent from
        // a location that lists and is online.
        let gone_folders: Vec<FolderId> = plan
            .gone_folders
            .iter()
            .filter(|id| {
                folders.get(id).is_some_and(|f| {
                    let absent = f.path.as_ref().is_some_and(|p| {
                        matches!(std::fs::symlink_metadata(p),
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound)
                    });
                    let anchor = f.anchor.as_deref();
                    absent
                        && anchor.is_some_and(|a| {
                            Some(a) != f.path.as_deref() && super::list_all(a).is_ok() && online(a)
                        })
                })
            })
            .copied()
            .collect();
        // The new files as scanned, rechecked before anything is written.
        let offered: HashMap<&PathBuf, &Identity> = plan
            .additions
            .iter()
            .map(|a| (&a.file, &a.identity))
            .collect();
        let mut files: Vec<PathBuf> = Vec::new();
        for file in &accepted.additions {
            let scanned = offered
                .get(file)
                .ok_or_else(|| anyhow::anyhow!("Not a new file of this synchronization"))?;
            match Identity::read(file) {
                Ok(now) if now == **scanned => files.push(file.clone()),
                Ok(_) => skip(&mut applied, file, "changed since the scan"),
                Err(_) => skip(&mut applied, file, "no longer there"),
            }
        }
        let computer = self.computer.id.clone();
        let (moved, protected, removed, folders_removed, skipped) = self.db.write(|w| {
            // Where each destination goes, by this transaction's locations.
            let placing = crate::catalog::ingest::Matcher::read(w, &computer)?;
            let mut moved = Vec::new();
            let mut protected = Vec::new();
            let mut removed = Vec::new();
            let mut skipped: Vec<NotChecked> = Vec::new();
            let mut skip = |path: &Path, reason: &str| {
                skipped.push(NotChecked {
                    path: path.to_path_buf(),
                    reason: reason.into(),
                })
            };
            // The photo open in Develop, as its master: it follows only a
            // verified match.
            let open_master: Option<PhotoId> = match open {
                Some(id) => w.read_optional(
                    sql!("SELECT COALESCE(master_id, id) FROM photos WHERE id=?"),
                    &[&id],
                )?,
                None => None,
            };
            // New roots made by this transaction, by directory.
            let mut new_roots: HashMap<PathBuf, RootId> = HashMap::new();
            for m in ready {
                if let Some(reason) = unchanged(w, plan, &computer, m.photo, &m.from)? {
                    skip(&m.from, reason);
                    continue;
                }
                if open_master == Some(m.photo) && !m.verified {
                    skip(&m.to, "open in Develop, and not verified");
                    continue;
                }
                let folder = match &m.target {
                    Target::Folder(root, logical) => {
                        if !placed_as(&placing, m) {
                            skip(&m.to, "its folder was located elsewhere since the scan");
                            continue;
                        }
                        if known(w, *root, logical, &m.to)? {
                            skip(&m.to, "a photo of the catalog is there now");
                            continue;
                        }
                        folder_id(w, *root, logical)?
                    }
                    Target::NewRoot(directory) => {
                        let root = match new_roots.get(directory) {
                            Some(root) => *root,
                            None => {
                                if !placed_as(&placing, m) {
                                    skip(
                                        &m.to,
                                        "its folder was added to the catalog since the scan",
                                    );
                                    continue;
                                }
                                // As Add Folder adds a root: where it was added.
                                let root: RootId = w.insert_returning_id(
                                    sql!("INSERT INTO roots(original_path) VALUES(?) RETURNING id"),
                                    &[&directory.to_string_lossy()],
                                )?;
                                *new_roots.entry(directory.clone()).or_insert(root)
                            }
                        };
                        folder_id(w, root, "")?
                    }
                };
                let filename = m.to.file_name().unwrap_or_default().to_string_lossy();
                let family: Vec<PhotoId> = w.read(
                    sql!("SELECT id FROM photos WHERE id=? OR master_id=?"),
                    &[&m.photo, &m.photo],
                )?;
                crate::catalog::stacks::leave_folder(w, m.photo, folder)?;
                w.execute(
                    sql!("UPDATE photos SET folder=?, filename=? WHERE id=? OR master_id=?"),
                    &[&folder, &filename, &m.photo, &m.photo],
                )?;
                if m.verified {
                    // The version the checksum was read from is the one found.
                    let was: Option<String> = w.read_optional(
                        sql!("SELECT identity FROM photo_checksums WHERE photo=?"),
                        &[&m.photo],
                    )?;
                    let was: Option<Identity> =
                        was.and_then(|text| serde_json::from_str(&text).ok());
                    for member in &family {
                        let refreshed = match &was {
                            Some(was) => refresh_identity(w, *member, was, &m.identity)?,
                            None => Refresh::Protected,
                        };
                        if refreshed == Refresh::Protected {
                            protected.push(*member);
                        }
                    }
                    w.execute(
                        sql!("UPDATE photo_checksums SET identity=? WHERE photo=?"),
                        &[&serde_json::to_string(&m.identity)?, &m.photo],
                    )?;
                } else {
                    // Its edit opens only if the file passes the check every
                    // photo's does; its checksum and sequence are read again.
                    w.execute(
                        sql!("DELETE FROM photo_checksums WHERE photo=?"),
                        &[&m.photo],
                    )?;
                    w.execute(
                        sql!("DELETE FROM photo_sequences WHERE photo=?"),
                        &[&m.photo],
                    )?;
                }
                moved.push(m.photo);
            }
            for (id, path) in &removals {
                if let Some(reason) = unchanged(w, plan, &computer, *id, path)? {
                    skip(path, reason);
                    continue;
                }
                let copies: Vec<PhotoId> =
                    w.read(sql!("SELECT id FROM photos WHERE master_id=?"), &[id])?;
                // The photo open in Develop, a copy made since included, stays.
                if open.is_some_and(|open| open == *id || copies.contains(&open)) {
                    skip(path, "open in Develop");
                    continue;
                }
                for photo in copies.iter().chain([id]) {
                    delete_photo(w, *photo)?;
                    removed.push(*photo);
                }
            }
            // A synchronization adding nothing leaves Previous Import empty;
            // Find on Disk adds nothing by design, and leaves it as it is.
            if files.is_empty() && !finding {
                crate::catalog::ingest::new_batch(w, crate::catalog::ingest::ImportKind::Sync)?;
            }
            let mut folders_removed = Vec::new();
            for id in &gone_folders {
                if remove_if_empty(w, *id)? {
                    folders_removed.push(*id);
                }
            }
            Ok((moved, protected, removed, folders_removed, skipped))
        })?;
        applied.moved = moved;
        applied.protected = protected;
        applied.removed = removed;
        applied.folders_removed = folders_removed;
        applied.skipped.extend(skipped);
        // Then the new files, as Add Folder adds them.
        if !files.is_empty() {
            match self.add_files(&files, defaults) {
                Ok((added, report, not_placed)) => {
                    applied.added = added;
                    applied.report = report;
                    applied.skipped.extend(not_placed);
                }
                Err(e) => applied.add_error = Some(e),
            }
        }
        Ok(applied)
    }
    /// Adds `files`, whose directories must be in catalog folders, with
    /// what Add Folder gives each. Returns how many were added, what reading
    /// their sidecars found, and the directories that could not be placed.
    fn add_files(
        &mut self,
        files: &[PathBuf],
        defaults: &NewPhotoDefaults,
    ) -> Result<(usize, SidecarReport, Vec<NotChecked>)> {
        let mut placed = self.place(files, Path::new(""), &[])?;
        let mut not_placed: Vec<NotChecked> = placed
            .conflicts
            .iter()
            .map(|c| NotChecked {
                path: c.directory.clone(),
                reason: c.message(),
            })
            .collect();
        not_placed.extend(placed.ambiguous.iter().flat_map(|a| {
            a.directories.iter().map(|d| NotChecked {
                path: d.clone(),
                reason: "more than one catalog folder could be it".into(),
            })
        }));
        // A directory no catalog folder contains is not synchronization's
        // to add as a new root.
        placed.places.retain(|directory, place| {
            let keep = matches!(place, Place::Folder(..));
            if !keep {
                not_placed.push(NotChecked {
                    path: directory.clone(),
                    reason: "not in a catalog folder".into(),
                });
            }
            keep
        });
        let (added, report) = self
            .insert(
                Path::new(""),
                files.to_vec(),
                &placed.places,
                defaults,
                crate::catalog::ingest::ImportKind::Sync,
                || true,
            )?
            .unwrap_or_default();
        Ok((added.len(), report, not_placed))
    }
}

/// Whether `m`'s destination is still placed as scanned by `matcher`: in
/// the same catalog folder, or for a new root still in none, with no root
/// or folder location at or below it.
fn placed_as(matcher: &Matcher, m: &Move) -> bool {
    let Some(directory) = m.to.parent() else {
        return false;
    };
    match (&m.target, matcher.place(directory, directory, &[])) {
        (Target::Folder(root, logical), Matched::Place(Place::Folder(r, l))) => {
            r == *root && l == *logical
        }
        (Target::NewRoot(at), Matched::Place(Place::New(_))) => {
            at == directory && !matcher.holds_below(directory)
        }
        _ => false,
    }
}

/// The plan's moves the user accepted, each photo and each file once.
fn chosen_moves<'a>(plan: &'a SyncPlan, accepted: &Accepted) -> Result<Vec<&'a Move>> {
    let offered: HashMap<(PhotoId, &PathBuf), &Move> = plan
        .moves
        .iter()
        .chain(plan.choices.iter().flat_map(|c| c.options.iter()))
        .map(|m| ((m.photo, &m.to), m))
        .collect();
    let mut photos = HashSet::new();
    let mut files = HashSet::new();
    accepted
        .moves
        .iter()
        .map(|(photo, to)| {
            let m = offered
                .get(&(*photo, to))
                .copied()
                .ok_or_else(|| anyhow::anyhow!("Not a move of this synchronization"))?;
            ensure!(
                photos.insert(*photo) && files.insert(to),
                "A photo or file was chosen twice"
            );
            Ok(m)
        })
        .collect()
}

/// Why photo `id` can't be acted on as scanned, if it can't: it must still
/// be the master scanned, its family exactly as sealed, and its file where
/// it was scanned (`path`), through this computer's locations now.
fn unchanged(
    w: &impl Reads,
    plan: &SyncPlan,
    computer: &str,
    id: PhotoId,
    path: &Path,
) -> Result<Option<&'static str>> {
    let Some((master, sealed)) = plan.scanned.get(&id) else {
        return Ok(Some("changed in the catalog during the scan"));
    };
    if current_master(w, id)?.as_ref() != Some(master) {
        return Ok(Some("changed in the catalog since the scan"));
    }
    if digest::family(w, id)?.as_ref() != Some(sealed) {
        return Ok(Some("changed in the catalog since the scan"));
    }
    let here = folder_path(w, computer, master.folder)?.map(|f| f.join(&master.filename));
    if here.as_deref() != Some(path) {
        return Ok(Some("its folder was located elsewhere since the scan"));
    }
    Ok(None)
}

/// Removes catalog folder `id` when it has no photos, no folders below it
/// and no location on any computer. Returns whether it did.
fn remove_if_empty(w: &mut crate::catalog::db::Write<'_>, id: FolderId) -> Result<bool> {
    let photo: Option<PhotoId> =
        w.read_optional(sql!("SELECT id FROM photos WHERE folder=?"), &[&id])?;
    if photo.is_some() {
        return Ok(false);
    }
    let Some(logical): Option<String> =
        w.read_optional(sql!("SELECT path FROM folder_paths WHERE folder=?"), &[&id])?
    else {
        return Ok(false);
    };
    let root: crate::catalog::RootId =
        w.read_one(sql!("SELECT root FROM folders WHERE id=?"), &[&id])?;
    let siblings: Vec<String> = w.read(
        sql!(
            "SELECT p.path FROM folders f JOIN folder_paths p ON p.folder=f.id
             WHERE f.root=? AND f.id<>?"
        ),
        &[&root, &id],
    )?;
    // The root folder, or one with folders below it, stays.
    if logical.is_empty() || siblings.iter().any(|s| super::below(s, &logical)) {
        return Ok(false);
    }
    let located: Option<i64> = w.read_optional(
        sql!("SELECT 1 FROM folder_locations WHERE root=? AND relative_path=?"),
        &[&root, &logical],
    )?;
    if located.is_some() {
        return Ok(false);
    }
    w.execute(sql!("DELETE FROM folder_mappings WHERE folder=?"), &[&id])?;
    w.execute(sql!("DELETE FROM folder_paths WHERE folder=?"), &[&id])?;
    w.execute(sql!("DELETE FROM folders WHERE id=?"), &[&id])?;
    Ok(true)
}
