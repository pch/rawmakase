//! Making a folder (New Folder Inside): an exclusive `mkdir` below a folder
//! of a root, then the catalog's folder. The path is journalled before the
//! folder is made, and the file object and the name the disk gave it after;
//! the catalog has the folder only once it is proven there. A folder that
//! was already there is never taken for the new one, and one that was made
//! is never deleted: what a crash leaves is added to the catalog when the
//! user asks and it is proven the one made, or kept as it is.
use super::super::db::{Reads, Write, sql};
use super::super::folder_sync::locate_folders;
use super::super::locations::{join, logical_location};
use super::super::sidecar::caseless;
use super::super::{FolderId, RootId};
use super::conflicts::Names;
use super::disk::{Disk, Presence, parent, presence, spelled};
use super::journal::{self, Item, ItemState, Subject};
use super::names::{self, NameProblem};
use super::preconditions::RELOCATED;
use crate::fsops::{EntryKind, FileId, FsError};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// A folder item's journalled state: where the folder goes, and what must
/// still hold of the catalog for it to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::catalog) struct FolderState {
    pub root: RootId,
    /// The parent's logical path, and where it resolved on this computer.
    pub parent: String,
    pub parent_path: PathBuf,
    /// The name asked for.
    pub name: String,
    /// The catalog folder it became, once the catalog has it.
    pub folder: Option<FolderId>,
}

impl FolderState {
    /// Why the folder can't be made or added: its parent resolves elsewhere
    /// now. `None` when it still resolves where it was planned.
    pub(super) fn check(&self, db: &impl Reads, computer: &str) -> Result<Option<&'static str>> {
        let now = logical_location(db, computer, self.root, &self.parent)?;
        Ok((now.as_ref() != Some(&self.parent_path)).then_some(RELOCATED))
    }
}

/// A folder a plan makes.
#[derive(Clone, Debug)]
pub(super) struct NewFolder {
    pub state: FolderState,
    /// Where it is made: below its parent's canonical path.
    pub path: PathBuf,
    /// How the parent compares names.
    pub rules: Names,
}

/// A folder made and in the catalog.
pub(super) struct Made {
    pub folder: FolderId,
    /// Its canonical path, as the disk spells it.
    pub dir: PathBuf,
    /// Where the catalog places it on this computer.
    pub resolved: PathBuf,
}

/// Why `name` can't be a folder's name, on any system the catalog may be
/// used on; `None` when it can.
pub fn folder_name_problem(name: &str) -> Option<&'static str> {
    Some(match names::problem(name)? {
        NameProblem::Empty => "Give the folder a name.",
        NameProblem::Dots => "That name is taken by the system.",
        NameProblem::Separator => "A folder's name can't have / or \\ in it.",
        NameProblem::Control => "A folder's name can't have control characters in it.",
        NameProblem::Windows => {
            "A folder's name can't have : * ? \" < > or | in it, which Windows refuses."
        }
        NameProblem::Trailing => {
            "A folder's name can't end in a dot or a space, which Windows refuses."
        }
        NameProblem::Device => "Windows keeps that name for a device.",
        NameProblem::Long => "That name is too long.",
    })
}

/// The catalog folders that hold logical path `logical` of `root`, or a
/// path below it, as `sidecar::caseless` compares names, offline or
/// missing ones included; and those of any root that resolve there on
/// `computer`.
pub(super) fn in_catalog(
    db: &impl Reads,
    computer: &str,
    root: RootId,
    logical: &str,
    path: &Path,
) -> Result<Vec<(FolderId, String)>> {
    let key = caseless(OsStr::new(logical));
    let below = |k: &str| k == key || k.starts_with(&format!("{key}/"));
    let at = caseless(path.as_os_str());
    let resolves = |p: &Path| {
        let k = caseless(p.as_os_str());
        k == at || Path::new(&k).starts_with(&at)
    };
    Ok(locate_folders(db, computer)?
        .into_iter()
        .filter(|(_, f)| {
            (f.root == root && below(&caseless(OsStr::new(&f.logical))))
                || f.path.as_deref().is_some_and(resolves)
        })
        .map(|(id, f)| (id, f.logical))
        .collect())
}

