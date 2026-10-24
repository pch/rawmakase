//! The edit of the photo open in Develop, apart from the window that shows it:
//! its History and whether it still needs saving.
pub(crate) mod history;
pub(crate) mod save_state;

use crate::model::recipe::Recipe;
use history::{History, Step};
use save_state::SaveState;

/// Whether a drag, a wheel scroll or a dial turn is still changing the settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Gesture {
    /// Still under way: its change becomes one step once it ends.
    Held,
    /// None, or it ended this frame.
    Released,
}

/// The settings as a frame of the editor found them, from [`EditSession::begin`].
pub(crate) struct Frame {
    before: Recipe,
}
impl Frame {
    pub(crate) fn before(&self) -> &Recipe {
        &self.before
    }
}

/// What a frame did to the settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FrameOutcome {
    Unchanged,
    /// Changed, by an edit, Undo or a History click, and marked for saving.
    Edited,
}

/// The edit of the photo open in Develop: its settings, the History of how they
/// came to be, and whether they still need saving.
#[derive(Default)]
pub(crate) struct EditSession {
    recipe: Recipe,
    history: History,
    save: SaveState,
    /// An editor frame is open (see [`EditSession::begin`]): the settings may be
    /// changed in place, and the frame records the change when it finishes.
    frame_open: bool,
    /// See [`Self::mask_lists`].
    mask_lists: u64,
}
impl EditSession {
    /// The settings.
    pub(crate) fn recipe(&self) -> &Recipe {
        &self.recipe
    }
    /// The settings to change in place, while an editor frame is open: the frame
    /// records the change as a History step, marks it for saving and turns on a
    /// switched-off panel it changed. Outside a frame, [`Self::change`] does that.
    pub(crate) fn recipe_mut(&mut self) -> &mut Recipe {
        debug_assert!(
            self.frame_open,
            "the settings change in place only inside an editor frame; use change()"
        );
        &mut self.recipe
    }
    /// The settings, for a test to set up the photo it then edits; records
    /// nothing.
    #[cfg(test)]
    pub(crate) fn setup_mut(&mut self) -> &mut Recipe {
        &mut self.recipe
    }
    /// Changes the settings outside an editor frame (a command, a preset, a result
    /// computed off the UI thread) as one History step, `step` or one named for what
    /// changed, to be saved. Returns what `edit` returns.
    pub(crate) fn change<T>(
        &mut self,
        step: Option<Step>,
        edit: impl FnOnce(&mut Recipe) -> T,
    ) -> T {
        let before = self.recipe.clone();
        let out = edit(&mut self.recipe);
        // A change that changed nothing is no step, and names none.
        if self.recipe != before {
            self.commit(before, step);
        }
        out
    }
    /// The settings, to store what was analysed from the photo (Upright's
    /// corrections, Guided's solution) rather than an edit: no History step.
    pub(crate) fn analysed_mut(&mut self) -> &mut Recipe {
        &mut self.recipe
    }
    /// Starts over with `recipe`, recording nothing: a photo opened, a snapshot or
    /// History state shown, settings read back. History and save state are the
    /// caller's to set alongside, as the case needs.
    pub(crate) fn replace(&mut self, recipe: Recipe) -> Recipe {
        self.set_whole(|s| std::mem::replace(&mut s.recipe, recipe))
    }
    /// Counts the times setting the settings as a whole (a photo, preset, snapshot
    /// or History state shown, an Undo) brought other masks. A mask's index may name
    /// another mask once this moves, even one just like it; edits in place, a
    /// preset's Amount or settings that keep their masks leave it.
    pub(crate) fn mask_lists(&self) -> u64 {
        self.mask_lists
    }
    /// Sets the settings as a whole with `set`, counting a change of masks.
    fn set_whole<T>(&mut self, set: impl FnOnce(&mut Self) -> T) -> T {
        let masks = self.recipe.masks.clone();
        let out = set(self);
        if self.recipe.masks != masks {
            self.mask_lists += 1;
        }
        out
    }
    /// Ends a gesture still held (a drag, a wheel scroll), as one History step
    /// to be saved.
    pub(crate) fn finish_gesture(&mut self) {
        if self.history.in_gesture() {
            self.history.finish_gesture(&self.recipe);
            self.save.mark_changed();
        }
    }
    /// Ends a gesture still held whose state was just saved (the photo being
    /// left mid-drag): one History step, with nothing new to save.
    pub(crate) fn finish_saved_gesture(&mut self) {
        self.history.finish_gesture(&self.recipe);
    }
    /// Ends a gesture still held as the settings were when `frame` began, as one
    /// History step to be saved: what changes later in the frame is a step of
    /// its own.
    pub(crate) fn finish_gesture_before(&mut self, frame: &Frame) {
        if self.history.in_gesture() {
            self.history.finish_gesture(frame.before());
            self.save.mark_changed();
        }
    }
    /// Names the next History step `step`, for an edit whose own name would say
    /// less (a preset, a menu choice).
    pub(crate) fn name_next_step(&mut self, step: Step) {
        self.history.label(step);
    }
    /// The changes History recorded since the last call, for the shared Undo.
    pub(crate) fn take_recorded(&mut self) -> Vec<history::Recorded> {
        self.history.take_recorded()
    }
    /// The History read back from `saved`, for the current settings.
    pub(crate) fn restore_history(&mut self, saved: crate::model::saved_history::SavedHistory) {
        self.history = History::restored(saved, &self.recipe);
    }
    /// Returns the settings to `target`, the state History has at `at`, as the step
    /// `step` (Undo of a catalog command, say).
    pub(crate) fn restore(&mut self, at: history::Mark, target: &Recipe, step: Step) {
        let changed = self.recipe != *target;
        self.set_whole(|s| s.history.restore(at, target, &mut s.recipe, step));
        if changed {
            self.save.mark_changed();
        }
    }
    /// Sets the settings to `target` as History step `step`, to be saved.
    pub(crate) fn set(&mut self, target: &Recipe, step: Step) {
        let changed = self.recipe != *target;
        self.set_whole(|s| s.history.set(target, &mut s.recipe, step));
        if changed {
            self.save.mark_changed();
        }
    }
    /// The settings and every state History keeps, to bring all up to date at once
    /// (Upright's analysis arriving, say).
    pub(crate) fn states_mut(&mut self) -> impl Iterator<Item = &mut Recipe> {
        std::iter::once(&mut self.recipe).chain(self.history.states_mut())
    }
    /// The History of how the settings came to be.
    pub(crate) fn history(&self) -> &History {
        &self.history
    }
    /// The History, for a test to step through it directly.
    #[cfg(test)]
    pub(crate) fn history_mut(&mut self) -> &mut History {
        &mut self.history
    }
    /// Undoes the latest History step on the settings.
    #[cfg(test)]
    pub(crate) fn undo(&mut self) -> bool {
        self.set_whole(|s| s.history.undo(&mut s.recipe))
    }
    /// Shows History state `applied` (steps applied), as a History click does,
    /// to be saved.
    pub(crate) fn jump(&mut self, applied: usize) -> bool {
        let moved = self.set_whole(|s| s.history.jump(applied, &mut s.recipe));
        if moved {
            self.save.mark_changed();
        }
        moved
    }
    /// Whether the settings still need saving, and why not if they can't be.
    pub(crate) fn save_state(&self) -> &SaveState {
        &self.save
    }
    /// The save state, for autosave and for settings saved with the edit that are
    /// not History steps (export options).
    pub(crate) fn save_state_mut(&mut self) -> &mut SaveState {
        &mut self.save
    }
    /// Records the change from `before` to the current settings as one History
    /// step, named `step` or for what changed, to be saved. Returns whether the
    /// settings changed.
    pub(crate) fn commit(&mut self, before: Recipe, step: Option<Step>) -> bool {
        crate::model::edit::turn_on_edited_panel(&before, &mut self.recipe);
        if let Some(step) = step {
            self.history.label(step);
        }
        let changed = self.history.record(before, &self.recipe);
        if changed {
            self.save.mark_changed();
        }
        changed
    }
    /// Starts a frame of the editor, which may edit the settings.
    pub(crate) fn begin(&mut self) -> Frame {
        self.frame_open = true;
        self.history.begin_frame();
        Frame {
            before: self.recipe.clone(),
        }
    }
    /// Ends `frame`: an edit made while a `gesture` is held is recorded once it is
    /// released, as one step; Undo and History clicks are not recorded again. An
    /// edit that changed only a switched-off panel turns it on.
    pub(crate) fn finish(&mut self, frame: Frame, gesture: Gesture) -> FrameOutcome {
        self.frame_open = false;
        if !self.history.is_replaying() {
            crate::model::edit::turn_on_edited_panel(&frame.before, &mut self.recipe);
        }
        let held = gesture == Gesture::Held;
        if self.history.observe(frame.before, &self.recipe, held) {
            self.save.mark_changed();
            FrameOutcome::Edited
        } else {
            FrameOutcome::Unchanged
        }
    }
}

