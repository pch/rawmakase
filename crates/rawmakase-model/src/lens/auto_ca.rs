//! Lateral chromatic aberration measured from the photo itself, for Lightroom's Remove
//! Chromatic Aberration (`crs:AutoLateralCA`), which needs no lens data.
//!
//! The image is divided into tiles. In tiles with a strong edge across the radius,
//! red and blue are matched to green by sliding them along the radius: a coarse search
//! for the best correlation, then Lucas–Kanade steps to a fraction of a pixel. A
//! robust fit of every tile's shift gives each channel's radial scale relative to
//! green as a polynomial in the radius, in the [`Radial`] form the renderer applies.
use crate::optics::Radial;
use rayon::prelude::*;

/// Largest shift searched, in pixels at full resolution of a 24 MP image.
const SEARCH: i32 = 6;
/// Tiles kept for matching, the strongest edges first.
const TILES: usize = 1200;
/// Shifts smaller than this are not worth correcting, in pixels of a 6000 × 4000 image.
const MIN_SHIFT: f32 = 0.08;

/// One tile's measurement: radius from the optical centre (fraction of the half
/// diagonal), radius in pixels,
/// red and blue shift along the radius in pixels, and weight.
#[derive(Clone, Copy, Debug)]
struct Shift {
    r: f32,
    rho: f32,
    shift: [f32; 2],
    weight: [f32; 2],
}

/// Measures `im` once for every image made from the same decode (pyramid levels,
/// reduced copies), which share its metadata. Renders call this with the decoded
/// image before sampling, so the measurement never comes from a reduced copy. Radii
/// are measured from the optical centre of the camera's own correction, which lens
/// corrections are evaluated about, else from the image centre.
pub fn prime(im: &crate::camera_data::CameraImage) {
    let m = &im.metadata;
    m.lateral_ca
        .get_or_init(|| estimate(&im.pixels, im.width, im.height, centre(m)));
}
/// The optical centre [`prime`] measures about, in fractions of the decoded image: the
/// camera's own correction's, whether or not an edit applies it, else the image
/// centre. A measurement applied without lens data is evaluated about it.
pub fn centre(m: &crate::camera_data::Metadata) -> [f32; 2] {
    (m.lens.as_ref().or(m.lens_stand_in.as_ref()))
        .map_or(crate::optics::LensCorrection::image_centre(), |l| l.centre)
}
/// The measurement made by [`prime`], if any.
pub fn measured(im: &crate::camera_data::CameraImage) -> Option<&[Radial; 2]> {
    im.metadata.lateral_ca.get()?.as_ref()
}

/// Red and blue source-radius scale relative to green, or `None` when the photo shows
/// no measurable lateral chromatic aberration. `pixels` are linear camera values;
/// radii are measured from `centre`, in fractions of the image's width and height.
pub fn estimate(
    pixels: &[[f32; 3]],
    width: u32,
    height: u32,
    centre: [f32; 2],
) -> Option<[Radial; 2]> {
    let (w, h) = (width as usize, height as usize);
    if w < 64 || h < 64 || pixels.len() != w * h {
        return None;
    }
    let half = ((w * w + h * h) as f32).sqrt() * 0.5;
    let centre = [centre[0] * w as f32, centre[1] * h as f32];
    let shifts = measure(pixels, w, h, half, centre);
    let fit = |c: usize| fit_scale(&shifts, c);
    let (red, blue) = (fit(0)?, fit(1)?);
    // Corner tiles are few, so beyond the radius that 80% of the measurements lie
    // within the scale is held rather than extrapolated.
    let mut radii: Vec<f32> = shifts.iter().map(|s| s.r).collect();
    radii.sort_by(f32::total_cmp);
    let reach = radii[(radii.len() - 1) * 4 / 5];
    let radial = |k: &[f32; 3]| {
        let knots: Vec<f32> = (0..=16).map(|i| reach * i as f32 / 16.).collect();
        let values = knots.iter().map(|r| 1. + poly(k, *r)).collect();
        Radial::new(knots, values)
    };
    // Largest shift either channel would undergo, in pixels, out to the corner.
    let shift = |r: f32, at: f32| poly(&red, r).abs().max(poly(&blue, r).abs()) * at * half;
    let largest = (0..=16)
        .map(|i| reach * i as f32 / 16.)
        .map(|r| shift(r, r))
        .fold(shift(reach, 1.), f32::max);
    (largest >= MIN_SHIFT * half / 3606.).then(|| Some([radial(&red)?, radial(&blue)?]))?
}

