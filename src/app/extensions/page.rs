//! Preferences > Extensions: the loaded extensions with their source, state
//! and available update; one extension's page with what it declares, its
//! settings and panels, its last errors and its log; and installing from
//! GitHub, from a file or, in developer mode, from a folder.
//!
//! Every change goes through the host, which writes `extensions.json` first
//! and leaves the session as it was when that fails. Downloads, installs and
//! update checks run on a thread of their own ([`Editor::extension_work`]),
//! and the page says it is busy until they report, however they end.
use super::{Extension, Origin, PathTarget, panels, source_label};
use crate::app::Editor;
use crate::app::preferences::{gap, group, hint};
use crate::app::ui::widgets::form_row;
use eframe::egui;
use rawmakase_extensions::github;
use rawmakase_extensions::protocol::manifest::{Files, SettingControl};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How often developer mode looks for a changed manifest or program.
const WATCH: Duration = Duration::from_secs(1);

/// What an installed extension's page and an offer say of its trust.
const NOT_REVIEWED: &str = "RAWmakase hasn't reviewed this extension. It runs with your \
     permissions.";

/// What the page keeps between frames.
#[derive(Default)]
pub(in crate::app) struct Page {
    /// The extension whose page is shown.
    selected: Option<String>,
    /// Background work runs; its buttons wait.
    pub busy: bool,
    /// A package downloaded from GitHub or chosen as a file, inspected and
    /// waiting for the user to install it.
    pub offer: Option<Offered>,
    /// The updates Check for Updates found.
    pub updates: Vec<github::Update>,
    /// The repository typed for Install from GitHub….
    repository: String,
    /// Install from GitHub… is open.
    github: bool,
    /// Load from Folder… and the folder watch are offered.
    developer: bool,
    watched: Option<Instant>,
    /// Text settings being typed, by extension and setting.
    drafts: HashMap<(String, String), String>,
    /// Number and Angle settings being dragged, by extension and setting
    /// ([`number_control`]).
    numbers: HashMap<(String, String), f64>,
}

/// A package inspected for the user to review before it is installed: its
/// manifest, its access and the Not reviewed warning are shown first.
pub(in crate::app) enum Offered {
    /// Downloaded from a repository's releases.
    Github(Box<github::Offer>),
    /// A `.rmext` file chosen with Install from File….
    File {
        path: PathBuf,
        manifest: Box<rawmakase_extensions::Manifest>,
    },
}

impl Offered {
    fn manifest(&self) -> &rawmakase_extensions::Manifest {
        match self {
            Self::Github(offer) => &offer.manifest,
            Self::File { manifest, .. } => manifest,
        }
    }
}

impl Page {
    /// Shows extension `id`'s page the next time the list is drawn.
    pub(in crate::app) fn select(&mut self, id: &str) {
        self.selected = Some(id.to_string());
    }
}

impl Editor {
    /// The page was opened: `extensions.json` is read again, so what the
    /// command line changed meanwhile shows.
    pub(in crate::app) fn extensions_page_opened(&mut self) {
        self.reload_extensions();
    }

    pub(in crate::app) fn extensions_page(&mut self, ui: &mut egui::Ui) {
        if !self.extensions.active() {
            group(ui, "Extensions");
            hint(ui, "Extensions are off in this editor.");
            return;
        }
        if self.extension_page.developer {
            let due = self
                .extension_page
                .watched
                .is_none_or(|at| at.elapsed() >= WATCH);
            if due {
                self.extension_page.watched = Some(Instant::now());
                self.extensions.reload_changed_folders();
                self.sync_extension_commands();
            }
            ui.ctx().request_repaint_after(WATCH);
        }
        self.extensions_install(ui);
        if let Some(offer) = self.extension_page.offer.take() {
            self.extension_offer(ui, offer);
        }
        group(ui, "Extensions");
        if self.extensions.extensions().is_empty() {
            hint(
                ui,
                "No extensions yet. Install one from GitHub or from a .rmext file.",
            );
        }
        let mut chosen = None;
        for e in self.extensions.extensions() {
            let selected = self.extension_page.selected.as_deref() == Some(e.id());
            ui.horizontal(|ui| {
                if ui.selectable_label(selected, e.name()).clicked() {
                    chosen = Some(e.id().to_string());
                }
                let reviewed = if e.origin == Origin::Installed {
                    " · Not reviewed"
                } else {
                    ""
                };
                hint(
                    ui,
                    &format!(
                        "{} · {}{reviewed} · {}",
                        e.manifest.extension.version,
                        source_label(e),
                        state(e)
                    ),
                );
                if self.extension_page.updates.iter().any(|u| u.id == e.id()) {
                    ui.label(
                        egui::RichText::new("Update")
                            .color(crate::app::ui::theme::palette(ui.ctx()).accent()),
                    );
                }
            });
        }
        if let Some(id) = chosen {
            self.extension_page.selected = Some(id);
        }
        let selected = self
            .extension_page
            .selected
            .clone()
            .filter(|id| self.extensions.get(id).is_some())
            .or_else(|| {
                self.extensions
                    .extensions()
                    .first()
                    .map(|e| e.id().to_string())
            });
        self.extension_page.selected.clone_from(&selected);
        if let Some(id) = selected {
            gap(ui);
            self.extension_detail(ui, &id);
        }
    }

