//! The Keywording panel's Keyword Set, as Lightroom's: a menu of Recent
//! Keywords and the keyword sets, and nine buttons, laid out as a numeric
//! keypad (7 8 9 on top). A button shows whether the photos the panel edits
//! have its keyword; clicking it removes the keyword when all of them have
//! it, else adds it to all. ⌥1–⌥9 click them and ⌥0 / ⌥⇧0 go to the next or
//! previous set. A set's slots are typed keywords, found as typed ones are;
//! Recent Keywords' are the keywords themselves.
use super::Library;
use super::descriptive::{DescriptiveEdit, parse_keyword};
use crate::app::ui::design::{self, Tone, button, inset, space, text};
use crate::app::ui::theme;
use crate::app::ui::widgets::{modal_frame, primary_button};
use crate::catalog::ListedKeyword;
use crate::keyword_sets::{KeywordSet, KeywordSetList, KeywordSets, Origin, SLOTS};
use crate::metadata::keyword_key;
use eframe::egui::{self, Color32, Vec2};
use std::collections::HashMap;

/// The slots, top row first, as a numeric keypad lays them out.
pub(super) const KEYPAD: [usize; SLOTS] = [6, 7, 8, 3, 4, 5, 0, 1, 2];

/// What the buttons hold.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum Shown {
    /// The catalog's latest keywords.
    #[default]
    Recent,
    /// The set of this name, as saved.
    Set(String),
    /// Slots changed in Edit Set, from the set named if there was one.
    Edited {
        from: Option<String>,
        slots: Box<[String; SLOTS]>,
    },
}

/// The section's state, kept while RAWmakase runs; what it shows is kept
/// with the Library layout.
#[derive(Default)]
pub(super) struct KeywordSetPanel {
    store: KeywordSets,
    /// The sets, read when first needed and after each change.
    list: Option<KeywordSetList>,
    pub(super) shown: Shown,
    dialog: Option<Dialog>,
}

/// A window the section opened.
enum Dialog {
    /// Save Current Settings as New Preset, with the name typed.
    SaveAs {
        name: String,
        focused: bool,
    },
    Rename {
        from: String,
        name: String,
        focused: bool,
    },
    Delete(String),
    Edit([String; SLOTS]),
}

/// What a slot is, to show and to click.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Slot {
    Empty,
    /// A typed keyword: its path, top first, and the keyword it names now.
    Typed {
        label: String,
        path: Vec<String>,
        found: Option<i64>,
    },
    /// One of Recent Keywords; `typeable` when its label, typed, finds it.
    Keyword {
        label: String,
        id: i64,
        typeable: bool,
    },
}

/// The shortest path naming keyword `id` alone, as Recent Keywords shows
/// it: its name, else `Name < Parent`, and so on up; longer still where a
/// keyword typed so would be found as another, so a set saved from it keeps
/// it. A top-level keyword whose name an older one in the same case has
/// can't be typed apart from it, as in Lightroom, nor can one whose name
/// has "<", ">" or "|": for those, the whole path and false, as a set can't
/// keep them.
pub(super) fn shortest_label(keywords: &[ListedKeyword], id: i64) -> (String, bool) {
    let by_id: HashMap<i64, &ListedKeyword> = keywords.iter().map(|k| (k.id, k)).collect();
    // Each keyword's names, its own first, ignoring case.
    let path_of = |id: i64| {
        let mut names = Vec::new();
        let mut next = Some(id);
        while let Some(k) = next.and_then(|n| by_id.get(&n))
            && names.len() < 64
        {
            names.push(&k.name);
            next = k.parent;
        }
        names
    };
    let own = path_of(id);
    if own.is_empty() {
        return (String::new(), false);
    }
    let key = |names: &[&String]| -> Vec<String> { names.iter().map(|n| keyword_key(n)).collect() };
    let others: Vec<Vec<String>> = keywords
        .iter()
        .filter(|o| o.id != id && keyword_key(&o.name) == keyword_key(own[0]))
        .map(|o| key(&path_of(o.id)))
        .collect();
    let own_key = key(&own);
    let unique = (1..=own.len())
        .find(|&depth| {
            !others
                .iter()
                .any(|o| o.len() >= depth && o[..depth] == own_key[..depth])
        })
        .unwrap_or(own.len());
    let label = |depth: usize| {
        own[..depth]
            .iter()
            .map(|n| n.as_str())
            .collect::<Vec<_>>()
            .join(" < ")
    };
    // Read back as a set's slot is: a name with "<", ">" or "|" (from
    // Lightroom or a file) reads as a hierarchy, and finds another keyword.
    let found = (unique..=own.len()).find(|&depth| {
        parse_keyword(&label(depth))
            .is_ok_and(|typed| crate::catalog::find_typed_keyword(keywords, &typed) == Some(id))
    });
    match found {
        Some(depth) => (label(depth), true),
        None => (label(own.len()), false),
    }
}

