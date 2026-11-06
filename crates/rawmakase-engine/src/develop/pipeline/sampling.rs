//! Sampling the camera image: the source, previews and footprint sampling.
use super::*;

/// Camera pixels as the pipeline samples them, with the copies the photo is measured on.
#[derive(Clone, Copy)]
pub(crate) struct Source<'a> {
    pub(super) image: &'a CameraImage,
    /// The image before the measured Texture, which the photo's measures leave out.
    pub(crate) untextured: Option<&'a CameraImage>,
    /// The photo's measurement copy (see [`Toned::measured`]).
    pub(crate) measured: Option<&'a std::sync::Arc<CameraImage>>,
    /// Its copy without retouching (see [`Toned::unretouched`]).
    pub(crate) unretouched: Option<&'a std::sync::Arc<CameraImage>>,
    /// Where the measures of that copy are kept (see [`Toned::measures`]).
    pub(crate) measures: Option<&'a crate::develop::stage_cache::MeasuresCache>,
}
impl<'a> Source<'a> {
    pub(crate) fn new(image: &'a CameraImage) -> Self {
        Self {
            image,
            untextured: None,
            measured: None,
            unretouched: None,
            measures: None,
        }
    }
    pub(super) fn px(&self, i: usize) -> [f32; 3] {
        self.image.pixels[i]
    }
}
impl std::ops::Deref for Source<'_> {
    type Target = CameraImage;
    fn deref(&self) -> &CameraImage {
        self.image
    }
}
impl<'a> From<&'a CameraImage> for Source<'a> {
    fn from(image: &'a CameraImage) -> Self {
        Self::new(image)
    }
}
/// A camera image and its copies, as the pixel stages take them.
pub(crate) struct Toned {
    pub(crate) image: std::sync::Arc<CameraImage>,
    /// The image's size relative to the full-resolution photo.
    pub(crate) scale: f32,
    /// `image` before the measured Texture, when it has it.
    pub(crate) untextured: Option<std::sync::Arc<CameraImage>>,
    /// The full-resolution photo (recovered and retouched, without Texture and Clarity)
    /// reduced for measuring it, whatever resolution `image` has, so Fit previews,
    /// regions and exports measure the same pixels.
    pub(crate) measured: Option<std::sync::Arc<CameraImage>>,
    /// `measured` without red eye corrections and spot removal (the same copy without
    /// them), for the photo's darkest level (`PhotoMeasures::dark`).
    pub(crate) unretouched: Option<std::sync::Arc<CameraImage>>,
    /// The stage cache's measures of `measured`, so slider edits that do not change
    /// them reuse them; without it each render measures the photo.
    pub(crate) measures: Option<std::sync::Arc<crate::develop::stage_cache::MeasuresCache>>,
}
impl Toned {
    pub(crate) fn source(&self) -> Source<'_> {
        Source {
            untextured: self.untextured.as_deref(),
            measured: self.measured.as_ref(),
            unretouched: self.unretouched.as_ref(),
            measures: self.measures.as_deref(),
            ..Source::new(&self.image)
        }
    }
}
pub(super) fn sample(im: Source, x: f32, y: f32) -> [f32; 3] {
    let x = x.clamp(0., (im.width - 1) as f32);
    let y = y.clamp(0., (im.height - 1) as f32);
    let ix = x as u32;
    let iy = y as u32;
    let fx = x - ix as f32;
    let fy = y - iy as f32;
    let at =
        |x: u32, y: u32| im.px((y.min(im.height - 1) * im.width + x.min(im.width - 1)) as usize);
    let (a, b, c, d) = (
        at(ix, iy),
        at(ix + 1, iy),
        at(ix, iy + 1),
        at(ix + 1, iy + 1),
    );
    std::array::from_fn(|i| {
        (a[i] * (1. - fx) + b[i] * fx) * (1. - fy) + (c[i] * (1. - fx) + d[i] * fx) * fy
    })
}
/// The photo reduced to `max` for previews. An HDR file's light is reduced with it,
/// over the same footprints, or shared when the photo is small enough already: the
/// one derived image that keeps it (docs/records/hdr-stage4-design.md).
pub fn preview(im: &CameraImage, max: u32) -> CameraImage {
    let copy = preview_source(im.into(), max);
    let Some(light) = im.hdr() else {
        return copy;
    };
    if (copy.width, copy.height) == (im.width, im.height) {
        return copy.shares_hdr(im);
    }
    let (w, h) = (copy.width, copy.height);
    let reduced = light.reduced(w, h, |p| {
        box_reduce(light.width(), light.height(), w, h, |i| p[i])
    });
    match reduced.and_then(|light| copy.clone().with_hdr(std::sync::Arc::new(light))) {
        Some(paired) => paired,
        None => unreachable!("a light reduced to its preview's size pairs with it"),
    }
}
/// The photo reduced to `max` for measuring it. A DNG's OpcodeList3 GainMap (its lens
/// shading) is part of the photo as Camera Raw measures it, so the copy carries it in
/// full: the scene tone stage's measures, the haze and the Shadows/Highlights and
/// Clarity maps read the shading the render corrects. (The render applies it at the
/// lens Vignetting amount and with the built-in correction on; the copy, kept per
/// image, does not follow either.)
pub(crate) fn measurement_source(im: Source, max: u32) -> CameraImage {
    let mut copy = preview_source(im, max);
    if let Some(grid) = copy.metadata.lens.as_ref().and_then(|l| l.gain_map.clone()) {
        let (w, h) = (copy.width as usize, copy.height as usize);
        copy.pixels.par_iter_mut().enumerate().for_each(|(i, p)| {
            let gain = grid.gain(
                ((i / w) as f32 + 0.5) / h as f32,
                ((i % w) as f32 + 0.5) / w as f32,
            );
            (0..3).for_each(|c| p[c] *= gain[c]);
        });
    }
    copy
}
pub(crate) fn preview_source(im: Source, max: u32) -> CameraImage {
    if im.width.max(im.height) <= max {
        return im.image.clone();
    }
    let scale = max as f32 / im.width.max(im.height) as f32;
    let w = (im.width as f32 * scale).round() as u32;
    let h = (im.height as f32 * scale).round() as u32;
    let pixels = box_reduce(im.width, im.height, w, h, |i| im.px(i));
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
/// A `width` × `height` image whose pixel `i` is `px(i)` reduced to `w` × `h` by box
/// integration, which keeps fine detail from aliasing.
fn box_reduce(
    width: u32,
    height: u32,
    w: u32,
    h: u32,
    px: impl Fn(usize) -> [f32; 3] + Sync,
) -> Vec<[f32; 3]> {
    let mut pixels = vec![[0.; 3]; w as usize * h as usize];
    pixels.par_iter_mut().enumerate().for_each(|(i, out)| {
        let x = i as u32 % w;
        let y = i as u32 / w;
        let x0 = x * width / w;
        let x1 = ((x + 1) * width / w).max(x0 + 1);
        let y0 = y * height / h;
        let y1 = ((y + 1) * height / h).max(y0 + 1);
        for yy in y0..y1 {
            for xx in x0..x1 {
                let p = px((yy * width + xx) as usize);
                for c in 0..3 {
                    out[c] += p[c];
                }
            }
        }
        let n = ((x1 - x0) * (y1 - y0)) as f32;
        for v in out {
            *v /= n;
        }
    });
    pixels
}

/// A detail sample averaged over an output pixel's footprint: four taps at ±`spread`
/// source pixels, which with bilinear sampling approximate a box filter.
pub(super) fn footprint_sample(im: Source, x: f32, y: f32, spread: f32) -> [f32; 3] {
    if spread == 0. {
        return sample(im, x, y);
    }
    let mut sum = [0.; 3];
    for (dx, dy) in [(-1., -1.), (1., -1.), (-1., 1.), (1., 1.)] {
        let p = sample(im, x + dx * spread, y + dy * spread);
        for c in 0..3 {
            sum[c] += p[c] * 0.25;
        }
    }
    sum
}
/// Tap offset for [`footprint_sample`] when one output pixel covers `footprint`
/// source pixels: the four taps add the variance a box of that width has beyond a
/// single bilinear sample's.
pub(crate) fn footprint_spread(footprint: f32) -> f32 {
    (footprint * footprint / 12. - 1. / 6.).max(0.).sqrt()
}
