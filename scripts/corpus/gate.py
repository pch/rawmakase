#!/usr/bin/env python3
"""Photo gates for the scene tone stage (docs/scene-tone-stage.md#tier-3-photo-gates).

Frozen metric definitions and the acceptance rule are code here; the photos, Camera
Raw references and RAWmakase renders live outside the repository.

  survey     Camera Raw default render (Adobe Standard) of each RAW in a pool, its
             brightness (p90 L*) and stratum: pool.json
  split      stratified, deterministic train / validation / holdout manifests
  reference  Camera Raw renders of a manifest's photos for every gate setting
             (16-bit ProPhoto, 2048 px), checked against the settings Camera Raw
             embedded, then frozen with their hashes
  render     RAWmakase renders of the same settings (the XMP Camera Raw embedded)
  score      per-photo, per-setting metrics of a render folder against references
  accept     the acceptance rule: a candidate's scores against engine 4's

Requires numpy, Pillow, tifffile and exiftool; `survey` and `reference` drive
Photoshop 2026 (scripts/corpus/camera_raw.py). Never commit photos or renders.
"""
import argparse
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path

import numpy as np
from PIL import Image

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).parent))
import camera_raw  # noqa: E402
import parity_metrics as metrics  # noqa: E402

RAW_EXTENSIONS = {'.arw', '.raf', '.nef', '.nrw', '.cr2', '.cr3', '.dng', '.orf', '.rw2', '.pef', '.rwl'}
STRATA = [('low', 0., 40.), ('mid', 40., 75.), ('high', 75., 101.)]
EDGE = 2048          # renders are made at this long edge ...
SCORE_EDGE = 1024    # ... and scored after an area reduction to this one
ADOBE_COLOR = 'Adobe Color'
# Camera Raw's Adobe Color is a look over Adobe Standard.
ADOBE_COLOR_LOOK = Path('/Library/Application Support/Adobe/CameraRaw/Settings/Adobe/Profiles/Adobe Raw/'
                        'Adobe Color.xmp')
MASK = ('<crs:MaskGroupBasedCorrections><rdf:Seq><rdf:li><rdf:Description crs:What="Correction" '
        'crs:CorrectionAmount="1" crs:LocalExposure2012="0.5" crs:LocalShadows2012="0.5">'
        '<crs:CorrectionMasks><rdf:Seq><rdf:li crs:What="Mask/Gradient" crs:MaskBlendMode="0" '
        'crs:ZeroX="0.5" crs:ZeroY="0.65" crs:FullX="0.5" crs:FullY="0.35"/></rdf:Seq></crs:CorrectionMasks>'
        '</rdf:Description></rdf:li></rdf:Seq></crs:MaskGroupBasedCorrections>')
PERCENT = {'Contrast2012': 'contrast', 'Highlights2012': 'highlights', 'Shadows2012': 'shadows',
           'Whites2012': 'whites', 'Blacks2012': 'blacks', 'Texture': 'texture', 'Clarity2012': 'clarity',
           'Dehaze': 'dehaze'}


def settings():
    """Every gate setting: name -> (crs settings, profile, magnitude class, extra XML).

    The magnitude is the largest slider magnitude, Exposure ±1 EV counting as 50 and
    ±2 EV as 100 (acceptance rule 3)."""
    out = {'default': ({}, 'Adobe Standard', 0, '')}
    for key, label in PERCENT.items():
        for v in (-100, -50, -25, 25, 50, 100):
            out[f'{label}{v:+d}'] = ({key: str(v)}, 'Adobe Standard', abs(v), '')
    for v in (-2, -1, 1, 2):
        out[f'exposure{v:+d}'] = ({'Exposure2012': str(v)}, 'Adobe Standard', 50 if abs(v) == 1 else 100, '')
    for name, s, magnitude in [
            ('exposure-1+whites+100', {'Exposure2012': '-1', 'Whites2012': '100'}, 100),
            ('shadows+50+highlights-50', {'Shadows2012': '50', 'Highlights2012': '-50'}, 50),
            ('contrast+50+blacks-50', {'Contrast2012': '50', 'Blacks2012': '-50'}, 50),
            ('dehaze+40+clarity+40', {'Dehaze': '40', 'Clarity2012': '40'}, 40)]:
        out[name] = (s, 'Adobe Standard', magnitude, '')
    out['mask-exposure+shadows'] = ({}, 'Adobe Standard', 50, MASK)
    # The #354 report: Adobe Color.
    for name, s, magnitude in [('color-default', {}, 0), ('color-shadows+100', {'Shadows2012': '100'}, 100),
                               ('color-dehaze+40', {'Dehaze': '40'}, 40),
                               ('color-whites+100', {'Whites2012': '100'}, 100)]:
        out[name] = (s, ADOBE_COLOR, magnitude, '')
    return out


