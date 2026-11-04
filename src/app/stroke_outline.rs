//! The outline of a brush stroke, as Lightroom draws a Remove brush: the edge of
//! everything within the brush radius of the stroke's path, with round ends and
//! corners, traced on a grid with marching squares.
use eframe::egui::{Pos2, Vec2};

/// Most grid cells across the stroke's bounds, so a long stroke stays cheap to trace.
const MAX_CELLS: f32 = 240.;

/// The outline of the area within `radius` of the path through `points` (screen
/// space), as line segments about `step` points long or longer for large strokes.
/// A brush too thin for the grid gets no outline; the caller draws its path instead.
pub(in crate::app) fn outline(points: &[Pos2], radius: f32, step: f32) -> Vec<[Pos2; 2]> {
    if points.is_empty() || radius <= 0. {
        return Vec::new();
    }
    let (mut min, mut max) = (points[0], points[0]);
    for p in points {
        min = min.min(*p);
        max = max.max(*p);
    }
    let pad = Vec2::splat(radius + 2. * step);
    let (min, max) = (min - pad, max + pad);
    let size = max - min;
    let step = step.max(size.x.max(size.y) / MAX_CELLS);
    if radius < step {
        return Vec::new();
    }
    let grid = Grid {
        min,
        step,
        nx: (size.x / step).ceil() as usize + 1,
        ny: (size.y / step).ceil() as usize + 1,
    };
    let segments: Vec<(Pos2, Pos2)> = if points.len() == 1 {
        vec![(points[0], points[0])]
    } else {
        points.windows(2).map(|w| (w[0], w[1])).collect()
    };
    let field = grid.distance_field(&segments, radius);
    grid.contour(&field)
}

