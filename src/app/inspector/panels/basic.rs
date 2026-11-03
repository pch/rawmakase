//! The Basic panel: Auto, Treatment, Profile (with its imports, the community
//! profile offer and Fit Camera Look), White Balance, Tone and Presence.
use super::super::{
    basic_control, control_row, eyedropper_icon, profile_menu, profile_offer, subheading,
};
use super::Action;
use crate::app::basic_keys;
use crate::app::bulk_import::ImportKind;
use crate::app::design::{Tone, size, space};
use crate::app::profile_download::Offer;
use crate::app::state::{Tool, ViewState};
use crate::app::theme;
use crate::app::widgets::{
    adjustment_section, control_left, segmented, slider_with, toolbar_action,
};
use crate::app::worker::AutoKind;
use crate::camera_data::Metadata;
use crate::camera_profiles::{CameraProfile, Groups};
use crate::model::params::ParameterId;
use crate::model::recipe::{Recipe, Treatment};
use crate::model::white_balance::NamedWhiteBalance;
use eframe::egui::{self, Pos2, Rect, Sense, Vec2};
use std::path::PathBuf;
use std::sync::Arc;

/// What the Basic panel shows of the open photo and the editor this frame.
#[derive(Clone, Copy)]
pub(in crate::app::inspector) struct View<'a> {
    pub metadata: Option<&'a Metadata>,
    pub profiles: &'a [Arc<CameraProfile>],
    pub profile_errors: &'a [String],
    pub profile_groups: &'a Groups,
    /// Adobe's installed profiles for this camera, offered for import.
    pub adobe: &'a [PathBuf],
    /// Whether the catalogue has community profiles for this camera.
    pub community_profiles: bool,
    pub community_offer: Option<&'a Offer>,
    /// A conversion to black & white waiting for the photo to decode.
    pub pending_treatment: Option<Treatment>,
    /// Auto needs the decoded photo, and runs one estimate at a time.
    pub auto_ready: bool,
    pub auto_in_effect: bool,
    pub can_fit_camera_look: bool,
}

