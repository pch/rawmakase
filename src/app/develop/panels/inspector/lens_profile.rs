//! Lens Corrections › Profile: Lightroom's Setup, Make, Model and Profile menus over
//! the imported Adobe profiles that fit the photo's camera, then the Lensfun lenses
//! that do, when Lensfun's database is installed (`lensfun`).
use super::control_row;
use super::lensfun::{View, choices, choose, profile_label};
use crate::app::ui::design::{Tone, text};
use crate::app::ui::theme;
use crate::camera_data::Metadata;
use crate::lens::choice::{LensProfileSetup, ProfileMenus};
use crate::lens::lensfun::Lens;
use crate::model::recipe::Recipe;
use eframe::egui;

/// An item a menu can choose.
enum Pick<'a> {
    Adobe(&'a crate::optics::lcp::ImportedProfile),
    Lensfun(&'a Lens),
}

/// Draws the menus, or the profile in use as text when nothing fits.
pub(super) fn profile_menus(ui: &mut egui::Ui, r: &mut Recipe, m: Option<&Metadata>, view: &View) {
    let lensfun: Vec<&Lens> = match (m, &view.db) {
        (Some(m), Some(db)) if !m.is_rendered() => choices(db, m),
        _ => Vec::new(),
    };
    let chosen = r.lens_profile_choice.lensfun().cloned();
    let Some(m) =
        m.filter(|m| !m.lens_profiles.all().is_empty() || !lensfun.is_empty() || chosen.is_some())
    else {
        // Setup stays, so an edit naming a profile that isn't imported can go back to
        // Default or Auto.
        ui.add_enabled_ui(r.lens_profile, |ui| setup_row(ui, r, None));
        let builtin = m.and_then(|m| m.lens_stand_in.as_ref().or(m.lens.as_ref()));
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
        .or(r.lens_profile_choice.adobe().filter(|id| id.embedded))
        .map(|id| id.label().to_string());
    let menus = ProfileMenus::new(&m.lens_profiles, m);
    // Lensfun's makers and models as shown, after the imported profiles'.
    let maker = |l: &Lens| l.maker.shown.clone();
    let model = |l: &Lens| l.model.shown.clone();
    let chosen_lens = chosen.as_ref().and_then(|c| {
        view.db
            .as_ref()
            .and_then(|db| db.db.find(&c.lens))
            .filter(|l| lensfun.iter().any(|x| std::ptr::eq(*x, *l)))
    });
    let enabled = r.lens_profile;
    ui.add_enabled_ui(enabled, |ui| {
        setup_row(ui, r, in_use.as_deref());
        let (make, model_name, name) = match (&chosen, &in_use, &named) {
            (Some(c), _, _) => {
                let make = chosen_lens.map_or_else(|| c.lens.maker.clone(), maker);
                let model_name = chosen_lens.map_or_else(|| c.name().to_string(), model);
                let name = match chosen_lens {
                    Some(l) => profile_label(l, &lensfun),
                    None if c.is_resolved() => "Lensfun".into(),
                    None => "Lensfun (not resolved)".into(),
                };
                (make, model_name, name)
            }
            (None, Some(p), _) => (p.lens_make.clone(), p.lens_model.clone(), p.name.clone()),
            (None, None, Some(named)) => (String::new(), String::new(), named.clone()),
            (None, None, None) => (String::new(), String::new(), "None".into()),
        };
        let mut picked: Option<Pick> = None;
        control_row(ui, "Make", |ui| {
            egui::ComboBox::from_id_salt("lens-profile-make")
                .width(ui.available_width())
                .selected_text(&make)
                .show_ui(ui, |ui| {
                    let adobe = menus.makes();
                    for item in &adobe {
                        if ui.selectable_label(*item == make, *item).clicked() {
                            picked = menus.first_of_make(item).map(Pick::Adobe);
                        }
                    }
                    let mut seen: Vec<String> = adobe.iter().map(|s| s.to_string()).collect();
                    for lens in &lensfun {
                        let item = maker(lens);
                        if seen.contains(&item) {
                            continue;
                        }
                        if ui.selectable_label(item == make, &item).clicked() {
                            picked = Some(Pick::Lensfun(lens));
                        }
                        seen.push(item);
                    }
                });
        });
        control_row(ui, "Model", |ui| {
            egui::ComboBox::from_id_salt("lens-profile-model")
                .width(ui.available_width())
                .selected_text(&model_name)
                .height(320.)
                .show_ui(ui, |ui| {
                    let adobe = menus.models(&make);
                    for item in &adobe {
                        if ui.selectable_label(*item == model_name, *item).clicked() {
                            picked = menus.first_of_model(&make, item).map(Pick::Adobe);
                        }
                    }
                    let mut seen: Vec<String> = adobe.iter().map(|s| s.to_string()).collect();
                    for lens in lensfun.iter().filter(|l| maker(l) == make) {
                        let item = model(lens);
                        if seen.contains(&item) {
                            continue;
                        }
                        if ui.selectable_label(item == model_name, &item).clicked() {
                            picked = Some(Pick::Lensfun(lens));
                        }
                        seen.push(item);
                    }
                });
        });
        control_row(ui, "Profile", |ui| {
            egui::ComboBox::from_id_salt("lens-profile-name")
                .width(ui.available_width())
                .selected_text(&name)
                .show_ui(ui, |ui| {
                    for p in menus.profiles(&make, &model_name) {
                        let current = in_use.as_ref().is_some_and(|u| u.filename == p.filename);
                        if ui
                            .selectable_label(current, &p.name)
                            .on_hover_text(&p.filename)
                            .clicked()
                        {
                            picked = Some(Pick::Adobe(p));
                        }
                    }
                    for lens in lensfun
                        .iter()
                        .filter(|l| maker(l) == make && model(l) == model_name)
                    {
                        let current = chosen_lens.is_some_and(|c| std::ptr::eq(c, *lens));
                        if ui
                            .selectable_label(current, profile_label(lens, &lensfun))
                            .on_hover_text("Lensfun community profile, CC BY-SA 3.0")
                            .clicked()
                        {
                            picked = Some(Pick::Lensfun(lens));
                        }
                    }
                });
        });
        match picked {
            Some(Pick::Adobe(p)) => r.lens_profile_choice.choose(p),
            Some(Pick::Lensfun(lens)) => {
                if let Some(db) = &view.db {
                    choose(r, m, db, lens);
                }
            }
            None => {}
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
            .size(text::SMALL)
            .color(theme::palette(ui.ctx()).tone(Tone::SOFT)),
    );
}
