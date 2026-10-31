//! A reference card of every shortcut, grouped like Lightroom's: Cmd+/ (as in
//! Lightroom) or the keyboard
//! button in the top bar. Each shortcut's keys are drawn as key caps.
use super::Editor;
use super::icons::{self, Icon};
use super::theme;
use super::widgets::{modal_frame, search_field};
use crate::app::Module;
use crate::app::design::{self, Tone, inset, radius, space, text};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};

/// A shortcut: its keys and what it does. Keys are written "Cmd+Shift+Z";
/// " / " separates alternatives, and a word in lower case ("click") is an
/// action rather than a key.
type Shortcut = (&'static str, &'static str);
/// A titled group of shortcuts.
type Group<'a> = (&'a str, &'a [Shortcut]);

const LIBRARY: [Shortcut; 17] = [
    ("Arrows", "Move the active photo"),
    (
        "Page Up / Page Down",
        "Move the active photo a page up / down",
    ),
    ("Home / End", "First / last photo"),
    ("Shift+Arrows / Shift+click", "Extend the selection"),
    ("Cmd+click", "Add or remove a photo"),
    ("Cmd+A", "Select all"),
    ("Cmd+D", "Select none"),
    ("/", "Deselect the active photo"),
    ("Cmd+L", "Turn the filters off / on"),
    ("J", "Cycle the grid cell style"),
    ("Delete", "Move to Trash, or remove from the catalog"),
    ("Alt+Delete", "Remove from the catalog"),
    ("Cmd+Alt+Shift+Delete", "Move to Trash"),
    ("Cmd+Delete", "Delete the rejected photos"),
    (
        "F2",
        "Rename the photo, or the selected photos in capture order",
    ),
    (
        "Cmd+Shift+S",
        "Sync the active photo's settings to the selection",
    ),
    ("Cmd+Shift+U", "Auto Settings on each selected photo"),
];
const LOUPE: [Shortcut; 7] = [
    ("E / Return / double-click", "Open the Loupe"),
    ("G / Esc / double-click", "Back to the grid"),
    ("Z / click", "Fit or the last zoom"),
    ("Cmd+= / Cmd+-", "Zoom in / out"),
    ("Cmd+0 / Cmd+1", "100% / Fit"),
    ("drag", "Pan"),
    ("I", "Cycle photo info"),
];
const COMPARE: [Shortcut; 6] = [
    ("C", "Compare the active photo with the next"),
    ("Left / Right", "Change the candidate"),
    ("Up", "Make the candidate the select"),
    ("Down", "Swap select and candidate"),
    ("click", "Make a photo active for rating keys"),
    ("G / Esc", "Back to the grid"),
];
const SURVEY: [Shortcut; 5] = [
    ("N", "Survey the selected photos"),
    ("Arrows", "Move the active photo"),
    ("click", "Make a photo active for rating keys"),
    ("Cmd+click", "Take a photo out"),
    ("G / Esc", "Back to the grid"),
];
const RATING: [Shortcut; 8] = [
    ("0 / 1 / 2 / 3 / 4 / 5", "Star rating"),
    ("[ / ]", "Lower / raise the rating"),
    ("6 / 7 / 8 / 9", "Red, yellow, green, blue"),
    ("P / X / U", "Pick / reject / unflag"),
    ("`", "Toggle pick"),
    ("Cmd+Up / Cmd+Down", "Raise / lower the flag"),
    ("Shift+key", "Apply and go to the next photo"),
    ("Cmd+Z / Cmd+Shift+Z", "Undo / redo, anywhere"),
];
const QUICK: [Shortcut; 3] = [
    ("B", "Add to or take out of it"),
    ("Cmd+B", "Show it"),
    ("Cmd+Shift+B", "Clear it"),
];
const STACKS: [Shortcut; 4] = [
    ("Cmd+G", "Group into stack"),
    ("Cmd+Shift+G", "Unstack"),
    ("S", "Collapse / expand the stack"),
    ("Shift+S", "Move to the top of the stack"),
];
const GENERAL: [Shortcut; 12] = [
    ("G", "Library"),
    ("D", "Develop"),
    ("Tab", "Hide / show the side panels"),
    ("Shift+Tab", "Hide / show the side panels and filmstrip"),
    ("F", "Hide all but the photo; grid first in the Library"),
    ("Esc", "Leave the whole-window mode"),
    ("Shift+Esc", "Dismiss the messages in the top bar"),
    ("F7 / F8 / F6", "Left panel / right panel / filmstrip"),
    ("Shift+F8", "Fold / open Presets and History"),
    ("Cmd+'", "Create virtual copy"),
    ("Cmd+,", "Preferences"),
    ("Cmd+/", "These shortcuts"),
];
const DEVELOP: [Shortcut; 54] = [
    ("R", "Crop & Straighten"),
    ("Return", "Finish the crop"),
    ("X", "Swap the crop's orientation"),
    ("O / Shift+O", "Next crop overlay / turn it"),
    ("Cmd+drag", "Straighten along a line"),
    ("Shift+T", "Guided Upright: draw guides"),
    ("Delete", "Delete the selected spot, guide or red eye"),
    ("Q", "Spot removal"),
    ("Shift+W", "Masking"),
    ("W", "White balance selector"),
    ("Cmd+Option+Shift+T", "Targeted adjustment: Tone Curve"),
    (
        "Cmd+Option+Shift+H / S / L",
        "Targeted adjustment: Hue / Saturation / Luminance",
    ),
    ("Cmd+Option+Shift+G", "Targeted adjustment: B&W mix"),
    ("Esc", "Put the open tool away"),
    ("[ / ]", "Brush or red eye size (Shift: feather)"),
    (
        "Scroll",
        "Brush or red eye size over the photo (Shift: feather)",
    ),
    ("/", "New source for the selected spot"),
    ("H", "Hide spot pins"),
    ("A", "Visualize spots"),
    ("Space", "Pan while a tool is open"),
    ("\\", "Before alone"),
    ("Shift+\\", "Camera thumbnail (the embedded JPEG)"),
    ("Y", "Before and after, left and right"),
    ("Option+Y", "Before and after, top and bottom"),
    ("Shift+Y", "Before and after, split"),
    (
        "Cmd+Option+Shift+→ / ←",
        "Copy Before to After / After to Before",
    ),
    ("Cmd+Option+Shift+↑", "Swap Before and After"),
    ("Shift+R", "Reference View"),
    ("I", "Photo info overlay: Info 1, Info 2, off"),
    ("J", "Show shadow and highlight clipping"),
    ("Z", "Toggle zoom / fit"),
    (
        "Cmd+= / Cmd+- / Cmd+0 / Cmd+1",
        "Zoom in / out / 100% / Fit",
    ),
    ("Shift+Z", "Magnifier over the photo; wheel: magnification"),
    ("Left / Right", "Previous / next photo"),
    ("Cmd+A", "Select all in the filmstrip, for Sync"),
    ("Cmd+D", "Select none in the filmstrip"),
    ("Cmd+Shift+C / Cmd+Shift+V", "Copy / paste settings"),
    ("Cmd+Option+V", "Paste settings from previous photo"),
    ("Cmd+Shift+S", "Sync settings to the selected photos"),
    ("Cmd+Option+Shift+M", "Match total exposures"),
    ("Cmd+Shift+N", "New Develop Preset"),
    ("Cmd+Shift+R", "Reset all settings"),
    ("Cmd+Shift+U", "Auto tone"),
    ("V", "Convert to black & white or back to color"),
    ("Cmd+Shift+E", "Export…"),
    ("Cmd+Alt+Shift+E", "Export with previous"),
    ("double-click", "Reset a slider or grading wheel"),
    ("Shift+drag", "Grading wheel: hue or saturation only"),
    ("Cmd+drag", "Grading wheel: fine adjustment"),
    ("Up / Down", "Nudge the hovered slider (Shift: 10)"),
    (". / ,", "Select the next / previous Basic slider"),
    (
        "= / -",
        "Step the selected Basic slider (Shift: more, Option: less)",
    ),
    ("right-click", "Panel header: Solo Mode"),
    ("E", "Show the photo in the Loupe"),
];

