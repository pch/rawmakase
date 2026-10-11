//! Adobe lens profiles (LCP, Adobe Camera Model) as data: their entries, parsed
//! from the file, an imported profile, and the profiles a photo can choose from.
//! Matching them to a photo and evaluating their correction is `lens::lcp`'s.
//!
//! Model: with x, y the offset from the image centre in units of FocalLengthX × the
//! long edge (FocalLength × SensorFormatFactor / 36 when not given), r² = x² + y²,
//! distortion maps ideal to observed radius by 1 + k1 r² + k2 r⁴ + k3 r⁶, vignetting
//! darkens by 1 + a1 r² + a2 r⁴ + a3 r⁶, and the red/blue chromatic models scale the
//! radius relative to green the same way, times their ScaleFactor.
use super::LensCorrection;
use crate::xml::ns::{RDF, ST_CAMERA};
use anyhow::{Context, Result, ensure};
use std::sync::{Arc, OnceLock};

/// A chromatic model: ScaleFactor and radial parameters.
pub(crate) type Chromatic = (f32, [f32; 3]);

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub make: String,
    pub lens: Vec<String>,
    /// The lens's display name (`LensPrettyName`, else `ProfileName`).
    pub name: String,
    /// `ProfileName`, the name Lightroom records as `LensProfileName`.
    pub profile_name: String,
    pub raw: bool,
    /// Lightroom uses the camera's own distortion data instead of the profile's.
    pub prefer_metadata_distortion: bool,
    pub focal: f32,
    /// APEX aperture value; f-number is 2^(av / 2).
    pub aperture: Option<f32>,
    pub distance: Option<f32>,
    pub(crate) sensor_factor: f32,
    pub(crate) focal_x: Option<f32>,
    pub(crate) distortion: Option<[f32; 3]>,
    pub(crate) vignette: Option<[f32; 3]>,
    pub(crate) red: Option<Chromatic>,
    pub(crate) blue: Option<Chromatic>,
}

fn attr(node: roxmltree::Node, name: &str) -> Option<String> {
    node.attribute((ST_CAMERA, name))
        .map(str::to_string)
        .or_else(|| {
            node.children()
                .find(|c| {
                    c.tag_name().namespace() == Some(ST_CAMERA) && c.tag_name().name() == name
                })
                .and_then(|c| c.text())
                .map(|t| t.trim().to_string())
        })
}
fn number(node: roxmltree::Node, name: &str) -> Option<f32> {
    attr(node, name)?
        .parse::<f32>()
        .ok()
        .filter(|v| v.is_finite())
}
/// The element itself when attributes carry the model, or its rdf:Description child.
fn model<'a>(node: roxmltree::Node<'a, 'a>, name: &str) -> Option<roxmltree::Node<'a, 'a>> {
    let m = node
        .children()
        .find(|c| c.tag_name().namespace() == Some(ST_CAMERA) && c.tag_name().name() == name)?;
    Some(
        m.children()
            .find(|c| c.tag_name().namespace() == Some(RDF) && c.tag_name().name() == "Description")
            .unwrap_or(m),
    )
}
fn params(node: roxmltree::Node, prefix: &str) -> Option<[f32; 3]> {
    Some([
        number(node, &format!("{prefix}1"))?,
        number(node, &format!("{prefix}2")).unwrap_or(0.),
        number(node, &format!("{prefix}3")).unwrap_or(0.),
    ])
}

pub fn parse(text: &str) -> Result<Vec<Entry>> {
    ensure!(text.len() <= 16_000_000, "Lens profile too large");
    let doc = roxmltree::Document::parse(text).context("Invalid lens profile XML")?;
    let mut out = Vec::new();
    for d in doc.descendants().filter(|n| {
        n.tag_name().namespace() == Some(RDF)
            && matches!(n.tag_name().name(), "Description" | "li")
            && attr(*n, "FocalLength").is_some()
    }) {
        let mut lens: Vec<String> = attr(d, "Lens").into_iter().collect();
        if let Some(alt) = d
            .children()
            .find(|c| c.tag_name().name() == "AlternateLensNames")
        {
            lens.extend(
                alt.descendants()
                    .filter(|n| n.tag_name().name() == "li")
                    .filter_map(|n| n.text().map(|t| t.trim().to_string())),
            );
        }
        let perspective = model(d, "PerspectiveModel");
        let chromatic = |name| {
            let m = perspective.and_then(|p| model(p, name))?;
            Some((
                number(m, "ScaleFactor").unwrap_or(1.),
                params(m, "RadialDistortParam")?,
            ))
        };
        out.push(Entry {
            make: attr(d, "Make").unwrap_or_default(),
            name: attr(d, "LensPrettyName")
                .or_else(|| attr(d, "ProfileName"))
                .unwrap_or_default(),
            profile_name: attr(d, "ProfileName").unwrap_or_default(),
            lens,
            raw: attr(d, "CameraRawProfile").is_some_and(|v| v.eq_ignore_ascii_case("true")),
            prefer_metadata_distortion: attr(d, "PreferMetadataDistort")
                .is_some_and(|v| v.eq_ignore_ascii_case("true")),
            focal: number(d, "FocalLength").context("Lens profile without FocalLength")?,
            aperture: number(d, "ApertureValue"),
            distance: number(d, "FocusDistance"),
            sensor_factor: number(d, "SensorFormatFactor").unwrap_or(1.),
            focal_x: perspective.and_then(|p| number(p, "FocalLengthX")),
            distortion: perspective.and_then(|p| params(p, "RadialDistortParam")),
            vignette: perspective
                .and_then(|p| model(p, "VignetteModel"))
                .and_then(|v| params(v, "VignetteModelParam")),
            red: chromatic("ChromaticRedGreenModel"),
            blue: chromatic("ChromaticBlueGreenModel"),
        });
    }
    ensure!(
        !out.is_empty() && out.iter().all(|e| e.focal > 0. && e.sensor_factor > 0.),
        "No usable lens profile entries"
    );
    Ok(out)
}

