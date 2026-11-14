//! Preferences > Extensions: a table of the loaded extensions with their
//! version, source, state and available update, each turned on or off in its
//! row; each extension's own page, opened from its row, with its settings and
//! panels, its commands, and what it declares, its last errors and its log;
//! installing from GitHub, from a file or, in developer mode, from a folder;
//! and the extensions RAWmakase lists but doesn't bundle ([`AVAILABLE`]), each
//! installed through Install from GitHub with its confirmation and Not
//! reviewed mark.
//!
//! Every change goes through the host, which writes `extensions.json` first
//! and leaves the session as it was when that fails. Downloads, installs and
//! update checks run on a thread of their own ([`Editor::extension_work`]),
//! and the page says it is busy until they report, however they end.
use super::{Extension, Origin, PathTarget, panels, source_label};
use crate::app::Editor;
use crate::app::preferences::{gap, group, hint};
use crate::app::shell::automation::LocalAccess;
use crate::app::ui::design::{Tone, button, inset, space, text};
use crate::app::ui::icons::{self, Icon};
use crate::app::ui::theme;
use crate::app::ui::widgets::{form_row, menu_item, modal_frame, primary_button};
use eframe::egui::{self, Color32, Sense, Stroke, Vec2};
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

/// The bundled MCP extension: its clients reach the editor through local
/// access, which its page shows as its settings.
const MCP: &str = "rawmakase.mcp";

/// A row of the extensions table, in points.
const ROW: f32 = 48.;
/// The Version and Status columns' width.
const COLUMN: f32 = 86.;

/// An extension RAWmakase lists for installing, without bundling it.
pub(in crate::app) struct Available {
    pub(in crate::app) id: &'static str,
    pub(in crate::app) name: &'static str,
    /// One line of what it does.
    pub(in crate::app) line: &'static str,
    /// The GitHub repository whose releases it installs from.
    pub(in crate::app) repository: &'static str,
}

/// The extensions Preferences > Extensions offers to install, while they
/// aren't installed.
pub(in crate::app) const AVAILABLE: &[Available] = &[Available {
    id: "dev.pch.ai-remove",
    name: "AI Remove",
    line: "Experimental: removes what you paint over in the Remove tool with a hosted \
           model and your own API key (fal.ai or Replicate).",
    repository: "https://github.com/pch/rawmakase-ai-remove",
}];

/// What the page keeps between frames.
#[derive(Default)]
pub(in crate::app) struct Page {
    /// The extension whose own page shows in place of the list.
    open: Option<String>,
    /// The part of that page shown.
    section: Section,
    /// Background work runs; its buttons wait.
    pub busy: bool,
    /// A package downloaded from GitHub or chosen as a file, inspected and
    /// waiting for the user to install it.
    pub offer: Option<Offered>,
    /// The updates Check for Updates found.
    pub updates: Vec<github::Update>,
    /// The repository typed for Install from GitHub….
    repository: String,
    /// Install from GitHub…'s sheet is open.
    github: bool,
    /// Uninstall was clicked for this extension, waiting for the
    /// confirmation.
    uninstall: Option<String>,
    /// What couldn't be loaded when the page opened, such as an installed
    /// extension whose manifest this version can't read.
    problems: Vec<String>,
    /// Load from Folder… and the folder watch are offered.
    developer: bool,
    watched: Option<Instant>,
    /// Text settings being typed, by extension and setting.
    drafts: HashMap<(String, String), String>,
    /// Number and Angle settings being dragged, by extension and setting
    /// ([`number_control`]).
    numbers: HashMap<(String, String), f64>,
    /// Secret settings being typed, by extension and setting: never filled
    /// from what is kept, and forgotten once saved.
    secrets: HashMap<(String, String), String>,
}

/// The parts of an extension's own page, each a tab. A part the extension
/// declares nothing for has no tab.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(in crate::app) enum Section {
    /// Its `[[setting]]`s, its Preferences panels and its photo sources.
    #[default]
    Settings,
    /// Its commands, each with Run.
    Commands,
    /// What it is and reaches, its last errors, its log and Uninstall.
    About,
}

impl Section {
    fn title(self) -> &'static str {
        match self {
            Section::Settings => "Settings",
            Section::Commands => "Commands",
            Section::About => "About",
        }
    }
    /// The parts `m` has something to show in, in tab order. About always
    /// has.
    fn of(m: &rawmakase_extensions::Manifest) -> Vec<Section> {
        let mut out = Vec::new();
        let app_settings = m.extension.id == MCP;
        if app_settings || !m.settings.is_empty() || !m.panels.is_empty() || !m.sources.is_empty() {
            out.push(Section::Settings);
        }
        if !m.commands.is_empty() {
            out.push(Section::Commands);
        }
        out.push(Section::About);
        out
    }
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
    /// Shows extension `id`'s own page, at its first part, the next time the
    /// page is drawn.
    pub(in crate::app) fn select(&mut self, id: &str) {
        self.open = Some(id.to_string());
        self.section = Section::default();
    }
    /// Shows `section` of the open extension's page.
    #[cfg(test)]
    pub(in crate::app) fn show(&mut self, section: Section) {
        self.section = section;
    }
    /// What couldn't be loaded when the page opened.
    #[cfg(test)]
    pub(in crate::app) fn problems(&self) -> &[String] {
        &self.problems
    }
    /// Back from an extension's page to the list.
    pub(in crate::app) fn back(&mut self) {
        self.open = None;
    }
    /// The extension whose own page shows, if one does.
    pub(in crate::app) fn opened(&self) -> Option<&str> {
        self.open.as_deref()
    }
}