/// The sheet's widest; it narrows with the window.
const WIDTH: f32 = 960.;
/// The space between the two columns.
const GUTTER: f32 = 40.;
/// The space between a shortcut's keys and what it does.
const GAP: f32 = 14.;
/// A row's inner padding, so its hover band clears the text.
const PAD: f32 = space::M;
/// The height of a one-line row.
const ROW: f32 = 30.;

/// The search, kept while the sheet is open and dropped when it closes.
fn query_id() -> egui::Id {
    egui::Id::new("keyboard-shortcuts-query")
}

impl Editor {
    pub(super) fn shortcuts_window(&mut self, ctx: &egui::Context) {
        let palette = theme::palette(ctx);
        if !self.view.shortcuts {
            ctx.data_mut(|d| d.remove::<String>(query_id()));
            return;
        }
        // Cmd+/ closes it again; other keys stay with the sheet.
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Slash)) {
            self.view.shortcuts = false;
            return;
        }
        let stored = ctx.data(|d| d.get_temp::<String>(query_id()));
        let opened = stored.is_none();
        let mut query = stored.unwrap_or_default();
        // Esc clears a search before it closes the sheet, and typing goes on.
        let cleared = !query.is_empty()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        if cleared {
            query.clear();
        }
        let views: [Group; 5] = [
            ("Library grid", &LIBRARY),
            (
                "Stacks, in the grid; S and Shift+S in the Loupe too",
                &STACKS,
            ),
            ("Loupe", &LOUPE),
            ("Compare", &COMPARE),
            ("Survey", &SURVEY),
        ];
        let common: [Group; 3] = [
            ("Rating, flags and labels", &RATING),
            ("Quick Collection", &QUICK),
            ("Everywhere", &GENERAL),
        ];
        let develop: [Group; 1] = [("Develop", &DEVELOP)];
        // The current module's shortcuts come first, in the left column.
        let (left, right): (Vec<Group>, Vec<Group>) = if self.module == Module::Library {
            (views.to_vec(), [&common[..], &develop].concat())
        } else {
            ([&develop[..], &common].concat(), views.to_vec())
        };
        let width = WIDTH.min(ctx.content_rect().width() - 80.).max(520.);
        let height = (ctx.content_rect().height() * 0.85 - 150.).max(220.);
        let response = egui::Modal::new(egui::Id::new("keyboard-shortcuts"))
            .backdrop_color(Color32::from_black_alpha(140))
            .frame(modal_frame(&palette).inner_margin(design::margin_same(inset::DIALOG)))
            .show(ctx, |ui| {
                ui.set_width(width);
                let mut close = false;
                ui.horizontal(|ui| {
                    title(ui);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        close = icon_button(ui, Icon::Close, "Close · Esc").clicked();
                    });
                });
                ui.add_space(space::XL);
                let field =
                    search_field(ui, &mut query, "Search actions or keys, e.g. crop or cmd z");
                if opened || cleared {
                    field.request_focus();
                }
                ui.add_space(space::L);
                let tokens = tokens(&query);
                let (left, right) = filtered(&tokens, &left, &right);
                egui::ScrollArea::vertical()
                    .max_height(height)
                    .min_scrolled_height(height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if left.is_empty() && right.is_empty() {
                            ui.add_space(height * 0.3);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "No shortcuts match \u{201c}{}\u{201d}",
                                        query.trim()
                                    ))
                                    .size(text::BODY)
                                    .color(palette.tone(Tone::SECONDARY)),
                                );
                            });
                            return;
                        }
                        // Room on the right for the scroll bar.
                        let column = ((ui.available_width() - GUTTER - 10.) / 2.).floor();
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = GUTTER;
                            for groups in [&left, &right] {
                                ui.allocate_ui(Vec2::new(column, 0.), |ui| {
                                    ui.vertical(|ui| {
                                        ui.spacing_mut().item_spacing.y = 0.;
                                        // One keys column per side, as wide as
                                        // its widest keys allow.
                                        let keys = groups
                                            .iter()
                                            .flat_map(|(_, shortcuts)| shortcuts)
                                            .map(|(keys, _)| {
                                                paint_keys(ui, keys, Pos2::ZERO, false)
                                            })
                                            .fold(0., f32::max)
                                            .min(column * 0.38);
                                        for (title, shortcuts) in groups {
                                            group(ui, title, shortcuts, column, keys);
                                        }
                                    });
                                });
                            }
                        });
                    });
                close
            });
        if response.inner || response.should_close() {
            self.view.shortcuts = false;
        } else {
            ctx.data_mut(|d| d.insert_temp(query_id(), query));
        }
    }
}

