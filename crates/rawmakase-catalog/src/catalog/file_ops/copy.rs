//! Moving a photo's files to another volume, where no rename reaches: each
//! file copied, verified and given its name there, then the catalog, then
//! the originals to the Trash. Every step is journalled before the next.
//!
//! Per member (the photo first, then its companions):
//! 1. its partial copy, hidden in the destination folder by a name unique
//!    to the run, the item and the member, made where nothing has that
//!    name, and its file object journalled (`copying`);
//! 2. the original's bytes copied into it, hashed as they are read
//!    (stopping between chunks when the run is stopped), the hash
//!    journalled, synced, given the original's extras (extended
//!    attributes: Finder tags, FinderInfo, a resource fork; alternate data
//!    streams on Windows), time and permission bits, and synced again,
//!    each through the handle it was made through (#675);
//! 3. read back and compared with the hash, its extras with the
//!    original's, and the original checked to be the file object and
//!    `Identity` journalled, with the extras it had before they were copied;
//! 4. renamed exclusively to its name there, the folder synced, and the
//!    member journalled `copied`.
//!
//! Then, in one transaction, the evidence (every copy at its destination,
//! every original still the file it copied), the rows following the
//! copies, the photo's saved edit and checksum re-keyed to the copy when
//! its volume keeps times more coarsely, and the item `committed`. Last,
//! each original still that file, its extras still its copy's, goes to the
//! Trash (`trashed`), or stays, with why (`original_kept`); the item is
//! `done`. A drive that can't keep an original's extras undoes the item.
//!
//! Anything that fails before the commit undoes the item: partial copies
//! are removed, complete ones put in the Trash, and nothing else changes;
//! on an SMB share, which has no Trash, copies stay, for the user to
//! delete ([`LEFT_ON_SHARE`]).
//! The partial copy is journalled as the disk spells its name (HFS+ keeps
//! names decomposed) and found by any spelling of it with its file object.
//! Recovery does the same for an item a crash left before its commit
//! ([`decide`]), and finishes one left after it.
use super::super::Catalog;
use super::super::db::{Write, sql};
use super::super::sidecar::caseless;
use super::companions::is_legacy_edit;
use super::disk::{Disk, Presence, gone, parent, presence, spelled};
use super::journal::{
    self, CopyRecord, Item, ItemState, Member, MemberState, OperationId, Subject,
};
use super::plan::{self, Folders, occupied, shared};
use super::proof::{self, Proof, Proofs};
use super::recover::NOT_BACK;
use super::{Trash, TrashError};
use crate::fsops::{ContentHash, CopyError, EntryKind, FileId, FsError};
use crate::storage::Identity;
use anyhow::{Context, Result};
use rawmakase_cancel::Cancel;
use std::fmt;
use std::path::{Path, PathBuf};

/// What copying needs of a run or a recovery.
pub(super) struct Cx<'a> {
    pub computer: &'a str,
    pub folders: &'a Folders,
    pub disk: &'a dyn Disk,
    pub trash: &'a dyn Trash,
}

/// Why a move to another volume changed nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Problem {
    /// The run was stopped.
    Stopped,
    /// The destination drive is full.
    NoSpace,
    /// The original changed, or went, while it was copied.
    Changed(PathBuf),
    /// The copy read back otherwise than the original was read.
    Mismatch(PathBuf),
    /// A file of that name came to the destination meanwhile.
    NameTaken(String),
    /// The destination drive didn't keep the file's time.
    TimeNotKept(String),
    /// The destination drive can't keep the file's extras (extended
    /// attributes, alternate data streams), or didn't.
    ExtrasNotKept(String),
    /// The photo has an edit file of an earlier release, which names the
    /// photo's time, and the destination drive keeps times more coarsely.
    LegacyEdit(String),
    Disk(String),
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stopped => f.write_str("Stopped."),
            Self::NoSpace => f.write_str("The other drive is full."),
            Self::Changed(p) => write!(f, "{} changed while it was copied.", p.display()),
            Self::Mismatch(p) => write!(
                f,
                "The copy of {} didn't match the original, so nothing was moved.",
                p.display()
            ),
            Self::NameTaken(n) => write!(f, "A file named {n} is there now."),
            Self::TimeNotKept(n) => {
                write!(f, "The other drive didn't keep the date of {n}.")
            }
            Self::ExtrasNotKept(n) => write!(
                f,
                "The other drive can't keep what {n} has besides its contents (Finder tags, extended attributes or alternate data streams), so nothing was moved."
            ),
            Self::LegacyEdit(n) => write!(
                f,
                "{n} was edited by an earlier release, whose edit file the other drive's dates wouldn't match. Open and save its edit first."
            ),
            Self::Disk(e) => f.write_str(e),
        }
    }
}

