//! The frame's key presses and the one place shortcuts read them.
//!
//! Every shortcut is a [`Binding`]: a command in a context, with the chords
//! ([`Default`](struct@Default)s) that run it. A press is a [`Chord`] when its
//! modifiers are the chord's, except the command's variants and, on a
//! punctuation key, Shift and Alt ([`Chord::free`]). The handlers keep their place in the frame and
//! their guards, and ask [`FrameKeys`] for their bindings' presses instead of
//! reading egui's events themselves. A press runs one command: once a binding
//! takes it, no later binding sees it in that frame. The bindings themselves are
//! listed in [`crate::app::shell::commands::registry`].
//!
//! What stays outside, read in place: held modifiers and gestures, the "any key"
//! checks that stop a glide or cancel a pending click, dialogs' Enter and Escape,
//! text fields, popup-local keys, the hovered slider's Up and Down, the curve's
//! Delete, and the keys the macOS menu injects for a text field.
use eframe::egui::{self, Key, Modifiers};
use std::{borrow::Cow, sync::Arc};

pub(in crate::app) use chord::{Chord, Mods};
pub(in crate::app) use effective::Effective;

pub(in crate::app) mod chord;
mod effective;
pub(in crate::app) mod file;
#[cfg(test)]
mod owner_tests;
pub(in crate::app) mod rules;
#[cfg(test)]
mod tests;

/// How a [`Default`](struct@Default)'s keys are compared with a press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum KeyMatch {
    /// By what the key types, with the number row by its place
    /// ([`crate::app::shell::key_match::shortcut_key`]).
    Typed,
    /// By the key egui reports, as `consume_key` and the views' key loops compare.
    Logical,
    /// By the key's place on the keyboard, or the typed key when the place is
    /// unknown: the Basic panel's keys, which Lightroom places.
    Place,
}

/// The platforms a [`Default`](struct@Default) exists on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Platform {
    All,
    /// Not on macOS, where Cmd+Shift+Z alone redoes.
    NotMacos,
}

/// The chords a binding runs on by default: each of `keys` with exactly
/// `mods` held, the binding's variants and the punctuation rule aside
/// ([`Chord::free`]).
#[derive(Clone, Copy)]
pub(in crate::app) struct Default {
    /// The key and its aliases: `{` beside `[`, Backspace beside Delete. The
    /// first is the one the card shows.
    pub keys: &'static [Key],
    pub mods: Mods,
    /// Shift or Alt that a press of a punctuation key must not hold, though
    /// layouts that need them to type the symbol would otherwise have them
    /// ignored: the Basic panel's `,` and `.` refuse Shift, and `\` leaves
    /// Shift+`\` to the camera thumbnail.
    pub rejects: Mods,
    pub matching: KeyMatch,
    /// Whether a held key's repeats run it too.
    pub repeats: bool,
    pub platform: Platform,
    /// Whether the reference card shows this chord.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "Preferences and the generated card read it (#963)"
        )
    )]
    pub card: bool,
}

impl Default {
    /// `keys` with exactly `mods`, matched as egui reports them, with repeats,
    /// shown on the card.
    pub(in crate::app) const fn new(keys: &'static [Key], mods: Mods) -> Self {
        Self {
            keys,
            mods,
            rejects: Mods::NONE,
            matching: KeyMatch::Logical,
            repeats: true,
            platform: Platform::All,
            card: true,
        }
    }
    /// Only the first press of a held key, not its repeats.
    pub(in crate::app) const fn once_per_press(mut self) -> Self {
        self.repeats = false;
        self
    }
    pub(in crate::app) const fn typed(mut self) -> Self {
        self.matching = KeyMatch::Typed;
        self
    }
    pub(in crate::app) const fn placed(mut self) -> Self {
        self.matching = KeyMatch::Place;
        self
    }
    pub(in crate::app) const fn not_macos(mut self) -> Self {
        self.platform = Platform::NotMacos;
        self
    }
    /// Left off the reference card: an alias, or a key another row shows.
    pub(in crate::app) const fn off_card(mut self) -> Self {
        self.card = false;
        self
    }
    /// Refuses `mods` on a punctuation key ([`Default::rejects`]).
    pub(in crate::app) const fn rejecting(mut self, mods: Mods) -> Self {
        self.rejects = mods;
        self
    }

    /// A chord of the user's for a binding whose chords `template` stands
    /// for: matched by what the key types (the Basic panel's keys by their
    /// place), with the template's repeats and rejected modifiers, on every
    /// platform and on the card.
    pub(in crate::app) fn user(chord: Chord, template: Option<&Default>) -> Self {
        Self {
            keys: one_key(chord.key),
            mods: chord.mods,
            rejects: template.map_or(Mods::NONE, |t| t.rejects),
            matching: match template.map(|t| t.matching) {
                Some(KeyMatch::Place) => KeyMatch::Place,
                _ => KeyMatch::Typed,
            },
            repeats: template.is_none_or(|t| t.repeats),
            platform: Platform::All,
            card: true,
        }
    }

