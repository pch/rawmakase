#!/usr/bin/env python3
"""Measure Adobe's default frame for each camera and raw size, compare it with
the frame RAWmakase uses, and write data/adobe-frames.toml.

Adobe's frame is the default crop Camera Raw shows a photo in, and the frame
Lightroom's crops and local adjustments are relative to: the DNG's
DefaultCropOrigin and DefaultCropSize, inside its ActiveArea, as Adobe DNG
Converter writes them for the camera's RAW file. RAWmakase's frame is the
decoded image's default crop (`metadata.crop_*` of `rawmakase fingerprint`,
inside the visible area that starts at `header.facts.margins`). Both are
compared in the RAW's sensor coordinates: the stored raw data RAWmakase's
decoders read, before orientation (left, top, width, height).

DNG Converter stores some cameras' raw data trimmed. Its stored image is
placed in sensor coordinates by its size when it is the RAW's whole raw data;
otherwise by aligning its samples with the sensor values `rawmakase
fingerprint --sensor` writes (which needs numpy), else the file is reported
as unaligned and gives no row. A DNG file is its own Adobe output: its
ActiveArea and DefaultCrop are read from it directly (RAWmakase can read
them from the file, so they give no row either).

  # Files of the cameras and raw sizes to measure (one each is enough); each is
  # converted with Adobe DNG Converter -c -u -p0 -d <dir> <file> (uncompressed,
  # no preview) into a temporary directory:
  python3 scripts/cameras/adobe-frames.py measure --bin target/release/rawmakase \\
      --out frames.jsonl [--jobs 4] RAW...
  # Differences per make, and the files that could not be aligned:
  python3 scripts/cameras/adobe-frames.py summary frames.jsonl [--list]
  # The table the decoders read at the switch to Adobe's frame:
  python3 scripts/cameras/adobe-frames.py table frames.jsonl --write

`measure` appends to --out and skips files already in it, so an interrupted run
continues. Nothing of the RAW files or Adobe's output is kept beyond the numbers.
"""
import argparse
import concurrent.futures
import json
import os
import struct
import subprocess
import sys
import tempfile
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TABLE = ROOT / 'data/adobe-frames.toml'
CONVERTER = '/Applications/Adobe DNG Converter.app/Contents/MacOS/Adobe DNG Converter'
HEADER = """# Adobe's default frame for each camera and raw size: the default crop Camera
# Raw shows a photo in, and the frame Lightroom's crops and local adjustments are
# relative to (crates/rawmakase-raw/src/facts.rs, docs/raw-decoding.md). Read only
# once RAWmakase's own decoders develop photos in Adobe's frame; until then the
# frame is the one RAWmakase has always used.
#
# [[frame]] for the camera's non-DNG files of `raw_size` (sensor coordinates:
# the stored raw data, before orientation): `active`, the area Adobe develops
# (DNG ActiveArea), and `default_crop`, the frame inside it (DefaultCropOrigin
# and DefaultCropSize), both left, top, width, height in sensor coordinates.
# A DNG's own tags give its frame; it has no row.
#
# source is adobe-dng: Adobe DNG Converter 18.7's output for one file of the
# camera and raw size, placed in sensor coordinates by its size or by aligning
# its samples with the decoder's. Written by scripts/cameras/adobe-frames.py.
"""

# TIFF field types: size in bytes and struct code.
TYPES = {1: (1, 'B'), 2: (1, 'c'), 3: (2, 'H'), 4: (4, 'I'), 5: (8, 'II'), 6: (1, 'b'),
         7: (1, 'B'), 8: (2, 'h'), 9: (4, 'i'), 10: (8, 'ii'), 11: (4, 'f'), 12: (8, 'd'),
         13: (4, 'I'), 16: (8, 'Q'), 17: (8, 'q'), 18: (8, 'Q')}


