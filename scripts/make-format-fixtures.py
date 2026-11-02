#!/usr/bin/env python3
"""Synthetic WebP, AVIF and JPEG XL fixtures for the decoder and metadata tests.

Usage: python3 scripts/make-format-fixtures.py tests/data/formats
Needs Pillow (with WebP), avifenc (libavif) and cjxl (libjxl) on PATH. Every
image is drawn here; nothing comes from a camera or a photo.
"""
import io, shutil, subprocess, sys, tempfile
from pathlib import Path
from PIL import Image

XMP = (b'<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF '
       b'xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description '
       b'rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title><rdf:Alt>'
       b'<rdf:li xml:lang="x-default">Fixture title</rdf:li></rdf:Alt></dc:title>'
       b'</rdf:Description></rdf:RDF></x:xmpmeta>')


def corners(w=16, h=12):
    im = Image.new("RGB", (w, h), (128, 128, 128))
    for (x0, y0), colour in [((0, 0), (255, 0, 0)), ((w - 4, 0), (0, 255, 0)),
                             ((0, h - 4), (0, 0, 255)), ((w - 4, h - 4), (255, 255, 255))]:
        for x in range(x0, x0 + 4):
            for y in range(y0, y0 + 4):
                im.putpixel((x, y), colour)
    return im


