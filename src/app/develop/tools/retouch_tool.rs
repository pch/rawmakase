//! Spot removal (Lightroom's Remove panel, Heal and Clone modes): tool settings, the
//! selected spot, pointer handling on the photo and the panel drawer.
//!
//! Click adds a spot with an automatically chosen source; a drag paints a brushed
//! area, whose source is chosen when the stroke ends; Cmd/Ctrl-drag places a spot and
//! drags its source by hand. Dragging a spot's pin or circle moves it, dragging its
//! source circle moves the source.
use crate::app::Editor;
use crate::app::develop::tools::overlay;
use crate::app::ui::design::{Tone, space, text};
use crate::app::ui::theme;
use crate::app::ui::widgets::{segmented, slider_with};
use crate::develop::{ViewMapping, retouch};
use crate::model::retouch::{RetouchMode, RetouchOp, RetouchShape};
use eframe::egui::{self, Color32, Pos2, Rect, Vec2};

pub(in crate::app) struct RetouchTool {
    pub(in crate::app) mode: RetouchMode,
    /// Brush size for new spots, as a fraction of the long edge.
    pub(in crate::app) size: f32,
    pub(in crate::app) feather: f32,
    pub(in crate::app) opacity: f32,
    /// Index into the recipe's retouch operations.
    pub(in crate::app) selected: Option<usize>,
    /// Hide the pins (H).
    pub(in crate::app) hide_pins: bool,
    /// Visualize Spots (A) and its threshold.
    pub(in crate::app) visualize: bool,
    pub(in crate::app) threshold: f32,
    drag: Drag,
    /// Sources already offered for the selected spot, so `/` moves on to new ones.
    tried: Vec<[f32; 2]>,
}
impl Default for RetouchTool {
    fn default() -> Self {
        Self {
            mode: RetouchMode::Heal,
            size: 0.012,
            feather: 0.5,
            opacity: 1.,
            selected: None,
            hide_pins: false,
            visualize: false,
            threshold: 0.5,
            drag: Drag::None,
            tried: Vec::new(),
        }
    }
}
#[derive(Default)]
enum Drag {
    #[default]
    None,
    /// Moving a spot; image-space pointer position at the start and the spot then.
    Dest([f32; 2], RetouchOp),
    Source([f32; 2], RetouchOp),
    /// Painting a brushed area (image-space dab centres).
    Stroke(Vec<[f32; 2]>),
    /// A spot whose source follows the pointer (Cmd/Ctrl-drag).
    Manual,
}
impl RetouchTool {
    /// Whether a spot, its source or a brush stroke is being dragged.
    pub(in crate::app) fn is_dragging(&self) -> bool {
        !matches!(self.drag, Drag::None)
    }
    pub(in crate::app) fn clear_document(&mut self) {
        self.selected = None;
        self.drag = Drag::None;
        self.tried.clear();
    }
    fn select(&mut self, i: Option<usize>) {
        if self.selected != i {
            self.tried.clear();
        }
        self.selected = i;
    }
}
/// What the pointer is over.
#[derive(Clone, Copy, PartialEq)]
enum Hit {
    Dest(usize),
    Source(usize),
}

