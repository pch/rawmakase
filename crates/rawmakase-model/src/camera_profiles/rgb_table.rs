//! Adobe's RGB tables (the DNG SDK's `dng_rgb_table`), which Lightroom's creative looks
//! (Artistic, Vintage, most of Modern) and camera-matching XMPs carry beside or instead
//! of an HSV look table. A table maps colours in its own space (primaries and encoding)
//! through a 1D curve per channel or a 3D grid, and a look applies it at an amount:
//! `r + amount·(table(r) − r)` in table space, as the DNG SDK blends.
use super::{Matrix, PRO_TO_RGB, RGB_TO_PRO, cube::Cube, cube_look::LutSpace, matmul};
use crate::color::{mul, srgb_decode, srgb_encode};
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// ProPhoto (D50) to Adobe RGB (1998), Bradford-adapted to D50, and back.
const PRO_TO_ADOBE: Matrix = [
    [1.3896414, -0.169455, -0.2201864],
    [-0.2288268, 1.2317534, -0.0029266],
    [-0.0176251, -0.096258, 1.1138831],
];
const ADOBE_TO_PRO: Matrix = [
    [0.7401223, 0.1132767, 0.146601],
    [0.137551, 0.8330699, 0.0293791],
    [0.0235977, 0.0737835, 0.9026188],
];
/// ProPhoto (D50) to Display P3 (P3 primaries, D65 white), Bradford-adapted to D50,
/// and back.
const PRO_TO_P3: Matrix = [
    [1.6325645, -0.3797686, -0.2527959],
    [-0.1537019, 1.1667131, -0.0130112],
    [0.0103933, -0.0628079, 1.0524146],
];
const P3_TO_PRO: Matrix = [
    [0.6316912, 0.2139282, 0.1543806],
    [0.0832043, 0.8858575, 0.0309382],
    [-0.0012727, 0.0507551, 0.9505176],
];
/// ProPhoto (D50) to Rec. 2020 (ITU-R BT.2020), Bradford-adapted to D50, and back.
const PRO_TO_REC2020: Matrix = [
    [1.2006506, -0.0575637, -0.1430869],
    [-0.0699434, 1.080628, -0.0106845],
    [0.0055415, -0.0407826, 1.035241],
];
const REC2020_TO_PRO: Matrix = [
    [0.8351932, 0.0488653, 0.1159414],
    [0.0540346, 0.9289099, 0.0170555],
    [-0.002342, 0.0363322, 0.9660099],
];
const IDENTITY: Matrix = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];

