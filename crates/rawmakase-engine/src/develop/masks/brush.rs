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
#[derive(Clone, Copy)]
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
            let (dx, dy) = (pb[0] - pa[0], pb[1] - pa[1]);
            let len2 = dx * dx + dy * dy;
            for y in sy0..sy1 {
                for x in sx0..sx1 {
                    let (px, py) = (x as f32 - pa[0], y as f32 - pa[1]);
                    let t = if len2 > 0. {
                        ((px * dx + py * dy) / len2).clamp(0., 1.)
                    } else {
                        0.
                    };
                    let d2 = (px - t * dx).powi(2) + (py - t * dy).powi(2);
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
                if s.erase {
                    *v *= 1. - s.flow * c;
                } else {
                    let target = *v + s.flow * c * (1. - *v);
                    *v = v.max(target.min(s.density));
                }
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
