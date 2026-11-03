//! The Effects panel: Post-Crop Vignetting and Grain.
use super::super::{PanelSwitch, setting_control, subheading};
use crate::app::widgets::switched_section;
use crate::camera_data::Metadata;
use crate::model::panels::Panel;
use crate::model::params::ParameterId;
use crate::model::recipe::Recipe;
use eframe::egui;

/// Draws the panel for photo `metadata`.
pub(in crate::app::inspector) fn show(
    ui: &mut egui::Ui,
    r: &mut Recipe,
    metadata: Option<&Metadata>,
) {
    let mut switch = PanelSwitch::new(r, Panel::Effects);
    if switched_section(ui, "Effects", &mut switch.state, |ui| {
        let photo = metadata;
        subheading(ui, "Post-Crop Vignetting");
        setting_control(ui, r, ParameterId::VignetteAmount, 0., photo);
        setting_control(ui, r, ParameterId::VignetteMidpoint, 0.5, photo);
        setting_control(ui, r, ParameterId::VignetteFeather, 0.5, photo);
        subheading(ui, "Grain");
        ui.push_id("grain", |ui| {
            setting_control(ui, r, ParameterId::GrainAmount, 0., photo);
            setting_control(ui, r, ParameterId::GrainSize, 0.25, photo);
            setting_control(ui, r, ParameterId::GrainRoughness, 0.5, photo);
        });
    }) {
        r.effects.reset_post_crop();
    }
    switch.finish(r);
}
