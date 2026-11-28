//! Dehaze (positive and negative): unified dark-channel haze model on the scene tone
//! stage's input (docs/scene-tone-stage.md#dehaze).
//!
//! The haze is measured once per photo on its measurement copy at Exposure 0
//! ([`Haze::of_in_mask`]):
//! - the airlight `A`: the per-channel median of the haziest 0.17% of pixels, ranked by
//!   the dark channel (window minimum of the darkest channel) of the camera's
//!   white-balanced values;
//! - the optical-depth density map: the dark channel of the photo relative to `A`,
//!   smoothed with a wide joint bilateral filter guided by log2 luminance relative
//!   to `A`.
//!
//! Positive Dehaze removes haze via `J = (p − A) / t + A` with
//! `t = max(1 − ω(s) · d, t_min)` and physical cap `d ≤ min_c(p_c / A_c)` so sub-cell
//! edges never drive dark pixels below zero (#600). Negative Dehaze adds haze using the
//! same airlight and density map via `I = t · p + (1 − t) · A` with
//! `t = exp(−κ(|s|) · d)`.
use rayon::prelude::*;
use std::sync::Arc;

/// Window radius of the dark channel, relative to the measurement copy's long edge.
const RADIUS: f32 = 0.005;
/// Share of the haziest pixels whose per-channel median gives the airlight (0.17%,
/// matching Camera Raw 18.7's measured 0.15–0.17% majority threshold).
const HAZIEST_PER_TEN_THOUSAND: usize = 17;
/// Joint bilateral spatial σ, relative to the measurement copy's long edge.
const BILATERAL_SIGMA_S: f32 = 0.047;
/// Joint bilateral range σ on log2 relative luminance (`log2(Y / Y_A)`, stops).
const BILATERAL_SIGMA_R: f32 = 2.0;
/// Lower/upper clamps on the log2 relative luminance guide (stops).
const GUIDE_LO: f32 = -10.0;
const GUIDE_HI: f32 = 2.0;
/// Lowest transmission for positive Dehaze.
const MIN_TRANSMISSION: f32 = 0.28;
/// Share of the color-preserving result no channel falls below under positive Dehaze.
const CHANNEL_FLOOR: f32 = 0.5;
/// Scene-luminance ramp where Dehaze turns on (matches Camera Raw's dark floor near 2⁻¹⁰).
const DARK_FLOOR_LO: f32 = 0.0005;
const DARK_FLOOR_HI: f32 = 0.003;
/// Limits on each channel of the unit-luminance airlight: prevents a monochromatic
/// scene with near-zero luminance in ProPhoto blue from blowing up a channel.
const AIR_UNIT_RANGE: [f32; 2] = [0.1, 4.];
/// Positive Dehaze amounts and the fraction of the haze they remove; linear between them.
const OMEGA: [(f32, f32); 4] = [(0., 0.), (0.2, 0.23), (0.5, 0.46), (1., 0.88)];
/// Positive Dehaze haze-removal fractions for SDR rendered files (Camera Raw 18.7 on
/// `F⁻¹`-inverted JPEGs follows a slightly concave curve at intermediate amounts while
/// sharing the same +100 endpoint).
const OMEGA_RENDERED: [(f32, f32); 5] = [
    (0., 0.),
    (0.25, 0.26),
    (0.50, 0.50),
    (0.75, 0.72),
    (1., 0.88),
];
/// Negative Dehaze magnitudes `|s|` and optical-depth multiplier `κ(|s|)` in
/// `t = exp(−κ(|s|) · d)`; linear between knots.
const NEG_STRENGTH: [(f32, f32); 5] = [
    (0., 0.),
    (0.25, 0.50),
    (0.50, 1.25),
    (0.75, 2.30),
    (1.00, 4.30),
];
/// Negative Dehaze magnitudes `|s|` and optical-depth multiplier `κ(|s|)` in
/// `t = 1 / (1 + κ(|s|) · d)` for SDR rendered files.
const NEG_STRENGTH_RENDERED: [(f32, f32); 5] = [
    (0., 0.),
    (0.25, 0.55),
    (0.50, 1.45),
    (0.75, 3.00),
    (1.00, 6.50),
];
/// Guided-filter radius (relative to the measurement copy's long edge) and regularization
/// ε on linear relative luminance `clamp(Y / Y_A, 0, 2)` for SDR rendered files.
const RENDERED_GUIDE_RADIUS: f32 = 0.06;
const RENDERED_GUIDE_EPS: f32 = 0.02;

