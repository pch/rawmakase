#!/usr/bin/env python3
"""Measure Camera Raw's Color Grading on the synthetic chart and write
crates/rawmakase-engine/src/develop/color_grade_curves.bin.

`render` has Photoshop 2026 (Camera Raw) render synthetic-d65.dng with each fitting
case (about 750: every region at twelve hues and four saturations, Shadows, Midtones
and Highlights over a grid of Blending and Balance, the Luminance sliders, and
held-out checks) as 16-bit ProPhoto RGB, and keeps the mean of every chart patch in a
JSON file outside the repository (--refs); the TIFFs are deleted as they are read. It
refuses to start while Photoshop has documents open.

`fit` turns those patch means into the tables RAWmakase renders with, and prints how
far the fitted operator is from every render (mean CIEDE2000 over the chart, with
Camera Raw's own default render as input, so only the grading's error is measured).

What the renders show (see docs/color-mixer.md#color-grading):
- Color grading is a curve per channel of linear ProPhoto RGB: a channel's output
  depends only on that channel's input (predicting every patch from the gray ramp's
  per-channel curves leaves 0.1-0.2 ΔE00; the same curves in sRGB, Display P3,
  Rec. 2020 or ACES AP1 primaries leave 0.6-4.4).
- Each region's curves are, as gains, close to 1 + A1(x) p1 + A2(x) p2 (three terms
  for Global), with profiles A over the channel value and coefficients p per channel
  that depend on hue and saturation only. Blending and Balance change the profiles,
  not the coefficients. Regions multiply.
- Blending and Balance leave Global alone. Legacy split toning (SplitToning keys
  without ColorGrade keys) renders exactly as Blending 100.
- Luminance is a curve common to the channels, applied before the tint. Under other
  Blending or Balance it keeps its strength for the same relative region weight.

Tables (f32, little endian; E = 64 samples of x^(1/2.2) over 0-1):
  GRID  3 regions (Shadows, Midtones, Highlights) x 5 Blending x 17 Balance x E x 2
  GLOBAL E x 3                      Global's profiles
  COEF  4 regions x 12 hues x 4 saturations (25-100) x 3 channels x 3 terms
  LUM   4 regions x 8 amounts (-100..+100 without 0) x E   gains

Requires numpy. Run from the repository root:
  python3 scripts/corpus/color-grading.py render [--refs FILE]
  python3 scripts/corpus/color-grading.py fit [--refs FILE]
"""
import argparse
import importlib.util
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
CORPUS = ROOT / 'tests/corpus'
_spec = importlib.util.spec_from_file_location('charts', Path(__file__).parent / 'camera-raw-charts.py')
charts = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(charts)

N = 64
E = np.linspace(0, 1, N)
BLENDS = [0, 25, 50, 75, 100]
BALANCES = [-100, -88, -75, -62, -50, -38, -25, -12, 0, 12, 25, 38, 50, 62, 75, 88, 100]
HUES = list(range(0, 360, 30))
SATURATIONS = [25, 50, 75, 100]
LUMINANCES = [-100, -75, -50, -25, 25, 50, 75, 100]
TERMS = {'S': 2, 'M': 2, 'H': 2, 'G': 3}
KEYS = {'S': ('SplitToningShadowHue', 'SplitToningShadowSaturation', 'ColorGradeShadowLum'),
        'M': ('ColorGradeMidtoneHue', 'ColorGradeMidtoneSat', 'ColorGradeMidtoneLum'),
        'H': ('SplitToningHighlightHue', 'SplitToningHighlightSaturation', 'ColorGradeHighlightLum'),
        'G': ('ColorGradeGlobalHue', 'ColorGradeGlobalSat', 'ColorGradeGlobalLum')}


def settings(regions, blend=50, balance=0):
    """regions: {region: (hue, saturation, luminance)}. ColorGradeBlending is always
    written, so Camera Raw doesn't read the case as legacy split toning."""
    s = {'ColorGradeBlending': str(blend), 'SplitToningBalance': f'{balance:+d}' if balance else '0'}
    for r, (h, sat, lum) in regions.items():
        kh, ks, kl = KEYS[r]
        if sat:
            s[kh], s[ks] = str(h), str(sat)
        if lum:
            s[kl] = f'{lum:+d}'
    return s


