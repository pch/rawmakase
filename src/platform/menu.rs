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
    /// Whether the item's key equivalent never changes: the Edit menu's text
    /// keys and Close Window, which the system owns, and the items that are
    /// no command of the key map.
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
