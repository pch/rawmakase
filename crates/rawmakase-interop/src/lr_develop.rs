//! Lightroom's Develop settings, as a catalog stores them (a serialized Lua
//! table), translated into a recipe through the XMP settings they mirror. The
//! table is parsed as data; no interpreter runs. Used to show and resolve photos
//! imported from Lightroom; reading the Lightroom catalog itself is
//! `catalog::lightroom`.
use crate::model::recipe::Recipe;
use crate::xmp::look::LookAmount;
use anyhow::{Context, Result, ensure};
use std::path::PathBuf;
/// A look's own `ConvertToGrayscale`, as [`crate::xmp::look::GRAYSCALE`] holds it, from
/// the settings the catalog stores with the look (`Look = { Name = …, Parameters = { … } }`).
/// `None` when the look carries no settings, or settings that can't be read: the
/// photo's own flag then decides, as before the look's was read.
fn look_grayscale(look: &std::collections::BTreeMap<String, String>) -> Option<&'static str> {
    let parameters = develop_fields(&format!("s = {}", look.get("Parameters")?)).ok()?;
    Some(crate::xmp::look::grayscale(
        parameters.get("ConvertToGrayscale").map(String::as_str),
    ))
}

/// Whether a Lightroom edit's Treatment is Black & White, from its parsed
/// [`develop_fields`]: the look's own `ConvertToGrayscale` when a named look carries
/// its settings (as Camera Raw takes it, #666), else the edit's. Without the photo's
/// profiles at hand it can't tell whether the look is available, which
/// [`convert_develop`] also checks.
pub fn grayscale(fields: &std::collections::BTreeMap<String, String>) -> bool {
    let look = fields
        .get("Look")
        .filter(|v| v.starts_with('{'))
        .and_then(|v| develop_fields(&format!("s = {v}")).ok())
        .filter(|look| look.contains_key("Name"));
    match look.as_ref().and_then(look_grayscale) {
        Some(flag) => flag == "True",
        None => fields
            .get("ConvertToGrayscale")
            .is_some_and(|v| v.trim() == "true"),
    }
}