class Tiff:
    """A bounded reader of a TIFF file's directories (enough for DNG tags)."""

    def __init__(self, data):
        self.d = data
        if data[:2] == b'II':
            self.e = '<'
        elif data[:2] == b'MM':
            self.e = '>'
        else:
            raise ValueError('not a TIFF file')
        if self.u(2, 'H') != 42:
            raise ValueError('not a classic TIFF file')

    def u(self, at, code):
        return struct.unpack_from(self.e + code, self.d, at)[0]

    def ifd(self, at):
        """The directory at `at`: {tag: values}, and the next directory's offset."""
        if at <= 0 or at + 2 > len(self.d):
            return {}, 0
        n = self.u(at, 'H')
        tags = {}
        for i in range(min(n, 4096)):
            e = at + 2 + 12 * i
            if e + 12 > len(self.d):
                break
            tag, typ, count = struct.unpack_from(self.e + 'HHI', self.d, e)
            if typ not in TYPES or count > 1 << 24:
                continue
            size, code = TYPES[typ]
            where = e + 8 if size * count <= 4 else self.u(e + 8, 'I')
            if where + size * count > len(self.d):
                continue
            if typ == 2:
                tags[tag] = self.d[where:where + count].split(b'\0')[0].decode('latin-1')
                continue
            values = struct.unpack_from(self.e + code * count, self.d, where)
            if typ in (5, 10):
                values = tuple(values[k] / values[k + 1] if values[k + 1] else 0.
                               for k in range(0, len(values), 2))
            tags[tag] = values
        nxt = at + 2 + 12 * n
        return tags, (self.u(nxt, 'I') if nxt + 4 <= len(self.d) else 0)

    def directories(self):
        """Every directory: IFD0's chain and their SubIFDs, depth first."""
        seen, out = set(), []

        def walk(at, depth):
            while at and at not in seen and depth < 4 and len(out) < 64:
                seen.add(at)
                tags, nxt = self.ifd(at)
                out.append(tags)
                for sub in tags.get(330, ()):
                    walk(sub, depth + 1)
                at = nxt

        walk(self.u(4, 'I'), 0)
        return out


def read_dng(path, pixels):
    """A DNG's raw directory facts, and its stored samples when `pixels` (a list of
    rows of floats, the mean of a pixel's samples), else None."""
    data = Path(path).read_bytes()
    t = Tiff(data)
    dirs = t.directories()
    ifd0 = dirs[0] if dirs else {}
    raw = next((d for d in dirs if d.get(254, (1,))[0] == 0 and d.get(262, (0,))[0] in (32803, 34892)),
               None)
    if raw is None:
        raise ValueError('no raw image directory')
    width, height = raw[256][0], raw[257][0]
    top, left, bottom, right = raw.get(50829, (0, 0, height, width))
    origin = raw.get(50719, (0, 0))
    size = raw.get(50720, (right - left, bottom - top))
    facts = {
        'stored': [width, height],
        'active': [left, top, right - left, bottom - top],
        'crop_origin': list(origin),
        'crop_size': list(size),
        'default_scale': list(raw.get(50718, (1., 1.))),
        'orientation': ifd0.get(274, (1,))[0],
        'unique_model': ifd0.get(50708, ''),
    }
    samples = stored_samples(t, raw, width, height) if pixels else None
    return facts, samples


