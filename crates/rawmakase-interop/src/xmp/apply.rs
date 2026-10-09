use super::PhotoMeasures;
use super::Preset;
use crate::model::recipe::Recipe;
use crate::{camera_data::Metadata, camera_profiles::CameraProfile, color::mul};
use anyhow::{Context, Result, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
fn number(values: &BTreeMap<String, String>, key: &str) -> Result<Option<f32>> {
    values
        .get(key)
        .map(|s| {
            let v = s
                .parse::<f32>()
                .with_context(|| format!("Invalid {key}: {s}"))?;
            ensure!(v.is_finite(), "Non-finite {key}");
            Ok(v)
        })
        .transpose()
}
fn boolean(values: &BTreeMap<String, String>, key: &str) -> Result<Option<bool>> {
    values
        .get(key)
        // Lightroom writes some flags empty (e.g. ConvertToGrayscale=""),
        // meaning not set.
        .filter(|s| !s.trim().is_empty())
        .map(|s| match s.to_ascii_lowercase().as_str() {
            "true" | "1" => Ok(true),
            "false" | "0" => Ok(false),
            _ => anyhow::bail!("Invalid boolean {key}"),
        })
        .transpose()
}
struct Settings<'a> {
    values: &'a BTreeMap<String, String>,
    seen: BTreeSet<String>,
}
impl Settings<'_> {
    fn assign(&mut self, key: &str, out: &mut f32, scale: f32, lo: f32, hi: f32) -> Result<()> {
        self.seen.insert(key.to_string());
        if let Some(value) = number(self.values, key)? {
            let value = value * scale;
            ensure!(
                (lo..=hi).contains(&value),
                "{key} is outside supported range"
            );
            *out = value;
        }
        Ok(())
    }
}
const METADATA: &[&str] = &[
    "Version",
    "ProcessVersion",
    "HasSettings",
    "HasCrop",
    "PresetType",
    "Cluster",
    "UUID",
    "SupportsAmount",
    "SupportsAmount2",
    "SupportsColor",
    "SupportsMonochrome",
    "SupportsHighDynamicRange",
    "SupportsNormalDynamicRange",
    "SupportsSceneReferred",
    "SupportsOutputReferred",
    "CameraModelRestriction",
    "Copyright",
    "ContactInfo",
    "Name",
    "ShortName",
    "SortName",
    "Description",
    "Group",
    "ToneCurveName",
    "ToneCurveName2012",
    "CameraProfileDigest",
    "ShowInPresets",
    "ShowInQuickActions",
    "OverrideLookVignette",
    "CompatibleVersion",
    "AlreadyApplied",
    "RawFileName",
    // Marks a preset made in RAWmakase, which it may update, rename or delete.
    "RAWmakasePreset",
    // Operators and white balance a recipe of an earlier RAWmakase kept from before
    // they were measured; one engine now renders every edit, so they are ignored.
    "RAWmakaseOriginal",
    "RAWmakaseMarkers",
    "RAWmakaseWhiteBalanceModel",
];
impl Preset {
    /// Apply to a private recipe, publishing only after every stage validates.
    pub fn apply(
        &self,
        base: &Recipe,
        m: &Metadata,
        profiles: &[Arc<CameraProfile>],
        image: Option<&dyn PhotoMeasures>,
    ) -> Result<Recipe> {
        ensure!(self.blockers.is_empty(), "{}", self.blockers.join("; "));
        let mut settings = Settings {
            values: &self.settings,
            seen: METADATA.iter().map(|key| key.to_string()).collect(),
        };
        let mut recipe = base.clone();
        self.apply_profile(&mut settings, &mut recipe, m, profiles)?;
        self.apply_basic(&mut settings, &mut recipe)?;
        // Auto white balance is measured on the crop, so geometry comes first.
        self.apply_geometry(&mut settings, &mut recipe, m)?;
        // Before white balance: Auto measures the photo as Upright frames it.
        self.apply_upright(&mut settings, &mut recipe)?;
        self.apply_white_balance(&mut settings, &mut recipe, m, image)?;
        self.apply_color_mixer(&mut settings, &mut recipe)?;
        self.apply_curves(&mut settings, &mut recipe)?;
        self.apply_grading(&mut settings, &mut recipe)?;
        self.apply_effects(&mut settings, &mut recipe)?;
        self.apply_auto_gray_mix(&mut settings, &mut recipe, m, image)?;
        self.apply_auto_tone(&mut settings, &mut recipe, m, image)?;
        self.apply_panels(&mut settings, &mut recipe)?;
        let skipped = self.apply_local(&mut recipe, m);
        ensure!(skipped.is_empty(), "{}", skipped.join("; "));
        self.validate_remaining(&mut settings)?;
        self.keep_upright_fitting(base, &mut recipe);
        recipe.preset_name = self.name.clone();
        recipe.preset_settings = self.settings.clone();
        recipe.validate()?;
        Ok(recipe)
    }
    /// Lightroom-style best effort: when `apply` rejects the preset (missing
    /// profile, camera restriction, unsupported settings), apply every stage
    /// that validates, keep the base values for stages that don't, and return
    /// what was skipped. Fails only if the combined recipe is invalid.
    pub fn apply_lenient(
        &self,
        base: &Recipe,
        m: &Metadata,
        profiles: &[Arc<CameraProfile>],
        image: Option<&dyn PhotoMeasures>,
    ) -> Result<(Recipe, Vec<String>)> {
        if let Ok(recipe) = self.apply(base, m, profiles, image) {
            return Ok((recipe, Vec::new()));
        }
        let mut warnings = self.blockers.clone();
        let mut settings = Settings {
            values: &self.settings,
            seen: METADATA.iter().map(|key| key.to_string()).collect(),
        };
        let mut recipe = base.clone();
        let mut stage = |recipe: &mut Recipe, result: &dyn Fn(&mut Recipe) -> Result<()>| {
            let backup = recipe.clone();
            if let Err(e) = result(recipe) {
                *recipe = backup;
                warnings.push(format!("{e:#}"));
            }
        };
        let settings = std::cell::RefCell::new(&mut settings);
        stage(&mut recipe, &|r| {
            self.apply_profile(&mut settings.borrow_mut(), r, m, profiles)
        });
        stage(&mut recipe, &|r| {
            self.apply_basic(&mut settings.borrow_mut(), r)
        });
        stage(&mut recipe, &|r| {
            self.apply_geometry(&mut settings.borrow_mut(), r, m)
        });
        stage(&mut recipe, &|r| {
            self.apply_upright(&mut settings.borrow_mut(), r)
        });
        stage(&mut recipe, &|r| {
            self.apply_white_balance(&mut settings.borrow_mut(), r, m, image)
        });
        stage(&mut recipe, &|r| {
            self.apply_color_mixer(&mut settings.borrow_mut(), r)
        });
        stage(&mut recipe, &|r| {
            self.apply_curves(&mut settings.borrow_mut(), r)
        });
        stage(&mut recipe, &|r| {
            self.apply_grading(&mut settings.borrow_mut(), r)
        });
        stage(&mut recipe, &|r| {
            self.apply_effects(&mut settings.borrow_mut(), r)
        });
        stage(&mut recipe, &|r| {
            self.apply_auto_gray_mix(&mut settings.borrow_mut(), r, m, image)
        });
        stage(&mut recipe, &|r| {
            self.apply_auto_tone(&mut settings.borrow_mut(), r, m, image)
        });
        stage(&mut recipe, &|r| {
            self.apply_panels(&mut settings.borrow_mut(), r)
        });
        stage(&mut recipe, &|_| {
            self.validate_remaining(&mut settings.borrow_mut())
        });
        // Spots and masks that convert apply; the rest are reported.
        let skipped = self.apply_local(&mut recipe, m);
        warnings.extend(skipped);
        self.keep_upright_fitting(base, &mut recipe);
        recipe.preset_name = self.name.clone();
        recipe.preset_settings = self.settings.clone();
        recipe.validate()?;
        Ok((recipe, warnings))
    }
    /// Upright corrections analysed through other lens settings than the result's no
    /// longer fit the photo: they are dropped for a new analysis, unless these settings
    /// bring Lightroom's own corrections, made with them.
    fn keep_upright_fitting(&self, base: &Recipe, r: &mut Recipe) {
        use crate::model::transform::LensInputs;
        // Whether the Upright stage installs corrections of its own: tried on a copy,
        // since a lenient apply rolls back a stage that fails.
        let mut trial = r.clone();
        trial.upright.corrections.clear();
        let mut settings = Settings {
            values: &self.settings,
            seen: BTreeSet::new(),
        };
        let brought = self.apply_upright(&mut settings, &mut trial).is_ok()
            && !trial.upright.corrections.is_empty();
        if !brought && LensInputs::of(r) != LensInputs::of(base) {
            r.upright.analyse_again();
        }
    }
    /// The camera profile this preset asks for, if any.
    fn requested_profile(&self) -> Option<&str> {
        self.settings
            .get("CameraProfile")
            .map(|name| match name.as_str() {
                "Default Profile" => "Adobe Standard",
                name => name,
            })
    }
    /// The imported profile named `name` for this camera. Built-in presets and a
    /// photo's own Lightroom edit fall back from Adobe Standard to the DNG's own
    /// profile, then RAWmakase Standard, and from Adobe Color to RAWmakase Color, so
    /// they render close to what was intended without Adobe's files (an edit made on
    /// Adobe Standard would otherwise keep RAWmakase Color, the new-photo default).
    /// Imported presets still need the exact profile. Another camera's profile is
    /// never used, and one Adobe look never stands in for another.
    fn resolve_profile(
        &self,
        name: &str,
        m: &Metadata,
        profiles: &[Arc<CameraProfile>],
    ) -> Option<Arc<CameraProfile>> {
        let find = |name: &str| {
            profiles
                .iter()
                .find(|p| p.name == name && p.ensure_camera(m).is_ok())
                .cloned()
        };
        find(name).or_else(|| {
            if !self.falls_back() {
                return None;
            }
            match name {
                "Adobe Standard" => crate::camera_profiles::builtin(m)
                    .or_else(|| find(crate::camera_profiles::open::STANDARD)),
                "Adobe Color" => find(crate::camera_profiles::open::COLOR),
                _ => None,
            }
        })
    }
    /// The imported enhanced look this preset names (Lightroom writes Adobe Color and
    /// the other Adobe looks as a `Look` over Adobe Standard). Where `resolve_profile`
    /// falls back, Adobe Color falls back to RAWmakase Color too.
    fn resolve_look(
        &self,
        m: &Metadata,
        profiles: &[Arc<CameraProfile>],
    ) -> Option<Arc<CameraProfile>> {
        profiles
            .iter()
            .find(|p| {
                p.name == self.look
                    && p.ensure_camera(m).is_ok()
                    && p.enhanced.as_ref().is_some_and(|look| {
                        self.settings
                            .get("RAWmakaseLookUUID")
                            .is_none_or(|uuid| look.uuid.eq_ignore_ascii_case(uuid))
                    })
            })
            .cloned()
            .or_else(|| {
                (self.falls_back() && self.look == "Adobe Color")
                    .then(|| {
                        profiles
                            .iter()
                            .find(|p| {
                                p.name == crate::camera_profiles::open::COLOR
                                    && p.ensure_camera(m).is_ok()
                            })
                            .cloned()
                    })
                    .flatten()
            })
    }
    /// Whether this preset's look, if it names one, can be rendered here.
    pub fn look_available(&self, m: &Metadata, profiles: &[Arc<CameraProfile>]) -> bool {
        self.look.is_empty() || self.resolve_look(m, profiles).is_some()
    }
    /// Built-in presets and a photo's own Lightroom edit stand in RAWmakase's profiles
    /// for missing Adobe ones; imported presets need the exact profile.
    fn falls_back(&self) -> bool {
        self.builtin || self.photo_settings
    }
    /// When this preset will render with a different profile than it names (see
    /// `resolve_profile`), the names of both.
    pub fn profile_substitute(
        &self,
        m: &Metadata,
        profiles: &[Arc<CameraProfile>],
    ) -> Option<(String, String)> {
        let (name, used) = if self.look.is_empty() {
            let name = self.requested_profile()?;
            (name, self.resolve_profile(name, m, profiles)?)
        } else {
            (self.look.as_str(), self.resolve_look(m, profiles)?)
        };
        (used.name != name).then(|| (name.to_string(), used.name.clone()))
    }
    fn apply_profile(
        &self,
        settings: &mut Settings<'_>,
        r: &mut Recipe,
        m: &Metadata,
        profiles: &[Arc<CameraProfile>],
    ) -> Result<()> {
        let v = settings.values;
        if let Some(model) = v.get("CameraModelRestriction").filter(|s| !s.is_empty()) {
            ensure!(
                model.eq_ignore_ascii_case(&m.model)
                    || model.eq_ignore_ascii_case(&format!("{} {}", m.make, m.model)),
                "Preset is restricted to {model}"
            );
        }
        settings.seen.insert("RequiresRGBTables".into());
        ensure!(
            boolean(v, "RequiresRGBTables")? != Some(true),
            "Preset requires external RGB tables"
        );
        settings.seen.insert("CameraProfile".into());
        if let Some(name) = self.requested_profile() {
            r.profile = Some(
                self.resolve_profile(name, m, profiles)
                    .with_context(|| format!("Missing camera profile ‘{name}’ for {}", m.model))?,
            );
        }
        settings.seen.insert("RAWmakaseLookUUID".into());
        if !self.look.is_empty() {
            r.profile = Some(self.resolve_look(m, profiles).with_context(|| {
                format!(
                    "Missing or unsupported enhanced profile ‘{}’ for {}",
                    self.look, m.model
                )
            })?);
        }
        // A new profile starts at 100%; a look that supports Amount takes the preset's.
        settings.seen.insert(super::look::SETTING.into());
        if v.contains_key("CameraProfile") || !self.look.is_empty() {
            r.profile_amount = 1.;
        }
        if let Some(amount) = v.get(super::look::SETTING)
            && !self.look.is_empty()
            && r.profile.as_ref().is_some_and(|p| p.supports_amount())
        {
            r.profile_amount = super::look::LookAmount::parse(amount)?.0;
        }
        if v.contains_key("CameraProfile") || !self.look.is_empty() {
            r.use_camera_baseline(m);
        }
        Ok(())
    }

