#!/usr/bin/env python3
"""Synthetic HDR JPEG XL fixtures in primaries other than BT.709 and BT.2020.

Usage: python3 scripts/make-hdr-jxl-fixtures.py tests/data/formats
Needs libjxl's cjxl on PATH; the standard library otherwise. Every image is drawn
here; nothing comes from a camera or a photo.

Each file is lossless, 4x1, of 16-bit signal (`PIXELS`, which the decoder test
repeats in a PNG with the same cICP): graphics white in PQ or HLG, a brighter grey and
two saturated colours. cjxl turns the PNG's cICP into the codestream's own colour
encoding:
- `hdr-pq-p3.jxl`: PQ in Display P3 (CICP 12), which JPEG XL writes as P3 primaries with
  a D65 white;
- `hdr-pq-dci.jxl`: PQ in DCI-P3 (CICP 11), P3 primaries with DCI's white;
- `hdr-hlg-ebu.jxl`: HLG in EBU Tech. 3213 (CICP 22), which JPEG XL has no name for, so
  custom chromaticities;
- `hdr-pq-709-dci.jxl` and `hdr-pq-2020-e.jxl`: PQ in BT.709's primaries with DCI's
  white, and in BT.2020's with illuminant E, which no CICP code names (cjxl's
  `color_space` strings `RGB_DCI_SRG_Rel_PeQ` and `RGB_EER_202_Rel_PeQ`, from a PNG of
  the same signal without a cICP);
- `linear-ebu.jxl` and `linear-srgb.jxl`: linear light in those custom chromaticities
  and in sRGB's primaries, which have always opened as SDR files.
"""
import shutil, struct, subprocess, sys, tempfile, zlib
from pathlib import Path

PIXELS = [(38050, 38050, 38050), (52000, 52000, 52000), (45000, 20000, 9000), (12000, 40000, 30000)]
FILES = [("hdr-pq-p3.jxl", 12, 16), ("hdr-pq-dci.jxl", 11, 16), ("hdr-hlg-ebu.jxl", 22, 18),
         ("linear-ebu.jxl", 22, 8), ("linear-srgb.jxl", 1, 8)]


def png16(path: Path, cicp):
    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))
    raw = b"\0" + b"".join(struct.pack(">HHH", *p) for p in PIXELS)
    path.write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", len(PIXELS), 1, 16, 2, 0, 0, 0))
                     + (chunk(b"cICP", cicp) if cicp else b"") + chunk(b"IDAT", zlib.compress(raw, 9))
                     + chunk(b"IEND", b""))


WHITES = [("hdr-pq-709-dci.jxl", "RGB_DCI_SRG_Rel_PeQ"), ("hdr-pq-2020-e.jxl", "RGB_EER_202_Rel_PeQ")]


def main(out: Path):
    enc = shutil.which("cjxl") or sys.exit("cjxl is not on PATH")
    with tempfile.TemporaryDirectory() as tmp:
        src = Path(tmp) / "signal.png"
        for name, primaries, transfer in FILES:
            png16(src, bytes([primaries, transfer, 0, 1]))
            subprocess.run([enc, "-d", "0", "-e", "7", src, out / name], check=True, capture_output=True)
        for name, space in WHITES:
            png16(src, None)
            subprocess.run([enc, "-d", "0", "-e", "7", "-x", f"color_space={space}", src, out / name],
                           check=True, capture_output=True)


if __name__ == "__main__":
    main(Path(sys.argv[1]))
