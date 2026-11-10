//! The golden table of every keyboard shortcut (#963): every key a shortcut
//! uses, in every place, with every combination of Cmd, Ctrl, Alt and Shift,
//! pressed afresh and held to repeat, and what each press changed.
//!
//! The table is checked in (`keymap_golden.txt`) and compared exactly. Moving the
//! shortcuts into one registry must reproduce it byte for byte; a change of
//! behaviour shows as its diff. It is written again only when
//! `RAWMAKASE_BLESS_KEYMAP=1` is set, never by default.
//!
//! A row reads `place key modifiers press effect`, the effect being the
//! fingerprint fields the frame changed ([`Fixture::fingerprint`]), and `taken`
//! when the key's press was no longer in the frame's input after the editor drew.
//! Each row starts from a newly built editor ([`Template::fixture`]), except that
//! a key's next modifiers are pressed in the same editor while its presses change
//! nothing (see [`key_rows`]). A table built with a new editor for every row was
//! the same, byte for byte.
//!
//! The rows are written for the platform the tests run on, as egui-winit
//! reports modifiers there. Linux and Windows share `keymap_golden.txt`; macOS,
//! where Cmd and Ctrl differ and Ctrl+Y is not Redo, has no table yet, and the
//! test is ignored there.
//!
//! Run it, as every keyboard shortcut pin, with `RAWMAKASE_DATA_DIR` and
//! `XDG_DATA_HOME` set to an empty folder: Export with Previous reads the last
//! export's settings from there.
use super::keymap_pins_harness::*;
use eframe::egui::{Key, Modifiers};
use std::collections::HashMap;

/// Every key the shortcuts use.
const KEYS: [Key; 68] = [
    Key::A,
    Key::B,
    Key::C,
    Key::D,
    Key::E,
    Key::F,
    Key::G,
    Key::H,
    Key::I,
    Key::J,
    Key::K,
    Key::L,
    Key::M,
    Key::N,
    Key::O,
    Key::P,
    Key::Q,
    Key::R,
    Key::S,
    Key::T,
    Key::U,
    Key::V,
    Key::W,
    Key::X,
    Key::Y,
    Key::Z,
    Key::Num0,
    Key::Num1,
    Key::Num2,
    Key::Num3,
    Key::Num4,
    Key::Num5,
    Key::Num6,
    Key::Num7,
    Key::Num8,
    Key::Num9,
    Key::OpenBracket,
    Key::CloseBracket,
    Key::OpenCurlyBracket,
    Key::CloseCurlyBracket,
    Key::Backtick,
    Key::Quote,
    Key::Comma,
    Key::Period,
    Key::Minus,
    Key::Equals,
    Key::Plus,
    Key::Slash,
    Key::Questionmark,
    Key::Backslash,
    Key::Pipe,
    Key::ArrowLeft,
    Key::ArrowRight,
    Key::ArrowUp,
    Key::ArrowDown,
    Key::Home,
    Key::End,
    Key::PageUp,
    Key::PageDown,
    Key::Escape,
    Key::Enter,
    Key::Tab,
    Key::Delete,
    Key::Backspace,
    Key::F2,
    Key::F6,
    Key::F7,
    Key::F8,
];

/// The table for this platform. egui-winit reports Ctrl as the command key off
/// the Mac, so Linux and Windows share one.
const TABLE: &str = if cfg!(target_os = "macos") {
    "src/app/shell/keymap_golden_macos.txt"
} else {
    "src/app/shell/keymap_golden.txt"
};

/// The sixteen combinations of Cmd, Ctrl, Alt and Shift, by name.
fn combinations() -> Vec<(String, Modifiers)> {
    (0..16u8)
        .map(|bits| {
            let [cmd, ctrl, alt, shift] = [1, 2, 4, 8].map(|bit| bits & bit != 0);
            let name: Vec<&str> = [(cmd, "Cmd"), (ctrl, "Ctrl"), (alt, "Alt"), (shift, "Shift")]
                .into_iter()
                .filter_map(|(held, name)| held.then_some(name))
                .collect();
            let name = if name.is_empty() {
                "-".to_string()
            } else {
                name.join("+")
            };
            (name, modifiers([cmd, ctrl, alt, shift]))
        })
        .collect()
}