/// What a row of the extensions table shows.
struct Row {
    id: String,
    name: String,
    description: String,
    version: String,
    /// The version Check for Updates found.
    update: Option<String>,
    /// Where it came from, or that it isn't reviewed.
    badge: (&'static str, bool),
    status: Status,
    /// What the status means for it, shown on hover.
    why: String,
    enabled: bool,
}

/// What a row was clicked for.
enum RowClick {
    Open,
    Switch,
}

impl Editor {
    /// Extension `id`'s status as its row says it.
    #[cfg(test)]
    pub(in crate::app) fn extension_status(&self, id: &str) -> &'static str {
        let e = self.extensions.get(id).expect("loaded");
        Status::of(e, self.local_access()).text()
    }

    /// Commits what is being typed on the open extension's page. A text
    /// field commits when it loses focus, which it hears a frame later, so a
    /// tab or page left before then (another tab, Back, Escape, another
    /// Preferences page, closing Preferences) would no longer draw it to hear
    /// it, and the edit would stay a draft.
    pub(in crate::app) fn commit_extension_drafts(&mut self) {
        // A draft stays until it is saved, so one that fails shows again
        // when its page does.
        let drafts: Vec<_> = self
            .extension_page
            .drafts
            .iter()
            .map(|(key, text)| (key.clone(), text.clone()))
            .collect();
        for ((extension, setting), text) in drafts {
            let saved = self
                .extensions
                .get(&extension)
                .and_then(|e| e.settings().get(&setting).cloned());
            let value = Value::from(text);
            let result = if saved.as_ref() == Some(&value) {
                Ok(())
            } else {
                self.extensions.set_setting(&extension, &setting, value)
            };
            match result {
                Ok(()) => {
                    self.extension_page.drafts.remove(&(extension, setting));
                }
                Err(why) => self.messages.fail(why),
            }
        }
        let Some(id) = self.extension_page.open.clone() else {
            return;
        };
        let panels: Vec<String> = self
            .extensions
            .get(&id)
            .map(|e| e.manifest.panels.iter().map(|p| p.id.clone()).collect())
            .unwrap_or_default();
        for panel in panels {
            // Sent drafts stay until the program answers with a new tree, as
            // they do when a field commits on its own.
            let unsent = self
                .extensions
                .panel(&id, &panel)
                .map(panels::unsent)
                .unwrap_or_default();
            for (control, value) in unsent {
                if let Err(why) = self.extensions.panel_changed(&id, &panel, &control, value) {
                    self.messages.fail(why);
                }
            }
        }
    }

    /// Back from an extension's page to the list, keeping what was typed
    /// there.
    pub(in crate::app) fn leave_extension_page(&mut self) {
        self.commit_extension_drafts();
        self.extension_page.back();
    }

    /// The page was opened: `extensions.json` is read again, so what the
    /// command line changed meanwhile shows.
    pub(in crate::app) fn extensions_page_opened(&mut self) {
        self.extensions.reload();
        self.sync_extension_commands();
        // Said on the page, in full, rather than cut short in the status line.
        self.extension_page.problems = self.extensions.take_errors();
    }

    /// The name of the extension whose own page shows, for the bar over the
    /// page: "Preferences / Extensions / MIDI".
    pub(in crate::app) fn extension_crumb(&self) -> Option<&str> {
        let id = self.extension_page.opened()?;
        self.extensions.get(id).map(Extension::name)
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
        if let Some(offer) = self.extension_page.offer.take() {
            self.extension_offer(ui.ctx(), offer);
        } else if self.extension_page.github {
            self.github_sheet(ui.ctx());
        } else if let Some(id) = self.extension_page.uninstall.clone() {
            self.uninstall_confirmation(ui.ctx(), &id);
        }
        // An extension uninstalled, or gone from its folder, leaves the list.
        let open = self
            .extension_page
            .open
            .clone()
            .filter(|id| self.extensions.get(id).is_some());
        self.extension_page.open.clone_from(&open);
        match open {
            Some(id) => self.extension_detail(ui, &id),
            None => self.extension_list(ui),
        }
    }

