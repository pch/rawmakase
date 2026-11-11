//! The control socket's legacy `cc` and `note` commands, a core compatibility
//! adapter apart from MIDI devices. It keeps a frozen copy of the Loupedeck+
//! table those commands have always used, its own held modifiers and, through
//! [`Hub::photo_turn`], its own photo dial, so it works whether or not any MIDI
//! device is configured, and no device's mapping or connection changes it.
use super::*;
use crate::model::params::ParameterId;

const PHOTO_DIAL: u8 = 48;
const PHOTO_DETENT: i32 = 2;

#[derive(Default)]
pub(super) struct Legacy {
    pub held: Modifiers,
}
impl Legacy {
    /// The operation a legacy command stands for. A control the table doesn't
    /// bind is refused, so a script learns it sent the wrong number.
    pub(crate) fn translate(&mut self, input: Input) -> commands::Result<Option<DeviceOp>> {
        match input {
            // Every Loupedeck+ dial sends relative ticks as two's complement.
            Input::Cc(cc, value) => {
                let ticks = if value < 64 {
                    i32::from(value)
                } else {
                    i32::from(value) - 128
                };
                if cc == PHOTO_DIAL {
                    return Ok(Some(DeviceOp::Photo {
                        ticks,
                        detent: PHOTO_DETENT,
                    }));
                }
                let param = dial(cc).ok_or_else(|| {
                    commands::Error::new("unmapped_control", "No action is mapped to this CC")
                })?;
                Ok(Some(DeviceOp::Adjust(param, ticks)))
            }
            Input::Note(note, down) => {
                let action = button(note).ok_or_else(|| {
                    commands::Error::new("unmapped_control", "No action is mapped to this note")
                })?;
                Ok(press(&mut self.held, action, down))
            }
        }
    }
}

/// What a dial turns: the Loupedeck+ dials as its default Lightroom profile
/// maps them, and faders P1–P8 the Color Mixer's bands.
fn dial(cc: u8) -> Option<Param> {
    use ParameterId as P;
    Some(match cc {
        17..=24 => Param::Band(usize::from(cc - 17)),
        33 => Param::Setting(P::Exposure),
        34 => Param::Setting(P::Blacks),
        35 => Param::Setting(P::Whites),
        36 => Param::Setting(P::Saturation),
        37 => Param::Setting(P::Vibrance),
        38 => Param::Setting(P::Temperature),
        39 => Param::Setting(P::Tint),
        40 => Param::Setting(P::Highlights),
        44 => Param::Setting(P::Shadows),
        45 => Param::Setting(P::Clarity),
        46 => Param::Setting(P::Contrast),
        _ => return None,
    })
}

/// What a button does, as on the Loupedeck+ with its default profile.
fn button(note: u8) -> Option<Action> {
    let key = |key| Action::Key(key, Modifiers::NONE);
    Some(match note {
        66 => Action::Hold(SHIFT),
        67 => Action::Hold(Modifiers::CTRL),
        68 => Action::Hold(command()),
        69 => Action::Hold(Modifiers::ALT),
        76 => key(Key::ArrowUp),
        77 => key(Key::ArrowDown),
        78 => key(Key::ArrowLeft),
        79 => key(Key::ArrowRight),
        // P1-P5 rate 1-5 stars, P6 clears the rating, P7 picks and P8 rejects.
        80 => key(Key::Num1),
        81 => key(Key::Num2),
        82 => key(Key::Num3),
        83 => key(Key::Num4),
        84 => key(Key::Num5),
        85 => key(Key::Num0),
        86 => key(Key::P),
        87 => key(Key::X),
        88 => Action::Key(Key::E, command() | SHIFT),
        92 => Action::Key(Key::C, command() | SHIFT),
        93 => Action::Key(Key::V, command() | SHIFT),
        95 => Action::Key(Key::Z, command()),
        96 => Action::Key(Key::Z, command() | SHIFT),
        // C1 toggles zoom; C3-C6 are the colour labels red, yellow, green and blue.
        49 => key(Key::Z),
        51 => key(Key::Num6),
        52 => key(Key::Num7),
        53 => key(Key::Num8),
        54 => key(Key::Num9),
        // Screen Mode shows the clipping overlay.
        97 => key(Key::J),
        101 => Action::ToggleMono,
        98 => Action::Mixer(0),
        99 => Action::Mixer(1),
        100 => Action::Mixer(2),
        102 => key(Key::Backslash),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    /// The frozen table is the Loupedeck+ profile's as this adapter was split
    /// from it: the MIDI extension's profile, read the way the app reads what
    /// that extension sends. The profile may change later, this table may not.
    #[test]
    fn the_frozen_table_is_the_loupedeck_profile() {
        let profile = rawmakase_midi::profiles::Profile::Loupedeck.mapping();
        assert_eq!(profile.photo_dial, Some(PHOTO_DIAL));
        assert_eq!(profile.photo_detent, PHOTO_DETENT);
        assert_eq!(
            profile.default_encoder,
            rawmakase_midi::config::Encoder::TwosComplement
        );
        assert!(profile.encoders.is_empty());
        assert_eq!(profile.sensitivity, 1);
        for n in 0..=127 {
            let profile_dial = profile.dials.get(&n).map(|p| Param::parse(p).unwrap());
            assert_eq!(dial(n), profile_dial, "CC {n}");
            let profile_button = profile.buttons.get(&n).map(|s| parse_action(s).unwrap());
            assert_eq!(button(n), profile_button, "note {n}");
        }
    }
}
