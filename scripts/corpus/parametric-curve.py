#!/usr/bin/env python3
"""Measure Camera Raw's parametric tone curve on the synthetic chart and write
crates/rawmakase-engine/src/develop/parametric.bin.

`render` has Photoshop 2026 (Camera Raw) render synthetic-d65.dng with each fitting
case (about 380 settings of the Shadows, Darks, Lights and Highlights regions and
their splits) and keeps the mean of every chart patch in a JSON file outside the
repository (--refs, default parametric-refs.json in a temporary folder); the TIFFs
are deleted as they are read. It refuses to start while Photoshop has documents open.

`fit` turns those patch means into the curve tables RAWmakase renders with. Camera Raw
applies the parametric curve DNG RGBTone fashion, so the brightest and darkest channel
of every patch (in ProPhoto RGB) are two samples of the curve; gray-ramp patches
clipped at white or black say where the curve reaches 1 or 0. The curve is fitted in
gamma-2.2 encoded ProPhoto RGB (eight times closer than the sRGB transfer function)
as a cubic Hermite spline with 17 knots, and sampled at 65 points.

What the renders show (see docs/tone-controls.md#parametric-curve):
- Shadows then Darks, and Highlights then Lights, compose exactly; Shadows acts on
  0 to the midtone split, Highlights from it to 1.
- Shadows depends on the shadow and midtone splits only, scaling with the midtone
  split; Highlights likewise on the midtone and highlight splits. Darks and Lights
  depend on the midtone split only.
- Darks and Lights together are not the sum of each alone, so they are measured
  together on a grid.

Tables (f32, little endian, each a change to the identity at 65 points over 0-1):
  LOW   8 amounts x 5 ratios   Shadows over 0..midtone, normalized, by shadow/midtone
  HIGH  8 amounts x 5 ratios   Highlights over midtone..1, normalized
  DARKS 8 amounts x 7 midtone splits
  LIGHTS 8 amounts x 7 midtone splits
  JOINT 6 x 6                  Darks and Lights together at the default splits

Requires numpy. Run from the repository root:
  python3 scripts/corpus/parametric-curve.py render [--refs FILE]
  python3 scripts/corpus/parametric-curve.py fit [--refs FILE]
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

AMOUNTS = [-100, -75, -50, -25, 25, 50, 75, 100]
SHADOW_SPLITS = [10, 18, 25, 32, 40]      # with the midtone split at 50: ratios .2-.8
HIGHLIGHT_SPLITS = [60, 68, 75, 82, 90]
MIDTONE_SPLITS = [20, 30, 40, 50, 60, 70, 80]
JOINT = [-100, -50, -25, 25, 50, 100]
REGIONS = ['Shadows', 'Darks', 'Lights', 'Highlights']
SAMPLES = 65
GAMMA = 2.2


def case(name, values, splits=(25, 50, 75)):
    settings = {f'Parametric{r}': f'{v:+d}' if v else '0' for r, v in zip(REGIONS, values)}
    settings.update(ParametricShadowSplit=str(splits[0]), ParametricMidtoneSplit=str(splits[1]),
                    ParametricHighlightSplit=str(splits[2]))
    return {'name': name, 'settings': settings}


def cases():
    out = [{'name': 'default', 'settings': {}}]
    for a in AMOUNTS:
        out += [case(f'S{a:+d}-r{s}', [a, 0, 0, 0], (s, 50, 75)) for s in SHADOW_SPLITS]
        out += [case(f'H{a:+d}-r{s}', [0, 0, 0, a], (25, 50, s)) for s in HIGHLIGHT_SPLITS]
        for m in MIDTONE_SPLITS:
            splits = (m // 2, m, (100 + m) // 2)
            out += [case(f'D{a:+d}-m{m}', [0, a, 0, 0], splits), case(f'L{a:+d}-m{m}', [0, 0, a, 0], splits)]
    out += [case(f'DL{d:+d}{l:+d}', [0, d, l, 0]) for d in JOINT for l in JOINT]
    return out


def render(refs):
    done = json.loads(refs.read_text()) if refs.exists() else {}
    todo = [c for c in cases() if c['name'] not in done]
    if not todo:
        return
    if not charts.photoshop_idle():
        sys.exit(f'{charts.PHOTOSHOP} is not running or has documents open; close them (another session may be using it).')
    base = json.loads((CORPUS / 'cases.json').read_text())['base']
    layout = json.loads((CORPUS / 'charts/layout.json').read_text())
    work = Path(tempfile.mkdtemp(prefix='parametric-'))
    source = CORPUS / 'charts/synthetic-d65.dng'
    jobs = [{'source': str(source), 'dng': str(work / 'chart.dng'), 'xmp': str(work / 'chart.xmp'),
             'settings': charts.xmp(base, c, {}), 'out': str(work / 'out' / f"{c['name']}.tif")} for c in todo]
    script = work / 'render.jsx'
    script.write_text(charts.TEMPLATE % {'jobs': json.dumps(jobs)})
    print(f'Rendering {len(jobs)} cases in {work}', flush=True)
    subprocess.run(['osascript', '-e', f'with timeout of 36000 seconds\ntell application "{charts.PHOTOSHOP}" '
                    f'to do javascript file (POSIX file "{script}")\nend timeout'], check=True, stdout=subprocess.DEVNULL)
    for c in todo:
        tiff = work / 'out' / f"{c['name']}.tif"
        if tiff.exists():
            done[c['name']] = charts.patch_values(tiff, layout['patches'])
        else:
            print(f"  missing render: {c['name']}", file=sys.stderr)
    shutil.rmtree(work)
    refs.write_text(json.dumps(done))


# sRGB (D65) to ProPhoto (D50, Bradford), linear.
SRGB_TO_XYZ = np.array([[0.4124564, 0.3575761, 0.1804375], [0.2126729, 0.7151522, 0.0721750],
                        [0.0193339, 0.1191920, 0.9503041]])
BRADFORD = np.array([[1.0478112, 0.0228866, -0.0501270], [0.0295424, 0.9904844, -0.0170491],
                     [-0.0092345, 0.0150436, 0.7521316]])
PRO_TO_XYZ = np.array([[0.7976749, 0.1351917, 0.0313534], [0.2880402, 0.7118741, 0.0000857],
                       [0.0, 0.0, 0.8252100]])
SRGB_TO_PRO = np.linalg.inv(PRO_TO_XYZ) @ BRADFORD @ SRGB_TO_XYZ


def decode(v):
    return np.where(v <= 0.04045, v / 12.92, ((v + 0.055) / 1.055) ** 2.4)


def gamma(rgb16):
    """Encoded sRGB (16-bit) to gamma-2.2 encoded ProPhoto RGB."""
    return np.clip(decode(np.asarray(rgb16, float) / 65535) @ SRGB_TO_PRO.T, 0, 1) ** (1 / GAMMA)


def hermite_basis(knots, x):
    """Columns: the change from the identity for each knot value (interior) and slope."""
    i = np.clip(np.searchsorted(knots, x) - 1, 0, len(knots) - 2)
    h = knots[i + 1] - knots[i]
    t = (x - knots[i]) / h
    h00, h10, h01, h11 = 2 * t**3 - 3 * t**2 + 1, t**3 - 2 * t**2 + t, -2 * t**3 + 3 * t**2, t**3 - t**2
    cols = []
    for j in range(1, len(knots) - 1):
        cols.append(np.where(i == j, h00, 0) + np.where(i + 1 == j, h01, 0))
    for j in range(len(knots)):
        cols.append(np.where(i == j, h10 * h, 0) + np.where(i + 1 == j, h11 * h, 0))
    return np.array(cols).T


KNOTS = np.linspace(0, 1, 17)
GRID = np.linspace(0, 1, SAMPLES)


def fit_curve(default, rendered, layout):
    """The case's curve as changes to the identity at GRID, in gamma-2.2 encoding."""
    groups = np.array([p['group'] for p in layout])
    d, c = np.asarray(default, float), np.asarray(rendered, float)
    ok = (groups != 'wide') & (c.min(1) > 1) & (c.max(1) < 65534) & (d.min(1) > 1) & (d.max(1) < 65534)
    pd, pc = gamma(d[ok]), gamma(c[ok])
    # Neutral ramp patches sample the 1D curve directly, while saturated patches carry a
    # small secant offset in Camera Raw; weight ramp patches higher and taper by chroma.
    is_ramp = groups[ok] == 'ramp'
    chroma = pd.max(1) - pd.min(1)
    base_w = np.where(is_ramp, 4.0, 1.0 / (1.0 + (chroma / 0.15) ** 2))
    x = np.r_[pd.max(1), pd.min(1)]
    y = np.r_[pc.max(1), pc.min(1)]
    bw = np.r_[base_w, base_w]

    ramp = groups == 'ramp'
    level = c[ramp, 1]
    # Only truly clipped 16-bit ramp patches (near 0 or 65535) constrain the clipped ends;
    # dim non-zero sRGB values (e.g. 12–30) are still ~0.006–0.010 in gamma-2.2.
    clipped = (level >= 65533) | (level <= 2)
    cx = gamma(d[ramp][clipped])[:, 1]
    cy = np.where(level[clipped] > 32768, 1., 0.)

    a0, ac = hermite_basis(KNOTS, x), hermite_basis(KNOTS, cx)
    ag = hermite_basis(KNOTS, GRID)
    # Regularize curvature across GRID rather than pulling toward the identity, so sparse
    # clipped shadow/highlight spans do not bulge back toward y = x.
    d2 = np.diff(ag, n=2, axis=0) * 0.5
    ridge = np.sqrt(1e-5) * np.eye(a0.shape[1])

    weight = bw.copy()
    active = np.zeros(len(cx), bool)
    for _ in range(8):
        a = np.r_[a0 * weight[:, None], ac[active] * 6, d2, ridge]
        b = np.r_[(y - x) * weight, (cy - cx)[active] * 6, np.zeros(len(d2)), np.zeros(a0.shape[1])]
        p = np.linalg.lstsq(a, b, rcond=None)[0]
        residual = np.abs(a0 @ p - (y - x))
        # Huber reweighting with a floor prevents an untouched half of the curve (residual ~0)
        # from collapsing the scale and dropping valid samples in the active region.
        delta = max(float(np.median(residual)) * 2.0, 0.003)
        huber = np.where(residual <= delta, 1.0, np.sqrt(delta / residual))
        weight = bw * huber
        reached = cx + ac @ p
        active |= ((cy == 1) & (reached < 1)) | ((cy == 0) & (reached > 0))
    return np.clip(GRID + ag @ p, 0, 1)