/// A sampling grid over the stroke, `step` points apart.
struct Grid {
    min: Pos2,
    step: f32,
    nx: usize,
    ny: usize,
}
impl Grid {
    fn at(&self, i: usize, j: usize) -> Pos2 {
        self.min + Vec2::new(i as f32 * self.step, j as f32 * self.step)
    }
    /// Each grid point's distance to the path, less `radius`: negative inside. The
    /// nearest segment is found by seeding the cells along each segment and passing
    /// nearest segments to neighbours (two raster sweeps), so the work grows with the
    /// grid and the path's length, not with how often the path covers one place.
    fn distance_field(&self, segments: &[(Pos2, Pos2)], radius: f32) -> Vec<f32> {
        let (nx, ny) = (self.nx, self.ny);
        let mut nearest: Vec<Option<usize>> = vec![None; nx * ny];
        let mut dist = vec![f32::INFINITY; nx * ny];
        let mut offer = |i: usize, j: usize, s: usize, nearest: &mut Vec<Option<usize>>| {
            let (a, b) = segments[s];
            let d = distance_to_segment(self.at(i, j), a, b);
            if d < dist[j * nx + i] {
                dist[j * nx + i] = d;
                nearest[j * nx + i] = Some(s);
            }
        };
        for (s, &(a, b)) in segments.iter().enumerate() {
            let samples = ((b - a).length() / (0.5 * self.step)).ceil().max(1.) as usize;
            for k in 0..=samples {
                let p = a + (b - a) * (k as f32 / samples as f32) - self.min;
                let (ci, cj) = ((p.x / self.step).floor(), (p.y / self.step).floor());
                for (di, dj) in [(0., 0.), (1., 0.), (0., 1.), (1., 1.)] {
                    let (i, j) = ((ci + di) as usize, (cj + dj) as usize);
                    if i < nx && j < ny {
                        offer(i, j, s, &mut nearest);
                    }
                }
            }
        }
        let neighbours_forward = [(-1, -1), (0, -1), (1, -1), (-1, 0)];
        let neighbours_back = [(1, 1), (0, 1), (-1, 1), (1, 0)];
        let mut sweep = |order: Vec<(usize, usize)>, from: [(isize, isize); 4]| {
            for (i, j) in order {
                for (di, dj) in from {
                    let (ni, nj) = (i as isize + di, j as isize + dj);
                    if ni < 0 || nj < 0 || ni as usize >= nx || nj as usize >= ny {
                        continue;
                    }
                    if let Some(s) = nearest[nj as usize * nx + ni as usize] {
                        offer(i, j, s, &mut nearest);
                    }
                }
            }
        };
        let forward: Vec<(usize, usize)> =
            (0..ny).flat_map(|j| (0..nx).map(move |i| (i, j))).collect();
        let back: Vec<(usize, usize)> = forward.iter().rev().copied().collect();
        sweep(forward, neighbours_forward);
        sweep(back, neighbours_back);
        self.refine(&mut dist, segments, radius);
        dist.iter().map(|d| d - radius).collect()
    }
    /// The sweeps give each point the distance to some segment, which can be more than
    /// the nearest one where a stroke folds back, leaving a point that is inside marked
    /// outside. Only where the inside meets the outside can that show, so those points
    /// are checked against every segment that can reach them (found through buckets a
    /// brush wide, stopping at the first that puts the point inside); each one found
    /// inside has its neighbours checked too, until the edge is where it belongs.
    fn refine(&self, dist: &mut [f32], segments: &[(Pos2, Pos2)], radius: f32) {
        let (nx, ny) = (self.nx, self.ny);
        let bucket = radius.max(8. * self.step);
        let (bx, by) = (
            ((nx as f32 * self.step) / bucket).ceil() as usize + 1,
            ((ny as f32 * self.step) / bucket).ceil() as usize + 1,
        );
        let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); bx * by];
        for (s, &(a, b)) in segments.iter().enumerate() {
            let lo = a.min(b) - Vec2::splat(radius) - self.min;
            let hi = a.max(b) + Vec2::splat(radius) - self.min;
            let (i0, j0) = (
                (lo.x / bucket).floor().max(0.) as usize,
                (lo.y / bucket).floor().max(0.) as usize,
            );
            let (i1, j1) = (
                ((hi.x / bucket).floor() as usize).min(bx - 1),
                ((hi.y / bucket).floor() as usize).min(by - 1),
            );
            for j in j0..=j1 {
                for i in i0..=i1 {
                    buckets[j * bx + i].push(s);
                }
            }
        }
        let outside = |d: f32| d >= radius;
        let mut checked = vec![false; nx * ny];
        let mut queue: Vec<(usize, usize)> = Vec::new();
        for j in 0..ny {
            for i in 0..nx {
                if !outside(dist[j * nx + i]) {
                    continue;
                }
                let next_to_inside =
                    neighbours(i, j, nx, ny).any(|(ni, nj)| !outside(dist[nj * nx + ni]));
                if next_to_inside {
                    checked[j * nx + i] = true;
                    queue.push((i, j));
                }
            }
        }
        while let Some((i, j)) = queue.pop() {
            let p = self.at(i, j);
            let rel = p - self.min;
            let b = ((rel.y / bucket) as usize).min(by - 1) * bx
                + ((rel.x / bucket) as usize).min(bx - 1);
            let d = &mut dist[j * nx + i];
            for &s in &buckets[b] {
                let (a, e) = segments[s];
                *d = d.min(distance_to_segment(p, a, e));
                if !outside(*d) {
                    break;
                }
            }
            if !outside(*d) {
                for (ni, nj) in neighbours(i, j, nx, ny) {
                    let k = nj * nx + ni;
                    if !checked[k] && outside(dist[k]) {
                        checked[k] = true;
                        queue.push((ni, nj));
                    }
                }
            }
        }
    }
    /// Marching squares: where the field crosses zero along each cell edge.
    fn contour(&self, field: &[f32]) -> Vec<[Pos2; 2]> {
        let nx = self.nx;
        let mut out = Vec::new();
        let cross = |p: Pos2, q: Pos2, dp: f32, dq: f32| p + (q - p) * (dp / (dp - dq));
        for j in 0..self.ny - 1 {
            for i in 0..nx - 1 {
                // Corners clockwise from the top left; edge k joins corner k to k + 1.
                let corners = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)];
                let d = corners.map(|(i, j)| field[j * nx + i]);
                let p = corners.map(|(i, j)| self.at(i, j));
                let mut crossings = Vec::with_capacity(4);
                for k in 0..4 {
                    let n = (k + 1) % 4;
                    if (d[k] < 0.) != (d[n] < 0.) && d[k].is_finite() && d[n].is_finite() {
                        crossings.push(cross(p[k], p[n], d[k], d[n]));
                    }
                }
                match crossings.len() {
                    2 => out.push([crossings[0], crossings[1]]),
                    // A saddle: cut off the two corners on the other side from the
                    // cell's centre, so what the centre joins stays joined.
                    4 => {
                        let centre_inside = d.iter().sum::<f32>() < 0.;
                        let corner_0_inside = d[0] < 0.;
                        if centre_inside == corner_0_inside {
                            // Corners 1 and 3 are cut off.
                            out.push([crossings[0], crossings[1]]);
                            out.push([crossings[2], crossings[3]]);
                        } else {
                            // Corners 0 and 2 are cut off.
                            out.push([crossings[3], crossings[0]]);
                            out.push([crossings[1], crossings[2]]);
                        }
                    }
                    _ => {}
                }
            }
        }
        out
    }
}

