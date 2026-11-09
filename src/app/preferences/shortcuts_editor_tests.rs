//! Preferences > Shortcuts through its UI: the whole editor drawn with the
//! window open on the page, clicked and typed at as a user would.
use crate::app::shell::keymap::file::{KeymapFile, State};
use crate::app::shell::keymap::{Chord, Mods};
use crate::app::*;
use eframe::egui::{Event, Key, Modifiers};

struct Page {
    ctx: egui::Context,
    e: Editor,
    tree: Option<egui::accesskit::TreeUpdate>,
    _dir: tempfile::TempDir,
}

impl Page {
    /// The editor with Preferences open on Shortcuts, its key map in a
    /// temporary folder's keymap.toml, holding `file` if given.
    fn new(file: Option<&str>) -> Self {
        let dir = tempfile::tempdir().expect("a temporary folder");
        let path = dir.path().join("keymap.toml");
        if let Some(text) = file {
            std::fs::write(&path, text).expect("written");
        }
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        ctx.all_styles_mut(|s| s.animation_time = 0.);
        let mut e = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        e.onboarding.visible = false;
        e.keymap = KeymapFile::load(path);
        e.open_preferences(super::super::Tab::Shortcuts);
        let mut page = Self {
            ctx,
            e,
            tree: None,
            _dir: dir,
        };
        page.frame(Vec::new());
        page.frame(Vec::new());
        page
    }
    fn frame(&mut self, events: Vec<Event>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600., 1000.),
            )),
            events,
            ..Default::default()
        };
        let e = &mut self.e;
        let mut output = self.ctx.run_ui(input, |ui| e.draw(ui));
        output.textures_delta.clear();
        self.tree = Some(
            output
                .platform_output
                .accesskit_update
                .expect("accessibility is on"),
        );
    }
    fn node(&self, label: &str) -> Option<&egui::accesskit::Node> {
        crate::app::testing::labelled(self.tree.as_ref()?, label)
    }
    fn labels(&self) -> Vec<String> {
        self.tree
            .iter()
            .flat_map(|t| t.nodes.iter())
            .filter_map(|(_, n)| n.label().or(n.value()).map(str::to_string))
            .collect()
    }
    fn shows(&self, label: &str) -> bool {
        self.node(label).is_some()
    }
    fn click(&mut self, label: &str) {
        let at = crate::app::testing::centre(
            self.node(label)
                .unwrap_or_else(|| panic!("no {label} on the page")),
        );
        for events in crate::app::testing::press_and_release(at) {
            self.frame(events);
        }
        self.frame(Vec::new());
    }
    /// `key` pressed and released with `m`, as egui-winit sends it.
    fn press(&mut self, key: Key, m: Modifiers) {
        self.press_at(key, key, m);
    }
    /// `key` typed by the key at `physical`, with `m`.
    fn press_at(&mut self, key: Key, physical: Key, m: Modifiers) {
        let event = |pressed| Event::Key {
            key,
            physical_key: Some(physical),
            pressed,
            repeat: false,
            modifiers: m,
        };
        self.frame(vec![
            Event::ModifiersChanged(m),
            event(true),
            event(false),
            Event::ModifiersChanged(Modifiers::NONE),
        ]);
        self.frame(Vec::new());
    }
    fn keys(&self, id: &str) -> Vec<String> {
        super::keys_of(&self.e.keymap.effective(), id)
            .iter()
            .map(Chord::to_string)
            .collect()
    }
    fn query(&mut self, query: &str) {
        self.e.preferences.shortcuts.query = query.into();
        self.frame(Vec::new());
        self.frame(Vec::new());
    }
}

fn key_event(key: Key, pressed: bool, modifiers: Modifiers) -> Event {
    Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat: false,
        modifiers,
    }
}
fn cmd() -> Modifiers {
    Modifiers::COMMAND
        | if cfg!(target_os = "macos") {
            Modifiers::MAC_CMD
        } else {
            Modifiers::CTRL
        }
}
fn cmd_text(key: &str) -> String {
    Chord::new(Key::from_name(key).expect("a key"), Mods::CMD).to_string()
}

#[test]
fn shortcuts_sit_between_display_and_automation() {
    let titles: Vec<&str> = super::super::Tab::ALL.iter().map(|t| t.title()).collect();
    assert_eq!(
        titles,
        [
            "General",
            "Catalog",
            "Profiles & Presets",
            "Performance",
            "Display",
            "Shortcuts",
            "Automation"
        ]
    );
}