/// Why an undone item is unfinished: copies of it are left on an SMB share
/// (#675). Nothing removes a name there only while it is still the file
/// proven ours, and another computer could put its own file at it between
/// the proof and the removal.
pub(super) const LEFT_ON_SHARE: &str = "its copies on the network share were left there for you to delete: removing one could remove a file another computer had just put at its name";

/// Why an original whose extras aren't its copy's stays.
pub(super) const EXTRAS_DIFFER: &str =
    "its Finder tags or other extended attributes aren't its copy's any more";

/// Why file or folder `original` must stay: its extras aren't `copy`'s, or
/// can't be read; `None` when they are.
pub(super) fn extras_differ(disk: &dyn Disk, original: &Path, copy: &Path) -> Option<String> {
    match (disk.extras(original), disk.extras(copy)) {
        (Ok(a), Ok(b)) if a == b => None,
        (Ok(_), Ok(_)) => Some(EXTRAS_DIFFER.into()),
        (Err(e), _) | (_, Err(e)) => Some(format!("its extended attributes can't be read: {e}")),
    }
}

/// Gives file or folder `to` the extras of `from`, read as `extras`, and
/// checks them there: [`Problem::Changed`] when `from`'s are no longer
/// those, [`Problem::ExtrasNotKept`] when `to`'s volume can't keep them or
/// reads back others. A file's through `into`, the handle it was made
/// through, where given.
pub(super) fn carry_extras(
    disk: &dyn Disk,
    from: &Path,
    (to, into): (&Path, Option<&std::fs::File>),
    extras: ContentHash,
) -> Result<(), Problem> {
    let not_kept = || Problem::ExtrasNotKept(name(from));
    disk.copy_extras(from, to, into).map_err(|e| match e {
        FsError::Unsupported => not_kept(),
        e => problem(e),
    })?;
    if disk.extras(from).map_err(problem)? != extras {
        return Err(Problem::Changed(from.to_path_buf()));
    }
    if disk.extras(to).map_err(problem)? != extras {
        return Err(not_kept());
    }
    Ok(())
}

fn problem(e: FsError) -> Problem {
    match e {
        FsError::NoSpace => Problem::NoSpace,
        e => Problem::Disk(e.to_string()),
    }
}

/// Why a member's copy didn't finish: a disk step, or the journal.
pub(super) enum Failed {
    Problem(Problem),
    Journal(anyhow::Error),
}

impl From<Problem> for Failed {
    fn from(p: Problem) -> Self {
        Self::Problem(p)
    }
}

/// Copies journalled item `id`, planned as `item`, then lets the catalog
/// follow and puts its originals in the Trash; undoes it on any failure
/// before the catalog follows. Errs only when the catalog can't be written.
pub(super) fn run(
    cx: &Cx<'_>,
    catalog: &mut Catalog,
    item: &plan::Item,
    id: OperationId,
    cancel: Cancel<'_>,
) -> Result<()> {
    let temps = item.copy.as_ref().map_or(&[][..], |c| &c.temps[..]);
    for (ordinal, m) in item.members.iter().enumerate() {
        let (Some(to), Some(temp)) = (m.destination.as_deref(), temps.get(ordinal)) else {
            return undo(cx, catalog, id, "it has no destination");
        };
        let proofs = item.preconditions.proofs;
        let copied = copy_one(
            cx,
            catalog,
            (id, ordinal as i64),
            m,
            (temp, to),
            proofs,
            cancel,
        );
        match copied {
            Ok(()) => {}
            Err(Failed::Problem(p)) => return undo(cx, catalog, id, &p.to_string()),
            Err(Failed::Journal(e)) => return undo(cx, catalog, id, &format!("{e:#}")),
        }
    }
    let committed = catalog.db.write_immediate(|w| {
        let journalled = journal::item(w, id)?.context("the journalled item is gone")?;
        commit(w, cx, &journalled)
    });
    match committed {
        Ok(None) => {}
        Ok(Some(why)) => return undo(cx, catalog, id, &why),
        Err(e) => return undo(cx, catalog, id, &format!("{e:#}")),
    }
    let journalled = journal::item(&catalog.db, id)?.context("the journalled item is gone")?;
    finish(cx, catalog, &journalled, false)
}

/// Steps 1 to 4 for member `ordinal` of item `id`, planned as `m`, through
/// its partial copy `temp` to `to` (the same, where the copy is made at its
/// name), proven as `proofs` says.
fn copy_one(
    cx: &Cx<'_>,
    catalog: &mut Catalog,
    (id, ordinal): (OperationId, i64),
    m: &journal::NewMember,
    (temp, to): (&Path, &Path),
    proofs: Proofs,
    cancel: Cancel<'_>,
) -> Result<(), Failed> {
    let disk = cx.disk;
    let (temp, made) = write(
        cx,
        catalog,
        (id, ordinal),
        (&m.source, m.id, &m.identity),
        temp,
        proofs,
        cancel,
    )?;
    // 4. Its name, never replacing one there; made at it already where the
    // volume can't rename exclusively.
    let at = match proofs.direct {
        true => temp,
        false => {
            disk.rename(&temp, to).map_err(|e| match e {
                FsError::AlreadyExists => Problem::NameTaken(name(to)),
                e => problem(e),
            })?;
            disk.sync_dir(parent(to)).map_err(problem)?;
            // As the disk spells it, which a volume that normalises names
            // may not as given.
            match proofs.copies {
                Proof::Ids => spelled(disk, to, made),
                Proof::Names => proof::spelled(disk, to),
            }
            .map_err(problem)?
        }
    };
    catalog
        .db
        .write(|w| journal::set_copied(w, id, ordinal, &at))
        .map_err(Failed::Journal)?;
    Ok(())
}

