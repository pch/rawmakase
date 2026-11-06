//! Presets made here: New Develop Preset, and Update, Rename and Delete in a preset's
//! menu, for presets in the user library's "User Presets" folder and those imported
//! into it; and Reveal in Finder for any preset file.
use crate::app::ui::design::{self, Tone, button, inset, space, text};
use crate::app::ui::widgets::{modal_frame, primary_button};
use crate::app::{Editor, develop::settings_transfer::PresetForm, ui::theme};
use crate::model::settings_groups::GroupSelection;
use crate::presets::user::UserPresets;
use crate::xmp::preset_write::PresetInfo;
use eframe::egui::{self, Color32};

/// A change asked for in a preset's menu.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) enum PresetAction {
    Update(usize),
    StartRename(usize),
    Delete(usize),
    Reveal(usize),
}

/// The menu of preset `i`, `p`, and what was chosen in it: only what `p` allows.
pub(in crate::app) fn preset_menu(
    ui: &mut egui::Ui,
    user: &UserPresets,
    p: &crate::xmp::Preset,
    i: usize,
) -> Option<PresetAction> {
    let mut action = None;
    if user.manages(p) {
        if ui.button("Update with Current Settings").clicked() {
            action = Some(PresetAction::Update(i));
        }
        if ui.button("Rename…").clicked() {
            action = Some(PresetAction::StartRename(i));
        }
        if ui.button("Delete").clicked() {
            action = Some(PresetAction::Delete(i));
        }
        ui.separator();
    }
    if ui.button(crate::platform::reveal::LABEL).clicked() {
        action = Some(PresetAction::Reveal(i));
    }
    action
}

/// A preset being renamed: its file, which stays put while the library reloads,
/// and the name being typed.
pub(in crate::app) struct PresetRename {
    path: std::path::PathBuf,
    name: String,
}

/// What the rename window asked for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RenameChoice {
    Rename,
    Cancel,
}

