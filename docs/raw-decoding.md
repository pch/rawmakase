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
| `codec/` | Decoders from bytes to samples, knowing no container or camera: bit pumps, JPEG Huffman codes, lossless JPEG, Nikon's Huffman-coded data and small RAWs, Nikon High Efficiency (JPEG XS), Panasonic's blocks and strips, Sony's codings, Fujifilm's compressed stripes, Canon's CRX, Olympus's coding, packed samples |
| `container/` | Probing and structure per container family: where the sensor data, previews and maker notes are (`tiff`, `dng`, `nef`, `rw2`, `arw`, `raf`, `cr3`, `orf`) |
| `vendor/` | Per-maker rules: what a maker's own tags and maker notes mean (white balance, presets, lens, crops, levels) and their decryption (`nikon`, `panasonic`, `sony`, `fujifilm`, `canon_cr3`, `olympus`) |
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
| `Rw2Seeded` | Panasonic RW2 raw format 6: seeded delta-coded 16-byte blocks of 12 or 14-bit samples | TIFF 6, ExifTool's PanasonicRaw tag names, our own measurements |
| `Rw2Packed` | Panasonic RW2 raw format 7 and older RAW raw format 2: packed 14 or 12-bit samples in 16-byte blocks | TIFF 6, ExifTool's PanasonicRaw tag names, our own measurements |
| `Rw2Huffman` | Panasonic RW2 raw format 8: prefix-coded strips of 12 to 16-bit samples | TIFF 6, ITU T.81 (differences), ExifTool's PanasonicRaw tag names, our own measurements |
| `Rw2Unpacked` | Panasonic and Leica RAW of the RW2 structure without a raw format: 12-bit samples in 16-bit words | TIFF 6, ExifTool's PanasonicRaw tag names, our own measurements |
| `ArwCurve` | Sony ARW2: 16-sample blocks of 11-bit codes on the SonyToneCurve | Dietz (IS&T EI 2016), ExifTool's Sony tags, our measurements |
| `ArwUncompressed` | Sony ARW and SR2, uncompressed 16-bit words | TIFF 6, ExifTool's Sony tags |
| `ArwPacked` | Sony ARW, packed 12-bit samples (DSLR-A850, A900) | Our measurements |
| `ArwLosslessJpeg` | Sony ARW, lossless JPEG tiles of 2 × 2 colour cells | ITU T.81, our measurements |
| `ArwYcbcr` | Sony ARW small RAWs: lossless JPEG luma and chroma tiles | ITU T.81, T.871, TIFF 6, our measurements |
| `ArwV6` | Sony ARW "compressed RAW 2": LLVC tiles of wavelet planes, a curve to linear values | SMPTE RDD 34, Adobe DNG Converter, our measurements |
| `ArwV1` | Sony's first ARW: Huffman-coded columns (DSLR-A100 to A390) | ITU T.81 (difference coding), ExifTool's Minolta tags, our measurements |
| `NefHighEfficiency` | Nikon NEF, High Efficiency and High Efficiency★ (JPEG XS), recognised by the codestream's SOC and CAP markers on any body | ISO/IEC 21122, our own measurements against Adobe DNG Converter |
| `NefUnpacked` | Nikon NEF and NRW, 16-bit words: the file's byte order, or big-endian with 12-bit samples in their top bits (Coolpix NRW) | TIFF 6, our own measurements |
| `NefPacked` | Nikon NEF and NRW, 12-bit samples packed: most or least significant bit first, rows padded to 16 bytes or not, in little-endian 32-bit words (Coolpix NRW), in groups of ten (D100), in two fields (Coolpix E-series) | TIFF 6, our own measurements |
| `NefPacked14` | Nikon NEF, 14-bit samples packed least significant bit first, rows padded to 16 bytes | TIFF 6, our own measurements |
| `NefSmallRaw` | Nikon NEF small RAW: luma and chroma (three bytes a pixel), converted to RGB through a measured tone curve | ITU-R BT.601, our own measurements |
| `NefHuffman` | Nikon NEF, Huffman-coded (TIFF compression 34713): lossless, lossy, lossy with a split | TIFF 6, ITU T.81, Laurent Clévy's NEF notes, ExifTool's Nikon tag documentation, our own measurements |
| `RafUnpacked` | Fujifilm RAF, 16-bit words row by row (the raw image directory's, or the older bodies' samples after the header) | ExifTool's FujiFilm tag names, TIFF 6, our own measurements |
| `RafSuperCcdSr` | Fujifilm RAF of a Super CCD SR (FinePix F700): each row's S pixels, then its R pixels; the S pixels are read | ExifTool's FujiFilm tag names, our own measurements |
| `RafTiled` | Fujifilm RAF of the GX680 digital back: big-endian 16-bit words in vertical tiles | ExifTool's FujiFilm tag names, our own measurements |
| `RafPacked` | Fujifilm RAF, 12-bit samples packed least significant bit first | ExifTool's FujiFilm tag names, TIFF 6, our own measurements |
| `RafPackedWords` | Fujifilm RAF, 12 or 14-bit samples packed most significant bit first in little-endian 32-bit words (raw image directory tag 0xF006 = 1) | ExifTool's FujiFilm tag names, TIFF 6, our own measurements |
| `RafCompressed` | Fujifilm RAF, compressed stripes, lossless or lossy, Bayer or X-Trans | The public description of Fujifilm's lossless compression on capnfabs.net, ExifTool's FujiFilm tag names, our own measurements |
| `Cr3Lossless` | Canon CR3, lossless CRX: Bayer planes predicted as JPEG-LS predicts, Golomb-Rice coded | ISO/IEC 14496-12, Laurent Clévy's CR3 description, ITU T.87, ExifTool's Canon tag names, our own measurements |
| `Cr3Craw` | Canon CR3 C-RAW: CRX planes in three quantized integer 5/3 wavelet levels (version 2 of the coding with quantization maps) | as `Cr3Lossless`, ITU T.800 Annex F, ITU-T H.265 (quantization steps) |
| `OrfCompressed` | Olympus and OM Digital ORF/ORI, Olympus's compressed coding of 12 or 14-bit samples | TIFF 6, ExifTool's Olympus tag names, our own measurements |
| `OrfPacked` | Olympus ORF, packed 12-bit samples: 16-byte blocks of ten, 32-bit words, or two interlaced fields of rows | TIFF 6, ExifTool's Olympus tag names, our own measurements |
| `OrfUnpacked` | Olympus ORF, 16-bit words holding samples of the note's bit depth | TIFF 6, ExifTool's Olympus tag names, our own measurements |

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
them for raw formats 3 and 4; white is the bit depth's range, less 1/256 of
it for raw formats 6 to 8 below 16 bits. Raw format 6 is read 15 below its
coded levels, and raw format 7 in whole groups of four rows (the rows past
them are 0). Raw format 8 strips stacked in more than one row (a
high-resolution DC-S1RM2 mode) are `Unsupported`. Older bodies
whose files RAWmakase has always read with other areas or levels have
`compat-observed` rows in `data/raw-cameras.toml` (`visible`, `white`,
`black`).

