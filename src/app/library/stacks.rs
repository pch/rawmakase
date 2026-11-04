//! Lightroom's folder stacks in the Library: Group into Stack (Cmd+G),
//! Unstack (Cmd+Shift+G), Remove from Stack, Move to Top of Stack (Shift+S),
//! moving a photo up or down, collapsing and expanding (S, or the badge on a
//! stack's thumbnail), and Auto-Stack by Capture Time. All Photographs and
//! folders show stacks; a collection lists their photos one by one (see
//! `Filters::shows_stacks`).
//!
//! A command acts on the photos as the grid shows them: a collapsed stack
//! shown by its top is the whole stack to Group into Stack and Unstack, and
//! that photo alone to everything else, metadata included, as in Lightroom.
//! Each change of stacks is one undoable command; collapsing and expanding
//! are a view, saved with the catalog, which Undo leaves alone.
use super::{Library, Place};
use crate::app::ui::design::{self, Tone, button, inset, space, text};
use crate::app::ui::widgets::{self, plural};
use crate::catalog::{
    BurstCandidate, Photo, PhotoId, SavedSequence, StackChange, StackId, StackState, StacksChanged,
};
use crate::catalog_session::background::Reader;
use crate::exif::Sequence;
use anyhow::Result;
use eframe::egui;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// A stack command from a thumbnail's menu or a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum StackAction {
    Group,
    Unstack,
    Remove,
    MoveToTop,
    MoveUp,
    MoveDown,
    /// Collapses or expands the stacks of the photos.
    Toggle,
    CollapseAll,
    ExpandAll,
    /// Opens Auto-Stack by Capture Time.
    AutoStack,
}

/// A change to stacks, for the shared undo log: the stacks of the photos it
/// touched before and after.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StackCommand {
    /// Orders it among other changes made in the same frame.
    pub sequence: u64,
    pub before: StackState,
    pub after: StackState,
    pub place_before: Place,
    pub place_after: Place,
    /// What changed, as the status line said it.
    pub summary: String,
}
impl StackCommand {
    /// Takes photo `id` out of the command; false when nothing of it is left.
    pub(in crate::app) fn forget_photo(&mut self, id: PhotoId) -> bool {
        self.before.forget_photo(id);
        self.after.forget_photo(id);
        self.place_before.forget_photo(id);
        self.place_after.forget_photo(id);
        !(self.before.stacks.is_empty() && self.after.stacks.is_empty())
    }
}

/// Auto-Stack by Capture Time's window, while it is open.
pub(super) struct AutoStack {
    /// Time Between Stacks, in seconds.
    pub(super) seconds: f64,
    /// Group Camera Bursts: stacks the bursts the cameras numbered as they
    /// numbered them, and the other photos by time.
    pub(super) bursts: bool,
    /// What the cameras say of the photos, once Group Camera Bursts is
    /// first ticked: from the catalog for files unchanged since they were
    /// read, else read from them and saved.
    sequences: Sequences,
}
impl AutoStack {
    /// Whether what it shows can be made: not while reading the cameras'
    /// data, nor once the read stopped before every photo.
    pub(super) fn can_stack(&self) -> bool {
        !self.bursts || (self.sequences.reader.is_none() && self.sequences.pending.is_empty())
    }
}
impl Default for AutoStack {
    fn default() -> Self {
        Self {
            // Lightroom's default.
            seconds: 60.,
            bursts: false,
            sequences: Sequences::default(),
        }
    }
}

/// What the cameras say of the sequences the source's photos are in.
#[derive(Default)]
struct Sequences {
    /// What each candidate's camera says, as checked so far; `None` before
    /// the check starts.
    read: Option<std::collections::HashMap<PhotoId, Option<Sequence>>>,
    /// Checking the candidates' files, with how many there are; `None`
    /// before and after.
    reader: Option<(Reader<Checked>, usize)>,
    /// The photos the reader has still to check; after it, those it never
    /// checked, as it stopped (a panic) before them.
    pending: HashSet<PhotoId>,
    /// The session's structural generation when the check started: under
    /// another, photo ids may name other photos, and it starts again.
    generation: u64,
}