def digest(path):
    with Path(path).open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


# Frozen metrics -------------------------------------------------------------------

PROPHOTO_TO_XYZ = np.array([[0.7976749, 0.1351917, 0.0313534], [0.2880402, 0.7118741, 0.0000857],
                            [0., 0., 0.8252100]])
BRADFORD_D50_D65 = np.array([[0.9555766, -0.0230393, 0.0631636], [-0.0282895, 1.0099416, 0.0210077],
                             [0.0122982, -0.0204830, 1.3299098]])
XYZ_TO_SRGB = np.array([[3.2404542, -1.5371385, -0.4985314], [-0.9692660, 1.8760108, 0.0415560],
                        [0.0556434, -0.2040259, 1.0572252]])


def encode_srgb(linear):
    v = np.clip(linear, 0, 1)
    return np.where(v <= 0.0031308, 12.92 * v, 1.055 * v ** (1 / 2.4) - 0.055)


def reference_srgb(path):
    """A Camera Raw ProPhoto reference as encoded sRGB, clipped (relative colorimetric)."""
    linear = camera_raw.read_linear(path)
    return encode_srgb(linear @ (XYZ_TO_SRGB @ BRADFORD_D50_D65 @ PROPHOTO_TO_XYZ).T)


def render_srgb(path):
    import tifffile
    a = tifffile.imread(str(path))
    return a[..., :3].astype(np.float64) / (65535. if a.dtype == np.uint16 else 255.)


def reduce(a, edge=SCORE_EDGE):
    """Area reduction to `edge` on the long side (PIL BOX filter per channel)."""
    h, w = a.shape[:2]
    s = edge / max(w, h)
    size = (max(1, round(w * s)), max(1, round(h * s)))
    return np.stack([np.array(Image.fromarray(a[..., i].astype(np.float32)).resize(size, Image.Resampling.BOX))
                     for i in range(3)], -1).astype(np.float64)


def lab(srgb):
    return metrics.lab(srgb)


def at_white(a):
    return (a >= 0.995).any(-1)


def at_black(a):
    return (a <= 0.005).all(-1)


def photo_metrics(reference, render):
    """Per-photo metrics of two encoded-sRGB images of one framing, already reduced.

    Returns the median and mean ΔE00 over valid pixels (all pixels of the common frame
    but a 3-pixel border), the fraction of valid pixels, the clipping disagreement, and
    the per-pixel L* of both for the band statistics."""
    # Reductions of one framing may round to a pixel more or less; anything more is a
    # different framing.
    if any(abs(x - y) > 2 for x, y in zip(reference.shape, render.shape)):
        return dict(valid=0., error=f'framing differs: {reference.shape} / {render.shape}')
    h, w = min(reference.shape[0], render.shape[0]), min(reference.shape[1], render.shape[1])
    a, b = reference[3:h - 3, 3:w - 3], render[3:h - 3, 3:w - 3]
    la, lb = lab(a), lab(b)
    d = metrics.de00(la, lb)
    valid = np.isfinite(d)
    clip = (at_white(a) ^ at_white(b)) | (at_black(a) ^ at_black(b))
    return dict(median_de00=float(np.median(d[valid])), mean_de00=float(d[valid].mean()),
                valid=float(valid.mean()) * (a.shape[0] * a.shape[1]) / (reference.shape[0] * reference.shape[1]),
                clipping=float(clip[valid].mean()), L_ref=la[..., 0][valid], L_out=lb[..., 0][valid])


