//! The colour spaces an export can be written in: sRGB, Adobe RGB (1998) and ProPhoto
//! RGB (ROMM RGB), each with the profile's primaries, white point and transfer curve
//! (docs/color-pipeline.md#export-color-spaces).
//!
//! The renderer works in linear sRGB (D65) up to the end of its colour stage. A space's
//! [`OutputSpace::from_linear_srgb`] matrix takes those values to its own linear RGB;
//! ProPhoto's white is D50, so its matrix includes the Bradford D65→D50 adaptation ICC
//! profiles use for their D50 connection space. Each channel is then clipped to 0–1 and
//! encoded with [`OutputSpace::encode`]. sRGB keeps exactly the functions it always had.
use serde::{Deserialize, Serialize};

/// The colour space of an export's pixels, and of the ICC profile it embeds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OutputSpace {
    #[default]
    Srgb,
    /// Adobe RGB (1998): sRGB's red and blue primaries with a greener green, D65 white,
    /// gamma 563/256.
    AdobeRgb,
    /// ProPhoto RGB as ROMM RGB defines it (ISO 22028-2): D50 white, gamma 1.8 with a
    /// linear segment below 1/512.
    ProPhoto,
}

/// Linear sRGB to linear Adobe RGB (1998), from the two spaces' published
/// chromaticities (both D65).
const SRGB_TO_ADOBE: [[f32; 3]; 3] = [
    [0.715_125_6, 0.284_874_4, 0.],
    [0., 1., 0.],
    [0., 0.041_161_95, 0.958_838_05],
];
/// Linear sRGB (D65) to linear ProPhoto RGB (D50), through XYZ with Bradford's
/// adaptation, from the published chromaticities.
const SRGB_TO_PROPHOTO: [[f32; 3]; 3] = [
    [0.529_280_4, 0.330_153, 0.140_566_61],
    [0.098_366_22, 0.873_463_9, 0.028_169_824],
    [0.016_875_341, 0.117_659_41, 0.865_465_2],
];
/// The Y rows of Adobe RGB's and ProPhoto's RGB-to-XYZ matrices: the luminance of
/// their linear values.
const ADOBE_Y: [f32; 3] = [0.297_344_98, 0.627_363_57, 0.075_291_46];
const PROPHOTO_Y: [f32; 3] = [0.288_071_13, 0.711_843_2, 0.000_085_654];
/// Adobe RGB's gamma as its specification and the profile give it: 2 + 51/256.
const ADOBE_GAMMA: f32 = 563. / 256.;
/// ROMM RGB's linear segment: below 1/512 linear (1/32 encoded) the curve is 16 × v.
const ROMM_LINEAR: f32 = 1. / 512.;
const ROMM_ENCODED: f32 = 1. / 32.;

impl OutputSpace {
    pub const ALL: [Self; 3] = [Self::Srgb, Self::AdobeRgb, Self::ProPhoto];

