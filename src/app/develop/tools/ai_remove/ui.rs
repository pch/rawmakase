//! The Remove drawer's side of AI Remove: that it is experimental, why it
//! can't run, the one-time question before pixels leave the computer, the
//! request running, what went wrong, and what made the selected spot.
use super::Failure;
use crate::app::Editor;
use crate::app::develop::tools::retouch_tool::{hint, indented};
use crate::app::extensions::retouch_generator::Offer;
use crate::app::preferences::Tab;
use crate::app::ui::design::space;
use eframe::egui;

impl Editor {
    /// Below Mode while AI Remove is chosen or a Generate spot is selected.
    pub(in crate::app) fn ai_remove_section(
        &mut self,
        ui: &mut egui::Ui,
        offer: Option<(Offer, Option<String>)>,
        selected: Option<usize>,
    ) {
        ui.add_space(space::XS);
        if let Some(op) = selected.and_then(|i| self.document.edit.recipe().retouch.get(i))
            && let Some(patch) = &op.patch
        {
            hint(
                ui,
                &format!("Made by {} · {}", patch.generator, patch.model),
            );
            let now = self.document.edit.recipe().ai_denoise.renders();
            if patch.ai_denoise != now {
                hint(
                    ui,
                    if patch.ai_denoise {
                        "Made with AI Denoise on; the photo now has it off"
                    } else {
                        "Made with AI Denoise off; the photo now has it on"
                    },
                );
            }
            hint(
                ui,
                "Fixed where it was made: Feather and Opacity still change",
            );
        }
        let Some((offer, why)) = offer else {
            return;
        };
        if self.view.retouch.mode != crate::model::retouch::RetouchMode::Generate {
            return;
        }
        hint(
            ui,
            &format!(
                "Experimental · paint over what to remove; the area and its surroundings go to \
                 the service set up in {}, one paid call each",
                offer.extension_name
            ),
        );
        if let Some(why) = why.or_else(|| self.ai_remove_unavailable()) {
            hint(ui, &why);
        }
        if let Some(consent) = self.ai_remove.consent.clone() {
            hint(
                ui,
                &format!(
                    "AI Remove sends the painted area and its surroundings to the service set up \
                     in {}. Continue?",
                    consent.offer.extension_name
                ),
            );
            indented(ui, |ui| {
                if ui.button("Continue").clicked() {
                    self.answer_ai_remove_consent(true);
                }
                if ui.button("Cancel").clicked() {
                    self.answer_ai_remove_consent(false);
                }
            });
        }
        if self.ai_remove.running().is_some() {
            indented(ui, |ui| {
                crate::app::ui::widgets::spinner(ui, 14.);
                ui.label("Removing…");
                if ui
                    .button("Cancel")
                    .on_hover_text("Stop; the photo is left as it was")
                    .clicked()
                {
                    self.cancel_ai_remove();
                }
            });
        }
        if let Some(failure) = self.ai_remove.failure.clone() {
            hint(ui, &failure.message());
            indented(ui, |ui| {
                if matches!(failure, Failure::NeedsSetup(_))
                    && ui
                        .button(format!("Set up {}…", offer.extension_name))
                        .clicked()
                {
                    self.open_extension_page(&offer.extension);
                }
                if ui.button("Dismiss").clicked() {
                    self.ai_remove.failure = None;
                }
            });
        }
    }

    /// Where AI Remove comes from when no extension offers it.
    pub(in crate::app) fn ai_remove_install_hint(&mut self, ui: &mut egui::Ui) {
        hint(
            ui,
            "AI Remove (experimental) comes as an extension, with your own API key",
        );
        indented(ui, |ui| {
            if ui.small_button("Preferences > Extensions…").clicked() {
                self.open_preferences(Tab::Extensions);
            }
        });
    }

    /// Preferences > Extensions, on extension `id`'s page.
    pub(in crate::app) fn open_extension_page(&mut self, id: &str) {
        self.extension_page.select(id);
        self.open_preferences(Tab::Extensions);
    }
}
