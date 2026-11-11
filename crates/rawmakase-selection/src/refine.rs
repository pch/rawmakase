//! Edge refinement of a coverage matte against the photo it was made from.
//!
//! The network's matte is a soft ramp that is wider than the real edge, so a
//! local adjustment through it lights a halo around the subject. A guided filter
//! (He et al., 2010) moves the ramp onto the photo's own edges, and a gentle
//! contrast curve then narrows what is left. Both are local, deterministic and
//! take a few tens of milliseconds at the sizes used here.
use crate::process::{Coverage, RgbImage};

/// Moves `coverage`'s edges onto `image`'s. Does nothing when they differ in size.
pub fn refine(coverage: &mut Coverage, image: &RgbImage) {
    let (w, h) = (coverage.width, coverage.height);
    if (image.width, image.height) != (w, h) || w == 0 || h == 0 {
        return;
    }
    let radius = (w.max(h) / 160).clamp(3, 24);
    let guide: Vec<f32> = image
        .data
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.)
        .collect();
    let p: Vec<f32> = coverage.data.iter().map(|v| *v as f32 / 255.).collect();
    let q = guided(&guide, &p, w, h, radius, 2e-3);
    for (out, v) in coverage.data.iter_mut().zip(q) {
        *out = (smooth(v) * 255. + 0.5) as u8;
    }
}

/// Narrows the soft ramp: below 0.25 is nothing, above 0.75 is everything.
pub(crate) fn smooth(v: f32) -> f32 {
    let t = ((v - 0.25) / 0.5).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

/// Mean over the `(2r+1)²` window around every pixel, clamped at the edges.
pub(crate) fn mean(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    // Horizontal running sums, then vertical.
    let mut tmp = vec![0f32; w * h];
    for y in 0..h {
        let row = &src[y * w..(y + 1) * w];
        let mut prefix = vec![0f64; w + 1];
        for x in 0..w {
            prefix[x + 1] = prefix[x] + row[x] as f64;
        }
        for x in 0..w {
            let (a, b) = (x.saturating_sub(r), (x + r + 1).min(w));
            tmp[y * w + x] = ((prefix[b] - prefix[a]) / (b - a) as f64) as f32;
        }
    }
    let mut out = vec![0f32; w * h];
    for x in 0..w {
        let mut prefix = vec![0f64; h + 1];
        for y in 0..h {
            prefix[y + 1] = prefix[y] + tmp[y * w + x] as f64;
        }
        for y in 0..h {
            let (a, b) = (y.saturating_sub(r), (y + r + 1).min(h));
            out[y * w + x] = ((prefix[b] - prefix[a]) / (b - a) as f64) as f32;
        }
    }
    out
}

pub(crate) fn guided(i: &[f32], p: &[f32], w: usize, h: usize, r: usize, eps: f32) -> Vec<f32> {
    let n = w * h;
    let mean_i = mean(i, w, h, r);
    let mean_p = mean(p, w, h, r);
    let ip: Vec<f32> = (0..n).map(|k| i[k] * p[k]).collect();
    let ii: Vec<f32> = (0..n).map(|k| i[k] * i[k]).collect();
    let mean_ip = mean(&ip, w, h, r);
    let mean_ii = mean(&ii, w, h, r);
    let mut a = vec![0f32; n];
    let mut b = vec![0f32; n];
    for k in 0..n {
        let var = mean_ii[k] - mean_i[k] * mean_i[k];
        let cov = mean_ip[k] - mean_i[k] * mean_p[k];
        a[k] = cov / (var + eps);
        b[k] = mean_p[k] - a[k] * mean_i[k];
    }
    let mean_a = mean(&a, w, h, r);
    let mean_b = mean(&b, w, h, r);
    (0..n)
        .map(|k| (mean_a[k] * i[k] + mean_b[k]).clamp(0., 1.))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bright square on dark ground, and a matte whose ramp is three times too wide.
    fn scene() -> (RgbImage, Coverage) {
        let (w, h) = (96usize, 64usize);
        let mut data = vec![0u8; w * h * 3];
        let mut cov = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                let inside = (30..66).contains(&x) && (16..48).contains(&y);
                let v = if inside { 220 } else { 30 };
                data[(y * w + x) * 3..][..3].fill(v);
                // Distance outside the square, ramped over 12 pixels.
                let d = (30isize - x as isize)
                    .max(x as isize - 65)
                    .max(16 - y as isize)
                    .max(y as isize - 47);
                cov[y * w + x] = (255. * (1. - (d as f32 / 12. + 0.5)).clamp(0., 1.)) as u8;
            }
        }
        (
            RgbImage {
                width: w,
                height: h,
                data,
            },
            Coverage {
                width: w,
                height: h,
                data: cov,
            },
        )
    }

    #[test]
    fn a_wide_ramp_is_pulled_onto_the_photos_edge() {
        let (image, mut coverage) = scene();
        let before = coverage.data.clone();
        refine(&mut coverage, &image);
        let at = |c: &Coverage, x: usize, y: usize| c.data[y * c.width + x] as i32;
        // Two pixels outside the edge the halo is gone; inside stays selected.
        let halo = |data: &[u8]| data[32 * 96 + 28] as i32;
        assert!(halo(&before) > 70, "{}", halo(&before));
        assert!(halo(&coverage.data) < 45, "{}", halo(&coverage.data));
        assert!(at(&coverage, 48, 32) > 250);
        assert!(at(&coverage, 5, 5) < 5);
    }

    #[test]
    fn a_matte_of_another_size_is_left_alone() {
        let (image, mut coverage) = scene();
        coverage.width = 48;
        coverage.height = 128;
        let before = coverage.clone();
        refine(&mut coverage, &image);
        assert_eq!(coverage, before);
    }

    #[test]
    fn flat_mattes_stay_flat() {
        let (image, mut coverage) = scene();
        coverage.data.fill(255);
        refine(&mut coverage, &image);
        assert!(coverage.data.iter().all(|v| *v == 255));
        coverage.data.fill(0);
        refine(&mut coverage, &image);
        assert!(coverage.data.iter().all(|v| *v == 0));
    }
}
