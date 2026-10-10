//! Lightroom's Targeted Adjustment Tool for the Tone Curve, the Color Mixer's HSL
//! sliders and the B&W mix: a drag up or down on the photo moves the sliders that
//! change the color where it started (`develop::targeted`). The color is sampled off
//! the UI thread as the drag starts; what is dragged before it arrives is applied
//! when it does. Each drag is one History step; Esc or the panel's target button puts
//! the tool away.
use super::state::Tool;
use super::{Editor, history::Step, theme};
use crate::app::Module;
use crate::develop::targeted::{DRAG_RATE, HslChannel, Target, TargetSample, TargetWeights};
use crate::model::recipe::Recipe;
use crate::model::recipe::Treatment;
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};

/// A drag with the tool, from the press to the sample's arrival or the release,
/// whichever comes last.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct TargetDrag {
    target: Target,
    /// Where it started, in the photo's screen rectangle (0–1).
    at: [f32; 2],
    /// The edit when it started, which the sliders move from.
    start: Recipe,
    /// The edit as the drag last left it; anything else changing it ends the drag.
    last: Recipe,
    /// Points dragged up (down is negative).
    travel: f32,
    /// How the drag is shared among the sliders, once the sample is in.
    weights: Option<TargetWeights>,
    pointer: Pointer,
}

/// Whether the drag's button is still down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pointer {
    Down,
    Released,
}

impl TargetDrag {
    /// The sliders the drag moves, for the panel to highlight, once known.
    pub(super) fn weights(&self) -> Option<&TargetWeights> {
        self.weights.as_ref().filter(|w| !w.is_empty())
    }
}

