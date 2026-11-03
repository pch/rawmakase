//! The develop settings controls move, described once: their label, the range a
//! slider or dial covers, the range a recipe may hold, how one dial tick moves
//! them and how their value is shown. Sliders, the control socket, MIDI and
//! History read them here rather than keeping their own copies.
//!
//! A control's interactive range can be narrower than the values a recipe may
//! hold: the Exposure slider spans ±5 EV, while an imported edit can carry up to
//! ±8. Relative input must not quietly pull such a value into the narrower range.
use crate::model::masks::LocalAdjust;
use crate::model::recipe::{EXPOSURE_LIMIT, Recipe, TEMPERATURE_MAX, TEMPERATURE_MIN, TINT_LIMIT};
use std::ops::RangeInclusive;

/// The Crop panel's Angle limit either way, in degrees, as `Recipe::validate` allows.
pub const STRAIGHTEN_LIMIT: f32 = 45.;

/// A develop setting that sliders, dials and commands move.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParameterId {
    Exposure,
    Contrast,
    Highlights,
    Shadows,
    Whites,
    Blacks,
    Texture,
    Clarity,
    Dehaze,
    Vibrance,
    Saturation,
    Temperature,
    Tint,
    /// A rendered file's Temp and Tint, relative −100..+100.
    IncrementalTemperature,
    IncrementalTint,
    /// The Crop panel's Angle, in degrees.
    Straighten,
    /// Detail: Sharpening.
    SharpeningAmount,
    /// In pixels.
    SharpeningRadius,
    SharpeningDetail,
    SharpeningMasking,
    /// Detail: Noise Reduction.
    LuminanceNoise,
    LuminanceDetail,
    LuminanceContrast,
    ColorNoise,
    ColorNoiseDetail,
    ColorNoiseSmoothness,
    /// Detail: AI Denoise's Amount.
    AiDenoiseAmount,
    /// Effects: Post-Crop Vignetting.
    VignetteAmount,
    VignetteMidpoint,
    VignetteFeather,
    /// Effects: Grain.
    GrainAmount,
    GrainSize,
    GrainRoughness,
    /// Transform, stored along the photo's own axes; the panel shows them along the
    /// displayed photo's.
    TransformVertical,
    TransformHorizontal,
    /// In degrees.
    TransformRotate,
    TransformAspect,
    TransformScale,
    TransformOffsetX,
    TransformOffsetY,
    /// Lens Corrections: how much of the lens profile's correction applies, 0..2.
    LensDistortion,
    LensVignetting,
    /// Lens Corrections: Manual.
    ManualDistortion,
    LensVignetteAmount,
    LensVignetteMidpoint,
    /// Calibration.
    ShadowTint,
    RedPrimaryHue,
    RedPrimarySaturation,
    GreenPrimaryHue,
    GreenPrimarySaturation,
    BluePrimaryHue,
    BluePrimarySaturation,
    /// Tone Curve: the parametric curve's regions, darkest first.
    ParametricShadows,
    ParametricDarks,
    ParametricLights,
    ParametricHighlights,
    /// Tone Curve: Refine Saturation. Camera Raw stores up to 2 and renders above
    /// 1 as 1; the slider covers 0..1.
    CurveSaturation,
    /// Tone Curve: Levels' Midtone, a gamma.
    Midtone,
    /// Color Grading.
    GradingBlending,
    GradingBalance,
}

/// How one dial tick or `turn` step moves a setting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tick {
    /// By this much, in the setting's own units.
    Linear(f32),
    /// By this many mireds, as the Temp slider moves: evenly in mireds, so a
    /// tick is a similar change of colour anywhere on the scale; clockwise is
    /// warmer.
    Mireds(f32),
}

/// How far one press of Lightroom's `+` or `-` moves the selected Basic slider:
/// Option for fine, nothing for normal, Shift for large.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyStep {
    Fine,
    Normal,
    Large,
}

/// How a setting's value is shown and typed: in its own unit, or scaled to
/// −100..100.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Display {
    /// What the stored value is multiplied by to show it.
    pub scale: f32,
    pub decimals: usize,
    /// Whether positive values show a `+`.
    pub signed: bool,
}

