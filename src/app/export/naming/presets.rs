//! The Filename Template Editor's Preset menu, as Lightroom's: the preset
//! the template started from ("(edited)" once changed), the presets to start
//! from (Lightroom's, RAWmakase's own and the user's saved templates), and
//! Save Current Settings as New Preset…, Update Preset, Rename preset… and
//! Delete preset… for a saved template. A saved template is acted on by the
//! one chosen here, never found again by its tokens. Each change is saved
//! before the menu shows it (`SavedTemplates`); one that can't be is said.
use crate::app::ui::design::{self, Tone, button, inset, space, text};
use crate::app::ui::theme;
use crate::app::ui::widgets::{modal_frame, primary_button};
use crate::export_settings::{BUILTIN, Builtin, Naming, SavedTemplate, SavedTemplates, builtin};
use eframe::egui::{self, Color32, Vec2};

/// The user's saved templates as their file was read, or why it can't be.
pub(in crate::app) struct Library(pub Result<SavedTemplates, String>);

impl Default for Library {
    fn default() -> Self {
        Self::load()
    }
}

impl Library {
    /// The saved templates now. Tests read none, and can save none, unless
    /// given a library of their own.
    pub(crate) fn load() -> Self {
        #[cfg(test)]
        return Self(SavedTemplates::load_from(std::path::Path::new(
            "/nonexistent/rawmakase-tests/filename_templates.json",
        )));
        #[cfg(not(test))]
        Self(SavedTemplates::load())
    }
    /// The templates, by name; none when the file can't be read.
    pub(crate) fn list(&self) -> Vec<SavedTemplate> {
        self.0
            .as_ref()
            .map(SavedTemplates::list)
            .unwrap_or_default()
    }
    /// The name of the one saved template whose tokens are `tokens`.
    pub(crate) fn name_of(&self, tokens: &str) -> Option<String> {
        self.0.as_ref().ok()?.name_of(tokens)
    }
}

/// The preset a template is, or started from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) enum Preset {
    Naming(Naming),
    Builtin(&'static Builtin),
    /// A saved template, by its name.
    Saved(String),
    /// None of them.
    Custom,
}

impl Preset {
    /// What a dialog's choice is: `naming`, or `custom` (Lightroom's form)
    /// when a template names files in its place.
    pub(crate) fn of(naming: Naming, custom: Option<&str>, library: &Library) -> Self {
        match custom {
            None => Preset::Naming(naming),
            Some(tokens) => match (builtin(tokens), library.name_of(tokens)) {
                (Some(b), _) => Preset::Builtin(b),
                (None, Some(name)) => Preset::Saved(name),
                (None, None) => Preset::Custom,
            },
        }
    }
    pub(crate) fn label(&self) -> String {
        match self {
            Preset::Naming(n) => n.label().into(),
            Preset::Builtin(b) => b.name.into(),
            Preset::Saved(name) => name.clone(),
            Preset::Custom => "Custom Settings".into(),
        }
    }
}

/// A question the Preset menu asks before it changes a saved template.
#[derive(Clone, Debug, PartialEq)]
enum Prompt {
    /// Save Current Settings as New Preset…: its name.
    New(String),
    Rename {
        old: String,
        name: String,
    },
    Delete(String),
}

/// What the Preset menu does to saved templates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Action {
    /// Save Current Settings as New Preset…: asks its name.
    New,
    /// Update Preset: the saved template chosen gets the tokens typed.
    Update,
    /// Rename preset…: asks its new name.
    Rename,
    /// Delete preset…: asks whether to.
    Delete,
}

/// The Preset menu's state while the editor is open.
pub(in crate::app) struct Presets {
    preset: Preset,
    /// The preset's tokens as chosen: the template is "(edited)" once it
    /// differs.
    origin: String,
    prompt: Option<Prompt>,
    /// The question's name field is yet to be given the keyboard.
    focus_prompt: bool,
    /// What went wrong saving the last change.
    message: Option<String>,
}