/// A number that orders the changes made in the same frame, edits and catalog
/// commands alike.
pub(crate) fn sequence() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_the_settings_as_a_whole_counts_only_other_masks() {
        let mut session = EditSession::default();
        let mut other = session.recipe().clone();
        other.exposure = 1.;
        session.replace(other);
        session.change(None, |r| r.masks.push(Default::default()));
        assert_eq!(session.mask_lists(), 0);
        // Another mask, though just like the one there.
        let mut more = session.recipe().clone();
        more.masks.push(Default::default());
        session.replace(more);
        assert_eq!(session.mask_lists(), 1);
        assert!(session.undo());
        assert_eq!(session.mask_lists(), 2);
        // Back to the step that added the first mask.
        assert!(session.jump(1));
        assert_eq!(session.mask_lists(), 3);
    }

    #[test]
    fn a_change_outside_a_frame_is_one_named_step_to_save() {
        let mut session = EditSession::default();
        let returned = session.change(Some(Step::new("Auto Settings", "")), |r| {
            r.exposure = 1.;
            r.exposure
        });
        assert_eq!(returned, 1.);
        assert_eq!(session.recipe().exposure, 1.);
        let (steps, applied) = session.history().steps();
        assert_eq!((steps[0].name.as_str(), applied), ("Auto Settings", 1));
        assert!(session.save_state().needs_save());
        assert!(session.undo());
        assert_eq!(session.recipe().exposure, 0.);
    }

    #[test]
    fn a_change_that_changes_nothing_records_and_saves_nothing() {
        let mut session = EditSession::default();
        session.change(Some(Step::new("Straighten", "Auto")), |r| r.exposure = 0.);
        assert_eq!(session.history().steps().1, 0);
        assert!(!session.save_state().needs_save());
    }

    #[test]
    fn what_was_analysed_is_stored_with_no_step() {
        let mut session = EditSession::default();
        session.analysed_mut().upright.corrections = vec![[0.; 9]];
        assert_eq!(session.history().steps().1, 0);
        assert!(!session.save_state().needs_save());
    }

    #[test]
    fn a_frame_records_what_changed_in_it() {
        let mut session = EditSession::default();
        let frame = session.begin();
        session.recipe_mut().exposure = 1.;
        assert_eq!(
            session.finish(frame, Gesture::Released),
            FrameOutcome::Edited
        );
        assert_eq!(session.history().steps().1, 1);
        assert!(session.save_state().needs_save());
    }

    #[test]
    fn stepping_and_restoring_history_marks_the_edit_for_saving() {
        let mut session = EditSession::default();
        session.change(None, |r| r.exposure = 1.);
        session.save_state_mut().saved();
        assert!(session.jump(0));
        assert!(session.save_state().needs_save());
        session.save_state_mut().saved();
        let target = Recipe {
            exposure: 2.,
            ..session.recipe().clone()
        };
        session.set(&target, Step::new("Undo", ""));
        assert!(session.save_state().needs_save());
        // Setting what is already there needs no save.
        session.save_state_mut().saved();
        session.set(&target, Step::new("Undo", ""));
        assert!(!session.save_state().needs_save());
    }

    #[test]
    fn a_gesture_finished_outside_a_frame_is_a_step_to_save() {
        let mut session = EditSession::default();
        let frame = session.begin();
        session.recipe_mut().exposure = 1.;
        session.finish(frame, Gesture::Held);
        session.save_state_mut().saved();
        session.finish_gesture();
        assert_eq!(session.history().steps().1, 1);
        assert!(session.save_state().needs_save());
    }

    #[test]
    fn a_gesture_whose_state_was_saved_stays_saved_when_it_ends() {
        let mut session = EditSession::default();
        let frame = session.begin();
        session.recipe_mut().exposure = 1.;
        session.finish(frame, Gesture::Held);
        session.save_state_mut().saved();
        session.finish_saved_gesture();
        assert_eq!(session.history().steps().1, 1);
        assert!(!session.save_state().needs_save());
    }

    #[test]
    fn a_commit_is_one_named_step_to_save() {
        let mut session = EditSession::default();
        let before = session.recipe.clone();
        session.recipe.exposure = 1.;
        assert!(session.commit(before, Some(Step::new("Auto Settings", ""))));
        let (steps, applied) = session.history.steps();
        assert_eq!((steps[0].name.as_str(), applied), ("Auto Settings", 1));
        assert!(session.save.needs_save());
    }

    #[test]
    fn a_commit_that_changes_nothing_records_and_saves_nothing() {
        let mut session = EditSession::default();
        let before = session.recipe.clone();
        assert!(!session.commit(before, Some(Step::new("Straighten", "Auto"))));
        assert_eq!(session.history.steps().1, 0);
        assert!(!session.save.needs_save());
        // Its name is not left for the next edit.
        let before = session.recipe.clone();
        session.recipe.exposure = 1.;
        session.commit(before, None);
        assert_ne!(session.history.steps().0[0].name, "Straighten");
    }

    #[test]
    fn a_gesture_is_saved_as_it_goes_and_recorded_once_it_ends() {
        let mut session = EditSession::default();
        let frame = session.begin();
        session.recipe.exposure = 0.5;
        assert_eq!(session.finish(frame, Gesture::Held), FrameOutcome::Edited);
        assert!(session.save.needs_save());
        assert_eq!(session.history.steps().1, 0);
        let frame = session.begin();
        session.recipe.exposure = 1.;
        session.finish(frame, Gesture::Released);
        assert_eq!(session.history.steps().1, 1);
        assert!(session.history.undo(&mut session.recipe));
        assert_eq!(session.recipe.exposure, 0.);
    }
}