def stored_samples(t, raw, width, height):
    """An uncompressed raw directory's samples as a float32 array (height, width),
    the mean over samples per pixel; None for other encodings."""
    import numpy as np
    if raw.get(259, (1,))[0] != 1:
        return None
    bits = raw.get(258, (16,))[0]
    spp = raw.get(277, (1,))[0]
    if bits not in (8, 16):
        return None
    dtype = np.dtype(t.e + ('u1' if bits == 8 else 'u2'))
    out = np.zeros((height, width), dtype=np.float32)
    if 324 in raw:
        tw, th = raw[322][0], raw[323][0]
        across = (width + tw - 1) // tw
        for i, (off, n) in enumerate(zip(raw[324], raw[325])):
            tile = np.frombuffer(t.d, dtype, count=n // dtype.itemsize, offset=off)
            if tile.size < tw * th * spp:
                return None
            tile = tile[:tw * th * spp].reshape(th, tw, spp).mean(axis=2)
            y, x = (i // across) * th, (i % across) * tw
            h, w = min(th, height - y), min(tw, width - x)
            out[y:y + h, x:x + w] = tile[:h, :w]
        return out
    rows = raw.get(278, (height,))[0]
    for i, (off, n) in enumerate(zip(raw[273], raw[279])):
        y = i * rows
        h = min(rows, height - y)
        strip = np.frombuffer(t.d, dtype, count=min(n // dtype.itemsize, h * width * spp), offset=off)
        if strip.size < h * width * spp:
            return None
        out[y:y + h] = strip.reshape(h, width, spp).mean(axis=2)
    return out


def our_frame(m, facts):
    """RAWmakase's frame in sensor coordinates, as `ImageFrame` places it: the
    default crop inside the decoded image, centred when its position is unknown,
    the whole image without one."""
    w, h = m['width'], m['height']
    cw = m['crop_width'] if 0 < m['crop_width'] <= w else w
    ch = m['crop_height'] if 0 < m['crop_height'] <= h else h
    left = m['crop_left'] if m['crop_left'] + cw <= w else (w - cw) / 2
    top = m['crop_top'] if m['crop_top'] + ch <= h else (h - ch) / 2
    mx, my = facts['margins']
    return [mx + left, my + top, cw, ch]


def align(ours, ours_origin, adobe):
    """Where Adobe's stored image starts in sensor coordinates, and how it was
    found, or None. Candidate offsets lie near the 2 × 2-binned phase
    correlation's and near the images lined up at their corners and centres.
    Each is scored over the patches spread across Adobe's image that tell
    positions apart (not flat, saturated or repeating) and lie in the overlap,
    at least three: the share of samples equal, and the correlation of their
    logarithms. An offset is kept only when it is clearly the one: at least 90%
    of samples equal, a quarter more than at any other offset, else a
    correlation of at least 0.9 that beats every other offset's by 0.02 or
    leaves at most half as much uncorrelated; inside its candidate window (not
    on its edge), with no offset within two pixels of it better, and ahead of
    the offsets up to sixteen pixels away along each axis and diagonal too. A
    repeating or saturated region thus never decides alone, and an image that
    cannot be placed gives no row."""
    import numpy as np

    def binned(a):
        h, w = a.shape[0] // 2 * 2, a.shape[1] // 2 * 2
        a = np.log1p(np.clip(a[:h, :w], 0, None))
        return 0.25 * (a[0::2, 0::2] + a[1::2, 0::2] + a[0::2, 1::2] + a[1::2, 1::2])

    a, b = binned(adobe), binned(ours)
    n = min(1024, a.shape[0], a.shape[1], b.shape[0], b.shape[1])
    if n < 32:
        return None
    ay, ax = (a.shape[0] - n) // 2, (a.shape[1] - n) // 2
    by, bx = (b.shape[0] - n) // 2, (b.shape[1] - n) // 2
    wa, wb = a[ay:ay + n, ax:ax + n], b[by:by + n, bx:bx + n]
    wa, wb = wa - wa.mean(), wb - wb.mean()
    r = np.conj(np.fft.rfft2(wa)) * np.fft.rfft2(wb)
    c = np.fft.irfft2(r / (np.abs(r) + 1e-9), s=(n, n))
    sy, sx = np.unravel_index(int(np.argmax(c)), c.shape)
    sy, sx = (sy - n if sy > n // 2 else sy), (sx - n if sx > n // 2 else sx)
    # adobe[y, x] ~ ours[y + dy, x + dx], in binned pixels.
    cy, cx = by + sy - ay, bx + sx - ax
    centred = ((ours.shape[1] - adobe.shape[1]) // 2, (ours.shape[0] - adobe.shape[0]) // 2)
    anchors = ((2 * cx, 2 * cy), (0, 0), centred)
    offsets = sorted({(dx, dy) for ox, oy in anchors
                      for dy in range(oy - 3, oy + 4) for dx in range(ox - 3, ox + 4)})
    patches = informative_patches(adobe)
    logs = {at: np.log1p(np.clip(adobe[at[0]:at[0] + at[2], at[1]:at[1] + at[2]], 0, None)).ravel()
            for at in patches}

    def score(dx, dy):
        """(share equal, correlation) over the patches inside the overlap at
        this offset; None with fewer than three."""
        exact, corr = [], []
        inside = [(py, px, size) for py, px, size in patches
                  if py + dy >= 0 and px + dx >= 0 and py + dy + size <= ours.shape[0]
                  and px + dx + size <= ours.shape[1]]
        if len(inside) < 3:
            return None
        for (py, px, size) in inside:
            p = adobe[py:py + size, px:px + size]
            q = ours[py + dy:py + dy + size, px + dx:px + dx + size]
            exact.append(float(np.mean(q == p)))
            cc = np.corrcoef(logs[(py, px, size)], np.log1p(np.clip(q, 0, None)).ravel())[0, 1]
            corr.append(float(np.nan_to_num(cc)))
        return float(np.mean(exact)), float(np.mean(corr))

    scored = {d: score(*d) for d in offsets}
    scores = [(e, c, dx, dy) for (dx, dy), v in scored.items() if v for e, c in [v]]
    if len(scores) < 2:
        return None
    by_exact = sorted(scores, reverse=True)
    exact_mode = by_exact[0][0] >= 0.9
    # The measure that decides: the share equal, else the correlation.
    rank = (lambda e, c: (e, c)) if exact_mode else (lambda e, c: (c, e))
    exact, corr, dx, dy = max(scores, key=lambda t: rank(t[0], t[1]))
    # The best must lie inside a window, not on its edge, and no offset around it
    # may do better: an offset beyond the windows would otherwise lose to the
    # edge one a filter period short of it.
    if not any(abs(dx - ox) < 3 and abs(dy - oy) < 3 for ox, oy in anchors):
        return None
    # Its neighbours, and offsets up to sixteen pixels along each axis and
    # diagonal: on an image that repeats itself, those score as well as it.
    around = [(dx + i, dy + j) for j in range(-2, 3) for i in range(-2, 3)]
    around += [(dx + sx * k, dy + sy * k) for k in range(3, 17)
               for sx, sy in ((1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (-1, -1), (1, -1), (-1, 1))]
    for nx, ny in around:
        if (nx, ny) in scored:
            continue
        v = scored[(nx, ny)] = score(nx, ny)
        if v:
            scores.append((v[0], v[1], nx, ny))
        if v and rank(*v) > rank(exact, corr):
            return None
    others = [t for t in scores if (t[2], t[3]) != (dx, dy)]
    if exact_mode:
        if exact - max(t[0] for t in others) < 0.25:
            return None
        how = 'pixels-exact'
    else:
        # Clearly ahead of every other offset: by 0.02, or with at most half
        # the decorrelation (near 1, the same colours a filter period away come
        # close in correlation, never in what is left of it).
        runner_up = max(t[1] for t in others)
        if corr < 0.9 or (corr - runner_up < 0.02 and 1. - runner_up < 2. * (1. - corr)):
            return None
        how = 'pixels-correlated'
    # Stored sample (x, y) is visible sample (x + dx, y + dy).
    return [ours_origin[0] + dx, ours_origin[1] + dy], how, round(exact, 4), round(corr, 4)


def informative_patches(adobe, grid=8):
    """Square patches (top, left, size) of Adobe's image on a `grid` × `grid`
    lattice whose values vary (not flat), are not mostly at their extremes
    (saturated or black) and do not repeat themselves along."""
    import numpy as np
    h, w = adobe.shape
    size = min(128, h // (grid + 1), w // (grid + 1))
    if size < 16:
        return []
    out = []
    for i in range(grid):
        for j in range(grid):
            py = int((i + 0.5) / grid * (h - size))
            px = int((j + 0.5) / grid * (w - size))
            p = adobe[py:py + size, px:px + size]
            if np.std(np.log1p(np.clip(p, 0, None))) < 0.02:
                continue
            if np.mean(p == p.max()) > 0.5 or np.mean(p == p.min()) > 0.5:
                continue
            # Nor repeating: mostly equal to itself shifted along by up to
            # sixteen pixels (further than any two candidate offsets differ).
            if any(np.mean(p[sy:, sx:] == p[:size - sy, :size - sx]) > 0.5
                   for k in range(1, 17) for sy, sx in ((0, k), (k, 0), (k, k))):
                continue
            out.append((py, px, size))
    return out


def fingerprint(binary, decoder, path, sensor_dir):
    args = [binary, 'fingerprint', '--decoder', decoder]
    if sensor_dir:
        args += ['--sensor', sensor_dir]
    out = subprocess.run(args + [path], capture_output=True, text=True, timeout=900)
    lines = out.stdout.strip().splitlines()
    if not lines:
        raise RuntimeError(out.stderr.strip()[-300:] or 'no fingerprint')
    return json.loads(lines[-1])


def measure_one(args, path):
    """The record of one RAW file."""
    rec = {'path': str(path)}
    with tempfile.TemporaryDirectory() as work:
        try:
            f = fingerprint(args.bin, args.decoder, str(path), work)
        except Exception as e:  # noqa: BLE001 - a tool's failure is the record
            return {**rec, 'error': f'fingerprint: {e}'}
        if 'header' not in f or 'sensor' not in f:
            return {**rec, 'error': f"fingerprint: {f.get('error', 'no header')}"}
        m, facts, s = f['header']['metadata'], f['header']['facts'], f['sensor']
        rec.update(sha256=f['sha256'], make=m['make'], model=m['model'],
                   dng=facts['dng_version'] > 0, raw_size=[m['raw_width'], m['raw_height']],
                   visible=[*s['origin'], *s['area']], channels=s['channels'], flip=m['flip'],
                   ours=our_frame(m, facts))
        if rec['dng']:
            dng = str(path)
        else:
            out = os.path.join(work, 'out')
            os.mkdir(out)
            subprocess.run([args.converter, '-c', '-u', '-p0', '-d', out, str(path)],
                           capture_output=True, timeout=900)
            made = os.listdir(out)
            if not made:
                return {**rec, 'error': 'DNG Converter wrote nothing'}
            dng = os.path.join(out, made[0])
        try:
            a, stored = read_dng(dng, pixels=not rec['dng'])
        except Exception as e:  # noqa: BLE001
            return {**rec, 'error': f'DNG: {e}'}
        rec['adobe'] = a
        if a['stored'] == rec['raw_size']:
            rec['placed'], rec['how'] = [0, 0], 'size'
        elif stored is not None:
            import numpy as np
            w, h = s['area']
            values = np.fromfile(os.path.join(work, f"{f['sha256']}.sensor"), dtype='<u2')
            if values.size == w * h * s['channels']:
                ours = values.reshape(h, w, s['channels']).mean(axis=2).astype(np.float32)
                found = align(ours, s['origin'], stored)
                if found:
                    rec['placed'], rec['how'], rec['exact'], rec['corr'] = found
        if 'placed' not in rec:
            rec['how'] = 'unaligned'
            return rec
        px, py = rec['placed']
        al, at, aw, ah = a['active']
        (ox, oy), (cw, ch) = a['crop_origin'], a['crop_size']
        rec['adobe_active'] = [px + al, py + at, aw, ah]
        rec['adobe_frame'] = [px + al + ox, py + at + oy, cw, ch]
        return rec


def measure(args):
    done = set()
    if os.path.exists(args.out):
        with open(args.out) as fh:
            done = {json.loads(line)['path'] for line in fh if line.strip()}
    todo = [p for p in args.files if str(p) not in done]
    print(f'{len(todo)} files to measure', file=sys.stderr)
    with open(args.out, 'a') as out, concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
        for n, rec in enumerate(pool.map(lambda p: measure_one(args, p), todo)):
            out.write(json.dumps(rec) + '\n')
            out.flush()
            print(f'\r{n + 1}/{len(todo)}', end='', file=sys.stderr, flush=True)
    print(file=sys.stderr)


def load(path):
    with open(path) as fh:
        return [json.loads(line) for line in fh if line.strip()]


def insets(ours, adobe):
    """Adobe's frame against ours, per side (left, top, right, bottom): how many
    pixels Adobe's lies inside ours (negative: outside)."""
    return [round(adobe[0] - ours[0], 2), round(adobe[1] - ours[1], 2),
            round(ours[0] + ours[2] - adobe[0] - adobe[2], 2),
            round(ours[1] + ours[3] - adobe[1] - adobe[3], 2)]


def summary(args):
    recs = load(args.frames)
    by_make = defaultdict(Counter)
    sides = defaultdict(Counter)
    listed = defaultdict(list)
    for r in recs:
        make = r.get('make', '?')
        if 'error' in r:
            by_make[make]['error'] += 1
            listed[make].append(f"  error {r['path']}: {r['error']}")
            continue
        kind = 'dng' if r['dng'] else 'raw'
        if r['how'] == 'unaligned':
            by_make[make][f'{kind} unaligned'] += 1
            listed[make].append(f"  unaligned {r['model']} {r['raw_size']}: stored {r['adobe']['stored']}")
            continue
        d = insets(r['ours'], r['adobe_frame'])
        same = all(abs(v) < 0.5 for v in d)
        by_make[make][f"{kind} {'same' if same else 'differs'}"] += 1
        vl, vt, vw, vh = r['visible']
        fl, ft, fw, fh = r['adobe_frame']
        if not (fl >= vl and ft >= vt and fl + fw <= vl + vw and ft + fh <= vt + vh):
            by_make[make][f'{kind} beyond visible'] += 1
        why = usable(r)
        if why:
            by_make[make][f'{kind} {why}'] += 1
        flip = {1: 0, 3: 3, 6: 6, 8: 5}.get(r['adobe']['orientation'], -1)
        if flip != r['flip']:
            by_make[make][f'{kind} orientation differs'] += 1
        if not same:
            sides[make][tuple(d)] += 1
            listed[make].append(f"  {r['model']} {r['raw_size']} ({r['how']}): ours {r['ours']} "
                                f"Adobe {r['adobe_frame']} inset l,t,r,b {d}")
    for make in sorted(by_make):
        counts = ', '.join(f'{k} {v}' for k, v in sorted(by_make[make].items()))
        common = ', '.join(f'{list(k)}×{v}' for k, v in sides[make].most_common(3))
        print(f'{make}: {counts}' + (f'; commonest insets (l,t,r,b) {common}' if common else ''))
        if args.list:
            print('\n'.join(listed[make]))


def aspect(rect):
    return rect[2] / rect[3]


def other_shape(a, b, tolerance=0.03):
    """Whether rectangles `a` and `b` differ in aspect ratio by more than
    `tolerance`."""
    return abs(aspect(a) / aspect(b) - 1) > tolerance


def usable(r):
    """Why a placed record gives no row, or None: pixels that are not square
    (DefaultScale differs by more than 1% by axis), a frame that is not whole
    pixels, areas outside the RAW's raw data (the decoders read none of it),
    or a crop that is the shot's aspect ratio, not the camera's: of another
    shape than both Adobe's active area and RAWmakase's frame (where the raw
    size gives the aspect ratio, as on Panasonic's multi-aspect sensors,
    RAWmakase's frame has it too), so other files of the camera and raw size,
    shot at another ratio, would take it."""
    sx, sy = r['adobe']['default_scale']
    if abs(sx - sy) > 0.01 * max(sx, sy):
        return 'pixels not square'
    if any(v != round(v) for v in r['adobe_frame'] + r['adobe_active']):
        return 'not whole pixels'
    w, h = r['raw_size']
    for left, top, width, height in (r['adobe_active'], r['adobe_frame']):
        if left < 0 or top < 0 or left + width > w or top + height > h:
            return 'outside the raw data'
    crop = r['adobe_frame']
    if other_shape(crop, r['adobe_active']) and other_shape(crop, r['ours']):
        return "the shot's aspect ratio"
    return None


def table(args):
    """One row per camera and raw size, from its non-DNG files that were placed;
    files of one key that disagree are reported and the commonest frame kept."""
    rows = defaultdict(Counter)
    for r in load(args.frames):
        if 'error' in r or r['dng'] or r['how'] == 'unaligned':
            continue
        why = usable(r)
        if why:
            print(f"# skipped ({why}) {r['make']} {r['model']} {r['raw_size']}", file=sys.stderr)
            continue
        active, frame = (tuple(round(v) for v in r[k]) for k in ('adobe_active', 'adobe_frame'))
        rows[(r['make'], r['model'], tuple(r['raw_size']))][(active, frame)] += 1
    text = [HEADER]
    for (make, model, size), found in sorted(rows.items()):
        if len(found) > 1:
            print(f'# files disagree for {make} {model} {list(size)}: {dict(found)}', file=sys.stderr)
        (active, frame), _ = found.most_common(1)[0]
        text.append(f'[[frame]]\nmake = {json.dumps(make)}\nmodel = {json.dumps(model)}\n'
                    f'raw_size = [{size[0]}, {size[1]}]\nactive = {list(active)}\n'
                    f'default_crop = {list(frame)}\nsource = "adobe-dng"\n')
    out = '\n'.join(text)
    if args.write:
        TABLE.write_text(out)
        print(f'{len(rows)} rows written to {TABLE.relative_to(ROOT)}', file=sys.stderr)
    else:
        sys.stdout.write(out)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n\n')[0],
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest='command', required=True)
    m = sub.add_parser('measure')
    m.add_argument('--bin', required=True, help='the rawmakase binary')
    m.add_argument('--decoder', default='own', choices=['own', 'libraw'])
    m.add_argument('--converter', default=CONVERTER)
    m.add_argument('--out', required=True)
    m.add_argument('--jobs', type=int, default=2)
    m.add_argument('files', nargs='+', type=Path)
    s = sub.add_parser('summary')
    s.add_argument('frames')
    s.add_argument('--list', action='store_true')
    t = sub.add_parser('table')
    t.add_argument('frames')
    t.add_argument('--write', action='store_true')
    args = ap.parse_args()
    {'measure': measure, 'summary': summary, 'table': table}[args.command](args)


if __name__ == '__main__':
    main()