/// What one press of `key` with `m` held does in a new editor in `place`.
pub(super) fn effect(
    t: &mut Template,
    place: Place,
    key: Key,
    m: Modifiers,
    repeat: bool,
) -> String {
    let mut f = t.fixture(place);
    effect_on(&mut f, t.path(), key, m, repeat)
}

/// What one press of `key` with `m` held does to `f`; `dir`, the template's
/// folder, is written `<dir>` where a message names it.
fn effect_on(
    f: &mut Fixture,
    dir: &std::path::Path,
    key: Key,
    m: Modifiers,
    repeat: bool,
) -> String {
    let (before, left) = if repeat {
        f.hold(key, m);
        let before = f.fingerprint();
        (before, f.first_repeat(key, m))
    } else {
        let before = f.fingerprint();
        (before, f.frame_watching(tap_events(key, key, m), Some(key)))
    };
    let mut effect = changes(&before, &f.fingerprint());
    if left == Some(false) {
        effect.push("taken".into());
    }
    let effect = if effect.is_empty() {
        "-".to_string()
    } else {
        effect.join("; ")
    };
    effect.replace(&dir.display().to_string(), "<dir>")
}

/// The rows of `key` in `place`: one per modifiers and press.
///
/// A press that changed nothing, and was not taken, leaves its editor as it
/// found it as far as any row can see, so the key's next modifiers are pressed
/// in the same editor rather than a new one: building one is most of a row's
/// time. Any press that changed something is followed by a new editor.
fn key_rows(t: &mut Template, place: Place, key: Key) -> Vec<String> {
    let mut lines = Vec::new();
    // Off the Mac, Cmd and Ctrl are the same modifiers: each is pressed once.
    let mut seen: HashMap<(String, bool), String> = HashMap::new();
    for repeat in [false, true] {
        let mut unchanged: Option<Fixture> = None;
        for (name, m) in combinations() {
            let effect = seen
                .entry((format!("{m:?}"), repeat))
                .or_insert_with(|| {
                    let mut f = unchanged.take().unwrap_or_else(|| t.fixture(place));
                    let effect = effect_on(&mut f, t.path(), key, m, repeat);
                    if effect == "-" {
                        unchanged = Some(f);
                    }
                    effect
                })
                .clone();
            let press = if repeat { "repeat" } else { "fresh" };
            lines.push(format!(
                "{place:?}\t{}\t{name}\t{press}\t{effect}",
                key.name()
            ));
        }
    }
    lines
}

