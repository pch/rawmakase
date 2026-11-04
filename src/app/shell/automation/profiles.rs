//! Device-type presets. Adding a profile does not change the queue or editor.
use super::*;
use crate::model::params::ParameterId;
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Profile {
    Loupedeck,
    Custom,
}
impl Profile {
    pub(crate) const ALL: [Self; 2] = [Self::Loupedeck, Self::Custom];
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Loupedeck => "Loupedeck+",
            Self::Custom => "Custom MIDI device",
        }
    }
    pub(crate) fn mapping(self) -> Config {
        match self {
            Self::Loupedeck => Config::loupedeck(),
            Self::Custom => Config {
                dials: HashMap::new(),
                buttons: HashMap::new(),
                photo_dial: None,
                photo_detent: 1,
                default_encoder: Encoder::Absolute,
                encoders: HashMap::new(),
                sensitivity: 1,
            },
        }
    }
    pub(crate) fn dials(self) -> &'static [(&'static str, u8)] {
        if self == Self::Loupedeck { &DIALS } else { &[] }
    }
    pub(crate) fn buttons(self) -> &'static [(&'static str, u8)] {
        if self == Self::Loupedeck {
            &BUTTONS
        } else {
            &[]
        }
    }
}
impl Config {
    /// The Loupedeck+ layout, as mapped with its default Lightroom profile.
    pub(super) fn loupedeck() -> Self {
        let faders = (0..8).map(|i| (17 + i as u8, Param::Band(i)));
        let dials = [
            (33, Param::Setting(ParameterId::Exposure)),
            (34, Param::Setting(ParameterId::Blacks)),
            (35, Param::Setting(ParameterId::Whites)),
            (36, Param::Setting(ParameterId::Saturation)),
            (37, Param::Setting(ParameterId::Vibrance)),
            (38, Param::Setting(ParameterId::Temperature)),
            (39, Param::Setting(ParameterId::Tint)),
            (40, Param::Setting(ParameterId::Highlights)),
            (44, Param::Setting(ParameterId::Shadows)),
            (45, Param::Setting(ParameterId::Clarity)),
            (46, Param::Setting(ParameterId::Contrast)),
        ]
        .into_iter()
        .chain(faders);
        let buttons = [
            (66, Action::Hold(SHIFT)),
            (67, Action::Hold(Modifiers::CTRL)),
            (68, Action::Hold(command())),
            (69, Action::Hold(Modifiers::ALT)),
            (76, Action::Key(Key::ArrowUp, Modifiers::NONE)),
            (77, Action::Key(Key::ArrowDown, Modifiers::NONE)),
            (78, Action::Key(Key::ArrowLeft, Modifiers::NONE)),
            (79, Action::Key(Key::ArrowRight, Modifiers::NONE)),
            // P1-P5 rate 1-5 stars, P6 clears the rating, P7 picks and P8 rejects,
            // as the keys do.
            (80, Action::Key(Key::Num1, Modifiers::NONE)),
            (81, Action::Key(Key::Num2, Modifiers::NONE)),
            (82, Action::Key(Key::Num3, Modifiers::NONE)),
            (83, Action::Key(Key::Num4, Modifiers::NONE)),
            (84, Action::Key(Key::Num5, Modifiers::NONE)),
            (85, Action::Key(Key::Num0, Modifiers::NONE)),
            (86, Action::Key(Key::P, Modifiers::NONE)),
            (87, Action::Key(Key::X, Modifiers::NONE)),
            (88, Action::Key(Key::E, command() | SHIFT)),
            (92, Action::Key(Key::C, command() | SHIFT)),
            (93, Action::Key(Key::V, command() | SHIFT)),
            (95, Action::Key(Key::Z, command())),
            (96, Action::Key(Key::Z, command() | SHIFT)),
            // C1 toggles zoom, as Z does.
            (49, Action::Key(Key::Z, Modifiers::NONE)),
            // C3-C6 are the colour labels red, yellow, green and blue.
            (51, Action::Key(Key::Num6, Modifiers::NONE)),
            (52, Action::Key(Key::Num7, Modifiers::NONE)),
            (53, Action::Key(Key::Num8, Modifiers::NONE)),
            (54, Action::Key(Key::Num9, Modifiers::NONE)),
            // Screen Mode shows the clipping overlay.
            (97, Action::Key(Key::J, Modifiers::NONE)),
            (101, Action::ToggleMono),
            (98, Action::Mixer(0)),
            (99, Action::Mixer(1)),
            (100, Action::Mixer(2)),
            (102, Action::Key(Key::Backslash, Modifiers::NONE)),
        ];
        Self {
            photo_dial: Some(48),
            photo_detent: 2,
            default_encoder: Encoder::TwosComplement,
            encoders: HashMap::new(),
            sensitivity: 1,
            dials: dials.collect(),
            buttons: buttons.into_iter().collect(),
        }
    }
}
/// The device's dials and faders: name and CC (tools/loupedeck/controls.json).
pub(in crate::app::shell::automation) const DIALS: [(&str, u8); 22] = [
    ("Fader P1", 17),
    ("Fader P2", 18),
    ("Fader P3", 19),
    ("Fader P4", 20),
    ("Fader P5", 21),
    ("Fader P6", 22),
    ("Fader P7", 23),
    ("Fader P8", 24),
    ("Exposure", 33),
    ("Blacks", 34),
    ("Whites", 35),
    ("Saturation", 36),
    ("Vibrance", 37),
    ("Temperature", 38),
    ("Tint", 39),
    ("Highlights", 40),
    ("D1", 41),
    ("D2", 42),
    ("Shadows", 44),
    ("Clarity", 45),
    ("Contrast", 46),
    ("Control Dial", 48),
];

/// The device's buttons: name and note.
pub(in crate::app::shell::automation) const BUTTONS: [(&str, u8); 40] = [
    ("C1", 49),
    ("C2", 50),
    ("C3", 51),
    ("C4", 52),
    ("C5", 53),
    ("C6", 54),
    ("Col", 65),
    ("Shift", 66),
    ("Ctrl", 67),
    ("Command", 68),
    ("Alt", 69),
    ("Tab", 70),
    ("Up Arrow", 76),
    ("Bottom Arrow", 77),
    ("Left Arrow", 78),
    ("Right Arrow", 79),
    ("P1", 80),
    ("P2", 81),
    ("P3", 82),
    ("P4", 83),
    ("P5", 84),
    ("P6", 85),
    ("P7", 86),
    ("P8", 87),
    ("Export", 88),
    ("Copy", 92),
    ("Paste", 93),
    ("Undo", 95),
    ("Redo", 96),
    ("Screen Mode", 97),
    ("Hue", 98),
    ("Sat", 99),
    ("Lum", 100),
    ("Clr/BW", 101),
    ("Before After", 102),
    ("Fn", 110),
    ("L1", 114),
    ("L2", 115),
    ("L3", 116),
    ("Custom mode", 117),
];
