//! The journal of file operations: the `file_operations` and
//! `file_operation_members` rows, written and read only here. Each disk step
//! is journalled before the next one starts, so whatever a crash leaves is
//! decided by [`super::recover`](fn@super::recover) from the journal and the disk, never
//! guessed.
use super::super::PhotoId;
use super::super::db::{Reads, Write, sql};
use super::super::value::row;
use super::folder::FolderState;
use super::preconditions::Preconditions;
use super::tree::TreeState;
use super::tree_trash::TrashTreeState;
use crate::fsops::{ContentHash, FileId};
use crate::storage::Identity;
use anyhow::{Context, Result, anyhow};
use std::path::{Path, PathBuf};

/// Maps each variant to the one text the journal stores for it, and back.
macro_rules! stored {
    ($(#[$meta:meta])* $name:ident { $($(#[$v_meta:meta])* $variant:ident = $text:literal),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub(in crate::catalog) enum $name {
            $($(#[$v_meta])* $variant),*
        }
        impl $name {
            #[cfg(test)]
            pub(super) const ALL: &[Self] = &[$(Self::$variant),*];
            pub(super) fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),*
                }
            }
        }
        impl std::str::FromStr for $name {
            type Err = anyhow::Error;
            fn from_str(text: &str) -> Result<Self> {
                match text {
                    $($text => Ok(Self::$variant),)*
                    other => Err(anyhow!(concat!("unknown ", stringify!($name), " {:?}"), other)),
                }
            }
        }
    };
}

stored! {
    /// What an item does.
    Kind {
        /// Its photos leave the catalog; nothing on disk changes.
        RemoveFromCatalog = "remove_from_catalog",
        /// Its photo's file and companions move to another folder.
        MovePhotos = "move_photos",
        /// Its photo's file and companions go to the Trash, then its rows.
        TrashPhotos = "trash_photos",
        /// A folder is made, on disk and then in the catalog.
        CreateFolder = "create_folder",
        /// Its photo's file and companions get new names in their folder.
        RenamePhotos = "rename_photos",
        /// A folder gets a new name, and the catalog's folders of its
        /// subtree new logical paths.
        RenameFolder = "rename_folder",
        /// A folder moves into another of its root, and the catalog's
        /// folders of its subtree get new logical paths.
        MoveFolder = "move_folder",
        /// A folder goes to the Trash whole, then the catalog's folders and
        /// photos of its subtree (#575). An older release can't read the
        /// kind, and fails closed on an unfinished one.
        TrashFolder = "trash_folder",
        /// A root folder moves into a folder of another root, the catalog's
        /// folders of it joining that root and the root leaving (#586). An
        /// older release can't read the kind, and fails closed on an
        /// unfinished one rather than take it for a folder move.
        MoveRoot = "move_root",
    }
}

stored! {
    /// Where an item stands.
    ItemState {
        /// Journalled; its steps may have begun.
        Planned = "planned",
        /// Being put back: never completed forward again.
        Restoring = "restoring",
        /// Copied to another volume, and the catalog follows the copies:
        /// only putting its originals in the Trash is left, which never
        /// changes the catalog.
        Committed = "committed",
        /// Finished with nothing changed, on disk or in the catalog.
        NoChange = "no_change",
        /// Unfinished, for the user to decide.
        NeedsRecovery = "needs_recovery",
        /// Left as it is by the user; never finished later.
        Kept = "kept",
        Done = "done",
    }
}

stored! {
    /// Where a member stands.
    MemberState {
        Planned = "planned",
        Moved = "moved",
        Trashed = "trashed",
        /// A folder made, at its destination: the name the disk gave it.
        Created = "created",
        /// Put back where it was.
        Restored = "restored",
        /// Its copy to another volume begun: the partial copy made, and
        /// its file object journalled.
        Copying = "copying",
        /// Its copy complete, verified and at its destination; the original
        /// as it was.
        Copied = "copied",
        /// Its copy gone again (the partial copy removed, or the complete
        /// one in the Trash); the original as it was.
        Discarded = "discarded",
        /// Its original left at its source, the catalog following its copy.
        OriginalKept = "original_kept",
    }
}