def bands(pairs):
    """Signed ΔL* (render − reference) per 10-wide band of the reference's L*, pooled over
    the pixels of the photos given as (L_ref, L_out); bands holding < 1% are skipped."""
    ref = np.concatenate([p[0] for p in pairs])
    out = np.concatenate([p[1] for p in pairs])
    result = {}
    for lo in range(0, 100, 10):
        m = (ref >= lo) & (ref < lo + 10 if lo < 90 else ref <= 100.5)
        if m.mean() >= 0.01:
            result[f'{lo}-{lo + 10}'] = float((out[m] - ref[m]).mean())
    return result


def stratum(p90):
    return next(name for name, lo, hi in STRATA if lo <= p90 < hi)


# Survey and split -----------------------------------------------------------------

def survey(args):
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    raws = sorted({p.resolve() for r in args.pool for p in (Path(r).rglob('*') if Path(r).is_dir() else [Path(r)])
                   if p.suffix.lower() in RAW_EXTENSIONS})
    jobs = []
    for raw in raws:
        tif = out / 'renders' / (digest(raw)[:16] + '.tif')
        jobs.append(dict(source=str(raw), settings={'CameraProfile': 'Adobe Standard'}, out=str(tif), edge=1024,
                         raw=str(raw)))
    failures = camera_raw.render(jobs, work=out / 'work', chunk=args.chunk)
    pool = []
    for j in jobs:
        if not Path(j['out']).exists():
            continue
        L = lab(reduce(reference_srgb(j['out'])))[..., 0]
        p90 = float(np.percentile(L, 90))
        pool.append(dict(raw=j['raw'], sha256=digest(j['raw']), p90_L=p90, stratum=stratum(p90),
                         make=Path(j['raw']).parent.parent.name))
    (out / 'pool.json').write_text(json.dumps(dict(schema=1, camera_raw=camera_raw.version(), photos=pool), indent=1))
    print(f'{len(pool)} photos surveyed; {len(failures)} failures')
    for name, _, _ in STRATA:
        print(f'  {name}: {sum(p["stratum"] == name for p in pool)}')


def scene_key(photo):
    """raw.pixls.us keeps several files of one camera model that show the same scene
    (bit depths, compression modes): one photo per model, so a scene stays in one set."""
    path = Path(photo['raw'])
    return str(path.parent) if 'pixls' in path.parts else path.name.lower()


