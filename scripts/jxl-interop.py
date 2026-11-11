#!/usr/bin/env python3
"""JPEG XL HDR export interoperability harness (docs/records/hdr-jxl-interop.md).

Writes synthetic public fixtures, exports them (and any CC0 RAWs given with --photo)
through RAWmakase's export job in every JPEG XL encoding of #894 (PQ and HLG in Rec. 2020,
Display P3 and Rec. 709, the gain map over the SDR rendition, and the SDR rendition, each
lossy and lossless), then decodes every file with independent decoders and checks it:

- libjxl's jxlinfo and djxl, jxl-oxide's CLI and, on macOS, ImageIO (a small Swift
  reader, compiled on the first run), each where installed;
- the header's colour encoding, intensity target, bit depth and boxes;
- the light each decoder gives back (PQ's EOTF, HLG's inverse OETF and OOTF on the
  1,000 cd/m² reference display, computed here, not by RAWmakase) against the HDR master
  RAWmakase rendered, converted here to the export's gamut;
- a gain map's `jhgm` bundle parsed here, its base against the SDR export, and its ISO
  21496-1 reconstruction, computed here, at several display headrooms against the log
  interpolation of the SDR rendition and the master that the standard defines.

Needs numpy, cargo and libjxl's tools. Nothing here is a runtime dependency.

    python3 scripts/jxl-interop.py --work /tmp/jxl-interop [--photo some-cc0.nef ...]

Exits 1 when any check fails; prints a table and writes results.json in the work folder.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import platform
import re
import shutil
import struct
import subprocess
import sys
import zlib
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
WHITE = 203.0  # graphics white, cd/m² (BT.2408)
HLG_LW = 1000.0  # HLG's reference display peak, cd/m²
HLG_GAMMA = 1.2  # BT.2100's system gamma at 1,000 cd/m²

# -- colour -------------------------------------------------------------------------

PRIMARIES = {
    "rec709": ((0.64, 0.33), (0.30, 0.60), (0.15, 0.06)),
    "p3": ((0.680, 0.320), (0.265, 0.690), (0.150, 0.060)),
    "rec2020": ((0.708, 0.292), (0.170, 0.797), (0.131, 0.046)),
    "prophoto": ((0.7347, 0.2653), (0.1596, 0.8404), (0.0366, 0.0001)),
}
D65 = (0.3127, 0.3290)
D50_XYZ = np.array([0.9642, 1.0, 0.8249])  # ICC's D50


def xyz(xy):
    x, y = xy
    return np.array([x / y, 1.0, (1 - x - y) / y])


def rgb_to_xyz(name, white_xyz):
    p = np.stack([xyz(c) for c in PRIMARIES[name]], axis=1)
    s = np.linalg.solve(p, white_xyz)
    return p * s


BRADFORD = np.array(
    [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]]
)


def bradford(src, dst):
    s, d = BRADFORD @ src, BRADFORD @ dst
    return np.linalg.inv(BRADFORD) @ np.diag(d / s) @ BRADFORD


def prophoto_to(gamut):
    """Linear ProPhoto (D50) to linear `gamut` (D65), Bradford-adapted."""
    w65 = xyz(D65)
    m = rgb_to_xyz("prophoto", D50_XYZ)
    return np.linalg.inv(rgb_to_xyz(gamut, w65)) @ bradford(D50_XYZ, w65) @ m


def gamut_to(src, dst):
    w = xyz(D65)
    return np.linalg.inv(rgb_to_xyz(dst, w)) @ rgb_to_xyz(src, w)


def luminance_weights(gamut):
    return rgb_to_xyz(gamut, xyz(D65))[1]


M1, M2 = 2610 / 16384, 2523 / 4096 * 128
C1, C2, C3 = 3424 / 4096, 2413 / 4096 * 32, 2392 / 4096 * 32


def pq_encode(light):
    """PQ signal of light in graphics-white units."""
    y = np.clip(light * WHITE / 10000.0, 0, 1) ** M1
    return ((C1 + C2 * y) / (1 + C3 * y)) ** M2


def pq_decode(signal):
    e = np.clip(signal, 0, 1) ** (1 / M2)
    return (np.maximum(e - C1, 0) / (C2 - C3 * e)) ** (1 / M1) * 10000.0 / WHITE


HA = 0.17883277
HB = 1 - 4 * HA
HC = 0.5 - HA * math.log(4 * HA)


def hlg_oetf(e):
    e = np.clip(e, 0, 1)
    with np.errstate(divide="ignore", invalid="ignore"):
        return np.where(e <= 1 / 12, np.sqrt(3 * e), HA * np.log(np.maximum(12 * e - HB, 1e-12)) + HC)


def hlg_inverse_oetf(s):
    s = np.clip(s, 0, 1)
    return np.where(s <= 0.5, s * s / 3, (np.exp((s - HC) / HA) + HB) / 12)


def hlg_encode(light, gamut):
    """HLG signal of display light (graphics-white units) on the reference display:
    BT.2100's inverse OOTF, then the OETF."""
    d = np.clip(light * WHITE / HLG_LW, 0, 1)
    yd = d @ luminance_weights(gamut)
    with np.errstate(divide="ignore", invalid="ignore"):
        scale = np.where(yd > 0, yd ** ((1 - HLG_GAMMA) / HLG_GAMMA), 0)
    return hlg_oetf(d * scale[..., None])


def hlg_decode(signal, gamut):
    """Display light (graphics-white units) of an HLG signal: the inverse OETF, then the
    OOTF on the 1,000 cd/m² reference display (BT.2100, black level 0)."""
    e = hlg_inverse_oetf(signal)
    ys = e @ luminance_weights(gamut)
    return e * (ys ** (HLG_GAMMA - 1))[..., None] * HLG_LW / WHITE


def srgb_decode(v):
    v = np.asarray(v, dtype=np.float64)
    return np.where(v <= 0.04045, v / 12.92, ((v + 0.055) / 1.055) ** 2.4)


# ΔE_ITP (BT.2124) from linear Rec. 2020 light in graphics-white units.
LMS = np.array([[1688, 2146, 262], [683, 2951, 462], [99, 309, 3688]]) / 4096


def ictcp(rgb2020):
    lms = pq_encode(np.maximum(rgb2020 @ LMS.T, 0))
    i = 0.5 * lms[..., 0] + 0.5 * lms[..., 1]
    t = (6610 * lms[..., 0] - 13613 * lms[..., 1] + 7003 * lms[..., 2]) / 4096
    p = (17933 * lms[..., 0] - 17390 * lms[..., 1] - 543 * lms[..., 2]) / 4096
    return np.stack([i, t * 0.5, p], axis=-1)


def delta_e_itp(a2020, b2020):
    return 720 * np.linalg.norm(ictcp(a2020) - ictcp(b2020), axis=-1)


# -- file formats ---------------------------------------------------------------------


def write_png16(path, signal, cicp):
    """A 16-bit RGB PNG of `signal` (0–1) with a cICP chunk (primaries, transfer)."""
    h, w, _ = signal.shape
    rgb = np.round(np.clip(signal, 0, 1) * 65535).astype(">u2")
    raw = b"".join(b"\0" + rgb[i].tobytes() for i in range(h))

    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", w, h, 16, 2, 0, 0, 0)
    Path(path).write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"cICP", bytes([cicp[0], cicp[1], 0, 1]))
        + chunk(b"IDAT", zlib.compress(raw, 6))
        + chunk(b"IEND", b"")
    )


