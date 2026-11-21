//! Rendering one correction on linear camera pixels (white-balanced as shot).
//!
//! Inside a soft ellipse every pixel moves towards a dark neutral, whatever its color,
//! as Camera Raw's does: the pupil's brightness is taken from green and blue alone, so
//! the red cast leaves no trace, then Darken scales it. Pupil Size scales the ellipse.
//! The constants are fitted to Camera Raw 18.7 renders of synthetic pupils (see
//! docs/retouching.md).
use crate::model::image_frame::ImageFrame;
use crate::model::red_eye::{EyeKind, RedEyeOp, half, mahalanobis2};
use crate::{camera_data::CameraImage, develop::retouch::profile};

/// A rectangle of decoded pixels, `[x0, y0, x1, y1)`.
type PixelRect = [i32; 4];

/// The catchlight: full up to `CATCHLIGHT_INNER` times the half-way distance from its
/// centre, none beyond `CATCHLIGHT_OUTER` (measured: half at 0.094), and its value.
const CATCHLIGHT_INNER: f32 = 0.06;
const CATCHLIGHT_OUTER: f32 = 0.17;
const CATCHLIGHT: f32 = 0.5;
/// The falloff: full up to `INNER` times the half-way distance, none beyond `OUTER`.
const INNER: f32 = 0.55;
const OUTER: f32 = 1.42;

/// How a corrected pixel's value is found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Model {
    /// The encoding brightness is mixed in (`v^(1/gamma)`).
    pub(crate) gamma: f32,
    /// How much red counts in the pupil's brightness, against green and blue.
    pub(crate) red: f32,
    /// Encoded gain at Darken 0, 0.5 and 1; quadratic between.
    pub(crate) gain: [f32; 3],
    /// How much of the original color stays, as a power of its channel ratios.
    pub(crate) keep: f32,
}
pub(crate) const MODEL: Model = Model {
    gamma: 2.4,
    red: -0.11,
    gain: [1.28, 0.9, 0.39],
    keep: 0.022,
};

/// A correction placed on one decoded image.
#[derive(Clone, Debug)]
pub(crate) struct Placed {
    kind: EyeKind,
    center: [f32; 2],
    /// Semi-axes in decoded pixels, along its x and y.
    radius: [f32; 2],
    correlation: f32,
    /// Half-way distance of the falloff, in units of the ellipse.
    half: f32,
    darken: f32,
    /// The catchlight's centre in decoded pixels.
    catchlight: Option<[f32; 2]>,
}
impl Placed {
    pub(crate) fn new(op: &RedEyeOp, frame: &ImageFrame) -> Self {
        let long = frame.long_edge();
        let [rx, ry] = op.radius.map(|r| (r * long).max(0.5));
        // A quarter turn swaps the axes and mirrors the tilt.
        let (radius, correlation) = if frame.turns % 2 == 1 {
            ([ry, rx], -op.correlation)
        } else {
            ([rx, ry], op.correlation)
        };
        let center = frame.to_source(op.center);
        let half = half(op);
        // The catchlight's offset turns with the image (as positions do), in units of
        // the decoded semi-axes.
        let catchlight = match op.kind {
            EyeKind::Pet {
                catchlight: Some(c),
            } => {
                let o = crate::model::image_frame::turn(frame.turns, 0.5 + c[0], 0.5 + c[1]);
                let o = [o[0] - 0.5, o[1] - 0.5];
                Some([
                    center[0] + o[0] * radius[0] * half,
                    center[1] + o[1] * radius[1] * half,
                ])
            }
            _ => None,
        };
        Self {
            kind: op.kind,
            center,
            radius,
            correlation,
            half,
            darken: op.darken,
            catchlight,
        }
    }
    /// Pixels the correction writes (and reads).
    pub(crate) fn dest(&self) -> PixelRect {
        let reach = OUTER * self.half;
        let [cx, cy] = self.center;
        let (ex, ey) = (reach * self.radius[0] + 1., reach * self.radius[1] + 1.);
        [
            (cx - ex).floor() as i32,
            (cy - ey).floor() as i32,
            (cx + ex).ceil() as i32 + 1,
            (cy + ey).ceil() as i32 + 1,
        ]
    }
    /// How much of the correction applies at decoded pixel (`x`, `y`).
    fn weight(&self, x: f32, y: f32) -> f32 {
        let d = [x - self.center[0], y - self.center[1]];
        let rho = mahalanobis2(self.radius, self.correlation, d).sqrt() / self.half;
        profile(rho, INNER, OUTER)
    }
    /// Renders the correction into `im`.
    pub(crate) fn apply(&self, im: &mut CameraImage) {
        self.apply_with(im, &MODEL);
    }
    pub(crate) fn apply_with(&self, im: &mut CameraImage, model: &Model) {
        let r = self.dest();
        let (w, h) = (im.width as i32, im.height as i32);
        let [x0, y0, x1, y1] = [r[0].max(0), r[1].max(0), r[2].min(w), r[3].min(h)];
        for y in y0..y1 {
            for x in x0..x1 {
                let a = self.weight(x as f32, y as f32);
                // Blended in encoded values, as the falloff was measured.
                let (e, d) = (
                    |v: f32| v.max(0.).powf(1. / model.gamma),
                    |v: f32| v.powf(model.gamma),
                );
                let p = &mut im.pixels[(y * w + x) as usize];
                if a > 0. {
                    let target = match self.kind {
                        EyeKind::Red => red_pupil(*p, self.darken, model),
                        EyeKind::Pet { .. } => [0.; 3],
                    };
                    *p = std::array::from_fn(|c| d(e(p[c]) + (e(target[c]) - e(p[c])) * a));
                }
                if let Some(at) = self.catchlight {
                    let offset = [x as f32 - at[0], y as f32 - at[1]];
                    let rho = mahalanobis2(self.radius, 0., offset).sqrt() / self.half;
                    // It fades with the pupil's correction towards the ellipse's edge.
                    let a = profile(rho, CATCHLIGHT_INNER, CATCHLIGHT_OUTER) * a;
                    if a > 0. {
                        let white = e(CATCHLIGHT);
                        *p = p.map(|v| d(e(v) + (white - e(v)) * a));
                    }
                }
            }
        }
    }
}

/// The quadratic through `g` at 0, 0.5 and 1.
fn gain(g: [f32; 3], t: f32) -> f32 {
    let (a, b, c) = (g[0], g[1], g[2]);
    // Lagrange form on 0, ½, 1.
    (a * 2. * (t - 0.5) * (t - 1.) - b * 4. * t * (t - 1.) + c * 2. * t * (t - 0.5)).max(0.)
}
/// A red pupil's corrected value: green and blue's brightness, darkened, with a trace
/// of the original color.
fn red_pupil(p: [f32; 3], darken: f32, m: &Model) -> [f32; 3] {
    let encode = |v: f32| v.max(0.).powf(1. / m.gamma);
    let (r, g, b) = (encode(p[0]), encode(p[1]), encode(p[2]));
    let gb = (g + b) / 2.;
    let level = (gb + m.red * (r - gb)).max(0.) * gain(m.gain, darken);
    let level = level.powf(m.gamma);
    // A trace of the original color: its log ratios to their mean, scaled by `keep`.
    let logs = p.map(|v| (v.max(1e-5)).ln());
    let mean = (logs[0] + logs[1] + logs[2]) / 3.;
    logs.map(|l| level * (m.keep * (l - mean)).exp())
}
