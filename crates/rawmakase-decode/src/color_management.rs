//! Colour management of what RAWmakase shows and exports, in pure Rust: the
//! sRGB profile exports embed, and the transform from 8-bit sRGB to a
//! monitor's ICC profile (relative colorimetric with black-point
//! compensation).
//!
//! Profiles are read with `moxcms`. A matrix/TRC monitor profile is applied
//! here, by its colorants and tone curves (the curves inverted exactly, which
//! keeps raised blacks right), and so is a LUT-based one whose lut8/lut16
//! tables connect through XYZ, such as DisplayCAL's "XYZ LUT + matrix"
//! profiles, evaluated as Little CMS does (moxcms ignores those tables'
//! matrix and miscolours them, issue 548); any other LUT-based profile goes
//! through moxcms's transform from sRGB. Its colorimetric tables fall back to
//! the perceptual ones when it has none, as ICC asks. Black-point
//! compensation follows Adobe's published method: XYZ is scaled per channel
//! so sRGB's black lands on the monitor's, white staying white. The
//! monitor's black is its device black through the profile's colorimetric
//! transform (matrix, or the A2B tables of a profile with A2B1/B2A1 or
//! matrix/TRC tags); a LUT profile with only perceptual tables, whose black
//! already is the reference medium's, gets none. Measured against Little CMS
//! 2.19 over a dense grid: within one code value wherever Little CMS's own
//! 8-bit result is within one of its floating-point result
//! (`docs/color-pipeline.md`).
use anyhow::{Context, Result, bail, ensure};
use moxcms::{
    ColorProfile, DataColorSpace, Layout, LutDataType, LutStore, LutWarehouse, RenderingIntent,
    ToneReprCurve, TransformOptions,
};
use rawmakase_model::color::OutputSpace;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The sRGB profile exports embed: the bytes RAWmakase has always written
/// (Little CMS 2.19's built-in sRGB, dated 2024-01-01, without a profile ID).
pub fn srgb_profile() -> &'static [u8] {
    include_bytes!("../../../assets/srgb.icc")
}

/// The profile an export in `space` embeds: [`srgb_profile`] for sRGB, and for Adobe
/// RGB (1998) and ProPhoto RGB the CC0 `AdobeCompat-v2.icc` and `ProPhoto-v4.icc` of
/// Compact-ICC-Profiles (`licenses/Compact-ICC-Profiles-CC0.txt`), whose colorants and
/// curves are [`OutputSpace`]'s conversion.
pub fn output_profile(space: OutputSpace) -> &'static [u8] {
    match space {
        OutputSpace::Srgb => srgb_profile(),
        OutputSpace::AdobeRgb => include_bytes!("../../../assets/AdobeCompat-v2.icc"),
        OutputSpace::ProPhoto => include_bytes!("../../../assets/ProPhoto-v4.icc"),
    }
}

/// The ICC profile an HDR export in `output` embeds: an ICC v4 profile with the
/// gamut's colorants and D65 white, a `cicp` tag naming the primaries and transfer
/// (which HDR readers use), and the transfer's curve as moxcms approximates it for
/// readers without CICP. A linear export's curve is the identity.
pub fn hdr_profile(output: rawmakase_model::color::hdr_output::HdrOutput) -> Result<Vec<u8>> {
    use moxcms::{CicpColorPrimaries, CicpProfile, MatrixCoefficients, TransferCharacteristics};
    let cicp = CicpProfile {
        color_primaries: CicpColorPrimaries::try_from(output.gamut.cicp())?,
        transfer_characteristics: TransferCharacteristics::try_from(output.transfer.cicp())?,
        matrix_coefficients: MatrixCoefficients::Identity,
        full_range: true,
    };
    let mut profile = ColorProfile::new_from_cicp(cicp);
    ensure!(
        profile.cicp.is_some(),
        "No ICC profile for this HDR encoding"
    );
    profile.description = Some(moxcms::ProfileText::Localizable(vec![
        moxcms::LocalizableString {
            language: "en".into(),
            country: "US".into(),
            value: format!("{} {}", output.gamut.label(), output.transfer.label()),
        },
    ]));
    Ok(profile.encode()?)
}

/// Converts 8-bit sRGB pixels in place to the monitor profile at `path`. On
/// any failure the pixels are left as they were.
pub fn display_transform(path: &Path, data: &mut [u8]) -> Result<()> {
    ensure!(data.len().is_multiple_of(3), "Invalid RGB buffer");
    let transform = cached(path)
        .with_context(|| format!("Cannot use monitor ICC profile {}", path.display()))?;
    let mut out = data.to_vec();
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| transform.apply(&mut out)))
        .map_err(|_| anyhow::anyhow!("Cannot use monitor ICC profile {}", path.display()))??;
    data.copy_from_slice(&out);
    Ok(())
}

/// The transform for the profile at `path`, built once while the file is
/// unchanged.
fn cached(path: &Path) -> Result<Arc<Transform>> {
    type Key = (PathBuf, u64, Option<std::time::SystemTime>);
    static LAST: Mutex<Option<(Key, Arc<Transform>)>> = Mutex::new(None);
    let meta = std::fs::metadata(path)?;
    let key = (path.to_path_buf(), meta.len(), meta.modified().ok());
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((k, t)) = last.as_ref()
        && *k == key
    {
        return Ok(t.clone());
    }
    let bytes = std::fs::read(path)?;
    let t = Arc::new(
        std::panic::catch_unwind(|| Transform::new(&bytes))
            .map_err(|_| anyhow::anyhow!("The profile could not be read"))??,
    );
    *last = Some((key, t.clone()));
    Ok(t)
}