/// The sheet's title and, smaller beside it on the same baseline, the key
/// that shows it.
fn title(ui: &mut egui::Ui) {
    let palette = theme::palette(ui.ctx());
    let painter = ui.painter();
    let name = painter.layout_no_wrap(
        "Keyboard Shortcuts".into(),
        FontId::proportional(text::TITLE),
        palette.tone(Tone::STRONG),
    );
    let hint = painter.layout_no_wrap(
        format!("{} shows this anytime", keys_text("Cmd+/")),
        FontId::proportional(text::LABEL),
        palette.tone(Tone::FAINT),
    );
    let baseline = |galley: &egui::Galley| {
        let row = galley.rows.first();
        row.and_then(|row| row.glyphs.first())
            .map_or(galley.size().y, |glyph| glyph.pos.y)
    };
    let size = Vec2::new(name.size().x + 10. + hint.size().x, name.size().y);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let hint_top = rect.top() + baseline(&name) - baseline(&hint);
    let hint_left = rect.left() + name.size().x + 10.;
    ui.painter().galley(rect.min, name, Color32::WHITE);
    ui.painter()
        .galley(Pos2::new(hint_left, hint_top), hint, Color32::WHITE);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Label, true, "Keyboard Shortcuts")
    });
}

/// A titled group of the shortcuts a search kept.
type Found<'a> = (&'a str, Vec<Shortcut>);

