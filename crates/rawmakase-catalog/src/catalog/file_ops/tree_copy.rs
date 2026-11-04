//! Moving a folder to another volume (#572 C), where no rename reaches: its
//! whole tree (every file and folder below it, hidden ones and those the
//! catalog doesn't know too) copied there, the catalog following, then the
//! originals to the Trash. Every step is journalled before the next.
//!
//! The tree is built inside one hidden folder in the destination folder,
//! named uniquely to the run (as a photo's partial copy is), which
//! Synchronize Folder and Add Folder pass by; nothing of it is seen at the
//! folder's name until it is whole. Members are the folder itself, the
//! folders below it (each before those in it), then its files:
//! 1. the hidden folder, then each folder in it, made where nothing has its
//!    name, its file object and spelling journalled (`copying`);
//! 2. each file made in its folder there where nothing has its name, then
//!    copied, hashed, synced, given the original's time, read back and the
//!    original re-checked, as a photo's copy is ([`copy::write`]), and
//!    journalled `copied`;
//! 3. the folders given their originals' extras (extended attributes,
//!    alternate data streams), checked, and synced, deepest first; the
//!    hidden folder renamed
//!    exclusively to the folder's name, its folder synced, and every
//!    member's destination journalled as the disk spells it (the folder
//!    `copied`).
//!
//! Then, in one transaction, the evidence (every copy where it was made,
//! every original still the file it copied, the catalog's folders and photos
//! of the subtree as planned), the subtree re-keyed to the destination as a
//! rename does, a photo's saved edit and checksum re-keyed where the volume
//! keeps times more coarsely, and the item `committed`. Last, each folder
//! there is given its original's times and permission bits (now nothing
//! more is written in it, and a read-only one could no longer be renamed),
//! then the originals: when every one is proven as copied, its extras still
//! its copy's, and the folder holds exactly what was planned, it goes to
//! the Trash whole; otherwise each file proven so goes on its own, and the
//! rest stay, with the folders, reported (`original_kept`).
//!
//! Anything that fails before the commit undoes it. Still hidden, its copies
//! are removed newest first (one written whole only while its original is
//! proven as copied; on Put Back to the Trash instead) and its empty folders
//! with them, by their journalled file objects (or, made but not yet
//! journalled, by their name inside the hidden folder and only while
//! empty). Already named, the whole copy goes to the Trash, and only while
//! it holds exactly what was made, each copy read back. Otherwise it is
//! left for the user, as is every file of it on an SMB share, where nothing
//! is removed ([`copy::LEFT_ON_SHARE`]). Recovery does the same
//! ([`decide`]).
use super::super::Catalog;
use super::super::db::{Reads, Write, sql};
use super::super::locations::{join, logical_from_os, names};
use super::super::sidecar::caseless;
use super::super::value::row;
use super::super::{FolderId, PhotoId};
use super::companions::is_legacy_edit;
use super::conflicts::Names;
use super::copy::{self, Cx, Failed, Originals, Problem, copy_at, extras_differ, original_here};
use super::disk::{Disk, Presence, gone, parent, presence, spelled};
use super::journal::{self, Item, ItemState, Kind, Member, MemberState, NewTreeMember, Subject};
use super::plan::occupied;
use super::preconditions::CHANGED;
use super::proof::{self, Proof, Proofs};
use super::recover::NOT_BACK;
use super::tree::TreeMove;
use super::{FileOpError, TrashError};
use crate::fsops::{EntryKind, FileId, FsError, VolumeId};
use crate::storage::Identity;
use anyhow::{Context, Result};
use rawmakase_cancel::Cancel;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// What a folder item copied to another volume journals of its layout: its
/// first `directories` members are folders (the folder itself first), the
/// rest files.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::catalog) struct TreeAcross {
    pub directories: usize,
    /// How its copies and originals are proven and made (#573).
    #[serde(default, skip_serializing_if = "Proofs::is_default")]
    pub proofs: Proofs,
}

/// A name of the tree, as planned.
#[derive(Clone, Debug)]
pub(super) struct Entry {
    /// Its path below the folder ("" for the folder itself).
    pub rel: PathBuf,
    pub id: FileId,
    /// A file's `Identity`; `None` for a folder.
    pub identity: Option<Identity>,
}

/// A folder copied to another volume, as planned.
#[derive(Clone, Debug)]
pub(super) struct TreeCopy {
    /// The hidden folder it is built in, beside where it goes; named once
    /// the run is known.
    pub hidden: PathBuf,
    /// The destination's volume.
    pub volume: VolumeId,
    /// The folder, its folders (each before those in it), then its files.
    pub entries: Vec<Entry>,
    /// The bytes it copies.
    pub bytes: u64,
    /// Where the destination's volume is mounted, and the bytes free there.
    pub top: PathBuf,
    pub available: u64,
    /// Where each catalog folder of the subtree is: the deepest folder
    /// copied its path finds on disk, and the rest of its own names (those
    /// of a missing folder); and the entry each master photo's file is (see
    /// [`map`]).
    pub folders: Vec<(FolderId, usize, PathBuf)>,
    pub photos: Vec<(PhotoId, usize)>,
    /// How its copies and originals are proven and made (#573).
    pub proofs: Proofs,
    /// The folder stays where it is once copied ([`proof::no_trash`]).
    pub originals_stay: bool,
}

impl TreeCopy {
    pub(super) fn files(&self) -> usize {
        self.entries.iter().filter(|e| e.identity.is_some()).count()
    }
    pub(super) fn directories(&self) -> usize {
        self.entries.len() - self.files()
    }
}

/// Every name below folder `top`, read through `disk`: the folder, its
/// folders breadth first, then its files by path. Why it can't be copied,
/// otherwise: a link, a special file, another volume inside it, a file with
/// another name elsewhere, a name the journal can't record, or two names
/// that `rules` (where given) take for one.
/// On FAT and exFAT (`compose`), whose drivers list names decomposed but
/// take them composed, each name is given as `proof::usable` finds it; and
/// on macOS a file's
/// AppleDouble file there (`._name` beside `name`, where the system keeps
/// what the drive can't, and which it makes, moves and removes with it) is
/// part of that file, not a name of its own.
pub(super) fn walk(
    disk: &dyn Disk,
    top: &Path,
    rules: Option<Names>,
    compose: bool,
) -> std::result::Result<Vec<Entry>, String> {
    let at = disk.entry(top).map_err(|e| e.to_string())?;
    if at.kind != EntryKind::Directory {
        return Err(format!("{} is not a folder.", top.display()));
    }
    let mut dirs = vec![Entry {
        rel: PathBuf::new(),
        id: at.id,
        identity: None,
    }];
    let mut files = Vec::new();
    let mut next = 0;
    while next < dirs.len() {
        let rel = dirs[next].rel.clone();
        next += 1;
        let dir = top.join(&rel);
        let mut listed = disk.list(&dir).map_err(|e| e.to_string())?;
        if compose && cfg!(target_vendor = "apple") {
            listed = without_apple_double(disk, &dir, listed);
        }
        listed.sort();
        // Every later step finds a copy by `sidecar::caseless`, which must
        // tell each name from the others; and the destination's own rules.
        let mut keys: HashMap<String, &std::ffi::OsString> = HashMap::new();
        for name in &listed {
            let mut alike = vec![caseless(name)];
            alike.extend(rules.map(|r| format!("\u{0}{}", r.key(name))));
            for key in alike {
                if let Some(other) = keys.insert(key, name) {
                    return Err(format!(
                        "{} and {} in {} would be one name on the other drive.",
                        other.to_string_lossy(),
                        name.to_string_lossy(),
                        dir.display()
                    ));
                }
            }
        }
        for name in listed {
            let path = dir.join(&name);
            let name = match compose {
                true => proof::usable(disk, &dir, &name),
                false => name,
            };
            if name.to_str().is_none() {
                return Err(format!(
                    "{} has a name RAWmakase can't record.",
                    path.display()
                ));
            }
            let e = disk.entry(&path).map_err(|e| e.to_string())?;
            let shown = path.display();
            match e.kind {
                EntryKind::Directory if e.id.volume() != at.id.volume() => {
                    return Err(format!("{shown} is another drive inside the folder."));
                }
                EntryKind::Directory => dirs.push(Entry {
                    rel: rel.join(&name),
                    id: e.id,
                    identity: None,
                }),
                EntryKind::File if e.links != 1 => {
                    return Err(format!("{shown} has another name elsewhere (a hard link)."));
                }
                EntryKind::File => {
                    let identity = disk.identity(&path).map_err(|e| format!("{e:#}"))?;
                    files.push(Entry {
                        rel: rel.join(&name),
                        id: e.id,
                        identity: Some(identity),
                    });
                }
                EntryKind::Symlink => return Err(format!("{shown} is a link.")),
                EntryKind::Other => return Err(format!("{shown} isn't a file or a folder.")),
            }
        }
    }
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    dirs.extend(files);
    Ok(dirs)
}