impl KeywordSetPanel {
    /// The sets, read from their folders the first time.
    fn sets(&mut self) -> &KeywordSetList {
        self.list.get_or_insert_with(|| self.store.list())
    }
    /// What the section shows, as the layout keeps it: the set's name, or
    /// none for Recent Keywords; and the slots when they were edited.
    pub(super) fn kept(&self) -> (Option<String>, Vec<String>) {
        match &self.shown {
            Shown::Recent => (None, Vec::new()),
            Shown::Set(name) => (Some(name.clone()), Vec::new()),
            Shown::Edited { from, slots } => (from.clone(), slots.to_vec()),
        }
    }
    /// Shows what a layout kept.
    pub(super) fn restore(&mut self, set: Option<String>, slots: &[String]) {
        self.shown = match (set, <[String; SLOTS]>::try_from(slots.to_vec())) {
            (from, Ok(slots)) => Shown::Edited {
                from,
                slots: Box::new(slots),
            },
            (Some(name), Err(_)) => Shown::Set(name),
            (None, Err(_)) => Shown::Recent,
        };
    }
    /// The set shown or edited from, when it is listed.
    fn set(&mut self) -> Option<KeywordSet> {
        let name = match &self.shown {
            Shown::Set(name) => name.clone(),
            Shown::Edited {
                from: Some(name), ..
            } => name.clone(),
            _ => return None,
        };
        self.sets().get(&name).cloned()
    }
    /// The slots' typed keywords, for a set or edited slots.
    fn typed_slots(&mut self) -> Option<[String; SLOTS]> {
        match self.shown.clone() {
            Shown::Recent => None,
            Shown::Set(_) => Some(self.set().map(|s| s.keywords).unwrap_or_default()),
            Shown::Edited { slots, .. } => Some(*slots),
        }
    }
    /// The name the menu shows.
    fn title(&self) -> String {
        match &self.shown {
            Shown::Recent => "Recent Keywords".into(),
            Shown::Set(name) => name.clone(),
            Shown::Edited {
                from: Some(name), ..
            } => format!("{name} (edited)"),
            Shown::Edited { from: None, .. } => "Custom".into(),
        }
    }
    /// The next set after the one shown, or the one before; from Recent
    /// Keywords the first or the last, as Lightroom's ⌥0 and ⌥⇧0.
    pub(super) fn step(&mut self, forward: bool) {
        let names: Vec<String> = self.sets().sets.iter().map(|s| s.name.clone()).collect();
        if names.is_empty() {
            return;
        }
        let current = match &self.shown {
            Shown::Set(name)
            | Shown::Edited {
                from: Some(name), ..
            } => names.iter().position(|n| n.eq_ignore_ascii_case(name)),
            _ => None,
        };
        let n = names.len();
        let next = match (current, forward) {
            (None, true) => 0,
            (None, false) => n - 1,
            (Some(i), true) => (i + 1) % n,
            (Some(i), false) => (i + n - 1) % n,
        };
        self.shown = Shown::Set(names[next].clone());
    }
}

