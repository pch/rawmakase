//! Find on Disk (issue #693): the user picks the file of an offline photo,
//! and the other offline photos of its catalog folder are looked for beside
//! it. The result is a [`SyncPlan`] of moves and choices only, applied by
//! [`Catalog::apply_folder_sync`] with every check a synchronization gets.
//!
//! The picked file is that photo unless the checksum on record says
//! otherwise; the others are matched as Synchronize matches, and by name
//! where a photo has no checksum. A photo whose old location is only
//! offline, not observed missing, moves only on a verified match.
use super::scan::{Listing, list, merge_choices};
use super::{FileState, Move, PlanKind, SyncInputs, SyncPlan, SyncScope, Target, folded, state_of};
use crate::catalog::db::{Reads, sql};
use crate::catalog::ingest::{Matched, Place};
use crate::catalog::{Catalog, PhotoId};
use crate::storage::{ChecksumRead, FileChecksum, Identity};
use anyhow::{Result, bail};
use rawmakase_cancel::Cancel;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// What Find on Disk works from: the catalog in one snapshot, as a
/// synchronization's, and the file picked for a photo.
pub struct FindInputs {
    sync: SyncInputs,
    /// The photo's master.
    photo: PhotoId,
    file: PathBuf,
}

impl Catalog {
    /// What finding `photo` (a master or a virtual copy, which stands for
    /// its master) at `file` needs, read in one snapshot.
    pub fn find_inputs(&self, photo: PhotoId, file: &Path) -> Result<FindInputs> {
        let master: Option<(PhotoId, crate::catalog::FolderId)> = self.db.snapshot(|r| {
            let id: Option<PhotoId> = r.read_optional(
                sql!("SELECT COALESCE(master_id, id) FROM photos WHERE id=?"),
                &[&photo],
            )?;
            let Some(id) = id else { return Ok(None) };
            let folder = r.read_optional(sql!("SELECT folder FROM photos WHERE id=?"), &[&id])?;
            Ok(folder.map(|folder| (id, folder)))
        })?;
        let Some((id, folder)) = master else {
            bail!("The photo is no longer in the catalog");
        };
        // Its directory as the disk has it, so it is placed as Add Folder
        // places it; the file's own name is kept.
        let name = file
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("{} is not a file", file.display()))?;
        let directory = file
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .canonicalize()?;
        Ok(FindInputs {
            sync: self.sync_inputs(SyncScope::Folder(folder))?,
            photo: id,
            file: directory.join(name),
        })
    }
}

/// How a file matched a photo.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Evidence {
    /// The full checksum on record.
    Verified,
    /// The quick hash, or the identity its edit was saved for, with no full
    /// checksum on record.
    Probable,
    /// The name, with no checksum on record.
    Name,
}

