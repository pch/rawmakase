#!/usr/bin/env python3
"""Give a DNG a linear embedded profile, to read Camera Raw's scene tone stage on a photo.

Converts a RAW with Adobe DNG Converter (or takes a DNG), then rewrites IFD0 so its
embedded profile keeps the camera's matrices, HueSatMap and baseline exposure but has
a linear ProfileToneCurve and no LookTable, under a new ProfileName. Rendered by
Camera Raw with that profile and no other edits, a photo then shows its scene tone
stage directly at the comparison boundary of docs/scene-tone-stage.md, the same way
the synthetic probes do. The pixels are not touched: IFD0 is copied to the end of
the file with the changed entries and the header points to the copy.

  python3 scripts/corpus/linear_profile.py OUT_DIR RAW...

Writes OUT_DIR/<stem>.dng. RAWs are only read.
"""
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

DNG_CONVERTER = '/Applications/Adobe DNG Converter.app/Contents/MacOS/Adobe DNG Converter'
NAME = 'RAWmakase Linear'
PROFILE_NAME, TONE_CURVE = 50936, 50940
LOOK_TAGS = {50981, 50982, 51108}  # ProfileLookTableDims, Data, Encoding
SIZES = {1: 1, 2: 1, 3: 2, 4: 4, 5: 8, 6: 1, 7: 1, 8: 2, 9: 4, 10: 8, 11: 4, 12: 8, 13: 4}


def relink(data, name=NAME):
    """`data` (a little- or big-endian DNG) with IFD0's profile made linear."""
    order = {b'II': '<', b'MM': '>'}[data[:2]]
    ifd = struct.unpack(order + 'I', data[4:8])[0]
    count = struct.unpack(order + 'H', data[ifd:ifd + 2])[0]
    entries = []
    for i in range(count):
        raw = data[ifd + 2 + 12 * i: ifd + 14 + 12 * i]
        tag, kind, n = struct.unpack(order + 'HHI', raw[:8])
        entries.append((tag, kind, n, raw[8:]))
    following = data[ifd + 2 + 12 * count: ifd + 6 + 12 * count]
    out = bytearray(data)
    if len(out) % 2:
        out += b'\0'

    def store(payload):
        if len(payload) <= 4:
            return payload + b'\0' * (4 - len(payload))
        offset = len(out)
        out.extend(payload)
        if len(out) % 2:
            out.append(0)
        return struct.pack(order + 'I', offset)

    entries = [e for e in entries if e[0] not in LOOK_TAGS | {TONE_CURVE, PROFILE_NAME}]
    text = name.encode() + b'\0'
    entries.append((PROFILE_NAME, 2, len(text), store(text)))
    entries.append((TONE_CURVE, 11, 4, store(struct.pack(order + '4f', 0, 0, 1, 1))))
    entries.sort(key=lambda e: e[0])
    table = struct.pack(order + 'H', len(entries))
    for tag, kind, n, value in entries:
        table += struct.pack(order + 'HHI', tag, kind, n) + value
    table += following
    offset = len(out)
    out.extend(table)
    out[4:8] = struct.pack(order + 'I', offset)
    return bytes(out)


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    for raw in map(Path, sys.argv[2:]):
        target = out / (raw.stem + '.dng')
        if target.exists():
            continue
        if raw.suffix.lower() == '.dng':
            data = raw.read_bytes()
        else:
            with tempfile.TemporaryDirectory() as work:
                subprocess.run([DNG_CONVERTER, '-c', '-p0', '-d', work, str(raw.resolve())], check=True,
                               capture_output=True)
                converted = list(Path(work).glob('*.dng'))
                if len(converted) != 1:
                    print(f'{raw.name}: DNG Converter produced {len(converted)} files', file=sys.stderr)
                    continue
                data = converted[0].read_bytes()
        target.write_bytes(relink(data))
        print(target)


if __name__ == '__main__':
    main()