/// What checking a candidate's file found.
enum Checked {
    /// The file is the one the catalog read: what it said then.
    Kept(Option<Sequence>),
    /// Read now, to be saved.
    Read(SavedSequence),
    /// Offline, or changed while being read: it stacks by time, and
    /// nothing is saved, so it is read once it can be.
    Unreadable,
}

/// Checks the file at `path` against what the catalog read of it, `stored`,
/// and reads it with `read` when it isn't that file.
fn check(
    path: &Path,
    stored: Option<&SavedSequence>,
    read: fn(&Path) -> Option<Sequence>,
) -> Checked {
    let Ok(stamp) = crate::storage::Stamp::read(path) else {
        return Checked::Unreadable;
    };
    match stored {
        Some(saved) if saved.stamp == stamp => Checked::Kept(saved.sequence.clone()),
        _ => crate::catalog_session::file_sequence(path, read)
            .map_or(Checked::Unreadable, Checked::Read),
    }
}
impl Sequences {
    /// What photo `p`'s camera says.
    fn of(&self, p: &Photo) -> Option<&Sequence> {
        self.read.as_ref()?.get(&p.id)?.as_ref()
    }
}

/// The longest Time Between Stacks, as Lightroom's: an hour.
const MOST_SECONDS: f64 = 3600.;

