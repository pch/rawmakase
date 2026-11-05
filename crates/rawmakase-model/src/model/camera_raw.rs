//! Camera Raw's setting keys for the recipe's sliders, named once: each key, how many
//! of Lightroom's units make one of the recipe's, and the preset group it belongs to.
//! XMP reading and writing (`rawmakase_interop::xmp`), presets and a look's settings
//! use them. Keys mean what they mean in Lightroom; what each reader accepts (its
//! range, which keys a look takes, what a missing key leaves) stays with that reader.
use crate::model::settings_groups::SettingGroup::{self, *};

/// One slider's Camera Raw key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slider {
    /// The key, without the `crs:` prefix.
    pub key: &'static str,
    /// Lightroom's units per recipe unit: 100 for most sliders (the recipe holds
    /// -1 to 1), 150 for Sharpness, 360 for hues, 20 for Defringe's amounts, 1 for
    /// stops, Kelvin, degrees, pixels and the crop's fractions.
    pub units: f32,
    /// The preset group Lightroom's New Develop Preset dialog files it under.
    pub group: SettingGroup,
}

impl Slider {
    /// Recipe units per Lightroom unit: what a value read is multiplied by, and a
    /// value written divided by. `1 / units`, which is the same f32 as the decimal
    /// it stands for (0.01, 1/150, 1/360, 0.05).
    pub fn scale(self) -> f32 {
        1. / self.units
    }
}

// Basic panel: tone (Exposure in stops, the rest -100 to +100).
pub const EXPOSURE: Slider = Slider {
    key: "Exposure2012",
    units: 1.,
    group: Exposure,
};
pub const CONTRAST: Slider = Slider {
    key: "Contrast2012",
    units: 100.,
    group: Contrast,
};
pub const HIGHLIGHTS: Slider = Slider {
    key: "Highlights2012",
    units: 100.,
    group: Highlights,
};
pub const SHADOWS: Slider = Slider {
    key: "Shadows2012",
    units: 100.,
    group: Shadows,
};
pub const WHITES: Slider = Slider {
    key: "Whites2012",
    units: 100.,
    group: Whites,
};
pub const BLACKS: Slider = Slider {
    key: "Blacks2012",
    units: 100.,
    group: Blacks,
};

// Basic panel: presence.
pub const TEXTURE: Slider = Slider {
    key: "Texture",
    units: 100.,
    group: Texture,
};
pub const CLARITY: Slider = Slider {
    key: "Clarity2012",
    units: 100.,
    group: Clarity,
};
pub const DEHAZE: Slider = Slider {
    key: "Dehaze",
    units: 100.,
    group: Dehaze,
};
pub const VIBRANCE: Slider = Slider {
    key: "Vibrance",
    units: 100.,
    group: Vibrance,
};
pub const SATURATION: Slider = Slider {
    key: "Saturation",
    units: 100.,
    group: Saturation,
};

// White balance: Kelvin and Tint for RAW files, relative steps for rendered files.
pub const TEMPERATURE: Slider = Slider {
    key: "Temperature",
    units: 1.,
    group: WhiteBalance,
};
pub const TINT: Slider = Slider {
    key: "Tint",
    units: 1.,
    group: WhiteBalance,
};
pub const INCREMENTAL_TEMPERATURE: Slider = Slider {
    key: "IncrementalTemperature",
    units: 1.,
    group: WhiteBalance,
};
pub const INCREMENTAL_TINT: Slider = Slider {
    key: "IncrementalTint",
    units: 1.,
    group: WhiteBalance,
};

// Tone Curve.
pub const CURVE_REFINE_SATURATION: Slider = Slider {
    key: "CurveRefineSaturation",
    units: 100.,
    group: ToneCurve,
};

