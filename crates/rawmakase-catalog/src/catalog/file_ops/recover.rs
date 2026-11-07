//! Recovery: what a crash, a lost thread or a failed step left unfinished,
//! decided item by item from the journal and the disk, on open and before
//! any file operation, under the operation lock. Only this computer's items
//! are looked at. Where a member is is decided by its exact name in its
//! folder's listing and its file object, never by looking its name up, and
//! the disk decides over anything the journal says. Recovery creates,
//! deletes and renames nothing, except putting a member back: an exclusive
//! rename of a member proven at its destination (or in the Trash) to its
//! source; and, for a move to another volume ([`super::copy`]), removing a
//! partial copy it made and putting copies and originals in the Trash, as
//! below. Every decision is journalled before any disk step.
//!
//! 0. An item being put back (`restoring`) is only ever put back, and stays
//!    `restoring` until it is (or the user keeps it as it is): its
//!    preconditions first (a mismatch, such as an older release having
//!    synchronized the photo meanwhile, leaves it unfinished, untouched);
//!    then members at their destinations go back, and once every member
//!    is at its source it is `no_change`.
//! 1. An item whose family changed in the catalog meanwhile, or whose file
//!    another photo is now, is left unfinished, untouched.
//! 2. Every member at its source: `no_change` (the user may have put a
//!    file back from the Trash).
//! 3. A move with every member at its destination and gone from its
//!    source: its rows follow, `done`.
//! 4. A trash with every member trashed with a receipt and gone from its
//!    source: its rows go, `done`. Trashed without a receipt: unfinished
//!    ("it may be in the Trash").
//! 5. A folder made: proven there (its file object, by the name the disk
//!    gave it), it is added to the catalog when the user asks; never made,
//!    or gone, nothing changed. A folder there whose making wasn't
//!    journalled is never adopted, and no folder is ever deleted.
//! 6. Anything else (an offline volume, an error, another file object,
//!    members split): unfinished, for the user to Put Back or Keep as Is.
//! 7. A move to another volume the catalog follows (`committed`) is
//!    finished: each original still the file it copied, whose copy is
//!    where the catalog says and reads back the same, goes to the Trash;
//!    one that can't stays, and is reported. One the catalog doesn't follow
//!    is undone once every original is proven at its source, as it was
//!    copied: its partial copies are removed, its complete copies (proven
//!    by their file object and read back the same) go to the Trash, and it
//!    is `no_change`. An original that can't be proven leaves it for the
//!    user: Put Back undoes it once each original is the file object it
//!    was at its source, changed since or not (the user chose to keep the
//!    originals), and refuses while one isn't; Keep as Is leaves the
//!    copies.
//! 8. Nothing is ever cleaned up because its name looks like ours: a
//!    partial copy is removed by its journalled file object, or, made but
//!    not yet journalled, only at the name unique to its run and member
//!    that the journal gave it, and only while empty; one written whole,
//!    only while its original is proven at its source as it was copied
//!    (on Put Back it goes to the Trash, as a complete copy). A name held by
//!    another file object than the journalled one is never taken for the
//!    copy gone. Nothing is removed from an SMB share: a copy there stays
//!    for the user (#675).
//! 9. Finished items are forgotten after 30 days.
use super::super::folder_sync::locate_folders;
use super::super::{Catalog, PhotoId};
use super::disk::{Disk, Presence, Real, parent, presence};
use super::execute::{complete, restore};
use super::folder::{self, FolderState};
use super::journal::{self, Item, ItemState, Kind, Member, MemberState, OperationId, Subject};
use super::plan::{Folders, shared};
use super::{FileOpError, ItemOutcome, ItemResult, KeptOriginal, Trash};
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Why an item being put back is still unfinished.
pub(super) const NOT_BACK: &str = "its files could not all be put back";

/// What recovery left for the user.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Recovery {
    /// Items it decided, finished or put back.
    pub finished: Vec<ItemOutcome>,
    /// Items it can't decide, for "Unfinished file operations".
    pub unfinished: Vec<Unfinished>,
}