def hs_name(r, h, s):
    return f'{r}-h{h}-s{s}'


def grid_name(r, h, blend, balance):
    return hs_name(r, h, 50) if (blend, balance) == (50, 0) else f'{r}-h{h}-blend{blend}-balance{balance:+d}'


# Held out from the fit: other hues, saturations, Blending and Balance, regions and
# Luminance together.
CHECKS = {
    'check-shadows-h75-s40-blend60-balance+20': ({'S': (75, 40, 0)}, 60, 20),
    'check-midtones-h300-s65-blend30-balance-70': ({'M': (300, 65, 0)}, 30, -70),
    'check-highlights-h150-s85-blend90-balance+45': ({'H': (150, 85, 0)}, 90, 45),
    'check-all-regions': ({'S': (200, 30, -20), 'M': (40, 15, 10), 'H': (50, 35, 15), 'G': (20, 10, 0)}, 70, -15),
    'check-split-blend100': ({'S': (190, 60, 0), 'H': (35, 60, 0)}, 100, 0),
    'check-three-way': ({'S': (15, 20, 0), 'M': (100, 25, 0), 'H': (260, 20, 0)}, 50, 0),
    'check-shadow-lum+30-blend80-balance-40': ({'S': (0, 0, 30)}, 80, -40),
    'check-midtone-lum-40-blend20-balance+60': ({'M': (0, 0, -40)}, 20, 60),
    'check-global-h100-s70-lum-30': ({'G': (100, 70, -30)}, 50, 0),
    'check-split-blend35-balance+33': ({'S': (230, 45, 0), 'H': (45, 45, 0)}, 35, 33),
    'check-shadow-lum+50-blend100': ({'S': (0, 0, 50)}, 100, 0),
    'check-highlight-lum-50-balance+50': ({'H': (0, 0, -50)}, 50, 50),
    'check-midtone-lum+50-balance+50': ({'M': (0, 0, 50)}, 50, 50),
}


def cases():
    out = {'default': {}}
    for r in 'SMHG':
        for h in HUES:
            for s in SATURATIONS:
                out[hs_name(r, h, s)] = settings({r: (h, s, 0)})
        for lum in LUMINANCES:
            out[f'{r}-lum{lum:+d}'] = settings({r: (0, 0, lum)})
    for r in 'SMH':
        for h in (30, 210):
            for b in BLENDS:
                for a in BALANCES:
                    out[grid_name(r, h, b, a)] = settings({r: (h, 50, 0)}, b, a)
    for name, (regions, b, a) in CHECKS.items():
        out[name] = settings(regions, b, a)
    return out


TEMPLATE = charts.TEMPLATE.replace('"sRGB IEC61966-2.1"', '"ProPhoto RGB"')


def render(refs):
    done = json.loads(refs.read_text()) if refs.exists() else {}
    todo = {k: v for k, v in cases().items() if k not in done}
    if not todo:
        return
    if not charts.photoshop_idle():
        sys.exit(f'{charts.PHOTOSHOP} is not running or has documents open; close them (another session may be using it).')
    base = json.loads((CORPUS / 'cases.json').read_text())['base']
    layout = json.loads((CORPUS / 'charts/layout.json').read_text())
    work = Path(tempfile.mkdtemp(prefix='color-grading-'))
    source = CORPUS / 'charts/synthetic-d65.dng'
    jobs = [{'source': str(source), 'dng': str(work / 'chart.dng'), 'xmp': str(work / 'chart.xmp'),
             'settings': charts.xmp(base, {'settings': s}, {}), 'out': str(work / 'out' / f'{k}.tif')}
            for k, s in todo.items()]
    script = work / 'render.jsx'
    script.write_text(TEMPLATE % {'jobs': json.dumps(jobs)})
    print(f'Rendering {len(jobs)} cases in {work}', flush=True)
    subprocess.run(['osascript', '-e', f'with timeout of 36000 seconds\ntell application "{charts.PHOTOSHOP}" '
                    f'to do javascript file (POSIX file "{script}")\nend timeout'], check=True, stdout=subprocess.DEVNULL)
    for k in todo:
        tiff = work / 'out' / f'{k}.tif'
        if tiff.exists():
            done[k] = charts.patch_values(tiff, layout['patches'])
        else:
            print(f'  missing render: {k}', file=sys.stderr)
    shutil.rmtree(work)
    refs.write_text(json.dumps(done))