impl Editor {
    /// Handles the pointer on the photo and draws the spots. The tool owns the pointer.
    pub(in crate::app) fn retouch_overlay(
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
        let radius_on_screen = |p: [f32; 2], r: f32| {
            let [rx, ry] = map.view_radius(p, r);
            Vec2::new(rx * rect.width(), ry * rect.height())
        };
        let source_of = |op: &RetouchOp, p: [f32; 2]| [p[0] + op.offset[0], p[1] + op.offset[1]];
        let ops = self.document.edit.recipe().retouch.clone();
        let selected = self.view.retouch.selected.filter(|i| *i < ops.len());
        let hit = |pos: Pos2| -> Option<Hit> {
            // The selected spot's source first, then pins and shapes, newest on top.
            if let Some(i) = selected {
                let op = &ops[i];
                if covers(op, pos, |p| to_screen(source_of(op, p)), &radius_on_screen) {
                    return Some(Hit::Source(i));
                }
            }
            (0..ops.len()).rev().find_map(|i| {
                let op = &ops[i];
                let pin = to_screen(op.pin());
                (pin.distance(pos) < 9. || covers(op, pos, to_screen, &radius_on_screen))
                    .then_some(Hit::Dest(i))
            })
        };
        let pointer = response.hover_pos();
        let hovered = pointer.and_then(hit);
        let command = ui.input(|i| i.modifiers.command);

        // Pointer input.
        if response.drag_started()
            && let Some(origin) = ui.input(|i| i.pointer.press_origin())
        {
            let at = to_image(origin);
            self.view.retouch.drag = match hit(origin) {
                _ if command => {
                    self.add_spot(at, [0., 0.]);
                    Drag::Manual
                }
                Some(Hit::Dest(i)) => {
                    self.view.retouch.select(Some(i));
                    Drag::Dest(at, ops[i].clone())
                }
                Some(Hit::Source(i)) => Drag::Source(at, ops[i].clone()),
                None => Drag::Stroke(vec![at]),
            };
        }
        if response.dragged()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let at = to_image(pos);
            let selected = self.view.retouch.selected;
            let spacing = self.view.retouch.size * 0.25;
            let aspect = map.frame().aspect();
            let moved = match &mut self.view.retouch.drag {
                Drag::Dest(start, original) => {
                    let mut op = original.clone();
                    op.translate([at[0] - start[0], at[1] - start[1]]);
                    Some(op)
                }
                Drag::Source(start, original) => {
                    let mut op = original.clone();
                    op.offset[0] += at[0] - start[0];
                    op.offset[1] += at[1] - start[1];
                    Some(op)
                }
                Drag::Stroke(points) => {
                    let last = points[points.len() - 1];
                    // Spacing in long-edge units, whatever the photo's aspect.
                    let (dx, dy) = if aspect >= 1. {
                        (at[0] - last[0], (at[1] - last[1]) / aspect)
                    } else {
                        ((at[0] - last[0]) * aspect, at[1] - last[1])
                    };
                    if dx.hypot(dy) >= spacing && points.len() < crate::model::retouch::MAX_POINTS {
                        points.push(at);
                    }
                    None
                }
                Drag::Manual => selected
                    .and_then(|i| self.document.edit.recipe().retouch.get(i))
                    .map(|op| {
                        let pin = op.pin();
                        RetouchOp {
                            offset: [at[0] - pin[0], at[1] - pin[1]],
                            ..op.clone()
                        }
                    }),
                Drag::None => None,
            };
            if let (Some(op), Some(i)) = (moved, selected)
                && i < self.document.edit.recipe().retouch.len()
            {
                self.document.edit.recipe_mut().retouch[i] = op;
            }
        }
        if response.drag_stopped() {
            if let Drag::Stroke(points) = std::mem::take(&mut self.view.retouch.drag) {
                self.add_brush(points);
            }
            self.view.retouch.drag = Drag::None;
        }
        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            match hit(pos) {
                Some(Hit::Dest(i) | Hit::Source(i)) => self.view.retouch.select(Some(i)),
                None => self.add_spot(to_image(pos), [f32::NAN; 2]),
            }
        }

        // Drawing.
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        let tool = &self.view.retouch;
        for (i, op) in self.document.edit.recipe().retouch.iter().enumerate() {
            let selected = tool.selected == Some(i);
            if selected || hovered == Some(Hit::Dest(i)) {
                draw_shape(&painter, op, to_screen, &radius_on_screen, true);
            }
            if selected {
                draw_shape(
                    &painter,
                    op,
                    |p| to_screen(source_of(op, p)),
                    &radius_on_screen,
                    false,
                );
                let (from, to) = (to_screen(source_of(op, op.pin())), to_screen(op.pin()));
                overlay::path(&painter, vec![from, to], Color32::from_white_alpha(200));
            }
            if !tool.hide_pins {
                overlay::pin(&painter, to_screen(op.pin()), selected);
            }
        }
        if let Drag::Stroke(points) = &tool.drag {
            let screen: Vec<Pos2> = points.iter().map(|p| to_screen(*p)).collect();
            let r = radius_on_screen(points[0], tool.size);
            overlay::stroke_outline(&painter, &screen, r.x.max(r.y), 1.5);
        }
        if let Some(pos) = pointer
            && hovered.is_none()
            && rect.contains(pos)
        {
            let r = radius_on_screen(to_image(pos), tool.size);
            overlay::brush_cursor(&painter, pos, r.x.max(r.y), tool.feather);
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        } else if hovered.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
        }
    }
    /// A new spot at image position `at` with the tool's settings. A NaN offset asks
    /// for an automatic source.
    fn add_spot(&mut self, at: [f32; 2], offset: [f32; 2]) {
        let t = &self.view.retouch;
        let op = RetouchOp {
            mode: t.mode,
            shape: RetouchShape::Spot {
                center: at,
                radius: t.size,
            },
            feather: t.feather,
            opacity: t.opacity,
            offset,
        };
        self.push_op(op);
    }
    fn add_brush(&mut self, points: Vec<[f32; 2]>) {
        let t = &self.view.retouch;
        let op = RetouchOp {
            mode: t.mode,
            shape: RetouchShape::Brush {
                points: points.into(),
                radius: t.size,
            },
            feather: t.feather,
            opacity: t.opacity,
            offset: [f32::NAN; 2],
        };
        self.push_op(op);
    }
    fn push_op(&mut self, mut op: RetouchOp) {
        if self.document.edit.recipe().retouch.len() >= crate::model::retouch::MAX_OPS {
            self.messages.warn("Too many spots on this photo");
            return;
        }
        if op.offset[0].is_nan() {
            op.offset = self
                .automatic_source(&op, &[])
                .unwrap_or([op.radius() * 3., 0.]);
        }
        self.document.edit.recipe_mut().add_retouch(op);
        let i = self.document.edit.recipe().retouch.len() - 1;
        self.view.retouch.select(Some(i));
    }
    fn automatic_source(&self, op: &RetouchOp, avoid: &[[f32; 2]]) -> Option<[f32; 2]> {
        let im = self.document.full()?;
        retouch::find_source(im, op, &self.document.edit.recipe().retouch, avoid)
    }
    /// The Remove tool's keys: `[` `]` size, with Shift feather, `/` next source,
    /// Delete, H pins and A Visualize Spots.
    pub(in crate::app) fn retouch_keys(&mut self, i: &egui::InputState) {
        use egui::Key;
        let plain = !i.modifiers.command && !i.modifiers.alt;
        if !plain {
            return;
        }
        let pressed = |keys: &[Key]| keys.iter().any(|k| i.key_pressed(*k));
        if pressed(&[Key::OpenBracket, Key::OpenCurlyBracket]) {
            self.step_retouch_size(false, i.modifiers.shift);
        }
        if pressed(&[Key::CloseBracket, Key::CloseCurlyBracket]) {
            self.step_retouch_size(true, i.modifiers.shift);
        }
        if pressed(&[Key::Slash, Key::Questionmark]) {
            self.next_source();
        }
        if pressed(&[Key::Delete, Key::Backspace]) {
            self.delete_spot();
        }
        if pressed(&[Key::H]) && !i.modifiers.shift {
            self.view.retouch.hide_pins ^= true;
        }
        // With the modifiers held for the press: a quick Cmd+A, Select All, can
        // release Cmd within the frame.
        let a = i.events.iter().any(|event| {
            matches!(event, egui::Event::Key { key: Key::A, pressed: true, modifiers, .. }
                if !modifiers.any())
        });
        if a {
            self.view.retouch.visualize ^= true;
        }
    }
    /// `/`: the next best source for the selected spot.
    pub(in crate::app) fn next_source(&mut self) {
        let Some(i) = self.view.retouch.selected else {
            return;
        };
        let Some(op) = self.document.edit.recipe().retouch.get(i).cloned() else {
            return;
        };
        let tool = &mut self.view.retouch;
        if !tool.tried.contains(&op.offset) {
            tool.tried.push(op.offset);
        }
        let avoid = self.view.retouch.tried.clone();
        if let Some(offset) = self.automatic_source(&op, &avoid) {
            self.document.edit.recipe_mut().retouch[i].offset = offset;
        }
    }
    pub(in crate::app) fn delete_spot(&mut self) {
        if let Some(i) = self.view.retouch.selected.take()
            && i < self.document.edit.recipe().retouch.len()
        {
            self.document.edit.recipe_mut().retouch.remove(i);
        }
    }
    /// `[` and `]`: brush size, or the selected spot's; with Shift, feather.
    pub(in crate::app) fn step_retouch_size(&mut self, grow: bool, feather: bool) {
        let tool = &mut self.view.retouch;
        let op = tool
            .selected
            .and_then(|i| self.document.edit.recipe_mut().retouch.get_mut(i));
        if feather {
            let step = if grow { 0.1 } else { -0.1 };
            tool.feather = (tool.feather + step).clamp(0., 1.);
            if let Some(op) = op {
                op.feather = (op.feather + step).clamp(0., 1.);
            }
        } else {
            let k = if grow { 1.15 } else { 1. / 1.15 };
            tool.size = (tool.size * k).clamp(0.001, 0.25);
            if let Some(op) = op {
                op.set_radius((op.radius() * k).clamp(0.001, 0.25));
            }
        }
    }
    /// The Remove panel's drawer below the tool strip.
    pub(in crate::app) fn retouch_panel(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(ui.ctx());
        let long = self.document.full().map_or(1000., |im| {
            crate::model::image_frame::ImageFrame::new(im).long_edge()
        });
        let selected = self
            .view
            .retouch
            .selected
            .filter(|i| *i < self.document.edit.recipe().retouch.len());
        crate::app::ui::widgets::set_edit_context(ui, "Spot");
        control_label(ui, "Mode", |ui| {
            let mut mode = selected.map_or(self.view.retouch.mode, |i| {
                self.document.edit.recipe().retouch[i].mode
            });
            let w = ui.available_width();
            if segmented(
                ui,
                &mut mode,
                &[(RetouchMode::Heal, "Heal"), (RetouchMode::Clone, "Clone")],
                w,
            ) {
                self.view.retouch.mode = mode;
                if let Some(i) = selected {
                    self.document.edit.recipe_mut().retouch[i].mode = mode;
                }
            }
        });
        ui.add_space(space::XS);
        let (mut size, mut feather, mut opacity) = match selected {
            Some(i) => {
                let op = &self.document.edit.recipe().retouch[i];
                (op.radius(), op.feather, op.opacity)
            }
            None => {
                let t = &self.view.retouch;
                (t.size, t.feather, t.opacity)
            }
        };
        slider_with(
            ui,
            &mut self.step_name,
            "Size",
            &mut size,
            0.001..=0.25,
            0.012,
            Some((long, 0)),
            None,
        );
        slider_with(
            ui,
            &mut self.step_name,
            "Feather",
            &mut feather,
            0. ..=1.,
            0.5,
            Some((100., 0)),
            None,
        );
        slider_with(
            ui,
            &mut self.step_name,
            "Opacity",
            &mut opacity,
            0. ..=1.,
            1.,
            Some((100., 0)),
            None,
        );
        let t = &mut self.view.retouch;
        (t.size, t.feather, t.opacity) = (size, feather, opacity);
        if let Some(i) = selected {
            let op = &mut self.document.edit.recipe_mut().retouch[i];
            op.set_radius(size);
            (op.feather, op.opacity) = (feather, opacity);
        }
        ui.add_space(space::S);
        indented(ui, |ui| {
            ui.checkbox(&mut self.view.retouch.visualize, "Visualize Spots")
                .on_hover_text("A: show fine detail in black and white, so dust stands out");
        });
        if self.view.retouch.visualize {
            slider_with(
                ui,
                &mut self.step_name,
                "Threshold",
                &mut self.view.retouch.threshold,
                0. ..=1.,
                0.5,
                Some((100., 0)),
                None,
            );
        }
        indented(ui, |ui| {
            ui.checkbox(&mut self.view.retouch.hide_pins, "Hide pins")
                .on_hover_text("H");
        });
        ui.add_space(space::XS);
        hint(
            ui,
            "Click a spot to remove it · drag to paint · ⌘/Ctrl-drag to pick the source",
        );
        hint(
            ui,
            "[ ] size · ⇧[ ] feather · / new source · Delete removes",
        );
        ui.add_space(space::XS);
        indented(ui, |ui| {
            let w = (ui.available_width() - 4.) / 2.;
            let any = !self.document.edit.recipe().retouch.is_empty();
            if ui
                .add_enabled(any, egui::Button::new("Reset").min_size(Vec2::new(w, 22.)))
                .on_hover_text("Remove every spot")
                .clicked()
            {
                self.document.edit.recipe_mut().retouch.clear();
                self.view.retouch.select(None);
            }
            if ui
                .add_sized(
                    [w, 22.],
                    egui::Button::new(egui::RichText::new("Close").color(palette.on_accent()))
                        .fill(palette.accent()),
                )
                .on_hover_text("Close the tool · Q")
                .clicked()
            {
                self.view.tool = crate::app::develop::state::Tool::None;
            }
        });
    }
}
/// Whether `pos` lies inside the shape of `op` drawn through `place`.
fn covers(
    op: &RetouchOp,
    pos: Pos2,
    place: impl Fn([f32; 2]) -> Pos2,
    radius: &impl Fn([f32; 2], f32) -> Vec2,
) -> bool {
    let near = |p: [f32; 2]| {
        let r = radius(p, op.radius());
        let d = pos - place(p);
        (d.x / r.x.max(1.)).powi(2) + (d.y / r.y.max(1.)).powi(2) <= 1.
    };
    match &op.shape {
        RetouchShape::Spot { center, .. } => near(*center),
        RetouchShape::Brush { points, .. } => points.iter().any(|p| near(*p)),
    }
}
fn draw_shape(
    painter: &egui::Painter,
    op: &RetouchOp,
    place: impl Fn([f32; 2]) -> Pos2,
    radius: &impl Fn([f32; 2], f32) -> Vec2,
    dest: bool,
) {
    let width = if dest { 1.5 } else { 1. };
    match &op.shape {
        RetouchShape::Spot { center, .. } => {
            overlay::ellipse(
                painter,
                place(*center),
                radius(*center, op.radius()),
                0.,
                width,
            );
        }
        RetouchShape::Brush { points, .. } => {
            let screen: Vec<Pos2> = points.iter().map(|p| place(*p)).collect();
            let r = radius(points[0], op.radius());
            overlay::stroke_outline(painter, &screen, r.x.max(r.y), width);
        }
    }
}
/// A row with a right-aligned label in the sliders' label column.
pub(in crate::app) fn control_label(
    ui: &mut egui::Ui,
    label: &str,
    add: impl FnOnce(&mut egui::Ui),
) {
    ui.horizontal(|ui| {
        // To the label column, less the gap the row puts after it.
        let width = crate::app::ui::widgets::label_column(ui) - ui.spacing().item_spacing.x;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 22.), egui::Sense::hover());
        ui.painter().text(
            rect.right_center(),
            egui::Align2::RIGHT_CENTER,
            label,
            egui::FontId::proportional(text::SMALL),
            theme::palette(ui.ctx()).tone(Tone::SOFT),
        );
        add(ui);
    });
}
/// A row starting where slider rails start.
pub(in crate::app) fn indented(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_space(crate::app::ui::widgets::label_column(ui));
        add(ui);
    });
}
pub(in crate::app) fn hint(ui: &mut egui::Ui, text: &str) {
    indented(ui, |ui| {
        ui.add(
            egui::Label::new(
                egui::RichText::new(text)
                    .size(text::MICRO)
                    .color(theme::palette(ui.ctx()).tone(Tone::FAINT)),
            )
            .wrap(),
        );
    });
}

#[cfg(test)]
#[path = "retouch_tool_editor_tests.rs"]
mod editor_tests;
