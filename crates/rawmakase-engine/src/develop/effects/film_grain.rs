//! Effects > Film Grain: the grain of film as a print shows it, after Newson, Delon &
//! Galerne, "A Stochastic Film Grain Model for Resolution-Independent Rendering"
//! (2017), in the limit their model takes when grains are much smaller than a pixel.
//!
//! Each pixel of a print averages many silver grains, so what shows is not single grains
//! but how many fall where: a continuous, clumpy random texture. It is drawn here as
//! sparse convolution noise: soft bumps of lognormal radius and random weight at random
//! positions in full-resolution pixels, so a preview, a region and the export show the
//! same grain. Each of `CANDIDATES` staggered grids of square cells holds one bump a
//! cell, of hashed centre, radius and weight, so there is no lattice, and Roughness
//! mixes bump sizes. The texture is scaled to unit deviation and by tone: for a negative
//! of small grains, the share of light a pixel lets through, `t`, varies as
//! `t·√(−ln t)`, so the grain is strongest in the lower midtones and fades smoothly into
//! black and white, with no isolated grains at either end.
//!
//! Film's grain is also uneven: its strength varies by a few percent from one 0.2–0.5 mm
//! patch to the next. A coarse noise of the same kind modulates it, more with Roughness,
//! keeping its mean strength.
//!
//! The grain is not equally strong everywhere: as in scans of film, its strength varies
//! by a few percent from one 0.2–0.5 mm patch to the next, more with Roughness. A
//! coarse noise modulates it, scaled so the grain keeps its overall strength.
//!
//! Previews down to half size average exactly the full-resolution pixels under each
//! pixel; smaller ones take one sample a pixel and scale what is left: a statistical
//! approximation of the resized export, as the measured Grain's.
//!
//! Color adds the colour of colour film's grain: each dye layer has a grain of its own,
//! as fine as the brightness grain, over its own channel. Only the layers' differences
//! from their luminance are added, so the grain's brightness stays the mono grain's,
//! and at Color 1 the channels' grain correlates and varies as in scans of colour
//! negatives.
use crate::model::film_grain::FilmGrain;

/// The long edge, in pixels, at which grain sizes below are given.
const REFERENCE_EDGE: f32 = 6000.;
/// Mean grain radius (pixels at the reference edge) at Size 0 and per unit of Size, and
/// the smallest in pixels at any photo size: smaller grain would be a pixel's noise.
const RADIUS_BASE: f32 = 0.5;
const RADIUS_PER_SIZE: f32 = 1.5;
const RADIUS_MIN: f32 = 0.5;
/// Lognormal log standard deviation at Roughness 0 and per unit of Roughness.
const SIGMA_BASE: f32 = 0.1;
const SIGMA_PER_ROUGHNESS: f32 = 0.6;
/// A bump reaches `KERNEL` times its radius.
const KERNEL: f32 = 2.2;
/// The largest radius, in mean radii: the lognormal's tail beyond is clamped.
const TAIL: f32 = 2.;
/// The grain's deviation at Amount 1 in the midtones, in encoded values.
pub(crate) const STRENGTH: f32 = 0.12;
/// Candidate bumps per cell.
pub(crate) const CANDIDATES: u32 = 8;
/// Clumping: the grain's strength varies slowly, as in scans of film, where it varies
/// about 1.3 (colour negatives, T-Max) to 1.65 (Tri-X) times as much from one 0.2 mm
/// patch to the next as an even grain's. A coarse noise of `CLUMP_RADIUS` pixels at the
/// reference edge, `CLUMP_COUNT` bumps per cell, modulates it by `CLUMP_BASE` plus
/// `CLUMP_PER_ROUGHNESS` times Roughness.
const CLUMP_RADIUS: f32 = 30.;
const CLUMP_SIGMA: f32 = 0.4;
pub(crate) const CLUMP_COUNT: u32 = 2;
const CLUMP_BASE: f32 = 0.03;
const CLUMP_PER_ROUGHNESS: f32 = 0.05;
const SALT_CLUMP: u32 = 0x5be0_cd19;
/// `t·√(−ln t)` at its peak, `t = e^(−1/2)`.
const TONE_PEAK: f32 = 0.428_881;
/// Hash salts for a candidate's centre, weight and radius.
const SALT_X: u32 = 0xb5ad_4ece;
const SALT_Y: u32 = 0x1b87_3593;
const SALT_WEIGHT: u32 = 0x68e3_1da4;
const SALT_RADIUS: u32 = 0xcc9e_2d51;
const SALT_ANGLE: u32 = 0x27d4_eb2f;
const CANDIDATE_STEP: u32 = 0x632b_e5ab;
/// Colour grain: each dye layer's grain size relative to the mono grain, red, green,
/// blue, and its seed's salt. In scans of colour negatives (Kodak Ektar 100 and
/// Ultramax 400, Nikon Coolscan V, 4000 dpi) every layer's grain is as fine as the
/// grain's brightness.
const LAYER_SIZE: [f32; 3] = [1., 1., 1.];
/// How much of the dye layers' colour Color 1 adds, and how much of the red and blue
/// layers' grain each takes, negated, from the other. Fitted to those scans, where the
/// channels' grain correlates by 0.31–0.51 (red–green), 0.35–0.44 (green–blue) and
/// 0.14–0.20 (red–blue), and its colour varies 1.2–1.35 times as much as its
/// brightness: these give 0.33, 0.47, 0.18 and 1.17.
pub(crate) const COLOR_STRENGTH: f32 = 1.1;
const OUTER_COUPLING: f32 = -0.425;
const LAYER_SALT: [u32; 3] = [0x3c6e_f372, 0x9b05_688c, 0xa54f_f53a];
/// The R2 low-discrepancy sequence's steps, offsetting each candidate's grid.
const R2: [f32; 2] = [0.754_877_7, 0.569_840_3];

