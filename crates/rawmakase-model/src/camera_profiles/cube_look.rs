//! A `.cube` as a creative look profile, as Camera Raw's Create Profile makes one: the
//! cube becomes the look's RGB table (`RgbTable::from_cube`), and the look is written
//! into the profile library as an ordinary look XMP, which then lists, renders, travels
//! in recipes and presets and takes a Profile Amount like any other. How faithfully the
//! table holds the cube is measured at import ([`Fidelity`]). The import says which
//! colour space the cube expects ([`LutSpace`]), as Create Profile asks.
//!
//! The look is named after the file: the name is what RAWmakase lists, dedupes and
//! resolves looks by, so the import owns collisions ([`Names`]) and a cube's `TITLE`,
//! which packs often repeat across files, is only its description.
use super::{
    cube::Cube,
    rgb_table::{RgbTable, Tail},
};
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

/// The colour space a `.cube` expects its colours in, chosen at import as Camera Raw's
/// Create Profile asks for it. The cube's values become the table's samples unchanged,
/// read as values encoded in this space: the space changes only the primaries and the
/// encoding the table names (Camera Raw 18.7 converts nothing either; see
/// docs/lightroom-profiles.md#camera-raws-create-profile). The one owner of each space's
/// table codes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LutSpace {
    /// sRGB primaries and encoding: Create Profile's default, and what most LUTs expect.
    #[default]
    Srgb,
    /// Adobe RGB (1998) primaries, gamma 2.2.
    AdobeRgb,
    /// Display P3 primaries, sRGB encoding.
    DisplayP3,
    /// ProPhoto RGB primaries, gamma 1.8.
    ProPhotoRgb,
    /// Rec. 2020 primaries and encoding.
    Rec2020,
}
impl LutSpace {
    /// In Create Profile's order.
    pub const ALL: [Self; 5] = [
        Self::Srgb,
        Self::AdobeRgb,
        Self::DisplayP3,
        Self::ProPhotoRgb,
        Self::Rec2020,
    ];
    /// The space's name, as Camera Raw and Photoshop call it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Srgb => "sRGB",
            Self::AdobeRgb => "Adobe RGB (1998)",
            Self::DisplayP3 => "Display P3",
            Self::ProPhotoRgb => "ProPhoto RGB",
            Self::Rec2020 => "Rec. 2020",
        }
    }
    /// The table's primaries, encoding and gamut codes and its amount bounds, as Create
    /// Profile writes them for this space: colours outside it clipped before the lookup,
    /// at amounts 0–2, in every space.
    pub(super) fn tail(self) -> Tail {
        let (primaries, encoding) = match self {
            Self::Srgb => (0, 1),
            Self::AdobeRgb => (1, 3),
            Self::DisplayP3 => (3, 1),
            Self::ProPhotoRgb => (2, 2),
            Self::Rec2020 => (4, 4),
        };
        (primaries, encoding, 0, [0., 2.])
    }
}
impl std::fmt::Display for LutSpace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// How faithfully a cube's look holds the cube, measured on a dense sample of inputs:
/// the cube's nodes and the centres of its cells and of the table's.
#[derive(Clone, Debug, PartialEq)]
pub enum Fidelity {
    /// Within 16-bit rounding everywhere.
    Exact,
    /// Approximation with a sampled maximum of `max_levels` 8-bit levels (of 255), because
    /// the cube was resampled from `resampled` divisions, its 1D shaper was baked into
    /// the table (`shaper`), its outputs outside 0–1 were clamped, or several of these.
    Approximated {
        max_levels: f32,
        resampled: Option<usize>,
        shaper: bool,
        clamped: bool,
    },
}
impl std::fmt::Display for Fidelity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exact => write!(f, "exact"),
            Self::Approximated {
                max_levels,
                resampled,
                shaper,
                clamped,
            } => {
                write!(
                    f,
                    "approximated, sampled error {max_levels:.1} 8-bit levels"
                )?;
                let mut why = Vec::new();
                if let Some(size) = resampled {
                    why.push(format!("resampled from {size} divisions"));
                }
                if *shaper {
                    why.push("1D shaper baked in".into());
                }
                if *clamped {
                    why.push("outputs outside 0–1 clamped".into());
                }
                if !why.is_empty() {
                    write!(f, " ({})", why.join("; "))?;
                }
                Ok(())
            }
        }
    }
}

/// A cube converted, not yet named.
pub(super) struct Converted {
    /// The table's id: the MD5 of its bytes, which also names the look (`crs:UUID`).
    /// The bytes name the space, so the same cube in two spaces is two looks.
    pub uuid: String,
    /// The table in Adobe's encoding.
    table: String,
    /// The cube's `TITLE`.
    title: Option<String>,
    pub fidelity: Fidelity,
}