// Detail: Sharpening (Amount 0 to 150, Radius in pixels) and Noise Reduction.
pub const SHARPNESS: Slider = Slider {
    key: "Sharpness",
    units: 150.,
    group: Sharpening,
};
pub const SHARPEN_RADIUS: Slider = Slider {
    key: "SharpenRadius",
    units: 1.,
    group: Sharpening,
};
pub const SHARPEN_DETAIL: Slider = Slider {
    key: "SharpenDetail",
    units: 100.,
    group: Sharpening,
};
pub const SHARPEN_EDGE_MASKING: Slider = Slider {
    key: "SharpenEdgeMasking",
    units: 100.,
    group: Sharpening,
};
pub const LUMINANCE_SMOOTHING: Slider = Slider {
    key: "LuminanceSmoothing",
    units: 100.,
    group: LuminanceNoiseReduction,
};
pub const LUMINANCE_NOISE_REDUCTION_DETAIL: Slider = Slider {
    key: "LuminanceNoiseReductionDetail",
    units: 100.,
    group: LuminanceNoiseReduction,
};
pub const LUMINANCE_NOISE_REDUCTION_CONTRAST: Slider = Slider {
    key: "LuminanceNoiseReductionContrast",
    units: 100.,
    group: LuminanceNoiseReduction,
};
pub const COLOR_NOISE_REDUCTION: Slider = Slider {
    key: "ColorNoiseReduction",
    units: 100.,
    group: ColorNoiseReduction,
};
pub const COLOR_NOISE_REDUCTION_DETAIL: Slider = Slider {
    key: "ColorNoiseReductionDetail",
    units: 100.,
    group: ColorNoiseReduction,
};
pub const COLOR_NOISE_REDUCTION_SMOOTHNESS: Slider = Slider {
    key: "ColorNoiseReductionSmoothness",
    units: 100.,
    group: ColorNoiseReduction,
};

// Color Grading: legacy Split Toning's balance, the midtones, and the blending (hues in degrees).
pub const SPLIT_TONING_BALANCE: Slider = Slider {
    key: "SplitToningBalance",
    units: 100.,
    group: ColorGrading,
};
pub const COLOR_GRADE_MIDTONE_HUE: Slider = Slider {
    key: "ColorGradeMidtoneHue",
    units: 360.,
    group: ColorGrading,
};
pub const COLOR_GRADE_MIDTONE_SAT: Slider = Slider {
    key: "ColorGradeMidtoneSat",
    units: 100.,
    group: ColorGrading,
};
pub const COLOR_GRADE_MIDTONE_LUM: Slider = Slider {
    key: "ColorGradeMidtoneLum",
    units: 100.,
    group: ColorGrading,
};
pub const COLOR_GRADE_BLENDING: Slider = Slider {
    key: "ColorGradeBlending",
    units: 100.,
    group: ColorGrading,
};

// Calibration: the shadows' tint.
pub const SHADOW_TINT: Slider = Slider {
    key: "ShadowTint",
    units: 100.,
    group: Calibration,
};

// Effects: grain and post-crop vignetting (its Style is a mode, not a slider).
pub const GRAIN_AMOUNT: Slider = Slider {
    key: "GrainAmount",
    units: 100.,
    group: Grain,
};
pub const GRAIN_SIZE: Slider = Slider {
    key: "GrainSize",
    units: 100.,
    group: Grain,
};
pub const GRAIN_FREQUENCY: Slider = Slider {
    key: "GrainFrequency",
    units: 100.,
    group: Grain,
};
pub const POST_CROP_VIGNETTE_AMOUNT: Slider = Slider {
    key: "PostCropVignetteAmount",
    units: 100.,
    group: PostCropVignetting,
};
pub const POST_CROP_VIGNETTE_MIDPOINT: Slider = Slider {
    key: "PostCropVignetteMidpoint",
    units: 100.,
    group: PostCropVignetting,
};
pub const POST_CROP_VIGNETTE_FEATHER: Slider = Slider {
    key: "PostCropVignetteFeather",
    units: 100.,
    group: PostCropVignetting,
};
pub const POST_CROP_VIGNETTE_ROUNDNESS: Slider = Slider {
    key: "PostCropVignetteRoundness",
    units: 100.,
    group: PostCropVignetting,
};
pub const POST_CROP_VIGNETTE_HIGHLIGHT_CONTRAST: Slider = Slider {
    key: "PostCropVignetteHighlightContrast",
    units: 100.,
    group: PostCropVignetting,
};