Sony (from our own measurements unless said otherwise):

- An ARW2 code `c` is the point `2c` on Sony's 12-bit tone curve, whose slope
  starts at 1 and doubles at each SonyToneCurve threshold (in quarters).
- The SR2SubIFD (white balance as shot and its presets, levels) is encrypted.
  Its keystream generator is written from prior general knowledge of the
  format (ExifTool's documentation names the key but describes no algorithm;
  no permitted public description was found). Validation: decrypted, it is a
  well-formed directory on every ARW of our dataset, and its
  values equal the plain copies newer bodies also write in the raw directory.
- ARW1 is one running sum of differences, each a Huffman code (a fixed table)
  and the difference's bits as JPEG codes them, read down each column from the
  right, even rows then odd rows. The code table and scan order are written
  from prior general knowledge of the format (no permitted public description
  was found) and validated by an exact match with LibRaw's output on all our
  ARW1 files; that the columns are eight rows taller than the directory says
  is our measurement. The DSLR-A100 has no raw directory: its data offset is in
  IFD0's SubIFDs tag and its size and white balance in Minolta's MRW blocks.
- Lossless JPEG ARWs' tiles are frames of half the tile's size whose four
  components are the tile's 2 × 2 cells. Small RAWs' tiles sample luma 2 × 1 or
  2 × 2; luma sampled 2 × 2 predicts a block's first row from the first row of
  the block above, not the row just above as T.81 does. Their RGB is JFIF's
  conversion in single precision with fused multiply-adds, rounded down.
- Compressed RAW 2 (ARW6) is a strip of tiles, each an SMPTE RDD 34 (LLVC)
  picture of the green mean, two chroma differences and the greens'
  residual. Sony's streams differ from RDD 34: a zero code at depth 0 is a zero
  set starting a run, a run ends when its count reaches one, and
  dequantisation reconstructs to the middle of the step. The transmission
  units' rows are shifted by what the first residual half holds beyond two
  lines. The 12-bit codes go through a curve that is the identity to 1408 and
  then interpolates a value every eighth code; its values are Adobe DNG
  Converter's for a synthetic file holding every code, and LibRaw's agree on
  every code our files hold.