/// The most a `.cube` file may hold: a 65³ cube is about 8 MB of text.
pub(super) const MAX_BYTES: u64 = 64_000_000;

/// Reads and converts a cube expecting colours in `space`, or says why it cannot be
/// imported.
pub(super) fn convert(bytes: &[u8], space: LutSpace) -> Result<Converted> {
    let text = std::str::from_utf8(bytes).map_err(|_| {
        anyhow::anyhow!("not a .cube file: it is not text (a macOS ._ file, perhaps)")
    })?;
    let cube = Cube::parse(text)?;
    let (table, table_bytes) = RgbTable::from_cube(&cube, space)?;
    let fidelity = fidelity(&cube, &table);
    let uuid = {
        use md5::{Digest, Md5};
        Md5::digest(&table_bytes)
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect()
    };
    Ok(Converted {
        uuid,
        table: super::enhanced::compress(&table_bytes),
        title: super::cube::title_of(text).filter(|t| !t.trim().is_empty()),
        fidelity,
    })
}

/// How far the table is from the cube, measured in table space: the same in every
/// space, as the samples are.
fn fidelity(cube: &Cube, table: &RgbTable) -> Fidelity {
    let divisions = table.divisions();
    let shaper = matches!(cube, Cube::Three(lut) if lut.has_shaper());
    let (size, clamped) = match cube {
        Cube::Three(lut) => {
            let n = lut.size();
            let outside = (0..n.pow(3))
                .map(|i| lut.node(i % n, i / n % n, i / (n * n)))
                .any(|v| v.iter().any(|c| !(0. ..=1.).contains(c)));
            (n, outside)
        }
        Cube::One(lut) => {
            let outside = (0..lut.size())
                .map(|i| lut.entry(i))
                .any(|v| v.iter().any(|c| !(0. ..=1.).contains(c)));
            (lut.size(), outside)
        }
    };
    let source = |p: [f32; 3]| match cube {
        Cube::Three(lut) => lut.lookup(p),
        Cube::One(lut) => lut.lookup(p),
    };
    let error = |p: [f32; 3]| {
        let (want, got) = (source(p), table.lookup(p));
        (0..3)
            .map(|c| (want[c] - got[c]).abs())
            .fold(0f32, f32::max)
    };
    // A grid's nodes and the centres of its cells along one axis.
    let points = |n: usize| -> Vec<f32> {
        let last = (n - 1) as f32;
        (0..2 * n - 1).map(|i| i as f32 / 2. / last).collect()
    };
    // The cube's own grid, and the table's where it was resampled.
    let mut grids = vec![points(size)];
    if divisions != size {
        grids.push(points(divisions));
    }
    let mut worst = match cube {
        // A 1D table's channels are independent, so the gray axis tries each curve.
        Cube::One(_) => grids
            .iter()
            .flatten()
            .map(|&v| error([v; 3]))
            .fold(0f32, f32::max),
        Cube::Three(_) => {
            use rayon::prelude::*;
            grids
                .iter()
                .map(|axis| {
                    axis.par_iter()
                        .map(|&r| {
                            let mut worst = 0f32;
                            for &g in axis {
                                for &b in axis {
                                    worst = worst.max(error([r, g, b]));
                                }
                            }
                            worst
                        })
                        .reduce(|| 0., f32::max)
                })
                .fold(0f32, f32::max)
        }
    };
    // A shaper can have narrow features between all of the 3D probe points.
    // Include its own breakpoints along the gray axis, but do not infer exactness:
    // cross-channel features may still fall between the sampled inputs.
    if let Cube::Three(lut) = cube
        && let Some(size) = lut.shaper_size()
    {
        for v in points(size) {
            worst = worst.max(error([v; 3]));
        }
    }
    // 16-bit rounding is half a step; a step allows for the arithmetic around it.
    if !shaper && worst <= 1. / 65535. {
        Fidelity::Exact
    } else {
        Fidelity::Approximated {
            max_levels: worst * 255.,
            // A shaped cube is always sampled anew; its own grid is not the table's.
            resampled: (divisions != size && !shaper).then_some(size),
            shaper,
            clamped,
        }
    }
}