/// A journalled item's row id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OperationId(pub(super) i64);

/// A file or folder an item will touch, as planned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct NewMember {
    pub source: PathBuf,
    /// Where it goes; `None` for the Trash.
    pub destination: Option<PathBuf>,
    pub id: FileId,
    pub identity: Identity,
}

/// A journalled member.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Member {
    pub ordinal: i64,
    pub source: PathBuf,
    pub destination: Option<PathBuf>,
    pub id: Option<FileId>,
    pub identity: Option<Identity>,
    pub state: MemberState,
    /// Where the Trash put it.
    pub receipt: Option<PathBuf>,
    /// How it is copied, when it is copied to another volume.
    pub copy: Option<CopyRecord>,
}

/// A member copied to another volume, as journalled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CopyRecord {
    /// Its partial copy, hidden in the destination folder by a name unique
    /// to this item and member.
    pub temp: PathBuf,
    /// The copy's file object, from when it is made: the partial copy's,
    /// and the complete one's at its destination.
    pub id: Option<FileId>,
    /// The SHA-256 of the bytes copied, once the copy is written whole.
    pub content: Option<ContentHash>,
    /// The copy's `Identity` at its destination, once the catalog follows
    /// it.
    pub identity: Option<Identity>,
    /// Where the Trash put the copy, when the move was undone.
    pub receipt: Option<PathBuf>,
    /// Why the original stayed at its source.
    pub kept: Option<String>,
}

impl Member {
    /// Where the member is once its step is done: its destination, or where
    /// the Trash put it.
    pub(super) fn after(&self) -> Option<&Path> {
        self.destination.as_deref().or(self.receipt.as_deref())
    }
}

/// What a journalled item is about, with what must still hold of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Subject {
    /// A family of photos, or a lone virtual copy.
    Photos(Preconditions),
    /// A folder it makes.
    Folder(FolderState),
    /// A folder it renames or moves, with the subtree it re-keys.
    Tree(TreeState),
    /// A folder it puts in the Trash, with the subtree it removes.
    TrashTree(TrashTreeState),
}

/// A journalled item.
#[derive(Clone, Debug)]
pub(super) struct Item {
    pub id: OperationId,
    /// The computer whose files it changes, the only one to decide it.
    pub computer: String,
    pub kind: Kind,
    pub state: ItemState,
    pub subject: Subject,
    pub members: Vec<Member>,
    pub error: Option<String>,
}

impl Item {
    /// Whether it copies its files to another volume.
    pub(super) fn copies(&self) -> bool {
        self.members.iter().any(|m| m.copy.is_some())
    }
    /// The catalog photos it concerns: none for a folder it makes, those
    /// in the subtree of one it renames.
    pub(super) fn photos(&self) -> &[PhotoId] {
        match &self.subject {
            Subject::Photos(pre) => &pre.rows,
            Subject::Folder(_) => &[],
            Subject::Tree(tree) => &tree.photos,
            Subject::TrashTree(tree) => &tree.photos,
        }
    }
}

/// A path as the journal stores it. Catalog paths are text, so a path that
/// isn't can't be journalled, and is never moved.
pub(super) fn text(path: &Path) -> Result<&str> {
    path.to_str()
        .with_context(|| format!("{} is not a name RAWmakase can record", path.display()))
}