/// Scale minus one at radius `r`: k0 + k1 r² + k2 r⁴.
fn poly(k: &[f32; 3], r: f32) -> f32 {
    let r2 = r * r;
    k[0] + k[1] * r2 + k[2] * r2 * r2
}

fn measure(pixels: &[[f32; 3]], w: usize, h: usize, half: f32, centre: [f32; 2]) -> Vec<Shift> {
    // Tiles of about 1/90 of the long edge: 64 px on a 6000 px photo.
    let tile = (w.max(h) / 90).clamp(24, 64);
    let margin = (SEARCH as f32 * w.max(h) as f32 / 6000.).ceil() as usize + 3;
    let clip: [f32; 3] = pixels
        .par_chunks(w)
        .map(|row| {
            row.iter().fold([0f32; 3], |m, p| {
                [m[0].max(p[0]), m[1].max(p[1]), m[2].max(p[2])]
            })
        })
        .reduce(
            || [0.; 3],
            |a, b| [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])],
        );
    let [cx, cy] = centre;
    // A very narrow image has no room for a tile inside the search margin.
    let rows = h.saturating_sub(2 * margin) / tile;
    let columns = w.saturating_sub(2 * margin) / tile;
    let tiles: Vec<[usize; 2]> = (0..rows)
        .flat_map(|ty| (0..columns).map(move |tx| [tx, ty]))
        .map(|[tx, ty]| [margin + tx * tile, margin + ty * tile])
        .collect();
    let at = |x: usize, y: usize| pixels[y * w + x];
    // Edge strength across the radius in green, relative to the tile's brightness;
    // tiles near the centre, with clipped pixels or too dark are skipped.
    let mut scored: Vec<(f32, [usize; 2])> = tiles
        .par_iter()
        .filter_map(|&[x0, y0]| {
            let (mx, my) = (
                x0 as f32 + tile as f32 * 0.5 - cx,
                y0 as f32 + tile as f32 * 0.5 - cy,
            );
            let rho = (mx * mx + my * my).sqrt();
            if rho < 0.15 * half {
                return None;
            }
            let (ux, uy) = (mx / rho, my / rho);
            let (mut energy, mut sum) = (0f32, 0f32);
            for y in y0..y0 + tile {
                for x in x0..x0 + tile {
                    let p = at(x, y);
                    if (0..3).any(|c| p[c] >= 0.97 * clip[c]) {
                        return None;
                    }
                    let gx = at(x + 1, y)[1] - at(x - 1, y)[1];
                    let gy = at(x, y + 1)[1] - at(x, y - 1)[1];
                    let g = gx * ux + gy * uy;
                    energy += g * g;
                    sum += p[1];
                }
            }
            let mean = sum / (tile * tile) as f32;
            (mean > 1e-3).then_some((energy / (mean * mean), [x0, y0]))
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.truncate(TILES);
    let search = (SEARCH as f32 * w.max(h) as f32 / 6000.).ceil() as i32;
    scored
        .par_iter()
        .filter_map(|&(_, [x0, y0])| {
            let (mx, my) = (
                x0 as f32 + tile as f32 * 0.5 - cx,
                y0 as f32 + tile as f32 * 0.5 - cy,
            );
            let rho = (mx * mx + my * my).sqrt();
            let u = [mx / rho, my / rho];
            let window = Window {
                pixels,
                w,
                x0,
                y0,
                size: tile,
                u,
            };
            let [red_costs, blue_costs] = window.costs(search);
            let red = window.shift(0, &red_costs, search)?;
            let blue = window.shift(2, &blue_costs, search)?;
            Some(Shift {
                r: rho / half,
                rho,
                // Outward displacement of the channel, the opposite of the green offset
                // that matches it.
                shift: [-red.0, -blue.0],
                weight: [red.1, blue.1],
            })
        })
        .collect()
}

struct Window<'a> {
    pixels: &'a [[f32; 3]],
    w: usize,
    x0: usize,
    y0: usize,
    size: usize,
    /// Unit vector pointing away from the optical centre.
    u: [f32; 2],
}
impl Window<'_> {
    /// Green at (x, y) + s·u, bilinear.
    fn green(&self, x: usize, y: usize, s: f32) -> f32 {
        let fx = x as f32 + s * self.u[0];
        let fy = y as f32 + s * self.u[1];
        let (ix, iy) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - ix, fy - iy);
        let (ix, iy) = (ix as usize, iy as usize);
        let g = |x: usize, y: usize| self.pixels[y * self.w + x][1];
        let top = g(ix, iy) + (g(ix + 1, iy) - g(ix, iy)) * tx;
        let bottom = g(ix, iy + 1) + (g(ix + 1, iy + 1) - g(ix, iy + 1)) * tx;
        top + (bottom - top) * ty
    }
    fn points(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        (self.y0..self.y0 + self.size)
            .flat_map(move |y| (self.x0..self.x0 + self.size).map(move |x| (x, y)))
    }
    /// Red's and blue's [`residual`](Self::residual) at each whole shift from
    /// `-search` to `search`.
    fn costs(&self, search: i32) -> [Vec<f32>; 2] {
        let (red, blue) = (-search..=search)
            .map(|s| self.residual(s as f32))
            .map(|[r, b]| (r, b))
            .unzip();
        [red, blue]
    }
    /// For red and blue, least squares of channel ≈ a·green(s) + b: the fraction
    /// of the channel's variance left unexplained. Both read the same greens.
    fn residual(&self, s: f32) -> [f32; 2] {
        let n = (self.size * self.size) as f32;
        let (mut sg, mut sgg) = (0f32, 0f32);
        let (mut sv, mut svv, mut sgv) = ([0f32; 2], [0f32; 2], [0f32; 2]);
        for (x, y) in self.points() {
            let g = self.green(x, y, s);
            let p = self.pixels[y * self.w + x];
            sg += g;
            sgg += g * g;
            for (k, v) in [p[0], p[2]].into_iter().enumerate() {
                sv[k] += v;
                svv[k] += v * v;
                sgv[k] += g * v;
            }
        }
        std::array::from_fn(|k| {
            let vg = sgg - sg * sg / n;
            let vv = svv[k] - sv[k] * sv[k] / n;
            let cov = sgv[k] - sg * sv[k] / n;
            if vg <= 0. || vv <= 0. {
                return 1.;
            }
            1. - cov * cov / (vg * vv)
        })
    }
    /// Where channel `c` is displaced along the radius relative to green, in pixels,
    /// and the measurement's weight, or `None` when the tile gives no clear answer;
    /// `costs` are its residuals at each whole shift ([`costs`](Self::costs)).
    fn shift(&self, c: usize, costs: &[f32], search: i32) -> Option<(f32, f32)> {
        let best = (0..costs.len()).min_by(|a, b| costs[*a].total_cmp(&costs[*b]))?;
        if best == 0 || best == costs.len() - 1 {
            return None;
        }
        // Parabola through the best integer shift and its neighbours.
        let (a, b, d) = (costs[best - 1], costs[best], costs[best + 1]);
        let curvature = a - 2. * b + d;
        if curvature <= 0. {
            return None;
        }
        let mut s = (best as i32 - search) as f32 + 0.5 * (a - d) / curvature;
        // Gauss–Newton on channel ≈ a·green(s) + b, where green(s + δ) ≈ green(s) +
        // δ·∂green/∂u.
        let mut weight = 0.;
        let mut fit = 1.;
        for _ in 0..3 {
            let mut m = [[0f64; 3]; 3];
            let mut rhs = [0f64; 3];
            let mut vsum = [0f64; 2];
            for (x, y) in self.points() {
                let g = self.green(x, y, s) as f64;
                let dg = (self.green(x, y, s + 0.5) - self.green(x, y, s - 0.5)) as f64;
                let v = self.pixels[y * self.w + x][c] as f64;
                let row = [g, 1., dg];
                for i in 0..3 {
                    for j in 0..3 {
                        m[i][j] += row[i] * row[j];
                    }
                    rhs[i] += row[i] * v;
                }
                vsum[0] += v;
                vsum[1] += v * v;
            }
            let [gain, offset, slope] = solve3(m, rhs)?;
            if gain <= 1e-6 {
                return None;
            }
            let step = (slope / gain) as f32;
            s += step.clamp(-0.5, 0.5);
            let n = (self.size * self.size) as f64;
            let var = vsum[1] - vsum[0] * vsum[0] / n;
            // Residual variance of the fit, from the normal equations.
            let explained = gain * rhs[0] + offset * rhs[1] + slope * rhs[2];
            let sse = (vsum[1] - explained).max(0.);
            fit = if var > 0. { (sse / var) as f32 } else { 1. };
            // Information about the shift: the gradient energy, scaled to this
            // channel's level, divided by the residual.
            weight = (m[2][2] * gain * gain / (sse / n).max(1e-12)) as f32;
        }
        if s.is_nan() || s.abs() > search as f32 || fit > 0.1 {
            return None;
        }
        Some((s, weight))
    }
}

