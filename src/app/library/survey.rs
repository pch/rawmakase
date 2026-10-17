//! Lightroom's Survey (N): every selected photo at once, as large as the
//! view allows, to narrow a set down. The photos are the selection itself:
//! a click makes one active (rating, flag and label keys go to it), the
//! arrows move between them, and a photo's × or Cmd+click takes it out.
use super::grid::filter_caption;
use super::stage::MARGIN;
use super::{Action, Library};
use crate::app::theme;
use crate::catalog::PhotoId;
use eframe::egui::{self, Rect, Vec2};

/// Photos shown at most, keeping previews and memory bounded however many
/// are selected; Lightroom's tiles are too small to judge long before this.
const MOST: usize = 48;
/// The shape photos are assumed to have until their previews are in: 3:2.
const ASPECT: f32 = 1.5;

#[derive(Debug, Default)]
pub(super) struct Survey {
    pub open: bool,
}

impl Library {
    #[cfg(test)]
    pub(crate) fn survey_open(&self) -> bool {
        self.survey.open
    }
    /// N: the selected photos side by side; the active one alone if it is
    /// the only one.
    pub(crate) fn open_survey(&mut self) {
        if self.selection.active.is_none() {
            self.select(self.visible.first().map(|i| self.session.photos[*i].id));
        }
        if self.selection.active.is_none() {
            return;
        }
        self.close_loupe();
        self.close_compare();
        self.survey.open = true;
    }
    /// Esc or G: back to the grid, the selection as Survey left it.
    pub(crate) fn close_survey(&mut self) {
        if self.survey.open {
            self.survey.open = false;
            self.scroll_to_active = true;
        }
    }
    /// The photos surveyed, in display order.
    pub(super) fn surveyed(&self) -> Vec<PhotoId> {
        self.selected_ids()
    }
    /// Left/Up and Right/Down: the photo before or after becomes active.
    /// The photos surveyed that are shown: all of them, or the `MOST` up to
    /// and including the active photo.
    pub(super) fn shown_surveyed(&self) -> Vec<PhotoId> {
        let ids = self.surveyed();
        let at = self
            .selection
            .active
            .and_then(|id| ids.iter().position(|i| *i == id))
            .unwrap_or(0);
        let start = (at + 1).saturating_sub(MOST);
        ids.into_iter().skip(start).take(MOST).collect()
    }
    pub(super) fn step_surveyed(&mut self, by: isize) {
        let ids = self.surveyed();
        let Some(at) = self
            .selection
            .active
            .and_then(|id| ids.iter().position(|i| *i == id))
        else {
            return;
        };
        let to = (at as isize + by).clamp(0, ids.len() as isize - 1) as usize;
        self.make_active(ids[to]);
    }
    /// × or Cmd+click: takes a photo out of the survey.
    pub(super) fn drop_surveyed(&mut self, id: PhotoId) {
        if self.selection.selected.len() > 1 {
            self.click(id, egui::Modifiers::COMMAND);
        }
    }
    /// A rating, flag or label key in Survey: the active photo only. A
    /// photo the filter now hides leaves the survey and the next one
    /// surveyed becomes active; with Shift, the next one does anyway.
    pub(crate) fn edit_surveyed(
        &mut self,
        edit: crate::app::photo_metadata::Edit,
        advance: bool,
    ) -> anyhow::Result<()> {
        let Some(id) = self.selection.active else {
            return Ok(());
        };
        let surveyed = self.surveyed();
        let recorded = self.done.len();
        self.edit_metadata(id, edit, false)?;
        let shown = |library: &Self, id: &PhotoId| {
            library
                .visible
                .iter()
                .any(|i| library.session.photos[*i].id == *id)
        };
        let at = surveyed.iter().position(|i| *i == id).unwrap_or(0);
        let active = surveyed[at..]
            .iter()
            .chain(surveyed[..at].iter().rev())
            .find(|i| shown(self, i))
            .copied();
        self.selection.selected = surveyed.into_iter().filter(|i| shown(self, i)).collect();
        self.selection.active = active;
        self.selection.anchor = active;
        if advance && active == Some(id) {
            self.step_surveyed(1);
        }
        // Undo and redo return to the survey as the edit left it.
        let place = self.place();
        if self.done.len() > recorded
            && let Some(command) = self.done.last_mut()
        {
            command.place_after = place;
        }
        Ok(())
    }
    pub(super) fn survey_keys(&mut self, presses: &[super::selection::Press]) {
        use egui::Key;
        for press in presses {
            if press.key == Key::B {
                if !press.repeat {
                    let ids = self.selection.active.into_iter().collect();
                    self.quick_key(press.modifiers, ids)
                }
                continue;
            }
            if press.modifiers.command || press.modifiers.alt {
                continue;
            }
            match press.key {
                Key::ArrowLeft | Key::ArrowUp => self.step_surveyed(-1),
                Key::ArrowRight | Key::ArrowDown => self.step_surveyed(1),
                Key::Escape => self.close_survey(),
                Key::E | Key::Enter => self.open_loupe(),
                Key::C if !press.modifiers.any() => self.open_compare(),
                _ => {}
            }
        }
    }
    /// Survey in place of the grid: the photos tiled, with a toolbar.
    pub(super) fn survey(&mut self, ui: &mut egui::Ui, chrome: bool) -> Action {
        let palette = theme::palette(ui.ctx());
        if self.selection.active.is_none() {
            self.close_survey();
            return Action::None;
        }
        // On the photo's backdrop, as the view above it.
        if chrome {
            egui::Panel::bottom("library-survey-toolbar")
                .frame(
                    egui::Frame::new()
                        .fill(palette.photo_backdrop())
                        .inner_margin(egui::Margin::symmetric(10, 4)),
                )
                .show_separator_line(false)
                .show(ui, |ui| self.survey_toolbar(ui));
        }
        let area = ui.available_rect_before_wrap();
        ui.allocate_rect(area, egui::Sense::hover());
        ui.painter().rect_filled(area, 0., palette.photo_backdrop());
        let ids = self.shown_surveyed();
        // The previews of every photo shown stay while they are shown.
        self.cache.shown_at_once = ids.len();
        for (id, rect) in ids
            .iter()
            .zip(tiles(area.shrink(MARGIN), ids.len(), self.aspect(&ids)))
        {
            self.survey_tile(ui, *id, rect);
        }
        Action::None
    }
    /// The photos' usual shape, width over height, from their previews; 3:2
    /// until those are in.
    fn aspect(&self, ids: &[PhotoId]) -> f32 {
        let shapes: Vec<f32> = ids
            .iter()
            .filter_map(|id| self.texture(self.photo(*id)?))
            .map(|t| t.size_vec2())
            .filter(|size| size.y > 0.)
            .map(|size| size.x / size.y)
            .collect();
        if shapes.is_empty() {
            ASPECT
        } else {
            shapes.iter().sum::<f32>() / shapes.len() as f32
        }
    }
    fn survey_toolbar(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(ui.ctx());
        ui.horizontal(|ui| {
            self.view_buttons(ui);
            ui.add_space(12.);
            let count = self.surveyed().len();
            ui.label(filter_caption(
                &palette,
                &match count {
                    1 => "1 photo".to_string(),
                    n if n > MOST => format!("{MOST} of {n} photos selected, up to the active one"),
                    n => format!("{n} photos"),
                },
            ));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let done = ui
                    .add(egui::Button::new(egui::RichText::new("Done").size(11.)).small())
                    .on_hover_text("Back to the grid (Esc)");
                if done.clicked() {
                    self.close_survey();
                }
                ui.label(filter_caption(
                    &palette,
                    "Click to make active · × or Cmd+click to take out",
                ));
            });
        });
    }
    /// One surveyed photo in `rect`: a click makes it active, a double-click
    /// opens it in the Loupe, Cmd+click or its × takes it out.
    fn survey_tile(&mut self, ui: &mut egui::Ui, id: PhotoId, rect: Rect) {
        let palette = theme::palette(ui.ctx());
        let Some(photo) = self.photo(id).cloned() else {
            return;
        };
        let response = ui.interact(
            rect,
            egui::Id::new(("library-survey", id)),
            egui::Sense::click(),
        );
        let active = self.selection.active == Some(id);
        let shown = self.stage_photo(ui, rect, &photo, None, active);
        // The × shows on the hovered photo while more than one is surveyed.
        let mut dropped = false;
        if let Some(at) = shown
            && response.hovered()
            && self.selection.selected.len() > 1
        {
            let close =
                Rect::from_center_size(at.right_top() + Vec2::new(-14., 14.), Vec2::splat(20.));
            let button = ui.interact(
                close,
                egui::Id::new(("library-survey-drop", id)),
                egui::Sense::click(),
            );
            ui.painter().circle_filled(
                close.center(),
                10.,
                palette.gray(if button.hovered() { 70 } else { 30 }),
            );
            crate::app::icons::paint_at(
                ui.painter(),
                crate::app::icons::Icon::Close,
                close.center(),
                12.,
                palette.gray(230),
            );
            if button.on_hover_text("Take out of the survey").clicked() {
                self.drop_surveyed(id);
                dropped = true;
            }
        }
        if dropped {
            return;
        }
        if response.double_clicked() {
            self.make_active(id);
            self.open_loupe();
        } else if response.clicked() {
            if ui.input(|i| i.modifiers.command) {
                self.drop_surveyed(id);
            } else {
                self.make_active(id);
            }
        }
    }
}

