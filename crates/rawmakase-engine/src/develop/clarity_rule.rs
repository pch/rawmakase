//! Positive Clarity in the scene tone stage (`G_base(b) + G_pix(l)`): Camera Raw 18.7's
//! Clarity +100 response decomposes additively into a base-level curve on the full-depth
//! (`1×1`) symmetric local Laplacian base (`MapBase::clarity_laplacian`) plus a
//! pixel-level contrast curve on each pixel's own log2 luminance, scaling linearly with
//! the slider (`0..=1`).
//!
//! Trained in cancellation-free form `G_flat(b) + (G_pix(l) - G_pix(b))` on 72
//! shift-invariant descriptors of the map's blurred histogram, local-Laplacian base
//! histogram and macro-spatial contrast (`clarity_rule_data.rs`), then folded into two
//! 48-knot curves (`G_base = G_flat - G_pix` and `G_pix`) sharing the same `[lo, hi]`
//! domain so `G_base(x) + G_pix(x) == G_flat(x)` with zero grid mismatch on flat areas.
use super::clarity_rule_data::{
    FB1, FB2, FB3, FEATURES, FW1, FW2, FW3, HIDDEN, MEMBERS, MU, PB1, PB2, PB3, PW1, PW2, PW3, SD,
    Z_CLAMP,
};
use super::local_tone::{Curve, blur};
use super::local_tone_rule::monotone;

const LEVELS: usize = 48;
const STAT_BLUR: usize = 4;
const TEXTURE_RADIUS: usize = 2;
const COARSE_RADIUS: usize = 12;
const TOP_BAND: f32 = 2.;
const TEX_CLAMP: f32 = 0.6;
const TEX_TOP_CLAMP: f32 = 0.05;
const LEVEL_WINDOW: f32 = 0.5;
const TEX_AT_CLAMP: f32 = 1.0;
const LEVEL_MIN: usize = 50;
/// Maximum stops below the 1st percentile of log luminance that the dark tail may reach
/// before being clamped: across 322 real training photos `q01 - q001` has median 0.64 EV
/// (90th percentile 1.17 EV), whereas a <1% dead-black optical border or lens corner sits
/// 6–8 stops below `q01` and would otherwise pull `bot`, `range` and `b_skew` out of range.
const DARK_TAIL_GAP: f32 = 1.0;