    /// Each key with this chord's modifiers.
    pub(in crate::app) fn chords(&self) -> impl Iterator<Item = Chord> + '_ {
        self.keys.iter().map(|k| Chord::new(*k, self.mods))
    }
    /// The chord as the reference card writes it ("Cmd+Shift+E"): its first key's.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "Preferences and the generated card read it (#963)"
        )
    )]
    pub(in crate::app) fn label(&self) -> String {
        self.chords()
            .next()
            .map(|c| c.to_string())
            .unwrap_or_default()
    }

    fn exists(&self) -> bool {
        self.exists_on(cfg!(target_os = "macos"))
    }
    /// Whether the chord exists on a Mac (`macos`) or elsewhere.
    pub(in crate::app) fn exists_on(&self, macos: bool) -> bool {
        match self.platform {
            Platform::All => true,
            Platform::NotMacos => !macos,
        }
    }
    /// The key a press of `key` at `physical` stands for, as this chord reads keys.
    fn read_key(&self, key: Key, physical: Option<Key>) -> Key {
        match self.matching {
            KeyMatch::Typed => crate::app::shell::key_match::shortcut_key(key, physical),
            KeyMatch::Logical => key,
            KeyMatch::Place => physical.unwrap_or(key),
        }
    }
    fn matches_key(&self, key: Key, physical: Option<Key>) -> bool {
        self.keys.contains(&self.read_key(key, physical))
    }
    /// Whether a press of `key` at `physical` with `modifiers` is one of these
    /// chords, `variants` being free: repeats aside.
    fn takes_key(
        &self,
        key: Key,
        physical: Option<Key>,
        modifiers: Modifiers,
        variants: Mods,
    ) -> bool {
        let key = self.read_key(key, physical);
        let held = Mods::of(modifiers);
        self.exists()
            && self.chords().any(|c| {
                let free = c.free(variants, self.rejects);
                c.accepts(key, held, free)
            })
    }
    /// Whether `press` is one of these chords, `variants` being free.
    fn matches(&self, press: &Press, variants: Mods) -> bool {
        (self.repeats || !press.repeat)
            && self.takes_key(press.key, press.physical, press.modifiers, variants)
    }
}

/// Where a binding acts. A binding runs only where its handler runs; the
/// context names that place for the registry, Preferences and conflicts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Context {
    Everywhere,
    /// The Library, whatever its view.
    Library,
    Grid,
    Loupe,
    Compare,
    Survey,
    /// The Library's Loupe and Develop: the zoom keys.
    LoupeZoom,
    Develop,
    Tool(Tool),
    /// The reference card, the one window with keys of its own.
    ShortcutSheet,
}

/// A Develop tool with keys of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Tool {
    Crop,
    Remove,
    RedEye,
    Mask,
    Guided,
}

/// How many of a frame's presses one binding takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum PerFrame {
    /// The first press not yet taken.
    Each,
    /// Every matching press, acting once: two presses of F in a frame hide the
    /// chrome once.
    Once,
}

/// A command's keys in one context.
pub(in crate::app) struct Binding {
    /// The command's id in [`crate::app::shell::commands::registry::COMMANDS`].
    pub command: &'static str,
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "conflicts and Preferences read it (#963)")
    )]
    pub context: Context,
    /// Tried in order: the first chord with a press not yet taken wins, as a
    /// chain of `consume_key` calls tries its patterns.
    pub defaults: &'static [Default],
    /// The modifiers passed to the command rather than matched: Shift goes on
    /// to the next photo after a rating, extends the Grid's selection, or
    /// feathers a brush; Shift and Alt pick a Basic slider's step.
    pub variants: Mods,
    /// Whether a press this binding takes leaves egui's input, as
    /// `consume_key` takes it, so no widget sees it either.
    pub consumes: bool,
    pub per_frame: PerFrame,
}

impl Binding {
    pub(in crate::app) const fn new(
        command: &'static str,
        context: Context,
        defaults: &'static [Default],
    ) -> Self {
        Self {
            command,
            context,
            defaults,
            variants: Mods::NONE,
            consumes: false,
            per_frame: PerFrame::Each,
        }
    }
    pub(in crate::app) const fn consuming(mut self) -> Self {
        self.consumes = true;
        self
    }
    pub(in crate::app) const fn once(mut self) -> Self {
        self.per_frame = PerFrame::Once;
        self
    }
    /// Passes `mods` to the command ([`Binding::variants`]).
    pub(in crate::app) const fn variant(mut self, mods: Mods) -> Self {
        self.variants = mods;
        self
    }
    /// Whether `press` is one of this binding's default chords.
    #[cfg_attr(not(test), allow(dead_code, reason = "the tests read defaults alone"))]
    pub(in crate::app) fn matches(&self, press: &Press) -> bool {
        matches_any(self.defaults, press, self.variants)
    }
}

