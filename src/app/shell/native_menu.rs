//! Routes native menu choices through the same editor operations as the UI.
use crate::app::shell::keymap::{Chord, Effective, Mods, conflicts};
use crate::app::{
    Editor, Module,
    shell::dialogs::{CatalogDialog, FolderAction},
    shell::panels::{PanelChange, WorkspacePanel},
};
use crate::platform::menu::{Action, KeyEquivalent};
use eframe::egui;

/// The key map's command a menu item runs, whose keys it shows. The Edit
/// menu's text keys, Close Window and the items that are no key map command
/// keep the keys `platform::menu` gives them.
pub(in crate::app) fn command_of(action: Action) -> Option<&'static str> {
    Some(match action {
        _ if action.fixed_key() => return None,
        Action::Preferences => "preferences",
        Action::Export => "export_dialog",
        Action::Library => "library",
        Action::Develop => "develop",
        Action::Sidebar => "panels:left",
        Action::Inspector => "panels:right",
        Action::Filmstrip => "panels:filmstrip",
        Action::Shortcuts => "shortcuts",
        _ => return None,
    })
}

/// Every menu item whose key follows the key map.
pub(in crate::app) const KEYED: [Action; 8] = [
    Action::Preferences,
    Action::Export,
    Action::Library,
    Action::Develop,
    Action::Sidebar,
    Action::Inspector,
    Action::Filmstrip,
    Action::Shortcuts,
];

/// The chord `action`'s item shows in `map`, on a Mac (`macos`) or elsewhere:
/// the first chord with Cmd that every binding of its command has, in every
/// context it has one in, and that no other binding shares. A shared chord
/// would let AppKit run the item's command where a tool's binding should win,
/// or pick one of two conflicting commands; it is left to the keys. A plain
/// key would be taken from a text field. The item holds it only while one of
/// those bindings could take it ([`Editor::menu_state`]): Export…'s
/// Cmd+Shift+E is the Library's and Develop's.
pub(in crate::app) fn menu_chord(map: &Effective, action: Action, macos: bool) -> Option<Chord> {
    let id = command_of(action)?;
    let bindings = map.bindings_of(id);
    let chords: Vec<Vec<(crate::app::shell::keymap::Default, Chord)>> = bindings
        .iter()
        .map(|b| {
            map.defaults_of(b)
                .iter()
                .filter(|d| d.exists_on(macos))
                .flat_map(|d| d.chords().map(move |c| (*d, c)))
                .collect()
        })
        .collect();
    chords
        .first()?
        .iter()
        .map(|(_, c)| *c)
        .filter(|c| c.mods.contains(Mods::CMD))
        .find(|c| {
            bindings.iter().zip(&chords).all(|(binding, own)| {
                own.iter()
                    .any(|(d, o)| o == c && !conflicts::shared(map, binding, d, *c, macos))
            })
        })
}

/// Whether `action`'s item has a key in `map` that isn't the one it had
/// before its keys followed the key map
/// ([`Action::native_key_equivalent`]).
fn reassigned(map: &Effective, action: Action) -> bool {
    menu_chord(map, action, true)
        .and_then(Chord::key_equivalent)
        .is_some_and(|k| Some(k) != action.native_key_equivalent())
}

impl Editor {
    /// Each keyed menu item's key equivalent in the key map in effect.
    pub(in crate::app) fn menu_keys(&self) -> Vec<(Action, Option<KeyEquivalent>)> {
        let map = self.keymap.effective();
        KEYED
            .iter()
            .map(|a| {
                (
                    *a,
                    menu_chord(&map, *a, true).and_then(Chord::key_equivalent),
                )
            })
            .collect()
    }
    /// The chords AppKit takes for an item before egui sees them: those of
    /// the installed menu's items that are enabled now. A disabled item leaves
    /// its key to the keys: with the reference card open, Shortcuts is
    /// disabled, so Cmd+/ reaches the card and closes it.
    pub(in crate::app) fn menu_chords(&self, ctx: &egui::Context, installed: bool) -> Vec<Chord> {
        if !installed {
            return Vec::new();
        }
        let map = self.keymap.effective();
        KEYED
            .iter()
            .filter(|a| self.menu_state(**a, ctx).0)
            .filter_map(|a| menu_chord(&map, *a, cfg!(target_os = "macos")))
            .collect()
    }

