//! Copy Settings, Paste Settings and Paste from Previous: settings moving from one
//! photo to another by group (see `model::settings_groups`).
use crate::app::ui::design::{self, Tone, button, inset, space, text};
use crate::app::ui::widgets::{modal_frame, primary_button};
use crate::app::{Editor, ui::theme};
use crate::model::recipe::Recipe;
use crate::{
    camera_data::Metadata,
    model::settings_groups::{self, GroupInclusion, GroupSelection, SettingGroup, Source, Target},
};
use eframe::egui::{self, Color32, Vec2};

/// A photo's settings with its camera, as Copy or leaving a photo keeps them.
#[derive(Clone, Debug)]
pub(in crate::app) struct Settings {
    pub(in crate::app) recipe: Recipe,
    pub(in crate::app) metadata: Metadata,
}

/// Settings copied from a photo, and the groups Paste applies.
#[derive(Clone, Debug)]
pub(in crate::app) struct Clipboard {
    pub(in crate::app) settings: Settings,
    pub(in crate::app) groups: GroupSelection,
}

/// Copy Settings or Synchronize Settings while it is open: the groups being chosen.
#[derive(Clone, Debug)]
pub(in crate::app) struct CopyDialog {
    pub(in crate::app) purpose: Transfer,
    pub(in crate::app) groups: GroupSelection,
    /// New Develop Preset's name and group.
    pub(in crate::app) preset: PresetForm,
}

/// What New Develop Preset asks for beside the settings.
#[derive(Clone, Debug)]
pub(in crate::app) struct PresetForm {
    pub(in crate::app) name: String,
    pub(in crate::app) group: String,
}
impl Default for PresetForm {
    fn default() -> Self {
        Self {
            name: String::new(),
            group: "User Presets".into(),
        }
    }
}

/// Where the chosen groups go: the clipboard, or the other selected photos.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Transfer {
    Copy,
    Sync,
    /// New Develop Preset: the chosen groups as a preset in the user library.
    NewPreset,
}
impl Transfer {
    fn title(self) -> &'static str {
        match self {
            Transfer::Copy => "Copy Settings",
            Transfer::Sync => "Synchronize Settings",
            Transfer::NewPreset => "New Develop Preset",
        }
    }
    fn button(self) -> &'static str {
        match self {
            Transfer::Copy => "Copy",
            Transfer::Sync => "Synchronize",
            Transfer::NewPreset => "Create",
        }
    }
}

/// What the user did in the dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum CopyChoice {
    Confirm,
    Cancel,
}

