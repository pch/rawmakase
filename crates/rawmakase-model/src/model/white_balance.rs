//! Fallback white balance for cameras without calibrated DCP matrices, and Lightroom's
//! named white balance presets.
use crate::{
    camera_data::Metadata,
    color::{inverse, mul},
};
pub fn illuminant_camera(t: f32, m: &Metadata) -> [f32; 3] {
    // Planckian locus approximation in CIE xy; extrapolation avoided by UI range.
    let t = t.clamp(2000., 15000.);
    let x = if t <= 4000. {
        -0.2661239e9 / t.powi(3) - 0.234358e6 / t.powi(2) + 0.8776956e3 / t + 0.179910
    } else {
        -3.0258469e9 / t.powi(3) + 2.107038e6 / t.powi(2) + 0.2226347e3 / t + 0.240390
    };
    let y = if t <= 2222. {
        -1.1063814 * x.powi(3) - 1.3481102 * x * x + 2.1855583 * x - 0.2021968
    } else if t <= 4000. {
        -0.9549476 * x.powi(3) - 1.3741859 * x * x + 2.091_37 * x - 0.1674887
    } else {
        3.081758 * x.powi(3) - 5.873_387 * x * x + 3.75113 * x - 0.3700148
    };
    let rgb = mul(
        [
            [3.2404542, -1.5371385, -0.4985314],
            [-0.969266, 1.8760108, 0.041556],
            [0.0556434, -0.2040259, 1.0572252],
        ],
        [x / y, 1., (1. - x - y) / y],
    );
    let balanced = mul(inverse(m.matrix), rgb);
    std::array::from_fn(|c| balanced[c] / m.daylight_wb[c])
}
pub fn estimate_temperature(m: &Metadata) -> f32 {
    (2000..=15000)
        .step_by(50)
        .min_by(|a, b| {
            let error = |t: i32| {
                let p = illuminant_camera(t as f32, m);
                (0..3)
                    .map(|c| (p[1] / p[c].max(0.01) / m.wb[c]).ln().powi(2))
                    .sum::<f32>()
            };
            error(*a).total_cmp(&error(*b))
        })
        .unwrap_or(6500) as f32
}

/// Lightroom's named white balance presets for RAW files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedWhiteBalance {
    Daylight,
    Cloudy,
    Shade,
    Tungsten,
    Fluorescent,
    Flash,
}

/// A white balance as Temperature (kelvin) and Tint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TemperatureTint {
    pub temperature: f32,
    pub tint: f32,
}

impl NamedWhiteBalance {
    pub const ALL: [NamedWhiteBalance; 6] = [
        NamedWhiteBalance::Daylight,
        NamedWhiteBalance::Cloudy,
        NamedWhiteBalance::Shade,
        NamedWhiteBalance::Tungsten,
        NamedWhiteBalance::Fluorescent,
        NamedWhiteBalance::Flash,
    ];
    /// The menu name, also Lightroom's `WhiteBalance` value.
    pub fn name(self) -> &'static str {
        match self {
            NamedWhiteBalance::Daylight => "Daylight",
            NamedWhiteBalance::Cloudy => "Cloudy",
            NamedWhiteBalance::Shade => "Shade",
            NamedWhiteBalance::Tungsten => "Tungsten",
            NamedWhiteBalance::Fluorescent => "Fluorescent",
            NamedWhiteBalance::Flash => "Flash",
        }
    }
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|w| w.name() == name)
    }
    /// The values Lightroom sets for RAW files.
    pub fn values(self) -> TemperatureTint {
        let (temperature, tint) = match self {
            NamedWhiteBalance::Daylight => (5500., 10.),
            NamedWhiteBalance::Cloudy => (6500., 10.),
            NamedWhiteBalance::Shade => (7500., 10.),
            NamedWhiteBalance::Tungsten => (2850., 0.),
            NamedWhiteBalance::Fluorescent => (3800., 21.),
            NamedWhiteBalance::Flash => (5500., 0.),
        };
        TemperatureTint { temperature, tint }
    }
}

