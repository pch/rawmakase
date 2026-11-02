//! Bounded edit history; a pointer gesture is a single transaction. Each step
//! is named, like Lightroom's History panel ("Exposure +0.50").
use crate::model::recipe::Recipe;
use crate::model::saved_history::{SavedHistory, SavedStep};
use std::collections::VecDeque;

const LIMIT: usize = 100;

/// A History panel entry: what changed, and its new value when there is one.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Step {
    pub name: String,
    pub value: String,
    /// Identifies the state this step leaves, set when it is recorded.
    state: u64,
}
impl Step {
    pub(crate) fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            state: 0,
        }
    }
}

/// A state in History: how many steps are applied, counted from the first
/// step ever recorded so it holds when old steps are dropped, and which
/// state that is, since after undoing and editing the same count can name
/// another branch.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Mark {
    applied: usize,
    state: u64,
}

/// A change to the recipe for the shared undo log: a recorded step or a
/// click in the History panel, with the History positions around it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Recorded {
    /// Orders it among Library commands made in the same frame.
    pub sequence: u64,
    pub before: Recipe,
    pub after: Recipe,
    pub at_before: Mark,
    pub at_after: Mark,
}

pub(crate) struct History {
    /// Tells this photo's History apart from earlier ones, e.g. after the
    /// photo was opened again.
    id: u64,
    /// Steps dropped from the front to stay within the limit, and the state
    /// the oldest remaining one starts from.
    dropped: usize,
    origin: u64,
    /// Changes not yet handed to the shared undo log.
    recorded: Vec<Recorded>,
    /// States before each step, oldest first, with the step that left them.
    undo: VecDeque<(Recipe, Step)>,
    /// States after each undone step, the next one last.
    redo: Vec<(Recipe, Step)>,
    gesture: Option<Recipe>,
    replaying: bool,
    /// The name for the next recorded step; otherwise it is derived.
    label: Option<Step>,
}
impl Default for History {
    fn default() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            dropped: 0,
            origin: 0,
            recorded: Vec::new(),
            undo: VecDeque::new(),
            redo: Vec::new(),
            gesture: None,
            replaying: false,
            label: None,
        }
    }
}
/// A new state identity for a step.
fn next_state() -> u64 {
    static STATES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    STATES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}
