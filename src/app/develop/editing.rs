//! A frame edits exactly one document generation, even when navigation happens mid-frame.
use crate::app::Editor;
use crate::edit_session::FrameOutcome;
use crate::model::recipe::Recipe;
use eframe::egui;

pub(in crate::app) struct EditFrame {
    pub(in crate::app) command_adjust: bool,
    generation: u64,
    edit: crate::edit_session::Frame,
    modes: RenderModes,
    overlay: crate::app::worker::Overlay,
    aspect: f32,
    export: (u8, u32),
}
/// The view modes a render depends on.
#[derive(Clone, Copy, PartialEq, Eq)]
struct RenderModes {
    crop: bool,
    clipping: crate::rendered::ClipOverlay,
    compare: crate::app::develop::before_after::Compare,
    zoom: bool,
    /// The swatch Point Color's Visualize Range shows.
    visualized: Option<usize>,
}
impl Editor {
    fn render_modes(&self) -> RenderModes {
        RenderModes {
            crop: self.view.is(crate::app::develop::state::Tool::Crop),
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
    pub(in crate::app) fn follow_edit_frame(&mut self, frame: &mut EditFrame) {
        if frame.generation != self.document.load_id() {
            *frame = self.begin_edit_frame();
        }
    }
    pub(in crate::app) fn begin_edit_frame(&mut self) -> EditFrame {
        // Before the frame and any command look at the settings: an uncropped photo
        // taking the chosen aspect is an edit of its own, not part of what follows.
        self.read_aspect();
        self.sync_command_revision();
        let frame = EditFrame {
            command_adjust: false,
            generation: self.document.load_id(),
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
    /// [`Self::begin_edit_frame`] for a frame that draws Develop: a mask's pin or row
    /// is found hovered again as it is drawn, after the frame took the overlay it
    /// showed. A command's frame, which draws nothing, keeps the hover.
    pub(in crate::app) fn begin_develop_frame(&mut self) -> EditFrame {
        let frame = self.begin_edit_frame();
        self.view.masking.leave();
        frame
    }
    /// A dial turned on a control surface is one History step once it pauses. A
    /// drawn frame ends it in [`Self::finish_edit_frame`]; this ends it where no
    /// frame is drawn (the window hidden), so the step is taken and saved.
    pub(in crate::app) fn finish_paused_turn(&mut self, ctx: &egui::Context) {
        // A hidden window keeps the pointer of its last drawn frame: a press then
        // holds nothing once the window lost focus.
        let held =
            ctx.input(|i| i.raw.focused && i.pointer.primary_down()) || self.view.wheel.active();
        if self.automation.has_turn() && !self.automation.turning() && !held {
            self.automation.end_turn();
            self.finish_gesture();
        }
    }
    pub(in crate::app) fn finish_edit_frame(&mut self, frame: EditFrame, ctx: &egui::Context) {
        let step = self.step_name.take();
        if frame.generation != self.document.load_id() {
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
            crate::app::develop::tools::brush_scroll::Edit::Unchanged
        } else {
            crate::app::develop::tools::brush_scroll::Edit::Changed
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
        if let Some(step) = step {
            self.document.edit.name_next_step(step);
        }
        self.leave_compare_for_tools();
        if frame.aspect != self.view.aspect {
            self.fit_aspect();
        }
        // The Guided tool goes with the mode, however it was left: a reset, an undo, a
        // preset, with the Transform panel open or not.
        if self.view.is(crate::app::develop::state::Tool::Guided)
            && self.document.edit.recipe().upright.mode
                != crate::model::transform::UprightMode::Guided
        {
            self.view.tool = crate::app::develop::state::Tool::None;
        }
        // Point Color's dropper goes with its tab: another tab, black & white, an older
        // process or the Library put it away, so a click never adds a hidden swatch.
        if self.view.is(crate::app::develop::state::Tool::PointColor)
            && !self.point_color_tab_shown()
        {
            self.view.tool = crate::app::develop::state::Tool::None;
        }
        // The Targeted Adjustment Tool goes with its panel's treatment.
        self.keep_targeted_tool();
        // A sample still being taken is dropped with the dropper.
        if !self.view.is(crate::app::develop::state::Tool::PointColor)
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
    pub(in crate::app) fn commit_edit(
        &mut self,
        before: Recipe,
        step: Option<crate::app::history::Step>,
    ) {
        if self.document.edit.commit(before, step) {
            self.edited();
        }
        self.end_stale_preset_amount();
        self.schedule();
    }
    /// Changes the settings outside an edit frame (a result computed off the UI
    /// thread, say) as one History step, `step` or one named for what changed, with
    /// what an edit frame does after a slider moves. A change of nothing is no step.
    pub(in crate::app) fn change_edit<T>(
        &mut self,
        step: Option<crate::app::history::Step>,
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

#[cfg(test)]
#[path = "editing_editor_tests.rs"]
mod editor_tests;
