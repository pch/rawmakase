//! The Metadata filter's controls. A strip under the filter bar shows four
//! columns, as Lightroom's Library Filter › Metadata does, each header
//! choosing its attribute. The bar has a button for camera, lens and
//! keyword, and for any other attribute with a choice, that opens the same
//! list. Each list starts with All and its count, then one row per value
//! with its count; clicking a row adds the value or takes it out.
use super::Library;
use super::facets::{Attribute, Counts};
use super::filter::toggle;
use super::grid::filter_caption;
use crate::app::ui::design::{Tone, radius, size, space, text};
use crate::app::ui::theme;
use eframe::egui::{self, Vec2};
use std::collections::HashMap;

/// Attributes with a button in the bar, whatever is chosen.
const QUICK: [Attribute; 3] = [Attribute::Camera, Attribute::Lens, Attribute::Keyword];
/// A list longer than this has a Find field.
const FIND_FROM: usize = 12;
/// The height of the strip's lists.
const STRIP_LIST: f32 = 150.;

/// The strip and the lists' Find fields, kept while RAWmakase runs.
pub(super) struct MetadataBar {
    /// Whether the strip shows.
    pub open: bool,
    /// Each strip column's attribute, as its header chose it.
    columns: [Attribute; 4],
    /// What each attribute's list is narrowed to, by its Find field.
    find: HashMap<Attribute, String>,
}
impl Default for MetadataBar {
    fn default() -> Self {
        Self {
            open: false,
            columns: [
                Attribute::Camera,
                Attribute::Lens,
                Attribute::FocalLength,
                Attribute::Keyword,
            ],
            find: HashMap::new(),
        }
    }
}

impl Library {
    /// The bar's part: the strip's switch, then a button per quick attribute
    /// and per other attribute with a choice.
    pub(super) fn metadata_buttons(&mut self, ui: &mut egui::Ui, compact: bool) -> bool {
        let palette = theme::palette(ui.ctx());
        if !compact {
            ui.label(filter_caption(&palette, "Metadata"));
        }
        let open = self.metadata_bar.open;
        if ui
            .selectable_label(open, egui::RichText::new("☰").size(text::LABEL))
            .on_hover_text(if open {
                "Hide the metadata columns"
            } else {
                "Show the metadata columns, as Lightroom's Metadata filter"
            })
            .clicked()
        {
            self.metadata_bar.open = !open;
        }
        let mut changed = false;
        for attribute in Attribute::ALL {
            let chosen = self.chosen_count(attribute);
            // A narrow bar keeps only what is chosen.
            if chosen == 0 && (compact || !QUICK.contains(&attribute)) {
                continue;
            }
            changed |= self.attribute_button(ui, attribute);
        }
        changed
    }
    /// The strip under the filter bar, while it is open: four columns, each
    /// header a menu choosing its attribute.
    pub(super) fn metadata_strip(&mut self, ui: &mut egui::Ui) -> bool {
        if !self.metadata_bar.open {
            return false;
        }
        self.update_metadata_counts();
        let palette = theme::palette(ui.ctx());
        let mut changed = false;
        egui::Frame::new()
            .fill(palette.tone(Tone::BAR))
            .inner_margin(egui::Margin::symmetric(space::L as i8, space::S as i8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let gap = space::L;
                let width = ((ui.available_width() - 3. * gap) / 4.).max(120.);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for column in 0..4 {
                        ui.vertical(|ui| {
                            ui.set_width(width);
                            self.column_header(ui, column, width);
                            let attribute = self.metadata_bar.columns[column];
                            changed |= self.value_list(ui, attribute, width, STRIP_LIST);
                        });
                    }
                });
            });
        changed
    }
    fn column_header(&mut self, ui: &mut egui::Ui, column: usize, width: f32) {
        let palette = theme::palette(ui.ctx());
        let shown = &mut self.metadata_bar.columns[column];
        egui::ComboBox::from_id_salt(("metadata-column", column))
            .selected_text(filter_caption(&palette, shown.name()))
            .width(width)
            .show_ui(ui, |ui| {
                for attribute in Attribute::ALL {
                    ui.selectable_value(shown, attribute, attribute.name());
                }
            });
    }
    /// A bar button: the attribute's name, with the value chosen or how many,
    /// opening its list.
    fn attribute_button(&mut self, ui: &mut egui::Ui, attribute: Attribute) -> bool {
        let chosen = self.filters.metadata.get(&attribute);
        let label = match chosen.map(|c| c.iter().collect::<Vec<_>>()).as_deref() {
            None | Some([]) => attribute.name().to_string(),
            Some([one]) => one.as_deref().unwrap_or(attribute.unknown()).to_string(),
            Some(more) => format!("{} ({})", attribute.name(), more.len()),
        };
        let on = chosen.is_some_and(|c| !c.is_empty());
        let button = egui::Button::selectable(
            on,
            egui::RichText::new(format!("{} ▾", elided(&label, 24))).size(text::LABEL),
        );
        let response = ui.add(button).on_hover_text(if on {
            format!("{}: {label}", attribute.name())
        } else {
            format!("Filter by {}", attribute.name().to_lowercase())
        });
        let mut changed = false;
        egui::Popup::menu(&response)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                self.update_metadata_counts();
                changed = self.value_list(ui, attribute, 260., 280.);
            });
        changed
    }
    /// An attribute's list: a Find field when it is long, All with its count,
    /// then each value and its count. Only the rows in view are laid out.
    fn value_list(
        &mut self,
        ui: &mut egui::Ui,
        attribute: Attribute,
        width: f32,
        height: f32,
    ) -> bool {
        let empty = Counts::default();
        let counts = self.facets.counts(attribute).unwrap_or(&empty);
        let chosen = &mut self.filters.metadata;
        let mut changed = false;
        if counts.values.len() > FIND_FROM {
            let find = self.metadata_bar.find.entry(attribute).or_default();
            ui.add(
                egui::TextEdit::singleline(find)
                    .hint_text("Find")
                    .font(egui::FontId::proportional(text::LABEL))
                    .desired_width(width),
            );
        }
        let find = self
            .metadata_bar
            .find
            .get(&attribute)
            .map(|f| f.trim().to_lowercase())
            .filter(|f| !f.is_empty() && counts.values.len() > FIND_FROM);
        let all = chosen.get(&attribute).is_none_or(|c| c.is_empty());
        if value_row(ui, width, "All", counts.total, all).clicked() {
            chosen.remove(&attribute);
            changed = true;
        }
        let shown: Vec<&(Option<String>, usize)> = counts
            .values
            .iter()
            .filter(|(value, _)| {
                find.as_ref().is_none_or(|f| {
                    value
                        .as_deref()
                        .unwrap_or(attribute.unknown())
                        .to_lowercase()
                        .contains(f.as_str())
                })
            })
            .collect();
        egui::ScrollArea::vertical()
            .id_salt(("metadata-values", attribute, width as u32))
            .max_height(height)
            .show_rows(ui, size::ROW, shown.len(), |ui, rows| {
                ui.spacing_mut().item_spacing.y = 0.;
                for (value, count) in &shown[rows] {
                    let on = chosen.get(&attribute).is_some_and(|c| c.contains(value));
                    let name = value.as_deref().unwrap_or(attribute.unknown());
                    if value_row(ui, width, name, *count, on).clicked() {
                        let values = chosen.entry(attribute).or_default();
                        toggle(values, value.clone());
                        if values.is_empty() {
                            chosen.remove(&attribute);
                        }
                        changed = true;
                    }
                }
            });
        self.turn_on_for(changed)
    }
    fn chosen_count(&self, attribute: Attribute) -> usize {
        self.filters.metadata.get(&attribute).map_or(0, |c| c.len())
    }
    /// A choice made in the strip or a list applies the bar, as Lightroom's.
    fn turn_on_for(&mut self, changed: bool) -> bool {
        if changed {
            self.filters.enabled = true;
        }
        changed
    }
}

