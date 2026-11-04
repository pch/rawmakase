//! The Detail panel: Sharpening, Noise Reduction and AI Denoise
//! (`inspector::ai_denoise`).
use super::super::{PanelSwitch, ai_denoise, subheading};
use super::Action;
use crate::app::develop::step_name::StepName;
use crate::app::ui::widgets::{setting_slider, switched_section};
use crate::model::operators::SharpeningSliders;
use crate::model::panels::Panel;
use crate::model::params::ParameterId;
use crate::model::recipe::Recipe;
use eframe::egui;

/// Draws the panel; `ai_view` is what AI Denoise's controls show.
pub(in crate::app::develop::panels::inspector) fn show(
    ui: &mut egui::Ui,
    step: &mut StepName,
    r: &mut Recipe,
    ai_view: &ai_denoise::View,
) -> Vec<Action> {
    let mut actions = Vec::new();
    let mut switch = PanelSwitch::new(r, Panel::Detail);
    let sharpening = SharpeningSliders::defaults();
    if switched_section(ui, step, "Detail", &mut switch.state, |ui, step| {
        // Lightroom's Color noise reduction default for raw files.
        let color_default = 0.25;
        let mut control = |ui: &mut egui::Ui, id: ParameterId, default: f32| {
            setting_slider(ui, step, id, id.value_mut(r), default);
        };
        subheading(ui, "Sharpening");
        control(ui, ParameterId::SharpeningAmount, sharpening.amount);
        control(ui, ParameterId::SharpeningRadius, sharpening.radius);
        control(ui, ParameterId::SharpeningDetail, sharpening.detail);
        control(ui, ParameterId::SharpeningMasking, sharpening.masking);
        subheading(ui, "Noise Reduction");
        control(ui, ParameterId::LuminanceNoise, 0.);
        ui.push_id("luma-nr", |ui| {
            control(ui, ParameterId::LuminanceDetail, 0.5);
            control(ui, ParameterId::LuminanceContrast, 0.);
        });
        control(ui, ParameterId::ColorNoise, color_default);
        ui.push_id("color-nr", |ui| {
            control(ui, ParameterId::ColorNoiseDetail, 0.5);
            control(ui, ParameterId::ColorNoiseSmoothness, 0.5);
        });
        ui.push_id("ai-denoise", |ui| {
            if let Some(action) = ai_denoise::controls(ui, step, r, ai_view) {
                actions.push(Action::AiDenoise(action));
            }
        });
    }) {
        r.ai_denoise = Default::default();
        let d = Recipe::default().effects;
        r.noise_luma = 0.;
        r.effects.luma_detail = d.luma_detail;
        r.effects.luma_contrast = d.luma_contrast;
        r.set_color_noise_defaults();
        r.set_sharpening_defaults();
    }
    switch.finish(r);
    actions
}
