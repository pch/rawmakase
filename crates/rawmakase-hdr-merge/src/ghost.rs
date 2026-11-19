//! Deghosting: where something moved between the frames, each frame that
//! shows it elsewhere than the reference does is left out of the merge, so
//! it appears once, as the reference saw it, rather than as a half-
//! transparent ghost of every frame's place.
//!
//! A frame's pixel disagrees with the reference's when their luminance (the
//! sum of the three channels), the frame's brought to the reference's
//! exposure, differ by more than the deghost level's [`Deghost::threshold`]
//! standard deviations of what their noise, the exposure ratio's error and a
//! fraction of a pixel's misalignment at an edge explain. Where one of them
//! clipped, its luminance is only a lower bound, and only the other falling
//! short of it disagrees: a frame that saw a person where the reference saw
//! a clipped sky is left out there too. The pixels that disagree are opened
//! (so noise's lone outliers leave nothing), dilated and feathered into the
//! frame's mask; inside it the frame's weight goes to 0, falling back to its
//! own over the feather outside.
//!
//! Where a mask leaves no frame to merge (the reference clipped there, and
//! every frame that wasn't left out), each connected region takes the
//! brightest frame left out that saw all of it unclipped, or the darkest, as
//! a single RAW would: all of it from one frame, so no seam runs through it.
use crate::merge::RAMP_END;
use crate::noise::Noise;
use crate::{Deghost, Error};
use rawmakase_cancel::Cancel;
use rawmakase_decode::raw::Frame;
use rayon::prelude::*;

/// The exposure ratio's error a difference may come from, as a fraction of
/// the reference's luminance: about 0.03 stop.
const RATIO_ERROR: f64 = 0.02;
/// The misalignment, in pixels, a difference at an edge may come from:
/// Auto Align's fits are a few tenths of a pixel off at worst, and two
/// frames' demosaics put an edge a little apart.
const MISALIGNMENT: f64 = 0.3;
/// The opening's radius, the dilation's and the feather's standard
/// deviation, as fractions of the long edge (at least a pixel each).
const OPENING: f64 = 1. / 2000.;
const DILATION: f64 = 0.005;
const FEATHER: f64 = 0.005;

impl Deghost {
    /// How many standard deviations a frame's pixel may differ from the
    /// reference's before it counts as something that moved; `None` for
    /// [`Deghost::None`].
    pub fn threshold(self) -> Option<f64> {
        match self {
            Self::None => None,
            Self::Low => Some(6.),
            Self::Medium => Some(4.),
            Self::High => Some(3.),
        }
    }
}

/// `frame`'s mask against `reference`: per pixel how much of its weight is
/// taken away, 1 where it disagrees, falling to 0 over the feather; `None`
/// where it agrees everywhere. `ratio` brings the frame's levels to the
/// reference's exposure; `covered` says which pixels a warped frame covers.
#[allow(clippy::too_many_arguments)]
pub(crate) fn mask(
    reference: &Frame,
    reference_noise: Noise,
    frame: &Frame,
    noise: Noise,
    ratio: f64,
    covered: Option<&[bool]>,
    threshold: f64,
    cancel: Cancel<'_>,
) -> Result<Option<Vec<f32>>, Error> {
    let (w, h) = (frame.width as usize, frame.height as usize);
    if reference.pixels.len() != w * h
        || frame.pixels.len() != w * h
        || covered.is_some_and(|c| c.len() != w * h)
    {
        return Err(Error::Invalid("frames of different sizes"));
    }
    let disagrees = disagreement(
        reference,
        reference_noise,
        frame,
        noise,
        ratio,
        covered,
        threshold,
    );
    if !disagrees.contains(&1.) {
        return Ok(None);
    }
    let check = || {
        if cancel.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    };
    check()?;
    let long = w.max(h) as f64;
    let radius = |fraction: f64| ((long * fraction).round() as usize).max(1);
    let opened = dilate(
        &erode(&disagrees, w, h, radius(OPENING)),
        w,
        h,
        radius(OPENING),
    );
    if !opened.contains(&1.) {
        return Ok(None);
    }
    check()?;
    let dilated = dilate(&opened, w, h, radius(DILATION));
    drop(opened);
    check()?;
    let sigma = (long * FEATHER).max(1.);
    let blurred = blur(&dilated, w, h, sigma);
    // Twice the blur: 1 throughout the dilated mask (the blur is at least
    // about a half at its edge, and the mask itself is 1), falling to 0
    // outside over about two standard deviations.
    Ok(Some(
        blurred
            .par_iter()
            .zip(dilated.par_iter())
            .map(|(b, d)| (2. * b).clamp(0., 1.).max(*d))
            .collect(),
    ))
}