def write_float_tiff(path, rgb):
    """An uncompressed 32-bit float RGB TIFF without a profile (linear sRGB primaries,
    1 = graphics white, as RAWmakase opens it)."""
    h, w, _ = rgb.shape
    data = np.ascontiguousarray(rgb, dtype="<f4").tobytes()
    tags = [
        (256, 4, 1, w),
        (257, 4, 1, h),
        (258, 3, 3, None),
        (259, 3, 1, 1),
        (262, 3, 1, 2),
        (273, 4, 1, None),
        (277, 3, 1, 3),
        (278, 4, 1, h),
        (279, 4, 1, len(data)),
        (284, 3, 1, 1),
        (339, 3, 3, None),
    ]
    ifd_at = 8
    ifd_len = 2 + len(tags) * 12 + 4
    bits_at = ifd_at + ifd_len
    format_at = bits_at + 6
    data_at = format_at + 6
    out = bytearray(b"II*\0" + struct.pack("<I", ifd_at))
    out += struct.pack("<H", len(tags))
    for tag, kind, count, value in tags:
        if tag == 258:
            value = bits_at
        elif tag == 339:
            value = format_at
        elif tag == 273:
            value = data_at
        if kind == 3 and count == 1:
            out += struct.pack("<HHIHH", tag, kind, count, value, 0)
        else:
            out += struct.pack("<HHII", tag, kind, count, value)
    out += struct.pack("<I", 0)
    out += struct.pack("<3H", 32, 32, 32) + struct.pack("<3H", 3, 3, 3)
    out += data
    Path(path).write_bytes(bytes(out))


def read_pnm(path):
    """A PPM, PGM or PFM as floats: integers scaled to 0–1."""
    b = Path(path).read_bytes()
    parts, pos = [], 0
    while len(parts) < 4:
        m = re.compile(rb"\s*(#[^\n]*\n\s*)*(\S+)").match(b, pos)
        parts.append(m.group(2))
        pos = m.end()
    pos += 1
    magic, w, h, maxval = parts[0], int(parts[1]), int(parts[2]), float(parts[3])
    channels = 3 if magic in (b"P6", b"PF") else 1
    if magic in (b"PF", b"Pf"):
        dtype = "<f4" if maxval < 0 else ">f4"
        a = np.frombuffer(b, dtype=dtype, count=w * h * channels, offset=pos).reshape(h, w, channels)
        return a[::-1].astype(np.float64)
    dtype = ">u2" if maxval > 255 else "u1"
    a = np.frombuffer(b, dtype=dtype, count=w * h * channels, offset=pos).reshape(h, w, channels)
    return a.astype(np.float64) / maxval


def boxes(data):
    """A JPEG XL container's boxes: (type, payload)."""
    out, pos = [], 0
    while pos < len(data):
        size, kind = struct.unpack(">I4s", data[pos : pos + 8])
        head = 8
        if size == 1:
            size = struct.unpack(">Q", data[pos + 8 : pos + 16])[0]
            head = 16
        elif size == 0:
            size = len(data) - pos
        out.append((kind.decode("latin-1"), data[pos + head : pos + size]))
        pos += size
    return out


def parse_jhgm(payload):
    """libjxl's gain-map bundle (lib/extras/gain_map.cc): version, ISO 21496-1 metadata,
    the alternate colour encoding, the alternate ICC, then the map's codestream."""
    pos = 0
    version = payload[pos]
    pos += 1
    (meta_len,) = struct.unpack(">H", payload[pos : pos + 2])
    pos += 2
    metadata = payload[pos : pos + meta_len]
    pos += meta_len
    colour_len = payload[pos]
    pos += 1
    colour = payload[pos : pos + colour_len]
    pos += colour_len
    (icc_len,) = struct.unpack(">I", payload[pos : pos + 4])
    pos += 4
    icc = payload[pos : pos + icc_len]
    pos += icc_len
    return {
        "version": version,
        "metadata": metadata,
        "colour_encoding": colour,
        "icc": icc,
        "codestream": payload[pos:],
    }


def parse_iso21496(m):
    """ISO 21496-1's binary metadata (clause 7): versions, flags, headrooms, then for each
    channel the map's minimum and maximum (log2), gamma and the two offsets."""
    min_version, writer_version, flags = struct.unpack(">HHB", m[:5])
    # Bits beyond multichannel (0x80) and the base's colour space (0x40): RAWmakase's
    # writer sets none, and their readings differ between readers.
    other_flags = flags & 0x3F
    channels = 3 if flags & 0x80 else 1
    out = {
        "minimum_version": min_version,
        "writer_version": writer_version,
        "multichannel": bool(flags & 0x80),
        "use_base_colour_space": bool(flags & 0x40),
        "backward_direction": bool(flags & 0x04),
        "common_denominator": bool(flags & 0x08),
        "other_flags": other_flags,
    }
    pos = 5
    if out["common_denominator"]:
        d, bn, an = struct.unpack(">III", m[pos : pos + 12])
        pos += 12
        out["base_headroom"], out["alternate_headroom"] = bn / d, an / d
        chans = []
        for _ in range(channels):
            mn, mx, g, bo, ao = struct.unpack(">iiIii", m[pos : pos + 20])
            pos += 20
            chans.append({"min": mn / d, "max": mx / d, "gamma": g / d, "base_offset": bo / d, "alternate_offset": ao / d})
    else:
        bn, bd, an, ad = struct.unpack(">IIII", m[pos : pos + 16])
        pos += 16
        out["base_headroom"], out["alternate_headroom"] = bn / bd, an / ad
        chans = []
        for _ in range(channels):
            v = struct.unpack(">iIiIIIiIiI", m[pos : pos + 40])
            pos += 40
            chans.append(
                {
                    "min": v[0] / v[1],
                    "max": v[2] / v[3],
                    "gamma": v[4] / v[5],
                    "base_offset": v[6] / v[7],
                    "alternate_offset": v[8] / v[9],
                }
            )
    out["channels"] = chans
    out["trailing_bytes"] = len(m) - pos
    return out


def reconstruct(base_linear, gain, meta, headroom_stops):
    """ISO 21496-1's reconstruction at a display headroom (stops): the map's log2 gain
    weighted by where the display lies between the base's and the alternate's headroom."""
    hb, ha = meta["base_headroom"], meta["alternate_headroom"]
    w = np.clip((headroom_stops - hb) / (ha - hb), 0, 1)
    out = np.empty_like(base_linear)
    for c in range(3):
        ch = meta["channels"][min(c, len(meta["channels"]) - 1)]
        g = gain[..., min(c, gain.shape[-1] - 1)]
        log_gain = ch["min"] + (ch["max"] - ch["min"]) * np.clip(g, 0, 1) ** (1 / ch["gamma"])
        out[..., c] = (base_linear[..., c] + ch["base_offset"]) * np.exp2(log_gain * w) - ch["alternate_offset"]
    return out, w


# -- fixtures -------------------------------------------------------------------------

ANCHORS = [0.001, 0.01, 0.05, 0.18, 0.5, 1.0, 2.0, 4.0, HLG_LW / WHITE, 8.0, 16.0, 32.0, 10000 / WHITE]
PATCH = 32