fn matches_any(defaults: &[Default], press: &Press, variants: Mods) -> bool {
    defaults.iter().any(|d| d.matches(press, variants))
}

/// The one key of a user's chord, as the `'static` slice a [`Default`](struct@Default) holds.
fn one_key(key: Key) -> &'static [Key] {
    let at = Key::ALL
        .iter()
        .position(|k| *k == key)
        .expect("every egui key is in Key::ALL");
    std::slice::from_ref(&Key::ALL[at])
}

/// A key press in this frame, with the modifiers held for it: a quick
/// Cmd+Shift+R can release both before the frame ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) struct Press {
    pub key: Key,
    pub physical: Option<Key>,
    pub modifiers: Modifiers,
    pub repeat: bool,
    /// Which of the frame's identical presses this is: egui events carry no
    /// identity of their own.
    occurrence: usize,
}

impl Press {
    /// The press `event` is, if it is one; it counts as the first of its kind.
    pub(in crate::app) fn of(event: &egui::Event) -> Option<Self> {
        match event {
            egui::Event::Key {
                key,
                physical_key,
                pressed: true,
                repeat,
                modifiers,
            } => Some(Self {
                key: *key,
                physical: *physical_key,
                modifiers: *modifiers,
                repeat: *repeat,
                occurrence: 0,
            }),
            _ => None,
        }
    }
    fn same_keys(&self, other: &Self) -> bool {
        (self.key, self.physical, self.modifiers, self.repeat)
            == (other.key, other.physical, other.modifiers, other.repeat)
    }
}

/// The presses taken in the pass `pass`.
#[derive(Clone, Default)]
struct Taken {
    pass: u64,
    presses: Vec<Press>,
}

fn taken_id() -> egui::Id {
    egui::Id::new("keymap-taken-presses")
}
fn effective_id() -> egui::Id {
    egui::Id::new("keymap-effective")
}

/// The frame's key presses, for the handlers to take.
///
/// It reads the context's input as it is when asked, so a press a widget has
/// already consumed (a text field's, a dialog's Enter) is not offered. What has
/// been taken is kept in the context's memory for the pass: the handlers that
/// read keys are on `Editor`, the Library and the message line, and a test may
/// run one of them alone. So are the chords in effect, which
/// [`FrameKeys::begin`] puts there: the defaults until it has run. Nothing here
/// holds egui's lock while a command runs.
#[derive(Clone)]
pub(in crate::app) struct FrameKeys<'a> {
    ctx: &'a egui::Context,
    map: Arc<Effective>,
}

impl<'a> FrameKeys<'a> {
    pub(in crate::app) fn of(ctx: &'a egui::Context) -> Self {
        let map = ctx
            .data(|d| d.get_temp::<Arc<Effective>>(effective_id()))
            .unwrap_or_default();
        Self { ctx, map }
    }
    /// Starts the frame with `map`'s chords in effect: nothing taken yet.
    pub(in crate::app) fn begin(ctx: &'a egui::Context, map: Arc<Effective>) -> Self {
        ctx.data_mut(|d| d.insert_temp(effective_id(), map.clone()));
        let keys = Self { ctx, map };
        keys.set_taken(Vec::new());
        keys
    }

    /// `binding`'s chords in effect: the user's for its command, or its defaults.
    pub(in crate::app) fn chords_of<'b>(&'b self, binding: &'b Binding) -> Cow<'b, [Default]> {
        self.map.defaults_of(binding)
    }
    /// Whether `press` is one of `binding`'s chords in effect, and still to be
    /// taken: for a handler that walks [`Self::presses`] and takes the press
    /// it acts on.
    pub(in crate::app) fn is(&self, press: &Press, binding: &Binding) -> bool {
        matches_any(&self.chords_of(binding), press, binding.variants)
    }
    /// Whether a press of `key` at `physical` is one of `binding`'s keys in
    /// effect, whatever the modifiers.
    pub(in crate::app) fn has_key(
        &self,
        binding: &Binding,
        key: Key,
        physical: Option<Key>,
    ) -> bool {
        self.chords_of(binding)
            .iter()
            .any(|d| d.exists() && d.matches_key(key, physical))
    }

