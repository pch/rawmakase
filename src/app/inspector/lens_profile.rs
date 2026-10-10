//! Lens Corrections › Profile: Lightroom's Setup, Make, Model and Profile menus over
//! the imported Adobe profiles that fit the photo's camera.
use super::control_row;
use crate::app::theme;
use crate::camera_data::Metadata;
use crate::lens::choice::{LensProfileSetup, ProfileMenus};
use crate::model::recipe::Recipe;
use eframe::egui;

/// Draws the menus, or the profile in use as text when no imported profile fits.
pub(super) fn profile_menus(ui: &mut egui::Ui, r: &mut Recipe, m: Option<&Metadata>) {
    let Some(m) = m.filter(|m| !m.lens_profiles.all().is_empty()) else {
        // Setup stays, so an edit naming a profile that isn't imported can go back to
        // Default or Auto.
        ui.add_enabled_ui(r.lens_profile, |ui| setup_row(ui, r, None));
        let builtin = m.and_then(|m| m.lens.as_ref());
        control_row(ui, "Profile", |ui| {
            muted(
                ui,
                builtin.map_or("No matching profile", |l| l.source.as_str()),
            );
        });
        return;
    };
    // What the choice uses, shown also while Enable Profile Corrections is off.
    let resolved = r.lens_profile_choice.resolve(&m.lens_profiles, m);
    let in_use = resolved.used.map(|c| std::sync::Arc::clone(&c.profile));
    // A profile that isn't imported, or the one the RAW carries, shown by name.
    let named = resolved
        .missing
        .or(r.lens_profile_choice.id.as_ref().filter(|id| id.embedded))
        .map(|id| id.label().to_string());
    let menus = ProfileMenus::new(&m.lens_profiles, m);
    let enabled = r.lens_profile;
    ui.add_enabled_ui(enabled, |ui| {
        setup_row(ui, r, in_use.as_deref());
        let (make, model, name) = match (&in_use, &named) {
            (Some(p), _) => (p.lens_make.as_str(), p.lens_model.as_str(), p.name.as_str()),
            (None, Some(named)) => ("", "", named.as_str()),
            (None, None) => ("", "", "None"),
        };
        let mut chosen = None;
        control_row(ui, "Make", |ui| {
            egui::ComboBox::from_id_salt("lens-profile-make")
                .width(ui.available_width())
                .selected_text(make)
                .show_ui(ui, |ui| {
                    for item in menus.makes() {
                        if ui.selectable_label(item == make, item).clicked() {
                            chosen = menus.first_of_make(item);
                        }
                    }
                });
        });
        control_row(ui, "Model", |ui| {
            egui::ComboBox::from_id_salt("lens-profile-model")
                .width(ui.available_width())
                .selected_text(model)
                .show_ui(ui, |ui| {
                    for item in menus.models(make) {
                        if ui.selectable_label(item == model, item).clicked() {
                            chosen = menus.first_of_model(make, item);
                        }
                    }
                });
        });
        control_row(ui, "Profile", |ui| {
            egui::ComboBox::from_id_salt("lens-profile-name")
                .width(ui.available_width())
                .selected_text(name)
                .show_ui(ui, |ui| {
                    for p in menus.profiles(make, model) {
                        let current = in_use.as_ref().is_some_and(|u| u.filename == p.filename);
                        if ui
                            .selectable_label(current, &p.name)
                            .on_hover_text(&p.filename)
                            .clicked()
                        {
                            chosen = Some(p);
                        }
                    }
                });
        });
        if let Some(p) = chosen {
            r.lens_profile_choice.choose(p);
        }
    });
}

/// Lightroom's Setup menu: Default, Auto or Custom.
fn setup_row(
    ui: &mut egui::Ui,
    r: &mut Recipe,
    in_use: Option<&crate::optics::lcp::ImportedProfile>,
) {
    control_row(ui, "Setup", |ui| {
        let mut setup = r.lens_profile_choice.setup;
        egui::ComboBox::from_id_salt("lens-profile-setup")
            .width(ui.available_width())
            .selected_text(setup.label())
            .show_ui(ui, |ui| {
                for s in LensProfileSetup::ALL {
                    ui.selectable_value(&mut setup, s, s.label());
                }
            });
        if setup != r.lens_profile_choice.setup {
            r.lens_profile_choice.set_setup(setup, in_use);
        }
    });
}

fn muted(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(11.)
            .color(theme::palette(ui.ctx()).gray(200)),
    );
}
