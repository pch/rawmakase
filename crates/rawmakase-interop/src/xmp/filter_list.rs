//! Lightroom's filters (`crs:FilterList`): today the Enhance filter (Raw Details,
//! Super Resolution and Denoise). Denoise alone imports as AI Denoise at its Amount (in a
//! photo's settings, not a preset); Raw Details, Super Resolution and Denoise whose
//! result Camera Raw flattened into the image are not rendered.
//!
//! Each filter (`crs:Filters/rdf:Seq/rdf:li`) names its settings by
//! `CompressedSettings`: a `Table_<id>` attribute of the same description, in
//! Adobe's table encoding, holding a 16-byte header (its last word the length of
//! the rest) and an XMP packet of Camera Raw settings. A filter whose settings are
//! all at rest renders nothing; Denoise alone (`Denoise="True"`, `DenoiseLumaAmount`
//! 0–100, as Camera Raw 18.7 writes it) becomes the setting [`AI_DENOISE`]; any other is a
//! blocker naming what it needs.
use crate::xml::ns::{CRS, RDF};
use anyhow::{Result, ensure};
use std::collections::{BTreeMap, BTreeSet};

/// The setting a Denoise-only Enhance filter leaves for `apply`: its Amount, 0–100.
pub(crate) const AI_DENOISE: &str = "RAWmakaseAiDenoise";