/// Parse Lightroom's serialized Lua table as data only. No interpreter is used.
pub fn develop_fields(text: &str) -> Result<std::collections::BTreeMap<String, String>> {
    ensure!(text.len() < 16_000_000, "Develop settings too large");
    let text = text.trim();
    let text = text
        .strip_prefix("s")
        .context("Expected Lightroom settings table")?
        .trim_start()
        .strip_prefix('=')
        .context("Expected settings assignment")?
        .trim_start();
    ensure!(
        text.starts_with('{') && text.ends_with('}'),
        "Malformed settings table"
    );
    let body = &text[1..text.len() - 1];
    let mut fields = std::collections::BTreeMap::new();
    let mut start = 0;
    let mut depth = 0i32;
    let mut quote = None;
    let mut escape = false;
    let mut insert = |part: &str| -> Result<()> {
        let part = part.trim();
        if part.is_empty() {
            return Ok(());
        }
        let (key, value) = part.split_once('=').context("Malformed Develop field")?;
        let key = key.trim();
        ensure!(
            !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "Invalid Develop key"
        );
        ensure!(
            fields.insert(key.into(), value.trim().into()).is_none(),
            "Duplicate Develop field {key}"
        );
        Ok(())
    };
    for (i, c) in body.char_indices() {
        if let Some(q) = quote {
            if escape {
                escape = false
            } else if c == '\\' {
                escape = true
            } else if c == q {
                quote = None
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '{' => {
                depth += 1;
                ensure!(depth < 128, "Develop nesting too deep");
            }
            '}' => {
                depth -= 1;
                ensure!(depth >= 0, "Unbalanced Develop table");
            }
            ',' if depth == 0 => {
                insert(&body[start..i])?;
                start = i + 1;
            }
            _ => {}
        }
    }
    ensure!(quote.is_none() && depth == 0, "Unterminated Develop value");
    insert(&body[start..])?;
    Ok(fields)
}
pub fn convert_develop(
    text: &str,
    m: &crate::camera_data::Metadata,
    profiles: &[std::sync::Arc<crate::camera_profiles::CameraProfile>],
    image: Option<&dyn crate::xmp::PhotoMeasures>,
) -> Result<(Recipe, Vec<String>)> {
    let fields = develop_fields(text)?;
    let mut preset = crate::xmp::Preset {
        photo_settings: true,
        id: String::new(),
        name: "Imported Lightroom edit".into(),
        group: String::new(),
        path: PathBuf::new(),
        settings: Default::default(),
        curves: Default::default(),
        look: String::new(),
        blockers: Vec::new(),
        notes: Vec::new(),
        local: Default::default(),
        builtin: false,
    };
    let mut warnings = Vec::new();
    // A record from before process version 2012 (6.7). Lightroom 5 stores the 2012 tone
    // keys in these too, all 0, and ignores them (#1060).
    let pv = fields.get("ProcessVersion").map(|v| v.trim_matches('"'));
    let legacy = pv.is_some_and(crate::xmp::before_process_2012);
    // Only real 2003 records (below 5.7); the Clarity scale was fitted on those.
    let pv2003 = pv.is_some_and(|v| v.parse::<f32>().is_ok_and(|v| v < 5.7));
    let mut old = std::collections::BTreeMap::new();
    for (key, value) in &fields {
        if value.starts_with('{') {
            if key == "Look" && !value[1..value.len() - 1].trim().is_empty() {
                let look = develop_fields(&format!("s = {value}"))?;
                if let Some(name) = look.get("Name") {
                    preset.look = serde_json::from_str(name)?;
                }
                if let Some(uuid) = look.get("UUID") {
                    preset
                        .settings
                        .insert("RAWmakaseLookUUID".into(), serde_json::from_str(uuid)?);
                }
                if let Some(flag) = look_grayscale(&look) {
                    preset
                        .settings
                        .insert(crate::xmp::look::GRAYSCALE.into(), flag.into());
                }
                // An amount Lightroom can't store is reported, and the look renders
                // at 100%.
                match look.get("Amount").map(|a| LookAmount::parse(a)) {
                    Some(Ok(amount)) => {
                        preset
                            .settings
                            .insert(crate::xmp::look::SETTING.into(), amount.0.to_string());
                    }
                    Some(Err(e)) => warnings.push(format!("{e:#}; rendered at 100%")),
                    None => {}
                }
            } else if crate::xmp::local::KEYS.contains(&key.as_str()) {
                match crate::xmp::local::Node::from_lua(value) {
                    Ok(node) if !node.is_empty() => {
                        preset.local.insert(key.clone(), node);
                    }
                    Ok(_) => {}
                    Err(e) => warnings.push(format!("{key}: {e:#}")),
                }
            } else if matches!(key.as_str(), "PointColors" | "ColorVariance") {
                // One quoted string of 19 values per swatch (as in XMP), or bare numbers.
                let items =
                    lua_list_items(value).with_context(|| format!("Unsupported {key} encoding"))?;
                let items = if key == "PointColors" && items.iter().all(|i| !i.contains(',')) {
                    items.chunks(19).map(|c| c.join(", ")).collect()
                } else {
                    items
                };
                preset.settings.insert(
                    key.clone(),
                    items.join(crate::model::point_color::LIST_SEPARATOR),
                );
            } else if key.starts_with("ToneCurve") && !key.contains("Name") {
                let values = value
                    .trim_matches(['{', '}'])
                    .split(',')
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| s.trim().parse::<f32>())
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                ensure!(values.len() % 2 == 0 && values.len() >= 4, "Invalid {key}");
                let points: Vec<_> = values
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|p| [p[0] / 255., p[1] / 255.])
                    .collect();
                let curve = crate::color::curve::ToneCurve {
                    points,
                    ..Default::default()
                };
                curve.validate()?;
                preset.curves.insert(key.clone(), curve);
            } else if !value[1..value.len() - 1].trim().is_empty() {
                warnings.push(key.clone())
            }
        } else {
            let value = if value.starts_with('"') {
                serde_json::from_str::<String>(value)
                    .with_context(|| format!("Unsupported string encoding in {key}"))?
            } else {
                ensure!(
                    matches!(value.as_str(), "true" | "false") || value.parse::<f64>().is_ok(),
                    "Unsupported literal for {key}"
                );
                value.clone()
            };
            // Process version 2003/2010 basic controls. Newer records keep them next to
            // their 2012 counterparts; never apply both generations. Older records have
            // only these, and they convert after this loop.
            if legacy
                && matches!(
                    key.as_str(),
                    "Exposure2012"
                        | "Contrast2012"
                        | "Highlights2012"
                        | "Shadows2012"
                        | "Whites2012"
                        | "Blacks2012"
                        | "Clarity2012"
                )
            {
                continue;
            }
            if matches!(
                key.as_str(),
                "Exposure"
                    | "Contrast"
                    | "Brightness"
                    | "Shadows"
                    | "Clarity"
                    | "FillLight"
                    | "HighlightRecovery"
            ) {
                if legacy {
                    match value.parse::<f64>() {
                        Ok(v) => _ = old.insert(key.clone(), v),
                        Err(_) => warnings.push(format!("{key}: {value} is not a number")),
                    }
                }
                continue;
            }
            if matches!(
                key.as_str(),
                "CustomTemperature"
                    | "CustomTint"
                    | "CropConstrainAspectRatio"
                    | "OverrideLookVignette"
            ) {
                continue;
            }
            preset.settings.insert(key.clone(), value);
        }
    }
    if legacy {
        // Any Exposure2012, even 0, means Auto Tone already ran, so without an Exposure in
        // the record an Auto Tone record gets none.
        let auto = crate::xmp::boolean(&preset.settings, "AutoTone")
            .ok()
            .flatten()
            == Some(true);
        for (key, value) in legacy_tone(&old, pv2003) {
            let write = if key == "Exposure2012" {
                old.contains_key("Exposure") || (!auto && value != 0.)
            } else {
                value != 0.
            };
            if write {
                preset.settings.insert(key.into(), value.to_string());
            }
        }
    }
    // The catalog leaves out corners at their defaults (Lightroom 5 writes no CropLeft or
    // CropRight for a crop that keeps the full width, #1059). The angle turns the crop only
    // with all four corners, as in Camera Raw's sidecars, so the missing ones are written out.
    const CORNERS: [(&str, &str); 4] = [
        ("CropLeft", "0"),
        ("CropTop", "0"),
        ("CropRight", "1"),
        ("CropBottom", "1"),
    ];
    if CORNERS
        .iter()
        .map(|(key, _)| *key)
        .chain(["CropAngle"])
        .any(|key| preset.settings.contains_key(key))
    {
        for (key, default) in CORNERS {
            preset
                .settings
                .entry(key.into())
                .or_insert_with(|| default.into());
        }
    }
    // A look that isn't available is reported on its own, so the base profile's
    // fallback and every other setting still apply.
    if !preset.look_available(m, profiles) {
        warnings.push(format!(
            "Missing or unsupported enhanced profile ‘{}’ for {}",
            preset.look, m.model
        ));
        preset.look.clear();
        preset.settings.remove("RAWmakaseLookUUID");
        preset.settings.remove(crate::xmp::look::SETTING);
        preset.settings.remove(crate::xmp::look::GRAYSCALE);
    }
    // Spots and masks convert on their own and report what they skip.
    let local_settings = std::mem::take(&mut preset.local);
    let base = Recipe::with_profiles(m, profiles);
    let (mut recipe, substitute) = match can_try_whole_preset_fast_path(&preset, &base, image)
        .then(|| preset.apply(&base, m, profiles, image))
        .and_then(Result::ok)
    {
        Some(recipe) => (recipe, preset.profile_substitute(m, profiles)),
        None => {
            // Build a compatible patch before application. Related fields are validated together.
            let mut accepted = preset.clone();
            accepted.settings.clear();
            let mut grouped = std::collections::BTreeSet::new();
            // Upright's mode and the corrections Lightroom stored for it.
            let upright: Vec<&str> = std::iter::once("PerspectiveUpright")
                .chain(
                    preset
                        .settings
                        .keys()
                        .map(String::as_str)
                        .filter(|k| k.starts_with("Upright")),
                )
                .collect();
            // Auto black & white is judged with the mix Lightroom resolved for it; without
            // Auto, each mixer channel stands on its own.
            let auto_gray_mix = preset
                .settings
                .get("AutoGrayscaleMix")
                .is_some_and(|v| v == "true");
            let gray_mix: Vec<&str> = if auto_gray_mix {
                preset
                    .settings
                    .keys()
                    .map(String::as_str)
                    .filter(|k| *k == "AutoGrayscaleMix" || k.starts_with("GrayMixer"))
                    .collect()
            } else {
                Vec::new()
            };
            for keys in [
                vec!["WhiteBalance", "Temperature", "Tint"],
                vec![
                    "CropLeft",
                    "CropTop",
                    "CropRight",
                    "CropBottom",
                    "CropAngle",
                ],
                vec![
                    "ParametricShadowSplit",
                    "ParametricMidtoneSplit",
                    "ParametricHighlightSplit",
                ],
                upright,
                gray_mix,
                vec!["PointColors", "ColorVariance"],
            ] {
                let mut p = preset.clone();
                p.settings.retain(|k, _| keys.contains(&k.as_str()));
                for k in &keys {
                    grouped.insert(k.to_string());
                }
                match p.apply(&base, m, profiles, image) {
                    Ok(_) => accepted.settings.extend(p.settings),
                    Err(e) => warnings.push(e.to_string()),
                }
            }
            for (key, value) in &preset.settings {
                if grouped.contains(key) {
                    continue;
                }
                let mut p = preset.clone();
                p.settings.clear();
                p.settings.insert(key.clone(), value.clone());
                // A tone slider is read with Auto, whose placeholder it may hold.
                if crate::xmp::AUTO_TONE_KEYS.contains(&key.as_str())
                    && let Some(auto) = preset.settings.get("AutoTone")
                {
                    p.settings.insert("AutoTone".into(), auto.clone());
                }
                match p.apply(&base, m, profiles, image) {
                    Ok(_) => {
                        accepted.settings.insert(key.clone(), value.clone());
                    }
                    Err(e) => warnings.push(e.to_string()),
                }
            }
            (
                accepted.apply(&base, m, profiles, image)?,
                accepted.profile_substitute(m, profiles),
            )
        }
    };
    if let Some((asked, used)) = substitute {
        warnings.push(format!("{asked} isn't imported; rendered with {used}"));
    }
    warnings.extend(recipe.missing_lens_profile(m));
    warnings.extend(preset.ignored_white_balance(m));
    let local = crate::xmp::local::convert(
        &local_settings,
        crate::model::image_frame::ImageFrame::for_metadata(m),
    );
    if let Some(retouch) = local.retouch {
        recipe.retouch = retouch;
    }
    if let Some(red_eye) = local.red_eye {
        recipe.red_eye = red_eye.into();
    }
    if let Some(masks) = local.masks {
        recipe.masks = masks;
    }
    warnings.extend(local.skipped);
    recipe.validate()?;
    warnings.sort();
    warnings.dedup();
    Ok((recipe, warnings))
}

