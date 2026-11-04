//! File operations (#370, planned in #556): the one owner of changing where
//! a photo is, on disk and in the catalog. A request is planned against a
//! snapshot ([`plan`](fn@plan)), confirmed by the user ([`Plan::confirm`]), executed
//! item by item under the catalog's operation lock with every disk step
//! journalled first ([`execute`](fn@execute)), and anything a crash cut short is
//! decided from the journal and the disk ([`recover`](fn@recover)).
//!
//! What it guarantees:
//! - No byte of an original is lost or overwritten. A file leaves its path
//!   only by an exclusive rename on its volume or by the system Trash;
//!   nothing is replaced. A folder is made only where nothing has its name,
//!   and never deleted.
//! - A photo moved to another volume is copied, never renamed across: into
//!   a hidden partial copy made where nothing had its name, hashed as it is
//!   read, synced, given the original's time, read back and compared, then
//!   renamed exclusively to its name there. The catalog follows the copies
//!   only once every file of the photo is so copied and its original is
//!   still the file it copied; only then does the original go to the Trash,
//!   and only while it is still that file. The one file ever unlinked is a
//!   partial copy this run made (by its journalled file object, or by its
//!   unique name where the journal says it was being made, empty) that was
//!   never renamed to its name, and one written whole only while its
//!   original is proven at its source as it was copied (when the user puts
//!   back a move whose original changed since, a partial copy written whole
//!   goes to the Trash instead): a complete copy goes only to the
//!   Trash, and only while it is proven the copy (its file object, its
//!   bytes those of the original) and its original is still at its source.
//!   A copy's name held by another file object is never taken for the copy
//!   gone: it is left for the user.
//! - Catalog rows change only on positive evidence: they follow a file
//!   proven at its new path (the same file object, or a copy proven so),
//!   and go only with a Trash receipt and the file proven gone from its
//!   path. A file's absence alone authorises nothing; what can't be proven
//!   is left as it was and reported.
//! - A rename keeps the file object, its bytes and its time, so a photo's
//!   `Identity`, saved edit, checksum and caches stay valid. A copy keeps its
//!   bytes and its time as closely as its volume can; where it keeps times
//!   more coarsely, the photo's saved edit and checksum are re-keyed to the
//!   copy, which has the same bytes.
//!
//! Policy (which photos and files an operation takes, what it refuses) is
//! here; the file system calls are `rawmakase_fs`'s, reached through the
//! [`disk`] seam, and the Trash is the app's, passed in as a [`Trash`].
mod companions;
mod conflicts;
mod copy;
mod disk;
mod execute;
mod folder;
mod journal;
mod lock;
mod names;
mod plan;
mod preconditions;
mod proof;
mod recover;
mod rename;
mod tree;
mod tree_copy;
mod tree_trash;

pub use execute::execute;
pub use folder::folder_name_problem;
pub use journal::OperationId;
pub(in crate::catalog) use lock::opening;
pub use names::{NameProblem, photo_name_problem};
pub use plan::plan;
pub use recover::{
    Recovery, Resolution, Unfinished, UnfinishedMember, recover, resolve, unfinished_here,
};
pub use rename::{Ordered, RenameOrder, RenamePreview, RenameRow, rename_order, rename_preview};

use super::{CatalogLocation, FolderId, PhotoId, RootId};

/// Whether any computer has a file operation still to be decided: its
/// catalog rows may then be halfway between where the files were and are.
pub(in crate::catalog) fn any_unresolved(db: &impl super::db::Reads) -> anyhow::Result<bool> {
    Ok(!journal::unresolved_anywhere(db)?.is_empty())
}
use std::fmt;
use std::path::PathBuf;

