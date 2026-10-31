#!/usr/bin/env python3
"""Synthetic HEIC fixtures for the decoder and metadata tests.

Usage: python3 scripts/make-heic-fixtures.py tests/data/formats
Needs Pillow and pillow-heif (libheif with x265). Every image is drawn here;
nothing comes from a camera or a photo. metadata.heic, ten-bit.heic, p3.heic,
pq.heic and identity.heic are libheif's own files; the grid and the turned
files are written here around HEVC x265 coded, so each has the boxes its test
needs, and without x265's SEI banner, which would make them several kilobytes.
All but identity.heic (lossless 4:4:4 RGB) and wide.heic are 4:2:0, as phones
write.
"""
import importlib.util, io, sys
from pathlib import Path
import pillow_heif
from PIL import Image, ImageCms

spec = importlib.util.spec_from_file_location(
    "formats", Path(__file__).with_name("make-format-fixtures.py"))
formats = importlib.util.module_from_spec(spec)
spec.loader.exec_module(formats)
corners, exif, XMP = formats.corners, formats.exif, formats.XMP

be = lambda b: int.from_bytes(b, "big")


def box(kind: bytes, body: bytes) -> bytes:
    return (len(body) + 8).to_bytes(4, "big") + kind + body


def full(kind: bytes, version: int, flags: int, body: bytes) -> bytes:
    return box(kind, bytes([version]) + flags.to_bytes(3, "big") + body)


def children(d: bytes, at: int, end: int):
    out = {}
    while at + 8 <= end:
        size = be(d[at:at + 4])
        out.setdefault(d[at + 4:at + 8], []).append(d[at + 8:at + size])
        at += size
    return out


def coded(im: Image.Image, **options):
    """`im` as libheif codes it alone: its `hvcC` and other properties, in
    association order, and its NAL units; without SEI, in either."""
    out = io.BytesIO()
    pillow_heif.from_pillow(im).save(out, **options)
    d = out.getvalue()
    top = children(d, 0, len(d))
    meta = top[b"meta"][0]
    m = children(meta, 4, len(meta))
    iprp = children(m[b"iprp"][0], 0, len(m[b"iprp"][0]))
    ipco = iprp[b"ipco"][0]
    props, at = [], 0
    while at < len(ipco):
        size = be(ipco[at:at + 4])
        props.append(ipco[at:at + size])
        at += size
    ipma = iprp[b"ipma"][0]
    assert be(ipma[4:8]) == 1, "one item"
    count = ipma[10]
    associated = [(props[(i & 0x7f) - 1], bool(i & 0x80)) for i in ipma[11:11 + count]]
    hvcc = next(p for p, _ in associated if p[4:8] == b"hvcC")
    length = (hvcc[8 + 21] & 3) + 1
    data, nals, at = top[b"mdat"][0], b"", 0
    while at < len(data):
        n = be(data[at:at + length])
        nal = data[at:at + length + n]
        if (nal[length] >> 1) & 0x3f not in (39, 40):
            nals += nal
        at += length + n
    return without_sei(hvcc), associated, nals


def without_sei(hvcc: bytes) -> bytes:
    body = hvcc[8:]
    out, at = bytearray(body[:22]), 23
    arrays = []
    for _ in range(body[22]):
        kind, count = body[at], be(body[at + 1:at + 3])
        at += 3
        start = at
        for _ in range(count):
            at += 2 + be(body[at:at + 2])
        if kind & 0x3f not in (39, 40):
            arrays.append(body[start - 3:at])
    out += bytes([len(arrays)]) + b"".join(arrays)
    return box(b"hvcC", bytes(out))


def heif(brands, primary, items, refs=()):
    """A HEIF file of `items`, each (id, type, properties as (box, essential),
    data, hidden, stored in idat), with `refs` as (type, from, [to]). Item data
    is in the mdat box, or the idat box when asked."""
    ftyp = box(b"ftyp", brands[0] + b"\0\0\0\0" + b"".join(brands))
    hdlr = full(b"hdlr", 0, 0, b"\0" * 4 + b"pict" + b"\0" * 13)
    pitm = full(b"pitm", 0, 0, primary.to_bytes(2, "big"))
    iinf = full(b"iinf", 0, 0, len(items).to_bytes(2, "big") + b"".join(
        full(b"infe", 2, int(hidden), id.to_bytes(2, "big") + b"\0\0" + kind + b"\0")
        for id, kind, _, _, hidden, _ in items))
    ipco, ipma = [], b""
    for id, _, props, _, _, _ in items:
        indexes = []
        for p, essential in props:
            if p not in ipco:
                ipco.append(p)
            indexes.append(ipco.index(p) + 1 | (0x80 if essential else 0))
        ipma += id.to_bytes(2, "big") + bytes([len(indexes)] + indexes)
    ipma = full(b"ipma", 0, 0, len(items).to_bytes(4, "big") + ipma)
    iprp = box(b"iprp", box(b"ipco", b"".join(ipco)) + ipma)
    iref = full(b"iref", 0, 0, b"".join(
        box(kind, src.to_bytes(2, "big") + len(to).to_bytes(2, "big")
            + b"".join(t.to_bytes(2, "big") for t in to))
        for kind, src, to in refs))
    idat = b"".join(data for *_, data, _, in_idat in items if in_idat)

    def meta(mdat_at):
        entries, at, idat_at = b"", mdat_at, 0
        for id, _, _, data, _, in_idat in items:
            offset = idat_at if in_idat else at
            entries += (id.to_bytes(2, "big") + int(in_idat).to_bytes(2, "big") + b"\0\0"
                        + b"\0\x01" + offset.to_bytes(4, "big") + len(data).to_bytes(4, "big"))
            if in_idat:
                idat_at += len(data)
            else:
                at += len(data)
        iloc = full(b"iloc", 1, 0, b"\x44\x00" + len(items).to_bytes(2, "big") + entries)
        return full(b"meta", 0, 0, hdlr + pitm + iloc + iinf + iprp
                    + (iref if refs else b"") + (box(b"idat", idat) if idat else b""))

    mdat_at = len(ftyp) + len(meta(0)) + 8
    mdat = box(b"mdat", b"".join(data for *_, data, _, in_idat in items if not in_idat))
    return ftyp + meta(mdat_at) + mdat


