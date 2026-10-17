//! A reference card of every shortcut, grouped like Lightroom's: Cmd+/ (as in
//! Lightroom) or the keyboard
//! button in the top bar. Each shortcut's keys are drawn as key caps.
use super::Editor;
use super::icons::{self, Icon};
use super::theme;
use super::widgets::modal_frame;
use crate::app::Module;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};

/// A shortcut: its keys and what it does. Keys are written "Cmd+Shift+Z";
/// " / " separates alternatives, and a word in lower case ("click") is an
/// action rather than a key.
type Shortcut = (&'static str, &'static str);
/// A titled group of shortcuts.
type Group<'a> = (&'a str, &'a [Shortcut]);

const LIBRARY: [Shortcut; 10] = [
    ("Arrows", "Move the active photo"),
    ("Home / End", "First / last photo"),
    ("Shift+Arrows / Shift+click", "Extend the selection"),
    ("Cmd+click", "Add or remove a photo"),
    ("Cmd+A", "Select all"),
    ("Cmd+D", "Select none"),
    ("/", "Deselect the active photo"),
    ("Cmd+L", "Turn the filters off / on"),
    ("J", "Cycle the grid cell style"),
    (
        "Cmd+Shift+S",
        "Sync the active photo's settings to the selection",
    ),
];
const LOUPE: [Shortcut; 7] = [
    ("E / Return / double-click", "Open the Loupe"),
    ("G / Esc / double-click", "Back to the grid"),
    ("Z / click", "Fit or the last zoom"),
    ("Cmd+= / Cmd+-", "Zoom in / out"),
    ("Cmd+Alt+0", "100%"),
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
const GENERAL: [Shortcut; 11] = [
    ("G", "Library"),
    ("D", "Develop"),
    ("Tab", "Hide / show the side panels"),
    ("Shift+Tab", "Hide / show the side panels and filmstrip"),
    ("F", "Hide all but the photo; grid first in the Library"),
    ("Esc", "Leave the whole-window mode"),
    ("F7 / F8 / F6", "Left panel / right panel / filmstrip"),
    ("Shift+F8", "Fold / open Presets and History"),
    ("Cmd+'", "Create virtual copy"),
    ("Cmd+,", "Preferences"),
    ("Cmd+/", "These shortcuts"),
];
const DEVELOP: [Shortcut; 49] = [
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
    ("right-click", "Panel header: Solo Mode"),
    ("E", "Show the photo in the Loupe"),
];

const WIDTH: f32 = 820.;
const COLUMN: f32 = 380.;
const KEYS: f32 = 165.;
const ROW: f32 = 26.;

impl Editor {
    pub(super) fn shortcuts_window(&mut self, ctx: &egui::Context) {
        let palette = theme::palette(ctx);
        if !self.view.shortcuts {
            return;
        }
        // Cmd+/ closes it again; other keys stay with the sheet.
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Slash)) {
            self.view.shortcuts = false;
            return;
        }
        let views: [Group; 4] = [
            ("Library grid", &LIBRARY),
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
        let (left, right) = (left.as_slice(), right.as_slice());
        let response = egui::Modal::new(egui::Id::new("keyboard-shortcuts"))
            .backdrop_color(Color32::from_black_alpha(140))
            .frame(modal_frame(&palette).inner_margin(egui::Margin::symmetric(28, 22)))
            .show(ctx, |ui| {
                ui.set_width(WIDTH);
                let mut close = false;
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Keyboard Shortcuts")
                            .size(17.)
                            .color(palette.gray(235)),
                    );
                    ui.add_space(10.);
                    ui.label(
                        egui::RichText::new(format!("{} shows this anytime", keys_text("Cmd+/")))
                            .size(12.)
                            .color(palette.gray(130)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        close = icon_button(ui, Icon::Close, "Close · Esc").clicked();
                    });
                });
                ui.add_space(14.);
                let height = (ctx.content_rect().height() * 0.8 - 90.).max(200.);
                egui::ScrollArea::vertical()
                    .max_height(height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = WIDTH - 2. * COLUMN;
                            for column in [left, right] {
                                ui.allocate_ui(Vec2::new(COLUMN, 0.), |ui| {
                                    ui.vertical(|ui| {
                                        for (title, shortcuts) in column {
                                            group(ui, title, shortcuts);
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
        }
    }
}

fn group(ui: &mut egui::Ui, title: &str, shortcuts: &[Shortcut]) {
    let palette = theme::palette(ui.ctx());
    ui.label(
        egui::RichText::new(title.to_uppercase())
            .size(10.5)
            .color(palette.gray(125)),
    );
    ui.add_space(6.);
    for (keys, action) in shortcuts {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(COLUMN, ROW), Sense::hover());
        // Key caps right-aligned in their column, the action after them.
        let mut x = rect.left() + KEYS;
        for (i, alternative) in keys
            .split(" / ")
            .enumerate()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            for cap in alternative.split('+').collect::<Vec<_>>().into_iter().rev() {
                x = key_cap(ui, cap, Pos2::new(x, rect.center().y)) - 3.;
            }
            if i > 0 {
                x -= 2.;
                ui.painter().text(
                    Pos2::new(x, rect.center().y),
                    Align2::RIGHT_CENTER,
                    "/",
                    FontId::proportional(11.),
                    palette.gray(100),
                );
                x -= 10.;
            }
        }
        ui.painter().text(
            Pos2::new(rect.left() + KEYS + 14., rect.center().y),
            Align2::LEFT_CENTER,
            *action,
            FontId::proportional(12.5),
            palette.gray(205),
        );
    }
    ui.add_space(16.);
}

/// Draws one key cap ending at `right`; returns its left edge. A word in
/// lower case is an action, drawn as plain text.
fn key_cap(ui: &egui::Ui, cap: &str, right: Pos2) -> f32 {
    let palette = theme::palette(ui.ctx());
    let label = cap_label(cap);
    let action = cap.chars().all(|c| c.is_lowercase() || c == '-') && cap.len() > 1;
    let galley = ui.painter().layout_no_wrap(
        label,
        FontId::proportional(if action { 11.5 } else { 11. }),
        palette.gray(if action { 150 } else { 230 }),
    );
    if action {
        let left = right.x - galley.size().x;
        ui.painter().galley(
            Pos2::new(left, right.y - galley.size().y / 2.),
            galley,
            Color32::WHITE,
        );
        return left;
    }
    let width = (galley.size().x + 10.).max(20.);
    let cap_rect = Rect::from_min_max(
        Pos2::new(right.x - width, right.y - 10.),
        Pos2::new(right.x, right.y + 10.),
    );
    ui.painter().rect(
        cap_rect,
        4.,
        palette.gray(46),
        Stroke::new(1., palette.gray(62)),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(
        cap_rect.center() - galley.size() / 2.,
        galley,
        Color32::WHITE,
    );
    cap_rect.left()
}

/// A small borderless icon button, as in the top bar.
pub(super) fn icon_button(ui: &mut egui::Ui, icon: Icon, hover: &str) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(26.), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, 4., palette.gray(45));
    }
    let color = palette.gray(if response.hovered() { 235 } else { 160 });
    icons::paint_at(ui.painter(), icon, rect.center(), 15., color);
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
        "Alt" if mac => "⌥".into(),
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
