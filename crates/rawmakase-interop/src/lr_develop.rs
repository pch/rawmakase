//! Lightroom's Develop settings, as a catalog stores them (a serialized Lua
//! table), translated into a recipe through the XMP settings they mirror. The
//! table is parsed as data; no interpreter runs. Used to show and resolve photos
//! imported from Lightroom; reading the Lightroom catalog itself is
//! `catalog::lightroom`.
use crate::model::recipe::Recipe;
use crate::xmp::look::LookAmount;
use anyhow::{Context, Result, ensure};
use std::path::PathBuf;
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
    // A record from before process version 2012 (6.7) with no 2012 tone keys.
    let legacy = fields
        .get("ProcessVersion")
        .and_then(|v| v.trim_matches('"').parse::<f32>().ok())
        .is_some_and(|v| v < 6.7)
        && !fields.keys().any(|k| k.ends_with("2012"));
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
            // only these: exposure converts directly, the rest cannot be converted
            // faithfully and are reported instead of silently dropped.
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
                    if key == "Exposure" {
                        preset.settings.insert("Exposure2012".into(), value);
                    } else if value.parse::<f64>().is_ok_and(|v| v != legacy_default(key)) {
                        warnings.push(format!("{key} (process version 2003/2010, not converted)"));
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
    }
    // HDR on is refused outright, not skipped as one setting among others.
    preset.check_hdr_renderable()?;
    // Spots and masks convert on their own and report what they skip.
    let local_settings = std::mem::take(&mut preset.local);
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
        match p.apply(&Recipe::with_profiles(m, profiles), m, profiles, image) {
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
        match p.apply(&Recipe::with_profiles(m, profiles), m, profiles, image) {
            Ok(_) => {
                accepted.settings.insert(key.clone(), value.clone());
            }
            Err(e) => warnings.push(e.to_string()),
        }
    }
    let base = Recipe::with_profiles(m, profiles);
    let mut recipe = accepted.apply(&base, m, profiles, image)?;
    if let Some((asked, used)) = accepted.profile_substitute(m, profiles) {
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

/// Lightroom's defaults for process version 2003/2010 basic controls.
fn legacy_default(key: &str) -> f64 {
    match key {
        "Contrast" => 25.,
        "Brightness" => 50.,
        "Shadows" => 5.,
        _ => 0.,
    }
}
