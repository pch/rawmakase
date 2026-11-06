//! The half-size checks: a photo's half-size decode against its full decode
//! reduced to the same size, and its time,
//! for `rawmakase half-check` and `scripts/corpus/dataset-half.py`.
//!
//! - Colour: the CIEDE2000 difference of blocks of [`BLOCK`] × [`BLOCK`]
//!   half-size pixels (each block's mean colour, through the photo's camera
//!   to sRGB matrix to CIELAB): mean and 95th percentile at most
//!   [`REFERENCE_SLACK`] above the file's reference.
//! - Detail: the energy of the luminance Laplacian, half-size over the full
//!   decode reduced: its distance from 1 at most [`REFERENCE_SLACK`] above
//!   the reference's.
//! - Time: opening and decoding at half size over opening and developing at
//!   full size, each the fastest of [`RUNS`] runs in the same check: the half
//!   size time at most [`MAX_TIME`] times the reference ratio times the full
//!   time, plus [`TIME_SLACK`] (so files decoded in a few milliseconds do not
//!   fail on noise). Ratios of one run on one machine, so the gate holds on
//!   any machine.
//!
//! The references ([`Reference`]) are LibRaw's half-size decode of the same
//! file, measured the same way, judged with RAWmakase's camera matrix, and
//! recorded once while RAWmakase still linked LibRaw
//! (`tests/corpus/dataset/pixls-half.json` and the dataset's local
//! recordings; the caller passes them). Without a reference a file is held to
//! the absolute limits [`MAX_MEAN`], [`MAX_P95`], [`DETAIL`] and
//! [`FALLBACK_RATIO`]. Two kinds of file take their own time reference: a
//! small RAW develops at full size in both modes, and an X-Trans draft is the
//! full demosaic reduced (it keeps the directional demosaic's freedom from
//! moiré, which a cheaper draft would lose), so their half-size path may cost
//! their full develop: a reference ratio of 1.
//!
//! The full decode is reduced as the half-size decode is laid out: half-size
//! pixel (i, j) is the mean of the full decode's 2 × 2 block at (2i, 2j),
//! reaching one pixel back where an odd side leaves one row or column.
use crate::camera_data::{CameraImage, Decode, Demosaic};
use crate::color_difference::{delta_e2000, lab};
use rawmakase_cancel::Cancel;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Side of the colour check's blocks, in half-size pixels.
pub const BLOCK: usize = 16;
/// Limits of a file without a reference.
pub const MAX_MEAN: f64 = 0.3;
pub const MAX_P95: f64 = 1.;
pub const DETAIL: std::ops::RangeInclusive<f64> = 0.8..=1.25;
/// How far the colour differences and the detail ratio's distance from 1 may
/// exceed the reference's.
pub const REFERENCE_SLACK: f64 = 0.1;
/// How much the half ÷ full time may exceed its reference ratio.
pub const MAX_TIME: f64 = 1.1;
/// Seconds the half-size time may exceed its limit by.
pub const TIME_SLACK: f64 = 0.02;
/// Runs of each decode; the fastest is timed.
pub const RUNS: usize = 3;
/// The reference ratio of a file without a recorded one: a half-size draft
/// costs no more than the full develop. Above 95% of LibRaw's recorded ratios
/// (median 0.72, 95th percentile 0.86).
pub const FALLBACK_RATIO: f64 = 1.;

/// LibRaw's half-size decode of a file, as the check measures ours: what the
/// gates hold ours to. Each value is optional.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Reference {
    /// Half-size time over full develop time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub de_mean: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub de_p95: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<f64>,
}

/// One photo's half-size check.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct HalfCheck {
    pub path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Full and half-size width and height.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full: Option<[u32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub half: Option<[u32; 2]>,
    /// Whether the half-size decode is the full decode's halves (⌈W/2⌉ ×
    /// ⌈H/2⌉); small RAWs develop at full size in both modes and are judged
    /// on their time only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub halved: Option<bool>,
    /// Whether the photo is X-Trans data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xtrans: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub de_mean: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub de_p95: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<f64>,
    /// Seconds to open and decode at half size, and to open and develop at
    /// full size, in the same check.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_seconds: Option<f64>,
    /// The file's reference, when one was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<Reference>,
}

