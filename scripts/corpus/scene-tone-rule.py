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
  scenes ROOT [--count N]     renders N (4,000) scenes made from the photos' maps in Camera
                              Raw (default and the four full sliders) into ROOT/scenes;
                              run after `taps` has the photos' maps, and `taps` again after
  gaps ROOT                   renders the gap probes in Camera Raw into ROOT/gaps
  taps ROOT RAWMAKASE         dumps the engine's measurement map of every probe, photo,
                              scene and gap probe into ROOT/maps (RAWMAKASE_RULE_DUMP; needs
                              RAWMAKASE_DATA_DIR), with its white curve there, and each
                              photo's four rule curves into ROOT/curves (a build of the
                              ensembles `pair` is for: run it again after `train`)
  train ROOT [--cv] [--out] [--first]
                              builds the samples, optionally cross-validates by photo, fits
                              the second ensemble on everything and writes the JSON and
                              crates/rawmakase-engine/src/develop/local_tone_rule_ext_data.rs;
                              with --first it refits the first ensemble too and writes
                              local_tone_rule_data.rs (both refitted 2026-11-23 on 436
                              mosaics, 467 photos, 4,000 scenes and 128 gap probes)
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

Scenes (`scene`, 2026-11-22) are Camera Raw questioned on input that is known exactly:
a training photo's own map as a grey picture, or two joined, through a random tone
curve and exposure, written as a probe DNG (DefaultBlackRender None). Camera Raw treats
such a picture as it treats the photo (a photo's map rendered as a scene keeps the
photo's departure from the average curve on 9 or 10 of 12), a scene's gain needs no
alignment and carries no default black, and a tone curve gives a distribution of
levels no photo in the set has. Held out by photo, with a scene made from a held-out
photo's map left out too, 4,000 scenes take the first ensemble's error on the photos
from 0.152 to 0.134 EV (median) at Shadows +100 and from 0.101 to 0.092 at Highlights
−100, most of it with the first thousand; a network two or four times as wide is
worse. A scene's or gap probe's samples are 1,500 map pixels, kept as float32.

Gap probes (`gap_plan`, 2026-11-22) measure what a small region is to Camera Raw: a
field with round spots of 2 to 64 pixels' radius covering 0.03 to 10% of a 1024-pixel
frame. One bright spot of 16 pixels' radius (0.12% of the frame) lifts a dark field
0.63 EV at Shadows +100, seventy-five spots of 2 pixels with the same total area 0.04
EV, and with no spot the field is not lifted at all. The field's lift is the same at
1024, 2048 and 4096 pixels for the same layout, so a region counts in the curve from
about 1.5% of the frame's width whatever the frame's size; the gain a spot takes itself
goes by its size in pixels (a dark spot of 2 pixels +0.1 EV, of 4 pixels +1.2, of 8 and
more +1.9). The histogram the rule reads is therefore the log map's under a box blur
of STAT_BLUR map pixels (`Photo`), while a level's gain is read at its base level:
with the histogram from the base levels the rule lifted the dark field 0.6 to 1.0 EV
for spots of any size, and read from the blurred map its error on the photos is no
larger (0.133 and 0.090 EV). Block means in linear light do not do (a few bright
pixels then count); block means that do not overlap alias.

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

The maps' base level is the engine's own (`engine_map`): since 2026-11-23 a local
Laplacian filter of the map (`laplacian_base` in local_tone.rs).

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
import hashlib
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
# `local_tone_rule.rs`: where a curve is read for its extreme, the least extreme the
# fractions' inputs divide by, input z-score clamp, and uniform shift clamp.
PAIR_LOW, PAIR_HIGH, PAIR_EXTREME_MIN = 0.01, 0.99, 0.05
PAIR_Z_CLAMP, PAIR_SHIFT_MAX = 4.0, 1.5
PAIR_RIDGE = 10.0   # on the standardized weights, per hundred photos
# The second ensemble's targets count up to WHITE at default. A Highlights +100 result
# from CLIP_HIGH up sits in Camera Raw's clipping and a Shadows −100 result from
# CLIP_LOW down at its floor: such a render shows only a bound on the gain.
WHITE, CLIP_HIGH, CLIP_LOW = 0.98, 0.97, 2.0 ** -13
# A result at white asks for the level the engine renders white and this many stops
# more: twice the fit's error on held-out photos, so that its scatter does not leave
# half of those pixels just under white.
CLIP_MARGIN = 0.25
PHOTO_WEIGHT = 2    # photo samples against scene/mosaic samples in the fit
PIX_REG = 1e-3      # L2 penalty on G_pix(l) to fix the constant gauge against G_base(b)
HIDDEN = 64         # units in each of the two hidden layers
MEMBERS = 4         # networks in the ensemble (seeds 0..MEMBERS-1), averaged by the engine
STAT_BLUR = 4       # radius, in map pixels, of the box blur the histogram is read under (`STAT_BLUR` in local_tone_rule.rs)
SCENES = 4000       # scenes `scenes` makes
SCENE_SAMPLES = 1500    # map pixels drawn from a scene or gap probe (a mosaic or photo gives 2500)
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
        'tex_at', 'coarse_at', 'm_top1', 'm_top2', 'lin_mean-x', 'p99-x', 'x-p01', 'lin_skew']
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
        # The base levels in order: the per-level texture reads them.
        self.sorted = np.sort(self.base.ravel())
        # The histogram's levels (top, bottom, mean, quantiles, CDF) are the log map's
        # under a box blur: Camera Raw counts a region in its curve from about 1.5% of the
        # frame's width, while the gain is read at the base level (`gaps`).
        b = np.sort(box(self.logs, STAT_BLUR).ravel())
        self.levels = b
        n = b.size
        q = lambda p: b[min(n - 1, int(p * (n - 1)))]
        self.top, self.bot, self.mean = b[-1], q(0.001), b.mean()
        self.lin_mean = float(np.log2(np.mean(np.exp2(b))))
        self.q = {p: q(p) for p in (0.01, 0.1, 0.25, 0.5, 0.75, 0.9, 0.99)}
        d = self.logs - box(self.logs, 2)
        self.tex = float(np.sqrt(np.mean(d * d)))
        top = self.base > self.sorted[-1] - 2
        self.tex_top = float(np.sqrt(np.mean(d[top] ** 2))) if top.any() else self.tex
        # The residuals in order of base level, for the per-level medians; a median
        # ignores the few edge pixels whose residual spans stops.
        order = np.argsort(self.base.ravel(), kind='stable')
        self.fine_by_level = np.abs(d.ravel())[order]
        self.coarse_by_level = np.abs((self.logs - box(self.logs, COARSE_RADIUS)).ravel())[order]
        self.fine_median, self.coarse_median = float(np.median(self.fine_by_level)), float(np.median(self.coarse_by_level))

    def cdf(self, x):
        return np.searchsorted(self.levels, x, side='right') / self.levels.size

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
                         fine, coarse, const(1 - c(self.top - 1)), const(1 - c(self.top - 2)),
                         self.lin_mean - x, self.q[0.99] - x, x - self.q[0.01], const(self.lin_mean - self.mean)], -1)

    def features_first(self, x):
        """Exposure-invariant 28-feature vector the first ensemble reads (`FEAT`),
        without BOTTOM_GAP clamping (`RuleStats::features_first_z`)."""
        return self.features(x, self.bot)

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


# --- scenes and gap probes -----------------------------------------------------------

