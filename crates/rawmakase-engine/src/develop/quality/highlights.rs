//! Highlight reconstruction: Camera Raw 18.7's rule for bright and clipped
//! samples, measured on flat patches rendered through a linear profile
//! (`docs/rendering-quality.md`, "Partially clipped highlights").
//!
//! In white-balanced camera space every channel is pulled up towards the mean
//! of the other two, never down, by a weight that rises with that channel's own
//! sensor level `s` (its fraction of the sensor's white): `((s - 0.25) / 0.75)^1.8`,
//! so a clipped channel (at the white) becomes the mean of the other two
//! outright, a channel near the white mostly follows them, and a channel in the
//! lower quarter keeps its value. The pulls are solved together: each channel's
//! target is the others' pulled values, anchored at what the sensor recorded.
//! Flat patches at every mix of clipped and unclipped channels under two white
//! balances fit this within 0.005 of the level (RMS per channel over 136 patches).
use super::*;
use rawmakase_cancel::Cancel;

/// The sensor fraction below which a channel is not pulled, and the exponent of
/// the weight's rise from there to the white.
const PULL_FLOOR: f32 = 0.25;
const PULL_POWER: f32 = 1.8;
/// Fixed-point iterations of the three pulls (the weights are at most 1 and the
/// targets are means, so the error shrinks by at least half each round).
const ROUNDS: usize = 8;

/// Camera Raw's reconstruction of one white-balanced pixel: `sensor` is each
/// channel's fraction of the sensor's white (1 and above is clipped).
fn reconstruct(p: [f32; 3], sensor: [f32; 3]) -> [f32; 3] {
    let weight = sensor.map(|s| {
        ((s - PULL_FLOOR) / (1. - PULL_FLOOR))
            .clamp(0., 1.)
            .powf(PULL_POWER)
    });
    let mut x = p;
    for _ in 0..ROUNDS {
        for c in 0..3 {
            let mean = (x[(c + 1) % 3] + x[(c + 2) % 3]) * 0.5;
            x[c] = p[c] + weight[c] * (mean - p[c]).max(0.);
        }
    }
    x
}

/// One white-balanced pixel reconstructed, or `None` for a pixel no channel of
/// which is bright enough to be pulled: `wb` are the photo's as-shot multipliers,
/// which give each channel's fraction of the sensor's white.
fn recovered_pixel(p: [f32; 3], wb: [f32; 3]) -> Option<[f32; 3]> {
    let sensor = std::array::from_fn(|c| p[c] / wb[c]);
    (!sensor.iter().all(|s| *s <= PULL_FLOOR)).then(|| reconstruct(p, sensor))
}

/// The multipliers [`recovered_pixel`] takes, kept away from zero.
fn sensor_white(im: &CameraImage) -> [f32; 3] {
    im.metadata.wb.map(|v| v.max(0.001))
}

/// Highlight reconstruction in camera space, before color conversion. Stops with an
/// error once `cancel` is set, and only then.
pub fn recover_highlights(im: &CameraImage, cancel: Cancel<'_>) -> Result<CameraImage> {
    check_cancel(cancel)?;
    let mut out = im.clone();
    let wb = sensor_white(im);
    out.pixels
        .par_chunks_mut(im.width.max(1) as usize)
        .for_each(|row| {
            if cancel.is_cancelled() {
                return;
            }
            for p in row {
                if let Some(recovered) = recovered_pixel(*p, wb) {
                    *p = recovered;
                }
            }
        });
    check_cancel(cancel)?;
    Ok(out)
}

