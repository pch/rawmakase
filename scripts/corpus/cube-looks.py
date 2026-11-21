#!/usr/bin/env python3
"""Camera Raw references for looks made from `.cube` files (docs/lightroom-profiles.md#cube-luts).

`reference` renders photos with look profiles in Camera Raw through Photoshop, over
Adobe Standard as the gate does, at 1024 px on the long edge (16-bit ProPhoto TIFFs,
recompressed losslessly): a LUT is per-pixel color and the gate scores at 1024 anyway.
Each look is embedded in the render's sidecar (`crs:Look` with its `Table_` attributes),
so nothing is installed in Camera Raw. Every render's own sidecar is checked against
what was asked and the set is frozen in `references.json`, in the layout gate.py's
`render` and `score` read, so RAWmakase's side is

  gate.py render --refs OUT/<set> --rawmakase … --data-dir LIBRARY --out RENDERS
  gate.py score --refs OUT/<set> --renders RENDERS --out scores.json

with a LIBRARY holding Adobe Standard and the same looks (`library` makes one), and
`summary` groups the scores by look and setting.

  cube-looks.py reference --photos photos.json --looks DIR --cases CASES --set NAME --out OUT
      photos.json: [{"raw", "sha256", "stratum"}…]; DIR: the look XMPs (every *.xmp, or
      --only NAME… for some); CASES: a JSON object {case: {"amount": 0-2, "settings":
      {crs…}, "curves": {name: [points]}}}. Renders photo × look × case.
  cube-looks.py library --data-dir LIBRARY --looks DIR --out NEW
      a copy of LIBRARY (APFS clones where possible) with the look XMPs added.
  cube-looks.py summary SCORES… [--by look|case]

References and renders stay outside the repository. Requires numpy and tifffile.
"""
import argparse
import hashlib
import html
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

import numpy as np

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).parent))
import camera_raw  # noqa: E402

EDGE = 1024
# A 1024 px 16-bit RGB TIFF before compression, and room to spare.
BYTES_PER_RENDER = 1024 * 1024 * 6
HEADROOM = 3e9


def digest(path):
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        for block in iter(lambda: f.read(1 << 20), b''):
            h.update(block)
    return h.hexdigest()


def look_name(path):
    text = Path(path).read_text()
    m = re.search(r'<crs:Name>\s*<rdf:Alt>\s*<rdf:li[^>]*>([^<]*)<', text)
    return html.unescape(m.group(1) if m else re.search(r'xml:lang="x-default">([^<]*)<', text).group(1))


def case_key(look, case):
    """A case's file name: the look's digest prefix (look names may be private) and the case."""
    return f'{hashlib.sha256(look.encode()).hexdigest()[:12]}-{case}'


def compress(path):
    """Rewrites a TIFF with lossless deflate, after checking the pixels survive."""
    import tifffile
    with tifffile.TiffFile(path) as t:
        page = t.pages[0]
        pixels = page.asarray()
        icc = page.tags.get('InterColorProfile')
        xmp = page.tags.get('XMP')
        extratags = []
        if icc is not None:
            extratags.append((34675, 7, len(icc.value), icc.value, True))
        if xmp is not None:
            value = xmp.value if isinstance(xmp.value, bytes) else bytes(xmp.value)
            extratags.append((700, 1, len(value), value, True))
    temporary = path.with_suffix('.zip.tif')
    tifffile.imwrite(temporary, pixels, compression='zlib', photometric='rgb', extratags=extratags)
    if not np.array_equal(tifffile.imread(temporary), pixels):
        temporary.unlink()
        raise ValueError(f'{path}: compressed pixels differ')
    temporary.replace(path)