    fn apply_basic(&self, settings: &mut Settings<'_>, r: &mut Recipe) -> Result<()> {
        settings.assign("Exposure2012", &mut r.exposure, 1., -8., 8.)?;
        settings.assign("Contrast2012", &mut r.contrast, 0.01, -1., 1.)?;
        settings.assign("Highlights2012", &mut r.highlights, 0.01, -1., 1.)?;
        settings.assign("Shadows2012", &mut r.shadows, 0.01, -1., 1.)?;
        settings.assign("Whites2012", &mut r.whites, 0.01, -1., 1.)?;
        settings.assign("Blacks2012", &mut r.blacks, 0.01, -1., 1.)?;
        settings.assign("Saturation", &mut r.saturation, 0.01, -1., 1.)?;
        settings.assign("Vibrance", &mut r.vibrance, 0.01, -1., 1.)?;
        settings.assign("Sharpness", &mut r.sharpening, 1. / 150., 0., 1.)?;
        settings.assign("SharpenRadius", &mut r.sharpening_radius, 1., 0.5, 3.)?;
        settings.assign("SharpenDetail", &mut r.sharpening_detail, 0.01, 0., 1.)?;
        settings.assign(
            "SharpenEdgeMasking",
            &mut r.sharpening_masking,
            0.01,
            0.,
            1.,
        )?;
        settings.assign("LuminanceSmoothing", &mut r.noise_luma, 0.01, 0., 1.)?;
        settings.assign("ColorNoiseReduction", &mut r.noise_chroma, 0.01, 0., 1.)?;
        Ok(())
    }

