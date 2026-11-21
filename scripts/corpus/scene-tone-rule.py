#!/usr/bin/env python3
"""The global rule of Shadows and Highlights (`local_tone_rule.rs`).

Camera Raw's gain for a base level is a function of where that level sits in the
photo's histogram of base levels, measured on flat-patch mosaics whose histograms are
controlled (docs/scene-tone-stage.md#the-global-rule). The rule is a small network
from 24 histogram descriptors of a level to the log2 gain at Shadows +100 and
Highlights −100, trained on those mosaics and on linear-profile training photos, and
a second one for Shadows −100 and Highlights +100 (2026-11-17), which also reads the
level and the photo's top, median and mean themselves: Highlights +100 lifts the
bright areas of a dark photo by up to three stops and those of a bright photo hardly
at all, and the level's place in the histogram does not tell the two apart (held-out
photos, with the gains read off the renders: 0.130 EV with the 24 descriptors, 0.112
with the four more; Shadows −100 0.216 and 0.215, its worst tenth 0.54 and 0.49).

  probes ROOT                 writes the mosaic DNGs under ROOT/probes and renders them
                              in Camera Raw (camera_raw.py), skipping renders that exist
  photos ROOT [RAW...] [--list JSON]
                              converts RAWs (given, or listed in JSON relative to
                              $RAWMAKASE_DATASET) with a linear profile
                              (linear_profile.py) into ROOT/photos/dng and renders them
                              in Camera Raw with that profile into ROOT/photos/crlin
                              (default, S+100, H−100, S−100, H+100, S−50 and H+50 for
                              the second ensemble's clipped targets, and the four pairs
                              of Shadows ±100 with Highlights ±100); numbering continues
                              ROOT/photos
  taps ROOT RAWMAKASE         dumps the engine's measurement map of every probe and photo
                              into ROOT/maps (RAWMAKASE_RULE_DUMP; needs RAWMAKASE_DATA_DIR),
                              with its white curve there, and each photo's four rule
                              curves into ROOT/curves (a build of the ensembles `pair` is
                              for: run it again after `train`)
  train ROOT [--cv] [--out] [--first]
                              builds the samples, optionally cross-validates by photo, fits
                              the second ensemble on everything and writes the JSON and
                              crates/rawmakase-engine/src/develop/local_tone_rule_ext_data.rs;
                              with --first it refits the first ensemble too and writes
                              local_tone_rule_data.rs (shipped from the fit of 2026-10-30,
                              on 496 mosaics and 283 photos, and left alone otherwise)
  pair ROOT [--cv]            fits how Shadows and Highlights act together (`Pair` in
                              local_tone_rule.rs) on the photos' renders of the four
                              pairs and writes local_tone_rule_pair_data.rs

Every step takes --histograms DIR when the photo mosaics came from another photo set
than ROOT/maps' first 26.

A render may be kept as a `.npy` beside its TIFF's name instead of the TIFF: linear
ProPhoto values or their luminance, at any size from the map's up (the targets are
read on the map's grid).

Mosaics are 1024-pixel scenes of flat square patches (W* = 1, DefaultBlackRender
None) with faint blob texture, since a perfectly flat maximum is a degenerate case
for Camera Raw: any texture on the brightest patch, even 0.03 stop, changes every
dark patch's lift by about 0.4 EV. Families: random histograms at a fixed and at a
random texture amplitude, histograms sampled from the training photos' base levels,
small bright minorities above a gap, a few tiny bright spots in a dark field, deep
ranges of 10.5 to 13.5 stops (the Shadows lift keeps growing to about 4.3 EV twelve
stops under the top), bimodal histograms with most of the frame 9 to 12 stops down
and the rest 2 to 5 stops down (Camera Raw lifts the deep cluster 3 to 5 EV and the
mid cluster only 0.1 to 0.3), and top patches clipped in every channel, in green
only or just under white (Highlights pulls them all alike), smooth skies of varying
cloud texture and extent over textured ground, dark regions of varying texture under a
fixed sky (Camera Raw lifts a smooth dark region far less than a rough one at the same
depth, which no histogram can see: hence the per-level texture inputs), colored
patches of equal luminance (the pull is color-blind), and three families under a
non-neutral AsShotNeutral so that clipped patches reconstruct above the sensor's
white, as a photo's do: a square of 0.05 to 12% of the frame either fully clipped or
just under white (`clipwb`: the square adds only 0.05 EV of deep lift either way, so
the rule reads a reconstructed top like an unclipped one), 10 to 35% of the patches
clipped in every channel or in green (`clipwb2`), and a square of 0.2 to 12% at four
levels from just under white to clipped in every channel (`partial`: Camera Raw's lift
hardly depends on how many channels clip, 2026-10-30).

Targets count down to 13 stops under white (`targets`): with a 9-stop floor the
network never saw the deepest tones and under-lifted them by 1.5 EV on interiors.
Weighting the photos up, or a wider and longer-trained network, traded the deep
tones of dark-heavy photos against the mid-shadows of ordinary ones in both
directions on the stress sets; this configuration had the lowest mean error there
(2026-10-28, docs/records/parity-gaps.md).

The second ensemble's targets (`samples_ext`) differ from the first's in three ways
(2026-11-17). They count up to white, not to 0.6, since Highlights +100 works there.

They are taken with the engine's white curve undone (`engine_map` keeps it with the
map). The engine applies the rule's gain before that curve, which is fitted per photo
and compresses everything near the photo's top, while Camera Raw's renders show its
gain after its own, so a gain read off the renders reaches only a part of the way:
Highlights +100 put 0.44% of the training photos' pixels at white against Camera Raw's
0.89%. The curve has to be the engine's own above the photo's top too. Three quarters
of the training photos top out under 0.9 of white, and a curve measured on a photo's
own pixels ends at its top, which read every result above the top as the top: the top
stop came out 0.05 stop short on average, with 0.60% of the pixels at white.

And a clipped result is a bound, not a target. At the black floor the gain was at most
what the render shows. At white, in any channel, it was at least what takes the
brightest channel to the level the engine renders white, and `CLIP_MARGIN` more: a fit
lands on either side of a bound, and without the margin half of those pixels stop just
under white. More than half of the pixels Camera Raw clips at Highlights +50 are
colored highlights with a luminance under 0.97, so the brightest channel decides what
is clipped, not luminance (share of the pixels at white at +50: Camera Raw 0.57%;
without the margin 0.35%, with it 0.40%, with the brightest channel deciding 0.49%).
Camera Raw's gain is linear in the slider before the white curve (half the slider
gives 0.50 of the lift in stops below the top and up to 0.56 just under it; predicting
its Highlights +50 render from default and +100 that way is off by 0.00 to 0.02 stop),
so where the full slider clipped, the half-slider render of a photo gives twice its own
gain as the target, or twice the bound where it clipped too.

Features (`Photo.features`, mirrored by `RuleStats::features`): top − x, x − bottom,
mean − x, five quantiles − x, the CDF at x, the range, the mass within eight bands
of stops around x, two global texture measures clamped so the network cannot
extrapolate on amplitude, the fine and coarse texture of the pixels at the level
itself (median absolute residuals, so edge pixels do not count), and the fraction of
the map within one and within two stops of the top: how sparse the top is. Without
it a small bright spot sets the top and every deep level reads as farther down than
Camera Raw lifts it (a lit field under a bright sky at Shadows +100, 6.2 → 3.3 ΔE00);
quantiles near the top instead (99th, 99.9th) made the rule over-lift the mid-tones of
filled histograms by 0.3 to 0.5 EV. The second ensemble reads a bottom held within
BOTTOM_GAP of the 10th percentile (`Photo.features_ext`): a few black pixels set the
0.1th percentile on their own, and the four training photos where they did were its
four worst at Shadows −100. Requires numpy, scipy, tifffile, Pillow and mlx.

Photos: the engine's map is in the sensor's orientation and the Camera Raw render in
the display's; `samples` takes the rotation whose log luminance correlates with the
map and drops a photo below 0.8 (a dozen of the 325 render with a different crop or
fail to convert). The photos are CC0 raw.pixls.us files of all three dataset families'
training sets plus 76 chosen for small clipped spots, skies over deep ground, large
bright areas and heavy clipping, of which 62 align (docs/records/parity-gaps.md). The
second ensemble was fitted on 436 mosaics and the 243 training photos of the three
families, of which 228 render with the linear profile (the rest have no profile of
that name to render with) and 213 of those align; a photo whose render sits a pixel or
two off the map (lens geometry) is taken at the rotation that correlates on an
eighth-size copy. Photos shot with an in-camera aspect crop do not align: the engine
measures the whole sensor area and Camera Raw renders the crop. Its maps and white
curves were dumped by the engine of the revision that added it, its renders made by
Camera Raw 18.7, and each set's samples drawn with its place in the list of sets as
the seed (`load`).
"""
import argparse
import json
import os
import pickle
import subprocess
import sys
from pathlib import Path

