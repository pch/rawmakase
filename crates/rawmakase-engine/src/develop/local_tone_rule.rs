//! The global rule of Shadows and Highlights: Camera Raw's gain for a base level is a
//! function of where that level sits in the photo's histogram of base levels (how far
//! below the top, above the bottom and from the mean and quantiles, the mass in the
//! stops around it, the range) and of how textured the photo is at the top. Measured on
//! flat-patch mosaics with controlled histograms (including deep ranges, bimodal
//! histograms and clipped tops) and fitted together with linear-profile training
//! photos, targets counted to 13 stops under white (2026-10-28;
//! docs/scene-tone-stage.md). A flat maximum is a degenerate case for Camera Raw, so the
//! mosaics carry a faint texture. The slider scales the +100 / −100 gain linearly
//! (positive Shadows and negative Highlights exactly at ±50; on 243 training photos
//! Shadows −50 is 0.49 to 0.50 of the lift at −100 in stops and Highlights +50 0.50
//! of that at +100, rising to 0.56 just under the photo's top).
//!
//! Positive Shadows and negative Highlights are the mean of four small networks
//! fitted from different seeds (`local_tone_rule_data.rs`; two hidden layers of
//! 64 units), since one fit's gain at a level varies by 0.1 to 0.3 EV with the
//! seed on real photos. Negative Shadows and positive Highlights are a second ensemble
//! (`local_tone_rule_ext_data.rs`, 2026-11-17), which also reads the level and the
//! photo's top, median and mean themselves: Camera Raw's Highlights +100 lifts the bright
//! areas of a dark photo by up to three stops and those of a bright one hardly at all,
//! which no place in a histogram tells. Its gains are those that give Camera Raw's
//! result through the scene tone stage's default white curve, which follows the gain
//! and compresses everything near the photo's top; where Camera Raw's result was
//! clipped, they come from its render at half the slider
//! (docs/scene-tone-stage.md#the-global-rule). Each is evaluated once per photo at
//! 48 base levels, which gives a [`Curve`] the per-pixel code and the GPU read like any
//! other. With both sliders set, each curve is blended toward a uniform gain at its
//! extreme ([`Pair`], 2026-11-17): Camera Raw's two sliders together are not the product
//! of their single gains. With `RAWMAKASE_RULE_DUMP=<path>` set, each curve built appends
//! its statistics and table to `<path>` as a JSON line and writes the map it read to
//! `<path>.map` (diagnostics; `scene-tone-rule.py taps` trains on those maps and curves).
use super::local_tone::{Curve, blur};
use super::local_tone_rule_data::{
    B1, B2, B3, FEATURES, MU, PB1, PB2, PB3, PW1, PW2, PW3, SD, W1, W2, W3,
};
use super::local_tone_rule_ext_data as ext;
use super::local_tone_rule_pair_data as pair;

/// Number of histogram-relative features produced by [`RuleStats::features`].
const HIST_FEATURES: usize = 28;

/// The controls the rule has a curve for, in the order of [`RuleStats`]' gains.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuleCurve {
    /// Shadows +100.
    ShadowsUp,
    /// Highlights −100.
    HighlightsDown,
    /// Shadows −100.
    ShadowsDown,
    /// Highlights +100.
    HighlightsUp,
}
impl RuleCurve {
    pub(crate) const ALL: [Self; 4] = [
        Self::ShadowsUp,
        Self::HighlightsDown,
        Self::ShadowsDown,
        Self::HighlightsUp,
    ];
    /// Whether the curve is one of Shadows' (else of Highlights').
    fn shadows(self) -> bool {
        matches!(self, Self::ShadowsUp | Self::ShadowsDown)
    }
}

/// Radius (map pixels) of the mean whose residual measures texture.
const TEXTURE_RADIUS: usize = 2;
/// How far below the top (stops) texture is measured for `tex_top`.
const TOP_BAND: f32 = 2.;
/// Texture beyond this (log2 RMS) counts no further: the mosaics measured a step at a flat
/// top and a slow growth after it; photos are always past the step, so the inputs are
/// clamped rather than extrapolated (`scripts/corpus/scene-tone-rule.py`).
const TEX_CLAMP: f32 = 0.6;
const TEX_TOP_CLAMP: f32 = 0.05;
/// Per-level texture: the median absolute fine (radius [`TEXTURE_RADIUS`]) and coarse
/// ([`COARSE_RADIUS`]) residual of the pixels whose base lies within [`LEVEL_WINDOW`] of
/// the level, clamped at [`TEX_AT_CLAMP`]. Camera Raw lifts a smooth dark region far
/// less than a rough one at the same depth (1.95 against 3.21 EV seven stops down on
/// the `ground` probes); a median ignores the few edge pixels whose residual spans
/// stops. Levels with fewer than [`LEVEL_MIN`] pixels take the photo's median.
const COARSE_RADIUS: usize = 12;
const LEVEL_WINDOW: f32 = 0.5;
const TEX_AT_CLAMP: f32 = 1.0;
const LEVEL_MIN: usize = 50;