    /// Install from GitHub…, Install from File…, Check for Updates and, in
    /// developer mode, Load from Folder….
    fn extensions_install(&mut self, ui: &mut egui::Ui) {
        group(ui, "Add extensions");
        let busy = self.extension_page.busy || self.activity.is_busy();
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!busy, egui::Button::new("Install from GitHub…"))
                .clicked()
            {
                self.extension_page.github = !self.extension_page.github;
            }
            if ui
                .add_enabled(!busy, egui::Button::new("Install from File…"))
                .clicked()
            {
                self.extension_dialog(PathTarget::Package);
            }
            if ui
                .add_enabled(!busy, egui::Button::new("Check for Updates"))
                .clicked()
            {
                self.check_extension_updates();
            }
            if self.extension_page.developer
                && ui
                    .add_enabled(!busy, egui::Button::new("Load from Folder…"))
                    .clicked()
            {
                self.extension_dialog(PathTarget::Folder);
            }
        });
        if self.extension_page.github {
            form_row(ui, "Repository", |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.extension_page.repository)
                        .hint_text("https://github.com/owner/repository")
                        .desired_width(300.),
                );
                if ui.add_enabled(!busy, egui::Button::new("Find")).clicked() {
                    self.inspect_repository();
                }
            });
        }
        form_row(ui, "Developer mode", |ui| {
            ui.checkbox(
                &mut self.extension_page.developer,
                "Load extensions from folders, reloaded as they change",
            );
        });
        if busy && self.extension_page.busy {
            form_row(ui, "", |ui| {
                ui.spinner();
            });
        }
        gap(ui);
    }

    fn inspect_repository(&mut self) {
        let Some(store) = self.extensions.store().cloned() else {
            return;
        };
        let app = self.extensions.app_version().clone();
        let url = self.extension_page.repository.trim().to_string();
        self.extension_work(
            move || {
                super::Done::Offered(
                    github::inspect(&store, &github::Https, &url, &app)
                        .map(Box::new)
                        .map_err(|e| e.to_string()),
                )
            },
            |why| super::Done::Offered(Err(why)),
        );
    }

    fn check_extension_updates(&mut self) {
        let Some(store) = self.extensions.store().cloned() else {
            return;
        };
        let app = self.extensions.app_version().clone();
        self.extension_work(
            move || {
                super::Done::Checked(
                    github::check_updates(&store, &github::Https, &app)
                        .map(Box::new)
                        .map_err(|e| e.to_string()),
                )
            },
            |why| super::Done::Checked(Err(why)),
        );
    }

    /// A downloaded or chosen package with its manifest and the trust
    /// warning, to install or drop.
    fn extension_offer(&mut self, ui: &mut egui::Ui, offer: Offered) {
        let m = offer.manifest();
        group(
            ui,
            &format!("Install {} {}?", m.extension.name, m.extension.version),
        );
        if let Some(description) = &m.extension.description {
            form_row(ui, "", |ui| hint(ui, description));
        }
        let from = match &offer {
            Offered::Github(offer) => format!("{} · {}", offer.repository, offer.release),
            Offered::File { path, .. } => crate::app::ui::widgets::pretty_path(path),
        };
        form_row(ui, "From", |ui| {
            ui.add(egui::Label::new(from).truncate());
        });
        form_row(ui, "Access", |ui| hint(ui, &access(m)));
        form_row(ui, "Not reviewed", |ui| {
            hint(
                ui,
                &format!("{NOT_REVIEWED} Install it only if you trust its author."),
            );
        });
        let (mut install, mut cancel) = (false, false);
        ui.horizontal(|ui| {
            install = ui.button("Install").clicked();
            cancel = ui.button("Cancel").clicked();
        });
        gap(ui);
        if install {
            self.install_offered(offer);
            return;
        }
        let keep = !cancel;
        if keep {
            self.extension_page.offer = Some(offer);
        }
    }

    /// Installs the package the user reviewed, off the UI thread.
    pub(in crate::app) fn install_offered(&mut self, offer: Offered) {
        match offer {
            Offered::Github(offer) => self.install_offer(*offer),
            Offered::File { path, manifest } => {
                let Some(store) = self.extensions.store().cloned() else {
                    return;
                };
                let app = self.extensions.app_version().clone();
                self.extension_work(
                    move || {
                        // The file is read again: what is installed must be
                        // what was shown.
                        let installed = store
                            .inspect(&path, &app)
                            .map_err(|e| e.to_string())
                            .and_then(|now| {
                                if now.manifest == *manifest {
                                    Ok(())
                                } else {
                                    Err(format!("{} changed since it was read", path.display()))
                                }
                            })
                            .and_then(|()| {
                                let source = rawmakase_extensions::state::Source::File {
                                    path: path.clone(),
                                };
                                store
                                    .install(&path, &source, &app)
                                    .map(|i| i.manifest.extension.name)
                                    .map_err(|e| e.to_string())
                            });
                        super::Done::Installed(installed)
                    },
                    |why| super::Done::Installed(Err(why)),
                );
            }
        }
    }

    /// One extension's page.
    fn extension_detail(&mut self, ui: &mut egui::Ui, id: &str) {
        let Some(e) = self.extensions.get(id) else {
            return;
        };
        let m = e.manifest.clone();
        let origin = e.origin.clone();
        let enabled = e.enabled();
        let failed = e.failed;
        let errors: Vec<String> = e.errors.iter().cloned().collect();
        let problem = e.problem.clone();
        let settings = e.settings();
        let log = self
            .extensions
            .store()
            .map(|s| s.extension_data_dir(id).join("log.txt"));
        group(ui, &m.extension.name);
        if let Some(description) = &m.extension.description {
            form_row(ui, "", |ui| hint(ui, description));
        }
        form_row(ui, "Version", |ui| {
            ui.label(format!("{} · {}", m.extension.version, source_label(e)));
        });
        if !m.extension.authors.is_empty() {
            form_row(ui, "Author", |ui| hint(ui, &m.extension.authors.join(", ")));
        }
        if let Some(repository) = &m.extension.repository {
            form_row(ui, "Repository", |ui| {
                if ui.link(repository).clicked() {
                    let _ = crate::platform::web::open(repository);
                }
            });
        }
        form_row(ui, "Access", |ui| hint(ui, &access(&m)));
        if origin == Origin::Installed {
            form_row(ui, "Not reviewed", |ui| hint(ui, NOT_REVIEWED));
        }
        form_row(ui, "State", |ui| hint(ui, &state(e)));
        if let Some(problem) = &problem {
            form_row(ui, "", |ui| hint(ui, problem));
        }
        let update = self.extension_page.updates.iter().position(|u| u.id == id);
        ui.horizontal_wrapped(|ui| {
            if enabled {
                if ui.button("Disable").clicked() {
                    self.set_extension_enabled(id, false);
                }
            } else if ui.button("Enable").clicked() {
                self.set_extension_enabled(id, true);
            }
            if self.extensions.command_running(id) && ui.button("Stop").clicked() {
                let outcomes = self.extensions.cancel_commands(id);
                self.extension_outcomes(outcomes);
            }
            if failed && ui.button("Retry").clicked() {
                self.extensions.retry(id);
            }
            if origin != Origin::Bundled && ui.button("Uninstall").clicked() {
                self.uninstall_extension(id);
            }
            if let Some(at) = update
                && ui
                    .add_enabled(!self.extension_page.busy, egui::Button::new("Update"))
                    .clicked()
            {
                self.update_extension(at);
            }
        });
        if let Some(at) = update {
            let u = &self.extension_page.updates[at];
            let mut text = format!("{} is available.", u.offer.manifest.extension.version);
            if !u.more_access.is_empty() {
                text.push_str(&format!(
                    " It asks for more access: {}.",
                    u.more_access.join(", ")
                ));
            }
            form_row(ui, "Update", |ui| hint(ui, &text));
        }
        if !m.commands.is_empty() {
            gap(ui);
            group(ui, "Commands");
            for c in &m.commands {
                let full = format!("{id}/{}", c.id);
                form_row(ui, &c.title, |ui| {
                    if ui
                        .add_enabled(enabled && !failed, egui::Button::new("Run"))
                        .clicked()
                    {
                        // Preferences closes first, as its other buttons that
                        // act on the window do: the command's change to the
                        // open photo is then made, not refused as behind a
                        // dialog, and seen, as is what the status line says.
                        self.preferences.open = false;
                        if let Err(e) = self.run_extension_command(&full, super::RunBy::Button) {
                            self.messages.fail(format!("{}: {}", c.title, e.message));
                        }
                    }
                    hint(ui, &full);
                });
            }
        }
        if !m.settings.is_empty() {
            gap(ui);
            group(ui, "Settings");
            for s in &m.settings {
                let value = settings.get(&s.id).cloned().unwrap_or(Value::Null);
                let label = s.name.clone().unwrap_or_else(|| s.id.clone());
                let mut set = None;
                form_row(ui, &label, |ui| {
                    set = self.setting_control(ui, id, s, &value);
                });
                if let Some(help) = &s.help {
                    form_row(ui, "", |ui| hint(ui, help));
                }
                if let Some(value) = set
                    && let Err(why) = self.extensions.set_setting(id, &s.id, value)
                {
                    self.messages.fail(why);
                }
            }
        }
        if enabled && !failed && problem.is_none() {
            for panel in &m.panels {
                gap(ui);
                group(ui, &panel.title);
                self.panel_ui(ui, id, &panel.id);
            }
        }
        if !m.control_adapters.is_empty() {
            gap(ui);
            group(ui, "Control adapters");
            for a in &m.control_adapters {
                // An externally launched adapter's program is started by its
                // client (an MCP client runs `rawmakase mcp`), never here.
                let started = match a.launch {
                    rawmakase_extensions::protocol::manifest::Launch::External => {
                        " · started by its client"
                    }
                    rawmakase_extensions::protocol::manifest::Launch::App => "",
                };
                form_row(ui, a.name.as_deref().unwrap_or(&a.id), |ui| {
                    hint(ui, &format!("{id}/{}{started}", a.id));
                });
            }
        }
        if !errors.is_empty() {
            gap(ui);
            group(ui, "Last errors");
            for error in &errors {
                hint(ui, error);
            }
        }
        if let Some(log) = log {
            gap(ui);
            group(ui, "Log");
            form_row(ui, "Log file", |ui| {
                if ui
                    .add_enabled(
                        log.exists(),
                        egui::Button::new(crate::platform::reveal::LABEL),
                    )
                    .clicked()
                {
                    let _ = crate::platform::reveal::reveal(&log);
                }
                ui.add(egui::Label::new(crate::app::ui::widgets::pretty_path(&log)).truncate());
            });
        }
    }

    /// A setting's control, drawn from its `[[setting]]`; the new value once
    /// committed.
    fn setting_control(
        &mut self,
        ui: &mut egui::Ui,
        id: &str,
        s: &rawmakase_extensions::protocol::manifest::Setting,
        value: &Value,
    ) -> Option<Value> {
        match &s.control {
            SettingControl::Slider([min, max]) => number_control(
                &mut self.extension_page.numbers,
                (id.to_string(), s.id.clone()),
                value.as_f64().unwrap_or(*min),
                |v| {
                    let mut slider = egui::Slider::new(v, *min..=*max);
                    if let Some(step) = s.step {
                        slider = slider.step_by(step);
                    }
                    if let Some(unit) = &s.unit {
                        slider = slider.suffix(format!(" {unit}"));
                    }
                    ui.add(slider)
                },
            ),
            SettingControl::Number([min, max]) => number_control(
                &mut self.extension_page.numbers,
                (id.to_string(), s.id.clone()),
                value.as_f64().unwrap_or(*min),
                |v| ui.add(egui::DragValue::new(v).range(*min..=*max)),
            ),
            SettingControl::Angle => number_control(
                &mut self.extension_page.numbers,
                (id.to_string(), s.id.clone()),
                value.as_f64().unwrap_or(0.),
                |v| ui.add(egui::DragValue::new(v).suffix("°")),
            ),
            SettingControl::Toggle => {
                let mut v = value.as_bool().unwrap_or(false);
                ui.checkbox(&mut v, "").changed().then(|| Value::from(v))
            }
            SettingControl::Choice(options) => {
                let mut out = None;
                egui::ComboBox::from_id_salt(("extension-setting", id, s.id.as_str()))
                    .selected_text(value.as_str().unwrap_or(""))
                    .show_ui(ui, |ui| {
                        for option in options {
                            if ui
                                .selectable_label(value.as_str() == Some(option), option)
                                .clicked()
                            {
                                out = Some(Value::from(option.clone()));
                            }
                        }
                    });
                out.filter(|v| v != value)
            }
            SettingControl::Segmented(options) => {
                let mut out = None;
                for option in options {
                    if ui
                        .selectable_label(value.as_str() == Some(option), option)
                        .clicked()
                    {
                        out = Some(Value::from(option.clone()));
                    }
                }
                out.filter(|v| v != value)
            }
            SettingControl::Text => {
                let key = (id.to_string(), s.id.clone());
                let mut text = self
                    .extension_page
                    .drafts
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| value.as_str().unwrap_or("").to_string());
                let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(260.));
                if r.changed() {
                    self.extension_page.drafts.insert(key.clone(), text.clone());
                }
                if r.lost_focus() {
                    self.extension_page.drafts.remove(&key);
                    return (value.as_str() != Some(&text)).then(|| Value::from(text));
                }
                None
            }
            SettingControl::Color => {
                let c: Vec<f32> = value
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_f64)
                            .map(|c| c as f32)
                            .collect()
                    })
                    .unwrap_or_default();
                let mut rgb = [
                    c.first().copied().unwrap_or(0.),
                    c.get(1).copied().unwrap_or(0.),
                    c.get(2).copied().unwrap_or(0.),
                ];
                let r = ui.color_edit_button_rgb(&mut rgb);
                let released = !ui.input(|i| i.pointer.any_down());
                (r.changed() && released)
                    .then(|| Value::from(rgb.iter().map(|c| f64::from(*c)).collect::<Vec<_>>()))
            }
            SettingControl::File | SettingControl::Folder => {
                let folder = matches!(s.control, SettingControl::Folder);
                ui.add(egui::Label::new(value.as_str().unwrap_or("None chosen")).truncate());
                if ui.button("Choose…").clicked() {
                    self.extension_dialog(PathTarget::Setting {
                        extension: id.to_string(),
                        setting: s.id.clone(),
                        folder,
                    });
                }
                None
            }
        }
    }

    /// A panel of the extension's page: opened once its extension runs, then
    /// drawn from what its program last sent.
    fn panel_ui(&mut self, ui: &mut egui::Ui, id: &str, panel: &str) {
        let opened = self.extensions.panel(id, panel).is_some_and(|v| v.opened);
        if !opened && let Err(why) = self.extensions.open_panel(id, panel) {
            hint(ui, &why);
            return;
        }
        let Some(view) = self.extensions.panel_mut(id, panel) else {
            return;
        };
        if let Some(error) = view.error.clone() {
            hint(ui, &error);
        }
        if view.waiting && view.controls.is_empty() {
            ui.spinner();
        }
        let actions = panels::draw(ui, &format!("{id}/{panel}"), view);
        for action in actions {
            let result = match action {
                panels::Action::Changed { control, value } => {
                    self.extensions.panel_changed(id, panel, &control, value)
                }
                panels::Action::Clicked { control } => {
                    self.extensions.panel_clicked(id, panel, &control)
                }
                panels::Action::Choose {
                    control,
                    folder,
                    extensions,
                } => {
                    self.extension_dialog(PathTarget::Control {
                        extension: id.to_string(),
                        panel: panel.to_string(),
                        control,
                        folder,
                        extensions,
                    });
                    Ok(())
                }
            };
            if let Err(why) = result {
                self.messages.fail(why);
            }
        }
    }

    /// Installs update `at` of the last Check for Updates.
    fn update_extension(&mut self, at: usize) {
        let update = self.extension_page.updates.remove(at);
        self.install_offer(update.offer);
    }

    /// Installs a downloaded package off the UI thread.
    fn install_offer(&mut self, offer: github::Offer) {
        let Some(store) = self.extensions.store().cloned() else {
            return;
        };
        let app = self.extensions.app_version().clone();
        self.extension_work(
            move || {
                super::Done::Installed(
                    store
                        .install_offer(offer, &app)
                        .map(|i| i.manifest.extension.name)
                        .map_err(|e| e.to_string()),
                )
            },
            |why| super::Done::Installed(Err(why)),
        );
    }
}