    pub(in crate::app) fn menu_state(&self, action: Action, ctx: &egui::Context) -> (bool, bool) {
        let free = !self.activity.is_busy() && !self.workspace_modal() && !self.close_confirm;
        let text = ctx.text_edit_focused();
        let library = self.library.is_some();
        let selected = self.library.as_ref().and_then(|l| l.selected()).is_some();
        let workspace = free && library && !self.onboarding.visible;
        let enabled = match action {
            Action::Close | Action::Fullscreen => true,
            // Like the sidebar's button, while a background task runs too.
            Action::ReportIssue => !self.workspace_modal() && !self.close_confirm,
            Action::Cut | Action::Copy | Action::Paste => text && !self.activity.is_busy(),
            Action::Undo | Action::Redo if text => !self.activity.is_busy(),
            Action::SelectAll if text => !self.activity.is_busy(),
            Action::Undo => free && self.undo_log.can_undo(),
            Action::Redo => free && self.undo_log.can_redo(),
            Action::Preferences | Action::NewCatalog | Action::OpenCatalog | Action::Shortcuts => {
                free
            }
            Action::AddFolder => free && library,
            Action::Export => workspace && selected,
            Action::Develop => workspace && selected,
            Action::SelectAll => workspace,
            // As the thumbnail menu's: in the Library, where stacks show.
            Action::AutoStack => {
                workspace
                    && self.module == Module::Library
                    && self.library.as_ref().is_some_and(|l| l.shows_stacks())
            }
            Action::Library | Action::Sidebar | Action::Inspector | Action::Filmstrip => workspace,
        };
        // An item with a key runs its command where AppKit sees the key,
        // before any guard of the command's bindings. An item whose key is
        // the one it always had keeps the availability it always had: Export…
        // stays on while typing, as Cmd+Shift+E reached it through AppKit. An
        // item the user gave another key waits whenever none of its
        // command's bindings could take that key (a text field, a menu or a
        // focused control has it, or its context isn't the one in front).
        // [`Self::menu_chords`] holds the key from the bindings by the same
        // answer, so a press is run once.
        let keyed = command_of(action).filter(|_| reassigned(&self.keymap.effective(), action));
        let enabled = enabled
            && keyed.is_none_or(|id| {
                let live = self.live_keys(ctx);
                self.keymap
                    .effective()
                    .bindings_of(id)
                    .iter()
                    .any(|b| live.runs(b))
            });
        let checked = match action {
            Action::Library => self.module == Module::Library && !self.onboarding.visible,
            Action::Develop => self.module == Module::Develop && !self.onboarding.visible,
            Action::Sidebar => self.panel_shown(WorkspacePanel::Left),
            Action::Inspector => self.panel_shown(WorkspacePanel::Right),
            Action::Filmstrip => self.panel_shown(WorkspacePanel::Filmstrip),
            Action::Fullscreen => ctx.input(|i| i.viewport().fullscreen.unwrap_or(false)),
            _ => false,
        };
        (enabled, checked)
    }

