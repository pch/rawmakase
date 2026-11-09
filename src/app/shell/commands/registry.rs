//! Every command a key runs, and the keys that run it in each context.
//!
//! A command's id names it to the key map, automation and MIDI: the
//! [`Action`](super::Action) names are kept as they are, and new ids follow
//! their style (lower-case snake_case words, `:` before a parameter, never `/`,
//! which will separate an extension's name from its command). A [`Binding`]
//! says where a key runs the command and how its press is matched; the handler
//! that runs it names it (`bindings::CLIPPING`) and takes its press from
//! [`FrameKeys`](crate::app::shell::keymap::FrameKeys).
use crate::app::shell::keymap::{Binding, Context, Default, Mods, Tool};
use eframe::egui::Key;

/// A command: its id and what it does.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "Preferences > Shortcuts and the generated card read the commands (#963)"
    )
)]
pub(in crate::app) struct Command {
    pub id: &'static str,
    /// What it does, for Preferences and for a message naming it.
    pub title: &'static str,
    /// False for the text-editing keys (Undo, Redo, Select All), which are the
    /// Edit menu's on macOS.
    pub rebindable: bool,
}

#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "Preferences > Shortcuts and the generated card read the commands (#963)"
    )
)]
const fn command(id: &'static str, title: &'static str) -> Command {
    Command {
        id,
        title,
        rebindable: true,
    }
}
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "Preferences > Shortcuts and the generated card read the commands (#963)"
    )
)]
const fn fixed(id: &'static str, title: &'static str) -> Command {
    Command {
        id,
        title,
        rebindable: false,
    }
}