/// A number setting's control, which `add` draws from the value being
/// dragged, else from `saved`; the value to commit, once the drag ends or a
/// value typed in lands. Nothing is committed while it is dragged, and the
/// value dragged to is kept in `numbers` until then: the setting holds its
/// old value meanwhile, and egui reports a drag's end in a frame that moves
/// nothing, so the value drawn then must be the one dragged to.
fn number_control(
    numbers: &mut HashMap<(String, String), f64>,
    key: (String, String),
    saved: f64,
    add: impl FnOnce(&mut f64) -> egui::Response,
) -> Option<Value> {
    let mut v = numbers.get(&key).copied().unwrap_or(saved);
    let r = add(&mut v);
    if r.dragged() {
        numbers.insert(key, v);
        return None;
    }
    let dragged = numbers.remove(&key).is_some();
    ((dragged && r.drag_stopped()) || r.changed()).then(|| Value::from(v))
}

/// What a manifest declares it reaches, in words.
fn access(m: &rawmakase_extensions::Manifest) -> String {
    let a = &m.access;
    let mut out = vec![match a.files {
        Files::Photos => "the photos RAWmakase hands it".to_string(),
        Files::Any => "any file".to_string(),
    }];
    if a.network {
        out.push("the network".into());
    }
    for device in &a.devices {
        out.push(format!("{device} devices"));
    }
    out.join(", ")
}