def fixtures(folder: Path):
    """The synthetic HDR fixtures, as files RAWmakase opens as HDR edits."""
    folder.mkdir(parents=True, exist_ok=True)
    # Neutral anchors in Rec. 2020 PQ: flat patches across 14 stops, 1 = graphics white,
    # with HLG's peak and PQ's 10,000 cd/m²; below them a smooth log ramp.
    n = len(ANCHORS)
    img = np.zeros((PATCH * 2, PATCH * n, 3))
    for i, level in enumerate(ANCHORS):
        img[:PATCH, i * PATCH : (i + 1) * PATCH] = level
    ramp = np.exp2(np.linspace(-10, math.log2(10000 / WHITE), PATCH * n))
    img[PATCH:] = ramp[None, :, None]
    write_png16(folder / "anchors.png", pq_encode(img), (9, 16))
    # Gamut edges in Rec. 2020 PQ: Rec. 2020's, Display P3's and Rec. 709's primaries and
    # secondaries at a quarter of white, white and 4× white.
    colours = []
    for g in ("rec2020", "p3", "rec709"):
        m = gamut_to(g, "rec2020")
        for c in ([1, 0, 0], [0, 1, 0], [0, 0, 1], [0, 1, 1], [1, 0, 1], [1, 1, 0]):
            colours.append(m @ np.array(c, dtype=float))
    levels = [0.25, 1.0, 4.0]
    img = np.zeros((PATCH * len(levels), PATCH * len(colours), 3))
    for j, level in enumerate(levels):
        for i, c in enumerate(colours):
            y = c @ luminance_weights("rec2020")
            img[j * PATCH : (j + 1) * PATCH, i * PATCH : (i + 1) * PATCH] = c * level / y
    write_png16(folder / "gamut.png", pq_encode(img), (9, 16))
    # Overrange and negative float light (linear sRGB primaries): a ramp to 100× white,
    # past PQ's 10,000 cd/m² and HLG's 1,000, and patches with a negative channel.
    w = PATCH * 12
    img = np.zeros((PATCH * 2, w, 3), dtype=np.float32)
    img[:PATCH] = np.exp2(np.linspace(-6, math.log2(100), w))[None, :, None]
    for i, c in enumerate([[-0.2, 0.5, 0.5], [2.0, -0.5, 1.0], [8.0, 8.0, -1.0], [60.0, 30.0, 0.0]]):
        img[PATCH:, i * 3 * PATCH : (i + 1) * 3 * PATCH] = c
    write_float_tiff(folder / "overrange.tif", img)
    # A synthetic HDR scene in Rec. 2020 PQ: a sky brightening towards a sun at 40× white,
    # a saturated sign at 3× white, and textured ground below white.
    # Odd in both directions: partial 8×8 blocks and groups at the edges.
    h, w = 397, 601
    yy, xx = np.mgrid[0:h, 0:w].astype(float)
    sky = np.array([0.35, 0.55, 1.0])
    d = np.hypot(xx - 430, yy - 90)
    glow = 0.6 + 5.0 * np.exp(-((d / 140) ** 2))
    scene = sky[None, None, :] * glow[..., None]
    scene[d < 18] = 40.0
    ground = yy > 250
    tex = 0.05 + 0.5 * (0.5 + 0.5 * np.sin(xx / 7) * np.cos(yy / 5)) * np.exp2(-(yy - 250) / 60)
    scene[ground] = (tex[..., None] * np.array([0.9, 0.7, 0.45]))[ground]
    sign = (xx > 80) & (xx < 220) & (yy > 280) & (yy < 330)
    scene[sign] = np.array([3.0, 0.15, 0.4])
    write_png16(folder / "scene.png", pq_encode(scene), (9, 16))
    # The same scene as HLG in Rec. 2020: HLG input decoded through the OOTF.
    write_png16(folder / "scene-hlg.png", hlg_encode(np.minimum(scene, HLG_LW / WHITE), "rec2020"), (9, 18))


# -- decoders -------------------------------------------------------------------------


class Decoders:
    def __init__(self, work: Path, use_imageio: bool):
        self.work = work
        self.djxl = shutil.which("djxl")
        self.jxlinfo = shutil.which("jxlinfo")
        self.oxide = shutil.which("jxl-oxide")
        self.imageio = None
        if use_imageio and platform.system() == "Darwin" and shutil.which("swiftc"):
            exe = work / "imageio-reader"
            if not exe.exists():
                src = work / "imageio-reader.swift"
                src.write_text(IMAGEIO_SWIFT)
                subprocess.run(["swiftc", "-O", str(src), "-o", str(exe)], check=True, capture_output=True)
            self.imageio = str(exe)

    def names(self):
        return [n for n, v in (("djxl", self.djxl), ("jxl-oxide", self.oxide), ("ImageIO", self.imageio)) if v]

    def info(self, path):
        if not self.jxlinfo:
            return None
        out = subprocess.run([self.jxlinfo, "-v", str(path)], capture_output=True, text=True)
        return out.stdout + out.stderr

    def oxide_info(self, path):
        if not self.oxide:
            return None
        out = subprocess.run([self.oxide, "info", str(path)], capture_output=True, text=True)
        return out.stdout + out.stderr

    def decode(self, name, path, bits=16):
        """The decoded signal (0–1, as the header encodes it) or, for ImageIO, linear
        extended Rec. 2020 with ImageIO's own scale, and what the decoder said."""
        tmp = self.work / "tmp"
        tmp.mkdir(exist_ok=True)
        if name == "djxl":
            out = tmp / ("d.ppm")
            r = subprocess.run([self.djxl, str(path), str(out), f"--bits_per_sample={bits}"], capture_output=True, text=True)
            if r.returncode:
                return None, r.stderr.strip()
            return np.clip(read_pnm(out), 0, 1), ""
        if name == "jxl-oxide":
            out = tmp / "o.npy"
            r = subprocess.run([self.oxide, str(path), "-o", str(out), "-f", "npy", "-q"], capture_output=True, text=True)
            if r.returncode:
                return None, r.stderr.strip()
            a = np.load(out).astype(np.float64)
            # jxl-oxide's npy is (frames, height, width, channels).
            while a.ndim > 3:
                a = a[0]
            # Unclamped floats for lossy frames; the signal is 0–1, as an integer
            # output would hold it. NaN and infinities are kept (as NaN) for the
            # checks to count.
            return np.where(np.isfinite(a), np.clip(a, 0, 1), np.nan), ""
        if name == "ImageIO":
            out = tmp / "i.f32"
            r = subprocess.run([self.imageio, str(path), str(out), "hdr"], capture_output=True, text=True)
            if r.returncode:
                return None, r.stdout.strip() + r.stderr.strip()
            meta = json.loads(r.stdout)
            a = np.fromfile(out, dtype="<f4").astype(np.float64).reshape(meta["height"], meta["width"], 3)
            return a, meta
        raise ValueError(name)


