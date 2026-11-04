//! Putting a folder in the Trash (Move to Trash… in the Folders panel,
//! #575): its directory, with everything in it, goes to the system Trash in
//! one call, then the catalog's folders and photos of its subtree (virtual
//! copies included) leave the catalog in one transaction. Its root stays,
//! even with nothing left in the catalog, as Lightroom keeps a parent
//! folder.
//!
//! a. In one transaction: the catalog checked again ([`TrashTreeState::
//!    check`]) and the item journalled with every name of the tree as a
//!    member (the folder, its folders breadth first, its files by path), or
//!    `no_change` with the reason.
//! b. The tree walked again and compared with the journal, name by name
//!    ([`as_planned`]): any difference, and nothing moves. Changes to
//!    `.DS_Store`, `Thumbs.db` and `desktop.ini` don't count; they go with
//!    the folder.
//! c. One Trash call for the folder. A failure leaves it `no_change` only
//!    while every member is at its source; a Trash that took part of it is
//!    left for the user.
//! d. The folder's parent synced, then every member journalled `trashed`
//!    with its receipt. A failed sync puts it back from the receipt.
//! e. The receipt read ([`ReceiptSeen`]) before, and in one transaction
//!    the catalog checked again, the folder proven gone from its path and
//!    in the Trash ([`TreeGone`]), the rows removed and the item `done`,
//!    with a note naming what the Trash also took that was added after the
//!    check of b.
//!
//! Refused when planned (and checked again when run and recovered): a
//! root, a folder with a location of its own in it, anything else of the
//! catalog inside it on disk, the catalog or RAWmakase's own folders, a
//! link, mount point or hard link in it, a read-only drive, a network share
//! (macOS has no Trash there), a drive whose files RAWmakase can't tell
//! apart, a drive whose Trash surely refuses it, and an unfinished
//! operation involving it. Recovery ([`decide`]) never acts on absence
//! alone.
use super::super::db::{Reads, Write};
use super::super::folder_sync::digest::{placement, user_state};
use super::super::ingest;
use super::super::locations::{logical_anchored, logical_location, names};
use super::super::sidecar::caseless;
use super::super::{Catalog, FolderId, PhotoId, RootId};
use super::disk::{Disk, Presence, gone, parent, presence, spelled};
use super::journal::{self, Item, ItemState, Kind, Member, MemberState, NewTrashMember, Subject};
use super::preconditions::{CHANGED, Digest, RELOCATED};
use super::proof::{self, Proof};
use super::recover::NOT_BACK;
use super::tree::{UNFINISHED, holds_own, inside, known, located, overlapped, plain};
use super::tree_copy::{Entry, walk};
use super::{
    BatchId, FileOpError, Plan, Summary, Trash, TrashError, TrashReceipt, TrashSupport, TreeCounts,
};
use crate::fsops::{EntryKind, FileId, VolumeKind};
use anyhow::{Context, Result};
use rawmakase_cancel::Cancel;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// What a folder Trash item journals (as its `preconditions`). Every field
/// added later must default when absent, so this release's journal reads
/// in later ones.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::catalog) struct TrashTreeState {
    pub root: RootId,
    /// The folder's logical path, and where it resolved on this computer.
    pub from: String,
    pub from_path: PathBuf,
    /// The catalog's folders of the subtree (the folder, if it has one of
    /// its own, and every one below it), by logical path, sorted.
    pub folders: Vec<(FolderId, String)>,
    /// Every row it removes, sorted: the photos in those folders and their
    /// masters' virtual copies wherever they are.
    pub photos: Vec<PhotoId>,
    /// Each family's digests when planned (master, placement, user state):
    /// of every master in the subtree, and of the master elsewhere of every
    /// virtual copy in it, so an edit saved meanwhile is never removed
    /// unseen.
    pub families: Vec<(PhotoId, Digest, Digest)>,
    /// How the tree's names are proven: by file ids, or on a local FAT or
    /// exFAT drive by names and `Identity`.
    #[serde(default)]
    pub proof: Proof,
    /// When it was planned, in seconds since 1970.
    #[serde(default)]
    pub since: i64,
}

/// A folder Trash planned.
#[derive(Clone, Debug)]
pub(super) struct TrashTree {
    pub state: TrashTreeState,
    /// The folder below which its root resolves, and the folder's
    /// directory below it as `tree::plain` gave it: checked again before
    /// the Trash.
    anchor: PathBuf,
    canonical: PathBuf,
    /// Its directory as it is used: on FAT and exFAT composed, where that
    /// finds it.
    pub source: PathBuf,
    /// Its directory's file object when planned.
    id: FileId,
    /// The walk: the folder, its folders breadth first, its files by path.
    entries: Vec<Entry>,
}

/// Why a root is refused.
const ROOT: &str = "A root folder can't be moved to the Trash in RAWmakase. Remove it from the catalog, then put it in the Trash in Finder.";
/// Why a subtree with a location of its own is refused.
const LOCATED: &str = "A folder in it is located on its own (on this or another computer), which RAWmakase can't move to the Trash with it yet.";
/// Why the catalog's or RAWmakase's own folders are refused.
const OWN: &str =
    "Folders holding the catalog or RAWmakase's own folders can't be moved to the Trash.";