/// The groups whose shortcuts match every word of the search, in their
/// columns. A search fills the two columns evenly, as the groups left may
/// otherwise all sit on one side.
fn filtered<'a>(
    tokens: &[String],
    left: &[Group<'a>],
    right: &[Group<'a>],
) -> (Vec<Found<'a>>, Vec<Found<'a>>) {
    let keep = |&(title, shortcuts): &Group<'a>| -> Option<Found<'a>> {
        let kept: Vec<Shortcut> = shortcuts
            .iter()
            .filter(|shortcut| matches(tokens, title, shortcut))
            .copied()
            .collect();
        (!kept.is_empty()).then_some((title, kept))
    };
    if tokens.is_empty() {
        return (
            left.iter().filter_map(keep).collect(),
            right.iter().filter_map(keep).collect(),
        );
    }
    let (mut a, mut b) = (Vec::new(), Vec::new());
    let (mut rows_a, mut rows_b) = (0, 0);
    for found in left.iter().chain(right).filter_map(keep) {
        // A group's title counts as about two rows.
        let rows = found.1.len() + 2;
        if rows_a <= rows_b {
            rows_a += rows;
            a.push(found);
        } else {
            rows_b += rows;
            b.push(found);
        }
    }
    (a, b)
}

/// The search's words, lower case. "+" separates words as a space does, and
/// the macOS modifier symbols are words of their own, so "cmd+z" and "⌘Z"
/// both find Cmd+Z.
fn tokens(query: &str) -> Vec<String> {
    let mut spaced = String::new();
    for c in query.chars() {
        if matches!(c, '⌘' | '⌥' | '⇧' | '+') {
            spaced.extend([' ', c, ' ']);
        } else {
            spaced.push(c);
        }
    }
    spaced
        .split_whitespace()
        .filter(|word| *word != "+")
        .map(str::to_lowercase)
        .collect()
}

/// Whether a shortcut has every word of the search: one letter or symbol
/// names a key; a longer word may also start a word of what it does or of its
/// group's title.
fn matches(tokens: &[String], title: &str, (keys, action): &Shortcut) -> bool {
    tokens.iter().all(|token| {
        let key = keys
            .split(" / ")
            .flat_map(|alternative| alternative.split('+'))
            .any(|cap| key_matches(cap, token));
        key || token.chars().count() > 1
            && words(title)
                .chain(words(action))
                .any(|w| w.starts_with(token.as_str()))
    })
}

fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
}

