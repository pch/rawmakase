#!/usr/bin/env python3
"""Score RAWmakase against Lightroom exports.

Each reference must be a Lightroom TIFF/JPEG export with its develop settings embedded
as XMP (Lightroom's default when metadata is included). The script finds the source
RAW by crs:RawFileName, renders it with `rawmakase render --xmp`, and reports encoded-sRGB
MAE and CIE76 color difference overall, in the centre and in the corners. No
alignment, exposure or color fitting is applied.

Requires numpy, Pillow and exiftool. RAW files are copied into the output directory so
existing sidecars are never read or written. Profiles that the references need
(e.g. the camera's Adobe Standard DCP and Adobe Raw looks) are imported into a private
data directory with --profiles.
"""
import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import numpy as np
from PIL import Image

RAW_SUFFIXES = {'.raf', '.arw', '.dng', '.nef', '.cr2', '.cr3', '.orf', '.rw2'}


def exif(path, *tags):
    out = subprocess.run(['exiftool', '-j', '-n', *tags, str(path)],
                         capture_output=True, text=True, check=True).stdout
    return json.loads(out)[0]


def load(path, size):
    im = Image.open(path)
    a = np.asarray(im)
    a = a.astype(np.float64) / (65535 if a.dtype == np.uint16 else 255)
    if a.ndim == 2:
        a = np.repeat(a[..., None], 3, -1)
    a = a[..., :3]
    return np.stack([np.asarray(Image.fromarray(a[..., c].astype(np.float32), 'F')
                                .resize(size, Image.Resampling.BOX)) for c in range(3)], -1)


def lab(rgb):
    lin = np.where(rgb <= 0.04045, rgb / 12.92, ((rgb + 0.055) / 1.055) ** 2.4)
    m = np.array([[0.4124, 0.3576, 0.1805], [0.2126, 0.7152, 0.0722], [0.0193, 0.1192, 0.9505]])
    xyz = lin @ m.T / np.array([0.95047, 1., 1.08883])
    f = np.where(xyz > 216 / 24389, np.cbrt(xyz), (24389 / 27 * xyz + 16) / 116)
    return np.stack([116 * f[..., 1] - 16, 500 * (f[..., 0] - f[..., 1]),
                     200 * (f[..., 1] - f[..., 2])], -1), lin


def score(reference, candidate, long_edge):
    ref = Image.open(reference)
    cand = Image.open(candidate)
    ratio = (ref.width / ref.height) / (cand.width / cand.height)
    if abs(ratio - 1) > 0.005:
        raise ValueError(f'aspect ratio differs by {abs(ratio - 1):.3%}')
    s = long_edge / max(ref.size)
    size = (round(ref.width * s), round(ref.height * s))
    a, b = load(reference, size), load(candidate, size)
    h, w, _ = a.shape
    yy, xx = np.mgrid[0:h, 0:w]
    r = np.hypot((xx + 0.5 - w / 2) / (w / 2), (yy + 0.5 - h / 2) / (h / 2)) / np.sqrt(2)
    la, lina = lab(a)
    lb, linb = lab(b)
    d = np.abs(a - b).mean(-1)
    de = np.linalg.norm(la - lb, axis=-1)
    centre, corners = r < 0.3, r > 0.8
    ev = lambda m: float(np.log2(linb[m].mean() / max(lina[m].mean(), 1e-9)))
    return dict(mae=float(d.mean()), mae_centre=float(d[centre].mean()),
                mae_corners=float(d[corners].mean()), de=float(de.mean()),
                de_p95=float(np.percentile(de, 95)), ev_centre=ev(centre), ev_corners=ev(corners))


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--references', type=Path, required=True, nargs='+',
                   help='Lightroom exports, or directories containing them')
    p.add_argument('--raws', type=Path, required=True, nargs='+', help='directories to search for source RAWs')
    p.add_argument('--out', type=Path, required=True)
    p.add_argument('--rawmakase', type=Path, default=Path('target/release/rawmakase'))
    p.add_argument('--profiles', type=Path, nargs='*', default=[], help='DCP/XMP profiles to import first')
    p.add_argument('--long-edge', type=int, default=1200)
    p.add_argument('--label', default='', help='name recorded with the results')
    args = p.parse_args()

    refs = sorted(f for d in args.references for f in ([d] if d.is_file() else d.iterdir())
                  if f.suffix.lower() in {'.tif', '.tiff', '.jpg', '.jpeg'})
    raws = {}
    for d in args.raws:
        for root, _, files in os.walk(d):
            for f in files:
                if Path(f).suffix.lower() in RAW_SUFFIXES:
                    raws.setdefault(f.lower(), Path(root) / f)
    for sub in ['raws', 'xmp', 'render', 'data']:
        (args.out / sub).mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, RAWMAKASE_DATA_DIR=str((args.out / 'data').resolve()))
    if args.profiles:
        subprocess.run([str(args.rawmakase), 'import-profiles', *map(str, args.profiles)],
                       env=env, check=True, capture_output=True)

    results = []
    for ref in refs:
        meta = exif(ref, '-XMP-crs:RawFileName', '-XMP-crs:ProcessVersion')
        name = meta.get('RawFileName')
        if not name and 'ProcessVersion' in meta:
            # Photoshop/Camera Raw exports carry settings but not the source name.
            stem = ref.stem.lower()
            name = next((raws[k].name for k in raws
                         if stem == Path(k).stem or stem.startswith(Path(k).stem + '-')), None)
        row = dict(reference=ref.name, raw=name)
        results.append(row)
        if not name or name.lower() not in raws:
            row['error'] = 'no embedded settings' if not name else 'source RAW not found'
            continue
        raw = args.out / 'raws' / name
        if not raw.exists():
            shutil.copy2(raws[name.lower()], raw)
        xmp = args.out / 'xmp' / (ref.stem + '.xmp')
        with open(xmp, 'wb') as f:
            subprocess.run(['exiftool', '-b', '-XMP', str(ref)], stdout=f, check=True)
        out = args.out / 'render' / (ref.stem + '.tif')
        run = subprocess.run([str(args.rawmakase), 'render', str(raw), str(out), '--xmp', str(xmp), '--overwrite'],
                             env=env, capture_output=True, text=True)
        if run.returncode:
            row['error'] = (run.stderr.strip().splitlines() or ['render failed'])[-1]
            continue
        try:
            row.update(score(ref, out, args.long_edge))
        except ValueError as e:
            row['error'] = str(e)

    ok = [r for r in results if 'mae' in r]
    summary = dict(label=args.label, count=len(ok), errors=len(results) - len(ok),
                   mae=float(np.mean([r['mae'] for r in ok])) if ok else None,
                   de=float(np.mean([r['de'] for r in ok])) if ok else None)
    (args.out / 'scorecard.json').write_text(json.dumps(dict(summary=summary, results=results), indent=2))
    print(f"{'reference':44} {'MAE':>7} {'centre':>7} {'corner':>7} {'ΔE':>6} {'ΔE95':>6} {'EV c':>6} {'EV k':>6}")
    for r in results:
        if 'mae' in r:
            print(f"{r['reference'][:44]:44} {r['mae']:7.4f} {r['mae_centre']:7.4f} {r['mae_corners']:7.4f} "
                  f"{r['de']:6.2f} {r['de_p95']:6.2f} {r['ev_centre']:+6.2f} {r['ev_corners']:+6.2f}")
        else:
            print(f"{r['reference'][:44]:44} {r['error']}")
    print(f"\n{summary['count']} scored, {summary['errors']} errors; mean MAE {summary['mae']}, mean ΔE {summary['de']}")


if __name__ == '__main__':
    sys.exit(main())
