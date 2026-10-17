//! The camera fact tables: what a RAW file does not say but RAWmakase needs
//! to read it as it always has. `data/raw-cameras.toml` holds names (the
//! body a file's model string is, the canonical name of a body sold under
//! several) and decode facts (visible area, levels); `data/raw-matrices.toml`
//! the XYZ-to-camera matrices. Every row says where its values come from
//! ([`Source`]). Rendering facts (baseline exposure, unit calibration) are the
//! model's `data/cameras.toml`, keyed by the same names.
use serde::Deserialize;
use std::sync::OnceLock;

/// Where a row's values come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Source {
    /// A rule from the file's own tags, written down for files that lack them.
    File,
    /// Adobe DNG Converter's output for the camera.
    AdobeDng,
    /// RAWmakase's earlier decoder's output on our own files, kept so saved
    /// edits keep their geometry and scale; never a colour matrix.
    CompatObserved,
}

/// A model name as files write it, and the body RAWmakase names it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Name {
    pub make: String,
    pub model: String,
    pub is: String,
    // Read by the tests: every row says where its values come from.
    #[allow(dead_code)]
    pub source: Source,
}

/// The canonical make and model of a body sold under several names.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Canonical {
    pub make: String,
    pub model: String,
    pub canonical: [String; 2],
    #[allow(dead_code)]
    pub source: Source,
}

/// Decode facts of a camera, for its files that match `dng` and `raw_size`
/// when given.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Camera {
    pub make: String,
    pub model: String,
    #[serde(default)]
    pub dng: Option<bool>,
    #[serde(default)]
    pub raw_size: Option<[u32; 2]>,
    /// The sample bits of the files the row is for, when it is for some.
    #[serde(default)]
    pub bits: Option<u32>,
    /// The visible area: left, top, width, height in sensor coordinates.
    #[serde(default)]
    pub visible: Option<[u32; 4]>,
    /// The white level, and the black level of every channel, where the file
    /// does not give the ones RAWmakase has always read it with.
    #[serde(default)]
    pub white: Option<u32>,
    #[serde(default)]
    pub black: Option<u32>,
    /// The orientation (RAWmakase's flip code) and the default crop (`[left,
    /// top, width, height]` in sensor coordinates, `[65535, 65535, 0, 0]` for
    /// none) where the file does not give the ones RAWmakase has always read
    /// it with.
    #[serde(default)]
    pub flip: Option<i32>,
    #[serde(default)]
    pub frame: Option<[u32; 4]>,
    /// Factors (red, blue) the white balance levels are scaled by where
    /// RAWmakase has always read them scaled and the file does not say by
    /// how much (observed; applied only where the maker's rules say the
    /// levels are scaled).
    #[serde(default)]
    pub wb_scale: Option<[f64; 2]>,
    /// The 2 × 2 colour filter at the sensor's corner (0 red, 1 green, 2
    /// blue, row by row), where the file's is not the one RAWmakase has
    /// always read it with.
    #[serde(default)]
    pub cfa: Option<[u8; 4]>,
    /// The masked columns (start, end) the black level is measured on, where
    /// they are not the borders beside the visible area.
    #[serde(default)]
    pub black_columns: Option<[u32; 2]>,
    #[allow(dead_code)]
    pub source: Source,
    #[allow(dead_code)]
    #[serde(default)]
    pub note: Option<String>,
}

/// An XYZ (D65) to camera matrix, in ten-thousandths.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Matrix {
    pub make: String,
    pub model: String,
    pub xyz: [i32; 9],
    #[allow(dead_code)]
    pub source: Source,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cameras {
    #[serde(default)]
    name: Vec<Name>,
    #[serde(default)]
    canonical: Vec<Canonical>,
    #[serde(default)]
    camera: Vec<Camera>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Matrices {
    #[serde(default)]
    matrix: Vec<Matrix>,
}

const CAMERAS: &str = include_str!("../../../data/raw-cameras.toml");
const MATRICES: &str = include_str!("../../../data/raw-matrices.toml");

fn cameras() -> &'static Cameras {
    static TABLE: OnceLock<Cameras> = OnceLock::new();
    // The tables are compiled in and checked by the tests below.
    TABLE.get_or_init(|| toml::from_str(CAMERAS).unwrap_or_default())
}

fn matrices() -> &'static Matrices {
    static TABLE: OnceLock<Matrices> = OnceLock::new();
    TABLE.get_or_init(|| toml::from_str(MATRICES).unwrap_or_default())
}

