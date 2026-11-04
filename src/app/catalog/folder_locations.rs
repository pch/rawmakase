//! Folder locations on this computer (see `catalog::locations`): their list
//! in Preferences › Catalog, and the questions changing a root or adding a
//! folder can raise.
use crate::app::Editor;
use crate::app::preferences::{gap, group, hint};
use crate::app::shell::dialogs::{CatalogDialog, FolderAction};
use crate::app::ui::design::{Tone, inset, radius, space, text};
use crate::app::ui::theme;
use crate::app::ui::widgets::{confirm_modal, form_row, pretty_path};
use crate::app::worker::Event;
use crate::catalog::{
    Ambiguity, Catalog, CatalogLocation, Choice, Conflict, Override, Overrides, RootId,
    RootLocations,
};
use eframe::egui;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// A question only the user can answer before a folder change is made.
pub(crate) enum FolderQuestion {
    /// A root, or its folder at logical path `relative`, moves while folders
    /// below it have their own locations here.
    Overrides {
        catalog: CatalogLocation,
        root: RootId,
        relative: String,
        path: PathBuf,
        overrides: Vec<Override>,
    },
    /// Folders found on disk that equally close locations claim; asked one
    /// at a time, then added with every choice.
    Ambiguous {
        catalog: CatalogLocation,
        folder: PathBuf,
        open: Vec<Ambiguity>,
        chosen: Vec<Choice>,
    },
}
/// What a folder change does once settled.
enum FolderJob {
    Relink {
        root: RootId,
        relative: String,
        path: PathBuf,
        overrides: Overrides,
    },
    Import {
        folder: PathBuf,
        choices: Vec<Choice>,
    },
    Clear {
        root: RootId,
        relative: String,
    },
}

/// The Folder locations list, read when the Catalog page shows.
#[derive(Default)]
pub(in crate::app) struct LocationsView {
    roots: Vec<RootLocations>,
    /// This computer's name, as typed.
    computer: String,
    /// Typed and not saved yet: saved when the field or the page is left.
    computer_dirty: bool,
    /// Whether each path shown is there, checked off the UI thread: a
    /// missing network share can take seconds to answer.
    found: Arc<Mutex<HashMap<PathBuf, bool>>>,
    error: Option<String>,
}
impl LocationsView {
    fn load(catalog: &Catalog, ctx: &egui::Context) -> Self {
        let roots = match catalog.folder_locations() {
            Ok(roots) => roots,
            Err(e) => {
                return Self {
                    error: Some(format!("{e:#}")),
                    ..Default::default()
                };
            }
        };
        let found: Arc<Mutex<HashMap<PathBuf, bool>>> = Default::default();
        let paths: Vec<PathBuf> = roots
            .iter()
            .flat_map(|r| {
                std::iter::once(r.path.clone()).chain(r.overrides.iter().map(|o| o.path.clone()))
            })
            .collect();
        let shared = found.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            for path in paths {
                let there = path.is_dir();
                shared.lock().unwrap().insert(path, there);
                ctx.request_repaint();
            }
        });
        Self {
            roots,
            computer: catalog.computer_name().unwrap_or_default(),
            computer_dirty: false,
            found,
            error: None,
        }
    }
}