/// Integer hash to `[0, 1)` in 24-bit steps, which `f32` holds exactly on the CPU and
/// the GPU.
pub(crate) fn unit_hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut v = (x as u32).wrapping_mul(0x9e37_79b9) ^ (y as u32).wrapping_mul(0x85eb_ca6b) ^ seed;
    v ^= v >> 16;
    v = v.wrapping_mul(0x7feb_352d);
    v ^= v >> 15;
    v = v.wrapping_mul(0x846c_a68b);
    v ^= v >> 16;
    (v >> 8) as f32 / 16_777_216.
}
/// The grain's strength over encoded tone `l`, 1 at its peak: `t·√(−ln t)` for the
/// light a negative lets through, `t = 1 − l`.
pub(crate) fn tone_strength(l: f32) -> f32 {
    let t = (1. - l).clamp(0., 1.);
    if t <= 0. || t >= 1. {
        return 0.;
    }
    t * (-t.ln()).sqrt() / TONE_PEAK
}

/// A recipe's Film Grain for an output whose full-resolution long edge is `edge`
/// pixels: the mono grain, and the red, green and blue dye layers' for Color. Shared
/// by the CPU and the GPU preview.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FilmGrainLayers {
    pub(crate) color: f32,
    pub(crate) mono: FilmGrainField,
    /// Red, green and blue.
    pub(crate) layers: [FilmGrainField; 3],
    /// The coarse noise that modulates the grain's strength, and by how much.
    pub(crate) clump: FilmGrainField,
    pub(crate) clumping: f32,
}
impl FilmGrainLayers {
    pub(crate) fn new(g: &FilmGrain, edge: f32) -> Self {
        Self {
            color: g.color,
            mono: FilmGrainField::layer(g, edge, 1., g.seed),
            layers: std::array::from_fn(|c| {
                FilmGrainField::layer(g, edge, LAYER_SIZE[c], g.seed ^ LAYER_SALT[c])
            }),
            clump: FilmGrainField::with(
                1.,
                (CLUMP_RADIUS * edge / REFERENCE_EDGE).max(2.),
                CLUMP_SIGMA,
                CLUMP_COUNT,
                g.seed ^ SALT_CLUMP,
            ),
            clumping: CLUMP_BASE + CLUMP_PER_ROUGHNESS * g.roughness,
        }
    }
    /// How much stronger or weaker than its mean the grain is at full-resolution
    /// position `x`, `y`: 1 on average over the photo, so Amount keeps its strength.
    pub(crate) fn clumps(&self, x: f32, y: f32) -> f32 {
        let a = self.clumping;
        (1. + a * self.clump.noise(x, y)).max(0.) / (1. + a * a).sqrt()
    }
    /// What the grain adds to each channel of encoded `rgb` at full-resolution
    /// position `x`, `y`, for an output of `scale` pixels per full-resolution pixel:
    /// the mono grain over the luminance, and with Color each dye layer's grain over its
    /// own channel less what they add to the luminance, so Color changes the grain's
    /// colour, not its brightness.
    pub(crate) fn delta(&self, x: f32, y: f32, scale: f32, rgb: [f32; 3]) -> [f32; 3] {
        let mono = self.mono.delta(x, y, scale, crate::color::luminance(rgb));
        if self.color == 0. {
            if mono == 0. {
                return [0.; 3];
            }
            return [mono * self.clumps(x, y); 3];
        }
        let [r, g, b] = std::array::from_fn(|c| self.layers[c].delta(x, y, scale, rgb[c]));
        let dye = [r + OUTER_COUPLING * b, g, b + OUTER_COUPLING * r];
        let neutral = crate::color::luminance(dye);
        let clumps = self.clumps(x, y);
        dye.map(|d| (mono + COLOR_STRENGTH * self.color * (d - neutral)) * clumps)
    }
}

