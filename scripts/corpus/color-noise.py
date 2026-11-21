#!/usr/bin/env python3
"""Measure Camera Raw's Color noise reduction on photos, whose noise it knows.

`charts` writes synthetic RGGB DNGs into DIR: neutral flats at eight levels, color
flats and a red/cyan edge, with shot and read noise of a Nikon D7100 at ISO 2000
(variance 4.88e-4 x + 5.6e-7 of the white level, measured on the photo of issue 465)
scaled by 1/16, 1/4, 1 and 4, each carrying its DNG NoiseProfile. Camera Raw reads
the profile as it knows a camera's; without one it assumes little noise and keeps
most color noise (the charts `--no-profile` writes).

`reference` has Photoshop 2026 (Camera Raw) render every chart at Color 0, 10, 25, 50
and 100 (Detail and Smoothness 50), and at 25 and 100 with Detail or Smoothness at
0, 25, 75 and 100, into DIR/camera-raw. It refuses to start while Photoshop has
documents open.

`compare` renders the same settings with --rawmakase (a built `rawmakase` binary) and
prints, for each case, the share of the color noise of the neutral flats each keeps
at four scales (band-pass levels of 1, 2, 4 and 8 pixels), averaged over the flats
at 200 to 3200 of 16383, and the mean |log| ratio of RAWmakase's share to Camera Raw's.
The adaptive operator's tables in crates/rawmakase-engine/src/develop/color_noise.rs
were fitted to these shares.

Requires numpy and tifffile. Run from the repository root:
  python3 scripts/corpus/color-noise.py charts|reference|compare DIR [--rawmakase BIN]
"""
import argparse
import struct
import subprocess
import sys
from pathlib import Path

import numpy as np

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).parent))
import camera_raw  # noqa: E402

WHITE = 16383
SIZE, PATCH = (1024, 768), 256
GREENS = [50, 100, 200, 400, 800, 1600, 3200, 6400]
MEASURED = [2, 3, 4, 5, 6]  # the flats at 200 to 3200
AS_SHOT = np.array([2.30078125, 1.0, 1.62890625])  # camera multipliers
NOISE = (4.88e-4, 5.6e-7)  # variance S x + O at scale 1
SCALES = ['0.0625', '0.25', '1', '4']
# Nikon D7100 color matrix (XYZ to camera, D65), as dcraw lists it.
COLOR_MATRIX = [0.8322, -0.3112, -0.1047, -0.6367, 1.4342, 0.2179, -0.0988, 0.1638, 0.6394]
CASES = ([(a, 50, 50) for a in (0, 10, 25, 50, 100)]
         + [(a, d, s) for a in (25, 100)
            for d, s in [(0, 50), (25, 50), (75, 50), (100, 50), (50, 0), (50, 25), (50, 75), (50, 100)]])


