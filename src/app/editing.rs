//! A frame edits exactly one document generation, even when navigation happens mid-frame.
use super::Editor;
use crate::edit_session::FrameOutcome;
use crate::model::recipe::Recipe;
use eframe::egui;

pub(super) struct EditFrame {
    pub(super) command_adjust: bool,
    generation: u64,
    edit: crate::edit_session::Frame,
    modes: RenderModes,
    overlay: super::worker::Overlay,
    aspect: f32,
    export: (u8, u32),
}
/// The view modes a render depends on.
#[derive(Clone, Copy, PartialEq, Eq)]
struct RenderModes {
    crop: bool,
    clipping: crate::rendered::ClipOverlay,
    compare: super::before_after::Compare,
    zoom: bool,
    /// The swatch Point Color's Visualize Range shows.
    visualized: Option<usize>,
}
impl Editor {
    fn render_modes(&self) -> RenderModes {
        RenderModes {
            crop: self.view.is(super::state::Tool::Crop),
            clipping: self.view.clipping.overlay(),
            compare: self.view.compare,
            zoom: self.view.zoom.on,
            visualized: self.visualized_swatch(),
        }
    }
    /// Starts `frame` again on the photo now open, when the frame so far opened
    /// another (a shortcut or the filmstrip moved to the next photo): what is
    /// drawn after edits the new photo, in a frame of its own. The old photo's
    /// frame ends with its document.
    pub(super) fn follow_edit_frame(&mut self, frame: &mut EditFrame) {
        if frame.generation != self.load.id() {
            *frame = self.begin_edit_frame();
        }
    }
    pub(super) fn begin_edit_frame(&mut self) -> EditFrame {
        self.sync_command_revision();
        // Before the frame looks at it, so reading it changes no crop.
        self.read_aspect();
        let frame = EditFrame {
            command_adjust: false,
            generation: self.load.id(),
            edit: self.document.edit.begin(),
            modes: self.render_modes(),
            overlay: self.overlay(),
            aspect: self.view.aspect,
            export: (self.document.export.quality, self.document.export.max_edge),
        };
        // The histogram sets it again while a triangle stays hovered, so the
        // warning goes once the pointer leaves or the histogram is not drawn.
        self.view.clipping.set_hover(None);
        frame
    }
    pub(super) fn finish_edit_frame(&mut self, frame: EditFrame, ctx: &egui::Context) {
        let step =
            ctx.data_mut(|d| d.remove_temp::<(String, String)>(super::widgets::history_step_id()));
        if frame.generation != self.load.id() {
            return;
        }
        // Something other than the wheel changing the edit ends a scroll first, as its
        // own step, before this frame's step name is given.
        // A click (selecting another spot, say) also ends it, edit or not.
        let clicked = ctx.input(|i| {
            i.events
                .iter()
                .any(|e| matches!(e, egui::Event::PointerButton { .. }))
        });
        let edit = if *self.document.edit.recipe() == *frame.edit.before() && !clicked {
            super::brush_scroll::Edit::Unchanged
        } else {
            super::brush_scroll::Edit::Changed
        };
        if self.view.wheel.ends_before(edit) {
            self.document.edit.finish_gesture_before(&frame.edit);
        }
        if self.automation.has_turn()
            && !frame.command_adjust
            && (*self.document.edit.recipe() != *frame.edit.before() || clicked)
        {
            self.automation.end_turn();
            self.document.edit.finish_gesture_before(&frame.edit);
        }
        if let Some((name, value)) = step {
            self.document
                .edit
                .name_next_step(super::history::Step::new(name, value));
        }
        self.leave_compare_for_tools();
        if frame.aspect != self.view.aspect {
            self.fit_aspect();
        }
        // The Guided tool goes with the mode, however it was left: a reset, an undo, a
        // preset, with the Transform panel open or not.
        if self.view.is(super::state::Tool::Guided)
            && self.document.edit.recipe().upright.mode
                != crate::model::transform::UprightMode::Guided
        {
            self.view.tool = super::state::Tool::None;
        }
        // Point Color's dropper goes with its tab: another tab, black & white, an older
        // process or the Library put it away, so a click never adds a hidden swatch.
        if self.view.is(super::state::Tool::PointColor) && !self.point_color_tab_shown() {
            self.view.tool = super::state::Tool::None;
        }
        // The Targeted Adjustment Tool goes with its panel's treatment.
        self.keep_targeted_tool();
        // A sample still being taken is dropped with the dropper.
        if !self.view.is(super::state::Tool::PointColor)
            && self.document.point_color_pick.is_running()
        {
            self.document.point_color_pick.invalidate();
        }
        // A conversion waiting for the photo, once it is decoded and nothing else
        // changed this frame (any edit drops it below).
        if *self.document.edit.recipe() == *frame.edit.before() {
            self.finish_pending_treatment();
        }
        // A wheel scroll sizing a spot is a gesture like a drag: one step once it pauses.
        if let Some(left) = self.view.wheel.remaining() {
            ctx.request_repaint_after(left);
        }
        // A dial turned on a control surface is one too.
        let held = ctx.input(|i| i.pointer.primary_down())
            || self.view.wheel.active()
            || self.automation.turning();
        let gesture = if held {
            crate::edit_session::Gesture::Held
        } else {
            crate::edit_session::Gesture::Released
        };
        let edited = self.document.edit.finish(frame.edit, gesture) == FrameOutcome::Edited;
        // Any change but the Amount's own ends the preset Amount, whether or not the
        // Presets panel is open.
        self.end_stale_preset_amount();
        if edited {
            self.edited();
        }
        if edited || frame.modes != self.render_modes() || frame.overlay != self.overlay() {
            if edited {
                self.schedule_edit();
            } else {
                self.schedule();
            }
        }
        if frame.export != (self.document.export.quality, self.document.export.max_edge) {
            self.document.edit.save_state_mut().mark_changed();
        }
    }
}

impl Editor {
    /// Records an edit made outside an edit frame (a preset loaded from a file, a
    /// result computed off the UI thread) as one History step, `step` or one named
    /// for what changed, with what an edit frame does after a slider moves.
    pub(super) fn commit_edit(&mut self, before: Recipe, step: Option<super::history::Step>) {
        if self.document.edit.commit(before, step) {
            self.edited();
        }
        self.end_stale_preset_amount();
        self.schedule();
    }
    /// Changes the settings outside an edit frame (a result computed off the UI
    /// thread, say) as one History step, `step` or one named for what changed, with
    /// what an edit frame does after a slider moves. A change of nothing is no step.
    pub(super) fn change_edit<T>(
        &mut self,
        step: Option<super::history::Step>,
        edit: impl FnOnce(&mut Recipe) -> T,
    ) -> T {
        let before = self.document.edit.recipe().clone();
        let out = self.document.edit.change(step, edit);
        if *self.document.edit.recipe() != before {
            self.edited();
        }
        self.end_stale_preset_amount();
        self.schedule();
        out
    }
    /// What any change to the photo's settings ends or advances, Undo included;
    /// the edit session has marked it for saving.
    fn edited(&mut self) {
        self.sync_command_revision();
        // A conversion waiting for the photo lapses with any other edit.
        self.document.pending_treatment = None;
    }
}
