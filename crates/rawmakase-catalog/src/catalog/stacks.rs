//! Lightroom's folder stacks: photos of one folder grouped under one of them,
//! the top, and shown as it alone while the stack is collapsed. A stack is
//! organization only: no edit, rating or metadata of its own, and nothing of
//! it in a photo's recipe, previews or XMP.
//!
//! A photo moved to another folder leaves its stack, as Lightroom's folder
//! stacks hold the photos of one folder; renaming or moving a whole folder
//! keeps its stacks. A photo removed takes its row with it.
//!
//! The rows are read leniently, so no release can leave a stack this one
//! misreads: a stack is the rows of one `stack` whose photos are in the same
//! folder, in position order, and only while it has two photos or more. A
//! release that knows nothing of stacks removes rows with their photos
//! (`ON DELETE CASCADE`), the next photo becoming the top when the top goes,
//! and a photo it moves to another folder is left out of the stack there.
//! This module alone writes the rows, each change in the transaction of the
//! change that calls for it.
use super::db::{Reads, Write, sql};
use super::value::row;
use super::{Catalog, FolderId, PhotoId, StackId};
use anyhow::{Result, ensure};
use rawmakase_interop::exif::{Mark, Sequence};
use std::collections::{HashMap, HashSet};

/// A photo's place in its stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StackPlace {
    pub id: StackId,
    /// 0 for the top.
    pub index: usize,
    /// How many photos the stack has.
    pub count: usize,
    pub collapsed: bool,
}
impl StackPlace {
    pub fn is_top(&self) -> bool {
        self.index == 0
    }
}

/// One stack as it was: its photos, top first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedStack {
    pub photos: Vec<PhotoId>,
    pub collapsed: bool,
}

/// The stacks of some photos as they were, for Undo to put back: every
/// stack any of `photos` was in, with all of its photos, which `photos`
/// includes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StackState {
    pub photos: Vec<PhotoId>,
    pub stacks: Vec<SavedStack>,
}
impl StackState {
    /// Leaves out a photo now removed from the catalog, and a stack left
    /// with fewer than two photos.
    pub fn forget_photo(&mut self, id: PhotoId) {
        self.photos.retain(|p| *p != id);
        for stack in &mut self.stacks {
            stack.photos.retain(|p| *p != id);
        }
        self.stacks.retain(|s| s.photos.len() >= 2);
    }
}

row! {
    struct Member {
        stack: StackId,
        photo: PhotoId,
        folder: FolderId,
        collapsed: bool,
    }
}

/// A stack as the rows make it (see the module documentation).
#[derive(Debug)]
struct Group {
    id: StackId,
    folder: FolderId,
    collapsed: bool,
    photos: Vec<PhotoId>,
}

/// Every stack, from members ordered by stack, folder and position.
fn groups(members: Vec<Member>) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for m in members {
        match groups.last_mut() {
            Some(g) if g.id == m.stack && g.folder == m.folder => g.photos.push(m.photo),
            _ => groups.push(Group {
                id: m.stack,
                folder: m.folder,
                collapsed: m.collapsed,
                photos: vec![m.photo],
            }),
        }
    }
    groups.retain(|g| g.photos.len() >= 2);
    groups
}

fn all_groups(db: &impl Reads) -> Result<Vec<Group>> {
    Ok(groups(db.read(
        sql!(
            "SELECT m.stack, m.photo, p.folder, s.collapsed
             FROM stack_photos m JOIN photos p ON p.id=m.photo JOIN stacks s ON s.id=m.stack
             ORDER BY m.stack, p.folder, m.position, m.photo"
        ),
        &[],
    )?))
}

/// The stack `photo` is in, if any.
fn group_of(db: &impl Reads, photo: PhotoId) -> Result<Option<Group>> {
    let members: Vec<Member> = db.read(
        sql!(
            "SELECT m.stack, m.photo, p.folder, s.collapsed
             FROM stack_photos m JOIN photos p ON p.id=m.photo JOIN stacks s ON s.id=m.stack
             WHERE m.stack=(SELECT stack FROM stack_photos WHERE photo=?1)
               AND p.folder=(SELECT folder FROM photos WHERE id=?1)
             ORDER BY m.position, m.photo"
        ),
        &[&photo],
    )?;
    Ok(groups(members).pop())
}