    fn apply_white_balance(
        &self,
        settings: &mut Settings<'_>,
        r: &mut Recipe,
        m: &Metadata,
        image: Option<&dyn PhotoMeasures>,
    ) -> Result<()> {
        let v = settings.values;
        settings.seen.insert("WhiteBalance".into());
        settings.seen.insert("Temperature".into());
        settings.seen.insert("Tint".into());
        match v.get("WhiteBalance").map(String::as_str) {
            Some("As Shot") => {
                r.reset_white_balance(m);
                // Photo sidecars/catalogs contain resolved WB numbers, even when
                // the provenance label remains As Shot. Presets retain camera-relative semantics.
                if self.photo_settings
                    && let (Some(temperature), Some(tint)) =
                        (number(v, "Temperature")?, number(v, "Tint")?)
                {
                    r.temperature = temperature;
                    r.tint = tint;
                    r.update_wb(m);
                }
            }
            Some("Auto") => {
                if self.photo_settings
                    && let (Some(temperature), Some(tint)) =
                        (number(v, "Temperature")?, number(v, "Tint")?)
                {
                    r.temperature = temperature;
                    r.tint = tint;
                    r.update_wb(m);
                    r.auto_white_balance = Some([r.temperature, r.tint]);
                } else if let Some(photo) = image {
                    // Settings without resolved values get the WB menu's Auto.
                    *r = photo.auto_white_balance(r)?;
                }
            }
            Some("Custom") | None => {
                let mut changed = false;
                if let Some(t) = number(v, "Temperature")? {
                    r.temperature = t;
                    changed = true;
                }
                if let Some(t) = number(v, "Tint")? {
                    r.tint = t;
                    changed = true;
                }
                if changed {
                    r.update_wb(m);
                }
            }
            // Lightroom's named presets. Photo settings carry the values Lightroom
            // resolved for the camera; a preset may name the mode alone.
            Some(name)
                if let Some(named) =
                    crate::model::white_balance::NamedWhiteBalance::from_name(name) =>
            {
                let values = named.values();
                r.temperature = number(v, "Temperature")?.unwrap_or(values.temperature);
                r.tint = number(v, "Tint")?.unwrap_or(values.tint);
                r.update_wb(m);
                r.auto_white_balance = None;
            }
            Some(other) => anyhow::bail!("Unsupported white balance mode: {other}"),
        }
        Ok(())
    }

