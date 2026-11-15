//! The AI Retouch drawer's own parts: the generator's controls above the
//! brush, what made the selected spot above the sliders that change it, and
//! painting and sending below the brush, with the one-time question before
//! pixels leave the computer, the request running and what went wrong.
use super::Failure;
use crate::app::Editor;
use crate::app::develop::tools::retouch_tool::{hint, indented};
use crate::app::extensions::panels;
use crate::app::extensions::retouch_generator::Offer;
use crate::app::preferences::Tab;
use crate::app::ui::design::{size, space, text};
use crate::app::ui::theme;
use crate::app::ui::widgets::drawer_help;
use eframe::egui::{self, Vec2};

impl Editor {
    /// AI Retouch's own controls, above the brush: the generator's panel (a
    /// mode, a prompt), drawn as the drawer's rows, and why it can't run.
    pub(in crate::app) fn ai_controls(
        &mut self,
        ui: &mut egui::Ui,
        offer: &Offer,
        why: Option<String>,
    ) {
        // Held while a request is asked about or under way: the program
        // reads its values when the request reaches it, after its rendition,
        // so a change now would change what was sent.
        let busy = self.ai_remove.running().is_some() || self.ai_remove.consent.is_some();
        if let Some(panel) = offer.panel.clone() {
            ui.add_enabled_ui(!busy, |ui| {
                self.extension_panel_ui(ui, &offer.extension, &panel, panels::Layout::Drawer);
            });
        }
        if let Some(why) = why.or_else(|| self.ai_remove_unavailable()) {
            hint(ui, &why);
        }
    }

    /// What made the selected AI spot, above the sliders that change it.
    pub(in crate::app) fn ai_spot_info(&mut self, ui: &mut egui::Ui, selected: Option<usize>) {
        let Some(patch) = selected
            .and_then(|i| self.document.edit.recipe().retouch.get(i))
            .and_then(|op| op.patch.clone())
        else {
            return;
        };
        // The generator by its name when it is still offered.
        let made_by = self
            .ai_remove_offers()
            .into_iter()
            .find(|(o, _)| o.provenance == patch.generator)
            .map_or(patch.generator.clone(), |(o, _)| o.name);
        // Under the panel's controls, where their help lines start.
        drawer_help(ui, &format!("Made by {made_by} · {}", patch.model));
        // What its generator said it did, in the user's words.
        if let Some(summary) = &patch.summary {
            drawer_help(ui, &format!("\u{201c}{summary}\u{201d}"));
        }
        // As rendered: Detail switched off bypasses AI Denoise.
        let recipe = self.document.edit.recipe();
        let now = self
            .document
            .full()
            .map_or(recipe.ai_denoise.renders(), |im| {
                recipe.resolved(&im.metadata).ai_denoise.renders()
            });
        if patch.ai_denoise != now {
            drawer_help(
                ui,
                if patch.ai_denoise {
                    "Made with AI Denoise on; the photo now has it off"
                } else {
                    "Made with AI Denoise off; the photo now has it on"
                },
            );
        }
    }

    /// Painting and sending, below the brush: what to do, Send and Clear (as
    /// Reset and Close are drawn), the request running with Cancel in Send's
    /// place, the upload question, and what went wrong.
    pub(in crate::app) fn ai_send(&mut self, ui: &mut egui::Ui, offer: &Offer) {
        let palette = theme::palette(ui.ctx());
        hint(ui, "Paint over what to change, then Send.");
        let blocked = self.ai_send_blocked();
        let painted = !self.ai_remove.draft.is_empty();
        let working = self.ai_remove.working();
        indented(ui, |ui| {
            let w = (ui.available_width() - space::XS) / 2.;
            let size = Vec2::new(w, size::CONTROL);
            if working.is_some() {
                if ui
                    .add_sized(size, egui::Button::new("Cancel"))
                    .on_hover_text("Stop; the photo is left as it was")
                    .clicked()
                {
                    self.cancel_ai_remove();
                }
            } else {
                let send = ui.add_enabled(
                    blocked.is_none(),
                    egui::Button::new(egui::RichText::new("Send").color(palette.on_accent()))
                        .fill(palette.accent())
                        .min_size(size),
                );
                let send = match &blocked {
                    Some(why) => send.on_disabled_hover_text(why),
                    None => send.on_hover_text("Send the area painted · Enter"),
                };
                if send.clicked() {
                    self.send_ai_draft();
                }
            }
            if ui
                .add_enabled(
                    painted && working.is_none(),
                    egui::Button::new("Clear").min_size(size),
                )
                .on_hover_text("Forget the area painted · Esc")
                .clicked()
            {
                self.clear_ai_draft();
            }
        });
        if let Some(working) = working {
            indented(ui, |ui| {
                crate::app::ui::widgets::spinner(ui, text::SMALL);
                ui.label(egui::RichText::new(working).size(text::SMALL));
            });
        } else if painted && let Some(why) = &blocked {
            // Why it can't go, once something is painted: nothing to say before.
            hint(ui, why);
        }
        if let Some(consent) = self.ai_remove.consent.clone() {
            // In the text's own colour, not a hint's: this waits for an answer.
            indented(ui, |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "{} sends the painted area and its surroundings to the service set \
                             up in {}. Send it?",
                            consent.offer.name, consent.offer.extension_name
                        ))
                        .size(text::SMALL),
                    )
                    .wrap(),
                );
            });
            indented(ui, |ui| {
                if ui.button("Send").clicked() {
                    self.answer_ai_remove_consent(true);
                }
                if ui.button("Cancel").clicked() {
                    self.answer_ai_remove_consent(false);
                }
            });
        }
        if let Some(failure) = self.ai_remove.failure.clone() {
            // The failed request's generator, not the one chosen since.
            let failed = self
                .ai_remove
                .failed
                .as_ref()
                .map_or(offer.clone(), |(failed, _)| failed.clone());
            // One line in the warning tone; a button only where there is
            // something to do. A prompt to write goes once the next send does.
            indented(ui, |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(failure.message(&failed.name))
                            .size(text::MICRO)
                            .color(palette.warning()),
                    )
                    .wrap(),
                );
            });
            match failure {
                Failure::NeedsInput(_) => {}
                Failure::NeedsSetup(_) => indented(ui, |ui| {
                    if ui
                        .small_button(format!("Set up {}…", failed.extension_name))
                        .clicked()
                    {
                        self.open_extension_page(&failed.extension);
                    }
                    if ui.small_button("Dismiss").clicked() {
                        self.dismiss_ai_failure();
                    }
                }),
                _ => indented(ui, |ui| {
                    if ui.small_button("Dismiss").clicked() {
                        self.dismiss_ai_failure();
                    }
                }),
            }
        } else if self.ai_setup_needed(offer) {
            hint(
                ui,
                &format!("{} needs setting up before you send", offer.extension_name),
            );
            indented(ui, |ui| {
                if ui
                    .button(format!("Set up {}…", offer.extension_name))
                    .clicked()
                {
                    self.open_extension_page(&offer.extension);
                }
            });
        }
    }

    /// Where AI Remove comes from when no extension offers it.
    pub(in crate::app) fn ai_remove_install_hint(&mut self, ui: &mut egui::Ui) {
        hint(
            ui,
            "AI Retouch (experimental) comes as an extension, with your own API key",
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