def split(args):
    pool = []
    seen = set()
    surveyed = [q for f in args.pool for q in json.loads(Path(f).read_text())['photos']]
    for p in sorted(surveyed, key=lambda p: p['sha256']):
        # A render Camera Raw could not make reads as black; reduced-resolution RAW
        # modes are not mosaics.
        reduced = any(m in Path(p['raw']).name.lower() for m in ('small raw', 'sraw', 'mraw'))
        if p['p90_L'] <= 1 or reduced or scene_key(p) in seen:
            continue
        seen.add(scene_key(p))
        pool.append(p)
    per = {'train': args.train, 'validation': args.validation, 'holdout': args.holdout}
    sets = {k: [] for k in per}
    train_only = lambda p: any(t in p['raw'] for t in args.train_only)
    for name, _, _ in STRATA:
        # Deterministic order from the file hash, interleaving makes so each set gets
        # a mix of cameras.
        photos = sorted((p for p in pool if p['stratum'] == name), key=lambda p: p['sha256'])
        by_make = {}
        for p in photos:
            if not train_only(p):
                by_make.setdefault(p['make'], []).append(p)
        order = []
        while any(by_make.values()):
            for make in sorted(by_make):
                if by_make[make]:
                    order.append(by_make[make].pop(0))
        # A scarce stratum is shared evenly between the gates; the rest trains.
        gate = min(per['holdout'], per['validation'], len(order) // 2)
        sets['holdout'] += order[:gate]
        sets['validation'] += order[gate:2 * gate]
        rest = order[2 * gate:] + [p for p in photos if train_only(p)]
        sets['train'] += rest[:per['train']]
    args.out.mkdir(parents=True, exist_ok=True)
    for key, photos in sets.items():
        (args.out / f'{key}.json').write_text(json.dumps(dict(schema=1, set=key, photos=photos), indent=1))
        print(key, len(photos), {n: sum(p['stratum'] == n for p in photos) for n, _, _ in STRATA})


# References and renders -----------------------------------------------------------

def gate_xmp(profile, settings_, extra):
    s = dict(settings_, CameraProfile=profile)
    text = camera_raw.xmp(s)
    return text.replace('></rdf:Description>', '>' + extra + '</rdf:Description>', 1) if extra else text


def camera_profile(profile):
    """The CameraProfile Camera Raw renders a gate profile with, and its look."""
    if profile == ADOBE_COLOR:
        return 'Adobe Standard', dict(file=str(ADOBE_COLOR_LOOK), amount=1)
    return profile, None


def reference(args):
    manifest = json.loads(args.manifest.read_text())
    out = args.out.resolve()
    (out / 'references').mkdir(parents=True, exist_ok=True)
    chosen = settings()
    if args.settings:
        chosen = {k: v for k, v in chosen.items() if k in args.settings or k == 'default'}
    jobs, plan = [], []
    for i, photo in enumerate(manifest['photos']):
        if digest(photo['raw']) != photo['sha256']:
            raise ValueError(f'{photo["raw"]}: changed since the manifest was frozen')
        for name, (s, profile, magnitude, extra) in chosen.items():
            target = out / 'references' / f'{i}-{name}.tif'
            base, look = camera_profile(profile)
            job = dict(source=photo['raw'], settings=dict(s, CameraProfile=base), out=str(target), edge=EDGE)
            if look:
                job['look'] = look
            if extra:
                job['xmp_text'] = gate_xmp(base, s, extra)
            jobs.append(job)
            plan.append(dict(photo=i, name=name, reference=str(target.relative_to(out))))
    failures = camera_raw.render(jobs, work=out / 'work', chunk=args.chunk)
    for f in failures:
        print('  failed:', f, file=sys.stderr)
    frozen = []
    for c in plan:
        target = out / c['reference']
        if not target.exists():
            continue
        xmp = target.with_suffix('.xmp')
        if not xmp.exists():
            xmp.write_bytes(subprocess.check_output(['exiftool', '-b', '-XMP', str(target)]))
        s, profile, _, _ = chosen[c['name']]
        base, look = camera_profile(profile)
        tags = json.loads(subprocess.check_output(['exiftool', '-j', '-n', '-XMP-crs:all', str(xmp)]))[0]
        if tags.get('CameraProfile') != base or (look and tags.get('LookName') != profile):
            raise ValueError(f'{target.name}: rendered with {tags.get("CameraProfile")!r} '
                             f'{tags.get("LookName", "")!r}, expected {profile!r}')
        for k, v in s.items():
            if abs(float(tags.get(k, 'nan')) - float(v)) > 1e-4:
                raise ValueError(f'{target.name}: {k} = {tags.get(k)!r}, expected {v}')
        frozen.append(dict(c, reference_sha256=digest(target), xmp=str(xmp.relative_to(out)), xmp_sha256=digest(xmp)))
    (out / 'references.json').write_text(json.dumps(dict(
        schema=1, camera_raw=camera_raw.version(), set=manifest['set'], photos=manifest['photos'], cases=frozen),
        indent=1))
    print(f'{len(frozen)} references frozen in {out / "references.json"}')


def render(args):
    """One `rawmakase render-batch` per photo: it develops the RAW once."""
    refs = args.refs.resolve()
    frozen = json.loads((refs / 'references.json').read_text())
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ)
    if args.data_dir:
        env['RAWMAKASE_DATA_DIR'] = str(args.data_dir)
    for i, photo in enumerate(frozen['photos']):
        done = lambda name: (out / name).exists() or (out / name).with_suffix('.npy').exists()
        jobs = [dict(xmp=str(refs / c['xmp']), output=str(out / Path(c['reference']).name), max_edge=EDGE)
                for c in frozen['cases'] if c['photo'] == i and not done(Path(c['reference']).name)]
        if not jobs:
            continue
        listing = out / f'jobs-{i}.json'
        listing.write_text(json.dumps(jobs))
        result = subprocess.run([str(args.rawmakase), 'render-batch', photo['raw'], str(listing)],
                                capture_output=True, text=True, env=env)
        listing.unlink()
        if result.returncode or result.stderr.strip():
            print(f'photo {i}: {result.stderr.strip()[:400]}', file=sys.stderr)
        # Keep what the metric reads: the render reduced to SCORE_EDGE.
        for job in jobs:
            tif = Path(job['output'])
            if tif.exists():
                np.save(tif.with_suffix('.npy'), reduce(render_srgb(tif)).astype(np.float32))
                tif.unlink()


