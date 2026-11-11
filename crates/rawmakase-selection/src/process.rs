//! Pure mappings between photos and tensors: the image to the encoder's input, a
//! prompt to the decoder's, and the decoder's low-resolution mask logits back to
//! 8-bit coverage in the photo's frame. No runtime is involved, so these are tested
//! without a model.
use crate::manifest::ModelSpec;
use rawmakase_inference::InferenceError;

/// Largest coverage returned, per side.
pub const MAX_COVERAGE_SIDE: usize = 4096;
/// Largest input accepted, per side; larger photos are for the caller to downscale
/// first, and the cap keeps index arithmetic far from overflow.
pub const MAX_INPUT_SIDE: usize = 16_384;
/// Most points in one prompt.
pub const MAX_POINTS: usize = 32;

/// 8-bit sRGB, interleaved `R G B`, row-major, `data.len() == width * height * 3`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbImage {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

/// 8-bit coverage (0 = unselected, 255 = selected), row-major, one byte per pixel,
/// at the aspect ratio of the image it was computed from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

/// An integer rectangle in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

/// One click: a position as fractions of the photo's width and height, and whether
/// it marks the object (`true`) or something to leave out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
    pub positive: bool,
}

/// What to select: clicks and/or a box (`[left, top, right, bottom]`, fractions).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Prompt {
    pub points: Vec<Point>,
    pub bounds: Option<[f32; 4]>,
}

impl Prompt {
    /// Checks the prompt is something the decoder can use.
    pub fn validate(&self) -> Result<(), InferenceError> {
        let unit = |v: f32| v.is_finite() && (0.0..=1.0).contains(&v);
        let bad = |why: &str| Err(InferenceError::OutputInvalid(why.into()));
        if self.points.is_empty() && self.bounds.is_none() {
            return bad("the prompt has no point and no box");
        }
        if self.points.len() > MAX_POINTS {
            return bad("the prompt has too many points");
        }
        if !self.points.iter().all(|p| unit(p.x) && unit(p.y)) {
            return bad("a prompt point is outside the photo");
        }
        if let Some([l, t, r, b]) = self.bounds
            && !(unit(l) && unit(t) && unit(r) && unit(b) && l < r && t < b)
        {
            return bad("the prompt box is empty or outside the photo");
        }
        Ok(())
    }
    /// The decoder's tensors: points in the encoder's pixel coordinates, their labels
    /// (1 object, 0 elsewhere) and the box.
    pub fn tensors(&self, spec: &ModelSpec) -> (Vec<f32>, Vec<i64>, Vec<f32>) {
        let size = spec.input_size as f32;
        let points = self
            .points
            .iter()
            .flat_map(|p| [p.x * size, p.y * size])
            .collect();
        let labels = self.points.iter().map(|p| i64::from(p.positive)).collect();
        let bounds = self
            .bounds
            .map(|b| b.map(|v| v * size).to_vec())
            .unwrap_or_default();
        (points, labels, bounds)
    }
}

/// Stretches `image` to the encoder's square input and normalizes it: the planar
/// `[1, 3, S, S]` float tensor, flat and channel-major.
pub fn preprocess(
    image: &RgbImage,
    size: usize,
    mean: [f32; 3],
    std: [f32; 3],
) -> Result<Vec<f32>, InferenceError> {
    preprocess_sized(image, size, size, mean, std)
}

/// As [`preprocess`], to a `width` x `height` input that need not be square.
pub fn preprocess_sized(
    image: &RgbImage,
    width: usize,
    height: usize,
    mean: [f32; 3],
    std: [f32; 3],
) -> Result<Vec<f32>, InferenceError> {
    check_image(image)?;
    let whole = Rect {
        x: 0,
        y: 0,
        width: image.width,
        height: image.height,
    };
    let mut tensor = Vec::with_capacity(3 * width * height);
    let mut plane = vec![0.0f32; image.width * image.height];
    for channel in 0..3 {
        for (dst, rgb) in plane.iter_mut().zip(image.data.as_chunks::<3>().0) {
            *dst = f32::from(rgb[channel]) / 255.0;
        }
        let resized = resample(&plane, image.width, whole, width, height);
        tensor.extend(resized.iter().map(|v| (v - mean[channel]) / std[channel]));
    }
    Ok(tensor)
}

pub(crate) fn check_image(image: &RgbImage) -> Result<(), InferenceError> {
    if image.width == 0 || image.height == 0 {
        return Err(InferenceError::OutputInvalid(
            "the input image is empty".into(),
        ));
    }
    if image.width > MAX_INPUT_SIDE || image.height > MAX_INPUT_SIDE {
        return Err(InferenceError::OutputInvalid(format!(
            "the input image {}x{} exceeds {MAX_INPUT_SIDE} px per side",
            image.width, image.height
        )));
    }
    let expected = image.width * image.height * 3;
    if image.data.len() != expected {
        return Err(InferenceError::OutputInvalid(format!(
            "the input image has {} bytes, expected {expected} for {}x{} RGB",
            image.data.len(),
            image.width,
            image.height
        )));
    }
    Ok(())
}