def fit(refs):
    data = json.loads(refs.read_text())
    layout = json.loads((CORPUS / 'charts/layout.json').read_text())['patches']
    default = data['default']

    def curve(name):
        return fit_curve(default, data[name], layout)

    def low(name, midtone):  # Shadows over 0..midtone, normalized
        f = curve(name)
        return np.interp(GRID * midtone, GRID, f) / midtone - GRID

    def high(name, midtone):
        f = curve(name)
        return (np.interp(midtone + GRID * (1 - midtone), GRID, f) - midtone) / (1 - midtone) - GRID

    tables = [
        [[low(f'S{a:+d}-r{s}', 0.5) for s in SHADOW_SPLITS] for a in AMOUNTS],
        [[high(f'H{a:+d}-r{s}', 0.5) for s in HIGHLIGHT_SPLITS] for a in AMOUNTS],
        [[curve(f'D{a:+d}-m{m}') - GRID for m in MIDTONE_SPLITS] for a in AMOUNTS],
        [[curve(f'L{a:+d}-m{m}') - GRID for m in MIDTONE_SPLITS] for a in AMOUNTS],
        [[curve(f'DL{d:+d}{l:+d}') - GRID for l in JOINT] for d in JOINT],
    ]
    # Every table keeps its ends: 0 and 1 overall, and the midtone split for Shadows
    # and Highlights, so the regions join without a step. The fits miss them by at
    # most about 0.01, which a correction fading out over the outer 15% removes.
    def fade(v):
        v = np.clip(v, 0, 1)
        return v * v * (3 - 2 * v)
    tables = [np.asarray(t) for t in tables]
    tables = [t - t[..., :1] * fade((0.15 - GRID) / 0.15) - t[..., -1:] * fade((GRID - 0.85) / 0.15)
              for t in tables]
    out = np.concatenate([t.astype('<f4').ravel() for t in tables])
    path = ROOT / 'crates/rawmakase-engine/src/develop/parametric.bin'
    path.write_bytes(out.tobytes())
    print(f'{path.relative_to(ROOT)}: {out.size} values')


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('step', choices=['render', 'fit'])
    p.add_argument('--refs', type=Path, default=Path(tempfile.gettempdir()) / 'parametric-refs.json',
                   help='patch means of the renders (kept outside the repository)')
    args = p.parse_args()
    if args.step == 'render':
        render(args.refs)
    else:
        fit(args.refs)


if __name__ == '__main__':
    main()