/// `names` in folder `dir` without the AppleDouble files of others of
/// them: a file (never a folder) `._name` where `name` (by
/// `sidecar::caseless`) is among them too.
fn without_apple_double(
    disk: &dyn Disk,
    dir: &Path,
    names: Vec<std::ffi::OsString>,
) -> Vec<std::ffi::OsString> {
    let keys: std::collections::HashSet<String> = names.iter().map(|n| caseless(n)).collect();
    names
        .into_iter()
        .filter(|n| !apple_double(disk, dir, n, &keys))
        .collect()
}

/// Whether `name` in folder `dir` is the AppleDouble file macOS keeps for
/// another of the names `keys` (by `sidecar::caseless`) on FAT and exFAT:
/// a file `._name`.
fn apple_double(
    disk: &dyn Disk,
    dir: &Path,
    name: &OsStr,
    keys: &std::collections::HashSet<String>,
) -> bool {
    name.to_string_lossy()
        .strip_prefix("._")
        .is_some_and(|of| keys.contains(&caseless(OsStr::new(of))))
        && disk
            .entry(&dir.join(name))
            .is_ok_and(|e| e.kind == EntryKind::File)
}

/// Plans copying folder `source` into folder `dir` of volume `volume` on
/// `disk`, whose names compare by `rules`: refused unless both volumes'
/// file ids are proven, every name below it can be copied, and `dir`'s
/// volume has room for it.
pub(super) fn plan(
    disk: &dyn Disk,
    source: &Path,
    dir: &Path,
    volume: VolumeId,
    rules: Names,
) -> std::result::Result<TreeCopy, FileOpError> {
    let refuse = |why: String| FileOpError::Destination(why);
    let here = disk
        .volume_facts(parent(source))
        .map_err(|e| refuse(e.to_string()))?;
    let there = disk.volume_facts(dir).map_err(|e| refuse(e.to_string()))?;
    let (Some(originals), Some(copies)) = (proof::of(&here), proof::of(&there)) else {
        return Err(refuse(FOLDER_IDS_UNPROVEN.into()));
    };
    let proofs = Proofs {
        originals,
        copies,
        direct: !there.exclusive_rename,
        since: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64),
    };
    // Its originals go to the Trash once copied.
    if here.read_only {
        return Err(refuse("The folder's volume is read-only.".into()));
    }
    if let Some(there) = alike(disk, dir, source.file_name().unwrap_or_default())
        .map_err(|e| refuse(e.to_string()))?
    {
        return Err(refuse(format!(
            "A file or folder named {} is already there.",
            there.to_string_lossy()
        )));
    }
    let entries = walk(disk, source, Some(rules), originals == Proof::Names)
        .map_err(|why| refuse(format!("{why} The folder can't be moved to another drive.")))?;
    // Made on FAT or exFAT, a file `._name` beside `name` would be taken
    // there for `name`'s AppleDouble file, and written over.
    if cfg!(target_vendor = "apple") && copies == Proof::Names {
        let keys: std::collections::HashSet<(PathBuf, String)> = entries
            .iter()
            .filter_map(|e| Some((e.rel.parent()?.to_path_buf(), caseless(e.rel.file_name()?))))
            .collect();
        let taken = entries.iter().find(|e| {
            let (Some(up), Some(name)) = (e.rel.parent(), e.rel.file_name()) else {
                return false;
            };
            e.identity.is_some()
                && name
                    .to_string_lossy()
                    .strip_prefix("._")
                    .is_some_and(|of| keys.contains(&(up.to_path_buf(), caseless(OsStr::new(of)))))
        });
        if let Some(e) = taken {
            return Err(refuse(format!(
                "{} would be taken for Mac metadata of the file beside it on the other drive. The folder can't be moved there.",
                source.join(&e.rel).display()
            )));
        }
    }
    let bytes = entries
        .iter()
        .filter_map(|e| e.identity.as_ref())
        .fold(0u64, |sum, i| sum.saturating_add(i.size));
    if bytes.saturating_add(super::plan::SPARE) > there.available {
        return Err(FileOpError::NoSpace {
            needed: bytes,
            available: there.available,
        });
    }
    Ok(TreeCopy {
        hidden: PathBuf::new(),
        volume,
        entries,
        bytes,
        top: super::disk::top(disk, dir),
        available: there.available,
        folders: Vec::new(),
        photos: Vec::new(),
        proofs,
        originals_stay: proof::no_trash(&here),
    })
}

/// A name in folder `dir` that `sidecar::caseless` takes for `name`, which
/// every later step would confuse with the copy's.
fn alike(
    disk: &dyn Disk,
    dir: &Path,
    name: &OsStr,
) -> std::result::Result<Option<std::ffi::OsString>, FsError> {
    let wanted = caseless(name);
    Ok(disk.list(dir)?.into_iter().find(|n| caseless(n) == wanted))
}

/// Why a folder isn't copied to or from a drive whose file ids can't prove
/// which file a name is.
const FOLDER_IDS_UNPROVEN: &str = "Moving folders to or from this kind of drive (network drives other than SMB shares, and others) isn't available yet. Copy it in Finder, then add the folder.";

/// The members `tree` journals, copied as `copy`: each name's original,
/// where it goes, and where it is made inside the hidden folder.
fn members(tree: &TreeMove, copy: &TreeCopy) -> Vec<NewTreeMember> {
    copy.entries
        .iter()
        .map(|e| NewTreeMember {
            source: tree.source.join(&e.rel),
            destination: tree.destination.join(made_rel(copy, &e.rel)),
            id: e.id,
            identity: e.identity.clone(),
            temp: copy.hidden.join(made_rel(copy, &e.rel)),
        })
        .collect()
}

/// `rel`, as what is made there is named: composed, each name, where names
/// prove its copies (#573).
fn made_rel(copy: &TreeCopy, rel: &Path) -> PathBuf {
    match copy.proofs.copies {
        Proof::Names => rel.iter().map(proof::composed).collect(),
        Proof::Ids => rel.to_path_buf(),
    }
}