/// Journals a new item of `batch` in `state`, with its members planned.
#[allow(clippy::too_many_arguments, reason = "one row's columns")]
pub(super) fn insert(
    w: &mut Write<'_>,
    batch: &str,
    computer: &str,
    kind: Kind,
    state: ItemState,
    preconditions: &Preconditions,
    members: &[NewMember],
    error: Option<&str>,
) -> Result<OperationId> {
    let id: i64 = w.insert_returning_id(
        sql!(
            "INSERT INTO file_operations(batch, computer, kind, state, preconditions, created_at, error)
             VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id"
        ),
        &[
            &batch,
            &computer,
            &kind.as_str(),
            &state.as_str(),
            &serde_json::to_string(preconditions)?,
            &crate::time::now_text(),
            &error,
        ],
    )?;
    insert_members(w, id, members)?;
    Ok(OperationId(id))
}

fn insert_members(w: &mut Write<'_>, id: i64, members: &[NewMember]) -> Result<()> {
    for (ordinal, m) in members.iter().enumerate() {
        let destination = m.destination.as_deref().map(text).transpose()?;
        w.execute(
            sql!(
                "INSERT INTO file_operation_members(operation, ordinal, source, destination,
                    file_id, identity, state)
                 VALUES (?, ?, ?, ?, ?, ?, ?)"
            ),
            &[
                &id,
                &(ordinal as i64),
                &text(&m.source)?,
                &destination,
                &m.id.to_string(),
                &serde_json::to_string(&m.identity)?,
                &MemberState::Planned.as_str(),
            ],
        )?;
    }
    Ok(())
}

/// Journals that item `id` copies its members to another volume, each
/// first to its partial copy `temps[ordinal]`.
pub(super) fn insert_copies(w: &mut Write<'_>, id: OperationId, temps: &[PathBuf]) -> Result<()> {
    for (ordinal, temp) in temps.iter().enumerate() {
        w.execute(
            sql!("INSERT INTO file_operation_copies(operation, ordinal, temp) VALUES (?, ?, ?)"),
            &[&id.0, &(ordinal as i64), &text(temp)?],
        )?;
    }
    Ok(())
}

/// Records that member `ordinal` of item `id` is being copied, into the
/// partial copy at `temp` as the disk spells it: file object `file`, where
/// its volume keeps them (never journalled where it doesn't, #573).
pub(super) fn set_copying(
    w: &mut Write<'_>,
    id: OperationId,
    ordinal: i64,
    file: Option<FileId>,
    temp: &Path,
) -> Result<()> {
    let changed = w.execute(
        sql!(
            "UPDATE file_operation_copies SET copy_id=?, temp=?
             WHERE operation=? AND ordinal=?"
        ),
        &[&file.map(|f| f.to_string()), &text(temp)?, &id.0, &ordinal],
    )?;
    anyhow::ensure!(changed == 1, "unknown file operation copy");
    set_member(w, id, ordinal, MemberState::Copying, None)
}

/// Records the SHA-256 of the bytes member `ordinal` of item `id` was
/// copied with, once its copy is written whole.
pub(super) fn set_content(
    w: &mut Write<'_>,
    id: OperationId,
    ordinal: i64,
    content: ContentHash,
) -> Result<()> {
    let changed = w.execute(
        sql!("UPDATE file_operation_copies SET content=? WHERE operation=? AND ordinal=?"),
        &[&content.to_string(), &id.0, &ordinal],
    )?;
    anyhow::ensure!(changed == 1, "unknown file operation copy");
    Ok(())
}

/// Marks member `ordinal` of item `id` copied, at `destination`: the name
/// the disk gave it.
pub(super) fn set_copied(
    w: &mut Write<'_>,
    id: OperationId,
    ordinal: i64,
    destination: &Path,
) -> Result<()> {
    let changed = w.execute(
        sql!(
            "UPDATE file_operation_members SET state=?, destination=?
             WHERE operation=? AND ordinal=?"
        ),
        &[
            &MemberState::Copied.as_str(),
            &text(destination)?,
            &id.0,
            &ordinal,
        ],
    )?;
    anyhow::ensure!(changed == 1, "unknown file operation member");
    Ok(())
}

