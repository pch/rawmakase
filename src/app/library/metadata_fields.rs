//! Title, Caption, Creator, Copyright and Location in the Metadata panel and
//! the keywords in Keywording, edited in place like the Copy Name: saved on
//! Return or when focus leaves, for every photo the panel shows. A field whose
//! photos differ shows "< mixed >"; typing replaces it on all of them, leaving
//! it alone changes nothing.
use super::descriptive::{DescriptiveEdit, parse_keywords};
use super::rows::{ROW, VALUE_GRAY, caption_at, field_rect, paint_truncated, panel_edit, value_at};
use super::{Library, Place};
use crate::app::ui::design::{space, text};
use crate::app::ui::theme;
use crate::catalog::PhotoId;
use crate::metadata::{Descriptive, Keyword, Location, TextField, Value};
use eframe::egui::{self, Vec2};

const MIXED: &str = "< mixed >";
const CAPTION: f32 = 46.;

/// A field's value across the photos shown.
#[derive(Clone, Debug, Default, PartialEq)]
enum Shared<T> {
    #[default]
    None,
    Same(T),
    Mixed,
}
impl<T: PartialEq + Clone> Shared<T> {
    fn of(values: impl IntoIterator<Item = T>) -> Self {
        let mut shared = Self::None;
        for v in values {
            shared = match shared {
                Self::None => Self::Same(v),
                Self::Same(s) if s == v => Self::Same(s),
                _ => Self::Mixed,
            };
        }
        shared
    }
}

/// What the panel shows, and what is being typed, for `targets`.
#[derive(Default)]
pub(super) struct Fields {
    /// The photos shown; edits go to all of them.
    pub(super) targets: Vec<PhotoId>,
    /// Read again before the next frame draws.
    stale: bool,
    title: Shared<String>,
    caption: Shared<String>,
    copyright: Shared<String>,
    creators: Shared<Vec<String>>,
    location: Shared<Option<Location>>,
    /// Each keyword, and whether every photo shown has it.
    keywords: Vec<(Keyword, bool)>,
    /// Text as typed, from the value shown when typing started.
    pub(super) drafts: Drafts,
    pub(super) keyword_entry: String,
    /// Where the Library was when the values were read, for undo to return
    /// to after a draft is saved elsewhere.
    place: Option<Place>,
    /// "+" added an empty creator entry, to keep after a save reads the
    /// values again.
    add_creator: bool,
    /// Fields typed in, even back to how they were: emptying a mixed field
    /// clears it on every photo.
    edited: Edited,
    /// The drafts were saved and the values not read again yet.
    saved: bool,
}
/// The fields typed in since the values were read or saved.
#[derive(Clone, Copy, Default)]
struct Edited {
    title: bool,
    caption: bool,
    copyright: bool,
    creators: bool,
}
impl Edited {
    fn any(&self) -> bool {
        self.title || self.caption || self.copyright || self.creators
    }
}
#[derive(Clone, Default, PartialEq)]
pub(super) struct Drafts {
    pub(super) title: String,
    caption: String,
    copyright: String,
    creators: Vec<String>,
}
impl Fields {
    /// Reads the shown values again before the next frame.
    pub(super) fn reload(&mut self) {
        self.stale = true;
    }
    #[cfg(test)]
    pub(super) fn mark_title_edited_for_tests(&mut self) {
        self.edited.title = true;
    }
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }
    /// The drafts as they are when nothing was typed.
    fn untouched(&self) -> Drafts {
        let text = |s: &Shared<String>| match s {
            Shared::Same(t) => t.clone(),
            _ => String::new(),
        };
        Drafts {
            title: text(&self.title),
            caption: text(&self.caption),
            copyright: text(&self.copyright),
            creators: match &self.creators {
                Shared::Same(names) if !names.is_empty() => {
                    let mut names = names.clone();
                    if self.add_creator {
                        names.push(String::new());
                    }
                    names
                }
                _ => vec![String::new()],
            },
        }
    }
}

/// The default language's text of a field, as the panel shows it.
fn text_of(d: &Descriptive, field: TextField) -> String {
    match d.text(field) {
        Some(Value::Set(langs)) => langs.default_text().unwrap_or_default().to_string(),
        _ => String::new(),
    }
}