/// Copies folder `tree` as `copy`, journalled first as item of `batch`,
/// then lets the catalog follow and puts its originals in the Trash;
/// undoes it on any failure before the catalog follows. Left alone, with
/// why, when `busy` (an unfinished operation involves its photos) or
/// something changed since it was planned. Errs only when the catalog
/// can't be written.
#[allow(clippy::too_many_arguments, reason = "one item's run")]
pub(super) fn run(
    cx: &Cx<'_>,
    catalog: &mut Catalog,
    batch: &str,
    kind: Kind,
    tree: &TreeMove,
    copy: &TreeCopy,
    busy: bool,
    cancel: Cancel<'_>,
) -> Result<()> {
    // a. Checked again and journalled, in one transaction.
    let started = catalog.db.write_immediate(|w| {
        let reason = match tree.state.check(w, cx.computer)? {
            Some(reason) => Some(reason),
            None if busy || super::tree::unfinished(w, tree)? => {
                Some(super::execute::UNFINISHED.into())
            }
            None if super::tree::photos_in(w, &tree.state.folders)? != tree.state.photos => {
                Some(CHANGED.into())
            }
            None => changed(cx.disk, tree, copy)?,
        };
        let state = match reason {
            None => ItemState::Planned,
            Some(_) => ItemState::NoChange,
        };
        let id = journal::insert_tree_copy(
            w,
            batch,
            cx.computer,
            kind,
            state,
            &tree.state,
            &members(tree, copy),
            reason.as_deref(),
        )?;
        Ok(reason.is_none().then_some(id))
    })?;
    let Some(id) = started else {
        return Ok(());
    };
    match build(cx, catalog, id, tree, copy, cancel) {
        Ok(()) => {}
        Err(Failed::Problem(p)) => return undo(cx, catalog, id, &p.to_string()),
        Err(Failed::Journal(e)) => return undo(cx, catalog, id, &format!("{e:#}")),
    }
    let committed = catalog.db.write_immediate(|w| {
        let journalled = journal::item(w, id)?.context("the journalled item is gone")?;
        commit(w, cx, &journalled, copy)
    });
    match committed {
        Ok(None) => {}
        Ok(Some(why)) => return undo(cx, catalog, id, &why),
        Err(e) => return undo(cx, catalog, id, &format!("{e:#}")),
    }
    let journalled = journal::item(&catalog.db, id)?.context("the journalled item is gone")?;
    finish(cx, catalog, &journalled, false)
}

/// Why folder `tree` is no longer as planned for copying as `copy`: its
/// directory another file object, a link or mount point put in on the way,
/// or its name or the hidden folder's taken there since.
fn changed(disk: &dyn Disk, tree: &TreeMove, copy: &TreeCopy) -> Result<Option<String>> {
    let same = disk
        .entry(&tree.source)
        .is_ok_and(|e| e.id == tree.id && e.kind == EntryKind::Directory);
    if !same {
        return Ok(Some(format!(
            "{} changed since it was planned",
            tree.source.display()
        )));
    }
    for (dir, volume) in [
        (parent(&tree.source), tree.id.volume()),
        (parent(&tree.destination), copy.volume),
    ] {
        let plain = disk.canonical(dir).is_ok_and(|c| c == dir)
            && disk.entry(dir).is_ok_and(|e| e.id.volume() == volume);
        if !plain {
            return Ok(Some(format!(
                "{} changed since it was planned",
                dir.display()
            )));
        }
    }
    if let Some(why) = super::tree::taken(disk, tree)? {
        return Ok(Some(why));
    }
    let name = tree.destination.file_name().unwrap_or_default();
    if let Some(there) = alike(disk, parent(&tree.destination), name)? {
        return Ok(Some(format!("{} is there now", there.to_string_lossy())));
    }
    let hidden = copy.hidden.file_name().unwrap_or_default();
    let listed = disk.list(parent(&copy.hidden))?.iter().any(|n| n == hidden);
    if listed || !matches!(disk.entry(&copy.hidden), Err(FsError::NotFound)) {
        return Ok(Some(format!("{} is there now", copy.hidden.display())));
    }
    Ok(None)
}

/// Steps 1 to 3 of journalled item `id`.
fn build(
    cx: &Cx<'_>,
    catalog: &mut Catalog,
    id: journal::OperationId,
    tree: &TreeMove,
    copy: &TreeCopy,
    cancel: Cancel<'_>,
) -> std::result::Result<(), Failed> {
    let disk = cx.disk;
    let problem = |e: FsError| match e {
        FsError::NoSpace => Problem::NoSpace,
        e => Problem::Disk(e.to_string()),
    };
    let proofs = copy.proofs;
    // Each folder made, as the disk spells it, by its path below the tree.
    let mut made: HashMap<PathBuf, PathBuf> = HashMap::new();
    let mut top = None;
    for (ordinal, e) in copy.entries.iter().enumerate() {
        if cancel.is_cancelled() {
            return Err(Problem::Stopped.into());
        }
        let ordinal = ordinal as i64;
        let temp = match (e.rel.parent(), e.rel.file_name()) {
            (Some(up), Some(name)) => made
                .get(up)
                .ok_or_else(|| Problem::Disk("its folder wasn't made".into()))?
                .join(made_rel(copy, Path::new(name))),
            _ => copy.hidden.clone(),
        };
        let Some(identity) = &e.identity else {
            // 1. A folder. Not made, nothing of it is there: what is (made
            // by another program at its name) is never taken for it.
            if let Err(err) = disk.create_dir(&temp) {
                catalog
                    .db
                    .write(|w| journal::set_discarded(w, id, ordinal, None))
                    .map_err(Failed::Journal)?;
                return Err(match err {
                    FsError::AlreadyExists => Problem::NameTaken(name(&temp)),
                    err => problem(err),
                }
                .into());
            }
            let found = disk.entry(&temp).map_err(problem)?;
            if found.kind != EntryKind::Directory {
                return Err(
                    Problem::Disk(format!("{} isn't the folder made", temp.display())).into(),
                );
            }
            // Its file object only where it stays; elsewhere one no file
            // has, so a release before #573 finds no folder it is.
            let (at, file) = match proofs.copies {
                Proof::Ids => (spelled(disk, &temp, found.id), found.id),
                Proof::Names => (proof::spelled(disk, &temp), FileId::NOBODY),
            };
            let at = at.map_err(problem)?;
            catalog
                .db
                .write(|w| journal::set_copying(w, id, ordinal, Some(file), &at))
                .map_err(Failed::Journal)?;
            if e.rel.as_os_str().is_empty() {
                top = Some(found.id);
            }
            made.insert(e.rel.clone(), at);
            continue;
        };
        // 2. A file, copied and verified.
        let source = tree.source.join(&e.rel);
        copy::write(
            cx,
            catalog,
            (id, ordinal),
            (&source, e.id, identity),
            &temp,
            proofs,
            cancel,
        )?;
        let destination = tree.destination.join(made_rel(copy, &e.rel));
        catalog
            .db
            .write(|w| journal::set_copied(w, id, ordinal, &destination))
            .map_err(Failed::Journal)?;
    }
    // 3. With their originals' extras (which on FAT and exFAT macOS keeps
    // in a file beside each, in the folder holding it), durable, then
    // named, never replacing anything there; built at its name already
    // where the volume can't rename exclusively.
    for e in copy.entries.iter().rev().filter(|e| e.identity.is_none()) {
        let dir = made
            .get(&e.rel)
            .ok_or_else(|| Problem::Disk("a folder wasn't made".into()))?;
        let source = tree.source.join(&e.rel);
        let extras = disk.extras(&source).map_err(problem)?;
        copy::carry_extras(disk, &source, (dir, None), extras)?;
        disk.sync_dir(dir).map_err(problem)?;
    }
    let (Some(hidden), Some(top)) = (made.get(Path::new("")), top) else {
        return Err(Problem::Disk("the folder wasn't made".into()).into());
    };
    if !proofs.direct {
        disk.rename(hidden, &tree.destination)
            .map_err(|e| match e {
                FsError::AlreadyExists => Problem::NameTaken(name(&tree.destination)),
                e => problem(e),
            })?;
    }
    disk.sync_dir(parent(&tree.destination)).map_err(problem)?;
    let at = match proofs.copies {
        Proof::Ids => spelled(disk, &tree.destination, top),
        Proof::Names => proof::spelled(disk, &tree.destination),
    }
    .map_err(problem)?;
    catalog
        .db
        .write(|w| named(w, id, &at))
        .map_err(Failed::Journal)?;
    Ok(())
}

