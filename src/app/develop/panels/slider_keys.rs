//! Slider Keys, a mode of Develop as LrSuperKeys is for Lightroom: hold a
//! slider's letter (E for Exposure) and press the arrows or turn the wheel to
//! move it, Shift by more and Option by less. Up and Right raise it. Like a
//! slider's drag, the change is one History step once the letter is let go.
//! With a mask selected in Masking, the mask's slider moves instead.
//!
//! Each slider's key is a command of the key map (`slider_keys:exposure` and
//! the rest, in the Slider Keys context), so Preferences > Shortcuts changes
//! or removes it. By default they are letters by their place on the keyboard,
//! as the Basic panel's keys are. While the mode is on, a press of one is its
//! own in Develop: it runs no command, and alone it changes nothing. A press
//! with other modifiers (Shift+W, Cmd+E) is the command's; so is a key of the
//! open tool (Crop's X and A). It is off at each start.
//!
//! The keys and the wheel are read in `raw_input_hook`, before egui
//! ([`Editor::slider_key_input`]), and matched with the key map's chords
//! ([`FrameKeys::has_chord`]); what is taken never reaches egui. That relies on
//! two things of egui, to check on an upgrade: the hook runs before egui reads
//! the frame's input, and the raw events come in order, each key's release
//! among them. Repeats are worked out here from the keys down by their place,
//! not from egui's own record, which goes by the key typed.
use crate::app::Editor;
use crate::app::develop::panels::basic_keys;
use crate::app::develop::state::Tool;
use crate::app::shell::commands::Param;
use crate::app::shell::commands::registry::bindings;
use crate::app::shell::keymap::{Binding, Context, FrameKeys, Mods, Press};
use crate::model::params::{KeyStep, LocalParameterId, ParameterId};
use eframe::egui::{self, Key};

/// Each slider's key, in the key map. By default a letter of its name: Temp and
/// Tint by their color, Texture by its X, Clarity by its L and Saturation by its
/// A, since S is Shadows.
pub(in crate::app) static KEYS: [(&Binding, ParameterId); 13] = [
    (&bindings::SLIDER_EXPOSURE, ParameterId::Exposure),
    (&bindings::SLIDER_CONTRAST, ParameterId::Contrast),
    (&bindings::SLIDER_HIGHLIGHTS, ParameterId::Highlights),
    (&bindings::SLIDER_SHADOWS, ParameterId::Shadows),
    (&bindings::SLIDER_WHITES, ParameterId::Whites),
    (&bindings::SLIDER_BLACKS, ParameterId::Blacks),
    (&bindings::SLIDER_TEMP, ParameterId::Temperature),
    (&bindings::SLIDER_TINT, ParameterId::Tint),
    (&bindings::SLIDER_TEXTURE, ParameterId::Texture),
    (&bindings::SLIDER_CLARITY, ParameterId::Clarity),
    (&bindings::SLIDER_DEHAZE, ParameterId::Dehaze),
    (&bindings::SLIDER_VIBRANCE, ParameterId::Vibrance),
    (&bindings::SLIDER_SATURATION, ParameterId::Saturation),
];

/// A trackpad's scroll, in points, for one step; a mouse wheel's notch is one.
const WHEEL_POINTS: f32 = 12.;

#[derive(Default)]
pub(in crate::app) struct SliderKeys {
    pub(in crate::app) on: bool,
    /// The slider whose key is held, named as on a RAW file.
    held: Option<Held>,
    /// Counts the letters taken up: each hold is a History step of its own.
    hold: u32,
    /// The places of the keys down, from their press to their release: a
    /// press of one of them is its repeat.
    down: Vec<Key>,
    /// The steps the arrows and wheel asked for, in order: what they move, the
    /// step's size, and up. A step waits for the next frame when another
    /// target's step is ahead of it.
    steps: Vec<(Target, KeyStep, bool)>,
    /// This frame's steps wait for the next: another key or the pointer was
    /// pressed with them, an edit of its own.
    wait: bool,
    /// Another key or the pointer was pressed this frame: the step under way ends.
    interrupted: bool,
    /// What the History step under way moves, once a step has moved it.
    moving: Option<Target>,
    /// A trackpad's scroll short of a step, in steps.
    scrolled: f32,
    /// The slider's name and value, shown over the photo while it moves.
    shown: Option<(String, String)>,
}