impl HalfCheck {
    /// The gates this check fails, by name; empty when it passes.
    pub fn failures(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.error.is_some() {
            out.push("error");
        }
        // Every file is timed, small RAWs too.
        let slow = match (self.seconds, self.full_seconds, self.time_limit()) {
            (Some(half), Some(full), Some(limit)) => half > limit * full + TIME_SLACK,
            _ => false,
        };
        if self.halved == Some(false) {
            if slow {
                out.push("time");
            }
            return out;
        }
        let reference = self.reference.unwrap_or_default();
        let colour = |value: Option<f64>, reference: Option<f64>, absolute: f64| {
            value.is_some_and(|v| v > reference.map_or(absolute, |r| r + REFERENCE_SLACK))
        };
        if colour(self.de_mean, reference.de_mean, MAX_MEAN) {
            out.push("colour mean");
        }
        if colour(self.de_p95, reference.de_p95, MAX_P95) {
            out.push("colour p95");
        }
        if self.detail.is_some_and(|v| match reference.detail {
            Some(r) => (v - 1.).abs() > (r - 1.).abs() + REFERENCE_SLACK,
            None => !DETAIL.contains(&v),
        }) {
            out.push("detail");
        }
        if slow {
            out.push("time");
        }
        out
    }

    /// The half-size time over the full develop time.
    pub fn time_ratio(&self) -> Option<f64> {
        let (half, full) = (self.seconds?, self.full_seconds?);
        (full > 0.).then(|| half / full)
    }

    /// The most [`Self::time_ratio`] may be before [`TIME_SLACK`]:
    /// [`MAX_TIME`] times the reference ratio (see the module's documentation).
    pub fn time_limit(&self) -> Option<f64> {
        let own = !self.halved? || self.xtrans == Some(true);
        let reference = match own {
            true => 1.,
            false => self
                .reference
                .and_then(|r| r.ratio)
                .unwrap_or(FALLBACK_RATIO),
        };
        Some(MAX_TIME * reference)
    }
}

fn develop(path: &Path, decode: Decode) -> anyhow::Result<(CameraImage, f64)> {
    let mut best = f64::INFINITY;
    let mut image = None;
    for _ in 0..RUNS {
        let t = Instant::now();
        let im = crate::photo::open(path)?.develop(decode, Cancel::NEVER)?;
        best = best.min(t.elapsed().as_secs_f64());
        image = Some(im);
    }
    Ok((image.expect("developed at least once"), best))
}

/// Checks the photo at `path` against its `reference`, when there is one.
pub fn check(path: &Path, reference: Option<Reference>) -> HalfCheck {
    let mut c = HalfCheck {
        path: path.to_path_buf(),
        reference,
        ..Default::default()
    };
    let result = (|| -> anyhow::Result<()> {
        let (half, seconds) = develop(path, Decode::Half)?;
        c.half = Some([half.width, half.height]);
        c.seconds = Some(seconds);
        let (full, full_seconds) = develop(path, Decode::Full(Demosaic::Rawmakase))?;
        c.full = Some([full.width, full.height]);
        c.full_seconds = Some(full_seconds);
        c.xtrans = Some(full.metadata.xtrans);
        let halved = [half.width, half.height] == [full.width.div_ceil(2), full.height.div_ceil(2)];
        c.halved = Some(halved);
        if halved {
            let (mean, p95, detail) = compare(&full, &half);
            (c.de_mean, c.de_p95, c.detail) = (Some(mean), Some(p95), Some(detail));
        }
        Ok(())
    })();
    if let Err(e) = result {
        c.error = Some(format!("{e:#}"));
    }
    c
}