#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "Preferences > Shortcuts and the generated card read the commands (#963)"
    )
)]
pub(in crate::app) static COMMANDS: &[Command] = &[
    // The automation and MIDI actions (`Action::NAMED`), in their order.
    fixed("undo", "Undo"),
    fixed("redo", "Redo"),
    command("rating:0", "No stars"),
    command("rating:1", "One star"),
    command("rating:2", "Two stars"),
    command("rating:3", "Three stars"),
    command("rating:4", "Four stars"),
    command("rating:5", "Five stars"),
    command("pick", "Pick"),
    command("reject", "Reject"),
    command("unflag", "Unflag"),
    command("label:red", "Label red"),
    command("label:yellow", "Label yellow"),
    command("label:green", "Label green"),
    command("label:blue", "Label blue"),
    command("label:purple", "Label purple"),
    command("label:none", "No label"),
    command("toggle_label:red", "Toggle the red label"),
    command("toggle_label:yellow", "Toggle the yellow label"),
    command("toggle_label:green", "Toggle the green label"),
    command("toggle_label:blue", "Toggle the blue label"),
    command("toggle_label:purple", "Toggle the purple label"),
    command("copy", "Copy settings"),
    command("paste", "Paste settings"),
    command("paste_previous", "Paste settings from previous photo"),
    command("sync", "Sync settings"),
    command("reset", "Reset all settings"),
    command("auto_tone", "Auto settings"),
    command("auto_white_balance", "Auto white balance"),
    command("curve:linear", "Tone curve: linear"),
    command("curve:medium_contrast", "Tone curve: medium contrast"),
    command("curve:strong_contrast", "Tone curve: strong contrast"),
    command("export_dialog", "Export…"),
    command("export_previous", "Export with previous"),
    command("toggle:bw", "Convert to black & white or back to color"),
    command("treatment:bw", "Black & white"),
    command("treatment:color", "Color"),
    command("compare", "Before alone"),
    command("camera_thumbnail", "Camera thumbnail (the embedded JPEG)"),
    command("clipping", "Show shadow and highlight clipping"),
    command("zoom", "Toggle zoom / fit"),
    command("fit", "Fit"),
    command("crop", "Crop & Straighten"),
    command("remove", "Spot removal"),
    command("white_balance", "White balance selector"),
    command("mask", "Masking"),
    command("mixer:hue", "Color mixer: hue"),
    command("mixer:sat", "Color mixer: saturation"),
    command("mixer:lum", "Color mixer: luminance"),
    command("library", "Library"),
    command("develop", "Develop"),
    command("loupe", "Open the Loupe"),
    command("next", "Next photo"),
    command("previous", "Previous photo"),
    command("panels:sides", "Hide / show the side panels"),
    command("panels:all", "Hide / show the side panels and filmstrip"),
    command("panels:left", "Left panel"),
    command("panels:right", "Right panel"),
    command("panels:filmstrip", "Filmstrip"),
    command("panels:presets", "Fold / open Presets and History"),
    command("chrome", "Hide all but the photo"),
    // Keys only, for now.
    command(
        "messages:dismiss_all",
        "Dismiss the messages in the top bar",
    ),
    command("chrome:show", "Leave the whole-window mode"),
    command("virtual_copy", "Create virtual copy"),
    command("preferences", "Preferences"),
    command("shortcuts", "Keyboard shortcuts"),
    command("shortcuts:clear_search", "Clear the shortcuts search"),
    command("flag:raise", "Raise the flag"),
    command("flag:lower", "Lower the flag"),
    command("toggle_pick", "Toggle pick"),
    command("rating:lower", "Lower the rating"),
    command("rating:raise", "Raise the rating"),
    command("filters", "Turn the filters off / on"),
    command("library:compare", "Compare the active photo with the next"),
    command("library:survey", "Survey the selected photos"),
    command("close_view", "Back to the grid"),
    command("grid:cell_style", "Cycle the grid cell style"),
    command("grid:row_up", "Move the active photo a row up"),
    command("grid:row_down", "Move the active photo a row down"),
    command("grid:page_up", "Move the active photo a page up"),
    command("grid:page_down", "Move the active photo a page down"),
    command("first_photo", "First photo"),
    command("last_photo", "Last photo"),
    command("quick:toggle", "Add to or take out of the Quick Collection"),
    command("quick:show", "Show the Quick Collection"),
    command("quick:clear", "Clear the Quick Collection"),
    command("delete", "Move to Trash, or remove from the catalog"),
    command("remove_from_catalog", "Remove from the catalog"),
    command("delete_rejected", "Delete the rejected photos"),
    command("trash", "Move to Trash"),
    command("rename", "Rename the photo, or the selected photos"),
    command("stack:group", "Group into stack"),
    command("stack:unstack", "Unstack"),
    command("stack:toggle", "Collapse / expand the stack"),
    command("stack:to_top", "Move to the top of the stack"),
    fixed("select_all", "Select all"),
    command("select_none", "Select none"),
    command("deselect_active", "Deselect the active photo"),
    command("info", "Cycle photo info"),
    command("compare_view:previous_candidate", "Previous candidate"),
    command("compare_view:next_candidate", "Next candidate"),
    command("compare_view:make_select", "Make the candidate the select"),
    command("compare_view:swap", "Swap select and candidate"),
    command("zoom:100", "Zoom to 100%"),
    command("zoom:in", "Zoom in"),
    command("zoom:out", "Zoom out"),
    command("magnifier", "Magnifier over the photo"),
    command("before_after:copy_to_before", "Copy After to Before"),
    command("before_after:copy_to_after", "Copy Before to After"),
    command("before_after:swap", "Swap Before and After"),
    command(
        "before_after:left_right",
        "Before and after, left and right",
    ),
    command(
        "before_after:top_bottom",
        "Before and after, top and bottom",
    ),
    command("before_after:split", "Before and after, split"),
    command("match_exposures", "Match total exposures"),
    command("new_preset", "New Develop Preset"),
    command("targeted:tone_curve", "Targeted adjustment: Tone Curve"),
    command("targeted:hue", "Targeted adjustment: Hue"),
    command("targeted:saturation", "Targeted adjustment: Saturation"),
    command("targeted:luminance", "Targeted adjustment: Luminance"),
    command("targeted:black_white", "Targeted adjustment: B&W mix"),
    command("reference_view", "Reference View"),
    command("guided_upright", "Guided Upright: draw guides"),
    command("tool:finish", "Finish the crop or the guides"),
    command("tool:close", "Put the open tool away"),
    command(
        "tool:delete",
        "Delete the selected spot, guide, red eye or mask",
    ),
    command("brush:smaller", "Smaller brush or red eye (Shift: feather)"),
    command("brush:larger", "Larger brush or red eye (Shift: feather)"),
    command("crop:swap_orientation", "Swap the crop's orientation"),
    command("crop:aspect_lock", "Lock or unlock the crop's aspect"),
    command("crop:next_overlay", "Next crop overlay"),
    command("crop:turn_overlay", "Turn the crop overlay"),
    command("remove:new_source", "New source for the selected spot"),
    command("remove:hide_pins", "Hide spot pins"),
    command("remove:visualize", "Visualize spots"),
    command(
        "mask:back",
        "Stop aiming, drop the mask not yet drawn, or close Masking",
    ),
    command("mask:overlay", "Show or hide the mask overlay"),
    command("mask:switch_brush", "Switch between brushes A and B"),
    command("mask:new_brush", "New brush mask"),
    command("mask:new_linear", "New linear gradient mask"),
    command("mask:new_radial", "New radial gradient mask"),
    command("mask:new_color_range", "New color range mask"),
    command("basic:next", "Select the next Basic slider"),
    command("basic:previous", "Select the previous Basic slider"),
    command(
        "basic:increase",
        "Raise the selected Basic slider (Shift: more, Option: less)",
    ),
    command(
        "basic:decrease",
        "Lower the selected Basic slider (Shift: more, Option: less)",
    ),
];

/// The command `id` names.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "Preferences > Shortcuts and the generated card read the commands (#963)"
    )
)]
pub(in crate::app) fn command_by_id(id: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.id == id)
}

