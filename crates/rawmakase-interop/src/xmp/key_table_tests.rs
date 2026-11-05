//! What every Camera Raw setting key means today, frozen before the key table that
//! names them once (#808 step 4.12): reading each key alone at values across and
//! beyond its range, strictly and leniently, over a plain recipe, over one whose
//! every setting differs from the default and whose conditional settings are on
//! (black and white, grain, vignetting, grading), over one with every panel off,
//! and over a rendered file's (relative white balance); what the writer then writes
//! and stops writing; the preset group of each key and of keys only a prefix
//! matches; the preset each group writes on its own; and Lightroom catalog edits
//! with Auto Settings still to measure, and raw defaults made of a preset.
//!
//! Each outcome is compared with `key_table_golden.txt`, written by this test with
//! `RAWMAKASE_BLESS=1`. Floats are written in full (`{:?}` of the f32), so a change
//! of one bit in what a key reads or writes fails the test.
use super::*;
use crate::xml::ns::{CRS, RDF};
use crate::{camera_data::Metadata, model::recipe::Recipe};
use anyhow::Result;
use std::{collections::BTreeSet, fmt::Write as _, path::Path};

const GOLDEN: &str = include_str!("key_table_golden.txt");

/// Values each key is read at over the plain recipe: inside, at and beyond the
/// sliders' ranges (Exposure's ±8, Kelvin), at and beside the written form's rounding
/// edges, signed zeros and tiny values, and text that isn't a number or is a flag.
const VALUES: &[&str] = &[
    "0", "-0", "1e-7", "-1e-7", "+0.5", "-0.5", "0.49", "0.51", "1", "-1", "2.4", "2.005", "2.015",
    "5", "7.99", "8", "8.01", "-8", "-8.01", "-12.5", "25", "33.3", "49.5", "50", "-75", "99.5",
    "100", "-100", "100.01", "150", "180", "360", "2000", "5500", "50000", "50001", "inf", "-inf",
    "NaN", "x", "True", "False", "Custom", "As Shot",
];
/// Values each key is read at over the other recipes.
const SHORT: &[&str] = &[
    "0", "-0", "1", "-1", "7", "50", "-100", "100", "5500", "True", "x",
];

fn packet(attrs: &str) -> String {
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><r:RDF xmlns:r="{RDF}"><r:Description xmlns:c="{CRS}" {attrs}/></r:RDF></x:xmpmeta>"#
    )
}

fn metadata() -> Metadata {
    Metadata {
        wb: [2., 1., 1.5],
        daylight_wb: [2., 1., 1.5],
        width: 600,
        height: 400,
        ..Default::default()
    }
}

/// Camera Raw `key` set to `value` as an attribute, the value escaped for XML.
fn attribute(key: &str, value: &str) -> String {
    let value = value.replace('&', "&amp;").replace('"', "&quot;");
    format!("c:{key}=\"{value}\"")
}

