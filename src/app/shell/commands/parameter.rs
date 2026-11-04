//! Parameter names and units shared by application command adapters.
use crate::model::params::ParameterId::*;
use crate::model::params::{format_value, nudged};
use crate::model::recipe::Recipe;
use crate::{app::develop::panels::inspector::BANDS, model::params::ParameterId};

/// The Color Mixer's channels, in the order of `Recipe::hsl`.
const MIXER_CHANNELS: [&str; 3] = ["Hue", "Saturation", "Luminance"];

/// A slider a dial can turn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) enum Param {
    /// A setting with a descriptor (see `model::params`).
    Setting(ParameterId),
    /// A Color Mixer colour band (0 Red .. 7 Magenta), on the channel the
    /// panel's Hue / Sat / Lum selector shows.
    Band(usize),
    /// One channel (0 Hue, 1 Saturation, 2 Luminance) of a Color Mixer band,
    /// whichever channel the panel shows.
    Hsl(usize, usize),
    /// A band's gray mix, which Black & White uses in place of the mixer.
    Gray(usize),
}
impl Param {
    /// The sliders with a name of their own, as `midi.json` and the control
    /// socket spell them.
    pub(in crate::app) const NAMED: [(&'static str, Self); 14] = [
        ("exposure", Self::Setting(Exposure)),
        ("contrast", Self::Setting(Contrast)),
        ("highlights", Self::Setting(Highlights)),
        ("shadows", Self::Setting(Shadows)),
        ("whites", Self::Setting(Whites)),
        ("blacks", Self::Setting(Blacks)),
        ("texture", Self::Setting(Texture)),
        ("clarity", Self::Setting(Clarity)),
        ("dehaze", Self::Setting(Dehaze)),
        ("vibrance", Self::Setting(Vibrance)),
        ("saturation", Self::Setting(Saturation)),
        ("temperature", Self::Setting(Temperature)),
        ("tint", Self::Setting(Tint)),
        ("straighten", Self::Setting(Straighten)),
    ];
    /// `exposure`, `band3` (the channel the panel shows), `band3.sat` or
    /// `band3.gray`; bands count from 1 (Red). `temp` and `angle` are aliases.
    pub(in crate::app) fn parse(name: &str) -> Option<Self> {
        let name = name.to_ascii_lowercase();
        match name.as_str() {
            "temp" => return Some(Self::Setting(Temperature)),
            "angle" => return Some(Self::Setting(Straighten)),
            _ => {}
        }
        if let Some((_, param)) = Self::NAMED.iter().find(|(n, _)| *n == name) {
            return Some(*param);
        }
        let (band, channel) = match name.strip_prefix("band")?.split_once('.') {
            Some((band, channel)) => (band, Some(channel)),
            None => (name.strip_prefix("band")?, None),
        };
        let i = match band.parse::<usize>() {
            Ok(n) if (1..=8).contains(&n) => n - 1,
            _ => return None,
        };
        Some(match channel {
            None => Self::Band(i),
            Some("hue") => Self::Hsl(i, 0),
            Some("sat" | "saturation") => Self::Hsl(i, 1),
            Some("lum" | "luminance") => Self::Hsl(i, 2),
            Some("gray" | "grey") => Self::Gray(i),
            Some(_) => return None,
        })
    }
    /// The name `parse` reads back: `exposure`, `band3`, `band3.sat`.
    pub(in crate::app) fn spec(self) -> String {
        if let Some((name, _)) = Self::NAMED.iter().find(|(_, p)| *p == self) {
            return (*name).into();
        }
        match self {
            Self::Band(i) => format!("band{}", i + 1),
            Self::Hsl(i, c) => format!("band{}.{}", i + 1, ["hue", "sat", "lum"][c]),
            Self::Gray(i) => format!("band{}.gray", i + 1),
            _ => unreachable!("every other slider has a name"),
        }
    }
    /// The name the slider and its History step carry.
    pub(in crate::app) fn label(self, channel: usize) -> String {
        match self {
            Self::Band(i) => return format!("{} {}", BANDS[i], MIXER_CHANNELS[channel]),
            Self::Hsl(i, c) => return format!("{} {}", BANDS[i], MIXER_CHANNELS[c]),
            Self::Gray(i) => return format!("{} Gray", BANDS[i]),
            _ => {}
        }
        match self {
            Self::Setting(id) => id.descriptor().label.to_string(),
            Self::Band(_) | Self::Hsl(..) | Self::Gray(_) => unreachable!("named above"),
        }
    }
    pub(in crate::app) fn value(self, r: &mut Recipe, channel: usize) -> &mut f32 {
        match self {
            // Black & White swaps the mixer for one gray mix per band.
            Self::Band(i) if r.effects.monochrome => &mut r.effects.gray_mix[i],
            Self::Band(i) => &mut r.hsl[i][channel],
            Self::Hsl(i, c) => &mut r.hsl[i][c],
            Self::Gray(i) => &mut r.effects.gray_mix[i],
            Self::Setting(id) => id.value_mut(r),
        }
    }
    /// The slider's number as it shows: EV, kelvin, tint units or degrees, else
    /// -100..100.
    pub(in crate::app) fn shown(self, r: &mut Recipe, channel: usize) -> f64 {
        let value = *self.value(r, channel);
        match self {
            Self::Setting(id) => id.shown(value),
            _ => (f64::from(value) * 100. * 1000.).round() / 1000.,
        }
    }
    /// Sets the slider to `shown`, a number as `shown` returns it, and returns
    /// the text for the History step.
    pub(in crate::app) fn set(self, r: &mut Recipe, shown: f32, channel: usize) -> String {
        let v = self.value(r, channel);
        match self {
            // An exact number is an explicit request: anything a recipe may hold.
            Self::Setting(id) => {
                *v = id.from_shown(shown);
                id.text(*v)
            }
            _ => {
                *v = (shown / 100.).clamp(-1., 1.);
                format_value(f64::from(*v * 100.), 0, true)
            }
        }
    }
    /// Moves the slider by `ticks` (clockwise positive) and returns the value as
    /// the slider shows it, for the History step.
    pub(in crate::app) fn turn(self, r: &mut Recipe, ticks: i32, channel: usize) -> String {
        let v = self.value(r, channel);
        match self {
            Self::Setting(id) => {
                *v = id.turned(*v, ticks);
                id.text(*v)
            }
            _ => {
                *v = nudged(*v, 0.01 * ticks as f32, -1. ..=1.);
                format_value(f64::from(*v * 100.), 0, true)
            }
        }
    }
}

impl Param {
    pub(in crate::app) fn all() -> Vec<(String, Self)> {
        Self::NAMED
            .iter()
            .map(|(n, p)| ((*n).into(), *p))
            .chain((0..8).flat_map(|i| {
                (0..4).map(move |c| {
                    let p = if c == 3 {
                        Self::Gray(i)
                    } else {
                        Self::Hsl(i, c)
                    };
                    (p.spec(), p)
                })
            }))
            .collect()
    }
    /// The range a control covers, in the units it shows.
    pub(in crate::app) fn range(self, mask: bool) -> (f32, f32) {
        match self {
            Self::Setting(Exposure) if mask => (-4., 4.),
            // On a mask, Temp and Tint are relative shifts of −100..100.
            Self::Setting(Temperature | Tint) if mask => (-100., 100.),
            Self::Setting(id) => {
                let d = id.descriptor();
                let scale = d.display.scale;
                (d.interactive.start() * scale, d.interactive.end() * scale)
            }
            _ => (-100., 100.),
        }
    }
    /// Preserve both endpoints and the neutral centre of bipolar controls.
    pub(in crate::app) fn control_value(self, value: u8, mask: bool) -> f32 {
        let (min, max) = self.range(mask);
        let value = f32::from(value.min(127));
        if min < 0. && max > 0. {
            if value <= 64. {
                min * (64. - value) / 64.
            } else {
                max * (value - 64.) / 63.
            }
        } else {
            min + (max - min) * value / 127.
        }
    }
    pub(in crate::app) fn capabilities() -> Vec<super::reply::Parameter> {
        let items: Vec<_> = Self::all()
            .into_iter()
            .map(|(name, param)| {
                let (min, max) = param.range(false);
                let (mask_min, mask_max) = param.range(true);
                let unit = match param {
                    Self::Setting(Exposure) => "EV",
                    Self::Setting(Temperature) => "kelvin",
                    Self::Setting(Tint) => "tint",
                    Self::Setting(Straighten) => "degrees",
                    _ => "percent",
                };
                let local = param
                    .local_shown(&crate::model::masks::LocalAdjust::default())
                    .is_some();
                super::reply::Parameter {
                    name,
                    unit,
                    min,
                    max,
                    mask: local,
                    mask_unit: if param == Self::Setting(Exposure) {
                        "EV"
                    } else {
                        "percent"
                    },
                    mask_min,
                    mask_max,
                }
            })
            .collect();
        items
    }
    fn local_value(self, a: &mut crate::model::masks::LocalAdjust) -> Option<&mut f32> {
        Some(match self {
            Self::Setting(Exposure) => &mut a.exposure,
            Self::Setting(Temperature) => &mut a.temperature,
            Self::Setting(Tint) => &mut a.tint,
            Self::Setting(Contrast) => &mut a.contrast,
            Self::Setting(Highlights) => &mut a.highlights,
            Self::Setting(Shadows) => &mut a.shadows,
            Self::Setting(Whites) => &mut a.whites,
            Self::Setting(Blacks) => &mut a.blacks,
            Self::Setting(Texture) => &mut a.texture,
            Self::Setting(Clarity) => &mut a.clarity,
            Self::Setting(Dehaze) => &mut a.dehaze,
            Self::Setting(Saturation) => &mut a.saturation,
            _ => return None,
        })
    }
    pub(in crate::app) fn local_shown(self, a: &crate::model::masks::LocalAdjust) -> Option<f32> {
        let mut copy = *a;
        self.local_value(&mut copy).map(|v| {
            *v * if self == Self::Setting(Exposure) {
                1.
            } else {
                100.
            }
        })
    }
    pub(in crate::app) fn local_set(
        self,
        a: &mut crate::model::masks::LocalAdjust,
        shown: Option<f32>,
        ticks: i32,
    ) -> super::Result<String> {
        let v = self.local_value(a).ok_or_else(|| {
            super::Error::new(
                "unsupported_parameter",
                "This parameter is not available on masks",
            )
        })?;
        let exposure = self == Self::Setting(Exposure);
        let scale = if exposure { 1. } else { 100. };
        let limit = if exposure { 4. } else { 1. };
        *v = shown
            .map_or(
                *v + ticks as f32 * if exposure { 0.02 } else { 0.01 },
                |n| n / scale,
            )
            .clamp(-limit, limit);
        Ok(format_value(
            f64::from(*v * scale),
            if exposure { 2 } else { 0 },
            true,
        ))
    }
}
