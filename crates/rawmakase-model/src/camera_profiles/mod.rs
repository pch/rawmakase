//! DCP forward-matrix profiles. Unsupported matrix-only/HDR/triple-illuminant profiles
//! are rejected explicitly. Copyright and rendering data travel with the recipe.
use crate::{
    camera_data::Metadata,
    color::{mul, srgb_encode},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
type Matrix = [[f32; 3]; 3];
const XYZ_TO_PRO: Matrix = [
    [1.345943, -0.255608, -0.051111],
    [-0.544599, 1.508167, 0.020535],
    [0., 0., 1.211813],
];
pub const PRO_TO_RGB: Matrix = [
    [2.034075, -0.727334, -0.306742],
    [-0.228813, 1.231731, -0.002918],
    [-0.00857, -0.153286, 1.161856],
];
pub const RGB_TO_PRO: Matrix = [
    [0.529345, 0.330072, 0.140583],
    [0.098374, 0.873462, 0.028164],
    [0.016883, 0.117673, 0.865444],
];
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Table {
    dims: [usize; 3],
    data: Vec<[f32; 3]>,
    srgb: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CameraProfile {
    pub name: String,
    pub camera: String,
    pub copyright: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enhanced: Option<enhanced::Enhanced>,
    #[serde(default)]
    color1: Option<Matrix>,
    #[serde(default)]
    calibration_signature: String,
    /// A matrix-only DNG fallback's signature. Separate from the original field
    /// so saved Original white balance retains the old unsigned fallback behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    matrix_calibration_signature: Option<String>,
    #[serde(default)]
    color2: Option<Matrix>,
    forward1: Matrix,
    forward2: Matrix,
    kelvin1: f32,
    kelvin2: f32,
    hue1: Option<Table>,
    hue2: Option<Table>,
    look: Option<Table>,
    tone: Vec<[f32; 2]>,
    exposure: f32,
    #[serde(default, skip_serializing_if = "BlackRender::is_auto")]
    black_render: BlackRender,
}
/// The DNG `DefaultBlackRender` tag: whether the raw converter subtracts its
/// default black (the exposure ramp at Shadows 5) under this profile. Adobe's
/// camera-matching profiles say `None`, which keeps the camera's lifted shadows.
/// A profile made for another camera than the photo's. Expected in a shared
/// library, so callers listing profiles skip it rather than report it.
#[derive(Debug)]
pub struct OtherCamera {
    pub profile: String,
    pub photo: String,
}
impl std::fmt::Display for OtherCamera {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Profile belongs to {}, not {}", self.profile, self.photo)
    }
}
impl std::error::Error for OtherCamera {}

/// An imported look whose base camera profile was not imported with it.
#[derive(Debug)]
pub struct MissingBase {
    /// The look's file name.
    pub file: String,
    /// The base profile it names.
    pub name: String,
}
impl std::fmt::Display for MissingBase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: Missing base camera profile {}. Import its matching base DCP together with the XMP profile",
            self.file, self.name
        )
    }
}
impl std::error::Error for MissingBase {}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum BlackRender {
    #[default]
    Auto,
    None,
}
impl BlackRender {
    fn is_auto(&self) -> bool {
        *self == Self::Auto
    }
}
/// How far `t` sits from the first calibration illuminant towards the second,
/// interpolated in inverse temperature as the DNG specification does.
fn weight(t: f32, kelvin1: f32, kelvin2: f32) -> f32 {
    let d = 1. / kelvin2 - 1. / kelvin1;
    if d.abs() < 1e-8 {
        0.
    } else {
        ((1. / t - 1. / kelvin1) / d).clamp(0., 1.)
    }
}
fn decode(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
impl Table {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.dims[0] > 0
                && self.dims[1] >= 2
                && self.dims[2] > 0
                && self.dims.iter().all(|d| *d <= 256),
            "Invalid profile table dimensions"
        );
        ensure!(
            self.data.len() == self.dims.iter().product::<usize>() && self.data.len() <= 1_000_000,
            "Invalid profile table length"
        );
        ensure!(
            self.data.iter().all(|p| p.iter().all(|v| v.is_finite())
                && p[0].abs() <= 360.
                && (0. ..=10.).contains(&p[1])
                && (0. ..=10.).contains(&p[2])),
            "Invalid profile table value"
        );
        Ok(())
    }
    /// The table's hue shifts and saturation and value scales at `strength` of
    /// their effect (1 unchanged, 0 none).
    fn scaled(&self, strength: f32) -> Self {
        Self {
            data: self
                .data
                .iter()
                .map(|[h, s, v]| {
                    [
                        h * strength,
                        (1. + (s - 1.) * strength).max(0.),
                        (1. + (v - 1.) * strength).max(0.),
                    ]
                })
                .collect(),
            ..self.clone()
        }
    }
    fn lookup(&self, h: f32, s: f32, v: f32) -> [f32; 3] {
        let [nh, ns, nv] = self.dims;
        let coords = [
            h.rem_euclid(1.) * nh as f32,
            s.clamp(0., 1.) * (ns - 1) as f32,
            v.clamp(0., 1.) * (nv - 1) as f32,
        ];
        let base = coords.map(|v| v.floor() as usize);
        let f = std::array::from_fn::<_, 3, _>(|c| coords[c] - base[c] as f32);
        let mut out = [0.; 3];
        for z in 0..2 {
            for y in 0..2 {
                for x in 0..2 {
                    let ix = (base[0] + x) % nh;
                    let iy = (base[1] + y).min(ns - 1);
                    let iz = (base[2] + z).min(nv - 1);
                    let weight = [x, y, z]
                        .into_iter()
                        .enumerate()
                        .map(|(c, n)| if n == 0 { 1. - f[c] } else { f[c] })
                        .product::<f32>();
                    let p = self.data[(iz * nh + ix) * ns + iy];
                    for c in 0..3 {
                        out[c] += p[c] * weight;
                    }
                }
            }
        }
        out
    }
    fn apply(&self, rgb: [f32; 3], other: Option<&Table>, weight: f32) -> [f32; 3] {
        let max = rgb.into_iter().fold(0f32, f32::max);
        let min = rgb.into_iter().fold(f32::INFINITY, f32::min).max(0.);
        let d = max - min;
        let h = if d < 1e-9 {
            0.
        } else if max == rgb[0] {
            ((rgb[1] - rgb[2]) / d).rem_euclid(6.) / 6.
        } else if max == rgb[1] {
            ((rgb[2] - rgb[0]) / d + 2.) / 6.
        } else {
            ((rgb[0] - rgb[1]) / d + 4.) / 6.
        };
        let sat = if max > 0. { d / max } else { 0. };
        let encoded = self.srgb && self.dims[2] > 1;
        let v = if encoded { srgb_encode(max) } else { max };
        let mut delta = self.lookup(h, sat, v);
        if let Some(other) = other {
            let b = other.lookup(h, sat, v);
            for c in 0..3 {
                delta[c] = delta[c] * (1. - weight) + b[c] * weight;
            }
        }
        let h = (h + delta[0] / 360.).rem_euclid(1.) * 6.;
        let s = (sat * delta[1]).clamp(0., 1.);
        // Preserve scene headroom beyond the SDR table domain instead of truncating highlights.
        let v = if encoded {
            decode(v * delta[2])
        } else {
            v * delta[2]
        };
        let c = v * s;
        let x = c * (1. - (h % 2. - 1.).abs());
        let rgb = match h as usize {
            0 => [c, x, 0.],
            1 => [x, c, 0.],
            2 => [0., c, x],
            3 => [0., x, c],
            4 => [x, 0., c],
            _ => [c, 0., x],
        };
        rgb.map(|q| q + v - c)
    }
}
impl CameraProfile {
    /// This profile with its look at a Profile Amount (see `Enhanced::at_amount`).
    pub fn at_amount(&self, amount: f32) -> Self {
        Self {
            enhanced: self.enhanced.as_ref().map(|look| look.at_amount(amount)),
            ..self.clone()
        }
    }
    /// The camera part of the profile, without its look: what the stages before the
    /// per-pixel color stage read.
    pub fn camera_part(self: &std::sync::Arc<Self>) -> std::sync::Arc<Self> {
        if self.enhanced.is_none() {
            return self.clone();
        }
        std::sync::Arc::new(Self {
            enhanced: None,
            ..(**self).clone()
        })
    }
    /// Whether this is a look with Lightroom's Profile Amount.
    pub fn supports_amount(&self) -> bool {
        self.enhanced.as_ref().is_some_and(|e| e.amount.is_some())
    }
    pub fn validate(&self) -> Result<()> {
        if let Some(look) = &self.enhanced {
            look.validate()?;
        }
        ensure!(
            !self.name.is_empty()
                && !self.camera.is_empty()
                && self.name.len() < 1024
                && self.camera.len() < 1024,
            "Invalid profile identity"
        );
        ensure!(
            [self.kelvin1, self.kelvin2]
                .iter()
                .all(|v| (1500. ..=25000.).contains(v)),
            "Unsupported profile illuminant"
        );
        ensure!(
            self.color1
                .iter()
                .chain(self.color2.iter())
                .flatten()
                .flatten()
                .chain(self.forward1.iter().flatten())
                .chain(self.forward2.iter().flatten())
                .all(|v| v.is_finite() && v.abs() < 10.),
            "Invalid profile matrix"
        );
        ensure!(
            self.color2.is_none() || self.color1.is_some(),
            "Second color matrix requires the first"
        );
        for matrix in self.color1.iter().chain(self.color2.iter()) {
            let [a, b, c] = *matrix;
            let determinant = a[0] * (b[1] * c[2] - b[2] * c[1])
                - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0]);
            ensure!(determinant.abs() > 1e-8, "Singular profile color matrix");
        }
        ensure!(
            self.exposure.is_finite() && self.exposure.abs() <= 5.,
            "Invalid profile exposure"
        );
        for t in [&self.hue1, &self.hue2, &self.look].into_iter().flatten() {
            t.validate()?;
        }
        if let (Some(a), Some(b)) = (&self.hue1, &self.hue2) {
            ensure!(
                a.dims == b.dims && a.srgb == b.srgb,
                "Inconsistent profile tables"
            );
        }
        ensure!(
            self.tone.len() >= 2
                && self.tone.len() <= 65536
                && self
                    .tone
                    .iter()
                    .flatten()
                    .all(|v| v.is_finite() && (0. ..=1.).contains(v))
                && self
                    .tone
                    .windows(2)
                    .all(|p| p[0][0] < p[1][0] && p[0][1] <= p[1][1]),
            "Invalid profile tone curve"
        );
        Ok(())
    }
    pub fn ensure_camera(&self, m: &Metadata) -> Result<()> {
        let key = |s: &str| {
            s.to_ascii_lowercase()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        };
        if key(&self.camera) == key(&format!("{} {}", m.make, m.model))
            || key(&self.camera) == key(&m.model)
        {
            return Ok(());
        }
        Err(OtherCamera {
            profile: self.camera.clone(),
            photo: format!("{} {}", m.make, m.model),
        }
        .into())
    }
    fn neutral_calibration(&self, m: &Metadata) -> [f32; 3] {
        if m.baseline_exposure.is_some() {
            m.dng_neutral_calibration
                .as_ref()
                .filter(|c| {
                    c.signature
                        == self
                            .matrix_calibration_signature
                            .as_deref()
                            .unwrap_or(&self.calibration_signature)
                })
                .map_or([1.; 3], |c| c.gains)
        } else if self.calibration_signature == "com.adobe" {
            crate::camera_profiles::reference::neutral_calibration(m)
        } else {
            [1.; 3]
        }
    }
    fn color_matrix(&self, temperature: f32) -> Option<Matrix> {
        let a = self.color1?;
        let b = self.color2.unwrap_or(a);
        let w = self.weight(temperature);
        Some(std::array::from_fn(|i| {
            std::array::from_fn(|j| a[i][j] * (1. - w) + b[i][j] * w)
        }))
    }
    pub fn white_balance(&self, temperature: f32, tint: f32, m: &Metadata) -> Option<[f32; 3]> {
        self.white_balance_calibrated(temperature, tint, m, self.neutral_calibration(m))
    }
    fn white_balance_calibrated(
        &self,
        temperature: f32,
        tint: f32,
        m: &Metadata,
        calibration: [f32; 3],
    ) -> Option<[f32; 3]> {
        let [x, y] = crate::camera_profiles::temperature::xy(temperature, tint);
        let neutral = mul(
            self.color_matrix(temperature)?,
            [x / y, 1., (1. - x - y) / y],
        );
        if neutral.iter().any(|v| !v.is_finite() || *v <= 0.) {
            return None;
        }
        let gains: [f32; 3] =
            std::array::from_fn(|c| 1. / (calibration[c] * neutral[c] * m.wb[c].max(1e-6)));
        Some(gains.map(|v| (v / gains[1]).clamp(0.01, 100.)))
    }
    pub fn as_shot_white_balance(&self, m: &Metadata) -> Option<[f32; 2]> {
        self.as_shot_calibrated(m, self.neutral_calibration(m))
    }
    fn as_shot_calibrated(&self, m: &Metadata, calibration: [f32; 3]) -> Option<[f32; 2]> {
        let neutral = std::array::from_fn(|c| 1. / (m.wb[c].max(1e-6) * calibration[c]));
        let mut xy = [0.3457, 0.3585];
        for pass in 0..30 {
            let temperature = crate::camera_profiles::temperature::from_xy(xy)[0];
            let xyz = mul(
                crate::color::inverse(self.color_matrix(temperature)?),
                neutral,
            );
            let sum: f32 = xyz.iter().sum();
            if sum <= 0. || !sum.is_finite() {
                return None;
            }
            let next = [xyz[0] / sum, xyz[1] / sum];
            let change = (next[0] - xy[0]).abs() + (next[1] - xy[1]).abs();
            xy = if pass == 29 {
                [(xy[0] + next[0]) * 0.5, (xy[1] + next[1]) * 0.5]
            } else {
                next
            };
            if change < 1e-7 {
                break;
            }
        }
        Some(crate::camera_profiles::temperature::from_xy(xy))
    }
    fn weight(&self, t: f32) -> f32 {
        weight(t, self.kelvin1, self.kelvin2)
    }
    pub fn camera_matrix(&self, t: f32) -> Matrix {
        let w = self.weight(t);
        matmul(
            XYZ_TO_PRO,
            std::array::from_fn(|i| {
                std::array::from_fn(|j| self.forward1[i][j] * (1. - w) + self.forward2[i][j] * w)
            }),
        )
    }
    pub fn camera_color(&self, p: [f32; 3], matrix: Matrix, t: f32) -> [f32; 3] {
        let mut rgb = mul(matrix, p);
        if let Some(table) = &self.hue1 {
            rgb = table.apply(rgb, self.hue2.as_ref(), self.weight(t));
        }
        mul(PRO_TO_RGB, rgb).map(|v| v * 2f32.powf(self.exposure))
    }
    /// The profile's look and tone curve: linear ProPhoto RGB from the scene tone stage
    /// in, linear sRGB-primaries display RGB out.
    pub fn finish(&self, pro: [f32; 3]) -> [f32; 3] {
        let mut p = pro;
        if let Some(table) = &self.look {
            p = table.apply(p, None, 0.);
        }
        if let Some(look) = &self.enhanced {
            p = look.apply_table(p);
        }
        let evaluate = |v: f32| {
            let x = v.clamp(0., 1.);
            let i = self
                .tone
                .partition_point(|p| p[0] <= x)
                .saturating_sub(1)
                .min(self.tone.len() - 2);
            let a = self.tone[i];
            let b = self.tone[i + 1];
            let t = ((x - a[0]) / (b[0] - a[0])).clamp(0., 1.);
            a[1] * (1. - t) + b[1] * t
        };
        // Map the darkest and brightest channels; preserve the middle channel's
        // relative position. This preserves hue without flattening saturation.
        p = p.map(|v| v.clamp(0., 1.));
        let low = p.into_iter().fold(f32::INFINITY, f32::min);
        let high = p.into_iter().fold(0., f32::max);
        let a = evaluate(low);
        let b = evaluate(high);
        p = if high - low > 1e-8 {
            p.map(|v| a + (b - a) * (v - low) / (high - low))
        } else {
            [a; 3]
        };
        if let Some(look) = &self.enhanced {
            p = look.apply_curve(p);
        }
        mul(PRO_TO_RGB, p)
    }
}
/// A profile's tables as the GPU develop stage reads them (see `develop::gpu`).
pub struct GpuTables<'a> {
    /// HueSatMap for this temperature: the first table, the second and its weight.
    pub hue: Option<(&'a Table, Option<&'a Table>, f32)>,
    pub look: Option<&'a Table>,
    pub enhanced: Option<&'a Table>,
    pub enhanced_curve: Option<&'a crate::color::curve::CurveLut>,
    pub tone: &'a [[f32; 2]],
    pub exposure_scale: f32,
}
impl Table {
    pub fn dims(&self) -> [usize; 3] {
        self.dims
    }
    pub fn data(&self) -> &[[f32; 3]] {
        &self.data
    }
    pub fn srgb(&self) -> bool {
        self.srgb
    }
}
impl CameraProfile {
    pub fn black_render(&self) -> BlackRender {
        self.black_render
    }
    pub fn gpu_tables(&self, temperature: f32) -> GpuTables<'_> {
        GpuTables {
            hue: self
                .hue1
                .as_ref()
                .map(|t| (t, self.hue2.as_ref(), self.weight(temperature))),
            look: self.look.as_ref(),
            enhanced: self.enhanced.as_ref().and_then(|e| e.table.as_ref()),
            enhanced_curve: self.enhanced.as_ref().map(|e| e.curve.as_ref()),
            tone: &self.tone,
            exposure_scale: 2f32.powf(self.exposure),
        }
    }
    /// RAWmakase Color as a creative look with Lightroom's Profile Amount, named
    /// "Test Creative", for tests.
    #[cfg(any(test, feature = "test-support"))]
    pub fn creative_for_test(m: &Metadata) -> Self {
        let mut p = open::color(m).unwrap();
        p.name = "Test Creative".into();
        p.enhanced.as_mut().unwrap().amount = Some(enhanced::AmountRange {
            table_min: 0.,
            table_max: 2.,
        });
        p
    }
    /// This profile with each kind of RGB table (see `rgb_table::tests::variety`) in
    /// its look, at amounts below, at and above the table's own.
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_test_rgb_tables(self) -> Vec<Self> {
        rgb_table::tests::variety()
            .into_iter()
            .zip([0.7, 1., 1.5])
            .map(|(table, amount)| {
                let mut p = self.clone();
                let look = p.enhanced.as_mut().expect("a profile with a look");
                look.rgb = Some(enhanced::RgbLook::for_test(table, amount));
                p
            })
            .collect()
    }
    /// A profile with hue/saturation, look and enhanced-look tables for GPU tests.
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_test_tables(mut self) -> Self {
        let table = |dims: [usize; 3], srgb: bool, seed: f32| Table {
            dims,
            data: (0..dims.iter().product::<usize>())
                .map(|i| {
                    let f = i as f32 * 0.37 + seed;
                    [
                        f.sin() * 12.,
                        1. + 0.2 * f.cos(),
                        1. + 0.1 * (f * 1.7).sin(),
                    ]
                })
                .collect(),
            srgb,
        };
        self.hue1 = Some(table([90, 30, 1], false, 0.));
        self.hue2 = Some(table([90, 30, 1], false, 1.));
        self.kelvin1 = 2856.;
        self.kelvin2 = 6504.;
        self.look = Some(table([36, 8, 16], true, 2.));
        self.exposure = 0.2;
        self.enhanced = Some(enhanced::Enhanced::for_test(table([24, 6, 8], true, 3.)));
        self
    }
}
/// Bradford adaptation from D65 to the D50 profile connection space.
const D65_TO_D50: Matrix = [
    [1.0478112, 0.0228866, -0.0501270],
    [0.0295424, 0.9904844, -0.0170491],
    [-0.0092345, 0.0150436, 0.7521316],
];
const D65: [f32; 3] = [0.95047, 1., 1.08883];
impl CameraProfile {
    /// The DNG rendering of a file that carries only a ColorMatrix: LibRaw's XYZ-to-camera
    /// matrix, no hue/look tables, and the published ACR 3 default tone curve. This is the
    /// neutral starting point when the user has imported no camera profile.
    pub fn camera_matrix_default(m: &Metadata) -> Option<Self> {
        let cm = m.cam_xyz;
        let [a, b, c] = cm;
        let det = a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
            + a[2] * (b[0] * c[1] - b[1] * c[0]);
        if !det.is_finite() || det.abs() < 1e-8 {
            return None;
        }
        // White-balanced camera values of 1 are the illuminant's neutral, mapped to D50 white.
        let neutral = mul(cm, D65);
        if neutral.iter().any(|v| !v.is_finite() || *v <= 0.) {
            return None;
        }
        let inv = crate::color::inverse(cm);
        let scaled: Matrix =
            std::array::from_fn(|i| std::array::from_fn(|j| inv[i][j] * neutral[j]));
        let forward = matmul(D65_TO_D50, scaled);
        let p = Self {
            name: "Camera Matrix".into(),
            camera: format!("{} {}", m.make, m.model),
            copyright: String::new(),
            enhanced: None,
            color1: Some(cm),
            calibration_signature: String::new(),
            matrix_calibration_signature: m.dng_matrix_profile_signature.clone(),
            color2: None,
            forward1: forward,
            forward2: forward,
            kelvin1: 6504.,
            kelvin2: 6504.,
            hue1: None,
            hue2: None,
            look: None,
            tone: dng_tone::DEFAULT_TONE
                .iter()
                .enumerate()
                .map(|(i, y)| [i as f32 / 1024., *y])
                .collect(),
            exposure: 0.,
            black_render: BlackRender::Auto,
        };
        p.validate().ok()?;
        Some(p)
    }
}
fn matmul(a: Matrix, b: Matrix) -> Matrix {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

mod dcp;
mod enhanced;
mod library;
mod look_settings;
pub mod open;
mod rgb_table;
pub use dcp::{d65_color_matrix, from_bytes};
pub use enhanced::RgbLook;
pub use library::{
    adobe_installed, builtin, compose_look, import_files, installed, library_dirs, load,
};
pub use look_settings::LookSettings;
pub use rgb_table::{Dimensions, Gamut};
#[cfg(test)]
mod tests;

pub(crate) mod dng_tone;
pub(crate) mod temperature;

pub mod reference;