/// Per pixel 1 where the frame disagrees with the reference, else 0.
fn disagreement(
    reference: &Frame,
    reference_noise: Noise,
    frame: &Frame,
    noise: Noise,
    ratio: f64,
    covered: Option<&[bool]>,
    threshold: f64,
) -> Vec<f32> {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let luminance = |p: [f32; 3]| p.iter().map(|v| f64::from(*v)).sum::<f64>();
    let variance =
        |p: [f32; 3], noise: Noise| p.iter().map(|v| noise.variance(f64::from(*v))).sum::<f64>();
    let at = |x: usize, y: usize| luminance(reference.pixels[y * w + x]);
    (0..h)
        .into_par_iter()
        .flat_map_iter(|y| {
            (0..w).map(move |x| {
                let i = y * w + x;
                if covered.is_some_and(|c| !c[i]) {
                    return 0.;
                }
                let a = luminance(frame.pixels[i]) * ratio;
                let b = luminance(reference.pixels[i]);
                let (a_bound, b_bound) = (bounded(frame, i), bounded(reference, i));
                let short = match (a_bound, b_bound) {
                    (false, false) => (a - b).abs(),
                    (false, true) => b - a,
                    (true, false) => a - b,
                    (true, true) => 0.,
                };
                if short.is_nan() || short <= 0. {
                    return 0.;
                }
                let gx = at((x + 1).min(w - 1), y) - at(x.saturating_sub(1), y);
                let gy = at(x, (y + 1).min(h - 1)) - at(x, y.saturating_sub(1));
                let edge = MISALIGNMENT * gx.hypot(gy) / 2.;
                let spread = variance(frame.pixels[i], noise) * ratio * ratio
                    + variance(reference.pixels[i], reference_noise)
                    + (RATIO_ERROR * b).powi(2)
                    + edge * edge;
                if short > threshold * spread.sqrt() {
                    1.
                } else {
                    0.
                }
            })
        })
        .collect()
}

/// Whether `f`'s pixel `i` is clipped, or so near its saturation that the
/// merge's ramp gives it no weight: its luminance is only a lower bound.
fn bounded(f: &Frame, i: usize) -> bool {
    f.clipped[i] || (0..3).any(|c| f.pixels[i][c] >= f.saturation[c] * RAMP_END)
}

/// Per pixel 1 where every pixel of the square of `radius` around it, within
/// the image, is 1.
fn erode(mask: &[f32], w: usize, h: usize, radius: usize) -> Vec<f32> {
    let sums = box_sum(mask, w, h, radius);
    sums.par_iter()
        .enumerate()
        .map(|(i, s)| {
            let n = span(i % w, w, radius) * span(i / w, h, radius);
            if *s >= n as f32 - 0.5 { 1. } else { 0. }
        })
        .collect()
}

/// Per pixel 1 where any pixel of the square of `radius` around it is 1.
fn dilate(mask: &[f32], w: usize, h: usize, radius: usize) -> Vec<f32> {
    box_sum(mask, w, h, radius)
        .into_par_iter()
        .map(|s| if s > 0.5 { 1. } else { 0. })
        .collect()
}

