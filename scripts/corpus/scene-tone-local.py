#!/usr/bin/env python3
"""Measure the scene tone stage's local operators in Camera Raw and write their tables.

  synth   DIR        writes the synthetic scenes (scripts/corpus/probe_scenes.py, seeds
                     0-39) as probe DNGs into DIR and renders them in Camera Raw
                     (camera_raw.py) at each slider; renders are kept as half-size
                     float16 arrays (DIR/scene<seed>-<setting>.npy)
  photos  DIR LIST   prepares the training photos listed in LIST (one RAW path per
                     line): linear-profile DNGs (linear_profile.py) in DIR/dng, Camera
                     Raw renders at each Shadows position in DIR/renders, and RAWmakase's
                     scene-input and scene-output taps (--rawmakase) in DIR/maps;
                     existing renders and taps are kept, so delete DIR/maps to
                     refit after an engine change
  tables  SYNTH PHOTOS TONE
                     fits the tables and writes local_tone_data.rs and clarity_data.rs
                     (--out, default crates/rawmakase-engine/src/develop). TONE is
                     scene-tone-tables.py's render folder: its white ramps at default
                     settings give Camera Raw's global curve on the synthetic scenes.

With a linear profile Camera Raw's ProPhoto output is its scene tone stage's
(docs/scene-tone-stage.md#comparison-contract). A slider's effect is read as a log2
gain on scene values: both the slider's render and the default render are mapped back
through the default render's global curve, and the gain is their log2 difference. The
synthetic scenes use the curve measured on the white ramps at their white point; a
training photo uses its own curve, RAWmakase's scene output against its scene input.
The taps must come from the engine the tables are for: its global curves, the default
black included, are what the gains are measured through.

Each image is box-averaged to a map of 512 pixels on the long edge, with L the log2 of
its luminance (floor 2^-14) and B a self-guided box filter of L (radius 0.032 of the
long edge, eps 0.5), as local_tone.rs computes them. Pixels near black, near the
output's white or below 2^-10 are left out of the fits.

- Shadows (training photos): the gain as a function of B minus the key log2(mean Y),
  over [-8, 4].
- Highlights: the gain as a function of B minus the key, the mean of L's 75th and 1st
  percentiles, over [-6, 7]; negative Highlights on the synthetic scenes, positive on
  the training photos (closer to Camera Raw on held-out photos: display ΔE00 +100 1.91
  → 1.80, +50 1.69 → 1.57; −100 even).
- Dehaze (training photos): each channel's gain as a function of its log2 level minus
  L's 99th percentile, over [-12, 1].
  Each is a 48-bin piecewise-linear table per slider position, fitted by least squares
  with a second-difference smoothness penalty.
- Clarity (synthetic scenes): the gain as a sum over four bilateral blurs of L (0.004,
  0.015, 0.05 and 0.15 of the long edge, range sigma 2 stops) of the detail L - blur
  times weights that are piecewise linear in B minus the Shadows key (a quarter of L's
  99.9th and three quarters of its 99th percentile) on 9 knots over [-8, 0.5]; a ridge
  least-squares fit per slider position.
"""
import argparse
import importlib.util
import os
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np
import tifffile
from PIL import Image

sys.dont_write_bytecode = True
HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
import camera_raw  # noqa: E402
import probe_dng  # noqa: E402
import probe_scenes  # noqa: E402

_spec = importlib.util.spec_from_file_location('scene_tone_tables', HERE / 'scene-tone-tables.py')
tone_tables = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(tone_tables)

ROOT = HERE.resolve().parents[1]
OUT = ROOT / 'crates/rawmakase-engine/src/develop'
SEEDS = range(40)
PROFILE = 'RAWmakase Linear'
YW = np.array([0.2880402, 0.7118741, 0.0000857])     # ProPhoto luminance
T = np.arange(-16, 6.001, 0.0625)                     # log2 scene value (global curve)
MAP, RADIUS, EPS, FLOOR = 512, 0.032, 0.5, 2.0 ** -14
SH = [-1., -0.5, -0.25, 0.25, 0.5, 1.]
DEHAZE = [-1., -0.4, -0.2, 0.2, 0.4, 1.]
CLARITY = [-1., -0.5, 0.5, 1.]
RANGES = dict(S=(-8., 4.), H=(-6., 7.), D=(-12., 1.))
SCALES, KNOTS, KNOT_LO, KNOT_HI = [0.004, 0.015, 0.05, 0.15], 9, -8., 0.5
SETTINGS = dict(S='Shadows2012', H='Highlights2012', D='Dehaze', C='Clarity2012')