/// The primaries a table's colours are in (`dng_rgb_table::primaries`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Primaries {
    Srgb,
    AdobeRgb,
    ProPhoto,
    DisplayP3,
    Rec2020,
}
impl Primaries {
    fn from_code(code: u32) -> Result<Self> {
        Ok(match code {
            0 => Self::Srgb,
            1 => Self::AdobeRgb,
            2 => Self::ProPhoto,
            3 => Self::DisplayP3,
            4 => Self::Rec2020,
            _ => bail!("Unsupported RGB table primaries {code}"),
        })
    }
    /// From linear display RGB (sRGB primaries adapted to D50, where the colour
    /// stage works) into these primaries, and back.
    fn matrices(self) -> [Matrix; 2] {
        // Display RGB is sRGB already; the others go through ProPhoto.
        let [from_pro, to_pro] = match self {
            Self::Srgb => return [IDENTITY, IDENTITY],
            Self::AdobeRgb => [PRO_TO_ADOBE, ADOBE_TO_PRO],
            Self::ProPhoto => [IDENTITY, IDENTITY],
            Self::DisplayP3 => [PRO_TO_P3, P3_TO_PRO],
            Self::Rec2020 => [PRO_TO_REC2020, REC2020_TO_PRO],
        };
        [matmul(from_pro, RGB_TO_PRO), matmul(PRO_TO_RGB, to_pro)]
    }
}
/// How a table's values are encoded (`dng_rgb_table::gamma`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gamma {
    Linear,
    Srgb,
    Power18,
    Power22,
    /// Rec. 2020's (ITU-R BT.2020, as BT.709): linear near black, then a 0.45 power.
    Rec2020,
}
impl Gamma {
    fn from_code(code: u32) -> Result<Self> {
        Ok(match code {
            0 => Self::Linear,
            1 => Self::Srgb,
            2 => Self::Power18,
            3 => Self::Power22,
            4 => Self::Rec2020,
            _ => bail!("Unsupported RGB table gamma {code}"),
        })
    }
    pub fn code(self) -> u32 {
        match self {
            Self::Linear => 0,
            Self::Srgb => 1,
            Self::Power18 => 2,
            Self::Power22 => 3,
            Self::Rec2020 => 4,
        }
    }
    fn encode(self, v: f32) -> f32 {
        match self {
            Self::Linear => v,
            Self::Srgb => srgb_encode(v),
            Self::Power18 => v.powf(1. / 1.8),
            Self::Power22 => v.powf(1. / 2.2),
            Self::Rec2020 => rec2020_encode(v),
        }
    }
    fn decode(self, v: f32) -> f32 {
        match self {
            Self::Linear => v,
            Self::Srgb => srgb_decode(v),
            Self::Power18 => v.powf(1.8),
            Self::Power22 => v.powf(2.2),
            Self::Rec2020 => rec2020_decode(v),
        }
    }
}
/// Rec. 2020's curve (BT.2020's exact constants), measured to be the encoding of code 4
/// with Camera Raw 18.7 (`rgb-code4-half-*`). Mirrored in `develop.wgsl`.
const REC2020_ALPHA: f32 = 1.099_296_8;
const REC2020_BETA: f32 = 0.018_053_97;
fn rec2020_encode(v: f32) -> f32 {
    if v < REC2020_BETA {
        4.5 * v
    } else {
        REC2020_ALPHA * v.powf(0.45) - (REC2020_ALPHA - 1.)
    }
}
fn rec2020_decode(v: f32) -> f32 {
    if v < 4.5 * REC2020_BETA {
        v / 4.5
    } else {
        ((v + REC2020_ALPHA - 1.) / REC2020_ALPHA).powf(1. / 0.45)
    }
}
/// What happens to colours outside the table's space (`dng_rgb_table::gamut`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gamut {
    /// Clipped into it before the table.
    Clip,
    /// Clipped for the lookup, with what was clipped added back afterwards.
    Extend,
}
/// Whether the table is a curve for each channel or a grid over all three.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dimensions {
    One,
    Three,
}
/// A decoded RGB table. It is stored in recipes in Adobe's encoding, which is a
/// quarter the size of its samples written out.
#[derive(Clone, Debug, PartialEq)]
pub struct RgbTable {
    dimensions: Dimensions,
    divisions: usize,
    /// Output colours, 0–1: per division for a 1D table; red outermost, then green,
    /// then blue for a 3D one.
    samples: Vec<[f32; 3]>,
    primaries: Primaries,
    gamma: Gamma,
    gamut: Gamut,
    /// The amounts the table may be applied at.
    bounds: [f32; 2],
    /// From linear display RGB into the table's primaries and back.
    matrices: [Matrix; 2],
    encoded: String,
}
impl RgbTable {
    pub fn decode(text: &str) -> Result<Self> {
        let data = super::enhanced::expand(text)?;
        ensure!(data.len() >= 16, "Truncated RGB table");
        let word = |i: usize| u32::from_le_bytes(data[i..i + 4].try_into().unwrap());
        ensure!(word(0) == 1, "Not an RGB table");
        ensure!(word(4) == 1, "Unsupported RGB table version {}", word(4));
        let divisions = word(12) as usize;
        let (dimensions, count) = match word(8) {
            1 => {
                ensure!((2..=MAX_1D).contains(&divisions), "Invalid RGB table size");
                (Dimensions::One, divisions)
            }
            3 => {
                ensure!((2..=MAX_3D).contains(&divisions), "Invalid RGB table size");
                (Dimensions::Three, divisions.pow(3))
            }
            n => bail!("Unsupported {n}-dimensional RGB table"),
        };
        let end = 16 + count * 6;
        // Primaries, gamma, gamut and the amount bounds; Adobe's camera-matching
        // tables add a word that Camera Raw 18.7 renders the same at 0 and 1.
        ensure!(
            data.len() == end + 28 || data.len() == end + 32,
            "Invalid RGB table payload size"
        );
        if data.len() == end + 32 {
            ensure!(word(end + 28) <= 1, "Unsupported RGB table flags");
        }
        let grid = |i: usize| ((i * 0xFFFF + (divisions - 1) / 2) / (divisions - 1)) as u16;
        let samples = data[16..end]
            .as_chunks::<6>()
            .0
            .iter()
            .enumerate()
            .map(|(i, s)| {
                // Each value is stored as its difference from the identity.
                let index = match dimensions {
                    Dimensions::One => [i; 3],
                    Dimensions::Three => [
                        i / (divisions * divisions),
                        i / divisions % divisions,
                        i % divisions,
                    ],
                };
                std::array::from_fn(|c| {
                    let delta = u16::from_le_bytes([s[c * 2], s[c * 2 + 1]]);
                    f32::from(delta.wrapping_add(grid(index[c]))) / 65535.
                })
            })
            .collect();
        let gamut = match word(end + 8) {
            0 => Gamut::Clip,
            1 => Gamut::Extend,
            code => bail!("Unsupported RGB table gamut {code}"),
        };
        let real = |i: usize| f64::from_le_bytes(data[i..i + 8].try_into().unwrap());
        let bounds = [real(end + 12), real(end + 20)];
        ensure!(
            bounds.iter().all(|v| v.is_finite())
                && (0. ..=1.).contains(&bounds[0])
                && (1. ..=4.).contains(&bounds[1]),
            "Invalid RGB table amount bounds"
        );
        let primaries = Primaries::from_code(word(end))?;
        Ok(Self {
            dimensions,
            divisions,
            samples,
            primaries,
            matrices: primaries.matrices(),
            gamma: Gamma::from_code(word(end + 4))?,
            gamut,
            bounds: bounds.map(|v| v as f32),
            encoded: text.into(),
        })
    }
    /// The amount the table applies at: a look's `RGBTableAmount` times its Profile
    /// Amount, within the bounds the table stores (measured with Camera Raw 18.7).
    pub fn amount(&self, table_amount: f32, profile_amount: f32) -> f32 {
        (table_amount * profile_amount).clamp(self.bounds[0], self.bounds[1])
    }
    /// `rgb` (linear display RGB) through the table at `amount`.
    pub fn apply(&self, rgb: [f32; 3], amount: f32) -> [f32; 3] {
        // At 0 the table is off: colours outside its space are not clipped either.
        if amount == 0. {
            return rgb;
        }
        let [into, back] = self.matrices;
        let full = mul(into, rgb).map(|v| v.signum() * self.gamma.encode(v.abs()));
        let encoded = full.map(|v| v.clamp(0., 1.));
        let looked = self.lookup(encoded);
        let out: [f32; 3] = std::array::from_fn(|c| {
            let v = encoded[c] + amount * (looked[c] - encoded[c]);
            match self.gamut {
                // Over 100% the blend can leave 0–1; Camera Raw clips it per channel.
                Gamut::Clip => v.clamp(0., 1.),
                Gamut::Extend => v + full[c] - encoded[c],
            }
        });
        mul(back, out.map(|v| v.signum() * self.gamma.decode(v.abs())))
    }
    /// `rgb`, in table space and within 0–1, through the table.
    pub(super) fn lookup(&self, rgb: [f32; 3]) -> [f32; 3] {
        let n = self.divisions;
        let scaled = rgb.map(|v| v * (n - 1) as f32);
        let base = scaled.map(|v| (v as usize).min(n - 2));
        let f: [f32; 3] = std::array::from_fn(|c| scaled[c] - base[c] as f32);
        match self.dimensions {
            Dimensions::One => std::array::from_fn(|c| {
                let a = self.samples[base[c]][c];
                a + (self.samples[base[c] + 1][c] - a) * f[c]
            }),
            Dimensions::Three => {
                let at = |r: usize, g: usize, b: usize| {
                    self.samples[((base[0] + r) * n + base[1] + g) * n + base[2] + b]
                };
                tetrahedral(at, f)
            }
        }
    }
    pub fn dimensions(&self) -> Dimensions {
        self.dimensions
    }
    pub fn divisions(&self) -> usize {
        self.divisions
    }
    pub fn samples(&self) -> &[[f32; 3]] {
        &self.samples
    }
    pub fn gamma(&self) -> Gamma {
        self.gamma
    }
    pub fn primaries(&self) -> Primaries {
        self.primaries
    }
    pub fn gamut(&self) -> Gamut {
        self.gamut
    }
    /// The matrices from linear display RGB into the table's primaries and back.
    pub fn matrices(&self) -> [Matrix; 2] {
        self.matrices
    }
}
/// The largest 3D and 1D tables the DNG SDK reads.
const MAX_3D: usize = 64;
const MAX_1D: usize = 4096;
/// A table's primaries, gamma and gamut codes and its amount bounds, as stored after
/// its samples.
pub(super) type Tail = (u32, u32, u32, [f64; 2]);
/// A table's bytes as the DNG SDK stores them, the inverse of [`RgbTable::decode`]'s
/// layout: `samples` (0–1, clamped and rounded to 16 bits) in its order, each stored as
/// its difference from the identity, then `tail`.
pub(super) fn encode_table(
    dimensions: Dimensions,
    divisions: usize,
    samples: &[[f32; 3]],
    tail: Tail,
) -> Vec<u8> {
    let (code, count) = match dimensions {
        Dimensions::One => (1u32, divisions),
        Dimensions::Three => (3, divisions.pow(3)),
    };
    assert_eq!(samples.len(), count, "a table's samples fill its grid");
    let mut bytes = Vec::with_capacity(16 + count * 6 + 28);
    for v in [1u32, 1, code, divisions as u32] {
        bytes.extend(v.to_le_bytes());
    }
    let grid = |i: usize| ((i * 0xFFFF + (divisions - 1) / 2) / (divisions - 1)) as u16;
    for (i, sample) in samples.iter().enumerate() {
        let index = match dimensions {
            Dimensions::One => [i; 3],
            Dimensions::Three => [
                i / (divisions * divisions),
                i / divisions % divisions,
                i % divisions,
            ],
        };
        for c in 0..3 {
            let v = (sample[c].clamp(0., 1.) * 65535.).round() as u16;
            bytes.extend(v.wrapping_sub(grid(index[c])).to_le_bytes());
        }
    }
    for v in [tail.0, tail.1, tail.2] {
        bytes.extend(v.to_le_bytes());
    }
    for v in tail.3 {
        bytes.extend(v.to_le_bytes());
    }
    bytes
}
impl RgbTable {
    /// A `.cube` as an RGB table in `space`, as Camera Raw 18.7's Create Profile makes
    /// one (the space's primaries and encoding, clipped, amounts 0–2; sRGB by default),
    /// with the table's bytes (which name it). The space changes only the codes after
    /// the samples. Create Profile samples every 3D cube onto at most 32 divisions;
    /// here one of up to 64 keeps its grid, node for node, since resampling onto
    /// another grid is measurably lossy. A larger one (65 is common;
    /// Create Profile refuses it) is sampled onto 64, the most a table holds, by
    /// tetrahedral lookup in the cube, as Create Profile resamples; so is a cube with a
    /// 1D shaper, which is baked in (Create Profile drops it). A 1D cube over 4096
    /// entries is sampled onto 4096. Outputs are clamped to 0–1. A cube over another
    /// domain than 0–1 is refused, as Create Profile refuses it, and so is a shaper
    /// reading another input range.
    pub(super) fn from_cube(cube: &Cube, space: LutSpace) -> Result<(Self, Vec<u8>)> {
        let (dimensions, divisions, samples) = match cube {
            Cube::Three(lut) => {
                ensure!(
                    lut.domain() == ([0.; 3], [1.; 3]),
                    "the cube's domain is {:?} to {:?} rather than 0–1, which RAWmakase \
                     cannot import (nor can Camera Raw)",
                    lut.domain().0,
                    lut.domain().1
                );
                if let Some(range) = lut.shaper_input_range() {
                    ensure!(
                        range == [0., 1.],
                        "the cube's shaper reads inputs from {} to {} rather than 0–1, as a \
                         cube for log or scene-linear input does, which RAWmakase cannot import",
                        range[0],
                        range[1]
                    );
                }
                // A shaper bends the input before the 3D table, so the cube's grid is
                // not a grid in table space: shaper and cube are sampled together on
                // the most divisions a table holds.
                let n = if lut.has_shaper() {
                    MAX_3D
                } else {
                    lut.size().min(MAX_3D)
                };
                let keep = n == lut.size() && !lut.has_shaper();
                let at = |i: usize| i as f32 / (n - 1) as f32;
                let mut samples = Vec::with_capacity(n.pow(3));
                // Red outermost here, red fastest in the cube.
                for r in 0..n {
                    for g in 0..n {
                        for b in 0..n {
                            samples.push(if keep {
                                lut.node(r, g, b)
                            } else {
                                lut.lookup([at(r), at(g), at(b)])
                            });
                        }
                    }
                }
                (Dimensions::Three, n, samples)
            }
            Cube::One(lut) => {
                ensure!(
                    lut.domain() == ([0.; 3], [1.; 3]),
                    "the cube's domain is {:?} to {:?} rather than 0–1, which RAWmakase \
                     cannot import",
                    lut.domain().0,
                    lut.domain().1
                );
                let n = lut.size().min(MAX_1D);
                let samples = (0..n)
                    .map(|i| {
                        if n == lut.size() {
                            lut.entry(i)
                        } else {
                            lut.lookup([i as f32 / (n - 1) as f32; 3])
                        }
                    })
                    .collect();
                (Dimensions::One, n, samples)
            }
        };
        let bytes = encode_table(dimensions, divisions, &samples, space.tail());
        let table = Self::decode(&super::enhanced::compress(&bytes))?;
        Ok((table, bytes))
    }
}
/// Tetrahedral interpolation in a grid cell, as the DNG SDK does: the cell is split
/// into six tetrahedra along its gray diagonal, picked by the order of the fractions.
pub(super) fn tetrahedral(at: impl Fn(usize, usize, usize) -> [f32; 3], f: [f32; 3]) -> [f32; 3] {
    let [fr, fg, fb] = f;
    let c000 = at(0, 0, 0);
    let c111 = at(1, 1, 1);
    // The two corners passed on the way from c000 to c111, and the weights of the
    // three edges walked.
    let (a, b, w) = if fr > fg {
        if fg > fb {
            (at(1, 0, 0), at(1, 1, 0), [fr, fg, fb])
        } else if fr > fb {
            (at(1, 0, 0), at(1, 0, 1), [fr, fb, fg])
        } else {
            (at(0, 0, 1), at(1, 0, 1), [fb, fr, fg])
        }
    } else if fb > fg {
        (at(0, 0, 1), at(0, 1, 1), [fb, fg, fr])
    } else if fb > fr {
        (at(0, 1, 0), at(0, 1, 1), [fg, fb, fr])
    } else {
        (at(0, 1, 0), at(1, 1, 0), [fg, fr, fb])
    };
    std::array::from_fn(|c| {
        c000[c] + w[0] * (a[c] - c000[c]) + w[1] * (b[c] - a[c]) + w[2] * (c111[c] - b[c])
    })
}
impl Serialize for RgbTable {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.encoded)
    }
}
impl<'de> Deserialize<'de> for RgbTable {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Self::decode(&text).map_err(serde::de::Error::custom)
    }
}