/// Whether `preset` can take the single-pass [`crate::xmp::Preset::apply`] fast path
/// without diverging from the per-group/per-key validator. Returns `false` when `base`
/// itself is not valid, or whenever an ungrouped setting could alter another group's
/// validation outcome compared to testing each group on its own (photo measurements
/// under a preset crop/edit, `CameraProfile` paired with white balance, legacy
/// `RAWmakasePreset` crop interpretation, carried fitted profiles or look UUIDs, or
/// multi-key Lensfun choices).
pub(crate) fn can_try_whole_preset_fast_path(
    preset: &crate::xmp::Preset,
    base: &Recipe,
    image: Option<&dyn crate::xmp::PhotoMeasures>,
) -> bool {
    if base.validate().is_err() {
        return false;
    }
    if image.is_some() && (preset.measures_photo() || preset.settings.contains_key("AutoTone")) {
        return false;
    }
    if preset.settings.contains_key("CameraProfile")
        && ["WhiteBalance", "Temperature", "Tint"]
            .iter()
            .any(|k| preset.settings.contains_key(*k))
    {
        return false;
    }
    if preset.settings.contains_key("RAWmakasePreset")
        || preset.settings.contains_key(crate::xmp::FITTED_PROFILE_KEY)
        || preset.settings.contains_key("RAWmakaseLookUUID")
        || crate::xmp::LENSFUN_KEYS
            .iter()
            .any(|k| preset.settings.contains_key(*k))
    {
        return false;
    }
    true
}

