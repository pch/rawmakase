//! Native menu transport. The editor owns command availability and execution;
//! AppKit only sends typed choices and wakes the event loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Preferences,
    NewCatalog,
    OpenCatalog,
    AddFolder,
    Export,
    Close,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    /// Photo › Stacking › Auto-Stack by Capture Time…
    AutoStack,
    Library,
    Develop,
    Sidebar,
    Inspector,
    Filmstrip,
    Fullscreen,
    Shortcuts,
    ReportIssue,
}

/// A menu item's key equivalent, as AppKit takes it: the character it
/// matches and the modifiers held with it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct KeyEquivalent {
    /// A character, lower case for a letter, or one of AppKit's function-key
    /// characters (F1, the arrows, Forward Delete…).
    pub key: String,
    pub command: bool,
    pub shift: bool,
    pub option: bool,
    pub control: bool,
}

impl Action {
    /// Every item, in the menus' order.
    pub(crate) const ALL: [Self; 21] = [
        Self::Preferences,
        Self::NewCatalog,
        Self::OpenCatalog,
        Self::AddFolder,
        Self::Export,
        Self::Close,
        Self::Undo,
        Self::Redo,
        Self::Cut,
        Self::Copy,
        Self::Paste,
        Self::SelectAll,
        Self::AutoStack,
        Self::Library,
        Self::Develop,
        Self::Sidebar,
        Self::Inspector,
        Self::Filmstrip,
        Self::Fullscreen,
        Self::Shortcuts,
        Self::ReportIssue,
    ];

    /// The key equivalent of an item whose key never changes
    /// ([`Action::fixed_key`]), if it has one. No command of the key map may
    /// have these, on any platform's check, so a Mac never has two items on
    /// one key.
    pub(crate) fn fixed_key_equivalent(self) -> Option<KeyEquivalent> {
        let (key, shift) = match self {
            Self::OpenCatalog => ("o", false),
            Self::AddFolder => ("o", true),
            Self::Close => ("w", false),
            Self::Undo => ("z", false),
            Self::Redo => ("z", true),
            Self::Cut => ("x", false),
            Self::Copy => ("c", false),
            Self::Paste => ("v", false),
            Self::SelectAll => ("a", false),
            _ => return None,
        };
        Some(KeyEquivalent {
            key: key.into(),
            command: true,
            shift,
            option: false,
            control: false,
        })
    }

    /// Whether the item's key equivalent never changes: the Edit menu's text
    /// keys and Close Window, which the system owns, and the items that are
    /// no command of the key map.
    #[cfg_attr(
        not(any(target_os = "macos", test)),
        allow(dead_code, reason = "the macOS menu's alone")
    )]
    pub(crate) fn fixed_key(self) -> bool {
        matches!(
            self,
            Self::Undo
                | Self::Redo
                | Self::Cut
                | Self::Copy
                | Self::Paste
                | Self::SelectAll
                | Self::Close
                | Self::NewCatalog
                | Self::OpenCatalog
                | Self::AddFolder
                | Self::AutoStack
                | Self::Fullscreen
                | Self::ReportIssue
        )
    }
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub(crate) use macos::{clear, install, set_key_equivalents, take_actions, update};