/// Takes `photos` out of whatever rows they have.
fn leave(w: &mut Write<'_>, photos: &[PhotoId]) -> Result<()> {
    for photo in photos {
        w.execute(sql!("DELETE FROM stack_photos WHERE photo=?"), &[photo])?;
    }
    Ok(())
}

/// Removes stacks without photos left.
fn drop_empty(w: &mut Write<'_>) -> Result<()> {
    w.execute(
        sql!("DELETE FROM stacks WHERE id NOT IN (SELECT stack FROM stack_photos)"),
        &[],
    )?;
    Ok(())
}

/// Makes `photos` (top first, two or more, of one folder) a new stack.
fn create(w: &mut Write<'_>, photos: &[PhotoId], collapsed: bool) -> Result<StackId> {
    leave(w, photos)?;
    let id: StackId = w.insert_returning_id(
        sql!("INSERT INTO stacks(collapsed) VALUES (?) RETURNING id"),
        &[&collapsed],
    )?;
    place(w, id, photos)?;
    Ok(id)
}

/// Puts `photos` in stack `id` in this order, replacing their rows.
fn place(w: &mut Write<'_>, id: StackId, photos: &[PhotoId]) -> Result<()> {
    leave(w, photos)?;
    for (position, photo) in photos.iter().enumerate() {
        w.execute(
            sql!("INSERT INTO stack_photos(photo, stack, position) VALUES (?, ?, ?)"),
            &[photo, &id, &(position as i64)],
        )?;
    }
    Ok(())
}

/// Checks `photos` can be one stack: two or more, each once, all in the
/// catalog and in one folder.
fn check_stackable(w: &Write<'_>, photos: &[PhotoId]) -> Result<()> {
    ensure!(photos.len() >= 2, "A stack needs two photos or more");
    let unique: HashSet<_> = photos.iter().collect();
    ensure!(
        unique.len() == photos.len(),
        "A photo can't be stacked twice"
    );
    let mut folders = HashSet::new();
    for photo in photos {
        let folder: Option<FolderId> =
            w.read_optional(sql!("SELECT folder FROM photos WHERE id=?"), &[photo])?;
        let Some(folder) = folder else {
            anyhow::bail!("Unknown photo");
        };
        folders.insert(folder);
    }
    ensure!(
        folders.len() == 1,
        "Only photos in the same folder can be stacked"
    );
    Ok(())
}

/// Takes photo `family` and its virtual copies out of their stacks where
/// they are about to move to another folder than `to`; in the move's
/// transaction, before the move.
pub(super) fn leave_folder(w: &mut Write<'_>, family: PhotoId, to: FolderId) -> Result<()> {
    w.execute(
        sql!(
            "DELETE FROM stack_photos WHERE photo IN
             (SELECT id FROM photos WHERE (id=?1 OR master_id=?1) AND folder<>?2)"
        ),
        &[&family, &to],
    )?;
    drop_empty(w)
}

/// Puts a new virtual copy `copy` right after `of` in its stack, or makes
/// the two a stack, open so the copy shows; in the copy's transaction.
pub(super) fn stack_copy(w: &mut Write<'_>, of: PhotoId, copy: PhotoId) -> Result<()> {
    match group_of(w, of)? {
        Some(group) => {
            let mut photos = group.photos;
            let at = photos.iter().position(|p| *p == of).unwrap_or(0);
            photos.insert(at + 1, copy);
            place(w, group.id, &photos)?;
            w.execute(
                sql!("UPDATE stacks SET collapsed=0 WHERE id=?"),
                &[&group.id],
            )?;
        }
        None => {
            create(w, &[of, copy], false)?;
        }
    }
    drop_empty(w)
}

