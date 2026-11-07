"""Compares RAWmakase's HDR masters with a captured Camera Raw HDR reference family.

Run `validate FAMILY BINARY OUT [--split validation|holdout] [--tag TAG]`. FAMILY is a
folder written by `hdr_reference.py capture`; BINARY is a RAWmakase build. Each HDR
job is rendered with `rawmakase render --hdr` from the job's source, its requested
settings (without CameraProfile, which is passed as the job's DCP) and written to OUT,
then reduced and compared as the reference was: linear Display P3, graphics white 1,
area mean in linear light. Nothing in FAMILY is written.

The metrics are the HDR tolerances of docs/records/hdr-stage1-results.md: luminance
error in stops above white and below 0.5, the peak, and ΔE_ITP (ITU-R BT.2124) with
graphics white at 203 cd/m².
"""
import argparse
import json
import re
import subprocess
from pathlib import Path

import numpy as np

import hdr_reference

# Linear ProPhoto RGB (D50) to linear Display P3 (D65): ProPhoto to XYZ D50, Bradford
# to D65, XYZ to P3.
PROPHOTO_TO_XYZ = np.array([[0.7976749, 0.1351917, 0.0313534],
                            [0.2880402, 0.7118741, 0.0000857],
                            [0.0, 0.0, 0.8252100]])
BRADFORD_D50_TO_D65 = np.array([[0.9555766, -0.0230393, 0.0631636],
                                [-0.0282895, 1.0099416, 0.0210077],
                                [0.0122982, -0.0204830, 1.3299098]])
XYZ_TO_P3 = np.array([[2.4934969, -0.9313836, -0.4027108],
                      [-0.8294890, 1.7626641, 0.0236247],
                      [0.0358458, -0.0761724, 0.9568845]])
PROPHOTO_TO_P3 = XYZ_TO_P3 @ BRADFORD_D50_TO_D65 @ PROPHOTO_TO_XYZ
P3_LUMA = np.array([0.2289745641, 0.6917385218, 0.0792869141])
P3_TO_REC2020 = np.array([[0.7538330, 0.1985973, 0.0475697],
                          [0.0457438, 0.9417772, 0.0124789],
                          [-0.0012103, 0.0176017, 0.9836086]])


def pq_encode(nits):
    """ST 2084 inverse EOTF of absolute cd/m²."""
    y = np.clip(np.asarray(nits, dtype=np.float64) / 10000, 0, 1)
    m1, m2, c1, c2, c3 = 2610 / 16384, 2523 / 4096 * 128, 3424 / 4096, 2413 / 128, 2392 / 128
    p = y ** m1
    return ((c1 + c2 * p) / (1 + c3 * p)) ** m2


def ictcp(p3, white_nits=203):
    """ICtCp (BT.2100 PQ) of linear Display P3 with graphics white at `white_nits`."""
    rec2020 = np.maximum(p3 @ P3_TO_REC2020.T, 0) * white_nits
    lms = rec2020 @ np.array([[1688, 2146, 262], [683, 2951, 462], [99, 309, 3688]]).T / 4096
    lms = pq_encode(lms)
    return lms @ np.array([[2048, 2048, 0], [6610, -13613, 7003], [17933, -17390, -543]]).T / 4096


def delta_e_itp(a, b):
    """ΔE_ITP (BT.2124) of two linear Display P3 images."""
    i1, i2 = ictcp(a), ictcp(b)
    d = i1 - i2
    return 720 * np.sqrt(d[..., 0] ** 2 + (0.5 * d[..., 1]) ** 2 + d[..., 2] ** 2)


def metrics(reference, candidate):
    """The stage 1 HDR tolerances' metrics of two reduced linear Display P3 images."""
    ref_y = np.maximum(reference @ P3_LUMA, 1e-6)
    our_y = np.maximum(candidate @ P3_LUMA, 1e-6)
    stops = np.abs(np.log2(our_y / ref_y))
    above, below = ref_y > 1, ref_y < 0.5
    de = delta_e_itp(reference, candidate)
    q = lambda values, p: float(np.quantile(values, p)) if values.size else None
    return dict(
        above_white_share=float(above.mean()),
        above_median_stops=q(stops[above], 0.5), above_p95_stops=q(stops[above], 0.95),
        below_median_stops=q(stops[below], 0.5), below_p99_stops=q(stops[below], 0.99),
        peak_stops=float(np.log2(candidate.max() / reference.max())),
        reference_peak=float(reference.max()), candidate_peak=float(candidate.max()),
        delta_e_itp_mean=float(de.mean()), delta_e_itp_p95=q(de, 0.95))


def read_master(path):
    import tifffile
    pixels = tifffile.imread(path).astype(np.float64)
    return pixels @ PROPHOTO_TO_P3.T


def read_sdr(path):
    """A 16-bit ProPhoto TIFF (ROMM RGB encoding) as linear Display P3."""
    import tifffile
    e = tifffile.imread(path).astype(np.float64) / 65535
    return np.where(e < 1 / 32, e / 16, e ** 1.8) @ PROPHOTO_TO_P3.T