/// An extension's state, in words.
fn state(e: &Extension) -> String {
    if e.failed {
        "Failed".into()
    } else if !e.enabled() {
        "Disabled".into()
    } else if e.problem.is_some() {
        "Can't run".into()
    } else if e.running() {
        "Running".into()
    } else {
        "Enabled".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame drawing a Number setting saved at 0.5 as the page draws it,
    /// given `events`; where the control is.
    fn number_frame(
        ctx: &egui::Context,
        numbers: &mut HashMap<(String, String), f64>,
        events: Vec<egui::Event>,
        committed: &mut Vec<Value>,
    ) -> egui::Pos2 {
        let mut rect = egui::Rect::NOTHING;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400., 200.),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let key = ("ext".to_string(), "amount".to_string());
                    committed.extend(number_control(numbers, key, 0.5, |v| {
                        let r = ui.add(egui::DragValue::new(v).range(0. ..=100.));
                        rect = r.rect;
                        r
                    }));
                });
            },
        );
        output.textures_delta.clear();
        rect.center()
    }

    #[test]
    fn a_number_dragged_commits_where_it_was_let_go() {
        let ctx = egui::Context::default();
        let mut numbers = HashMap::new();
        let mut committed = Vec::new();
        let start = number_frame(&ctx, &mut numbers, Vec::new(), &mut committed);
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let press = vec![egui::Event::PointerMoved(start), button(start, true)];
        number_frame(&ctx, &mut numbers, press, &mut committed);
        let mut at = start;
        for _ in 0..4 {
            at.x += 10.;
            let moved = vec![egui::Event::PointerMoved(at)];
            number_frame(&ctx, &mut numbers, moved, &mut committed);
            assert!(committed.is_empty(), "nothing is committed mid-drag");
        }
        number_frame(&ctx, &mut numbers, vec![button(at, false)], &mut committed);
        number_frame(&ctx, &mut numbers, Vec::new(), &mut committed);
        let [value] = &committed[..] else {
            panic!("one commit, on release: {committed:?}")
        };
        let value = value.as_f64().unwrap();
        assert!(value > 20., "committed {value}, not the value dragged to");
        assert!(numbers.is_empty());
    }
}
