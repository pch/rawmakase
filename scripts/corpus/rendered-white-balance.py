"""Measure a rendered file's Temp/Tint mapping (model/white_balance.rs:
RENDERED_TEMPERATURE and rendered_dng_tint) from Camera Raw 18.7 renders of synthetic
JPEG charts.
Usage: rendered-white-balance.py GAINS.json [GAINS.json ...]
Each GAINS.json maps a file name ending in `__temp+25.jpg`, `__tint-50.jpg` or
`__temp+25_tint-50.jpg` to its [r, g, b] ProPhoto log2 gains: Lightroom's export over
the file, each through F⁻¹ (develop::rendered::scene_from_rendered), averaged over the
chart's greys whose export channels are all between 0.002 and 0.5 (brighter greys reach
Lightroom's highlight compression). The renders themselves are not part of the
repository.

Lightroom treats the pseudo-scene as a camera whose RGB is linear ProPhoto, as the DNG
SDK would: the gains are the file's D50 white Bradford-adapted from the white Temp and
Tint name to D50, scaled by that white's largest ProPhoto channel, with camera white
floored at 0.001 and per-channel WB gain capped at 16× (`wb_scale = clip(cw * 16, 0, 1)`
on each column of the matrix). Where the gain spread is extreme (Temp −100…−75 or
Tint +100), inverting all three channels through F⁻¹ on dark greys picks up rgb_tone
cross-channel bias; unbiased endpoints are measured from the rgb_tone-invariant
`(mid - lo) / (hi - lo)` ratio and the 16× cool-magenta ramp, giving 500 mired (2000 K)
with zero tint drift at Temp −100 and the quadratic curve
`t = 0.5 * u * (1 + |u|), dng_tint = 9.6 * (1 - t^2) + 150 * t` for Tint −100…+100."""
import json
import re
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
MODEL = ROOT / 'crates/rawmakase-model/src'


def rust_rows(text):
    """The rows of a Rust array of float arrays, as lists of floats."""
    return [[float(v) for v in row.split(',') if v.strip()]
            for row in re.findall(r'\[([-\d., ]+)\]', text)]


def rust_matrix(path, name):
    """A `const NAME: ... = [[...], ...];` 3×3 matrix from the Rust sources."""
    src = (MODEL / path).read_text()
    body = src.split(f'const {name}:')[1].split('= [', 1)[1].split('];')[0]
    return np.array(rust_rows(body))


T = np.array(rust_rows((MODEL / 'camera_profiles/temperature.rs').read_text()
                       .split('const TABLE')[1].split('= [', 1)[1].split('];')[0]))
XYZ_TO_PRO = rust_matrix('camera_profiles/mod.rs', 'XYZ_TO_PRO')
BRADFORD = rust_matrix('model/white_balance.rs', 'BRADFORD')
RUST_TEMP = np.array(rust_rows((MODEL / 'model/white_balance.rs').read_text()
                               .split('const RENDERED_TEMPERATURE:')[1]
                               .split('= [', 1)[1].split('];')[0]))
D50 = np.linalg.solve(XYZ_TO_PRO, np.ones(3))


def xy(mired, tint):  # temperature::xy_reciprocal
    i = next((i for i in range(30) if mired < T[i + 1][0]), 29)
    a, b = T[i], T[i + 1]
    f = (b[0] - mired) / (b[0] - a[0])

    def direction(slope):
        return np.array([1, slope]) / np.sqrt(1 + slope * slope)
    du, dv = direction(a[3]) * f + direction(b[3]) * (1 - f)
    length = np.hypot(du, dv)
    off = tint / -3000.
    u = a[1] * f + b[1] * (1 - f) + off * du / length
    v = a[2] * f + b[2] * (1 - f) + off * dv / length
    den = u - 4 * v + 2
    return np.array([1.5 * u / den, v / den])


def white_xyz(mired, tint):
    x, y = xy(mired, tint)
    return np.array([x / y, 1, (1 - x - y) / y])


def rendered_dng_tint(n):
    u = np.clip(n / 100., -1., 1.)
    t = 0.5 * u * (1. + abs(u))
    return 9.6 * (1. - t * t) + 150. * t