/// Why a folder in a Trash is refused.
const PROTECTED: &str =
    "Folders in the Trash or in RAWmakase's own folders can't be moved to the Trash.";
/// Why a drive whose files RAWmakase can't tell apart is refused.
const UNPROVEN: &str = "Moving folders on this kind of drive (network and others) to the Trash isn't available yet. Put it in the Trash in Finder, then remove it from the catalog.";
/// Why a network share is refused: macOS has no Trash on one (measured on
/// SMB, #575), and no other system's is relied on.
const NETWORK: &str = "Folders on a network drive can't be moved to the Trash: it has no Trash there. Put it in the Trash in Finder, then remove it from the catalog.";
/// Why a family changed since it was planned.
const PHOTO_CHANGED: &str = "a photo in it changed since you were asked";
/// Why an item whose receipt was never journalled is left for the user.
pub(super) const MAY_BE_IN_TRASH: &str = "it may be in the Trash";
/// Files the system writes on its own, whose changes don't stop a run.
const SYSTEM_FILES: [&str; 3] = [".ds_store", "thumbs.db", "desktop.ini"];

/// Whether `rel`, a file of the tree, is one the system writes on its own.
fn exempt(rel: &Path) -> bool {
    rel.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|n| SYSTEM_FILES.contains(&n.to_ascii_lowercase().as_str()))
}

/// Seconds since 1970.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The rel path `rel` as people read it.
fn shown(rel: &Path) -> String {
    rel.to_string_lossy().replace('\\', "/")
}

/// Plans putting the folder at logical path `folder` of `root` in `trash`,
/// on this computer, as run `batch`.
pub(super) fn plan(
    catalog: &Catalog,
    disk: &dyn Disk,
    trash: &dyn Trash,
    root: RootId,
    folder: &str,
    batch: BatchId,
) -> Result<Plan, FileOpError> {
    let refuse = |why: &str| FileOpError::Destination(why.to_string());
    if folder.is_empty() {
        return Err(refuse(ROOT));
    }
    known(catalog, root, folder)?;
    let computer = &catalog.computer.id;
    let db = &catalog.db;
    let (anchor, from_path) = logical_anchored(db, computer, root, folder)?
        .ok_or_else(|| refuse("The folder is offline or missing."))?;
    let canonical = plain(&anchor, &from_path)?;
    let found = disk.entry(&canonical).map_err(|e| refuse(&e.to_string()))?;
    if found.kind != EntryKind::Directory {
        return Err(refuse("The folder is not a folder on disk."));
    }
    let listed = spelled(disk, &canonical, found.id).map_err(|e| refuse(&e.to_string()))?;
    let facts = disk
        .volume_facts(&listed)
        .map_err(|e| refuse(&e.to_string()))?;
    // On FAT and exFAT, whose drivers list names decomposed but trash a
    // folder only as its name is stored: composed where that finds it, as
    // `tree::plan_to` does (#573).
    let fat = matches!(facts.kind, VolumeKind::Fat | VolumeKind::Exfat);
    let source = match (fat, listed.file_name()) {
        (true, Some(name)) => parent(&listed).join(proof::usable(disk, parent(&listed), name)),
        _ => listed.clone(),
    };
    let spellings = [&listed, &source];
    if spellings.iter().any(|s| super::plan::protected(s)) {
        return Err(refuse(PROTECTED));
    }
    if spellings.iter().any(|s| holds_own(catalog, s)) {
        return Err(refuse(OWN));
    }
    if facts.read_only {
        return Err(refuse("The folder's volume is read-only."));
    }
    // Told by the drive's kind, never by the Trash's own probe, which can't
    // say no on macOS (#587).
    if !facts.local {
        return Err(refuse(NETWORK));
    }
    let proof = proof::of(&facts).ok_or_else(|| refuse(UNPROVEN))?;
    if trash.supports(&source) == TrashSupport::No {
        let name = names(folder).last().unwrap_or_default();
        return Err(FileOpError::Destination(format!(
            "“{name}” is on a drive without a Trash. RAWmakase never deletes files outright; delete it in Finder if you're sure."
        )));
    }
    let (state, rows, computers) = db.snapshot(|s| {
        let folders = super::tree::subtree(s, root, folder)?;
        let photos = ingest::folder_rows(s, &ids(&folders))?;
        let rows = rows(s, &photos)?;
        let families = families(s, &rows)?;
        let computers: i64 = s.read_one(
            super::super::db::sql!("SELECT count(*) FROM computers"),
            &[],
        )?;
        let state = TrashTreeState {
            root,
            from: folder.to_string(),
            from_path: from_path.clone(),
            folders,
            photos,
            families,
            proof,
            since: now(),
        };
        Ok((state, rows, computers))
    })?;
    if let Some(why) = state.check(db, computer)? {
        return Err(FileOpError::Destination(why));
    }
    if let Some(why) = overlapped(
        db,
        computer,
        &ids(&state.folders),
        None,
        &[&listed, &source],
    )? {
        return Err(refuse(why));
    }
    if unfinished(db, &state, &source)? {
        return Err(refuse(UNFINISHED));
    }
    // The names last, once nothing else refuses it.
    let entries =
        walk(disk, &source, None, proof == Proof::Names).map_err(|e| refuse(&walked(e)))?;
    let planned = TrashTree {
        state,
        anchor,
        canonical,
        source,
        id: found.id,
        entries,
    };
    let counts = counts(&planned, &rows);
    let summary = Summary {
        photos: rows.len(),
        copies: rows.iter().filter(|r| r.master.is_some()).count(),
        edited: rows.iter().filter(|r| r.edited).count(),
        files: counts.photos,
        other_computers: computers > 1,
        folder: names(folder).last().map(String::from),
        tree: Some(counts),
        ..Summary::default()
    };
    Ok(Plan {
        batch,
        location: catalog.location.clone(),
        computer: computer.clone(),
        kind: Kind::TrashFolder,
        create: None,
        tree: None,
        trash_tree: Some(planned),
        items: Vec::new(),
        skipped: Vec::new(),
        summary,
    })
}

