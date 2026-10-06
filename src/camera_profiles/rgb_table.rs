//! Adobe's RGB tables (the DNG SDK's `dng_rgb_table`), which Lightroom's creative looks
//! (Artistic, Vintage, most of Modern) and camera-matching XMPs carry beside or instead
//! of an HSV look table. A table maps colours in its own space (primaries and encoding)
//! through a 1D curve per channel or a 3D grid, and a look applies it at an amount:
//! `r + amount·(table(r) − r)` in table space, as the DNG SDK blends.
use super::{Matrix, PRO_TO_RGB, RGB_TO_PRO, matmul};
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
const IDENTITY: Matrix = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];

/// The primaries a table's colours are in (`dng_rgb_table::primaries`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Primaries {
    Srgb,
    AdobeRgb,
    ProPhoto,
}
impl Primaries {
    fn from_code(code: u32) -> Result<Self> {
        Ok(match code {
            0 => Self::Srgb,
            1 => Self::AdobeRgb,
            2 => Self::ProPhoto,
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
        };
        [matmul(from_pro, RGB_TO_PRO), matmul(PRO_TO_RGB, to_pro)]
    }
}
/// How a table's values are encoded (`dng_rgb_table::gamma`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Gamma {
    Linear,
    Srgb,
    Power18,
    Power22,
}
impl Gamma {
    fn from_code(code: u32) -> Result<Self> {
        Ok(match code {
            0 => Self::Linear,
            1 => Self::Srgb,
            2 => Self::Power18,
            3 => Self::Power22,
            _ => bail!("Unsupported RGB table gamma {code}"),
        })
    }
    pub(crate) fn code(self) -> u32 {
        match self {
            Self::Linear => 0,
            Self::Srgb => 1,
            Self::Power18 => 2,
            Self::Power22 => 3,
        }
    }
    fn encode(self, v: f32) -> f32 {
        match self {
            Self::Linear => v,
            Self::Srgb => srgb_encode(v),
            Self::Power18 => v.powf(1. / 1.8),
            Self::Power22 => v.powf(1. / 2.2),
        }
    }
    fn decode(self, v: f32) -> f32 {
        match self {
            Self::Linear => v,
            Self::Srgb => srgb_decode(v),
            Self::Power18 => v.powf(1.8),
            Self::Power22 => v.powf(2.2),
        }
    }
}
/// What happens to colours outside the table's space (`dng_rgb_table::gamut`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Gamut {
    /// Clipped into it before the table.
    Clip,
    /// Clipped for the lookup, with what was clipped added back afterwards.
    Extend,
}
/// Whether the table is a curve for each channel or a grid over all three.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Dimensions {
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
                ensure!((2..=4096).contains(&divisions), "Invalid RGB table size");
                (Dimensions::One, divisions)
            }
            3 => {
                ensure!((2..=64).contains(&divisions), "Invalid RGB table size");
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
                Gamut::Clip => v,
                Gamut::Extend => v + full[c] - encoded[c],
            }
        });
        mul(back, out.map(|v| v.signum() * self.gamma.decode(v.abs())))
    }
    fn lookup(&self, rgb: [f32; 3]) -> [f32; 3] {
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
    pub(crate) fn dimensions(&self) -> Dimensions {
        self.dimensions
    }
    pub(crate) fn divisions(&self) -> usize {
        self.divisions
    }
    pub(crate) fn samples(&self) -> &[[f32; 3]] {
        &self.samples
    }
    pub(crate) fn gamma(&self) -> Gamma {
        self.gamma
    }
    pub(crate) fn gamut(&self) -> Gamut {
        self.gamut
    }
    /// The matrices from linear display RGB into the table's primaries and back.
    pub(crate) fn matrices(&self) -> [Matrix; 2] {
        self.matrices
    }
}
/// Tetrahedral interpolation in a grid cell, as the DNG SDK does: the cell is split
/// into six tetrahedra along its gray diagonal, picked by the order of the fractions.
pub(crate) fn tetrahedral(at: impl Fn(usize, usize, usize) -> [f32; 3], f: [f32; 3]) -> [f32; 3] {
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

#[cfg(test)]
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
        tail: (u32, u32, u32, [f64; 2]),
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        for v in [1u32, 1, 3, divisions as u32] {
            bytes.extend(v.to_le_bytes());
        }
        let grid = |i: usize| ((i * 0xFFFF + (divisions - 1) / 2) / (divisions - 1)) as u16;
        for r in 0..divisions {
            for g in 0..divisions {
                for b in 0..divisions {
                    let out = f([r, g, b].map(|i| i as f32 / (divisions - 1) as f32));
                    for (c, i) in [r, g, b].into_iter().enumerate() {
                        let v = (out[c].clamp(0., 1.) * 65535.).round() as u16;
                        bytes.extend(v.wrapping_sub(grid(i)).to_le_bytes());
                    }
                }
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
    /// A 1D table of `f` (one value to three channel curves) over `divisions`.
    pub(crate) fn table_bytes_1d(
        divisions: usize,
        f: impl Fn(f32) -> [f32; 3],
        tail: (u32, u32, u32, [f64; 2]),
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        for v in [1u32, 1, 1, divisions as u32] {
            bytes.extend(v.to_le_bytes());
        }
        for i in 0..divisions {
            let identity = ((i * 0xFFFF + (divisions - 1) / 2) / (divisions - 1)) as u16;
            for v in f(i as f32 / (divisions - 1) as f32) {
                let v = (v.clamp(0., 1.) * 65535.).round() as u16;
                bytes.extend(v.wrapping_sub(identity).to_le_bytes());
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
    pub(crate) const ADOBE: (u32, u32, u32, [f64; 2]) = (1, 3, 0, [0., 2.]);
    /// Tables of every kind for GPU tests: a 3D Adobe RGB table, a 3D linear ProPhoto
    /// one that extends the gamut, and a 1D sRGB one.
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
        ]
        .iter()
        .map(|b| RgbTable::decode(&encode(b)).unwrap())
        .collect()
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
        // Profile Amount times the look's own amount, within the table's bounds.
        let bounded = RgbTable::decode(&encode(&table_bytes(5, fade, (1, 3, 0, [0.5, 1.5]))))?;
        assert_eq!(bounded.amount(0.5, 2.), 1.);
        assert_eq!(bounded.amount(1., 0.), 0.5);
        assert_eq!(bounded.amount(1., 2.), 1.5);
        Ok(())
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
