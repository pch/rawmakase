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
        run("avifenc", "--lossless", "--cicp", "12/13/0", src, out / "p3.avif")
        run("avifenc", "-d", "10", "--cicp", "9/16/9", src, out / "pq.avif")
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
        run("avifenc", "--lossless", "-d", "10", src, out / "ten-bit.avif")

        # JPEG XL
        jpeg = tmp / "meta.jpg"
        corners().save(jpeg, quality=95, exif=exif(), xmp=XMP)
        run("cjxl", jpeg, out / "metadata.jxl", "--compress_boxes=0")
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