/// One imported LCP file, an item of Lightroom's Profile menu.
#[derive(Debug)]
pub struct ImportedProfile {
    /// The file's name as imported, Lightroom's `LensProfileFilename`.
    pub filename: String,
    /// `ProfileName`, Lightroom's `LensProfileName`, e.g. "Adobe (Sony FE 55mm F1.8 ZA)".
    pub name: String,
    /// The lens maker and model, Lightroom's Make and Model menus.
    pub lens_make: String,
    pub lens_model: String,
    pub(crate) entries: Vec<Entry>,
}
impl ImportedProfile {
    pub(crate) fn new(filename: String, entries: Vec<Entry>) -> Self {
        let first = |get: fn(&Entry) -> &str| {
            entries
                .iter()
                .map(get)
                .find(|v| !v.is_empty())
                .unwrap_or_default()
                .to_string()
        };
        let lens_model = Some(first(|e| &e.name))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| first(|e| e.lens.first().map_or("", String::as_str)));
        let name = Some(first(|e| &e.profile_name))
            .filter(|v| !v.is_empty())
            .or_else(|| Some(lens_model.clone()).filter(|v| !v.is_empty()))
            .unwrap_or_else(|| filename.trim_end_matches(".lcp").to_string());
        // Adobe's pretty names start with the lens maker ("Sigma 35mm F1.4 DG HSM
        // A013" on a Sony body); the profile's Make is the camera's.
        let lens_make = lens_model
            .split_whitespace()
            .next()
            .map(str::to_string)
            .unwrap_or_else(|| first(|e| &e.make));
        Self {
            filename,
            name,
            lens_make,
            lens_model,
            entries,
        }
    }
    /// Entries for raw files when the profile has them, as a raw photo uses.
    pub(crate) fn usable(&self) -> impl Iterator<Item = &Entry> {
        let raw = self.entries.iter().any(|e| e.raw);
        self.entries.iter().filter(move |e| e.raw || !raw)
    }
    pub(crate) fn has_raw(&self) -> bool {
        self.entries.iter().any(|e| e.raw)
    }
    /// Whether a recorded profile identity names this file: its file name, else its
    /// profile name when the identity has no file name.
    pub fn is(&self, filename: &str, name: &str) -> bool {
        if filename.is_empty() {
            !name.is_empty() && self.name == name
        } else {
            self.filename.eq_ignore_ascii_case(filename)
        }
    }
}

/// An imported profile as one photo can use it.
#[derive(Debug)]
pub struct Candidate {
    pub profile: Arc<ImportedProfile>,
    /// How well it fits the photo's lens, as `make_rank`; `None` when it profiles
    /// another lens.
    pub lens_rank: Option<u8>,
    /// Its correction for the photo, worked out once (see `lens::lcp`).
    pub(crate) correction: OnceLock<Option<LensCorrection>>,
}
/// The imported profiles that fit one photo's camera; rebuilt when it opens.
#[derive(Clone, Debug, Default)]
pub struct PhotoProfiles {
    /// In the order the files were read, which breaks ties in automatic matching.
    pub(crate) candidates: Arc<[Candidate]>,
    /// `candidates` indices by make, model, profile name and file name.
    pub(crate) menu: Arc<[usize]>,
}
impl PhotoProfiles {
    pub fn all(&self) -> &[Candidate] {
        &self.candidates
    }
    /// The profiles in menu order: by make, model, profile name and file name.
    pub fn in_menu_order(&self) -> impl Iterator<Item = &Candidate> {
        self.menu.iter().map(|&i| &self.candidates[i])
    }
    /// The profile a recorded identity names: by file name when it records one, else
    /// by profile name. A recorded file that isn't imported is not stood in for by
    /// another file of the same name.
    pub fn find(&self, filename: &str, name: &str) -> Option<&Candidate> {
        if filename.is_empty() {
            self.candidates.iter().find(|c| c.profile.is("", name))
        } else {
            self.candidates.iter().find(|c| c.profile.is(filename, ""))
        }
    }
}
