//! Finding the pupil inside the circle the user drags over an eye, as Lightroom's tool
//! does before it places the correction.
//!
//! A red pupil is scored by redness, the log ratio of red to the larger of green and
//! blue; a pet's glowing one by brightness, the log of the largest channel. The pupil
//! is the connected area around the highest-scoring point near the centre whose score
//! is more than half-way from the surrounding face's to the pupil's, with any catchlight
//! inside filled in. Its ellipse has the area's centre, and semi-axes and correlation
//! from its second moments (a filled ellipse has semi-axes of twice the standard
//! deviation).
use crate::model::red_eye::MAX_CORRELATION;
use crate::{camera_data::CameraImage, model::image_frame::ImageFrame};

/// Why no pupil was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetectError {
    /// Nothing red enough near the centre of the circle.
    NotRed,
    /// The red area has no edge within the circle: the circle is too small, or the
    /// area isn't a pupil.
    NoEdge,
    /// The red area is larger than a correction can be.
    TooLarge,
}
impl std::fmt::Display for DetectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Lightroom's warning.
        f.write_str("Unable to find red eye. Be sure to use an area that includes the entire eye.")
    }
}

/// A pupil's ellipse in image space (see [`RedEyeOp`](crate::model::red_eye::RedEyeOp)).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pupil {
    pub center: [f32; 2],
    pub radius: [f32; 2],
    pub correlation: f32,
}

/// What a pupil to correct looks like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glow {
    /// Red: scored by the log ratio of red to the larger of green and blue.
    Red,
    /// Any colour brighter than the eye around it (an animal's eye shine): scored by
    /// the log of the largest channel.
    Bright,
}
/// A glowing pupil must be at least this many times brighter than what borders it.
const MIN_GLOW: f32 = 1.6;
/// Red must be at least this many times the larger of green and blue at the pupil (in
/// linear values a brown iris reaches about 2.5, a red pupil 10 or more).
const MIN_RATIO: f32 = 3.5;
/// Where a glowing pupil's edge lies, as a fraction of its brightness.
const GLOW_EDGE: f32 = 0.65;
/// The largest search radius, in grid cells; larger circles are subsampled.
const MAX_GRID_RADIUS: f32 = 160.;
/// Where the pupil's edge lies between the surroundings' redness and the pupil's.
const EDGE: f32 = 0.5;