/// The recipe's fields as `path=value` lines, floats in full.
fn fields(r: &Recipe) -> BTreeSet<String> {
    fn walk(prefix: &str, v: &serde_json::Value, out: &mut BTreeSet<String>) {
        match v {
            serde_json::Value::Object(map) => {
                for (k, v) in map {
                    walk(&format!("{prefix}.{k}"), v, out);
                }
            }
            serde_json::Value::Array(items) => {
                for (i, v) in items.iter().enumerate() {
                    walk(&format!("{prefix}[{i}]"), v, out);
                }
            }
            serde_json::Value::Number(n) => {
                // serde_json writes an f32 widened to f64, which keeps every bit.
                let text = match n.as_f64() {
                    Some(f) if n.is_f64() => format!("{:?}", f as f32),
                    _ => n.to_string(),
                };
                out.insert(format!("{prefix}={text}"));
            }
            other => {
                out.insert(format!("{prefix}={other}"));
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(
        "",
        &serde_json::to_value(r).expect("a recipe serializes"),
        &mut out,
    );
    out
}

/// What changed from `base` to `r`, field by field: new values, and fields gone.
fn changes(base: &BTreeSet<String>, r: &BTreeSet<String>) -> String {
    // The preset's own record of what it applied repeats the input.
    let kept = |line: &&String| !line.starts_with(".preset_");
    let path = |line: &String| line.split('=').next().unwrap_or_default().to_string();
    let now: BTreeSet<String> = r.iter().map(path).collect();
    let mut out = String::new();
    for line in r.difference(base).filter(kept) {
        let _ = write!(out, " {line}");
    }
    for line in base.difference(r).filter(kept) {
        if !now.contains(&path(line)) {
            let _ = write!(out, " -{}", path(line));
        }
    }
    out
}

/// What the writer writes for `r` and no longer writes, against `before`.
fn writes(before: &BTreeSet<String>, r: &Recipe, m: &Metadata) -> String {
    let now = written(r, m);
    let mut out = String::new();
    for line in now.difference(before) {
        let _ = write!(out, " {line}");
    }
    let keys: BTreeSet<&str> = now.iter().filter_map(|l| l.split('=').next()).collect();
    for line in before.difference(&now) {
        let key = line.split('=').next().unwrap_or_default();
        if !keys.contains(key) {
            let _ = write!(out, " -{key}");
        }
    }
    out
}

/// The writer's output for `r` as `key=value` lines.
fn written(r: &Recipe, m: &Metadata) -> BTreeSet<String> {
    write::settings(r, m, None)
        .0
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect()
}

/// Every key the writer writes for `bases`, the Auto Settings keys and the keys only
/// the reader knows of that a preset may carry.
fn keys(bases: &[(&str, Recipe)], m: &Metadata) -> Vec<String> {
    let mut keys: BTreeSet<String> = bases
        .iter()
        .flat_map(|(_, r)| write::settings(r, m, None).0.into_iter().map(|(k, _)| k))
        .collect();
    keys.extend(AUTO_TONE_KEYS.iter().map(|k| k.to_string()));
    keys.extend(
        [
            "Exposure",
            "Contrast",
            "Highlights",
            "Shadows",
            "Clarity",
            "Brightness",
            // High Dynamic Range, written only once set.
            "HDREditMode",
            "HDRMaxValue",
            "SDRBrightness",
            "SDRContrast",
            "SDRClarity",
            "SDRHighlights",
            "SDRShadows",
            "SDRWhites",
            "SDRBlend",
            "AutoTone",
            "AutoWhiteVersion",
            "LensManualDistortionAmount",
            "VignetteAmount",
            "VignetteMidpoint",
            "PerspectiveVertical",
            "PerspectiveHorizontal",
            "PerspectiveRotate",
            "PerspectiveScale",
            "PerspectiveAspect",
            "PerspectiveX",
            "PerspectiveY",
            "DefringePurpleAmount",
            "DefringeGreenAmount",
            "ShadowTint",
            "RedHue",
            "RedSaturation",
            "GreenHue",
            "GreenSaturation",
            "BlueHue",
            "BlueSaturation",
            "SplitToningShadowHue",
            "SplitToningShadowSaturation",
            "SplitToningHighlightHue",
            "SplitToningHighlightSaturation",
            "SplitToningBalance",
            "ColorGradeBlending",
            "ColorGradeGlobalHue",
            "ColorGradeGlobalSat",
            "ColorGradeGlobalLum",
            "GrainAmount",
            "GrainSize",
            "GrainFrequency",
            "PostCropVignetteAmount",
            "PostCropVignetteMidpoint",
            "PostCropVignetteFeather",
            "PostCropVignetteRoundness",
            "PostCropVignetteStyle",
            "PostCropVignetteHighlightContrast",
            "CropAngle",
            "CropLeft",
            "CropTop",
            "CropRight",
            "CropBottom",
            // Keys `group_of_key` names exactly, and switches and identities the writer
            // writes only under conditions.
            "WhiteBalance",
            "Temperature",
            "Tint",
            "IncrementalTemperature",
            "IncrementalTint",
            "CameraProfile",
            "ConvertToGrayscale",
            "ProcessVersion",
            "EnableDetail",
            "AutoLateralCA",
            "PerspectiveUpright",
            "EnableToneCurve",
            "CurveRefineSaturation",
            "EnableColorAdjustments",
            "PointColors",
            "ColorVariance",
            "EnableGrayscaleMix",
            "EnableSplitToning",
            "EnableLensCorrections",
            "EnableTransform",
            "EnableEffects",
            "EnableCalibration",
            "EnableRetouch",
            "HasCrop",
            "CropConstrainToWarp",
            "LensProfileSetup",
            "LensProfileName",
            "LensProfileFilename",
            "LensProfileEnable",
            "UprightVersion",
            "UprightCenterMode",
            "UprightFocalMode",
            "UprightPreview",
            "UprightTransformCount",
        ]
        .map(String::from),
    );
    keys.into_iter()
        .filter(|k| !k.starts_with(RAWMAKASE_PREFIX))
        .collect()
}

/// A recipe the cases read over: its name, the recipe, its photo and profiles, and
/// the values its keys are read at.
struct Base {
    name: &'static str,
    recipe: Recipe,
    metadata: Metadata,
    profiles: Vec<std::sync::Arc<crate::camera_profiles::CameraProfile>>,
    values: &'static [&'static str],
}

#[test]
fn every_camera_raw_key_reads_writes_and_groups_as_before() -> Result<()> {
    let m = metadata();
    let plain = Recipe::with_profiles(&m, &[]);
    let read = |attrs: &str| parse(Path::new("base.xmp"), &packet(attrs));
    // The conditional settings on, then every key set, one at a time and strictly
    // over the recipe so far, to the first of these values that applies and changes
    // it: a key read alone over this recipe shows whether a reader resets what the
    // preset leaves out. The values chosen are part of the golden.
    let mut rich = read(
        r#"c:ConvertToGrayscale="True" c:GrainAmount="25" c:GrainSize="30" c:PostCropVignetteAmount="-20" c:SplitToningShadowHue="200" c:SplitToningShadowSaturation="20" c:SplitToningHighlightHue="50" c:SplitToningHighlightSaturation="30" c:SplitToningBalance="+15" c:ColorGradeMidtoneHue="40" c:ColorGradeMidtoneSat="15" c:VignetteAmount="10" c:DefringePurpleAmount="5""#,
    )?
    .apply(&plain, &m, &[], None)?;
    let mut chosen = Vec::new();
    let candidates = ["7", "-7", "3", "13", "0.3", "45", "1.5", "5300", "70"];
    for key in keys(&[("plain", plain.clone()), ("on", rich.clone())], &m) {
        // Legacy split toning (set above, with the modern grading) converts to
        // grading and clears its modern values, and the lens profile's identity is one
        // choice its keys replace together; their keys are read over this recipe like
        // any other.
        let lens_identity = matches!(
            key.as_str(),
            "LensProfileSetup"
                | "LensProfileName"
                | "LensProfileFilename"
                | "LensProfileDigest"
                | "LensProfileIsEmbedded"
        );
        if key.starts_with("SplitToning") || lens_identity {
            continue;
        }
        for value in candidates {
            let Ok(preset) = read(&attribute(&key, value)) else {
                continue;
            };
            if let Ok(next) = preset.apply(&rich, &m, &[], None)
                && fields(&next) != fields(&rich)
            {
                rich = next;
                chosen.push(format!("{key}={value}"));
                break;
            }
        }
    }
    // No later key undid an earlier one: setting each again changes nothing.
    for set in &chosen {
        let (key, value) = set.split_once('=').expect("key=value");
        let again = read(&attribute(key, value))?.apply(&rich, &m, &[], None)?;
        let kept = |r: &Recipe| {
            fields(r)
                .into_iter()
                .filter(|l| !l.starts_with(".preset_"))
                .collect::<BTreeSet<_>>()
        };
        assert_eq!(kept(&again), kept(&rich), "the rich recipe lost {set}");
    }
    let off = read(
        r#"c:EnableDetail="False" c:EnableToneCurve="False" c:EnableColorAdjustments="False" c:EnableGrayscaleMix="False" c:EnableSplitToning="False" c:EnableLensCorrections="False" c:EnableTransform="False" c:EnableEffects="False" c:EnableCalibration="False" c:EnableRetouch="False""#,
    )?
    .apply(&rich, &m, &[], None)?;
    let rendered = Metadata {
        source: crate::camera_data::Source::Rendered,
        ..Default::default()
    };
    let rendered_profiles = crate::camera_profiles::installed(&rendered).0;
    let jpeg =
        read(r#"c:IncrementalTemperature="+25" c:IncrementalTint="-10" c:WhiteBalance="Custom""#)?
            .apply(
                &Recipe::for_rendered(&rendered),
                &rendered,
                &rendered_profiles,
                None,
            )?;
    let bases = [
        Base {
            name: "plain",
            recipe: plain,
            metadata: m.clone(),
            profiles: Vec::new(),
            values: VALUES,
        },
        Base {
            name: "rich",
            recipe: rich,
            metadata: m.clone(),
            profiles: Vec::new(),
            values: SHORT,
        },
        Base {
            name: "off",
            recipe: off,
            metadata: m.clone(),
            profiles: Vec::new(),
            values: SHORT,
        },
        Base {
            name: "jpeg",
            recipe: jpeg,
            metadata: rendered,
            profiles: rendered_profiles,
            values: SHORT,
        },
    ];
    let mut out = String::new();
    let _ = writeln!(out, "# rich sets {}", chosen.join(" "));
    for base in &bases {
        let _ = writeln!(out, "# base {}", base.name);
        for line in written(&base.recipe, &base.metadata) {
            let _ = writeln!(out, "  writes {line}");
        }
    }
    let written_keys: Vec<(&str, Recipe)> =
        bases.iter().map(|b| (b.name, b.recipe.clone())).collect();
    for key in keys(&written_keys, &m) {
        let _ = writeln!(out, "# {key} group {:?}", preset_write::group_of_key(&key));
        for base in &bases {
            let before = fields(&base.recipe);
            let before_written = written(&base.recipe, &base.metadata);
            // Values with the same outcome share a line, in the order first read.
            let mut outcomes: Vec<(String, Vec<&str>)> = Vec::new();
            let mut record = |value: &'static str, outcome: String| match outcomes
                .iter_mut()
                .find(|(o, _)| *o == outcome)
            {
                Some((_, values)) => values.push(value),
                None => outcomes.push((outcome, vec![value])),
            };
            for value in base.values {
                let text = packet(&attribute(&key, value));
                let preset = match parse(Path::new("case.xmp"), &text) {
                    Ok(p) => p,
                    Err(e) => {
                        record(value, format!("parse error {e:#}"));
                        continue;
                    }
                };
                let strict = preset.apply(&base.recipe, &base.metadata, &base.profiles, None);
                let lenient =
                    preset.apply_lenient(&base.recipe, &base.metadata, &base.profiles, None);
                // A lenient read that agrees with the strict one, with no warning, is
                // written as such.
                let same = matches!((&strict, &lenient), (Ok(a), Ok((b, w))) if w.is_empty() && fields(a) == fields(b));
                let strict = match strict {
                    Ok(r) => format!(
                        "ok{} | writes{}",
                        changes(&before, &fields(&r)),
                        writes(&before_written, &r, &base.metadata)
                    ),
                    Err(e) => format!("error {e:#}"),
                };
                let lenient = match lenient {
                    _ if same => "lenient same".to_string(),
                    Ok((r, warnings)) => format!(
                        "lenient{} warns {warnings:?}",
                        changes(&before, &fields(&r))
                    ),
                    Err(e) => format!("lenient error {e:#}"),
                };
                record(value, format!("{strict} || {lenient}"));
            }
            for (outcome, values) in outcomes {
                let _ = writeln!(out, "{} {key}={values:?} {outcome}", base.name);
            }
        }
    }
    let bases = [
        ("plain", bases[0].recipe.clone()),
        ("rich", bases[1].recipe.clone()),
    ];
    // Keys a prefix family groups, and keys nothing groups.
    for key in [
        "Parametric",
        "ParametricSomething",
        "ToneCurveName2012",
        "ToneCurvePV2012Purple",
        "HueAdjustmentTeal",
        "SaturationAdjustment",
        "LuminanceAdjustmentRed",
        "GrayMixerTeal",
        "SplitToning",
        "ColorGradeSomething",
        "Sharpen",
        "SharpenSomething",
        "LuminanceNoiseReductionSomething",
        "ColorNoiseReductionSomething",
        "LensProfileSomething",
        "rawmakase:LensfunSomething",
        "DefringeSomething",
        "UprightSomething",
        "PerspectiveSomething",
        "PostCropVignetteSomething",
        "GrainSomething",
        "Exposure2012Extra",
        "crs:Exposure2012",
        "exposure2012",
        "Unknown",
        "",
    ] {
        let _ = writeln!(out, "# {key:?} group {:?}", preset_write::group_of_key(key));
    }
    // A preset of each group on its own, and of every group, of the recipe with its
    // conditional settings on.
    {
        use crate::model::settings_groups::{GroupInclusion, GroupSelection, SettingGroup};
        let info = preset_write::PresetInfo {
            name: "Case".into(),
            group: "Tests".into(),
            uuid: "0123456789ABCDEF0123456789ABCDEF".into(),
        };
        let mut selections: Vec<(String, GroupSelection)> = SettingGroup::ALL
            .into_iter()
            .map(|g| {
                let mut only = GroupSelection::none();
                only.set(g, GroupInclusion::Included);
                (format!("{g:?}"), only)
            })
            .collect();
        selections.push(("all".into(), GroupSelection::all()));
        for (name, selection) in selections {
            let text = preset_write::preset(&bases[1].1, &m, &info, &selection);
            let _ = writeln!(out, "# preset {name}");
            for line in text.lines() {
                let _ = writeln!(out, "  {line}");
            }
        }
    }
    // Lightroom catalog edits: Auto Settings still to measure, and legacy keys.
    for (case, text) in [
        (
            "auto unmeasured",
            "s = { AutoTone = true, Exposure2012 = -999999, Contrast2012 = -999999, Vibrance = -999999 }",
        ),
        (
            "auto measured",
            "s = { AutoTone = true, Exposure2012 = 0.35, Contrast2012 = 12 }",
        ),
        (
            "not auto",
            "s = { AutoTone = false, Exposure2012 = -999999 }",
        ),
        (
            "legacy exposure",
            "s = { Exposure = 0.5, ProcessVersion = \"6.7\" }",
        ),
        (
            "current and legacy",
            "s = { Exposure2012 = 0.25, Exposure = 0.5 }",
        ),
        (
            "out of range",
            "s = { Contrast2012 = 150, Exposure2012 = 9 }",
        ),
    ] {
        let line = match crate::lr_develop::convert_develop(text, &m, &[], None) {
            Ok((r, warnings)) => format!(
                "ok{} warns {warnings:?}",
                changes(&fields(&bases[0].1), &fields(&r))
            ),
            Err(e) => format!("error {e:#}"),
        };
        let _ = writeln!(out, "lightroom {case}: {line}");
    }
    // Raw defaults: a preset over Adobe Default, one that can't apply, and Upright.
    for (case, attrs) in [
        (
            "preset",
            r#"c:Exposure2012="+0.50" c:Contrast2012="+20" c:Texture="-10""#,
        ),
        ("invalid", r#"c:Contrast2012="+150""#),
        (
            "upright",
            r#"c:PerspectiveUpright="1" c:Exposure2012="+0.25""#,
        ),
        (
            "unknown setting",
            r#"c:SomethingNew="1" c:Exposure2012="+0.25""#,
        ),
    ] {
        let mut preset = read(attrs)?;
        preset.id = "case".into();
        preset.name = "Case".into();
        let defaults = crate::raw_defaults::DevelopDefaults::with_presets(
            crate::raw_defaults::RawDefaults {
                master: crate::raw_defaults::DefaultChoice::Preset {
                    id: "case".into(),
                    name: "Case".into(),
                },
                ..Default::default()
            },
            |id| (id == "case").then(|| preset.clone()),
        );
        let resolved = defaults.resolve(&m, &[]);
        let _ = writeln!(
            out,
            "raw defaults {case}: {}{} note {:?}",
            resolved.name,
            changes(&fields(&bases[0].1), &fields(&resolved.recipe)),
            resolved.note
        );
    }
    if std::env::var_os("RAWMAKASE_BLESS").is_some() {
        std::fs::write(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("src/xmp/key_table_golden.txt"),
            &out,
        )?;
        return Ok(());
    }
    for (i, (want, got)) in GOLDEN.lines().zip(out.lines()).enumerate() {
        assert_eq!(got, want, "line {}", i + 1);
    }
    assert_eq!(out.lines().count(), GOLDEN.lines().count(), "line count");
    Ok(())
}