- The DSC-R1's SR2 stores its words big-endian in a little-endian file.
- Visible areas, some white levels and some model names RAWmakase has always
  read are rows of `data/raw-cameras.toml` (`compat-observed`).

Nikon, from our own measurements of LibRaw's output (on the dataset's files,
and on synthetic NEFs that probe one code word, key or tag at a time):

- Nikon writes no Huffman tables. The six (lossy, lossy after the split row,
  lossless; at 12 and 14 bits) were recovered from the decoded values and
  checked code word by code word on synthetic streams. The second lossy table
  drops low bits of large differences (a symbol's high nibble); the split row
  switches tables for linearization tables of version 0x44 0x20 and 0x44
  0x40, and the bit stream runs on without alignment. Predictions are
  clamped to 14 bits before the curve.
- The curve: 0x44 0x20 stores points every 2^bits / (n − 1) values, 0x44 0x40
  over a quarter of that span, linearly interpolated; 0x44 0x10 stores the
  curve's start; values past the points are themselves. The D1X keeps its
  table (version 0x44 0x58) in tag 0x008c, its predictions 2110 bytes on.
- The maker note's encrypted blocks (ColorBalance 02xx, LensData 02xx and
  08xx) are XORed after their version with a running sum: a step from the
  serial number's low byte, a start from the XOR of the shutter count's
  bytes, both through tables measured over all 256 values; the sum grows by
  the step times a counter from 0x60. A serial number that is not all digits
  keys as 96; the D50's as 34. Lens data is read only alongside a shutter
  count.
- Levels: BlackLevel (0x003d) is at 14 bits and shifted to the data's bits;
  white is the data's full scale except where `raw-cameras.toml` says.
  CropArea's odd left or top edge moves in by one, keeping the far edges.
- ISO comes from EXIF, else the note's ISO (0x0002), else ISOInfo's byte v as
  100 × 2^(v / 12 − 5) in single precision.
- Observed compatibility keyed on the model, where the file carries nothing
  that tells (each LibRaw's reading of the dataset's files, kept so saved
  edits keep it): the D1's as-shot multipliers are unity; the COOLPIX P6000's
  CFA is read red first; the D50's encrypted blocks are keyed as serial 34.
  Rules the camera table can express are its rows instead (white and black
  levels).
- Uncompressed data is told apart by its size, whatever the compression tag
  says. Little-endian bodies pack least significant bit first, except a
  NEFCompression of 9 (Packed 12 bits); Coolpix NRWs (colour data "NRW ")
  pack into little-endian 32-bit words and store unpacked samples in the top
  bits of big-endian words, which are shifted to 12 bits unless the camera's
  white level is the words' own; the Coolpix E-series (ColorBalanceA) store
  the even rows, then the odd ones. Coolpix NRWs' ColorBalanceC black level
  and presets (daylight, shade, and fluorescent N as D65) are their own.
- Small RAWs (three bytes a pixel): BT.601 over luma ÷ 2549 and chroma ÷ 1536,
  times 3072, index a tone curve of 3072 steps topping at 16287, measured step
  by step on synthetic files and shipped as a compat-observed fact (LibRaw's
  output; the file does not carry it); from luma 2047 chroma is ignored; the second
  pixel of a pair takes the mean chroma of its pair and the next. LibRaw's
  floating-point evaluation order is not known: on the dataset's two files
  0.008% and 0.001% of the values are one or a few curve steps off (at most 4
  of 16383), so their data hashes differ: D810 `DSC_1888.NEF` (0cb5b819…,
  3013 of 36152320 values) and D4S `FU9_3802.NEF` (c8256566…, 161 of
  16163840). This is to be an approved delta at the switch.