/// What the user asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileOp {
    /// Takes photos out of the catalog with their edits and metadata; no
    /// file changes. A master takes its virtual copies with it; a copy
    /// alone goes alone.
    RemoveFromCatalog { photos: Vec<PhotoId> },
    /// Moves masters' files, with the companions only they read, into the
    /// folder at logical path `to` of `root`, which the catalog may have no
    /// folder of its own for (it gains one as their rows follow, the
    /// directory being there); their rows, copies included, follow. On the
    /// same volume a file is renamed; on another it is copied, and its
    /// original put in the Trash once the catalog follows the copy.
    MovePhotos {
        photos: Vec<PhotoId>,
        root: RootId,
        to: String,
    },
    /// Puts masters' files, with the companions only they read, in the
    /// system Trash; then their rows, copies included, leave the catalog.
    TrashPhotos { photos: Vec<PhotoId> },
    /// Makes folder `name` below logical folder `parent` of `root` (which
    /// the catalog may have no folder of its own for), on disk and in the
    /// catalog; then moves `then_move` into it, as [`Self::MovePhotos`].
    CreateFolder {
        root: RootId,
        parent: String,
        name: String,
        then_move: Vec<PhotoId>,
    },
    /// Gives masters' files new names in their folders: each a new stem,
    /// the extension kept, the companions only they read renamed to match;
    /// their rows, copies included, follow. All or none: if one can't be
    /// renamed, none is.
    RenamePhotos { renames: Vec<(PhotoId, String)> },
    /// Gives the folder at logical path `folder` of `root` the name
    /// `name`, on disk and in the catalog, with every catalog folder below
    /// it; their photos follow.
    RenameFolder {
        root: RootId,
        folder: String,
        name: String,
    },
    /// Moves the folder at logical path `folder` of `root` into logical
    /// folder `parent` of root `to_root` (`root` itself, or another; either
    /// folder may be one the catalog has no folder of its own for), on the
    /// same volume, with every catalog folder below it; their photos
    /// follow.
    MoveFolder {
        root: RootId,
        folder: String,
        to_root: RootId,
        parent: String,
    },
    /// Moves root folder `root` into logical folder `parent` of another
    /// root, `to_root` (#586), with every catalog folder of it: on its
    /// drive by one rename, onto another by a copy of its whole tree, as
    /// [`Self::MoveFolder`] moves a folder. Its folders join `to_root`,
    /// keeping their ids, and `root` leaves the catalog's roots.
    MoveRoot {
        root: RootId,
        to_root: RootId,
        parent: String,
    },
    /// Puts the folder at logical path `folder` of `root` in the system
    /// Trash, with everything in it; then the catalog's folders of its
    /// subtree leave the catalog with their photos (virtual copies
    /// included). Its root stays. Planned only by [`plan_with_trash`].
    TrashFolder { root: RootId, folder: String },
}

/// One run of an operation, by which its outcome is read back from the
/// journal, whatever happened to the thread that ran it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BatchId(String);

impl fmt::Display for BatchId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Why a photo the user chose is left out of an operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Skip {
    /// It is no longer in the catalog.
    Unknown,
    /// A virtual copy whose master wasn't chosen: it has no file of its own.
    CopyOnly,
    /// Its file is missing, or its folder or volume is offline.
    Missing,
    /// It is in that folder already.
    AlreadyThere,
    /// It has that name already.
    SameName,
    /// Another photo of the rename can't be renamed, so none is.
    Blocked,
    Refused(Refusal),
}

/// Why a photo's file can't be changed safely.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A link, a junction or a mount point is between its root folder and
    /// it, or it is one itself.
    NotPlain(PathBuf),
    /// Another photo of the catalog is the same file: a second root or
    /// folder over the same directory, a duplicate row or a hard link.
    Shared,
    /// It was chosen twice, as the same file through another photo.
    Twice,
    /// Its volume is read-only.
    ReadOnly,
    /// It is in the Trash, or in RAWmakase's own folders.
    Protected,
    /// The destination has a file or folder of that name, or a photo of the
    /// catalog is named so there.
    NameTaken(String),
    /// Two photos of this operation would have that name there.
    NameTwice(String),
    /// That file there would read a companion arriving with it, or the
    /// photo would read that file as its own.
    CompanionClash(String),
    /// Its file can't be read.
    Unreadable(String),
    /// The new name can't be a file's.
    BadName(NameProblem),
    /// Another photo of the same rename has that name now: a name freed in
    /// the same rename isn't taken.
    Chain(String),
    /// The new name differs only in case or accents, which its volume
    /// can't change on its own.
    SpellingOnly,
    /// Its companion can't be renamed to match.
    CompanionName(String),
    /// It would be copied to or from a drive whose files RAWmakase can't
    /// tell apart: any but a local APFS, HFS+, ext, XFS, Btrfs, NTFS, FAT or
    /// exFAT one (a network share, FUSE, ReFS…).
    FileIdsUnproven,
}

