//! AI Denoise's side of decoding, with no knowledge of the model: the visible Bayer mosaic
//! in the model's input domain ([`PackedBayer`]), and the camera image built from the
//! model's output ([`assemble`]), white balanced as development balances the mosaic.
//! [`AiBase`] is how export and the app ask for a photo's complete AI source; the app
//! implements it with `rawmakase-inference` (docs/rendering-quality.md#ai-denoise).
use crate::camera_data::CameraImage;
use anyhow::{Result, ensure};
use rawmakase_cancel::Cancel;
use rayon::prelude::*;
use std::{path::Path, sync::Arc};

/// Sensor pixels around a clipped sample where the AI source is the plain demosaic: the
/// model sees the mosaic clipped at white, so near clipping its output is what the plain
/// image's highlight recovery should rebuild from. The plain image fades out over a
/// further `CLIPPED_RADIUS` (docs/rendering-quality.md#ai-denoise).
pub const CLIPPED_RADIUS: usize = 8;

/// Why a photo cannot have AI Denoise, for the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unsupported(pub &'static str);
impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Unsupported {}

/// How a visible mosaic of `width × height` maps onto the model's packed RGGB planes.
/// When red is not the first sample, one mirrored row and/or column goes before the
/// mosaic (rawnind_jddc crops one instead; the samples inside are the same) and the
/// planes start there; a mirrored row or column after an odd side completes the last
/// 2 × 2 block. Both are dropped from the output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub width: usize,
    pub height: usize,
    /// Mirrored columns and rows before the mosaic: where red sits in its period.
    pub pad: [usize; 2],
    pub packed_width: usize,
    pub packed_height: usize,
}
impl Layout {
    pub fn new(width: usize, height: usize, red: [usize; 2]) -> Self {
        Self {
            width,
            height,
            pad: red,
            packed_width: (width + red[0]).div_ceil(2),
            packed_height: (height + red[1]).div_ceil(2),
        }
    }
    /// Pixels of the model's output: twice the packed size.
    pub fn output_pixels(&self) -> usize {
        4 * self.packed_width * self.packed_height
    }
}

/// The visible mosaic as the model takes it.
pub struct PackedBayer {
    pub layout: Layout,
    /// R, G1 (beside R), G2 (below R) and B planes, `packed_width × packed_height` each,
    /// (raw − black) / (white − black) clipped to 0..=1, not white balanced.
    pub planes: Vec<f32>,
    /// Per packed pixel, red, the mean of the greens and blue without clipping: the
    /// half-size demosaic before white balance, which the model's scale is fitted to.
    pub reference: Vec<[f32; 3]>,
    /// Per packed pixel, whether any of its four samples is clipped (as development
    /// counts them, at 0.999 of white).
    pub clipped: Vec<bool>,
}

/// Index `i` of `n` samples mirrored without repeating the edge sample.
fn mirror(i: isize, n: usize) -> usize {
    if n == 1 {
        return 0;
    }
    let period = 2 * (n as isize - 1);
    let i = i.rem_euclid(period);
    (if i >= n as isize { period - i } else { i }) as usize
}