import numpy as np

sys.dont_write_bytecode = True
HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
import camera_raw  # noqa: E402
import linear_profile  # noqa: E402
import probe_dng  # noqa: E402
import probe_scenes  # noqa: E402

ROOT = HERE.resolve().parents[1]
DATA = ROOT / 'crates/rawmakase-engine/src/develop/local_tone_rule_data.rs'
DATA_EXT = ROOT / 'crates/rawmakase-engine/src/develop/local_tone_rule_ext_data.rs'
DATA_PAIR = ROOT / 'crates/rawmakase-engine/src/develop/local_tone_rule_pair_data.rs'
YW = np.array([0.2880402, 0.7118741, 0.0000857])    # ProPhoto luminance
N = 1024
SETTINGS = {'default': {}, 'S+100': {'Shadows2012': '100'}, 'H-100': {'Highlights2012': '-100'},
            'S-100': {'Shadows2012': '-100'}, 'H+100': {'Highlights2012': '100'}}
OUTPUTS = [s for s in SETTINGS if s != 'default']    # the targets' columns
EXT_OUTPUTS = ['S-100', 'H+100']                     # those the second ensemble keeps
# The photos' renders at half of the second ensemble's sliders, by output (`samples_ext`).
HALVES = {'S-100': ('S-50', {'Shadows2012': '-50'}), 'H+100': ('H+50', {'Highlights2012': '50'})}
# Shadows with Highlights, in the order of `Pair::KINDS`: each pair's two single settings
# and its render (`pair`).
PAIRS = {'S+100_H+100': ('S+100', 'H+100'), 'S-100_H-100': ('S-100', 'H-100'),
         'S+100_H-100': ('S+100', 'H-100'), 'S-100_H+100': ('S-100', 'H+100')}
# `local_tone_rule.rs`: where a curve is read for its extreme, and the least extreme the
# fractions' inputs divide by.
PAIR_LOW, PAIR_HIGH, PAIR_EXTREME_MIN = 0.01, 0.99, 0.05
PAIR_RIDGE = 3.0    # on the standardized weights, per hundred photos
# The second ensemble's targets count up to WHITE at default. A Highlights +100 result
# from CLIP_HIGH up sits in Camera Raw's clipping and a Shadows −100 result from
# CLIP_LOW down at its floor: such a render shows only a bound on the gain.
WHITE, CLIP_HIGH, CLIP_LOW = 0.98, 0.97, 2.0 ** -13
# A result at white asks for the level the engine renders white and this many stops
# more: twice the fit's error on held-out photos, so that its scatter does not leave
# half of those pixels just under white.
CLIP_MARGIN = 0.25
PHOTO_WEIGHT = 1    # photo samples against mosaic samples in the fit
HIDDEN = 64         # units in each of the two hidden layers
MEMBERS = 4         # networks in the ensemble (seeds 0..MEMBERS-1), averaged by the engine
# `local_tone.rs`: the measurement copy's long edge, the map's floor, the guided filter.
MAP, FLOOR, EPS, RADIUS = 512, 2.0 ** -14, 0.5, 0.032
TEX_CLAMP, TEX_TOP_CLAMP = 0.6, 0.05
# Per-level texture (`RuleStats::texture_at`): the median absolute fine (radius 2) and
# coarse (radius COARSE_RADIUS) residual of the pixels within LEVEL_WINDOW of the level.
LEVEL_WINDOW, COARSE_RADIUS, TEX_AT_CLAMP, LEVEL_MIN = 0.5, 12, 1.0, 50
# How far under the 10th percentile the second ensemble's bottom may sit (`local_tone_rule.rs`).
BOTTOM_GAP = 3.5
FEAT = ['top-x', 'x-bot', 'mean-x', 'p50-x', 'p75-x', 'p90-x', 'p25-x', 'p10-x', 'cdf', 'range',
        'm(0,1]', 'm(1,2]', 'm(2,3]', 'm(3,5]', 'm(-1,0]', 'm(-2,-1]', 'm(-3,-2]', 'm(-5,-3]', 'tex', 'tex_top',
        'tex_at', 'coarse_at', 'm_top1', 'm_top2']
# The second ensemble's inputs: FEAT, then the photo's top, median and mean and the level.
FEAT_EXT = FEAT + ['top', 'median', 'mean', 'x']
NEUTRAL = {'A': (0.5, 1., 0.7), 'B': (0.35, 1., 0.7)}   # AsShotNeutral of the clipping families


# --- the engine's measurement pipeline, in numpy -------------------------------------

def reduce(lum, edge=MAP):
    from PIL import Image
    h, w = lum.shape
    s = edge / max(h, w)
    if s >= 1:
        return lum
    size = (max(1, round(w * s)), max(1, round(h * s)))
    return np.asarray(Image.fromarray(lum.astype(np.float32), 'F').resize(size, Image.BOX), dtype=np.float64)


def box(x, r):
    from scipy.ndimage import uniform_filter
    return uniform_filter(x, size=2 * r + 1, mode='nearest')


def guided(logs):
    h, w = logs.shape
    r = max(1, round(RADIUS * max(h, w)))
    m, m2 = box(logs, r), box(logs * logs, r)
    var = np.maximum(m2 - m * m, 0)
    a = var / (var + EPS)
    return box(a, r) * logs + box(m - a * m, r)


class Photo:
    """Histogram statistics of one image's measurement copy, from linear luminance."""

    def __init__(self, lum=None, logs=None, base=None):
        if logs is None:
            small = reduce(lum)
            logs = np.log2(np.maximum(small, FLOOR))
            base = guided(logs)
        self.logs, self.base = np.asarray(logs, dtype=np.float64), np.asarray(base, dtype=np.float64)
        b = np.sort(self.base.ravel())
        self.sorted = b
        n = b.size
        q = lambda p: b[min(n - 1, int(p * (n - 1)))]
        self.top, self.bot, self.mean = b[-1], q(0.001), b.mean()
        self.q = {p: q(p) for p in (0.1, 0.25, 0.5, 0.75, 0.9)}
        d = self.logs - box(self.logs, 2)
        self.tex = float(np.sqrt(np.mean(d * d)))
        top = self.base > self.top - 2
        self.tex_top = float(np.sqrt(np.mean(d[top] ** 2))) if top.any() else self.tex
        # The residuals in order of base level, for the per-level medians; a median
        # ignores the few edge pixels whose residual spans stops.
        order = np.argsort(self.base.ravel(), kind='stable')
        self.fine_by_level = np.abs(d.ravel())[order]
        self.coarse_by_level = np.abs((self.logs - box(self.logs, COARSE_RADIUS)).ravel())[order]
        self.fine_median, self.coarse_median = float(np.median(self.fine_by_level)), float(np.median(self.coarse_by_level))

    def cdf(self, x):
        return np.searchsorted(self.sorted, x, side='right') / self.sorted.size

    def texture_at(self, x):
        """Fine and coarse texture of the pixels within LEVEL_WINDOW of each level in `x`."""
        x = np.atleast_1d(np.asarray(x, dtype=np.float64))
        lo, hi = np.searchsorted(self.sorted, x - LEVEL_WINDOW), np.searchsorted(self.sorted, x + LEVEL_WINDOW, side='right')
        fine, coarse = np.full(x.size, self.fine_median), np.full(x.size, self.coarse_median)
        for i, (l, h) in enumerate(zip(lo, hi)):
            if h - l >= LEVEL_MIN:
                fine[i], coarse[i] = np.median(self.fine_by_level[l:h]), np.median(self.coarse_by_level[l:h])
        return np.minimum(fine, TEX_AT_CLAMP), np.minimum(coarse, TEX_AT_CLAMP)

    def features(self, x, bot=None):
        x = np.asarray(x, dtype=np.float64)
        bot = self.bot if bot is None else bot
        c = self.cdf
        m = lambda lo, hi: c(x + hi) - c(x + lo)
        const = lambda v: np.full_like(x, v)
        fine, coarse = self.texture_at(x)
        return np.stack([self.top - x, x - bot, self.mean - x, self.q[0.5] - x, self.q[0.75] - x,
                         self.q[0.9] - x, self.q[0.25] - x, self.q[0.1] - x, c(x), const(self.top - bot),
                         m(0, 1), m(1, 2), m(2, 3), m(3, 5), m(-1, 0), m(-2, -1), m(-3, -2), m(-5, -3),
                         const(min(self.tex, TEX_CLAMP)), const(min(self.tex_top, TEX_TOP_CLAMP) / TEX_TOP_CLAMP),
                         fine, coarse, const(1 - c(self.top - 1)), const(1 - c(self.top - 2))], -1)

    def features_ext(self, x):
        """`features`, then the absolute levels the second ensemble reads (`FEAT_EXT`), with
        the bottom no more than BOTTOM_GAP under the 10th percentile and darker levels
        read as it (`RuleStats::gain`)."""
        x = np.asarray(x, dtype=np.float64)
        floor = max(self.bot, self.q[0.1] - BOTTOM_GAP)
        if floor > self.bot:
            x = np.maximum(x, floor)
        const = lambda v: np.full_like(x, v)
        return np.concatenate([self.features(x, floor), np.stack([const(self.top), const(self.q[0.5]), const(self.mean), x], -1)], -1)


