//! The file operations' dialogs that come before a plan, and what a crash
//! left unfinished. Move to Folder… chooses a catalog folder; New Folder
//! Inside… names a folder and offers to move the selection into it. Neither
//! holds anything: each ends in a request planned and confirmed as any
//! other. When a catalog opens, recovery decides what a crash left
//! unfinished, holding the catalog as an operation does; what it can't
//! decide waits in "Unfinished file operations" for the user to put back,
//! keep as it is, or add (a folder made).
use super::{State, Verb};
use crate::app::ui::design::{self, Tone, button, inset, space, text};
use crate::app::{Editor, ui::widgets};
use crate::catalog::file_ops::{
    self, FileOp, ItemOutcome, ItemResult, OperationId, Outcome, Recovery, Resolution, Unfinished,
};
use crate::catalog::{CatalogLocation, FolderId, PhotoId, RootId};
use eframe::egui;
use std::path::{Path, PathBuf};

/// Move to Folder…, while the folder is chosen.
pub(super) struct Choosing {
    catalog: CatalogLocation,
    photos: Vec<PhotoId>,
    filter: String,
    chosen: Option<FolderId>,
}

/// New Folder Inside…, while the folder is named.
pub(super) struct Naming {
    catalog: CatalogLocation,
    root: RootId,
    /// The parent's logical path and name.
    parent: String,
    parent_name: String,
    /// The selection, offered to move into the new folder.
    photos: Vec<PhotoId>,
    move_them: bool,
    name: String,
    focused: bool,
}

