//! Red Eye Correction (Lightroom's tool between Remove and Masking): click the centre
//! of an eye, with the circle sized by the mouse wheel or `[` `]`; the red (or, for Pet Eye,
//! glowing) pupil found inside that circle gets a correction. Drag a correction to move
//! it, and a pet eye's catchlight to place it; Pupil Size, Darken and Add Catchlight
//! change the selected one; Delete removes it.
use crate::app::Editor;
use crate::app::develop::tools::retouch_tool::{hint, indented};
use crate::app::ui::design::space;
use crate::app::ui::theme;
use crate::app::ui::widgets::{segmented, slider_with};
use crate::develop::red_eye::PupilGlow;
use crate::develop::{ViewMapping, red_eye};
use crate::model::red_eye::{EyeKind, RedEyeOp};
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};

pub(in crate::app) struct RedEyeTool {
    /// The Type menu: what a new correction corrects.
    pub(in crate::app) pet: PupilType,
    /// Index into the recipe's red eye corrections.
    pub(in crate::app) selected: Option<usize>,
    /// The last circle's radius, as a fraction of the long edge, for clicks.
    pub(in crate::app) size: f32,
    drag: Drag,
}
impl Default for RedEyeTool {
    fn default() -> Self {
        Self {
            pet: PupilType::Red,
            selected: None,
            size: 0.02,
            drag: Drag::None,
        }
    }
}
/// Lightroom's Type menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum PupilType {
    Red,
    Pet,
}
impl PupilType {
    fn of(kind: EyeKind) -> Self {
        match kind {
            EyeKind::Red => PupilType::Red,
            EyeKind::Pet { .. } => PupilType::Pet,
        }
    }
    /// A new correction's kind: a pet eye has Lightroom's catchlight.
    fn kind(self) -> EyeKind {
        match self {
            PupilType::Red => EyeKind::Red,
            PupilType::Pet => EyeKind::Pet {
                catchlight: Some(crate::model::red_eye::DEFAULT_CATCHLIGHT),
            },
        }
    }
}
#[derive(Default)]
enum Drag {
    #[default]
    None,
    /// A press off any correction, placing one there (image space) on release.
    Place([f32; 2]),
    /// Moving a correction; image-space pointer position at the start and the
    /// correction then.
    Move([f32; 2], RedEyeOp),
    /// Placing the selected pet eye's catchlight.
    Catchlight,
}
/// A step of `[` or `]`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SizeStep {
    Smaller,
    Larger,
}
impl RedEyeTool {
    /// The click circle one step smaller or larger, within sizes a pupil can have.
    fn resize(&mut self, step: SizeStep) {
        self.scale(match step {
            SizeStep::Smaller => 1. / 1.15,
            SizeStep::Larger => 1.15,
        });
    }
    /// The click circle scaled by `k`, within sizes a pupil can have.
    pub(in crate::app) fn scale(&mut self, k: f32) {
        self.size = (self.size * k).clamp(0.002, 0.25);
    }
    pub(in crate::app) fn clear_document(&mut self) {
        self.selected = None;
        self.drag = Drag::None;
    }
}
impl Editor {
    /// Handles the pointer on the photo and draws the corrections. The tool owns the
    /// pointer.
    pub(in crate::app) fn red_eye_overlay(
        &mut self,
        ui: &mut egui::Ui,
        response: &egui::Response,
        rect: Rect,
    ) {
        let Some(im) = self.document.full().cloned() else {
            return;
        };
        let recipe = self.effective_recipe();
        let map = ViewMapping::new(&im, &recipe);
        let aspect = map.frame().aspect();
        let to_screen = |p: [f32; 2]| {
            let [u, v] = map.to_view(p);
            Pos2::new(
                rect.left() + u * rect.width(),
                rect.top() + v * rect.height(),
            )
        };
        let to_image = |p: Pos2| {
            map.to_image(
                (p.x - rect.left()) / rect.width(),
                (p.y - rect.top()) / rect.height(),
            )
        };
        let ops = self.document.edit.recipe().red_eye.clone();
        let hit = |pos: Pos2| {
            let at = to_image(pos);
            (0..ops.len()).rev().find(|i| ops[*i].contains(at, aspect))
        };
        let pointer = response.hover_pos();
        let hovered = pointer.and_then(hit);

        // The selected pet eye's catchlight handle, which can lie outside its ellipse.
        let selected = self.view.red_eye.selected;
        let on_catchlight = |pos: Pos2| {
            selected
                .and_then(|i| ops.get(i))
                .and_then(|op| op.catchlight_at(aspect))
                .is_some_and(|c| to_screen(c).distance(pos) < 7.)
        };
        if response.drag_started()
            && let Some(origin) = ui.input(|i| i.pointer.press_origin())
        {
            let at = to_image(origin);
            self.view.red_eye.drag = match hit(origin) {
                _ if on_catchlight(origin) => Drag::Catchlight,
                Some(i) => {
                    self.select_red_eye(Some(i));
                    Drag::Move(at, ops[i].clone())
                }
                None => Drag::Place(at),
            };
        }
        if response.dragged()
            && let Some(pos) = response.interact_pointer_pos()
            && let Drag::Move(start, original) = &self.view.red_eye.drag
            && let Some(i) = self.view.red_eye.selected
            && i < self.document.edit.recipe().red_eye.len()
        {
            let at = to_image(pos);
            let mut op = original.clone();
            op.translate([at[0] - start[0], at[1] - start[1]]);
            self.document.edit.recipe_mut().red_eye[i] = op;
            self.show_red_eye();
        }
        if response.dragged()
            && let Some(pos) = response.interact_pointer_pos()
            && let Drag::Catchlight = self.view.red_eye.drag
            && let Some(i) = self.view.red_eye.selected
            && i < self.document.edit.recipe().red_eye.len()
        {
            self.document.edit.recipe_mut().red_eye[i].set_catchlight(to_image(pos), aspect);
            self.show_red_eye();
        }
        // As in Lightroom, the size comes from the wheel or `[` `]`, not the drag: a
        // drag that starts off a correction places one where it started.
        if response.drag_stopped() {
            if let Drag::Place(center) = std::mem::take(&mut self.view.red_eye.drag) {
                let size = self.view.red_eye.size;
                self.add_red_eye(center, size);
            }
            self.view.red_eye.drag = Drag::None;
        }
        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            match hit(pos) {
                _ if on_catchlight(pos) => {}
                Some(i) => self.select_red_eye(Some(i)),
                None => {
                    let size = self.view.red_eye.size;
                    self.add_red_eye(to_image(pos), size);
                }
            }
        }