impl Library {
    /// The nine slots as they are now.
    pub(super) fn keyword_slots(&mut self) -> [Slot; SLOTS] {
        let mut slots: [Slot; SLOTS] = std::array::from_fn(|_| Slot::Empty);
        match self.keyword_sets.typed_slots() {
            None => {
                let keywords = self.session.keywords();
                // The latest three on the top row, as Lightroom lays them.
                for (i, id) in self.session.recent_keywords().iter().enumerate() {
                    if let Some(&slot) = KEYPAD.get(i) {
                        let (label, typeable) = shortest_label(keywords, *id);
                        slots[slot] = Slot::Keyword {
                            label,
                            id: *id,
                            typeable,
                        };
                    }
                }
            }
            Some(typed) => {
                for (slot, text) in slots.iter_mut().zip(typed) {
                    let text = text.trim();
                    if text.is_empty() {
                        continue;
                    }
                    let Ok(path) = parse_keyword(text) else {
                        continue;
                    };
                    // From the keywords read, not the catalog: this runs each frame.
                    let found = crate::catalog::find_typed_keyword(self.session.keywords(), &path);
                    *slot = Slot::Typed {
                        label: text.to_string(),
                        path,
                        found,
                    };
                }
            }
        }
        slots
    }
    /// Clicks slot `slot` (0 to 8) for the photos the Keywording panel edits.
    pub(in crate::app) fn click_keyword_slot(&mut self, slot: usize) {
        self.sync_fields();
        let targets = self.fields.targets.clone();
        if targets.is_empty() {
            return;
        }
        let Some(chosen) = self.keyword_slots().get(slot).cloned() else {
            return;
        };
        let (label, id) = match &chosen {
            Slot::Empty => return,
            Slot::Typed { label, found, .. } => (label.clone(), *found),
            Slot::Keyword { label, id, .. } => (label.clone(), Some(*id)),
        };
        let all = id.and_then(|id| self.fields.keyword_state(id)) == Some(true);
        let edit = match (all, chosen) {
            (true, _) => DescriptiveEdit::RemoveKeyword(id.expect("a keyword all have")),
            (false, Slot::Keyword { id, .. }) => DescriptiveEdit::AddKeyword(id),
            // Found again as typed, and made when missing, as Lightroom's.
            (false, Slot::Typed { path, .. }) => DescriptiveEdit::AddKeywords(vec![path]),
            (false, Slot::Empty) => return,
        };
        let verb = if all { "removed" } else { "added" };
        match self.edit_descriptive(&targets, edit) {
            Ok(()) => self.messages.note(format!("Keyword {verb}: {label}")),
            Err(e) => self.messages.fail(format!("Keyword not {verb}: {e:#}")),
        }
    }
    /// Shows the next keyword set, or the one before.
    pub(in crate::app) fn step_keyword_set(&mut self, forward: bool) {
        self.keyword_sets.step(forward);
        let title = self.keyword_sets.title();
        self.messages.note(format!("Keyword Set: {title}"));
    }
    /// Whether one of the section's windows is open.
    pub(in crate::app) fn keyword_set_dialog_open(&self) -> bool {
        self.keyword_sets.dialog.is_some()
    }
    /// The Keyword Set section, under the keyword field.
    pub(super) fn keyword_set_section(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(ui.ctx());
        ui.add_space(space::S);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Keyword Set")
                    .size(text::SMALL)
                    .color(palette.tone(Tone::MUTED)),
            );
            self.keyword_set_menu(ui);
        });
        ui.add_space(space::XS);
        let slots = self.keyword_slots();
        let enabled = !self.fields.targets.is_empty();
        let width = (ui.available_width() - 2. * space::XS) / 3.;
        let mut clicked = None;
        for row in KEYPAD.chunks(3) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = space::XS;
                for &slot in row {
                    let (label, id) = match &slots[slot] {
                        Slot::Empty => (String::new(), None),
                        Slot::Typed { label, found, .. } => (label.clone(), *found),
                        Slot::Keyword { label, id, .. } => (label.clone(), Some(*id)),
                    };
                    let state = id.and_then(|id| self.fields.keyword_state(id));
                    let mut button = egui::Button::selectable(
                        state.is_some(),
                        egui::RichText::new(if state == Some(false) {
                            format!("{label} *")
                        } else {
                            label.clone()
                        })
                        .size(text::SMALL),
                    )
                    .truncate();
                    if label.is_empty() {
                        button = button.frame(false);
                    }
                    let response = ui
                        .add_enabled_ui(enabled && !label.is_empty(), |ui| {
                            ui.add_sized(Vec2::new(width, 20.), button)
                        })
                        .inner
                        .on_hover_text(format!(
                            "{label}{} · ⌥{}",
                            match state {
                                Some(true) => ": remove it from the selected photos",
                                Some(false) => ": some selected photos have it; add it to all",
                                None => "",
                            },
                            slot + 1
                        ));
                    if response.clicked() {
                        clicked = Some(slot);
                    }
                }
            });
        }
        if let Some(slot) = clicked {
            self.click_keyword_slot(slot);
        }
    }
    /// The section's menu: Recent Keywords, the sets, and what can be done
    /// with them, in Lightroom's order.
    fn keyword_set_menu(&mut self, ui: &mut egui::Ui) {
        let title = self.keyword_sets.title();
        let sets: Vec<KeywordSet> = self.keyword_sets.sets().sets.clone();
        let current = self.keyword_sets.set();
        let edited = matches!(self.keyword_sets.shown, Shown::Edited { .. });
        let slots = self.keyword_slots();
        egui::ComboBox::from_id_salt("keyword-set")
            .selected_text(egui::RichText::new(&title).size(text::SMALL))
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(self.keyword_sets.shown == Shown::Recent, "Recent Keywords")
                    .clicked()
                {
                    self.keyword_sets.shown = Shown::Recent;
                }
                ui.separator();
                if edited {
                    ui.add_enabled(false, egui::Button::selectable(true, &title));
                    ui.separator();
                }
                for set in &sets {
                    let on = self.keyword_sets.shown == Shown::Set(set.name.clone());
                    if ui.selectable_label(on, &set.name).clicked() {
                        self.keyword_sets.shown = Shown::Set(set.name.clone());
                    }
                }
                ui.separator();
                if ui.button("Save Current Settings as New Preset…").clicked() {
                    self.keyword_sets.dialog = Some(Dialog::SaveAs {
                        name: String::new(),
                        focused: false,
                    });
                }
                if let Some(set) = &current {
                    let lightroom = "Lightroom's own set: RAWmakase never changes it";
                    if edited {
                        if ui.button(format!("Update Preset “{}”", set.name)).clicked() {
                            let typed = slot_texts(&slots);
                            self.save_keyword_set(&set.name, &typed, true);
                        }
                    } else {
                        let own = set.origin == Origin::Own;
                        if ui
                            .add_enabled(
                                own,
                                egui::Button::new(format!("Delete preset “{}”…", set.name)),
                            )
                            .on_disabled_hover_text(lightroom)
                            .clicked()
                        {
                            self.keyword_sets.dialog = Some(Dialog::Delete(set.name.clone()));
                        }
                        if ui
                            .add_enabled(
                                own,
                                egui::Button::new(format!("Rename preset “{}”…", set.name)),
                            )
                            .on_disabled_hover_text(lightroom)
                            .clicked()
                        {
                            self.keyword_sets.dialog = Some(Dialog::Rename {
                                from: set.name.clone(),
                                name: set.name.clone(),
                                focused: false,
                            });
                        }
                    }
                }
                ui.separator();
                if ui.button("Edit Set…").clicked() {
                    self.keyword_sets.dialog = Some(Dialog::Edit(slot_texts(&slots)));
                }
            });
    }
    /// Saves `slots` as set `name`, replacing RAWmakase's own of that name
    /// when `replace`, and shows it.
    fn save_keyword_set(&mut self, name: &str, slots: &[String; SLOTS], replace: bool) {
        match self.keyword_sets.store.save(name, slots, replace) {
            Ok(_) => {
                self.keyword_sets.list = None;
                self.keyword_sets.shown = Shown::Set(name.trim().to_string());
                self.messages
                    .done(format!("Keyword set {} saved", name.trim()));
            }
            Err(e) => self.messages.fail(format!("Keyword set not saved: {e:#}")),
        }
    }
    /// The section's window, while one is open.
    pub(in crate::app) fn keyword_set_window(&mut self, ctx: &egui::Context) {
        let palette = theme::palette(ctx);
        let Some(dialog) = &mut self.keyword_sets.dialog else {
            return;
        };
        let mut done: Option<bool> = None;
        let response = egui::Modal::new(egui::Id::new("keyword-set-dialog"))
            .backdrop_color(Color32::from_black_alpha(140))
            .frame(modal_frame(&palette).inner_margin(design::margin_same(inset::DIALOG)))
            .show(ctx, |ui| {
                ui.set_width(420.);
                let (title, action) = match dialog {
                    Dialog::SaveAs { .. } => ("New Keyword Set", "Create"),
                    Dialog::Rename { .. } => ("Rename Keyword Set", "Rename"),
                    Dialog::Delete(_) => ("Delete Keyword Set", "Delete"),
                    Dialog::Edit(_) => ("Edit Keyword Set", "Change"),
                };
                ui.label(
                    egui::RichText::new(title)
                        .size(text::TITLE)
                        .color(palette.tone(Tone::STRONG)),
                );
                ui.add_space(space::L);
                let mut ready = true;
                match dialog {
                    Dialog::SaveAs { name, focused } | Dialog::Rename { name, focused, .. } => {
                        let field = ui.add(
                            egui::TextEdit::singleline(name)
                                .hint_text("Keyword set name")
                                .desired_width(f32::INFINITY),
                        );
                        if !*focused {
                            field.request_focus();
                            *focused = true;
                        }
                        ready = !name.trim().is_empty();
                        if ready
                            && field.lost_focus()
                            && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        {
                            done = Some(true);
                        }
                    }
                    Dialog::Delete(name) => {
                        ui.label(format!("Delete the preset named “{name}”?"));
                        ui.label(
                            egui::RichText::new("This operation cannot be undone.")
                                .size(text::SMALL)
                                .color(palette.tone(Tone::SECONDARY)),
                        );
                    }
                    Dialog::Edit(slots) => {
                        ui.label(
                            egui::RichText::new(
                                "Each slot takes one keyword, typed as in the Keywording \
                                 panel: Child < Parent.",
                            )
                            .size(text::SMALL)
                            .color(palette.tone(Tone::SECONDARY)),
                        );
                        ui.add_space(space::S);
                        egui::Grid::new("keyword-set-slots")
                            .num_columns(3)
                            .spacing(Vec2::splat(space::XS))
                            .show(ui, |ui| {
                                for (i, slot) in KEYPAD.iter().enumerate() {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut slots[*slot])
                                            .hint_text(format!("⌥{}", slot + 1))
                                            .desired_width(128.),
                                    );
                                    if i % 3 == 2 {
                                        ui.end_row();
                                    }
                                }
                            });
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
        if done == Some(true) {
            let dialog = self.keyword_sets.dialog.take();
            self.finish_keyword_set_dialog(dialog);
        } else if done == Some(false) {
            self.keyword_sets.dialog = None;
        }
    }
    fn finish_keyword_set_dialog(&mut self, dialog: Option<Dialog>) {
        match dialog {
            Some(Dialog::SaveAs { name, .. }) => {
                let slots = slot_texts(&self.keyword_slots());
                self.save_keyword_set(&name, &slots, false);
            }
            Some(Dialog::Rename { from, name, .. }) => {
                match self.keyword_sets.store.rename(&from, &name) {
                    Ok(()) => {
                        self.keyword_sets.list = None;
                        self.keyword_sets.shown = Shown::Set(name.trim().to_string());
                    }
                    Err(e) => self
                        .messages
                        .fail(format!("Keyword set not renamed: {e:#}")),
                }
            }
            Some(Dialog::Delete(name)) => match self.keyword_sets.store.delete(&name) {
                Ok(()) => {
                    self.keyword_sets.list = None;
                    // Lightroom's or the built-in set of that name, if any, or
                    // Recent Keywords.
                    let left = self.keyword_sets.sets().get(&name).is_some();
                    self.keyword_sets.shown = if left {
                        Shown::Set(name)
                    } else {
                        Shown::Recent
                    };
                }
                Err(e) => self
                    .messages
                    .fail(format!("Keyword set not deleted: {e:#}")),
            },
            Some(Dialog::Edit(slots)) => {
                let slots = slots.map(|s| s.trim().to_string());
                let set = self.keyword_sets.set();
                self.keyword_sets.shown = match set {
                    Some(set) if set.keywords == slots => Shown::Set(set.name),
                    set => Shown::Edited {
                        from: set.map(|s| s.name),
                        slots: Box::new(slots),
                    },
                };
            }
            None => {}
        }
    }
}

/// The slots as text, as a set saves them: Recent Keywords' as their
/// shortest paths, and empty for one no text finds again.
fn slot_texts(slots: &[Slot; SLOTS]) -> [String; SLOTS] {
    std::array::from_fn(|i| match &slots[i] {
        Slot::Empty
        | Slot::Keyword {
            typeable: false, ..
        } => String::new(),
        Slot::Typed { label, .. } | Slot::Keyword { label, .. } => label.clone(),
    })
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

    #[test]
    fn a_recent_keyword_s_label_finds_it_again_when_typed() {
        // "Dup" typed finds the older, nested one, in the same case.
        let list = [
            keyword(1, "A", None),
            keyword(2, "Dup", Some(1)),
            keyword(3, "dup", None),
            keyword(4, "B", None),
            keyword(5, "Dup", Some(4)),
        ];
        for id in [2, 3, 5] {
            let (label, typeable) = shortest_label(&list, id);
            assert!(typeable, "{label}");
            let path = crate::app::library::descriptive::parse_keyword(&label).unwrap();
            assert_eq!(
                crate::catalog::find_typed_keyword(&list, &path),
                Some(id),
                "{label}"
            );
        }
        assert_eq!(shortest_label(&list, 3).0, "dup");
        // A name that reads as a hierarchy is never saved as one that finds
        // another keyword.
        let list = [
            keyword(1, "A > B", None),
            keyword(2, "A", None),
            keyword(3, "B", Some(2)),
        ];
        assert_eq!(shortest_label(&list, 1), ("A > B".to_string(), false));
        let slots: [Slot; SLOTS] = std::array::from_fn(|i| match i {
            0 => Slot::Keyword {
                label: "A > B".into(),
                id: 1,
                typeable: false,
            },
            _ => Slot::Empty,
        });
        assert_eq!(slot_texts(&slots)[0], "");
    }

    #[test]
    fn recent_keywords_show_the_shortest_path_naming_them_alone() {
        let list = [
            keyword(1, "France", None),
            keyword(2, "Paris", Some(1)),
            keyword(3, "Texas", None),
            keyword(4, "Paris", Some(3)),
            keyword(5, "Rome", None),
        ];
        assert_eq!(shortest_label(&list, 2).0, "Paris < France");
        assert_eq!(shortest_label(&list, 4).0, "Paris < Texas");
        assert_eq!(shortest_label(&list, 5).0, "Rome");
        assert_eq!(shortest_label(&list, 1).0, "France");
    }

    #[test]
    fn sets_step_round_and_start_from_recent_keywords_at_either_end() {
        let d = tempfile::tempdir().unwrap();
        let mut panel = KeywordSetPanel {
            store: KeywordSets {
                dir: d.path().join("own"),
                lightroom: None,
            },
            ..Default::default()
        };
        panel.step(true);
        assert_eq!(panel.shown, Shown::Set("Outdoor Photography".into()));
        panel.step(false);
        assert_eq!(panel.shown, Shown::Set("Wedding Photography".into()));
        panel.step(true);
        assert_eq!(panel.shown, Shown::Set("Outdoor Photography".into()));
        panel.shown = Shown::Recent;
        panel.step(false);
        assert_eq!(panel.shown, Shown::Set("Wedding Photography".into()));
    }

    #[test]
    fn a_kept_layout_comes_back() {
        let mut panel = KeywordSetPanel::default();
        let slots: Box<[String; SLOTS]> = Box::new(std::array::from_fn(|i| format!("k{i}")));
        for shown in [
            Shown::Recent,
            Shown::Set("Festival".into()),
            Shown::Edited {
                from: Some("Festival".into()),
                slots: slots.clone(),
            },
            Shown::Edited { from: None, slots },
        ] {
            panel.shown = shown.clone();
            let (set, kept) = panel.kept();
            let mut other = KeywordSetPanel::default();
            other.restore(set, &kept);
            assert_eq!(other.shown, shown);
        }
    }
}