/// A photo's haze, from its measurement copy at the scene stage's input, Exposure 0.
#[derive(Debug, PartialEq)]
pub(crate) struct Haze {
    pub(crate) width: usize,
    pub(crate) height: usize,
    /// The airlight per channel, linear ProPhoto at Exposure 0.
    pub(crate) air: [f32; 3],
    /// The haze's density on the copy's grid (0 to 1): the smoothed dark channel of
    /// the photo relative to `air`. When `rendered` is `true`, holds the Fast Guided
    /// Filter linear coefficient planes `[mean_a, mean_b]` (`2 * width * height`
    /// unconstrained floats) reconstructed per pixel in [`Dehazing::sample_density`].
    pub(crate) density: Vec<f32>,
    /// Whether this haze was measured for an SDR rendered file (guided-filtered density,
    /// rendered `omega` knots, and `1 / (1 + κ·d)` negative transmission).
    pub(crate) rendered: bool,
}

impl Haze {
    /// The haze of `scene` (`width` × `height` linear ProPhoto values), ranking dark
    /// channel candidates by `ranked` (the same pixels in the camera's white-balanced
    /// space, where saturated lights retain more of their weakest channel than in
    /// ProPhoto).
    #[cfg(test)]
    pub(crate) fn of(ranked: &[[f32; 3]], scene: &[[f32; 3]], width: usize, height: usize) -> Self {
        Self::build(ranked, scene, width, height, false, None)
    }

    /// As [`Self::build`], restricting the airlight sample pool to cells where `active` is
    /// true (used for cropped negative scans so clamped border copies do not bias the
    /// haziest percentile).
    pub(crate) fn of_in_mask(
        ranked: &[[f32; 3]],
        scene: &[[f32; 3]],
        width: usize,
        height: usize,
        active: Option<&[bool]>,
    ) -> Self {
        Self::build(ranked, scene, width, height, false, active)
    }

    /// As [`Self::of`], for an SDR rendered file (`F⁻¹`-inverted JPEG/PNG/TIFF): refines
    /// the dark-channel density with He et al.'s guided filter on linear relative
    /// luminance `clamp(Y / Y_A, 0, 2)` rather than the 2-stop log joint bilateral
    /// filter, preventing bright sky optical depth from bleeding across horizons.
    #[cfg(test)]
    pub(crate) fn of_rendered(
        ranked: &[[f32; 3]],
        scene: &[[f32; 3]],
        width: usize,
        height: usize,
    ) -> Self {
        Self::build(ranked, scene, width, height, true, None)
    }

    /// As [`Self::of_in_mask`], for an SDR rendered file (`F⁻¹`-inverted JPEG/PNG/TIFF),
    /// restricting the airlight sample pool to `active`.
    pub(crate) fn of_rendered_in_mask(
        ranked: &[[f32; 3]],
        scene: &[[f32; 3]],
        width: usize,
        height: usize,
        active: Option<&[bool]>,
    ) -> Self {
        Self::build(ranked, scene, width, height, true, active)
    }