/// Everything controls need to know about a setting: a develop setting
/// ([`ParameterId`]) or a mask's ([`LocalParameterId`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Descriptor<Id = ParameterId> {
    pub id: Id,
    /// The slider's label, and the name of its History step.
    pub label: &'static str,
    /// What a slider or dial covers, in the recipe's units.
    pub interactive: RangeInclusive<f32>,
    /// What a recipe may hold, and what a typed value or the `set` command may set.
    pub valid: RangeInclusive<f32>,
    pub tick: Tick,
    /// What dragging its slider snaps to, as Lightroom's 0.05 EV for Exposure;
    /// `None` moves freely.
    pub drag_step: Option<f32>,
    pub display: Display,
}

const UNIT: Display = Display {
    scale: 100.,
    decimals: 0,
    signed: true,
};
const HUNDREDTHS: Display = Display {
    scale: 1.,
    decimals: 2,
    signed: true,
};

/// A 0..100 slider stored as 0..1.
const fn amount<Id: Copy>(id: Id, label: &'static str) -> Descriptor<Id> {
    Descriptor {
        id,
        label,
        interactive: 0. ..=1.,
        valid: 0. ..=1.,
        tick: Tick::Linear(0.01),
        drag_step: None,
        display: Display {
            scale: 100.,
            decimals: 0,
            signed: false,
        },
    }
}

/// A 0..200 slider stored as 0..2, where 100 applies a profile's correction as it is.
const fn profile_amount(id: ParameterId, label: &'static str) -> Descriptor {
    Descriptor {
        interactive: 0. ..=2.,
        valid: 0. ..=2.,
        ..amount(id, label)
    }
}

/// A −100..100 slider stored as −1..1.
const fn percent<Id: Copy>(id: Id, label: &'static str) -> Descriptor<Id> {
    Descriptor {
        id,
        label,
        interactive: -1. ..=1.,
        valid: -1. ..=1.,
        tick: Tick::Linear(0.01),
        drag_step: None,
        display: UNIT,
    }
}