/// A catalog folder as Move to Folder… lists it: its root's name, then its
/// folders.
fn folder_label(
    roots: &[(RootId, String, Option<String>)],
    root: RootId,
    relative: &str,
) -> String {
    let root_name = roots
        .iter()
        .find(|r| r.0 == root)
        .and_then(|r| Path::new(&r.1).file_name())
        .map_or_else(
            || "Folder".to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
    std::iter::once(root_name.as_str())
        .chain(relative.split('/').filter(|n| !n.is_empty()))
        .collect::<Vec<_>>()
        .join(" / ")
}

/// What the user decided in "Unfinished file operations".
enum Decided {
    Later,
    Resolve(OperationId, Resolution),
    Reveal(PathBuf),
}

impl Editor {
    /// Move to Folder… for `photos`: asks where to.
    pub(in crate::app) fn choose_folder_for(&mut self, photos: Vec<PhotoId>) {
        let Some(catalog) = self.idle_catalog() else {
            return;
        };
        if photos.is_empty() {
            return;
        }
        self.file_ops.state = State::Choosing(Box::new(Choosing {
            catalog,
            photos,
            filter: String::new(),
            chosen: None,
        }));
    }

    /// New Folder Inside… logical folder `parent` of `root`, named
    /// `parent_name`: asks its name, offering to move `photos` (the
    /// selection) into it, ticked when they are `inside` that folder.
    pub(in crate::app) fn name_new_folder(
        &mut self,
        root: RootId,
        parent: String,
        parent_name: String,
        photos: Vec<PhotoId>,
        inside: bool,
    ) {
        let Some(catalog) = self.idle_catalog() else {
            return;
        };
        if let Some(library) = &mut self.library {
            library.new_folder_in(root, &parent);
        }
        self.file_ops.state = State::Naming(Box::new(Naming {
            catalog,
            root,
            parent,
            parent_name,
            photos,
            move_them: inside,
            name: String::new(),
            focused: false,
        }));
    }

    /// The catalog shown, when no file operation or other work is under
    /// way; says so when there is.
    fn idle_catalog(&mut self) -> Option<CatalogLocation> {
        if self.activity.is_busy() || !matches!(self.file_ops.state, State::Idle) {
            self.messages.warn("Finish what is running first");
            return None;
        }
        self.library
            .as_ref()
            .map(|l| l.session.catalog().location().clone())
    }

    /// Move to Folder…'s window: the catalog's folders, filtered by name.
    pub(super) fn choose_folder_window(&mut self, ctx: &egui::Context) {
        let (State::Choosing(c), Some(library)) = (&mut self.file_ops.state, &self.library) else {
            self.file_ops.state = State::Idle;
            return;
        };
        if library.session.catalog().location() != &c.catalog {
            self.file_ops.state = State::Idle;
            return;
        }
        let palette = crate::app::ui::theme::palette(ctx);
        let roots = library.session.roots();
        let mut rows: Vec<(FolderId, String)> = library
            .session
            .folders()
            .iter()
            .map(|f| (f.id, folder_label(roots, f.root, &f.relative)))
            .collect();
        rows.sort_by_key(|(_, label)| label.to_lowercase());
        let filter = c.filter.to_lowercase();
        rows.retain(|(_, label)| label.to_lowercase().contains(&filter));
        let mut go = None;
        let mut cancel = false;
        let response = egui::Modal::new(egui::Id::new("move-to-folder"))
            .frame(widgets::modal_frame(&palette).inner_margin(design::margin_same(inset::DIALOG)))
            .show(ctx, |ui| {
                ui.set_width(460.);
                ui.label(
                    egui::RichText::new(format!(
                        "Move {} to Folder",
                        widgets::plural(c.photos.len(), "Photo", "Photos")
                    ))
                    .size(text::TITLE)
                    .color(palette.tone(Tone::STRONG)),
                );
                ui.add_space(space::M);
                ui.add(
                    egui::TextEdit::singleline(&mut c.filter)
                        .hint_text("Filter folders")
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(space::S);
                egui::ScrollArea::vertical()
                    .max_height(320.)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (id, label) in &rows {
                            let row = ui.selectable_label(c.chosen == Some(*id), label);
                            if row.clicked() {
                                c.chosen = Some(*id);
                            }
                            if row.double_clicked() {
                                go = Some((*id, label.clone()));
                            }
                        }
                        if rows.is_empty() {
                            ui.label(
                                egui::RichText::new("No folder of the catalog is named so.")
                                    .size(text::LABEL)
                                    .color(palette.tone(Tone::MUTED)),
                            );
                        }
                    });
                ui.add_space(space::XL);
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding = button::DIALOG;
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    let chosen = c
                        .chosen
                        .and_then(|id| rows.iter().find(|(r, _)| *r == id).cloned());
                    let clicked = ui
                        .add_enabled_ui(chosen.is_some(), |ui| widgets::primary_button(ui, "Move…"))
                        .inner
                        .clicked();
                    if clicked {
                        go = chosen;
                    }
                });
            });
        if cancel || response.should_close() {
            self.file_ops.state = State::Idle;
            return;
        }
        if let Some((to, label)) = go {
            let State::Choosing(c) = std::mem::take(&mut self.file_ops.state) else {
                return;
            };
            let name = label.rsplit(" / ").next().unwrap_or(&label).to_string();
            let place = self.library.as_ref().and_then(|l| l.folder_place(to));
            if let Some((root, to)) = place {
                self.request_photo_move(c.photos, root, to, name);
            }
        }
    }

    /// New Folder Inside…'s window: the name, checked as it is typed, and
    /// whether the selection moves into it.
    pub(super) fn new_folder_window(&mut self, ctx: &egui::Context) {
        let State::Naming(n) = &mut self.file_ops.state else {
            return;
        };
        let palette = crate::app::ui::theme::palette(ctx);
        let problem = (!n.name.is_empty())
            .then(|| file_ops::folder_name_problem(&n.name))
            .flatten();
        let valid = !n.name.is_empty() && problem.is_none();
        let mut create = false;
        let mut cancel = false;
        let response = egui::Modal::new(egui::Id::new("new-folder-inside"))
            .frame(widgets::modal_frame(&palette).inner_margin(design::margin_same(inset::DIALOG)))
            .show(ctx, |ui| {
                ui.set_width(420.);
                ui.label(
                    egui::RichText::new(format!("New Folder Inside “{}”", n.parent_name))
                        .size(text::TITLE)
                        .color(palette.tone(Tone::STRONG)),
                );
                ui.add_space(space::L);
                let field = ui.add(
                    egui::TextEdit::singleline(&mut n.name)
                        .hint_text("Folder name")
                        .desired_width(f32::INFINITY),
                );
                if !n.focused {
                    field.request_focus();
                    n.focused = true;
                }
                if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && valid {
                    create = true;
                }
                if let Some(problem) = problem {
                    ui.add_space(space::XS);
                    ui.label(
                        egui::RichText::new(problem)
                            .size(text::LABEL)
                            .color(palette.warning()),
                    );
                }
                if !n.photos.is_empty() {
                    ui.add_space(space::M);
                    ui.checkbox(
                        &mut n.move_them,
                        format!(
                            "Move the {} into it",
                            widgets::plural(n.photos.len(), "selected photo", "selected photos")
                        ),
                    );
                }
                ui.add_space(space::XS);
                ui.label(
                    egui::RichText::new("The folder is made on disk, and added to the catalog.")
                        .size(text::LABEL)
                        .color(palette.tone(Tone::MUTED)),
                );
                ui.add_space(space::XL);
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding = button::DIALOG;
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    let label = if n.move_them && !n.photos.is_empty() {
                        "Create and Move…"
                    } else {
                        "Create"
                    };
                    if ui
                        .add_enabled_ui(valid, |ui| widgets::primary_button(ui, label))
                        .inner
                        .clicked()
                    {
                        create = true;
                    }
                });
            });
        if cancel || response.should_close() {
            self.file_ops.state = State::Idle;
            return;
        }
        if create {
            let State::Naming(n) = std::mem::take(&mut self.file_ops.state) else {
                return;
            };
            if !self.showing_catalog(&n.catalog) {
                return;
            }
            let moving = n.move_them && !n.photos.is_empty();
            let verb = Verb::Create {
                name: n.name.clone(),
                parent: n.parent_name,
                moving,
            };
            let op = FileOp::CreateFolder {
                root: n.root,
                parent: n.parent,
                name: n.name,
                then_move: if moving { n.photos } else { Vec::new() },
            };
            self.request_file_op(op, None, verb);
        }
    }

    /// Recovers catalog `catalog`'s unfinished file operations once nothing
    /// else runs: it was just opened.
    pub(in crate::app) fn recover_files_later(&mut self, catalog: CatalogLocation) {
        self.file_ops.pending_recovery = Some(catalog);
    }

    /// Starts the recovery waiting, when nothing else runs: the catalog is
    /// held as an operation holds it, and recovery runs on a worker.
    /// Another window changing files (or a catalog that can't take file
    /// operations) leaves it for later, silently.
    pub(in crate::app) fn poll_file_recovery(&mut self) {
        if self.file_ops.pending_recovery.is_none()
            || self.activity.is_busy()
            || !matches!(self.file_ops.state, State::Idle)
        {
            return;
        }
        let Some(catalog) = self.file_ops.pending_recovery.take() else {
            return;
        };
        if !self.showing_catalog(&catalog) {
            return;
        }
        // Most catalogs have nothing a crash left: then nothing is held.
        let unfinished = self
            .library
            .as_ref()
            .map(|l| file_ops::unfinished_here(l.session.catalog()));
        match unfinished {
            Some(Ok(true)) => {}
            Some(Err(e)) => {
                self.messages.fail(format!(
                    "Unfinished file operations could not be checked: {e:#}"
                ));
                return;
            }
            Some(Ok(false)) | None => return,
        }
        // Nothing is busy, so a save that fails won't go through next
        // frame either: given up once, not tried every frame.
        if !self.flush() {
            self.messages.fail("Unfinished file operations weren't checked: the open photo couldn't be saved. Open the catalog again to check them.");
            return;
        }
        if !self.activity.begin_file_change() {
            self.file_ops.pending_recovery = Some(catalog);
            return;
        }
        if let Some(library) = &mut self.library {
            library.suspend_background_reads();
        }
        self.file_ops.state = State::Recovering {
            catalog: catalog.clone(),
        };
        let failed = catalog.clone();
        let trash = self.file_ops.trash.clone();
        crate::app::task::spawn(
            self.tx.clone(),
            self.context.clone(),
            move |tx| {
                let recovered = crate::catalog::Catalog::open(&catalog)
                    .map_err(|e| format!("{e:#}"))
                    .and_then(|mut opened| match file_ops::recover(&mut opened, &*trash) {
                        Ok(recovery) => Ok(Some(recovery)),
                        // Another window holds it, or file operations
                        // can't run here: nothing to decide here now.
                        Err(
                            file_ops::FileOpError::Busy | file_ops::FileOpError::Unsupported(_),
                        ) => Ok(None),
                        Err(e) => Err(e.to_string()),
                    });
                let _ = tx.send(crate::app::worker::Event::FileOpsRecovered(
                    catalog,
                    Box::new(recovered),
                ));
            },
            move |tx, message| {
                let _ = tx.send(crate::app::worker::Event::FileOpsRecovered(
                    failed,
                    Box::new(Err(message)),
                ));
            },
        );
    }

    /// Recovery finished: what it decided is applied, the catalog let go,
    /// and what it couldn't decide is asked about.
    pub(in crate::app) fn file_ops_recovered(
        &mut self,
        catalog: CatalogLocation,
        recovered: Result<Option<Recovery>, String>,
    ) {
        if !matches!(&self.file_ops.state, State::Recovering { catalog: c } if *c == catalog) {
            return;
        }
        self.file_ops.state = State::Idle;
        if !self.showing_catalog(&catalog) {
            self.release_files();
            return;
        }
        match recovered {
            Ok(Some(recovery)) => {
                let finished = recovery
                    .finished
                    .iter()
                    .filter(|i| !matches!(i.result, ItemResult::Unchanged(_)))
                    .count();
                if finished > 0 {
                    self.messages.done(format!(
                        "Finished {} a crash left",
                        widgets::plural(finished, "file operation", "file operations")
                    ));
                }
                self.apply_file_outcome(&Outcome {
                    items: recovery.finished,
                    stopped: None,
                });
                self.release_files();
                if !recovery.unfinished.is_empty() {
                    self.file_ops.state = State::Unfinished {
                        catalog,
                        items: recovery.unfinished,
                    };
                }
            }
            Ok(None) => self.release_files(),
            Err(e) => {
                self.messages.fail(format!(
                    "Unfinished file operations could not be checked: {e}"
                ));
                self.release_files();
            }
        }
    }

    /// "Unfinished file operations": each item, why, its files, and what
    /// may be decided for it.
    pub(super) fn unfinished_window(&mut self, ctx: &egui::Context) {
        let State::Unfinished { catalog, items } = &self.file_ops.state else {
            return;
        };
        if !self.showing_catalog(catalog) {
            self.file_ops.state = State::Idle;
            return;
        }
        let palette = crate::app::ui::theme::palette(ctx);
        let library = self.library.as_deref();
        let name_of = |item: &Unfinished| -> String {
            match item.photos.first() {
                Some(id) => {
                    let name = library
                        .and_then(|l| l.photo(*id))
                        .map_or_else(|| format!("Photo {}", id.0), |p| p.filename.clone());
                    let copies = item.photos.len() - 1;
                    if copies > 0 {
                        format!(
                            "{name} (with {})",
                            widgets::plural(copies, "virtual copy", "virtual copies")
                        )
                    } else {
                        name
                    }
                }
                None => format!(
                    "Folder “{}”",
                    item.members
                        .first()
                        .and_then(|m| m.source.file_name())
                        .unwrap_or_default()
                        .to_string_lossy()
                ),
            }
        };
        let mut decided = None;
        let response = egui::Modal::new(egui::Id::new("unfinished-file-operations"))
            .frame(widgets::modal_frame(&palette).inner_margin(design::margin_same(inset::DIALOG)))
            .show(ctx, |ui| {
                ui.set_width(560.);
                ui.label(
                    egui::RichText::new("Unfinished file operations")
                        .size(text::TITLE)
                        .color(palette.tone(Tone::STRONG)),
                );
                ui.add_space(space::S);
                ui.label(
                    egui::RichText::new(
                        "RAWmakase stopped while it was changing these files. Their photos stay as they are in the catalog until you decide.",
                    )
                    .size(text::LABEL)
                    .color(palette.tone(Tone::SECONDARY)),
                );
                ui.add_space(space::L);
                egui::ScrollArea::vertical()
                    .max_height(380.)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for item in items {
                            ui.label(
                                egui::RichText::new(name_of(item))
                                    .size(text::BODY)
                                    .color(palette.tone(Tone::TEXT)),
                            );
                            if !item.reason.is_empty() {
                                ui.label(
                                    egui::RichText::new(&item.reason)
                                        .size(text::LABEL)
                                        .color(palette.warning()),
                                );
                            }
                            // A folder's every file is a member: the first
                            // few say where it is.
                            for m in item.members.iter().take(MEMBERS_SHOWN) {
                                let text = match (&m.after, m.changed) {
                                    (Some(after), true) => format!(
                                        "{} → {}",
                                        m.source.display(),
                                        after.display()
                                    ),
                                    _ => m.source.display().to_string(),
                                };
                                ui.label(
                                    egui::RichText::new(text).size(text::SMALL).color(palette.tone(Tone::MUTED)),
                                );
                            }
                            if item.members.len() > MEMBERS_SHOWN {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "and {} more",
                                        item.members.len() - MEMBERS_SHOWN
                                    ))
                                    .size(text::SMALL)
                                    .color(palette.tone(Tone::MUTED)),
                                );
                            }
                            ui.add_space(space::XS);
                            ui.horizontal(|ui| {
                                for choice in &item.choices {
                                    let label = match choice {
                                        Resolution::PutBack => "Put Back",
                                        Resolution::Keep => "Keep as Is",
                                        Resolution::AddFolder => "Add to Catalog",
                                        Resolution::RemoveFromCatalog => "Remove from Catalog",
                                    };
                                    if ui.button(label).clicked() {
                                        decided = Some(Decided::Resolve(item.operation, *choice));
                                    }
                                }
                                if ui.button(crate::platform::reveal::LABEL).clicked() {
                                    decided = Some(Decided::Reveal(shown_path(item)));
                                }
                            });
                            ui.add_space(space::L);
                        }
                    });
                ui.add_space(space::M);
                if ui.button("Decide Later").clicked() {
                    decided = Some(Decided::Later);
                }
            });
        if response.should_close() && decided.is_none() {
            decided = Some(Decided::Later);
        }
        match decided {
            None => {}
            // Asked again when the catalog next opens.
            Some(Decided::Later) => self.file_ops.state = State::Idle,
            Some(Decided::Reveal(path)) => {
                if let Err(e) = crate::platform::reveal::reveal(&path) {
                    self.messages
                        .fail(format!("Could not show {}: {e}", path.display()));
                }
            }
            Some(Decided::Resolve(operation, resolution)) => {
                self.resolve_unfinished(operation, resolution)
            }
        }
    }

    /// Carries out the user's choice for unfinished item `operation` on a
    /// worker, holding the catalog as an operation does.
    pub(super) fn resolve_unfinished(&mut self, operation: OperationId, resolution: Resolution) {
        let State::Unfinished { catalog, items } = std::mem::take(&mut self.file_ops.state) else {
            return;
        };
        if self.activity.is_busy() || !self.flush() || !self.activity.begin_file_change() {
            self.messages.warn("Finish what is running first");
            self.file_ops.state = State::Unfinished { catalog, items };
            return;
        }
        if let Some(library) = &mut self.library {
            library.suspend_background_reads();
        }
        self.file_ops.state = State::Resolving {
            catalog: catalog.clone(),
            operation,
            items,
        };
        let failed = catalog.clone();
        let trash = self.file_ops.trash.clone();
        crate::app::task::spawn(
            self.tx.clone(),
            self.context.clone(),
            move |tx| {
                let resolved = crate::catalog::Catalog::open(&catalog)
                    .map_err(|e| format!("{e:#}"))
                    .and_then(|mut opened| {
                        file_ops::resolve(&mut opened, operation, resolution, &*trash)
                            .map_err(|e| e.to_string())
                    });
                let _ = tx.send(crate::app::worker::Event::FileOpResolved(
                    catalog,
                    operation,
                    Box::new(resolved),
                ));
            },
            move |tx, message| {
                let _ = tx.send(crate::app::worker::Event::FileOpResolved(
                    failed,
                    operation,
                    Box::new(Err(message)),
                ));
            },
        );
    }

    /// The user's choice was carried out: applied, the catalog let go, and
    /// what is still unfinished shown again.
    pub(in crate::app) fn file_op_resolved(
        &mut self,
        catalog: CatalogLocation,
        operation: OperationId,
        resolved: Result<ItemOutcome, String>,
    ) {
        let State::Resolving {
            catalog: running,
            operation: resolving,
            mut items,
        } = std::mem::take(&mut self.file_ops.state)
        else {
            return;
        };
        if running != catalog || resolving != operation {
            self.file_ops.state = State::Resolving {
                catalog: running,
                operation: resolving,
                items,
            };
            return;
        }
        if !self.showing_catalog(&catalog) {
            self.release_files();
            return;
        }
        match resolved {
            Ok(outcome) => {
                match &outcome.result {
                    ItemResult::Unchanged(_) => self.messages.done("Put back where it was"),
                    ItemResult::Kept => self.messages.done("Kept as it is"),
                    ItemResult::Created { .. } => {
                        self.messages.done("Added the folder to the catalog")
                    }
                    ItemResult::Removed => self.messages.done("Removed from the catalog"),
                    ItemResult::NeedsRecovery(why) => {
                        self.messages.fail(format!("Still unfinished: {why}"))
                    }
                    _ => self.messages.done("Decided"),
                }
                match &outcome.result {
                    ItemResult::NeedsRecovery(why) => {
                        for u in items.iter_mut().filter(|u| u.operation == operation) {
                            u.reason = why.clone();
                        }
                    }
                    _ => items.retain(|u| u.operation != operation),
                }
                self.apply_file_outcome(&Outcome {
                    items: vec![outcome],
                    stopped: None,
                });
            }
            Err(e) => self.messages.fail(format!("Not decided: {e}")),
        }
        self.release_files();
        if !items.is_empty() {
            self.file_ops.state = State::Unfinished { catalog, items };
        }
    }
}

/// How many of an unfinished item's files are listed.
const MEMBERS_SHOWN: usize = 8;

/// Where Show in Finder looks for an unfinished item: where its first file
/// is now, if it is anywhere it was, else its folder.
fn shown_path(item: &Unfinished) -> PathBuf {
    let Some(m) = item.members.first() else {
        return PathBuf::new();
    };
    [m.after.as_deref(), Some(m.source.as_path())]
        .into_iter()
        .flatten()
        .find(|p| p.exists())
        .or_else(|| m.source.parent())
        .map(Path::to_path_buf)
        .unwrap_or_default()
}
