//! A RAW developed as one frame of a bracket for Merge to HDR
//! (crates/rawmakase-hdr-merge): demosaiced as development demosaics it, then its
//! white balance taken off again, with nothing recovered, and with each pixel's
//! clipping marked, so the merge can weigh it against the bracket's other frames.
use super::{Cfa, Raw};
use crate::camera_data::{Demosaic, Metadata};
use crate::demosaic::PATTERN;
use anyhow::{Result, ensure};
use rawmakase_cancel::Cancel;
use rayon::prelude::*;

/// One frame of a bracket, as [`Raw::develop_frame`] makes it.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// Camera RGB, black-subtracted and not white balanced, where 1 is the
    /// sensor's white level, row by row. Highlights are not recovered: a
    /// clipped pixel keeps what the demosaic made of it.
    pub pixels: Vec<[f32; 3]>,
    /// Per pixel, whether a sensor sample the demosaic may have read for it
    /// reached its color's [`Self::saturation`].
    pub clipped: Vec<bool>,
    /// Per color, the level its samples clip at, measured on the frame, at
    /// most 1 (the white level). A sensor that saturates below the white level
    /// its file states clips there.
    pub saturation: [f32; 3],
    pub metadata: Metadata,
}

/// How far, in pixels, a sample can change the demosaic's pixels: three on
/// Bayer (Hamilton–Adams's second differences, then the color difference to
/// the green beside them), and the X-Trans demosaic's candidate reach on X-Trans
/// (its green along a line, then a red or blue as far again). X-Trans's stripe
/// pass averages color along rows and columns of fine stripes beyond that.
const BAYER_REACH: usize = 3;
const XTRANS_REACH: usize = crate::demosaic::XTRANS_REACH;

/// A clipped sample is one within this fraction of its color's saturation.
const CLIP_MARGIN: f32 = 0.005;

/// Saturation is looked for from this level up, in bins this wide: about a
/// DN of a 12-bit sensor.
const LOWEST: f32 = 0.25;
const BIN: f32 = 2.5e-4;
const BINS: usize = ((1. - LOWEST) / BIN) as usize + 1;
/// A bin is a pile when it holds at least [`MIN_SATURATED`] samples and
/// [`PILE`] times as many as the bins a little below it hold on average
/// ([`BELOW`] bins down): a sensor that clips stacks a highlight's samples on
/// one level, while noise spreads an unclipped area's over many bins.
const MIN_SATURATED: u32 = 16;
const PILE: f32 = 8.;
const BELOW: std::ops::RangeInclusive<usize> = 4..=16;

impl Raw {
    /// The photo developed as one frame of a bracket ([`Frame`]): its mosaic
    /// demosaiced with `demosaic`, as [`Raw::develop`] does it, then its white
    /// balance divided out. An error for a RAW without a mosaic (a small RAW or
    /// a linear DNG), which Merge to HDR does not take.
    pub fn develop_frame(self, demosaic: Demosaic, cancel: Cancel<'_>) -> Result<Frame> {
        // The only demosaic there is, which `develop_mosaic` runs; taken so
        // callers capture it with the rest of a merge's inputs, and matched so
        // another one can't be passed and ignored.
        let Demosaic::Rawmakase = demosaic;
        let cfa = self
            .own
            .cfa(cancel)?
            .ok_or_else(|| anyhow::anyhow!("Not a mosaic RAW"))?;
        cancel.check("Development cancelled")?;
        let reach = if self.metadata.xtrans {
            XTRANS_REACH
        } else {
            BAYER_REACH
        };
        let saturation = saturation(&cfa);
        let clipped = dilate(
            &clipped_sites(&cfa, saturation),
            cfa.width as usize,
            cfa.height as usize,
            reach,
        );
        cancel.check("Development cancelled")?;
        let wb = self.metadata.wb;
        ensure!(
            wb.iter().all(|w| w.is_finite() && *w > 0.),
            "No white balance as shot to take off"
        );
        let image = self
            .develop_mosaic(Some(cfa), false, cancel)?
            .ok_or_else(|| anyhow::anyhow!("Not a mosaic RAW"))?;
        let unbalance = wb.map(|w| 1. / w);
        let mut pixels = image.pixels.into_vec();
        pixels.par_iter_mut().for_each(|p| {
            for c in 0..3 {
                p[c] *= unbalance[c];
            }
        });
        cancel.check("Development cancelled")?;
        Ok(Frame {
            width: image.width,
            height: image.height,
            pixels,
            clipped,
            saturation,
            metadata: image.metadata,
        })
    }
}

