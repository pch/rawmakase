//! Preferences > Shortcuts: every command's keys, grouped as the reference
//! card groups them, to search, change and reset. A key cap starts a capture:
//! the next key pressed, with its modifiers, replaces that key (Delete takes
//! it away, Escape keeps it). The keys belong to the user's keymap.toml
//! ([`crate::app::shell::keymap::file`]); a file that doesn't read leaves the
//! page read-only but for Reset All.
use super::{cards, gap, group, hint};
use crate::app::Editor;
use crate::app::shell::commands::registry::{COMMANDS, Command, bindings};
use crate::app::shell::keymap::conflicts::{Clash, clashes};
use crate::app::shell::keymap::file::{Saved, State};
use crate::app::shell::keymap::rules::{self, FIXED_KEYS};
use crate::app::shell::keymap::{
    Binding, Chord, Context, Effective, FrameKeys, Mods, Press, Tool, chord_of,
};
use crate::app::shell::shortcuts::{cap_label, search_matches, split_caps, tokens};
use crate::app::ui::design::{Tone, radius, space, text};
use crate::app::ui::icons::{self, Icon};
use crate::app::ui::theme;
use crate::app::ui::widgets::search_field;
use eframe::egui::{self, Key, Pos2, Rect, Sense, Stroke, Vec2};

/// What the page keeps between frames.
#[derive(Default)]
pub(in crate::app) struct Page {
    query: String,
    conflicts_only: bool,
    capture: Option<Capture>,
    /// Where the cap waiting for a key was last drawn: a press anywhere else
    /// gives up waiting.
    capture_at: Option<Rect>,
    /// The command whose change was refused, and why: said under its row.
    refused: Option<(&'static str, String)>,
}

/// A key cap waiting for the key that replaces it, or for a key to add.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Capture {
    command: &'static str,
    replacing: Option<Chord>,
}

impl Page {
    /// Whether the page waits for a key, which then runs no command.
    pub(in crate::app) fn capturing(&self) -> bool {
        self.capture.is_some()
    }
    pub(in crate::app) fn stop_capturing(&mut self) {
        self.capture = None;
    }
}

/// The groups, in the reference card's order.
const GROUPS: [&str; 10] = [
    "Everywhere",
    "Rating, flags and labels",
    "Quick Collection",
    "Library grid",
    "Stacks",
    "Loupe",
    "Compare",
    "Survey",
    "Develop",
    "Develop tools",
];

/// The group a command's row is in: the card's, by what it does and where.
fn group_of(command: &Command, bindings: &[&Binding]) -> &'static str {
    let id = command.id;
    if bindings::METADATA.iter().any(|b| b.command == id) {
        return "Rating, flags and labels";
    }
    if id.starts_with("quick:") {
        return "Quick Collection";
    }
    if id.starts_with("stack:") {
        return "Stacks";
    }
    match bindings.first().map(|b| b.context) {
        Some(Context::Library | Context::Grid) => "Library grid",
        Some(Context::Loupe | Context::LoupeZoom) => "Loupe",
        Some(Context::Compare) => "Compare",
        Some(Context::Survey) => "Survey",
        Some(Context::Develop) => "Develop",
        Some(Context::Tool(_)) => "Develop tools",
        Some(Context::Everywhere | Context::ShortcutSheet) | None => "Everywhere",
    }
}

/// Where a context's keys work, for a sentence.
fn place(context: Context) -> &'static str {
    match context {
        Context::Everywhere => "everywhere",
        Context::Library => "the Library",
        Context::Grid => "the grid",
        Context::Loupe => "the Loupe",
        Context::Compare => "Compare",
        Context::Survey => "Survey",
        Context::LoupeZoom => "the Loupe and Develop",
        Context::Develop => "Develop",
        Context::Tool(Tool::Crop) => "Crop",
        Context::Tool(Tool::Remove) => "Spot Removal",
        Context::Tool(Tool::RedEye) => "Red Eye",
        Context::Tool(Tool::Mask) => "Masking",
        Context::Tool(Tool::Guided) => "Guided Upright",
        Context::ShortcutSheet => "the shortcuts",
    }
}

fn title(id: &str) -> &'static str {
    COMMANDS
        .iter()
        .find(|c| c.id == id)
        .map_or("a command", |c| c.title)
}

