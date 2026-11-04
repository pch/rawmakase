//! The Import .cube LUTs window: the colour space the LUTs expect, chosen for the
//! batch before the files, as Camera Raw's Create Profile asks for it. It opens at
//! sRGB every time, so a space chosen for one pack never applies to the next.
use crate::app::presets::bulk_import::ImportKind;
use crate::app::shell::dialogs::FileDialog;
use crate::app::ui::design::{self, Tone, button, inset, space, text};
use crate::app::ui::widgets::{modal_frame, primary_button};
use crate::app::{Editor, ui::theme};
use crate::camera_profiles::LutSpace;
use eframe::egui::{self, Color32};

impl Editor {
    /// Opens the Import .cube LUTs window at sRGB.
    pub(in crate::app) fn open_lut_import(&mut self) {
        self.lut_import = Some(LutSpace::Srgb);
    }
    /// Whether the Import .cube LUTs window is open.
    pub(in crate::app) fn lut_import_open(&self) -> bool {
        self.lut_import.is_some()
    }
    /// The Import .cube LUTs window, while open; its choice opens the file chooser.
    pub(in crate::app) fn lut_import_window(&mut self, ctx: &egui::Context) {
        if let Some(dialog) = self.lut_import_ui(ctx) {
            self.dialog(dialog, ctx);
        }
    }
    /// The window itself, and the chooser it asks for: files or a folder of LUTs for
    /// the space chosen, or none, when cancelled or still open.
    pub(in crate::app) fn lut_import_ui(&mut self, ctx: &egui::Context) -> Option<FileDialog> {
        let palette = theme::palette(ctx);
        let space = self.lut_import.as_mut()?;
        let mut choice = None;
        let response = egui::Modal::new(egui::Id::new("import-luts"))
            .backdrop_color(Color32::from_black_alpha(140))
            .frame(modal_frame(&palette).inner_margin(design::margin_same(inset::DIALOG)))
            .show(ctx, |ui| {
                ui.set_width(400.);
                ui.label(
                    egui::RichText::new("Import .cube LUTs")
                        .size(text::TITLE)
                        .color(palette.tone(Tone::STRONG)),
                );
                ui.add_space(space::S);
                ui.label(
                    egui::RichText::new(
                        "A LUT expects colours in one colour space. Most are made for sRGB; \
                         choose another only when the LUT's maker says so.",
                    )
                    .size(text::SMALL)
                    .color(palette.tone(Tone::SECONDARY)),
                );
                ui.add_space(space::L);
                ui.horizontal(|ui| {
                    ui.label("Colour space");
                    egui::ComboBox::from_id_salt("lut-space")
                        .width(180.)
                        .selected_text(space.name())
                        .show_ui(ui, |ui| {
                            for option in LutSpace::ALL {
                                ui.selectable_value(space, option, option.name());
                            }
                        });
                });
                ui.add_space(space::XL);
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding = button::DIALOG;
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if primary_button(ui, "Choose Files…").clicked() {
                            choice = Some(Some(FileDialog::Luts(*space)));
                        }
                        if ui.button("Choose Folder…").clicked() {
                            choice = Some(Some(FileDialog::ImportFolder(ImportKind::Luts(*space))));
                        }
                        if ui.button("Cancel").clicked() {
                            choice = Some(None);
                        }
                    });
                });
            });
        if response.should_close() && choice.is_none() {
            choice = Some(None);
        }
        let choice = choice?;
        self.lut_import = None;
        choice
    }
}

#[cfg(test)]
#[path = "lut_import_editor_tests.rs"]
mod editor_tests;
