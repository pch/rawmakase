//! Brush components rasterised once into an image-space coverage bitmap, then sampled
//! at every render. The raster covers the strokes' bounds with at least four pixels
//! per brush radius, up to 2048 on the long side, and is cached by the strokes' hash.
use crate::model::masks::BrushStroke;
use crate::{camera_data::CameraImage, develop::retouch::profile};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

const MAX_SIDE: f32 = 2048.;

/// Coverage over an image-space rectangle, in long-edge units (see `Space`).
pub(crate) struct Raster {
    origin: [f32; 2],
    /// Long-edge units per raster pixel.
    step: f32,
    width: usize,
    height: usize,
    data: Vec<f32>,
}
impl Raster {
    pub(crate) fn bytes(&self) -> usize {
        self.data.len() * 4
    }
    /// Bilinear coverage at `p` (long-edge units); zero outside.
    pub(crate) fn sample(&self, p: [f32; 2]) -> f32 {
        let fx = (p[0] - self.origin[0]) / self.step - 0.5;
        let fy = (p[1] - self.origin[1]) / self.step - 0.5;
        if fx < -1. || fy < -1. || fx > self.width as f32 || fy > self.height as f32 {
            return 0.;
        }
        let (ix, iy) = (fx.floor() as isize, fy.floor() as isize);
        let (tx, ty) = (fx - ix as f32, fy - iy as f32);
        let at = |x: isize, y: isize| {
            if x < 0 || y < 0 || x >= self.width as isize || y >= self.height as isize {
                0.
            } else {
                self.data[y as usize * self.width + x as usize]
            }
        };
        (at(ix, iy) * (1. - tx) + at(ix + 1, iy) * tx) * (1. - ty)
            + (at(ix, iy + 1) * (1. - tx) + at(ix + 1, iy + 1) * tx) * ty
    }
}
/// Image-space positions scaled to long-edge units, so distances are round.
#[derive(Clone, Copy, Debug)]
pub struct Space {
    pub(crate) scale: [f32; 2],
}
impl Space {
    pub fn new(aspect: f32) -> Self {
        Self {
            scale: if aspect >= 1. {
                [1., 1. / aspect]
            } else {
                [aspect, 1.]
            },
        }
    }
    pub fn to(&self, p: [f32; 2]) -> [f32; 2] {
        [p[0] * self.scale[0], p[1] * self.scale[1]]
    }
    pub fn from(&self, p: [f32; 2]) -> [f32; 2] {
        [p[0] / self.scale[0], p[1] / self.scale[1]]
    }
}
/// A hash of everything a raster depends on.
pub(crate) fn key(strokes: &[BrushStroke]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for s in strokes {
        for p in s.points.iter() {
            p[0].to_bits().hash(&mut h);
            p[1].to_bits().hash(&mut h);
        }
        for v in [s.radius, s.feather, s.flow, s.density] {
            v.to_bits().hash(&mut h);
        }
        (s.erase, s.auto_mask).hash(&mut h);
    }
    h.finish()
}
/// The squared distance from `p` to the segment `a`–`b`, and how far along it
/// (0–1) the nearest point lies.
fn to_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> (f32, f32) {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let (px, py) = (p[0] - a[0], p[1] - a[1]);
    let t = if len2 > 0. {
        ((px * dx + py * dy) / len2).clamp(0., 1.)
    } else {
        0.
    };
    ((px - t * dx).powi(2) + (py - t * dy).powi(2), t)
}
/// Coverage `v` once stroke `s` is laid over it with its brush's coverage `c`
/// there: painting adds `flow` of what is left, up to `density`; erasing takes
/// away `flow` of it.
fn laid(v: f32, c: f32, s: &BrushStroke) -> f32 {
    if s.erase {
        v * (1. - s.flow * c)
    } else {
        let target = v + s.flow * c * (1. - v);
        v.max(target.min(s.density))
    }
}
/// One stroke as it is being painted, for a preview drawn at positions the
/// caller chooses (the screen's pixels) rather than the raster's: the same
/// path, brush profile and flow and density rule as [`rasterize`], without
/// Auto Mask, whose colors come from the photo. A caller keeps each position's
/// nearest squared distance to the path (long-edge units), lowered segment by
/// segment as points come ([`Self::to_segment`]), and turns it into coverage
/// with [`Self::coverage`] or [`Self::erased`].
#[derive(Clone, Debug)]
pub struct LiveStroke {
    /// The brush; its points are not kept here.
    stroke: BrushStroke,
    space: Space,
    /// The path in long-edge units.
    path: Vec<[f32; 2]>,
}
impl LiveStroke {
    /// `stroke` as painted so far; its points are image-space positions.
    pub fn new(stroke: &BrushStroke, space: Space) -> Self {
        Self {
            path: stroke.points.iter().map(|p| space.to(*p)).collect(),
            stroke: BrushStroke {
                points: Vec::new().into(),
                ..stroke.clone()
            },
            space,
        }
    }
    /// The brush's radius, in long-edge units.
    pub fn radius(&self) -> f32 {
        self.stroke.radius
    }
    pub fn erases(&self) -> bool {
        self.stroke.erase
    }
    /// The next point of the path, in image space.
    pub fn push(&mut self, p: [f32; 2]) {
        self.path.push(self.space.to(p));
    }
    pub fn points(&self) -> usize {
        self.path.len()
    }
    /// The path's segments: the dab alone while it has one point, else one
    /// between each two.
    pub fn segments(&self) -> usize {
        self.path
            .len()
            .saturating_sub(1)
            .max(usize::from(!self.path.is_empty()))
    }
    /// Segment `i`'s ends, in long-edge units.
    pub fn segment(&self, i: usize) -> [[f32; 2]; 2] {
        let a = self.path[i];
        [a, *self.path.get(i + 1).unwrap_or(&a)]
    }
    /// The squared distance from `q` (long-edge units) to segment `i`.
    pub fn to_segment(&self, i: usize, q: [f32; 2]) -> f32 {
        let [a, b] = self.segment(i);
        to_segment(q, a, b).0
    }
    /// The stroke's own coverage at squared distance `d2` from its path, laid
    /// over none, as [`rasterize`] lays it: what a painting stroke covers.
    pub fn coverage(&self, d2: f32) -> f32 {
        laid(0., self.brush(d2), &self.stroke)
    }
    /// The share of the coverage under it that an erasing stroke takes away at
    /// squared distance `d2`, as [`rasterize`] takes it.
    pub fn erased(&self, d2: f32) -> f32 {
        1. - laid(1., self.brush(d2), &self.stroke)
    }
    /// The brush's coverage at squared distance `d2`, before flow and density.
    fn brush(&self, d2: f32) -> f32 {
        let s = &self.stroke;
        profile(d2.sqrt(), s.radius * (1. - s.feather), s.radius)
    }
}
/// Camera colors along the strokes for Auto Mask, sampled from the photo.
pub(crate) struct Guide<'a> {
    pub(crate) image: &'a CameraImage,
    pub(crate) frame: crate::model::image_frame::ImageFrame,
}
impl Guide<'_> {
    /// Log camera RGB at image-space position `p`.
    /// Averaged over a box `reach` decoded pixels around `p`, so texture and noise do
    /// not stop the brush.
    fn at(&self, p: [f32; 2], reach: i64) -> [f32; 3] {
        let [x, y] = self.frame.to_source(p);
        let im = self.image;
        let (cx, cy) = (x.round() as i64, y.round() as i64);
        let step = (reach / 2).max(1) as usize;
        let (mut sum, mut n) = ([0.; 3], 0.);
        for dy in (-reach..=reach).step_by(step) {
            for dx in (-reach..=reach).step_by(step) {
                let x = (cx + dx).clamp(0, im.width as i64 - 1) as usize;
                let y = (cy + dy).clamp(0, im.height as i64 - 1) as usize;
                let q = im.pixels[y * im.width as usize + x];
                for c in 0..3 {
                    sum[c] += q[c];
                }
                n += 1.;
            }
        }
        sum.map(|v| ((v / n).max(0.) + 1e-3).ln())
    }
}
pub(crate) fn rasterize(
    strokes: &[BrushStroke],
    space: Space,
    guide: Option<&Guide>,
) -> Arc<Raster> {
    let mut bounds = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    let mut smallest = f32::INFINITY;
    for s in strokes {
        smallest = smallest.min(s.radius);
        for p in s.points.iter() {
            let q = space.to(*p);
            bounds = [
                bounds[0].min(q[0] - s.radius),
                bounds[1].min(q[1] - s.radius),
                bounds[2].max(q[0] + s.radius),
                bounds[3].max(q[1] + s.radius),
            ];
        }
    }
    if strokes.is_empty() {
        return Arc::new(Raster {
            origin: [0.; 2],
            step: 1.,
            width: 0,
            height: 0,
            data: Vec::new(),
        });
    }
    let long = (bounds[2] - bounds[0]).max(bounds[3] - bounds[1]);
    let step = (smallest / 4.).max(long / MAX_SIDE);
    let width = ((bounds[2] - bounds[0]) / step).ceil() as usize + 1;
    let height = ((bounds[3] - bounds[1]) / step).ceil() as usize + 1;
    let origin = [bounds[0], bounds[1]];
    let mut data = vec![0f32; width * height];
    for s in strokes {
        let points: Vec<[f32; 2]> = s
            .points
            .iter()
            .map(|p| {
                let q = space.to(*p);
                [
                    (q[0] - origin[0]) / step - 0.5,
                    (q[1] - origin[1]) / step - 0.5,
                ]
            })
            .collect();
        let radius = s.radius / step;
        let inner = radius * (1. - s.feather);
        // Auto Mask colors of the dab centres, averaged over a few percent of the
        // brush (in decoded pixels).
        let blur = guide.map_or(1, |g| {
            ((s.radius * g.frame.long_edge() * 0.08) as i64).max(1)
        });
        let colors: Option<Vec<[f32; 3]>> = guide
            .filter(|_| s.auto_mask)
            .map(|g| s.points.iter().map(|p| g.at(*p, blur)).collect());
        let segments: Vec<(usize, usize)> = if points.len() == 1 {
            vec![(0, 0)]
        } else {
            (0..points.len() - 1).map(|i| (i, i + 1)).collect()
        };
        // Nearest distance to the path, and the dab nearest to it.
        let mut nearest = vec![(f32::INFINITY, 0usize); 0];
        let reach = radius + 1.;
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        for p in &points {
            x0 = x0.min((p[0] - reach).floor().max(0.) as usize);
            y0 = y0.min((p[1] - reach).floor().max(0.) as usize);
            x1 = x1.max(((p[0] + reach).ceil() as usize + 1).min(width));
            y1 = y1.max(((p[1] + reach).ceil() as usize + 1).min(height));
        }
        let bw = x1.saturating_sub(x0);
        nearest.resize(bw * y1.saturating_sub(y0), (f32::INFINITY, 0));
        for (a, b) in segments {
            let (pa, pb) = (points[a], points[b]);
            let sx0 = ((pa[0].min(pb[0]) - reach).floor().max(x0 as f32)) as usize;
            let sx1 = ((pa[0].max(pb[0]) + reach).ceil() as usize + 1).min(x1);
            let sy0 = ((pa[1].min(pb[1]) - reach).floor().max(y0 as f32)) as usize;
            let sy1 = ((pa[1].max(pb[1]) + reach).ceil() as usize + 1).min(y1);
            for y in sy0..sy1 {
                for x in sx0..sx1 {
                    let (d2, t) = to_segment([x as f32, y as f32], pa, pb);
                    let cell = &mut nearest[(y - y0) * bw + x - x0];
                    if d2 < cell.0 {
                        *cell = (d2, if t < 0.5 { a } else { b });
                    }
                }
            }
        }
        for y in y0..y1 {
            for x in x0..x1 {
                let (d2, dab) = nearest[(y - y0) * bw + x - x0];
                let mut c = profile(d2.sqrt(), inner, radius);
                if c <= 0. {
                    continue;
                }
                if let (Some(colors), Some(g)) = (&colors, guide) {
                    let here = g.at(
                        space.from([
                            origin[0] + (x as f32 + 0.5) * step,
                            origin[1] + (y as f32 + 0.5) * step,
                        ]),
                        blur,
                    );
                    let d: f32 = (0..3).map(|k| (here[k] - colors[dab][k]).powi(2)).sum();
                    // Similar colors pass; about half a stop of difference in every
                    // channel (a clear edge) stops the brush.
                    c *= (-d / 0.08).exp();
                }
                let v = &mut data[y * width + x];
                *v = laid(*v, c, s);
            }
        }
    }
    Arc::new(Raster {
        origin,
        step,
        width,
        height,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(
        points: &[[f32; 2]],
        feather: f32,
        flow: f32,
        density: f32,
        erase: bool,
    ) -> BrushStroke {
        BrushStroke {
            points: points.to_vec().into(),
            radius: 0.05,
            feather,
            flow,
            density,
            erase,
            auto_mask: false,
        }
    }

    /// The live stroke, sampled at the raster's own pixel centres, is the
    /// committed stroke's raster: a painted stroke's coverage, and over full
    /// coverage, what an erasing stroke leaves.
    #[test]
    fn a_live_stroke_matches_its_raster() {
        let space = Space::new(1.5);
        let paths: [&[[f32; 2]]; 3] = [
            &[[0.4, 0.5]],
            &[[0.2, 0.3], [0.5, 0.35], [0.6, 0.7]],
            &[[0.3, 0.3], [0.3, 0.3], [0.31, 0.32], [0.7, 0.32]],
        ];
        for path in paths {
            for (feather, flow, density) in [(0., 1., 1.), (0.5, 0.6, 0.8), (1., 0.3, 1.)] {
                for erase in [false, true] {
                    let s = stroke(path, feather, flow, density, erase);
                    // Erasing is measured over a stroke that covers all it reaches.
                    let strokes: Vec<BrushStroke> = if erase {
                        let mut under = stroke(path, 0., 1., 1., false);
                        under.radius = 0.2;
                        vec![under, s.clone()]
                    } else {
                        vec![s.clone()]
                    };
                    let raster = rasterize(&strokes, space, None);
                    let mut live = LiveStroke::new(
                        &BrushStroke {
                            points: path[..1].to_vec().into(),
                            ..s
                        },
                        space,
                    );
                    for p in &path[1..] {
                        live.push(*p);
                    }
                    let mut worst = 0f32;
                    for y in 0..raster.height {
                        for x in 0..raster.width {
                            let q = [
                                raster.origin[0] + (x as f32 + 0.5) * raster.step,
                                raster.origin[1] + (y as f32 + 0.5) * raster.step,
                            ];
                            let d2 = (0..live.segments())
                                .map(|i| live.to_segment(i, q))
                                .fold(f32::INFINITY, f32::min);
                            let want = raster.data[y * raster.width + x];
                            let got = if erase {
                                // The under stroke's coverage is 1 wherever this one reaches.
                                1. - live.erased(d2)
                            } else {
                                live.coverage(d2)
                            };
                            if !erase || d2.sqrt() < s.radius {
                                worst = worst.max((got - want).abs());
                            }
                        }
                    }
                    assert!(
                        worst < 1e-4,
                        "{path:?} {feather} {flow} {density} {erase}: {worst}"
                    );
                }
            }
        }
    }
}