const DESCRIPTORS: [Descriptor; 60] = [
    Descriptor {
        id: ParameterId::Exposure,
        label: "Exposure",
        interactive: -5. ..=5.,
        valid: -EXPOSURE_LIMIT..=EXPOSURE_LIMIT,
        tick: Tick::Linear(0.02),
        drag_step: Some(0.05),
        display: HUNDREDTHS,
    },
    percent(ParameterId::Contrast, "Contrast"),
    percent(ParameterId::Highlights, "Highlights"),
    percent(ParameterId::Shadows, "Shadows"),
    percent(ParameterId::Whites, "Whites"),
    percent(ParameterId::Blacks, "Blacks"),
    percent(ParameterId::Texture, "Texture"),
    percent(ParameterId::Clarity, "Clarity"),
    percent(ParameterId::Dehaze, "Dehaze"),
    percent(ParameterId::Vibrance, "Vibrance"),
    percent(ParameterId::Saturation, "Saturation"),
    Descriptor {
        id: ParameterId::Temperature,
        label: "Temp",
        interactive: TEMPERATURE_MIN..=TEMPERATURE_MAX,
        valid: TEMPERATURE_MIN..=TEMPERATURE_MAX,
        tick: Tick::Mireds(4.),
        drag_step: None,
        display: Display {
            scale: 1.,
            decimals: 0,
            signed: false,
        },
    },
    Descriptor {
        id: ParameterId::Tint,
        label: "Tint",
        interactive: -TINT_LIMIT..=TINT_LIMIT,
        valid: -TINT_LIMIT..=TINT_LIMIT,
        tick: Tick::Linear(1.),
        drag_step: None,
        display: Display {
            scale: 1.,
            decimals: 0,
            signed: true,
        },
    },
    Descriptor {
        id: ParameterId::IncrementalTemperature,
        label: "Temp",
        interactive: -100. ..=100.,
        valid: -100. ..=100.,
        tick: Tick::Linear(1.),
        drag_step: None,
        display: Display {
            scale: 1.,
            decimals: 0,
            signed: true,
        },
    },
    Descriptor {
        id: ParameterId::IncrementalTint,
        label: "Tint",
        interactive: -100. ..=100.,
        valid: -100. ..=100.,
        tick: Tick::Linear(1.),
        drag_step: None,
        display: Display {
            scale: 1.,
            decimals: 0,
            signed: true,
        },
    },
    Descriptor {
        id: ParameterId::Straighten,
        label: "Angle",
        interactive: -STRAIGHTEN_LIMIT..=STRAIGHTEN_LIMIT,
        valid: -STRAIGHTEN_LIMIT..=STRAIGHTEN_LIMIT,
        // Fine enough to level a horizon.
        tick: Tick::Linear(0.1),
        drag_step: None,
        display: HUNDREDTHS,
    },
    Descriptor {
        // Lightroom's 0..150.
        display: Display {
            scale: 150.,
            decimals: 0,
            signed: false,
        },
        ..amount(ParameterId::SharpeningAmount, "Amount")
    },
    Descriptor {
        id: ParameterId::SharpeningRadius,
        label: "Radius",
        interactive: 0.5..=3.,
        valid: 0.5..=3.,
        tick: Tick::Linear(0.1),
        drag_step: None,
        display: Display {
            scale: 1.,
            decimals: 1,
            signed: false,
        },
    },
    amount(ParameterId::SharpeningDetail, "Detail"),
    amount(ParameterId::SharpeningMasking, "Masking"),
    amount(ParameterId::LuminanceNoise, "Luminance"),
    amount(ParameterId::LuminanceDetail, "Detail"),
    amount(ParameterId::LuminanceContrast, "Contrast"),
    amount(ParameterId::ColorNoise, "Color"),
    amount(ParameterId::ColorNoiseDetail, "Detail"),
    amount(ParameterId::ColorNoiseSmoothness, "Smoothness"),
    amount(ParameterId::AiDenoiseAmount, "Amount"),
    percent(ParameterId::VignetteAmount, "Amount"),
    amount(ParameterId::VignetteMidpoint, "Midpoint"),
    amount(ParameterId::VignetteFeather, "Feather"),
    amount(ParameterId::GrainAmount, "Amount"),
    amount(ParameterId::GrainSize, "Size"),
    amount(ParameterId::GrainRoughness, "Roughness"),
    percent(ParameterId::TransformVertical, "Vertical"),
    percent(ParameterId::TransformHorizontal, "Horizontal"),
    Descriptor {
        id: ParameterId::TransformRotate,
        label: "Rotate",
        interactive: -10. ..=10.,
        valid: -10. ..=10.,
        tick: Tick::Linear(0.1),
        drag_step: None,
        display: Display {
            scale: 1.,
            decimals: 1,
            signed: true,
        },
    },
    percent(ParameterId::TransformAspect, "Aspect"),
    Descriptor {
        id: ParameterId::TransformScale,
        label: "Scale",
        interactive: 0.5..=1.5,
        valid: 0.5..=1.5,
        tick: Tick::Linear(0.01),
        drag_step: None,
        display: Display {
            scale: 100.,
            decimals: 0,
            signed: false,
        },
    },
    percent(ParameterId::TransformOffsetX, "Offset X"),
    percent(ParameterId::TransformOffsetY, "Offset Y"),
    profile_amount(ParameterId::LensDistortion, "Distortion"),
    profile_amount(ParameterId::LensVignetting, "Vignetting"),
    percent(ParameterId::ManualDistortion, "Amount"),
    percent(ParameterId::LensVignetteAmount, "Amount"),
    amount(ParameterId::LensVignetteMidpoint, "Midpoint"),
    percent(ParameterId::ShadowTint, "Tint"),
    percent(ParameterId::RedPrimaryHue, "Hue"),
    percent(ParameterId::RedPrimarySaturation, "Saturation"),
    percent(ParameterId::GreenPrimaryHue, "Hue"),
    percent(ParameterId::GreenPrimarySaturation, "Saturation"),
    percent(ParameterId::BluePrimaryHue, "Hue"),
    percent(ParameterId::BluePrimarySaturation, "Saturation"),
    percent(ParameterId::ParametricShadows, "Shadows"),
    percent(ParameterId::ParametricDarks, "Darks"),
    percent(ParameterId::ParametricLights, "Lights"),
    percent(ParameterId::ParametricHighlights, "Highlights"),
    Descriptor {
        valid: 0. ..=2.,
        ..amount(ParameterId::CurveSaturation, "Saturation")
    },
    Descriptor {
        id: ParameterId::Midtone,
        label: "Midtone",
        interactive: 0.1..=4.,
        valid: 0.1..=4.,
        tick: Tick::Linear(0.01),
        drag_step: None,
        display: Display {
            scale: 1.,
            decimals: 2,
            signed: false,
        },
    },
    amount(ParameterId::GradingBlending, "Blending"),
    percent(ParameterId::GradingBalance, "Balance"),
];

