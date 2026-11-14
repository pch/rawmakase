//! Mask weights for a rendered region: each output pixel is traced back to image space
//! (geometry, then lens distortion) and every mask evaluated there.
use super::LocalDelta;
use super::brush::{self, Raster, Space};
use super::range::{self, RangeInput};
use crate::develop::Geometry;
use crate::develop::masks::local::LocalDeltas;
use crate::model::image_frame::ImageFrame;
use crate::model::masks::{MaskGroup, MaskOp, MaskShape};
use crate::model::recipe::Recipe;
use crate::rendered::unit_to_u8;
use crate::{
    camera_data::CameraImage,
    develop::{image_space::LensMap, retouch::profile},
};
use rayon::prelude::*;
use std::sync::{Arc, Weak};

/// Weights of the rendered masks over a region: one byte per mask and pixel,
/// pixel-major, and each mask's slider values times its Amount.
pub(crate) struct MaskWeights {
    pub(crate) deltas: Vec<LocalDelta>,
    pub(crate) data: Arc<Vec<u8>>,
}
impl MaskWeights {
    pub(crate) fn groups(&self) -> usize {
        self.deltas.len()
    }
    /// The summed slider values at pixel `i`, or `None` when no mask reaches it.
    pub(crate) fn delta(&self, i: usize) -> Option<LocalDelta> {
        let n = self.deltas.len();
        let w = &self.data[i * n..(i + 1) * n];
        if w.iter().all(|v| *v == 0) {
            return None;
        }
        let mut sum = [0.; super::local::LEN];
        for (d, v) in self.deltas.iter().zip(w) {
            if *v > 0 {
                super::local::accumulate(&mut sum, d, *v as f32 / 255.);
            }
        }
        Some(sum)
    }
    /// Whether any mask changes one of `slots`.
    pub(crate) fn uses(&self, slots: &[usize]) -> bool {
        self.deltas.iter().any(|d| super::local::uses(d, slots))
    }
    pub(crate) fn bytes(&self) -> usize {
        self.data.len() + self.deltas.len() * super::local::LEN * 4
    }
    /// The same weights with the active masks' current slider values, which may have
    /// changed since the weights were made.
    pub(crate) fn with_deltas(&self, masks: &[MaskGroup]) -> Self {
        Self {
            deltas: masks
                .iter()
                .filter(|m| m.is_active())
                .map(|m| m.adjust.delta(m.amount))
                .collect(),
            data: self.data.clone(),
        }
    }
}
/// Which masks to evaluate.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Selection {
    /// The masks that render.
    Active,
    /// One mask, whatever its adjustment (the overlay).
    One(usize),
}
/// What a brush raster was made from: its strokes, the frame's proportions and, for
/// Auto Mask, the image whose colours guided it.
struct RasterKey {
    strokes: u64,
    space: [u32; 2],
    /// Weak, so a cached raster keeps no photo alive, while its address cannot be
    /// reused by another image as long as the entry exists.
    guide: Option<Weak<CameraImage>>,
}
impl PartialEq for RasterKey {
    fn eq(&self, other: &Self) -> bool {
        self.strokes == other.strokes
            && self.space == other.space
            && match (&self.guide, &other.guide) {
                (None, None) => true,
                (Some(a), Some(b)) => a.ptr_eq(b),
                _ => false,
            }
    }
}
/// Brush rasters kept between renders.
#[derive(Default)]
pub(crate) struct RasterCache {
    entries: Vec<(RasterKey, Arc<Raster>)>,
}
impl RasterCache {
    fn get(&mut self, key: RasterKey, make: impl FnOnce() -> Arc<Raster>) -> Arc<Raster> {
        if let Some(i) = self.entries.iter().position(|e| e.0 == key) {
            let e = self.entries.remove(i);
            let raster = e.1.clone();
            self.entries.insert(0, e);
            return raster;
        }
        let raster = make();
        self.entries.insert(0, (key, raster.clone()));
        // Keep the most recent rasters within 256 MB.
        let mut total = 0;
        self.entries.retain(|e| {
            total += e.1.bytes();
            total <= 256 << 20
        });
        raster
    }
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}
enum Shape {
    Brush(Arc<Raster>),
    /// Start and direction over its squared length, in long-edge units.
    Linear([f32; 2], [f32; 2]),
    Radial {
        center: [f32; 2],
        radii: [f32; 2],
        sin_cos: (f32, f32),
        feather: f32,
    },
    Color(Vec<[f32; 3]>, f32),
    Luminance(f32, f32, [f32; 2]),
    /// Coverage over the whole image frame; `None` when its raster could not be
    /// provided, which contributes nothing (the app reports the missing asset before
    /// anything is shown, so this only keeps a render from failing).
    Bitmap(Option<Arc<rawmakase_model::storage::bitmaps::Bitmap>>),
}
/// Bilinear coverage of a frame-sized raster at image-space `p` (`[0,1]²` over the
/// frame), sampled at pixel centres and clamped at the raster's edge; zero outside
/// the frame.
fn raster_coverage(raster: &rawmakase_model::storage::bitmaps::Bitmap, p: [f32; 2]) -> Option<f32> {
    if !(0. ..=1.).contains(&p[0]) || !(0. ..=1.).contains(&p[1]) {
        return None;
    }
    let (w, h) = (raster.width as usize, raster.height as usize);
    let fx = p[0] * w as f32 - 0.5;
    let fy = p[1] * h as f32 - 0.5;
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let at = |x: f32, y: f32| {
        let x = (x as isize).clamp(0, w as isize - 1) as usize;
        let y = (y as isize).clamp(0, h as isize - 1) as usize;
        raster.data[y * w + x] as f32 / 255.
    };
    Some(
        (at(x0, y0) * (1. - tx) + at(x0 + 1., y0) * tx) * (1. - ty)
            + (at(x0, y0 + 1.) * (1. - tx) + at(x0 + 1., y0 + 1.) * tx) * ty,
    )
}
struct Component {
    op: MaskOp,
    invert: bool,
    opacity: f32,
    shape: Shape,
}
struct Group {
    invert: bool,
    components: Vec<Component>,
}
/// Evaluates the recipe's masks at image-space positions.
pub(crate) struct Weigher {
    space: Space,
    groups: Vec<Group>,
    deltas: Vec<LocalDelta>,
    ranges: bool,
}
impl Weigher {
    /// `image` supplies Auto Mask colours and the image frame.
    pub(crate) fn new(image: &CameraImage, masks: &[MaskGroup], selection: Selection) -> Self {
        Self::build(image, masks, selection, None)
    }
    /// As `new`, with brush rasters kept in `cache` between renders.
    pub(crate) fn cached(
        image: &Arc<CameraImage>,
        masks: &[MaskGroup],
        selection: Selection,
        cache: Option<&mut RasterCache>,
    ) -> Self {
        Self::build(
            image,
            masks,
            selection,
            cache.map(|cache| (cache, Arc::downgrade(image))),
        )
    }
    fn build(
        image: &CameraImage,
        masks: &[MaskGroup],
        selection: Selection,
        cache: Option<(&mut RasterCache, Weak<CameraImage>)>,
    ) -> Self {
        let frame = ImageFrame::new(image);
        let space = Space::new(frame.aspect());
        let guide = brush::Guide { image, frame };
        let mut cache = cache;
        let mut groups = Vec::new();
        let mut deltas = Vec::new();
        for (i, m) in masks.iter().enumerate() {
            let chosen = match selection {
                Selection::Active => m.is_active(),
                Selection::One(k) => k == i && !m.components.is_empty(),
            };
            if !chosen {
                continue;
            }
            let components = m
                .components
                .iter()
                .map(|c| Component {
                    op: c.op,
                    invert: c.invert,
                    opacity: c.opacity,
                    shape: match &c.shape {
                        MaskShape::Brush { strokes } => {
                            let auto = strokes.iter().any(|s| s.auto_mask);
                            let make = || brush::rasterize(strokes, space, auto.then_some(&guide));
                            Shape::Brush(match &mut cache {
                                Some((cache, guide_image)) => cache.get(
                                    RasterKey {
                                        strokes: brush::key(strokes),
                                        space: space.scale.map(f32::to_bits),
                                        guide: auto.then(|| guide_image.clone()),
                                    },
                                    make,
                                ),
                                None => make(),
                            })
                        }
                        MaskShape::Linear { from, to } => {
                            let (a, b) = (space.to(*from), space.to(*to));
                            let d = [b[0] - a[0], b[1] - a[1]];
                            let len2 = (d[0] * d[0] + d[1] * d[1]).max(1e-12);
                            Shape::Linear(a, [d[0] / len2, d[1] / len2])
                        }
                        MaskShape::Radial {
                            center,
                            radii,
                            angle,
                            feather,
                        } => Shape::Radial {
                            center: space.to(*center),
                            radii: *radii,
                            sin_cos: angle.to_radians().sin_cos(),
                            feather: *feather,
                        },
                        MaskShape::ColorRange { samples, amount } => {
                            Shape::Color(samples.clone(), *amount)
                        }
                        MaskShape::LuminanceRange { low, high, falloff } => {
                            Shape::Luminance(*low, *high, *falloff)
                        }
                        MaskShape::Bitmap(b) => Shape::Bitmap(
                            rawmakase_model::storage::mask_assets::resolve_coverage(&b.id)
                                .ok()
                                .filter(|r| (r.width, r.height) == (b.width, b.height)),
                        ),
                    },
                })
                .collect();
            groups.push(Group {
                invert: m.invert,
                components,
            });
            deltas.push(m.adjust.delta(m.amount));
        }
        let ranges = masks
            .iter()
            .any(|m| m.components.iter().any(|c| c.shape.is_range()));
        Self {
            space,
            groups,
            deltas,
            ranges,
        }
    }
    /// Whether the evaluated masks need developed colours.
    pub(crate) fn needs_range(&self) -> bool {
        self.ranges
            && self.groups.iter().any(|g| {
                g.components
                    .iter()
                    .any(|c| matches!(c.shape, Shape::Color(..) | Shape::Luminance(..)))
            })
    }
    /// Weight of group `g` at image position `p`, with the developed colour's Oklab
    /// for range components.
    fn weight(&self, g: &Group, p: [f32; 2], lab: Option<[f32; 3]>) -> f32 {
        let q = self.space.to(p);
        let mut m = 0f32;
        for c in &g.components {
            // A raster's contribution is zero outside the frame, inverted or not.
            if let Shape::Bitmap(raster) = &c.shape {
                let inside = raster.as_ref().and_then(|r| raster_coverage(r, p));
                let v = inside.map_or(0., |v| if c.invert { 1. - v } else { v }) * c.opacity;
                m = match c.op {
                    MaskOp::Add => m.max(v),
                    MaskOp::Subtract => (m - v).max(0.),
                    MaskOp::Intersect => m.min(v),
                };
                continue;
            }
            let v = match &c.shape {
                Shape::Brush(r) => r.sample(q),
                Shape::Linear(a, d) => {
                    let t = (q[0] - a[0]) * d[0] + (q[1] - a[1]) * d[1];
                    1. - smooth(t.clamp(0., 1.))
                }
                Shape::Radial {
                    center,
                    radii,
                    sin_cos: (s, co),
                    feather,
                } => {
                    let (x, y) = (q[0] - center[0], q[1] - center[1]);
                    let (u, v) = (co * x + s * y, -s * x + co * y);
                    let d = ((u / radii[0]).powi(2) + (v / radii[1]).powi(2)).sqrt();
                    profile(d, 1. - feather, 1.)
                }
                Shape::Color(samples, amount) => {
                    lab.map_or(0., |lab| range::color_weight(samples, *amount, lab))
                }
                Shape::Luminance(low, high, falloff) => lab.map_or(0., |lab| {
                    range::luminance_weight(*low, *high, *falloff, lab)
                }),
                Shape::Bitmap(_) => unreachable!("handled above"),
            };
            let v = if c.invert { 1. - v } else { v } * c.opacity;
            m = match c.op {
                MaskOp::Add => m.max(v),
                MaskOp::Subtract => (m - v).max(0.),
                MaskOp::Intersect => m.min(v),
            };
        }
        if g.invert { 1. - m } else { m }
    }
    /// Weights of every evaluated mask over `region` of the output `g` describes,
    /// rendered from `image` (the photo or a pyramid level). `range` holds the
    /// region's developed pixels when range components need them.
    pub(crate) fn weights(
        &self,
        image: &CameraImage,
        r: &Recipe,
        g: &Geometry,
        region: [u32; 4],
        range: Option<RangeInput>,
    ) -> MaskWeights {
        let [x0, y0, w, h] = region;
        let frame = ImageFrame::new(image);
        let lens = LensMap::new(image, r);
        let n = self.groups.len();
        let mut data = vec![0u8; w as usize * h as usize * n];
        if n > 0 {
            data.par_chunks_mut(w as usize * n)
                .enumerate()
                .for_each(|(row, out)| {
                    let y = y0 + row as u32;
                    for x in 0..w {
                        let [sx, sy] = g.source(
                            (x0 + x) as f32 / g.width as f32 + 0.5 / g.width as f32,
                            (y as f32 + 0.5) / g.height as f32,
                        );
                        if g.outside(sx, sy) {
                            continue;
                        }
                        let [sx, sy] = lens.as_ref().map_or([sx, sy], |l| l.forward(sx, sy));
                        let p = frame.to_image(sx, sy);
                        let i = row * w as usize + x as usize;
                        let lab = range.map(|im| range::oklab(im.pixels[i]));
                        for (k, group) in self.groups.iter().enumerate() {
                            let v = self.weight(group, p, lab);
                            out[x as usize * n + k] = unit_to_u8(v);
                        }
                    }
                });
        }
        MaskWeights {
            deltas: self.deltas.clone(),
            data: Arc::new(data),
        }
    }
}
/// The linear gradient's transition.
fn smooth(t: f32) -> f32 {
    t * t * (3. - 2. * t)
}