/// Each color's saturation in `cfa`: the highest level its samples pile up
/// on (the lower edge of that [`BIN`]), else the white level; never above the
/// white level. Samples above white count as at white.
fn saturation(cfa: &Cfa) -> [f32; 3] {
    let width = cfa.width as usize;
    let counts = cfa
        .data
        .par_chunks(width.max(1))
        .enumerate()
        .fold(
            || vec![[0u32; BINS]; 3],
            |mut counts, (y, row)| {
                for (x, v) in row.iter().enumerate() {
                    if *v >= LOWEST {
                        let c = cfa.pattern[(y % PATTERN) * PATTERN + x % PATTERN] as usize;
                        let bin = (((v.min(1.) - LOWEST) / BIN) as usize).min(BINS - 1);
                        counts[c][bin] += 1;
                    }
                }
                counts
            },
        )
        .reduce(
            || vec![[0u32; BINS]; 3],
            |mut a, b| {
                for (a, b) in a.iter_mut().zip(&b) {
                    for (a, b) in a.iter_mut().zip(b) {
                        *a += b;
                    }
                }
                a
            },
        );
    std::array::from_fn(|c| {
        let counts = &counts[c];
        (*BELOW.end()..BINS)
            .rev()
            .find(|&bin| {
                let below = BELOW.clone().map(|d| counts[bin - d] as f32).sum::<f32>()
                    / BELOW.clone().count() as f32;
                counts[bin] >= MIN_SATURATED && counts[bin] as f32 >= PILE * below.max(1.)
            })
            .map_or(1., |bin| {
                if bin == BINS - 1 {
                    1.
                } else {
                    LOWEST + bin as f32 * BIN
                }
            })
    })
}

/// Whether each sample of `cfa` reaches its color's `saturation`.
fn clipped_sites(cfa: &Cfa, saturation: [f32; 3]) -> Vec<bool> {
    let width = cfa.width as usize;
    let floor = saturation.map(|s| s * (1. - CLIP_MARGIN));
    let mut out = vec![false; cfa.data.len()];
    out.par_chunks_mut(width.max(1))
        .zip(cfa.data.par_chunks(width.max(1)))
        .enumerate()
        .for_each(|(y, (out, row))| {
            for (x, (o, v)) in out.iter_mut().zip(row).enumerate() {
                let c = cfa.pattern[(y % PATTERN) * PATTERN + x % PATTERN] as usize;
                *o = *v >= floor[c];
            }
        });
    out
}