/// Histogram statistics of a photo's base levels (its measurement copy).
pub(super) struct RuleStats {
    /// The base levels in order: the per-level texture and the pair model read them.
    sorted: Vec<f32>,
    /// The levels the histogram is read from, in order: the log map under a box blur
    /// of radius [`STAT_BLUR`].
    levels: Vec<f32>,
    top: f32,
    bot: f32,
    mean: f32,
    /// Log2 of the linear-light mean of the blurred levels: captures bright-tail energy
    /// that the log-space mean misses.
    lin_mean: f32,
    q01: f32,
    q10: f32,
    q25: f32,
    q50: f32,
    q75: f32,
    q90: f32,
    q99: f32,
    /// Fraction of the map within one and two stops of the top.
    m_top1: f32,
    m_top2: f32,
    tex: f32,
    tex_top: f32,
    /// Absolute fine and coarse residuals of every map pixel, in order of its base level
    /// (`sorted`), for the per-level medians.
    fine_by_level: Vec<f32>,
    coarse_by_level: Vec<f32>,
    fine_median: f32,
    coarse_median: f32,
    /// The curves' extent and the ensemble's gains at [`LEVELS`] levels across it,
    /// computed once per map: the per-level medians are the costly part, and every
    /// control's curve reads the same table (in [`RuleCurve::ALL`]'s order).
    lo: f32,
    hi: f32,
    gains: [[f32; 4]; LEVELS],
    /// Pixel-level contrast curves (`G_pix(l)`) sampled over `[pix_lo, pix_hi]` =
    /// `[bot, top]` (clamped outside so specular highlights and sensor noise do not
    /// extrapolate), in [`RuleCurve::ALL`]'s order.
    pix_lo: f32,
    pix_hi: f32,
    pix_gains: [[f32; 4]; LEVELS],
    /// Whether this photo is an SDR rendered file (`DefaultBlackRender::None`, `W* = 1`),
    /// which trims the uncompensated deep-shadow toe on `ShadowsUp` and the `W* > 1`
    /// shoulder overshoot on `HighlightsUp`.
    rendered: bool,
    /// Recipe `exposure` (EV) already baked into `logs`/`base`, used to anchor the SDR
    /// rendered-file tapers to pre-Exposure file levels.
    exposure: f32,
    /// How Shadows and Highlights act together on this photo ([`Self::pair`]).
    pair: std::sync::OnceLock<Pair>,
}
/// Levels at which the curves sample the rule.
const LEVELS: usize = 48;
/// How far under the 10th percentile (stops) the bottom the second ensemble reads may
/// sit. The bottom is the 0.1th percentile, which a tiny black area or a black sensor
/// border sets on its own, and the rule then reads every mid-tone as far above the
/// bottom: the four training photos whose bottom sat more than four stops under the
/// 10th percentile were its four worst at Shadows −100, their mid-tones 0.3 to 0.9
/// stop too dark, while the eight between three and four stops were as good as the
/// rest (2026-11-17). The first ensemble reads the bottom as it was fitted with.
const BOTTOM_GAP: f32 = 3.5;
/// The histogram's levels (top, bottom, mean, percentiles, masses) are the log map's
/// under a box blur of this radius in map pixels: nine of the map's 512, 1.8% of the
/// frame's width. Camera Raw counts a region in its curve from about 1.5% of the frame's
/// width, whatever the frame's size, while the gain the region takes itself goes by its
/// size in pixels: a bright spot of 0.12% of the frame lifts a dark field 0.63 EV at
/// Shadows +100, and seventy-five spots of the same total area 0.04 EV (gap probes,
/// docs/records/parity-gaps.md, 2026-11-22). So the levels a curve is read at stay the
/// base's, and what the curve is made from is blurred.
const STAT_BLUR: usize = 4;
impl RuleStats {
    /// Test shorthand for [`Self::new_for`] on a RAW measurement map.
    #[cfg(test)]
    pub(super) fn new(logs: &[f32], base: &[f32], w: usize, h: usize) -> Self {
        Self::new_for(logs, base, w, h, false, 0.)
    }

