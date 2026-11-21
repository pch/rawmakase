#!/usr/bin/env python3
"""Half-size checks of the classified test dataset: RAWmakase's half-size
decode of each RAW against its own full decode reduced to that size
(`rawmakase half-check`, which holds the gates), and its time.

Each file is held to LibRaw's half-size decode of it, recorded once at the
switch to RAWmakase's own decoders (882e6dcb, the last build that linked
LibRaw) and judged the same way, with RAWmakase's camera matrix: block
CIEDE2000 mean and 95th percentile at most 0.1 above LibRaw's, the
Laplacian energy ratio's distance from 1 at most 0.1 above LibRaw's, and
the half-size time at most 1.1 × LibRaw's half ÷ full ratio × the full
develop time (measured in the same check, fastest of three) + 20 ms. The
references cover this script's default selection: the raw.pixls.us CC0
files' in tests/corpus/dataset/pixls-half.json, the others' in
$RAWMAKASE_DATASET/baselines/half/<source>.json. A file without one is held
to absolute limits (colour mean 0.3, p95 1, detail 0.8 to 1.25, and a time
ratio of 1). X-Trans files and small RAWs take a time ratio of 1: an X-Trans
draft is the full demosaic reduced, which keeps it free of moiré, and a
small RAW develops at full size in both modes.

`dataset-develop.py` checks that the drafts do not change; this checks what
they are.

  python3 scripts/corpus/dataset-half.py                  # 3 files per make and format
  python3 scripts/corpus/dataset-half.py --per-group 0    # every file (hours)
  python3 scripts/corpus/dataset-half.py --manifest m.json --report half.json

Files run one at a time, so the times are not disturbed; run it on a quiet
machine. Small RAWs are judged on their time only. Exit status: 0 every file
passes, 1 a file fails a gate or errs, 2 the run could not be made.
"""
import argparse
import importlib.util
import json
import os
import random
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path

spec = importlib.util.spec_from_file_location('fingerprints', Path(__file__).with_name('dataset-fingerprints.py'))
fingerprints = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fingerprints)
Setup = fingerprints.Setup
ROOT = fingerprints.ROOT
# The limits of half_check.rs: absolute ones without a reference.
GATES = {'colour mean': ('de_mean', 0.3), 'colour p95': ('de_p95', 1.0)}
DETAIL = (0.8, 1.25)
REFERENCE_SLACK = 0.1
MAX_TIME = 1.1
TIME_SLACK = 0.02
# A file without a recorded ratio: no slower than the full develop.
FALLBACK_RATIO = 1.0
REFERENCES = ROOT / 'tests/corpus/dataset/pixls-half.json'
# What a recorded reference holds that the check uses.
REFERENCE_KEYS = ('ratio', 'de_mean', 'de_p95', 'detail')
FILE_TIMEOUT = 900


def reference_path(dataset, sample):
    """Where a file's recorded reference is: committed for raw.pixls.us CC0
    files, in the dataset's local baselines for the others."""
    return REFERENCES if fingerprints.committed(sample) else dataset / 'baselines/half' / f'{sample["source"]}.json'


def load_references(dataset, samples):
    """The recorded references by SHA-256, for the files that have one."""
    references = {}
    for path in {reference_path(dataset, s) for s in samples}:
        loaded = fingerprints.read_baseline(path)
        if loaded:
            for h, f in loaded[1].items():
                reference = {k: f[k] for k in REFERENCE_KEYS if f.get(k) is not None}
                if reference:
                    references[h] = reference
    return references


def write_references(path, recorded, files):
    """One file per line, sorted by SHA-256: LibRaw's half-size and full develop
    seconds, their ratio and its half-size color and detail measures, from one
    run of the last build that linked LibRaw."""
    path.parent.mkdir(parents=True, exist_ok=True)
    about = ("LibRaw's half-size decodes (rawmakase half-check: time over its full develop time, block "
             "CIEDE2000 mean and 95th percentile and detail ratio against the full decode reduced, judged "
             "with RAWmakase's camera matrix) for the raw.pixls.us CC0 files of "
             "scripts/corpus/dataset-half.py's default selection, recorded once with the last build that "
             'linked LibRaw: the half-size gates\' reference. Never re-recorded.') if path == REFERENCES else \
        "LibRaw's half-size decodes, the half-size gates' reference. Local only."
    lines = [json.dumps(files[h], separators=(',', ':')) for h in sorted(files)]
    path.write_text('{"about": ' + json.dumps(about) + ',\n "recorded": ' + json.dumps(recorded) +
                    ',\n "files": [\n' + ',\n'.join(lines) + '\n]}\n')


def time_limit(check):
    """The most half ÷ full time may be for a `half-check` line, before the slack."""
    if check.get('halved') is None:
        return None
    own = check['halved'] is False or check.get('xtrans') is True
    reference = 1.0 if own else ((check.get('reference') or {}).get('ratio') or FALLBACK_RATIO)
    return MAX_TIME * reference


def time_ratio(check):
    half, full = check.get('seconds'), check.get('full_seconds')
    return half / full if half is not None and full else None