/// The ids of `folders`.
fn ids(folders: &[(FolderId, String)]) -> Vec<FolderId> {
    folders.iter().map(|(id, _)| *id).collect()
}

/// Why the walk refused the tree, as the folder Trash says it.
fn walked(why: String) -> String {
    // `walk` speaks of copying to another drive where two names differ
    // only in case or accents (a follow-up drops that check for the Trash).
    match why.strip_suffix(" would be one name on the other drive.") {
        Some(pair) => format!(
            "{pair} differ only in case or accents, which RAWmakase can't move to the Trash yet."
        ),
        None => format!("{why} The folder can't be moved to the Trash."),
    }
}

/// A row the folder Trash removes, as planned.
struct Row {
    id: PhotoId,
    folder: FolderId,
    filename: String,
    master: Option<PhotoId>,
    edited: bool,
}

/// The rows `photos`, as `db` has them.
fn rows(db: &impl Reads, photos: &[PhotoId]) -> Result<Vec<Row>> {
    super::super::value::row! {
        struct PhotoRow {
            id: PhotoId,
            folder: FolderId,
            filename: String,
            master: Option<PhotoId>,
            edited: bool,
        }
    }
    let mut out = Vec::new();
    for photo in photos {
        let row: PhotoRow = db.read_one(
            super::super::db::sql!(
                "SELECT id, folder, filename, master_id, recipe IS NOT NULL FROM photos WHERE id=?"
            ),
            &[photo],
        )?;
        out.push(Row {
            id: row.id,
            folder: row.folder,
            filename: row.filename,
            master: row.master,
            edited: row.edited,
        });
    }
    Ok(out)
}

/// The families of `rows`' digests, sorted: each master's among them, and
/// the master elsewhere of each copy among them.
fn families(db: &impl Reads, rows: &[Row]) -> Result<Vec<(PhotoId, Digest, Digest)>> {
    let masters: std::collections::BTreeSet<PhotoId> =
        rows.iter().map(|r| r.master.unwrap_or(r.id)).collect();
    let mut out = Vec::new();
    for master in masters {
        let (Some(place), Some(user)) = (placement(db, master)?, user_state(db, master)?) else {
            continue;
        };
        out.push((master, Digest(place), Digest(user)));
    }
    Ok(out)
}

impl TrashTreeState {
    /// Why the folder can't go to the Trash now, read in `db` on
    /// `computer`: its subtree or its rows changed in the catalog, it
    /// resolves elsewhere, a folder in it has a location of its own,
    /// something else of the catalog is inside it, or a family in it
    /// changed. `None` when everything holds.
    pub(super) fn check(&self, db: &impl Reads, computer: &str) -> Result<Option<String>> {
        if super::tree::subtree(db, self.root, &self.from)? != self.folders {
            return Ok(Some(CHANGED.into()));
        }
        if logical_location(db, computer, self.root, &self.from)?.as_ref() != Some(&self.from_path)
        {
            return Ok(Some(RELOCATED.into()));
        }
        let ids = ids(&self.folders);
        if located(db, self.root, &self.from, &ids)? {
            return Ok(Some(LOCATED.into()));
        }
        if let Some(why) = overlapped(db, computer, &ids, None, &[&self.from_path])? {
            return Ok(Some(why.into()));
        }
        if ingest::folder_rows(db, &ids)? != self.photos {
            return Ok(Some(CHANGED.into()));
        }
        for (master, place, user) in &self.families {
            if placement(db, *master)? != Some(place.0) || user_state(db, *master)? != Some(user.0)
            {
                return Ok(Some(PHOTO_CHANGED.into()));
            }
        }
        Ok(None)
    }
}

/// Whether an unfinished file operation of any computer involves the
/// folder at `source` (planned as `state`): one of its photos, or a file
/// or folder in it.
fn unfinished(db: &impl Reads, state: &TrashTreeState, source: &Path) -> Result<bool> {
    let items = journal::unresolved_anywhere(db)?;
    Ok(items.iter().any(|i| involves(i, state, source)))
}

fn involves(i: &Item, state: &TrashTreeState, source: &Path) -> bool {
    i.photos().iter().any(|p| state.photos.contains(p))
        || i.members.iter().any(|m| {
            [
                Some(m.source.as_path()),
                m.destination.as_deref(),
                m.receipt.as_deref(),
            ]
            .into_iter()
            .flatten()
            .any(|p| inside(p, source) || inside(p, &state.from_path))
        })
}

