//! Whether a fitted look may be used: scored on the held-out tiles against the
//! render it replaces.
use super::Rgb;
use crate::color_difference::{delta_e2000, lab};
use serde::Serialize;

/// How a render compares with the preview on a set of pixels, in linear sRGB
/// clamped to 0–1: summed squared error, its root mean, mean CIE76 and CIEDE2000,
/// CIEDE2000 in each cell of a 3 × 3 grid (none for a cell of fewer than 20 pixels),
/// the luminance's squared error and rank correlation (Spearman).
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Scores {
    pub pairs: usize,
    pub sse: f32,
    pub rms: f32,
    pub de76: f32,
    pub de00: f32,
    pub regions_de00: [Option<f32>; 9],
    pub luma_sse: f32,
    pub spearman: f32,
}

fn luma(p: Rgb) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

/// `render` against `target` (both linear sRGB, `width` wide) where `mask` is set.
pub(super) fn score(
    render: &[Rgb],
    target: &[Rgb],
    mask: &[bool],
    width: u32,
    height: u32,
) -> Scores {
    let clamp = |p: Rgb| p.map(|v| if v.is_finite() { v.clamp(0., 1.) } else { 0. });
    let (mut sse, mut luma_sse, mut de76, mut de00) = (0f64, 0f64, 0f64, 0f64);
    let mut regions = [[0f64; 2]; 9];
    let mut lumas = Vec::new();
    for (i, ((r, t), m)) in render.iter().zip(target).zip(mask).enumerate() {
        if !*m {
            continue;
        }
        let (r, t) = (clamp(*r), clamp(*t));
        sse += (0..3).map(|c| f64::from(r[c] - t[c]).powi(2)).sum::<f64>();
        luma_sse += f64::from(luma(r) - luma(t)).powi(2);
        let (a, b) = (lab(r.map(f64::from)), lab(t.map(f64::from)));
        de76 += (0..3).map(|c| (a[c] - b[c]).powi(2)).sum::<f64>().sqrt();
        let d = delta_e2000(a, b);
        de00 += d;
        let (x, y) = (i as u32 % width, i as u32 / width);
        let cell = (y * 3 / height * 3 + x * 3 / width) as usize;
        regions[cell][0] += d;
        regions[cell][1] += 1.;
        lumas.push((luma(r), luma(t)));
    }
    let n = lumas.len();
    let mean = |v: f64| if n > 0 { (v / n as f64) as f32 } else { 0. };
    Scores {
        pairs: n,
        sse: sse as f32,
        rms: mean(sse / 3.).sqrt(),
        de76: mean(de76),
        de00: mean(de00),
        regions_de00: regions.map(|[d, k]| (k >= 20.).then(|| (d / k) as f32)),
        luma_sse: luma_sse as f32,
        spearman: spearman(&lumas),
    }
}

/// Spearman's rank correlation of the pairs: Pearson's of their ranks, tied values
/// sharing their mean rank; 0 when either side is constant.
fn spearman(pairs: &[(f32, f32)]) -> f32 {
    let n = pairs.len();
    if n < 3 {
        return 0.;
    }
    let ranks = |key: fn(&(f32, f32)) -> f32| {
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|a, b| key(&pairs[*a]).total_cmp(&key(&pairs[*b])));
        let mut r = vec![0f64; n];
        let mut i = 0;
        while i < n {
            let mut j = i + 1;
            while j < n && key(&pairs[order[j]]) == key(&pairs[order[i]]) {
                j += 1;
            }
            let mean = (i + j - 1) as f64 / 2.;
            for k in &order[i..j] {
                r[*k] = mean;
            }
            i = j;
        }
        r
    };
    let (a, b) = (ranks(|p| p.0), ranks(|p| p.1));
    let mean = (n - 1) as f64 / 2.;
    let (mut ab, mut aa, mut bb) = (0f64, 0f64, 0f64);
    for (x, y) in a.iter().zip(&b) {
        ab += (x - mean) * (y - mean);
        aa += (x - mean).powi(2);
        bb += (y - mean).powi(2);
    }
    if aa <= 0. || bb <= 0. {
        return 0.;
    }
    (ab / (aa * bb).sqrt()) as f32
}

/// The fewest held-out pairs, and the least share of them in colour (chroma above
/// 10), a colour look must be judged on.
const MIN_PAIRS: usize = 256;
const MIN_COLOURED: f32 = 0.05;

/// The share of `target`'s masked pixels in colour.
pub(super) fn coloured(target: &[Rgb], mask: &[bool]) -> f32 {
    let (mut n, mut c) = (0usize, 0usize);
    for (t, m) in target.iter().zip(mask) {
        if *m {
            n += 1;
            let l = lab(t.map(|v| f64::from(v.clamp(0., 1.))));
            c += usize::from(l[1].hypot(l[2]) > 10.);
        }
    }
    if n == 0 { 0. } else { c as f32 / n as f32 }
}