impl PackedBayer {
    /// Packs `data`, a `width × height` mosaic (raw − black) / (white − black) whose red
    /// sample is at `red` (`[x, y]`) of its 2 × 2 period.
    pub fn pack(width: usize, height: usize, data: &[f32], red: [usize; 2]) -> Self {
        let layout = Layout::new(width, height, red);
        let (pw, ph) = (layout.packed_width, layout.packed_height);
        let plane = pw * ph;
        let mut planes = vec![0f32; 4 * plane];
        let mut reference = vec![[0f32; 3]; plane];
        let mut clipped = vec![false; plane];
        let sample = |py: usize, px: usize, dy: usize, dx: usize| {
            let y = mirror((2 * py + dy) as isize - red[1] as isize, height);
            let x = mirror((2 * px + dx) as isize - red[0] as isize, width);
            data[y * width + x]
        };
        let (r_plane, rest) = planes.split_at_mut(plane);
        let (g1_plane, rest) = rest.split_at_mut(plane);
        let (g2_plane, b_plane) = rest.split_at_mut(plane);
        r_plane
            .par_chunks_mut(pw)
            .zip(g1_plane.par_chunks_mut(pw))
            .zip(g2_plane.par_chunks_mut(pw))
            .zip(b_plane.par_chunks_mut(pw))
            .zip(reference.par_chunks_mut(pw))
            .zip(clipped.par_chunks_mut(pw))
            .enumerate()
            .for_each(|(py, (((((r, g1), g2), b), reference), clipped))| {
                for px in 0..pw {
                    let v = [
                        sample(py, px, 0, 0),
                        sample(py, px, 0, 1),
                        sample(py, px, 1, 0),
                        sample(py, px, 1, 1),
                    ];
                    r[px] = v[0].clamp(0., 1.);
                    g1[px] = v[1].clamp(0., 1.);
                    g2[px] = v[2].clamp(0., 1.);
                    b[px] = v[3].clamp(0., 1.);
                    reference[px] = [v[0], (v[1] + v[2]) / 2., v[3]];
                    clipped[px] = v.iter().any(|v| *v >= 0.999);
                }
            });
        Self {
            layout,
            planes,
            reference,
            clipped,
        }
    }
}

/// The model's output for one photo, matched to the mosaic's scale.
pub struct ModelOutput<'a> {
    pub layout: Layout,
    /// Red, green and blue planes of `2 × packed_width` by `2 × packed_height`, unbalanced
    /// camera RGB with sensor white at 1.
    pub rgb: &'a [f32],
    /// Per packed pixel, whether the model has filled it: elsewhere the image is the
    /// plain demosaic (a partial result). `None` when complete.
    pub done: Option<&'a [bool]>,
    /// [`PackedBayer::clipped`].
    pub clipped: &'a [bool],
}

/// For each packed pixel, the share of the plain image near clipped samples: 1 within
/// [`CLIPPED_RADIUS`] sensor pixels of one, fading to 0 over as many again.
fn plain_share(layout: &Layout, clipped: &[bool]) -> Vec<f32> {
    let (w, h) = (layout.packed_width, layout.packed_height);
    // Chessboard distance to the nearest clipped packed pixel, two passes.
    let far = u32::MAX / 2;
    let mut d: Vec<u32> = clipped.iter().map(|c| if *c { 0 } else { far }).collect();
    for y in 0..h {
        for x in 0..w {
            let mut v = d[y * w + x];
            if x > 0 {
                v = v.min(d[y * w + x - 1] + 1);
            }
            if y > 0 {
                v = v.min(d[(y - 1) * w + x] + 1);
                if x > 0 {
                    v = v.min(d[(y - 1) * w + x - 1] + 1);
                }
                if x + 1 < w {
                    v = v.min(d[(y - 1) * w + x + 1] + 1);
                }
            }
            d[y * w + x] = v;
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let mut v = d[y * w + x];
            if x + 1 < w {
                v = v.min(d[y * w + x + 1] + 1);
            }
            if y + 1 < h {
                v = v.min(d[(y + 1) * w + x] + 1);
                if x + 1 < w {
                    v = v.min(d[(y + 1) * w + x + 1] + 1);
                }
                if x > 0 {
                    v = v.min(d[(y + 1) * w + x - 1] + 1);
                }
            }
            d[y * w + x] = v;
        }
    }
    let r = CLIPPED_RADIUS as f32;
    d.into_iter()
        .map(|d| {
            let sensor = 2. * d as f32;
            ((2. * r - sensor) / r).clamp(0., 1.)
        })
        .collect()
}