/// Records the `Identity` of member `ordinal` of item `id`'s copy at its
/// destination, as the catalog follows it.
pub(super) fn set_copy_identity(
    w: &mut Write<'_>,
    id: OperationId,
    ordinal: i64,
    identity: &Identity,
) -> Result<()> {
    let changed = w.execute(
        sql!("UPDATE file_operation_copies SET copy_identity=? WHERE operation=? AND ordinal=?"),
        &[&serde_json::to_string(identity)?, &id.0, &ordinal],
    )?;
    anyhow::ensure!(changed == 1, "unknown file operation copy");
    Ok(())
}

/// Marks member `ordinal` of item `id`'s copy gone again, with where the
/// Trash put it when it did.
pub(super) fn set_discarded(
    w: &mut Write<'_>,
    id: OperationId,
    ordinal: i64,
    receipt: Option<&Path>,
) -> Result<()> {
    let receipt = receipt.map(text).transpose()?;
    let changed = w.execute(
        sql!(
            "UPDATE file_operation_copies SET copy_receipt=COALESCE(?, copy_receipt)
             WHERE operation=? AND ordinal=?"
        ),
        &[&receipt, &id.0, &ordinal],
    )?;
    anyhow::ensure!(changed == 1, "unknown file operation copy");
    set_member(w, id, ordinal, MemberState::Discarded, None)
}

/// Marks member `ordinal` of item `id`'s original kept at its source, for
/// reason `why`.
pub(super) fn set_kept(w: &mut Write<'_>, id: OperationId, ordinal: i64, why: &str) -> Result<()> {
    let changed = w.execute(
        sql!("UPDATE file_operation_copies SET kept=? WHERE operation=? AND ordinal=?"),
        &[&why, &id.0, &ordinal],
    )?;
    anyhow::ensure!(changed == 1, "unknown file operation copy");
    set_member(w, id, ordinal, MemberState::OriginalKept, None)
}

/// Journals a new item of `batch` making folder `path`, in `state`: its one
/// member has no file object yet, as the folder isn't made.
pub(super) fn insert_folder(
    w: &mut Write<'_>,
    batch: &str,
    computer: &str,
    state: ItemState,
    folder: &FolderState,
    path: &Path,
    error: Option<&str>,
) -> Result<OperationId> {
    let id: i64 = w.insert_returning_id(
        sql!(
            "INSERT INTO file_operations(batch, computer, kind, state, preconditions, created_at, error)
             VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id"
        ),
        &[
            &batch,
            &computer,
            &Kind::CreateFolder.as_str(),
            &state.as_str(),
            &serde_json::to_string(folder)?,
            &crate::time::now_text(),
            &error,
        ],
    )?;
    w.execute(
        sql!(
            "INSERT INTO file_operation_members(operation, ordinal, source, state)
             VALUES (?, 0, ?, ?)"
        ),
        &[&id, &text(path)?, &MemberState::Planned.as_str()],
    )?;
    Ok(OperationId(id))
}

/// Journals a new item of `batch` renaming a folder, in `state`: its one
/// member is the folder's directory, file object `file`, going from
/// `source` to `destination`.
#[allow(clippy::too_many_arguments, reason = "one row's columns")]
pub(super) fn insert_tree(
    w: &mut Write<'_>,
    batch: &str,
    computer: &str,
    kind: Kind,
    state: ItemState,
    tree: &TreeState,
    (source, destination, file): (&Path, &Path, FileId),
    error: Option<&str>,
) -> Result<OperationId> {
    let id: i64 = w.insert_returning_id(
        sql!(
            "INSERT INTO file_operations(batch, computer, kind, state, preconditions, created_at, error)
             VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id"
        ),
        &[
            &batch,
            &computer,
            &kind.as_str(),
            &state.as_str(),
            &serde_json::to_string(tree)?,
            &crate::time::now_text(),
            &error,
        ],
    )?;
    w.execute(
        sql!(
            "INSERT INTO file_operation_members(operation, ordinal, source, destination, file_id, state)
             VALUES (?, 0, ?, ?, ?, ?)"
        ),
        &[
            &id,
            &text(source)?,
            &text(destination)?,
            &file.to_string(),
            &MemberState::Planned.as_str(),
        ],
    )?;
    Ok(OperationId(id))
}

