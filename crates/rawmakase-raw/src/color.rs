//! A camera's colour from its XYZ-to-camera matrix: the camera-to-sRGB matrix
//! and the daylight multipliers that make sRGB's white neutral, as RAWmakase
//! has always derived them (double precision, the result rounded to single);
//! on our files, bit for bit what LibRaw derived (our measurement).
use crate::info::RawInfo;

/// Linear sRGB to XYZ: IEC 61966-2-1's primaries with the D65 white
/// (0.95047, 1, 1.08883), to seven places.
const XYZ_RGB: [[f64; 3]; 3] = [
    [0.4124564, 0.3575761, 0.1804375],
    [0.2126729, 0.7151522, 0.0721750],
    [0.0193339, 0.1191920, 0.9503041],
];

/// The camera's colour in `info`: its XYZ matrix from the table of Adobe's,
/// else (a DNG's) the file's; the camera-to-sRGB matrix and the daylight
/// multipliers from the file's matrix when it has one, else the table's.
/// Without a matrix the daylight multipliers are neutral (1), so a file
/// without white balance as shot is never balanced by zeros, and the
/// camera-to-sRGB matrix is the identity (as LibRaw's), so the camera's
/// colours pass through rather than to black.
pub(crate) fn apply(info: &mut RawInfo) {
    let table = crate::facts::matrix(&info.make, &info.model);
    // A DNG whose white balance is a chromaticity, of a camera Adobe gives
    // its own matrix: the neutral through that matrix.
    if let (Some(m), Some([x, y])) = (table, info.as_shot_xy) {
        let xyz = [x / y, 1., (1. - x - y) / y];
        let neutral: [f64; 3] =
            std::array::from_fn(|r| (0..3).map(|k| f64::from(m[r][k]) * xyz[k]).sum());
        if neutral.iter().all(|n| n.is_finite() && *n > 0.) {
            for (mul, n) in info.cam_mul.iter_mut().zip(neutral) {
                *mul = 1. / (n as f32);
            }
            info.cam_mul[3] = info.cam_mul[1];
        }
    }
    if let Some(m) = table.or(info.file_matrix) {
        info.cam_xyz = m;
    }
    match info.file_matrix.or(table).and_then(|m| from_cam_xyz(&m)) {
        Some((rgb_cam, pre_mul)) => {
            info.rgb_cam = rgb_cam;
            info.pre_mul = [pre_mul[0], pre_mul[1], pre_mul[2], pre_mul[1]];
        }
        None => {
            info.rgb_cam = std::array::from_fn(|i| std::array::from_fn(|j| f32::from(i == j)));
            info.pre_mul = [1.; 4];
        }
    }
}

/// Isotemperature lines of the CIE 1960 (u, v) diagram: reciprocal
/// megakelvin, u, v and the line's slope (Robertson's method; the table of
/// Wyszecki and Stiles, Color Science).
const ISOTEMPERATURE: [[f64; 4]; 31] = [
    [0., 0.18006, 0.26352, -0.24341],
    [10., 0.18066, 0.26589, -0.25479],
    [20., 0.18133, 0.26846, -0.26876],
    [30., 0.18208, 0.27119, -0.28539],
    [40., 0.18293, 0.27407, -0.30470],
    [50., 0.18388, 0.27709, -0.32675],
    [60., 0.18494, 0.28021, -0.35156],
    [70., 0.18611, 0.28342, -0.37915],
    [80., 0.18740, 0.28668, -0.40955],
    [90., 0.18880, 0.28997, -0.44278],
    [100., 0.19032, 0.29326, -0.47888],
    [125., 0.19462, 0.30141, -0.58204],
    [150., 0.19962, 0.30921, -0.70471],
    [175., 0.20525, 0.31647, -0.84901],
    [200., 0.21142, 0.32312, -1.0182],
    [225., 0.21807, 0.32909, -1.2168],
    [250., 0.22511, 0.33439, -1.4512],
    [275., 0.23247, 0.33904, -1.7298],
    [300., 0.24010, 0.34308, -2.0637],
    [325., 0.24792, 0.34655, -2.4681],
    [350., 0.25591, 0.34951, -2.9641],
    [375., 0.26400, 0.35200, -3.5814],
    [400., 0.27218, 0.35407, -4.3633],
    [425., 0.28039, 0.35577, -5.3762],
    [450., 0.28863, 0.35714, -6.7262],
    [475., 0.29685, 0.35823, -8.5955],
    [500., 0.30505, 0.35907, -11.324],
    [525., 0.31320, 0.35968, -15.628],
    [550., 0.32129, 0.36011, -23.325],
    [575., 0.32931, 0.36038, -40.770],
    [600., 0.33724, 0.36051, -116.45],
];

