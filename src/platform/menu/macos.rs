use super::{Action, KeyEquivalent};
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained, sel,
};
use objc2_app_kit::{
    NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem, NSMenuItemValidation,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString, ns_string};
use std::sync::mpsc::{self, Receiver, Sender};

struct TargetState {
    send: Sender<Action>,
    wake: Box<dyn Fn()>,
    actions: Vec<Action>,
}

define_class!(
    // SAFETY: NSObject has no subclass requirements; all access is on the main thread.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = TargetState]
    struct MenuTarget;

    unsafe impl NSObjectProtocol for MenuTarget {}
    // SAFETY: validation only reports the state last supplied by the editor.
    unsafe impl NSMenuItemValidation for MenuTarget {
        #[unsafe(method(validateMenuItem:))]
        fn validate(&self, item: &NSMenuItem) -> bool { item.isEnabled() }
    }

    impl MenuTarget {
        #[unsafe(method(choose:))]
        fn choose(&self, item: &NSMenuItem) {
            if let Ok(index) = usize::try_from(item.tag())
                && let Some(action) = self.ivars().actions.get(index)
                && self.ivars().send.send(*action).is_ok()
            {
                (self.ivars().wake)();
            }
        }
    }
);

/// Kept on the UI thread for as long as native menu items can send actions.
/// No worker or shutdown wait: callbacks queue a choice on the main thread.
struct Menu {
    target: Retained<MenuTarget>,
    items: Vec<(Action, Retained<NSMenuItem>)>,
    receive: Receiver<Action>,
    /// The key equivalents last set on the items that follow the key map.
    keys: Vec<(Action, Option<KeyEquivalent>)>,
}

impl Menu {
    fn install(wake: impl Fn() + 'static) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let app = NSApplication::sharedApplication(mtm);
        let bar = app.mainMenu()?;
        let application = bar.itemAtIndex(0)?.submenu()?;
        let (send, receive) = mpsc::channel();
        type Entry = (Action, &'static str, &'static str, bool);
        // A menu's entries, or a submenu's when it has a title, as Photo's
        // Stacking.
        let definitions: &[(&str, Option<&str>, &[Entry])] = &[
            (
                "File",
                None,
                &[
                    (Action::NewCatalog, "New Catalog…", "", false),
                    (Action::OpenCatalog, "Open Catalog…", "o", false),
                    (Action::AddFolder, "Add Folder…", "o", true),
                    (Action::Export, "Export…", "", false),
                    (Action::Close, "Close Window", "w", false),
                ],
            ),
            (
                "Edit",
                None,
                &[
                    (Action::Undo, "Undo", "z", false),
                    (Action::Redo, "Redo", "z", true),
                    (Action::Cut, "Cut", "x", false),
                    (Action::Copy, "Copy", "c", false),
                    (Action::Paste, "Paste", "v", false),
                    (Action::SelectAll, "Select All", "a", false),
                ],
            ),
            (
                "Photo",
                Some("Stacking"),
                &[(Action::AutoStack, "Auto-Stack by Capture Time…", "", false)],
            ),
            (
                "View",
                None,
                &[
                    (Action::Library, "Library", "", false),
                    (Action::Develop, "Develop", "", false),
                    (Action::Sidebar, "Show Sidebar", "", false),
                    (Action::Inspector, "Show Inspector", "", false),
                    (Action::Filmstrip, "Show Filmstrip", "", false),
                    (Action::Fullscreen, "Enter Full Screen", "", false),
                ],
            ),
            (
                "Help",
                None,
                &[
                    (Action::Shortcuts, "Keyboard Shortcuts", "", false),
                    (Action::ReportIssue, "Report an Issue…", "", false),
                ],
            ),
        ];
        let actions = std::iter::once(Action::Preferences)
            .chain(
                definitions
                    .iter()
                    .flat_map(|(_, _, entries)| entries.iter().map(|e| e.0)),
            )
            .collect();
        let allocated = MenuTarget::alloc(mtm).set_ivars(TargetState {
            send,
            wake: Box::new(wake),
            actions,
        });
        // SAFETY: initialize NSObject once, with the ivars already set.
        let target: Retained<MenuTarget> = unsafe { msg_send![super(allocated), init] };
        let mut menu = Self {
            target,
            items: Vec::new(),
            receive,
            keys: Vec::new(),
        };
        // The key map's items get their keys from `set_key_equivalents`.
        let preferences = menu.item(Action::Preferences, "Preferences…", "", false);
        application.insertItem_atIndex(&preferences, 2);
        application.insertItem_atIndex(&NSMenuItem::separatorItem(mtm), 3);
        // The winit application menu and its guarded Quit action stay in place.
        for (title, nested, entries) in definitions {
            if *title == "Help" {
                let window = submenu(&bar, "Window", mtm);
                for (label, action, key) in [
                    ("Minimize", sel!(performMiniaturize:), "m"),
                    ("Zoom", sel!(performZoom:), ""),
                    ("Bring All to Front", sel!(arrangeInFront:), ""),
                ] {
                    // SAFETY: standard AppKit responder-chain selectors, all taking a sender.
                    let item = unsafe {
                        NSMenuItem::initWithTitle_action_keyEquivalent(
                            mtm.alloc(),
                            &NSString::from_str(label),
                            Some(action),
                            &NSString::from_str(key),
                        )
                    };
                    window.addItem(&item);
                }
                app.setWindowsMenu(Some(&window));
            }
            let top = submenu(&bar, title, mtm);
            top.setAutoenablesItems(false);
            let sub = match nested {
                Some(nested) => {
                    let sub = submenu(&top, nested, mtm);
                    sub.setAutoenablesItems(false);
                    sub
                }
                None => top.clone(),
            };
            for (action, label, key, shift) in *entries {
                if matches!(
                    action,
                    Action::Cut
                        | Action::Sidebar
                        | Action::Fullscreen
                        | Action::Export
                        | Action::Close
                ) {
                    sub.addItem(&NSMenuItem::separatorItem(mtm));
                }
                sub.addItem(&menu.item(*action, label, key, *shift));
            }
            if *title == "Help" {
                app.setHelpMenu(Some(&top));
            }
        }
        Some(menu)
    }

