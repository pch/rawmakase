#!/usr/bin/env python3
"""Decode fingerprints of the classified test dataset (issue #420, level 1).

Runs `rawmakase fingerprint` on the dataset's RAW files and compares what
opening and unpacking each one yields (whether it opens, sizes, crop,
orientation, levels, CFA, white balance, matrix and a checksum of the unpacked
data) with a recorded baseline. Run it on every LibRaw update and before
releases.

  check    fingerprint the files and report every difference from the baselines,
           grouped by make and format; exits 1 when anything changed
  record   write the baselines from this build; --reason is required, and a
           baseline only changes in a commit that says why

Baselines: raw.pixls.us CC0 files in tests/corpus/dataset/pixls-decode.json
(committed); every other file (raw.pixls.us CC-BY-NC-SA, Signature Edits, the
owner's photos) in $RAWMAKASE_DATASET/baselines/decode/<source>.json, which
never leaves the dataset's machine. Photos are named by SHA-256.

The dataset is pch/rawmakase-dataset with its RAWs in place
($RAWMAKASE_DATASET, or --dataset). By default every file of every source is
fingerprinted; --manifest takes the files of a `dataset.py manifest` output.
Run from the repository root with a release build (--rawmakase):

  python3 scripts/corpus/dataset-fingerprints.py check
  python3 scripts/corpus/dataset-fingerprints.py record --reason "LibRaw 0.23"

Exit status: 0 no difference, 1 differences, 2 the run could not be made
(no dataset, no baseline, files missing).
"""
import argparse
import concurrent.futures
import datetime
import json
import os
import platform
import re
import sqlite3
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
COMMITTED = ROOT / 'tests/corpus/dataset/pixls-decode.json'
PUBLIC_LICENSE = 'CC0-1.0'
DELTAS = ROOT / 'tests/corpus/decode-deltas.toml'
# Facts about the run rather than the file, kept out of the comparison.
NOT_COMPARED = {'path', 'libraw', 'decoder'}
BATCH = 32
# A file takes seconds; a batch that runs this long has hung.
BATCH_TIMEOUT = 900
FILE_TIMEOUT = 300


class Setup(Exception):
    """The run cannot be made; never a pass."""


def committed(sample):
    """Whether a sample's baseline may be committed: raw.pixls.us CC0 files only."""
    return sample['source'] == 'pixls' and sample['license'] == PUBLIC_LICENSE


def baseline_path(dataset, sample):
    return COMMITTED if committed(sample) else dataset / 'baselines/decode' / f'{sample["source"]}.json'


def load_samples(dataset, manifest):
    """The selected samples, each with its file's path. Fails on files the
    database lists without a file, and on manifest files it does not list."""
    db_path = dataset / 'dataset.sqlite'
    if not db_path.is_file():
        raise Setup(f'No dataset at {dataset} (no dataset.sqlite): is the SSD mounted? '
                    'Set RAWMAKASE_DATASET or pass --dataset.')
    db = sqlite3.connect(f'file:{db_path}?mode=ro', uri=True)
    folders = dict(db.execute('SELECT source, folder FROM sources'))
    rows = db.execute('SELECT sha256, source, path, make, model, ext, license FROM samples ORDER BY sha256')
    samples = {r[0]: dict(zip(('sha256', 'source', 'path', 'make', 'model', 'ext', 'license'), r)) for r in rows}
    if manifest:
        wanted = {f['sha256'] for f in json.loads(manifest.read_text())['files']}
        unknown = wanted - samples.keys()
        if unknown:
            # Not named: a hash the dataset does not know may be a private file's.
            raise Setup(f'{len(unknown)} manifest files are not in the dataset')
        samples = {h: s for h, s in samples.items() if h in wanted}
    missing = []
    for s in samples.values():
        folder = folders.get(s['source'])
        s['file'] = dataset / folder / s['path'] if folder and s['path'] else None
        if s['file'] is None or not s['file'].is_file():
            missing.append(s)
    if missing:
        # Counted by source; only a publishable file is named.
        counts = Counter(s['source'] for s in missing)
        public = [s['sha256'] for s in missing if committed(s)]
        raise Setup(f'{len(missing)} files are missing from {dataset} (' +
                    ', '.join(f'{n} {source}' for source, n in sorted(counts.items())) + ')' +
                    (f', e.g. {public[0]}' if public else ''))
    if not samples:
        raise Setup('No files selected')
    return samples


