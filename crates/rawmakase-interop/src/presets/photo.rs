//! A Develop preset applied to one photo: what a click in Develop's Presets panel
//! gives the open photo and what a batch gives each selected photo, worked out by
//! one function so both make the same edit.
use crate::{
    camera_data::Metadata, camera_profiles::CameraProfile, model::recipe::Recipe,
    xmp::PhotoMeasures,
};
use anyhow::Result;
use std::sync::Arc;

/// A preset applied to a photo's settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Applied {
    pub recipe: Recipe,
    /// The settings that do not fit this photo and were left as they were.
    pub skipped: Vec<String>,
    /// The profile used in place of one the preset names but this camera lacks.
    pub substitute: Option<String>,
}

/// `preset` applied at 100% to `current`, a photo's settings, as Lightroom applies
/// one: everything that fits this photo, the rest reported (see
/// `Preset::apply_lenient`). Its Auto settings are measured on `measures`, the
/// decoded photo; without it they leave the settings as they are. Upright keeps this
/// photo's own analysis, not one the preset carries from another photo. The edit
/// records the preset as the last one applied ([`Recipe::applied_preset`]).
pub fn apply_to_photo(
    preset: &crate::xmp::Preset,
    current: &Recipe,
    m: &Metadata,
    profiles: &[Arc<CameraProfile>],
    measures: Option<&dyn PhotoMeasures>,
) -> Result<Applied> {
    let (mut recipe, skipped) = preset.apply_lenient(current, m, profiles, measures)?;
    let substitute = preset.profile_substitute(m, profiles).map(|(_, used)| used);
    this_photos_upright(&mut recipe, current);
    // The last preset applied, as Lightroom records it in `crs:Preset`.
    recipe.applied_preset = Some(crate::model::recipe::AppliedPreset {
        name: preset.name.clone(),
        amount: Some(1.),
        uuid: preset.settings.get("UUID").cloned().unwrap_or_default(),
    });
    Ok(Applied {
        recipe,
        skipped,
        substitute,
    })
}

/// A preset's Upright mode, with this photo's own corrections rather than any the preset
/// carries from the photo it was saved from; a mode without one is analysed on apply.
/// New lens settings call for a new analysis, as pasting them does.
fn this_photos_upright(r: &mut Recipe, current: &Recipe) {
    if r.upright != current.upright {
        let mode = r.upright.mode;
        r.upright.clone_from(&current.upright);
        r.upright.mode = mode;
        if mode == crate::model::transform::UprightMode::Guided && r.upright.correction().is_none()
        {
            r.upright.mode = current.upright.mode;
        }
    }
    use crate::model::transform::LensInputs;
    if LensInputs::of(r) != LensInputs::of(current) {
        r.upright.analyse_again();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A preset that changes only a lens setting leaves no Upright correction analysed
    /// through the old one: the editor analyses the photo again.
    #[test]
    fn presets_with_new_lens_settings_drop_the_photos_upright_analysis() {
        let mut current = Recipe::default();
        current.upright.mode = crate::model::transform::UprightMode::Level;
        current.upright.corrections = vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 4];
        let mut same = current.clone();
        same.exposure = 1.;
        this_photos_upright(&mut same, &current);
        assert_eq!(same.upright, current.upright);
        let mut lens = current.clone();
        lens.lens_manual_distortion = 0.3;
        this_photos_upright(&mut lens, &current);
        assert!(lens.upright.corrections.is_empty());
        assert_eq!(
            lens.upright.mode,
            crate::model::transform::UprightMode::Level
        );
    }

    /// The edit records the preset chosen, at 100%, with its UUID when it has one,
    /// replacing any earlier record: the last preset applied, as Lightroom keeps it.
    #[test]
    fn applying_a_preset_records_its_name() {
        let text = format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><r:RDF xmlns:r="{}"><r:Description xmlns:c="{}" c:PresetType="Normal" c:UUID="F00D" c:Exposure2012="0.3"/></r:RDF></x:xmpmeta>"#,
            crate::xml::ns::RDF,
            crate::xml::ns::CRS
        );
        let preset = crate::xmp::parse(std::path::Path::new("Warm.xmp"), &text).unwrap();
        let current = Recipe {
            applied_preset: Some(crate::model::recipe::AppliedPreset {
                name: "Earlier".into(),
                amount: Some(0.4),
                uuid: String::new(),
            }),
            ..Recipe::default()
        };
        let applied = apply_to_photo(&preset, &current, &Metadata::default(), &[], None).unwrap();
        assert_eq!(
            applied.recipe.applied_preset,
            Some(crate::model::recipe::AppliedPreset {
                name: "Warm".into(),
                amount: Some(1.),
                uuid: "F00D".into(),
            })
        );
    }
}