impl Presets {
    pub(crate) fn new(preset: Preset, origin: &str) -> Self {
        Self {
            preset,
            origin: origin.into(),
            prompt: None,
            focus_prompt: false,
            message: None,
        }
    }
    #[cfg(test)]
    pub(crate) fn preset(&self) -> &Preset {
        &self.preset
    }
    /// Does `action` with the template's `tokens` (`None` when the text
    /// can't be read): Update at once, the others once their question is
    /// answered.
    pub(crate) fn act(&mut self, action: Action, tokens: Option<&str>, library: &mut Library) {
        let saved = match &self.preset {
            Preset::Saved(name) => Some(name.clone()),
            _ => None,
        };
        self.focus_prompt = true;
        match (action, saved) {
            (Action::New, _) => self.prompt = Some(Prompt::New("Untitled Preset".into())),
            (Action::Update, Some(name)) => {
                let (Some(tokens), Ok(saved)) = (tokens, library.0.as_mut()) else {
                    return;
                };
                match saved.update(&name, tokens) {
                    Ok(()) => {
                        self.origin = tokens.to_string();
                        self.message = None;
                    }
                    Err(e) => self.message = Some(format!("“{name}” not updated: {e:#}")),
                }
            }
            (Action::Rename, Some(name)) => {
                self.prompt = Some(Prompt::Rename {
                    old: name.clone(),
                    name,
                })
            }
            (Action::Delete, Some(name)) => self.prompt = Some(Prompt::Delete(name)),
            // Only a saved template is updated, renamed or deleted.
            (_, None) => {}
        }
    }

    /// Whether a question is open over the editor.
    pub(crate) fn asking(&self) -> bool {
        self.prompt.is_some()
    }
    /// Whether `tokens` (`None` when the text can't be read) differ from the
    /// preset's.
    fn edited(&self, tokens: Option<&str>) -> bool {
        tokens != Some(self.origin.as_str())
    }

    /// The Preset row; the tokens of a preset chosen to start from, which
    /// the editor then shows.
    pub(crate) fn row(
        &mut self,
        ui: &mut egui::Ui,
        palette: &theme::Palette,
        tokens: Option<&str>,
        library: &mut Library,
    ) -> Option<String> {
        let edited = self.edited(tokens) && self.preset != Preset::Custom;
        let shown = match (&self.preset, edited) {
            (Preset::Custom, _) => "Custom".to_string(),
            (preset, true) => format!("{} (edited)", preset.label()),
            (preset, false) => preset.label(),
        };
        let saved = library.list();
        let writable = library.0.is_ok();
        let mut picked: Option<(Preset, String)> = None;
        let mut action = None;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Preset:").color(palette.tone(Tone::SECONDARY)));
            egui::ComboBox::from_id_salt("filename-template-preset")
                .width(300.)
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    for n in Naming::ALL {
                        let on = self.preset == Preset::Naming(n);
                        if ui.selectable_label(on, n.label()).clicked() {
                            picked = Some((Preset::Naming(n), n.tokens().into()));
                        }
                    }
                    ui.separator();
                    for b in &BUILTIN {
                        let on = self.preset == Preset::Builtin(b);
                        if ui.selectable_label(on, b.name).clicked() {
                            picked = Some((Preset::Builtin(b), b.tokens.into()));
                        }
                    }
                    if !saved.is_empty() {
                        ui.separator();
                        for t in &saved {
                            let on = self.preset == Preset::Saved(t.name.clone());
                            if ui.selectable_label(on, &t.name).clicked() {
                                picked = Some((Preset::Saved(t.name.clone()), t.tokens.clone()));
                            }
                        }
                    }
                    ui.separator();
                    let save = ui.add_enabled(
                        writable && tokens.is_some(),
                        egui::Button::selectable(false, "Save Current Settings as New Preset…"),
                    );
                    if save.clicked() {
                        action = Some(Action::New);
                    }
                    if let Preset::Saved(name) = &self.preset {
                        if edited {
                            let label = format!("Update Preset “{name}”");
                            if ui
                                .add_enabled(
                                    writable && tokens.is_some(),
                                    egui::Button::selectable(false, label),
                                )
                                .clicked()
                            {
                                action = Some(Action::Update);
                            }
                        } else {
                            let rename = format!("Rename preset “{name}”…");
                            if ui
                                .add_enabled(writable, egui::Button::selectable(false, rename))
                                .clicked()
                            {
                                action = Some(Action::Rename);
                            }
                            let delete = format!("Delete preset “{name}”…");
                            if ui
                                .add_enabled(writable, egui::Button::selectable(false, delete))
                                .clicked()
                            {
                                action = Some(Action::Delete);
                            }
                        }
                    }
                });
        });
        if let Err(why) = &library.0 {
            ui.label(
                egui::RichText::new(format!("Saved presets can't be changed: {why}"))
                    .size(text::LABEL)
                    .color(palette.warning()),
            );
        } else if let Some(message) = &self.message {
            ui.label(
                egui::RichText::new(message)
                    .size(text::LABEL)
                    .color(Color32::from_rgb(235, 110, 100)),
            );
        }
        if let Some(action) = action {
            self.act(action, tokens, library);
        }
        let (preset, tokens) = picked?;
        self.origin = tokens.clone();
        self.preset = preset;
        self.message = None;
        Some(tokens)
    }
}

