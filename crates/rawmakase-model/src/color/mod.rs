//! Color arithmetic and tone-curve primitives shared by camera profiles, the
//! develop pipeline, XMP and the interface. Depends on nothing else in the crate.
pub mod curve;
pub mod hdr_output;
pub mod hsv;
pub mod output;
pub use output::OutputSpace;

/// Relative luminance with the Rec. 709 / sRGB weights, in the units of `p`.
pub fn luminance(p: [f32; 3]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}
pub fn mul(m: [[f32; 3]; 3], p: [f32; 3]) -> [f32; 3] {
    m.map(|r| r[0] * p[0] + r[1] * p[1] + r[2] * p[2])
}
/// The matrix product `a · b`.
pub fn mul_matrix(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}
pub fn inverse(m: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let [a, b, c] = m;
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let x = cross(b, c);
    let y = cross(c, a);
    let z = cross(a, b);
    let d = a[0] * x[0] + a[1] * x[1] + a[2] * x[2];
    if d.abs() < 1e-8 {
        return [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
    }
    std::array::from_fn(|r| [x[r] / d, y[r] / d, z[r] / d])
}
pub fn srgb_decode(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
pub fn srgb_encode(v: f32) -> f32 {
    if v <= 0.0031308 {
        12.92 * v
    } else {
        1.055 * v.max(0.).powf(1. / 2.4) - 0.055
    }
}
