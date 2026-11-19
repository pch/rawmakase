#!/usr/bin/env python3
"""Synthetic `.cube` LUTs for testing their import as looks (docs/lightroom-profiles.md#cube-luts).

Each cube is made up from a simple formula here (no third-party LUT data): identity,
a channel swap, a hue rotation, posterization, black and white, outputs outside 0–1,
a sharp creative grade at 32, 33 and 65 divisions, a 1D curve, a DOMAIN other than 0–1
and a 1D shaper in front of the 3D table. The cubes are not committed (a 65³ cube is
about 8 MB of text); this script writes the same bytes every time.

  python3 scripts/corpus/synthetic-cubes.py cubes OUT
      writes OUT/<name>.cube for every cube below.
  python3 scripts/corpus/synthetic-cubes.py looks --rawmakase target/release/rawmakase
      imports them with `rawmakase import-profiles` into a temporary library and copies
      the generated look XMPs the chart cases in tests/corpus/cases.json use (and
      scripts/corpus/camera-raw-charts.py renders) to tests/corpus/looks/cube-<name>.xmp.
      Cubes RAWmakase refuses (a DOMAIN other than 0–1) are listed and get no look.
      `cube-creative-33-mono.xmp` is the same look with `ConvertToGrayscale` true in its
      settings, as a black and white photo's sidecar carries it (#1187).
  python3 scripts/corpus/synthetic-cubes.py looks --lut-space adobe-rgb
      the same, imported with `--lut-space` (adobe-rgb, display-p3, prophoto-rgb or
      rec2020), into tests/corpus/looks/cube-<name>-<space>.xmp.

Run from the repository root.
"""
import argparse
import colorsys
import hashlib
import json
import math
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LOOKS = ROOT / 'tests/corpus/looks'


def identity(r, g, b):
    return r, g, b


def swap(r, g, b):
    """Red takes green's value, green blue's, blue red's."""
    return g, b, r


def hue_rotation(r, g, b):
    """Every hue turned by 60° at its saturation and value."""
    h, s, v = colorsys.rgb_to_hsv(r, g, b)
    return colorsys.hsv_to_rgb((h + 1 / 6) % 1, s, v)


def posterize(r, g, b):
    """Four levels per channel: steps a lookup must not smooth between nodes."""
    return tuple(min(3, math.floor(c * 4)) / 3 for c in (r, g, b))


def black_and_white(r, g, b):
    """Rec. 709 weights on the encoded values, as a simple B&W LUT does."""
    y = 0.2126 * r + 0.7152 * g + 0.0722 * b
    return y, y, y


def out_of_range(r, g, b):
    """More contrast than 0–1 holds: outputs from −0.2 to 1.2, clamped on import."""
    return tuple(1.4 * c - 0.2 for c in (r, g, b))


def creative(r, g, b):
    """A sharp creative grade: an S curve per channel, teal shadows and warm highlights,
    and a saturation boost that changes fast near the neutral axis."""
    def s_curve(c):
        return 0.5 + 0.5 * math.tanh(3.2 * (c - 0.5)) / math.tanh(1.6)
    r, g, b = s_curve(r), s_curve(g), s_curve(b)
    y = 0.2126 * r + 0.7152 * g + 0.0722 * b
    r += 0.08 * (y - 0.5) - 0.02
    b -= 0.10 * (y - 0.5) - 0.02
    k = 1.35 + 0.4 * math.sin(6 * math.pi * y)
    return tuple(min(1., max(0., y + k * (c - y))) for c in (r, g, b))


def cross_curve(v):
    """Channel curves for a 1D cube: red lifted, green with an S, blue compressed."""
    return 0.05 + 0.9 * v ** 0.85, v + 0.06 * math.sin(2 * math.pi * v), 0.1 + 0.75 * v


def number(v):
    return f'{v:.6f}'


def cube_3d(size, f, title, domain=None, shaper=None):
    lines = [f'TITLE "{title}"', '# Synthetic test LUT, public domain (scripts/corpus/synthetic-cubes.py)']
    if domain:
        lines += ['DOMAIN_MIN ' + ' '.join(map(number, [domain[0]] * 3)),
                  'DOMAIN_MAX ' + ' '.join(map(number, [domain[1]] * 3))]
    if shaper:
        entries, curve = shaper
        lines.append(f'LUT_1D_SIZE {entries}')
    lines.append(f'LUT_3D_SIZE {size}')
    if shaper:
        for i in range(entries):
            lines.append(' '.join([number(curve(i / (entries - 1)))] * 3))
    low, high = domain or (0., 1.)
    for b in range(size):
        for g in range(size):
            for r in range(size):
                # The node's input in the domain; the formula is defined on 0–1.
                p = [low + (high - low) * i / (size - 1) for i in (r, g, b)]
                if shaper:
                    p = [shaper_inverse(c) for c in p]
                p = [min(1., max(0., c)) for c in p]
                lines.append(' '.join(number(v) for v in f(*p)))
    return '\n'.join(lines) + '\n'