    /// The list: Check for Updates and Add Extension over a table of the
    /// extensions, and developer mode under it.
    fn extension_list(&mut self, ui: &mut egui::Ui) {
        let busy = self.extension_page.busy || self.activity.is_busy();
        let count = self.extensions.extensions().len();
        flush(ui, |ui| {
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.add_extension_menu(ui, busy);
                    if ui
                        .add_enabled(!busy, egui::Button::new("Check for Updates"))
                        .clicked()
                    {
                        self.check_extension_updates();
                    }
                    if self.extension_page.busy {
                        ui.spinner();
                    }
                });
            });
        });
        ui.add_space(space::S);
        self.extensions_available(ui);
        group(ui, "Installed");
        if count == 0 {
            hint(
                ui,
                "No extensions yet. Install one from GitHub or from a .rmext file.",
            );
        } else {
            let access = self.local_access();
            let rows: Vec<Row> = self
                .extensions
                .extensions()
                .iter()
                .map(|e| Row {
                    id: e.id().to_string(),
                    name: e.name().to_string(),
                    description: e.manifest.extension.description.clone().unwrap_or_default(),
                    version: e.manifest.extension.version.to_string(),
                    update: self
                        .extension_page
                        .updates
                        .iter()
                        .find(|u| u.id == e.id())
                        .map(|u| u.offer.manifest.extension.version.to_string()),
                    badge: match e.origin {
                        Origin::Installed => ("Not reviewed", true),
                        _ => (source_label(e), false),
                    },
                    status: Status::of(e, access),
                    why: Status::of(e, access).explain(e, access),
                    enabled: e.enabled(),
                })
                .collect();
            let mut clicked = None;
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.y = 0.;
                table_header(ui);
                let last = rows.len() - 1;
                for (i, row) in rows.iter().enumerate() {
                    if let Some(click) = table_row(ui, row, i == last) {
                        clicked = Some((row, click));
                    }
                }
            });
            match clicked {
                Some((row, RowClick::Open)) => self.extension_page.select(&row.id),
                Some((row, RowClick::Switch)) => {
                    self.set_extension_enabled(&row.id, !row.enabled);
                }
                None => {}
            }
        }
        gap(ui);
        if !self.extension_page.problems.is_empty() {
            group(ui, "Couldn't load");
            for problem in &self.extension_page.problems {
                hint(ui, &home_as_tilde(problem));
            }
            gap(ui);
        }
        ui.horizontal(|ui| {
            let developer = self.extension_page.developer;
            let toggled = crate::app::ui::widgets::switch(ui, developer, "Developer mode")
                .clicked()
                | ui.add(egui::Label::new("Developer mode").sense(Sense::click()))
                    .clicked();
            if toggled {
                self.extension_page.developer = !developer;
            }
            hint(ui, "Load extensions from folders, reloaded as they change");
        });
    }

    /// Add Extension: from GitHub, from a file and, in developer mode, from
    /// a folder.
    fn add_extension_menu(&mut self, ui: &mut egui::Ui, busy: bool) {
        let palette = theme::palette(ui.ctx());
        let add =
            egui::Button::new(egui::RichText::new("Add Extension").color(palette.on_accent()))
                .fill(palette.accent());
        ui.add_enabled_ui(!busy, |ui| {
            egui::containers::menu::MenuButton::from_button(add).ui(ui, |ui| {
                crate::app::ui::widgets::menu_width(ui, 190.);
                if menu_item(ui, "From GitHub…", "", true, false) {
                    self.extension_page.github = true;
                    ui.close();
                }
                if menu_item(ui, "From File…", "", true, false) {
                    self.extension_dialog(PathTarget::Package);
                    ui.close();
                }
                if self.extension_page.developer && menu_item(ui, "From Folder…", "", true, false)
                {
                    self.extension_dialog(PathTarget::Folder);
                    ui.close();
                }
            });
        });
    }

    /// Install from GitHub…: the repository to look in.
    fn github_sheet(&mut self, ctx: &egui::Context) {
        let palette = theme::palette(ctx);
        let busy = self.extension_page.busy || self.activity.is_busy();
        let (mut find, mut cancel) = (false, false);
        let response = egui::Modal::new(egui::Id::new("extension-github"))
            .frame(
                modal_frame(&palette)
                    .inner_margin(crate::app::ui::design::margin_same(inset::DIALOG)),
            )
            .show(ctx, |ui| {
                ui.set_width(440.);
                ui.label(
                    egui::RichText::new("Install from GitHub")
                        .size(text::TITLE)
                        .color(palette.tone(Tone::STRONG)),
                );
                ui.add_space(space::S);
                hint(
                    ui,
                    "The repository's latest release is downloaded and shown to you before \
                     anything is installed.",
                );
                ui.add_space(space::L);
                let field = ui.add(
                    egui::TextEdit::singleline(&mut self.extension_page.repository)
                        .hint_text("https://github.com/owner/repository")
                        .desired_width(f32::INFINITY),
                );
                if !field.has_focus() && self.extension_page.repository.is_empty() {
                    field.request_focus();
                }
                let typed = !self.extension_page.repository.trim().is_empty();
                find = typed
                    && !busy
                    && field.lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.add_space(space::XXL);
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding = button::DIALOG;
                    cancel = ui.button("Cancel").clicked();
                    ui.add_enabled_ui(typed && !busy, |ui| {
                        find |= primary_button(ui, "Find").clicked();
                    });
                });
            });
        if find {
            self.extension_page.github = false;
            self.inspect_repository();
        } else if cancel || response.should_close() {
            self.extension_page.github = false;
        }
    }

    /// Uninstall…'s question; the extension is uninstalled only once it is
    /// answered with Uninstall.
    fn uninstall_confirmation(&mut self, ctx: &egui::Context, id: &str) {
        let Some(e) = self.extensions.get(id) else {
            self.extension_page.uninstall = None;
            return;
        };
        let what = match e.origin {
            Origin::Developer => "RAWmakase forgets its folder, which stays where it is.",
            _ => "Its program and its settings are removed. Photos it added stay in the catalog.",
        };
        let title = format!("Uninstall {}?", e.name());
        let lines = [what.to_string()];
        match crate::app::ui::widgets::destructive_choice(
            ctx,
            "uninstall-extension",
            &title,
            &lines,
            "Cancel",
            &["Uninstall"],
        ) {
            None => {}
            Some(choice) => {
                self.extension_page.uninstall = None;
                if choice.is_some() {
                    self.uninstall_extension(id);
                }
            }
        }
    }

    /// The listed extensions not installed yet, each with Install, which
    /// finds its latest release as Install from GitHub… does and shows it
    /// for the user to confirm.
    fn extensions_available(&mut self, ui: &mut egui::Ui) {
        let missing: Vec<&Available> = AVAILABLE
            .iter()
            .filter(|a| self.extensions.get(a.id).is_none())
            .collect();
        if missing.is_empty() {
            return;
        }
        group(ui, "Available");
        let busy = self.extension_page.busy || self.activity.is_busy();
        for available in missing {
            form_row(ui, available.name, |ui| {
                if ui
                    .add_enabled(!busy, egui::Button::new("Install…"))
                    .on_hover_text(available.repository)
                    .clicked()
                {
                    self.extension_page.repository = available.repository.to_string();
                    self.inspect_repository();
                }
                hint(ui, available.line);
            });
        }
        form_row(ui, "", |ui| {
            hint(
                ui,
                "Not bundled with RAWmakase: each installs from its GitHub releases, \
                 marked Not reviewed.",
            );
        });
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
    fn extension_offer(&mut self, ctx: &egui::Context, offer: Offered) {
        let palette = theme::palette(ctx);
        let (mut install, mut cancel) = (false, false);
        let response = egui::Modal::new(egui::Id::new("extension-offer"))
            .frame(
                modal_frame(&palette)
                    .inner_margin(crate::app::ui::design::margin_same(inset::DIALOG)),
            )
            .show(ctx, |ui| {
                ui.set_width(480.);
                let m = offer.manifest();
                ui.label(
                    egui::RichText::new(format!(
                        "Install {} {}?",
                        m.extension.name, m.extension.version
                    ))
                    .size(text::TITLE)
                    .color(palette.tone(Tone::STRONG)),
                );
                if let Some(description) = &m.extension.description {
                    ui.add_space(space::S);
                    hint(ui, description);
                }
                ui.add_space(space::L);
                let from = match &offer {
                    Offered::Github(offer) => format!("{} · {}", offer.repository, offer.release),
                    Offered::File { path, .. } => crate::app::ui::widgets::pretty_path(path),
                };
                egui::Grid::new("extension-offer-facts")
                    .num_columns(2)
                    .spacing(Vec2::new(space::L, space::M))
                    .show(ui, |ui| {
                        fact(ui, "From");
                        ui.add(egui::Label::new(from).truncate());
                        ui.end_row();
                        fact(ui, "Access");
                        hint(ui, &access(m));
                        ui.end_row();
                    });
                ui.add_space(space::L);
                ui.label(
                    egui::RichText::new(format!(
                        "{NOT_REVIEWED} Install it only if you trust its author."
                    ))
                    .size(text::LABEL)
                    .color(palette.warning()),
                );
                ui.add_space(space::XXL);
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding = button::DIALOG;
                    cancel = ui.button("Cancel").clicked();
                    install = primary_button(ui, "Install").clicked();
                });
            });
        if install {
            self.install_offered(offer);
        } else if !(cancel || response.should_close()) {
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

    /// One extension's own page: who it is, its switch and what it can be
    /// asked to do now over tabs for its settings, commands and the rest.
    fn extension_detail(&mut self, ui: &mut egui::Ui, id: &str) {
        let Some(e) = self.extensions.get(id) else {
            return;
        };
        let m = e.manifest.clone();
        let origin = e.origin.clone();
        let enabled = e.enabled();
        let failed = e.failed;
        let access = self.local_access();
        let status = Status::of(e, access);
        let source = source_label(e);
        let problem = e.problem.clone();
        let palette = theme::palette(ui.ctx());
        let update = self.extension_page.updates.iter().position(|u| u.id == id);
        let running = self.extensions.command_running(id);

        let why = status.explain(e, access);
        flush(ui, |ui| {
            ui.spacing_mut().item_spacing.y = space::XS;
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(&m.extension.name)
                        .size(text::HERO)
                        .color(palette.tone(Tone::STRONG)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if enabled { "Enabled" } else { "Disabled" };
                    if crate::app::ui::widgets::switch(
                        ui,
                        enabled,
                        &format!("Enable {}", m.extension.name),
                    )
                    .clicked()
                    {
                        self.set_extension_enabled(id, !enabled);
                    }
                    ui.label(egui::RichText::new(label).color(palette.tone(Tone::SECONDARY)));
                    ui.add_space(space::L);
                    if let Some(at) = update {
                        let version = self.extension_page.updates[at]
                            .offer
                            .manifest
                            .extension
                            .version
                            .to_string();
                        let busy = self.extension_page.busy;
                        if ui
                            .add_enabled_ui(!busy, |ui| {
                                primary_button(ui, &format!("Update to {version}"))
                            })
                            .inner
                            .clicked()
                        {
                            self.update_extension(at);
                        }
                    }
                    if failed && ui.button("Retry").clicked() {
                        self.extensions.retry(id);
                    }
                    if running && ui.button("Stop").clicked() {
                        let outcomes = self.extensions.cancel_commands(id);
                        self.extension_outcomes(outcomes);
                    }
                });
            });
            ui.add_space(space::XS);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = space::S;
                let mut meta = format!("{} · {source}", m.extension.version);
                if !m.extension.authors.is_empty() {
                    meta.push_str(&format!(" · by {}", m.extension.authors.join(", ")));
                }
                hint(ui, &meta);
                status_label(ui, status, &why);
            });
            if let Some(description) = &m.extension.description {
                ui.label(
                    egui::RichText::new(description)
                        .size(text::BODY)
                        .color(palette.tone(Tone::SECONDARY)),
                );
            }
            if let Some(problem) = &problem {
                ui.label(
                    egui::RichText::new(problem)
                        .size(text::LABEL)
                        .color(palette.warning()),
                );
            }
            if let Some(at) = self.extension_page.updates.iter().position(|u| u.id == id) {
                let u = &self.extension_page.updates[at];
                if !u.more_access.is_empty() {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} asks for more access: {}.",
                            u.offer.manifest.extension.version,
                            u.more_access.join(", ")
                        ))
                        .size(text::LABEL)
                        .color(palette.warning()),
                    );
                }
            }

            let sections = Section::of(&m);
            if !sections.contains(&self.extension_page.section) {
                self.extension_page.section = sections[0];
            }
            // One part alone needs no tab to choose it.
            if sections.len() > 1 {
                ui.add_space(space::S);
                if let Some(section) = section_tabs(ui, &sections, self.extension_page.section) {
                    self.commit_extension_drafts();
                    self.extension_page.section = section;
                }
            }
        });
        ui.add_space(space::L);
        // Read again: the switch over them may have turned it on or off in
        // this frame, and a panel must not be opened for it as it was.
        let Some(e) = self.extensions.get(id) else {
            return;
        };
        let (enabled, failed, problem) = (e.enabled(), e.failed, e.problem.clone());
        match self.extension_page.section {
            Section::Settings => {
                self.extension_settings(ui, id, &m, enabled && !failed && problem.is_none())
            }
            Section::Commands => self.extension_commands(ui, id, &m, enabled && !failed),
            Section::About => self.extension_about(ui, id, &m, &origin),
        }
    }

    /// An extension's settings, its Preferences panels while it can run, and
    /// its photo sources.
    fn extension_settings(
        &mut self,
        ui: &mut egui::Ui,
        id: &str,
        m: &rawmakase_extensions::Manifest,
        runs: bool,
    ) {
        let Some(e) = self.extensions.get(id) else {
            return;
        };
        let enabled = e.enabled();
        let settings = e.settings();
        if id == MCP {
            group(ui, "Local access");
            self.local_access_rows(ui, "Ready for MCP clients");
            self.mcp_guide_row(ui);
            gap(ui);
        }
        if !m.settings.is_empty() {
            group(ui, "Options");
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
                if let Some(value) = set {
                    match self.extensions.set_setting(id, &s.id, value) {
                        Ok(()) => {
                            self.extension_page
                                .drafts
                                .remove(&(id.to_string(), s.id.clone()));
                        }
                        Err(why) => self.messages.fail(why),
                    }
                }
            }
            gap(ui);
        }
        for panel in &m.panels {
            group(ui, &panel.title);
            if runs {
                self.panel_ui(ui, id, &panel.id);
            } else {
                hint(
                    ui,
                    &format!("Shown while {} is on and runs.", m.extension.name),
                );
            }
            gap(ui);
        }
        if !m.sources.is_empty() {
            group(ui, "Photo sources");
            let catalog = self
                .library
                .as_ref()
                .map(|l| l.session.catalog().location().name());
            for source in &m.sources {
                let mut set = None;
                form_row(ui, source.name(), |ui| {
                    match (self.extensions.source_on(id, &source.id), &catalog) {
                        (Some(mut on), Some(catalog)) => {
                            let label = format!("Add its photos to {catalog}");
                            if ui
                                .add_enabled(enabled, egui::Checkbox::new(&mut on, label))
                                .changed()
                            {
                                set = Some(on);
                            }
                        }
                        _ => hint(ui, "Open a catalog to add its photos to"),
                    }
                });
                if let Some(on) = set
                    && let Err(why) = self.extensions.set_source_on(id, &source.id, on)
                {
                    self.messages.fail(why);
                }
            }
            gap(ui);
        }
    }

    /// An extension's commands, each with Run.
    fn extension_commands(
        &mut self,
        ui: &mut egui::Ui,
        id: &str,
        m: &rawmakase_extensions::Manifest,
        runs: bool,
    ) {
        let palette = theme::palette(ui.ctx());
        group(ui, "Commands");
        for c in &m.commands {
            let full = format!("{id}/{}", c.id);
            let unsupported = super::command_input::unsupported(c.input);
            // A row of its own rather than a form row: a title can be
            // longer than the form's label column.
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&c.title).color(palette.tone(Tone::TEXT)));
                hint(ui, &full);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let run =
                        ui.add_enabled(runs && unsupported.is_none(), egui::Button::new("Run"));
                    let run = match unsupported {
                        Some(why) => run.on_disabled_hover_text(why),
                        None => run,
                    };
                    if run.clicked() {
                        // Preferences closes first, as its other buttons that
                        // act on the window do: the command's change to the
                        // open photo is then made, not refused as behind a
                        // dialog, and seen, as is what the status line says.
                        self.preferences.open = false;
                        if let Err(e) = self.run_extension_command(&full, super::RunBy::Button) {
                            self.messages.fail(format!("{}: {}", c.title, e.message));
                        }
                    }
                });
            });
        }
        gap(ui);
    }

    /// What an extension is and reaches, what it provides, its last errors,
    /// its log, and Uninstall.
    fn extension_about(
        &mut self,
        ui: &mut egui::Ui,
        id: &str,
        m: &rawmakase_extensions::Manifest,
        origin: &Origin,
    ) {
        let Some(e) = self.extensions.get(id) else {
            return;
        };
        let errors: Vec<String> = e.errors.iter().cloned().collect();
        let source = source_label(e);
        let log = self
            .extensions
            .store()
            .map(|s| s.extension_data_dir(id).join("log.txt"));
        group(ui, "Details");
        form_row(ui, "Version", |ui| {
            ui.label(format!("{} · {source}", m.extension.version));
        });
        form_row(ui, "Identifier", |ui| hint(ui, id));
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
        form_row(ui, "Access", |ui| hint(ui, &access(m)));
        if *origin == Origin::Installed {
            form_row(ui, "Not reviewed", |ui| hint(ui, NOT_REVIEWED));
        }
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
                hint(ui, &format!("Control adapter {id}/{}{started}", a.id));
            });
        }
        if let Some(log) = &log {
            form_row(ui, "Log", |ui| {
                if ui
                    .add_enabled(
                        log.exists(),
                        egui::Button::new(crate::platform::reveal::LABEL),
                    )
                    .clicked()
                {
                    let _ = crate::platform::reveal::reveal(log);
                }
                ui.add(egui::Label::new(crate::app::ui::widgets::pretty_path(log)).truncate());
            });
        }
        if *origin != Origin::Bundled {
            form_row(ui, "", |ui| {
                if ui.button("Uninstall…").clicked() {
                    self.extension_page.uninstall = Some(id.to_string());
                }
            });
        }
        gap(ui);
        if !errors.is_empty() {
            group(ui, "Last errors");
            for error in &errors {
                hint(ui, error);
            }
            gap(ui);
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
                // The draft stays until the caller saves it, so a save that
                // fails leaves what was typed.
                if r.lost_focus() {
                    if value.as_str() == Some(&text) {
                        self.extension_page.drafts.remove(&key);
                        return None;
                    }
                    return Some(Value::from(text));
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
            SettingControl::Secret => {
                self.secret_control(ui, id, &s.id);
                None
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

    /// A secret setting: whether one is kept, a field to type a new one into
    /// (shown as dots, never filled with what is kept), Save and Remove. It
    /// says Set only once the credential store has it.
    fn secret_control(&mut self, ui: &mut egui::Ui, id: &str, setting: &str) {
        use super::secrets::{State, Writing};
        let Some(e) = self.extensions.get(id) else {
            return;
        };
        let writing = e.secret_writes.get(setting).cloned();
        let state = e.secret(setting).cloned().unwrap_or(State::Unset);
        let key = (id.to_string(), setting.to_string());
        let mut text = self
            .extension_page
            .secrets
            .get(&key)
            .cloned()
            .unwrap_or_default();
        let saving = writing == Some(Writing::Saving);
        let mut save = false;
        let mut remove = false;
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                let field = ui.add_enabled(
                    !saving,
                    egui::TextEdit::singleline(&mut text)
                        .password(true)
                        .hint_text(match state {
                            State::Set(_) => "Type a new one to replace it",
                            _ => "Paste it here",
                        })
                        .desired_width(220.),
                );
                if field.changed() {
                    self.extension_page
                        .secrets
                        .insert(key.clone(), text.clone());
                }
                let typed = !text.trim().is_empty();
                save = ui
                    .add_enabled(!saving && typed, egui::Button::new("Save"))
                    .clicked()
                    && typed;
                if matches!(state, State::Set(_) | State::Unreadable(_)) {
                    remove = ui
                        .add_enabled(!saving, egui::Button::new("Remove"))
                        .clicked();
                }
            });
            let status = match (&writing, &state) {
                (Some(Writing::Saving), _) => {
                    "Saving to the system's credential store…".to_string()
                }
                (Some(Writing::Failed(why)), _) => format!("Not saved: {why}"),
                (None, State::Loading) => "Reading from the system's credential store…".into(),
                (None, State::Set(_)) => "Set · kept in the system's credential store".into(),
                (None, State::Unset) => "Not set".into(),
                (None, State::Unreadable(why)) => format!("Set, but it can't be read: {why}"),
            };
            hint(ui, &status);
        });
        let result = if save {
            self.extension_page.secrets.remove(&key);
            self.extensions.set_secret(id, setting, Some(text))
        } else if remove {
            self.extensions.set_secret(id, setting, None)
        } else {
            Ok(())
        };
        if let Err(why) = result {
            self.messages.fail(why);
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

/// An extension's state, as its row and page say it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Status {
    Failed,
    Disabled,
    /// On, and missing something to run, such as its program.
    CantRun,
    Running,
    /// On, its program not running now: started when it is needed.
    Ready,
}