/// Whether a key cap is the key a search word names, by any of its names.
fn key_matches(cap: &str, token: &str) -> bool {
    let canonical = |name: &str| -> String {
        match name.to_lowercase().as_str() {
            "cmd" | "command" | "⌘" | "ctrl" | "control" => "cmd".into(),
            "alt" | "option" | "opt" | "⌥" => "alt".into(),
            "shift" | "⇧" => "shift".into(),
            "return" | "enter" | "↩" => "return".into(),
            "delete" | "del" | "backspace" | "⌫" => "delete".into(),
            "esc" | "escape" => "esc".into(),
            "up" | "↑" => "up".into(),
            "down" | "↓" => "down".into(),
            "left" | "←" => "left".into(),
            "right" | "→" => "right".into(),
            other => other.into(),
        }
    };
    let (cap, token) = (canonical(cap), canonical(token));
    cap == token
        || cap == "arrows" && matches!(token.as_str(), "up" | "down" | "left" | "right" | "arrow")
}

fn group(ui: &mut egui::Ui, title: &str, shortcuts: &[Shortcut], column: f32, keys: f32) {
    let palette = theme::palette(ui.ctx());
    // The title over a hairline that spans the column.
    let (rect, _) = ui.allocate_exact_size(Vec2::new(column, 26.), Sense::hover());
    ui.painter().text(
        Pos2::new(rect.left() + PAD, rect.top() + 8.),
        Align2::LEFT_CENTER,
        title.to_uppercase(),
        FontId::proportional(text::SMALL),
        palette.tone(Tone::MUTED),
    );
    ui.painter().hline(
        rect.x_range().shrink(PAD),
        rect.bottom() - 6.,
        Stroke::new(1., palette.tone(Tone::RAISED)),
    );
    for (shortcut_keys, action) in shortcuts {
        let needed = paint_keys(ui, shortcut_keys, Pos2::ZERO, false);
        // Keys too wide for the keys column get a line of their own.
        let stacked = needed > keys + 0.5;
        let action_x = PAD + keys + GAP;
        let galley = ui.painter().layout(
            action.to_string(),
            FontId::proportional(text::LABEL),
            palette.tone(Tone::SOFT),
            (column - action_x - PAD).max(80.),
        );
        let first_line = galley.rows.first().map_or(16., |row| row.height());
        let text_height = galley.size().y + (ROW - first_line);
        let height = if stacked {
            ROW + galley.size().y + 6.
        } else {
            text_height.max(ROW)
        };
        let (rect, response) = ui.allocate_exact_size(Vec2::new(column, height), Sense::hover());
        if response.hovered() {
            ui.painter()
                .rect_filled(rect, radius::CONTROL, palette.tone(Tone::FIELD));
        }
        let keys_y = rect.top() + ROW / 2.;
        let keys_right = if stacked {
            rect.left() + PAD + needed
        } else {
            rect.left() + PAD + keys
        };
        paint_keys(ui, shortcut_keys, Pos2::new(keys_right, keys_y), true);
        let text_top = if stacked {
            rect.top() + ROW
        } else {
            keys_y - first_line / 2.
        };
        ui.painter().galley(
            Pos2::new(rect.left() + action_x, text_top),
            galley,
            Color32::WHITE,
        );
    }
    ui.add_space(space::XL);
}

