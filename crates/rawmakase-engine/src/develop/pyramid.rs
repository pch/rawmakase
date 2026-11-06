//! Resolution pyramid of the highlight-recovered camera image, for Fit and zoomed-out
//! previews. Level 0 is the recovered image; each further level halves both sides with
//! a 2×2 box average in linear camera space. Levels are built on first use and kept
//! for the photo, so zooming and slider edits reuse them.
use crate::camera_data::CameraImage;
use rayon::prelude::*;
use std::sync::Arc;

/// No level is made smaller than this long edge.
const MIN_EDGE: u32 = 64;

pub(crate) struct Pyramid {
    levels: Vec<Arc<CameraImage>>,
}
impl Pyramid {
    pub(crate) fn new(recovered: Arc<CameraImage>) -> Self {
        Self {
            levels: vec![recovered],
        }
    }
    pub(crate) fn source(&self) -> &Arc<CameraImage> {
        &self.levels[0]
    }
    /// Replaces level 0 with `source`, which differs from it only in `rects` (level-0
    /// pixels), recomputing just those areas of the levels built so far.
    pub(crate) fn update(&mut self, source: Arc<CameraImage>, rects: &[[i32; 4]]) {
        let mut rects: Vec<[i32; 4]> = rects.to_vec();
        self.levels[0] = source;
        for k in 1..self.levels.len() {
            // A level pixel averages a 2×2 block of the level above.
            rects = rects
                .iter()
                .map(|r| [r[0] / 2, r[1] / 2, (r[2] + 1) / 2, (r[3] + 1) / 2])
                .collect();
            let above = self.levels[k - 1].clone();
            let mut level = CameraImage::clone(&self.levels[k]);
            for r in &rects {
                halve_into(&above, &mut level, *r);
            }
            self.levels[k] = Arc::new(level);
        }
    }
    /// The smallest level whose long edge, scaled by `needed / full long edge`, still
    /// covers `needed`: that is, the level with at least one pixel per output pixel.
    pub(crate) fn level_for(&mut self, needed: f32) -> Arc<CameraImage> {
        let full = self.levels[0].width.max(self.levels[0].height);
        let mut k = 0;
        while full.div_ceil(2 << k) >= MIN_EDGE && full.div_ceil(2 << k) as f32 >= needed {
            k += 1;
        }
        while self.levels.len() <= k {
            let next = halve(self.levels.last().unwrap());
            self.levels.push(Arc::new(next));
        }
        self.levels[k].clone()
    }
}
/// Recomputes `rect` of `level` from the level above it.
fn halve_into(above: &CameraImage, level: &mut CameraImage, rect: [i32; 4]) {
    let (w, h) = (level.width as i32, level.height as i32);
    for y in rect[1].max(0)..rect[3].min(h) {
        let y0 = 2 * y as u32;
        let y1 = (y0 + 1).min(above.height - 1);
        for x in rect[0].max(0)..rect[2].min(w) {
            let x0 = 2 * x as u32;
            let x1 = (x0 + 1).min(above.width - 1);
            let at = |x: u32, y: u32| above.pixels[(y * above.width + x) as usize];
            let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
            level.pixels[(y * w + x) as usize] =
                std::array::from_fn(|i| (a[i] + b[i] + c[i] + d[i]) * 0.25);
        }
    }
}
/// 2×2 box average; an odd last row or column averages with itself.
fn halve(im: &CameraImage) -> CameraImage {
    let (w, h) = (im.width.div_ceil(2), im.height.div_ceil(2));
    let mut pixels = vec![[0.; 3]; w as usize * h as usize];
    pixels
        .par_chunks_mut(w as usize)
        .enumerate()
        .for_each(|(y, row)| {
            let y0 = 2 * y as u32;
            let y1 = (y0 + 1).min(im.height - 1);
            for (x, out) in row.iter_mut().enumerate() {
                let x0 = 2 * x as u32;
                let x1 = (x0 + 1).min(im.width - 1);
                let at = |x: u32, y: u32| im.pixels[(y * im.width + x) as usize];
                let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
                *out = std::array::from_fn(|i| (a[i] + b[i] + c[i] + d[i]) * 0.25);
            }
        });
    CameraImage {
        recovered: Default::default(),
        width: w,
        height: h,
        pixels,
        metadata: im.metadata.clone(),
        fast: im.fast,
        scale_factor: im.scale_factor,
        scale_clipped: im.scale_clipped,
        hdr: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn image(width: u32, height: u32) -> CameraImage {
        CameraImage {
            recovered: Default::default(),
            width,
            height,
            pixels: (0..width * height)
                .map(|i| [(i % width) as f32, (i / width) as f32, 1.])
                .collect(),
            metadata: Default::default(),
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
        }
    }
    #[test]
    fn levels_halve_and_stop_at_the_needed_size() {
        let mut p = Pyramid::new(Arc::new(image(1001, 600)));
        assert_eq!(p.level_for(2000.).width, 1001);
        assert_eq!(p.level_for(1001.).width, 1001);
        let half = p.level_for(400.);
        assert_eq!((half.width, half.height), (501, 300));
        // Averages keep the mean position and the odd edge column.
        assert_eq!(half.pixels[0], [0.5, 0.5, 1.]);
        assert_eq!(half.pixels[500], [1000., 0.5, 1.]);
        assert_eq!(p.level_for(250.).width, 251);
        assert_eq!(p.level_for(1.).width, 126);
        assert_eq!(p.levels.len(), 4);
        // Updating a rectangle of level 0 gives the pyramid of the new image.
        let mut changed = image(1001, 600);
        for y in 100..140 {
            for x in 700..760 {
                changed.pixels[y * 1001 + x] = [5., 6., 7.];
            }
        }
        p.update(Arc::new(changed.clone()), &[[700, 100, 760, 140]]);
        let mut fresh = Pyramid::new(Arc::new(changed));
        fresh.level_for(1.);
        for k in 0..4 {
            assert_eq!(p.levels[k].pixels, fresh.levels[k].pixels, "level {k}");
        }
    }
}
