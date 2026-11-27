//! The Tone Curve panel: Parametric and Point curves, Refine Saturation and
//! Levels.
use super::super::{
    PanelSwitch, highlight_targeted, stacked_control, subheading, targeted_shortcut,
};
use super::Action;
use crate::app::develop::panels::curve_menu::CurveChoice;
use crate::app::develop::state::{Tool, ViewState};
use crate::app::develop::step_name::StepName;
use crate::app::develop::tools::targeted_tool::target_button;
use crate::app::ui::design::space;
use crate::app::ui::widgets::{
    CURVE_CHANNEL_NAMES, CURVE_CHANNELS_WIDTH, CurveMenu, CurveMenuChoice, curve_channels,
    parametric_curve_ui, segmented, setting_slider, slider, switched_section, tone_curve_ui,
};
use crate::color::curve::ToneCurve;
use crate::develop::targeted::{Target, TargetWeights};
use crate::model::panels::Panel;
use crate::model::params::ParameterId;
use crate::model::recipe::Recipe;
use crate::presets::curves::SavedCurve;
use eframe::egui::{self, Sense, Vec2};

/// Draws the panel over `histogram`, of what the curves take in, asked for only
/// while the panel is open. `targeted` is what a Targeted Adjustment Tool drag is
/// moving, `saved_curves` the Point Curve menu's own.
pub(in crate::app::develop::panels::inspector) fn show(
    ui: &mut egui::Ui,
    step: &mut StepName,
    r: &mut Recipe,
    view: &mut ViewState,
    histogram: impl FnOnce() -> [[u32; 256]; 3],
    targeted: Option<TargetWeights>,
    saved_curves: &[SavedCurve],
) -> Vec<Action> {
    let mut actions = Vec::new();
    let mut switch = PanelSwitch::new(r, Panel::ToneCurve);
    if switched_section(ui, step, "Tone Curve", &mut switch.state, |ui, step| {
        // One toolbar: the targeted tool, the curve's kind and, for the point
        // curve, its channel as colored dots at the end.
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
            // The channels' room is kept first: in a narrow panel the kind's
            // segments shrink rather than push the dots past the edge.
            let room = if view.parametric_curve {
                ui.available_width()
            } else {
                ui.available_width() - CURVE_CHANNELS_WIDTH - space::S
            };
            ui.allocate_ui(Vec2::new(room.max(0.), button.height()), |ui| {
                segmented(
                    ui,
                    &mut view.parametric_curve,
                    &[(true, "Parametric"), (false, "Point")],
                    room.min(132.),
                );
            });
            if !view.parametric_curve {
                // At the row's end, in RGB, R, G, B order.
                ui.add_space((ui.available_width() - CURVE_CHANNELS_WIDTH).max(0.));
                ui.spacing_mut().item_spacing.x = 0.;
                curve_channels(ui, &mut view.selected_curve);
            }
        });
        ui.add_space(space::M);
        if view.parametric_curve {
            let moving = targeted.filter(|w| w.target == Target::ToneCurve);
            parametric_curve_ui(
                ui,
                &mut r.effects,
                &histogram(),
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
            let menu = CurveMenu {
                any_changed: point_curves(r).iter().any(|c| **c != ToneCurve::default()),
                copied: view.copied_curve.as_ref().map(|(from, _)| *from),
            };
            let channel = view.selected_curve;
            let chosen = ui
                .push_id(channel, |ui| {
                    tone_curve_ui(ui, point_curves(r)[channel], &histogram(), channel, menu)
                })
                .inner;
            if let Some(choice) = chosen {
                curve_menu_choice(step, r, view, choice);
            }
            // Lightroom's Point Curve menu: the built-in curves, saved ones, Save….
            use crate::presets::curves::ShownCurve;
            let chosen = ShownCurve::of(r, saved_curves);
            let shown = chosen.name(saved_curves).to_string();
            stacked_control(ui, "Point Curve", |ui| {
                crate::app::ui::widgets::named_menu(
                    crate::app::ui::widgets::dropdown(ui, "point-curve")
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
                                .on_hover_text(
                                    "Save the RGB, Red, Green and Blue curves under a name",
                                )
                                .clicked()
                            {
                                actions.push(Action::Curve(CurveChoice::Save));
                            }
                        }),
                    "Point Curve",
                );
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

/// The point curves, RGB first, then Red, Green and Blue.
fn point_curves(r: &mut Recipe) -> [&mut ToneCurve; 4] {
    let [red, green, blue] = &mut r.effects.channels;
    [&mut r.curve, red, green, blue]
}

/// Carries out a choice from the point curve's right-click menu on the channel
/// shown, as Lightroom's Reset Channel, Reset All Channels and Copy and Paste
/// Channel Settings do. Each change is one "Point Curve" step; copying changes
/// nothing.
fn curve_menu_choice(
    step: &mut StepName,
    r: &mut Recipe,
    view: &mut ViewState,
    choice: CurveMenuChoice,
) {
    let channel = view.selected_curve;
    let name = CURVE_CHANNEL_NAMES[channel];
    let value = match choice {
        CurveMenuChoice::Copy => {
            view.copied_curve = Some((channel, point_curves(r)[channel].clone()));
            return;
        }
        CurveMenuChoice::Paste => {
            let Some((from, curve)) = view.copied_curve.clone() else {
                return;
            };
            let curves = point_curves(r);
            if *curves[channel] == curve {
                return;
            }
            *curves[channel] = curve;
            format!("Paste {} into {name}", CURVE_CHANNEL_NAMES[from])
        }
        CurveMenuChoice::ResetChannel => {
            let curves = point_curves(r);
            if *curves[channel] == ToneCurve::default() {
                return;
            }
            *curves[channel] = ToneCurve::default();
            format!("Reset {name} Channel")
        }
        CurveMenuChoice::ResetAll => {
            let mut changed = false;
            for curve in point_curves(r) {
                if *curve != ToneCurve::default() {
                    *curve = ToneCurve::default();
                    changed = true;
                }
            }
            if !changed {
                return;
            }
            "Reset All Channels".into()
        }
    };
    step.name("Point Curve".into(), value);
}