/// The up to eight grid points around `(i, j)`.
fn neighbours(i: usize, j: usize, nx: usize, ny: usize) -> impl Iterator<Item = (usize, usize)> {
    (-1isize..=1)
        .flat_map(|dj| (-1isize..=1).map(move |di| (di, dj)))
        .filter(|&(di, dj)| di != 0 || dj != 0)
        .filter_map(move |(di, dj)| {
            let (ni, nj) = (i as isize + di, j as isize + dj);
            (ni >= 0 && nj >= 0 && (ni as usize) < nx && (nj as usize) < ny)
                .then_some((ni as usize, nj as usize))
        })
}

fn distance_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len = ab.length_sq();
    let t = if len > 0. {
        ((p - a).dot(ab) / len).clamp(0., 1.)
    } else {
        0.
    };
    p.distance(a + ab * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn distance_to_path(p: Pos2, path: &[Pos2]) -> f32 {
        if path.len() == 1 {
            return p.distance(path[0]);
        }
        path.windows(2)
            .map(|w| distance_to_segment(p, w[0], w[1]))
            .fold(f32::INFINITY, f32::min)
    }

    #[test]
    fn the_outline_follows_the_brush_edge_with_round_corners_and_ends() {
        let r = 20.;
        for path in [
            vec![Pos2::new(100., 100.)],
            vec![Pos2::new(100., 100.), Pos2::new(300., 100.)],
            // A sharp turn, where a wide line draws a miter spike.
            vec![
                Pos2::new(100., 100.),
                Pos2::new(200., 300.),
                Pos2::new(300., 110.),
            ],
            // Doubling back over itself.
            vec![
                Pos2::new(100., 100.),
                Pos2::new(300., 100.),
                Pos2::new(110., 105.),
            ],
        ] {
            let edges = outline(&path, r, 2.);
            assert!(!edges.is_empty());
            for p in edges.iter().flatten() {
                let d = distance_to_path(*p, &path);
                assert!((d - r).abs() < 1., "{p:?} is {d} from the path, not {r}");
            }
        }
    }

    #[test]
    fn a_stroke_folding_back_gets_its_true_edge() {
        let path = [
            Pos2::new(49.86, 464.05),
            Pos2::new(3.52, 224.11),
            Pos2::new(126.01, 30.38),
            Pos2::new(67.08, 274.34),
            Pos2::new(136.93, 58.93),
        ];
        let r = 84.09;
        for p in outline(&path, r, 2.).iter().flatten() {
            let d = distance_to_path(*p, &path);
            assert!((d - r).abs() < 1., "{p:?} is {d} from the path, not {r}");
        }
    }

    #[test]
    fn a_brush_thinner_than_the_grid_has_no_outline_to_draw() {
        assert!(outline(&[Pos2::new(0., 0.), Pos2::new(50., 0.)], 1., 2.).is_empty());
    }

    #[test]
    fn a_dense_scribble_takes_work_bounded_by_the_grid() {
        // Back and forth 4096 times over one place with a wide brush: still quick.
        let path: Vec<Pos2> = (0..4096)
            .map(|k| Pos2::new(if k % 2 == 0 { 100. } else { 400. }, 100. + (k % 7) as f32))
            .collect();
        let started = std::time::Instant::now();
        let edges = outline(&path, 250., 2.);
        assert!(!edges.is_empty());
        assert!(started.elapsed() < std::time::Duration::from_millis(500));
    }

    #[test]
    fn two_strokes_touching_diagonally_stay_one_shape() {
        // Two dots overlapping corner to corner, in both diagonal directions.
        for (a, b) in [
            (Pos2::new(100., 100.), Pos2::new(127., 127.)),
            (Pos2::new(127., 100.), Pos2::new(100., 127.)),
        ] {
            let edges = outline(&[a, b], 20., 2.);
            let mid = a + (b - a) * 0.5;
            // No edge crosses the middle of the joined shape.
            assert!(edges.iter().flatten().all(|p| p.distance(mid) > 5.));
        }
    }

    #[test]
    fn a_stroke_inside_another_part_leaves_no_inner_edge() {
        // Back and forth along one line: one outline around both passes.
        let path = [
            Pos2::new(100., 100.),
            Pos2::new(200., 100.),
            Pos2::new(105., 100.),
        ];
        let edges = outline(&path, 20., 2.);
        assert!(
            edges
                .iter()
                .flatten()
                .all(|p| distance_to_path(*p, &path) > 19.)
        );
    }
}
