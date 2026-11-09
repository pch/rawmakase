//! Every command a key runs, and the keys that run it in each context.
//!
//! A command's id names it to the key map, automation and MIDI: the
//! [`Action`](super::Action) names are kept as they are, and new ids follow
//! their style (lower-case snake_case words, `:` before a parameter, never `/`,
//! which will separate an extension's name from its command). A [`Binding`]
//! says where a key runs the command and how its press is matched; the handler
//! that runs it names it (`bindings::CLIPPING`) and takes its press from
//! [`FrameKeys`](crate::app::shell::keymap::FrameKeys).
use crate::app::shell::keymap::{Binding, Context, Default, Tool, rules};
use eframe::egui::{Key, Modifiers};

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

/// The bindings, named for the handler that takes their presses. Each chord's
/// modifier rule is the one that handler applied before the key map, extra
/// modifiers it tolerates included.
pub(in crate::app) mod bindings {
    use super::*;
    use Context::{
        Compare, Develop, Everywhere, Grid, Library, Loupe, LoupeZoom, ShortcutSheet, Survey,
    };

    const fn d(keys: &'static [Key], mods: fn(Modifiers) -> bool, label: &'static str) -> Default {
        Default::new(keys, mods, label)
    }
    const fn b(command: &'static str, context: Context, defaults: &'static [Default]) -> Binding {
        Binding::new(command, context, defaults)
    }
    /// A metadata key: by what it types, the number row by its place, once per
    /// press, Shift going on to the next photo.
    const fn rate(key: &'static [Key], label: &'static str) -> Default {
        Default::new(key, rules::metadata, label)
            .typed()
            .once_per_press()
    }
    const DELETE_KEYS: &[Key] = &[Key::Delete, Key::Backspace];
    const OPEN_BRACKETS: &[Key] = &[Key::OpenBracket, Key::OpenCurlyBracket];
    const CLOSE_BRACKETS: &[Key] = &[Key::CloseBracket, Key::CloseCurlyBracket];
    const SLASHES: &[Key] = &[Key::Slash, Key::Questionmark];
    fn not_all(m: Modifiers) -> bool {
        !rules::command_alt_shift(m)
    }
    /// Delete and Backspace's chords, as `Library::delete_press` reads them.
    fn delete(m: Modifiers) -> bool {
        !m.command && !m.alt && !m.shift && !m.ctrl
    }
    fn remove_from_catalog(m: Modifiers) -> bool {
        !m.command && m.alt && !m.shift && !m.ctrl
    }
    fn delete_rejected(m: Modifiers) -> bool {
        m.command && !m.alt && !m.shift
    }
    /// B, Cmd+B and Cmd+Shift+B, as `Library::quick_key` reads them: Option, and
    /// Control on a Mac, make other keys.
    fn quick(m: Modifiers) -> bool {
        !(m.alt || (m.ctrl && (m.mac_cmd || !m.command)))
    }
    /// The Library's S and Shift+S.
    fn stack_key(m: Modifiers) -> bool {
        !m.command && !m.alt && !m.ctrl
    }
    /// The Crop tool's keys.
    fn crop_key(m: Modifiers) -> bool {
        !(m.command || m.alt || m.ctrl)
    }