def targets(default_lum, setting_lum, lo=2 ** -13, hi=0.6, size=None):
    """log2 gain on the map (`size`: its height and width) and the pixels that count
    (Camera Raw neither near black nor in the white point's shoulder)."""
    from PIL import Image
    if size is None:
        d, s = reduce(default_lum), reduce(setting_lum)
    else:
        rs = lambda a: np.asarray(Image.fromarray(a.astype(np.float32), 'F').resize((size[1], size[0]), Image.BOX), dtype=np.float64)
        d, s = rs(default_lum), rs(setting_lum)
    return np.log2(np.maximum(s, FLOOR)) - np.log2(np.maximum(d, FLOOR)), (d > lo) & (d < hi)


# --- probes --------------------------------------------------------------------------

def blobs(sigma, seed):
    t = probe_scenes._blur(np.random.default_rng(seed).normal(0, 1, (N, N)), sigma)
    return t / t.std()


def mosaic(levels, patch, seed, amp, tex):
    n = N // patch
    order = np.random.default_rng(seed).permutation(n * n)
    log = np.empty((N, N))
    for k, idx in enumerate(order):
        i, j = divmod(k, n)
        sl = (slice(i * patch, (i + 1) * patch), slice(j * patch, (j + 1) * patch))
        log[sl] = levels[idx] + amp * tex[sl]
    return log