impl fmt::Display for Skip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown => f.write_str("It is no longer in the catalog."),
            Self::CopyOnly => f.write_str("Only original files can be moved or renamed."),
            Self::Missing => f.write_str("Its file is missing or offline."),
            Self::AlreadyThere => f.write_str("It is in that folder already."),
            Self::SameName => f.write_str("It has that name already."),
            Self::Blocked => f.write_str("Another photo can't be renamed, so none is."),
            Self::Refused(r) => r.fmt(f),
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotPlain(p) => write!(
                f,
                "{} is a link or another volume inside its folder.",
                p.display()
            ),
            Self::Shared => f.write_str("Another photo in the catalog is the same file."),
            Self::Twice => f.write_str("The same file was chosen twice."),
            Self::ReadOnly => f.write_str("Its volume is read-only."),
            Self::Protected => f.write_str("It is in the Trash or in RAWmakase's own folders."),
            Self::NameTaken(n) => write!(f, "A file or folder named {n} is already there."),
            Self::NameTwice(n) => write!(f, "Two of the photos would be named {n} there."),
            Self::CompanionClash(n) => {
                write!(f, "{n} there would share an XMP sidecar with it.")
            }
            Self::Unreadable(e) => write!(f, "Its file can't be read: {e}"),
            Self::BadName(p) => p.fmt(f),
            Self::Chain(n) => write!(
                f,
                "Another photo being renamed is named {n} now. Rename them in two steps."
            ),
            Self::SpellingOnly => f.write_str(
                "Its volume can't change only the case or accents of a name. Give it another name first.",
            ),
            Self::CompanionName(n) => write!(f, "{n} can't be renamed to match it."),
            Self::FileIdsUnproven => f.write_str(
                "Moving photos to or from this kind of drive (network drives other than SMB shares, and others) isn't available yet. Copy them in Finder, then use Synchronize Folder.",
            ),
        }
    }
}

/// A chosen photo left out, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    pub photo: PhotoId,
    pub reason: Skip,
}

/// What a plan will do, counted for its confirmation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// Catalog photos it changes, virtual copies included.
    pub photos: usize,
    /// Of those, virtual copies.
    pub copies: usize,
    /// Of those, photos with an edit made in RAWmakase.
    pub edited: usize,
    /// Photo files it changes on disk.
    pub files: usize,
    /// Companion files (XMP sidecars, earlier releases' edit files) going
    /// with them.
    pub companions: usize,
    /// Companions another file reads too, which stay where they are.
    pub shared_companions: Vec<PathBuf>,
    /// The photos' file names, in order.
    pub names: Vec<String>,
    /// Whether another computer uses the catalog, whose copies of the files
    /// don't move with them.
    pub other_computers: bool,
    /// The folder it makes, by name.
    pub folder: Option<String>,
    /// The photos' file names and the names they are given, in order.
    pub renames: Vec<(String, String)>,
    /// What it copies to another drive, when the destination is on one.
    pub across: Option<Across>,
    /// What a folder it puts in the Trash holds.
    pub tree: Option<TreeCounts>,
}

/// What a folder put in the Trash holds, from the walk of its tree and the
/// catalog, for its confirmation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TreeCounts {
    /// Folders below it (not itself).
    pub folders: usize,
    /// Files in it that are a catalog photo's.
    pub photos: usize,
    /// Every other file in it: sidecars, files the catalog doesn't know,
    /// hidden ones. On macOS's FAT and exFAT a file's AppleDouble file
    /// (`._name`) is part of it, and not counted.
    pub other_files: usize,
    /// The first three of those by name, sorted.
    pub other_names: Vec<String>,
    /// Catalog photos in it whose files are missing, and catalog folders in
    /// it missing on disk: they leave the catalog with it.
    pub missing_photos: usize,
    pub missing_folders: usize,
    /// Virtual copies in it of photos elsewhere, which leave with it.
    pub stray_copies: usize,
}

/// A move to another drive: its files are copied there, and their
/// originals put in the Trash once the catalog follows the copies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Across {
    /// The photo files copied, of [`Summary::files`]; the rest are renamed
    /// on their own drive.
    pub files: usize,
    /// The companion files copied with them, of [`Summary::companions`].
    pub companions: usize,
    /// The bytes copied, companions included.
    pub bytes: u64,
    /// The destination drive's top folder (where it is mounted).
    pub volume: PathBuf,
    /// The bytes free on it when planned.
    pub available: u64,
    /// How many of what it copies keep their originals where they are:
    /// photo files, with their companions, or the folder, which counts
    /// once. They are on an SMB share, which has no Trash RAWmakase uses;
    /// nothing is deleted instead.
    pub originals_stay: usize,
}