def run(rawmakase, files, timeout, decoder=None):
    """One JSON object per file, in order; None when the process failed or its
    output does not match the files."""
    command = [str(rawmakase), 'fingerprint', *(['--decoder', decoder] if decoder else []), *map(str, files)]
    try:
        p = subprocess.run(command, capture_output=True,
                           text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return None, f'timed out after {timeout} s'
    # Not its stderr, which may name a private file.
    if p.returncode != 0:
        return None, f'exit status {p.returncode}'
    try:
        lines = [json.loads(line) for line in p.stdout.splitlines()]
    except json.JSONDecodeError as e:
        return None, f'unreadable output: {e}'
    if [line.get('path') for line in lines] != [str(f) for f in files]:
        return None, f'{len(lines)} lines for {len(files)} files'
    return lines, None


def fingerprint(rawmakase, samples, jobs, decoder=None):
    """Fingerprints by SHA-256, and the run's problems as (publishable, text).
    A batch that fails is
    run again file by file: a file that fails alone gets the failure as its
    fingerprint (a crash is a regression too); the batch's failure stays a
    problem of the run even when every file then succeeds."""
    order = sorted(samples.values(), key=lambda s: (s['source'], s['make'] or '', s['model'] or '', s['sha256']))
    batches = [order[i:i + BATCH] for i in range(0, len(order), BATCH)]
    results, problems, done = {}, [], 0

    def batch(chunk):
        lines, error = run(rawmakase, [s['file'] for s in chunk], BATCH_TIMEOUT, decoder)
        if lines is not None:
            return list(zip(chunk, lines)), None
        out = []
        for s in chunk:
            alone, why = run(rawmakase, [s['file']], FILE_TIMEOUT, decoder)
            out.append((s, alone[0] if alone else {'failed': 'crash', 'error': why}))
        return out, f'a batch of {len(chunk)} files failed ({error}); its files were run one by one'

    with concurrent.futures.ThreadPoolExecutor(jobs) as pool:
        for out, problem in pool.map(batch, batches):
            if problem:
                problems.append((True, problem))
            for s, line in out:
                line.pop('path', None)
                # A file that could not be read says nothing about decoding, and
                # must not replace a baseline entry.
                if line.get('failed') == 'crash':
                    line['sha256'] = s['sha256']
                elif line.get('failed') == 'read' or line.get('sha256') is None:
                    problems.append((committed(s), f'{s["sha256"]}: could not be read ({line.get("error")})'))
                    line['sha256'] = s['sha256']
                elif line['sha256'] != s['sha256']:
                    # Another file, whose rights are unknown: nothing of it is kept.
                    problems.append((committed(s), f'{s["sha256"]}: the file on disk is not the dataset\'s file'))
                    line = {'sha256': s['sha256'], 'failed': 'read', 'error': 'not the dataset\'s file'}
                results[s['sha256']] = line
            done += len(out)
            print(f'\r{done}/{len(order)} files', end='', file=sys.stderr, flush=True)
    print(file=sys.stderr)
    return results, problems


def read_baseline(path):
    if not path.is_file():
        return None
    data = json.loads(path.read_text())
    return data['recorded'], {f['sha256']: f for f in data['files']}


def write_baseline(path, recorded, files):
    """One file per line, sorted by SHA-256, so a diff shows what moved."""
    path.parent.mkdir(parents=True, exist_ok=True)
    about = ('Decode fingerprints (rawmakase fingerprint) of the raw.pixls.us CC0 files in the classified '
             'test dataset, recorded by scripts/corpus/dataset-fingerprints.py. Change only in a commit '
             'that says why.') if path == COMMITTED else \
        'Decode fingerprints recorded by RAWmakase scripts/corpus/dataset-fingerprints.py. Local only.'
    lines = [json.dumps(files[h], separators=(',', ':')) for h in sorted(files)]
    path.write_text('{"about": ' + json.dumps(about) + ',\n "recorded": ' + json.dumps(recorded) +
                    ',\n "files": [\n' + ',\n'.join(lines) + '\n]}\n')


def recorded_by(results, reason):
    commit = subprocess.run(['git', 'rev-parse', 'HEAD'], cwd=ROOT, capture_output=True, text=True).stdout.strip()
    dirty = subprocess.run(['git', 'status', '--porcelain', '--untracked-files=no'], cwd=ROOT,
                           capture_output=True, text=True).stdout.strip()
    decoder = sorted({r.get('decoder') for r in results.values() if r.get('decoder')})
    # LibRaw's version string does not name a master commit; this is the one
    # packaging/native-deps.sh builds, which the build is expected to link.
    pinned = re.search(r'^libraw=(\w+)', (ROOT / 'packaging/native-deps.sh').read_text(), re.M)
    return {'reason': reason, 'date': datetime.date.today().isoformat(),
            'rawmakase': commit + ('-dirty' if dirty else ''), 'decoder': ', '.join(decoder),
            'libraw_pinned': pinned[1] if pinned else None,
            'platform': f'{platform.system()} {platform.machine()}'}


def record(args, samples, results):
    by_path = defaultdict(dict)
    for h, s in samples.items():
        by_path[baseline_path(args.dataset, s)][h] = {k: v for k, v in results[h].items() if k not in NOT_COMPARED}
    recorded = recorded_by(results, args.reason)
    for path, files in sorted(by_path.items()):
        old = read_baseline(path)
        # A selection replaces its own files; the rest of the baseline stays.
        merged = {**(old[1] if old else {}), **files}
        write_baseline(path, recorded, merged)
        print(f'{len(files)} files recorded in {path} ({len(merged)} in all)')


def flatten(fingerprint, prefix=''):
    """A fingerprint's fields by dotted name (`header.metadata.wb`); lists are values."""
    out = {}
    for k, v in fingerprint.items():
        if not prefix and k in NOT_COMPARED:
            continue
        if isinstance(v, dict):
            out.update(flatten(v, f'{prefix}{k}.'))
        else:
            out[prefix + k] = v
    return out


ABSENT = {'absent': True}


def load_deltas(path=DELTAS):
    """Approved decode differences, by (file SHA-256, field): (before, after).
    A difference whose approval is "pending" is listed but not approved. A
    field the fingerprint does not have (null in its JSON) is written
    `{ absent = true }`, as TOML has no null."""
    import tomllib
    if not path.is_file():
        return {}
    deltas = {}
    for d in tomllib.loads(path.read_text()).get('delta', []):
        for key in ('sha256', 'field', 'before', 'after', 'evidence', 'approval'):
            if key not in d:
                raise Setup(f'{path.name}: a delta without {key}')
        values = []
        for key in ('before', 'after'):
            v = d[key]
            if isinstance(v, dict) and v != ABSENT:
                raise Setup(f'{path.name}: {key} of {d["field"]} is a table other than {{ absent = true }}')
            values.append(None if v == ABSENT else v)
        if d['approval'] == 'pending':
            continue
        deltas[(d['sha256'], d['field'])] = tuple(values)
    return deltas


def compare(old, new, sha256=None, deltas=None):
    """The changed fields: name, before, after; a change the delta file
    approves (the same before and after) is not one."""
    old, new = flatten(old), flatten(new)
    keys = sorted(old.keys() | new.keys())
    deltas = deltas or {}
    return [(k, old.get(k), new.get(k)) for k in keys
            if old.get(k) != new.get(k) and deltas.get((sha256, k)) != (old.get(k), new.get(k))]


def check(args, samples, results, problems):
    baselines, notes = {}, []
    paths = {baseline_path(args.dataset, s) for s in samples.values()}
    # With every file selected, every baseline is expected, also one whose
    # source the dataset no longer has.
    if not args.manifest:
        paths |= {COMMITTED, *(args.dataset / 'baselines/decode').glob('*.json')}
    for path in sorted(paths):
        loaded = read_baseline(path)
        if loaded is None:
            raise Setup(f'No baseline at {path}: record one with `record --reason …`')
        baselines[path] = loaded[1]
        here = recorded_by(results, None)
        for key in ('decoder', 'libraw_pinned', 'platform'):
            if loaded[0].get(key) != here[key]:
                notes.append(f'{path.name} was recorded with {key} {loaded[0].get(key)}; this run has {here[key]}')
    # Every change, by (make, format): (kind, sample, detail).
    changes = defaultdict(list)
    deltas = load_deltas()
    for h, s in samples.items():
        old, new = baselines[baseline_path(args.dataset, s)].get(h), results[h]
        group = (s['make'] or '?', (s['ext'] or Path(s['path']).suffix[1:]).lower())
        if old is None:
            changes[group].append(('no baseline', s, None))
        elif 'failed' in old and 'failed' not in new:
            changes[group].append(('starts opening', s, None))
        elif 'failed' not in old and 'failed' in new:
            changes[group].append(('stops opening', s, new.get('error')))
        else:
            for field in compare(old, new, h, deltas):
                changes[group].append(('changed', s, field))
    # With every file selected, a baseline file the dataset no longer has is a change too.
    if not args.manifest:
        for path, files in baselines.items():
            for h in files.keys() - samples.keys():
                problems.append((path == COMMITTED, f'{h} is in {path.name} but not in the dataset'))
    report(args, samples, changes, problems, notes)
    return 1 if changes or problems else 0


def visible(problems, public):
    """The problems' texts; with --public, private files' only as a count."""
    hidden = sum(1 for shown, _ in problems if public and not shown)
    texts = [text for shown, text in problems if shown or not public]
    if hidden:
        texts.append(f'{hidden} problems with files of private sources (not shown)')
    return texts


def report(args, samples, changes, problems, notes):
    """Per file for publishable files; with --public, other sources only as counts."""
    def shown(s):
        return not args.public or committed(s)

    shown_problems = visible(problems, args.public)
    out = {'files': len(samples), 'notes': notes, 'problems': shown_problems, 'groups': []}
    for note in notes:
        print(f'note: {note}')
    for problem in shown_problems:
        print(f'problem: {problem}')
    for (make, ext), items in sorted(changes.items()):
        kinds = Counter(kind if kind != 'changed' else f'{detail[0]} changed' for kind, _, detail in items)
        print(f'\n{make} .{ext}: ' + ', '.join(f'{n} {k}' for k, n in sorted(kinds.items())))
        group = {'make': make, 'format': ext, 'counts': dict(kinds), 'files': []}
        for kind, s, detail in items:
            if not shown(s):
                continue
            entry = {'sha256': s['sha256'], 'model': s['model'], 'change': kind}
            line = f'  {s["sha256"][:12]} {s["model"]}: {kind}'
            if kind == 'changed':
                entry.update(field=detail[0], before=detail[1], after=detail[2])
                line += f' {detail[0]}: {json.dumps(detail[1])} -> {json.dumps(detail[2])}'
            elif detail:
                entry['error'] = detail
                line += f' ({detail})'
            group['files'].append(entry)
            print(line)
        out['groups'].append(group)
    total = sum(len(v) for v in changes.values())
    print(f'\n{len(samples)} files: {total} differences, {len(problems)} problems')
    if args.report:
        args.report.write_text(json.dumps(out, indent=1) + '\n')


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('command', choices=['check', 'record'])
    p.add_argument('--dataset', type=Path, default=os.environ.get('RAWMAKASE_DATASET'),
                   help='the dataset checkout (default: $RAWMAKASE_DATASET)')
    p.add_argument('--rawmakase', type=Path, default=ROOT / 'target/release/rawmakase')
    p.add_argument('--manifest', type=Path, help='a dataset.py manifest; default: every file')
    p.add_argument('--decoder', help="the build's decoder to use, when it has more than one")
    # Each process takes its files one at a time (see `rawmakase fingerprint`).
    p.add_argument('--jobs', type=int, default=min(8, os.cpu_count() or 4), help='rawmakase processes at a time')
    p.add_argument('--reason', help='why the baseline changes (record)')
    p.add_argument('--public', action='store_true',
                   help='name only raw.pixls.us CC0 files; count the others (for shared reports)')
    p.add_argument('--report', type=Path, help='also write the report as JSON')
    args = p.parse_args()
    try:
        if args.dataset is None:
            raise Setup('Set RAWMAKASE_DATASET or pass --dataset')
        if args.command == 'record' and not args.reason:
            raise Setup('record needs --reason')
        if not os.access(args.rawmakase, os.X_OK):
            raise Setup(f'No rawmakase build at {args.rawmakase}: cargo build --release, or pass --rawmakase')
        samples = load_samples(args.dataset, args.manifest)
        results, problems = fingerprint(args.rawmakase, samples, args.jobs, args.decoder)
        if args.command == 'record':
            if problems:
                raise Setup('Not recording a run with problems:\n  ' + '\n  '.join(visible(problems, args.public)))
            record(args, samples, results)
            return 0
        return check(args, samples, results, problems)
    except Setup as e:
        print(f'error: {e}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
