#!/usr/bin/env python3
"""Write data/raw-matrices.toml, the XYZ (D65) to camera matrices of RAWmakase's
own RAW decoders, from Adobe's: Adobe DNG Converter's output, else (Adobe has
nothing for the camera) what LibRaw reported. Nothing is read from Adobe's
camera profiles.

Sources, per camera, in order:

1. adobe-dng: the ColorMatrix Adobe DNG Converter writes for the camera's own
   RAW: the one for illuminant D65 (21), else the only one. A camera known only
   from DNGs it wrote gets a row where Adobe's matrix is the one every file of
   the camera carries, or where Adobe replaces the file's matrix with its own;
   elsewhere (phones whose DNGs carry their own matrices, which the converter
   keeps, only rescaled) the decoder reads each file's.
2. compat-observed: where the converter has none, the cam_xyz LibRaw reported
   for every native RAW of the camera (one non-zero matrix), except for the
   cameras of NO_ROW.

A matrix is unusable when it is diagonal (Adobe keeps such a camera's colour in
its forward matrix), has an entry beyond ±10 (a camera's own values passed
through unscaled) or is Adobe's placeholder for an unknown camera (D50 XYZ to
linear sRGB).

Inputs: --sweep, JSON lines of one converted sample per camera and raw mode,
each {"sha": ..., "path": dataset path, "adobe": {exiftool -n of the converted
DNG}} (Adobe's tags only; nothing of the RAW is kept), several files allowed;
--dataset, the dataset root, to read a camera DNG's own matrix; the
camera's name is the one RAWmakase gives it, from the decode fingerprints
recorded for the same files (tests/corpus/dataset/pixls-decode.json and the
local baselines). A sample comes from

  "Adobe DNG Converter" -c -u -p0 -d OUT FILE
  exiftool -n -j -IFD0:UniqueCameraModel -IFD0:CalibrationIlluminant1 \\
      -IFD0:CalibrationIlluminant2 -IFD0:ColorMatrix1 -IFD0:ColorMatrix2 OUT/*.dng

(IFD0's: converting a camera's DNG, the converter writes Adobe's matrices
there and keeps the file's own as an extra profile named "Embedded", whose
tags exiftool reports without the group.)

and the table from

  python3 scripts/cameras/raw-matrices.py --sweep sweep.jsonl more.jsonl \\
      --dataset $RAWMAKASE_DATASET \\
      --fingerprints $RAWMAKASE_DATASET/baselines/decode/*.json --write
"""
import argparse
import json
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TABLE = ROOT / 'data/raw-matrices.toml'
COMMITTED = ROOT / 'tests/corpus/dataset/pixls-decode.json'
HEADER = """# XYZ (D65) to camera matrices, in ten-thousandths, for RAWmakase's own RAW
# decoders (crates/rawmakase-raw/src/facts.rs), from Adobe (2026-10-17):
# adobe-dng is the ColorMatrix Adobe DNG Converter 18.7 writes for the camera
# (for illuminant D65, else its only one), from a native RAW when there is one,
# else from a DNG the camera wrote; compat-observed, only for a camera Adobe
# DNG Converter has no matrix for, the cam_xyz LibRaw reported on our files.
# Never from Adobe's camera profiles. Written by scripts/cameras/raw-matrices.py.
"""
# D50 XYZ to linear sRGB: what Adobe writes for a camera it does not know.
PLACEHOLDER = [31339, -16169, -4906, -9788, 19161, 335, 719, -2290, 14052]
MATRIX_TAGS = ['-CalibrationIlluminant1', '-CalibrationIlluminant2', '-ColorMatrix1', '-ColorMatrix2']
# Cameras the converter gives no usable matrix for whose LibRaw matrix is not
# kept either, as it is the one of Adobe's camera profiles, which RAWmakase does
# not use (Piotr, 2026-10-17): the Olympus compacts' decoder reads the matrix
# each ORF carries; the Leaf Aptus 75 has none.
NO_ROW = {('Leaf', 'Aptus 75'), ('Olympus', 'SP350'), ('Olympus', 'SP500UZ'),
          ('Olympus', 'SP510UZ'), ('Olympus', 'SP550UZ'), ('Olympus', 'SP570UZ')}