def scene(root, idx):
    """Scene `idx`, made from the training photos' maps: its image (linear scene values,
    grey, twice the map's size), its luminance at the map's size, and its parameters.

    One photo's map (ROOT/maps/photoNN.npz, its log2 luminance), or two joined along a
    random straight line with the second up to 3 stops off the first; through a random
    monotone tone curve about the scene's key (a slope of 0.5 to 1.74 for each stop);
    exposed −1.5 to +1 stop; with 1% noise a pixel in half of the scenes; clipped at the
    sensor's top, which is 0.65, 1, 1.5 or 2 stops over white (the DNG's baseline
    exposure, `head`). Everything is drawn from `idx` alone."""
    from PIL import Image
    names = sorted(p.stem[5:] for p in (root / 'maps').glob('photo[0-9]*.npz'))
    rng = np.random.default_rng(900000 + idx)
    load = lambda k: np.load(root / 'maps' / f'photo{k}.npz')['logs'].astype(np.float64)
    resize = lambda a, w, h: np.asarray(Image.fromarray(a.astype(np.float32), 'F').resize((w, h), Image.BICUBIC),
                                        dtype=np.float64)
    a = names[rng.integers(len(names))]
    l = load(a)
    l = l[:l.shape[0] // 2 * 2, :l.shape[1] // 2 * 2]
    par = dict(idx=idx, a=a, b=None)
    if rng.random() < 0.5:
        b = names[rng.integers(len(names))]
        lb = load(b)
        if (lb.shape[0] > lb.shape[1]) != (l.shape[0] > l.shape[1]):
            lb = np.rot90(lb)
        lb = resize(lb, l.shape[1], l.shape[0])
        off = rng.uniform(-3, 3)
        lb = lb - np.log2(np.exp2(lb).mean()) + np.log2(np.exp2(l).mean()) + off
        frac = rng.uniform(0.2, 0.8)
        yy, xx = np.mgrid[0:l.shape[0], 0:l.shape[1]]
        ang = rng.uniform(0, np.pi)
        proj = np.cos(ang) * xx / l.shape[1] + np.sin(ang) * yy / l.shape[0]
        l = np.where(proj < np.quantile(proj, frac), l, lb)
        par.update(b=b, off=float(off), frac=float(frac), ang=float(ang))
    key = np.log2(np.exp2(l).mean())
    knots = np.arange(-14, 10.01, 1.)
    slopes = np.exp2(rng.uniform(-1.0, 0.8, len(knots) - 1))
    f = np.r_[0, np.cumsum(slopes)]
    f -= np.interp(0, knots, f)
    ev = rng.uniform(-1.5, 1.0)
    out = key + np.interp(l - key, knots, f) + ev
    head = float(rng.choice([0.65, 1., 1.5, 2., 2.]))
    noise = float(rng.choice([0., 0.01]))
    big = resize(np.exp2(out), l.shape[1] * 2, l.shape[0] * 2)
    if noise:
        big = big * (1 + noise * rng.normal(0, 1, big.shape))
    big = np.clip(big, FLOOR, 2. ** head)
    par.update(ev=float(ev), head=head, noise=noise, slopes=[float(v) for v in slopes])
    return np.repeat(big[..., None], 3, -1), big.reshape(l.shape[0], 2, l.shape[1], 2).mean((1, 3)), par


GAP_SIZE = (1024, 680)
GAP_RADII = (2, 4, 8, 16, 32, 64)
GAP_SHARES = (0.0003, 0.0015, 0.006, 0.025, 0.10)


def gap_plan():
    """The gap probes' parameters: a field with round spots of radius `r` pixels on a
    jittered lattice, covering `share` of a 1024 × 680 frame. `bright`: a field 6.3
    stops under white with spots at `level` (0.7 over white, 1.3 or 3.3 under);
    `dark`: a field a stop under white with spots 6 stops under. The field is the slow
    sinusoid of ±0.7 stop of tests/color/scene_probes.rs's dark scene (`smooth`), or
    that with 0.15-stop blobs and 1% noise (`tex`)."""
    w, h = GAP_SIZE
    count = lambda r, share: int(round(share * w * h / (np.pi * r * r)))
    plan = []
    for field in ('smooth', 'tex'):
        plan.append(dict(kind='bright', field=field, r=0, share=0., level=0.7))
        for level in (0.7, -1.3):
            plan += [dict(kind='bright', field=field, r=r, share=share, level=level)
                     for r in GAP_RADII for share in GAP_SHARES if count(r, share) >= 1]
        plan += [dict(kind='bright', field=field, r=r, share=share, level=-3.3) for r in (4, 32) for share in (0.006, 0.10)]
        plan.append(dict(kind='dark', field=field, r=0, share=0., level=-6.))
        plan += [dict(kind='dark', field=field, r=r, share=share, level=-6.)
                 for r in GAP_RADII for share in (0.0015, 0.025) if count(r, share) >= 1]
    return plan


def gap_scene(idx):
    """Gap probe `idx` of `gap_plan`: its luminance (linear scene values, a stop of
    headroom over white) and the spots' mask."""
    from scipy.ndimage import gaussian_filter
    w, h = GAP_SIZE
    p = gap_plan()[idx]
    rng = np.random.default_rng(570000 + idx)
    yy, xx = np.mgrid[0:h, 0:w].astype(np.float64)
    f = (-6.3 if p['kind'] == 'bright' else -1.0) + 0.7 * np.sin(xx / 640 * 2 * np.pi) * np.sin(yy / 640 * 2 * np.pi)
    if p['field'] == 'tex':
        blobs = gaussian_filter(rng.normal(0, 1, (h, w)), 6)
        f = f + 0.15 * blobs / blobs.std()
    cover = np.zeros((h, w))
    if p['r']:
        r = p['r']
        n = int(round(p['share'] * w * h / (np.pi * r ** 2)))
        pitch = np.sqrt(w * h / n)
        nx, ny = max(1, int(round(w / pitch))), max(1, int(round(h / pitch)))
        cells = [(a, b) for b in range(ny) for a in range(nx)]
        if len(cells) >= n:
            cells = [cells[k] for k in rng.permutation(len(cells))[:n]]
        free = max(0., min(w / nx, h / ny) / 2 - r - 1)
        for a, b in cells:
            cx = (a + 0.5) * w / nx + rng.uniform(-1, 1) * min(free, 0.3 * w / nx)
            cy = (b + 0.5) * h / ny + rng.uniform(-1, 1) * min(free, 0.3 * h / ny)
            x0, x1, y0, y1 = int(max(0, cx - r - 2)), int(min(w, cx + r + 3)), int(max(0, cy - r - 2)), int(min(h, cy + r + 3))
            d = np.hypot(xx[y0:y1, x0:x1] - cx, yy[y0:y1, x0:x1] - cy)
            cover[y0:y1, x0:x1] = np.maximum(cover[y0:y1, x0:x1], np.clip(r + 0.5 - d, 0, 1))
    lin = np.exp2(f) * (1 - cover) + np.exp2(p['level']) * cover
    if p['field'] == 'tex':
        lin = lin * (1 + 0.01 * rng.normal(0, 1, lin.shape))
    return np.clip(lin, FLOOR, 2.), cover > 0.5


def capture(folder, todo, make, digits):
    """Camera Raw's renders (default and the four full sliders) of the scenes `todo`
    as `folder`/NNN.npz: `make(i)` gives a scene's image, the arrays to keep with it,
    its headroom and the reduction its renders are kept at. Ten scenes a batch."""
    folder.mkdir(parents=True, exist_ok=True)
    name = lambda i: f'{i:0{digits}d}'
    for start in range(0, len(todo), 10):
        chunk, jobs, kept = todo[start:start + 10], [], {}
        for i in chunk:
            image, keep, head, factor = make(i)
            kept[i] = (image.shape[:2], keep, factor)
            source = folder / f'{name(i)}.dng'
            probe_dng.write(source, image, baseline_exposure=head, black_render=1)
            jobs += [dict(source=str(source), settings=st, out=str(folder / f'{name(i)}-{n}.tif')) for n, st in SETTINGS.items()]
        failures = camera_raw.render(jobs, work=folder / 'work')
        if failures:
            print('failed', failures[:3])
        for i in chunk:
            shape, keep, factor = kept[i]
            tifs = {n: folder / f'{name(i)}-{n}.tif' for n in SETTINGS}
            if all(t.exists() for t in tifs.values()):
                renders = {n: np.maximum(camera_raw.read_linear(t) @ YW, 0) for n, t in tifs.items()}
                if all(r.shape == shape for r in renders.values()):
                    small = lambda a: a.reshape(shape[0] // factor, factor, shape[1] // factor, factor).mean((1, 3))
                    kind = np.float16 if factor > 1 else np.float32
                    np.savez(folder / f'{name(i)}.npz', **keep, **{n: small(r).astype(kind) for n, r in renders.items()})
                else:
                    print(name(i), 'rendered at another size')
            for t in tifs.values():
                t.unlink(missing_ok=True)
            (folder / f'{name(i)}.dng').unlink(missing_ok=True)
        print(f'{min(start + 10, len(todo))} of {len(todo)}', flush=True)


def scenes(args):
    """ROOT/scenes/NNNNN.npz: each scene's luminance at the map's size (`lum`), its
    parameters (`par`, JSON) and Camera Raw's renders at that size (luminance)."""
    folder = args.root / 'scenes'
    def make(i):
        image, lum, par = scene(args.root, i)
        return image, dict(lum=lum.astype(np.float32), par=json.dumps(par)), par['head'], 2
    capture(folder, [i for i in range(args.count) if not (folder / f'{i:05d}.npz').exists()], make, 5)


def gaps(args):
    """ROOT/gaps/NNN.npz: each gap probe's luminance (`lum`), its spots (`mask`), its
    parameters (`par`, JSON) and Camera Raw's renders at full size (luminance)."""
    folder = args.root / 'gaps'
    def make(i):
        lum, mask = gap_scene(i)
        keep = dict(lum=lum.astype(np.float32), mask=mask, par=json.dumps(dict(gap_plan()[i], idx=i)))
        return np.repeat(lum[..., None], 3, -1), keep, 1., 1
    capture(folder, [i for i in range(len(gap_plan())) if not (folder / f'{i:03d}.npz').exists()], make, 3)


def scene_sets(root):
    """The captured scenes and gap probes: (map name, renders, the DNG's image and
    headroom when asked for, sample seed's offset)."""
    sets = [(f's{p.stem}', p, lambda i=int(p.stem): (scene(root, i)[0], scene(root, i)[2]['head']), int(p.stem))
            for p in sorted((root / 'scenes').glob('[0-9]*.npz'))]
    sets += [(f'g{p.stem}', p, lambda i=int(p.stem): (np.repeat(gap_scene(i)[0][..., None], 3, -1), 1.), 500000 + int(p.stem))
             for p in sorted((root / 'gaps').glob('[0-9]*.npz'))]
    return sets


def engine_map(rawmakase, dng, out, settings):
    """The engine's own measurement map of `dng` (log luminance and base level, 512 px)
    and its white curve there (scene value against output, from 2^-6 to 2^5, so also
    above the photo's top), dumped under RAWMAKASE_RULE_DUMP by `local_tone_rule.rs`
    and `pipeline/tone.rs`, as `out` (npz). The scene tone stage applies the rule's
    gain before that curve, which is fitted per photo and compresses everything near
    the photo's top. A map from before the engine wrote the curve is made again."""
    if out.exists() and {'white', 'measures'} <= set(np.load(out).files):
        return
    tmp = Path(f'/tmp/rule-map-{os.getpid()}')
    for p in (tmp, tmp.with_suffix('.map'), tmp.with_suffix('.white'), tmp.with_suffix('.measures')):
        p.unlink(missing_ok=True)
    tmp.with_suffix('.xmp').write_text(camera_raw.xmp({**settings, 'Shadows2012': '100'}))
    env = dict(os.environ, RAWMAKASE_RULE_DUMP=str(tmp))
    subprocess.run([str(rawmakase), 'render', str(dng), str(tmp.with_suffix('.tif')), '--tap', 'scene-input',
                    '--overwrite', '--xmp', str(tmp.with_suffix('.xmp'))], env=env, capture_output=True)
    raw = np.fromfile(tmp.with_suffix('.map'), dtype=np.float32)
    w, h = int(raw[0]), int(raw[1])
    n = w * h
    white = np.fromfile(tmp.with_suffix('.white'), dtype=np.float32).reshape(-1, 2).T
    extra = {}
    if tmp.with_suffix('.measures').exists():
        extra['measures'] = np.fromfile(tmp.with_suffix('.measures'), dtype=np.float32)
    np.savez(out, logs=raw[2:2 + n].reshape(h, w), base=raw[2 + n:2 + 2 * n].reshape(h, w), white=white, **extra)


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
    if any('pix_table' not in line for line in lines):
        raise SystemExit(f'{dng}: the dump is missing pix_table; rebuild rawmakase with G_pix dump support')
    np.savez(out, tables=np.array([line['table'] for line in lines], np.float32),
             lo=np.float32(lines[0]['lo']), hi=np.float32(lines[0]['hi']),
             pix_tables=np.array([line['pix_table'] for line in lines], np.float32),
             pix_lo=np.float32(lines[0]['pix_lo']), pix_hi=np.float32(lines[0]['pix_hi']))


def taps(args):
    """The engine's measurement map and white curve of every probe, photo, scene and gap
    probe, and each photo's four rule curves."""
    out = args.root / 'maps'
    out.mkdir(parents=True, exist_ok=True)
    (args.root / 'curves').mkdir(exist_ok=True)
    for name, _, _, _ in probe_jobs(args.root, args.histograms, images=False):
        engine_map(args.rawmakase, args.root / 'probes' / f'{name}.dng', out / f'{name}.npz', {})
    for dng in sorted((args.root / 'photos' / 'dng').glob('*.dng')):
        settings = {'CameraProfile': linear_profile.NAME}
        engine_map(args.rawmakase, dng, out / f'photo{dng.stem}.npz', settings)
        engine_curves(args.rawmakase, dng, args.root / 'curves' / f'photo{dng.stem}.npz', settings)
    # A scene's DNG is not kept: it is written again from the scene's number.
    dng = Path(f'/tmp/rule-scene-{os.getpid()}.dng')
    for name, _, image, _ in scene_sets(args.root):
        if not (out / f'{name}.npz').exists():
            pixels, head = image()
            probe_dng.write(dng, pixels, baseline_exposure=head, black_render=1)
            engine_map(args.rawmakase, dng, out / f'{name}.npz', {})
    dng.unlink(missing_ok=True)


# --- training ------------------------------------------------------------------------

def cr_rgb(path):
    """Linear ProPhoto RGB (H, W, 3) or luminance (H, W) of a Camera Raw render."""
    npy = Path(path).with_suffix('.npy')
    if npy.exists():
        return np.maximum(np.load(npy).astype(np.float64), 0)
    import tifffile
    return (tifffile.imread(str(path)).astype(np.float64)[..., :3] / 65535.) ** 1.8


def cr_linear(path, peak=False):
    """Luminance of a Camera Raw render, or its brightest channel (`peak`): the TIFF, or
    the `.npy` kept in its place (a luminance-only one gives luminance for both)."""
    a = cr_rgb(path)
    if a.ndim == 2:
        return a
    return a.max(-1) if peak else a @ YW


def on_map_rgb(ph, image):
    """`image` (2D luminance or 3D RGB) at the size of `ph`'s map."""
    if image.ndim == 2:
        return on_map(ph, image)
    return np.stack([on_map(ph, image[..., c]) for c in range(image.shape[2])], -1)


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


# The default black in a photo's targets. The stage applies the rule's gain and then the
# default black (the None → Auto transfer at the photo's dark key, which Shadows moves),
# as Camera Raw does: on the dark scene with gaps (tests/color/scene_probes.rs) the stage
# lifts 0.59, 0.76 and 0.55 EV at Exposure −2, 0 and +2 against Camera Raw's 0.60, 0.79
# and 0.57. A photo's renders carry that black, so the ratio of two of them is not the
# gain in the deep tones: it read 0.79 EV where the gain was 0.52, and the rule lifted
# the deepest tones of photos 0.6 stop too far at Shadows +100. Each render of a photo
# is therefore taken back through the stage's own black before the gain is read. The
# mosaics are rendered without the black, and their ratios are gains as they are.

BLACK = ROOT / 'crates/rawmakase-engine/src/develop/scene_tone/default_black.bin'
WHITE_BIN = ROOT / 'crates/rawmakase-engine/src/develop/scene_tone/white.bin'
WHITE3_BIN = ROOT / 'crates/rawmakase-engine/src/develop/scene_tone/white3.bin'
BLACK_KEYS = -14 + 0.5 * np.arange(15)      # the table's dark keys (`DEFAULT_BLACK_KEYS`)
BLACK_LEVELS = -16 + np.arange(129) / 8.    # log2 of the table's inputs
WHITE_DUMP_X = np.exp2(np.arange(-96, 81) / 16.)  # 2^-6 .. 2^5 at 1/16 stops (`pipeline/tone.rs`)
# The settings under which the stage moves the black's key: True by positive Shadows'
# rule, False by negative's (`key_shift`).
KEY_MOVES = {'S+100': True, 'S-100': False, 'S-50': False}
_TABLES = {}


def _load_tables():
    if not _TABLES:
        _TABLES['black'] = np.fromfile(BLACK, dtype='<f4').reshape(len(BLACK_KEYS), len(BLACK_LEVELS)).astype(np.float64)
        _TABLES['white'] = np.fromfile(WHITE_BIN, dtype='<f4').reshape(25, 17, 177).astype(np.float64)
        _TABLES['white3'] = np.fromfile(WHITE3_BIN, dtype='<u2').reshape(15, 11, 17, 113).astype(np.float64) / 65535.0
    return _TABLES


def _eval_log_lut(samples, first, step, x):
    x = np.asarray(x, np.float64)
    low = 2.0 ** first
    n = len(samples)
    f = (np.log2(np.maximum(x, 1e-30)) - first) / step
    fc = np.clip(f, 0.0, n - 1)
    i = np.minimum(fc.astype(int), n - 2)
    t = fc - i
    return np.where(x <= low, x * samples[0] / low, np.where(f >= n - 1, samples[-1], samples[i] * (1.0 - t) + samples[i + 1] * t))


def white_curve_for(sensor, top, black_point=True):
    """Exact replica of `GlobalTone::new` (Whites = 0, SDR) white curve on `WHITE_DUMP_X`:
    returns `(x, y, cal_key)` where `cal_key` is `top_clamped - 14.` (the default-black
    calibration key already baked into `white3.bin`)."""
    t = _load_tables()
    white, white3 = t['white'], t['white3']
    sensor = float(np.clip(sensor, -3.0, 4.0))
    top = sensor - float(np.clip(sensor - top, 0.0, 5.0))
    wp = max(sensor, -1.0) if sensor < 0.0 else min(sensor, max(top + 1.0, 0.0))
    fw = float(np.clip((wp - (-1.0)) / 0.25, 0.0, 24.0))
    wi = min(int(fw), 23)
    tw = fw - wi
    tail = white[wi, 8] * (1.0 - tw) + white[wi + 1, 8] * tw
    fi = float(np.clip((sensor - (-3.0)) / 0.5, 0.0, 14.0))
    ii = min(int(fi), 13)
    ti = fi - ii
    fj = float(np.clip((sensor - top) / 0.5, 0.0, 10.0))
    jj = min(int(fj), 9)
    tj = fj - jj
    logs = top - 14.0 + 0.125 * np.arange(113)
    tail_logs = _eval_log_lut(tail, -16.0, 0.125, np.exp2(logs))

    def corner_curve(i, j):
        corner = -3.0 + i * 0.5 - j * 0.5
        u = (logs - corner + 14.0) / 0.125
        at_top = white3[i, j, 8, 112]
        rm = float(_eval_log_lut(tail, -16.0, 0.125, 2.0 ** corner))
        above = (at_top + (1.0 - at_top) * np.clip((tail_logs - rm) / (1.0 - rm), 0.0, 1.0)) if rm < 1.0 else np.full_like(logs, at_top)
        uc = np.clip(u, 0.0, 112.0)
        n = np.minimum(uc.astype(int), 111)
        frac = uc - n
        inside = white3[i, j, 8, n] * (1.0 - frac) + white3[i, j, 8, n + 1] * frac
        return np.where(u > 112.0, above, inside)

    row = ((corner_curve(ii, jj) * (1.0 - tj) + corner_curve(ii, jj + 1) * tj) * (1.0 - ti)
           + (corner_curve(ii + 1, jj) * (1.0 - tj) + corner_curve(ii + 1, jj + 1) * tj) * ti)
    toe = None if black_point else white[4, 8]
    linear_below = 0.0
    if toe is not None:
        xs_row = np.exp2(logs)
        row = row * np.minimum(xs_row, 1.0) / np.maximum(_eval_log_lut(toe, -16.0, 0.125, xs_row), 1e-9)
        idx = np.flatnonzero(toe >= 1.0 / 512.0)
        k0 = int(idx[0]) if idx.size else 176
        linear_below = 2.0 ** (-16.0 + k0 * 0.125)
    row = np.maximum.accumulate(row)
    top_lin = 2.0 ** top
    y_le = _eval_log_lut(row, top - 14.0, 0.125, WHITE_DUMP_X)
    at_top = float(row[-1])
    r_x = _eval_log_lut(tail, -16.0, 0.125, WHITE_DUMP_X)
    rm = float(_eval_log_lut(tail, -16.0, 0.125, top_lin))
    y_gt = (at_top + (1.0 - at_top) * np.clip((r_x - rm) / (1.0 - rm), 0.0, 1.0)) if (rm < 1.0 and at_top < 1.0) else np.full_like(WHITE_DUMP_X, at_top)
    y = np.where(WHITE_DUMP_X <= top_lin, y_le, y_gt)
    if toe is not None:
        y_lb = float(_eval_log_lut(row, top - 14.0, 0.125, linear_below))
        y = np.where((WHITE_DUMP_X > 0.0) & (WHITE_DUMP_X < linear_below), y_lb * WHITE_DUMP_X / linear_below, y)
    return WHITE_DUMP_X.copy(), y, top - 14.0


def infer_sensor_white(m, key=None, par=None, sensor_map=None):
    """log2 sensor white for a photo, scene, gap probe or mosaic."""
    if 'measures' in m.files:
        return float(m['measures'][0])
    if sensor_map and key in sensor_map:
        return float(sensor_map[key])
    if par is not None:
        return float(par['head']) if 'head' in par else 1.0
    if key:
        if key.startswith(('clipwb-', 'partial-')):
            return float(-np.log2(min(NEUTRAL['A'])))
        if key.startswith('clipwb2-'):
            k = int(key.split('-')[1])
            neutral = NEUTRAL['B'] if k % 3 == 1 else NEUTRAL['A']
            return float(-np.log2(min(neutral)))
        if not key.startswith('photo'):
            return 0.0
    raise ValueError(
        f'{key}: missing sensor_white (`measures` absent from map and key absent from sensor_whites.json); '
        're-run `taps` with a current build so maps include `.measures`'
    )


def black_row(key):
    """The default black's transfer at dark key `key`: its output at each of
    `BLACK_LEVELS`, between the table's rows and held past its ends, as
    `scene_tone::global` reads default_black.bin."""
    table = _load_tables()['black']
    f = (np.clip(key, BLACK_KEYS[0], BLACK_KEYS[-1]) - BLACK_KEYS[0]) / 0.5
    i = min(int(f), len(BLACK_KEYS) - 2)
    return table[i] * (1 - (f - i)) + table[i + 1] * (f - i)


def black_taken(row, v):
    """What the transfer `row` takes from scene values `v`: in proportion under the
    table's first input, and nothing from white up."""
    v = np.asarray(v, np.float64)
    low = 2. ** BLACK_LEVELS[0]
    out = np.where(v <= low, v * row[0] / low, np.interp(np.log2(np.maximum(v, low)), BLACK_LEVELS, row))
    return np.where(v < 1, np.maximum(v - out, 0), 0.)


def dark_key(m):
    """A photo's dark key from its map `m`: log2 of the 0.1th percentile of its
    luminance, no lower than the table's first key."""
    return float(np.log2(max(np.percentile(np.exp2(m['logs'].astype(np.float64)), 0.1), 2. ** BLACK_KEYS[0])))


def black_undone(m, shift=0.):
    """A photo's render value → that value before the default black at the photo's dark
    key moved by `shift` stops. Values the black took to nothing stay as they are."""
    x = np.exp2(np.arange(-16, 0.01, 1 / 32.))
    y = np.maximum(x - black_taken(black_row(dark_key(m) + shift), x), 0.)
    rising = np.r_[True, np.diff(y) > 0]
    x, y = x[rising], y[rising]
    return lambda a: np.where((a > y[0]) & (a < y[-1]), np.interp(a, y, x), a)


def stage_curve_from(x, y, m, shift=0., cal_key=None):
    """Stage curve (scene -> output) from a white curve `(x, y)` and a photo's default
    black at `dark_key(m) + shift`."""
    photo = black_row(dark_key(m) + shift)
    own = black_row(BLACK_KEYS[0] if cal_key is None else cal_key)
    deep = np.exp2(np.arange(-16, np.log2(x[0]) - 1e-9, 1 / 16.))
    xs = np.r_[deep, x]
    ys = np.maximum(np.r_[deep - black_taken(photo, deep), y - (black_taken(photo, x) - black_taken(own, x))], 0.)
    rising = np.r_[True, np.diff(ys) > 0]
    return xs[rising], ys[rising]


def stage_curve(m, shift=0., cal_key=None):
    """A photo's global curve through the stage, scene value → output, with the default
    black at the photo's dark key moved by `shift` stops. From 2^-6 up it is the dumped
    white curve, which carries the black of the white3 calibration key (`top - 14`,
    matching `GlobalTone::new`), less what the photo's black takes beyond that; below,
    where the white curve is straight, it is the black's transfer alone."""
    x, y = m['white'].astype(np.float64)
    if cal_key is None and 'measures' in m.files:
        cal_key = float(m['measures'][1]) - 14.0
    return stage_curve_from(x, y, m, shift, cal_key)


def invert_curve(x, y):
    rising = np.r_[True, np.diff(y) > 0]
    xr, yr = x[rising], y[rising]
    return lambda a: np.where(a > yr[0], np.interp(a, yr, xr), a), xr, yr


def to_scene(m, shift=0., cal_key=None):
    """A photo's render value → the scene value `stage_curve` renders as it."""
    x, y = stage_curve(m, shift, cal_key)
    return invert_curve(x, y)[0]


def _inv_rgb_tone(rgb, xr, yr):
    """Exact inverse of DNG RGBTone for a monotone white curve `(xr, yr)`."""
    if rgb.ndim == 2:
        return np.where(rgb > yr[0], np.interp(rgb, yr, xr), rgb)
    lo, hi = rgb.min(-1), rgb.max(-1)
    lo_in = np.where(lo > yr[0], np.interp(lo, yr, xr), lo)
    hi_in = np.where(hi > yr[0], np.interp(hi, yr, xr), hi)
    d = hi - lo
    return np.where(d[..., None] > 1e-9, lo_in[..., None] + (hi_in - lo_in)[..., None] * (rgb - lo[..., None]) / np.maximum(d[..., None], 1e-9), lo_in[..., None])


def post_h_white(sensor, img_on_map, black_point=True, m=None, shift=0., rounds=4):
    """Fixed-point post-Highlights white curve `(x, y, cal_key)` matching
    `CurveSet::for_image`'s 128 px post-Highlights maximum (`MAX_EDGE = 128`)."""
    top = float(sensor)
    for _ in range(rounds):
        xw, yw, cal = white_curve_for(sensor, top, black_point=black_point)
        xs, ys = stage_curve_from(xw, yw, m, shift, cal) if (black_point and m is not None) else (xw, yw)
        _, xr, yr = invert_curve(xs, ys)
        sc = _inv_rgb_tone(img_on_map, xr, yr)
        if sc.ndim == 3:
            mx = max(float(reduce(sc[..., c], 128).max()) for c in range(3))
        else:
            mx = float(reduce(sc, 128).max())
        top = min(float(sensor), float(np.log2(max(mx, 1e-6))))
    return white_curve_for(sensor, top, black_point=black_point)


def default_cal_key(m, sensor, default_rgb):
    """Calibration black key (`top_pre - 14`) baked into the default white curve, matching
    `GlobalTone::new`'s `black_correction(dark_key, top - 14.)`."""
    if 'measures' in m.files:
        return float(m['measures'][1]) - 14.0
    return post_h_white(sensor, default_rgb, black_point=True, m=m)[2]


def key_shift(m, ph, default, setting, positive=True, rounds=4, setting_white=None, default_cal=None):
    """The stops by which the stage moves the black's key under a Shadows setting
    (`pipeline/tone.rs`), from Camera Raw's renders at the map's size: the change in
    the 0.1th percentile of scene log-luminance (`q0.001(post) - q0.001(pre)`) for both
    positive and negative Shadows. Solved by fixed-point iteration from no shift."""
    del ph, positive
    d = np.log2(np.maximum(to_scene(m, 0., default_cal)(default), FLOOR)).ravel()
    q_pre = float(np.quantile(d, 0.001))
    shift = 0.
    for _ in range(rounds):
        if setting_white is not None:
            xw, yw, cal = setting_white
            xs, ys = stage_curve_from(xw, yw, m, shift, cal)
            inv = invert_curve(xs, ys)[0]
        else:
            inv = to_scene(m, shift, default_cal)
        s = np.log2(np.maximum(inv(setting), FLOOR)).ravel()
        shift = float(np.quantile(s, 0.001) - q_pre)
    return shift


def samples(key, root, images, rng, n=2500, photo=False, peaks=None, rgbs=None, par=None, sensor_map=None):
    """The first ensemble's inputs (`Xb` at `base`, `Xl` at `logs`, both `FEAT_EXT`),
    targets (S+100, H−100) with the engine's white curve undone (and post-Highlights W*
    for H−100), and validity mask `C` (0 valid, 2 ignored) at `n` map pixels of one
    mosaic, scene or photo."""
    m = np.load(root / 'maps' / f'{key}.npz')
    ph = Photo(logs=m['logs'], base=m['base'])
    if peaks is None:
        peaks = images
    to_align = list(images) + list(peaks) + ([rgbs[0], rgbs[2]] if rgbs is not None else [])
    al = aligned(ph, to_align, coarse=photo)
    if al is None:
        print('misaligned', key)
        return None
    count = len(images)
    imgs, pks = [on_map(ph, a) for a in al[:count]], [on_map(ph, a) for a in al[count:2 * count]]
    def_rgb = on_map_rgb(ph, al[2 * count]) if rgbs is not None else imgs[0]
    h_rgb = on_map_rgb(ph, al[2 * count + 1]) if rgbs is not None else imgs[2]
    x, y = m['white'].astype(np.float64)
    white_undone, _, _ = invert_curve(x, y)
    sensor = infer_sensor_white(m, key, par=par, sensor_map=sensor_map)
    xh, yh, cal_h = post_h_white(sensor, h_rgb, black_point=photo, m=m if photo else None)
    if photo:
        cal_def = default_cal_key(m, sensor, def_rgb)
        moved = key_shift(m, ph, imgs[0], imgs[1], True, default_cal=cal_def)
        ld = np.log2(np.maximum(to_scene(m, 0., cal_def)(imgs[0]), FLOOR))
        ls = np.log2(np.maximum(to_scene(m, moved, cal_def)(imgs[1]), FLOOR))
        h_inv = invert_curve(*stage_curve_from(xh, yh, m, 0., cal_h))[0]
        lh = np.log2(np.maximum(h_inv(imgs[2]), FLOOR))
    else:
        ld = np.log2(np.maximum(white_undone(imgs[0]), FLOOR))
        ls = np.log2(np.maximum(white_undone(imgs[1]), FLOOR))
        h_inv = invert_curve(xh, yh)[0]
        lh = np.log2(np.maximum(h_inv(imgs[2]), FLOOR))
    vs = (imgs[0] > CLIP_LOW) & (pks[0] < WHITE) & (pks[1] < CLIP_HIGH)
    vh = (imgs[0] > CLIP_LOW) & (pks[0] < WHITE) & (pks[2] < CLIP_HIGH)
    idx = np.flatnonzero((vs | vh).ravel())
    if idx.size < 100:
        return None
    idx = rng.choice(idx, min(n, idx.size), replace=False)
    Y = np.stack([(ls - ld).ravel()[idx], (lh - ld).ravel()[idx]], 1)
    C = np.stack([np.where(vs.ravel()[idx], 0, 2), np.where(vh.ravel()[idx], 0, 2)], 1).astype(np.int8)
    return dict(key=key, X=ph.features_first(ph.base.ravel()[idx]), Xl=ph.features_first(ph.logs.ravel()[idx]), Y=Y, C=C)


def samples_ext(key, root, images, halves, peaks, rng, n=2500, photo=False, rgbs=None, par=None, sensor_map=None):
    """The second ensemble's inputs (`FEAT_EXT`), targets (`OUTPUTS`) and their censoring
    (`C`: 0 an exact target, +1 a lower bound, −1 an upper bound) at `n` map pixels of one
    mosaic or photo. `images`: Camera Raw's luminance at default and at each of
    `OUTPUTS`; `halves`: at each of `HALVES`, or none; `peaks`: the brightest channel of
    each of the two lists' renders, in their order.

    Highlights renders (`H−100`, `H+100`, `H+50`) invert through their own post-Highlights
    white curve (`post_h_white`), matching `CurveSet::for_image`."""
    m = np.load(root / 'maps' / f'{key}.npz')
    ph = Photo(logs=m['logs'], base=m['base'])
    count, total = len(images), len(images) + len(halves)
    names = list(SETTINGS) + ([half for half, _ in HALVES.values()] if halves else [])
    h_idx = [k for k, nm in enumerate(names) if nm.startswith('H')]
    rgb_idx = ([0] if photo else []) + h_idx
    h_rgbs = [rgbs[k] for k in rgb_idx] if rgbs is not None else []
    to_align = images + halves + peaks + h_rgbs
    al = aligned(ph, to_align, coarse=True)
    if al is None:
        print('misaligned', key)
        return None
    images, peaks = al[:total], al[total:2 * total]
    rgb_aligned = {k: on_map_rgb(ph, a) for k, a in zip(rgb_idx, al[2 * total:])}
    # Default white curve up to where it reaches white.
    x, y = m['white'].astype(np.float64)
    white_undone, xr_def, _ = invert_curve(x, y)
    in_stops = lambda a: np.log2(np.maximum(on_map(ph, a), FLOOR))
    on = [on_map(ph, a) for a in images]
    sensor = infer_sensor_white(m, key, par=par, sensor_map=sensor_map)
    post_curves = {}
    for k in h_idx:
        h_src = rgb_aligned.get(k, on[k])
        post_curves[k] = post_h_white(sensor, h_src, black_point=photo, m=m if photo else None)
    if photo:
        cal_def = default_cal_key(m, sensor, rgb_aligned.get(0, on[0]))
        moved = [key_shift(m, ph, on[0], on[k], KEY_MOVES[names[k]], default_cal=cal_def) if names[k] in KEY_MOVES else 0. for k in range(total)]
        def level(k):
            if k in post_curves:
                xw, yw, cal = post_curves[k]
                inv = invert_curve(*stage_curve_from(xw, yw, m, moved[k], cal))[0]
                return in_stops(inv(images[k]))
            return in_stops(to_scene(m, moved[k], cal_def)(images[k]))
    else:
        def level(k):
            if k in post_curves:
                xw, yw, _ = post_curves[k]
                return in_stops(invert_curve(xw, yw)[0](images[k]))
            return in_stops(white_undone(images[k]))
    default = level(0)
    valid = (on[0] > CLIP_LOW) & (on_map(ph, peaks[0]) < WHITE)
    idx = np.flatnonzero(valid.ravel())
    if idx.size < 200:
        return None
    idx = rng.choice(idx, min(n, idx.size), replace=False)
    gain = lambda k: (level(k) - default).ravel()[idx]
    at = lambda group, k: on_map(ph, group[k]).ravel()[idx]
    def white_top(k):
        if k in post_curves:
            xw, yw, _ = post_curves[k]
            _, xr, _ = invert_curve(xw, yw)
            return float(xr[-1])
        return float(xr_def[-1])
    base_peak = in_stops(white_undone(peaks[0])).ravel()[idx]
    to_white_for = lambda k: np.log2(white_top(k)) + CLIP_MARGIN - base_peak
    Y = np.stack([gain(k) for k in range(1, count)], 1)
    C = np.zeros(Y.shape, np.int8)
    for output, sign in (('H+100', 1), ('S-100', -1)):
        clipped = (lambda k: at(peaks, k) >= CLIP_HIGH) if sign > 0 else (lambda k: at(images, k) <= CLIP_LOW)
        j = OUTPUTS.index(output)
        bound = clipped(1 + j)
        if sign > 0:
            Y[bound, j] = to_white_for(1 + j)[bound]
        if halves:
            h = count + list(HALVES).index(output)
            twice = 2 * gain(h)
            exact = bound & ~clipped(h)
            Y[exact, j] = twice[exact]
            bound &= ~exact
            Y[bound, j] = 2 * to_white_for(h)[bound] if sign > 0 else np.minimum(Y[bound, j], twice[bound])
        C[bound, j] = sign
    return dict(key=key, X=ph.features_ext(ph.base.ravel()[idx]), Xl=ph.features_ext(ph.logs.ravel()[idx]), Y=Y, C=C)


def sample_job(job):
    kind, key, root, renders, halves, ext, seed, sensor_map = job
    rng = np.random.default_rng(seed)
    if kind == 'scene':
        # A scene's renders show its gain as a mosaic's do (no default black, grey), so
        # its brightest channel is its luminance. Kept as float32: there are thousands.
        z = np.load(renders[0])
        images = [z[n].astype(np.float64) for n in SETTINGS]
        par = json.loads(str(z['par']))
        if ext:
            smp = samples_ext(key, root, images, [], list(images), rng, n=SCENE_SAMPLES, par=par)
            if smp:
                # Scenes have no half-slider renders, so clipped pixels would enter as
                # unbounded one-sided bounds (with CLIP_MARGIN) and bias H+100/S-100
                # clipping; keep only exact scene samples.
                exact = (smp['C'][:, OUTPUTS.index('S-100')] == 0) & (smp['C'][:, OUTPUTS.index('H+100')] == 0)
                if not exact.any():
                    return None
                smp = {k: (v[exact] if k in ('X', 'Xl', 'Y', 'C') else v) for k, v in smp.items()}
        else:
            smp = samples(key, root, images[:3], rng, n=SCENE_SAMPLES, par=par)
        if not smp:
            return None
        made_of = {f'photo{par[k]}' for k in 'ab' if par.get(k)}
        return dict({k: v.astype(np.float32) if k in ('X', 'Xl', 'Y') else v for k, v in smp.items()}, kind=kind, made_of=made_of)
    all_rgbs = [cr_rgb(r) for r in renders + halves]
    to_lum = lambda a: a if a.ndim == 2 else a @ YW
    to_pk = lambda a: a if a.ndim == 2 else a.max(-1)
    r_lum = [to_lum(a) for a in all_rgbs[:len(renders)]]
    h_lum = [to_lum(a) for a in all_rgbs[len(renders):]]
    all_pk = [to_pk(a) for a in all_rgbs]
    if ext:
        smp = samples_ext(key, root, r_lum, h_lum, all_pk, rng, photo=kind == 'photo',
                          rgbs=all_rgbs, sensor_map=sensor_map)
    else:
        smp = samples(key, root, r_lum, rng, photo=kind == 'photo', peaks=all_pk,
                      rgbs=all_rgbs, sensor_map=sensor_map)
    return dict(smp, kind=kind) if smp else None


def load_sensor_map(root):
    p = root / 'sensor_whites.json'
    return json.loads(p.read_text()) if p.exists() else {}


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
    sensor_map = load_sensor_map(root)
    jobs = []
    d = root / 'probes'
    for name, _, _, _ in probe_jobs(root, photos_dir, images=False):
        if all(rendered(d / f'{name}-{s}.tif') for s in settings) and (root / 'maps' / f'{name}.npz').exists():
            jobs.append(('mosaic', name, root, [d / f'{name}-{s}.tif' for s in settings], [], ext, len(jobs), None))
    for npz in sorted((root / 'maps').glob('photo[0-9]*.npz')):
        i = npz.stem[5:]
        cr = root / 'photos' / 'crlin'
        if not all(rendered(cr / f'{i}-{s}.tif') for s in settings):
            continue
        renders, halves = [cr / f'{i}-{s}.tif' for s in settings], []
        if ext:
            halves = [cr / f'{i}-{s}.tif' for s, _ in HALVES.values()]
            halves = halves if all(rendered(h) for h in halves) else []
        jobs.append(('photo', f'photo{i}', root, renders, halves, ext, len(jobs), sensor_map))
    for name, renders, _, offset in scene_sets(root):
        if (root / 'maps' / f'{name}.npz').exists():
            jobs.append(('scene', name, root, [renders], [], ext, (80000 if ext else 70000) + offset, None))
    sensor_digest = hashlib.sha256(
        json.dumps(sorted(sensor_map.items()), separators=(',', ':')).encode()
    ).hexdigest()
    made_from = dict(
        features=FEAT_EXT if ext else FEAT,
        black=[hashlib.sha256(BLACK.read_bytes()).hexdigest(), sorted(KEY_MOVES.items())],
        white=[hashlib.sha256(WHITE_BIN.read_bytes()).hexdigest(), hashlib.sha256(WHITE3_BIN.read_bytes()).hexdigest()],
        constants=dict(BOTTOM_GAP=BOTTOM_GAP, WHITE=WHITE, CLIP_HIGH=CLIP_HIGH, CLIP_LOW=CLIP_LOW,
                       CLIP_MARGIN=CLIP_MARGIN, STAT_BLUR=STAT_BLUR, SCENE_SAMPLES=SCENE_SAMPLES,
                       dual_pix=True, post_h_white=True, sensor_digest=sensor_digest),
        inputs=[[key, stamp(root / 'maps' / f'{key}.npz')] + [stamp(r) for r in renders + halves]
                for _, key, _, renders, halves, _, _, _ in jobs])
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


def fit(X, Xl, Y, C, mu, sd, steps=6000, seed=0, lr=2e-3, pix_reg=PIX_REG):
    """Additive dual-MLP `G_base(b) + G_pix(l)` from `(X, Xl)` to `Y`. Where `C` is 2
    the target is ignored; +1 a lower bound; −1 an upper bound."""
    import mlx.core as mx
    import mlx.nn as nn
    import mlx.optimizers as optim

    class DualMLP(nn.Module):
        def __init__(s, n, h=HIDDEN):
            super().__init__()
            s.b1, s.b2, s.b3 = nn.Linear(n, h), nn.Linear(h, h), nn.Linear(h, Y.shape[1])
            s.p1, s.p2, s.p3 = nn.Linear(n, h), nn.Linear(h, h), nn.Linear(h, Y.shape[1])

        def __call__(s, xb, xl):
            gb = s.b3(nn.silu(s.b2(nn.silu(s.b1(xb)))))
            gp = s.p3(nn.silu(s.p2(nn.silu(s.p1(xl)))))
            return gb, gp

    mx.random.seed(seed)
    m, opt = DualMLP(X.shape[1]), optim.Adam(lr)
    xb = mx.array(((X - mu) / sd).astype(np.float32))
    xl = mx.array(((Xl - mu) / sd).astype(np.float32))
    y, c = mx.array(Y.astype(np.float32)), mx.array(C.astype(np.float32))

    def loss(m, xb, xl, yb, cb):
        gb, gp = m(xb, xl)
        d = (gb + gp) - yb
        d = mx.where(cb == 2, 0., mx.where(cb > 0, mx.minimum(d, 0), mx.where(cb < 0, mx.maximum(d, 0), d)))
        return (d ** 2).mean() + pix_reg * (gp ** 2).mean()

    g = nn.value_and_grad(m, loss)
    rng = np.random.default_rng(seed)
    for _ in range(steps):
        b = mx.array(rng.integers(0, xb.shape[0], 8192))
        _, grads = g(m, xb[b], xl[b], y[b], c[b])
        opt.update(m, grads)
        mx.eval(m.parameters(), opt.state)

    def predict(Xbn, Xln):
        gb, gp = m(mx.array(((Xbn - mu) / sd).astype(np.float32)), mx.array(((Xln - mu) / sd).astype(np.float32)))
        return np.array(gb + gp)

    ext_w = lambda l1, l2, l3: {k: np.array(v).tolist() for k, v in [
        ('l1.w', l1.weight), ('l1.b', l1.bias), ('l2.w', l2.weight),
        ('l2.b', l2.bias), ('l3.w', l3.weight), ('l3.b', l3.bias)]}
    return predict, (ext_w(m.b1, m.b2, m.b3), ext_w(m.p1, m.p2, m.p3))


def report(predict, ds, label, outputs=OUTPUTS):
    """Per-set rms error of the exact targets. Bounds are left out, and so is a set for
    an output all of whose targets are bounds; the count scored is printed when sets
    were left out."""
    def rms(d):
        exact = d['C'] == 0
        n = exact.sum(0)
        return np.where(n > 0, np.sqrt(((predict(d['X'], d['Xl']) - d['Y']) ** 2 * exact).sum(0) / np.maximum(n, 1)), np.nan)
    r = np.array([rms(d) for d in ds])
    scored = lambda k: '' if np.isfinite(r[:, k]).all() else f' ({int(np.isfinite(r[:, k]).sum())} scored)'
    print(f'  {label} (n={len(ds)}): ' + ' | '.join(
        f'{o} rms median {np.nanmedian(r[:, k]):.3f} mean {np.nanmean(r[:, k]):.3f}{scored(k)}' for k, o in enumerate(outputs)))


def train(args):
    out = args.out or args.root / 'global_rule.json'
    # The second ensemble fits only EXT_OUTPUTS (S-100, H+100) so its hidden units
    # are not diluted across the first ensemble's outputs.
    ext_rows = [OUTPUTS.index(o) for o in EXT_OUTPUTS]
    ext_data = [dict(d, Y=d['Y'][:, ext_rows], C=d['C'][:, ext_rows])
                for d in load(args.root, True, args.histograms)]
    ensemble(ext_data, EXT_OUTPUTS, args.cv, out.with_name(out.stem + '_ext.json'), DATA_EXT, EXT_OUTPUTS, FEAT_EXT)
    if args.first:
        ensemble(load(args.root, False, args.histograms), OUTPUTS[:2], args.cv, out, DATA, OUTPUTS[:2], FEAT)


def ensemble(data, outputs, cv, json_path, rust_path, keep, feat_names=FEAT_EXT):
    """Fits MEMBERS dual networks to the samples' targets `outputs` and writes those in `keep`."""
    data = [dict(d, C=d.get('C', np.zeros(d['Y'].shape, np.int8))) for d in data]
    mos, pho = [d for d in data if d['kind'] == 'mosaic'], [d for d in data if d['kind'] == 'photo']
    sce = [d for d in data if d['kind'] == 'scene']
    stack = lambda ds: tuple(np.concatenate([d[k] for d in ds]) for k in ('X', 'Xl', 'Y', 'C'))
    Xall = np.concatenate([d['X'] for d in mos + pho + sce])
    mu, sd = Xall.mean(0), Xall.std(0) + 1e-6
    if cv:
        # A scene made from a held-out photo's map is left out with it.
        folds = np.array_split(np.random.default_rng(1).permutation(len(pho)), 4)
        for f in folds:
            held = [pho[i] for i in f]
            keys = {d['key'] for d in held}
            others = [pho[i] for i in range(len(pho)) if i not in f] * PHOTO_WEIGHT
            predict, _ = fit(*stack(mos + others + [d for d in sce if not d['made_of'] & keys]), mu, sd, steps=4000)
            report(predict, held, 'held-out photos', outputs)
    members = [fit(*stack(mos + pho * PHOTO_WEIGHT + sce), mu, sd, seed=seed) for seed in range(MEMBERS)]
    predict = lambda Xbn, Xln: np.mean([p(Xbn, Xln) for p, _ in members], 0)
    report(predict, pho, 'photos (in-sample, ensemble)', outputs)
    report(predict, mos, 'mosaics (in-sample, ensemble)', outputs)
    if sce:
        report(predict, sce, 'scenes and gap probes (in-sample, ensemble)', outputs)
    rows = [outputs.index(o) for o in keep]
    slice_w = lambda w: dict(w, **{'l3.w': np.array(w['l3.w'])[rows].tolist(), 'l3.b': np.array(w['l3.b'])[rows].tolist()})
    kept_base = [slice_w(wb) for _, (wb, _) in members]
    kept_pix = [slice_w(wp) for _, (_, wp) in members]
    rule = dict(features=feat_names, outputs=keep, mu=mu.tolist(), sd=sd.tolist(),
                members_base=kept_base, members_pix=kept_pix)
    json_path.write_text(json.dumps(rule))
    rust_path.write_text(rust(rule, rust_path == DATA_EXT))
    print(f'-> {rust_path.relative_to(ROOT)}')


def rust(g, ext=False):
    mb = g.get('members_base', g.get('members'))
    mp = g['members_pix']
    members, hidden, features = len(mb), len(mb[0]['l1.b']), len(g['mu'])
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
    about = ('//! The global rule of negative Shadows and positive Highlights (`G_base(b) + G_pix(l)`):\n'
             '//! dual ensembles of small networks from histogram, per-level texture and absolute-level\n'
             '//! descriptors to the log2 gain at Shadows −100 and Highlights +100, fitted from different\n'
             '//! seeds to Camera Raw 18.7 on textured flat-patch mosaics, linear-profile training photos\n'
             "//! and scenes made from those photos' maps, from the engine's own measurement maps\n" if ext else
             '//! The global Shadows/Highlights rule (`G_base(b) + G_pix(l)`): dual ensembles of small\n'
             '//! networks from exposure-invariant histogram and per-level texture descriptors to the\n'
             '//! log2 gain at Shadows +100 and Highlights −100, fitted from different seeds to Camera\n'
             '//! Raw 18.7 on textured flat-patch mosaics, linear-profile training photos and scenes\n'
             "//! made from those photos' maps, from the engine's own measurement maps\n")
    head = (about +
            '//! (scripts/corpus/scene-tone-rule.py). Generated; do not edit.\n'
            '#![allow(\n    clippy::excessive_precision,\n    clippy::approx_constant,\n    clippy::unreadable_literal\n)]\n'
            f'/// Networks in the ensemble.\npub(super) const MEMBERS: usize = {members};\n'
            f'/// Units in each hidden layer.\npub(super) const HIDDEN: usize = {hidden};\n'
            f'/// Inputs the networks read.\npub(super) const FEATURES: usize = {features};\n')
    stack = lambda ms, k: [m[k] for m in ms]
    return head + arr('MU', g['mu'], '[f32; FEATURES]') + arr('SD', g['sd'], '[f32; FEATURES]') + \
        arr('W1', stack(mb, 'l1.w'), '[[[f32; FEATURES]; HIDDEN]; MEMBERS]') + arr('B1', stack(mb, 'l1.b'), '[[f32; HIDDEN]; MEMBERS]') + \
        arr('W2', stack(mb, 'l2.w'), '[[[f32; HIDDEN]; HIDDEN]; MEMBERS]') + arr('B2', stack(mb, 'l2.b'), '[[f32; HIDDEN]; MEMBERS]') + \
        arr('W3', stack(mb, 'l3.w'), '[[[f32; HIDDEN]; 2]; MEMBERS]') + arr('B3', stack(mb, 'l3.b'), '[[f32; 2]; MEMBERS]') + \
        arr('PW1', stack(mp, 'l1.w'), '[[[f32; FEATURES]; HIDDEN]; MEMBERS]') + arr('PB1', stack(mp, 'l1.b'), '[[f32; HIDDEN]; MEMBERS]') + \
        arr('PW2', stack(mp, 'l2.w'), '[[[f32; HIDDEN]; HIDDEN]; MEMBERS]') + arr('PB2', stack(mp, 'l2.b'), '[[f32; HIDDEN]; MEMBERS]') + \
        arr('PW3', stack(mp, 'l3.w'), '[[[f32; HIDDEN]; 2]; MEMBERS]') + arr('PB3', stack(mp, 'l3.b'), '[[f32; 2]; MEMBERS]')


# --- Shadows and Highlights together -----------------------------------------------------

def curve_at(curves, k, base):
    """Curve `k` of a photo's `curves` (engine_curves) at base levels, as `Curve::eval` reads it."""
    table, lo, hi = curves['tables'][k].astype(np.float64), float(curves['lo']), float(curves['hi'])
    f = np.clip((np.asarray(base, np.float64) - lo) / (hi - lo) * 48 - 0.5, 0, 47)
    i = np.minimum(f.astype(int), 46)
    return table[i] + (table[i + 1] - table[i]) * (f - i)


def pair_sample(job):
    """One photo for `pair`: per pair of `PAIRS`, at up to 6000 map pixels where none of
    that pair's renders (default, single Shadows, single Highlights, pair) is clipped,
    what blending each curve toward its extreme and adding a uniform shift adds per unit
    of its coefficients (`X`: Shadows' blend, Highlights' blend, uniform shift), what
    the sum of the engine's two curves is off Camera Raw's combined gain (`E`, the
    engine's white curve and pair's own `dark_key` undone), Camera Raw's own difference
    from the sum of its single gains (`I`) and each curve's own error at its single
    setting (`S`); and the 14 photo inputs the engine predicts the coefficients from
    (`F`, `RuleStats::pair`)."""
    key, root, sensor_map = job
    cr = root / 'photos' / 'crlin'
    m, curves = np.load(root / 'maps' / f'{key}.npz'), np.load(root / 'curves' / f'{key}.npz')
    ph = Photo(logs=m['logs'], base=m['base'])
    names = list(SETTINGS) + list(PAIRS)
    rgbs_raw = [cr_rgb(cr / f'{key[5:]}-{n}.tif') for n in names]
    lum = [a if a.ndim == 2 else a @ YW for a in rgbs_raw]
    peak = [a if a.ndim == 2 else a.max(-1) for a in rgbs_raw]
    h_idx = [k for k, n in enumerate(names) if ('H+' in n) or ('H-' in n)]
    rgb_idx = [0] + h_idx
    images = aligned(ph, lum + peak + [rgbs_raw[k] for k in rgb_idx], coarse=True)
    if images is None:
        print('misaligned', key)
        return None
    nn = len(names)
    lum, peak, sel_rgbs = images[:nn], images[nn:2 * nn], images[2 * nn:]
    on = [on_map(ph, a) for a in lum]
    pk = [on_map(ph, a) for a in peak]
    rgb_map = {k: on_map_rgb(ph, a) for k, a in zip(rgb_idx, sel_rgbs)}
    sensor = infer_sensor_white(m, key, sensor_map=sensor_map)
    cal_def = default_cal_key(m, sensor, rgb_map[0])
    # Every Highlights setting (single and pair) uses its own post-Highlights white curve,
    # and every Shadows setting (single and pair) moves default_black by its own q0.001 shift.
    post_w, moved = {}, {}
    for idx_n, n in enumerate(names[1:], 1):
        has_s = n.startswith('S+') or n.startswith('S-')
        has_h = idx_n in h_idx
        if has_h and has_s:
            sh = 0.
            pw = None
            for _ in range(2):
                pw = post_h_white(sensor, rgb_map[idx_n], black_point=True, m=m, shift=sh)
                sh = key_shift(m, ph, on[0], on[idx_n], n.startswith('S+'), setting_white=pw, default_cal=cal_def)
            post_w[n], moved[n] = pw, sh
        elif has_h:
            post_w[n] = post_h_white(sensor, rgb_map[idx_n], black_point=True, m=m)
        elif has_s:
            moved[n] = key_shift(m, ph, on[0], on[idx_n], n.startswith('S+'), default_cal=cal_def)

    def level(n, a):
        if n in post_w:
            xw, yw, cal = post_w[n]
            inv = invert_curve(*stage_curve_from(xw, yw, m, moved.get(n, 0.), cal))[0]
        else:
            inv = to_scene(m, moved.get(n, 0.), cal_def)
        return np.log2(np.maximum(on_map(ph, inv(a)), FLOOR))

    default = level(names[0], lum[0])
    gain_maps = {n: level(n, a) - default for n, a in zip(names[1:], lum[1:])}
    base, logs = ph.base.ravel(), ph.logs.ravel()
    ordered = np.sort(base.astype(np.float32))
    at = lambda q: ordered[int(np.float32(base.size - 1) * np.float32(q))]
    kinds = list(OUTPUTS)    # RuleCurve::ALL's order
    extreme = {n: float(curve_at(curves, k, at(PAIR_LOW if n[0] == 'S' else PAIR_HIGH))) for k, n in enumerate(kinds)}
    reach = {n: float(np.clip(curve_at(curves, k, base) / extreme[n], 0, 1).mean()) if extreme[n] else 0.
             for k, n in enumerate(kinds)}
    if 'pix_tables' not in curves.files:
        raise SystemExit(f'{key}: curves npz is missing pix_tables; re-run `taps` with a current build')
    def pix_at(k, x):
        tab, lo, hi = curves['pix_tables'][k].astype(np.float64), float(curves['pix_lo']), float(curves['pix_hi'])
        f = np.clip((np.asarray(x, np.float64) - lo) / (hi - lo) * len(tab) - 0.5, 0, len(tab) - 1)
        i = np.minimum(f.astype(int), len(tab) - 2)
        return tab[i] + (tab[i + 1] - tab[i]) * (f - i)
    photo_stats = [float(ph.top), float(ph.q[0.5]), float(ph.mean), float(ph.bot),
                   float(ph.top - ph.q[0.5]), float(ph.q[0.5] - ph.bot),
                   float(ph.q[0.9] - ph.q[0.1]), float(ph.lin_mean - ph.mean)]
    rng = np.random.default_rng(int(key[5:]))
    out = dict(key=key, X=[], E=[], I=[], S=[], F=[])
    for name, (sn, hn) in PAIRS.items():
        ks, kh = kinds.index(sn), kinds.index(hn)
        i_s, i_h, i_p = names.index(sn), names.index(hn), names.index(name)
        valid = np.ones(ph.base.shape, bool)
        for j in (0, i_s, i_h, i_p):
            valid &= (on[j] > CLIP_LOW) & (pk[j] < CLIP_HIGH)
        idx = np.flatnonzero(valid.ravel())
        if idx.size < 500:
            print('too few unclipped pixels', key, name)
            return None
        idx = rng.choice(idx, min(6000, idx.size), replace=False)
        a, b = curve_at(curves, ks, base[idx]), curve_at(curves, kh, base[idx])
        pa, pb = pix_at(ks, logs[idx]), pix_at(kh, logs[idx])
        gp, gs, gh = gain_maps[name].ravel()[idx], gain_maps[sn].ravel()[idx], gain_maps[hn].ravel()[idx]
        out['X'].append(np.stack([extreme[sn] - a, extreme[hn] - b, np.ones_like(a)], 1))
        out['E'].append((a + pa) + (b + pb) - gp)
        out['I'].append(gp - gs - gh)
        out['S'].append(np.stack([(a + pa) - gs, (b + pb) - gh], 1))
        es, eh = abs(extreme[sn]), abs(extreme[hn])
        out['F'].append(np.array([reach[sn], reach[hn], es, eh,
                                  1 / max(es, PAIR_EXTREME_MIN), 1 / max(eh, PAIR_EXTREME_MIN)] + photo_stats))
    return out


def pair_fit(data, c):
    """For pair `c`, the 14 inputs' means and deviations and the intercept and weights
    for Shadows' blend fraction, Highlights' blend fraction and the uniform shift: the
    linear map that best reproduces Camera Raw's own pair interaction `I` (`gp - gs - gh`,
    free of single-curve noise) over the photos (each photo's pixels weighing as one
    photo)."""
    F = np.array([d['F'][c] for d in data])
    mu, sd = F.mean(0), F.std(0) + 1e-9
    rows, target = [], []
    for d, z in zip(data, np.clip((F - mu) / sd, -PAIR_Z_CLAMP, PAIR_Z_CLAMP)):
        p = np.r_[1., z]
        w = 1 / np.sqrt(len(d['I'][c]))
        X = d['X'][c]
        rows.append(np.concatenate([X[:, :1] * p, X[:, 1:2] * p, X[:, 2:3] * p], 1) * w)
        target.append(d['I'][c] * w)
    A, t = np.concatenate(rows), np.concatenate(target)
    n = F.shape[1] + 1
    ridge = PAIR_RIDGE * len(data) / 100 * np.eye(3 * n)
    ridge[0, 0] = ridge[n, n] = ridge[2 * n, 2 * n] = 0
    beta = np.linalg.solve(A.T @ A + ridge, A.T @ t)
    return mu, sd, beta.reshape(3, n)


def pair_fractions(model, d, c):
    mu, sd, beta = model
    z = np.clip((d['F'][c] - mu) / sd, -PAIR_Z_CLAMP, PAIR_Z_CLAMP)
    coef = beta @ np.r_[1., z]
    return np.array([np.clip(coef[0], 0., 1.), np.clip(coef[1], 0., 1.),
                     np.clip(coef[2], -PAIR_SHIFT_MAX, PAIR_SHIFT_MAX)])


def pair(args):
    """Fits `Pair`'s fractions and uniform shifts and writes local_tone_rule_pair_data.rs."""
    keys = [p.stem for p in sorted((args.root / 'maps').glob('photo[0-9]*.npz'))
            if (args.root / 'curves' / p.name).exists()
            and all(rendered(args.root / 'photos' / 'crlin' / f'{p.stem[5:]}-{n}.tif') for n in list(SETTINGS) + list(PAIRS))]
    sensor_map = load_sensor_map(args.root)
    import multiprocessing
    with multiprocessing.get_context('fork').Pool(6) as pool:
        data = [d for d in pool.imap(pair_sample, [(k, args.root, sensor_map) for k in keys], chunksize=4) if d]
    print(f'{len(data)} of {len(keys)} photos')
    rms = lambda v: float(np.sqrt(np.mean(v ** 2)))
    line = lambda label, v: print(f'  {label:48s}' + '  '.join(f'{np.median(v[:, c]):.3f} ({v[:, c].mean():.3f})' for c in range(4)))
    print('gain error at each pair, RMS over a photo\'s pixels (EV), median (mean) over photos: ' + ', '.join(PAIRS))
    line('Camera Raw\'s gain less the sum of its singles', np.array([[rms(d['I'][c]) for c in range(4)] for d in data]))
    for k, label in enumerate(('Shadows', 'Highlights')):
        line(f'the engine\'s {label} curve alone', np.array([[rms(d['S'][c][:, k]) for c in range(4)] for d in data]))
    plain = np.array([[rms(d['E'][c]) for c in range(4)] for d in data])
    line('the sum of the engine\'s two curves', plain)
    def own(d, c):
        w = np.linalg.lstsq(d['X'][c], -d['E'][c], rcond=None)[0]
        return np.array([np.clip(w[0], 0., 1.), np.clip(w[1], 0., 1.), np.clip(w[2], -PAIR_SHIFT_MAX, PAIR_SHIFT_MAX)])
    line('coefs fitted to each photo', np.array([[rms(d['E'][c] + d['X'][c] @ own(d, c)) for c in range(4)] for d in data]))
    if args.cv:
        fold = np.random.default_rng(0).permutation(len(data)) % 5
        held = np.zeros((len(data), 4))
        for k in range(5):
            train = [d for d, f in zip(data, fold) if f != k]
            for c in range(4):
                model = pair_fit(train, c)
                for i in np.flatnonzero(fold == k):
                    held[i, c] = rms(data[i]['E'][c] + data[i]['X'][c] @ pair_fractions(model, data[i], c))
        line('coefs predicted, photo held out', held)
        print('  photos closer / further by 0.02 EV than the sum: ' + '  '.join(
            f'{int((held[:, c] < plain[:, c] - 0.02).sum())} / {int((held[:, c] > plain[:, c] + 0.02).sum())}' for c in range(4)))
    models = [pair_fit(data, c) for c in range(4)]
    fitted = np.array([[rms(d['E'][c] + d['X'][c] @ pair_fractions(models[c], d, c)) for c in range(4)] for d in data])
    line('coefs predicted, fitted on all', fitted)
    coefs = np.array([[pair_fractions(models[c], d, c) for c in range(4)] for d in data])
    print('  median (Shadows f, Highlights f, shift c0): ' + '  '.join(
        f'({np.median(coefs[:, c, 0]):.2f}, {np.median(coefs[:, c, 1]):.2f}, {np.median(coefs[:, c, 2]):+.2f})' for c in range(4)))
    nf = models[0][0].size
    arr = lambda v: '[' + ', '.join(repr(float(np.float32(x))) for x in v) + ']'
    DATA_PAIR.write_text(
        "//! How Shadows' and Highlights' blend fractions and uniform shift follow the photo\n"
        "//! (`local_tone_rule::Pair`): per sign combination, the 14 inputs' means and\n"
        "//! deviations and, for Shadows' blend, Highlights' blend and the uniform shift,\n"
        "//! the intercept and weights. Written by `scripts/corpus/scene-tone-rule.py pair`.\n"
        '#![allow(clippy::excessive_precision, clippy::unreadable_literal)]\n'
        f'/// Photo inputs `Pair` reads.\npub(super) const FEATURES: usize = {nf};\n'
        # One row of numbers per line: rustfmt would put each number on a line of its own.
        + f'#[rustfmt::skip]\npub(super) const MU: [[f32; FEATURES]; 4] = [\n' + ''.join(f'    {arr(m[0])},\n' for m in models) + '];\n'
        + f'#[rustfmt::skip]\npub(super) const SD: [[f32; FEATURES]; 4] = [\n' + ''.join(f'    {arr(m[1])},\n' for m in models) + '];\n'
        + f'#[rustfmt::skip]\npub(super) const BETA: [[[f32; FEATURES + 1]; 3]; 4] = [\n'
        + ''.join(f'    [\n        {arr(m[2][0])},\n        {arr(m[2][1])},\n        {arr(m[2][2])},\n    ],\n' for m in models) + '];\n')
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
    s = sub.add_parser('scenes')
    s.add_argument('root', type=Path)
    s.add_argument('--count', type=int, default=SCENES)
    s = sub.add_parser('gaps')
    s.add_argument('root', type=Path)
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
    steps = {'probes': probes, 'photos': photos, 'scenes': scenes, 'gaps': gaps, 'taps': taps, 'train': train, 'pair': pair}
    steps[args.step](args)


if __name__ == '__main__':
    main()
