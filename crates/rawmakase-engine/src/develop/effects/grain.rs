//! Effects > Grain, following Camera Raw 18.7, fitted to the
//! statistics (not the pixels) of renders of flat synthetic DNGs 1500, 3000 and 6000
//! pixels wide at Amount 25 to 100, Size 0 to 100 and Roughness 0 to 100.
//!
//! Camera Raw's grain scales with the photo: Size sets the grain's width as a share of
//! the long edge, so a 6000-pixel photo gets grains four times as wide in pixels as a
//! 1500-pixel one, and finer, lower-contrast grain per pixel. Size 0 is grain of single
//! pixels at any size. Here the grain is a smooth value noise of that width plus
//! single-pixel noise that fades as the grain grows; Roughness shortens the grain and
//! adds pixel noise at 0, and widens and strengthens it at 100. The grain is about the
//! same strength in L* at every brightness, fading into black and white.
//!
//! The standard deviation of the fitted model is within about 10% of Camera Raw's over
//! the 21 measured settings. Camera Raw's slightly negative pixel-to-pixel correlation
//! at small sizes (its grain is a little "blue") is not modelled.
use super::Effects;

/// The long edge, in pixels, at which grain sizes below are given.
const REFERENCE_EDGE: f32 = 6000.;
/// Grain width (pixels at the reference edge) at Size 0 and per unit of Size.
const WIDTH_BASE: f32 = 0.1555;
const WIDTH_PER_SIZE: f32 = 3.627;
/// Roughness 0, 50, 100: width factor, value-noise weight and pixel-noise factor.
const ROUGHNESS: [f32; 3] = [0., 0.5, 1.];
const ROUGHNESS_WIDTH: [f32; 3] = [1.1487, 1., 1.4675];
const ROUGHNESS_COARSE: [f32; 3] = [0.1546, 0.1132, 0.2058];
const ROUGHNESS_FINE: [f32; 3] = [1.8644, 1., 1.];
/// Pixel noise weight, fading with the grain width over `FINE_FADE` pixels.
const FINE: f32 = 0.3538;
const FINE_FADE: f32 = 0.7055;
/// Strength by encoded luminance, relative to mid gray (Camera Raw's L* standard
/// deviation on flat grays).
const LUMINANCE: [f32; 9] = [0., 0.023, 0.156, 0.41, 0.62, 0.8, 0.91, 0.964, 1.];
const LUMINANCE_GAIN: [f32; 9] = [0., 0.5, 1.14, 1.08, 1., 0.94, 0.86, 0.61, 0.3];

fn interpolate(x: f32, xs: &[f32], ys: &[f32]) -> f32 {
    let i = xs.partition_point(|v| *v <= x).clamp(1, xs.len() - 1);
    let t = ((x - xs[i - 1]) / (xs[i] - xs[i - 1])).clamp(0., 1.);
    ys[i - 1] + (ys[i] - ys[i - 1]) * t
}

pub(crate) fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut v = (x as u32).wrapping_mul(0x9e3779b9) ^ (y as u32).wrapping_mul(0x85ebca6b) ^ seed;
    v ^= v >> 16;
    v = v.wrapping_mul(0x7feb352d);
    v ^= v >> 15;
    v = v.wrapping_mul(0x846ca68b);
    v ^= v >> 16;
    (v as f64 / u32::MAX as f64 * 2. - 1.) as f32
}
/// Value noise on a lattice of `cell` pixels, smoothstep between the knots.
fn value_noise(x: f32, y: f32, cell: f32, seed: u32) -> f32 {
    let x = x / cell;
    let y = y / cell;
    let ix = x.floor() as i32;
    let iy = y.floor() as i32;
    let smooth = |v: f32| v * v * (3. - 2. * v);
    let a = smooth(x - ix as f32);
    let b = smooth(y - iy as f32);
    let n = hash(ix, iy, seed) * (1. - a) + hash(ix + 1, iy, seed) * a;
    let m = hash(ix, iy + 1, seed) * (1. - a) + hash(ix + 1, iy + 1, seed) * a;
    n * (1. - b) + m * b
}

