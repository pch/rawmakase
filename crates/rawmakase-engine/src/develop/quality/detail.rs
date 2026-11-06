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
    space: crate::color::OutputSpace,
    cancel: Cancel<'_>,
) -> Result<()> {
    sharpen_in(im, r, sigma, None, space, |v| v.max(0.), cancel)
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
    let mut horizontal = vec![0.; lum.len()];
    horizontal.par_iter_mut().enumerate().for_each(|(i, p)| {
        if cancel.is_cancelled() {
            return;
        }
        let x = i % w;
        let y = i / w;
        for (k, weight) in weights.iter().enumerate() {
            let xx = (x as i32 + k as i32 - radius).clamp(0, w as i32 - 1) as usize;
            *p += lum[y * w + xx] * weight;
        }
    });
    im.pixels.par_iter_mut().enumerate().for_each(|(i, p)| {
        if cancel.is_cancelled() {
            return;
        }
        let x = i % w;
        let y = i / w;
        let mut blur = 0.;
        for (k, weight) in weights.iter().enumerate() {
            let yy = (y as i32 + k as i32 - radius).clamp(0, h as i32 - 1) as usize;
            blur += horizontal[yy * w + x] * weight;
        }
        let d = lum[i] - blur;
        let amount = r.sharpening
            + local
                .and_then(|w| w.delta(i))
                .map_or(0., |d| d[slot::SHARPNESS]);
        let delta = sharpener.delta(d, amount);
        // Add only luminance detail, preserving inter-channel differences.
        for v in p {
            *v = bound(*v + delta);
        }
    });
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
