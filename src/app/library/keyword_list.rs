//! Lightroom's Keyword List panel: every keyword of the catalog as a tree,
//! with how many photos have it and a filter field. Each keyword's box shows
//! whether the photos the Keywording panel edits have it (ticked, empty, or
//! a dash when only some do); clicking it adds the keyword to all of them,
//! or removes it when all have it. The arrow shows the photos with it. Its
//! menus create, rename and delete keywords and purge the unused ones, each
//! one step of the shared undo log.
use super::descriptive::DescriptiveEdit;
use super::facets::Attribute;
use super::rows::{ROW, VALUE_GRAY, paint_truncated, panel_edit};
use super::{Library, Place};
use crate::app::ui::design::text;
use crate::app::ui::design::{self, Tone, button, inset, space};
use crate::app::ui::theme;
use crate::app::ui::widgets::{modal_frame, primary_button};
use crate::catalog::ListedKeyword;
use crate::catalog_session::KeywordListEdit;
use crate::metadata::keyword_key;
use eframe::egui::Color32;
use eframe::egui::{self, Vec2};
use std::collections::{HashMap, HashSet};

/// How far each level is indented.
const INDENT: f32 = 12.;

/// The panel's own state, kept while RAWmakase runs.
#[derive(Default)]
pub(super) struct KeywordList {
    /// The filter field's text.
    filter: String,
    /// The keywords whose children show.
    expanded: HashSet<i64>,
    dialog: Option<ListDialog>,
}

/// A window of the panel's.
enum ListDialog {
    /// Create Keyword Tag, inside the keyword named or at the top level.
    Create {
        parent: Option<(i64, String)>,
        name: String,
        focused: bool,
    },
    Rename {
        id: i64,
        name: String,
        focused: bool,
    },
    /// Delete, asked for a keyword some photo has or with keywords inside.
    Delete {
        id: i64,
        question: String,
        detail: String,
    },
}

/// A change to the Keyword List, for the shared undo log.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct KeywordCommand {
    /// Orders it among other changes made in the same frame.
    pub sequence: u64,
    pub change: crate::catalog::KeywordChange,
    pub place: Place,
    /// What changed, as the status line said it.
    pub summary: String,
}
impl KeywordCommand {
    /// Takes photo `id` out of the command; it always keeps the keywords.
    pub(in crate::app) fn forget_photo(&mut self, id: crate::catalog::PhotoId) {
        self.change.before.forget_photo(id);
        self.change.after.forget_photo(id);
        self.place.forget_photo(id);
    }
}

/// One row of the tree as drawn.
#[derive(Debug, PartialEq)]
pub(super) struct Row<'k> {
    pub keyword: &'k ListedKeyword,
    pub depth: usize,
    pub has_children: bool,
    pub expanded: bool,
}

/// The rows shown, in order: each keyword's children under it by name,
/// those of keywords not expanded left out. With a filter, the keywords whose
/// name contains it, ignoring case, and their parents, all expanded.
pub(super) fn rows<'k>(
    keywords: &'k [ListedKeyword],
    expanded: &HashSet<i64>,
    filter: &str,
) -> Vec<Row<'k>> {
    let mut children: HashMap<Option<i64>, Vec<&ListedKeyword>> = HashMap::new();
    for k in keywords {
        children.entry(k.parent).or_default().push(k);
    }
    for list in children.values_mut() {
        list.sort_by(|a, b| {
            keyword_key(&a.name)
                .cmp(&keyword_key(&b.name))
                .then(a.id.cmp(&b.id))
        });
    }
    let filter = keyword_key(filter);
    // With a filter, the keywords shown: the matches and their parents.
    let shown: Option<HashSet<i64>> = (!filter.is_empty()).then(|| {
        let by_id: HashMap<i64, &ListedKeyword> = keywords.iter().map(|k| (k.id, k)).collect();
        let mut shown = HashSet::new();
        for k in keywords {
            if keyword_key(&k.name).contains(&filter) {
                let mut next = Some(k.id);
                while let Some(id) = next {
                    if !shown.insert(id) {
                        break;
                    }
                    next = by_id.get(&id).and_then(|k| k.parent);
                }
            }
        }
        shown
    });
    let mut rows = Vec::new();
    let mut stack: Vec<(&ListedKeyword, usize)> = children
        .get(&None)
        .into_iter()
        .flatten()
        .rev()
        .map(|k| (*k, 0))
        .collect();
    while let Some((k, depth)) = stack.pop() {
        if shown.as_ref().is_some_and(|s| !s.contains(&k.id)) {
            continue;
        }
        let kids = children.get(&Some(k.id));
        let has_children = kids.is_some_and(|kids| {
            kids.iter()
                .any(|c| shown.as_ref().is_none_or(|s| s.contains(&c.id)))
        });
        let open = has_children && (shown.is_some() || expanded.contains(&k.id));
        rows.push(Row {
            keyword: k,
            depth,
            has_children,
            expanded: open,
        });
        // A cycle in a damaged catalog would never end.
        if open && depth < 64 {
            stack.extend(kids.into_iter().flatten().rev().map(|c| (*c, depth + 1)));
        }
    }
    rows
}