type Mat3 = [[f64; 3]; 3];

/// The PCS white, D50.
const D50: [f64; 3] = [0.9642, 1.0, 0.8249];

/// XYZ in a lut8/lut16 table, whose 0–1 stands for 0–1.99997 (0x8000 is 1.0).
const LUT_XYZ: f64 = 65535. / 32768.;

/// sRGB to a monitor, built from its profile.
struct Transform {
    srgb: Curves,
    /// sRGB to XYZ (D50), the sRGB profile's colorants, and back.
    to_xyz: Mat3,
    from_xyz: Mat3,
    /// The monitor's black in XYZ, which black-point compensation maps sRGB's
    /// black to.
    black: [f64; 3],
    monitor: Monitor,
}

enum Monitor {
    /// XYZ to the monitor's linear RGB, and its tone curves.
    Matrix { from_xyz: Mat3, curves: Curves },
    /// A lut8/lut16 table from XYZ to the monitor's RGB.
    XyzLut(Table),
    /// moxcms's transform from sRGB.
    Lut(Arc<moxcms::TransformF32Executor>),
}

impl Transform {
    fn new(bytes: &[u8]) -> Result<Self> {
        let srgb = ColorProfile::new_from_slice(srgb_profile()).map_err(error)?;
        let mut dst = ColorProfile::new_from_slice(bytes).map_err(error)?;
        ensure!(dst.color_space == DataColorSpace::Rgb, "Not an RGB profile");
        let to_xyz = colorants(&srgb);
        let from_xyz = invert(to_xyz).context("Singular sRGB colorants")?;
        let srgb_curves = Curves::of(&srgb)?;
        let colorimetric =
            dst.lut_b_to_a_colorimetric.is_some() || dst.lut_a_to_b_colorimetric.is_some();
        let perceptual = dst.lut_b_to_a_perceptual.is_some();
        // Little CMS takes a LUT profile's black from its A2B tables also
        // when only its matrix/TRC tags are colorimetric (DisplayCAL's "XYZ
        // LUT + matrix" profiles).
        let colorimetric_black = colorimetric || dst.is_matrix_shaper();
        let (monitor, black) = if !colorimetric && !perceptual {
            // Matrix/TRC.
            let m = colorants(&dst);
            let curves = Curves::of(&dst)?;
            let inverse = invert(m).context("Singular monitor colorants")?;
            let device_black = curves.0.each_ref().map(|c| eval(c, 0.));
            (
                Monitor::Matrix {
                    from_xyz: inverse,
                    curves,
                },
                mul(m, device_black),
            )
        } else {
            // ICC: relative colorimetric falls back to the perceptual tables.
            if dst.lut_a_to_b_colorimetric.is_none() {
                dst.lut_a_to_b_colorimetric = dst.lut_a_to_b_perceptual.clone();
            }
            if dst.lut_b_to_a_colorimetric.is_none() {
                dst.lut_b_to_a_colorimetric = dst.lut_b_to_a_perceptual.clone();
            }
            // Full precision: the defaults' fixed point and low-precision
            // weights lose up to a few code values in the shadows.
            let options = TransformOptions {
                rendering_intent: RenderingIntent::RelativeColorimetric,
                prefer_fixed_point: false,
                interpolation_method: moxcms::InterpolationMethod::Tetrahedral,
                barycentric_weight_scale: moxcms::BarycentricWeightScale::High,
                ..Default::default()
            };
            // lut8/lut16 tables on an XYZ PCS are applied here.
            let xyz = dst.pcs == DataColorSpace::Xyz;
            let xyz_table = |t: Option<&LutWarehouse>| match t {
                Some(LutWarehouse::Lut(t)) if xyz => Some(t.clone()),
                _ => None,
            };
            let monitor = match xyz_table(dst.lut_b_to_a_colorimetric.as_ref()) {
                Some(t) => Monitor::XyzLut(Table::of(&t, true)?),
                None => Monitor::Lut(
                    srgb.create_transform_f32(Layout::Rgb, &dst, Layout::Rgb, options)
                        .map_err(error)?,
                ),
            };
            // The device black through the colorimetric table, to XYZ (through
            // linear sRGB when moxcms applies it); with only perceptual
            // tables, whose black is the reference medium's, none.
            let black = if !colorimetric_black {
                [0.; 3]
            } else if let Some(t) = xyz_table(dst.lut_a_to_b_colorimetric.as_ref()) {
                Table::of(&t, false)?.apply([0.; 3]).map(|v| v * LUT_XYZ)
            } else {
                let mut linear = srgb;
                let one = ToneReprCurve::Parametric(vec![1.0]);
                linear.red_trc = Some(one.clone());
                linear.green_trc = Some(one.clone());
                linear.blue_trc = Some(one);
                let back = dst
                    .create_transform_f32(Layout::Rgb, &linear, Layout::Rgb, options)
                    .map_err(error)?;
                let mut rgb = [0f32; 3];
                back.transform(&[0., 0., 0.], &mut rgb).map_err(error)?;
                mul(to_xyz, rgb.map(f64::from))
            };
            (monitor, black)
        };
        ensure!(
            black.iter().all(|v| v.is_finite() && *v >= 0.) && black[1] < 0.5,
            "Implausible monitor black"
        );
        Ok(Self {
            srgb: srgb_curves,
            to_xyz,
            from_xyz,
            black,
            monitor,
        })
    }