/// Lightroom's Temp for a rendered file (`IncrementalTemperature`, −100…100) as the
/// DNG SDK's reciprocal temperature (mired) of the white it names: `[Temp, mired]`.
/// Measured on Camera Raw 18.7 renders of synthetic JPEG charts
/// (`scripts/corpus/rendered-white-balance.py`). At 0 it is the file's D50 white
/// (199.9 mired), reaching 500.0 mired (2000 K) at −100; Temp moves only temperature.
const RENDERED_TEMPERATURE: [[f32; 2]; 27] = [
    [-100., 500.0],
    [-90., 464.0],
    [-75., 413.7],
    [-60., 366.5],
    [-50., 336.1],
    [-35., 291.7],
    [-25., 263.8],
    [-15., 237.3],
    [-10., 224.6],
    [-5., 212.1],
    [0., 199.9],
    [5., 188.1],
    [10., 176.6],
    [15., 165.3],
    [25., 143.7],
    [35., 123.3],
    [40., 113.5],
    [45., 104.0],
    [50., 94.7],
    [55., 85.8],
    [60., 77.1],
    [65., 68.8],
    [70., 60.7],
    [75., 52.9],
    [80., 45.3],
    [90., 31.1],
    [100., 17.9],
];
/// Lightroom's Tint for a rendered file (`IncrementalTint`, −100…100) as the DNG
/// tint of the white it names: D50's tint (+9.6) at 0, reaching ±150 at ±100 along a
/// quadratic curve. Matches unbiased Camera Raw 18.7 JPEG and linear-DNG measurements
/// within ~0.2 tint units across −100…100 (`scripts/corpus/rendered-white-balance.py`),
/// avoiding the `rgb_tone` bias that previously pushed +100 to +154.41 and −100 to
/// −151.11.
fn rendered_dng_tint(tint: f32) -> f32 {
    let u = (tint / 100.).clamp(-1., 1.);
    let t = 0.5 * u * (1. + u.abs());
    9.6 * (1. - t * t) + 150. * t
}

/// `rows` (sorted by their first value) at `x`, linearly between rows, clamped to the ends.
fn interpolate<const K: usize>(rows: &[[f32; K]], x: f32) -> [f32; K] {
    let x = x.clamp(rows[0][0], rows[rows.len() - 1][0]);
    let i = rows
        .windows(2)
        .position(|w| x <= w[1][0])
        .unwrap_or(rows.len() - 2);
    let (a, b) = (rows[i], rows[i + 1]);
    let t = (x - a[0]) / (b[0] - a[0]);
    std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t)
}

/// The Bradford cone response matrix, from XYZ.
const BRADFORD: [[f32; 3]; 3] = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];

/// The XYZ (Y = 1) of the white at `mired`, `tint`, its xy held inside the range of
/// real ones as the DNG SDK's `XYtoXYZ` holds them.
fn white_xyz(mired: f32, tint: f32) -> [f32; 3] {
    let [x, y] = crate::camera_profiles::temperature::xy_reciprocal(mired, tint);
    let (mut x, mut y) = (x.clamp(1e-6, 1. - 1e-6), y.clamp(1e-6, 1. - 1e-6));
    if x + y > 1. - 1e-6 {
        let scale = (1. - 1e-6) / (x + y);
        (x, y) = (x * scale, y * scale);
    }
    [x / y, 1., (1. - x - y) / y]
}

/// The DNG SDK's `MapWhiteMatrix`: Bradford adaptation from white `from` to white `to`
/// (XYZ), each cone's response taken as at least 0 and its scaling held to 0.1…10.
pub(crate) fn map_white(from: [f32; 3], to: [f32; 3]) -> [[f32; 3]; 3] {
    use crate::color::{inverse, mul, mul_matrix};
    let (from, to) = (mul(BRADFORD, from), mul(BRADFORD, to));
    let scale: [f32; 3] = std::array::from_fn(|c| {
        let (from, to) = (from[c].max(0.), to[c].max(0.));
        if from > 0. { to / from } else { 10. }.clamp(0.1, 10.)
    });
    let diagonal =
        std::array::from_fn(|i| std::array::from_fn(|j| if i == j { scale[i] } else { 0. }));
    mul_matrix(inverse(BRADFORD), mul_matrix(diagonal, BRADFORD))
}

/// Minimum channel of the normalized camera white in Camera Raw 18.7, measured on
/// matrix-only linear DNG renders in the cool magenta corner.
const CAMERA_WHITE_FLOOR: f32 = 0.001;
/// Maximum per-channel white-balance gain (4 stops) Camera Raw applies before the
/// camera-to-PCS matrix. Where `camera_white[c] < 1 / MAX_WB_GAIN`, capping the gain
/// at `MAX_WB_GAIN` scales column `c` of the matrix by `camera_white[c] * MAX_WB_GAIN`.
const MAX_WB_GAIN: f32 = 16.;