/// An operation planned and not yet confirmed. Nothing has changed.
#[must_use]
#[derive(Debug)]
pub struct Plan {
    batch: BatchId,
    location: CatalogLocation,
    computer: String,
    kind: journal::Kind,
    /// The folder it makes before its moves, if any.
    create: Option<folder::NewFolder>,
    /// The folder it renames, if it renames one.
    tree: Option<tree::TreeMove>,
    /// The folder it puts in the Trash, if it trashes one.
    trash_tree: Option<tree_trash::TrashTree>,
    items: Vec<plan::Item>,
    skipped: Vec<Skipped>,
    summary: Summary,
}

impl Plan {
    /// The run this plan's outcome will be journalled as.
    pub fn batch(&self) -> &BatchId {
        &self.batch
    }
    pub fn summary(&self) -> &Summary {
        &self.summary
    }
    pub fn skipped(&self) -> &[Skipped] {
        &self.skipped
    }
    /// Every catalog photo the plan would change.
    pub fn photos(&self) -> Vec<PhotoId> {
        self.items
            .iter()
            .flat_map(|i| i.preconditions.rows.iter().copied())
            .chain(
                self.tree
                    .iter()
                    .flat_map(|t| t.state.photos.iter().copied()),
            )
            .chain(
                self.trash_tree
                    .iter()
                    .flat_map(|t| t.state.photos.iter().copied()),
            )
            .collect()
    }
    /// How many items it runs: each family, and the folder it makes,
    /// renames or puts in the Trash.
    pub fn steps(&self) -> usize {
        self.items.len()
            + usize::from(self.create.is_some())
            + usize::from(self.tree.is_some())
            + usize::from(self.trash_tree.is_some())
    }
    /// Whether there is anything to do.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
            && self.create.is_none()
            && self.tree.is_none()
            && self.trash_tree.is_none()
    }
    /// The user's confirmation, which only the confirming dialog's action
    /// gives: it enforces the order of plan, confirm, execute, not that a
    /// person clicked.
    pub fn confirm(self) -> Confirmed {
        Confirmed(self)
    }
}

/// A plan the user confirmed, executed once.
#[must_use]
#[derive(Debug)]
pub struct Confirmed(Plan);

/// Where a Trash put a file: a path inside it (the item macOS returns, the
/// file under the freedesktop `files/`, the Recycle Bin's item).
#[must_use]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrashReceipt {
    pub location: PathBuf,
}

/// Why a Trash didn't take a file. The file is where it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrashError {
    /// The volume has no Trash that can take it; nothing is deleted
    /// instead.
    Unsupported,
    Failed(String),
}

impl fmt::Display for TrashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => f.write_str("Its volume has no Trash."),
            Self::Failed(e) => f.write_str(e),
        }
    }
}

/// Whether a folder's drive has a Trash, as far as can be told without
/// trying or writing anything. Only a sure [`TrashSupport::No`] refuses a
/// plan; the Trash itself still fails closed, so a wrong `Yes` or
/// `Unknown` costs a refusal when it runs, never a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrashSupport {
    Yes,
    /// Its Trash would refuse what is on it.
    No,
    /// It can't be told before trying.
    Unknown,
}

/// The system Trash. Implemented by the app over the OS, which is trusted
/// to have moved the file when it gives a receipt; the catalog still
/// removes rows only once the file is gone from its path as well.
pub trait Trash {
    fn trash(&self, path: &std::path::Path) -> Result<TrashReceipt, TrashError>;
    /// Whether `path`, a file or folder, can go to this Trash, asked
    /// without trying: by default it can't be told.
    fn supports(&self, _path: &std::path::Path) -> TrashSupport {
        TrashSupport::Unknown
    }
}

/// Why file operations can't run.
#[derive(Debug)]
pub enum FileOpError {
    /// Another RAWmakase window is changing files in this catalog.
    Busy,
    /// They can't be made safe for this catalog or here; the reason says
    /// why.
    Unsupported(&'static str),
    /// The plan is of another catalog, or another computer.
    WrongCatalog,
    /// The unfinished operation is another computer's to decide.
    OtherComputer,
    /// The destination can't take files.
    Destination(String),
    /// The destination drive hasn't room for the bytes copied there.
    NoSpace { needed: u64, available: u64 },
    /// It was stopped before it finished.
    Cancelled,
    /// The catalog couldn't be read or written.
    Catalog(anyhow::Error),
}

impl From<anyhow::Error> for FileOpError {
    fn from(e: anyhow::Error) -> Self {
        Self::Catalog(e)
    }
}

impl fmt::Display for FileOpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => {
                f.write_str("Another RAWmakase window is changing files in this catalog.")
            }
            Self::Unsupported(why) => f.write_str(why),
            Self::WrongCatalog => f.write_str("The operation was planned for another catalog."),
            Self::OtherComputer => {
                f.write_str("The operation changed another computer's files; finish it there.")
            }
            Self::Destination(why) => f.write_str(why),
            Self::NoSpace { needed, available } => write!(
                f,
                "The photos need {} on the other drive, and {} must stay free there; it has {} free.",
                size(*needed),
                size(plan::SPARE),
                size(*available)
            ),
            Self::Cancelled => f.write_str("Stopped."),
            Self::Catalog(e) => write!(f, "{e:#}"),
        }
    }
}