#[test]
fn a_key_cap_takes_the_next_key_pressed_with_its_modifiers() {
    let mut page = Page::new(None);
    assert_eq!(page.keys("crop"), ["C", "R"]);
    page.query("straighten");
    page.click("Crop & Straighten: R");
    assert!(page.e.capturing_keys());
    // A modifier pressed alone is a key of its own to egui-winit: the
    // capture waits for the key it is held with, in a later frame...
    page.frame(vec![
        Event::ModifiersChanged(cmd()),
        key_event(Key::ControlLeft, true, cmd()),
    ]);
    assert!(page.e.capturing_keys(), "still waiting");
    page.frame(vec![
        key_event(Key::F9, true, cmd()),
        key_event(Key::F9, false, cmd()),
        key_event(Key::ControlLeft, false, Modifiers::NONE),
        Event::ModifiersChanged(Modifiers::NONE),
    ]);
    assert!(!page.e.capturing_keys());
    assert_eq!(page.keys("crop"), ["C".to_string(), cmd_text("F9")]);
    // ...or in the same frame.
    page.frame(Vec::new());
    page.click(&format!("Crop & Straighten: {}", cmd_text("F9")));
    page.frame(vec![
        Event::ModifiersChanged(Modifiers::SHIFT),
        key_event(Key::ShiftLeft, true, Modifiers::SHIFT),
        key_event(Key::F10, true, Modifiers::SHIFT),
        key_event(Key::F10, false, Modifiers::SHIFT),
        key_event(Key::ShiftLeft, false, Modifiers::NONE),
        Event::ModifiersChanged(Modifiers::NONE),
    ]);
    assert_eq!(page.keys("crop"), ["C", "Shift+F10"]);
    // The file holds it.
    let text = std::fs::read_to_string(page.e.keymap.path().unwrap()).unwrap();
    assert!(text.contains("crop = [\"C\", \"Shift+F10\"]"), "{text}");
}

#[test]
fn a_captured_key_runs_no_command() {
    let mut page = Page::new(None);
    page.e.module = Module::Develop;
    page.frame(Vec::new());
    let clipping = format!("{:?}", page.e.view.clipping);
    page.query("straighten");
    page.click("Crop & Straighten: R");
    // J is a key to bind here, not Develop's clipping.
    page.press(Key::J, Modifiers::NONE);
    assert_eq!(format!("{:?}", page.e.view.clipping), clipping);
    assert_eq!(page.keys("crop"), ["C", "J"]);
    assert!(page.e.preferences.open);
}

#[test]
fn escape_keeps_the_keys_and_the_window() {
    let mut page = Page::new(None);
    page.query("straighten");
    page.click("Add a key to Crop & Straighten");
    assert!(page.e.capturing_keys());
    page.press(Key::Escape, Modifiers::NONE);
    assert!(!page.e.capturing_keys());
    assert!(page.e.preferences.open, "Escape went to the capture");
    assert_eq!(page.keys("crop"), ["C", "R"]);
    assert!(!page.e.keymap.path().unwrap().exists(), "nothing written");
}

#[test]
fn delete_takes_the_key_away_and_add_adds_one() {
    let mut page = Page::new(None);
    page.query("straighten");
    page.click("Crop & Straighten: C");
    page.press(Key::Delete, Modifiers::NONE);
    assert_eq!(page.keys("crop"), ["R"]);
    page.click("Add a key to Crop & Straighten");
    page.press(Key::F9, Modifiers::NONE);
    assert_eq!(page.keys("crop"), ["R", "F9"]);
    // Taking the last key away leaves none.
    for key in ["R", "F9"] {
        page.click(&format!("Crop & Straighten: {key}"));
        page.press(Key::Backspace, Modifiers::NONE);
    }
    assert_eq!(page.keys("crop"), Vec::<String>::new());
    assert!(page.e.keymap.effective().overrides("crop"));
}

#[test]
fn a_refused_key_says_why_under_its_row() {
    let mut page = Page::new(None);
    page.query("straighten");
    page.click("Add a key to Crop & Straighten");
    // Select All belongs to text fields.
    page.press(Key::A, cmd());
    assert_eq!(page.keys("crop"), ["C", "R"]);
    assert!(
        page.labels()
            .iter()
            .any(|l| l.contains("text fields keep this key")),
        "the reason shows"
    );
}

#[test]
fn search_finds_commands_by_title_and_by_key() {
    let mut page = Page::new(None);
    page.query("straigh");
    assert!(page.shows("Crop & Straighten"));
    assert!(!page.shows("Show shadow and highlight clipping"));
    page.query("cmd shift e");
    assert!(page.shows("Export…"));
    assert!(!page.shows("Crop & Straighten"));
    page.query("nothing like this");
    assert!(page.shows("No shortcuts match."));
}

#[test]
fn reset_puts_back_one_commands_keys_and_reset_all_every_ones() {
    let mut page = Page::new(Some(
        "version = 1\n# mine\n[bindings]\ncrop = [\"F9\"]\nclipping = [\"F10\"]\n",
    ));
    assert_eq!(page.keys("crop"), ["F9"]);
    page.query("crop");
    assert!(
        !page.shows("Reset Lock or unlock the crop's aspect"),
        "not changed"
    );
    page.click("Reset Crop & Straighten");
    page.query("");
    assert_eq!(page.keys("crop"), ["C", "R"]);
    assert_eq!(page.keys("clipping"), ["F10"]);
    page.click("Reset All");
    assert_eq!(page.keys("clipping"), ["J"]);
    let text = std::fs::read_to_string(page.e.keymap.path().unwrap()).unwrap();
    assert_eq!(text, "version = 1\n# mine\n[bindings]\n");
}