/// Declares each binding as a static, and [`bindings::ALL`] as all of them.
macro_rules! bindings {
    ($($(#[$doc:meta])* $name:ident = $value:expr;)*) => {
        $($(#[$doc])* pub(in crate::app) static $name: Binding = $value;)*
        /// Every binding, for the registry's checks.
        #[cfg_attr(not(test), allow(dead_code, reason = "the registry's tests check them all"))]
        pub(in crate::app) static ALL: &[&Binding] = &[$(&$name),*];
    };
}

/// The bindings, named for the handler that takes their presses. Each chord
/// names its modifiers exactly. A binding's variants are passed to its
/// command, and on a punctuation key Shift and Alt are free unless the chord
/// names or rejects them ([`Chord::free`](crate::app::shell::keymap::Chord::free)).
pub(in crate::app) mod bindings {
    use super::*;
    use Context::{
        Compare, Develop, Everywhere, Grid, Library, Loupe, LoupeZoom, ShortcutSheet, Survey,
    };

    const NONE: Mods = Mods::NONE;
    const CMD: Mods = Mods::CMD;
    const ALT: Mods = Mods::ALT;
    const SHIFT: Mods = Mods::SHIFT;
    const CMD_SHIFT: Mods = CMD.with(SHIFT);
    const CMD_ALT: Mods = CMD.with(ALT);
    const CMD_ALT_SHIFT: Mods = CMD_ALT.with(SHIFT);
    const ALT_SHIFT: Mods = ALT.with(SHIFT);

    const fn d(keys: &'static [Key], mods: Mods) -> Default {
        Default::new(keys, mods)
    }
    const fn b(command: &'static str, context: Context, defaults: &'static [Default]) -> Binding {
        Binding::new(command, context, defaults)
    }
    /// A rating, flag or label key: by what it types, the number row by its
    /// place, once per press. Alt types other symbols on the punctuation keys.
    /// Its binding passes Shift on, to go on to the next photo.
    const fn rate(keys: &'static [Key]) -> Default {
        Default::new(keys, NONE)
            .typed()
            .once_per_press()
            .rejecting(ALT)
    }
    /// The rating, flag and label keys' binding, Shift going on to the next photo.
    const fn rating(command: &'static str, defaults: &'static [Default]) -> Binding {
        b(command, Everywhere, defaults).variant(SHIFT)
    }
    const DELETE_KEYS: &[Key] = &[Key::Delete, Key::Backspace];
    const OPEN_BRACKETS: &[Key] = &[Key::OpenBracket, Key::OpenCurlyBracket];
    const CLOSE_BRACKETS: &[Key] = &[Key::CloseBracket, Key::CloseCurlyBracket];
    const SLASHES: &[Key] = &[Key::Slash, Key::Questionmark];

    bindings! {
        // The message line and the window (`Editor::draw`).
        DISMISS_MESSAGES = b("messages:dismiss_all", Everywhere, &[
            d(&[Key::Escape], SHIFT),
        ]).consuming();
        SHOW_CHROME = b("chrome:show", Everywhere, &[d(&[Key::Escape], NONE)]).consuming();

        // Rating, flag and label keys (`photo_metadata::shortcut`), in the
        // order they are matched.
        FLAG_RAISE = b("flag:raise", Everywhere, &[d(&[Key::ArrowUp], CMD).once_per_press()]);
        FLAG_LOWER = b("flag:lower", Everywhere, &[d(&[Key::ArrowDown], CMD).once_per_press()]);
        RATING_0 = rating("rating:0", &[rate(&[Key::Num0])]);
        RATING_1 = rating("rating:1", &[rate(&[Key::Num1])]);
        RATING_2 = rating("rating:2", &[rate(&[Key::Num2])]);
        RATING_3 = rating("rating:3", &[rate(&[Key::Num3])]);
        RATING_4 = rating("rating:4", &[rate(&[Key::Num4])]);
        RATING_5 = rating("rating:5", &[rate(&[Key::Num5])]);
        LABEL_RED = rating("toggle_label:red", &[rate(&[Key::Num6])]);
        LABEL_YELLOW = rating("toggle_label:yellow", &[rate(&[Key::Num7])]);
        LABEL_GREEN = rating("toggle_label:green", &[rate(&[Key::Num8])]);
        LABEL_BLUE = rating("toggle_label:blue", &[rate(&[Key::Num9])]);
        PICK = rating("pick", &[rate(&[Key::P])]);
        UNFLAG = rating("unflag", &[rate(&[Key::U])]);
        REJECT = rating("reject", &[rate(&[Key::X])]);
        TOGGLE_PICK = rating("toggle_pick", &[rate(&[Key::Backtick])]);
        // Shift+[ and Shift+] type `{` and `}` on US layouts.
        RATING_LOWER = rating("rating:lower", &[rate(OPEN_BRACKETS)]);
        RATING_RAISE = rating("rating:raise", &[rate(CLOSE_BRACKETS)]);

        // The Library's views (`Library::selection_keys`).
        FILTERS = b("filters", Library, &[d(&[Key::L], CMD).once_per_press()]);
        GRID_LOUPE = b("loupe", Grid, &[d(&[Key::E], NONE), d(&[Key::Enter], NONE)]);
        GRID_COMPARE = b("library:compare", Grid, &[d(&[Key::C], NONE)]);
        GRID_SURVEY = b("library:survey", Grid, &[d(&[Key::N], NONE)]);
        GRID_CELL_STYLE = b("grid:cell_style", Grid, &[d(&[Key::J], NONE).once_per_press()]);
        GRID_QUICK_TOGGLE = b("quick:toggle", Grid, &[d(&[Key::B], NONE).once_per_press()]);
        GRID_QUICK_SHOW = b("quick:show", Grid, &[d(&[Key::B], CMD).once_per_press()]);
        GRID_QUICK_CLEAR = b("quick:clear", Grid, &[d(&[Key::B], CMD_SHIFT).once_per_press()]);
        GRID_DELETE = b("delete", Grid, &[d(DELETE_KEYS, NONE).once_per_press()]);
        GRID_REMOVE_FROM_CATALOG = b("remove_from_catalog", Grid, &[
            d(DELETE_KEYS, ALT).once_per_press(),
        ]);
        GRID_DELETE_REJECTED = b("delete_rejected", Grid, &[
            d(DELETE_KEYS, CMD).once_per_press(),
        ]);
        GRID_TRASH = b("trash", Grid, &[d(DELETE_KEYS, CMD_ALT_SHIFT).once_per_press()]);
        GRID_RENAME = b("rename", Grid, &[d(&[Key::F2], NONE).once_per_press()]);
        STACK_GROUP = b("stack:group", Grid, &[d(&[Key::G], CMD).once_per_press()]);
        STACK_UNSTACK = b("stack:unstack", Grid, &[d(&[Key::G], CMD_SHIFT).once_per_press()]);
        GRID_STACK_TOGGLE = b("stack:toggle", Grid, &[d(&[Key::S], NONE).once_per_press()]);
        GRID_STACK_TO_TOP = b("stack:to_top", Grid, &[d(&[Key::S], SHIFT).once_per_press()]);
        GRID_SELECT_ALL = b("select_all", Grid, &[d(&[Key::A], CMD)]);
        GRID_SELECT_NONE = b("select_none", Grid, &[d(&[Key::D], CMD)]);
        GRID_DESELECT_ACTIVE = b("deselect_active", Grid, &[d(&[Key::Slash], NONE)]);
        // Shift extends the selection.
        GRID_PREVIOUS = b("previous", Grid, &[d(&[Key::ArrowLeft], NONE)]).variant(SHIFT);
        GRID_NEXT = b("next", Grid, &[d(&[Key::ArrowRight], NONE)]).variant(SHIFT);
        GRID_ROW_UP = b("grid:row_up", Grid, &[d(&[Key::ArrowUp], NONE)]).variant(SHIFT);
        GRID_ROW_DOWN = b("grid:row_down", Grid, &[d(&[Key::ArrowDown], NONE)]).variant(SHIFT);
        GRID_PAGE_UP = b("grid:page_up", Grid, &[d(&[Key::PageUp], NONE)]).variant(SHIFT);
        GRID_PAGE_DOWN = b("grid:page_down", Grid, &[d(&[Key::PageDown], NONE)]).variant(SHIFT);
        GRID_FIRST = b("first_photo", Grid, &[d(&[Key::Home], NONE)]).variant(SHIFT);
        GRID_LAST = b("last_photo", Grid, &[d(&[Key::End], NONE)]).variant(SHIFT);

        LOUPE_PREVIOUS = b("previous", Loupe, &[
            d(&[Key::ArrowLeft, Key::ArrowUp], NONE).off_card(),
        ]);
        LOUPE_NEXT = b("next", Loupe, &[
            d(&[Key::ArrowRight, Key::ArrowDown], NONE).off_card(),
        ]);
        LOUPE_FIRST = b("first_photo", Loupe, &[d(&[Key::Home], NONE).off_card()]);
        LOUPE_LAST = b("last_photo", Loupe, &[d(&[Key::End], NONE).off_card()]);
        LOUPE_CLOSE = b("close_view", Loupe, &[d(&[Key::Escape], NONE)]);
        LOUPE_DELETE = b("delete", Loupe, &[d(DELETE_KEYS, NONE).once_per_press().off_card()]);
        LOUPE_REMOVE_FROM_CATALOG = b("remove_from_catalog", Loupe, &[
            d(DELETE_KEYS, ALT).once_per_press().off_card(),
        ]);
        LOUPE_DELETE_REJECTED = b("delete_rejected", Loupe, &[
            d(DELETE_KEYS, CMD).once_per_press().off_card(),
        ]);
        LOUPE_TRASH = b("trash", Loupe, &[
            d(DELETE_KEYS, CMD_ALT_SHIFT).once_per_press().off_card(),
        ]);
        LOUPE_RENAME = b("rename", Loupe, &[d(&[Key::F2], NONE).once_per_press().off_card()]);
        LOUPE_COMPARE = b("library:compare", Loupe, &[d(&[Key::C], NONE).off_card()]);
        LOUPE_SURVEY = b("library:survey", Loupe, &[d(&[Key::N], NONE).off_card()]);
        LOUPE_QUICK_TOGGLE = b("quick:toggle", Loupe, &[
            d(&[Key::B], NONE).once_per_press().off_card(),
        ]);
        LOUPE_QUICK_SHOW = b("quick:show", Loupe, &[
            d(&[Key::B], CMD).once_per_press().off_card(),
        ]);
        LOUPE_QUICK_CLEAR = b("quick:clear", Loupe, &[
            d(&[Key::B], CMD_SHIFT).once_per_press().off_card(),
        ]);
        LOUPE_INFO = b("info", Loupe, &[d(&[Key::I], NONE).once_per_press()]);
        LOUPE_STACK_TOGGLE = b("stack:toggle", Loupe, &[
            d(&[Key::S], NONE).once_per_press().off_card(),
        ]);
        LOUPE_STACK_TO_TOP = b("stack:to_top", Loupe, &[
            d(&[Key::S], SHIFT).once_per_press().off_card(),
        ]);

        // Compare and Survey take B first.
        COMPARE_QUICK_TOGGLE = b("quick:toggle", Compare, &[
            d(&[Key::B], NONE).once_per_press().off_card(),
        ]);
        COMPARE_QUICK_SHOW = b("quick:show", Compare, &[
            d(&[Key::B], CMD).once_per_press().off_card(),
        ]);
        COMPARE_QUICK_CLEAR = b("quick:clear", Compare, &[
            d(&[Key::B], CMD_SHIFT).once_per_press().off_card(),
        ]);
        COMPARE_PREVIOUS = b("compare_view:previous_candidate", Compare, &[
            d(&[Key::ArrowLeft], NONE),
        ]);
        COMPARE_NEXT = b("compare_view:next_candidate", Compare, &[d(&[Key::ArrowRight], NONE)]);
        COMPARE_MAKE_SELECT = b("compare_view:make_select", Compare, &[
            d(&[Key::ArrowUp], NONE).once_per_press(),
        ]);
        COMPARE_SWAP = b("compare_view:swap", Compare, &[
            d(&[Key::ArrowDown], NONE).once_per_press(),
        ]);
        COMPARE_CLOSE = b("close_view", Compare, &[d(&[Key::Escape], NONE)]);
        COMPARE_LOUPE = b("loupe", Compare, &[d(&[Key::E, Key::Enter], NONE).off_card()]);
        COMPARE_SURVEY = b("library:survey", Compare, &[d(&[Key::N], NONE).off_card()]);

        SURVEY_QUICK_TOGGLE = b("quick:toggle", Survey, &[
            d(&[Key::B], NONE).once_per_press().off_card(),
        ]);
        SURVEY_QUICK_SHOW = b("quick:show", Survey, &[
            d(&[Key::B], CMD).once_per_press().off_card(),
        ]);
        SURVEY_QUICK_CLEAR = b("quick:clear", Survey, &[
            d(&[Key::B], CMD_SHIFT).once_per_press().off_card(),
        ]);
        SURVEY_PREVIOUS = b("previous", Survey, &[
            d(&[Key::ArrowLeft, Key::ArrowUp], NONE).off_card(),
        ]);
        SURVEY_NEXT = b("next", Survey, &[
            d(&[Key::ArrowRight, Key::ArrowDown], NONE).off_card(),
        ]);
        SURVEY_CLOSE = b("close_view", Survey, &[d(&[Key::Escape], NONE)]);
        SURVEY_LOUPE = b("loupe", Survey, &[d(&[Key::E, Key::Enter], NONE).off_card()]);
        SURVEY_COMPARE = b("library:compare", Survey, &[d(&[Key::C], NONE).off_card()]);

        // Undo and redo in the Library (`Editor::workspace_shortcuts`); Develop
        // has its own.
        LIBRARY_REDO = b("redo", Library, &[
            d(&[Key::Z], CMD_SHIFT),
            d(&[Key::Y], CMD).not_macos().off_card(),
        ]).consuming();
        LIBRARY_UNDO = b("undo", Library, &[d(&[Key::Z], CMD)]).consuming();
        // The workspace's keys (`Editor::workspace_shortcuts`).
        LIBRARY = b("library", Everywhere, &[d(&[Key::G], NONE)]);
        LOUPE_FROM_DEVELOP = b("loupe", Develop, &[d(&[Key::E], NONE)]);
        VIRTUAL_COPY = b("virtual_copy", Everywhere, &[d(&[Key::Quote], CMD).once_per_press()]);
        CHROME = b("chrome", Everywhere, &[d(&[Key::F], NONE).once_per_press()]).once();
        DEVELOP = b("develop", Everywhere, &[d(&[Key::D], NONE)]);

        // `Editor::preferences_shortcut` and the reference card's window.
        PREFERENCES = b("preferences", Everywhere, &[d(&[Key::Comma], CMD)]).consuming();
        SHORTCUTS = b("shortcuts", Everywhere, &[d(&[Key::Slash], CMD)]).consuming();
        SHORTCUTS_CLOSE = b("shortcuts", ShortcutSheet, &[
            d(&[Key::Slash], CMD).off_card(),
        ]).consuming();
        SHORTCUTS_CLEAR_SEARCH = b("shortcuts:clear_search", ShortcutSheet, &[
            d(&[Key::Escape], NONE).off_card(),
        ]).consuming();

        // `Editor::panel_keys`, in its order.
        PANELS_ALL = b("panels:all", Everywhere, &[d(&[Key::Tab], SHIFT)]).consuming();
        PANELS_SIDES = b("panels:sides", Everywhere, &[d(&[Key::Tab], NONE)]).consuming();
        PANELS_PRESETS = b("panels:presets", Everywhere, &[d(&[Key::F8], SHIFT)]).consuming();
        PANELS_LEFT = b("panels:left", Everywhere, &[d(&[Key::F7], NONE)]).consuming();
        PANELS_RIGHT = b("panels:right", Everywhere, &[d(&[Key::F8], NONE)]).consuming();
        PANELS_FILMSTRIP = b("panels:filmstrip", Everywhere, &[d(&[Key::F6], NONE)]).consuming();

        // `Editor::library_workspace`: on the Library's selection.
        LIBRARY_EXPORT = b("export_dialog", Library, &[d(&[Key::E], CMD_SHIFT).once_per_press()]);
        LIBRARY_EXPORT_PREVIOUS = b("export_previous", Library, &[
            d(&[Key::E], CMD_ALT_SHIFT).once_per_press(),
        ]);
        LIBRARY_SYNC = b("sync", Library, &[d(&[Key::S], CMD_SHIFT)]).consuming();
        LIBRARY_AUTO = b("auto_tone", Library, &[d(&[Key::U], CMD_SHIFT)]).consuming();

        // `Editor::zoom_keys`, in the Library's Loupe and in Develop.
        // Cmd+Option+0 is Lightroom's 100%.
        ZOOM_100 = b("zoom:100", LoupeZoom, &[
            d(&[Key::Num0], CMD).typed(),
            d(&[Key::Num0], CMD_ALT).typed().off_card(),
        ]);
        ZOOM_FIT = b("fit", LoupeZoom, &[d(&[Key::Num1], CMD).typed()]);
        // Cmd++ for a layout with a key of its own for `+`; US layouts type it
        // as Cmd+Shift+=, which is Cmd+= on a punctuation key.
        ZOOM_IN = b("zoom:in", LoupeZoom, &[d(&[Key::Equals, Key::Plus], CMD)]);
        ZOOM_OUT = b("zoom:out", LoupeZoom, &[d(&[Key::Minus], CMD)]);
        ZOOM = b("zoom", LoupeZoom, &[d(&[Key::Z], NONE).once_per_press()]);
        MAGNIFIER = b("magnifier", LoupeZoom, &[d(&[Key::Z], SHIFT).once_per_press()]);

        // Develop (`Editor::develop_delete_key`, `Editor::develop_shortcuts`).
        DEVELOP_DELETE = b("delete", Develop, &[
            d(DELETE_KEYS, NONE).once_per_press().off_card(),
        ]);
        DEVELOP_REMOVE_FROM_CATALOG = b("remove_from_catalog", Develop, &[
            d(DELETE_KEYS, ALT).once_per_press().off_card(),
        ]);
        DEVELOP_DELETE_REJECTED = b("delete_rejected", Develop, &[
            d(DELETE_KEYS, CMD).once_per_press().off_card(),
        ]);
        DEVELOP_TRASH = b("trash", Develop, &[
            d(DELETE_KEYS, CMD_ALT_SHIFT).once_per_press().off_card(),
        ]);
        COPY_TO_BEFORE = b("before_after:copy_to_before", Develop, &[
            d(&[Key::ArrowLeft], CMD_ALT_SHIFT),
        ]);
        COPY_TO_AFTER = b("before_after:copy_to_after", Develop, &[
            d(&[Key::ArrowRight], CMD_ALT_SHIFT),
        ]);
        SWAP_BEFORE_AFTER = b("before_after:swap", Develop, &[d(&[Key::ArrowUp], CMD_ALT_SHIFT)]);
        NEXT = b("next", Develop, &[d(&[Key::ArrowRight], NONE)]);
        PREVIOUS = b("previous", Develop, &[d(&[Key::ArrowLeft], NONE)]);
        BEFORE_AFTER_LEFT_RIGHT = b("before_after:left_right", Develop, &[
            d(&[Key::Y], NONE).once_per_press(),
        ]);
        BEFORE_AFTER_TOP_BOTTOM = b("before_after:top_bottom", Develop, &[
            d(&[Key::Y], ALT).once_per_press(),
        ]);
        BEFORE_AFTER_SPLIT = b("before_after:split", Develop, &[
            d(&[Key::Y], SHIFT).once_per_press(),
        ]);
        COPY_SETTINGS = b("copy", Develop, &[d(&[Key::C], CMD_SHIFT)]);
        PASTE_SETTINGS = b("paste", Develop, &[d(&[Key::V], CMD_SHIFT)]);
        PASTE_PREVIOUS = b("paste_previous", Develop, &[d(&[Key::V], CMD_ALT).once_per_press()]);
        MATCH_EXPOSURES = b("match_exposures", Develop, &[
            d(&[Key::M], CMD_ALT_SHIFT).once_per_press(),
        ]);
        NEW_PRESET = b("new_preset", Develop, &[d(&[Key::N], CMD_SHIFT)]);
        TARGETED_TONE_CURVE = b("targeted:tone_curve", Develop, &[
            d(&[Key::T], CMD_ALT_SHIFT).once_per_press(),
        ]);
        TARGETED_HUE = b("targeted:hue", Develop, &[
            d(&[Key::H], CMD_ALT_SHIFT).once_per_press(),
        ]);
        TARGETED_SATURATION = b("targeted:saturation", Develop, &[
            d(&[Key::S], CMD_ALT_SHIFT).once_per_press(),
        ]);
        TARGETED_LUMINANCE = b("targeted:luminance", Develop, &[
            d(&[Key::L], CMD_ALT_SHIFT).once_per_press(),
        ]);
        TARGETED_BLACK_WHITE = b("targeted:black_white", Develop, &[
            d(&[Key::G], CMD_ALT_SHIFT).once_per_press(),
        ]);
        DEVELOP_SYNC = b("sync", Develop, &[d(&[Key::S], CMD_SHIFT)]);
        RESET = b("reset", Develop, &[d(&[Key::R], CMD_SHIFT)]);
        TOGGLE_BLACK_WHITE = b("toggle:bw", Develop, &[d(&[Key::V], NONE).once_per_press()]);
        DEVELOP_AUTO = b("auto_tone", Develop, &[d(&[Key::U], CMD_SHIFT)]);
        DEVELOP_EXPORT = b("export_dialog", Develop, &[d(&[Key::E], CMD_SHIFT).once_per_press()]);
        DEVELOP_EXPORT_PREVIOUS = b("export_previous", Develop, &[
            d(&[Key::E], CMD_ALT_SHIFT).once_per_press(),
        ]);
        DEVELOP_SELECT_ALL = b("select_all", Develop, &[d(&[Key::A], CMD)]);
        DEVELOP_SELECT_NONE = b("select_none", Develop, &[d(&[Key::D], CMD)]);
        DEVELOP_UNDO = b("undo", Develop, &[d(&[Key::Z], CMD).off_card()]);
        DEVELOP_REDO = b("redo", Develop, &[d(&[Key::Z], CMD_SHIFT).off_card()]);
        /// Ctrl+Y, read apart from Cmd+Z: both in one frame undo and redo.
        DEVELOP_REDO_Y = b("redo", Develop, &[d(&[Key::Y], CMD).not_macos().off_card()]);
        CROP = b("crop", Develop, &[d(&[Key::C], NONE).off_card(), d(&[Key::R], NONE)]).once();
        REFERENCE_VIEW = b("reference_view", Develop, &[d(&[Key::R], SHIFT)]);
        DEVELOP_INFO = b("info", Develop, &[d(&[Key::I], NONE).once_per_press()]);
        CLIPPING = b("clipping", Develop, &[d(&[Key::J], NONE)]);
        // Shift+\ arrives as | on most layouts. A German Mac types | with
        // Option+7 and \ with Option+Shift+7: Alt is free on punctuation.
        CAMERA_THUMBNAIL = b("camera_thumbnail", Develop, &[
            d(&[Key::Pipe], NONE).off_card(),
            d(&[Key::Backslash], SHIFT),
        ]);
        // Shift+\ is the camera thumbnail's.
        BEFORE_ONLY = b("compare", Develop, &[d(&[Key::Backslash], NONE).rejecting(SHIFT)]);
        FINISH_CROP = b("tool:finish", Context::Tool(Tool::Crop), &[d(&[Key::Enter], NONE)]);
        FINISH_GUIDED = b("tool:finish", Context::Tool(Tool::Guided), &[
            d(&[Key::Enter], NONE).off_card(),
        ]);
        WHITE_BALANCE = b("white_balance", Develop, &[d(&[Key::W], NONE)]);
        REMOVE = b("remove", Develop, &[d(&[Key::Q], NONE)]);
        MASK = b("mask", Develop, &[d(&[Key::W], SHIFT)]);
        GUIDED_UPRIGHT = b("guided_upright", Develop, &[d(&[Key::T], SHIFT)]);
        /// Any tool but Masking, which takes Escape itself.
        CLOSE_TOOL = b("tool:close", Develop, &[d(&[Key::Escape], NONE)]);
        /// Puts Masking away at once, leaving nothing behind. No key by default:
        /// Escape goes back step by step (`MASK_BACK`).
        CLOSE_MASKING = b("tool:close", Context::Tool(Tool::Mask), &[]);
        NEW_BRUSH_MASK = b("mask:new_brush", Develop, &[d(&[Key::K], NONE)]);
        NEW_LINEAR_MASK = b("mask:new_linear", Develop, &[d(&[Key::M], NONE)]);
        NEW_RADIAL_MASK = b("mask:new_radial", Develop, &[d(&[Key::M], SHIFT)]);
        NEW_COLOR_RANGE_MASK = b("mask:new_color_range", Develop, &[d(&[Key::J], SHIFT)]);

        // The tools' keys (`Editor::crop_keys` and the rest).
        CROP_SWAP_ORIENTATION = b("crop:swap_orientation", Context::Tool(Tool::Crop), &[
            d(&[Key::X], NONE),
        ]);
        CROP_ASPECT_LOCK = b("crop:aspect_lock", Context::Tool(Tool::Crop), &[d(&[Key::A], NONE)]);
        CROP_NEXT_OVERLAY = b("crop:next_overlay", Context::Tool(Tool::Crop), &[
            d(&[Key::O], NONE),
        ]);
        CROP_TURN_OVERLAY = b("crop:turn_overlay", Context::Tool(Tool::Crop), &[
            d(&[Key::O], SHIFT),
        ]);
        // Shift feathers; Alt types other symbols.
        REMOVE_SMALLER = b("brush:smaller", Context::Tool(Tool::Remove), &[
            d(OPEN_BRACKETS, NONE).rejecting(ALT),
        ]).variant(SHIFT);
        REMOVE_LARGER = b("brush:larger", Context::Tool(Tool::Remove), &[
            d(CLOSE_BRACKETS, NONE).rejecting(ALT),
        ]).variant(SHIFT);
        REMOVE_NEW_SOURCE = b("remove:new_source", Context::Tool(Tool::Remove), &[
            d(SLASHES, NONE).rejecting(ALT),
        ]);
        REMOVE_DELETE = b("tool:delete", Context::Tool(Tool::Remove), &[d(DELETE_KEYS, NONE)]);
        REMOVE_HIDE_PINS = b("remove:hide_pins", Context::Tool(Tool::Remove), &[
            d(&[Key::H], NONE),
        ]);
        /// Not a quick Cmd+A, Select All, released within the frame.
        REMOVE_VISUALIZE = b("remove:visualize", Context::Tool(Tool::Remove), &[
            d(&[Key::A], NONE),
        ]);
        RED_EYE_DELETE = b("tool:delete", Context::Tool(Tool::RedEye), &[
            d(DELETE_KEYS, NONE).off_card(),
        ]);
        RED_EYE_SMALLER = b("brush:smaller", Context::Tool(Tool::RedEye), &[
            d(OPEN_BRACKETS, NONE).rejecting(ALT).off_card(),
        ]);
        RED_EYE_LARGER = b("brush:larger", Context::Tool(Tool::RedEye), &[
            d(CLOSE_BRACKETS, NONE).rejecting(ALT).off_card(),
        ]);
        MASK_SMALLER = b("brush:smaller", Context::Tool(Tool::Mask), &[
            d(OPEN_BRACKETS, NONE).rejecting(ALT).off_card(),
        ]).variant(SHIFT);
        MASK_LARGER = b("brush:larger", Context::Tool(Tool::Mask), &[
            d(CLOSE_BRACKETS, NONE).rejecting(ALT).off_card(),
        ]).variant(SHIFT);
        MASK_BACK = b("mask:back", Context::Tool(Tool::Mask), &[
            d(&[Key::Escape], NONE).off_card(),
        ]);
        MASK_OVERLAY = b("mask:overlay", Context::Tool(Tool::Mask), &[
            d(&[Key::O], NONE).off_card(),
        ]);
        MASK_SWITCH_BRUSH = b("mask:switch_brush", Context::Tool(Tool::Mask), &[
            d(SLASHES, NONE).rejecting(ALT).off_card(),
        ]);
        MASK_DELETE = b("tool:delete", Context::Tool(Tool::Mask), &[
            d(DELETE_KEYS, NONE).off_card(),
        ]);
        GUIDED_DELETE = b("tool:delete", Context::Tool(Tool::Guided), &[
            d(DELETE_KEYS, NONE).off_card(),
        ]);

        // The Basic panel's keys (`Editor::develop_key_steps`), by their place.
        // `.` and `,` refuse Shift and Alt; Shift steps `=` and `-` more, Alt less.
        BASIC_NEXT = b("basic:next", Develop, &[
            d(&[Key::Period], NONE).placed().rejecting(ALT_SHIFT),
        ]);
        BASIC_PREVIOUS = b("basic:previous", Develop, &[
            d(&[Key::Comma], NONE).placed().rejecting(ALT_SHIFT),
        ]);
        BASIC_INCREASE = b("basic:increase", Develop, &[
            d(&[Key::Equals, Key::Plus], NONE).placed(),
        ]).variant(ALT_SHIFT);
        BASIC_DECREASE = b("basic:decrease", Develop, &[d(&[Key::Minus], NONE).placed()])
            .variant(ALT_SHIFT);
    }

    /// The rating, flag and label keys, in the order a press is matched.
    pub(in crate::app) static METADATA: [&Binding; 18] = [
        &FLAG_RAISE,
        &FLAG_LOWER,
        &RATING_0,
        &RATING_1,
        &RATING_2,
        &RATING_3,
        &RATING_4,
        &RATING_5,
        &LABEL_RED,
        &LABEL_YELLOW,
        &LABEL_GREEN,
        &LABEL_BLUE,
        &PICK,
        &UNFLAG,
        &REJECT,
        &TOGGLE_PICK,
        &RATING_LOWER,
        &RATING_RAISE,
    ];
}

#[cfg(test)]
mod tests;