/// The look profile XMP for a converted cube named `name`, in `group`: written the
/// same way every time, as `scripts/corpus/synthetic-looks.py` writes its looks. It
/// names no base, so it goes over the photo's own profile (Adobe Standard when
/// imported), as in Camera Raw.
pub(super) fn look_xmp(converted: &Converted, name: &str, group: &str) -> String {
    let id = &converted.uuid;
    let mut xml = String::from(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n \
         <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
         <rdf:Description rdf:about=\"\"\n    \
         xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"",
    );
    for (key, value) in [
        ("PresetType", "Look"),
        ("Cluster", ""),
        ("UUID", id),
        ("SupportsAmount", "True"),
        ("SupportsColor", "True"),
        ("SupportsMonochrome", "True"),
        ("SupportsHighDynamicRange", "True"),
        ("SupportsNormalDynamicRange", "True"),
        ("SupportsSceneReferred", "True"),
        ("SupportsOutputReferred", "True"),
        ("CameraModelRestriction", ""),
        ("Copyright", ""),
        ("ContactInfo", ""),
        ("Version", "18.7"),
        ("ProcessVersion", "15.4"),
        ("ConvertToGrayscale", "False"),
        ("RGBTable", id),
    ] {
        xml += &format!("\n   crs:{key}=\"{value}\"");
    }
    xml += &format!("\n   crs:Table_{id}=\"{}\">\n", converted.table);
    let title = converted.title.as_deref().unwrap_or("");
    for (element, text) in [("Name", name), ("Group", group), ("Description", title)] {
        if text.is_empty() {
            continue;
        }
        xml += &format!(
            "   <crs:{element}>\n    <rdf:Alt>\n     \
             <rdf:li xml:lang=\"x-default\">{}</rdf:li>\n    </rdf:Alt>\n   </crs:{element}>\n",
            crate::xml::escape_text(text)
        );
    }
    xml + "  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n"
}

/// The profile names taken in the library, with the looks' UUIDs and files, so an
/// imported cube neither shadows nor is shadowed by a profile of the same name.
#[derive(Default)]
pub(super) struct Names {
    /// Under each name, the UUID of each look listed by it and its file; a DCP or a
    /// built-in profile has neither.
    taken: HashMap<String, Vec<Option<(String, PathBuf)>>>,
    /// The library's file names, lowercased, as file systems may ignore case.
    files: Vec<String>,
}
/// Where a cube's look goes.
pub(super) enum Placement {
    /// A new look under this name.
    New(String),
    /// The library already has it, under this name, in this file.
    Already(String, PathBuf),
}
impl Names {
    /// `name` taken by a profile that is not a look.
    pub(super) fn profile(&mut self, name: &str) {
        self.taken.entry(name.into()).or_default().push(None);
    }
    /// `name` taken by the look `uuid` in `file`.
    pub(super) fn look(&mut self, name: &str, uuid: &str, file: &Path) {
        self.taken
            .entry(name.into())
            .or_default()
            .push(Some((uuid.into(), file.into())));
    }
    /// A file name the library holds.
    pub(super) fn file(&mut self, file: &str) {
        self.files.push(file.to_lowercase());
    }
    /// The name a cube from `stem` in `group` lists under: its stem, or when a
    /// different profile has that name, "stem (group)", then "stem (group 2)" and so
    /// on. A name of the stem's already holding this cube's look, whichever folder
    /// named it, means it is imported already.
    pub(super) fn place(&self, stem: &str, group: &str, uuid: &str) -> Result<Placement> {
        let family = format!("{stem} (");
        let imported = self
            .taken
            .iter()
            .filter(|(name, _)| *name == stem || name.starts_with(&family))
            .filter_map(|(name, looks)| {
                let (_, file) = looks
                    .iter()
                    .flatten()
                    .find(|(id, _)| id.eq_ignore_ascii_case(uuid))?;
                Some((name, file))
            })
            .min();
        if let Some((name, file)) = imported {
            return Ok(Placement::Already(name.clone(), file.clone()));
        }
        let candidate = |n: usize| match (n, group.is_empty()) {
            (0, _) => stem.to_string(),
            (1, false) => format!("{stem} ({group})"),
            (n, false) => format!("{stem} ({group} {n})"),
            (n, true) => format!("{stem} ({})", n + 1),
        };
        for n in 0..10_000 {
            let name = candidate(n);
            if !self.taken.contains_key(&name) {
                return Ok(Placement::New(name));
            }
        }
        bail!("too many profiles named {stem}")
    }
    /// A file name for the look `name` that the library does not hold yet.
    pub(super) fn file_for(&self, name: &str) -> String {
        // Characters some file systems refuse, and a leading dot, which hides it.
        let safe: String = name
            .chars()
            .map(|c| match c {
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
                c if c.is_control() => '_',
                c => c,
            })
            .collect();
        let safe = safe.trim().trim_start_matches('.').trim_end_matches('.');
        let safe = if safe.is_empty() { "Look" } else { safe };
        (1..)
            .map(|n| match n {
                1 => format!("{safe}.xmp"),
                n => format!("{safe} {n}.xmp"),
            })
            .find(|file| !self.files.contains(&file.to_lowercase()))
            .expect("an unused file name")
    }
}

