//! The naming templates a dialog offers (Export's Rename To, Rename
//! Photos' File Naming): Lightroom's presets, RAWmakase's own templates, the
//! user's saved ones, the template made in the editor, and Edit… to make
//! one.
use super::presets::{Library, Preset};
use crate::export_settings::{BUILTIN, Naming};
use eframe::egui;

/// What was picked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) enum Choice {
    Preset(Naming),
    /// A template in Lightroom's form.
    Tokens(String),
    Edit,
}

/// The menu, `width` wide, for `naming`, or for `custom` (Lightroom's form)
/// when a template names files in its place: what was picked, if anything.
pub(in crate::app) fn combo(
    ui: &mut egui::Ui,
    id_salt: &str,
    width: f32,
    naming: Naming,
    custom: Option<&str>,
    library: &Library,
) -> Option<Choice> {
    let current = Preset::of(naming, custom, library);
    let mut picked = None;
    egui::ComboBox::from_id_salt(id_salt)
        .width(width)
        .selected_text(current.label())
        .show_ui(ui, |ui| {
            for n in Naming::ALL {
                if ui
                    .selectable_label(current == Preset::Naming(n), n.label())
                    .clicked()
                {
                    picked = Some(Choice::Preset(n));
                }
            }
            ui.separator();
            for b in &BUILTIN {
                if ui
                    .selectable_label(current == Preset::Builtin(b), b.name)
                    .clicked()
                {
                    picked = Some(Choice::Tokens(b.tokens.into()));
                }
            }
            let saved = library.list();
            if !saved.is_empty() {
                ui.separator();
                for t in saved {
                    let on = current == Preset::Saved(t.name.clone());
                    if ui.selectable_label(on, &t.name).clicked() {
                        picked = Some(Choice::Tokens(t.tokens));
                    }
                }
            }
            if current == Preset::Custom {
                let _ = ui.selectable_label(true, "Custom Settings");
            }
            ui.separator();
            if ui.selectable_label(false, "Edit…").clicked() {
                picked = Some(Choice::Edit);
            }
        });
    picked
}
