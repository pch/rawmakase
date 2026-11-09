#!/usr/bin/env python3
"""Synthetic HDR HEIC fixtures (PQ and HLG, CICP in the `colr` box).

Usage: python3 scripts/make-hdr-heic-fixtures.py tests/data/formats
Needs heif-enc (libheif with x265) on PATH; the standard library otherwise. Every
image is drawn here; nothing comes from a camera or a photo.

Each file is 64x32: eight flat 16x16 patches of known 10-bit signal (`PATCHES`,
which the decoder test repeats), BT.2020 primaries and matrix, full range, 4:2:0 as
phones write. The patches cover deep shadow, graphics white, highlights and
saturated colours; flat patches keep the codec's error at their centres to a code
value or two. The files are `hdr-pq.heic`, `hdr-hlg.heic`, and `hdr-pq-p3.heic`,
the PQ signal in Display P3 primaries. `hdr-pq-icc.heic` is the PQ signal with no
`nclx`, its transfer named only by the `cicp` tag of an ICC profile written here (BT.2020
colorants adapted to D50, linear curves), which Develop refuses.
"""
import shutil, struct, subprocess, sys, tempfile, zlib
from pathlib import Path

W, H, SIDE = 64, 32, 16

# 10-bit code values, row by row, four patches a row. 594 is graphics white in PQ
# (203 cd/m², signal 0.5806); in HLG, 0.75 (767) is.
PATCHES = [
    (64, 64, 64), (300, 300, 300), (594, 594, 594), (800, 800, 800),
    (700, 200, 150), (200, 650, 180), (160, 220, 720), (900, 880, 860),
]


def pq_icc() -> bytes:
    """A minimal ICC v4 RGB display profile: BT.2020 colorants adapted to D50, D50
    white, linear curves, and a `cicp` tag of BT.2020 primaries, PQ, full range."""
    s15 = lambda v: struct.pack(">i", round(v * 65536))
    xyz = lambda x, y, z: b"XYZ \0\0\0\0" + s15(x) + s15(y) + s15(z)
    tags = [
        (b"wtpt", xyz(0.9642, 1.0, 0.8249)),
        (b"rXYZ", xyz(0.6734, 0.2790, -0.0019)),
        (b"gXYZ", xyz(0.1656, 0.6753, 0.0300)),
        (b"bXYZ", xyz(0.1251, 0.0457, 0.7973)),
        (b"rTRC", b"para\0\0\0\0\0\0\0\0" + s15(1.0)),
        (b"gTRC", b"para\0\0\0\0\0\0\0\0" + s15(1.0)),
        (b"bTRC", b"para\0\0\0\0\0\0\0\0" + s15(1.0)),
        (b"cicp", b"cicp\0\0\0\0" + bytes([9, 16, 0, 1])),
    ]
    at, table, data = 128 + 4 + 12 * len(tags), b"", b""
    for sig, body in tags:
        body += b"\0" * (-len(body) % 4)
        table += sig + struct.pack(">II", at + len(data), len(body))
        data += body
    size = 128 + 4 + len(table) + len(data)
    header = (struct.pack(">I", size) + b"\0\0\0\0" + bytes([4, 0x40, 0, 0]) + b"mntrRGB XYZ "
              + b"\0" * 12 + b"acsp" + b"\0" * 24 + s15(0.9642) + s15(1.0) + s15(0.8249))
    header += b"\0" * (128 - len(header))
    return header + struct.pack(">I", len(tags)) + table + data


def png16(path: Path, rows, icc=None):
    """A 16-bit RGB PNG of `rows` (lists of (r, g, b) 16-bit samples), with `icc` as
    its iCCP chunk when given."""
    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))
    raw = b"".join(b"\0" + b"".join(struct.pack(">HHH", *p) for p in row) for row in rows)
    iccp = chunk(b"iCCP", b"PQ\0\0" + zlib.compress(icc)) if icc else b""
    path.write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", W, H, 16, 2, 0, 0, 0))
                     + iccp + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def main(out: Path):
    enc = shutil.which("heif-enc") or sys.exit("heif-enc is not on PATH")
    rows = []
    for y in range(H):
        row = []
        for x in range(W):
            code = PATCHES[(y // SIDE) * (W // SIDE) + x // SIDE]
            # heif-enc keeps the top 10 bits of a 16-bit PNG's samples.
            row.append(tuple(c << 6 for c in code))
        rows.append(row)
    with tempfile.TemporaryDirectory() as tmp:
        src = Path(tmp) / "patches.png"
        png16(src, rows)
        for name, primaries, transfer in [("hdr-pq.heic", 9, 16), ("hdr-hlg.heic", 9, 18), ("hdr-pq-p3.heic", 12, 16)]:
            subprocess.run([enc, "-q", "100", "-b", "10", "-p", "chroma=420", "-p", "preset=slow",
                            "--matrix_coefficients", "9", "--colour_primaries", str(primaries),
                            "--transfer_characteristic", str(transfer), "--full_range_flag", "1",
                            "--no-alpha", "-o", out / name, src],
                           check=True, capture_output=True)
        profiled = Path(tmp) / "profiled.png"
        png16(profiled, rows, pq_icc())
        subprocess.run([enc, "-q", "100", "-b", "10", "-p", "chroma=420", "-p", "preset=slow",
                        "--no-alpha", "-o", out / "hdr-pq-icc.heic", profiled],
                       check=True, capture_output=True)


if __name__ == "__main__":
    main(Path(sys.argv[1]))