impl Status {
    /// `e`'s status, with `access` deciding whether MCP's clients can reach
    /// the editor.
    fn of(e: &Extension, access: LocalAccess) -> Self {
        if e.failed {
            Self::Failed
        } else if !e.enabled() {
            Self::Disabled
        } else if e.problem.is_some() || (e.id() == MCP && access != LocalAccess::Ready) {
            Self::CantRun
        } else if e.running() {
            Self::Running
        } else {
            Self::Ready
        }
    }
    fn text(self) -> &'static str {
        match self {
            Self::Failed => "Failed",
            Self::Disabled => "Disabled",
            Self::CantRun => "Can't run",
            Self::Running => "Running",
            Self::Ready => "Ready",
        }
    }
    /// What the status means for `e`, in a sentence.
    fn explain(self, e: &Extension, access: LocalAccess) -> String {
        match self {
            Self::Failed => "Its program stopped working. Retry it from its page.".to_string(),
            Self::Disabled => "Off: its commands, panels and adapters aren't available.".into(),
            Self::CantRun => e.problem.clone().unwrap_or_else(|| match access {
                LocalAccess::Failed => {
                    "Local access couldn't start, so MCP clients can't connect.".into()
                }
                _ => {
                    "Local access is off, so MCP clients can't connect. Turn it on in its settings."
                        .into()
                }
            }),
            Self::Running => "On, and its program is running.".into(),
            Self::Ready if !e.manifest.runs_in_app() => {
                "On. Its clients start its program when they need it.".into()
            }
            Self::Ready => "On. Its program starts when it is needed.".into(),
        }
    }
    fn color(self, palette: &theme::Palette) -> Color32 {
        match self {
            Self::Failed => palette.danger(),
            Self::CantRun => palette.warning(),
            Self::Running => palette.accent(),
            Self::Disabled | Self::Ready => palette.tone(Tone::FAINT),
        }
    }
}