/// The DNG SDK's camera to ProPhoto matrix, and camera white, for a camera whose RGB
/// is linear ProPhoto and has no forward matrix (Lightroom's view of a rendered file's
/// pseudo-scene), under `white` (XYZ): ProPhoto to camera is the color matrix after
/// adaptation from D50 to the white, scaled so its largest channel of D50 is 1; the
/// camera white is the white in camera RGB, its largest channel 1 and none below
/// [`CAMERA_WHITE_FLOOR`]. When `cap_gain` is true (rendering), each column `c` of the
/// matrix is scaled by `(camera_white[c] * MAX_WB_GAIN).clamp(0., 1.)`, matching
/// Camera Raw's 16× gain cap in the cool magenta corner while leaving normal settings
/// untouched; `rendered_controls` passes `false` so strongly blue picks warm toward
/// positive Temp instead of folding into the capped corner.
fn camera_to_prophoto(white: [f32; 3], cap_gain: bool) -> ([[f32; 3]; 3], [f32; 3]) {
    use crate::camera_profiles::XYZ_TO_PRO;
    use crate::color::{inverse, mul, mul_matrix};
    let d50 = mul(inverse(XYZ_TO_PRO), [1.; 3]);
    let to_camera = mul_matrix(XYZ_TO_PRO, map_white(d50, white));
    let largest = |v: [f32; 3]| v.into_iter().fold(f32::NEG_INFINITY, f32::max);
    let scale = largest(mul(to_camera, d50));
    let to_camera = to_camera.map(|row| row.map(|v| v / scale));
    let camera_white = mul(XYZ_TO_PRO, white);
    let camera_white =
        camera_white.map(|v| (v / largest(camera_white)).clamp(CAMERA_WHITE_FLOOR, 1.));
    let matrix = mul_matrix(XYZ_TO_PRO, inverse(to_camera));
    let matrix = if cap_gain {
        let wb_scale = camera_white.map(|v| (v * MAX_WB_GAIN).clamp(0., 1.));
        std::array::from_fn(|i| std::array::from_fn(|j| matrix[i][j] * wb_scale[j]))
    } else {
        matrix
    };
    (matrix, camera_white)
}

/// Shared matrix and camera-white builder for [`rendered_white_balance`] (`cap_gain = true`)
/// and [`rendered_controls`] (`cap_gain = false`).
fn rendered_wb_matrix(temperature: f32, tint: f32, cap_gain: bool) -> ([[f32; 3]; 3], [f32; 3]) {
    if temperature == 0. && tint == 0. {
        return ([[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]], [1.; 3]);
    }
    let white = |temperature: f32, tint: f32| {
        let [_, mired] = interpolate(&RENDERED_TEMPERATURE, temperature);
        white_xyz(mired, rendered_dng_tint(tint))
    };
    let (file, _) = camera_to_prophoto(white(0., 0.), false);
    let (matrix, camera_white) = camera_to_prophoto(white(temperature, tint), cap_gain);
    (
        crate::color::mul_matrix(matrix, crate::color::inverse(file)),
        camera_white,
    )
}

/// A rendered file's white balance as Lightroom applies it to the scene its rendering
/// is of (`develop::rendered`): each channel of the scene held at most at `clip`, then
/// `matrix`, linear ProPhoto to linear ProPhoto.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderedWhiteBalance {
    pub matrix: [[f32; 3]; 3],
    /// `f32::MAX` where nothing is held.
    pub clip: [f32; 3],
}

/// Where a rendered file's scene stands against the camera white Lightroom clips it
/// at: the file's white (about 1 in the scene) times this. Measured on Camera Raw 18.7
/// renders across strong cooling and the cool magenta transition: at 4 the brightest
/// greys and near-whites at Temp −25, Tint +100 (sRGB 208–224/255) clip blue too early;
/// 6 keeps those levels intact and drops blue at 240/255 as Camera Raw does.
const CAMERA_WHITE_SCALE: f32 = 6.;

/// A rendered file's white balance at its Temp and Tint (−100…100): what the DNG SDK
/// does to a camera whose RGB is linear ProPhoto ([`camera_to_prophoto`]) under the
/// white they name, against the file's own D50 white. Each channel is held at
/// [`CAMERA_WHITE_SCALE`] times the effective camera white (floored at `1 / MAX_WB_GAIN`
/// by the 16× gain cap) before the matrix. The file as it is at 0, 0.
pub fn rendered_white_balance(temperature: f32, tint: f32) -> RenderedWhiteBalance {
    if temperature == 0. && tint == 0. {
        return RenderedWhiteBalance {
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            clip: [f32::MAX; 3],
        };
    }
    let (matrix, camera_white) = rendered_wb_matrix(temperature, tint, true);
    RenderedWhiteBalance {
        matrix,
        clip: camera_white.map(|c| c.max(1. / MAX_WB_GAIN) * CAMERA_WHITE_SCALE),
    }
}

