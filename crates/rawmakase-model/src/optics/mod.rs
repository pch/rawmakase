//! The lens model the renderer evaluates: radial functions of the normalized image
//! radius for vignetting, distortion and lateral chromatic aberration. Readers in
//! [`lens`](crate::lens) and [`dng`](crate::dng) produce it. [`lcp`] holds Adobe
//! lens profiles as data, [`lensfun`] Lensfun calibrations and their evaluation.
//! Only `lcp` depends on anything in the crate: `xml`, to parse the profiles.
pub mod lcp;
pub mod lensfun;

use serde::{Deserialize, Serialize};

/// A radial function sampled at increasing radii. Radius 0 is the correction's optical
/// centre ([`LensCorrection::centre`]) and 1 is half of the decoded image's diagonal,
/// so an off-centre correction's farthest corner lies beyond 1
/// ([`LensCorrection::reach`]). Values are linearly interpolated and held constant
/// beyond the outermost knots.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Radial {
    pub knots: Vec<f32>,
    pub values: Vec<f32>,
}
impl Radial {
    /// The most knots a table may have; `develop`'s GPU port sizes its search by it.
    pub const MAX_KNOTS: usize = 256;
    /// The table of `knots` and `values`, or `None` when it is not a valid one
    /// (see `valid`).
    pub fn new(knots: Vec<f32>, values: Vec<f32>) -> Option<Self> {
        let r = Self { knots, values };
        r.valid().then_some(r)
    }
    pub(crate) fn valid(&self) -> bool {
        self.knots.len() >= 2
            && self.knots.len() == self.values.len()
            && self.knots.len() <= Self::MAX_KNOTS
            && self.knots.iter().chain(&self.values).all(|v| v.is_finite())
            && self.knots.windows(2).all(|k| k[0] < k[1])
            && self.knots[0] >= 0.
            && self.values.iter().all(|v| (0.2..=5.).contains(v))
    }
    pub fn eval(&self, r: f32) -> f32 {
        let k = &self.knots;
        let i = k.partition_point(|x| *x <= r);
        if i == 0 {
            return self.values[0];
        }
        if i == k.len() {
            return self.values[k.len() - 1];
        }
        let t = (r - k[i - 1]) / (k[i] - k[i - 1]);
        self.values[i - 1] + (self.values[i] - self.values[i - 1]) * t
    }
}

