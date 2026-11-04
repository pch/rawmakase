use super::folder_selection::{Click, FolderSelection};
use crate::app::ui::design::{Tone, radius, text};
use crate::app::ui::icons::{self, Icon};
use crate::app::ui::theme;
use crate::catalog::{Folder, FolderId, PhotoId, RootId};
use eframe::egui::{self, Vec2};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
#[derive(Default)]
pub(super) struct FolderNode {
    pub(super) key: String,
    pub(super) name: String,
    pub(super) path: PathBuf,
    pub(super) root: Option<RootId>,
    /// The root it is in, and its logical path there ("" for the root).
    pub(super) in_root: RootId,
    pub(super) logical: String,
    pub(super) folder: Option<FolderId>,
    pub(super) own_count: usize,
    pub(super) count: usize,
    pub(super) ids: HashSet<FolderId>,
    pub(super) children: std::collections::BTreeMap<String, FolderNode>,
}
impl FolderNode {
    pub(super) fn root(id: RootId, name: String, path: PathBuf) -> Self {
        Self {
            key: format!("root:{id}"),
            name,
            path,
            root: Some(id),
            in_root: id,
            ..Default::default()
        }
    }
    /// Adds folder `f` to this root's tree, with the folders above it.
    /// `located` is where folders of the root are apart from it, by logical
    /// path: a folder above `f` with no catalog folder of its own is there.
    pub(super) fn insert(&mut self, f: &Folder, located: &HashMap<&str, &Path>) {
        let mut node = self;
        for component in f.relative.split('/').filter(|c| !c.is_empty()) {
            let key = format!("{}/{}", node.key, component);
            let in_root = node.in_root;
            let logical = match node.logical.as_str() {
                "" => component.to_string(),
                parent => format!("{parent}/{component}"),
            };
            let path = located
                .get(logical.as_str())
                .map_or_else(|| node.path.join(component), |p| p.to_path_buf());
            node = node
                .children
                .entry(component.into())
                .or_insert_with(|| FolderNode {
                    key,
                    name: component.into(),
                    path,
                    in_root,
                    logical,
                    ..Default::default()
                });
        }
        node.folder = Some(f.id);
        node.own_count += f.count;
        node.path = f.path.clone();
    }
    /// Takes the folder at logical path `logical` below this root out of
    /// its tree, with the folders in it.
    pub(super) fn detach(&mut self, logical: &str) -> Option<FolderNode> {
        let (parent, name) = logical.rsplit_once('/').unwrap_or(("", logical));
        let mut node = self;
        for component in parent.split('/').filter(|c| !c.is_empty()) {
            node = node.children.get_mut(component)?;
        }
        node.children.remove(name)
    }
    /// The photos of this folder and the folders still in its tree.
    pub(super) fn photos_in_tree(&self) -> usize {
        self.own_count
            + self
                .children
                .values()
                .map(Self::photos_in_tree)
                .sum::<usize>()
    }
    pub(super) fn finish(&mut self) {
        self.count = self.own_count;
        if let Some(id) = self.folder {
            self.ids.insert(id);
        }
        for child in self.children.values_mut() {
            child.finish();
            self.count += child.count;
            self.ids.extend(&child.ids);
        }
    }
    /// The keys of this row and the rows unfolded below it, in the order
    /// they are drawn.
    pub(super) fn shown_rows<'a>(&'a self, expanded: &HashSet<String>, rows: &mut Vec<&'a str>) {
        rows.push(&self.key);
        if expanded.contains(&self.key) {
            for child in self.children.values() {
                child.shown_rows(expanded, rows);
            }
        }
    }
}
pub(super) enum TreeAction {
    /// The folder's key, and how it was clicked.
    Select(String, Click),
    RelinkRoot(RootId),
    /// Locate this folder…: the folder of `root` at logical path `logical`,
    /// whether or not it has a catalog folder of its own.
    RelinkFolder {
        root: RootId,
        logical: String,
    },
    /// Remove from Catalog: the folder's name, it and its subfolders, and
    /// their photos' count.
    Remove(String, HashSet<FolderId>, usize),
    /// Export Folder as Catalog: the folder's name, it and its subfolders,
    /// and their photos' count.
    ExportCatalog(String, HashSet<FolderId>, usize),
    /// Synchronize Folder: what to synchronize, and its name.
    Synchronize(crate::catalog::folder_sync::SyncScope, String),
    /// New Folder Inside…: the folder's root, logical path, name and
    /// catalog folder, if it has one.
    NewFolder {
        root: RootId,
        parent: String,
        name: String,
        folder: Option<FolderId>,
    },
    /// Rename Folder…: the folder of `root` at logical path `logical`.
    RenameFolder {
        root: RootId,
        logical: String,
    },
    /// Move Folder…: the folder of `root` at logical path `logical`.
    MoveFolder {
        root: RootId,
        logical: String,
    },
    /// Move to Trash…: the folder of `root` at logical path `logical`.
    TrashFolder {
        root: RootId,
        logical: String,
    },
    /// Remove from Catalog… for a root with nothing left in the catalog:
    /// the root and its name.
    RemoveRoot(RootId, String),
    /// The folder of `root` at logical path `from`, dropped on logical
    /// folder `to` of root `to_root` (its own, or another).
    DropFolder {
        root: RootId,
        from: String,
        to_root: RootId,
        to: String,
    },
    /// Show the folder in the file manager.
    Reveal(std::path::PathBuf),
    /// Photos dropped on the folder of `root` at logical path `to`, named
    /// `name`: a catalog folder, or one with no catalog folder of its own.
    Drop {
        root: RootId,
        to: String,
        name: String,
        photos: Vec<PhotoId>,
    },
    /// Show Photos in Subfolders turned on or off.
    ShowSubfolders(bool),
}
/// The photos a row counts: its own, and its subfolders' while they're shown.
pub(super) fn shown_count(node: &FolderNode, subfolders: bool) -> usize {
    if subfolders {
        node.count
    } else {
        node.own_count
    }
}
/// What of type `P` was dropped on `response` this frame. egui's own
/// `dnd_release_payload` takes whatever is dragged, of any type, so a row
/// asking for photos would lose a folder dropped on it.
fn released<P: std::any::Any + Send + Sync>(
    response: &egui::Response,
) -> Option<std::sync::Arc<P>> {
    egui::DragAndDrop::has_payload_of_type::<P>(&response.ctx)
        .then(|| response.dnd_release_payload::<P>())
        .flatten()
}
/// A dragged folder's icon and name, following the pointer.
fn dragged_chip(ui: &egui::Ui, name: &str) {
    let Some(pos) = ui.ctx().pointer_latest_pos() else {
        return;
    };
    let palette = theme::palette(ui.ctx());
    let layer = egui::LayerId::new(egui::Order::Tooltip, ui.id().with("dragged-folder"));
    let painter = ui.ctx().layer_painter(layer);
    let text = painter.layout_no_wrap(
        name.to_string(),
        egui::FontId::proportional(text::LABEL),
        palette.tone(Tone::STRONG),
    );
    let chip =
        egui::Rect::from_min_size(pos + Vec2::splat(10.), Vec2::new(text.size().x + 34., 22.));
    painter.rect_filled(chip, radius::CONTROL, palette.selected_row());
    painter.rect_stroke(
        chip.shrink(0.5),
        4.,
        egui::Stroke::new(1., palette.selected_marker()),
        egui::StrokeKind::Inside,
    );
    icons::paint_at(
        &painter,
        Icon::Folder,
        egui::pos2(chip.left() + 14., chip.center().y),
        13.,
        palette.tone(Tone::SOFT),
    );
    painter.galley(
        egui::pos2(chip.left() + 26., chip.center().y - text.size().y / 2.),
        text,
        palette.tone(Tone::STRONG),
    );
}
pub(super) fn folder_tree_row(
    ui: &mut egui::Ui,
    node: &FolderNode,
    depth: usize,
    expanded: &mut HashSet<String>,
    selected: &FolderSelection,
    subfolders: bool,
) -> Option<TreeAction> {
    let palette = theme::palette(ui.ctx());
    use egui::{Align2, FontId, Pos2, Rect, Sense};
    // Any folder but a root can be renamed, put in the Trash or dragged
    // onto another folder of its root, one with no catalog folder of its
    // own (only folders below) too; a root, only dragged onto a folder of
    // another root, or that root (#586).
    let movable = node.root.is_none();
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), 22.),
        Sense::click_and_drag(),
    );
    // Named from what is dragged: a row's id is its place in the panel.
    if let Some(dragged) = super::grid::drag_once(&response, || super::DraggedFolder {
        root: node.in_root,
        from: node.logical.clone(),
        name: node.name.clone(),
    }) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        dragged_chip(ui, &dragged.name);
    }
    let painter = ui.painter();
    let active = selected.contains(&node.key);
    let open = expanded.contains(&node.key);
    // Not where it is already, nor itself or a folder in it; any folder of
    // another root (whose drive the plan checks). A root, only those.
    let takes = |d: &super::DraggedFolder| {
        d.root != node.in_root
            || (!d.from.is_empty()
                && node.logical != d.from
                && !node.logical.starts_with(&format!("{}/", d.from))
                && node.logical != d.from.rsplit_once('/').map_or("", |(p, _)| p))
    };
    // Photos dragged over any folder can be dropped on it, and a folder
    // over any folder that takes it.
    let dropping = response
        .dnd_hover_payload::<super::DraggedPhotos>()
        .is_some()
        || response
            .dnd_hover_payload::<super::DraggedFolder>()
            .is_some_and(|d| takes(&d));
    if dropping {
        painter.rect_filled(rect, radius::CONTROL, palette.selected_row());
        painter.rect_stroke(
            rect.shrink(0.5),
            3.,
            egui::Stroke::new(1., palette.selected_marker()),
            egui::StrokeKind::Inside,
        );
    } else if active || response.hovered() {
        painter.rect_filled(
            rect,
            3.,
            if active {
                palette.selected_row()
            } else {
                palette.tone(Tone::RAISED)
            },
        );
    }
    if active {
        painter.rect_filled(
            Rect::from_min_size(rect.min, Vec2::new(2., rect.height())),
            0.,
            palette.selected_marker(),
        );
    }
    let indent = depth.min(12) as f32 * 14.;
    // The chevron in the gutter, starting at the Catalog rows' 10 px inset
    // under a drive's dot; the folder icon under the drive's name.
    let x = rect.left() + indent + 12.;
    let y = rect.center().y;
    if !node.children.is_empty() {
        let chevron = if open {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        };
        icons::paint_at(
            painter,
            chevron,
            Pos2::new(x, y),
            11.,
            palette.tone(Tone::SECONDARY),
        );
    }
    icons::paint_at(
        painter,
        Icon::Folder,
        Pos2::new(x + 13., y),
        13.,
        palette.tone(Tone::MUTED),
    );
    let label_rect = Rect::from_min_max(
        Pos2::new(x + 26., rect.top()),
        Pos2::new(rect.right() - 44., rect.bottom()),
    );
    let text = egui::WidgetText::from(node.name.clone()).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        label_rect.width().max(1.),
        FontId::proportional(text::LABEL),
    );
    painter.galley(
        Pos2::new(label_rect.left(), y - text.size().y / 2.),
        text,
        palette.tone(if active { Tone::STRONG } else { Tone::SOFT }),
    );
    painter.text(
        Pos2::new(rect.right() - 10., y),
        Align2::RIGHT_CENTER,
        shown_count(node, subfolders).to_string(),
        FontId::proportional(text::MICRO),
        palette.tone(Tone::FAINT),
    );
    let relink = || {
        node.root
            .map(TreeAction::RelinkRoot)
            .unwrap_or_else(|| TreeAction::RelinkFolder {
                root: node.in_root,
                logical: node.logical.clone(),
            })
    };
    let mut action = None;
    // Photos are dragged only once the pointer went far enough (see
    // `grid::drag_payload`).
    if let Some(dragged) = released::<super::DraggedPhotos>(&response) {
        action = Some(TreeAction::Drop {
            root: node.in_root,
            to: node.logical.clone(),
            name: node.name.clone(),
            photos: dragged.0.clone(),
        });
    }
    if let Some(dragged) = released::<super::DraggedFolder>(&response)
        && takes(&dragged)
    {
        action = Some(TreeAction::DropFolder {
            root: dragged.root,
            from: dragged.from.clone(),
            to_root: node.in_root,
            to: node.logical.clone(),
        });
    }
    if response.clicked() && !crate::app::ui::widgets::context_clicked(&response) {
        let pointer = response.interact_pointer_pos().unwrap_or(rect.center());
        if !node.children.is_empty() && pointer.x < x + 6. {
            if open {
                expanded.remove(&node.key);
            } else {
                expanded.insert(node.key.clone());
            }
        } else {
            let click = Click::of(ui.input(|i| i.modifiers));
            action = Some(TreeAction::Select(node.key.clone(), click));
        }
    }
    crate::app::ui::widgets::context_menu(&response, |ui| {
        use crate::app::ui::widgets::{menu_item, menu_separator};
        ui.set_width(210.);
        ui.spacing_mut().item_spacing.y = 0.;
        if menu_item(
            ui,
            if node.root.is_some() {
                "Locate root folder…"
            } else {
                "Locate this folder…"
            },
            "",
            true,
            false,
        ) {
            action = Some(relink());
            ui.close();
        }
        let scope = match (node.root, node.folder) {
            (Some(root), _) => Some(crate::catalog::folder_sync::SyncScope::Root(root)),
            (None, Some(folder)) => Some(crate::catalog::folder_sync::SyncScope::Folder(folder)),
            _ => None,
        };
        if let Some(scope) = scope
            && menu_item(ui, "Synchronize Folder…", "", true, false)
        {
            action = Some(TreeAction::Synchronize(scope, node.name.clone()));
            ui.close();
        }
        menu_separator(ui);
        if menu_item(ui, "New Folder Inside…", "", true, false) {
            action = Some(TreeAction::NewFolder {
                root: node.in_root,
                parent: node.logical.clone(),
                name: node.name.clone(),
                folder: node.folder,
            });
            ui.close();
        }
        // A root is renamed in Finder, then located again.
        if movable && menu_item(ui, "Rename Folder…", "", true, false) {
            action = Some(TreeAction::RenameFolder {
                root: node.in_root,
                logical: node.logical.clone(),
            });
            ui.close();
        }
        // A root, into a folder of another root (#586).
        if menu_item(ui, "Move Folder…", "", true, false) {
            action = Some(TreeAction::MoveFolder {
                root: node.in_root,
                logical: node.logical.clone(),
            });
            ui.close();
        }
        if menu_item(ui, crate::platform::reveal::LABEL, "", true, false) {
            action = Some(TreeAction::Reveal(node.path.clone()));
            ui.close();
        }
        menu_separator(ui);
        if menu_item(ui, "Show Photos in Subfolders", "", true, subfolders) {
            action = Some(TreeAction::ShowSubfolders(!subfolders));
            ui.close();
        }
        if movable || !node.ids.is_empty() || node.root.is_some() {
            menu_separator(ui);
        }
        if !node.ids.is_empty() && menu_item(ui, "Export Folder as Catalog…", "", true, false) {
            action = Some(TreeAction::ExportCatalog(
                node.name.clone(),
                node.ids.clone(),
                node.count,
            ));
            ui.close();
        }
        if !node.ids.is_empty() && menu_item(ui, "Remove from Catalog…", "", true, false) {
            action = Some(TreeAction::Remove(
                node.name.clone(),
                node.ids.clone(),
                node.count,
            ));
            ui.close();
        }
        // A root its last folder left (a folder put in the Trash, or moved
        // to another root), as Lightroom keeps a parent folder.
        if let (Some(root), true) = (node.root, node.ids.is_empty())
            && menu_item(ui, "Remove from Catalog…", "", true, false)
        {
            action = Some(TreeAction::RemoveRoot(root, node.name.clone()));
            ui.close();
        }
        // A root is put in the Trash in Finder, once removed here.
        if movable && menu_item(ui, "Move to Trash…", "", true, false) {
            action = Some(TreeAction::TrashFolder {
                root: node.in_root,
                logical: node.logical.clone(),
            });
            ui.close();
        }
    });
    if expanded.contains(&node.key) {
        for child in node.children.values() {
            if let Some(a) = folder_tree_row(ui, child, depth + 1, expanded, selected, subfolders) {
                action = Some(a)
            }
        }
    }
    action
}