#[cfg(any(test, feature = "test-support"))]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) mod tests {
    use super::*;

    /// A table's bytes in Adobe's encoding (base 85 over zlib).
    pub(crate) fn encode(bytes: &[u8]) -> String {
        super::super::enhanced::tests::encode(bytes)
    }
    /// A 3D table of `f` over `divisions`, in Adobe RGB with gamma 2.2 unless
    /// `tail` gives other primaries, gamma, gamut and bounds.
    pub(crate) fn table_bytes(
        divisions: usize,
        f: impl Fn([f32; 3]) -> [f32; 3],
        tail: Tail,
    ) -> Vec<u8> {
        let at = |i: usize| i as f32 / (divisions - 1) as f32;
        let mut samples = Vec::new();
        for r in 0..divisions {
            for g in 0..divisions {
                for b in 0..divisions {
                    samples.push(f([at(r), at(g), at(b)]));
                }
            }
        }
        encode_table(Dimensions::Three, divisions, &samples, tail)
    }
    /// A 1D table of `f` (one value to three channel curves) over `divisions`.
    pub(crate) fn table_bytes_1d(
        divisions: usize,
        f: impl Fn(f32) -> [f32; 3],
        tail: Tail,
    ) -> Vec<u8> {
        let samples: Vec<_> = (0..divisions)
            .map(|i| f(i as f32 / (divisions - 1) as f32))
            .collect();
        encode_table(Dimensions::One, divisions, &samples, tail)
    }
    pub(crate) const ADOBE: Tail = (1, 3, 0, [0., 2.]);
    /// Tables of every kind for GPU tests: a 3D Adobe RGB table, a 3D linear ProPhoto
    /// one that extends the gamut, a 1D sRGB one, a 3D sRGB one whose blend leaves
    /// 0–1 at amounts over 1 (`clips_over_one`), a 3D ProPhoto one in gamma 1.8, as
    /// a `.cube` imported for ProPhoto RGB is, and 3D Display P3 and Rec. 2020 ones in
    /// those spaces' encodings.
    pub(crate) fn variety() -> Vec<RgbTable> {
        let curves = |v: f32| {
            [
                0.05 + 0.9 * v.powf(0.85),
                v + 0.06 * (6.3 * v).sin(),
                0.1 + 0.75 * v,
            ]
        };
        [
            table_bytes(17, fade, ADOBE),
            table_bytes(9, fade, (2, 0, 1, [0., 2.])),
            table_bytes_1d(64, curves, (0, 1, 0, [0., 2.])),
            table_bytes(9, clips_over_one, (0, 1, 0, [0., 2.])),
            table_bytes(9, fade, (2, 2, 0, [0., 2.])),
            table_bytes(9, fade, (3, 1, 0, [0., 2.])),
            table_bytes(9, fade, (4, 4, 0, [0., 2.])),
        ]
        .iter()
        .map(|b| RgbTable::decode(&encode(b)).unwrap())
        .collect()
    }
    /// Red quartered and blue lifted by half: at amount 2 red goes below 0 and blue above 1.
    pub(crate) fn clips_over_one([r, g, b]: [f32; 3]) -> [f32; 3] {
        [0.25 * r, g, (b + 0.5).min(1.)]
    }
    /// A smooth creative grade, as `fade` in scripts/corpus/synthetic-looks.py.
    pub(crate) fn fade([r, g, b]: [f32; 3]) -> [f32; 3] {
        use std::f32::consts::PI;
        [
            0.04 + 0.9 * r.powf(1.1) + 0.05 * g * (1. - r),
            0.03 + 0.92 * g + 0.04 * (PI * g).sin() + 0.03 * r * (1. - g),
            0.08 + 0.8 * b + 0.06 * r * (1. - b) - 0.03 * (PI * g).sin(),
        ]
    }

    #[test]
    fn rgb_tables_decode_and_interpolate_tetrahedrally() -> Result<()> {
        let table = RgbTable::decode(&encode(&table_bytes(9, fade, ADOBE)))?;
        assert_eq!((table.dimensions, table.divisions), (Dimensions::Three, 9));
        assert_eq!(
            (table.primaries, table.gamma, table.gamut),
            (Primaries::AdobeRgb, Gamma::Power22, Gamut::Clip)
        );
        // Grid points come back within 16-bit rounding; between them the table
        // follows the smooth grade closely.
        for p in [[0.; 3], [1.; 3], [0.25, 0.5, 0.875], [0.3, 0.71, 0.12]] {
            let got = table.lookup(p);
            let want = fade(p);
            assert!(
                got.iter().zip(want).all(|(a, b)| (a - b).abs() < 2e-3),
                "{p:?}: {got:?} {want:?}"
            );
        }
        // Tetrahedral interpolation is exact for a linear map.
        let linear = |[r, g, b]: [f32; 3]| [0.6 * r + 0.3 * g, 0.2 + 0.5 * b, 0.9 * g];
        let table = RgbTable::decode(&encode(&table_bytes(5, linear, ADOBE)))?;
        let got = table.lookup([0.33, 0.61, 0.47]);
        let want = linear([0.33, 0.61, 0.47]);
        assert!(got.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-4));
        Ok(())
    }
    #[test]
    fn rgb_tables_blend_by_amount_in_table_space() -> Result<()> {
        let table = RgbTable::decode(&encode(&table_bytes(9, fade, ADOBE)))?;
        let [into, back] = Primaries::AdobeRgb.matrices();
        let gray = mul(back, [0.2; 3]);
        // At 0 the table changes nothing; at 1 it is the table; at 2 it goes as far
        // again, all in gamma 2.2.
        let none = table.apply(gray, 0.);
        assert!(none.iter().zip(gray).all(|(a, b)| (a - b).abs() < 1e-5));
        // Not even colours outside Adobe RGB, which the table clips.
        let wide = [1.2, -0.1, 0.05];
        assert_eq!(table.apply(wide, 0.), wide);
        let encoded = 0.2f32.powf(1. / 2.2);
        let looked = table.lookup([encoded; 3]);
        for amount in [1., 2.] {
            let got = mul(into, table.apply(gray, amount));
            for c in 0..3 {
                let want = (encoded + amount * (looked[c] - encoded)).powf(2.2);
                assert!((got[c] - want).abs() < 1e-4, "{amount}: {got:?}");
            }
        }
        // Over 1 the blend can leave 0–1; a clipping table clips it there per channel,
        // as Camera Raw 18.7 renders `cube-creative-33-200` (whose dark grays lose
        // red and green, not their hue). A sRGB table keeps the encoding visible.
        let srgb = RgbTable::decode(&encode(&table_bytes(
            5,
            clips_over_one,
            (0, 1, 0, [0., 2.]),
        )))?;
        let x = 0.4f32;
        let got = srgb.apply([srgb_decode(x); 3], 2.);
        let want = [0., x, 1.].map(srgb_decode);
        assert!(
            got.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-4),
            "{got:?} {want:?}"
        );
        // Profile Amount times the look's own amount, within the table's bounds.
        let bounded = RgbTable::decode(&encode(&table_bytes(5, fade, (1, 3, 0, [0.5, 1.5]))))?;
        assert_eq!(bounded.amount(0.5, 2.), 1.);
        assert_eq!(bounded.amount(1., 0.), 0.5);
        assert_eq!(bounded.amount(1., 2.), 1.5);
        Ok(())
    }
    /// A `.cube`'s table in each space Create Profile offers has that space's primaries
    /// and encoding, Clip and amounts 0–2, and the cube's samples unchanged: the bytes
    /// differ only after them, as Camera Raw 18.7 writes `creative-32` in each space.
    #[test]
    fn a_cubes_space_changes_only_its_codes() -> Result<()> {
        let mut text = String::from("LUT_3D_SIZE 5\n");
        for i in 0..125 {
            let [r, g, b] = [i % 5, i / 5 % 5, i / 25].map(|v| v as f32 / 4.);
            text += &format!("{} {} {}\n", 0.1 + 0.8 * g, r * r, 1. - b);
        }
        let cube = Cube::parse(&text)?;
        let samples = 16 + 125 * 6;
        let (_, srgb) = RgbTable::from_cube(&cube, LutSpace::Srgb)?;
        for (space, primaries, gamma) in [
            (LutSpace::Srgb, Primaries::Srgb, Gamma::Srgb),
            (LutSpace::AdobeRgb, Primaries::AdobeRgb, Gamma::Power22),
            (LutSpace::DisplayP3, Primaries::DisplayP3, Gamma::Srgb),
            (LutSpace::ProPhotoRgb, Primaries::ProPhoto, Gamma::Power18),
            (LutSpace::Rec2020, Primaries::Rec2020, Gamma::Rec2020),
        ] {
            let (table, bytes) = RgbTable::from_cube(&cube, space)?;
            assert_eq!(
                (table.primaries, table.gamma, table.gamut, table.bounds),
                (primaries, gamma, Gamut::Clip, [0., 2.]),
                "{space}"
            );
            assert_eq!(bytes[..samples], srgb[..samples], "{space}");
            assert_eq!(bytes.len(), samples + 28);
        }
        Ok(())
    }
    /// A table blends in its own encoding: a cube that maps each channel linearly
    /// gives `decode(f(encode(x)))` on a gray, a different colour in each space's
    /// encoding. A colour inside Adobe RGB but outside sRGB and Display P3 passes an
    /// identity cube unchanged in Adobe RGB, ProPhoto and Rec. 2020, and is clipped into
    /// sRGB or Display P3 in those.
    #[test]
    fn a_cubes_space_is_the_one_it_applies_in() -> Result<()> {
        let cube = |f: fn([f32; 3]) -> [f32; 3], space| -> Result<RgbTable> {
            let mut text = String::from("LUT_3D_SIZE 3\n");
            for i in 0..27 {
                let [x, y, z] = f([i % 3, i / 3 % 3, i / 9].map(|v| v as f32 / 2.));
                text += &format!("{x} {y} {z}\n");
            }
            Ok(RgbTable::from_cube(&Cube::parse(&text)?, space)?.0)
        };
        let f = |[r, g, b]: [f32; 3]| [0.2 + 0.6 * r, 0.9 * g, 0.1 + 0.5 * b];
        let x = 0.18f32;
        let mut outputs = Vec::new();
        for (space, encode, decode) in [
            (
                LutSpace::Srgb,
                srgb_encode as fn(f32) -> f32,
                srgb_decode as fn(f32) -> f32,
            ),
            (
                LutSpace::AdobeRgb,
                |v: f32| v.powf(1. / 2.2),
                |v: f32| v.powf(2.2),
            ),
            (
                LutSpace::ProPhotoRgb,
                |v: f32| v.powf(1. / 1.8),
                |v: f32| v.powf(1.8),
            ),
            (LutSpace::DisplayP3, srgb_encode, srgb_decode),
            // BT.2020's curve, written out here rather than taken from `Gamma`.
            (
                LutSpace::Rec2020,
                |v: f32| {
                    if v < 0.018_054 {
                        4.5 * v
                    } else {
                        1.099_297 * v.powf(0.45) - 0.099_297
                    }
                },
                |v: f32| {
                    if v < 0.081_243 {
                        v / 4.5
                    } else {
                        ((v + 0.099_297) / 1.099_297).powf(1. / 0.45)
                    }
                },
            ),
        ] {
            let table = cube(f, space)?;
            let [into, back] = table.matrices;
            let got = mul(into, table.apply(mul(back, [x; 3]), 1.));
            let want = f([encode(x); 3]).map(decode);
            assert!(
                got.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-4),
                "{space}: {got:?} {want:?}"
            );
            outputs.push(want);
        }
        assert!(outputs[0][0] != outputs[1][0] && outputs[1][0] != outputs[2][0]);
        assert!(outputs[0][0] != outputs[4][0] && outputs[1][0] != outputs[4][0]);
        // Adobe RGB's green, which sRGB and Display P3 hold only with a negative red,
        // and Rec. 2020 holds.
        let [_, from_adobe] = Primaries::AdobeRgb.matrices();
        let green = mul(from_adobe, [0.05, 0.6, 0.1]);
        assert!(green[0] < -0.01, "{green:?}");
        let [into_p3, _] = Primaries::DisplayP3.matrices();
        assert!(mul(into_p3, green)[0] < -0.01);
        let [into_2020, _] = Primaries::Rec2020.matrices();
        assert!(mul(into_2020, green).iter().all(|&v| v > 0.01));
        for space in LutSpace::ALL {
            let table = cube(|p| p, space)?;
            let got = table.apply(green, 1.);
            let want = match space {
                LutSpace::Srgb | LutSpace::DisplayP3 => {
                    let [into, back] = table.matrices;
                    mul(back, mul(into, green).map(|v| v.clamp(0., 1.)))
                }
                _ => green,
            };
            assert!(
                got.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-3),
                "{space}: {got:?} {want:?}"
            );
        }
        Ok(())
    }
    /// Display P3 and Rec. 2020 tables, as Camera Raw writes them (primaries 3 with the
    /// sRGB encoding, primaries 4 with code 4), are read with their spaces' matrices.
    #[test]
    fn display_p3_and_rec2020_tables_decode() -> Result<()> {
        let p3 = RgbTable::decode(&encode(&table_bytes(5, fade, (3, 1, 0, [0., 2.]))))?;
        assert_eq!(
            (p3.primaries, p3.gamma),
            (Primaries::DisplayP3, Gamma::Srgb)
        );
        let rec = RgbTable::decode(&encode(&table_bytes(5, fade, (4, 4, 0, [0., 2.]))))?;
        assert_eq!(
            (rec.primaries, rec.gamma),
            (Primaries::Rec2020, Gamma::Rec2020)
        );
        let near = |a: [f32; 3], b: [f32; 3], tolerance: f32| {
            a.iter().zip(b).all(|(x, y)| (x - y).abs() < tolerance)
        };
        // sRGB's red in each, as the spaces' published primaries give it (Rec. 2020's
        // as the engine's own `TO_2020`), and white kept white.
        for (primaries, red) in [
            (Primaries::DisplayP3, [0.822_49, 0.033_20, 0.017_09]),
            (Primaries::Rec2020, [0.627_404, 0.069_097, 0.016_391]),
        ] {
            let [into, back] = primaries.matrices();
            assert!(near(mul(into, [1., 0., 0.]), red, 2e-4), "{primaries:?}");
            assert!(near(mul(into, [1.; 3]), [1.; 3], 1e-4), "{primaries:?}");
            let p = [0.3, 0.7, 0.1];
            assert!(near(mul(back, mul(into, p)), p, 1e-5), "{primaries:?}");
        }
        Ok(())
    }
    /// Code 4 is BT.2020's curve: linear below 0.018, a 0.45 power above, continuous
    /// at the knee, and decoded by its exact inverse.
    #[test]
    fn rec2020_encoding_is_bt2020s_curve() {
        let curve = Gamma::Rec2020;
        assert_eq!(Gamma::from_code(4).unwrap(), curve);
        assert_eq!(curve.code(), 4);
        for (linear, encoded) in [(0., 0.), (0.01, 0.045), (0.18, 0.408_88), (1., 1.)] {
            assert!((curve.encode(linear) - encoded).abs() < 1e-4, "{linear}");
        }
        let below = curve.encode(REC2020_BETA - 1e-6);
        let above = curve.encode(REC2020_BETA);
        assert!((above - below).abs() < 1e-5, "{below} {above}");
        for i in 0..=1000 {
            let v = i as f32 / 1000.;
            assert!((curve.decode(curve.encode(v)) - v).abs() < 1e-5, "{v}");
        }
    }
    #[test]
    fn malformed_rgb_tables_are_refused() {
        let good = table_bytes(3, fade, ADOBE);
        assert!(RgbTable::decode(&encode(&good)).is_ok());
        let with = |at: usize, v: u32| {
            let mut b = good.clone();
            b[at..at + 4].copy_from_slice(&v.to_le_bytes());
            encode(&b)
        };
        let tail = 16 + 27 * 6;
        for bad in [
            with(0, 0),        // an HSV table
            with(4, 2),        // an unknown version
            with(8, 2),        // two dimensions
            with(12, 4),       // samples missing
            with(tail, 7),     // unknown primaries
            with(tail + 4, 9), // unknown gamma
            with(tail + 8, 2), // unknown gamut
            encode(&good[..good.len() - 1]),
        ] {
            assert!(RgbTable::decode(&bad).is_err());
        }
        let mut flagged = good.clone();
        flagged.extend(1u32.to_le_bytes());
        assert!(RgbTable::decode(&encode(&flagged)).is_ok());
        flagged.splice(flagged.len() - 4.., 2u32.to_le_bytes());
        assert!(RgbTable::decode(&encode(&flagged)).is_err());
    }
    /// Samples are differences from the identity, rounded to 16 bits: a table of
    /// zero differences is the identity at any size, endpoints included.
    #[test]
    fn zero_differences_decode_to_the_identity() {
        for divisions in [2, 16, 32] {
            let mut bytes = Vec::new();
            for v in [1u32, 1, 3, divisions] {
                bytes.extend(v.to_le_bytes());
            }
            bytes.resize(16 + divisions.pow(3) as usize * 6, 0);
            for v in [1u32, 3, 0] {
                bytes.extend(v.to_le_bytes());
            }
            for v in [0f64, 2.] {
                bytes.extend(v.to_le_bytes());
            }
            let table = RgbTable::decode(&encode(&bytes)).unwrap();
            for p in [[0.; 3], [1.; 3], [1., 0., 0.5], [0.3, 0.7, 0.9]] {
                let got = table.lookup(p);
                assert!(
                    got.iter().zip(p).all(|(a, b)| (a - b).abs() < 1e-4),
                    "{divisions}: {p:?} {got:?}"
                );
            }
        }
    }
    #[test]
    fn one_dimensional_tables_are_a_curve_per_channel() {
        let tables = variety();
        let table = &tables[2];
        assert_eq!(table.dimensions, Dimensions::One);
        let got = table.lookup([0.5, 0.25, 1.]);
        let want = [
            0.05 + 0.9 * 0.5f32.powf(0.85),
            0.25 + 0.06 * (6.3f32 * 0.25).sin(),
            0.85,
        ];
        assert!(
            got.iter().zip(want).all(|(a, b)| (a - b).abs() < 2e-3),
            "{got:?}"
        );
    }
}