fn name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// Journals item `id`'s hidden folder named `at`: each member's
/// destination there, as its path below the hidden folder is spelled, and
/// the folder `copied`.
fn named(w: &mut Write<'_>, id: journal::OperationId, at: &Path) -> Result<()> {
    let item = journal::item(w, id)?.context("the journalled item is gone")?;
    let hidden = item
        .members
        .first()
        .and_then(|m| m.copy.as_ref())
        .map(|c| c.temp.clone())
        .context("the folder has no hidden folder")?;
    for m in &item.members {
        let temp = &m.copy.as_ref().context("a member isn't copied")?.temp;
        let rel = temp
            .strip_prefix(&hidden)
            .context("a copy outside the hidden folder")?;
        journal::set_destination(w, id, m.ordinal, &at.join(rel))?;
    }
    journal::set_member(w, id, 0, MemberState::Copied, None)
}

/// The proofs journalled folder `item` was copied with.
fn proofs_of(item: &Item) -> Proofs {
    match &item.subject {
        Subject::Tree(state) => state.across.as_ref().map(|a| a.proofs).unwrap_or_default(),
        _ => Proofs::default(),
    }
}

/// Where member `m`'s copy, made and journalled, given the name `at`, is
/// now, as `proofs` says: by its file object ([`copy_at`]); or where names
/// prove it, by the name alone, a folder there, or a file holding the bytes
/// copied (written whole) or its original's first bytes (part way).
/// `Some(None)` when it isn't there, `None` when that can't be told.
fn copy_found(disk: &dyn Disk, proofs: Proofs, m: &Member, at: &Path) -> Option<Option<PathBuf>> {
    let copy = m.copy.as_ref()?;
    match proofs.copies {
        Proof::Ids => copy_at(disk, at, copy.id?),
        Proof::Names => match proof::presence(disk, at) {
            Presence::Absent => Some(None),
            Presence::Unknown => None,
            Presence::Here => {
                let ours = match (&m.identity, copy.content) {
                    (None, _) => disk.entry(at).is_ok_and(|e| e.kind == EntryKind::Directory),
                    (Some(_), Some(content)) => proof::holds(disk, at, content),
                    (Some(_), None) => proof::begins(disk, at, &m.source),
                };
                ours.then(|| Some(proof::composed_path(at)))
            }
        },
    }
}

/// Whether member `m`'s original file is at its source as it was copied,
/// as `proofs` says: its file object and `Identity`, or its name and
/// `Identity`.
fn original_proven(disk: &dyn Disk, proofs: Proofs, m: &Member) -> bool {
    match proofs.originals {
        Proof::Ids => original_here(disk, m),
        Proof::Names => {
            proof::presence(disk, &m.source) == Presence::Here
                && m.identity.is_some()
                && disk.identity(&m.source).ok() == m.identity
        }
    }
}

/// Where member `m`'s original (a file or a folder) is, as `proofs` says.
fn source_presence(disk: &dyn Disk, proofs: Proofs, m: &Member) -> Presence {
    match (proofs.originals, m.id) {
        (Proof::Ids, Some(id)) => presence(disk, &m.source, id),
        (Proof::Ids, None) => Presence::Unknown,
        (Proof::Names, _) => proof::presence(disk, &m.source),
    }
}

/// Whether member `m`'s original is gone from its source by any spelling,
/// as `proofs` says.
fn source_gone(disk: &dyn Disk, proofs: Proofs, m: &Member) -> bool {
    match (proofs.originals, m.id) {
        (Proof::Ids, Some(id)) => gone(disk, &m.source, id),
        (Proof::Ids, None) => false,
        (Proof::Names, _) => proof::gone(disk, &m.source),
    }
}

/// Puts `path` in the Trash; on FAT and exFAT (`proofs`' originals), by its
/// name composed, then decomposed where that is the same file, as those
/// drivers take a name only as it is stored.
fn trash_original(
    cx: &Cx<'_>,
    proofs: Proofs,
    path: &Path,
) -> std::result::Result<super::TrashReceipt, super::TrashError> {
    cx.trash.trash(path).or_else(|e| {
        match (proofs.originals, proof::other_spelling(cx.disk, path)) {
            (Proof::Names, Some(other)) => cx.trash.trash(&other),
            _ => Err(e),
        }
    })
}

/// Each member's path below the folder: its original's below the folder's.
fn rel<'a>(item: &'a Item, m: &'a Member) -> Option<&'a Path> {
    m.source.strip_prefix(&item.members.first()?.source).ok()
}

