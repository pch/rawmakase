use super::{
    Preset,
    ns::{CRS, PHOTOSHOP, RDF, XML},
};
use crate::develop::curve::ToneCurve;
use anyhow::{Context, Result, ensure};
use std::{collections::BTreeMap, path::Path};
/// A preset's name and description, not settings.
const LABELS: [&str; 5] = ["Name", "Group", "ShortName", "SortName", "Description"];
/// Lightroom 18.5's empty point color selection: 19 values of -1.
fn is_empty_point_colors(node: roxmltree::Node<'_, '_>) -> bool {
    let values: Vec<_> = node
        .descendants()
        .filter(|n| n.has_tag_name((RDF, "li")))
        .filter_map(|n| n.text())
        .flat_map(|s| s.split(','))
        .collect();
    values.len() == 19 && values.iter().all(|v| v.trim().parse::<f32>() == Ok(-1.))
}
fn child_text(node: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    let child = node.children().find(|n| n.has_tag_name((CRS, name)))?;
    let item = child
        .descendants()
        .find(|n| n.has_tag_name((RDF, "li")) && n.attribute((XML, "lang")) == Some("x-default"))
        .or_else(|| child.descendants().find(|n| n.has_tag_name((RDF, "li"))));
    Some(
        item.and_then(|n| n.text())
            .or_else(|| child.text())
            .unwrap_or("")
            .trim()
            .to_string(),
    )
}
pub fn parse(path: &Path, text: &str) -> Result<Preset> {
    ensure!(text.len() < 8_000_000, "XMP file too large");
    // A specific malformed export in this collection repeats the Group close tag.
    // Repair only consecutive duplicate Group closes; never rewrite the source file.
    let mut normalized = String::with_capacity(text.len());
    let mut previous = "";
    let mut repaired = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == "</crs:Group>" && previous == trimmed {
            repaired = true;
            continue;
        }
        normalized.push_str(line);
        normalized.push('\n');
        previous = trimmed;
    }
    let doc = roxmltree::Document::parse(&normalized)?;
    let description = doc
        .descendants()
        .find(|n| {
            n.has_tag_name((RDF, "Description"))
                && (n.attributes().any(|a| a.namespace() == Some(CRS))
                    || n.children().any(|c| c.tag_name().namespace() == Some(CRS)))
                && n.parent().is_some_and(|p| p.has_tag_name((RDF, "RDF")))
        })
        .context("No top-level XMP description")?;
    let sidecar = description
        .attribute((PHOTOSHOP, "SidecarForExtension"))
        .is_some();
    let mut settings: BTreeMap<_, _> = description
        .attributes()
        .filter(|a| a.namespace() == Some(CRS))
        .map(|a| (a.name().to_string(), a.value().to_string()))
        .collect();
    let mut curves = BTreeMap::new();
    let mut blockers = Vec::new();
    let mut look = String::new();
    let mut local = BTreeMap::new();
    for node in description
        .children()
        .filter(|n| n.is_element() && n.tag_name().namespace() == Some(CRS))
    {
        let name = node.tag_name().name();
        if !node.children().any(|n| n.is_element())
            && !LABELS.contains(&name)
            && !matches!(name, "Look" | "PointColors" | "ColorVariance")
            && let Some(text) = node.text()
        {
            settings.insert(name.to_string(), text.trim().to_string());
            continue;
        }
        if [
            "ToneCurve",
            "ToneCurveRed",
            "ToneCurveGreen",
            "ToneCurveBlue",
            "ToneCurvePV2012",
            "ToneCurvePV2012Red",
            "ToneCurvePV2012Green",
            "ToneCurvePV2012Blue",
        ]
        .contains(&name)
        {
            let mut points = Vec::new();
            for item in node.descendants().filter(|n| n.has_tag_name((RDF, "li"))) {
                let v = item
                    .text()
                    .context("Empty curve point")?
                    .split(',')
                    .map(|s| s.trim().parse::<f32>())
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                ensure!(v.len() == 2, "Invalid curve point");
                points.push([v[0] / 255., v[1] / 255.]);
            }
            let curve = ToneCurve {
                points,
                smooth: true,
                natural: true,
            };
            curve.validate()?;
            curves.insert(name.to_string(), curve);
        } else if super::local::KEYS.contains(&name) {
            let value = super::local::Node::from_xml(node);
            if !value.is_empty() {
                local.insert(name.to_string(), value);
            }
        } else if name == "Look" {
            let d = node
                .descendants()
                .find(|n| n.has_tag_name((RDF, "Description")))
                .unwrap_or(node);
            look = d
                .attribute((CRS, "Name"))
                .map(str::to_string)
                .or_else(|| child_text(d, "Name"))
                .unwrap_or_default();
            let rendering = d
                .attribute((CRS, "Amount"))
                .map(|amount| super::look::LookAmount::parse(amount).map(|a| a.rendering()))
                .transpose()?;
            // A strict preset stays atomic: an amount other than 0 or 100% blocks
            // `apply`, and lenient application reports it.
            if let Some(rendering) = rendering {
                blockers.extend(rendering.warning);
                if rendering.look == super::look::LookUse::Omit {
                    look.clear();
                }
            }
            if let Some(uuid) = d.attribute((CRS, "UUID")).filter(|_| !look.is_empty()) {
                settings.insert("RAWmakaseLookUUID".into(), uuid.into());
            }
        } else if name == "Preset" && sidecar {
            // In a photo sidecar, the nested preset-amount record is provenance.
            // The resolved top-level edits (including curves) remain authoritative.
        } else if matches!(name, "PointColors" | "ColorVariance") && {
            // Lightroom 18.5 writes these exact empty-selection sentinels.
            if name == "PointColors" {
                is_empty_point_colors(node)
            } else {
                let values: Vec<_> = node
                    .descendants()
                    .filter(|n| n.has_tag_name((RDF, "li")))
                    .filter_map(|n| n.text())
                    .flat_map(|s| s.split(','))
                    .map(|v| v.trim().parse::<f32>())
                    .collect();
                values.len() == 1
                    && matches!(values[0], Ok(n) if n == -50.)
                    && description
                        .children()
                        .find(|n| n.has_tag_name((CRS, "PointColors")))
                        .is_some_and(is_empty_point_colors)
            }
        } {
            // No selected point color to adjust.
        } else if !LABELS.contains(&name) {
            let active = node.descendants().any(|n| {
                n.has_tag_name((RDF, "li")) && n.text().is_some_and(|s| !s.trim().is_empty())
            });
            if active || !matches!(name, "PointColors" | "ColorVariance") {
                blockers.push(format!("Unsupported structured setting: {name}"));
            }
        }
    }
    let name = child_text(description, "Name")
        .filter(|s| !s.is_empty())
        .or_else(|| settings.get("Name").cloned())
        .unwrap_or_else(|| {
            path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
    let fallback = path
        .parent()
        .and_then(|p| p.file_name())
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let group = child_text(description, "Group")
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback);
    let id = path.to_string_lossy().to_string();
    let mut notes = Vec::new();
    if repaired {
        notes.push("Recovered duplicated XML Group closing tag; original file unchanged".into());
    }
    let preset = Preset {
        photo_settings: sidecar,
        id,
        name,
        group,
        path: path.to_path_buf(),
        settings,
        curves,
        look,
        blockers,
        notes,
        local,
        builtin: false,
    };
    ensure!(
        !preset.settings.is_empty() || !preset.curves.is_empty(),
        "No camera settings"
    );
    Ok(preset)
}