def linear(v16):
    """16-bit ProPhoto RGB (gamma 1.8, as Photoshop's profile) to linear."""
    return (np.asarray(v16, float) / 65535) ** 1.8


RAMP = np.array([p['group'] == 'ramp' for p in json.loads((CORPUS / 'charts/layout.json').read_text())['patches']])


def channel_gains(default, rendered):
    """The case's per-channel gain curves at E (N x 3), from the gray ramp.

    Every patch's channels lie on these curves (0.1-0.2 ΔE00 left when the ramp's
    curves are applied to the whole chart), but the ramp's are the least noisy: dark
    channels of saturated patches lose precision in 16 bits."""
    d, c = linear(default)[RAMP], linear(rendered)[RAMP]
    e = d[:, 1] ** (1 / 2.2)
    g = c / d
    return np.stack([np.interp(E, e, g[:, ch]) for ch in range(3)], 1)


def fit_tables(data):
    default = data['default']
    gain = {k: channel_gains(default, v) for k, v in data.items() if k != 'default'}
    coef = np.zeros((4, 12, 4, 3, 3))
    grid = np.zeros((3, 5, 17, N, 2))
    globals_ = np.zeros((N, 3))
    lum = np.zeros((4, 8, N))
    for ri, r in enumerate('SMHG'):
        k = TERMS[r]
        keys = [(h, s) for h in HUES for s in SATURATIONS]
        y = np.array([gain[hs_name(r, h, s)] - 1 for h, s in keys])          # cases x E x 3
        u, sv, vt = np.linalg.svd(y.transpose(1, 0, 2).reshape(N, -1), full_matrices=False)
        profiles = u[:, :k] * sv[:k]
        scale = np.abs(profiles).max(0)
        profiles /= scale
        p = (vt[:k] * scale[:, None]).T.reshape(len(keys), 3, k)
        for i, (h, s) in enumerate(keys):
            coef[ri, HUES.index(h), SATURATIONS.index(s), :, :k] = p[i]
        if r == 'G':
            globals_[:, :k] = profiles
        else:
            for bi, b in enumerate(BLENDS):
                for ai, a in enumerate(BALANCES):
                    ys = np.concatenate([gain[grid_name(r, h, b, a)] - 1 for h in (30, 210)], 1)
                    ps = np.concatenate([coef[ri, HUES.index(h), 1, :, :k] for h in (30, 210)], 0)
                    grid[ri, bi, ai] = np.linalg.lstsq(ps, ys.T, rcond=None)[0].T
        for li, l in enumerate(LUMINANCES):
            lum[ri, li] = np.clip(gain[f'{r}-lum{l:+d}'].mean(1), 1e-3, None)
    return {'grid': grid, 'global': globals_, 'coef': coef, 'lum': lum}, gain


# The operator, as crates/rawmakase-engine/src/develop/color_grade.rs renders it, for checking the fit.
def _interp(xs, x):
    x = min(max(x, xs[0]), xs[-1])
    i = min(int(np.searchsorted(xs, x, 'right')) - 1, len(xs) - 2)
    return i, (x - xs[i]) / (xs[i + 1] - xs[i])


def profiles_at(t, ri, blend, balance):
    if ri == 3:
        return t['global']
    g = t['grid'][ri]
    i, u = _interp(BLENDS, blend)
    j, v = _interp(BALANCES, balance)
    a = g[i, j] * (1 - u) * (1 - v) + g[i + 1, j] * u * (1 - v) + g[i, j + 1] * (1 - u) * v + g[i + 1, j + 1] * u * v
    return np.c_[a, np.zeros(N)]