/// Lets the catalog follow folder item `item`'s copy, planned as `copy`,
/// in `w`'s transaction, if the evidence allows, and marks it
/// `committed`; `Some` with why not otherwise, having written nothing:
/// every check comes before the first write.
fn commit(w: &mut Write<'_>, cx: &Cx<'_>, item: &Item, copy: &TreeCopy) -> Result<Option<String>> {
    let disk = cx.disk;
    let Subject::Tree(state) = &item.subject else {
        return Ok(Some("not a folder operation".into()));
    };
    if let Some(why) = state.check(w, cx.computer)? {
        return Ok(Some(why));
    }
    if super::tree::photos_in(w, &state.folders)? != state.photos {
        return Ok(Some(CHANGED.into()));
    }
    let Some(top) = item.members.first() else {
        return Ok(Some("it has no folder".into()));
    };
    let proofs = copy.proofs;
    let named = match top.destination.as_deref() {
        Some(at) => {
            top.state == MemberState::Copied
                && matches!(copy_found(disk, proofs, top, at), Some(Some(_)))
        }
        None => false,
    };
    let Some(at) = top.destination.as_deref().filter(|_| named) else {
        return Ok(Some("the folder isn't where it was copied".into()));
    };
    // Every copy where it was made, every original still what it copied.
    let mut copies: HashMap<usize, Identity> = HashMap::new();
    for (ordinal, m) in item.members.iter().enumerate().skip(1) {
        let found = match m.destination.as_deref() {
            Some(at) => matches!(copy_found(disk, proofs, m, at), Some(Some(_))),
            None => false,
        };
        if !found {
            return Ok(Some("its copies aren't all where they were made".into()));
        }
        let (Some(was), Some(copy_path)) = (&m.identity, m.destination.as_deref()) else {
            continue;
        };
        if m.state != MemberState::Copied {
            return Ok(Some("its copies aren't all where they were made".into()));
        }
        if !original_proven(disk, proofs, m) {
            return Ok(Some(Problem::Changed(m.source.clone()).to_string()));
        }
        let Ok(now) = disk.identity(copy_path) else {
            return Ok(Some(format!("{} can't be read", copy_path.display())));
        };
        if now != *was && !copy::coarser(was, &now) {
            return Ok(Some(Problem::TimeNotKept(name(copy_path)).to_string()));
        }
        copies.insert(ordinal, now);
    }
    // Each photo by the file object it was planned as: its edit and
    // checksum re-keyed where the copy keeps a coarser time, and its name
    // the copy's, as the disk spells it there.
    let mut rekeys = Vec::new();
    let mut renames = Vec::new();
    for (photo, ordinal) in &copy.photos {
        let (Some(m), Some(now)) = (item.members.get(*ordinal), copies.get(ordinal)) else {
            return Ok(Some("its copies aren't all where they were made".into()));
        };
        let (Some(was), Some(copied)) = (&m.identity, m.destination.as_deref()) else {
            continue;
        };
        let source_name = m.source.file_name().unwrap_or_default();
        if was != now {
            // An earlier release's edit file names the original's time.
            let legacy = item.members.iter().any(|other| {
                parent(&other.source) == parent(&m.source)
                    && is_legacy_edit(source_name, other.source.file_name().unwrap_or_default())
            });
            if legacy {
                return Ok(Some(
                    Problem::LegacyEdit(source_name.to_string_lossy().into_owned()).to_string(),
                ));
            }
            rekeys.push((*photo, was, now));
        }
        let copied = copied
            .file_name()
            .and_then(OsStr::to_str)
            .context("not a name the catalog keeps")?;
        let filename: Option<String> =
            w.read_optional(sql!("SELECT filename FROM photos WHERE id=?"), &[photo])?;
        if filename.as_deref() != Some(copied) {
            renames.push((*photo, copied.to_string()));
        }
    }
    // Each folder by the directory it was planned as, named as the disk
    // spells it there; one with no directory (missing) by its logical path.
    let top_name = at
        .file_name()
        .and_then(OsStr::to_str)
        .context("not a name the catalog keeps")?;
    let mut up: Vec<&str> = names(&state.to).collect();
    up.pop();
    let to = join(&up.join("/"), top_name);
    // Each folder below the folder copied it was planned to be in, as the
    // disk spells that there, then its own names; one not planned (none
    // should be) by its logical path.
    let planned: HashMap<FolderId, (usize, &Path)> = copy
        .folders
        .iter()
        .map(|(f, i, rest)| (*f, (*i, rest.as_path())))
        .collect();
    let mut paths = Vec::new();
    for (folder, logical) in &state.folders {
        let base = planned.get(folder).and_then(|(i, rest)| {
            let base = item
                .members
                .get(*i)?
                .destination
                .as_deref()?
                .strip_prefix(at)
                .ok()?;
            Some(base.join(rest))
        });
        let new = match base {
            Some(rel) => join(&to, &logical_from_os(&rel)),
            None => state.moved(logical, &to)?,
        };
        paths.push((*folder, new));
    }
    // Everything holds: the catalog follows.
    for (photo, was, now) in rekeys {
        let rows: Vec<PhotoId> = w.read(
            sql!("SELECT id FROM photos WHERE id=? OR master_id=?"),
            &[&photo, &photo],
        )?;
        for r in rows {
            super::super::edit_rows::refresh_identity(w, r, was, now)?;
        }
        super::super::checksums::refresh_identity(w, photo, was, now)?;
    }
    for (photo, filename) in &renames {
        w.execute(
            sql!("UPDATE photos SET filename=? WHERE id=? OR master_id=?"),
            &[filename, photo, photo],
        )?;
    }
    super::tree::follow(w, state, &paths)?;
    for (ordinal, now) in &copies {
        journal::set_copy_identity(w, item.id, *ordinal as i64, now)?;
    }
    journal::set_state(w, item.id, ItemState::Committed, None)?;
    Ok(None)
}

/// Finds, for planned `copy` of folder `tree`, the copy each of its
/// catalog folders and photos (masters) is, by the file object its path
/// finds on disk now (which, on a volume that ignores case, finds `A.NEF`
/// for a photo the catalog names `a.NEF`, and on one that doesn't, never
/// another of that name): a photo's entry, when its file is there; a
/// folder's deepest folder found so, and the rest of its own names, which
/// a missing folder keeps.
pub(super) fn map(
    db: &impl Reads,
    disk: &dyn Disk,
    tree: &TreeMove,
    copy: &mut TreeCopy,
) -> Result<()> {
    row! {
        struct PhotoRow {
            id: PhotoId,
            folder: FolderId,
            filename: String,
        }
    }
    let by_id: HashMap<FileId, usize> = copy
        .entries
        .iter()
        .enumerate()
        .map(|(i, e)| (e.id, i))
        .collect();
    let dir_of = |logical: &str| {
        let rest = logical.strip_prefix(tree.state.from.as_str())?;
        Some(names(rest).fold(tree.source.clone(), |p, n| p.join(n)))
    };
    let found = |path: &Path, file: bool| {
        let e = disk.entry(path).ok()?;
        by_id
            .get(&e.id)
            .copied()
            .filter(|i| copy.entries[*i].identity.is_some() == file)
    };
    let mut folders = Vec::new();
    let mut logical: HashMap<FolderId, &str> = HashMap::new();
    for (folder, l) in &tree.state.folders {
        logical.insert(*folder, l);
        let Some(rest) = l.strip_prefix(tree.state.from.as_str()) else {
            continue;
        };
        let (mut deepest, mut path, mut own) = (0, tree.source.clone(), PathBuf::new());
        for name in names(rest) {
            path.push(name);
            match found(&path, false) {
                Some(i) if own.as_os_str().is_empty() => deepest = i,
                _ => own.push(name),
            }
        }
        folders.push((*folder, deepest, own));
    }
    let mut photos = Vec::new();
    for photo in &tree.state.photos {
        let row: Option<PhotoRow> = db.read_optional(
            sql!("SELECT id, folder, filename FROM photos WHERE id=? AND master_id IS NULL"),
            &[photo],
        )?;
        let Some(row) = row else {
            continue;
        };
        let path = logical
            .get(&row.folder)
            .and_then(|l| dir_of(l))
            .map(|d| d.join(&row.filename));
        if let Some(i) = path.and_then(|p| found(&p, true)) {
            photos.push((row.id, i));
        }
    }
    copy.folders = folders;
    copy.photos = photos;
    Ok(())
}

/// Whether every file of `item`'s originals is at its source as it was
/// copied: what discarding its whole copy needs. Its folders are never
/// removed, so they needn't be.
fn originals(disk: &dyn Disk, item: &Item) -> bool {
    let proofs = proofs_of(item);
    item.members
        .iter()
        .filter(|m| m.identity.is_some())
        .all(|m| original_proven(disk, proofs, m))
}

/// Whether folder `item`'s original holds exactly what was planned: the
/// same folders and files (by name and file object), each file as it was
/// copied.
fn as_planned(disk: &dyn Disk, item: &Item) -> bool {
    let Some(top) = item.members.first() else {
        return false;
    };
    let proofs = proofs_of(item);
    let Ok(now) = walk(disk, &top.source, None, proofs.originals == Proof::Names) else {
        return false;
    };
    let planned: HashMap<&Path, &Member> = item
        .members
        .iter()
        .filter_map(|m| Some((rel(item, m)?, m)))
        .collect();
    // By file object where it proves which it is; by name elsewhere, each
    // a folder or a file as planned.
    now.len() == planned.len()
        && now.iter().all(|e| {
            planned.get(e.rel.as_path()).is_some_and(|m| {
                let same = match proofs.originals {
                    Proof::Ids => m.id == Some(e.id),
                    Proof::Names => m.identity.is_some() == e.identity.is_some(),
                };
                same && (m.identity.is_none() || m.identity == e.identity)
            })
        })
}

