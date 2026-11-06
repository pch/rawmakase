//! Fields over the image the geometry stage evaluates: vignetting and the lens warp.
use super::*;
use crate::develop::recipe::RenderedRecipe;

/// Vignetting over the camera image: the lens profile's at its Vignetting amount, and
/// measured manual Vignetting. Radius 1 is the half diagonal, from the lens
/// correction's optical centre (the image centre without one), which `LensMap` also
/// uses; `x`, `y` use sample coordinates, where pixel `i` is centred at `i`.
pub(crate) struct VignetteField<'a> {
    table: VignetteTable<'a>,
    pub(super) center: [f32; 2],
    pub(super) half: f32,
    /// The power the table is raised to: Lightroom's profile Vignetting amount (1 =
    /// 100%), or 1 for a combined table, which holds it already.
    pub(super) amount: f32,
}
enum VignetteTable<'a> {
    Lens(std::borrow::Cow<'a, crate::optics::Radial>),
    Combined(crate::optics::Radial),
}
/// A part of a lens correction, borrowed as long as the correction is the photo's
/// own, else (a Lensfun lens's, made for the edit) copied.
fn part<'a, T: Clone>(
    correction: &std::borrow::Cow<'a, crate::optics::LensCorrection>,
    get: impl Fn(&crate::optics::LensCorrection) -> Option<&T>,
) -> Option<std::borrow::Cow<'a, T>> {
    use std::borrow::Cow;
    match correction {
        Cow::Borrowed(c) => get(c).map(Cow::Borrowed),
        Cow::Owned(c) => get(c).cloned().map(Cow::Owned),
    }
}
impl<'a> VignetteField<'a> {
    pub(crate) fn new(im: &'a CameraImage, r: &Recipe) -> Option<Self> {
        let correction = r.lens_correction(&im.metadata);
        let lens = correction
            .as_ref()
            .and_then(|c| part(c, |l| l.vignetting.as_ref()));
        let (w, h) = (im.width as f32, im.height as f32);
        let half = (w * w + h * h).sqrt() * 0.5;
        let [cx, cy] = crate::develop::image_space::LensMap::centre(im, r);
        let (table, amount) = match (lens, r.manual_vignette()) {
            (lens, Some(manual)) => {
                // Manual Vignetting spans the photo frame, inside the camera's default crop.
                let inset = crate::model::image_frame::ImageFrame::new(im).inset;
                let frame = (w * inset[2]).hypot(h * inset[3]) * 0.5;
                (
                    VignetteTable::Combined(crate::develop::effects::combined_table(
                        lens.as_deref(),
                        r.lens_amounts(&im.metadata)[1],
                        &manual,
                        half / frame,
                        crate::optics::reach([cx, cy], [w, h]),
                    )),
                    1.,
                )
            }
            (Some(lens), None) => (VignetteTable::Lens(lens), r.lens_amounts(&im.metadata)[1]),
            (None, None) => return None,
        };
        Some(Self {
            table,
            center: [cx * w, cy * h],
            half,
            amount,
        })
    }
    pub(super) fn table(&self) -> &crate::optics::Radial {
        match &self.table {
            VignetteTable::Lens(t) => t,
            VignetteTable::Combined(t) => t,
        }
    }
    pub(crate) fn gain(&self, x: f32, y: f32) -> f32 {
        let dx = x + 0.5 - self.center[0];
        let dy = y + 0.5 - self.center[1];
        self.table()
            .eval((dx * dx + dy * dy).sqrt() / self.half)
            .powf(self.amount)
    }
}
/// Built-in lens correction applied while sampling the camera image, so no corrected
/// intermediate is stored: vignetting gain in linear camera space, then distortion and
/// lateral chromatic aberration as per-channel radial remapping.
pub(super) struct LensWarp<'a> {
    pub(super) map: crate::develop::image_space::LensMap<'a>,
    pub(super) vignetting: Option<VignetteField<'a>>,
    pub(super) gain_map: Option<GainField<'a>>,
}
/// A DNG's OpcodeList3 GainMap over the camera image, at the lens profile's Vignetting
/// amount like the radial vignetting table.
pub(crate) struct GainField<'a> {
    pub(super) grid: std::borrow::Cow<'a, crate::optics::GainGrid>,
    pub(super) size: [f32; 2],
    pub(super) amount: f32,
}
impl<'a> GainField<'a> {
    pub(super) fn new(im: &'a CameraImage, r: &Recipe) -> Option<Self> {
        Some(Self {
            grid: part(&r.lens_correction(&im.metadata)?, |l| l.gain_map.as_ref())?,
            size: [im.width as f32, im.height as f32],
            amount: r.lens_amounts(&im.metadata)[1],
        })
    }
    /// The gain of each channel at sample position `x`, `y` (pixel `i` centred at `i`),
    /// as a fraction of the image's width and height to the pixel's centre, as the DNG
    /// SDK reads the map.
    pub(crate) fn gain(&self, x: f32, y: f32) -> [f32; 3] {
        self.grid
            .gain((y + 0.5) / self.size[1], (x + 0.5) / self.size[0])
            .map(|g| g.max(0.).powf(self.amount))
    }
}
impl<'a> LensWarp<'a> {
    pub(super) fn new(im: &'a CameraImage, r: &Recipe) -> Option<Self> {
        Some(Self {
            map: crate::develop::image_space::LensMap::new(im, r)?,
            vignetting: VignetteField::new(im, r),
            gain_map: GainField::new(im, r),
        })
    }
    pub(super) fn sample(&self, im: Source, x: f32, y: f32, spread: f32) -> [f32; 3] {
        let m = &self.map;
        let ([dx, dy], scale) = m.scales(x, y);
        let at = |c: usize| {
            [
                m.center[0] + dx * scale[c] - 0.5,
                m.center[1] + dy * scale[c] - 0.5,
            ]
        };
        let [gx, gy] = at(1);
        let p = if scale[0] == scale[1] && scale[2] == scale[1] {
            footprint_sample(im, gx, gy, spread)
        } else {
            let samples = std::array::from_fn(|c| {
                let [sx, sy] = at(c);
                footprint_sample(im, sx, sy, spread)
            });
            if im.metadata.xtrans {
                stripe_ca_sample(im, gx, gy, samples)
            } else {
                [samples[0][0], samples[1][1], samples[2][2]]
            }
        };
        let gain = self.vignetting.as_ref().map_or(1., |v| v.gain(gx, gy));
        // A GainMap the opcode list applies after its warp reads the corrected
        // position: the output's, at the fill's scale. Before the warp, each channel
        // reads it where that channel samples the photo.
        let channel = self.gain_map.as_ref().map_or([1.; 3], |g| {
            if g.grid.after_warp {
                g.gain(m.center[0] + dx - 0.5, m.center[1] + dy - 0.5)
            } else {
                std::array::from_fn(|c| {
                    let [sx, sy] = at(c);
                    g.gain(sx, sy)[c]
                })
            }
        });
        std::array::from_fn(|c| p[c] * gain * channel[c])
    }
}