def reference(args):
    photos = json.loads(args.photos.read_text())
    cases = json.loads(args.cases.read_text())
    looks = sorted(p for p in args.looks.glob('*.xmp') if not args.only or p.stem in args.only)
    if not looks:
        sys.exit(f'no look XMPs in {args.looks}')
    out = (args.out / args.set).resolve()
    inputs = dict(schema=1, edge=EDGE, photos=photos, cases=cases,
                  looks={p.name: digest(p) for p in looks})
    stamp = out / 'inputs.json'
    if stamp.exists():
        if json.loads(stamp.read_text()) != inputs:
            raise ValueError('reference inputs changed: use a new --set')
    elif out.exists() and any(out.iterdir()):
        raise ValueError('reference inputs were not recorded: use a new --set')
    else:
        out.mkdir(parents=True, exist_ok=True)
        stamp.write_text(json.dumps(inputs, indent=1))
    (out / 'references').mkdir(parents=True, exist_ok=True)
    (out / 'looks').mkdir(exist_ok=True)
    jobs, plan = [], []
    for i, photo in enumerate(photos):
        if digest(photo['raw']) != photo['sha256']:
            raise ValueError(f'{photo["raw"]}: not the photo named by its SHA-256')
        for look in looks:
            # The look as it was rendered, kept with the set.
            kept = out / 'looks' / look.name
            if not kept.exists():
                shutil.copyfile(look, kept)
            elif digest(kept) != digest(look):
                # References of the set were made with the kept look; another one under
                # its name would be rendered by RAWmakase against them.
                sys.exit(f'{look.name} differs from the look {out} was rendered with: use a new --set')
            for name, case in cases.items():
                key = case_key(look.stem, name)
                target = out / 'references' / f'{i}-{key}.tif'
                settings = dict(case.get('settings', {}), CameraProfile='Adobe Standard')
                jobs.append(dict(source=photo['raw'], settings=settings, curves=case.get('curves'), out=str(target),
                                 look=dict(file=str(kept), amount=case.get('amount', 1)), edge=EDGE))
                plan.append(dict(photo=i, name=key, look=look.stem, case=name, reference=str(target.relative_to(out))))
    todo = [j for j in jobs if not Path(j['out']).exists()]
    need = len(todo) * BYTES_PER_RENDER + HEADROOM
    if shutil.disk_usage(out).free < need:
        sys.exit(f'{len(todo)} renders need {need / 1e9:.1f} GB free; {shutil.disk_usage(out).free / 1e9:.1f} GB is')
    print(f'{len(todo)} of {len(jobs)} references to render in {out}', flush=True)
    # In chunks, compressing as they come, so a full set never sits uncompressed.
    for start in range(0, len(todo), args.chunk):
        chunk = todo[start:start + args.chunk]
        for f in camera_raw.render(chunk, work=out / 'work', chunk=args.chunk):
            print('  failed:', f, file=sys.stderr)
        for j in chunk:
            if Path(j['out']).exists():
                compress(Path(j['out']))
        print(f'  {min(start + args.chunk, len(todo))} / {len(todo)}', flush=True)
    shutil.rmtree(out / 'work', ignore_errors=True)
    frozen, skipped = [], []
    for c in plan:
        target = out / c['reference']
        if not target.exists():
            skipped.append(dict(c, reason='not rendered'))
            continue
        xmp = target.with_suffix('.xmp')
        if not xmp.exists():
            xmp.write_bytes(subprocess.check_output(['exiftool', '-b', '-XMP', str(target)]))
        tags = json.loads(subprocess.check_output(['exiftool', '-j', '-n', '-XMP-crs:all', str(xmp)]))[0]
        case = cases[c['case']]
        wrong = []
        if tags.get('CameraProfile') != 'Adobe Standard':
            wrong.append(f'profile {tags.get("CameraProfile")!r}')
        if tags.get('LookName') != look_name(out / 'looks' / f'{c["look"]}.xmp'):
            wrong.append(f'look {tags.get("LookName")!r}')
        amount = tags.get('LookAmount', 1)
        if abs(float(amount) - float(case.get('amount', 1))) > 1e-4:
            wrong.append(f'amount {amount!r}')
        for k, v in case.get('settings', {}).items():
            got = tags.get(k)
            try:
                same = abs(float(got) - float(v)) <= 1e-4
            except (TypeError, ValueError):
                same = str(got).lower() == str(v).lower()
            if not same:
                wrong.append(f'{k} = {got!r}, expected {v}')
        if wrong:
            skipped.append(dict(c, reason='; '.join(wrong)))
            print(f'  {target.name}: {"; ".join(wrong)}', file=sys.stderr)
            continue
        frozen.append(dict(c, reference_sha256=digest(target), xmp=str(xmp.relative_to(out)), xmp_sha256=digest(xmp)))
    (out / 'references.json').write_text(json.dumps(dict(
        schema=1, camera_raw=camera_raw.version(), set=args.set, edge=EDGE, photos=photos,
        cases_spec=cases, cases=frozen, skipped=skipped), indent=1))
    print(f'{len(frozen)} references frozen, {len(skipped)} skipped -> {out / "references.json"}')