def relative(reference, candidate, reference_sdr, candidate_sdr):
    """How the HDR rendering departs from the SDR one, ours against Camera Raw's: the
    per-pixel log2 of (HDR / SDR luminance), compared where Camera Raw's HDR is above
    white. This separates the HDR extension from differences both renders share."""
    y = lambda a: np.maximum(a @ P3_LUMA, 1e-6)
    ours = np.log2(y(candidate) / y(candidate_sdr))
    theirs = np.log2(y(reference) / y(reference_sdr))
    above = y(reference) > 1
    error = np.abs(ours - theirs)[above]
    return dict(relative_above_median_stops=float(np.median(error)) if error.size else None,
                relative_above_p95_stops=float(np.quantile(error, .95)) if error.size else None)


def without_profile(xmp):
    """The requested sidecar without its profile, which is passed as the job's DCP, and
    without the look tables Camera Raw reads from it: RAWmakase takes a look from its
    profile library (`RAWMAKASE_DATA_DIR`)."""
    xmp = re.sub(r'\s*crs:Table_[0-9A-F]+="[^"]*"', '', xmp)
    return re.sub(r'\s*crs:CameraProfile="[^"]*"', '', xmp)


def render(binary, job, xmp, output, hdr):
    command = [str(binary), 'render', job['source'], '--xmp', str(xmp)]
    if job['profile'] != 'embedded':
        command += ['--profile', job['profile']]
    command += ['--hdr', str(output)] if hdr else ['--color-space', 'prophoto', str(output)]
    return subprocess.run(command, capture_output=True, text=True)


def validate(family, binary, out, split, tag):
    out.mkdir(parents=True, exist_ok=True)
    results = []
    manifest = json.loads((family / 'manifest.json').read_text())
    # The SDR capture of each source, profile and look, for the relative metric.
    def pairing(job):
        look = job.get('look')
        return (job['source'], job['profile'],
                (look['file'], float(look.get('amount', 1))) if look else None)
    sdr = {pairing(r['job']): i for i, r in enumerate(manifest)
           if r['output_transfer'] == 'sRGB' and set(r['job']['settings']) <= {'CameraProfile', 'HDREditMode'}}
    for index, record in enumerate(manifest):
        job = record['job']
        if (split and job['split'] != split) or (tag and job.get('tag') != tag):
            continue
        if record['output_transfer'] != 'ST2084':
            continue
        folder = family / f'{index:03d}'
        xmp = out / f'{index:03d}.xmp'
        xmp.write_text(without_profile((folder / 'requested.xmp').read_text()))
        master = out / f'{index:03d}.tif'
        if not master.exists():
            done = render(binary, job, xmp, master, True)
            if done.returncode:
                results.append(dict(index=index, tag=job.get('tag'), error=done.stderr.strip()[-300:]))
                print(index, job.get('tag'), 'error', done.stderr.strip()[-200:], flush=True)
                continue
        reference = np.load(folder / 'linear-p3.npy')
        factor = record['resampling']['factor']
        candidate = hdr_reference.reduce_linear(read_master(master), factor)
        h, w = min(reference.shape[0], candidate.shape[0]), min(reference.shape[1], candidate.shape[1])
        row = dict(index=index, source=Path(job['source']).name, tag=job.get('tag'), split=job['split'],
                   shape=[list(reference.shape[:2]), list(candidate.shape[:2])],
                   **metrics(reference[:h, :w], candidate[:h, :w]))
        pair = sdr.get(pairing(job))
        if pair is not None:
            # Our SDR render of the HDR job's settings with HDR off: its SDR rendition.
            plain = out / f'{index:03d}-sdr.tif'
            if not plain.exists():
                off = out / f'{index:03d}-sdr.xmp'
                off.write_text(xmp.read_text().replace('crs:HDREditMode="1"', 'crs:HDREditMode="0"'))
                render(binary, job, off, plain, False).check_returncode()
            ours_sdr = hdr_reference.reduce_linear(read_sdr(plain), factor)[:h, :w]
            theirs_sdr = np.load(family / f'{pair:03d}' / 'linear-p3.npy')[:h, :w]
            row.update(relative(reference[:h, :w], candidate[:h, :w], theirs_sdr, ours_sdr))
            y = lambda a: np.maximum(a @ P3_LUMA, 1e-6)
            if 'Exposure' not in (job.get('tag') or '') and 'Whites' not in (job.get('tag') or '') \
                    and 'Blacks' not in (job.get('tag') or ''):
                sdr_stops = np.abs(np.log2(y(ours_sdr) / y(theirs_sdr)))
                row['sdr_median_stops'] = float(np.median(sdr_stops))
            # Our HDR and SDR renders below 0.5, as Camera Raw's agree (stage 1).
            below = y(ours_sdr) < 0.5
            agree = np.abs(np.log2(y(candidate[:h, :w]) / y(ours_sdr)))[below]
            row['own_below_median_stops'] = float(np.median(agree))
            row['own_below_p99_stops'] = float(np.quantile(agree, .99))
        results.append(row)
        print(json.dumps(row), flush=True)
    (out / 'results.json').write_text(json.dumps(results, indent=1) + '\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    v = sub.add_parser('validate')
    v.add_argument('family', type=Path)
    v.add_argument('binary', type=Path)
    v.add_argument('out', type=Path)
    v.add_argument('--split')
    v.add_argument('--tag')
    args = parser.parse_args()
    validate(args.family.resolve(), args.binary.resolve(), args.out.resolve(), args.split, args.tag)


if __name__ == '__main__':
    main()
