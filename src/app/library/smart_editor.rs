//! Create Smart Collection and Edit Smart Collection, as Lightroom's
//! dialog: a name, an optional collection set, "Match all / any / none of
//! the following rules" and the rules, each a field, an operation and its
//! value. Option-click (Alt on Windows and Linux) on a rule's + adds a nested
//! group ("All / Any / None of the following are true"), as Lightroom's
//! does. Only fields RAWmakase evaluates are offered (`collection_rules::fields`);
//! a rule it can't evaluate, imported from Lightroom, is shown read-only and
//! can be removed. Values are kept as Lightroom stores them.
use super::Library;
use crate::app::ui::design::{Tone, button, inset, space, text};
use crate::app::ui::{design, theme, widgets};
use crate::catalog::{CollectionId, CollectionKind, SmartRules};
use crate::collection_rules::fields::{
    self, FIELDS, Input, PERIOD_UNITS, aperture_apex, aperture_text, field_of, shutter_apex,
    shutter_text,
};
use crate::collection_rules::{Combine, Group, Item, Rule, lua::Value};
use eframe::egui;

/// The dialog while it is open.
pub(super) struct SmartEditor {
    /// The smart collection edited; None when creating one.
    pub(super) editing: Option<CollectionId>,
    pub(super) name: String,
    pub(super) inside: bool,
    pub(super) parent: Option<CollectionId>,
    pub(super) rules: Group,
    /// Why the rules couldn't be read: they are shown as Lightroom's text,
    /// and saving replaces them.
    pub(super) unreadable: Option<(String, String)>,
    pub(super) error: Option<String>,
}

/// What a click in the rules asked for.
enum Change {
    Remove(Vec<usize>),
    /// Add a rule (or, with Option, a nested group) after this one.
    AddAfter(Vec<usize>, bool),
}

impl SmartEditor {
    fn new_rule(today: &str) -> Rule {
        fields::FIELDS[0].new_rule(today)
    }
}

