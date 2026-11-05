"""Separate, immutable Camera Raw HDR reference capture and linear diagnostics.

Run `capture CONFIG OUT`, where CONFIG is a local JSON array of jobs with source,
licence, split, profile (a DCP path, or `embedded` for a DNG's own profile), settings,
and optional curves. OUT must not
exist. Originals are hashed and copied before Adobe opens anything. Full-size
TIFFs, complete returned XMP, and linear Display P3 arrays are retained. No SDR
corpus files are read or written. `compare REF.npy CANDIDATE.npy` is diagnostic,
not an Adobe parity acceptance gate.
"""
import argparse
import hashlib
import json
import shutil
import subprocess
from pathlib import Path

import numpy as np

import camera_raw


def sha256(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def pq_to_nits(encoded):
    """ST 2084 EOTF; input is normalized PQ, output is absolute cd/m²."""
    encoded = np.asarray(encoded, dtype=np.float64)
    if not np.isfinite(encoded).all() or (encoded < 0).any() or (encoded > 1).any():
        raise ValueError('PQ samples must be finite and in [0, 1]')
    p = encoded ** (32 / 2523)
    return 10000 * (np.maximum(p - 3424 / 4096, 0) /
                    (2413 / 128 - 2392 / 128 * p)) ** (16384 / 2610)


def reduce_linear(pixels, factor):
    """Mean of non-overlapping linear blocks, retaining partial edge blocks."""
    pixels = np.asarray(pixels, dtype=np.float64)
    if (not isinstance(factor, int) or factor < 1 or pixels.ndim != 3
            or pixels.shape[2] != 3 or min(pixels.shape) == 0
            or not np.isfinite(pixels).all()):
        raise ValueError('Expected finite nonempty RGB and a positive integer factor')
    ys = np.arange(0, pixels.shape[0], factor)
    xs = np.arange(0, pixels.shape[1], factor)
    sums = np.add.reduceat(np.add.reduceat(pixels, ys, axis=0), xs, axis=1)
    counts = (np.minimum(factor, pixels.shape[0] - ys)[:, None] *
              np.minimum(factor, pixels.shape[1] - xs)[None, :])
    return sums / counts[..., None]


def store_compressed(path, pixels, icc):
    """Rewrites Photoshop's uncompressed TIFF losslessly (Deflate with a horizontal
    predictor) with the same pixels and ICC profile, and checks it reads back exactly.
    Returns the SHA-256 of the pixel data."""
    import tifffile
    pixels = np.ascontiguousarray(pixels)
    temporary = path.with_suffix('.tmp.tif')
    tifffile.imwrite(temporary, pixels, photometric='rgb', compression='zlib',
                     predictor=True, iccprofile=icc)
    with tifffile.TiffFile(temporary) as tiff:
        if not np.array_equal(tiff.asarray(), pixels) or tiff.pages[0].tags[34675].value != icc:
            raise RuntimeError('Compressed reference does not read back exactly')
    temporary.replace(path)
    return hashlib.sha256(pixels.tobytes()).hexdigest()


def compare(reference, candidate):
    """Linear Display P3 RGB normalized to graphics white; no alignment or fit."""
    reference, candidate = (np.asarray(x, dtype=np.float64) for x in (reference, candidate))
    if (reference.shape != candidate.shape or reference.ndim != 3
            or reference.shape[-1] != 3 or reference.size == 0
            or not np.isfinite(reference).all() or not np.isfinite(candidate).all()):
        raise ValueError('Expected matching nonempty finite RGB arrays')
    error = np.abs(reference - candidate)
    highlight = np.max(reference, axis=-1) > 1
    # Display P3's D65 luminance coefficients. The floor limits the EV diagnostic
    # to one millionth of graphics white; linear errors still include negatives.
    luma = np.array([0.2289745641, 0.6917385218, 0.0792869141])
    stops = np.abs(np.log2(np.maximum(reference @ luma, 1e-6) /
                           np.maximum(candidate @ luma, 1e-6)))
    return dict(mean_abs_reference_white=float(error.mean()),
                p99_abs_reference_white=float(np.quantile(error, .99)),
                max_abs_reference_white=float(error.max()),
                highlight_max_abs_reference_white=float(error[highlight].max()) if highlight.any() else None,
                max_luminance_error_stops=float(stops.max()))


TEMPLATE = r'''#target photoshop
var oldDialogs = app.displayDialogs;
app.displayDialogs = DialogModes.NO;
var doc = null;
try {
    doc = app.open(new File(%(source)s));
    var w = doc.width.as('px'), h = doc.height.as('px');
    var profile = doc.colorProfileName;
    if (doc.bitsPerChannel != BitsPerChannelType.SIXTEEN)
        throw new Error('Expected 16-bit workflow, got ' + doc.bitsPerChannel);
    if (profile != 'P3 PQ' && profile != 'P3D65 PQ Display Full 12-16-0-1' && profile != 'Display P3')
        throw new Error('Unqualified output profile: ' + profile);
    var options = new TiffSaveOptions();
    options.imageCompression = TIFFEncoding.NONE;
    options.embedColorProfile = true;
    doc.saveAs(new File(%(output)s), options, true);
    [w, h, profile].join('|');
} finally {
    if (doc) doc.close(SaveOptions.DONOTSAVECHANGES);
    app.displayDialogs = oldDialogs;
}
'''


def capture(config, out, factor):
    import tifffile

    jobs = json.loads(config.read_text())
    if not jobs or not isinstance(jobs, list):
        raise ValueError('Expected a nonempty job array')
    for job in jobs:
        for key in ('source', 'licence', 'split', 'profile', 'settings'):
            if not job.get(key):
                raise ValueError(f'Missing {key}')
        if job['split'] not in ('training', 'validation', 'holdout', 'diagnostic'):
            raise ValueError('Unknown split')
        if not Path(job['source']).is_file() or not (
                job['profile'] == 'embedded' or Path(job['profile']).is_file()):
            raise ValueError('Source and DCP must exist')
    if factor < 1:
        raise ValueError('Reduction factor must be positive')
    if not camera_raw.idle():
        raise RuntimeError('Photoshop must be idle with no open documents')
    out.mkdir(parents=True, exist_ok=False)
    records = []
    for index, job in enumerate(jobs):
        folder = out / f'{index:03d}'
        folder.mkdir()
        source = Path(job['source']).resolve()
        source_hash = sha256(source)
        profile_hash = None if job['profile'] == 'embedded' else sha256(job['profile'])
        copy = folder / ('input' + source.suffix)
        shutil.copyfile(source, copy)
        if sha256(copy) != source_hash:
            raise RuntimeError('Source changed while copying')
        xmp = camera_raw.xmp(job['settings'], job.get('curves'))
        copy.with_suffix('.xmp').write_text(xmp)
        (folder / 'requested.xmp').write_text(xmp)
        output = folder / 'reference.tif'
        script = folder / 'render.jsx'
        script.write_text(TEMPLATE % dict(source=json.dumps(str(copy)), output=json.dumps(str(output))))
        completed = subprocess.run(['osascript', '-e', 'with timeout of 600 seconds\n'
                        f'tell application "{camera_raw.PHOTOSHOP}"\nactivate\n'
                        f'do javascript file (POSIX file {json.dumps(str(script))})\n'
                        'end tell\nend timeout'], capture_output=True, text=True, timeout=660)
        (folder / 'photoshop.log').write_text(completed.stdout + completed.stderr)
        completed.check_returncode()
        if sha256(source) != source_hash or (
                profile_hash is not None and sha256(job['profile']) != profile_hash):
            raise RuntimeError('Original source or profile changed during capture')
        metadata = json.loads(subprocess.check_output(['exiftool', '-j', '-G1', '-s', str(output)]))[0]
        (folder / 'returned.xmp').write_bytes(subprocess.check_output(['exiftool', '-b', '-XMP', str(output)]))
        with tifffile.TiffFile(output) as tiff:
            pixels = tiff.asarray()
            icc = tiff.pages[0].tags[34675].value
        if pixels.dtype != np.uint16 or pixels.ndim != 3 or pixels.shape[2] != 3:
            raise ValueError('Expected full-resolution RGB16 TIFF')
        profile = metadata.get('ICC_Profile:ProfileDescription')
        encoded = pixels.astype(np.float64) / 65535
        if profile == 'P3 PQ':
            linear = pq_to_nits(encoded) / 203
            transfer = 'ST2084'
        elif profile == 'Display P3':
            linear = np.where(encoded <= .04045, encoded / 12.92,
                              ((encoded + .055) / 1.055) ** 2.4)
            transfer = 'sRGB'
        else:
            raise ValueError(f'Unqualified profile {profile}')
        expected_hdr = str(job['settings'].get('HDREditMode', '0')) == '1'
        if expected_hdr != (transfer == 'ST2084'):
            raise ValueError('Requested HDR intent and returned transfer disagree')
        output_hash = sha256(output)
        pixels_hash = store_compressed(output, pixels, icc)
        reduced = reduce_linear(linear, factor)
        np.save(folder / 'linear-p3.npy', reduced)
        record = dict(job=job, source_sha256=source_hash, profile_sha256=profile_hash,
                      camera_raw=camera_raw.version(), requested_xmp_sha256=sha256(folder / 'requested.xmp'),
                      returned_xmp_sha256=sha256(folder / 'returned.xmp'),
                      output_sha256=output_hash, stored='Deflate TIFF, identical pixels and ICC',
                      pixels_sha256=pixels_hash, output_icc_sha256=hashlib.sha256(icc).hexdigest(),
                      linear_sha256=sha256(folder / 'linear-p3.npy'),
                      output_dimensions=[pixels.shape[1], pixels.shape[0]],
                      reduced_dimensions=[reduced.shape[1], reduced.shape[0]],
                      resampling=dict(method='nonoverlapping area mean; partial edge blocks retained',
                                      factor=factor, domain='linear Display P3 D65'),
                      output_transfer=transfer, reference_white_nits=203,
                      reference_white_status='normalization convention; Adobe UI-white match not yet qualified',
                      max_reference_white=float(linear.max()), metadata=metadata)
        (folder / 'manifest.json').write_text(json.dumps(record, indent=2) + '\n')
        records.append(record)
        # Only the working source is disposable; references and returned settings stay.
        copy.unlink()
        (out / 'manifest.json').write_text(json.dumps(records, indent=2) + '\n')
        print(f'{index:03d}: {profile}, peak {linear.max():.6f} reference whites', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    capture_parser = sub.add_parser('capture')
    capture_parser.add_argument('config', type=Path)
    capture_parser.add_argument('out', type=Path)
    capture_parser.add_argument('--factor', type=int, default=8)
    compare_parser = sub.add_parser('compare')
    compare_parser.add_argument('reference', type=Path)
    compare_parser.add_argument('candidate', type=Path)
    args = parser.parse_args()
    if args.command == 'capture':
        capture(args.config, args.out.resolve(), args.factor)
    else:
        print(json.dumps(compare(np.load(args.reference), np.load(args.candidate)), indent=2))


if __name__ == '__main__':
    main()