/// A slider of a photo, or of one of its masks, while one letter is held:
/// what one History step moves.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Target {
    id: ParameterId,
    photo: Option<crate::catalog::PhotoId>,
    mask: Option<usize>,
    hold: u32,
}

impl SliderKeys {
    /// Keeps [`Self::down`] up to date with `event`; whether it is a repeat.
    fn track(&mut self, event: &egui::Event) -> bool {
        match event {
            egui::Event::Key {
                key,
                physical_key,
                pressed,
                ..
            } => {
                let at = place(*key, *physical_key);
                let repeat = self.down.contains(&at);
                if !pressed {
                    self.down.retain(|k| *k != at);
                } else if !repeat {
                    self.down.push(at);
                }
                repeat && *pressed
            }
            egui::Event::WindowFocused(false) => {
                self.down.clear();
                false
            }
            _ => false,
        }
    }
    /// Whether a held letter has its History step under way.
    pub(in crate::app) fn held(&self) -> bool {
        self.held.is_some() && self.moving.is_some()
    }
    /// The slider a held letter is moving, to light in its panel.
    pub(in crate::app) fn moved(&self) -> Option<ParameterId> {
        self.moving.map(|t| t.id)
    }
}

/// A slider's key held down.
#[derive(Clone, Copy)]
struct Held {
    id: ParameterId,
    /// Where it is on the keyboard, or the key when that is unknown.
    place: Key,
    /// Taken up with Cmd or Ctrl: its arrows and wheel come with them too.
    chord: bool,
}

/// Where a key is on the keyboard, or the key typed when that is unknown.
fn place(key: Key, physical: Option<Key>) -> Key {
    physical.unwrap_or(key)
}

/// The mask slider of a Basic slider: a mask has no Vibrance.
pub(in crate::app) fn local(id: ParameterId) -> Option<LocalParameterId> {
    use LocalParameterId as L;
    Some(match id {
        ParameterId::Exposure => L::Exposure,
        ParameterId::Contrast => L::Contrast,
        ParameterId::Highlights => L::Highlights,
        ParameterId::Shadows => L::Shadows,
        ParameterId::Whites => L::Whites,
        ParameterId::Blacks => L::Blacks,
        ParameterId::Temperature => L::Temperature,
        ParameterId::Tint => L::Tint,
        ParameterId::Texture => L::Texture,
        ParameterId::Clarity => L::Clarity,
        ParameterId::Dehaze => L::Dehaze,
        ParameterId::Saturation => L::Saturation,
        _ => return None,
    })
}

fn size(modifiers: egui::Modifiers) -> KeyStep {
    if modifiers.alt {
        KeyStep::Fine
    } else if modifiers.shift {
        KeyStep::Large
    } else {
        KeyStep::Normal
    }
}

/// A mask slider's steps, as the slider shows it: Exposure's are the photo's,
/// the rest go by one, five and twenty of their hundred.
fn mask_steps(id: ParameterId) -> [f32; 3] {
    match id {
        ParameterId::Exposure => id.key_steps().unwrap_or([0.02, 0.1, 0.33]),
        _ => [1., 5., 20.],
    }
}

impl Editor {
    /// What a step of `id` in the hold `hold` moves now: the selected mask's
    /// slider in Masking.
    fn slider_target(&self, id: ParameterId, hold: u32) -> Target {
        Target {
            id,
            hold,
            photo: self.document.catalog_photo,
            mask: self
                .view
                .masking
                .selected
                .filter(|_| self.view.is(Tool::Mask)),
        }
    }

