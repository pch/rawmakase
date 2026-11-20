//! The Collections panel: collection sets, collections and smart
//! collections, made here or imported from Lightroom, as Lightroom
//! Classic's panel shows them. A set is selected to show every photo in it;
//! photos are dragged onto a collection, and collections and sets into a
//! set. A smart collection's count is its members' once worked out; one
//! whose rules can't be evaluated shows a warning instead. Lightroom's own
//! (the Quick Collection, unsaved creations) never show.
use crate::app::ui::design::{Tone, text};
use crate::app::ui::icons::{self, Icon};
use crate::app::ui::theme;
use crate::catalog::{Collection, CollectionId, CollectionKind, PhotoId};
use eframe::egui::{self, Vec2};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) struct CollectionNode {
    pub id: CollectionId,
    pub name: String,
    pub parent: Option<CollectionId>,
    pub set: bool,
    pub count: usize,
    /// For a smart collection, what is known of its members.
    pub smart: Option<SmartCount>,
    /// The target collection, B's, marked `+` as Lightroom marks it.
    pub target: bool,
    pub children: Vec<CollectionNode>,
}
impl CollectionNode {
    /// It and everything inside it.
    fn subtree(&self, ids: &mut HashSet<CollectionId>) {
        ids.insert(self.id);
        for child in &self.children {
            child.subtree(ids);
        }
    }
}

/// What a row asked for.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum RowAction {
    /// Show the collection's photos, or a set's.
    Select(CollectionId),
    /// Photos dropped on a collection.
    AddPhotos(CollectionId, Vec<PhotoId>),
    /// A collection or set dropped on a set (`None`: the top level).
    Move(CollectionId, Option<CollectionId>),
    /// Create Collection… (`set` false) or Create Collection Set… in a set.
    Create {
        set: bool,
        parent: Option<CollectionId>,
    },
    Rename(CollectionId),
    Delete(CollectionId),
    /// Edit Smart Collection: a smart collection's double-click or menu.
    EditSmart(CollectionId),
    /// Create Smart Collection… in a set (`None`: at the top).
    CreateSmart(Option<CollectionId>),
    /// Set as Target Collection (`None`: the Quick Collection).
    SetTarget(Option<CollectionId>),
}

/// What the panel shows of a smart collection's members.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SmartCount {
    Pending,
    Photos(usize),
    /// Its rules can't be evaluated, or the catalog couldn't be read.
    Unknown,
}

/// The panel's tree, ordered as Lightroom orders it: sets first, then
/// collections, each by name. A set is left out when everything in it is
/// hidden. `smart` is what is known of smart collections' members.
pub(super) fn tree(
    collections: &[Collection],
    counts: &HashMap<CollectionId, HashSet<PhotoId>>,
    smart: &HashMap<CollectionId, SmartCount>,
    target: Option<CollectionId>,
) -> Vec<CollectionNode> {
    let mut children: HashMap<Option<CollectionId>, Vec<&Collection>> = HashMap::new();
    let ids: HashSet<CollectionId> = collections.iter().map(|c| c.id).collect();
    for c in collections {
        // A parent that isn't in the catalog puts the collection at the top.
        let parent = c.parent.filter(|p| ids.contains(p) && *p != c.id);
        children.entry(parent).or_default().push(c);
    }
    let mut seen = HashSet::new();
    nodes(None, &children, counts, smart, target, &mut seen)
}

fn nodes(
    parent: Option<CollectionId>,
    children: &HashMap<Option<CollectionId>, Vec<&Collection>>,
    counts: &HashMap<CollectionId, HashSet<PhotoId>>,
    smart: &HashMap<CollectionId, SmartCount>,
    target: Option<CollectionId>,
    seen: &mut HashSet<CollectionId>,
) -> Vec<CollectionNode> {
    let mut out = Vec::new();
    for c in children.get(&parent).into_iter().flatten() {
        // A parent cycle in a damaged catalog would otherwise never end.
        if !seen.insert(c.id) {
            continue;
        }
        match c.kind {
            CollectionKind::Set => {
                let inner = nodes(Some(c.id), children, counts, smart, target, seen);
                let had_children = children.get(&Some(c.id)).is_some_and(|v| !v.is_empty());
                if inner.is_empty() && had_children {
                    continue;
                }
                out.push(CollectionNode {
                    id: c.id,
                    name: c.name.clone(),
                    parent,
                    set: true,
                    count: 0,
                    smart: None,
                    target: false,
                    children: inner,
                });
            }
            CollectionKind::Collection => out.push(CollectionNode {
                id: c.id,
                name: c.name.clone(),
                parent,
                set: false,
                count: counts.get(&c.id).map_or(0, HashSet::len),
                smart: None,
                target: target == Some(c.id),
                children: Vec::new(),
            }),
            CollectionKind::Smart => {
                let known = smart.get(&c.id).copied().unwrap_or(SmartCount::Pending);
                out.push(CollectionNode {
                    id: c.id,
                    name: c.name.clone(),
                    parent,
                    set: false,
                    count: match known {
                        SmartCount::Photos(n) => n,
                        _ => 0,
                    },
                    smart: Some(known),
                    target: false,
                    children: Vec::new(),
                });
            }
            CollectionKind::System => {}
        }
    }
    out.sort_by_cached_key(|n| (!n.set, n.name.to_lowercase()));
    out
}