/// Steps 1 to 3 for member `ordinal` of item `id`: the original `source`
/// (file object and `Identity` as planned) copied into a file made at
/// `temp` where nothing had its name, journalled as it goes, synced, given
/// the original's time, read back, and the original checked to be still
/// what was copied, each side as `proofs` says. Returns the copy's path as
/// the disk spells it, and its file object (which proves nothing where its
/// volume's ids don't stay).
pub(super) fn write(
    cx: &Cx<'_>,
    catalog: &mut Catalog,
    (id, ordinal): (OperationId, i64),
    (source, source_id, identity): (&Path, FileId, &Identity),
    temp: &Path,
    proofs: Proofs,
    cancel: Cancel<'_>,
) -> Result<(PathBuf, FileId), Failed> {
    let disk = cx.disk;
    let journal = |catalog: &mut Catalog, f: &dyn Fn(&mut Write<'_>) -> Result<()>| {
        catalog.db.write(|w| f(w)).map_err(Failed::Journal)
    };
    if cancel.is_cancelled() {
        return Err(Problem::Stopped.into());
    }
    // 1. Made where nothing is; what a crash leaves before it is journalled
    // is found by its name, empty. Not made, nothing of it is there: what
    // is (a file another program made at its name, which may be the
    // photo's own name where copies are made at it) is never taken for it.
    // Copied through the handle it was made through: into this file,
    // whatever another computer puts at its name meanwhile (on a share).
    let mut into = match disk.create_file(temp) {
        Ok(into) => into,
        Err(e) => {
            journal(catalog, &|w| journal::set_discarded(w, id, ordinal, None))?;
            return Err(match e {
                FsError::AlreadyExists => Problem::NameTaken(name(temp)),
                e => problem(e),
            }
            .into());
        }
    };
    // The file object made, from its handle; its name must still be it.
    let made = disk.handle_id(temp, &into).map_err(problem)?;
    let named = disk.entry(temp).map_err(problem)?;
    if named.kind != EntryKind::File || named.links != 1 || named.id != made {
        return Err(Problem::Disk(format!("{} isn't the file made", temp.display())).into());
    }
    // As the disk spells it (HFS+ keeps names decomposed), which every
    // later step and recovery find it by. Its file object only where it
    // stays.
    let file = (proofs.copies == Proof::Ids).then_some(made);
    let temp = match file {
        Some(file) => spelled(disk, temp, file),
        None => proof::spelled(disk, temp),
    }
    .map_err(problem)?;
    // Where names prove it, a file object no file has: a release before
    // #573 reading the journal finds no file it is, and leaves it for the
    // user.
    let journalled = file.unwrap_or(FileId::NOBODY);
    journal(catalog, &|w| {
        journal::set_copying(w, id, ordinal, Some(journalled), &temp)
    })?;
    // 2. Copied, durable, with the original's time.
    let content = disk
        .copy_into(source, &temp, &mut into, &mut |_| !cancel.is_cancelled())
        .map_err(|e| match e {
            CopyError::Stopped => Problem::Stopped,
            CopyError::NotTheFile => Problem::Disk(e.to_string()),
            CopyError::Fs(e) => problem(e),
        })?;
    // Written whole: from here it is never removed while its original
    // can't be proven, whatever fails next.
    journal(catalog, &|w| journal::set_content(w, id, ordinal, content))?;
    // Durable, with its extras, time and permissions, through the handle
    // too, not given to a file another computer puts at its name
    // meanwhile (#675), where the system keeps to the handle (a share's
    // drivers don't: see `fsops::copy`). Such a file fails the read-back.
    disk.sync_file(&temp, &into).map_err(problem)?;
    // Its extras before its time and permissions: writing a resource fork
    // changes a file's time, and a read-only file takes no more.
    let extras = disk.extras(source).map_err(problem)?;
    carry_extras(disk, source, (&temp, Some(&into)), extras)?;
    disk.copy_times(source, &temp, Some(&into))
        .map_err(problem)?;
    drop(into);
    // 3. Read back, and the original still what was copied.
    if disk.hash(&temp).map_err(problem)? != content {
        return Err(Problem::Mismatch(source.to_path_buf()).into());
    }
    let id_proves = (proofs.originals == Proof::Ids).then_some(source_id);
    if !unchanged(disk, source, id_proves, identity) {
        return Err(Problem::Changed(source.to_path_buf()).into());
    }
    Ok((temp, made))
}