impl Library {
    /// Create Smart Collection…: an empty dialog, in the set shown when one is.
    pub(in crate::app) fn new_smart_collection(&mut self) {
        let today = today();
        self.smart_editor = Some(SmartEditor {
            editing: None,
            name: "Smart Collection".into(),
            inside: false,
            parent: None,
            rules: Group::new(
                Combine::Intersect,
                vec![Item::Rule(SmartEditor::new_rule(&today))],
            ),
            unreadable: None,
            error: None,
        });
    }
    /// Edit Smart Collection… for `id`.
    pub(in crate::app) fn edit_smart_collection(&mut self, id: CollectionId) {
        let Some(collection) = self
            .session
            .collections()
            .iter()
            .find(|c| c.id == id && c.kind == CollectionKind::Smart)
        else {
            return;
        };
        let stored = self.session.smart_rules_of(id);
        let (rules, unreadable) = match stored.map(SmartRules::rules) {
            Some(Ok(rules)) => (rules, None),
            Some(Err(why)) => (
                Group::new(Combine::Intersect, Vec::new()),
                Some((
                    why.to_string(),
                    stored
                        .and_then(SmartRules::lightroom)
                        .unwrap_or_default()
                        .to_string(),
                )),
            ),
            None => (Group::new(Combine::Intersect, Vec::new()), None),
        };
        self.smart_editor = Some(SmartEditor {
            editing: Some(id),
            name: collection.name.clone(),
            inside: collection.parent.is_some(),
            parent: collection.parent,
            rules,
            unreadable,
            error: None,
        });
    }
    pub(in crate::app) fn smart_editor_open(&self) -> bool {
        self.smart_editor.is_some()
    }
    /// The dialog, while it is open.
    pub(in crate::app) fn smart_editor_window(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.smart_editor.take() else {
            return;
        };
        let palette = theme::palette(ctx);
        let today = today();
        let sets: Vec<(CollectionId, String)> = self
            .session
            .collections()
            .iter()
            .filter(|c| c.kind == CollectionKind::Set)
            .map(|c| (c.id, c.name.clone()))
            .collect();
        let mut save = false;
        let mut cancel = false;
        let alt = ctx.input(|i| i.modifiers.alt);
        let response = egui::Modal::new(egui::Id::new("smart-collection-editor"))
            .frame(widgets::modal_frame(&palette).inner_margin(design::margin_same(inset::DIALOG)))
            .show(ctx, |ui| {
                ui.set_width(640.);
                ui.label(
                    egui::RichText::new(if dialog.editing.is_some() {
                        "Edit Smart Collection"
                    } else {
                        "Create Smart Collection"
                    })
                    .size(text::TITLE)
                    .color(palette.tone(Tone::STRONG)),
                );
                ui.add_space(space::M);
                // Renaming and moving are the Collections panel's, as for any
                // collection; here the rules change.
                let creating = dialog.editing.is_none();
                ui.horizontal(|ui| {
                    ui.label("Name:");
                    ui.add_enabled(
                        creating,
                        egui::TextEdit::singleline(&mut dialog.name).desired_width(300.),
                    );
                });
                ui.add_enabled_ui(creating, |ui| {
                    widgets::dropdown_row(ui, |ui| {
                        ui.add_enabled(
                            !sets.is_empty(),
                            egui::Checkbox::new(&mut dialog.inside, "Inside a Collection Set"),
                        );
                        if dialog.inside {
                            let chosen = dialog
                                .parent
                                .and_then(|p| sets.iter().find(|(id, _)| *id == p))
                                .map_or("Choose a set", |(_, name)| name.as_str());
                            widgets::dropdown(ui, "smart-editor-set")
                                .selected_text(chosen)
                                .show_ui(ui, |ui| {
                                    for (id, name) in &sets {
                                        ui.selectable_value(&mut dialog.parent, Some(*id), name);
                                    }
                                });
                        }
                    })
                });
                ui.add_space(space::M);
                if let Some((why, lightroom)) = &dialog.unreadable {
                    ui.label(
                        egui::RichText::new(format!(
                            "These rules can't be read ({why}). They are kept as Lightroom wrote \
                             them until you save new ones:"
                        ))
                        .size(text::LABEL)
                        .color(palette.tone(Tone::MUTED)),
                    );
                    ui.add(
                        egui::Label::new(egui::RichText::new(lightroom).monospace().small()).wrap(),
                    );
                    ui.add_space(space::S);
                }
                widgets::dropdown_row(ui, |ui| {
                    ui.label("Match");
                    combine_menu(
                        ui,
                        egui::Id::new("smart-editor-match"),
                        &mut dialog.rules.combine,
                        false,
                    );
                    ui.label("of the following rules:");
                });
                ui.add_space(space::S);
                let mut change = None;
                egui::ScrollArea::vertical()
                    .max_height(360.)
                    .show(ui, |ui| {
                        group_rows(
                            ui,
                            &mut dialog.rules,
                            &mut Vec::new(),
                            &today,
                            alt,
                            &mut change,
                        );
                    });
                if let Some(change) = change {
                    apply(&mut dialog.rules, change, &today);
                }
                if dialog.rules.items.is_empty() && ui.button("+ Add a rule").clicked() {
                    dialog
                        .rules
                        .items
                        .push(Item::Rule(SmartEditor::new_rule(&today)));
                }
                if let Some(why) = dialog
                    .rules
                    .unsupported()
                    .filter(|_| !dialog.rules.items.is_empty())
                {
                    ui.label(
                        egui::RichText::new(format!(
                            "A rule can't be evaluated ({why}): the collection lists no photos \
                             until it is removed."
                        ))
                        .size(text::LABEL)
                        .color(palette.tone(Tone::SECONDARY)),
                    );
                }
                if let Some(error) = &dialog.error {
                    ui.label(
                        egui::RichText::new(error)
                            .size(text::LABEL)
                            .color(palette.tone(Tone::SECONDARY)),
                    );
                }
                ui.add_space(space::XL);
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding = button::DIALOG;
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    let ready = !dialog.name.trim().is_empty()
                        && (!dialog.inside || dialog.parent.is_some());
                    let label = if dialog.editing.is_some() {
                        "Save"
                    } else {
                        "Create"
                    };
                    save = ui
                        .add_enabled_ui(ready, |ui| widgets::primary_button(ui, label))
                        .inner
                        .clicked();
                });
            });
        if cancel || response.should_close() {
            return;
        }
        if save {
            match self.save_smart_editor(&dialog) {
                Ok(()) => return,
                Err(e) => dialog.error = Some(format!("{e:#}")),
            }
        }
        self.smart_editor = Some(dialog);
    }
    /// Creates the smart collection, or saves its new rules (one step for
    /// Undo), and shows it. Creating one is undone with the Collections
    /// panel's other changes to the collection tree.
    pub(super) fn save_smart_editor(&mut self, dialog: &SmartEditor) -> anyhow::Result<()> {
        // Values typed here must read; rules kept read-only as imported stay.
        if let Some(why) = dialog
            .rules
            .rules()
            .into_iter()
            .filter(|r| fields::editable(r))
            .find_map(fields::invalid)
        {
            anyhow::bail!(why);
        }
        let parent = dialog.parent.filter(|_| dialog.inside);
        let rules = SmartRules::new(&dialog.rules);
        let id = match dialog.editing {
            None => {
                let place_before = self.place();
                let id = self
                    .session
                    .create_smart_collection(&dialog.name, parent, &rules)?;
                // One step for Undo, as Create Collection is.
                self.record_smart_created(id, place_before);
                id
            }
            Some(id) => {
                let before = self.session.smart_rules_of(id).cloned();
                self.session.set_smart_rules(id, &rules)?;
                self.record_smart_rules(id, &dialog.name, before, rules);
                id
            }
        };
        self.sidebar_trees = None;
        self.select_collection(id);
        Ok(())
    }
}

