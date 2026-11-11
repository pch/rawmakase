# JPEG XL HDR export: interoperability checks

Status: done, 2026-11-12. This is the record of [#894](https://github.com/pch/rawmakase/issues/894)'s
completion criterion in the [HDR implementation plan](hdr-implementation-plan.md#validation-and-completion-criteria):
JPEG XL PQ, HLG and gain-map exports pass independent interoperability checks. The
export contract itself is in [HDR export](../hdr.md#hdr-export). Encoder choice and
size measurements are in [Codecs](hdr-stage1-results.md#codecs).

## The harness

`scripts/jxl-interop.py --work DIR [--photo RAW ...] [--long-edge N]` is opt-in. It
needs numpy, cargo and libjxl's tools, and uses jxl-oxide's CLI, macOS ImageIO and
Chrome when it finds them. It uses no private data. It runs in three steps:

1. It writes synthetic fixtures, files that RAWmakase opens as HDR edits:
   - `anchors`: 16-bit Rec. 2020 PQ PNG with cICP. Neutral patches at 0.001–49.3×
     graphics white, including 1 (203 cd/m²), HLG's 1,000 cd/m² and PQ's 10,000, over
     a log ramp of the same range.
   - `gamut`: the primaries and secondaries of Rec. 2020, Display P3 and Rec. 709, at
     a quarter of white, at white and at 4× white, in Rec. 2020 PQ.
   - `overrange`: a 32-bit float TIFF without a profile. It holds a ramp to 100×
     white (past PQ's and HLG's peaks) and patches with negative channels.
   - `scene`: a synthetic HDR scene, 601×397 so that blocks and groups are partial at
     the edges. It has a sky brightening to a 40× white sun, a saturated sign at 3×
     white and textured ground below white. It is written as PQ, and again as HLG
     (`scene-hlg`).
2. It exports each fixture, and each `--photo` RAW (copied in), through the export job
   as the Export dialog's queue runs it. That is the opt-in test
   `export::jxl_interop::jxl_interop_exports`, which writes 16 files per source:
   - PQ and HLG in each of Rec. 2020, Display P3 and Rec. 709;
   - the gain map over the SDR rendition;
   - the SDR rendition alone;
   - each of these lossy at the default quality 92 and lossless.

   It also writes the HDR master as floats. A RAW is exported with HDR on, limit 4
   and the camera matrix.
3. It decodes every file with each decoder and checks it as described below. It
   prints a table, writes `results.json`, and exits 1 on any failure. A missing
   fixture or export, a decoder that can't read a file, and any pixel that isn't
   finite all count as failures.

The colour math is the script's own, not RAWmakase's, so it is independent of the
code under test:
- PQ's EOTF;
- HLG's inverse OETF, then the OOTF of the 1,000 cd/m² reference display with system
  gamma 1.2 on the gamut's luminance;
- the Bradford-adapted gamut matrices, derived from the primaries' chromaticities;
- ΔE_ITP (BT.2124);
- the `jhgm` and ISO 21496-1 parsers and the reconstruction.

The harness was checked against deliberately wrong readings, and it catches each:
- a Display P3 file judged as Rec. 2020 (header and 0.054 stop);
- an HLG file read as PQ (2.7 stops);
- HLG decoded without the OOTF (0.24 stop above white).

## Decoders

| Decoder | Version | What it is used for |
| --- | --- | --- |
| libjxl `jxlinfo`, `djxl` | 0.12.0 (Homebrew) | Header fields, boxes and level; signal decoding (PPM/PFM); the gain map's base and map |
| jxl-oxide CLI | 0.12.6 | Signal decoding (float `npy`); header |
| macOS ImageIO | macOS 27.2 | HDR decode request, drawn into extended linear Rec. 2020 (a Swift reader the script compiles) |
| Google Chrome | 156.0.8078.47 | Headless: every file must load at its size, and SDR files and gain-map bases must read back from an sRGB canvas as `djxl` decodes them |

None of them applies a JPEG XL gain map. ImageIO shows a gain-map JPEG XL as its sRGB
base and exposes no auxiliary map. Chrome draws an HDR image into any canvas, even a
`rec2100-linear` float16 one, mapped to SDR range: graphics white reads 0.5 and
10,000 cd/m² reads 1. Chrome therefore can't read HDR light back. The gain map's
reconstruction is computed in the script from the standard (below). Safari decodes
through ImageIO and is not scripted separately.

## Checks and tolerances

The light each decoder gives back is compared with the master's light. That light is
converted to the export's gamut, with negative channels at 0 and the limit's roll-off
applied. It is then held to what the transfer can carry:
- PQ: 10,000 cd/m²;
- HLG: the reference display's 1,000 cd/m², and scene light 1 in each channel after
  the inverse OOTF. Saturated colours near the peak exceed that, so BT.2100's display
  can't show them at that luminance.

The script reports the share of pixels that HLG's limits change. That share is 0–48%
of the synthetic fixtures (by design, much of them is above 1,000 cd/m²) and 7% of
each photo.

Errors are |log2| of luminance in stops, with the median, p95 and signed median (bias),
above white and from 0.05 to 1 × white:

| Files | Median | p95 | Bias | Peak (p99.9) | ΔE_ITP mean | Other |
| --- | --- | --- | --- | --- | --- | --- |
| Lossless | 0.001 | 0.005 | 0.001 | 0.002 | 0.5 | every pixel within 0.01 stop; graphics white at the decoder's 1.0 within 0.5% |
| Lossy and ImageIO | 0.02 | 0.06 | 0.01 | 0.03 | 5 | anchor patches within 0.02 stop; judged after a 4× linear area reduction |
| Gain map | 0.03 | 0.1 | – | – | – | at every headroom checked; reconstruction never brighter than the display's headroom (+0.02 stop) |

The lossless tolerance allows for 16-bit quantisation and for the script's float64
matrices against RAWmakase's f32 ones. The lossy tolerance follows jixel's VarDCT at
distance 0.82. Per pixel, a photo's noise dominates the lossy error. For example, the
8-bit Z 8 raw is off by a median 0.07 stop from 0.05 to 1× white, with bias −0.004.
cjxl at effort 7 and the same distance gives 0.05 stop, at 1.2× the size. After a 4×
reduction, both encoders are at 0.011. The stage 1 record measures lossy JPEG XL the
same way. ImageIO renders through ColorSync in floats, not bit-exactly, so it gets
the lossy tolerances.

Header and container checks, with `jxlinfo`:
- primaries (Rec. 2100, P3, sRGB), transfer (PQ, HLG) and intensity target (10,000
  cd/m² for PQ, 1,000 for HLG);
- 16-bit for HDR and 8-bit for SDR and gain-map files;
- the lossy or lossless flag;
- the `Exif` and `xml ` boxes, and `jhgm` for gain maps;
- the codestream level: a lossless 16-bit frame must declare Level 10 in a `jxll` box.

ImageIO must report the matching colour space, for example `kCGColorSpaceITUR_2100_PQ`
or `kCGColorSpaceDisplayP3_HLG`.

Gain-map checks:
- The `jhgm` bundle, parsed independently: version 0, the 61-byte ISO 21496-1
  metadata with no trailing bytes, no alternate colour encoding or ICC, and
  `use_base_colour_space` set.
- The headrooms: base 0, alternate log2 of the limit's peak.
- Base-only decoding: `djxl` and jxl-oxide must give exactly the SDR export's samples
  (0 codes), and so must Chrome and ImageIO, which show the base.
- The reconstruction, computed with ISO 21496-1's formula at display headrooms 0, 1,
  2, half the alternate's and the alternate's. It is compared with the standard's log
  interpolation between the SDR export and the master at that weight. At full headroom
  it is also compared with the master's luminance.

### Decoder behaviour that isn't the export's

Each of these is judged on neutral pixels only, that is, pixels whose 8-pixel
neighbourhood is neutral, away from colour edges that lossy blocks mix in. Every
pixel's numbers are kept in `results.json`.

- **ImageIO and HLG colour.** Neutral HLG decodes within 0.0001 stop, with graphics
  white at 1.0. Saturated HLG colours come back differently, up to 0.30 stop (median)
  on the gamut patches and 0.11 stop on the scene. ImageIO decodes a lossless export
  bit-identically to the same signal in a 16-bit PNG with cICP and in cjxl's own
  lossless file (checked automatically for every lossless HLG export). This is
  ImageIO's HLG rendering, independent of the file. Its PQ decode matches the
  standard to 0.0004 stop.
- **jxl-oxide and lossy wide-gamut files.** jxl-oxide clips a lossy frame's XYB light
  in BT.709 primaries before converting it to wider ones. Saturated Rec. 2020 and P3
  colours lose up to 0.35 stop (ΔE_ITP 21–37 on the gamut patches; `djxl` gives 0.6
  on the same files). Its lossy Rec. 709 and neutral results are within the lossy
  tolerance. This is also recorded in [Codecs](hdr-stage1-results.md#codecs).
- **ImageIO ignores the intensity target.** It reports a headroom of 4.93 (1,000 /
  203) for any PQ or HLG JPEG XL, whether the intensity target is 1,000, 3,248 or
  10,000 cd/m², and doesn't clip PQ light above it (the 49× anchor decodes exactly).

## Results

Two runs on 2026-11-12, with `main` at 23a1ad046 (which includes the fix below).
Every check passes:
- 70 synthetic exports;
- 42 exports of three CC0 raw.pixls.us photos at a 2,048 px long edge (Nikon Z 8 8-bit
  compressed NEF, Canon EOS R5 CR3, Fujifilm X-T5 RAF);
- all 208 files in Chrome.

Worst case over the files in each group (`djxl`; jxl-oxide and ImageIO agree to
0.001 stop where they are judged):

| Group | Above white median / p95 | 0.05–1× white p95 | Peak | ΔE_ITP mean |
| --- | --- | --- | --- | --- |
| PQ lossless (synthetic, photos) | 0.0001 / 0.0005 | 0.0007 | 0.0002 | 0.02 |
| HLG lossless | 0.0000 / 0.0006 | 0.0025 | 0.0001 | 0.06 |
| PQ lossy, synthetic | 0.0013 / 0.0038 | 0.013 | 0.002 | 0.65 |
| PQ lossy, photos (4×) | 0.0043 / 0.0092 | 0.040 | 0.004 | 3.99 |
| HLG lossy, synthetic | 0.0013 / 0.0039 | 0.013 | 0.001 | 0.64 |
| HLG lossy, photos (4×) | 0.0043 / 0.0092 | 0.040 | 0.004 | 3.99 |

| Gain map | Worst headroom, median / p95 | Full headroom against the master, p95 above white | Over the display's headroom |
| --- | --- | --- | --- |
| Lossless | 0.0075 / 0.018 | 0.013 | never |
| Lossy (photos 4×) | 0.010 / 0.043 | 0.013 | never |

Graphics white is at 203 cd/m²: PQ signal 0.5806 and HLG signal 0.75 (0.7499 here).
ImageIO puts it at 1.0, its SDR white, to within 0.4%.

## Found and fixed

- **Codestream level** ([#1030](https://github.com/pch/rawmakase/pull/1030)). A
  lossless HDR export's 16-bit samples need wider modular buffers than codestream
  Level 5 allows, and the container had no `jxll` box, so it claimed Level 5. cjxl
  writes `jxll` 10 for the same samples. Level 5 also limits a side to 2^18 and the
  area to 2^28 pixels. Files beyond those limits now declare Level 10.

Nothing else failed. The exports' pixels, headers and gain maps were as documented.

## Decisions

- **The HLG workaround stays.** The problem is in jixel, not in libjxl: jixel 0.5.7
  (still the newest release) reads HLG input as scene light, without the OOTF. libjxl
  and every decoder above read RAWmakase's HLG header correctly.
- **Intensity target.** PQ files say 10,000 cd/m², cjxl's default for PQ and the
  format's upper bound. The content's own peak (2^limit × 203 cd/m², at most 10,000) is
  not written. ImageIO ignores the field, and libjxl uses it only when asked to tone
  map (`djxl --display_nits`). Mastering-display metadata stays unwritten.
- **Saturated HLG near its peak** is clipped per channel after the inverse OOTF, as
  BT.2100's display defines what HLG can hold. The harness compares HLG exports with
  that representable light.

## Export queue

The test `hdr_jpeg_xl_exports_cancel_fail_and_publish_as_sdr_ones_do` (batch tests)
exports lossy PQ and gain-map JPEG XL through the batch queue:
- A cancel before the commit leaves neither the export nor a temporary file.
- A cancel after the commit counts as exported.
- A photo whose edit has HDR off fails with the reason, and the next photo is still
  exported.
- A destination that can't be created fails every photo and writes nothing.
- What is published decodes whole, at its size and with its encoding.

HDR files are staged and committed by the same code as SDR ones (`stage_hdr`,
`stage_gain_map`, `batch::commit`). It needed no change.

## Not covered

- No viewer applies a JPEG XL gain map, so reconstruction is checked against the
  standard's formula, implemented independently, rather than against a viewer.
- HDR pixels in browsers: Chrome maps HDR images to SDR in canvases. HDR display on
  screen wasn't measured.
- Full-resolution photos: photos were checked at 2,048 px. The stage 1 record
  measured the 45 MP Z 8 photo's lossy export with `djxl`.
- Windows and Linux decoders: the decoders above ran on macOS. The files are
  platform-independent.
- Out of scope for #894, as the plan says:
  - **JPEG gain maps:** exported and checked with libultrahdr ([Gain maps](../hdr.md#gain-maps)).
  - **HDR TIFF:** PQ, HLG and linear float, checked against Camera Raw's PQ TIFF through
    `tifffile`.
  - **AVIF and PNG HDR:** not exported (they open as HDR files).