    bindings! {
        // The message line and the window (`Editor::draw`).
        DISMISS_MESSAGES = b("messages:dismiss_all", Everywhere, &[
            d(&[Key::Escape], rules::logically_shift, "Shift+Esc"),
        ]).consuming();
        SHOW_CHROME = b("chrome:show", Everywhere, &[
            d(&[Key::Escape], rules::logically_none, "Esc"),
        ]).consuming();

        // Rating, flag and label keys (`photo_metadata::shortcut`), in the
        // order they are matched.
        FLAG_RAISE = b("flag:raise", Everywhere, &[
            d(&[Key::ArrowUp], |m| m.command && !m.shift && !m.alt, "Cmd+Up").once_per_press(),
        ]);
        FLAG_LOWER = b("flag:lower", Everywhere, &[
            d(&[Key::ArrowDown], |m| m.command && !m.shift && !m.alt, "Cmd+Down").once_per_press(),
        ]);
        RATING_0 = b("rating:0", Everywhere, &[rate(&[Key::Num0], "0")]);
        RATING_1 = b("rating:1", Everywhere, &[rate(&[Key::Num1], "1")]);
        RATING_2 = b("rating:2", Everywhere, &[rate(&[Key::Num2], "2")]);
        RATING_3 = b("rating:3", Everywhere, &[rate(&[Key::Num3], "3")]);
        RATING_4 = b("rating:4", Everywhere, &[rate(&[Key::Num4], "4")]);
        RATING_5 = b("rating:5", Everywhere, &[rate(&[Key::Num5], "5")]);
        LABEL_RED = b("toggle_label:red", Everywhere, &[rate(&[Key::Num6], "6")]);
        LABEL_YELLOW = b("toggle_label:yellow", Everywhere, &[rate(&[Key::Num7], "7")]);
        LABEL_GREEN = b("toggle_label:green", Everywhere, &[rate(&[Key::Num8], "8")]);
        LABEL_BLUE = b("toggle_label:blue", Everywhere, &[rate(&[Key::Num9], "9")]);
        PICK = b("pick", Everywhere, &[rate(&[Key::P], "P")]);
        UNFLAG = b("unflag", Everywhere, &[rate(&[Key::U], "U")]);
        REJECT = b("reject", Everywhere, &[rate(&[Key::X], "X")]);
        TOGGLE_PICK = b("toggle_pick", Everywhere, &[rate(&[Key::Backtick], "`")]);
        // Shift+[ and Shift+] type `{` and `}` on US layouts.
        RATING_LOWER = b("rating:lower", Everywhere, &[rate(OPEN_BRACKETS, "[")]);
        RATING_RAISE = b("rating:raise", Everywhere, &[rate(CLOSE_BRACKETS, "]")]);

        // The Library's views (`Library::selection_keys`).
        FILTERS = b("filters", Library, &[
            d(&[Key::L], |m| m.matches_exact(Modifiers::COMMAND), "Cmd+L").once_per_press(),
        ]);
        GRID_LOUPE = b("loupe", Grid, &[
            d(&[Key::E], rules::no_command, "E"),
            d(&[Key::Enter], rules::no_command, "Return"),
        ]);
        GRID_COMPARE = b("library:compare", Grid, &[d(&[Key::C], rules::none, "C")]);
        GRID_SURVEY = b("library:survey", Grid, &[d(&[Key::N], rules::none, "N")]);
        GRID_CELL_STYLE = b("grid:cell_style", Grid, &[
            d(&[Key::J], rules::none, "J").once_per_press(),
        ]);
        GRID_QUICK_TOGGLE = b("quick:toggle", Grid, &[
            d(&[Key::B], |m| quick(m) && !m.command && !m.shift, "B").once_per_press(),
        ]);
        GRID_QUICK_SHOW = b("quick:show", Grid, &[
            d(&[Key::B], |m| quick(m) && m.command && !m.shift, "Cmd+B").once_per_press(),
        ]);
        GRID_QUICK_CLEAR = b("quick:clear", Grid, &[
            d(&[Key::B], |m| quick(m) && m.command && m.shift, "Cmd+Shift+B").once_per_press(),
        ]);
        GRID_DELETE = b("delete", Grid, &[d(DELETE_KEYS, delete, "Delete").once_per_press()]);
        GRID_REMOVE_FROM_CATALOG = b("remove_from_catalog", Grid, &[
            d(DELETE_KEYS, remove_from_catalog, "Alt+Delete").once_per_press(),
        ]);
        GRID_DELETE_REJECTED = b("delete_rejected", Grid, &[
            d(DELETE_KEYS, delete_rejected, "Cmd+Delete").once_per_press(),
        ]);
        GRID_TRASH = b("trash", Grid, &[
            d(DELETE_KEYS, rules::command_alt_shift, "Cmd+Alt+Shift+Delete").once_per_press(),
        ]);
        GRID_RENAME = b("rename", Grid, &[d(&[Key::F2], rules::none, "F2").once_per_press()]);
        STACK_GROUP = b("stack:group", Grid, &[
            d(&[Key::G], |m| m.command && !m.alt && !m.shift, "Cmd+G").once_per_press(),
        ]);
        STACK_UNSTACK = b("stack:unstack", Grid, &[
            d(&[Key::G], |m| m.command && !m.alt && m.shift, "Cmd+Shift+G").once_per_press(),
        ]);
        GRID_STACK_TOGGLE = b("stack:toggle", Grid, &[
            d(&[Key::S], |m| stack_key(m) && !m.shift, "S").once_per_press(),
        ]);
        GRID_STACK_TO_TOP = b("stack:to_top", Grid, &[
            d(&[Key::S], |m| stack_key(m) && m.shift, "Shift+S").once_per_press(),
        ]);
        GRID_SELECT_ALL = b("select_all", Grid, &[d(&[Key::A], rules::command, "Cmd+A")]);
        GRID_SELECT_NONE = b("select_none", Grid, &[d(&[Key::D], rules::command, "Cmd+D")]);
        GRID_DESELECT_ACTIVE = b("deselect_active", Grid, &[
            d(&[Key::Slash], rules::no_command, "/"),
        ]);
        // Shift extends the selection.
        GRID_PREVIOUS = b("previous", Grid, &[d(&[Key::ArrowLeft], rules::no_command, "Left")]);
        GRID_NEXT = b("next", Grid, &[d(&[Key::ArrowRight], rules::no_command, "Right")]);
        GRID_ROW_UP = b("grid:row_up", Grid, &[d(&[Key::ArrowUp], rules::no_command, "Up")]);
        GRID_ROW_DOWN = b("grid:row_down", Grid, &[
            d(&[Key::ArrowDown], rules::no_command, "Down"),
        ]);
        GRID_PAGE_UP = b("grid:page_up", Grid, &[
            d(&[Key::PageUp], rules::no_command, "Page Up"),
        ]);
        GRID_PAGE_DOWN = b("grid:page_down", Grid, &[
            d(&[Key::PageDown], rules::no_command, "Page Down"),
        ]);
        GRID_FIRST = b("first_photo", Grid, &[d(&[Key::Home], rules::no_command, "Home")]);
        GRID_LAST = b("last_photo", Grid, &[d(&[Key::End], rules::no_command, "End")]);

        LOUPE_PREVIOUS = b("previous", Loupe, &[
            d(&[Key::ArrowLeft, Key::ArrowUp], rules::no_command, "Left").off_card(),
        ]);
        LOUPE_NEXT = b("next", Loupe, &[
            d(&[Key::ArrowRight, Key::ArrowDown], rules::no_command, "Right").off_card(),
        ]);
        LOUPE_FIRST = b("first_photo", Loupe, &[
            d(&[Key::Home], rules::no_command, "Home").off_card(),
        ]);
        LOUPE_LAST = b("last_photo", Loupe, &[
            d(&[Key::End], rules::no_command, "End").off_card(),
        ]);
        LOUPE_CLOSE = b("close_view", Loupe, &[d(&[Key::Escape], rules::any, "Esc")]);
        LOUPE_DELETE = b("delete", Loupe, &[
            d(DELETE_KEYS, delete, "Delete").once_per_press().off_card(),
        ]);
        LOUPE_REMOVE_FROM_CATALOG = b("remove_from_catalog", Loupe, &[
            d(DELETE_KEYS, remove_from_catalog, "Alt+Delete").once_per_press().off_card(),
        ]);
        LOUPE_DELETE_REJECTED = b("delete_rejected", Loupe, &[
            d(DELETE_KEYS, delete_rejected, "Cmd+Delete").once_per_press().off_card(),
        ]);
        LOUPE_TRASH = b("trash", Loupe, &[
            d(DELETE_KEYS, rules::command_alt_shift, "Cmd+Alt+Shift+Delete")
                .once_per_press()
                .off_card(),
        ]);
        LOUPE_RENAME = b("rename", Loupe, &[
            d(&[Key::F2], rules::none, "F2").once_per_press().off_card(),
        ]);
        LOUPE_COMPARE = b("library:compare", Loupe, &[
            d(&[Key::C], rules::none, "C").off_card(),
        ]);
        LOUPE_SURVEY = b("library:survey", Loupe, &[d(&[Key::N], rules::none, "N").off_card()]);
        LOUPE_QUICK_TOGGLE = b("quick:toggle", Loupe, &[
            d(&[Key::B], |m| quick(m) && !m.command && !m.shift, "B").once_per_press().off_card(),
        ]);
        LOUPE_QUICK_SHOW = b("quick:show", Loupe, &[
            d(&[Key::B], |m| quick(m) && m.command && !m.shift, "Cmd+B")
                .once_per_press()
                .off_card(),
        ]);
        LOUPE_QUICK_CLEAR = b("quick:clear", Loupe, &[
            d(&[Key::B], |m| quick(m) && m.command && m.shift, "Cmd+Shift+B")
                .once_per_press()
                .off_card(),
        ]);
        LOUPE_INFO = b("info", Loupe, &[d(&[Key::I], rules::none, "I").once_per_press()]);
        LOUPE_STACK_TOGGLE = b("stack:toggle", Loupe, &[
            d(&[Key::S], |m| stack_key(m) && !m.shift, "S").once_per_press().off_card(),
        ]);
        LOUPE_STACK_TO_TOP = b("stack:to_top", Loupe, &[
            d(&[Key::S], |m| stack_key(m) && m.shift, "Shift+S").once_per_press().off_card(),
        ]);

        // Compare and Survey take B with any modifiers first, then nothing
        // with Cmd or Alt.
        COMPARE_QUICK_TOGGLE = b("quick:toggle", Compare, &[
            d(&[Key::B], |m| quick(m) && !m.command && !m.shift, "B").once_per_press().off_card(),
        ]);
        COMPARE_QUICK_SHOW = b("quick:show", Compare, &[
            d(&[Key::B], |m| quick(m) && m.command && !m.shift, "Cmd+B")
                .once_per_press()
                .off_card(),
        ]);
        COMPARE_QUICK_CLEAR = b("quick:clear", Compare, &[
            d(&[Key::B], |m| quick(m) && m.command && m.shift, "Cmd+Shift+B")
                .once_per_press()
                .off_card(),
        ]);
        COMPARE_PREVIOUS = b("compare_view:previous_candidate", Compare, &[
            d(&[Key::ArrowLeft], rules::no_command_alt, "Left"),
        ]);
        COMPARE_NEXT = b("compare_view:next_candidate", Compare, &[
            d(&[Key::ArrowRight], rules::no_command_alt, "Right"),
        ]);
        COMPARE_MAKE_SELECT = b("compare_view:make_select", Compare, &[
            d(&[Key::ArrowUp], rules::no_command_alt, "Up").once_per_press(),
        ]);
        COMPARE_SWAP = b("compare_view:swap", Compare, &[
            d(&[Key::ArrowDown], rules::no_command_alt, "Down").once_per_press(),
        ]);
        COMPARE_CLOSE = b("close_view", Compare, &[
            d(&[Key::Escape], rules::no_command_alt, "Esc"),
        ]);
        COMPARE_LOUPE = b("loupe", Compare, &[
            d(&[Key::E, Key::Enter], rules::no_command_alt, "E").off_card(),
        ]);
        COMPARE_SURVEY = b("library:survey", Compare, &[
            d(&[Key::N], rules::none, "N").off_card(),
        ]);

        SURVEY_QUICK_TOGGLE = b("quick:toggle", Survey, &[
            d(&[Key::B], |m| quick(m) && !m.command && !m.shift, "B").once_per_press().off_card(),
        ]);
        SURVEY_QUICK_SHOW = b("quick:show", Survey, &[
            d(&[Key::B], |m| quick(m) && m.command && !m.shift, "Cmd+B")
                .once_per_press()
                .off_card(),
        ]);
        SURVEY_QUICK_CLEAR = b("quick:clear", Survey, &[
            d(&[Key::B], |m| quick(m) && m.command && m.shift, "Cmd+Shift+B")
                .once_per_press()
                .off_card(),
        ]);
        SURVEY_PREVIOUS = b("previous", Survey, &[
            d(&[Key::ArrowLeft, Key::ArrowUp], rules::no_command_alt, "Left").off_card(),
        ]);
        SURVEY_NEXT = b("next", Survey, &[
            d(&[Key::ArrowRight, Key::ArrowDown], rules::no_command_alt, "Right").off_card(),
        ]);
        SURVEY_CLOSE = b("close_view", Survey, &[
            d(&[Key::Escape], rules::no_command_alt, "Esc"),
        ]);
        SURVEY_LOUPE = b("loupe", Survey, &[
            d(&[Key::E, Key::Enter], rules::no_command_alt, "E").off_card(),
        ]);
        SURVEY_COMPARE = b("library:compare", Survey, &[
            d(&[Key::C], rules::none, "C").off_card(),
        ]);

        // Undo and redo in the Library (`Editor::workspace_shortcuts`); Develop
        // has its own. Redo's chords before Undo's: `consume_key` ignores an
        // extra Shift.
        LIBRARY_REDO = b("redo", Library, &[
            d(&[Key::Z], rules::logically_command_shift, "Cmd+Shift+Z"),
            d(&[Key::Y], rules::logically_command, "Cmd+Y").not_macos().off_card(),
        ]).consuming();
        LIBRARY_UNDO = b("undo", Library, &[
            d(&[Key::Z], rules::logically_command, "Cmd+Z"),
        ]).consuming();
        // The workspace's keys (`Editor::workspace_shortcuts`).
        LIBRARY = b("library", Everywhere, &[d(&[Key::G], rules::plain, "G")]);
        LOUPE_FROM_DEVELOP = b("loupe", Develop, &[d(&[Key::E], rules::plain, "E")]);
        VIRTUAL_COPY = b("virtual_copy", Everywhere, &[
            d(&[Key::Quote], rules::command, "Cmd+'").once_per_press(),
        ]);
        CHROME = b("chrome", Everywhere, &[
            d(&[Key::F], rules::none, "F").once_per_press(),
        ]).once();
        DEVELOP = b("develop", Everywhere, &[d(&[Key::D], rules::plain, "D")]);

        // `Editor::preferences_shortcut` and the reference card's window.
        PREFERENCES = b("preferences", Everywhere, &[
            d(&[Key::Comma], rules::logically_command, "Cmd+,"),
        ]).consuming();
        SHORTCUTS = b("shortcuts", Everywhere, &[
            d(&[Key::Slash], rules::logically_command, "Cmd+/"),
        ]).consuming();
        SHORTCUTS_CLOSE = b("shortcuts", ShortcutSheet, &[
            d(&[Key::Slash], rules::logically_command, "Cmd+/").off_card(),
        ]).consuming();
        SHORTCUTS_CLEAR_SEARCH = b("shortcuts:clear_search", ShortcutSheet, &[
            d(&[Key::Escape], rules::logically_none, "Esc").off_card(),
        ]).consuming();

        // `Editor::panel_keys`, in its order: Shift+Tab before Tab, Shift+F8
        // before F8, as their plain patterns also match with Shift held.
        PANELS_ALL = b("panels:all", Everywhere, &[
            d(&[Key::Tab], rules::logically_shift, "Shift+Tab"),
        ]).consuming();
        PANELS_SIDES = b("panels:sides", Everywhere, &[
            d(&[Key::Tab], rules::logically_none, "Tab"),
        ]).consuming();
        PANELS_PRESETS = b("panels:presets", Everywhere, &[
            d(&[Key::F8], rules::logically_shift, "Shift+F8"),
        ]).consuming();
        PANELS_LEFT = b("panels:left", Everywhere, &[
            d(&[Key::F7], rules::logically_none, "F7"),
        ]).consuming();
        PANELS_RIGHT = b("panels:right", Everywhere, &[
            d(&[Key::F8], rules::logically_none, "F8"),
        ]).consuming();
        PANELS_FILMSTRIP = b("panels:filmstrip", Everywhere, &[
            d(&[Key::F6], rules::logically_none, "F6"),
        ]).consuming();

        // `Editor::library_workspace`: on the Library's selection.
        LIBRARY_EXPORT = b("export_dialog", Library, &[
            d(&[Key::E], |m| m.command && m.shift && !m.alt, "Cmd+Shift+E").once_per_press(),
        ]);
        LIBRARY_EXPORT_PREVIOUS = b("export_previous", Library, &[
            d(&[Key::E], rules::command_alt_shift, "Cmd+Alt+Shift+E").once_per_press(),
        ]);
        LIBRARY_SYNC = b("sync", Library, &[
            d(&[Key::S], rules::logically_command_shift, "Cmd+Shift+S"),
        ]).consuming();
        LIBRARY_AUTO = b("auto_tone", Library, &[
            d(&[Key::U], rules::logically_command_shift, "Cmd+Shift+U"),
        ]).consuming();

        // `Editor::zoom_keys`, in the Library's Loupe and in Develop.
        ZOOM_100 = b("zoom:100", LoupeZoom, &[
            d(&[Key::Num0], |m| m.command && !m.shift, "Cmd+0").typed(),
        ]);
        ZOOM_FIT = b("fit", LoupeZoom, &[
            d(&[Key::Num1], |m| m.command_only(), "Cmd+1").typed(),
        ]);
        ZOOM_IN = b("zoom:in", LoupeZoom, &[
            d(&[Key::Plus, Key::Equals], rules::command, "Cmd+="),
        ]);
        ZOOM_OUT = b("zoom:out", LoupeZoom, &[d(&[Key::Minus], rules::command, "Cmd+-")]);
        ZOOM = b("zoom", LoupeZoom, &[d(&[Key::Z], rules::none, "Z").once_per_press()]);
        MAGNIFIER = b("magnifier", LoupeZoom, &[
            d(&[Key::Z], |m| m.shift_only(), "Shift+Z").once_per_press(),
        ]);

        // Develop (`Editor::develop_delete_key`, `Editor::develop_shortcuts`).
        DEVELOP_DELETE = b("delete", Develop, &[
            d(DELETE_KEYS, delete, "Delete").once_per_press().off_card(),
        ]);
        DEVELOP_REMOVE_FROM_CATALOG = b("remove_from_catalog", Develop, &[
            d(DELETE_KEYS, remove_from_catalog, "Alt+Delete").once_per_press().off_card(),
        ]);
        DEVELOP_DELETE_REJECTED = b("delete_rejected", Develop, &[
            d(DELETE_KEYS, delete_rejected, "Cmd+Delete").once_per_press().off_card(),
        ]);
        DEVELOP_TRASH = b("trash", Develop, &[
            d(DELETE_KEYS, rules::command_alt_shift, "Cmd+Alt+Shift+Delete")
                .once_per_press()
                .off_card(),
        ]);
        COPY_TO_BEFORE = b("before_after:copy_to_before", Develop, &[
            d(&[Key::ArrowLeft], rules::command_alt_shift, "Cmd+Alt+Shift+Left"),
        ]);
        COPY_TO_AFTER = b("before_after:copy_to_after", Develop, &[
            d(&[Key::ArrowRight], rules::command_alt_shift, "Cmd+Alt+Shift+Right"),
        ]);
        SWAP_BEFORE_AFTER = b("before_after:swap", Develop, &[
            d(&[Key::ArrowUp], rules::command_alt_shift, "Cmd+Alt+Shift+Up"),
        ]);
        NEXT = b("next", Develop, &[d(&[Key::ArrowRight], not_all, "Right")]);
        PREVIOUS = b("previous", Develop, &[d(&[Key::ArrowLeft], not_all, "Left")]);
        BEFORE_AFTER_LEFT_RIGHT = b("before_after:left_right", Develop, &[
            d(&[Key::Y], |m| !m.command && !m.alt && !m.shift, "Y").once_per_press(),
        ]);
        BEFORE_AFTER_TOP_BOTTOM = b("before_after:top_bottom", Develop, &[
            d(&[Key::Y], |m| !m.command && m.alt, "Alt+Y").once_per_press(),
        ]);
        BEFORE_AFTER_SPLIT = b("before_after:split", Develop, &[
            d(&[Key::Y], |m| !m.command && !m.alt && m.shift, "Shift+Y").once_per_press(),
        ]);
        COPY_SETTINGS = b("copy", Develop, &[d(&[Key::C], rules::command_shift, "Cmd+Shift+C")]);
        PASTE_SETTINGS = b("paste", Develop, &[
            d(&[Key::V], rules::command_shift, "Cmd+Shift+V"),
        ]);
        PASTE_PREVIOUS = b("paste_previous", Develop, &[
            d(&[Key::V], |m| m.command && m.alt && !m.shift, "Cmd+Alt+V").once_per_press(),
        ]);
        MATCH_EXPOSURES = b("match_exposures", Develop, &[
            d(&[Key::M], rules::command_alt_shift, "Cmd+Alt+Shift+M").once_per_press(),
        ]);
        NEW_PRESET = b("new_preset", Develop, &[
            d(&[Key::N], rules::command_shift, "Cmd+Shift+N"),
        ]);
        TARGETED_TONE_CURVE = b("targeted:tone_curve", Develop, &[
            d(&[Key::T], rules::command_alt_shift, "Cmd+Alt+Shift+T").once_per_press(),
        ]);
        TARGETED_HUE = b("targeted:hue", Develop, &[
            d(&[Key::H], rules::command_alt_shift, "Cmd+Alt+Shift+H").once_per_press(),
        ]);
        TARGETED_SATURATION = b("targeted:saturation", Develop, &[
            d(&[Key::S], rules::command_alt_shift, "Cmd+Alt+Shift+S").once_per_press(),
        ]);
        TARGETED_LUMINANCE = b("targeted:luminance", Develop, &[
            d(&[Key::L], rules::command_alt_shift, "Cmd+Alt+Shift+L").once_per_press(),
        ]);
        TARGETED_BLACK_WHITE = b("targeted:black_white", Develop, &[
            d(&[Key::G], rules::command_alt_shift, "Cmd+Alt+Shift+G").once_per_press(),
        ]);
        DEVELOP_SYNC = b("sync", Develop, &[
            d(&[Key::S], |m| m.command && m.shift && !m.alt, "Cmd+Shift+S"),
        ]);
        RESET = b("reset", Develop, &[d(&[Key::R], rules::command_shift, "Cmd+Shift+R")]);
        TOGGLE_BLACK_WHITE = b("toggle:bw", Develop, &[
            d(&[Key::V], rules::none, "V").once_per_press(),
        ]);
        DEVELOP_AUTO = b("auto_tone", Develop, &[
            d(&[Key::U], rules::command_shift, "Cmd+Shift+U"),
        ]);
        DEVELOP_EXPORT = b("export_dialog", Develop, &[
            d(&[Key::E], |m| m.command && m.shift && !m.alt, "Cmd+Shift+E").once_per_press(),
        ]);
        DEVELOP_EXPORT_PREVIOUS = b("export_previous", Develop, &[
            d(&[Key::E], rules::command_alt_shift, "Cmd+Alt+Shift+E").once_per_press(),
        ]);
        DEVELOP_SELECT_ALL = b("select_all", Develop, &[d(&[Key::A], rules::command, "Cmd+A")]);
        DEVELOP_SELECT_NONE = b("select_none", Develop, &[
            d(&[Key::D], rules::command, "Cmd+D"),
        ]);
        DEVELOP_UNDO = b("undo", Develop, &[
            d(&[Key::Z], |m| m.command && !m.shift, "Cmd+Z").off_card(),
        ]);
        DEVELOP_REDO = b("redo", Develop, &[
            d(&[Key::Z], rules::command_shift, "Cmd+Shift+Z").off_card(),
        ]);
        /// Ctrl+Y, read apart from Cmd+Z: both in one frame undo and redo.
        DEVELOP_REDO_Y = b("redo", Develop, &[
            d(&[Key::Y], |m| m.command && !m.shift, "Cmd+Y").not_macos().off_card(),
        ]);
        CROP = b("crop", Develop, &[
            d(&[Key::C], rules::no_command, "C").off_card(),
            d(&[Key::R], |m| !m.shift && !m.command, "R"),
        ]).once();
        REFERENCE_VIEW = b("reference_view", Develop, &[
            d(&[Key::R], |m| m.shift && !m.command && !m.alt, "Shift+R"),
        ]);
        DEVELOP_INFO = b("info", Develop, &[d(&[Key::I], rules::none, "I").once_per_press()]);
        CLIPPING = b("clipping", Develop, &[d(&[Key::J], rules::none, "J")]);
        // Shift+\ arrives as | on most layouts.
        CAMERA_THUMBNAIL = b("camera_thumbnail", Develop, &[
            d(&[Key::Pipe], rules::any, "|").off_card(),
            d(&[Key::Backslash], |m| m.shift, "Shift+\\"),
        ]);
        BEFORE_ONLY = b("compare", Develop, &[d(&[Key::Backslash], |m| !m.shift, "\\")]);
        FINISH_CROP = b("tool:finish", Context::Tool(Tool::Crop), &[
            d(&[Key::Enter], rules::any, "Return"),
        ]);
        FINISH_GUIDED = b("tool:finish", Context::Tool(Tool::Guided), &[
            d(&[Key::Enter], rules::any, "Return").off_card(),
        ]);
        WHITE_BALANCE = b("white_balance", Develop, &[d(&[Key::W], rules::none, "W")]);
        REMOVE = b("remove", Develop, &[d(&[Key::Q], rules::none, "Q")]);
        MASK = b("mask", Develop, &[d(&[Key::W], |m| m.shift && !m.command, "Shift+W")]);
        GUIDED_UPRIGHT = b("guided_upright", Develop, &[
            d(&[Key::T], |m| m.shift && !m.command, "Shift+T"),
        ]);
        /// Any tool but Masking, which takes a plain Escape itself.
        CLOSE_TOOL = b("tool:close", Develop, &[d(&[Key::Escape], rules::any, "Esc")]);
        /// Escape with Cmd or Alt, which Masking's own keys refuse: it closes at once.
        CLOSE_MASKING = b("tool:close", Context::Tool(Tool::Mask), &[
            d(&[Key::Escape], |m| m.command || m.alt, "Cmd+Esc").off_card(),
        ]);
        NEW_BRUSH_MASK = b("mask:new_brush", Develop, &[
            d(&[Key::K], rules::no_command_alt_shift, "K"),
        ]);
        NEW_LINEAR_MASK = b("mask:new_linear", Develop, &[
            d(&[Key::M], rules::no_command_alt_shift, "M"),
        ]);
        NEW_RADIAL_MASK = b("mask:new_radial", Develop, &[
            d(&[Key::M], |m| !m.command && !m.alt && m.shift, "Shift+M"),
        ]);
        NEW_COLOR_RANGE_MASK = b("mask:new_color_range", Develop, &[
            d(&[Key::J], |m| !m.command && !m.alt && m.shift, "Shift+J"),
        ]);

        // The tools' keys (`Editor::crop_keys` and the rest).
        CROP_SWAP_ORIENTATION = b("crop:swap_orientation", Context::Tool(Tool::Crop), &[
            d(&[Key::X], |m| crop_key(m) && !m.shift, "X"),
        ]);
        CROP_ASPECT_LOCK = b("crop:aspect_lock", Context::Tool(Tool::Crop), &[
            d(&[Key::A], |m| crop_key(m) && !m.shift, "A"),
        ]);
        CROP_NEXT_OVERLAY = b("crop:next_overlay", Context::Tool(Tool::Crop), &[
            d(&[Key::O], |m| crop_key(m) && !m.shift, "O"),
        ]);
        CROP_TURN_OVERLAY = b("crop:turn_overlay", Context::Tool(Tool::Crop), &[
            d(&[Key::O], |m| crop_key(m) && m.shift, "Shift+O"),
        ]);
        // Shift: feather.
        REMOVE_SMALLER = b("brush:smaller", Context::Tool(Tool::Remove), &[
            d(OPEN_BRACKETS, rules::no_command_alt, "["),
        ]);
        REMOVE_LARGER = b("brush:larger", Context::Tool(Tool::Remove), &[
            d(CLOSE_BRACKETS, rules::no_command_alt, "]"),
        ]);
        REMOVE_NEW_SOURCE = b("remove:new_source", Context::Tool(Tool::Remove), &[
            d(SLASHES, rules::no_command_alt, "/"),
        ]);
        REMOVE_DELETE = b("tool:delete", Context::Tool(Tool::Remove), &[
            d(DELETE_KEYS, rules::no_command_alt, "Delete"),
        ]);
        REMOVE_HIDE_PINS = b("remove:hide_pins", Context::Tool(Tool::Remove), &[
            d(&[Key::H], rules::no_command_alt_shift, "H"),
        ]);
        /// Not a quick Cmd+A, Select All, released within the frame.
        REMOVE_VISUALIZE = b("remove:visualize", Context::Tool(Tool::Remove), &[
            d(&[Key::A], rules::none, "A"),
        ]);
        RED_EYE_DELETE = b("tool:delete", Context::Tool(Tool::RedEye), &[
            d(DELETE_KEYS, rules::no_command_alt, "Delete").off_card(),
        ]);
        RED_EYE_SMALLER = b("brush:smaller", Context::Tool(Tool::RedEye), &[
            d(OPEN_BRACKETS, rules::no_command_alt, "[").off_card(),
        ]);
        RED_EYE_LARGER = b("brush:larger", Context::Tool(Tool::RedEye), &[
            d(CLOSE_BRACKETS, rules::no_command_alt, "]").off_card(),
        ]);
        MASK_SMALLER = b("brush:smaller", Context::Tool(Tool::Mask), &[
            d(OPEN_BRACKETS, rules::no_command_alt, "[").off_card(),
        ]);
        MASK_LARGER = b("brush:larger", Context::Tool(Tool::Mask), &[
            d(CLOSE_BRACKETS, rules::no_command_alt, "]").off_card(),
        ]);
        MASK_BACK = b("mask:back", Context::Tool(Tool::Mask), &[
            d(&[Key::Escape], rules::no_command_alt, "Esc").off_card(),
        ]);
        MASK_OVERLAY = b("mask:overlay", Context::Tool(Tool::Mask), &[
            d(&[Key::O], rules::no_command_alt_shift, "O").off_card(),
        ]);
        MASK_SWITCH_BRUSH = b("mask:switch_brush", Context::Tool(Tool::Mask), &[
            d(SLASHES, rules::no_command_alt, "/").off_card(),
        ]);
        MASK_DELETE = b("tool:delete", Context::Tool(Tool::Mask), &[
            d(DELETE_KEYS, rules::no_command_alt, "Delete").off_card(),
        ]);
        GUIDED_DELETE = b("tool:delete", Context::Tool(Tool::Guided), &[
            d(DELETE_KEYS, rules::no_command_alt, "Delete").off_card(),
        ]);

        // The Basic panel's keys (`Editor::develop_key_steps`), by their place.
        BASIC_NEXT = b("basic:next", Develop, &[d(&[Key::Period], rules::none, ".").placed()]);
        BASIC_PREVIOUS = b("basic:previous", Develop, &[
            d(&[Key::Comma], rules::none, ",").placed(),
        ]);
        BASIC_INCREASE = b("basic:increase", Develop, &[
            d(&[Key::Equals, Key::Plus], basic_step, "=").placed(),
        ]);
        BASIC_DECREASE = b("basic:decrease", Develop, &[
            d(&[Key::Minus], basic_step, "-").placed(),
        ]);
    }

    /// A Basic slider step: never with Cmd or Ctrl, which make other keys;
    /// Shift steps more, Option less.
    fn basic_step(m: Modifiers) -> bool {
        !(m.command || m.ctrl || m.mac_cmd)
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