    fn apply(&self, data: &mut [u8]) -> Result<()> {
        let decode: [Vec<f64>; 3] = std::array::from_fn(|c| {
            (0..256)
                .map(|v| eval(&self.srgb.0[c], v as f64 / 255.))
                .collect()
        });
        // XYZ after black-point compensation, per pixel.
        let compensated = |p: &[u8]| -> [f64; 3] {
            let xyz = mul(
                self.to_xyz,
                std::array::from_fn(|c| decode[c][usize::from(p[c])]),
            );
            std::array::from_fn(|c| xyz[c] * (1. - self.black[c] / D50[c]) + self.black[c])
        };
        match &self.monitor {
            Monitor::Matrix { from_xyz, curves } => {
                let inverse = curves.inverses();
                for p in data.as_chunks_mut::<3>().0 {
                    let rgb = mul(*from_xyz, compensated(p));
                    for c in 0..3 {
                        p[c] = quantize(inverse[c].apply(rgb[c]));
                    }
                }
            }
            Monitor::XyzLut(table) => {
                for p in data.as_chunks_mut::<3>().0 {
                    let rgb = table.apply(compensated(p).map(|v| v / LUT_XYZ));
                    for c in 0..3 {
                        p[c] = quantize(rgb[c]);
                    }
                }
            }
            Monitor::Lut(forward) => {
                // Back to encoded sRGB, which moxcms's transform takes.
                let inverse = self.srgb.inverses();
                let mut src = Vec::with_capacity(data.len());
                for p in data.as_chunks::<3>().0 {
                    let rgb = mul(self.from_xyz, compensated(p));
                    src.extend((0..3).map(|c| inverse[c].apply(rgb[c]) as f32));
                }
                let mut out = vec![0f32; src.len()];
                forward.transform(&src, &mut out).map_err(error)?;
                for (d, v) in data.iter_mut().zip(out) {
                    *d = quantize(f64::from(v));
                }
            }
        }
        Ok(())
    }
}

fn error(e: moxcms::CmsError) -> anyhow::Error {
    anyhow::anyhow!("{e:?}")
}

fn quantize(v: f64) -> u8 {
    (v.clamp(0., 1.) * 255.).round() as u8
}

/// A profile's colorants as the columns of RGB to XYZ.
fn colorants(p: &ColorProfile) -> Mat3 {
    let [r, g, b] = [p.red_colorant, p.green_colorant, p.blue_colorant];
    [[r.x, g.x, b.x], [r.y, g.y, b.y], [r.z, g.z, b.z]]
}

fn mul(m: Mat3, v: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|r| (0..3).map(|c| m[r][c] * v[c]).sum())
}

fn invert(m: Mat3) -> Option<Mat3> {
    let c = |a: usize, b: usize, e: usize, f: usize| m[a][b] * m[e][f] - m[a][f] * m[e][b];
    let det = m[0][0] * c(1, 1, 2, 2) - m[0][1] * c(1, 0, 2, 2) + m[0][2] * c(1, 0, 2, 1);
    if det == 0. || !det.is_finite() {
        return None;
    }
    Some([
        [
            c(1, 1, 2, 2) / det,
            -c(0, 1, 2, 2) / det,
            c(0, 1, 1, 2) / det,
        ],
        [
            -c(1, 0, 2, 2) / det,
            c(0, 0, 2, 2) / det,
            -c(0, 0, 1, 2) / det,
        ],
        [
            c(1, 0, 2, 1) / det,
            -c(0, 0, 2, 1) / det,
            c(0, 0, 1, 1) / det,
        ],
    ])
}

/// A lut8/lut16 table (`mft1`/`mft2`) of three channels in and out, as
/// Little CMS evaluates one: the matrix (on an XYZ input only, as ICC asks),
/// the input curves, the CLUT by tetrahedral interpolation, the output
/// curves; every value 0–1, clamped before each curve and the CLUT.
struct Table {
    matrix: Option<Mat3>,
    input: [Vec<f64>; 3],
    grid: usize,
    clut: Vec<f64>,
    output: [Vec<f64>; 3],
}

impl Table {
    fn of(t: &LutDataType, xyz_input: bool) -> Result<Self> {
        ensure!(
            t.num_input_channels == 3 && t.num_output_channels == 3,
            "Not a three-channel table"
        );
        let grid = usize::from(t.num_clut_grid_points);
        ensure!(grid >= 2, "Too few CLUT grid points");
        let values = |s: &LutStore| -> Vec<f64> {
            match s {
                LutStore::Store8(v) => v.iter().map(|v| f64::from(*v) / 255.).collect(),
                LutStore::Store16(v) => v.iter().map(|v| f64::from(*v) / 65535.).collect(),
            }
        };
        let curves = |s: &LutStore, n: u16| -> Result<[Vec<f64>; 3]> {
            let v = values(s);
            let n = usize::from(n);
            ensure!(n >= 2 && v.len() == 3 * n, "Malformed table curves");
            Ok(std::array::from_fn(|c| v[c * n..(c + 1) * n].to_vec()))
        };
        let clut = values(&t.clut_table);
        ensure!(clut.len() == grid.pow(3) * 3, "Malformed CLUT");
        Ok(Self {
            matrix: xyz_input.then_some(t.matrix.v),
            input: curves(&t.input_table, t.num_input_table_entries)?,
            grid,
            clut,
            output: curves(&t.output_table, t.num_output_table_entries)?,
        })
    }