/// What a click in the panel asked for.
enum Clicked {
    Toggle(i64),
    Tick { id: i64, all: bool },
    Show(i64),
    Create(Option<(i64, String)>),
    Rename(i64, String),
    Delete(i64),
    Purge,
}

/// The menu of a keyword's row, or of the panel's own button without one.
fn keyword_menu(ui: &mut egui::Ui, keyword: Option<&ListedKeyword>, clicked: &mut Option<Clicked>) {
    if let Some(k) = keyword
        && ui
            .button(format!("Create Keyword Tag inside “{}”…", k.name))
            .clicked()
    {
        *clicked = Some(Clicked::Create(Some((k.id, k.name.clone()))));
    }
    if ui.button("Create Keyword Tag…").clicked() {
        *clicked = Some(Clicked::Create(None));
    }
    if let Some(k) = keyword {
        ui.separator();
        if ui.button("Rename…").clicked() {
            *clicked = Some(Clicked::Rename(k.id, k.name.clone()));
        }
        if ui.button(format!("Delete “{}”", k.name)).clicked() {
            *clicked = Some(Clicked::Delete(k.id));
        }
    }
    ui.separator();
    if ui.button("Purge Unused Keywords").clicked() {
        *clicked = Some(Clicked::Purge);
    }
}

impl Library {
    /// The Keyword List panel's contents.
    pub(super) fn keyword_list(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(ui.ctx());
        self.sync_fields();
        let targets = self.fields.targets.clone();
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), egui::Sense::hover());
        let menu = egui::Rect::from_min_max(egui::pos2(rect.right() - 20., rect.top()), rect.max);
        ui.put(
            egui::Rect::from_min_max(rect.min, egui::pos2(menu.left() - 2., rect.bottom()))
                .shrink2(Vec2::new(0., 1.)),
            panel_edit(
                &palette,
                egui::TextEdit::singleline(&mut self.keyword_list.filter),
            )
            .hint_text("Filter Keywords")
            .vertical_align(egui::Align::Center),
        );
        let mut clicked = None;
        let mut menu_ui = ui.new_child(egui::UiBuilder::new().max_rect(menu));
        menu_ui
            .menu_button(egui::RichText::new("⋯").size(text::LABEL), |ui| {
                keyword_menu(ui, None, &mut clicked);
            })
            .response
            .on_hover_text("Create a keyword or purge the unused ones");
        let keywords = self.session.keywords();
        let rows = rows(
            keywords,
            &self.keyword_list.expanded,
            &self.keyword_list.filter,
        );
        if rows.is_empty() {
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), egui::Sense::hover());
            paint_truncated(
                ui,
                rect.left_center(),
                rect.width(),
                if keywords.is_empty() {
                    "No keywords yet"
                } else {
                    "No keyword matches"
                },
                palette.tone(VALUE_GRAY),
            );
        }
        for row in &rows {
            let (rect, response) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), egui::Sense::click());
            let left = rect.left() + row.depth as f32 * INDENT;
            let k = row.keyword;
            response.context_menu(|ui| keyword_menu(ui, Some(k), &mut clicked));
            if row.has_children {
                let triangle =
                    egui::Rect::from_min_size(egui::pos2(left, rect.top()), Vec2::new(14., ROW));
                if ui
                    .put(
                        triangle,
                        egui::Button::new(
                            egui::RichText::new(if row.expanded { "▾" } else { "▸" })
                                .size(text::SMALL),
                        )
                        .frame(false),
                    )
                    .clicked()
                {
                    clicked = Some(Clicked::Toggle(k.id));
                }
            }
            // Whether the photos edited have it: all, some or none.
            let state = self.fields.keyword_state(k.id);
            let mut all = state == Some(true);
            let tick =
                egui::Rect::from_min_size(egui::pos2(left + 14., rect.top()), Vec2::new(16., ROW));
            if ui
                .add_enabled_ui(!targets.is_empty(), |ui| {
                    ui.put(
                        tick,
                        egui::Checkbox::without_text(&mut all).indeterminate(state == Some(false)),
                    )
                })
                .inner
                .on_hover_text(match state {
                    Some(true) => "Remove this keyword from the selected photos",
                    Some(false) => "Some of the selected photos have it; add it to all",
                    None => "Add this keyword to the selected photos",
                })
                .clicked()
            {
                clicked = Some(Clicked::Tick {
                    id: k.id,
                    all: state == Some(true),
                });
            }
            let count = k.photos.to_string();
            let arrow = egui::Rect::from_min_max(
                egui::pos2(rect.right() - 16., rect.top()),
                rect.right_bottom(),
            );
            let count_width = 8. * count.len() as f32 + 6.;
            let name_left = tick.right() + 4.;
            paint_truncated(
                ui,
                egui::pos2(name_left, rect.center().y),
                arrow.left() - count_width - name_left,
                &k.name,
                palette.tone(VALUE_GRAY),
            );
            ui.painter().text(
                egui::pos2(arrow.left() - 4., rect.center().y),
                egui::Align2::RIGHT_CENTER,
                &count,
                super::rows::font(),
                palette.tone(VALUE_GRAY),
            );
            // As Lightroom's, the arrow shows while the pointer is on the row.
            if (response.hovered() || ui.rect_contains_pointer(rect))
                && ui
                    .put(
                        arrow,
                        egui::Button::new(egui::RichText::new("→").size(text::SMALL)).frame(false),
                    )
                    .on_hover_text("Show the photos with this keyword")
                    .clicked()
            {
                clicked = Some(Clicked::Show(k.id));
            }
        }
        match clicked {
            Some(Clicked::Toggle(id)) => {
                if !self.keyword_list.expanded.remove(&id) {
                    self.keyword_list.expanded.insert(id);
                }
            }
            Some(Clicked::Tick { id, all }) => {
                let edit = if all {
                    DescriptiveEdit::RemoveKeyword(id)
                } else {
                    DescriptiveEdit::AddKeyword(id)
                };
                if let Err(e) = self.edit_descriptive(&targets, edit) {
                    self.messages
                        .fail(format!("Keyword could not be changed: {e}"));
                }
            }
            Some(Clicked::Show(id)) => self.show_keyword(id),
            Some(Clicked::Create(parent)) => {
                self.keyword_list.dialog = Some(ListDialog::Create {
                    parent,
                    name: String::new(),
                    focused: false,
                });
            }
            Some(Clicked::Rename(id, name)) => {
                self.keyword_list.dialog = Some(ListDialog::Rename {
                    id,
                    name,
                    focused: false,
                });
            }
            Some(Clicked::Delete(id)) => self.ask_to_delete_keyword(id),
            Some(Clicked::Purge) => {
                self.edit_keyword_list(KeywordListEdit::PurgeUnused, "Purge Unused Keywords");
            }
            None => {}
        }
    }
    /// Deletes keyword `id` at once when no photo has it and it has no
    /// keywords inside; else asks first, as Lightroom does.
    pub(super) fn ask_to_delete_keyword(&mut self, id: i64) {
        let keywords = self.session.keywords();
        let Some(k) = keywords.iter().find(|k| k.id == id) else {
            return;
        };
        let inside = keywords.iter().any(|c| c.parent == Some(id));
        let detail = if inside {
            "It contains additional keywords which will also be deleted. Those keywords \
             will be removed from any photos which use them."
                .to_string()
        } else {
            match k.photos {
                0 => String::new(),
                1 => "It is used on one photo, from which this keyword will be removed.".into(),
                n => format!("It is used on {n} photos from which this keyword will be removed."),
            }
        };
        if detail.is_empty() {
            self.edit_keyword_list(KeywordListEdit::Delete(vec![id]), "Delete Keyword");
        } else {
            self.keyword_list.dialog = Some(ListDialog::Delete {
                id,
                question: format!("Delete the keyword “{}”?", k.name),
                detail,
            });
        }
    }
    /// Makes `edit` to the Keyword List and hands it to the undo log as
    /// `summary`.
    pub(super) fn edit_keyword_list(&mut self, edit: KeywordListEdit, summary: &str) {
        match self.session.edit_keyword_list(edit) {
            Ok(committed) => {
                self.filter();
                self.fields.reload();
                if committed.value.before != committed.value.after {
                    self.messages.note(summary.to_string());
                    self.keyword_done.push(KeywordCommand {
                        sequence: crate::edit_session::sequence(),
                        change: committed.value,
                        place: self.place(),
                        summary: summary.to_string(),
                    });
                }
                if let Err(e) = committed.listed {
                    self.messages
                        .fail(format!("Keywords could not be read again: {e:#}"));
                }
            }
            Err(e) => self.messages.fail(format!("{summary}: {e:#}")),
        }
    }
    /// Puts the Keyword List back as `rows` has it, for undo and redo.
    pub(in crate::app) fn restore_keywords(
        &mut self,
        rows: &crate::catalog::KeywordRows,
        other: &crate::catalog::KeywordRows,
    ) -> anyhow::Result<()> {
        self.session.restore_keywords(rows, other)?;
        self.filter();
        self.fields.reload();
        Ok(())
    }
    /// The Keyword List changes made since the last call, for the undo log.
    pub(in crate::app) fn take_keyword_done(&mut self) -> Vec<KeywordCommand> {
        std::mem::take(&mut self.keyword_done)
    }
    /// Whether one of the panel's windows is open.
    pub(in crate::app) fn keyword_list_dialog_open(&self) -> bool {
        self.keyword_list.dialog.is_some()
    }
    /// The panel's window, while one is open.
    pub(in crate::app) fn keyword_list_window(&mut self, ctx: &egui::Context) {
        let palette = theme::palette(ctx);
        let Some(dialog) = &mut self.keyword_list.dialog else {
            return;
        };
        let mut done: Option<bool> = None;
        let response = egui::Modal::new(egui::Id::new("keyword-list-dialog"))
            .backdrop_color(Color32::from_black_alpha(140))
            .frame(modal_frame(&palette).inner_margin(design::margin_same(inset::DIALOG)))
            .show(ctx, |ui| {
                ui.set_width(380.);
                let (title, action) = match dialog {
                    ListDialog::Create { .. } => ("Create Keyword Tag", "Create"),
                    ListDialog::Rename { .. } => ("Edit Keyword Tag", "Save"),
                    ListDialog::Delete { .. } => ("Delete Keyword", "Delete"),
                };
                ui.label(
                    egui::RichText::new(title)
                        .size(text::TITLE)
                        .color(palette.tone(Tone::STRONG)),
                );
                ui.add_space(space::L);
                let mut ready = true;
                match dialog {
                    ListDialog::Create {
                        parent,
                        name,
                        focused,
                    } => {
                        if let Some((_, parent)) = parent {
                            ui.label(
                                egui::RichText::new(format!("Inside “{parent}”"))
                                    .size(text::SMALL)
                                    .color(palette.tone(Tone::SECONDARY)),
                            );
                            ui.add_space(space::XS);
                        }
                        ready = name_field(ui, name, focused, &mut done);
                    }
                    ListDialog::Rename { name, focused, .. } => {
                        ready = name_field(ui, name, focused, &mut done);
                    }
                    ListDialog::Delete {
                        question, detail, ..
                    } => {
                        ui.label(question.as_str());
                        ui.label(
                            egui::RichText::new(detail.as_str())
                                .size(text::SMALL)
                                .color(palette.tone(Tone::SECONDARY)),
                        );
                    }
                }
                ui.add_space(space::XL);
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding = button::DIALOG;
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled_ui(ready, |ui| primary_button(ui, action))
                            .inner
                            .clicked()
                        {
                            done = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            done = Some(false);
                        }
                    });
                });
            });
        if response.should_close() {
            done = Some(false);
        }
        match done {
            Some(true) => {
                let dialog = self.keyword_list.dialog.take();
                // A refusal (a name taken) is said; the window has closed.
                match dialog {
                    Some(ListDialog::Create { parent, name, .. }) => {
                        self.edit_keyword_list(
                            KeywordListEdit::Create {
                                name,
                                parent: parent.map(|(id, _)| id),
                            },
                            "Create Keyword",
                        );
                    }
                    Some(ListDialog::Rename { id, name, .. }) => {
                        self.edit_keyword_list(
                            KeywordListEdit::Rename { id, name },
                            "Rename Keyword",
                        );
                    }
                    Some(ListDialog::Delete { id, .. }) => {
                        self.edit_keyword_list(KeywordListEdit::Delete(vec![id]), "Delete Keyword");
                    }
                    None => {}
                }
            }
            Some(false) => self.keyword_list.dialog = None,
            None => {}
        }
    }
    /// Shows every photo in the catalog with keyword `id`, as Lightroom's
    /// arrow does: All Photographs, filtered by it in the Metadata filter,
    /// where a keyword another shares a name with is listed apart.
    pub(super) fn show_keyword(&mut self, id: i64) {
        let Some(name) = crate::catalog::keyword_labels(self.session.keywords()).remove(&id) else {
            return;
        };
        self.filters.folder_scope = None;
        self.selected_folders.clear();
        self.filters.collection = None;
        self.filters.only_missing = false;
        self.filters.previous_import = false;
        self.filters.metadata.clear();
        self.filters
            .metadata
            .insert(Attribute::Keyword, [Some(name)].into_iter().collect());
        self.filters.enabled = true;
        self.metadata_bar.open = true;
        self.filter();
        self.show_grid();
    }
}