/// The items of a flat Lua list (`{ "a", 1, 2.5 }`): strings decoded, numbers as written.
fn lua_list_items(value: &str) -> Result<Vec<String>> {
    let body = value
        .strip_prefix('{')
        .and_then(|v| v.strip_suffix('}'))
        .context("Expected a list")?;
    let mut items = Vec::new();
    let mut rest = body.trim();
    while !rest.is_empty() {
        let (item, tail) = if let Some(quoted) = rest.strip_prefix('"') {
            let end = quoted
                .char_indices()
                .scan(false, |escaped, (i, c)| {
                    let done = !*escaped && c == '"';
                    *escaped = !*escaped && c == '\\';
                    Some((i, done))
                })
                .find(|(_, done)| *done)
                .map(|(i, _)| i + 2)
                .context("Unterminated string")?;
            (serde_json::from_str::<String>(&rest[..end])?, &rest[end..])
        } else {
            let end = rest.find(',').unwrap_or(rest.len());
            let number = rest[..end].trim();
            ensure!(number.parse::<f64>().is_ok(), "Expected a number");
            (number.to_string(), &rest[end..])
        };
        items.push(item);
        rest = tail.trim_start();
        rest = rest.strip_prefix(',').unwrap_or(rest).trim_start();
    }
    Ok(items)
}