IMAGEIO_SWIFT = r"""
// imageio-reader <in> <out.f32> <hdr|sdr>: decodes with ImageIO (an HDR decode request
// for "hdr") and draws into extended linear Rec. 2020 floats; prints what it found.
import Foundation
import ImageIO
import CoreGraphics
let a = CommandLine.arguments
guard let src = CGImageSourceCreateWithURL(URL(fileURLWithPath: a[1]) as CFURL, nil) else { print("{\"error\":\"no source\"}"); exit(1) }
var info: [String: Any] = [:]
info["type"] = (CGImageSourceGetType(src) as String?) ?? "?"
let props = CGImageSourceCopyPropertiesAtIndex(src, 0, nil) as? [String: Any] ?? [:]
for (k, v) in props where !(v is [String: Any]) { info["p_" + k] = "\(v)" }
info["iso_gain_map"] = CGImageSourceCopyAuxiliaryDataInfoAtIndex(src, 0, kCGImageAuxiliaryDataTypeISOGainMap) != nil
info["apple_gain_map"] = CGImageSourceCopyAuxiliaryDataInfoAtIndex(src, 0, kCGImageAuxiliaryDataTypeHDRGainMap) != nil
let opts: [CFString: Any] = [kCGImageSourceShouldCache: false, kCGImageSourceDecodeRequest: a[3] == "hdr" ? kCGImageSourceDecodeToHDR : kCGImageSourceDecodeToSDR]
guard let img = CGImageSourceCreateImageAtIndex(src, 0, opts as CFDictionary) else { print("{\"error\":\"no image\"}"); exit(1) }
info["bits"] = img.bitsPerComponent
info["space"] = (img.colorSpace?.name as String?) ?? "?"
info["headroom"] = img.contentHeadroom
info["width"] = img.width; info["height"] = img.height
let w = img.width, h = img.height
var buf = [Float](repeating: 0, count: w * h * 4)
let ctx = CGContext(data: &buf, width: w, height: h, bitsPerComponent: 32, bytesPerRow: w * 16, space: CGColorSpace(name: CGColorSpace.extendedLinearITUR_2020)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.floatComponents.rawValue | CGBitmapInfo.byteOrder32Little.rawValue)!
ctx.draw(img, in: CGRect(x: 0, y: 0, width: w, height: h))
var rgb = [Float](); rgb.reserveCapacity(w * h * 3)
for i in 0..<(w*h) { rgb.append(buf[4*i]); rgb.append(buf[4*i+1]); rgb.append(buf[4*i+2]) }
rgb.withUnsafeBufferPointer { _ = FileManager.default.createFile(atPath: a[2], contents: Data(buffer: $0)) }
print(String(data: try! JSONSerialization.data(withJSONObject: info, options: [.sortedKeys]), encoding: .utf8)!)
"""

# -- checks ---------------------------------------------------------------------------

# Tolerances (docs/records/hdr-jxl-interop.md#tolerances). Errors are |log2| of
# luminance in stops; "lossless" allows the 16-bit quantisation of float light, "lossy"
# jixel's VarDCT at the default quality 92 (distance 0.82).
TOL = {
    "lossless": {"max_stop": 0.01, "median": 0.001, "p95": 0.005, "bias": 0.001, "peak": 0.002, "itp": 0.5, "white": 0.005},
    "lossy": {"median": 0.02, "p95": 0.06, "bias": 0.01, "peak": 0.03, "patch": 0.02, "itp": 5.0, "white": 0.005},
    "gain_map": {"median": 0.03, "p95": 0.1, "headroom_peak": 0.02},
}
GAMUT_CODE = {"rec2020": "Rec.2100", "p3": "P3", "rec709": "sRGB"}


def luminance(rgb, gamut):
    return rgb @ luminance_weights(gamut)


def stops(a, b, floor=1e-4):
    return np.abs(np.log2(np.maximum(a, floor) / np.maximum(b, floor)))


def light_metrics(decoded, expected, gamut):
    """Stop errors of `decoded` light against `expected`, both linear in `gamut`."""
    yd, ye = luminance(decoded, gamut), luminance(expected, gamut)
    finite = np.isfinite(yd)
    above = (ye > 1.0) & finite
    mid = (ye > 0.05) & (ye <= 1.0) & finite
    out = {"nan": int((~np.isfinite(decoded)).any(axis=-1).sum())}
    for name, sel in (("above", above), ("mid", mid)):
        if sel.any():
            e = stops(yd[sel], ye[sel])
            signed = np.log2(np.maximum(yd[sel], 1e-4) / np.maximum(ye[sel], 1e-4))
            out[name] = {
                "median": float(np.median(e)),
                "p95": float(np.percentile(e, 95)),
                "max": float(e.max()),
                "bias": float(np.median(signed)),
            }
    sel = (ye > 0.01) & finite
    out["max_stop_error"] = float(stops(yd[sel], ye[sel]).max()) if sel.any() else 0.0
    # The brightest 0.1% (lossy frames ring at hard edges), and the raw maximum.
    out["peak"] = float(np.log2(np.nanpercentile(yd, 99.9) / np.percentile(ye, 99.9))) if ye.max() > 0 else 0.0
    out["max_over_expected"] = float(np.log2(np.nanmax(yd) / ye.max())) if ye.max() > 0 else 0.0
    sel = (ye > 0.01) & finite
    if sel.any():
        to2020 = gamut_to(gamut, "rec2020")
        itp = delta_e_itp(np.maximum(decoded[sel], 0) @ to2020.T, np.maximum(expected[sel], 0) @ to2020.T)
        out["itp"] = {"mean": float(itp.mean()), "p95": float(np.percentile(itp, 95))}
    return out


def patch_errors(decoded, expected, gamut):
    """Stop errors at the centres of the anchor patches (the first row of patches)."""
    errs = []
    for i in range(len(ANCHORS)):
        y0, x0 = PATCH // 2 - 4, i * PATCH + PATCH // 2 - 4
        d = luminance(decoded[y0 : y0 + 8, x0 : x0 + 8], gamut).mean()
        e = luminance(expected[y0 : y0 + 8, x0 : x0 + 8], gamut).mean()
        errs.append(float(np.log2(max(d, 1e-6) / max(e, 1e-6))))
    return errs


def roll_off(v, peak):
    """The HDR limit's roll-off (docs/hdr.md): unchanged up to 97% of the peak, then
    an exponential approach to it."""
    knee = 0.97 * peak
    room = peak - knee
    return np.where(v <= knee, v, peak - room * np.exp(-(v - knee) / room))


def expected_light(master, gamut, transfer, peak):
    """The light an export of `master` must carry: the master in the export's gamut,
    negative channels at 0, each rolled off to the limit's peak (a saturated colour can
    gain a channel above it in a narrower gamut), then what the transfer can hold: PQ
    to 10,000 cd/m²; HLG to its reference display's peak and, after the inverse OOTF,
    to scene light 1 in each channel. That last clip takes saturated colours near the
    peak, which BT.2100's display can't show at that luminance. Returns the light and
    the share of pixels HLG's scene clip changed."""
    rgb = roll_off(np.maximum(master @ prophoto_to(gamut).T, 0), peak)
    if transfer == "pq":
        return np.minimum(rgb, 10000 / WHITE), 0.0
    d = np.minimum(rgb, HLG_LW / WHITE)
    representable = hlg_decode(hlg_encode(d, gamut), gamut)
    changed = (np.abs(representable - d) > 1e-3 * np.maximum(d, 0.01)).any(axis=-1)
    return representable, float(changed.mean())


