//! Spot removal (Lightroom's Remove panel, Heal and Clone modes): tool settings, the
//! selected spot, pointer handling on the photo and the panel drawer.
//!
//! Click adds a spot with an automatically chosen source; a drag paints a brushed
//! area, whose source is chosen when the stroke ends; Cmd/Ctrl-drag places a spot and
//! drags its source by hand. Dragging a spot's pin or circle moves it, dragging its
//! source circle moves the source.
//!
//! The experimental AI Retouch tool ([`super::ai_remove`]) shares this one:
//! there a click or a painted area is sent to an extension's retouch
//! generator instead, and the spot is added once its patch comes back. Each
//! tool shows, selects and resets only its own spots. A Generate spot is fixed
//! where its patch was made: it can be selected, its Feather and Opacity
//! changed, hidden and deleted, but not moved, resized or given a source, and
//! it never becomes a Heal or Clone spot, nor one of those a Generate spot.
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
    /// The retouch generator new AI spots go to, by provenance
    /// (`<extension>/<generator>`); the first one offered when none or gone.
    pub(in crate::app) generator: Option<String>,
    /// What the user wrote for each generator that takes text, by provenance.
    pub(in crate::app) prompts: std::collections::HashMap<String, String>,
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
            generator: None,
            prompts: Default::default(),
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
    /// Starts moving `op` from `at`, as a drag on its pin does, for tests.
    #[cfg(test)]
    pub(in crate::app) fn drag_spot_for_tests(&mut self, at: [f32; 2], op: RetouchOp) {
        self.drag = Drag::Dest(at, op);
    }
    /// Forgets a drag of a spot or its source under way, which writes into
    /// the selected spot: its next frames change nothing. A stroke being
    /// painted, which edits no spot until it ends, goes on.
    pub(in crate::app) fn end_spot_drag(&mut self) {
        if !matches!(self.drag, Drag::Stroke(_)) {
            self.drag = Drag::None;
        }
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
        self.fit_retouch_selection();
        // Spot Removal's spots are Heal and Clone ones, AI Retouch's the
        // generated ones; each tool shows and takes only its own.
        let generating = self.ai_retouching();
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
            // A Generate spot has no source to drag.
            if let Some(i) = selected.filter(|i| !ops[*i].is_generated()) {
                let op = &ops[i];
                if covers(op, pos, |p| to_screen(source_of(op, p)), &radius_on_screen) {
                    return Some(Hit::Source(i));
                }
            }
            (0..ops.len()).rev().find_map(|i| {
                let op = &ops[i];
                if op.is_generated() != generating {
                    return None;
                }
                let pin = to_screen(op.pin());
                (pin.distance(pos) < 9. || covers(op, pos, to_screen, &radius_on_screen))
                    .then_some(Hit::Dest(i))
            })
        };
        let pointer = response.hover_pos();
        let hovered = pointer.and_then(hit);
        let command = ui.input(|i| i.modifiers.command);

        // A stroke or a click on the photo takes the keyboard from a text
        // field (AI Retouch's), as a click elsewhere does: egui keeps it
        // through a drag, and Cmd/Ctrl+Z after painting would undo the text,
        // not the spot.
        if response.drag_started() || response.clicked() {
            ui.memory_mut(|m| {
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
        }
        // Pointer input.
        if response.drag_started()
            && let Some(origin) = ui.input(|i| i.pointer.press_origin())
        {
            let at = to_image(origin);
            self.view.retouch.drag = match hit(origin) {
                _ if command && !generating => {
                    self.add_spot(at, [0., 0.]);
                    Drag::Manual
                }
                // Fixed where its patch was made: selected, never moved.
                Some(Hit::Dest(i)) if ops[i].is_generated() => {
                    self.view.retouch.select(Some(i));
                    Drag::None
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
                // AI Retouch gathers strokes; sending is the user's.
                if generating {
                    self.add_ai_stroke(points);
                } else {
                    self.add_brush(points);
                }
            }
            self.view.retouch.drag = Drag::None;
        }
        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            match hit(pos) {
                Some(Hit::Dest(i) | Hit::Source(i)) => self.view.retouch.select(Some(i)),
                // A click in AI Retouch is a stroke of one dab.
                None if generating => self.add_ai_stroke(vec![to_image(pos)]),
                None => self.add_spot(to_image(pos), [f32::NAN; 2]),
            }
        }

        // Drawing.
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        let tool = &self.view.retouch;
        for (i, op) in self.document.edit.recipe().retouch.iter().enumerate() {
            if op.is_generated() != generating {
                continue;
            }
            let selected = tool.selected == Some(i);
            if selected || hovered == Some(Hit::Dest(i)) {
                draw_shape(&painter, op, to_screen, &radius_on_screen, true);
            }
            if selected && !op.is_generated() {
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
        // The area waiting for the upload question, with the question beside
        // it, where the user just painted: the drawer alone is easy to miss.
        if let Some(consent) = self.ai_remove.consent.clone() {
            draw_shape(&painter, &consent.op, to_screen, &radius_on_screen, true);
            let visible = rect.intersect(ui.clip_rect());
            let question = format!(
                "Send this area and its surroundings to the service set up in {}? Asked once.",
                consent.offer.extension_name
            );
            let (answer, _) = consent_card(ui, to_screen(consent.op.pin()), visible, &question);
            if let Some(allow) = answer {
                self.answer_ai_remove_consent(allow);
            }
        }
        // The area painted and not sent yet, at the brush's size now; Enter
        // sends it, unless a text field has the keyboard.
        if generating && !self.ai_remove.draft.is_empty() {
            // Its coverage as the request will send it, the feather fading it.
            if let Some(tint) = self.draft_tint(ui.ctx(), &im, &to_screen) {
                painter.add(tint);
            }
            let size = self.view.retouch.size;
            for stroke in &self.ai_remove.draft {
                let screen: Vec<Pos2> = stroke.iter().map(|p| to_screen(*p)).collect();
                let r = radius_on_screen(stroke[0], size);
                overlay::stroke_outline(&painter, &screen, r.x.max(r.y), 1.5);
            }
            let typing = ui.memory(|m| m.focused().is_some());
            if !typing && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.send_ai_draft();
            }
        }
        // The area a request failed for, with why and what to do: the photo
        // says so where the user painted, not only the drawer.
        if generating
            && self.ai_remove.running().is_none()
            && self.ai_remove.consent.is_none()
            && let (Some(failure), Some((failed, op))) = (
                self.ai_remove.failure.clone(),
                self.ai_remove.failed.clone(),
            )
        {
            draw_shape(&painter, &op, to_screen, &radius_on_screen, true);
            let visible = rect.intersect(ui.clip_rect());
            let setup = matches!(failure, super::ai_remove::Failure::NeedsSetup(_));
            let buttons: &[&str] = if setup {
                &["Set up…", "Dismiss"]
            } else {
                &["Dismiss"]
            };
            let (clicked, _) = card(
                ui,
                to_screen(op.pin()),
                visible,
                &failure.message(&failed.name),
                buttons,
            );
            match clicked {
                // The failed request's extension, whatever is chosen now.
                Some(0) if setup => self.open_extension_page(&failed.extension),
                Some(_) => self.dismiss_ai_failure(),
                None => {}
            }
        }
        // The area being removed, with its card: what it waits for, and Cancel.
        if let Some(op) = self.ai_remove.running().cloned() {
            draw_shape(&painter, &op, to_screen, &radius_on_screen, true);
            let at = to_screen(op.pin()) + Vec2::new(14., -10.);
            let card = Rect::from_min_size(at, Vec2::new(150., 26.)).intersect(rect);
            painter.rect_filled(card, 4., ui.visuals().window_fill);
            let mut cancel = false;
            ui.scope_builder(egui::UiBuilder::new().max_rect(card.shrink(3.)), |ui| {
                ui.horizontal_centered(|ui| {
                    crate::app::ui::widgets::spinner(ui, 12.);
                    let working = self.ai_remove.working().unwrap_or_default();
                    ui.label(egui::RichText::new(working).size(text::SMALL));
                    cancel = ui.small_button("Cancel").clicked();
                });
            });
            if cancel {
                self.cancel_ai_remove();
            }
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
    /// A tinted quad of the draft's coverage, feather and all, as
    /// [`rendition::coverage`] makes it for the request, over the draft's
    /// bounds at about the screen's resolution. The texture is made again
    /// only when the draft, the brush or that resolution changes.
    ///
    /// [`rendition::coverage`]: crate::develop::retouch::rendition::coverage
    fn draft_tint(
        &mut self,
        ctx: &egui::Context,
        im: &crate::camera_data::CameraImage,
        to_screen: &impl Fn([f32; 2]) -> Pos2,
    ) -> Option<egui::Shape> {
        use std::hash::{Hash, Hasher};
        let t = &self.view.retouch;
        let op = RetouchOp {
            mode: RetouchMode::Generate,
            shape: RetouchShape::Strokes {
                strokes: self
                    .ai_remove
                    .draft
                    .iter()
                    .map(|s| std::sync::Arc::from(s.as_slice()))
                    .collect(),
                radius: t.size,
            },
            feather: t.feather,
            opacity: 1.,
            offset: [0.; 2],
            patch: None,
        };
        let frame = crate::model::image_frame::ImageFrame::new(im);
        let (rx, ry) = crate::model::retouch::radii(t.size, frame.aspect());
        let (dabs, _) = op.shape.dabs();
        let mut b = [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ];
        for p in &dabs {
            b = [
                b[0].min(p[0] - rx),
                b[1].min(p[1] - ry),
                b[2].max(p[0] + rx),
                b[3].max(p[1] + ry),
            ];
        }
        let rect = [b[0].max(0.), b[1].max(0.), b[2].min(1.), b[3].min(1.)];
        if rect[0] >= rect[2] || rect[1] >= rect[3] {
            return None;
        }
        let corners = [
            to_screen([rect[0], rect[1]]),
            to_screen([rect[2], rect[1]]),
            to_screen([rect[2], rect[3]]),
            to_screen([rect[0], rect[3]]),
        ];
        // About a texel per screen point, at most 512 a side.
        let width = (corners[0].distance(corners[1]).ceil() as u32).clamp(1, 512);
        let height = (corners[0].distance(corners[3]).ceil() as u32).clamp(1, 512);
        let [fw, _] = frame.size();
        let region = crate::develop::retouch::rendition::Region {
            rect,
            width,
            height,
            scale: width as f32 / ((rect[2] - rect[0]) * fw),
        };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for p in &dabs {
            (p[0].to_bits(), p[1].to_bits()).hash(&mut h);
        }
        self.ai_remove.draft.len().hash(&mut h);
        (t.size.to_bits(), t.feather.to_bits(), width, height).hash(&mut h);
        let key = h.finish();
        if self
            .ai_remove
            .draft_tint
            .as_ref()
            .is_none_or(|(k, _)| *k != key)
        {
            let coverage = crate::develop::retouch::rendition::coverage(&op, &frame, &region);
            let pixels = coverage
                .iter()
                .map(|c| Color32::from_rgba_unmultiplied(255, 70, 70, (*c as f32 * 0.45) as u8))
                .collect();
            let image = egui::ColorImage::new([width as usize, height as usize], pixels);
            let texture =
                ctx.load_texture("ai-draft-coverage", image, egui::TextureOptions::LINEAR);
            self.ai_remove.draft_tint = Some((key, texture));
        }
        let texture = self.ai_remove.draft_tint.as_ref()?.1.id();
        let mut mesh = egui::Mesh::with_texture(texture);
        for (corner, uv) in corners.iter().zip([[0., 0.], [1., 0.], [1., 1.], [0., 1.]]) {
            mesh.vertices.push(egui::epaint::Vertex {
                pos: *corner,
                uv: Pos2::new(uv[0], uv[1]),
                color: Color32::WHITE,
            });
        }
        mesh.indices.extend([0, 1, 2, 0, 2, 3]);
        Some(egui::Shape::mesh(mesh))
    }
    /// A spot at image position `at` with the tool's settings.
    fn spot_op(&self, at: [f32; 2], offset: [f32; 2]) -> RetouchOp {
        let t = &self.view.retouch;
        RetouchOp {
            mode: t.mode,
            shape: RetouchShape::Spot {
                center: at,
                radius: t.size,
            },
            feather: t.feather,
            opacity: t.opacity,
            offset,
            patch: None,
        }
    }
    /// A brushed area along `points` with the tool's settings.
    fn brush_op(&self, points: Vec<[f32; 2]>) -> RetouchOp {
        let t = &self.view.retouch;
        RetouchOp {
            mode: t.mode,
            shape: RetouchShape::Brush {
                points: points.into(),
                radius: t.size,
            },
            feather: t.feather,
            opacity: t.opacity,
            offset: [f32::NAN; 2],
            patch: None,
        }
    }
    /// A new spot at image position `at` with the tool's settings. A NaN offset asks
    /// for an automatic source.
    fn add_spot(&mut self, at: [f32; 2], offset: [f32; 2]) {
        self.push_op(self.spot_op(at, offset));
    }
    fn add_brush(&mut self, points: Vec<[f32; 2]>) {
        self.push_op(self.brush_op(points));
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
    pub(in crate::app) fn retouch_keys(&mut self, ctx: &egui::Context) {
        use crate::app::shell::commands::registry::bindings as b;
        let keys = crate::app::shell::keymap::FrameKeys::of(ctx);
        // Each key with the modifiers held for it; Shift feathers.
        if let Some(m) = keys.take(&b::REMOVE_SMALLER) {
            self.step_retouch_size(false, m.shift);
        }
        if let Some(m) = keys.take(&b::REMOVE_LARGER) {
            self.step_retouch_size(true, m.shift);
        }
        if keys.take(&b::REMOVE_NEW_SOURCE).is_some() {
            self.next_source();
        }
        if keys.take(&b::REMOVE_DELETE).is_some() {
            self.delete_spot();
        }
        if keys.take(&b::REMOVE_HIDE_PINS).is_some() {
            self.view.retouch.hide_pins ^= true;
        }
        if keys.take(&b::REMOVE_VISUALIZE).is_some() {
            self.view.retouch.visualize ^= true;
        }
    }
    /// Mode chosen in Spot Removal's drawer: new spots' mode, and the selected
    /// spot's, which Heal and Clone trade places on. A Generate spot, AI
    /// Retouch's, never becomes Heal or Clone.
    pub(in crate::app) fn choose_retouch_mode(&mut self, mode: RetouchMode) {
        if mode == RetouchMode::Generate {
            return;
        }
        self.view.retouch.mode = mode;
        let selected = self
            .view
            .retouch
            .selected
            .filter(|i| *i < self.document.edit.recipe().retouch.len());
        if let Some(i) = selected
            && !self.document.edit.recipe().retouch[i].is_generated()
        {
            self.document.edit.recipe_mut().retouch[i].mode = mode;
        }
    }
    /// Whether AI Retouch is the open tool: new spots go to a generator, and
    /// only generated spots are shown and taken.
    pub(in crate::app) fn ai_retouching(&self) -> bool {
        self.view.is(crate::app::develop::state::Tool::AiRetouch)
    }
    /// Whether spot `i` belongs to the open spot tool: a generated one to AI
    /// Retouch, a Heal or Clone one to Spot Removal.
    pub(in crate::app) fn spot_in_tool(&self, i: usize) -> bool {
        self.document
            .edit
            .recipe()
            .retouch
            .get(i)
            .is_some_and(|op| op.is_generated() == self.ai_retouching())
    }
    /// Lets go of a selected spot the open tool doesn't show, so its keys and
    /// sliders never reach the other tool's spots. Every change to the
    /// selected spot checks it first, as a tool opened by a key in the same
    /// frame hasn't drawn yet.
    ///
    /// Only a spot that is there and the other tool's: an index past the end
    /// (the list shortened under it) is left for its users' bounds checks, as
    /// it was, so whether it is let go of never hangs on when the photo drew.
    pub(in crate::app) fn fit_retouch_selection(&mut self) {
        let ops = &self.document.edit.recipe().retouch;
        if let Some(i) = self.view.retouch.selected
            && i < ops.len()
            && !self.spot_in_tool(i)
        {
            self.view.retouch.select(None);
        }
    }
    /// Reset: removes the open tool's spots, keeping the other's.
    pub(in crate::app) fn reset_tool_spots(&mut self) {
        let generating = self.ai_retouching();
        let ops = &self.document.edit.recipe().retouch;
        if ops.iter().any(|op| op.is_generated() == generating) {
            self.document
                .edit
                .recipe_mut()
                .retouch
                .retain(|op| op.is_generated() != generating);
        }
        self.view.retouch.select(None);
    }
    /// The drawer tools the rail offers now: AI Retouch only while an enabled
    /// extension offers a retouch generator.
    pub(in crate::app) fn rail_tools(&self) -> Vec<crate::app::develop::state::Tool> {
        use crate::app::develop::state::Tool;
        let ai = !self.ai_remove_offers().is_empty();
        crate::app::develop::panels::inspector::DRAWER_TOOLS
            .iter()
            .map(|(t, ..)| *t)
            .filter(|t| *t != Tool::AiRetouch || ai)
            .collect()
    }
    /// Closes AI Retouch once no generator is offered (its extension disabled
    /// or removed): its drawer would have nothing to run.
    pub(in crate::app) fn close_tools_gone(&mut self) {
        use crate::app::develop::state::Tool;
        if self.view.is(Tool::AiRetouch) && self.ai_remove_offers().is_empty() {
            self.view.set_tool(Tool::None);
        }
    }
    /// `/`: the next best source for the selected spot.
    pub(in crate::app) fn next_source(&mut self) {
        self.fit_retouch_selection();
        let Some(i) = self.view.retouch.selected else {
            return;
        };
        let Some(op) = self.document.edit.recipe().retouch.get(i).cloned() else {
            return;
        };
        // A Generate spot's source is its patch.
        if op.is_generated() {
            return;
        }
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
        self.fit_retouch_selection();
        if let Some(i) = self.view.retouch.selected.take()
            && i < self.document.edit.recipe().retouch.len()
        {
            self.document.edit.recipe_mut().retouch.remove(i);
        }
    }
    /// `[` and `]`: brush size, or the selected spot's; with Shift, feather.
    pub(in crate::app) fn step_retouch_size(&mut self, grow: bool, feather: bool) {
        self.fit_retouch_selection();
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
            // A Generate spot keeps the size its patch was made for.
            if let Some(op) = op.filter(|op| !op.is_generated()) {
                op.set_radius((op.radius() * k).clamp(0.001, 0.25));
            }
        }
    }
    /// Spot Removal's and AI Retouch's drawer below the tool strip: the same
    /// brush and pins, each with its own modes and spots.
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
        let generating = self.ai_retouching();
        let selected = selected.filter(|i| self.spot_in_tool(*i));
        let selected_generated = generating && selected.is_some();
        if generating {
            let offers: Vec<_> = self
                .ai_remove_offers()
                .into_iter()
                .map(|(o, _)| o)
                .collect();
            let offer = self.ai_remove_offer();
            // Each generator is a mode of its own: AI Remove, AI Replace.
            if offers.len() > 1 {
                control_label(ui, "Mode", |ui| {
                    let mut pick = offer
                        .as_ref()
                        .and_then(|(o, _)| offers.iter().position(|x| x.provenance == o.provenance))
                        .unwrap_or(0);
                    let modes: Vec<(usize, &str)> = offers
                        .iter()
                        .enumerate()
                        .map(|(i, o)| (i, o.name.as_str()))
                        .collect();
                    let w = ui.available_width();
                    if segmented(ui, &mut pick, &modes, w) {
                        self.choose_ai_generator(&offers[pick]);
                    }
                });
            }
            self.ai_remove_section(ui, offer, selected);
        } else {
            control_label(ui, "Mode", |ui| {
                // What a stroke now makes, or the selected spot's mode, which
                // Mode changes.
                let mut mode = match selected {
                    Some(i) => self.document.edit.recipe().retouch[i].mode,
                    None => self.view.retouch.mode,
                };
                let w = ui.available_width();
                let modes = [(RetouchMode::Heal, "Heal"), (RetouchMode::Clone, "Clone")];
                if segmented(ui, &mut mode, &modes, w) {
                    self.choose_retouch_mode(mode);
                }
            });
            if self.ai_remove_offers().is_empty() {
                self.ai_remove_install_hint(ui);
            }
        }
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
        // A Generate spot keeps the size its patch was made for.
        ui.add_enabled_ui(!selected_generated, |ui| {
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
        });
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
        (t.feather, t.opacity) = (feather, opacity);
        // A Generate spot keeps the size its patch was made for.
        if !selected_generated {
            t.size = size;
        }
        if let Some(i) = selected {
            let op = &mut self.document.edit.recipe_mut().retouch[i];
            if !op.is_generated() {
                op.set_radius(size);
            }
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
        if generating {
            hint(ui, "Click a spot or drag to paint over an area");
            hint(ui, "[ ] size · ⇧[ ] feather · Delete removes");
        } else {
            hint(
                ui,
                "Click a spot to remove it · drag to paint · ⌘/Ctrl-drag to pick the source",
            );
            hint(
                ui,
                "[ ] size · ⇧[ ] feather · / new source · Delete removes",
            );
        }
        ui.add_space(space::XS);
        indented(ui, |ui| {
            let w = (ui.available_width() - 4.) / 2.;
            let any = self
                .document
                .edit
                .recipe()
                .retouch
                .iter()
                .any(|op| op.is_generated() == generating);
            if ui
                .add_enabled(any, egui::Button::new("Reset").min_size(Vec2::new(w, 22.)))
                .on_hover_text("Remove every spot of this tool")
                .clicked()
            {
                self.reset_tool_spots();
            }
            if ui
                .add_sized(
                    [w, 22.],
                    egui::Button::new(egui::RichText::new("Close").color(palette.on_accent()))
                        .fill(palette.accent()),
                )
                .on_hover_text(if generating {
                    "Close the tool"
                } else {
                    "Close the tool · Q"
                })
                .clicked()
            {
                self.view.set_tool(crate::app::develop::state::Tool::None);
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
        RetouchShape::Strokes { strokes, .. } => {
            strokes.iter().flat_map(|s| s.iter()).any(|p| near(*p))
        }
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
        RetouchShape::Strokes { strokes, .. } => {
            for stroke in strokes.iter() {
                let screen: Vec<Pos2> = stroke.iter().map(|p| place(*p)).collect();
                let r = radius(stroke[0], op.radius());
                overlay::stroke_outline(painter, &screen, r.x.max(r.y), width);
            }
        }
    }
}
/// [`control_label`] for a box taller than a line: the label beside its
/// first line, not its middle.
pub(in crate::app) fn control_label_top(
    ui: &mut egui::Ui,
    label: &str,
    add: impl FnOnce(&mut egui::Ui),
) {
    ui.with_layout(egui::Layout::left_to_right(egui::Align::TOP), |ui| {
        // As tall as a text box's first line with its margin above and below.
        label_row(ui, label, 30., add);
    });
}
/// A row with a right-aligned label in the sliders' label column.
pub(in crate::app) fn control_label(
    ui: &mut egui::Ui,
    label: &str,
    add: impl FnOnce(&mut egui::Ui),
) {
    ui.horizontal(|ui| label_row(ui, label, 22., add));
}
/// A label `height` tall, then `add`, spaced as the sliders' rows are: the
/// caption ends [`LABEL_GAP`] short of the label column and the control
/// starts as far past it.
///
/// [`LABEL_GAP`]: crate::app::ui::widgets::LABEL_GAP
fn label_row(ui: &mut egui::Ui, label: &str, height: f32, add: impl FnOnce(&mut egui::Ui)) {
    use crate::app::ui::widgets::{LABEL_GAP, label_column};
    let spacing = ui.spacing().item_spacing.x;
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(label_column(ui) - LABEL_GAP, height),
        egui::Sense::hover(),
    );
    ui.painter().text(
        rect.right_center(),
        egui::Align2::RIGHT_CENTER,
        label,
        egui::FontId::proportional(text::SMALL),
        theme::palette(ui.ctx()).tone(Tone::SOFT),
    );
    ui.add_space((2. * LABEL_GAP - spacing).max(0.));
    add(ui);
}
/// [`hint`] starting `left` from the row's start, to line up with what it
/// is about.
pub(in crate::app) fn hint_at(ui: &mut egui::Ui, left: f32, text: &str) {
    ui.horizontal(|ui| {
        ui.add_space(left);
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

/// The upload question's card beside the area pinned at `pin` ([`card`]),
/// with Send and Cancel: the answer, if one was given, and where the
/// buttons are.
fn consent_card(
    ui: &mut egui::Ui,
    pin: Pos2,
    visible: Rect,
    question: &str,
) -> (Option<bool>, Rect) {
    let (clicked, buttons) = card(ui, pin, visible, question, &["Send", "Cancel"]);
    (clicked.map(|i| i == 0), buttons)
}

/// A card beside the area pinned at `pin` with `text` and a row of
/// `buttons`, kept inside `visible` and as tall as its wrapped text needs, so
/// the buttons stay in view however narrow the view is: which button was
/// clicked, if one was, and where the buttons are.
fn card(
    ui: &mut egui::Ui,
    pin: Pos2,
    visible: Rect,
    text: &str,
    buttons: &[&str],
) -> (Option<usize>, Rect) {
    const PAD: f32 = 6.;
    let width = 250f32.min(visible.width());
    let text = egui::RichText::new(text).size(text::SMALL);
    let galley = egui::WidgetText::from(text.clone()).into_galley(
        ui,
        Some(egui::TextWrapMode::Wrap),
        (width - 2. * PAD).max(1.),
        egui::TextStyle::Body,
    );
    let spacing = ui.spacing();
    let height = galley.size().y + spacing.item_spacing.y + spacing.interact_size.y + 2. * PAD;
    let rect = card_rect(pin, Vec2::new(width, height), visible);
    ui.painter()
        .with_clip_rect(visible)
        .rect_filled(rect, 4., ui.visuals().window_fill);
    let mut clicked = None;
    let mut row = Rect::NOTHING;
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink(PAD)), |ui| {
        ui.add(egui::Label::new(text).wrap());
        ui.horizontal(|ui| {
            for (i, label) in buttons.iter().enumerate() {
                let button = ui.small_button(*label);
                row = row.union(button.rect);
                if button.clicked() {
                    clicked = Some(i);
                }
            }
        });
    });
    (clicked, row)
}

/// Where a card `size` big about the area pinned at `pin` goes: beside it,
/// moved inside `visible` (the part of the photo in view, which a zoomed-in
/// area may be outside of) and shrunk to it if it is smaller.
fn card_rect(pin: Pos2, size: Vec2, visible: Rect) -> Rect {
    let size = size.min(visible.size());
    let at = pin + Vec2::new(14., -10.);
    let min = Pos2::new(
        at.x.min(visible.max.x - size.x).max(visible.min.x),
        at.y.min(visible.max.y - size.y).max(visible.min.y),
    );
    Rect::from_min_size(min, size)
}

#[cfg(test)]
#[path = "retouch_tool_editor_tests.rs"]
mod editor_tests;