/// [`recover_highlights`] on `pixels`, the pixels of `im`, where they are:
/// reconstruction is each pixel's own, so the values are the same. Returns which
/// pixels it changed, one bit a pixel (64 a word, from the lowest bit), and the
/// values those had, in pixel order: what [`Pixels::pair_masked`] takes. A pixel
/// counts as changed when any bit of it is, so writing the returned values back
/// restores `pixels` exactly.
///
/// The values are 12 bytes a changed pixel: where recovery changes much of a photo
/// (28% of one measured) they are the most memory recovery takes, in place of
/// the second buffer of 12 bytes a pixel.
///
/// [`Pixels::pair_masked`]: crate::camera_data::Pixels::pair_masked
pub(super) fn recover_in_place(
    pixels: &mut [[f32; 3]],
    im: &CameraImage,
    cancel: Cancel<'_>,
) -> Result<(Vec<u64>, Vec<[f32; 3]>)> {
    use std::sync::atomic::{AtomicU64, Ordering};
    check_cancel(cancel)?;
    let wb = sensor_white(im);
    let width = im.width.max(1) as usize;
    // Rows are recovered side by side, and a word of the mask can span two of
    // them: each bit is set atomically.
    let changed: Vec<AtomicU64> = (0..pixels.len().div_ceil(64))
        .map(|_| AtomicU64::new(0))
        .collect();
    let rows: Vec<Vec<[f32; 3]>> = pixels
        .par_chunks_mut(width)
        .enumerate()
        .map(|(y, row)| {
            let mut had = Vec::new();
            if cancel.is_cancelled() {
                return had;
            }
            for (x, p) in row.iter_mut().enumerate() {
                if let Some(recovered) = recovered_pixel(*p, wb)
                    && recovered.map(f32::to_bits) != p.map(f32::to_bits)
                {
                    let i = y * width + x;
                    changed[i / 64].fetch_or(1 << (i % 64), Ordering::Relaxed);
                    had.push(*p);
                    *p = recovered;
                }
            }
            had
        })
        .collect();
    check_cancel(cancel)?;
    let mut values = Vec::with_capacity(rows.iter().map(Vec::len).sum());
    for row in rows {
        values.extend(row);
    }
    let changed = changed.into_iter().map(AtomicU64::into_inner).collect();
    Ok((changed, values))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3)
    }
    #[test]
    fn a_clipped_channel_becomes_the_mean_of_the_other_two() {
        // Red at the white, green and blue at half: red's target is their mean,
        // below it, so red keeps the recorded value; they are pulled a little
        // (weight w of a half-white channel) towards the mean of red and each
        // other. Fixed point: g = 0.5 + w * ((1 + g) / 2 - 0.5).
        let w = (0.25f32 / 0.75).powf(PULL_POWER);
        let g = 0.5 / (1. - w / 2.);
        assert!(close(
            reconstruct([1., 0.5, 0.5], [1., 0.5, 0.5]),
            [1., g, g]
        ));
        // Green clipped under red and blue above it: exactly the mean of the
        // other two as they come out (blue itself pulled up towards red and green).
        let r = reconstruct([1.4, 1., 1.2], [0.7, 1., 0.85]);
        assert!(
            close(r, [1.4, (1.4 + r[2]) / 2., r[2]]) && r[2] > 1.3,
            "{r:?}"
        );
    }
    #[test]
    fn the_pull_is_upwards_only_and_the_recorded_value_is_a_floor() {
        // Red clipped over a dark green and blue: red stays at the white (the
        // light was at least that bright); green and blue barely move.
        let r = reconstruct([1., 0.4, 0.2], [1., 0.4, 0.2]);
        assert_eq!(r[0], 1.);
        assert!(r[1] > 0.4 && r[1] < 0.43 && r[2] == 0.2, "{r:?}");
        // Fully clipped stays white; two clipped channels pull the third up.
        assert_eq!(reconstruct([1.; 3], [1.; 3]), [1.; 3]);
        let r = reconstruct([1., 1., 0.9], [1., 1., 0.9]);
        let w = (0.65f32 / 0.75).powf(PULL_POWER);
        assert!(close(r, [1., 1., 0.9 + w * 0.1]), "{r:?}");
    }
    #[test]
    fn dark_pixels_are_untouched_and_white_balance_sets_the_sensor_fraction() {
        let mut im = crate::develop::quality::tests::fixture();
        im.metadata.wb = [2., 1., 1.5];
        im.pixels.fill([0.3, 0.1, 0.2]);
        // Green at the sensor's white: pulled to the mean of red and blue, which
        // sit at 0.8 and 0.9 of theirs (and blue follows red and green a little).
        im.pixels[40 * 96 + 40] = [1.6, 1., 1.35];
        let recovered = recover_highlights(&im, Cancel::NEVER).unwrap();
        assert_eq!(recovered.pixels[0], [0.3, 0.1, 0.2]);
        let p = recovered.pixels[40 * 96 + 40];
        assert!(
            p[0] == 1.6 && p[1] > 1.5 && p[1] < 1.6 && p[2] > 1.35,
            "{p:?}"
        );
    }
}
