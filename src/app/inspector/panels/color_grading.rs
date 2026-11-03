//! The Color Grading panel: Shadows, Midtones and Highlights wheels, Global,
//! Blending and Balance (`app::color_grading`).
use super::super::PanelSwitch;
use crate::app::state::ViewState;
use crate::app::widgets::switched_section;
use crate::model::panels::Panel;
use crate::model::recipe::Recipe;
use eframe::egui;

/// Draws the panel; `grading_document` keeps the wheels' state apart per photo.
pub(in crate::app::inspector) fn show(
    ui: &mut egui::Ui,
    r: &mut Recipe,
    view: &mut ViewState,
    grading_document: u64,
) {
    let mut switch = PanelSwitch::new(r, Panel::ColorGrading);
    if switched_section(ui, "Color Grading", &mut switch.state, |ui| {
        ui.push_id(grading_document, |ui| {
            crate::app::color_grading::color_grading_ui(ui, r, &mut view.grading);
        });
    }) {
        r.grading = [[0.; 3]; 3];
        r.effects.global_grade = [0.; 3];
        r.effects.balance = 0.;
        r.effects.blending = 0.5;
    }
    switch.finish(r);
}