impl Editor {
    /// The Folder locations block of Preferences › Catalog.
    pub(in crate::app) fn folder_locations_block(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(ui.ctx());
        let ctx = ui.ctx().clone();
        let Some(library) = &self.library else {
            return;
        };
        let view = self
            .preferences
            .locations
            .get_or_insert_with(|| LocationsView::load(library.session.catalog(), &ctx));
        group(ui, "Folder locations");
        if let Some(error) = &view.error {
            form_row(ui, "", |ui| hint(ui, error));
            return;
        }
        let mut renamed = false;
        form_row(ui, "This computer", |ui| {
            let field = ui.add(egui::TextEdit::singleline(&mut view.computer).desired_width(220.));
            view.computer_dirty |= field.changed();
            renamed = field.lost_focus();
        });
        form_row(ui, "", |ui| {
            hint(
                ui,
                "Each computer keeps its own location for the catalog's folders. \
                 Changing one here never moves them on another computer.",
            );
        });
        let found = view.found.lock().unwrap().clone();
        let status = |path: &Path| match found.get(path) {
            Some(true) => ("Found", palette.tone(Tone::SECONDARY)),
            Some(false) => ("Offline", egui::Color32::from_rgb(222, 150, 90)),
            None => ("Checking…", palette.tone(Tone::FAINT)),
        };
        enum Click {
            Change(FolderAction),
            Clear(RootId, String),
        }
        let mut click = None;
        for root in &view.roots {
            let name = Path::new(&root.original)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| root.original.clone());
            ui.add_space(space::S);
            egui::Frame::new()
                .fill(palette.tone(Tone::PANEL))
                .corner_radius(radius::CARD)
                .inner_margin(inset::CARD)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let note = if root.location.is_some() {
                        "Located on this computer".to_string()
                    } else {
                        "Where it was added; not located on this computer".to_string()
                    };
                    match entry(
                        ui,
                        &name,
                        &root.path,
                        &note,
                        status(&root.path),
                        root.location.is_some(),
                    ) {
                        Some(Button::Change) => {
                            click = Some(Click::Change(FolderAction::RelinkRoot(root.root)))
                        }
                        Some(Button::Clear) => click = Some(Click::Clear(root.root, String::new())),
                        None => {}
                    }
                    for over in &root.overrides {
                        ui.add_space(space::S);
                        ui.horizontal(|ui| {
                            ui.add_space(space::XL);
                            ui.vertical(|ui| {
                                let note = format!("Folder of {name}, located separately");
                                match entry(
                                    ui,
                                    &over.relative,
                                    &over.path,
                                    &note,
                                    status(&over.path),
                                    true,
                                ) {
                                    Some(Button::Change) => {
                                        click = Some(Click::Change(FolderAction::RelinkFolder(
                                            root.root,
                                            over.relative.clone(),
                                        )))
                                    }
                                    Some(Button::Clear) => {
                                        click = Some(Click::Clear(root.root, over.relative.clone()))
                                    }
                                    None => {}
                                }
                            });
                        });
                    }
                    for (computer, relative, path) in &root.elsewhere {
                        let what = if relative.is_empty() {
                            String::new()
                        } else {
                            format!("{relative} at ")
                        };
                        ui.add_space(space::XS);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!(
                                    "On {computer}: {what}{}",
                                    pretty_path(path)
                                ))
                                .size(text::SMALL)
                                .color(palette.tone(Tone::FAINT)),
                            )
                            .truncate(),
                        );
                    }
                });
        }
        if view.roots.is_empty() {
            form_row(ui, "", |ui| hint(ui, "No folders yet."));
        }
        gap(ui);
        if renamed {
            self.save_computer_name();
        }
        match click {
            Some(Click::Change(action)) => self.catalog_dialog(CatalogDialog::Folder(action), &ctx),
            Some(Click::Clear(root, relative)) => self.clear_folder_location(root, &relative),
            None => {}
        }
    }
    /// Saves this computer's name still being typed, once its field or
    /// page is left.
    pub(in crate::app) fn save_computer_name(&mut self) {
        let (Some(view), Some(library)) = (&mut self.preferences.locations, &mut self.library)
        else {
            return;
        };
        if !std::mem::take(&mut view.computer_dirty) {
            return;
        }
        if let Err(e) = library.session.rename_computer(&view.computer) {
            self.messages.fail(format!("Computer not renamed: {e:#}"));
        }
    }
    /// Clears a location, then opens the catalog again as relinking does,
    /// so a photo open in Develop follows its new path.
    fn clear_folder_location(&mut self, root: RootId, relative: &str) {
        if !self.ready_for_catalog() {
            return;
        }
        let Some(catalog) = self
            .library
            .as_ref()
            .map(|l| l.session.catalog().location().clone())
        else {
            return;
        };
        let ctx = self.context.clone();
        self.folder_job(
            catalog,
            FolderJob::Clear {
                root,
                relative: relative.into(),
            },
            &ctx,
        );
    }
    /// Asks the pending folder question, then carries out the change.
    pub(in crate::app) fn folder_question_window(&mut self, ctx: &egui::Context) {
        let Some(crate::app::Modal::FolderQuestion(question)) = &mut self.modal else {
            return;
        };
        match question {
            FolderQuestion::Overrides {
                catalog,
                root,
                relative,
                path,
                overrides,
            } => {
                // A few, so the buttons stay on screen.
                let mut list: Vec<String> = overrides
                    .iter()
                    .take(6)
                    .map(|o| format!("{}: {}", o.relative, pretty_path(&o.path)))
                    .collect();
                if overrides.len() > 6 {
                    list.push(format!("and {} more", overrides.len() - 6));
                }
                let detail = format!(
                    "These folders have their own location on this computer:\n\n{}\n\n\
                     Keep them there, or clear them so they are found in the new location.",
                    list.join("\n")
                );
                let Some(choice) = confirm_modal(
                    ctx,
                    "folder-overrides",
                    "Keep the folders located separately?",
                    &detail,
                    false,
                    &[
                        ("Cancel", None),
                        ("Clear Overrides", Some(Overrides::Clear)),
                        ("Keep Overrides", Some(Overrides::Keep)),
                    ],
                    None,
                ) else {
                    return;
                };
                let (catalog, root, relative, path) =
                    (catalog.clone(), *root, relative.clone(), path.clone());
                self.modal = None;
                if let Some(overrides) = choice {
                    self.folder_job(
                        catalog,
                        FolderJob::Relink {
                            root,
                            relative,
                            path,
                            overrides,
                        },
                        ctx,
                    );
                }
            }
            FolderQuestion::Ambiguous {
                catalog,
                folder,
                open,
                chosen,
            } => {
                let Some(ambiguity) = open.first() else {
                    let (catalog, folder, choices) =
                        (catalog.clone(), folder.clone(), std::mem::take(chosen));
                    self.modal = None;
                    self.folder_job(catalog, FolderJob::Import { folder, choices }, ctx);
                    return;
                };
                let shown: Vec<String> = ambiguity
                    .directories
                    .iter()
                    .take(5)
                    .map(|d| pretty_path(d))
                    .collect();
                let detail = format!(
                    "{}{}\n\nmatches more than one folder of the catalog on this computer. \
                     Choose the one these photos belong to.",
                    shown.join("\n"),
                    if ambiguity.directories.len() > 5 {
                        format!("\nand {} more", ambiguity.directories.len() - 5)
                    } else {
                        String::new()
                    }
                );
                let labels: Vec<String> = ambiguity
                    .options
                    .iter()
                    .map(|o| {
                        if o.relative.is_empty() {
                            format!("Root {} ({})", o.root, pretty_path(&o.path))
                        } else {
                            format!("{} in root {}", o.relative, o.root)
                        }
                    })
                    .collect();
                let mut buttons: Vec<(&str, Option<usize>)> = vec![("Cancel", None)];
                buttons.extend(
                    labels
                        .iter()
                        .enumerate()
                        .map(|(i, l)| (l.as_str(), Some(i))),
                );
                let Some(choice) = confirm_modal(
                    ctx,
                    "folder-ambiguous",
                    "Which folder of the catalog is this?",
                    &detail,
                    false,
                    &buttons,
                    None,
                ) else {
                    return;
                };
                match choice {
                    Some(i) => {
                        let answered = open.remove(0);
                        chosen.push(Choice {
                            chosen: answered.options[i].clone(),
                            options: answered.options,
                        });
                    }
                    None => self.modal = None,
                }
            }
        }
    }
    /// Carries out a settled folder change and opens the catalog again.
    fn folder_job(&mut self, catalog: CatalogLocation, job: FolderJob, ctx: &egui::Context) {
        if !self.activity.begin_folder_change() {
            return;
        }
        // A panic ends the folder change as a failure does, so closing never waits
        // for it.
        let opened = ctx.clone();
        crate::app::task::spawn(
            self.tx.clone(),
            ctx.clone(),
            move |tx| {
                let event = match change_folders(&catalog, job) {
                    Ok((relinked, report, conflicts)) => {
                        reopened(&catalog, relinked, &report, &conflicts, &opened)
                    }
                    Err(e) => Event::CatalogReady(Err(format!("{e:#}"))),
                };
                let _ = tx.send(event);
            },
            |tx, message| {
                let _ = tx.send(Event::CatalogReady(Err(message)));
            },
        );
    }
}
/// Makes a settled folder change in the catalog at `catalog`: whether it relinked a
/// root, and what adding a folder found.
fn change_folders(
    catalog: &CatalogLocation,
    job: FolderJob,
) -> anyhow::Result<(bool, crate::catalog::SidecarReport, Vec<Conflict>)> {
    let mut cat = Catalog::open(catalog)?;
    Ok(match job {
        FolderJob::Relink {
            root,
            relative,
            path,
            overrides,
        } => {
            if relative.is_empty() {
                cat.relink_root_with(root, &path, overrides)?;
            } else {
                cat.relink_folder_at(root, &relative, &path, overrides)?;
            }
            (true, Default::default(), Vec::new())
        }
        FolderJob::Clear { root, relative } => {
            cat.clear_folder_location(root, &relative)?;
            (true, Default::default(), Vec::new())
        }
        FolderJob::Import { folder, choices } => {
            let added = cat.import_folder(
                &folder,
                &crate::catalog_session::new_photo_defaults(),
                &choices,
            )?;
            anyhow::ensure!(
                added.ambiguous.is_empty(),
                "The folders changed meanwhile; add the folder again"
            );
            (false, added.report, added.conflicts)
        }
    })
}
/// A button of a Folder locations entry.
enum Button {
    Change,
    Clear,
}
/// One root or folder of Folder locations: its name, whether it is found
/// and its buttons on the first line, then where it is and why.
fn entry(
    ui: &mut egui::Ui,
    name: &str,
    path: &Path,
    note: &str,
    status: (&str, egui::Color32),
    clearable: bool,
) -> Option<Button> {
    let palette = theme::palette(ui.ctx());
    let mut clicked = None;
    // Buttons and status first, from the right, so a long name can't push
    // them out of the window; the name takes what is left.
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if clearable
            && ui
                .button("Clear")
                .on_hover_text("Forget this location on this computer")
                .clicked()
        {
            clicked = Some(Button::Clear);
        }
        if ui.button("Change…").clicked() {
            clicked = Some(Button::Change);
        }
        ui.add_space(space::S);
        ui.label(
            egui::RichText::new(status.0)
                .size(text::SMALL)
                .color(status.1),
        );
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(name)
                        .size(text::BODY)
                        .strong()
                        .color(palette.tone(Tone::TEXT)),
                )
                .truncate(),
            );
        });
    });
    ui.add(
        egui::Label::new(
            egui::RichText::new(pretty_path(path))
                .size(text::LABEL)
                .color(palette.tone(Tone::SOFT)),
        )
        .truncate(),
    )
    .on_hover_text(path.display().to_string());
    ui.label(
        egui::RichText::new(note)
            .size(text::SMALL)
            .color(palette.tone(Tone::FAINT)),
    );
    clicked
}
/// The catalog at `location` opened again after a folder change, with what
/// the change found.
pub(in crate::app) fn reopened(
    location: &CatalogLocation,
    relinked: bool,
    report: &crate::catalog::SidecarReport,
    conflicts: &[Conflict],
    ctx: &egui::Context,
) -> Event {
    Event::CatalogReady(
        crate::catalog_session::CatalogSession::open(location)
            .map(|opened| crate::app::library::Library::new(opened, ctx.clone()))
            .map(|mut l| {
                if relinked {
                    l.wait_for_availability();
                    let available = l.available_count();
                    let said = format!(
                        "Folder location changed. {available} of {} photos are available.",
                        l.session.photos().len()
                    );
                    if available == 0 {
                        l.messages.warn(format!(
                            "{said} No files matched this location; check that the selected \
                             folder contains the expected subfolders."
                        ));
                    } else {
                        l.messages.done(said);
                    }
                }
                folder_added(&mut l, report, conflicts);
                Box::new(l)
            })
            .map_err(|e| format!("{e:#}")),
    )
}
/// Reports what adding a folder skipped, if anything, on the Library's
/// status line: sidecars that could not be read and folders found elsewhere.
pub(in crate::app) fn folder_added(
    library: &mut crate::app::library::Library,
    report: &crate::catalog::SidecarReport,
    conflicts: &[Conflict],
) {
    let mut summary: Vec<String> = report.summary().into_iter().collect();
    let mut detail = report.details();
    if let Some(first) = conflicts.first() {
        summary.push(if conflicts.len() == 1 {
            first.message()
        } else {
            format!(
                "{} folders not added: they are linked elsewhere on this computer",
                conflicts.len()
            )
        });
        for conflict in conflicts {
            if !detail.is_empty() {
                detail.push('\n');
            }
            detail.push_str(&conflict.message());
        }
    }
    if !summary.is_empty() {
        library
            .messages
            .warn_with_detail(format!("Folder added · {}", summary.join(" · ")), detail);
    }
}
