//! Positive Dehaze: dark-channel haze removal on the scene tone stage's input, fitted
//! to Camera Raw 18.7 on the 22 training photos (docs/scene-tone-stage.md#dehaze).
//!
//! The haze is measured once per photo on its measurement copy at Exposure 0
//! ([`Haze::of`]): the airlight A, the mean colour of the haziest 1% (by the dark
//! channel, the window minimum of the darkest channel), and the guided-filtered dark
//! channel of the photo relative to A, the haze's density. A pixel at Dehaze `s`
//! removes `ω(s)` of that density ([`Dehazing::apply`]).
use std::sync::Arc;

/// Window radius of the dark channel, relative to the measurement copy's long edge.
const RADIUS: f32 = 0.005;
/// The guided filter's ε (on the dark channel relative to A, 0 to 1).
const EPSILON: f32 = 0.01;
/// The lowest transmission: densest haze is removed by at most this factor.
const MIN_TRANSMISSION: f32 = 0.3;
/// The share of the colour-preserving result no channel falls below.
const CHANNEL_FLOOR: f32 = 0.5;
/// Dehaze amounts and the fraction of the haze they remove; linear between them.
const OMEGA: [(f32, f32); 4] = [(0., 0.), (0.2, 0.3), (0.4, 0.45), (1., 0.9)];

/// A photo's haze, from its measurement copy at the scene stage's input, Exposure 0.
#[derive(Debug, PartialEq)]
pub(crate) struct Haze {
    pub(crate) width: usize,
    pub(crate) height: usize,
    /// The airlight per channel, linear ProPhoto at Exposure 0.
    pub(crate) air: [f32; 3],
    /// The haze's density on the copy's grid: the guided-filtered dark channel of the
    /// photo relative to `air`.
    pub(crate) density: Vec<f32>,
}
impl Haze {
    /// The haze of `scene`, `width` × `height` linear ProPhoto values.
    pub(crate) fn of(scene: &[[f32; 3]], width: usize, height: usize) -> Self {
        assert_eq!(scene.len(), width * height);
        let r = ((RADIUS * width.max(height) as f32).round() as usize).max(1);
        let darkest = |p: &[f32; 3]| p[0].min(p[1]).min(p[2]);
        let dark = min_filter(
            &scene.iter().map(darkest).collect::<Vec<_>>(),
            width,
            height,
            r,
        );
        // The airlight: the mean of the haziest 1%, at or above the dark channel's 99th
        // percentile (as local_tone.rs takes percentiles).
        let mut sorted = dark.clone();
        let k = (sorted.len() - 1) * 99 / 100;
        let threshold = *sorted.select_nth_unstable_by(k, f32::total_cmp).1;
        let (mut sum, mut n) = ([0f64; 3], 0usize);
        for (p, d) in scene.iter().zip(&dark) {
            if *d >= threshold {
                (0..3).for_each(|c| sum[c] += p[c] as f64);
                n += 1;
            }
        }
        let air = sum.map(|v| ((v / n.max(1) as f64) as f32).max(1e-6));
        let relative: Vec<f32> = scene
            .iter()
            .map(|p| (p[0] / air[0]).min(p[1] / air[1]).min(p[2] / air[2]))
            .collect();
        let raw = min_filter(&relative, width, height, r);
        Self {
            width,
            height,
            air,
            density: guided(&raw, width, height, 2 * r, EPSILON),
        }
    }
}

/// The fraction of the haze Dehaze `s` (0 to 1) removes.
pub(crate) fn omega(s: f32) -> f32 {
    let s = s.clamp(0., 1.);
    let j = OMEGA
        .windows(2)
        .position(|w| s <= w[1].0)
        .unwrap_or(OMEGA.len() - 2);
    let ((s0, w0), (s1, w1)) = (OMEGA[j], OMEGA[j + 1]);
    w0 + (w1 - w0) * (s - s0) / (s1 - s0)
}

