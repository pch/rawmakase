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
pub(in crate::app) mod conflicts;
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

impl KeyMatch {
    /// How a user's chord on `key` is matched for a binding whose chords
    /// `template` stands for: the Basic panel's keys by their place; a digit
    /// by the number row's place, so AZERTY's digit row still reaches it;
    /// anything else by what it types, so German Cmd+Shift+7 (`/`) is the
    /// user's Cmd+/ as it is the default's. Where a press could be either, a
    /// binding of its digit wins ([`FrameKeys::is`]).
    fn for_user(template: Option<&Default>, key: Key) -> Self {
        match template.map(|t| t.matching) {
            Some(Self::Place) => Self::Place,
            _ if crate::app::shell::key_match::is_digit(key) => Self::Typed,
            _ => Self::Logical,
        }
    }
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
    /// for, with the template's repeats and rejected modifiers, on every
    /// platform and on the card. It is matched as the binding's own keys are
    /// ([`KeyMatch::for_user`]).
    pub(in crate::app) fn user(chord: Chord, template: Option<&Default>) -> Self {
        Self {
            keys: one_key(chord.key),
            mods: chord.mods,
            rejects: template.map_or(Mods::NONE, |t| t.rejects),
            matching: KeyMatch::for_user(template, chord.key),
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

/// The states of the window in which a binding's handler doesn't run, beside
/// its context: what each handler checks before it takes its keys. They say
/// which bindings a press can be left to, or conflict with
/// ([`FrameKeys::set_live`]), and when the macOS menu holds a key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) struct Guards(u8);

impl Guards {
    pub(in crate::app) const NONE: Self = Self(0);
    /// A text field has the keys.
    pub(in crate::app) const TYPING: Self = Self(1);
    /// A menu or popup is open.
    pub(in crate::app) const POPUP: Self = Self(2);
    /// A control or field has the keyboard focus.
    pub(in crate::app) const FOCUS: Self = Self(4);
    /// A file dialog or a Sync runs.
    pub(in crate::app) const BUSY: Self = Self(8);
    /// A window over the workspace (Preferences, the reference card, a dialog).
    pub(in crate::app) const MODAL: Self = Self(16);

    pub(in crate::app) const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
    /// Whether any state of `other` is one of these.
    pub(in crate::app) const fn meets(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

/// A command's keys in one context.
pub(in crate::app) struct Binding {
    /// The command's id in [`crate::app::shell::commands::registry::COMMANDS`].
    pub command: &'static str,
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
    /// When its handler doesn't run. By default: while typing, while busy and
    /// behind a window, and in a Library view with a menu open too; nothing
    /// stops the reference card's keys, which only run while it is open.
    pub guards: Guards,
}

impl Binding {
    pub(in crate::app) const fn new(
        command: &'static str,
        context: Context,
        defaults: &'static [Default],
    ) -> Self {
        let usual = Guards::TYPING.with(Guards::BUSY).with(Guards::MODAL);
        Self {
            command,
            context,
            defaults,
            variants: Mods::NONE,
            consumes: false,
            per_frame: PerFrame::Each,
            guards: match context {
                Context::ShortcutSheet => Guards::NONE,
                Context::Grid | Context::Loupe | Context::Compare | Context::Survey => {
                    usual.with(Guards::POPUP)
                }
                _ => usual,
            },
        }
    }
    /// Runs except in the states `guards` names, instead of the usual ones.
    pub(in crate::app) const fn guarded(mut self, guards: Guards) -> Self {
        self.guards = guards;
        self
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

/// The presses taken in the pass `pass`, and those left to a binding of
/// higher precedence (by its address).
#[derive(Clone, Default)]
struct Taken {
    pass: u64,
    presses: Vec<Press>,
    reserved: Vec<(Press, usize)>,
    /// Preferences is waiting for a key to bind: no binding takes any.
    capturing: bool,
    /// The chords the macOS menu takes before egui sees them.
    menu: Vec<Chord>,
}

/// A binding's identity: bindings are statics.
fn address(binding: &Binding) -> usize {
    std::ptr::from_ref(binding) as usize
}

/// The contexts active now, which the editor says as it goes through the
/// frame ([`FrameKeys::set_live`]): what a press can be left to, or conflict
/// with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::app) struct Live {
    contexts: Vec<Context>,
    /// The states the window is in, which some bindings don't run in.
    now: Guards,
}

impl Live {
    pub(in crate::app) fn new(contexts: Vec<Context>) -> Self {
        Self {
            contexts,
            now: Guards::NONE,
        }
    }
    /// With the window in the states `now`.
    pub(in crate::app) fn during(mut self, now: Guards) -> Self {
        self.now = now;
        self
    }
    pub(in crate::app) fn has(&self, context: Context) -> bool {
        self.contexts.contains(&context)
    }
    /// Whether `binding`'s handler can take a press now: its context is
    /// active and none of its guards holds.
    pub(in crate::app) fn runs(&self, binding: &Binding) -> bool {
        self.has(binding.context) && !binding.guards.meets(self.now)
    }
}

/// What becomes of a press a binding matches.
enum Verdict<'b> {
    Run,
    /// Left to a binding of higher precedence, which may take it later.
    Leave,
    /// Two bindings of the highest precedence that can take it have it: neither
    /// runs, nor anything below them.
    Conflict(&'b Binding, &'b Binding),
}

/// How a binding reads a press of a number-row key that types a symbol
/// (AZERTY's `'` on the 4 key): as the digit, or as the symbol.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reading {
    Digit,
    Symbol,
    /// Not a number-row key that types a symbol.
    Plain,
}

/// How the first of `chords` that `press` is reads it, if one is.
fn reading(chords: &[Default], press: &Press, variants: Mods) -> Option<Reading> {
    let default = chords.iter().find(|d| d.matches(press, variants))?;
    let symbol_on_digit_row =
        crate::app::shell::key_match::shortcut_key(press.key, press.physical) != press.key;
    Some(if !symbol_on_digit_row {
        Reading::Plain
    } else if default.read_key(press.key, press.physical) == press.key {
        Reading::Symbol
    } else {
        Reading::Digit
    })
}

fn taken_id() -> egui::Id {
    egui::Id::new("keymap-taken-presses")
}
fn effective_id() -> egui::Id {
    egui::Id::new("keymap-effective")
}
fn live_id() -> egui::Id {
    egui::Id::new("keymap-live")
}
fn reports_id() -> egui::Id {
    egui::Id::new("keymap-conflicts")
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
    /// Starts the frame with `map`'s chords in effect: nothing taken yet,
    /// and no context known to be active until [`Self::set_live`].
    pub(in crate::app) fn begin(ctx: &'a egui::Context, map: Arc<Effective>) -> Self {
        ctx.data_mut(|d| {
            d.insert_temp(effective_id(), map.clone());
            d.remove::<Live>(live_id());
        });
        let keys = Self { ctx, map };
        keys.set_taken(Vec::new());
        keys
    }
    /// The contexts active from now on, until the editor says otherwise. A
    /// user's chord defers to a binding of higher precedence, or conflicts
    /// with one of the same, only in an active context.
    pub(in crate::app) fn set_live(&self, live: Live) {
        self.ctx.data_mut(|d| d.insert_temp(live_id(), live));
    }
    /// While `on`, no binding sees a press, so a key pressed for Preferences
    /// to bind runs nothing; [`Self::capture`] hands it over instead.
    pub(in crate::app) fn set_capturing(&self, on: bool) {
        let mut state = self.state();
        state.capturing = on;
        self.set_state(state);
    }
    /// The chords the installed macOS menu's enabled items take: no binding
    /// takes a press of one, which AppKit has already run.
    #[cfg_attr(
        not(any(target_os = "macos", test)),
        allow(dead_code, reason = "the macOS menu's alone")
    )]
    pub(in crate::app) fn set_menu_chords(&self, chords: Vec<Chord>) {
        let mut state = self.state();
        state.menu = chords;
        self.set_state(state);
    }
    /// The first press of a key that isn't a modifier, which Preferences
    /// binds, out of egui's input so nothing else sees it. A modifier pressed
    /// alone (egui-winit sends Shift, Control, Alt and Super as keys of their
    /// own) is left, and the capture waits for the key it is held with.
    pub(in crate::app) fn capture(&self) -> Option<Press> {
        let press = self
            .read()
            .into_iter()
            .find(|p| !p.repeat && !is_modifier(p.key))?;
        self.remove_press(&press);
        Some(press)
    }
    /// The messages about conflicting chords since the last call: one for
    /// each press that ran neither of two commands.
    pub(in crate::app) fn take_reports(&self) -> Vec<String> {
        self.ctx
            .data_mut(|d| d.remove_temp::<Vec<String>>(reports_id()))
            .unwrap_or_default()
    }

    /// `binding`'s chords in effect: the user's for its command, or its defaults.
    pub(in crate::app) fn chords_of<'b>(&'b self, binding: &'b Binding) -> Cow<'b, [Default]> {
        self.map.defaults_of(binding)
    }
    /// Whether `press` is one of `binding`'s chords in effect, and still to be
    /// taken: for a handler that walks [`Self::presses`] and takes the press
    /// it acts on.
    pub(in crate::app) fn is(&self, press: &Press, binding: &Binding) -> bool {
        matches_any(&self.chords_of(binding), press, binding.variants) && self.runs(press, binding)
    }
    /// Whether `press` is one of `binding`'s chords in effect, `free`
    /// modifiers aside as well as its variants, whatever its repeats and
    /// whoever takes it: for a handler that gives way to another's key.
    pub(in crate::app) fn has_chord(&self, press: &Press, binding: &Binding, free: Mods) -> bool {
        self.chords_of(binding).iter().any(|d| {
            d.takes_key(
                press.key,
                press.physical,
                press.modifiers,
                binding.variants.with(free),
            )
        })
    }

    /// The first press, in the binding's chord order, that `binding` matches
    /// and no handler has taken: taken now, and gone from egui's input as well
    /// when the binding consumes. Its modifiers.
    pub(in crate::app) fn take(&self, binding: &Binding) -> Option<Modifiers> {
        let open = self.presses();
        let chords = self.chords_of(binding);
        let (default, press) = chords.iter().find_map(|d| {
            open.iter()
                .find(|p| d.matches(p, binding.variants) && self.runs(p, binding))
                .map(|p| (d, *p))
        })?;
        let mut taken = match binding.per_frame {
            PerFrame::Each => vec![press],
            // Each further press goes through the same arbitration as the
            // first: a press reserved for, or owned by, another binding stays
            // theirs, and a conflict is reported and runs neither.
            PerFrame::Once => open
                .iter()
                .filter(|p| {
                    **p == press
                        || (matches_any(&chords, p, binding.variants) && self.runs(p, binding))
                })
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

    /// Whether `binding`, which matches `press`, runs it, by the user's chords'
    /// precedence. Among the bindings that can take the press now (their
    /// context active, none of their guards holding), the highest level
    /// owns it: a lower binding leaves it, and it is reserved for that owner
    /// alone. Two commands on that level that both have it conflict: neither
    /// runs, the press is taken, and it is reported. On the number row, a
    /// binding of the digit comes before one of the symbol the key types.
    /// Only where the user's chords are involved: the defaults keep their
    /// explicit order.
    fn runs(&self, press: &Press, binding: &Binding) -> bool {
        match self.verdict(press, binding) {
            Verdict::Run => true,
            Verdict::Leave => false,
            Verdict::Conflict(a, b) => {
                self.take_press(press);
                let chord = self
                    .chords_of(binding)
                    .iter()
                    .find(|d| d.matches(press, binding.variants))
                    .map(Default::label)
                    .unwrap_or_default();
                let title = |b: &Binding| {
                    crate::app::shell::commands::registry::command_by_id(b.command)
                        .map_or(b.command, |c| c.title)
                };
                let report = format!(
                    "{chord} is the key of both \u{201c}{}\u{201d} and \u{201c}{}\u{201d}, so neither ran. \
                     Change one in Preferences > Shortcuts.",
                    title(a),
                    title(b)
                );
                self.ctx.data_mut(|d| {
                    d.get_temp_mut_or_default::<Vec<String>>(reports_id())
                        .push(report)
                });
                false
            }
        }
    }
    fn verdict<'b>(&self, press: &Press, binding: &'b Binding) -> Verdict<'b> {
        let state = self.state();
        // Taken by a conflict earlier in a handler's walk of the presses.
        if state.presses.contains(press) {
            return Verdict::Leave;
        }
        if let Some((_, owner)) = state.reserved.iter().find(|(p, _)| p == press) {
            return if *owner == address(binding) {
                Verdict::Run
            } else {
                Verdict::Leave
            };
        }
        if self.map.is_default() {
            return Verdict::Run;
        }
        let Some(live) = self.ctx.data(|d| d.get_temp::<Live>(live_id())) else {
            return Verdict::Run;
        };
        let Some(mine) = reading(&self.chords_of(binding), press, binding.variants) else {
            return Verdict::Run;
        };
        let mut rivals: Vec<(&'static Binding, Reading)> =
            crate::app::shell::commands::registry::bindings::ALL
                .iter()
                .copied()
                .filter(|other| {
                    other.command != binding.command
                        && live.runs(other)
                        && other.context.overlaps(binding.context)
                })
                .filter_map(|other| {
                    reading(&self.chords_of(other), press, other.variants).map(|r| (other, r))
                })
                .collect();
        let users = self.map.overrides(binding.command)
            || rivals.iter().any(|(o, _)| self.map.overrides(o.command));
        if !users {
            return Verdict::Run;
        }
        // The number row's digit comes first.
        let outranked = match mine {
            Reading::Digit => {
                rivals.retain(|(_, r)| *r != Reading::Symbol);
                false
            }
            Reading::Symbol if rivals.iter().any(|(_, r)| *r == Reading::Digit) => {
                rivals.retain(|(_, r)| *r == Reading::Digit);
                true
            }
            _ => false,
        };
        let level = rules::level(binding.context);
        let top = rivals.iter().map(|(o, _)| rules::level(o.context)).max();
        match top {
            Some(top) if outranked || top > level => {
                let owners: Vec<&'static Binding> = rivals
                    .iter()
                    .map(|(o, _)| *o)
                    .filter(|o| rules::level(o.context) == top)
                    .collect();
                let owner = owners[0];
                if let Some(other) = owners.iter().find(|o| o.command != owner.command) {
                    return Verdict::Conflict(owner, other);
                }
                let mut state = state;
                state.reserved.push((*press, address(owner)));
                self.set_state(state);
                Verdict::Leave
            }
            _ => match rivals
                .iter()
                .find(|(o, _)| rules::level(o.context) == level)
            {
                Some((other, _)) => Verdict::Conflict(binding, other),
                None => Verdict::Run,
            },
        }
    }

    /// The presses no handler has taken yet, in the order they came, for a
    /// handler that walks them as the views' key loops do.
    pub(in crate::app) fn presses(&self) -> Vec<Press> {
        let state = self.state();
        if state.capturing {
            return Vec::new();
        }
        let menu = |p: &Press| {
            let key = crate::app::shell::key_match::shortcut_key(p.key, p.physical);
            state
                .menu
                .iter()
                .any(|c| c.key == key && c.mods == Mods::of(p.modifiers))
        };
        self.read()
            .into_iter()
            .filter(|p| !state.presses.contains(p) && !menu(p))
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
    fn state(&self) -> Taken {
        let pass = self.ctx.cumulative_pass_nr();
        self.ctx
            .data(|d| d.get_temp::<Taken>(taken_id()))
            .filter(|t| t.pass == pass)
            .unwrap_or(Taken {
                pass,
                ..Taken::default()
            })
    }
    fn set_state(&self, state: Taken) {
        self.ctx.data_mut(|d| d.insert_temp(taken_id(), state));
    }
    fn taken(&self) -> Vec<Press> {
        self.state().presses
    }
    fn set_taken(&self, presses: Vec<Press>) {
        let mut state = self.state();
        state.presses = presses;
        self.set_state(state);
    }
    /// Takes the presses `gone` picks out of egui's input. The presses left
    /// keep whether they were taken, numbered again among identical ones.
    fn remove(&self, gone: impl Fn(&Press) -> bool) {
        let before = self.read();
        let state = self.state();
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
        let kept = || before.iter().zip(&keep).filter(|(_, k)| **k).zip(&left);
        let presses = kept()
            .filter(|((p, _), _)| state.presses.contains(p))
            .map(|(_, renumbered)| *renumbered)
            .collect();
        let reserved = kept()
            .filter_map(|((p, _), renumbered)| {
                state
                    .reserved
                    .iter()
                    .find(|(r, _)| r == p)
                    .map(|(_, owner)| (*renumbered, *owner))
            })
            .collect();
        self.set_state(Taken {
            pass: state.pass,
            presses,
            reserved,
            capturing: state.capturing,
            menu: state.menu,
        });
    }
}

/// Whether `key` is a modifier pressed as a key of its own.
pub(in crate::app) fn is_modifier(key: Key) -> bool {
    matches!(
        key,
        Key::ShiftLeft
            | Key::ShiftRight
            | Key::ControlLeft
            | Key::ControlRight
            | Key::AltLeft
            | Key::AltRight
            | Key::SuperLeft
            | Key::SuperRight
    )
}

/// The chord a captured press stands for, for a command whose chords
/// `template` stands for: its key as that command reads keys (the Basic
/// panel's by their place, the number row by its place where the command goes
/// by what keys type), with the modifiers held for it, except that on a
/// punctuation key Shift and Alt are left out, as a layout may need them to
/// type the symbol at all ([`Chord::free`]).
pub(in crate::app) fn chord_of(press: &Press, template: Option<&Default>) -> Chord {
    let key = match template.map(|t| t.matching) {
        Some(KeyMatch::Place) => press.physical.unwrap_or(press.key),
        Some(KeyMatch::Logical) => press.key,
        Some(KeyMatch::Typed) | None => {
            crate::app::shell::key_match::shortcut_key(press.key, press.physical)
        }
    };
    let mut mods = Mods::of(press.modifiers);
    if chord::is_punctuation(key) {
        mods = mods.without(Mods::SHIFT.with(Mods::ALT));
    }
    Chord::new(key, mods)
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
