use super::PhotoMeasures;
use super::Preset;
use crate::lens::lensfun::installed::Handle;
use crate::model::camera_raw as crs;
use crate::model::recipe::Recipe;
use crate::{camera_data::Metadata, camera_profiles::CameraProfile};
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
/// The settings Lightroom's Auto Settings sets.
pub(crate) const AUTO_TONE_KEYS: [&str; 8] = [
    crs::EXPOSURE.key,
    crs::CONTRAST.key,
    crs::HIGHLIGHTS.key,
    crs::SHADOWS.key,
    crs::WHITES.key,
    crs::BLACKS.key,
    crs::VIBRANCE.key,
    crs::SATURATION.key,
];
/// The value Lightroom's catalog holds in each of [`AUTO_TONE_KEYS`] while an Auto
/// Settings is still to be measured.
const UNMEASURED: f32 = -999999.;
/// Whether `key` holds Lightroom's placeholder for an Auto Settings not measured
/// yet: [`UNMEASURED`], with `AutoTone` on. Any other value is read as it is.
fn unmeasured(values: &BTreeMap<String, String>, key: &str) -> Result<bool> {
    Ok(AUTO_TONE_KEYS.contains(&key)
        && boolean(values, "AutoTone")? == Some(true)
        && values
            .get(key)
            .is_some_and(|v| v.trim().parse::<f32>().ok() == Some(UNMEASURED)))
}
struct Settings<'a> {
    values: &'a BTreeMap<String, String>,
    seen: BTreeSet<String>,
}
impl Settings<'_> {
    /// [`Self::assign`], leaving `out` as it is for an Auto Settings still to be
    /// measured (see [`unmeasured`]).
    fn assign_tone(&mut self, slider: crs::Slider, out: &mut f32) -> Result<()> {
        if unmeasured(self.values, slider.key)? {
            self.seen.insert(slider.key.to_string());
            return Ok(());
        }
        let limit = if slider == crs::EXPOSURE { 8. } else { 1. };
        self.assign(slider, out, -limit, limit)
    }
    /// `slider`'s value in recipe units into `out`, when it is set; an error outside
    /// `lo..=hi` (in recipe units).
    fn assign(&mut self, slider: crs::Slider, out: &mut f32, lo: f32, hi: f32) -> Result<()> {
        let key = slider.key;
        self.seen.insert(key.to_string());
        if let Some(value) = number(self.values, key)? {
            let value = value * slider.scale();
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
    // Lightroom 18 marks presets that may carry filters; `FilterList` holds them.
    "AllowFilters",
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
    /// Apply to a private recipe, publishing only after every stage validates. A
    /// Lensfun lens the settings name resolves against the database installed now.
    pub fn apply(
        &self,
        base: &Recipe,
        m: &Metadata,
        profiles: &[Arc<CameraProfile>],
        image: Option<&dyn PhotoMeasures>,
    ) -> Result<Recipe> {
        self.apply_with_lensfun(base, m, profiles, image, &Handle::Installed)
    }
    /// [`apply`](Self::apply), resolving a Lensfun lens against `lensfun`.
    pub(crate) fn apply_with_lensfun(
        &self,
        base: &Recipe,
        m: &Metadata,
        profiles: &[Arc<CameraProfile>],
        image: Option<&dyn PhotoMeasures>,
        lensfun: &Handle,
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
        self.apply_geometry(&mut settings, &mut recipe, m, lensfun)?;
        // Before white balance: Auto measures the photo as Upright frames it.
        self.apply_upright(&mut settings, &mut recipe)?;
        self.apply_white_balance(&mut settings, &mut recipe, m, image)?;
        self.apply_color_mixer(&mut settings, &mut recipe)?;
        self.apply_curves(&mut settings, &mut recipe)?;
        self.apply_grading(&mut settings, &mut recipe)?;
        self.apply_effects(&mut settings, &mut recipe)?;
        self.apply_auto_gray_mix(&mut settings, &mut recipe, m, image)?;
        self.apply_auto_tone(&mut settings, &mut recipe, image)?;
        self.apply_panels(&mut settings, &mut recipe)?;
        self.apply_hdr(&mut settings, &mut recipe)?;
        let skipped = self.apply_local(&mut recipe, m);
        ensure!(skipped.is_empty(), "{}", skipped.join("; "));
        self.validate_remaining(&mut settings, m)?;
        self.keep_upright_fitting(base, &mut recipe);
        recipe.preset_name = self.name.clone();
        recipe.preset_settings = self.settings.clone();
        recipe.validate()?;
        Ok(recipe)
    }
    /// Refuses settings with HDR on while it can't be rendered. Best-effort readers
    /// call it first, so HDR on is never dropped as one warning among others and the
    /// edit rendered as SDR ([`HdrSettings::check_renderable`]).
    ///
    /// [`HdrSettings::check_renderable`]: crate::model::hdr::HdrSettings::check_renderable
    pub(crate) fn check_hdr_renderable(&self) -> Result<()> {
        let on = number(&self.settings, "HDREditMode").ok().flatten() == Some(1.);
        crate::model::hdr::HdrSettings {
            enabled: on,
            ..Default::default()
        }
        .check_renderable()
    }
    /// Lightroom-style best effort: when `apply` rejects the preset (missing
    /// profile, camera restriction, unsupported settings), apply every stage
    /// that validates, keep the base values for stages that don't, and return
    /// what was skipped. Fails if the combined recipe is invalid, and for HDR on,
    /// which is never skipped ([`Self::check_hdr_renderable`]).
    pub fn apply_lenient(
        &self,
        base: &Recipe,
        m: &Metadata,
        profiles: &[Arc<CameraProfile>],
        image: Option<&dyn PhotoMeasures>,
    ) -> Result<(Recipe, Vec<String>)> {
        self.apply_lenient_with_lensfun(base, m, profiles, image, &Handle::Installed)
    }
    /// [`apply_lenient`](Self::apply_lenient), resolving a Lensfun lens against
    /// `lensfun`.
    pub(crate) fn apply_lenient_with_lensfun(
        &self,
        base: &Recipe,
        m: &Metadata,
        profiles: &[Arc<CameraProfile>],
        image: Option<&dyn PhotoMeasures>,
        lensfun: &Handle,
    ) -> Result<(Recipe, Vec<String>)> {
        self.check_hdr_renderable()?;
        if let Ok(recipe) = self.apply_with_lensfun(base, m, profiles, image, lensfun) {
            return Ok((recipe, self.ignored_white_balance(m).into_iter().collect()));
        }
        let mut warnings = self.blockers.clone();
        warnings.extend(self.ignored_white_balance(m));
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
            self.apply_geometry(&mut settings.borrow_mut(), r, m, lensfun)
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
            self.apply_auto_tone(&mut settings.borrow_mut(), r, image)
        });
        stage(&mut recipe, &|r| {
            self.apply_panels(&mut settings.borrow_mut(), r)
        });
        stage(&mut recipe, &|r| {
            self.apply_hdr(&mut settings.borrow_mut(), r)
        });
        stage(&mut recipe, &|_| {
            self.validate_remaining(&mut settings.borrow_mut(), m)
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
    /// The fitted camera look these settings carry (`rawmakase:FittedProfile`), for
    /// a look that names one: an error when it is missing, damaged, altered or for
    /// another camera, which the settings then can't render as they were made.
    fn fitted_profile(&self, m: &Metadata) -> Result<Option<CameraProfile>> {
        let carried = self.settings.get(super::FITTED_PROFILE_KEY);
        if self.look != crate::camera_profiles::fitted::LOOK && carried.is_none() {
            return Ok(None);
        }
        let carried = carried.context("The fitted camera look's profile is missing")?;
        let p = crate::camera_profiles::fitted::from_carried(carried, m)
            .context("The fitted camera look can't be used")?;
        if let Some(uuid) = self.settings.get("RAWmakaseLookUUID") {
            let own = p.enhanced.as_ref().map(|e| e.uuid.as_str()).unwrap_or("");
            ensure!(
                own.eq_ignore_ascii_case(uuid),
                "The fitted camera look does not match its UUID"
            );
        }
        Ok(Some(p))
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
        settings.seen.insert(super::FITTED_PROFILE_KEY.into());
        settings.seen.insert("RAWmakaseLookUUID".into());
        // A fitted camera look renders from the profile the settings carry, checked,
        // never from a profile of its name: no other photo's fit is this one.
        if let Some(fitted) = self.fitted_profile(m)? {
            r.profile = Some(Arc::new(fitted));
        } else {
            if let Some(name) = self.requested_profile() {
                r.profile =
                    Some(self.resolve_profile(name, m, profiles).with_context(|| {
                        format!("Missing camera profile ‘{name}’ for {}", m.model)
                    })?);
            }
            if !self.look.is_empty() {
                r.profile = Some(self.resolve_look(m, profiles).with_context(|| {
                    format!(
                        "Missing or unsupported enhanced profile ‘{}’ for {}",
                        self.look, m.model
                    )
                })?);
            }
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
        settings.assign_tone(crs::EXPOSURE, &mut r.exposure)?;
        settings.assign_tone(crs::CONTRAST, &mut r.contrast)?;
        settings.assign_tone(crs::HIGHLIGHTS, &mut r.highlights)?;
        settings.assign_tone(crs::SHADOWS, &mut r.shadows)?;
        settings.assign_tone(crs::WHITES, &mut r.whites)?;
        settings.assign_tone(crs::BLACKS, &mut r.blacks)?;
        settings.assign_tone(crs::SATURATION, &mut r.saturation)?;
        settings.assign_tone(crs::VIBRANCE, &mut r.vibrance)?;
        settings.assign(crs::SHARPNESS, &mut r.sharpening, 0., 1.)?;
        settings.assign(crs::SHARPEN_RADIUS, &mut r.sharpening_radius, 0.5, 3.)?;
        settings.assign(crs::SHARPEN_DETAIL, &mut r.sharpening_detail, 0., 1.)?;
        settings.assign(crs::SHARPEN_EDGE_MASKING, &mut r.sharpening_masking, 0., 1.)?;
        settings.assign(crs::LUMINANCE_SMOOTHING, &mut r.noise_luma, 0., 1.)?;
        settings.assign(crs::COLOR_NOISE_REDUCTION, &mut r.noise_chroma, 0., 1.)?;
        // Lightroom's Denoise, from the Enhance filter (`filter_list`): AI Denoise at its
        // Amount. Without one, the photo's own setting stays.
        let key = super::filter_list::AI_DENOISE;
        settings.seen.insert(key.into());
        if let Some(amount) = number(settings.values, key)? {
            ensure!(
                (0. ..=100.).contains(&amount),
                "{key} is outside supported range"
            );
            r.ai_denoise = crate::model::ai_denoise::AiDenoise {
                enabled: true,
                amount: amount / 100.,
            };
        }
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
        if m.is_rendered() {
            return self.apply_relative_white_balance(settings, r, m, image);
        }
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

    /// A rendered file's white balance, as Lightroom keeps it: relative Temp and Tint
    /// (`IncrementalTemperature`, `IncrementalTint`). Kelvin values and the named
    /// modes are for RAW files and are ignored (see `ignored_white_balance`).
    fn apply_relative_white_balance(
        &self,
        settings: &mut Settings<'_>,
        r: &mut Recipe,
        m: &Metadata,
        image: Option<&dyn PhotoMeasures>,
    ) -> Result<()> {
        let v = settings.values;
        let keys = ["IncrementalTemperature", "IncrementalTint"];
        settings.seen.extend(keys.map(String::from));
        let given = keys.iter().any(|key| v.contains_key(*key));
        match v.get("WhiteBalance").map(String::as_str) {
            Some("As Shot") => r.reset_white_balance(m),
            // Photo settings carry the values Auto resolved to; a preset names the
            // mode alone and gets the WB menu's Auto.
            Some("Auto") if !(self.photo_settings && given) => {
                if let Some(photo) = image {
                    *r = photo.auto_white_balance(r)?;
                }
                return Ok(());
            }
            Some("Auto" | "Custom") | None => {}
            Some(name)
                if crate::model::white_balance::NamedWhiteBalance::from_name(name).is_some() => {}
            Some(other) => anyhow::bail!("Unsupported white balance mode: {other}"),
        }
        settings.assign(
            crs::INCREMENTAL_TEMPERATURE,
            &mut r.incremental_temperature,
            -100.,
            100.,
        )?;
        settings.assign(crs::INCREMENTAL_TINT, &mut r.incremental_tint, -100., 100.)?;
        if given {
            r.update_wb(m);
            r.auto_white_balance = None;
        }
        Ok(())
    }

    /// The note for white balance these settings carry that photo `m` ignores:
    /// Kelvin Temperature and Tint, and the named modes they stand for, apply to RAW
    /// files only, as in Lightroom.
    pub(crate) fn ignored_white_balance(&self, m: &Metadata) -> Option<String> {
        let v = &self.settings;
        let kelvin = v.contains_key("Temperature")
            || v.contains_key("Tint")
            || v.get("WhiteBalance").is_some_and(|name| {
                crate::model::white_balance::NamedWhiteBalance::from_name(name).is_some()
            });
        (m.is_rendered() && kelvin)
            .then(|| "Temperature in Kelvin applies to RAW files only".to_string())
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
        for i in 0..8 {
            for j in 0..3 {
                settings.assign(crs::HSL[i][j], &mut r.hsl[i][j], -1., 1.)?;
            }
            settings.assign(crs::GRAY_MIXER[i], &mut r.effects.gray_mix[i], -1., 1.)?;
        }
        for i in 0..3 {
            settings.assign(
                crs::CALIBRATION[i][0],
                &mut r.effects.calibration[i][0],
                -1.,
                1.,
            )?;
            settings.assign(
                crs::CALIBRATION[i][1],
                &mut r.effects.calibration[i][1],
                -1.,
                1.,
            )?;
        }
        settings.assign(crs::SHADOW_TINT, &mut r.effects.shadow_tint, -1., 1.)?;
        Ok(())
    }

    fn apply_curves(&self, settings: &mut Settings<'_>, r: &mut Recipe) -> Result<()> {
        let v = settings.values;
        for i in 0..4 {
            settings.assign(crs::PARAMETRIC[i], &mut r.effects.parametric[i], -1., 1.)?;
        }
        settings.assign(
            crs::CURVE_REFINE_SATURATION,
            &mut r.curve_saturation,
            0.,
            2.,
        )?;
        for i in 0..3 {
            settings.assign(
                crs::PARAMETRIC_SPLITS[i],
                &mut r.effects.splits[i],
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
        // The shadows are grading[0], the highlights grading[2].
        for (n, i) in [0, 2].into_iter().enumerate() {
            settings.assign(crs::SPLIT_TONING_HUE[n], &mut r.grading[i][0], 0., 1.)?;
            settings.assign(
                crs::SPLIT_TONING_SATURATION[n],
                &mut r.grading[i][1],
                0.,
                1.,
            )?;
            settings.assign(crs::COLOR_GRADE_LUM[n], &mut r.grading[i][2], -1., 1.)?;
        }
        settings.assign(crs::COLOR_GRADE_MIDTONE_HUE, &mut r.grading[1][0], 0., 1.)?;
        settings.assign(crs::COLOR_GRADE_MIDTONE_SAT, &mut r.grading[1][1], 0., 1.)?;
        settings.assign(crs::COLOR_GRADE_MIDTONE_LUM, &mut r.grading[1][2], -1., 1.)?;
        settings.assign(crs::SPLIT_TONING_BALANCE, &mut r.effects.balance, -1., 1.)?;
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
        settings.assign(crs::COLOR_GRADE_BLENDING, &mut r.effects.blending, 0., 1.)?;
        for i in 0..3 {
            settings.assign(
                crs::COLOR_GRADE_GLOBAL[i],
                &mut r.effects.global_grade[i],
                if i == 2 { -1. } else { 0. },
                1.,
            )?;
        }
        Ok(())
    }

    fn apply_effects(&self, settings: &mut Settings<'_>, r: &mut Recipe) -> Result<()> {
        let v = settings.values;
        settings.assign(crs::CLARITY, &mut r.effects.clarity, -1., 1.)?;
        settings.assign(crs::TEXTURE, &mut r.effects.texture, -1., 1.)?;
        settings.assign(crs::DEHAZE, &mut r.effects.dehaze, -1., 1.)?;
        settings.assign(crs::GRAIN_AMOUNT, &mut r.effects.grain, 0., 1.)?;
        settings.assign(crs::GRAIN_SIZE, &mut r.effects.grain_size, 0., 1.)?;
        settings.assign(crs::GRAIN_FREQUENCY, &mut r.effects.grain_roughness, 0., 1.)?;
        settings.seen.insert("GrainSeed".into());
        if let Some(seed) = v.get("GrainSeed") {
            r.effects.grain_seed = seed.parse().context("Invalid grain seed")?;
        }
        settings.assign(
            crs::POST_CROP_VIGNETTE_AMOUNT,
            &mut r.effects.vignette,
            -1.,
            1.,
        )?;
        settings.assign(
            crs::POST_CROP_VIGNETTE_MIDPOINT,
            &mut r.effects.vignette_midpoint,
            0.,
            1.,
        )?;
        settings.assign(
            crs::POST_CROP_VIGNETTE_ROUNDNESS,
            &mut r.effects.vignette_roundness,
            -1.,
            1.,
        )?;
        settings.assign(
            crs::POST_CROP_VIGNETTE_FEATHER,
            &mut r.effects.vignette_feather,
            0.,
            1.,
        )?;
        settings.assign(
            crs::POST_CROP_VIGNETTE_HIGHLIGHT_CONTRAST,
            &mut r.effects.vignette_highlights,
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
        settings.assign(crs::VIGNETTE_AMOUNT, &mut r.effects.lens_vignette, -1., 1.)?;
        settings.assign(
            crs::VIGNETTE_MIDPOINT,
            &mut r.effects.lens_vignette_midpoint,
            0.,
            1.,
        )?;
        for i in 0..2 {
            settings.assign(crs::DEFRINGE_AMOUNT[i], &mut r.effects.defringe[i], 0., 1.)?;
            settings.assign(
                crs::DEFRINGE_HUE_LO[i],
                &mut r.effects.defringe_ranges[i][0],
                0.,
                1.,
            )?;
            settings.assign(
                crs::DEFRINGE_HUE_HI[i],
                &mut r.effects.defringe_ranges[i][1],
                0.,
                1.,
            )?;
        }
        settings.assign(
            crs::LUMINANCE_NOISE_REDUCTION_DETAIL,
            &mut r.effects.luma_detail,
            0.,
            1.,
        )?;
        settings.assign(
            crs::LUMINANCE_NOISE_REDUCTION_CONTRAST,
            &mut r.effects.luma_contrast,
            0.,
            1.,
        )?;
        settings.assign(
            crs::COLOR_NOISE_REDUCTION_DETAIL,
            &mut r.effects.chroma_detail,
            0.,
            1.,
        )?;
        settings.assign(
            crs::COLOR_NOISE_REDUCTION_SMOOTHNESS,
            &mut r.effects.chroma_smoothness,
            0.,
            1.,
        )?;
        Ok(())
    }

    /// Lightroom's Auto Settings saved in settings (`AutoTone`). A preset saves the
    /// flag alone, and Lightroom's catalog [`UNMEASURED`] placeholders until it is
    /// measured: the engine's Auto then sets all eight sliders on this photo, over
    /// any tone value saved with it, as Lightroom's Auto does. Settings Lightroom
    /// resolved carry the values Auto chose (with an Exposure), which are kept, so
    /// every route (open, reset, history, hover) renders the same edit. Without the
    /// photo (raw defaults, a catalog record read before the photo opens) the
    /// sliders stay as they were.
    fn apply_auto_tone(
        &self,
        settings: &mut Settings<'_>,
        r: &mut Recipe,
        image: Option<&dyn PhotoMeasures>,
    ) -> Result<()> {
        settings.seen.insert("AutoTone".into());
        if self.auto_tone_pending()?
            && let Some(photo) = image
        {
            *r = photo.auto_tone(r)?;
        }
        Ok(())
    }
    /// Whether these settings ask for an Auto Settings Lightroom has not measured.
    fn auto_tone_pending(&self) -> Result<bool> {
        let v = &self.settings;
        Ok(boolean(v, "AutoTone")? == Some(true)
            && (!v.contains_key("Exposure2012") || unmeasured(v, "Exposure2012")?))
    }
    /// Whether applying these settings may measure the photo (Auto Settings, Auto
    /// white balance or Auto black & white), so a caller without it decoded yet
    /// knows to decode it first. Settings that can't be read count as measuring.
    pub fn measures_photo(&self) -> bool {
        let v = &self.settings;
        self.auto_tone_pending().unwrap_or(true)
            || boolean(v, "AutoGrayscaleMix").unwrap_or(Some(true)) == Some(true)
            || v.get("WhiteBalance").is_some_and(|wb| wb == "Auto")
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
    #[allow(clippy::too_many_lines)]
    fn apply_geometry(
        &self,
        settings: &mut Settings<'_>,
        r: &mut Recipe,
        m: &Metadata,
        lensfun: &Handle,
    ) -> Result<()> {
        let v = settings.values;
        settings.assign(crs::CROP_ANGLE, &mut r.straighten, -45., 45.)?;
        // An angle the settings give is theirs, not Auto's, even Auto's own number.
        if v.contains_key("CropAngle") {
            r.straighten_auto = None;
        }
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
        let id = id.map(crate::lens::choice::LensProfileRef::Adobe);
        if let Some(setup) = v.get("LensProfileSetup") {
            // A Setup names its profile, or none (a preset's "Default").
            r.lens_profile_choice = crate::lens::choice::LensProfileChoice {
                setup: crate::lens::choice::LensProfileSetup::from_xmp(setup.trim()),
                id,
            };
        } else if id.is_some() {
            r.lens_profile_choice.id = id;
        }
        // A Lensfun lens RAWmakase wrote, over the Setup it writes for Lightroom
        // ("Auto"): resolved against the installed database now, which is the edit;
        // without it, or without the lens, it stays named, unresolved.
        for key in super::LENSFUN_KEYS {
            settings.seen.insert(key.into());
        }
        if let Some(choice) = lensfun_choice(v)? {
            let choice = lensfun
                .get()
                .and_then(|installed| choice.resolved(&installed.db, m))
                .unwrap_or(choice);
            r.lens_profile_choice.choose_lensfun(choice);
        }
        settings.assign(
            crs::LENS_PROFILE_DISTORTION_SCALE,
            &mut r.lens_distortion,
            0.,
            2.,
        )?;
        settings.assign(
            crs::LENS_PROFILE_VIGNETTING_SCALE,
            &mut r.lens_vignetting,
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
            crs::LENS_MANUAL_DISTORTION_AMOUNT,
            &mut r.lens_manual_distortion,
            -1.,
            1.,
        )?;
        settings.seen.insert("AutoLateralCA".into());
        if let Some(ca) = number(v, "AutoLateralCA")? {
            r.lens_ca = ca != 0.;
        }
        let t = &mut r.transform;
        settings.assign(crs::PERSPECTIVE_VERTICAL, &mut t.vertical, -1., 1.)?;
        settings.assign(crs::PERSPECTIVE_HORIZONTAL, &mut t.horizontal, -1., 1.)?;
        settings.assign(crs::PERSPECTIVE_ROTATE, &mut t.rotate, -10., 10.)?;
        settings.assign(crs::PERSPECTIVE_ASPECT, &mut t.aspect, -1., 1.)?;
        settings.assign(crs::PERSPECTIVE_SCALE, &mut t.scale, 0.5, 1.5)?;
        settings.assign(crs::PERSPECTIVE_X, &mut t.offset_x, -1., 1.)?;
        settings.assign(crs::PERSPECTIVE_Y, &mut t.offset_y, -1., 1.)?;
        // Camera Raw crops only with HasCrop: without it the photo is uncropped, even
        // one the camera cropped to another aspect ratio, which Camera Raw records as
        // a crop. Without HasCrop or crop values the photo keeps its starting crop.
        if boolean(v, "HasCrop")? == Some(false) {
            r.crop = [0., 0., 1., 1.];
            for name in ["Left", "Top", "Right", "Bottom"] {
                settings.seen.insert(format!("Crop{name}"));
            }
        } else {
            // Lightroom records the crop on the frame before the camera orientation, as
            // the camera records its aspect-ratio crop; RAWmakase's is of the photo as
            // shown. Rotate and Flip stay out of XMP (docs/transform.md). Presets from
            // RAWmakase before #483 (marked 1) hold the crop as shown, and keep meaning it.
            let mut frame = crate::model::image_frame::ImageFrame::for_metadata(m);
            if v.get("RAWmakasePreset").is_some_and(|v| v == "1") {
                frame.turns = 0;
            }
            let start = frame.rect_to_unrotated(r.crop);
            let mut crop = start;
            for (edge, out) in crs::CROP.into_iter().zip(&mut crop) {
                settings.assign(edge, out, 0., 1.)?;
            }
            if crop != start {
                // Turning sorts the edges, so a reversed crop is refused before it is.
                ensure!(crop[0] < crop[2] && crop[1] < crop[3], "Invalid crop");
                r.crop = frame.rect_from_unrotated(crop);
            }
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
    /// High Dynamic Range's mode, limit and SDR rendition. Camera Raw 18.7 replaces a
    /// limit outside its range with its default and an SDR slider outside -100..+100
    /// with 0, rather than refusing them: they are read the same way. Values kept while
    /// HDR is off are read too, so switching HDR on restores them. HDR on is refused
    /// while it can't be rendered ([`HdrSettings::check_renderable`]).
    ///
    /// [`HdrSettings::check_renderable`]: crate::model::hdr::HdrSettings::check_renderable
    fn apply_hdr(&self, settings: &mut Settings<'_>, r: &mut Recipe) -> Result<()> {
        use crate::model::hdr::{DEFAULT_LIMIT, LIMIT_RANGE};
        let v = settings.values;
        let mode = "HDREditMode";
        settings.seen.insert(mode.into());
        if let Some(on) = number(v, mode)? {
            ensure!(on == 0. || on == 1., "Invalid {mode}");
            r.hdr.enabled = on == 1.;
        }
        settings.seen.insert(crs::HDR_MAX_VALUE.key.into());
        if let Some(limit) = number(v, crs::HDR_MAX_VALUE.key)? {
            r.hdr.limit = if LIMIT_RANGE.contains(&limit) {
                limit
            } else {
                DEFAULT_LIMIT
            };
        }
        for (slider, out) in crs::SDR_RENDITION.iter().zip(r.hdr.sdr.values_mut()) {
            settings.seen.insert(slider.key.into());
            if let Some(value) = number(v, slider.key)? {
                let value = value * slider.scale();
                *out = if (-1. ..=1.).contains(&value) {
                    value
                } else {
                    0.
                };
            }
        }
        r.hdr.check_renderable()
    }
    fn validate_remaining(&self, settings: &mut Settings<'_>, m: &Metadata) -> Result<()> {
        let v = settings.values;
        // No-op geometry/default flags are safe; active unsupported operations are explicit blockers.
        for (key, default) in [
            ("IncrementalTemperature", "0"),
            ("IncrementalTint", "0"),
            // Camera Raw 18.6 effects without rendering support yet.
            ("Glow", "0"),
            ("ReshapeAmount", "0"),
        ] {
            // A rendered file's relative white balance is applied with white balance.
            if m.is_rendered() && key.starts_with("Incremental") {
                continue;
            }
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
        // while Glow is 0 (an active Glow is refused above), and Distraction Removal's
        // switch changes nothing without removals, which are reported on their own.
        let at_rest = |key: &str| v.get(key).is_none_or(|value| value.parse() == Ok(0f32));
        let mut neutral = vec!["EnableDistractionRemoval"];
        if at_rest("Glow") {
            neutral.extend(["GlowRange", "GlowSpread", "GlowStyle", "GlowWarmth"]);
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

/// The Lensfun lens RAWmakase's own settings name (`LENSFUN_KEYS`), unresolved.
fn lensfun_choice(
    v: &BTreeMap<String, String>,
) -> Result<Option<crate::lens::lensfun::LensfunChoice>> {
    let text = |key: &str| v.get(key).map(|s| s.trim().to_string()).unwrap_or_default();
    let model = text("rawmakase:LensfunModel");
    if model.is_empty() {
        return Ok(None);
    }
    let number = |key: &str| -> Result<f32> {
        let n: f32 = text(key)
            .parse()
            .with_context(|| format!("Invalid {key}"))?;
        ensure!(n.is_finite() && n > 0., "Invalid {key}");
        Ok(n)
    };
    let lens = crate::lens::lensfun::LensfunLens {
        maker: text("rawmakase:LensfunMaker"),
        model,
        mounts: text("rawmakase:LensfunMounts")
            .split(';')
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty())
            .collect(),
        crop: number("rawmakase:LensfunCrop")?,
        aspect: number("rawmakase:LensfunAspect")?.max(1.),
    };
    let database = text("rawmakase:LensfunDatabase").parse().unwrap_or(0);
    Ok(Some(crate::lens::lensfun::LensfunChoice::unresolved(
        lens,
        text("rawmakase:LensfunName"),
        database,
    )))
}
