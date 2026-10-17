#!/usr/bin/env python3
"""Write data/raw-matrices.toml, the XYZ (D65) to camera matrices of RAWmakase's
own RAW decoders, from an Adobe DNG Converter sweep.

The sweep is a JSON-lines file of one sample per camera and raw mode, each
{"sha": ..., "adobe": {exiftool -n of the converted DNG}} (Adobe's tags only;
nothing of the RAW or Adobe's profiles is kept). The camera's name is the one
RAWmakase gives it, from the decode fingerprints recorded for the same files
(tests/corpus/dataset/pixls-decode.json and the local baselines). A camera's
matrix is the converted DNG's ColorMatrix for illuminant D65 (21), from a native
RAW when the camera has one, else from a DNG the camera wrote.

--compare splits the rows: those whose matrix equals the cam_xyz RAWmakase read
for every file of the camera (written), and those that differ (listed, for a
separate, reviewed change).

  python3 scripts/cameras/raw-matrices.py --sweep sweep.jsonl \
      --fingerprints $RAWMAKASE_DATASET/baselines/decode/*.json --compare --write
"""
import argparse
import json
import sys
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TABLE = ROOT / 'data/raw-matrices.toml'
COMMITTED = ROOT / 'tests/corpus/dataset/pixls-decode.json'
HEADER = """# XYZ (D65) to camera matrices, in ten-thousandths, for RAWmakase's own RAW
# decoders (crates/rawmakase-raw/src/facts.rs). source is adobe-dng: the
# ColorMatrix for illuminant D65 that Adobe DNG Converter 18.7 writes for the
# camera (2026-10-16), from a native RAW when there is one, else from a DNG the
# camera wrote. Written by scripts/cameras/raw-matrices.py.
"""


def d65_matrix(adobe):
    """The converted DNG's D65 ColorMatrix in ten-thousandths, or None."""
    for i in (2, 1):
        if adobe.get(f'CalibrationIlluminant{i}') == 21 and adobe.get(f'ColorMatrix{i}'):
            values = [float(v) for v in str(adobe[f'ColorMatrix{i}']).split()]
            if len(values) == 9:
                return [round(v * 10000) for v in values]
    return None


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--sweep', type=Path, required=True)
    p.add_argument('--fingerprints', type=Path, nargs='*', default=[])
    p.add_argument('--compare', action='store_true', help='write only matrices equal to the recorded cam_xyz')
    p.add_argument('--write', action='store_true')
    args = p.parse_args()
    files = {}
    for path in [COMMITTED, *args.fingerprints]:
        for f in json.loads(path.read_text())['files']:
            files[f['sha256']] = f
    # Per camera: (native?, matrix) candidates, and every file's recorded cam_xyz.
    candidates = defaultdict(list)
    recorded = defaultdict(set)
    for f in files.values():
        if 'header' in f:
            m = f['header']['metadata']
            recorded[(m['make'], m['model'])].add(json.dumps(m.get('cam_xyz')))
    for line in args.sweep.read_text().splitlines():
        s = json.loads(line)
        f = files.get(s['sha'])
        if not f or 'header' not in f or 'adobe' not in s:
            continue
        matrix = d65_matrix(s['adobe'])
        if matrix is None:
            continue
        m = f['header']['metadata']
        native = f['header']['facts']['dng_version'] == 0
        candidates[(m['make'], m['model'])].append((native, matrix))
    rows, differ = [], []
    for camera, found in sorted(candidates.items()):
        native = [mx for n, mx in found if n]
        matrix = (native or [mx for _, mx in found])[0]
        as_f32 = [[float(f'{matrix[r * 3 + c] / 10000:.9g}') for c in range(3)] for r in range(3)]
        equal = all(_close(json.loads(x), as_f32) for x in recorded[camera])
        (rows if equal or not args.compare else differ).append((camera, matrix))
    print(f'{len(rows)} matrices' + (f', {len(differ)} differ from the recorded cam_xyz' if args.compare else ''),
          file=sys.stderr)
    for (make, model), matrix in differ:
        print(f'differs: {make} {model} {matrix}', file=sys.stderr)
    text = HEADER + ''.join(
        f'\n[[matrix]]\nmake = {json.dumps(make)}\nmodel = {json.dumps(model)}\n'
        f'xyz = [{", ".join(map(str, matrix))}]\nsource = "adobe-dng"\n'
        for (make, model), matrix in rows)
    if args.write:
        TABLE.write_text(text)
    return 0


def _close(a, b):
    """Whether a recorded matrix is the f32 of these ten-thousandths."""
    import struct
    f32 = lambda v: struct.unpack('f', struct.pack('f', v))[0]
    return a is not None and all(f32(a[r][c]) == f32(b[r][c]) for r in range(3) for c in range(3))


if __name__ == '__main__':
    sys.exit(main())