impl Library {
    /// The photos the Metadata panel edits: every one selected in the Grid,
    /// else the active one.
    fn field_targets(&self) -> Vec<PhotoId> {
        let mut ids: Vec<PhotoId> = if self.edits_active_only() {
            self.selection.active.into_iter().collect()
        } else {
            self.selected_ids()
        };
        // The same photos in another order (a re-sort) are the same targets.
        ids.sort_unstable();
        ids
    }
    /// Saves what is being typed, for the photos it was typed for. The
    /// drafts are kept on failure.
    pub(super) fn commit_fields(&mut self) -> anyhow::Result<()> {
        // Saved already, and not read again since (an undo may have
        // reversed it meanwhile): nothing to save.
        if self.fields.saved {
            return Ok(());
        }
        let targets = self.fields.targets.clone();
        let untouched = self.fields.untouched();
        let drafts = self.fields.drafts.clone();
        let mut edits = Vec::new();
        let edited = self.fields.edited;
        for (field, now, was, edited) in [
            (
                TextField::Title,
                &drafts.title,
                &untouched.title,
                edited.title,
            ),
            (
                TextField::Caption,
                &drafts.caption,
                &untouched.caption,
                edited.caption,
            ),
            (
                TextField::Copyright,
                &drafts.copyright,
                &untouched.copyright,
                edited.copyright,
            ),
        ] {
            if now != was || edited {
                edits.push(DescriptiveEdit::Text(field, now.trim_end().to_string()));
            }
        }
        let names = |list: &[String]| -> Vec<String> {
            list.iter()
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty())
                .collect()
        };
        // An entry added and left empty changes nothing.
        let creators = names(&drafts.creators);
        if creators != names(&untouched.creators) || edited.creators {
            edits.push(DescriptiveEdit::Creators(creators));
        }
        let place = self.fields.place.clone();
        let saving = !edits.is_empty();
        for edit in edits {
            self.edit_descriptive_at(&targets, edit, place.clone())?;
        }
        self.fields.edited = Edited::default();
        self.fields.saved |= saving;
        self.fields.reload();
        Ok(())
    }
    /// Reads the values shown when the photos shown changed or were edited,
    /// first saving what was typed for the ones before.
    /// Whether a metadata field holds typing not saved yet, for photos shown.
    pub(super) fn has_unsaved_fields(&self) -> bool {
        let f = &self.fields;
        !f.targets.is_empty() && !f.saved && (f.drafts != f.untouched() || f.edited.any())
    }
    pub(super) fn sync_fields(&mut self) {
        let targets = self.field_targets();
        if targets == self.fields.targets && !self.fields.stale {
            return;
        }
        let moved = targets != self.fields.targets;
        // Drafts not shown (a collapsed section) are saved before the values
        // are read again, never dropped.
        let f = &self.fields;
        let pending = !f.saved && (f.drafts != f.untouched() || f.edited.any());
        if (moved || pending)
            && let Err(e) = self.commit_fields()
        {
            // The drafts stay with the photos they were typed for, to be
            // saved again or discarded.
            self.messages
                .fail(format!("Metadata could not be saved: {e}"));
            return;
        }
        let read = self.session.catalog().descriptive_of(&targets);
        let keywords: anyhow::Result<Vec<Vec<Keyword>>> = targets
            .iter()
            .map(|id| self.session.catalog().keywords(*id))
            .collect();
        let (read, keywords) = match (read, keywords) {
            (Ok(r), Ok(k)) => (r, k),
            (Err(e), _) | (_, Err(e)) => {
                self.messages
                    .fail(format!("Metadata could not be read: {e}"));
                return;
            }
        };
        let all: Vec<&Descriptive> = targets.iter().filter_map(|id| read.get(id)).collect();
        let place = self.place();
        let f = &mut self.fields;
        if moved {
            // What was typed for other photos never goes to these.
            f.keyword_entry.clear();
            f.add_creator = false;
            f.place = Some(place);
        }
        f.targets = targets;
        f.stale = false;
        f.title = Shared::of(all.iter().map(|d| text_of(d, TextField::Title)));
        f.caption = Shared::of(all.iter().map(|d| text_of(d, TextField::Caption)));
        f.copyright = Shared::of(all.iter().map(|d| text_of(d, TextField::Copyright)));
        f.creators = Shared::of(all.iter().map(|d| match &d.creator {
            Some(Value::Set(names)) => names.clone(),
            _ => Vec::new(),
        }));
        f.location = Shared::of(all.iter().map(|d| d.location.clone()));
        let mut counts: std::collections::HashMap<i64, (Keyword, usize)> = Default::default();
        for k in keywords.iter().flatten() {
            counts.entry(k.id).or_insert_with(|| (k.clone(), 0)).1 += 1;
        }
        let mut counts: Vec<(Keyword, usize)> = counts.into_values().collect();
        counts.sort_by(|a, b| a.0.path.cmp(&b.0.path).then(a.0.id.cmp(&b.0.id)));
        let n = keywords.len();
        f.keywords = counts.into_iter().map(|(k, c)| (k, c == n)).collect();
        f.drafts = f.untouched();
        f.edited = Edited::default();
        f.saved = false;
    }
    /// Title, Caption, Creator, Copyright and Location rows.
    pub(super) fn metadata_fields(&mut self, ui: &mut egui::Ui) {
        self.sync_fields();
        let mut commit = false;
        let enabled = !self.fields.targets.is_empty();
        ui.add_enabled_ui(enabled, |ui| {
            let f = &mut self.fields;
            let title = text_row(ui, "Title", &mut f.drafts.title, &f.title, false);
            let caption = text_row(ui, "Caption", &mut f.drafts.caption, &f.caption, true);
            let creators = creator_rows(ui, &mut f.drafts.creators, &f.creators);
            f.edited.creators |= creators.typed;
            commit |= creators.commit;
            // A saved entry needs no empty one after it; "+" clicked while
            // an entry was saved keeps its new one.
            if creators.commit && !creators.added {
                f.add_creator = false;
            }
            f.add_creator |= creators.added;
            let copyright = text_row(
                ui,
                "Copyright",
                &mut f.drafts.copyright,
                &f.copyright,
                false,
            );
            commit |= title.left || caption.left || copyright.left;
            f.edited.title |= title.typed;
            f.edited.caption |= caption.typed;
            f.edited.copyright |= copyright.typed;
        });
        if commit && let Err(e) = self.commit_fields() {
            self.messages
                .fail(format!("Metadata could not be saved: {e}"));
        }
        let (text, can_clear) = match &self.fields.location {
            Shared::None | Shared::Same(None) => (String::new(), enabled),
            Shared::Same(Some(Location::At { lat, lon, .. })) => {
                (format!("{lat:.6}, {lon:.6}"), true)
            }
            Shared::Same(Some(Location::Cleared)) => ("Removed".into(), false),
            Shared::Mixed => (MIXED.into(), true),
        };
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), egui::Sense::hover());
        caption_at(ui, rect, "GPS");
        let button = egui::Rect::from_min_max(
            egui::pos2(rect.right() - 40., rect.top()),
            rect.right_bottom(),
        );
        value_at(
            ui,
            egui::Rect::from_min_max(rect.min, egui::pos2(button.left() - 4., rect.bottom())),
            if text.is_empty() { "—" } else { &text },
            !text.is_empty(),
        );
        response.on_hover_text(if text.is_empty() {
            "The location in the file, if it has one"
        } else {
            &text
        });
        let clear = ui
            .put(
                button,
                egui::Button::new(egui::RichText::new("Clear").size(text::SMALL)).frame(false),
            )
            .on_hover_text("Clear Location: leave it out of exports, the file's included");
        if clear.clicked() && can_clear {
            let targets = self.fields.targets.clone();
            if let Err(e) = self.edit_descriptive(&targets, DescriptiveEdit::ClearLocation) {
                self.messages
                    .fail(format!("Location could not be cleared: {e}"));
            }
        }
    }
    /// The Keywording panel: the keywords of the photos shown, one per row
    /// (with "*" when only some have it), and a field to add more.
    pub(super) fn keyword_fields(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(ui.ctx());
        self.sync_fields();
        let mut remove = None;
        for (keyword, all) in &self.fields.keywords {
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), egui::Sense::hover());
            let name = format!(
                "{}{}",
                keyword.path.join(" > "),
                if *all { "" } else { " *" }
            );
            let button = egui::Rect::from_min_max(
                egui::pos2(rect.right() - 20., rect.top()),
                rect.right_bottom(),
            );
            paint_truncated(
                ui,
                rect.left_center(),
                button.left() - rect.left(),
                &name,
                palette.tone(VALUE_GRAY),
            );
            let tip = if *all {
                "Remove this keyword".to_string()
            } else {
                "Some of the selected photos have it (*); remove it from them".to_string()
            };
            if ui
                .put(
                    button,
                    egui::Button::new(egui::RichText::new("×").size(text::LABEL)).frame(false),
                )
                .on_hover_text(tip)
                .clicked()
            {
                remove = Some(keyword.id);
            }
        }
        let targets = self.fields.targets.clone();
        if let Some(keyword) = remove
            && let Err(e) = self.edit_descriptive(&targets, DescriptiveEdit::RemoveKeyword(keyword))
        {
            self.messages
                .fail(format!("Keyword could not be removed: {e}"));
        }
        ui.add_space(space::XS);
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), egui::Sense::hover());
        let response = ui.add_enabled(!targets.is_empty(), |ui: &mut egui::Ui| {
            ui.put(
                rect.shrink2(Vec2::new(0., 1.)),
                panel_edit(
                    &palette,
                    egui::TextEdit::singleline(&mut self.fields.keyword_entry),
                )
                .hint_text("Add keywords: Child < Parent, …")
                .vertical_align(egui::Align::Center),
            )
        });
        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            let added = parse_keywords(&self.fields.keyword_entry).and_then(|paths| {
                self.edit_descriptive(&targets, DescriptiveEdit::AddKeywords(paths))
            });
            match added {
                Ok(()) => self.fields.keyword_entry.clear(),
                Err(e) => self.messages.fail(format!("Keywords not added: {e}")),
            }
            response.request_focus();
        }
    }
}