/// A gain per colour channel sampled on a grid over part of the image: a DNG GainMap
/// opcode of OpcodeList3, which applies to the demosaiced image (DJI and Hasselblad
/// drones' and the Sigma fp's lens shading). It covers `area` (top, left, bottom, right
/// in fractions of the image's height and width; no gain outside it). Points are
/// `points` = [rows, columns], starting at `origin` and `spacing` apart, in fractions of
/// the image's height and width, as the DNG SDK reads them; each holds `planes` gains, one per channel (red, green,
/// blue) or one for all three. Between points the gain is bilinear, beyond the outermost
/// ones that of the nearest. `after_warp`: the opcode list warps the image before it,
/// so it applies at the corrected position rather than the source's.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GainGrid {
    #[serde(default = "GainGrid::whole")]
    pub area: [f32; 4],
    #[serde(default)]
    pub after_warp: bool,
    pub points: [usize; 2],
    pub origin: [f32; 2],
    pub spacing: [f32; 2],
    pub planes: usize,
    pub gains: Vec<f32>,
}
impl Default for GainGrid {
    fn default() -> Self {
        Self {
            area: Self::whole(),
            after_warp: false,
            points: [1, 1],
            origin: [0.; 2],
            spacing: [1.; 2],
            planes: 1,
            gains: vec![1.],
        }
    }
}
impl GainGrid {
    fn whole() -> [f32; 4] {
        [0., 0., 1., 1.]
    }
    pub(crate) fn valid(&self) -> bool {
        let [rows, columns] = self.points;
        let [top, left, bottom, right] = self.area;
        (0. ..1.).contains(&top)
            && (0. ..1.).contains(&left)
            && top < bottom
            && left < right
            && bottom <= 1.
            && right <= 1.
            && (1..=256).contains(&rows)
            && (1..=256).contains(&columns)
            && matches!(self.planes, 1 | 3)
            && self.gains.len() == rows * columns * self.planes
            && self.origin.iter().all(|v| v.is_finite())
            && self.spacing.iter().all(|v| v.is_finite() && *v > 0.)
            && self.gains.iter().all(|g| (0.2..=8.).contains(g))
    }
    /// The gain of each channel at `v`, `h`: fractions of the image's height and width.
    pub fn gain(&self, v: f32, h: f32) -> [f32; 3] {
        let [top, left, bottom, right] = self.area;
        if !(top..bottom).contains(&v) || !(left..right).contains(&h) {
            return [1.; 3];
        }
        let at = |x: f32, axis: usize| {
            let n = self.points[axis];
            let f = ((x - self.origin[axis]) / self.spacing[axis]).clamp(0., (n - 1) as f32);
            let i = (f as usize).min(n.saturating_sub(2));
            (i, (i + 1).min(n - 1), f - i as f32)
        };
        let (r0, r1, tv) = at(v, 0);
        let (c0, c1, th) = at(h, 1);
        let g =
            |r: usize, c: usize, p: usize| self.gains[(r * self.points[1] + c) * self.planes + p];
        std::array::from_fn(|c| {
            let p = c.min(self.planes - 1);
            let top = g(r0, c0, p) + (g(r0, c1, p) - g(r0, c0, p)) * th;
            let bottom = g(r1, c0, p) + (g(r1, c1, p) - g(r1, c0, p)) * th;
            top + (bottom - top) * tv
        })
    }
}

