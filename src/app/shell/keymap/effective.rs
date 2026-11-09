//! The chords in effect: the user's for the commands their key map names, the
//! defaults for the rest.
#![cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "conflicts, Preferences and the card read the user's chords (#963)"
    )
)]
use super::{Binding, Chord, Default};
use std::{borrow::Cow, collections::BTreeMap};

/// The user's chords, by command. A command the user's key map names runs on
/// those chords alone, in every context it has a binding in; an empty list
/// leaves it with no key. Every other command keeps this release's defaults,
/// so new defaults arrive with an update.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::app) struct Effective {
    user: BTreeMap<&'static str, Vec<Chord>>,
}

impl Effective {
    /// `user`'s chords over the defaults. The ids are the registry's
    /// ([`crate::app::shell::commands::registry::COMMANDS`]).
    pub(in crate::app) fn new(user: BTreeMap<&'static str, Vec<Chord>>) -> Self {
        Self { user }
    }
    /// The user's chords for `command`, if the key map names it.
    pub(in crate::app) fn user_chords(&self, command: &str) -> Option<&[Chord]> {
        self.user.get(command).map(Vec::as_slice)
    }
    /// Whether the key map names no command: the defaults alone.
    pub(in crate::app) fn is_default(&self) -> bool {
        self.user.is_empty()
    }
    /// Whether the key map names `command`.
    pub(in crate::app) fn overrides(&self, command: &str) -> bool {
        self.user.contains_key(command)
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
