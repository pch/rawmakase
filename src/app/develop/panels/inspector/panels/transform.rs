//! The Transform panel: Upright, its Guided tool, Constrain Crop and the
//! manual Transform sliders.
use super::super::{PanelSwitch, control_row, subheading};
use super::Action;
use crate::app::develop::state::{Tool, ViewState};
use crate::app::develop::step_name::StepName;
use crate::app::ui::design::Tone;
use crate::app::ui::theme;
use crate::app::ui::widgets::{segmented, setting_slider, switched_section};
use crate::camera_data::Metadata;
use crate::model::panels::Panel;
use crate::model::params::ParameterId;
use crate::model::recipe::Recipe;
use eframe::egui;

/// What the Transform panel asks of the Guided Upright tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app::develop::panels::inspector) enum GuidedAction {
    /// Guided was chosen among the modes.
    Choose,
    /// Draw: open or close the tool.
    Toggle,
    Clear,
}

/// Draws the panel for photo `metadata`. `upright_ready` is whether Upright can
/// analyse the photo now: decoded, and no analysis running.
pub(in crate::app::develop::panels::inspector) fn show(
    ui: &mut egui::Ui,
    step: &mut StepName,
    r: &mut Recipe,
    view: &mut ViewState,
    metadata: Option<&Metadata>,
    upright_ready: bool,
    palette: &theme::Palette,
) -> Vec<Action> {
    let mut actions = Vec::new();
    let mut switch = PanelSwitch::new(r, Panel::Transform);
    if switched_section(ui, step, "Transform", &mut switch.state, |ui, step| {
        use crate::model::transform::UprightMode;
        // As Lightroom: Update beside the heading, then the modes in two rows.
        control_row(ui, "Upright", |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Guided solves its guides again; without guides it has nothing to go by.
                let analysed = match r.upright.mode {
                    UprightMode::Off => false,
                    UprightMode::Guided => !r.upright.guides.is_empty(),
                    _ => true,
                };
                if ui
                    .add_enabled(upright_ready && analysed, egui::Button::new("Update"))
                    .on_hover_text("Analyse the photo again, e.g. after changing lens corrections")
                    .clicked()
                {
                    actions.push(Action::Upright);
                }
            });
        });
        let u = &mut r.upright;
        let before = u.mode;
        for row in [
            [
                (UprightMode::Off, "Off"),
                (UprightMode::Auto, "Auto"),
                (UprightMode::Guided, "Guided"),
            ],
            [
                (UprightMode::Level, "Level"),
                (UprightMode::Vertical, "Vertical"),
                (UprightMode::Full, "Full"),
            ],
        ] {
            let w = ui.available_width();
            segmented(ui, &mut u.mode, &row, w);
        }
        if u.mode != before {
            if u.mode == UprightMode::Guided {
                // As in Lightroom, choosing Guided picks up its tool.
                actions.push(Action::Guided(GuidedAction::Choose));
            } else if u.mode != UprightMode::Off && u.corrections.len() <= u.mode.code() {
                actions.push(Action::Upright);
            }
        }
        if u.mode == UprightMode::Guided {
            let count = u.guides.len();
            control_row(ui, "Guides", |ui| {
                let drawing = view.is(Tool::Guided);
                if ui
                    .selectable_label(drawing, "Draw")
                    .on_hover_text(
                        "Draw up to four guides along verticals and horizontals · Shift+T",
                    )
                    .clicked()
                {
                    actions.push(Action::Guided(GuidedAction::Toggle));
                }
                ui.label(
                    egui::RichText::new(format!(
                        "{count} of {}",
                        crate::model::transform::MAX_GUIDES
                    ))
                    .color(palette.tone(Tone::LABEL)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(count > 0, egui::Button::new("Clear"))
                        .on_hover_text("Remove every guide")
                        .clicked()
                    {
                        actions.push(Action::Guided(GuidedAction::Clear));
                    }
                });
            });
            if view.is(Tool::Guided) {
                control_row(ui, "", |ui| {
                    ui.checkbox(&mut view.guided.loupe, "Show Loupe")
                        .on_hover_text("Magnify the photo while placing a guide's end");
                    ui.checkbox(&mut view.guided.grid, "Grid");
                });
            }
        } else if view.is(Tool::Guided) {
            view.tool = Tool::None;
        }
        control_row(ui, "", |ui| {
            ui.checkbox(&mut r.constrain_crop, "Constrain Crop")
                .on_hover_text("Shrink the crop, keeping its aspect, to leave out the white areas outside the photo");
        });
        subheading(ui, "Transform");
        // Stored as Camera Raw applies them, before the photo is turned for
        // display; shown, as in Lightroom, along the displayed photo's axes.
        let turns = metadata.map_or(0, |m| {
            crate::model::image_frame::ImageFrame::for_metadata(m).turns
        });
        let axes =
            crate::model::transform::display_axes((turns + r.rotation) % 4, r.flip_x, r.flip_y);
        let mut shown = r.transform.displayed(axes);
        let t = &mut shown;
        setting_slider(
            ui,
            step,
            ParameterId::TransformVertical,
            &mut t.vertical,
            0.,
        );
        setting_slider(
            ui,
            step,
            ParameterId::TransformHorizontal,
            &mut t.horizontal,
            0.,
        );
        setting_slider(ui, step, ParameterId::TransformRotate, &mut t.rotate, 0.);
        setting_slider(ui, step, ParameterId::TransformAspect, &mut t.aspect, 0.);
        setting_slider(ui, step, ParameterId::TransformScale, &mut t.scale, 1.);
        setting_slider(ui, step, ParameterId::TransformOffsetX, &mut t.offset_x, 0.);
        setting_slider(ui, step, ParameterId::TransformOffsetY, &mut t.offset_y, 0.);
        if shown != r.transform.displayed(axes) {
            r.transform = shown.recorded(axes);
        }
    }) {
        r.transform = Default::default();
        r.upright = Default::default();
        r.constrain_crop = false;
    }
    switch.finish(r);
    actions
}