    fn build(
        ranked: &[[f32; 3]],
        scene: &[[f32; 3]],
        width: usize,
        height: usize,
        rendered: bool,
        active: Option<&[bool]>,
    ) -> Self {
        assert_eq!(scene.len(), width * height);
        assert_eq!(ranked.len(), scene.len());
        if let Some(mask) = active {
            assert_eq!(mask.len(), scene.len());
        }
        if scene.is_empty() {
            return Self {
                width,
                height,
                air: [1e-6; 3],
                density: Vec::new(),
                rendered,
            };
        }
        let long = width.max(height) as f32;
        let r = ((RADIUS * long).round() as usize).max(1);
        let darkest = |p: &[f32; 3]| p[0].min(p[1]).min(p[2]);
        let dark = min_filter(
            &ranked.iter().map(darkest).collect::<Vec<_>>(),
            width,
            height,
            r,
        );
        // Airlight: per-channel median over the haziest 0.17% of pixels (within `active`
        // when provided).
        let mut sorted: Vec<f32> = match active {
            Some(mask) => dark
                .iter()
                .zip(mask)
                .filter_map(|(&d, &on)| on.then_some(d))
                .collect(),
            None => dark.clone(),
        };
        let mut chans = [Vec::new(), Vec::new(), Vec::new()];
        if !sorted.is_empty() {
            let k = (sorted.len() - 1) * (10_000 - HAZIEST_PER_TEN_THOUSAND) / 10_000;
            let threshold = *sorted.select_nth_unstable_by(k, f32::total_cmp).1;
            for (i, (p, d)) in scene.iter().zip(&dark).enumerate() {
                if active.is_none_or(|mask| mask[i])
                    && *d >= threshold
                    && p.iter().all(|v| v.is_finite())
                {
                    for c in 0..3 {
                        chans[c].push(p[c].max(0.));
                    }
                }
            }
        }
        let raw_air = if chans[0].is_empty() {
            [1e-6; 3]
        } else {
            chans.map(|mut v| {
                let mid = v.len() / 2;
                let hi = *v.select_nth_unstable_by(mid, f32::total_cmp).1;
                if v.len() % 2 == 1 {
                    hi.max(1e-6)
                } else {
                    let lo = v[..mid].iter().copied().fold(f32::NEG_INFINITY, f32::max);
                    (0.5 * (lo + hi)).max(1e-6)
                }
            })
        };
        let air = clamp_airlight(raw_air);
        let ya = super::luminance(air).max(1e-6);

        let relative: Vec<f32> = scene
            .iter()
            .map(|p| {
                (p[0] / air[0])
                    .min(p[1] / air[1])
                    .min(p[2] / air[2])
                    .max(0.)
            })
            .collect();
        let raw = min_filter(&relative, width, height, r);
        let density = if rendered {
            let guide: Vec<f32> = scene
                .iter()
                .map(|p| clamp_guide(super::luminance(*p) / ya))
                .collect();
            let r_gf = ((RENDERED_GUIDE_RADIUS * long).round() as usize).max(1);
            // Store Fast Guided Filter linear coefficients `[mean_a, mean_b]` (2 * w * h)
            // so per-pixel evaluation reconstructs sharp optical depth against each
            // full-resolution pixel's own `clamp(Y / Y_A, 0, 2)`.
            let (ma, mb) = guided_filter_ab(&guide, &raw, width, height, r_gf, RENDERED_GUIDE_EPS);
            ma.into_iter()
                .chain(mb)
                .map(|v| if v.is_finite() { v } else { 0. })
                .collect()
        } else {
            let guide: Vec<f32> = scene
                .iter()
                .map(|p| {
                    (super::luminance(*p) / ya)
                        .max(2f32.powf(GUIDE_LO))
                        .log2()
                        .clamp(GUIDE_LO, GUIDE_HI)
                })
                .collect();
            let sigma_s = (BILATERAL_SIGMA_S * long).max(4.);
            joint_bilateral(&guide, &raw, width, height, sigma_s, BILATERAL_SIGMA_R)
                .into_iter()
                .map(|v| if v.is_finite() { v.clamp(0., 1.) } else { 0. })
                .collect()
        };
        Self {
            width,
            height,
            air,
            density,
            rendered,
        }
    }
}

/// Pull an extreme monochromatic airlight toward neutral while preserving its luminance,
/// so every unit-luminance channel stays inside [`AIR_UNIT_RANGE`].
fn clamp_airlight(air: [f32; 3]) -> [f32; 3] {
    let y = super::luminance(air);
    if !y.is_finite() || y <= 1e-6 {
        return air.map(|v| v.max(1e-6));
    }
    let unit = air.map(|v| v / y);
    let mut keep = 1.0f32;
    for u in unit {
        if u < AIR_UNIT_RANGE[0] {
            keep = keep.min((1. - AIR_UNIT_RANGE[0]) / (1. - u));
        } else if u > AIR_UNIT_RANGE[1] {
            keep = keep.min((AIR_UNIT_RANGE[1] - 1.) / (u - 1.));
        }
    }
    let keep = keep.clamp(0., 1.);
    unit.map(|u| ((1. + keep * (u - 1.)) * y).max(1e-6))
}