impl History {
    /// History as saved with the photo's edit: undone steps are kept, so they can
    /// still be redone after the photo opens again, as in Lightroom.
    ///
    /// A drag still in progress is saved as the step it will become, without being
    /// recorded yet, so a save that fails leaves the drag going.
    pub(crate) fn saved(&self, current: &Recipe) -> SavedHistory {
        let gesture = self.gesture.as_ref().filter(|before| *before != current);
        let mut done: Vec<_> = self
            .undo
            .iter()
            .map(|(r, step)| (r, step.clone()))
            .collect();
        if let Some(before) = gesture {
            let step = self
                .label
                .clone()
                .unwrap_or_else(|| describe(before, current));
            done.push((before, step));
        }
        let mut steps: Vec<SavedStep> = done
            .iter()
            .enumerate()
            .map(|(i, (_, step))| SavedStep {
                name: step.name.clone(),
                value: step.value.clone(),
                recipe: done.get(i + 1).map_or(current, |(r, _)| r).clone(),
            })
            .collect();
        // Finishing the drag will drop the steps undone before it.
        if gesture.is_none() {
            steps.extend(self.redo.iter().rev().map(|(r, step)| SavedStep {
                name: step.name.clone(),
                value: step.value.clone(),
                recipe: r.clone(),
            }));
        }
        SavedHistory {
            origin: done.first().map_or(current, |(r, _)| *r).clone(),
            steps,
            applied: done.len(),
        }
    }
    /// History restored from `saved` for an edit now at `current`. When the edit was
    /// changed since (Undo after moving to another photo, or a sidecar import), that change
    /// becomes the latest step, as Lightroom adds one.
    pub(crate) fn restored(saved: SavedHistory, current: &Recipe) -> Self {
        let mut history = Self::default();
        let SavedHistory {
            origin,
            steps,
            applied,
        } = saved;
        let applied = applied.min(steps.len());
        // Oldest steps beyond the limit are dropped, as when they were recorded.
        let skip = applied.saturating_sub(LIMIT);
        let mut before = if skip == 0 {
            origin
        } else {
            steps[skip - 1].recipe.clone()
        };
        let mut redo = Vec::new();
        for (i, saved) in steps.into_iter().enumerate().skip(skip) {
            let step = Step {
                name: saved.name,
                value: saved.value,
                state: next_state(),
            };
            if i < applied {
                history
                    .undo
                    .push_back((std::mem::replace(&mut before, saved.recipe), step));
            } else {
                redo.push((saved.recipe, step));
            }
        }
        history.dropped = skip;
        history.origin = next_state();
        history.redo = redo.into_iter().rev().collect();
        if before != *current {
            let mut shown = before;
            let step = describe(&shown, current);
            history.set(current, &mut shown, step);
        }
        history
    }
    pub(crate) fn id(&self) -> u64 {
        self.id
    }
    /// The state applied now.
    pub(crate) fn mark(&self) -> Mark {
        Mark {
            applied: self.dropped + self.undo.len(),
            state: self.undo.back().map_or(self.origin, |(_, s)| s.state),
        }
    }
    /// The changes made since the last call, for the shared undo log.
    pub(crate) fn take_recorded(&mut self) -> Vec<Recorded> {
        std::mem::take(&mut self.recorded)
    }
    /// A click on a History step: `go_to`, recorded as one change.
    pub(crate) fn jump(&mut self, applied: usize, current: &mut Recipe) -> bool {
        let (before, at_before) = (current.clone(), self.mark());
        if !self.go_to(applied, current) {
            return false;
        }
        self.recorded.push(Recorded {
            sequence: super::sequence(),
            before,
            after: current.clone(),
            at_before,
            at_after: self.mark(),
        });
        true
    }
    /// Undo or redo from the shared log: back to state `at` when it is still
    /// here; otherwise `target` is set as a new step named `step`.
    pub(crate) fn restore(&mut self, at: Mark, target: &Recipe, current: &mut Recipe, step: Step) {
        // Not an edit of the user's for `observe` to record.
        self.replaying = true;
        // History's own states are kept up to date (e.g. by Upright's
        // analysis), so they win over the copy in the log.
        // Moves only when that state is still in History, so a miss leaves
        // History as it was and records the fallback from the state shown.
        if self.state_at(at.applied) == Some(at.state) {
            self.go_to(at.applied - self.dropped, current);
        } else {
            self.set(target, current, step);
        }
    }
    /// The state with `applied` steps applied, among the steps History has,
    /// done and undone.
    fn state_at(&self, applied: usize) -> Option<u64> {
        let index = applied.checked_sub(self.dropped)?;
        if index == 0 {
            return Some(self.origin);
        }
        self.undo
            .iter()
            .map(|(_, s)| s)
            .chain(self.redo.iter().rev().map(|(_, s)| s))
            .nth(index - 1)
            .map(|s| s.state)
    }
    /// Sets `target` as a new step named `step`, without handing it to the
    /// shared undo log.
    pub(crate) fn set(&mut self, target: &Recipe, current: &mut Recipe, step: Step) {
        self.replaying = true;
        let before = std::mem::replace(current, target.clone());
        self.label(step);
        self.push(before, current);
    }
    /// Names the step being made, e.g. by the slider being dragged.
    pub(crate) fn label(&mut self, step: Step) {
        self.label = Some(step);
    }
    /// Every step, oldest first, and how many of them are applied.
    pub(crate) fn steps(&self) -> (Vec<&Step>, usize) {
        let steps = self
            .undo
            .iter()
            .map(|(_, s)| s)
            .chain(self.redo.iter().rev().map(|(_, s)| s))
            .collect();
        (steps, self.undo.len())
    }
    /// The edit at a History state, done or undone: `applied` steps applied, counted
    /// as `steps` lists them (0 is the oldest state kept), with `current` the state
    /// applied now. For Copy History Step Settings to Before.
    pub(crate) fn state(&self, applied: usize, current: &Recipe) -> Option<Recipe> {
        let now = self.undo.len();
        match applied.cmp(&now) {
            std::cmp::Ordering::Less => Some(self.undo[applied].0.clone()),
            std::cmp::Ordering::Equal => Some(current.clone()),
            std::cmp::Ordering::Greater => self
                .redo
                .iter()
                .rev()
                .nth(applied - now - 1)
                .map(|(r, _)| r.clone()),
        }
    }
    /// Undoes or redoes until `applied` steps are applied, as clicking a
    /// History step in Lightroom does. Later steps stay until a new edit.
    pub(crate) fn go_to(&mut self, applied: usize, current: &mut Recipe) -> bool {
        let mut moved = false;
        while self.undo.len() > applied && self.undo(current) {
            moved = true;
        }
        while self.undo.len() < applied && self.redo(current) {
            moved = true;
        }
        moved
    }
    #[cfg(test)]
    pub(crate) fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    #[cfg(test)]
    pub(crate) fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub(crate) fn in_gesture(&self) -> bool {
        self.gesture.is_some()
    }
    /// Records a pointer gesture still in progress, up to `current`, so a step made
    /// outside the UI (an asynchronous result) lands after it; the rest of the drag
    /// becomes a step of its own.
    pub(crate) fn finish_gesture(&mut self, current: &Recipe) {
        if let Some(before) = self.gesture.take() {
            self.record(before, current);
        }
    }
    /// Undo, redo or a History click changed the recipe this frame, not an edit.
    pub(crate) fn is_replaying(&self) -> bool {
        self.replaying
    }
    pub(crate) fn begin_frame(&mut self) {
        self.replaying = false;
    }