/// A cube's file stem and its folder (see [`group_of`]), which name and group its
/// look, as the XMP holds them: control characters, which XML drops or rewrites,
/// become spaces, so the name checked for collisions is the name stored.
pub(super) fn stem_and_group(path: &Path) -> Result<(String, String)> {
    let clean = |name: &std::ffi::OsStr| {
        name.to_string_lossy()
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect::<String>()
            .trim()
            .to_string()
    };
    let stem = clean(path.file_stem().context("Missing profile filename")?);
    ensure!(!stem.is_empty(), "Missing profile filename");
    let folders: Vec<String> = path
        .ancestors()
        .skip(1)
        .map_while(Path::file_name)
        .map(clean)
        .collect();
    Ok((stem, group_of(&folders)))
}

/// The group of a cube in `folders`, its folder first, then that folder's and so
/// on: the nearest whose name says more than that it holds cubes ("Cube Files",
/// "LUTs", "For Other Programs", as LUT packs name the folder of each format), up
/// to four folders up. A cube in only such folders, or whose search reaches a
/// standard folder ("Downloads", "Desktop") or a user's home folder (a folder in
/// "Users", "home" or Linux's "media" at the top of the path) first, takes its own
/// folder's name, and one right in a home folder none, so a group never names the
/// user. A drive's or disk image's name can be one: packs ship as disk images
/// named after them. Only the path decides, so the same file always gets the same
/// group.
pub(super) fn group_of(folders: &[String]) -> String {
    const GENERIC: &[&str] = &[
        "1d",
        "3d",
        "3dl",
        "and",
        "app",
        "apps",
        "application",
        "applications",
        "cube",
        "cubes",
        "export",
        "file",
        "files",
        "for",
        "format",
        "formats",
        "lut",
        "luts",
        "macosx",
        "other",
        "others",
        "program",
        "programs",
        "software",
    ];
    const STANDARD: &[&str] = &[
        "desktop",
        "documents",
        "downloads",
        "home",
        "movies",
        "pictures",
        "tmp",
        "users",
        "volumes",
    ];
    let words = |name: &str| -> Vec<String> {
        name.split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_lowercase)
            .collect()
    };
    let generic = |name: &str| words(name).iter().all(|w| GENERIC.contains(&w.as_str()));
    let own = folders.first().cloned().unwrap_or_default();
    let standard = |name: &String| STANDARD.contains(&name.to_lowercase().as_str());
    for (i, name) in folders.iter().enumerate().take(4) {
        // A home folder is in "Users" or "home" at the top of the path, or in
        // "media" or "run/media" there; a folder of those names deeper is not.
        let above: Vec<String> = folders[i + 1..].iter().map(|f| f.to_lowercase()).collect();
        let above: Vec<&str> = above.iter().map(String::as_str).collect();
        let home = matches!(
            above.as_slice(),
            ["users"] | ["home"] | ["media"] | ["media", "run"]
        );
        if home && i == 0 {
            return String::new();
        }
        if standard(name) || home {
            break;
        }
        if !generic(name) {
            return name.clone();
        }
    }
    own
}

#[cfg(test)]
pub(super) mod tests {
    use super::super::enhanced::LookFile;
    use super::*;