/// Whether the copy named `at` holds exactly what `item` made: every
/// folder and file by its name below it and its file object, no other
/// name, and each file reading back as copied.
fn as_made(disk: &dyn Disk, item: &Item, at: &Path) -> bool {
    let Some(hidden) = item.members.first().and_then(|m| m.copy.as_ref()) else {
        return false;
    };
    let proofs = proofs_of(item);
    let Ok(now) = walk(disk, at, None, proofs.copies == Proof::Names) else {
        return false;
    };
    let made: HashMap<&Path, &Member> = item
        .members
        .iter()
        .filter_map(|m| Some((m.copy.as_ref()?.temp.strip_prefix(&hidden.temp).ok()?, m)))
        .collect();
    now.len() == made.len()
        && now.iter().all(|e| {
            made.get(e.rel.as_path()).is_some_and(|m| {
                let Some(copy) = &m.copy else {
                    return false;
                };
                let same = match proofs.copies {
                    Proof::Ids => copy.id == Some(e.id),
                    Proof::Names => m.identity.is_some() == e.identity.is_some(),
                };
                same && (m.identity.is_none()
                    || copy
                        .content
                        .is_some_and(|c| disk.hash(&at.join(&e.rel)).is_ok_and(|h| h == c)))
            })
        })
}

/// Puts committed folder item `item`'s originals in the Trash: whole, when
/// each is proven as copied and the folder holds exactly what was planned;
/// otherwise each file proven as copied on its own, the rest and the
/// folders staying, with why. Recovery (`recovering`) also reads each copy
/// back first. Marks it `done`.
fn finish(cx: &Cx<'_>, catalog: &mut Catalog, item: &Item, recovering: bool) -> Result<()> {
    let disk = cx.disk;
    let id = item.id;
    let Some(top) = item.members.first() else {
        return catalog
            .db
            .write(|w| journal::set_state(w, id, ItemState::Done, None));
    };
    let proofs = proofs_of(item);
    let copy_here = |m: &Member| {
        let (Some(at), Some(copy)) = (m.destination.as_deref(), &m.copy) else {
            return false;
        };
        let named = match proofs.copies {
            Proof::Ids => copy
                .id
                .is_some_and(|c| presence(disk, at, c) == Presence::Here),
            Proof::Names => proof::presence(disk, at) == Presence::Here,
        };
        // Where names prove it, its bytes always.
        named
            && copy.identity.is_some()
            && disk.identity(at).ok() == copy.identity
            && ((!recovering && proofs.copies == Proof::Ids)
                || copy
                    .content
                    .is_some_and(|c| disk.hash(at).is_ok_and(|now| now == c)))
    };
    // Where names prove an original, its bytes too, before it goes.
    let unchanged = |m: &Member| {
        original_proven(disk, proofs, m)
            && (proofs.originals == Proof::Ids
                || m.copy
                    .as_ref()
                    .and_then(|c| c.content)
                    .is_some_and(|c| disk.hash(&m.source).is_ok_and(|now| now == c)))
    };
    let files: Vec<&Member> = item
        .members
        .iter()
        .filter(|m| m.identity.is_some())
        .collect();
    // In the Trash whole, where a crash came before its receipt was
    // journalled, or gone some other way: nothing is left to do.
    if source_gone(disk, proofs, top) {
        catalog.db.write(|w| {
            for m in &item.members {
                if matches!(m.state, MemberState::Copied | MemberState::Copying) {
                    journal::set_member(w, id, m.ordinal, MemberState::Trashed, None)?;
                }
            }
            journal::set_state(w, id, ItemState::Done, None)
        })?;
        return Ok(());
    }
    // Each folder there given its original's times and permission bits,
    // deepest first; one that can't be, or whose extras aren't its
    // original's any more, keeps its original, with why. Not again once a
    // file went from it (finishing after a crash, its receipt journalled
    // or not): that dated the original.
    let emptied = |dir: &Path| {
        item.members.iter().any(|f| {
            f.identity.is_some()
                && parent(&f.source) == dir
                && (f.state == MemberState::Trashed
                    || source_presence(disk, proofs, f) == Presence::Absent)
        })
    };
    let mut folders_kept: HashMap<i64, String> = HashMap::new();
    for m in item.members.iter().rev().filter(|m| m.identity.is_none()) {
        let at = match m.destination.as_deref() {
            Some(at) => copy_found(disk, proofs, m, at).flatten(),
            None => None,
        };
        let Some(at) = at else {
            folders_kept.insert(m.ordinal, "its copy isn't as it was made".into());
            continue;
        };
        if source_presence(disk, proofs, m) != Presence::Here {
            continue;
        }
        let timed = match emptied(&m.source) {
            true => Ok(()),
            false => disk.copy_times(&m.source, &at, None),
        };
        if let Err(e) = timed {
            let why = format!("its dates and permissions couldn't be given to its copy: {e}");
            folders_kept.insert(m.ordinal, why);
        } else if let Some(why) = extras_differ(disk, &m.source, &at) {
            folders_kept.insert(m.ordinal, why);
        }
    }
    // Why file `m` must stay for its extras, if it must.
    let extras_differ = |m: &Member| match m.destination.as_deref() {
        Some(at) => extras_differ(disk, &m.source, at),
        None => Some("its copy isn't journalled".into()),
    };
    let whole = source_presence(disk, proofs, top) == Presence::Here
        && folders_kept.is_empty()
        && files.iter().all(|m| {
            m.state == MemberState::Copied
                && copy_here(m)
                && unchanged(m)
                && extras_differ(m).is_none()
        })
        && as_planned(disk, item);
    let kept = |catalog: &mut Catalog, m: &Member, why: &str| {
        catalog
            .db
            .write(|w| journal::set_kept(w, id, m.ordinal, why))
    };
    let mut note = None;
    if whole {
        match trash_original(cx, proofs, &top.source) {
            Ok(receipt) => {
                if let Err(e) = disk.sync_dir(parent(&top.source)) {
                    note = Some(format!(
                        "its folder couldn't be synced: {}: {e}",
                        parent(&top.source).display()
                    ));
                }
                catalog.db.write(|w| {
                    for m in &item.members {
                        let at = rel(item, m).map(|r| receipt.location.join(r));
                        journal::set_member(w, id, m.ordinal, MemberState::Trashed, at.as_deref())?;
                    }
                    Ok(())
                })?;
                if !source_gone(disk, proofs, top) {
                    kept(catalog, top, "the Trash took it, but it is still there")?;
                }
            }
            Err(e) => {
                for m in &item.members {
                    kept(catalog, m, &e.to_string())?;
                }
            }
        }
        return catalog
            .db
            .write(|w| journal::set_state(w, id, ItemState::Done, note.as_deref()));
    }
    for m in files {
        if m.state != MemberState::Copied {
            continue;
        }
        if !copy_here(m) {
            kept(catalog, m, "its copy isn't as it was made")?;
            continue;
        }
        match source_presence(disk, proofs, m) {
            Presence::Here => {}
            Presence::Absent => {
                catalog
                    .db
                    .write(|w| journal::set_member(w, id, m.ordinal, MemberState::Trashed, None))?;
                continue;
            }
            Presence::Unknown => {
                kept(catalog, m, "whether it is still there can't be told")?;
                continue;
            }
        }
        if !unchanged(m) {
            kept(catalog, m, "it changed since it was copied")?;
            continue;
        }
        if let Some(why) = extras_differ(m) {
            kept(catalog, m, &why)?;
            continue;
        }
        let receipt = match trash_original(cx, proofs, &m.source) {
            Ok(receipt) => receipt.location,
            Err(e) => {
                kept(catalog, m, &e.to_string())?;
                continue;
            }
        };
        if let Err(e) = disk.sync_dir(parent(&m.source)) {
            note = Some(format!(
                "its folder couldn't be synced: {}: {e}",
                parent(&m.source).display()
            ));
        }
        catalog.db.write(|w| {
            journal::set_member(w, id, m.ordinal, MemberState::Trashed, Some(&receipt))
        })?;
        if !source_gone(disk, proofs, m) {
            kept(catalog, m, "the Trash took it, but it is still there")?;
        }
    }
    // Folders are never deleted: those still there stay, reported, with
    // what in them wasn't copied, which is only there now.
    let planned: std::collections::HashSet<&Path> =
        item.members.iter().filter_map(|m| rel(item, m)).collect();
    let compose = proofs.originals == Proof::Names;
    let extra: Option<Vec<PathBuf>> = walk(disk, &top.source, None, compose).ok().map(|now| {
        now.into_iter()
            .map(|e| e.rel)
            .filter(|r| !planned.contains(r.as_path()))
            .collect()
    });
    for m in item.members.iter().filter(|m| m.identity.is_none()) {
        if source_presence(disk, proofs, m) != Presence::Here {
            continue;
        }
        let at = rel(item, m).unwrap_or(Path::new(""));
        let why = match &extra {
            _ if folders_kept.contains_key(&m.ordinal) => folders_kept[&m.ordinal].clone(),
            None => "what it holds now can't be read".to_string(),
            Some(extra) => match extra.iter().find(|r| r.starts_with(at)) {
                Some(r) => format!(
                    "it holds {}, which wasn't copied",
                    r.strip_prefix(at).unwrap_or(r).display()
                ),
                None => "the folder stays where it was".to_string(),
            },
        };
        kept(catalog, m, &why)?;
    }
    catalog
        .db
        .write(|w| journal::set_state(w, id, ItemState::Done, note.as_deref()))
}