    pub(crate) fn record(&mut self, before: Recipe, after: &Recipe) -> bool {
        let at_before = self.mark();
        let recorded = before.clone();
        if !self.push(before, after) {
            return false;
        }
        self.recorded.push(Recorded {
            sequence: super::sequence(),
            before: recorded,
            after: after.clone(),
            at_before,
            at_after: self.mark(),
        });
        true
    }
    /// Adds a step without handing it to the shared undo log.
    fn push(&mut self, before: Recipe, after: &Recipe) -> bool {
        let label = self.label.take();
        if before == *after {
            return false;
        }
        let mut step = label.unwrap_or_else(|| describe(&before, after));
        step.state = next_state();
        if self.undo.len() == LIMIT
            && let Some((_, oldest)) = self.undo.pop_front()
        {
            self.dropped += 1;
            self.origin = oldest.state;
        }
        self.undo.push_back((before, step));
        self.redo.clear();
        true
    }
    pub(crate) fn undo(&mut self, current: &mut Recipe) -> bool {
        self.replaying = true;
        self.gesture = None;
        let Some((previous, step)) = self.undo.pop_back() else {
            return false;
        };
        self.redo.push((std::mem::replace(current, previous), step));
        true
    }
    pub(crate) fn redo(&mut self, current: &mut Recipe) -> bool {
        self.replaying = true;
        self.gesture = None;
        let Some((next, step)) = self.redo.pop() else {
            return false;
        };
        self.undo
            .push_back((std::mem::replace(current, next), step));
        true
    }
    /// Every recorded state, to update what is derived from the photo rather than
    /// edited, such as Upright's analysis.
    pub(crate) fn states_mut(&mut self) -> impl Iterator<Item = &mut Recipe> {
        self.undo
            .iter_mut()
            .chain(self.redo.iter_mut())
            .map(|(r, _)| r)
            .chain(self.gesture.as_mut())
    }
    /// Observe UI edits after drawing. Undo/redo must not create a new undo entry.
    pub(crate) fn observe(&mut self, before: Recipe, after: &Recipe, pointer_down: bool) -> bool {
        let changed = before != *after;
        if changed && !self.replaying {
            if pointer_down {
                self.gesture.get_or_insert(before);
            } else if self.gesture.is_none() {
                self.record(before, after);
            }
        }
        if !pointer_down && let Some(before) = self.gesture.take() {
            self.record(before, after);
        }
        changed
    }
}