/// What the question counts, from the walk and the rows.
fn counts(planned: &TrashTree, rows: &[Row]) -> TreeCounts {
    let state = &planned.state;
    // Every name of the walk by its folded path, which tells them apart
    // (the walk refuses two names one folded name takes) and finds the
    // catalog's spelling of it.
    let key = |rel: &Path| {
        rel.components()
            .map(|c| caseless(c.as_os_str()))
            .collect::<Vec<_>>()
            .join("/")
    };
    let by_key: HashMap<String, &Entry> =
        planned.entries.iter().map(|e| (key(&e.rel), e)).collect();
    let rel_of = |logical: &str| -> Option<PathBuf> {
        let rest = logical.strip_prefix(state.from.as_str())?;
        Some(names(rest).collect())
    };
    let folder_rel: HashMap<FolderId, PathBuf> = state
        .folders
        .iter()
        .filter_map(|(id, l)| Some((*id, rel_of(l)?)))
        .collect();
    let missing_folders = state
        .folders
        .iter()
        .filter(|(id, _)| {
            !folder_rel
                .get(id)
                .and_then(|rel| by_key.get(&key(rel)))
                .is_some_and(|e| e.identity.is_none())
        })
        .count();
    // The masters' files of the subtree, by the entry each is.
    let mut photo_files = HashSet::new();
    let mut missing_photos = 0;
    let in_subtree: HashSet<PhotoId> = rows
        .iter()
        .filter(|r| folder_rel.contains_key(&r.folder))
        .map(|r| r.id)
        .collect();
    let mut stray_copies = 0;
    for r in rows {
        match (r.master, folder_rel.get(&r.folder)) {
            (Some(master), Some(_)) if !in_subtree.contains(&master) => stray_copies += 1,
            (Some(_), _) | (None, None) => {}
            (None, Some(rel)) => match by_key.get(&key(&rel.join(&r.filename))) {
                Some(e) if e.identity.is_some() => {
                    photo_files.insert(e.rel.clone());
                }
                _ => missing_photos += 1,
            },
        }
    }
    let files: Vec<&Entry> = planned
        .entries
        .iter()
        .filter(|e| e.identity.is_some())
        .collect();
    let mut other_names: Vec<String> = files
        .iter()
        .filter(|e| !photo_files.contains(&e.rel))
        .filter_map(|e| e.rel.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    other_names.sort();
    let other_files = other_names.len();
    other_names.truncate(3);
    TreeCounts {
        folders: planned.entries.len() - files.len() - 1,
        photos: photo_files.len(),
        other_files,
        other_names,
        missing_photos,
        missing_folders,
        stray_copies,
    }
}

/// What a run needs.
pub(super) struct Cx<'a> {
    pub batch: &'a str,
    pub computer: &'a str,
    pub disk: &'a dyn Disk,
    pub trash: &'a dyn Trash,
}

/// Runs folder Trash `planned`; left alone (`no_change`) when an
/// unfinished operation involves it (`involved`, this computer's). Stops
/// before the Trash once `cancel` is set. An `Err` is a catalog that can't
/// be written: what was journalled stands.
pub(super) fn run(
    cx: &Cx<'_>,
    catalog: &mut Catalog,
    planned: &TrashTree,
    involved: bool,
    cancel: Cancel<'_>,
) -> Result<()> {
    if cancel.is_cancelled() {
        return Ok(());
    }
    let state = &planned.state;
    let members: Vec<NewTrashMember> = planned
        .entries
        .iter()
        .map(|e| NewTrashMember {
            source: join(&planned.source, &e.rel),
            id: e.id,
            identity: e.identity.clone(),
        })
        .collect();
    // a. Checked again and journalled, in one transaction.
    let started = catalog.db.write_immediate(|w| {
        let reason = match state.check(w, cx.computer)? {
            Some(reason) => Some(reason),
            None if involved || unfinished(w, state, &planned.source)? => {
                Some(super::execute::UNFINISHED.into())
            }
            None => None,
        };
        let at = match reason {
            None => ItemState::Planned,
            Some(_) => ItemState::NoChange,
        };
        let id = journal::insert_tree_trash(
            w,
            cx.batch,
            cx.computer,
            at,
            state,
            &members,
            reason.as_deref(),
        )?;
        Ok(reason.is_none().then_some(id))
    })?;
    let Some(id) = started else {
        return Ok(());
    };
    let set = |catalog: &mut Catalog, at: ItemState, why: &str| {
        catalog
            .db
            .write(|w| journal::set_state(w, id, at, Some(why)))
    };
    let item = journal::item(&catalog.db, id)?.context("the journalled item is gone")?;
    // b. Exactly as planned, right before the Trash.
    if cancel.is_cancelled() {
        return set(catalog, ItemState::NoChange, "Stopped.");
    }
    if let Some(why) = changed(cx.disk, planned, &item) {
        return set(catalog, ItemState::NoChange, &why);
    }
    // The walk takes no Stop: asked again once it is done.
    if cancel.is_cancelled() {
        return set(catalog, ItemState::NoChange, "Stopped.");
    }
    // c. One Trash call, by its name as used, or the other spelling of the
    // same folder where the drive trashes only by the one stored (#573).
    let trashed = cx.trash.trash(&planned.source).or_else(|e| {
        match (state.proof, proof::other_spelling(cx.disk, &planned.source)) {
            (Proof::Names, Some(other)) => cx.trash.trash(&other),
            _ => Err(e),
        }
    });
    let receipt = match trashed {
        Ok(receipt) => receipt,
        Err(e) => return failed(cx.disk, catalog, &item, &e),
    };
    // d. Durable, then journalled with every member's receipt.
    let receipts = receipts(&item, &receipt);
    if let Err(e) = cx.disk.sync_dir(parent(&planned.source)) {
        // Journalled first, so a crash in the put back leaves recovery the
        // receipt to put it back from, and never completes it.
        catalog.db.write(|w| {
            journal::set_receipts(w, id, &receipts)?;
            journal::set_state(w, id, ItemState::Restoring, Some(&e.to_string()))
        })?;
        let item = journal::item(&catalog.db, id)?.context("the journalled item is gone")?;
        return put_back(cx.disk, catalog, &item);
    }
    catalog
        .db
        .write(|w| journal::set_receipts(w, id, &receipts))?;
    // e. The receipt read first, then the catalog follows on the evidence.
    let item = journal::item(&catalog.db, id)?.context("the journalled item is gone")?;
    let seen = ReceiptSeen::of(cx.disk, &item);
    let finished = catalog
        .db
        .write_immediate(|w| complete(w, cx.computer, cx.disk, &item, &seen));
    if let Err(e) = finished {
        let error = format!("{e:#}");
        catalog
            .db
            .write(|w| journal::set_state(w, id, ItemState::NeedsRecovery, Some(&error)))
            .map_err(|again| again.context(error))?;
    }
    Ok(())
}