/// A status's dot and word, laid out, saying `why` on hover.
fn status_label(ui: &mut egui::Ui, status: Status, why: &str) {
    let palette = theme::palette(ui.ctx());
    let (dot, _) = ui.allocate_exact_size(Vec2::new(8., 16.), Sense::hover());
    ui.painter()
        .circle_filled(dot.center(), 3.5, status.color(&palette));
    ui.label(
        egui::RichText::new(status.text())
            .size(text::LABEL)
            .color(palette.tone(Tone::SECONDARY)),
    )
    .on_hover_text(why);
}

/// `text` with the home folder written `~`, as paths are shown elsewhere.
fn home_as_tilde(text: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() => text.replace(&home, "~"),
        _ => text.to_string(),
    }
}

/// `add` as wide as the cards, its edges in line with theirs rather than with
/// what they hold. It is laid out in a child the page isn't widened for (a
/// page wider than its scroll area would scroll sideways), then the height it
/// took is taken from the page.
fn flush<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let free = ui.available_rect_before_wrap();
    let rect = egui::Rect::from_min_max(
        egui::pos2(free.left() - space::XL, free.top()),
        egui::pos2(free.right() + space::XL, f32::INFINITY),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    let inner = add(&mut child);
    let height = child.min_rect().height();
    ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    inner
}