impl Library {
    /// Records new rules of smart collection `id` for Undo.
    fn record_smart_rules(
        &mut self,
        id: CollectionId,
        name: &str,
        before: Option<SmartRules>,
        after: SmartRules,
    ) {
        let summary = format!("Edit Smart Collection “{}”", name.trim());
        self.messages.note(summary.clone());
        self.smart_done.push(SmartRulesCommand {
            sequence: crate::edit_session::sequence(),
            collection: id,
            before,
            after,
            place: self.place(),
            summary,
        });
    }
    /// The rule changes made since the last call, for the undo log.
    pub(in crate::app) fn take_smart_done(&mut self) -> Vec<SmartRulesCommand> {
        std::mem::take(&mut self.smart_done)
    }
    /// Undo and Redo of a rule change: `rules` back on smart collection `id`.
    pub(in crate::app) fn put_back_smart_rules(
        &mut self,
        id: CollectionId,
        rules: Option<&SmartRules>,
    ) -> anyhow::Result<()> {
        match rules {
            Some(rules) => self.session.set_smart_rules(id, rules),
            // An older release's import kept none.
            None => self.session.clear_smart_rules(id),
        }
    }
}

/// A smart collection's rules changed in Edit Smart Collection, for Undo.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) struct SmartRulesCommand {
    /// Orders it among other changes made in the same frame.
    pub sequence: u64,
    pub collection: CollectionId,
    pub before: Option<SmartRules>,
    pub after: SmartRules,
    pub place: super::Place,
    pub summary: String,
}

/// Today's local date as rules write dates.
fn today() -> String {
    let (y, m, d) = crate::platform::clock::clock().today.ymd();
    format!("{y:04}-{m:02}-{d:02}")
}

