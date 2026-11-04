//! The Color Mixer panel (HSL, Color and Point Color), or the B&W panel in its
//! place whenever the photo renders black & white, by its Treatment or by a
//! black & white profile.
use super::super::{
    BANDS, PanelSwitch, band_color, control_row, highlight_targeted, hsl_gradient, subheading,
    targeted_shortcut,
};
use super::Action;
use crate::app::develop::state::{MixerTab, PhotoColorSource, Tool, ViewState};
use crate::app::develop::step_name::StepName;
use crate::app::develop::tools::targeted_tool::{hsl_target, target_button};
use crate::app::ui::design::{Tone, space};
use crate::app::ui::theme;
use crate::app::ui::widgets::{segmented, slider_with, switched_section};
use crate::develop::targeted::{Target, TargetWeights};
use crate::model::panels::Panel;
use crate::model::recipe::{Recipe, Treatment};
use eframe::egui::{self, Pos2, Rect, Sense, Stroke, Vec2};

/// Draws the panel. `photo` measures the Auto black & white mix; `targeted` is
/// what a Targeted Adjustment Tool drag is moving.
pub(in crate::app::develop::panels::inspector) fn show(
    ui: &mut egui::Ui,
    step: &mut StepName,
    r: &mut Recipe,
    view: &mut ViewState,
    photo: &PhotoColorSource<'_>,
    targeted: Option<TargetWeights>,
    palette: &theme::Palette,
) -> Vec<Action> {
    let mut actions = Vec::new();
    // The B&W panel replaces the Color Mixer whenever the photo renders black &
    // white, by its Treatment or by a black & white profile.
    let black_white = r.treatment() == Treatment::BlackWhite;
    let (mixer_title, mixer_panel) = if black_white {
        ("B&W", Panel::BlackWhiteMix)
    } else {
        ("Color Mixer", Panel::ColorMixer)
    };
    let mut switch = PanelSwitch::new(r, mixer_panel);
    if switched_section(ui, step, mixer_title, &mut switch.state, |ui, step| {
        if black_white {
            let heading = subheading(ui, "Black & White Mix");
            // Measured (once) only while this panel is open.
            let auto_mix = photo
                .spread()
                .zip(photo.metadata)
                .map(|(spread, metadata)| {
                    crate::develop::AutoMix {
                        spread: &spread,
                        metadata,
                    }
                    .for_recipe(r)
                });
            let button = Rect::from_min_size(
                Pos2::new(heading.right() - 52., heading.top() - 4.),
                Vec2::new(52., 24.),
            );
            // Greyed out while the mix is Auto's, as the Basic panel's Auto.
            let enabled = auto_mix.is_some_and(|mix| mix != r.effects.gray_mix);
            let auto = ui
                .scope_builder(egui::UiBuilder::new().max_rect(button), |ui| {
                    ui.add_enabled(enabled, egui::Button::new("Auto").small())
                })
                .inner;
            if auto
                .on_hover_text("Set the mix from the photo's colors")
                .on_disabled_hover_text("The Auto mix is applied")
                .clicked()
            {
                actions.push(Action::AutoMix);
            }
            let target = Rect::from_min_size(
                Pos2::new(button.left() - 28., button.top() + 1.),
                Vec2::splat(22.),
            );
            if target_button(
                ui,
                target,
                view.is(Tool::Targeted(Target::BlackWhite)),
                &format!(
                    "Targeted adjustment: drag up or down on the photo to brighten or darken its color · {}",
                    targeted_shortcut("G")
                ),
            ) {
                actions.push(Action::Targeted(Target::BlackWhite));
            }
            let moving = targeted.filter(|w| w.target == Target::BlackWhite);
            for (i, name) in BANDS.iter().enumerate() {
                let row = ui.push_id(("bw", i), |ui| {
                    slider_with(
                        ui,
                        step,
                        name,
                        &mut r.effects.gray_mix[i],
                        -1. ..=1.,
                        0.,
                        None,
                        Some((palette.tone(Tone::FIELD), band_color(i))),
                    )
                });
                highlight_targeted(ui, row.response.rect, moving.map_or(0., |w| w.shares[i]));
            }
            return;
        }
        // Lightroom's tabs: the HSL and Color mixer, and Point Color.
        ui.horizontal(|ui| {
            let w = ui.available_width();
            segmented(
                ui,
                &mut view.mixer_tab,
                &[
                    (MixerTab::Mixer, "Mixer"),
                    (MixerTab::PointColor, "Point Color"),
                ],
                w,
            );
        });
        if view.mixer_tab == MixerTab::PointColor {
            crate::app::develop::panels::point_color_panel::point_color_panel(
                ui,
                step,
                &mut r.point_colors,
                view,
            );
            return;
        }
        control_row(ui, "Mixer", |ui| {
            let w = ui.available_width();
            segmented(
                ui,
                &mut view.mixer_color,
                &[(false, "HSL"), (true, "Color")],
                w,
            );
        });
        if view.mixer_color {
            ui.horizontal(|ui| {
                for (i, band) in BANDS.iter().enumerate() {
                    let (rect, response) = ui.allocate_exact_size(Vec2::splat(27.), Sense::click());
                    ui.painter().circle_filled(rect.center(), 7., band_color(i));
                    if view.selected_band == i {
                        ui.painter().circle_stroke(
                            rect.center(),
                            10.,
                            Stroke::new(1.5, palette.tone(Tone::TEXT)),
                        );
                    }
                    if response.on_hover_text(*band).clicked() {
                        view.selected_band = i;
                    }
                }
            });
            let i = view.selected_band;
            ui.push_id(("hsl", i), |ui| {
                for (c, name) in ["Hue", "Saturation", "Luminance"].into_iter().enumerate() {
                    slider_with(
                        ui,
                        step,
                        name,
                        &mut r.hsl[i][c],
                        -1. ..=1.,
                        0.,
                        None,
                        Some(hsl_gradient(palette, i, c)),
                    );
                }
            });
        } else {
            let all = view.mixer_adjust == 3;
            let channel_tip = |c: usize| {
                let name = ["hue", "saturation", "luminance"][c];
                format!(
                    "Targeted adjustment: drag up or down on the photo to change its color's {name} · {}",
                    targeted_shortcut(["H", "S", "L"][c])
                )
            };
            control_row(ui, "Adjust", |ui| {
                ui.spacing_mut().item_spacing.x = space::S;
                if !all {
                    let (button, _) = ui.allocate_exact_size(Vec2::new(22., 22.), Sense::hover());
                    let target = hsl_target(view.mixer_adjust);
                    if target_button(
                        ui,
                        button,
                        view.is(Tool::Targeted(target)),
                        &channel_tip(view.mixer_adjust),
                    ) {
                        actions.push(Action::Targeted(target));
                    }
                }
                let w = ui.available_width();
                segmented(
                    ui,
                    &mut view.mixer_adjust,
                    &[(0, "Hue"), (1, "Sat"), (2, "Lum"), (3, "All")],
                    w,
                );
            });
            // Again after the tabs, which may have just changed.
            let all = view.mixer_adjust == 3;
            let channels = if all {
                0..3
            } else {
                view.mixer_adjust..view.mixer_adjust + 1
            };
            for c in channels {
                let target = hsl_target(c);
                if all {
                    let heading = subheading(ui, ["Hue", "Saturation", "Luminance"][c]);
                    let button = Rect::from_min_size(
                        Pos2::new(heading.right() - 24., heading.top() - 3.),
                        Vec2::splat(22.),
                    );
                    if target_button(ui, button, view.is(Tool::Targeted(target)), &channel_tip(c)) {
                        actions.push(Action::Targeted(target));
                    }
                }
                let moving = targeted.filter(|w| w.target == target);
                for (i, name) in BANDS.iter().enumerate() {
                    let row = ui.push_id(("hsl-all", c, i), |ui| {
                        slider_with(
                            ui,
                            step,
                            name,
                            &mut r.hsl[i][c],
                            -1. ..=1.,
                            0.,
                            None,
                            Some(hsl_gradient(palette, i, c)),
                        );
                    });
                    highlight_targeted(ui, row.response.rect, moving.map_or(0., |w| w.shares[i]));
                }
            }
        }
    }) {
        if black_white {
            r.effects.gray_mix = [0.; 8];
        } else {
            // Both tabs: the mixer and Point Color.
            r.hsl = [[0.; 3]; 8];
            r.point_colors.clear();
        }
    }
    switch.finish(r);
    actions
}