// Lens Corrections: manual vignetting and distortion, and the profile's scales.
pub const VIGNETTE_AMOUNT: Slider = Slider {
    key: "VignetteAmount",
    units: 100.,
    group: LensVignetting,
};
pub const VIGNETTE_MIDPOINT: Slider = Slider {
    key: "VignetteMidpoint",
    units: 100.,
    group: LensVignetting,
};
pub const LENS_MANUAL_DISTORTION_AMOUNT: Slider = Slider {
    key: "LensManualDistortionAmount",
    units: 100.,
    group: LensProfileCorrections,
};
pub const LENS_PROFILE_DISTORTION_SCALE: Slider = Slider {
    key: "LensProfileDistortionScale",
    units: 100.,
    group: LensProfileCorrections,
};
pub const LENS_PROFILE_VIGNETTING_SCALE: Slider = Slider {
    key: "LensProfileVignettingScale",
    units: 100.,
    group: LensProfileCorrections,
};

// Transform (Rotate in degrees).
pub const PERSPECTIVE_VERTICAL: Slider = Slider {
    key: "PerspectiveVertical",
    units: 100.,
    group: TransformAdjustments,
};
pub const PERSPECTIVE_HORIZONTAL: Slider = Slider {
    key: "PerspectiveHorizontal",
    units: 100.,
    group: TransformAdjustments,
};
pub const PERSPECTIVE_ROTATE: Slider = Slider {
    key: "PerspectiveRotate",
    units: 1.,
    group: TransformAdjustments,
};
pub const PERSPECTIVE_ASPECT: Slider = Slider {
    key: "PerspectiveAspect",
    units: 100.,
    group: TransformAdjustments,
};
pub const PERSPECTIVE_SCALE: Slider = Slider {
    key: "PerspectiveScale",
    units: 100.,
    group: TransformAdjustments,
};
pub const PERSPECTIVE_X: Slider = Slider {
    key: "PerspectiveX",
    units: 100.,
    group: TransformAdjustments,
};
pub const PERSPECTIVE_Y: Slider = Slider {
    key: "PerspectiveY",
    units: 100.,
    group: TransformAdjustments,
};

// Crop: the straightening angle in degrees.
pub const CROP_ANGLE: Slider = Slider {
    key: "CropAngle",
    units: 1.,
    group: Crop,
};

