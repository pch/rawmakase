//! The AI Retouch drawer's side of AI Remove: that it is experimental, why it
//! can't run, the one-time question before pixels leave the computer, the
//! request running, what went wrong, and what made the selected spot.
use super::Failure;
use crate::app::Editor;
use crate::app::develop::tools::retouch_tool::{control_label_top, hint, indented};
use crate::app::extensions::retouch_generator::Offer;
use crate::app::preferences::Tab;
use crate::app::ui::design::space;
use eframe::egui;
use rawmakase_extensions::protocol::manifest::PromptUse;
use rawmakase_extensions::protocol::method::MAX_PROMPT_LEN;

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
            if let Some(prompt) = &patch.prompt {
                hint(ui, &format!("Asked: \u{201c}{prompt}\u{201d}"));
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
        if !self.ai_retouching() {
            return;
        }
        if offer.prompt != PromptUse::None {
            let placeholder = offer
                .prompt_hint
                .clone()
                .unwrap_or_else(|| match offer.prompt {
                    PromptUse::Required => "What to do with the area".into(),
                    _ => "What to do with the area, if anything".into(),
                });
            control_label_top(ui, "Prompt", |ui| {
                let text = self.ai_prompt_mut(&offer);
                // Characters, not bytes: four bytes each at most.
                crate::app::ui::widgets::text_box(ui, text, &placeholder, 2, MAX_PROMPT_LEN / 4);
            });
            // Each mode keeps its own prompt; say whether this one needs it.
            hint(
                ui,
                &match offer.prompt {
                    PromptUse::Required => {
                        format!("{} needs a prompt before you paint", offer.name)
                    }
                    _ => format!("Optional for {}: say what to remove or keep", offer.name),
                },
            );
            ui.add_space(space::M);
        }
        hint(
            ui,
            &format!(
                "Paint over an area, in as many strokes as it takes, then send it: it and its \
                 surroundings go to the service set up in {}, one paid call",
                offer.extension_name
            ),
        );
        if let Some(why) = why.or_else(|| self.ai_remove_unavailable()) {
            hint(ui, &why);
        }
        // Send: what was painted goes only when asked.
        let blocked = self.ai_send_blocked();
        let painted = !self.ai_remove.draft.is_empty();
        indented(ui, |ui| {
            let send = ui.add_enabled_ui(blocked.is_none(), |ui| {
                crate::app::ui::widgets::primary_button(ui, &offer.name)
            });
            let send = match &blocked {
                Some(why) => send.inner.on_disabled_hover_text(why),
                None => send.inner.on_hover_text("Send the area painted · Enter"),
            };
            if send.clicked() {
                self.send_ai_draft();
            }
            if ui
                .add_enabled(painted, egui::Button::new("Clear"))
                .on_hover_text("Forget the area painted · Esc")
                .clicked()
            {
                self.clear_ai_draft();
            }
        });
        // Why it can't go, once something is painted: nothing to say before.
        if painted && let Some(why) = &blocked {
            hint(ui, why);
        }
        if let Some(consent) = self.ai_remove.consent.clone() {
            // In the text's own colour, not a hint's: this waits for an answer.
            ui.add(
                egui::Label::new(format!(
                    "AI Remove sends the painted area and its surroundings to the service set up \
                     in {}. Send it?",
                    consent.offer.extension_name
                ))
                .wrap(),
            );
            indented(ui, |ui| {
                if ui.button("Send").clicked() {
                    self.answer_ai_remove_consent(true);
                }
                if ui.button("Cancel").clicked() {
                    self.answer_ai_remove_consent(false);
                }
            });
        }
        if let Some(working) = self.ai_remove.working() {
            indented(ui, |ui| {
                crate::app::ui::widgets::spinner(ui, 14.);
                ui.label(working);
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
            let name = self
                .ai_remove
                .failed
                .as_ref()
                .map_or(offer.name.clone(), |(name, _)| name.clone());
            // In the text's own colour, not a hint's: the last request failed.
            ui.add(egui::Label::new(failure.message(&name)).wrap());
            indented(ui, |ui| {
                if matches!(failure, Failure::NeedsSetup(_))
                    && ui
                        .button(format!("Set up {}…", offer.extension_name))
                        .clicked()
                {
                    self.open_extension_page(&offer.extension);
                }
                if ui.button("Dismiss").clicked() {
                    self.dismiss_ai_failure();
                }
            });
        } else if self.ai_setup_needed(&offer) {
            ui.add(
                egui::Label::new(format!(
                    "{} needs setting up before you paint",
                    offer.extension_name
                ))
                .wrap(),
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
