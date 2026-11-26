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
    pub(in crate::app) fn document_metadata(&self) -> Option<Metadata> {
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
    /// Opens Copy Settings, or Synchronize Settings, with the groups chosen last time:
    /// Synchronize with the ones it applied, which Auto Sync applies too.
    pub(in crate::app) fn open_copy_dialog(&mut self, purpose: Transfer) {
        let groups = match purpose {
            Transfer::Sync => self.sync_groups(),
            Transfer::Copy | Transfer::NewPreset => self.session.copy_groups.clone(),
        };
        self.modal = Some(crate::app::Modal::CopySettings(CopyDialog {
            purpose,
            groups,
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
                crate::app::ui::widgets::name_dialog(ui, dialog.purpose.title());
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
            if dialog.purpose == Transfer::Sync {
                self.session.sync_groups = Some(dialog.groups.clone());
            }
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
    /// Carries out a click on the filmstrip bar's [`TransferButtons`].
    pub(in crate::app) fn transfer_clicked(&mut self, click: TransferClick) {
        match click {
            TransferClick::Copy => self.open_copy_dialog(Transfer::Copy),
            TransferClick::Paste => self.paste_settings(),
            TransferClick::Sync if self.session.auto_sync => self.start_sync(
                crate::app::library::sync::BatchChange::Settings(self.sync_groups()),
            ),
            TransferClick::Sync => self.open_copy_dialog(Transfer::Sync),
            TransferClick::AutoSync => self.toggle_auto_sync(),
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
            self.lensfun_db(),
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

/// Develop's Copy, Paste and Sync buttons at the left of the filmstrip's bar, as
/// they can be used now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TransferButtons {
    pub(crate) can_copy: bool,
    pub(crate) can_paste: bool,
    /// Other photos are selected with the open one.
    pub(crate) can_sync: bool,
    pub(crate) auto_sync: bool,
    /// Auto Sync has been writing the other photos for a while.
    pub(crate) syncing: bool,
}
/// A click on one of the [`TransferButtons`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TransferClick {
    Copy,
    Paste,
    /// Sync…, or with Auto Sync on, Auto Sync: synchronize now.
    Sync,
    /// The switch beside Sync turning Auto Sync on or off.
    AutoSync,
}
/// How the buttons are drawn: with their labels, or as icons when the bar is short
/// of room.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ButtonSize {
    Labelled,
    Icons,
}
/// An icon button's width.
const ICON_BUTTON: f32 = 28.;
/// The Auto Sync switch's width, and the indicator's beside it.
const SWITCH: f32 = 26.;
const INDICATOR: f32 = 16.;
const SPINNER: f32 = 12.;
impl TransferButtons {
    /// The width the row takes at `size`.
    pub(crate) fn width(self, ui: &egui::Ui, size: ButtonSize) -> f32 {
        let buttons = match size {
            ButtonSize::Labelled => {
                label_width(ui, "Copy…") + label_width(ui, "Paste") + label_width(ui, "Auto Sync")
            }
            ButtonSize::Icons => 3. * ICON_BUTTON,
        };
        buttons + SWITCH + INDICATOR + 3. * space::XS + space::S
    }
    /// Lightroom's Copy…, Paste and Sync… with its Auto Sync switch, on one row,
    /// with their labels or as icons. Each is named for screen readers by its label.
    pub(crate) fn show(self, ui: &mut egui::Ui, size: ButtonSize) -> Option<TransferClick> {
        use crate::app::shell::shortcuts::keys_text;
        let mut click = None;
        ui.spacing_mut().item_spacing.x = space::XS;
        let copy = format!(
            "Copy the settings you choose · {}",
            keys_text("Cmd+Shift+C")
        );
        if bar_button(ui, "Copy…", "Copy…", size, 5, self.can_copy)
            .on_hover_text(copy)
            .clicked()
        {
            click = Some(TransferClick::Copy);
        }
        let paste = if self.can_paste {
            format!("Paste the copied settings · {}", keys_text("Cmd+Shift+V"))
        } else {
            "Copy settings first, to paste them".into()
        };
        if bar_button(ui, "Paste", "Paste", size, 6, self.can_paste)
            .on_hover_text(paste)
            .clicked()
        {
            click = Some(TransferClick::Paste);
        }
        ui.add_space(space::S - space::XS);
        let (label, name, hover) = if self.auto_sync {
            (
                "Auto Sync",
                // Named for what a click does while Auto Sync is on.
                "Auto Sync, sync now",
                "Auto Sync is on: edits go to the selected photos too. Click to sync them now"
                    .to_string(),
            )
        } else {
            (
                "Sync…",
                "Sync…",
                format!(
                    "Sync settings to the selected photos · {}",
                    keys_text("Cmd+Shift+S")
                ),
            )
        };
        let hover = if self.can_sync {
            hover
        } else if cfg!(target_os = "macos") {
            "Cmd-click photos in the filmstrip to sync to them".into()
        } else {
            "Ctrl-click photos in the filmstrip to sync to them".into()
        };
        // As wide as its longer label, so the switch stays put when it turns.
        let width = match size {
            ButtonSize::Labelled => label_width(ui, "Auto Sync"),
            ButtonSize::Icons => ICON_BUTTON,
        };
        let shown = match size {
            ButtonSize::Labelled => label,
            ButtonSize::Icons => "",
        };
        if bar_button_sized(ui, shown, name, width, 7, self.can_sync)
            .on_hover_text(hover)
            .clicked()
        {
            click = Some(TransferClick::Sync);
        }
        let switch = crate::app::ui::widgets::switch(ui, self.auto_sync, "Auto Sync");
        if switch
            .on_hover_text(format!(
                "Auto Sync: edits go to the selected photos too · {}",
                keys_text("Cmd+Option+Shift+A")
            ))
            .clicked()
        {
            click = Some(TransferClick::AutoSync);
        }
        // Held at its width, so nothing after it moves when it shows.
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(INDICATOR, design::size::CONTROL),
            egui::Sense::hover(),
        );
        if self.syncing {
            crate::app::ui::widgets::paint_spinner(
                ui,
                egui::Rect::from_center_size(rect.center(), Vec2::splat(SPINNER)),
                None,
            );
            response.on_hover_text("Auto Sync is writing the other photos");
        }
        click
    }
}
/// The width of a bar button labelled `label`.
fn label_width(ui: &egui::Ui, label: &str) -> f32 {
    let text = ui.painter().layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(text::LABEL),
        Color32::WHITE,
    );
    text.size().x + 2. * space::M
}
/// A button of the filmstrip's bar, as the Library's bars draw theirs: as wide as
/// its label, or as its `icon` alone.
fn bar_button(
    ui: &mut egui::Ui,
    label: &str,
    name: &str,
    size: ButtonSize,
    icon: u8,
    enabled: bool,
) -> egui::Response {
    match size {
        ButtonSize::Labelled => {
            let width = label_width(ui, label);
            bar_button_sized(ui, label, name, width, 0, enabled)
        }
        ButtonSize::Icons => bar_button_sized(ui, "", name, ICON_BUTTON, icon, enabled),
    }
}
/// A bar button `width` wide, showing `label`, or `icon` when it has none, and
/// named `name`.
fn bar_button_sized(
    ui: &mut egui::Ui,
    label: &str,
    name: &str,
    width: f32,
    icon: u8,
    enabled: bool,
) -> egui::Response {
    let icon = if label.is_empty() { icon } else { 0 };
    let response = crate::app::ui::widgets::toolbar_action_sized(
        ui,
        label,
        Vec2::new(width, design::size::CONTROL),
        false,
        enabled,
        icon,
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, name));
    response
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
        ui.add(crate::app::ui::widgets::dropdown_text_edit(&mut form.name).desired_width(260.));
    });
    crate::app::ui::widgets::form_row(ui, "Group", |ui| {
        ui.add(crate::app::ui::widgets::dropdown_text_edit(&mut form.group).desired_width(220.));
        let mut groups: Vec<&str> = library
            .presets
            .iter()
            .filter(|p| !p.builtin)
            .map(|p| p.group.as_str())
            .filter(|g| !g.is_empty())
            .collect();
        groups.sort_unstable();
        groups.dedup();
        crate::app::ui::widgets::named_menu(
            crate::app::ui::widgets::dropdown(ui, "preset-group")
                .selected_text("")
                .width(24.)
                .show_ui(ui, |ui| {
                    for group in groups {
                        if ui.selectable_label(form.group == group, group).clicked() {
                            form.group = group.to_string();
                        }
                    }
                }),
            "Existing groups",
        );
    });
}

#[cfg(test)]
#[path = "settings_transfer_editor_tests.rs"]
mod editor_tests;