/// A fact's name in the install sheet, in the form rows' colour.
fn fact(ui: &mut egui::Ui, name: &str) {
    ui.label(egui::RichText::new(name).color(theme::palette(ui.ctx()).tone(Tone::SECONDARY)));
}

/// `text` in one line no wider than `width`, cut with an ellipsis.
fn one_line(
    ui: &egui::Ui,
    text: &str,
    size: f32,
    color: Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(
        text.to_string(),
        egui::FontId::proportional(size),
        color,
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(0.));
    ui.fonts_mut(|f| f.layout_job(job))
}

/// Where a table row's columns are: name, version, status, switch and
/// chevron, from `row`'s right end.
struct Columns {
    name: egui::Rangef,
    version: f32,
    status: f32,
    switch: egui::Rect,
    chevron: egui::Pos2,
}

impl Columns {
    fn of(row: egui::Rect) -> Self {
        let chevron = egui::pos2(row.right() - 6., row.center().y);
        let switch = egui::Rect::from_center_size(
            egui::pos2(chevron.x - 30., row.center().y),
            Vec2::new(26., 20.),
        );
        let status = switch.left() - space::L - COLUMN;
        let version = status - space::M - COLUMN;
        Self {
            name: egui::Rangef::new(row.left(), version - space::L),
            version,
            status,
            switch,
            chevron,
        }
    }
}