def check_header(path, info, oxide_info, transfer, gamut, lossy, gain_map):
    """Header facts libjxl's jxlinfo (and jxl-oxide's info) report, against what the
    export must say."""
    fails, facts = [], {}
    if info is None:
        return fails, facts
    want = {}
    if gain_map or transfer == "sdr":
        want = {"Primaries": "sRGB", "Transfer function": "sRGB", "bits": "8-bit"}
    else:
        want = {
            "Primaries": GAMUT_CODE[gamut],
            "Transfer function": "PQ" if transfer == "pq" else "HLG",
            "Intensity target": "10000.000000" if transfer == "pq" else "1000.000000",
            "bits": "16-bit",
        }
    for key, value in want.items():
        if key == "bits":
            m = re.search(r"(\d+-bit) RGB", info)
            facts["bits"] = m.group(1) if m else None
            ok = facts["bits"] == value
        else:
            m = re.search(rf"{re.escape(key)}: ([^\n]+)", info)
            got = m.group(1).split(" nits")[0].strip() if m else None
            facts[key] = got
            ok = got == value
        if not ok:
            fails.append(f"jxlinfo {key}: {facts.get(key)!r}, want {value!r}")
    if ("lossy" in info) != lossy:
        fails.append("jxlinfo lossy/lossless flag")
    for box in ("Exif", "xml "):
        if f'type: "{box}"' not in info:
            fails.append(f"no {box!r} box")
    # ISO/IEC 18181-1's levels: a lossless frame of more than 14 bits needs 32-bit
    # modular buffers, Level 10, which the container must declare in a `jxll` box
    # (absent, it means Level 5), as cjxl writes for such a file.
    sixteen_lossless = "16-bit" in info and not lossy
    jxll = [p for k, p in boxes(path.read_bytes()) if k == "jxll"]
    declared = jxll[0][0] if jxll else 5
    facts["level"] = declared
    if sixteen_lossless and declared != 10:
        fails.append(f"a lossless 16-bit frame declares Level {declared}, not 10")
    if gain_map and "Gain map (jhgm) box" not in info:
        fails.append("jxlinfo does not see the jhgm box")
    if oxide_info is not None:
        facts["jxl-oxide"] = " ".join(oxide_info.split())[:300]
    return fails, facts


# The CoreGraphics colour space ImageIO must decode each HDR encoding into.
IMAGEIO_SPACE = {
    ("pq", "rec2020"): "kCGColorSpaceITUR_2100_PQ",
    ("pq", "p3"): "kCGColorSpaceDisplayP3_PQ",
    ("pq", "rec709"): "kCGColorSpaceITUR_709_PQ",
    ("hlg", "rec2020"): "kCGColorSpaceITUR_2100_HLG",
    ("hlg", "p3"): "kCGColorSpaceDisplayP3_HLG",
    ("hlg", "rec709"): "kCGColorSpaceITUR_709_HLG",
}


def imageio_same_as_cjxl(dec, path, gamut):
    """Whether ImageIO decodes a lossless export exactly as it decodes libjxl's own
    lossless file of the same signal (djxl's decode, re-encoded by cjxl), or None
    without cjxl."""
    cjxl = shutil.which("cjxl")
    if not cjxl:
        return None
    signal, _ = dec.decode("djxl", path)
    png, jxl = dec.work / "tmp" / "same.png", dec.work / "tmp" / "same.jxl"
    write_png16(png, signal, ({"rec709": 1, "rec2020": 9, "p3": 12}[gamut], 18))
    subprocess.run([cjxl, str(png), str(jxl), "-d", "0"], check=True, capture_output=True)
    ours, _ = dec.decode("ImageIO", path)
    theirs, _ = dec.decode("ImageIO", jxl)
    return bool(np.array_equal(ours, theirs))


def check_hdr_export(dec, path, master, meta, transfer, gamut, lossy, anchors):
    results = {"file": path.name, "size": path.stat().st_size, "decoders": {}}
    fails = []
    info = dec.info(path)
    hf, facts = check_header(path, info, dec.oxide_info(path), transfer, gamut, lossy, False)
    fails += hf
    results["header"] = facts
    expected, clipped = expected_light(master, gamut, transfer, meta["peak"])
    results["hlg_scene_clipped"] = clipped
    neutral = erode((expected.max(-1) <= 1.05 * expected.min(-1)) & (expected.max(-1) > 0), 8)
    for name in dec.names():
        signal, note = dec.decode(name, path)
        if signal is None:
            fails.append(f"{name}: no decode ({note})")
            continue
        r = {}
        scope = judging_scope(name, lossy, transfer, gamut)
        if scope:
            r["scope"] = scope
        if name == "ImageIO":
            r["imageio"] = {k: note.get(k) for k in ("space", "headroom", "bits", "p_ProfileName", "p_Headroom")}
            want_space = IMAGEIO_SPACE[(transfer, gamut)]
            if note.get("space") != want_space:
                fails.append(f"ImageIO: colour space {note.get('space')}, want {want_space}")
            # ImageIO gives linear Rec. 2020, 1 = its SDR white; the light must be at
            # graphics white without rescaling. The scale is reported to say so.
            light = signal @ gamut_to("rec2020", gamut).T
            ye, yi = luminance(expected, gamut), luminance(light, gamut)
            sel = (ye > 0.1) & (ye < 1.0) & (neutral if scope else True)
            r["white_scale"] = float(np.median(ye[sel] / np.maximum(yi[sel], 1e-9))) if sel.any() else 1.0
            if transfer == "hlg" and not lossy:
                r["same_as_cjxl"] = imageio_same_as_cjxl(dec, path, gamut)
                if r["same_as_cjxl"] is False:
                    fails.append("ImageIO: decodes differently from cjxl's file of the same signal")
        elif transfer == "pq":
            light = pq_decode(signal)
        else:
            light = hlg_decode(signal, gamut)
        # A lossy frame (and ImageIO's float rendering) is judged after a 4× linear area
        # reduction, as docs/records/hdr-stage1-results.md#codecs measures jixel: per
        # pixel, a photo's noise dominates. Per-pixel numbers are kept beside them.
        reduced = lossy or name == "ImageIO"
        r["per_pixel"] = light_metrics(light, expected, gamut)
        judged_light, judged_expected, judged_neutral = light, expected, neutral
        if reduced:
            judged_light, judged_expected = reduce(light), reduce(expected)
            judged_neutral = reduce(neutral[..., None].astype(float))[..., 0] == 1
        if scope:
            # Judged on neutral pixels only.
            r["judged_on"] = int(judged_neutral.sum())
            if judged_neutral.any():
                r.update(light_metrics(judged_light[judged_neutral][None], judged_expected[judged_neutral][None], gamut))
        else:
            r.update(light_metrics(judged_light, judged_expected, gamut))
            # Every pixel decoded, the reduction's or not.
            r["nan"] = r["per_pixel"]["nan"]
        if anchors:
            r["patches"] = patch_errors(light, expected, gamut)
        if name != "ImageIO" and not lossy:
            want = pq_encode(expected) if transfer == "pq" else hlg_encode(expected, gamut)
            r["max_code_error"] = float(np.nanmax(np.abs(signal - want)) * 65535)
        results["decoders"][name] = r
        fails += judge(name, r, TOL["lossy" if reduced else "lossless"], lossy)
    results["fails"] = fails
    return results


