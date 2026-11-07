#!/usr/bin/env python3
"""Synthetic Ultra HDR (ISO 21496-1 gain-map) JPEG fixtures, written by libultrahdr.

Usage: python3 scripts/make-gain-map-fixtures.py tests/data/formats
Needs ultrahdr_app (libultrahdr) on PATH; the standard library otherwise. Every
image is drawn here; nothing comes from a camera or a photo.

Both files are 64x48 with a Display P3 base and a three-channel map in the base's
colour space: `ultrahdr.jpg` has a map the base's size, `ultrahdr-quarter-map.jpg`
one a quarter of it (16x12). The script then decodes `ultrahdr.jpg` with libultrahdr
at its full headroom as linear half floats and prints the pixels the decoder test
checks, which go in that test (`raster::tests`).
"""
import shutil, struct, subprocess, sys, tempfile
from pathlib import Path

W, H = 64, 48


def half(v):
    return struct.pack("<e", v)


def srgb(v):
    v = min(max(v, 0.0), 1.0)
    return 12.92 * v if v <= 0.0031308 else 1.055 * v ** (1 / 2.4) - 0.055


def renditions():
    """The HDR intent (linear, 1 = SDR white, to 8) and an SDR intent that
    compresses each channel its own way, so the map's channels differ."""
    hdr, sdr = bytearray(), bytearray()
    for y in range(H):
        for x in range(W):
            t, s = x / (W - 1), y / (H - 1)
            lum = 0.02 + 7.98 * t ** 3
            rgb = (lum * (0.6 + 0.4 * s), lum * (1.0 - 0.3 * s), lum * (0.5 + 0.5 * (1 - s)))
            hdr += b"".join(half(v) for v in rgb) + half(1.0)
            r, g, b = rgb
            tone = (r / (1 + r), min(g / (1 + 0.5 * g), 1.0), b ** 0.7 / (1 + b ** 0.7))
            sdr += bytes(round(srgb(v) * 255) for v in tone) + b"\xff"
    return bytes(hdr), bytes(sdr)


def main(out: Path):
    app = shutil.which("ultrahdr_app") or sys.exit("ultrahdr_app is not on PATH")
    usage = subprocess.run([app], capture_output=True, text=True)
    print(next(l for l in (usage.stdout + usage.stderr).splitlines() if "version" in l))
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        hdr, sdr = renditions()
        (tmp / "hdr.raw").write_bytes(hdr)
        (tmp / "sdr.raw").write_bytes(sdr)
        for name, scale in [("ultrahdr.jpg", 1), ("ultrahdr-quarter-map.jpg", 4)]:
            subprocess.run(
                [app, "-m", "0", "-p", tmp / "hdr.raw", "-a", "4", "-t", "0", "-C", "1",
                 "-y", tmp / "sdr.raw", "-b", "3", "-c", "1", "-w", str(W), "-h", str(H),
                 "-M", "1", "-s", str(scale), "-q", "95", "-Q", "95", "-z", out / name],
                check=True, capture_output=True)
        subprocess.run([app, "-m", "1", "-j", out / "ultrahdr.jpg", "-o", "0", "-O", "4",
                        "-z", tmp / "decoded.raw"], check=True, capture_output=True)
        decoded = (tmp / "decoded.raw").read_bytes()
    print("libultrahdr's linear Display P3 decode of ultrahdr.jpg, row 24:")
    for x in range(0, W, 9):
        at = (24 * W + x) * 8
        rgb = struct.unpack("<eee", decoded[at:at + 6])
        print(f"    ({x}, 24, [{rgb[0]:.5}, {rgb[1]:.5}, {rgb[2]:.5}]),")


if __name__ == "__main__":
    main(Path(sys.argv[1]))