def split_extent(src: Path, dst: Path, kind: bytes):
    """`src` with the data of its `kind` item in two iloc extents, as some
    writers store it. Handles avifenc's iloc: version 0, 4-byte offsets and
    lengths, no base offsets, item data after the meta box."""
    d = src.read_bytes()
    be = lambda b: int.from_bytes(b, "big")
    top, at = {}, 0
    while at < len(d):
        top[d[at + 4:at + 8]] = (at, be(d[at:at + 4]))
        at += be(d[at:at + 4])
    meta_at, meta_len = top[b"meta"]
    children, at = {}, meta_at + 12
    while at < meta_at + meta_len:
        children[d[at + 4:at + 8]] = (at, be(d[at:at + 4]))
        at += be(d[at:at + 4])
    iinf = d[slice(children[b"iinf"][0], sum(children[b"iinf"]))]
    entry = iinf.index(kind) - 4  # infe v2: ID, protection index, then type
    item = be(iinf[entry:entry + 2])
    iloc_at, iloc_len = children[b"iloc"]
    iloc = d[iloc_at + 8:iloc_at + iloc_len]
    assert iloc[0] == 0 and iloc[4:6] == b"\x44\x00", "avifenc's iloc layout"
    body, at = bytearray(iloc[:8]), 8
    for _ in range(be(iloc[6:8])):
        id, count = be(iloc[at:at + 2]), be(iloc[at + 4:at + 6])
        extents = [(be(iloc[e:e + 4]), be(iloc[e + 4:e + 8]))
                   for e in range(at + 6, at + 6 + 8 * count, 8)]
        at += 6 + 8 * count
        if id == item:
            (offset, length), = extents
            extents = [(offset, length // 2), (offset + length // 2, length - length // 2)]
        body += id.to_bytes(2, "big") + b"\0\0" + len(extents).to_bytes(2, "big")
        for offset, length in extents:
            body += (offset + 8).to_bytes(4, "big") + length.to_bytes(4, "big")
    assert len(body) == len(iloc) + 8
    iloc = (len(body) + 8).to_bytes(4, "big") + b"iloc" + body
    meta = d[meta_at:iloc_at] + iloc + d[iloc_at + iloc_len:meta_at + meta_len]
    meta = (meta_len + 8).to_bytes(4, "big") + meta[4:]
    dst.write_bytes(d[:meta_at] + meta + d[meta_at + meta_len:])


def exif(orientation=None):
    e = Image.Exif()
    e[0x010F] = "Fixture"
    e[0x0110] = "Synthetic 1"
    if orientation:
        e[0x0112] = orientation
    e.get_ifd(0x8769)[0x9003] = "2024:05:04 10:21:33"
    return e


def run(*args):
    r = subprocess.run([str(a) for a in args], capture_output=True, text=True)
    if r.returncode:
        sys.exit(f"{args[0]} failed: {r.stdout}{r.stderr}")


def main(out: Path):
    out.mkdir(parents=True, exist_ok=True)
    tmp = Path(tempfile.mkdtemp())
    try:
        src = out / "corners.png"
        corners().save(src)
        tiff_exif = tmp / "exif.bin"
        tiff_exif.write_bytes(exif().tobytes())  # TIFF stream, as avifenc --exif expects
        xmp = tmp / "xmp.xml"
        xmp.write_bytes(XMP)

        # AVIF
        run("avifenc", "--lossless", "--irot", "1", src, out / "rotated.avif")
        run("avifenc", "--lossless", "--irot", "1", "--imir", "0", src, out / "rotated-mirrored.avif")
        run("avifenc", "--lossless", "--crop", "2,2,8,8", src, out / "cropped.avif")
        run("avifenc", "--lossless", "--cicp", "12/13/0", src, out / "p3.avif")
        run("avifenc", "-d", "10", "--cicp", "9/16/9", src, out / "pq.avif")
        run("avifenc", "-d", "10", "--cicp", "9/18/9", src, out / "hlg.avif")
        big = tmp / "big.png"
        corners(128, 128).save(big)  # MIAF: grid cells are at least 64x64
        run("avifenc", "--lossless", "--grid", "2x2", big, out / "tiled.avif")
        frame2 = tmp / "frame2.png"
        corners().transpose(Image.FLIP_LEFT_RIGHT).save(frame2)
        run("avifenc", "--lossless", src, frame2, out / "animated.avif")
        odd = tmp / "odd.png"
        corners(15, 11).save(odd)
        run("avifenc", "-y", "420", "-q", "100", odd, out / "odd-420.avif")
        run("avifenc", "--lossless", "--exif", tiff_exif, "--xmp", xmp, src, out / "metadata.avif")
        split_extent(out / "metadata.avif", out / "split-exif.avif", b"Exif")
        run("avifenc", "--lossless", "-d", "10", src, out / "ten-bit.avif")
        # 4:2:0 above 8 bits, one flat colour: yuv's NEON code tinted every
        # other pixel of it (avif::deep420_to_rgb).
        flat = tmp / "flat.png"
        Image.new("RGB", (16, 12), (200, 80, 40)).save(flat)
        run("avifenc", "-d", "10", "-y", "420", "-q", "100", flat, out / "ten-bit-420.avif")

        # JPEG XL
        jpeg = tmp / "meta.jpg"
        corners().save(jpeg, quality=95, exif=exif(), xmp=XMP)
        run("cjxl", jpeg, out / "metadata.jxl", "--compress_boxes=0")
        # cjxl's default: the Exif and XMP boxes brotli-compressed ("brob").
        run("cjxl", jpeg, out / "metadata-brob.jxl", "--compress_boxes=1")
        rotated_jpeg = tmp / "rotated.jpg"
        corners().save(rotated_jpeg, quality=95, exif=exif(orientation=6))
        run("cjxl", rotated_jpeg, out / "exif-rotated.jxl", "--compress_boxes=0")
        apng = tmp / "anim.png"
        corners().save(apng, save_all=True, append_images=[Image.open(frame2)], duration=100)
        run("cjxl", apng, out / "animated.jxl", "-d", "0")
        sixteen = tmp / "sixteen.png"
        Image.open(src).convert("RGB").save(sixteen)  # 8-bit source; cjxl stores 16 with -x
        run("cjxl", sixteen, out / "sixteen-bit.jxl", "-d", "0", "--override_bitdepth=16")
        run("cjxl", src, out / "pq.jxl", "-d", "1", "-x", "color_space=RGB_D65_202_Rel_PeQ")
        run("cjxl", src, out / "hlg.jxl", "-d", "1", "-x", "color_space=RGB_D65_202_Rel_HLG")

        # WebP
        corners().save(out / "metadata.webp", lossless=True, exif=exif().tobytes(), xmp=XMP)
        corners().save(out / "rotated.webp", lossless=True, exif=exif(orientation=6).tobytes())
        corners().save(out / "animated.webp", lossless=True, save_all=True,
                       append_images=[Image.open(frame2)], duration=100)
    finally:
        shutil.rmtree(tmp)
    for f in sorted(out.iterdir()):
        assert f.stat().st_size <= 4096, f"{f.name} is {f.stat().st_size} bytes"
        print(f"{f.name}\t{f.stat().st_size}")
    for tool in ("avifenc", "cjxl"):
        print(subprocess.run([tool, "--version"], capture_output=True, text=True).stdout.splitlines()[0])


if __name__ == "__main__":
    main(Path(sys.argv[1]))
