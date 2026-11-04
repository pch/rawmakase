//! Photos shown large, side by side, as Compare and Survey show them: each
//! fitted to its place with its edit at the size shown (see `screen`), the
//! grid's preview standing in until it is ready, and a caption below.
use super::Library;
use super::previews::EditSource;
use super::screen::Shown;
use crate::app::library::photo_metadata::{flag_icon, label_color};
use crate::app::ui::design::{Tone, inset, radius, text};
use crate::app::ui::theme;
use crate::catalog::{Photo, PhotoId};
use eframe::egui::{self, Color32, Rect, Vec2};
use std::collections::HashMap;

/// The strip under each photo: its role, name, rating, flag and label.
const CAPTION: f32 = 26.;
/// Space between the photos and around them.
pub(super) const MARGIN: f32 = inset::STAGE;
/// Below this many pixels the grid's preview of a JPEG, TIFF or PNG is
/// sharp enough, and no screen preview is made.
const GRID_EDGE: u32 = 640;
/// Seconds an edit stamp is trusted before it is read again.
const STAMP_AGE: f64 = 0.5;

/// Edit stamps by photo, with when each was read.
#[derive(Debug, Default)]
pub(super) struct Stamps(HashMap<PhotoId, (u64, f64)>);

impl Library {
    /// Draws `photo` fitted to `rect`, outlined when `active`, with its
    /// caption below (led by `role`, e.g. "Select"). Returns where the
    /// photo is drawn.
    pub(super) fn stage_photo(
        &mut self,
        ui: &egui::Ui,
        rect: Rect,
        photo: &Photo,
        role: Option<&str>,
        active: bool,
    ) -> Option<Rect> {
        let palette = theme::palette(ui.ctx());
        // A tile too small for a caption shows the photo alone.
        let captioned = rect.height() > CAPTION * 3.;
        let image_area = if captioned {
            Rect::from_min_max(rect.min, rect.max - Vec2::new(0., CAPTION))
        } else {
            rect
        };
        let (texture, note) = self.stage_preview(ui.ctx(), photo, image_area);
        let shown = texture.map(|texture| {
            let size = texture.size_vec2();
            let scale = (image_area.width() / size.x).min(image_area.height() / size.y);
            let at = Rect::from_center_size(image_area.center(), size * scale);
            let uv = Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.));
            ui.painter().image(texture.id(), at, uv, Color32::WHITE);
            if active {
                ui.painter().rect_stroke(
                    at.expand(3.),
                    1.,
                    egui::Stroke::new(1.5, palette.tone(Tone::TEXT)),
                    egui::StrokeKind::Outside,
                );
            }
            at
        });
        if let Some(note) = note {
            ui.painter().text(
                image_area.center_bottom() - Vec2::new(0., 8.),
                egui::Align2::CENTER_BOTTOM,
                note,
                egui::FontId::proportional(text::SMALL),
                palette.tone(Tone::LABEL),
            );
        }
        if captioned {
            let caption = Rect::from_min_max(rect.left_bottom() - Vec2::new(0., CAPTION), rect.max);
            let painter = ui.painter().with_clip_rect(caption);
            caption_strip(&painter, caption, role, photo, active);
        }
        shown
    }
    /// The photo's screen preview, or the grid's until it is ready, and a
    /// note on what is shown.
    fn stage_preview(
        &mut self,
        ctx: &egui::Context,
        photo: &Photo,
        area: Rect,
    ) -> (Option<egui::TextureHandle>, Option<String>) {
        self.request_previews(photo, ctx);
        let stand_in = self.texture(photo).cloned();
        if !self.is_available(&photo.path) {
            let note = match stand_in {
                Some(_) => "Offline: showing the cached preview",
                None => "Offline, and there is no cached preview",
            };
            return (stand_in, Some(note.into()));
        }
        let edge = (area.width().max(area.height()) * ctx.pixels_per_point()) as u32;
        // A JPEG, TIFF or PNG without an edit looks as its file does: the grid's
        // preview of it will do. A photo developed is rendered, so its current
        // edit always shows.
        if edge <= GRID_EDGE && stand_in.is_some() && !self.develops(photo) {
            return (stand_in, None);
        }
        let stamp = self.edit_stamp(ctx, photo.id);
        let (catalog, defaults) = (self.session.catalog(), &self.defaults);
        let edit = || {
            super::edit_source(catalog, photo.id)
                .or_else(|| Some(EditSource::Defaults(defaults.clone())))
        };
        match self.screen.get(photo, edge, stamp, edit) {
            Shown::Ready(texture) => (Some(texture.clone()), None),
            Shown::Loading => (stand_in, Some("Loading…".into())),
            // Said in the top bar (`poll_previews`); the grid's preview stays.
            Shown::Failed => (stand_in, None),
        }
    }
    /// The photo's edit stamp (see `Catalog::edit_stamp`), read again at
    /// most every `STAMP_AGE` seconds, so an edit saved elsewhere shows soon
    /// without reading the edit every frame.
    fn edit_stamp(&mut self, ctx: &egui::Context, id: PhotoId) -> u64 {
        let now = ctx.input(|i| i.time);
        let stamps = &mut self.stamps.0;
        match stamps.get(&id) {
            Some((stamp, read)) if now - read < STAMP_AGE => *stamp,
            _ => {
                let stamp = self.session.catalog().edit_stamp(id).unwrap_or_default();
                stamps.retain(|_, (_, read)| now - *read < STAMP_AGE);
                stamps.insert(id, (stamp, now));
                stamp
            }
        }
    }
}

/// "SELECT  DSCF0001.RAF  ★★★  ⚑  ■" under a photo.
fn caption_strip(
    painter: &egui::Painter,
    rect: Rect,
    role: Option<&str>,
    photo: &Photo,
    active: bool,
) {
    let palette = theme::palette(painter.ctx());
    let y = rect.center().y;
    let mut x = rect.left();
    if let Some(role) = role {
        let shown = painter.text(
            egui::pos2(x, y),
            egui::Align2::LEFT_CENTER,
            role.to_uppercase(),
            egui::FontId::proportional(text::SMALL),
            palette.tone(if active { Tone::SOFT } else { Tone::FAINT }),
        );
        x = shown.right() + 10.;
    }
    let name = painter.text(
        egui::pos2(x, y),
        egui::Align2::LEFT_CENTER,
        format!("{}{}", photo.filename, super::cell::copy_suffix(photo)),
        egui::FontId::proportional(text::LABEL),
        palette.tone(if active { Tone::STRONG } else { Tone::LABEL }),
    );
    let mut x = name.right() + 12.;
    if photo.rating > 0 {
        let stars = painter.text(
            egui::pos2(x, y),
            egui::Align2::LEFT_CENTER,
            "★".repeat(photo.rating.clamp(0, 5) as usize),
            egui::FontId::proportional(text::SMALL),
            palette.tone(Tone::SOFT),
        );
        x = stars.right() + 10.;
    }
    if photo.flag != 0 {
        flag_icon(painter, &palette, egui::pos2(x + 6., y), photo.flag, true);
        x += 20.;
    }
    if let Some(color) = label_color(&palette, &photo.label) {
        let chip = Rect::from_center_size(egui::pos2(x + 5., y), Vec2::splat(10.));
        painter.rect_filled(chip, radius::SMALL, color);
    }
}