/// `dir` joined with relative path `rel` ("" is `dir` itself).
fn join(dir: &Path, rel: &Path) -> PathBuf {
    match rel.as_os_str().is_empty() {
        true => dir.to_path_buf(),
        false => dir.join(rel),
    }
}

/// Each member of `item` with where `receipt` (the folder's) puts it.
fn receipts(item: &Item, receipt: &TrashReceipt) -> Vec<(i64, PathBuf)> {
    let Some(top) = item.members.first() else {
        return Vec::new();
    };
    item.members
        .iter()
        .filter_map(|m| {
            let rel = m.source.strip_prefix(&top.source).ok()?;
            Some((m.ordinal, join(&receipt.location, rel)))
        })
        .collect()
}

/// Why `planned`'s tree is no longer as journalled in `item`, read right
/// before the Trash: its folder resolves through a link or mount point
/// now, or is another, or a name in it was added, removed or changed.
fn changed(disk: &dyn Disk, planned: &TrashTree, item: &Item) -> Option<String> {
    let moved = format!("{} changed since you were asked.", planned.source.display());
    let same_dir =
        plain(&planned.anchor, &planned.state.from_path).is_ok_and(|p| p == planned.canonical);
    let top = disk.entry(&planned.source);
    let same_top = top.as_ref().is_ok_and(|e| {
        e.kind == EntryKind::Directory
            && e.id.volume() == planned.id.volume()
            && (planned.state.proof == Proof::Names || e.id == planned.id)
    });
    if !same_dir || !same_top {
        return Some(moved);
    }
    match walk(
        disk,
        &planned.source,
        None,
        planned.state.proof == Proof::Names,
    ) {
        Ok(now) => as_planned(planned.state.proof, item, &now),
        Err(why) => Some(format!("{why} It changed since you were asked.")),
    }
}

/// A journalled member's path below the folder.
fn rel<'a>(item: &'a Item, m: &'a Member) -> Option<&'a Path> {
    m.source.strip_prefix(&item.members.first()?.source).ok()
}

/// The first difference between walk `now` and the members `item`
/// journalled, as `proof` compares them; `None` when it holds exactly
/// what was planned, `.DS_Store`, `Thumbs.db` and `desktop.ini` aside.
fn as_planned(proof: Proof, item: &Item, now: &[Entry]) -> Option<String> {
    let planned: BTreeMap<&Path, &Member> = item
        .members
        .iter()
        .filter_map(|m| Some((rel(item, m)?, m)))
        .filter(|(r, m)| !(m.identity.is_some() && exempt(r)))
        .collect();
    let found: BTreeMap<&Path, &Entry> = now
        .iter()
        .map(|e| (e.rel.as_path(), e))
        .filter(|(r, e)| !(e.identity.is_some() && exempt(r)))
        .collect();
    let name = |r: &Path| match r.as_os_str().is_empty() {
        true => "The folder".to_string(),
        false => shown(r),
    };
    for (r, e) in &found {
        let Some(m) = planned.get(r) else {
            return Some(format!("{} was added to it since you were asked.", name(r)));
        };
        let same = m.identity.is_some() == e.identity.is_some()
            && (proof == Proof::Names || m.id == Some(e.id))
            && m.identity == e.identity;
        if !same {
            return Some(format!("{} changed since you were asked.", name(r)));
        }
    }
    planned
        .keys()
        .find(|r| !found.contains_key(*r))
        .map(|r| format!("{} is gone from it since you were asked.", name(r)))
}

/// Where a member is missing from, when not every member is at its source.
enum Missing {
    /// The folder itself, with how it was found.
    Top(Presence),
    /// A name in it, the folder there.
    Member(PathBuf),
}

