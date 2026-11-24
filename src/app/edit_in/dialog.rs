//! Edit In's question for photos the editor can open as they are, as
//! Lightroom's Edit Photo window asks it: the original, a copy, or a copy
//! with RAWmakase's adjustments.
use super::How;
use crate::app::Editor;
use crate::app::ui::design::{self, Tone, button, inset, space, text};
use crate::app::ui::widgets;
use eframe::egui;

impl Editor {
    /// Edit In's window while it asks; Edit goes on as chosen, Cancel or
    /// Escape drops it.
    pub(in crate::app) fn edit_in_window(&mut self, ctx: &egui::Context) {
        let Some(mut asking) = self.edit_in.asking.take() else {
            return;
        };
        // Its photos are another catalog's ids once another is open.
        if !self.asking_here(&asking) {
            self.messages
                .warn("Edit In cancelled: another catalog is open now");
            return;
        }
        let palette = crate::app::ui::theme::palette(ctx);
        let program = crate::platform::open_with::program_name(&asking.editor);
        let title = match asking.ids.len() {
            1 => format!("Edit Photo with {program}"),
            n => format!("Edit {n} Photos with {program}"),
        };
        let raws = asking.ids.len() > asking.plain.len();
        let (mut edit, mut cancel) = (false, false);
        let hint = |ui: &mut egui::Ui, text: &str| {
            // Under its choice's label, past the radio button.
            ui.horizontal(|ui| {
                ui.add_space(ui.spacing().icon_width + ui.spacing().icon_spacing);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(text)
                            .size(text::LABEL)
                            .color(palette.tone(Tone::MUTED)),
                    )
                    .wrap(),
                );
            });
        };
        let response = egui::Modal::new(egui::Id::new("edit-in"))
            .frame(widgets::modal_frame(&palette).inner_margin(design::margin_same(inset::DIALOG)))
            .show(ctx, |ui| {
                widgets::name_dialog(ui, &title);
                ui.set_width(420.);
                ui.label(
                    egui::RichText::new(&title)
                        .size(text::TITLE)
                        .color(palette.tone(Tone::STRONG)),
                );
                ui.add_space(space::M);
                let unseen = if asking.adjusted {
                    " Adjustments made in RAWmakase are not visible."
                } else {
                    ""
                };
                for (how, label, about) in [
                    (
                        How::Original,
                        "Edit Original",
                        format!("Edit the original file.{unseen}"),
                    ),
                    (
                        How::Copy,
                        "Edit a Copy",
                        format!("Edit a copy of the original file.{unseen}"),
                    ),
                    (
                        How::Adjusted,
                        "Edit a Copy with RAWmakase Adjustments",
                        "Apply the RAWmakase adjustments to a 16-bit TIFF copy of the file and edit that one.".to_string(),
                    ),
                ] {
                    ui.radio_value(&mut asking.how, how, label);
                    hint(ui, &about);
                    ui.add_space(space::S);
                }
                if raws {
                    ui.add_space(space::S);
                    ui.label(
                        egui::RichText::new(
                            "RAW photos are always edited as a 16-bit TIFF copy with their adjustments.",
                        )
                        .size(text::LABEL)
                        .color(palette.tone(Tone::MUTED)),
                    );
                }
                ui.add_space(space::XL);
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding = button::DIALOG;
                    cancel = ui.button("Cancel").clicked();
                    edit = widgets::primary_button(ui, "Edit").clicked();
                });
            });
        if cancel || response.should_close() {
            return;
        }
        if edit {
            self.edit_in_as(asking);
        } else {
            self.edit_in.asking = Some(asking);
        }
    }
}