/// One row of the page.
struct Row {
    group: &'static str,
    title: &'static str,
    /// The command it changes; none for a fixed key.
    id: Option<&'static str>,
    chords: Vec<Chord>,
    overridden: bool,
    /// Shown, not changed: the text-editing commands and the fixed keys.
    locked: bool,
    /// The commands whose keys this one shares where both run, neither of
    /// which does: each with the chord and where.
    conflicts: Vec<(&'static str, Chord, Context)>,
    /// What runs instead where a binding of higher precedence has its key.
    shadows: Vec<String>,
}

/// A command's keys in `map`: the user's, or every default chord of its
/// bindings that exists on this platform.
pub(in crate::app) fn keys_of(map: &Effective, id: &str) -> Vec<Chord> {
    if let Some(user) = map.user_chords(id) {
        return user.to_vec();
    }
    let mut out = Vec::new();
    for b in rules::bindings_of(id) {
        for d in map
            .defaults_of(b)
            .iter()
            .filter(|d| d.exists_on(cfg!(target_os = "macos")))
        {
            for c in d.chords() {
                if !out.contains(&c) {
                    out.push(c);
                }
            }
        }
    }
    out
}

fn rows(map: &Effective, clashes: &[Clash]) -> Vec<Row> {
    let mut out = Vec::new();
    for command in COMMANDS {
        let bindings = rules::bindings_of(command.id);
        let mine = |c: &&Clash| c.a.command == command.id || c.b.command == command.id;
        let mut conflicts: Vec<(&'static str, Chord, Context)> = Vec::new();
        let mut shadows = Vec::new();
        for clash in clashes.iter().filter(mine) {
            let (me, other) = if clash.a.command == command.id {
                (clash.a, clash.b)
            } else {
                (clash.b, clash.a)
            };
            if clash.conflict {
                if !conflicts
                    .iter()
                    .any(|(id, chord, _)| *id == other.command && *chord == clash.chord)
                {
                    // The narrower of the two places is where both run.
                    let at = if rules::level(other.context) > rules::level(me.context) {
                        other.context
                    } else {
                        me.context
                    };
                    conflicts.push((other.command, clash.chord, at));
                }
            } else if (map.overrides(me.command) || map.overrides(other.command))
                && rules::level(other.context) > rules::level(me.context)
            {
                shadows.push(format!(
                    "In {}, {} runs \u{201c}{}\u{201d} instead",
                    place(other.context),
                    clash.chord,
                    title(other.command)
                ));
            }
        }
        out.push(Row {
            group: group_of(command, &bindings),
            title: command.title,
            id: Some(command.id),
            chords: keys_of(map, command.id),
            overridden: map.overrides(command.id),
            locked: !command.rebindable,
            conflicts,
            shadows,
        });
    }
    for fixed in FIXED_KEYS {
        out.push(Row {
            group: if fixed.keys.contains(&Key::Space) {
                "Develop tools"
            } else {
                "Develop"
            },
            title: fixed.title,
            id: None,
            chords: fixed
                .mods
                .iter()
                .flat_map(|m| fixed.keys.iter().map(|k| Chord::new(*k, *m)))
                .collect(),
            overridden: false,
            locked: true,
            conflicts: Vec::new(),
            shadows: Vec::new(),
        });
    }
    out
}

/// What a click on the page asked for, done once it is drawn.
enum Ask {
    Capture(Capture),
    Reset(&'static str),
    ResetAll,
}

/// The width of a row's title.
const TITLE: f32 = 236.;
/// The height of a row.
const ROW: f32 = 28.;

impl Editor {
    /// Whether Preferences waits for a key to bind, which then runs nothing.
    pub(in crate::app) fn capturing_keys(&self) -> bool {
        self.preferences.open
            && self.preferences.tab == super::Tab::Shortcuts
            && self.preferences.shortcuts.capturing()
    }

    pub(super) fn shortcuts_page(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let palette = theme::palette(&ctx);
        // A click anywhere but on the waiting cap meant something else (the
        // search field, another row): the cap stops waiting, before the key
        // that follows the click is taken for it.
        if self.preferences.shortcuts.capture.is_some() {
            let pressed = ctx.input(|i| i.pointer.any_pressed().then(|| i.pointer.press_origin()));
            if let Some(at) = pressed
                && !at.is_some_and(|at| {
                    self.preferences
                        .shortcuts
                        .capture_at
                        .is_some_and(|cap| cap.contains(at))
                })
            {
                self.preferences.shortcuts.capture = None;
            }
        }
        self.preferences.shortcuts.capture_at = None;
        if let Some(capture) = self.preferences.shortcuts.capture
            && let Some(press) = FrameKeys::of(&ctx).capture()
        {
            self.captured(capture, &press);
        }
        let editable = self.keymap.editable();
        let mut asks = Vec::new();
        if let State::Unreadable(why) = self.keymap.state() {
            egui::Frame::new()
                .fill(palette.tone(Tone::CARD))
                .stroke(Stroke::new(1., palette.warning()))
                .corner_radius(radius::CARD)
                .inner_margin(space::L)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        egui::RichText::new(format!(
                            "keymap.toml isn't used: {why}. The default shortcuts apply, \
                             and the file is left as it is."
                        ))
                        .color(palette.tone(Tone::TEXT)),
                    );
                    hint(
                        ui,
                        "Fix the file and restart, or Reset All to keep it as keymap.toml.broken and start afresh.",
                    );
                });
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = space::M;
            let reset_width = 96.;
            let check_width = 170.;
            ui.allocate_ui(
                Vec2::new(
                    (ui.available_width() - reset_width - check_width - 2. * space::M).max(120.),
                    ROW,
                ),
                |ui| {
                    let field = search_field(
                        ui,
                        &mut self.preferences.shortcuts.query,
                        "Search, e.g. crop or cmd z",
                    );
                    if self.preferences.shortcuts.capture.is_some() {
                        field.surrender_focus();
                    }
                    // Focus gone to another control: the keys are its now.
                    if ui
                        .ctx()
                        .memory(|m| m.focused().is_some_and(|f| f != field.id))
                    {
                        self.preferences.shortcuts.capture = None;
                    }
                },
            );
            ui.checkbox(
                &mut self.preferences.shortcuts.conflicts_only,
                "Show conflicts only",
            );
            if ui
                .button("Reset All")
                .on_hover_text("Every command back to its default keys")
                .clicked()
            {
                asks.push(Ask::ResetAll);
            }
        });
        if let Some(path) = self.keymap.path() {
            hint(
                ui,
                &format!(
                    "Your keys are kept in {}.",
                    crate::app::ui::widgets::pretty_path(path)
                ),
            );
        }
        ui.add_space(space::M);
        let map = self.keymap.effective();
        let found = clashes(&map, cfg!(target_os = "macos"));
        let tokens = tokens(&self.preferences.shortcuts.query);
        let conflicts_only = self.preferences.shortcuts.conflicts_only;
        let shown: Vec<Row> = rows(&map, &found)
            .into_iter()
            .filter(|r| !conflicts_only || !r.conflicts.is_empty())
            .filter(|r| {
                let keys: Vec<String> = r.chords.iter().map(Chord::to_string).collect();
                search_matches(&tokens, r.group, &keys.join(" / "), r.title)
            })
            .collect();
        let capture = self.preferences.shortcuts.capture;
        let mut capture_at = None;
        let refused = self.preferences.shortcuts.refused.clone();
        let unknown = self.keymap.unknown().to_vec();
        let skipped = self.keymap.skipped().to_vec();
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                cards(ui, |ui| {
                    if shown.is_empty() {
                        hint(ui, "No shortcuts match.");
                    }
                    for name in GROUPS {
                        let rows: Vec<&Row> = shown.iter().filter(|r| r.group == name).collect();
                        if rows.is_empty() {
                            continue;
                        }
                        group(ui, name);
                        for row in rows {
                            row_ui(
                                ui,
                                row,
                                editable,
                                capture,
                                refused.as_ref(),
                                &mut asks,
                                &mut capture_at,
                            );
                        }
                        gap(ui);
                    }
                    if !skipped.is_empty() {
                        group(ui, "Skipped entries in keymap.toml");
                        for s in &skipped {
                            hint(ui, &format!("{}: {}. Its default keys apply.", s.id, s.why));
                        }
                        gap(ui);
                    }
                    if !unknown.is_empty() {
                        group(ui, "Not known to this release");
                        hint(
                            ui,
                            "keymap.toml has keys for these, kept as they are for the release or extension that knows them:",
                        );
                        for id in &unknown {
                            hint(ui, id);
                        }
                    }
                });
            });
        self.preferences.shortcuts.capture_at = capture_at;
        for ask in asks {
            match ask {
                Ask::Capture(capture) => {
                    self.preferences.shortcuts.capture = Some(capture);
                    self.preferences.shortcuts.refused = None;
                }
                Ask::Reset(id) => {
                    let result = self.keymap.reset(id);
                    self.keys_saved(id, result);
                }
                Ask::ResetAll => {
                    self.preferences.shortcuts.capture = None;
                    let result = self.keymap.reset_all();
                    self.keys_saved("", result);
                }
            }
        }
    }

    /// The key pressed for `capture`: it replaces the key, or is added;
    /// Escape keeps the keys as they were, and Delete takes the key away.
    fn captured(&mut self, capture: Capture, press: &Press) {
        self.preferences.shortcuts.capture = None;
        let alone = Mods::of(press.modifiers) == Mods::NONE;
        if press.key == Key::Escape && alone {
            return;
        }
        let mut keys = keys_of(&self.keymap.effective(), capture.command);
        if matches!(press.key, Key::Delete | Key::Backspace) && alone {
            match capture.replacing {
                Some(old) => keys.retain(|k| *k != old),
                None => return,
            }
        } else {
            // As the command reads keys: the Basic panel's by their place.
            let template = rules::bindings_of(capture.command)
                .first()
                .and_then(|b| b.defaults.first());
            let chord = chord_of(press, template);
            match capture
                .replacing
                .and_then(|old| keys.iter().position(|k| *k == old))
            {
                Some(at) => keys[at] = chord,
                None => keys.push(chord),
            }
        }
        let result = self.keymap.set(capture.command, &keys);
        self.keys_saved(capture.command, result);
    }

    /// Says how a change to `id`'s keys went.
    fn keys_saved(&mut self, id: &'static str, result: Result<Saved, String>) {
        match result {
            Ok(Saved::Durably) => self.preferences.shortcuts.refused = None,
            Ok(Saved::MaybeNotDurably(error)) => {
                self.preferences.shortcuts.refused = None;
                self.messages.warn(format!(
                    "The shortcuts were saved, but may not survive a crash: {error}"
                ));
            }
            Err(why) => {
                self.messages.fail(why.clone());
                self.preferences.shortcuts.refused = Some((id, why));
            }
        }
    }
}