/// Whether `path` is a file with `identity` (and file object `id`, where
/// that proves it), looked up by its name: what checks the original between
/// its copy's steps.
fn unchanged(disk: &dyn Disk, path: &Path, id: Option<FileId>, identity: &Identity) -> bool {
    disk.entry(path)
        .is_ok_and(|e| e.kind == EntryKind::File && id.is_none_or(|id| e.id == id))
        && disk.identity(path).ok().as_ref() == Some(identity)
}

/// Whether member `m`'s original is proven at its source as it was copied:
/// that file object, by its exact name in its folder's listing, with its
/// journalled `Identity`.
pub(super) fn original_here(disk: &dyn Disk, m: &Member) -> bool {
    original_proven(disk, m, Proof::Ids)
}

/// [`original_here`], as `proof` says: by its exact name alone where file
/// ids don't stay.
fn original_proven(disk: &dyn Disk, m: &Member, proof: Proof) -> bool {
    let here = match (proof, m.id) {
        (Proof::Ids, Some(id)) => presence(disk, &m.source, id) == Presence::Here,
        (Proof::Ids, None) => false,
        (Proof::Names, _) => proof::presence(disk, &m.source) == Presence::Here,
    };
    here && m.identity.is_some() && disk.identity(&m.source).ok() == m.identity
}

/// Where member `m`'s original is, as `proof` says.
fn source_presence(disk: &dyn Disk, m: &Member, proof: Proof) -> Presence {
    match (proof, m.id) {
        (Proof::Ids, Some(id)) => presence(disk, &m.source, id),
        (Proof::Ids, None) => Presence::Unknown,
        (Proof::Names, _) => proof::presence(disk, &m.source),
    }
}

/// The proofs journalled `item` was copied with.
fn proofs_of(item: &Item) -> Proofs {
    match &item.subject {
        Subject::Photos(pre) => pre.proofs,
        _ => Proofs::default(),
    }
}

/// Whether `copy`'s time differs from `original`'s only as a volume that
/// keeps times more coarsely makes it (exFAT's 10 ms, FAT's 2 s, HFS+'s
/// 1 s, NTFS's 100 ns, and the like), its size and first bytes the same.
pub(super) fn coarser(original: &Identity, copy: &Identity) -> bool {
    const STEPS: [u128; 6] = [
        100,
        1_000,
        1_000_000,
        10_000_000,
        1_000_000_000,
        2_000_000_000,
    ];
    original.size == copy.size
        && original.prefix_hash == copy.prefix_hash
        && STEPS.iter().any(|step| {
            copy.modified_ns.is_multiple_of(*step)
                && copy.modified_ns.abs_diff(original.modified_ns) < *step
        })
}