impl Editor {
    /// Opens the tool for `target`, or puts it away when it is open, and shows the
    /// sliders it moves.
    pub(super) fn toggle_targeted(&mut self, target: Target) {
        if !self.targeted_available(target) {
            self.status = match target {
                Target::BlackWhite => "The B&W mix adjusts black & white photos".into(),
                _ => "The Color Mixer adjusts color photos; this one is black & white".into(),
            };
            return;
        }
        self.view.toggle(Tool::Targeted(target));
        match target {
            Target::ToneCurve => self.view.parametric_curve = true,
            Target::Hsl(channel) => {
                self.view.mixer_tab = super::state::MixerTab::Mixer;
                self.view.mixer_color = false;
                // All shows every channel already.
                if self.view.mixer_adjust != 3 {
                    self.view.mixer_adjust = channel.index();
                }
            }
            Target::BlackWhite => {}
        }
    }
    /// Whether the panel `target` adjusts is the one this photo shows.
    pub(super) fn targeted_available(&self, target: Target) -> bool {
        let r = self.document.edit.recipe();
        let black_white = r.treatment() == Treatment::BlackWhite;
        match target {
            Target::ToneCurve => true,
            Target::Hsl(_) => !black_white,
            Target::BlackWhite => black_white,
        }
    }
    /// Whether the sliders `target` moves are on show: the parametric curve, or the
    /// HSL view with its channel.
    fn targeted_shown(&self, target: Target) -> bool {
        let v = &self.view;
        match target {
            Target::ToneCurve => v.parametric_curve,
            Target::Hsl(channel) => {
                v.mixer_tab == super::state::MixerTab::Mixer
                    && !v.mixer_color
                    && (v.mixer_adjust == 3 || v.mixer_adjust == channel.index())
            }
            Target::BlackWhite => true,
        }
    }
    /// Puts the tool away when its sliders are no longer shown (the Library, a
    /// conversion to or from black & white, another view of the panel), and drops a
    /// drag it leaves behind, or one for another target.
    pub(super) fn keep_targeted_tool(&mut self) {
        // Before shown: a drag still waiting for its sample is dropped.
        if self.view.compare.shows_before() && self.view.targeted.is_some() {
            self.view.targeted = None;
            self.document.targeted_pick.invalidate();
        }
        if let Tool::Targeted(target) = self.view.tool
            && (self.module == Module::Library
                || !self.targeted_available(target)
                || !self.targeted_shown(target))
        {
            self.view.tool = Tool::None;
        }
        let current = match self.view.tool {
            Tool::Targeted(target) => Some(target),
            _ => None,
        };
        if self
            .view
            .targeted
            .as_ref()
            .is_some_and(|d| Some(d.target) != current)
        {
            self.view.targeted = None;
            self.document.targeted_pick.invalidate();
        }
    }
    /// The tool over the photo in `rect`: a press starts a drag and samples the color
    /// there; dragging moves the sliders.
    pub(super) fn targeted_overlay(
        &mut self,
        ui: &mut egui::Ui,
        response: &egui::Response,
        rect: Rect,
        target: Target,
    ) {
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        let origin = ui.input(|i| i.pointer.press_origin());
        if response.drag_started()
            && let Some(origin) = origin.filter(|p| rect.contains(*p))
        {
            let at = [
                (origin.x - rect.left()) / rect.width(),
                (origin.y - rect.top()) / rect.height(),
            ];
            self.start_targeted_drag(target, at);
        }
        if response.dragged() {
            self.drag_targeted(-response.drag_delta().y);
        }
        if response.drag_stopped() {
            // Travel made while the sample was on its way lands even without a last move.
            self.drag_targeted(0.);
            self.release_targeted();
        }
        if let Some(drag) = &self.view.targeted {
            let at = rect.min + Vec2::new(drag.at[0], drag.at[1]) * rect.size();
            target_icon(ui.painter(), at, 9., Color32::WHITE);
        }
    }
    /// Starts a drag at (`u`, `v`) of the shown photo, sampling the color there.
    fn start_targeted_drag(&mut self, target: Target, at: [f32; 2]) {
        self.view.targeted = Some(TargetDrag {
            target,
            at,
            start: self.document.edit.recipe().clone(),
            last: self.document.edit.recipe().clone(),
            travel: 0.,
            weights: None,
            pointer: Pointer::Down,
        });
        let Some(im) = self.document.full().cloned() else {
            self.view.targeted = None;
            return;
        };
        let (generation, cancel) = self.document.targeted_pick.start();
        let id = self.load.id();
        let sampled = self.document.edit.recipe().clone();
        let tx = self.tx.clone();
        let ctx = self.context.clone();
        std::thread::spawn(move || {
            let [u, v] = at;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::develop::quality::targeted_sample(&im, &sampled, u, v, &cancel)
            }))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("sampling failed unexpectedly")))
            .map_err(|e| format!("Cannot sample the photo: {e:#}"));
            let _ = tx.send(super::worker::Event::TargetedSample {
                id,
                generation,
                sampled: Box::new(sampled),
                result,
            });
            ctx.request_repaint();
        });
    }
    /// The pointer moved `up` points up during the drag: the sliders follow, once
    /// the sample is in. Part of the pointer gesture, so one History step.
    fn drag_targeted(&mut self, up: f32) {
        let Some(drag) = &mut self.view.targeted else {
            return;
        };
        // Changed by something else (Auto finishing, say): the sample no longer holds.
        if *self.document.edit.recipe() != drag.last {
            self.view.targeted = None;
            return;
        }
        drag.travel += up;
        if let Some(weights) = drag.weights.filter(|w| !w.is_empty()) {
            let before = self.document.edit.recipe().clone();
            weights.apply(
                &drag.start,
                drag.travel * DRAG_RATE,
                self.document.edit.recipe_mut(),
            );
            // Named through the frame, as a slider's drag is; only when it moved, so
            // the name never lands on another edit.
            if *self.document.edit.recipe() != before {
                let (name, value) = weights.step(self.document.edit.recipe());
                super::widgets::name_frame_step(&self.context, name, value);
            }
            drag.last = self.document.edit.recipe().clone();
        }
    }
    /// Ends a drag whose button came up elsewhere (Space or Before took over).
    pub(super) fn end_targeted_drag(&mut self) {
        self.drag_targeted(0.);
        self.release_targeted();
    }
    /// The button is up. A drag whose sample is in is done (the gesture records it);
    /// one still waiting is finished when the sample arrives.
    fn release_targeted(&mut self) {
        match &mut self.view.targeted {
            Some(drag) if drag.weights.is_none() => drag.pointer = Pointer::Released,
            _ => self.view.targeted = None,
        }
    }
    /// The sample for the drag under way: it now moves the sliders, or, when the
    /// button is already up, its whole travel is applied as one History step.
    pub(super) fn targeted_sample_ready(
        &mut self,
        sampled: &Recipe,
        result: Result<TargetSample, String>,
    ) {
        self.document.targeted_pick.invalidate();
        // Only where the edit is shown and can be adjusted.
        if self.module == Module::Library || self.view.compare.shows_before() {
            self.view.targeted = None;
            return;
        }
        let Some(drag) = &mut self.view.targeted else {
            return;
        };
        // Taken from the edit the drag started on, and nothing (Auto finishing, say)
        // has changed it since.
        if *sampled != drag.start || *self.document.edit.recipe() != drag.start {
            self.view.targeted = None;
            return;
        }
        let sample = match result {
            Ok(sample) => sample,
            Err(e) => {
                self.status = e;
                self.view.targeted = None;
                return;
            }
        };
        let weights = TargetWeights::new(drag.target, &sample, sampled);
        if weights.is_empty() {
            self.status = "Nothing to adjust there: drag over a colored area".into();
        }
        drag.weights = Some(weights);
        if drag.pointer == Pointer::Released {
            let drag = self.view.targeted.take().expect("checked above");
            if drag.travel != 0.
                && !weights.is_empty()
                && *self.document.edit.recipe() == drag.start
            {
                let mut adjusted = drag.start.clone();
                weights.apply(&drag.start, drag.travel * DRAG_RATE, &mut adjusted);
                let (name, value) = weights.step(&adjusted);
                self.change_edit(Some(Step::new(name, value)), |r| *r = adjusted);
            }
        }
    }
    /// The sliders a drag is moving, for the panel to highlight.
    pub(super) fn targeted_weights(&self) -> Option<TargetWeights> {
        self.view
            .targeted
            .as_ref()
            .and_then(|d| d.weights())
            .copied()
    }
}