/// The first member of `item` not at its source, by one walk of the folder
/// (never a listing per member); `None` when every member is there as
/// journalled (by file id on `Ids`, by name, kind and `Identity` on
/// `Names`; files `Identity` alike), extra names allowed, and the system's
/// own files aside.
fn missing(disk: &dyn Disk, proof: Proof, item: &Item) -> Option<Missing> {
    let top = item.members.first()?;
    let at = match (proof, top.id) {
        (Proof::Ids, Some(id)) => presence(disk, &top.source, id),
        (Proof::Ids, None) => Presence::Unknown,
        (Proof::Names, _) => proof::presence(disk, &top.source),
    };
    if at != Presence::Here {
        return Some(Missing::Top(at));
    }
    let Ok(now) = walk(disk, &top.source, None, proof == Proof::Names) else {
        return Some(Missing::Member(top.source.clone()));
    };
    let found: HashMap<&Path, &Entry> = now.iter().map(|e| (e.rel.as_path(), e)).collect();
    item.members
        .iter()
        .filter(|m| rel(item, m).is_some_and(|r| !(m.identity.is_some() && exempt(r))))
        .find(|m| {
            let Some(e) = rel(item, m).and_then(|r| found.get(r)) else {
                return true;
            };
            !(m.identity.is_some() == e.identity.is_some()
                && (proof == Proof::Names || m.id == Some(e.id))
                && m.identity == e.identity)
        })
        .map(|m| Missing::Member(m.source.clone()))
}

/// The journalled state's proof.
fn proof_of(item: &Item) -> Proof {
    match &item.subject {
        Subject::TrashTree(state) => state.proof,
        _ => Proof::Ids,
    }
}

/// After the Trash call failed with `e`: `no_change` while every member is
/// at its source; otherwise the Trash took part of it, left for the user.
fn failed(disk: &dyn Disk, catalog: &mut Catalog, item: &Item, e: &TrashError) -> Result<()> {
    let (at, why) = match missing(disk, proof_of(item), item) {
        None => (ItemState::NoChange, e.to_string()),
        Some(m) => (
            ItemState::NeedsRecovery,
            split(&m, "the Trash failed part way"),
        ),
    };
    catalog
        .db
        .write(|w| journal::set_state(w, item.id, at, Some(&why)))
}

/// Why an item whose members aren't all at their sources, the folder
/// there, is left for the user.
fn split(m: &Missing, how: &str) -> String {
    match m {
        Missing::Member(p) => format!(
            "{how}: not all its files are where they were ({}); some may be in the Trash",
            p.display()
        ),
        Missing::Top(_) => format!("{how}: its folder can't be found as it was"),
    }
}

/// What was read of a folder Trash's receipt, before the transaction that
/// completes it: made only by [`Self::of`], which reads the item's own
/// journalled receipt.
pub(super) struct ReceiptSeen {
    /// The receipt read.
    at: Option<PathBuf>,
    found: Found,
}

enum Found {
    /// The receipt's folder can't be listed (macOS's Trash without Full
    /// Disk Access): no evidence either way, as for a photo's Trash.
    Unlistable,
    /// The receipt's folder lists without it.
    Absent,
    /// Its folder walked.
    Walked(Vec<Entry>),
    /// Listed, but its walk refused or failed.
    Unreadable(String),
}

impl ReceiptSeen {
    pub(super) fn of(disk: &dyn Disk, item: &Item) -> Self {
        let Some(at) = item.members.first().and_then(|m| m.receipt.clone()) else {
            return Self {
                at: None,
                found: Found::Absent,
            };
        };
        let found = match (disk.list(parent(&at)), at.file_name()) {
            // No evidence from a receipt that names nothing.
            (_, None) => Found::Unreadable("the Trash named no item".into()),
            (Err(_), _) => Found::Unlistable,
            (Ok(listed), Some(name)) => {
                let wanted = caseless(name);
                if !listed.iter().any(|n| caseless(n) == wanted) {
                    Found::Absent
                } else {
                    match walk(disk, &at, None, proof_of(item) == Proof::Names) {
                        Ok(entries) => Found::Walked(entries),
                        Err(why) => Found::Unreadable(why),
                    }
                }
            }
        };
        Self {
            at: Some(at),
            found,
        }
    }

    /// The names of the walk that `item` didn't journal: what the Trash
    /// took that was added after the check before it.
    fn extra(&self, item: &Item) -> Option<String> {
        let Found::Walked(entries) = &self.found else {
            return None;
        };
        let planned: HashSet<&Path> = item.members.iter().filter_map(|m| rel(item, m)).collect();
        let mut extra: Vec<&Entry> = entries
            .iter()
            .filter(|e| !planned.contains(e.rel.as_path()))
            .filter(|e| !(e.identity.is_some() && exempt(&e.rel)))
            .collect();
        extra.sort_by(|a, b| a.rel.cmp(&b.rel));
        let first = extra.first()?;
        Some(format!(
            "the Trash also took {}, added after it was checked",
            shown(&first.rel)
        ))
    }
}

/// Proof that a folder Trash's folder is in the Trash and gone from its
/// path: made only by [`Self::of`], and the only thing that lets
/// [`remove_rows`] remove its rows on its own.
pub(super) struct TreeGone(());