/// A file or folder of a folder copied to another volume, as planned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct NewTreeMember {
    pub source: PathBuf,
    /// Where it is once the folder has its name there.
    pub destination: PathBuf,
    pub id: FileId,
    /// A file's; `None` for a folder.
    pub identity: Option<Identity>,
    /// Where it is made, inside the hidden folder the copy is built in.
    pub temp: PathBuf,
}

/// Journals a new item of `batch` copying a folder to another volume, in
/// `state`: its members planned, each with the place it is made.
#[allow(clippy::too_many_arguments, reason = "one row's columns")]
pub(super) fn insert_tree_copy(
    w: &mut Write<'_>,
    batch: &str,
    computer: &str,
    kind: Kind,
    state: ItemState,
    tree: &TreeState,
    members: &[NewTreeMember],
    error: Option<&str>,
) -> Result<OperationId> {
    let id: i64 = w.insert_returning_id(
        sql!(
            "INSERT INTO file_operations(batch, computer, kind, state, preconditions, created_at, error)
             VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id"
        ),
        &[
            &batch,
            &computer,
            &kind.as_str(),
            &state.as_str(),
            &serde_json::to_string(tree)?,
            &crate::time::now_text(),
            &error,
        ],
    )?;
    for (ordinal, m) in members.iter().enumerate() {
        let identity = m.identity.as_ref().map(serde_json::to_string).transpose()?;
        w.execute(
            sql!(
                "INSERT INTO file_operation_members(operation, ordinal, source, destination,
                    file_id, identity, state)
                 VALUES (?, ?, ?, ?, ?, ?, ?)"
            ),
            &[
                &id,
                &(ordinal as i64),
                &text(&m.source)?,
                &text(&m.destination)?,
                &m.id.to_string(),
                &identity,
                &MemberState::Planned.as_str(),
            ],
        )?;
        w.execute(
            sql!("INSERT INTO file_operation_copies(operation, ordinal, temp) VALUES (?, ?, ?)"),
            &[&id, &(ordinal as i64), &text(&m.temp)?],
        )?;
    }
    Ok(OperationId(id))
}

/// A name of a folder put in the Trash, as planned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct NewTrashMember {
    pub source: PathBuf,
    /// Its file object as planned; on FAT and exFAT, whose ids change,
    /// journalled but never relied on.
    pub id: FileId,
    /// A file's; `None` for a folder.
    pub identity: Option<Identity>,
}

/// Journals a new item of `batch` putting a folder in the Trash, in
/// `state`: its members planned, the folder first, then its folders and
/// files as walked.
pub(super) fn insert_tree_trash(
    w: &mut Write<'_>,
    batch: &str,
    computer: &str,
    state: ItemState,
    tree: &TrashTreeState,
    members: &[NewTrashMember],
    error: Option<&str>,
) -> Result<OperationId> {
    let id: i64 = w.insert_returning_id(
        sql!(
            "INSERT INTO file_operations(batch, computer, kind, state, preconditions, created_at, error)
             VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id"
        ),
        &[
            &batch,
            &computer,
            &Kind::TrashFolder.as_str(),
            &state.as_str(),
            &serde_json::to_string(tree)?,
            &crate::time::now_text(),
            &error,
        ],
    )?;
    for (ordinal, m) in members.iter().enumerate() {
        let identity = m.identity.as_ref().map(serde_json::to_string).transpose()?;
        w.execute(
            sql!(
                "INSERT INTO file_operation_members(operation, ordinal, source, file_id, identity,
                    state)
                 VALUES (?, ?, ?, ?, ?, ?)"
            ),
            &[
                &id,
                &(ordinal as i64),
                &text(&m.source)?,
                &m.id.to_string(),
                &identity,
                &MemberState::Planned.as_str(),
            ],
        )?;
    }
    Ok(OperationId(id))
}