    fn apply(&self, v: [f64; 3]) -> [f64; 3] {
        let v = self.matrix.map_or(v, |m| mul(m, v));
        let v: [f64; 3] = std::array::from_fn(|c| table_curve(&self.input[c], v[c]));
        let v = self.clut(v);
        std::array::from_fn(|c| table_curve(&self.output[c], v[c]))
    }

    fn clut(&self, v: [f64; 3]) -> [f64; 3] {
        let n = self.grid;
        let cell = v.map(|v| {
            let p = v.clamp(0., 1.) * (n - 1) as f64;
            let i = (p.floor() as usize).min(n - 2);
            (i, p - i as f64)
        });
        let node = |a: usize, b: usize, c: usize| -> [f64; 3] {
            let i = (((cell[0].0 + a) * n + cell[1].0 + b) * n + cell[2].0 + c) * 3;
            [self.clut[i], self.clut[i + 1], self.clut[i + 2]]
        };
        let (x, y, z) = (cell[0].1, cell[1].1, cell[2].1);
        let c0 = node(0, 0, 0);
        let d = |a: [f64; 3], b: [f64; 3]| -> [f64; 3] { std::array::from_fn(|c| a[c] - b[c]) };
        let (c1, c2, c3) = if x >= y && y >= z {
            (
                d(node(1, 0, 0), c0),
                d(node(1, 1, 0), node(1, 0, 0)),
                d(node(1, 1, 1), node(1, 1, 0)),
            )
        } else if x >= z && z >= y {
            (
                d(node(1, 0, 0), c0),
                d(node(1, 1, 1), node(1, 0, 1)),
                d(node(1, 0, 1), node(1, 0, 0)),
            )
        } else if z >= x && x >= y {
            (
                d(node(1, 0, 1), node(0, 0, 1)),
                d(node(1, 1, 1), node(1, 0, 1)),
                d(node(0, 0, 1), c0),
            )
        } else if y >= x && x >= z {
            (
                d(node(1, 1, 0), node(0, 1, 0)),
                d(node(0, 1, 0), c0),
                d(node(1, 1, 1), node(1, 1, 0)),
            )
        } else if y >= z && z >= x {
            (
                d(node(1, 1, 1), node(0, 1, 1)),
                d(node(0, 1, 0), c0),
                d(node(0, 1, 1), node(0, 1, 0)),
            )
        } else {
            (
                d(node(1, 1, 1), node(0, 1, 1)),
                d(node(0, 1, 1), node(0, 0, 1)),
                d(node(0, 0, 1), c0),
            )
        };
        std::array::from_fn(|c| c0[c] + c1[c] * x + c2[c] * y + c3[c] * z)
    }
}

/// A table's curve at `x` (clamped to 0–1), interpolated linearly.
fn table_curve(t: &[f64], x: f64) -> f64 {
    let p = x.clamp(0., 1.) * (t.len() - 1) as f64;
    let i = (p.floor() as usize).min(t.len() - 2);
    let f = p - i as f64;
    t[i] * (1. - f) + t[i + 1] * f
}

/// A profile's red, green and blue tone curves (device value to linear).
struct Curves([ToneReprCurve; 3]);

impl Curves {
    fn of(p: &ColorProfile) -> Result<Self> {
        let curve = |c: &Option<ToneReprCurve>| c.clone().context("A tone curve is missing");
        let curves = Self([
            curve(&p.red_trc)?,
            curve(&p.green_trc)?,
            curve(&p.blue_trc)?,
        ]);
        for c in &curves.0 {
            if let ToneReprCurve::Parametric(params) = c
                && !matches!(params.len(), 1 | 3 | 4 | 5 | 7)
            {
                bail!("Unsupported parametric tone curve");
            }
        }
        Ok(curves)
    }
    fn inverses(&self) -> [Inverse; 3] {
        self.0.each_ref().map(Inverse::of)
    }
}

/// An ICC tone curve at `x` (clamped to 0–1): a table interpolated linearly,
/// a gamma, or one of the five parametric functions.
fn eval(c: &ToneReprCurve, x: f64) -> f64 {
    let x = x.clamp(0., 1.);
    match c {
        ToneReprCurve::Lut(t) if t.is_empty() => x,
        ToneReprCurve::Lut(t) if t.len() == 1 => x.powf(f64::from(t[0]) / 256.),
        ToneReprCurve::Lut(t) => {
            let p = x * (t.len() - 1) as f64;
            let i = (p.floor() as usize).min(t.len() - 2);
            let f = p - i as f64;
            (f64::from(t[i]) * (1. - f) + f64::from(t[i + 1]) * f) / 65535.
        }
        ToneReprCurve::Parametric(p) => {
            let q = |i: usize| p.get(i).copied().map_or(0., f64::from);
            let g = q(0);
            match p.len() {
                1 => x.powf(g),
                3 if x >= -q(2) / q(1) => (q(1) * x + q(2)).max(0.).powf(g),
                3 => 0.,
                4 if x >= -q(2) / q(1) => (q(1) * x + q(2)).max(0.).powf(g) + q(3),
                4 => q(3),
                5 if x >= q(4) => (q(1) * x + q(2)).max(0.).powf(g),
                5 => q(3) * x,
                _ if x >= q(4) => (q(1) * x + q(2)).max(0.).powf(g) + q(5),
                _ => q(3) * x + q(6),
            }
        }
    }
}

