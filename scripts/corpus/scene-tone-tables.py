#!/usr/bin/env python3
"""Measure the scene tone stage's global curves in Camera Raw and write their tables.

  render  DIR   writes neutral-ramp probe DNGs (scripts/corpus/probe_dng.py) into DIR
                and renders them in Camera Raw (Photoshop 2026; camera_raw.py)
  tables  DIR   reads the renders and writes white.bin, white3.bin, blacks.bin,
                default_black.bin and masks.bin in
                crates/rawmakase-engine/src/develop/scene_tone

Probes: a ramp of 192 neutral patches from 2^-14 to 1 of the sensor's white, with a
linear profile tone curve, so Camera Raw's ProPhoto output is its scene tone stage's
(docs/scene-tone-stage.md#comparison-contract).

- White point and Whites: the ramp at a sensor white W* of 2^-1 to 2^5 in quarter
  stops (BaselineExposure), each at Whites −100 to +100 in steps of 12.5. The image
  reaches the sensor's white, so W* is the white point. Table: scene value (log2, -16
  to 6 in eighth stops) to output, per W* and Whites.
- White point, Whites and the photo's maximum (white3.bin): the ramp up to a maximum M,
  with the sensor's white at 2^-3 to 2^4 in half stops and M 0 to 5 stops below it in
  half stops, each at Whites −100 to +100 in steps of 12.5. Positive Whites stretches
  toward the photo's maximum, not only the white point, so it needs both. Table: output
  per sensor white, stops below it and Whites, at log2(x / M) from −14 to 0 in eighth
  stops, as 16-bit values.
- Blacks: the ramp at a sensor white of 4, starting at a darkest level ("black key")
  of 2^-8, 2^-6.5 … 2^-2 (the background is at that level too), at Blacks −100 to +100
  in steps of 12.5. Blacks applies after the white point and Whites, to their output,
  so the table maps the default render's output (log2, -16 to 0 in eighth stops) to
  the Blacks render's, per black key and Blacks.
- Default black (default_black.bin): Camera Raw's default black adapts to the photo's
  darkest level. The white ramps (darkest patch 2^-14 of their maximum) show almost
  none; the black ramps' default renders a much stronger one that depends on the key.
  The table maps the white ramp's default output at a sensor white of 4 (what the
  white curves give) to the black ramp's default output at the same scene value
  (log2, −16 to 0 in eighth stops), per black key, made monotone. Below the darkest
  patch it is 0: undefined, as no pixel of a photo with that key is darker.
- Masks (masks.bin): the ramp at a sensor white of 1 and 4 with a mask covering the
  whole frame, at its Whites and then its Blacks −100 to +100 in steps of 25. A mask's
  Whites and Blacks apply after the global curves, as a fixed function of their
  output, so the table maps the default render's output (log2, −16 to 0 in eighth
  stops) to the mask's, the same at every white point.

The white and black tables include Camera Raw's black point (the DNG exposure ramp's toe), which does
not change with the sensor's white.
"""
import argparse
import sys
from pathlib import Path

import numpy as np

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).parent))
import camera_raw  # noqa: E402
import probe_dng  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / 'crates/rawmakase-engine/src/develop/scene_tone'
REL = 2.0 ** np.linspace(-14, 0, 192)
WHITE_POINTS = np.arange(-1, 5.001, 0.25)            # log2 W*
SLIDER = np.arange(-100, 100.001, 12.5)
BLACK_KEYS = np.array([-8.] + list(np.arange(-6.5, -1.999, 0.5)))
T = np.arange(-16, 6.0001, 0.125)                     # log2 scene value
SENSOR = np.arange(-3, 4.001, 0.5)                    # log2 sensor white (white3)
BELOW = np.arange(0, 5.001, 0.5)                      # stops the maximum is below it
U = np.arange(-14, 0.0001, 0.125)                     # log2(x / M)
Y = np.arange(-16, 0.0001, 0.125)                     # log2 output
MASK_WHITES = [0, 2]                                  # log2 sensor whites (masks)
MASK_VALUES = [-1, -0.75, -0.5, -0.25, 0.25, 0.5, 0.75, 1]


class Grid:
    cols, patch, gap, margin = 24, 28, 8, 32

    def __init__(self, values):
        self.values = values
        rows = (len(values) + self.cols - 1) // self.cols
        self.w = 2 * self.margin + self.cols * self.patch + (self.cols - 1) * self.gap
        self.h = 2 * self.margin + rows * self.patch + (rows - 1) * self.gap

    def pos(self, i):
        r, c = divmod(i, self.cols)
        return self.margin + r * (self.patch + self.gap), self.margin + c * (self.patch + self.gap)

    def image(self, background):
        im = np.full((self.h, self.w, 3), background)
        for i, v in enumerate(self.values):
            y, x = self.pos(i)
            im[y:y + self.patch, x:x + self.patch] = v
        return im

    def measure(self, a, inset=8):
        return np.array([a[y + inset:y + self.patch - inset, x + inset:x + self.patch - inset].mean()
                         for y, x in map(self.pos, range(len(self.values)))])