Fujifilm (`container::raf`, `vendor::fujifilm`): the camera name is the RAF
header's, without its "FinePix" series name. The colour filter is X-Trans's
from the XTransLayout record (last colour first), a Super CCD's fixed
diagonal pattern (FujiLayout below 4, or 0x80 to 0x83), else RGGB moved by the
raw image directory's phase (0xF009). The default crop is the header's
recommended crop (records 0x110 and 0x111) starting on the filter's period (a
Super CCD's as it is); a camera aspect ratio (0x115) wider than it by more
than 1% cuts its height, centred. A Super CCD's visible area is its stored
rows (for the tall layouts, half the image record 0x121's columns from half
its 0x120 offset), developing to a square of the shorter side plus half the
longer one (`RawInfo::super_ccd`). Development places each stored pixel
where `SuperCcd::position` turns it (rows of the tall layout two to a step
across the diagonals, columns of the wide one two to a step along them; our
own measurements of LibRaw's developed images), 0 outside the turned area, and
demosaics the result as the Bayer mosaic it is; the image keeps LibRaw's size
and orientation. Black is the raw image directory's per
filter position (0xF00A), or the older bodies' record 0x4000 (green, red,
second green, blue); white is the bit depth's range. As-shot white balance is
the raw image directory's (0xF00E) or record 0x2FF0; D65 the illuminant list's
(0xF00C, light source 21) or record 0x2301, and fine weather and shade the
RAFData table's (the first run of six presets of the as-shot green, after an
as-shot entry in the first X bodies'), the X-A bodies' RAFData list (light
sources 1, 8 and 83, red and blue doubled) or records 0x2100 and 0x2200.
DevelopmentDynamicRange and RawExposureBias come from the maker notes and
record 0x9650. Visible areas, white levels, orientations and crops RAWmakase
has always read differently have `compat-observed` rows in
`data/raw-cameras.toml`. Lossy compressed data decodes exactly as LibRaw's.
The older bodies whose record 0x8200 is set (FinePix F700, S5000, S7000,
S2Pro and the GX680 back) have their as-shot white balance and presets read
with red and blue scaled down by factors the file does not give; the
factors are observed compatibility data (`wb_scale` rows, about 0.901 and
0.885, 0.761 and 0.857 for the back), so these files keep the white balance
RAWmakase has always read.

Olympus and OM Digital (`vendor::olympus`): white balance, presets, black,
bit depth, crop and aspect ratio come from the maker note's ImageProcessing
directory (RawInfo on older compacts; the main directory's red and blue
balance first where it has them). The default crop and the aspect-ratio crop
(centred in it, at the ratio the note names) start on even rows and columns.
Black is on the 12-bit scale and scaled to 14-bit samples; white is the bit
depth's range. The compressed coding was written from prior general
knowledge of the format (no decoder source consulted), then verified and
corrected by measurement against LibRaw's output; that 14-bit data's codes
start a byte later is what the measurement found. Visible widths shorter
than the stored rows are a rule by
stored width (our measurements, every body of a sensor alike) or
`compat-observed` rows; so are the white levels of older bodies and the
canonical names (`E-M1MarkIII` is `E-M1 Mark III`, OM Digital's bodies are
Olympus's). 14-bit compressed data starts a byte later than 12-bit.

Canon CR3 (`container::cr3`, `codec::crx`, `vendor::canon_cr3`): the raw image
is the first of the largest raw tracks (a Dual Pixel file's second image is as
large). The visible area is the image areas' (`IAD1`) active area, the default
crop their crop, and the aspect crop the maker notes' AspectInfo inside it.
White balance and the daylight and shade presets are the timed metadata's
ColorData. White is ColorData's SpecularWhiteLevel, which Adobe DNG
Converter's WhiteLevel follows, but for the cameras whose data Adobe reads
unclipped at that level (EOS R5, R5 Mark II, R6, R7, R8, R10, R50, R50 V and
1D X Mark III): their `adobe-dng` rows give Adobe's level. LibRaw read every
CR3 with the bit depth's range (16383), so CR3 files develop on a different
normalized scale than before (approved decode deltas, Adobe's values as
evidence). Black is ColorData's per-channel
levels (PerChannelBlackLevel), which Adobe DNG Converter's BlackLevel follows
on our files; where LibRaw measured the black in the masked border instead,
the difference is an approved decode delta (three EOS M50 files, where
Adobe measures a fractional black, are pending). Without
those levels, black is each channel's mean, rounded down, in the masked
columns from the third to 26 short of the visible (or active, if earlier) area
over the visible rows. Cameras whose visible area RAWmakase has always read
otherwise have `compat-observed` rows (`visible`). Files with HEVC previews
(HDR shots) have no JPEG preview.
C-RAW tiles next to a tile on their right are coded one column wider at
each level: the line is extended to an odd length, and below the coarsest
level its low-pass band is coded at an odd length too, its last sample unused
(our measurement). Tiles between two neighbours, and tiles stacked
vertically, are `Unsupported` (no file of the test dataset has them).

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
