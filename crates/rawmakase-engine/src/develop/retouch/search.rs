//! Automatic source selection for a new spot, as Lightroom picks one: the nearby area
//! whose border matches the destination's border best and whose texture is similar,
//! avoiding other spots, strong edges and clipped highlights.
use super::{FeatherProfile, RetouchOp, heal};
use crate::{camera_data::CameraImage, model::image_frame::ImageFrame};

/// The search runs on a reduced copy of the neighbourhood, with the shape's radius at
/// about this many pixels.
const SEARCH_RADIUS: f32 = 10.;
/// Candidate distances, in radii (or half the brushed area's size).
const DISTANCES: [f32; 10] = [1.5, 2., 2.5, 3., 3.5, 4., 4.5, 5., 5.5, 6.];
const ANGLES: usize = 32;

/// A reduced neighbourhood of the photo in log values.
struct Patch {
    width: usize,
    height: usize,
    /// Decoded pixel of the patch's (0, 0) and decoded pixels per patch pixel.
    origin: [f32; 2],
    scale: f32,
    logs: Vec<[f32; 3]>,
    clipped: Vec<bool>,
}
impl Patch {
    fn new(im: &CameraImage, rect: heal::PixelRect, scale: f32) -> Self {
        let width = (((rect[2] - rect[0]) as f32 / scale).floor() as usize).max(1);
        let height = (((rect[3] - rect[1]) as f32 / scale).floor() as usize).max(1);
        let step = scale.max(1.);
        let mut logs = Vec::with_capacity(width * height);
        let mut clipped = Vec::with_capacity(width * height);
        let wb = im.metadata.wb.map(|v| v.max(1e-3));
        for y in 0..height {
            for x in 0..width {
                let x0 = rect[0] as f32 + x as f32 * scale;
                let y0 = rect[1] as f32 + y as f32 * scale;
                let (mut sum, mut n, mut clip) = ([0.; 3], 0., false);
                let taps = (step.ceil() as usize).min(4);
                for j in 0..taps {
                    for i in 0..taps {
                        let sx = (x0 + (i as f32 + 0.5) * step / taps as f32) as i64;
                        let sy = (y0 + (j as f32 + 0.5) * step / taps as f32) as i64;
                        let sx = sx.clamp(0, im.width as i64 - 1) as usize;
                        let sy = sy.clamp(0, im.height as i64 - 1) as usize;
                        let p = im.pixels[sy * im.width as usize + sx];
                        for c in 0..3 {
                            sum[c] += p[c];
                        }
                        clip |= (0..3).any(|c| p[c] / wb[c] >= 0.97);
                        n += 1.;
                    }
                }
                logs.push(sum.map(|v| ((v / n).max(0.) + 1e-3).ln()));
                clipped.push(clip);
            }
        }
        Self {
            width,
            height,
            origin: [rect[0] as f32, rect[1] as f32],
            scale,
            logs,
            clipped,
        }
    }
    fn to_patch(&self, p: [f32; 2]) -> [f32; 2] {
        [
            (p[0] - self.origin[0]) / self.scale,
            (p[1] - self.origin[1]) / self.scale,
        ]
    }
    fn at(&self, x: i32, y: i32) -> Option<usize> {
        (x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height)
            .then(|| y as usize * self.width + x as usize)
    }
    fn gradient(&self, i: usize) -> f32 {
        let (x, y) = ((i % self.width) as i32, (i / self.width) as i32);
        let get = |x: i32, y: i32| self.at(x, y).map_or(self.logs[i], |j| self.logs[j]);
        let (l, r, u, d) = (get(x - 1, y), get(x + 1, y), get(x, y - 1), get(x, y + 1));
        (0..3)
            .map(|c| (r[c] - l[c]).powi(2) + (d[c] - u[c]).powi(2))
            .sum::<f32>()
            * 0.25
    }
}
/// A destination and its border ring, as patch pixel indices.
struct Shape {
    inside: Vec<(i32, i32)>,
    ring: Vec<(i32, i32)>,
}
impl Shape {
    fn new(points: &[[f32; 2]], starts: &[usize], radius: f32, patch: &Patch) -> Self {
        let rect = [0, 0, patch.width as i32, patch.height as i32];
        let core = heal::coverage(points, starts, radius, 0., FeatherProfile::Smoothstep, rect);
        let outer = heal::coverage(
            points,
            starts,
            radius * 1.4 + 1.,
            0.,
            FeatherProfile::Smoothstep,
            rect,
        );
        let (mut inside, mut ring) = (Vec::new(), Vec::new());
        for i in 0..core.len() {
            let p = ((i % patch.width) as i32, (i / patch.width) as i32);
            if core[i] > 0. {
                inside.push(p);
            } else if outer[i] > 0. {
                ring.push(p);
            }
        }
        Self { inside, ring }
    }
}
/// The source offset (image space) for `op`, or `None` when no candidate fits in the
/// photo. `others` are the photo's other operations, whose destinations make poor
/// sources; offsets within a radius of those in `avoid` are skipped, so asking again
/// gives the next best source.
pub fn find_source(
    image: &CameraImage,
    op: &RetouchOp,
    others: &[RetouchOp],
    avoid: &[[f32; 2]],
) -> Option<[f32; 2]> {
    let frame = ImageFrame::new(image);
    let placed = heal::Placed::new(op, &frame, FeatherProfile::Smoothstep);
    let dest = placed.dest();
    // Brushed areas search at distances relative to their size, not the dab radius.
    let extent = ((dest[2] - dest[0]).max(dest[3] - dest[1]) as f32 * 0.5).max(placed.radius);
    let reach = extent * DISTANCES[DISTANCES.len() - 1] + extent + 2.;
    let rect = [
        dest[0] - reach as i32,
        dest[1] - reach as i32,
        dest[2] + reach as i32,
        dest[3] + reach as i32,
    ];
    let rect = [
        rect[0].max(0),
        rect[1].max(0),
        rect[2].min(image.width as i32),
        rect[3].min(image.height as i32),
    ];
    // Keep the patch small enough for an interactive search.
    let scale = (placed.radius / SEARCH_RADIUS)
        .max((rect[2] - rect[0]).max(rect[3] - rect[1]) as f32 / 600.)
        .max(1.);
    let patch = Patch::new(image, rect, scale);
    let points: Vec<[f32; 2]> = placed.points.iter().map(|p| patch.to_patch(*p)).collect();
    let radius = placed.radius / scale;
    let shape = Shape::new(&points, &placed.starts, radius, &patch);
    if shape.inside.is_empty() || shape.ring.is_empty() {
        return None;
    }
    let occupied: Vec<bool> = {
        let mut o = vec![false; patch.width * patch.height];
        let r = [0, 0, patch.width as i32, patch.height as i32];
        for other in others.iter().filter(|o| *o != op) {
            let p = heal::Placed::new(other, &frame, FeatherProfile::Smoothstep);
            let pts: Vec<[f32; 2]> = p.points.iter().map(|q| patch.to_patch(*q)).collect();
            for (i, c) in heal::coverage(
                &pts,
                &p.starts,
                p.radius / scale,
                0.,
                FeatherProfile::Smoothstep,
                r,
            )
            .iter()
            .enumerate()
            {
                o[i] |= *c > 0.;
            }
        }
        o
    };
    let own: std::collections::HashSet<(i32, i32)> = shape.inside.iter().copied().collect();
    let ring_texture = mean(
        shape
            .ring
            .iter()
            .filter_map(|(x, y)| patch.at(*x, *y))
            .map(|i| patch.gradient(i)),
    );
    let score = |ox: i32, oy: i32| -> Option<f32> {
        let mut ssd = 0.;
        for (x, y) in &shape.ring {
            let (a, b) = (patch.at(*x, *y)?, patch.at(x + ox, y + oy)?);
            ssd += (0..3)
                .map(|c| (patch.logs[a][c] - patch.logs[b][c]).powi(2))
                .sum::<f32>();
        }
        let ssd = ssd / shape.ring.len() as f32;
        let (mut texture, mut penalty) = (0., 0.);
        for (x, y) in &shape.inside {
            let b = patch.at(x + ox, y + oy)?;
            texture += patch.gradient(b);
            if patch.clipped[b] {
                penalty += 1.;
            }
            if occupied[b] {
                penalty += 0.5;
            }
            if own.contains(&(x + ox, y + oy)) {
                penalty += 1.;
            }
        }
        let n = shape.inside.len() as f32;
        let texture = ((texture / n).sqrt() - ring_texture.sqrt()).powi(2);
        Some(ssd + 2. * texture + 0.05 * penalty / n)
    };
    let avoided = |ox: f32, oy: f32| {
        avoid.iter().any(|a| {
            let s = frame.to_source([op.pin()[0] + a[0], op.pin()[1] + a[1]]);
            let d = frame.to_source(op.pin());
            let (ax, ay) = ((s[0] - d[0]) / scale, (s[1] - d[1]) / scale);
            (ax - ox).hypot(ay - oy) < radius.max(2.)
        })
    };
    let reach = extent / scale;
    let mut best: Option<(f32, i32, i32)> = None;
    for distance in DISTANCES {
        for k in 0..ANGLES {
            let angle = k as f32 / ANGLES as f32 * std::f32::consts::TAU;
            let (ox, oy) = (
                angle.cos() * distance * reach,
                angle.sin() * distance * reach,
            );
            if avoided(ox, oy) {
                continue;
            }
            let (ox, oy) = (ox.round() as i32, oy.round() as i32);
            if let Some(s) = score(ox, oy)
                && best.is_none_or(|b| s < b.0)
            {
                best = Some((s, ox, oy));
            }
        }
    }
    let (mut s, mut ox, mut oy) = best?;
    // Refine around the best candidate.
    for _ in 0..4 {
        let mut moved = false;
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            if avoided((ox + dx) as f32, (oy + dy) as f32) {
                continue;
            }
            if let Some(t) = score(ox + dx, oy + dy)
                && t < s
            {
                (s, ox, oy, moved) = (t, ox + dx, oy + dy, true);
            }
        }
        if !moved {
            break;
        }
    }
    let anchor = op.pin();
    let a = frame.to_source(anchor);
    let target = frame.to_image(a[0] + ox as f32 * scale, a[1] + oy as f32 * scale);
    Some([target[0] - anchor[0], target[1] - anchor[1]])
}
fn mean(values: impl Iterator<Item = f32>) -> f32 {
    let (mut sum, mut n) = (0., 0.);
    for v in values {
        sum += v;
        n += 1.;
    }
    if n > 0. { sum / n } else { 0. }
}