    /// Builds statistics from the map's log2 luminance and base level (`a · log + b`),
    /// `w` × `h`. When `rendered` is `true`, enables the SDR rendered-file extreme tapers
    /// anchored to pre-Exposure levels (`x - exposure`).
    pub(super) fn new_for(
        logs: &[f32],
        base: &[f32],
        w: usize,
        h: usize,
        rendered: bool,
        exposure: f32,
    ) -> Self {
        if let Ok(path) = std::env::var("RAWMAKASE_RULE_DUMP") {
            // Diagnostics: the map's log luminance and base level as raw f32 (w, h, logs, base).
            let mut bytes = Vec::with_capacity(8 + 8 * logs.len());
            bytes.extend((w as f32).to_le_bytes());
            bytes.extend((h as f32).to_le_bytes());
            for v in logs.iter().chain(base) {
                bytes.extend(v.to_le_bytes());
            }
            let _ = std::fs::write(format!("{path}.map"), bytes);
        }
        let mut order: Vec<u32> = (0..base.len() as u32).collect();
        order.sort_by(|&a, &b| base[a as usize].total_cmp(&base[b as usize]));
        let sorted: Vec<f32> = order.iter().map(|&i| base[i as usize]).collect();
        let n = sorted.len();
        let mut levels = blur(logs, w, h, STAT_BLUR);
        levels.sort_by(f32::total_cmp);
        let q = |p: f32| levels[((p * (n - 1) as f32) as usize).min(n - 1)];
        let mean = (levels.iter().map(|v| *v as f64).sum::<f64>() / n as f64) as f32;
        let lin_mean =
            (levels.iter().map(|v| f64::from(*v).exp2()).sum::<f64>() / n as f64).log2() as f32;
        // The maximum, not a percentile: a bright object sets the top, and the mass
        // features then see it as a small mass (the textured-square probes).
        let stat_top = levels[n - 1];
        // The band the top's texture is read in is the base's.
        let top = sorted[n - 1];
        let smooth = blur(logs, w, h, TEXTURE_RADIUS);
        let (mut sum, mut sum_top, mut count_top) = (0f64, 0f64, 0usize);
        for ((l, s), b) in logs.iter().zip(&smooth).zip(base) {
            let d = f64::from(l - s) * f64::from(l - s);
            sum += d;
            if *b > top - TOP_BAND {
                sum_top += d;
                count_top += 1;
            }
        }
        let tex = (sum / n as f64).sqrt() as f32;
        let tex_top = if count_top > 0 {
            (sum_top / count_top as f64).sqrt() as f32
        } else {
            tex
        };
        let coarse = blur(logs, w, h, COARSE_RADIUS);
        let fine_by_level: Vec<f32> = order
            .iter()
            .map(|&i| (logs[i as usize] - smooth[i as usize]).abs())
            .collect();
        let coarse_by_level: Vec<f32> = order
            .iter()
            .map(|&i| (logs[i as usize] - coarse[i as usize]).abs())
            .collect();
        let fine_median = median(&fine_by_level);
        let coarse_median = median(&coarse_by_level);
        let (base_lo, base_hi) = (sorted[0], top);
        let top = stat_top;
        let mut stats = Self {
            top,
            bot: q(0.001),
            mean,
            lin_mean,
            q01: q(0.01),
            q10: q(0.1),
            q25: q(0.25),
            q50: q(0.5),
            q75: q(0.75),
            q90: q(0.9),
            q99: q(0.99),
            m_top1: 1. - levels.partition_point(|v| *v <= top - 1.) as f32 / n as f32,
            m_top2: 1. - levels.partition_point(|v| *v <= top - 2.) as f32 / n as f32,
            tex,
            tex_top,
            sorted,
            levels,
            fine_by_level,
            coarse_by_level,
            fine_median,
            coarse_median,
            lo: 0.,
            hi: 0.,
            gains: [[0.; 4]; LEVELS],
            pix_lo: 0.,
            pix_hi: 0.,
            pix_gains: [[0.; 4]; LEVELS],
            rendered,
            exposure,
            pair: Default::default(),
        };
        // The curves span the base levels too: a small region's level lies outside the
        // blurred histogram's ends, and the rule was fitted at such levels as they are.
        stats.lo = stats.bot.min(base_lo) - 1.;
        stats.hi = stats.top.max(base_hi) + 1.;
        let step = (stats.hi - stats.lo) / LEVELS as f32;
        stats.gains = std::array::from_fn(|i| stats.gain(stats.lo + (i as f32 + 0.5) * step));
        stats.pix_lo = stats.bot;
        stats.pix_hi = stats.top.max(stats.bot + 1e-3);
        let pix_step = (stats.pix_hi - stats.pix_lo) / LEVELS as f32;
        stats.pix_gains =
            std::array::from_fn(|i| stats.gain_pix(stats.pix_lo + (i as f32 + 0.5) * pix_step));
        stats
    }
    /// Fine and coarse texture of the pixels within [`LEVEL_WINDOW`] of base level `x`.
    fn texture_at(&self, x: f32) -> (f32, f32) {
        let lo = self.sorted.partition_point(|v| *v < x - LEVEL_WINDOW);
        let hi = self.sorted.partition_point(|v| *v <= x + LEVEL_WINDOW);
        if hi - lo < LEVEL_MIN {
            return (
                self.fine_median.min(TEX_AT_CLAMP),
                self.coarse_median.min(TEX_AT_CLAMP),
            );
        }
        (
            median(&self.fine_by_level[lo..hi]).min(TEX_AT_CLAMP),
            median(&self.coarse_by_level[lo..hi]).min(TEX_AT_CLAMP),
        )
    }
    /// Fraction of the histogram's levels at or below `x`.
    fn cdf(&self, x: f32) -> f32 {
        self.levels.partition_point(|v| *v <= x) as f32 / self.levels.len() as f32
    }
    /// The network's inputs for base level `x` (`scene-tone-rule.py`'s `FEAT`, in order).
    /// The descriptors of level `x`, with the histogram's bottom at `bot`.
    fn features(&self, x: f32, bot: f32) -> [f32; HIST_FEATURES] {
        let m = |lo: f32, hi: f32| self.cdf(x + hi) - self.cdf(x + lo);
        let (fine, coarse) = self.texture_at(x);
        [
            self.top - x,
            x - bot,
            self.mean - x,
            self.q50 - x,
            self.q75 - x,
            self.q90 - x,
            self.q25 - x,
            self.q10 - x,
            self.cdf(x),
            self.top - bot,
            m(0., 1.),
            m(1., 2.),
            m(2., 3.),
            m(3., 5.),
            m(-1., 0.),
            m(-2., -1.),
            m(-3., -2.),
            m(-5., -3.),
            self.tex.min(TEX_CLAMP),
            self.tex_top.min(TEX_TOP_CLAMP) / TEX_TOP_CLAMP,
            fine,
            coarse,
            // How sparse the top is: a small bright spot sets the maximum, and Camera Raw
            // lifts the darks only 0.05 EV more for a square of 0.2 to 12% of the frame a
            // stop or two over the rest (the `partial` and `clipwb` probes).
            self.m_top1,
            self.m_top2,
            self.lin_mean - x,
            self.q99 - x,
            x - self.q01,
            self.lin_mean - self.mean,
        ]
    }
    /// Standardised 28-feature exposure-invariant vector for the first ensemble
    /// (`Shadows +100` and `Highlights −100`, without `BOTTOM_GAP` clamping). Keeping
    /// absolute levels out of the first ensemble avoids extrapolating in HDR where
    /// `top` exceeds the SDR training range.
    fn features_first_z(&self, x: f32) -> [f32; FEATURES] {
        let f = self.features(x, self.bot);
        std::array::from_fn(|i| (f[i] - MU[i]) / SD[i])
    }
    /// Standardised 32-feature vector for the second ensemble (`[28 hist, top, q50, mean, x]`),
    /// with the bottom held within [`BOTTOM_GAP`] of the 10th percentile.
    fn features_ext_z(&self, x: f32) -> [f32; ext::FEATURES] {
        let floor = self.bot.max(self.q10 - BOTTOM_GAP);
        let (x, f) = if floor > self.bot {
            let x = x.max(floor);
            (x, self.features(x, floor))
        } else {
            (x, self.features(x, self.bot))
        };
        let absolute = [self.top, self.q50, self.mean, x];
        std::array::from_fn(|i| {
            let v = if i < HIST_FEATURES {
                f[i]
            } else {
                absolute[i - HIST_FEATURES]
            };
            (v - ext::MU[i]) / ext::SD[i]
        })
    }
    /// Log2 gain at base level `x`, in [`RuleCurve::ALL`]'s order (each ensemble's mean).
    fn gain(&self, x: f32) -> [f32; 4] {
        let z = self.features_first_z(x);
        let [up, down] = ensemble(&z, &W1, &B1, &W2, &B2, &W3, &B3);
        let z_ext = self.features_ext_z(x);
        let [shadows_down, highlights_up] = ensemble(
            &z_ext,
            &ext::W1,
            &ext::B1,
            &ext::W2,
            &ext::B2,
            &ext::W3,
            &ext::B3,
        );
        [up, down, shadows_down, highlights_up]
    }
    /// Pixel-level contrast gain `G_pix(l)` at log2 luminance `x`, in [`RuleCurve::ALL`]'s order.
    fn gain_pix(&self, x: f32) -> [f32; 4] {
        let z = self.features_first_z(x);
        let [up, down] = ensemble(&z, &PW1, &PB1, &PW2, &PB2, &PW3, &PB3);
        let z_ext = self.features_ext_z(x);
        let [shadows_down, highlights_up] = ensemble(
            &z_ext,
            &ext::PW1,
            &ext::PB1,
            &ext::PW2,
            &ext::PB2,
            &ext::PW3,
            &ext::PB3,
        );
        [up, down, shadows_down, highlights_up]
    }
    /// The curve of one control at full strength over the photo's base levels, as
    /// [`Curve`] reads it: absolute levels (key 0) from a stop below the bottom to a
    /// stop above the top, held outside. Shadows changes nothing above the top and
    /// Highlights nothing below the bottom, so holding the ends extends the rule as
    /// the mosaics measured it.
    pub(super) fn curve(&self, kind: RuleCurve) -> Curve {
        let (lo, hi) = (self.lo, self.hi);
        let step = (hi - lo) / LEVELS as f32;
        let k = RuleCurve::ALL.iter().position(|c| *c == kind).unwrap_or(0);
        let shadows = kind == RuleCurve::ShadowsUp;
        let mut table: [f32; LEVELS] = std::array::from_fn(|i| self.gains[i][k]);
        if self.rendered {
            for (i, g) in table.iter_mut().enumerate() {
                let x_pre = lo + (i as f32 + 0.5) * step - self.exposure;
                match kind {
                    // Rendered files have no `default_black` toe, so `ShadowsUp` does not
                    // need the extra deep-shadow lift fitted to compensate for it.
                    RuleCurve::ShadowsUp => {
                        let u = ((x_pre - (-11.0)) / (-7.0 - (-11.0))).clamp(0., 1.);
                        let w = 0.65 + 0.35 * (u * u * (3. - 2. * u));
                        *g *= w;
                    }
                    // Rendered files pin `W* = 1` (no highlight shoulder compression), so
                    // `HighlightsUp` trims the extra near-white lift fitted against `W* > 1`.
                    RuleCurve::HighlightsUp => {
                        let u = ((x_pre - (-2.5)) / (0.0 - (-2.5))).clamp(0., 1.);
                        let w = 1.0 - 0.20 * (u * u * (3. - 2. * u));
                        *g *= w;
                    }
                    RuleCurve::HighlightsDown | RuleCurve::ShadowsDown => {}
                }
            }
        }
        monotone(&mut table, step);
        let curve = Curve { lo, hi, table };
        if let Ok(path) = std::env::var("RAWMAKASE_RULE_DUMP") {
            let pix = self.curve_pix(kind, &curve);
            let (pix_lo, pix_hi, pix_table) = match pix {
                Some(p) => (p.lo, p.hi, p.table),
                None => (self.pix_lo, self.pix_hi, [0.; LEVELS]),
            };
            // Diagnostics: the statistics and curve, one JSON line per curve.
            let line = format!(
                "{{\"shadows\":{shadows},\"top\":{},\"bot\":{},\"mean\":{},\"q\":[{},{},{},{},{}],\"tex\":{},\"tex_top\":{},\"n\":{},\"lo\":{lo},\"hi\":{hi},\"table\":{:?},\"pix_lo\":{pix_lo},\"pix_hi\":{pix_hi},\"pix_table\":{:?}}}\n",
                self.top,
                self.bot,
                self.mean,
                self.q10,
                self.q25,
                self.q50,
                self.q75,
                self.q90,
                self.tex,
                self.tex_top,
                self.sorted.len(),
                table,
                pix_table,
            );
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
        }
        curve
    }
    /// Pixel-level contrast curve (`G_pix(l)`) of one control at full strength over
    /// `[bot, top]`, held flat outside so specular highlights and sensor noise do not
    /// extrapolate. Projected so both `l + G_pix(l)` (detail at a fixed base) and
    /// `l + G_base(l) + G_pix(l)` (smooth gradients where `base ≈ l`) stay nondecreasing.
    pub(super) fn curve_pix(&self, kind: RuleCurve, base: &Curve) -> Option<Curve> {
        let k = RuleCurve::ALL.iter().position(|c| *c == kind)?;
        let (lo, hi) = (self.pix_lo, self.pix_hi);
        let step = (hi - lo) / LEVELS as f32;
        let mut table: [f32; LEVELS] = std::array::from_fn(|i| self.pix_gains[i][k]);
        let mut pos = [0f32; LEVELS];
        let mut prev_base = base.eval(lo + 0.5 * step);
        for i in 1..LEVELS {
            let cur_base = base.eval(lo + (i as f32 + 0.5) * step);
            let delta = step.min(step + (cur_base - prev_base)).max(0.);
            pos[i] = pos[i - 1] + delta;
            prev_base = cur_base;
        }
        monotone_pos(&mut table, &pos);
        Some(Curve { lo, hi, table })
    }
    /// How Shadows and Highlights act together on this photo, from its four unit
    /// curves `units` ([`RuleCurve::ALL`]'s order; [`Self::curve`]'s). Kept with the
    /// statistics: the curves are the photo's alone.
    pub(super) fn pair(&self, units: [&Curve; 4]) -> &Pair {
        self.pair.get_or_init(|| {
            let n = self.sorted.len();
            let level = |q: f32| self.sorted[((n - 1) as f32 * q) as usize];
            let (low, high) = (level(PAIR_LOW), level(PAIR_HIGH));
            let extremes: [f32; 4] = std::array::from_fn(|k| {
                units[k].eval(if RuleCurve::ALL[k].shadows() {
                    low
                } else {
                    high
                })
            });
            // How much of the photo each curve reaches: the mean over the map of its
            // gain as a fraction of the extreme's.
            let reach: [f32; 4] = std::array::from_fn(|k| {
                if extremes[k] == 0. {
                    return 0.;
                }
                let sum: f64 = self
                    .sorted
                    .iter()
                    .map(|b| f64::from((units[k].eval(*b) / extremes[k]).clamp(0., 1.)))
                    .sum();
                (sum / n as f64) as f32
            });
            let mut fractions = [[0f32; 2]; 4];
            let mut shifts = [0f32; 4];
            for c in 0..4 {
                let [s, h] = Pair::KINDS[c].map(|kind| kind as usize);
                let size = |k: usize| extremes[k].abs();
                let x = [
                    reach[s],
                    reach[h],
                    size(s),
                    size(h),
                    1. / size(s).max(PAIR_EXTREME_MIN),
                    1. / size(h).max(PAIR_EXTREME_MIN),
                    self.top,
                    self.q50,
                    self.mean,
                    self.bot,
                    self.top - self.q50,
                    self.q50 - self.bot,
                    self.q90 - self.q10,
                    self.lin_mean - self.mean,
                ];
                let eval = |row: usize| {
                    let w = &pair::BETA[c][row];
                    x.iter()
                        .zip(&pair::MU[c])
                        .zip(&pair::SD[c])
                        .zip(&w[1..])
                        .fold(w[0], |sum, (((&v, &mu), &sd), &w)| {
                            let z = ((v - mu) / sd).clamp(-PAIR_Z_CLAMP, PAIR_Z_CLAMP);
                            sum + w * z
                        })
                };
                fractions[c] = [eval(0).clamp(0., 1.), eval(1).clamp(0., 1.)];
                shifts[c] = eval(2).clamp(-PAIR_SHIFT_MAX, PAIR_SHIFT_MAX);
            }
            Pair {
                extremes,
                fractions,
                shifts,
            }
        })
    }
}
/// The quantiles of the map's base levels at which a Shadows curve and a Highlights
/// curve are read for their extreme ([`Pair::extremes`]): the level the photo's pixels
/// reach, not the end of the curve's table, which runs a stop past them.
const PAIR_LOW: f32 = 0.01;
const PAIR_HIGH: f32 = 0.99;
/// The smallest extreme (stops) the fractions' fit divides by.
const PAIR_EXTREME_MIN: f32 = 0.05;
/// Standardized feature clamp for [`Pair`] so out-of-distribution synthetic frames never
/// extrapolate linearly.
const PAIR_Z_CLAMP: f32 = 4.;
/// Maximum magnitude (stops) of [`Pair::shifts`] at equal full sliders.
const PAIR_SHIFT_MAX: f32 = 1.5;
/// How a slider's fraction sets in with the other slider's share of it
/// ([`Pair::at`]): as this power of the ratio for two sliders of one sign.
pub(crate) const PAIR_SAME_POWER: f32 = 1.7;
/// And for sliders of opposite signs, as 1 − e^(−rate × ratio), scaled to reach 1 at
/// equal sliders.
pub(crate) const PAIR_OPPOSITE_RATE: f32 = 2.4;
/// Shadows and Highlights together. Camera Raw's render of the two is not the product
/// of their single gains (60 training photos, 2026-11-17): moved the same way they
/// brighten or darken every level by nearly half a stop more (and over a stop on dark
/// scenes), moved opposite ways each does less at its own end. With both set, each
/// slider's curve is blended toward a uniform gain at its extreme,
/// `(1 − f) · curve(level) + f · extreme`, plus a photo-dependent uniform shift `c0`
/// (predicted from the curves and photo brightness statistics in
/// `local_tone_rule_pair_data.rs`, `scene-tone-rule.py pair`). Fractions ramp in with
/// the other slider's share up to equal amounts; the uniform shift scales with the
/// smaller slider times the same ramp, so both vanish smoothly when either slider is 0.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Pair {
    /// Each unit curve's gain at the photo's extreme for its control, in
    /// [`RuleCurve::ALL`]'s order: Shadows' at the 1st percentile of the base levels,
    /// Highlights' at the 99th.
    pub(crate) extremes: [f32; 4],
    /// Per sign combination ([`Self::combination`]): Shadows' and Highlights' fraction
    /// at equal sliders.
    pub(crate) fractions: [[f32; 2]; 4],
    /// Per sign combination ([`Self::combination`]): uniform log2 gain shift at equal
    /// full sliders (`|s| = |h| = 1`).
    pub(crate) shifts: [f32; 4],
}
impl Pair {
    /// The curves of each sign combination, Shadows' then Highlights': both up, both
    /// down, Shadows up with Highlights down, Shadows down with Highlights up.
    const KINDS: [[RuleCurve; 2]; 4] = [
        [RuleCurve::ShadowsUp, RuleCurve::HighlightsUp],
        [RuleCurve::ShadowsDown, RuleCurve::HighlightsDown],
        [RuleCurve::ShadowsUp, RuleCurve::HighlightsDown],
        [RuleCurve::ShadowsDown, RuleCurve::HighlightsUp],
    ];
    /// The sign combination of two sliders, neither 0: an index into [`Self::KINDS`].
    pub(crate) fn combination(shadows: f32, highlights: f32) -> usize {
        match (shadows > 0., highlights > 0.) {
            (true, true) => 0,
            (false, false) => 1,
            (true, false) => 2,
            (false, true) => 3,
        }
    }
    /// For Shadows and for Highlights at these slider values: each slider's blend
    /// fraction and unit extreme, plus the uniform log2 gain shift. None with either
    /// slider at 0, where each renders as it does alone.
    pub(crate) fn at(&self, shadows: f32, highlights: f32) -> Option<([[f32; 2]; 2], f32)> {
        if shadows == 0. || highlights == 0. {
            return None;
        }
        let c = Self::combination(shadows, highlights);
        let same = c < 2;
        let (s, h) = (shadows.abs().min(1.), highlights.abs().min(1.));
        let lo = s.min(h);
        let hi = s.max(h);
        let extreme = |slider: usize| self.extremes[Self::KINDS[c][slider] as usize];
        Some((
            [
                [self.fractions[c][0] * ramp(h / s, same), extreme(0)],
                [self.fractions[c][1] * ramp(s / h, same), extreme(1)],
            ],
            self.shifts[c] * lo * ramp(lo / hi, same),
        ))
    }
}
/// How much of a slider's fraction applies when the other slider is `ratio` of it
/// (magnitudes): all of it from equal sliders on. On 16 training photos the median
/// fraction at a ratio of 0.1, 0.25 and 0.5 was 0.00–0.03, 0.08–0.11 and 0.31 of that at
/// 1 for sliders of one sign, and 0.17–0.20, 0.42–0.50 and 0.75–0.83 for opposite signs.
fn ramp(ratio: f32, same: bool) -> f32 {
    let r = ratio.clamp(0., 1.);
    if same {
        r.powf(PAIR_SAME_POWER)
    } else {
        (1. - (-PAIR_OPPOSITE_RATE * r).exp()) / (1. - (-PAIR_OPPOSITE_RATE).exp())
    }
}
/// Projects the output log luminance (`pos[i] + table[i]`) of a 48-column table onto
/// nondecreasing values, by pooled adjacent violators: the least-squares correction,
/// as `Family::monotone` does for the fitted tables. A brighter level never renders
/// darker than a dimmer one, so a clipped plateau or smooth gradient keeps no solarized
/// rim (`clipped_highlights_do_not_solarize`); flat compression is allowed.
fn monotone_pos(table: &mut [f32; 48], pos: &[f32; 48]) {
    let mut sums = [0f32; 48];
    let mut counts = [0usize; 48];
    let mut blocks = 0;
    for (i, g) in table.iter().enumerate() {
        sums[blocks] = g + pos[i];
        counts[blocks] = 1;
        blocks += 1;
        while blocks > 1
            && sums[blocks - 2] / counts[blocks - 2] as f32
                > sums[blocks - 1] / counts[blocks - 1] as f32
        {
            sums[blocks - 2] += sums[blocks - 1];
            counts[blocks - 2] += counts[blocks - 1];
            blocks -= 1;
        }
    }
    let mut i = 0;
    for block in 0..blocks {
        for _ in 0..counts[block] {
            if counts[block] > 1 {
                table[i] = sums[block] / counts[block] as f32 - pos[i];
            }
            i += 1;
        }
    }
}
pub(super) fn monotone(table: &mut [f32; 48], step: f32) {
    let pos: [f32; 48] = std::array::from_fn(|i| i as f32 * step);
    monotone_pos(table, &pos);
}
/// The median of `values`: the mean of the two middle values of an even count, as
/// `numpy.median` computes the training inputs.
fn median(values: &[f32]) -> f32 {
    if values.is_empty() {
        return 0.;
    }
    let mut v = values.to_vec();
    let n = v.len();
    let (lower, upper, _) = v.select_nth_unstable_by(n / 2, f32::total_cmp);
    if n % 2 == 1 {
        return *upper;
    }
    let below = lower.iter().copied().fold(f32::MIN, f32::max);
    (below + *upper) / 2.
}
fn silu(x: f32) -> f32 {
    x / (1. + (-x).exp())
}
/// The mean of an ensemble's two outputs for the standardized inputs `z`: `M` networks
/// of two hidden layers of `H` units.
fn ensemble<const F: usize, const H: usize, const M: usize>(
    z: &[f32; F],
    w1: &[[[f32; F]; H]; M],
    b1: &[[f32; H]; M],
    w2: &[[[f32; H]; H]; M],
    b2: &[[f32; H]; M],
    w3: &[[[f32; H]; 2]; M],
    b3: &[[f32; 2]; M],
) -> [f32; 2] {
    // Each sum starts from its bias and adds the products in order. Another order
    // rounds differently, and the first ensemble's curves must stay what they were.
    let dot = |input: &[f32], weights: &[f32], bias: f32| {
        input.iter().zip(weights).fold(bias, |s, (v, w)| s + w * v)
    };
    let mut sum = [0f32; 2];
    for m in 0..M {
        let h1: [f32; H] = std::array::from_fn(|j| silu(dot(z, &w1[m][j], b1[m][j])));
        let h2: [f32; H] = std::array::from_fn(|j| silu(dot(&h1, &w2[m][j], b2[m][j])));
        for (k, out) in sum.iter_mut().enumerate() {
            *out += dot(&h2, &w3[m][k], b3[m][k]);
        }
    }
    sum.map(|v| v / M as f32)
}
/// `unit` scaled to slider fraction `s` (the rule is linear in the slider).
pub(super) fn at(unit: &Curve, s: f32) -> Curve {
    Curve {
        lo: unit.lo,
        hi: unit.hi,
        table: unit.table.map(|v| v * s.abs()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> Pair {
        Pair {
            extremes: [2., -1., -3., 1.5],
            fractions: [[0.1, 0.4], [0.2, 0.3], [0.12, 0.25], [0.08, 0.35]],
            shifts: [0.3, -0.2, 0.05, 0.15],
        }
    }

    #[test]
    fn a_pair_leaves_a_single_slider_alone() {
        assert_eq!(pair().at(0.7, 0.), None);
        assert_eq!(pair().at(0., -1.), None);
    }

    #[test]
    fn a_pair_reads_its_sign_combinations_curves() {
        // Each combination has its own fractions and uniform shift, blending toward the
        // extremes of the two curves it uses, at equal sliders in full (shift scaling
        // linearly with slider size).
        let p = pair();
        assert_eq!(p.at(1., 1.), Some(([[0.1, 2.], [0.4, 1.5]], 0.3)));
        assert_eq!(p.at(-0.5, -0.5), Some(([[0.2, -3.], [0.3, -1.]], -0.1)));
        assert_eq!(
            p.at(0.3, -0.3),
            Some(([[0.12, 2.], [0.25, -1.]], 0.05 * 0.3))
        );
        assert_eq!(p.at(-1., 1.), Some(([[0.08, -3.], [0.35, 1.5]], 0.15)));
    }

    #[test]
    fn a_pairs_fraction_sets_in_with_the_other_slider() {
        // A slider's fraction grows with the other slider's share of it and is whole
        // from equal sliders on; the larger slider's stays whole. There is no jump at 0.
        let p = pair();
        let shadows = |h: f32| p.at(1., h).unwrap().0[0][0];
        assert!(shadows(1e-4) < 1e-6 && p.at(1., -1e-4).unwrap().0[0][0] < 1e-4);
        assert!(p.at(1., 1e-4).unwrap().1.abs() < 1e-6);
        let mut last = 0.;
        for h in [0.1, 0.25, 0.5, 0.75, 1.] {
            assert!(shadows(h) > last, "{h}");
            last = shadows(h);
        }
        assert_eq!(shadows(1.), 0.1);
        assert_eq!(p.at(0.25, 1.).unwrap().0[0][0], 0.1);
        // The measured shapes: the power 1.7 of the ratio for one sign, faster for
        // opposite signs.
        assert!((shadows(0.25) / 0.1 - 0.25f32.powf(1.7)).abs() < 1e-5);
        let opposite = p.at(1., -0.25).unwrap().0[0][0] / 0.12;
        assert!((opposite - 0.4962).abs() < 1e-3, "{opposite}");
        assert_eq!(p.at(1., -0.25).unwrap().0[1][0], 0.25);
        // Shift scales with min(|s|, |h|) * ramp(min/max, same).
        assert!((p.at(1., 0.25).unwrap().1 - 0.3 * 0.25 * 0.25f32.powf(1.7)).abs() < 1e-5);
    }

    #[test]
    fn a_photos_pair_reads_each_curve_where_its_pixels_reach() {
        // The extremes are the curves' gains at the 1st and 99th percentile of the map's
        // base levels, not the ends of the tables, and the fractions stay fractions.
        let (w, h) = (128usize, 128);
        let logs: Vec<f32> = (0..w * h)
            .map(|i| -8. + 7. * ((i % w) as f32 / w as f32) + 0.2 * ((i * 37 % 11) as f32 / 11.))
            .collect();
        let stats = RuleStats::new(&logs, &logs, w, h);
        let units = RuleCurve::ALL.map(|kind| stats.curve(kind));
        let pair = stats.pair([&units[0], &units[1], &units[2], &units[3]]);
        let level = |q: f32| stats.sorted[((stats.sorted.len() - 1) as f32 * q) as usize];
        for (k, kind) in RuleCurve::ALL.into_iter().enumerate() {
            let at = if kind.shadows() {
                level(0.01)
            } else {
                level(0.99)
            };
            assert_eq!(pair.extremes[k], units[k].eval(at));
        }
        assert!(
            pair.extremes[0] > 0.5 && pair.extremes[2] < -0.5,
            "{:?}",
            pair.extremes
        );
        for [s, h] in pair.fractions {
            assert!((0. ..=1.).contains(&s) && (0. ..=1.).contains(&h));
        }
    }

    #[test]
    fn median_averages_the_middle_pair_like_numpy() {
        assert_eq!(median(&[]), 0.);
        assert_eq!(median(&[3.]), 3.);
        assert_eq!(median(&[0.875, 0.125, 0.5]), 0.5);
        // An even count: the mean of the two middle values, not the lower one.
        let mut v = vec![0.125; 25];
        v.extend(std::iter::repeat_n(0.875, 25));
        assert_eq!(median(&v), 0.5);
        assert_eq!(median(&[4., 1., 3., 2.]), 2.5);
    }

    #[test]
    fn monotone_never_lets_a_brighter_level_render_darker() {
        let step = 0.25;
        // A pull that steepens past -1 per stop over the top columns.
        let mut t: [f32; 48] = std::array::from_fn(|i| -(i as f32) * 0.4);
        monotone(&mut t, step);
        for i in 1..48 {
            assert!(
                t[i] + i as f32 * step >= t[i - 1] + (i - 1) as f32 * step - 1e-5,
                "{i}"
            );
        }
        // An already nondecreasing output is untouched.
        let mut u: [f32; 48] = std::array::from_fn(|i| -(i as f32) * 0.1);
        let before = u;
        monotone(&mut u, step);
        assert_eq!(u, before);
    }

    /// A 16-level staircase over 6 stops with faint texture, as the mosaics: the rule
    /// lifts the darkest levels by about a stop at Shadows +100, leaves the top stops
    /// alone, and pulls the top at Highlights −100 while leaving the bottom.
    #[test]
    fn staircase_follows_the_measured_shape() {
        let (w, h) = (64, 64);
        let mut logs = vec![0f32; w * h];
        for (i, v) in logs.iter_mut().enumerate() {
            let (x, y) = (i % w, i / w);
            let patch = (y / 16) * 4 + x / 16;
            let noise = ((i * 7919) % 13) as f32 / 13. - 0.5;
            *v = -1.3 - 6. * patch as f32 / 15. + 0.05 * noise;
        }
        let stats = RuleStats::new(&logs, &logs, w, h);
        let s = stats.curve(RuleCurve::ShadowsUp);
        let hc = stats.curve(RuleCurve::HighlightsDown);
        let at = |c: &Curve, x: f32| {
            let f = ((x - c.lo) / (c.hi - c.lo) * 48. - 0.5).clamp(0., 47.);
            c.table[(f as usize).min(47)]
        };
        assert!(at(&s, -1.3) < 0.1, "top not lifted: {}", at(&s, -1.3));
        assert!(at(&s, -7.) > 0.6, "bottom lifted: {}", at(&s, -7.));
        assert!(at(&hc, -1.5) < -0.5, "top pulled: {}", at(&hc, -1.5));
        assert!(at(&hc, -7.) > -0.15, "bottom left: {}", at(&hc, -7.));
    }

    /// The networks' sums start from the bias, the order the first ensemble's curves
    /// were shipped with: summing the products first rounds differently (here to
    /// 2^27 + 16), which moved positive Shadows and negative Highlights by 4e-7 EV.
    #[test]
    fn a_sum_starts_from_its_bias() {
        // Hidden units of exactly 20 (SiLU of 20 in f32), four products of 5 each on
        // a bias whose spacing is 16: one at a time they round away, together they
        // would not.
        let (z, zero) = ([0f32; 4], [[[0f32; 4]; 4]; 1]);
        let out = ensemble(
            &z,
            &zero,
            &[[0.; 4]],
            &zero,
            &[[20.; 4]],
            &[[[0.25; 4]; 2]],
            &[[134_217_728.; 2]],
        );
        assert_eq!(out, [134_217_728.; 2]);
    }

    /// A few near-black pixels (a hole, a black sensor border) set the histogram's
    /// bottom on their own. Shadows −100 must not read the mid-tones as far above the
    /// bottom for them: the second ensemble's bottom is held within [`BOTTOM_GAP`] of
    /// the 10th percentile.
    #[test]
    fn a_sparse_dark_tail_leaves_negative_shadows_on_the_mid_tones_alone() {
        let (w, h) = (128, 128);
        let curve = |tail: bool| {
            let mut logs = vec![0f32; w * h];
            for (i, v) in logs.iter_mut().enumerate() {
                let (x, y) = (i % w, i / w);
                let patch = (y / 32) * 4 + x / 32;
                let noise = ((i * 7919) % 13) as f32 / 13. - 0.5;
                *v = -1.3 - 4. * patch as f32 / 15. + 0.05 * noise;
                // 0.2% of the frame, eight stops under everything else.
                if tail && i < 33 {
                    *v = -13.5;
                }
            }
            RuleStats::new(&logs, &logs, w, h).curve(RuleCurve::ShadowsDown)
        };
        let at = |c: &Curve, x: f32| {
            let f = ((x - c.lo) / (c.hi - c.lo) * 48. - 0.5).clamp(0., 47.);
            c.table[(f as usize).min(47)]
        };
        let (plain, tailed) = (curve(false), curve(true));
        for level in [-2., -3., -4., -5.] {
            let (a, b) = (at(&plain, level), at(&tailed, level));
            assert!(
                (a - b).abs() < 0.15,
                "{level}: {a} without the tail, {b} with it"
            );
        }
    }

    /// All four rule curves carry a monotone pixel-level contrast curve (`G_pix(l)`)
    /// over `[bot, top]` that holds flat outside `[bot, top]` so specular highlights
    /// and deep sensor noise never extrapolate.
    #[test]
    fn pixel_contrast_curves_exist_are_monotone_and_clamp_outside_bot_top() {
        let (w, h) = (64, 64);
        let logs: Vec<f32> = (0..w * h)
            .map(|i| -6. + 6.5 * ((i % w) as f32 / w as f32))
            .collect();
        let stats = RuleStats::new(&logs, &logs, w, h);
        for kind in RuleCurve::ALL {
            let base = stats.curve(kind);
            let pix = stats
                .curve_pix(kind, &base)
                .expect("every rule curve has a pixel-level contrast curve");
            let step = (pix.hi - pix.lo) / LEVELS as f32;
            for i in 1..LEVELS {
                let x_prev = pix.lo + (i as f32 - 0.5) * step;
                let x_curr = pix.lo + (i as f32 + 0.5) * step;
                assert!(
                    pix.table[i] + i as f32 * step
                        >= pix.table[i - 1] + (i - 1) as f32 * step - 1e-5,
                    "{kind:?} bin {i} pix inverted"
                );
                assert!(
                    x_curr + base.eval(x_curr) + pix.eval(x_curr)
                        >= x_prev + base.eval(x_prev) + pix.eval(x_prev) - 1e-5,
                    "{kind:?} bin {i} combined base+pix inverted"
                );
            }
            assert_eq!(pix.eval(pix.lo - 5.), pix.eval(pix.lo - 0.5));
            assert_eq!(pix.eval(pix.hi + 5.), pix.eval(pix.hi + 0.5));
        }
    }

    /// Even on high-contrast dark-surround scenes with steep highlight compression,
    /// `l + G_base(l) + G_pix(l)` stays nondecreasing so smooth gradients never solarize.
    #[test]
    fn combined_base_and_pix_curve_is_monotone_on_high_contrast_scenes() {
        let (w, h) = (128, 128);
        let mut ramp_in_dark = vec![-8f32; w * h];
        for y in 96..h {
            for x in 0..w {
                ramp_in_dark[y * w + x] = -8. + 8. * (x as f32 / (w - 1) as f32);
            }
        }
        let mut block_in_dark = vec![-10f32; w * h];
        for y in 48..80 {
            for x in 48..80 {
                block_in_dark[y * w + x] = 0.;
            }
        }
        for (name, logs) in [("ramp", ramp_in_dark), ("block", block_in_dark)] {
            let stats = RuleStats::new(&logs, &logs, w, h);
            for kind in RuleCurve::ALL {
                let base = stats.curve(kind);
                let pix = stats.curve_pix(kind, &base).unwrap();
                let step = (pix.hi - pix.lo) / LEVELS as f32;
                for i in 1..LEVELS {
                    let x0 = pix.lo + (i as f32 - 0.5) * step;
                    let x1 = pix.lo + (i as f32 + 0.5) * step;
                    let out0 = x0 + base.eval(x0) + pix.eval(x0);
                    let out1 = x1 + base.eval(x1) + pix.eval(x1);
                    assert!(
                        out1 >= out0 - 1e-4,
                        "{name} {kind:?} bin {i}: {out0} -> {out1}"
                    );
                }
            }
        }
    }

    #[test]
    fn rendered_shadows_up_and_highlights_up_taper_extremes_while_staying_monotone() {
        let (w, h) = (96, 64);
        let logs: Vec<f32> = (0..w * h)
            .map(|i| -11.5 + 11.5 * ((i % w) as f32 / (w - 1) as f32))
            .collect();
        let raw = RuleStats::new_for(&logs, &logs, w, h, false, 0.);
        let ren = RuleStats::new_for(&logs, &logs, w, h, true, 0.);

        // ShadowsDown and HighlightsDown stay identical between RAW and rendered.
        for kind in [RuleCurve::ShadowsDown, RuleCurve::HighlightsDown] {
            assert_eq!(raw.curve(kind).table, ren.curve(kind).table, "{kind:?}");
        }

        // ShadowsUp trims deep-shadow lift below -7 EV, HighlightsUp trims near-white shoulder.
        let s_raw = raw.curve(RuleCurve::ShadowsUp);
        let s_ren = ren.curve(RuleCurve::ShadowsUp);
        assert!(
            s_ren.eval(-10.5) < s_raw.eval(-10.5) - 0.05,
            "raw={} ren={}",
            s_raw.eval(-10.5),
            s_ren.eval(-10.5)
        );
        let h_raw = raw.curve(RuleCurve::HighlightsUp);
        let h_ren = ren.curve(RuleCurve::HighlightsUp);
        assert!(
            h_ren.eval(-0.5) <= h_raw.eval(-0.5) + 1e-5,
            "raw={} ren={}",
            h_raw.eval(-0.5),
            h_ren.eval(-0.5)
        );

        // Pre-Exposure anchoring: shifting logs and exposure together by -1.5 EV keeps the
        // HighlightsUp taper active at the file's original white ceiling (x = -1.5 EV).
        let shifted: Vec<f32> = logs.iter().map(|&v| v - 1.5).collect();
        let h_shifted_raw = RuleStats::new_for(&shifted, &shifted, w, h, false, -1.5)
            .curve(RuleCurve::HighlightsUp);
        let h_shifted_ren =
            RuleStats::new_for(&shifted, &shifted, w, h, true, -1.5).curve(RuleCurve::HighlightsUp);
        let h_shifted_unanchored =
            RuleStats::new_for(&shifted, &shifted, w, h, true, 0.).curve(RuleCurve::HighlightsUp);
        assert!(
            h_shifted_ren.eval(-1.5) < h_shifted_unanchored.eval(-1.5) - 0.02
                && h_shifted_ren.eval(-1.5) < h_shifted_raw.eval(-1.5) - 0.05,
            "raw={} unanchored={} anchored={}",
            h_shifted_raw.eval(-1.5),
            h_shifted_unanchored.eval(-1.5),
            h_shifted_ren.eval(-1.5)
        );

        // All rendered base + pix curves stay monotone.
        for kind in RuleCurve::ALL {
            let base = ren.curve(kind);
            let pix = ren.curve_pix(kind, &base).unwrap();
            let step = (pix.hi - pix.lo) / LEVELS as f32;
            for i in 1..LEVELS {
                let x0 = pix.lo + (i as f32 - 0.5) * step;
                let x1 = pix.lo + (i as f32 + 0.5) * step;
                assert!(
                    x1 + base.eval(x1) + pix.eval(x1) >= x0 + base.eval(x0) + pix.eval(x0) - 1e-4,
                    "rendered {kind:?} inverted at {i}"
                );
            }
        }
    }
}