    /// The first press, in the binding's chord order, that `binding` matches
    /// and no handler has taken: taken now, and gone from egui's input as well
    /// when the binding consumes. Its modifiers.
    pub(in crate::app) fn take(&self, binding: &Binding) -> Option<Modifiers> {
        let open = self.presses();
        let chords = self.chords_of(binding);
        let (default, press) = chords.iter().find_map(|d| {
            open.iter()
                .find(|p| d.matches(p, binding.variants))
                .map(|p| (d, *p))
        })?;
        let mut taken = match binding.per_frame {
            PerFrame::Each => vec![press],
            PerFrame::Once => open
                .iter()
                .filter(|p| matches_any(&chords, p, binding.variants))
                .copied()
                .collect(),
        };
        let mut all = self.taken();
        all.append(&mut taken);
        self.set_taken(all);
        if binding.consumes {
            // Every press of this chord, repeats included, as `consume_key`
            // takes every press its pattern matches.
            self.remove(|p| default.takes_key(p.key, p.physical, p.modifiers, binding.variants));
        }
        Some(press.modifiers)
    }
    /// The first of `bindings`, in their order, with a press to take, as a
    /// chain of `if`s over them: its index and the press's modifiers.
    pub(in crate::app) fn take_any(&self, bindings: &[&Binding]) -> Option<(usize, Modifiers)> {
        bindings
            .iter()
            .enumerate()
            .find_map(|(i, b)| self.take(b).map(|m| (i, m)))
    }
    /// The frame's first press, in the order the keys came, that one of
    /// `bindings` matches: taken by the first binding that does. That
    /// binding's index and the press's modifiers. For a key whose modifiers
    /// pick the command (Y, Option+Y and Shift+Y), as one read of the key.
    pub(in crate::app) fn take_first(&self, bindings: &[&Binding]) -> Option<(usize, Modifiers)> {
        let (index, press) = self.presses().into_iter().find_map(|p| {
            bindings
                .iter()
                .position(|b| self.is(&p, b))
                .map(|index| (index, p))
        })?;
        debug_assert!(
            !bindings[index].consumes && bindings[index].per_frame == PerFrame::Each,
            "take_first is for bindings that take one press and leave it in the input"
        );
        self.take_press(&press);
        Some((index, press.modifiers))
    }

    /// The presses no handler has taken yet, in the order they came, for a
    /// handler that walks them as the views' key loops do.
    pub(in crate::app) fn presses(&self) -> Vec<Press> {
        let taken = self.taken();
        self.read()
            .into_iter()
            .filter(|p| !taken.contains(p))
            .collect()
    }
    /// Takes `press`, one of [`Self::presses`], for a command.
    pub(in crate::app) fn take_press(&self, press: &Press) {
        let mut taken = self.taken();
        if !taken.contains(press) {
            taken.push(*press);
            self.set_taken(taken);
        }
    }
    /// Takes `press` out of egui's input, that one event alone, so no widget
    /// drawn after sees it either.
    pub(in crate::app) fn remove_press(&self, press: &Press) {
        self.remove(|p| p == press);
    }

    /// Every key press in the input now, numbered among identical ones.
    fn read(&self) -> Vec<Press> {
        self.ctx
            .input(|i| number(i.events.iter().filter_map(Press::of)))
    }
    fn taken(&self) -> Vec<Press> {
        let pass = self.ctx.cumulative_pass_nr();
        self.ctx
            .data(|d| d.get_temp::<Taken>(taken_id()))
            .filter(|t| t.pass == pass)
            .map(|t| t.presses)
            .unwrap_or_default()
    }
    fn set_taken(&self, presses: Vec<Press>) {
        let pass = self.ctx.cumulative_pass_nr();
        self.ctx
            .data_mut(|d| d.insert_temp(taken_id(), Taken { pass, presses }));
    }
    /// Takes the presses `gone` picks out of egui's input. The presses left
    /// keep whether they were taken, numbered again among identical ones.
    fn remove(&self, gone: impl Fn(&Press) -> bool) {
        let before = self.read();
        let taken = self.taken();
        let keep: Vec<bool> = before.iter().map(|p| !gone(p)).collect();
        if keep.iter().all(|k| *k) {
            return;
        }
        self.ctx.input_mut(|i| {
            let mut at = 0;
            i.events.retain(|e| {
                if Press::of(e).is_none() {
                    return true;
                }
                at += 1;
                keep[at - 1]
            });
        });
        let left = number(
            before
                .iter()
                .zip(&keep)
                .filter(|(_, k)| **k)
                .map(|(p, _)| *p),
        );
        let still: Vec<Press> = before
            .iter()
            .zip(&keep)
            .filter(|(_, k)| **k)
            .zip(&left)
            .filter(|((p, _), _)| taken.contains(p))
            .map(|(_, renumbered)| *renumbered)
            .collect();
        self.set_taken(still);
    }
}

/// `presses` with each numbered among the identical ones before it.
fn number(presses: impl Iterator<Item = Press>) -> Vec<Press> {
    let mut out: Vec<Press> = Vec::new();
    for mut p in presses {
        p.occurrence = out.iter().filter(|q| q.same_keys(&p)).count();
        out.push(p);
    }
    out
}
