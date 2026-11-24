//! Preferences > External Editing: the programs Edit In opens photos in, and
//! the files it makes for them.
use super::{gap, group, hint};
use crate::app::Editor;
use crate::app::shell::dialogs::FileDialog;
use crate::app::ui::widgets::form_row;
use crate::color::OutputSpace;
use eframe::egui;
use std::path::PathBuf;

/// The color spaces offered, widest first.
const SPACES: [OutputSpace; 3] = [
    OutputSpace::ProPhoto,
    OutputSpace::AdobeRgb,
    OutputSpace::Srgb,
];

/// What a click on an editor's row asked for.
enum Change {
    Up(usize),
    Remove(usize),
}

impl Editor {
    pub(super) fn external_editing_page(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        group(ui, "External editors");
        let editors = self.edit_in.editors().to_vec();
        let mut change = None;
        for (i, editor) in editors.iter().enumerate() {
            let label = if i == 0 { "Edit In" } else { "" };
            form_row(ui, label, |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Remove").clicked() {
                        change = Some(Change::Remove(i));
                    }
                    if ui
                        .add_enabled(i > 0, egui::Button::new("Move Up"))
                        .clicked()
                    {
                        change = Some(Change::Up(i));
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        let name = crate::platform::open_with::program_name(editor);
                        let shortcut = if i == 0 {
                            format!("  {}", crate::app::library::EDIT_IN_SHORTCUT)
                        } else {
                            String::new()
                        };
                        super::value(ui, &format!("{name}{shortcut}"));
                    });
                });
            });
            form_row(ui, "", |ui| {
                hint(ui, &crate::app::ui::widgets::pretty_path(editor));
            });
        }
        let mut add = false;
        form_row(ui, if editors.is_empty() { "Edit In" } else { "" }, |ui| {
            add = ui.button("Add Editor…").clicked();
        });
        form_row(ui, "", |ui| {
            hint(
                ui,
                if editors.is_empty() {
                    "Add a program to open photos in from a photo's Edit In menu: Photoshop, Affinity Photo, GIMP or any other that edits TIFF files."
                } else {
                    "The first editor opens with the shortcut; the photo's Edit In menu lists them all."
                },
            );
        });
        match change {
            Some(Change::Up(i)) => {
                let mut editors = editors;
                editors.swap(i - 1, i);
                self.set_external_editors(editors);
            }
            Some(Change::Remove(i)) => {
                let mut editors = editors;
                editors.remove(i);
                self.set_external_editors(editors);
            }
            None => {}
        }
        if add {
            self.dialog(FileDialog::ExternalEditor, &ctx);
        }
        gap(ui);
        group(ui, "Edit In files");
        let mut space = self.edit_in.space;
        form_row(ui, "File Format", |ui| {
            super::value(ui, "TIFF, 16 bits per channel")
        });
        form_row(ui, "Color Space", |ui| {
            crate::app::ui::widgets::named_menu(
                crate::app::ui::widgets::dropdown(ui, "edit-in-color-space")
                    .width(220.)
                    .selected_text(space.label())
                    .show_ui(ui, |ui| {
                        for choice in SPACES {
                            ui.selectable_value(&mut space, choice, choice.label());
                        }
                    }),
                "Color Space",
            );
        });
        form_row(ui, "", |ui| {
            hint(
                ui,
                "Edit In saves a copy of the photo with its edit beside the original, named like DSC_0001-Edit.tif, stacks it on top of the original and opens it in the editor. When the editor saves it, it is updated here as you come back.",
            );
        });
        if space != self.edit_in.space {
            self.edit_in.space = space;
            let _ = self.save_session();
        }
    }
    /// Offers `editors` for Edit In, in this order, and saves them.
    pub(in crate::app) fn set_external_editors(&mut self, editors: Vec<PathBuf>) {
        self.edit_in.set_editors(editors);
        if let Err(e) = self.save_session() {
            self.messages
                .fail(format!("External editors not saved: {e:#}"));
        }
    }
    /// A program chosen with Add Editor…, added at the end unless it is
    /// there already.
    pub(in crate::app) fn add_external_editor(&mut self, program: PathBuf) {
        let mut editors = self.edit_in.editors().to_vec();
        if !editors.contains(&program) {
            editors.push(program);
            self.set_external_editors(editors);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::preferences::Tab;
    use crate::app::preferences::session::Session;

    fn editor_with(editors: Vec<PathBuf>) -> Editor {
        let ctx = egui::Context::default();
        Editor::with_context(
            &ctx,
            None,
            Session {
                external_editors: editors,
                ..Default::default()
            },
            None,
        )
    }

    /// The page with two editors and Adobe RGB chosen, and with none, for
    /// `RAWMAKASE_UI_SHOTS` ([`crate::app::ui::shots`]).
    #[test]
    fn shots_of_external_editing() {
        let apps = if cfg!(windows) {
            [
                r"C:\Program Files\Adobe\Photoshop.exe",
                r"C:\Program Files\GIMP\gimp.exe",
            ]
        } else {
            [
                "/Applications/Adobe Photoshop 2026/Adobe Photoshop 2026.app",
                "/Applications/Affinity Photo 2.app",
            ]
        };
        crate::app::ui::shots::shot(
            "preferences-external-editing",
            egui::Vec2::new(1100., 900.),
            move |e, _| {
                e.edit_in
                    .set_editors(apps.iter().map(PathBuf::from).collect());
                e.edit_in.space = OutputSpace::AdobeRgb;
                e.onboarding.visible = false;
                e.preferences.open = true;
                e.preferences.tab = Tab::ExternalEditing;
            },
            |e, ctx| e.preferences_window(ctx),
        );
    }

    #[test]
    fn editors_are_added_once_and_kept_in_order() {
        let mut e = editor_with(vec![PathBuf::from("/Applications/A.app")]);
        e.add_external_editor(PathBuf::from("/Applications/B.app"));
        e.add_external_editor(PathBuf::from("/Applications/A.app"));
        assert_eq!(
            e.edit_in.editors(),
            [
                PathBuf::from("/Applications/A.app"),
                PathBuf::from("/Applications/B.app")
            ]
        );
        assert_eq!(&*e.edit_in.names(), ["A".to_string(), "B".to_string()]);
    }
}