/// The color mixer's Hue, Saturation and Luminance of each band, `[band][control]`.
pub const HSL: [[Slider; 3]; 8] = [
    [
        Slider {
            key: "HueAdjustmentRed",
            units: 100.,
            group: ColorAdjustments,
        },
        Slider {
            key: "SaturationAdjustmentRed",
            units: 100.,
            group: ColorAdjustments,
        },
        Slider {
            key: "LuminanceAdjustmentRed",
            units: 100.,
            group: ColorAdjustments,
        },
    ],
    [
        Slider {
            key: "HueAdjustmentOrange",
            units: 100.,
            group: ColorAdjustments,
        },
        Slider {
            key: "SaturationAdjustmentOrange",
            units: 100.,
            group: ColorAdjustments,
        },
        Slider {
            key: "LuminanceAdjustmentOrange",
            units: 100.,
            group: ColorAdjustments,
        },
    ],
    [
        Slider {
            key: "HueAdjustmentYellow",
            units: 100.,
            group: ColorAdjustments,
        },
        Slider {
            key: "SaturationAdjustmentYellow",
            units: 100.,
            group: ColorAdjustments,
        },
        Slider {
            key: "LuminanceAdjustmentYellow",
            units: 100.,
            group: ColorAdjustments,
        },
    ],
    [
        Slider {
            key: "HueAdjustmentGreen",
            units: 100.,
            group: ColorAdjustments,
        },
        Slider {
            key: "SaturationAdjustmentGreen",
            units: 100.,
            group: ColorAdjustments,
        },
        Slider {
            key: "LuminanceAdjustmentGreen",
            units: 100.,
            group: ColorAdjustments,
        },
    ],
    [
        Slider {
            key: "HueAdjustmentAqua",
            units: 100.,
            group: ColorAdjustments,
        },
        Slider {
            key: "SaturationAdjustmentAqua",
            units: 100.,
            group: ColorAdjustments,
        },
        Slider {
            key: "LuminanceAdjustmentAqua",
            units: 100.,
            group: ColorAdjustments,
        },
    ],
    [
        Slider {
            key: "HueAdjustmentBlue",
            units: 100.,
            group: ColorAdjustments,
        },
        Slider {
            key: "SaturationAdjustmentBlue",
            units: 100.,
            group: ColorAdjustments,
        },
        Slider {
            key: "LuminanceAdjustmentBlue",
            units: 100.,
            group: ColorAdjustments,
        },
    ],
    [
        Slider {
            key: "HueAdjustmentPurple",
            units: 100.,
            group: ColorAdjustments,
        },
        Slider {
            key: "SaturationAdjustmentPurple",
            units: 100.,
            group: ColorAdjustments,
        },
        Slider {
            key: "LuminanceAdjustmentPurple",
            units: 100.,
            group: ColorAdjustments,
        },
    ],
    [
        Slider {
            key: "HueAdjustmentMagenta",
            units: 100.,
            group: ColorAdjustments,
        },
        Slider {
            key: "SaturationAdjustmentMagenta",
            units: 100.,
            group: ColorAdjustments,
        },
        Slider {
            key: "LuminanceAdjustmentMagenta",
            units: 100.,
            group: ColorAdjustments,
        },
    ],
];
/// The black and white mix of each band.
pub const GRAY_MIXER: [Slider; 8] = [
    Slider {
        key: "GrayMixerRed",
        units: 100.,
        group: BlackWhiteMix,
    },
    Slider {
        key: "GrayMixerOrange",
        units: 100.,
        group: BlackWhiteMix,
    },
    Slider {
        key: "GrayMixerYellow",
        units: 100.,
        group: BlackWhiteMix,
    },
    Slider {
        key: "GrayMixerGreen",
        units: 100.,
        group: BlackWhiteMix,
    },
    Slider {
        key: "GrayMixerAqua",
        units: 100.,
        group: BlackWhiteMix,
    },
    Slider {
        key: "GrayMixerBlue",
        units: 100.,
        group: BlackWhiteMix,
    },
    Slider {
        key: "GrayMixerPurple",
        units: 100.,
        group: BlackWhiteMix,
    },
    Slider {
        key: "GrayMixerMagenta",
        units: 100.,
        group: BlackWhiteMix,
    },
];
/// The parametric curve's regions: Shadows, Darks, Lights, Highlights.
pub const PARAMETRIC: [Slider; 4] = [
    Slider {
        key: "ParametricShadows",
        units: 100.,
        group: ToneCurve,
    },
    Slider {
        key: "ParametricDarks",
        units: 100.,
        group: ToneCurve,
    },
    Slider {
        key: "ParametricLights",
        units: 100.,
        group: ToneCurve,
    },
    Slider {
        key: "ParametricHighlights",
        units: 100.,
        group: ToneCurve,
    },
];
/// The parametric curve's splits: shadow, midtone, highlight.
pub const PARAMETRIC_SPLITS: [Slider; 3] = [
    Slider {
        key: "ParametricShadowSplit",
        units: 100.,
        group: ToneCurve,
    },
    Slider {
        key: "ParametricMidtoneSplit",
        units: 100.,
        group: ToneCurve,
    },
    Slider {
        key: "ParametricHighlightSplit",
        units: 100.,
        group: ToneCurve,
    },
];
/// Legacy Split Toning's hue of the shadows and highlights, in degrees.
pub const SPLIT_TONING_HUE: [Slider; 2] = [
    Slider {
        key: "SplitToningShadowHue",
        units: 360.,
        group: ColorGrading,
    },
    Slider {
        key: "SplitToningHighlightHue",
        units: 360.,
        group: ColorGrading,
    },
];
/// Legacy Split Toning's saturation of the shadows and highlights.
pub const SPLIT_TONING_SATURATION: [Slider; 2] = [
    Slider {
        key: "SplitToningShadowSaturation",
        units: 100.,
        group: ColorGrading,
    },
    Slider {
        key: "SplitToningHighlightSaturation",
        units: 100.,
        group: ColorGrading,
    },
];
/// Color Grading's luminance of the shadows and highlights.
pub const COLOR_GRADE_LUM: [Slider; 2] = [
    Slider {
        key: "ColorGradeShadowLum",
        units: 100.,
        group: ColorGrading,
    },
    Slider {
        key: "ColorGradeHighlightLum",
        units: 100.,
        group: ColorGrading,
    },
];
/// Color Grading's global hue (in degrees), saturation and luminance.
pub const COLOR_GRADE_GLOBAL: [Slider; 3] = [
    Slider {
        key: "ColorGradeGlobalHue",
        units: 360.,
        group: ColorGrading,
    },
    Slider {
        key: "ColorGradeGlobalSat",
        units: 100.,
        group: ColorGrading,
    },
    Slider {
        key: "ColorGradeGlobalLum",
        units: 100.,
        group: ColorGrading,
    },
];
/// Calibration's hue and saturation of the red, green and blue primaries.
pub const CALIBRATION: [[Slider; 2]; 3] = [
    [
        Slider {
            key: "RedHue",
            units: 100.,
            group: Calibration,
        },
        Slider {
            key: "RedSaturation",
            units: 100.,
            group: Calibration,
        },
    ],
    [
        Slider {
            key: "GreenHue",
            units: 100.,
            group: Calibration,
        },
        Slider {
            key: "GreenSaturation",
            units: 100.,
            group: Calibration,
        },
    ],
    [
        Slider {
            key: "BlueHue",
            units: 100.,
            group: Calibration,
        },
        Slider {
            key: "BlueSaturation",
            units: 100.,
            group: Calibration,
        },
    ],
];
/// Defringe's purple and green amounts (0 to 20).
pub const DEFRINGE_AMOUNT: [Slider; 2] = [
    Slider {
        key: "DefringePurpleAmount",
        units: 20.,
        group: ChromaticAberration,
    },
    Slider {
        key: "DefringeGreenAmount",
        units: 20.,
        group: ChromaticAberration,
    },
];
/// The low end of Defringe's purple and green hue ranges.
pub const DEFRINGE_HUE_LO: [Slider; 2] = [
    Slider {
        key: "DefringePurpleHueLo",
        units: 100.,
        group: ChromaticAberration,
    },
    Slider {
        key: "DefringeGreenHueLo",
        units: 100.,
        group: ChromaticAberration,
    },
];
/// The high end of Defringe's purple and green hue ranges.
pub const DEFRINGE_HUE_HI: [Slider; 2] = [
    Slider {
        key: "DefringePurpleHueHi",
        units: 100.,
        group: ChromaticAberration,
    },
    Slider {
        key: "DefringeGreenHueHi",
        units: 100.,
        group: ChromaticAberration,
    },
];
/// The crop's edges, as fractions of the frame: left, top, right, bottom.
pub const CROP: [Slider; 4] = [
    Slider {
        key: "CropLeft",
        units: 1.,
        group: Crop,
    },
    Slider {
        key: "CropTop",
        units: 1.,
        group: Crop,
    },
    Slider {
        key: "CropRight",
        units: 1.,
        group: Crop,
    },
    Slider {
        key: "CropBottom",
        units: 1.,
        group: Crop,
    },
];