impl ParameterId {
    /// The setting this control moves on photo `m`: a JPEG, PNG or TIFF keeps its
    /// white balance as relative Temp and Tint, so Temperature and Tint move those.
    pub fn for_photo(self, m: Option<&crate::camera_data::Metadata>) -> Self {
        match self {
            Self::Temperature if m.is_some_and(|m| m.is_rendered()) => Self::IncrementalTemperature,
            Self::Tint if m.is_some_and(|m| m.is_rendered()) => Self::IncrementalTint,
            other => other,
        }
    }
    /// The parametric curve's regions, in the order of `Effects::parametric`.
    pub const PARAMETRIC: [Self; 4] = [
        Self::ParametricShadows,
        Self::ParametricDarks,
        Self::ParametricLights,
        Self::ParametricHighlights,
    ];
    /// Calibration's Hue and Saturation of each primary, red to blue.
    pub const PRIMARIES: [[Self; 2]; 3] = [
        [Self::RedPrimaryHue, Self::RedPrimarySaturation],
        [Self::GreenPrimaryHue, Self::GreenPrimarySaturation],
        [Self::BluePrimaryHue, Self::BluePrimarySaturation],
    ];
    #[cfg(test)]
    pub const ALL: [Self; 60] = [
        Self::Exposure,
        Self::Contrast,
        Self::Highlights,
        Self::Shadows,
        Self::Whites,
        Self::Blacks,
        Self::Texture,
        Self::Clarity,
        Self::Dehaze,
        Self::Vibrance,
        Self::Saturation,
        Self::Temperature,
        Self::Tint,
        Self::IncrementalTemperature,
        Self::IncrementalTint,
        Self::Straighten,
        Self::SharpeningAmount,
        Self::SharpeningRadius,
        Self::SharpeningDetail,
        Self::SharpeningMasking,
        Self::LuminanceNoise,
        Self::LuminanceDetail,
        Self::LuminanceContrast,
        Self::ColorNoise,
        Self::ColorNoiseDetail,
        Self::ColorNoiseSmoothness,
        Self::AiDenoiseAmount,
        Self::VignetteAmount,
        Self::VignetteMidpoint,
        Self::VignetteFeather,
        Self::GrainAmount,
        Self::GrainSize,
        Self::GrainRoughness,
        Self::TransformVertical,
        Self::TransformHorizontal,
        Self::TransformRotate,
        Self::TransformAspect,
        Self::TransformScale,
        Self::TransformOffsetX,
        Self::TransformOffsetY,
        Self::LensDistortion,
        Self::LensVignetting,
        Self::ManualDistortion,
        Self::LensVignetteAmount,
        Self::LensVignetteMidpoint,
        Self::ShadowTint,
        Self::RedPrimaryHue,
        Self::RedPrimarySaturation,
        Self::GreenPrimaryHue,
        Self::GreenPrimarySaturation,
        Self::BluePrimaryHue,
        Self::BluePrimarySaturation,
        Self::ParametricShadows,
        Self::ParametricDarks,
        Self::ParametricLights,
        Self::ParametricHighlights,
        Self::CurveSaturation,
        Self::Midtone,
        Self::GradingBlending,
        Self::GradingBalance,
    ];
    pub fn descriptor(self) -> &'static Descriptor {
        &DESCRIPTORS[self as usize]
    }
    /// The recipe field this setting is.
    pub fn value_mut(self, r: &mut Recipe) -> &mut f32 {
        match self {
            Self::Exposure => &mut r.exposure,
            Self::Contrast => &mut r.contrast,
            Self::Highlights => &mut r.highlights,
            Self::Shadows => &mut r.shadows,
            Self::Whites => &mut r.whites,
            Self::Blacks => &mut r.blacks,
            Self::Texture => &mut r.effects.texture,
            Self::Clarity => &mut r.effects.clarity,
            Self::Dehaze => &mut r.effects.dehaze,
            Self::Vibrance => &mut r.vibrance,
            Self::Saturation => &mut r.saturation,
            Self::Temperature => &mut r.temperature,
            Self::Tint => &mut r.tint,
            Self::IncrementalTemperature => &mut r.incremental_temperature,
            Self::IncrementalTint => &mut r.incremental_tint,
            Self::Straighten => &mut r.straighten,
            Self::SharpeningAmount => &mut r.sharpening,
            Self::SharpeningRadius => &mut r.sharpening_radius,
            Self::SharpeningDetail => &mut r.sharpening_detail,
            Self::SharpeningMasking => &mut r.sharpening_masking,
            Self::LuminanceNoise => &mut r.noise_luma,
            Self::LuminanceDetail => &mut r.effects.luma_detail,
            Self::LuminanceContrast => &mut r.effects.luma_contrast,
            Self::ColorNoise => &mut r.noise_chroma,
            Self::ColorNoiseDetail => &mut r.effects.chroma_detail,
            Self::ColorNoiseSmoothness => &mut r.effects.chroma_smoothness,
            Self::AiDenoiseAmount => &mut r.ai_denoise.amount,
            Self::VignetteAmount => &mut r.effects.vignette,
            Self::VignetteMidpoint => &mut r.effects.vignette_midpoint,
            Self::VignetteFeather => &mut r.effects.vignette_feather,
            Self::GrainAmount => &mut r.effects.grain,
            Self::GrainSize => &mut r.effects.grain_size,
            Self::GrainRoughness => &mut r.effects.grain_roughness,
            Self::TransformVertical => &mut r.transform.vertical,
            Self::TransformHorizontal => &mut r.transform.horizontal,
            Self::TransformRotate => &mut r.transform.rotate,
            Self::TransformAspect => &mut r.transform.aspect,
            Self::TransformScale => &mut r.transform.scale,
            Self::TransformOffsetX => &mut r.transform.offset_x,
            Self::TransformOffsetY => &mut r.transform.offset_y,
            Self::LensDistortion => &mut r.lens_distortion,
            Self::LensVignetting => &mut r.lens_vignetting,
            Self::ManualDistortion => &mut r.lens_manual_distortion,
            Self::LensVignetteAmount => &mut r.effects.lens_vignette,
            Self::LensVignetteMidpoint => &mut r.effects.lens_vignette_midpoint,
            Self::ShadowTint => &mut r.effects.shadow_tint,
            Self::RedPrimaryHue => &mut r.effects.calibration[0][0],
            Self::RedPrimarySaturation => &mut r.effects.calibration[0][1],
            Self::GreenPrimaryHue => &mut r.effects.calibration[1][0],
            Self::GreenPrimarySaturation => &mut r.effects.calibration[1][1],
            Self::BluePrimaryHue => &mut r.effects.calibration[2][0],
            Self::BluePrimarySaturation => &mut r.effects.calibration[2][1],
            Self::ParametricShadows => &mut r.effects.parametric[0],
            Self::ParametricDarks => &mut r.effects.parametric[1],
            Self::ParametricLights => &mut r.effects.parametric[2],
            Self::ParametricHighlights => &mut r.effects.parametric[3],
            Self::CurveSaturation => &mut r.curve_saturation,
            Self::Midtone => &mut r.midtone,
            Self::GradingBlending => &mut r.effects.blending,
            Self::GradingBalance => &mut r.effects.balance,
        }
    }
    /// `value` in the units the slider shows, rounded to thousandths.
    pub fn shown(self, value: f32) -> f64 {
        let scale = f64::from(self.descriptor().display.scale);
        (f64::from(value) * scale * 1000.).round() / 1000.
    }
    /// `value` as the slider shows it: "+0.50", "-35", "6500".
    pub fn text(self, value: f32) -> String {
        let display = self.descriptor().display;
        format_value(
            f64::from(value * display.scale),
            display.decimals,
            display.signed,
        )
    }
    /// The value of a number typed in the units the slider shows, within what a
    /// recipe may hold.
    pub fn from_shown(self, shown: f32) -> f32 {
        let d = self.descriptor();
        (shown / d.display.scale).clamp(*d.valid.start(), *d.valid.end())
    }
    /// `value` moved by `ticks` dial ticks (clockwise positive); see [`nudged`]
    /// for values outside the interactive range.
    pub fn turned(self, value: f32, ticks: i32) -> f32 {
        let d = self.descriptor();
        let t = ticks as f32;
        match d.tick {
            Tick::Linear(step) => nudged(value, step * t, d.interactive.clone()),
            Tick::Mireds(step) => {
                let (low, high) = (*d.interactive.start(), *d.interactive.end());
                let mired = (1e6 / value - step * t).max(1e6 / high);
                (1e6 / mired).clamp(low, high)
            }
        }
    }
    /// How far one press of `+` or `-` moves this setting, in the recipe's units:
    /// Lightroom Classic's steps (fine, normal, large) for the Basic sliders it
    /// selects with `.` and `,`; `None` for the rest. Temp steps evenly in kelvin,
    /// unlike its dial's mireds; a rendered file's relative Temp steps as Tint.
    pub fn key_steps(self) -> Option<[f32; 3]> {
        match self {
            Self::Exposure => Some([0.02, 0.1, 0.33]),
            Self::Contrast
            | Self::Highlights
            | Self::Shadows
            | Self::Whites
            | Self::Blacks
            | Self::Texture
            | Self::Clarity
            | Self::Dehaze
            | Self::Vibrance
            | Self::Saturation => Some([0.01, 0.05, 0.2]),
            Self::Temperature => Some([5., 50., 200.]),
            Self::Tint | Self::IncrementalTemperature | Self::IncrementalTint => {
                Some([1., 5., 20.])
            }
            _ => None,
        }
    }
    /// `value` moved one `size` key step, up or down, from where it is: no snapping
    /// to a grid, clamped to the slider's range (see [`nudged`] for values beyond
    /// it). A setting without key steps keeps its value.
    pub fn key_stepped(self, value: f32, size: KeyStep, up: bool) -> f32 {
        let Some(steps) = self.key_steps() else {
            return value;
        };
        let step = match size {
            KeyStep::Fine => steps[0],
            KeyStep::Normal => steps[1],
            KeyStep::Large => steps[2],
        };
        let delta = if up { step } else { -step };
        nudged(value, delta, self.descriptor().interactive.clone())
    }
}