impl Editor {
    /// The open photo's settings, once its camera is known.
    pub(in crate::app) fn current_settings(&self) -> Option<Settings> {
        Some(Settings {
            recipe: self.document.edit.recipe().clone(),
            metadata: self.document_metadata()?,
        })
    }
    /// The header's metadata, or the decoded image's while the header is pending.
    fn document_metadata(&self) -> Option<Metadata> {
        self.document
            .metadata
            .clone()
            .or_else(|| self.document.full().map(|im| im.metadata.clone()))
    }
    /// Copies the open photo's settings for Paste, which applies only `groups`.
    pub(in crate::app) fn copy_settings(&mut self, groups: GroupSelection) {
        let Some(settings) = self.current_settings() else {
            return;
        };
        self.clipboard = Some(Clipboard { settings, groups });
        self.messages.note("Settings copied");
    }
    /// Opens Copy Settings, or Synchronize Settings, with the groups chosen last time.
    pub(in crate::app) fn open_copy_dialog(&mut self, purpose: Transfer) {
        self.modal = Some(crate::app::Modal::CopySettings(CopyDialog {
            purpose,
            groups: self.session.copy_groups.clone(),
            preset: PresetForm::default(),
        }));
    }
    /// Lightroom's Copy Settings: a checkbox per group, under its section.
    pub(in crate::app) fn copy_dialog_window(&mut self, ctx: &egui::Context) {
        let palette = theme::palette(ctx);
        let Some(crate::app::Modal::CopySettings(dialog)) = &mut self.modal else {
            return;
        };
        // Return is the primary button, as in Lightroom; it is taken before the
        // dialog draws so a focused checkbox or button does not also act on it.
        // Space still presses the focused one, and an open group list keeps Return.
        let confirm = !egui::Popup::is_any_open(ctx)
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        let mut choice = confirm.then_some(CopyChoice::Confirm);
        let response = egui::Modal::new(egui::Id::new("copy-settings"))
            .backdrop_color(Color32::from_black_alpha(140))
            .frame(modal_frame(&palette).inner_margin(design::margin_same(inset::DIALOG)))
            .show(ctx, |ui| {
                ui.set_width(COLUMN * 2. + GAP);
                ui.label(
                    egui::RichText::new(dialog.purpose.title())
                        .size(text::TITLE)
                        .color(palette.tone(Tone::STRONG)),
                );
                ui.add_space(space::XL);
                if dialog.purpose == Transfer::NewPreset {
                    preset_fields(ui, &mut dialog.preset, &self.presets.library);
                    ui.add_space(space::L);
                }
                // Sections fill the left column, then the right, in Lightroom's order.
                // They scroll in a short window, so the buttons stay in view.
                let (left, right) = SettingGroup::SECTIONS.split_at(LEFT_SECTIONS);
                let height = (ctx.content_rect().height() * 0.85 - 130.).max(160.);
                egui::ScrollArea::vertical()
                    .max_height(height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = GAP;
                            for column in [left, right] {
                                ui.allocate_ui(Vec2::new(COLUMN, 0.), |ui| {
                                    ui.vertical(|ui| {
                                        ui.set_width(COLUMN);
                                        for section in column {
                                            section_checkboxes(
                                                ui,
                                                section,
                                                &mut dialog.groups,
                                                dialog.purpose,
                                            );
                                        }
                                    });
                                });
                            }
                        });
                    });
                ui.add_space(space::XL);
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding = button::DIALOG;
                    if ui.button("Check All").clicked() {
                        dialog.groups = GroupSelection::all();
                    }
                    if ui.button("Check None").clicked() {
                        dialog.groups = GroupSelection::none();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if primary_button(ui, dialog.purpose.button()).clicked() {
                            choice = Some(CopyChoice::Confirm);
                        }
                        if ui.button("Cancel").clicked() {
                            choice = Some(CopyChoice::Cancel);
                        }
                    });
                });
            });
        if response.should_close() {
            choice = Some(CopyChoice::Cancel);
        }
        if let Some(choice) = choice {
            self.close_copy_dialog(choice);
        }
    }
    /// Copy keeps the groups chosen for Paste, and Synchronize applies them to the
    /// other selected photos; both start the next dialog from that choice. A new
    /// preset's groups are its own and leave that choice as it was.
    pub(in crate::app) fn close_copy_dialog(&mut self, choice: CopyChoice) {
        let Some(crate::app::Modal::CopySettings(dialog)) = self
            .modal
            .take_if(|m| matches!(m, crate::app::Modal::CopySettings(_)))
        else {
            return;
        };
        if choice == CopyChoice::Cancel {
            return;
        }
        if dialog.purpose != Transfer::NewPreset {
            self.session.copy_groups = dialog.groups.clone();
            let _ = self.save_session();
        }
        match dialog.purpose {
            Transfer::Copy => self.copy_settings(dialog.groups),
            Transfer::Sync => self.start_sync(crate::app::library::sync::BatchChange::Settings(
                dialog.groups,
            )),
            Transfer::NewPreset => self.create_preset(&dialog.preset, &dialog.groups),
        }
    }
    /// Pastes the copied settings. Spot removal and masks belong to their photo and
    /// stay as they were, as with Lightroom's default Paste Settings.
    pub(in crate::app) fn paste_settings(&mut self) {
        if let Some(Clipboard { settings, groups }) = self.clipboard.clone() {
            self.apply_settings(&settings, &groups, "Paste Settings");
        }
    }
    /// Lightroom's Paste Settings from Previous: the settings of the photo open before
    /// this one, with the groups Paste uses by default.
    pub(in crate::app) fn paste_previous(&mut self) {
        if let Some(previous) = self.previous_settings.clone() {
            self.apply_settings(&previous, &GroupSelection::default(), "Paste from Previous");
        }
    }
    /// `from`'s settings in `groups` over the open photo's, as one History step.
    fn apply_settings(&mut self, from: &Settings, groups: &GroupSelection, step: &str) {
        let Some(metadata) = self.document_metadata() else {
            return;
        };
        let out = settings_groups::transfer(
            Source {
                recipe: &from.recipe,
                metadata: &from.metadata,
            },
            self.document.edit.recipe(),
            groups,
            Target {
                metadata: &metadata,
                profiles: &self.document.profiles,
            },
        );
        self.document
            .edit
            .name_next_step(crate::app::history::Step::new(step, ""));
        self.document.edit.replace(out.recipe);
        self.ensure_analysis();
        if out.notes.is_empty() {
            self.messages.note("Settings pasted");
        } else {
            self.messages
                .warn(format!("Settings pasted · {}", out.notes.join(" · ")));
        }
    }
}