/// Draws the panel.
pub(in crate::app::inspector) fn show(
    ui: &mut egui::Ui,
    r: &mut Recipe,
    view: &mut ViewState,
    facts: &View<'_>,
    palette: &theme::Palette,
) -> Vec<Action> {
    let View {
        metadata,
        profiles,
        profile_errors,
        profile_groups,
        adobe,
        community_profiles,
        community_offer,
        pending_treatment,
        auto_ready,
        auto_in_effect,
        can_fit_camera_look,
    } = *facts;
    // A JPEG, TIFF or PNG: its Embedded profile, and relative white balance.
    let rendered = metadata.is_some_and(|m| m.is_rendered());
    let mut actions = Vec::new();
    if adjustment_section(ui, "Basic", |ui| {
        let shortcut = if cfg!(target_os = "macos") {
            "⌘⇧U"
        } else {
            "Ctrl+Shift+U"
        };
        // Auto, right-aligned, styled as the toolbar's Before and Clipping.
        let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.), Sense::hover());
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(row)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
            |ui| {
                let tip = if auto_in_effect {
                    "Auto settings are applied".to_owned()
                } else {
                    format!("Set the tone sliders automatically · {shortcut}")
                };
                let auto = toolbar_action(ui, "Auto", 52., false, auto_ready && !auto_in_effect, 0);
                if auto.on_hover_text(tip).clicked() {
                    actions.push(Action::Auto(AutoKind::Settings));
                }
            },
        );
        // Lightroom's Treatment, above the profile; V switches it.
        // A conversion waiting for the photo to decode shows as made.
        let shown = pending_treatment.unwrap_or_else(|| r.treatment());
        let mut treatment = shown;
        control_row(ui, "Treatment", |ui| {
            let w = ui.available_width();
            segmented(
                ui,
                &mut treatment,
                &[
                    (Treatment::Color, "Color"),
                    (Treatment::BlackWhite, "Black & White"),
                ],
                w,
            );
        });
        if treatment != shown {
            actions.push(Action::Treatment(treatment));
        }
        let old_profile = r.profile.clone();
        // Without a profile the matrix path renders through the DNG default look.
        let matrix = "Default (camera matrix)";
        control_row(ui, "Profile", |ui| {
            egui::ComboBox::from_id_salt("camera-profile")
                .width(ui.available_width())
                // Leave room for the import actions as well as the profiles.
                .height(size::MENU * 12.)
                .selected_text(r.profile.as_ref().map_or_else(
                    || matrix.to_owned(),
                    |p| profile_groups.entry(p).title,
                ))
                .show_ui(ui, |ui| {
                    if !rendered {
                        ui.selectable_value(&mut r.profile, None, matrix);
                    }
                    profile_menu::items(ui, &mut r.profile, profiles, profile_groups);
                    if rendered {
                        return;
                    }
                    ui.separator();
                    if community_profiles
                        && ui
                            .button("Show community profiles for this camera")
                            .on_hover_text("Free camera profiles made for this camera by other projects, which you can download and apply")
                            .clicked()
                    {
                        actions.push(Action::ShowCommunityProfiles);
                        ui.close();
                    }
                    if !adobe.is_empty()
                        && ui
                            .button("Import Adobe profiles for this camera")
                            .on_hover_text(format!(
                                "Adds Adobe Standard and the Camera Matching profiles from Lightroom's installation ({} files), which Adobe Color and the other Adobe looks build on.",
                                adobe.len()
                            ))
                            .clicked()
                    {
                        actions.push(Action::ImportAdobe);
                        ui.close();
                    }
                    if ui
                        .button("Import profiles…")
                        .on_hover_text("Import DCP camera profiles and XMP look profiles. Select the matching base DCP and XMP together.")
                        .clicked()
                    {
                        actions.push(Action::ImportProfiles);
                        ui.close();
                    }
                    if ui
                        .button("Import profiles from folder…")
                        .on_hover_text("Import every DCP profile and XMP look in a folder and its subfolders")
                        .clicked()
                    {
                        actions.push(Action::ImportFolder(ImportKind::CameraProfiles));
                        ui.close();
                    }
                    if ui
                        .button("Import .cube LUTs…")
                        .on_hover_text("Import .cube LUTs as looks, choosing the colour space they expect first")
                        .clicked()
                    {
                        actions.push(Action::ImportLuts);
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(can_fit_camera_look, egui::Button::new("Fit Camera Look"))
                        .on_hover_text("Fits a look to this photo's own JPEG preview, the camera's rendering of it, and uses it as the profile: Camera Look (fitted). It is saved with the edit; fitting again makes a new one.")
                        .clicked()
                    {
                        actions.push(Action::FitCameraLook);
                        ui.close();
                    }
                });
        });
        if let Some(offer) = community_offer
            && let Some(action) = profile_offer::card(ui, offer)
        {
            actions.push(Action::Offer(action));
        }
        // Lightroom's Profile Amount, under the profile. Profiles without one show
        // it dimmed at 100%, so the panel doesn't move when switching.
        let supports_amount = r.profile.as_ref().is_some_and(|p| p.supports_amount());
        ui.add_enabled_ui(supports_amount, |ui| {
            let mut fixed = 1.;
            let amount = if supports_amount {
                &mut r.profile_amount
            } else {
                &mut fixed
            };
            ui.push_id("profile-amount", |ui| {
                slider_with(ui, "Amount", amount, 0. ..=2., 1., Some((100., 0)), None)
            });
        })
        .response
        .on_disabled_hover_text("This profile has no Amount");
        if !profile_errors.is_empty() {
            ui.horizontal(|ui| {
                ui.add_space(control_left(ui));
                ui.small(format!("{} profiles unavailable", profile_errors.len()))
                    .on_hover_text(profile_errors.join("\n"));
            });
            // On a line of its own, so it never widens the panel.
            if !adobe.is_empty() {
                ui.horizontal(|ui| {
                    ui.add_space(control_left(ui));
                    if ui.small_button("Import Adobe base profile").clicked() {
                        actions.push(Action::ImportAdobe);
                    }
                });
            }
        }
        if old_profile != r.profile {
            // A newly chosen profile starts at 100%, as in Lightroom.
            r.profile_amount = 1.;
        }
        if old_profile != r.profile
            && let Some(m) = metadata
        {
            r.profile_changed(m);
            actions.push(Action::ProfileChanged(old_profile));
        }
        // White balance is its own group below the profile, as in Lightroom.
        ui.add_space(space::L);
        let as_shot = metadata.map(|m| {
            let mut shot = r.clone();
            shot.reset_white_balance(m);
            (shot.temperature, shot.tint)
        });
        let photo = metadata;
        // A monochrome photo has no white balance: Lightroom disables it.
        let white_balance = basic_keys::enabled(ParameterId::Temperature, photo);
        ui.add_enabled_ui(white_balance, |ui| {
            control_row(ui, "WB", |ui| {
                // A rendered file's white balance is relative Temp and Tint, which
                // Auto records too; As Shot is 0 and 0.
                let (current, as_shot) = if rendered {
                    (
                        (r.incremental_temperature, r.incremental_tint),
                        Some((0., 0.)),
                    )
                } else {
                    ((r.temperature, r.tint), as_shot)
                };
                let near = |(t, n): (f32, f32)| {
                    (t - current.0).abs() < 1. && (n - current.1).abs() < 0.5
                };
                let selected = if r.auto_white_balance.is_some_and(|[t, n]| near((t, n))) {
                    "Auto"
                } else if as_shot.is_some_and(near) {
                    "As Shot"
                } else if rendered {
                    "Custom"
                } else {
                    NamedWhiteBalance::ALL
                        .into_iter()
                        .find(|w| near((w.values().temperature, w.values().tint)))
                        .map_or("Custom", NamedWhiteBalance::name)
                };
                // The selector sits at the row's far left, so the menu lines up
                // with Profile's.
                let rect = Rect::from_center_size(
                    Pos2::new(
                        ui.max_rect().left() - control_left(ui) + 13.,
                        ui.max_rect().center().y,
                    ),
                    Vec2::new(26., 20.),
                );
                let response = ui.interact(rect, ui.id().with("wb-selector"), Sense::click());
                let picking = view.is(Tool::WhiteBalance);
                if picking || response.hovered() {
                    ui.painter().rect_filled(
                        rect,
                        3.,
                        palette.tone(if picking {
                            Tone::PRESSED
                        } else {
                            Tone::OUTLINE
                        }),
                    );
                }
                eyedropper_icon(ui.painter(), rect.center(), picking || response.hovered());
                if response
                    .on_hover_text(
                        "White balance selector (W): click a neutral area of the photo",
                    )
                    .clicked()
                {
                    view.toggle(Tool::WhiteBalance);
                }
                egui::ComboBox::from_id_salt("white-balance")
                    .width(ui.available_width())
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(selected == "As Shot", "As Shot")
                            .clicked()
                            && let Some(m) = metadata
                        {
                            r.wb = [1.; 3];
                            r.reset_white_balance(m);
                        }
                        if ui
                            .add_enabled(
                                auto_ready,
                                egui::Button::selectable(selected == "Auto", "Auto"),
                            )
                            .on_hover_text("Make the photo's near-neutral areas neutral")
                            .clicked()
                        {
                            actions.push(Action::Auto(AutoKind::WhiteBalance));
                        }
                        // Kelvin presets are for RAW files, as in Lightroom.
                        let named = if rendered {
                            &[][..]
                        } else {
                            &NamedWhiteBalance::ALL[..]
                        };
                        // A fitted matrix base reads As Shot as D65 whatever the
                        // light: its temperatures are relative, so named whites
                        // would be wrong (imported XMP keeps them as written).
                        let uncalibrated = r
                            .profile
                            .as_ref()
                            .is_some_and(|p| crate::camera_profiles::fitted::is_uncalibrated_base(p));
                        for &named in named {
                            if ui
                                .add_enabled(
                                    !uncalibrated,
                                    egui::Button::selectable(selected == named.name(), named.name()),
                                )
                                .on_disabled_hover_text("This photo has no colour data of its own: its Camera Look (fitted) knows the white as shot, not the light's temperature. As Shot, Auto, the selector and Temp and Tint still work.")
                                .clicked()
                                && let Some(m) = metadata
                            {
                                let values = named.values();
                                r.temperature = values.temperature;
                                r.tint = values.tint;
                                r.update_wb(m);
                                r.auto_white_balance = None;
                            }
                        }
                        ui.add_enabled(
                            false,
                            egui::Button::selectable(selected == "Custom", "Custom"),
                        );
                    });
            });
            let selected = &mut view.selected_setting;
            if rendered {
                let (temp, tint) = (
                    ParameterId::IncrementalTemperature,
                    ParameterId::IncrementalTint,
                );
                basic_control(ui, r, temp, 0., photo, selected);
                basic_control(ui, r, tint, 0., photo, selected);
            } else {
                basic_control(ui, r, ParameterId::Temperature, 6500., photo, selected);
                basic_control(ui, r, ParameterId::Tint, 0., photo, selected);
            }
        });
        subheading(ui, "Tone");
        for id in [
            ParameterId::Exposure,
            ParameterId::Contrast,
            ParameterId::Highlights,
            ParameterId::Shadows,
            ParameterId::Whites,
            ParameterId::Blacks,
        ] {
            basic_control(ui, r, id, 0., photo, &mut view.selected_setting);
        }
        subheading(ui, "Presence");
        for id in [
            ParameterId::Texture,
            ParameterId::Clarity,
            ParameterId::Dehaze,
            ParameterId::Vibrance,
            ParameterId::Saturation,
        ] {
            basic_control(ui, r, id, 0., photo, &mut view.selected_setting);
        }
    }) {
        r.wb = [1.; 3];
        r.tint = 0.;
        if let Some(m) = metadata {
            r.reset_white_balance(m);
        }
        r.effects.monochrome = false;
        r.effects.texture = 0.;
        r.effects.clarity = 0.;
        r.effects.dehaze = 0.;
        r.vibrance = 0.;
        r.saturation = 0.;
        r.exposure = Recipe::default().exposure;
        r.contrast = 0.;
        r.highlights = 0.;
        r.shadows = 0.;
        r.whites = 0.;
        r.blacks = 0.;
    }
    actions
}
