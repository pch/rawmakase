//! Writes a Lightroom Develop preset: the chosen setting groups of a recipe as an XMP
//! file Lightroom and Camera Raw read, laid out as the presets Lightroom writes.
use super::write::{curve, settings};
use crate::model::recipe::Recipe;
use crate::xml::{escape_text, ns::CRS, xmpmeta};
use crate::{
    color::curve::ToneCurve,
    model::settings_groups::{GroupInclusion, GroupSelection, SettingGroup},
};
use std::fmt::Write;

/// A preset's name, group and identity.
#[derive(Clone, Debug, PartialEq)]
pub struct PresetInfo {
    pub name: String,
    pub group: String,
    /// 32 hexadecimal digits, as Lightroom writes; kept when a preset is updated.
    pub uuid: String,
}

impl PresetInfo {
    /// A new preset with a fresh identity.
    pub fn new(name: &str, group: &str) -> Self {
        Self {
            name: name.trim().to_string(),
            group: group.trim().to_string(),
            uuid: new_uuid(),
        }
    }
}

/// 128 random bits as Lightroom's UUID text.
fn new_uuid() -> String {
    use std::hash::{BuildHasher, Hasher};
    let half = || {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        h.finish()
    };
    format!("{:016X}{:016X}", half(), half())
}

