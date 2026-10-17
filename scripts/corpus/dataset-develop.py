#!/usr/bin/env python3
"""Develop fingerprints of the classified test dataset: what each decode mode
(full, half) makes of every RAW file, as the renderer receives it.

Runs `rawmakase decode-dump` on the dataset's files and compares each mode's
size, scale, pixel and metadata hashes with a recorded baseline, as
dataset-fingerprints.py does for decoding (whose file selection, batching and
approved deltas, tests/corpus/decode-deltas.toml, it shares).

  check    develop the files and report every difference; exits 1 on any
  record   write the baselines from this build; --reason is required

Baselines stay in $RAWMAKASE_DATASET/baselines/develop/<source>.json (every
source: the hashes depend on the platform's floating point). --floats writes
each full decode that is not a mosaic (small RAWs) as raw floats, for a
per-pixel comparison (`rawmakase decode-diff`); keep them off the dataset's
disk, they are large.

  python3 scripts/corpus/dataset-develop.py record --reason "oracle" --floats ~/oracle-floats
  python3 scripts/corpus/dataset-develop.py check

Exit status: 0 no difference, 1 differences, 2 the run could not be made.
"""
import argparse
import concurrent.futures
import importlib.util
import json
import os
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path

spec = importlib.util.spec_from_file_location('fingerprints', Path(__file__).with_name('dataset-fingerprints.py'))
fingerprints = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fingerprints)
Setup = fingerprints.Setup
ROOT = fingerprints.ROOT
BATCH = 8
# A full and a half decode of the largest files take a minute.
BATCH_TIMEOUT = 1800
FILE_TIMEOUT = 600


def baseline_path(dataset, sample):
    return dataset / 'baselines/develop' / f'{sample["source"]}.json'