def reduce(a, f=4):
    """`a` (height, width, channels) reduced by an area mean of f×f blocks, partial edge
    blocks kept (the odd-sized fixture's edges are what it tests). A NaN stays NaN."""
    h, w = a.shape[:2]
    ys, xs = np.arange(0, h, f), np.arange(0, w, f)
    sums = np.add.reduceat(np.add.reduceat(a, ys, axis=0), xs, axis=1)
    rows, cols = np.diff(np.append(ys, h)), np.diff(np.append(xs, w))
    return sums / (rows[:, None, None] * cols[None, :, None])


def erode(mask, radius):
    """`mask` where every pixel within `radius` (a square) is in it too: neutral pixels
    away from colour edges, which a lossy frame's 8×8 blocks mix into them."""
    out = mask.copy()
    padded = np.pad(mask, radius, constant_values=False)
    h, w = mask.shape
    for dy in range(2 * radius + 1):
        for dx in range(2 * radius + 1):
            out &= padded[dy : dy + h, dx : dx + w]
    return out


def judging_scope(name, lossy, transfer, gamut):
    """Why a decoder's light is judged on neutral pixels only, for a decoder behaviour
    that is not the export's (docs/records/hdr-jxl-interop.md#decoders), or None:
    - ImageIO renders saturated HLG colours its own way, identically for the same signal
      in a PNG or in cjxl's own file (checked for lossless exports);
    - jxl-oxide clips a lossy frame's XYB light in BT.709 primaries, which takes
      saturated colours of wider gamuts, and gives NaN for some lossy HLG pixels
      (docs/records/hdr-stage1-results.md#codecs)."""
    if name == "ImageIO" and transfer == "hlg":
        return "ImageIO's own rendering of saturated HLG colours"
    if name == "jxl-oxide" and lossy and (gamut != "rec709" or transfer == "hlg"):
        return "jxl-oxide's BT.709 clip of lossy XYB light"
    return None


def judge(name, r, tol, lossy):
    fails = []
    if r.get("nan"):
        fails.append(f"{name}: {r['nan']} NaN pixels")
    if r.get("judged_on") == 0:
        return fails
    for part in ("above", "mid"):
        m = r.get(part)
        if m and (m["median"] > tol["median"] or m["p95"] > tol["p95"] or abs(m["bias"]) > tol["bias"]):
            fails.append(f"{name}: {part} white median {m['median']:.4f} / p95 {m['p95']:.4f} / bias {m['bias']:+.4f} stop")
    if abs(r.get("peak", 0)) > tol["peak"]:
        fails.append(f"{name}: peak {r['peak']:+.4f} stop")
    if "patches" in r and lossy:
        worst = max(abs(e) for i, e in enumerate(r["patches"]) if ANCHORS[i] >= 0.05)
        if worst > tol["patch"]:
            fails.append(f"{name}: anchor patch off by {worst:.4f} stop")
    if abs(r.get("white_scale", 1) - 1) > tol["white"]:
        fails.append(f"{name}: graphics white at {r['white_scale']:.4f} of its SDR white")
    if "max_stop" in tol and r.get("max_stop_error", 0) > tol["max_stop"]:
        fails.append(f"{name}: lossless light off by up to {r['max_stop_error']:.4f} stop")
    if "itp" in r and r["itp"]["mean"] > tol["itp"]:
        fails.append(f"{name}: ΔE_ITP mean {r['itp']['mean']:.2f}")
    return fails