/// An item recovery can't decide.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unfinished {
    pub operation: OperationId,
    /// The catalog photos it concerns, whose rows are as they were.
    pub photos: Vec<PhotoId>,
    pub members: Vec<UnfinishedMember>,
    pub reason: String,
    /// What the user may decide for it, the first the one suggested.
    pub choices: Vec<Resolution>,
}

/// One file of an unfinished item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnfinishedMember {
    /// Where it was.
    pub source: PathBuf,
    /// Where its step put it, or would have: its destination, or where the
    /// Trash put it.
    pub after: Option<PathBuf>,
    /// Whether its step was journalled as done.
    pub changed: bool,
}

/// What the user decided for an unfinished item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// Rename its files back to where they were, where they are proven.
    PutBack,
    /// Leave it as it is, for good; Synchronize Folder can match the files.
    Keep,
    /// Add the folder it made to the catalog, where it is proven the one
    /// made.
    AddFolder,
    /// Take the photos and folders of a folder put in the Trash out of the
    /// catalog, where its receipt was lost and it is gone from its path.
    RemoveFromCatalog,
}

/// Recovers `catalog`'s unfinished file operations on this computer, and
/// forgets finished ones older than 30 days. A move to another drive puts
/// files in `trash`.
pub fn recover(catalog: &mut Catalog, trash: &dyn Trash) -> Result<Recovery, FileOpError> {
    let _lock = super::lock::acquire(catalog)?;
    prune(catalog)?;
    Ok(run(catalog, &Real, trash)?)
}

/// Whether `catalog` has file operations of this computer that a crash
/// left undecided, which [`recover`] is for; read without the lock, so
/// opening a catalog needn't hold it when there are none.
pub fn unfinished_here(catalog: &Catalog) -> anyhow::Result<bool> {
    Ok(!journal::unresolved(&catalog.db, &catalog.computer.id)?.is_empty())
}

/// Forgets finished items older than 30 days; the lock held.
pub(super) fn prune(catalog: &mut Catalog) -> Result<()> {
    let before = crate::time::utc_text(now() - 30 * 24 * 60 * 60);
    catalog.db.write(|w| journal::prune(w, &before))?;
    Ok(())
}

/// Seconds since 1970.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Applies the rules to every unresolved item, the lock held.
pub(super) fn run(catalog: &mut Catalog, disk: &dyn Disk, trash: &dyn Trash) -> Result<Recovery> {
    let computer = catalog.computer.id.clone();
    let items = journal::unresolved(&catalog.db, &computer)?;
    let mut recovery = Recovery::default();
    if items.is_empty() {
        return Ok(recovery);
    }
    let folders = Folders::index(&catalog.db.snapshot(|s| locate_folders(s, &computer))?);
    for item in items {
        decide(catalog, &computer, &folders, disk, trash, &item, false)?;
        let now = journal::item(&catalog.db, item.id)?.context("the journalled item is gone")?;
        match now.state {
            ItemState::NeedsRecovery | ItemState::Restoring => {
                recovery.unfinished.push(unfinished(&now))
            }
            _ => recovery.finished.push(outcome_of(&now)),
        }
    }
    Ok(recovery)
}

/// Decides unfinished item `operation` as the user chose. Putting back a
/// move to another drive puts its copies in `trash`.
pub fn resolve(
    catalog: &mut Catalog,
    operation: OperationId,
    resolution: Resolution,
    trash: &dyn Trash,
) -> Result<ItemOutcome, FileOpError> {
    resolve_with(catalog, operation, resolution, &Real, trash)
}

