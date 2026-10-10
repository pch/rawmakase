//! Lightroom's Treatment (Color or Black & White) and the Auto black & white mix.
use crate::camera_data::{CameraImage, Metadata};
use crate::camera_profiles::CameraProfile;
use crate::model::recipe::Recipe;
use crate::model::recipe::{Treatment, is_monochrome};
use std::sync::Arc;

/// How the photo's colors spread, as Auto black & white measures them: the mean and
/// covariance of its decoded camera values (white balanced as shot), without
/// pixels near clipping. Measured once per photo; a white balance or profile is
/// applied to the statistics, since both are linear.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorSpread {
    pub mean: [f64; 3],
    pub covariance: [[f64; 3]; 3],
}

impl ColorSpread {
    /// Measures every `step`-th pixel in each direction of a photo up to about 1024
    /// pixels on its long edge.
    pub fn measure(im: &CameraImage) -> Self {
        let step = (im.width.max(im.height) / 1024).max(1) as usize;
        let (mut n, mut sum, mut products) = (0f64, [0f64; 3], [[0f64; 3]; 3]);
        for y in (0..im.height as usize).step_by(step) {
            let row = &im.pixels[y * im.width as usize..][..im.width as usize];
            for p in row.iter().step_by(step) {
                let peak = (0..3)
                    .map(|c| p[c] / im.metadata.wb[c].max(0.001))
                    .fold(0f32, f32::max);
                if peak > CLIPPED || !p.iter().all(|v| v.is_finite()) {
                    continue;
                }
                let p = p.map(f64::from);
                n += 1.;
                for i in 0..3 {
                    sum[i] += p[i];
                    for j in 0..3 {
                        products[i][j] += p[i] * p[j];
                    }
                }
            }
        }
        let n = n.max(1.);
        let mean = sum.map(|s| s / n);
        let covariance = std::array::from_fn(|i| {
            std::array::from_fn(|j| products[i][j] / n - mean[i] * mean[j])
        });
        Self { mean, covariance }
    }

    /// The spread in linear ProPhoto RGB, as `r`'s white balance and profile render it.
    pub fn in_prophoto(&self, m: &Metadata, r: &Recipe) -> Self {
        let matrix = super::profile_matrix(m, r);
        let a: [[f64; 3]; 3] =
            std::array::from_fn(|i| std::array::from_fn(|j| (matrix[i][j] * r.wb[j]) as f64));
        let mean = std::array::from_fn(|i| (0..3).map(|k| a[i][k] * self.mean[k]).sum());
        let covariance = std::array::from_fn(|i| {
            std::array::from_fn(|j| {
                (0..3)
                    .flat_map(|k| (0..3).map(move |l| (k, l)))
                    .map(|(k, l)| a[i][k] * self.covariance[k][l] * a[j][l])
                    .sum()
            })
        });
        Self { mean, covariance }
    }
}

/// Camera values above this fraction of the white level count as clipped.
const CLIPPED: f32 = 0.94;

/// The black & white mix's tables, in the color mixer's grid (hue × saturation ×
/// value of linear ProPhoto RGB): log2 of the gray's luminance over the color's at a
/// zero mix, then each band's change at `GRAY_MIX_POSITIONS`. 1/4000 per step.
static GRAY_CHART: &[u8] = include_bytes!("black_white_chart.bin");
const GRAY_MIX_POSITIONS: [f32; 4] = [-1., -0.5, 0.5, 1.];
const GRAY_SCALE: f32 = 1. / 4000.;

fn gray_value(table: usize, cell: usize) -> f32 {
    let i = 2 * (table * super::color_mixer::CELLS + cell);
    i16::from_le_bytes([GRAY_CHART[i], GRAY_CHART[i + 1]]) as f32 * GRAY_SCALE
}