/// The pupil glowing as `glow` within `radius` (a fraction of the long edge) of
/// image-space position `center` in `im`.
pub fn find_pupil(
    im: &CameraImage,
    center: [f32; 2],
    radius: f32,
    glow: Glow,
) -> Result<Pupil, DetectError> {
    let frame = ImageFrame::new(im);
    let [cx, cy] = frame.to_source(center);
    let r = (radius * frame.long_edge()).max(3.);
    let (w, h) = (im.width as i32, im.height as i32);
    let x0 = ((cx - r).floor() as i32).clamp(0, w);
    let y0 = ((cy - r).floor() as i32).clamp(0, h);
    let x1 = ((cx + r).ceil() as i32 + 1).clamp(0, w);
    let y1 = ((cy + r).ceil() as i32 + 1).clamp(0, h);
    // Large circles are searched on every `step`th pixel, so a search never costs more
    // than a circle of `MAX_GRID_RADIUS` pixels.
    let step = (r / MAX_GRID_RADIUS).ceil().max(1.) as usize;
    let (gw, gh) = ((x1 - x0) as usize / step, (y1 - y0) as usize / step);
    if gw < 3 || gh < 3 {
        return Err(DetectError::NotRed);
    }
    // Decoded coordinates of grid cell (`x`, `y`).
    let decoded = |x: usize, y: usize| (x0 as usize + x * step, y0 as usize + y * step);
    let at = |x: usize, y: usize| {
        let (sx, sy) = decoded(x, y);
        im.pixels[sy * im.width as usize + sx]
    };
    let distance = |x: usize, y: usize| {
        let (sx, sy) = decoded(x, y);
        (sx as f32 - cx).hypot(sy as f32 - cy) / r
    };
    // A small floor keeps noise in the darkest pixels from looking red.
    let floor = 2e-3;
    let redness: Vec<f32> = (0..gw * gh)
        .map(|i| {
            let p = at(i % gw, i / gw);
            match glow {
                Glow::Red => ((p[0].max(0.) + floor) / (p[1].max(p[2]).max(0.) + floor)).ln(),
                Glow::Bright => (p[0].max(p[1]).max(p[2]).max(0.) + floor).ln(),
            }
        })
        .collect();
    let smooth = |x: usize, y: usize| {
        let (mut sum, mut n) = (0., 0.);
        for yy in y.saturating_sub(1)..(y + 2).min(gh) {
            for xx in x.saturating_sub(1)..(x + 2).min(gw) {
                sum += redness[yy * gw + xx];
                n += 1.;
            }
        }
        sum / n
    };
    // The reddest point near the centre, and the redness of the circle's rim.
    let mut seed = None::<(usize, usize, f32)>;
    let mut rim = Vec::new();
    let mut central = Vec::new();
    let mut central_cells = Vec::new();
    for y in 0..gh {
        for x in 0..gw {
            let d = distance(x, y);
            if d <= 0.6 {
                let v = smooth(x, y);
                central.push(v);
                central_cells.push((y * gw + x, v));
                if seed.is_none_or(|s| v > s.2) {
                    seed = Some((x, y, v));
                }
            } else if (0.85..=1.).contains(&d) {
                rim.push(redness[y * gw + x]);
            }
        }
    }
    let (sx, sy, peak) = seed.ok_or(DetectError::NotRed)?;
    // A catchlight is the brightest thing in a glowing pupil; the pupil's own level is
    // taken where the brightest twentieth of the centre begins, so the edge found is
    // the pupil's, not the catchlight's. A pupil covers more than that of the centre
    // in circles up to about seven times its radius, a catchlight much less.
    let peak = match glow {
        Glow::Red => peak,
        Glow::Bright => {
            central.sort_by(f32::total_cmp);
            central[central.len() * 19 / 20].min(peak)
        }
    };
    if rim.is_empty() {
        return Err(DetectError::NotRed);
    }
    rim.sort_by(f32::total_cmp);
    let around = rim[rim.len() / 2];
    // A red pupil must be red; a glowing one is judged against its edge below.
    if glow == Glow::Red && peak < MIN_RATIO.ln() {
        return Err(DetectError::NotRed);
    }
    let threshold = match glow {
        Glow::Red => around + EDGE * (peak - around),
        // The face around an animal's eye can be brighter than its iris, or even its
        // pupil: the edge is where the glow falls to `GLOW_EDGE` of the pupil's level.
        Glow::Bright => peak + GLOW_EDGE.ln(),
    };
    // The connected area at least that red, within the circle.
    let inside = |x: usize, y: usize| distance(x, y) <= 1.;
    let mut area = vec![false; gw * gh];
    let mut stack = vec![(sx, sy)];
    area[sy * gw + sx] = true;
    while let Some((x, y)) = stack.pop() {
        for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
            let (nx, ny) = (x as i32 + dx, y as i32 + dy);
            if nx < 0 || ny < 0 || nx >= gw as i32 || ny >= gh as i32 {
                continue;
            }
            let (nx, ny) = (nx as usize, ny as usize);
            let i = ny * gw + nx;
            if !area[i] && inside(nx, ny) && redness[i] >= threshold {
                area[i] = true;
                stack.push((nx, ny));
            }
        }
    }
    // The bright cells that set a glowing pupil's level must be the pupil found, not a
    // catchlight in a dark pupil inside a lighter iris.
    if glow == Glow::Bright {
        let bright: Vec<usize> = central_cells
            .iter()
            .filter(|(_, v)| *v >= peak)
            .map(|(i, _)| *i)
            .collect();
        let found = bright.iter().filter(|i| area[**i]).count();
        if found * 2 < bright.len() {
            return Err(DetectError::NotRed);
        }
    }
    // A glowing pupil must stand out from what borders it (the iris, not the face),
    // judged on the second ring of pixels around it, past its anti-aliased edge.
    if glow == Glow::Bright {
        let grow = |inside: &[bool]| -> Vec<bool> {
            (0..gw * gh)
                .map(|i| {
                    let (x, y) = ((i % gw) as i32, (i / gw) as i32);
                    inside[i]
                        || [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)]
                            .iter()
                            .any(|(dx, dy)| {
                                let (nx, ny) = (x + dx, y + dy);
                                nx >= 0
                                    && ny >= 0
                                    && (nx as usize) < gw
                                    && (ny as usize) < gh
                                    && inside[ny as usize * gw + nx as usize]
                            })
                })
                .collect()
        };
        let (one, two) = {
            let one = grow(&area);
            let two = grow(&one);
            (one, two)
        };
        let mut border: Vec<f32> = (0..gw * gh)
            .filter(|i| two[*i] && !one[*i])
            .map(|i| redness[i])
            .collect();
        border.sort_by(f32::total_cmp);
        if border.is_empty() || peak - border[border.len() / 2] < MIN_GLOW.ln() {
            return Err(DetectError::NotRed);
        }
    }
    // An area reaching the circle's rim all round is not a pupil.
    let on_rim = (0..gw * gh)
        .filter(|i| area[*i] && distance(i % gw, i / gw) > 0.9)
        .count();
    let rim_pixels = (0..gw * gh)
        .filter(|i| (0.9..=1.).contains(&distance(i % gw, i / gw)))
        .count();
    if on_rim * 2 > rim_pixels {
        return Err(DetectError::NoEdge);
    }
    let before = area.clone();
    fill_holes(&mut area, gw, gh);
    // A glow is a disc, perhaps with a catchlight in it, brighter than the glow; a ring
    // around something darker at its middle (a light iris around an ordinary pupil,
    // however small) is not.
    if glow == Glow::Bright {
        let cells: Vec<usize> = (0..gw * gh).filter(|i| area[*i]).collect();
        let (mx, my) = cells.iter().fold((0., 0.), |(x, y), i| {
            (x + (i % gw) as f32, y + (i / gw) as f32)
        });
        let n = cells.len().max(1) as f32;
        let middle = (my / n).round() as usize * gw + (mx / n).round() as usize;
        let mean = |inside: bool| {
            let values: Vec<f32> = cells
                .iter()
                .filter(|i| before[**i] == inside)
                .map(|i| redness[*i])
                .collect();
            values.iter().sum::<f32>() / values.len().max(1) as f32
        };
        if !before[middle] && mean(false) < mean(true) {
            return Err(DetectError::NotRed);
        }
    }
    // Second moments of the filled area.
    let (mut n, mut mx, mut my) = (0f64, 0f64, 0f64);
    for i in (0..gw * gh).filter(|i| area[*i]) {
        n += 1.;
        mx += (i % gw) as f64;
        my += (i / gw) as f64;
    }
    if n < 3. {
        return Err(DetectError::NotRed);
    }
    let (mx, my) = (mx / n, my / n);
    let (mut vxx, mut vyy, mut vxy) = (0f64, 0f64, 0f64);
    for i in (0..gw * gh).filter(|i| area[*i]) {
        let (dx, dy) = ((i % gw) as f64 - mx, (i / gw) as f64 - my);
        vxx += dx * dx;
        vyy += dy * dy;
        vxy += dx * dy;
    }
    // In decoded pixels; a cell's own extent (1/12) keeps tiny areas from collapsing
    // to a line.
    let s2 = (step * step) as f64;
    let (vxx, vyy, vxy) = (
        (vxx / n + 1. / 12.) * s2,
        (vyy / n + 1. / 12.) * s2,
        vxy / n * s2,
    );
    let source_radius = [2. * vxx.sqrt() as f32, 2. * vyy.sqrt() as f32];
    let correlation = ((vxy / (vxx * vyy).sqrt()) as f32).clamp(-MAX_CORRELATION, MAX_CORRELATION);
    let long = frame.long_edge();
    let center = frame.to_image(
        x0 as f32 + (mx * step as f64) as f32,
        y0 as f32 + (my * step as f64) as f32,
    );
    // Back from decoded axes to image axes: a quarter turn swaps them and mirrors the tilt.
    let (radius, correlation) = if frame.turns % 2 == 1 {
        (
            [source_radius[1] / long, source_radius[0] / long],
            -correlation,
        )
    } else {
        (
            [source_radius[0] / long, source_radius[1] / long],
            correlation,
        )
    };
    if radius
        .iter()
        .any(|r| *r > crate::model::red_eye::MAX_RADIUS)
    {
        return Err(DetectError::TooLarge);
    }
    Ok(Pupil {
        center,
        radius,
        correlation,
    })
}

/// Marks pixels enclosed by `area` (not reachable from the grid's border without
/// crossing it) as part of it: a catchlight inside a pupil.
fn fill_holes(area: &mut [bool], w: usize, h: usize) {
    let mut outside = vec![false; w * h];
    let mut stack: Vec<usize> = (0..w * h)
        .filter(|i| {
            let (x, y) = (i % w, i / w);
            (x == 0 || y == 0 || x == w - 1 || y == h - 1) && !area[*i]
        })
        .collect();
    for i in &stack {
        outside[*i] = true;
    }
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        let mut visit = |j: usize| {
            if !area[j] && !outside[j] {
                outside[j] = true;
                stack.push(j);
            }
        };
        if x > 0 {
            visit(i - 1);
        }
        if x + 1 < w {
            visit(i + 1);
        }
        if y > 0 {
            visit(i - w);
        }
        if y + 1 < h {
            visit(i + w);
        }
    }
    for (a, o) in area.iter_mut().zip(outside) {
        *a = !o;
    }
}