/// A change to stacks, made in one transaction by [`Catalog::change_stacks`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StackChange {
    /// Group into Stack: the photos one collapsed stack in this order, the
    /// first its top. They leave any stack they were in; one of those left
    /// with a single photo is no stack any more.
    Group(Vec<PhotoId>),
    /// Several Group into Stack at once, all or none: Auto-Stack's.
    Groups(Vec<Vec<PhotoId>>),
    /// Unstack: every stack one of the photos is in comes apart.
    Unstack(Vec<PhotoId>),
    /// Remove from Stack: the photos leave their stacks, which keep the
    /// rest.
    Remove(Vec<PhotoId>),
    /// Move to Top of Stack: the photo becomes its stack's top, the others
    /// keeping their order.
    MoveToTop(PhotoId),
    /// Moves the photo one place up (-1) or down (1) in its stack.
    Move(PhotoId, isize),
    /// Puts back the stacks of the state's photos as it has them, for Undo
    /// and Redo: those photos leave the stacks they are in now. A photo
    /// removed since is left out, and a stack whose photos are no longer in
    /// one folder is left apart.
    Restore(StackState),
}
impl StackChange {
    /// The photos whose stacks it changes, before it.
    fn photos(&self) -> Vec<PhotoId> {
        match self {
            Self::Group(photos) | Self::Unstack(photos) | Self::Remove(photos) => photos.clone(),
            Self::Groups(stacks) => stacks.concat(),
            Self::MoveToTop(photo) | Self::Move(photo, _) => vec![*photo],
            Self::Restore(state) => state.photos.clone(),
        }
    }
}

/// What a [`StackChange`] did: the stacks of the photos it touched before
/// and after, read in its transaction, for Undo and Redo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StacksChanged {
    pub before: StackState,
    pub after: StackState,
}

/// The stacks of `photos`: every stack one of them is in, with all of its
/// photos.
fn state(db: &impl Reads, photos: &[PhotoId]) -> Result<StackState> {
    let mut state = StackState::default();
    let mut seen: HashSet<PhotoId> = HashSet::new();
    for photo in photos {
        if !seen.insert(*photo) {
            continue;
        }
        state.photos.push(*photo);
        if let Some(group) = group_of(db, *photo)? {
            for p in &group.photos {
                if seen.insert(*p) {
                    state.photos.push(*p);
                }
            }
            state.stacks.push(SavedStack {
                photos: group.photos,
                collapsed: group.collapsed,
            });
        }
    }
    Ok(state)
}

/// Makes `change`, inside a write transaction.
fn apply(w: &mut Write<'_>, change: &StackChange) -> Result<()> {
    match change {
        StackChange::Group(photos) => {
            check_stackable(w, photos)?;
            create(w, photos, true)?;
        }
        StackChange::Groups(stacks) => {
            for photos in stacks {
                check_stackable(w, photos)?;
                create(w, photos, true)?;
            }
        }
        StackChange::Unstack(photos) => {
            for photo in photos {
                if let Some(group) = group_of(w, *photo)? {
                    leave(w, &group.photos)?;
                }
            }
        }
        StackChange::Remove(photos) => leave(w, photos)?,
        StackChange::MoveToTop(photo) | StackChange::Move(photo, _) => {
            let Some(group) = group_of(w, *photo)? else {
                anyhow::bail!("The photo is in no stack");
            };
            let mut photos = group.photos;
            let at = photos.iter().position(|p| p == photo).unwrap_or(0);
            match change {
                StackChange::Move(_, delta) => {
                    let to = at.saturating_add_signed(*delta).min(photos.len() - 1);
                    photos.swap(at, to);
                }
                _ => {
                    let top = photos.remove(at);
                    photos.insert(0, top);
                }
            }
            place(w, group.id, &photos)?;
        }
        StackChange::Restore(state) => {
            // The stacks they are in now, with photos `state` may not name.
            let mut now = Vec::new();
            for photo in &state.photos {
                if let Some(group) = group_of(w, *photo)? {
                    now.extend(group.photos);
                }
            }
            leave(w, &now)?;
            leave(w, &state.photos)?;
            for stack in &state.stacks {
                let mut photos = Vec::new();
                for photo in &stack.photos {
                    let present: Option<PhotoId> =
                        w.read_optional(sql!("SELECT id FROM photos WHERE id=?"), &[photo])?;
                    photos.extend(present);
                }
                if photos.len() >= 2 && check_stackable(w, &photos).is_ok() {
                    create(w, &photos, stack.collapsed)?;
                }
            }
        }
    }
    drop_empty(w)
}