/// CA remapping must not turn reconstructed, constant-colour fine detail back
/// into coloured stripes. A low source chromaticity variation relative to the
/// colour introduced by remapping identifies that case. Require repeated fine
/// luminance extrema as well, so isolated edges retain their CA correction.
/// `gpu/local.wgsl` mirrors this calculation.
fn stripe_ca_sample(im: Source, x: f32, y: f32, samples: [[f32; 3]; 3]) -> [f32; 3] {
    let p = [samples[0][0], samples[1][1], samples[2][2]];
    let luma = |p: [f32; 3]| (p[0] + 2. * p[1] + p[2]) * 0.25;
    let chroma = |p: [f32; 3]| {
        let l = luma(p).max(1e-8);
        [(p[0] - p[1]) / l, (p[2] - p[1]) / l]
    };
    let base = chroma(samples[1]);
    let distance = |p: [f32; 3]| {
        let c = chroma(p);
        (c[0] - base[0]).powi(2) + (c[1] - base[1]).powi(2)
    };
    let introduced = distance(p);
    let variation = distance(samples[0]).max(distance(samples[2]));
    if introduced < 1e-8 || variation >= 0.16 * introduced {
        return p;
    }
    let periodic = |x: f32, y: f32, dx: i32, dy: i32| {
        let mut low = [f32::INFINITY; 2];
        let mut high = [f32::NEG_INFINITY; 2];
        let values: [f32; 7] = std::array::from_fn(|i| {
            let k = i as i32 - 3;
            let xx = (x.round_ties_even() as i32 + k * dx).clamp(0, im.width as i32 - 1);
            let yy = (y.round_ties_even() as i32 + k * dy).clamp(0, im.height as i32 - 1);
            let p = im.px(yy as usize * im.width as usize + xx as usize);
            let colour = chroma(p);
            for c in 0..2 {
                low[c] = low[c].min(colour[c]);
                high[c] = high[c].max(colour[c]);
            }
            luma(p).max(0.).sqrt()
        });
        let lo = values.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let mut previous = 0f32;
        let mut turns = 0;
        for pair in values.windows(2) {
            let d = pair[1] - pair[0];
            if d.abs() > (hi - lo) * 0.05 {
                turns += usize::from(d * previous < 0.);
                previous = d;
            }
        }
        // A pair of close interpolation positions can have similar colour by
        // chance in noise. Require coherence over the whole seven-pixel line:
        // full below a 2% chromaticity span, none above 5%.
        let span = (high[0] - low[0]).powi(2) + (high[1] - low[1]).powi(2);
        let colour = ((0.0025 - span) / 0.0021).clamp(0., 1.);
        if colour == 0. {
            return (0., false);
        }
        // Two reversals cover a four-pixel period at every phase; the
        // five-line coherence check below still rejects random extrema.
        if turns < 2 {
            return (0., colour == 1.);
        }
        // Random luminance noise also has many extrema. Slats stay in phase on
        // neighbouring lines; noise does not. Compare the variance of their
        // average with their individual variances, removing each line's DC.
        let variance = |line: &[f32; 7]| {
            let mean = line.iter().sum::<f32>() / 7.;
            line.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / 7.
        };
        let line = |across: i32| {
            std::array::from_fn(|i| {
                let k = i as i32 - 3;
                let xx = (x.round_ties_even() as i32 + k * dx + across * dy)
                    .clamp(0, im.width as i32 - 1);
                let yy = (y.round_ties_even() as i32 + k * dy + across * dx)
                    .clamp(0, im.height as i32 - 1);
                luma(im.px(yy as usize * im.width as usize + xx as usize))
                    .max(0.)
                    .sqrt()
            })
        };
        let centre_variance = variance(&values);
        let agreement = |lines: [[f32; 7]; 4], variances: [f32; 4]| {
            let mut average = values;
            let mut total = centre_variance;
            for (line, v) in lines.iter().zip(variances) {
                total += v;
                for i in 0..7 {
                    average[i] += line[i];
                }
            }
            variance(&average.map(|v| v / 5.)) / (total / 5.).max(1e-8)
        };
        // Both one-sided windows reuse two symmetric lines. Keep their samples
        // and variances, preserving the accumulation order while avoiding four
        // duplicate seven-pixel reads and repeated variance calculations.
        let neighbours = [-6, -3, 3, 6].map(line);
        let variances = neighbours.each_ref().map(variance);
        let mut coherence = agreement(neighbours, variances);
        // At a finite strip's boundary, half of a symmetric neighbourhood can
        // be flat background. Require the same five-line agreement on either
        // side rather than weakening the noise or source-colour tests.
        for side in [1, -1] {
            if coherence >= 0.7 {
                break;
            }
            // Clamped copies of one border line are not independent evidence.
            let end_x = x.round_ties_even() as i32 + 12 * side * dy;
            let end_y = y.round_ties_even() as i32 + 12 * side * dx;
            if !(0..im.width as i32).contains(&end_x) || !(0..im.height as i32).contains(&end_y) {
                continue;
            }
            let near = if side == 1 { [2, 3] } else { [1, 0] };
            let far = [line(9 * side), line(12 * side)];
            coherence = coherence.max(agreement(
                [neighbours[near[0]], neighbours[near[1]], far[0], far[1]],
                [
                    variances[near[0]],
                    variances[near[1]],
                    variance(&far[0]),
                    variance(&far[1]),
                ],
            ));
        }
        (colour * ((coherence - 0.55) / 0.15).clamp(0., 1.), false)
    };
    let mut coherent = 0f32;
    for (dx, dy) in [(1, 0), (0, 1)] {
        let (mut evidence, flat_colour_edge) = periodic(x, y, dx, dy);
        if flat_colour_edge && variation < 1e-6 {
            // The smooth-colour border beside reconstructed slats can itself
            // have too few extrema. Accept immediate parallel support only
            // when its source chromaticity agrees within 0.2%. Source-sample
            // variation must be below 0.1% too: proximity to slats alone must
            // not protect real CA on a neighbouring ordinary edge.
            for side in [-3, 3] {
                let (nx, ny) = (x + (side * dy) as f32, y + (side * dx) as f32);
                let (xx, yy) = (nx.round_ties_even() as i32, ny.round_ties_even() as i32);
                if !(0..im.width as i32).contains(&xx) || !(0..im.height as i32).contains(&yy) {
                    continue;
                }
                let matching = (1.
                    - distance(im.px(yy as usize * im.width as usize + xx as usize)) / 0.000004)
                    .clamp(0., 1.);
                if matching > 0. {
                    evidence = evidence.max(matching * periodic(nx, ny, dx, dy).0);
                    if evidence == 1. {
                        break;
                    }
                }
            }
        }
        coherent = coherent.max(evidence);
        // Each axis contributes at most one; no later evidence can improve it.
        if coherent == 1. {
            break;
        }
    }
    let keep = coherent * (1. - ((variation / introduced).sqrt() - 0.1) / 0.3).clamp(0., 1.);
    let gain = luma(p) / luma(samples[1]).max(1e-8);
    std::array::from_fn(|c| p[c] + keep * (samples[1][c] * gain - p[c]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vignetting, a GainMap after the warp and the warp itself all centre on the
    /// correction's optical centre, the same one `LensMap` uses.
    #[test]
    fn vignetting_is_centred_on_the_optical_centre() {
        use crate::camera_data::{CameraImage, Metadata};
        let lens = crate::optics::LensCorrection {
            default_on: true,
            vignetting: Some(crate::optics::Radial {
                knots: vec![0., 1.2],
                values: vec![1., 2.],
            }),
            centre: [0.3, 0.6],
            ..Default::default()
        };
        let im = CameraImage {
            width: 100,
            height: 50,
            pixels: vec![[0.2; 3]; 5000],
            metadata: Metadata {
                width: 100,
                height: 50,
                lens: Some(lens),
                ..Default::default()
            },
            recovered: Default::default(),
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
        };
        let r = Recipe {
            lens_builtin: true,
            ..Default::default()
        };
        let v = VignetteField::new(&im, &r).unwrap();
        let map = crate::develop::image_space::LensMap::new(&im, &r).unwrap();
        assert!((v.center[0] - 30.).abs() < 1e-4 && (v.center[1] - 30.).abs() < 1e-4);
        assert_eq!(v.center, map.center);
        assert!((v.gain(29.5, 29.5) - 1.).abs() < 1e-5);
        assert!((v.gain(29.5 - 20., 29.5) - v.gain(29.5 + 20., 29.5)).abs() < 1e-5);
        assert!(v.gain(99., 0.) > v.gain(0., 49.));
        // With the correction off, manual Vignetting alone keeps the centre the map
        // uses for a measured aberration: the camera's.
        let mut manual = Recipe {
            lens_builtin: false,
            ..Default::default()
        };
        manual.effects.lens_vignette = 0.4;
        assert!(manual.lens_correction(&im.metadata).is_none());
        let v = VignetteField::new(&im, &manual).unwrap();
        let map = crate::develop::image_space::LensMap::new(&im, &manual).unwrap();
        assert_eq!(v.center, map.center);
        assert!((v.center[0] - 30.).abs() < 1e-4);
    }

    #[test]
    fn stripe_window_centres_match_gpu_at_halfway_positions() {
        use crate::camera_data::{CameraImage, Metadata};
        let im = CameraImage {
            width: 64,
            height: 64,
            pixels: (0..64 * 64)
                .map(|i| {
                    let x = i % 64;
                    let l = 0.1 + 0.06 * (std::f32::consts::TAU * x as f32 / 3.8).sin();
                    if x == 34 {
                        [l * 1.5, l, l * 0.5]
                    } else {
                        [l; 3]
                    }
                })
                .collect(),
            metadata: Metadata::default(),
            recovered: Default::default(),
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
        };
        let samples = [[0.15; 3], [0.1; 3], [0.12; 3]];
        let at = |x| stripe_ca_sample((&im).into(), x, 30., samples);
        // GPU round(30.5) is 30; a 31-centred window includes the coloured
        // column at 34 and correctly rejects protection instead.
        assert_ne!(at(30.), at(31.));
        assert_eq!(at(30.5), at(30.));
        assert_eq!(at(29.5), at(30.));
    }
}
