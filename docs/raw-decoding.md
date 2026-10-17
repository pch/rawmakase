# RAW decoding

RAWmakase is replacing LibRaw with its own RAW decoders, in pure Rust:
`crates/rawmakase-raw`. Until the switch, the app decodes with LibRaw and only
the commands that compare decoders (`rawmakase fingerprint --decoder own`,
`rawmakase decode-dump --decoder own`) reach the new crate, through
`rawmakase-native`'s `raw` adapter. This page describes the crate, how it is
checked against LibRaw, and how to add a camera or a variant.

## Boundaries

- `crates/rawmakase-tiff` reads TIFF directories (bounded, no RAW knowledge);
  the model's DNG reader and `rawmakase-raw` use it.
- `crates/rawmakase-raw` depends only on `rawmakase-tiff` and small pure-Rust
  crates. It has no unsafe code (`#![forbid(unsafe_code)]`), no build script,
  and never panics on malformed input; every allocation is checked against
  header limits (`io::samples`). Nothing above `rawmakase-native` depends on it
  directly, and no `rawmakase_raw::` path appears outside native's `raw`
  adapter (`scripts/crate-closures.sh`).
- `rawmakase-native`'s `raw` maps its results to `camera_data` (`Metadata`,
  `CameraImage`) with the same code for both decoders (`raw::Header`), and
  develops them; no format or camera rule lives there.

## The crate

Three operations on the file's bytes, read into memory once:

- `open(bytes) -> RawInfo`: the header facts, no sensor decode;
- `preview(&RawInfo, bytes)`: the embedded JPEG thumbnails use;
- `decode(&RawInfo, bytes, &cancel) -> Sensor`: the sensor data, with the
  levels read after unpacking. Decoders check `cancel` inside their loops.

Modules, each with a small `pub(crate)` surface:

| Module | Owns |
| --- | --- |
| `io` | Bounded byte reads and the allocation limits |
| `codec/` | Decoders from bytes to samples, knowing no container or camera: bit pumps, JPEG Huffman codes, lossless JPEG, Panasonic's blocks |
| `container/` | Probing and structure per container family: where the sensor data, previews and maker notes are (`tiff`, `dng`, `rw2`) |
| `vendor/` | Per-maker rules: what a maker's own tags and maker notes mean (white balance, presets, lens, crops, levels) (`panasonic`) |
| `variant` | The one selector: `enum Variant`, and one exhaustive `match` from variant to reader and codec |
| `levels`, `geometry` | The one policy each for black and white levels, and for orientation and crops |
| `color` | The camera-to-sRGB matrix and daylight multipliers from a camera's XYZ matrix |
| `preview`, `info`, `sensor`, `error` | Output types |

A file of a recognised container in a variant this crate does not decode is
`Error::Unsupported`; there is no fallback decoder.

## Variants

| Variant | Container and encoding | Sources |
| --- | --- | --- |
| `DngUncompressed` | DNG, uncompressed 8 to 16-bit samples | DNG 1.7, TIFF 6 |
| `DngLosslessJpeg` | DNG, lossless JPEG tiles or strips; DCT JPEG ones some writers store under the same code | DNG 1.7, ITU T.81 |
| `DngDeflate` | DNG, Deflate tiles, integer (horizontal differences) or floating point (byte planes) | DNG 1.7, Adobe TIFF Technical Note 3 |
| `DngUncompressedFloat` | DNG, uncompressed 16, 24 or 32-bit floating point | DNG 1.7, IEEE 754 |
| `Rw2Delta` | Panasonic RW2/RWL raw formats 3 and 4: delta-coded 16-byte blocks | TIFF 6, ExifTool's PanasonicRaw tag names, our own measurements |
| `Rw2PackedRotated` | Panasonic RW2 raw format 5: packed 12 or 14-bit samples in 16-byte blocks | TIFF 6, ExifTool's PanasonicRaw tag names, our own measurements |

Notes from our own measurements: some action cameras' DNGs write 16 bits after
a lossless JPEG difference of length 16, which T.81 codes with none; those bits
hold the difference. A stream that fails to decode as T.81 says is decoded
again reading them, and that reading is kept only when it consumes the stream
exactly. Floating-point DNGs hold their levels on the integer scale; RAWmakase
keeps a sample's integer part. HDRMerge's floating-point mosaics (predictor
34894) difference bytes two apart, the same colour's in a Bayer row. Blackmagic's
DCT-coded tiles decode with an exact inverse DCT; they differ from LibRaw's
(libjpeg-turbo's) by one 12-bit level in under 0.03% of samples.

Panasonic and Leica (`vendor::panasonic`): the visible area is the full
height and the columns up to the right sensor border plus the left border's
width; the default crop is the camera's crop tags (else the sensor borders)
starting on even rows and columns; black is the black level tags, 15 above
them for raw formats 3 and 4; white is the bit depth's range. Older bodies
whose files RAWmakase has always read with other areas or levels have
`compat-observed` rows in `data/raw-cameras.toml` (`visible`, `white`,
`black`).

## Checking against LibRaw

LibRaw is the oracle while the decoders are written, never a fallback after.
Its results on the test dataset are recorded once (`tests/corpus/README.md`,
*Decode fingerprints*), and each variant is done when every dataset file of
it matches them: `scripts/corpus/dataset-fingerprints.py check --decoder own`
and `dataset-develop.py check --decoder own`. A difference made on purpose is
approved in `tests/corpus/decode-deltas.toml` with its evidence.

## Provenance

The decoders are written from public specifications (TIFF 6, TIFF/EP, DNG 1.7,
Exif, ITU T.81), public write-ups of the vendor formats, and black-box
comparison of LibRaw's output and Adobe DNG Converter's on our own files. No
code or tables from LibRaw, dcraw, RawSpeed, darktable or RawTherapee are used,
and their source is not consulted.