/// Built-in lens correction for one photo.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LensCorrection {
    /// Where the data came from, for display, e.g. "Fujifilm built-in".
    pub source: String,
    /// Lightroom applies this correction by default. True where verified against
    /// Adobe's own rendering; other corrections are available but start disabled.
    #[serde(default)]
    pub default_on: bool,
    /// Gain that restores illumination at each radius (1 = none).
    pub vignetting: Option<Radial>,
    /// Source radius divided by output radius for the green channel (1 = none).
    pub distortion: Option<Radial>,
    /// Additional source radius scale for red and blue relative to green (1 = none).
    pub chromatic: Option<[Radial; 2]>,
    /// `chromatic` is a function of green's source radius (after distortion), as
    /// Lensfun's lateral CA is, rather than of the output radius. Corrections with it
    /// are made while rendering and never saved.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub chromatic_at_source: bool,
    /// Gain per channel over the image, before distortion, as a DNG's OpcodeList3
    /// GainMap applies it (1 = none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain_map: Option<GainGrid>,
    /// The optical centre the radial functions are measured from, in fractions of the
    /// decoded image's width and height: where the camera's data or Adobe's own output
    /// places it, the decoded image's centre (0.5, 0.5) when they record none. Data
    /// saved before it existed has the image centre.
    #[serde(
        default = "LensCorrection::image_centre",
        skip_serializing_if = "LensCorrection::is_image_centre"
    )]
    pub centre: [f32; 2],
}
impl Default for LensCorrection {
    fn default() -> Self {
        Self {
            source: String::new(),
            default_on: false,
            vignetting: None,
            distortion: None,
            chromatic: None,
            chromatic_at_source: false,
            gain_map: None,
            centre: Self::image_centre(),
        }
    }
}
/// No correction, for applying measured chromatic aberration alone.
pub static NO_CORRECTION: LensCorrection = LensCorrection {
    source: String::new(),
    default_on: false,
    vignetting: None,
    distortion: None,
    chromatic: None,
    chromatic_at_source: false,
    gain_map: None,
    centre: [0.5, 0.5],
};
/// The distance from `centre` (fractions of `size`'s width and height) to the farthest
/// corner of an image of `size` pixels, in units of its half diagonal: 1 for the image
/// centre, more off it.
pub fn reach(centre: [f32; 2], size: [f32; 2]) -> f32 {
    let [w, h] = size;
    let x = centre[0].max(1. - centre[0]) * w;
    let y = centre[1].max(1. - centre[1]) * h;
    if w > 0. && h > 0. {
        x.hypot(y) / (w.hypot(h) * 0.5)
    } else {
        1.
    }
}
impl LensCorrection {
    pub const fn image_centre() -> [f32; 2] {
        [0.5, 0.5]
    }
    fn is_image_centre(c: &[f32; 2]) -> bool {
        *c == Self::image_centre()
    }
    /// The optical centre in pixels of an image of `size`.
    pub fn centre_in(&self, size: [f32; 2]) -> [f32; 2] {
        [self.centre[0] * size[0], self.centre[1] * size[1]]
    }
    /// The radius of the farthest corner of an image of `size` from the optical
    /// centre, in units of its half diagonal ([`reach`]).
    pub fn reach(&self, size: [f32; 2]) -> f32 {
        reach(self.centre, size)
    }
    /// The `source` of the correction read from a Sony file's own tables (a DNG's
    /// opcodes are another source, with Lightroom's framing).
    pub const SONY_BUILTIN: &'static str = "Sony built-in";
    pub fn is_empty(&self) -> bool {
        self.vignetting.is_none()
            && self.distortion.is_none()
            && self.chromatic.is_none()
            && self.gain_map.is_none()
    }
    pub fn validate(&self) -> bool {
        self.centre.iter().all(|c| (0. ..=1.).contains(c))
            && self.vignetting.iter().all(Radial::valid)
            && self.distortion.iter().all(Radial::valid)
            && self.chromatic.iter().flatten().all(Radial::valid)
            && self.gain_map.iter().all(GainGrid::valid)
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn vignetting_gain(&self, r: f32) -> f32 {
        self.vignetting.as_ref().map_or(1., |v| v.eval(r))
    }
    /// As `radial_scale_with` at 100%.
    #[cfg(test)]
    pub fn radial_scale(&self, r: f32) -> [f32; 3] {
        self.radial_scale_with(r, 1.)
    }
    /// Source radius scale for red, green and blue at output radius `r`, with
    /// Lightroom's Distortion amount (1 = 100%).
    pub fn radial_scale_with(&self, r: f32, amount: f32) -> [f32; 3] {
        self.radial_scale_ca(r, amount, None)
    }
    /// As `radial_scale_with`, with `chromatic` measured from the decoded image in place
    /// of the lens data's own. A measurement is a function of the decoded radius, so it
    /// is evaluated where green samples, after distortion, as the lens data's own is
    /// when it says so (`chromatic_at_source`).
    pub fn radial_scale_ca(
        &self,
        r: f32,
        amount: f32,
        chromatic: Option<&[Radial; 2]>,
    ) -> [f32; 3] {
        let g = self
            .distortion
            .as_ref()
            .map_or(1., |d| 1. + (d.eval(r) - 1.) * amount);
        match (chromatic, &self.chromatic) {
            (Some([red, blue]), _) => [g * red.eval(r * g), g, g * blue.eval(r * g)],
            (None, Some([red, blue])) if self.chromatic_at_source => {
                [g * red.eval(r * g), g, g * blue.eval(r * g)]
            }
            (None, Some([red, blue])) => [g * red.eval(r), g, g * blue.eval(r)],
            (None, None) => [g; 3],
        }
    }
    /// The scale output offsets from the optical centre take so that green's sample for
    /// every point of the photo's edge, the rectangle `frame` (`[left, top, right,
    /// bottom]`, in pixels) of the decoded image of `size`, lies inside the decoded
    /// image, with Lightroom's Distortion amount (1 = 100%): as Lightroom frames the
    /// corrected photo, never zooming out. Only green frames it, as in Adobe's warps,
    /// whose red and blue planes may reach a little past the image's edge; samplers clamp
    /// them there. Where the corrected radius grows with the output radius, as it does
    /// for any real lens, the interior samples nearer the centre than the edge along the
    /// same ray, so it stays inside too; a correction that folds back keeps every radius
    /// out to the farthest corner inside instead.
    pub fn fill_scale(&self, amount: f32, size: [f32; 2], frame: [f32; 4]) -> f32 {
        let Some(distortion) = &self.distortion else {
            return 1.;
        };
        let [w, h] = size;
        let half = w.hypot(h) * 0.5;
        if half.is_nan() || half <= 0. {
            return 1.;
        }
        let green = |r: f32| 1. + (distortion.eval(r) - 1.) * amount;
        let reach = self.reach(size);
        // Every radius out to the farthest corner sampling at or inside its own keeps
        // every sample inside the image, which is convex around the centre.
        let radii = (0..=256).map(|i| reach * i as f32 / 256.);
        let whole = 1. / radii.clone().map(green).fold(1f32, f32::max);
        let mut previous = 0f32;
        let monotonic = radii.skip(1).all(|r| {
            let s = r * green(r);
            let rising = s > previous;
            previous = s;
            rising
        });
        if !monotonic {
            return whole;
        }
        let [cx, cy] = self.centre_in(size);
        let [left, top, right, bottom] = frame;
        let (x0, x1) = (left.max(0.) - cx, right.min(w) - cx);
        let (y0, y1) = (top.max(0.) - cy, bottom.min(h) - cy);
        let inside = |dx: f32, dy: f32, fill: f32| {
            let s = fill * green(dx.hypot(dy) * fill / half);
            (0. ..=w).contains(&(cx + dx * s)) && (0. ..=h).contains(&(cy + dy * s))
        };
        // About one sample per pixel along the edge, so no extremum between samples
        // leaves the image by more than a fraction of a pixel.
        let steps = (w.max(h).ceil() as usize).clamp(64, 8192);
        let mut fill = 1f32;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let (x, y) = (x0 + (x1 - x0) * t, y0 + (y1 - y0) * t);
            for (dx, dy) in [(x, y0), (x, y1), (x0, y), (x1, y)] {
                if inside(dx, dy, fill) {
                    continue;
                }
                let (mut lo, mut hi) = (whole, fill);
                for _ in 0..24 {
                    let mid = (lo + hi) * 0.5;
                    if inside(dx, dy, mid) {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                fill = lo;
            }
        }
        fill
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Lateral CA given as a function of green's source radius is read there, so the
    /// Distortion amount doesn't move it; the default reads it at the output radius.
    #[test]
    fn chromatic_at_source_follows_green() {
        let c = LensCorrection {
            distortion: Radial::new(vec![0., 1.], vec![1., 1.2]),
            chromatic: Some([
                Radial::new(vec![0., 2.], vec![1., 1.2]).unwrap(),
                Radial::new(vec![0., 2.], vec![1., 0.8]).unwrap(),
            ]),
            chromatic_at_source: true,
            ..Default::default()
        };
        for amount in [0., 1., 2.] {
            let [r, g, b] = c.radial_scale_with(1., amount);
            let source = 1. * g;
            assert!((r / g - (1. + 0.1 * source)).abs() < 1e-6, "{amount}");
            assert!((b / g - (1. - 0.1 * source)).abs() < 1e-6, "{amount}");
        }
        let output = LensCorrection {
            chromatic_at_source: false,
            ..c
        };
        let [r, g, _] = output.radial_scale_with(1., 1.);
        assert!((r / g - 1.1).abs() < 1e-6);
        // Never saved: corrections that are saved don't have it.
        assert!(
            !serde_json::to_string(&output)
                .unwrap()
                .contains("chromatic_at_source")
        );
    }
    #[test]
    fn radial_interpolates_and_clamps() {
        let r = Radial::new(vec![0., 0.5, 1.], vec![1., 1.2, 1.6]).unwrap();
        assert_eq!(r.eval(-1.), 1.);
        assert!((r.eval(0.25) - 1.1).abs() < 1e-6);
        assert!((r.eval(0.75) - 1.4).abs() < 1e-6);
        assert_eq!(r.eval(2.), 1.6);
        assert!(Radial::new(vec![0., 0.], vec![1., 1.]).is_none());
        assert!(Radial::new(vec![0., 1.], vec![1., f32::NAN]).is_none());
    }
    /// The fill for the whole decoded image of `size` as its frame.
    fn fill(c: &LensCorrection, size: [f32; 2]) -> f32 {
        c.fill_scale(1., size, [0., 0., size[0], size[1]])
    }
    /// How far green's sample for any pixel of the image's edge lies outside it.
    fn worst_outside(c: &LensCorrection, size: [f32; 2], fill: f32) -> f32 {
        let [w, h] = size;
        let [cx, cy] = c.centre_in(size);
        let half = w.hypot(h) * 0.5;
        let mut worst = f32::NEG_INFINITY;
        let edge = (0..w as usize)
            .flat_map(|i| [(i as f32 + 0.5, 0.), (i as f32 + 0.5, h)])
            .chain((0..h as usize).flat_map(|i| [(0., i as f32 + 0.5), (w, i as f32 + 0.5)]));
        for (px, py) in edge {
            let (dx, dy) = (px - cx, py - cy);
            let s = fill * c.radial_scale_with(dx.hypot(dy) * fill / half, 1.)[1];
            let (x, y) = (cx + dx * s, cy + dy * s);
            worst = worst.max(-x).max(x - w).max(-y).max(y - h);
        }
        worst
    }
    #[test]
    fn fill_scale_keeps_corners_inside() {
        let c = LensCorrection {
            distortion: Radial::new(vec![0., 1.], vec![1., 1.02]),
            ..Default::default()
        };
        let size = [300., 200.];
        let s = fill(&c, size);
        assert!(s < 1. && s > 0.97);
        assert!(c.radial_scale(s)[1] * s <= 1.0001);
        assert!(worst_outside(&c, size, s) < 0.01);
        assert_eq!(fill(&LensCorrection::default(), size), 1.);
    }
    /// Only the photo's edge can leave the image. Adobe's warp of a Z5 II photo
    /// (24-70mm f/4 S at 24mm in DX crop, 4000 × 2672) has green's ratio 1.0149 at the
    /// centre and 0.9725 at the corner, and no zoom: the edge samples just inside.
    #[test]
    fn a_ratio_above_one_inside_the_edges_needs_no_zoom() {
        let k = [1.01488, -0.05341, 0.012095, -0.0010548];
        let knots: Vec<f32> = (0..=32).map(|i| i as f32 / 32.).collect();
        let values = knots
            .iter()
            .map(|r| {
                let r2 = r * r;
                k[0] + k[1] * r2 + k[2] * r2 * r2 + k[3] * r2 * r2 * r2
            })
            .collect();
        let c = LensCorrection {
            distortion: Radial::new(knots, values),
            ..Default::default()
        };
        assert_eq!(fill(&c, [4000., 2672.]), 1.);
    }
    /// Adobe frames by green: red reaching just past the corner (a Coolpix P1100's
    /// 1.0009) does not zoom, and neither does lateral CA alone.
    #[test]
    fn chromatic_aberration_does_not_frame() {
        let ca = [
            Radial::new(vec![0., 1.], vec![1., 1.0009]).unwrap(),
            Radial::new(vec![0., 1.], vec![1., 0.9995]).unwrap(),
        ];
        let alone = LensCorrection {
            chromatic: Some(ca.clone()),
            ..Default::default()
        };
        assert_eq!(fill(&alone, [6000., 4000.]), 1.);
        let barrel = LensCorrection {
            distortion: Radial::new(vec![0., 1.], vec![1., 0.97]),
            chromatic: Some(ca),
            ..Default::default()
        };
        assert_eq!(fill(&barrel, [6000., 4000.]), 1.);
    }
    /// An off-centre optical centre: the corner farthest from it sets the zoom, and every
    /// edge sample stays inside without zooming more than that needs.
    #[test]
    fn fill_around_an_off_centre_axis_keeps_the_far_corner_inside() {
        let size = [6000f32, 4000.];
        let distortion = Radial::new(
            (0..=12).map(|i| i as f32 / 10.).collect(),
            (0..=12)
                .map(|i| 1. + 0.03 * (i as f32 / 10.).powi(2))
                .collect(),
        );
        let centred = LensCorrection {
            distortion: distortion.clone(),
            ..Default::default()
        };
        let off = LensCorrection {
            distortion,
            centre: [0.46, 0.53],
            ..Default::default()
        };
        assert!(off.reach(size) > 1.05 && centred.reach(size) == 1.);
        let (a, b) = (fill(&centred, size), fill(&off, size));
        assert!(b < a, "{a} {b}");
        let worst = worst_outside(&off, size, b);
        assert!((-0.5..0.01).contains(&worst), "{worst}");
        // With the centred fill, the far corner would sample outside.
        assert!(worst_outside(&off, size, a) > 5.);
    }
    /// A correction that folds back has no edge to test; every radius out to the
    /// farthest corner keeps inside instead.
    #[test]
    fn a_folding_correction_keeps_every_radius_inside() {
        let c = LensCorrection {
            distortion: Radial::new(vec![0., 0.5, 1.], vec![1.2, 1., 0.6]),
            ..Default::default()
        };
        let s = fill(&c, [300., 200.]);
        assert!((s - 1. / 1.2).abs() < 1e-4, "{s}");
    }
    #[test]
    fn saved_corrections_without_a_centre_have_the_image_centre() {
        let c: LensCorrection = serde_json::from_str(
            r#"{"source":"x","vignetting":null,"distortion":null,"chromatic":null}"#,
        )
        .unwrap();
        assert_eq!(c.centre, [0.5, 0.5]);
        assert!(!serde_json::to_string(&c).unwrap().contains("centre"));
        let off = LensCorrection {
            centre: [0.4, 0.5],
            ..c
        };
        let back: LensCorrection =
            serde_json::from_str(&serde_json::to_string(&off).unwrap()).unwrap();
        assert_eq!(back.centre, [0.4, 0.5]);
        assert!(
            !LensCorrection {
                centre: [1.2, 0.5],
                ..back
            }
            .validate()
        );
    }
    #[test]
    fn fill_within_a_crop_of_a_wider_sensor_is_looser_and_still_inside() {
        let c = LensCorrection {
            distortion: Radial::new(vec![0., 1.], vec![1., 1.08]),
            ..Default::default()
        };
        let size = [6000., 4000.];
        // A 16:9 crop of the 3:2 image.
        let crop = [0., 437.5, 6000., 3812.5];
        let whole = fill(&c, size);
        let within = c.fill_scale(1., size, crop);
        assert!(within > whole && within <= 1., "{whole} {within}");
        let half = 6000f32.hypot(4000.) * 0.5;
        for (x, y) in [
            (3000f32, 1562.5f32),
            (3000., 0.),
            (0., 1562.5),
            (1500., 1562.5),
        ] {
            let s = c.radial_scale_with(x.hypot(y) * within / half, 1.)[1];
            assert!(
                x * within * s <= 3000.01 && y * within * s <= 2000.01,
                "{x} {y}"
            );
        }
        assert_eq!(LensCorrection::default().fill_scale(1., size, crop), 1.);
    }
    #[test]
    fn fill_leaves_no_edge_pixel_outside_between_samples() {
        // A table with bumps, the kind Sony's signed values give.
        let c = LensCorrection {
            distortion: Radial::new(
                (0..11).map(|i| i as f32 / 10.).collect(),
                vec![
                    1.08, 1.06, 1.04, 1.02, 1.02, 1.02, 1.02, 1.02, 1.02, 1.03, 0.99,
                ],
            ),
            ..Default::default()
        };
        let size = [6000f32, 4000f32];
        let worst = worst_outside(&c, size, fill(&c, size));
        assert!(worst < 0.5, "{worst}");
    }
}
