use super::panels::PanelSwitches;
use crate::model::white_balance::{estimate_temperature, illuminant_camera, rendered_controls};
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
/// are kept in `unknown` and saved again, so an older build never drops them; the
/// selectors of earlier releases' engines and operators are dropped as they are read.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Recipe {
    /// Apply the lens correction the camera stored in the RAW. Missing in older
    /// recipes, which therefore render without it.
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
    pub effects: crate::model::effects::Effects,
    pub preset_name: String,
    pub preset_settings: std::collections::BTreeMap<String, String>,
    /// A saved Embedded profile loads with today's tone curve (`CameraProfile::loaded`),
    /// wherever a recipe is read: sessions, the catalog, History, presets and sidecars.
    #[serde(deserialize_with = "loaded_profile")]
    pub profile: Option<std::sync::Arc<crate::camera_profiles::CameraProfile>>,
    /// Lightroom's Profile Amount (`crs:Look` `Amount`, 0–2, 1 = 100%): the strength of
    /// a look that supports it. Other profiles render at 100% whatever it is. Omitted
    /// at 1, so releases that predate it read the recipe.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub profile_amount: f32,
    pub sharpening_radius: f32,
    pub sharpening_detail: f32,
    pub sharpening_masking: f32,
    pub exposure: f32,
    #[serde(default)]
    pub camera_exposure: f32,
    pub temperature: f32,
    pub tint: f32,
    /// A rendered file's Temp and Tint, Lightroom's relative −100..+100 sliders; its
    /// kelvin `temperature` and `tint` stay 6500 and 0 so older builds still read the
    /// recipe. Omitted at 0, so RAW recipes save as before.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub incremental_temperature: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub incremental_tint: f32,
    pub wb: [f32; 3],
    /// Temperature and Tint that Auto white balance chose, so the WB menu shows Auto
    /// while the photo still has them: Kelvin for a RAW, the relative
    /// `incremental_temperature` and `incremental_tint` for a rendered file.
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
    /// AI Denoise (Detail). Omitted when off at the default Amount, so releases that
    /// predate it read the recipe; any other value needs the saved format that knows it
    /// (`saved_format::schema_for`).
    #[serde(
        default,
        skip_serializing_if = "crate::model::ai_denoise::AiDenoise::is_default"
    )]
    pub ai_denoise: crate::model::ai_denoise::AiDenoise,
    /// High Dynamic Range editing. Omitted when off with every value at its default,
    /// so releases that predate it read the recipe; an active HDR edit needs the saved
    /// format that knows it (`saved_format::schema_for`).
    #[serde(
        default,
        skip_serializing_if = "crate::model::hdr::HdrSettings::is_default"
    )]
    pub hdr: crate::model::hdr::HdrSettings,
    pub sharpening: f32,
    pub crop: [f32; 4],
    pub straighten: f32,
    /// Whether `straighten` is the Crop panel's Auto, which Copy, Paste and Sync of the
    /// Crop measure on each photo again; see [`Recipe::auto_straighten`]. Omitted
    /// when none, so releases that predate it read the recipe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub straighten_auto: Option<crate::model::transform::AutoStraighten>,
    /// Lightroom's Constrain Crop (`crs:CropConstrainToWarp`): the crop as rendered
    /// shrinks, keeping its aspect, to leave out the white areas Upright, the Transform
    /// sliders and manual Distortion uncover. `crop` stays as the user set it. Omitted
    /// when off, so releases that predate it read the recipe.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub constrain_crop: bool,
    /// Transform panel sliders.
    #[serde(default)]
    pub transform: crate::model::transform::Transform,
    /// Lightroom's Upright, applied before the Transform sliders.
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
    /// Settings from a newer release, preserved as they were. Those of earlier
    /// releases that chose an engine or operator are dropped as they are read
    /// (`saved_format::OBSOLETE_SETTINGS`).
    #[serde(
        flatten,
        deserialize_with = "crate::model::saved_format::unknown_settings"
    )]
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
            lens_builtin: true,
            lens_profile: false,
            lens_profile_choice: Default::default(),
            lens_distortion: 1.,
            lens_vignetting: 1.,
            lens_manual_distortion: 0.,
            lens_ca: false,
            effects: Default::default(),
            preset_name: String::new(),
            preset_settings: Default::default(),
            profile: None,
            sharpening_radius: 0.8,
            sharpening_detail: 0.25,
            sharpening_masking: 0.35,
            exposure: 0.,
            camera_exposure: 0.,
            temperature: 6500.,
            tint: 0.,
            incremental_temperature: 0.,
            incremental_tint: 0.,
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
            ai_denoise: Default::default(),
            hdr: Default::default(),
            sharpening: 0.35,
            crop: [0., 0., 1., 1.],
            straighten: 0.,
            straighten_auto: None,
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
        if self.effects.monochrome || is_monochrome(self.profile.as_deref()) {
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
    /// A look's Exposure, Saturation and colour mixer added to the user's, and its
    /// vignette in place of the user's, as Camera Raw 18.7 renders them. Its
    /// parametric curve and split toning render as passes of their own.
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
            // As Camera Raw: the aspect ratio chosen in the camera is the crop a
            // photo starts with, which can be widened to the whole frame. The camera
            // records it on the unrotated frame; the crop is of the photo as shown.
            crop: m.camera_crop.map_or([0., 0., 1., 1.], |c| {
                crate::model::image_frame::ImageFrame::for_metadata(m).rect_from_unrotated(c)
            }),
            ..Default::default()
        }
    }
    /// A rendered file's starting edit, as Lightroom's for JPEG and TIFF: the
    /// Embedded profile, no baseline exposure, Sharpening and Color noise
    /// reduction at 0, white balance as shot.
    pub fn for_rendered(m: &Metadata) -> Self {
        Self {
            profile: Some(std::sync::Arc::new(
                crate::camera_profiles::CameraProfile::embedded(m),
            )),
            sharpening: 0.,
            noise_chroma: 0.,
            camera_exposure: 0.,
            temperature: 6500.,
            tint: 0.,
            wb: [1.; 3],
            ..Self::default()
        }
    }
    /// What a rendered photo without a saved edit starts from: [`Self::for_rendered`],
    /// and for an HDR file an HDR edit at a limit its own light renders within
    /// (docs/records/hdr-stage4-design.md). Saved edits, Lightroom's among them, read
    /// on `for_rendered`'s settings, where a missing HDR field means HDR off, as it
    /// did before HDR files opened.
    pub fn for_new_rendered(m: &Metadata) -> Self {
        let mut recipe = Self::for_rendered(m);
        if let Some(source) = m.hdr_source {
            recipe.hdr.enabled = true;
            recipe.hdr.limit = crate::model::hdr::limit_containing(source.peak);
        }
        recipe
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
        if m.is_rendered() {
            return Self::for_rendered(m);
        }
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
        // keeps the profile it embeds, and any other file gets RAWmakase Color, or
        // the Camera Look fitted as the default of a photo without colour data (only
        // that photo's list has one).
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
            .or_else(|| {
                recipe
                    .profile
                    .is_none()
                    .then(|| find(crate::camera_profiles::fitted::LOOK))
                    .flatten()
            })
        {
            recipe.profile = Some(profile.clone());
        }
        recipe.set_sharpening_defaults();
        recipe.set_color_noise_defaults();
        recipe.use_camera_baseline(m);
        recipe.reset_white_balance(m);
        recipe
    }
    pub fn validate(&self) -> Result<()> {
        self.effects.validate()?;
        ensure!(
            self.lens_profile_choice
                .lensfun()
                .is_none_or(|l| l.validate()),
            "Invalid Lensfun lens profile"
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
            self.incremental_temperature.abs() <= 100. && self.incremental_tint.abs() <= 100.,
            "Relative white balance out of bounds"
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
        ensure!(self.ai_denoise.is_valid(), "Invalid AI Denoise Amount");
        self.hdr.validate()?;
        self.hdr.check_renderable()?;
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
    /// The camera profile used for rendering. Photos without an imported or bundled
    /// profile render through the DNG ColorMatrix default.
    pub fn color_profile(
        &self,
        m: &Metadata,
    ) -> Option<std::borrow::Cow<'_, crate::camera_profiles::CameraProfile>> {
        match &self.profile {
            Some(p) => Some(std::borrow::Cow::Borrowed(p.as_ref())),
            None => crate::camera_profiles::CameraProfile::camera_matrix_default(m)
                .map(std::borrow::Cow::Owned),
        }
    }
    /// Lightroom's Enable Profile Corrections: an imported Adobe lens profile, else the
    /// correction the camera stored in the RAW. Built-in data that is off by default
    /// (Nikon's for lenses with an optional correction) follows the switch; Fuji's, a
    /// DNG's and Sony's lateral CA stay on, as in Lightroom, while Sony's remaining
    /// tables stand in for the profile (`builtin_correction`).
    pub fn set_profile_corrections(&mut self, m: &Metadata, state: ProfileCorrections) {
        self.lens_profile = state == ProfileCorrections::On;
        if m.lens.as_ref().is_some_and(|l| !l.default_on) {
            self.lens_builtin = self.lens_profile;
        }
    }
    /// The imported Adobe profile Enable Profile Corrections uses here, and the one
    /// the edit names when it isn't imported.
    pub fn lens_profile_in_use<'c, 'p>(
        &'c self,
        m: &'p Metadata,
    ) -> crate::lens::choice::Resolved<'c, 'p> {
        if !self.lens_profile {
            return Default::default();
        }
        self.lens_profile_choice.resolve(&m.lens_profiles, m)
    }
    /// Whether the edit renders in HDR or uses an HDR camera profile: it is saved in
    /// a format releases before HDR refuse (`saved_format::schema_for`, the catalog's
    /// version 6). Values kept while HDR is off do not count.
    pub fn uses_hdr(&self) -> bool {
        self.hdr.is_active() || self.profile.as_ref().is_some_and(|p| p.is_hdr())
    }
    /// Whether the edit names a Lensfun lens, in use or not, resolved or not: it is
    /// saved in a format releases before Lensfun refuse (`saved_format::schema_for`,
    /// the catalog's version 4).
    pub fn uses_lensfun(&self) -> bool {
        self.lens_profile_choice.lensfun().is_some()
    }
    /// The Lensfun lens Enable Profile Corrections uses, when the edit chose one.
    pub fn lensfun_in_use(&self) -> Option<&crate::lens::lensfun::LensfunChoice> {
        self.lens_profile
            .then(|| self.lens_profile_choice.lensfun())
            .flatten()
    }
    /// Why Enable Profile Corrections cannot render with the profile it should here,
    /// and what renders instead, when it is on.
    pub fn missing_lens_profile(&self, m: &Metadata) -> Option<String> {
        // A Lens Corrections panel switched off renders no lens correction at all.
        let panel = self
            .panels
            .state(crate::model::panels::Panel::LensCorrections);
        if panel == crate::model::panels::PanelState::Off || !self.lens_profile {
            return None;
        }
        // A Lensfun lens renders from the calibration the edit copied, as far as it
        // covers this photo; never from an Adobe profile.
        if let Some(lensfun) = self.lensfun_in_use() {
            let applied = lensfun.apply(m);
            if applied.notes.is_empty() {
                return None;
            }
            let why = applied.notes.join("; it ");
            let profile = format!("Lensfun profile \"{}\" {why}", lensfun.name());
            return Some(match (&applied.correction, self.builtin_correction(m)) {
                (Some(_), _) => profile,
                (None, Some(builtin)) => format!("{profile}; using {}", builtin.source),
                (None, None) => format!("{profile}; no lens correction"),
            });
        }
        let resolved = self.lens_profile_in_use(m);
        // The profile the RAW carries renders as its built-in correction.
        if let Some(id) = self.lens_profile_choice.adobe().filter(|id| id.embedded) {
            return self.builtin_correction(m).is_none().then(|| {
                format!(
                    "Lens profile \"{}\" comes with the camera, but this file has none; no lens correction",
                    id.label()
                )
            });
        }
        let profile = match (resolved.missing, m.lens_model.as_str()) {
            (Some(id), _) => format!("Lens profile \"{}\" isn't imported", id.label()),
            (None, _) if resolved.used.is_some() => return None,
            (None, "") => "No matching imported Adobe lens profile for this lens".to_string(),
            (None, lens) => format!("No matching imported Adobe lens profile for {lens}"),
        };
        let instead = match (resolved.used, self.builtin_correction(m)) {
            (Some(used), _) => format!("using {}", used.profile.name),
            (None, Some(builtin)) => format!("using {}", builtin.source),
            (None, None) => "no lens correction".to_string(),
        };
        Some(format!("{profile}; {instead}"))
    }
    /// Adds a Heal or Clone operation.
    pub fn add_retouch(&mut self, op: crate::model::retouch::RetouchOp) {
        self.retouch.push(op);
    }
    /// Color noise reduction at its defaults: Lightroom's Amount 25 for raw files
    /// (Detail and Smoothness 50).
    /// AI Denoise's switch, as Lightroom's Denoise switch edits: switched on, manual
    /// Luminance and Color noise reduction go to 0 (Camera Raw 18.7 writes them so; the
    /// AI result has no noise left for them); switched off, Color's defaults come back
    /// while it is still at the 0 switching on left (Camera Raw restores them). Amount is
    /// kept either way.
    pub fn switch_ai_denoise(&mut self, on: bool) {
        if self.ai_denoise.enabled == on {
            return;
        }
        self.ai_denoise.enabled = on;
        if on {
            self.noise_luma = 0.;
            self.noise_chroma = 0.;
        } else if self.noise_chroma == 0. {
            self.set_color_noise_defaults();
        }
    }
    pub fn set_color_noise_defaults(&mut self) {
        let d = crate::model::effects::Effects::default();
        self.noise_chroma = 0.25;
        self.effects.chroma_detail = d.chroma_detail;
        self.effects.chroma_smoothness = d.chroma_smoothness;
    }
    /// The Sharpening sliders at their defaults: Lightroom's for raw files (Amount 40,
    /// Radius 1.0, Detail 25, Masking 0).
    pub fn set_sharpening_defaults(&mut self) {
        let d = crate::model::operators::SharpeningSliders::defaults();
        self.sharpening = d.amount;
        self.sharpening_radius = d.radius;
        self.sharpening_detail = d.detail;
        self.sharpening_masking = d.masking;
    }
    /// The built-in correction this edit renders when no Adobe profile is in use: with
    /// Enable Profile Corrections on, the camera's fuller data where it has some (Sony's
    /// tables, standing in for the Adobe profile Lightroom would use), else what the
    /// camera applies; none with built-in corrections off.
    pub fn builtin_correction<'a>(
        &self,
        m: &'a Metadata,
    ) -> Option<&'a crate::optics::LensCorrection> {
        if !self.lens_builtin {
            return None;
        }
        (m.lens_stand_in.as_ref())
            .filter(|_| self.lens_profile)
            .or(m.lens.as_ref())
    }
    /// Lightroom's profile Distortion and Vignetting amounts (1 = 100%) as they apply
    /// to the lens correction in use. They scale a profile: an imported Adobe one, or
    /// the camera data standing in for it (Sony's tables, Nikon's optional "Camera
    /// Settings" profile). A correction the camera always applies (a DNG's opcodes,
    /// Fujifilm's, Olympus's, Panasonic's, Sony's CA and Nikon's required data)
    /// renders in full whatever the amounts and the switch, as Camera Raw 18.7 renders
    /// such files' opcodes, also where the stand-in repeats it (a Sony compact's
    /// distortion).
    pub fn lens_amounts(&self, m: &Metadata) -> [f32; 2] {
        let amounts = [self.lens_distortion, self.lens_vignetting];
        if self.lens_profile_in_use(m).used.is_some()
            || self
                .lensfun_in_use()
                .is_some_and(|l| l.apply(m).correction.is_some())
        {
            return amounts;
        }
        if !self.lens_profile {
            return [1., 1.];
        }
        match (&m.lens_stand_in, &m.lens) {
            (Some(_), Some(always)) => [
                if always.distortion.is_some() {
                    1.
                } else {
                    amounts[0]
                },
                if always.vignetting.is_some() {
                    1.
                } else {
                    amounts[1]
                },
            ],
            (Some(_), None) => amounts,
            (None, Some(lens)) if !lens.default_on => amounts,
            _ => [1., 1.],
        }
    }
    /// The lens correction to apply: the Lensfun lens the edit chose or the Adobe
    /// profile in use, else the built-in correction if enabled and present in the
    /// file. A Lensfun lens renders from the edit's own copy of its calibration; when
    /// it can't correct this photo (not resolved, or not covering it), the built-in
    /// correction renders, never an Adobe profile.
    pub fn lens_correction<'a>(
        &self,
        m: &'a Metadata,
    ) -> Option<std::borrow::Cow<'a, crate::optics::LensCorrection>> {
        use std::borrow::Cow;
        if let Some(lensfun) = self.lensfun_in_use() {
            return lensfun
                .apply(m)
                .correction
                .map(Cow::Owned)
                .or_else(|| self.builtin_correction(m).map(Cow::Borrowed));
        }
        self.lens_profile_in_use(m)
            .used
            .and_then(|c| c.correction(m))
            .or_else(|| self.builtin_correction(m))
            .map(Cow::Borrowed)
    }
    /// Recipe as rendered in SDR: switched-off panels bypassed, profile-internal
    /// adjustments and the default profile. A monochrome photo renders without white
    /// balance gains, whatever an edit from another photo or a preset gave it.
    ///
    /// An HDR edit renders here as its SDR rendition: the edit with HDR off
    /// (docs/records/hdr-stage1-results.md#sdr-rendition). Only
    /// [`Self::resolved_hdr`] renders its HDR master, so no SDR caller can receive
    /// HDR pixels.
    pub fn resolved(&self, m: &Metadata) -> std::borrow::Cow<'_, Self> {
        let mut r = self.resolved_hdr(m);
        if r.hdr.is_active() {
            r.to_mut().hdr.enabled = false;
        }
        r
    }
    /// [`Self::resolved`] keeping HDR on: the recipe of an edit's HDR master.
    pub fn resolved_hdr(&self, m: &Metadata) -> std::borrow::Cow<'_, Self> {
        let mut r = match self.as_rendered() {
            std::borrow::Cow::Borrowed(r) => r.with_profile_adjustments(),
            std::borrow::Cow::Owned(r) => {
                std::borrow::Cow::Owned(r.with_profile_adjustments().into_owned())
            }
        };
        if m.monochrome && r.wb != [1.; 3] {
            r.to_mut().wb = [1.; 3];
        }
        // A Lens Corrections panel switched off also turns off built-in data that is
        // off by default (Nikon's optional), which Enable Profile Corrections turned on; what a
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
        if r.profile.is_some() {
            return r;
        }
        let Some(profile) = crate::camera_profiles::CameraProfile::camera_matrix_default(m) else {
            return r;
        };
        let mut r = r.into_owned();
        r.profile = Some(std::sync::Arc::new(profile));
        std::borrow::Cow::Owned(r)
    }
    /// Settings that follow a newly chosen profile: the camera baseline and white
    /// balance controls are those of the new profile.
    pub fn profile_changed(&mut self, m: &Metadata) {
        self.use_camera_baseline(m);
        self.sync_white_balance_controls(m);
    }
    /// The Crop panel's Auto straighten, while Straighten still has the angle it is
    /// about.
    pub fn auto_straighten(&self) -> Option<crate::model::transform::AutoStraighten> {
        self.straighten_auto
            .filter(|auto| auto.angle() == self.straighten)
    }
    /// Whether Straighten waits to be measured on this photo (see
    /// [`crate::model::transform::AutoStraighten::Pending`]).
    pub fn straighten_pending(&self) -> bool {
        matches!(
            self.auto_straighten(),
            Some(crate::model::transform::AutoStraighten::Pending(_))
        )
    }
    /// Sets the angle Auto straighten measured on this photo. Without one (no horizon
    /// or verticals to go by) the angle stays as it is, no longer Auto.
    pub fn set_auto_straighten(&mut self, measured: Option<f32>) {
        match measured {
            Some(angle) => {
                self.straighten = angle;
                self.straighten_auto =
                    Some(crate::model::transform::AutoStraighten::Measured(angle));
            }
            None => self.straighten_auto = None,
        }
    }
    pub fn use_camera_baseline(&mut self, m: &Metadata) {
        self.camera_exposure = crate::camera_profiles::reference::baseline_exposure(m);
    }
    pub fn reset_white_balance(&mut self, m: &Metadata) {
        if m.is_rendered() {
            self.incremental_temperature = 0.;
            self.incremental_tint = 0.;
            self.wb = [1.; 3];
            self.auto_white_balance = None;
            return;
        }
        let values = self
            .color_profile(m)
            .and_then(|p| p.as_shot_white_balance(m))
            .unwrap_or([estimate_temperature(m), 0.]);
        self.temperature = values[0].clamp(TEMPERATURE_MIN, TEMPERATURE_MAX);
        self.tint = values[1].clamp(-TINT_LIMIT, TINT_LIMIT);
        self.wb = [1.; 3];
        self.auto_white_balance = None;
    }
    pub fn sync_white_balance_controls(&mut self, m: &Metadata) {
        if m.is_rendered() || m.monochrome {
            // Temp and Tint are a rendered file's white balance (and keep Auto); its
            // gains stay 1, as a monochrome photo's, which has none.
            self.wb = [1.; 3];
            return;
        }
        let mut adjusted = m.clone();
        adjusted.wb = std::array::from_fn(|c| m.wb[c] * self.wb[c]);
        // A profile that cannot map gains back (no ColorMatrix1) is also left to the
        // fallback model, which is what update_wb then uses.
        let Some([temperature, tint]) = self
            .color_profile(m)
            .and_then(|p| p.as_shot_white_balance(&adjusted))
        else {
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
    /// Between a RAW and a rendered file (JPEG, PNG, TIFF), which keep white balance in
    /// different terms (Kelvin against relative) and have profiles of their own: this
    /// recipe takes `own`'s white balance and profile, with the profile's baseline
    /// exposure, as Copy, Sync and presets apply settings from the other kind of file.
    pub fn keep_white_balance_and_profile(&mut self, own: &Recipe) {
        self.temperature = own.temperature;
        self.tint = own.tint;
        self.incremental_temperature = own.incremental_temperature;
        self.incremental_tint = own.incremental_tint;
        self.wb = own.wb;
        self.auto_white_balance = own.auto_white_balance;
        self.profile = own.profile.clone();
        self.profile_amount = own.profile_amount;
        self.camera_exposure = own.camera_exposure;
    }
    /// White balance from `gains` that make a picked colour neutral (the eyedropper's,
    /// Auto's): a RAW's gains, or for a rendered file the nearest Temp and Tint, which
    /// carry its white balance (`white_balance::rendered_white_balance`).
    pub fn pick_white_balance(&mut self, gains: [f32; 3], m: &Metadata) {
        if m.is_rendered() {
            [self.incremental_temperature, self.incremental_tint] = rendered_controls(gains);
            self.wb = [1.; 3];
            return;
        }
        if m.monochrome {
            // Grey has no white balance to pick.
            self.wb = [1.; 3];
            return;
        }
        self.wb = gains;
        self.sync_white_balance_controls(m);
    }
    pub fn update_wb(&mut self, m: &Metadata) {
        if m.is_rendered() || m.monochrome {
            // Temp and Tint render as `white_balance::rendered_white_balance`; a
            // monochrome photo has no white balance (Lightroom disables it).
            self.wb = [1.; 3];
            return;
        }
        if let Some(wb) = self
            .color_profile(m)
            .and_then(|p| p.white_balance(self.temperature, self.tint, m))
        {
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
/// [`Recipe::profile`] as a saved edit stores it, with a stored Embedded profile given
/// today's tone curve.
fn loaded_profile<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<std::sync::Arc<crate::camera_profiles::CameraProfile>>, D::Error> {
    let profile = Option::<crate::camera_profiles::CameraProfile>::deserialize(deserializer)?;
    Ok(profile.map(|p| std::sync::Arc::new(p.loaded())))
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

    /// A recipe as JSON whose profile is the Embedded one, edited by `change`.
    fn saved_embedded(change: impl FnOnce(&mut serde_json::Value)) -> String {
        let m = Metadata {
            source: crate::camera_data::Source::Rendered,
            ..Default::default()
        };
        let r = Recipe {
            profile: Some(std::sync::Arc::new(
                crate::camera_profiles::CameraProfile::embedded(&m),
            )),
            ..Recipe::default()
        };
        let mut v = serde_json::to_value(&r).unwrap();
        v["profile"]["tone"] = serde_json::json!([[0., 0.], [1., 1.]]);
        change(&mut v["profile"]);
        v.to_string()
    }
    fn tone_of(json: &str) -> serde_json::Value {
        let r: Recipe = serde_json::from_str(json).unwrap();
        serde_json::to_value(r).unwrap()["profile"]["tone"].clone()
    }

    /// A rendered photo's edit saved while the Embedded profile had no tone curve
    /// loads with Camera Raw's default tone curve, wherever the edit is read from:
    /// the Embedded profile's curve comes from the code, not from storage.
    #[test]
    fn a_saved_embedded_profile_loads_with_todays_tone_curve() {
        assert_eq!(
            tone_of(&saved_embedded(|_| {})),
            serde_json::to_value(crate::camera_profiles::default_tone()).unwrap()
        );
    }

    /// Only the Embedded profile itself: one named so with a colour matrix or a look
    /// table, or any other profile, keeps the tone curve it was saved with.
    #[test]
    fn other_profiles_keep_their_saved_tone_curve() {
        let identity = serde_json::json!([[0., 0.], [1., 1.]]);
        let with_color = saved_embedded(|p| {
            p["color1"] = serde_json::json!([[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]])
        });
        assert_eq!(tone_of(&with_color), identity);
        let with_look = saved_embedded(|p| {
            p["look"] =
                serde_json::json!({"dims": [1, 1, 1], "data": [[0., 0., 0.]], "srgb": false})
        });
        assert_eq!(tone_of(&with_look), identity);
        let other = saved_embedded(|p| p["name"] = "Adobe Standard".into());
        assert_eq!(tone_of(&other), identity);
    }

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
            "profile_amount": 0.5,
            "incremental_temperature": 12.0,
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

    #[test]
    fn a_rendered_photo_starts_from_lightrooms_non_raw_defaults() {
        use crate::camera_data::Source;
        let m = Metadata {
            source: Source::Rendered,
            make: "Apple".into(),
            model: "iPhone 15".into(),
            ..Default::default()
        };
        let r = Recipe::with_profile_preference(&m, &[], ProfilePreference::Camera);
        assert!(r.profile.as_ref().unwrap().is_embedded());
        assert_eq!(
            (r.sharpening, r.noise_chroma, r.camera_exposure),
            (0., 0., 0.)
        );
        assert!(r.validate().is_ok());
        assert_eq!(crate::camera_profiles::reference::baseline_exposure(&m), 0.);
    }
    /// Sony: Lightroom always applies the camera's lateral CA, while its other tables
    /// stand in for an Adobe profile only with Enable Profile Corrections on.
    #[test]
    fn sony_ca_always_applies_and_its_tables_stand_in_for_a_profile() {
        use crate::optics::{LensCorrection, Radial};
        let ca = Radial::new(vec![0., 1.], vec![1., 1.001]).unwrap();
        let full = LensCorrection {
            source: LensCorrection::SONY_BUILTIN.into(),
            default_on: true,
            vignetting: Radial::new(vec![0., 1.], vec![1., 1.8]),
            distortion: Radial::new(vec![0., 1.], vec![1., 1.02]),
            chromatic: Some([ca.clone(), ca]),
            gain_map: None,
            ..Default::default()
        };
        let m = Metadata {
            lens: Some(LensCorrection {
                vignetting: None,
                distortion: None,
                ..full.clone()
            }),
            lens_stand_in: Some(full),
            ..Default::default()
        };
        let mut r = Recipe::for_metadata(&m);
        assert!(r.lens_builtin);
        let vignetting = |r: &Recipe| r.lens_correction(&m).map(|c| c.vignetting.is_some());
        assert!(r.lens_correction(&m).unwrap().chromatic.is_some());
        assert_eq!(vignetting(&r), Some(false));
        r.set_profile_corrections(&m, ProfileCorrections::On);
        assert!(r.lens_builtin);
        assert_eq!(vignetting(&r), Some(true));
        assert!(
            r.missing_lens_profile(&m)
                .unwrap()
                .ends_with("using Sony built-in")
        );
        r.set_profile_corrections(&m, ProfileCorrections::Off);
        assert_eq!(vignetting(&r), Some(false));
        // The profile's amounts scale the stand-in only: a Lightroom edit with profile
        // corrections off keeps them for when they are turned on.
        r.lens_distortion = 0.;
        assert_eq!(r.lens_amounts(&m), [1., 1.]);
        r.set_profile_corrections(&m, ProfileCorrections::On);
        assert_eq!(r.lens_amounts(&m), [0., 1.]);
        // A compact's distortion, which Lightroom always applies, ignores its amount
        // with the stand-in in use too, while the stand-in's vignetting takes its own.
        let compact = Metadata {
            lens: Some(LensCorrection {
                distortion: m.lens_stand_in.as_ref().unwrap().distortion.clone(),
                ..m.lens.clone().unwrap()
            }),
            ..m.clone()
        };
        r.lens_vignetting = 0.5;
        assert_eq!(r.lens_amounts(&compact), [1., 0.5]);
        r.set_profile_corrections(&compact, ProfileCorrections::Off);
        assert_eq!(r.lens_amounts(&compact), [1., 1.]);
        r.set_profile_corrections(&compact, ProfileCorrections::On);
        r.lens_vignetting = 1.;
        // Without a stand-in, what the camera always applies ignores them either way.
        let opcodes = Metadata {
            lens_stand_in: None,
            ..m.clone()
        };
        assert_eq!(r.lens_amounts(&opcodes), [1., 1.]);
        r.set_profile_corrections(&m, ProfileCorrections::Off);
        // An edit saved with built-in corrections off keeps them off.
        r.lens_builtin = false;
        r.set_profile_corrections(&m, ProfileCorrections::On);
        assert!(r.lens_correction(&m).is_none());
    }

    /// As Camera Raw 18.7 writes it: Denoise on sets manual noise reduction to 0, off
    /// brings Color's defaults back; the Amount stays.
    #[test]
    fn switching_ai_denoise_follows_lightrooms_noise_reduction() {
        let mut r = Recipe {
            noise_luma: 0.3,
            ..Default::default()
        };
        r.set_color_noise_defaults();
        r.ai_denoise.amount = 0.7;
        r.switch_ai_denoise(true);
        assert!(r.ai_denoise.enabled);
        assert_eq!((r.noise_luma, r.noise_chroma), (0., 0.));
        r.switch_ai_denoise(false);
        assert_eq!(r.noise_chroma, 0.25);
        assert_eq!(r.ai_denoise.amount, 0.7);
        // A Color amount set while on is the user's: kept when switching off.
        r.switch_ai_denoise(true);
        r.noise_chroma = 0.4;
        r.switch_ai_denoise(false);
        assert_eq!(r.noise_chroma, 0.4);
    }
}