/// Finds `inputs`' photo at the file picked, and the other offline photos
/// of its folder in the same directory. Refuses, with why, a file that
/// isn't that photo by the checksum on record, an unverified file for a
/// photo whose old location is only offline, and a directory that can't
/// go in the catalog. `online`, `cancel` and `progress` are as [`super::scan`](fn@super::scan)'s.
pub fn find_on_disk(
    inputs: &FindInputs,
    online: &dyn Fn(&Path) -> bool,
    cancel: Cancel<'_>,
    progress: &dyn Fn(usize),
) -> Result<SyncPlan> {
    let s = &inputs.sync;
    let go_on = |_: usize| !cancel.is_cancelled();
    let stopped = || anyhow::anyhow!("Find on Disk stopped");
    let Some(master) = s.masters.get(&inputs.photo) else {
        bail!("The photo is no longer in the catalog");
    };
    let name = &master.filename;
    let folder = &s.folders[&master.folder];
    let Some(from) = s.path_of(inputs.photo) else {
        bail!("{name}'s folder can't be on this computer; locate its root folder first");
    };
    // Where each photo of the folder stands. They share a directory, so the
    // evidence for one missing is the evidence for all.
    let mut evidence: Option<FileState> = None;
    let mut state = |path: &Path| -> FileState {
        if std::fs::symlink_metadata(path).is_ok() {
            return state_of(Some(path), folder.anchor.as_deref(), online);
        }
        if let Some(known) = &evidence {
            return known.clone();
        }
        let found = state_of(Some(path), folder.anchor.as_deref(), online);
        if found != FileState::Present {
            evidence = Some(found.clone());
        }
        found
    };
    let offline = match state(&from) {
        FileState::Missing => false,
        FileState::Unavailable => true,
        FileState::Present => bail!("{name} is not missing: its file is where the catalog has it"),
        FileState::Error(e) => bail!("{name} could not be checked: {e}"),
    };
    // The file picked.
    let file = &inputs.file;
    let known: HashSet<PathBuf> = s.masters.keys().filter_map(|id| s.path_of(*id)).collect();
    if known.contains(file) {
        bail!("{} is already in the catalog", file.display());
    }
    match std::fs::symlink_metadata(file) {
        Ok(m) if m.is_file() && crate::storage::is_photo(file) => {}
        Ok(_) => bail!("{} is not a photo file", file.display()),
        Err(e) => bail!("{}: {e}", file.display()),
    }
    let different = || anyhow::anyhow!("This file is a different photo than {name}");
    let quick = match FileChecksum::read_quick(file, go_on)? {
        ChecksumRead::Done(quick) => quick,
        ChecksumRead::Changed => bail!("{} changed while it was read", file.display()),
        ChecksumRead::Cancelled => return Err(stopped()),
    };
    let mut identity = quick.identity.clone();
    let verified = match s.checksums.get(&inputs.photo) {
        Some(c) if c.size != quick.size || c.quick != quick.quick => return Err(different()),
        Some(FileChecksum {
            full: Some(theirs), ..
        }) => match FileChecksum::read_full(file, go_on)? {
            ChecksumRead::Done(read) if read.full.as_ref() == Some(theirs) => {
                identity = read.identity;
                true
            }
            ChecksumRead::Done(_) => return Err(different()),
            ChecksumRead::Changed => bail!("{} changed while it was read", file.display()),
            ChecksumRead::Cancelled => return Err(stopped()),
        },
        _ => false,
    };
    if offline && !verified {
        bail!(
            "{name}'s drive isn't connected, and this file can't be verified as that photo. \
             Connect the drive, or use Synchronize Folder once it is"
        );
    }
    // Where the directory goes in the catalog.
    let Some(directory) = file.parent().map(Path::to_path_buf) else {
        bail!("{} is not in a folder", file.display());
    };
    let target = match s.matcher.place(&directory, &directory, &[]) {
        Matched::Place(Place::Folder(root, logical)) => Target::Folder(root, logical),
        Matched::Place(Place::New(_)) if s.matcher.holds_below(&directory) => bail!(
            "{} holds folders of the catalog, so it can't be added as a folder of its own",
            directory.display()
        ),
        Matched::Place(Place::New(_)) => Target::NewRoot(directory.clone()),
        Matched::Conflict(conflict) => bail!("{}", conflict.message()),
        Matched::Ambiguous(_) => bail!(
            "{}: more than one catalog folder could be it",
            directory.display()
        ),
    };
    let picked = Move {
        photo: inputs.photo,
        from,
        to: file.clone(),
        identity,
        verified,
        by_name: false,
        offline,
        target: target.clone(),
    };
    // The other offline photos of the folder.
    struct Candidate {
        id: PhotoId,
        from: PathBuf,
        offline: bool,
    }
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut ids: Vec<PhotoId> = s
        .masters
        .values()
        .filter(|m| m.folder == master.folder && m.id != inputs.photo)
        .map(|m| m.id)
        .collect();
    ids.sort();
    for id in ids {
        let Some(path) = s.path_of(id) else { continue };
        let offline = match state(&path) {
            FileState::Missing => false,
            FileState::Unavailable => true,
            FileState::Present | FileState::Error(_) => continue,
        };
        candidates.push(Candidate {
            id,
            from: path,
            offline,
        });
    }
    // The files beside the one picked, by name; never one a photo names.
    let files: Vec<PathBuf> = match list(&directory) {
        Listing::Listed { files, .. } => {
            let mut files: Vec<PathBuf> = files
                .into_iter()
                .map(|n| directory.join(n))
                .filter(|f| f != file && !known.contains(f))
                .collect();
            files.sort();
            files
        }
        Listing::Absent => Vec::new(),
        Listing::Failed(e) => bail!("{}: {e}", directory.display()),
    };
    let mut found: BTreeMap<PathBuf, (Identity, Vec<(PhotoId, Evidence)>)> = BTreeMap::new();
    if !candidates.is_empty() {
        let by_id: HashMap<PhotoId, &Candidate> = candidates.iter().map(|c| (c.id, c)).collect();
        // What can be asked of each file without reading it: the sizes on
        // record, the edits' identities and the names.
        let sizes: HashSet<u64> = candidates
            .iter()
            .filter_map(|c| s.checksums.get(&c.id).map(|k| k.size))
            .collect();
        let edits: HashMap<(u64, String), Vec<PhotoId>> = s
            .by_edit
            .iter()
            .map(|(key, photos)| {
                let photos: Vec<PhotoId> = photos
                    .iter()
                    .filter(|p| by_id.get(p).is_some_and(|c| !c.offline))
                    .filter(|p| !s.checksums.contains_key(p))
                    .copied()
                    .collect();
                (key.clone(), photos)
            })
            .filter(|(_, photos)| !photos.is_empty())
            .collect();
        let edit_sizes: HashSet<u64> = edits.keys().map(|(size, _)| *size).collect();
        let mut by_name: HashMap<String, Vec<PhotoId>> = HashMap::new();
        for c in &candidates {
            if !c.offline && !s.checksums.contains_key(&c.id) {
                let name = c.from.file_name().unwrap_or_default();
                by_name.entry(folded(name)).or_default().push(c.id);
            }
        }
        for (n, path) in files.iter().enumerate() {
            if cancel.is_cancelled() {
                return Err(stopped());
            }
            progress(n + 1);
            let Ok(size) = std::fs::metadata(path).map(|m| m.len()) else {
                continue;
            };
            let mut hits: Vec<(PhotoId, Evidence)> = Vec::new();
            let mut read: Option<FileChecksum> = None;
            if sizes.contains(&size) {
                let quick = match FileChecksum::read_quick(path, go_on)? {
                    ChecksumRead::Done(quick) => quick,
                    ChecksumRead::Changed => continue,
                    ChecksumRead::Cancelled => return Err(stopped()),
                };
                let mut full: Option<Option<String>> = None;
                for c in &candidates {
                    let Some(theirs) = s.checksums.get(&c.id) else {
                        continue;
                    };
                    if theirs.size != quick.size || theirs.quick != quick.quick {
                        continue;
                    }
                    match &theirs.full {
                        Some(theirs) => {
                            let ours = match &full {
                                Some(ours) => ours.clone(),
                                None => {
                                    let ours = match FileChecksum::read_full(path, go_on)? {
                                        ChecksumRead::Done(read) => read.full,
                                        ChecksumRead::Changed => None,
                                        ChecksumRead::Cancelled => return Err(stopped()),
                                    };
                                    full.insert(ours).clone()
                                }
                            };
                            if ours.as_ref() == Some(theirs) {
                                hits.push((c.id, Evidence::Verified));
                            }
                        }
                        // Only a photo observed missing is offered unverified.
                        None if !c.offline => hits.push((c.id, Evidence::Probable)),
                        None => {}
                    }
                }
                read = Some(quick);
            }
            let mut identity = read.map(|r| r.identity);
            if edit_sizes.contains(&size) {
                let Ok(now) = Identity::read(path) else {
                    continue;
                };
                for id in edits
                    .get(&(now.size, now.prefix_hash.clone()))
                    .into_iter()
                    .flatten()
                {
                    hits.push((*id, Evidence::Probable));
                }
                identity.get_or_insert(now);
            }
            let name = folded(path.file_name().unwrap_or_default());
            for id in by_name.get(&name).into_iter().flatten() {
                if !hits.iter().any(|(hit, _)| hit == id) {
                    hits.push((*id, Evidence::Name));
                }
            }
            if hits.is_empty() {
                continue;
            }
            let identity = match identity {
                Some(identity) => identity,
                None => match Identity::read(path) {
                    Ok(identity) => identity,
                    Err(_) => continue,
                },
            };
            found.insert(path.clone(), (identity, hits));
        }
    }
    if cancel.is_cancelled() {
        return Err(stopped());
    }
    // A photo two files claim, or a file two photos claim, is the user's
    // to settle, as in a synchronization.
    let offline_of: HashMap<PhotoId, (&PathBuf, bool)> = candidates
        .iter()
        .map(|c| (c.id, (&c.from, c.offline)))
        .collect();
    let mut claims: HashMap<PhotoId, usize> = HashMap::new();
    for (_, hits) in found.values() {
        for (id, _) in hits {
            *claims.entry(*id).or_default() += 1;
        }
    }
    let mut plan = SyncPlan {
        kind: PlanKind::FindOnDisk {
            photo: inputs.photo,
            file: file.clone(),
        },
        ..Default::default()
    };
    let mut others: Vec<Move> = Vec::new();
    let mut choices: Vec<Vec<Move>> = Vec::new();
    for (to, (identity, hits)) in found {
        let moves: Vec<Move> = hits
            .iter()
            .map(|(id, how)| {
                let (from, offline) = offline_of[id];
                Move {
                    photo: *id,
                    from: from.clone(),
                    to: to.clone(),
                    identity: identity.clone(),
                    verified: *how == Evidence::Verified,
                    by_name: *how == Evidence::Name,
                    offline,
                    target: target.clone(),
                }
            })
            .collect();
        if moves.len() == 1 && claims[&moves[0].photo] == 1 {
            others.extend(moves);
        } else {
            choices.push(moves);
        }
    }
    others.sort_by_key(|m| (!m.verified, m.by_name, m.to.clone()));
    plan.moves.push(picked);
    plan.moves.extend(others);
    plan.choices = merge_choices(choices);
    let acted = plan.moves.iter().map(|m| m.photo).chain(
        plan.choices
            .iter()
            .flat_map(|c| c.options.iter().map(|m| m.photo)),
    );
    for id in acted {
        if let (Some(master), Some(digest)) = (s.masters.get(&id), s.digests.get(&id)) {
            plan.scanned.insert(id, (master.clone(), *digest));
        }
    }
    Ok(plan)
}

#[cfg(test)]
#[path = "find_tests.rs"]
mod tests;
