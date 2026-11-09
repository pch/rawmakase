"""The broad HDR gate: Camera Raw HDR references of many photos, reduced to blocks.

Each photo has one reference: Camera Raw's neutral HDR render (Adobe Standard, HDR
limit 4, the harness's base settings), output as 16-bit P3 PQ, decoded to linear
Display P3 with graphics white (203 cd/m²) at 1 and reduced in linear light. Only the
reduction is kept: 48-across block means on the SDR corpus's grid (`blocks.grid`),
highlights and negatives unclipped, plus the peak and the share of area above white
of the factor-8 reduction the HDR family uses (`hdr_reference.reduce_linear`). Full
renders are deleted as soon as they are reduced, on both sides.

  capture SELECTION OUT     renders the selection's photos in Camera Raw, one at a
                            time, into OUT (resumable; `--minutes` bounds a batch so
                            others can use Photoshop between batches)
  run REFS BINARY OUT       renders each reference's photo with `rawmakase render --hdr`
                            (`--data-dir`: the profile library with Adobe Standard),
                            reduces and scores it; resumable
  report RUN                prints each photo's scores and the tolerances it misses
  accept --candidate RUN --baseline RUN
                            judges a candidate run against a baseline run of `main`

SELECTION is a JSON list of photos (`sha256`, `path` relative to `--dataset`, `make`,
`model`, `split`); the dataset's `scripts/hdr.py select` writes one. Scores use the HDR
tolerances of docs/records/hdr-stage1-results.md on the blocks: luminance error above
white (median ≤ 0.15 stop, p95 ≤ 0.35), the peak (within 0.1 stop) and ΔE_ITP (mean ≤ 5,
p95 ≤ 15). Blocks average small highlights away, so the gate sees large bright areas
and the peak; specular detail needs the family's factor-8 references
(`hdr_validate.py`).

`accept` is a regression rule, like `gate.py accept`: no photo may get further from
Camera Raw than in the baseline by more than a margin per metric, or leave a tolerance
it met, and every reference of the split must be scored by both runs, except photos
listed in `--exclude` (with their reason) and photos only the candidate renders. The margins are
engineering budgets, not measured noise: Camera Raw's references repeat exactly, and so
do RAWmakase's renders.
"""
import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

import numpy as np

import blocks as sdr_blocks
import camera_raw
import hdr_reference
import hdr_validate

SETTINGS = {'CameraProfile': 'Adobe Standard', 'HDREditMode': '1', 'HDRMaxValue': '4.0'}
FACTOR = 8
# Stage 1's HDR tolerances. The peak's is on its size, in either direction.
TOLERANCES = {'above_median_stops': 0.15, 'above_p95_stops': 0.35, 'peak_stops': 0.1,
              'delta_e_itp_mean': 5.0, 'delta_e_itp_p95': 15.0}
# How much worse than the baseline a photo may get before `accept` fails it.
MARGINS = {'above_median_stops': 0.02, 'above_p95_stops': 0.05, 'peak_stops': 0.02,
           'delta_e_itp_mean': 0.5, 'delta_e_itp_p95': 1.5, 'below_median_stops': 0.02}
# A render whose width or height differs from the reference's by more than this share
# is framed otherwise (an in-camera crop one side honours) and is not scored.
FRAMING = 0.02


def blocks(linear):
    """48-across block means of a linear image, on `blocks.grid`, unclipped."""
    linear = np.asarray(linear, dtype=np.float64)
    if linear.ndim != 3 or linear.shape[2] != 3 or linear.size == 0 or not np.isfinite(linear).all():
        raise ValueError('Expected finite nonempty RGB')
    height, width = linear.shape[:2]
    rows = max(1, int(sdr_blocks.COLUMNS * height / width + 0.5))
    means = [linear[y:y + h, x:x + w].reshape(-1, 3).mean(0) for x, y, w, h in sdr_blocks.grid(width, height)]
    return np.array(means).reshape(rows, sdr_blocks.COLUMNS, 3)


def reduce(linear):
    """What a reference or a render keeps: blocks (float32), the factor-8 peak (the
    largest channel) and the share of factor-8 blocks whose luminance is above white."""
    eight = hdr_reference.reduce_linear(linear, FACTOR)
    return dict(blocks=blocks(linear).astype(np.float32), peak=float(eight.max()),
                above_white_share=float(((eight @ hdr_validate.P3_LUMA) > 1).mean()),
                dimensions=[int(linear.shape[1]), int(linear.shape[0])])