/// The ProPhoto channel gains greys below [`rendered_white_balance`]'s clip get at a
/// rendered file's Temp and Tint, clamped at 0 where the cool magenta corner's 16× gain
/// cap leaves blue below 0. Exactly `[1, 1, 1]` at 0, 0.
pub fn rendered_gains(temperature: f32, tint: f32) -> [f32; 3] {
    crate::color::mul(rendered_white_balance(temperature, tint).matrix, [1.; 3]).map(|v| v.max(0.))
}

/// The rendered file's Temp and Tint that render most neutral the color `gains`
/// would (the picked color is `1 / gains`): least squares on the log2 spread of its
/// channels after the illuminant matrix, whose cross-channel terms a comparison of
/// gains would miss, over the uncapped domain (`camera_white >= 1 / MAX_WB_GAIN`)
/// within −100…100: a coarse grid, then an 8-direction pattern search to 0.01.
/// Restricting the search to the uncapped domain prevents strongly blue picks from
/// folding into the 16×-capped cool-magenta corner and keeps very yellow picks (>4
/// stops short of blue) on the 16× maximum-blue-gain boundary instead of falling into
/// the zero-blue corner. A color no Temp and Tint neutralise (an eyedropper's, say)
/// gives the nearest that do.
pub fn rendered_controls(gains: [f32; 3]) -> [f32; 2] {
    let sample = gains.map(|g| if g.is_finite() { 1. / g.max(1e-6) } else { 1. });
    let cost = |t: f32, n: f32| {
        let (m, cw) = rendered_wb_matrix(t, n, false);
        if cw.iter().any(|&c| c < 1. / MAX_WB_GAIN) {
            return f32::INFINITY;
        }
        let l = crate::color::mul(m, sample).map(|v| v.max(1e-6).log2());
        let mean = l.iter().sum::<f32>() / 3.;
        l.iter().map(|v| (v - mean).powi(2)).sum::<f32>()
    };
    let mut best = (cost(0., 0.), 0., 0.);
    for t in (-100..=100).step_by(5) {
        for n in (-100..=100).step_by(5) {
            let c = cost(t as f32, n as f32);
            if c < best.0 {
                best = (c, t as f32, n as f32);
            }
        }
    }
    let mut step = 2.5;
    while step > 0.01 {
        let (_, t0, n0) = best;
        for (dt, dn) in [
            (-step, 0.),
            (step, 0.),
            (0., -step),
            (0., step),
            (-step, -step),
            (-step, step),
            (step, -step),
            (step, step),
        ] {
            let (t, n) = ((t0 + dt).clamp(-100., 100.), (n0 + dn).clamp(-100., 100.));
            let c = cost(t, n);
            if c < best.0 {
                best = (c, t, n);
            }
        }
        if (best.1, best.2) == (t0, n0) {
            step /= 2.;
        }
    }
    [best.1, best.2]
}

#[cfg(test)]
mod rendered_tests {
    use super::*;
    use crate::camera_data::{Metadata, Source};
    use crate::model::recipe::Recipe;

    #[test]
    fn rendered_white_balance_is_neutral_at_zero() {
        assert_eq!(rendered_gains(0., 0.), [1.; 3]);
        assert_eq!(rendered_controls([1.; 3]), [0., 0.]);
    }