/// [`resolve`] on `disk`.
pub(super) fn resolve_with(
    catalog: &mut Catalog,
    operation: OperationId,
    resolution: Resolution,
    disk: &dyn Disk,
    trash: &dyn Trash,
) -> Result<ItemOutcome, FileOpError> {
    let _lock = super::lock::acquire(catalog)?;
    let computer = catalog.computer.id.clone();
    let item = journal::item(&catalog.db, operation)?.context("No such file operation")?;
    if item.computer != computer {
        return Err(FileOpError::OtherComputer);
    }
    let folder = item.kind == Kind::CreateFolder;
    let trash_tree = matches!(item.subject, Subject::TrashTree(_));
    match resolution {
        Resolution::PutBack
            if trash_tree && item.members.first().is_none_or(|m| m.receipt.is_none()) =>
        {
            return Err(FileOpError::Unsupported(
                "It may be in the Trash; put it back in Finder.",
            ));
        }
        Resolution::RemoveFromCatalog if !trash_tree => {
            return Err(FileOpError::Unsupported(
                "Only a folder put in the Trash can be removed from the catalog so.",
            ));
        }
        Resolution::PutBack if folder => {
            return Err(FileOpError::Unsupported(
                "A folder that was made is never deleted. Keep it, or add it to the catalog.",
            ));
        }
        Resolution::AddFolder if !folder => {
            return Err(FileOpError::Unsupported(
                "Only a folder that was made can be added.",
            ));
        }
        Resolution::AddFolder if item.members.first().and_then(|m| m.id).is_none() => {
            return Err(FileOpError::Unsupported(folder::NOT_PROVEN));
        }
        _ => {}
    }
    if matches!(item.state, ItemState::NeedsRecovery | ItemState::Restoring) {
        let state = match resolution {
            Resolution::Keep => ItemState::Kept,
            Resolution::PutBack => ItemState::Restoring,
            Resolution::AddFolder => {
                catalog
                    .db
                    .write_immediate(|w| folder::complete(w, &computer, disk, &item))?;
                let now =
                    journal::item(&catalog.db, operation)?.context("No such file operation")?;
                return Ok(outcome_of(&now));
            }
            Resolution::RemoveFromCatalog => {
                if let Err(why) =
                    super::tree_trash::remove_from_catalog(catalog, &computer, disk, &item)?
                {
                    return Err(FileOpError::Destination(why));
                }
                let now =
                    journal::item(&catalog.db, operation)?.context("No such file operation")?;
                return Ok(outcome_of(&now));
            }
        };
        catalog
            .db
            .write(|w| journal::set_state(w, operation, state, None))?;
        if state == ItemState::Restoring {
            let item = journal::item(&catalog.db, operation)?.context("No such file operation")?;
            let folders = Folders::index(&catalog.db.snapshot(|s| locate_folders(s, &computer))?);
            decide(catalog, &computer, &folders, disk, trash, &item, true)?;
        }
    }
    let now = journal::item(&catalog.db, operation)?.context("No such file operation")?;
    Ok(outcome_of(&now))
}

/// Applies the rules to one unresolved `item`.
fn decide(
    catalog: &mut Catalog,
    computer: &str,
    folders: &Folders,
    disk: &dyn Disk,
    trash: &dyn Trash,
    item: &Item,
    put_back: bool,
) -> Result<()> {
    let id = item.id;
    if let Subject::TrashTree(_) = item.subject {
        return super::tree_trash::decide(catalog, computer, disk, item);
    }
    if item.kind == Kind::CreateFolder {
        return catalog
            .db
            .write_immediate(|w| folder::decide(w, computer, disk, item));
    }
    // Rule 7, before anything that reads members as renamed.
    if item.copies() {
        let cx = super::copy::Cx {
            computer,
            folders,
            disk,
            trash,
        };
        return match item.subject {
            Subject::Tree(_) => super::tree_copy::decide(&cx, catalog, item, put_back),
            _ => super::copy::decide(&cx, catalog, item, put_back),
        };
    }
    // An item being put back stays so however it is left: it is never
    // completed forward, whatever happens to its files later.
    let left = match item.state {
        ItemState::Restoring => ItemState::Restoring,
        _ => ItemState::NeedsRecovery,
    };
    let unfinished = |catalog: &mut Catalog, reason: &str| {
        catalog
            .db
            .write(|w| journal::set_state(w, id, left, Some(reason)))
    };
    // Rules 0 and 1: the catalog first, before any disk step.
    if let Some(reason) = changed(catalog, computer, folders, item)? {
        return unfinished(catalog, &reason);
    }
    // A rename by a placeholder (ntfs-3g, #914) a crash cut short may have
    // left one at a member's destination, or, putting it back, at its
    // source: it is removed first, so the rules below see only the files.
    if matches!(item.kind, Kind::MovePhotos | Kind::RenamePhotos) {
        for m in &item.members {
            let Some(after) = m.destination.as_deref() else {
                continue;
            };
            let settled = disk
                .settle_rename(&m.source, after)
                .and_then(|()| disk.settle_rename(after, &m.source));
            if settled.is_err() {
                return unfinished(catalog, "its files are split or unreachable");
            }
        }
    }
    // Rule 2, whatever the states say.
    if at_sources(disk, &item.members) {
        return catalog
            .db
            .write(|w| journal::set_state(w, id, ItemState::NoChange, None));
    }
    if item.state == ItemState::Restoring {
        // Rule 0: only ever back.
        let mut back = Vec::new();
        for m in &item.members {
            match where_is(disk, m) {
                Where::Source => {}
                Where::After => back.push(m.clone()),
                Where::Unknown => return unfinished(catalog, "its files are split or unreachable"),
            }
        }
        if restore(catalog, disk, id, &back)? && at_sources(disk, &item.members) {
            return catalog
                .db
                .write(|w| journal::set_state(w, id, ItemState::NoChange, None));
        }
        return unfinished(catalog, NOT_BACK);
    }
    match item.kind {
        // Rule 3.
        Kind::MovePhotos
        | Kind::RenamePhotos
        | Kind::RenameFolder
        | Kind::MoveFolder
        | Kind::MoveRoot
            if item
                .members
                .iter()
                .all(|m| where_is(disk, m) == Where::After) =>
        {
            catalog
                .db
                .write_immediate(|w| complete(w, computer, folders, disk, item))
        }
        // Rule 4.
        Kind::TrashPhotos if item.members.iter().all(|m| m.state == MemberState::Trashed) => {
            catalog
                .db
                .write_immediate(|w| complete(w, computer, folders, disk, item))
        }
        Kind::TrashPhotos => unfinished(catalog, "it may be in the Trash"),
        // Rule 6.
        _ => unfinished(catalog, "its files are not all where it left them"),
    }
}