/// The mix's grid for the color mixer's lookup: no hue or saturation change, and a
/// value factor that scales the color to the gray's luminance (`gray`).
pub(crate) fn gray_grid(mix: [f32; 8]) -> Vec<[f32; 3]> {
    let mut weights: Vec<(usize, f32)> = vec![(0, 1.)];
    for (band, &v) in mix.iter().enumerate() {
        let v = v.clamp(-1., 1.);
        if v == 0. {
            continue;
        }
        // Linear between the measured positions, and toward no change at 0.
        let (near, far) = if v < 0. { (1, 0) } else { (2, 3) };
        let table = |position: usize| 1 + band * 4 + position;
        let half = GRAY_MIX_POSITIONS[near].abs();
        if v.abs() <= half {
            weights.push((table(near), v.abs() / half));
        } else {
            let t = (v.abs() - half) / (1. - half);
            weights.push((table(near), 1. - t));
            weights.push((table(far), t));
        }
    }
    (0..super::color_mixer::CELLS)
        .map(|cell| {
            let dv = weights.iter().map(|(t, w)| w * gray_value(*t, cell)).sum();
            [0., 0., dv]
        })
        .collect()
}

/// The gray a color (linear display RGB) becomes under `grid`, as linear luminance.
pub(crate) fn gray(grid: &[[f32; 3]], rgb: [f32; 3]) -> f32 {
    let scaled = super::color_mixer::tables(grid, rgb.map(|v| v.max(0.)));
    (crate::color::luminance(scaled)).max(0.)
}

/// A photo's Auto black & white mix, for the settings it is converted with. Camera
/// Raw's Auto, measured on synthetic scenes, behaves as a fixed linear function of
/// the direction in which the photo's colors spread most: the principal axis of
/// their covariance in linear ProPhoto RGB, white balanced and profiled as rendered.
/// A photo that varies mostly in brightness gets the neutral mix (for a gray ramp
/// Camera Raw gives Red -9, Orange -19, Yellow -22, Green -27, Aqua -19, Blue +9,
/// Purple +15, Magenta +4); one spread along a color axis moves that axis's bands.
#[derive(Clone, Copy, Debug)]
pub struct AutoMix<'a> {
    pub spread: &'a ColorSpread,
    pub metadata: &'a Metadata,
}

impl AutoMix<'_> {
    /// The mix in slider units (-1..=1), in whole slider steps as Lightroom stores it.
    pub fn for_recipe(&self, r: &Recipe) -> [f32; 8] {
        let axis = self.spread.in_prophoto(self.metadata, r).principal_axis();
        std::array::from_fn(|band| {
            let value = AUTO_MIX_OFFSET[band]
                + (0..3)
                    .map(|c| axis[c] * AUTO_MIX_AXIS[c][band])
                    .sum::<f64>();
            (value.round().clamp(-100., 100.) / 100.) as f32
        })
    }
}

/// The Auto mix's fit, in slider steps per unit of the principal axis's red, green
/// and blue, and its offset. Least squares over Camera Raw 18.7's Auto on 122
/// synthetic scenes (docs/color-mixer.md).
const AUTO_MIX_AXIS: [[f64; 8]; 3] = [
    [26.96, 17.88, 13.31, 3.65, -14.04, -8.61, 8.10, 26.27],
    [-20.48, -5.86, 4.09, 10.99, 10.16, -11.82, -29.83, -31.56],
    [-24.99, -27.05, -24.71, -12.52, 15.47, 43.57, 27.02, 2.93],
];
const AUTO_MIX_OFFSET: [f64; 8] = [3.48, -8.34, -17.09, -27.00, -24.55, -3.27, 13.51, 6.84];