        // Drawing.
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        let tool = &self.view.red_eye;
        for (i, op) in self.document.edit.recipe().red_eye.iter().enumerate() {
            let selected = tool.selected == Some(i);
            let points: Vec<Pos2> = op.outline(aspect, 72).into_iter().map(to_screen).collect();
            let width = if selected || hovered == Some(i) {
                1.5
            } else {
                1.
            };
            outline(&painter, points, width);
            if selected {
                crosshair(&painter, to_screen(op.center));
                if let Some(c) = op.catchlight_at(aspect) {
                    let c = to_screen(c);
                    painter.circle_stroke(c, 4., Stroke::new(2.5, Color32::from_black_alpha(110)));
                    painter.circle_stroke(c, 4., Stroke::new(1., Color32::WHITE));
                }
            }
        }
        if let Drag::Place(center) = &tool.drag
            && pointer.is_some()
        {
            let circle = circle_points(*center, tool.size, aspect);
            outline(&painter, circle.into_iter().map(to_screen).collect(), 1.);
            crosshair(&painter, to_screen(*center));
        } else if let Some(pos) = pointer
            && hovered.is_none()
            && rect.contains(pos)
        {
            let circle = circle_points(to_image(pos), tool.size, aspect);
            outline(&painter, circle.into_iter().map(to_screen).collect(), 1.);
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        } else if hovered.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
        }
    }
    /// Corrects the pupil (red, or glowing for Pet Eye) found within `size` (long-edge
    /// fraction) of image position `center`, or says none was found.
    pub(in crate::app) fn add_red_eye(&mut self, center: [f32; 2], size: f32) {
        if self.document.edit.recipe().red_eye.len() >= crate::model::red_eye::MAX_OPS {
            self.messages
                .warn("Too many red eye corrections on this photo");
            return;
        }
        let Some(im) = self.document.full() else {
            return;
        };
        // The selected correction's type, as the Type menu shows it.
        let pet = self
            .view
            .red_eye
            .selected
            .and_then(|i| self.document.edit.recipe().red_eye.get(i))
            .map_or(self.view.red_eye.pet, |op| PupilType::of(op.kind));
        let kind = pet.kind();
        match red_eye::find_pupil(
            im,
            center,
            size.min(crate::model::red_eye::MAX_RADIUS),
            kind.glow(),
        ) {
            Ok(pupil) => {
                let (pupil_size, darken) = self
                    .view
                    .red_eye
                    .selected
                    .and_then(|i| self.document.edit.recipe().red_eye.get(i))
                    .map_or(
                        (
                            crate::model::red_eye::DEFAULT_PUPIL_SIZE,
                            crate::model::red_eye::DEFAULT_DARKEN,
                        ),
                        |op| (op.pupil_size, op.darken),
                    );
                let mut op = RedEyeOp {
                    kind,
                    center: pupil.center,
                    radius: pupil.radius,
                    correlation: pupil.correlation,
                    pupil_size,
                    darken,
                };
                op.fit_catchlight();
                if let Err(e) = op.validate() {
                    self.messages.fail(e.to_string());
                    return;
                }
                self.document.edit.recipe_mut().red_eye.push(op);
                self.show_red_eye();
                self.select_red_eye(Some(self.document.edit.recipe().red_eye.len() - 1));
            }
            // Lightroom's warning.
            Err(_) => self.messages.warn(format!(
                "Unable to find {}. Be sure to use an area that includes the entire eye.",
                kind.name().to_lowercase()
            )),
        }
    }
    /// Selects correction `i`; its type becomes the one new corrections get, as the
    /// Type menu shows it.
    pub(in crate::app) fn select_red_eye(&mut self, i: Option<usize>) {
        self.view.red_eye.selected = i;
        if let Some(op) = i.and_then(|i| self.document.edit.recipe().red_eye.get(i)) {
            self.view.red_eye.pet = PupilType::of(op.kind);
        }
    }
    /// Turns the Red Eye switch on, so a correction just made or changed shows, as
    /// Lightroom does.
    fn show_red_eye(&mut self) {
        use crate::model::panels::{Panel, PanelState};
        self.document
            .edit
            .recipe_mut()
            .panels
            .set(Panel::RedEye, PanelState::On);
    }
    /// The Red Eye tool's keys: Delete removes the selected correction.
    /// Delete removes the selected correction; `[` and `]` resize the circle a click
    /// corrects within, as they size the Remove brush.
    pub(in crate::app) fn red_eye_keys(&mut self, i: &egui::InputState) {
        use egui::Key;
        if i.modifiers.command || i.modifiers.alt {
            return;
        }
        if i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace) {
            self.delete_red_eye();
        }
        let pressed = |keys: &[Key]| keys.iter().any(|k| i.key_pressed(*k));
        if pressed(&[Key::OpenBracket, Key::OpenCurlyBracket]) {
            self.view.red_eye.resize(SizeStep::Smaller);
        }
        if pressed(&[Key::CloseBracket, Key::CloseCurlyBracket]) {
            self.view.red_eye.resize(SizeStep::Larger);
        }
    }
    pub(in crate::app) fn delete_red_eye(&mut self) {
        if let Some(i) = self.view.red_eye.selected.take()
            && i < self.document.edit.recipe().red_eye.len()
        {
            self.document.edit.recipe_mut().red_eye.remove(i);
        }
    }
    /// The Red Eye drawer below the tool strip.
    pub(in crate::app) fn red_eye_panel(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(ui.ctx());
        crate::app::ui::widgets::set_edit_context(ui, "Red Eye");
        let selected = self
            .view
            .red_eye
            .selected
            .filter(|i| *i < self.document.edit.recipe().red_eye.len());
        crate::app::develop::tools::retouch_tool::control_label(ui, "Type", |ui| {
            let mut pet = selected.map_or(self.view.red_eye.pet, |i| {
                PupilType::of(self.document.edit.recipe().red_eye[i].kind)
            });
            let w = ui.available_width();
            if segmented(
                ui,
                &mut pet,
                &[(PupilType::Red, "Red Eye"), (PupilType::Pet, "Pet Eye")],
                w,
            ) {
                self.view.red_eye.pet = pet;
                if let Some(i) = selected {
                    let op = &mut self.document.edit.recipe_mut().red_eye[i];
                    op.kind = pet.kind();
                    op.fit_catchlight();
                    self.show_red_eye();
                }
            }
        });
        match selected {
            Some(i) => {
                let op = &mut self.document.edit.recipe_mut().red_eye[i];
                let before = op.clone();
                slider_with(
                    ui,
                    &mut self.step_name,
                    "Pupil Size",
                    &mut op.pupil_size,
                    0. ..=1.,
                    crate::model::red_eye::DEFAULT_PUPIL_SIZE,
                    Some((100., 0)),
                    None,
                );
                match &mut op.kind {
                    // Camera Raw's Pet Eye is black whatever Darken says.
                    EyeKind::Red => {
                        slider_with(
                            ui,
                            &mut self.step_name,
                            "Darken",
                            &mut op.darken,
                            0. ..=1.,
                            crate::model::red_eye::DEFAULT_DARKEN,
                            Some((100., 0)),
                            None,
                        );
                    }
                    EyeKind::Pet { catchlight } => indented(ui, |ui| {
                        let mut on = catchlight.is_some();
                        if ui
                            .checkbox(&mut on, "Add Catchlight")
                            .on_hover_text("Drag the catchlight to place it")
                            .changed()
                        {
                            *catchlight = on.then_some(crate::model::red_eye::DEFAULT_CATCHLIGHT);
                        }
                    }),
                }
                op.fit_catchlight();
                if *op != before {
                    let name = format!("Update {} Correction", op.kind.name());
                    self.show_red_eye();
                    self.step_name.name(name, String::new());
                }
            }
            None => hint(ui, "Select a correction to change it"),
        }
        ui.add_space(space::XS);
        hint(
            ui,
            "Click the center of the eye; scroll or [ ] to size the circle",
        );
        hint(ui, "Delete removes the selected correction");
        ui.add_space(space::XS);
        indented(ui, |ui| {
            let w = (ui.available_width() - 4.) / 2.;
            let any = !self.document.edit.recipe().red_eye.is_empty();
            if ui
                .add_enabled(any, egui::Button::new("Reset").min_size(Vec2::new(w, 22.)))
                .on_hover_text("Remove every red eye correction")
                .clicked()
            {
                self.document.edit.recipe_mut().red_eye.clear();
                self.view.red_eye.selected = None;
            }
            if ui
                .add_sized(
                    [w, 22.],
                    egui::Button::new(egui::RichText::new("Close").color(palette.on_accent()))
                        .fill(palette.accent()),
                )
                .on_hover_text("Close the tool")
                .clicked()
            {
                self.view.tool = crate::app::develop::state::Tool::None;
            }
        });
    }
}
/// A circle of `radius` (long-edge fraction) around image position `center`.
fn circle_points(center: [f32; 2], radius: f32, aspect: f32) -> Vec<[f32; 2]> {
    RedEyeOp {
        kind: EyeKind::Red,
        center,
        radius: [radius; 2],
        correlation: 0.,
        pupil_size: 0.,
        darken: 0.,
    }
    .outline(aspect, 72)
}
fn outline(painter: &egui::Painter, points: Vec<Pos2>, width: f32) {
    painter.add(egui::Shape::closed_line(
        points.clone(),
        Stroke::new(width + 1.5, Color32::from_black_alpha(110)),
    ));
    painter.add(egui::Shape::closed_line(
        points,
        Stroke::new(width, Color32::from_white_alpha(230)),
    ));
}
fn crosshair(painter: &egui::Painter, at: Pos2) {
    for (a, b) in [
        (Vec2::new(-5., 0.), Vec2::new(5., 0.)),
        (Vec2::new(0., -5.), Vec2::new(0., 5.)),
    ] {
        painter.line_segment(
            [at + a, at + b],
            Stroke::new(2.5, Color32::from_black_alpha(110)),
        );
        painter.line_segment([at + a, at + b], Stroke::new(1., Color32::WHITE));
    }
}

#[cfg(test)]
#[path = "red_eye_tool_editor_tests.rs"]
mod editor_tests;