impl Library {
    /// Whether the source shown shows stacks: All Photographs and folders.
    /// For the macOS menu bar's Photo › Stacking.
    #[cfg(any(target_os = "macos", test))]
    pub(in crate::app) fn shows_stacks(&self) -> bool {
        self.filters.shows_stacks()
    }
    /// Whether Auto-Stack by Capture Time's window is open.
    pub(in crate::app) fn auto_stack_open(&self) -> bool {
        self.auto_stack.is_some()
    }
    /// Stack changes not yet handed to the shared undo log.
    pub(in crate::app) fn take_stack_done(&mut self) -> Vec<StackCommand> {
        std::mem::take(&mut self.stack_done)
    }
    /// The photos of the stack `photo` is in as the session has them, top
    /// first; empty when it is in none.
    fn stack_members(&self, photo: &Photo) -> Vec<PhotoId> {
        let Some(place) = photo.stack else {
            return Vec::new();
        };
        let mut members: Vec<&Photo> = self
            .session
            .photos()
            .iter()
            .filter(|p| p.folder == photo.folder && p.stack.is_some_and(|s| s.id == place.id))
            .collect();
        members.sort_by_key(|p| p.stack.map(|s| s.index));
        members.iter().map(|p| p.id).collect()
    }
    /// Whether `photo` stands for its whole stack in the grid: shown, in a
    /// collapsed stack, where the source shows stacks.
    fn stands_for_stack(&self, photo: &Photo) -> bool {
        self.filters.shows_stacks() && photo.stack.is_some_and(|s| s.collapsed)
    }
    /// `ids` as Group into Stack and Unstack take them: a photo standing for
    /// a collapsed stack is all of its photos. In blocks, by photo, in the
    /// order of `ids`.
    fn stack_blocks(&self, ids: &[PhotoId]) -> Vec<Vec<PhotoId>> {
        let mut seen = std::collections::HashSet::new();
        let mut blocks = Vec::new();
        for id in ids {
            let Some(photo) = self.photo(*id) else {
                continue;
            };
            let block = if self.stands_for_stack(photo) {
                self.stack_members(photo)
            } else {
                vec![*id]
            };
            let block: Vec<PhotoId> = block.into_iter().filter(|p| seen.insert(*p)).collect();
            if !block.is_empty() {
                blocks.push(block);
            }
        }
        blocks
    }
    /// A stack key in the grid or the Loupe, for `ids` where the source
    /// shows stacks.
    pub(super) fn stack_key(&mut self, action: StackAction, ids: Vec<PhotoId>) {
        if self.filters.shows_stacks() {
            self.stack_action(action, ids);
        } else {
            self.messages
                .warn("Stacks show in folders and All Photographs, not here.");
        }
    }
    /// Carries out a stack command on `ids`, the photos it was chosen for
    /// in display order; a failure is said on the top bar.
    pub(in crate::app) fn stack_action(&mut self, action: StackAction, ids: Vec<PhotoId>) {
        if let Err(e) = self.try_stack_action(action, ids) {
            self.messages.fail(format!("Stacks not changed: {e:#}"));
        }
    }
    fn try_stack_action(&mut self, action: StackAction, ids: Vec<PhotoId>) -> Result<()> {
        let active = self.selection.active.filter(|a| ids.contains(a));
        let active = active.or_else(|| ids.first().copied());
        match action {
            StackAction::Group => {
                let mut blocks = self.stack_blocks(&ids);
                // The active photo's block first: it is the top.
                if let Some(at) = active.and_then(|a| blocks.iter().position(|b| b.contains(&a))) {
                    // The active photo itself first, even when it stands
                    // for a collapsed stack whose top the filters hide.
                    let mut block = blocks.remove(at);
                    if let Some(a) = active
                        && let Some(i) = block.iter().position(|p| *p == a)
                    {
                        block.remove(i);
                        block.insert(0, a);
                    }
                    blocks.insert(0, block);
                }
                let photos: Vec<PhotoId> = blocks.concat();
                anyhow::ensure!(photos.len() >= 2, "select two photos or more to stack");
                let summary = format!(
                    "Grouped into Stack · {}",
                    plural(photos.len(), "photo", "photos")
                );
                let top = photos[0];
                self.record_stacks(summary, StackChange::Group(photos))?;
                self.select(Some(top));
            }
            StackAction::Unstack => {
                let photos: Vec<PhotoId> = self.stack_blocks(&ids).concat();
                anyhow::ensure!(
                    photos
                        .iter()
                        .any(|p| self.photo(*p).is_some_and(|p| p.stack.is_some())),
                    "no photo chosen is in a stack"
                );
                self.record_stacks("Unstacked".into(), StackChange::Unstack(photos))?;
            }
            StackAction::Remove => {
                let photos: Vec<PhotoId> = ids
                    .iter()
                    .copied()
                    .filter(|p| self.photo(*p).is_some_and(|p| p.stack.is_some()))
                    .collect();
                anyhow::ensure!(!photos.is_empty(), "no photo chosen is in a stack");
                let summary = format!(
                    "Removed from Stack · {}",
                    plural(photos.len(), "photo", "photos")
                );
                self.record_stacks(summary, StackChange::Remove(photos))?;
            }
            StackAction::MoveToTop | StackAction::MoveUp | StackAction::MoveDown => {
                let Some(photo) =
                    active.filter(|a| self.photo(*a).is_some_and(|p| p.stack.is_some()))
                else {
                    anyhow::bail!("the photo is in no stack");
                };
                let (summary, delta) = match action {
                    StackAction::MoveToTop => ("Moved to Top of Stack", 0),
                    StackAction::MoveUp => ("Moved Up in Stack", -1),
                    _ => ("Moved Down in Stack", 1),
                };
                let change = if delta == 0 {
                    StackChange::MoveToTop(photo)
                } else {
                    StackChange::Move(photo, delta)
                };
                self.record_stacks(summary.into(), change)?;
            }
            StackAction::Toggle => {
                let stacks: Vec<(StackId, bool)> = ids
                    .iter()
                    .filter_map(|id| self.photo(*id)?.stack)
                    .map(|s| (s.id, s.collapsed))
                    .collect();
                let Some(&(_, collapsed)) = stacks.first() else {
                    anyhow::bail!("no photo chosen is in a stack");
                };
                let ids: Vec<StackId> = stacks.iter().map(|(id, _)| *id).collect();
                self.set_stacks_collapsed(&ids, !collapsed)?;
            }
            StackAction::CollapseAll | StackAction::ExpandAll => {
                let mut ids: Vec<StackId> = self
                    .session
                    .photos()
                    .iter()
                    .filter_map(|p| p.stack.map(|s| s.id))
                    .collect();
                ids.sort();
                ids.dedup();
                self.set_stacks_collapsed(&ids, action == StackAction::CollapseAll)?;
            }
            StackAction::AutoStack => self.auto_stack = Some(AutoStack::default()),
        }
        Ok(())
    }
    /// Makes `change` and hands it to the undo log once it is saved, with
    /// the stacks it touched before and after, read in its transaction.
    fn record_stacks(&mut self, summary: String, change: StackChange) -> Result<()> {
        let place_before = self.place();
        let committed = self.session.change_stacks(&change)?;
        if self.stacks_changed(committed.listed) {
            self.messages.note(summary.clone());
        }
        let StacksChanged { before, after } = committed.value;
        self.stack_done.push(StackCommand {
            sequence: crate::edit_session::sequence(),
            before,
            after,
            place_before,
            place_after: self.place(),
            summary,
        });
        Ok(())
    }
    /// Puts stacks back as `state` has them, for Undo and Redo.
    pub(in crate::app) fn restore_stacks(&mut self, state: &StackState) -> Result<()> {
        let committed = self
            .session
            .change_stacks(&StackChange::Restore(state.clone()))?;
        self.stacks_changed(committed.listed);
        Ok(())
    }
    /// Collapses or expands stacks `ids`.
    fn set_stacks_collapsed(&mut self, ids: &[StackId], collapsed: bool) -> Result<()> {
        let committed = self.session.set_stacks_collapsed(ids, collapsed)?;
        self.stacks_changed(committed.listed);
        Ok(())
    }
    /// Expands the stack hiding photo `id`, so it can be shown.
    pub(super) fn expand_stack_of(&mut self, id: PhotoId) {
        let Some(place) = self.photo(id).and_then(|p| p.stack) else {
            return;
        };
        if place.collapsed
            && let Err(e) = self.set_stacks_collapsed(&[place.id], false)
        {
            self.messages
                .fail(format!("The stack could not be expanded: {e:#}"));
        }
    }
    /// Shows stacks as they are now. A photo of a stack that was active
    /// and is hidden now hands over to the photo its stack shows. False,
    /// with the top bar saying so, when the stacks saved could not be
    /// read again.
    pub(super) fn stacks_changed(&mut self, listed: Result<()>) -> bool {
        let read = listed.is_ok();
        if let Err(e) = listed {
            self.messages.fail(format!(
                "Stacks were saved but could not be read again: {e:#}"
            ));
        }
        let active = self.selection.active.and_then(|id| {
            let photo = self.photo(id)?;
            Some((photo.stack?.id, photo.folder))
        });
        self.filter();
        if self.selection.active.is_none()
            && let Some((stack, folder)) = active
        {
            let shown = self
                .visible
                .iter()
                .map(|i| &self.session.photos()[*i])
                .find(|p| p.folder == folder && p.stack.is_some_and(|s| s.id == stack))
                .map(|p| p.id);
            if let Some(id) = shown {
                self.select(Some(id));
            }
        }
        read
    }
    /// Auto-Stack by Capture Time's candidates: the photos of the source
    /// shown in no stack yet, filters aside.
    fn auto_stack_photos(&self) -> Vec<&Photo> {
        let available = |path: &std::path::Path| self.is_available(path);
        self.session
            .photos()
            .iter()
            .filter(|p| p.stack.is_none() && self.filters.source_holds(p, available))
            .collect()
    }
    /// The stacks Auto-Stack would make at `seconds`.
    pub(super) fn auto_stacks(&self, seconds: f64) -> Vec<Vec<PhotoId>> {
        let photos: Vec<_> = self
            .auto_stack_photos()
            .into_iter()
            .map(|p| (p.id, p.folder, p.captured.as_str()))
            .collect();
        crate::catalog::auto_stacks(&photos, seconds)
    }
    /// The stacks Auto-Stack would make with the window's choices. With
    /// Group Camera Bursts, virtual copies are left alone: a copy's camera
    /// numbers it as its master.
    pub(super) fn auto_stacks_of(&self, dialog: &AutoStack) -> Vec<Vec<PhotoId>> {
        if !dialog.bursts {
            return self.auto_stacks(dialog.seconds);
        }
        let photos: Vec<_> = self
            .auto_stack_photos()
            .into_iter()
            .filter(|p| p.master.is_none())
            .map(|p| BurstCandidate {
                id: p.id,
                folder: p.folder,
                captured: &p.captured,
                filename: &p.filename,
                format: &p.format,
                sequence: dialog.sequences.of(p),
            })
            .collect();
        crate::catalog::auto_stacks_with_bursts(&photos, dialog.seconds)
    }
    /// Checks what the cameras say of the candidates, once: what the
    /// catalog keeps of a file unchanged since, else read from the file.
    /// Saves what has been read since the last call. True while checking.
    pub(super) fn read_sequences(&mut self, dialog: &mut AutoStack) -> bool {
        self.read_sequences_by(dialog, crate::exif::sequence)
    }
    /// [`Self::read_sequences`], each file read by `read`.
    pub(super) fn read_sequences_by(
        &mut self,
        dialog: &mut AutoStack,
        read: fn(&std::path::Path) -> Option<Sequence>,
    ) -> bool {
        let generation = self.session.generation();
        let sequences = &mut dialog.sequences;
        let failed = sequences.reader.is_none() && !sequences.pending.is_empty();
        if sequences.generation != generation || (!dialog.bursts && failed) {
            // Dropping the reader stops it after the batch it is reading;
            // what it read under the old generation is never saved.
            *sequences = Sequences {
                generation,
                ..Sequences::default()
            };
        }
        if !dialog.bursts && sequences.reader.is_none() {
            return false;
        }
        if sequences.read.is_none() {
            let mut stored = match self.session.catalog().sequences() {
                Ok(stored) => stored,
                Err(e) => {
                    self.messages
                        .fail(format!("Camera bursts could not be read: {e:#}"));
                    Default::default()
                }
            };
            let todo: Vec<(PhotoId, PathBuf)> = self
                .auto_stack_photos()
                .into_iter()
                .filter(|p| p.master.is_none())
                .map(|p| (p.id, p.path.clone()))
                .collect();
            // By the file each was asked for, so what was read of a photo
            // is only ever checked against its own file.
            let stored: std::collections::HashMap<PathBuf, SavedSequence> = todo
                .iter()
                .filter_map(|(id, path)| Some((path.clone(), stored.remove(id)?)))
                .collect();
            // Not checked yet: kept apart from those that say nothing.
            sequences.pending = todo.iter().map(|(id, _)| *id).collect();
            sequences.read = Some(Default::default());
            if !todo.is_empty() {
                let count = todo.len();
                let reader = Reader::start(todo, self.wake(), move |path| {
                    check(path, stored.get(path), read)
                });
                sequences.reader = Some((reader, count));
            }
        }
        let Some((reader, _)) = &sequences.reader else {
            return false;
        };
        let (read, done) = reader.poll();
        if done {
            // What it didn't read stays pending: the read failed.
            sequences.reader = None;
        }
        let known = sequences.read.get_or_insert_default();
        let mut save = Vec::new();
        for (id, checked) in read {
            sequences.pending.remove(&id);
            let sequence = match checked {
                Checked::Kept(sequence) => sequence,
                Checked::Read(saved) => {
                    let sequence = saved.sequence.clone();
                    save.push((id, saved));
                    sequence
                }
                Checked::Unreadable => None,
            };
            known.insert(id, sequence);
        }
        if !save.is_empty()
            && let Err(e) = self.session.save_sequences(&save)
        {
            // Still used while the window is open; read again next time.
            self.messages
                .fail(format!("Camera bursts could not be saved: {e:#}"));
        }
        sequences.reader.is_some()
    }
    /// Auto-Stack by Capture Time's window: Time Between Stacks, Group
    /// Camera Bursts, what it would make, and Stack.
    pub(in crate::app) fn auto_stack_window(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.auto_stack.take() else {
            return;
        };
        let palette = crate::app::ui::theme::palette(ctx);
        self.read_sequences(&mut dialog);
        let stacks = self.auto_stacks_of(&dialog);
        let photos: usize = stacks.iter().map(Vec::len).sum();
        let mut stack = false;
        let mut cancel = false;
        let response = egui::Modal::new(egui::Id::new("auto-stack"))
            .frame(widgets::modal_frame(&palette).inner_margin(design::margin_same(inset::DIALOG)))
            .show(ctx, |ui| {
                ui.set_width(380.);
                ui.label(
                    egui::RichText::new("Auto-Stack by Capture Time")
                        .size(text::TITLE)
                        .color(palette.tone(Tone::STRONG)),
                );
                ui.add_space(space::M);
                ui.label(
                    egui::RichText::new(
                        "Stacks photos of the same folder taken less than this long after the \
                         one before. Photos already in a stack are left as they are.",
                    )
                    .size(text::LABEL)
                    .color(palette.tone(Tone::MUTED)),
                );
                ui.add_space(space::M);
                ui.horizontal(|ui| {
                    ui.label("Time Between Stacks");
                    ui.add(
                        egui::Slider::new(&mut dialog.seconds, 0.0..=MOST_SECONDS)
                            .logarithmic(true)
                            .smallest_positive(1.)
                            .custom_formatter(|s, _| duration(s))
                            .custom_parser(parse_duration),
                    );
                });
                ui.add_space(space::S);
                ui.checkbox(&mut dialog.bursts, "Group camera bursts");
                ui.label(
                    egui::RichText::new(
                        "Bursts and brackets stack as the camera numbered them, however long \
                         they took; single shots are left alone. Sony, Fujifilm, Panasonic, \
                         Olympus and OM System cameras number frames and iPhones mark bursts; \
                         Canon and Nikon say only that the drive was continuous, so those \
                         photos stack by time, each camera apart. Other photos stack by time.",
                    )
                    .size(text::LABEL)
                    .color(palette.tone(Tone::MUTED)),
                );
                ui.add_space(space::S);
                let unread = dialog.sequences.pending.len();
                let summary = match &dialog.sequences.reader {
                    Some((_, count)) if dialog.bursts => {
                        format!("Reading camera bursts… {} of {count}", count - unread)
                    }
                    None if dialog.bursts && unread > 0 => format!(
                        "Reading camera bursts stopped before {}",
                        plural(unread, "photo", "photos")
                    ),
                    _ if stacks.is_empty() => "No stacks".to_string(),
                    _ => format!(
                        "{} of {}",
                        plural(stacks.len(), "stack", "stacks"),
                        plural(photos, "photo", "photos")
                    ),
                };
                ui.label(egui::RichText::new(summary).size(text::LABEL));
                ui.add_space(space::XL);
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding = button::DIALOG;
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    let ready = !stacks.is_empty() && dialog.can_stack();
                    stack = ui
                        .add_enabled_ui(ready, |ui| widgets::primary_button(ui, "Stack"))
                        .inner
                        .clicked();
                });
            });
        if cancel || response.should_close() {
            return;
        }
        if !stack {
            self.auto_stack = Some(dialog);
            return;
        }
        if let Err(e) = self.auto_stack_now(stacks) {
            self.messages.fail(format!("Stacks not changed: {e:#}"));
        }
    }
    /// Auto-Stack's Stack: the stacks the window showed, as one command.
    pub(super) fn auto_stack_now(&mut self, stacks: Vec<Vec<PhotoId>>) -> Result<()> {
        let summary = format!("Auto-Stacked · {}", plural(stacks.len(), "stack", "stacks"));
        self.record_stacks(summary, StackChange::Groups(stacks))
    }
}

/// `seconds` as Time Between Stacks shows it: "0:01:30".
fn duration(seconds: f64) -> String {
    let s = seconds.round() as i64;
    format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
}

/// A Time Between Stacks typed as "h:mm:ss", "m:ss" or seconds.
fn parse_duration(text: &str) -> Option<f64> {
    let mut seconds = 0.;
    for part in text.trim().split(':') {
        seconds = seconds * 60. + part.trim().parse::<f64>().ok()?;
    }
    Some(seconds.clamp(0., MOST_SECONDS))
}

#[cfg(test)]
mod tests {
    #[test]
    fn time_between_stacks_reads_as_it_is_shown() {
        assert_eq!(super::duration(90.), "0:01:30");
        assert_eq!(super::duration(3600.), "1:00:00");
        assert_eq!(super::parse_duration("0:01:30"), Some(90.));
        assert_eq!(super::parse_duration("2:05"), Some(125.));
        assert_eq!(super::parse_duration("45"), Some(45.));
        assert_eq!(super::parse_duration("9:00:00"), Some(3600.));
        assert_eq!(super::parse_duration("soon"), None);
    }
}
