use super::panels::PanelSwitches;
use crate::model::white_balance::{estimate_temperature, illuminant_camera};
use crate::{camera_data::Metadata, color::curve::ToneCurve};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
/// Lightroom's white balance slider ranges for RAW files.
pub const TEMPERATURE_MIN: f32 = 2000.;
pub const TEMPERATURE_MAX: f32 = 50000.;
pub const TINT_LIMIT: f32 = 150.;
/// The Exposure a recipe may hold, in EV. The slider spans ±5; imported edits and
/// typed values may go further.
pub const EXPOSURE_LIMIT: f32 = 8.;
/// A photo's develop settings. Fields this build does not know (from a newer release)
/// are kept in `unknown` and saved again, so an older build never drops them.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Recipe {
    pub engine: u32,
    /// Apply the lens correction the camera stored in the RAW (engine 4). Missing in older
    /// recipes, which therefore keep rendering without it.
    #[serde(default)]
    pub lens_builtin: bool,
    /// Lightroom's Enable Profile Corrections: use an imported Adobe lens profile that
    /// matches the lens, in place of the built-in correction.
    #[serde(default)]
    pub lens_profile: bool,
    /// Lightroom's profile Setup and the profile the edit names. Omitted at Default
    /// with no profile named, so releases that predate it read the recipe.
    #[serde(
        default,
        skip_serializing_if = "crate::lens::choice::LensProfileChoice::is_default"
    )]
    pub lens_profile_choice: crate::lens::choice::LensProfileChoice,
    /// Profile correction amounts, Lightroom's Distortion and Vignetting sliders
    /// (0–2, 1 = 100).
    #[serde(default = "one")]
    pub lens_distortion: f32,
    #[serde(default = "one")]
    pub lens_vignetting: f32,
    /// Lightroom's manual Distortion (Lens Corrections > Manual,
    /// `crs:LensManualDistortionAmount` / 100, −1 to 1): positive corrects barrel
    /// distortion, negative pincushion. Omitted at 0, so releases that predate it read
    /// the recipe (and keep the field when it is set).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub lens_manual_distortion: f32,
    /// Lightroom's Remove Chromatic Aberration: red and blue fringing measured from the
    /// photo itself, in place of any lens data's lateral CA (`crs:AutoLateralCA`).
    #[serde(default)]
    pub lens_ca: bool,
    /// Use the DCP tone curve without a second generic scene shoulder.
    #[serde(default)]
    pub profile_tone: bool,
    pub effects: crate::model::effects::Effects,
    /// Which operator renders Grain. Missing means the original grain, so recipes
    /// saved before the measured one look as they did; omitted at that default, and
    /// kept by releases that predate it.
    #[serde(
        default,
        skip_serializing_if = "crate::model::operators::GrainModel::is_original"
    )]
    pub grain_model: crate::model::operators::GrainModel,
    /// Which operator renders positive Clarity, as `grain_model`.
    #[serde(
        default,
        skip_serializing_if = "crate::model::operators::ClarityModel::is_original"
    )]
    pub clarity_model: crate::model::operators::ClarityModel,
    /// Which operator renders Texture, as `grain_model`.
    #[serde(
        default,
        skip_serializing_if = "crate::model::operators::TextureModel::is_original"
    )]
    pub texture_model: crate::model::operators::TextureModel,
    /// Effects > Film Grain, RAWmakase's own. Kept outside `effects` and omitted at
    /// its defaults, so releases that predate it read the recipe and keep it.
    #[serde(
        default,
        skip_serializing_if = "crate::model::film_grain::FilmGrain::is_default"
    )]
    pub film_grain: crate::model::film_grain::FilmGrain,
    pub preset_name: String,
    pub preset_settings: std::collections::BTreeMap<String, String>,
    pub profile: Option<std::sync::Arc<crate::camera_profiles::CameraProfile>>,
    /// Lightroom's Profile Amount (`crs:Look` `Amount`, 0–2, 1 = 100%): the strength of
    /// a look that supports it. Other profiles render at 100% whatever it is. Omitted
    /// at 1, so releases that predate it read the recipe.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub profile_amount: f32,
    /// Which operator renders Sharpening. Missing means the original unsharp mask, so
    /// recipes saved before the measured one look as they did; omitted at that
    /// default, and kept by releases that predate it.
    #[serde(
        default,
        skip_serializing_if = "crate::model::operators::SharpeningModel::is_original"
    )]
    pub sharpening_model: crate::model::operators::SharpeningModel,
    pub sharpening_radius: f32,
    pub sharpening_detail: f32,
    pub sharpening_masking: f32,
    pub exposure: f32,
    #[serde(default)]
    pub camera_exposure: f32,
    #[serde(default)]
    pub wide_gamut_curves: bool,
    #[serde(default)]
    pub reference_curves: bool,
    #[serde(default)]
    pub reference_calibration: bool,
    /// RGB-hue grading and reference-calibrated color response. Missing means legacy.
    #[serde(default)]
    pub reference_color: bool,
    /// How the Tone Curve's parametric regions render. Missing means the original
    /// approximation, so recipes saved before the measured curve look as they did;
    /// omitted at that default, and kept by releases that predate it.
    #[serde(
        default,
        skip_serializing_if = "crate::model::operators::ParametricModel::is_original"
    )]
    pub parametric_model: crate::model::operators::ParametricModel,
    /// Whether Contrast pivots where the photo puts it, after Whites and Blacks.
    /// Missing means the original averaged curve before them, so older recipes look as
    /// they did; omitted at that default, and kept by releases that predate it.
    #[serde(
        default,
        skip_serializing_if = "crate::model::operators::ContrastModel::is_original"
    )]
    pub contrast_model: crate::model::operators::ContrastModel,
    /// How manual lens Vignetting renders. Missing means the original operator, so
    /// recipes saved before the measured one look as they did; omitted at that
    /// default, and kept by releases that predate it.
    #[serde(
        default,
        skip_serializing_if = "crate::model::operators::LensVignetteModel::is_original"
    )]
    pub lens_vignette_model: crate::model::operators::LensVignetteModel,
    /// The soft edge Heal and Clone render with. Missing means the original one, so
    /// recipes saved before the measured feather look as they did; omitted at that
    /// default, and kept by releases that predate it.
    #[serde(
        default,
        skip_serializing_if = "crate::model::operators::RetouchModel::is_original"
    )]
    pub retouch_model: crate::model::operators::RetouchModel,
    /// How color grading renders. Missing means the original operator, so recipes
    /// saved before the measured curves look as they did; omitted at that default.
    #[serde(
        default,
        skip_serializing_if = "crate::model::operators::GradingModel::is_original"
    )]
    pub grading_model: crate::model::operators::GradingModel,
    /// Which measured tables render the color mixer. Missing means the tables
    /// measured on photos, so older recipes look as they did; omitted at that default.
    #[serde(
        default,
        skip_serializing_if = "crate::model::operators::MixerModel::is_original"
    )]
    pub mixer_model: crate::model::operators::MixerModel,
    /// Which operator renders the global Saturation slider. Missing means the
    /// photo-measured tables, so older recipes look as they did; omitted at that default.
    #[serde(
        default,
        skip_serializing_if = "crate::model::operators::SaturationModel::is_original"
    )]
    pub saturation_model: crate::model::operators::SaturationModel,
    /// Which tables render Vibrance. Missing means the photo-measured ones, so older
    /// recipes look as they did; omitted at that default.
    #[serde(
        default,
        skip_serializing_if = "crate::model::operators::VibranceModel::is_original"
    )]
    pub vibrance_model: crate::model::operators::VibranceModel,
    /// Which operator renders the black & white mix. Missing means the Oklab
    /// lightness shift, so older recipes look as they did; omitted at that default.
    #[serde(
        default,
        skip_serializing_if = "crate::model::operators::BlackWhiteModel::is_original"
    )]
    pub black_white_model: crate::model::operators::BlackWhiteModel,
    /// Which fit renders Camera Calibration's primary sliders. Missing means the
    /// original coefficients, so older recipes look as they did; omitted at that default.
    #[serde(
        default,
        skip_serializing_if = "crate::model::operators::CalibrationModel::is_original"
    )]
    pub calibration_model: crate::model::operators::CalibrationModel,
    /// Whether positive Whites follows the photo's highlights. Missing means the
    /// original median curve, so older recipes look as they did; omitted at that
    /// default, and kept by releases that predate it.
    #[serde(
        default,
        skip_serializing_if = "crate::model::operators::WhitesModel::is_original"
    )]
    pub whites_model: crate::model::operators::WhitesModel,
    /// Saved independently of gains so the next control edit uses the same calibration.
    #[serde(
        default,
        skip_serializing_if = "crate::model::operators::WhiteBalanceModel::is_original"
    )]
    pub white_balance_model: crate::model::operators::WhiteBalanceModel,
    /// How out-of-gamut colors reach sRGB. Missing means compressed, so recipes saved
    /// before the clipped model look as they did; omitted at that default.
    #[serde(
        default,
        skip_serializing_if = "crate::model::operators::GamutModel::is_compress"
    )]
    pub gamut_model: crate::model::operators::GamutModel,
    /// How Color noise reduction renders. Missing means the original operator, so
    /// recipes saved before the measured one look as they did; omitted at that
    /// default, and kept by releases that predate it.
    #[serde(
        default,
        skip_serializing_if = "crate::model::operators::NoiseModel::is_original"
    )]
    pub noise_model: crate::model::operators::NoiseModel,
    pub temperature: f32,
    pub tint: f32,
    pub wb: [f32; 3],
    /// Temperature and Tint that Auto white balance chose, so the WB menu shows Auto
    /// while the photo still has them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_white_balance: Option<[f32; 2]>,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub black_point: f32,
    pub white_point: f32,
    pub midtone: f32,
    pub curve: ToneCurve,
    /// Lightroom's Refine Saturation (`crs:CurveRefineSaturation` / 100): how much of the
    /// saturation the master point curve adds or removes is kept. 1 is Lightroom's
    /// default; Camera Raw renders values above 1 as 1. Omitted at 1, so releases that
    /// predate it read the recipe (and keep the field when it is set).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub curve_saturation: f32,
    pub saturation: f32,
    pub vibrance: f32,
    pub hsl: [[f32; 3]; 8],
    /// Lightroom's Point Color swatches, at most eight (`crs:PointColors` and
    /// `crs:ColorVariance`). Omitted when empty, so releases that predate it read the
    /// recipe (and keep the swatches when they are set).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub point_colors: Vec<crate::model::point_color::PointColor>,
    pub grading: [[f32; 3]; 3],
    pub noise_luma: f32,
    pub noise_chroma: f32,
    pub sharpening: f32,
    pub crop: [f32; 4],
    pub straighten: f32,
    /// Lightroom's Constrain Crop (`crs:CropConstrainToWarp`): the crop as rendered
    /// shrinks, keeping its aspect, to leave out the white areas Upright, the Transform
    /// sliders and manual Distortion uncover. `crop` stays as the user set it. Omitted
    /// when off, so releases that predate it read the recipe.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub constrain_crop: bool,
    /// Transform panel sliders (engine 4).
    #[serde(default)]
    pub transform: crate::model::transform::Transform,
    /// Lightroom's Upright (engine 4), applied before the Transform sliders.
    #[serde(
        default,
        skip_serializing_if = "crate::model::transform::Upright::is_default"
    )]
    pub upright: crate::model::transform::Upright,
    pub rotation: u8,
    pub flip_x: bool,
    pub flip_y: bool,
    /// Heal and Clone operations, in order. Saved apart from the recipe (see
    /// [`LocalEdits`]); omitted from recipe JSON when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retouch: Vec<crate::model::retouch::RetouchOp>,
    /// Red eye corrections, in order; saved apart, as `retouch`.
    #[serde(
        default,
        skip_serializing_if = "crate::model::red_eye::RedEyeList::is_blank"
    )]
    pub red_eye: crate::model::red_eye::RedEyeList,
    /// Masks with local adjustments; saved apart, as `retouch`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub masks: Vec<crate::model::masks::MaskGroup>,
    /// Lightroom's panel switches; omitted while every panel is on.
    #[serde(default, skip_serializing_if = "PanelSwitches::all_on")]
    pub panels: PanelSwitches,
    /// Settings from a newer release, preserved as they were.
    #[serde(flatten)]
    pub unknown: std::collections::BTreeMap<String, serde_json::Value>,
}
/// A recipe's spot removal, red eye corrections and masks (experimental). They are saved beside the recipe,
/// not in it, so releases that predate them still read every other setting.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LocalEdits {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub retouch: Vec<crate::model::retouch::RetouchOp>,
    #[serde(skip_serializing_if = "crate::model::red_eye::RedEyeList::is_blank")]
    pub red_eye: crate::model::red_eye::RedEyeList,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub masks: Vec<crate::model::masks::MaskGroup>,
}
impl LocalEdits {
    pub fn is_empty(&self) -> bool {
        self.retouch.is_empty() && self.red_eye.is_blank() && self.masks.is_empty()
    }
    pub fn validate(&self) -> Result<()> {
        crate::model::retouch::validate(&self.retouch)?;
        crate::model::red_eye::validate(&self.red_eye)?;
        crate::model::masks::validate(&self.masks)
    }
}
impl Default for Recipe {
    fn default() -> Self {
        Self {
            engine: 4,
            lens_builtin: true,
            lens_profile: false,
            lens_profile_choice: Default::default(),
            lens_distortion: 1.,
            lens_vignetting: 1.,
            lens_manual_distortion: 0.,
            lens_ca: false,
            profile_tone: true,
            effects: Default::default(),
            grain_model: Default::default(),
            clarity_model: Default::default(),
            texture_model: Default::default(),
            film_grain: Default::default(),
            preset_name: String::new(),
            preset_settings: Default::default(),
            profile: None,
            sharpening_model: Default::default(),
            sharpening_radius: 0.8,
            sharpening_detail: 0.25,
            sharpening_masking: 0.35,
            exposure: 0.,
            camera_exposure: 0.,
            wide_gamut_curves: false,
            reference_curves: false,
            reference_calibration: false,
            reference_color: false,
            parametric_model: Default::default(),
            contrast_model: Default::default(),
            lens_vignette_model: Default::default(),
            retouch_model: Default::default(),
            grading_model: Default::default(),
            mixer_model: Default::default(),
            saturation_model: Default::default(),
            vibrance_model: Default::default(),
            black_white_model: Default::default(),
            calibration_model: Default::default(),
            whites_model: Default::default(),
            white_balance_model: Default::default(),
            gamut_model: Default::default(),
            temperature: 6500.,
            tint: 0.,
            wb: [1.; 3],
            auto_white_balance: None,
            contrast: 0.,
            highlights: 0.,
            shadows: 0.,
            whites: 0.,
            blacks: 0.,
            black_point: 0.,
            white_point: 1.,
            midtone: 1.,
            curve: ToneCurve::default(),
            curve_saturation: 1.,
            profile_amount: 1.,
            saturation: 0.,
            vibrance: 0.,
            hsl: [[0.; 3]; 8],
            point_colors: Vec::new(),
            grading: [[0.; 3]; 3],
            noise_luma: 0.,
            noise_chroma: 0.,
            noise_model: Default::default(),
            sharpening: 0.35,
            crop: [0., 0., 1., 1.],
            straighten: 0.,
            constrain_crop: false,
            transform: Default::default(),
            upright: Default::default(),
            rotation: 0,
            flip_x: false,
            flip_y: false,
            retouch: Vec::new(),
            red_eye: Default::default(),
            masks: Vec::new(),
            panels: PanelSwitches::default(),
            unknown: Default::default(),
        }
    }
}
/// Which profile a photo's starting settings use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfilePreference {
    /// Adobe Color, else Adobe Standard, else the DNG's own, else RAWmakase Color.
    Adobe,
    /// RAWmakase Color wherever it fits the camera, else as `Adobe`.
    Rawmakase,
    /// Lightroom's Camera Settings: the imported profile matching the camera's
    /// standard look (see [`camera_matching_profile`]), else as `Adobe`.
    Camera,
}
/// Names of the camera-matching profile Adobe ships for a camera's standard
/// look, by maker. RAWmakase doesn't read the picture style set in the camera,
/// so Camera Settings always starts from this one.
pub fn camera_matching_names(m: &Metadata) -> &'static [&'static str] {
    let make = m.make.trim().to_ascii_lowercase();
    if make.starts_with("fujifilm") {
        &["Camera PROVIA/Standard"]
    } else {
        &["Camera Standard"]
    }
}
/// The imported camera-matching profile for this camera's standard look.
pub fn camera_matching_profile<'a>(
    m: &Metadata,
    profiles: &'a [std::sync::Arc<crate::camera_profiles::CameraProfile>],
) -> Option<&'a std::sync::Arc<crate::camera_profiles::CameraProfile>> {
    camera_matching_names(m).iter().find_map(|name| {
        profiles
            .iter()
            .find(|p| p.name == *name && p.ensure_camera(m).is_ok())
    })
}
impl Recipe {
    /// The Treatment the photo renders with: Black & White by its own setting or by a
    /// black & white profile.
    pub fn treatment(&self) -> Treatment {
        // As `with_profile_adjustments` renders it, without building the look.
        if self.effects.monochrome || (self.engine >= 3 && is_monochrome(self.profile.as_deref())) {
            Treatment::BlackWhite
        } else {
            Treatment::Color
        }
    }