/// Lets the catalog follow item `item`'s copies, in `w`'s transaction, if
/// the evidence allows, and marks it `committed`; `Some` with why not
/// otherwise, having changed nothing.
fn commit(w: &mut Write<'_>, cx: &Cx<'_>, item: &Item) -> Result<Option<String>> {
    let Subject::Photos(pre) = &item.subject else {
        return Ok(Some("not a photo operation".into()));
    };
    if let Some(why) = pre.check(w, cx.computer)? {
        return Ok(Some(why.into()));
    }
    let (true, Some(photo)) = (pre.to.is_some() || pre.into.is_some(), item.members.first()) else {
        return Ok(Some("it has no destination".into()));
    };
    let to = pre.to.as_ref().map(|(to, _)| *to);
    let filename = photo.source.file_name().unwrap_or_default();
    if shared(
        w,
        cx.folders,
        parent(&photo.source),
        filename,
        photo.id,
        &pre.rows,
    )? {
        return Ok(Some(
            "another photo in the catalog is the same file now".into(),
        ));
    }
    // Every copy at its destination, every original still what it copied.
    let mut copies = Vec::new();
    for m in &item.members {
        let (Some(at), Some(copy)) = (m.destination.as_deref(), &m.copy) else {
            return Ok(Some("it has no destination".into()));
        };
        let here = match pre.proofs.copies {
            Proof::Ids => copy
                .id
                .is_some_and(|id| presence(cx.disk, at, id) == Presence::Here),
            Proof::Names => copy.content.is_some_and(|c| proof::holds(cx.disk, at, c)),
        };
        if m.state != MemberState::Copied || !here {
            return Ok(Some("its copies aren't all where they were made".into()));
        }
        if !original_proven(cx.disk, m, pre.proofs.originals) {
            return Ok(Some(Problem::Changed(m.source.clone()).to_string()));
        }
        let (Ok(now), Some(was)) = (cx.disk.identity(at), &m.identity) else {
            return Ok(Some(format!("{} can't be read", at.display())));
        };
        if now != *was && !coarser(was, &now) {
            return Ok(Some(Problem::TimeNotKept(name(at)).to_string()));
        }
        copies.push(now);
    }
    let (Some(was), Some(now)) = (&photo.identity, copies.first()) else {
        return Ok(Some("it has no files".into()));
    };
    let rekey = was != now;
    // An earlier release's edit file names the photo's time, which the
    // copy no longer has.
    let legacy = item.members[1..]
        .iter()
        .any(|m| is_legacy_edit(filename, m.source.file_name().unwrap_or_default()));
    if rekey && legacy {
        return Ok(Some(Problem::LegacyEdit(name(&photo.source)).to_string()));
    }
    let Some(arrived) = photo.destination.as_deref() else {
        return Ok(Some("it has no destination".into()));
    };
    let arrived_name = arrived.file_name().unwrap_or_default();
    if occupied(w, cx.folders, to, parent(arrived), arrived_name, &pre.rows)? {
        return Ok(Some(format!(
            "a photo of the catalog is named {} there now",
            arrived_name.to_string_lossy()
        )));
    }
    let text = arrived_name
        .to_str()
        .context("not a name the catalog keeps")?;
    let to = match super::execute::arrive(w, cx.computer, item.id, pre, parent(arrived))? {
        Ok(to) => to,
        Err(why) => return Ok(Some(why)),
    };
    crate::catalog::stacks::leave_folder(w, pre.master, to)?;
    w.execute(
        sql!("UPDATE photos SET folder=?, filename=? WHERE id=? OR master_id=?"),
        &[&to, &text, &pre.master, &pre.master],
    )?;
    // The same bytes, kept with a coarser time: what was saved for the
    // original is the copy's. Only what names exactly the original.
    if rekey {
        for row in &pre.rows {
            super::super::edit_rows::refresh_identity(w, *row, was, now)?;
        }
        super::super::checksums::refresh_identity(w, pre.master, was, now)?;
    }
    for (m, now) in item.members.iter().zip(&copies) {
        journal::set_copy_identity(w, item.id, m.ordinal, now)?;
    }
    journal::set_state(w, item.id, ItemState::Committed, None)?;
    Ok(None)
}

fn name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// Puts committed item `item`'s originals in the Trash, each only while it
/// is still the file it copied and its copy is where the catalog says;
/// one that can't go stays, with why. Recovery (`recovering`) also reads
/// each copy back first. Marks it `done`.
fn finish(cx: &Cx<'_>, catalog: &mut Catalog, item: &Item, recovering: bool) -> Result<()> {
    let id = item.id;
    let proofs = proofs_of(item);
    let mut unsynced = None;
    for m in &item.members {
        if m.state != MemberState::Copied {
            continue;
        }
        let kept = |catalog: &mut Catalog, why: &str| {
            catalog
                .db
                .write(|w| journal::set_kept(w, id, m.ordinal, why))
        };
        let (Some(at), Some(copy), Some(file)) = (m.destination.as_deref(), &m.copy, m.id) else {
            kept(catalog, "its copy isn't journalled")?;
            continue;
        };
        let named = match proofs.copies {
            Proof::Ids => copy
                .id
                .is_some_and(|c| presence(cx.disk, at, c) == Presence::Here),
            Proof::Names => proof::presence(cx.disk, at) == Presence::Here,
        };
        let copy_here =
            named && copy.identity.is_some() && cx.disk.identity(at).ok() == copy.identity;
        // Where names prove it, its bytes always.
        let read_back = (!recovering && proofs.copies == Proof::Ids)
            || copy
                .content
                .is_some_and(|c| cx.disk.hash(at).is_ok_and(|now| now == c));
        if !copy_here || !read_back {
            kept(catalog, "its copy isn't as it was made")?;
            continue;
        }
        match source_presence(cx.disk, m, proofs.originals) {
            Presence::Here => {}
            // In the Trash, where a crash came before its receipt was
            // journalled, or gone some other way: nothing is left to do.
            Presence::Absent => {
                catalog
                    .db
                    .write(|w| journal::set_member(w, id, m.ordinal, MemberState::Trashed, None))?;
                continue;
            }
            Presence::Unknown => {
                kept(catalog, "whether it is still there can't be told")?;
                continue;
            }
        }
        // Where names prove it, its bytes too: the ones copied.
        let unchanged = original_proven(cx.disk, m, proofs.originals)
            && (proofs.originals == Proof::Ids
                || copy
                    .content
                    .is_some_and(|c| cx.disk.hash(&m.source).is_ok_and(|now| now == c)));
        if !unchanged {
            kept(catalog, "it changed since it was copied")?;
            continue;
        }
        if let Some(why) = extras_differ(cx.disk, &m.source, at) {
            kept(catalog, &why)?;
            continue;
        }
        // Its name as the plan took it, composed where names prove it; a
        // file stored decomposed, which those drivers take only so, by
        // that spelling of the one file proven there.
        let trashed = cx.trash.trash(&m.source).or_else(|e| {
            match (proofs.originals, proof::other_spelling(cx.disk, &m.source)) {
                (Proof::Names, Some(other)) => cx.trash.trash(&other),
                _ => Err(e),
            }
        });
        let receipt = match trashed {
            Ok(receipt) => receipt.location,
            Err(e @ TrashError::Unsupported) => {
                kept(catalog, &e.to_string())?;
                continue;
            }
            Err(TrashError::Failed(e)) => {
                kept(catalog, &e)?;
                continue;
            }
        };
        if let Err(e) = cx.disk.sync_dir(parent(&m.source)) {
            unsynced = Some(format!("{}: {e}", parent(&m.source).display()));
        }
        catalog.db.write(|w| {
            journal::set_member(w, id, m.ordinal, MemberState::Trashed, Some(&receipt))
        })?;
        let left = match proofs.originals {
            Proof::Ids => !gone(cx.disk, &m.source, file),
            Proof::Names => !proof::gone(cx.disk, &m.source),
        };
        if left {
            kept(catalog, "the Trash took it, but it is still there")?;
        }
    }
    let note = unsynced.map(|d| format!("its folder couldn't be synced: {d}"));
    catalog
        .db
        .write(|w| journal::set_state(w, id, ItemState::Done, note.as_deref()))
}