    /// Lightroom Classic 15.6 (Camera Raw 18.7) on synthetic charts: the per-channel
    /// ProPhoto log2 gains of its renders of a JPEG at each setting, measured on the
    /// charts' unclipped greys as Lightroom's export over the file, each through `F⁻¹`.
    #[test]
    fn rendered_gains_match_lightroom() {
        let cases: [(f32, f32, [f32; 3], f32); 13] = [
            (10., 0., [0.236, 0.195, 0.001], 0.02),
            (25., 0., [0.564, 0.466, 0.004], 0.02),
            (50., 0., [1.026, 0.849, 0.013], 0.02),
            (100., 0., [1.603, 1.342, 0.029], 0.03),
            (-10., 0., [0.005, 0.047, 0.254], 0.02),
            (-25., 0., [0.035, 0.137, 0.677], 0.02),
            (-50., 0., [0.170, 0.322, 1.496], 0.03),
            (-75., 0., [0.510, 0.580, 2.497], 0.05),
            (0., 25., [0.102, 0., 0.195], 0.02),
            (0., -50., [0.173, 0.443, 0.006], 0.02),
            (50., 50., [1.076, 0.641, -0.006], 0.03),
            (-50., -50., [-0.027, 0.523, 0.738], 0.02),
            (-25., 25., [0.077, 0.050, 0.913], 0.02),
        ];
        for (t, n, want, tolerance) in cases {
            let got = rendered_gains(t, n).map(f32::log2);
            assert!(
                (0..3).all(|c| (got[c] - want[c]).abs() <= tolerance),
                "Temp {t} Tint {n}: {got:?}, Lightroom {want:?}"
            );
        }
    }

    /// Lightroom's scene values (its export through `F⁻¹`) for a file's scene values,
    /// at Temp and Tint: a color, which channel gains alone miss (foliage at Temp −75
    /// came out 0.089 red against Lightroom's 0.069), and greys in the cool magenta
    /// corner, whose blue Lightroom takes to 0.
    #[test]
    fn rendered_white_balance_matches_lightroom() {
        let foliage = [0.0617, 0.0867, 0.0391];
        let cases: [(f32, f32, [f32; 3], [f32; 3]); 4] = [
            (-75., 0., foliage, [0.0693, 0.1314, 0.2162]),
            (75., 0., foliage, [0.171, 0.1928, 0.0425]),
            (-75., 75., [0.0141; 3], [0.0115, 0.017, 0.]),
            (-50., 100., [0.0141; 3], [0.013, 0.0139, 0.]),
        ];
        for (t, n, scene, want) in cases {
            let wb = rendered_white_balance(t, n);
            let held: [f32; 3] = std::array::from_fn(|c| scene[c].min(wb.clip[c]));
            let got = crate::color::mul(wb.matrix, held).map(|v| v.max(0.));
            assert!(
                (0..3).all(|c| (got[c] - want[c]).abs() <= 0.0005_f32.max(0.03 * want[c])),
                "Temp {t} Tint {n}: {got:?}, Lightroom {want:?}"
            );
        }
    }

    /// Camera Raw 18.7 caps per-channel WB gain at 16× (`camera_white` floored at
    /// 0.001): at Temp −25, Tint +100 (`DNG tint = +150`), whites stay gently warm
    /// with positive blue rather than collapsing to yellow; across the transition
    /// (`−24, +100`, `−40, +75`) blue scales continuously; and deep in the corner
    /// (`−50, +100`) a saturated blue sample keeps the 0.001 floor's blue response
    /// while greys drop to 0 blue.
    #[test]
    fn cool_magenta_corner_matches_camera_raw() {
        let apply = |t: f32, n: f32, scene: [f32; 3]| {
            let wb = rendered_white_balance(t, n);
            let held: [f32; 3] = std::array::from_fn(|c| scene[c].min(wb.clip[c]));
            crate::color::mul(wb.matrix, held).map(|v| v.max(0.))
        };

        // Temp -25, Tint +100: ACR 18.7 matrix-only linear DNG at identical white
        // (3791 K, +150) maps grey [0.02003, 0.02, 0.02006] to [0.02397, 0.02002, 0.01375]
        // — warm with positive blue, not zero-blue yellow.
        let wb_25_100 = rendered_white_balance(-25., 100.);
        assert!(
            wb_25_100.clip[2] >= 0.25,
            "Temp -25 Tint +100 clip must not collapse midtones: {:?}",
            wb_25_100.clip
        );
        let got = apply(-25., 100., [0.02003, 0.02, 0.02006]);
        let want = [0.02397, 0.02002, 0.01375];
        assert!(
            (0..3).all(|c| (got[c] - want[c]).abs() <= 0.001),
            "Temp -25 Tint +100 grey: got {got:?}, ACR {want:?}"
        );

        // Temp -50, Tint +100: ACR 18.7 linear DNG (2975 K, +100) maps blue-ish
        // [0.01002, 0.00999, 0.0401] to [0.00915, 0.01034, 0.00314] via the 0.001
        // camera-white floor (16 × 0.001 = 0.016), while greys drop to 0 blue.
        let got_blue = apply(-50., 100., [0.01002, 0.00999, 0.0401]);
        let want_blue = [0.00915, 0.01034, 0.00314];
        assert!(
            (0..3).all(|c| (got_blue[c] - want_blue[c]).abs() <= 0.001),
            "Temp -50 Tint +100 saturated blue: got {got_blue:?}, ACR {want_blue:?}"
        );

        // Regression pins on the 16× ramp: blue is attenuated relative to the uncapped
        // matrix while staying strongly positive.
        let got_24 = apply(-24., 100., [0.02; 3]);
        assert!(
            (0.05..=0.065).contains(&got_24[2]),
            "Temp -24 Tint +100 grey blue should sit on the 16× ramp: {got_24:?}"
        );
        let got_40 = apply(-40., 75., [0.02; 3]);
        assert!(
            (0.08..=0.10).contains(&got_40[2]),
            "Temp -40 Tint +75 grey blue should sit on the 16× ramp: {got_40:?}"
        );
    }