def adobe_matrix(tags):
    """Adobe's matrix in ten-thousandths: D65's, else the only one; None if unusable."""
    illuminants = [tags.get('CalibrationIlluminant1'), tags.get('CalibrationIlluminant2')]
    if 21 in illuminants:
        # The second when both are D65's: Adobe's own after the file's.
        text = tags.get(f'ColorMatrix{2 if illuminants[1] == 21 else 1}')
    elif not tags.get('ColorMatrix2'):
        text = tags.get('ColorMatrix1')
    else:
        text = None
    values = [float(v) for v in str(text or '').split()]
    if len(values) != 9:
        return None
    matrix = [round(v * 10000) for v in values]
    diagonal = all(matrix[i] == 0 for i in (1, 2, 3, 5, 6, 7))
    placeholder = all(abs(a - b) <= 10 for a, b in zip(matrix, PLACEHOLDER))
    if diagonal or placeholder or any(abs(v) >= 100000 for v in matrix):
        return None
    return matrix


def own_matrix(dataset, path):
    """The matrix a camera DNG carries itself, as the decoder reads it."""
    out = subprocess.run(['exiftool', '-n', '-j', *MATRIX_TAGS, str(dataset / path)],
                         capture_output=True, text=True).stdout
    tags = json.loads(out or '[{}]')[0]
    which = 2 if tags.get('CalibrationIlluminant2') == 21 else 1
    values = [float(v) for v in str(tags.get(f'ColorMatrix{which}') or '').split()]
    return values if len(values) == 9 else None


def rescaled(matrix, own):
    """Whether Adobe's matrix is the file's own, scaled (to Adobe's four places)."""
    if own is None:
        return False
    scale = sum(m / 10000 * o for m, o in zip(matrix, own)) / sum(o * o for o in own)
    return all(abs(m - round(o * scale * 10000)) <= 2 for m, o in zip(matrix, own))


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--sweep', type=Path, nargs='+', required=True)
    p.add_argument('--dataset', type=Path)
    p.add_argument('--fingerprints', type=Path, nargs='*', default=[])
    p.add_argument('--write', action='store_true')
    args = p.parse_args()
    files = {}
    for path in [COMMITTED, *args.fingerprints]:
        for f in json.loads(path.read_text())['files']:
            files[f['sha256']] = f
    # Per camera: every file's recorded cam_xyz (native?, ten-thousandths),
    # and the converted samples (native?, tags, dataset path).
    recorded = defaultdict(list)
    samples = defaultdict(list)
    for f in files.values():
        if 'header' in f:
            m = f['header']['metadata']
            xyz = [round(v * 10000) for row in m.get('cam_xyz') or [] for v in row]
            recorded[(m['make'], m['model'])].append((f['header']['facts']['dng_version'] == 0, xyz))
    for sweep in args.sweep:
        for line in sweep.read_text().splitlines():
            s = json.loads(line)
            f = files.get(s['sha'])
            if f and 'header' in f and s.get('adobe'):
                m = f['header']['metadata']
                native = f['header']['facts']['dng_version'] == 0
                samples[(m['make'], m['model'])].append((native, s['adobe'], s.get('path')))
    rows = []
    for camera in sorted(recorded):
        make, model = camera
        native_files = [xyz for native, xyz in recorded[camera] if native]
        found = samples[camera]
        converted = [adobe_matrix(t) for native, t, _ in found if native]
        row = next(((m, 'adobe-dng') for m in converted if m), None)
        # A DNG the camera wrote: the converter's matrix where it is the one
        # every file carries, or Adobe's own in place of the file's.
        dngs = [(adobe_matrix(t), path) for native, t, path in found if not native]
        # Any sample of a DNG the camera wrote may show it (an earlier one may
        # have been read from the converter's "Embedded" profile, the file's own).
        for matrix, path in [d for d in dngs if d[0]] if row is None else []:
            if all(xyz == matrix for _, xyz in recorded[camera]):
                row = (matrix, 'adobe-dng')
            elif args.dataset and not rescaled(matrix, own_matrix(args.dataset, path)):
                row = (matrix, 'adobe-dng')
            if row:
                break
        if row is None and native_files and camera not in NO_ROW:
            observed = {json.dumps(x) for x in native_files}
            if len(observed) == 1 and any(native_files[0]):
                row = (native_files[0], 'compat-observed')
        if row:
            rows.append((camera, *row))
    counts = defaultdict(int)
    for *_, source in rows:
        counts[source] += 1
    print(f'{len(rows)} matrices: ' + ', '.join(f'{n} {s}' for s, n in sorted(counts.items())), file=sys.stderr)
    for camera in sorted(set(recorded) - {r[0] for r in rows}):
        print(f'no row: {camera[0]} {camera[1]}', file=sys.stderr)
    text = HEADER + ''.join(
        f'\n[[matrix]]\nmake = {json.dumps(make)}\nmodel = {json.dumps(model)}\n'
        f'xyz = [{", ".join(map(str, matrix))}]\nsource = "{source}"\n'
        for (make, model), matrix, source in rows)
    if args.write:
        TABLE.write_text(text)
    return 0


if __name__ == '__main__':
    sys.exit(main())
