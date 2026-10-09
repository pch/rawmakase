#!/usr/bin/env python3
"""Camera Raw references for the scene tone stage's CI probes (tests/color/scene_probes.rs).

  render      ROOT  writes the probe DNGs under ROOT and renders the cases below in
                    Camera Raw (camera_raw.py), skipping renders that exist
  references  ROOT  reads the renders under ROOT and writes
                    tests/corpus/camera-raw/scene-probes.json and the reduced scene
                    DNGs in tests/corpus/scene-probes/

ROOT holds the survey's folders; the file names below are
the survey's, from the generators in this folder:

- survey/family3/fam<W*>: scene-tone-tables.py's white ramp (192 neutral patches up to
  the sensor's white W*, baseline exposure log2 W*), default and Whites.
- survey/family3/black<key>: its black ramp (sensor white 4, darkest level and
  background 2^key), default and Blacks.
- survey/masks2/ramp<log2 W*>: the white ramp again, with Exposure and a full-frame
  mask's Whites and Blacks (scene-tone-tables.py's mask_xmp).
- survey/white/be0-c0: 96 patches from 2^-8 to the sensor's white 1, Exposure −2 to +1.
- synth/fit/scene<seed>: probe_scenes.py's scenes as scene-tone-local.py renders them,
  kept as half-size float16 arrays.

The ramps are rebuilt by the test from their patch values and layout (stored in the
JSON). The scenes are random fields the test cannot regenerate, so a copy reduced 4×
by area (384×256, values clipped at the sensor's white first, as its mosaic was) is
committed as an uncompressed probe DNG; RAWmakase's local operators scale with the
long edge, and the test compares 24×16 block luminances. Camera Raw values are linear
ProPhoto: a ramp patch's mean over its three channels (inside an inset), a scene
block's luminance. All renders are Camera Raw 18.7 with a linear profile tone curve,
so they read its scene tone stage (docs/scene-tone-stage.md#comparison-contract).
"""
import argparse
import json
import sys
from pathlib import Path

import numpy as np

sys.dont_write_bytecode = True
HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
import camera_raw  # noqa: E402
import probe_dng  # noqa: E402
import probe_scenes  # noqa: E402

ROOT = HERE.resolve().parents[1]
JSON = ROOT / 'tests/corpus/camera-raw/scene-probes.json'
SCENES = ROOT / 'tests/corpus/scene-probes'
REL = 2.0 ** np.linspace(-14, 0, 192)
YW = np.array([0.2880402, 0.7118741, 0.0000857])     # ProPhoto luminance
REDUCE, BLOCKS = 4, (24, 16)
SEEDS = [1, 3, 4]                                     # wide range, dark, bright


def grid(cols, patch, gap, margin=32, inset=8):
    return dict(cols=cols, patch=patch, gap=gap, margin=margin, inset=inset)


def ramp(values, background, baseline_exposure, layout):
    return dict(values=[float(v) for v in values], background=float(background),
                baseline_exposure=float(baseline_exposure), grid=layout)


WHITE = grid(24, 28, 8)
PROBES = {
    **{f'white{lw:+d}': ramp(2.0 ** lw * REL, 0.05, lw, WHITE) for lw in (0, 2, 4)},
    **{f'black{key:+.0f}': ramp([4. * r for r in REL if np.log2(4. * r) >= key], 2.0 ** key, 2, WHITE)
       for key in (-4., -8.)},
    'exposure': ramp(2.0 ** np.linspace(-8, 0, 96), 0.05, 0, grid(16, 32, 12, inset=10)),
}
for seed in SEEDS:
    PROBES[f'scene{seed}'] = dict(file=f'scene-probes/scene{seed}.dng', blocks=list(BLOCKS))


def mask(kind, v):
    return {f'Local{kind}2012': f'{v:g}'}


