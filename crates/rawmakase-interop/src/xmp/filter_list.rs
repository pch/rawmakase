//! Lightroom's filters (`crs:FilterList`): today the Enhance filter (Raw Details,
//! Super Resolution and Denoise), none of which RAWmakase renders.
//!
//! Each filter (`crs:Filters/rdf:Seq/rdf:li`) names its settings by
//! `CompressedSettings`: a `Table_<id>` attribute of the same description, in
//! Adobe's table encoding, holding a 16-byte header (its last word the length of
//! the rest) and an XMP packet of Camera Raw settings. A filter whose settings are
//! all at rest renders nothing; any other is a blocker naming what it needs.
use crate::xml::ns::{CRS, RDF};
use anyhow::{Result, ensure};
use std::collections::{BTreeMap, BTreeSet};

/// The blockers of a `crs:FilterList` element. The `Table_<id>` settings its
/// filters name are taken out of `settings`: they belong to the filter.
pub(super) fn read(
    node: roxmltree::Node<'_, '_>,
    settings: &mut BTreeMap<String, String>,
) -> Vec<String> {
    let mut blockers = Vec::new();
    // Filters sharing settings are read once.
    let mut tables = BTreeSet::new();
    for child in node.children().filter(|n| n.is_element()) {
        if !child.has_tag_name((CRS, "Filters")) {
            blockers.push(format!(
                "Unsupported Lightroom filter list entry: {}",
                child.tag_name().name()
            ));
            continue;
        }
        let filters = child
            .children()
            .filter(|n| n.is_element())
            .flat_map(|seq| seq.children().filter(|n| n.is_element()));
        for filter in filters {
            let Some(id) = filter.attribute((CRS, "CompressedSettings")) else {
                blockers.push("Lightroom filter without settings is not supported".into());
                continue;
            };
            let key = format!("Table_{id}");
            if tables.contains(&key) {
                continue;
            }
            let Some(text) = settings.get(&key) else {
                blockers.push(format!("Lightroom filter settings {key} are missing"));
                continue;
            };
            match active(text) {
                Ok(active) if active.is_empty() => {}
                Ok(active) => blockers.push(active),
                Err(e) => blockers.push(format!("Unreadable Lightroom filter settings: {e:#}")),
            }
            tables.insert(key);
        }
    }
    for key in tables {
        settings.remove(&key);
    }
    blockers
}

/// What a filter's settings would change, as a blocker, or nothing when every
/// setting is at rest.
fn active(text: &str) -> Result<String> {
    let data = rawmakase_model::camera_profiles::table_bytes(text)?;
    ensure!(data.len() >= 16, "truncated header");
    let length = u32::from_le_bytes(data[12..16].try_into()?) as usize;
    ensure!(length == data.len() - 16, "invalid length");
    classify(std::str::from_utf8(&data[16..])?.trim_end_matches('\0'))
}

/// [`active`] for a filter's decoded XMP packet.
fn classify(packet: &str) -> Result<String> {
    let doc = roxmltree::Document::parse(packet)?;
    // Every description: XMP may spread one subject's settings over several.
    let values: Vec<_> = doc
        .descendants()
        .filter(|n| n.has_tag_name((RDF, "Description")))
        .flat_map(|d| {
            d.attributes()
                .filter(|a| a.namespace() == Some(CRS))
                .map(|a| (a.name(), a.value().trim()))
                .chain(
                    d.children()
                        .filter(|n| n.is_element() && n.tag_name().namespace() == Some(CRS))
                        .map(|n| (n.tag_name().name(), n.text().unwrap_or("").trim())),
                )
        })
        .collect();
    // An absent switch is off, as in Camera Raw, but a filter naming none of them
    // isn't known to be an Enhance filter at rest.
    ensure!(
        values
            .iter()
            .any(|(key, _)| matches!(*key, "Details" | "SuperResolution" | "Denoise")),
        "no Enhance settings"
    );
    let off = |v: &str| v.eq_ignore_ascii_case("False");
    let mut enhance = Vec::new();
    let mut unknown = Vec::new();
    for (key, value) in values {
        match key {
            "Details" if !off(value) => enhance.push("Raw Details"),
            "SuperResolution" if value != "1/1" => enhance.push("Super Resolution"),
            "Denoise" if !off(value) => enhance.push("Denoise"),
            // Denoise's strength, which does nothing while Denoise is off.
            "Details" | "SuperResolution" | "Denoise" | "DenoiseLumaAmount" => {}
            _ => unknown.push(key),
        }
    }
    Ok(if !unknown.is_empty() {
        format!(
            "Unsupported Lightroom filter settings: {}",
            unknown.join(", ")
        )
    } else if !enhance.is_empty() {
        format!("Enhance ({}) is not supported", enhance.join(", "))
    } else {
        String::new()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn packet(attrs: &str) -> String {
        format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="{RDF}"><rdf:Description xmlns:crs="{CRS}" {attrs}/></rdf:RDF></x:xmpmeta>"#
        )
    }
    #[test]
    fn each_enhance_setting_is_named() -> Result<()> {
        let rest = r#"crs:Details="False" crs:SuperResolution="1/1" crs:Denoise="False" crs:DenoiseLumaAmount="50""#;
        assert_eq!(classify(&packet(rest))?, "");
        for (attrs, expected) in [
            (
                r#"crs:Details="True""#,
                "Enhance (Raw Details) is not supported",
            ),
            (
                r#"crs:SuperResolution="2/1""#,
                "Enhance (Super Resolution) is not supported",
            ),
            (
                r#"crs:Details="True" crs:Denoise="True""#,
                "Enhance (Raw Details, Denoise) is not supported",
            ),
            (
                r#"crs:Denoise="False" crs:ReflectionRemoval="True""#,
                "Unsupported Lightroom filter settings: ReflectionRemoval",
            ),
        ] {
            assert_eq!(classify(&packet(attrs))?, expected, "{attrs}");
        }
        // A filter with no settings, or only Denoise's strength, is not known to be
        // neutral.
        assert!(classify(&packet("")).is_err());
        assert!(classify(&packet(r#"crs:DenoiseLumaAmount="50""#)).is_err());
        // Settings in a second description of the subject count too.
        let split = format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="{RDF}"><rdf:Description xmlns:crs="{CRS}" crs:Details="False" crs:SuperResolution="1/1"/><rdf:Description xmlns:crs="{CRS}" crs:Denoise="True"/></rdf:RDF></x:xmpmeta>"#
        );
        assert_eq!(classify(&split)?, "Enhance (Denoise) is not supported");
        Ok(())
    }
}