def coef_at(t, ri, hue, sat):
    f = (hue % 360) / 30
    i, h = int(f) % 12, f - int(f)
    c = t['coef'][ri, i] * (1 - h) + t['coef'][ri, (i + 1) % 12] * h
    c = np.concatenate([np.zeros((1, 3, 3)), c])
    j, s = _interp([0] + SATURATIONS, sat)
    return c[j] * (1 - s) + c[j + 1] * s


def weight(t, ri, blend, balance):
    """The region's first profile, normalized to its peak."""
    a = profiles_at(t, ri, blend, balance)[:, 0]
    a = a * np.sign(a[np.abs(a).argmax()])
    return a / a.max()


def lum_gain(t, ri, amount, blend, balance):
    xs = [-100, -75, -50, -25, 0, 25, 50, 75, 100]
    tables = list(np.log2(t['lum'][ri, :4])) + [np.zeros(N)] + list(np.log2(t['lum'][ri, 4:]))
    i, u = _interp(xs, amount)
    log = tables[i] * (1 - u) + tables[i + 1] * u
    if ri == 3 or (blend, balance) == (50, 0):
        return 2 ** log
    # The same strength where the region's weight is the same, on the same side of its peak.
    w, w0 = weight(t, ri, blend, balance), weight(t, ri, 50, 0)
    p, p0 = int(w.argmax()), int(w0.argmax())
    at = np.zeros(N)
    for j in range(N):
        seg = np.arange(0, p0 + 1) if j <= p else np.arange(p0, N)
        ws = w0[seg]
        k = int(np.argmin(np.abs(ws - w[j])))
        at[j] = seg[k]
        for a, b in ((k - 1, k), (k, k + 1)):
            if 0 <= a and b < len(seg) and (ws[a] - w[j]) * (ws[b] - w[j]) <= 0 and ws[a] != ws[b]:
                at[j] = seg[a] + (w[j] - ws[a]) / (ws[b] - ws[a]) * (seg[b] - seg[a])
                break
    if amount < 0:
        return 2 ** np.interp(at, np.arange(N), log)
    # Lifts keep their offset in encoded values.
    offset = np.interp(at, np.arange(N), E * (2 ** (log / 2.2) - 1))
    return np.where(E > 0, ((E + offset) / np.maximum(E, 1e-9)) ** 2.2, 2 ** log[0])


def operator_gains(t, regions, blend, balance):
    """Per-channel gains at E for {region: (hue, saturation, luminance)} (0-360, 0-100, ±100)."""
    lum = np.ones(N)
    tint = np.ones((N, 3))
    for r, (h, s, l) in regions.items():
        ri = 'SMHG'.index(r)
        if l:
            lum *= lum_gain(t, ri, l, blend, balance)
        if s:
            tint *= 1 + profiles_at(t, ri, blend, balance) @ coef_at(t, ri, h, s).T
    # Luminance first, then the tint on its result.
    after = np.clip(E ** 2.2 * lum, 0, 1) ** (1 / 2.2)
    return lum[:, None] * np.stack([np.interp(after, E, tint[:, c]) for c in range(3)], 1)