/// The whole table. Each (place, key) is one piece of work for whichever
/// thread is free; each thread builds its editors from templates of its own.
fn table() -> String {
    let work: Vec<(Place, Option<Key>)> = Place::ALL
        .into_iter()
        .flat_map(|place| std::iter::once((place, None)).chain(KEYS.map(|key| (place, Some(key)))))
        .collect();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut done: Vec<(usize, Vec<String>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut templates: HashMap<String, Template> = HashMap::new();
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some((place, key)) = work.get(i).copied() else {
                            return done;
                        };
                        let t = templates
                            .entry(format!("{place:?}"))
                            .or_insert_with(Template::new);
                        let lines = match key {
                            // The place's start, as a comment heading its rows.
                            None => vec![format!(
                                "# {place:?}: {}",
                                t.fixture(place)
                                    .fingerprint()
                                    .iter()
                                    .map(|(name, value)| format!("{name}={value}"))
                                    .collect::<Vec<_>>()
                                    .join("; ")
                            )],
                            Some(key) => key_rows(t, place, key),
                        };
                        done.push((i, lines));
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("a thread's rows"))
            .collect()
    });
    done.sort_by_key(|(i, _)| *i);
    let mut out = String::from(
        "# Every key the shortcuts use, in every place, with every combination of\n\
         # Cmd, Ctrl, Alt and Shift, pressed afresh and held to its first repeat:\n\
         # place, key, modifiers, press, and the fields of the fingerprint the frame\n\
         # changed (`taken` when the press had left the frame's input). Written by\n\
         # keymap_golden_editor_tests with RAWMAKASE_BLESS_KEYMAP=1.\n",
    );
    for line in done.into_iter().flat_map(|(_, lines)| lines) {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "no macOS table yet: write one on a Mac with RAWMAKASE_BLESS_KEYMAP=1"
)]
fn every_key_in_every_place_does_what_the_table_says() {
    let started = std::time::Instant::now();
    let table = table();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(TABLE);
    if std::env::var_os("RAWMAKASE_BLESS_KEYMAP").is_some_and(|v| v == "1") {
        std::fs::write(&path, &table).expect("the table is written");
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_default();
    if table != expected {
        let differing: Vec<String> = table
            .lines()
            .zip(expected.lines())
            .filter(|(now, then)| now != then)
            .take(40)
            .map(|(now, then)| format!("  was: {then}\n  now: {now}"))
            .collect();
        panic!(
            "{} rows differ from {TABLE} ({} lines now, {} then); the first:\n{}",
            table
                .lines()
                .zip(expected.lines())
                .filter(|(now, then)| now != then)
                .count(),
            table.lines().count(),
            expected.lines().count(),
            differing.join("\n")
        );
    }
    eprintln!("the keymap table took {:?}", started.elapsed());
}

/// Every action the shortcuts take, where it is taken, so the table can only
/// show a change of behaviour if the start gives each action something to do.
#[test]
fn every_action_changes_the_fingerprint_from_its_places_start() {
    let none = Modifiers::NONE;
    let (shift, alt) = (Modifiers::SHIFT, Modifiers::ALT);
    let cmd = command(none);
    let cmd_shift = command(shift);
    let cmd_alt = command(alt);
    let cmd_alt_shift = command(alt | shift);
    use Place::*;
    let mut actions: Vec<(Place, Key, Modifiers, &str)> = vec![
        // Everywhere.
        (Grid, Key::Escape, shift, "dismiss messages"),
        (Develop, Key::G, none, "Library grid"),
        (Loupe, Key::G, none, "Library grid from the Loupe"),
        (Develop, Key::E, none, "Library Loupe"),
        (Grid, Key::Quote, cmd, "virtual copy"),
        (Develop, Key::Quote, cmd, "virtual copy in Develop"),
        (Grid, Key::F, none, "hide the chrome"),
        (Grid, Key::D, none, "develop the active photo"),
        (Grid, Key::Z, cmd, "undo"),
        (Grid, Key::Z, cmd_shift, "redo"),
        (Grid, Key::Comma, cmd, "Preferences"),
        (GridTyping, Key::Comma, cmd, "Preferences while typing"),
        (Grid, Key::Slash, cmd, "the sheet"),
        (Grid, Key::Tab, none, "panels: all sides"),
        (Grid, Key::Tab, shift, "panels: all"),
        (Grid, Key::F6, none, "filmstrip"),
        (Grid, Key::F7, none, "left panel"),
        (Grid, Key::F8, none, "right panel"),
        (Grid, Key::F8, shift, "Presets panel"),
        (Develop, Key::F8, shift, "Develop's tab panel"),
        // Metadata.
        (Grid, Key::ArrowUp, cmd, "flag +1"),
        (Grid, Key::ArrowDown, cmd, "flag -1"),
        (Grid, Key::P, none, "pick"),
        (Grid, Key::U, none, "unflag"),
        (Grid, Key::X, none, "reject"),
        (Grid, Key::Backtick, none, "toggle pick"),
        (Grid, Key::OpenBracket, none, "rating -1"),
        (Grid, Key::CloseBracket, none, "rating +1"),
        (Loupe, Key::Num3, shift, "rating and advance"),
    ];
    for (i, key) in [
        Key::Num0,
        Key::Num1,
        Key::Num2,
        Key::Num3,
        Key::Num4,
        Key::Num5,
        Key::Num6,
        Key::Num7,
        Key::Num8,
        Key::Num9,
    ]
    .into_iter()
    .enumerate()
    {
        actions.push((
            Grid,
            key,
            none,
            [
                "0", "1", "2", "3", "4", "5", "Red", "Yellow", "Green", "Blue",
            ][i],
        ));
    }
    actions.extend([
        // The grid.
        (Grid, Key::E, none, "Loupe"),
        (Grid, Key::Enter, none, "Loupe by Enter"),
        (Grid, Key::C, none, "Compare"),
        (Grid, Key::N, none, "Survey"),
        (Grid, Key::J, none, "cell style"),
        (Grid, Key::B, none, "Quick Collection toggle"),
        (Grid, Key::B, cmd, "show the Quick Collection"),
        (Grid, Key::B, cmd_shift, "clear the Quick Collection"),
        (Grid, Key::Delete, none, "delete"),
        (Grid, Key::Backspace, none, "delete by Backspace"),
        (Grid, Key::Delete, alt, "remove from the catalog"),
        (Grid, Key::Delete, cmd, "delete rejected photos"),
        (Grid, Key::Delete, cmd_alt_shift, "move to the Trash"),
        (Grid, Key::F2, none, "rename"),
        (Grid, Key::G, cmd, "stack"),
        (Grid, Key::S, none, "stack collapse"),
        (Grid, Key::A, cmd, "select all"),
        (Grid, Key::D, cmd, "select none"),
        (Grid, Key::Slash, none, "deselect the active photo"),
        (Grid, Key::ArrowLeft, none, "left"),
        (Grid, Key::ArrowRight, none, "right"),
        (Grid, Key::ArrowUp, none, "up"),
        (Grid, Key::ArrowDown, shift, "down, extending"),
        (Grid, Key::PageUp, none, "page up"),
        (Grid, Key::PageDown, none, "page down"),
        (Grid, Key::Home, none, "home"),
        (Grid, Key::End, none, "end"),
        (Grid, Key::L, cmd, "filters"),
        (GridTyping, Key::L, cmd, "filters while typing"),
        (Grid, Key::E, cmd_shift, "export"),
        (Grid, Key::E, cmd_alt_shift, "export with previous"),
        (Grid, Key::S, cmd_shift, "sync"),
        (Grid, Key::U, cmd_shift, "auto settings"),
        // The Loupe.
        (Loupe, Key::ArrowLeft, none, "previous"),
        (Loupe, Key::ArrowUp, none, "previous by Up"),
        (Loupe, Key::ArrowRight, none, "next"),
        (Loupe, Key::ArrowDown, none, "next by Down"),
        (Loupe, Key::Home, none, "first"),
        (Loupe, Key::End, none, "last"),
        (Loupe, Key::Escape, none, "close the Loupe"),
        (Loupe, Key::Delete, none, "delete the photo"),
        (Loupe, Key::F2, none, "rename the photo"),
        (Loupe, Key::C, none, "Compare"),
        (Loupe, Key::N, none, "Survey"),
        (Loupe, Key::B, none, "Quick Collection"),
        (Loupe, Key::I, none, "info"),
        (Loupe, Key::S, none, "stack"),
        (Loupe, Key::Num0, cmd, "100%"),
        (Loupe, Key::Equals, cmd, "zoom in"),
        (Loupe, Key::Plus, cmd, "zoom in by +"),
        (Loupe, Key::Z, none, "zoom"),
        (Loupe, Key::Z, shift, "magnifier"),
        // Compare and Survey.
        (Compare, Key::B, none, "Quick Collection"),
        (Compare, Key::ArrowLeft, none, "candidate -1"),
        (Compare, Key::ArrowRight, none, "candidate +1"),
        (Compare, Key::ArrowUp, none, "make select"),
        (Compare, Key::ArrowDown, none, "swap"),
        (Compare, Key::Escape, none, "close"),
        (Compare, Key::E, none, "Loupe"),
        (Compare, Key::Enter, none, "Loupe by Enter"),
        (Compare, Key::N, none, "Survey"),
        (Survey, Key::B, none, "Quick Collection"),
        (Survey, Key::ArrowLeft, none, "previous"),
        (Survey, Key::ArrowUp, none, "previous by Up"),
        (Survey, Key::Escape, none, "close"),
        (Survey, Key::E, none, "Loupe"),
        (Survey, Key::Enter, none, "Loupe by Enter"),
        (Survey, Key::C, none, "Compare"),
        // Develop.
        (Develop, Key::Num0, cmd, "100%"),
        (Develop, Key::Equals, cmd, "zoom in"),
        (Develop, Key::Z, none, "zoom"),
        (Develop, Key::Z, shift, "magnifier"),
        (Develop, Key::Delete, none, "delete"),
        (Develop, Key::ArrowLeft, cmd_alt_shift, "After to Before"),
        (Develop, Key::ArrowRight, cmd_alt_shift, "Before to After"),
        (
            Develop,
            Key::ArrowUp,
            cmd_alt_shift,
            "swap Before and After",
        ),
        (Develop, Key::ArrowLeft, none, "previous photo"),
        (Develop, Key::ArrowRight, none, "next photo"),
        (Develop, Key::Y, none, "Before/After"),
        (Develop, Key::Y, alt, "Before/After top and bottom"),
        (Develop, Key::Y, shift, "Before/After split"),
        (Develop, Key::C, cmd_shift, "Copy Settings"),
        (Develop, Key::V, cmd_shift, "Paste Settings"),
        (Develop, Key::V, cmd_alt, "Paste from Previous"),
        (Develop, Key::M, cmd_alt_shift, "Match Total Exposures"),
        (Develop, Key::N, cmd_shift, "New Develop Preset"),
        (Develop, Key::T, cmd_alt_shift, "targeted Tone Curve"),
        (Develop, Key::H, cmd_alt_shift, "targeted Hue"),
        (Develop, Key::S, cmd_alt_shift, "targeted Saturation"),
        (Develop, Key::L, cmd_alt_shift, "targeted Luminance"),
        (Develop, Key::G, cmd_alt_shift, "targeted B&W"),
        (Develop, Key::S, cmd_shift, "Sync"),
        (Develop, Key::R, cmd_shift, "Reset"),
        (Develop, Key::V, none, "B&W"),
        (Develop, Key::U, cmd_shift, "Auto Settings"),
        (Develop, Key::E, cmd_shift, "Export"),
        (Develop, Key::E, cmd_alt_shift, "Export with Previous"),
        (Develop, Key::A, cmd, "select all"),
        (Develop, Key::D, cmd, "select none"),
        (Develop, Key::Z, cmd, "undo"),
        (Develop, Key::Z, cmd_shift, "redo"),
        (Develop, Key::C, none, "Crop"),
        (Develop, Key::R, none, "Crop by R"),
        (Develop, Key::R, shift, "Reference View"),
        (Develop, Key::I, none, "info"),
        (Develop, Key::J, none, "clipping"),
        (Develop, Key::Pipe, none, "camera thumbnail"),
        (
            Develop,
            Key::Backslash,
            shift,
            "camera thumbnail by Shift+\\",
        ),
        (Develop, Key::Backslash, none, "Before only"),
        (Crop, Key::Enter, none, "Crop away"),
        (Guided, Key::Enter, none, "Guided away"),
        (Develop, Key::W, none, "White Balance"),
        (Develop, Key::Q, none, "Remove"),
        (Develop, Key::W, shift, "Masking"),
        (Develop, Key::T, shift, "Guided"),
        (Crop, Key::Escape, none, "tool away"),
        (Develop, Key::K, none, "brush mask"),
        (Develop, Key::M, none, "linear mask"),
        (Develop, Key::M, shift, "radial mask"),
        (Develop, Key::J, shift, "colour range mask"),
        // The tools.
        (Crop, Key::X, none, "crop orientation"),
        (Crop, Key::A, none, "aspect lock"),
        (Crop, Key::O, none, "next overlay"),
        (Remove, Key::OpenBracket, none, "smaller"),
        (Remove, Key::OpenBracket, shift, "less feather"),
        (Remove, Key::CloseBracket, none, "larger"),
        (Remove, Key::CloseBracket, shift, "more feather"),
        (Remove, Key::Slash, none, "new source"),
        (Remove, Key::Delete, none, "delete the spot"),
        (Remove, Key::H, none, "hide pins"),
        (Remove, Key::A, none, "visualize spots"),
        (RedEye, Key::Delete, none, "delete the correction"),
        (RedEye, Key::OpenBracket, none, "smaller"),
        (RedEye, Key::CloseBracket, none, "larger"),
        (Mask, Key::OpenBracket, none, "smaller brush"),
        (Mask, Key::CloseBracket, shift, "more feather"),
        (Mask, Key::Escape, none, "close Masking"),
        (Mask, Key::O, none, "overlay"),
        (Mask, Key::Slash, none, "brush A/B"),
        (Mask, Key::Delete, none, "delete the mask"),
        (Guided, Key::Delete, none, "delete the guide"),
        // Basic.
        (Develop, Key::Period, none, "next slider"),
        (Develop, Key::Comma, none, "previous slider"),
        (Develop, Key::Equals, none, "step up"),
        (Develop, Key::Plus, none, "step up by +"),
        (Develop, Key::Minus, none, "step down"),
    ]);
    let mut t = Template::new();
    let idle: Vec<String> = actions
        .iter()
        .filter(|(place, key, m, _)| {
            let effect = effect(&mut t, *place, *key, *m, false);
            effect == "-" || effect == "taken"
        })
        .map(|(place, key, m, what)| format!("{what} ({place:?} {key:?} {m:?})"))
        .collect();
    assert!(
        idle.is_empty(),
        "these change nothing from their start: {idle:#?}"
    );
}
