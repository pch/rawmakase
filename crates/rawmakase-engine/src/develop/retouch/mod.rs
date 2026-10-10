//! Lightroom's Remove panel in Heal and Clone modes: spots and brushed areas that copy
//! pixels from another part of the photo. Operations are stored as parameters and
//! rendered on the linear camera image before everything else, so they follow every
//! later edit and export at full quality. The operations themselves are
//! [`crate::model::retouch`]'s.
//!
//! Positions are in image space (see [`crate::model::image_frame::ImageFrame`]): normalised to the
//! oriented photo before lens correction, Transform, crop and straightening. Sizes are
//! fractions of the photo's long edge.
mod heal;
mod layer;
mod search;

use crate::model::retouch::RetouchOp;

pub(crate) use heal::FeatherProfile;
pub(crate) use heal::profile;
pub(crate) use layer::{RetouchCache, Retouching, apply};
pub use search::find_source;

#[cfg(test)]
mod tests;

/// Lightroom's Visualize Spots: a black-and-white view of fine luminance detail in a
/// rendered preview, where dust and small blemishes stand out. `threshold` (0–1) is
/// the panel's slider: higher shows fainter detail. Returns 8-bit RGB.
pub fn visualize_spots(image: &crate::rendered::Rendered, threshold: f32) -> Vec<u8> {
    let (w, h) = (image.width as usize, image.height as usize);
    let lum: Vec<f32> = image
        .pixels
        .iter()
        .map(|p| crate::color::luminance(*p))
        .collect();
    // Detail = luminance minus its 5×5 mean, from running sums.
    let r = 2usize;
    let mut rows = vec![0.; w * h];
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (x.saturating_sub(r), (x + r + 1).min(w));
            rows[y * w + x] = lum[y * w + a..y * w + b].iter().sum::<f32>() / (b - a) as f32;
        }
    }
    let gain = 4. * 2f32.powf(threshold.clamp(0., 1.) * 5.);
    let mut out = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        let (a, b) = (y.saturating_sub(r), (y + r + 1).min(h));
        for x in 0..w {
            let mean = (a..b).map(|yy| rows[yy * w + x]).sum::<f32>() / (b - a) as f32;
            let v = ((lum[y * w + x] - mean).abs() * gain).min(1.);
            let v = (255. * v.sqrt()) as u8;
            out.extend([v, v, v]);
        }
    }
    out
}