/// `marks` of a `width` × `height` image, each set mark spread over the square
/// `reach` pixels around it.
fn dilate(marks: &[bool], width: usize, height: usize, reach: usize) -> Vec<bool> {
    // Along rows, then down columns: the square is the two in turn.
    let mut across = vec![false; marks.len()];
    across
        .par_chunks_mut(width.max(1))
        .zip(marks.par_chunks(width.max(1)))
        .for_each(|(out, row)| {
            for (x, o) in out.iter_mut().enumerate() {
                let (a, b) = (x.saturating_sub(reach), (x + reach).min(width - 1));
                *o = row[a..=b].iter().any(|m| *m);
            }
        });
    let mut out = vec![false; marks.len()];
    out.par_chunks_mut(width.max(1))
        .enumerate()
        .for_each(|(y, row)| {
            let (a, b) = (y.saturating_sub(reach), (y + reach).min(height - 1));
            for (x, o) in row.iter_mut().enumerate() {
                *o = (a..=b).any(|yy| across[yy * width + x]);
            }
        });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bayer() -> [u8; PATTERN * PATTERN] {
        std::array::from_fn(|i| {
            let (x, y) = (i % PATTERN, i / PATTERN);
            [[0, 1], [1, 2]][y % 2][x % 2]
        })
    }

    /// Normal deviates from a small deterministic generator.
    fn noise(seed: &mut u64) -> f32 {
        let mut uniform = || {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 7;
            *seed ^= *seed << 17;
            ((*seed >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        };
        let (a, b) = (uniform(), uniform());
        ((-2. * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos()) as f32
    }

    /// A 512 × 512 mosaic, every sample `level` with shot noise of a sensor
    /// of 20 000 electrons at white.
    fn plateau(level: f32) -> Cfa {
        let (w, h) = (512usize, 512usize);
        let mut seed = 0x9e3779b97f4a7c15;
        let sigma = (level / 20_000.).sqrt();
        Cfa {
            width: w as u32,
            height: h as u32,
            pattern: bayer(),
            data: (0..w * h)
                .map(|_| level + sigma * noise(&mut seed))
                .collect(),
        }
    }

    /// A sensor that clips below its stated white clips there, whatever hot
    /// pixel lies above; a bright, smooth, unclipped area clips nowhere.
    #[test]
    fn saturation_is_where_samples_pile_up() {
        // A quarter of a million samples at 0.95 of white: hundreds of them
        // within half a percent of the brightest, but no pile.
        let cfa = plateau(0.95);
        assert_eq!(saturation(&cfa), [1.; 3]);
        assert!(clipped_sites(&cfa, [1.; 3]).iter().all(|c| !*c));
        // A highlight clipped at 0.9 of white, as integers clip: every one of
        // its samples the same. A hot pixel sits above it.
        let mut cfa = plateau(0.6);
        let w = cfa.width as usize;
        for v in &mut cfa.data[..w * 16] {
            *v = 0.9;
        }
        cfa.data[w * 100 + 7] = 0.97;
        let s = saturation(&cfa);
        for c in s {
            assert!((0.9 - BIN..=0.9).contains(&c), "{s:?}");
        }
        let sites = clipped_sites(&cfa, s);
        assert!(sites[..w * 16].iter().all(|c| *c));
        assert_eq!(sites[w * 16..].iter().filter(|c| **c).count(), 1);
        // Clipped at the white level, and above it.
        for v in &mut cfa.data[..w * 16] {
            *v = 1.02;
        }
        assert_eq!(saturation(&cfa), [1.; 3]);
    }

    fn xtrans() -> [u8; PATTERN * PATTERN] {
        const X: [[u8; 6]; 6] = [
            [1, 1, 0, 1, 1, 2],
            [1, 1, 2, 1, 1, 0],
            [2, 0, 1, 0, 2, 1],
            [1, 1, 2, 1, 1, 0],
            [1, 1, 0, 1, 1, 2],
            [0, 2, 1, 2, 0, 1],
        ];
        std::array::from_fn(|i| X[(i / PATTERN) % 6][(i % PATTERN) % 6])
    }

    /// Every pixel whose demosaiced value a clipped sample changes is marked
    /// clipped, on Bayer and X-Trans: one sample at a time is clipped in a
    /// mosaic of smooth texture and noise, and the pixels that change must all
    /// be marked. Not on fine stripes, which X-Trans's stripe pass averages
    /// along whole rows (`demosaic::stripes`).
    #[test]
    fn the_marks_reach_every_pixel_a_clipped_sample_changes() {
        for (pattern, reach) in [(bayer(), BAYER_REACH), (xtrans(), XTRANS_REACH)] {
            let (w, h) = (96usize, 96usize);
            let mut seed = 0x2545f4914f6cdd1du64;
            let data: Vec<f32> = (0..w * h)
                .map(|i| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    let (x, y) = ((i % w) as f32, (i / w) as f32);
                    0.4 + 0.1 * (x / 7.).sin() * (y / 9.).cos()
                        + 0.02 * (seed >> 40) as f32 / (1u64 << 24) as f32
                })
                .collect();
            let plain = crate::demosaic::demosaic(&crate::demosaic::Cfa {
                data: &data,
                width: w,
                height: h,
                pattern: &pattern,
                white: [1.; 3],
            });
            for (cx, cy) in [(48, 48), (49, 48), (48, 49), (50, 51), (47, 50), (51, 47)] {
                let mut clipped = data.clone();
                clipped[cy * w + cx] = 1.;
                let changed = crate::demosaic::demosaic(&crate::demosaic::Cfa {
                    data: &clipped,
                    width: w,
                    height: h,
                    pattern: &pattern,
                    white: [1.; 3],
                });
                let mut marks = vec![false; w * h];
                marks[cy * w + cx] = true;
                let marks = dilate(&marks, w, h, reach);
                for i in 0..w * h {
                    let moved = (0..3).any(|c| (changed[i][c] - plain[i][c]).abs() > 1e-6);
                    assert!(
                        !moved || marks[i],
                        "reach {reach}: ({}, {}) changed by the sample at ({cx}, {cy})",
                        i % w,
                        i / w
                    );
                }
            }
        }
    }

    #[test]
    fn dilation_covers_the_square_around_a_mark() {
        let (w, h) = (9, 7);
        let mut marks = vec![false; w * h];
        marks[3 * w + 4] = true;
        let out = dilate(&marks, w, h, 2);
        for y in 0..h {
            for x in 0..w {
                let near = x.abs_diff(4) <= 2 && y.abs_diff(3) <= 2;
                assert_eq!(out[y * w + x], near, "{x}, {y}");
            }
        }
    }
}