# name: (probe, settings, mask, render under ROOT)
CASES = {}
for lw, sliders in ((0, (-100, -50, 50, 100)), (2, (-100, -50, 50, 100)), (4, (-100, 100))):
    CASES[f'white{lw:+d}/default'] = (f'white{lw:+d}', {}, None, f'survey/family3/fam{lw:+.2f}-default.tif')
    for v in sliders:
        CASES[f'white{lw:+d}/W{v:+d}'] = (f'white{lw:+d}', {'Whites2012': str(v)}, None,
                                          f'survey/family3/fam{lw:+.2f}-W{v:+d}.tif')
for lw in (0, 2):
    for ev in (-1, 1):
        CASES[f'white{lw:+d}/E{ev:+d}'] = (f'white{lw:+d}', {'Exposure2012': str(ev)}, None,
                                           f'survey/masks2/ramp{lw}-E{ev:+g}.tif')
    CASES[f'white{lw:+d}/mask-W+50'] = (f'white{lw:+d}', {}, mask('Whites', 0.5),
                                        f'survey/masks2/ramp{lw}-mW+0.5.tif')
    CASES[f'white{lw:+d}/mask-B-50'] = (f'white{lw:+d}', {}, mask('Blacks', -0.5),
                                        f'survey/masks2/ramp{lw}-mB-0.5.tif')
for key in (-4, -8):
    CASES[f'black{key:+d}/default'] = (f'black{key:+d}', {}, None, f'survey/family3/black{key:+.1f}-default.tif')
    for v in (-100, -50, 50, 100):
        CASES[f'black{key:+d}/B{v:+d}'] = (f'black{key:+d}', {'Blacks2012': str(v)}, None,
                                           f'survey/family3/black{key:+.1f}-B{v:+d}.tif')
for ev in (-2, 0, 1):
    CASES[f'exposure/E{ev:+d}'] = ('exposure', {'Exposure2012': str(ev)} if ev else {}, None,
                                   f'survey/white/be0-c0-e{ev}.tif')
SCENE_SETTINGS = {'default': {}, 'S+100': {'Shadows2012': '100'}, 'S-50': {'Shadows2012': '-50'},
                  'H-100': {'Highlights2012': '-100'}, 'H+50': {'Highlights2012': '50'},
                  'D+40': {'Dehaze': '40'}, 'D-40': {'Dehaze': '-40'},
                  'C+50': {'Clarity2012': '50'}, 'C-50': {'Clarity2012': '-50'}}
for seed in SEEDS:
    for n, s in SCENE_SETTINGS.items():
        CASES[f'scene{seed}/{n}'] = (f'scene{seed}', s, None, f'synth/fit/scene{seed}-{n}.npy')


def positions(p):
    g = p['grid']
    for i in range(len(p['values'])):
        r, c = divmod(i, g['cols'])
        yield g['margin'] + r * (g['patch'] + g['gap']), g['margin'] + c * (g['patch'] + g['gap'])


def ramp_image(p):
    g, n = p['grid'], len(p['values'])
    rows = (n + g['cols'] - 1) // g['cols']
    w = 2 * g['margin'] + g['cols'] * g['patch'] + (g['cols'] - 1) * g['gap']
    h = 2 * g['margin'] + rows * g['patch'] + (rows - 1) * g['gap']
    im = np.full((h + h % 2, w + w % 2, 3), p['background'])
    for v, (y, x) in zip(p['values'], positions(p)):
        im[y:y + g['patch'], x:x + g['patch']] = v
    return im


def ramp_measure(p, a):
    i = p['grid']['inset']
    s = p['grid']['patch'] - i
    return [float(a[y + i:y + s, x + i:x + s].mean()) for y, x in positions(p)]


def scene(seed):
    """Scene `seed` and its baseline exposure, as scene-tone-local.py writes them."""
    image, _ = probe_scenes.scene(seed)
    rng = np.random.default_rng(1000 + seed)
    be = 0.5 if rng.random() < 0.5 else float(np.ceil(np.log2(image.max()) * 4 + 0.5) / 4)
    return image, be


