#!/usr/bin/env python3
"""Reduce Camera Raw captures of the look channel-curve probes (#949) to patch rows.

  python3 measure/look-channels-hdr.py OUT.json CAPTURE_DIR...

Each CAPTURE_DIR is an `hdr_reference.py capture ... --factor 1` output. For every job
the script finds the probe DNG's patches (flat regions that differ from the 0.05
background), takes each patch's scene value from the DNG itself (sensor value times
2^BaselineExposure, in the probe's camera space, which is linear ProPhoto RGB), and the
mean of the render's linear Display P3 over the patch's interior (4 px inset), converted
to linear ProPhoto RGB (D50, Bradford from D65). Graphics white is 1 in both: the probe's
scene 1, and 203 cd/m² in the PQ output (HDR) or display white (SDR). It also tests
whether a look's curves apply per channel or RGBTone fashion on the colour patches that
stay inside Display P3 in every render compared.
"""
import json
import struct
import sys
from pathlib import Path

import numpy as np

P3_TO_XYZ_D65 = np.array([[0.4865709486, 0.2656676932, 0.1982172852],
                          [0.2289745641, 0.6917385218, 0.0792869141],
                          [0.0000000000, 0.0451133819, 1.0439443689]])
BRADFORD = np.array([[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]])
D65 = np.array([0.95047, 1., 1.08883])
D50 = np.array([0.96422, 1., 0.82521])
PROPHOTO_TO_XYZ_D50 = np.array([[0.7976749, 0.1351917, 0.0313534],
                                [0.2880402, 0.7118741, 0.0000857],
                                [0.0000000, 0.0000000, 0.8252100]])
ADAPT = np.linalg.inv(BRADFORD) @ np.diag((BRADFORD @ D50) / (BRADFORD @ D65)) @ BRADFORD
P3_TO_PROPHOTO = np.linalg.inv(PROPHOTO_TO_XYZ_D50) @ ADAPT @ P3_TO_XYZ_D65
BACKGROUND = 0.05005
INSET = 4


def read_probe(path):
    """Scene-linear camera RGB (= linear ProPhoto) of a probe DNG at half resolution, one
    value per RGGB quad, and BaselineExposure."""
    d = Path(path).read_bytes()
    ifd = struct.unpack('<I', d[4:8])[0]
    tags = {}
    for i in range(struct.unpack('<H', d[ifd:ifd + 2])[0]):
        e = ifd + 2 + 12 * i
        tag, kind, count = struct.unpack('<HHI', d[e:e + 8])
        size = {1: 1, 2: 1, 3: 2, 4: 4, 5: 8, 7: 1, 10: 8, 11: 4}[kind] * count
        raw = d[e + 8:e + 8 + size] if size <= 4 else d[struct.unpack('<I', d[e + 8:e + 12])[0]:][:size]
        if kind == 3:
            tags[tag] = struct.unpack(f'<{count}H', raw)
        elif kind == 4:
            tags[tag] = struct.unpack(f'<{count}I', raw)
        elif kind in (5, 10):
            r = struct.unpack(f'<{2 * count}{"I" if kind == 5 else "i"}', raw)
            tags[tag] = [r[2 * k] / r[2 * k + 1] for k in range(count)]
    w, h, offset = tags[256][0], tags[257][0], tags[273][0]
    black = tags.get(50714, [0])[0]
    mosaic = (np.frombuffer(d[offset:offset + 2 * w * h], '<u2').reshape(h, w) - black) / (65535 - black)
    exposure = tags.get(50730, [0.])[0]
    rgb = np.stack([mosaic[0::2, 0::2], (mosaic[0::2, 1::2] + mosaic[1::2, 0::2]) / 2, mosaic[1::2, 1::2]], -1)
    return rgb * 2 ** exposure / np.asarray(tags.get(50728, [1, 1, 1])), exposure


def runs(mask):
    out, start = [], None
    for i, v in enumerate(list(mask) + [False]):
        if v and start is None:
            start = i
        elif not v and start is not None:
            out.append((start, i - 1))
            start = None
    return out


def patches(scene):
    """Patch boxes (half-resolution, inclusive) in reading order."""
    content = ~np.isclose(scene, BACKGROUND, atol=1e-5).all(-1)
    boxes = []
    for y0, y1 in runs(content.any(1)):
        for x0, x1 in runs(content[y0:y1 + 1].any(0)):
            boxes.append((y0, y1, x0, x1))
    return boxes


def reduce_job(folder, record):
    scene, exposure = read_probe(record['job']['source'])
    p3 = np.load(folder / 'linear-p3.npy')
    render = p3 @ P3_TO_PROPHOTO.T
    if render.shape[:2] != (2 * scene.shape[0], 2 * scene.shape[1]):
        raise ValueError(f'{folder}: render is {render.shape}, probe {scene.shape}; capture with --factor 1')
    rows = []
    for y0, y1, x0, x1 in patches(scene):
        s = scene[y0 + 1:y1, x0 + 1:x1].reshape(-1, 3)
        box = np.s_[2 * y0 + INSET:2 * y1 + 2 - INSET, 2 * x0 + INSET:2 * x1 + 2 - INSET]
        r = render[box].reshape(-1, 3)
        row = dict(scene=[round(float(v), 6) for v in s.mean(0)],
                   rgb=[round(float(v), 6) for v in r.mean(0)],
                   rgb_sd=round(float(r.std(0).max()), 6))
        # A Display P3 channel at exactly 0 is the output's gamut clip: the row's ProPhoto
        # value is then not Camera Raw's pipeline value.
        if p3[box].reshape(-1, 3).mean(0).min() <= 1e-9:
            row['p3_clipped'] = True
        rows.append(row)
    return rows, exposure