impl TreeGone {
    /// The evidence for `item`, from `seen` (read before the transaction)
    /// and the folder's path (read now, inside it); why there is none
    /// otherwise.
    fn of(disk: &dyn Disk, item: &Item, seen: &ReceiptSeen) -> Result<Self, String> {
        let proof = proof_of(item);
        let Some(top) = item.members.first() else {
            return Err("it has no folder".into());
        };
        let (MemberState::Trashed, Some(receipt)) = (top.state, top.receipt.as_deref()) else {
            return Err(MAY_BE_IN_TRASH.into());
        };
        if seen.at.as_deref() != Some(receipt) {
            return Err("its folder in the Trash wasn't read".into());
        }
        let away = match (proof, top.id) {
            (Proof::Ids, Some(id)) => gone(disk, &top.source, id),
            (Proof::Ids, None) => false,
            (Proof::Names, _) => proof::gone(disk, &top.source),
        };
        if !away {
            return Err("its folder can't be proven gone from where it was".into());
        }
        match &seen.found {
            Found::Unlistable => Ok(Self(())),
            Found::Absent => Err("it isn't where the Trash put it".into()),
            Found::Unreadable(why) => Err(format!("its folder in the Trash can't be read: {why}")),
            Found::Walked(entries) => {
                let holds = match proof {
                    Proof::Ids => entries
                        .first()
                        .is_some_and(|e| e.rel.as_os_str().is_empty() && Some(e.id) == top.id),
                    Proof::Names => {
                        let found: HashMap<&Path, &Entry> =
                            entries.iter().map(|e| (e.rel.as_path(), e)).collect();
                        item.members.iter().all(|m| {
                            let Some(r) = rel(item, m) else {
                                return false;
                            };
                            (m.identity.is_some() && exempt(r))
                                || found.get(r).is_some_and(|e| {
                                    e.identity.is_some() == m.identity.is_some()
                                        && e.identity == m.identity
                                })
                        })
                    }
                };
                match holds {
                    true => Ok(Self(())),
                    false => {
                        Err("the folder in the Trash isn't the one that was asked about".into())
                    }
                }
            }
        }
    }
}

/// Why a folder Trash's rows may go.
enum Removal {
    /// Its folder is proven in the Trash and gone from its path.
    Gone(TreeGone),
    /// The user chose Remove from Catalog for it, its receipt lost.
    Requested,
}

/// Removes `state`'s folders and rows in `w`'s transaction; its root stays.
/// Takes why they may go, so they never go without [`TreeGone`] or the
/// user's choice. Only after `state.check` passed in the same transaction,
/// so they are exactly the journalled ones.
fn remove_rows(w: &mut Write<'_>, state: &TrashTreeState, _why: Removal) -> Result<()> {
    let removed = ingest::remove_folder_rows(w, &ids(&state.folders))?;
    anyhow::ensure!(
        removed == state.photos,
        "the folder's photos aren't the ones planned"
    );
    Ok(())
}

/// Finishes journalled folder Trash `item` in `w`'s transaction, if the
/// evidence allows: the catalog as planned, and [`TreeGone`] from `seen`.
/// Marks it `done` (with what the Trash also took), or `needs_recovery`
/// with why not; an `Err` only when the catalog can't be read or written.
pub(super) fn complete(
    w: &mut Write<'_>,
    computer: &str,
    disk: &dyn Disk,
    item: &Item,
    seen: &ReceiptSeen,
) -> Result<()> {
    let Subject::TrashTree(state) = &item.subject else {
        anyhow::bail!("not a folder Trash");
    };
    if let Some(why) = state.check(w, computer)? {
        return journal::set_state(w, item.id, ItemState::NeedsRecovery, Some(&why));
    }
    match TreeGone::of(disk, item, seen) {
        Ok(gone) => {
            remove_rows(w, state, Removal::Gone(gone))?;
            journal::set_done(w, item.id, seen.extra(item).as_deref())
        }
        Err(why) => journal::set_state(w, item.id, ItemState::NeedsRecovery, Some(&why)),
    }
}

/// Puts `item`'s folder back from its receipt, where it is proven the
/// folder trashed: the same file object (`Ids`), or a folder holding every
/// member as journalled (`Names`). `no_change` once it is renamed back to
/// its path (a file in it changed in the moment before the Trash is the
/// user's, not a reason to say it isn't back); otherwise it stays
/// `restoring`.
fn put_back(disk: &dyn Disk, catalog: &mut Catalog, item: &Item) -> Result<()> {
    let id = item.id;
    let proof = proof_of(item);
    let back = item.members.first().is_some_and(|top| {
        let Some(receipt) = top.receipt.as_deref() else {
            return false;
        };
        let proven = match (proof, top.id) {
            (Proof::Ids, Some(file)) => presence(disk, receipt, file) == Presence::Here,
            (Proof::Ids, None) => false,
            (Proof::Names, _) => {
                let seen = ReceiptSeen::of(disk, item);
                matches!(&seen.found, Found::Walked(entries) if holds_all(item, entries))
            }
        };
        proven
            && disk.rename(receipt, &top.source).is_ok()
            && disk.sync_dir(parent(&top.source)).is_ok()
    });
    if back {
        return catalog.db.write(|w| {
            for m in &item.members {
                journal::set_member(w, id, m.ordinal, MemberState::Restored, None)?;
            }
            journal::set_state(w, id, ItemState::NoChange, None)
        });
    }
    catalog
        .db
        .write(|w| journal::set_state(w, id, ItemState::Restoring, Some(NOT_BACK)))
}