def reduce(a, f):
    h, w = a.shape[0] // f * f, a.shape[1] // f * f
    return a[:h, :w].reshape(h // f, f, w // f, f, -1).mean((1, 3))


def blocks(a):
    bw, _ = BLOCKS
    return (reduce(a, a.shape[1] // bw) @ YW).ravel()


def render(args):
    jobs = []
    for name, (probe, settings, local, out) in CASES.items():
        p = PROBES[probe]
        out = args.root / out
        if out.suffix == '.npy':
            continue        # scene-tone-local.py synth renders the scenes
        source = out.parent / f'{probe}.dng'
        if not source.exists():
            out.parent.mkdir(parents=True, exist_ok=True)
            probe_dng.write(source, ramp_image(p), baseline_exposure=p['baseline_exposure'])
        job = dict(source=str(source), settings=settings, out=str(out))
        if local:
            job['xmp_text'] = camera_raw.xmp(settings).replace(
                '></rdf:Description>', '>' + mask_group(local) + '</rdf:Description>', 1)
        jobs.append(job)
    failures = camera_raw.render(jobs, work=args.root / 'work')
    print(f'{len(jobs)} renders, {len(failures)} failed')


def mask_group(local):
    attrs = ' '.join(f'crs:{k}="{v}"' for k, v in local.items())
    return ('<crs:MaskGroupBasedCorrections><rdf:Seq><rdf:li><rdf:Description crs:What="Correction" '
            f'crs:CorrectionAmount="1" {attrs}><crs:CorrectionMasks><rdf:Seq>'
            '<rdf:li crs:What="Mask/Gradient" crs:MaskBlendMode="0" crs:ZeroX="0.5" crs:ZeroY="-2" '
            'crs:FullX="0.5" crs:FullY="-1"/></rdf:Seq></crs:CorrectionMasks></rdf:Description></rdf:li>'
            '</rdf:Seq></crs:MaskGroupBasedCorrections>')


def references(args):
    SCENES.mkdir(parents=True, exist_ok=True)
    for seed in SEEDS:
        image, be = scene(seed)
        small = reduce(np.minimum(image, 2.0 ** be), REDUCE)
        probe_dng.write(SCENES / f'scene{seed}.dng', small, baseline_exposure=be)
    cases = {}
    for name, (probe, settings, local, path) in CASES.items():
        p, path = PROBES[probe], args.root / path
        if 'file' in p:
            values = blocks(np.load(path).astype(np.float64))
        else:
            values = ramp_measure(p, camera_raw.read_linear(path))
        case = dict(probe=probe, settings=settings)
        if local:
            case['mask'] = local
        case['camera_raw'] = [float(f'{v:.6g}') for v in values]
        cases[name] = case
    lines = ['{', ' "about": ' + json.dumps(
        'Camera Raw 18.7 references for tests/color/scene_probes.rs, written by '
        'scripts/corpus/scene-probes.py from renders of the probes with a linear profile tone curve '
        '(linear ProPhoto: ramp patch means, scene block luminances). Never re-blessed.') + ',',
        ' "probes": {']
    lines.append(',\n'.join(f'  {json.dumps(k)}: {json.dumps(v)}' for k, v in PROBES.items()))
    lines += [' },', ' "cases": {']
    lines.append(',\n'.join(f'  {json.dumps(k)}: {json.dumps(v)}' for k, v in cases.items()))
    lines += [' }', '}']
    JSON.write_text('\n'.join(lines) + '\n')
    print(f'{len(cases)} cases -> {JSON.relative_to(ROOT)}')


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('step', choices=['render', 'references'])
    p.add_argument('root', type=Path, help='folder holding the renders (outside the repository)')
    args = p.parse_args()
    args.root = args.root.expanduser().resolve()
    {'render': render, 'references': references}[args.step](args)


if __name__ == '__main__':
    main()