def mapping(source, target, channel):
    """Interpolated function from the no-look gray render to a look's gray render, per channel,
    log-log, clamped to the ramp."""
    x = np.array([r['rgb'][channel] for r in source])
    y = np.array([r['rgb'][channel] for r in target])
    keep = (x > 1e-4) & (y > 1e-6)
    x, y = np.log(x[keep]), np.log(y[keep])
    order = np.argsort(x)
    x, y = x[order], y[order]
    return lambda v: np.exp(np.interp(np.log(np.maximum(v, 1e-6)), x, y))


def per_channel(maps, rgb):
    return np.array([maps[c](rgb[c]) for c in range(3)])


def rgb_tone(curve, rgb):
    """The DNG SDK's RGBTone: the curve on the largest and smallest channel, the middle one
    interpolated between them linearly in its input's position."""
    out = np.empty(3)
    lo, mid, hi = np.argsort(rgb)
    out[hi], out[lo] = curve(rgb[hi]), curve(rgb[lo])
    span = rgb[hi] - rgb[lo]
    out[mid] = out[lo] + (out[hi] - out[lo]) * ((rgb[mid] - rgb[lo]) / span if span > 0 else 0)
    return out


def clipped(rgb):
    """Whether a rendered patch reached the edge of the Display P3 output: Camera Raw's
    pipeline value is then unknown, so it can't test a model."""
    return (np.linalg.solve(P3_TO_PROPHOTO, rgb)).min() < 0.003


def score(name, predict, base, observed):
    errors = {'below white': [], 'above white': []}
    for b, o in zip(base, observed):
        n, obs = np.array(b['rgb']), np.array(o['rgb'])
        if clipped(n) or clipped(obs) or clipped(predict(n)):
            continue
        e = np.abs(np.log2(predict(n) / obs))
        errors['above white' if max(b['scene']) > 1 else 'below white'].extend(e)
    return {k: dict(model=name, patches_x_channels=len(v), median_stops=round(float(np.median(v)), 4),
                    p95_stops=round(float(np.quantile(v, .95)), 4), max_stops=round(float(np.max(v)), 4))
            for k, v in errors.items() if v}


def main():
    out = Path(sys.argv[1])
    cases = {}
    camera_raw = set()
    for capture in map(Path, sys.argv[2:]):
        for index, record in enumerate(json.loads((capture / 'manifest.json').read_text())):
            job = record['job']
            rows, exposure = reduce_job(capture / f'{index:03d}', record)
            camera_raw.add(record['camera_raw'])
            look = job.get('look')
            cases[job['tag']] = dict(
                probe=Path(job['source']).name, probe_sha256=record['source_sha256'],
                settings=job['settings'],
                look=dict(file=Path(look['file']).name, amount=look['amount']) if look else None,
                camera_raw=record['camera_raw'], output_transfer=record['output_transfer'],
                baseline_exposure=exposure, pixels_sha256=record['pixels_sha256'], rows=rows)
    ramp = {k: v['rows'] for k, v in cases.items() if k.startswith('ramp ')}
    patch = {k: v['rows'] for k, v in cases.items() if k.startswith('patches ')}
    tests = {}
    if all(k in ramp for k in ('ramp none L8', 'ramp channels a1 L8', 'ramp curve-only a1 L8',
                               'ramp channels-curve a1 L8')):
        none = ramp['ramp none L8']
        g = [mapping(none, ramp['ramp channels a1 L8'], c) for c in range(3)]
        f = [mapping(none, ramp['ramp channels-curve a1 L8'], c) for c in range(3)]
        h = mapping(none, ramp['ramp curve-only a1 L8'], 1)
        base = patch.get('patches none L8')
        if base and 'patches channels a1 L8' in patch:
            tests['synthetic-channels a1, HDR'] = [
                score('channels per channel', lambda n: per_channel(g, n), base, patch['patches channels a1 L8'])]
        if base and 'patches curve-only a1 L8' in patch:
            tests['synthetic-curve a1, HDR'] = [
                score('master curve RGBTone', lambda n: rgb_tone(h, n), base, patch['patches curve-only a1 L8']),
                score('master curve per channel', lambda n: per_channel([h] * 3, n), base,
                      patch['patches curve-only a1 L8'])]
        if base and 'patches channels-curve a1 L8' in patch:
            obs = patch['patches channels-curve a1 L8']
            tests['synthetic-channels-curve a1, HDR'] = [
                score('combined gray map per channel', lambda n: per_channel(f, n), base, obs),
                score('master RGBTone, then channels per channel',
                      lambda n: per_channel(g, rgb_tone(h, n)), base, obs),
                score('channels per channel, then master RGBTone',
                      lambda n: rgb_tone(h, per_channel(g, n)), base, obs),
                score('master per channel, then channels per channel',
                      lambda n: per_channel(g, per_channel([h] * 3, n)), base, obs)]
    document = dict(
        about=('Camera Raw renders of synthetic looks with per-channel point curves (#949) on probe '
               'DNGs. scene: the probe patch in linear ProPhoto RGB (camera space), 1 = graphics white. '
               'rgb: the rendered patch mean, linear ProPhoto RGB D50 (Bradford from linear Display P3 '
               'D65), 1 = graphics white (203 cd/m2 in PQ). rgb_sd: largest channel standard deviation '
               'over the patch. See measure/README.md.'),
        camera_raw=sorted(camera_raw), cases=cases, tests=tests)
    out.write_text(json.dumps(document, indent=1) + '\n')
    print(json.dumps(tests, indent=1))


if __name__ == '__main__':
    main()