impl Presets {
    /// The question open over the editor, if any: a new preset's name, a
    /// preset's new name, or whether to delete it. Return answers it, Escape
    /// or Cancel drops it.
    pub(crate) fn prompt_window(
        &mut self,
        ctx: &egui::Context,
        tokens: Option<&str>,
        library: &mut Library,
    ) {
        let Some(prompt) = self.prompt.as_mut() else {
            return;
        };
        let focus = &mut self.focus_prompt;
        let palette = theme::palette(ctx);
        let Ok(saved) = library.0.as_mut() else {
            self.prompt = None;
            return;
        };
        let problem = match &*prompt {
            Prompt::New(name) => saved.name_problem(name, None),
            Prompt::Rename { old, name } if name.trim() == old => None,
            Prompt::Rename { old, name } => saved.name_problem(name, Some(old)),
            Prompt::Delete(_) => None,
        };
        let returned = !egui::Popup::is_any_open(ctx)
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        let mut confirm = false;
        let mut cancel = false;
        let response = egui::Modal::new(egui::Id::new("filename-preset-prompt"))
            .backdrop_color(Color32::from_black_alpha(60))
            .frame(modal_frame(&palette).inner_margin(design::margin_same(inset::DIALOG)))
            .show(ctx, |ui| {
                ui.set_width(360.);
                ui.spacing_mut().item_spacing = Vec2::new(space::M, space::M);
                let (title, action) = match &*prompt {
                    Prompt::New(_) => ("New Preset".to_string(), "Create"),
                    Prompt::Rename { old, .. } => (format!("Rename Preset “{old}”"), "Rename"),
                    Prompt::Delete(name) => {
                        (format!("Delete the preset named “{name}”?"), "Delete")
                    }
                };
                ui.label(
                    egui::RichText::new(title)
                        .size(text::LARGE)
                        .color(palette.tone(Tone::STRONG)),
                );
                match prompt {
                    Prompt::New(name) | Prompt::Rename { name, .. } => {
                        ui.horizontal(|ui| {
                            ui.label("Preset Name:");
                            let field = egui::TextEdit::singleline(name)
                                .id(egui::Id::new("filename-preset-name"))
                                .desired_width(220.)
                                .show(ui);
                            // The name is typed over, as it opens.
                            if std::mem::take(focus) {
                                field.response.request_focus();
                                let mut state = field.state.clone();
                                let end =
                                    egui::text::CCursor::new(field.galley.text().chars().count());
                                state
                                    .cursor
                                    .set_char_range(Some(egui::text::CCursorRange::two(
                                        egui::text::CCursor::new(0),
                                        end,
                                    )));
                                state.store(ui.ctx(), field.response.id);
                            }
                        });
                        if let Some(problem) = &problem {
                            ui.label(
                                egui::RichText::new(problem)
                                    .size(text::LABEL)
                                    .color(palette.warning()),
                            );
                        }
                    }
                    Prompt::Delete(_) => {
                        ui.label(
                            egui::RichText::new("This can't be undone.")
                                .size(text::LABEL)
                                .color(palette.tone(Tone::SECONDARY)),
                        );
                    }
                }
                ui.add_space(space::XS);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().button_padding = button::DIALOG;
                    let ok = problem.is_none();
                    confirm = ui
                        .add_enabled_ui(ok, |ui| primary_button(ui, action))
                        .inner
                        .clicked()
                        || (returned && ok);
                    cancel = ui.button("Cancel").clicked();
                });
            });
        if cancel || (!confirm && response.should_close()) {
            self.prompt = None;
            return;
        }
        if !confirm {
            return;
        }
        let Some(prompt) = self.prompt.take() else {
            return;
        };
        let done = match prompt {
            Prompt::New(name) => match tokens {
                Some(tokens) => saved.add(&name, tokens).map(|()| {
                    self.preset = Preset::Saved(name.trim().to_string());
                    self.origin = tokens.to_string();
                }),
                None => Ok(()),
            },
            Prompt::Rename { old, name } if name.trim() == old => Ok(()),
            Prompt::Rename { old, name } => saved.rename(&old, &name).map(|()| {
                self.preset = Preset::Saved(name.trim().to_string());
            }),
            Prompt::Delete(name) => saved.delete(&name).map(|()| {
                // The template stays as it is, a preset of none.
                self.preset = Preset::Custom;
                if let Some(tokens) = tokens {
                    self.origin = tokens.to_string();
                }
            }),
        };
        self.message = done.err().map(|e| format!("Not saved: {e:#}"));
    }
}