/// Undoes journalled folder item `id` before the catalog follows it, for
/// `why`: decided first (`restoring`), then its copy discarded; it ends
/// `no_change`, or stays being put back when that can't be done whole.
pub(super) fn undo(
    cx: &Cx<'_>,
    catalog: &mut Catalog,
    id: journal::OperationId,
    why: &str,
) -> Result<()> {
    undo_as(cx, catalog, id, why, Originals::AsCopied)
}

fn undo_as(
    cx: &Cx<'_>,
    catalog: &mut Catalog,
    id: journal::OperationId,
    why: &str,
    rule: Originals,
) -> Result<()> {
    catalog
        .db
        .write(|w| journal::set_state(w, id, ItemState::Restoring, Some(why)))?;
    let item = journal::item(&catalog.db, id)?.context("the journalled item is gone")?;
    let state = match discard(cx, catalog, &item, rule)? {
        None => (ItemState::NoChange, None),
        Some(left) => (ItemState::Restoring, Some(left)),
    };
    catalog
        .db
        .write(|w| journal::set_state(w, id, state.0, state.1))
}

/// Where a folder item's copy is.
enum Built {
    /// Nowhere: never made, or proven gone.
    Gone,
    /// Still in its hidden folder.
    Hidden,
    /// Named, here.
    Named(PathBuf),
    Unknown,
}

/// Where `item`'s copy is: its hidden folder, as its proofs say ([`copy_found`]),
/// at its hidden name or at its destination (named, whether or not that was
/// journalled); or, made but not journalled, at its hidden name while empty
/// ([`unjournalled`]). Built at its name (where the volume can't rename
/// exclusively), it is named once that is journalled.
fn built(disk: &dyn Disk, item: &Item) -> Built {
    let Some(top) = item.members.first() else {
        return Built::Unknown;
    };
    let Some(copy) = &top.copy else {
        return Built::Unknown;
    };
    let proofs = proofs_of(item);
    match top.state {
        // Never made: its making failed, so nothing of it is anywhere.
        MemberState::Discarded => return Built::Gone,
        MemberState::Planned => {
            let since = unjournalled_since(proofs);
            return match unjournalled(disk, &copy.temp, true, false, since) {
                Made::Gone => Built::Gone,
                Made::Here(_) => Built::Hidden,
                Made::Unknown => Built::Unknown,
            };
        }
        _ => {}
    }
    let hidden = copy_found(disk, proofs, top, &copy.temp);
    if proofs.direct {
        return match hidden {
            Some(Some(at)) if top.state == MemberState::Copied => Built::Named(at),
            Some(Some(_)) => Built::Hidden,
            Some(None) => Built::Gone,
            None => Built::Unknown,
        };
    }
    if let Some(Some(_)) = hidden {
        return Built::Hidden;
    }
    let named = top
        .destination
        .as_deref()
        .map(|d| copy_found(disk, proofs, top, d));
    match (hidden, named) {
        (_, Some(Some(Some(at)))) => Built::Named(at),
        (Some(None), Some(Some(None))) => Built::Gone,
        _ => Built::Unknown,
    }
}

/// What an empty file or folder found where one was being made must have
/// been made after to be taken for it, as `proofs` say: where its name isn't
/// unique to the run (made at its name) or names prove it, after the move
/// was planned (#573).
fn unjournalled_since(proofs: Proofs) -> Option<i64> {
    (proofs.direct || proofs.copies == Proof::Names).then_some(proofs.since)
}

/// What was made at a name inside the hidden folder before it was
/// journalled.
enum Made {
    Gone,
    Here(PathBuf),
    Unknown,
}

/// The one name in `temp`'s folder that `sidecar::caseless` takes for
/// `temp`'s, while it is empty (a folder when `dir`, else a file) and, if
/// `since` is given, made after it; gone when the folder has no such name,
/// or, `inside` the hidden folder (which is this run's: nothing else makes
/// a name in it), when that folder isn't there itself.
fn unjournalled(disk: &dyn Disk, temp: &Path, dir: bool, inside: bool, since: Option<i64>) -> Made {
    let (up, Some(name)) = (parent(temp), temp.file_name()) else {
        return Made::Unknown;
    };
    let names = match disk.list(up) {
        Ok(names) => names,
        Err(FsError::NotFound) if inside => return Made::Gone,
        Err(_) => return Made::Unknown,
    };
    let wanted = caseless(name);
    let alike: Vec<&std::ffi::OsString> = names.iter().filter(|n| caseless(n) == wanted).collect();
    let at = match alike.as_slice() {
        [] => return Made::Gone,
        [one] => up.join(one),
        _ => return Made::Unknown,
    };
    let empty = match disk.entry(&at) {
        Ok(e) if dir => {
            e.kind == EntryKind::Directory && disk.list(&at).is_ok_and(|n| n.is_empty())
        }
        Ok(e) => {
            e.kind == EntryKind::File
                && e.links == 1
                && disk.identity(&at).is_ok_and(|i| i.size == 0)
        }
        Err(_) => false,
    };
    let recent = since.is_none_or(|since| proof::born_after(disk, &at, since));
    match empty && recent {
        true => Made::Here(at),
        false => Made::Unknown,
    }
}