def library(args):
    source = args.data_dir / 'camera-profiles'
    target = args.out / 'camera-profiles'
    if target.exists():
        sys.exit(f'{target} exists')
    args.out.mkdir(parents=True, exist_ok=True)
    # APFS clones cost no space; elsewhere an ordinary copy.
    if subprocess.run(['cp', '-cR', str(source), str(target)], capture_output=True).returncode:
        shutil.copytree(source, target)
    for look in sorted(args.looks.glob('*.xmp')):
        if (target / look.name).exists():
            sys.exit(f'{look.name} is already in {source}')
        shutil.copyfile(look, target / look.name)
    print(f'{target}: {sum(1 for _ in target.iterdir())} files')


def summary(args):
    rows = []
    for path in args.scores:
        doc = json.loads(Path(path).read_text())
        refs = json.loads((Path(args.refs) / 'references.json').read_text()) if args.refs else None
        names = {c['name']: c for c in refs['cases']} if refs else {}
        for r in doc['photos']:
            c = names.get(r['name'], {})
            rows.append(dict(r, look=c.get('look', r['name'][:12]), case=c.get('case', r['name'][13:])))
    valid = [r for r in rows if r.get('valid', 0) >= 0.5]
    print(f'{len(valid)} of {len(rows)} scored')
    groups = {}
    for r in valid:
        groups.setdefault(r[args.by], []).append(r)
    for key, g in sorted(groups.items(), key=lambda kv: np.median([r['median_de00'] for r in kv[1]])):
        med = [r['median_de00'] for r in g]
        print(f'  {key:40s} n {len(g):4d}  median ΔE00 {np.median(med):.2f}  mean of medians {np.mean(med):.2f}'
              f'  worst {max(med):.2f}')
    med = [r['median_de00'] for r in valid]
    if med:
        print(f'  all: median {np.median(med):.2f}, p90 {np.percentile(med, 90):.2f}, worst {max(med):.2f}')


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest='command', required=True)
    s = sub.add_parser('reference')
    s.add_argument('--photos', type=Path, required=True)
    s.add_argument('--looks', type=Path, required=True)
    s.add_argument('--only', nargs='*', help='only these looks (file stems)')
    s.add_argument('--cases', type=Path, required=True)
    s.add_argument('--set', required=True)
    s.add_argument('--out', type=Path, required=True)
    s.add_argument('--chunk', type=int, default=40)
    s = sub.add_parser('library')
    s.add_argument('--data-dir', type=Path, required=True)
    s.add_argument('--looks', type=Path, required=True)
    s.add_argument('--out', type=Path, required=True)
    s = sub.add_parser('summary')
    s.add_argument('scores', nargs='+')
    s.add_argument('--refs', help='the set the scores are of, to name looks and cases')
    s.add_argument('--by', choices=['look', 'case', 'stratum'], default='case')
    args = p.parse_args()
    return {'reference': reference, 'library': library, 'summary': summary}[args.command](args)


if __name__ == '__main__':
    sys.exit(main())
