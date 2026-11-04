//! The Color Grading panel: Shadows, Midtones and Highlights wheels, Global,
//! Blending and Balance (`app::develop::panels::color_grading`).
use super::super::PanelSwitch;
use crate::app::develop::state::ViewState;
use crate::app::develop::step_name::StepName;
use crate::app::ui::widgets::switched_section;
use crate::model::panels::Panel;
use crate::model::recipe::Recipe;
use eframe::egui;

/// Draws the panel; `grading_document` keeps the wheels' state apart per photo.
pub(in crate::app::develop::panels::inspector) fn show(
    ui: &mut egui::Ui,
    step: &mut StepName,
    r: &mut Recipe,
    view: &mut ViewState,
    grading_document: u64,
) {
    let mut switch = PanelSwitch::new(r, Panel::ColorGrading);
    if switched_section(ui, step, "Color Grading", &mut switch.state, |ui, step| {
        ui.push_id(grading_document, |ui| {
            crate::app::develop::panels::color_grading::color_grading_ui(
                ui,
                step,
                r,
                &mut view.grading,
            );
        });
    }) {
        r.grading = [[0.; 3]; 3];
        r.effects.global_grade = [0.; 3];
        r.effects.balance = 0.;
        r.effects.blending = 0.5;
    }
    switch.finish(r);
}