# Scoring and acceptance -----------------------------------------------------------

def score(args):
    refs = args.refs.resolve()
    frozen = json.loads((refs / 'references.json').read_text())
    rows, pixels = [], {}
    for c in frozen['cases']:
        ref = refs / c['reference']
        out = args.renders / ref.name
        photo = frozen['photos'][c['photo']]
        row = dict(photo=c['photo'], name=c['name'], stratum=photo['stratum'])
        if digest(ref) != c['reference_sha256']:
            raise ValueError(f'{ref.name}: reference changed since it was frozen')
        reduced = out.with_suffix('.npy')
        if not out.exists() and not reduced.exists():
            rows.append(dict(row, valid=0., error='not rendered'))
            continue
        rendered = np.load(reduced).astype(np.float64) if reduced.exists() else reduce(render_srgb(out))
        m = photo_metrics(reduce(reference_srgb(ref)), rendered)
        L = (m.pop('L_ref', None), m.pop('L_out', None))
        if L[0] is not None:
            pixels.setdefault((c['name'], photo['stratum']), []).append(L)
        rows.append(dict(row, **m))
    band_rows = {f'{n}|{s}': bands(p) for (n, s), p in pixels.items()}
    args.out.write_text(json.dumps(dict(schema=1, set=frozen['set'], photos=rows, bands=band_rows), indent=1))
    print(f'{len(rows)} scores -> {args.out}')


def summarise(scores):
    """setting -> stratum -> dict(score, mean, photos{i: score}, clipping, n)."""
    out = {}
    for r in scores['photos']:
        s = out.setdefault(r['name'], {}).setdefault(r['stratum'], dict(photos={}, clipping=[], invalid=0))
        if r.get('valid', 0) < 0.5:
            s['invalid'] += 1
            continue
        s['photos'][r['photo']] = r['median_de00']
        s['clipping'].append(r['clipping'])
    for setting in out.values():
        for s in setting.values():
            v = list(s['photos'].values())
            s['n'] = len(v)
            s['score'] = float(np.median(v)) if v else float('nan')
            s['mean'] = float(np.mean(v)) if v else float('nan')
            s['clipping'] = float(np.mean(s['clipping'])) if s['clipping'] else float('nan')
    return out