/// Full-strength (+100) positive Clarity curves `(G_base, G_pix)` for a photo's
/// measurement map (`logs` and local-Laplacian `base`, `w` × `h`).
///
/// A constant image (`hi - lo < 1e-4`) returns exact zero curves so flat frames and
/// mask color/exposure adjustments never introduce spurious gain.
pub(super) fn unit_curves(logs: &[f32], base: &[f32], w: usize, h: usize) -> [Curve; 2] {
    let n = logs.len();
    if n == 0 || base.len() != n {
        return zero_curves(0., 1.);
    }
    let (logs_lo, logs_hi) = logs
        .iter()
        .copied()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
            (lo.min(v), hi.max(v))
        });
    let (raw_base_lo, raw_base_hi) = base
        .iter()
        .copied()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
            (lo.min(v), hi.max(v))
        });
    if (raw_base_hi - raw_base_lo) < 1e-4 && (logs_hi - logs_lo) < 1e-4 {
        return zero_curves(raw_base_lo - 1., raw_base_hi + 1.);
    }

    // Clamp dead-black sensor/optical borders to `q01(logs) - DARK_TAIL_GAP`.
    let floor = {
        let mut buf = logs.to_vec();
        let k = ((0.01 * (n - 1) as f64) as usize).min(n - 1);
        let (_, q01, _) = buf.select_nth_unstable_by(k, f32::total_cmp);
        *q01 - DARK_TAIL_GAP
    };
    let clamped_logs: Vec<f32> = logs.iter().map(|&v| v.max(floor)).collect();
    let clamped_base: Vec<f32> = base.iter().map(|&v| v.max(floor)).collect();
    let logs = clamped_logs.as_slice();
    let base = clamped_base.as_slice();

    let mut order: Vec<u32> = (0..n as u32).collect();
    order.sort_by(|&a, &b| base[a as usize].total_cmp(&base[b as usize]));
    let sorted: Vec<f32> = order.iter().map(|&i| base[i as usize]).collect();
    let (base_lo, base_hi) = (sorted[0], sorted[n - 1]);

    // Compute all spatial box blurs in parallel.
    let (blurs_log, blurs_base) = rayon::join(
        || {
            let (b4, (b2, b12)) = rayon::join(
                || blur(logs, w, h, STAT_BLUR),
                || {
                    rayon::join(
                        || blur(logs, w, h, TEXTURE_RADIUS),
                        || blur(logs, w, h, COARSE_RADIUS),
                    )
                },
            );
            (b4, b2, b12)
        },
        || {
            let ((b12, b24), (b36, (b72, b96))) = rayon::join(
                || rayon::join(|| blur(base, w, h, 12), || blur(base, w, h, 24)),
                || {
                    rayon::join(
                        || blur(base, w, h, 36),
                        || rayon::join(|| blur(base, w, h, 72), || blur(base, w, h, 96)),
                    )
                },
            );
            (b12, b24, b36, b72, b96)
        },
    );
    let (mut levels, smooth2, coarse12) = blurs_log;
    let (b12, b24, b36, b72, b96) = blurs_base;

    levels.sort_by(f32::total_cmp);
    let q4 = |p: f64| levels[((p * (n - 1) as f64) as usize).min(n - 1)];
    let top = levels[n - 1];
    let bot = q4(0.001);
    let mean = (levels.iter().map(|&v| f64::from(v)).sum::<f64>() / n as f64) as f32;
    let lin_mean =
        (levels.iter().map(|&v| f64::from(v).exp2()).sum::<f64>() / n as f64).log2() as f32;
    let (q01, q10, q25, q50, q75, q90, q99) = (
        q4(0.01),
        q4(0.10),
        q4(0.25),
        q4(0.50),
        q4(0.75),
        q4(0.90),
        q4(0.99),
    );
    let c4 = |x: f32| levels.partition_point(|v| *v <= x) as f32 / n as f32;
    let m_top1 = 1. - c4(top - 1.);
    let m_top2 = 1. - c4(top - 2.);

    let (mut sum_tex, mut sum_top, mut count_top) = (0f64, 0f64, 0usize);
    for ((&l, &s), &b) in logs.iter().zip(&smooth2).zip(base) {
        let d = f64::from(l - s) * f64::from(l - s);
        sum_tex += d;
        if b > base_hi - TOP_BAND {
            sum_top += d;
            count_top += 1;
        }
    }
    let tex = (sum_tex / n as f64).sqrt() as f32;
    let tex_top = if count_top > 0 {
        (sum_top / count_top as f64).sqrt() as f32
    } else {
        tex
    };

    // Base-histogram statistics (`sorted` = sorted local-Laplacian base).
    let bq = |p: f64| sorted[((p * (n - 1) as f64) as usize).min(n - 1)];
    let b_top = bq(0.999);
    let b_bot = bq(0.001);
    let b_mean = (sorted.iter().map(|&v| f64::from(v)).sum::<f64>() / n as f64) as f32;
    let b_lin_mean =
        (sorted.iter().map(|&v| f64::from(v).exp2()).sum::<f64>() / n as f64).log2() as f32;
    let (bq01, bq05, bq10, bq25, bq50, bq75, bq90, bq95, bq99) = (
        bq(0.01),
        bq(0.05),
        bq(0.10),
        bq(0.25),
        bq(0.50),
        bq(0.75),
        bq(0.90),
        bq(0.95),
        bq(0.99),
    );
    let cb = |x: f32| sorted.partition_point(|v| *v <= x) as f32 / n as f32;
    let b_m_top1 = 1. - cb(b_top - 1.);
    let b_m_top2 = 1. - cb(b_top - 2.);

    let b_std = stddev(&sorted, f64::from(b_mean));
    let l_mean = logs.iter().map(|&v| f64::from(v)).sum::<f64>() / n as f64;
    let l_std = stddev(logs, l_mean);
    let dlb: Vec<f32> = logs.iter().zip(base).map(|(&l, &b)| l - b).collect();
    let d_mean = dlb.iter().map(|&v| f64::from(v)).sum::<f64>() / n as f64;
    let d_std = stddev(&dlb, d_mean);
    let coh = if l_std > 1e-4 {
        b_std / l_std.max(1e-4)
    } else {
        1.0
    };
    let b_skew = if b_std > 1e-4 {
        let inv = 1. / f64::from(b_std.max(1e-4));
        let bm = f64::from(b_mean);
        let m3 = sorted
            .iter()
            .map(|&v| {
                let t = (f64::from(v) - bm) * inv;
                t * t * t
            })
            .sum::<f64>()
            / n as f64;
        (m3 as f32).clamp(-4., 4.)
    } else {
        0.
    };

    // Global macro-spatial descriptors (`g9`).
    let diff_std = |a: &[f32], b: &[f32]| -> f32 {
        let mut s = 0f64;
        let mut s2 = 0f64;
        for (&x, &y) in a.iter().zip(b) {
            let d = f64::from(x - y);
            s += d;
            s2 += d * d;
        }
        let m = s / n as f64;
        (s2 / n as f64 - m * m).max(0.).sqrt() as f32
    };
    let slice_std = |a: &[f32]| -> f32 {
        let m = a.iter().map(|&v| f64::from(v)).sum::<f64>() / n as f64;
        stddev(a, m)
    };
    let mut adlb_sorted: Vec<f32> = dlb.iter().map(|v| v.abs()).collect();
    let adlb_mean = (adlb_sorted.iter().map(|&v| f64::from(v)).sum::<f64>() / n as f64) as f32;
    let adlb_gt05 = adlb_sorted.iter().filter(|&&v| v > 0.5).count() as f32 / n as f32;
    let p90_idx = ((0.90 * (n - 1) as f64).round() as usize).min(n - 1);
    let (_, adlb_p90, _) = adlb_sorted.select_nth_unstable_by(p90_idx, f32::total_cmp);
    let adlb_p90 = *adlb_p90;
    let g9 = [
        slice_std(&b12),
        slice_std(&b36),
        slice_std(&b96),
        diff_std(base, &b12),
        diff_std(base, &b36),
        diff_std(base, &b96),
        adlb_mean,
        adlb_p90,
        adlb_gt05,
    ];

    // Per-level arrays sorted by base level (`order`), for exact median lookups at each knot.
    let by_order =
        |f: &dyn Fn(usize) -> f32| -> Vec<f32> { order.iter().map(|&i| f(i as usize)).collect() };
    let fine_by = by_order(&|i| (logs[i] - smooth2[i]).abs());
    let coarse_by = by_order(&|i| (logs[i] - coarse12[i]).abs());
    let adlb_by = by_order(&|i| dlb[i].abs());
    let dlb_by = by_order(&|i| dlb[i]);
    let ab24_by = by_order(&|i| (base[i] - b24[i]).abs());
    let db24_by = by_order(&|i| base[i] - b24[i]);
    let db72_by = by_order(&|i| base[i] - b72[i]);

    let globs = [
        median(&fine_by).min(TEX_AT_CLAMP),
        median(&coarse_by).min(TEX_AT_CLAMP),
        median(&adlb_by).clamp(-3., 3.),
        median(&dlb_by).clamp(-3., 3.),
        median(&ab24_by).clamp(-3., 3.),
        median(&db24_by).clamp(-3., 3.),
        median(&db72_by).clamp(-3., 3.),
    ];

    let lo = bot.min(base_lo) - 1.;
    let hi = top.max(base_hi) + 1.;
    let step = (hi - lo) / LEVELS as f32;

    let mut flat_table = [0f32; LEVELS];
    let mut pix_table = [0f32; LEVELS];
    for i in 0..LEVELS {
        let x = (lo + (i as f32 + 0.5) * step).max(floor);
        let w_lo = sorted.partition_point(|v| *v < x - LEVEL_WINDOW);
        let w_hi = sorted.partition_point(|v| *v <= x + LEVEL_WINDOW);
        let [fine, coarse, m_adlb, m_dlb, m_ab24, m_db24, m_db72] = if w_hi - w_lo >= LEVEL_MIN {
            [
                median(&fine_by[w_lo..w_hi]).min(TEX_AT_CLAMP),
                median(&coarse_by[w_lo..w_hi]).min(TEX_AT_CLAMP),
                median(&adlb_by[w_lo..w_hi]).clamp(-3., 3.),
                median(&dlb_by[w_lo..w_hi]).clamp(-3., 3.),
                median(&ab24_by[w_lo..w_hi]).clamp(-3., 3.),
                median(&db24_by[w_lo..w_hi]).clamp(-3., 3.),
                median(&db72_by[w_lo..w_hi]).clamp(-3., 3.),
            ]
        } else {
            globs
        };
        let m4 = |a: f32, b: f32| c4(x + b) - c4(x + a);
        let mb = |a: f32, b: f32| cb(x + b) - cb(x + a);
        let feat: [f32; FEATURES] = [
            // 0..28: RuleStats features
            top - x,
            x - bot,
            mean - x,
            q50 - x,
            q75 - x,
            q90 - x,
            q25 - x,
            q10 - x,
            c4(x),
            top - bot,
            m4(0., 1.),
            m4(1., 2.),
            m4(2., 3.),
            m4(3., 5.),
            m4(-1., 0.),
            m4(-2., -1.),
            m4(-3., -2.),
            m4(-5., -3.),
            tex.min(TEX_CLAMP),
            tex_top.min(TEX_TOP_CLAMP) / TEX_TOP_CLAMP,
            fine,
            coarse,
            m_top1,
            m_top2,
            lin_mean - x,
            q99 - x,
            x - q01,
            lin_mean - mean,
            // 28..58: local-Laplacian base histogram & coherence
            b_top - x,
            x - b_bot,
            b_mean - x,
            b_lin_mean - x,
            bq01 - x,
            bq10 - x,
            bq25 - x,
            bq50 - x,
            bq75 - x,
            bq90 - x,
            bq99 - x,
            cb(x),
            b_top - b_bot,
            bq95 - bq05,
            mb(0., 1.),
            mb(1., 2.),
            mb(2., 3.),
            mb(3., 5.),
            mb(-1., 0.),
            mb(-2., -1.),
            mb(-3., -2.),
            mb(-5., -3.),
            b_std,
            l_std,
            d_std,
            coh,
            b_skew,
            b_m_top1,
            b_m_top2,
            b_lin_mean - b_mean,
            // 58..72: macro-spatial & signed per-level contrast
            g9[0],
            g9[1],
            g9[2],
            g9[3],
            g9[4],
            g9[5],
            g9[6],
            g9[7],
            g9[8],
            m_adlb,
            m_dlb,
            m_ab24,
            m_db24,
            m_db72,
        ];
        let z: [f32; FEATURES] =
            std::array::from_fn(|k| ((feat[k] - MU[k]) / SD[k]).clamp(-Z_CLAMP, Z_CLAMP));
        flat_table[i] = eval_ensemble(&z, &FW1, &FB1, &FW2, &FB2, &FW3, &FB3);
        pix_table[i] = eval_ensemble(&z, &PW1, &PB1, &PW2, &PB2, &PW3, &PB3);
    }

    // Project both smooth-gradient output (`x + G_flat(x)`) and fine-detail output
    // (`x + G_pix(x)`) onto nondecreasing curves so Clarity never solarises or inverts tones.
    monotone(&mut flat_table, step);
    monotone(&mut pix_table, step);
    let base_table: [f32; LEVELS] = std::array::from_fn(|i| flat_table[i] - pix_table[i]);

    [
        Curve {
            lo,
            hi,
            table: base_table,
        },
        Curve {
            lo,
            hi,
            table: pix_table,
        },
    ]
}