/// The preset for `r`'s settings in `groups`.
pub fn preset(r: &Recipe, info: &PresetInfo, groups: &GroupSelection) -> String {
    // A preset never carries one photo's Upright correction, so Upright Transforms
    // brings nothing, not even the Transform panel's switch.
    let mut groups = groups.clone();
    groups.set(SettingGroup::UprightTransforms, GroupInclusion::Excluded);
    let groups = &groups;
    // Lightroom's Amount slider, offered when every setting written scales with it.
    // Spots and masks are never written.
    let supports_amount = groups
        .groups()
        .filter(|g| !matches!(g, SettingGroup::SpotRemoval | SettingGroup::Masking))
        .all(SettingGroup::scales_with_amount);
    let supports_amount = if supports_amount { "True" } else { "False" };
    let mut attributes: Vec<(String, String)> = [
        ("PresetType", "Normal"),
        ("Cluster", ""),
        ("UUID", info.uuid.as_str()),
        ("SupportsAmount", supports_amount),
        ("SupportsColor", "True"),
        ("SupportsMonochrome", "True"),
        ("SupportsHighDynamicRange", "True"),
        ("SupportsNormalDynamicRange", "True"),
        ("SupportsSceneReferred", "True"),
        ("SupportsOutputReferred", "True"),
        ("CameraModelRestriction", ""),
        ("Copyright", ""),
        ("ContactInfo", ""),
        ("Version", "15.4"),
        ("RAWmakasePreset", "1"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    attributes.extend(
        settings(r, None)
            .0
            .into_iter()
            .filter(|(key, _)| group_of_key(key).is_some_and(|g| groups.contains(g)))
            // Panel switches are written below; Upright's corrections were analysed
            // from this photo, so a preset names only the mode, as Lightroom's do.
            .filter(|(key, _)| !key.starts_with("Enable") && !key.starts_with("Upright")),
    );
    // Some controls are written only while they take effect: the mix for black-and-
    // white photos, grain and vignette shapes while their amount isn't zero. A preset
    // still carries every control of its groups, so applying or later raising the
    // amount gives the source photo's look.
    let mut active = r.clone();
    active.effects.monochrome = true;
    if active.effects.grain == 0. {
        active.effects.grain = 1.;
    }
    if active.effects.vignette == 0. {
        active.effects.vignette = -1.;
    }
    let dormant: Vec<_> = settings(&active, None)
        .0
        .into_iter()
        .filter(|(key, _)| {
            key.starts_with("GrayMixer")
                || key.starts_with("Grain")
                || key.starts_with("PostCropVignette")
        })
        .filter(|(key, _)| group_of_key(key).is_some_and(|g| groups.contains(g)))
        .filter(|(key, _)| !attributes.iter().any(|(k, _)| k == key))
        .collect();
    attributes.extend(dormant);
    // Each chosen panel's switch as the photo has it, on or off, so applying the
    // preset also sets that panel the same way.
    for panel in crate::model::panels::Panel::ALL {
        if groups.groups().any(|g| g.panel() == Some(panel)) {
            let on = r.panels.state(panel) == crate::model::panels::PanelState::On;
            for key in panel.lightroom_keys() {
                attributes.push((key.to_string(), if on { "True" } else { "False" }.into()));
            }
        }
    }
    // An enhanced profile (Adobe Color, a creative look) is, as Lightroom writes it, a
    // Look over its base profile, which `settings` names.
    let look = super::write::look_element(r)
        .filter(|_| groups.contains(SettingGroup::TreatmentAndProfile));
    // Written once, after the settings, as Lightroom does.
    attributes.push(("HasSettings".into(), "True".into()));
    let mut out = format!("  <rdf:Description rdf:about=\"\"\n    xmlns:crs=\"{CRS}\"");
    for (key, value) in &attributes {
        let _ = write!(out, "\n   crs:{key}=\"{}\"", escape_text(value));
    }
    out.push_str(">\n");
    for (element, text) in [
        ("Name", info.name.as_str()),
        ("ShortName", ""),
        ("SortName", ""),
        ("Group", info.group.as_str()),
        ("Description", ""),
    ] {
        let _ = write!(
            out,
            "   <crs:{element}>\n    <rdf:Alt>\n     <rdf:li xml:lang=\"x-default\">{}</rdf:li>\n    </rdf:Alt>\n   </crs:{element}>\n",
            escape_text(text)
        );
    }
    out.extend(look);
    if groups.contains(SettingGroup::ToneCurve) {
        curve(&mut out, "ToneCurvePV2012", &r.curve);
        for (i, name) in ["Red", "Green", "Blue"].iter().enumerate() {
            curve(
                &mut out,
                &format!("ToneCurvePV2012{name}"),
                &r.effects.channels[i],
            );
        }
    }
    if groups.contains(SettingGroup::ColorAdjustments) {
        super::write::point_colors(&mut out, r, super::write::NoPointColors::EmptySelection);
    }
    out.push_str("  </rdf:Description>\n");
    xmpmeta(&out)
}

/// A saved point curve as Lightroom and Camera Raw keep one in their Curves folder:
/// the RGB curve and the Red, Green and Blue curves, named by the file.
pub fn point_curve(rgb: &ToneCurve, channels: &[ToneCurve; 3]) -> String {
    let mut out = format!(
        "  <rdf:Description rdf:about=\"\"\n    xmlns:crs=\"{CRS}\"\n   crs:Version=\"15.4\"\n   crs:ProcessVersion=\"11.0\"\n   crs:ToneCurveName2012=\"Custom\"\n   crs:HasSettings=\"True\">\n"
    );
    curve(&mut out, "ToneCurvePV2012", rgb);
    for (channel, name) in channels.iter().zip(["Red", "Green", "Blue"]) {
        curve(&mut out, &format!("ToneCurvePV2012{name}"), channel);
    }
    out.push_str("  </rdf:Description>\n");
    xmpmeta(&out)
}

/// The setting group a Camera Raw key belongs to, as Lightroom's New Develop Preset
/// dialog groups them.
pub(crate) fn group_of_key(key: &str) -> Option<SettingGroup> {
    use SettingGroup::*;
    let starts = |prefix: &str| key.starts_with(prefix);
    Some(match key {
        "WhiteBalance" | "Temperature" | "Tint" => WhiteBalance,
        "Exposure2012" => Exposure,
        "Contrast2012" => Contrast,
        "Highlights2012" => Highlights,
        "Shadows2012" => Shadows,
        "Whites2012" => Whites,
        "Blacks2012" => Blacks,
        "Texture" => Texture,
        "Clarity2012" => Clarity,
        "Dehaze" => Dehaze,
        "Vibrance" => Vibrance,
        "Saturation" => Saturation,
        "CameraProfile" | "ConvertToGrayscale" => TreatmentAndProfile,
        "ProcessVersion" => ProcessVersion,
        "Sharpness" | "EnableDetail" => Sharpening,
        "LuminanceSmoothing" => LuminanceNoiseReduction,
        "ColorNoiseReduction" => ColorNoiseReduction,
        "AutoLateralCA" => ChromaticAberration,
        "VignetteAmount" | "VignetteMidpoint" => LensVignetting,
        "PerspectiveUpright" => UprightMode,
        "EnableToneCurve" | "CurveRefineSaturation" => ToneCurve,
        "LensManualDistortionAmount" => LensProfileCorrections,
        "EnableColorAdjustments" | "PointColors" | "ColorVariance" => ColorAdjustments,
        "EnableGrayscaleMix" => BlackWhiteMix,
        "EnableSplitToning" => ColorGrading,
        "EnableLensCorrections" => LensProfileCorrections,
        "EnableTransform" => TransformAdjustments,
        "EnableEffects" => PostCropVignetting,
        "EnableCalibration" | "ShadowTint" => Calibration,
        "EnableRetouch" => SpotRemoval,
        "CropLeft"
        | "CropTop"
        | "CropRight"
        | "CropBottom"
        | "CropAngle"
        | "HasCrop"
        | "CropConstrainToWarp" => Crop,
        _ if starts("Parametric") || starts("ToneCurve") => ToneCurve,
        _ if starts("HueAdjustment")
            || starts("SaturationAdjustment")
            || starts("LuminanceAdjustment") =>
        {
            ColorAdjustments
        }
        _ if starts("GrayMixer") => BlackWhiteMix,
        _ if starts("SplitToning") || starts("ColorGrade") => ColorGrading,
        _ if starts("Sharpen") => Sharpening,
        _ if starts("LuminanceNoiseReduction") => LuminanceNoiseReduction,
        _ if starts("ColorNoiseReduction") => ColorNoiseReduction,
        _ if starts("LensProfile") => LensProfileCorrections,
        _ if starts("Defringe") => ChromaticAberration,
        _ if starts("Upright") => UprightMode,
        _ if starts("Perspective") => TransformAdjustments,
        _ if starts("PostCropVignette") => PostCropVignetting,
        _ if starts("Grain") => Grain,
        _ if matches!(
            key,
            "RedHue"
                | "RedSaturation"
                | "GreenHue"
                | "GreenSaturation"
                | "BlueHue"
                | "BlueSaturation"
        ) =>
        {
            Calibration
        }
        _ if key.starts_with("Enable") && key.ends_with("Corrections") => Masking,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::settings_groups::GroupInclusion;
    use std::path::Path;

    /// A recipe whose every written setting differs from the default.
    fn edited() -> Recipe {
        let mut r = Recipe {
            exposure: 0.5,
            contrast: 0.2,
            temperature: 4800.,
            sharpening: 0.5,
            ..Default::default()
        };
        r.effects.monochrome = true;
        r.effects.vignette = -0.3;
        r.effects.grain = 0.2;
        r.curve.points = vec![[0., 0.1], [1., 1.]];
        r.panels.set(
            crate::model::panels::Panel::Effects,
            crate::model::panels::PanelState::Off,
        );
        r
    }

    #[test]
    fn a_looks_profile_amount_is_written_and_read_back() -> anyhow::Result<()> {
        use crate::camera_profiles::CameraProfile;
        let m = crate::camera_data::Metadata {
            make: "Test".into(),
            model: "Camera".into(),
            cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
            ..Default::default()
        };
        let profiles: Vec<_> = [
            crate::camera_profiles::open::standard(&m),
            Some(CameraProfile::creative_for_test(&m)),
        ]
        .into_iter()
        .flatten()
        .map(std::sync::Arc::new)
        .collect();
        let r = Recipe {
            profile: Some(profiles[1].clone()),
            profile_amount: 0.35,
            ..Default::default()
        };
        let text = preset(
            &r,
            &PresetInfo::new("Faded", "User Presets"),
            &GroupSelection::all(),
        );
        assert!(text.contains(r#"crs:Amount="0.35""#), "{text}");
        let back = crate::xmp::parse(Path::new("faded.xmp"), &text)?.apply(
            &Recipe::with_profiles(&m, &profiles),
            &m,
            &profiles,
            None,
        )?;
        assert_eq!(back.profile.as_ref().unwrap().name, "Test Creative");
        assert_eq!(back.profile_amount, 0.35);
        Ok(())
    }
    /// A preset offers Lightroom's Amount only when every setting it holds scales.
    #[test]
    fn presets_offer_an_amount_when_every_group_scales() -> anyhow::Result<()> {
        let info = PresetInfo::new("Soft", "User Presets");
        let mut tones = GroupSelection::none();
        for group in [
            SettingGroup::Exposure,
            SettingGroup::ToneCurve,
            SettingGroup::WhiteBalance,
            SettingGroup::TreatmentAndProfile,
            // Never written, so they don't count.
            SettingGroup::SpotRemoval,
            SettingGroup::Masking,
        ] {
            tones.set(group, GroupInclusion::Included);
        }
        let text = preset(&edited(), &info, &tones);
        assert!(text.contains(r#"crs:SupportsAmount="True""#), "{text}");
        let parsed = crate::xmp::parse(Path::new("Soft.xmp"), &text)?;
        let m = crate::camera_data::Metadata {
            wb: [2., 1., 1.8],
            daylight_wb: [2., 1., 1.8],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        };
        let before = Recipe::default();
        let full = parsed.apply(&before, &m, &[], None)?;
        let amount = crate::presets::amount::PresetAmount::new(&parsed, before, full)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        assert_eq!(amount.at(0.5, &m).exposure, 0.25);
        // Crop or lens corrections don't scale.
        for group in [SettingGroup::Crop, SettingGroup::LensProfileCorrections] {
            let mut with = tones.clone();
            with.set(group, GroupInclusion::Included);
            let text = preset(&edited(), &info, &with);
            assert!(text.contains(r#"crs:SupportsAmount="False""#), "{group:?}");
        }
        Ok(())
    }
    #[test]
    fn every_key_written_belongs_to_a_group() {
        let unplaced: Vec<_> = settings(&edited(), None)
            .0
            .into_iter()
            .map(|(k, _)| k)
            .filter(|k| k != "HasSettings" && group_of_key(k).is_none())
            .collect();
        assert!(unplaced.is_empty(), "{unplaced:?}");
    }

    #[test]
    fn color_presets_carry_point_colors_and_clear_them_when_empty() -> anyhow::Result<()> {
        let mut color = GroupSelection::none();
        color.set(SettingGroup::ColorAdjustments, GroupInclusion::Included);
        let info = PresetInfo::new("Warm skin", "User Presets");
        use crate::model::point_color::PointColor;
        let swatch = PointColor {
            shift: [0.1, -0.2, 0.15],
            variance: -0.3,
            ..PointColor::sampled([0.55, 0.39, 0.67])
        };
        let r = Recipe {
            point_colors: vec![swatch],
            ..Default::default()
        };
        let target = Recipe {
            point_colors: vec![PointColor::sampled([3., 0.5, 0.5])],
            ..Default::default()
        };
        let m = crate::camera_data::Metadata::default();
        let applied = crate::xmp::parse(Path::new("Warm.xmp"), &preset(&r, &info, &color))?.apply(
            &target,
            &m,
            &[],
            None,
        )?;
        assert_eq!(applied.point_colors.len(), 1);
        assert_eq!(applied.point_colors[0].shift, swatch.shift);
        assert_eq!(applied.point_colors[0].variance, swatch.variance);
        // Without swatches, Lightroom's empty selection clears the target's.
        let text = preset(&Recipe::default(), &info, &color);
        assert!(text.contains("<rdf:li>-1.000000, -1.000000"), "{text}");
        let cleared =
            crate::xmp::parse(Path::new("Clear.xmp"), &text)?.apply(&target, &m, &[], None)?;
        assert!(cleared.point_colors.is_empty());
        // Other groups leave them alone.
        let mut exposure = GroupSelection::none();
        exposure.set(SettingGroup::Exposure, GroupInclusion::Included);
        assert!(!preset(&r, &info, &exposure).contains("PointColors"));
        Ok(())
    }

    #[test]
    fn a_preset_holds_only_its_groups_and_reads_back_as_written() -> anyhow::Result<()> {
        let mut groups = GroupSelection::none();
        groups.set(SettingGroup::Exposure, GroupInclusion::Included);
        groups.set(SettingGroup::ToneCurve, GroupInclusion::Included);
        let info = PresetInfo::new("Bright & airy", "User Presets");
        assert_eq!(info.uuid.len(), 32);
        let text = preset(&edited(), &info, &groups);
        let parsed = crate::xmp::parse(Path::new("Bright.xmp"), &text)?;
        assert_eq!(parsed.name, "Bright & airy");
        assert_eq!(parsed.group, "User Presets");
        assert_eq!(parsed.settings["UUID"], info.uuid);
        assert!(parsed.settings.contains_key("Exposure2012"));
        for key in [
            "Contrast2012",
            "Temperature",
            "ConvertToGrayscale",
            "Sharpness",
        ] {
            assert!(!parsed.settings.contains_key(key), "{key}");
        }
        let m = crate::camera_data::Metadata {
            wb: [2., 1., 1.8],
            daylight_wb: [2., 1., 1.8],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        };
        let applied = parsed.apply(&Recipe::default(), &m, &[], None)?;
        assert_eq!(applied.exposure, 0.5);
        assert_eq!(applied.contrast, 0.);
        assert_eq!(applied.curve.points.len(), 2);
        // Every group: the whole edit comes back.
        let all = crate::xmp::parse(
            Path::new("All.xmp"),
            &preset(&edited(), &info, &GroupSelection::all()),
        )?
        .apply(&Recipe::default(), &m, &[], None)?;
        assert!(all.effects.monochrome);
        // A chosen panel's switch is written either way, so applying turns it on.
        let mut grain = GroupSelection::none();
        grain.set(SettingGroup::Grain, GroupInclusion::Included);
        let text = preset(&Recipe::default(), &info, &grain);
        assert!(text.contains(r#"crs:EnableEffects="True""#), "{text}");
        assert!(!text.contains("crs:EnableDetail"));
        // As the photo has it: an Effects panel switched off stays off.
        let mut off = Recipe::default();
        off.panels.set(
            crate::model::panels::Panel::Effects,
            crate::model::panels::PanelState::Off,
        );
        let text = preset(&off, &info, &grain);
        assert!(text.contains(r#"crs:EnableEffects="False""#), "{text}");
        // Upright travels as its mode only, never this photo's corrections.
        let mut upright = Recipe::default();
        upright.upright.mode = crate::model::transform::UprightMode::Level;
        upright.upright.corrections = vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 4];
        let text = preset(&upright, &info, &GroupSelection::all());
        assert!(text.contains("crs:PerspectiveUpright"));
        assert!(!text.contains("UprightTransform"), "{text}");
        // Upright Transforms alone, left over from a Copy selection, brings nothing.
        let mut transforms = GroupSelection::none();
        transforms.set(SettingGroup::UprightTransforms, GroupInclusion::Included);
        let text = preset(&upright, &info, &transforms);
        assert!(
            !text.contains("Transform") && !text.contains("Upright"),
            "{text}"
        );
        // A color photo's Black & White mix is still written when the group is chosen.
        let mut mix = GroupSelection::none();
        mix.set(SettingGroup::BlackWhiteMix, GroupInclusion::Included);
        let mut color = Recipe::default();
        color.effects.gray_mix[0] = 0.3;
        let text = preset(&color, &info, &mix);
        assert!(text.contains(r#"crs:GrayMixerRed="+30""#), "{text}");
        assert!(!text.contains("ConvertToGrayscale"), "{text}");
        // Grain and vignette shapes travel with their groups at a zero amount.
        let mut effects = GroupSelection::none();
        effects.set(SettingGroup::Grain, GroupInclusion::Included);
        effects.set(SettingGroup::PostCropVignetting, GroupInclusion::Included);
        let mut quiet = Recipe::default();
        quiet.effects.grain_size = 0.6;
        quiet.effects.vignette_roundness = 0.4;
        let text = preset(&quiet, &info, &effects);
        assert!(text.contains(r#"crs:GrainAmount="0""#), "{text}");
        assert!(text.contains(r#"crs:GrainSize="60""#), "{text}");
        assert!(text.contains(r#"crs:PostCropVignetteAmount="0""#), "{text}");
        assert!(
            text.contains(r#"crs:PostCropVignetteRoundness="+40""#),
            "{text}"
        );
        assert_eq!(
            all.panels.state(crate::model::panels::Panel::Effects),
            crate::model::panels::PanelState::Off
        );
        Ok(())
    }
}