/// The size of the coverage returned for an image: its own, scaled down to
/// [`MAX_COVERAGE_SIDE`] if larger.
pub fn output_size(image: &RgbImage) -> (usize, usize) {
    let longest = image.width.max(image.height);
    if longest <= MAX_COVERAGE_SIDE {
        return (image.width, image.height);
    }
    let scale = MAX_COVERAGE_SIDE as f64 / longest as f64;
    let scaled = |v: usize| ((v as f64 * scale).round() as usize).clamp(1, MAX_COVERAGE_SIDE);
    (scaled(image.width), scaled(image.height))
}

/// Turns the decoder's low-resolution mask logits into coverage at `width` x
/// `height`: the logits are resampled smoothly first and only then squashed, which
/// keeps the edge where the model put it instead of stepping at the logit grid.
pub fn postprocess(
    logits: &[f32],
    spec: &ModelSpec,
    width: usize,
    height: usize,
) -> Result<Coverage, InferenceError> {
    let side = spec.mask_size;
    if logits.len() != side * side {
        return Err(InferenceError::OutputInvalid(format!(
            "the mask has {} values, expected {}",
            logits.len(),
            side * side
        )));
    }
    if logits.iter().any(|v| !v.is_finite()) {
        return Err(InferenceError::OutputInvalid(
            "the mask contains NaN or infinity".into(),
        ));
    }
    let whole = Rect {
        x: 0,
        y: 0,
        width: side,
        height: side,
    };
    let resized = resample(logits, side, whole, width, height);
    let data = resized
        .iter()
        .map(|&l| (sigmoid(l) * 255.0).round() as u8)
        .collect();
    Ok(Coverage {
        width,
        height,
        data,
    })
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// Per-output-coordinate filter taps: the first source index and the weights.
struct Taps {
    first: usize,
    weights: Vec<f32>,
}

/// Triangle-filter taps mapping `dst_len` outputs onto `src_len` source samples
/// starting at `src_start`. The filter support grows with the shrink factor.
fn taps(src_start: usize, src_len: usize, dst_len: usize) -> Vec<Taps> {
    let scale = src_len as f64 / dst_len as f64;
    let filter_scale = scale.max(1.0);
    let support = filter_scale;
    (0..dst_len)
        .map(|i| {
            let center = (i as f64 + 0.5) * scale;
            let lo = ((center - support + 0.5).floor().max(0.0)) as usize;
            let hi = (((center + support + 0.5).floor()) as usize).min(src_len);
            let lo = lo.min(hi.saturating_sub(1));
            let mut weights: Vec<f32> = (lo..hi)
                .map(|x| {
                    let d = ((x as f64 + 0.5 - center) / filter_scale).abs();
                    (1.0 - d).max(0.0) as f32
                })
                .collect();
            let sum: f32 = weights.iter().sum();
            if sum > 0.0 {
                weights.iter_mut().for_each(|w| *w /= sum);
            } else {
                // Cannot happen for a triangle with these bounds; stay safe.
                weights = vec![1.0 / weights.len().max(1) as f32; weights.len().max(1)];
            }
            Taps {
                first: src_start + lo,
                weights,
            }
        })
        .collect()
}

/// Resamples the `window` of a single-channel `src` plane (row stride `stride`)
/// to `dst_width` x `dst_height`.
pub(crate) fn resample(
    src: &[f32],
    stride: usize,
    window: Rect,
    dst_width: usize,
    dst_height: usize,
) -> Vec<f32> {
    let xs = taps(window.x, window.width, dst_width);
    let ys = taps(window.y, window.height, dst_height);
    let first_row = ys.iter().map(|t| t.first).min().unwrap_or(window.y);
    let last_row = ys
        .iter()
        .map(|t| t.first + t.weights.len())
        .max()
        .unwrap_or(window.y);
    // Horizontal pass over only the rows the vertical pass will read.
    let mut rows = vec![0.0f32; (last_row - first_row) * dst_width];
    for (r, out_row) in rows.chunks_exact_mut(dst_width).enumerate() {
        let line = &src[(first_row + r) * stride..(first_row + r + 1) * stride];
        for (out, tap) in out_row.iter_mut().zip(&xs) {
            *out = tap
                .weights
                .iter()
                .zip(&line[tap.first..])
                .map(|(w, v)| w * v)
                .sum();
        }
    }
    let mut dst = vec![0.0f32; dst_width * dst_height];
    for (out_row, tap) in dst.chunks_exact_mut(dst_width).zip(&ys) {
        for (w, r) in tap.weights.iter().zip(tap.first - first_row..) {
            let row = &rows[r * dst_width..(r + 1) * dst_width];
            for (o, v) in out_row.iter_mut().zip(row) {
                *o += w * v;
            }
        }
    }
    dst
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::SUBJECT;

    fn image(width: usize, height: usize) -> RgbImage {
        RgbImage {
            width,
            height,
            data: (0..width * height * 3).map(|i| (i % 251) as u8).collect(),
        }
    }

    #[test]
    fn the_input_is_stretched_and_normalized_per_channel() {
        let flat = RgbImage {
            width: 3,
            height: 2,
            data: [255, 0, 51].repeat(6),
        };
        let tensor = preprocess(&flat, SUBJECT.input_size, SUBJECT.mean, SUBJECT.std).unwrap();
        let plane = SUBJECT.input_size * SUBJECT.input_size;
        assert_eq!(tensor.len(), 3 * plane);
        let expect = |c: usize, v: f32| (v - SUBJECT.mean[c]) / SUBJECT.std[c];
        for (c, v) in [(0, 1.0), (1, 0.0), (2, 0.2)] {
            assert!((tensor[c * plane + 5000] - expect(c, v)).abs() < 1e-4);
            assert!((tensor[(c + 1) * plane - 1] - expect(c, v)).abs() < 1e-4);
        }
    }

    #[test]
    fn a_malformed_image_is_refused() {
        let mut bad = image(4, 4);
        bad.data.pop();
        let pre = |i: &RgbImage| preprocess(i, 1024, SUBJECT.mean, SUBJECT.std);
        assert!(pre(&bad).is_err());
        assert!(pre(&image(0, 4)).is_err());
    }

    #[test]
    fn a_prompt_maps_to_encoder_pixels_and_labels() {
        let prompt = Prompt {
            points: vec![
                Point {
                    x: 0.5,
                    y: 0.25,
                    positive: true,
                },
                Point {
                    x: 1.0,
                    y: 0.0,
                    positive: false,
                },
            ],
            bounds: Some([0.0, 0.5, 0.5, 1.0]),
        };
        prompt.validate().unwrap();
        let (points, labels, bounds) = prompt.tensors(&SUBJECT);
        assert_eq!(points, [512.0, 256.0, 1024.0, 0.0]);
        assert_eq!(labels, [1, 0]);
        assert_eq!(bounds, [0.0, 512.0, 512.0, 1024.0]);
    }

    #[test]
    fn prompts_that_the_decoder_cannot_use_are_refused() {
        let at = |x: f32, y: f32| Point {
            x,
            y,
            positive: true,
        };
        assert!(Prompt::default().validate().is_err());
        for bad in [
            Prompt {
                points: vec![at(1.5, 0.5)],
                bounds: None,
            },
            Prompt {
                points: vec![at(f32::NAN, 0.5)],
                bounds: None,
            },
            Prompt {
                points: vec![at(0.5, 0.5); MAX_POINTS + 1],
                bounds: None,
            },
            Prompt {
                points: vec![],
                bounds: Some([0.5, 0.5, 0.5, 0.9]),
            },
            Prompt {
                points: vec![],
                bounds: Some([0.6, 0.1, 0.4, 0.9]),
            },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
        assert!(
            Prompt {
                points: vec![],
                bounds: Some([0.1, 0.1, 0.9, 0.9])
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn logits_become_smooth_coverage_in_the_photos_frame() {
        let side = SUBJECT.mask_size;
        // Selected on the left half, unselected on the right.
        let logits: Vec<f32> = (0..side * side)
            .map(|i| if i % side < side / 2 { 3.0 } else { -3.0 })
            .collect();
        let cov = postprocess(&logits, &SUBJECT, 300, 100).unwrap();
        assert_eq!((cov.width, cov.height, cov.data.len()), (300, 100, 30000));
        assert!(cov.data[50 * 300 + 10] > 235 && cov.data[50 * 300 + 290] < 20);
        // The step is a monotone ramp, never a ringing or a staircase.
        let row = &cov.data[50 * 300..51 * 300];
        assert!(row.windows(2).all(|w| w[0] >= w[1]));
    }

    #[test]
    fn a_damaged_mask_is_an_error() {
        let side = SUBJECT.mask_size;
        let mut logits = vec![0.0f32; side * side];
        assert!(postprocess(&logits[1..], &SUBJECT, 8, 8).is_err());
        logits[7] = f32::INFINITY;
        assert!(postprocess(&logits, &SUBJECT, 8, 8).is_err());
    }

    #[test]
    fn large_outputs_are_capped() {
        assert_eq!(
            output_size(&RgbImage {
                width: 8192,
                height: 4096,
                data: vec![]
            }),
            (4096, 2048)
        );
        assert_eq!(output_size(&image(30, 20)), (30, 20));
    }
}
