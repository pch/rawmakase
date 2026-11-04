//! Moving a folder (planned in #556, into another root in #572, a root
//! into another in #586): dragged onto another folder in the Folders
//! panel, or Move Folder… in its menu, which lists where it can go. Either
//! is confirmed as moving photos is, Cancel focused; the catalog's folder
//! stays the same folder, its photos follow.
use super::{State, Verb};
use crate::app::ui::design::{self, Tone, button, inset, space, text};
use crate::app::{Editor, ui::widgets};
use crate::catalog::file_ops::FileOp;
use crate::catalog::{CatalogLocation, RootId};
use eframe::egui;

/// Move Folder…, while where it goes is chosen. Holds nothing.
pub(super) struct ChoosingParent {
    catalog: CatalogLocation,
    root: RootId,
    /// Its logical path ("" for the root itself).
    from: String,
    /// Its name.
    name: String,
    filter: String,
    /// The root and logical path chosen ("" for the root itself).
    chosen: Option<(RootId, String)>,
}

/// The last name of logical path `path`.
fn last(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or_default()
}

/// Root `root`'s name, as the Folders panel shows it: its folder's.
fn root_name(roots: &[(RootId, String, Option<String>)], root: RootId) -> String {
    roots
        .iter()
        .find(|r| r.0 == root)
        .and_then(|r| std::path::Path::new(&r.1).file_name())
        .map_or_else(
            || "Folder".to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
}

/// Root `root`'s folder's name where it is on this computer (its location,
/// else where it was added): the name a root moved into another takes
/// there, as the plan gives it.
fn located_name(roots: &[(RootId, String, Option<String>)], root: RootId) -> String {
    roots
        .iter()
        .find(|r| r.0 == root)
        .and_then(|r| std::path::Path::new(r.2.as_deref().unwrap_or(&r.1)).file_name())
        .map_or_else(
            || "Folder".to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
}

/// Where Move Folder… may put `c`'s folder, as roots and logical paths
/// ("" for a root itself) with labels and where each is on this computer
/// (which tells apart two roots of one name): every root, and every folder
/// of each the Folders panel shows (those with no catalog folder of their
/// own too, such as `2025` above `2025/day2`), but where it is, itself and
/// the folders in it; for a root, every other root and its folders alone.
/// Whether another root is on its drive the plan checks.
fn parent_rows(
    library: &crate::app::library::Library,
    c: &ChoosingParent,
) -> Vec<((RootId, String), String, std::path::PathBuf)> {
    let parent = c.from.rsplit_once('/').map_or("", |(p, _)| p);
    let inside = format!("{}/", c.from);
    let mut rows = Vec::new();
    for (root, original, location) in library.session.roots() {
        let name = root_name(library.session.roots(), *root);
        let top = std::path::PathBuf::from(location.as_deref().unwrap_or(original));
        let mut logical: Vec<String> = vec![String::new()];
        for f in library.session.folders().iter().filter(|f| f.root == *root) {
            let mut at = String::new();
            for name in f.relative.split('/').filter(|n| !n.is_empty()) {
                at = join(&at, name);
                logical.push(at.clone());
            }
        }
        logical.sort();
        logical.dedup();
        // Where each is, as the Folders panel shows it: a catalog folder
        // where it resolves (located on its own, perhaps), any other below
        // the one above it. Sorted, a folder comes after the one above it.
        let mut paths: std::collections::HashMap<String, std::path::PathBuf> = library
            .session
            .folders()
            .iter()
            .filter(|f| f.root == *root)
            .map(|f| (f.relative.clone(), f.path.clone()))
            .collect();
        paths.entry(String::new()).or_insert(top);
        for l in &logical {
            if !paths.contains_key(l) {
                let (up, name) = l.rsplit_once('/').unwrap_or(("", l));
                let path = paths.get(up).map(|p| p.join(name)).unwrap_or_default();
                paths.insert(l.clone(), path);
            }
        }
        let own = *root == c.root;
        if own && c.from.is_empty() {
            continue;
        }
        rows.extend(
            logical
                .into_iter()
                .filter(|l| !own || (l != parent && *l != c.from && !l.starts_with(&inside)))
                .map(|l| {
                    let label = if l.is_empty() {
                        name.clone()
                    } else {
                        format!("{name} / {}", l.replace('/', " / "))
                    };
                    let path = paths.get(l.as_str()).cloned().unwrap_or_default();
                    ((*root, l), label, path)
                }),
        );
    }
    rows.sort_by_key(|(_, label, _)| label.to_lowercase());
    rows
}

/// Logical path `parent`, then `name`.
fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

impl Editor {
    /// What Move Folder… offers, as roots and logical paths.
    #[cfg(test)]
    pub(super) fn parent_choices(&self) -> Vec<(RootId, String)> {
        match (&self.file_ops.state, &self.library) {
            (State::ChoosingParent(c), Some(library)) => parent_rows(library, c)
                .into_iter()
                .map(|(at, ..)| at)
                .collect(),
            _ => Vec::new(),
        }
    }
    /// Where Move Folder… says its row for `root`'s logical path `at` is.
    #[cfg(test)]
    pub(super) fn parent_path(&self, root: RootId, at: &str) -> Option<std::path::PathBuf> {
        match (&self.file_ops.state, &self.library) {
            (State::ChoosingParent(c), Some(library)) => parent_rows(library, c)
                .into_iter()
                .find(|((r, l), ..)| *r == root && l == at)
                .map(|(.., path)| path),
            _ => None,
        }
    }
    /// Moves the folder of `root` at logical path `from` into logical
    /// folder `to` of `to_root` ("" for that root itself), once confirmed.
    pub(in crate::app) fn request_folder_move(
        &mut self,
        root: RootId,
        from: String,
        to_root: RootId,
        to: String,
    ) {
        let roots = self
            .library
            .as_ref()
            .map(|l| l.session.roots())
            .unwrap_or_default();
        let into = match last(&to) {
            "" => root_name(roots, to_root),
            name => name.to_string(),
        };
        // A root, named as its folder is here, joins the other root (#586).
        let (op, name, joins) = match from.as_str() {
            "" => (
                FileOp::MoveRoot {
                    root,
                    to_root,
                    parent: to.clone(),
                },
                located_name(roots, root),
                Some(root_name(roots, to_root)),
            ),
            from => (
                FileOp::MoveFolder {
                    root,
                    folder: from.to_string(),
                    to_root,
                    parent: to.clone(),
                },
                last(from).to_string(),
                None,
            ),
        };
        let verb = Verb::MoveFolder {
            root,
            to_root,
            to: join(&to, &name),
            from,
            name,
            into,
            joins,
        };
        self.request_file_op(op, None, verb);
    }

    /// Move Folder… for the folder of `root` at logical path `from`: asks
    /// where it goes.
    pub(in crate::app) fn choose_parent_for(&mut self, root: RootId, from: String) {
        if self.activity.is_busy() || !matches!(self.file_ops.state, State::Idle) {
            self.messages.warn("Finish what is running first");
            return;
        }
        let Some((catalog, name)) = self.library.as_ref().map(|l| {
            let name = match last(&from) {
                "" => located_name(l.session.roots(), root),
                name => name.to_string(),
            };
            (l.session.catalog().location().clone(), name)
        }) else {
            return;
        };
        self.file_ops.state = State::ChoosingParent(Box::new(ChoosingParent {
            catalog,
            root,
            name,
            from,
            filter: String::new(),
            chosen: None,
        }));
    }

    /// Move Folder…'s window: the roots and their folders it can go into,
    /// filtered by name.
    pub(super) fn choose_parent_window(&mut self, ctx: &egui::Context) {
        let (State::ChoosingParent(c), Some(library)) = (&mut self.file_ops.state, &self.library)
        else {
            self.file_ops.state = State::Idle;
            return;
        };
        if library.session.catalog().location() != &c.catalog {
            self.file_ops.state = State::Idle;
            return;
        }
        let palette = crate::app::ui::theme::palette(ctx);
        let mut rows = parent_rows(library, c);
        let filter = c.filter.to_lowercase();
        rows.retain(|(_, label, _)| label.to_lowercase().contains(&filter));
        let mut go = None;
        let mut cancel = false;
        let response = egui::Modal::new(egui::Id::new("move-folder"))
            .frame(widgets::modal_frame(&palette).inner_margin(design::margin_same(inset::DIALOG)))
            .show(ctx, |ui| {
                ui.set_width(460.);
                ui.label(
                    egui::RichText::new(format!("Move Folder “{}”", c.name))
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
                        for (at, label, path) in &rows {
                            let row = ui
                                .selectable_label(c.chosen.as_ref() == Some(at), label)
                                .on_hover_text(path.display().to_string());
                            if row.clicked() {
                                c.chosen = Some(at.clone());
                            }
                            if row.double_clicked() {
                                go = Some(at.clone());
                            }
                        }
                        if rows.is_empty() {
                            ui.label(
                                egui::RichText::new("No folder is named so.")
                                    .size(text::LABEL)
                                    .color(palette.tone(Tone::MUTED)),
                            );
                        }
                    });
                ui.add_space(space::XS);
                ui.label(
                    egui::RichText::new(
                        "To another drive, a folder is copied, checked, and then put in the Trash.",
                    )
                    .size(text::LABEL)
                    .color(palette.tone(Tone::MUTED)),
                );
                ui.add_space(space::XL);
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding = button::DIALOG;
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    let chosen = c
                        .chosen
                        .clone()
                        .filter(|chosen| rows.iter().any(|(r, ..)| r == chosen));
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
        if let Some((to_root, to)) = go {
            let State::ChoosingParent(c) = std::mem::take(&mut self.file_ops.state) else {
                return;
            };
            self.request_folder_move(c.root, c.from, to_root, to);
        }
    }
}