/// One grain layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FilmGrainField {
    pub(crate) amount: f32,
    /// Mean radius and the radii's lognormal log standard deviation.
    pub(crate) radius: f32,
    pub(crate) sigma: f32,
    /// Cell side; every bump lies within a cell of its own.
    pub(crate) cell: f32,
    pub(crate) max_radius: f32,
    /// What scales the noise to unit deviation.
    pub(crate) norm: f32,
    pub(crate) seed: u32,
    /// Candidate bumps per cell.
    pub(crate) count: u32,
}
impl FilmGrainField {
    /// The mono grain.
    #[cfg(test)]
    pub(crate) fn new(g: &FilmGrain, edge: f32) -> Self {
        Self::layer(g, edge, 1., g.seed)
    }
    /// A layer whose grains are `size` times the mono grain's, with `seed`.
    fn layer(g: &FilmGrain, edge: f32, size: f32, seed: u32) -> Self {
        let radius = ((RADIUS_BASE + RADIUS_PER_SIZE * g.size) * size * edge / REFERENCE_EDGE)
            .max(RADIUS_MIN);
        let sigma = SIGMA_BASE + SIGMA_PER_ROUGHNESS * g.roughness;
        Self::with(g.amount, radius, sigma, CANDIDATES, seed)
    }
    /// A noise of mean radius `radius` with `count` bumps per cell, scaled by `amount`.
    fn with(amount: f32, radius: f32, sigma: f32, count: u32, seed: u32) -> Self {
        let max_radius = TAIL * radius;
        let cell = KERNEL * max_radius;
        let mut field = Self {
            amount,
            radius,
            sigma,
            cell,
            max_radius,
            norm: 1.,
            seed,
            count,
        };
        // A bump `(1 − (d/R)²)²` of reach `R` has `∫b² = πR²/5`; weights uniform in
        // −1..1 have `E[w²] = 1/3`; there are `count` bumps per cell. The mean over
        // the normal `z` of each radius's `R²`, by the midpoint rule over ±5.
        let nodes = 80;
        let (mut reach2, mut weights) = (0., 0.);
        for i in 0..nodes {
            let z = -5. + (i as f32 + 0.5) * 10. / nodes as f32;
            let weight = (-0.5 * z * z).exp();
            reach2 += weight * (KERNEL * field.grain_radius(z)).powi(2);
            weights += weight;
        }
        let variance =
            count as f32 / (cell * cell) / 3. * std::f32::consts::PI * (reach2 / weights) / 5.;
        field.norm = 1. / variance.sqrt();
        field
    }
    fn grain_radius(&self, z: f32) -> f32 {
        (self.radius * (self.sigma * z - 0.5 * self.sigma * self.sigma).exp()).min(self.max_radius)
    }
    /// The grain texture at full-resolution position `x`, `y`: mean 0, deviation 1.
    pub(crate) fn noise(&self, x: f32, y: f32) -> f32 {
        let mut sum = 0.;
        for k in 0..self.count {
            // Each candidate's grid is offset by the R2 sequence, so a position's phase
            // within its cell differs between candidates and no cell pattern shows.
            let ox = (0.5 + k as f32 * R2[0]).fract() * self.cell;
            let oy = (0.5 + k as f32 * R2[1]).fract() * self.cell;
            let (lx, ly) = (x - ox, y - oy);
            let cx = (lx / self.cell).floor() as i32;
            let cy = (ly / self.cell).floor() as i32;
            let key = self.seed ^ k.wrapping_mul(CANDIDATE_STEP);
            for j in -1..=1 {
                for i in -1..=1 {
                    let (gx, gy) = (cx + i, cy + j);
                    let px = (gx as f32 + unit_hash(gx, gy, key ^ SALT_X)) * self.cell;
                    let py = (gy as f32 + unit_hash(gx, gy, key ^ SALT_Y)) * self.cell;
                    let d2 = (lx - px).powi(2) + (ly - py).powi(2);
                    if d2 >= self.cell * self.cell {
                        continue;
                    }
                    // Box–Muller.
                    let u = 1. - unit_hash(gx, gy, key ^ SALT_RADIUS);
                    let angle = unit_hash(gx, gy, key ^ SALT_ANGLE);
                    let z = (-2. * u.ln()).sqrt() * (2. * std::f32::consts::PI * angle).cos();
                    let reach = KERNEL * self.grain_radius(z);
                    let b = 1. - d2 / (reach * reach);
                    if b <= 0. {
                        continue;
                    }
                    let weight = 2. * unit_hash(gx, gy, key ^ SALT_WEIGHT) - 1.;
                    sum += weight * b * b;
                }
            }
        }
        sum * self.norm
    }
    /// What the grain adds at full-resolution position `x`, `y` for an output of
    /// `scale` pixels per full-resolution pixel over encoded luminance `l`.
    pub(crate) fn delta(&self, x: f32, y: f32, scale: f32, l: f32) -> f32 {
        if self.amount == 0. {
            return 0.;
        }
        let strength = tone_strength(l);
        if strength == 0. {
            return 0.;
        }
        let (n, attenuation) = if scale >= 1. {
            (1, 1.)
        } else {
            // Down to half size, 2×2 samples: exactly the full-resolution pixels under
            // the preview pixel. Smaller, one sample: further apart than a grain, it
            // averages fewer grains than the pixel holds, so what is left is scaled
            // down by the grain's correlation width.
            let n = if scale >= 0.5 { 2 } else { 1 };
            let width = WIDTH_PER_RADIUS * self.radius + WIDTH_BASE;
            (n, (n as f32 * scale * width).min(1.))
        };
        let mut sum = 0.;
        for j in 0..n {
            for i in 0..n {
                let ox = ((i as f32 + 0.5) / n as f32 - 0.5) / scale.min(1.);
                let oy = ((j as f32 + 0.5) / n as f32 - 0.5) / scale.min(1.);
                sum += self.noise(x + ox, y + oy);
            }
        }
        STRENGTH * self.amount * strength * sum / (n * n) as f32 * attenuation
    }
}
/// The grain's correlation width, in pixels, per mean radius and at none, for previews
/// below half size (fitted to the resized full render).
const WIDTH_PER_RADIUS: f32 = 2.;
const WIDTH_BASE: f32 = 0.8;

#[cfg(test)]
mod tests;