fn piecewise(table: &[(f32, f32)], s: f32) -> f32 {
    let s = s.clamp(0., 1.);
    let j = table
        .windows(2)
        .position(|w| s <= w[1].0)
        .unwrap_or(table.len() - 2);
    let ((s0, v0), (s1, v1)) = (table[j], table[j + 1]);
    v0 + (v1 - v0) * (s - s0) / (s1 - s0)
}

/// The fraction of the haze positive Dehaze `s` (0 to 1) removes.
pub(crate) fn omega(s: f32) -> f32 {
    piecewise(&OMEGA, s)
}

/// As [`omega`], for SDR rendered files.
pub(crate) fn omega_rendered(s: f32) -> f32 {
    piecewise(&OMEGA_RENDERED, s)
}

/// [`omega`] of an HDR master, whose masks' Dehaze can take a pixel past +100
/// ([`super::hdr_mask_dehaze`]): beyond it the table's last segment continues.
pub(crate) fn omega_hdr(s: f32) -> f32 {
    if s <= 1. {
        return omega(s);
    }
    let [.., (s0, w0), (s1, w1)] = OMEGA;
    w1 + (w1 - w0) / (s1 - s0) * (s - s1)
}

/// Optical-depth multiplier `κ(|s|)` for negative Dehaze `s < 0` (`mag = -s`, 0 to 1).
pub(crate) fn neg_strength(mag: f32) -> f32 {
    piecewise(&NEG_STRENGTH, mag)
}

/// As [`neg_strength`], for SDR rendered files (`t = 1 / (1 + κ(|s|) · d)`).
pub(crate) fn neg_strength_rendered(mag: f32) -> f32 {
    piecewise(&NEG_STRENGTH_RENDERED, mag)
}

/// Clamps linear relative luminance `Y / Y_A` to `[0, 2]`, mapping `NaN` to `0` so a
/// non-finite pixel cannot poison the guided-filter running-sum blur.
#[inline]
fn clamp_guide(v: f32) -> f32 {
    if v.is_nan() { 0. } else { v.clamp(0., 2.) }
}

/// Smooth dark luminance ramp (0 below [`DARK_FLOOR_LO`], 1 above [`DARK_FLOOR_HI`]).
#[inline]
fn dark_weight(y: f32) -> f32 {
    let u = ((y - DARK_FLOOR_LO) / (DARK_FLOOR_HI - DARK_FLOOR_LO)).clamp(0., 1.);
    u * u * (3. - 2. * u)
}