    /// As the Export dialog names it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Srgb => "sRGB",
            Self::AdobeRgb => "Adobe RGB (1998)",
            Self::ProPhoto => "ProPhoto RGB",
        }
    }
    /// As the command line and the automation socket name it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Srgb => "srgb",
            Self::AdobeRgb => "adobe-rgb",
            Self::ProPhoto => "prophoto",
        }
    }
    /// The space [`OutputSpace::name`] gives `name`.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.name() == name)
    }
    /// Whether 8 bits a channel are too coarse for it: ProPhoto's gamut is so wide
    /// that 8-bit files band, so it is written only as 16-bit TIFF, as Lightroom
    /// recommends.
    pub fn needs_sixteen_bits(self) -> bool {
        self == Self::ProPhoto
    }
    /// Linear sRGB (D65) to this space's linear RGB.
    pub fn from_linear_srgb(self) -> [[f32; 3]; 3] {
        match self {
            Self::Srgb => [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            Self::AdobeRgb => SRGB_TO_ADOBE,
            Self::ProPhoto => SRGB_TO_PROPHOTO,
        }
    }
    /// A linear value (0–1) in this space's transfer curve.
    pub fn encode(self, v: f32) -> f32 {
        match self {
            Self::Srgb => super::srgb_encode(v),
            Self::AdobeRgb => v.max(0.).powf(1. / ADOBE_GAMMA),
            Self::ProPhoto if v < ROMM_LINEAR => 16. * v,
            Self::ProPhoto => v.powf(1. / 1.8),
        }
    }
    /// The linear value of an encoded one (0–1).
    pub fn decode(self, v: f32) -> f32 {
        match self {
            Self::Srgb => super::srgb_decode(v),
            Self::AdobeRgb => v.max(0.).powf(ADOBE_GAMMA),
            Self::ProPhoto if v < ROMM_ENCODED => v / 16.,
            Self::ProPhoto => v.powf(1.8),
        }
    }
    /// Relative luminance of `p` with this space's primaries, in the units of `p`.
    /// sRGB's is [`super::luminance`], with the Rec. 709 weights.
    pub fn luminance(self, p: [f32; 3]) -> f32 {
        let w = match self {
            Self::Srgb => return super::luminance(p),
            Self::AdobeRgb => ADOBE_Y,
            Self::ProPhoto => PROPHOTO_Y,
        };
        w[0] * p[0] + w[1] * p[1] + w[2] * p[2]
    }
    /// Linear sRGB in this space, encoded: converted, each channel clipped to 0–1 on
    /// its own (as Camera Raw's conversion to sRGB does) and encoded. sRGB's is what
    /// the renderer has always written.
    pub fn from_srgb_linear(self, linear: [f32; 3]) -> [f32; 3] {
        let linear = match self {
            Self::Srgb => linear,
            _ => super::mul(self.from_linear_srgb(), linear),
        };
        linear.map(|v| self.encode(v.clamp(0., 1.)).clamp(0., 1.))
    }
    /// An encoded sRGB colour (0–1), such as a watermark's, in this space.
    pub fn from_srgb(self, encoded: [f32; 3]) -> [f32; 3] {
        match self {
            Self::Srgb => encoded,
            _ => self.from_srgb_linear(encoded.map(|v| super::srgb_decode(v.clamp(0., 1.)))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type M = [[f64; 3]; 3];
    fn mul(a: M, b: M) -> M {
        std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
    }
    fn inverse(m: M) -> M {
        let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        std::array::from_fn(|i| {
            std::array::from_fn(|j| {
                let (r0, r1) = ((j + 1) % 3, (j + 2) % 3);
                let (c0, c1) = ((i + 1) % 3, (i + 2) % 3);
                (m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0]) / det
            })
        })
    }
    fn xyz([x, y]: [f64; 2]) -> [f64; 3] {
        [x / y, 1., (1. - x - y) / y]
    }
    /// RGB to XYZ from a space's primaries and white.
    fn to_xyz(primaries: [[f64; 2]; 3], white: [f64; 2]) -> M {
        let p: M = std::array::from_fn(|i| std::array::from_fn(|j| xyz(primaries[j])[i]));
        let s = inverse(p);
        let w = xyz(white);
        let scale: [f64; 3] = std::array::from_fn(|i| (0..3).map(|k| s[i][k] * w[k]).sum());
        std::array::from_fn(|i| std::array::from_fn(|j| p[i][j] * scale[j]))
    }
    const D65: [f64; 2] = [0.3127, 0.3290];
    const D50: [f64; 2] = [0.3457, 0.3585];
    const SRGB: [[f64; 2]; 3] = [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]];
    const ADOBE: [[f64; 2]; 3] = [[0.64, 0.33], [0.21, 0.71], [0.15, 0.06]];
    const ROMM: [[f64; 2]; 3] = [[0.7347, 0.2653], [0.1596, 0.8404], [0.0366, 0.0001]];
    fn bradford(from: [f64; 2], to: [f64; 2]) -> M {
        let b: M = [
            [0.8951, 0.2664, -0.1614],
            [-0.7502, 1.7135, 0.0367],
            [0.0389, -0.0685, 1.0296],
        ];
        let cone = |w: [f64; 2]| -> [f64; 3] {
            let w = xyz(w);
            std::array::from_fn(|i| (0..3).map(|k| b[i][k] * w[k]).sum())
        };
        let (s, d) = (cone(from), cone(to));
        let scale: M =
            std::array::from_fn(|i| std::array::from_fn(|j| if i == j { d[i] / s[i] } else { 0. }));
        mul(inverse(b), mul(scale, b))
    }
    fn close(a: [[f32; 3]; 3], b: M) {
        for (ra, rb) in a.iter().zip(b) {
            for (x, y) in ra.iter().zip(rb) {
                assert!((f64::from(*x) - y).abs() < 1e-7, "{a:?} != {b:?}");
            }
        }
    }

    /// The matrices and luminance weights are those of the published chromaticities.
    #[test]
    fn matrices_follow_the_published_primaries_and_whites() {
        let srgb = to_xyz(SRGB, D65);
        let adobe = to_xyz(ADOBE, D65);
        let romm = to_xyz(ROMM, D50);
        close(SRGB_TO_ADOBE, mul(inverse(adobe), srgb));
        close(
            SRGB_TO_PROPHOTO,
            mul(inverse(romm), mul(bradford(D65, D50), srgb)),
        );
        close(
            [ADOBE_Y, PROPHOTO_Y, ADOBE_Y],
            [adobe[1], romm[1], adobe[1]],
        );
        // White stays white.
        for space in OutputSpace::ALL {
            let white = super::super::mul(space.from_linear_srgb(), [1.; 3]);
            assert!(white.iter().all(|v| (v - 1.).abs() < 1e-6), "{space:?}");
            assert!((space.luminance([1.; 3]) - 1.).abs() < 1e-4, "{space:?}");
        }
    }

    /// sRGB is the renderer's own encoding and weights, untouched.
    #[test]
    fn srgb_is_what_it_always_was() {
        let s = OutputSpace::Srgb;
        for i in 0..=1000 {
            let v = i as f32 / 1000.;
            assert_eq!(
                s.encode(v).to_bits(),
                super::super::srgb_encode(v).to_bits()
            );
            assert_eq!(
                s.decode(v).to_bits(),
                super::super::srgb_decode(v).to_bits()
            );
            let p = [v, 1. - v, v * v];
            assert_eq!(s.luminance(p), super::super::luminance(p));
            assert_eq!(s.from_srgb(p), p);
            assert_eq!(
                s.from_srgb_linear([v * 1.5 - 0.2, v, -v]),
                [v * 1.5 - 0.2, v, -v]
                    .map(|v| super::super::srgb_encode(v.clamp(0., 1.)).clamp(0., 1.))
            );
        }
    }

    /// The curves invert each other, and ROMM's two segments meet at 1/512.
    #[test]
    fn curves_round_trip_and_romm_meets_its_linear_segment() {
        for space in OutputSpace::ALL {
            for i in 0..=4096 {
                let v = i as f32 / 4096.;
                let back = space.decode(space.encode(v));
                assert!((back - v).abs() < 2e-6, "{space:?} {v}: {back}");
            }
        }
        let p = OutputSpace::ProPhoto;
        assert!((p.encode(ROMM_LINEAR) - ROMM_ENCODED).abs() < 1e-7);
        assert!((ROMM_LINEAR.powf(1. / 1.8) - ROMM_ENCODED).abs() < 1e-6);
        assert_eq!(p.encode(0.001), 0.016);
        assert_eq!(OutputSpace::AdobeRgb.encode(0.), 0.);
    }

    #[test]
    fn names_read_back() {
        for space in OutputSpace::ALL {
            assert_eq!(OutputSpace::from_name(space.name()), Some(space));
        }
        assert_eq!(OutputSpace::from_name("display-p3"), None);
    }
}