/// Discards folder `item`'s copy: still hidden, each member newest first,
/// on its own (a file removed, one written whole only while its own
/// original is proven as copied, else left, or on Put Back (`rule`
/// `Present`) put in the Trash; a folder removed once empty); named, the
/// whole copy to the Trash, only while it holds exactly what was made and,
/// unless on Put Back, every original is proven as copied. Each copy gone
/// is journalled `discarded`. Returns why not, unless all of it is gone.
fn discard(
    cx: &Cx<'_>,
    catalog: &mut Catalog,
    item: &Item,
    rule: Originals,
) -> Result<Option<&'static str>> {
    let disk = cx.disk;
    let proofs = proofs_of(item);
    let proven = originals(disk, item);
    let gone_all = |catalog: &mut Catalog, receipt: Option<&Path>| {
        catalog.db.write(|w| {
            for m in &item.members {
                if m.state != MemberState::Discarded {
                    let at = receipt.and_then(|r| {
                        let hidden = &item.members.first()?.copy.as_ref()?.temp;
                        Some(r.join(m.copy.as_ref()?.temp.strip_prefix(hidden).ok()?))
                    });
                    journal::set_discarded(w, item.id, m.ordinal, at.as_deref())?;
                }
            }
            Ok(())
        })
    };
    match built(disk, item) {
        Built::Gone => {
            gone_all(catalog, None)?;
            Ok(None)
        }
        Built::Unknown => Ok(Some(NOT_BACK)),
        // Built at its name, and part of it undone a copy at a time
        // already (its volume has no Trash): the rest goes so too, as what
        // is left no longer matches what was made. On a share, where
        // nothing goes whole, always so: what the user deleted of it is
        // then found gone, and its folders go once empty.
        Built::Named(at)
            if proofs.direct
                && (proof::removes_nothing(disk, &at)
                    || item
                        .members
                        .iter()
                        .any(|m| m.state == MemberState::Discarded)) =>
        {
            discard_each(cx, catalog, item, rule)
        }
        Built::Named(at) => {
            // Undone on its own only with every original proven as copied;
            // on Put Back, the Trash keeps whatever it holds.
            let ready = proven || rule == Originals::Present;
            if !ready || !as_made(disk, item, &at) {
                return Ok(Some(NOT_BACK));
            }
            // Built at its name on a volume with no Trash (an SMB share),
            // each copy goes on its own, as one still being built.
            let receipt = match cx.trash.trash(&at) {
                Ok(receipt) => receipt,
                Err(TrashError::Unsupported) if proofs.direct => {
                    return discard_each(cx, catalog, item, rule);
                }
                Err(_) => return Ok(Some(NOT_BACK)),
            };
            if disk.sync_dir(parent(&at)).is_err() {
                return Ok(Some(NOT_BACK));
            }
            gone_all(catalog, Some(&receipt.location))?;
            Ok(None)
        }
        Built::Hidden => discard_each(cx, catalog, item, rule),
    }
}

/// Discards each of folder `item`'s copies on its own, newest first: a
/// file removed, one written whole only while its own original is proven
/// as copied, else left, or on Put Back (`rule` `Present`) put in the
/// Trash; a folder removed once empty; on an SMB share no file removed
/// ([`copy::LEFT_ON_SHARE`]). Returns why not, unless all of it is gone:
/// that some are left on a share only when every one left is so, and is
/// safe to delete.
fn discard_each(
    cx: &Cx<'_>,
    catalog: &mut Catalog,
    item: &Item,
    rule: Originals,
) -> Result<Option<&'static str>> {
    let disk = cx.disk;
    let proofs = proofs_of(item);
    // Each copy on its own: a whole one whose original isn't
    // proven as copied may be the one copy of those bytes left, so
    // it stays (Put Back puts it in the Trash), with the folders
    // holding it; what can't be told stays too.
    let mut left = None;
    // What is left on a share for the user to delete, safely.
    let mut deletable = std::collections::HashSet::new();
    for m in item.members.iter().rev() {
        if m.state == MemberState::Discarded {
            continue;
        }
        let Some(copy) = &m.copy else {
            left = left.or(Some(NOT_BACK));
            continue;
        };
        let dir = m.identity.is_none();
        let found = match m.state {
            MemberState::Planned => unjournalled(
                disk,
                &copy.temp,
                dir,
                m.ordinal != 0,
                unjournalled_since(proofs),
            ),
            _ => match copy_found(disk, proofs, m, &copy.temp) {
                Some(Some(at)) => Made::Here(at),
                Some(None) => Made::Gone,
                None => Made::Unknown,
            },
        };
        let mut receipt = None;
        let here = match &found {
            Made::Here(at) => Some(at.clone()),
            _ => None,
        };
        // Why it stays, if it does.
        let mut why = NOT_BACK;
        let done = match found {
            Made::Gone => true,
            Made::Unknown => false,
            // Only while empty: one holding a copy left stays, on a share
            // the user's to delete only while it holds nothing but copies
            // left so (never another computer's file).
            Made::Here(at) if dir => {
                let removed = disk
                    .remove_dir(&at)
                    .and_then(|()| disk.sync_dir(parent(&at)))
                    .is_ok();
                let only_ours = || {
                    disk.list(&at)
                        .is_ok_and(|names| only_deletable(&at, &names, &deletable))
                };
                if !removed && proof::removes_nothing(disk, &at) && only_ours() {
                    why = copy::LEFT_ON_SHARE;
                }
                removed
            }
            Made::Here(at) if copy.content.is_some() && !original_proven(disk, proofs, m) => {
                match rule {
                    Originals::AsCopied => false,
                    Originals::Present => match copy::trash_copy(cx, &at, &m.source) {
                        Ok(r) => {
                            receipt = r;
                            true
                        }
                        Err(left) => {
                            why = left;
                            false
                        }
                    },
                }
            }
            // Built at its name, a file is at a visible name from
            // the start: one a photo of the catalog is named as now
            // (Synchronize Folder may have added it) stays; any not
            // proven empty goes to the Trash.
            Made::Here(at)
                if proofs.direct
                    && occupied(
                        &catalog.db,
                        cx.folders,
                        None,
                        parent(&at),
                        at.file_name().unwrap_or_default(),
                        item.photos(),
                    )? =>
            {
                false
            }
            Made::Here(at) if proofs.direct && !disk.identity(&at).is_ok_and(|i| i.size == 0) => {
                match copy::trash_copy(cx, &at, &m.source) {
                    Ok(r) => {
                        receipt = r;
                        true
                    }
                    Err(left) => {
                        why = left;
                        false
                    }
                }
            }
            // Never removed there; the user's to delete only while its
            // original begins with its bytes (as many as it has).
            Made::Here(at) if proof::removes_nothing(disk, &at) => {
                if proof::begins(disk, &at, &m.source) {
                    why = copy::LEFT_ON_SHARE;
                }
                false
            }
            Made::Here(at) => disk
                .remove_file(&at)
                .and_then(|()| disk.sync_dir(parent(&at)))
                .is_ok(),
        };
        if !done {
            // Left on a share says the user may delete it, so only while
            // nothing else stays, which may be the only copy of its bytes.
            left = match left {
                Some(NOT_BACK) => left,
                _ => Some(why),
            };
            if let (copy::LEFT_ON_SHARE, Some(at)) = (why, here) {
                deletable.insert(proof::composed_path(&at));
            }
            continue;
        }
        catalog
            .db
            .write(|w| journal::set_discarded(w, item.id, m.ordinal, receipt.as_deref()))?;
    }
    Ok(left)
}

/// Whether folder `dir`, listing `names`, holds only what is in
/// `deletable` (paths with their names composed), by its names composed
/// as name proofs take them, each once. An AppleDouble file (`._name`)
/// macOS may keep beside a copy is not among them: what it holds can't be
/// told from another computer's file of that name.
fn only_deletable(
    dir: &Path,
    names: &[std::ffi::OsString],
    deletable: &std::collections::HashSet<PathBuf>,
) -> bool {
    let names: Vec<std::ffi::OsString> = names.iter().map(|n| proof::composed(n)).collect();
    let once = names.iter().collect::<std::collections::HashSet<_>>().len() == names.len();
    once && names.iter().all(|n| deletable.contains(&dir.join(n)))
}

/// Recovery of folder item `item` copied to another volume: one the
/// catalog follows is finished; otherwise it is undone as far as each
/// copy's own original allows, and what is left waits for the user, whose
/// Put Back (`put_back`) puts the copies of originals changed since in the
/// Trash.
pub(super) fn decide(
    cx: &Cx<'_>,
    catalog: &mut Catalog,
    item: &Item,
    put_back: bool,
) -> Result<()> {
    let id = item.id;
    if item.state == ItemState::Committed {
        return finish(cx, catalog, item, true);
    }
    let rule = match put_back {
        true => Originals::Present,
        false => Originals::AsCopied,
    };
    let why = item.error.as_deref().unwrap_or("it was interrupted");
    undo_as(cx, catalog, id, why, rule)
}