impl Catalog {
    /// Every stacked photo's place in its stack.
    pub fn stack_places(&self) -> Result<HashMap<PhotoId, StackPlace>> {
        let mut places = HashMap::new();
        for group in all_groups(&self.db)? {
            for (index, photo) in group.photos.iter().enumerate() {
                places.insert(
                    *photo,
                    StackPlace {
                        id: group.id,
                        index,
                        count: group.photos.len(),
                        collapsed: group.collapsed,
                    },
                );
            }
        }
        Ok(places)
    }
    #[cfg(any(test, feature = "test-support"))]
    /// The photos of the stack `photo` is in, top first; empty when it is
    /// in none.
    pub fn stack_of(&self, photo: PhotoId) -> Result<Vec<PhotoId>> {
        Ok(group_of(&self.db, photo)?.map_or_else(Vec::new, |g| g.photos))
    }
    #[cfg(any(test, feature = "test-support"))]
    /// The stacks of `photos`: every stack one of them is in, with all of
    /// its photos.
    pub fn stack_state(&self, photos: &[PhotoId]) -> Result<StackState> {
        state(&self.db, photos)
    }
    /// Makes `change` in one transaction, which also reads the stacks it
    /// touches before and after it: all of it is saved, with what Undo and
    /// Redo need, or none.
    pub fn change_stacks(&mut self, change: &StackChange) -> Result<StacksChanged> {
        self.db.write(|w| {
            let before = state(w, &change.photos())?;
            apply(w, change)?;
            let after = state(w, &before.photos)?;
            Ok(StacksChanged { before, after })
        })
    }
    /// Stack Camera Bursts for photos added: makes `stacks` (each top first)
    /// and marks import `batches` done, in one transaction. Only what is still
    /// to be done is: a batch already done (or gone) is left out, and so is
    /// any photo that isn't a master of a batch still to be done, or is in a
    /// stack by now (or an earlier one of `stacks`), or isn't in the folder
    /// of its stack's first photo; a stack left with fewer than two photos
    /// isn't made. `None` when no batch
    /// was still to be done, and nothing changed.
    pub fn stack_imported_bursts(
        &mut self,
        batches: &[i64],
        stacks: &[Vec<PhotoId>],
    ) -> Result<Option<StacksChanged>> {
        row! {
            struct Eligible {
                folder: FolderId,
                batch: i64,
            }
        }
        self.db.write(|w| {
            let mut pending = Vec::new();
            for batch in batches {
                let found: Option<i64> = w.read_optional(
                    sql!("SELECT batch FROM import_burst_stacks WHERE batch=?"),
                    &[batch],
                )?;
                pending.extend(found);
            }
            if pending.is_empty() {
                return Ok(None);
            }
            let mut groups = Vec::new();
            let mut taken: HashSet<PhotoId> = HashSet::new();
            for stack in stacks {
                let mut group: Vec<PhotoId> = Vec::new();
                let mut folder = None;
                for photo in stack {
                    let found: Option<Eligible> = w.read_optional(
                        sql!(
                            "SELECT p.folder, i.batch FROM photos p
                             JOIN photo_imports i ON i.photo = p.id
                             WHERE p.id=? AND p.master_id IS NULL"
                        ),
                        &[photo],
                    )?;
                    let Some(Eligible { folder: at, batch }) = found else {
                        continue;
                    };
                    // In a stack as stacks are read: a row left alone in its
                    // folder is none (see `group_of`).
                    if group_of(w, *photo)?.is_some() {
                        continue;
                    }
                    if !pending.contains(&batch) || taken.contains(photo) || group.contains(photo) {
                        continue;
                    }
                    if *folder.get_or_insert(at) == at {
                        group.push(*photo);
                    }
                }
                if group.len() >= 2 {
                    taken.extend(group.iter().copied());
                    groups.push(group);
                }
            }
            let change = StackChange::Groups(groups);
            let before = state(w, &change.photos())?;
            apply(w, &change)?;
            let after = state(w, &before.photos)?;
            for batch in &pending {
                w.execute(
                    sql!("DELETE FROM import_burst_stacks WHERE batch=?"),
                    &[batch],
                )?;
            }
            Ok(Some(StacksChanged { before, after }))
        })
    }
    /// Collapses or expands stacks `ids`: a view of the catalog, saved with
    /// it, and no change Undo takes back.
    pub fn set_stacks_collapsed(&mut self, ids: &[StackId], collapsed: bool) -> Result<()> {
        self.db.write(|w| {
            for id in ids {
                w.execute(
                    sql!("UPDATE stacks SET collapsed=? WHERE id=?"),
                    &[&collapsed, id],
                )?;
            }
            Ok(())
        })
    }
}