/// Marks each of `receipts`' members of item `id` (by ordinal) trashed,
/// with where the Trash put it.
pub(super) fn set_receipts(
    w: &mut Write<'_>,
    id: OperationId,
    receipts: &[(i64, PathBuf)],
) -> Result<()> {
    for (ordinal, receipt) in receipts {
        set_member(w, id, *ordinal, MemberState::Trashed, Some(receipt))?;
    }
    Ok(())
}

/// Marks item `id` done, its error set to `note` exactly: no earlier
/// failure's text survives into a finished item.
pub(super) fn set_done(w: &mut Write<'_>, id: OperationId, note: Option<&str>) -> Result<()> {
    let changed = w.execute(
        sql!("UPDATE file_operations SET state=?, error=? WHERE id=?"),
        &[&ItemState::Done.as_str(), &note, &id.0],
    )?;
    anyhow::ensure!(changed == 1, "unknown file operation");
    Ok(())
}

/// Records where member `ordinal` of item `id` is once its step is done:
/// `destination`, as the disk spells it.
pub(super) fn set_destination(
    w: &mut Write<'_>,
    id: OperationId,
    ordinal: i64,
    destination: &Path,
) -> Result<()> {
    let changed = w.execute(
        sql!("UPDATE file_operation_members SET destination=? WHERE operation=? AND ordinal=?"),
        &[&text(destination)?, &id.0, &ordinal],
    )?;
    anyhow::ensure!(changed == 1, "unknown file operation member");
    Ok(())
}

/// Records the folder item `id` made: its file object, and the name the
/// disk gave it at `made`.
pub(super) fn set_created(
    w: &mut Write<'_>,
    id: OperationId,
    file: FileId,
    made: &Path,
) -> Result<()> {
    let changed = w.execute(
        sql!(
            "UPDATE file_operation_members SET state=?, file_id=?, destination=?
             WHERE operation=? AND ordinal=0"
        ),
        &[
            &MemberState::Created.as_str(),
            &file.to_string(),
            &text(made)?,
            &id.0,
        ],
    )?;
    anyhow::ensure!(changed == 1, "unknown file operation member");
    Ok(())
}

/// Replaces photo item `id`'s preconditions, as they are once its
/// destination is the catalog's folder it added or found there.
pub(super) fn set_preconditions(
    w: &mut Write<'_>,
    id: OperationId,
    preconditions: &Preconditions,
) -> Result<()> {
    let changed = w.execute(
        sql!("UPDATE file_operations SET preconditions=? WHERE id=? AND kind=?"),
        &[
            &serde_json::to_string(preconditions)?,
            &id.0,
            &Kind::MovePhotos.as_str(),
        ],
    )?;
    anyhow::ensure!(changed == 1, "unknown move operation");
    Ok(())
}

/// Replaces folder item `id`'s state, as it is once the catalog has it.
pub(super) fn set_folder(w: &mut Write<'_>, id: OperationId, folder: &FolderState) -> Result<()> {
    let changed = w.execute(
        sql!("UPDATE file_operations SET preconditions=? WHERE id=? AND kind=?"),
        &[
            &serde_json::to_string(folder)?,
            &id.0,
            &Kind::CreateFolder.as_str(),
        ],
    )?;
    anyhow::ensure!(changed == 1, "unknown folder operation");
    Ok(())
}