    fn apply_color_mixer(&self, settings: &mut Settings<'_>, r: &mut Recipe) -> Result<()> {
        let v = settings.values;
        settings.seen.insert("PointColors".into());
        settings.seen.insert("ColorVariance".into());
        if let Some(points) = v.get("PointColors") {
            r.point_colors = crate::model::point_color::parse_list(
                points,
                v.get("ColorVariance").map(String::as_str),
            )?;
        }
        let bands = [
            "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta",
        ];
        for (i, band) in bands.iter().enumerate() {
            for (j, control) in ["Hue", "Saturation", "Luminance"].iter().enumerate() {
                settings.assign(
                    &format!("{control}Adjustment{band}"),
                    &mut r.hsl[i][j],
                    0.01,
                    -1.,
                    1.,
                )?;
            }
            settings.assign(
                &format!("GrayMixer{band}"),
                &mut r.effects.gray_mix[i],
                0.01,
                -1.,
                1.,
            )?;
        }
        for (i, band) in ["Red", "Green", "Blue"].iter().enumerate() {
            settings.assign(
                &format!("{band}Hue"),
                &mut r.effects.calibration[i][0],
                0.01,
                -1.,
                1.,
            )?;
            settings.assign(
                &format!("{band}Saturation"),
                &mut r.effects.calibration[i][1],
                0.01,
                -1.,
                1.,
            )?;
        }
        settings.assign("ShadowTint", &mut r.effects.shadow_tint, 0.01, -1., 1.)?;
        Ok(())
    }

