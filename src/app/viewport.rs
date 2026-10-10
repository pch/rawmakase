use super::Editor;
use super::before_after;
use super::crop_tool::{Attention, Guide, Ruler};
use super::icons::{self, Icon};
use super::navigator;
use super::state::{TextureMode, Tool};
use crate::app::Module;
use crate::app::theme;
use crate::develop::{self, Geometry};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};

impl Editor {
    /// Constrains the crop to the chosen aspect: negative keeps the photo's
    /// own ratio, zero is free, and presets follow the photo's orientation.
    pub(super) fn fit_aspect(&mut self) {
        if self.view.aspect == 0. {
            return;
        }
        if let Some(im) = self.document.full().cloned() {
            let mut r = self.document.edit.recipe().clone();
            r.crop = [0., 0., 1., 1.];
            let g = Geometry::new(&im, &r, 0);
            let portrait = g.oriented_height > g.oriented_width;
            let aspect = if self.view.aspect < 0. {
                g.oriented_width / g.oriented_height
            } else if portrait {
                1. / self.view.aspect
            } else {
                self.view.aspect
            };
            let ratio = aspect * g.oriented_height / g.oriented_width;
            let c = &mut self.document.edit.recipe_mut().crop;
            let cx = (c[0] + c[2]) / 2.;
            let cy = (c[1] + c[3]) / 2.;
            let mut w = c[2] - c[0];
            let mut h = c[3] - c[1];
            if w / h > ratio {
                w = h * ratio;
            } else {
                h = w / ratio;
            }
            let w = w.max(0.01);
            let h = h.max(0.01);
            *c = [cx - w / 2., cy - h / 2., cx + w / 2., cy + h / 2.];
        }
    }
    /// Opening the Crop tool shows the photo's own aspect, as Lightroom does,
    /// rather than the last photo's: Original when uncropped or at the photo's
    /// ratio, a preset it matches, or else its exact ratio as Custom.
    pub(super) fn read_aspect(&mut self) {
        if !self.view.is(Tool::Crop) || self.view.aspect_read {
            return;
        }
        let Some(im) = self.document.full().cloned() else {
            return;
        };
        self.view.aspect_read = true;
        let c = self.document.edit.recipe().crop;
        if c == [0., 0., 1., 1.] {
            self.view.aspect = -1.;
            return;
        }
        let mut r = self.document.edit.recipe().clone();
        r.crop = [0., 0., 1., 1.];
        let g = Geometry::new(&im, &r, 0);
        let photo = g.oriented_width / g.oriented_height;
        let crop = (c[2] - c[0]) / (c[3] - c[1]) * photo;
        // As fit_aspect reads it: presets are long over short for landscape photos.
        let aspect = if g.oriented_height > g.oriented_width {
            1. / crop
        } else {
            crop
        };
        let near = |a: f32, b: f32| (a / b - 1.).abs() < 0.005;
        // A preset either way round: its reciprocal is the other orientation (X).
        self.view.aspect = if near(crop, photo) {
            -1.
        } else {
            super::inspector::ASPECTS
                .iter()
                .map(|(a, _)| *a)
                .filter(|a| *a > 0.)
                .flat_map(|a| [a, 1. / a])
                .find(|a| near(aspect, *a))
                .unwrap_or(aspect)
        };
    }
    /// Lightroom's Navigator: the whole photo with the zoomed area outlined.
    /// Clicking or dragging in it moves the zoomed view there.
    pub(super) fn navigator_ui(&mut self, ui: &mut egui::Ui) {
        let shown = self
            .region()
            .zip(self.document.full())
            .map(|([x, y, w, h], im)| {
                let g = Geometry::new(im, &self.effective_recipe(), 0);
                let (gw, gh) = (g.width as f32, g.height as f32);
                [x as f32 / gw, y as f32 / gh, w as f32 / gw, h as f32 / gh]
            });
        // Until a whole render makes one (none while zoomed in), the
        // Library's preview stands in.
        let photo = self
            .preview
            .navigator
            .as_ref()
            .map(|p| (p.id(), p.size_vec2()))
            .or_else(|| {
                let texture = self
                    .library
                    .as_ref()?
                    .thumbnail(self.document.catalog_photo?)?;
                Some((texture.id(), texture.size_vec2()))
            });
        match navigator::navigator(ui, photo, Some(self.view.zoom), shown) {
            Some(navigator::Change::Level(level)) => self.set_zoom(level),
            Some(navigator::Change::Inspect(at)) => {
                self.view.zoom.pan = at;
                self.view.zoom.on = true;
                if self.view.is(Tool::Crop) {
                    self.view.tool = Tool::None;
                }
            }
            None => {}
        }
    }
    /// Before the first image arrives: the Library preview of the photo being
    /// opened with a spinner, or a hint when nothing is open.
    fn loading_placeholder(&self, ui: &mut egui::Ui, area: Rect) {
        let palette = theme::palette(ui.ctx());
        // A photo counts as opening until its first render arrives, even after
        // the decode has finished; only an empty document shows the hint.
        let opening = self.load.is_running()
            || self.document.catalog_photo.is_some()
            || self.document.path.is_some();
        if !opening {
            let empty = self
                .library
                .as_ref()
                .is_none_or(|l| l.session.photos.is_empty());
            ui.painter().text(
                area.center(),
                egui::Align2::CENTER_CENTER,
                if empty {
                    "Add photos in the Library to start editing"
                } else {
                    "Pick a photo in the Library to edit it"
                },
                egui::FontId::proportional(15.),
                palette.gray(120),
            );
            return;
        }
        // The stored Standard preview, else the Library's.
        let thumb = self
            .preview
            .stand_in
            .as_ref()
            .map(|p| (p.id(), p.size_vec2()))
            .or_else(|| {
                let t = self
                    .document
                    .catalog_photo
                    .and_then(|id| self.library.as_ref()?.thumbnail(id))?;
                Some((t.id(), t.size_vec2()))
            });
        if let Some((texture, size)) = thumb {
            let k = (area.width() / size.x).min(area.height() / size.y);
            ui.painter().image(
                texture,
                Rect::from_center_size(area.center(), size * k),
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
                Color32::WHITE,
            );
        }
        let badge = Rect::from_center_size(
            Pos2::new(area.center().x, area.bottom() - 36.),
            Vec2::new(170., 30.),
        );
        ui.painter()
            .rect_filled(badge, 15., Color32::from_black_alpha(170));
        let spinner =
            Rect::from_center_size(badge.left_center() + Vec2::new(20., 0.), Vec2::splat(14.));
        ui.put(spinner, egui::Spinner::new().size(14.));
        ui.painter().text(
            badge.left_center() + Vec2::new(36., 0.),
            egui::Align2::LEFT_CENTER,
            "Loading photo…",
            egui::FontId::proportional(12.),
            palette.gray(225),
        );
    }
    /// Sets a zoom level; 0 means Fit.
    pub(super) fn set_zoom(&mut self, level: f32) {
        self.view.zoom.set(level);
        // A JPEG, TIFF or PNG in the Loupe zooms on its own; the document
        // behind it is not rendered again.
        let raster_loupe = self.module == Module::Library
            && self
                .library
                .as_ref()
                .is_some_and(|l| l.loupe_open() && l.loupe_develops().is_none());
        if raster_loupe {
            return;
        }
        if self.view.zoom.on && self.view.is(Tool::Crop) {
            self.view.tool = Tool::None;
        }
        self.schedule();
    }
    /// Cmd/Ctrl + and −: step through Lightroom-like zoom levels, with Fit
    /// below the first level larger than the fitted size.
    pub(super) fn step_zoom(&mut self, direction: i32) {
        const LEVELS: [f32; 6] = [0.25, 0.5, 1., 2., 3., 4.];
        // A JPEG, TIFF or PNG in the Loupe fits by its own size.
        let raster_fit = self
            .library
            .as_ref()
            .filter(|l| {
                self.module == Module::Library && l.loupe_open() && l.loupe_develops().is_none()
            })
            .map(|l| l.loupe_fit());
        // Not known until the image is decoded: no step until then.
        if raster_fit == Some(None) {
            return;
        }
        let fit = raster_fit.flatten().unwrap_or_else(|| {
            self.document.full().map_or(0., |im| {
                let g = Geometry::new(im, &self.effective_recipe(), 0);
                (self.view.viewport.x / g.width as f32).min(self.view.viewport.y / g.height as f32)
            })
        });
        let current = if self.view.zoom.on {
            self.view.zoom.level
        } else {
            fit
        };
        let next = if direction > 0 {
            LEVELS.iter().copied().find(|l| *l > current + 1e-3)
        } else {
            LEVELS
                .iter()
                .rev()
                .copied()
                .find(|l| *l < current - 1e-3 && *l > fit + 1e-3)
                .or(Some(0.))
        };
        if let Some(level) = next {
            self.set_zoom(level);
        }
    }
    /// Screen rectangle of the whole photo for the current zoom: see `Zoom::photo_rect`.
    fn photo_rect(&self, area: Rect, g: &Geometry, ppp: f32) -> Rect {
        let size = Vec2::new(g.width as f32, g.height as f32);
        self.view.zoom.photo_rect(area, size, ppp)
    }
    pub(super) fn viewport_ui(&mut self, ui: &mut egui::Ui) {
        let (whole, response) =
            ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        ui.painter()
            .rect_filled(whole, 0., theme::palette(ui.ctx()).photo_backdrop());
        // The edit's side of the viewport; all of it unless Before shows beside it.
        let panes = before_after::panes(self.view.compare, whole);
        let area = panes.after.area;
        let available = area.size();
        let ppp = ui.ctx().pixels_per_point();
        self.view.viewport = available * ppp;
        if !self.view.zoom.on
            && let Some(im) = self.document.full().cloned()
        {
            let g = Geometry::new(&im, &self.effective_recipe(), 0);
            let edge = crate::develop::quality::fit_edge(
                g.width,
                g.height,
                [self.view.viewport.x as u32, self.view.viewport.y as u32],
            );
            if edge != self.preview.last_fit_edge {
                self.schedule();
            }
        }
        // Before's own size can need a new render when the edit's does not, e.g. a
        // Transform of its own; it renders only when its job changes.
        if self.view.compare.two_up() {
            self.schedule_before();
        }
        let region_texture = match self.preview.mode {
            TextureMode::Region(_) => self.preview.region.clone(),
            TextureMode::Whole => None,
        };
        // The stored preview outranks the embedded JPEG until the first render.
        let shown = (!self.preview.standing_in())
            .then(|| self.preview.texture.clone().or(region_texture.clone()))
            .flatten();
        let Some(texture) = shown else {
            // The reference stays on screen, and takes drops, while the Active photo
            // loads.
            if self.reference_view() {
                self.loading_placeholder(ui, panes.after.clip);
                let reference = self.before_pane_ui(ui, &panes, panes.after.area);
                self.reference_pointer(ui, &response, &panes, reference);
            } else {
                self.loading_placeholder(ui, whole);
            }
            return;
        };
        let geometry = self
            .document
            .full()
            .map(|im| Geometry::new(im, &self.effective_recipe(), 0));
        // `rect` is where the whole (cropped) photo sits on screen. The last whole-photo
        // render always fills it, and a 100% region is drawn over its part of it, so
        // zooming and panning scale images that are already there instead of showing
        // a stale region alone until the next render lands.
        let now = ui.input(|i| i.time);
        let target = match &geometry {
            Some(g) => self.photo_rect(area, g, ppp),
            None => {
                let size = texture.size_vec2();
                let k = (available.x / size.x).min(available.y / size.y);
                Rect::from_center_size(area.center(), size * k)
            }
        };
        let key = (self.view.zoom.on, self.view.zoom.level);
        if key != self.view.zoom_key {
            self.view.zoom_key = key;
            self.schedule();
            if let Some(from) = self.view.shown_rect {
                self.view.zoom_anim = Some((now, from));
            }
        }
        let rect = match self.view.zoom_anim {
            Some((start, from)) => {
                let t = ((now - start) / 0.22).clamp(0., 1.) as f32;
                let ease = 1. - (1. - t).powi(3);
                if t >= 1. {
                    self.view.zoom_anim = None;
                } else {
                    ui.ctx().request_repaint();
                }
                Rect::from_min_max(
                    from.min.lerp(target.min, ease),
                    from.max.lerp(target.max, ease),
                )
            }
            None => target,
        };
        self.view.shown_rect = Some(rect);
        let region_rect = match (self.preview.mode, &geometry) {
            (TextureMode::Region(region), Some(g)) => {
                Some(before_after::region_on(rect, g, region))
            }
            _ => None,
        };
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.));
        let painter = ui.painter().with_clip_rect(panes.after.clip);
        if self.preview.texture.is_some() {
            // Opening or leaving the Crop tool changes the crop before the render for
            // it lands: the old render goes where its crop sits, clipped to the new one.
            let now = geometry
                .as_ref()
                .map_or(self.effective_recipe().crop, |g| g.crop());
            match self.preview.crop {
                Some(then) if then != now && geometry.is_some() => {
                    let size = rect.size() / Vec2::new(now[2] - now[0], now[3] - now[1]);
                    let whole = rect.min - Vec2::new(now[0], now[1]) * size;
                    let at = Rect::from_min_max(
                        whole + Vec2::new(then[0], then[1]) * size,
                        whole + Vec2::new(then[2], then[3]) * size,
                    );
                    painter
                        .with_clip_rect(panes.after.clip.intersect(rect))
                        .image(texture.id(), at, uv, Color32::WHITE);
                }
                _ => {
                    painter.image(texture.id(), rect, uv, Color32::WHITE);
                }
            }
        }
        if let (Some(region), Some(at)) = (&region_texture, region_rect) {
            painter.image(region.id(), at, uv, Color32::WHITE);
        }
        if self.view.compare.before_only() {
            before_after::badge(ui, area, "Before");
        }
        self.develop_info_overlay(ui, panes.after.clip);
        // A click or drag on Before acts where the same point is on the edit; the
        // reference photo takes its own (see `reference_pointer`).
        let before_rect = self.before_pane_ui(ui, &panes, rect);
        let on_reference = self.reference_pointer(ui, &response, &panes, before_rect);
        let mirrored = before_rect.filter(|_| !self.reference_view());
        let on_after = |pos: Pos2| match (mirrored, panes.before) {
            (Some(before), Some(pane))
                if pane.clip.contains(pos) && !panes.after.clip.contains(pos) =>
            {
                before_after::to_after(pos, before, rect)
            }
            _ => pos,
        };
        // A tool that owns the pointer (spots, brushes, gradients) takes drags and
        // clicks; the hand tool pans only when no tool claims them.
        // The wheel over the photo sizes the brush, spot or red eye circle, before the
        // tool draws it; not over Before, which can't be edited.
        // Not while Space holds the Hand tool, which hides the brush.
        let hand_held = ui.input(|i| i.key_down(egui::Key::Space));
        if !self.view.compare.shows_before()
            && !hand_held
            && response.hovered()
            && ui.rect_contains_pointer(rect)
            && self.tool_has_size()
        {
            let scrolls = ui.input(super::brush_scroll::Scroll::read);
            for scroll in &scrolls {
                self.scroll_tool_size(*scroll);
            }
            // The panel's Size and Feather were drawn before this; show the new ones.
            if !scrolls.is_empty() {
                ui.ctx().request_repaint();
            }
        }
        let tool_owns_pointer = self.tool_overlay(ui, &response, rect, area);
        // Holding Space pans while an eyedropper is open, as in Lightroom.
        let space = ui.input(|i| i.key_down(egui::Key::Space));
        let picking = self.view.picks_color() && !space && !self.view.compare.shows_before();
        let hand = !tool_owns_pointer && (!self.view.picks_color() || space);
        if self.view.zoom.on && hand && response.dragged() && !on_reference {
            let delta = ui.input(|i| i.pointer.delta());
            // Moved by the photo under the hand: Before's when the drag is on it.
            let on_before = response
                .interact_pointer_pos()
                .is_some_and(|p| on_after(p) != p);
            let dragged = mirrored.filter(|_| on_before).unwrap_or(rect);
            let pan = &mut self.view.zoom.pan;
            pan[0] = (pan[0] - delta.x / dragged.width()).clamp(0., 1.);
            pan[1] = (pan[1] - delta.y / dragged.height()).clamp(0., 1.);
        }
        if response.hovered() && hand && !self.view.is(Tool::Crop) {
            ui.ctx().set_cursor_icon(if self.view.zoom.on {
                egui::CursorIcon::Grab
            } else {
                egui::CursorIcon::ZoomIn
            });
            if response.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            }
        }
        // Lightroom: a click zooms in keeping the clicked point under the
        // pointer; the next click returns to Fit.
        // The second click of a double-click is not another toggle.
        if response.clicked()
            && !response.double_clicked()
            && !response.triple_clicked()
            && hand
            && !on_reference
            && !self.view.is(Tool::Crop)
            && let Some(pos) = response.interact_pointer_pos().map(on_after)
            && rect.contains(pos)
        {
            match &geometry {
                Some(g) => {
                    let size = Vec2::new(g.width as f32, g.height as f32);
                    self.view.zoom.toggle_at(pos, rect, area, size, ppp);
                }
                None => self.view.zoom.on = !self.view.zoom.on,
            }
            self.schedule();
        }
        // The RGB readout follows the pointer over the photo being edited, not over
        // Before (alone or beside it) or the reference, nor in the Library's Loupe.
        let edit_shown = self.module == Module::Develop && !self.view.compare.before_only();
        let hover = response
            .hover_pos()
            .filter(|p| edit_shown && panes.after.clip.contains(*p));
        // Not over pixels rendered for another crop, which sit elsewhere until the
        // render for this one lands.
        let crop = self.effective_recipe().crop;
        let sampled_crop = self.preview.samples_recipe.as_ref().map(|r| r.crop);
        let hover = hover.filter(|_| sampled_crop.is_none_or(|c| c == crop));
        self.update_readout(hover, rect, region_rect);
        if self.wants_samples() {
            // The loupe and the readout read the shown pixels, which renders keep only
            // while they are wanted.
            if self.preview.ask_for_samples() {
                self.schedule();
            }
        } else {
            self.preview.stop_asking_for_samples();
        }
        if self.view.picks_color()
            && let Some(pos) = response.hover_pos()
            && rect.contains(pos)
        {
            self.white_balance_loupe(ui, pos, rect, region_rect, area);
        }
        if self.view.is(Tool::WhiteBalance)
            && picking
            && response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
            && rect.contains(pos)
            && let Some(im) = self.document.full().cloned()
        {
            let u = (pos.x - rect.left()) / rect.width();
            let v = (pos.y - rect.top()) / rect.height();
            self.document.edit.recipe_mut().wb =
                develop::neutral_pick(&im, self.document.edit.recipe(), u, v);
            self.document.edit.recipe_mut().auto_white_balance = None;
            self.document
                .edit
                .recipe_mut()
                .sync_white_balance_controls(&im.metadata);
            self.view.tool = Tool::None;
        }
        // Point Color's dropper adds a swatch of the color under the pointer and
        // selects it; the status line says why when it can't. The preview must show the
        // settings the sample is taken with, or the clicked color isn't the one sampled.
        if self.view.is(Tool::PointColor)
            && picking
            && response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
            && rect.contains(pos)
        {
            // And the final render, not a reduced draft of it.
            if self.preview.samples_recipe.as_ref() == Some(&self.effective_recipe())
                && !self.preview.task.is_running()
            {
                let u = (pos.x - rect.left()) / rect.width();
                let v = (pos.y - rect.top()) / rect.height();
                self.start_point_color_sample(u, v);
            } else {
                self.status = "Wait for the preview to update, then pick again".into();
            }
        }
        // Before shows the unedited photo, so picking there would edit what is not shown.
        if self.view.is(Tool::Defringe)
            && picking
            && response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
            && rect.contains(pos)
        {
            // The samples must come from the settings the pick is mapped through.
            let current = self.preview.samples_recipe.as_ref() == Some(&self.effective_recipe())
                && self.document.full().is_some();
            match self.shown_color(pos, rect, region_rect).filter(|_| current) {
                Some(rgb) => match develop::pick_fringe(self.document.edit.recipe_mut(), rgb) {
                    Some(_) => self.view.tool = Tool::None,
                    None => {
                        self.status =
                            "Cannot set the fringe color: click a purple or green fringe".into();
                    }
                },
                None => self.status = "Wait for the preview to update, then pick again".into(),
            }
        }
        if self.view.is(Tool::Crop) && !self.view.zoom.on && self.document.full().is_some() {
            let c = self.document.edit.recipe().crop;
            let cr = Rect::from_min_max(
                Pos2::new(
                    rect.left() + c[0] * rect.width(),
                    rect.top() + c[1] * rect.height(),
                ),
                Pos2::new(
                    rect.left() + c[2] * rect.width(),
                    rect.top() + c[3] * rect.height(),
                ),
            );
            for shade in [
                Rect::from_min_max(rect.min, Pos2::new(rect.right(), cr.top())),
                Rect::from_min_max(Pos2::new(rect.left(), cr.bottom()), rect.max),
                Rect::from_min_max(
                    Pos2::new(rect.left(), cr.top()),
                    Pos2::new(cr.left(), cr.bottom()),
                ),
                Rect::from_min_max(
                    Pos2::new(cr.right(), cr.top()),
                    Pos2::new(rect.right(), cr.bottom()),
                ),
            ] {
                ui.painter()
                    .rect_filled(shade, 0., Color32::from_black_alpha(140));
            }
            ui.painter().rect_stroke(
                cr,
                0.,
                Stroke::new(1., Color32::WHITE),
                egui::StrokeKind::Inside,
            );
            self.crop_guides_ui(ui, cr, rect);
            let handles = [
                cr.left_top(),
                cr.right_top(),
                cr.right_bottom(),
                cr.left_bottom(),
                cr.left_center(),
                cr.right_center(),
                cr.center_top(),
                cr.center_bottom(),
            ];
            for p in handles {
                ui.painter().rect_filled(
                    Rect::from_center_size(p, Vec2::splat(8.)),
                    1.,
                    Color32::WHITE,
                );
            }
            let ruling = self.straighten_ruler(ui, &response, rect);
            if !ruling
                && response.drag_started()
                && let Some(p) = response.interact_pointer_pos()
            {
                let nearest = handles
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| a.distance(p).total_cmp(&b.distance(p)))
                    .map(|(i, q)| (i, q.distance(p)));
                let handle = nearest
                    .filter(|(_, d)| *d < 25.)
                    .map(|(i, _)| i)
                    .unwrap_or(8);
                self.view.crop_drag = Some((c, handle));
            }
            if !ruling
                && response.dragged()
                && let Some((start, handle)) = self.view.crop_drag
            {
                let delta = response.total_drag_delta().unwrap_or_default();
                let dx = delta.x / rect.width();
                let dy = delta.y / rect.height();
                let mut c = start;
                match handle {
                    0 => {
                        c[0] += dx;
                        c[1] += dy;
                    }
                    1 => {
                        c[2] += dx;
                        c[1] += dy;
                    }
                    2 => {
                        c[2] += dx;
                        c[3] += dy;
                    }
                    3 => {
                        c[0] += dx;
                        c[3] += dy;
                    }
                    4 => c[0] += dx,
                    5 => c[2] += dx,
                    6 => c[1] += dy,
                    7 => c[3] += dy,
                    _ => {
                        let dx = dx.clamp(-c[0], 1. - c[2]);
                        let dy = dy.clamp(-c[1], 1. - c[3]);
                        c[0] += dx;
                        c[2] += dx;
                        c[1] += dy;
                        c[3] += dy;
                    }
                }
                c[0] = c[0].clamp(0., start[2] - 0.01);
                c[1] = c[1].clamp(0., start[3] - 0.01);
                c[2] = c[2].clamp(c[0] + 0.01, 1.);
                c[3] = c[3].clamp(c[1] + 0.01, 1.);
                self.document.edit.recipe_mut().crop = c;
                self.fit_aspect();
            }
            if response.drag_stopped() {
                self.view.crop_drag = None;
            }
        }
        if self.view.zoom.on && self.region() != self.preview.last_region {
            self.schedule();
        }
    }
    /// The crop guide overlay over the crop at `crop` on screen, when it shows: always;
    /// with the pointer over the photo (`photo`), while something is dragged or just after a new
    /// overlay is picked; or never. The Straighten ruler shows a grid.
    fn crop_guides_ui(&self, ui: &egui::Ui, crop: Rect, photo: Rect) {
        let drawing = matches!(self.view.ruler, Ruler::Drawing { .. });
        const SHOWN_AFTER_CHANGE: std::time::Duration = std::time::Duration::from_millis(1500);
        let since_change = self.view.crop_guides_changed.map(|at| at.elapsed());
        let attention = if drawing || self.view.crop_drag.is_some() {
            Attention::Dragging
        } else if ui.rect_contains_pointer(photo) {
            Attention::Hovered
        } else if let Some(since) = since_change.filter(|s| *s < SHOWN_AFTER_CHANGE) {
            ui.ctx().request_repaint_after(SHOWN_AFTER_CHANGE - since);
            Attention::JustChanged
        } else {
            Attention::Away
        };
        let guides = if drawing {
            super::crop_tool::CropGuides {
                guide: Guide::Grid,
                ..self.view.crop_guides
            }
        } else {
            self.view.crop_guides
        };
        if !guides.visible(attention) {
            return;
        }
        let painter = ui.painter().with_clip_rect(crop);
        for line in guides.lines(crop.size()) {
            let points: Vec<Pos2> = line.iter().map(|p| crop.min + p.to_vec2()).collect();
            painter.add(egui::Shape::line(
                points,
                Stroke::new(1., Color32::from_white_alpha(110)),
            ));
        }
    }
    /// The Straighten ruler on the Crop tool: picked in the panel, or a Cmd-drag as in
    /// Lightroom. Releasing it sets the angle that makes the line level or plumb, as one
    /// History step. Returns whether it has the drag, which then moves no crop.
    fn straighten_ruler(&mut self, ui: &egui::Ui, response: &egui::Response, photo: Rect) -> bool {
        let cmd = ui.input(|i| i.modifiers.command);
        let ready = self.view.ruler == Ruler::Armed || cmd;
        let drawing = matches!(self.view.ruler, Ruler::Drawing { .. });
        let over_photo = response.hover_pos().is_some_and(|p| photo.contains(p));
        if (over_photo && ready || drawing) && self.view.crop_drag.is_none() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        // Started on the photo, not the backdrop around it; it may then run off it.
        if response.drag_started()
            && ready
            && let Some(p) = response.interact_pointer_pos()
            && photo.contains(p)
        {
            self.view.ruler = Ruler::Drawing { from: p, to: p };
        }
        let Ruler::Drawing { from, to } = &mut self.view.ruler else {
            return false;
        };
        if let Some(p) = response.interact_pointer_pos() {
            *to = p;
        }
        let (from, to) = (*from, *to);
        let painter = ui.painter();
        painter.line_segment([from, to], Stroke::new(3., Color32::from_black_alpha(140)));
        painter.line_segment([from, to], Stroke::new(1., Color32::WHITE));
        for end in [from, to] {
            painter.circle_filled(end, 3., Color32::WHITE);
        }
        if response.drag_stopped() || !response.dragged() {
            self.view.ruler = Ruler::Off;
            let current = self.document.edit.recipe().straighten;
            if let Some(angle) = super::crop_tool::ruler_angle(from, to, current)
                && angle != current
            {
                self.document.edit.recipe_mut().straighten = angle;
                self.document.edit.name_next_step(super::history::Step::new(
                    "Straighten",
                    format!("{angle:+.2}"),
                ));
            }
        }
        true
    }
    /// The Loupe's Info overlay (I) over the photo being edited, below the Before,
    /// After or Active label when there is one. The Library's Loupe draws its own.
    fn develop_info_overlay(&mut self, ui: &egui::Ui, pane: Rect) {
        if self.module == Module::Library {
            return;
        }
        let labelled = self.view.compare != before_after::Compare::Off;
        let top = if labelled { 37. } else { 0. };
        let at = Rect::from_min_max(pane.min + Vec2::new(0., top), pane.max);
        if let Some(library) = &mut self.library {
            library.loupe_overlay(&ui.painter().with_clip_rect(pane), at);
        }
    }
    /// Lightroom's white balance selector over the photo: an eyedropper cursor
    /// whose tip is the picked point, and a loupe of the pixels around it with the
    /// values of the one under the tip.
    /// The shown color (encoded sRGB, 0–1) at `pos`, from the samples renders keep while
    /// an eyedropper is active.
    fn shown_color(&self, pos: Pos2, rect: Rect, region: Option<Rect>) -> Option<[f32; 3]> {
        let (samples, shown) = match (&self.preview.region_samples, region) {
            (Some(samples), Some(region)) if region.contains(pos) => (samples, region),
            _ => (self.preview.samples.as_ref()?, rect),
        };
        let (w, h) = samples.dimensions();
        let cx = ((pos.x - shown.left()) / shown.width() * w as f32).floor() as i32;
        let cy = ((pos.y - shown.top()) / shown.height() * h as f32).floor() as i32;
        // The most colourful of the 3 × 3 pixels: a fringe can be a single pixel wide
        // beside neutral ones, which an average would wash out.
        (cy - 1..=cy + 1)
            .flat_map(|y| (cx - 1..=cx + 1).map(move |x| (x, y)))
            .filter(|&(x, y)| x >= 0 && y >= 0 && x < w as i32 && y < h as i32)
            .map(|(x, y)| {
                samples
                    .get_pixel(x as u32, y as u32)
                    .0
                    .map(|v| f32::from(v) / 255.)
            })
            .max_by(|a, b| {
                let spread = |p: &[f32; 3]| {
                    p.iter().fold(0f32, |m, v| m.max(*v)) - p.iter().fold(1f32, |m, v| m.min(*v))
                };
                spread(a).total_cmp(&spread(b))
            })
    }
    fn white_balance_loupe(
        &self,
        ui: &egui::Ui,
        pos: Pos2,
        rect: Rect,
        region: Option<Rect>,
        area: Rect,
    ) {
        let palette = theme::palette(ui.ctx());
        const CELLS: i32 = 5;
        const CELL: f32 = 30.;
        let painter = ui
            .ctx()
            .layer_painter(egui::LayerId::new(
                egui::Order::Tooltip,
                egui::Id::new("white-balance-loupe"),
            ))
            .with_clip_rect(area);
        // Lucide's pipette has its tip at (2, 22) of 24.
        ui.ctx().set_cursor_icon(egui::CursorIcon::None);
        let icon = 22.;
        let tip = Vec2::new(icon / 2. - icon * 2. / 24., icon / 2. - icon * 22. / 24.);
        for d in [
            Vec2::new(1., 0.),
            Vec2::new(-1., 0.),
            Vec2::new(0., 1.),
            Vec2::new(0., -1.),
        ] {
            icons::paint_at(
                &painter,
                Icon::Eyedropper,
                pos + tip + d,
                icon,
                Color32::BLACK,
            );
        }
        icons::paint_at(&painter, Icon::Eyedropper, pos + tip, icon, Color32::WHITE);
        let (samples, shown) = match (&self.preview.region_samples, region) {
            (Some(samples), Some(region)) if region.contains(pos) => (samples, region),
            _ => match &self.preview.samples {
                Some(samples) => (samples, rect),
                None => return,
            },
        };
        let (w, h) = samples.dimensions();
        let at = |p: f32, lo: f32, len: f32, n: u32| ((p - lo) / len * n as f32).floor() as i32;
        let cx = at(pos.x, shown.left(), shown.width(), w);
        let cy = at(pos.y, shown.top(), shown.height(), h);
        let pixel = |x: i32, y: i32| {
            (x >= 0 && y >= 0 && x < w as i32 && y < h as i32)
                .then(|| samples.get_pixel(x as u32, y as u32).0)
        };
        // In the readout's Melissa RGB, as Lightroom's loupe.
        let text = pixel(cx, cy).map_or_else(String::new, |p| {
            super::readout::text(super::readout::melissa_percent(
                p.map(|v| f32::from(v) / 255.),
            ))
        });
        let values =
            painter.layout_no_wrap(text, egui::FontId::proportional(13.), palette.gray(225));
        let grid = CELLS as f32 * CELL;
        let pad = 8.;
        let line = 24.;
        // Wide enough for the values, with the grid centred.
        let size = Vec2::new(
            (grid + 2. * pad).max(values.size().x + 2. * pad + 8.),
            grid + 2. * line,
        );
        // Just below and right of the tip, as in Lightroom, sliding along the photo
        // area's edges. Only at the bottom does it move, to above-left, clear of the
        // eyedropper.
        let gap = Vec2::new(10., 14.);
        let mut min = pos + gap;
        if min.y + size.y > area.bottom() {
            min = pos - gap - size;
        }
        min.x = min.x.min(area.right() - size.x).max(area.left());
        min.y = min.y.max(area.top());
        let frame = Rect::from_min_size(min, size);
        painter.add(
            egui::Shadow {
                offset: [0, 4],
                blur: 16,
                spread: 0,
                color: Color32::from_black_alpha(120),
            }
            .as_shape(frame, 8.),
        );
        painter.rect_filled(frame, 8., palette.gray(24));
        painter.text(
            Pos2::new(frame.center().x, frame.top() + line / 2.),
            egui::Align2::CENTER_CENTER,
            self.view.loupe_prompt(),
            egui::FontId::proportional(12.),
            palette.gray(215),
        );
        let origin = Pos2::new(frame.center().x - grid / 2., frame.top() + line);
        let cells = Rect::from_min_size(origin, Vec2::splat(grid));
        painter.rect_filled(cells, 0., palette.gray(60));
        let half = CELLS / 2;
        for j in 0..CELLS {
            for i in 0..CELLS {
                let cell = Rect::from_min_size(
                    origin + Vec2::new(i as f32, j as f32) * CELL,
                    Vec2::splat(CELL),
                )
                .shrink(0.5);
                let color = pixel(cx + i - half, cy + j - half)
                    .map_or(palette.photo_backdrop(), |[r, g, b]| {
                        Color32::from_rgb(r, g, b)
                    });
                painter.rect_filled(cell, 0., color);
            }
        }
        // A small cross marks the picked pixel, dark or light to stay visible on it.
        let center = cells.center();
        let mark = pixel(cx, cy).map_or(Color32::WHITE, |[r, g, b]| {
            let luma = 0.3 * f32::from(r) + 0.59 * f32::from(g) + 0.11 * f32::from(b);
            if luma > 110. {
                Color32::from_black_alpha(150)
            } else {
                Color32::from_white_alpha(170)
            }
        });
        for d in [Vec2::new(4., 0.), Vec2::new(0., 4.)] {
            painter.line_segment([center - d, center + d], Stroke::new(1., mark));
        }
        painter.galley(
            Pos2::new(frame.center().x, frame.bottom() - line / 2.) - values.size() / 2.,
            values,
            palette.gray(225),
        );
    }
}
