//! The Library's right panel: Quick Develop and the selected photo's metadata.
use super::rows::{ROW, caption_at, paint_truncated, value_at};
use super::{Action, Library};
use crate::app::ui::design::{Tone, space};
use crate::app::ui::theme;
use crate::app::ui::widgets::section;
use crate::catalog::{Photo, PhotoId};
use crate::metadata::PhotoInfo;
use eframe::egui::{self, Vec2};

impl Library {
    /// Right panel: the selected photo's rating, flag, label and file details,
    /// with `quick` (the batch edits of the selection) in Quick Develop. The layout is identical with
    /// or without a selection, so nothing moves.
    pub(crate) fn info_panel(
        &mut self,
        ui: &mut egui::Ui,
        quick: impl FnOnce(&mut egui::Ui),
    ) -> Action {
        let mut action = Action::None;
        ui.spacing_mut().item_spacing.y = 0.;
        let photo = self.selected().and_then(|id| self.photo(id)).cloned();
        let info = self.active_info();
        egui::ScrollArea::vertical()
            .id_salt("library-info")
            .auto_shrink(false)
            .show(ui, |ui| {
                section(ui, "Quick Develop", |ui| {
                    let refusal = photo.as_ref().and_then(|p| self.shown_refusal(p));
                    let open = ui
                        .add_enabled(
                            photo.is_some() && refusal.is_none(),
                            egui::Button::new("Open in Develop")
                                .min_size(Vec2::new(ui.available_width(), 24.)),
                        )
                        .on_hover_text("Develop · D")
                        .on_disabled_hover_text(
                            refusal
                                .as_ref()
                                .map(super::Refusal::detail)
                                .unwrap_or_default(),
                        );
                    if open.clicked()
                        && let Some(p) = &photo
                    {
                        action = Action::Develop(p.id);
                    }
                    ui.add_space(space::XS);
                    // Why the button is greyed out, in the line kept for this.
                    let note = match &refusal {
                        Some(super::Refusal::Offline) => {
                            "Offline: relink its folder to edit".into()
                        }
                        Some(super::Refusal::EditablePreview(_)) => {
                            "Offline: its editable preview can't be used".into()
                        }
                        None if photo.as_ref().is_some_and(|p| {
                            self.has_editable_preview(p.id) && !self.is_available(&p.path)
                        }) =>
                        {
                            "Offline: edits its editable preview".into()
                        }
                        Some(super::Refusal::Unsupported { reason, .. }) => {
                            format!("{reason} images can't be edited in Develop")
                        }
                        None if photo.as_ref().is_some_and(|p| p.has_lightroom_edits) => {
                            "Has Lightroom edits".into()
                        }
                        None => String::new(),
                    };
                    info_text(ui, &note);
                    quick(ui);
                    ui.add_space(space::XS);
                });
                section(ui, "Metadata", |ui| {
                    match &photo {
                        Some(p) => {
                            // In Loupe and Compare, as for their keys, only the
                            // active photo changes.
                            self.metadata_controls(ui, p.id, !self.edits_active_only());
                        }
                        None => {
                            ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 22.),
                                egui::Sense::hover(),
                            );
                        }
                    }
                    ui.add_space(space::S);
                    let folder = photo
                        .as_ref()
                        .and_then(|p| p.path.parent()?.file_name())
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let field = |f: fn(&Photo) -> &str| photo.as_ref().map_or("", f).to_string();
                    // Camera settings and size, as Lightroom's EXIF metadata shows them.
                    let info = info.clone().unwrap_or_default();
                    let text = |f: fn(&PhotoInfo) -> Option<String>| f(&info).unwrap_or_default();
                    metadata_row(ui, "File Name", &field(|p| &p.filename));
                    match photo.as_ref().filter(|p| p.master.is_some()) {
                        Some(p) => match self.copy_names.row(ui, p, &mut self.session) {
                            Ok(true) => self.filter(),
                            Ok(false) => {}
                            Err(e) => self
                                .messages
                                .fail(format!("Copy name could not be saved: {e}")),
                        },
                        None => {
                            metadata_row(ui, "Copy Name", "");
                        }
                    }
                    self.metadata_fields(ui);
                    for (key, value, hover) in [
                        (
                            "Folder",
                            folder,
                            photo.as_ref().map(|p| p.path.display().to_string()),
                        ),
                        (
                            "Capture Time",
                            photo.as_ref().map(Photo::capture_text).unwrap_or_default(),
                            None,
                        ),
                        ("Format", field(|p| &p.format), None),
                        ("Dimensions", text(PhotoInfo::dimensions_text), None),
                        ("Exposure", text(PhotoInfo::exposure_text), None),
                        ("Focal Length", text(PhotoInfo::focal_text), None),
                        ("ISO Speed", text(PhotoInfo::iso_text), None),
                        ("Camera", text(|i| i.camera.clone()), None),
                        ("Lens", text(|i| i.lens.clone()), None),
                    ] {
                        let response = metadata_row(ui, key, &value);
                        if let Some(hover) = hover {
                            response.on_hover_text(hover);
                        } else if !value.is_empty() {
                            response.on_hover_text(value);
                        }
                    }
                });
                section(ui, "Keywording", |ui| {
                    self.keyword_fields(ui);
                });
            });
        action
    }
    /// Rating, flag and label of `id`; a change applies to the whole
    /// selection when `whole_selection` (the Grid) and `id` is in it.
    pub(crate) fn metadata_controls(
        &mut self,
        ui: &mut egui::Ui,
        id: PhotoId,
        whole_selection: bool,
    ) -> bool {
        if let Some(photo) = self.photo(id).cloned()
            && let Some(edit) =
                crate::app::library::photo_metadata::controls(ui, &photo, &self.labels())
        {
            let ids = if whole_selection && self.selection.selected.contains(&id) {
                self.selected_ids()
            } else {
                vec![id]
            };
            // The active photo goes as its keys do, so Compare keeps its
            // pair and Survey its photos.
            let done = if self.edits_active_only() && self.selection.active == Some(id) {
                self.edit_shown(edit, false)
            } else {
                self.edit_photos(&ids, edit, false).map(|_| ())
            };
            if let Err(e) = done {
                self.messages
                    .fail(format!("Metadata could not be saved: {e}"));
            }
            return true;
        }
        false
    }
}
/// A fixed-height metadata row: caption column, then the truncated value
/// (a dash when empty), so the panel never widens or reflows.
fn metadata_row(ui: &mut egui::Ui, key: &str, value: &str) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), egui::Sense::hover());
    caption_at(ui, rect, key);
    value_at(
        ui,
        rect,
        if value.is_empty() { "—" } else { value },
        !value.is_empty(),
    );
    response
}
/// One truncated line of secondary text at a fixed height.
fn info_text(ui: &mut egui::Ui, text: &str) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.), egui::Sense::hover());
    paint_truncated(
        ui,
        rect.left_center(),
        rect.width(),
        text,
        theme::palette(ui.ctx()).tone(Tone::SECONDARY),
    );
}