def name(kind, v):
    return f'{kind}{int(round(v * 100)):+d}'


# Shadows and Dehaze are fitted on photos; their scene renders are scene-probes.py's
# references.
SYNTH_RENDERS = {'default': {}} | {name(k, v): {SETTINGS[k]: f'{v * 100:g}'}
                                   for k, vs in [('S', SH), ('H', SH), ('D', DEHAZE), ('C', CLARITY)]
                                   for v in vs}
PHOTO_RENDERS = {'default': {}} | {name(k, v): {SETTINGS[k]: f'{v * 100:g}'}
                                   for k, vs in [('S', SH), ('H', SH), ('D', DEHAZE)] for v in vs}


# Image helpers.

def area(a, w, h):
    return np.stack([np.array(Image.fromarray(a[..., i].astype(np.float32)).resize((w, h), Image.Resampling.BOX))
                     for i in range(a.shape[-1])], -1).astype(np.float64)


def size(shape, edge):
    h, w = shape[:2]
    s = edge / max(h, w)
    return max(1, round(w * s)), max(1, round(h * s))


def box(x, r):
    """local_tone::blur: mean over a (2r+1)^2 window, clamped at the borders."""
    def line(a, axis):
        pad = np.pad(a, [(r, r) if i == axis else (0, 0) for i in range(2)], mode='edge')
        c = np.cumsum(pad, axis=axis, dtype=np.float64)
        c = np.concatenate([np.zeros_like(np.take(c, [0], axis=axis)), c], axis=axis)
        n = a.shape[axis]
        return (np.take(c, np.arange(2 * r + 1, 2 * r + 1 + n), axis=axis) - np.take(c, np.arange(n), axis=axis)) / (2 * r + 1)
    return line(line(x, 1), 0)


def guided(L, r, eps):
    m, m2 = box(L, r), box(L * L, r)
    var = np.maximum(m2 - m * m, 0)
    a = var / (var + eps)
    b = m - a * m
    return box(a, r) * L + box(b, r)