/// Why `item` can't be acted on: its family changed in the catalog, a
/// folder of it resolves elsewhere, or another photo is its file now.
fn changed(
    catalog: &Catalog,
    computer: &str,
    folders: &Folders,
    item: &Item,
) -> Result<Option<String>> {
    let pre = match &item.subject {
        Subject::Photos(pre) => pre,
        Subject::Tree(tree) => return tree.check(&catalog.db, computer),
        Subject::TrashTree(tree) => return tree.check(&catalog.db, computer),
        Subject::Folder(_) => return Ok(None),
    };
    if let Some(reason) = pre.check(&catalog.db, computer)? {
        return Ok(Some(reason.into()));
    }
    if let Some(photo) = item.members.first() {
        let filename = photo.source.file_name().unwrap_or_default();
        if shared(
            &catalog.db,
            folders,
            parent(&photo.source),
            filename,
            photo.id,
            &pre.rows,
        )? {
            return Ok(Some(
                "another photo in the catalog is the same file now".into(),
            ));
        }
    }
    Ok(None)
}

/// Where a member is now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Where {
    /// At its source, whatever else.
    Source,
    /// Where its step puts it (its destination, or where the Trash put it)
    /// and gone from its source.
    After,
    Unknown,
}

fn where_is(disk: &dyn Disk, m: &Member) -> Where {
    let Some(id) = m.id else {
        return Where::Unknown;
    };
    if presence(disk, &m.source, id) == Presence::Here {
        return Where::Source;
    }
    match m.after() {
        Some(at)
            if presence(disk, at, id) == Presence::Here
                && presence(disk, &m.source, id) == Presence::Absent =>
        {
            Where::After
        }
        _ => Where::Unknown,
    }
}

/// Whether every member is at its source.
fn at_sources(disk: &dyn Disk, members: &[Member]) -> bool {
    members.iter().all(|m| where_is(disk, m) == Where::Source)
}