def white_ramp(lw):
    return Grid([2.0 ** lw * r for r in REL])


def black_ramp(key):
    return Grid([4. * r for r in REL if np.log2(4. * r) >= key])


def white3_ramp(ls, c):
    return Grid([2.0 ** (ls - c) * r for r in REL])


def mask_xmp(kind, v):
    """Settings with one mask over the whole frame (a gradient beyond the top edge) at
    Whites or Blacks `v` (−1 to 1)."""
    mask = ('<crs:MaskGroupBasedCorrections><rdf:Seq><rdf:li><rdf:Description crs:What="Correction" '
            f'crs:CorrectionAmount="1" crs:Local{kind}2012="{v:g}"><crs:CorrectionMasks><rdf:Seq>'
            '<rdf:li crs:What="Mask/Gradient" crs:MaskBlendMode="0" crs:ZeroX="0.5" crs:ZeroY="-2" '
            'crs:FullX="0.5" crs:FullY="-1"/></rdf:Seq></crs:CorrectionMasks></rdf:Description></rdf:li>'
            '</rdf:Seq></crs:MaskGroupBasedCorrections>')
    return camera_raw.xmp({}).replace('></rdf:Description>', '>' + mask + '</rdf:Description>', 1)


def name(v):
    return 'default' if v == 0 else f'{v:+g}'


def render(args):
    d = args.dir.resolve()
    d.mkdir(parents=True, exist_ok=True)
    jobs = []
    for lw in WHITE_POINTS:
        dng = d / f'white{lw:+.2f}.dng'
        probe_dng.write(dng, white_ramp(lw).image(0.05), baseline_exposure=lw)
        for v in SLIDER:
            jobs.append(dict(source=str(dng), settings={} if v == 0 else {'Whites2012': f'{v:g}'},
                             out=str(d / f'white{lw:+.2f}-{name(v)}.tif')))
    for ls in SENSOR:
        for c in BELOW:
            dng = d / f'white3{ls:+.1f}-{c:.1f}.dng'
            top = 2.0 ** (ls - c)
            probe_dng.write(dng, white3_ramp(ls, c).image(min(0.05, top / 8)), baseline_exposure=ls)
            for v in SLIDER:
                jobs.append(dict(source=str(dng), settings={} if v == 0 else {'Whites2012': f'{v:g}'},
                                 out=str(d / f'white3{ls:+.1f}-{c:.1f}-{name(v)}.tif')))
    for key in BLACK_KEYS:
        dng = d / f'black{key:+.1f}.dng'
        probe_dng.write(dng, black_ramp(key).image(2.0 ** key), baseline_exposure=2)
        for v in SLIDER:
            jobs.append(dict(source=str(dng), settings={} if v == 0 else {'Blacks2012': f'{v:g}'},
                             out=str(d / f'black{key:+.1f}-{name(v)}.tif')))
    for lw in MASK_WHITES:
        dng = d / f'mask{lw}.dng'
        probe_dng.write(dng, white_ramp(lw).image(0.05), baseline_exposure=lw)
        jobs.append(dict(source=str(dng), settings={}, out=str(d / f'mask{lw}-default.tif')))
        for kind in ('Whites', 'Blacks'):
            for v in MASK_VALUES:
                jobs.append(dict(source=str(dng), settings={}, xmp_text=mask_xmp(kind, v),
                                 out=str(d / f'mask{lw}-{kind}{v:+g}.tif')))
    failures = camera_raw.render(jobs, work=d / 'work')
    print(f'{len(jobs)} renders, {len(failures)} failed')


def white_table(d):
    out = np.zeros((len(WHITE_POINTS), len(SLIDER), len(T)), np.float32)
    for i, lw in enumerate(WHITE_POINTS):
        g = white_ramp(lw)
        x = np.log2(np.array(g.values))
        for j, v in enumerate(SLIDER):
            y = g.measure(camera_raw.read_linear(d / f'white{lw:+.2f}-{name(v)}.tif'))
            row = np.interp(T, x, y, right=1.0)
            below = T < x[0]
            # Below the ramp the stage is linear (the toe is above it).
            row[below] = 2.0 ** (T[below] - x[0]) * y[0]
            out[i, j] = row
    return out