#[test]
fn a_conflict_is_marked_on_both_rows_and_can_be_shown_alone() {
    let mut page = Page::new(Some(
        "version = 1\n[bindings]\nclipping = [\"F9\"]\n\"basic:next\" = [\"F9\"]\n",
    ));
    let badge = |page: &Page, other: &str| {
        page.shows(&format!(
            "Conflicts with \u{201c}{other}\u{201d} on F9 in Develop: neither runs"
        ))
    };
    assert!(badge(&page, "Select the next Basic slider"));
    assert!(badge(&page, "Show shadow and highlight clipping"));
    assert!(page.shows("Crop & Straighten"));
    page.click("Show conflicts only");
    assert!(page.e.preferences.shortcuts.conflicts_only);
    assert!(!page.shows("Crop & Straighten"));
    assert!(page.shows("Show shadow and highlight clipping"));
    assert!(page.shows("Select the next Basic slider"));
}

#[test]
fn fixed_keys_are_shown_locked() {
    let mut page = Page::new(None);
    page.query("undo");
    let undo = format!("Undo: {}", cmd_text("Z"));
    assert!(page.shows(&undo));
    assert!(!page.shows("Add a key to Undo"));
    page.click(&undo);
    assert!(!page.e.capturing_keys(), "a locked key starts no capture");
}

#[test]
fn a_file_that_doesnt_read_leaves_only_reset_all() {
    let broken = "version = 1\n[bindings\n";
    let mut page = Page::new(Some(broken));
    assert!(
        page.labels()
            .iter()
            .any(|l| l.starts_with("keymap.toml isn't used")),
        "a banner says so"
    );
    page.query("straighten");
    assert!(!page.shows("Add a key to Crop & Straighten"));
    page.click("Crop & Straighten: R");
    assert!(!page.e.capturing_keys());
    let path = page.e.keymap.path().unwrap().to_path_buf();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
    page.click("Reset All");
    assert_eq!(*page.e.keymap.state(), State::Read);
    let aside = path.with_file_name("keymap.toml.broken");
    assert_eq!(std::fs::read_to_string(aside).unwrap(), broken);
    assert!(page.shows("Add a key to Crop & Straighten"));
}

#[test]
fn commands_this_release_doesnt_know_are_listed() {
    let mut page = Page::new(Some(
        "version = 1\n[bindings]\n\"someone/extension\" = [\"F9\"]\ncrop = [1]\n",
    ));
    // At the end of the page, after every command: narrowed to nothing.
    page.query("zzz");
    assert!(page.shows("No shortcuts match."));
    assert!(page.shows("someone/extension"));
    assert!(page.shows("crop: 1 isn't a key in quotes. Its default keys apply."));
}

#[test]
fn a_captured_key_is_saved_as_its_command_reads_keys() {
    // The Basic keys go by their place: Dvorak types = on the ] key, which
    // is then the key to press, not whatever types =.
    let mut page = Page::new(None);
    page.query("raise the selected basic");
    let title = "Raise the selected Basic slider (Shift: more, Option: less)";
    page.click(&format!("{title}: +"));
    page.press_at(Key::Equals, Key::CloseBracket, Modifiers::NONE);
    assert_eq!(page.keys("basic:increase"), ["=", "]"]);
    // A command that goes by what keys type saves the typed key: German
    // Cmd+Shift+7 is Cmd+/.
    page.query("keyboard shortcuts");
    page.click(&format!("Keyboard shortcuts: {}", cmd_text("/")));
    page.press_at(Key::Slash, Key::Num7, cmd() | Modifiers::SHIFT);
    assert_eq!(page.keys("shortcuts"), [cmd_text("/")]);
}

#[test]
fn a_click_elsewhere_stops_a_key_cap_waiting() {
    let mut page = Page::new(None);
    page.query("straighten");
    page.click("Crop & Straighten: R");
    assert!(page.e.preferences.shortcuts.capturing());
    // Back into the search field, to type in it.
    page.click("straighten");
    assert!(!page.e.preferences.shortcuts.capturing());
    page.press(Key::J, Modifiers::NONE);
    assert_eq!(page.keys("crop"), ["C", "R"]);
    // A click on the waiting cap itself keeps it waiting.
    page.click("Crop & Straighten: R");
    page.click("Crop & Straighten: R");
    assert!(page.e.preferences.shortcuts.capturing());
    page.press(Key::J, Modifiers::NONE);
    assert_eq!(page.keys("crop"), ["C", "J"]);
}