/// Auto-Stack by Capture Time: runs of photos of one folder whose capture
/// times, in order, are each less than `seconds` after the one before;
/// each run of two or more, earliest first, becomes a stack. `photos` are
/// (id, folder, capture time as the catalog keeps it); a photo without a
/// time that can be read is left alone.
pub fn auto_stacks(photos: &[(PhotoId, FolderId, &str)], seconds: f64) -> Vec<Vec<PhotoId>> {
    let mut timed: Vec<(FolderId, f64, PhotoId)> = photos
        .iter()
        .filter_map(|(id, folder, captured)| Some((*folder, capture_seconds(captured)?, *id)))
        .collect();
    timed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut runs: Vec<Vec<PhotoId>> = Vec::new();
    let mut last: Option<(FolderId, f64)> = None;
    for (folder, at, id) in timed {
        match (runs.last_mut(), last) {
            (Some(run), Some((f, t))) if f == folder && at - t < seconds => run.push(id),
            _ => runs.push(vec![id]),
        }
        last = Some((folder, at));
    }
    runs.retain(|r| r.len() >= 2);
    runs
}

/// A photo for Auto-Stack's Group Camera Bursts: [`auto_stacks`]' tuple
/// and the sequence its camera says it is in, `None` when the camera says
/// nothing this reads.
#[derive(Clone, Copy, Debug)]
pub struct BurstCandidate<'a> {
    pub id: PhotoId,
    pub folder: FolderId,
    /// The capture time as the catalog keeps it.
    pub captured: &'a str,
    /// The file name, which orders photos taken in the same second (a
    /// camera without subseconds shoots several frames a second) as the
    /// camera counted its files.
    pub filename: &'a str,
    /// The file's format ("RAF", "JPG"…): a RAW and the JPEG shot with it
    /// are numbered alike and make two runs.
    pub format: &'a str,
    pub sequence: Option<&'a Sequence>,
}

/// The longest a frame of a burst may come after the one before it: room
/// for a long exposure or a full buffer in continuous drive.
const MOST_SECONDS_BETWEEN_FRAMES: f64 = 60.;