    fn apply_curves(&self, settings: &mut Settings<'_>, r: &mut Recipe) -> Result<()> {
        let v = settings.values;
        for (i, name) in ["Shadows", "Darks", "Lights", "Highlights"]
            .iter()
            .enumerate()
        {
            settings.assign(
                &format!("Parametric{name}"),
                &mut r.effects.parametric[i],
                0.01,
                -1.,
                1.,
            )?;
        }
        settings.assign(
            "CurveRefineSaturation",
            &mut r.curve_saturation,
            0.01,
            0.,
            2.,
        )?;
        for (i, name) in ["Shadow", "Midtone", "Highlight"].iter().enumerate() {
            settings.assign(
                &format!("Parametric{name}Split"),
                &mut r.effects.splits[i],
                0.01,
                0.01,
                0.99,
            )?;
        }
        if let Some(c) = self
            .curves
            .get("ToneCurvePV2012")
            .or_else(|| self.curves.get("ToneCurve"))
        {
            r.curve = c.clone();
        }
        // Camera Raw reads the Red, Green and Blue curves only as the full set, with
        // the master curve, that Lightroom writes; a partial set changes nothing.
        let channel_set = |prefix: &str| {
            self.curves.get(prefix)?;
            let [red, green, blue] =
                ["Red", "Green", "Blue"].map(|name| self.curves.get(&format!("{prefix}{name}")));
            Some([red?, green?, blue?])
        };
        if let Some(set) = channel_set("ToneCurvePV2012").or_else(|| channel_set("ToneCurve")) {
            r.effects.channels = set.map(Clone::clone);
        }
        settings.seen.insert("ConvertToGrayscale".into());
        if let Some(b) = boolean(v, "ConvertToGrayscale")? {
            r.effects.monochrome = b;
        }
        Ok(())
    }

    /// Lightroom resolves Auto black & white into the stored `GrayMixer` values,
    /// which then render as Auto off does in Camera Raw, so stored values are kept.
    /// Without them (a preset naming Auto alone), the Auto mix is estimated from the
    /// photo as the B&W panel's Auto does; without the photo the current mix stays,
    /// as Auto white balance leaves white balance.
    fn apply_auto_gray_mix(
        &self,
        settings: &mut Settings<'_>,
        r: &mut Recipe,
        m: &Metadata,
        image: Option<&dyn PhotoMeasures>,
    ) -> Result<()> {
        settings.seen.insert("AutoGrayscaleMix".into());
        if self.leaves_auto_gray_mix(r)?
            && let Some(photo) = image
        {
            r.effects.gray_mix = photo.auto_gray_mix(r, m);
        }
        Ok(())
    }
    /// Whether Auto black & white applies to a black & white result (by Treatment
    /// or a monochrome profile) without the mixer values Lightroom resolves for it.
    fn leaves_auto_gray_mix(&self, r: &Recipe) -> Result<bool> {
        let v = &self.settings;
        let auto = boolean(v, "AutoGrayscaleMix")? == Some(true);
        Ok(auto
            && r.with_profile_adjustments().effects.monochrome
            && !v.keys().any(|k| k.starts_with("GrayMixer")))
    }

    fn apply_grading(&self, settings: &mut Settings<'_>, r: &mut Recipe) -> Result<()> {
        let v = settings.values;
        for (i, name) in [(0, "Shadow"), (2, "Highlight")] {
            settings.assign(
                &format!("SplitToning{name}Hue"),
                &mut r.grading[i][0],
                1. / 360.,
                0.,
                1.,
            )?;
            settings.assign(
                &format!("SplitToning{name}Saturation"),
                &mut r.grading[i][1],
                0.01,
                0.,
                1.,
            )?;
            settings.assign(
                &format!("ColorGrade{name}Lum"),
                &mut r.grading[i][2],
                0.01,
                -1.,
                1.,
            )?;
        }
        settings.assign(
            "ColorGradeMidtoneHue",
            &mut r.grading[1][0],
            1. / 360.,
            0.,
            1.,
        )?;
        settings.assign("ColorGradeMidtoneSat", &mut r.grading[1][1], 0.01, 0., 1.)?;
        settings.assign("ColorGradeMidtoneLum", &mut r.grading[1][2], 0.01, -1., 1.)?;
        settings.assign("SplitToningBalance", &mut r.effects.balance, 0.01, -1., 1.)?;
        // Adobe's pre-Color-Grading split toning used full tonal overlap.
        // Only split-tone records without any modern grading keys imply that mode.
        if v.keys().any(|key| key.starts_with("SplitToning"))
            && !v.keys().any(|key| key.starts_with("ColorGrade"))
        {
            r.effects.blending = 1.;
            r.grading[1] = [0.; 3];
            r.grading[0][2] = 0.;
            r.grading[2][2] = 0.;
            r.effects.global_grade = [0.; 3];
        }
        settings.assign("ColorGradeBlending", &mut r.effects.blending, 0.01, 0., 1.)?;
        for (i, (name, scale)) in [("Hue", 1. / 360.), ("Sat", 0.01), ("Lum", 0.01)]
            .iter()
            .enumerate()
        {
            settings.assign(
                &format!("ColorGradeGlobal{name}"),
                &mut r.effects.global_grade[i],
                *scale,
                if i == 2 { -1. } else { 0. },
                1.,
            )?;
        }
        Ok(())
    }