/// The tool's button in a panel: a target, lit while the tool is open. Returns
/// whether it was clicked.
pub(super) fn target_button(ui: &mut egui::Ui, rect: Rect, active: bool, tip: &str) -> bool {
    let palette = theme::palette(ui.ctx());
    let response = ui
        .interact(rect, ui.id().with(("targeted", tip)), Sense::click())
        .on_hover_text(tip);
    let fill = if active {
        palette.gray(72)
    } else if response.hovered() {
        palette.gray(50)
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 3., fill);
    let ink = palette.gray(if active || response.hovered() {
        235
    } else {
        175
    });
    target_icon(ui.painter(), rect.center(), rect.height() * 0.32, ink);
    response.clicked()
}

/// A target with up and down arrows, `radius` points across its ring.
fn target_icon(painter: &egui::Painter, c: Pos2, radius: f32, ink: Color32) {
    let stroke = Stroke::new(1.3, ink);
    painter.circle_stroke(c, radius, stroke);
    painter.circle_filled(c, 1.4, ink);
    let arrow = radius * 0.55;
    for dir in [-1., 1.] {
        let tip = c + Vec2::new(0., dir * (radius + arrow + 1.));
        let base = tip - Vec2::new(0., dir * arrow);
        painter.add(egui::Shape::convex_polygon(
            vec![
                tip,
                base + Vec2::new(arrow * 0.8, 0.),
                base - Vec2::new(arrow * 0.8, 0.),
            ],
            ink,
            Stroke::NONE,
        ));
    }
}