    #[test]
    fn cool_magenta_transition_is_continuous_and_uncollapsed() {
        let apply = |t: f32, n: f32, scene: [f32; 3]| {
            let wb = rendered_white_balance(t, n);
            let held: [f32; 3] = std::array::from_fn(|c| scene[c].min(wb.clip[c]));
            crate::color::mul(wb.matrix, held).map(|v| v.max(0.))
        };
        let min_clip = (1. / MAX_WB_GAIN) * CAMERA_WHITE_SCALE;
        for t in (-100..=100).step_by(5) {
            for n in (-100..=100).step_by(5) {
                let wb = rendered_white_balance(t as f32, n as f32);
                assert!(
                    wb.clip.iter().all(|&c| c >= min_clip - 1e-6),
                    "Temp {t} Tint {n}: clip below 16× floor: {:?}",
                    wb.clip
                );
            }
        }
        // Adjacent 1-unit steps across the Tint +100 transition (Temp -30..-15) and
        // Temp -100 transition (Tint 0..30) move continuously without a cliff collapse.
        for t in -30..-15 {
            let a = apply(t as f32, 100., [0.02; 3]);
            let b = apply((t + 1) as f32, 100., [0.02; 3]);
            let step = (0..3).map(|c| (b[c] - a[c]).abs()).fold(0.0f32, f32::max);
            assert!(
                step < 0.055,
                "discontinuous jump at Tint +100 Temp {t}->{}: {a:?} -> {b:?} (max {step})",
                t + 1
            );
        }
        for n in 0..30 {
            let a = apply(-100., n as f32, [0.02; 3]);
            let b = apply(-100., (n + 1) as f32, [0.02; 3]);
            let step = (0..3).map(|c| (b[c] - a[c]).abs()).fold(0.0f32, f32::max);
            assert!(
                step < 0.035,
                "discontinuous jump at Temp -100 Tint {n}->{}: {a:?} -> {b:?} (max {step})",
                n + 1
            );
        }
        // Regression pin along Temp -100: Tint +6 sits above the 16× cap, Tint +22
        // remains positive, and blue reaches 0 by Tint +24.
        let b6 = rendered_gains(-100., 6.)[2];
        let b20 = rendered_gains(-100., 20.)[2];
        let b22 = rendered_gains(-100., 22.)[2];
        let b24 = rendered_gains(-100., 24.)[2];
        assert!(b6 > b20 && b20 > b22 && b22 > 0.5, "{b6} {b20} {b22}");
        assert_eq!(b24, 0., "Temp -100 Tint +24 should reach 0 blue");

        // `rendered_dng_tint` hits exact D50 at 0, ±150 at ±100, and increases strictly.
        assert_eq!(rendered_dng_tint(-100.), -150.);
        assert!((rendered_dng_tint(0.) - 9.6).abs() < 1e-6);
        assert_eq!(rendered_dng_tint(100.), 150.);
        let mut prev = f32::NEG_INFINITY;
        for n in -100..=100 {
            let v = rendered_dng_tint(n as f32);
            assert!(v > prev, "non-monotonic tint at {n}: {v} <= {prev}");
            prev = v;
        }
    }