/// The correlated colour temperature of chromaticity `xy` (CIE 1931), by
/// Robertson's method; `None` outside the table's 1667 K to infinity.
pub(crate) fn temperature([x, y]: [f64; 2]) -> Option<f64> {
    let d = -2. * x + 12. * y + 3.;
    if d == 0. || !d.is_finite() {
        return None;
    }
    let (u, v) = (4. * x / d, 6. * y / d);
    let mut previous: Option<(f64, f64)> = None;
    for [r, ui, vi, ti] in ISOTEMPERATURE {
        // Signed distance from the isotemperature line.
        let di = ((v - vi) - ti * (u - ui)) / (1. + ti * ti).sqrt();
        if let Some((r0, d0)) = previous
            && (di <= 0.) != (d0 <= 0.)
        {
            let f = d0 / (d0 - di);
            return Some(1e6 / (r0 + f * (r - r0)));
        }
        previous = Some((r, di));
    }
    None
}

/// From `cam_xyz` (XYZ to camera, rows red, green, blue), the camera to sRGB
/// matrix (rows red, green, blue) and the daylight multipliers (red, green,
/// blue). `None` when a row sums to zero or the matrix is singular.
pub(crate) fn from_cam_xyz(cam_xyz: &[[f32; 3]; 3]) -> Option<([[f32; 3]; 3], [f32; 3])> {
    // camera from sRGB: cam_xyz × xyz_rgb, each row scaled to sum to one.
    let mut cam_rgb = [[0f64; 3]; 3];
    let mut pre_mul = [0f32; 3];
    for i in 0..3 {
        for j in 0..3 {
            cam_rgb[i][j] = (0..3)
                .map(|k| f64::from(cam_xyz[i][k]) * XYZ_RGB[k][j])
                .sum::<f64>();
        }
        let num: f64 = cam_rgb[i].iter().sum();
        if num == 0. || !num.is_finite() {
            return None;
        }
        for v in &mut cam_rgb[i] {
            *v /= num;
        }
        pre_mul[i] = (1. / num) as f32;
    }
    let inverse = pseudoinverse(&cam_rgb)?;
    let rgb_cam = std::array::from_fn(|i| std::array::from_fn(|j| inverse[j][i] as f32));
    Some((rgb_cam, pre_mul))
}

/// The XYZ (D65) to camera matrix of a camera's own matrix: `rgb_cam`, from
/// white balanced camera values to linear sRGB (each row scaled to sum to
/// one, so balanced white stays white), and `daylight`, the multipliers that
/// balance D65 (red, green, blue). The inverse of [`from_cam_xyz`]: it gives
/// back `rgb_cam` and `daylight` (green one). `None` when a row sums to zero,
/// the matrix is singular or a multiplier is not positive.
pub(crate) fn cam_xyz_of(rgb_cam: &[[f64; 3]; 3], daylight: [f64; 3]) -> Option<[[f32; 3]; 3]> {
    if !daylight.iter().all(|d| d.is_finite() && *d > 0.) {
        return None;
    }
    let mut balanced = *rgb_cam;
    for row in &mut balanced {
        let num: f64 = row.iter().sum();
        if num == 0. || !num.is_finite() {
            return None;
        }
        row.iter_mut().for_each(|v| *v /= num);
    }
    // camera = diag(1 / daylight) × rgb_cam⁻¹ × XYZ-to-sRGB.
    let cam_rgb = pseudoinverse(&balanced)?;
    let rgb_xyz = pseudoinverse(&XYZ_RGB)?;
    let m: [[f64; 3]; 3] = std::array::from_fn(|i| {
        std::array::from_fn(|j| {
            (0..3).map(|k| cam_rgb[k][i] * rgb_xyz[j][k]).sum::<f64>() / daylight[i]
        })
    });
    m.iter()
        .flatten()
        .all(|v| v.is_finite())
        .then(|| std::array::from_fn(|i| std::array::from_fn(|j| m[i][j] as f32)))
}