def compare(reference, candidate):
    """Scores a candidate's reduction against a reference's (dicts with `blocks`,
    `peak` and `dimensions`)."""
    (rw, rh), (cw, ch) = reference['dimensions'], candidate['dimensions']
    if abs(cw - rw) > FRAMING * rw or abs(ch - rh) > FRAMING * rh:
        return dict(valid=False, why=f'framed otherwise: {cw}×{ch} against {rw}×{rh}')
    ref = np.asarray(reference['blocks'], dtype=np.float64)
    ours = np.asarray(candidate['blocks'], dtype=np.float64)
    if ref.shape != ours.shape:
        # Sizes within the framing allowance can still round to another row count.
        return dict(valid=False, why=f'block grid {ours.shape[:2]} against {ref.shape[:2]}')
    m = hdr_validate.metrics(ref, ours)
    m['peak_stops'] = float(np.log2(candidate['peak'] / reference['peak']))
    m['reference_peak'], m['candidate_peak'] = reference['peak'], candidate['peak']
    return dict(valid=True, failed=missed(m), **m)


def missed(score):
    """The stage 1 tolerances a score's measurements miss."""
    return [k for k, limit in TOLERANCES.items() if score.get(k) is not None and abs(score[k]) > limit]


# Measurements every scored photo has; the above-white ones are None only when no
# reference block is above white.
REQUIRED = ('peak_stops', 'delta_e_itp_mean', 'delta_e_itp_p95', 'above_white_share')
OPTIONAL = ('above_median_stops', 'above_p95_stops', 'below_median_stops')


def scored(score):
    """Whether a score record is a complete measurement."""
    if not score or not score.get('valid') or 'error' in score or not isinstance(score.get('failed'), list):
        return False
    finite = lambda v: isinstance(v, (int, float)) and np.isfinite(v)  # noqa: E731
    above = score.get('above_white_share', 0) > 0
    return all(finite(score.get(k)) for k in REQUIRED) and all(
        k in score and (finite(score[k]) or (score[k] is None and not (above and k.startswith('above'))))
        for k in OPTIONAL)


def accept(baseline, candidate, photos, exclude=()):
    """Per photo (by key), the candidate's score against the baseline's. Returns the
    verdict with what each photo worsened (`worse`) or no longer meets (`left`). Every
    photo must be scored by both runs, except one the candidate scores and the baseline
    could not (`renders now`), and photos in `exclude`, which are reported."""
    if not photos:
        raise ValueError('No photos to judge')
    out, unscored = {}, []
    excluded = sorted(p for p in photos if p in exclude)
    for photo in photos:
        if photo in exclude:
            continue
        b, c = baseline.get(photo), candidate.get(photo)
        if b is None or c is None:
            # A reference a run never reached: an incomplete run proves nothing.
            unscored.append(photo)
            continue
        b_ok, c_ok = scored(b), scored(c)
        if not b_ok:
            # Only a baseline that did not render, or was framed otherwise, lets the
            # candidate's first score stand alone; a malformed one proves nothing.
            unrendered = 'error' in b or b.get('valid') is False
            if c_ok and unrendered:
                out[photo] = dict(note='renders now')
            else:
                unscored.append(photo)
            continue
        if not c_ok:
            unscored.append(photo)
            continue
        worse = []
        for k, margin in MARGINS.items():
            if b.get(k) is None or c.get(k) is None:
                if (b.get(k) is None) != (c.get(k) is None):
                    worse.append(k)
                continue
            if abs(c[k]) > abs(b[k]) + margin:
                worse.append(k)
        left = [k for k in missed(c) if k not in missed(b)]
        if worse or left:
            out[photo] = dict(worse=worse, left=left)
    failed = sorted(p for p, r in out.items() if r.get('worse') or r.get('left'))
    return dict(accepted=not failed and not unscored, failed=failed, unscored=sorted(unscored),
                excluded=excluded, photos=out)