    /// Takes the slider keys, and the arrows and wheel while one is held, from
    /// the raw input in the order they came, before egui reads it: no command,
    /// view or slider sees what is taken, and the wheel is taken before egui
    /// smooths it. A key is held by its place on the keyboard, from its press
    /// to its release; another press of that place is its repeat. The steps
    /// wait for the end of the frame: [`Self::slider_key_steps`].
    pub(in crate::app) fn slider_key_input(
        &mut self,
        ctx: &egui::Context,
        raw: &mut egui::RawInput,
    ) {
        let ready = self.view.slider_keys.on
            && self.module == crate::app::Module::Develop
            && self.basic_keys_ready(ctx);
        let tool = self.key_tool();
        let photo = self.document.catalog_photo;
        let mask = self.slider_target(ParameterId::Exposure, 0).mask;
        let keys = &mut self.view.slider_keys;
        if !ready {
            for event in &raw.events {
                keys.track(event);
            }
            keys.held = None;
            keys.scrolled = 0.;
            keys.steps.clear();
            keys.wait = false;
            return;
        }
        let frame = FrameKeys::before_frame(ctx, self.keymap.effective());
        // A key of the open tool is the tool's.
        let tools_key = |press: &Press| {
            tool.is_some_and(|tool| {
                bindings::ALL
                    .iter()
                    .filter(|b| b.context == Context::Tool(tool))
                    .any(|b| frame.has_chord(press, b, Mods::NONE))
            })
        };
        // Another key or a pointer press with the frame's steps: an edit of its own.
        let mut others = false;
        raw.events.retain(|event| {
            let repeat = keys.track(event);
            let target = |id| Target {
                id,
                photo,
                mask,
                hold: keys.hold,
            };
            match event {
                // The window lost the keys: none is held any more.
                egui::Event::WindowFocused(false) => {
                    keys.held = None;
                    keys.scrolled = 0.;
                    return true;
                }
                egui::Event::PointerButton { pressed: true, .. } => {
                    others = true;
                    return true;
                }
                egui::Event::Key {
                    key,
                    physical_key,
                    pressed: false,
                    ..
                } => {
                    if keys
                        .held
                        .is_none_or(|h| h.place != place(*key, *physical_key))
                    {
                        return true;
                    }
                    keys.held = None;
                    keys.scrolled = 0.;
                    return false;
                }
                egui::Event::MouseWheel {
                    unit,
                    delta,
                    modifiers,
                    ..
                } => {
                    // With Cmd or Ctrl the wheel keeps its own use, unless the key
                    // was taken up with them.
                    let cmd = modifiers.command || modifiers.ctrl;
                    let Some(id) = keys.held.filter(|h| !cmd || h.chord).map(|h| h.id) else {
                        return true;
                    };
                    // Shift turns a mouse wheel sideways on macOS.
                    let d = if delta.y == 0. { delta.x } else { delta.y };
                    keys.scrolled += match unit {
                        egui::MouseWheelUnit::Point => d / WHEEL_POINTS,
                        _ => d,
                    };
                    let whole = keys.scrolled.trunc();
                    keys.scrolled -= whole;
                    for _ in 0..whole.abs() as usize {
                        keys.steps.push((target(id), size(*modifiers), whole > 0.));
                    }
                    return false;
                }
                _ => {}
            }
            let Some(press) = Press::of(event) else {
                return true;
            };
            let chord = press.modifiers.command || press.modifiers.ctrl;
            let at = place(press.key, press.physical);
            let holding = keys.held.is_some_and(|h| h.place == at);
            let up = match press.key {
                Key::ArrowUp | Key::ArrowRight => Some(true),
                Key::ArrowDown | Key::ArrowLeft => Some(false),
                _ => None,
            };
            let tools = tools_key(&press);
            // While a key is held the arrows step its slider, though one is
            // another slider's key, unless it is the open tool's or the held key
            // itself; with Cmd or Ctrl only when it was taken up so.
            if let Some(h) = keys.held
                && let Some(up) = up
                && !holding
                && !tools
                && (!chord || h.chord)
            {
                keys.steps.push((target(h.id), size(press.modifiers), up));
                return false;
            }
            // The held key's repeats are the mode's, but for the open tool's key
            // (Crop's Shift+X), which is the tool's: egui never saw the press, so
            // it would take a repeat with Cmd or Ctrl as a command's first press.
            // With Cmd or Ctrl pressed since, one ends the step under way and
            // moves nothing.
            if holding {
                if tools {
                    others = true;
                    return true;
                }
                if chord && !keys.held.is_some_and(|h| h.chord) {
                    others = true;
                }
                return false;
            }
            // Two sliders on one key are a conflict the key map reports: neither.
            let mut sliders = KEYS
                .iter()
                .filter(|(b, _)| !tools && frame.has_chord(&press, b, Mods::NONE))
                .map(|(_, id)| *id);
            let slider = sliders.next().filter(|_| sliders.next().is_none());
            // A repeat of a key pressed for something else (Cmd+E, Cmd let go)
            // takes up no slider.
            if let Some(id) = slider.filter(|_| !repeat) {
                // A press of a slider's key takes it up: each hold its own step.
                keys.scrolled = 0.;
                keys.hold = keys.hold.wrapping_add(1);
                keys.held = Some(Held {
                    id,
                    place: at,
                    chord,
                });
                return false;
            }
            if !crate::app::shell::keymap::is_modifier(press.key) {
                others = true;
            }
            true
        });
        keys.wait = others && !keys.steps.is_empty();
        keys.interrupted = others;
    }

