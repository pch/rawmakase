//! Detail › AI Denoise: the switch and Amount (edits like any other, one History step
//! each), the credit RawNIND's licence asks for, and a status line: the model to
//! install (credit shown first, nothing fetched until the user accepts), the result
//! being computed, ready, or why there is none. The result arriving is not an edit.
use super::lensfun::{accent_button, band, note, progress, small, title, warning};
use super::{control_row, subheading};
use crate::ai_denoise::State;
use crate::app::develop::step_name::StepName;
use crate::app::ui::design::text;
use crate::model::params::ParameterId;
use crate::model::recipe::Recipe;
use eframe::egui;
use rawmakase_inference::RAWNIND;

/// What the panel knows this frame.
pub(in crate::app) struct View {
    pub installed: bool,
    pub progress: Option<(u64, u64)>,
    pub install_failure: Option<String>,
    pub megabytes: u64,
    pub status: Option<State>,
    /// Why AI Denoise can't run for the open photo, if it can't: it needs the RAW's
    /// sensor data, which an editable preview doesn't keep.
    pub unavailable: Option<&'static str>,
}

/// What the user asked for that the panel can't do itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Action {
    Install,
    Cancel,
    Remove,
}

/// The AI Denoise rows of the Detail panel.
pub(super) fn controls(
    ui: &mut egui::Ui,
    step: &mut StepName,
    r: &mut Recipe,
    view: &View,
) -> Option<Action> {
    let heading = subheading(ui, "AI Denoise");
    credit_button(ui, heading);
    if let Some(why) = view.unavailable {
        // The setting stays as saved, and renders once the photo opens from its
        // original; nothing about it can change meanwhile.
        ui.add_enabled_ui(false, |ui| {
            control_row(ui, "", |ui| {
                let mut on = r.ai_denoise.enabled;
                ui.checkbox(&mut on, "AI Denoise")
                    .on_disabled_hover_text(why);
            });
        });
        status(ui, why);
        return None;
    }
    control_row(ui, "", |ui| {
        let mut on = r.ai_denoise.enabled;
        if ui
            .checkbox(&mut on, "AI Denoise")
            .on_hover_text(
                "Denoise and demosaic the RAW with a neural network (RawNIND). Computed \
                 once per photo, on this computer. Switching it on sets manual noise \
                 reduction to 0.",
            )
            .changed()
        {
            r.switch_ai_denoise(on);
        }
    });
    ui.add_enabled_ui(r.ai_denoise.enabled, |ui| {
        crate::app::ui::widgets::setting_slider(
            ui,
            step,
            ParameterId::AiDenoiseAmount,
            &mut r.ai_denoise.amount,
            0.5,
        );
    });
    if !r.ai_denoise.enabled {
        return None;
    }
    let mut action = None;
    if !view.installed {
        band(ui, "Model", |ui| action = install_offer(ui, view));
        return action;
    }
    if let Some(remove) = remove_link(ui) {
        action = Some(remove);
    }
    match &view.status {
        Some(State::Queued) => status(ui, "Waiting for another photo…"),
        Some(State::Running(done)) => {
            status(ui, &format!("Denoising… {:.0}%", done * 100.));
        }
        Some(State::Partial(_)) => status(ui, "Denoising… the view is ready"),
        Some(State::Ready(_)) => status(ui, "Ready"),
        Some(State::Failed(why)) => {
            control_row(ui, "", |ui| warning(ui, why));
        }
        None => {}
    }
    action
}

/// The installed model's removal, beside the status.
fn remove_link(ui: &mut egui::Ui) -> Option<Action> {
    control_row(ui, "", |ui| {
        ui.small_button("Remove model")
            .on_hover_text(
                "Delete AI Denoise's model from this computer; edits keep their settings",
            )
            .clicked()
            .then_some(Action::Remove)
    })
}

fn status(ui: &mut egui::Ui, text: &str) {
    control_row(ui, "", |ui| note(ui, text));
}

/// The download offer: the credit and licence first; nothing is fetched until the
/// user accepts.
fn install_offer(ui: &mut egui::Ui, view: &View) -> Option<Action> {
    let mut action = None;
    title(ui, "RawNIND UtNet2 (Bayer)");
    note(ui, RAWNIND.attribution);
    if let Some(why) = &view.install_failure {
        warning(ui, &format!("The download failed: {why}"));
    }
    if let Some((done, total)) = view.progress {
        if progress(ui, done, total) {
            action = Some(Action::Cancel);
        }
    } else {
        let label = if view.install_failure.is_some() {
            "Retry".to_string()
        } else {
            format!("Accept and download · {} MB", view.megabytes)
        };
        if accent_button(ui, &label)
            .on_hover_text("Accept the licence above and download the model")
            .clicked()
        {
            action = Some(Action::Install);
        }
    }
    links(ui);
    action
}

fn links(ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.;
        ui.hyperlink_to(
            egui::RichText::new("CC BY 4.0").size(text::SMALL),
            "https://creativecommons.org/licenses/by/4.0/",
        );
        small(ui, " · ");
        ui.hyperlink_to(
            egui::RichText::new("rawnind_jddc").size(text::SMALL),
            "https://github.com/trougnouf/rawnind_jddc",
        );
        small(ui, " · ");
        ui.hyperlink_to(
            egui::RichText::new("arXiv:2501.08924").size(text::SMALL),
            "https://arxiv.org/abs/2501.08924",
        );
    });
}

/// The ⓘ beside the heading: the credit, from the manifest's one text.
fn credit_button(ui: &mut egui::Ui, heading: egui::Rect) {
    let size = egui::vec2(16., 16.);
    let rect = egui::Rect::from_min_size(
        egui::pos2(
            heading.right() - size.x - 4.,
            heading.center().y - size.y / 2.,
        ),
        size,
    );
    let response = ui
        .put(
            rect,
            egui::Button::new(egui::RichText::new("ⓘ").size(text::SMALL)).frame(false),
        )
        .on_hover_text("About AI Denoise's model");
    egui::Popup::from_toggle_button_response(&response)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_max_width(320.);
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            note(ui, RAWNIND.attribution);
            links(ui);
        });
}
