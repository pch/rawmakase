//! Presets shipped with RAWmakase, embedded in the binary from `assets/presets`.
//! They are ordinary Lightroom XMP presets, so they also work in Lightroom.
use crate::xmp::{Preset, parse};
use anyhow::{Context, Result};
use std::path::Path;

/// Built-in groups in the order the Presets panel lists them, before imported ones.
pub const GROUPS: &[&str] = &[
    "Color",
    "Creative",
    "Film",
    "B&W",
    "Curve",
    "Grain",
    "Vignetting",
];

macro_rules! files {
    ($($path:literal),* $(,)?) => {
        &[$(($path, include_str!(concat!("../../../../assets/presets/", $path)))),*]
    };
}
/// Every file in `assets/presets`, in panel order within each group.
pub(super) const FILES: &[(&str, &str)] = files![
    "Color/Warm.xmp",
    "Color/Cool.xmp",
    "Color/Punch.xmp",
    "Color/Muted.xmp",
    "Color/Matte.xmp",
    "Creative/Concrete Haze.xmp",
    "Creative/Faded Gold.xmp",
    "Film/Portra-ish 400.xmp",
    "Film/Portra-ish 400 Print.xmp",
    "Film/Kodachrome-ish 64.xmp",
    "Film/Kodachrome-ish 64 Print.xmp",
    "BandW/Neutral.xmp",
    "BandW/High Contrast.xmp",
    "BandW/Soft.xmp",
    "BandW/Red Filter.xmp",
    "BandW/Green Filter.xmp",
    "BandW/Sepia.xmp",
    "Curve/Linear.xmp",
    "Curve/Medium Contrast.xmp",
    "Curve/Strong Contrast.xmp",
    "Curve/Lighten.xmp",
    "Curve/Darken.xmp",
    "Curve/Flat.xmp",
    "Curve/Matte.xmp",
    "Grain/Light.xmp",
    "Grain/Medium.xmp",
    "Grain/Heavy.xmp",
    "Vignetting/Subtle.xmp",
    "Vignetting/Strong.xmp",
    "Vignetting/Light.xmp",
];

fn load(path: &str, text: &str) -> Result<Preset> {
    let mut preset = parse(&Path::new("builtin").join(path), text)?;
    let uuid = preset
        .settings
        .get("UUID")
        .filter(|u| !u.is_empty())
        .context("Built-in preset without a UUID")?;
    // Favorites follow the UUID, so a preset can be renamed or regrouped.
    preset.id = format!("builtin:{uuid}");
    preset.builtin = true;
    Ok(preset)
}

/// The built-in presets, grouped in `GROUPS` order. A file that fails to parse is a
/// bug caught by the tests; at run time it is reported like an imported one.
pub fn presets() -> (Vec<Preset>, Vec<String>) {
    let mut presets = Vec::new();
    let mut errors = Vec::new();
    for (path, text) in FILES {
        match load(path, text) {
            Ok(p) => presets.push(p),
            Err(e) => errors.push(format!("Built-in {path}: {e:#}")),
        }
    }
    presets.sort_by_key(|p| group_rank(&p.group));
    (presets, errors)
}

/// Where a built-in group sorts in the panel.
pub fn group_rank(group: &str) -> usize {
    GROUPS
        .iter()
        .position(|g| *g == group)
        .unwrap_or(GROUPS.len())
}