impl Editor {
    /// Saves the open photo's `groups` as a new preset in the user library.
    pub(in crate::app) fn create_preset(&mut self, form: &PresetForm, groups: &GroupSelection) {
        self.create_preset_in(&UserPresets::default(), form, groups);
    }
    /// A photo is open and loaded, so its settings are the ones shown, not the
    /// defaults a document holds while the next photo loads.
    fn settings_ready(&self) -> bool {
        self.document.full().is_some() && !self.document.loading()
    }
    fn create_preset_in(&mut self, user: &UserPresets, form: &PresetForm, groups: &GroupSelection) {
        let shown = self.document.full().map(|im| im.metadata.clone());
        let (true, Some(shown)) = (self.settings_ready(), shown) else {
            self.messages.warn("Preset not created: no photo is open");
            return;
        };
        let info = PresetInfo::new(&form.name, &form.group);
        // Presets do not carry AI Denoise (docs/rendering-quality.md#ai-denoise).
        let mut groups = groups.clone();
        groups.set(
            crate::model::settings_groups::SettingGroup::AiDenoise,
            crate::model::settings_groups::GroupInclusion::Excluded,
        );
        if groups.is_empty() {
            self.messages.warn("Preset not created: no settings chosen");
            return;
        }
        let groups = &groups;
        match user.create(self.document.edit.recipe(), &shown, &info, groups) {
            Ok(_) => {
                self.messages
                    .done(format!("Preset {} created in {}", info.name, info.group));
                self.reload_presets(&self.context.clone());
            }
            Err(e) => self.messages.fail(format!("Preset not created: {e:#}")),
        }
    }
    /// Carries out a preset menu's choice.
    pub(in crate::app) fn preset_action(&mut self, action: PresetAction) {
        let library = self.presets.library.clone();
        let user = UserPresets::default();
        let result = match action {
            PresetAction::Update(_) if !self.settings_ready() => {
                self.messages.warn("Preset not updated: no photo is open");
                return;
            }
            PresetAction::Update(i) => {
                library
                    .presets
                    .get(i)
                    .zip(self.document.full())
                    .map(|(p, im)| {
                        user.update(p, self.document.edit.recipe(), &im.metadata)
                            .map(|backup| match backup {
                                None => format!("Preset {} updated", p.name),
                                Some(b) => format!(
                                    "Preset {} updated; the imported original is kept as {}",
                                    p.name,
                                    b.display()
                                ),
                            })
                    })
            }
            PresetAction::Delete(i) => library.presets.get(i).map(|p| {
                user.delete(p).map(|()| {
                    // A new preset later saved at the same place starts unfavored.
                    if self.presets.favorites.remove(&p.id) {
                        self.presets.revision += 1;
                        let _ = crate::presets::save_favorites(&self.presets.favorites);
                    }
                    format!("Preset {} deleted", p.name)
                })
            }),
            PresetAction::Reveal(i) => {
                if let Some(Err(e)) = library
                    .presets
                    .get(i)
                    .map(|p| crate::platform::reveal::reveal(&p.path))
                {
                    self.messages
                        .fail(format!("Couldn't show the preset: {e:#}"));
                }
                return;
            }
            PresetAction::StartRename(i) => {
                if let Some(p) = library.presets.get(i) {
                    self.modal = Some(crate::app::Modal::RenamePreset(PresetRename {
                        path: p.path.clone(),
                        name: p.name.clone(),
                    }));
                }
                return;
            }
        };
        match result {
            Some(Ok(status)) => {
                self.messages.done(status);
                self.reload_presets(&self.context.clone());
            }
            Some(Err(e)) => self.messages.fail(format!("Preset not changed: {e:#}")),
            None => {}
        }
    }
    /// The Rename Preset window, while one is being renamed.
    pub(in crate::app) fn preset_rename_window(&mut self, ctx: &egui::Context) {
        let palette = theme::palette(ctx);
        let Some(crate::app::Modal::RenamePreset(rename)) = &mut self.modal else {
            return;
        };
        let mut choice = None;
        let response = egui::Modal::new(egui::Id::new("rename-preset"))
            .backdrop_color(Color32::from_black_alpha(140))
            .frame(modal_frame(&palette).inner_margin(design::margin_same(inset::DIALOG)))
            .show(ctx, |ui| {
                ui.set_width(380.);
                ui.label(
                    egui::RichText::new("Rename Preset")
                        .size(text::TITLE)
                        .color(palette.tone(Tone::STRONG)),
                );
                ui.add_space(space::L);
                let field = ui
                    .add(egui::TextEdit::singleline(&mut rename.name).desired_width(f32::INFINITY));
                if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    choice = Some(RenameChoice::Rename);
                }
                ui.add_space(space::XL);
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding = button::DIALOG;
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if primary_button(ui, "Rename").clicked() {
                            choice = Some(RenameChoice::Rename);
                        }
                        if ui.button("Cancel").clicked() {
                            choice = Some(RenameChoice::Cancel);
                        }
                    });
                });
            });
        if response.should_close() {
            choice = Some(RenameChoice::Cancel);
        }
        let Some(choice) = choice else {
            return;
        };
        let Some(crate::app::Modal::RenamePreset(rename)) = self.modal.take() else {
            unreachable!("open above");
        };
        if choice == RenameChoice::Cancel {
            return;
        }
        let library = self.presets.library.clone();
        let Some(preset) = library.presets.iter().find(|p| p.path == rename.path) else {
            return;
        };
        match UserPresets::default().rename(preset, &rename.name) {
            Ok(path) => {
                // A favorite follows the preset to its new file.
                let old = preset.id.clone();
                if self.presets.favorites.remove(&old) {
                    self.presets
                        .favorites
                        .insert(path.to_string_lossy().to_string());
                    self.presets.revision += 1;
                    let _ = crate::presets::save_favorites(&self.presets.favorites);
                }
                // So does a raw default.
                let mut defaults = self.raw_defaults.settings().clone();
                defaults.rename_preset(&old, &path.to_string_lossy(), rename.name.trim());
                match self.set_raw_defaults(defaults) {
                    Ok(()) => self
                        .messages
                        .done(format!("Preset renamed to {}", rename.name.trim())),
                    Err(e) => self
                        .messages
                        .fail(format!("Preset renamed; raw defaults not saved: {e:#}")),
                }
                self.reload_presets(ctx);
            }
            Err(e) => self.messages.fail(format!("Preset not renamed: {e:#}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::settings_groups::{GroupInclusion, SettingGroup};

    #[test]
    fn new_develop_preset_writes_the_chosen_settings_to_the_user_library() {
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        let d = tempfile::tempdir().unwrap();
        let user = UserPresets {
            dir: d.path().join("User Presets"),
            imported: d.path().join("Imported"),
            backups: d.path().join("preset-backups"),
        };
        // Without a loaded photo there is nothing to save.
        let form = PresetForm {
            name: "Bright".into(),
            group: "Mine".into(),
        };
        e.create_preset_in(&user, &form, &GroupSelection::all());
        assert!(
            e.messages.line().contains("no photo"),
            "{}",
            e.messages.line()
        );
        e.document
            .set_image(std::sync::Arc::new(crate::camera_data::CameraImage {
                recovered: Default::default(),
                width: 4,
                height: 4,
                pixels: vec![[0.1; 3]; 16],
                metadata: Default::default(),
                fast: false,
                scale_factor: 1.,
                scale_clipped: 0,
                hdr: Default::default(),
            }));
        e.document.edit.setup_mut().exposure = 0.7;
        let mut groups = GroupSelection::none();
        groups.set(SettingGroup::Exposure, GroupInclusion::Included);
        e.create_preset_in(&user, &form, &groups);
        let path = user.dir.join("Mine/Bright.xmp");
        let preset = crate::xmp::parse(&path, &std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(preset.settings["Exposure2012"], "+0.70");
        assert!(
            e.messages.line().contains("created"),
            "{}",
            e.messages.line()
        );
        // A second one with the same name says why it wasn't made.
        e.create_preset_in(&user, &form, &groups);
        assert!(
            e.messages.line().starts_with("Preset not created"),
            "{}",
            e.messages.line()
        ); // Presets do not carry AI Denoise: chosen alone, nothing is made.
        let mut ai_only = GroupSelection::none();
        ai_only.set(SettingGroup::AiDenoise, GroupInclusion::Included);
        let form = PresetForm {
            name: "Denoised".into(),
            group: "Mine".into(),
        };
        e.create_preset_in(&user, &form, &ai_only);
        assert!(!user.dir.join("Mine/Denoised.xmp").exists());
        assert!(
            e.messages.line().contains("no settings chosen"),
            "{}",
            e.messages.line()
        );
    }

    #[test]
    fn a_preset_s_groups_leave_the_copy_and_sync_choice_alone() {
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        let before = e.session.copy_groups.clone();
        e.open_copy_dialog(crate::app::develop::settings_transfer::Transfer::NewPreset);
        let mut narrow = GroupSelection::none();
        narrow.set(SettingGroup::Exposure, GroupInclusion::Included);
        let Some(crate::app::Modal::CopySettings(dialog)) = &mut e.modal else {
            panic!("Copy Settings is open");
        };
        dialog.groups = narrow;
        e.close_copy_dialog(crate::app::develop::settings_transfer::CopyChoice::Confirm);
        assert_eq!(e.session.copy_groups, before);
    }

    #[test]
    fn an_older_preset_scan_never_replaces_a_newer_one() {
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        let library = |name: &str| {
            let info = PresetInfo::new(name, "Mine");
            let text = crate::xmp::preset_write::preset(
                &crate::model::recipe::Recipe::default(),
                &Default::default(),
                &info,
                &GroupSelection::all(),
            );
            std::sync::Arc::new(crate::presets::Library {
                presets: vec![crate::xmp::parse(std::path::Path::new("p.xmp"), &text).unwrap()],
                errors: Vec::new(),
            })
        };
        let older = e.presets.next_scan();
        let newer = e.presets.next_scan();
        e.presets_scanned(newer, library("new"));
        e.presets_scanned(older, library("old"));
        assert_eq!(e.presets.library.presets[0].name, "new");
    }
}