/// Scales a unit (+100) positive Clarity curve pair by slider `amount` (clamped to `0..=1`).
pub(super) fn scale(unit: &[Curve; 2], amount: f32) -> [Curve; 2] {
    let a = amount.clamp(0., 1.);
    std::array::from_fn(|k| Curve {
        lo: unit[k].lo,
        hi: unit[k].hi,
        table: unit[k].table.map(|v| v * a),
    })
}

fn zero_curves(lo: f32, hi: f32) -> [Curve; 2] {
    let hi = hi.max(lo + 1e-3);
    std::array::from_fn(|_| Curve {
        lo,
        hi,
        table: [0.; LEVELS],
    })
}

fn stddev(v: &[f32], mean: f64) -> f32 {
    if v.is_empty() {
        return 0.;
    }
    let var = v
        .iter()
        .map(|&x| {
            let d = f64::from(x) - mean;
            d * d
        })
        .sum::<f64>()
        / v.len() as f64;
    var.max(0.).sqrt() as f32
}

fn median(v: &[f32]) -> f32 {
    let mut buf = v.to_vec();
    let mid = buf.len() / 2;
    let (_, m, _) = buf.select_nth_unstable_by(mid, f32::total_cmp);
    let hi = *m;
    if buf.len() % 2 == 1 {
        hi
    } else {
        let lo = buf[..mid].iter().copied().fold(f32::NEG_INFINITY, f32::max);
        0.5 * (lo + hi)
    }
}

