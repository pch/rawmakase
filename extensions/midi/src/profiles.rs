//! Device-type presets: a device's starting mapping and the controls its page
//! lists by name. Adding a profile changes nothing else.
use crate::config::{Encoder, Mapping};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    Loupedeck,
    Custom,
}

impl Profile {
    pub(crate) const ALL: [Self; 2] = [Self::Loupedeck, Self::Custom];
    pub fn label(self) -> &'static str {
        match self {
            Self::Loupedeck => "Loupedeck+",
            Self::Custom => "Custom MIDI device",
        }
    }
    /// Its id in `midi.json`.
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Loupedeck => "loupedeck",
            Self::Custom => "custom",
        }
    }
    /// The mapping a new device of this profile starts with.
    pub fn mapping(self) -> Mapping {
        match self {
            Self::Loupedeck => loupedeck(),
            Self::Custom => Mapping {
                dials: BTreeMap::new(),
                buttons: BTreeMap::new(),
                photo_dial: None,
                photo_detent: 1,
                default_encoder: Encoder::Absolute,
                encoders: BTreeMap::new(),
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

/// The Loupedeck+ layout, as mapped with its default Lightroom profile, in
/// the notation the core wrote to `midi.json`. The control socket's legacy
/// `cc` and `note` commands keep a frozen copy of it in the core; a test
/// there compares the two.
fn loupedeck() -> Mapping {
    let faders = (0..8u8).map(|i| (17 + i, format!("band{}", i + 1)));
    let dials = [
        (33, "exposure"),
        (34, "blacks"),
        (35, "whites"),
        (36, "saturation"),
        (37, "vibrance"),
        (38, "temperature"),
        (39, "tint"),
        (40, "highlights"),
        (44, "shadows"),
        (45, "clarity"),
        (46, "contrast"),
    ]
    .map(|(cc, p)| (cc, p.to_string()))
    .into_iter()
    .chain(faders);
    let buttons = [
        (66, "hold:shift"),
        (67, "hold:control-key"),
        (68, "hold:cmd"),
        (69, "hold:alt"),
        (76, "Up"),
        (77, "Down"),
        (78, "Left"),
        (79, "Right"),
        // P1-P5 rate 1-5 stars, P6 clears the rating, P7 picks and P8 rejects,
        // as the keys do.
        (80, "1"),
        (81, "2"),
        (82, "3"),
        (83, "4"),
        (84, "5"),
        (85, "0"),
        (86, "P"),
        (87, "X"),
        (88, "cmd+shift+E"),
        (92, "cmd+shift+C"),
        (93, "cmd+shift+V"),
        (95, "cmd+Z"),
        (96, "cmd+shift+Z"),
        // C1 toggles zoom, as Z does.
        (49, "Z"),
        // C3-C6 are the colour labels red, yellow, green and blue.
        (51, "6"),
        (52, "7"),
        (53, "8"),
        (54, "9"),
        // Screen Mode shows the clipping overlay.
        (97, "J"),
        (101, "toggle:bw"),
        (98, "mixer:hue"),
        (99, "mixer:sat"),
        (100, "mixer:lum"),
        (102, "Backslash"),
    ];
    Mapping {
        photo_dial: Some(48),
        photo_detent: 2,
        default_encoder: Encoder::TwosComplement,
        encoders: BTreeMap::new(),
        sensitivity: 1,
        dials: dials.collect(),
        buttons: buttons
            .into_iter()
            .map(|(n, a)| (n, a.to_string()))
            .collect(),
    }
}

/// The Loupedeck+'s dials and faders: name and CC (tools/loupedeck/controls.json).
const DIALS: [(&str, u8); 22] = [
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

/// The Loupedeck+'s buttons: name and note.
const BUTTONS: [(&str, u8); 40] = [
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The page lists what the device sends; the defaults must be among it.
    #[test]
    fn every_default_control_is_listed_for_the_page() {
        let mapping = Profile::Loupedeck.mapping();
        for cc in mapping.dials.keys().chain(mapping.photo_dial.iter()) {
            assert!(DIALS.iter().any(|(_, n)| n == cc), "CC {cc}");
        }
        for note in mapping.buttons.keys() {
            assert!(BUTTONS.iter().any(|(_, n)| n == note), "note {note}");
        }
    }

    #[test]
    fn profile_ids_are_their_names_in_the_file() {
        for profile in Profile::ALL {
            assert_eq!(serde_json::to_value(profile).unwrap(), profile.id());
        }
    }
}