/// What an Enhance filter's settings ask for.
#[derive(Debug, PartialEq)]
enum Enhance {
    /// Everything at rest: nothing to render.
    Rest,
    /// Denoise alone, at this Amount (0–100).
    Denoise(f32),
    /// What RAWmakase cannot render, for the user.
    Blocker(String),
}

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
            // On the list item itself (Camera Raw's presets), or on the description
            // nested in it (Camera Raw's photo sidecars).
            let id = filter.attribute((CRS, "CompressedSettings")).or_else(|| {
                filter
                    .children()
                    .filter(|n| n.has_tag_name((RDF, "Description")))
                    .find_map(|d| d.attribute((CRS, "CompressedSettings")))
            });
            let Some(id) = id else {
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
                Ok(Enhance::Rest) => {}
                Ok(Enhance::Denoise(amount)) => {
                    settings.insert(AI_DENOISE.into(), amount.to_string());
                }
                Ok(Enhance::Blocker(why)) => blockers.push(why),
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

/// What a filter's settings ask for.
fn active(text: &str) -> Result<Enhance> {
    let data = rawmakase_model::camera_profiles::table_bytes(text)?;
    ensure!(data.len() >= 16, "truncated header");
    let length = u32::from_le_bytes(data[12..16].try_into()?) as usize;
    ensure!(length == data.len() - 16, "invalid length");
    classify(std::str::from_utf8(&data[16..])?.trim_end_matches('\0'))
}

/// [`active`] for a filter's decoded XMP packet.
fn classify(packet: &str) -> Result<Enhance> {
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
    let mut amount = None;
    let mut baked = false;
    for (key, value) in values {
        match key {
            "Details" if !off(value) => enhance.push("Raw Details"),
            "SuperResolution" if value != "1/1" => enhance.push("Super Resolution"),
            "Denoise" if !off(value) => enhance.push("Denoise"),
            // Denoise's strength, which does nothing while Denoise is off.
            "DenoiseLumaAmount" => amount = value.parse::<f32>().ok(),
            // Camera Raw flattened an earlier AI result into the image: not a setting
            // RAWmakase can redo.
            "UpstreamWasBaked" => baked = !off(value),
            "Details" | "SuperResolution" | "Denoise" => {}
            _ => unknown.push(key),
        }
    }
    Ok(if !unknown.is_empty() {
        Enhance::Blocker(format!(
            "Unsupported Lightroom filter settings: {}",
            unknown.join(", ")
        ))
    } else if baked {
        Enhance::Blocker("Enhance with flattened AI settings is not supported".into())
    } else if enhance == ["Denoise"] {
        match amount.filter(|a| a.is_finite() && (0. ..=100.).contains(a)) {
            Some(amount) => Enhance::Denoise(amount),
            None => Enhance::Blocker("Enhance (Denoise) without a valid amount".into()),
        }
    } else if !enhance.is_empty() {
        Enhance::Blocker(format!("Enhance ({}) is not supported", enhance.join(", ")))
    } else {
        Enhance::Rest
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
        assert_eq!(classify(&packet(rest))?, Enhance::Rest);
        for (attrs, expected) in [
            (
                r#"crs:Details="True""#,
                "Enhance (Raw Details) is not supported",
            ),
            (
                r#"crs:Denoise="True""#,
                "Enhance (Denoise) without a valid amount",
            ),
            (
                r#"crs:Denoise="True" crs:DenoiseLumaAmount="150""#,
                "Enhance (Denoise) without a valid amount",
            ),
            (
                r#"crs:Denoise="True" crs:DenoiseLumaAmount="50" crs:UpstreamWasBaked="True""#,
                "Enhance with flattened AI settings is not supported",
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
            assert_eq!(
                classify(&packet(attrs))?,
                Enhance::Blocker(expected.into()),
                "{attrs}"
            );
        }
        // A filter with no settings, or only Denoise's strength, is not known to be
        // neutral.
        assert!(classify(&packet("")).is_err());
        assert!(classify(&packet(r#"crs:DenoiseLumaAmount="50""#)).is_err());
        // Settings in a second description of the subject count too.
        let split = format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="{RDF}"><rdf:Description xmlns:crs="{CRS}" crs:Details="False" crs:SuperResolution="1/1"/><rdf:Description xmlns:crs="{CRS}" crs:Denoise="True"/></rdf:RDF></x:xmpmeta>"#
        );
        assert_eq!(
            classify(&split)?,
            Enhance::Blocker("Enhance (Denoise) without a valid amount".into())
        );
        Ok(())
    }

    /// Camera Raw 18.7's own Enhance tables for Denoise at 25, 50 and 100, as it wrote
    /// them on 2026-11-02 (the same at every Amount across five cameras and a DNG): the
    /// Amount comes through as AI Denoise's.
    #[test]
    fn camera_raws_denoise_tables_import_as_their_amount() -> Result<()> {
        for (table, amount) in [(DENOISE_25, 25.), (DENOISE_50, 50.), (DENOISE_100, 100.)] {
            assert_eq!(active(table)?, Enhance::Denoise(amount));
        }
        Ok(())
    }
    const DENOISE_25: &str = "n5000pY/cq'KcVfG'+7/!ZTkTS.KRK4Vzw91n@]ow}y^8.l]-Lr*jLtj!J|Xw8U]Aaif/EP2M)enDg5(/uI05F}^odCGiBhbn]%kts6QKHn'%0[CEbje6(/5hX:DNXvLr:[?VY:xf'T+.YV6.+SGn]@Ek@2$*yZh5SoQ*@9)D?LjOeBYa%S{#uxn-s^[SsL.1]q3=o9e)mrP4/NuQreO.`yOFYa?%dv?5rxEG=Ym|*P9mY}gGkIDhCGXnoY'xmd#GC+jRIIdt)7uh3vKrZLKn$=hjRk(-rVp4[|uGG.q}8gEKS5'[TJhfzO7yKoTqM4aLtvCS4FJ4NaE!v/L3n:pXgXg:H-a?8CbDCoLbNyiS(mNIUuv0Vux2";
    const DENOISE_50: &str = "n5000pY/cquVcVffup(T[k}as/H+T2Yttj4-p(NGE{9TVjT]|osF/bt%r%UblZt.U[G3wO:M|ePG#z7'`-7|/`1y`goF:3ufXr8G$ROCv|ed]3z2@V@b`/Moy`c#m3O)kVyC[IqrR'ir=AdrQSzz.LrD%L6ag4+^t79$R*a{37VKHvDy%-(t6I:Or@=@hV*TGSApKx!2U(%jJfygtB{{m89ZVNNr(Uuxv:GeD*:SD|bLsTYMlJKVluiW2tRyNX^^ij=SzPFks8D`jc^HG-pa{K`S{E+`Sl7umBWxkvl-W%X}ozm1)V]bRIq.O$T.CC{}peAZWo'wIY+K0FW(*{$|vhfB(:(RtHMv}@`Ogj36gKA0c5vA/G+0";
    const DENOISE_100: &str = "o5000pY/cqgVcVfV.v5/PunbM=h8GP%xck.z/`Hxn[v)Be315LYq{3]G-|Gn+9b+'iw@S{/u!dxxvw/CZ30f{Pv`hmY+HoyH^!j`!*L6+9tDRFfCWtPb.8639u6q-odS%BIin(nVfD9+$K%@V*Bwlg4^%I.+|jf*}gcR*#eUc.p^WO{)QH'Bg:%$Qc6Lot|J7o|1@y(pTLTl{HR=iUWp$0w@[wKrcI^!sMAO5tP{yg-C3?^^7iHiYVP?HeTaV+6kJe#i*YD8ovr%43hzJ0r*LzM!a-Qq*EqJHzCMZYA3MZ^?zV7G@cR6AisfXhz0'A4QbigDRM[GtN^*AFWdfjUS*l1ZCtqM@mEl$EAr8Q13Xa3cLMf{-Gp1";
}