/// High Dynamic Range's limit, in stops above graphics white
/// ([`crate::model::hdr::HdrSettings::limit`]).
pub const HDR_MAX_VALUE: Slider = Slider {
    key: "HDRMaxValue",
    units: 1.,
    group: HighDynamicRange,
};
/// The SDR rendition's sliders (-100 to +100), in the order of
/// [`crate::model::hdr::SdrRendition::values`].
pub const SDR_RENDITION: [Slider; 7] = [
    Slider {
        key: "SDRBrightness",
        units: 100.,
        group: HighDynamicRange,
    },
    Slider {
        key: "SDRContrast",
        units: 100.,
        group: HighDynamicRange,
    },
    Slider {
        key: "SDRClarity",
        units: 100.,
        group: HighDynamicRange,
    },
    Slider {
        key: "SDRHighlights",
        units: 100.,
        group: HighDynamicRange,
    },
    Slider {
        key: "SDRShadows",
        units: 100.,
        group: HighDynamicRange,
    },
    Slider {
        key: "SDRWhites",
        units: 100.,
        group: HighDynamicRange,
    },
    Slider {
        key: "SDRBlend",
        units: 100.,
        group: HighDynamicRange,
    },
];

/// The sliders that aren't part of a family.
const SINGLES: [Slider; 54] = [
    EXPOSURE,
    CONTRAST,
    HIGHLIGHTS,
    SHADOWS,
    WHITES,
    BLACKS,
    TEXTURE,
    CLARITY,
    DEHAZE,
    VIBRANCE,
    SATURATION,
    TEMPERATURE,
    TINT,
    INCREMENTAL_TEMPERATURE,
    INCREMENTAL_TINT,
    CURVE_REFINE_SATURATION,
    SHARPNESS,
    SHARPEN_RADIUS,
    SHARPEN_DETAIL,
    SHARPEN_EDGE_MASKING,
    LUMINANCE_SMOOTHING,
    LUMINANCE_NOISE_REDUCTION_DETAIL,
    LUMINANCE_NOISE_REDUCTION_CONTRAST,
    COLOR_NOISE_REDUCTION,
    COLOR_NOISE_REDUCTION_DETAIL,
    COLOR_NOISE_REDUCTION_SMOOTHNESS,
    SPLIT_TONING_BALANCE,
    COLOR_GRADE_MIDTONE_HUE,
    COLOR_GRADE_MIDTONE_SAT,
    COLOR_GRADE_MIDTONE_LUM,
    COLOR_GRADE_BLENDING,
    SHADOW_TINT,
    GRAIN_AMOUNT,
    GRAIN_SIZE,
    GRAIN_FREQUENCY,
    POST_CROP_VIGNETTE_AMOUNT,
    POST_CROP_VIGNETTE_MIDPOINT,
    POST_CROP_VIGNETTE_FEATHER,
    POST_CROP_VIGNETTE_ROUNDNESS,
    POST_CROP_VIGNETTE_HIGHLIGHT_CONTRAST,
    VIGNETTE_AMOUNT,
    VIGNETTE_MIDPOINT,
    LENS_MANUAL_DISTORTION_AMOUNT,
    LENS_PROFILE_DISTORTION_SCALE,
    LENS_PROFILE_VIGNETTING_SCALE,
    PERSPECTIVE_VERTICAL,
    PERSPECTIVE_HORIZONTAL,
    PERSPECTIVE_ROTATE,
    PERSPECTIVE_ASPECT,
    PERSPECTIVE_SCALE,
    PERSPECTIVE_X,
    PERSPECTIVE_Y,
    CROP_ANGLE,
    HDR_MAX_VALUE,
];