/// A tone curve's inverse, sampled where curves are steep: at linear values
/// (i / N)⁴, each found by bisection, and interpolated between them.
struct Inverse {
    table: Vec<f64>,
    low: f64,
    high: f64,
}

impl Inverse {
    const N: usize = 4096;
    fn of(c: &ToneReprCurve) -> Self {
        let (low, high) = (eval(c, 0.), eval(c, 1.));
        let solve = |y: f64| {
            let (mut lo, mut hi) = (0f64, 1f64);
            for _ in 0..48 {
                let mid = (lo + hi) / 2.;
                if eval(c, mid) < y {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            (lo + hi) / 2.
        };
        let table = (0..=Self::N)
            .map(|i| solve((i as f64 / Self::N as f64).powi(4)))
            .collect();
        Self { table, low, high }
    }
    /// The device value whose linear value is `y`: 0 below the curve's
    /// black, 1 above its white.
    fn apply(&self, y: f64) -> f64 {
        // Below the curve's black, or not a number.
        if y.is_nan() || y <= self.low {
            return 0.;
        }
        if y >= self.high {
            return 1.;
        }
        let p = y.sqrt().sqrt() * Self::N as f64;
        let i = (p.floor() as usize).min(Self::N - 1);
        let f = p - i as f64;
        self.table[i] * (1. - f) + self.table[i + 1] * f
    }
}

#[cfg(test)]
mod hdr_profile_tests {
    use super::*;
    use rawmakase_model::color::hdr_output::{HdrGamut, HdrOutput, HdrTransfer};

    /// Every HDR encoding's profile reads back with its CICP and RGB colorants.
    #[test]
    fn every_hdr_encoding_has_a_profile_that_reads_back() {
        for gamut in HdrGamut::ALL {
            for transfer in HdrTransfer::ALL {
                let output = HdrOutput {
                    gamut,
                    transfer,
                    gain_map: false,
                };
                let bytes = hdr_profile(output).unwrap();
                let back = ColorProfile::new_from_slice(&bytes).unwrap();
                let cicp = back.cicp.expect("cicp");
                assert_eq!(cicp.color_primaries as u8, gamut.cicp(), "{output:?}");
                assert_eq!(
                    cicp.transfer_characteristics as u8,
                    transfer.cicp(),
                    "{output:?}"
                );
                assert_eq!(back.color_space, DataColorSpace::Rgb);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ColorProfile, Curves, Layout, Mat3, OutputSpace, TransformOptions, colorants, eval, invert,
        output_profile, srgb_profile,
    };
    use std::path::{Path, PathBuf};

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/monitor")
            .join(name)
    }

    /// The 18-level grid the fixtures were made over.
    fn grid() -> Vec<u8> {
        let levels: Vec<u8> = (0..18).map(|i| i * 15).collect();
        let mut rgb = Vec::new();
        for &r in &levels {
            for &g in &levels {
                for &b in &levels {
                    rgb.extend([r, g, b]);
                }
            }
        }
        rgb
    }

    fn sha256(bytes: &[u8]) -> String {
        let sha: [u8; 32] = <sha2::Sha256 as sha2::Digest>::digest(bytes).into();
        sha.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// The wide-gamut profiles are the pinned Compact-ICC-Profiles files, and sRGB's
    /// is the one exports have always embedded.
    #[test]
    fn each_output_space_embeds_its_pinned_profile() {
        assert_eq!(output_profile(OutputSpace::Srgb), srgb_profile());
        assert_eq!(
            sha256(output_profile(OutputSpace::AdobeRgb)),
            "60fb2adecacf82132db0b1c09b303316f3bbd9e2823e7ba096d01627d12d57c9"
        );
        assert_eq!(
            sha256(output_profile(OutputSpace::ProPhoto)),
            "090daf740c136b4a63bf979d64f034b4a65aa5abbb04a0917729222afe2bb5c2"
        );
    }

    /// Each profile's tone curves, as the ICC evaluator here reads them, are
    /// [`OutputSpace`]'s curve within one code value at 16 bits over every code value.
    #[test]
    fn output_profiles_curves_are_the_encoders() {
        for space in [OutputSpace::AdobeRgb, OutputSpace::ProPhoto] {
            let profile = ColorProfile::new_from_slice(output_profile(space)).unwrap();
            let curves = Curves::of(&profile).unwrap();
            let mut worst = 0i32;
            for code in 0..=65535u16 {
                let encoded = f64::from(code) / 65535.;
                for c in &curves.0 {
                    let linear = eval(c, encoded);
                    let ours = (space.encode(linear as f32).clamp(0., 1.) * 65535.).round() as i32;
                    worst = worst.max((ours - i32::from(code)).abs());
                }
            }
            assert!(worst <= 1, "{space:?}: {worst} code values");
        }
    }

    /// The profiles' colorants give [`OutputSpace::from_linear_srgb`] from sRGB's
    /// within their s15.16 rounding, and moxcms's own transform from sRGB to each
    /// profile agrees with the conversion on ramps, primaries and saturated patches.
    #[test]
    fn output_profiles_colorants_and_transforms_are_the_conversion() {
        let srgb = ColorProfile::new_from_slice(srgb_profile()).unwrap();
        let from = colorants(&srgb);
        let mut patches: Vec<[f64; 3]> = (0..=64).map(|i| [f64::from(i) / 64.; 3]).collect();
        for p in [
            [1., 0., 0.],
            [0., 1., 0.],
            [0., 0., 1.],
            [1., 1., 0.],
            [0., 1., 1.],
            [1., 0., 1.],
            [0.8, 0.3, 0.1],
            [0.1, 0.6, 0.3],
            [0.2, 0.3, 0.9],
            [0.05, 0.02, 0.01],
        ] {
            patches.push(p);
        }
        for space in [OutputSpace::AdobeRgb, OutputSpace::ProPhoto] {
            let dst = ColorProfile::new_from_slice(output_profile(space)).unwrap();
            let profile = mul3(invert(colorants(&dst)).unwrap(), from);
            for (row, ours) in profile.iter().zip(space.from_linear_srgb()) {
                for (p, o) in row.iter().zip(ours) {
                    assert!((p - f64::from(o)).abs() < 3e-5, "{space:?}: {profile:?}");
                }
            }
            let transform = srgb
                .create_transform_f64(Layout::Rgb, &dst, Layout::Rgb, TransformOptions::default())
                .unwrap();
            let src: Vec<f64> = patches.iter().flatten().copied().collect();
            let mut out = vec![0.; src.len()];
            transform.transform(&src, &mut out).unwrap();
            for (p, theirs) in patches.iter().zip(out.chunks(3)) {
                let ours = space.from_srgb(p.map(|v| v as f32));
                for (o, t) in ours.iter().zip(theirs) {
                    // Compared linear: near zero a pure gamma curve turns the profile's
                    // rounding into many code values.
                    let (o, t) = (
                        f64::from(space.decode(*o)),
                        f64::from(space.decode(*t as f32)),
                    );
                    assert!(
                        (o - t).abs() < 1e-4,
                        "{space:?} {p:?}: {ours:?} vs {theirs:?}"
                    );
                }
            }
        }
    }

    fn mul3(a: Mat3, b: Mat3) -> Mat3 {
        std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
    }

    #[test]
    fn the_srgb_profile_is_the_one_exports_have_always_embedded() {
        let profile = super::srgb_profile();
        assert_eq!(profile.len(), 588);
        let date: Vec<u16> = profile[24..36]
            .chunks(2)
            .map(|b| u16::from_be_bytes([b[0], b[1]]))
            .collect();
        assert_eq!(date, [2024, 1, 1, 0, 0, 0]);
        // No profile ID, which the date would have to be hashed into.
        assert!(profile[84..100].iter().all(|b| *b == 0));
        let sha: [u8; 32] = <sha2::Sha256 as sha2::Digest>::digest(profile).into();
        let hex: String = sha.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "d68baed8428c7b500c33a60da52a08a845b4bf4ae4745cbda046811e2b0eb3c9"
        );
    }

    /// How `rgb` (the transform of `grid`) compares with Little CMS's 8-bit
    /// result `eight` and floating-point result `float` (0–255): the samples
    /// that miss (more than one code value from the 8-bit result, and, where
    /// that is itself more than one from the floating-point result, more than
    /// one and a half from the latter; in a gamma curve's toe, where a code
    /// value is a few millionths of white, more than 0.005% of white apart),
    /// and the largest distance from the floating-point result.
    fn compare(rgb: &[u8], eight: &[u8], float: &[f64]) -> (usize, f64) {
        let light = |v: f64| (v / 255.).powf(2.2);
        let mut misses = 0;
        let mut worst = 0f64;
        for ((&ours, &lcms), &exact) in rgb.iter().zip(eight).zip(float) {
            let near = (i16::from(ours) - i16::from(lcms)).abs() <= 1;
            let toe = (light(f64::from(ours)) - light(exact)).abs() <= 5e-5;
            let lcms_rounds = (f64::from(lcms) - exact).abs() > 1.;
            let near_float = (f64::from(ours) - exact).abs() <= 1.5;
            misses += usize::from(!(near || toe || (lcms_rounds && near_float)));
            worst = worst.max((f64::from(ours) - exact).abs());
        }
        (misses, worst)
    }

    /// Against Little CMS 2.19 (relative colorimetric, black-point
    /// compensation) on synthetic profiles. This holds the measured state, not
    /// the goal of agreeing with Little CMS's 8-bit result within one code
    /// value everywhere, which is not met: matrix/TRC profiles miss it in a few
    /// shadow samples where Little CMS's own 8-bit result is further from its
    /// exact one than ours; LUT profiles, which moxcms resamples, in at most
    /// 0.05% of samples (saturated shadows), by under four code values here.
    /// See docs/color-pipeline.md.
    #[test]
    fn monitor_transforms_match_little_cms() {
        for name in [
            "adobe-rgb-v2",
            "display-p3-v4",
            "raised-black-matrix-v4",
            "raised-black-lut-v2",
            "raised-black-lut-v4",
            "raised-black-lut1-v2",
            "raised-black-lut1-v4",
        ] {
            let mut rgb = grid();
            super::display_transform(&fixture(&format!("{name}.icc")), &mut rgb).unwrap();
            let eight = std::fs::read(fixture(&format!("{name}.lcms8"))).unwrap();
            let float: Vec<f64> = std::fs::read(fixture(&format!("{name}.lcms16")))
                .unwrap()
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| f64::from(u16::from_le_bytes(*b)) / 257.)
                .collect();
            let (misses, worst) = compare(&rgb, &eight, &float);
            assert!(
                misses * 2000 <= rgb.len() && worst < 4.,
                "{name}: {misses} of {} samples miss, worst {worst:.2}",
                rgb.len()
            );
        }
    }

    /// The same over the dense 52-level grid (0, 5 … 255) for every profile
    /// in `$RAWMAKASE_ICC_GATE` (`<name>.icc`, Little CMS's `<name>.lcms.bin`
    /// and its floating-point `<name>.lcms.bin.f32`), such as a machine's
    /// own monitor profiles; prints the result per profile.
    #[test]
    #[ignore]
    fn dense_gate_against_little_cms() {
        let Some(dir) = std::env::var_os("RAWMAKASE_ICC_GATE") else {
            return;
        };
        let levels: Vec<u8> = (0..52).map(|i| i * 5).collect();
        let mut grid = Vec::new();
        for &r in &levels {
            for &g in &levels {
                for &b in &levels {
                    grid.extend([r, g, b]);
                }
            }
        }
        let mut entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
        entries.sort();
        for icc in entries
            .iter()
            .filter(|p| p.extension().is_some_and(|e| e == "icc"))
        {
            let bin = icc.with_extension("lcms.bin");
            let Ok(eight) = std::fs::read(&bin) else {
                continue;
            };
            let float: Vec<f64> = std::fs::read(bin.with_extension("bin.f32"))
                .unwrap()
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f64::from(f32::from_le_bytes(*b).clamp(0., 1.)) * 255.)
                .collect();
            let mut rgb = grid.clone();
            super::display_transform(icc, &mut rgb).unwrap();
            let (misses, worst) = compare(&rgb, &eight, &float);
            eprintln!(
                "{}: {misses} of {} miss ({:.4}%), worst {worst:.2} from Little CMS's float result",
                icc.file_name().unwrap().to_string_lossy(),
                rgb.len(),
                misses as f64 * 100. / rgb.len() as f64
            );
        }
    }

    #[test]
    fn srgb_to_srgb_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("srgb.icc");
        std::fs::write(&p, super::srgb_profile()).unwrap();
        let mut rgb = grid();
        let before = rgb.clone();
        super::display_transform(&p, &mut rgb).unwrap();
        for (a, b) in rgb.iter().zip(before) {
            assert!((i16::from(*a) - i16::from(b)).abs() <= 1);
        }
    }

