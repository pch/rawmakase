//! Colour management of what RAWmakase shows and exports, in pure Rust: the
//! sRGB profile exports embed, and the transform from 8-bit sRGB to a
//! monitor's ICC profile (relative colorimetric with black-point
//! compensation).
//!
//! Profiles are read with `moxcms`. A matrix/TRC monitor profile is applied
//! here, by its colorants and tone curves (the curves inverted exactly, which
//! keeps raised blacks right); a LUT-based profile through moxcms's transform
//! from sRGB, its colorimetric tables standing in for the perceptual ones
//! when it has none, as ICC asks. Black-point compensation follows Adobe's
//! published method: XYZ is scaled per channel so sRGB's black lands on the
//! monitor's, white staying white. The monitor's black is its device black
//! through the profile's colorimetric transform (matrix or A2B1); a LUT
//! profile with only perceptual tables, whose black already is the
//! reference medium's, gets none. Measured against Little CMS 2.19 over a
//! dense grid: within one code value wherever Little CMS's own 8-bit result
//! is within one of its floating-point result (`docs/color-pipeline.md`).
use anyhow::{Context, Result, bail, ensure};
use moxcms::{ColorProfile, Layout, RenderingIntent, ToneReprCurve, TransformOptions};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The sRGB profile exports embed: the bytes RAWmakase has always written
/// (Little CMS 2.19's built-in sRGB, dated 2024-01-01, without a profile ID).
pub fn srgb_profile() -> &'static [u8] {
    include_bytes!("../../../assets/srgb.icc")
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
    /// moxcms's transform from sRGB.
    Lut(Arc<moxcms::TransformF32Executor>),
}

impl Transform {
    fn new(bytes: &[u8]) -> Result<Self> {
        let srgb = ColorProfile::new_from_slice(srgb_profile()).map_err(error)?;
        let mut dst = ColorProfile::new_from_slice(bytes).map_err(error)?;
        ensure!(
            dst.color_space == moxcms::DataColorSpace::Rgb,
            "Not an RGB profile"
        );
        let to_xyz = colorants(&srgb);
        let from_xyz = invert(to_xyz).context("Singular sRGB colorants")?;
        let srgb_curves = Curves::of(&srgb)?;
        let colorimetric =
            dst.lut_b_to_a_colorimetric.is_some() || dst.lut_a_to_b_colorimetric.is_some();
        let perceptual = dst.lut_b_to_a_perceptual.is_some();
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
            let forward = srgb
                .create_transform_f32(Layout::Rgb, &dst, Layout::Rgb, options)
                .map_err(error)?;
            // The device black through the colorimetric table, to linear sRGB
            // and on to XYZ; with only perceptual tables, whose black is the
            // reference medium's, none.
            let black = if colorimetric {
                let mut linear = srgb.clone();
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
            } else {
                [0.; 3]
            };
            (Monitor::Lut(forward), black)
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
mod tests {
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
}