def map_white(fr, to):
    f, t = BRADFORD @ fr, BRADFORD @ to
    sc = [np.clip(max(t[c], 0.) / max(f[c], 0.) if max(f[c], 0.) > 0 else 10., 0.1, 10.)
          for c in range(3)]
    return np.linalg.solve(BRADFORD, np.diag(sc) @ BRADFORD)


def gains(white):  # log2, matching model::white_balance::camera_to_prophoto
    to_cam = XYZ_TO_PRO @ map_white(D50, white)
    to_cam = to_cam / np.max(to_cam @ D50)
    cw = XYZ_TO_PRO @ white
    cw = np.clip(cw / np.max(cw), 0.001, 1.)
    wb_scale = np.clip(cw * 16., 0., 1.)
    m = (XYZ_TO_PRO @ np.linalg.inv(to_cam)) @ np.diag(wb_scale)
    # Deep in the cool magenta corner the 16× cap leaves grey blue <= 0 (log2 NaN).
    with np.errstate(invalid='ignore'):
        return np.log2(m @ np.ones(3))


def white_of(g):  # the white whose gains have the shape of g
    cone = BRADFORD @ np.linalg.solve(XYZ_TO_PRO, 2. ** np.asarray(g))
    return np.linalg.solve(BRADFORD, (BRADFORD @ D50) ** 2 / cone)


def mired_tint(white):  # nearest DNG reciprocal temperature and tint
    target = white[:2] / white.sum()

    def err(p):
        return np.sum((xy(*p) - target) ** 2)
    grid = ((m, t) for m in np.arange(10, 600, 2.) for t in np.arange(-200, 200, 2.))
    p, step = np.array(min(grid, key=err), float), np.array([1., 1.])
    while step.max() > 1e-3:
        moved = False
        for i in range(2):
            for s in (-1, 1):
                q = p.copy()
                q[i] += s * step[i]
                if err(q) < err(p):
                    p, moved = q, True
        if not moved:
            step /= 2
    return p


def setting(name, kind):
    m = re.search(rf'{kind}([-+]\d+)', name)
    return int(m.group(1)) if m else 0


files = {}
for fn in sys.argv[1:]:
    with open(fn) as f:
        files.update(json.load(f))
measured = {k: (setting(k, 'temp'), setting(k, 'tint'), mired_tint(white_of(g)))
            for k, g in files.items()}
d50_mired, d50_tint = mired_tint(D50)
temp, tint = {0: [(d50_mired, d50_tint)]}, {0: [d50_tint]}
for s, n, (m, t) in measured.values():
    if n == 0 and s:
        temp.setdefault(s, []).append((m, t))
    if s == 0 and n:
        tint.setdefault(n, []).append(t)
temp = {s: (np.mean([m for m, _ in v]), np.mean([t for _, t in v]) - d50_tint)
        for s, v in temp.items()}
tint = {n: np.mean(v) for n, v in tint.items()}
print(f'D50: mired {d50_mired:.2f}, DNG tint {d50_tint:.2f}')
print('RENDERED_TEMPERATURE [Temp, mired]')
for s in sorted(temp):
    print(f'    [{s}., {temp[s][0]:.1f}],')
print('Measured Tint vs quadratic rendered_dng_tint(n)')
for n in sorted(tint):
    print(f'    Tint {n:+4d}: measured {tint[n]:+7.2f}  formula {rendered_dng_tint(n):+7.2f}')


def table_gains(s, n):
    """Evaluate the Rust RENDERED_TEMPERATURE table + rendered_dng_tint at (s, n)."""
    m = np.interp(s, RUST_TEMP[:, 0], RUST_TEMP[:, 1])
    return gains(white_xyz(m, rendered_dng_tint(n))) - gains(white_xyz(d50_mired, d50_tint))


print('Rust RENDERED_TEMPERATURE + rendered_dng_tint against each file (largest |log2| error per channel)')
for k, (s, n, (m, t)) in sorted(measured.items(), key=lambda kv: kv[1][:2]):
    error = np.abs(table_gains(s, n) - np.array(files[k])).max()
    print(f'  {k:36s} mired {m:6.1f} tint {t:+7.2f}  {error:.3f}')
