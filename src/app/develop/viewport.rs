use crate::app::Editor;
use crate::app::Module;
use crate::app::develop::before_after;
use crate::app::develop::state::{TextureMode, Tool};
use crate::app::develop::tools::crop_drag::{self, CropDrag, Handle};
use crate::app::develop::tools::crop_tool::{Attention, Guide, Ruler, crop_aspect_shown};
use crate::app::ui::design::{Tone, radius, text};
use crate::app::ui::icons::{self, Icon};
use crate::app::ui::theme;
use crate::app::view::navigator;
use crate::develop::{self, Geometry};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};

impl Editor {
    /// Constrains the crop to [`Self::crop_constraint`]; a Free crop is left as it is.
    pub(in crate::app) fn fit_aspect(&mut self) {
        if let Some(crop) = self.fitted_crop() {
            self.document.edit.recipe_mut().crop = crop;
        }
    }
    /// The crop [`Self::fit_aspect`] makes, when it changes it.
    fn fitted_crop(&self) -> Option<[f32; 4]> {
        let ratio = self.crop_constraint()?;
        let c = self.document.edit.recipe().crop;
        let cx = (c[0] + c[2]) / 2.;
        let cy = (c[1] + c[3]) / 2.;
        let mut w = c[2] - c[0];
        let mut h = c[3] - c[1];
        // A crop already at the aspect and size stays exactly as it is: rounding
        // would otherwise nudge an uncropped photo's crop off the whole frame.
        if (w / h / ratio - 1.).abs() < 1e-5 && w >= 0.01 && h >= 0.01 {
            return None;
        }
        if w / h > ratio {
            w = h * ratio;
        } else {
            h = w / ratio;
        }
        let w = w.max(0.01);
        let h = h.max(0.01);
        Some([cx - w / 2., cy - h / 2., cx + w / 2., cy + h / 2.])
    }
    /// Opening the Crop tool shows the photo's own aspect, as Lightroom does,
    /// rather than the last photo's: Original at the photo's ratio, a preset it
    /// matches, or else its exact ratio as Custom. An uncropped photo takes the
    /// aspect last chosen from the menu, and a preset crops it as an edit of its
    /// own, recorded before any frame or command that follows; not when the photo
    /// was loaded again with the tool open, which keeps the edit loaded. With Free
    /// chosen the aspect stays unlocked, whatever the crop.
    pub(in crate::app) fn read_aspect(&mut self) {
        if !self.view.is(Tool::Crop) || self.view.aspect_read {
            return;
        }
        let Some(photo) = self.crop_frame() else {
            return;
        };
        self.view.aspect_read = true;
        let c = self.document.edit.recipe().crop;
        // Free is the padlock open: reading a photo names its ratio in the menu but
        // never locks it, as Lightroom's open padlock stays open when Crop opens again
        // on its Custom crop.
        let free = self.view.chosen_aspect == 0.;
        if c == [0., 0., 1., 1.] && self.view.aspect_reloaded {
            self.view.aspect = if free { 0. } else { -1. };
            return;
        }
        if c == [0., 0., 1., 1.] {
            self.view.aspect = self.view.chosen_aspect;
            if let Some(crop) = self.fitted_crop() {
                // A drag still held (R pressed mid-drag) is a step of its own first.
                self.document.edit.finish_gesture();
                self.document.edit.change(None, |r| r.crop = crop);
            }
            return;
        }
        self.view.aspect = if free {
            0.
        } else {
            crop_aspect_shown(c, photo)
        };
    }
    /// What Develop's minimap shows: the photo, and the part in view, at any
    /// zoom (the 100% region the renderer draws exists only from 100% up).
    pub(in crate::app) fn navigator_view(&self) -> (Option<navigator::Photo>, Option<[f32; 4]>) {
        let shown = self.document.full().and_then(|im| {
            let g = Geometry::new(im, &self.effective_recipe(), 0);
            let size = Vec2::new(g.width as f32, g.height as f32);
            self.view.zoom.shown(self.view.viewport, size)
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
        (photo, shown)
    }
    /// A point of the edit chosen on the minimap: zoomed in there.
    pub(in crate::app) fn inspect(&mut self, at: [f32; 2]) {
        self.view.glide_to(at, self.document.catalog_photo);
        if self.view.is(Tool::Crop) {
            self.view.tool = Tool::None;
        }
    }
    /// Before the first image arrives: the Library preview of the photo being
    /// opened with a spinner, or without one if it could not be opened, or a
    /// hint when nothing is open.
    fn loading_placeholder(&self, ui: &mut egui::Ui, area: Rect) {
        let palette = theme::palette(ui.ctx());
        // A photo counts as opening until its first render arrives, even after
        // the decode has finished; only an empty document shows the hint.
        let opening = self.document.loading()
            || self.document.catalog_photo.is_some()
            || self.document.path.is_some();
        if !opening {
            let empty = self
                .library
                .as_ref()
                .is_none_or(|l| l.session.photos().is_empty());
            ui.painter().text(
                area.center(),
                egui::Align2::CENTER_CENTER,
                if empty {
                    "Add photos in the Library to start editing"
                } else {
                    "Pick a photo in the Library to edit it"
                },
                egui::FontId::proportional(text::TITLE),
                palette.tone(Tone::FAINT),
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
        if self.document.failed.is_some() {
            ui.painter().text(
                badge.center(),
                egui::Align2::CENTER_CENTER,
                "Couldn't open this photo",
                egui::FontId::proportional(text::LABEL),
                palette.tone(Tone::TEXT),
            );
            return;
        }
        let spinner =
            Rect::from_center_size(badge.left_center() + Vec2::new(20., 0.), Vec2::splat(14.));
        crate::app::ui::widgets::paint_spinner(ui, spinner, None);
        ui.painter().text(
            badge.left_center() + Vec2::new(36., 0.),
            egui::Align2::LEFT_CENTER,
            "Loading photo…",
            egui::FontId::proportional(text::LABEL),
            palette.tone(Tone::TEXT),
        );
    }
    /// Sets a zoom level; 0 means Fit.
    pub(in crate::app) fn set_zoom(&mut self, level: f32) {
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
    /// The level the photo shown fits the view at: Develop's photo, or a JPEG,
    /// TIFF or PNG in the Library's Loupe. None until its size is known.
    pub(in crate::app) fn fit_level(&self) -> Option<f32> {
        let raster = self.module == Module::Library
            && self
                .library
                .as_ref()
                .is_some_and(|l| l.loupe_open() && l.loupe_develops().is_none());
        if raster {
            return self.library.as_ref()?.loupe_fit();
        }
        let im = self.document.full()?;
        let g = Geometry::new(im, &self.effective_recipe(), 0);
        Some((self.view.viewport.x / g.width as f32).min(self.view.viewport.y / g.height as f32))
    }
    /// Cmd/Ctrl = and −: zoom in or out by a constant factor, stopping at 100% and
    /// Fit (see `navigator::stepped`).
    pub(in crate::app) fn step_zoom(&mut self, direction: i32) {
        // Not known until the image is decoded: no step until then.
        let Some(fit) = self.fit_level() else {
            return;
        };
        let current = if self.view.zoom.on {
            self.view.zoom.level
        } else {
            fit
        };
        self.set_zoom(navigator::stepped(current, fit, direction > 0));
    }
    /// Screen rectangle of the whole photo for the current zoom: see `Zoom::photo_rect`.
    fn photo_rect(&self, area: Rect, g: &Geometry, ppp: f32) -> Rect {
        let size = Vec2::new(g.width as f32, g.height as f32);
        self.view.zoom.photo_rect(area, size, ppp)
    }
    pub(in crate::app) fn viewport_ui(&mut self, ui: &mut egui::Ui) -> egui::Response {
        let (whole, response) =
            ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        ui.painter()
            .rect_filled(whole, 0., theme::palette(ui.ctx()).photo_backdrop());
        if self.module == Module::Develop
            && self.view.compare == before_after::Compare::CameraThumbnail
        {
            self.camera_thumbnail_ui(ui, whole);
            return response;
        }
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
            return response;
        };
        let geometry = self
            .document
            .full()
            .map(|im| Geometry::new(im, &self.effective_recipe(), 0));
        self.view.glide_pan(ui.ctx(), self.document.catalog_photo);
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
            // Zoomed in, the photo sits on whole screen pixels: at 1.5x scaling a
            // pointer step is a pixel and a half, and a photo drawn between
            // pixels softens every other frame of a drag.
            None if self.view.zoom.on => {
                let snap = |v: f32| (v * ppp).round() / ppp;
                target.translate(Pos2::new(snap(target.min.x), snap(target.min.y)) - target.min)
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
        if let Some(g) = &geometry {
            let due = self.hdr_preview_due();
            self.hdr_preview_ui(&painter, rect, g, due, false);
        }
        if self.view.compare.renders_before_alone() {
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
            let scrolls = ui.input(crate::app::develop::tools::brush_scroll::Scroll::read);
            for scroll in &scrolls {
                self.scroll_tool_size(*scroll);
            }
            // The panel's Size and Feather were drawn before this; show the new ones.
            if !scrolls.is_empty() {
                ui.ctx().request_repaint();
            }
        }
        // Zoomed in, the minimap shows where, as Lightroom's Navigator does.
        let on_minimap = self.view.zoom.on && self.develop_minimap(ui, panes.after.clip);
        let tool_owns_pointer = self.tool_overlay(ui, &response, rect, area);
        // Holding Space pans while an eyedropper is open, as in Lightroom.
        let space = ui.input(|i| i.key_down(egui::Key::Space));
        let picking = self.view.picks_color() && !space && !self.view.compare.shows_before();
        let hand = !tool_owns_pointer && (!self.view.picks_color() || space);
        if self.view.zoom.on && hand && response.dragged() && !on_reference && !on_minimap {
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
        // The magnifier over the fitted edit, in Develop or the Library's Loupe,
        // when no tool owns the pointer.
        if self.view.magnifier.on
            && !self.view.zoom.on
            && self.view.zoom_anim.is_none()
            && self.view.compare == before_after::Compare::Off
            && self.view.is(Tool::None)
            && !response.dragged()
        {
            self.magnifier_ui(ui, response.hover_pos(), rect);
        }
        // Lightroom: a click zooms in keeping the clicked point under the
        // pointer; the next click returns to Fit.
        // The second click of a double-click is not another toggle.
        if response.clicked()
            && !response.double_clicked()
            && !response.triple_clicked()
            && hand
            && !on_reference
            && !on_minimap
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
            let gains = develop::neutral_pick(&im, self.document.edit.recipe(), u, v);
            let recipe = self.document.edit.recipe_mut();
            recipe.pick_white_balance(gains, &im.metadata);
            recipe.auto_white_balance = None;
            self.view.tool = Tool::None;
        }
        // Point Color's dropper adds a swatch of the color under the pointer and
        // selects it; the top bar says why when it can't. The preview must show the
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
                self.messages
                    .warn("Wait for the preview to update, then pick again");
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
                        self.messages
                            .warn("Cannot set the fringe color: click a purple or green fringe");
                    }
                },
                None => self
                    .messages
                    .warn("Wait for the preview to update, then pick again"),
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
            for h in Handle::ALL {
                ui.painter().rect_filled(
                    Rect::from_center_size(h.on(cr), Vec2::splat(8.)),
                    1.,
                    Color32::WHITE,
                );
            }
            let ruling = self.straighten_ruler(ui, &response, rect);
            let alt = ui.input(|i| i.modifiers.alt);
            if !ruling
                && response.drag_started()
                && let Some(p) = ui
                    .input(|i| i.pointer.press_origin())
                    .or_else(|| response.interact_pointer_pos())
            {
                // The handle under the press, not under the pointer once it has moved far
                // enough to start a drag: a quick flick would otherwise miss it.
                self.view.crop_drag = Some(CropDrag::new(c, Handle::pressed(cr, p), alt));
            }
            if !ruling
                && response.dragged()
                && let Some(drag) = self.view.crop_drag.as_mut()
            {
                let moved = response.total_drag_delta().unwrap_or_default();
                drag.follow_alt(alt, c, moved);
                let drag = *drag;
                let d = (moved - drag.from) / rect.size();
                match drag.handle.filter(|_| drag.about_centre) {
                    Some(handle) => {
                        let ratio = self.crop_constraint();
                        let crop = crop_drag::resize_about_centre(
                            drag.start,
                            handle,
                            d,
                            ratio,
                            rect.width() / rect.height(),
                        );
                        self.document.edit.recipe_mut().crop = crop;
                    }
                    None => {
                        self.document.edit.recipe_mut().crop =
                            crop_drag::drag_sides(drag.start, drag.handle, d);
                        self.fit_aspect();
                    }
                }
            }
            if response.drag_stopped() {
                self.view.crop_drag = None;
            }
        }
        if self.view.zoom.on && self.region() != self.preview.last_region {
            self.schedule_pan();
        }
        response
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
            crate::app::develop::tools::crop_tool::CropGuides {
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
            if let Some(angle) =
                crate::app::develop::tools::crop_tool::ruler_angle(from, to, current)
                && angle != current
            {
                crate::model::edit::change_setting(
                    self.document.edit.recipe_mut(),
                    crate::model::params::ParameterId::Straighten,
                    None,
                    |r| r.straighten = angle,
                );
                self.document
                    .edit
                    .name_next_step(crate::app::history::Step::new(
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
            crate::app::develop::readout::text(crate::app::develop::readout::melissa_percent(
                p.map(|v| f32::from(v) / 255.),
            ))
        });
        let values = painter.layout_no_wrap(
            text,
            egui::FontId::proportional(text::BODY),
            palette.tone(Tone::TEXT),
        );
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
        painter.rect_filled(frame, radius::CARD, palette.tone(Tone::WINDOW));
        painter.text(
            Pos2::new(frame.center().x, frame.top() + line / 2.),
            egui::Align2::CENTER_CENTER,
            self.view.loupe_prompt(),
            egui::FontId::proportional(text::LABEL),
            palette.tone(Tone::TEXT),
        );
        let origin = Pos2::new(frame.center().x - grid / 2., frame.top() + line);
        let cells = Rect::from_min_size(origin, Vec2::splat(grid));
        painter.rect_filled(cells, 0., palette.tone(Tone::HOVER));
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
            palette.tone(Tone::TEXT),
        );
    }
}

/// Why a side shows its SDR rendition when its settings have HDR off.
pub(in crate::app) const HDR_OFF: &str = "HDR is off";

/// An HDR preview due on one side of the viewport.
pub(in crate::app) struct HdrDue {
    pub(in crate::app) image: std::sync::Arc<crate::camera_data::CameraImage>,
    /// What the side shows at its zoom: the whole photo, or a 100% region.
    pub(in crate::app) key: crate::app::develop::hdr_preview::Key,
    /// Under a region, the whole photo the SDR rendition draws there too; shown when
    /// a preview of it is there (from Fit), never asked for while zoomed in.
    pub(in crate::app) under: Option<crate::app::develop::hdr_preview::Key>,
}
impl std::fmt::Debug for HdrDue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HdrDue")
            .field("key", &self.key)
            .field("under", &self.under)
            .finish_non_exhaustive()
    }
}
impl HdrDue {
    /// The slots of `key` and `under`, for the edit's side or Before's.
    fn slots(
        &self,
        before: bool,
    ) -> (
        crate::app::develop::hdr_preview::Slot,
        crate::app::develop::hdr_preview::Slot,
    ) {
        use crate::app::develop::hdr_preview::Slot;
        let (region, whole) = if before {
            (Slot::BeforeRegion, Slot::BeforeWhole)
        } else {
            (Slot::Region, Slot::Whole)
        };
        match self.key.part() {
            crate::app::develop::hdr_preview::Part::Region(_) => (region, whole),
            crate::app::develop::hdr_preview::Part::Whole(_) => (whole, region),
        }
    }
}

impl crate::app::Editor {
    /// Why no HDR preview shows anywhere in Develop's viewport, if none can.
    fn hdr_refused(&self) -> Result<(), &'static str> {
        let refuse = |no: bool, why| if no { Err(why) } else { Ok(()) };
        refuse(
            !self.view.hdr_display.available(),
            "this display shows no HDR",
        )?;
        // A chosen monitor profile converts the SDR rendition alone; the surface
        // then holds device RGB, which has no extended range.
        refuse(
            self.view.monitor.device_rgb(),
            "HDR shows only with the system's display profile",
        )?;
        refuse(self.module != Module::Develop, "HDR shows in Develop")?;
        refuse(
            self.view.compare == before_after::Compare::CameraThumbnail,
            "the camera's thumbnail is shown",
        )?;
        // Visualizations render into the SDR rendition: the clipping warning,
        // Remove's spots, a mask's overlay and Point Color's range.
        refuse(
            self.view.clipping.overlay().any()
                || self.overlay() != crate::app::worker::Overlay::None
                || self.visualized_swatch().is_some(),
            "a visualization is shown",
        )
    }
    /// The HDR preview due for `recipe` of `image`, shown as `part` with `under` it
    /// (see [`HdrDue`]), or why its SDR rendition shows.
    fn hdr_due_for(
        &self,
        image: &std::sync::Arc<crate::camera_data::CameraImage>,
        recipe: &crate::model::recipe::Recipe,
        region: Option<[u32; 4]>,
        max_edge: u32,
    ) -> Result<HdrDue, &'static str> {
        use crate::app::develop::hdr_preview::{Key, Part};
        let refuse = |no: bool, why| if no { Err(why) } else { Ok(()) };
        refuse(!recipe.hdr.is_active(), HDR_OFF)?;
        refuse(
            crate::model::hdr::check_for(recipe, &image.metadata).is_err(),
            "this edit doesn't render in HDR yet",
        )?;
        // Its result is the full photo's, rendered on its own worker.
        refuse(
            recipe.ai_denoise.enabled,
            "AI Denoise isn't previewed in HDR yet",
        )?;
        refuse(max_edge == 0, "the photo is still opening")?;
        let headroom = self.view.hdr_display.headroom;
        let whole = Key::new(image, recipe, Part::Whole(max_edge), headroom);
        Ok(match region {
            Some(region) => HdrDue {
                image: image.clone(),
                key: Key::new(image, recipe, Part::Region(region), headroom),
                under: Some(whole),
            },
            None => HdrDue {
                image: image.clone(),
                key: whole,
                under: None,
            },
        })
    }
    /// The HDR preview Develop's viewport is due to show of the photo it renders
    /// (the edit, or Before alone): where the display has headroom and the photo is
    /// an edit with HDR on whose master renders, with nothing drawn into its SDR
    /// rendition; the whole photo at its zoom, or the 100% region it renders.
    /// Otherwise why the SDR rendition shows, for the Basic panel's note.
    pub(in crate::app) fn hdr_preview_due(&self) -> Result<HdrDue, &'static str> {
        self.hdr_refused()?;
        let image = self.document.full().ok_or("the photo is still opening")?;
        if self.preview.last_fit_edge == 0 {
            return Err("the photo is still opening");
        }
        let recipe = self.effective_recipe();
        let g = Geometry::new(image, &recipe, 0);
        let max_edge = self.render_edges(&g).max_edge;
        self.hdr_due_for(image, &recipe, self.region_in(&g), max_edge)
    }
    /// The HDR preview due beside the edit: of Before, or of the reference photo, as
    /// its SDR rendition last asked for it.
    pub(in crate::app) fn hdr_second_side_due(&self) -> Result<HdrDue, &'static str> {
        self.hdr_refused()?;
        let side = self
            .second_side_submitted()
            .ok_or("nothing is beside the edit")?;
        self.hdr_due_for(side.image, side.recipe, side.region, side.max_edge)
    }
    /// What Develop shows of the open edit, for the Basic panel's note.
    pub(in crate::app) fn hdr_presentation(
        &self,
    ) -> crate::app::develop::hdr_preview::Presentation {
        self.presentation_of(self.hdr_preview_due(), false)
    }
    /// What Develop shows of the side beside the edit, named (Before, or the
    /// reference photo), while one is shown.
    pub(in crate::app) fn hdr_second_side_presentation(
        &self,
    ) -> Option<(&'static str, crate::app::develop::hdr_preview::Presentation)> {
        let compare = self.view.compare;
        if !compare.two_up() || self.second_side_submitted().is_none() {
            return None;
        }
        let side = if compare.reference() {
            "Reference"
        } else {
            "Before"
        };
        Some((side, self.presentation_of(self.hdr_second_side_due(), true)))
    }
    fn presentation_of(
        &self,
        due: Result<HdrDue, &'static str>,
        before: bool,
    ) -> crate::app::develop::hdr_preview::Presentation {
        use crate::app::develop::hdr_preview::Presentation;
        match due {
            Err(why) => Presentation::Sdr(why),
            Ok(due) => {
                let (slot, _) = due.slots(before);
                match self.hdr_preview.failure(slot, &due.key) {
                    Some(e) => Presentation::Failed(e.to_owned()),
                    None if self.hdr_preview.shows(slot, &due.key) => Presentation::Hdr {
                        stops: self.view.hdr_display.headroom.log2(),
                    },
                    None => Presentation::Rendering,
                }
            }
        }
    }
    /// Draws the HDR master of one side of the viewport over its SDR rendition when
    /// it is `due`: the photo drawn at `rect` with geometry `g`, and, over it, its
    /// 100% region. Each draws once its preview for exactly this edit, part and
    /// headroom is there; the SDR rendition shows until then. What a side leaves
    /// unused is released when the frame ends ([`crate::app::develop::hdr_preview::HdrPreview::sweep`]). Asks for EDR (`ViewState::hdr_shown`) whenever a master is due and
    /// hasn't failed.
    pub(in crate::app) fn hdr_preview_ui(
        &mut self,
        painter: &egui::Painter,
        rect: Rect,
        g: &Geometry,
        due: Result<HdrDue, &'static str>,
        before: bool,
    ) {
        use crate::app::develop::hdr_preview::Part;
        let state = self.render_state.clone();
        let (Some(state), Ok(due)) = (state, due) else {
            return;
        };
        let (slot, other) = due.slots(before);
        self.hdr_preview
            .request(slot, due.key.clone(), due.image.clone());
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.));
        let draw = |preview: &mut crate::app::develop::hdr_preview::HdrPreview,
                    slot,
                    key: &crate::app::develop::hdr_preview::Key| {
            let at = match key.part() {
                Part::Whole(_) => rect,
                Part::Region(region) => before_after::region_on(rect, g, region),
            };
            if let Some(id) = preview.texture(slot, key, &state) {
                painter.image(id, at, uv, Color32::WHITE);
            }
        };
        // The SDR rendition's own region is drawn over its whole photo; an HDR one
        // under it would hide it. So the whole photo's preview shows under a region
        // only with the region's.
        let shown = self.hdr_preview.texture(slot, &due.key, &state).is_some();
        if let Some(under) = &due.under {
            self.hdr_preview.keep(other);
            if shown {
                draw(&mut self.hdr_preview, other, under);
            }
        }
        draw(&mut self.hdr_preview, slot, &due.key);
        // EDR stays on while an edit's preview is re-rendered: macOS ramps the
        // display's headroom when it is switched, and the next preview needs it.
        if self.hdr_preview.failure(slot, &due.key).is_none() {
            self.view.hdr_shown = true;
        }
    }
}

#[cfg(test)]
#[path = "viewport_editor_tests.rs"]
mod editor_tests;

/// "Editable Preview" in the top-left corner of `area`, the photo's view, with what
/// it means on hover: Develop and the Loupe say when a photo is edited from its
/// editable preview rather than its original.
pub(in crate::app) fn source_badge(ui: &mut egui::Ui, area: Rect, detail: &str) {
    let palette = theme::palette(ui.ctx());
    let font = egui::FontId::proportional(text::SMALL);
    let galley = ui.painter().layout_no_wrap(
        crate::app::worker::OpenedFrom::EDITABLE_PREVIEW.to_string(),
        font,
        palette.tone(Tone::TEXT),
    );
    let pad = Vec2::new(6., 3.);
    let rect = Rect::from_min_size(area.min + Vec2::splat(8.), galley.size() + pad * 2.);
    ui.painter().rect_filled(
        rect,
        radius::CONTROL,
        palette.tone(Tone::WINDOW).gamma_multiply(0.85),
    );
    ui.painter()
        .galley(rect.min + pad, galley, palette.tone(Tone::TEXT));
    ui.interact(rect, ui.id().with("editable-preview-badge"), Sense::hover())
        .on_hover_text(detail);
}