/// Solves the 3×3 system `m · x = b`, or `None` when it is singular.
fn solve3(m: [[f64; 3]; 3], b: [f64; 3]) -> Option<[f64; 3]> {
    let det = |a: [[f64; 3]; 3]| {
        a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
            - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
            + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
    };
    let d = det(m);
    if d.abs() < 1e-30 || !d.is_finite() {
        return None;
    }
    let mut out = [0.; 3];
    for (i, o) in out.iter_mut().enumerate() {
        let mut a = m;
        for r in 0..3 {
            a[r][i] = b[r];
        }
        *o = det(a) / d;
    }
    Some(out)
}

/// Robust weighted fit of channel `c`'s shifts as ρ·(k0 + k1 r² + k2 r⁴): returns the
/// coefficients, or `None` with too few measurements.
fn fit_scale(shifts: &[Shift], c: usize) -> Option<[f32; 3]> {
    if shifts.len() < 24 {
        return None;
    }
    // Fewer terms when the measurements cover the radius poorly.
    let spread = shifts.iter().map(|s| s.r).fold(0f32, f32::max)
        - shifts.iter().map(|s| s.r).fold(1f32, f32::min);
    let terms = if shifts.len() >= 150 && spread > 0.5 {
        3
    } else if shifts.len() >= 60 && spread > 0.3 {
        2
    } else {
        1
    };
    let basis = |s: &Shift| {
        let r2 = s.r * s.r;
        [s.rho, s.rho * r2, s.rho * r2 * r2]
    };
    let mut k = [0f32; 3];
    let mut robust = vec![1f32; shifts.len()];
    for _ in 0..8 {
        let mut m = [[0f64; 3]; 3];
        let mut b = [0f64; 3];
        for (s, rw) in shifts.iter().zip(&robust) {
            let f = basis(s);
            let wt = (s.weight[c] * rw) as f64;
            for i in 0..3 {
                for j in 0..3 {
                    m[i][j] += wt * (f[i] * f[j]) as f64;
                }
                b[i] += wt * (f[i] * s.shift[c]) as f64;
            }
        }
        // Unused terms are pinned to zero.
        for i in terms..3 {
            m[i] = [0.; 3];
            for row in &mut m {
                row[i] = 0.;
            }
            m[i][i] = 1.;
            b[i] = 0.;
        }
        let x = solve3(m, b)?;
        k = x.map(|v| v as f32);
        // Tukey biweight on residuals in pixels, scaled by their median deviation.
        let residuals: Vec<f32> = shifts
            .iter()
            .map(|s| {
                let f = basis(s);
                s.shift[c] - (k[0] * f[0] + k[1] * f[1] + k[2] * f[2])
            })
            .collect();
        let mut abs: Vec<f32> = residuals.iter().map(|r| r.abs()).collect();
        abs.sort_by(f32::total_cmp);
        let scale = (abs[abs.len() / 2] * 1.4826).max(0.05) * 4.685;
        for (rw, r) in robust.iter_mut().zip(&residuals) {
            let t = r / scale;
            *rw = if t.abs() < 1. {
                (1. - t * t).powi(2)
            } else {
                0.
            };
        }
    }
    k.iter().all(|v| v.is_finite()).then_some(k)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CENTRE: [f32; 2] = [0.5, 0.5];
    /// A grid of soft-edged discs, with red and blue scaled about the centre.
    fn scene(w: u32, h: u32, red: f32, blue: f32) -> Vec<[f32; 3]> {
        scene_about(w, h, red, blue, CENTRE)
    }
    /// As `scene`, with red and blue scaled about `centre` (fractions of the image).
    fn scene_about(w: u32, h: u32, red: f32, blue: f32, centre: [f32; 2]) -> Vec<[f32; 3]> {
        let (cx, cy) = (w as f32 * centre[0], h as f32 * centre[1]);
        let value = |x: f32, y: f32| {
            let (gx, gy) = ((x / 37.).fract() - 0.5, (y / 37.).fract() - 0.5);
            let d = (gx * gx + gy * gy).sqrt() * 37.;
            0.1 + 0.6 * (1. / (1. + ((d - 11.) / 1.2).exp()))
        };
        let mut px = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                let at = |k: f32| value(cx + dx / k + 1000., cy + dy / k + 1000.);
                px.push([at(red), at(1.), at(blue)]);
            }
        }
        px
    }

    #[test]
    fn measures_a_known_scale() {
        let (w, h) = (1500, 1000);
        // Red magnified by 0.08% and blue shrunk by 0.05%: 0.72 and 0.45 px at the corner.
        let px = scene(w, h, 1.0008, 0.9995);
        let [red, blue] = estimate(&px, w, h, CENTRE).expect("aberration found");
        for r in [0.3, 0.6, 0.9] {
            assert!((red.eval(r) - 1.0008).abs() < 0.0001, "{}", red.eval(r));
            assert!((blue.eval(r) - 0.9995).abs() < 0.0001, "{}", blue.eval(r));
        }
    }

    /// Lens corrections are evaluated about the optical centre, so the aberration is
    /// measured about it too: measured about the image centre, a scale about a centre
    /// 5% off it looks like a radius-dependent one.
    #[test]
    fn measures_about_the_optical_centre() {
        let (w, h) = (1500, 1000);
        let centre = [0.45, 0.54];
        let px = scene_about(w, h, 1.0008, 0.9995, centre);
        let [red, blue] = estimate(&px, w, h, centre).expect("aberration found");
        for r in [0.3, 0.6, 0.9] {
            assert!((red.eval(r) - 1.0008).abs() < 0.0001, "{}", red.eval(r));
            assert!((blue.eval(r) - 0.9995).abs() < 0.0001, "{}", blue.eval(r));
        }
    }

    #[test]
    fn narrow_images_have_no_tiles() {
        let (w, h) = (30_000, 64);
        assert!(estimate(&vec![[0.5; 3]; w * h], w as u32, h as u32, CENTRE).is_none());
    }

    /// The residual of one channel, measured on its own as it was before red and
    /// blue shared a pass over the greens.
    fn residual_alone(window: &Window, c: usize, s: f32) -> f32 {
        let n = (window.size * window.size) as f32;
        let (mut sg, mut sv, mut sgg, mut svv, mut sgv) = (0f32, 0f32, 0f32, 0f32, 0f32);
        for (x, y) in window.points() {
            let g = window.green(x, y, s);
            let v = window.pixels[y * window.w + x][c];
            sg += g;
            sv += v;
            sgg += g * g;
            svv += v * v;
            sgv += g * v;
        }
        let vg = sgg - sg * sg / n;
        let vv = svv - sv * sv / n;
        let cov = sgv - sg * sv / n;
        if vg <= 0. || vv <= 0. {
            return 1.;
        }
        1. - cov * cov / (vg * vv)
    }

    #[test]
    fn red_and_blue_together_cost_as_each_alone() {
        let (w, h) = (600usize, 400usize);
        let mut px = scene(w as u32, h as u32, 1.0008, 0.9995);
        // Noise, so no sum is exact, and a flat patch, whose variance is zero.
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        for p in &mut px {
            for v in p.iter_mut() {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                *v += (seed >> 40) as f32 / (1u64 << 24) as f32 * 0.02;
            }
        }
        for y in 300..340 {
            px[y * w + 500..y * w + 540].fill([0.3; 3]);
        }
        let search = 4;
        for (x0, y0, u) in [
            (40, 40, [-0.6, -0.8]),
            (300, 200, [0.8, 0.6]),
            (520, 120, [1., 0.]),
            (500, 300, [0.6, 0.8]),
        ] {
            let window = Window {
                pixels: &px,
                w,
                x0,
                y0,
                size: 32,
                u,
            };
            let [red, blue] = window.costs(search);
            for (i, s) in (-search..=search).enumerate() {
                let s = s as f32;
                assert_eq!(red[i].to_bits(), residual_alone(&window, 0, s).to_bits());
                assert_eq!(blue[i].to_bits(), residual_alone(&window, 2, s).to_bits());
            }
        }
    }

    #[test]
    fn finds_nothing_in_a_corrected_scene() {
        let (w, h) = (1500, 1000);
        assert!(estimate(&scene(w, h, 1., 1.), w, h, CENTRE).is_none());
    }
}