fn edit<'t>(
    palette: &theme::Palette,
    text: &'t mut String,
    shared: &Shared<String>,
    multiline: bool,
) -> egui::TextEdit<'t> {
    let edit = if multiline {
        egui::TextEdit::multiline(text).desired_rows(3)
    } else {
        egui::TextEdit::singleline(text)
    };
    panel_edit(palette, edit).hint_text(if *shared == Shared::Mixed { MIXED } else { "" })
}
/// What a text row's field did this frame.
struct RowInput {
    /// Focus left it, to save it.
    left: bool,
    typed: bool,
}
/// A text row. The caption is a fixed three lines that scroll, so the panel
/// never reflows.
fn text_row(
    ui: &mut egui::Ui,
    key: &str,
    text: &mut String,
    shared: &Shared<String>,
    multiline: bool,
) -> RowInput {
    let palette = theme::palette(ui.ctx());
    let height = if multiline { CAPTION } else { ROW };
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), height),
        egui::Sense::hover(),
    );
    caption_at(ui, rect, key);
    let field = field_rect(rect);
    let response = if multiline {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(field));
        egui::ScrollArea::vertical()
            .id_salt(("metadata-field", key))
            .max_height(field.height())
            .show(&mut child, |ui| {
                ui.add_sized(
                    Vec2::new(field.width(), field.height()),
                    edit(&palette, text, shared, true),
                )
            })
            .inner
    } else {
        ui.put(
            field,
            edit(&palette, text, shared, false).vertical_align(egui::Align::Center),
        )
    };
    RowInput {
        left: response.lost_focus(),
        typed: response.changed(),
    }
}
/// What the creator rows did this frame.
struct CreatorInput {
    /// The list changed or an entry was left, to save it.
    commit: bool,
    /// "+" added an entry.
    added: bool,
    typed: bool,
}
/// The button clicked on a creator row.
enum CreatorButton {
    /// "+" on the last row.
    Add,
    /// "−" on the row at this index.
    Remove(usize),
}
/// One row per creator, each removable, and a button to add one.
fn creator_rows(
    ui: &mut egui::Ui,
    names: &mut Vec<String>,
    shared: &Shared<Vec<String>>,
) -> CreatorInput {
    let palette = theme::palette(ui.ctx());
    let mut commit = false;
    let mut typed = false;
    let mut clicked = None;
    // Always a row, so the panel keeps its shape with nothing selected.
    if names.is_empty() {
        names.push(String::new());
    }
    let count = names.len();
    for (i, name) in names.iter_mut().enumerate() {
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), egui::Sense::hover());
        if i == 0 {
            caption_at(ui, rect, "Creator");
        }
        let field = field_rect(rect);
        let button = egui::Rect::from_min_max(
            egui::pos2(field.right() - 18., field.top()),
            field.right_bottom(),
        );
        let text =
            egui::Rect::from_min_max(field.min, egui::pos2(button.left() - 2., field.bottom()));
        let hint = if *shared == Shared::Mixed && count == 1 {
            MIXED
        } else {
            ""
        };
        let response = ui.put(
            text,
            panel_edit(&palette, egui::TextEdit::singleline(name))
                .hint_text(hint)
                .vertical_align(egui::Align::Center),
        );
        commit |= response.lost_focus();
        typed |= response.changed();
        let last = i + 1 == count;
        let (label, tip) = if last {
            ("+", "Add a creator")
        } else {
            ("−", "Remove this creator")
        };
        if ui
            .put(
                button,
                egui::Button::new(egui::RichText::new(label).size(text::LABEL)).frame(false),
            )
            .on_hover_text(tip)
            .clicked()
        {
            clicked = Some(if last {
                CreatorButton::Add
            } else {
                CreatorButton::Remove(i)
            });
        }
    }
    let mut added = false;
    match clicked {
        // "+" adds an empty entry to type in, saved when it is left.
        Some(CreatorButton::Add) => {
            names.push(String::new());
            added = true;
        }
        Some(CreatorButton::Remove(i)) => {
            names.remove(i);
            commit = true;
        }
        None => {}
    }
    CreatorInput {
        commit,
        added,
        typed,
    }
}