/// Sets item `id`'s state, and its error when one is given.
pub(super) fn set_state(
    w: &mut Write<'_>,
    id: OperationId,
    state: ItemState,
    error: Option<&str>,
) -> Result<()> {
    let changed = w.execute(
        sql!("UPDATE file_operations SET state=?, error=COALESCE(?, error) WHERE id=?"),
        &[&state.as_str(), &error, &id.0],
    )?;
    anyhow::ensure!(changed == 1, "unknown file operation");
    Ok(())
}

/// Marks member `ordinal` of item `id` moved, to `destination`: the
/// name the disk gave it, which a volume that normalises names may spell
/// otherwise than asked.
pub(super) fn set_moved(
    w: &mut Write<'_>,
    id: OperationId,
    ordinal: i64,
    destination: &Path,
) -> Result<()> {
    let changed = w.execute(
        sql!(
            "UPDATE file_operation_members SET state=?, destination=?
             WHERE operation=? AND ordinal=?"
        ),
        &[
            &MemberState::Moved.as_str(),
            &text(destination)?,
            &id.0,
            &ordinal,
        ],
    )?;
    anyhow::ensure!(changed == 1, "unknown file operation member");
    Ok(())
}

/// Sets member `ordinal` of item `id`'s state, with where the Trash put it.
pub(super) fn set_member(
    w: &mut Write<'_>,
    id: OperationId,
    ordinal: i64,
    state: MemberState,
    receipt: Option<&Path>,
) -> Result<()> {
    let receipt = receipt.map(text).transpose()?;
    let changed = w.execute(
        sql!(
            "UPDATE file_operation_members SET state=?, receipt=COALESCE(?, receipt)
             WHERE operation=? AND ordinal=?"
        ),
        &[&state.as_str(), &receipt, &id.0, &ordinal],
    )?;
    anyhow::ensure!(changed == 1, "unknown file operation member");
    Ok(())
}

row! {
    struct ItemRow {
        id: i64,
        computer: String,
        kind: String,
        state: String,
        preconditions: String,
        error: Option<String>,
    }
}
row! {
    struct CopyRow {
        ordinal: i64,
        temp: String,
        copy_id: Option<String>,
        content: Option<String>,
        copy_identity: Option<String>,
        copy_receipt: Option<String>,
        kept: Option<String>,
    }
}
row! {
    struct MemberRow {
        ordinal: i64,
        source: String,
        destination: Option<String>,
        file_id: Option<String>,
        identity: Option<String>,
        state: String,
        receipt: Option<String>,
    }
}