    fn apply_effects(&self, settings: &mut Settings<'_>, r: &mut Recipe) -> Result<()> {
        let v = settings.values;
        settings.assign("Clarity2012", &mut r.effects.clarity, 0.01, -1., 1.)?;
        settings.assign("Texture", &mut r.effects.texture, 0.01, -1., 1.)?;
        settings.assign("Dehaze", &mut r.effects.dehaze, 0.01, -1., 1.)?;
        settings.assign("GrainAmount", &mut r.effects.grain, 0.01, 0., 1.)?;
        settings.assign("GrainSize", &mut r.effects.grain_size, 0.01, 0., 1.)?;
        settings.assign(
            "GrainFrequency",
            &mut r.effects.grain_roughness,
            0.01,
            0.,
            1.,
        )?;
        settings.seen.insert("GrainSeed".into());
        if let Some(seed) = v.get("GrainSeed") {
            r.effects.grain_seed = seed.parse().context("Invalid grain seed")?;
        }
        settings.assign(
            "PostCropVignetteAmount",
            &mut r.effects.vignette,
            0.01,
            -1.,
            1.,
        )?;
        settings.assign(
            "PostCropVignetteMidpoint",
            &mut r.effects.vignette_midpoint,
            0.01,
            0.,
            1.,
        )?;
        settings.assign(
            "PostCropVignetteRoundness",
            &mut r.effects.vignette_roundness,
            0.01,
            -1.,
            1.,
        )?;
        settings.assign(
            "PostCropVignetteFeather",
            &mut r.effects.vignette_feather,
            0.01,
            0.,
            1.,
        )?;
        settings.assign(
            "PostCropVignetteHighlightContrast",
            &mut r.effects.vignette_highlights,
            0.01,
            0.,
            1.,
        )?;
        settings.seen.insert("PostCropVignetteStyle".into());
        if let Some(style) = v.get("PostCropVignetteStyle") {
            r.effects.vignette_style = style
                .parse::<u8>()
                .ok()
                .and_then(|code| code.try_into().ok())
                .with_context(|| format!("Unsupported PostCropVignetteStyle {style}"))?;
        }
        settings.assign(
            "VignetteAmount",
            &mut r.effects.lens_vignette,
            0.01,
            -1.,
            1.,
        )?;
        settings.assign(
            "VignetteMidpoint",
            &mut r.effects.lens_vignette_midpoint,
            0.01,
            0.,
            1.,
        )?;
        for (i, name) in ["Purple", "Green"].iter().enumerate() {
            settings.assign(
                &format!("Defringe{name}Amount"),
                &mut r.effects.defringe[i],
                0.05,
                0.,
                1.,
            )?;
            settings.assign(
                &format!("Defringe{name}HueLo"),
                &mut r.effects.defringe_ranges[i][0],
                0.01,
                0.,
                1.,
            )?;
            settings.assign(
                &format!("Defringe{name}HueHi"),
                &mut r.effects.defringe_ranges[i][1],
                0.01,
                0.,
                1.,
            )?;
        }
        settings.assign(
            "LuminanceNoiseReductionDetail",
            &mut r.effects.luma_detail,
            0.01,
            0.,
            1.,
        )?;
        settings.assign(
            "LuminanceNoiseReductionContrast",
            &mut r.effects.luma_contrast,
            0.01,
            0.,
            1.,
        )?;
        settings.assign(
            "ColorNoiseReductionDetail",
            &mut r.effects.chroma_detail,
            0.01,
            0.,
            1.,
        )?;
        settings.assign(
            "ColorNoiseReductionSmoothness",
            &mut r.effects.chroma_smoothness,
            0.01,
            0.,
            1.,
        )?;
        Ok(())
    }

    fn apply_auto_tone(
        &self,
        settings: &mut Settings<'_>,
        r: &mut Recipe,
        m: &Metadata,
        image: Option<&dyn PhotoMeasures>,
    ) -> Result<()> {
        let v = settings.values;
        settings.seen.insert("AutoTone".into());
        // Lightroom stores the exposure Auto Tone chose; recompute only when it is absent,
        // so every route (open, reset, history, hover) renders the same edit.
        if boolean(v, "AutoTone")? == Some(true)
            && !v.contains_key("Exposure2012")
            && let Some(photo) = image
        {
            let mut l: Vec<f32> = photo
                .camera_image()
                .pixels
                .iter()
                .step_by(64)
                .map(|p| {
                    let q = mul(m.matrix, std::array::from_fn(|c| p[c] * r.wb[c]));
                    crate::color::luminance(q).max(1e-6)
                })
                .collect();
            l.sort_by(f32::total_cmp);
            if !l.is_empty() {
                r.exposure = (0.18 / l[l.len() / 2]).log2().clamp(-8., 8.);
            }
        }
        Ok(())
    }

    /// Lightroom's panel switches. A panel is off when any of its keys says so, and
    /// turned back on by a setting that says it is on.
    fn apply_panels(&self, settings: &mut Settings<'_>, r: &mut Recipe) -> Result<()> {
        use crate::model::panels::{Panel, PanelState};
        for panel in Panel::ALL {
            let mut state = None;
            for key in panel.lightroom_keys() {
                settings.seen.insert(key.to_string());
                match boolean(settings.values, key)? {
                    Some(false) => state = Some(PanelState::Off),
                    Some(true) if state.is_none() => state = Some(PanelState::On),
                    _ => {}
                }
            }
            if let Some(state) = state {
                r.panels.set(panel, state);
            }
        }
        Ok(())
    }

