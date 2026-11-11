//! The chords in effect: the user's for the commands their key map names, the
//! defaults for the rest, and the commands of the extensions enabled now.
use super::{Binding, Chord, Default};
use crate::app::shell::commands::registry::{self, Command, Dynamic};
use std::{borrow::Cow, collections::BTreeMap};

/// The user's chords, by command. A command the user's key map names runs on
/// those chords alone, in every context it has a binding in; an empty list
/// leaves it with no key. Every other command keeps this release's defaults,
/// so new defaults arrive with an update.
///
/// Beside the built-in commands it holds the enabled extensions' commands
/// ([`Dynamic`]): everything that looks a command or a binding up (the keys,
/// conflicts, the Shortcuts page, the key map file) asks this, so an
/// extension's command is one like any other while its extension is on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::app) struct Effective {
    user: BTreeMap<&'static str, Vec<Chord>>,
    dynamic: Vec<&'static Dynamic>,
}

impl Effective {
    /// `user`'s chords over the defaults. The ids are the registry's
    /// ([`crate::app::shell::commands::registry::COMMANDS`]).
    pub(in crate::app) fn new(user: BTreeMap<&'static str, Vec<Chord>>) -> Self {
        Self {
            user,
            dynamic: Vec::new(),
        }
    }
    /// With the enabled extensions' commands, `dynamic`, whose ids `user`
    /// may name too.
    pub(in crate::app) fn with_dynamic(mut self, dynamic: Vec<&'static Dynamic>) -> Self {
        self.dynamic = dynamic;
        self
    }
    /// The user's chords for `command`, if the key map names it.
    pub(in crate::app) fn user_chords(&self, command: &str) -> Option<&[Chord]> {
        self.user.get(command).map(Vec::as_slice)
    }
    /// Whether every chord is a built-in default: no command the key map names
    /// and no extension's command, so no two chords can clash.
    pub(in crate::app) fn is_default(&self) -> bool {
        self.user.is_empty() && self.dynamic.is_empty()
    }
    /// Whether the key map names `command`.
    pub(in crate::app) fn overrides(&self, command: &str) -> bool {
        self.user.contains_key(command)
    }
    /// Whether `command`'s chords are not a built-in default the registry's
    /// tests keep free of clashes: the user's, or an extension's.
    pub(in crate::app) fn chosen(&self, command: &str) -> bool {
        self.overrides(command) || self.dynamic.iter().any(|d| d.command.id == command)
    }
    /// The command `id` names, built in or an enabled extension's.
    pub(in crate::app) fn command(&self, id: &str) -> Option<&'static Command> {
        registry::command_by_id(id).or_else(|| {
            self.dynamic
                .iter()
                .find(|d| d.command.id == id)
                .map(|d| &d.command)
        })
    }
    /// The enabled extensions' commands.
    pub(in crate::app) fn dynamic(&self) -> &[&'static Dynamic] {
        &self.dynamic
    }
    /// Every binding: the registry's, then the extensions'.
    pub(in crate::app) fn bindings(&self) -> impl Iterator<Item = &'static Binding> + '_ {
        registry::bindings::ALL
            .iter()
            .copied()
            .chain(self.dynamic.iter().map(|d| &d.binding))
    }
    /// The bindings of command `id`.
    pub(in crate::app) fn bindings_of(&self, id: &str) -> Vec<&'static Binding> {
        self.bindings().filter(|b| b.command == id).collect()
    }

    /// `binding`'s chords: its defaults, or the user's for its command, each
    /// read as the binding's first default reads keys (see [`Default::user`]).
    pub(in crate::app) fn defaults_of<'b>(&'b self, binding: &'b Binding) -> Cow<'b, [Default]> {
        match self.user.get(binding.command) {
            None => Cow::Borrowed(binding.defaults),
            Some(chords) => Cow::Owned(
                chords
                    .iter()
                    .map(|c| Default::user(*c, binding.defaults.first()))
                    .collect(),
            ),
        }
    }
}
