//! The Metadata filter's controls. In the bar, Metadata opens a strip under
//! it with four columns, as Lightroom's Library Filter › Metadata does, each
//! header choosing its attribute; then a button for camera, lens and keyword,
//! and for any other attribute with a choice, opens the same list. Each list
//! starts with All and its count, then one row per value with its count;
//! clicking a row adds the value or takes it out. The buttons are the
//! Library bars' 22-point buttons ([`bar_button`]), so they line up with the
//! bar's other controls.
use super::Library;
use super::facets::{Attribute, Counts};
use super::filter::toggle;
use crate::app::ui::design::{Tone, radius, size, space, text};
use crate::app::ui::icons::{self, Icon};
use crate::app::ui::theme;
use crate::app::ui::widgets::{menu_item, toolbar_action_sized};
use eframe::egui::{self, Vec2};
use std::collections::HashMap;

/// Attributes with a button in the bar, whatever is chosen.
const QUICK: [Attribute; 3] = [Attribute::Camera, Attribute::Lens, Attribute::Keyword];
/// A list longer than this has a Find field.
const FIND_FROM: usize = 12;
/// The height of the strip's lists.
const STRIP_LIST: f32 = 150.;
/// The narrowest a strip column gets; narrower, the strip scrolls sideways.
const COLUMN_WIDTH: f32 = 190.;
/// The width of a bar button's list.
const POPUP_WIDTH: f32 = 260.;

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
    /// The bar's part: Metadata, which opens and closes the strip, then a
    /// button per quick attribute and per other attribute with a choice.
    pub(super) fn metadata_buttons(&mut self, ui: &mut egui::Ui, compact: bool) -> bool {
        let open = self.metadata_bar.open;
        if bar_button(ui, "Metadata", open)
            .on_hover_text(if open {
                "Hide the metadata columns"
            } else {
                "Show the metadata columns: camera, lens, settings, file type and keyword"
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
    /// header a menu choosing its attribute, divided by hairlines. A window
    /// too narrow for them scrolls the strip sideways.
    pub(super) fn metadata_strip(&mut self, ui: &mut egui::Ui) -> bool {
        if !self.metadata_bar.open {
            return false;
        }
        self.update_metadata_counts();
        let palette = theme::palette(ui.ctx());
        let mut changed = false;
        let frame = egui::Frame::new()
            .fill(palette.tone(Tone::FIELD))
            .inner_margin(egui::Margin {
                left: space::L as i8,
                right: space::L as i8,
                top: 0,
                bottom: space::S as i8,
            })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let gap = space::XL;
                let width = ((ui.available_width() - 3. * gap) / 4.).max(COLUMN_WIDTH);
                // A solid scroll bar shows whenever the columns don't fit, so
                // it is plain that the strip scrolls; the lists in it keep the
                // app's floating ones.
                let floating = ui.spacing().scroll;
                ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
                egui::ScrollArea::horizontal()
                    .id_salt("metadata-strip")
                    .show(ui, |ui| {
                        ui.spacing_mut().scroll = floating;
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.;
                            for column in 0..4 {
                                if column > 0 {
                                    divider(ui, gap);
                                }
                                ui.vertical(|ui| {
                                    ui.set_width(width);
                                    ui.spacing_mut().item_spacing.y = 0.;
                                    self.column_header(ui, column, width);
                                    let attribute = self.metadata_bar.columns[column];
                                    changed |= self.value_list(ui, attribute, width, STRIP_LIST);
                                });
                            }
                        });
                    });
            });
        // A hairline under the strip, as under the bar.
        let bottom = frame.response.rect.bottom();
        ui.painter().hline(
            frame.response.rect.x_range(),
            bottom,
            egui::Stroke::new(1., palette.tone(Tone::OUTLINE)),
        );
        changed
    }
    /// A strip column's header: its attribute, left-aligned with a chevron,
    /// opening a menu of the attributes.
    fn column_header(&mut self, ui: &mut egui::Ui, column: usize, width: f32) {
        let palette = theme::palette(ui.ctx());
        let shown = self.metadata_bar.columns[column];
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(width, size::CONTROL), egui::Sense::click());
        let hovered = response.hovered();
        if hovered {
            ui.painter()
                .rect_filled(rect, radius::CONTROL, palette.tone(Tone::OUTLINE));
        }
        let ink = palette.tone(if hovered {
            Tone::STRONG
        } else {
            Tone::SECONDARY
        });
        let name = ui.painter().layout_no_wrap(
            shown.name().to_string(),
            egui::FontId::proportional(text::SMALL),
            ink,
        );
        let left = rect.left() + space::S;
        ui.painter().galley(
            egui::pos2(left, rect.center().y - name.size().y / 2.),
            name.clone(),
            ink,
        );
        icons::paint_at(
            ui.painter(),
            Icon::ChevronDown,
            egui::pos2(left + name.size().x + space::S + 4., rect.center().y),
            10.,
            ink,
        );
        let response = response
            .on_hover_text("Choose what this column lists")
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        egui::Popup::menu(&response).show(|ui| {
            ui.set_width(170.);
            ui.spacing_mut().item_spacing.y = 0.;
            for attribute in Attribute::ALL {
                if menu_item(ui, attribute.name(), "", true, attribute == shown) {
                    self.metadata_bar.columns[column] = attribute;
                    ui.close();
                }
            }
        });
    }
    /// A bar button: the attribute's name, or the value chosen, or the name
    /// and how many, with a chevron; it opens the attribute's list.
    fn attribute_button(&mut self, ui: &mut egui::Ui, attribute: Attribute) -> bool {
        let chosen = self.filters.metadata.get(&attribute);
        let label = match chosen.map(|c| c.iter().collect::<Vec<_>>()).as_deref() {
            None | Some([]) => attribute.name().to_string(),
            Some([one]) => one.as_deref().unwrap_or(attribute.unknown()).to_string(),
            Some(more) => format!("{} ({})", attribute.name(), more.len()),
        };
        let on = chosen.is_some_and(|c| !c.is_empty());
        let response =
            bar_button(ui, &format!("{} ▾", elided(&label, 24)), on).on_hover_text(if on {
                format!("{}: {label}", attribute.name())
            } else {
                format!("Filter by {}", attribute.name().to_lowercase())
            });
        let mut changed = false;
        egui::Popup::menu(&response)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                ui.set_width(POPUP_WIDTH);
                ui.spacing_mut().item_spacing.y = 0.;
                self.update_metadata_counts();
                changed = self.value_list(ui, attribute, POPUP_WIDTH, 280.);
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

/// A Library bar button as wide as its label: 22 points high like the bar's
/// other controls, filled when `on`.
pub(super) fn bar_button(ui: &mut egui::Ui, label: &str, on: bool) -> egui::Response {
    let text = ui.painter().layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(text::LABEL),
        egui::Color32::WHITE,
    );
    let width = text.size().x + 2. * space::M;
    toolbar_action_sized(ui, label, Vec2::new(width, size::CONTROL), on, true, 0)
}

/// A hairline between strip columns, with `gap` around it.
fn divider(ui: &mut egui::Ui, gap: f32) {
    let palette = theme::palette(ui.ctx());
    let height = size::CONTROL + STRIP_LIST + size::ROW;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(gap, height), egui::Sense::hover());
    ui.painter().vline(
        rect.center().x,
        rect.y_range().shrink(space::XS),
        egui::Stroke::new(1., palette.tone(Tone::OUTLINE)),
    );
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