def check_gain_map(dec, path, sdr_path, master, meta, lossy):
    results = {"file": path.name, "size": path.stat().st_size, "decoders": {}}
    fails = []
    info = dec.info(path)
    hf, facts = check_header(path, info, dec.oxide_info(path), "sdr", "rec709", lossy, True)
    fails += hf
    results["header"] = facts
    data = path.read_bytes()
    bx = boxes(data)
    kinds = [k for k, _ in bx]
    results["boxes"] = kinds
    if kinds[:2] != ["JXL ", "ftyp"] or "jxlc" not in kinds:
        fails.append(f"container boxes {kinds}")
    jhgm = [p for k, p in bx if k == "jhgm"]
    if len(jhgm) != 1:
        results["fails"] = fails + [f"{len(jhgm)} jhgm boxes"]
        return results
    bundle = parse_jhgm(jhgm[0])
    iso = parse_iso21496(bundle["metadata"])
    results["bundle"] = {
        "version": bundle["version"],
        "metadata_bytes": len(bundle["metadata"]),
        "colour_encoding_bytes": len(bundle["colour_encoding"]),
        "icc_bytes": len(bundle["icc"]),
        "codestream_bytes": len(bundle["codestream"]),
    }
    results["iso21496"] = iso
    if bundle["version"] != 0:
        fails.append("jhgm version")
    # The reconstruction below is ISO 21496-1's forward one, from an SDR base, as a
    # reader that supports minimum version 0 applies it; anything else is not read so.
    # The layout RAWmakase writes and this record states: one channel, fractions with
    # their own denominators (61 bytes), no other flag bits.
    if iso["other_flags"] or len(bundle["metadata"]) != 61 or iso["multichannel"]:
        fails.append(f"ISO 21496-1 metadata layout: flags {iso['other_flags']:#04x}, {len(bundle['metadata'])} bytes")
    if iso["minimum_version"] != 0:
        fails.append(f"ISO 21496-1 minimum version {iso['minimum_version']}, which version 0 readers refuse")
    if iso["backward_direction"]:
        fails.append("ISO 21496-1 backward direction: the base would be the HDR rendition")
    if iso["trailing_bytes"]:
        fails.append(f"ISO 21496-1 metadata has {iso['trailing_bytes']} trailing bytes")
    # The reconstruction below works in the base's colour space (sRGB), as the export
    # documents: the flag set, and no alternate colour encoding or ICC to read.
    if not iso["use_base_colour_space"] or bundle["colour_encoding"] or bundle["icc"]:
        fails.append("the map doesn't apply in the base's colour space, or an alternate colour space is given")
    if iso["base_headroom"] != 0 or abs(iso["alternate_headroom"] - math.log2(meta["peak"])) > 1e-3:
        fails.append(f"headrooms {iso['base_headroom']} / {iso['alternate_headroom']}, want 0 / {math.log2(meta['peak'])}")
    map_file = dec.work / "tmp" / "map.jxl"
    map_file.parent.mkdir(exist_ok=True)
    map_file.write_bytes(bundle["codestream"])
    expected_hdr = roll_off(np.maximum(master @ prophoto_to("rec709").T, 0), meta["peak"])
    y_hdr = luminance(expected_hdr, "rec709")
    for name in [n for n in dec.names() if n != "ImageIO"]:
        base, note = dec.decode(name, path, bits=8)
        sdr, note2 = dec.decode(name, sdr_path, bits=8)
        gmap, note3 = dec.decode(name, map_file, bits=8)
        if base is None or sdr is None or gmap is None:
            fails.append(f"{name}: no decode ({note}{note2}{note3})")
            continue
        r = {}
        if not all(np.isfinite(a).all() for a in (base, sdr, gmap)):
            fails.append(f"{name}: pixels that aren't finite in the base, the SDR export or the map")
            results["decoders"][name] = r
            continue
        r["base_vs_sdr_max_code"] = float(np.abs(base - sdr).max() * 255)
        if r["base_vs_sdr_max_code"] > 0:
            fails.append(f"{name}: base differs from the SDR export by {r['base_vs_sdr_max_code']:.0f} codes")
        if gmap.shape[:2] != base.shape[:2]:
            r["map_size"] = list(gmap.shape)
        base_lin = srgb_decode(base)
        y_sdr = luminance(srgb_decode(sdr), "rec709")
        # Lossy: judged after the 4× area reduction, as the HDR exports are.
        shrink = (lambda y: reduce(y[..., None])[..., 0]) if lossy else (lambda y: y)
        heads = sorted({0.0, 1.0, 2.0, round(iso["alternate_headroom"] / 2, 3), iso["alternate_headroom"]})
        r["headrooms"] = {}
        for hs in heads:
            rec, w = reconstruct(base_lin, gmap, iso, hs)
            y = luminance(rec, "rec709")
            ch = iso["channels"][0]
            # ISO 21496-1's interpolation of the two renditions at this weight, in log
            # space with the map's offsets, its gain held to the map's range.
            g = np.clip(np.log2((y_hdr + ch["alternate_offset"]) / (y_sdr + ch["base_offset"])), ch["min"], ch["max"])
            want = (y_sdr + ch["base_offset"]) * np.exp2(g * w) - ch["alternate_offset"]
            if not np.isfinite(y).all():
                fails.append(f"{name}: headroom {hs:g}: {int((~np.isfinite(y)).sum())} pixels not finite")
            hp = float(np.log2(np.nanmax(y)) - hs) if np.nanmax(y) > 0 else 0.0
            y, want = shrink(y), shrink(want)
            sel = want > 0.05
            e = stops(y[sel], want[sel])
            above = want > 1.0
            ea = stops(y[above], want[above]) if above.any() else np.array([0.0])
            r["headrooms"][f"{hs:g}"] = {
                "weight": float(w),
                "median": float(np.median(e)),
                "p95": float(np.percentile(e, 95)),
                "above_median": float(np.median(ea)),
                "above_p95": float(np.percentile(ea, 95)),
                "peak_over_headroom": hp,
            }
            t = TOL["gain_map"]
            if np.median(e) > t["median"] or np.percentile(e, 95) > t["p95"]:
                fails.append(f"{name}: headroom {hs:g}: median {np.median(e):.4f} / p95 {np.percentile(e, 95):.4f} stop")
            if hp > t["headroom_peak"]:
                fails.append(f"{name}: headroom {hs:g}: reconstruction peaks {hp:+.3f} stop over the display")
        rec, _ = reconstruct(base_lin, gmap, iso, iso["alternate_headroom"])
        y_rec = shrink(luminance(rec, "rec709"))
        r["vs_master"] = light_metrics(np.repeat(y_rec[..., None], 3, -1), np.repeat(shrink(y_hdr)[..., None], 3, -1), "rec709")
        r["vs_master"].pop("itp", None)
        m = r["vs_master"].get("above")
        if m and (m["median"] > TOL["gain_map"]["median"] or m["p95"] > TOL["gain_map"]["p95"]):
            fails.append(f"{name}: full headroom against the master: median {m['median']:.4f} / p95 {m['p95']:.4f}")
        results["decoders"][name] = r
    if dec.imageio:
        signal, note = dec.decode("ImageIO", path)
        sdr, _ = dec.decode("ImageIO", sdr_path)
        r = {"imageio": note if isinstance(note, dict) else {"error": note}}
        if signal is None or sdr is None:
            fails.append(f"ImageIO: no decode of the gain-map file or its SDR export ({note})")
        elif not (np.isfinite(signal).all() and np.isfinite(sdr).all()):
            fails.append("ImageIO: pixels that aren't finite in the gain-map file or its SDR export")
        else:
            # ImageIO shows the base (it applies no JPEG XL gain map): it must be the
            # SDR export's pixels as ImageIO decodes that file.
            r["base_vs_sdr_max"] = float(np.abs(signal - sdr).max())
            r["applies_gain_map"] = bool(np.nanmax(signal) > 1.01)
            if r["base_vs_sdr_max"] > 0:
                # Brighter than SDR white would mean ImageIO now applies the map, which
                # this check would then have to compare with the reconstruction.
                what = "applies the gain map" if r["applies_gain_map"] else "shows a base unlike the SDR export"
                fails.append(f"ImageIO {what}: {r['base_vs_sdr_max']:.4f} from the SDR export")
        results["decoders"]["ImageIO"] = r
    results["fails"] = fails
    return results


CHROME_PAGE = r"""<html><body><pre id=o>pending</pre>%s<script>
// The images are in the page, so its load event waits for them; then each is read back,
// synchronously, from an 8-bit sRGB canvas: what Chrome shows on an SDR display (an HDR
// image tone-mapped, a gain-map file's base). Only the files named in readBack have their
// pixels kept.
const readBack = %s;
window.onload = () => {
  const out = {};
  for (const i of document.querySelectorAll('img')) {
    const f = i.getAttribute('src');
    try {
      if (!i.complete || !i.naturalWidth) { out[f] = {error: 'not decoded'}; continue; }
      out[f] = {w: i.naturalWidth, h: i.naturalHeight};
      if (!readBack.includes(f)) continue;
      const c = document.createElement('canvas'); c.width = i.naturalWidth; c.height = i.naturalHeight;
      const x = c.getContext('2d'); x.drawImage(i, 0, 0);
      const d = x.getImageData(0, 0, c.width, c.height).data;
      let s = ''; for (let k = 0; k < d.length; k += 8192) s += String.fromCharCode.apply(null, d.subarray(k, k + 8192));
      out[f].rgba = btoa(s);
    } catch (e) { out[f] = {error: '' + e}; }
  }
  document.getElementById('o').textContent = JSON.stringify(out);
};
</script></body></html>"""


def find_chrome():
    for c in (
        os.environ.get("CHROME"),
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        shutil.which("google-chrome"),
        shutil.which("chromium"),
    ):
        if c and Path(c).exists():
            return c
    return None


def chrome_check(dec, chrome, files, chunk=12):
    """[`chrome_run`] over `files` a few at a time: one page holding every photo's pixels
    is too large for headless Chrome's DOM dump."""
    out = {}
    for i in range(0, len(files), chunk):
        got = chrome_run(dec, chrome, files[i : i + chunk])
        if "error" in got:
            return got
        out.update(got)
    return out


def reads_back(f):
    return ".sdr-" in f.name or ".gainmap-" in f.name


def chrome_run(dec, chrome, files):
    """Chrome's decode of each file: it must load at the file's size, and an SDR file
    or a gain-map file's base must read back from an sRGB canvas as djxl decodes it.
    Chrome draws HDR images into any canvas mapped to SDR, so HDR light is not read."""
    import base64

    page = dec.work / "chrome.html"
    images = "".join(f'<img src="{f.name}" style="display:none">' for f in files)
    page.write_text(CHROME_PAGE % (images, json.dumps([f.name for f in files if reads_back(f)])))
    link = files[0].parent / "chrome.html"
    shutil.copy2(page, link)
    # Headless Chrome's virtual time can run out before the decodes finish when the
    # machine is busy: the page then still says "pending", and the run is retried.
    for _ in range(3):
        r = subprocess.run(
            [chrome, "--headless=new", f"--user-data-dir={dec.work / 'chrome-profile'}", "--allow-file-access-from-files",
             "--virtual-time-budget=60000", "--dump-dom", link.as_uri()],
            capture_output=True, text=True, timeout=600,
        )
        m = re.search(r'<pre id="o">(.*?)</pre>', r.stdout, re.S)
        if m and m.group(1) != "pending":
            break
    else:
        return {"error": "Chrome gave no result in three runs"}
    got = json.loads(m.group(1).replace("&quot;", '"').replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">"))
    out = {}
    for f in files:
        g = got.get(f.name, {"error": "missing"})
        res = {k: v for k, v in g.items() if k != "rgba"}
        if "rgba" in g:
            a = np.frombuffer(base64.b64decode(g["rgba"]), np.uint8).reshape(g["h"], g["w"], 4)[..., :3]
            res["rgb"] = a
        out[f.name] = res
    return out