impl ColorSpread {
    /// The unit direction of greatest variance, signed towards brighter. A photo
    /// without variance (one flat color) has the neutral axis.
    fn principal_axis(&self) -> [f64; 3] {
        let c = &self.covariance;
        let neutral = [1. / 3f64.sqrt(); 3];
        let trace = c[0][0] + c[1][1] + c[2][2];
        if trace.is_nan() || trace <= 1e-12 {
            return neutral;
        }
        // Cyclic Jacobi rotations diagonalise the symmetric 3×3 covariance; the
        // eigenvector of the largest eigenvalue is the axis.
        let mut a = *c;
        let mut vectors = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
        for _ in 0..50 {
            let off = a[0][1].powi(2) + a[0][2].powi(2) + a[1][2].powi(2);
            if off.is_nan() || off <= 1e-30 * trace * trace {
                break;
            }
            for (p, q) in [(0, 1), (0, 2), (1, 2)] {
                if a[p][q] == 0. {
                    continue;
                }
                let theta = (a[q][q] - a[p][p]) / (2. * a[p][q]);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.).sqrt());
                let t = if theta == 0. { 1. } else { t };
                let (cos, sin) = (1. / (t * t + 1.).sqrt(), t / (t * t + 1.).sqrt());
                let rotated = |m: &[[f64; 3]; 3], i: usize, j: usize| -> f64 {
                    // (Jᵀ m J)[i][j] for the rotation J in the (p, q) plane.
                    let column = |k: usize, j: usize| {
                        if j == p {
                            cos * m[k][p] - sin * m[k][q]
                        } else if j == q {
                            sin * m[k][p] + cos * m[k][q]
                        } else {
                            m[k][j]
                        }
                    };
                    if i == p {
                        cos * column(p, j) - sin * column(q, j)
                    } else if i == q {
                        sin * column(p, j) + cos * column(q, j)
                    } else {
                        column(i, j)
                    }
                };
                a = std::array::from_fn(|i| std::array::from_fn(|j| rotated(&a, i, j)));
                vectors = std::array::from_fn(|k| {
                    std::array::from_fn(|j| {
                        if j == p {
                            cos * vectors[k][p] - sin * vectors[k][q]
                        } else if j == q {
                            sin * vectors[k][p] + cos * vectors[k][q]
                        } else {
                            vectors[k][j]
                        }
                    })
                });
            }
        }
        let largest = (0..3).fold(0, |best, i| if a[i][i] > a[best][best] { i } else { best });
        let mut v: [f64; 3] = std::array::from_fn(|k| vectors[k][largest]);
        if v.iter().any(|x| x.is_nan()) {
            return neutral;
        }
        if v.iter().sum::<f64>() < 0. {
            v = v.map(|x| -x);
        }
        v
    }
}