/// A number as sliders show it: `decimals` places, and a `+` on positive values
/// when `signed`.
pub fn format_value(v: f64, decimals: usize, signed: bool) -> String {
    let text = format!("{v:.decimals$}");
    if signed && v > 0. && !text.trim_start_matches(['0', '.']).is_empty() {
        format!("+{text}")
    } else {
        text
    }
}

/// `value` moved by `delta` by relative input (a dial tick, an arrow key over a
/// slider) on a control whose interactive range is `range`.
///
/// Inside the range, the result is clamped to it. A value outside it moves by
/// `delta` toward the range, without snapping to the nearer bound, and a move
/// further away leaves it as it is. Relative input therefore never pushes a value
/// further out, and never discards the part of it beyond the bound.
pub fn nudged(value: f32, delta: f32, range: RangeInclusive<f32>) -> f32 {
    let (low, high) = (*range.start(), *range.end());
    let moved = value + delta;
    if value > high {
        if delta < 0. { moved.max(low) } else { value }
    } else if value < low {
        if delta > 0. { moved.min(high) } else { value }
    } else {
        moved.clamp(low, high)
    }
}

/// A mask's adjustment slider (see [`LocalAdjust`]). The ranges are a mask's
/// own: Temp and Tint move relative to the photo's, from −100 to 100.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LocalParameterId {
    Temperature,
    Tint,
    Exposure,
    Contrast,
    Highlights,
    Shadows,
    Whites,
    Blacks,
    Texture,
    Clarity,
    Dehaze,
    /// In degrees.
    Hue,
    Saturation,
    Sharpness,
    Noise,
    /// Lightroom's Color swatch: its hue, 0..1 shown as degrees.
    ColorHue,
    ColorSaturation,
}