/// Draws a shortcut's key caps ending at `right`, alternatives apart by "/",
/// or only measures them; returns their width.
fn paint_keys(ui: &egui::Ui, keys: &str, right: Pos2, paint: bool) -> f32 {
    let palette = theme::palette(ui.ctx());
    let mut x = right.x;
    let mut alternatives: Vec<&str> = keys.split(" / ").collect();
    // A run of number keys reads as its ends: "0 – 5".
    let range = digit_run(&alternatives);
    if range {
        alternatives = vec![alternatives[0], alternatives[alternatives.len() - 1]];
    }
    for (i, alternative) in alternatives.iter().enumerate().rev() {
        for cap in alternative.split('+').collect::<Vec<_>>().into_iter().rev() {
            x = key_cap(ui, cap, Pos2::new(x, right.y), paint) - 3.;
        }
        if i > 0 {
            x -= 3.;
            if paint {
                ui.painter().text(
                    Pos2::new(x, right.y),
                    Align2::RIGHT_CENTER,
                    if range { "–" } else { "/" },
                    FontId::proportional(text::SMALL),
                    palette.tone(Tone::DISABLED),
                );
            }
            x -= 9.;
        }
    }
    right.x - x - 3.
}

/// Whether the alternatives are three or more number keys in a row.
fn digit_run(alternatives: &[&str]) -> bool {
    let digits: Option<Vec<u32>> = alternatives
        .iter()
        .map(|key| match key.as_bytes() {
            [d] => (*d as char).to_digit(10),
            _ => None,
        })
        .collect();
    digits.is_some_and(|d| d.len() > 2 && d.windows(2).all(|w| w[1] == w[0] + 1))
}

/// Draws one key cap ending at `right`, or only measures it; returns its left
/// edge. A word in lower case is an action, drawn as plain text.
fn key_cap(ui: &egui::Ui, cap: &str, right: Pos2, paint: bool) -> f32 {
    let palette = theme::palette(ui.ctx());
    let label = cap_label(cap);
    let action = cap.chars().all(|c| c.is_lowercase() || c == '-') && cap.len() > 1;
    let galley = ui.painter().layout_no_wrap(
        label,
        FontId::proportional(if action { text::LABEL } else { text::SMALL }),
        palette.tone(if action { Tone::MUTED } else { Tone::TEXT }),
    );
    if action {
        let left = right.x - galley.size().x;
        if paint {
            ui.painter().galley(
                Pos2::new(left, right.y - galley.size().y / 2.),
                galley,
                Color32::WHITE,
            );
        }
        return left;
    }
    let width = (galley.size().x + 10.).max(20.);
    let cap_rect = Rect::from_min_max(
        Pos2::new(right.x - width, right.y - 10.),
        Pos2::new(right.x, right.y + 10.),
    );
    if paint {
        // A darker lip under the cap, so it reads as a key.
        ui.painter().rect_filled(
            cap_rect.translate(Vec2::new(0., 1.5)),
            4.,
            palette.tone(Tone::WINDOW),
        );
        ui.painter().rect(
            cap_rect,
            4.,
            palette.tone(Tone::OUTLINE),
            Stroke::new(1., palette.tone(Tone::HOVER)),
            egui::StrokeKind::Inside,
        );
        ui.painter().galley(
            cap_rect.center() - galley.size() / 2.,
            galley,
            Color32::WHITE,
        );
    }
    cap_rect.left()
}

/// A small borderless icon button, as in the top bar.
pub(super) fn icon_button(ui: &mut egui::Ui, icon: Icon, hover: &str) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(26.), Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, radius::CONTROL, palette.tone(Tone::RAISED));
    }
    let color = palette.tone(if response.hovered() {
        Tone::STRONG
    } else {
        Tone::SECONDARY
    });
    icons::paint_at(ui.painter(), icon, rect.center(), 15., color);
    // Named for screen readers by what it does, as its hover text says.
    let enabled = ui.is_enabled();
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, hover));
    response
        .on_hover_text(hover)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A key as the platform names it: ⌘ ⌥ ⇧ on macOS.