/// `n` tiles filling `area`, in rows, as large as photos of `aspect`
/// (width over height) can be: the column count is the one that makes them
/// largest. The gaps shrink when there are too many photos for them.
fn tiles(area: Rect, n: usize, aspect: f32) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    let layout = |columns: usize| {
        let rows = n.div_ceil(columns);
        let gap = Vec2::new(
            MARGIN.min(area.width() / columns as f32 / 4.),
            MARGIN.min(area.height() / rows as f32 / 4.),
        );
        let cell = Vec2::new(
            (area.width() - gap.x * (columns - 1) as f32) / columns as f32,
            (area.height() - gap.y * (rows - 1) as f32) / rows as f32,
        );
        // The photo's area once fitted to the cell.
        let width = cell.x.min(cell.y * aspect);
        (width * width / aspect, cell, gap)
    };
    let columns = (1..=n)
        .max_by(|a, b| layout(*a).0.total_cmp(&layout(*b).0))
        .unwrap_or(1);
    let (_, cell, gap) = layout(columns);
    (0..n)
        .map(|i| {
            let (row, column) = (i / columns, i % columns);
            let min = area.min
                + Vec2::new(
                    column as f32 * (cell.x + gap.x),
                    row as f32 * (cell.y + gap.y),
                );
            Rect::from_min_size(min, cell)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_are_as_large_as_the_view_allows() {
        let area = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1214., 614.));
        assert!(tiles(area, 0, ASPECT).is_empty());
        assert_eq!(tiles(area, 1, ASPECT), [area]);
        // Two photos side by side, four in two rows.
        let two = tiles(area, 2, ASPECT);
        assert_eq!(two[1].min, egui::pos2(614., 0.));
        let four = tiles(area, 4, ASPECT);
        assert_eq!(four[2].min, egui::pos2(0., four[0].max.y + MARGIN));
        // Three portraits side by side.
        assert_eq!(tiles(area, 3, 2. / 3.)[2].min.y, 0.);
        // However many, all alike, valid and within the view.
        for n in (1..=24).chain([500, 5000]) {
            let all = tiles(area, n, ASPECT);
            assert!(all.iter().all(|t| t.width() > 0. && t.height() > 0.));
            assert!(all.iter().all(|t| area.expand(0.01).contains_rect(*t)));
            assert!(
                all.iter()
                    .all(|t| (t.size() - all[0].size()).length() < 0.01)
            );
        }
    }
}