    /// Ends the step under way once its key is let go, what it moves is no
    /// longer what a step would move (another mask, Masking closed, another
    /// photo), or another key or a pointer press came with it.
    pub(in crate::app) fn slider_keys(&mut self) {
        let keys = &mut self.view.slider_keys;
        let interrupted = std::mem::take(&mut keys.interrupted);
        let Some(moving) = keys.moving else {
            self.show_held_slider();
            return;
        };
        let ended = interrupted
            || match keys.steps.first() {
                Some(next) => next.0 != moving,
                None => {
                    keys.held.is_none_or(|h| h.id != moving.id)
                        || keys.hold != moving.hold
                        || self.slider_target(moving.id, moving.hold) != moving
                }
            };
        if ended {
            self.end_slider_step();
            self.show_held_slider();
        }
    }

    /// The held slider's name and value over the photo from its key's press,
    /// before it moves; gone once the key is let go.
    fn show_held_slider(&mut self) {
        let keys = &self.view.slider_keys;
        if keys.moving.is_some() {
            return;
        }
        let shown = keys
            .held
            .and_then(|h| self.slider_shown(self.slider_target(h.id, keys.hold)));
        self.view.slider_keys.shown = shown;
    }

    /// `target`'s name and value as the label shows them.
    fn slider_shown(&self, target: Target) -> Option<(String, String)> {
        let id = target.id;
        let photo = self.document.metadata.as_ref();
        if !basic_keys::enabled(id, photo) {
            return None;
        }
        let recipe = self.document.edit.recipe();
        if let Some(index) = target.mask {
            let mask = recipe.masks.get(index)?;
            let mut adjust = mask.adjust;
            // A mask has no Vibrance.
            let text = Param::Setting(id).local_set(&mut adjust, None, 0).ok()?;
            let name = crate::app::develop::tools::mask_tool::mask_name(mask, index);
            return Some((format!("{name} {}", id.descriptor().label), text));
        }
        let id = id.for_photo(photo);
        let value = *id.value_mut(&mut recipe.clone());
        Some((id.descriptor().label.to_owned(), id.text(value)))
    }

    /// Ends the History step Slider Keys has under way.
    fn end_slider_step(&mut self) {
        self.finish_gesture();
        let keys = &mut self.view.slider_keys;
        keys.moving = None;
        keys.shown = None;
    }

