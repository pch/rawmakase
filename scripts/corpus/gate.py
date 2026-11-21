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
import re
import json
import os
import shutil
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
# What the mask case's sidecar must hold: the gradient where MASK puts it, applied
# in full, both local adjustments, and nothing switching it off or inverting it.
# Camera Raw writes these as attributes or as elements.
MASK_VALUES = {'What': 'Mask/Gradient', 'CorrectionAmount': 1, 'LocalExposure2012': 0.5,
               'LocalShadows2012': 0.5, 'ZeroX': 0.5, 'ZeroY': 0.65, 'FullX': 0.5, 'FullY': 0.35}
MASK_OFF = {'CorrectionActive': ('false', '0'), 'MaskActive': ('false', '0'), 'MaskInverted': ('true', '1')}


def crs_values(text, name):
    """Every value of crs:`name` in an XMP packet, as attribute or element."""
    return [m.group(1) or m.group(2) for m in
            re.finditer(rf'crs:{name}="([^"]*)"|<crs:{name}>([^<]*)</crs:{name}>', text)]


def has_gate_mask(text):
    def matches(value, want):
        try:
            return abs(float(value) - want) <= 1e-4 if not isinstance(want, str) else value == want
        except ValueError:
            return False
    return (all(any(matches(v, want) for v in crs_values(text, name)) for name, want in MASK_VALUES.items())
            and not any(v.strip().lower() in off for name, off in MASK_OFF.items() for v in crs_values(text, name)))