/// Why folder `new` can't be made now: a folder or file has its name there
/// on disk (or one the folder takes for it), or the catalog has a folder
/// there, or a photo of that name there, offline ones included. `None`
/// when it can. Planning asks, and execution again in the transaction that
/// journals the folder before it is made.
pub(super) fn taken(
    db: &impl Reads,
    computer: &str,
    disk: &dyn Disk,
    new: &NewFolder,
) -> Result<Option<String>> {
    let (dir, name) = (parent(&new.path), OsStr::new(&new.state.name));
    let names = disk.list(dir)?;
    let key = new.rules.key(name);
    let on_disk = names.iter().any(|n| n == name || new.rules.key(n) == key)
        || !matches!(disk.entry(&new.path), Err(FsError::NotFound));
    if on_disk {
        return Ok(Some(format!(
            "A folder or file named {} is already there. Add a folder with Add Folder.",
            new.state.name
        )));
    }
    let logical = join(&new.state.parent, &new.state.name);
    if !in_catalog(db, computer, new.state.root, &logical, &new.path)?.is_empty() {
        return Ok(Some(format!(
            "The catalog has a folder named {} there, though it may be missing.",
            new.state.name
        )));
    }
    // The catalog's folders over the parent, through any root or folder
    // that resolves there: by the directory itself (through a link too), or
    // by its path.
    let located = locate_folders(db, computer)?;
    let at = caseless(dir.as_os_str());
    let mut over = super::plan::Folders::index(&located).over(dir);
    over.extend(
        located
            .iter()
            .filter(|(_, f)| {
                f.path
                    .as_deref()
                    .is_some_and(|p| caseless(p.as_os_str()) == at)
            })
            .map(|(id, _)| *id),
    );
    over.sort();
    over.dedup();
    let wanted = caseless(name);
    for folder in over {
        let filenames: Vec<String> = db.read(
            sql!("SELECT filename FROM photos WHERE folder=?"),
            &[&folder],
        )?;
        if filenames.iter().any(|f| caseless(OsStr::new(f)) == wanted) {
            return Ok(Some(format!(
                "A photo of the catalog is named {} there.",
                new.state.name
            )));
        }
    }
    Ok(None)
}

/// The folder just made at `path`: its file object, and its path as the
/// disk spells its name (which may differ from the name asked for, on a
/// volume that normalises names), from its parent's listing.
pub(super) fn made(disk: &dyn Disk, path: &Path) -> Result<(FileId, PathBuf), FsError> {
    let found = disk.entry(path)?;
    if found.kind != EntryKind::Directory {
        return Err(FsError::Io(std::io::Error::other(
            "what was made is not a folder",
        )));
    }
    Ok((found.id, spelled(disk, path, found.id)?))
}

/// Adds the folder journalled `item` made to the catalog, in `w`'s
/// transaction, if it is proven there (the file object made, by the name
/// the disk gave it) and its parent still resolves where it did. Marks it
/// `done`, or `needs_recovery` with the reason it can't be.
pub(super) fn complete(
    w: &mut Write<'_>,
    computer: &str,
    disk: &dyn Disk,
    item: &Item,
) -> Result<()> {
    let Subject::Folder(state) = &item.subject else {
        anyhow::bail!("not a folder operation");
    };
    let member = item
        .members
        .first()
        .context("the folder operation has no folder")?;
    let added = match state.check(w, computer)? {
        Some(reason) => Err(reason.to_string()),
        None => match (member.id, member.destination.as_deref()) {
            (Some(id), Some(at)) if presence(disk, at, id) == Presence::Here => {
                add(w, computer, state, at)?
            }
            _ => Err("the folder that was made isn't there".into()),
        },
    };
    match added {
        Ok(folder) => {
            let state = FolderState {
                folder: Some(folder),
                ..state.clone()
            };
            journal::set_folder(w, item.id, &state)?;
            journal::set_state(w, item.id, ItemState::Done, None)
        }
        Err(reason) => journal::set_state(w, item.id, ItemState::NeedsRecovery, Some(&reason)),
    }
}

/// The catalog's folder for `at`, made below `state`'s parent: the one it
/// has at exactly that logical path, else a new one. Why not, when the
/// catalog has another folder of that name, or places it elsewhere.
fn add(
    w: &mut Write<'_>,
    computer: &str,
    state: &FolderState,
    at: &Path,
) -> Result<std::result::Result<FolderId, String>> {
    let name = at
        .file_name()
        .and_then(OsStr::to_str)
        .context("not a name the catalog keeps")?;
    let logical = join(&state.parent, name);
    if logical_location(w, computer, state.root, &logical)?.as_deref()
        != Some(&state.parent_path.join(name))
    {
        return Ok(Err(
            "the catalog places a folder of that name elsewhere".into()
        ));
    }
    let there = in_catalog(w, computer, state.root, &logical, at)?;
    match there.as_slice() {
        [] => Ok(Ok(super::super::ingest::folder_id(
            w, state.root, &logical,
        )?)),
        [(folder, l)] if *l == logical => Ok(Ok(*folder)),
        _ => Ok(Err(format!(
            "the catalog has another folder named {name} there"
        ))),
    }
}