def chart(scale, profile, seed=464):
    """The chart's raw values, 16-bit, at `scale` times the D7100's ISO 2000 noise."""
    w, h = SIZE
    scene = np.zeros((h, w, 3))
    for i, g in enumerate(GREENS):
        x, y = (i % 4) * PATCH, (i // 4) * PATCH
        scene[y:y + PATCH, x:x + PATCH] = g
    for i, c in enumerate([[1600, 500, 300], [400, 1200, 500], [300, 600, 1600]]):
        scene[512:768, i * PATCH:(i + 1) * PATCH] = c
    edge = (np.arange(256) < 128)[None, :, None]
    scene[512:768, 768:1024] = np.where(edge, [1600, 400, 400], [400, 1000, 1400])
    yy, xx = np.mgrid[0:h, 0:w]
    cfa = np.array([[0, 1], [1, 2]])[yy % 2, xx % 2]
    x = np.take_along_axis(scene / AS_SHOT, cfa[..., None], 2)[..., 0] / WHITE
    s, o = NOISE[0] * scale, NOISE[1] * scale
    noisy = x + np.random.default_rng(seed).normal(0, 1, x.shape) * np.sqrt(s * x + o)
    raw = np.clip(np.round(noisy * WHITE), 0, WHITE).astype('<u2')
    return dng(raw, (s, o) if profile else None)


def dng(raw, profile):
    """A minimal uncompressed little-endian CFA DNG."""
    h, w = raw.shape
    data = raw.tobytes()
    rational = lambda v, signed: [(int(round(t * 1e6)), 1000000) for t in v]  # noqa: E731
    tags = {
        254: (4, [0]), 256: (4, [w]), 257: (4, [h]), 258: (3, [16]), 259: (3, [1]),
        262: (3, [32803]), 271: (2, 'Synthetic'), 272: (2, 'Noise'), 273: (4, [0]),
        274: (3, [1]), 277: (3, [1]), 278: (4, [h]), 279: (4, [len(data)]), 284: (3, [1]),
        33421: (3, [2, 2]), 33422: (1, [0, 1, 1, 2]), 50706: (1, [1, 4, 0, 0]),
        50707: (1, [1, 1, 0, 0]), 50708: (2, 'Synthetic Noise'), 50710: (1, [0, 1, 2]),
        50711: (3, [1]), 50713: (3, [1, 1]), 50714: (4, [0]), 50717: (4, [WHITE]),
        50721: (10, rational(COLOR_MATRIX, True)), 50728: (5, rational(1 / AS_SHOT, False)),
        50778: (3, [21]),
    }
    if profile:
        tags[51041] = (12, list(profile))
    formats = {1: 'B', 3: 'H', 4: 'I', 12: 'd'}
    extra_at = 8 + 2 + len(tags) * 12 + 4
    entries, extra = b'', b''
    for tag in sorted(tags):
        kind, value = tags[tag]
        if kind == 2:
            body = value.encode() + b'\0'
        elif kind in (5, 10):
            body = b''.join(struct.pack('<ii' if kind == 10 else '<II', n, d) for n, d in value)
        else:
            body = struct.pack('<' + formats[kind] * len(value), *value)
        count = len(body) if kind == 2 else len(value)
        if tag == 273:
            entries += struct.pack('<HHI', tag, kind, count) + b'STRP'
        elif len(body) <= 4:
            entries += struct.pack('<HHI', tag, kind, count) + body.ljust(4, b'\0')
        else:
            extra += b'\0' * (len(extra) % 2)
            entries += struct.pack('<HHII', tag, kind, count, extra_at + len(extra))
            extra += body
    extra += b'\0' * (len(extra) % 2)
    entries = entries.replace(b'STRP', struct.pack('<I', extra_at + len(extra)))
    return b'II*\0' + struct.pack('<IH', 8, len(tags)) + entries + b'\0' * 4 + extra + data


def name(scale):
    return f'noise-{scale}.dng'


def settings(amount, detail, smoothness):
    return {'ColorNoiseReduction': str(amount), 'ColorNoiseReductionDetail': str(detail),
            'ColorNoiseReductionSmoothness': str(smoothness)}


def out(folder, scale, case):
    return folder / f'noise-{scale}-a{case[0]}-d{case[1]}-s{case[2]}.tif'


def linear_srgb(path):
    """A render as linear sRGB: Camera Raw's ProPhoto (gamma 1.8) or RAWmakase's sRGB."""
    import tifffile
    a = tifffile.imread(str(path)).astype(np.float64)[..., :3] / 65535.
    if 'camera-raw' in path.parts:
        pro_to_xyz = np.array([[0.7977, 0.1352, 0.0313], [0.2880, 0.7119, 0.0001], [0, 0, 0.8249]])
        xyz_to_srgb = np.array([[3.1339, -1.6169, -0.4906], [-0.9788, 1.9161, 0.0335],
                                [0.0719, -0.2290, 1.4052]])  # D50
        return a ** 1.8 @ (xyz_to_srgb @ pro_to_xyz).T
    return np.where(a <= 0.04045, a / 12.92, ((a + 0.055) / 1.055) ** 2.4)


def blur(x):
    """Gaussian blur (σ √3) of a 2-D array, mirrored at the edges."""
    taps = np.exp(-0.5 * (np.arange(-6, 7) / np.sqrt(3)) ** 2)
    taps /= taps.sum()
    x = np.pad(x, 6, mode='reflect')
    x = sum(t * x[:, i:i + x.shape[1] - 12] for i, t in enumerate(taps))
    return sum(t * x[i:i + x.shape[0] - 12] for i, t in enumerate(taps))


def bands(x):
    """Standard deviation of four band-pass levels, finest first."""
    out = []
    for _ in range(4):
        low = blur(x)
        out.append((x - low)[8:-8, 8:-8].std())
        x = low[::2, ::2]
    return np.array(out)


def chroma_bands(image, patch):
    x, y = (patch % 4) * PATCH, (patch // 4) * PATCH
    q = image[y + 32:y + PATCH - 32, x + 32:x + PATCH - 32]
    luma = q @ [0.2126, 0.7152, 0.0722]
    m = luma.mean()
    return np.hypot(bands((q[..., 0] - q[..., 1]) / m), bands((q[..., 2] - q[..., 1]) / m))


def kept(render, before):
    """Log of the share of each band's color noise kept, averaged over the flats."""
    a, b = linear_srgb(render), linear_srgb(before)
    return np.mean([np.log(chroma_bands(a, p) / chroma_bands(b, p)) for p in MEASURED], axis=0)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('command', choices=['charts', 'reference', 'compare'])
    ap.add_argument('dir', type=Path)
    ap.add_argument('--rawmakase', type=Path)
    ap.add_argument('--no-profile', action='store_true')
    a = ap.parse_args()
    folder = a.dir.resolve()
    if a.command == 'charts':
        folder.mkdir(parents=True, exist_ok=True)
        for scale in SCALES:
            (folder / name(scale)).write_bytes(chart(float(scale), not a.no_profile))
    elif a.command == 'reference':
        jobs = [dict(source=str(folder / name(scale)), settings=settings(*case),
                     out=str(out(folder / 'camera-raw', scale, case)))
                for scale in SCALES for case in CASES]
        failures = camera_raw.render(jobs)
        if failures:
            sys.exit('\n'.join(failures))
    else:
        if not a.rawmakase:
            sys.exit('compare needs --rawmakase')
        mine = folder / 'rawmakase'
        mine.mkdir(exist_ok=True)
        errors = []
        for scale in SCALES:
            for case in CASES:
                xmp = mine / f'a{case[0]}-d{case[1]}-s{case[2]}.xmp'
                xmp.write_text(camera_raw.xmp(settings(*case)))
                subprocess.run([str(a.rawmakase), 'render', '--xmp', str(xmp), '--overwrite',
                                str(folder / name(scale)), str(out(mine, scale, case))],
                               check=True, capture_output=True)
            for case in CASES[1:]:
                ours = kept(out(mine, scale, case), out(mine, scale, CASES[0]))
                theirs = kept(out(folder / 'camera-raw', scale, case),
                              out(folder / 'camera-raw', scale, CASES[0]))
                errors.append(np.abs(ours - theirs))
                print(f'noise x{scale:6s} Color {case[0]:3d} Detail {case[1]:3d} Smoothness {case[2]:3d}: '
                      f'RAWmakase {np.round(np.exp(ours), 3).tolist()} Camera Raw {np.round(np.exp(theirs), 3).tolist()}')
        print('mean |log ratio| per band', np.round(np.mean(errors, axis=0), 3).tolist())


if __name__ == '__main__':
    main()