/// A collection or set as the control protocol lists it
/// ([`super::Library::listed_collections`]).
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub(in crate::app) struct ListedCollection {
    pub id: CollectionId,
    pub name: String,
    /// `set`, `collection` or `smart`.
    pub kind: &'static str,
    /// The set it is in; `None` at the top.
    pub parent_id: Option<CollectionId>,
    /// How many photos it shows: a set every photo of the collections in
    /// it, each once. `None` for a smart collection whose photos aren't
    /// known yet, or can't be.
    pub photos: Option<usize>,
    /// Whether photos can be added to it and taken out of it.
    pub takes_photos: bool,
    /// For a smart collection: `ready`, `pending` while its photos are
    /// worked out, or `unsupported` when its rules can't be evaluated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smart: Option<&'static str>,
}

impl super::Library {
    /// The collections and sets as the panel shows them, in its order, each
    /// set before what is in it.
    pub(in crate::app) fn listed_collections(&self) -> Vec<ListedCollection> {
        let nodes = tree(
            self.session.collections(),
            self.session.collection_photos(),
            self.smart_counts(),
            self.target(),
        );
        let mut out = Vec::new();
        self.list_nodes(&nodes, &mut out);
        out
    }
    fn list_nodes(&self, nodes: &[CollectionNode], out: &mut Vec<ListedCollection>) {
        for node in nodes {
            let takes_photos =
                self.session.collections().iter().any(|c| {
                    c.id == node.id && c.kind == CollectionKind::Collection && c.takes_photos
                });
            let (kind, photos, smart) = match node.smart {
                _ if node.set => ("set", Some(self.source_members(node.id).len()), None),
                None => ("collection", Some(node.count), None),
                Some(SmartCount::Photos(n)) => ("smart", Some(n), Some("ready")),
                Some(SmartCount::Pending) => ("smart", None, Some("pending")),
                Some(SmartCount::Unknown) => ("smart", None, Some("unsupported")),
            };
            out.push(ListedCollection {
                id: node.id,
                name: node.name.clone(),
                kind,
                parent_id: node.parent,
                photos,
                takes_photos,
                smart,
            });
            self.list_nodes(&node.children, out);
        }
    }
}

/// The key that marks a set as collapsed in the Library's expanded set.
fn collapsed_key(id: CollectionId) -> String {
    format!("collection-set-collapsed:{id}")
}