/// A dialog's name field, which takes the keyboard once; whether a name is
/// typed. Return confirms.
fn name_field(
    ui: &mut egui::Ui,
    name: &mut String,
    focused: &mut bool,
    done: &mut Option<bool>,
) -> bool {
    let field = ui.add(
        egui::TextEdit::singleline(name)
            .hint_text("Keyword name")
            .desired_width(f32::INFINITY),
    );
    if !*focused {
        field.request_focus();
        *focused = true;
    }
    let named = !name.trim().is_empty();
    if named && field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        *done = Some(true);
    }
    named
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keyword(id: i64, name: &str, parent: Option<i64>) -> ListedKeyword {
        ListedKeyword {
            id,
            name: name.into(),
            parent,
            photos: 0,
        }
    }

    fn names(rows: &[Row<'_>]) -> Vec<String> {
        rows.iter()
            .map(|r| format!("{}{}", "  ".repeat(r.depth), r.keyword.name))
            .collect()
    }

    #[test]
    fn children_show_under_expanded_parents_by_name() {
        let list = [
            keyword(1, "Places", None),
            keyword(2, "Paris", Some(1)),
            keyword(3, "animals", None),
            keyword(4, "Berlin", Some(1)),
        ];
        let collapsed = rows(&list, &HashSet::new(), "");
        assert_eq!(names(&collapsed), ["animals", "Places"]);
        assert!(collapsed[1].has_children && !collapsed[1].expanded);
        let open = rows(&list, &[1].into_iter().collect(), "");
        assert_eq!(names(&open), ["animals", "Places", "  Berlin", "  Paris"]);
    }

    #[test]
    fn a_filter_shows_matches_with_their_parents_expanded() {
        let list = [
            keyword(1, "Places", None),
            keyword(2, "Paris", Some(1)),
            keyword(3, "Berlin", Some(1)),
            keyword(4, "Party", None),
        ];
        let found = rows(&list, &HashSet::new(), "par");
        assert_eq!(names(&found), ["Party", "Places", "  Paris"]);
        assert!(rows(&list, &HashSet::new(), "zzz").is_empty());
    }
}
