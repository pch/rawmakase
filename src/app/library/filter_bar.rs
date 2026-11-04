//! The filter bar above the grid: search, then Lightroom's Attribute
//! filter. Flags and labels toggle in any combination, the rating is
//! compared with the operator chosen from its menu, and Kind picks masters
//! or virtual copies. Cmd+L turns it off and on; changing it turns it on.
use super::Library;
use super::filter::{Kind, Label, RatingOp, toggle};
use super::grid::filter_caption;
use crate::app::library::photo_metadata::{LABELS, flag_icon, label_color};
use crate::app::shell::shortcuts::keys_text;
use crate::app::ui::design::{Tone, inset, radius, size, space, text};
use crate::app::ui::theme;
use crate::app::ui::widgets::{COMPACT_SEGMENT_HEIGHT, segmented};
use eframe::egui::{self, Rect, Vec2};

/// The height of the bar's toggles and stars.
const TOGGLE_HEIGHT: f32 = size::ROW;

impl Library {
    pub(super) fn filter_bar(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        egui::Frame::new()
            .fill(theme::palette(ui.ctx()).tone(Tone::FIELD))
            .inner_margin(inset::BAR)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                // Each control wraps as a whole onto another row when the
                // window is narrow.
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = space::XS;
                    ui.spacing_mut().interact_size.y = TOGGLE_HEIGHT;
                    let compact = ui.available_width() < 640.;
                    let opacity = ui.opacity();
                    if !self.filters.enabled {
                        // Off with Cmd+L: kept, but shown as not applying.
                        ui.multiply_opacity(0.45);
                    }
                    changed |= ui.horizontal(|ui| self.search_field(ui, compact)).inner;
                    ui.add_space(space::L);
                    changed |= ui.horizontal(|ui| self.flag_filter(ui, compact)).inner;
                    ui.add_space(space::L);
                    changed |= ui.horizontal(|ui| self.rating_filter(ui, compact)).inner;
                    ui.add_space(space::L);
                    changed |= ui.horizontal(|ui| self.label_filter(ui, compact)).inner;
                    if self.has_copies() || self.filters.kind != Kind::All {
                        ui.add_space(space::L);
                        changed |= ui.horizontal(|ui| self.kind_filter(ui, compact)).inner;
                    }
                    ui.add_space(space::L);
                    ui.set_opacity(opacity);
                    changed |= self.filters_switch(ui);
                });
            });
        if changed {
            self.filter()
        }
    }
    /// Cmd+L: turns the filter bar off or back on, keeping what is set.
    pub(super) fn toggle_filters(&mut self) {
        self.filters.enabled = !self.filters.enabled;
        self.filter();
    }
    fn search_field(&mut self, ui: &mut egui::Ui, compact: bool) -> bool {
        let changed = ui
            .add(
                egui::TextEdit::singleline(&mut self.filters.query)
                    .hint_text("Search")
                    .font(egui::FontId::proportional(text::LABEL))
                    .desired_width(if compact { 120. } else { 190. })
                    // As tall as the segmented controls in the bar.
                    .min_size(Vec2::new(0., COMPACT_SEGMENT_HEIGHT))
                    .vertical_align(egui::Align::Center),
            )
            .on_hover_text("Search filename, keyword, capture date or label")
            .changed();
        self.turn_on(changed)
    }
    fn flag_filter(&mut self, ui: &mut egui::Ui, compact: bool) -> bool {
        let palette = theme::palette(ui.ctx());
        if !compact {
            ui.label(filter_caption(&palette, "Flag"));
        }
        let mut changed = false;
        for (flag, name) in [(1, "Picked"), (0, "Unflagged"), (-1, "Rejected")] {
            let on = self.filters.flags.contains(&flag);
            let (rect, response) = toggle_cell(ui, 22., on);
            flag_icon(ui.painter(), &palette, rect.center(), flag, on);
            if response.on_hover_text(name).clicked() {
                toggle(&mut self.filters.flags, flag);
                changed = true;
            }
        }
        self.turn_on(changed)
    }
    fn rating_filter(&mut self, ui: &mut egui::Ui, compact: bool) -> bool {
        let palette = theme::palette(ui.ctx());
        if !compact {
            ui.label(filter_caption(&palette, "Rating"));
        }
        let mut changed = false;
        let filters = &mut self.filters;
        let unrated = filters.rating == Some(0);
        let shown = if unrated {
            "Unrated"
        } else {
            filters.rating_op.symbol()
        };
        ui.menu_button(egui::RichText::new(shown).size(text::LABEL), |ui| {
            for op in RatingOp::ALL {
                let on = !unrated && filters.rating_op == op;
                if ui.selectable_label(on, op.name()).clicked() {
                    filters.rating_op = op;
                    if unrated {
                        filters.rating = None;
                    }
                    changed = true;
                }
            }
            ui.separator();
            if ui
                .selectable_label(unrated, "Unrated photos only")
                .clicked()
            {
                filters.rating = Some(0);
                filters.rating_op = RatingOp::Exactly;
                changed = true;
            }
        })
        .response
        .on_hover_text("How the rating compares");
        for star in 1..=5 {
            let response = star_toggle(ui, filters.rating.is_some_and(|r| r >= star));
            let hover = format!(
                "Rating {} {star} · click again to clear",
                filters.rating_op.symbol()
            );
            if response.on_hover_text(hover).clicked() {
                filters.rating = (filters.rating != Some(star)).then_some(star);
                changed = true;
            }
        }
        self.turn_on(changed)
    }
    fn label_filter(&mut self, ui: &mut egui::Ui, compact: bool) -> bool {
        let palette = theme::palette(ui.ctx());
        if !compact {
            ui.label(filter_caption(&palette, "Color"));
        }
        let mut chips: Vec<(Label, &str)> = LABELS
            .iter()
            .map(|l| (Label::Color((*l).into()), *l))
            .collect();
        chips.push((Label::None, "No label"));
        if self.has_other_labels() || self.filters.labels.contains(&Label::Other) {
            chips.push((Label::Other, "Other labels"));
        }
        let mut changed = false;
        for (label, name) in chips {
            let on = self.filters.labels.contains(&label);
            let (rect, response) = toggle_cell(ui, 17., on);
            let chip = Rect::from_center_size(rect.center(), Vec2::splat(10.));
            let painter = ui.painter();
            match &label {
                Label::Color(color) => {
                    painter.rect_filled(
                        chip,
                        radius::SMALL,
                        label_color(&palette, color).unwrap_or_default(),
                    );
                }
                Label::None => {
                    let stroke = egui::Stroke::new(1., palette.tone(Tone::SECONDARY));
                    painter.rect_stroke(chip, radius::SMALL, stroke, egui::StrokeKind::Inside);
                }
                Label::Other => {
                    painter.rect_filled(chip, radius::SMALL, palette.tone(Tone::SECONDARY));
                }
            }
            if response.on_hover_text(name).clicked() {
                toggle(&mut self.filters.labels, label);
                changed = true;
            }
        }
        self.turn_on(changed)
    }
    fn kind_filter(&mut self, ui: &mut egui::Ui, compact: bool) -> bool {
        let palette = theme::palette(ui.ctx());
        if !compact {
            ui.label(filter_caption(&palette, "Kind"));
        }
        let changed = segmented(
            ui,
            &mut self.filters.kind,
            &[
                (Kind::All, "All"),
                (Kind::Masters, "Masters"),
                (Kind::Copies, "Copies"),
            ],
            if compact { 150. } else { 180. },
        );
        self.turn_on(changed)
    }
    /// Clear when the bar applies; when Cmd+L turned it off, a way back on.
    fn filters_switch(&mut self, ui: &mut egui::Ui) -> bool {
        let palette = theme::palette(ui.ctx());
        let toggle_keys = keys_text("Cmd+L");
        if !self.filters.enabled {
            let clicked = ui
                .add(egui::Button::new(filter_caption(&palette, "Filters Off")).small())
                .on_hover_text(format!("Turn the filters back on ({toggle_keys})"))
                .clicked();
            if clicked {
                self.filters.enabled = true;
            }
            return clicked;
        }
        if !self.filters.bar_set() {
            return false;
        }
        let clicked = ui
            .add(
                egui::Button::new(filter_caption(&palette, "Clear"))
                    .small()
                    .frame(false),
            )
            .on_hover_text(format!(
                "Clear every filter in the bar · {toggle_keys} turns them off for now"
            ))
            .clicked();
        if clicked {
            self.filters.clear_bar();
        }
        clicked
    }
    /// A change in the bar applies it, as in Lightroom.
    fn turn_on(&mut self, changed: bool) -> bool {
        if changed {
            self.filters.enabled = true;
        }
        changed
    }
    fn has_copies(&self) -> bool {
        self.session.photos().iter().any(|p| p.master.is_some())
    }
    fn has_other_labels(&self) -> bool {
        self.session
            .photos()
            .iter()
            .any(|p| !p.label.is_empty() && !LABELS.contains(&p.label.as_str()))
    }
}

/// A star in the rating filter, lit up to the rating chosen.
fn star_toggle(ui: &mut egui::Ui, lit: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(13., TOGGLE_HEIGHT), egui::Sense::click());
    let level = if lit {
        Tone::TEXT
    } else if response.hovered() {
        Tone::MUTED
    } else {
        Tone::OUTLINE_STRONG
    };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "★",
        egui::FontId::proportional(text::LABEL),
        theme::palette(ui.ctx()).tone(level),
    );
    response
}

/// A square toggle `width` wide, its background showing whether it is on.
fn toggle_cell(ui: &mut egui::Ui, width: f32, on: bool) -> (Rect, egui::Response) {
    let palette = theme::palette(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, TOGGLE_HEIGHT), egui::Sense::click());
    let fill = if on {
        Some(palette.tone(Tone::OUTLINE_STRONG))
    } else if response.hovered() {
        Some(palette.tone(Tone::OUTLINE))
    } else {
        None
    };
    if let Some(fill) = fill {
        ui.painter().rect_filled(rect, radius::CONTROL, fill);
    }
    (rect, response)
}
