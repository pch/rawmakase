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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Source {
    /// A rule from the file's own tags, written down for files that lack them.
    File,
    /// Adobe DNG Converter's output for the camera.
    AdobeDng,
    /// RAWmakase's earlier decoder's output on our own files, kept so saved
    /// edits keep their geometry and scale; a colour matrix only for a camera
    /// Adobe has none for.
    CompatObserved,
    /// Our own measurement of the files: a default crop aligned with the camera's
    /// embedded JPEGs.
    Measured,
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
    /// False where the camera's aspect-ratio crop is not shown (its maker
    /// notes give another image size's).
    #[serde(default)]
    pub aspect_crop: Option<bool>,
    /// The ISO speeds of the files whose `white` the row gives over the
    /// file's own (Adobe DNG Converter's per-ISO white levels); such rows
    /// give levels only and are found by [`iso_white`], not [`camera`].
    #[serde(default)]
    pub iso: Option<Iso>,
    /// The most the white level the file records is taken as (Adobe DNG
    /// Converter's cap).
    #[serde(default)]
    pub white_max: Option<u32>,
    /// Whether the white level is the one the maker notes record for
    /// normal exposures, not for specular highlights (Canon's
    /// NormalWhiteLevel, as Adobe DNG Converter takes it).
    #[serde(default)]
    pub normal_white: Option<bool>,
    #[allow(dead_code)]
    pub source: Source,
    #[allow(dead_code)]
    #[serde(default)]
    pub note: Option<String>,
}

/// The ISO speeds a row is for: one, or a range (inclusive).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub(crate) enum Iso {
    One(f32),
    Range([f32; 2]),
}

impl Iso {
    /// Whether a file of ISO speed `iso` is one of the row's (to the
    /// nearest whole speed, as files record them).
    pub(crate) fn contains(self, iso: f32) -> bool {
        match self {
            Iso::One(v) => (iso - v).abs() < 0.5,
            Iso::Range([lo, hi]) => iso > lo - 0.5 && iso < hi + 0.5,
        }
    }
}

/// A RAW file with no header (CHDK's), known by its size: the camera, the
/// stored image and its samples' bits, the visible area and the colour
/// filter at the sensor's corner (rows of R, G, B).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Headerless {
    pub file_size: u64,
    pub make: String,
    pub model: String,
    pub raw_size: [u32; 2],
    pub bits: u32,
    pub visible: [u32; 4],
    pub cfa: String,
    /// The masked columns (start, end) the black level is measured on, where
    /// they are not the borders beside the visible area.
    #[serde(default)]
    pub black_columns: Option<[u32; 2]>,
    /// The bytes of a packing word (1 for plain bit packing; 2, 16-bit
    /// little-endian words, when not given), and of a row when rows are
    /// padded.
    #[serde(default)]
    pub word: Option<usize>,
    #[serde(default)]
    pub row_bytes: Option<u32>,
    /// The rows stored as two fields: the even ones, then from the next
    /// 2 KiB boundary the odd ones.
    #[serde(default)]
    pub fields: Option<bool>,
    /// The bits each sample is shifted down by after unpacking.
    #[serde(default)]
    pub shift: Option<u32>,
    /// The black and white levels, where they are not measured on masked
    /// columns and the samples' range.
    #[serde(default)]
    pub black: Option<u32>,
    #[serde(default)]
    pub white: Option<u32>,
    /// The bytes before the samples (a header of the camera's).
    #[serde(default)]
    pub offset: Option<u64>,
    /// The value of each code: a curve of the row's, or one the file keeps
    /// as 2^bits little-endian 16-bit words at `curve_at`.
    #[serde(default)]
    pub curve: Option<Vec<u16>>,
    #[serde(default)]
    pub curve_at: Option<u64>,
    #[allow(dead_code)]
    pub source: Source,
}

impl Headerless {
    /// The bytes of one row.
    pub(crate) fn stride(&self) -> u64 {
        self.row_bytes.map_or_else(
            || (u64::from(self.raw_size[0]) * u64::from(self.bits)).div_ceil(8),
            u64::from,
        )
    }
}

/// An XYZ (D65) to camera matrix, in ten-thousandths.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Matrix {
    pub make: String,
    pub model: String,
    pub xyz: [i32; 9],
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
    #[serde(default)]
    headerless: Vec<Headerless>,
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

/// The headerless RAW file of `size` bytes, when the table knows one.
pub(crate) fn headerless(size: u64) -> Option<&'static Headerless> {
    cameras().headerless.iter().find(|h| h.file_size == size)
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
    rows(make, model, dng, raw_size).filter(|c| c.iso.is_none())
}