fn cap_label(cap: &str) -> String {
    let mac = cfg!(target_os = "macos");
    match cap {
        "Cmd" if mac => "⌘".into(),
        "Cmd" => "Ctrl".into(),
        "Alt" | "Option" if mac => "⌥".into(),
        "Option" => "Alt".into(),
        "Shift" if mac => "⇧".into(),
        "Up" => "↑".into(),
        "Down" => "↓".into(),
        "Left" => "←".into(),
        "Right" => "→".into(),
        "Arrows" => "← ↑ → ↓".into(),
        "Return" if mac => "↩".into(),
        other => other.into(),
    }
}

/// Keys as text, e.g. for a hover: "⌘/".
pub(super) fn keys_text(keys: &str) -> String {
    keys.split('+').map(cap_label).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(query: &str) -> Vec<Shortcut> {
        let groups: [Group; 3] = [
            ("Library grid", &LIBRARY),
            ("Loupe", &LOUPE),
            ("Develop", &DEVELOP),
        ];
        let (left, right) = filtered(&tokens(query), &groups, &[]);
        left.into_iter().chain(right).flat_map(|(_, s)| s).collect()
    }

    #[test]
    fn a_search_finds_actions_by_word_and_shortcuts_by_key() {
        assert!(found("crop").contains(&("R", "Crop & Straighten")));
        assert!(found("straigh").contains(&("R", "Crop & Straighten")));
        // Every spelling of a modifier names the same key.
        for query in ["cmd+shift+e", "⌘⇧E", "command shift e", "ctrl shift e"] {
            assert!(
                found(query).contains(&("Cmd+Shift+E", "Export…")),
                "{query}"
            );
        }
        assert!(found("option y").contains(&("Option+Y", "Before and after, top and bottom")));
        assert!(found("alt y").contains(&("Option+Y", "Before and after, top and bottom")));
        // A word starts where the search does: "rat" is no saturation.
        assert!(!found("rat").contains(&(
            "Cmd+Option+Shift+H / S / L",
            "Targeted adjustment: Hue / Saturation / Luminance"
        )));
        assert!(found("up").contains(&("Arrows", "Move the active photo")));
    }

    #[test]
    fn one_letter_names_a_key_not_a_word() {
        let z = found("z");
        assert!(z.contains(&("Z", "Toggle zoom / fit")));
        // "Zoom in / out" is Cmd+= / Cmd+-, no Z key.
        assert!(!z.contains(&("Cmd+= / Cmd+-", "Zoom in / out")));
    }

    #[test]
    fn a_group_title_finds_its_shortcuts_and_nothing_left_is_empty() {
        assert_eq!(found("loupe").len(), LOUPE.len() + 1, "and Develop's E");
        assert!(found("nothing-like-this").is_empty());
        assert_eq!(found("").len(), LIBRARY.len() + LOUPE.len() + DEVELOP.len());
    }

    #[test]
    fn a_search_shares_the_groups_between_both_columns() {
        let groups: [Group; 2] = [("Library grid", &LIBRARY), ("Loupe", &LOUPE)];
        let (left, right) = filtered(&tokens("photo"), &groups, &[]);
        assert_eq!((left.len(), right.len()), (1, 1));
    }

    #[test]
    fn number_keys_in_a_row_read_as_a_range() {
        assert!(digit_run(&["0", "1", "2", "3", "4", "5"]));
        assert!(digit_run(&["6", "7", "8", "9"]));
        assert!(!digit_run(&["1", "2"]));
        assert!(!digit_run(&["1", "3", "4"]));
        assert!(!digit_run(&["P", "X", "U"]));
        // The keys inside a range are still found.
        assert!(found_in(&RATING, "3").contains(&("0 / 1 / 2 / 3 / 4 / 5", "Star rating")));
    }

    fn found_in(shortcuts: &'static [Shortcut], query: &str) -> Vec<Shortcut> {
        let (left, _) = filtered(&tokens(query), &[("Rating", shortcuts)], &[]);
        left.into_iter().flat_map(|(_, s)| s).collect()
    }

    #[test]
    fn option_reads_as_the_platform_names_it() {
        let expected = if cfg!(target_os = "macos") {
            "⌥"
        } else {
            "Alt"
        };
        assert_eq!(cap_label("Option"), expected);
    }
}
