//! The grid, with the filter bar above it and its toolbar below.
use super::cell::{self, photo_cell};
use super::{Action, Library};
use crate::app::theme;
use crate::catalog::{Photo, PhotoId};
use eframe::egui::{self, Vec2};

impl Library {
    pub(super) fn grid_toolbar(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(ui.ctx());
        egui::Frame::new()
            .fill(palette.gray(38))
            .inner_margin(egui::Margin::symmetric(10, 4))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    self.view_buttons(ui);
                    ui.add_space(12.);
                    self.sort_menu(ui);
                    ui.add_space(12.);
                    self.cell_style_menu(ui);
                    ui.add_space(12.);
                    ui.small(if self.visible.len() == self.session.photos.len() {
                        format!("{} photos", self.session.photos.len())
                    } else {
                        format!(
                            "{} of {} photos",
                            self.visible.len(),
                            self.session.photos.len()
                        )
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().slider_width = 110.;
                        ui.add(
                            egui::Slider::new(&mut self.thumb_size, 110. ..=360.).show_value(false),
                        );
                        ui.label(filter_caption(&palette, "Thumbnails"));
                    });
                });
            });
    }
    /// Brings the photos up to date before the filmstrip and the view draw
    /// them, so both show the same order and the same active photo; with the
    /// filmstrip hidden, before the view alone.
    pub(crate) fn prepare(&mut self, ctx: &egui::Context) {
        self.poll_previews(ctx);
        // Back from Develop, sorted by edit time: edits made there move
        // their photos.
        let pass = ctx.cumulative_pass_nr();
        let returned = std::mem::replace(&mut self.drawn_pass, pass) + 1 < pass;
        if returned && self.filters.sort == super::sort::Sort::EditTime {
            self.sort_keys = None;
            self.filter();
            self.scroll_to_active = true;
        }
        // Survey keeps every photo's preview it shows; other views the usual
        // number.
        self.cache.shown_at_once = 0;
        // Compare settles its pair, so the strip highlights the photo it shows.
        if self.compare.open {
            if self.keep_compared_shown().is_some() {
                self.sync_compare_selection();
            } else {
                self.close_compare();
            }
        }
    }
    /// The grid, or the Loupe in its place at the shared `zoom`.
    /// The grid, or the Loupe in its place at the shared `zoom`. `chrome` is
    /// whether the toolbars are drawn: hidden, the view fills the window.
    pub(in crate::app) fn grid(
        &mut self,
        ui: &mut egui::Ui,
        zoom: &mut crate::app::navigator::Zoom,
        chrome: bool,
    ) -> Action {
        if self.compare.open {
            return self.compare(ui, chrome);
        }
        if self.survey.open {
            return self.survey(ui, chrome);
        }
        if self.loupe.open {
            return self.loupe(ui, zoom, chrome);
        }
        let mut action = Action::None;
        if chrome {
            self.filter_bar(ui);
            // On the photo's backdrop, as the grid above it.
            egui::Panel::bottom("library-grid-toolbar")
                .frame(egui::Frame::new().fill(theme::palette(ui.ctx()).photo_backdrop()))
                .show_separator_line(false)
                .show(ui, |ui| self.grid_toolbar(ui));
        }
        // Inset on the photo's backdrop, as the Loupe's photo is.
        let margin = crate::app::workspace::PHOTO_MARGIN;
        let inner = ui.available_width() - 2. * margin;
        let columns = ((inner / self.thumb_size).floor() as usize).max(1);
        let width = (inner / columns as f32).floor().max(80.);
        let height = self.cell_style.height(width);
        // A new cell style keeps the same row at the top of the view.
        let drawn = std::mem::replace(&mut self.drawn_style, self.cell_style);
        self.grid_columns = columns;
        let mut metadata_edit = None;
        let spacing = ui.spacing().item_spacing;
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("library-grid")
            .auto_shrink(false);
        // A re-sort moved the selected photo: scroll by the rows it moved.
        if drawn != self.cell_style {
            self.grid_kinetic.stop();
            let row = self.grid_offset / drawn.height(width);
            scroll = scroll.vertical_scroll_offset(row * height);
        }
        if let Some((id, before)) = self.keep_in_place.take()
            && let Some(after) = self
                .visible
                .iter()
                .position(|i| self.session.photos[*i].id == id)
        {
            let rows = (after / columns) as f32 - (before / columns) as f32;
            self.grid_kinetic.stop();
            scroll = scroll.vertical_scroll_offset((self.grid_offset + rows * height).max(0.));
        }
        egui::Frame::new()
            .fill(theme::palette(ui.ctx()).photo_backdrop())
            .inner_margin(egui::Margin::same(margin as i8))
            .show(ui, |ui| {
                // A key moved the active photo: bring its row into view.
                if std::mem::take(&mut self.scroll_to_active)
                    && let Some(at) = self.selection.active.and_then(|id| {
                        self.visible
                            .iter()
                            .position(|i| self.session.photos[*i].id == id)
                    })
                {
                    self.grid_kinetic.stop();
                    let top = (at / columns) as f32 * height;
                    let view = ui.available_height();
                    if top < self.grid_offset {
                        scroll = scroll.vertical_scroll_offset(top);
                    } else if top + height > self.grid_offset + view {
                        scroll = scroll.vertical_scroll_offset(top + height - view);
                    }
                }
                let glide = self.grid_kinetic.begin(ui);
                let output = scroll.show_rows(
                    ui,
                    height,
                    self.visible.len().div_ceil(columns),
                    |ui, rows| {
                        self.grid_shown = rows.start * columns..rows.end * columns;
                        for row in rows {
                            ui.horizontal(|ui| {
                                for col in 0..columns {
                                    let Some(&index) = self.visible.get(row * columns + col) else {
                                        break;
                                    };
                                    let p = self.session.photos[index].clone();
                                    let exists = self.is_available(&p.path);
                                    self.request_previews(&p, ui.ctx());
                                    let shown = cell::Shown {
                                        mark: self.mark(p.id),
                                        number: row * columns + col + 1,
                                        available: exists,
                                        quick: self.in_quick(p.id),
                                        style: self.cell_style,
                                        details: if self.cell_style == cell::Style::Expanded {
                                            self.cell_details(&p)
                                        } else {
                                            String::new()
                                        },
                                    };
                                    let (response, edit) =
                                        photo_cell(ui, &p, self.texture(&p), shown, width);
                                    let response = if response.hovered() {
                                        let text = self.hover_text(&p);
                                        response.on_hover_text(text)
                                    } else {
                                        response
                                    };
                                    if response.clicked() {
                                        self.click(p.id, ui.input(|i| i.modifiers));
                                    } else if response.secondary_clicked() {
                                        // A menu on a selected photo acts on the selection.
                                        self.make_active(p.id);
                                    }
                                    // As in Lightroom, a double-click opens the Loupe.
                                    if response.double_clicked() {
                                        self.make_active(p.id);
                                        self.open_loupe();
                                    }
                                    if let Some(edit) = edit {
                                        metadata_edit = Some((p.clone(), edit));
                                    }
                                }
                            });
                        }
                    },
                );
                self.grid_kinetic.end(ui, glide);
                self.grid_offset = output.state.offset.y;
            });
        ui.spacing_mut().item_spacing = spacing;
        if let Some((photo, menu)) = metadata_edit
            && let Some(id) = self.photo_action(ui.ctx(), &photo, menu, true)
        {
            action = Action::Develop(id);
        }
        action
    }
    /// Carries out a thumbnail menu choice; returns a photo to open in Develop.
    /// A metadata change covers the selection when `whole_selection` (the
    /// grid) and the photo is in it, else that photo alone (the filmstrip).
    pub(super) fn photo_action(
        &mut self,
        ctx: &egui::Context,
        photo: &Photo,
        action: cell::PhotoAction,
        whole_selection: bool,
    ) -> Option<PhotoId> {
        use cell::PhotoAction;
        match action {
            PhotoAction::Develop => return Some(photo.id),
            // Taken by Develop's filmstrip before it gets here.
            PhotoAction::SetReference => {}
            PhotoAction::Reveal => {
                if let Err(e) = crate::platform::reveal::reveal(&photo.path) {
                    self.message = format!("Could not show {}: {e}", photo.filename);
                }
            }
            PhotoAction::CopyPath => {
                ctx.copy_text(photo.path.display().to_string());
                self.message = format!("Copied {}", photo.path.display());
            }
            PhotoAction::Edit(edit) => {
                let ids = if whole_selection && self.selection.selected.contains(&photo.id) {
                    self.selected_ids()
                } else {
                    vec![photo.id]
                };
                if let Err(e) = self.edit_photos(&ids, edit, false) {
                    self.message = format!("Metadata could not be saved: {e}");
                }
            }
            PhotoAction::Copy(copy) => self.copy_request = Some(copy),
            // The selection when the photo is in it, in the grid and the
            // filmstrip alike, as Export.
            PhotoAction::BuildPreviews(kind) => {
                self.previews_request = Some(super::PreviewsRequest::Build(
                    self.menu_scope(photo.id),
                    kind,
                ));
            }
            PhotoAction::DiscardPreviews(kinds) => {
                self.previews_request = Some(super::PreviewsRequest::Discard(
                    self.menu_scope(photo.id),
                    kinds,
                ));
            }
            PhotoAction::ReadMetadata => {
                let ids = if whole_selection && self.selection.selected.contains(&photo.id) {
                    self.selected_ids()
                } else {
                    vec![photo.id]
                };
                self.read_request = Some(ids);
            }
        }
        None
    }
}
impl Library {
    /// The selection when `id` is part of it, else `id` alone.
    fn menu_scope(&self, id: PhotoId) -> Vec<PhotoId> {
        if self.selection.selected.contains(&id) {
            self.selected_ids()
        } else {
            vec![id]
        }
    }
    /// Lightroom's Sort: the order from a menu, its direction from the arrow.
    fn sort_menu(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(ui.ctx());
        ui.label(filter_caption(&palette, "Sort"));
        let mut changed = false;
        ui.menu_button(
            egui::RichText::new(self.filters.sort.name()).size(11.),
            |ui| {
                for sort in super::sort::Sort::ALL {
                    if ui
                        .selectable_label(self.filters.sort == sort, sort.name())
                        .clicked()
                    {
                        self.filters.sort = sort;
                        changed = true;
                        ui.close();
                    }
                }
            },
        )
        .response
        .on_hover_text("Sort the photos by");
        let arrow = if self.filters.reverse { "↓" } else { "↑" };
        if ui
            .add(
                egui::Button::new(egui::RichText::new(arrow).size(11.))
                    .small()
                    .frame(false),
            )
            .on_hover_text("Reverse the sort order")
            .clicked()
        {
            self.filters.reverse = !self.filters.reverse;
            changed = true;
        }
        if changed {
            self.filter();
            self.scroll_to_active = true;
        }
    }
    /// The grid cell style, from a menu as well as J.
    fn cell_style_menu(&mut self, ui: &mut egui::Ui) {
        let keys = crate::app::shortcuts::keys_text("J");
        ui.menu_button(
            egui::RichText::new(self.cell_style.name())
                .size(11.)
                .color(theme::palette(ui.ctx()).gray(150)),
            |ui| {
                for style in cell::Style::ALL {
                    if ui
                        .selectable_label(self.cell_style == style, style.name())
                        .clicked()
                    {
                        self.cell_style = style;
                        ui.close();
                    }
                }
            },
        )
        .response
        .on_hover_text(format!("How grid cells show their photos ({keys} cycles)"));
    }
}
pub(super) fn filter_caption(palette: &theme::Palette, text: &str) -> egui::RichText {
    egui::RichText::new(text).size(11.).color(palette.gray(150))
}