fn rows(
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

/// The white level a row gives a camera's files of ISO speed `iso` over the
/// file's own: a row for that one speed before one for a range.
pub(crate) fn iso_white(
    make: &str,
    model: &str,
    dng: bool,
    raw_size: [u32; 2],
    iso: f32,
) -> Option<u32> {
    let matching = || {
        rows(make, model, dng, raw_size)
            .filter(move |c| c.iso.is_some_and(|i| i.contains(iso)))
            .filter_map(|c| Some((c.iso?, c.white?)))
    };
    matching()
        .find(|(i, _)| matches!(i, Iso::One(_)))
        .or_else(|| matching().next())
        .map(|(_, w)| w)
}

/// The camera's XYZ (D65) to camera matrix.
pub(crate) fn matrix(make: &str, model: &str) -> Option<[[f32; 3]; 3]> {
    matrix_with_source(make, model).map(|(m, _)| m)
}

/// The camera's XYZ (D65) to camera matrix and where the table has it from.
pub(crate) fn matrix_with_source(
    make: &str,
    model: &str,
) -> Option<([[f32; 3]; 3], crate::info::MatrixSource)> {
    use crate::info::MatrixSource;
    let m = matrices()
        .matrix
        .iter()
        .find(|m| m.make == make && m.model == model)?;
    let source = match m.source {
        Source::File => MatrixSource::File,
        Source::AdobeDng => MatrixSource::AdobeDng,
        Source::CompatObserved | Source::Measured => MatrixSource::CompatObserved,
    };
    Some((
        std::array::from_fn(|r| std::array::from_fn(|c| m.xyz[r * 3 + c] as f32 / 10000.)),
        source,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The facts an `adobe-dng` row may give: levels (`white`, `black`, and
    /// how a file's recorded white is taken: `iso`, `white_max`,
    /// `normal_white`) and the as-shot white balance scale (`wb_scale`);
    /// never geometry (visible area, default crop, orientation, colour
    /// filter, aspect crop).
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
            iso: _,
            white_max: _,
            normal_white: _,
            source: _,
            note: _,
            visible,
            frame,
            flip,
            cfa,
            aspect_crop,
        } = c;
        visible.is_none()
            && frame.is_none()
            && flip.is_none()
            && cfa.is_none()
            && aspect_crop.is_none()
    }

    #[test]
    fn adobe_rows_give_levels_and_white_balance_never_geometry() {
        let row = |facts: &str| -> Camera {
            toml::from_str(&format!(
                "make = \"X\"\nmodel = \"Y\"\nsource = \"adobe-dng\"\n{facts}"
            ))
            .unwrap()
        };
        for allowed in [
            "white = 4000",
            "black = 64",
            "wb_scale = [0.96, 0.96]",
            "iso = 160\nwhite = 13100",
            "white_max = 15000",
            "normal_white = true",
        ] {
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

    /// Whether two inclusive ISO ranges share a speed.
    fn overlap(a: [f32; 2], b: [f32; 2]) -> bool {
        a[0] <= b[1] && b[0] <= a[1]
    }

    #[test]
    fn iso_ranges_overlap_when_they_share_a_speed() {
        assert!(overlap([100., 400.], [400., 800.]));
        assert!(overlap([1., 409600.], [160., 160.]));
        assert!(!overlap([100., 400.], [500., 800.]));
    }

    #[test]
    fn iso_rows_give_their_speeds_white_one_speed_first() {
        assert!(Iso::One(160.).contains(160.));
        assert!(!Iso::One(160.).contains(200.));
        assert!(Iso::Range([100., 400.]).contains(400.));
        assert!(!Iso::Range([100., 400.]).contains(500.));
        let white = |iso| iso_white("Canon", "EOS 6D", false, [5568, 3708], iso);
        // A speed of its own, else the row for every speed.
        assert_eq!(white(160.), Some(13100));
        assert_eq!(white(100.), Some(15000));
        assert_eq!(iso_white("Canon", "EOS 6D", true, [5568, 3708], 160.), None);
        // Rows by ISO are not the camera's other rows.
        assert!(camera("Canon", "EOS 6D", false, [5568, 3708]).all(|c| c.iso.is_none()));
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
        // One row per camera, files and source: an Adobe row's levels and an
        // earlier decoder's geometry may sit beside each other, never two
        // rows' levels.
        let mut cams = std::collections::HashSet::new();
        let mut levels = std::collections::HashSet::new();
        type Key<'a> = (
            &'a String,
            &'a String,
            Option<bool>,
            Option<[u32; 2]>,
            Option<u32>,
        );
        let mut ranges: Vec<(Key, [f32; 2])> = Vec::new();
        for c in &cameras.camera {
            assert!(
                cams.insert((
                    &c.make,
                    &c.model,
                    c.dng,
                    c.raw_size,
                    c.bits,
                    c.source,
                    format!("{:?}", c.iso)
                )),
                "{c:?}"
            );
            if c.white.is_some() || c.black.is_some() {
                assert!(
                    levels.insert((
                        &c.make,
                        &c.model,
                        c.dng,
                        c.raw_size,
                        c.bits,
                        format!("{:?}", c.iso)
                    )),
                    "{c:?}"
                );
            }
            // Rows by ISO give a white level, and nothing else; a camera's
            // ranges never overlap (one speed of its own may sit in one).
            if let Some(iso) = c.iso {
                assert!(c.white.is_some() && c.black.is_none(), "{c:?}");
                if let Iso::Range([lo, hi]) = iso {
                    assert!(lo <= hi, "{c:?}");
                    let key = (&c.make, &c.model, c.dng, c.raw_size, c.bits);
                    for &(k, [l, h]) in &ranges {
                        assert!(k != key || !overlap([l, h], [lo, hi]), "{c:?}");
                    }
                    ranges.push((key, [lo, hi]));
                }
            }
            assert!(
                c.visible.is_some()
                    || c.white.is_some()
                    || c.black.is_some()
                    || c.flip.is_some()
                    || c.frame.is_some()
                    || c.wb_scale.is_some()
                    || c.cfa.is_some()
                    || c.aspect_crop.is_some()
                    || c.white_max.is_some()
                    || c.normal_white.is_some(),
                "{c:?}"
            );
            // Adobe DNG Converter's decode facts where the file does not
            // tell them; never its geometry. The fields of how a file's white
            // is taken are Adobe's only.
            if c.iso.is_some() || c.white_max.is_some() || c.normal_white.is_some() {
                assert_eq!(c.source, Source::AdobeDng, "{c:?}");
            }
            if c.source == Source::AdobeDng {
                assert!(adobe_facts_only(c), "{c:?}");
            }
        }
        let mut sizes = std::collections::HashSet::new();
        for h in &cameras.headerless {
            assert!(sizes.insert(h.file_size), "{h:?}");
            let [w, ht] = h.raw_size;
            // The size names the file: every row, no more and no less, and a
            // row holds its samples.
            // Two fields, each starting on a 2 KiB boundary.
            let size = if h.fields == Some(true) {
                ((u64::from(ht).div_ceil(2) * h.stride()).next_multiple_of(2048)
                    + u64::from(ht) / 2 * h.stride())
                .next_multiple_of(2048)
            } else {
                u64::from(ht) * h.stride()
            };
            assert_eq!(size + h.offset.unwrap_or(0), h.file_size, "{h:?}");
            // A curve: a value per code, the file's within its header.
            let codes = 1u64 << h.bits;
            if let Some(c) = &h.curve {
                assert_eq!(c.len() as u64, codes, "{h:?}");
            }
            if let Some(at) = h.curve_at {
                assert!(
                    h.curve.is_none() && at + 2 * codes <= h.offset.unwrap_or(0),
                    "{h:?}"
                );
            }
            assert!(u64::from(w) * u64::from(h.bits) <= h.stride() * 8, "{h:?}");
            assert!((1..=16).contains(&h.bits), "{h:?}");
            assert!(matches!(h.word, None | Some(1 | 2 | 4)), "{h:?}");
            assert!(h.shift.unwrap_or(0) < h.bits, "{h:?}");
            assert!(
                w <= crate::io::MAX_SIDE && ht <= crate::io::MAX_SIDE,
                "{h:?}"
            );
            let [l, t, vw, vh] = h.visible;
            assert!(l + vw <= w && t + vh <= ht, "{h:?}");
            assert!(
                matches!(h.cfa.as_str(), "RGGB" | "BGGR" | "GRBG" | "GBRG"),
                "{h:?}"
            );
            assert_ne!(h.source, Source::AdobeDng, "{h:?}");
        }
        for m in &matrices.matrix {
            assert!(seen.insert(("matrix", &m.make, &m.model)), "{m:?}");
            // Colour comes from Adobe DNG Converter; an earlier decoder's
            // output only where Adobe has nothing, and never a matrix without
            // colour.
            assert_ne!(m.source, Source::File, "{m:?}");
            assert_ne!(m.source, Source::Measured, "{m:?}");
            assert!(m.xyz.iter().any(|v| *v != 0), "{m:?}");
        }
    }

    #[test]
    fn no_matrix_comes_from_adobes_camera_profiles() {
        // Adobe's camera profiles are not a source: such a row does not parse.
        let row = "[[matrix]]\nmake = \"A\"\nmodel = \"B\"\n\
                   xyz = [1, 0, 0, 0, 1, 0, 0, 0, 1]\nsource = \"adobe-dcp\"\n";
        assert!(toml::from_str::<Matrices>(row).is_err());
        let row = row.replace("adobe-dcp", "adobe-dng");
        assert!(toml::from_str::<Matrices>(&row).is_ok());
    }
}