const LOCAL_DESCRIPTORS: [Descriptor<LocalParameterId>; 17] = [
    percent(LocalParameterId::Temperature, "Temp"),
    percent(LocalParameterId::Tint, "Tint"),
    Descriptor {
        id: LocalParameterId::Exposure,
        label: "Exposure",
        interactive: -4. ..=4.,
        valid: -4. ..=4.,
        tick: Tick::Linear(0.02),
        drag_step: Some(0.05),
        display: HUNDREDTHS,
    },
    percent(LocalParameterId::Contrast, "Contrast"),
    percent(LocalParameterId::Highlights, "Highlights"),
    percent(LocalParameterId::Shadows, "Shadows"),
    percent(LocalParameterId::Whites, "Whites"),
    percent(LocalParameterId::Blacks, "Blacks"),
    percent(LocalParameterId::Texture, "Texture"),
    percent(LocalParameterId::Clarity, "Clarity"),
    percent(LocalParameterId::Dehaze, "Dehaze"),
    Descriptor {
        id: LocalParameterId::Hue,
        label: "Hue",
        interactive: -180. ..=180.,
        valid: -180. ..=180.,
        tick: Tick::Linear(1.),
        drag_step: None,
        display: Display {
            scale: 1.,
            decimals: 0,
            signed: true,
        },
    },
    percent(LocalParameterId::Saturation, "Saturation"),
    percent(LocalParameterId::Sharpness, "Sharpness"),
    percent(LocalParameterId::Noise, "Noise"),
    Descriptor {
        display: Display {
            scale: 360.,
            decimals: 0,
            signed: false,
        },
        ..amount(LocalParameterId::ColorHue, "Color Hue")
    },
    amount(LocalParameterId::ColorSaturation, "Color Sat"),
];