    fn item(
        &mut self,
        action: Action,
        label: &str,
        key: &str,
        shift: bool,
    ) -> Retained<NSMenuItem> {
        let mtm = self.target.mtm();
        // SAFETY: target implements choose: with NSMenuItem as sender and is retained by Menu.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                &NSString::from_str(label),
                Some(sel!(choose:)),
                &NSString::from_str(key),
            )
        };
        item.setTag(self.items.len() as isize);
        let mut modifiers = NSEventModifierFlags::Command;
        if shift {
            modifiers |= NSEventModifierFlags::Shift;
        }
        item.setKeyEquivalentModifierMask(modifiers);
        // SAFETY: the target lives until Drop clears every item's target and action.
        unsafe {
            item.setTarget(Some(&self.target));
        }
        item.setEnabled(false);
        self.items.push((action, item.clone()));
        item
    }

    /// Sets the key equivalents of the items that follow the key map, where
    /// they changed. None leaves an item with no key.
    fn set_keys(&mut self, keys: &[(Action, Option<KeyEquivalent>)]) {
        if self.keys == keys {
            return;
        }
        for (action, key) in keys {
            if action.fixed_key() {
                continue;
            }
            for (_, item) in self.items.iter().filter(|(a, _)| a == action) {
                let key = key.clone().unwrap_or_default();
                item.setKeyEquivalent(&NSString::from_str(&key.key));
                let mut mask = NSEventModifierFlags::empty();
                for (on, flag) in [
                    (key.command, NSEventModifierFlags::Command),
                    (key.shift, NSEventModifierFlags::Shift),
                    (key.option, NSEventModifierFlags::Option),
                    (key.control, NSEventModifierFlags::Control),
                ] {
                    if on {
                        mask |= flag;
                    }
                }
                item.setKeyEquivalentModifierMask(mask);
            }
        }
        self.keys = keys.to_vec();
    }

    fn take_actions(&self) -> Vec<Action> {
        self.receive.try_iter().collect()
    }

    fn update(&self, state: impl Fn(Action) -> (bool, bool)) {
        for (action, item) in &self.items {
            let (enabled, checked) = state(*action);
            item.setEnabled(enabled);
            item.setState(if checked { 1 } else { 0 });
            if *action == Action::Fullscreen {
                item.setTitle(if checked {
                    ns_string!("Exit Full Screen")
                } else {
                    ns_string!("Enter Full Screen")
                });
            }
        }
    }
}

impl Drop for Menu {
    fn drop(&mut self) {
        for (_, item) in &self.items {
            // SAFETY: disconnect the weak targets before releasing their owner.
            unsafe {
                item.setTarget(None);
                item.setAction(None);
            }
            item.setEnabled(false);
        }
    }
}

fn submenu(bar: &NSMenu, title: &str, mtm: MainThreadMarker) -> Retained<NSMenu> {
    let sub = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str(title));
    let top = NSMenuItem::new(mtm);
    top.setTitle(&NSString::from_str(title));
    top.setSubmenu(Some(&sub));
    bar.addItem(&top);
    sub
}

// AppKit objects stay on the UI thread; Editor remains usable in headless workers.
thread_local! { static MENU: std::cell::RefCell<Option<Menu>> = const { std::cell::RefCell::new(None) }; }

pub(crate) fn install(wake: impl Fn() + 'static) -> bool {
    MENU.with_borrow_mut(|menu| {
        if menu.is_none() {
            *menu = Menu::install(wake);
        }
        menu.is_some()
    })
}
pub(crate) fn take_actions() -> Vec<Action> {
    MENU.with_borrow(|menu| menu.as_ref().map(Menu::take_actions).unwrap_or_default())
}
pub(crate) fn set_key_equivalents(keys: &[(Action, Option<KeyEquivalent>)]) {
    MENU.with_borrow_mut(|menu| {
        if let Some(menu) = menu {
            menu.set_keys(keys);
        }
    });
}
pub(crate) fn update(state: impl Fn(Action) -> (bool, bool)) {
    MENU.with_borrow(|menu| {
        if let Some(menu) = menu {
            menu.update(state);
        }
    });
}
pub(crate) fn clear() {
    MENU.with_borrow_mut(|menu| {
        menu.take();
    });
}