/// A list row: the value on one line, cut short with its full text on hover,
/// and its count aligned right; highlighted when chosen.
fn value_row(ui: &mut egui::Ui, width: f32, name: &str, count: usize, on: bool) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, size::ROW), egui::Sense::click());
    let fill = if on {
        Some(palette.tone(Tone::PRESSED))
    } else if response.hovered() {
        Some(palette.tone(Tone::HOVER))
    } else {
        None
    };
    if let Some(fill) = fill {
        ui.painter().rect_filled(rect, radius::CONTROL, fill);
    }
    let font = egui::FontId::proportional(text::LABEL);
    let count =
        ui.painter()
            .layout_no_wrap(count.to_string(), font.clone(), palette.tone(Tone::FAINT));
    let inner = rect.shrink2(Vec2::new(space::S, 0.));
    let room = (inner.width() - count.size().x - space::S).max(0.);
    let mut job = egui::text::LayoutJob::single_section(
        name.to_string(),
        egui::TextFormat::simple(font, palette.tone(if on { Tone::TEXT } else { Tone::SOFT })),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(room);
    let galley = ui.painter().layout_job(job);
    let cut = galley.elided;
    let y = |h: f32| inner.center().y - h / 2.;
    ui.painter().galley(
        egui::pos2(inner.left(), y(galley.size().y)),
        galley,
        palette.tone(Tone::SOFT),
    );
    ui.painter().galley(
        egui::pos2(inner.right() - count.size().x, y(count.size().y)),
        count,
        palette.tone(Tone::FAINT),
    );
    if cut {
        response.on_hover_text(name)
    } else {
        response
    }
}

/// `text` cut to `most` characters, with an ellipsis when cut.
fn elided(text: &str, most: usize) -> String {
    if text.chars().count() <= most {
        return text.to_string();
    }
    let kept: String = text.chars().take(most - 1).collect();
    format!("{kept}…")
}