def de00(lab1, lab2):
    L1, a1, b1 = lab1.T
    L2, a2, b2 = lab2.T
    C1, C2 = np.hypot(a1, b1), np.hypot(a2, b2)
    G = 0.5 * (1 - np.sqrt(((C1 + C2) / 2) ** 7 / (((C1 + C2) / 2) ** 7 + 25 ** 7)))
    a1p, a2p = (1 + G) * a1, (1 + G) * a2
    C1p, C2p = np.hypot(a1p, b1), np.hypot(a2p, b2)
    h1, h2 = np.degrees(np.arctan2(b1, a1p)) % 360, np.degrees(np.arctan2(b2, a2p)) % 360
    dh = np.where(h2 - h1 > 180, h2 - h1 - 360, np.where(h2 - h1 < -180, h2 - h1 + 360, h2 - h1))
    dH = 2 * np.sqrt(C1p * C2p) * np.sin(np.radians(dh / 2))
    Lb, Cb = (L1 + L2) / 2, (C1p + C2p) / 2
    hb = np.where(np.abs(h1 - h2) > 180, (h1 + h2 + 360) / 2, (h1 + h2) / 2)
    T = (1 - 0.17 * np.cos(np.radians(hb - 30)) + 0.24 * np.cos(np.radians(2 * hb))
         + 0.32 * np.cos(np.radians(3 * hb + 6)) - 0.20 * np.cos(np.radians(4 * hb - 63)))
    Sl = 1 + 0.015 * (Lb - 50) ** 2 / np.sqrt(20 + (Lb - 50) ** 2)
    Rt = -np.sin(np.radians(60 * np.exp(-((hb - 275) / 25) ** 2))) * 2 * np.sqrt(Cb ** 7 / (Cb ** 7 + 25 ** 7))
    dC = (C2p - C1p) / (1 + 0.045 * Cb)
    dHs = dH / (1 + 0.015 * Cb * T)
    return np.sqrt(((L2 - L1) / Sl) ** 2 + dC ** 2 + dHs ** 2 + Rt * dC * dHs)


PRO_TO_XYZ = np.array([[0.7976749, 0.1351917, 0.0313534], [0.2880402, 0.7118741, 0.0000857], [0.0, 0.0, 0.8252100]])


def lab(pro):
    xyz = pro @ PRO_TO_XYZ.T / np.array([0.96422, 1.0, 0.82521])
    f = np.where(xyz > (6 / 29) ** 3, np.cbrt(np.maximum(xyz, 0)), xyz / (3 * (6 / 29) ** 2) + 4 / 29)
    return np.stack([116 * f[:, 1] - 16, 500 * (f[:, 0] - f[:, 1]), 200 * (f[:, 1] - f[:, 2])], 1)


def report(t, data):
    layout = json.loads((CORPUS / 'charts/layout.json').read_text())['patches']
    keep = np.array([p['group'] != 'wide' for p in layout])
    base = linear(data['default'])[keep]
    defs = cases()
    groups = {}
    for name, s in defs.items():
        if name == 'default':
            continue
        regions = {}
        for r, (kh, ks, kl) in KEYS.items():
            h, sat, l = float(s.get(kh, 0)), float(s.get(ks, 0)), float(s.get(kl, 0))
            if sat or l:
                regions[r] = (h, sat, l)
        g = operator_gains(t, regions, float(s['ColorGradeBlending']), float(s['SplitToningBalance']))
        e = np.clip(base, 0, 1) ** (1 / 2.2)
        pred = np.stack([base[:, c] * np.interp(e[:, c], E, g[:, c]) for c in range(3)], 1)
        err = de00(lab(linear(data[name])[keep]), lab(pred)).mean()
        group = 'check' if name.startswith('check') else ('luminance' if '-lum' in name else
                                                          ('blending/balance' if 'blend' in name else 'hue/saturation'))
        groups.setdefault(group, []).append((err, name))
    for g, v in groups.items():
        v.sort()
        print(f'{g}: {len(v)} renders, mean ΔE00 {np.mean([a for a, _ in v]):.2f}, worst '
              + ', '.join(f'{n} {a:.2f}' for a, n in v[-3:]))
    for a, n in sorted(groups.get('check', []), key=lambda x: x[1]):
        print(f'  {n}: {a:.2f}')


def fit(refs):
    data = json.loads(refs.read_text())
    t, _ = fit_tables(data)
    out = np.concatenate([t[k].astype('<f4').ravel() for k in ('grid', 'global', 'coef', 'lum')])
    path = ROOT / 'crates/rawmakase-engine/src/develop/color_grade_curves.bin'
    path.write_bytes(out.tobytes())
    print(f'{path.relative_to(ROOT)}: {out.size} values')
    report(t, data)


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('step', choices=['render', 'fit'])
    p.add_argument('--refs', type=Path, default=Path(tempfile.gettempdir()) / 'color-grading-refs.json',
                   help='patch means of the renders (kept outside the repository)')
    args = p.parse_args()
    render(args.refs) if args.step == 'render' else fit(args.refs)


if __name__ == '__main__':
    main()