fn unfinished(item: &Item) -> Unfinished {
    // A folder copied to another volume is in its hidden folder until that
    // is named.
    let hidden = matches!(item.subject, Subject::Tree(_))
        && item
            .members
            .first()
            .is_some_and(|m| m.copy.is_some() && m.state != MemberState::Copied);
    Unfinished {
        operation: item.id,
        photos: item.photos().to_vec(),
        members: item
            .members
            .iter()
            .map(|m| UnfinishedMember {
                source: m.source.clone(),
                // A copy not yet given its name is its hidden partial copy.
                after: match &m.copy {
                    Some(copy)
                        if hidden
                            || matches!(m.state, MemberState::Planned | MemberState::Copying) =>
                    {
                        Some(copy.temp.clone())
                    }
                    _ => m.after().map(PathBuf::from),
                },
                changed: m.state != MemberState::Planned,
            })
            .collect(),
        reason: item.error.clone().unwrap_or_default(),
        choices: match &item.subject {
            // A folder with its file object journalled may be the one made.
            Subject::Folder(_) if item.members.first().is_some_and(|m| m.id.is_some()) => {
                vec![Resolution::AddFolder, Resolution::Keep]
            }
            Subject::Folder(_) => vec![Resolution::Keep],
            Subject::TrashTree(_) => super::tree_trash::choices(item),
            Subject::Photos(_) | Subject::Tree(_) => vec![Resolution::PutBack, Resolution::Keep],
        },
    }
}

/// What journalled `item` came to.
pub(super) fn outcome_of(item: &Item) -> ItemOutcome {
    let result = match item.state {
        ItemState::Done => match &item.subject {
            Subject::Folder(FolderState {
                folder: Some(folder),
                ..
            }) => ItemResult::Created { folder: *folder },
            Subject::Photos(pre) => match (item.kind, &pre.to) {
                (Kind::RemoveFromCatalog, _) => ItemResult::Removed,
                (Kind::TrashPhotos, _) => ItemResult::Trashed,
                (Kind::MovePhotos, Some((to, _))) => ItemResult::Moved { to: *to },
                (Kind::RenamePhotos, _) => match item
                    .members
                    .first()
                    .and_then(|m| m.destination.as_deref()?.file_name())
                {
                    Some(name) => ItemResult::Renamed {
                        name: name.to_string_lossy().into_owned(),
                    },
                    None => ItemResult::Unchanged(item.error.clone()),
                },
                _ => ItemResult::Unchanged(item.error.clone()),
            },
            Subject::Folder(_) => ItemResult::Unchanged(item.error.clone()),
            Subject::Tree(_) => match item.kind {
                Kind::MoveFolder | Kind::MoveRoot => ItemResult::FolderMoved,
                _ => ItemResult::FolderRenamed,
            },
            Subject::TrashTree(_) if super::tree_trash::trashed(item) => ItemResult::Trashed,
            Subject::TrashTree(_) => ItemResult::Removed,
        },
        ItemState::NoChange => ItemResult::Unchanged(item.error.clone()),
        ItemState::NeedsRecovery => {
            ItemResult::NeedsRecovery(item.error.clone().unwrap_or_default())
        }
        ItemState::Kept => ItemResult::Kept,
        // Put back as far as it could be, and waiting for the user.
        ItemState::Restoring if item.error.is_some() => {
            ItemResult::NeedsRecovery(item.error.clone().unwrap_or_default())
        }
        // The catalog follows its copies; its originals are still to go.
        ItemState::Committed => match &item.subject {
            Subject::Photos(super::preconditions::Preconditions {
                to: Some((to, _)), ..
            }) => ItemResult::Moved { to: *to },
            Subject::Tree(_) => ItemResult::FolderMoved,
            _ => ItemResult::Interrupted,
        },
        ItemState::Planned | ItemState::Restoring => ItemResult::Interrupted,
    };
    // Files first: a folder of a folder copied to another drive stays
    // because of what stays in it.
    let mut kept: Vec<&Member> = item
        .members
        .iter()
        .filter(|m| m.state == MemberState::OriginalKept)
        .collect();
    kept.sort_by_key(|m| m.identity.is_none());
    let originals_kept = kept
        .into_iter()
        .map(|m| KeptOriginal {
            path: m.source.clone(),
            why: m
                .copy
                .as_ref()
                .and_then(|c| c.kept.clone())
                .unwrap_or_default(),
        })
        .collect();
    // A finished folder Trash's error is what the Trash also took.
    let note = match (&item.subject, item.state) {
        (Subject::TrashTree(_), ItemState::Done) => item.error.clone(),
        _ => None,
    };
    ItemOutcome {
        operation: item.id,
        photos: item.photos().to_vec(),
        result,
        originals_kept,
        note,
    }
}
