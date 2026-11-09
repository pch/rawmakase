//! The frame's key presses and the one place shortcuts read them.
//!
//! Every shortcut is a [`Binding`]: a command in a context, with the chords
//! ([`Default`](struct@Default)s) that run it. The handlers keep their place in the frame and
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

/// One chord a binding runs on by default.
#[derive(Clone, Copy)]
pub(in crate::app) struct Default {
    pub keys: &'static [Key],
    pub matching: KeyMatch,
    /// The modifiers held for the press that this chord accepts. Each is
    /// today's exact predicate, extra modifiers it tolerates included.
    pub mods: fn(Modifiers) -> bool,
    /// Whether a held key's repeats run it too.
    pub repeats: bool,
    pub platform: Platform,
    /// The chord as the reference card writes it ("Cmd+Shift+E").
    pub label: &'static str,
    /// Whether the reference card shows this chord.
    pub card: bool,
}

impl Default {
    /// A chord on `keys`, matched as egui reports them, with repeats, shown on
    /// the card.
    pub(in crate::app) const fn new(
        keys: &'static [Key],
        mods: fn(Modifiers) -> bool,
        label: &'static str,
    ) -> Self {
        Self {
            keys,
            matching: KeyMatch::Logical,
            mods,
            repeats: true,
            platform: Platform::All,
            label,
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

    fn exists(&self) -> bool {
        match self.platform {
            Platform::All => true,
            Platform::NotMacos => !cfg!(target_os = "macos"),
        }
    }
    fn matches_key(&self, key: Key, physical: Option<Key>) -> bool {
        let key = match self.matching {
            KeyMatch::Typed => crate::app::shell::key_match::shortcut_key(key, physical),
            KeyMatch::Logical => key,
            KeyMatch::Place => physical.unwrap_or(key),
        };
        self.keys.contains(&key)
    }
    /// Whether `press` is this chord.
    pub(in crate::app) fn matches(&self, press: &Press) -> bool {
        self.exists()
            && (self.repeats || !press.repeat)
            && self.matches_key(press.key, press.physical)
            && (self.mods)(press.modifiers)
    }
    /// Whether this chord is the press of `key` at `physical` with any
    /// modifiers and repeat: what `consume_key` takes once the chord's
    /// modifiers pass.
    fn takes_key(&self, key: Key, physical: Option<Key>, modifiers: Modifiers) -> bool {
        self.exists() && self.matches_key(key, physical) && (self.mods)(modifiers)
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
    pub context: Context,
    /// Tried in order: the first chord with a press not yet taken wins, as a
    /// chain of `consume_key` calls tries its patterns.
    pub defaults: &'static [Default],
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
    /// Whether `press` is one of this binding's chords.
    pub(in crate::app) fn matches(&self, press: &Press) -> bool {
        self.defaults.iter().any(|d| d.matches(press))
    }
    /// Whether a press of `key` at `physical` is one of this binding's keys,
    /// whatever the modifiers.
    pub(in crate::app) fn has_key(&self, key: Key, physical: Option<Key>) -> bool {
        self.defaults
            .iter()
            .any(|d| d.exists() && d.matches_key(key, physical))
    }
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
    pub(in crate::app) fn is(&self, binding: &Binding) -> bool {
        binding.matches(self)
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

/// The frame's key presses, for the handlers to take.
///
/// It reads the context's input as it is when asked, so a press a widget has
/// already consumed (a text field's, a dialog's Enter) is not offered. What has
/// been taken is kept in the context's memory for the pass: the handlers that
/// read keys are on `Editor`, the Library and the message line, and a test may
/// run one of them alone. Nothing here holds egui's lock while a command runs.
#[derive(Clone, Copy)]
pub(in crate::app) struct FrameKeys<'a> {
    ctx: &'a egui::Context,
}

impl<'a> FrameKeys<'a> {
    pub(in crate::app) fn of(ctx: &'a egui::Context) -> Self {
        Self { ctx }
    }
    /// Starts the frame: nothing taken yet. A new pass starts afresh anyway;
    /// this marks where the frame's key handling begins.
    pub(in crate::app) fn begin(self) {
        self.set_taken(Vec::new());
    }

    /// The first press, in the binding's chord order, that `binding` matches
    /// and no handler has taken: taken now, and gone from egui's input as well
    /// when the binding consumes. Its modifiers.
    pub(in crate::app) fn take(self, binding: &Binding) -> Option<Modifiers> {
        let open = self.presses();
        let press = binding
            .defaults
            .iter()
            .find_map(|d| open.iter().find(|p| d.matches(p)).map(|p| (d, *p)));
        let (default, press) = press?;
        let mut taken = match binding.per_frame {
            PerFrame::Each => vec![press],
            PerFrame::Once => open
                .iter()
                .filter(|p| binding.matches(p))
                .copied()
                .collect(),
        };
        let mut all = self.taken();
        all.append(&mut taken);
        self.set_taken(all);
        if binding.consumes {
            // As `consume_key` with this chord: every press of the key whose
            // modifiers pass, repeats included.
            self.remove(|p| default.takes_key(p.key, p.physical, p.modifiers));
        }
        Some(press.modifiers)
    }
    /// The first of `bindings`, in their order, with a press to take, as a
    /// chain of `if`s over them: its index and the press's modifiers.
    pub(in crate::app) fn take_any(self, bindings: &[&Binding]) -> Option<(usize, Modifiers)> {
        bindings
            .iter()
            .enumerate()
            .find_map(|(i, b)| self.take(b).map(|m| (i, m)))
    }
    /// The frame's first press, in the order the keys came, that one of
    /// `bindings` matches: taken by the first binding that does. That
    /// binding's index and the press's modifiers. For a key whose modifiers
    /// pick the command (Y, Option+Y and Shift+Y), as one read of the key.
    pub(in crate::app) fn take_first(self, bindings: &[&Binding]) -> Option<(usize, Modifiers)> {
        let press = self.presses().into_iter().find_map(|p| {
            bindings
                .iter()
                .position(|b| b.matches(&p))
                .map(|index| (index, p))
        })?;
        let (index, press) = press;
        debug_assert!(
            !bindings[index].consumes && bindings[index].per_frame == PerFrame::Each,
            "take_first is for bindings that take one press and leave it in the input"
        );
        self.take_press(&press);
        Some((index, press.modifiers))
    }

    /// The presses no handler has taken yet, in the order they came, for a
    /// handler that walks them as the views' key loops do.
    pub(in crate::app) fn presses(self) -> Vec<Press> {
        let taken = self.taken();
        self.read()
            .into_iter()
            .filter(|p| !taken.contains(p))
            .collect()
    }
    /// Takes `press`, one of [`Self::presses`], for a command.
    pub(in crate::app) fn take_press(self, press: &Press) {
        let mut taken = self.taken();
        if !taken.contains(press) {
            taken.push(*press);
            self.set_taken(taken);
        }
    }
    /// Takes `press` out of egui's input, that one event alone, so no widget
    /// drawn after sees it either.
    pub(in crate::app) fn remove_press(self, press: &Press) {
        self.remove(|p| p == press);
    }

    /// Every key press in the input now, numbered among identical ones.
    fn read(self) -> Vec<Press> {
        self.ctx
            .input(|i| number(i.events.iter().filter_map(Press::of)))
    }
    fn taken(self) -> Vec<Press> {
        let pass = self.ctx.cumulative_pass_nr();
        self.ctx
            .data(|d| d.get_temp::<Taken>(taken_id()))
            .filter(|t| t.pass == pass)
            .map(|t| t.presses)
            .unwrap_or_default()
    }
    fn set_taken(self, presses: Vec<Press>) {
        let pass = self.ctx.cumulative_pass_nr();
        self.ctx
            .data_mut(|d| d.insert_temp(taken_id(), Taken { pass, presses }));
    }
    /// Takes the presses `gone` picks out of egui's input. The presses left
    /// keep whether they were taken, numbered again among identical ones.
    fn remove(self, gone: impl Fn(&Press) -> bool) {
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

/// The modifier rules many bindings share. Each reproduces a predicate the
/// handlers used before the key map, extra modifiers it tolerates included.
pub(in crate::app) mod rules {
    use eframe::egui::Modifiers;

    /// Any modifiers.
    pub(in crate::app) fn any(_: Modifiers) -> bool {
        true
    }
    /// No modifier at all.
    pub(in crate::app) fn none(m: Modifiers) -> bool {
        !m.any()
    }
    /// `consume_key(Modifiers::NONE, …)`: neither Cmd nor Ctrl; Shift and Alt
    /// are ignored.
    pub(in crate::app) fn logically_none(m: Modifiers) -> bool {
        m.matches_logically(Modifiers::NONE)
    }
    /// `consume_key(Modifiers::SHIFT, …)`.
    pub(in crate::app) fn logically_shift(m: Modifiers) -> bool {
        m.matches_logically(Modifiers::SHIFT)
    }
    /// `consume_key(Modifiers::COMMAND, …)`: Cmd, with Shift and Alt ignored.
    pub(in crate::app) fn logically_command(m: Modifiers) -> bool {
        m.matches_logically(Modifiers::COMMAND)
    }
    /// `consume_key(Modifiers::COMMAND | Modifiers::SHIFT, …)`.
    pub(in crate::app) fn logically_command_shift(m: Modifiers) -> bool {
        m.matches_logically(Modifiers::COMMAND | Modifiers::SHIFT)
    }
    /// Cmd, whatever else.
    pub(in crate::app) fn command(m: Modifiers) -> bool {
        m.command
    }
    /// Cmd and Shift, whatever else.
    pub(in crate::app) fn command_shift(m: Modifiers) -> bool {
        m.command && m.shift
    }
    /// Cmd, Option and Shift: Lightroom's Develop chords.
    pub(in crate::app) fn command_alt_shift(m: Modifiers) -> bool {
        m.command && m.alt && m.shift
    }
    /// Neither Cmd, Ctrl nor Alt: G, E and D, with Shift tolerated.
    pub(in crate::app) fn plain(m: Modifiers) -> bool {
        !(m.command || m.ctrl || m.alt)
    }
    /// Neither Cmd nor Alt: a tool's keys, Shift being their variant.
    pub(in crate::app) fn no_command_alt(m: Modifiers) -> bool {
        !m.command && !m.alt
    }
    /// Neither Cmd nor Alt nor Shift.
    pub(in crate::app) fn no_command_alt_shift(m: Modifiers) -> bool {
        !m.command && !m.alt && !m.shift
    }
    /// Without Cmd: the Library views' keys, Shift extending the selection.
    pub(in crate::app) fn no_command(m: Modifiers) -> bool {
        !m.command
    }
    /// The rating, flag and label keys: none of Cmd, Ctrl and Alt, Shift
    /// going on to the next photo.
    pub(in crate::app) fn metadata(m: Modifiers) -> bool {
        !(m.command || m.ctrl || m.alt || m.mac_cmd)
    }
}