/// About a Gaussian blur of standard deviation `sigma`: three box blurs,
/// each the mean over the part of its square within the image.
fn blur(values: &[f32], w: usize, h: usize, sigma: f64) -> Vec<f32> {
    // Three boxes of radius r have a variance of r (r + 1).
    let radius = ((sigma * sigma + 0.25).sqrt() - 0.5).round().max(1.) as usize;
    let mut out = values.to_vec();
    for _ in 0..3 {
        out = box_sum(&out, w, h, radius)
            .into_par_iter()
            .enumerate()
            .map(|(i, s)| s / (span(i % w, w, radius) * span(i / w, h, radius)) as f32)
            .collect();
    }
    out
}

/// How many places within `0..n` lie within `radius` of `x`.
fn span(x: usize, n: usize, radius: usize) -> usize {
    (x + radius).min(n - 1) + 1 - x.saturating_sub(radius)
}

/// Per pixel the sum over the square of `radius` around it, within the
/// image.
fn box_sum(values: &[f32], w: usize, h: usize, radius: usize) -> Vec<f32> {
    let rows = row_sums(values, w, radius);
    let columns = row_sums(&transpose(&rows, w, h), h, radius);
    transpose(&columns, h, w)
}

/// Per place the sum along its row (of `w`) within `radius` of it.
fn row_sums(values: &[f32], w: usize, radius: usize) -> Vec<f32> {
    let mut out = vec![0f32; values.len()];
    out.par_chunks_mut(w)
        .zip(values.par_chunks(w))
        .for_each(|(out, row)| {
            let mut sum = 0f64;
            for v in row.iter().take(radius.min(w - 1) + 1) {
                sum += f64::from(*v);
            }
            for x in 0..w {
                out[x] = sum as f32;
                if x + radius + 1 < w {
                    sum += f64::from(row[x + radius + 1]);
                }
                if x >= radius {
                    sum -= f64::from(row[x - radius]);
                }
            }
        });
    out
}

/// The `w` × `h` image `values` with its rows and columns swapped.
fn transpose(values: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0f32; values.len()];
    out.par_chunks_mut(h).enumerate().for_each(|(x, column)| {
        for (y, v) in column.iter_mut().enumerate() {
            *v = values[y * w + x];
        }
    });
    out
}

/// What a frame left out of the merge saw where its mask is whole: per
/// pixel, its place, its light in the darkest frame's units and whether it
/// clipped there. The fallback where no frame is left to merge.
pub(crate) struct LeftOut {
    /// The frame's place in merge order, from the darkest.
    pub frame: usize,
    pub pixels: Vec<(u32, [f32; 3], bool)>,
}

impl LeftOut {
    /// The pixels where `mask` is whole, the frame, of `gain` over the
    /// darkest frame, covers the image and `reference` has no weight in the
    /// merge: only there can no frame be left. `None` for a reference that
    /// is the darkest frame, whose weight the merge never takes away.
    pub(crate) fn new(
        frame_at: usize,
        frame: &Frame,
        gain: f64,
        mask: &[f32],
        covered: Option<&[bool]>,
        reference: Option<&Frame>,
    ) -> Self {
        let g = gain as f32;
        let pixels = match reference {
            None => Vec::new(),
            Some(reference) => mask
                .par_iter()
                .enumerate()
                .filter(|(i, m)| {
                    **m >= 1. && covered.is_none_or(|c| c[*i]) && bounded(reference, *i)
                })
                .map(|(i, _)| (i as u32, frame.pixels[i].map(|v| v / g), bounded(frame, i)))
                .collect(),
        };
        Self {
            frame: frame_at,
            pixels,
        }
    }

    fn at(&self, i: u32) -> Option<([f32; 3], bool)> {
        self.pixels
            .binary_search_by_key(&i, |p| p.0)
            .ok()
            .map(|k| (self.pixels[k].1, self.pixels[k].2))
    }
}