    pub(in crate::app) fn menu_action(&mut self, action: Action, ctx: &egui::Context) {
        // Revalidate when dispatched: a queued click may precede a modal or job.
        if !self.menu_state(action, ctx).0 {
            return;
        }
        if ctx.text_edit_focused()
            && matches!(action, Action::Undo | Action::Redo | Action::SelectAll)
        {
            let key = if action == Action::SelectAll {
                egui::Key::A
            } else {
                egui::Key::Z
            };
            ctx.input_mut(|i| {
                i.events.push(egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers {
                        shift: action == Action::Redo,
                        ..egui::Modifiers::COMMAND
                    },
                })
            });
            return;
        }
        match action {
            Action::Preferences => self.open_preferences(crate::app::preferences::Tab::General),
            Action::NewCatalog => self.catalog_dialog(CatalogDialog::Create, ctx),
            Action::OpenCatalog => self.catalog_dialog(CatalogDialog::Open, ctx),
            Action::AddFolder => self.catalog_dialog(CatalogDialog::Folder(FolderAction::Add), ctx),
            Action::Export => self.open_export_dialog(),
            Action::Close => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Action::Undo => self.undo(),
            Action::Redo => self.redo(),
            Action::Cut => ctx.send_viewport_cmd(egui::ViewportCommand::RequestCut),
            Action::Copy => ctx.send_viewport_cmd(egui::ViewportCommand::RequestCopy),
            Action::Paste => ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste),
            Action::SelectAll => {
                if let Some(library) = &mut self.library {
                    if self.module == Module::Develop {
                        library.develop_select_all(self.document.catalog_photo);
                    } else {
                        library.select_all();
                    }
                }
            }
            Action::Library => {
                if self.flush() {
                    self.module = Module::Library;
                    if let Some(library) = &mut self.library {
                        library.show_grid();
                    }
                }
            }
            Action::AutoStack => {
                if let Some(library) = &mut self.library {
                    library.stack_action(crate::app::library::StackAction::AutoStack, Vec::new());
                }
            }
            Action::Develop => {
                if let Some(id) = self.library.as_ref().and_then(|l| l.selected()) {
                    self.develop_catalog_photo(id);
                }
            }
            Action::Sidebar | Action::Inspector | Action::Filmstrip => {
                let panel = match action {
                    Action::Sidebar => WorkspacePanel::Left,
                    Action::Inspector => WorkspacePanel::Right,
                    _ => WorkspacePanel::Filmstrip,
                };
                self.request_panels(PanelChange::Toggle(panel));
            }
            Action::Fullscreen => ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(
                !self.menu_state(action, ctx).1,
            )),
            Action::Shortcuts => self.view.shortcuts = true,
            Action::ReportIssue => self.open_report_issue(),
        }
    }

    #[cfg(target_os = "macos")]
    pub(in crate::app) fn native_menu_actions(&mut self, ctx: &egui::Context) {
        if !self.native_menu {
            return;
        }
        let actions = crate::platform::menu::take_actions();
        for action in actions {
            self.menu_action(action, ctx);
        }
    }

    #[cfg(target_os = "macos")]
    pub(in crate::app) fn update_native_menu(&mut self, ctx: &egui::Context) {
        if self.native_menu {
            self.sync_undo();
            crate::platform::menu::set_key_equivalents(&self.menu_keys());
            crate::platform::menu::update(|action| self.menu_state(action, ctx));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::testing::editor_with_catalog;
    use eframe::egui::Key;

    fn chord(text: &str) -> Chord {
        Chord::parse(text).expect("a chord")
    }
    fn user(entries: &[(&'static str, &[&str])]) -> Effective {
        Effective::new(
            entries
                .iter()
                .map(|(id, keys)| (*id, keys.iter().map(|k| chord(k)).collect()))
                .collect(),
        )
    }
    fn equivalent(text: &str) -> KeyEquivalent {
        chord(text).key_equivalent().expect("a key equivalent")
    }

    #[test]
    fn a_chord_becomes_appkits_character_and_modifiers() {
        let e = equivalent("Cmd+Shift+E");
        assert_eq!(e.key, "e");
        assert!(e.command && e.shift && !e.option && !e.control);
        assert_eq!(equivalent("Cmd+,").key, ",");
        assert_eq!(equivalent("Cmd+/").key, "/");
        assert_eq!(equivalent("Cmd+-").key, "-");
        assert_eq!(equivalent("Cmd+1").key, "1");
        assert_eq!(equivalent("Cmd+Up").key, "\u{F700}");
        assert_eq!(equivalent("Cmd+Delete").key, "\u{F728}");
        let f9 = equivalent("Cmd+Alt+F9");
        assert_eq!(f9.key, "\u{F70C}");
        assert!(f9.command && f9.option && !f9.shift);
        let ctrl = Chord::new(Key::K, Mods::CMD.with(Mods::CTRL))
            .key_equivalent()
            .unwrap();
        assert!(ctrl.command && ctrl.control);
    }

    #[test]
    fn the_menu_shows_the_defaults_cmd_chords_that_work_everywhere() {
        let defaults = Effective::default();
        for macos in [true, false] {
            let shown = |a| menu_chord(&defaults, a, macos).map(|c| c.to_string());
            assert_eq!(shown(Action::Preferences).as_deref(), Some("Cmd+,"));
            assert_eq!(shown(Action::Shortcuts).as_deref(), Some("Cmd+/"));
            // Plain keys would be taken from a text field.
            for a in [
                Action::Library,
                Action::Develop,
                Action::Sidebar,
                Action::Inspector,
                Action::Filmstrip,
            ] {
                assert_eq!(shown(a), None, "{a:?}");
            }
            // Export's keys are the Library's and Develop's, both Cmd+Shift+E.
            assert_eq!(shown(Action::Export).as_deref(), Some("Cmd+Shift+E"));
            // The Edit menu's and the items that are no command keep theirs.
            for a in [
                Action::Undo,
                Action::Copy,
                Action::SelectAll,
                Action::OpenCatalog,
            ] {
                assert_eq!(command_of(a), None, "{a:?}");
            }
        }
    }

    #[test]
    fn the_first_cmd_chord_no_other_binding_shares_is_shown() {
        let map = user(&[("preferences", &["F9", "Cmd+P", "Cmd+Shift+P"])]);
        assert_eq!(
            menu_chord(&map, Action::Preferences, true),
            Some(chord("Cmd+P"))
        );
        // A user's key for the grid view, everywhere.
        let map = user(&[("library", &["G", "Cmd+F1"])]);
        assert_eq!(
            menu_chord(&map, Action::Library, true),
            Some(chord("Cmd+F1"))
        );
        // Shared with another command everywhere: a conflict.
        let map = user(&[("shortcuts", &["Cmd+K"]), ("virtual_copy", &["Cmd+K"])]);
        assert_eq!(menu_chord(&map, Action::Shortcuts, true), None);
        // Shadowed: Cmd+G groups a stack in the grid.
        let map = user(&[("preferences", &["Cmd+G", "Cmd+F2"])]);
        assert_eq!(
            menu_chord(&map, Action::Preferences, true),
            Some(chord("Cmd+F2"))
        );
        let map = user(&[("preferences", &["Cmd+G"])]);
        assert_eq!(menu_chord(&map, Action::Preferences, true), None);
    }

    #[test]
    fn the_menu_holds_its_keys_only_while_installed_and_enabled() -> anyhow::Result<()> {
        let (_dir, mut editor, _) = editor_with_catalog(&["one.ARW"])?;
        let ctx = editor.context.clone();
        editor.onboarding.visible = false;
        assert!(editor.menu_chords(&ctx, false).is_empty(), "not installed");
        let held = editor.menu_chords(&ctx, true);
        assert!(held.contains(&chord("Cmd+/")) && held.contains(&chord("Cmd+,")));
        // With the card open, Shortcuts is disabled: Cmd+/ reaches the card.
        editor.view.shortcuts = true;
        let held = editor.menu_chords(&ctx, true);
        assert!(!held.contains(&chord("Cmd+/")));
        let cmd = egui::Modifiers::COMMAND;
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: Key::Slash,
                    physical_key: Some(Key::Slash),
                    pressed: true,
                    repeat: false,
                    modifiers: cmd,
                }],
                ..Default::default()
            },
            |ui| {
                let keys = crate::app::shell::keymap::FrameKeys::begin(
                    ui.ctx(),
                    editor.keymap.effective(),
                );
                keys.set_menu_chords(held.clone());
                editor.shortcuts_window(ui.ctx());
            },
        );
        output.textures_delta.clear();
        assert!(!editor.view.shortcuts, "Cmd+/ closed the card");
        Ok(())
    }

    #[test]
    fn the_menus_keys_follow_the_key_map() -> anyhow::Result<()> {
        let (_dir, mut editor, _) = editor_with_catalog(&[])?;
        let key = |editor: &Editor, action| {
            editor
                .menu_keys()
                .into_iter()
                .find(|(a, _)| *a == action)
                .and_then(|(_, k)| k)
                .map(|k| k.key)
        };
        assert_eq!(key(&editor, Action::Preferences).as_deref(), Some(","));
        assert_eq!(key(&editor, Action::Export).as_deref(), Some("e"));
        editor
            .keymap
            .set("preferences", &[chord("Cmd+Alt+P")])
            .expect("bound");
        assert_eq!(key(&editor, Action::Preferences).as_deref(), Some("p"));
        editor.keymap.set("preferences", &[]).expect("unbound");
        assert_eq!(key(&editor, Action::Preferences), None);
        assert!(
            editor.menu_keys().iter().all(|(a, _)| !a.fixed_key()),
            "the Edit menu keeps its keys"
        );
        Ok(())
    }

    #[test]
    fn a_chord_the_menu_holds_reaches_no_binding() {
        use crate::app::shell::commands::registry::bindings;
        let ctx = egui::Context::default();
        let press = |held: Vec<Chord>| {
            let mut taken = None;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events: vec![
                        egui::Event::Key {
                            key: Key::Comma,
                            physical_key: Some(Key::Comma),
                            pressed: true,
                            repeat: false,
                            modifiers: egui::Modifiers::COMMAND,
                        },
                        egui::Event::Key {
                            key: Key::Comma,
                            physical_key: Some(Key::Comma),
                            pressed: false,
                            repeat: false,
                            modifiers: egui::Modifiers::COMMAND,
                        },
                    ],
                    ..Default::default()
                },
                |ui| {
                    let keys =
                        crate::app::shell::keymap::FrameKeys::begin(ui.ctx(), Default::default());
                    keys.set_menu_chords(held.clone());
                    taken = keys.take(&bindings::PREFERENCES);
                },
            );
            output.textures_delta.clear();
            taken
        };
        assert!(press(Vec::new()).is_some());
        assert!(press(vec![chord("Cmd+,")]).is_none(), "AppKit ran it");
        assert!(press(vec![chord("Cmd+/")]).is_some());
    }

    #[test]
    fn an_item_whose_key_the_command_refuses_while_typing_waits_while_typing() -> anyhow::Result<()>
    {
        let (_dir, mut editor, _) = editor_with_catalog(&["one.ARW"])?;
        let ctx = editor.context.clone();
        editor.onboarding.visible = false;
        editor
            .keymap
            .set("library", &[chord("Cmd+F1")])
            .expect("bound");
        assert!(editor.menu_state(Action::Library, &ctx).0);
        let mut text = String::new();
        for _ in 0..2 {
            let mut output = ctx.run_ui(Default::default(), |ui| {
                ui.text_edit_singleline(&mut text).request_focus();
            });
            output.textures_delta.clear();
        }
        assert!(ctx.text_edit_focused());
        assert!(
            !editor.menu_state(Action::Library, &ctx).0,
            "Cmd+F1 would type-skip"
        );
        // Preferences runs while typing, as its key does.
        assert!(editor.menu_state(Action::Preferences, &ctx).0);
        // With no key of its own, an item stays clickable.
        editor.keymap.reset("library").expect("reset");
        assert!(editor.menu_state(Action::Library, &ctx).0);
        Ok(())
    }

    #[test]
    fn an_item_on_its_own_key_keeps_its_old_availability_while_typing() -> anyhow::Result<()> {
        use crate::app::shell::commands::registry::bindings;
        let (_dir, mut editor, ids) = editor_with_catalog(&["one.ARW"])?;
        let ctx = editor.context.clone();
        editor.onboarding.visible = false;
        editor.library.as_mut().unwrap().select(Some(ids[0]));
        let mut text = String::new();
        let mut typing = || {
            for _ in 0..2 {
                let mut output = ctx.run_ui(Default::default(), |ui| {
                    ui.text_edit_singleline(&mut text).request_focus();
                });
                output.textures_delta.clear();
            }
            assert!(ctx.text_edit_focused());
        };
        // What the menu and the bindings do with a press of `keys`: whether the
        // item is enabled, the menu holds the key and the Library's binding
        // runs it.
        let press = |editor: &Editor, keys: &str| {
            let enabled = editor.menu_state(Action::Export, &ctx).0;
            let held = editor.menu_chords(&ctx, true);
            let c = chord(keys);
            let mut modifiers = egui::Modifiers::COMMAND;
            modifiers.shift = c.mods.contains(Mods::SHIFT);
            modifiers.alt = c.mods.contains(Mods::ALT);
            let mut taken = None;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key: c.key,
                        physical_key: Some(c.key),
                        pressed: true,
                        repeat: false,
                        modifiers,
                    }],
                    ..Default::default()
                },
                |ui| {
                    let keys = crate::app::shell::keymap::FrameKeys::begin(
                        ui.ctx(),
                        editor.keymap.effective(),
                    );
                    keys.set_live(editor.live_keys(ui.ctx()));
                    keys.set_menu_chords(held.clone());
                    // As the Library's handler asks: only while its guards let it.
                    let export = &bindings::LIBRARY_EXPORT;
                    taken = editor
                        .live_keys(ui.ctx())
                        .runs(export)
                        .then(|| keys.take(export))
                        .flatten();
                },
            );
            output.textures_delta.clear();
            (enabled, held.contains(&c), taken.is_some())
        };
        // The defaults: Cmd+Shift+E is the item's key on main, and AppKit
        // runs Export… with it while typing, as a click does.
        typing();
        assert_eq!(press(&editor, "Cmd+Shift+E"), (true, true, false));
        // A key the user gave it waits while typing, as its bindings do, and
        // reaches neither.
        editor
            .keymap
            .set("export_dialog", &[chord("Cmd+F9")])
            .expect("bound");
        typing();
        assert_eq!(press(&editor, "Cmd+F9"), (false, false, false));
        // Off the text field, which the last frame didn't show, the menu
        // holds it and runs it, the bindings don't.
        assert!(!ctx.text_edit_focused());
        assert_eq!(press(&editor, "Cmd+F9"), (true, true, false));
        // Every keyed item, in each state: enabled exactly when its key is held.
        for typed in [false, true] {
            if typed {
                typing();
            }
            for action in KEYED {
                let Some(c) = menu_chord(&editor.keymap.effective(), action, true) else {
                    continue;
                };
                assert_eq!(
                    editor.menu_state(action, &ctx).0,
                    editor.menu_chords(&ctx, true).contains(&c),
                    "{action:?} typing {typed}"
                );
            }
        }
        Ok(())
    }

    /// With the defaults, every item has the key it had before the key map
    /// (`platform/menu/macos.rs`'s table at 03d50b81).
    #[test]
    fn with_the_defaults_every_item_keeps_its_key() -> anyhow::Result<()> {
        let (_dir, editor, _) = editor_with_catalog(&[])?;
        let keyed = editor.menu_keys();
        let shown = |action: Action| {
            if action.fixed_key() {
                action.fixed_key_equivalent()
            } else {
                keyed
                    .iter()
                    .find(|(a, _)| *a == action)
                    .and_then(|(_, k)| k.clone())
            }
            .map(|k| {
                assert!(k.command && !k.option && !k.control, "{action:?}");
                (k.key, k.shift)
            })
        };
        let main: &[(Action, &str, bool)] = &[
            (Action::Preferences, ",", false),
            (Action::OpenCatalog, "o", false),
            (Action::AddFolder, "o", true),
            (Action::Export, "e", true),
            (Action::Close, "w", false),
            (Action::Undo, "z", false),
            (Action::Redo, "z", true),
            (Action::Cut, "x", false),
            (Action::Copy, "c", false),
            (Action::Paste, "v", false),
            (Action::SelectAll, "a", false),
            (Action::Shortcuts, "/", false),
        ];
        for action in Action::ALL {
            let expected = main
                .iter()
                .find(|(a, _, _)| *a == action)
                .map(|(_, key, shift)| (key.to_string(), *shift));
            assert_eq!(shown(action), expected, "{action:?}");
        }
        Ok(())
    }

    #[test]
    fn a_menu_items_fixed_key_is_no_commands() -> anyhow::Result<()> {
        use crate::app::shell::keymap::rules::{Os, Refusal, refusal};
        let library = crate::app::shell::commands::registry::command_by_id("library").unwrap();
        let bindings = Effective::default().bindings_of("library");
        for keys in ["Cmd+O", "Cmd+Shift+O", "Cmd+W"] {
            assert_eq!(
                refusal(library, &bindings, chord(keys), Os::Macos),
                Some(Refusal::Reserved),
                "{keys}"
            );
            assert_eq!(refusal(library, &bindings, chord(keys), Os::Other), None);
        }
        Ok(())
    }

    #[test]
    fn an_item_with_a_key_waits_while_its_commands_bindings_would() -> anyhow::Result<()> {
        let (_dir, mut editor, _) = editor_with_catalog(&["one.ARW"])?;
        let ctx = editor.context.clone();
        editor.onboarding.visible = false;
        editor
            .keymap
            .set("panels:left", &[chord("Cmd+F9")])
            .expect("bound");
        assert!(editor.menu_state(Action::Sidebar, &ctx).0);
        // A focused button: the panel keys leave Tab and the rest to it.
        for _ in 0..2 {
            let mut output = ctx.run_ui(Default::default(), |ui| {
                ui.button("focused").request_focus();
            });
            output.textures_delta.clear();
        }
        assert!(
            !editor.menu_state(Action::Sidebar, &ctx).0,
            "a focused control"
        );
        ctx.memory_mut(|m| m.surrender_focus(m.focused().expect("focused")));
        assert!(editor.menu_state(Action::Sidebar, &ctx).0);
        // A menu open over the workspace.
        egui::Popup::open_id(&ctx, egui::Id::new("a menu"));
        assert!(egui::Popup::is_any_open(&ctx));
        assert!(!editor.menu_state(Action::Sidebar, &ctx).0, "a menu open");
        Ok(())
    }

    #[test]
    fn menu_commands_recheck_busy_and_modal_state() -> anyhow::Result<()> {
        let (_dir, mut editor, _) = editor_with_catalog(&["one.ARW"])?;
        let ctx = editor.context.clone();
        editor.onboarding.visible = false;
        assert!(editor.activity.begin_folder_change());
        for action in [
            Action::NewCatalog,
            Action::OpenCatalog,
            Action::AddFolder,
            Action::Export,
            Action::Undo,
            Action::Redo,
            Action::Cut,
            Action::Copy,
            Action::Paste,
            Action::SelectAll,
            Action::AutoStack,
            Action::Preferences,
            Action::Library,
            Action::Develop,
            Action::Sidebar,
            Action::Inspector,
            Action::Filmstrip,
            Action::Shortcuts,
        ] {
            assert!(!editor.menu_state(action, &ctx).0, "{action:?}");
            editor.menu_action(action, &ctx);
        }
        assert!(editor.menu_state(Action::Fullscreen, &ctx).0);
        // Report an Issue stays available while a task runs, as the sidebar's
        // button does, and then makes the dialog modal.
        assert!(editor.menu_state(Action::ReportIssue, &ctx).0);
        editor.report_issue_test_probe();
        editor.menu_action(Action::ReportIssue, &ctx);
        assert!(editor.report_issue.is_open());
        assert!(!editor.menu_state(Action::ReportIssue, &ctx).0);
        assert!(!editor.menu_state(Action::Shortcuts, &ctx).0);
        editor.close_report_issue();
        assert!(!editor.preferences.open);
        assert!(!editor.view.shortcuts);
        assert!(editor.activity.is_changing_folder());
        editor.activity.finish_dialog();
        editor.view.shortcuts = true;
        assert!(!editor.menu_state(Action::OpenCatalog, &ctx).0);
        assert!(
            editor.menu_state(Action::Close, &ctx).0,
            "close still uses the save guard"
        );
        Ok(())
    }

    #[test]
    fn menu_undo_and_redo_use_the_shared_history() -> anyhow::Result<()> {
        let (_dir, mut editor, ids) = editor_with_catalog(&["one.ARW"])?;
        let ctx = editor.context.clone();
        editor.onboarding.visible = false;
        editor.library.as_mut().unwrap().select(Some(ids[0]));
        editor
            .command_metadata(
                crate::app::library::photo_metadata::Edit::Rating(4),
                None,
                false,
            )
            .unwrap();
        editor.menu_action(Action::Undo, &ctx);
        assert_eq!(
            editor
                .library
                .as_ref()
                .unwrap()
                .photo(ids[0])
                .unwrap()
                .rating,
            0
        );
        editor.menu_action(Action::Redo, &ctx);
        assert_eq!(
            editor
                .library
                .as_ref()
                .unwrap()
                .photo(ids[0])
                .unwrap()
                .rating,
            4
        );
        Ok(())
    }

    #[test]
    fn menu_library_cannot_leave_a_draft_that_failed_to_save() -> anyhow::Result<()> {
        let (_dir, mut editor, ids) = editor_with_catalog(&["one.ARW"])?;
        editor.onboarding.visible = false;
        let library = editor.library.as_mut().unwrap();
        let copy = library.create_virtual_copy(ids[0])?.value;
        library
            .session
            .catalog_mut_for_tests()
            .remove_virtual_copy(copy)?;
        library.set_copy_name_draft(copy, "Unsaved name");
        editor.module = Module::Develop;
        let ctx = editor.context.clone();
        editor.menu_action(Action::Library, &ctx);
        assert!(editor.module == Module::Develop);
        assert!(!editor.flush());
        editor.library.as_mut().unwrap().discard_drafts();
        Ok(())
    }

    #[test]
    fn menu_auto_stack_opens_the_window_in_the_library_where_stacks_show() -> anyhow::Result<()> {
        let (_dir, mut editor, _) = editor_with_catalog(&["one.ARW", "two.ARW"])?;
        let ctx = editor.context.clone();
        editor.onboarding.visible = false;
        editor.module = Module::Develop;
        assert!(!editor.menu_state(Action::AutoStack, &ctx).0);
        editor.menu_action(Action::AutoStack, &ctx);
        assert!(!editor.auto_stack_open());
        editor.module = Module::Library;
        assert!(editor.menu_state(Action::AutoStack, &ctx).0);
        editor.menu_action(Action::AutoStack, &ctx);
        assert!(editor.auto_stack_open());
        // Open, the window is modal: the menu waits for it.
        assert!(!editor.menu_state(Action::AutoStack, &ctx).0);
        Ok(())
    }

    #[test]
    fn menu_select_all_in_develop_preserves_the_open_photo() -> anyhow::Result<()> {
        let (_dir, mut editor, ids) = editor_with_catalog(&["one.ARW", "two.ARW"])?;
        editor.onboarding.visible = false;
        editor.module = Module::Develop;
        editor.document.catalog_photo = Some(ids[1]);
        editor.library.as_mut().unwrap().select(Some(ids[1]));
        let ctx = editor.context.clone();
        editor.menu_action(Action::SelectAll, &ctx);
        let library = editor.library.as_ref().unwrap();
        assert_eq!(library.selected_photos().len(), 2);
        assert_eq!(library.selected(), Some(ids[1]));
        Ok(())
    }

    #[test]
    fn menu_close_requests_the_normal_window_close() -> anyhow::Result<()> {
        let (_dir, mut editor, _) = editor_with_catalog(&[])?;
        let ctx = editor.context.clone();
        let mut output = ctx.run_ui(Default::default(), |ui| {
            editor.menu_action(Action::Close, ui.ctx())
        });
        output.textures_delta.clear();
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .any(|c| matches!(c, egui::ViewportCommand::Close))
        );
        Ok(())
    }
}