/// The table's column titles.
fn table_header(ui: &mut egui::Ui) {
    let palette = theme::palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.), Sense::hover());
    let columns = Columns::of(rect);
    let color = palette.tone(Tone::MUTED);
    let font = egui::FontId::proportional(text::SMALL);
    for (x, title) in [
        (columns.name.min, "Name"),
        (columns.version, "Version"),
        (columns.status, "Status"),
    ] {
        ui.painter().text(
            egui::pos2(x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            title,
            font.clone(),
            color,
        );
    }
    ui.painter().text(
        egui::pos2(columns.switch.center().x, rect.center().y),
        egui::Align2::CENTER_CENTER,
        "On",
        font,
        color,
    );
    separator(ui, rect);
}

/// The line under a table row, across the card.
fn separator(ui: &egui::Ui, row: egui::Rect) {
    let palette = theme::palette(ui.ctx());
    ui.painter().hline(
        row.x_range().expand(space::XL),
        row.bottom(),
        Stroke::new(1., palette.tone(Tone::RAISED)),
    );
}

/// One extension's row: its name and source over its description, its
/// version and any update, its state, its switch and a chevron to its page.
/// The row opens the page; the switch turns it on or off.
fn table_row(ui: &mut egui::Ui, row: &Row, last: bool) -> Option<RowClick> {
    let palette = theme::palette(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &row.name));
    let switch = Columns::of(rect).switch;
    let hovered = response.hovered() && !ui.rect_contains_pointer(switch);
    if hovered {
        ui.painter().rect_filled(
            rect.expand2(Vec2::new(space::M, -2.)),
            crate::app::ui::design::radius::CARD,
            palette.tone(Tone::PANEL),
        );
    }
    let columns = Columns::of(rect);
    let top = rect.center().y - 9.;
    let bottom = rect.center().y + 9.;
    let width = columns.name.span();

    // The badge, then the name in what is left.
    let badge = one_line(
        ui,
        row.badge.0,
        text::SMALL,
        if row.badge.1 {
            palette.warning()
        } else {
            palette.tone(Tone::SECONDARY)
        },
        width / 2.,
    );
    let name = one_line(
        ui,
        &row.name,
        text::BODY,
        palette.tone(Tone::STRONG),
        width - badge.size().x - 16.,
    );
    let name_rect =
        egui::Align2::LEFT_CENTER.anchor_size(egui::pos2(columns.name.min, top), name.size());
    let badge_rect = egui::Rect::from_center_size(
        egui::pos2(name_rect.right() + 8. + badge.size().x / 2. + 5., top),
        badge.size() + Vec2::new(10., 4.),
    );
    ui.painter()
        .galley(name_rect.min, name, Color32::PLACEHOLDER);
    ui.painter().rect_filled(
        badge_rect,
        crate::app::ui::design::radius::CONTROL,
        palette.tone(Tone::RAISED),
    );
    ui.painter().galley(
        badge_rect.center() - badge.size() / 2.,
        badge,
        Color32::PLACEHOLDER,
    );
    let description = one_line(
        ui,
        &row.description,
        text::LABEL,
        palette.tone(Tone::MUTED),
        width,
    );
    ui.painter().galley(
        egui::pos2(columns.name.min, bottom - description.size().y / 2.),
        description,
        Color32::PLACEHOLDER,
    );

    let version_y = if row.update.is_some() {
        top
    } else {
        rect.center().y
    };
    ui.painter().text(
        egui::pos2(columns.version, version_y),
        egui::Align2::LEFT_CENTER,
        &row.version,
        egui::FontId::proportional(text::BODY),
        palette.tone(Tone::SECONDARY),
    );
    if let Some(update) = &row.update {
        ui.painter().text(
            egui::pos2(columns.version, bottom),
            egui::Align2::LEFT_CENTER,
            format!("{update} available"),
            egui::FontId::proportional(text::SMALL),
            palette.accent(),
        );
    }

    ui.painter().circle_filled(
        egui::pos2(columns.status + 3.5, rect.center().y),
        3.5,
        row.status.color(&palette),
    );
    let status = ui.painter().text(
        egui::pos2(columns.status + 13., rect.center().y),
        egui::Align2::LEFT_CENTER,
        row.status.text(),
        egui::FontId::proportional(text::LABEL),
        palette.tone(Tone::SECONDARY),
    );
    ui.interact(
        status.expand(4.),
        response.id.with("status"),
        Sense::hover(),
    )
    .on_hover_text(&row.why);

    icons::paint_at(
        ui.painter(),
        Icon::ChevronRight,
        columns.chevron,
        12.,
        palette.tone(if hovered { Tone::SOFT } else { Tone::DISABLED }),
    );
    if !last {
        separator(ui, rect);
    }
    // Over the row, so a click on it is the switch's alone.
    let switched = crate::app::ui::widgets::switch_in(
        ui,
        columns.switch,
        response.id.with("switch"),
        row.enabled,
        &format!("Enable {}", row.name),
    )
    .clicked();
    if switched {
        Some(RowClick::Switch)
    } else if response.clicked() {
        Some(RowClick::Open)
    } else {
        None
    }
}