# What `jxl_interop_exports` writes for each source (its `encodings`), and the
# synthetic fixtures `fixtures` writes.
VARIANTS = [
    f"{t}-{g}-{m}" for m in ("lossy", "lossless") for t in ("pq", "hlg") for g in ("rec2020", "p3", "rec709")
] + [f"{k}-{m}" for m in ("lossy", "lossless") for k in ("gainmap", "sdr")]
FIXTURES = {"anchors", "gamut", "overrange", "scene", "scene-hlg"}


def run_exports(fixture_dir: Path, long_edge: int, cargo_args):
    env = dict(os.environ, RAWMAKASE_JXL_INTEROP=str(fixture_dir))
    if long_edge:
        env["RAWMAKASE_JXL_INTEROP_LONG_EDGE"] = str(long_edge)
    cmd = ["cargo", "test", "--release", "--locked", "-p", "rawmakase-export", *cargo_args, "--", "--ignored", "--exact", "export::jxl_interop::jxl_interop_exports"]
    subprocess.run(cmd, cwd=ROOT, env=env, check=True)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--work", type=Path, required=True, help="a folder for fixtures, exports and results")
    ap.add_argument("--photo", type=Path, action="append", default=[], help="a CC0 RAW to export too (copied in); with --no-export, the RAWs whose exports must be there")
    ap.add_argument("--long-edge", type=int, default=0, help="resize every export to this long edge")
    ap.add_argument("--no-export", action="store_true", help="check the exports already in the work folder")
    ap.add_argument("--no-check", action="store_true", help="export only")
    ap.add_argument("--no-imageio", action="store_true")
    ap.add_argument("--no-chrome", action="store_true")
    a = ap.parse_args()
    work = a.work.resolve()
    fx = work / "fixtures"
    if not a.no_export:
        if fx.exists():
            shutil.rmtree(fx)
        fixtures(fx)
        for i, p in enumerate(a.photo):
            # A dot-free stem of its own: export names are "<stem>.<variant>.jxl", and
            # two photos of one name (or of names that read the same) both get checked.
            shutil.copy2(p, fx / (f"photo{i + 1}-" + re.sub(r"[^A-Za-z0-9]+", "-", p.stem) + p.suffix))
        run_exports(fx, a.long_edge, [])
    if a.no_check:
        return
    dec = Decoders(work, not a.no_imageio)
    if not dec.djxl:
        sys.exit("djxl (libjxl) is required")
    print("decoders:", ", ".join(dec.names()), "| jxlinfo" if dec.jxlinfo else "")
    out = fx / "out"
    all_results, failed = [], 0
    masters = sorted(out.glob("*.master.json"))
    stems = {m.name[: -len(".master.json")] for m in masters}
    # Every source's sixteen exports, and every synthetic fixture: nothing missing is
    # passed over.
    wanted = FIXTURES | {f"photo{i + 1}-" + re.sub(r"[^A-Za-z0-9]+", "-", p.stem) for i, p in enumerate(a.photo)}
    missing = sorted(wanted - stems) + [
        f"{stem}.{v}.jxl" for stem in sorted(stems) for v in VARIANTS if not (out / f"{stem}.{v}.jxl").exists()
    ]
    if missing:
        print("FAIL missing exports:", ", ".join(missing))
        all_results.append({"missing": missing, "fails": missing})
        failed += 1
    for meta_path in masters:
        stem = meta_path.name[: -len(".master.json")]
        meta = json.loads(meta_path.read_text())
        master = np.fromfile(out / f"{stem}.master.f32", dtype="<f4").astype(np.float64).reshape(meta["height"], meta["width"], 3)
        for path in sorted(out.glob(f"{stem}.*.jxl")):
            variant = path.name[len(stem) + 1 : -4]
            lossy = variant.endswith("-lossy")
            if variant.startswith("sdr"):
                continue
            if variant.startswith("gainmap"):
                r = check_gain_map(dec, path, out / f"{stem}.sdr-{'lossy' if lossy else 'lossless'}.jxl", master, meta, lossy)
            else:
                transfer, gamut = variant.split("-")[:2]
                r = check_hdr_export(dec, path, master, meta, transfer, gamut, lossy, stem == "anchors")
            r["fixture"], r["variant"] = stem, variant
            all_results.append(r)
            status = "FAIL" if r["fails"] else "ok"
            failed += bool(r["fails"])
            summary = []
            for name, d in r["decoders"].items():
                if "above" in d:
                    summary.append(f"{name} {d['above']['median']:.4f}/{d['above']['p95']:.4f} pk{d['peak']:+.3f}")
                elif "headrooms" in d:
                    full = d["headrooms"][max(d["headrooms"], key=float)]
                    summary.append(f"{name} full {full['median']:.4f}/{full['p95']:.4f}")
            print(f"{status:4} {stem:24} {variant:22} {r['size']:>9} " + " | ".join(summary))
            for f in r["fails"]:
                print("      -", f)
    chrome = None if a.no_chrome else find_chrome()
    if chrome:
        files = sorted(out.glob("*.jxl"))
        seen = chrome_check(dec, chrome, files)
        n_ok, chrome_fails = 0, []
        if "error" in seen:
            chrome_fails.append(seen["error"])
        else:
            for f in files:
                g = seen[f.name]
                if "error" in g:
                    chrome_fails.append(f"{f.name}: {g['error']}")
                    continue
                size = json.loads((out / (f.name.split(".")[0] + ".master.json")).read_text())
                if (g["w"], g["h"]) != (size["width"], size["height"]):
                    chrome_fails.append(f"{f.name}: {g['w']}×{g['h']}, not {size['width']}×{size['height']}")
                    continue
                want, _ = dec.decode("djxl", f, bits=8) if reads_back(f) else (None, None)
                ref = np.round(want * 255) if want is not None else None
                if ref is None:
                    n_ok += 1
                elif ref.shape[:2] != g["rgb"].shape[:2]:
                    chrome_fails.append(f"{f.name}: size {g['rgb'].shape[:2]}")
                else:
                    diff = int(np.abs(ref - g["rgb"]).max())
                    if diff > 1:
                        chrome_fails.append(f"{f.name}: {diff} codes from djxl's decode")
                    else:
                        n_ok += 1
        print(f"Chrome ({chrome}): {n_ok} of {len(files)} files load; SDR files and gain-map bases as djxl decodes them")
        for f in chrome_fails:
            print("      -", f)
        all_results.append({"chrome": chrome, "files": len(files), "ok": n_ok, "fails": chrome_fails})
        failed += bool(chrome_fails)
    (work / "results.json").write_text(json.dumps(all_results, indent=1, default=str))
    print(f"{len(all_results)} files, {failed} failing; results in {work / 'results.json'}")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