/// A photo's haze as one render removes it: at that render's Exposure and sample grid.
#[derive(Clone, Debug)]
pub(crate) struct Dehazing {
    pub(crate) haze: Arc<Haze>,
    /// The haze's grid relative to the rendered camera image's samples.
    pub(crate) scale: [f32; 2],
    /// The airlight at the render's Exposure.
    pub(crate) air: [f32; 3],
}
impl Dehazing {
    /// `haze` for a `source`-sized camera image rendered at `exposure` stops.
    pub(crate) fn new(haze: Arc<Haze>, source: [u32; 2], exposure: f32) -> Self {
        let gain = exposure.exp2();
        Self {
            scale: [
                haze.width as f32 / source[0] as f32,
                haze.height as f32 / source[1] as f32,
            ],
            air: haze.air.map(|a| a * gain),
            haze,
        }
    }
    /// Pixel `p` at camera-image sample position `pos` with Dehaze `s` > 0 removed:
    /// J = (p − A) / t + A, with t = 1 − ω(s) · density, at least `MIN_TRANSMISSION`,
    /// each channel at least `CHANNEL_FLOOR` of p scaled by the luminance's change.
    pub(crate) fn apply(&self, p: [f32; 3], s: f32, pos: [f32; 2]) -> [f32; 3] {
        let density = crate::develop::local_tone::grid_sample(
            &self.haze.density,
            [self.haze.width, self.haze.height],
            self.scale,
            pos,
        );
        let t = (1. - omega(s) * density).max(MIN_TRANSMISSION);
        let j: [f32; 3] = std::array::from_fn(|c| (p[c] - self.air[c]) / t + self.air[c]);
        // No channel falls below half of the pixel's colour scaled by its luminance's
        // change: weak channels of bright, saturated colours are not driven to black.
        let ratio = super::luminance(j).max(0.) / super::luminance(p).max(1e-9);
        std::array::from_fn(|c| j[c].max(CHANNEL_FLOOR * p[c] * ratio))
    }
}

/// Minimum over a (2r+1)² window, clamped at the borders: rows, then columns.
fn min_filter(x: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let line = |len: usize, get: &dyn Fn(usize) -> f32, out: &mut dyn FnMut(usize, f32)| {
        for i in 0..len {
            let (a, b) = (i.saturating_sub(r), (i + r).min(len - 1));
            out(i, (a..=b).map(get).fold(f32::INFINITY, f32::min));
        }
    };
    let mut rows = vec![0.; x.len()];
    for y in 0..h {
        line(w, &|i| x[y * w + i], &mut |i, v| rows[y * w + i] = v);
    }
    let mut out = vec![0.; x.len()];
    for c in 0..w {
        line(h, &|i| rows[i * w + c], &mut |i, v| out[i * w + c] = v);
    }
    out
}