/// A name for an edit no control named: the panel it belongs to.
fn describe(before: &Recipe, after: &Recipe) -> Step {
    let (b, a) = (before, after);
    if a.preset_name != b.preset_name && !a.preset_name.is_empty() {
        return Step::new("Preset", crate::presets::display_name(&a.preset_name));
    }
    let profile = |r: &Recipe| r.profile.as_ref().map(|p| p.name.clone());
    if profile(a) != profile(b) {
        return Step::new("Profile", profile(a).unwrap_or_default());
    }
    if a.profile_amount != b.profile_amount {
        return Step::new("Profile Amount", format!("{:.0}", a.profile_amount * 100.));
    }
    if a.ai_denoise.enabled != b.ai_denoise.enabled {
        return Step::new(
            "AI Denoise",
            if a.ai_denoise.enabled { "On" } else { "Off" },
        );
    }
    if a.ai_denoise.amount != b.ai_denoise.amount {
        return Step::new(
            "AI Denoise Amount",
            format!("{:.0}", a.ai_denoise.amount * 100.),
        );
    }
    if a.effects.monochrome != b.effects.monochrome {
        let treatment = if a.effects.monochrome {
            "Black & White"
        } else {
            "Color"
        };
        return Step::new("Treatment", treatment);
    }
    let name = if (a.temperature, a.tint, a.wb) != (b.temperature, b.tint, b.wb) {
        "White Balance"
    } else if (
        a.crop,
        a.straighten,
        a.constrain_crop,
        a.rotation,
        a.flip_x,
        a.flip_y,
    ) != (
        b.crop,
        b.straighten,
        b.constrain_crop,
        b.rotation,
        b.flip_x,
        b.flip_y,
    ) {
        "Crop"
    } else if a.curve != b.curve
        || a.effects.channels != b.effects.channels
        || a.curve_saturation != b.curve_saturation
        || a.effects.parametric != b.effects.parametric
    {
        "Tone Curve"
    } else if a.point_colors != b.point_colors {
        "Point Color"
    } else if a.hsl != b.hsl || a.effects.gray_mix != b.effects.gray_mix {
        "HSL / Color"
    } else if a.grading != b.grading || a.effects.global_grade != b.effects.global_grade {
        "Color Grading"
    } else if (
        a.lens_builtin,
        a.lens_profile,
        a.lens_ca,
        a.lens_manual_distortion,
    ) != (
        b.lens_builtin,
        b.lens_profile,
        b.lens_ca,
        b.lens_manual_distortion,
    ) {
        "Lens Corrections"
    } else if a.transform != b.transform || a.upright != b.upright {
        "Transform"
    } else if a.retouch != b.retouch {
        return Step::new(retouch_step(b, a), "");
    } else if a.red_eye != b.red_eye {
        return Step::new(red_eye_step(b, a), "");
    } else if a.masks != b.masks {
        return mask_step(b, a);
    } else if a.panels != b.panels {
        return panel_switch_step(b, a);
    } else {
        "Edit"
    };
    Step::new(name, "")
}