/// What the HSL target button adjusts for the Adjust tab `index` (0 Hue, 1
/// Saturation, 2 Luminance).
pub(super) fn hsl_target(index: usize) -> Target {
    Target::Hsl(HslChannel::ALL[index.min(2)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera_data::{CameraImage, Metadata};
    use crate::model::panels::{Panel, PanelState};
    use std::sync::Arc;

    /// An editor showing a 200 × 200 photo: an orange left half, a gray right one.
    fn editor(ctx: &egui::Context) -> Editor {
        let mut editor =
            Editor::with_context(ctx, None, crate::app::session::Session::default(), None);
        let image = Arc::new(CameraImage {
            recovered: Default::default(),
            width: 200,
            height: 200,
            pixels: (0..40000)
                .map(|i| {
                    if i % 200 < 100 {
                        [0.5, 0.25, 0.08]
                    } else {
                        [0.2; 3]
                    }
                })
                .collect(),
            metadata: Metadata {
                width: 200,
                height: 200,
                wb: [1.; 3],
                daylight_wb: [1.; 3],
                matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                ..Default::default()
            },
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
        });
        editor.document.set_image(image);
        editor.preview.texture = Some(
            ctx.load_texture(
                "photo",
                egui::ColorImage::filled([200, 200], Color32::GRAY),
                egui::TextureOptions::LINEAR,
            )
            .into(),
        );
        editor
    }

    /// One frame of the photo, as an edit frame, with `events`.
    fn frame(ctx: &egui::Context, editor: &mut Editor, events: Vec<egui::Event>) {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.))),
                events,
                ..Default::default()
            },
            |ui| {
                editor.events(ui.ctx());
                let edit = editor.begin_edit_frame();
                editor.viewport_ui(ui);
                editor.finish_edit_frame(edit, ui.ctx());
            },
        );
        output.textures_delta.clear();
    }

    fn button(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// Waits for the drag's sample, drawing frames with no input meanwhile.
    fn wait_for_sample(ctx: &egui::Context, editor: &mut Editor) {
        let start = std::time::Instant::now();
        while editor.document.targeted_pick.is_running() {
            assert!(start.elapsed().as_secs() < 60, "no sample");
            std::thread::sleep(std::time::Duration::from_millis(5));
            editor.events(ctx);
        }
    }

    #[test]
    fn a_drag_on_orange_raises_its_saturation_as_one_step() {
        let ctx = egui::Context::default();
        let mut e = editor(&ctx);
        e.document
            .edit
            .setup_mut()
            .panels
            .set(Panel::ColorMixer, PanelState::Off);
        e.toggle_targeted(Target::Hsl(HslChannel::Saturation));
        assert_eq!(
            e.view.tool,
            Tool::Targeted(Target::Hsl(HslChannel::Saturation))
        );
        assert_eq!(e.view.mixer_adjust, 1);
        frame(&ctx, &mut e, vec![]);
        let at = Pos2::new(50., 100.);
        frame(
            &ctx,
            &mut e,
            vec![egui::Event::PointerMoved(at), button(at, true)],
        );
        // Past egui's drag threshold, the drag starts and samples the press point.
        let mut y = 100.;
        for _ in 0..3 {
            y -= 10.;
            frame(
                &ctx,
                &mut e,
                vec![egui::Event::PointerMoved(Pos2::new(50., y))],
            );
        }
        assert!(e.view.targeted.is_some());
        wait_for_sample(&ctx, &mut e);
        for _ in 0..2 {
            y -= 10.;
            frame(
                &ctx,
                &mut e,
                vec![egui::Event::PointerMoved(Pos2::new(50., y))],
            );
        }
        let weights = e.targeted_weights().expect("sampled");
        assert_eq!(weights.shares[1], 1., "orange leads: {:?}", weights.shares);
        let end = Pos2::new(50., y);
        frame(&ctx, &mut e, vec![button(end, false)]);
        frame(&ctx, &mut e, vec![]);
        let r = e.document.edit.recipe();
        // Dragged up 50 points from the press: orange saturation up by 50/250.
        let travelled = (100. - y) * DRAG_RATE;
        assert!(
            (r.hsl[1][1] - travelled).abs() < 0.05 && r.hsl[1][1] > 0.,
            "{:?}",
            r.hsl
        );
        assert!(r.hsl.iter().all(|b| b[0] == 0. && b[2] == 0.));
        assert_eq!(r.panels.state(Panel::ColorMixer), PanelState::On);
        let (steps, applied) = e.document.edit.history().steps();
        assert_eq!(applied, 1);
        assert_eq!(steps[0].name, "Orange Saturation");
        assert!(e.view.targeted.is_none());
        // The tool stays until Esc or its button puts it away.
        assert!(matches!(e.view.tool, Tool::Targeted(_)));
        e.toggle_targeted(Target::Hsl(HslChannel::Saturation));
        assert_eq!(e.view.tool, Tool::None);
    }

    #[test]
    fn a_drag_released_before_its_sample_is_applied_when_it_arrives() {
        let ctx = egui::Context::default();
        let mut e = editor(&ctx);
        e.toggle_targeted(Target::ToneCurve);
        assert!(e.view.parametric_curve);
        let before = e.document.edit.recipe().clone();
        e.start_targeted_drag(Target::ToneCurve, [0.75, 0.5]);
        crate::app::tests::in_edit_frame(&ctx, &mut e, |e| e.drag_targeted(25.));
        e.release_targeted();
        assert_eq!(*e.document.edit.recipe(), before);
        wait_for_sample(&ctx, &mut e);
        let r = e.document.edit.recipe();
        let moved: Vec<usize> = (0..4).filter(|i| r.effects.parametric[*i] != 0.).collect();
        assert_eq!(moved.len(), 1, "{:?}", r.effects.parametric);
        assert!((r.effects.parametric[moved[0]] - 0.1).abs() < 1e-6);
        let (steps, applied) = e.document.edit.history().steps();
        assert_eq!(applied, 1);
        assert!(steps[0].name.starts_with("Region "), "{}", steps[0].name);
        e.undo();
        assert_eq!(*e.document.edit.recipe(), before);
    }

    #[test]
    fn travel_before_the_sample_lands_when_released_without_moving_again() {
        let ctx = egui::Context::default();
        let mut e = editor(&ctx);
        e.toggle_targeted(Target::ToneCurve);
        e.start_targeted_drag(Target::ToneCurve, [0.75, 0.5]);
        crate::app::tests::in_edit_frame(&ctx, &mut e, |e| e.drag_targeted(25.));
        // The sample arrives while the button is still down; then it is released.
        wait_for_sample(&ctx, &mut e);
        assert!(e.view.targeted.is_some());
        crate::app::tests::in_edit_frame(&ctx, &mut e, |e| e.drag_targeted(0.));
        e.release_targeted();
        assert!(
            (e.document
                .edit
                .recipe()
                .effects
                .parametric
                .iter()
                .sum::<f32>()
                - 0.1)
                .abs()
                < 1e-6
        );
    }

    #[test]
    fn a_sample_is_dropped_when_the_edit_changed_meanwhile() {
        let ctx = egui::Context::default();
        let mut e = editor(&ctx);
        e.toggle_targeted(Target::ToneCurve);
        e.start_targeted_drag(Target::ToneCurve, [0.75, 0.5]);
        e.document.edit.setup_mut().exposure = 0.7;
        wait_for_sample(&ctx, &mut e);
        assert!(e.view.targeted.is_none());
        // Or after it arrived, while the drag goes on.
        e.start_targeted_drag(Target::ToneCurve, [0.75, 0.5]);
        wait_for_sample(&ctx, &mut e);
        crate::app::tests::in_edit_frame(&ctx, &mut e, |e| e.drag_targeted(10.));
        e.document.edit.setup_mut().exposure = 0.2;
        let changed = e.document.edit.recipe().clone();
        crate::app::tests::in_edit_frame(&ctx, &mut e, |e| e.drag_targeted(10.));
        assert!(e.view.targeted.is_none());
        assert_eq!(*e.document.edit.recipe(), changed);
    }

    #[test]
    fn neutral_colors_move_nothing_and_the_tool_follows_the_treatment() {
        let ctx = egui::Context::default();
        let mut e = editor(&ctx);
        let before = e.document.edit.recipe().clone();
        e.toggle_targeted(Target::Hsl(HslChannel::Hue));
        e.start_targeted_drag(Target::Hsl(HslChannel::Hue), [0.75, 0.5]);
        wait_for_sample(&ctx, &mut e);
        crate::app::tests::in_edit_frame(&ctx, &mut e, |e| e.drag_targeted(40.));
        assert_eq!(*e.document.edit.recipe(), before);
        assert!(e.status.contains("Nothing to adjust"), "{}", e.status);
        // B&W's tool is for black & white photos only; converting puts the Color
        // Mixer's away.
        e.toggle_targeted(Target::BlackWhite);
        assert!(matches!(e.view.tool, Tool::Targeted(Target::Hsl(_))));
        e.document.edit.setup_mut().effects.monochrome = true;
        e.keep_targeted_tool();
        assert_eq!(e.view.tool, Tool::None);
        assert!(e.view.targeted.is_none());
        e.toggle_targeted(Target::BlackWhite);
        assert_eq!(e.view.tool, Tool::Targeted(Target::BlackWhite));
        // Hiding the sliders a tool moves puts it away, and another target drops a
        // drag still waiting for its sample.
        e.document.edit.setup_mut().effects.monochrome = false;
        e.toggle_targeted(Target::ToneCurve);
        e.view.parametric_curve = false;
        e.keep_targeted_tool();
        assert_eq!(e.view.tool, Tool::None);
        e.toggle_targeted(Target::Hsl(HslChannel::Hue));
        e.view.mixer_adjust = 3;
        e.start_targeted_drag(Target::Hsl(HslChannel::Hue), [0.25, 0.5]);
        e.toggle_targeted(Target::Hsl(HslChannel::Saturation));
        e.keep_targeted_tool();
        assert!(e.view.targeted.is_none());
    }

    #[test]
    fn lightrooms_shortcuts_open_each_tool_and_esc_puts_it_away() {
        let ctx = egui::Context::default();
        let mut e = editor(&ctx);
        let press = |e: &mut Editor, key: egui::Key, modifiers: egui::Modifiers| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events: std::iter::once(egui::Event::ModifiersChanged(modifiers))
                        .chain([true, false].map(|pressed| egui::Event::Key {
                            key,
                            physical_key: Some(key),
                            pressed,
                            repeat: false,
                            modifiers,
                        }))
                        .collect(),
                    ..Default::default()
                },
                |ui| e.develop_shortcuts(ui.ctx()),
            );
            output.textures_delta.clear();
        };
        let all = egui::Modifiers::COMMAND | egui::Modifiers::ALT | egui::Modifiers::SHIFT;
        for (key, target) in [
            (egui::Key::T, Target::ToneCurve),
            (egui::Key::H, Target::Hsl(HslChannel::Hue)),
            (egui::Key::S, Target::Hsl(HslChannel::Saturation)),
            (egui::Key::L, Target::Hsl(HslChannel::Luminance)),
        ] {
            press(&mut e, key, all);
            assert_eq!(e.view.tool, Tool::Targeted(target), "{key:?}");
        }
        press(&mut e, egui::Key::Escape, egui::Modifiers::NONE);
        assert_eq!(e.view.tool, Tool::None);
        // B&W's is for black & white photos.
        press(&mut e, egui::Key::G, all);
        assert_eq!(e.view.tool, Tool::None);
        e.document.edit.setup_mut().effects.monochrome = true;
        press(&mut e, egui::Key::G, all);
        assert_eq!(e.view.tool, Tool::Targeted(Target::BlackWhite));
    }
}