def accept(args):
    cand, base = json.loads(args.candidate.read_text()), json.loads(args.baseline.read_text())
    C, B = summarise(cand), summarise(base)
    magnitude = {k: v[2] for k, v in settings().items()}
    failed = []
    print(f'{"setting":28s} {"stratum":7s} {"n":>2s} {"e4":>6s} {"new":>6s} {"dflt":>6s}  result')
    for name in sorted(C, key=lambda n: (n != 'default', n)):
        default_name = 'color-default' if name.startswith('color-') else 'default'
        for st, _, _ in STRATA:
            c, b = C[name].get(st), B.get(name, {}).get(st)
            cd, bd = C.get(default_name, {}).get(st), B.get(default_name, {}).get(st)
            reasons = []
            if not c or not b or c['n'] < 3 or c['invalid'] or b['n'] < 3:
                reasons.append('inconclusive (fewer than 3 photos or invalid photos)')
            else:
                if name.endswith('default'):
                    if c['score'] > b['score'] + 0.1:
                        reasons.append('5: default worse than engine 4 + 0.1')
                else:
                    near = b['score'] <= bd['score'] + 0.5
                    if not near:
                        if not (c['score'] <= cd['score'] + 1.0 or c['score'] <= 0.6 * b['score']):
                            reasons.append('1: no material improvement')
                        cb = cand['bands'].get(f'{name}|{st}', {})
                        bb = base['bands'].get(f'{name}|{st}', {})
                        for band, v in cb.items():
                            if abs(v) > 3 and not (band in bb and abs(v) <= 0.5 * abs(bb[band])):
                                reasons.append(f'2: band {band} ΔL* {v:+.1f}')
                    limit = 1.0 if magnitude[name] <= 50 else 2.0
                    for p, v in c['photos'].items():
                        if p in b['photos'] and v > b['photos'][p] + limit:
                            reasons.append(f'3: photo {p} {b["photos"][p]:.2f} -> {v:.2f}')
                    if c['clipping'] > b['clipping'] + 0.005:
                        reasons.append(f'4: clipping {100 * b["clipping"]:.2f}% -> {100 * c["clipping"]:.2f}%')
            dflt = cd['score'] if cd else float('nan')
            print(f'{name:28s} {st:7s} {c["n"] if c else 0:2d} {b["score"] if b else float("nan"):6.2f} '
                  f'{c["score"] if c else float("nan"):6.2f} {dflt:6.2f}  {"PASS" if not reasons else "; ".join(reasons)}')
            if reasons:
                failed.append((name, st))
    print(f'{len(failed)} failing setting/stratum pairs')
    return int(bool(failed))


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest='command', required=True)
    s = sub.add_parser('survey')
    s.add_argument('--pool', nargs='+', required=True, help='RAW files or folders')
    s.add_argument('--out', type=Path, required=True)
    s.add_argument('--chunk', type=int, default=20)
    s = sub.add_parser('split')
    s.add_argument('--pool', nargs='+', required=True, help='pool.json files from survey')
    s.add_argument('--out', type=Path, required=True)
    for key, n in [('train', 8), ('validation', 6), ('holdout', 6)]:
        s.add_argument(f'--{key}', type=int, default=n, help='photos per stratum')
    s.add_argument('--train-only', nargs='*', default=[],
                   help='path parts of photos that may only train (a session of similar scenes)')
    s = sub.add_parser('reference')
    s.add_argument('--manifest', type=Path, required=True)
    s.add_argument('--out', type=Path, required=True)
    s.add_argument('--settings', nargs='*')
    s.add_argument('--chunk', type=int, default=20)
    s = sub.add_parser('render')
    s.add_argument('--refs', type=Path, required=True)
    s.add_argument('--rawmakase', type=Path, required=True)
    s.add_argument('--out', type=Path, required=True)
    s.add_argument('--data-dir', type=Path)
    s = sub.add_parser('score')
    s.add_argument('--refs', type=Path, required=True)
    s.add_argument('--renders', type=Path, required=True)
    s.add_argument('--out', type=Path, required=True)
    s = sub.add_parser('accept')
    s.add_argument('--candidate', type=Path, required=True)
    s.add_argument('--baseline', type=Path, required=True)
    args = p.parse_args()
    return {'survey': survey, 'split': split, 'reference': reference, 'render': render, 'score': score,
            'accept': accept}[args.command](args)


if __name__ == '__main__':
    raise SystemExit(main())
