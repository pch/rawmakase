//! The Calibration panel: the process version, the camera's exposure baseline,
//! Shadows tint and the red, green and blue primaries.
use super::super::{PanelSwitch, control_row, rail_row, subheading};
use crate::app::develop::step_name::StepName;
use crate::app::ui::design::{Tone, space, text};
use crate::app::ui::theme;
use crate::app::ui::widgets::{setting_slider, switched_section};
use crate::camera_data::Metadata;
use crate::model::panels::Panel;
use crate::model::params::ParameterId;
use crate::model::recipe::Recipe;
use eframe::egui;

/// Draws the panel for photo `metadata`.
pub(in crate::app::develop::panels::inspector) fn show(
    ui: &mut egui::Ui,
    step: &mut StepName,
    r: &mut Recipe,
    metadata: Option<&Metadata>,
) {
    let mut switch = PanelSwitch::new(r, Panel::Calibration);
    if switched_section(ui, step, "Calibration", &mut switch.state, |ui, step| {
        // The process version as a row of the slider grid, the camera's
        // exposure baseline under it at the rails' start.
        ui.add_space(space::XS);
        control_row(ui, "Process", |ui| {
            let palette = theme::palette(ui.ctx());
            ui.label(
                egui::RichText::new("Current version")
                    .size(text::LABEL)
                    .color(palette.tone(Tone::SOFT)),
            );
        });
        if r.profile.is_some()
            && let Some(m) = metadata
        {
            let baseline = crate::camera_profiles::reference::baseline_exposure(m);
            if baseline != r.camera_exposure {
                let use_baseline = rail_row(ui, |ui| {
                    ui.small_button("Use camera exposure baseline")
                            .on_hover_text("Apply the camera's reference exposure offset while leaving the Exposure slider unchanged.")
                });
                if use_baseline.clicked() {
                    r.camera_exposure = baseline;
                }
            }
        }
        subheading(ui, "Shadows");
        ui.push_id("calibration-shadows", |ui| {
            setting_slider(
                ui,
                step,
                ParameterId::ShadowTint,
                &mut r.effects.shadow_tint,
                0.,
            );
        });
        for (i, name) in ["Red Primary", "Green Primary", "Blue Primary"]
            .iter()
            .enumerate()
        {
            subheading(ui, name);
            ui.push_id(("calibration", i), |ui| {
                let primary = ParameterId::PRIMARIES[i];
                for id in primary {
                    setting_slider(ui, step, id, id.value_mut(r), 0.);
                }
            });
        }
    }) {
        r.effects.calibration = [[0.; 2]; 3];
        r.effects.shadow_tint = 0.;
    }
    switch.finish(r);
}