/// Copy Settings' column width, the space between columns, and how many sections the
/// left column holds.
const COLUMN: f32 = 230.;
const GAP: f32 = 28.;
const LEFT_SECTIONS: usize = 7;

/// A section's checkbox, which checks or clears all of it, and one indented checkbox per
/// group when it has several.
fn section_checkboxes(
    ui: &mut egui::Ui,
    section: &settings_groups::Section,
    groups: &mut GroupSelection,
    purpose: Transfer,
) {
    // A preset never carries one photo's Upright correction: it names the mode only.
    let shown: Vec<SettingGroup> = section
        .groups
        .iter()
        .copied()
        .filter(|g| {
            purpose != Transfer::NewPreset
                || !matches!(
                    *g,
                    SettingGroup::UprightTransforms | SettingGroup::AiDenoise
                )
        })
        .collect();
    let chosen = shown.iter().filter(|g| groups.contains(**g)).count();
    let mut all = chosen == shown.len();
    let checkbox = egui::Checkbox::new(&mut all, section.title)
        .indeterminate(chosen > 0 && chosen < shown.len());
    if ui.add(checkbox).changed() {
        let inclusion = if all {
            GroupInclusion::Included
        } else {
            GroupInclusion::Excluded
        };
        for group in &shown {
            groups.set(*group, inclusion);
        }
    }
    if let [_, _, ..] = shown[..] {
        ui.indent(section.title, |ui| {
            for group in &shown {
                let mut on = groups.contains(*group);
                if ui.checkbox(&mut on, group.label()).changed() {
                    let inclusion = if on {
                        GroupInclusion::Included
                    } else {
                        GroupInclusion::Excluded
                    };
                    groups.set(*group, inclusion);
                }
            }
        });
    }
    ui.add_space(space::S);
}

/// New Develop Preset's name and group, as form rows; the group can be typed or picked
/// from the groups presets already use.
fn preset_fields(ui: &mut egui::Ui, form: &mut PresetForm, library: &crate::presets::Library) {
    crate::app::ui::widgets::form_row(ui, "Preset Name", |ui| {
        ui.add(egui::TextEdit::singleline(&mut form.name).desired_width(260.));
    });
    crate::app::ui::widgets::form_row(ui, "Group", |ui| {
        ui.add(egui::TextEdit::singleline(&mut form.group).desired_width(220.));
        let mut groups: Vec<&str> = library
            .presets
            .iter()
            .filter(|p| !p.builtin)
            .map(|p| p.group.as_str())
            .filter(|g| !g.is_empty())
            .collect();
        groups.sort_unstable();
        groups.dedup();
        egui::ComboBox::from_id_salt("preset-group")
            .selected_text("")
            .width(24.)
            .show_ui(ui, |ui| {
                for group in groups {
                    if ui.selectable_label(form.group == group, group).clicked() {
                        form.group = group.to_string();
                    }
                }
            });
    });
}

#[cfg(test)]
#[path = "settings_transfer_editor_tests.rs"]
mod editor_tests;