fn read(db: &impl Reads, row: ItemRow) -> Result<Item> {
    let members: Vec<MemberRow> = db.read(
        sql!(
            "SELECT ordinal, source, destination, file_id, identity, state, receipt
             FROM file_operation_members WHERE operation=? ORDER BY ordinal"
        ),
        &[&row.id],
    )?;
    let copies: Vec<CopyRow> = db.read(
        sql!(
            "SELECT ordinal, temp, copy_id, content, copy_identity, copy_receipt, kept
             FROM file_operation_copies WHERE operation=? ORDER BY ordinal"
        ),
        &[&row.id],
    )?;
    let mut copies: std::collections::HashMap<i64, CopyRecord> = copies
        .into_iter()
        .map(|c| {
            let content = c
                .content
                .map(|t| t.parse::<ContentHash>().map_err(|e| anyhow!("{e}")))
                .transpose()?;
            Ok((
                c.ordinal,
                CopyRecord {
                    temp: c.temp.into(),
                    id: c.copy_id.map(|t| t.parse()).transpose()?,
                    content,
                    identity: c
                        .copy_identity
                        .map(|t| serde_json::from_str(&t))
                        .transpose()?,
                    receipt: c.copy_receipt.map(PathBuf::from),
                    kept: c.kept,
                },
            ))
        })
        .collect::<Result<_>>()?;
    let members = members
        .into_iter()
        .map(|m| {
            Ok(Member {
                copy: copies.remove(&m.ordinal),
                ordinal: m.ordinal,
                source: m.source.into(),
                destination: m.destination.map(PathBuf::from),
                id: m.file_id.map(|t| t.parse()).transpose()?,
                identity: m.identity.map(|t| serde_json::from_str(&t)).transpose()?,
                state: m.state.parse()?,
                receipt: m.receipt.map(PathBuf::from),
            })
        })
        .collect::<Result<_>>()?;
    let kind: Kind = row.kind.parse()?;
    let subject = match kind {
        Kind::CreateFolder => Subject::Folder(serde_json::from_str(&row.preconditions)?),
        Kind::RenameFolder | Kind::MoveFolder | Kind::MoveRoot => {
            let tree: TreeState = serde_json::from_str(&row.preconditions)?;
            // A root's move is journalled as one alone: the other kinds
            // never move a whole root, nor it a folder.
            anyhow::ensure!(
                tree.moves_root() == (kind == Kind::MoveRoot),
                "file operation {} is a {} of {:?}, which this release never journals",
                row.id,
                kind.as_str(),
                tree.from
            );
            Subject::Tree(tree)
        }
        Kind::TrashFolder => Subject::TrashTree(serde_json::from_str(&row.preconditions)?),
        Kind::RemoveFromCatalog | Kind::MovePhotos | Kind::TrashPhotos | Kind::RenamePhotos => {
            Subject::Photos(serde_json::from_str(&row.preconditions)?)
        }
    };
    Ok(Item {
        id: OperationId(row.id),
        computer: row.computer,
        kind,
        state: row.state.parse()?,
        subject,
        members,
        error: row.error,
    })
}

/// Item `id`, if it is journalled.
pub(super) fn item(db: &impl Reads, id: OperationId) -> Result<Option<Item>> {
    let row: Option<ItemRow> = db.read_optional(
        sql!(
            "SELECT id, computer, kind, state, preconditions, error FROM file_operations WHERE id=?"
        ),
        &[&id.0],
    )?;
    row.map(|row| read(db, row)).transpose()
}

/// Every item of `batch`, in the order they were journalled.
pub(super) fn of_batch(db: &impl Reads, batch: &str) -> Result<Vec<Item>> {
    let rows: Vec<ItemRow> = db.read(
        sql!(
            "SELECT id, computer, kind, state, preconditions, error FROM file_operations
             WHERE batch=? ORDER BY id"
        ),
        &[&batch],
    )?;
    rows.into_iter().map(|row| read(db, row)).collect()
}

/// Every item of `computer` still to be decided, oldest first.
pub(super) fn unresolved(db: &impl Reads, computer: &str) -> Result<Vec<Item>> {
    let rows: Vec<ItemRow> = db.read(
        sql!(
            "SELECT id, computer, kind, state, preconditions, error FROM file_operations
             WHERE computer=? AND state IN ('planned', 'restoring', 'committed', 'needs_recovery')
             ORDER BY id"
        ),
        &[&computer],
    )?;
    rows.into_iter().map(|row| read(db, row)).collect()
}

/// Every item still to be decided, whichever computer's.
pub(super) fn unresolved_anywhere(db: &impl Reads) -> Result<Vec<Item>> {
    let rows: Vec<ItemRow> = db.read(
        sql!(
            "SELECT id, computer, kind, state, preconditions, error FROM file_operations
             WHERE state IN ('planned', 'restoring', 'committed', 'needs_recovery')
             ORDER BY id"
        ),
        &[],
    )?;
    rows.into_iter().map(|row| read(db, row)).collect()
}

/// Forgets the finished items journalled before `before` (a time as
/// [`crate::time::utc_text`] writes it). Returns how many.
pub(super) fn prune(w: &mut Write<'_>, before: &str) -> Result<usize> {
    w.execute(
        sql!(
            "DELETE FROM file_operations
             WHERE state IN ('done', 'no_change', 'kept') AND created_at < ?"
        ),
        &[&before],
    )
}