/// Auto-Stack with Group Camera Bursts: a folder's photos of one burst id,
/// or one run of a camera's frame numbers, become a stack, earliest first,
/// however long they took. A run goes on while the frames' numbers rise (a
/// burst's culled frames may be gone), each frame at most
/// [`MOST_SECONDS_BETWEEN_FRAMES`] after the one before; a number that
/// doesn't rise starts the next press's run. Frame numbers are no burst's
/// id: a burst whose first frames were deleted, shot within that time of
/// the one before, joins it. Photos a camera says it took in continuous drive
/// without numbering them are stacked by capture time as [`auto_stacks`]
/// does, each camera's and format's apart: two presses less than `seconds`
/// apart make one stack. A camera's photo of another kind (a single shot,
/// a numbered frame) ends its runs of the others. A photo the camera says is
/// a single shot is left alone. Photos it says nothing of are stacked by
/// capture time as [`auto_stacks`] does, except that a photo that the
/// camera says something of comes between them. Photos without a time that
/// can be read are left alone.
pub fn auto_stacks_with_bursts(photos: &[BurstCandidate<'_>], seconds: f64) -> Vec<Vec<PhotoId>> {
    let mut timed: Vec<(f64, &BurstCandidate<'_>)> = photos
        .iter()
        .filter_map(|c| Some((capture_seconds(c.captured)?, c)))
        .collect();
    timed.sort_by(|(at, a), (bt, b)| {
        (a.folder.cmp(&b.folder))
            .then(at.total_cmp(bt))
            .then(a.filename.cmp(b.filename))
            .then(a.id.cmp(&b.id))
    });
    let mut stacks: Vec<Vec<PhotoId>> = Vec::new();
    for folder in timed.chunk_by(|(_, a), (_, b)| a.folder == b.folder) {
        let mut bursts: HashMap<(&str, &str), Vec<PhotoId>> = HashMap::new();
        // Each camera's and format's run of numbered frames, with its last
        // frame's number and time.
        let mut runs: HashMap<(&str, &str), (Vec<PhotoId>, u32, f64)> = HashMap::new();
        // Each camera's and format's run of frames not numbered, with its
        // last frame's time.
        let mut continuous: HashMap<(&str, &str), (Vec<PhotoId>, f64)> = HashMap::new();
        let mut by_time: Vec<Vec<PhotoId>> = Vec::new();
        let mut ended: Vec<Vec<PhotoId>> = Vec::new();
        // The time of the last photo, when it was one the camera said nothing of.
        let mut last_unknown: Option<f64> = None;
        for (at, c) in folder {
            let Some(sequence) = c.sequence else {
                match (by_time.last_mut(), last_unknown) {
                    (Some(run), Some(t)) if at - t < seconds => run.push(c.id),
                    _ => by_time.push(vec![c.id]),
                }
                last_unknown = Some(*at);
                continue;
            };
            last_unknown = None;
            let camera = sequence.camera.as_str();
            // This camera's runs of the other kind end here.
            if !matches!(sequence.mark, Mark::Frame(_)) {
                for ((cam, _), (photos, ..)) in &mut runs {
                    if *cam == camera && !photos.is_empty() {
                        ended.push(std::mem::take(photos));
                    }
                }
            }
            if sequence.mark != Mark::Continuous {
                for ((cam, _), (photos, _)) in &mut continuous {
                    if *cam == camera && !photos.is_empty() {
                        ended.push(std::mem::take(photos));
                    }
                }
            }
            match &sequence.mark {
                Mark::Burst(id) => bursts.entry((camera, id)).or_default().push(c.id),
                Mark::Frame(n) => {
                    let run = runs
                        .entry((camera, c.format))
                        .or_insert((Vec::new(), 0, *at));
                    let (photos, last, t) = run;
                    if !(*n > *last && at - *t <= MOST_SECONDS_BETWEEN_FRAMES) {
                        ended.push(std::mem::take(photos));
                    }
                    photos.push(c.id);
                    (*last, *t) = (*n, *at);
                }
                Mark::Continuous => {
                    let (photos, t) = continuous
                        .entry((camera, c.format))
                        .or_insert((Vec::new(), *at));
                    if photos.is_empty() || at - *t >= seconds {
                        ended.push(std::mem::take(photos));
                    }
                    photos.push(c.id);
                    *t = *at;
                }
                Mark::Single => {}
            }
        }
        // In the order their first photos were taken.
        let mut found: Vec<Vec<PhotoId>> = bursts.into_values().collect();
        found.extend(runs.into_values().map(|(photos, ..)| photos));
        found.extend(continuous.into_values().map(|(photos, _)| photos));
        found.extend(ended.into_iter().chain(by_time));
        found.retain(|stack| !stack.is_empty());
        let first: HashMap<PhotoId, usize> = folder
            .iter()
            .enumerate()
            .map(|(i, (_, c))| (c.id, i))
            .collect();
        found.sort_by_key(|stack| stack.first().map(|id| first[id]));
        stacks.extend(found);
    }
    stacks.retain(|s| s.len() >= 2);
    stacks
}

/// Seconds since an arbitrary epoch of a capture time as the catalog keeps
/// it ("2024-03-02T10:00:01.120", local time, no offset), for comparing two.
fn capture_seconds(captured: &str) -> Option<f64> {
    let digits = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = captured.get(range)?;
        part.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    let (year, month, day) = (digits(0..4)?, digits(5..7)?, digits(8..10)?);
    let (hour, minute, second) = (digits(11..13)?, digits(14..16)?, digits(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let fraction = captured
        .get(19..)
        .and_then(|rest| rest.strip_prefix('.'))
        .map(|f| {
            f.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
        })
        .filter(|f| !f.is_empty())
        .and_then(|f| format!("0.{f}").parse::<f64>().ok())
        .unwrap_or(0.);
    let days = crate::time::days(year, month, day);
    Some((days * 86_400 + hour * 3600 + minute * 60 + second) as f64 + fraction)
}

#[cfg(test)]
pub(super) fn capture_seconds_for_tests(captured: &str) -> Option<f64> {
    capture_seconds(captured)
}