fn silu(x: f32) -> f32 {
    x / (1. + (-x).exp())
}

fn eval_ensemble(
    z: &[f32; FEATURES],
    w1: &[[[f32; FEATURES]; HIDDEN]; MEMBERS],
    b1: &[[f32; HIDDEN]; MEMBERS],
    w2: &[[[f32; HIDDEN]; HIDDEN]; MEMBERS],
    b2: &[[f32; HIDDEN]; MEMBERS],
    w3: &[[f32; HIDDEN]; MEMBERS],
    b3: &[f32; MEMBERS],
) -> f32 {
    let mut sum = 0f32;
    for m in 0..MEMBERS {
        let mut h1 = [0f32; HIDDEN];
        for j in 0..HIDDEN {
            let mut s = b1[m][j];
            for k in 0..FEATURES {
                s += w1[m][j][k] * z[k];
            }
            h1[j] = silu(s);
        }
        let mut h2 = [0f32; HIDDEN];
        for j in 0..HIDDEN {
            let mut s = b2[m][j];
            for k in 0..HIDDEN {
                s += w2[m][j][k] * h1[k];
            }
            h2[j] = silu(s);
        }
        let mut out = b3[m];
        for k in 0..HIDDEN {
            out += w3[m][k] * h2[k];
        }
        sum += out;
    }
    sum / MEMBERS as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_scene(w: usize, h: usize, offset: f32) -> (Vec<f32>, Vec<f32>) {
        let logs: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 / w as f32, (i / w) as f32 / h as f32);
                offset
                    + 1.8 * (x - 0.5)
                    + 0.9 * (y * 6.0).sin()
                    + if (x - 0.5).hypot(y - 0.5) < 0.2 {
                        1.4
                    } else {
                        0.0
                    }
                    + if (i % 7) == 0 { 0.25 } else { -0.1 }
            })
            .collect();
        let base = blur(&logs, w, h, 4);
        (logs, base)
    }

    #[test]
    fn flat_image_produces_exact_zero_curves() {
        let (w, h) = (32, 24);
        let flat = vec![-3.0; w * h];
        let [cb, cp] = unit_curves(&flat, &flat, w, h);
        assert!(cb.table.iter().all(|&v| v == 0.));
        assert!(cp.table.iter().all(|&v| v == 0.));
    }

    #[test]
    fn positive_clarity_rule_is_shift_invariant() {
        let (w, h) = (64, 48);
        let (logs0, base0) = sample_scene(w, h, -4.0);
        let (logs1, base1) = sample_scene(w, h, -1.5);
        let shift = 2.5;
        let [cb0, cp0] = unit_curves(&logs0, &base0, w, h);
        let [cb1, cp1] = unit_curves(&logs1, &base1, w, h);
        assert!(((cb1.lo - cb0.lo) - shift).abs() < 1e-4);
        assert!(((cb1.hi - cb0.hi) - shift).abs() < 1e-4);
        for i in 0..LEVELS {
            assert!(
                (cb0.table[i] - cb1.table[i]).abs() < 1e-4,
                "base knot {i}: {} vs {}",
                cb0.table[i],
                cb1.table[i]
            );
            assert!(
                (cp0.table[i] - cp1.table[i]).abs() < 1e-4,
                "pix knot {i}: {} vs {}",
                cp0.table[i],
                cp1.table[i]
            );
        }
    }

    #[test]
    fn dead_black_border_strip_does_not_corrupt_midtones() {
        let (w, h) = (128, 96);
        let (logs_clean, base_clean) = sample_scene(w, h, -3.0);
        let mut logs_border = logs_clean.clone();
        let mut base_border = base_clean.clone();
        // 0.5% dead-black optical border strip at -14 EV (like DJI_0023.DNG).
        let strip = (w * h) / 200;
        for i in 0..strip {
            logs_border[i] = -14.0;
            base_border[i] = -13.5;
        }
        let [cb0, cp0] = unit_curves(&logs_clean, &base_clean, w, h);
        let [cb1, cp1] = unit_curves(&logs_border, &base_border, w, h);
        for x in [-3.0f32, -2.0, -1.0] {
            let g0 = cb0.eval(x) + cp0.eval(x);
            let g1 = cb1.eval(x) + cp1.eval(x);
            assert!(
                (g0 - g1).abs() < 0.15,
                "x={x}: clean={g0:.3} EV vs border={g1:.3} EV"
            );
        }
    }

    #[test]
    fn curves_are_monotone_and_scale_linearly() {
        let (w, h) = (64, 48);
        let (logs, base) = sample_scene(w, h, -3.5);
        let unit = unit_curves(&logs, &base, w, h);
        let half = scale(&unit, 0.5);
        let [cb, cp] = &unit;
        let step = (cb.hi - cb.lo) / LEVELS as f32;
        for i in 1..LEVELS {
            let prev_flat = (i - 1) as f32 * step + cb.table[i - 1] + cp.table[i - 1];
            let cur_flat = i as f32 * step + cb.table[i] + cp.table[i];
            assert!(cur_flat + 1e-5 >= prev_flat, "flat inverted at {i}");
            let prev_pix = (i - 1) as f32 * step + cp.table[i - 1];
            let cur_pix = i as f32 * step + cp.table[i];
            assert!(cur_pix + 1e-5 >= prev_pix, "pix inverted at {i}");
        }
        for k in 0..2 {
            for i in 0..LEVELS {
                assert!((half[k].table[i] - 0.5 * unit[k].table[i]).abs() < 1e-6);
            }
        }
    }
}