/// The all / any / none menu of a group.
fn combine_menu(ui: &mut egui::Ui, id: egui::Id, combine: &mut Combine, nested: bool) {
    let label = |c: Combine| match (c, nested) {
        (Combine::Intersect, false) => "all",
        (Combine::Union, false) => "any",
        (Combine::Exclude, false) => "none",
        (Combine::Intersect, true) => "All",
        (Combine::Union, true) => "Any",
        (Combine::Exclude, true) => "None",
    };
    widgets::keep_style(ui, |ui| {
        widgets::dropdown(ui, id)
            .selected_text(label(*combine))
            .show_ui(ui, |ui| {
                for c in [Combine::Intersect, Combine::Union, Combine::Exclude] {
                    ui.selectable_value(combine, c, label(c));
                }
            });
    });
}

/// The rows of `group`, nested groups indented; `path` is where the group
/// is (item indices from the top).
fn group_rows(
    ui: &mut egui::Ui,
    group: &mut Group,
    path: &mut Vec<usize>,
    today: &str,
    alt: bool,
    change: &mut Option<Change>,
) {
    for (at, item) in group.items.iter_mut().enumerate() {
        path.push(at);
        match item {
            Item::Rule(rule) => {
                widgets::dropdown_row(ui, |ui| {
                    ui.add_space(indent(path));
                    rule_row(ui, rule, path, today);
                    buttons(ui, path, alt, change);
                });
            }
            Item::Group(inner) => {
                widgets::dropdown_row(ui, |ui| {
                    ui.add_space(indent(path));
                    combine_menu(
                        ui,
                        egui::Id::new(("smart-editor-group", path.clone())),
                        &mut inner.combine,
                        true,
                    );
                    ui.label("of the following are true");
                    buttons(ui, path, alt, change);
                });
                group_rows(ui, inner, path, today, alt, change);
            }
        }
        path.pop();
    }
}

/// How far in a row at `path` starts: a step for each group it is in.
fn indent(path: &[usize]) -> f32 {
    path.len().saturating_sub(1) as f32 * space::XXL
}

/// A row's − and +.
fn buttons(ui: &mut egui::Ui, path: &[usize], alt: bool, change: &mut Option<Change>) {
    if ui.small_button("−").on_hover_text("Remove").clicked() {
        *change = Some(Change::Remove(path.to_vec()));
    }
    let hint = "Add a rule; with ⌥ (Alt), a group of rules";
    if ui.small_button("+").on_hover_text(hint).clicked() {
        *change = Some(Change::AddAfter(path.to_vec(), alt));
    }
}

fn apply(group: &mut Group, change: Change, today: &str) {
    let (path, add) = match change {
        Change::Remove(path) => (path, None),
        Change::AddAfter(path, nested) => {
            let rule = Item::Rule(SmartEditor::new_rule(today));
            let item = if nested {
                Item::Group(Group::new(Combine::Union, vec![rule]))
            } else {
                rule
            };
            (path, Some(item))
        }
    };
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut target = group;
    for at in parents {
        let Some(Item::Group(inner)) = target.items.get_mut(*at) else {
            return;
        };
        target = inner;
    }
    match add {
        Some(item) => target.items.insert(last + 1, item),
        None => {
            if *last < target.items.len() {
                target.items.remove(*last);
            }
        }
    }
}