def white3_table(d):
    out = np.zeros((len(SENSOR), len(BELOW), len(SLIDER), len(U)))
    for i, ls in enumerate(SENSOR):
        for j, c in enumerate(BELOW):
            g = white3_ramp(ls, c)
            u = np.log2(REL)
            for k, v in enumerate(SLIDER):
                y = g.measure(camera_raw.read_linear(d / f'white3{ls:+.1f}-{c:.1f}-{name(v)}.tif'))
                out[i, j, k] = np.interp(U, u, y)
    return np.round(np.clip(out, 0, 1) * 65535).astype('<u2')


def blacks_table(d):
    out = np.zeros((len(BLACK_KEYS), len(SLIDER), len(Y)), np.float32)
    for i, key in enumerate(BLACK_KEYS):
        g = black_ramp(key)
        default = g.measure(camera_raw.read_linear(d / f'black{key:+.1f}-default.tif'))
        order = np.argsort(default)
        x = np.log2(np.maximum(default[order], 2.0 ** -20))
        for j, v in enumerate(SLIDER):
            y = g.measure(camera_raw.read_linear(d / f'black{key:+.1f}-{name(v)}.tif'))[order]
            row = np.interp(Y, x, y, right=1.0)
            below = Y < x[0]
            # Darker than the darkest patch: negative Blacks has clipped it to black,
            # positive Blacks scales like it.
            row[below] = 0. if v < 0 else 2.0 ** (Y[below] - x[0]) * y[0]
            out[i, j] = row
    return out


def default_black_table(d):
    w = white_ramp(2.0)
    xw = np.log2(np.array(w.values))
    yw = w.measure(camera_raw.read_linear(d / 'white+2.00-default.tif'))
    out = np.zeros((len(BLACK_KEYS), len(Y)), np.float32)
    for i, key in enumerate(BLACK_KEYS):
        g = black_ramp(key)
        yb = g.measure(camera_raw.read_linear(d / f'black{key:+.1f}-default.tif'))
        # The white curves' output at the black ramp's scene values.
        ywx = np.interp(np.log2(np.array(g.values)), xw, yw)
        order = np.argsort(ywx)
        x = np.log2(np.maximum(ywx[order], 2.0 ** -20))
        y = np.maximum.accumulate(yb[order])
        row = np.interp(Y, x, y, right=1.0)
        row[Y < x[0]] = 0.
        out[i] = row
    return out


def masks_table(d):
    out = np.zeros((2, len(MASK_VALUES), len(Y)), np.float32)
    for i, kind in enumerate(('Whites', 'Blacks')):
        for j, v in enumerate(MASK_VALUES):
            xs, ys = [], []
            for lw in MASK_WHITES:
                g = white_ramp(lw)
                xs += list(g.measure(camera_raw.read_linear(d / f'mask{lw}-default.tif')))
                ys += list(g.measure(camera_raw.read_linear(d / f'mask{lw}-{kind}{v:+g}.tif')))
            order = np.argsort(xs)
            x, y = np.array(xs)[order], np.maximum.accumulate(np.array(ys)[order])
            keep = x > 2e-6
            x, y = np.log2(x[keep]), y[keep]
            row = np.interp(Y, x, y, right=1.0)
            below = Y < x[0]
            # As blacks_table below the darkest patch.
            row[below] = 0. if kind == 'Blacks' and v < 0 else 2.0 ** (Y[below] - x[0]) * y[0]
            out[i, j] = row
    return out


def tables(args):
    d = args.dir.resolve()
    white, white3, blacks = white_table(d), white3_table(d), blacks_table(d)
    masks, default_black = masks_table(d), default_black_table(d)
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    (out / 'white.bin').write_bytes(white.astype('<f4').tobytes())
    (out / 'white3.bin').write_bytes(white3.tobytes())
    (out / 'blacks.bin').write_bytes(blacks.astype('<f4').tobytes())
    (out / 'default_black.bin').write_bytes(default_black.astype('<f4').tobytes())
    (out / 'masks.bin').write_bytes(masks.astype('<f4').tobytes())
    print(f'white {white.shape}, white3 {white3.shape}, blacks {blacks.shape}, masks {masks.shape},'
          f' default black {default_black.shape} -> {out}')


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('step', choices=['render', 'tables'])
    p.add_argument('dir', type=Path, help='folder for the probes and their renders (outside the repository)')
    p.add_argument('--out', type=Path, default=OUT, help='folder for the tables (default: the engine\'s)')
    args = p.parse_args()
    {'render': render, 'tables': tables}[args.step](args)


if __name__ == '__main__':
    main()