/// A recipe's grain for an output whose full-resolution long edge is `edge` pixels;
/// shared by the CPU and the GPU preview.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GrainField {
    pub(crate) amount: f32,
    /// Value-noise lattice spacing, in full-resolution pixels.
    pub(crate) cell: f32,
    /// Weights of the value noise and of the single-pixel noise.
    pub(crate) coarse: f32,
    pub(crate) fine: f32,
    pub(crate) seed: u32,
}
impl GrainField {
    pub(crate) fn new(e: &Effects, edge: f32) -> Self {
        let f = e.grain_roughness;
        let width = (WIDTH_BASE + WIDTH_PER_SIZE * e.grain_size) * edge / REFERENCE_EDGE
            * interpolate(f, &ROUGHNESS, &ROUGHNESS_WIDTH);
        Self {
            amount: e.grain,
            cell: 2. * width,
            coarse: interpolate(f, &ROUGHNESS, &ROUGHNESS_COARSE),
            fine: FINE * (-width / FINE_FADE).exp() * interpolate(f, &ROUGHNESS, &ROUGHNESS_FINE),
            seed: e.grain_seed,
        }
    }
    /// The grain at full-resolution position `x`, `y` for an output of `scale` pixels
    /// per full-resolution pixel (a preview pixel averages several grains) over encoded
    /// luminance `l`.
    pub(crate) fn noise(&self, x: f32, y: f32, scale: f32, l: f32) -> f32 {
        if self.amount == 0. {
            return 0.;
        }
        // Grain below a pixel wide averages away within the pixel.
        let averaged = (self.cell * scale).min(1.);
        let coarse = value_noise(x, y, self.cell, self.seed) * averaged;
        let fine = hash(x.round() as i32, y.round() as i32, self.seed ^ 0x21f09) * scale.min(1.);
        (coarse * self.coarse + fine * self.fine) * self.amount * self.strength(l)
    }
    fn strength(&self, l: f32) -> f32 {
        interpolate(l, &LUMINANCE, &LUMINANCE_GAIN)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deviation(field: &GrainField) -> f32 {
        let values: Vec<f32> = (0..256 * 256)
            .map(|i| field.noise((i % 256) as f32, (i / 256) as f32, 1., 0.62))
            .collect();
        let mean = values.iter().sum::<f32>() / values.len() as f32;
        (values.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / values.len() as f32).sqrt()
    }
    fn measured(amount: f32, size: f32, roughness: f32) -> Effects {
        Effects {
            grain: amount,
            grain_size: size,
            grain_roughness: roughness,
            ..Effects::default()
        }
    }

    /// Camera Raw 18.7's grain on flat mid gray: L* standard deviation over about 98
    /// per unit of encoded value.
    #[test]
    fn measured_grain_matches_camera_raw_strength_and_scales_with_the_photo() {
        for (edge, size, roughness, camera_raw) in [
            (1500., 0.25, 0.5, 7.32),
            (3000., 0.25, 0.5, 5.06),
            (6000., 0.25, 0.5, 3.23),
            (3000., 0., 0.5, 8.78),
            (3000., 1., 0.5, 2.54),
            (3000., 0.25, 0., 8.48),
            (3000., 0.25, 1., 5.55),
        ] {
            let field = GrainField::new(&measured(0.5, size, roughness), edge);
            let ours = deviation(&field) * 98.;
            assert!(
                (ours / camera_raw - 1.).abs() < 0.2,
                "{edge} {size} {roughness}: {ours} against {camera_raw}"
            );
        }
    }

    #[test]
    fn measured_grain_fades_into_black_and_white() {
        let field = GrainField::new(&measured(1., 0.25, 0.5), 3000.);
        assert_eq!(field.strength(0.), 0.);
        assert!(field.strength(0.97) < field.strength(0.62) * 0.7);
        assert!(field.strength(0.2) > field.strength(0.62));
    }
}