def ispe(w, h):
    return full(b"ispe", 0, 0, w.to_bytes(4, "big") + h.to_bytes(4, "big"))


def nclx(primaries, transfer, matrix, full_range):
    return box(b"colr", b"nclx" + primaries.to_bytes(2, "big") + transfer.to_bytes(2, "big")
               + matrix.to_bytes(2, "big") + bytes([0x80 if full_range else 0]))


def turned(out: Path, name: str, transforms):
    """corners() in 4:2:0, with libheif's own crop of its 64x64 picture, then
    `transforms`."""
    hvcc, props, data = coded(corners(), quality=95)
    props = [(p, e) for p, e in props if p[4:8] != b"hvcC"]
    props = [(hvcc, True)] + props + [(t, True) for t in transforms]
    (out / name).write_bytes(heif([b"heic", b"mif1", b"miaf"], 1, [(1, b"hvc1", props, data, False, False)]))


def main(out: Path):
    out.mkdir(parents=True, exist_ok=True)
    src = corners()

    # libheif's own files: 4:2:0 in a 64x64 picture libheif crops (clap).
    pillow_heif.from_pillow(src).save(out / "metadata.heic", quality=95,
                                      exif=exif().tobytes(), xmp=XMP)
    # 16-bit samples (native, little endian), which libheif stores in 10 bits.
    sixteen = b"".join((v * 257).to_bytes(2, "little") for v in src.tobytes())
    deep = pillow_heif.from_bytes(mode="RGB;16", size=src.size, data=sixteen)
    deep.save(out / "ten-bit.heic", quality=95)
    pillow_heif.from_pillow(src).save(out / "p3.heic", quality=95, matrix_coefficients=6,
                                      color_primaries=12, transfer_characteristics=13)
    # Lossless 4:4:4 through the identity matrix: the stored values are the
    # drawing's.
    pillow_heif.from_pillow(src).save(out / "identity.heic", quality=-1, chroma=444,
                                      matrix_coefficients=0)
    # Coded larger than a 64x64 block, so a header claiming it smaller is
    # caught before the picture is allocated.
    pillow_heif.from_pillow(corners(192, 128)).save(out / "wide.heic", quality=80)
    deep.save(out / "pq.heic", quality=95, matrix_coefficients=9, color_primaries=9,
              transfer_characteristics=16)

    # irot 1, then imir 0, as rotated*.avif.
    turned(out, "rotated.heic", [box(b"irot", b"\x01")])
    turned(out, "rotated-mirrored.heic",
           [box(b"irot", b"\x01"), box(b"imir", b"\x00")])

    # A grid as an iPhone writes one: hidden tiles with the nclx (here BT.709 in
    # limited range, so a reader taking another matrix or range is caught), an
    # ICC profile on the grid, its description in idat, and edge tiles the
    # grid's size cuts.
    big = corners(120, 100)
    canvas = Image.new("RGB", (128, 128), (128, 128, 128))
    canvas.paste(big)
    items, tiles = [], []
    for row in range(2):
        for col in range(2):
            tile = canvas.crop((col * 64, row * 64, col * 64 + 64, row * 64 + 64))
            hvcc, props, data = coded(tile, quality=95, matrix_coefficients=1, full_range_flag=0)
            assert any(p[4:8] == b"ispe" and p[-8:] == ispe(64, 64)[-8:] for p, _ in props)
            assert not any(p[4:8] == b"clap" for p, _ in props), "a tile libheif cropped"
            id = 2 + len(tiles)
            tiles.append(id)
            items.append((id, b"hvc1", [(hvcc, True), (ispe(64, 64), False),
                                        (nclx(1, 13, 1, False), False)], data, True, False))
    srgb = ImageCms.ImageCmsProfile(ImageCms.createProfile("sRGB")).tobytes()
    grid = b"\0\0\x01\x01" + (120).to_bytes(2, "big") + (100).to_bytes(2, "big")
    items.insert(0, (1, b"grid", [(ispe(120, 100), False), (box(b"colr", b"prof" + srgb), False)],
                     grid, False, True))
    (out / "grid.heic").write_bytes(heif([b"heic", b"mif1", b"heic", b"miaf"], 1, items,
                                         [(b"dimg", 1, tiles)]))

    for f in sorted(out.glob("*.heic")):
        assert f.stat().st_size <= 4096, f"{f.name} is {f.stat().st_size} bytes"
        print(f"{f.name}\t{f.stat().st_size}")
    print("pillow-heif", pillow_heif.__version__, pillow_heif.libheif_info())


if __name__ == "__main__":
    main(Path(sys.argv[1]))