# How render runs rawmakase (its environment); a change makes earlier renders stale.
RENDER_POLICY = 2
# Independent of render identity: old pixels can be rescored, old summaries
# cannot supply the regional measurements introduced here.
METRIC_POLICY = 2
LOCAL_REGRESSION_LIMIT = 2.0  # ΔE00 in any matched region, not pooled over photos
PARITY_MEDIAN_LIMIT = 2.0
PARITY_LOCAL_LIMIT = 5.0
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
            ('dehaze+40+clarity+40', {'Dehaze': '40', 'Clarity2012': '40'}, 40),
            # Shadows with Highlights (the `pairs` sets): Camera Raw's render of the two is
            # not the sum of each alone, so every sign combination is its own case; then
            # unequal sliders, and the pair beside Whites and Blacks.
            ('shadows+100+highlights-100', {'Shadows2012': '100', 'Highlights2012': '-100'}, 100),
            ('shadows-100+highlights+100', {'Shadows2012': '-100', 'Highlights2012': '100'}, 100),
            ('shadows+100+highlights+100', {'Shadows2012': '100', 'Highlights2012': '100'}, 100),
            ('shadows-100+highlights-100', {'Shadows2012': '-100', 'Highlights2012': '-100'}, 100),
            ('shadows+80+highlights-30', {'Shadows2012': '80', 'Highlights2012': '-30'}, 80),
            ('shadows+60+highlights-60+whites+30+blacks-30',
             {'Shadows2012': '60', 'Highlights2012': '-60', 'Whites2012': '30', 'Blacks2012': '-30'}, 60)]:
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
    if not valid.all():
        return dict(valid=0., error='non-finite pixels')
    clip = (at_white(a) ^ at_white(b)) | (at_black(a) ^ at_black(b))
    # Overlapping windows, 1/16 of the long edge, at half-window steps. Keep
    # each window so a pre-existing error elsewhere cannot hide a new defect.
    edge = max(1, round(max(d.shape) / 16))
    step = max(1, edge // 2)
    windows = lambda n: sorted(set(range(0, max(1, n - edge + 1), step)) | {max(0, n - edge)})
    regions = [float(d[y:y + edge, x:x + edge].mean())
               for y in windows(d.shape[0]) for x in windows(d.shape[1])]
    # Reference-defined masks include tiny bright/dark objects (>=16 pixels),
    # independently of their share of the whole image; no signed cancellation.
    tones = {}
    for name, mask in (('highlights', la[..., 0] >= 90), ('shadows', la[..., 0] <= 10)):
        if mask.sum() >= 16:
            tones[name] = float(d[mask].mean())
    return dict(median_de00=float(np.median(d[valid])), mean_de00=float(d[valid].mean()),
                regions_de00=regions, tone_de00=tones,
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
    # Existing renders are reused by photo index, so only for the same photos.
    photos = [p['sha256'] for p in manifest['photos']]
    stamp = out / 'photos.json'
    rendered = (out / 'references').is_dir() and any((out / 'references').iterdir())
    if (stamp.exists() and json.loads(stamp.read_text()) != photos) or (rendered and not stamp.exists()):
        raise ValueError(f'{out} holds references of other photos: use a new folder')
    (out / 'references').mkdir(parents=True, exist_ok=True)
    stamp.write_text(json.dumps(photos))
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
    # A set frozen without some of its cases would never be scored on them.
    missing = [c['reference'] for c in plan if not (out / c['reference']).exists()]
    if missing:
        raise ValueError(f'{len(missing)} references were not rendered, e.g. {missing[0]}: nothing frozen')
    frozen = []
    for c in plan:
        target = out / c['reference']
        # Always from this TIFF: a sidecar left from an earlier render would vouch for it.
        xmp = target.with_suffix('.xmp')
        xmp.write_bytes(subprocess.check_output(['exiftool', '-b', '-XMP', str(target)]))
        s, profile, _, extra = chosen[c['name']]
        base, look = camera_profile(profile)
        # The mask case's settings are all in its mask, which the tags below don't list.
        text = xmp.read_text(errors='replace')
        if extra and not has_gate_mask(text):
            raise ValueError(f'{target.name}: the sidecar lacks the gate\'s gradient mask')
        tags = json.loads(subprocess.check_output(['exiftool', '-j', '-n', '-XMP-crs:all', str(xmp)]))[0]
        if tags.get('CameraProfile') != base or (look and tags.get('LookName') != profile):
            raise ValueError(f'{target.name}: rendered with {tags.get("CameraProfile")!r} '
                             f'{tags.get("LookName", "")!r}, expected {profile!r}')
        # Every setting the reference was asked for, the base included: a sidecar
        # that lacks one, or holds another value, would freeze another render
        # under this case's name.
        for k, v in dict(camera_raw.BASE, **s).items():
            got = tags.get(k)
            try:
                same = np.isfinite(float(got)) and abs(float(got) - float(v)) <= 1e-4
            except (TypeError, ValueError):
                same = str(got) == str(v)
            if not same:
                raise ValueError(f'{target.name}: {k} = {got!r}, expected {v}')
        frozen.append(dict(c, reference_sha256=digest(target), xmp=str(xmp.relative_to(out)), xmp_sha256=digest(xmp)))
    (out / 'references.json').write_text(json.dumps(dict(
        schema=1, camera_raw=camera_raw.version(), set=manifest['set'], photos=manifest['photos'], cases=frozen),
        indent=1))
    print(f'{len(frozen)} references frozen in {out / "references.json"}')


def library_digest(folder):
    """One hash over every file in a profile library: names and contents."""
    h = hashlib.sha256()
    for f in sorted(p for p in Path(folder).rglob('*') if p.is_file()):
        h.update(str(f.relative_to(folder)).encode() + b'\0' + bytes.fromhex(digest(f)))
    return h.hexdigest()


def render_env(args, out):
    """The environment every gate render runs rawmakase in, and its profile library."""
    # The binary that is hashed is the one that runs, never one found on PATH.
    args.rawmakase = args.rawmakase.resolve(strict=True)
    env = dict(os.environ)
    if args.data_dir:
        env['RAWMAKASE_DATA_DIR'] = str(args.data_dir)
    # The profile library decides which Adobe profile a render uses, so it is
    # named explicitly and identified by its contents.
    if not env.get('RAWMAKASE_DATA_DIR'):
        raise ValueError('Pass --data-dir (or set RAWMAKASE_DATA_DIR): the library with the Adobe profiles')
    library = Path(env['RAWMAKASE_DATA_DIR']) / 'camera-profiles'
    # Nothing else decides how a render is made: no profiles or other assets from
    # the user's data folder ($XDG_DATA_HOME/rawmakase).
    (out / 'empty-data-home').mkdir(parents=True, exist_ok=True)
    env['XDG_DATA_HOME'] = str(out / 'empty-data-home')
    return env, library


def default_parity(args):
    """Every surveyed photo's default render (Adobe Standard, the settings Camera Raw
    embedded in its survey render) against Camera Raw's, at the survey's 1024 px.
    Photos in a holdout set are left out, so this never looks at them."""
    survey = args.survey.resolve()
    pool = json.loads((survey / 'pool.json').read_text())
    references = (args.references or survey.parent.parent).resolve()
    # The pool names its RAWs relative to the dataset folder, references/'s parent.
    root = references.parent
    held = [p for f in references.glob('*/*-holdout/references.json') for p in json.loads(f.read_text())['photos']]
    # By file and by scene: other files of a held-out scene (raw.pixls.us keeps a
    # camera's modes and bit depths side by side) are held out too, as split() does.
    holdout = {p['sha256'] for p in held}
    held_scenes = {scene_key(p) for p in held}
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    env, library = render_env(args, out)
    # Rendered with a copy of the build taken now, so a rebuild meanwhile can't mix engines.
    pinned = out / 'rawmakase'
    shutil.copy2(args.rawmakase, pinned)
    args.rawmakase = pinned
    rows, left_out, crashed = [], 0, []
    for photo in pool['photos']:
        if photo['sha256'] in holdout or scene_key(photo) in held_scenes:
            left_out += 1
            continue
        key = photo['sha256'][:16]
        reference = survey / 'renders' / f'{key}.tif'
        row = dict(sha256=photo['sha256'], make=photo.get('make'), stratum=photo['stratum'])
        if not reference.exists():
            rows.append(dict(row, valid=0., error='no survey render'))
            continue
        raw = root / photo['raw']
        if digest(raw) != photo['sha256']:
            raise ValueError(f'{photo["raw"]}: changed since the survey')
        # The settings Camera Raw rendered with, from its TIFF, as `reference` does.
        sidecar = out / f'{key}.xmp'
        sidecar.write_bytes(subprocess.check_output(['exiftool', '-b', '-XMP', str(reference)]))
        tif = out / f'{key}.tif'
        tif.unlink(missing_ok=True)
        listing = out / f'{key}.json'
        listing.write_text(json.dumps([dict(xmp=str(sidecar), output=str(tif), max_edge=1024)]))
        result = subprocess.run([str(args.rawmakase), 'render-batch', str(raw), str(listing)],
                                capture_output=True, text=True, env=env)
        listing.unlink()
        if result.returncode:
            # rawmakase itself failed (a crash, the RAW unreadable): not a result.
            crashed.append(photo['sha256'][:12])
            rows.append(dict(row, valid=0., error=f'rawmakase failed (exit {result.returncode})'))
            continue
        if not tif.exists():
            # render-batch prints "<output>: <why>"; keep the why, not the path.
            why = (result.stderr.strip().splitlines() or ['no message'])[-1].split(': ', 1)[-1][:200]
            rows.append(dict(row, valid=0., error=f'not rendered: {why}'))
            continue
        m = photo_metrics(reduce(reference_srgb(reference)), reduce(render_srgb(tif)))
        m.pop('L_ref', None), m.pop('L_out', None)
        rows.append(dict(row, **m))
        tif.unlink()
    valid = [r for r in rows if r.get('valid', 0) >= 0.5]
    summary = {}
    for key in ('stratum', 'make'):
        groups = {}
        for r in valid:
            groups.setdefault(r[key], []).append(r['median_de00'])
        summary[key] = {k: dict(n=len(v), median=float(np.median(v))) for k, v in sorted(groups.items())}
    result = dict(schema=2, metric_policy=METRIC_POLICY,
                  survey=str(survey), camera_raw=pool.get('camera_raw'), holdout_left_out=left_out,
                  rendered_by=dict(rawmakase=digest(args.rawmakase), profiles=library_digest(library),
                                   policy=RENDER_POLICY),
                  median_de00=float(np.median([r['median_de00'] for r in valid])) if valid else None,
                  summary=summary, photos=rows)
    (out / 'default-parity.json').write_text(json.dumps(result, indent=1))
    print(f'{len(valid)} of {len(rows)} photos scored ({left_out} holdout photos left out); '
          f'median ΔE00 {result["median_de00"]}')
    for st, v in summary['stratum'].items():
        print(f'  {st}: {v["n"]} photos, median {v["median"]:.2f}')
    if crashed:
        print(f'rawmakase failed on {len(crashed)} photos ({", ".join(crashed)}): the result is incomplete',
              file=sys.stderr)
        return 1


def render(args):
    """One `rawmakase render-batch` per photo: it develops the RAW once."""
    refs = args.refs.resolve()
    frozen = json.loads((refs / 'references.json').read_text())
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    env, library = render_env(args, out)
    # Renders, new or kept from an earlier run, must come from the frozen inputs.
    for photo in frozen['photos']:
        if digest(photo['raw']) != photo['sha256']:
            raise ValueError(f'{photo["raw"]}: changed since the references were frozen')
    for c in frozen['cases']:
        if digest(refs / c['xmp']) != c['xmp_sha256']:
            raise ValueError(f'{c["xmp"]}: changed since it was frozen')
    # Renders already in `out` are kept only for the same references, build and
    # profile library; anything else needs a folder of its own.
    made_by = dict(references=digest(refs / 'references.json'), rawmakase=digest(args.rawmakase),
                   profiles=library_digest(library), policy=RENDER_POLICY)
    stamp = out / 'rendered-by.json'
    kept = any(p.suffix in ('.tif', '.npy') for p in out.iterdir())
    if (stamp.exists() and json.loads(stamp.read_text()) != made_by) or (kept and not stamp.exists()):
        raise ValueError(f'{out} holds renders of other references or another build: use a new folder')
    stamp.write_text(json.dumps(made_by))
    failed = []
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
        # render-batch reports a case it can't render and goes on (a result to
        # score); exiting with an error means the photo could not be developed
        # at all this time (a crash, the RAW unreadable), which a rerun may not
        # repeat, so the run is not complete.
        if result.returncode:
            failed.append(i)
        # Keep what the metric reads: the render reduced to SCORE_EDGE.
        for job in jobs:
            tif = Path(job['output'])
            if tif.exists():
                np.save(tif.with_suffix('.npy'), reduce(render_srgb(tif)).astype(np.float32))
                tif.unlink()
    if failed:
        print(f'rawmakase failed on photos {failed}: rerun render', file=sys.stderr)
        return 1


# Scoring and acceptance -----------------------------------------------------------

def score(args):
    refs = args.refs.resolve()
    frozen = json.loads((refs / 'references.json').read_text())
    # Renders are matched to cases by file name, so only renders made for these
    # references are scored.
    stamp = args.renders / 'rendered-by.json'
    made_by = json.loads(stamp.read_text()) if stamp.exists() else None
    if not made_by or made_by.get('references') != digest(refs / 'references.json') \
            or made_by.get('policy') != RENDER_POLICY:
        raise ValueError(f'{args.renders} was not rendered for {refs} by `render`')
    rows, pixels = [], {}
    for c in frozen['cases']:
        ref = refs / c['reference']
        out = args.renders / ref.name
        photo = frozen['photos'][c['photo']]
        row = dict(photo=c['photo'], sha256=photo['sha256'], name=c['name'], stratum=photo['stratum'])
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
    args.out.write_text(json.dumps(dict(schema=2, metric_policy=METRIC_POLICY,
                                        settings=sorted({c['name'] for c in frozen['cases']}),
                                        set=frozen['set'], rendered_by=made_by, photos=rows,
                                        bands=band_rows), indent=1))
    print(f'{len(rows)} scores -> {args.out}')


def summarise(scores):
    """setting -> stratum -> dict(score, mean, photos{i: score}, clipping, n)."""
    out = {}
    for r in scores['photos']:
        s = out.setdefault(r['name'], {}).setdefault(r['stratum'], dict(photos={}, clipping=[], invalid={}, rows={}))
        if r.get('valid', 0) < 0.5:
            # With why: a photo framed otherwise than Camera Raw in both is the
            # same limitation; one that is no longer rendered is not.
            s['invalid'][r.get('sha256', r['photo'])] = r.get('error')
            continue
        # By the RAW's hash: photo indices restart in every set.
        s['photos'][r.get('sha256', r['photo'])] = r['median_de00']
        s['rows'][r.get('sha256', r['photo'])] = r
        s.setdefault('clipped', {})[r.get('sha256', r['photo'])] = r['clipping']
        s['clipping'].append(r['clipping'])
    for setting in out.values():
        for s in setting.values():
            v = list(s['photos'].values())
            s['n'] = len(v)
            s['score'] = float(np.median(v)) if v else float('nan')
            s['mean'] = float(np.mean(v)) if v else float('nan')
            s['clipping'] = float(np.mean(s['clipping'])) if s['clipping'] else float('nan')
    return out


# The acceptance rules. Every setting: no photo more than a limit worse than the
# baseline (rule 3) and no more clipping (rule 4); defaults: the median no more
# than 0.1 ΔE00 worse (rule 5). Other settings, by --rule: `regression` (the
# default, for any change) holds their median to the same 0.1; `switch` (the
# scene tone engine's switch, docs/scene-tone-stage.md) asks tone settings far
# from the default render to improve materially (rule 1) without a band more
# than 3 L* off (rule 2). Color families always take the regression rule, with
# per-photo limits of their own: half the 90th percentile of the family's
# per-photo ΔE00 in `main`'s baseline, between 1.0 and 2.0 (the tone limits).
# Derived with `color-limits` from main 418d0ca7's validation
# scores (Camera Raw 18.7, all three sources); frozen, change only with a reason.
COLOR_LIMITS = {'color-wb': 1.9, 'color-vibrance': 2.0, 'color-mixer-skin': 1.4, 'color-mixer-sky': 2.0,
                'color-mixer-foliage': 2.0, 'color-grading': 1.7}


def color_family(set_name):
    """The color family a set belongs to (`color-wb-validation` -> `color-wb`), or None."""
    family = (set_name or '').rsplit('-', 1)[0]
    return family if family in COLOR_LIMITS else None


def color_limits(args):
    """Derive COLOR_LIMITS from a baseline's score files."""
    per = {}
    for f in sorted(Path(args.scores).rglob('*.json')):
        d = json.loads(f.read_text())
        family = (d.get('set') or '').rsplit('-', 1)[0]
        if family.startswith('color-') and family != 'color-default':
            per.setdefault(family, []).extend(r['median_de00'] for r in d['photos']
                                              if r.get('valid', 0) >= 0.5 and not r['name'].endswith('default'))
    for family, v in sorted(per.items()):
        p90 = float(np.percentile(v, 90))
        print(f"'{family}': {min(2.0, max(1.0, round(0.5 * p90, 1)))},  # n {len(v)}, median {np.median(v):.2f}, "
              f"p90 {p90:.2f}")


def excluded(path):
    """SHA-256 hashes (or prefixes), one per line, `#` comments allowed."""
    if not path:
        return ()
    return tuple(line.split('#')[0].strip() for line in Path(path).read_text().splitlines()
                 if line.split('#')[0].strip())


def accept(args):
    cand, base = json.loads(args.candidate.read_text()), json.loads(args.baseline.read_text())
    for label, doc in (('candidate', cand), ('baseline', base)):
        if doc.get('metric_policy') != METRIC_POLICY:
            print(f'{label}: metric policy {METRIC_POLICY} required; rescore the renders in a new run folder')
            return 1
        if not doc.get('settings'):
            print(f'{label}: missing declared settings')
            return 1
    # Photos left out of the verdict (holdout photos already looked at): dropped
    # from both sides, and said so.
    skip = excluded(args.exclude)
    # Band measurements pool the photos' pixels, so they can't drop one: pairs that
    # lost a photo can't be judged on rule 2.
    pooled_with_excluded = set()
    if skip:
        for x in (cand, base):
            kept = [r for r in x['photos'] if not str(r.get('sha256', '')).startswith(skip)]
            pooled_with_excluded |= {(r['name'], r['stratum']) for r in x['photos'] if r not in kept}
            print(f'{len(x["photos"]) - len(kept)} cases of excluded photos left out')
            x['photos'] = kept
    if cand.get('set') != base.get('set'):
        print(f'the candidate scores set {cand.get("set")!r}, the baseline {base.get("set")!r}')
        return 1
    # Both against the same frozen references and with the same profiles; only the
    # build may differ.
    if any((x.get('rendered_by') or {}).get('policy') != RENDER_POLICY for x in (cand, base)):
        print(f'scores of renders made under another render policy than {RENDER_POLICY}: render and score again')
        return 1
    for key in ('references', 'profiles'):
        if (cand.get('rendered_by') or {}).get(key) != (base.get('rendered_by') or {}).get(key) or \
                not (cand.get('rendered_by') or {}).get(key):
            print(f'the candidate and baseline were not rendered with the same {key}')
            return 1
    C, B = summarise(cand), summarise(base)
    magnitude = {k: v[2] for k, v in settings().items()}
    family = color_family(cand.get('set'))
    failed = []
    print(f'{"setting":28s} {"stratum":7s} {"n":>2s} {"e4":>6s} {"new":>6s} {"dflt":>6s}  result')
    # Every setting either side scored: one the candidate lacks fails, as does an
    # empty candidate.
    names = sorted(C.keys() | B.keys() | set(cand['settings']) | set(base['settings']),
                   key=lambda n: (n != 'default', n))
    if not names:
        print('no scores')
        return 1
    for name in names:
        default_name = 'color-default' if name.startswith('color-') else 'default'
        for st, _, _ in STRATA:
            c, b = C.get(name, {}).get(st), B.get(name, {}).get(st)
            cd, bd = C.get(default_name, {}).get(st), B.get(default_name, {}).get(st)
            reasons = []
            if not c and not b:
                reasons.append('inconclusive (no reference photos in this stratum)')
            elif not c or not b:
                reasons.append('missing from the ' + ('candidate' if not c else 'baseline'))
            elif any(b['invalid'].get(p, 'scored') != why for p, why in c['invalid'].items()):
                # A photo the candidate can't score as the baseline could, or for
                # another reason (no longer rendered, say).
                n = sum(b['invalid'].get(p, 'scored') != why for p, why in c['invalid'].items())
                reasons.append(f'{n} photos no longer render or score as in the baseline')
            elif (c['n'] < 3 or b['n'] < 3 or c['invalid'] or b['invalid'] or not cd or not bd
                  or c['photos'].keys() != b['photos'].keys()):
                # Unscored references are coverage gaps even when both engines
                # fail on them. Still check the photos both sides did score.
                reasons.append('inconclusive (fewer than 3 photos, unscored references or different photos)')
                limit = COLOR_LIMITS[family] if family else 1.0 if magnitude.get(name, 0) <= 50 else 2.0
                common = c['photos'].keys() & b['photos'].keys()
                for p in common:
                    if c['photos'][p] > b['photos'][p] + limit:
                        reasons.append(f'3: photo {str(p)[:12]} {b["photos"][p]:.2f} -> {c["photos"][p]:.2f}')
                if common:
                    cc = float(np.mean([c['clipped'][p] for p in common]))
                    bc = float(np.mean([b['clipped'][p] for p in common]))
                    if cc > bc + 0.005:
                        reasons.append(f'4: clipping {100 * bc:.2f}% -> {100 * cc:.2f}%')
            else:
                if name.endswith('default'):
                    if c['score'] > b['score'] + 0.1:
                        reasons.append('5: default worse than engine 4 + 0.1')
                elif family or args.rule in ('regression', 'parity'):
                    if c['score'] > b['score'] + 0.1:
                        reasons.append('R: median worse than the baseline + 0.1')
                elif name not in magnitude:
                    reasons.append('unknown setting: no acceptance rule')
                else:
                    near = b['score'] <= bd['score'] + 0.5
                    if not near:
                        if not (c['score'] <= cd['score'] + 1.0 or c['score'] <= 0.6 * b['score']):
                            reasons.append('1: no material improvement')
                        if (name, st) in pooled_with_excluded:
                            reasons.append('inconclusive on rule 2: its bands pool excluded photos')
                        cb = cand['bands'].get(f'{name}|{st}', {})
                        bb = base['bands'].get(f'{name}|{st}', {})
                        if not cb or cb.keys() != bb.keys() or not all(np.isfinite(v) for v in cb.values()):
                            reasons.append('2: band measurements missing')
                        for band, v in cb.items():
                            if abs(v) > 3 and not (band in bb and abs(v) <= 0.5 * abs(bb[band])):
                                reasons.append(f'2: band {band} ΔL* {v:+.1f}')
                # One photo much worse, or more clipping, fails every setting,
                # the defaults too.
                limit = COLOR_LIMITS[family] if family else 1.0 if magnitude.get(name, 0) <= 50 else 2.0
                for p, v in c['photos'].items():
                    if p in b['photos'] and v > b['photos'][p] + limit:
                        reasons.append(f'3: photo {str(p)[:12]} {b["photos"][p]:.2f} -> {v:.2f}')
                if c['clipping'] > b['clipping'] + 0.005:
                    reasons.append(f'4: clipping {100 * b["clipping"]:.2f}% -> {100 * c["clipping"]:.2f}%')
            # Regional and absolute checks apply even to underfilled strata.
            # Invalid measurements never become a successful parity verdict.
            if c and b:
                for p in c['photos'].keys() & b['photos'].keys():
                    cr, br = c['rows'][p], b['rows'][p]
                    reasons += regional_reasons(cr, br, args.rule, str(p)[:12])
                if args.rule == 'parity' and c['invalid']:
                    reasons.append('parity: unscored reference photos')
            dflt = cd['score'] if cd else float('nan')
            print(f'{name:28s} {st:7s} {c["n"] if c else 0:2d} {b["score"] if b else float("nan"):6.2f} '
                  f'{c["score"] if c else float("nan"):6.2f} {dflt:6.2f}  {"PASS" if not reasons else "; ".join(reasons)}')
            if reasons:
                failed.append((name, st))
            # For callers that judge many sets (dataset-gate.py): every pair's reasons.
            if getattr(args, 'results', None) is not None:
                args.results.append(dict(name=name, stratum=st, reasons=reasons))
    print(f'{len(failed)} failing setting/stratum pairs')
    return int(bool(failed))


def regional_reasons(candidate, baseline, rule, photo):
    """Compare matching spatial/tonal regions, never averages across photos."""
    reasons = []
    for key in ('regions_de00', 'tone_de00'):
        c, b = candidate.get(key), baseline.get(key)
        if key == 'tone_de00':
            if not isinstance(c, dict) or not isinstance(b, dict) or c.keys() != b.keys():
                reasons.append(f'regional: photo {photo} missing or different tone regions')
                continue
            c, b = list(c.values()), [b[k] for k in candidate[key]]
        elif not isinstance(c, list) or not isinstance(b, list) or not c or len(c) != len(b):
            reasons.append(f'regional: photo {photo} missing or different spatial regions')
            continue
        if not all(isinstance(v, (int, float)) and np.isfinite(v) and v >= 0 for v in c + b):
            reasons.append(f'regional: photo {photo} invalid measurements')
            continue
        if any(cv > bv + LOCAL_REGRESSION_LIMIT for cv, bv in zip(c, b)):
            reasons.append(f'regional: photo {photo} {key} worsens by > {LOCAL_REGRESSION_LIMIT} ΔE00')
        if rule == 'parity' and any(v > PARITY_LOCAL_LIMIT for v in c):
            reasons.append(f'parity: photo {photo} {key} exceeds {PARITY_LOCAL_LIMIT} ΔE00')
    for key in ('median_de00', 'mean_de00', 'clipping'):
        if not all(isinstance(r.get(key), (int, float)) and np.isfinite(r[key]) and r[key] >= 0
                   for r in (candidate, baseline)):
            reasons.append(f'regional: photo {photo} invalid {key}')
    if reasons:
        return reasons
    if candidate['clipping'] > baseline['clipping'] + 0.005:
        reasons.append(f'4: photo {photo} clipping worsens by > 0.5 percentage points')
    if rule == 'parity':
        if candidate['median_de00'] > PARITY_MEDIAN_LIMIT:
            reasons.append(f'parity: photo {photo} median exceeds {PARITY_MEDIAN_LIMIT} ΔE00')
        if candidate['clipping'] > 0.005:
            reasons.append(f'parity: photo {photo} clipping disagreement exceeds 0.5%')
    return reasons


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
    s.add_argument('--exclude', type=Path, help='photos to leave out: SHA-256 hashes or prefixes, one per line')
    s.add_argument('--rule', choices=['regression', 'switch', 'parity'], default='regression',
                   help='regression (default): no setting worse than the baseline; switch: the scene tone '
                        'engine\'s improvement rule; parity: regression plus absolute per-photo and regional limits')
    s = sub.add_parser('default-parity', help='default renders against the survey\'s Camera Raw renders')
    s.add_argument('--survey', type=Path, required=True, help='a references/<family>/survey folder')
    s.add_argument('--references', type=Path, help='the references root, for the holdout sets to leave out')
    s.add_argument('--rawmakase', type=Path, required=True)
    s.add_argument('--data-dir', type=Path)
    s.add_argument('--out', type=Path, required=True)
    s = sub.add_parser('color-limits', help='derive COLOR_LIMITS from a baseline')
    s.add_argument('--scores', type=Path, required=True, help='a folder of the baseline\'s score files')
    args = p.parse_args()
    return {'survey': survey, 'split': split, 'reference': reference, 'render': render, 'score': score,
            'color-limits': color_limits, 'default-parity': default_parity,
            'accept': accept}[args.command](args)


if __name__ == '__main__':
    raise SystemExit(main())