/// A rule's field, operation and value; a rule the editor doesn't offer is
/// shown as Lightroom names it, read-only.
fn rule_row(ui: &mut egui::Ui, rule: &mut Rule, path: &[usize], today: &str) {
    let field = field_of(&rule.criteria);
    let (Some(field), true) = (field, fields::editable(rule)) else {
        ui.label(
            egui::RichText::new(read_only(rule)).color(theme::palette(ui.ctx()).tone(Tone::MUTED)),
        );
        return;
    };
    let id = |part: &str| egui::Id::new(("smart-editor", part, path.to_vec()));
    // The field menu, in Lightroom's sections.
    let mut chosen = None;
    widgets::keep_style(ui, |ui| {
        widgets::dropdown(ui, id("field"))
            .selected_text(field.name)
            .width(170.)
            .show_ui(ui, |ui| {
                let mut section = None;
                for f in FIELDS {
                    if f.section != section {
                        section = f.section;
                        if let Some(name) = section {
                            ui.label(egui::RichText::new(name).small().strong());
                        }
                    }
                    if ui
                        .selectable_label(f.criteria == field.criteria, f.name)
                        .clicked()
                    {
                        chosen = Some(f);
                    }
                }
            });
    });
    if let Some(f) = chosen.filter(|f| f.criteria != field.criteria) {
        *rule = f.new_rule(today);
        return;
    }
    let Some(operation) = field.operation(&rule.operation) else {
        return;
    };
    let mut op_chosen = None;
    widgets::keep_style(ui, |ui| {
        widgets::dropdown(ui, id("op"))
            .selected_text(operation.label)
            .width(170.)
            .show_ui(ui, |ui| {
                for o in field.operations {
                    if ui.selectable_label(o.op == operation.op, o.label).clicked() {
                        op_chosen = Some(o);
                    }
                }
            });
    });
    if let Some(o) = op_chosen.filter(|o| o.op != operation.op) {
        fields::set_operation(rule, o, today);
        return;
    }
    value_input(ui, rule, operation.input, &id);
}

/// A rule as Lightroom names it, with its value, for one the editor can't
/// show as menus.
fn read_only(rule: &Rule) -> String {
    let name = crate::collection_rules::criteria_name(&rule.criteria).unwrap_or(&rule.criteria);
    let value = |v: &Option<Value>| match v {
        Some(Value::Str(s)) => format!(" \"{s}\""),
        Some(Value::Number(n)) => format!(" {}", crate::collection_rules::lua::number(*n)),
        Some(Value::Bool(b)) => format!(" {b}"),
        Some(Value::Table(_)) => " …".into(),
        None => String::new(),
    };
    format!(
        "{name} {}{}{} (can't be evaluated here)",
        rule.operation,
        value(&rule.value),
        if rule.operation == "in" {
            value(&rule.value2)
        } else {
            String::new()
        }
    )
}

fn value_input(ui: &mut egui::Ui, rule: &mut Rule, input: Input, id: &dyn Fn(&str) -> egui::Id) {
    match input {
        Input::None => {}
        Input::Range(kind) => {
            single_input(ui, &mut rule.value, kind.input(), id("from"));
            ui.label("to");
            single_input(ui, &mut rule.value2, kind.input(), id("to"));
        }
        Input::Period => {
            number_value(ui, &mut rule.value);
            let units = rule.units.clone().unwrap_or_else(|| "days".into());
            widgets::keep_style(ui, |ui| {
                widgets::dropdown(ui, id("units"))
                    .selected_text(units.clone())
                    .show_ui(ui, |ui| {
                        for (unit, label) in PERIOD_UNITS {
                            if ui.selectable_label(units == unit, label).clicked() {
                                rule.units = Some(unit.into());
                            }
                        }
                    });
            });
        }
        Input::Choice(choices) => {
            let current = choices
                .iter()
                .find(|(_, c)| rule.value.as_ref() == Some(&c.value()))
                .map_or("", |(label, _)| *label);
            widgets::keep_style(ui, |ui| {
                widgets::dropdown(ui, id("choice"))
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        for (label, choice) in choices {
                            if ui.selectable_label(*label == current, *label).clicked() {
                                rule.value = Some(choice.value());
                            }
                        }
                    });
            });
        }
        single => single_input(ui, &mut rule.value, single, id("value")),
    }
}