/// The pseudoinverse of a 3 × 3 matrix by its normal equations, (AᵀA)⁻¹Aᵀ,
/// transposed: `out[i][j]` for row `i` of A.
fn pseudoinverse(a: &[[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    // [AᵀA | I], reduced to [I | (AᵀA)⁻¹].
    let mut work: [[f64; 6]; 3] = std::array::from_fn(|i| {
        std::array::from_fn(|j| {
            if j < 3 {
                (0..3).map(|k| a[k][i] * a[k][j]).sum()
            } else if j == i + 3 {
                1.
            } else {
                0.
            }
        })
    });
    for i in 0..3 {
        let num = work[i][i];
        if num == 0. || !num.is_finite() {
            return None;
        }
        work[i].iter_mut().for_each(|v| *v /= num);
        let pivot = work[i];
        for (k, row) in work.iter_mut().enumerate() {
            if k != i {
                let num = row[i];
                row.iter_mut().zip(&pivot).for_each(|(v, p)| *v -= p * num);
            }
        }
    }
    Some(std::array::from_fn(|i| {
        std::array::from_fn(|j| (0..3).map(|k| work[j][k + 3] * a[i][k]).sum())
    }))
}

#[cfg(test)]
mod tests {
    /// Robertson's method gives the standard illuminants' temperatures.
    #[test]
    fn correlated_colour_temperatures() {
        let t = |x, y| super::temperature([x, y]).unwrap();
        assert!(
            (t(0.31271, 0.32902) - 6504.).abs() < 10.,
            "{}",
            t(0.31271, 0.32902)
        );
        assert!(
            (t(0.44757, 0.40745) - 2856.).abs() < 5.,
            "{}",
            t(0.44757, 0.40745)
        );
        assert!(
            (t(0.34567, 0.35850) - 5003.).abs() < 10.,
            "{}",
            t(0.34567, 0.35850)
        );
        assert_eq!(super::temperature([0.7, 0.1]), None);
    }

    #[test]
    fn daylight_comes_from_the_matrix_else_is_neutral() {
        // No matrix: neutral daylight and the identity, never zeros.
        let mut info = crate::info::base(crate::Variant::MefPacked);
        info.make = "NoSuchMaker".into();
        super::apply(&mut info);
        assert_eq!(info.pre_mul, [1.; 4]);
        assert_eq!(info.rgb_cam, [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]]);
        assert_eq!(info.cam_xyz, [[0.; 3]; 3]);
        // A file's matrix: daylight from it.
        let m = [[0.5f32, 0., 0.], [0., 1., 0.], [0., 0., 0.25]];
        info.file_matrix = Some(m);
        super::apply(&mut info);
        let (_, daylight) = super::from_cam_xyz(&m).unwrap();
        assert_eq!(
            info.pre_mul,
            [daylight[0], daylight[1], daylight[2], daylight[1]]
        );
        assert!(info.pre_mul.iter().all(|&v| v > 0.));
    }

    #[test]
    fn srgb_camera_is_the_identity() {
        // A camera whose XYZ matrix is sRGB's own inverse sees sRGB.
        let m = [
            [3.240479f32, -1.53715, -0.498535],
            [-0.969256, 1.875992, 0.041556],
            [0.055648, -0.204043, 1.057311],
        ];
        let (rgb_cam, pre_mul) = super::from_cam_xyz(&m).unwrap();
        for i in 0..3 {
            assert!((pre_mul[i] - 1.).abs() < 1e-4, "{pre_mul:?}");
            for j in 0..3 {
                let want = if i == j { 1. } else { 0. };
                assert!((rgb_cam[i][j] - want).abs() < 1e-4, "{rgb_cam:?}");
            }
        }
        assert!(super::from_cam_xyz(&[[0.; 3]; 3]).is_none());
    }

    #[test]
    fn a_camera_s_own_srgb_matrix_becomes_its_xyz_matrix() {
        // A camera that sees sRGB once balanced by 2, 1, 1.5 (its rows
        // summing to 256 here): sRGB's inverse with each row over its
        // multiplier.
        let identity = [[256., 0., 0.], [0., 256., 0.], [0., 0., 256.]];
        let m = super::cam_xyz_of(&identity, [2., 1., 1.5]).unwrap();
        let srgb = [
            [3.240479f32, -1.53715, -0.498535],
            [-0.969256, 1.875992, 0.041556],
            [0.055648, -0.204043, 1.057311],
        ];
        for (i, d) in [2f32, 1., 1.5].into_iter().enumerate() {
            for j in 0..3 {
                assert!((m[i][j] - srgb[i][j] / d).abs() < 1e-3, "{m:?}");
            }
        }
        let (rgb_cam, pre_mul) = super::from_cam_xyz(&m).unwrap();
        assert!((pre_mul[0] - 2.).abs() < 1e-5 && (pre_mul[2] - 1.5).abs() < 1e-5);
        assert!((rgb_cam[1][1] - 1.).abs() < 1e-5 && rgb_cam[0][1].abs() < 1e-5);
        // Singular, a row of zero sum, or no multiplier: none.
        assert!(super::cam_xyz_of(&[[1., 1., 1.]; 3], [1.; 3]).is_none());
        assert!(super::cam_xyz_of(&[[1., -1., 0.], [0., 1., 0.], [0., 0., 1.]], [1.; 3]).is_none());
        assert!(super::cam_xyz_of(&identity, [1., 0., 1.]).is_none());
        assert!(super::cam_xyz_of(&identity, [1., f64::NAN, 1.]).is_none());
    }
}