def blur(a, sigma):
    """Gaussian blur with reflected edges."""
    if sigma < 0.3:
        return a
    h, w = a.shape
    pad = np.pad(a, ((h // 2,) * 2, (w // 2,) * 2), mode='reflect')
    fy, fx = np.fft.fftfreq(pad.shape[0])[:, None], np.fft.rfftfreq(pad.shape[1])[None, :]
    g = np.exp(-2 * (np.pi * sigma) ** 2 * (fx ** 2 + fy ** 2))
    return np.fft.irfft2(np.fft.rfft2(pad) * g, pad.shape)[h // 2:h // 2 + h, w // 2:w // 2 + w]


def bilateral(L, sigma_s, sigma_r):
    """Piecewise-linear bilateral filter (Durand & Dorsey) of L: spatial sigma `sigma_s`
    pixels, range sigma `sigma_r` stops."""
    lo, hi = np.percentile(L, 0.1) - 1, np.percentile(L, 99.9) + 1
    levels = np.arange(lo, hi + sigma_r / 2, sigma_r / 2)
    vals = []
    for level in levels:
        w = np.exp(-0.5 * ((L - level) / sigma_r) ** 2)
        vals.append(blur(w * L, sigma_s) / np.maximum(blur(w, sigma_s), 1e-12))
    vals = np.array(vals)
    f = np.clip((L - levels[0]) / (levels[1] - levels[0]), 0, len(levels) - 1 - 1e-9)
    i = f.astype(int)
    t = f - i
    rows, cols = np.indices(L.shape)
    return vals[i, rows, cols] * (1 - t) + vals[np.minimum(i + 1, len(levels) - 1), rows, cols] * t


def pct(L, q):
    v = np.sort(L.ravel())
    return v[int((v.size - 1) * q)]


def log_gain(after, before):
    return np.log2(np.maximum(after, 1e-9)) - np.log2(np.maximum(before, 1e-9))


class Map:
    """An image at map resolution: linear RGB, L and B."""
    def __init__(self, x):
        self.size = size(x.shape, MAP)
        self.x = area(x, *self.size)
        self.L = np.log2(np.maximum(self.x @ YW, FLOOR))
        self.B = guided(self.L, max(1, round(RADIUS * max(self.size))), EPS)


# Fits.

def hat(u, lo, hi, n):
    """Hat-function basis of u on n knots over [lo, hi]: (N, n)."""
    f = np.clip((u - lo) / (hi - lo) * (n - 1), 0, n - 1 - 1e-9)
    i = f.astype(int)
    w = f - i
    A = np.zeros((u.size, n))
    A[np.arange(u.size), i] = 1 - w
    A[np.arange(u.size), i + 1] = w
    return A


def fit_table(samples, lo, hi, n=48, smooth=3e-2):
    """Least-squares table on n bin centres over [lo, hi] from (u, gain) samples, with a
    second-difference penalty."""
    u = np.concatenate([u for u, _ in samples])
    t = np.concatenate([t for _, t in samples])
    f = np.clip((u - lo) / (hi - lo) * n - 0.5, 0, n - 1 - 1e-9)
    i = f.astype(int)
    w = f - i
    A = np.zeros((u.size, n))
    A[np.arange(u.size), i] = 1 - w
    A[np.arange(u.size), i + 1] = w
    D = np.diff(np.eye(n), 2, axis=0)
    M = A.T @ A + smooth * len(t) / n * D.T @ D + 1e-9 * np.eye(n)
    return np.linalg.solve(M, A.T @ t)


def synthetic(seed):
    """Scene `seed` as written to its DNG, and its baseline exposure: the sensor's white
    is 2^0.5 (clipping the brightest regions) in half the scenes, else just above the
    scene's maximum."""
    image, _ = probe_scenes.scene(seed)
    rng = np.random.default_rng(1000 + seed)
    be = 0.5 if rng.random() < 0.5 else float(np.ceil(np.log2(image.max()) * 4 + 0.5) / 4)
    return image.astype(np.float32), be


def white_point(sensor, maximum):
    """Camera Raw's white point: the sensor's white, limited to twice the image's maximum
    (never below 1); below 1 it expands by at most one stop."""
    return max(sensor, 0.5) if sensor < 1 else min(sensor, max(2 * maximum, 1.0))


def global_curve(tone):
    """Camera Raw's default global curve per white point (rows, tone_tables.WHITE_POINTS)
    at log2 scene values T, from scene-tone-tables.py's white ramps."""
    rows = []
    for lw in tone_tables.WHITE_POINTS:
        g = tone_tables.white_ramp(lw)
        x = np.array(g.values)
        y = g.measure(camera_raw.read_linear(tone / f'white{lw:+.2f}-default.tif'))
        row = np.interp(T, np.log2(x), y, right=1.0)
        below = T < np.log2(x[0])
        row[below] = 2.0 ** T[below] * y[0] / x[0]        # linear below the ramp
        rows.append(row)
    return np.array(rows)


def inverse(curves, lw):
    """Scene values from the global curve's output at white point 2^lw."""
    lws = tone_tables.WHITE_POINTS
    lw = np.clip(lw, lws[0], lws[-1])
    i = min(np.searchsorted(lws, lw, side='right') - 1, len(lws) - 2)
    f = (lw - lws[i]) / (lws[i + 1] - lws[i])
    c = curves[i] * (1 - f) + curves[i + 1] * f
    c = np.maximum.accumulate(c) + np.arange(len(c)) * 1e-12
    return lambda y: 2.0 ** np.interp(y, c, T)


def synthetic_tables(folder, tone):
    curves = global_curve(tone)
    scenes = []
    for seed in SEEDS:
        x, be = synthetic(seed)
        x = np.minimum(x.astype(np.float64), 2.0 ** be)
        m = Map(x)
        inv = inverse(curves, np.log2(white_point(2.0 ** be, area(x, *size(x.shape, 128)).max())))
        scenes.append((seed, m, inv, lambda n, seed=seed, m=m: area(np.load(folder / f'scene{seed}-{n}.npy').astype(np.float64), *m.size)))
        print(f'scene {seed}', flush=True)

    def luminance_gain(m, inv, render, n):
        d, r = render('default'), render(n)
        dl, rl = d @ YW, r @ YW
        ok = (dl > 2 ** -12) & (rl > 2 ** -12) & (d.max(-1) < 0.99) & (r.max(-1) < 0.99) & (m.L > -10)
        return log_gain(inv(rl), inv(dl)), ok

    def report(n, samples, table, lo, hi):
        err = sum(np.abs(np.interp(u, np.linspace(lo, hi, 97)[1::2], table) - t).sum() for u, t in samples)
        count = sum(t.size for _, t in samples)
        print(f'{n}: error {err / count:.4f}, no-op {sum(np.abs(t).sum() for _, t in samples) / count:.4f}', flush=True)

    out = {}
    lo, hi = RANGES['H']
    out['H'] = []
    for v in SH:
        samples = []
        for seed, m, inv, render in scenes:
            t, ok = luminance_gain(m, inv, render, name('H', v))
            samples.append(((m.B - (0.5 * pct(m.L, 0.75) + 0.5 * pct(m.L, 0.01)))[ok], t[ok]))
        out['H'].append(fit_table(samples, lo, hi))
        report(name('H', v), samples, out['H'][-1], lo, hi)
    features = []
    for seed, m, inv, render in scenes:
        key = 0.25 * pct(m.L, 0.999) + 0.75 * pct(m.L, 0.99)
        weights = hat((m.B - key).ravel(), KNOT_LO, KNOT_HI, KNOTS)
        features.append(np.hstack([weights * (m.L - bilateral(m.L, s * max(m.size), 2.0)).ravel()[:, None]
                                   for s in SCALES]))
    out['C'] = {}
    for v in CLARITY:
        X, y = [], []
        for (seed, m, inv, render), f in zip(scenes, features):
            t, ok = luminance_gain(m, inv, render, name('C', v))
            X.append(f[ok.ravel()])
            y.append(t[ok])
        A, y = np.vstack(X), np.concatenate(y)
        w = np.linalg.solve(A.T @ A + 1e-3 * np.eye(A.shape[1]) * len(y) / A.shape[1], A.T @ y)
        out['C'][name('C', v)] = w.reshape(len(SCALES), KNOTS)
        print(f"{name('C', v)}: error {np.abs(A @ w - y).mean():.4f}, no-op {np.abs(y).mean():.4f}", flush=True)
    return out


def photo_tables(d):
    """Shadows, positive Highlights and Dehaze from the training photos with complete
    renders and taps."""
    photos = []
    for dng in sorted((d / 'dng').glob('*.dng')):
        tap = d / 'maps' / f'{dng.stem}.npz'
        renders = {n: d / 'renders' / f'{dng.stem}-{n}.tif' for n in PHOTO_RENDERS}
        if not tap.exists() or not all(p.exists() for p in renders.values()):
            print(f'{dng.name}: no taps or renders, left out', flush=True)
            continue
        z = np.load(tap)
        xin, xout = z['xin'].astype(np.float64), z['xout'].astype(np.float64)
        shape = (xin.shape[1], xin.shape[0])
        lin = xin @ YW
        L = np.log2(np.maximum(lin, FLOOR))
        B = guided(L, max(1, round(RADIUS * max(shape))), EPS)
        # The photo's own global curve: scene output against input luminance, made monotone.
        o = np.argsort(lin.ravel())
        xs, ys = lin.ravel()[o], np.maximum.accumulate((xout @ YW).ravel()[o])
        photos.append((xin, L, B, lambda y, xs=xs, ys=ys: np.interp(y, ys, xs),
                       {n: area(camera_raw.read_linear(p), *shape) for n, p in renders.items()}))
    print(f'{len(photos)} training photos', flush=True)
    lo, hi = RANGES['S']
    tables = []
    for v in SH:
        samples = []
        for xin, L, B, inv, renders in photos:
            d, r = renders['default'] @ YW, renders[name('S', v)] @ YW
            ok = (d > 2 ** -11) & (r > 2 ** -11) & (renders['default'].max(-1) < 0.98) & (L > -10)
            samples.append(((B - np.log2(np.mean(2.0 ** L)))[ok], log_gain(inv(r), inv(d))[ok]))
        tables.append(fit_table(samples, lo, hi))
        print(f"{name('S', v)}: {sum(t.size for _, t in samples)} samples", flush=True)
    lo, hi = RANGES['H']
    highlights = []
    for v in [v for v in SH if v > 0]:
        samples = []
        for xin, L, B, inv, renders in photos:
            d, r = renders['default'] @ YW, renders[name('H', v)] @ YW
            ok = (d > 2 ** -11) & (r > 2 ** -11) & (renders['default'].max(-1) < 0.98) & (L > -10)
            key = 0.5 * pct(L, 0.75) + 0.5 * pct(L, 0.01)
            samples.append(((B - key)[ok], log_gain(inv(r), inv(d))[ok]))
        highlights.append(fit_table(samples, lo, hi))
        print(f"{name('H', v)}: {sum(t.size for _, t in samples)} samples", flush=True)
    lo, hi = RANGES['D']
    dehaze = []
    for v in DEHAZE:
        samples = []
        for xin, L, B, inv, renders in photos:
            d, r = renders['default'], renders[name('D', v)]
            for c in range(3):
                ok = ((d[..., c] > 2 ** -11) & (r[..., c] > 2 ** -11) & (d.max(-1) < 0.98) & (r.max(-1) < 0.98)
                      & (xin[..., c] > 2 ** -12))
                u = np.log2(np.maximum(xin[..., c], 1e-9)) - pct(L, 0.99)
                samples.append((u[ok], log_gain(inv(r[..., c]), inv(d[..., c]))[ok]))
        dehaze.append(fit_table(samples, lo, hi))
        print(f"{name('D', v)}: {sum(t.size for _, t in samples)} samples", flush=True)
    return tables, highlights, dehaze


# Rust output.

def family(const, tables, values, lo, hi):
    rows = []
    for table in tables:
        vals = [f'{v:.4f}' for v in table]
        lines = [', '.join(vals[i:i + 9]) for i in range(0, 48, 9)]
        rows.append('        [\n            ' + ',\n            '.join(lines) + ',\n        ],')
    vs = ', '.join(f'{v:g}' if v != int(v) else f'{v:.0f}.' for v in values)
    return (f'pub(super) const {const}: Family = Family {{\n    values: [{vs}],\n    lo: {lo:.1f},\n'
            f'    hi: {hi:.1f},\n    tables: [\n' + '\n'.join(rows) + '\n    ],\n};\n')


LOCAL_HEADER = '''//! Camera Raw 18.7's Shadows, Highlights and Dehaze on scene values (process version
//! 2012), measured with a linear profile (`scripts/corpus/scene-tone-local.py`):
//! Shadows, Dehaze and positive Highlights on the training photos, negative Highlights
//! on synthetic scenes. Each table is a log2 gain as a function of a level relative to
//! an image key: Shadows' and Highlights' of the local base level (local_tone.rs) and
//! their keys, Dehaze's of each channel's level relative to the photo's 99th
//! percentile. Rows are the slider positions `values`; columns are bin centres over
//! [lo, hi].
#![allow(clippy::excessive_precision, clippy::approx_constant)]
pub(crate) struct Family {
    pub values: [f32; 6],
    pub lo: f32,
    pub hi: f32,
    pub tables: [[f32; 48]; 6],
}
'''

CLARITY_HEADER = '''// Clarity in the scene tone stage (clarity.rs): per blur scale (rows, as `SCALES`) the
// log2 gain per unit of detail at base levels from `KNOT_LO` to `KNOT_HI` relative to
// the photo's brightest levels. Least-squares fits of the log2 change Camera Raw 18.7
// renders on scene values at Clarity −100, −50, +50 and +100 on synthetic scenes
// (scripts/corpus/scene-tone-local.py).
const KNOTS: usize = 9;
'''


def weights(const, rows):
    return (f'const {const}: [[f32; KNOTS]; 4] = [\n'
            + ''.join('    [' + ', '.join(f'{v:.4f}' for v in r) + '],\n' for r in rows) + '];\n')


# Steps.

def synth(args):
    d = args.dir.resolve()
    d.mkdir(parents=True, exist_ok=True)
    for seed in SEEDS:
        todo = {n: s for n, s in SYNTH_RENDERS.items() if not (d / f'scene{seed}-{n}.npy').exists()}
        if not todo:
            continue
        image, be = synthetic(seed)
        dng = d / f'scene{seed}.dng'
        probe_dng.write(dng, image, baseline_exposure=be)
        jobs = [dict(source=str(dng), settings=s, out=str(d / f'scene{seed}-{n}.tif')) for n, s in todo.items()]
        failures = camera_raw.render(jobs, work=d / 'work')
        for job in jobs:
            # Half size as float16 keeps the 680 renders small; the fits use 512-pixel maps.
            tif = Path(job['out'])
            if tif.exists():
                a = camera_raw.read_linear(tif)
                np.save(tif.with_suffix('.npy'), area(a, a.shape[1] // 2, a.shape[0] // 2).astype(np.float16))
                tif.unlink()
        print(f'scene {seed}: {len(jobs)} renders, {len(failures)} failed', flush=True)


def photos(args):
    d = args.dir.resolve()
    raws = [Path(line.strip()) for line in args.list.read_text().splitlines() if line.strip()]
    subprocess.run([sys.executable, str(HERE / 'linear_profile.py'), str(d / 'dng'), *map(str, raws)], check=True)
    dngs = sorted((d / 'dng').glob('*.dng'))
    (d / 'renders').mkdir(exist_ok=True)
    jobs = [dict(source=str(dng), settings=dict(s, CameraProfile=PROFILE), edge=1024,
                 out=str(d / 'renders' / f'{dng.stem}-{n}.tif')) for dng in dngs for n, s in PHOTO_RENDERS.items()]
    failures = camera_raw.render(jobs, work=d / 'work')
    print(f'{len(jobs)} renders, {len(failures)} failed', flush=True)
    (d / 'maps').mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory() as t:
        t = Path(t)
        (t / 'data').mkdir()
        (t / 'linear.xmp').write_text(camera_raw.xmp({'CameraProfile': PROFILE}))
        # An empty data folder, so no presets or profiles of this machine apply.
        env = dict(os.environ, RAWMAKASE_DATA_DIR=str(t / 'data'))
        for dng in dngs:
            target = d / 'maps' / f'{dng.stem}.npz'
            if target.exists():
                continue
            taps = {}
            for key, stage in [('xin', 'scene-input'), ('xout', 'scene-output')]:
                r = subprocess.run([str(args.rawmakase), 'render', str(dng), str(t / 'tap.tif'), '--tap', stage,
                                    '--xmp', str(t / 'linear.xmp'), '--overwrite'], capture_output=True, text=True, env=env)
                if r.returncode:
                    print(f'{dng.name}: {stage} failed: {r.stderr.strip()}', flush=True)
                    break
                taps[key] = tifffile.imread(str(t / 'tap.tif')).astype(np.float64)
            else:
                shape = size(taps['xin'].shape, MAP)
                np.savez(target, **{k: area(v, *shape).astype(np.float32) for k, v in taps.items()})
                print(target, flush=True)


def tables(args):
    S, H, D = photo_tables(args.photos.resolve())
    t = synthetic_tables(args.synth.resolve(), args.tone.resolve())
    out = args.out.resolve()
    (out / 'local_tone_data.rs').write_text(
        LOCAL_HEADER + family('SHADOWS', S, SH, *RANGES['S']) + family('HIGHLIGHTS', t['H'][:3] + H, SH, *RANGES['H'])
        + family('DEHAZE', D, DEHAZE, *RANGES['D']))
    c = t['C']
    (out / 'clarity_data.rs').write_text(
        CLARITY_HEADER + weights('WEIGHTS_M100', c['C-100']) + weights('WEIGHTS_M50', c['C-50'])
        + weights('WEIGHTS_50', c['C+50']) + weights('WEIGHTS_100', c['C+100']))
    print(f'local_tone_data.rs, clarity_data.rs -> {out}')


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest='step', required=True)
    s = sub.add_parser('synth', help='write and render the synthetic scenes')
    s.add_argument('dir', type=Path, help='folder for the scenes and renders (outside the repository)')
    s = sub.add_parser('photos', help='prepare the training photos')
    s.add_argument('dir', type=Path, help='folder for the DNGs, renders and taps (outside the repository)')
    s.add_argument('list', type=Path, help='text file of RAW paths, one per line')
    s.add_argument('--rawmakase', type=Path, required=True, help='RAWmakase binary for the scene taps')
    s = sub.add_parser('tables', help='fit and write the tables')
    s.add_argument('synth', type=Path, help="the synth step's folder")
    s.add_argument('photos', type=Path, help="the photos step's folder")
    s.add_argument('tone', type=Path, help="scene-tone-tables.py's render folder (its white ramps)")
    s.add_argument('--out', type=Path, default=OUT, help='folder for the .rs files (default: %(default)s)')
    args = p.parse_args()
    {'synth': synth, 'photos': photos, 'tables': tables}[args.step](args)


if __name__ == '__main__':
    main()