    /// The look at its Profile Amount, and its internal controls added to the user's
    /// sliders without changing them.
    pub fn with_profile_adjustments(&self) -> std::borrow::Cow<'_, Self> {
        let Some((profile, look)) = self
            .profile
            .as_ref()
            .and_then(|p| Some((p, p.enhanced.as_ref()?)))
            .filter(|_| self.engine >= 3)
        else {
            return std::borrow::Cow::Borrowed(self);
        };
        let amount = if look.amount.is_some() {
            self.profile_amount
        } else {
            1.
        };
        let adjustments = [
            look.highlights,
            look.shadows,
            look.clarity,
            look.contrast,
            look.blacks,
        ];
        if amount == 1.
            && adjustments.iter().all(|v| *v == 0.)
            && !look.monochrome
            && look.settings.is_default()
        {
            return std::borrow::Cow::Borrowed(self);
        }
        let mut r = self.clone();
        let mut look = look.clone();
        if amount != 1. {
            let profile = profile.at_amount(amount);
            look = profile.enhanced.clone().unwrap_or(look);
            r.profile = Some(std::sync::Arc::new(profile));
            r.profile_amount = 1.;
        }
        r.highlights = (r.highlights + look.highlights).clamp(-1., 1.);
        r.shadows = (r.shadows + look.shadows).clamp(-1., 1.);
        r.effects.clarity = (r.effects.clarity + look.clarity).clamp(-1., 1.);
        r.contrast = (r.contrast + look.contrast).clamp(-1., 1.);
        r.blacks = (r.blacks + look.blacks).clamp(-1., 1.);
        r.effects.monochrome |= look.monochrome;
        r.add_look_settings(&look.settings);
        std::borrow::Cow::Owned(r)
    }
    /// A look's Exposure, Saturation, colour mixer and parametric curve added to the
    /// user's; its split toning for the shadows or highlights the user doesn't tone,
    /// and its vignette in place of the user's, as Camera Raw 18.7 renders them.
    fn add_look_settings(&mut self, s: &crate::camera_profiles::LookSettings) {
        let add = |a: f32, b: f32| (a + b).clamp(-1., 1.);
        self.exposure = (self.exposure + s.exposure).clamp(-8., 8.);
        self.saturation = add(self.saturation, s.saturation);
        for (band, look) in self.hsl.iter_mut().zip(s.hsl) {
            for (v, l) in band.iter_mut().zip(look) {
                *v = add(*v, l);
            }
        }
        let e = &mut self.effects;
        // The layered curve renders a look's parametric curve as a curve of its own,
        // on the measured path only; elsewhere the look's regions join the user's.
        let merge_parametric = !(self.parametric_model
            == crate::model::operators::ParametricModel::Layered
            && self.engine >= 4
            && self.reference_curves);
        if s.parametric != [0.; 4] && merge_parametric {
            for (v, l) in e.parametric.iter_mut().zip(s.parametric) {
                *v = add(*v, l);
            }
            if e.splits == crate::model::effects::Effects::default().splits {
                e.splits = s.splits;
            }
        }
        // The measured grading renders a look's split toning as a pass of its own.
        let merge_toning = self.grading_model == crate::model::operators::GradingModel::Original;
        if let Some(t) = s.toning.filter(|_| merge_toning) {
            // Split toning, as Lightroom's looks store it, overlaps all tones; the
            // user's own toning of shadows or highlights wins over the look's.
            if self.grading.iter().all(|g| g[1] == 0. && g[2] == 0.) && e.global_grade == [0.; 3] {
                e.blending = 1.;
            }
            for (slot, [hue, saturation]) in [(0, t.shadows), (2, t.highlights)] {
                if self.grading[slot][1] == 0. {
                    self.grading[slot] = [hue, saturation, self.grading[slot][2]];
                }
            }
            if e.balance == 0. {
                e.balance = t.balance;
            }
        }
        if let Some(v) = s.vignette {
            e.vignette = v.amount.clamp(-1., 1.);
            e.vignette_midpoint = v.midpoint;
            e.vignette_feather = v.feather;
            e.vignette_roundness = v.roundness;
            e.vignette_highlights = v.highlights;
            e.vignette_style = v.style.try_into().unwrap_or_default();
        }
    }

    pub fn for_metadata(m: &Metadata) -> Self {
        Self {
            profile: crate::camera_profiles::builtin(m),
            temperature: estimate_temperature(m),
            lens_builtin: m.lens.as_ref().is_none_or(|l| l.default_on),
            ..Default::default()
        }
    }
    /// Lightroom's Adobe Default: the settings a photo starts from.
    pub fn with_profiles(
        m: &Metadata,
        profiles: &[std::sync::Arc<crate::camera_profiles::CameraProfile>],
    ) -> Self {
        Self::with_profile_preference(m, profiles, ProfilePreference::Adobe)
    }
    /// The starting settings, with the profile `preference` picks.
    pub fn with_profile_preference(
        m: &Metadata,
        profiles: &[std::sync::Arc<crate::camera_profiles::CameraProfile>],
        preference: ProfilePreference,
    ) -> Self {
        let mut recipe = Self::for_metadata(m);
        let find = |name: &str| {
            profiles
                .iter()
                .find(|p| p.name == name && p.ensure_camera(m).is_ok())
        };
        let own = match preference {
            ProfilePreference::Adobe => None,
            ProfilePreference::Rawmakase => find(crate::camera_profiles::open::COLOR),
            ProfilePreference::Camera => camera_matching_profile(m, profiles),
        };
        // As in Lightroom: Adobe Color, else Adobe Standard. Without those, a DNG
        // keeps the profile it embeds, and any other file gets RAWmakase Color.
        if let Some(profile) = own
            .or_else(|| find("Adobe Color"))
            .or_else(|| find("Adobe Standard"))
            .or_else(|| {
                recipe
                    .profile
                    .is_none()
                    .then(|| find(crate::camera_profiles::open::COLOR))
                    .flatten()
            })
        {
            recipe.profile = Some(profile.clone());
        }
        recipe.wide_gamut_curves = recipe.profile.is_some();
        recipe.reference_color = true;
        recipe.reference_curves = true;
        recipe.reference_calibration = true;
        recipe.parametric_model = crate::model::operators::ParametricModel::Layered;
        recipe.set_sharpening_defaults(crate::model::operators::SharpeningModel::Measured);
        recipe.grain_model = crate::model::operators::GrainModel::Measured;
        recipe.clarity_model = crate::model::operators::ClarityModel::Measured;
        recipe.texture_model = crate::model::operators::TextureModel::Measured;
        recipe.contrast_model = crate::model::operators::ContrastModel::Adaptive;
        recipe.lens_vignette_model = crate::model::operators::LensVignetteModel::Measured;
        recipe.retouch_model = crate::model::operators::RetouchModel::Measured;
        recipe.grading_model = crate::model::operators::GradingModel::Measured;
        recipe.mixer_model = crate::model::operators::MixerModel::Chart;
        recipe.saturation_model = crate::model::operators::SaturationModel::Gray;
        recipe.vibrance_model = crate::model::operators::VibranceModel::Chart;
        recipe.black_white_model = crate::model::operators::BlackWhiteModel::Chart;
        recipe.calibration_model = crate::model::operators::CalibrationModel::Measured;
        recipe.white_balance_model = crate::model::operators::WhiteBalanceModel::Calibrated;
        recipe.whites_model = crate::model::operators::WhitesModel::Adaptive;
        recipe.gamut_model = crate::model::operators::GamutModel::Clip;
        recipe.set_color_noise_defaults(crate::model::operators::NoiseModel::Measured);
        recipe.use_camera_baseline(m);
        recipe.reset_white_balance(m);
        recipe
    }
    pub fn validate(&self) -> Result<()> {
        self.effects.validate()?;
        self.film_grain.validate()?;
        ensure!(
            (1..=4).contains(&self.engine),
            "Unsupported rendering engine"
        );
        ensure!(
            (0.5..=3.).contains(&self.sharpening_radius)
                && (0. ..=1.).contains(&self.sharpening_detail)
                && (0. ..=1.).contains(&self.sharpening_masking),
            "Invalid sharpening controls"
        );
        if let Some(p) = &self.profile {
            p.validate()?;
        }
        ensure!(
            (-EXPOSURE_LIMIT..=EXPOSURE_LIMIT).contains(&self.exposure)
                && self.camera_exposure.is_finite()
                && self.camera_exposure.abs() <= 5.,
            "Exposure out of bounds"
        );
        ensure!(
            (TEMPERATURE_MIN..=TEMPERATURE_MAX).contains(&self.temperature)
                && self.tint.abs() <= TINT_LIMIT,
            "White balance out of bounds"
        );
        ensure!(
            self.wb
                .iter()
                .all(|v| v.is_finite() && *v >= 0.01 && *v <= 100.),
            "Invalid white balance"
        );
        for v in [
            self.contrast,
            self.highlights,
            self.shadows,
            self.whites,
            self.blacks,
            self.saturation,
            self.vibrance,
        ] {
            ensure!(v.abs() <= 1., "Adjustment out of bounds");
        }
        ensure!(
            self.black_point >= 0.
                && self.white_point <= 1.
                && self.white_point - self.black_point >= 0.01
                && (0.1..=4.).contains(&self.midtone),
            "Invalid levels"
        );
        self.curve.validate()?;
        ensure!(
            self.hsl
                .iter()
                .flatten()
                .chain(self.grading.iter().flatten())
                .all(|v| v.abs() <= 1.),
            "Invalid color adjustment"
        );
        ensure!(
            [self.noise_luma, self.noise_chroma, self.sharpening]
                .iter()
                .all(|v| (0. ..=1.).contains(v)),
            "Invalid detail adjustment"
        );
        ensure!(
            self.crop.iter().all(|v| (0. ..=1.).contains(v))
                && self.crop[2] - self.crop[0] >= 0.01
                && self.crop[3] - self.crop[1] >= 0.01,
            "Invalid crop"
        );
        ensure!(
            self.rotation < 4 && self.straighten.abs() <= 45.,
            "Invalid rotation"
        );
        ensure!(self.transform.validate(), "Invalid transform");
        ensure!(self.upright.validate(), "Invalid Upright");
        ensure!(
            (0. ..=2.).contains(&self.lens_distortion)
                && (0. ..=2.).contains(&self.lens_vignetting)
                && (-1. ..=1.).contains(&self.lens_manual_distortion),
            "Invalid lens correction amount"
        );
        ensure!(
            (0. ..=2.).contains(&self.curve_saturation),
            "Invalid Refine Saturation"
        );
        ensure!(
            (0. ..=2.).contains(&self.profile_amount),
            "Invalid Profile Amount"
        );
        // Every other number is range-checked above, which also rejects NaN.
        crate::model::retouch::validate(&self.retouch)?;
        crate::model::red_eye::validate(&self.red_eye)?;
        crate::model::masks::validate(&self.masks)?;
        Ok(())
    }
    /// The content IDs of the mask rasters this recipe refers to.
    pub fn mask_asset_ids(&self) -> impl Iterator<Item = &str> {
        crate::model::masks::bitmap_ids(&self.masks)
    }
    /// The recipe as saved, without spots and masks, and those apart.
    pub fn split_local(&self) -> (Recipe, LocalEdits) {
        let mut saved = self.clone();
        let local = LocalEdits {
            retouch: std::mem::take(&mut saved.retouch),
            red_eye: std::mem::take(&mut saved.red_eye),
            masks: std::mem::take(&mut saved.masks),
        };
        (saved, local)
    }
    /// Adds spots and masks saved apart. Ones already in the recipe (written by
    /// development builds into the recipe itself) stay when `local` has none.
    pub fn with_local(mut self, local: LocalEdits) -> Recipe {
        if !local.retouch.is_empty() {
            self.retouch = local.retouch;
        }
        if !local.red_eye.is_blank() {
            self.red_eye = local.red_eye;
        }
        if !local.masks.is_empty() {
            self.masks = local.masks;
        }
        self
    }
    /// The camera profile used for rendering. From engine 4, photos without an imported or
    /// bundled profile render through the DNG ColorMatrix default instead of the legacy path.
    pub fn color_profile(
        &self,
        m: &Metadata,
    ) -> Option<std::borrow::Cow<'_, crate::camera_profiles::CameraProfile>> {
        match &self.profile {
            Some(p) => Some(std::borrow::Cow::Borrowed(p.as_ref())),
            None if self.engine >= 4 => {
                crate::camera_profiles::CameraProfile::camera_matrix_default(m)
                    .map(std::borrow::Cow::Owned)
            }
            None => None,
        }
    }
    /// Lightroom's Enable Profile Corrections: an imported Adobe lens profile, else the
    /// correction the camera stored in the RAW. Built-in data that is off by default
    /// (Sony's) follows the switch; Fuji's and a DNG's stay on, as in Lightroom.
    pub fn set_profile_corrections(&mut self, m: &Metadata, state: ProfileCorrections) {
        self.lens_profile = state == ProfileCorrections::On;
        // Lens corrections render from process version 4 only.
        if self.engine >= 4 && m.lens.as_ref().is_some_and(|l| !l.default_on) {
            self.lens_builtin = self.lens_profile;
        }
    }
    /// The imported Adobe profile Enable Profile Corrections uses here, and the one
    /// the edit names when it isn't imported.
    pub fn lens_profile_in_use<'c, 'p>(
        &'c self,
        m: &'p Metadata,
    ) -> crate::lens::choice::Resolved<'c, 'p> {
        if !self.lens_profile || self.engine < 4 {
            return Default::default();
        }
        self.lens_profile_choice.resolve(&m.lens_profiles, m)
    }
    /// Why Enable Profile Corrections cannot render with the Adobe profile it should
    /// here, and what renders instead, when it is on.
    pub fn missing_lens_profile(&self, m: &Metadata) -> Option<String> {
        // A Lens Corrections panel switched off renders no lens correction at all.
        let panel = self
            .panels
            .state(crate::model::panels::Panel::LensCorrections);
        if panel == crate::model::panels::PanelState::Off || !self.lens_profile || self.engine < 4 {
            return None;
        }
        let resolved = self.lens_profile_in_use(m);
        // The profile the RAW carries renders as its built-in correction.
        if let Some(id) = self
            .lens_profile_choice
            .id
            .as_ref()
            .filter(|id| id.embedded)
        {
            return m.lens.as_ref().filter(|_| self.lens_builtin).is_none().then(|| {
                format!(
                    "Lens profile \"{}\" comes with the camera, but this file has none; no lens correction",
                    id.label()
                )
            });
        }
        let profile = match (resolved.missing, m.lens_model.as_str()) {
            (Some(id), _) => format!("Lens profile \"{}\"", id.label()),
            (None, _) if resolved.used.is_some() => return None,
            (None, "") => "Adobe lens profile for this lens".to_string(),
            (None, lens) => format!("Adobe lens profile for {lens}"),
        };
        let instead = match (resolved.used, m.lens.as_ref().filter(|_| self.lens_builtin)) {
            (Some(used), _) => format!("using {}", used.profile.name),
            (None, Some(builtin)) => format!("using {}", builtin.source),
            (None, None) => "no lens correction".to_string(),
        };
        Some(format!("{profile} isn't imported; {instead}"))
    }
    /// Adds a Heal or Clone operation. The first on a recipe has no spots of the
    /// original feather to keep, so it takes the measured one.
    pub fn add_retouch(&mut self, op: crate::model::retouch::RetouchOp) {
        if self.retouch.is_empty() {
            self.retouch_model = crate::model::operators::RetouchModel::Measured;
        }
        self.retouch.push(op);
    }
    /// After an edit of manual Vignetting from Amount `previous`: an Amount moved from 0
    /// has nothing of the original operator's to keep, so it takes the measured one.
    pub fn adopt_measured_vignette(&mut self, previous: f32) {
        if previous == 0. && self.effects.lens_vignette != 0. {
            self.lens_vignette_model = crate::model::operators::LensVignetteModel::Measured;
        }
    }
    /// The manual lens Vignetting Amount the finishing stage applies: only the
    /// original operator's; the measured one is applied with the lens profile.
    pub fn finished_lens_vignette(&self) -> f32 {
        if self.lens_vignette_model.is_original() {
            self.effects.lens_vignette
        } else {
            0.
        }
    }
    /// Calibration's Update: the current process, with the camera's built-in profile
    /// and the sharpening that version 3 introduced for older recipes.
    pub fn update_process(&mut self, m: Option<&Metadata>) {
        if self.engine < 3 {
            self.profile = m.and_then(crate::camera_profiles::builtin);
            if self.sharpening == 0. {
                self.sharpening =
                    crate::model::operators::SharpeningSliders::defaults(self.sharpening_model)
                        .amount;
            }
        }
        self.engine = 4;
    }
    /// Color noise reduction at its defaults for `model`, which the recipe then uses:
    /// Lightroom's Amount 25 for raw files with the measured operator (Detail and
    /// Smoothness 50), off with the original.
    pub fn set_color_noise_defaults(&mut self, model: crate::model::operators::NoiseModel) {
        let d = crate::model::effects::Effects::default();
        self.noise_model = model;
        self.noise_chroma = match model {
            crate::model::operators::NoiseModel::Original => 0.,
            crate::model::operators::NoiseModel::Measured => 0.25,
        };
        self.effects.chroma_detail = d.chroma_detail;
        self.effects.chroma_smoothness = d.chroma_smoothness;
    }
    /// The Color amount the original operator applies when sampling the camera image:
    /// none for the measured one, which runs on the camera image first.
    pub fn sampled_noise_chroma(&self) -> f32 {
        if self.measures_color_noise() {
            0.
        } else {
            self.noise_chroma
        }
    }
    /// Whether the measured Color noise reduction renders: engines before 3 develop
    /// without it and keep the original filter.
    pub fn measures_color_noise(&self) -> bool {
        !self.noise_model.is_original() && self.engine >= 3
    }
    /// The Sharpening sliders at their defaults for `model`, which the recipe then
    /// uses: Lightroom's for raw files (Amount 40, Radius 1.0, Detail 25, Masking 0)
    /// with the measured operator, RAWmakase's earlier ones with the original.
    pub fn set_sharpening_defaults(&mut self, model: crate::model::operators::SharpeningModel) {
        let d = crate::model::operators::SharpeningSliders::defaults(model);
        self.sharpening_model = model;
        self.sharpening = if self.engine >= 3 { d.amount } else { 0. };
        self.sharpening_radius = d.radius;
        self.sharpening_detail = d.detail;
        self.sharpening_masking = d.masking;
    }
    /// The Effects panel's reset: post-crop vignette, Grain and Film Grain at their
    /// defaults, the grain seeds kept.
    pub fn reset_post_crop(&mut self) {
        self.effects.reset_post_crop();
        self.film_grain.reset();
    }
    /// After an edit of Grain from Amount `previous`: grain added from none has nothing
    /// of the original operator's to keep, so it takes the measured one.
    pub fn adopt_measured_grain(&mut self, previous: f32) {
        if previous == 0. && self.effects.grain != 0. {
            self.grain_model = crate::model::operators::GrainModel::Measured;
        }
    }
    /// After an edit of Clarity from `previous`: Clarity added from none has nothing
    /// of the original operator's to keep, so it takes the measured one.
    pub fn adopt_measured_clarity(&mut self, previous: f32) {
        if previous == 0. && self.effects.clarity != 0. {
            self.clarity_model = crate::model::operators::ClarityModel::Measured;
        }
    }
    /// After an edit of Texture from `previous`: Texture added from none has nothing
    /// of the original operator's to keep, so it takes the measured one.
    pub fn adopt_measured_texture(&mut self, previous: f32) {
        if previous == 0. && self.effects.texture != 0. {
            self.texture_model = crate::model::operators::TextureModel::Measured;
        }
    }
    /// The lens correction to apply: the Adobe profile in use, else the built-in
    /// correction if enabled and present in the file.
    pub fn lens_correction<'a>(
        &self,
        m: &'a Metadata,
    ) -> Option<&'a crate::optics::LensCorrection> {
        if self.engine < 4 {
            return None;
        }
        self.lens_profile_in_use(m)
            .used
            .and_then(|c| c.correction(m))
            .or_else(|| m.lens.as_ref().filter(|_| self.lens_builtin))
    }
    /// Recipe as rendered: switched-off panels bypassed, profile-internal adjustments
    /// and the engine-4 default profile.
    pub fn resolved(&self, m: &Metadata) -> std::borrow::Cow<'_, Self> {
        let mut r = match self.as_rendered() {
            std::borrow::Cow::Borrowed(r) => r.with_profile_adjustments(),
            std::borrow::Cow::Owned(r) => {
                std::borrow::Cow::Owned(r.with_profile_adjustments().into_owned())
            }
        };
        // A Lens Corrections panel switched off also turns off built-in data that is
        // off by default (Sony's), which Enable Profile Corrections turned on; what a
        // camera always applies (Fuji, DNG) stays, as the panel bypass leaves it.
        if self
            .panels
            .state(crate::model::panels::Panel::LensCorrections)
            == crate::model::panels::PanelState::Off
            && r.lens_builtin
            && m.lens.as_ref().is_some_and(|l| !l.default_on)
        {
            r.to_mut().lens_builtin = false;
        }
        if r.profile.is_some() || r.engine < 4 {
            return r;
        }
        let Some(profile) = crate::camera_profiles::CameraProfile::camera_matrix_default(m) else {
            return r;
        };
        let mut r = r.into_owned();
        r.profile = Some(std::sync::Arc::new(profile));
        r.profile_tone = true;
        std::borrow::Cow::Owned(r)
    }
    /// Settings that follow a newly chosen profile: an XMP look renders through its
    /// own tone and curves, and the camera baseline and white balance controls are
    /// those of the new profile.
    pub fn profile_changed(&mut self, m: &Metadata) {
        if self.profile.as_ref().is_some_and(|p| p.enhanced.is_some()) {
            self.profile_tone = true;
            self.reference_curves = true;
            self.wide_gamut_curves = true;
        }
        self.use_camera_baseline(m);
        self.sync_white_balance_controls(m);
    }
    pub fn use_camera_baseline(&mut self, m: &Metadata) {
        self.camera_exposure = if self.profile.is_some() || self.engine >= 4 {
            crate::camera_profiles::reference::baseline_exposure(m)
        } else {
            0.
        };
    }
    pub fn reset_white_balance(&mut self, m: &Metadata) {
        let values = self
            .color_profile(m)
            .and_then(|p| {
                if self.white_balance_model.is_original() {
                    p.legacy_as_shot_white_balance(m)
                } else {
                    p.as_shot_white_balance(m)
                }
            })
            .unwrap_or([estimate_temperature(m), 0.]);
        self.temperature = values[0].clamp(TEMPERATURE_MIN, TEMPERATURE_MAX);
        self.tint = values[1].clamp(-TINT_LIMIT, TINT_LIMIT);
        self.wb = [1.; 3];
        self.auto_white_balance = None;
    }
    pub fn sync_white_balance_controls(&mut self, m: &Metadata) {
        let mut adjusted = m.clone();
        adjusted.wb = std::array::from_fn(|c| m.wb[c] * self.wb[c]);
        // A profile that cannot map gains back (no ColorMatrix1) is also left to the
        // fallback model, which is what update_wb then uses.
        let Some([temperature, tint]) = self.color_profile(m).and_then(|p| {
            if self.white_balance_model.is_original() {
                p.legacy_as_shot_white_balance(&adjusted)
            } else {
                p.as_shot_white_balance(&adjusted)
            }
        }) else {
            self.sync_fallback_white_balance_controls(m);
            return;
        };
        self.temperature = temperature.clamp(TEMPERATURE_MIN, TEMPERATURE_MAX);
        self.tint = tint.clamp(-TINT_LIMIT, TINT_LIMIT);
        // Gains the controls cannot show become the nearest they can, so the next
        // Temperature or Tint edit does not jump.
        if self.temperature != temperature || self.tint != tint {
            self.update_wb(m);
        }
    }
    /// Temperature and Tint that [`Self::update_wb`] maps to the current gains, solved
    /// under its fallback model (recipes without a camera profile). Temperature sets the red/blue
    /// ratio and Tint scales both against green, so each is solved in turn.
    pub fn sync_fallback_white_balance_controls(&mut self, m: &Metadata) {
        let model = |temperature: f32| {
            let mut r = self.clone();
            r.temperature = temperature;
            r.tint = 0.;
            r.update_wb(m);
            r.wb
        };
        let red_blue = |wb: [f32; 3]| (wb[0].max(1e-6) / wb[2].max(1e-6)).ln();
        let target = red_blue(self.wb);
        // The model's locus covers 2000–15000 K; the ratio is monotonic along it.
        let (mut lo, mut hi) = (2000f32, 15000f32);
        let rising = red_blue(model(hi)) > red_blue(model(lo));
        for _ in 0..40 {
            let mid = (lo * hi).sqrt();
            if (red_blue(model(mid)) < target) == rising {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let temperature = (lo * hi).sqrt();
        let neutral = model(temperature);
        let tint = 50.
            * ((self.wb[0] * self.wb[2]).max(1e-12) / (neutral[0] * neutral[2]).max(1e-12)).log2();
        self.temperature = temperature.clamp(TEMPERATURE_MIN, TEMPERATURE_MAX);
        self.tint = tint.clamp(-TINT_LIMIT, TINT_LIMIT);
        // Gains the controls cannot reproduce become the ones they show, so the next
        // Temperature or Tint edit does not jump. That covers gains past either end of
        // the locus or the Tint limit, and a profile whose white balance Tint does not
        // scale the way the fallback model's does.
        let mut shown = self.clone();
        shown.update_wb(m);
        if (0..3).any(|c| (shown.wb[c] / self.wb[c].max(1e-6) - 1.).abs() > 1e-3) {
            self.wb = shown.wb;
        }
    }
    pub fn update_wb(&mut self, m: &Metadata) {
        if let Some(wb) = self.color_profile(m).and_then(|p| {
            if self.white_balance_model.is_original() {
                p.legacy_white_balance(self.temperature, self.tint, m)
            } else {
                p.white_balance(self.temperature, self.tint, m)
            }
        }) {
            self.wb = wb;
            return;
        }
        let current = illuminant_camera(self.temperature, m);
        let baseline = illuminant_camera(estimate_temperature(m), m);
        for c in 0..3 {
            self.wb[c] = ((baseline[c] / baseline[1]) / (current[c] / current[1]).max(0.001))
                .clamp(0.01, 100.);
        }
        self.wb[1] *= 2f32.powf(-self.tint / 100.);
        let g = self.wb[1];
        for v in &mut self.wb {
            *v /= g;
        }
    }
}
fn is_zero(v: &f32) -> bool {
    *v == 0.
}
fn is_one(v: &f32) -> bool {
    *v == 1.
}
/// The Basic panel's Treatment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Treatment {
    Color,
    BlackWhite,
}