fn row_ui(
    ui: &mut egui::Ui,
    row: &Row,
    editable: bool,
    capture: Option<Capture>,
    refused: Option<&(&'static str, String)>,
    asks: &mut Vec<Ask>,
    capture_at: &mut Option<Rect>,
) {
    let palette = theme::palette(ui.ctx());
    let changeable = editable && !row.locked && row.id.is_some();
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = space::S;
        let tint = palette.tone(if row.locked { Tone::MUTED } else { Tone::TEXT });
        ui.allocate_ui_with_layout(
            Vec2::new(TITLE, ROW),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_size(Vec2::new(TITLE, ROW));
                if row.locked {
                    icons::paint_at(
                        ui.painter(),
                        Icon::AspectLocked,
                        ui.cursor().left_center() + Vec2::new(6., ROW / 2. - 1.),
                        12.,
                        palette.tone(Tone::FAINT),
                    );
                    ui.add_space(space::XL);
                }
                ui.add(egui::Label::new(egui::RichText::new(row.title).color(tint)).truncate());
            },
        );
        let rest = ui.available_width();
        ui.allocate_ui_with_layout(
            Vec2::new(rest, ROW),
            egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true),
            |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(space::S, space::S);
                let waiting = row
                    .id
                    .is_some_and(|id| capture.is_some_and(|c| c.command == id));
                for chord in &row.chords {
                    let this = waiting && capture.is_some_and(|c| c.replacing == Some(*chord));
                    let label = if this {
                        "Press a key…".to_string()
                    } else {
                        chord_text(chord)
                    };
                    let response = key_cap(ui, &label, this, !row.locked);
                    if this {
                        *capture_at = Some(response.rect);
                    }
                    let name = format!("{}: {}", row.title, chord);
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Button, changeable, &name)
                    });
                    if changeable && response.clicked() {
                        asks.push(Ask::Capture(Capture {
                            command: row.id.expect("changeable"),
                            replacing: Some(*chord),
                        }));
                    }
                }
                if row.chords.is_empty() && !waiting {
                    ui.label(
                        egui::RichText::new("No key")
                            .size(text::LABEL)
                            .color(palette.tone(Tone::FAINT)),
                    );
                }
                if waiting && capture.is_some_and(|c| c.replacing.is_none()) {
                    *capture_at = Some(key_cap(ui, "Press a key…", true, true).rect);
                }
                if changeable {
                    let title = row.title;
                    let add = crate::app::shell::shortcuts::icon_button(
                        ui,
                        Icon::Add,
                        &format!("Add a key to {title}"),
                    );
                    if add.clicked() {
                        asks.push(Ask::Capture(Capture {
                            command: row.id.expect("changeable"),
                            replacing: None,
                        }));
                    }
                    if row.overridden {
                        let reset = ui
                            .add(egui::Button::new(
                                egui::RichText::new("Reset").size(text::LABEL),
                            ))
                            .on_hover_text("Back to the default keys");
                        let name = format!("Reset {title}");
                        reset.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name)
                        });
                        if reset.clicked() {
                            asks.push(Ask::Reset(row.id.expect("changeable")));
                        }
                    }
                }
            },
        );
    });
    let indent = TITLE + space::S;
    let note = |ui: &mut egui::Ui, text: String, color: egui::Color32| {
        ui.horizontal(|ui| {
            ui.add_space(indent);
            ui.add(
                egui::Label::new(egui::RichText::new(text).size(text::LABEL).color(color)).wrap(),
            );
        });
    };
    if row
        .id
        .is_some_and(|id| capture.is_some_and(|c| c.command == id))
    {
        note(
            ui,
            "Press the key and its modifiers · Esc keeps the keys · Delete removes this one".into(),
            palette.tone(Tone::MUTED),
        );
    }
    for (other, chord, at) in &row.conflicts {
        note(
            ui,
            format!(
                "Conflicts with \u{201c}{}\u{201d} on {chord} in {}: neither runs",
                title(other),
                place(*at)
            ),
            palette.warning(),
        );
    }
    for shadow in &row.shadows {
        note(ui, shadow.clone(), palette.tone(Tone::FAINT));
    }
    if let (Some(id), Some((refused_id, why))) = (row.id, refused)
        && id == *refused_id
    {
        note(ui, why.clone(), palette.danger());
    }
}