/// The body a `make` file names `model` is.
pub(crate) fn name(make: &str, model: &str) -> Option<&'static str> {
    cameras()
        .name
        .iter()
        .find(|n| n.make == make && n.model == model)
        .map(|n| n.is.as_str())
}

/// The canonical make and model of a body sold under several names.
pub(crate) fn canonical(make: &str, model: &str) -> Option<&'static [String; 2]> {
    cameras()
        .canonical
        .iter()
        .find(|c| c.make == make && c.model == model)
        .map(|c| &c.canonical)
}

/// The decode facts for a file of this camera, DNG or not, of this stored
/// size: every row that matches, the first giving a fact being its source.
pub(crate) fn camera(
    make: &str,
    model: &str,
    dng: bool,
    raw_size: [u32; 2],
) -> impl Iterator<Item = &'static Camera> {
    cameras().camera.iter().filter(move |c| {
        c.make == make
            && c.model == model
            && c.dng.is_none_or(|d| d == dng)
            && c.raw_size.is_none_or(|s| s == raw_size)
    })
}

/// The camera's XYZ (D65) to camera matrix.
pub(crate) fn matrix(make: &str, model: &str) -> Option<[[f32; 3]; 3]> {
    let m = matrices()
        .matrix
        .iter()
        .find(|m| m.make == make && m.model == model)?;
    Some(std::array::from_fn(|r| {
        std::array::from_fn(|c| m.xyz[r * 3 + c] as f32 / 10000.)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The facts an `adobe-dng` row may give: levels (`white`, `black`) and
    /// the as-shot white balance scale (`wb_scale`); never geometry (visible
    /// area, default crop, orientation, colour filter).
    fn adobe_facts_only(c: &Camera) -> bool {
        let Camera {
            make: _,
            model: _,
            dng: _,
            raw_size: _,
            bits: _,
            white: _,
            black: _,
            wb_scale: _,
            black_columns: _,
            source: _,
            note: _,
            visible,
            frame,
            flip,
            cfa,
        } = c;
        visible.is_none() && frame.is_none() && flip.is_none() && cfa.is_none()
    }

    #[test]
    fn adobe_rows_give_levels_and_white_balance_never_geometry() {
        let row = |facts: &str| -> Camera {
            toml::from_str(&format!(
                "make = \"X\"\nmodel = \"Y\"\nsource = \"adobe-dng\"\n{facts}"
            ))
            .unwrap()
        };
        for allowed in ["white = 4000", "black = 64", "wb_scale = [0.96, 0.96]"] {
            assert!(adobe_facts_only(&row(allowed)), "{allowed}");
        }
        for geometry in [
            "visible = [0, 0, 8, 8]",
            "frame = [0, 0, 8, 8]",
            "flip = 6",
            "cfa = [0, 1, 1, 2]",
        ] {
            assert!(!adobe_facts_only(&row(geometry)), "{geometry}");
        }
    }
    #[test]
    fn tables_parse_and_have_no_duplicate_rows() {
        let cameras: Cameras = toml::from_str(CAMERAS).unwrap();
        let matrices: Matrices = toml::from_str(MATRICES).unwrap();
        let mut seen = std::collections::HashSet::new();
        for n in &cameras.name {
            assert!(seen.insert(("name", &n.make, &n.model)), "{n:?}");
            assert_ne!(n.source, Source::AdobeDng, "{n:?}");
        }
        for c in &cameras.canonical {
            assert!(seen.insert(("canonical", &c.make, &c.model)), "{c:?}");
        }
        let mut cams = std::collections::HashSet::new();
        for c in &cameras.camera {
            assert!(
                cams.insert((&c.make, &c.model, c.dng, c.raw_size, c.bits)),
                "{c:?}"
            );
            assert!(
                c.visible.is_some()
                    || c.white.is_some()
                    || c.black.is_some()
                    || c.flip.is_some()
                    || c.frame.is_some()
                    || c.wb_scale.is_some()
                    || c.cfa.is_some()
                    || c.black_columns.is_some(),
                "{c:?}"
            );
            // Adobe DNG Converter's decode facts where the file does not
            // tell them; never its geometry.
            if c.source == Source::AdobeDng {
                assert!(adobe_facts_only(c), "{c:?}");
            }
        }
        for m in &matrices.matrix {
            assert!(seen.insert(("matrix", &m.make, &m.model)), "{m:?}");
            // Colour comes from Adobe, never from an earlier decoder's output.
            assert_eq!(m.source, Source::AdobeDng, "{m:?}");
        }
    }
}
