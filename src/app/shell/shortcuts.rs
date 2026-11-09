//! A reference card of every shortcut, grouped like Lightroom's: Cmd+/ (as in
//! Lightroom) or the keyboard
//! button in the top bar. Each shortcut's keys are drawn as key caps.
use crate::app::Editor;
use crate::app::Module;
use crate::app::shell::keymap::{Binding, Chord, Effective, Mods};
use crate::app::ui::design::{self, Tone, inset, radius, space, text};
use crate::app::ui::icons::{self, Icon};
use crate::app::ui::theme;
use crate::app::ui::widgets::{modal_frame, search_field};
use eframe::egui::{self, Align2, Color32, FontId, Key, Pos2, Rect, Sense, Stroke, Vec2};

/// A shortcut: its keys and what it does. Keys are written "Cmd+Shift+Z";
/// " / " separates alternatives, and a word in lower case ("click") is an
/// action rather than a key. The keys are the registry's chords in effect
/// ([`row_keys`]), so the user's keys show.
type Shortcut = (String, &'static str);
/// A titled group of shortcuts.
type Group<'a> = (&'a str, &'a [Shortcut]);

/// What a card row's keys are made of.
#[derive(Clone, Copy)]
enum Part {
    /// A binding's chords in effect: every one of the user's for its command,
    /// or its defaults the card shows, each by its first key.
    Keys(&'static Binding),
    /// Every key of a binding's default chords, aliases too, or the user's.
    EveryKey(&'static Binding),
    /// A gesture, or keys no command has: "drag", "Shift+click".
    Text(&'static str),
}

/// How a row writes its chords, as the hand-written card did.
#[derive(Clone, Copy, Default)]
struct Spelling {
    /// Alt as "Option", as Lightroom's card writes its Develop shortcuts.
    option: bool,
    /// The arrow keys as arrows.
    arrows: bool,
    /// An alternative with the same modifiers as the one before it as its key
    /// alone: "Cmd+Option+Shift+H / S / L".
    short: bool,
}

/// A row of the card: its keys and what it does.
struct Row {
    parts: &'static [Part],
    action: &'static str,
    spelling: Spelling,
}

const fn row(parts: &'static [Part], action: &'static str) -> Row {
    Row {
        parts,
        action,
        spelling: Spelling {
            option: false,
            arrows: false,
            short: false,
        },
    }
}
/// A row written with Option, as Lightroom's Develop shortcuts are.
const fn option(parts: &'static [Part], action: &'static str) -> Row {
    Row {
        parts,
        action,
        spelling: Spelling {
            option: true,
            arrows: false,
            short: false,
        },
    }
}
/// [`option`], with arrows and the modifiers written once.
const fn option_arrows(parts: &'static [Part], action: &'static str) -> Row {
    Row {
        parts,
        action,
        spelling: Spelling {
            option: true,
            arrows: true,
            short: true,
        },
    }
}
/// [`option`], with the modifiers written once.
const fn option_short(parts: &'static [Part], action: &'static str) -> Row {
    Row {
        parts,
        action,
        spelling: Spelling {
            option: true,
            arrows: false,
            short: true,
        },
    }
}

use crate::app::shell::commands::registry::bindings as b;
use Part::{EveryKey, Keys, Text};

static LIBRARY: [Row; 17] = [
    row(
        &[
            Keys(&b::GRID_PREVIOUS),
            Keys(&b::GRID_NEXT),
            Keys(&b::GRID_ROW_UP),
            Keys(&b::GRID_ROW_DOWN),
        ],
        "Move the active photo",
    ),
    row(
        &[Keys(&b::GRID_PAGE_UP), Keys(&b::GRID_PAGE_DOWN)],
        "Move the active photo a page up / down",
    ),
    row(
        &[Keys(&b::GRID_FIRST), Keys(&b::GRID_LAST)],
        "First / last photo",
    ),
    row(
        &[Text("Shift+Arrows / Shift+click")],
        "Extend the selection",
    ),
    row(&[Text("Cmd+click")], "Add or remove a photo"),
    row(&[Keys(&b::GRID_SELECT_ALL)], "Select all"),
    row(&[Keys(&b::GRID_SELECT_NONE)], "Select none"),
    row(
        &[Keys(&b::GRID_DESELECT_ACTIVE)],
        "Deselect the active photo",
    ),
    row(&[Keys(&b::FILTERS)], "Turn the filters off / on"),
    row(&[Keys(&b::GRID_CELL_STYLE)], "Cycle the grid cell style"),
    row(
        &[Keys(&b::GRID_DELETE)],
        "Move to Trash, or remove from the catalog",
    ),
    row(
        &[Keys(&b::GRID_REMOVE_FROM_CATALOG)],
        "Remove from the catalog",
    ),
    row(&[Keys(&b::GRID_TRASH)], "Move to Trash"),
    row(
        &[Keys(&b::GRID_DELETE_REJECTED)],
        "Delete the rejected photos",
    ),
    row(
        &[Keys(&b::GRID_RENAME)],
        "Rename the photo, or the selected photos in capture order",
    ),
    row(
        &[Keys(&b::LIBRARY_SYNC)],
        "Sync the active photo's settings to the selection",
    ),
    row(
        &[Keys(&b::LIBRARY_AUTO)],
        "Auto Settings on each selected photo",
    ),
];
static LOUPE: [Row; 7] = [
    row(
        &[Keys(&b::GRID_LOUPE), Text("double-click")],
        "Open the Loupe",
    ),
    row(
        &[
            Keys(&b::LIBRARY),
            Keys(&b::LOUPE_CLOSE),
            Text("double-click"),
        ],
        "Back to the grid",
    ),
    row(&[Keys(&b::ZOOM), Text("click")], "Fit or the last zoom"),
    row(&[Keys(&b::ZOOM_IN), Keys(&b::ZOOM_OUT)], "Zoom in / out"),
    row(&[Keys(&b::ZOOM_100), Keys(&b::ZOOM_FIT)], "100% / Fit"),
    row(&[Text("drag")], "Pan"),
    row(&[Keys(&b::LOUPE_INFO)], "Cycle photo info"),
];
static COMPARE: [Row; 6] = [
    row(
        &[Keys(&b::GRID_COMPARE)],
        "Compare the active photo with the next",
    ),
    row(
        &[Keys(&b::COMPARE_PREVIOUS), Keys(&b::COMPARE_NEXT)],
        "Change the candidate",
    ),
    row(
        &[Keys(&b::COMPARE_MAKE_SELECT)],
        "Make the candidate the select",
    ),
    row(&[Keys(&b::COMPARE_SWAP)], "Swap select and candidate"),
    row(&[Text("click")], "Make a photo active for rating keys"),
    row(
        &[Keys(&b::LIBRARY), Keys(&b::COMPARE_CLOSE)],
        "Back to the grid",
    ),
];
static SURVEY: [Row; 5] = [
    row(&[Keys(&b::GRID_SURVEY)], "Survey the selected photos"),
    row(
        &[EveryKey(&b::SURVEY_PREVIOUS), EveryKey(&b::SURVEY_NEXT)],
        "Move the active photo",
    ),
    row(&[Text("click")], "Make a photo active for rating keys"),
    row(&[Text("Cmd+click")], "Take a photo out"),
    row(
        &[Keys(&b::LIBRARY), Keys(&b::SURVEY_CLOSE)],
        "Back to the grid",
    ),
];
static RATING: [Row; 8] = [
    row(
        &[
            Keys(&b::RATING_0),
            Keys(&b::RATING_1),
            Keys(&b::RATING_2),
            Keys(&b::RATING_3),
            Keys(&b::RATING_4),
            Keys(&b::RATING_5),
        ],
        "Star rating",
    ),
    row(
        &[Keys(&b::RATING_LOWER), Keys(&b::RATING_RAISE)],
        "Lower / raise the rating",
    ),
    row(
        &[
            Keys(&b::LABEL_RED),
            Keys(&b::LABEL_YELLOW),
            Keys(&b::LABEL_GREEN),
            Keys(&b::LABEL_BLUE),
        ],
        "Red, yellow, green, blue",
    ),
    row(
        &[Keys(&b::PICK), Keys(&b::REJECT), Keys(&b::UNFLAG)],
        "Pick / reject / unflag",
    ),
    row(&[Keys(&b::TOGGLE_PICK)], "Toggle pick"),
    row(
        &[Keys(&b::FLAG_RAISE), Keys(&b::FLAG_LOWER)],
        "Raise / lower the flag",
    ),
    row(&[Text("Shift+key")], "Apply and go to the next photo"),
    row(
        &[Keys(&b::LIBRARY_UNDO), Keys(&b::LIBRARY_REDO)],
        "Undo / redo, anywhere",
    ),
];
static QUICK: [Row; 3] = [
    row(&[Keys(&b::GRID_QUICK_TOGGLE)], "Add to or take out of it"),
    row(&[Keys(&b::GRID_QUICK_SHOW)], "Show it"),
    row(&[Keys(&b::GRID_QUICK_CLEAR)], "Clear it"),
];
static STACKS: [Row; 4] = [
    row(&[Keys(&b::STACK_GROUP)], "Group into stack"),
    row(&[Keys(&b::STACK_UNSTACK)], "Unstack"),
    row(
        &[Keys(&b::GRID_STACK_TOGGLE)],
        "Collapse / expand the stack",
    ),
    row(
        &[Keys(&b::GRID_STACK_TO_TOP)],
        "Move to the top of the stack",
    ),
];
static GENERAL: [Row; 12] = [
    row(&[Keys(&b::LIBRARY)], "Library"),
    row(&[Keys(&b::DEVELOP)], "Develop"),
    row(&[Keys(&b::PANELS_SIDES)], "Hide / show the side panels"),
    row(
        &[Keys(&b::PANELS_ALL)],
        "Hide / show the side panels and filmstrip",
    ),
    row(
        &[Keys(&b::CHROME)],
        "Hide all but the photo; grid first in the Library",
    ),
    row(&[Keys(&b::SHOW_CHROME)], "Leave the whole-window mode"),
    row(
        &[Keys(&b::DISMISS_MESSAGES)],
        "Dismiss the messages in the top bar",
    ),
    row(
        &[
            Keys(&b::PANELS_LEFT),
            Keys(&b::PANELS_RIGHT),
            Keys(&b::PANELS_FILMSTRIP),
        ],
        "Left panel / right panel / filmstrip",
    ),
    row(
        &[Keys(&b::PANELS_PRESETS)],
        "Fold / open Presets and History",
    ),
    row(&[Keys(&b::VIRTUAL_COPY)], "Create virtual copy"),
    row(&[Keys(&b::PREFERENCES)], "Preferences"),
    row(&[Keys(&b::SHORTCUTS)], "These shortcuts"),
];
static DEVELOP: [Row; 55] = [
    row(&[Keys(&b::CROP)], "Crop & Straighten"),
    row(&[Keys(&b::FINISH_CROP)], "Finish the crop"),
    row(
        &[Keys(&b::CROP_SWAP_ORIENTATION)],
        "Swap the crop's orientation",
    ),
    row(
        &[Keys(&b::CROP_ASPECT_LOCK)],
        "Lock or unlock the crop's aspect",
    ),
    row(
        &[Keys(&b::CROP_NEXT_OVERLAY), Keys(&b::CROP_TURN_OVERLAY)],
        "Next crop overlay / turn it",
    ),
    row(&[Text("Cmd+drag")], "Straighten along a line"),
    row(&[Keys(&b::GUIDED_UPRIGHT)], "Guided Upright: draw guides"),
    row(
        &[Keys(&b::REMOVE_DELETE)],
        "Delete the selected spot, guide or red eye",
    ),
    row(&[Keys(&b::REMOVE)], "Spot removal"),
    row(&[Keys(&b::MASK)], "Masking"),
    row(&[Keys(&b::WHITE_BALANCE)], "White balance selector"),
    option(
        &[Keys(&b::TARGETED_TONE_CURVE)],
        "Targeted adjustment: Tone Curve",
    ),
    option_short(
        &[
            Keys(&b::TARGETED_HUE),
            Keys(&b::TARGETED_SATURATION),
            Keys(&b::TARGETED_LUMINANCE),
        ],
        "Targeted adjustment: Hue / Saturation / Luminance",
    ),
    option(
        &[Keys(&b::TARGETED_BLACK_WHITE)],
        "Targeted adjustment: B&W mix",
    ),
    row(&[Keys(&b::CLOSE_TOOL)], "Put the open tool away"),
    row(
        &[Keys(&b::REMOVE_SMALLER), Keys(&b::REMOVE_LARGER)],
        "Brush or red eye size (Shift: feather)",
    ),
    row(
        &[Text("Scroll")],
        "Brush or red eye size over the photo (Shift: feather)",
    ),
    row(
        &[Keys(&b::REMOVE_NEW_SOURCE)],
        "New source for the selected spot",
    ),
    row(&[Keys(&b::REMOVE_HIDE_PINS)], "Hide spot pins"),
    row(&[Keys(&b::REMOVE_VISUALIZE)], "Visualize spots"),
    row(&[Text("Space")], "Pan while a tool is open"),
    row(&[Keys(&b::BEFORE_ONLY)], "Before alone"),
    row(
        &[Keys(&b::CAMERA_THUMBNAIL)],
        "Camera thumbnail (the embedded JPEG)",
    ),
    row(
        &[Keys(&b::BEFORE_AFTER_LEFT_RIGHT)],
        "Before and after, left and right",
    ),
    option(
        &[Keys(&b::BEFORE_AFTER_TOP_BOTTOM)],
        "Before and after, top and bottom",
    ),
    row(&[Keys(&b::BEFORE_AFTER_SPLIT)], "Before and after, split"),
    option_arrows(
        &[Keys(&b::COPY_TO_AFTER), Keys(&b::COPY_TO_BEFORE)],
        "Copy Before to After / After to Before",
    ),
    option_arrows(&[Keys(&b::SWAP_BEFORE_AFTER)], "Swap Before and After"),
    row(&[Keys(&b::REFERENCE_VIEW)], "Reference View"),
    row(
        &[Keys(&b::DEVELOP_INFO)],
        "Photo info overlay: Info 1, Info 2, off",
    ),
    row(&[Keys(&b::CLIPPING)], "Show shadow and highlight clipping"),
    row(&[Keys(&b::ZOOM)], "Toggle zoom / fit"),
    row(
        &[
            Keys(&b::ZOOM_IN),
            Keys(&b::ZOOM_OUT),
            Keys(&b::ZOOM_100),
            Keys(&b::ZOOM_FIT),
        ],
        "Zoom in / out / 100% / Fit",
    ),
    row(
        &[Keys(&b::MAGNIFIER)],
        "Magnifier over the photo; wheel: magnification",
    ),
    row(
        &[Keys(&b::PREVIOUS), Keys(&b::NEXT)],
        "Previous / next photo",
    ),
    row(
        &[Keys(&b::DEVELOP_SELECT_ALL)],
        "Select all in the filmstrip, for Sync",
    ),
    row(
        &[Keys(&b::DEVELOP_SELECT_NONE)],
        "Select none in the filmstrip",
    ),
    row(
        &[Keys(&b::COPY_SETTINGS), Keys(&b::PASTE_SETTINGS)],
        "Copy / paste settings",
    ),
    option(
        &[Keys(&b::PASTE_PREVIOUS)],
        "Paste settings from previous photo",
    ),
    row(
        &[Keys(&b::DEVELOP_SYNC)],
        "Sync settings to the selected photos",
    ),
    option(&[Keys(&b::MATCH_EXPOSURES)], "Match total exposures"),
    row(&[Keys(&b::NEW_PRESET)], "New Develop Preset"),
    row(&[Keys(&b::RESET)], "Reset all settings"),
    row(&[Keys(&b::DEVELOP_AUTO)], "Auto tone"),
    row(
        &[Keys(&b::TOGGLE_BLACK_WHITE)],
        "Convert to black & white or back to color",
    ),
    row(&[Keys(&b::DEVELOP_EXPORT)], "Export…"),
    row(&[Keys(&b::DEVELOP_EXPORT_PREVIOUS)], "Export with previous"),
    row(&[Text("double-click")], "Reset a slider or grading wheel"),
    row(
        &[Text("Shift+drag")],
        "Grading wheel: hue or saturation only",
    ),
    row(&[Text("Cmd+drag")], "Grading wheel: fine adjustment"),
    row(&[Text("Up / Down")], "Nudge the hovered slider (Shift: 10)"),
    row(
        &[Keys(&b::BASIC_NEXT), Keys(&b::BASIC_PREVIOUS)],
        "Select the next / previous Basic slider",
    ),
    row(
        &[Keys(&b::BASIC_INCREASE), Keys(&b::BASIC_DECREASE)],
        "Step the selected Basic slider (Shift: more, Option: less)",
    ),
    row(&[Text("right-click")], "Panel header: Solo Mode"),
    row(
        &[Keys(&b::LOUPE_FROM_DEVELOP)],
        "Show the photo in the Loupe",
    ),
];

/// The chords a part of a row stands for in `map`.
fn part_chords(part: &Part, map: &Effective) -> Vec<Chord> {
    let macos = cfg!(target_os = "macos");
    match part {
        Keys(binding) if !map.overrides(binding.command) => binding
            .defaults
            .iter()
            .filter(|d| d.card && d.exists_on(macos))
            .filter_map(|d| d.chords().next())
            .collect(),
        Keys(binding) | EveryKey(binding) => map
            .defaults_of(binding)
            .iter()
            .filter(|d| d.exists_on(macos))
            .flat_map(|d| d.chords().collect::<Vec<_>>())
            .collect(),
        Text(_) => Vec::new(),
    }
}

/// A row's keys in `map`, written as the card writes them: alternatives
/// apart by " / ", the four arrows as "Arrows".
fn row_keys(row: &Row, map: &Effective) -> String {
    let spelling = row.spelling;
    let mut alternatives: Vec<String> = Vec::new();
    let mut chords: Vec<Chord> = Vec::new();
    let mut texts: Vec<&str> = Vec::new();
    for part in row.parts {
        match part {
            Text(text) => texts.push(text),
            part => chords.extend(part_chords(part, map)),
        }
    }
    let arrows = [
        Key::ArrowLeft,
        Key::ArrowRight,
        Key::ArrowUp,
        Key::ArrowDown,
    ];
    let all_arrows = chords.len() == 4
        && arrows.iter().all(|a| chords.iter().any(|c| c.key == *a))
        && chords.iter().all(|c| c.mods == chords[0].mods);
    if all_arrows {
        let prefix = Chord::new(Key::A, chords[0].mods).to_string();
        alternatives.push(format!("{}Arrows", prefix.trim_end_matches('A')));
    } else {
        let mut before: Option<Mods> = None;
        for chord in &chords {
            let mut text = chord.to_string();
            if spelling.short && before == Some(chord.mods) && chord.mods != Mods::NONE {
                text = Chord::new(chord.key, Mods::NONE).to_string();
            }
            if spelling.option {
                text = text.replace("Alt+", "Option+");
            }
            if spelling.arrows {
                for (name, arrow) in [("Left", "←"), ("Right", "→"), ("Up", "↑"), ("Down", "↓")]
                {
                    if let Some(mods) = text.strip_suffix(name) {
                        text = format!("{mods}{arrow}");
                    }
                }
            }
            before = Some(chord.mods);
            alternatives.push(text);
        }
    }
    alternatives.extend(texts.iter().map(|t| t.to_string()));
    alternatives.join(" / ")
}

/// The card's rows with their keys in `map`.
fn shortcuts(rows: &[Row], map: &Effective) -> Vec<Shortcut> {
    rows.iter()
        .map(|row| (row_keys(row, map), row.action))
        .collect()
}

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
    pub(in crate::app) fn shortcuts_window(&mut self, ctx: &egui::Context) {
        let palette = theme::palette(ctx);
        if !self.view.shortcuts {
            ctx.data_mut(|d| d.remove::<String>(query_id()));
            return;
        }
        // Cmd+/ closes it again; other keys stay with the sheet.
        use crate::app::shell::commands::registry::bindings;
        let keys = self.keys(ctx);
        if keys.take(&bindings::SHORTCUTS_CLOSE).is_some() {
            self.view.shortcuts = false;
            return;
        }
        let stored = ctx.data(|d| d.get_temp::<String>(query_id()));
        let opened = stored.is_none();
        let mut query = stored.unwrap_or_default();
        // Esc clears a search before it closes the sheet, and typing goes on.
        let cleared = !query.is_empty() && keys.take(&bindings::SHORTCUTS_CLEAR_SEARCH).is_some();
        if cleared {
            query.clear();
        }
        let map = self.keymap.effective();
        let [
            library,
            stacks,
            loupe,
            compare,
            survey,
            rating,
            quick,
            general,
            develop,
        ] = [
            &LIBRARY[..],
            &STACKS,
            &LOUPE,
            &COMPARE,
            &SURVEY,
            &RATING,
            &QUICK,
            &GENERAL,
            &DEVELOP,
        ]
        .map(|rows| shortcuts(rows, &map));
        let views: [Group; 5] = [
            ("Library grid", &library),
            (
                "Stacks, in the grid; S and Shift+S in the Loupe too",
                &stacks,
            ),
            ("Loupe", &loupe),
            ("Compare", &compare),
            ("Survey", &survey),
        ];
        let common: [Group; 3] = [
            ("Rating, flags and labels", &rating),
            ("Quick Collection", &quick),
            ("Everywhere", &general),
        ];
        let develop: [Group; 1] = [("Develop", &develop)];
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
            .cloned()
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
/// both find Cmd+Z. Preferences > Shortcuts searches the same way.
pub(in crate::app) fn tokens(query: &str) -> Vec<String> {
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
    search_matches(tokens, title, keys, action)
}
/// Whether a shortcut with `keys` (written "Cmd+Shift+Z", " / " between
/// alternatives) that does `action`, in the group `title`, has every word of
/// the search ([`tokens`]).
pub(in crate::app) fn search_matches(
    tokens: &[String],
    title: &str,
    keys: &str,
    action: &str,
) -> bool {
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
pub(in crate::app) fn icon_button(ui: &mut egui::Ui, icon: Icon, hover: &str) -> egui::Response {
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
pub(in crate::app) fn cap_label(cap: &str) -> String {
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
pub(in crate::app) fn keys_text(keys: &str) -> String {
    keys.split('+').map(cap_label).collect()
}

#[cfg(test)]
mod today;

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults(rows: &[Row]) -> Vec<Shortcut> {
        shortcuts(rows, &Effective::default())
    }
    fn found(query: &str) -> Vec<Shortcut> {
        let (library, loupe, develop) = (defaults(&LIBRARY), defaults(&LOUPE), defaults(&DEVELOP));
        let groups: [Group; 3] = [
            ("Library grid", &library),
            ("Loupe", &loupe),
            ("Develop", &develop),
        ];
        let (left, right) = filtered(&tokens(query), &groups, &[]);
        left.into_iter().chain(right).flat_map(|(_, s)| s).collect()
    }
    fn has(found: &[Shortcut], keys: &str, action: &str) -> bool {
        found.iter().any(|(k, a)| k == keys && *a == action)
    }

    #[test]
    fn with_the_defaults_the_card_reads_as_it_was_written() {
        use super::today;
        let today = |rows: &[(&str, &'static str)]| -> Vec<Shortcut> {
            rows.iter().map(|(k, a)| (k.to_string(), *a)).collect()
        };
        for (name, generated, written) in [
            ("library", defaults(&LIBRARY), today(&today::LIBRARY)),
            ("loupe", defaults(&LOUPE), today(&today::LOUPE)),
            ("compare", defaults(&COMPARE), today(&today::COMPARE)),
            ("survey", defaults(&SURVEY), today(&today::SURVEY)),
            ("rating", defaults(&RATING), today(&today::RATING)),
            ("quick", defaults(&QUICK), today(&today::QUICK)),
            ("stacks", defaults(&STACKS), today(&today::STACKS)),
            ("general", defaults(&GENERAL), today(&today::GENERAL)),
            ("develop", defaults(&DEVELOP), today(&today::DEVELOP)),
        ] {
            assert_eq!(generated, written, "{name}");
        }
    }

    #[test]
    fn the_users_keys_show_on_the_card() {
        let chord = |t: &str| Chord::parse(t).expect("a chord");
        let map = Effective::new(
            [
                ("crop", vec![chord("F9")]),
                ("delete", vec![chord("Cmd+Backspace"), chord("X")]),
                ("compare", vec![]),
                ("previous", vec![chord("F10")]),
            ]
            .into_iter()
            .collect(),
        );
        let develop = shortcuts(&DEVELOP, &map);
        assert!(has(&develop, "F9", "Crop & Straighten"));
        assert!(has(&develop, "", "Before alone"), "no key");
        assert!(has(&develop, "F10 / Right", "Previous / next photo"));
        let library = shortcuts(&LIBRARY, &map);
        assert!(has(
            &library,
            "Cmd+Backspace / X",
            "Move to Trash, or remove from the catalog"
        ));
        // No longer the four arrows.
        assert!(has(
            &library,
            "F10 / Right / Up / Down",
            "Move the active photo"
        ));
        let survey = shortcuts(&SURVEY, &map);
        assert!(has(&survey, "F10 / Right / Down", "Move the active photo"));
    }

    #[test]
    fn a_search_finds_actions_by_word_and_shortcuts_by_key() {
        assert!(has(&found("crop"), "R", "Crop & Straighten"));
        assert!(has(&found("straigh"), "R", "Crop & Straighten"));
        // Every spelling of a modifier names the same key.
        for query in ["cmd+shift+e", "⌘⇧E", "command shift e", "ctrl shift e"] {
            assert!(has(&found(query), "Cmd+Shift+E", "Export…"), "{query}");
        }
        assert!(has(
            &found("option y"),
            "Option+Y",
            "Before and after, top and bottom"
        ));
        assert!(has(
            &found("alt y"),
            "Option+Y",
            "Before and after, top and bottom"
        ));
        // A word starts where the search does: "rat" is no saturation.
        assert!(!has(
            &found("rat"),
            "Cmd+Option+Shift+H / S / L",
            "Targeted adjustment: Hue / Saturation / Luminance"
        ));
        assert!(has(&found("up"), "Arrows", "Move the active photo"));
    }

    #[test]
    fn one_letter_names_a_key_not_a_word() {
        let z = found("z");
        assert!(has(&z, "Z", "Toggle zoom / fit"));
        // "Zoom in / out" is Cmd+= / Cmd+-, no Z key.
        assert!(!has(&z, "Cmd+= / Cmd+-", "Zoom in / out"));
    }

    #[test]
    fn a_group_title_finds_its_shortcuts_and_nothing_left_is_empty() {
        assert_eq!(found("loupe").len(), LOUPE.len() + 1, "and Develop's E");
        assert!(found("nothing-like-this").is_empty());
        assert_eq!(found("").len(), LIBRARY.len() + LOUPE.len() + DEVELOP.len());
    }

    #[test]
    fn a_search_shares_the_groups_between_both_columns() {
        let (library, loupe) = (defaults(&LIBRARY), defaults(&LOUPE));
        let groups: [Group; 2] = [("Library grid", &library), ("Loupe", &loupe)];
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
        let rating = defaults(&RATING);
        let (left, _) = filtered(&tokens("3"), &[("Rating", &rating)], &[]);
        let found: Vec<Shortcut> = left.into_iter().flat_map(|(_, s)| s).collect();
        assert!(has(&found, "0 / 1 / 2 / 3 / 4 / 5", "Star rating"));
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

#[cfg(test)]
#[path = "shortcuts_editor_tests.rs"]
mod editor_tests;