/// What must hold of an item's originals for its copies to be discarded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Originals {
    /// Each the file object and `Identity` it was copied as: whatever is
    /// undone on its own.
    AsCopied,
    /// Each the file object it was, changed or not: what the user's Put
    /// Back asks, the originals being where they were.
    Present,
}

/// Whether every one of `item`'s originals is at its source as `rule` asks.
fn originals(disk: &dyn Disk, item: &Item, rule: Originals) -> bool {
    let proof = proofs_of(item).originals;
    item.members.iter().all(|m| match rule {
        Originals::AsCopied => original_proven(disk, m, proof),
        Originals::Present => source_presence(disk, m, proof) == Presence::Here,
    })
}

/// Undoes journalled item `id` before the catalog follows it, for `why`:
/// decided first (`restoring`), then each member's copies discarded; it
/// ends `no_change`, or stays being put back when a copy can't be.
pub(super) fn undo(cx: &Cx<'_>, catalog: &mut Catalog, id: OperationId, why: &str) -> Result<()> {
    undo_as(cx, catalog, id, why, Originals::AsCopied)
}

/// [`undo`], the originals held to `rule`.
fn undo_as(
    cx: &Cx<'_>,
    catalog: &mut Catalog,
    id: OperationId,
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

/// Where a member's partial copy is.
enum Partial {
    /// Not there.
    Gone,
    /// There, at this name, and proven this run's.
    Ours(PathBuf),
    Unknown,
}

/// Where member `m`'s partial copy `temp` is, by any name its folder's
/// listing has that `sidecar::caseless` takes for `temp`'s (a volume may
/// spell it otherwise than it was given): by its journalled file object
/// ([`copy_at`]); or, made but not journalled yet, the one such name, only
/// while it is an empty plain file.
fn partial(disk: &dyn Disk, m: &Member, temp: &Path, proofs: Proofs) -> Partial {
    if proofs.copies == Proof::Names {
        return partial_named(disk, m, temp, proofs.since);
    }
    if let Some(id) = m.copy.as_ref().and_then(|c| c.id) {
        return match copy_at(disk, temp, id) {
            Some(Some(at)) => Partial::Ours(at),
            Some(None) => Partial::Gone,
            None => Partial::Unknown,
        };
    }
    let (dir, Some(name)) = (parent(temp), temp.file_name()) else {
        return Partial::Unknown;
    };
    let Ok(names) = disk.list(dir) else {
        return Partial::Unknown;
    };
    let wanted = caseless(name);
    let alike: Vec<&std::ffi::OsString> = names.iter().filter(|n| caseless(n) == wanted).collect();
    let at = match alike.as_slice() {
        [] => return Partial::Gone,
        [one] => dir.join(one),
        _ => return Partial::Unknown,
    };
    let empty = disk
        .entry(&at)
        .is_ok_and(|e| e.kind == EntryKind::File && e.links == 1)
        && disk.identity(&at).is_ok_and(|i| i.size == 0);
    match empty {
        true => Partial::Ours(at),
        false => Partial::Unknown,
    }
}

/// [`partial`] where names prove copies (#573), at exactly the name the
/// journal gave it: made and journalled, while it holds the bytes copied
/// (written whole) or its original's first bytes (part way); made but not
/// journalled, only while empty and made after the item was planned.
fn partial_named(disk: &dyn Disk, m: &Member, temp: &Path, since: i64) -> Partial {
    let Some(copy) = &m.copy else {
        return Partial::Unknown;
    };
    match proof::presence(disk, temp) {
        Presence::Absent => Partial::Gone,
        Presence::Unknown => Partial::Unknown,
        Presence::Here => {
            let ours = match (m.state, copy.content) {
                (MemberState::Planned, _) => proof::made_after(disk, temp, since),
                (_, Some(content)) => proof::holds(disk, temp, content),
                (_, None) => proof::begins(disk, temp, &m.source),
            };
            match ours {
                true => Partial::Ours(temp.to_path_buf()),
                false => Partial::Unknown,
            }
        }
    }
}

/// Where member `m`'s complete copy, given the name `at`, is now, as
/// `proofs` says: by its file object ([`copy_at`]), or where names prove it
/// by the name alone, holding the bytes copied. `Some(None)` when it isn't
/// there, `None` when that can't be told.
fn complete_at(
    disk: &dyn Disk,
    at: &Path,
    copy: &CopyRecord,
    proofs: Proofs,
) -> Option<Option<PathBuf>> {
    match proofs.copies {
        Proof::Ids => match copy.id {
            Some(id) => copy_at(disk, at, id),
            None => Some(None),
        },
        Proof::Names => match proof::presence(disk, at) {
            Presence::Absent => Some(None),
            Presence::Here if copy.content.is_some_and(|c| proof::holds(disk, at, c)) => {
                Some(Some(at.to_path_buf()))
            }
            _ => None,
        },
    }
}

/// Where the copy that is file object `id`, given the name `at`, is now:
/// `Some(Some(path))` at the one name in its folder's listing that
/// `sidecar::caseless` takes for `at`'s and that is that file object (a
/// volume may spell a name otherwise than it was given); `Some(None)` when
/// no such name is (the folder listed whole, on the copy's volume); `None`
/// when it can't be told, as when a file there has that name but is
/// another file object: on a volume that renumbers its files it may be the
/// copy, so its absence is never concluded from it.
pub(super) fn copy_at(disk: &dyn Disk, at: &Path, id: FileId) -> Option<Option<PathBuf>> {
    let dir = parent(at);
    let wanted = caseless(at.file_name()?);
    let mut ours = Vec::new();
    for name in disk
        .list(dir)
        .ok()?
        .iter()
        .filter(|n| caseless(n) == wanted)
    {
        match disk.entry(&dir.join(name)) {
            Ok(e) if e.id == id => ours.push(dir.join(name)),
            Ok(_) | Err(_) => return None,
        }
    }
    match ours.len() {
        0 => disk
            .entry(dir)
            .ok()
            .filter(|d| d.id.volume() == id.volume())
            .map(|_| None),
        1 => Some(ours.pop()),
        _ => None,
    }
}

/// Discards journalled `item`'s copies, newest first, each journalled
/// `discarded`: a partial copy is removed (one written whole only while
/// every original is proven at its source as it was copied, and to the
/// Trash instead on the user's Put Back); a complete one goes to the
/// Trash, only while it is proven the copy (its file object, its bytes the
/// original's), no photo of the catalog is named so there, and every
/// original is proven at its source; on an SMB share none is removed
/// ([`LEFT_ON_SHARE`]). Returns why not, unless every one is gone.
fn discard(
    cx: &Cx<'_>,
    catalog: &mut Catalog,
    item: &Item,
    rule: Originals,
) -> Result<Option<&'static str>> {
    let disk = cx.disk;
    let proofs = proofs_of(item);
    let originals = originals(disk, item, rule);
    let rows: &[_] = item.photos();
    let to = match &item.subject {
        Subject::Photos(pre) => pre.to.as_ref().map(|(to, _)| *to),
        _ => None,
    };
    // Copies left on a share, the user's to delete: said only once no
    // other copy stays, which may hold the only bytes left.
    let mut on_share = false;
    for m in item.members.iter().rev() {
        let Some(copy) = &m.copy else {
            return Ok(Some(NOT_BACK));
        };
        match m.state {
            MemberState::Discarded => continue,
            MemberState::Planned | MemberState::Copying | MemberState::Copied => {}
            _ => return Ok(Some(NOT_BACK)),
        }
        let mut receipt = None;
        match partial(disk, m, &copy.temp, proofs) {
            // Never renamed, so nothing of it is at its destination. One
            // written whole stays while its original can't be proven: it
            // may be the one copy of those bytes left.
            Partial::Ours(_) if copy.content.is_some() && !originals => return Ok(Some(NOT_BACK)),
            // Put Back with an original changed since: written whole, it has
            // the bytes the original had, perhaps the only ones left, so it
            // goes to the Trash, never unlinked; as does one made at its name
            // there, where the volume can't rename exclusively.
            // Made at its name there, it is at a visible name from the
            // start: one a photo of the catalog is named as now (Synchronize
            // Folder may have added it) stays; any not proven empty goes to
            // the Trash too.
            Partial::Ours(at)
                if proofs.direct
                    && occupied(
                        &catalog.db,
                        cx.folders,
                        to,
                        parent(&at),
                        at.file_name().unwrap_or_default(),
                        rows,
                    )? =>
            {
                return Ok(Some(NOT_BACK));
            }
            Partial::Ours(at)
                if (copy.content.is_some() && rule == Originals::Present)
                    || (proofs.direct && !disk.identity(&at).is_ok_and(|i| i.size == 0)) =>
            {
                receipt = match trash_copy(cx, &at, &m.source) {
                    Ok(r) => r,
                    Err(LEFT_ON_SHARE) => {
                        on_share = true;
                        continue;
                    }
                    Err(left) => return Ok(Some(left)),
                };
            }
            // Never removed there; the user's to delete only while its
            // original begins with its bytes (as many as it has).
            Partial::Ours(at) if proof::removes_nothing(disk, &at) => {
                match proof::begins(disk, &at, &m.source) {
                    true => on_share = true,
                    false => return Ok(Some(NOT_BACK)),
                }
                continue;
            }
            Partial::Ours(at) => {
                let removed = disk
                    .remove_file(&at)
                    .and_then(|()| disk.sync_dir(parent(&at)));
                if removed.is_err() {
                    return Ok(Some(NOT_BACK));
                }
            }
            Partial::Unknown => return Ok(Some(NOT_BACK)),
            Partial::Gone => {
                if let Some(asked) = m.destination.as_deref() {
                    match complete_at(disk, asked, copy, proofs) {
                        Some(None) => {}
                        None => return Ok(Some(NOT_BACK)),
                        Some(Some(at)) => {
                            let at = at.as_path();
                            let proven = originals
                                && copy
                                    .content
                                    .is_some_and(|c| disk.hash(at).is_ok_and(|now| now == c));
                            let filename = at.file_name().unwrap_or_default();
                            let named =
                                occupied(&catalog.db, cx.folders, to, parent(at), filename, rows)?;
                            if !proven || named {
                                return Ok(Some(NOT_BACK));
                            }
                            receipt = match trash_copy(cx, at, &m.source) {
                                Ok(r) => r,
                                Err(LEFT_ON_SHARE) => {
                                    on_share = true;
                                    continue;
                                }
                                Err(left) => return Ok(Some(left)),
                            };
                        }
                    }
                }
            }
        }
        catalog
            .db
            .write(|w| journal::set_discarded(w, item.id, m.ordinal, receipt.as_deref()))?;
    }
    Ok(on_share.then_some(LEFT_ON_SHARE))
}