    /// A cube of `size` per side whose node `(r, g, b)` is `f` of its position, red
    /// index fastest as the specification stores them.
    pub(crate) fn cube_text(size: usize, f: impl Fn([f32; 3]) -> [f32; 3]) -> String {
        let last = (size - 1) as f32;
        let mut text = format!("TITLE \"Test cube\"\nLUT_3D_SIZE {size}\n");
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    let [x, y, z] = f([r as f32 / last, g as f32 / last, b as f32 / last]);
                    text += &format!("{x:.6} {y:.6} {z:.6}\n");
                }
            }
        }
        text
    }
    /// A sharp creative grade: hue swaps and a posterized step, so resampling shows.
    fn sharp([r, g, b]: [f32; 3]) -> [f32; 3] {
        [
            (0.1 + 0.8 * g + 0.15 * (12. * r).sin()).clamp(0., 1.),
            if b > 0.47 { 0.9 * r } else { 0.2 + 0.5 * r },
            (0.95 * b * b + 0.05 * (9. * g).cos().abs()).clamp(0., 1.),
        ]
    }
    fn converted_table(text: &str) -> (RgbTable, Converted) {
        let converted = convert(text.as_bytes(), LutSpace::Srgb).unwrap();
        (RgbTable::decode(&converted.table).unwrap(), converted)
    }

    /// The table keeps the cube's grid, node for node, with red outermost where the
    /// cube has red fastest: an asymmetric cube catches the order.
    #[test]
    fn a_cubes_nodes_become_the_tables_in_its_order() {
        let f = |[r, g, b]: [f32; 3]| [r, 0.5 * g, 0.25 + 0.5 * b * r];
        let (table, converted) = converted_table(&cube_text(5, f));
        assert_eq!(table.divisions(), 5);
        assert_eq!(converted.fidelity, Fidelity::Exact);
        let samples = table.samples();
        for (r, g, b) in [(4, 0, 0), (0, 4, 0), (0, 0, 4), (1, 2, 3), (4, 4, 1)] {
            let want = f([r, g, b].map(|i| i as f32 / 4.));
            let got = samples[(r * 5 + g) * 5 + b];
            for c in 0..3 {
                assert!((got[c] - want[c]).abs() <= 0.5 / 65535. + 1e-6, "{r}{g}{b}");
            }
        }
    }

    /// Sizes up to 64 keep their grid, 33 included, which a table of 32 would cost
    /// whole 8-bit levels on a sharp cube; the table is exact.
    #[test]
    fn sizes_up_to_64_are_never_resampled() {
        for size in [2, 17, 32, 33] {
            let (table, converted) = converted_table(&cube_text(size, sharp));
            assert_eq!(table.divisions(), size);
            assert_eq!(converted.fidelity, Fidelity::Exact, "{size}");
        }
    }

    /// A 65³ cube is resampled onto 64 divisions by tetrahedral lookup in the cube,
    /// so a node the two grids share (the corners) comes back exact, and the report
    /// says how far off the rest is.
    #[test]
    fn a_65_cube_is_resampled_to_64_and_reported() {
        let (table, converted) = converted_table(&cube_text(65, sharp));
        assert_eq!(table.divisions(), 64);
        let samples = table.samples();
        let corner = |r: usize, g: usize, b: usize| samples[(r * 64 + g) * 64 + b];
        for (node, at) in [
            ((0, 0, 0), [0.; 3]),
            ((63, 63, 63), [1.; 3]),
            ((63, 0, 63), [1., 0., 1.]),
        ] {
            let want = sharp(at);
            let got = corner(node.0, node.1, node.2);
            for c in 0..3 {
                assert!(
                    (got[c] - want[c]).abs() < 1e-5,
                    "{node:?}: {got:?} {want:?}"
                );
            }
        }
        let Fidelity::Approximated {
            max_levels,
            resampled,
            shaper: false,
            clamped,
        } = converted.fidelity
        else {
            panic!("{:?}", converted.fidelity);
        };
        assert_eq!((resampled, clamped), (Some(65), false));
        assert!(max_levels > 0.01 && max_levels < 40., "{max_levels}");
        // A smooth cube survives resampling nearly untouched.
        let smooth = |[r, g, b]: [f32; 3]| [r * 0.9, g, 0.1 + 0.8 * b];
        assert_eq!(table_fidelity(&cube_text(65, smooth)), Fidelity::Exact);
    }
    fn table_fidelity(text: &str) -> Fidelity {
        convert(text.as_bytes(), LutSpace::Srgb).unwrap().fidelity
    }

    /// Outputs outside 0–1 do not fit the table and are clamped, and the report says so.
    #[test]
    fn clamped_outputs_are_reported() {
        let over = |[r, g, b]: [f32; 3]| [1.3 * r - 0.1, g, b];
        let Fidelity::Approximated {
            max_levels,
            resampled,
            shaper: false,
            clamped,
        } = table_fidelity(&cube_text(9, over))
        else {
            panic!("not approximated");
        };
        assert_eq!((resampled, clamped), (None, true));
        // 0.2 above 1 at white, in 8-bit levels.
        assert!((max_levels - 0.2 * 255.).abs() < 0.5, "{max_levels}");
    }

    /// 1D cubes map onto 1D tables; over 4096 entries they are resampled to 4096.
    #[test]
    fn a_1d_cube_becomes_a_1d_table() {
        let curve = |n: usize| {
            let mut text = format!("LUT_1D_SIZE {n}\n");
            for i in 0..n {
                let v = i as f32 / (n - 1) as f32;
                text += &format!("{:.7} {:.7} {:.7}\n", v.powf(0.8), v, 1. - v);
            }
            text
        };
        let (table, converted) = converted_table(&curve(1024));
        assert_eq!(
            (table.dimensions(), table.divisions()),
            (super::super::Dimensions::One, 1024)
        );
        assert_eq!(converted.fidelity, Fidelity::Exact);
        let (table, converted) = converted_table(&curve(8192));
        assert_eq!(table.divisions(), 4096);
        assert!(
            matches!(
                converted.fidelity,
                Fidelity::Exact
                    | Fidelity::Approximated {
                        resampled: Some(8192),
                        ..
                    }
            ),
            "{:?}",
            converted.fidelity
        );
    }

    #[test]
    fn a_shaper_feature_between_baking_nodes_is_not_reported_exact() {
        let mut text = String::from("LUT_1D_SIZE 1025\nLUT_3D_SIZE 2\n");
        for i in 0..1025 {
            text.push_str(if i == 1 { "1 1 1\n" } else { "0 0 0\n" });
        }
        text.push_str(cube_text(2, |p| p).split_once("LUT_3D_SIZE 2\n").unwrap().1);
        let (table, converted) = converted_table(&text);
        assert_eq!(table.lookup([1. / 1024.; 3]), [0.; 3]);
        assert!(
            matches!(converted.fidelity, Fidelity::Approximated { shaper: true, max_levels, .. } if max_levels == 255.)
        );
    }

    /// A 1D shaper in front of the 3D table is baked in: the table samples the shaper
    /// and the cube together on 64 divisions, and the report says so. (Camera Raw 18.7's
    /// Create Profile accepts such a file but drops the shaper; see
    /// docs/lightroom-profiles.md#cube-luts.)
    #[test]
    fn a_shaper_is_baked_into_64_divisions() {
        // A log-like shaper spending the 17 nodes on the shadows, as Resolve writes them.
        let curve = |v: f32| (1. + 15. * v).log2() / 4.;
        let mut text = String::from("TITLE \"Shaped\"\nLUT_1D_SIZE 256\nLUT_3D_SIZE 17\n");
        for i in 0..256 {
            let v = curve(i as f32 / 255.);
            text += &format!("{v:.6} {v:.6} {v:.6}\n");
        }
        text += cube_text(17, sharp)
            .split_once("LUT_3D_SIZE 17\n")
            .unwrap()
            .1;
        let cube = Cube::parse(&text).unwrap();
        assert!(matches!(&cube, Cube::Three(lut) if lut.has_shaper()));
        let (table, converted) = converted_table(&text);
        assert_eq!(table.divisions(), 64);
        let Fidelity::Approximated {
            max_levels,
            resampled,
            shaper,
            clamped,
        } = converted.fidelity
        else {
            panic!("{:?}", converted.fidelity);
        };
        assert_eq!((resampled, shaper, clamped), (None, true, false));
        assert!(max_levels > 0.01 && max_levels < 40., "{max_levels}");
        assert!(converted.fidelity.to_string().contains("shaper baked in"));
        // The table follows the shaped cube, not the 3D table read on its own.
        let Cube::Three(lut) = cube else {
            unreachable!()
        };
        let p = [0.1, 0.3, 0.6];
        let (got, want) = (table.lookup(p), lut.lookup(p));
        assert!(
            (0..3).all(|c| (got[c] - want[c]).abs() < 0.02),
            "{got:?} {want:?}"
        );
        assert!(
            (0..3).any(|c| (got[c] - sharp(p)[c]).abs() > 0.05),
            "{got:?}"
        );
    }

    /// What the table cannot hold is refused with the reason, not approximated.
    #[test]
    fn other_domains_and_non_text_are_refused() {
        let domain = cube_text(2, |p| p).replace(
            "LUT_3D_SIZE 2",
            "LUT_3D_SIZE 2\nDOMAIN_MAX 2 2 2\nDOMAIN_MIN 0 0 0",
        );
        let error = convert(domain.as_bytes(), LutSpace::Srgb)
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("domain"), "{error}");
        // A shaper reading another range than 0–1 is a cube for log or scene-linear
        // input, as a 3D domain other than 0–1 is.
        let mut shaped =
            String::from("LUT_1D_SIZE 2\nLUT_1D_INPUT_RANGE 0 2\nLUT_3D_SIZE 2\n0 0 0\n1 1 1\n");
        shaped += cube_text(2, |p| p).split_once("LUT_3D_SIZE 2\n").unwrap().1;
        let error = convert(shaped.as_bytes(), LutSpace::Srgb)
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("shaper reads"), "{error}");
        // A macOS AppleDouble file beside the cube, as zips carry them.
        let apple_double = b"\x00\x05\x16\x07\x00\x02\x00\x00Mac OS X        \xff\xfe";
        let error = convert(apple_double, LutSpace::Srgb)
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("not text"), "{error}");
    }

    /// The generated look is the same bytes every time, names its table by the table's
    /// MD5, carries the name, group and title as elements, and reads back as a look
    /// over any base with Profile Amount.
    #[test]
    fn the_generated_look_is_deterministic_and_reads_back() {
        let text = cube_text(9, sharp).replace("Test cube", "Warm & <Bright>");
        let a = convert(text.as_bytes(), LutSpace::Srgb).unwrap();
        let b = convert(text.as_bytes(), LutSpace::Srgb).unwrap();
        let xml = look_xmp(&a, "Portra", "Film");
        assert_eq!(xml, look_xmp(&b, "Portra", "Film"));
        assert_eq!(a.uuid.len(), 32);
        assert!(xml.contains(&format!("crs:RGBTable=\"{}\"", a.uuid)));
        assert!(xml.contains(&format!("crs:Table_{}=", a.uuid)));
        assert!(xml.contains(&format!("crs:UUID=\"{}\"", a.uuid)));
        // Offered for B&W and for rendered (JPEG, TIFF) photos in Lightroom, as Camera
        // Raw 18.7's Create Profile marks the looks it makes from cubes.
        assert!(xml.contains("crs:SupportsMonochrome=\"True\""), "{xml}");
        assert!(xml.contains("crs:SupportsOutputReferred=\"True\""), "{xml}");
        assert!(xml.contains("Warm &amp; &lt;Bright&gt;</rdf:li>"));
        let look = LookFile::parse(&xml).unwrap();
        assert_eq!(look.name, "Portra");
        assert!(matches!(look.base, super::super::enhanced::LookBase::Any));
        assert_eq!(LookFile::identity(&xml).unwrap(), ("Portra".into(), a.uuid));
        // Without a title or a folder, those elements are left out.
        let plain = convert(
            cube_text(2, |p| p)
                .replace("TITLE \"Test cube\"\n", "")
                .as_bytes(),
            LutSpace::Srgb,
        )
        .unwrap();
        let xml = look_xmp(&plain, "Plain", "");
        assert!(!xml.contains("crs:Group") && !xml.contains("crs:Description"));
        assert!(LookFile::parse(&xml).is_ok());
    }

    /// An sRGB import is the same bytes, and so the same UUID, as before the space
    /// could be chosen (taken on main at b553df67): a cube imported by an earlier
    /// release imports again as already imported, not as a second look.
    #[test]
    fn srgb_imports_keep_their_uuids() {
        let three = convert(cube_text(9, sharp).as_bytes(), LutSpace::Srgb).unwrap();
        let mut text = String::from("LUT_1D_SIZE 16\n");
        for i in 0..16 {
            let v = i as f32 / 15.;
            text += &format!("{:.6} {:.6} {:.6}\n", v.powf(0.8), v, 1. - v);
        }
        let one = convert(text.as_bytes(), LutSpace::Srgb).unwrap();
        assert_eq!(three.uuid, "2C6D97DE4C1FFCCC069D018C727397D9");
        assert_eq!(one.uuid, "531113C520AB3E707C1376AC03C74633");
        assert_eq!(LutSpace::default(), LutSpace::Srgb);
    }

    /// The space is in the table's bytes, so the same cube in each space is its own
    /// look; how faithfully the table holds the cube is the same in every space, an
    /// exact cube and a resampled one alike.
    #[test]
    fn each_space_is_its_own_look_with_the_same_fidelity() {
        for (size, exact) in [(17, true), (65, false)] {
            let text = cube_text(size, sharp);
            let all: Vec<Converted> = LutSpace::ALL
                .iter()
                .map(|&space| convert(text.as_bytes(), space).unwrap())
                .collect();
            for (i, a) in all.iter().enumerate() {
                assert_eq!(a.fidelity, all[0].fidelity, "{size}");
                for b in &all[i + 1..] {
                    assert_ne!(a.uuid, b.uuid, "{size}");
                }
            }
            assert_eq!(all[0].fidelity == Fidelity::Exact, exact, "{size}");
        }
    }

    #[test]
    fn names_are_stored_as_checked() {
        let (stem, group) = stem_and_group(Path::new("/x/Pack\r1/A\u{1}B.cube")).unwrap();
        assert_eq!((stem.as_str(), group.as_str()), ("A B", "Pack 1"));
        let converted = convert(cube_text(2, |p| p).as_bytes(), LutSpace::Srgb).unwrap();
        let xml = look_xmp(&converted, &stem, &group);
        assert_eq!(LookFile::identity(&xml).unwrap().0, stem);
    }

    /// A cube's group is the nearest folder whose name says more than that it holds
    /// cubes, so a pack that keeps every look's `.cube` in "Cube Files" groups them
    /// by the pack's folder, and the group is read back from the look.
    #[test]
    fn a_cube_is_grouped_by_its_nearest_meaningful_folder() {
        let group = |path: &str| stem_and_group(Path::new(path)).unwrap().1;
        assert_eq!(group("/x/Film/Portra.cube"), "Film");
        for (path, want) in [
            (
                "/x/Pack Moody/For Other Programs/Cube Files/A.cube",
                "Pack Moody",
            ),
            (
                "/x/Pack Bold/For Other Software/CUBE Files/A.cube",
                "Pack Bold",
            ),
            ("/x/Pack/LUTs/A.cube", "Pack"),
            ("/x/Pack/3D_LUTs/cube/A.cube", "Pack"),
            ("/x/2. Gold 200 LUT/A.cube", "2. Gold 200 LUT"),
            // Four folders at most: past them, the cube's own folder.
            ("/x/Pack/luts/cubes/files/lut/A.cube", "lut"),
            // A standard folder ends the search, rather than naming the user.
            ("/home/me/Downloads/LUTs/A.cube", "LUTs"),
            ("/home/me/Downloads/A.cube", "Downloads"),
            ("/Users/me/LUTs/A.cube", "LUTs"),
            ("/home/me/Cube Files/A.cube", "Cube Files"),
            ("/Volumes/T5/Pack/LUTs/A.cube", "Pack"),
            ("/Users/me/Pack/A.cube", "Pack"),
            // Right in a home folder: no group, rather than the user's name.
            ("/Users/me/A.cube", ""),
            ("/home/me/A.cube", ""),
            ("/media/me/A.cube", ""),
            // A disk image or drive is named after what it holds.
            (
                "/Volumes/Moody Pack/For Other Programs/Cube Files/A.cube",
                "Moody Pack",
            ),
            ("/Volumes/T5/LUTs/A.cube", "T5"),
            ("/media/me/T5/LUTs/A.cube", "T5"),
            ("/run/media/me/T5/A.cube", "T5"),
            // Folders named like those deeper in the path are ordinary ones.
            ("/Users/me/Media/Film Emulation/A.cube", "Film Emulation"),
            ("/Users/me/Media/Pack/Cube Files/A.cube", "Pack"),
            ("/Volumes/T5/Home/Pack/A.cube", "Pack"),
            ("/LUTs/A.cube", "LUTs"),
            ("A.cube", ""),
        ] {
            assert_eq!(group(path), want, "{path}");
        }
        let converted = convert(cube_text(2, |p| p).as_bytes(), LutSpace::Srgb).unwrap();
        let xml = look_xmp(&converted, "A", &group("/x/Pack/Cube Files/A.cube"));
        assert_eq!(LookFile::parse(&xml).unwrap().group, "Pack");
    }

    #[test]
    fn names_are_kept_renamed_or_found_imported() {
        let mut names = Names::default();
        names.profile("RAWmakase Standard");
        names.look("Portra", "AAAA", Path::new("/lib/Portra.xmp"));
        names.look("Portra (Cool)", "BBBB", Path::new("/lib/Cool.xmp"));
        let new = |p: Placement| match p {
            Placement::New(name) => name,
            Placement::Already(name, _) => panic!("{name} imported already"),
        };
        assert_eq!(new(names.place("Fuji", "Cool", "CCCC").unwrap()), "Fuji");
        assert_eq!(
            new(names.place("Portra", "Cool", "CCCC").unwrap()),
            "Portra (Cool 2)"
        );
        assert_eq!(
            new(names.place("Portra", "Warm", "CCCC").unwrap()),
            "Portra (Warm)"
        );
        assert_eq!(
            new(names.place("RAWmakase Standard", "", "CCCC").unwrap()),
            "RAWmakase Standard (2)"
        );
        // The same table under the name is the same look, whatever the case of its id.
        let Placement::Already(name, file) = names.place("Portra", "Elsewhere", "aaaa").unwrap()
        else {
            panic!("not found");
        };
        assert_eq!(
            (name.as_str(), file),
            ("Portra", PathBuf::from("/lib/Portra.xmp"))
        );
        assert!(matches!(
            names.place("Portra", "Cool", "BBBB").unwrap(),
            Placement::Already(name, _) if name == "Portra (Cool)"
        ));
        // Renamed once, it is found again from any folder, not imported twice.
        assert!(matches!(
            names.place("Portra", "Cool copy", "BBBB").unwrap(),
            Placement::Already(name, _) if name == "Portra (Cool)"
        ));
        // File names avoid the library's, whatever their case, and characters file
        // systems refuse.
        names.file("portra.XMP");
        assert_eq!(names.file_for("Portra"), "Portra 2.xmp");
        assert_eq!(names.file_for("a/b: c?"), "a_b_ c_.xmp");
        assert_eq!(names.file_for(".."), "Look.xmp");
    }
}