/// The tabs of an extension's page, with a line under them; the one
/// clicked, if another.
fn section_tabs(ui: &mut egui::Ui, sections: &[Section], current: Section) -> Option<Section> {
    let palette = theme::palette(ui.ctx());
    let mut chosen = None;
    let row = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::XL;
        for &section in sections {
            let selected = section == current;
            let galley = ui.painter().layout_no_wrap(
                section.title().to_string(),
                egui::FontId::proportional(text::BODY),
                palette.tone(if selected { Tone::STRONG } else { Tone::MUTED }),
            );
            let (rect, response) =
                ui.allocate_exact_size(galley.size() + Vec2::new(0., 14.), Sense::click());
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::SelectableLabel,
                    true,
                    selected,
                    section.title(),
                )
            });
            let at = egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.);
            if response.hovered() && !selected {
                ui.painter()
                    .galley_with_override_text_color(at, galley, palette.tone(Tone::TEXT));
            } else {
                ui.painter().galley(at, galley, Color32::PLACEHOLDER);
            }
            if selected {
                ui.painter().rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(rect.left(), rect.bottom() - 2.),
                        rect.right_bottom(),
                    ),
                    1.,
                    palette.accent(),
                );
            }
            if response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
                && !selected
            {
                chosen = Some(section);
            }
        }
    });
    let line = row.response.rect;
    ui.painter().hline(
        ui.max_rect().x_range(),
        line.bottom(),
        Stroke::new(1., palette.tone(Tone::RAISED)),
    );
    chosen
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