    /// Straighten, lens corrections, Transform and crop.
    fn apply_geometry(
        &self,
        settings: &mut Settings<'_>,
        r: &mut Recipe,
        m: &Metadata,
    ) -> Result<()> {
        let v = settings.values;
        settings.assign("CropAngle", &mut r.straighten, 1., -45., 45.)?;
        settings.seen.insert("LensProfileEnable".into());
        // Which Adobe profile the edit uses. Whether Lightroom took it from the RAW
        // instead does not change which profile is named.
        for key in [
            "LensProfileSetup",
            "LensProfileName",
            "LensProfileFilename",
            "LensProfileDigest",
            "LensProfileIsEmbedded",
        ] {
            settings.seen.insert(key.into());
        }
        let embedded = boolean(v, "LensProfileIsEmbedded")?.unwrap_or(false);
        let text = |key: &str| v.get(key).map(|s| s.trim().to_string()).unwrap_or_default();
        let (name, filename) = (text("LensProfileName"), text("LensProfileFilename"));
        let id = (!name.is_empty() || !filename.is_empty()).then(|| {
            crate::lens::choice::LensProfileId {
                name,
                filename,
                digest: text("LensProfileDigest"),
                embedded,
            }
        });
        if let Some(setup) = v.get("LensProfileSetup") {
            // A Setup names its profile, or none (a preset's "Default").
            r.lens_profile_choice = crate::lens::choice::LensProfileChoice {
                setup: crate::lens::choice::LensProfileSetup::from_xmp(setup.trim()),
                id,
            };
        } else if id.is_some() {
            r.lens_profile_choice.id = id;
        }
        settings.assign(
            "LensProfileDistortionScale",
            &mut r.lens_distortion,
            0.01,
            0.,
            2.,
        )?;
        settings.assign(
            "LensProfileVignettingScale",
            &mut r.lens_vignetting,
            0.01,
            0.,
            2.,
        )?;
        if let Some(enable) = number(v, "LensProfileEnable")? {
            let state = if enable != 0. {
                crate::model::recipe::ProfileCorrections::On
            } else {
                crate::model::recipe::ProfileCorrections::Off
            };
            r.set_profile_corrections(m, state);
        }
        settings.assign(
            "LensManualDistortionAmount",
            &mut r.lens_manual_distortion,
            0.01,
            -1.,
            1.,
        )?;
        settings.seen.insert("AutoLateralCA".into());
        if let Some(ca) = number(v, "AutoLateralCA")? {
            r.lens_ca = ca != 0.;
        }
        let t = &mut r.transform;
        settings.assign("PerspectiveVertical", &mut t.vertical, 0.01, -1., 1.)?;
        settings.assign("PerspectiveHorizontal", &mut t.horizontal, 0.01, -1., 1.)?;
        settings.assign("PerspectiveRotate", &mut t.rotate, 1., -10., 10.)?;
        settings.assign("PerspectiveAspect", &mut t.aspect, 0.01, -1., 1.)?;
        settings.assign("PerspectiveScale", &mut t.scale, 0.01, 0.5, 1.5)?;
        settings.assign("PerspectiveX", &mut t.offset_x, 0.01, -1., 1.)?;
        settings.assign("PerspectiveY", &mut t.offset_y, 0.01, -1., 1.)?;
        for (i, name) in ["Left", "Top", "Right", "Bottom"].iter().enumerate() {
            settings.assign(&format!("Crop{name}"), &mut r.crop[i], 1., 0., 1.)?;
        }
        // Lightroom's Constrain Crop, not to be confused with `CropConstrainToUnitSquare`.
        settings.seen.insert("CropConstrainToWarp".into());
        if let Some(constrain) = number(v, "CropConstrainToWarp")? {
            r.constrain_crop = constrain != 0.;
        }
        Ok(())
    }