def slow(check):
    half, full, limit = check.get('seconds'), check.get('full_seconds'), time_limit(check)
    return half is not None and full is not None and limit is not None and half > limit * full + TIME_SLACK


def failures(check):
    """The gates a `half-check` line fails (as the binary judges them)."""
    out = []
    if check.get('error'):
        out.append('error')
    reference = check.get('reference') or {}
    # Small RAWs (not halved) skip the image gates, not the time.
    if check.get('halved') is not False:
        for name, (key, absolute) in GATES.items():
            limit = reference[key] + REFERENCE_SLACK if reference.get(key) is not None else absolute
            if check.get(key) is not None and check[key] > limit:
                out.append(name)
        detail = check.get('detail')
        if detail is not None and (abs(detail - 1) > abs(reference['detail'] - 1) + REFERENCE_SLACK
                                   if reference.get('detail') is not None else not DETAIL[0] <= detail <= DETAIL[1]):
            out.append('detail')
    if slow(check):
        out.append('time')
    return out


def select(samples, per_group, seed):
    if per_group <= 0:
        return list(samples.values())
    groups = defaultdict(list)
    for s in sorted(samples.values(), key=lambda s: s['sha256']):
        groups[(s['make'] or '?', (s['ext'] or '').lower())].append(s)
    rng = random.Random(seed)
    return [s for g in sorted(groups) for s in rng.sample(groups[g], min(per_group, len(groups[g])))]


def summarize(results, public, samples):
    """The report: counts per gate, failing files by group, and the worst values."""
    checked = [r for r in results if r.get('halved')]
    fails = {r['sha256']: failures(r) for r in results}
    report = {
        'files': len(results),
        'checked': len(checked),
        'not halved': sum(1 for r in results if r.get('halved') is False),
        'errors': sum(1 for r in results if r.get('error')),
        'failing': Counter(f for v in fails.values() for f in v),
        'passing': sum(1 for v in fails.values() if not v),
    }
    for key in ('de_mean', 'de_p95', 'detail'):
        vals = sorted(r[key] for r in checked if r.get(key) is not None)
        if vals:
            report[key] = {'median': vals[len(vals) // 2], 'p90': vals[int(0.9 * (len(vals) - 1))],
                           'min': vals[0], 'max': vals[-1]}
    # Half ÷ full time over its limit: 1 is at the limit.
    margins = sorted(time_ratio(r) / time_limit(r) for r in results if time_ratio(r) and time_limit(r))
    if margins:
        report['time over limit'] = {'median': margins[len(margins) // 2],
                                     'p90': margins[int(0.9 * (len(margins) - 1))], 'max': margins[-1]}
    report['without reference'] = sum(1 for r in results if r.get('halved') and not r.get('reference'))
    groups = defaultdict(list)
    for r in results:
        if fails[r['sha256']]:
            s = samples[r['sha256']]
            name = r['sha256'][:12] if public and not fingerprints.committed(s) else f"{r['sha256'][:12]} {s['model']}"
            groups[f"{s['make']} .{(s['ext'] or '').lower()}"].append((name, fails[r['sha256']],
                                                                        {k: r.get(k) for k in ('de_mean', 'de_p95', 'detail')}))
    report['failing files'] = dict(sorted(groups.items()))
    return report


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--dataset', type=Path, default=os.environ.get('RAWMAKASE_DATASET'))
    p.add_argument('--rawmakase', type=Path, default=ROOT / 'target/release/rawmakase')
    p.add_argument('--manifest', type=Path, help='a dataset.py manifest; default: every file')
    p.add_argument('--per-group', type=int, default=3, help='files per make and format (0: every file)')
    p.add_argument('--seed', type=int, default=1)
    p.add_argument('--public', action='store_true', help='name only raw.pixls.us CC0 files')
    p.add_argument('--report', type=Path, help='also write the report as JSON')
    args = p.parse_args()
    try:
        if args.dataset is None:
            raise Setup('Set RAWMAKASE_DATASET or pass --dataset')
        samples = fingerprints.load_samples(args.dataset, args.manifest)
        chosen = select(samples, args.per_group, args.seed)
        references = load_references(args.dataset, chosen)
        results = []
        for i, s in enumerate(chosen):
            reference = references.get(s['sha256'])
            command = [str(args.rawmakase), 'half-check',
                       *(['--reference', json.dumps(reference)] if reference else []), str(s['file'])]
            try:
                out = subprocess.run(command, capture_output=True, text=True, timeout=FILE_TIMEOUT)
                line = json.loads(out.stdout) if out.returncode == 0 else {'error': f'exit status {out.returncode}'}
            except (subprocess.TimeoutExpired, json.JSONDecodeError) as e:
                line = {'error': type(e).__name__}
            line['sha256'] = s['sha256']
            line.pop('path', None)
            results.append(line)
            print(f'\r{i + 1}/{len(chosen)} files', end='', file=sys.stderr, flush=True)
        print(file=sys.stderr)
        report = summarize(results, args.public, samples)
        if args.report:
            args.report.write_text(json.dumps({'report': report, 'files': results}, indent=1, default=list) + '\n')
        print(json.dumps(report, indent=1, default=list))
        return 1 if report['passing'] < report['files'] else 0
    except Setup as e:
        print(f'error: {e}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
