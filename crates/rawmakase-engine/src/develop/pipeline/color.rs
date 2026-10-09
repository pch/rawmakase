//! Colour conversions the pipeline shares: sRGB and Lab, Rec. 2020, hue weights and the profile matrix.
use super::*;

pub(crate) fn srgb_to_lab(p: [f32; 3]) -> [f32; 3] {
    let a = mul(
        [
            [0.41222146, 0.53633255, 0.051445995],
            [0.2119035, 0.6806995, 0.10739696],
            [0.08830246, 0.28171885, 0.6299787],
        ],
        p,
    )
    .map(f32::cbrt);
    mul(
        [
            [0.21045426, 0.7936178, -0.004072047],
            [1.9779985, -2.4285922, 0.4505937],
            [0.025904037, 0.78277177, -0.80867577],
        ],
        a,
    )
}
pub(super) fn lab_to_srgb(p: [f32; 3]) -> [f32; 3] {
    let a = mul(
        [
            [1., 0.39633778, 0.21580376],
            [1., -0.105561346, -0.06385417],
            [1., -0.08948418, -1.2914855],
        ],
        p,
    )
    .map(|v| v * v * v);
    mul(
        [
            [4.0767417, -3.3077116, 0.23096994],
            [-1.268438, 2.6097574, -0.3413194],
            [-0.0041960863, -0.7034186, 1.7076147],
        ],
        a,
    )
}
pub(super) const TO_2020: [[f32; 3]; 3] = [
    [0.627404, 0.329283, 0.043313],
    [0.069097, 0.91954, 0.011362],
    [0.016391, 0.088013, 0.895595],
];
pub(super) const FROM_2020: [[f32; 3]; 3] = [
    [1.660491, -0.587641, -0.07285],
    [-0.12455, 1.1329, -0.008349],
    [-0.018151, -0.100579, 1.11873],
];
pub(crate) fn hue_weights(hue: f32) -> [f32; 8] {
    // Centers correspond to red, orange, yellow, green, cyan, blue, purple, magenta in Oklab.
    const CENTERS: [f32; 8] = [0.081, 0.151, 0.305, 0.395, 0.541, 0.733, 0.815, 0.912];
    let mut weights = [0.; 8];
    let hue = hue.rem_euclid(1.);
    for i in 0..8 {
        let left = CENTERS[i];
        let right = if i == 7 {
            CENTERS[0] + 1.
        } else {
            CENTERS[i + 1]
        };
        let h = if hue < left { hue + 1. } else { hue };
        if h >= left && h <= right {
            let t = (h - left) / (right - left);
            weights[i] = 1. - t;
            weights[(i + 1) % 8] = t;
            break;
        }
    }
    weights
}
pub(crate) fn profile_matrix(m: &Metadata, r: &Recipe) -> [[f32; 3]; 3] {
    r.profile
        .as_ref()
        .map_or(m.matrix, |p| p.camera_matrix(r.temperature))
}
