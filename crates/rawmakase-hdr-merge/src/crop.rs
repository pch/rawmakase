//! The merge's starting crop: what Auto Align left uncovered cut away, as
//! Lightroom's merges start.

/// Cells on the long edge the search works on: a cell is uncovered when any of
/// its pixels is, so the crop found never reaches an uncovered pixel and
/// loses at most a cell to that rounding.
const CELLS: u32 = 1024;

/// The largest crop of `region`'s aspect wholly inside the covered pixels of
/// `coverage` (row-major, `width` wide), as near the region's centre as it
/// fits: `[left, top, right, bottom]` in 0–1 of `region` (`[x, y, width,
/// height]` in pixels; the DNG's default crop), as a recipe's crop is. `None`
/// when the whole region is covered, or no crop of 1% of it fits.
pub(crate) fn covered(coverage: &[bool], width: u32, region: [u32; 4]) -> Option<[f32; 4]> {
    let [x0, y0, rw, rh] = region.map(|v| v as usize);
    let width = width as usize;
    if rw == 0 || rh == 0 {
        return None;
    }
    let step = rw.max(rh).div_ceil(CELLS as usize).max(1);
    let (cw, ch) = (rw.div_ceil(step), rh.div_ceil(step));
    let mut open = vec![false; cw * ch];
    for y in 0..rh {
        let row = &coverage[(y0 + y) * width + x0..][..rw];
        for (x, covered) in row.iter().enumerate() {
            if !covered {
                open[(y / step) * cw + x / step] = true;
            }
        }
    }
    if !open.contains(&true) {
        return None;
    }
    // Uncovered cells above and left of each corner.
    let mut sum = vec![0u32; (cw + 1) * (ch + 1)];
    for y in 0..ch {
        let mut row = 0;
        for x in 0..cw {
            row += u32::from(open[y * cw + x]);
            sum[(y + 1) * (cw + 1) + x + 1] = sum[y * (cw + 1) + x + 1] + row;
        }
    }
    let clear = |x: usize, y: usize, w: usize, h: usize| {
        let at = |x: usize, y: usize| sum[y * (cw + 1) + x];
        at(x + w, y + h) + at(x, y) == at(x + w, y) + at(x, y + h)
    };
    let aspect = rw as f64 / rh as f64;
    let cells_wide = |h: usize| ((h as f64 * aspect).floor() as usize).min(cw);
    // The clear place for `h` cells high nearest the centre, if any.
    let place = |h: usize| -> Option<(usize, usize)> {
        let w = cells_wide(h);
        if w == 0 {
            return None;
        }
        let mut best: Option<((usize, usize), f64)> = None;
        for y in 0..=ch - h {
            for x in 0..=cw - w {
                if !clear(x, y, w, h) {
                    continue;
                }
                let dx = (x as f64 + w as f64 / 2.) - cw as f64 / 2.;
                let dy = (y as f64 + h as f64 / 2.) - ch as f64 / 2.;
                let d = dx * dx + dy * dy;
                if best.is_none_or(|(_, b)| d < b) {
                    best = Some(((x, y), d));
                }
            }
        }
        best.map(|(at, _)| at)
    };
    // A crop that fits leaves room for every smaller one.
    let (mut lo, mut hi) = (0, ch);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if place(mid).is_some() {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let (cx, cy) = place(lo)?;
    // The cells in pixels, then the region's exact aspect centred in them.
    let left = (cx * step) as f64;
    let top = (cy * step) as f64;
    let right = (((cx + cells_wide(lo)) * step).min(rw)) as f64;
    let bottom = (((cy + lo) * step).min(rh)) as f64;
    let (mut w, mut h) = (right - left, bottom - top);
    if w / h > aspect {
        w = h * aspect;
    } else {
        h = w / aspect;
    }
    if w < 0.01 * rw as f64 || h < 0.01 * rh as f64 {
        return None;
    }
    let (mx, my) = ((left + right) / 2., (top + bottom) / 2.);
    Some([
        ((mx - w / 2.) / rw as f64) as f32,
        ((my - h / 2.) / rh as f64) as f32,
        ((mx + w / 2.) / rw as f64) as f32,
        ((my + h / 2.) / rh as f64) as f32,
    ])
}

#[cfg(test)]
mod tests {
    use super::covered;

    /// A `w` × `h` coverage, covered where `inside` says.
    fn coverage(w: usize, h: usize, inside: impl Fn(usize, usize) -> bool) -> Vec<bool> {
        (0..w * h).map(|i| inside(i % w, i / w)).collect()
    }

    /// Pixels of a crop, in `region`.
    fn pixels(crop: [f32; 4], region: [u32; 4]) -> [f64; 4] {
        let [x, y, w, h] = region.map(f64::from);
        let c = crop.map(f64::from);
        [x + c[0] * w, y + c[1] * h, x + c[2] * w, y + c[3] * h]
    }

    /// Every pixel of the crop is covered.
    fn all_covered(cov: &[bool], width: usize, px: [f64; 4]) -> bool {
        let [l, t, r, b] = px;
        (t.floor() as usize..b.ceil() as usize)
            .all(|y| (l.floor() as usize..r.ceil() as usize).all(|x| cov[y * width + x]))
    }

    #[test]
    fn a_whole_coverage_needs_no_crop() {
        let cov = coverage(300, 200, |_, _| true);
        assert_eq!(covered(&cov, 300, [0, 0, 300, 200]), None);
        // Nor does one uncovered only outside the region.
        let cov = coverage(300, 200, |x, _| x >= 10);
        assert_eq!(covered(&cov, 300, [10, 0, 290, 200]), None);
    }

    /// A frame shifted 40 px right and 20 px down leaves an L uncovered:
    /// the crop keeps the aspect, misses the L and stays near the centre.
    #[test]
    fn an_uncovered_edge_is_cut_keeping_the_aspect() {
        let (w, h) = (3000, 2000);
        let cov = coverage(w, h, |x, y| x >= 40 && y >= 20);
        let region = [0, 0, w as u32, h as u32];
        let crop = covered(&cov, w as u32, region).unwrap();
        let px = pixels(crop, region);
        assert!(all_covered(&cov, w, px), "{px:?}");
        let (cw, ch) = (px[2] - px[0], px[3] - px[1]);
        assert!((cw / ch - 1.5).abs() < 1e-4, "{cw} × {ch}");
        // Within a cell (3 px here) of the largest, which the width bounds:
        // 2960 wide, 1973.3 high.
        assert!(ch > 2960. / 1.5 - 2. * 3., "{ch}");
        assert!(px[1] >= 20. && px[0] >= 40.);
    }

    /// A turned frame leaves triangles in the corners; the crop stays clear of
    /// them, and of a default crop's margin.
    #[test]
    fn a_turned_frame_and_a_default_crop() {
        let (w, h) = (1200, 800);
        let (s, c) = 2f64.to_radians().sin_cos();
        let inside = |x: usize, y: usize| {
            let (dx, dy) = (x as f64 - 600., y as f64 - 400.);
            let (u, v) = (c * dx + s * dy, -s * dx + c * dy);
            u.abs() < 600. && v.abs() < 400.
        };
        let cov = coverage(w, h, inside);
        let region = [8, 6, 1184, 788];
        let crop = covered(&cov, w as u32, region).unwrap();
        let px = pixels(crop, region);
        assert!(all_covered(&cov, w, px), "{px:?}");
        assert!(px[0] >= 8. && px[1] >= 6. && px[2] <= 1192. && px[3] <= 794.);
        let (cw, ch) = (px[2] - px[0], px[3] - px[1]);
        assert!((cw / ch - 1184. / 788.).abs() < 1e-4);
        // Close to centred.
        assert!(((px[0] + px[2]) / 2. - 600.).abs() < 3.);
        assert!(((px[1] + px[3]) / 2. - 400.).abs() < 3.);
    }

    #[test]
    fn nothing_covered_is_no_crop() {
        let cov = coverage(300, 300, |x, y| x == 150 && y == 150);
        assert_eq!(covered(&cov, 300, [0, 0, 300, 300]), None);
    }
}