impl LocalParameterId {
    /// In Lightroom's order.
    #[cfg(test)]
    pub const ALL: [Self; 17] = [
        Self::Temperature,
        Self::Tint,
        Self::Exposure,
        Self::Contrast,
        Self::Highlights,
        Self::Shadows,
        Self::Whites,
        Self::Blacks,
        Self::Texture,
        Self::Clarity,
        Self::Dehaze,
        Self::Hue,
        Self::Saturation,
        Self::Sharpness,
        Self::Noise,
        Self::ColorHue,
        Self::ColorSaturation,
    ];
    pub fn descriptor(self) -> &'static Descriptor<Self> {
        &LOCAL_DESCRIPTORS[self as usize]
    }
    /// The mask adjustment this slider is.
    pub fn value_mut(self, a: &mut LocalAdjust) -> &mut f32 {
        match self {
            Self::Temperature => &mut a.temperature,
            Self::Tint => &mut a.tint,
            Self::Exposure => &mut a.exposure,
            Self::Contrast => &mut a.contrast,
            Self::Highlights => &mut a.highlights,
            Self::Shadows => &mut a.shadows,
            Self::Whites => &mut a.whites,
            Self::Blacks => &mut a.blacks,
            Self::Texture => &mut a.texture,
            Self::Clarity => &mut a.clarity,
            Self::Dehaze => &mut a.dehaze,
            Self::Hue => &mut a.hue,
            Self::Saturation => &mut a.saturation,
            Self::Sharpness => &mut a.sharpness,
            Self::Noise => &mut a.noise,
            Self::ColorHue => &mut a.color[0],
            Self::ColorSaturation => &mut a.color[1],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rendered_photo_moves_temp_and_tint_relatively() {
        let rendered = crate::camera_data::Metadata {
            source: crate::camera_data::Source::Rendered,
            ..Default::default()
        };
        let raw = crate::camera_data::Metadata::default();
        for (id, relative) in [
            (
                ParameterId::Temperature,
                ParameterId::IncrementalTemperature,
            ),
            (ParameterId::Tint, ParameterId::IncrementalTint),
        ] {
            assert_eq!(id.for_photo(Some(&rendered)), relative);
            assert_eq!(id.for_photo(Some(&raw)), id);
            assert_eq!(id.for_photo(None), id);
        }
        assert_eq!(
            ParameterId::Exposure.for_photo(Some(&rendered)),
            ParameterId::Exposure
        );
    }

    #[test]
    fn every_parameter_has_its_own_descriptor() {
        for id in ParameterId::ALL {
            assert_eq!(id.descriptor().id, id);
        }
    }

    #[test]
    fn a_recipe_is_valid_exactly_within_each_valid_range() {
        for id in ParameterId::ALL {
            let d = id.descriptor();
            assert!(
                d.valid.contains(d.interactive.start()) && d.valid.contains(d.interactive.end()),
                "{id:?}"
            );
            for (value, valid) in [
                (*d.valid.start(), true),
                (*d.valid.end(), true),
                (d.valid.start() - 0.01, false),
                (d.valid.end() + 0.01, false),
            ] {
                let mut r = Recipe::default();
                *id.value_mut(&mut r) = value;
                assert_eq!(r.validate().is_ok(), valid, "{id:?} at {value}");
            }
        }
    }

    #[test]
    fn a_mask_adjustment_is_valid_exactly_within_each_valid_range() {
        for id in LocalParameterId::ALL {
            let d = id.descriptor();
            assert_eq!(d.id, id);
            assert_eq!(d.interactive, d.valid, "{id:?}");
            for (value, valid) in [
                (*d.valid.start(), true),
                (*d.valid.end(), true),
                (d.valid.start() - 0.01, false),
                (d.valid.end() + 0.01, false),
            ] {
                let mut a = LocalAdjust::default();
                *id.value_mut(&mut a) = value;
                assert_eq!(a.validate().is_ok(), valid, "{id:?} at {value}");
            }
        }
    }

    #[test]
    fn values_show_and_read_back_in_slider_units() {
        assert_eq!(ParameterId::Contrast.text(0.35), "+35");
        assert_eq!(ParameterId::Exposure.text(-0.5), "-0.50");
        assert_eq!(ParameterId::Temperature.text(6500.), "6500");
        assert_eq!(ParameterId::Contrast.from_shown(150.), 1.);
        assert_eq!(ParameterId::Exposure.from_shown(9.), EXPOSURE_LIMIT);
        assert_eq!(ParameterId::Contrast.shown(0.35), 35.);
    }

    #[test]
    fn key_steps_are_lightrooms_in_shown_units() {
        use KeyStep::{Fine, Large, Normal};
        let close = |a: f64, b: f64| (a - b).abs() < 1e-3;
        // (setting, start, size, up, shown result): Lightroom Classic 15.6's steps,
        // added to the value as it is, never snapped to a grid.
        for (id, start, size, up, shown) in [
            (ParameterId::Exposure, 0.37, Normal, false, 0.27),
            (ParameterId::Exposure, 0., Large, true, 0.33),
            (ParameterId::Exposure, 0., Fine, false, -0.02),
            (ParameterId::Contrast, 0.07, Normal, false, 2.),
            (ParameterId::Contrast, 0., Large, true, 20.),
            (ParameterId::Contrast, 0., Fine, true, 1.),
            (ParameterId::Saturation, 0., Normal, true, 5.),
            (ParameterId::Dehaze, 0., Large, false, -20.),
            (ParameterId::Temperature, 5350., Normal, false, 5300.),
            (ParameterId::Temperature, 3000., Large, true, 3200.),
            (ParameterId::Temperature, 20000., Large, false, 19800.),
            (ParameterId::Temperature, 3000., Fine, true, 3005.),
            (ParameterId::Temperature, 20000., Fine, true, 20005.),
            (ParameterId::Tint, 18., Normal, true, 23.),
            (ParameterId::Tint, 18., Large, false, -2.),
            (ParameterId::Tint, 18., Fine, true, 19.),
            (ParameterId::IncrementalTemperature, 0., Normal, true, 5.),
            (ParameterId::IncrementalTint, 0., Large, false, -20.),
            (ParameterId::IncrementalTemperature, 0., Fine, true, 1.),
        ] {
            let result = id.shown(id.key_stepped(start, size, up));
            assert!(
                close(result, shown),
                "{id:?} {start} {size:?} {up}: {result}"
            );
        }
    }

    #[test]
    fn key_steps_stop_at_the_slider_ends_and_never_push_further_out() {
        use KeyStep::{Large, Normal};
        assert_eq!(ParameterId::Exposure.key_stepped(4.9, Large, true), 5.);
        assert_eq!(ParameterId::Exposure.key_stepped(5., Normal, true), 5.);
        assert_eq!(ParameterId::Contrast.key_stepped(1., Large, true), 1.);
        assert_eq!(ParameterId::Tint.key_stepped(140., Large, true), 150.);
        assert_eq!(
            ParameterId::Temperature.key_stepped(TEMPERATURE_MAX, Large, true),
            TEMPERATURE_MAX
        );
        assert_eq!(
            ParameterId::Temperature.key_stepped(TEMPERATURE_MIN, Large, false),
            TEMPERATURE_MIN
        );
        // An imported Exposure beyond the slider's ±5 moves back by the step, and
        // further out not at all.
        assert_eq!(ParameterId::Exposure.key_stepped(7., Large, true), 7.);
        assert!((ParameterId::Exposure.key_stepped(7., Normal, false) - 6.9).abs() < 1e-5);
        // Only the Basic sliders have key steps.
        assert_eq!(ParameterId::VignetteAmount.key_steps(), None);
        assert_eq!(
            ParameterId::VignetteAmount.key_stepped(0.3, Large, true),
            0.3
        );
    }

    #[test]
    fn relative_input_never_moves_a_value_further_out_or_snaps_it() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-5;
        // (value, delta) -> result, on a ±5 control.
        for (value, delta, expected) in [
            (0., 0.02, 0.02),
            (4.99, 0.02, 5.),
            (-4.99, -0.02, -5.),
            (6., -0.02, 5.98),
            (6., 0.02, 6.),
            (6., -20., -5.),
            (-6., 0.02, -5.98),
            (-6., -0.02, -6.),
        ] {
            let result = nudged(value, delta, -5. ..=5.);
            assert!(close(result, expected), "{value} {delta:+}: {result}");
        }
    }
}