    /// Moves the sliders the steps [`Self::slider_keys`] took, last in the frame's
    /// edit, on the photo the frame ends on: one History step while the letter
    /// is held. Only the first slider's steps: another's wait for the next frame,
    /// which ends this step first.
    pub(in crate::app) fn slider_key_steps(&mut self, ctx: &egui::Context) {
        let keys = &mut self.view.slider_keys;
        let Some(&(target, ..)) = keys.steps.first() else {
            return;
        };
        // Read before the frame: a window opened or a close asked for in it
        // since leaves the photo alone.
        if !self.basic_keys_ready(ctx) {
            self.view.slider_keys.steps.clear();
            return;
        }
        let keys = &mut self.view.slider_keys;
        // While a pointer button is down, a drag may be under way: its own step.
        if std::mem::take(&mut keys.wait) || ctx.input(|i| i.pointer.any_down()) {
            ctx.request_repaint();
            return;
        }
        let ahead = keys.steps.iter().take_while(|s| s.0 == target).count();
        // The photo, the mask or History moved on in this frame: the steps were
        // for what was there.
        if self.slider_target(target.id, target.hold) != target
            || self.document.edit.history().is_replaying()
        {
            self.view.slider_keys.steps.drain(..ahead);
            ctx.request_repaint();
            return;
        }
        if self.view.slider_keys.moving.is_some_and(|m| m != target) {
            // Another slider, mask or hold: its own step, from the next frame.
            ctx.request_repaint();
            return;
        }
        let keys = &mut self.view.slider_keys;
        let steps: Vec<(KeyStep, bool)> = keys
            .steps
            .drain(..ahead)
            .map(|(_, size, up)| (size, up))
            .collect();
        if !keys.steps.is_empty() {
            ctx.request_repaint();
        }
        let id = target.id;
        let photo = self.document.metadata.clone();
        if !basic_keys::enabled(id, photo.as_ref()) {
            return;
        }
        let shown = if let Some(index) = target.mask {
            let param = Param::Setting(id);
            let Some(mask) = self.document.edit.recipe().masks.get(index) else {
                return;
            };
            let label = format!(
                "{} {}",
                crate::app::develop::tools::mask_tool::mask_name(mask, index),
                id.descriptor().label
            );
            let sizes = mask_steps(id);
            let mut adjust = mask.adjust;
            // A mask has no Vibrance.
            let Ok(mut text) = param.local_set(&mut adjust, None, 0) else {
                return;
            };
            // Each step within the slider's ends, as the photo's are.
            for (size, up) in &steps {
                let step = match size {
                    KeyStep::Fine => sizes[0],
                    KeyStep::Normal => sizes[1],
                    KeyStep::Large => sizes[2],
                };
                let value = param.local_shown(&adjust).unwrap_or_default();
                let value = value + if *up { step } else { -step };
                if let Ok(t) = param.local_set(&mut adjust, Some(value), 0) {
                    text = t;
                }
            }
            if adjust != mask.adjust {
                self.document.edit.recipe_mut().masks[index].adjust = adjust;
                self.step_name.name(label.clone(), text.clone());
                // As a mask slider's drag does, the change hides the red overlay.
                self.view.masking.adjusted();
            }
            (label, text)
        } else {
            let id = id.for_photo(photo.as_ref());
            let mut recipe = self.document.edit.recipe().clone();
            let before = *id.value_mut(&mut recipe);
            crate::model::edit::change_setting(&mut recipe, id, photo.as_ref(), |r| {
                let value = id.value_mut(r);
                for (size, up) in &steps {
                    *value = id.key_stepped(*value, *size, *up);
                }
            });
            let value = *id.value_mut(&mut recipe);
            let label = id.descriptor().label.to_owned();
            let text = id.text(value);
            if value != before {
                *self.document.edit.recipe_mut() = recipe;
                self.step_name.name(label.clone(), text.clone());
            }
            (label, text)
        };
        let keys = &mut self.view.slider_keys;
        keys.moving = Some(target);
        keys.shown = Some(shown);
        ctx.request_repaint();
    }

    /// The held slider's name and value, over the top of the photo.
    pub(in crate::app) fn slider_keys_label(&self, ctx: &egui::Context) {
        let Some((label, value)) = &self.view.slider_keys.shown else {
            return;
        };
        let top = self
            .view
            .shown_rect
            .map_or(ctx.content_rect().center_top(), |r| r.center_top());
        egui::Area::new(egui::Id::new("slider_keys"))
            .order(egui::Order::Foreground)
            .interactable(false)
            .pivot(egui::Align2::CENTER_TOP)
            .fixed_pos(top + egui::vec2(0., 16.))
            .show(ctx, |ui| {
                // Flat, as the rest of the photo's overlays.
                egui::Frame::popup(ui.style())
                    .shadow(egui::Shadow::NONE)
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!("{label}  {value}")).strong(),
                            )
                            .extend(),
                        );
                    });
            });
    }
}

#[cfg(test)]
#[path = "slider_keys_tests.rs"]
mod tests;