/// Puts copy `at` of original `source` in the Trash, as an undone copy
/// goes. Where its volume has none it is removed instead, only while
/// `source` holds every byte it does, as many as it has
/// ([`proof::begins`]): then nothing is lost; but never on an SMB share
/// ([`proof::removes_nothing`]), where it stays, the user's to delete
/// ([`LEFT_ON_SHARE`]) only when that holds.
/// `Ok` once it is gone, with the Trash's receipt if the Trash took it;
/// why not while it stays.
pub(super) fn trash_copy(
    cx: &Cx<'_>,
    at: &Path,
    source: &Path,
) -> Result<Option<PathBuf>, &'static str> {
    let receipt = match cx.trash.trash(at) {
        Ok(r) => Some(r.location),
        // The user's to delete only while nothing would be lost.
        Err(TrashError::Unsupported) if proof::removes_nothing(cx.disk, at) => {
            return Err(match proof::begins(cx.disk, at, source) {
                true => LEFT_ON_SHARE,
                false => NOT_BACK,
            });
        }
        Err(TrashError::Unsupported) if proof::begins(cx.disk, at, source) => {
            cx.disk.remove_file(at).map_err(|_| NOT_BACK)?;
            None
        }
        Err(_) => return Err(NOT_BACK),
    };
    cx.disk.sync_dir(parent(at)).map_err(|_| NOT_BACK)?;
    Ok(receipt)
}

/// Recovery of copy item `item`: one the catalog follows is finished;
/// otherwise its originals are what counts, and it is undone once every
/// one is proven at its source, or left for the user.
/// Put Back (`put_back`, the user's choice) asks only that each original is
/// still the file object it was, changed or not.
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
    if !originals(cx.disk, item, rule) {
        let left = match item.state {
            ItemState::Restoring => ItemState::Restoring,
            _ => ItemState::NeedsRecovery,
        };
        let why = match put_back {
            true => "It can't be put back: an original isn't where it was.",
            false => "an original isn't where it was copied from",
        };
        return catalog
            .db
            .write(|w| journal::set_state(w, id, left, Some(why)));
    }
    let why = item.error.as_deref().unwrap_or("it was interrupted");
    undo_as(cx, catalog, id, why, rule)
}