impl std::error::Error for FileOpError {}

/// `bytes` for people, in decimal units as Finder and Explorer show drives.
fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} bytes");
    }
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// Plans `op` in `catalog` on this computer, as [`plan`](fn@plan) does, with the
/// `trash` it would run with: the only way to plan [`FileOp::TrashFolder`],
/// which refuses a drive `trash` says has none.
pub fn plan_with_trash(
    catalog: &super::Catalog,
    op: FileOp,
    trash: &dyn Trash,
) -> Result<Plan, FileOpError> {
    plan::plan_trash_with(catalog, op, &disk::Real, trash)
}

/// What became of one item of a run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemResult {
    /// Its rows left the catalog.
    Removed,
    /// Its files and rows are in folder `to` now.
    Moved { to: FolderId },
    /// Its files are in the Trash and its rows left the catalog.
    Trashed,
    /// Its files are renamed: the photo is named `name` now.
    Renamed { name: String },
    /// The folder is renamed, with the catalog's folders below it; their
    /// photos' paths changed with it.
    FolderRenamed,
    /// The folder is moved, with the catalog's folders below it; their
    /// photos' paths changed with it.
    FolderMoved,
    /// The folder is made, and in the catalog as `folder`.
    Created { folder: FolderId },
    /// Nothing changed, for this reason when there is one.
    Unchanged(Option<String>),
    /// It is unfinished and waits for the user (see [`recover`](fn@recover)).
    NeedsRecovery(String),
    /// The user left it unfinished.
    Kept,
    /// Its thread stopped before it finished; [`recover`](fn@recover) decides it.
    Interrupted,
}

/// One item's outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemOutcome {
    pub operation: OperationId,
    /// The catalog photos it concerns: removed, moved or untouched as
    /// [`Self::result`] says. Removed ids may be given to new photos.
    pub photos: Vec<PhotoId>,
    pub result: ItemResult,
    /// Of a move to another drive, the originals left where they were
    /// beside their copies, the catalog following the copies.
    pub originals_kept: Vec<KeptOriginal>,
    /// Of a folder put in the Trash, what it also took that was added after
    /// it was checked, when that can be read.
    pub note: Option<String>,
}

/// An original a move to another drive left at its source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeptOriginal {
    pub path: PathBuf,
    pub why: String,
}

/// What a run did, read from the journal: the same after success, an
/// error, a panic or a lost thread. Items a cancel or an error stopped
/// before they were journalled are not in it, and are untouched.
#[must_use]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub items: Vec<ItemOutcome>,
    /// Why the run stopped early, when the catalog couldn't be written: the
    /// items after it are untouched. Only [`execute`](fn@execute) knows it.
    pub stopped: Option<String>,
}

/// The outcome of `batch` in `catalog`, as journalled.
pub fn outcome(catalog: &super::Catalog, batch: &BatchId) -> anyhow::Result<Outcome> {
    let items = journal::of_batch(&catalog.db, &batch.0)?;
    Ok(Outcome {
        items: items.iter().map(recover::outcome_of).collect(),
        stopped: None,
    })
}

#[cfg(test)]
mod arrive_tests;
#[cfg(test)]
mod batch_tests;
#[cfg(test)]
mod copy_tests;
#[cfg(test)]
mod folder_tests;
#[cfg(test)]
mod journal_tests;
#[cfg(test)]
mod lock_tests;
#[cfg(test)]
mod move_folder_tests;
#[cfg(test)]
mod move_root_tests;
#[cfg(test)]
mod nest_root_tests;
#[cfg(test)]
mod plan_tests;
#[cfg(test)]
mod recover_tests;
#[cfg(test)]
mod rename_tests;
#[cfg(test)]
mod rules_tests;
#[cfg(test)]
mod stack_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod throughput_tests;
#[cfg(test)]
mod tree_copy_tests;
#[cfg(test)]
mod tree_tests;
#[cfg(test)]
mod tree_trash_tests;