/// He et al.'s guided filter of `x` with itself as the guide, box radius `r`.
fn guided(x: &[f32], w: usize, h: usize, r: usize, epsilon: f32) -> Vec<f32> {
    use crate::develop::local_tone::blur;
    let sq: Vec<f32> = x.iter().map(|v| v * v).collect();
    let (m, m2) = rayon::join(|| blur(x, w, h, r), || blur(&sq, w, h, r));
    let a: Vec<f32> = m
        .iter()
        .zip(&m2)
        .map(|(m, m2)| {
            let var = (m2 - m * m).max(0.);
            var / (var + epsilon)
        })
        .collect();
    let b: Vec<f32> = m.iter().zip(&a).map(|(m, a)| m - a * m).collect();
    let (a, b) = rayon::join(|| blur(&a, w, h, r), || blur(&b, w, h, r));
    x.iter()
        .zip(a.iter().zip(&b))
        .map(|(v, (a, b))| a * v + b)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hazy scene: a dark-to-mid gradient of slightly coloured surfaces, seen through
    /// a veil of bright bluish air that thickens toward the top.
    fn hazy(w: usize, h: usize) -> Vec<[f32; 3]> {
        let air = [0.6, 0.65, 0.75];
        (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 / w as f32, (i / w) as f32 / h as f32);
                let surface = [0.02 + 0.3 * x, 0.015 + 0.25 * x, 0.01 + 0.2 * x];
                let t = 0.35 + 0.5 * y;
                std::array::from_fn(|c| surface[c] * t + air[c] * (1. - t))
            })
            .collect()
    }

    #[test]
    fn omega_interpolates_between_the_fitted_amounts() {
        assert_eq!(omega(0.), 0.);
        assert!((omega(0.1) - 0.15).abs() < 1e-6);
        assert!((omega(0.2) - 0.3).abs() < 1e-6);
        assert!((omega(0.3) - 0.375).abs() < 1e-6);
        assert!((omega(0.4) - 0.45).abs() < 1e-6);
        assert!((omega(0.7) - 0.675).abs() < 1e-6);
        assert!((omega(1.) - 0.9).abs() < 1e-6);
        // Masks summing past +100 remove no more.
        assert!((omega(1.6) - 0.9).abs() < 1e-6);
    }

    /// The airlight is the colour of the haziest pixels, not of the brightest channel.
    #[test]
    fn the_airlight_comes_from_the_haziest_pixels() {
        let (w, h) = (100, 60);
        let mut scene = hazy(w, h);
        // A small saturated highlight: bright in one channel, dark in the others.
        for y in 30..33 {
            for x in 40..43 {
                scene[y * w + x] = [4., 0.05, 0.05];
            }
        }
        // The haziest pixels (by the dark channel): a white-ish patch of dense haze, larger
        // than 1% of the photo inside the dark channel's window.
        for y in 0..8 {
            for x in 0..14 {
                scene[y * w + x] = [0.8, 0.82, 0.9];
            }
        }
        let haze = Haze::of(&scene, w, h);
        for (a, want) in haze.air.iter().zip([0.8, 0.82, 0.9]) {
            assert!((a - want).abs() < 0.03, "{:?}", haze.air);
        }
    }

    #[test]
    fn dehaze_darkens_hazy_shadows_and_adds_contrast() {
        let (w, h) = (120, 80);
        let scene = hazy(w, h);
        let haze = Arc::new(Haze::of(&scene, w, h));
        let dehazing = Dehazing::new(haze.clone(), [w as u32, h as u32], 0.);
        let at =
            |x: usize, y: usize, s: f32| dehazing.apply(scene[y * w + x], s, [x as f32, y as f32]);
        let lum = super::super::luminance;
        // Denser haze at the top: more of it measured there.
        let density = |y: usize| haze.density[y * w + w / 2];
        assert!(density(5) > density(h - 5) + 0.1);
        for s in [0.2, 0.4, 1.] {
            let (y, dark, light) = (h / 3, 5, w - 5);
            let (d0, l0) = (lum(scene[y * w + dark]), lum(scene[y * w + light]));
            let (d, l) = (lum(at(dark, y, s)), lum(at(light, y, s)));
            assert!(d < d0, "{s}: shadows {d0} -> {d}");
            assert!(l / d > l0 / d0, "{s}: contrast {} -> {}", l0 / d0, l / d);
        }
        // More Dehaze removes more.
        let p = |s| lum(at(5, h / 3, s));
        assert!(p(1.) < p(0.4) && p(0.4) < p(0.2));
    }

    /// At no Dehaze the model is the identity, and Exposure scales the airlight with the
    /// scene, so an exposed pixel dehazes to the exposed result.
    #[test]
    fn identity_at_zero_and_exposure_scales_the_airlight() {
        let (w, h) = (64, 48);
        let scene = hazy(w, h);
        let haze = Arc::new(Haze::of(&scene, w, h));
        let plain = Dehazing::new(haze.clone(), [w as u32, h as u32], 0.);
        let exposed = Dehazing::new(haze, [w as u32, h as u32], 1.);
        let p = scene[20 * w + 10];
        assert_eq!(plain.apply(p, 0., [10., 20.]), p);
        let a = plain.apply(p, 0.6, [10., 20.]);
        let b = exposed.apply(p.map(|v| 2. * v), 0.6, [10., 20.]);
        for c in 0..3 {
            assert!((b[c] - 2. * a[c]).abs() < 1e-5, "{a:?} {b:?}");
        }
    }

    #[test]
    fn min_filter_takes_the_window_minimum_within_the_borders() {
        let mut x = vec![1.; 7 * 5];
        x[2 * 7 + 3] = 0.;
        let m = min_filter(&x, 7, 5, 1);
        for y in 0..5 {
            for xx in 0..7 {
                let near = (y as i32 - 2).abs() <= 1 && (xx as i32 - 3).abs() <= 1;
                assert_eq!(m[y * 7 + xx], if near { 0. } else { 1. }, "{xx} {y}");
            }
        }
    }
}