/// The slider whose key is `key`, if `key` is one.
pub fn slider(key: &str) -> Option<Slider> {
    every().find(|s| s.key == key)
}

/// Every slider: the single ones, then the families.
fn every() -> impl Iterator<Item = Slider> {
    SINGLES
        .into_iter()
        .chain(HSL.into_iter().flatten())
        .chain(GRAY_MIXER)
        .chain(PARAMETRIC)
        .chain(PARAMETRIC_SPLITS)
        .chain(SPLIT_TONING_HUE)
        .chain(SPLIT_TONING_SATURATION)
        .chain(COLOR_GRADE_LUM)
        .chain(COLOR_GRADE_GLOBAL)
        .chain(CALIBRATION.into_iter().flatten())
        .chain(DEFRINGE_AMOUNT)
        .chain(DEFRINGE_HUE_LO)
        .chain(DEFRINGE_HUE_HI)
        .chain(CROP)
        .chain(SDR_RENDITION)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The scale readers multiply by and writers divide by is the same f32 as the
    /// decimal they were written with before the table (0.01, 1/150, 1/360, 0.05).
    #[test]
    fn scales_are_the_decimals_they_stand_for() {
        for s in every() {
            let expected = match s.units {
                1. => 1.,
                100. => 0.01,
                150. => 1. / 150.,
                360. => 1. / 360.,
                20. => 0.05,
                other => panic!("{} has units {other}", s.key),
            };
            assert_eq!(s.scale().to_bits(), f32::to_bits(expected), "{}", s.key);
        }
        assert_eq!(CONTRAST.scale().to_bits(), 0.01f32.to_bits());
    }

    /// Every key names one slider, which `slider` finds.
    #[test]
    fn keys_are_unique_and_found() {
        let mut seen = std::collections::BTreeSet::new();
        for s in every() {
            assert!(seen.insert(s.key), "{} twice", s.key);
            assert_eq!(slider(s.key), Some(s));
        }
        assert_eq!(seen.len(), every().count());
        assert_eq!(slider("crs:Exposure2012"), None);
        assert_eq!(slider("Exposure"), None);
    }
}