/// Whether walk `entries` holds every member of `item` by its name below
/// the folder, its kind and a file's `Identity`; extra names allowed.
fn holds_all(item: &Item, entries: &[Entry]) -> bool {
    let found: HashMap<&Path, &Entry> = entries.iter().map(|e| (e.rel.as_path(), e)).collect();
    item.members.iter().all(|m| {
        rel(item, m).is_some_and(|r| {
            (m.identity.is_some() && exempt(r))
                || found.get(r).is_some_and(|e| {
                    e.identity.is_some() == m.identity.is_some() && e.identity == m.identity
                })
        })
    })
}

/// Decides unresolved folder Trash `item` from the journal and the disk:
/// every member at its source, `no_change`; being put back, only put back;
/// its receipt journalled, completed on the evidence; otherwise left for
/// the user, with why.
pub(super) fn decide(
    catalog: &mut Catalog,
    computer: &str,
    disk: &dyn Disk,
    item: &Item,
) -> Result<()> {
    let id = item.id;
    let proof = proof_of(item);
    // Nothing happened, or the user put it all back: always safe.
    let missing = missing(disk, proof, item);
    if missing.is_none() {
        return catalog
            .db
            .write(|w| journal::set_state(w, id, ItemState::NoChange, None));
    }
    // Only ever back; the rows are untouched, whatever the catalog says.
    // Its folder's own file object at its path (it went to the Trash whole)
    // is back; where only names prove it (FAT, exFAT), another folder of its
    // name may be there, and only putting it back from its receipt counts.
    if item.state == ItemState::Restoring {
        return match missing {
            Some(Missing::Member(_)) if proof == Proof::Ids => catalog
                .db
                .write(|w| journal::set_state(w, id, ItemState::NoChange, None)),
            _ => put_back(disk, catalog, item),
        };
    }
    let unfinished = |catalog: &mut Catalog, why: &str| {
        catalog
            .db
            .write(|w| journal::set_state(w, id, ItemState::NeedsRecovery, Some(why)))
    };
    let top = item.members.first();
    if top.is_some_and(|t| t.state == MemberState::Trashed && t.receipt.is_some()) {
        // The receipt read before the transaction, never in it.
        let seen = ReceiptSeen::of(disk, item);
        return catalog
            .db
            .write_immediate(|w| complete(w, computer, disk, item, &seen));
    }
    match missing {
        Some(Missing::Top(Presence::Absent)) => unfinished(catalog, MAY_BE_IN_TRASH),
        Some(Missing::Top(_)) => unfinished(catalog, "its folder can't be found as it was"),
        Some(m @ Missing::Member(_)) => unfinished(catalog, &split(&m, "unfinished")),
        None => Ok(()),
    }
}

/// Removes folder Trash `item`'s rows as the user chose (Remove from
/// Catalog), its receipt lost and its folder gone from its path: only
/// while the catalog is as planned and no other unfinished operation
/// involves its photos. Marks it `done`; why not otherwise.
pub(super) fn remove_from_catalog(
    catalog: &mut Catalog,
    computer: &str,
    disk: &dyn Disk,
    item: &Item,
) -> Result<Result<(), String>> {
    let Subject::TrashTree(state) = &item.subject else {
        return Ok(Err(
            "Only a folder put in the Trash can be removed so.".into()
        ));
    };
    let Some(top) = item.members.first() else {
        return Ok(Err("It has no folder.".into()));
    };
    if top.receipt.is_some() {
        return Ok(Err(
            "Its folder is in the Trash where it was put: put it back, or keep it as it is.".into(),
        ));
    }
    let away = match (state.proof, top.id) {
        (Proof::Ids, Some(id)) => gone(disk, &top.source, id),
        (Proof::Ids, None) => false,
        (Proof::Names, _) => proof::gone(disk, &top.source),
    };
    if !away {
        return Ok(Err(
            "Its folder is still where it was, or can't be read. Keep it as it is.".into(),
        ));
    }
    catalog.db.write_immediate(|w| {
        if let Some(why) = state.check(w, computer)? {
            return Ok(Err(why));
        }
        let others = journal::unresolved_anywhere(w)?
            .iter()
            .any(|i| i.id != item.id && i.photos().iter().any(|p| state.photos.contains(p)));
        if others {
            return Ok(Err(UNFINISHED.into()));
        }
        remove_rows(w, state, Removal::Requested)?;
        journal::set_done(w, item.id, None)?;
        Ok(Ok(()))
    })
}

/// What a done folder Trash came to: in the Trash (its receipt journalled),
/// or removed from the catalog by the user's choice.
pub(super) fn trashed(item: &Item) -> bool {
    item.members.first().is_some_and(|m| m.receipt.is_some())
}

/// Whether `item` is a folder Trash, and its choices when unfinished.
pub(super) fn choices(item: &Item) -> Vec<super::Resolution> {
    use super::Resolution;
    match item.members.first() {
        Some(top) if top.receipt.is_some() => vec![Resolution::PutBack, Resolution::Keep],
        _ if item.error.as_deref() == Some(MAY_BE_IN_TRASH) => {
            vec![Resolution::Keep, Resolution::RemoveFromCatalog]
        }
        _ => vec![Resolution::Keep],
    }
}