    #[test]
    fn rendered_white_balance_leaves_the_file_as_it_is_at_zero() {
        let wb = rendered_white_balance(0., 0.);
        assert_eq!(wb.matrix, [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]]);
        assert!(wb.clip.iter().all(|&c| c == f32::MAX));
    }

    #[test]
    fn rendered_gains_stay_finite_and_non_negative() {
        let mut previous = f32::NEG_INFINITY;
        for t in (-100..=100).step_by(5) {
            for n in (-100..=100).step_by(25) {
                let g = rendered_gains(t as f32, n as f32);
                assert!(
                    g[0].is_finite()
                        && g[1].is_finite()
                        && g[2].is_finite()
                        && (0.5..=32.).contains(&g[0])
                        && (0.5..=32.).contains(&g[1])
                        && (0.0..=32.).contains(&g[2]),
                    "{t} {n}: {g:?}"
                );
            }
            // Warmer is always redder against blue.
            let g = rendered_gains(t as f32, 0.);
            assert!(
                g.iter().all(|v| v.is_finite() && (0.5..=32.).contains(v)),
                "{t} 0: {g:?}"
            );
            let warmth = (g[0] / g[2]).log2();
            assert!(warmth >= previous, "{t}: {warmth} < {previous}");
            previous = warmth;
        }
    }

    /// The gains the eyedropper takes from a grey under the illuminant a Temp and Tint
    /// name (using the uncapped illuminant matrix, which matches
    /// [`rendered_white_balance`] everywhere outside the 16×-capped corner).
    fn picked_gains(temperature: f32, tint: f32) -> [f32; 3] {
        let (m, _) = rendered_wb_matrix(temperature, tint, false);
        let sample = crate::color::mul(crate::color::inverse(m), [0.05; 3]);
        sample.map(|v| sample[1] / v)
    }

    /// A strongly blue pick (e.g. a blue sign or saturated sky) warms the photo toward
    /// positive Temp instead of folding into the 16×-capped cool magenta corner.
    #[test]
    fn strongly_blue_pick_warms_rather_than_folding_into_cool_magenta() {
        for sample in [[1., 1., 6.], [1., 1., 8.], [0.9, 1., 8.]] {
            let gains = sample.map(|v| 1. / v);
            let [t, _n] = rendered_controls(gains);
            assert!(
                t > 50.,
                "strongly blue pick {sample:?} should warm (Temp > 50), got Temp {t}"
            );
        }
    }

    /// A very yellow pick (>4 stops short of blue) stays on the 16× maximum-blue-gain
    /// boundary where the capped render actually boosts blue, instead of entering the
    /// zero-blue cool-magenta corner where the pick renders with `b/g == 0`.
    #[test]
    fn very_yellow_pick_stays_on_the_16x_boundary_instead_of_collapsing_blue() {
        for sample in [[1., 1., 0.1], [1., 0.8, 0.1], [1., 1., 0.05]] {
            let gains = sample.map(|v| 1. / v);
            let [t, n] = rendered_controls(gains);
            let out = crate::color::mul(rendered_white_balance(t, n).matrix, sample);
            assert!(
                out[2] > 0.1 * out[1],
                "very yellow pick {sample:?} landed at ({t}, {n}) with collapsed blue: {out:?}"
            );
        }
    }

    #[test]
    fn rendered_controls_invert_rendered_white_balance() {
        for t in [-90., -50., -20., 0., 15., 40., 80.] {
            for n in [-60., -10., 0., 30., 70.] {
                // Pairs inside the 16×-capped cool-magenta corner cannot neutralise their
                // uncapped illuminant color in a capped render; `rendered_controls` stays
                // on the uncapped domain.
                if rendered_wb_matrix(t, n, false)
                    .1
                    .iter()
                    .any(|&c| c < 1. / MAX_WB_GAIN)
                {
                    continue;
                }
                let [t2, n2] = rendered_controls(picked_gains(t, n));
                assert!(
                    (t2 - t).abs() <= 1. && (n2 - n).abs() <= 1.,
                    "{t},{n} -> {t2},{n2}"
                );
            }
        }
    }

    /// A color some Temp and Tint render neutral (a grey under that white) is picked
    /// neutral: the cross-channel terms of the white balance count, not only its row
    /// sums.
    #[test]
    fn rendered_controls_neutralise_a_reachable_pick() {
        for (t, n) in [(-50., 0.), (40., 0.), (-30., 40.), (60., -50.)] {
            let gains = picked_gains(t, n);
            let sample = gains.map(|g| 1. / g);
            let [t2, n2] = rendered_controls(gains);
            let got = crate::color::mul(rendered_white_balance(t2, n2).matrix, sample);
            for c in [0, 2] {
                let ratio = got[c] / got[1];
                assert!((ratio - 1.).abs() < 0.005, "{t},{n} -> {t2},{n2}: {got:?}");
            }
        }
    }

    #[test]
    fn rendered_controls_of_unreachable_gains_clamp() {
        for gains in [
            [1., 1., 64.],
            [64., 1., 1.],
            [1., 64., 1.],
            [0., 0., 0.],
            [f32::NAN, 1., 1.],
        ] {
            let [t, n] = rendered_controls(gains);
            assert!(t.is_finite() && n.is_finite(), "{gains:?}");
            assert!(
                (-100. ..=100.).contains(&t) && (-100. ..=100.).contains(&n),
                "{gains:?}"
            );
        }
    }

    #[test]
    fn a_rendered_photo_keeps_white_balance_relative() {
        let m = Metadata {
            source: Source::Rendered,
            ..Default::default()
        };
        let mut r = Recipe::for_rendered(&m);
        r.incremental_temperature = 40.;
        r.update_wb(&m);
        assert_eq!(r.wb, [1.; 3]);
        assert_eq!((r.temperature, r.tint), (6500., 0.));
        r.sync_white_balance_controls(&m);
        assert_eq!(r.incremental_temperature, 40.);
        r.reset_white_balance(&m);
        assert_eq!(
            (r.incremental_temperature, r.incremental_tint, r.wb),
            (0., 0., [1.; 3])
        );
    }

    /// Every Temp and Tint a rendered photo can be given, the cool magenta corner
    /// outside what was measured among them, keeps a recipe that validates, renders
    /// and saves.
    #[test]
    fn every_rendered_temp_and_tint_gives_a_valid_recipe() {
        let m = Metadata {
            source: Source::Rendered,
            ..Default::default()
        };
        for t in (-100..=100).step_by(5) {
            for n in (-100..=100).step_by(5) {
                let mut r = Recipe::for_rendered(&m);
                r.incremental_temperature = t as f32;
                r.incremental_tint = n as f32;
                r.update_wb(&m);
                assert!(r.validate().is_ok(), "Temp {t} Tint {n}: {:?}", r.wb);
            }
        }
    }

    /// Copying Treatment & Profile without White Balance, or picking a profile,
    /// re-syncs the controls: a rendered photo's Temp and Tint are its white balance,
    /// so they stay as they are, and with them the WB menu's Auto.
    #[test]
    fn a_rendered_photo_keeps_its_temp_and_tint_when_controls_sync() {
        let m = Metadata {
            source: Source::Rendered,
            ..Default::default()
        };
        let mut r = Recipe::for_rendered(&m);
        r.incremental_temperature = -77.;
        r.incremental_tint = 30.;
        r.auto_white_balance = Some([-77., 30.]);
        r.sync_white_balance_controls(&m);
        assert_eq!(
            (r.incremental_temperature, r.incremental_tint, r.wb),
            (-77., 30., [1.; 3])
        );
        assert_eq!(r.auto_white_balance, Some([-77., 30.]));
        r.profile_changed(&m);
        assert_eq!((r.incremental_temperature, r.incremental_tint), (-77., 30.));
    }

    /// An eyedropper's (or Auto's) gains on a rendered photo become the nearest Temp
    /// and Tint, which then carry the white balance: gains stay 1.
    #[test]
    fn picked_gains_on_a_rendered_photo_become_its_temp_and_tint() {
        let m = Metadata {
            source: Source::Rendered,
            ..Default::default()
        };
        let mut r = Recipe::for_rendered(&m);
        // Off the model's reach (and green ≠ 1).
        r.pick_white_balance([1.4, 0.9, 1.2], &m);
        assert_eq!(r.wb, [1.; 3]);
        let [t, n] = rendered_controls([1.4, 0.9, 1.2]);
        assert_eq!((r.incremental_temperature, r.incremental_tint), (t, n));
        assert!(t > 0. && n != 0., "{t} {n}");
        // A neutral pick on a warmed photo takes it back to the file's white balance.
        r.pick_white_balance([1.; 3], &m);
        assert_eq!((r.incremental_temperature, r.incremental_tint), (0., 0.));
        assert_eq!(r.wb, [1.; 3]);
    }

    #[test]
    fn picked_gains_on_a_raw_are_its_white_balance() {
        let m = Metadata::default();
        let mut r = Recipe::for_metadata(&m);
        r.pick_white_balance([1.4, 1., 1.2], &m);
        assert_eq!(r.wb, [1.4, 1., 1.2]);
    }

    #[test]
    fn relative_white_balance_is_validated_for_every_photo() {
        let r = Recipe {
            incremental_tint: 101.,
            ..Recipe::default()
        };
        assert!(r.validate().is_err());
    }
}