def histograms(rng, k, patch):
    """A random histogram of (N/patch)² levels: uniform, clustered, skewed or quantised."""
    n = (N // patch) ** 2
    top = np.log2(rng.uniform(0.1, 0.5))
    r = rng.uniform(2, 10)
    kind = ['uniform', 'clusters', 'skew', 'quantised'][k % 4]
    if kind == 'uniform':
        lv = rng.uniform(top - r, top, n)
    elif kind == 'clusters':
        c = rng.integers(2, 4)
        centers, widths = np.sort(rng.uniform(top - r, top, c)), rng.uniform(0.2, 1.5, c)
        which = rng.integers(0, c, n)
        lv = np.clip(centers[which] + rng.normal(0, 1, n) * widths[which], top - r, top)
    elif kind == 'skew':
        a = rng.uniform(0.3, 3)
        lv = top - r * rng.beta(a, 1 / a, n)
    else:
        lv = rng.choice(np.sort(rng.uniform(top - r, top, rng.integers(3, 9))), n)
    lv[0] = top
    return lv


def minority(rng, patch, spots):
    """A dark textured majority under a gap, with a small bright minority (`spots`: a few
    32-pixel patches) or a minority of up to 12% of the frame."""
    n = (N // patch) ** 2
    top = np.log2(rng.uniform(0.15, 0.5))
    gap = rng.uniform(4, 9) if spots else rng.uniform(2, 8)
    nb = int(rng.integers(1, 11)) if spots else int(rng.integers(1, max(2, int(n * 0.12)) + 1))
    dark_w = rng.uniform(0.5, 3) if spots else rng.uniform(1, 4)
    lv = np.clip(top - gap - dark_w / 2 + rng.normal(0, dark_w / 2, n), top - 12, top - gap)
    if rng.random() < (0.5 if spots else 0.4):
        mid = int(rng.integers(n // 20, n // 3)) if spots else int(rng.integers(1, n // 4))
        lv[nb:nb + mid] = rng.uniform(top - gap, top - (1 if spots else 0.5), mid)
    lv[:nb] = top - rng.uniform(0, 0.5 if spots else rng.uniform(0.1, 1.0), nb)
    return lv


def rgb(log):
    """A log2 luminance image as the neutral linear image the DNG is written from."""
    return np.repeat((2.0 ** log)[:, :, None], 3, 2)


def deep(rng, k):
    """64 levels over 10.5 to 13.5 stops, uniform or skewed to the top."""
    top, r = rng.uniform(-1.0, -0.3), rng.uniform(10.5, 13.5)
    lv = top - r * (rng.beta(1, 1, 64) if k % 2 == 0 else rng.beta(0.6, 1.4, 64))
    lv[0], lv[1] = top, top - r
    return lv


def bimodal(rng):
    """A deep cluster (9 to 12 stops under the top, 30 to 70% of the frame) and a mid
    cluster 2 to 5 stops under it."""
    top, nd = rng.uniform(-1.0, -0.3), int(round(64 * rng.uniform(0.3, 0.7)))
    dc, dw, mc, mw = rng.uniform(9, 12), rng.uniform(0.5, 2), rng.uniform(2.5, 4.5), rng.uniform(0.3, 1.0)
    lv = np.concatenate([top - np.clip(dc + rng.normal(0, dw, nd), 8, 13.5),
                         top - np.clip(mc + rng.normal(0, mw, 64 - nd), 1.5, 5.5)])
    lv[0] = top
    return lv


def clipped(rng, k):
    """64 levels over 6 stops below -1, with 5, 15 or 30% of the patches clipped in every
    channel, in green only, or set just under white."""
    kind, frac = ['all', 'green', 'near'][k % 3], [0.05, 0.15, 0.30][(k // 3) % 3]
    lv = -1 - 6 * rng.uniform(0, 1, 64)
    lv[0] = -1.0
    log, where = mosaic_where(lv, 128, k, 0.05, blobs(4, 5))
    img = rgb(log)
    sel = np.isin(where, np.arange(1, 1 + max(1, int(round(frac * 64)))))
    img[sel] = {'all': 1.3, 'green': [0.6, 1.3, 0.6], 'near': 0.95}[kind]
    return img


def sky_scene(a, f, d, seed):
    """A smooth bright region (fraction `f` of the frame, cloud texture of amplitude `a`)
    over dark textured ground `d` stops below it."""
    rng = np.random.default_rng(seed)
    top, h = rng.uniform(-0.8, -0.3), int(N * f)
    log = np.empty((N, N))
    log[:h] = top + a * blobs(32, seed + 1)[:h]
    log[h:] = top - d - 1.0 + 0.45 * blobs(4, seed + 2)[h:] + 1.0 * np.clip(blobs(24, seed + 3)[h:], -1, 1)
    return np.clip(log, -12, 0)


def ground_scene(a, d, broad, seed):
    """A fixed textured sky (30% of the frame) over a dark region `d` stops down whose fine
    texture amplitude is `a`, with or without broad shapes."""
    rng = np.random.default_rng(seed)
    top, h = rng.uniform(-0.8, -0.3), int(N * 0.3)
    log = np.empty((N, N))
    log[:h] = top + 0.1 * blobs(32, seed + 1)[:h]
    log[h:] = top - d + a * blobs(3, seed + 2)[h:] + broad * np.clip(blobs(24, seed + 3)[h:], -1, 1) * 0.7
    return np.clip(log, -13, 0)


CHROMA = {'neutral': (1, 1, 1.), 'blue': (0.55, 0.75, 1.6), 'blue2': (0.3, 0.6, 2.2), 'orange': (1.6, 0.9, 0.4),
          'green': (0.6, 1.3, 0.5), 'cyan': (0.5, 1.1, 1.4)}


def chroma_scene(k):
    """64 patches over 6 stops, each colored at equal luminance (the pull is color-blind;
    the family keeps the fit honest about that)."""
    rng = np.random.default_rng(8000 + k)
    top = rng.uniform(-1.0, -0.3)
    lv = top - 6 * rng.uniform(0, 1, 64)
    lv[0] = top
    names = list(CHROMA)
    log, where = mosaic_where(lv, 128, k, 0.05, blobs(4, 5))
    img = rgb(log)
    for i in range(64):
        c = np.array(CHROMA[names[(i + k) % len(names)]])
        img[where == i] *= c / (c @ YW) * (np.ones(3) @ YW)
    return np.clip(img, 0, 0.98)


def mosaic_where(levels, patch, seed, amp, tex):
    """`mosaic` and the patch index of every pixel."""
    n = N // patch
    order = np.random.default_rng(seed).permutation(n * n)
    log, where = np.empty((N, N)), np.empty((N, N), int)
    for k, idx in enumerate(order):
        i, j = divmod(k, n)
        sl = (slice(i * patch, (i + 1) * patch), slice(j * patch, (j + 1) * patch))
        log[sl], where[sl] = levels[idx] + amp * tex[sl], idx
    return log, where


def seven_stop(seed):
    """A 64-level mosaic over 7 stops below -1 (patch 128, texture 0.05), and the patch
    index of every pixel."""
    rng = np.random.default_rng(seed)
    lv = -1 - 7 * rng.uniform(0, 1, 64)
    lv[0] = -1.0
    return mosaic_where(lv, 128, seed % 1000, 0.05, blobs(4, 5))


def square_scene(seed, value, frac):
    """`seven_stop(7000 + seed)` with a centred square of `frac` of the frame at scene
    `value` (neutral), under AsShotNeutral A: 1 clips green, 1.43 blue, 2 red."""
    log, _ = seven_stop(7000 + seed)
    img = rgb(log)
    s = int(round(np.sqrt(frac) * N))
    y0 = x0 = (N - s) // 2
    img[y0:y0 + s, x0:x0 + s] = value
    return img


def clipped_wb(k):
    """`clipwb2`: `seven_stop(8000 + k)` with 10, 20 or 35% of the patches at scene 2.5
    under neutral A (every channel clipped) or B (green and blue clipped, red at 0.875 of
    its white), or at (0.6, 1.3, 0.6) under A (green only)."""
    frac = [0.10, 0.20, 0.35][(k // 3) % 3]
    kind, neutral = [('all', 'A'), ('all', 'B'), ('green', 'A')][k % 3]
    log, where = seven_stop(8000 + k)
    img = rgb(log)
    img[np.isin(where, np.arange(1, 1 + max(1, int(round(frac * 64)))))] = 2.5 if kind == 'all' else [0.6, 1.3, 0.6]
    return img, NEUTRAL[neutral]


def probe_jobs(root, photos_dir=None, images=True):
    """Every mosaic family: (name, patch, linear image, AsShotNeutral). The photo
    mosaics take the histograms of the first 26 photo maps of `photos_dir`, or of
    ROOT/maps. Without `images` the images are None: the same names in the same order,
    for the steps that only read the mosaics' files, without building 400 megapixel
    images."""
    out = []
    make = (lambda image: image()) if images else (lambda image: None)
    for k in range(20):
        out.append((f'deep-{k:02d}', 128, make(lambda: rgb(mosaic(deep(np.random.default_rng(5000 + k), k), 128, k, 0.05, blobs(4, 5))))))
    for k in range(18):
        out.append((f'clip-{k:02d}', 128, make(lambda: clipped(np.random.default_rng(6000 + k), k))))
    for k in range(16):
        out.append((f'bimodal-{k:02d}', 128, make(lambda: rgb(mosaic(bimodal(np.random.default_rng(7000 + k)), 128, k, 0.05, blobs(4, 5))))))
    k = 0
    for amps, fracs, depths in (([0.0, 0.02, 0.05, 0.1, 0.2, 0.4], [0.25, 0.45, 0.65], [3.0, 5.0]),
                                ([0.6, 0.9, 1.3], [0.15, 0.35, 0.55], [2.0, 4.0])):
        for a in amps:
            for f in fracs:
                for d in depths:
                    out.append((f'sky-{k:02d}', 0, make(lambda: rgb(sky_scene(a, f, d, 9000 + k)))))
                    k += 1
    k = 0
    for a in [0.02, 0.1, 0.3, 0.7]:
        for d in [4.0, 7.0, 10.0]:
            for broad in [0.0, 1.0]:
                out.append((f'ground-{k:02d}', 0, make(lambda: rgb(ground_scene(a, d, broad, 9500 + k)))))
                k += 1
    for k in range(6):
        out.append((f'chroma-{k:02d}', 128, make(lambda: chroma_scene(k))))
    for seed in (0, 1):
        for kind, value in (('c', 2.5), ('n', 0.95)):
            for i, frac in enumerate([0.0005, 0.005, 0.03, 0.12]):
                out.append((f'clipwb-{seed}{kind}{i}', 128, make(lambda: square_scene(seed, value, frac)), NEUTRAL['A']))
    for k in range(18):
        image, neutral = clipped_wb(k) if images else (None, None)
        out.append((f'clipwb2-{k:02d}', 128, image, neutral))
    for seed in (0, 1):
        for kind, value in (('near', 0.95), ('c1', 1.2), ('c2', 1.7), ('c3', 2.5)):
            for i, frac in enumerate([0.002, 0.03, 0.12]):
                out.append((f'partial-{seed}{kind}{i}', 128, make(lambda: square_scene(seed, value, frac)), NEUTRAL['A']))
    for k in range(60):
        rng = np.random.default_rng(2026 + k)
        patch = 256 if k % 2 == 0 else 128
        out.append((f'rand-{k:02d}', patch, make(lambda: rgb(mosaic(histograms(rng, k, patch), patch, k, 0.05, blobs(4, 5))))))
    for k in range(60):
        rng = np.random.default_rng(3030 + k)
        patch = 256 if k % 2 == 0 else 128
        amp = float(np.exp(rng.uniform(np.log(0.02), np.log(1.0))))
        sigma = int(rng.choice([2, 4, 8, 16]))
        out.append((f'amp-{k:02d}', patch, make(lambda: rgb(mosaic(histograms(rng, k, patch), patch, 1000 + k, amp, blobs(sigma, 100 + sigma))))))
    for k in range(40):
        rng = np.random.default_rng(909 + k)
        patch = 64 if k % 2 == 0 else 128
        amp = float(np.exp(rng.uniform(np.log(0.05), np.log(0.6))))
        sigma = int(rng.choice([2, 4, 8]))
        out.append((f'minority-{k:02d}', patch, make(lambda: rgb(mosaic(minority(rng, patch, False), patch, k, amp, blobs(sigma, 500 + sigma))))))
    for k in range(30):
        rng = np.random.default_rng(4242 + k)
        amp = float(np.exp(rng.uniform(np.log(0.05), np.log(0.5))))
        sigma = int(rng.choice([2, 4]))
        out.append((f'spots-{k:02d}', 32, make(lambda: rgb(mosaic(minority(rng, 32, True), 32, k, amp, blobs(sigma, 700 + sigma))))))
    # Histograms of the training photos' base levels, two mosaics each.
    # `photo[0-9]`: these mosaics' own maps are named photo-NN-a… and must not be read as photos.
    for npz in sorted((photos_dir or root / 'maps').glob('photo[0-9]*.npz'))[:26]:
        if images:
            m = np.load(npz)
            ph = Photo(logs=m['logs'], base=m['base'])
            q = np.sort(ph.base.ravel())
            rng = np.random.default_rng(77 + int(npz.stem[5:]))
        for amp, sigma, patch in [(0.1, 3, 128), (0.3, 6, 256)]:
            image = None
            if images:
                n = (N // patch) ** 2
                levels = np.quantile(q, (np.arange(n) + 0.5) / n) - ph.top + np.log2(rng.uniform(0.15, 0.5))
                image = rgb(mosaic(levels, patch, int(npz.stem[5:]), amp, blobs(sigma, 300 + sigma)))
            out.append((f'photo-{npz.stem[5:]}-a{amp}', patch, image))
    return [job if len(job) == 4 else (*job, (1., 1., 1.)) for job in out]


def probes(args):
    d = args.root / 'probes'
    d.mkdir(parents=True, exist_ok=True)
    jobs = []
    for name, _, image, neutral in probe_jobs(args.root, args.histograms):
        source = d / f'{name}.dng'
        if not source.exists():
            probe_dng.write(source, image, baseline_exposure=0., black_render=1, as_shot_neutral=neutral)
        jobs += [dict(source=str(source), settings=s, out=str(d / f'{name}-{n}.tif')) for n, s in SETTINGS.items()]
    failures = camera_raw.render(jobs, work=d / 'work')
    print(f'{len(jobs)} renders, {len(failures)} failed')


def photos(args):
    dng_dir, cr_dir = args.root / 'photos' / 'dng', args.root / 'photos' / 'crlin'
    dng_dir.mkdir(parents=True, exist_ok=True)
    cr_dir.mkdir(exist_ok=True)
    raws = list(args.raws)
    if args.list:
        dataset = Path(os.environ['RAWMAKASE_DATASET'])
        raws += [dataset / r for r in json.loads(args.list.read_text())]
    jobs = []
    first = max((int(p.stem) for p in dng_dir.glob('*.dng') if p.stem.isdigit()), default=-1) + 1
    for i, raw in enumerate(raws, first):
        dng = dng_dir / f'{i:02d}.dng'
        if not dng.exists():
            tmp = dng_dir / 'tmp'
            tmp.mkdir(exist_ok=True)
            subprocess.run([sys.executable, '-I', str(HERE / 'linear_profile.py'), str(tmp), str(raw)], check=True)
            next(tmp.glob('*.dng')).rename(dng)
        pairs = {n: {**SETTINGS[a], **SETTINGS[b]} for n, (a, b) in PAIRS.items()}
        for n, s in {**SETTINGS, **dict(HALVES.values()), **pairs}.items():
            jobs.append(dict(source=str(dng), settings={'CameraProfile': linear_profile.NAME, **s},
                             out=str(cr_dir / f'{i:02d}-{n}.tif'), edge=2048))
    failures = camera_raw.render(jobs, work=args.root / 'photos' / 'work')
    print(f'{len(jobs)} renders, {len(failures)} failed')


def engine_map(rawmakase, dng, out, settings):
    """The engine's own measurement map of `dng` (log luminance and base level, 512 px)
    and its white curve there (scene value against output, from 2^-6 to 2^5, so also
    above the photo's top), dumped under RAWMAKASE_RULE_DUMP by `local_tone_rule.rs`
    and `pipeline/tone.rs`, as `out` (npz). The scene tone stage applies the rule's
    gain before that curve, which is fitted per photo and compresses everything near
    the photo's top. A map from before the engine wrote the curve is made again."""
    if out.exists() and 'white' in np.load(out).files:
        return
    tmp = Path(f'/tmp/rule-map-{os.getpid()}')
    for p in (tmp, tmp.with_suffix('.map'), tmp.with_suffix('.white')):
        p.unlink(missing_ok=True)
    tmp.with_suffix('.xmp').write_text(camera_raw.xmp({**settings, 'Shadows2012': '100'}))
    env = dict(os.environ, RAWMAKASE_RULE_DUMP=str(tmp))
    subprocess.run([str(rawmakase), 'render', str(dng), str(tmp.with_suffix('.tif')), '--tap', 'scene-input',
                    '--overwrite', '--xmp', str(tmp.with_suffix('.xmp'))], env=env, capture_output=True)
    raw = np.fromfile(tmp.with_suffix('.map'), dtype=np.float32)
    w, h = int(raw[0]), int(raw[1])
    n = w * h
    white = np.fromfile(tmp.with_suffix('.white'), dtype=np.float32).reshape(-1, 2).T
    np.savez(out, logs=raw[2:2 + n].reshape(h, w), base=raw[2 + n:2 + 2 * n].reshape(h, w), white=white)


def engine_curves(rawmakase, dng, out, settings):
    """The rule's four curves of `dng` as the engine built them (`RuleCurve::ALL`'s order:
    Shadows +100, Highlights −100, Shadows −100, Highlights +100): their 48-column
    tables and the levels they span, from the JSON lines of its dump, as `out` (npz)."""
    tmp = Path(f'/tmp/rule-curves-{os.getpid()}')
    for p in (tmp, tmp.with_suffix('.map'), tmp.with_suffix('.white')):
        p.unlink(missing_ok=True)
    tmp.with_suffix('.xmp').write_text(camera_raw.xmp({**settings, 'Shadows2012': '100'}))
    env = dict(os.environ, RAWMAKASE_RULE_DUMP=str(tmp))
    subprocess.run([str(rawmakase), 'render', str(dng), str(tmp.with_suffix('.tif')), '--tap', 'scene-input',
                    '--overwrite', '--xmp', str(tmp.with_suffix('.xmp'))], env=env, capture_output=True)
    lines = [json.loads(line) for line in tmp.read_text().splitlines() if line.strip()][-4:]
    if len(lines) != 4 or not lines[0]['shadows'] or any(line['shadows'] for line in lines[1:]):
        raise SystemExit(f'{dng}: the dump does not end with the four curves of one map')
    np.savez(out, tables=np.array([line['table'] for line in lines], np.float32),
             lo=np.float32(lines[0]['lo']), hi=np.float32(lines[0]['hi']))


def taps(args):
    """The engine's measurement map and white curve of every probe and photo, and each
    photo's four rule curves."""
    out = args.root / 'maps'
    out.mkdir(parents=True, exist_ok=True)
    (args.root / 'curves').mkdir(exist_ok=True)
    for name, _, _, _ in probe_jobs(args.root, args.histograms, images=False):
        engine_map(args.rawmakase, args.root / 'probes' / f'{name}.dng', out / f'{name}.npz', {})
    for dng in sorted((args.root / 'photos' / 'dng').glob('*.dng')):
        settings = {'CameraProfile': linear_profile.NAME}
        engine_map(args.rawmakase, dng, out / f'photo{dng.stem}.npz', settings)
        engine_curves(args.rawmakase, dng, args.root / 'curves' / f'photo{dng.stem}.npz', settings)


# --- training ------------------------------------------------------------------------

def cr_linear(path, peak=False):
    """Luminance of a Camera Raw render, or its brightest channel (`peak`): the TIFF, or
    the `.npy` kept in its place (a luminance-only one gives luminance for both)."""
    npy = Path(path).with_suffix('.npy')
    if npy.exists():
        a = np.maximum(np.load(npy).astype(np.float64), 0)
    else:
        import tifffile
        a = (tifffile.imread(str(path)).astype(np.float64)[..., :3] / 65535.) ** 1.8
    if a.ndim == 2:
        return a
    return a.max(-1) if peak else a @ YW


def rendered(path):
    return Path(path).exists() or Path(path).with_suffix('.npy').exists()


def aligned(ph, images, coarse=False):
    """`images` (Camera Raw's renders, in the display's orientation) rotated into the
    engine map's orientation: the rotation whose log luminance correlates best with the
    map's; None when none reaches 0.8 (another crop, or a failed conversion). With
    `coarse` (the second ensemble's samples; the first was fitted without it), a render
    that fails so is taken at the rotation that correlates on an eighth-size copy."""
    from PIL import Image
    h, w = ph.base.shape
    best = None
    for rot in range(4):
        c = np.rot90(images[0], rot)
        if (c.shape[0] > c.shape[1]) != (h > w):
            continue
        small = np.asarray(Image.fromarray(c.astype(np.float32), 'F').resize((w, h), Image.BOX), dtype=np.float64)
        corr = np.corrcoef(ph.logs.ravel(), np.log2(np.maximum(small, FLOOR)).ravel())[0, 1]
        if np.isfinite(corr) and (best is None or corr > best[0]):
            best = (corr, rot)
    if best is not None and best[0] >= 0.8:
        return [np.rot90(a, best[1]) for a in images]
    if not coarse:
        return None
    # A render a pixel or two off the map (lens geometry) decorrelates fine texture:
    # take the rotation that correlates on an eighth-size copy, at the same aspect.
    rs = lambda a: np.asarray(Image.fromarray(a.astype(np.float32), 'F').resize((w // 8, h // 8), Image.BOX), dtype=np.float64)
    best = None
    for rot in range(4):
        c = np.rot90(images[0], rot)
        if (c.shape[0] > c.shape[1]) != (h > w) or abs(c.shape[0] / c.shape[1] - h / w) > 0.01:
            continue
        corr = np.corrcoef(rs(ph.logs).ravel(), np.log2(np.maximum(rs(c), FLOOR)).ravel())[0, 1]
        if np.isfinite(corr) and (best is None or corr > best[0]):
            best = (corr, rot)
    if best is None or best[0] < 0.8:
        return None
    return [np.rot90(a, best[1]) for a in images]


def on_map(ph, image):
    """`image` at the size of `ph`'s map."""
    from PIL import Image
    return np.asarray(Image.fromarray(image.astype(np.float32), 'F').resize(ph.base.shape[::-1], Image.BOX), dtype=np.float64)


def samples(key, root, images, rng, n=2500):
    """The first ensemble's inputs (`FEAT`) and targets (S+100, H−100) at `n` map pixels
    of one mosaic or photo; `images`: Camera Raw's luminance at default and at the two."""
    m = np.load(root / 'maps' / f'{key}.npz')
    ph = Photo(logs=m['logs'], base=m['base'])
    images = aligned(ph, images)
    if images is None:
        print('misaligned', key)
        return None
    gains, valid = [], np.ones(ph.base.shape, bool)
    for setting in images[1:]:
        g, v = targets(images[0], setting, size=ph.base.shape)
        gains.append(g)
        valid &= v
    idx = np.flatnonzero(valid.ravel())
    idx = rng.choice(idx, min(n, idx.size), replace=False)
    return dict(key=key, X=ph.features(ph.base.ravel()[idx]), Y=np.stack([g.ravel()[idx] for g in gains], 1))


def samples_ext(key, root, images, halves, peaks, rng, n=2500):
    """The second ensemble's inputs (`FEAT_EXT`), targets (`OUTPUTS`) and their censoring
    (`C`: 0 an exact target, +1 a lower bound, −1 an upper bound) at `n` map pixels of one
    mosaic or photo. `images`: Camera Raw's luminance at default and at each of
    `OUTPUTS`; `halves`: at each of `HALVES`, or none; `peaks`: the brightest channel of
    each of the two lists' renders, in their order.

    The engine applies the rule's gain before its white curve (the map's `white`) and
    Camera Raw's renders show the gain after its own, so the targets are taken with the
    engine's curve undone: near the photo's top the engine shows only a part of a gain.
    Where the full slider's result is clipped, the render shows only a bound on the
    gain. At the black floor it is at most what is seen. At white (in any channel: a
    colored highlight clips one channel well before its luminance nears white, and
    its luminance then understates the gain) it is at least what takes the brightest
    channel to the level the engine renders white, and `CLIP_MARGIN` more. Camera Raw's
    gain is linear in the slider before the white curve, so the half-slider render tells
    more there: twice its gain, exact where it is unclipped itself and a tighter bound
    where not."""
    m = np.load(root / 'maps' / f'{key}.npz')
    ph = Photo(logs=m['logs'], base=m['base'])
    count, total = len(images), len(images) + len(halves)
    images = aligned(ph, images + halves + peaks, coarse=True)
    if images is None:
        print('misaligned', key)
        return None
    images, peaks = images[:total], images[total:]
    # The curve up to where it reaches white; past that every scene value renders white.
    x, y = m['white']
    rising = np.r_[True, np.diff(y) > 0]
    x, y = x[rising], y[rising]
    level = lambda a: np.log2(np.maximum(on_map(ph, np.where(a > y[0], np.interp(a, y, x), a)), FLOOR))
    default = level(images[0])
    valid = (on_map(ph, images[0]) > CLIP_LOW) & (on_map(ph, peaks[0]) < WHITE)
    idx = np.flatnonzero(valid.ravel())
    if idx.size < 200:
        return None
    idx = rng.choice(idx, min(n, idx.size), replace=False)
    gain = lambda k: (level(images[k]) - default).ravel()[idx]
    at = lambda group, k: on_map(ph, group[k]).ravel()[idx]
    to_white = (np.log2(x[-1]) + CLIP_MARGIN - level(peaks[0])).ravel()[idx]
    Y = np.stack([gain(k) for k in range(1, count)], 1)
    C = np.zeros(Y.shape, np.int8)
    for output, sign in (('H+100', 1), ('S-100', -1)):
        clipped = (lambda k: at(peaks, k) >= CLIP_HIGH) if sign > 0 else (lambda k: at(images, k) <= CLIP_LOW)
        j = OUTPUTS.index(output)
        bound = clipped(1 + j)
        if sign > 0:
            Y[bound, j] = to_white[bound]
        if halves:
            h = count + list(HALVES).index(output)
            twice = 2 * gain(h)
            exact = bound & ~clipped(h)
            Y[exact, j] = twice[exact]
            bound &= ~exact
            Y[bound, j] = 2 * to_white[bound] if sign > 0 else np.minimum(Y[bound, j], twice[bound])
        C[bound, j] = sign
    return dict(key=key, X=ph.features_ext(ph.base.ravel()[idx]), Y=Y, C=C)


def sample_job(job):
    kind, key, root, renders, halves, ext, seed = job
    rng = np.random.default_rng(seed)
    if ext:
        smp = samples_ext(key, root, [cr_linear(r) for r in renders], [cr_linear(r) for r in halves],
                          [cr_linear(r, peak=True) for r in renders + halves], rng)
    else:
        smp = samples(key, root, [cr_linear(r) for r in renders], rng)
    return dict(smp, kind=kind) if smp else None


def stamp(path):
    """A render's or a map's identity for the sample cache: the file read in its place
    (`cr_linear`), its size and its modification time."""
    path = Path(path)
    path = path.with_suffix('.npy') if path.suffix == '.tif' and path.with_suffix('.npy').exists() else path
    st = path.stat()
    return [path.name, st.st_size, st.st_mtime_ns]


def load(root, ext, photos_dir=None):
    """The sample sets of the second ensemble (`ext`) or the first, cached in
    ROOT/samples_ext.pkl or ROOT/samples.pkl with what they were built from: the
    feature list, the constants the targets depend on and every map and render read. A
    cache built from anything else is rebuilt."""
    cache = root / ('samples_ext.pkl' if ext else 'samples.pkl')
    settings = list(SETTINGS) if ext else list(SETTINGS)[:3]
    jobs = []
    d = root / 'probes'
    for name, _, _, _ in probe_jobs(root, photos_dir, images=False):
        if all(rendered(d / f'{name}-{s}.tif') for s in settings) and (root / 'maps' / f'{name}.npz').exists():
            jobs.append(('mosaic', name, root, [d / f'{name}-{s}.tif' for s in settings], [], ext, len(jobs)))
    for npz in sorted((root / 'maps').glob('photo[0-9]*.npz')):
        i = npz.stem[5:]
        cr = root / 'photos' / 'crlin'
        if not all(rendered(cr / f'{i}-{s}.tif') for s in settings):
            continue
        renders, halves = [cr / f'{i}-{s}.tif' for s in settings], []
        if ext:
            halves = [cr / f'{i}-{s}.tif' for s, _ in HALVES.values()]
            halves = halves if all(rendered(h) for h in halves) else []
        jobs.append(('photo', f'photo{i}', root, renders, halves, ext, len(jobs)))
    made_from = dict(
        features=FEAT_EXT if ext else FEAT,
        constants=dict(BOTTOM_GAP=BOTTOM_GAP, WHITE=WHITE, CLIP_HIGH=CLIP_HIGH, CLIP_LOW=CLIP_LOW, CLIP_MARGIN=CLIP_MARGIN) if ext else {},
        inputs=[[key, stamp(root / 'maps' / f'{key}.npz')] + [stamp(r) for r in renders + halves]
                for _, key, _, renders, halves, _, _ in jobs])
    if cache.exists():
        cached = pickle.loads(cache.read_bytes())
        if isinstance(cached, dict) and cached.get('made_from') == made_from:
            return cached['data']
        print('rebuilding the sample cache: its features, constants or inputs differ')
    # Reading 2500 renders takes most of an hour on one core; fork keeps mlx (imported
    # lazily by `fit`) out of the workers.
    import multiprocessing
    with multiprocessing.get_context('fork').Pool(8) as pool:
        data = [d for d in pool.imap(sample_job, jobs, chunksize=4) if d]
    print(f'{len(data)} of {len(jobs)} sample sets')
    cache.write_bytes(pickle.dumps(dict(made_from=made_from, data=data)))
    return data


def fit(X, Y, C, mu, sd, steps=6000, seed=0, lr=2e-3):
    """A network from `X` to `Y`. Where `C` is +1 the target is a lower bound and costs
    only a prediction under it; where −1 an upper bound, only a prediction over it."""
    import mlx.core as mx
    import mlx.nn as nn
    import mlx.optimizers as optim

    class MLP(nn.Module):
        def __init__(s, n, h=HIDDEN):
            super().__init__()
            s.l1, s.l2, s.l3 = nn.Linear(n, h), nn.Linear(h, h), nn.Linear(h, Y.shape[1])

        def __call__(s, x):
            return s.l3(nn.silu(s.l2(nn.silu(s.l1(x)))))

    mx.random.seed(seed)
    m, opt = MLP(X.shape[1]), optim.Adam(lr)
    x, y, c = mx.array(((X - mu) / sd).astype(np.float32)), mx.array(Y.astype(np.float32)), mx.array(C.astype(np.float32))

    def loss(m, xb, yb, cb):
        d = m(xb) - yb
        d = mx.where(cb > 0, mx.minimum(d, 0), mx.where(cb < 0, mx.maximum(d, 0), d))
        return (d ** 2).mean()

    g = nn.value_and_grad(m, loss)
    rng = np.random.default_rng(seed)
    for _ in range(steps):
        b = mx.array(rng.integers(0, x.shape[0], 8192))
        _, grads = g(m, x[b], y[b], c[b])
        opt.update(m, grads)
        mx.eval(m.parameters(), opt.state)
    predict = lambda Xn: np.array(m(mx.array(((Xn - mu) / sd).astype(np.float32))))
    weights = {k: np.array(v).tolist() for k, v in [('l1.w', m.l1.weight), ('l1.b', m.l1.bias), ('l2.w', m.l2.weight),
                                                     ('l2.b', m.l2.bias), ('l3.w', m.l3.weight), ('l3.b', m.l3.bias)]}
    return predict, weights


def report(predict, ds, label, outputs=OUTPUTS):
    """Per-set rms error of the exact targets. Bounds are left out, and so is a set for
    an output all of whose targets are bounds; the count scored is printed when sets
    were left out."""
    def rms(d):
        exact = d['C'] == 0
        n = exact.sum(0)
        return np.where(n > 0, np.sqrt(((predict(d['X']) - d['Y']) ** 2 * exact).sum(0) / np.maximum(n, 1)), np.nan)
    r = np.array([rms(d) for d in ds])
    scored = lambda k: '' if np.isfinite(r[:, k]).all() else f' ({int(np.isfinite(r[:, k]).sum())} scored)'
    print(f'  {label} (n={len(ds)}): ' + ' | '.join(
        f'{o} rms median {np.nanmedian(r[:, k]):.3f} mean {np.nanmean(r[:, k]):.3f}{scored(k)}' for k, o in enumerate(outputs)))


def train(args):
    out = args.out or args.root / 'global_rule.json'
    # The second ensemble fits every target; the engine keeps EXT_OUTPUTS.
    ensemble(load(args.root, True, args.histograms), OUTPUTS, args.cv, out.with_name(out.stem + '_ext.json'), DATA_EXT, EXT_OUTPUTS)
    if args.first:
        ensemble(load(args.root, False, args.histograms), OUTPUTS[:2], args.cv, out, DATA, OUTPUTS[:2])


def ensemble(data, outputs, cv, json_path, rust_path, keep):
    """Fits MEMBERS networks to the samples' targets `outputs` and writes those in `keep`."""
    data = [dict(d, C=d.get('C', np.zeros(d['Y'].shape, np.int8))) for d in data]
    mos, pho = [d for d in data if d['kind'] == 'mosaic'], [d for d in data if d['kind'] == 'photo']
    stack = lambda ds: tuple(np.concatenate([d[k] for d in ds]) for k in 'XYC')
    Xall = np.concatenate([d['X'] for d in data])
    mu, sd = Xall.mean(0), Xall.std(0) + 1e-6
    if cv:
        folds = np.array_split(np.random.default_rng(1).permutation(len(pho)), 4)
        for f in folds:
            held = [pho[i] for i in f]
            predict, _ = fit(*stack(mos + [pho[i] for i in range(len(pho)) if i not in f] * PHOTO_WEIGHT), mu, sd, steps=4000)
            report(predict, held, 'held-out photos', outputs)
    members = [fit(*stack(mos + pho * PHOTO_WEIGHT), mu, sd, seed=seed) for seed in range(MEMBERS)]
    predict = lambda Xn: np.mean([p(Xn) for p, _ in members], 0)
    report(predict, pho, 'photos (in-sample, ensemble)', outputs)
    report(predict, mos, 'mosaics (in-sample, ensemble)', outputs)
    rows = [outputs.index(o) for o in keep]
    kept = [dict(w, **{'l3.w': np.array(w['l3.w'])[rows].tolist(), 'l3.b': np.array(w['l3.b'])[rows].tolist()}) for _, w in members]
    names = (FEAT_EXT if rust_path == DATA_EXT else FEAT)
    rule = dict(features=names, outputs=keep, mu=mu.tolist(), sd=sd.tolist(), members=kept)
    json_path.write_text(json.dumps(rule))
    rust_path.write_text(rust(rule, rust_path == DATA_EXT))
    print(f'-> {rust_path.relative_to(ROOT)}')


def rust(g, ext=False):
    members, hidden, features = len(g['members']), len(g['members'][0]['l1.b']), len(g['mu'])
    # One row of numbers per line: rustfmt would put each number on a line of its own.
    def arr(name, v, ty):
        v = np.array(v, dtype=np.float32)
        row = lambda r: '[' + ', '.join(f'{x:.7}' for x in r) + ']'
        item = f'#[rustfmt::skip]\npub(super) static {name}: {ty} = '
        if v.ndim == 1:
            return f'{item}{row(v)};\n'
        if v.ndim == 2:
            return f'{item}[\n    ' + ',\n    '.join(row(r) for r in v) + ',\n];\n'
        mats = ',\n    '.join('[\n        ' + ',\n        '.join(row(r) for r in m) + ',\n    ]' for m in v)
        return f'{item}[\n    {mats},\n];\n'
    about = ('//! The global rule of negative Shadows and positive Highlights: an ensemble of small networks\n'
             '//! from histogram and per-level texture descriptors of a base level, and from the level and the\n'
             "//! photo's top, median and mean themselves, to the log2 gain at Shadows −100 and Highlights\n"
             '//! +100, fitted from different seeds to Camera Raw 18.7 on textured flat-patch mosaics and\n'
             "//! linear-profile training photos, from the engine's own measurement maps\n" if ext else
             '//! The global Shadows/Highlights rule: an ensemble of small networks from histogram and\n'
             '//! per-level texture descriptors of a base level to the log2 gain at Shadows +100 and\n'
             '//! Highlights −100, fitted from different seeds to Camera Raw 18.7 on textured flat-patch\n'
             "//! mosaics and linear-profile training photos, from the engine's own measurement maps\n")
    head = (about +
            '//! (scripts/corpus/scene-tone-rule.py). Generated; do not edit.\n'
            '#![allow(\n    clippy::excessive_precision,\n    clippy::approx_constant,\n    clippy::unreadable_literal\n)]\n'
            f'/// Networks in the ensemble.\npub(super) const MEMBERS: usize = {members};\n'
            f'/// Units in each hidden layer.\npub(super) const HIDDEN: usize = {hidden};\n'
            f'/// Inputs the networks read.\npub(super) const FEATURES: usize = {features};\n')
    stack = lambda k: [m[k] for m in g['members']]
    return head + arr('MU', g['mu'], '[f32; FEATURES]') + arr('SD', g['sd'], '[f32; FEATURES]') + \
        arr('W1', stack('l1.w'), '[[[f32; FEATURES]; HIDDEN]; MEMBERS]') + arr('B1', stack('l1.b'), '[[f32; HIDDEN]; MEMBERS]') + \
        arr('W2', stack('l2.w'), '[[[f32; HIDDEN]; HIDDEN]; MEMBERS]') + arr('B2', stack('l2.b'), '[[f32; HIDDEN]; MEMBERS]') + \
        arr('W3', stack('l3.w'), '[[[f32; HIDDEN]; 2]; MEMBERS]') + arr('B3', stack('l3.b'), '[[f32; 2]; MEMBERS]')


# --- Shadows and Highlights together -----------------------------------------------------

def curve_at(curves, k, base):
    """Curve `k` of a photo's `curves` (engine_curves) at base levels, as `Curve::eval` reads it."""
    table, lo, hi = curves['tables'][k].astype(np.float64), float(curves['lo']), float(curves['hi'])
    f = np.clip((np.asarray(base, np.float64) - lo) / (hi - lo) * 48 - 0.5, 0, 47)
    i = np.minimum(f.astype(int), 46)
    return table[i] + (table[i + 1] - table[i]) * (f - i)


def pair_sample(job):
    """One photo for `pair`: per pair of `PAIRS`, at up to 6000 map pixels where none of
    the renders is clipped, what blending each curve toward its extreme adds per unit
    of its fraction (`X`, Shadows' then Highlights'), what the sum of the engine's two
    curves is off Camera Raw's combined gain (`E`, the engine's white curve undone as in
    `samples_ext`), Camera Raw's own difference from the sum of its single gains (`I`)
    and each curve's own error at its single setting (`S`); and the inputs the engine
    predicts the fractions from (`F`, `RuleStats::pair`)."""
    key, root = job
    cr = root / 'photos' / 'crlin'
    m, curves = np.load(root / 'maps' / f'{key}.npz'), np.load(root / 'curves' / f'{key}.npz')
    ph = Photo(logs=m['logs'], base=m['base'])
    names = list(SETTINGS) + list(PAIRS)
    lum = [cr_linear(cr / f'{key[5:]}-{n}.tif') for n in names]
    peak = [cr_linear(cr / f'{key[5:]}-{n}.tif', peak=True) for n in names]
    images = aligned(ph, lum + peak, coarse=True)
    if images is None:
        print('misaligned', key)
        return None
    lum, peak = images[:len(names)], images[len(names):]
    x, y = m['white']
    rising = np.r_[True, np.diff(y) > 0]
    x, y = x[rising], y[rising]
    level = lambda a: np.log2(np.maximum(on_map(ph, np.where(a > y[0], np.interp(a, y, x), a)), FLOOR))
    valid = np.ones(ph.base.shape, bool)
    for a, b in zip(lum, peak):
        valid &= (on_map(ph, a) > CLIP_LOW) & (on_map(ph, b) < CLIP_HIGH)
    idx = np.flatnonzero(valid.ravel())
    if idx.size < 2000:
        print('too few unclipped pixels', key)
        return None
    idx = np.random.default_rng(int(key[5:])).choice(idx, min(6000, idx.size), replace=False)
    default = level(lum[0])
    gain = {n: (level(a) - default).ravel()[idx] for n, a in zip(names[1:], lum[1:])}
    base = ph.base.ravel()
    ordered = np.sort(base.astype(np.float32))
    at = lambda q: ordered[int(np.float32(base.size - 1) * np.float32(q))]
    kinds = list(OUTPUTS)    # RuleCurve::ALL's order
    extreme = {n: float(curve_at(curves, k, at(PAIR_LOW if n[0] == 'S' else PAIR_HIGH))) for k, n in enumerate(kinds)}
    reach = {n: float(np.clip(curve_at(curves, k, base) / extreme[n], 0, 1).mean()) if extreme[n] else 0.
             for k, n in enumerate(kinds)}
    out = dict(key=key, X=[], E=[], I=[], S=[], F=[])
    for name, (sn, hn) in PAIRS.items():
        a, b = curve_at(curves, kinds.index(sn), base[idx]), curve_at(curves, kinds.index(hn), base[idx])
        out['X'].append(np.stack([extreme[sn] - a, extreme[hn] - b], 1))
        out['E'].append(a + b - gain[name])
        out['I'].append(gain[name] - gain[sn] - gain[hn])
        out['S'].append(np.stack([a - gain[sn], b - gain[hn]], 1))
        es, eh = abs(extreme[sn]), abs(extreme[hn])
        out['F'].append(np.array([reach[sn], reach[hn], es, eh, 1 / max(es, PAIR_EXTREME_MIN), 1 / max(eh, PAIR_EXTREME_MIN)]))
    return out


def pair_fit(data, c):
    """For pair `c`, the inputs' means and deviations and each slider's intercept and
    weights: the linear map from a photo's inputs to its two fractions that leaves the
    least of `E` over the photos (each photo's pixels weighing as one photo)."""
    F = np.array([d['F'][c] for d in data])
    mu, sd = F.mean(0), F.std(0) + 1e-9
    rows, target = [], []
    for d, z in zip(data, (F - mu) / sd):
        p = np.r_[1., z]
        w = 1 / np.sqrt(len(d['E'][c]))
        rows.append(np.concatenate([d['X'][c][:, :1] * p, d['X'][c][:, 1:] * p], 1) * w)
        target.append(-d['E'][c] * w)
    A, t = np.concatenate(rows), np.concatenate(target)
    n = F.shape[1] + 1
    ridge = PAIR_RIDGE * len(data) / 100 * np.eye(2 * n)
    ridge[0, 0] = ridge[n, n] = 0
    beta = np.linalg.solve(A.T @ A + ridge, A.T @ t)
    return mu, sd, beta.reshape(2, n)


def pair_fractions(model, d, c):
    mu, sd, beta = model
    return np.clip(beta @ np.r_[1., (d['F'][c] - mu) / sd], 0, 1)


def pair(args):
    """Fits `Pair`'s fractions and writes local_tone_rule_pair_data.rs."""
    keys = [p.stem for p in sorted((args.root / 'maps').glob('photo[0-9]*.npz'))
            if (args.root / 'curves' / p.name).exists()
            and all(rendered(args.root / 'photos' / 'crlin' / f'{p.stem[5:]}-{n}.tif') for n in list(SETTINGS) + list(PAIRS))]
    import multiprocessing
    with multiprocessing.get_context('fork').Pool(6) as pool:
        data = [d for d in pool.imap(pair_sample, [(k, args.root) for k in keys], chunksize=4) if d]
    print(f'{len(data)} of {len(keys)} photos')
    rms = lambda v: float(np.sqrt(np.mean(v ** 2)))
    line = lambda label, v: print(f'  {label:48s}' + '  '.join(f'{np.median(v[:, c]):.3f} ({v[:, c].mean():.3f})' for c in range(4)))
    print('gain error at each pair, RMS over a photo\'s pixels (EV), median (mean) over photos: ' + ', '.join(PAIRS))
    line('Camera Raw\'s gain less the sum of its singles', np.array([[rms(d['I'][c]) for c in range(4)] for d in data]))
    for k, label in enumerate(('Shadows', 'Highlights')):
        line(f'the engine\'s {label} curve alone', np.array([[rms(d['S'][c][:, k]) for c in range(4)] for d in data]))
    plain = np.array([[rms(d['E'][c]) for c in range(4)] for d in data])
    line('the sum of the engine\'s two curves', plain)
    own = lambda d, c: np.clip(np.linalg.lstsq(d['X'][c], -d['E'][c], rcond=None)[0], 0, 1)
    line('fractions fitted to each photo', np.array([[rms(d['E'][c] + d['X'][c] @ own(d, c)) for c in range(4)] for d in data]))
    if args.cv:
        fold = np.random.default_rng(0).permutation(len(data)) % 5
        held = np.zeros((len(data), 4))
        for k in range(5):
            train = [d for d, f in zip(data, fold) if f != k]
            for c in range(4):
                model = pair_fit(train, c)
                for i in np.flatnonzero(fold == k):
                    held[i, c] = rms(data[i]['E'][c] + data[i]['X'][c] @ pair_fractions(model, data[i], c))
        line('fractions predicted, photo held out', held)
        print('  photos closer / further by 0.02 EV than the sum: ' + '  '.join(
            f'{int((held[:, c] < plain[:, c] - 0.02).sum())} / {int((held[:, c] > plain[:, c] + 0.02).sum())}' for c in range(4)))
    models = [pair_fit(data, c) for c in range(4)]
    fitted = np.array([[rms(d['E'][c] + d['X'][c] @ pair_fractions(models[c], d, c)) for c in range(4)] for d in data])
    line('fractions predicted, fitted on all', fitted)
    fractions = np.array([[pair_fractions(models[c], d, c) for c in range(4)] for d in data])
    print('  median fractions (Shadows, Highlights): ' + '  '.join(
        f'{np.median(fractions[:, c, 0]):.2f}, {np.median(fractions[:, c, 1]):.2f}' for c in range(4)))
    arr = lambda v: '[' + ', '.join(repr(float(np.float32(x))) for x in v) + ']'
    DATA_PAIR.write_text(
        "//! How Shadows' and Highlights' fractions follow the photo (`local_tone_rule::Pair`):\n"
        "//! per sign combination, the inputs' means and deviations and, for each slider, the\n"
        "//! intercept and weights. Written by `scripts/corpus/scene-tone-rule.py pair`.\n"
        '#![allow(clippy::excessive_precision, clippy::unreadable_literal)]\n'
        # One row of numbers per line: rustfmt would put each number on a line of its own.
        + '#[rustfmt::skip]\npub(super) const MU: [[f32; 6]; 4] = [\n' + ''.join(f'    {arr(m[0])},\n' for m in models) + '];\n'
        + '#[rustfmt::skip]\npub(super) const SD: [[f32; 6]; 4] = [\n' + ''.join(f'    {arr(m[1])},\n' for m in models) + '];\n'
        + '#[rustfmt::skip]\npub(super) const BETA: [[[f32; 7]; 2]; 4] = [\n'
        + ''.join(f'    [\n        {arr(m[2][0])},\n        {arr(m[2][1])},\n    ],\n' for m in models) + '];\n')
    print('wrote', DATA_PAIR)


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest='step', required=True)
    photo_set = dict(type=Path, help="maps of the photos whose histograms the photo mosaics take, when they "
                                     "are not ROOT/maps' first 26 (the mosaics of 2026-10-30 came from an "
                                     "earlier photo set than the second ensemble's photos)")
    s = sub.add_parser('probes')
    s.add_argument('root', type=Path)
    s.add_argument('--histograms', **photo_set)
    s = sub.add_parser('photos')
    s.add_argument('root', type=Path)
    s.add_argument('raws', type=Path, nargs='*')
    s.add_argument('--list', type=Path, help='JSON list of raw paths relative to $RAWMAKASE_DATASET')
    s = sub.add_parser('taps')
    s.add_argument('root', type=Path)
    s.add_argument('rawmakase', type=Path)
    s.add_argument('--histograms', **photo_set)
    s = sub.add_parser('train')
    s.add_argument('root', type=Path)
    s.add_argument('--cv', action='store_true')
    s.add_argument('--out', type=Path)
    s.add_argument('--first', action='store_true', help='refit the first ensemble (S+100, H-100) too')
    s.add_argument('--histograms', **photo_set)
    s = sub.add_parser('pair')
    s.add_argument('root', type=Path)
    s.add_argument('--cv', action='store_true')
    args = p.parse_args()
    args.root = args.root.expanduser().resolve()
    if getattr(args, 'histograms', None):
        args.histograms = args.histograms.expanduser().resolve()
    {'probes': probes, 'photos': photos, 'taps': taps, 'train': train, 'pair': pair}[args.step](args)


if __name__ == '__main__':
    main()
