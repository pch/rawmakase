//! Detail: sharpening and the noise reduction masks apply.
use super::*;
use rawmakase_cancel::Cancel;

#[cfg(test)]
pub(in crate::develop) fn sharpen(
    im: &mut Rendered,
    r: &Recipe,
    local: Option<&MaskWeights>,
    cancel: Cancel<'_>,
) -> Result<()> {
    sharpen_with_radius(
        im,
        r,
        Sharpener::new(r).sigma,
        local,
        crate::color::OutputSpace::Srgb,
        cancel,
    )
}
/// [`sharpen`] with the blur's `sigma` given, for checking other paths against.
#[cfg(test)]
pub(in crate::develop) fn sharpen_at(im: &mut Rendered, r: &Recipe, sigma: f32) -> Result<()> {
    let srgb = crate::color::OutputSpace::Srgb;
    sharpen_with_radius(im, r, sigma, None, srgb, Cancel::NEVER)
}
/// Normalized Gaussian taps. Below half a pixel, which only scaled previews use, a
/// sampled Gaussian degenerates to a single tap; three taps with the same variance
/// keep the sharpening response of the full-resolution render.
pub(crate) fn gaussian(sigma: f32) -> (i32, Vec<f32>) {
    if sigma < 0.5 {
        let side = sigma * sigma / 2.;
        return (1, vec![side, 1. - 2. * side, side]);
    }
    let radius = (sigma * 3.).ceil() as i32;
    let weights: Vec<f32> = (-radius..=radius)
        .map(|x| (-0.5 * (x as f32 / sigma).powi(2)).exp())
        .collect();
    let sum: f32 = weights.iter().sum();
    (radius, weights.into_iter().map(|x| x / sum).collect())
}
/// Sharpening; masks' Sharpness adds to the amount per pixel, and below zero softens.
/// The detail is that of luminance with the primaries of `space`, the one `im` is
/// encoded in.
pub(super) fn sharpen_with_radius(
    im: &mut Rendered,
    r: &Recipe,
    sigma: f32,
    local: Option<&MaskWeights>,
    space: crate::color::OutputSpace,
    cancel: Cancel<'_>,
) -> Result<()> {
    sharpen_in(im, r, sigma, local, space, |v| v.clamp(0., 1.), cancel)
}
/// [`sharpen_with_radius`] of an HDR master's encoded values ([`super::hdr`]): nothing
/// is clipped at white, and values stay at or above black.
pub(super) fn sharpen_unbounded(
    im: &mut Rendered,
    r: &Recipe,
    sigma: f32,
    local: Option<&MaskWeights>,
    space: crate::color::OutputSpace,
    cancel: Cancel<'_>,
) -> Result<()> {
    sharpen_in(im, r, sigma, local, space, |v| v.max(0.), cancel)
}
fn sharpen_in(
    im: &mut Rendered,
    r: &Recipe,
    sigma: f32,
    local: Option<&MaskWeights>,
    space: crate::color::OutputSpace,
    bound: impl Fn(f32) -> f32 + Sync,
    cancel: Cancel<'_>,
) -> Result<()> {
    check_cancel(cancel)?;
    let local = local.filter(|w| w.uses(&[slot::SHARPNESS]));
    if r.sharpening == 0. && local.is_none() {
        return Ok(());
    }
    let sharpener = Sharpener::new(r);
    let (radius, weights) = gaussian(sigma);
    let lum: Vec<f32> = im.pixels.par_iter().map(|p| space.luminance(*p)).collect();
    let w = im.width as usize;
    let h = im.height as usize;
    if w == 0 || h == 0 {
        return Ok(());
    }
    let r_usize = radius as usize;
    let mut horizontal = vec![0.; lum.len()];
    horizontal
        .par_chunks_mut(w)
        .zip(lum.par_chunks(w))
        .for_each(|(dst_row, src_row)| {
            if cancel.is_cancelled() {
                return;
            }
            let interior_end = w.saturating_sub(r_usize);
            for (x, p) in dst_row.iter_mut().enumerate() {
                let mut sum = 0.;
                if x >= r_usize && x < interior_end {
                    let win = &src_row[x - r_usize..x + r_usize + 1];
                    for (&src, &weight) in win.iter().zip(&weights) {
                        sum += src * weight;
                    }
                } else {
                    for (k, &weight) in weights.iter().enumerate() {
                        let xx = (x as i32 + k as i32 - radius).clamp(0, w as i32 - 1) as usize;
                        sum += src_row[xx] * weight;
                    }
                }
                *p = sum;
            }
        });
    im.pixels.par_chunks_mut(w).enumerate().for_each_init(
        || vec![0f32; w],
        |blur_row, (y, dst_row)| {
            if cancel.is_cancelled() {
                return;
            }
            blur_row.fill(0.);
            for (k, &weight) in weights.iter().enumerate() {
                let yy = (y as i32 + k as i32 - radius).clamp(0, h as i32 - 1) as usize;
                let src_row = &horizontal[yy * w..(yy + 1) * w];
                for (b, &h_val) in blur_row.iter_mut().zip(src_row) {
                    *b += h_val * weight;
                }
            }
            let row_start = y * w;
            let lum_row = &lum[row_start..row_start + w];
            for (x, p) in dst_row.iter_mut().enumerate() {
                let i = row_start + x;
                let d = lum_row[x] - blur_row[x];
                let amount = r.sharpening
                    + local
                        .and_then(|w| w.delta(i))
                        .map_or(0., |d| d[slot::SHARPNESS]);
                let delta = sharpener.delta(d, amount);
                // Add only luminance detail, preserving inter-channel differences.
                for v in p {
                    *v = bound(*v + delta);
                }
            }
        },
    );
    check_cancel(cancel)
}
/// Masks' Noise: an edge-aware 5×5 average blended in by the pixel's amount (0–1).
/// Negative values have no effect. Edges are told by luminance with the primaries of
/// `space`, the one `im` is encoded in.
pub(super) fn local_noise(
    im: &mut Rendered,
    local: Option<&MaskWeights>,
    space: crate::color::OutputSpace,
) {
    let Some(weights) = local.filter(|w| w.uses(&[slot::NOISE])) else {
        return;
    };
    let src = im.pixels.clone();
    let (w, h) = (im.width as i32, im.height as i32);
    im.pixels.par_iter_mut().enumerate().for_each(|(i, p)| {
        let Some(amount) = weights
            .delta(i)
            .map(|d| d[slot::NOISE].clamp(0., 1.))
            .filter(|a| *a > 0.)
        else {
            return;
        };
        let (x, y) = (i as i32 % w, i as i32 / w);
        let center = space.luminance(src[i]);
        let (mut sum, mut total) = ([0.; 3], 0.);
        for dy in -2..=2 {
            for dx in -2..=2 {
                let q = src[((y + dy).clamp(0, h - 1) * w + (x + dx).clamp(0, w - 1)) as usize];
                let k = 1. / (1. + (space.luminance(q) - center).powi(2) / 0.0004);
                for c in 0..3 {
                    sum[c] += q[c] * k;
                }
                total += k;
            }
        }
        for c in 0..3 {
            p[c] += (sum[c] / total - p[c]) * amount;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naive_sharpen(im: &Rendered, r: &Recipe, sigma: f32) -> Rendered {
        let space = crate::color::OutputSpace::Srgb;
        let sharpener = Sharpener::new(r);
        let (radius, weights) = gaussian(sigma);
        let lum: Vec<f32> = im.pixels.iter().map(|p| space.luminance(*p)).collect();
        let (w, h) = (im.width as usize, im.height as usize);
        let mut horizontal = vec![0f32; lum.len()];
        for (i, p) in horizontal.iter_mut().enumerate() {
            let (x, y) = (i % w, i / w);
            for (k, weight) in weights.iter().enumerate() {
                let xx = (x as i32 + k as i32 - radius).clamp(0, w as i32 - 1) as usize;
                *p += lum[y * w + xx] * weight;
            }
        }
        let mut out = im.clone();
        for (i, p) in out.pixels.iter_mut().enumerate() {
            let (x, y) = (i % w, i / w);
            let mut blur = 0f32;
            for (k, weight) in weights.iter().enumerate() {
                let yy = (y as i32 + k as i32 - radius).clamp(0, h as i32 - 1) as usize;
                blur += horizontal[yy * w + x] * weight;
            }
            let delta = sharpener.delta(lum[i] - blur, r.sharpening);
            for v in p {
                *v = (*v + delta).clamp(0., 1.);
            }
        }
        out
    }

    #[test]
    fn row_chunked_sharpen_is_bit_exact_with_per_pixel_reference() {
        let r = Recipe {
            sharpening: 0.8,
            ..Default::default()
        };
        for &(w, h) in &[(1u32, 1u32), (2, 3), (3, 3), (17, 13), (64, 48)] {
            let pixels: Vec<[f32; 3]> = (0..(w * h) as usize)
                .map(|i| {
                    [
                        ((i * 17) % 101) as f32 / 100.,
                        ((i * 31 + 7) % 97) as f32 / 96.,
                        ((i * 53 + 13) % 89) as f32 / 88.,
                    ]
                })
                .collect();
            let base = Rendered {
                width: w,
                height: h,
                pixels,
            };
            for sigma in [0.3, 1.0, 2.5] {
                let expected = naive_sharpen(&base, &r, sigma);
                let mut actual = base.clone();
                sharpen_at(&mut actual, &r, sigma).unwrap();
                for (idx, (a, b)) in actual.pixels.iter().zip(&expected.pixels).enumerate() {
                    for c in 0..3 {
                        assert_eq!(
                            a[c].to_bits(),
                            b[c].to_bits(),
                            "{w}x{h} sigma={sigma} pixel {idx} ch {c}: {} vs {}",
                            a[c],
                            b[c]
                        );
                    }
                }
            }
        }
    }
}