def cube_1d(size, f, title):
    lines = [f'TITLE "{title}"', '# Synthetic test LUT, public domain (scripts/corpus/synthetic-cubes.py)',
             f'LUT_1D_SIZE {size}']
    lines += [' '.join(number(v) for v in f(i / (size - 1))) for i in range(size)]
    return '\n'.join(lines) + '\n'


def shaper_curve(v):
    """A log-like shaper: spends the 3D table's nodes on the shadows."""
    return math.log2(1 + 15 * v) / 4


def shaper_inverse(v):
    return (2 ** (4 * v) - 1) / 15


CUBES = {
    'identity-17': lambda: cube_3d(17, identity, 'Identity'),
    'identity-33': lambda: cube_3d(33, identity, 'Identity'),
    'swap-17': lambda: cube_3d(17, swap, 'Channel swap'),
    'hue-rotation-33': lambda: cube_3d(33, hue_rotation, 'Hue rotation'),
    'posterize-17': lambda: cube_3d(17, posterize, 'Posterize'),
    'bw-33': lambda: cube_3d(33, black_and_white, 'Black and white'),
    'out-of-range-17': lambda: cube_3d(17, out_of_range, 'Out of range'),
    'creative-32': lambda: cube_3d(32, creative, 'Creative'),
    'creative-33': lambda: cube_3d(33, creative, 'Creative'),
    'creative-65': lambda: cube_3d(65, creative, 'Creative'),
    'curve-1d-1024': lambda: cube_1d(1024, cross_curve, 'Cross curves'),
    # A domain wider than 0–1, as some LUTs for scene-referred input have.
    'domain-33': lambda: cube_3d(33, creative, 'Creative, wide domain', domain=(-0.1, 1.2)),
    'shaper-33': lambda: cube_3d(33, creative, 'Creative, shaper', shaper=(1024, shaper_curve)),
}


def write_cubes(out, quiet=False):
    out.mkdir(parents=True, exist_ok=True)
    for name, make in CUBES.items():
        (out / f'{name}.cube').write_text(make())
        if not quiet:
            print(out / f'{name}.cube')


def write_looks(rawmakase, space='srgb'):
    with tempfile.TemporaryDirectory(prefix='synthetic-cubes-') as tmp:
        tmp = Path(tmp)
        cubes, data = tmp / 'Synthetic', tmp / 'data'
        write_cubes(cubes, quiet=True)
        env = dict(os.environ, RAWMAKASE_DATA_DIR=str(data))
        result = subprocess.run([str(rawmakase), 'import-profiles', '--lut-space', space, str(cubes)],
                                env=env, capture_output=True, text=True)
        print(result.stdout + result.stderr, end='')
        library = data / 'camera-profiles'
        # Only the looks the chart cases use are kept in the repository.
        used = {c['look']['file'] for c in json.loads((ROOT / 'tests/corpus/cases.json').read_text())['cases']
                if 'look' in c}
        LOOKS.mkdir(parents=True, exist_ok=True)
        suffix = '' if space == 'srgb' else f'-{space}'
        for name in CUBES:
            look = library / f'{name}.xmp'
            target = LOOKS / f'cube-{name}{suffix}.xmp'
            if not look.exists():
                print(f'{name}: no look (refused)')
            elif target.name in used:
                shutil.copyfile(look, target)
                print(target)
            mono = LOOKS / f'cube-{name}{suffix}-mono.xmp'
            if look.exists() and mono.name in used:
                mono.write_text(monochrome(look.read_text(), mono.stem))
                print(mono)


def monochrome(text, name):
    """A look whose settings say black and white, under its own name and UUID."""
    uuid = hashlib.md5(name.encode()).hexdigest().upper()
    text = re.sub(r'crs:UUID="[^"]*"', f'crs:UUID="{uuid}"', text, count=1)
    text = text.replace('crs:ConvertToGrayscale="False"', 'crs:ConvertToGrayscale="True"', 1)
    return re.sub(r'(<crs:Name>\s*<rdf:Alt>\s*<rdf:li xml:lang="x-default">)[^<]*', rf'\g<1>{name}', text, count=1)


if __name__ == '__main__':
    sys.dont_write_bytecode = True
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest='command', required=True)
    s = sub.add_parser('cubes')
    s.add_argument('out', type=Path)
    s = sub.add_parser('looks')
    s.add_argument('--rawmakase', type=Path, default=ROOT / 'target/release/rawmakase')
    s.add_argument('--lut-space', choices=['srgb', 'adobe-rgb', 'display-p3', 'prophoto-rgb', 'rec2020'], default='srgb')
    args = p.parse_args()
    if args.command == 'cubes':
        write_cubes(args.out)
    else:
        write_looks(args.rawmakase, args.lut_space)