/// Block CIEDE2000 mean and 95th percentile, and the detail ratio, of `half`
/// against `full` reduced; `half` must be ⌈W/2⌉ × ⌈H/2⌉ of `full`.
pub(crate) fn compare(full: &CameraImage, half: &CameraImage) -> (f64, f64, f64) {
    let (fw, fh) = (full.width as usize, full.height as usize);
    let (hw, hh) = (half.width as usize, half.height as usize);
    let m = full.metadata.matrix;
    let srgb = |p: [f32; 3]| -> [f64; 3] {
        std::array::from_fn(|r| (0..3).map(|c| f64::from(m[r][c]) * f64::from(p[c])).sum())
    };
    let start = |i: usize, n: usize| (2 * i).min(n.saturating_sub(2));
    let mut reduced = vec![[0f64; 3]; hw * hh];
    for j in 0..hh {
        for i in 0..hw {
            let (x0, y0) = (start(i, fw), start(j, fh));
            let (x1, y1) = ((x0 + 2).min(fw), (y0 + 2).min(fh));
            let n = ((x1 - x0) * (y1 - y0)) as f64;
            let mut s = [0f64; 3];
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = srgb(full.pixels[y * fw + x]);
                    for c in 0..3 {
                        s[c] += p[c] / n;
                    }
                }
            }
            reduced[j * hw + i] = s;
        }
    }
    let ours: Vec<[f64; 3]> = half.pixels.iter().map(|&p| srgb(p)).collect();
    let mut de = Vec::new();
    for by in 0..hh / BLOCK {
        for bx in 0..hw / BLOCK {
            let (mut a, mut b) = ([0f64; 3], [0f64; 3]);
            for y in by * BLOCK..(by + 1) * BLOCK {
                for x in bx * BLOCK..(bx + 1) * BLOCK {
                    for c in 0..3 {
                        a[c] += ours[y * hw + x][c];
                        b[c] += reduced[y * hw + x][c];
                    }
                }
            }
            let n = (BLOCK * BLOCK) as f64;
            de.push(delta_e2000(lab(a.map(|v| v / n)), lab(b.map(|v| v / n))));
        }
    }
    de.sort_by(f64::total_cmp);
    let mean = de.iter().sum::<f64>() / de.len().max(1) as f64;
    let p95 = de
        .get(((de.len() as f64 * 0.95) as usize).min(de.len().saturating_sub(1)))
        .copied()
        .unwrap_or(0.);
    let luma = |p: &[f64; 3]| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
    let energy = |v: &[[f64; 3]]| {
        let (mut e, mut n) = (0., 0.);
        for j in 1..hh.saturating_sub(1) {
            for i in 1..hw.saturating_sub(1) {
                let k = j * hw + i;
                let l = 4. * luma(&v[k])
                    - luma(&v[k - 1])
                    - luma(&v[k + 1])
                    - luma(&v[k - hw])
                    - luma(&v[k + hw]);
                e += l * l;
                n += 1.;
            }
        }
        if n > 0. { e / n } else { 0. }
    };
    let reference = energy(&reduced);
    let detail = if reference > 0. {
        energy(&ours) / reference
    } else {
        1.
    };
    (mean, p95, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32, pixel: impl Fn(usize, usize) -> [f32; 3]) -> CameraImage {
        let metadata = crate::camera_data::Metadata {
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        };
        CameraImage {
            width,
            height,
            recovered: Default::default(),
            pixels: (0..(width * height) as usize)
                .map(|k| pixel(k % width as usize, k / width as usize))
                .collect(),
            metadata,
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
        }
    }

    /// The full decode reduced is its own perfect draft; a uniformly
    /// brighter draft fails on colour, a flattened one on detail.
    #[test]
    fn a_draft_is_judged_against_the_full_decode_reduced() {
        let full = image(64, 64, |x, y| {
            let v = 0.2 + 0.3 * ((x / 3 + y / 5) % 2) as f32;
            [v, v * 0.9, v * 0.8]
        });
        let reduce = |gain: f32| {
            image(32, 32, |i, j| {
                let mut s = [0.; 3];
                for (x, y) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let p = full.pixels[(2 * j + y) * 64 + 2 * i + x];
                    for c in 0..3 {
                        s[c] += p[c] / 4.;
                    }
                }
                s.map(|v| v * gain)
            })
        };
        let (mean, p95, detail) = compare(&full, &reduce(1.));
        assert!(
            mean < 1e-4 && p95 < 1e-4 && (detail - 1.).abs() < 1e-5,
            "{mean} {p95} {detail}"
        );
        let brighter = HalfCheck {
            de_mean: Some(compare(&full, &reduce(1.2)).0),
            de_p95: Some(compare(&full, &reduce(1.2)).1),
            halved: Some(true),
            ..Default::default()
        };
        assert_eq!(brighter.failures(), ["colour mean", "colour p95"]);
        let flat = image(32, 32, |_, _| [0.35, 0.315, 0.28]);
        let (_, _, detail) = compare(&full, &flat);
        assert!(detail < 0.01, "{detail}");
    }

    fn reference(ratio: Option<f64>) -> Option<Reference> {
        Some(Reference {
            ratio,
            de_mean: Some(0.4),
            de_p95: Some(1.2),
            detail: Some(1.3),
        })
    }

    /// Colour and detail against the file's reference, plus 0.1; without one,
    /// the absolute limits.
    #[test]
    fn colour_and_detail_are_judged_against_the_reference() {
        let judged = |de_mean, de_p95, detail, reference| HalfCheck {
            halved: Some(true),
            de_mean: Some(de_mean),
            de_p95: Some(de_p95),
            detail: Some(detail),
            reference,
            ..Default::default()
        };
        // Within LibRaw's own values plus 0.1, though above the absolute limits.
        assert!(
            judged(0.49, 1.29, 1.39, reference(None))
                .failures()
                .is_empty()
        );
        // The detail ratio's distance from 1, either side.
        assert!(
            judged(0.2, 0.5, 0.61, reference(None))
                .failures()
                .is_empty()
        );
        assert_eq!(
            judged(0.51, 1.31, 1.41, reference(None)).failures(),
            ["colour mean", "colour p95", "detail"]
        );
        assert_eq!(
            judged(0.2, 0.5, 0.59, reference(None)).failures(),
            ["detail"]
        );
        // Without a reference, the absolute limits.
        assert_eq!(
            judged(0.31, 1.01, 1.26, None).failures(),
            ["colour mean", "colour p95", "detail"]
        );
        assert!(judged(0.29, 0.99, 0.81, None).failures().is_empty());
    }

    /// Time: half ≤ 1.1 × reference ratio × full + 20 ms.
    #[test]
    fn time_is_judged_against_the_reference_ratio_with_slack() {
        let timed = |halved, xtrans, seconds, full: f64, reference| HalfCheck {
            halved: Some(halved),
            xtrans: Some(xtrans),
            seconds: Some(seconds),
            full_seconds: Some(full),
            reference,
            ..Default::default()
        };
        // 1.1 × 0.4 × 1 s + 20 ms = 0.46 s.
        assert!(
            timed(true, false, 0.459, 1., reference(Some(0.4)))
                .failures()
                .is_empty()
        );
        assert_eq!(
            timed(true, false, 0.461, 1., reference(Some(0.4))).failures(),
            ["time"]
        );
        // A fast file: 1.1 × 0.4 × 50 ms + 20 ms = 42 ms.
        assert!(
            timed(true, false, 0.041, 0.05, reference(Some(0.4)))
                .failures()
                .is_empty()
        );
        assert_eq!(
            timed(true, false, 0.043, 0.05, reference(Some(0.4))).failures(),
            ["time"]
        );
        // Without a recording, the fallback ratio.
        let fallback = MAX_TIME * FALLBACK_RATIO + TIME_SLACK;
        assert!(
            timed(true, false, fallback - 0.01, 1., None)
                .failures()
                .is_empty()
        );
        assert_eq!(
            timed(true, false, fallback + 0.01, 1., None).failures(),
            ["time"]
        );
        // X-Trans drafts are the full demosaic reduced: their own full develop
        // is the reference, whatever LibRaw's ratio.
        assert!(
            timed(true, true, 1.1, 1., reference(Some(0.27)))
                .failures()
                .is_empty()
        );
        assert_eq!(
            timed(true, true, 1.2, 1., reference(Some(0.27))).failures(),
            ["time"]
        );
        // A small RAW (not halved) skips the image gates but not the time: it
        // develops at full size in both modes, so its half-size path may cost
        // its full develop.
        let small = HalfCheck {
            de_p95: Some(9.),
            ..timed(false, false, 1.1, 1., reference(Some(0.3)))
        };
        assert!(small.failures().is_empty());
        assert_eq!(
            timed(false, false, 1.2, 1., reference(Some(0.3))).failures(),
            ["time"]
        );
        // Untimed (an error before the full develop): no time gate.
        let untimed = HalfCheck {
            full_seconds: None,
            ..timed(true, false, 9., 1., reference(Some(0.4)))
        };
        assert!(untimed.failures().is_empty());
    }

    #[test]
    fn the_chart_is_checked_at_half_size() {
        let chart = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/corpus/charts/synthetic-d65.dng"
        );
        let c = check(chart.as_ref(), reference(Some(0.5)));
        assert!(c.error.is_none(), "{:?}", c.error);
        assert_eq!(c.reference, reference(Some(0.5)));
        assert_eq!(c.xtrans, Some(false));
        assert!(c.time_ratio().is_some_and(|r| r > 0.));
        assert_eq!(
            (c.full, c.half, c.halved),
            (Some([970, 742]), Some([485, 371]), Some(true))
        );
        assert!(c.de_mean.is_some_and(f64::is_finite) && c.detail.is_some_and(f64::is_finite));
    }
}