/// Draws `node` and, unless it is a collapsed set, its children. Returns
/// what a row asked for.
pub(super) fn collection_row(
    ui: &mut egui::Ui,
    node: &CollectionNode,
    depth: usize,
    expanded: &mut HashSet<String>,
    selected: Option<CollectionId>,
) -> Option<RowAction> {
    let palette = theme::palette(ui.ctx());
    use egui::{Align2, FontId, Pos2, Rect, Sense};
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), 22.),
        Sense::click_and_drag(),
    );
    if let Some(dragged) = super::grid::drag_once(&response, || {
        let mut inside = HashSet::new();
        node.subtree(&mut inside);
        super::DraggedCollection {
            id: node.id,
            parent: node.parent,
            inside,
            name: node.name.clone(),
            set: node.set,
        }
    }) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        let icon = if dragged.set {
            Icon::CollectionSet
        } else {
            Icon::Collection
        };
        super::tree::dragged_chip(ui, &dragged.name, icon);
    }
    // Photos go into a collection; a collection or set into a set, but not
    // into itself, one inside it or the set it is in already.
    let takes = |d: &super::DraggedCollection| {
        node.set && !d.inside.contains(&node.id) && d.parent != Some(node.id)
    };
    // A smart collection takes no photos by hand, as in Lightroom.
    let takes_photos = !node.set && node.smart.is_none();
    let dropping = (takes_photos
        && response
            .dnd_hover_payload::<super::DraggedPhotos>()
            .is_some())
        || response
            .dnd_hover_payload::<super::DraggedCollection>()
            .is_some_and(|d| takes(&d));
    let painter = ui.painter();
    let active = selected == Some(node.id);
    let collapsed = expanded.contains(&collapsed_key(node.id));
    if dropping {
        painter.rect_filled(rect, 3., palette.selected_row());
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
    // The same columns as the folder tree.
    let indent = depth.min(12) as f32 * 14.;
    let x = rect.left() + indent + 12.;
    let y = rect.center().y;
    if node.set && !node.children.is_empty() {
        let chevron = if collapsed {
            Icon::ChevronRight
        } else {
            Icon::ChevronDown
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
        if node.set {
            Icon::CollectionSet
        } else if node.smart.is_some() {
            Icon::SmartCollection
        } else {
            Icon::Collection
        },
        Pos2::new(x + 13., y),
        13.,
        palette.tone(Tone::MUTED),
    );
    let label_rect = Rect::from_min_max(
        Pos2::new(x + 26., rect.top()),
        Pos2::new(rect.right() - 40., rect.bottom()),
    );
    let label = if node.target {
        format!("{} +", node.name)
    } else {
        node.name.clone()
    };
    let text = egui::WidgetText::from(label).into_galley(
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
    // Lightroom counts collections, not sets.
    if node.smart == Some(SmartCount::Unknown) {
        icons::paint_at(
            painter,
            Icon::Failed,
            Pos2::new(rect.right() - 14., y),
            11.,
            palette.tone(Tone::SECONDARY),
        );
    } else if node.smart == Some(SmartCount::Pending) {
        painter.text(
            Pos2::new(rect.right() - 10., y),
            Align2::RIGHT_CENTER,
            "…",
            FontId::proportional(text::MICRO),
            palette.tone(Tone::FAINT),
        );
    } else if !node.set {
        painter.text(
            Pos2::new(rect.right() - 10., y),
            Align2::RIGHT_CENTER,
            node.count.to_string(),
            FontId::proportional(text::MICRO),
            palette.tone(Tone::FAINT),
        );
    }
    let mut action = None;
    if takes_photos && let Some(dragged) = super::tree::released::<super::DraggedPhotos>(&response)
    {
        action = Some(RowAction::AddPhotos(node.id, dragged.0.clone()));
    }
    if let Some(dragged) = super::tree::released::<super::DraggedCollection>(&response)
        && takes(&dragged)
    {
        action = Some(RowAction::Move(dragged.id, Some(node.id)));
    }
    if response.clicked() && !crate::app::ui::widgets::context_clicked(&response) {
        let pointer = response.interact_pointer_pos().unwrap_or(rect.center());
        // The chevron folds a set; the rest of the row selects it.
        if node.set && !node.children.is_empty() && pointer.x < x + 6. {
            let key = collapsed_key(node.id);
            if collapsed {
                expanded.remove(&key);
            } else {
                expanded.insert(key);
            }
        } else {
            action = Some(RowAction::Select(node.id));
        }
    }
    if node.smart.is_some() && response.double_clicked() {
        action = Some(RowAction::EditSmart(node.id));
    }
    crate::app::ui::widgets::context_menu(&response, |ui| {
        use crate::app::ui::widgets::{menu_item, menu_separator};
        crate::app::ui::widgets::menu_width(ui, 210.);
        ui.spacing_mut().item_spacing.y = 0.;
        // Made inside a set, or beside a collection.
        let parent = if node.set { Some(node.id) } else { node.parent };
        for (title, set) in [
            ("Create Collection…", false),
            ("Create Collection Set…", true),
        ] {
            if menu_item(ui, title, "", true, false) {
                action = Some(RowAction::Create { set, parent });
                ui.close();
            }
        }
        if menu_item(ui, "Create Smart Collection…", "", true, false) {
            action = Some(RowAction::CreateSmart(parent));
            ui.close();
        }
        menu_separator(ui);
        if node.smart.is_some() && menu_item(ui, "Edit Smart Collection…", "", true, false) {
            action = Some(RowAction::EditSmart(node.id));
            ui.close();
        }
        if !node.set
            && node.smart.is_none()
            && menu_item(ui, "Set as Target Collection", "", true, node.target)
        {
            action = Some(RowAction::SetTarget((!node.target).then_some(node.id)));
            ui.close();
        }
        if menu_item(ui, "Rename…", "", true, false) {
            action = Some(RowAction::Rename(node.id));
            ui.close();
        }
        if menu_item(ui, "Delete…", "", true, false) {
            action = Some(RowAction::Delete(node.id));
            ui.close();
        }
    });
    if node.set && !expanded.contains(&collapsed_key(node.id)) {
        for child in &node.children {
            if let Some(child) = collection_row(ui, child, depth + 1, expanded, selected) {
                action = Some(child);
            }
        }
    }
    action
}

/// Shown while a collection or set inside a set is dragged: dropped here,
/// it goes to the top level.
pub(super) fn top_level_drop(ui: &mut egui::Ui) -> Option<RowAction> {
    let dragged = egui::DragAndDrop::payload::<super::DraggedCollection>(ui.ctx())?;
    dragged.parent?;
    let palette = theme::palette(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.), egui::Sense::hover());
    let hovered = response
        .dnd_hover_payload::<super::DraggedCollection>()
        .is_some();
    ui.painter().rect_stroke(
        rect.shrink(0.5),
        3.,
        egui::Stroke::new(
            1.,
            if hovered {
                palette.selected_marker()
            } else {
                palette.tone(Tone::FAINT)
            },
        ),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "Move to the top level",
        egui::FontId::proportional(text::SMALL),
        palette.tone(Tone::MUTED),
    );
    super::tree::released::<super::DraggedCollection>(&response)
        .map(|d| RowAction::Move(d.id, None))
}
