//! The Tone Curve panel: Parametric and Point curves, Refine Saturation and
//! Levels.
use super::super::{PanelSwitch, control_row, highlight_targeted, subheading, targeted_shortcut};
use super::Action;
use crate::app::develop::panels::curve_menu::CurveChoice;
use crate::app::develop::state::{Tool, ViewState};
use crate::app::develop::step_name::StepName;
use crate::app::develop::tools::targeted_tool::target_button;
use crate::app::ui::design::space;
use crate::app::ui::widgets::{
    parametric_curve_ui, segmented, setting_slider, slider, switched_section, tone_curve_ui,
};
use crate::develop::targeted::{Target, TargetWeights};
use crate::model::panels::Panel;
use crate::model::params::ParameterId;
use crate::model::recipe::Recipe;
use crate::presets::curves::SavedCurve;
use eframe::egui::{self, Sense, Vec2};

/// Draws the panel over the photo's `histogram`. `targeted` is what a Targeted
/// Adjustment Tool drag is moving, `saved_curves` the Point Curve menu's own.
pub(in crate::app::develop::panels::inspector) fn show(
    ui: &mut egui::Ui,
    step: &mut StepName,
    r: &mut Recipe,
    view: &mut ViewState,
    histogram: &[[u32; 256]; 3],
    targeted: Option<TargetWeights>,
    saved_curves: &[SavedCurve],
) -> Vec<Action> {
    let mut actions = Vec::new();
    let mut switch = PanelSwitch::new(r, Panel::ToneCurve);
    if switched_section(ui, step, "Tone Curve", &mut switch.state, |ui, step| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = space::S;
            let (button, _) = ui.allocate_exact_size(Vec2::new(22., 22.), Sense::hover());
            if target_button(
                ui,
                button,
                view.is(Tool::Targeted(Target::ToneCurve)),
                &format!("Targeted adjustment: drag up or down on the photo to move the region of the tone curve there · {}", targeted_shortcut("T")),
            ) {
                actions.push(Action::Targeted(Target::ToneCurve));
            }
            segmented(
                ui,
                &mut view.parametric_curve,
                &[(true, "Parametric"), (false, "Point")],
                112.,
            );
            if !view.parametric_curve {
                let w = ui.available_width();
                segmented(
                    ui,
                    &mut view.selected_curve,
                    &[(0, "RGB"), (1, "R"), (2, "G"), (3, "B")],
                    w,
                );
            }
        });
        ui.add_space(space::XS);
        if view.parametric_curve {
            let moving = targeted.filter(|w| w.target == Target::ToneCurve);
            parametric_curve_ui(
                ui,
                &mut r.effects,
                histogram,
                moving.and_then(|w| (0..4).find(|i| w.shares[*i] > 0.)),
            );
            subheading(ui, "Region");
            // Lightroom lists them lightest first.
            for (i, id) in ParameterId::PARAMETRIC.into_iter().enumerate().rev() {
                let row = ui.push_id(("parametric", i), |ui| {
                    setting_slider(ui, step, id, id.value_mut(r), 0.)
                });
                highlight_targeted(ui, row.response.rect, moving.map_or(0., |w| w.shares[i]));
            }
        } else {
            ui.push_id(view.selected_curve, |ui| {
                tone_curve_ui(
                    ui,
                    if view.selected_curve == 0 {
                        &mut r.curve
                    } else {
                        &mut r.effects.channels[view.selected_curve - 1]
                    },
                    histogram,
                    view.selected_curve,
                );
            });
            // Lightroom's Point Curve menu: the built-in curves, saved ones, Save….
            use crate::presets::curves::ShownCurve;
            let chosen = ShownCurve::of(r, saved_curves);
            let shown = chosen.name(saved_curves).to_string();
            control_row(ui, "Point Curve", |ui| {
                egui::ComboBox::from_id_salt("point-curve")
                    .width(ui.available_width())
                    .selected_text(&shown)
                    .show_ui(ui, |ui| {
                        use crate::presets::curves::BuiltinCurve;
                        for curve in BuiltinCurve::ALL {
                            if ui
                                .selectable_label(
                                    chosen == ShownCurve::Builtin(curve),
                                    curve.name(),
                                )
                                .clicked()
                            {
                                actions.push(Action::Curve(CurveChoice::Builtin(curve)));
                            }
                        }
                        if !saved_curves.is_empty() {
                            ui.separator();
                        }
                        for (i, saved) in saved_curves.iter().enumerate() {
                            if ui
                                .selectable_label(chosen == ShownCurve::Saved(i), &saved.name)
                                .clicked()
                            {
                                actions.push(Action::Curve(CurveChoice::Saved(saved.clone())));
                            }
                        }
                        ui.separator();
                        if ui
                            .selectable_label(false, "Save…")
                            .on_hover_text("Save the RGB, Red, Green and Blue curves under a name")
                            .clicked()
                        {
                            actions.push(Action::Curve(CurveChoice::Save));
                        }
                    });
            });
            // Lightroom's Refine Saturation, under the RGB point curve it acts on;
            // the channel curves keep the same height so nothing below moves.
            subheading(ui, "Refine");
            // Camera Raw renders 101–200 as 100: shown there, and kept as imported
            // until the slider is moved.
            let mut shown = r.curve_saturation.min(1.);
            let before = shown;
            ui.add_enabled_ui(view.selected_curve == 0, |ui| {
                setting_slider(ui, step, ParameterId::CurveSaturation, &mut shown, 1.);
            });
            if shown != before {
                r.curve_saturation = shown;
            }
        }
        subheading(ui, "Levels");
        slider(
            ui,
            step,
            "Black point",
            &mut r.black_point,
            0. ..=r.white_point - 0.01,
            0.,
        );
        slider(
            ui,
            step,
            "White point",
            &mut r.white_point,
            r.black_point + 0.01..=1.,
            1.,
        );
        setting_slider(ui, step, ParameterId::Midtone, &mut r.midtone, 1.);
    }) {
        r.black_point = 0.;
        r.white_point = 1.;
        r.midtone = 1.;
        r.curve = Recipe::default().curve;
        r.effects.channels = std::array::from_fn(|_| Default::default());
        r.curve_saturation = 1.;
        r.effects.parametric = [0.; 4];
    }
    switch.finish(r);
    actions
}