/// One value of a kind a range has two of.
fn single_input(ui: &mut egui::Ui, value: &mut Option<Value>, input: Input, id: egui::Id) {
    match input {
        Input::Text => text_value(ui, value, 160.),
        Input::Date => text_value(ui, value, 90.),
        Input::Number => number_value(ui, value),
        Input::Rating => {
            let stars = match value {
                Some(Value::Number(n)) => n.clamp(0., 5.) as usize,
                _ => 0,
            };
            let label = |n: usize| {
                if n == 0 {
                    "no stars".to_string()
                } else {
                    "★".repeat(n)
                }
            };
            widgets::keep_style(ui, |ui| {
                widgets::dropdown(ui, id)
                    .selected_text(label(stars))
                    .show_ui(ui, |ui| {
                        for n in 0..=5 {
                            if ui.selectable_label(n == stars, label(n)).clicked() {
                                *value = Some(Value::Number(n as f64));
                            }
                        }
                    });
            });
        }
        Input::Shutter => converted(ui, value, id, shutter_text, shutter_apex, "sec"),
        Input::Aperture => converted(ui, value, id, aperture_text, aperture_apex, ""),
        _ => {}
    }
}

fn text_value(ui: &mut egui::Ui, value: &mut Option<Value>, width: f32) {
    let mut text = match value {
        Some(Value::Str(s)) => s.clone(),
        _ => String::new(),
    };
    if ui
        .add(widgets::dropdown_text_edit(&mut text).desired_width(width))
        .changed()
    {
        *value = Some(Value::Str(text));
    }
}

fn number_value(ui: &mut egui::Ui, value: &mut Option<Value>) {
    let mut n = match value {
        Some(Value::Number(n)) => *n,
        _ => 0.,
    };
    if ui.add(egui::DragValue::new(&mut n).speed(1.)).changed() {
        *value = Some(Value::Number(n));
    }
}

/// A value shown one way and stored another (a shutter speed's "1/250" as
/// its APEX value): what is typed is kept while it doesn't read yet.
fn converted(
    ui: &mut egui::Ui,
    value: &mut Option<Value>,
    id: egui::Id,
    show: fn(f64) -> String,
    read: fn(&str) -> Option<f64>,
    unit: &str,
) {
    let stored = match value {
        Some(Value::Number(n)) => show(*n),
        _ => String::new(),
    };
    let mut draft: String = ui.data(|d| d.get_temp(id)).unwrap_or(stored);
    let response = ui.add(widgets::dropdown_text_edit(&mut draft).desired_width(80.));
    if response.changed()
        && let Some(n) = read(&draft)
    {
        *value = Some(Value::Number(n));
    }
    if response.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, draft));
    } else {
        ui.data_mut(|d| d.remove::<String>(id));
    }
    if !unit.is_empty() {
        ui.label(unit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rating(n: f64) -> Item {
        Item::Rule(Rule::new("rating", "==", Some(Value::Number(n))))
    }

    #[test]
    fn plus_adds_after_and_option_plus_adds_a_nested_group() {
        let mut group = Group::new(Combine::Intersect, vec![rating(1.), rating(2.)]);
        apply(&mut group, Change::AddAfter(vec![0], true), "2026-11-17");
        assert!(
            matches!(&group.items[1], Item::Group(g) if g.combine == Combine::Union && g.items.len() == 1)
        );
        apply(
            &mut group,
            Change::AddAfter(vec![1, 0], false),
            "2026-11-17",
        );
        let Item::Group(inner) = &group.items[1] else {
            panic!("a group");
        };
        assert_eq!(inner.items.len(), 2);
        apply(&mut group, Change::Remove(vec![1]), "2026-11-17");
        assert_eq!(group.items, [rating(1.), rating(2.)]);
        // A path that is gone changes nothing.
        apply(&mut group, Change::Remove(vec![5, 0]), "2026-11-17");
        assert_eq!(group.items.len(), 2);
    }
}