/// A photo's haze as one render applies it: at that render's Exposure and sample grid.
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

    /// Samples the optical-depth density at camera-image position `pos`. For SDR rendered
    /// files, reconstructs He et al.'s Fast Guided Filter `clamp(mean_a * clamp(y_rel, 0, 2) + mean_b, 0, 1)`.
    #[inline]
    fn sample_density(&self, pos: [f32; 2], y_rel: f32) -> f32 {
        let size = [self.haze.width, self.haze.height];
        if self.haze.rendered {
            let n = self.haze.width * self.haze.height;
            let ma = crate::develop::local_tone::grid_sample(
                &self.haze.density[..n],
                size,
                self.scale,
                pos,
            );
            let mb = crate::develop::local_tone::grid_sample(
                &self.haze.density[n..],
                size,
                self.scale,
                pos,
            );
            (ma * clamp_guide(y_rel) + mb).clamp(0., 1.)
        } else {
            crate::develop::local_tone::grid_sample(&self.haze.density, size, self.scale, pos)
        }
    }

    /// Positive Dehaze: pixel `p` at camera-image sample position `pos` with the
    /// fraction `omega` of the haze removed: `J = (p − A) / t + A`, with
    /// `t = max(1 − omega · d, MIN_TRANSMISSION)`, `d` capped at the pixel's own
    /// relative dark channel so sub-cell edges never clip dark pixels below zero (#600),
    /// and each channel at least `CHANNEL_FLOOR` of `p` scaled by the luminance's change.
    /// `tone_gain` is the local tone and mask Exposure gain already applied to `p`.
    pub(crate) fn apply(&self, p: [f32; 3], omega: f32, pos: [f32; 2], tone_gain: f32) -> [f32; 3] {
        if omega <= 0. || self.haze.density.is_empty() {
            return p;
        }
        // Haze was measured before local tone and mask Exposure. Apply the same
        // gain to its airlight as to the pixel, so negative Highlights cannot
        // subtract an unchanged bright sky from a now darker one (#600).
        let air = self.air.map(|v| (v * tone_gain).max(1e-9));
        let y = super::luminance(p).max(0.);
        let y_rel = if self.haze.rendered {
            y / super::luminance(air).max(1e-9)
        } else {
            0.
        };
        let density = self.sample_density(pos, y_rel);
        let pixel_dc = (p[0] / air[0])
            .min(p[1] / air[1])
            .min(p[2] / air[2])
            .max(0.);
        let d = density.min(pixel_dc) * dark_weight(y);
        let drop = (omega * d).min(1. - MIN_TRANSMISSION);
        let t = 1. - drop;
        // Algebraically `(p - A) / t + A = (p - (1 - t) * A) / t`, avoiding catastrophic
        // cancellation when `A >> p` and `t` is near 1.
        let j: [f32; 3] = std::array::from_fn(|c| (p[c] - drop * air[c]) / t);
        // No channel falls below half of the pixel's color scaled by its luminance's
        // change: weak channels of bright, saturated colors are not driven to black.
        let ratio = super::luminance(j).max(0.) / y.max(1e-9);
        std::array::from_fn(|c| j[c].max(CHANNEL_FLOOR * p[c] * ratio))
    }

    /// Negative Dehaze: adds atmospheric haze `I = t · p + (1 − t) · A` using the same
    /// airlight and optical-depth map. RAW files use `t = exp(−κ(|s|) · d)` and SDR
    /// rendered files use `t = 1 / (1 + κ(|s|) · d)`, both with `d ≤ Y / Y_A`.
    pub(crate) fn apply_negative(
        &self,
        p: [f32; 3],
        strength: f32,
        pos: [f32; 2],
        tone_gain: f32,
    ) -> [f32; 3] {
        if strength <= 0. || self.haze.density.is_empty() {
            return p;
        }
        let air = self.air.map(|v| v * tone_gain);
        let y = super::luminance(p).max(0.);
        let ya = super::luminance(air).max(1e-9);
        let y_rel = y / ya;
        let density = self.sample_density(pos, y_rel);
        let d = density.min(y_rel) * dark_weight(y);
        let t = if self.haze.rendered {
            1. / (1. + strength * d)
        } else {
            (-strength * d).exp()
        };
        std::array::from_fn(|c| t * p[c] + (1. - t) * air[c])
    }
}