/// A chord as its caps read on this platform: "⌘⇧E" on a Mac, "Ctrl+Shift+E".
fn chord_text(chord: &Chord) -> String {
    let text = chord.to_string();
    let caps: Vec<String> = split_caps(&text).into_iter().map(cap_label).collect();
    if cfg!(target_os = "macos") {
        caps.concat()
    } else {
        caps.join("+")
    }
}

/// A key cap as the reference card draws one, which a click on starts a
/// capture; `waiting` while it waits for the key.
fn key_cap(ui: &mut egui::Ui, label: &str, waiting: bool, live: bool) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let galley = ui.painter().layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(text::SMALL),
        palette.tone(if live { Tone::TEXT } else { Tone::MUTED }),
    );
    let size = Vec2::new((galley.size().x + 12.).max(22.), 22.);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let hovered = live && response.hovered();
    let cap = Rect::from_min_size(rect.min, size);
    // The keyboard's focus, outlined as the app's fields outline it.
    if response.has_focus() {
        ui.painter().rect_stroke(
            cap.expand(2.),
            6.,
            Stroke::new(1., palette.accent()),
            egui::StrokeKind::Outside,
        );
    }
    ui.painter().rect_filled(
        cap.translate(Vec2::new(0., 1.5)),
        4.,
        palette.tone(Tone::WINDOW),
    );
    ui.painter().rect(
        cap,
        4.,
        palette.tone(if hovered { Tone::HOVER } else { Tone::OUTLINE }),
        Stroke::new(
            1.,
            if waiting {
                palette.accent()
            } else {
                palette.tone(Tone::HOVER)
            },
        ),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(
        Pos2::new(
            cap.center().x - galley.size().x / 2.,
            cap.center().y - galley.size().y / 2.,
        ),
        galley,
        egui::Color32::WHITE,
    );
    if live {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}

#[cfg(test)]
#[path = "shortcuts_editor_tests.rs"]
mod editor_tests;