/// The catalog's folder for move destination `into`, directory `at`, in
/// `w`'s transaction: the one it has at exactly that logical path (`false`)
/// or, when it has none and no other folder resolves to `at`, a new one
/// (`true`). Why not, when the catalog places the path elsewhere now, or
/// has another folder over that directory.
pub(super) fn arrive(
    w: &mut Write<'_>,
    computer: &str,
    into: &super::preconditions::Arrival,
    at: &Path,
) -> Result<std::result::Result<(FolderId, bool), String>> {
    if logical_location(w, computer, into.root, &into.logical)?.as_ref() != Some(&into.resolved) {
        return Ok(Err("the catalog places that folder elsewhere now".into()));
    }
    let key = caseless(OsStr::new(&into.logical));
    let (resolved, dir) = (
        caseless(into.resolved.as_os_str()),
        caseless(at.as_os_str()),
    );
    let over: Vec<(FolderId, super::super::folder_sync::Located)> = locate_folders(w, computer)?
        .into_iter()
        .filter(|(_, f)| {
            (f.root == into.root && caseless(OsStr::new(&f.logical)) == key)
                || f.path.as_deref().is_some_and(|p| {
                    let p = caseless(p.as_os_str());
                    p == resolved || p == dir
                })
        })
        .collect();
    match over.as_slice() {
        [] => Ok(Ok((
            super::super::ingest::folder_id(w, into.root, &into.logical)?,
            true,
        ))),
        [(folder, f)] if f.root == into.root && f.logical == into.logical => {
            Ok(Ok((*folder, false)))
        }
        _ => Ok(Err("the catalog has another folder there now".into())),
    }
}

/// Why a folder item is still unfinished when no file object was
/// journalled: what is there may not be the folder made.
pub(super) const NOT_PROVEN: &str = "A folder is there that RAWmakase can't tell it made; it stays out of the catalog. Add it with Add Folder if it is yours.";

/// Decides journalled folder `item` (recovery's rule 5): made and proven
/// there, it waits for the user to add it (or is finished, if the catalog
/// has it already); never made, or gone, nothing changed; anything else,
/// such as a folder there whose making wasn't journalled, is never adopted.
pub(super) fn decide(
    w: &mut Write<'_>,
    computer: &str,
    disk: &dyn Disk,
    item: &Item,
) -> Result<()> {
    let Subject::Folder(state) = &item.subject else {
        anyhow::bail!("not a folder operation");
    };
    let member = item
        .members
        .first()
        .context("the folder operation has no folder")?;
    let unfinished = |w: &mut Write<'_>, reason: &str| {
        journal::set_state(w, item.id, ItemState::NeedsRecovery, Some(reason))
    };
    if let Some(reason) = state.check(w, computer)? {
        return unfinished(w, reason);
    }
    match (member.id, member.destination.as_deref()) {
        (Some(id), Some(at)) => match presence(disk, at, id) {
            Presence::Here => {
                let name = at.file_name().and_then(OsStr::to_str).unwrap_or_default();
                let logical = join(&state.parent, name);
                let there = in_catalog(w, computer, state.root, &logical, at)?;
                if matches!(there.as_slice(), [(_, l)] if *l == logical) {
                    complete(w, computer, disk, item)
                } else {
                    unfinished(w, "The new folder isn't in the catalog yet.")
                }
            }
            Presence::Absent => journal::set_state(w, item.id, ItemState::NoChange, None),
            Presence::Unknown => unfinished(w, "The folder that was made can't be found."),
        },
        // Made or not, nothing says which folder is the one made.
        _ => {
            let (dir, name) = (parent(&member.source), member.source.file_name());
            let wanted = caseless(name.unwrap_or_default());
            let never_made = disk
                .list(dir)
                .is_ok_and(|names| names.iter().all(|n| caseless(n) != wanted))
                && matches!(disk.entry(&member.source), Err(FsError::NotFound));
            if never_made {
                journal::set_state(w, item.id, ItemState::NoChange, None)
            } else {
                unfinished(w, NOT_PROVEN)
            }
        }
    }
}