/// Fills the merge's pixels that no frame was merged into (NaN in `light`)
/// from the frames left out there, one frame for each connected region; the
/// rest of them, which no frame covers, are 0.
pub(crate) fn fill(light: &mut [[f32; 3]], width: usize, left_out: &[LeftOut]) {
    let empty = |p: &[f32; 3]| p[0].is_nan();
    let fillable = |i: usize| left_out.iter().any(|l| l.at(i as u32).is_some());
    if !left_out.is_empty() && light.iter().any(empty) {
        let mut seen = vec![false; light.len()];
        let height = light.len() / width.max(1);
        for start in 0..light.len() {
            if seen[start] || !empty(&light[start]) || !fillable(start) {
                continue;
            }
            // The region: the empty pixels a frame left out saw, connected
            // side to side.
            let mut region = vec![start];
            seen[start] = true;
            let mut k = 0;
            while k < region.len() {
                let i = region[k];
                k += 1;
                let (x, y) = (i % width, i / width);
                let mut visit = |j: usize| {
                    if !seen[j] && empty(&light[j]) && fillable(j) {
                        seen[j] = true;
                        region.push(j);
                    }
                };
                if x > 0 {
                    visit(i - 1);
                }
                if x + 1 < width {
                    visit(i + 1);
                }
                if y > 0 {
                    visit(i - width);
                }
                if y + 1 < height {
                    visit(i + width);
                }
            }
            // The brightest frame that saw all of it unclipped, else the
            // darkest that saw all of it.
            let whole = |l: &&LeftOut| region.iter().all(|&i| l.at(i as u32).is_some());
            let unclipped =
                |l: &&LeftOut| region.iter().all(|&i| l.at(i as u32).is_some_and(|p| !p.1));
            let chosen = left_out
                .iter()
                .filter(whole)
                .filter(unclipped)
                .max_by_key(|l| l.frame)
                .or_else(|| left_out.iter().filter(whole).min_by_key(|l| l.frame));
            for &i in &region {
                let from = chosen.and_then(|l| l.at(i as u32)).or_else(|| {
                    // No one frame saw all of it: each pixel from the
                    // brightest that saw it unclipped, else the darkest.
                    let mut here: Vec<_> = left_out
                        .iter()
                        .filter_map(|l| l.at(i as u32).map(|p| (l.frame, p)))
                        .collect();
                    here.sort_by_key(|h| h.0);
                    here.iter()
                        .rev()
                        .find(|h| !h.1.1)
                        .or(here.first())
                        .map(|h| h.1)
                });
                if let Some((p, _)) = from {
                    light[i] = p;
                }
            }
        }
    }
    light.par_iter_mut().for_each(|p| {
        if empty(p) {
            *p = [0.; 3];
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use rawmakase_model::camera_data::Metadata;

    const W: usize = 600;
    const H: usize = 400;

    /// A small deterministic generator of normal deviates.
    struct Random(u64);
    impl Random {
        fn uniform(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        }
        fn normal(&mut self) -> f64 {
            let (a, b) = (self.uniform(), self.uniform());
            (-2. * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos()
        }
    }

    fn noise() -> Noise {
        Noise {
            shot: 1. / 20_000.,
            read: 1e-7,
        }
    }

    /// A frame of `light` (per pixel, 1 = white) `exposure` times over, with
    /// the noise of [`noise`], clipped at 1.
    fn frame(light: impl Fn(usize, usize) -> f64, exposure: f64, seed: u64) -> Frame {
        let noise = noise();
        let mut r = Random(seed);
        let mut pixels = Vec::with_capacity(W * H);
        let mut clipped = Vec::with_capacity(W * H);
        for y in 0..H {
            for x in 0..W {
                let s = light(x, y) * exposure;
                let p: [f32; 3] = std::array::from_fn(|_| {
                    (s + noise.variance(s).sqrt() * r.normal()).min(1.) as f32
                });
                clipped.push(s >= 1.);
                pixels.push(p);
            }
        }
        Frame {
            width: W as u32,
            height: H as u32,
            pixels,
            clipped,
            saturation: [1.; 3],
            metadata: Metadata::default(),
        }
    }

    /// A scene of soft stripes and a gradient, nowhere flat.
    fn scene(x: usize, y: usize) -> f64 {
        let (x, y) = (x as f64, y as f64);
        0.02 + 0.015 * (x / 37.).sin() * (y / 23.).cos() + 0.03 * x / W as f64
    }

    fn masked(m: &Option<Vec<f32>>) -> usize {
        m.as_ref()
            .map_or(0, |m| m.iter().filter(|v| **v > 0.).count())
    }

    /// Frames of the same scene, only their noise different, leave no mask
    /// at any level, two stops apart either way.
    #[test]
    fn noise_alone_leaves_no_mask() {
        let reference = frame(scene, 4., 1);
        for (exposure, seed) in [(1., 2), (16., 3)] {
            let other = frame(scene, exposure, seed);
            for level in [Deghost::Low, Deghost::Medium, Deghost::High] {
                let got = mask(
                    &reference,
                    noise(),
                    &other,
                    noise(),
                    4. / exposure,
                    None,
                    level.threshold().unwrap(),
                    Cancel::NEVER,
                )
                .unwrap();
                assert_eq!(masked(&got), 0, "{level:?} at {exposure}");
            }
        }
        assert_eq!(Deghost::None.threshold(), None);
    }

    /// Something in a frame that the reference doesn't show is masked whole,
    /// with its feather; what agrees beyond is not.
    #[test]
    fn what_moved_is_masked() {
        let block = |x: usize, y: usize| (200..260).contains(&x) && (150..210).contains(&y);
        let reference = frame(scene, 4., 1);
        let other = frame(|x, y| if block(x, y) { 0.005 } else { scene(x, y) }, 1., 2);
        for level in [Deghost::Low, Deghost::Medium, Deghost::High] {
            let m = mask(
                &reference,
                noise(),
                &other,
                noise(),
                4.,
                None,
                level.threshold().unwrap(),
                Cancel::NEVER,
            )
            .unwrap()
            .expect("a mask");
            for y in 150..210 {
                for x in 200..260 {
                    assert_eq!(m[y * W + x], 1., "{level:?} at {x}, {y}");
                }
            }
            // The feather: the dilation (3 px here) and about two standard
            // deviations of the blur beyond, no further.
            assert_eq!(m[180 * W + 262], 1.);
            assert!(m[180 * W + 266] > 0.);
            assert_eq!(m[180 * W + 280], 0.);
            assert_eq!(m[20 * W + 20], 0.);
        }
    }

    /// Where the reference clipped, a frame that saw something darker than
    /// the clip there disagrees; one that agrees with the clip does not.
    #[test]
    fn a_clipped_reference_is_a_lower_bound() {
        let window = |x: usize, y: usize| (100..400).contains(&x) && (100..300).contains(&y);
        // The reference clips in the window: its light is 0.4, four times
        // over.
        let light = |x, y| if window(x, y) { 0.4 } else { scene(x, y) };
        let reference = frame(light, 4., 1);
        let agrees = frame(light, 1., 2);
        let got = mask(
            &reference,
            noise(),
            &agrees,
            noise(),
            4.,
            None,
            3.,
            Cancel::NEVER,
        )
        .unwrap();
        assert_eq!(masked(&got), 0);
        // A person, at a tenth of the window's light, walked into it.
        let person = |x: usize, y: usize| (200..240).contains(&x) && (150..250).contains(&y);
        let walked = frame(|x, y| if person(x, y) { 0.04 } else { light(x, y) }, 1., 3);
        let m = mask(
            &reference,
            noise(),
            &walked,
            noise(),
            4.,
            None,
            3.,
            Cancel::NEVER,
        )
        .unwrap()
        .expect("a mask");
        assert_eq!(m[200 * W + 220], 1.);
        assert_eq!(m[200 * W + 350], 0.);
    }

    /// The feather's blur and the morphology keep to the image's edges.
    #[test]
    fn sums_stay_within_the_image() {
        let ones = vec![1f32; 7 * 5];
        assert_eq!(erode(&ones, 7, 5, 2), ones);
        let blurred = blur(&ones, 7, 5, 1.5);
        assert!(blurred.iter().all(|v| (v - 1.).abs() < 1e-6));
        let mut dot = vec![0f32; 7 * 5];
        dot[2 * 7 + 3] = 1.;
        assert_eq!(erode(&dot, 7, 5, 1), vec![0.; 35]);
        let grown = dilate(&dot, 7, 5, 1);
        assert_eq!(grown.iter().filter(|v| **v == 1.).count(), 9);
        assert_eq!(grown[7 + 2], 1.);
        assert_eq!(grown[7 + 1], 0.);
    }

    /// A frame left out keeps only what the fill can need: where its mask is
    /// whole, it covers the image and the reference has no weight; nothing
    /// when the reference is the darkest frame.
    #[test]
    fn a_frame_left_out_keeps_only_where_the_reference_has_no_weight() {
        let window = |x: usize, y: usize| (100..400).contains(&x) && (100..300).contains(&y);
        let reference = frame(|x, y| if window(x, y) { 0.4 } else { scene(x, y) }, 4., 1);
        let other = frame(scene, 1., 2);
        let mut mask = vec![0f32; W * H];
        for y in 50..150 {
            for x in 50..150 {
                mask[y * W + x] = 1.;
            }
        }
        // Auto Align moved the frame off the image's left 75 columns.
        let covered: Vec<bool> = (0..W * H).map(|i| i % W >= 75).collect();
        let kept = LeftOut::new(2, &other, 1., &mask, Some(&covered), Some(&reference));
        assert_eq!(kept.pixels.len(), 50 * 50);
        assert!(kept.pixels.iter().all(|p| {
            let (x, y) = (p.0 as usize % W, p.0 as usize / W);
            window(x, y) && mask[y * W + x] == 1.
        }));
        let kept = LeftOut::new(2, &other, 1., &mask, None, None);
        assert!(kept.pixels.is_empty());
    }

    /// An empty region takes all of its pixels from one frame: the
    /// brightest that saw all of it unclipped.
    #[test]
    fn an_empty_region_comes_from_one_frame() {
        let (w, h) = (6, 4);
        let nan = [f32::NAN; 3];
        let mut light = vec![[0.5; 3]; w * h];
        // The region: 1..4 of rows 1 and 2; and one pixel no frame saw.
        let region: Vec<usize> = (1..3)
            .flat_map(|y| (1..4).map(move |x| y * w + x))
            .collect();
        for &i in &region {
            light[i] = nan;
        }
        light[5] = nan;
        let all = |value: f32, clipped_at: Option<usize>| {
            region
                .iter()
                .map(|&i| (i as u32, [value; 3], Some(i) == clipped_at))
                .collect::<Vec<_>>()
        };
        let left_out = [
            LeftOut {
                frame: 0,
                pixels: all(0.1, None),
            },
            LeftOut {
                frame: 1,
                pixels: all(0.2, None),
            },
            // The brightest clipped at one of the region's pixels.
            LeftOut {
                frame: 2,
                pixels: all(0.3, Some(region[2])),
            },
        ];
        fill(&mut light, w, &left_out);
        for &i in &region {
            assert_eq!(light[i], [0.2; 3], "{i}");
        }
        assert_eq!(light[5], [0.; 3]);
        assert_eq!(light[0], [0.5; 3]);
        // Every frame clipped somewhere in it: the darkest.
        let mut light = vec![nan; w * h];
        let left_out = [
            LeftOut {
                frame: 0,
                pixels: all(0.1, Some(region[0])),
            },
            LeftOut {
                frame: 1,
                pixels: all(0.2, Some(region[1])),
            },
        ];
        fill(&mut light, w, &left_out);
        assert_eq!(light[region[3]], [0.1; 3]);
    }
}