/// Lightroom names a panel switch "Enable Tone Curve", "Yes" or "No".
fn panel_switch_step(before: &Recipe, after: &Recipe) -> Step {
    use crate::model::panels::{Panel, PanelState};
    let Some(panel) = Panel::ALL
        .into_iter()
        .find(|p| after.panels.state(*p) != before.panels.state(*p))
    else {
        return Step::new("Edit", "");
    };
    let name = match panel {
        Panel::ToneCurve => "Tone Curve",
        Panel::ColorMixer => "Color Adjustments",
        Panel::BlackWhiteMix => "Grayscale Mix",
        Panel::ColorGrading => "Color Grading",
        Panel::Detail => "Detail",
        Panel::LensCorrections => "Lens Corrections",
        Panel::Transform => "Transform",
        Panel::Effects => "Effects",
        Panel::Calibration => "Calibration",
        Panel::SpotRemoval => "Spot Removal",
        Panel::RedEye => "Red Eye",
        Panel::Masks => "Masks",
    };
    let value = match after.panels.state(panel) {
        PanelState::On => "Yes",
        PanelState::Off => "No",
    };
    Step::new(format!("Enable {name}"), value)
}
/// Lightroom names red eye edits by what happened and the type: "Add Red Eye
/// Correction", "Update Pet Eye Correction", "Delete Red Eye Correction".
fn red_eye_step(before: &Recipe, after: &Recipe) -> String {
    let (b, a) = (&before.red_eye, &after.red_eye);
    // The first correction that differs, from the list that has it.
    let changed = (0..a.len().max(b.len())).find(|i| a.get(*i) != b.get(*i));
    let kind = |list: &[crate::model::red_eye::RedEyeOp]| {
        changed.and_then(|i| list.get(i)).map(|op| op.kind)
    };
    let (verb, kind) = match a.len().cmp(&b.len()) {
        std::cmp::Ordering::Greater => ("Add", a.last().map(|op| op.kind)),
        std::cmp::Ordering::Less => ("Delete", kind(b)),
        std::cmp::Ordering::Equal => ("Update", kind(a)),
    };
    let name = kind.unwrap_or_default().name();
    format!("{verb} {name} Correction")
}
/// Lightroom names spot edits by mode: "Spot Removal", "Clone", "Delete Spot".
fn retouch_step(before: &Recipe, after: &Recipe) -> &'static str {
    use crate::model::retouch::RetouchMode;
    if after.retouch.len() < before.retouch.len() {
        return "Delete Spot";
    }
    let changed = after
        .retouch
        .iter()
        .zip(
            before
                .retouch
                .iter()
                .map(Some)
                .chain(std::iter::repeat(None)),
        )
        .find(|(a, b)| Some(*a) != *b)
        .map(|(a, _)| a.mode);
    match changed {
        Some(RetouchMode::Clone) => "Clone",
        _ => "Spot Removal",
    }
}
/// "Brush Mask" for a new mask, "Mask 2: Exposure" for a changed slider, otherwise
/// what happened to which mask.
fn mask_step(before: &Recipe, after: &Recipe) -> Step {
    let (b, a) = (&before.masks, &after.masks);
    if a.len() > b.len() {
        let kind = a
            .last()
            .and_then(|m| m.components.first())
            .map_or("New", |c| c.shape.kind());
        return Step::new(format!("{kind} Mask"), "");
    }
    if a.len() < b.len() {
        return Step::new("Delete Mask", "");
    }
    let Some(i) = (0..a.len()).find(|i| a[*i] != b[*i]) else {
        return Step::new("Masking", "");
    };
    let (m, old) = (&a[i], &b[i]);
    let name = if m.name.is_empty() {
        format!("Mask {}", i + 1)
    } else {
        m.name.clone()
    };
    if m.components.len() != old.components.len() {
        let what = if m.components.len() > old.components.len() {
            m.components.last().map_or("Add", |c| c.shape.kind())
        } else {
            "Remove Component"
        };
        return Step::new(format!("{name}: {what}"), "");
    }
    if m.components != old.components {
        let kind = m
            .components
            .iter()
            .zip(&old.components)
            .find(|(x, y)| x != y)
            .map_or("Mask", |(c, _)| c.shape.kind());
        return Step::new(format!("{name}: {kind}"), "");
    }
    Step::new(name, "")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_panel_switch_is_one_step_named_as_lightroom_names_it() {
        use crate::model::panels::{Panel, PanelState};
        let mut history = History::default();
        let mut recipe = Recipe::default();
        let before = recipe.clone();
        recipe.panels.set(Panel::ToneCurve, PanelState::Off);
        history.record(before, &recipe);
        assert_eq!(history.undo.len(), 1);
        let step = &history.undo[0].1;
        assert_eq!(
            (step.name.as_str(), step.value.as_str()),
            ("Enable Tone Curve", "No")
        );
        let off = recipe.clone();
        recipe.panels.set(Panel::ColorMixer, PanelState::Off);
        assert_eq!(
            describe(&off, &recipe),
            Step::new("Enable Color Adjustments", "No")
        );
        assert_eq!(
            describe(&recipe, &off),
            Step::new("Enable Color Adjustments", "Yes")
        );
    }
    #[test]
    fn drag_is_one_undo_step_and_replay_does_not_record_itself() {
        let mut history = History::default();
        let mut recipe = Recipe::default();
        let original = recipe.clone();
        for value in [0.25, 0.5, 1.] {
            history.begin_frame();
            let before = recipe.clone();
            recipe.exposure = value;
            assert!(history.observe(before, &recipe, true));
        }
        history.observe(recipe.clone(), &recipe, false);
        assert_eq!(history.undo.len(), 1);
        let before = recipe.clone();
        assert!(history.undo(&mut recipe));
        assert_eq!(recipe, original);
        history.observe(before, &recipe, false);
        assert!(!history.can_undo());
        assert!(history.can_redo());
        history.redo(&mut recipe);
        assert_eq!(recipe.exposure, 1.);
    }
    #[test]
    fn new_edit_discards_redo_and_history_is_bounded() {
        let mut history = History::default();
        let mut recipe = Recipe::default();
        for step in 1..=150 {
            let before = recipe.clone();
            recipe.exposure = step as f32 / 100.;
            history.record(before, &recipe);
        }
        assert_eq!(history.undo.len(), LIMIT);
        history.undo(&mut recipe);
        let before = recipe.clone();
        recipe.exposure = -1.;
        history.record(before, &recipe);
        assert!(!history.can_redo());
        for _ in 0..LIMIT {
            assert!(history.undo(&mut recipe));
        }
        assert!(!history.undo(&mut recipe));
        assert!((recipe.exposure - 0.5).abs() < f32::EPSILON);
    }
    #[test]
    fn unchanged_gesture_and_document_reset_leave_no_history() {
        let mut history = History::default();
        let recipe = Recipe::default();
        let mut changed = recipe.clone();
        changed.exposure = 1.;
        history.observe(recipe.clone(), &changed, true);
        history.observe(changed, &recipe, false);
        assert!(!history.can_undo());
        history.observe(
            recipe.clone(),
            &Recipe {
                exposure: 1.,
                ..Default::default()
            },
            true,
        );
        history = History::default();
        assert!(!history.in_gesture());
        assert!(!history.can_undo());
        assert!(!history.can_redo());
    }
    #[test]
    fn red_and_pet_eye_edits_have_lightroom_names() {
        use crate::model::red_eye::{EyeKind, RedEyeOp};
        let eye = |kind| RedEyeOp {
            kind,
            center: [0.5, 0.5],
            radius: [0.01; 2],
            correlation: 0.,
            pupil_size: 0.5,
            darken: 0.5,
        };
        let before = Recipe::default();
        let mut red = before.clone();
        red.red_eye.push(eye(EyeKind::Red));
        let mut pet = red.clone();
        pet.red_eye.push(eye(EyeKind::Pet { catchlight: None }));
        assert_eq!(describe(&before, &red).name, "Add Red Eye Correction");
        assert_eq!(describe(&red, &pet).name, "Add Pet Eye Correction");
        assert_eq!(describe(&pet, &red).name, "Delete Pet Eye Correction");
        let mut moved = pet.clone();
        moved.red_eye[1].kind = EyeKind::Pet {
            catchlight: Some([0.2, 0.]),
        };
        assert_eq!(describe(&pet, &moved).name, "Update Pet Eye Correction");
    }
    #[test]
    fn retouch_and_mask_edits_have_lightroom_names() {
        let before = Recipe::default();
        let mut after = before.clone();
        after.retouch.push(crate::model::retouch::RetouchOp {
            mode: crate::model::retouch::RetouchMode::Heal,
            shape: crate::model::retouch::RetouchShape::Spot {
                center: [0.5, 0.5],
                radius: 0.01,
            },
            feather: 0.5,
            opacity: 1.,
            offset: [0.02, 0.],
        });
        assert_eq!(describe(&before, &after).name, "Spot Removal");
        assert_eq!(describe(&after, &before).name, "Delete Spot");
        let mut masked = before.clone();
        masked.masks.push(crate::model::masks::MaskGroup {
            components: vec![crate::model::masks::MaskComponent::new(
                crate::model::masks::MaskShape::Brush {
                    strokes: Vec::new(),
                },
            )],
            ..Default::default()
        });
        masked.masks.push(masked.masks[0].clone());
        let mut exposed = masked.clone();
        exposed.masks[1].adjust.exposure = 1.;
        assert_eq!(describe(&before, &masked).name, "Brush Mask");
        assert_eq!(describe(&masked, &exposed).name, "Mask 2");
    }
    #[test]
    fn saved_history_restores_steps_position_and_redo() {
        let mut history = History::default();
        let mut recipe = Recipe::default();
        let original = recipe.clone();
        for (name, value) in [("Exposure", 0.5), ("Exposure", 1.), ("Exposure", 1.5)] {
            history.label(Step::new(name, format!("{value:+.2}")));
            let before = recipe.clone();
            recipe.exposure = value;
            history.record(before, &recipe);
        }
        history.undo(&mut recipe);
        let saved = history.saved(&recipe);
        assert_eq!(saved.applied, 2);
        assert_eq!(saved.steps.len(), 3);
        let mut restored = History::restored(saved, &recipe);
        let names: Vec<_> = restored.steps().0.iter().map(|s| s.value.clone()).collect();
        assert_eq!(names, ["+0.50", "+1.00", "+1.50"]);
        assert_eq!(restored.steps().1, 2);
        assert!(restored.redo(&mut recipe));
        assert_eq!(recipe.exposure, 1.5);
        assert!(restored.go_to(0, &mut recipe));
        assert_eq!(recipe, original);
        // Editing an earlier state starts a new branch, as before.
        restored.go_to(1, &mut recipe);
        let before = recipe.clone();
        recipe.contrast = 0.2;
        restored.record(before, &recipe);
        assert_eq!(restored.steps().0.len(), 2);
        assert!(!restored.can_redo());
    }
    #[test]
    fn an_edit_changed_elsewhere_becomes_the_latest_step() {
        let mut history = History::default();
        let mut recipe = Recipe::default();
        let before = recipe.clone();
        recipe.exposure = 0.5;
        history.record(before, &recipe);
        let saved = history.saved(&recipe);
        // Changed by Undo while another photo was open.
        let pasted = Recipe {
            exposure: 0.5,
            temperature: 4000.,
            ..Default::default()
        };
        let mut restored = History::restored(saved, &pasted);
        let (steps, applied) = restored.steps();
        assert_eq!(applied, 2);
        assert_eq!(steps[1].name, "White Balance");
        let mut current = pasted;
        assert!(restored.undo(&mut current));
        assert_eq!(current, recipe);
    }
    #[test]
    fn each_state_reads_back_done_or_undone_without_moving() {
        let mut history = History::default();
        let mut recipe = Recipe::default();
        let mut states = vec![recipe.clone()];
        for value in [0.5, 1., 1.5] {
            let before = recipe.clone();
            recipe.exposure = value;
            history.record(before, &recipe);
            states.push(recipe.clone());
        }
        history.undo(&mut recipe);
        let shown = recipe.clone();
        for (applied, state) in states.iter().enumerate() {
            assert_eq!(history.state(applied, &recipe).as_ref(), Some(state));
        }
        assert_eq!(history.state(4, &recipe), None);
        // Reading a state changes neither the edit nor where History is.
        assert_eq!(recipe, shown);
        assert_eq!(history.steps().1, 2);
    }
    #[test]
    fn steps_are_named_and_go_to_moves_between_them() {
        let mut history = History::default();
        let mut recipe = Recipe::default();
        let original = recipe.clone();
        history.label(Step::new("Exposure", "+0.50"));
        let before = recipe.clone();
        recipe.exposure = 0.5;
        history.record(before, &recipe);
        let before = recipe.clone();
        recipe.temperature += 100.;
        history.record(before, &recipe);
        let (steps, applied) = history.steps();
        let names: Vec<_> = steps.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Exposure", "White Balance"]);
        assert_eq!(applied, 2);
        assert!(history.go_to(0, &mut recipe));
        assert_eq!(recipe, original);
        assert_eq!(history.steps().1, 0);
        assert!(history.go_to(1, &mut recipe));
        assert_eq!(recipe.exposure, 0.5);
        assert_eq!(recipe.temperature, original.temperature);
        assert_eq!(history.steps().0.len(), 2);
    }
}