/// Setting a recipe's Treatment, with the Auto mix it measures.
pub trait TreatmentChoice {
    /// Sets the Treatment. Converting to black & white sets the Auto mix when the mix
    /// was never set and `first` gives one (Lightroom's "Apply auto mix when first
    /// converting to black and white"); a mix already set (by hand, by Auto, or kept
    /// from an earlier conversion) stays, as do the color mixer's settings.
    fn set_treatment(&mut self, treatment: Treatment, first: Option<AutoMix>);
    /// Lightroom's Treatment switcher: as [`Recipe::set_treatment`], and choosing Color
    /// while a black & white profile is in use changes to `color_profile`, the photo's
    /// default, since the profile alone would keep it black & white.
    fn choose_treatment(
        &mut self,
        treatment: Treatment,
        first: Option<AutoMix>,
        color_profile: Option<Arc<CameraProfile>>,
        m: &Metadata,
    );
    /// Keeps the Treatment with the profile, as Lightroom does: choosing a black &
    /// white profile converts to black & white, and leaving one for a color profile
    /// converts back to color.
    fn follow_profile_treatment(&mut self, old: Option<&CameraProfile>, first: Option<AutoMix>);
}
impl TreatmentChoice for Recipe {
    fn set_treatment(&mut self, treatment: Treatment, first: Option<AutoMix>) {
        let black_white = treatment == Treatment::BlackWhite;
        if black_white
            && !self.effects.monochrome
            && self.effects.gray_mix == [0.; 8]
            && let Some(auto) = first
        {
            self.effects.gray_mix = auto.for_recipe(self);
        }
        self.effects.monochrome = black_white;
    }
    fn choose_treatment(
        &mut self,
        treatment: Treatment,
        first: Option<AutoMix>,
        color_profile: Option<Arc<CameraProfile>>,
        m: &Metadata,
    ) {
        if treatment == Treatment::Color && is_monochrome(self.profile.as_deref()) {
            self.profile = color_profile.filter(|p| !is_monochrome(Some(p)));
            // A newly chosen profile starts at 100%, as in Lightroom.
            self.profile_amount = 1.;
            self.profile_changed(m);
        }
        self.set_treatment(treatment, first);
    }
    fn follow_profile_treatment(&mut self, old: Option<&CameraProfile>, first: Option<AutoMix>) {
        if is_monochrome(self.profile.as_deref()) {
            // From one black & white profile to another the photo was black & white
            // already: its mix stays as it is.
            if !is_monochrome(old) {
                self.set_treatment(Treatment::BlackWhite, first);
            }
        } else if is_monochrome(old) {
            self.set_treatment(Treatment::Color, first);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn chart_gray_follows_camera_raws_measurements() {
        use super::{gray, gray_grid};
        let lum = |p: [f32; 3]| crate::color::luminance(p);
        let zero = gray_grid([0.; 8]);
        // Neutrals keep their luminance; a zero mix stays near the color's own.
        assert!((gray(&zero, [0.2; 3]) - 0.2).abs() < 0.01);
        let orange = [0.6, 0.3, 0.1];
        assert!((gray(&zero, orange) / lum(orange) - 1.).abs() < 0.25);
        // Camera Raw's −100 on a band takes its saturated colors to near black, its
        // +100 brightens them; positions between are between.
        let band = |v: f32| {
            let mut mix = [0.; 8];
            mix[1] = v;
            gray(&gray_grid(mix), orange)
        };
        let (dark, base, light) = (band(-1.), band(0.), band(1.));
        assert!(
            dark < base * 0.3 && light > base * 1.5,
            "{dark} {base} {light}"
        );
        assert!(band(-0.75) < band(-0.5) && band(-0.5) < base);
        assert!(band(0.25) > base && band(0.25) < band(0.5));
        // No band moves a gray, as in Camera Raw.
        for b in 0..8 {
            for v in [-1., 1.] {
                let mut mix = [0.; 8];
                mix[b] = v;
                let moved = gray(&gray_grid(mix), [0.2; 3]);
                assert!(
                    (moved - gray(&zero, [0.2; 3])).abs() < 1e-5,
                    "{b} {v} {moved}"
                );
            }
        }
    }
    use super::*;

    fn metadata() -> Metadata {
        Metadata {
            model: "Synthetic".into(),
            cam_xyz: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            wb: [1.; 3],
            daylight_wb: [1.; 3],
            ..Default::default()
        }
    }

    /// Colors spread along `axis` around a mid gray.
    fn spread(axis: [f64; 3]) -> ColorSpread {
        ColorSpread {
            mean: [0.2; 3],
            covariance: std::array::from_fn(|i| std::array::from_fn(|j| 0.01 * axis[i] * axis[j])),
        }
    }

    fn monochrome_profile(m: &Metadata) -> Arc<CameraProfile> {
        let mut profile = CameraProfile::camera_matrix_default(m)
            .unwrap()
            .with_test_tables();
        profile.name = "Monochrome Look".into();
        profile.enhanced.as_mut().unwrap().monochrome = true;
        Arc::new(profile)
    }

    #[test]
    fn auto_mix_follows_the_axis_the_colors_spread_along() {
        let m = metadata();
        let r = Recipe::default();
        let percent = |mix: [f32; 8]| mix.map(|v| (v * 100.).round() as i32);
        // Brightness alone: Camera Raw's mix for a gray ramp, within 2.
        let neutral = spread([1.; 3]);
        let mix = percent(
            AutoMix {
                spread: &neutral,
                metadata: &m,
            }
            .for_recipe(&r),
        );
        let camera_raw = [-9, -19, -22, -27, -19, 9, 15, 4];
        assert!(
            mix.iter().zip(camera_raw).all(|(a, b)| (a - b).abs() <= 2),
            "{mix:?}"
        );
        // A flat photo has no axis and gets the same.
        let flat = ColorSpread {
            mean: [0.2; 3],
            covariance: [[0.; 3]; 3],
        };
        assert_eq!(
            percent(
                AutoMix {
                    spread: &flat,
                    metadata: &m
                }
                .for_recipe(&r)
            ),
            mix
        );
        // Colors that vary more in blue: Blue and Purple brighter than for gray.
        let blue = spread([0.8, 0.8, 1.6]);
        let blue_mix = percent(
            AutoMix {
                spread: &blue,
                metadata: &m,
            }
            .for_recipe(&r),
        );
        assert!(
            blue_mix[5] > mix[5] + 5 && blue_mix[6] > mix[6] + 5,
            "{blue_mix:?}"
        );
        // A dominant axis away from the largest channel's variance is found too.
        let correlated = ColorSpread {
            mean: [0.2; 3],
            covariance: [[1., 1., 0.], [1., 1., 0.], [0., 0., 1.5]],
        };
        let axis = correlated.principal_axis();
        assert!(
            (axis[0] - 0.5f64.sqrt()).abs() < 1e-9 && axis[2].abs() < 1e-9,
            "{axis:?}"
        );
        // Two equally bright colors: an axis with no brightness component is found too.
        let opposed = spread([1., -1., 0.]);
        let opposed_axis = opposed.principal_axis();
        assert!(
            (opposed_axis[0].abs() - 0.5f64.sqrt()).abs() < 1e-6,
            "{opposed_axis:?}"
        );
        // White balance is part of what Auto measures: cooling the same photo does the same.
        let cooler = Recipe {
            wb: [0.8, 1., 1.6],
            ..r
        };
        let cooled = percent(
            AutoMix {
                spread: &neutral,
                metadata: &m,
            }
            .for_recipe(&cooler),
        );
        assert!(cooled[5] > mix[5] + 5, "{cooled:?}");
    }

    #[test]
    fn first_conversion_applies_auto_and_keeps_a_mix_already_set() {
        let m = metadata();
        let photo = spread([0.8, 0.8, 1.6]);
        let auto = AutoMix {
            spread: &photo,
            metadata: &m,
        };
        let mut r = Recipe {
            hsl: [[0.1, 0.2, 0.3]; 8],
            ..Default::default()
        };
        r.set_treatment(Treatment::BlackWhite, Some(auto));
        assert_eq!(r.treatment(), Treatment::BlackWhite);
        assert_ne!(r.effects.gray_mix, [0.; 8]);
        assert_eq!(r.effects.gray_mix, auto.for_recipe(&r));
        // Back to color and again: the color mixer and the mix are both kept.
        r.effects.gray_mix[0] = 0.5;
        r.set_treatment(Treatment::Color, Some(auto));
        assert_eq!(r.treatment(), Treatment::Color);
        assert_eq!(r.hsl, [[0.1, 0.2, 0.3]; 8]);
        r.set_treatment(Treatment::BlackWhite, Some(auto));
        assert_eq!(r.effects.gray_mix[0], 0.5);
        // Without an Auto mix (the preference is off), a mix never set stays at zero.
        let mut r = Recipe::default();
        r.set_treatment(Treatment::BlackWhite, None);
        assert!(r.effects.monochrome);
        assert_eq!(r.effects.gray_mix, [0.; 8]);
    }

    #[test]
    fn black_white_profiles_carry_the_treatment() {
        let m = metadata();
        let photo = spread([1.; 3]);
        let auto = Some(AutoMix {
            spread: &photo,
            metadata: &m,
        });
        let mono = monochrome_profile(&m);
        let mut r = Recipe::with_profiles(&m, &[]);
        let color = r.profile.clone();
        assert!(!is_monochrome(color.as_deref()));
        // Choosing a black & white profile converts, with the Auto mix.
        r.profile = Some(mono.clone());
        r.follow_profile_treatment(color.as_deref(), auto);
        assert!(r.effects.monochrome);
        assert_ne!(r.effects.gray_mix, [0.; 8]);
        // Color with that profile in use goes back to a color profile.
        r.choose_treatment(Treatment::Color, auto, color.clone(), &m);
        assert_eq!(r.treatment(), Treatment::Color);
        assert!(!is_monochrome(r.profile.as_deref()));
        // Leaving a black & white profile for a color one converts back too.
        let mut r = Recipe::with_profiles(&m, &[]);
        r.profile = Some(mono.clone());
        r.follow_profile_treatment(None, auto);
        r.profile = color.clone();
        r.follow_profile_treatment(Some(&mono), auto);
        assert_eq!(r.treatment(), Treatment::Color);
        // From one black & white profile to another, a mix never set stays so.
        let mut r = Recipe::with_profiles(&m, &[]);
        r.profile = Some(mono.clone());
        let other = monochrome_profile(&m);
        r.profile = Some(other);
        r.follow_profile_treatment(Some(&mono), auto);
        assert_eq!(r.treatment(), Treatment::BlackWhite);
        assert_eq!(r.effects.gray_mix, [0.; 8]);
        // A color profile change leaves a black & white treatment alone.
        r.set_treatment(Treatment::BlackWhite, auto);
        r.follow_profile_treatment(color.as_deref(), auto);
        assert_eq!(r.treatment(), Treatment::BlackWhite);
    }
}
