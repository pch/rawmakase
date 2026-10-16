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
pub(crate) fn apply(info: &mut RawInfo) {
    let table = crate::facts::matrix(&info.make, &info.model);
    if let Some(m) = table.or(info.file_matrix) {
        info.cam_xyz = m;
    }
    if let Some((rgb_cam, pre_mul)) = info.file_matrix.or(table).and_then(|m| from_cam_xyz(&m)) {
        info.rgb_cam = rgb_cam;
        info.pre_mul = [pre_mul[0], pre_mul[1], pre_mul[2], pre_mul[1]];
    }
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
}