/// The camera image AI Denoise renders from: the model's output white balanced by the
/// photo's `Metadata::wb` (as development balances the mosaic), clamped at zero as the
/// demosaic clamps, with the `plain` demosaic (unrecovered) where the model saw clipped
/// samples and where it has not run yet. Its metadata and clipping count are the plain
/// image's; highlights are recovered later, on this image of its own.
pub fn assemble(
    output: &ModelOutput<'_>,
    plain: &CameraImage,
    cancel: Cancel<'_>,
) -> Result<CameraImage> {
    let layout = output.layout;
    let (w, h) = (layout.width, layout.height);
    ensure!(
        plain.width as usize == w && plain.height as usize == h && !plain.fast,
        "The AI Denoise result does not match the photo"
    );
    let (pw, ph) = (layout.packed_width, layout.packed_height);
    ensure!(
        output.rgb.len() == 3 * layout.output_pixels()
            && output.clipped.len() == pw * ph
            && output.done.is_none_or(|d| d.len() == pw * ph),
        "The AI Denoise result has the wrong size"
    );
    let mut share = plain_share(&layout, output.clipped);
    if let Some(done) = output.done {
        for (s, d) in share.iter_mut().zip(done) {
            if !d {
                *s = 1.;
            }
        }
    }
    let wb = plain.metadata.wb;
    let (ow, plane) = (2 * pw, 4 * pw * ph);
    let [ax, ay] = layout.pad;
    let mut pixels = vec![[0f32; 3]; w * h];
    pixels
        .par_chunks_mut(w)
        .zip(plain.pixels.par_chunks(w))
        .enumerate()
        .for_each(|(y, (row, plain_row))| {
            let oy = y + ay;
            for (x, (p, q)) in row.iter_mut().zip(plain_row).enumerate() {
                let ox = x + ax;
                let s = share[(oy / 2) * pw + ox / 2];
                let i = oy * ow + ox;
                if s == 1. {
                    *p = *q;
                    continue;
                }
                for c in 0..3 {
                    let ai = (output.rgb[c * plane + i] * wb[c]).max(0.);
                    p[c] = if s == 0. { ai } else { ai + s * (q[c] - ai) };
                }
            }
        });
    ensure!(!cancel.is_cancelled(), "Development cancelled");
    Ok(CameraImage {
        recovered: Default::default(),
        width: plain.width,
        height: plain.height,
        pixels,
        metadata: plain.metadata.clone(),
        fast: false,
        scale_factor: plain.scale_factor,
        scale_clipped: plain.scale_clipped,
        hdr: Default::default(),
    })
}

/// Gets a photo's complete AI Denoise source: export and the app ask the app's service,
/// which takes it from its cache, waits for a running job, or computes it.
pub trait AiBase: Send + Sync {
    /// The assembled AI source (not highlight-recovered) of the photo at `path`, whose
    /// full-size plain decode is `plain`, made from the file as `file` identifies it
    /// (read before decoding). Never partial, never of another file: an error names why
    /// there is none (no model, no runtime, an unsupported sensor, a failed run, a file
    /// changed since it was decoded).
    fn get(
        &self,
        path: &Path,
        file: &crate::storage::Identity,
        plain: &Arc<CameraImage>,
        cancel: Cancel<'_>,
    ) -> Result<Arc<CameraImage>>;
}

/// The [`AiBase`] an export asks, cloned with the batch or job that holds it.
#[derive(Clone)]
pub struct AiService(pub Arc<dyn AiBase>);
impl AiService {
    /// One that has none: a photo with AI Denoise on fails with `reason`.
    pub fn refusing(reason: &'static str) -> Self {
        Self(Arc::new(Refusing(reason)))
    }
}
impl std::fmt::Debug for AiService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AiService")
    }
}