def sha256(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def write_json(path, value):
    temporary = Path(str(path) + '.tmp')
    temporary.write_text(json.dumps(value, indent=1) + '\n')
    temporary.replace(path)


def returned_tag(xmp, name):
    """A Camera Raw setting of a sidecar, written as an attribute or as an element."""
    import re
    match = re.search(rf'crs:{name}="([^"]*)"', xmp) or re.search(rf'<crs:{name}>([^<]*)</crs:{name}>', xmp)
    return match.group(1) if match else None


def capture(args):
    """Camera Raw references of the selection's photos, one at a time: render to a
    working folder, reduce, check the stored reduction, delete the render."""
    import tifffile
    dataset = args.dataset.resolve()
    out = args.out.resolve()
    work = (args.work or out / 'work').resolve()
    selection = json.loads(args.selection.read_text())
    (out / 'blocks').mkdir(parents=True, exist_ok=True)
    (out / 'xmp').mkdir(exist_ok=True)
    index_path = out / 'references.json'
    references = json.loads(index_path.read_text()) if index_path.exists() else []
    failures_path = out / 'failures.json'
    failures = json.loads(failures_path.read_text()) if failures_path.exists() else {}
    done = {r['sha256'] for r in references} | set(failures)
    todo = [p for p in selection if p['sha256'] not in done][:args.limit or None]
    if not todo:
        print('Nothing to capture')
        return
    if not camera_raw.idle():
        raise RuntimeError('Photoshop must be idle with no open documents')
    deadline = time.monotonic() + args.minutes * 60
    for photo in todo:
        if time.monotonic() > deadline:
            print('Batch time is up; run again to continue', flush=True)
            break
        if shutil.disk_usage(work.parent if work.parent.exists() else out).free < args.min_free_gb * 2 ** 30:
            raise RuntimeError('Not enough free disk for a render')
        key = photo['sha256'][:16]
        source = dataset / photo['path']
        if sha256(source) != photo['sha256']:
            raise RuntimeError(f'{photo["path"]} does not match its SHA-256')
        shutil.rmtree(work, ignore_errors=True)
        work.mkdir(parents=True)
        copy = work / ('input' + source.suffix)
        shutil.copyfile(source, copy)
        xmp = camera_raw.xmp(SETTINGS)
        copy.with_suffix('.xmp').write_text(xmp)
        output = work / 'reference.tif'
        script = work / 'render.jsx'
        script.write_text(hdr_reference.TEMPLATE % dict(source=json.dumps(str(copy)), output=json.dumps(str(output))))
        started = time.monotonic()
        completed = subprocess.run(['osascript', '-e', 'with timeout of 600 seconds\n'
                                    f'tell application "{camera_raw.PHOTOSHOP}"\nactivate\n'
                                    f'do javascript file (POSIX file {json.dumps(str(script))})\n'
                                    'end tell\nend timeout'], capture_output=True, text=True, timeout=660)
        if completed.returncode or not output.exists():
            failures[photo['sha256']] = (completed.stdout + completed.stderr).strip()[-300:]
            write_json(failures_path, failures)
            print(f'{key} {photo["make"]} {photo["model"]}: failed: {failures[photo["sha256"]]}', flush=True)
            if not camera_raw.idle():
                raise RuntimeError('Photoshop was left with a document open')
            continue
        if sha256(source) != photo['sha256']:
            raise RuntimeError('The original changed during the capture')
        returned = subprocess.check_output(['exiftool', '-b', '-XMP', str(output)])
        description = subprocess.check_output(['exiftool', '-s3', '-ICC_Profile:ProfileDescription',
                                               str(output)], text=True).strip()
        with tifffile.TiffFile(output) as tiff:
            pixels = tiff.asarray()
            icc = tiff.pages[0].tags[34675].value
        if pixels.dtype != np.uint16 or pixels.ndim != 3 or pixels.shape[2] != 3 or description != 'P3 PQ':
            raise ValueError(f'{key}: expected a 16-bit P3 PQ RGB TIFF, got {description}')
        returned_text = returned.decode('utf-8', 'replace')
        profile = returned_tag(returned_text, 'CameraProfile')
        output_hash = sha256(output)
        linear = hdr_reference.pq_to_nits(pixels.astype(np.float64) / 65535) / 203
        del pixels
        reduced = reduce(linear)
        del linear
        target = out / 'blocks' / f'{key}.npy'
        np.save(target, reduced['blocks'])
        if not np.array_equal(np.load(target), reduced['blocks']):
            raise RuntimeError(f'{target} does not read back')
        (out / 'xmp' / f'{key}-requested.xmp').write_text(xmp)
        (out / 'xmp' / f'{key}-returned.xmp').write_bytes(returned)
        shutil.rmtree(work)
        record = dict(photo, settings=SETTINGS, camera_raw=camera_raw.version(),
                      process_version=returned_tag(returned_text, 'ProcessVersion'),
                      profile=profile, profile_digest=returned_tag(returned_text, 'CameraProfileDigest'),
                      requested_xmp_sha256=sha256(out / 'xmp' / f'{key}-requested.xmp'),
                      returned_xmp_sha256=sha256(out / 'xmp' / f'{key}-returned.xmp'),
                      output_sha256=output_hash, output_transfer='ST2084', output_profile=description,
                      output_icc_sha256=hashlib.sha256(icc).hexdigest(), output_dimensions=reduced['dimensions'],
                      blocks=f'blocks/{key}.npy', blocks_sha256=sha256(target), blocks_shape=list(reduced['blocks'].shape),
                      peak=reduced['peak'], above_white_share=reduced['above_white_share'],
                      reduction=dict(blocks='48-across means on blocks.grid, linear Display P3 D65, unclipped, float32',
                                     peak='largest channel of the factor-8 linear area mean',
                                     domain='ST 2084 decoded, 203 cd/m² = 1'),
                      seconds=round(time.monotonic() - started, 1))
        if profile != 'Adobe Standard':
            record['invalid'] = f'Camera Raw rendered with {profile!r}'
        references.append(record)
        write_json(index_path, references)
        print(f'{key} {photo["make"]} {photo["model"]}: peak {reduced["peak"]:.2f}, '
              f'{reduced["above_white_share"]:.1%} above white, {record["seconds"]} s', flush=True)


def run(args):
    refs = args.refs.resolve()
    out = args.out.resolve()
    (out / 'scores').mkdir(parents=True, exist_ok=True)
    references = [r for r in json.loads((refs / 'references.json').read_text())
                  if not r.get('invalid') and (not args.split or r['split'] == args.split)]
    # Renders with a copy of the build taken when the run starts, so a rebuild changes nothing.
    binary = out / 'rawmakase'
    if not binary.exists():
        shutil.copy2(args.binary, binary)
        version = subprocess.run([str(binary), '--version'], capture_output=True, text=True).stdout.strip()
        write_json(out / 'run.json', dict(binary_sha256=sha256(binary), version=version,
                                          started=time.strftime('%Y-%m-%dT%H:%M:%S')))
    env = dict(os.environ, RAWMAKASE_DATA_DIR=str(args.data_dir.resolve()))
    dataset = args.dataset.resolve()
    work = (args.work or out).resolve()
    work.mkdir(parents=True, exist_ok=True)
    for ref in references:
        key = ref['sha256'][:16]
        score_path = out / 'scores' / f'{key}.json'
        if score_path.exists():
            continue
        master = work / f'{key}.tif'
        master.unlink(missing_ok=True)
        started = time.monotonic()
        try:
            done = subprocess.run([str(binary), 'render', str(dataset / ref['path']), '--xmp',
                                   str(refs / 'xmp' / f'{key}-requested.xmp'), '--hdr', str(master)],
                                  capture_output=True, text=True, env=env)
            if done.returncode:
                score = dict(error=done.stderr.strip()[-300:])
            else:
                reduced = reduce(hdr_validate.read_master(master))
                reference = dict(blocks=np.load(refs / ref['blocks']), peak=ref['peak'],
                                 dimensions=ref['output_dimensions'])
                score = compare(reference, reduced)
                score['seconds'] = round(time.monotonic() - started, 1)
        finally:
            master.unlink(missing_ok=True)
        score.update(sha256=ref['sha256'], make=ref['make'], model=ref['model'], split=ref['split'])
        write_json(score_path, score)
        print(f'{key} {ref["make"]} {ref["model"]}: ' + (score.get('error') or score.get('why') or (
            f'above {fmt(score["above_median_stops"])}/{fmt(score["above_p95_stops"])}, peak {score["peak_stops"]:+.3f}, '
            f'ΔE_ITP {score["delta_e_itp_mean"]:.1f}/{score["delta_e_itp_p95"]:.1f}'
            + (f'  FAILS {",".join(score["failed"])}' if score['failed'] else ''))), flush=True)


def fmt(value):
    return '—' if value is None else f'{value:.3f}'


def scores(folder, split):
    out = {}
    for path in sorted((folder / 'scores').glob('*.json')):
        score = json.loads(path.read_text())
        if not split or score['split'] == split:
            out[score['sha256']] = score
    return out


def report(args):
    found = scores(args.run, args.split)
    valid = [s for s in found.values() if s.get('valid') and 'error' not in s]
    print(f'{len(found)} photos: {len(valid)} scored, {sum("error" in s for s in found.values())} failed to render, '
          f'{sum(s.get("valid") is False for s in found.values())} framed otherwise')
    print(f'within every tolerance: {sum(not s["failed"] for s in valid)}')
    for k in TOLERANCES:
        values = [abs(s[k]) for s in valid if s.get(k) is not None]
        if values:
            print(f'  {k}: median {np.median(values):.3f}, p90 {np.quantile(values, .9):.3f}, '
                  f'over {TOLERANCES[k]}: {sum(v > TOLERANCES[k] for v in values)}')
    for s in sorted(found.values(), key=lambda s: (s.get('make', ''), s.get('model', ''))):
        line = s.get('error') or s.get('why') or (
            f'above {fmt(s["above_median_stops"])}/{fmt(s["above_p95_stops"])} '
            f'({s["above_white_share"]:.1%} of blocks), peak {s["peak_stops"]:+.3f}, '
            f'ΔE_ITP {s["delta_e_itp_mean"]:.1f}/{s["delta_e_itp_p95"]:.1f}, below {fmt(s["below_median_stops"])}'
            + (f'  FAILS {",".join(s["failed"])}' if s['failed'] else ''))
        print(f'{s["sha256"][:10]} {s["split"]:10s} {s["make"]} {s["model"]}: {line}')


def accept_command(args):
    baseline, candidate = scores(args.baseline, args.split), scores(args.candidate, args.split)
    refs = json.loads((args.refs / 'references.json').read_text())
    photos = sorted(r['sha256'] for r in refs if not r.get('invalid') and (not args.split or r['split'] == args.split))
    exclude = set()
    if args.exclude:
        prefixes = [line.split()[0] for line in args.exclude.read_text().splitlines()
                    if line.strip() and not line.startswith('#')]
        exclude = {p for p in photos if any(p.startswith(x) for x in prefixes)}
    verdict = accept(baseline, candidate, photos, exclude)
    for photo in verdict['failed']:
        r = verdict['photos'][photo]
        print(f'{photo[:10]} {candidate[photo]["make"]} {candidate[photo]["model"]}: '
              f'worse {",".join(r["worse"]) or "—"}; no longer within {",".join(r["left"]) or "—"}')
    for photo, r in sorted(verdict['photos'].items()):
        if r.get('note'):
            print(f'{photo[:10]} {candidate[photo]["make"]} {candidate[photo]["model"]}: {r["note"]}, '
                  'no baseline to compare with')
    for photo in verdict['unscored']:
        print(f'{photo[:10]}: not scored by both runs')
    if verdict['excluded']:
        print(f'{len(verdict["excluded"])} photos excluded: {" ".join(p[:10] for p in verdict["excluded"])}')
    print(f'{len(photos)} photos; {"ACCEPTED" if verdict["accepted"] else "REJECTED"}')
    return 0 if verdict['accepted'] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--dataset', type=Path, default=os.environ.get('RAWMAKASE_DATASET'),
                        help='the dataset folder the selection\'s paths are relative to ($RAWMAKASE_DATASET)')
    sub = parser.add_subparsers(dest='command', required=True)
    c = sub.add_parser('capture')
    c.add_argument('selection', type=Path)
    c.add_argument('out', type=Path)
    c.add_argument('--work', type=Path, help='where each full-size render waits to be reduced')
    c.add_argument('--minutes', type=float, default=25, help='start no capture after this long')
    c.add_argument('--limit', type=int, help='capture at most this many photos')
    c.add_argument('--min-free-gb', type=float, default=30)
    r = sub.add_parser('run')
    r.add_argument('refs', type=Path)
    r.add_argument('binary', type=Path)
    r.add_argument('out', type=Path)
    r.add_argument('--data-dir', type=Path, required=True,
                   help='a RAWmakase data folder whose camera-profiles/ holds Adobe Standard')
    r.add_argument('--split', help='validation, holdout or train (default: every photo)')
    r.add_argument('--work', type=Path, help='where each full-size render waits to be reduced (default OUT)')
    p = sub.add_parser('report')
    p.add_argument('run', type=Path)
    p.add_argument('--split')
    a = sub.add_parser('accept')
    a.add_argument('--candidate', type=Path, required=True)
    a.add_argument('--baseline', type=Path, required=True)
    a.add_argument('--refs', type=Path, required=True, help='the references: every photo of the split is judged')
    a.add_argument('--exclude', type=Path, help='SHA-256 prefixes, one per line, left out of the verdict '
                   '(photos the baseline cannot render, listed with a reason)')
    a.add_argument('--split', default='validation')
    args = parser.parse_args()
    if args.command in ('capture', 'run') and not args.dataset:
        sys.exit('Pass --dataset or set RAWMAKASE_DATASET')
    if args.dataset and not Path(args.dataset).is_dir():
        sys.exit(f'{args.dataset} is not there: is the dataset SSD mounted?')
    sys.exit({'capture': capture, 'run': run, 'report': report, 'accept': accept_command}[args.command](args) or 0)


if __name__ == '__main__':
    main()
