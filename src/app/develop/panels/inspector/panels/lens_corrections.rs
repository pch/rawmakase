//! The Lens Corrections panel: Profile (Adobe lens profiles, the lens data in
//! the RAW, and Lensfun's, `inspector::lens_profile` and `inspector::lensfun`),
//! then Lightroom's Manual tab: Distortion, Defringe and Vignetting.
use super::super::{
    PanelSwitch, control_row, defringe_sliders, eyedropper_icon, lens_profile, lensfun,
    setting_control, subheading,
};
use super::Action;
use crate::app::develop::state::{Tool, ViewState};
use crate::app::develop::step_name::StepName;
use crate::app::presets::bulk_import::ImportKind;
use crate::app::ui::design::{Tone, radius, text};
use crate::app::ui::theme;
use crate::app::ui::widgets::switched_section;
use crate::camera_data::Metadata;
use crate::model::panels::Panel;
use crate::model::params::ParameterId;
use crate::model::recipe::Recipe;
use eframe::egui::{self, Pos2, Rect, Sense, Vec2};

/// Draws the panel for photo `metadata`; `lensfun_view` is what Lensfun's
/// database holds this frame.
pub(in crate::app::develop::panels::inspector) fn show(
    ui: &mut egui::Ui,
    step: &mut StepName,
    r: &mut Recipe,
    view: &mut ViewState,
    metadata: Option<&Metadata>,
    lensfun_view: &lensfun::View,
    palette: &theme::Palette,
) -> Vec<Action> {
    let mut actions = Vec::new();
    let mut switch = PanelSwitch::new(r, Panel::LensCorrections);
    if switched_section(
        ui,
        step,
        "Lens Corrections",
        &mut switch.state,
        |ui, step| {
            let photo = metadata;
            subheading(ui, "Profile");
            control_row(ui, "", |ui| {
                ui.checkbox(&mut r.lens_ca, "Remove Chromatic Aberration")
                .on_hover_text("Remove red/cyan and blue/yellow fringes toward the edges of the frame, measured from the photo itself.");
            });
            control_row(ui, "", |ui| {
                let mut on = r.lens_profile;
                if ui
                .checkbox(&mut on, "Enable Profile Corrections")
                .on_hover_text("Correct distortion and vignetting with an Adobe lens profile, or the lens data the camera stored in the RAW.")
                .changed()
                && let Some(m) = metadata
            {
                let state = if on {
                    crate::model::recipe::ProfileCorrections::On
                } else {
                    crate::model::recipe::ProfileCorrections::Off
                };
                r.set_profile_corrections(m, state);
            }
            });
            let lens = metadata
                .map(|m| m.lens_model.clone())
                .filter(|l| !l.is_empty());
            control_row(ui, "Lens", |ui| {
                ui.label(
                    egui::RichText::new(lens.as_deref().unwrap_or("Unknown"))
                        .size(text::SMALL)
                        .color(palette.tone(Tone::SOFT)),
                );
            });
            lens_profile::profile_menus(ui, r, metadata, lensfun_view);
            // Said here as well as on import, so a missing profile is never silent.
            if let Some(missing) = metadata.and_then(|m| r.missing_lens_profile(m)) {
                control_row(ui, "", |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(missing)
                                .size(text::SMALL)
                                .color(palette.tone(Tone::SECONDARY)),
                        )
                        .wrap(),
                    );
                });
            }
            if let Some(m) = metadata
                && let Some(action) = lensfun::card(ui, r, m, lensfun_view)
            {
                actions.push(Action::Lensfun(action));
            }
            if r.lens_profile {
                subheading(ui, "Amount");
                ui.push_id("lens-amount", |ui| {
                    setting_control(ui, step, r, ParameterId::LensDistortion, 1., photo);
                    setting_control(ui, step, r, ParameterId::LensVignetting, 1., photo);
                });
            }
            control_row(ui, "", |ui| {
                if ui
                .small_button("Import Lens Profiles…")
                .on_hover_text("Import Adobe .lcp lens profiles, e.g. from /Library/Application Support/Adobe/CameraRaw/LensProfiles")
                .clicked()
            {
                actions.push(Action::ImportLens);
            }
                if ui
                    .small_button("Import Folder…")
                    .on_hover_text("Import every .lcp lens profile in a folder and its subfolders")
                    .clicked()
                {
                    actions.push(Action::ImportFolder(ImportKind::LensProfiles));
                }
            });
            // Lightroom's Manual tab: Distortion, then Defringe and Vignetting.
            subheading(ui, "Distortion");
            ui.push_id("manual-distortion", |ui| {
                setting_control(ui, step, r, ParameterId::ManualDistortion, 0., photo);
            });
            let row = subheading(ui, "Defringe");
            // The Fringe Color Selector, at the row's far left as White Balance's.
            let rect = Rect::from_center_size(
                Pos2::new(row.left() + 13., row.center().y),
                Vec2::new(26., 20.),
            );
            let response = ui.interact(rect, ui.id().with("fringe-selector"), Sense::click());
            let picking = view.is(Tool::Defringe);
            if picking || response.hovered() {
                ui.painter().rect_filled(
                    rect,
                    radius::CONTROL,
                    palette.tone(if picking {
                        Tone::PRESSED
                    } else {
                        Tone::OUTLINE
                    }),
                );
            }
            eyedropper_icon(ui.painter(), rect.center(), picking || response.hovered());
            if response
                .on_hover_text("Fringe color selector: click a purple or green fringe in the photo")
                .clicked()
            {
                view.toggle(Tool::Defringe);
            }
            defringe_sliders(ui, step, &mut r.effects);
            subheading(ui, "Vignetting");
            ui.push_id("lens-vignette", |ui| {
                setting_control(ui, step, r, ParameterId::LensVignetteAmount, 0., photo);
                setting_control(ui, step, r, ParameterId::LensVignetteMidpoint, 0.5, photo);
            });
        },
    ) {
        if let Some(m) = metadata {
            r.lens_builtin = m.lens.as_ref().is_none_or(|l| l.default_on);
        }
        let defaults = Recipe::default();
        r.lens_ca = defaults.lens_ca;
        r.lens_profile = defaults.lens_profile;
        r.lens_profile_choice = defaults.lens_profile_choice;
        r.lens_distortion = defaults.lens_distortion;
        r.lens_vignetting = defaults.lens_vignetting;
        r.lens_manual_distortion = defaults.lens_manual_distortion;
        let d = Recipe::default().effects;
        r.effects.defringe = d.defringe;
        r.effects.defringe_ranges = d.defringe_ranges;
        r.effects.lens_vignette = d.lens_vignette;
        r.effects.lens_vignette_midpoint = d.lens_vignette_midpoint;
    }
    switch.finish(r);
    actions
}