/// Why a colour look `fit` fails against `baseline`, if it does: its held-out
/// error must be below 0.7 of the baseline's with an RMS of at most 0.1, no cell
/// more than 2 ΔE00 worse, on at least 256 pairs of which 5% are in colour.
pub(super) fn colour_refusal(fit: &Scores, baseline: &Scores, coloured: f32) -> Option<String> {
    if fit.pairs < MIN_PAIRS {
        return Some(format!("Only {} held-out pairs", fit.pairs));
    }
    if coloured < MIN_COLOURED {
        return Some(format!(
            "Only {:.1}% of the pairs are in colour",
            coloured * 100.
        ));
    }
    if fit.sse >= 0.7 * baseline.sse {
        return Some(format!(
            "Held-out error {:.3} is not below 0.7 of the baseline's {:.3}",
            fit.sse, baseline.sse
        ));
    }
    if fit.rms > 0.1 {
        return Some(format!("Held-out RMS {:.3} is above 0.1", fit.rms));
    }
    worse_cell(fit, baseline)
}
/// Why a tone-only or black and white look `fit` fails: its held-out luminance
/// error must be below 0.7 of the baseline's, its luminance rank correlation with
/// the preview at least 0.5, with no cell more than 2 ΔE00 worse.
pub(super) fn tone_refusal(fit: &Scores, baseline: &Scores) -> Option<String> {
    if fit.pairs < MIN_PAIRS {
        return Some(format!("Only {} held-out pairs", fit.pairs));
    }
    if fit.luma_sse >= 0.7 * baseline.luma_sse {
        return Some(format!(
            "Held-out luminance error {:.3} is not below 0.7 of the baseline's {:.3}",
            fit.luma_sse, baseline.luma_sse
        ));
    }
    if fit.spearman < 0.5 {
        return Some(format!(
            "Luminance rank correlation {:.2} is below 0.5",
            fit.spearman
        ));
    }
    worse_cell(fit, baseline)
}
fn worse_cell(fit: &Scores, baseline: &Scores) -> Option<String> {
    let worst = fit
        .regions_de00
        .iter()
        .zip(&baseline.regions_de00)
        .filter_map(|(f, b)| Some((*f)? - (*b)?))
        .fold(f32::NEG_INFINITY, f32::max);
    (worst > 2.).then(|| format!("A part of the photo is {worst:.1} ΔE00 worse than the baseline"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_look_must_beat_the_baseline_everywhere() {
        let (w, h) = (30, 30);
        let target: Vec<Rgb> = (0..w * h)
            .map(|i| {
                [
                    0.2 + (i % w) as f32 / 100.,
                    0.3,
                    0.1 + (i / w) as f32 / 200.,
                ]
            })
            .collect();
        let mask = vec![true; target.len()];
        let off = |d: f32| -> Vec<Rgb> { target.iter().map(|p| p.map(|v| v + d)).collect() };
        let baseline = score(&off(0.1), &target, &mask, w, h);
        let good = score(&off(0.01), &target, &mask, w, h);
        assert!(good.de00 < baseline.de00 && good.spearman > 0.99);
        assert_eq!(colour_refusal(&good, &baseline, 0.5), None);
        assert!(
            colour_refusal(&good, &baseline, 0.01).is_some(),
            "too little colour"
        );
        assert!(
            colour_refusal(&baseline, &baseline, 0.5).is_some(),
            "no better"
        );
        // Better overall, much worse in one cell.
        let mut patchy = off(0.01);
        for y in 0..10 {
            for x in 0..10 {
                patchy[(y * w + x) as usize] = [0.9, 0.05, 0.9];
            }
        }
        let patchy = score(&patchy, &target, &mask, w, h);
        assert!(colour_refusal(&patchy, &baseline, 0.5).is_some());
        assert_eq!(tone_refusal(&good, &baseline), None);
    }

    #[test]
    fn a_cell_the_baseline_matches_exactly_still_counts() {
        let (w, h) = (30, 30);
        let target: Vec<Rgb> = (0..w * h)
            .map(|i| [0.2 + (i % w) as f32 / 100., 0.3, 0.2])
            .collect();
        let mask = vec![true; target.len()];
        let in_first_cell = |i: usize| (i as u32 % w) < 10 && (i as u32 / w) < 10;
        // The baseline is exact in the first cell and off elsewhere; the fit the reverse.
        let shifted = |exact: bool| -> Vec<Rgb> {
            target
                .iter()
                .enumerate()
                .map(|(i, p)| match in_first_cell(i) == exact {
                    true => *p,
                    false => p.map(|v| v + if exact { 0.05 } else { 0.08 }),
                })
                .collect()
        };
        let baseline = score(&shifted(true), &target, &mask, w, h);
        assert_eq!(baseline.regions_de00[0], Some(0.));
        let fit = score(&shifted(false), &target, &mask, w, h);
        assert!(fit.sse < 0.7 * baseline.sse, "{} {}", fit.sse, baseline.sse);
        let refused = colour_refusal(&fit, &baseline, 0.5).expect("refused");
        assert!(refused.contains("worse"), "{refused}");
    }

    #[test]
    fn a_flat_render_has_no_rank_correlation() {
        let rising: Vec<(f32, f32)> = (0..100).map(|i| (0.5, 0.4 + i as f32 / 500.)).collect();
        assert_eq!(spearman(&rising), 0.);
        let tied: Vec<(f32, f32)> = (0..100).map(|i| ((i / 10) as f32, i as f32)).collect();
        let r = spearman(&tied);
        assert!(r > 0.98 && r < 1., "{r}");
    }
}