/// An [`AiBase`] that has none, for callers without the app's service: a photo with AI
/// Denoise on fails with `reason` instead of rendering without it.
pub struct Refusing(pub &'static str);
impl AiBase for Refusing {
    fn get(
        &self,
        _: &Path,
        _: &crate::storage::Identity,
        _: &Arc<CameraImage>,
        _: Cancel<'_>,
    ) -> Result<Arc<CameraImage>> {
        anyhow::bail!("{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `width × height` mosaic of `pattern` (the colours of its 2 × 2 period, row by
    /// row: 0 red, 1 green, 2 blue) sampling a smooth scene; the value encodes the
    /// position so packing can be checked exactly.
    fn mosaic(width: usize, height: usize) -> Vec<f32> {
        (0..width * height)
            .map(|i| 0.1 + 0.8 * ((i * 37) % 1001) as f32 / 1000.)
            .collect()
    }

    /// rawnind_jddc's `mono_any_to_mono_rggb` (at e80fb7b1): GBRG crops a row at the top
    /// and bottom, GRBG a column at the left and right, BGGR both; then sides are cut to
    /// multiples of 4, and the planes taken as RGGB.
    fn upstream(
        width: usize,
        height: usize,
        data: &[f32],
        red: [usize; 2],
    ) -> (usize, usize, Vec<f32>) {
        let (cx, cy) = (red[0], red[1]);
        let (w, h) = (width - 2 * cx, height - 2 * cy);
        let (w, h) = (w - w % 4, h - h % 4);
        let (pw, ph) = (w / 2, h / 2);
        let mut planes = vec![0.; 4 * pw * ph];
        for py in 0..ph {
            for px in 0..pw {
                for (c, (dy, dx)) in [(0, 0), (0, 1), (1, 0), (1, 1)].into_iter().enumerate() {
                    let y = 2 * py + dy + cy;
                    let x = 2 * px + dx + cx;
                    planes[c * pw * ph + py * pw + px] = data[y * width + x];
                }
            }
        }
        (pw, ph, planes)
    }

    #[test]
    fn packing_equals_upstreams_crop_inside_for_every_phase() {
        let (width, height) = (37, 29);
        let data = mosaic(width, height);
        for red in [[0, 0], [1, 0], [0, 1], [1, 1]] {
            let packed = PackedBayer::pack(width, height, &data, red);
            let l = packed.layout;
            assert_eq!(l.packed_width, (width + red[0]).div_ceil(2));
            assert_eq!(l.packed_height, (height + red[1]).div_ceil(2));
            let (uw, uh, theirs) = upstream(width, height, &data, red);
            // Upstream's packed (0, 0) is ours shifted by the pad: one packed pixel when
            // a row or column was mirrored in, none otherwise.
            let (sx, sy) = (red[0], red[1]);
            let plane = l.packed_width * l.packed_height;
            for c in 0..4 {
                for y in 0..uh {
                    for x in 0..uw {
                        assert_eq!(
                            packed.planes[c * plane + (y + sy) * l.packed_width + x + sx],
                            theirs[c * uw * uh + y * uw + x],
                            "red {red:?} plane {c} at {x},{y}"
                        );
                    }
                }
            }
            // The mirrored samples are real ones of the same colour.
            assert_eq!(
                packed.planes[0],
                data[mirror(-(red[1] as isize), height) * width
                    + mirror(-(red[0] as isize), width)]
            );
        }
    }

    #[test]
    fn values_are_clipped_for_the_model_but_not_for_the_gain_reference() {
        let data = vec![-0.02, 0.5, 0.25, 1.2];
        let packed = PackedBayer::pack(2, 2, &data, [0, 0]);
        assert_eq!(packed.planes, vec![0., 0.5, 0.25, 1.]);
        assert_eq!(packed.reference, vec![[-0.02, 0.375, 1.2]]);
        assert_eq!(packed.clipped, vec![true]);
    }

    fn plain(width: usize, height: usize, wb: [f32; 3]) -> CameraImage {
        let metadata = crate::camera_data::Metadata {
            wb,
            ..Default::default()
        };
        CameraImage {
            recovered: Default::default(),
            width: width as u32,
            height: height as u32,
            pixels: (0..width * height)
                .map(|i| [0.3 + (i % 5) as f32 * 0.01; 3])
                .collect(),
            metadata,
            fast: false,
            scale_factor: 1.,
            scale_clipped: 7,
            hdr: Default::default(),
        }
    }

    /// A model output at the packed layout holding `f(x, y, c)` at padded sensor position.
    fn output(layout: &Layout, f: impl Fn(usize, usize, usize) -> f32) -> Vec<f32> {
        let (w, h) = (2 * layout.packed_width, 2 * layout.packed_height);
        let mut v = vec![0.; 3 * w * h];
        for c in 0..3 {
            for y in 0..h {
                for x in 0..w {
                    v[c * w * h + y * w + x] = f(x, y, c);
                }
            }
        }
        v
    }

    #[test]
    fn assembly_drops_the_padding_and_balances_once() {
        let (width, height) = (9, 7);
        let wb = [2., 1., 1.5];
        let plain = plain(width, height, wb);
        for red in [[0, 0], [1, 0], [0, 1], [1, 1]] {
            let layout = Layout::new(width, height, red);
            let rgb = output(&layout, |x, y, c| {
                (x * 100 + y) as f32 * 1e-4 + c as f32 * 1e-6
            });
            let clipped = vec![false; layout.packed_width * layout.packed_height];
            let image = assemble(
                &ModelOutput {
                    layout,
                    rgb: &rgb,
                    done: None,
                    clipped: &clipped,
                },
                &plain,
                Cancel::NEVER,
            )
            .unwrap();
            assert_eq!((image.width, image.height), (width as u32, height as u32));
            assert_eq!(image.scale_clipped, 7);
            for y in 0..height {
                for x in 0..width {
                    let (ox, oy) = (x + red[0], y + red[1]);
                    for (c, balance) in wb.iter().enumerate() {
                        let expected = ((ox * 100 + oy) as f32 * 1e-4 + c as f32 * 1e-6) * balance;
                        assert_eq!(
                            image.pixels[y * width + x][c],
                            expected,
                            "{red:?} {x},{y},{c}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn near_clipped_samples_and_unfinished_tiles_are_the_plain_image() {
        let (width, height) = (60, 4);
        let plain = plain(width, height, [1.; 3]);
        let layout = Layout::new(width, height, [0, 0]);
        let rgb = output(&layout, |_, _, _| 0.9);
        let mut clipped = vec![false; layout.packed_width * layout.packed_height];
        clipped[0] = true; // packed (0, 0): sensor 0..2
        let mut done = vec![true; clipped.len()];
        done[29] = false; // the last packed column of row 0
        let image = assemble(
            &ModelOutput {
                layout,
                rgb: &rgb,
                done: Some(&done),
                clipped: &clipped,
            },
            &plain,
            Cancel::NEVER,
        )
        .unwrap();
        let at = |x: usize, y: usize| image.pixels[y * width + x][0];
        let p = |x: usize, y: usize| plain.pixels[y * width + x][0];
        // Within the radius: plain. Beyond twice the radius: the model.
        assert_eq!(at(0, 0), p(0, 0));
        assert_eq!(at(9, 0), p(9, 0)); // packed distance 4 = 8 sensor pixels
        assert_eq!(at(16, 0), 0.9); // packed distance 8 = 16 sensor pixels
        // In between: a blend.
        let mid = at(12, 0); // packed distance 6 = 12 sensor pixels: half
        assert!((mid - (0.9 + 0.5 * (p(12, 0) - 0.9))).abs() < 1e-6);
        // Not yet computed: plain.
        assert_eq!(at(58, 0), p(58, 0));
        assert_eq!(at(58, 2), 0.9);
    }

    #[test]
    fn a_result_of_another_size_or_a_draft_is_refused() {
        let plain = plain(8, 8, [1.; 3]);
        let layout = Layout::new(10, 8, [0, 0]);
        let rgb = output(&layout, |_, _, _| 0.5);
        let clipped = vec![false; layout.packed_width * layout.packed_height];
        let result = assemble(
            &ModelOutput {
                layout,
                rgb: &rgb,
                done: None,
                clipped: &clipped,
            },
            &plain,
            Cancel::NEVER,
        );
        assert!(result.is_err());
    }

    #[test]
    fn the_refusing_base_fails_with_its_reason() {
        let plain = Arc::new(plain(2, 2, [1.; 3]));
        let Err(error) = Refusing("no AI Denoise here").get(
            Path::new("x.dng"),
            &crate::storage::Identity {
                size: 0,
                modified_ns: 0,
                prefix_hash: String::new(),
            },
            &plain,
            Cancel::NEVER,
        ) else {
            panic!("refused nothing");
        };
        assert_eq!(error.to_string(), "no AI Denoise here");
    }
}