/// He et al.'s Fast Guided Filter linear coefficients `(mean_a, mean_b)` of `p` guided by
/// `guide` with box radius `r` and variance regularization `eps`, used to reconstruct
/// sharp full-resolution optical depth on SDR rendered files.
fn guided_filter_ab(
    guide: &[f32],
    p: &[f32],
    w: usize,
    h: usize,
    r: usize,
    eps: f32,
) -> (Vec<f32>, Vec<f32>) {
    let mean = |x: &[f32]| crate::develop::local_tone::blur(x, w, h, r);
    let ii: Vec<f32> = guide.iter().map(|&v| v * v).collect();
    let ip: Vec<f32> = guide.iter().zip(p).map(|(&g, &pv)| g * pv).collect();
    let ((m_i, m_p), (m_ii, m_ip)) = rayon::join(
        || rayon::join(|| mean(guide), || mean(p)),
        || rayon::join(|| mean(&ii), || mean(&ip)),
    );
    let mut a = Vec::with_capacity(guide.len());
    let mut b = Vec::with_capacity(guide.len());
    for i in 0..guide.len() {
        let var = (m_ii[i] - m_i[i] * m_i[i]).max(0.);
        let cov = m_ip[i] - m_i[i] * m_p[i];
        let ai = cov / (var + eps);
        a.push(ai);
        b.push(m_p[i] - ai * m_i[i]);
    }
    rayon::join(|| mean(&a), || mean(&b))
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

/// Joint bilateral filter of `p` guided by `guide` at spatial σ `sigma_s` (px) and
/// range σ `sigma_r`: Durand–Dorsey piecewise linear bilateral grid, with each level
/// blurred on a grid reduced to about a third of `sigma_s` and interpolated back.
fn joint_bilateral(
    guide: &[f32],
    p: &[f32],
    w: usize,
    h: usize,
    sigma_s: f32,
    sigma_r: f32,
) -> Vec<f32> {
    let (lo, hi) = guide
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), v| {
            (a.min(*v), b.max(*v))
        });
    if !lo.is_finite() || !hi.is_finite() || hi - lo < 1e-6 {
        return gauss(p, w, h, sigma_s);
    }
    let count = (((hi - lo) / (sigma_r * 0.5)).floor() as usize + 2).clamp(2, 16);
    let step = (hi - lo) / (count - 1) as f32;
    let factor = ((sigma_s / 3.).floor() as usize).max(1);
    let sigma = sigma_s / factor as f32;

    let planes: Vec<(Vec<f32>, usize, usize)> = (0..count)
        .into_par_iter()
        .map(|j| {
            let level = lo + step * j as f32;
            let mut weight = Vec::with_capacity(guide.len());
            let mut weighted = Vec::with_capacity(guide.len());
            for (&g, &pv) in guide.iter().zip(p) {
                let d = (g - level) / sigma_r;
                let wt = (-0.5 * d * d).exp();
                weight.push(wt);
                weighted.push(wt * pv);
            }
            let (den, gw, gh) = reduce(&weight, w, h, factor);
            let (num, ..) = reduce(&weighted, w, h, factor);
            let (den, num) = (gauss(&den, gw, gh, sigma), gauss(&num, gw, gh, sigma));
            let values = num
                .iter()
                .zip(&den)
                .map(|(n, d)| n / d.max(1e-12))
                .collect();
            (values, gw, gh)
        })
        .collect();

    let sample = |plane: &(Vec<f32>, usize, usize), x: usize, y: usize| -> f32 {
        let (values, gw, gh) = (&plane.0, plane.1, plane.2);
        if factor == 1 {
            return values[y * gw + x];
        }
        let f = factor as f32;
        let fx = ((x as f32 + 0.5) / f - 0.5).clamp(0., (gw - 1) as f32);
        let fy = ((y as f32 + 0.5) / f - 0.5).clamp(0., (gh - 1) as f32);
        let (ix, iy) = (fx as usize, fy as usize);
        let (jx, jy) = ((ix + 1).min(gw - 1), (iy + 1).min(gh - 1));
        let (tx, ty) = (fx - ix as f32, fy - iy as f32);
        let top = values[iy * gw + ix] * (1. - tx) + values[iy * gw + jx] * tx;
        let bottom = values[jy * gw + ix] * (1. - tx) + values[jy * gw + jx] * tx;
        top * (1. - ty) + bottom * ty
    };

    (0..guide.len())
        .into_par_iter()
        .map(|i| {
            let g = guide[i];
            let t = ((g - lo) / step).clamp(0., (count - 1) as f32);
            let j = (t.floor() as usize).min(count - 2);
            let fr = t - j as f32;
            let (px, py) = (i % w, i / w);
            sample(&planes[j], px, py) * (1. - fr) + sample(&planes[j + 1], px, py) * fr
        })
        .collect()
}

/// Means of `f` × `f` blocks (partial at the right and bottom edges).
fn reduce(x: &[f32], w: usize, h: usize, f: usize) -> (Vec<f32>, usize, usize) {
    if f == 1 {
        return (x.to_vec(), w, h);
    }
    let (gw, gh) = (w.div_ceil(f), h.div_ceil(f));
    let mut sum = vec![0f32; gw * gh];
    let mut count = vec![0u32; gw * gh];
    for (i, v) in x.iter().enumerate() {
        let g = (i / w / f) * gw + (i % w) / f;
        sum[g] += v;
        count[g] += 1;
    }
    (
        sum.iter().zip(&count).map(|(s, c)| s / *c as f32).collect(),
        gw,
        gh,
    )
}

/// A Gaussian blur of σ `sigma` px as three box blurs, clamped at the borders.
fn gauss(x: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    if sigma < 0.5 || w == 0 || h == 0 {
        return x.to_vec();
    }
    let r = (((12. * sigma * sigma / 3. + 1.).sqrt() / 2.).round() as usize).max(1);
    let rows = boxes(x, w, r);
    let columns = boxes(&transpose(&rows, w, h), h, r);
    transpose(&columns, h, w)
}