/// Process version 2003/2010 basic controls in process version 2012, as Lightroom 5's
/// "Update to Current Process (2012)" writes them. Every rule here reproduces at least
/// 99% of 341 updated catalog records. Fill Light also adds exposure, 0.02 EV a step up
/// to 25, 0.01 up to 50 and 0.005 above; the records stop at Fill Light 89.
fn legacy_tone(
    old: &std::collections::BTreeMap<String, f64>,
    pv2003: bool,
) -> Vec<(&'static str, f64)> {
    // Lightroom's placeholder for an Auto Tone not yet measured. As in XMP, it on Exposure
    // leaves Auto pending; on another control it counts as that control's default.
    let auto = old.get("Exposure") == Some(&-999999.);
    let get = |key: &str, default: f64| {
        old.get(key)
            .copied()
            .filter(|&v| v != -999999.)
            .unwrap_or(default)
    };
    let fill = get("FillLight", 0.);
    let fill_ev =
        0.02 * fill.min(25.) + 0.01 * (fill.clamp(25., 50.) - 25.) + 0.005 * (fill.max(50.) - 50.);
    let exposure = get("Exposure", 0.) + (get("Brightness", 50.) - 50.) / 50. + fill_ev;
    // The old Shadows slider set the black point; 5 was its default.
    let black = get("Shadows", 5.);
    let clarity = get("Clarity", 0.);
    vec![
        (
            "Exposure2012",
            if auto {
                -999999.
            } else {
                ((exposure * 100.).round() / 100.).clamp(-5., 5.)
            },
        ),
        (
            "Contrast2012",
            ((get("Contrast", 25.) - 25.) * 4. / 3.)
                .round()
                .clamp(-100., 100.),
        ),
        ("Highlights2012", -(fill * 0.5).trunc()),
        ("Shadows2012", (fill * 0.5).clamp(0., 100.).trunc()),
        ("Whites2012", -get("HighlightRecovery", 0.)),
        (
            "Blacks2012",
            if black <= 5. {
                5. * (5. - black)
            } else {
                ((5. - black) / 4.).round()
            },
        ),
        // Lightroom 5 writes half of positive Clarity. On process version 2003 records a
        // fifth comes closer to Camera Raw 19's own rendering of the old edit.
        (
            "Clarity2012",
            if clarity > 0. {
                (clarity * if pv2003 { 0.2 } else { 0.5 }).trunc()
            } else {
                clarity
            },
        ),
    ]
}