def run(rawmakase, files, timeout, floats, decoder):
    command = [str(rawmakase), 'decode-dump', *(['--decoder', decoder] if decoder else []),
               *(['--floats', str(floats)] if floats else []), *map(str, files)]
    try:
        p = subprocess.run(command, capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return None, f'timed out after {timeout} s'
    if p.returncode != 0:
        return None, f'exit status {p.returncode}'
    try:
        lines = [json.loads(line) for line in p.stdout.splitlines()]
    except json.JSONDecodeError as e:
        return None, f'unreadable output: {e}'
    if [line.get('path') for line in lines] != [str(f) for f in files]:
        return None, f'{len(lines)} lines for {len(files)} files'
    return lines, None


def develop(args, samples):
    order = sorted(samples.values(), key=lambda s: (s['source'], s['make'] or '', s['model'] or '', s['sha256']))
    batches = [order[i:i + BATCH] for i in range(0, len(order), BATCH)]
    results, problems, done = {}, [], 0

    def batch(chunk):
        lines, error = run(args.rawmakase, [s['file'] for s in chunk], BATCH_TIMEOUT, args.floats, args.decoder)
        if lines is not None:
            return list(zip(chunk, lines)), None
        out = []
        for s in chunk:
            alone, why = run(args.rawmakase, [s['file']], FILE_TIMEOUT, args.floats, args.decoder)
            out.append((s, alone[0] if alone else {'error': f'crash: {why}'}))
        return out, f'a batch of {len(chunk)} files failed ({error}); its files were run one by one'

    with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
        for out, problem in pool.map(batch, batches):
            if problem:
                problems.append((True, problem))
            for s, line in out:
                line.pop('path', None)
                if line.get('sha256') not in (None, s['sha256']):
                    problems.append((fingerprints.committed(s), f'{s["sha256"]}: the file on disk is not the dataset\'s file'))
                    line = {'error': 'not the dataset\'s file'}
                line['sha256'] = s['sha256']
                results[s['sha256']] = line
            done += len(out)
            print(f'\r{done}/{len(order)} files', end='', file=sys.stderr, flush=True)
    print(file=sys.stderr)
    return results, problems


def record(args, samples, results):
    by_path = defaultdict(dict)
    for h, s in samples.items():
        by_path[baseline_path(args.dataset, s)][h] = {k: v for k, v in results[h].items()
                                                      if k not in fingerprints.NOT_COMPARED}
    recorded = fingerprints.recorded_by(results, args.reason)
    for path, files in sorted(by_path.items()):
        old = fingerprints.read_baseline(path)
        merged = {**(old[1] if old else {}), **files}
        fingerprints.write_baseline(path, recorded, merged)
        print(f'{len(files)} files recorded in {path} ({len(merged)} in all)')


def check(args, samples, results, problems):
    deltas = fingerprints.load_deltas()
    baselines = {}
    changes = defaultdict(list)
    for h, s in samples.items():
        path = baseline_path(args.dataset, s)
        if path not in baselines:
            loaded = fingerprints.read_baseline(path)
            if loaded is None:
                raise Setup(f'No baseline at {path}: record one with `record --reason …`')
            baselines[path] = loaded[1]
        old, new = baselines[path].get(h), results[h]
        group = (s['make'] or '?', (s['ext'] or Path(s['path']).suffix[1:]).lower())
        if old is None:
            changes[group].append(('no baseline', s, None))
            continue
        for field in fingerprints.compare(old, new, h, deltas):
            changes[group].append(('changed', s, field))
    # With every file selected, a baseline file the dataset no longer has is a problem.
    if not args.manifest:
        for path in (args.dataset / 'baselines/develop').glob('*.json'):
            if path not in baselines:
                baselines[path] = fingerprints.read_baseline(path)[1]
        for path, files in baselines.items():
            for h in files.keys() - samples.keys():
                problems.append((False, f'{h[:12]} is in {path.name} but not in the dataset'))
    for problem in fingerprints.visible(problems, args.public):
        print(f'problem: {problem}')
    for (make, ext), items in sorted(changes.items()):
        kinds = Counter(kind if kind != 'changed' else f'{detail[0]} changed' for kind, _, detail in items)
        print(f'\n{make} .{ext}: ' + ', '.join(f'{n} {k}' for k, n in sorted(kinds.items())))
        for kind, s, detail in items:
            if args.public and not fingerprints.committed(s):
                continue
            line = f'  {s["sha256"][:12]} {s["model"]}: {kind}'
            if detail:
                line += f' {detail[0]}: {json.dumps(detail[1])} -> {json.dumps(detail[2])}'
            print(line)
    total = sum(len(v) for v in changes.values())
    print(f'\n{len(samples)} files: {total} differences, {len(problems)} problems')
    return 1 if changes or problems else 0


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('command', choices=['check', 'record'])
    p.add_argument('--dataset', type=Path, default=os.environ.get('RAWMAKASE_DATASET'))
    p.add_argument('--rawmakase', type=Path, default=ROOT / 'target/release/rawmakase')
    p.add_argument('--manifest', type=Path, help='a dataset.py manifest; default: every file')
    p.add_argument('--decoder', help="the build's decoder to use, when it has more than one")
    p.add_argument('--floats', type=Path, help='write full decodes that are not mosaics here')
    p.add_argument('--jobs', type=int, default=3, help='rawmakase processes at a time (each uses every core)')
    p.add_argument('--reason', help='why the baseline changes (record)')
    p.add_argument('--public', action='store_true', help='name only raw.pixls.us CC0 files')
    args = p.parse_args()
    try:
        if args.dataset is None:
            raise Setup('Set RAWMAKASE_DATASET or pass --dataset')
        if args.command == 'record' and not args.reason:
            raise Setup('record needs --reason')
        if args.floats:
            args.floats.mkdir(parents=True, exist_ok=True)
        samples = fingerprints.load_samples(args.dataset, args.manifest)
        results, problems = develop(args, samples)
        if args.command == 'record':
            if problems:
                raise Setup('Not recording a run with problems:\n  ' +
                            '\n  '.join(fingerprints.visible(problems, args.public)))
            record(args, samples, results)
            return 0
        return check(args, samples, results, problems)
    except Setup as e:
        print(f'error: {e}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