    /// Lightroom's Upright mode and the corrections it stored for every mode.
    fn apply_upright(&self, settings: &mut Settings<'_>, r: &mut Recipe) -> Result<()> {
        use crate::model::transform::UprightMode;
        let v = settings.values;
        settings.seen.insert("PerspectiveUpright".into());
        let mut lightroom = BTreeMap::new();
        let mut corrections = Vec::new();
        let mut guides = Vec::new();
        for (key, value) in v.range("Upright".to_string()..) {
            let Some(name) = key.strip_prefix("Upright") else {
                break;
            };
            settings.seen.insert(key.clone());
            if name == "TransformCount" || name == "FourSegmentsCount" {
                continue;
            }
            // Guided's guides: "x1,y1,x2,y2" in 0–1 of the frame as recorded.
            if let Some(i) = name.strip_prefix("FourSegments_") {
                let i: usize = i
                    .parse()
                    .ok()
                    .filter(|&i| i < crate::model::transform::MAX_GUIDES)
                    .with_context(|| format!("Unsupported {key}"))?;
                let guide = crate::model::transform::parse_guide(value)
                    .with_context(|| format!("Invalid {key}"))?;
                if guides.len() <= i {
                    guides.resize(i + 1, None);
                }
                guides[i] = Some(guide);
                continue;
            }
            let Some(i) = name.strip_prefix("Transform_") else {
                lightroom.insert(key.clone(), value.clone());
                continue;
            };
            let i: usize = i
                .parse()
                .ok()
                .filter(|&i| i < UprightMode::ALL.len())
                .with_context(|| format!("Unsupported {key}"))?;
            let m: Vec<f32> = value
                .split(',')
                .map(|x| x.trim().parse::<f32>())
                .collect::<Result<_, _>>()
                .with_context(|| format!("Invalid {key}"))?;
            let m: [f32; 9] = m
                .try_into()
                .ok()
                .filter(|m: &[f32; 9]| m.iter().all(|x| x.is_finite()))
                .with_context(|| format!("Invalid {key}"))?;
            if corrections.len() <= i {
                corrections.resize(i + 1, None);
            }
            corrections[i] = Some(m);
        }
        // Lightroom stores every mode; keep only the unbroken run from Off, so a mode
        // without its own correction is never stood in for by an identity.
        let corrections: Vec<[f32; 9]> = corrections.into_iter().map_while(|m| m).collect();
        let guides: Vec<_> = guides.into_iter().flatten().collect();
        let Some(code) = number(v, "PerspectiveUpright")? else {
            ensure!(
                corrections.is_empty(),
                "Upright corrections without PerspectiveUpright"
            );
            return Ok(());
        };
        let mode = UprightMode::from_code(code as usize)
            .filter(|_| code.fract() == 0.)
            .with_context(|| format!("Unsupported PerspectiveUpright {code}"))?;
        // A preset names only the mode; the app analyses each photo it is applied to.
        // A photo's own settings always carry Lightroom's corrections.
        let stored = mode == UprightMode::Off || mode.code() < corrections.len();
        ensure!(
            stored || !self.photo_settings,
            "PerspectiveUpright without Lightroom's stored correction is not supported yet"
        );
        // Guided can't be analysed without its guides.
        ensure!(
            stored || mode != UprightMode::Guided,
            "Guided Upright without Lightroom's stored correction is not supported yet"
        );
        r.upright = crate::model::transform::Upright {
            mode,
            corrections,
            guides,
            lightroom,
        };
        Ok(())
    }

    /// Lightroom's spot removal, red eye corrections and masks, replacing the recipe's when the settings
    /// have them; returns what could not be converted.
    fn apply_local(&self, r: &mut Recipe, m: &Metadata) -> Vec<String> {
        if self.local.is_empty() {
            return Vec::new();
        }
        let edits = super::local::convert(
            &self.local,
            crate::model::image_frame::ImageFrame::for_metadata(m),
        );
        if let Some(retouch) = edits.retouch {
            r.retouch = retouch;
        }
        if let Some(red_eye) = edits.red_eye {
            r.red_eye = red_eye.into();
        }
        if let Some(masks) = edits.masks {
            r.masks = masks;
        }
        edits.skipped
    }
    fn validate_remaining(&self, settings: &mut Settings<'_>) -> Result<()> {
        let v = settings.values;
        // No-op geometry/default flags are safe; active unsupported operations are explicit blockers.
        for (key, default) in [
            ("HDREditMode", "0"),
            ("IncrementalTemperature", "0"),
            ("IncrementalTint", "0"),
            // Camera Raw 18.6 effects without rendering support yet.
            ("Glow", "0"),
            ("ReshapeAmount", "0"),
        ] {
            settings.seen.insert(key.into());
            if let Some(value) = v.get(key) {
                ensure!(
                    value.parse::<f32>().ok() == default.parse::<f32>().ok(),
                    "Active {key} requires rendering support not yet available"
                );
            }
        }
        // Lightroom 15 records whether the crop is kept inside the image; it only
        // constrains the crop tool and does not change rendering.
        settings.seen.insert("CropConstrainToUnitSquare".into());
        // Lightroom 15 writes these into every record. Glow's own controls do nothing
        // while Glow is 0 (an active Glow is refused above), the SDR and HDR values
        // apply only in HDR editing, and Distraction Removal's switch changes nothing
        // without removals, which are reported on their own.
        let at_rest = |key: &str| v.get(key).is_none_or(|value| value.parse() == Ok(0f32));
        let mut neutral = vec!["EnableDistractionRemoval"];
        if at_rest("Glow") {
            neutral.extend(["GlowRange", "GlowSpread", "GlowStyle", "GlowWarmth"]);
        }
        if at_rest("HDREditMode") {
            neutral.extend([
                "HDRMaxValue",
                "SDRBlend",
                "SDRBrightness",
                "SDRClarity",
                "SDRContrast",
                "SDRHighlights",
                "SDRShadows",
                "SDRWhites",
            ]);
        }
        settings.seen.extend(neutral.into_iter().map(String::from));
        let unknown: Vec<_> = v
            .keys()
            .filter(|k| !settings.seen.contains(*k))
            .cloned()
            .collect();
        ensure!(
            unknown.is_empty(),
            "Unsupported preset settings: {}",
            unknown.join(", ")
        );
        Ok(())
    }
}