fn boxes(x: &[f32], w: usize, r: usize) -> Vec<f32> {
    let mut out = x.to_vec();
    let mut line = vec![0f32; w];
    for row in out.chunks_mut(w) {
        for _ in 0..3 {
            let get = |i: isize| row[i.clamp(0, w as isize - 1) as usize];
            let mut sum: f32 = (-(r as isize)..=r as isize).map(get).sum();
            for (i, v) in line.iter_mut().enumerate() {
                *v = sum / (2 * r + 1) as f32;
                sum += get(i as isize + r as isize + 1) - get(i as isize - r as isize);
            }
            row.copy_from_slice(&line);
        }
    }
    out
}

fn transpose(x: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0.; x.len()];
    for y in 0..h {
        for c in 0..w {
            out[c * h + y] = x[y * w + c];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dehaze_transforms_airlight_with_prior_tone_and_mask_exposure() {
        let (w, h) = (120, 80);
        let scene = hazy(w, h);
        let dehazing = Dehazing::new(
            Arc::new(Haze::of(&scene, &scene, w, h)),
            [w as u32, h as u32],
            0.,
        );
        let (p, pos) = (scene[25 * w + 10], [10., 25.]);
        for amount in [0.5, 1.] {
            let base = dehazing.apply(p, omega(amount), pos, 1.);
            for gain in [0.125, 0.5, 2.] {
                let adjusted = dehazing.apply(p.map(|v| v * gain), omega(amount), pos, gain);
                for c in 0..3 {
                    assert!((adjusted[c] - base[c] * gain).abs() < 1e-5);
                    assert!(adjusted[c] > 0.);
                }
            }
        }
    }

    /// A hazy scene: a dark-to-mid gradient of slightly colored surfaces, seen through
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
        assert!((omega(0.2) - 0.23).abs() < 1e-6);
        assert!((omega(0.5) - 0.46).abs() < 1e-6);
        assert!((omega(1.) - 0.88).abs() < 1e-6);
        // Masks summing past +100 remove no more.
        assert!((omega(1.6) - 0.88).abs() < 1e-6);
    }

    /// The airlight is the color of the haziest pixels, not of the brightest channel.
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
        // than 0.17% of the photo inside the dark channel's window.
        for y in 0..8 {
            for x in 0..14 {
                scene[y * w + x] = [0.8, 0.82, 0.9];
            }
        }
        let haze = Haze::of(&scene, &scene, w, h);
        for (a, want) in haze.air.iter().zip([0.8, 0.82, 0.9]) {
            assert!((a - want).abs() < 0.03, "{:?}", haze.air);
        }
    }

    #[test]
    fn dehaze_darkens_hazy_shadows_and_adds_contrast() {
        let (w, h) = (120, 80);
        let scene = hazy(w, h);
        let haze = Arc::new(Haze::of(&scene, &scene, w, h));
        let dehazing = Dehazing::new(haze.clone(), [w as u32, h as u32], 0.);
        let at = |x: usize, y: usize, s: f32| {
            dehazing.apply(scene[y * w + x], omega(s), [x as f32, y as f32], 1.)
        };
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

    #[test]
    fn negative_dehaze_adds_veil_toward_airlight_monotonically() {
        let (w, h) = (120, 80);
        let scene = hazy(w, h);
        let haze = Arc::new(Haze::of(&scene, &scene, w, h));
        let dehazing = Dehazing::new(haze, [w as u32, h as u32], 0.);
        let (x, y) = (10, h / 3);
        let p = scene[y * w + x];
        let lum = super::super::luminance;
        let l0 = lum(p);
        let l50 = lum(dehazing.apply_negative(p, neg_strength(0.5), [x as f32, y as f32], 1.));
        let l100 = lum(dehazing.apply_negative(p, neg_strength(1.0), [x as f32, y as f32], 1.));
        let la = lum(dehazing.air);
        assert!(
            l0 < l50 && l50 < l100 && l100 <= la,
            "{l0} {l50} {l100} {la}"
        );
    }

    /// At no Dehaze the model is the identity, and Exposure scales the airlight with the
    /// scene, so an exposed pixel above the dark floor dehazes to the exposed result.
    #[test]
    fn identity_at_zero_and_exposure_scales_the_airlight() {
        let (w, h) = (64, 48);
        let scene = hazy(w, h);
        let haze = Arc::new(Haze::of(&scene, &scene, w, h));
        let plain = Dehazing::new(haze.clone(), [w as u32, h as u32], 0.);
        let exposed = Dehazing::new(haze, [w as u32, h as u32], 1.);
        let p = scene[20 * w + 10];
        assert_eq!(plain.apply(p, omega(0.), [10., 20.], 1.), p);
        assert_eq!(plain.apply_negative(p, neg_strength(0.), [10., 20.], 1.), p);
        let a = plain.apply(p, omega(0.6), [10., 20.], 1.);
        let b = exposed.apply(p.map(|v| 2. * v), omega(0.6), [10., 20.], 1.);
        for c in 0..3 {
            assert!((b[c] - 2. * a[c]).abs() < 1e-5, "{a:?} {b:?}");
        }
        let na = plain.apply_negative(p, neg_strength(0.6), [10., 20.], 1.);
        let nb = exposed.apply_negative(p.map(|v| 2. * v), neg_strength(0.6), [10., 20.], 1.);
        for c in 0..3 {
            assert!((nb[c] - 2. * na[c]).abs() < 1e-5, "{na:?} {nb:?}");
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

    #[test]
    fn rendered_haze_preserves_horizon_edge_exposure_invariance_and_tables() {
        let (w, h) = (120, 80);
        // Bright hazy sky in top half, dark clean foreground in bottom half.
        let scene: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let y = i / w;
                if y < h / 2 {
                    [0.75, 0.78, 0.85]
                } else {
                    [0.04, 0.05, 0.04]
                }
            })
            .collect();
        let raw_haze = Haze::of(&scene, &scene, w, h);
        let ren_haze = Haze::of_rendered(&scene, &scene, w, h);
        assert!(!raw_haze.rendered);
        assert!(ren_haze.rendered);
        assert_eq!(ren_haze.density.len(), 2 * w * h);
        assert_eq!(raw_haze.air, ren_haze.air);

        let ren_d = Dehazing::new(Arc::new(ren_haze), [w as u32, h as u32], 0.);
        let sky_pos = [60., 15.];
        let fg_pos = [60., (h / 2 + 2) as f32];
        let sky_p = scene[15 * w + 60];
        let fg_p = scene[(h / 2 + 2) * w + 60];
        let ya = super::super::luminance(ren_d.air).max(1e-9);
        let d_sky = ren_d.sample_density(sky_pos, super::super::luminance(sky_p) / ya);
        let d_fg = ren_d.sample_density(fg_pos, super::super::luminance(fg_p) / ya);
        assert!(
            d_sky > 0.8 && d_fg < 0.2,
            "horizon bleed: d_sky={d_sky}, d_fg={d_fg}"
        );

        // Monotonicity of rendered tables and exact identity at 0.
        assert_eq!(omega_rendered(0.), 0.);
        assert_eq!(neg_strength_rendered(0.), 0.);
        assert!(omega_rendered(0.75) > omega(0.75));
        assert!((omega_rendered(1.) - omega(1.)).abs() < 1e-6);

        // Exposure and tone_gain invariance for both positive and negative rendered Dehaze.
        let exposed = Dehazing::new(ren_d.haze.clone(), [w as u32, h as u32], 1.);
        let p = [0.25, 0.28, 0.34];
        let pos = [40., 25.];
        let a = ren_d.apply(p, omega_rendered(0.75), pos, 1.);
        let b = exposed.apply(p.map(|v| 2. * v), omega_rendered(0.75), pos, 1.);
        let g = ren_d.apply(p.map(|v| 0.5 * v), omega_rendered(0.75), pos, 0.5);
        for c in 0..3 {
            assert!((b[c] - 2. * a[c]).abs() < 1e-5, "{a:?} {b:?}");
            assert!((g[c] - 0.5 * a[c]).abs() < 1e-5, "{a:?} {g:?}");
        }
        let na = ren_d.apply_negative(p, neg_strength_rendered(0.75), pos, 1.);
        let nb = exposed.apply_negative(p.map(|v| 2. * v), neg_strength_rendered(0.75), pos, 1.);
        for c in 0..3 {
            assert!((nb[c] - 2. * na[c]).abs() < 1e-5, "{na:?} {nb:?}");
        }
        assert!(super::super::luminance(na) > super::super::luminance(p));
    }
}