/// Whether a profile renders black & white (an XMP look with `ConvertToGrayscale`).
pub fn is_monochrome(profile: Option<&crate::camera_profiles::CameraProfile>) -> bool {
    profile
        .and_then(|p| p.enhanced.as_ref())
        .is_some_and(|look| look.monochrome)
}

/// Whether Enable Profile Corrections is ticked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileCorrections {
    On,
    Off,
}
fn one() -> f32 {
    1.
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names a new recipe is saved with; renaming one would lose that setting
    /// from every saved edit. Settings at their defaults beyond these are left out.
    #[test]
    fn a_new_recipe_keeps_its_stored_names() {
        let stored = serde_json::to_value(Recipe::default()).unwrap();
        let keys: Vec<&str> = stored
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "black_point",
                "blacks",
                "camera_exposure",
                "contrast",
                "crop",
                "curve",
                "effects",
                "engine",
                "exposure",
                "flip_x",
                "flip_y",
                "grading",
                "highlights",
                "hsl",
                "lens_builtin",
                "lens_ca",
                "lens_distortion",
                "lens_profile",
                "lens_vignetting",
                "midtone",
                "noise_chroma",
                "noise_luma",
                "preset_name",
                "preset_settings",
                "profile",
                "profile_tone",
                "reference_calibration",
                "reference_color",
                "reference_curves",
                "rotation",
                "saturation",
                "shadows",
                "sharpening",
                "sharpening_detail",
                "sharpening_masking",
                "sharpening_radius",
                "straighten",
                "temperature",
                "tint",
                "transform",
                "vibrance",
                "wb",
                "white_point",
                "whites",
                "wide_gamut_curves",
            ]
        );
        assert_eq!(
            serde_json::from_value::<Recipe>(stored).unwrap(),
            Recipe::default()
        );
        assert_eq!(serde_json::to_string(&LocalEdits::default()).unwrap(), "{}");
    }

    /// Settings a recipe was saved without take their defaults, and ones this build
    /// does not know are kept to be saved again.
    #[test]
    fn a_partly_stored_recipe_takes_defaults_and_keeps_unknown_settings() {
        let r: Recipe =
            serde_json::from_str(r#"{"exposure": 0.5, "from_a_later_release": [1, 2]}"#).unwrap();
        assert_eq!(r.exposure, 0.5);
        assert_eq!(r.contrast, Recipe::default().contrast);
        let saved = serde_json::to_value(&r).unwrap();
        assert_eq!(saved["from_a_later_release"], serde_json::json!([1, 2]));
        assert!(r.validate().is_ok());
    }

    /// Settings saved only when they differ from their defaults keep their names
    /// too: a renamed one would be read as unknown and lost from saved edits.
    #[test]
    fn settings_saved_only_when_set_keep_their_stored_names() {
        let stored = serde_json::json!({
            "lens_profile_choice": {"setup": "Custom"},
            "lens_vignetting": 0.5,
            "lens_manual_distortion": 0.25,
            "grain_model": "Measured",
            "clarity_model": "Measured",
            "texture_model": "Measured",
            "film_grain": {"amount": 0.0, "size": 0.5, "roughness": 0.25, "color": 0.5, "seed": 42},
            "profile_amount": 0.5,
            "sharpening_model": "Measured",
            "parametric_model": "Layered",
            "contrast_model": "Adaptive",
            "lens_vignette_model": "Measured",
            "retouch_model": "Measured",
            "grading_model": "Measured",
            "mixer_model": "Chart",
            "saturation_model": "Gray",
            "vibrance_model": "Chart",
            "black_white_model": "Chart",
            "calibration_model": "Measured",
            "whites_model": "Adaptive",
            "white_balance_model": "Calibrated",
            "gamut_model": "Clip",
            "noise_model": "Measured",
            "auto_white_balance": [5000.0, 10.0],
            "curve_saturation": 0.5,
            "point_colors": [{
                "source": [1.0, 0.5, 0.5], "shift": [0.0, 0.0, 0.0], "range": 0.5,
                "hue_range": [0.0, 0.25, 0.75, 1.0], "saturation_range": [0.0, 0.25, 0.75, 1.0],
                "luminance_range": [0.0, 0.25, 0.75, 1.0], "variance": 0.0,
            }],
            "constrain_crop": true,
            "upright": {"mode": "auto"},
            "panels": {"off": ["detail"]},
        });
        let r: Recipe = serde_json::from_value(stored.clone()).unwrap();
        assert!(
            r.unknown.is_empty(),
            "read as unknown: {:?}",
            r.unknown.keys()
        );
        let saved = serde_json::to_value(&r).unwrap();
        for (key, value) in stored.as_object().unwrap() {
            assert_eq!(&saved[key], value, "{key}");
        }
        let local: LocalEdits = serde_json::from_value(serde_json::json!({
            "retouch": [{"mode": "Heal", "shape": {"Spot": {"center": [0.5, 0.5], "radius": 0.01}},
                "feather": 0.5, "opacity": 1.0, "offset": [0.1, 0.0]}],
            "red_eye": [{"center": [0.5, 0.5], "radius": [0.01, 0.01], "pupil_size": 0.5, "darken": 0.5}],
            "masks": [{"name": "Sky"}],
        }))
        .unwrap();
        assert_eq!(
            (local.retouch.len(), local.red_eye.len(), local.masks.len()),
            (1, 1, 1)
        );
    }
}