    #[test]
    fn unusable_profiles_are_errors_and_leave_the_pixels_alone() {
        let dir = tempfile::tempdir().unwrap();
        let good = std::fs::read(fixture("raised-black-lut-v2.icc")).unwrap();
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("empty", Vec::new()),
            ("text", b"not a profile".to_vec()),
            ("truncated", good[..good.len() / 3].to_vec()),
            ("header only", good[..128].to_vec()),
        ];
        for (name, bytes) in cases {
            let p = dir.path().join(format!("{name}.icc"));
            std::fs::write(&p, bytes).unwrap();
            let mut rgb = grid();
            let before = rgb.clone();
            assert!(super::display_transform(&p, &mut rgb).is_err(), "{name}");
            assert_eq!(rgb, before, "{name}");
        }
        let mut rgb = grid();
        assert!(super::display_transform(&dir.path().join("missing.icc"), &mut rgb).is_err());
        assert!(super::display_transform(&fixture("display-p3-v4.icc"), &mut [0u8; 4]).is_err());
    }

    /// A version 2 display profile with an XYZ PCS and `tags`, written here
    /// so tests need no third-party profile.
    fn icc_v2(tags: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let table = 4 + 12 * tags.len();
        let mut body = Vec::new();
        let mut entries = Vec::new();
        for (sig, data) in tags {
            while body.len() % 4 != 0 {
                body.push(0);
            }
            entries.extend_from_slice(*sig);
            entries.extend(((128 + table + body.len()) as u32).to_be_bytes());
            entries.extend((data.len() as u32).to_be_bytes());
            body.extend(data);
        }
        let mut p = vec![0u8; 128];
        let size = (128 + table + body.len()) as u32;
        p[..4].copy_from_slice(&size.to_be_bytes());
        p[8..12].copy_from_slice(&[2, 0x20, 0, 0]);
        p[12..16].copy_from_slice(b"mntr");
        p[16..20].copy_from_slice(b"RGB ");
        p[20..24].copy_from_slice(b"XYZ ");
        p[36..40].copy_from_slice(b"acsp");
        p[68..80].copy_from_slice(&xyz_tag(super::D50)[8..]);
        p.extend((tags.len() as u32).to_be_bytes());
        p.extend(entries);
        p.extend(body);
        p
    }

    fn s15(v: f64) -> [u8; 4] {
        ((v * 65536.).round() as i32).to_be_bytes()
    }

    fn u16s(values: impl IntoIterator<Item = f64>) -> Vec<u8> {
        values
            .into_iter()
            .flat_map(|v| ((v.clamp(0., 1.) * 65535.).round() as u16).to_be_bytes())
            .collect()
    }

    fn xyz_tag(v: [f64; 3]) -> Vec<u8> {
        let mut t = b"XYZ \0\0\0\0".to_vec();
        t.extend(v.iter().flat_map(|c| s15(*c)));
        t
    }

    fn curv_tag(n: usize, f: impl Fn(f64) -> f64) -> Vec<u8> {
        let mut t = b"curv\0\0\0\0".to_vec();
        t.extend((n as u32).to_be_bytes());
        t.extend(u16s((0..n).map(|i| f(i as f64 / (n - 1) as f64))));
        t
    }

    /// A lut16Type (`mft2`) with three inputs and outputs: `matrix`, input
    /// curves of `inputs` entries, a CLUT of `grid` points per side, output
    /// curves of `outputs` entries.
    fn mft2(
        matrix: super::Mat3,
        inputs: usize,
        input: impl Fn(f64) -> f64,
        grid: usize,
        clut: impl Fn([f64; 3]) -> [f64; 3],
        outputs: usize,
        output: impl Fn(f64) -> f64,
    ) -> Vec<u8> {
        let mut t = b"mft2\0\0\0\0".to_vec();
        t.extend([3, 3, grid as u8, 0]);
        t.extend(matrix.iter().flatten().flat_map(|v| s15(*v)));
        t.extend((inputs as u16).to_be_bytes());
        t.extend((outputs as u16).to_be_bytes());
        let step = |n: usize| move |i: usize| i as f64 / (n - 1) as f64;
        for _ in 0..3 {
            t.extend(u16s((0..inputs).map(step(inputs)).map(&input)));
        }
        for i in 0..grid.pow(3) {
            let at = [i / (grid * grid), i / grid % grid, i % grid].map(step(grid));
            t.extend(u16s(clut(at)));
        }
        for _ in 0..3 {
            t.extend(u16s((0..outputs).map(step(outputs)).map(&output)));
        }
        t
    }

    /// DisplayCAL's default profile type, "XYZ LUT + matrix" (issue 548): a
    /// matrix/TRC profile whose lut16 A2B and B2A tables, on an XYZ PCS,
    /// describe the same monitor. sRGB through it must land where it does
    /// through the matrix/TRC tags alone, which the tests above hold to
    /// Little CMS. Two layouts: a B2A whose matrix takes XYZ to linear RGB
    /// (what ICC applies to an XYZ input before the input curves), and a
    /// B2A whose CLUT does, with colorimetric tables and a raised black.
    #[test]
    fn xyz_lut_display_profiles_render_like_their_matrix() {
        // Wide-gamut colorants (D50) and an sRGB-like curve.
        let m: super::Mat3 = [
            [0.5151, 0.2920, 0.1571],
            [0.2412, 0.6922, 0.0666],
            [-0.0011, 0.0419, 0.7841],
        ];
        let m_inv = super::invert(m).unwrap();
        let identity = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
        // XYZ in a lut16 table: 1.0 is 0x8000.
        let k = 65535. / 32768.;
        let srgb = |v: f64| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let srgb_inverse = |y: f64| {
            if y <= 0.0031308 {
                y * 12.92
            } else {
                1.055 * y.powf(1. / 2.4) - 0.055
            }
        };
        let dir = tempfile::tempdir().unwrap();
        for (name, black, matrix_b2a) in [("matrix-b2a", 0.002, true), ("clut-b2a", 0.003, false)] {
            let trc = move |v: f64| black + (1. - black) * srgb(v);
            let trc_inverse = move |y: f64| srgb_inverse(((y - black) / (1. - black)).max(0.));
            let a2b = mft2(
                identity,
                4096,
                trc,
                17,
                |rgb| super::mul(m, rgb).map(|v| v / k),
                2,
                |v| v,
            );
            let b2a = if matrix_b2a {
                let to_rgb = m_inv.map(|row| row.map(|v| v * k));
                mft2(to_rgb, 2, |v| v, 2, |rgb| rgb, 4096, trc_inverse)
            } else {
                // Linear RGB over the whole XYZ cube, offset and scaled into
                // the CLUT's 0–1 and back by the output curves, so that
                // interpolation is exact.
                let lo = m_inv.iter().flatten().map(|v| v.min(0.) * k).sum::<f64>();
                let hi = m_inv.iter().flatten().map(|v| v.max(0.) * k).sum::<f64>();
                mft2(
                    identity,
                    2,
                    |v| v,
                    17,
                    |xyz| super::mul(m_inv, xyz.map(|v| v * k)).map(|v| (v - lo) / (hi - lo)),
                    4096,
                    move |v| trc_inverse((lo + v * (hi - lo)).clamp(0., 1.)),
                )
            };
            let mut tags = vec![
                (b"wtpt", xyz_tag(super::D50)),
                (b"rXYZ", xyz_tag([m[0][0], m[1][0], m[2][0]])),
                (b"gXYZ", xyz_tag([m[0][1], m[1][1], m[2][1]])),
                (b"bXYZ", xyz_tag([m[0][2], m[1][2], m[2][2]])),
                (b"rTRC", curv_tag(4096, trc)),
                (b"gTRC", curv_tag(4096, trc)),
                (b"bTRC", curv_tag(4096, trc)),
            ];
            let matrix = dir.path().join(format!("{name}-matrix.icc"));
            std::fs::write(&matrix, icc_v2(&tags)).unwrap();
            if matrix_b2a {
                tags.extend([(b"A2B0", a2b), (b"B2A0", b2a)]);
            } else {
                tags.extend([
                    (b"A2B0", a2b.clone()),
                    (b"B2A0", b2a.clone()),
                    (b"A2B1", a2b),
                    (b"B2A1", b2a),
                ]);
            }
            let lut = dir.path().join(format!("{name}.icc"));
            std::fs::write(&lut, icc_v2(&tags)).unwrap();

            let mut expected = grid();
            super::display_transform(&matrix, &mut expected).unwrap();
            let mut rgb = grid();
            super::display_transform(&lut, &mut rgb).unwrap();
            let worst = rgb
                .iter()
                .zip(&expected)
                .map(|(a, b)| (i16::from(*a) - i16::from(*b)).abs())
                .max()
                .unwrap();
            assert!(worst <= 1, "{name}: up to {worst} code values off");
        }
    }
}
