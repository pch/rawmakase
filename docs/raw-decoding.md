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
  header limits (`io::samples`). Metadata records are read in place or up to
  what their reader uses (the `container::tiff` accessors take each tag's
  limit), so records of any declared length cost `open` no more; the test
  that counts its allocations (`tests/hostile_records.rs`, a counting global
  allocator, the only unsafe code and outside the library) keeps it so.
  Nothing above `rawmakase-native` depends on it
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
| `codec/` | Decoders from bytes to samples, knowing no container or camera: bit pumps, JPEG Huffman codes, lossless JPEG, Nikon's Huffman-coded data and small RAWs, Nikon High Efficiency (JPEG XS), Panasonic's blocks and strips, Sony's codings, Fujifilm's compressed stripes, Canon's CRX, Olympus's coding, packed samples, Pentax's Huffman-coded differences, Samsung's codings, Canon's CRW blocks, Phase One's IIQ rows, Hasselblad's pairs |
| `container/` | Probing and structure per container family: where the sensor data, previews and maker notes are (`tiff`, `dng`, `nef`, `rw2`, `arw`, `raf`, `cr3`, `orf`, `pef`, `srw`, `erf`, `cr2`, `headerless`, `ciff`, `iiq`, `fff`, `mos`, `sti`, `brcm`, `mrw`, `dcs`, `kdc`) |
| `vendor/` | Per-maker rules: what a maker's own tags and maker notes mean (white balance, presets, lens, crops, levels) and their decryption (`nikon`, `panasonic`, `sony`, `fujifilm`, `canon_cr3`, `olympus`, `pentax`, `samsung`, `epson`, `canon`, `phase_one`, `hasselblad`, `leaf`) |
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
| `Cr2LosslessJpeg` | Canon CR2: a mosaic in one lossless JPEG strip cut into vertical slices | TIFF 6, ITU T.81, Laurent Clévy's CR2 write-up, ExifTool's Canon tag names, our own measurements |
| `CanonTif` | Canon EOS-1D and 1Ds TIF: a thumbnail TIFF whose maker note's RawDataOffset is one unsliced lossless JPEG stream of the mosaic | TIFF 6, ITU T.81, ExifTool's Canon tag names, our own measurements |
| `Cr2Sraw` | Canon CR2 sRAW and mRAW: luma sampled 2 × 1 or 2 × 2 and chroma in one sliced lossless JPEG strip, converted to RGB | TIFF 6, ITU T.81, Laurent Clévy's CR2 write-up, ExifTool's Canon tag names, our own measurements |
| `HeaderlessPacked` | Headerless RAW files known by their size (CHDK's from Canon PowerShots, action cameras', the Pentax Optio S4's, the Kodak C603's): packed 8, 10, 12 or 16-bit samples, an 8-bit code's value its curve's | Our own measurements; the C603 CCDI curve measured from LibRaw's output |
| `CrwCompressed` | Canon CRW: a CIFF file of Canon's coded blocks of differences | ExifTool's CanonRaw tag names and CRW format notes, black-box measurements of the oracle on synthetic files, our own measurements |
| `IiqL` | Phase One IIQ L and L16 (Leaf Credo too): rows of differences in runs of even and odd columns, lengths chosen every eight columns | ExifTool's PhaseOne tag names, prior general knowledge of the coding verified against LibRaw's output, our own measurements |
| `IiqS` | Phase One's first IIQ S: IIQ L's rows, values below 256 on a square-law curve | as `IiqL` |
| `IiqSv2` | Phase One IIQ Sv2: per group of eight columns coded lengths and a width, quantised differences of 14-bit values | Our own black-box measurements of LibRaw's output |
| `IiqUncompressed` | Phase One IIQ and TIFF, 16-bit words | ExifTool's PhaseOne tag names, our own measurements |
| `FffUncompressed` | Hasselblad 3FR and FFF, 16-bit words | TIFF 6, DNG 1.7 (level tags), our own measurements |
| `MosLosslessJpeg` | Leaf MOS, one lossless JPEG tile whose MCU rows are the image's rows | TIFF 6, ITU T.81, ExifTool's Leaf tag names, our own measurements |
| `MosUncompressed` | Leaf MOS, 16-bit words | TIFF 6, ExifTool's Leaf tag names, our own measurements |
| `StiUncompressed` | Sinar STI, one tile of 16-bit words | TIFF 6, TIFF/EP, our own measurements |
| `BrcmPacked` | Raspberry Pi: a JPEG, then Broadcom's raw block of 10-bit samples packed four in five bytes | Our own measurements |
| `MrwPacked` | Minolta MRW, 12-bit samples packed most significant bit first | ExifTool's MinoltaRaw tag names, our own measurements |
| `MrwUnpacked` | Minolta MRW, 16-bit big-endian words | ExifTool's MinoltaRaw tag names, our own measurements |
| `DcsLosslessJpeg` | Kodak DCS TIFF (DCS520C, DCS560C, Canon EOS D2000C): one lossless JPEG strip of 12-bit codes on the KodakIFD's curve | TIFF 6, TIFF/EP, ITU T.81, ExifTool's Kodak tag names, our own measurements |
| `DcsKodak` | Kodak DCR (DCS760C, DCS Pro 14N, 14nx, SLR/c, SLR/n): the DCS TIFF, its strip in Kodak's coding (compression 65000) | TIFF 6, ExifTool's Kodak tag names, prior general knowledge of the coding verified against LibRaw's output, our own measurements |
| `DcsEightBit` | Kodak's older DCS TIFF (DCS460, EOS DCS 1 and 3): 8-bit codes on IFD0's GrayResponseCurve | TIFF 6, our own measurements |
| `KdcPacked` | Kodak KDC of the EasyShare and P-series compacts: packed 12-bit samples | TIFF 6, ExifTool's Kodak tag names, our own measurements |
| `FffCompressed` | Hasselblad 3FR and FFF (compression 7): lossless JPEG headers, then pairs of coded differences in little-endian 32-bit words | ITU T.81, prior general knowledge of the coding verified against LibRaw's output, our own measurements |
| `OrfCompressed` | Olympus and OM Digital ORF/ORI, Olympus's compressed coding of 12 or 14-bit samples | TIFF 6, ExifTool's Olympus tag names, our own measurements |
| `OrfPacked` | Olympus ORF, packed 12-bit samples: 16-byte blocks of ten, 32-bit words, or two interlaced fields of rows | TIFF 6, ExifTool's Olympus tag names, our own measurements |
| `OrfUnpacked` | Olympus ORF, 16-bit words holding samples of the note's bit depth | TIFF 6, ExifTool's Olympus tag names, our own measurements |
| `PefHuffman` | Pentax PEF (Ricoh's too), Huffman-coded differences with the maker note's table | TIFF 6, ITU T.81 (differences), ExifTool's Pentax tag names, our own measurements |
| `PefPacked` | Pentax and Samsung GX PEF, packed 12-bit rows (compression 32773 or 1) | TIFF 6, our own measurements |
| `PefUnpacked` | Pentax PEF, 16-bit words | TIFF 6, our own measurements |
| `SrwUnpacked` | Samsung SRW (compression 32769), 16-bit words | TIFF 6, our own measurements |
| `SrwPacked` | Samsung SRW (compression 32770), packed 12-bit rows | TIFF 6, our own measurements |
| `SrwBlocks` | Samsung SRW (compression 32770, smaller data), rows of blocks of 16 differences | ExifTool's Samsung tag names, our own measurements |
| `SrwHuffman` | Samsung SRW (compression 32772), Huffman-coded differences with a fixed table | ITU T.81 (differences), our own measurements |
| `SrwTiles` | Samsung SRW (compression 32773), tiles of 16 scaled differences | our own measurements |
| `ErfPacked` | Epson ERF, packed 12-bit samples in 16-byte blocks of ten and a pad byte | TIFF 6, ExifTool's Epson tag names, our own measurements |
| `MefPacked` | Mamiya MEF, packed 12-bit samples | TIFF 6, our own measurements |

Notes from our own measurements: some action cameras' DNGs write 16 bits after
a lossless JPEG difference of length 16, which T.81 codes with none; those bits
hold the difference. A stream that fails to decode as T.81 says is decoded
again reading them, and that reading is kept only when it consumes the stream
exactly. Floating-point DNGs hold their levels on the integer scale; RAWmakase
keeps a sample's integer part. HDRMerge's floating-point mosaics (predictor
34894) difference bytes two apart, the same colour's in a Bayer row. Blackmagic's
DCT-coded tiles decode with an exact inverse DCT; they differ from LibRaw's
(libjpeg-turbo's) by one 12-bit level in under 0.03% of samples.

DNG levels follow the DNG processing model ("Mapping Raw Values to Linear
Reference Values"). Whole BlackLevel values without BlackLevelDeltaH or
DeltaV stay the sensor's levels, exactly as stored. Other levels (fractional
blacks from Google's phones and the Sigma fp, the EOS-1Ds Mark III's per-row
deltas) cannot be sensor levels, so the samples themselves become linear
reference values on a 16-bit scale: each pixel's black (the pattern plus its
column's and row's deltas) is subtracted, the result scaled by 1 / (white −
the largest black) and clipped to 0–65535, with black 0 and white 65535.
LibRaw truncated fractional blacks and ignored the deltas.

DNG colour: a camera Adobe knows has Adobe's matrix (`data/raw-matrices.toml`;
Adobe DNG Converter rewrites such DNGs with its own matrices, so its D65
ColorMatrix for a DNG the camera wrote is the row). Otherwise the file's
ColorMatrix at D65, as the DNG Specification interpolates two calibrations:
the D65 one, else by inverse colour temperature between the two (Yuneec's A
and D75), else the nearer (D50 of A and D50). LibRaw interpolates the same way
(its Yuneec CGO3+ matrix is ours to 1e-7).

DNG crops: the default crop starts on even rows and columns, keeping its far
edges (one from the sensor's corner is none); where a camera row moves the
visible area, the crop moves with it, floored to even rows and columns.
DefaultUserCrop gives the aspect-ratio crop, its fractions of the default crop
rounded inward to even rows and columns.

DNG lens IDs come from the maker note DNGPrivateData keeps: Pentax's and
Samsung's Pentax bodies' own ("PENTAX \0", `vendor::pentax`: LensType's
series × 256 + number, as Adobe's aux:LensID "4 28" is 1052), or Nikon's in
Adobe's "MakN" record (`vendor::nikon`, read as a NEF's note).

A DNG whose white balance is AsShotWhiteXY (no AsShotNeutral) has the
white's camera neutral: AnalogBalance × CameraCalibration × ColorMatrix,
interpolated at the white's correlated colour temperature (Robertson's
method), times its XYZ; for a camera with an Adobe matrix row, through that
D65 matrix.

DNG names: a DNG without Make and Model (Blackmagic's, Parrot's) is named by
its UniqueCameraModel, the first word the maker. Ricoh Imaging's Pentax
bodies are Pentax's (`names`). Canonical makes are empty for makers outside
the camera tables (`names::KNOWN`), as RAWmakase has always had them.

DNG opcodes (`container/dng/opcodes.rs`, the specification's "Opcode List
Processing"): OpcodeList1's apply to the stored image before the
LinearizationTable, OpcodeList2's to the active area's linear values (so a
file with any becomes linear values as above): FixBadPixelsConstant and
FixBadPixelsList (each bad pixel the mean of its colour's nearest good
neighbours), MapTable, MapPolynomial, GainMap (phones' lens shading, gains up
to 6 in our files), DeltaPerRow/Column, ScalePerRow/Column and, in
OpcodeList1, FixVignetteRadial. OpcodeList2's FixVignetteRadial is left to
development, which reads it as the file's lens correction with OpcodeList3's
warps (`rawmakase-model`'s DNG reader). Warps, TrimBounds, later versions'
and unknown opcodes, and gain maps of gains 1 are skipped; a list that does
not parse is not applied. A GainMap's point for a pixel is its row and column
over the modified image's height and width. LibRaw applied none of them.

Panasonic and Leica (`vendor::panasonic`): the visible area is the full
height and the columns up to the right sensor border plus the left border's
width; the default crop is the camera's crop tags (else the sensor borders)
starting on even rows and columns; black is the black level tags, 15 above
them for raw formats 3 and 4; white is the bit depth's range over the green
white balance level (tag 0x25) by 256, for raw formats 6 to 8 below 16 bits
the linearity limit (green's; else 1/256 below the range): Adobe DNG
Converter's WhiteLevel, exactly on every file of a green level above 256. Raw format 6 is read 15 below its coded levels, and raw format 7
in whole groups of four rows (the rows past them are 0). Raw format 8 strips
stacked in more than one row (a high-resolution DC-S1RM2 mode) are
`Unsupported`. Where Adobe's WhiteLevel is
not the file's rule (the DMC-G2's and G10's 3900, the DMC-GH2's 3800, the
16380 of the DC-S1, S1H, S1R, S1M2, S1RM2 at 14 bits and S5, and the
DC-GH5M2's 4088), `adobe-dng` rows of `data/raw-cameras.toml` give it, on
RAWmakase's scale (the DMC-L10's data Adobe keeps 15 lower).
The DC-GH5S's and DMC-FZ2000's Adobe takes from the ISO speed: rows of `iso`
give its 8000 at ISO 800 and 2095 at ISO 80, the only speeds of theirs seen. Bodies
whose files RAWmakase has always read with other areas have
`compat-observed` rows (`visible`).

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
- Visible areas and some model names RAWmakase has always read are rows of
  `data/raw-cameras.toml` (`compat-observed`).
- Levels follow Adobe DNG Converter: black is the raw directory's or the
  SR2SubIFD's levels, the oldest (0x7300) by filter position, rows first (the
  DSC-R1's start with green); white is the raw directory's WhiteLevel, but
  Adobe's 16300 for the older bodies (`adobe-dng` rows, whatever their tag
  says), the bit depth's range without one, 16383 for small RAWs (whose RGB
  sits 512 above the black) and Adobe's 32800 for the ILCE-7M5's compressed
  RAW 2 (its values on twice the 14-bit scale: rows of `bits = 15`). The
  ILCE-7RM4 and 7RM4A's files Adobe reads with 15000 at ISO 160 (rows of
  `iso`), with their tag's at 50 to 1250 otherwise.

Nikon, from our own measurements of LibRaw's output (on the dataset's files,
and on synthetic NEFs that probe one code word, key or tag at a time):

- Nikon writes no Huffman tables. The six (lossy, lossy after the split row,
  lossless; at 12 and 14 bits) were recovered from the decoded values and
  checked code word by code word on synthetic streams. The second lossy table
  drops low bits of large differences (a symbol's high nibble); the split row
  switches tables for linearization tables of version 0x44 0x20 and 0x44
  0x40, and the bit stream runs on without alignment. Predictions are
  clamped to 14 bits before the curve.
- The curve is read as Adobe DNG Converter reads it (its LinearizationTable,
  equal on every entry for all our lossy files but three): 0x44 0x20 places
  point i at i·m / 255 (m = 2^bits − 1, so the first 256 points span every
  value), 0x44 0x40 at i·m / 1024, linearly interpolated and rounded half up;
  0x44 0x10 stores the curve's start; past the last point, its value. LibRaw
  spaced the points 2^bits / 256 apart (0.4% higher values). The D3, D300
  and D700's 12-bit lossy files Adobe reads with the standard curve of the
  first kind's files (683 points) instead of their own, which they do not
  hold: `data/raw-curves.toml` keeps it (`adobe-dng`, written by
  `scripts/cameras/raw-curves.py` from Adobe's DNGs). The D1X keeps its table (version 0x44 0x58) in tag
  0x008c, its predictions 2110 bytes on.
- The maker note's encrypted blocks (ColorBalance 02xx, LensData 02xx and
  08xx) are XORed after their version with a running sum: a step from the
  serial number's low byte, a start from the XOR of the shutter count's
  bytes, both through tables measured over all 256 values; the sum grows by
  the step times a counter from 0x60. A serial number that is not all digits
  keys as 96; the D50's as 34. Lens data is read only alongside a shutter
  count.
- Levels: BlackLevel (0x003d) is at 14 bits and shifted to the data's bits;
  a Coolpix's ColorBalance (0x0097) of version 0500 or 0502 gives black at 16
  bits (at byte 54 or 56; Adobe's BlackLevel for the COOLPIX P7700, P7800,
  P330 and P340). White is Adobe DNG Converter's WhiteLevel per camera and bit
  depth (`adobe-dng` rows of `raw-cameras.toml`; on lossy data, on its
  curve's scale), else the data's full scale. The Nikon 1 J1, J2, J3 and
  AW1's 3300 is Adobe's at the ISO speeds it was seen to read them with (100,
  and 160 for the J3 and AW1; rows of `iso`): the J1 at ISO 200 reads 4095.
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
and orientation. Levels follow Adobe DNG Converter. Black is the raw image
directory's per filter position (0xF00A), the pattern starting at the
camera's crop (Adobe's active area: the GFX 100's rows of 62 and 63 start at
its crop's odd row), or the older bodies' record 0x4000's first green for
every channel (record 0x4000 holds green, red, second green, blue; Adobe's
BlackLevel is the first green's); white is the bit depth's range. Where
Adobe's levels are not these, `adobe-dng` rows of `data/raw-cameras.toml`
give them: the older FinePix bodies' white (15564 to 15880, the HS10's 3900,
the S200EXR's 16250, the X100's 4000), the first X-Trans and X-A bodies' and
the X20, X30, XQ1 and XQ2's 12-bit 4094 white and black one above the
file's, and the 14-bit X-Trans II bodies' black of 1024 (a row's black
replaces the file's pattern). The EXR sensors' data Adobe preprocesses (its
levels and linearity differ from the file's); they keep the file's rules.
As-shot white balance is
the raw image directory's (0xF00E) or record 0x2FF0; D65 the illuminant list's
(0xF00C, light source 21) or record 0x2301, and fine weather and shade the
RAFData table's (the first run of six presets of the as-shot green, after an
as-shot entry in the first X bodies'), the X-A bodies' RAFData list (light
sources 1, 8 and 83, red and blue doubled) or records 0x2100 and 0x2200.
DevelopmentDynamicRange and RawExposureBias come from the maker notes and
record 0x9650. Visible areas, orientations and crops RAWmakase has always
read differently have `compat-observed` rows in `data/raw-cameras.toml`.
Lossy compressed data decodes exactly as LibRaw's. The older bodies whose
record 0x8200 is set (FinePix F700, S5000, S7000, S2Pro and the GX680 back)
have their as-shot white balance and presets read with red and blue scaled
down by factors the file does not give (`wb_scale` rows): 1/1.11 and
1/1.13, which give Adobe's AsShotNeutral on all four FinePix bodies'
files, kept whole; the GX680 back's, which Adobe does not read, are
LibRaw's (0.761 and 0.857, rounded down to whole levels).

Olympus and OM Digital (`vendor::olympus`): white balance (red and blue
over the mean of the two greens' levels, as Adobe DNG Converter reads it),
presets, black, bit depth, crop, aspect ratio and (for a camera with no
matrix row, see *Colour matrices*) the colour matrix come from the maker note's
ImageProcessing directory (RawInfo on older compacts; the main directory's red and blue
balance first where it has them). The default crop and the aspect-ratio crop
(centred in it, at the ratio the note names) start on even rows and columns.
Black is on the 12-bit scale and scaled to 14-bit samples; white is the bit
depth's range. The compressed coding was written from prior general
knowledge of the format (no decoder source consulted), then verified and
corrected by measurement against LibRaw's output; that 14-bit data's codes
start a byte later is what the measurement found. Visible widths shorter
than the stored rows are a rule by
stored width (our measurements, every body of a sensor alike) or
`compat-observed` rows; so are the
canonical names (`E-M1MarkIII` is `E-M1 Mark III`, OM Digital's bodies are
Olympus's). 14-bit compressed data starts a byte later than 12-bit.

Canon CR2 (`container::cr2`, `vendor::canon`): the sensor's size is
SensorInfo's, the strip's samples fill the slices top to bottom, left to
right, and CR2CFAPattern names the filter at the sensor's corner. The default
crop is SensorInfo's borders; the aspect crop is AspectInfo's where it is
smaller, else the default crop, starting on even rows and columns for a
mosaic. The visible area RAWmakase has always read is a `compat-observed` row
per body and image size. Levels follow Adobe DNG Converter's: black is
ColorData's PerChannelBlackLevel, as Adobe takes it, unless the masked border
reads more than 16 below it (the EOS 1000D records it on another scale); else
the border's, measured on the masked columns beside the image over the
visible rows, each channel's mean to the nearest: on the left without the two
outermost columns and the two next to the image, on the right every column
when there are three or more. White is Adobe's: the body's `adobe-dng` row
for the file's ISO speed where Adobe has a white of its own (a speed, or a
range; [1, 409600] for every speed); else ColorData's SpecularWhiteLevel
(NormalWhiteLevel for the bodies whose row says `normal_white`, the EOS
750D and 760D), at most the row's `white_max` (15000 for the bodies Adobe
caps), where the ColorData version records one for the data and it is below
the samples' maximum; else the body's row without an ISO (Adobe's white for
files that record none); else the bit depth's range. Two dataset files
remain off Adobe: an EOS 30D at ISO 100 (Adobe 4014 where another 30D file
at ISO 100 has 3398) and an EOS 5D Mark III at ISO 320 (13100 where two
others have 13000); Adobe's choice there depends on something the file does
not show. The maker notes give the as-shot
white balance and the Daylight and Shade presets (ColorData, by its version;
the PowerShot G9's WBInfo), Highlight Tone Priority (LightingOpt, else
CustomFunctions2), the lens (LensModel, with a space after the mount, when
EXIF has none), and the orientation in CameraInfo when IFD0's is normal.
Regional names map to the body (`[[name]]` rows). The EOS-1D Mark III's image
starts two samples into its strip.

Canon EOS-1D and 1Ds TIF (`container::cr2`, `Variant::CanonTif`): a TIFF
of thumbnails whose Canon maker note's RawDataOffset (0x0081) starts a
lossless JPEG stream that runs to the end of the file, of two components,
each of its lines two rows of the image (the frame's width the image's, its
lines half its rows; our measurement). The filter is green and red on the
first row; the as-shot white balance the first three words of
WhiteBalanceTable (0x00A4: red, green, blue), Adobe DNG Converter's
AsShotNeutral; black 0 and white the body's `adobe-dng` row (3500 for the
1D, 3514 for the 1Ds), both Adobe's. The 1Ds's model text, "EOS-1DS", is
the EOS-1Ds (a `[[name]]` row).

Canon sRAW and mRAW: a component's samples of an MCU are predicted as one run
on a line of the component's own (not T.81's two-dimensional sampling). A
pixel takes its MCU's chroma, averaged (rounding up) with the MCU to the right
for the right column, with the MCU below for the bottom row, and both for the
bottom right; MCUs past the visible width repeat their own. RGB is Adobe DNG
Converter's: `Y + d`, `d` a fixed-point mix of twice the chroma less its zero,
in 8192ths, each product rounded down (ColorData 6, 7 and 9, zero 32766:
R 200 Cb + 22929 Cr, G −(5460 Cb + 11751 Cr), B 29040 Cb − 101 Cr; newer,
zero 32768: R 4096 Cr, G −778 Cb − 2048 Cr, B 4096 Cb); the 40D's luma is 512
above black. That RGB is white balanced by ColorData's small RAW levels
(words 78, 123 or 128 by version), Adobe's AnalogBalance; red and blue are
taken back to the mosaic's camera RGB, times their level over green's, so
the as-shot white balance and the matrices apply as to the mosaic. A fourth
channel is 0; black is 0 and white the body's `adobe-dng` row for its small
RAW size (Adobe's is the body's at every ISO), else 16383. An mRAW's chroma
interpolation stays RAWmakase's own: Adobe's, measured on the dataset's
mRAWs, is not one of the simple ones (each MCU's chroma at its top-left or
its centre, linear or nearest), and matches it on under a third of the
pixels whatever the siting.

Before RAWmakase's own decoders replace LibRaw for Canon small RAWs, two
things outside this crate have to follow: `rawmakase-model`'s
`sraw_baseline_exposure` corrects LibRaw's scale (green times 1.303, white
16383), which the samples here no longer have, so it becomes Adobe's small
RAW BaselineExposure alone; and Adobe clips each channel of a small RAW at
its white before taking AnalogBalance out, so red and blue clip at the white
times their level over green's, which one `white` cannot say (`linear_white`
could carry it; the develop pipeline does not read it yet).

Canon CRW (`container::ciff`, `codec::crw`): a CIFF heap of directory blocks;
SensorInfo (0x1031) gives the sensor and the default crop, DecoderTable
(0x1835) one of three pairs of JPEG-style code tables, and the raw data record
(0x2005) holds, for the 12-bit bodies, two low bits per sample first, then a
514-byte header and the byte-stuffed coded data: blocks of 64 differences, a
DC code accumulated from block to block and AC codes of a zero run and a size,
each sample the previous one of its colour plus its difference (the first two
of a row from 512). The tables, the layout and the low bits were measured by
decoding synthetic files with the oracle. The visible area is a
`compat-observed` row; black is measured as for CR2; white is the bit depth's
range (1023 or 4095), 4000 for the EOS bodies (`adobe-dng` rows). ImageInfo
gives the rotation, ShotInfo the ISO, aperture and exposure time, FocalLength
the focal length; the white balance is ColorBalance's (EOS bodies) or, on the
older PowerShots, ColorInfo2's as-shot and ColorInfo1's Daylight levels.

CHDK's headerless files (`container::headerless`) are `[[headerless]]` rows
by file size: the camera, the image size and bits, the visible area and the
filter. Samples are packed most significant bit first in 16-bit little-endian
words; black is measured as for CR2 but on every left border column. The
same rows know other headerless files RAWmakase has always read by size:
action cameras' 16-bit words (GITUP's sizes, which other makers' cameras
write too, with LibRaw's black levels; one size it has always read as a
Samsung WB550, 10-bit samples in the words' top bits) and the Pentax Optio
S4's 12-bit samples packed in bytes, rows padded to 3520 bytes. Such a row
may give the packing word (`word`), the row size (`row_bytes`), a `shift`
and the levels (`black`, `white`). Their as-shot white balance LibRaw
derived from its own matrices; without an Adobe matrix it is neutral here.

Phase One (`container::iiq`, `vendor::phase_one`, `codec::phase_one`): IIQ
files, the Phase One backs' TIFFs and Leaf's Credo IIQs hold Phase One's own
block in the EXIF maker note, eight bytes into the file: a directory of
16-byte entries giving the sensor size, the image area (an odd left or top
edge moved in by one, as RAWmakase has always shown it), RawFormat, the data
and each row's offset, BlackLevel, the as-shot white balance and the
orientation. The IIQ L coding was written from prior general knowledge of the
format (no decoder source consulted, no public description found) and
verified by an exact match with LibRaw's output on every IIQ L, L16 and
first IIQ S file of the dataset; IIQ S maps values below 256 through
v² / 3.969, rounded. Black is BlackLevel (0 without one) and white 65535,
Adobe DNG Converter's WhiteLevel for every IIQ; LibRaw subtracts its black on
development and reports 0 and 0xFFFC less it (approved decode deltas; the
black ones stay pending where Adobe writes calibrated data with a black of
0). Not applied: the per-row and per-column black offsets the block may
carry (BlackLevelData, up to a few hundred levels on some backs), which
`Sensor` cannot express, nor Phase One's sensor calibration (flat fields,
defects), which Adobe applies. The older backs' colour filter is a
`compat-observed` row (Adobe's CFAPattern agrees). The scrambled RAW 1 and
RAW 2 formats are `Unsupported`.

IIQ Sv2 (RawFormat 6, `IiqSv2`) was worked out black-box: LibRaw's output
for rows written to test each code and for random rows of the coding, then
an exact match on all nine IIQ Sv2 files of the dataset. Each row (from its
offset, rows in any order) is bits read most significant first from 32-bit
words: a word and a bit not read, then eight 14-bit values; then per whole
group of eight columns a length code for the even columns, one for the odd
and a width code, then the eight values. A length code is a run of zeros:
none then a bit (the run's last length, or one more), one (one less), two
to five then a bit (6 or 5, 4 or 7, 9 or 8, 3 or 10), six (11), seven (12:
14-bit values as they are); lengths below 1 or past 16 are corrupt. A width
code is 1 (3) or 0 and two bits (4 to 7). A value of length `n` and width
`w` is `w` bits `b`: the value two columns left plus `b << (n − w)` (nothing
when `w` is the greater), less `2^(n−1)`, plus 1, kept to 0 to 16383 (the
run goes on from the unkept value) and stored times 4; columns past the
last whole group are 14-bit values. LibRaw's results for widths more than
two over the length differ (no camera writes them). Black and white are as
for the other IIQ formats; Adobe DNG Converter 18.7 converts none of the
dataset's IIQ Sv2 files, so the black and the Credo's white stay pending
decode deltas.

Hasselblad (`container::fff`, `vendor::hasselblad`, `codec::hasselblad`):
3FR (the camera's) and FFF (Phocus's, with its private tag 0xC519) are TIFFs
whose SubIFD holds 16-bit words or Hasselblad's coding (compression 7):
lossless JPEG headers for the Huffman table and predictor (8, or 11, which
adds half the change two rows up), then per pair of columns two difference
lengths and the two differences read from little-endian 32-bit words, each
sample its colour's neighbour two columns left plus its difference (0x8000
for the first). The coding was written from prior general knowledge of the
format (no decoder source consulted) and verified by an exact match with
LibRaw's output on every compressed file of the dataset but one, an H4D-60
3FR whose every sample LibRaw raises by 256 (taking 256 as its black;
Adobe's black on RAWmakase's samples is 0: a pending delta). Black is the
file's BlackLevel and white 96% of its WhiteLevel, as Adobe DNG Converter
takes them (fitted to RAWmakase's samples on every body measured). The
camera is the model without what follows a "/", looked up with the sensor's
megapixels appended for the older bodies' names (`[[name]]` rows); its
colour is named from the maker note's sensor code (0x0011) and coating
(0x0016), as RAWmakase has always named it ("50-15-Coated5-3FR"). Visible
areas are `compat-observed` rows. Multi-shot files are `Unsupported`.

Leaf MOS (`container::mos`, `vendor::leaf`): IFD0 is the raw image, one
tile, and its tag 0x8606 Leaf's data: nested packets ("PKTS", a version, a
40-byte name, a big-endian size) of text values. The back type
(ShootObj_back_type) names the back through `[[name]]` rows, NeutObj_neutrals
give the white balance (the first level over each channel's),
ImgProf_rotation_angle the orientation, CaptProf_mosaic_pattern the filter
(1 red, 0 blue, 2 and 3 green, row by row), JPEG_preview_data the preview,
and Rows_data 1 says the coded rows are the image's even rows, then its odd
rows from the bottom up (our measurement). The lossless JPEG's scan numbers
its components from one where the frame numbers them from zero, so a scan
naming none of the frame's components takes them in order. Black is 0;
white is the camera table's (`adobe-dng`: 15000 for the Aptus 22, Adobe
DNG Converter's), else 16383, what RAWmakase has always read. Files of
several planes are `Unsupported`.

Sinar STI (`container::sti`): CaptureShop's TIFF of a Sinarback, IFD0 a
thumbnail with Sinar's settings as text tags, the next directory one tile
of 16-bit words with its CFAPattern and orientation. The as-shot white
balance is the Neutral line of the last settings tag (0xC517, else 0xC516),
as RAWmakase has always read it; black 0 and white 16383 are what it has
always read (Adobe DNG Converter does not read the format). The ISO speed
is IFD0's ISOSpeedRatings, which LibRaw does not read (a pending delta).

Raspberry Pi (`container::brcm`): raspistill's and picamera's raw files are
a JPEG (the preview, its EXIF the exposure, its maker note the ISP's
settings as text, gain_r and gain_b the as-shot white balance) followed by
"BRCM" (found after the JPEG's end of image, its segments walked; up to 16
bytes after it, one in the dataset's files) and a 32768-byte header (at 0x10 the sensor's name, at 0xA0 the row
size, at 0xD0 the width and height, at 0xF6 the bits), then padded rows of
10-bit samples packed four in five bytes: the four samples' high eight
bits, then a byte of their low two bits, the first sample's least
significant (our measurements). The camera is named "RPi" and the sensor
("Broadcom" its maker, as RAWmakase has always named it); the filter and
levels are a `compat-observed` row per sensor (the IMX219's), and a sensor
without one is `Unsupported`, as are other bit depths. A block whose sizes
are zero or whose rows are not all in the file is none, and a JPEG without
the block is not RAW, whatever its size (as are PNG, GIF, RIFF, PDF and ZIP
files: none falls through to the headerless sizes). Adobe DNG Converter
reads only the JPEG.

Minolta MRW (`container::mrw`): "\0MRM" and the size of its big-endian
blocks: PRD (sensor and image size, RawDepth, BitDepth, StorageMethod 0x59
for 12-bit samples packed most significant bit first or 0x52 for 16-bit
words, BayerPattern 1 for RGGB or 4 for GBRG), WBG (the as-shot levels in
the filter's order), RIF (the daylight red and blue levels, 12 bytes in,
over a green of 256) and TTW (a TIFF of the EXIF, whose maker note's
PreviewImageStart and Length give the preview, its first byte written
as another than 0xFF and read as 0xFF, as Epson's); the sensor data follows
the blocks. Exposure times and apertures the EXIF gives only as APEX values
are read from those. Black is 0; white is Adobe DNG Converter's (`adobe-dng`
rows: 3965 for the DiMAGE bodies), else the samples' 12-bit range, Adobe's
for the Dynax and Alpha bodies (approved deltas; Adobe's levels and white
balance match RAWmakase's samples exactly on every body measured). The
bodies sold under several names have `[[canonical]]` rows (DG-7D, DG-5D).
The Kodak C603's (and C643's) raw files are `[[headerless]]` rows of 8-bit
codes, each taken through a curve whose top is the white. The "CFAI" file
has a 3160-byte header ("KodakCommonInfo") keeping that curve, 256
little-endian 16-bit words at byte 168 (`offset`, `curve_at`; our
measurement: the values LibRaw gives every code). The "CCDI" file is the
codes alone; its curve, in the row (`curve`), was measured from LibRaw's
output, every code on each of the dataset's three files, as no public
source gives it (`compat-observed`). The "YRGB" files hold no mosaic and
are not read; Adobe DNG Converter reads none of them.

The DiMAGE Z2's MRW has no header: a `[[headerless]]` row, its samples in
little-endian 32-bit words stored as two fields as Olympus's compacts store
them (`packed::fields`).

Kodak DCS TIFFs (`container::dcs`: the DCS520C and DCS560C, Canon's EOS
D2000C): IFD0 a thumbnail, its SubIFD one lossless JPEG strip whose MCU rows
are the image's rows, and the KodakIFD (tag 0x8290). The 12-bit codes are
taken through the KodakIFD's ImageExpansionTable (0x090D; else the strip's
GrayResponseCurve), as LibRaw takes them, which on the DCS560C is not the
GrayResponseCurve Adobe DNG Converter linearizes with. Black is the mean of
BlackLevelTop and BlackLevelBottom (0x03EF, 0x03F0) rounding up, and white
4095 or the camera table's (`adobe-dng`: 3277 for the DCS560C), both Adobe's
fitted to RAWmakase's samples; the as-shot white balance is the levels of
the WhiteBalance setting (WB_RGBLevelsDaylight, 0x0848, plus the setting
0x03FC), green over each, as Adobe's AsShotNeutral; the Daylight preset those
levels' green squared over each. SensorLeftBorder to SensorImageHeight
(0x03EB to 0x03EE) are the default crop, ExposureIndex the ISO speed where
EXIF gives none. Kodak's DCR files (the DCS760C and the DCS Pro bodies) are
the same TIFFs, their strip in Kodak's coding (`codec::kodak`, compression
65000: each row in blocks of up to 256 samples, a 4-bit length per sample
then its difference in that many bits, summed per colour; a block of a
length over 12 stored as plain 12-bit samples) and their ImageExpansionTable
up to 4096 entries (1024 of 10-bit codes on the 14N, 14nx and SLR/c). White
is the camera table's (`adobe-dng`: 3500 for the DCS760C, 3700 for the 14N,
14nx and SLR/c), else 4095; the previews, in Kodak's coding, are not JPEG.
Kodak's older DCS TIFFs (the DCS460 and the EOS DCS 1 and 3)
keep 8-bit codes in their largest uncompressed 8-bit directory, each taken
through IFD0's 256-entry GrayResponseCurve, whose top is the white; black is
0, the model the model text's first word, and the visible area (two columns
in from the left), filter and canonical names are `compat-observed` rows.
Adobe DNG Converter measures a fractional black of its own (7.2, 19.1 and
0.1 on the dataset's files), which RAWmakase does not follow.

Kodak KDC of the EasyShare and P-series compacts (`container::kdc`): the
KDC IFD (IFD0 tag 0xFE00) gives the raw size (0xFA13, 0xFA14; an odd height
is read a row taller, as RAWmakase always has), the filter as text (0xFA15;
`compat-observed` rows for the Z990 and Z981, whose text is not their filter
and whose filter Adobe DNG Converter's CFAPattern agrees with), the camera's
image (0xFA3E, 0xFA3F, 0xFA31, 0xFA32, starting on even rows and columns,
the default crop) and the white balance levels in 65536ths: the WhiteBalance
setting's (0xFA0D: 0 Auto, 0xFA25; 3 Daylight, 0xFA29; 6 Shade, 0xFA2A), else
the as-shot ones (0xFA12), as Adobe's AsShotNeutral; Daylight and Shade are
the presets. The data, 12-bit samples packed most significant bit first,
starts 64 bytes after the first of the 16-row strips a SubIFD's tag 0xFD04
lists from its seventeenth value on (our measurements). White is Adobe's
(`adobe-dng` rows for the P712 and Z990), else 4095; black 0.

Canon files whose values differ from LibRaw's on purpose are approved in
`tests/corpus/decode-deltas.toml`, with Adobe DNG Converter's value as the
evidence: the levels above; the sRAW and mRAW samples (Adobe's conversion);
three CRW files (PowerShot G5, S50, S60) whose samples LibRaw raises by 2
below 512, where Adobe's equal RAWmakase's; and multiple exposures, whose
as-shot white balance LibRaw sets to 1. Still open: the as-shot white balance of nine
PowerShot and EOS D30 CRW files and the Daylight preset of five, which LibRaw
reads from records whose layout is not known here; Adobe writes no levels
for them, only a chromaticity for three (AsShotWhiteXY, matching the
camera's white balance setting, not a record of the file).

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

Pentax (`vendor::pentax`): white balance, presets (WBLevels' records where
the note has them), black, the ISO code, the lens ID (LensType's two bytes as
one number) and the default crop (ImageAreaOffset and RawImageSize, starting
on even rows and columns, in sensor coordinates) come from the maker note
(its Huffman coding written from prior general knowledge of the format, no
decoder source consulted, and verified by an exact match with LibRaw's output
on every PEF of our dataset);
Pentax DNGs' lens ID and ISO from the same note in DNGPrivateData. Samsung's
GX PEFs read the note too, but not its crop. Ricoh Imaging's bodies are
named Pentax. The visible areas and the canonical
names (`*ist DL` is `*istDL`) are `compat-observed` rows. A PEF of linear
(monochrome) data is `Unsupported`.

Samsung (`vendor::samsung`): white balance, the D65 preset, black and white
come from the maker note's vectors, less (or plus) its EncryptionKey.
The three compressed codings and the fixed Huffman table of compression
32772 were written from prior general knowledge of the formats (no decoder
source consulted), then verified and corrected by measurement against
LibRaw's output on our files; the measurement found that tiles' samples are
kept within the bit depth, and the packing rules below.
12-bit words (compression 32769) hold 11-bit samples. The NX20 generation
(whose raw directory says three samples a pixel) packs pairs into
little-endian 3-byte words, earlier bodies plainly. Visible areas, the
EX2F's white level (2047 of its 11-bit samples: Adobe's 4095 of 12 bits) and,
for bodies whose CFAPattern is not the
filter (the NX300 and NX3000 generations, the NX mini; Adobe DNG Converter
agrees), the colour filter are `compat-observed` rows.

Epson (`vendor::epson`): black, the camera's image size (centred: the
default crop), the embedded JPEG (whose first byte, 0xEE, is mended to
0xFF) and white balance as shot, the red and blue levels in tag 0x0E80
scaled as Adobe DNG Converter's AsShotNeutral. Mamiya's MEF has no white
balance RAWmakase reads.

Values observed from LibRaw other than geometry and names are checked
against the file and Adobe DNG Converter: where both disagree with LibRaw,
the file's value is read and the change recorded in
`tests/corpus/decode-deltas.toml`. White levels the file does not give
are Adobe DNG Converter's (`adobe-dng` rows, in the files' sample units, by
bit depth where a body writes two), as are the 14-bit Olympus bodies' black
(the note's 0x0606) and the white balance scale of the SP-350, SP-500UZ,
SP-510UZ and SP-550UZ, whose red and blue levels are over a green Adobe
reads about 4% higher. An `adobe-dng` row may give
decode facts only (black and white levels, the as-shot white balance scale),
never geometry (visible area, default crop, orientation, colour filter), which
stays the file's or `compat-observed`; the facts test enforces this. Adobe's
geometry is a table of its own, read only in Adobe's frame (see
[Frames](#frames)).

## Frames

A photo's frame is the default crop of its decoded image: what Develop shows
uncropped, and what an edit's positions (crop, spots, red eye, masks, Upright's
guides) are normalised to (`model::image_frame`). RAWmakase has always decoded
in LibRaw's frame (its visible area and default crop, with RAWmakase's rules on
them); Lightroom's crops and local adjustments are relative to Adobe's, the
DNG DefaultCrop that Adobe DNG Converter writes for the camera, which differs
by a few pixels on many cameras (some, as Nikon's, have no default crop in
LibRaw's frame at all).

`rawmakase-raw` knows Adobe's frame for a file (`RawInfo::adobe_frame`): a
DNG's own ActiveArea and DefaultCrop, exactly, or for other files the row of
`data/adobe-frames.toml` for the camera and raw size (`active` and
`default_crop`, sensor coordinates, `source = "adobe-dng"`). The table is
written by `scripts/cameras/adobe-frames.py`, which converts a file per camera
and raw size with Adobe DNG Converter and places the DNG's areas in the RAW's
sensor coordinates, by size or by aligning its samples with the decoder's
(`rawmakase fingerprint --sensor`). `in_adobe_frame` reads the file in that
frame: Adobe's crop as the default crop, and Adobe's active area as the
visible area only where the crop does not lie in the one RAWmakase has always
read.

Adobe's frame is the camera's, not the shot's. On some cameras Adobe DNG
Converter makes the aspect ratio chosen in the camera the DefaultCrop (a
PowerShot G12 or E-P2 shot at 3:2 of a 4:3 sensor, an A580 or WB2000 shot at
16:9), which would give every file of that camera and raw size the shot's
frame. So the table has no row whose crop differs in shape by more than 3%
from both Adobe's active area and RAWmakase's frame (where the raw size gives
the ratio, as on Panasonic's multi-aspect sensors, RAWmakase's frame has it
too), and `in_adobe_frame` keeps the frame RAWmakase has always read for a
file that records an in-camera aspect-ratio crop of another shape than its
frame, or whose frame is itself one (Adobe's crop is of another shape, as the
whole sensor around a DC-GH6 high-resolution square shot is). Such files
open as they always have: the camera's aspect ratio as the frame or as the
starting crop.

Which frame a photo is decoded in travels with the decoder
(`raw::Decoder::frame`): LibRaw's photos are in the legacy frame, and so are
RAWmakase's own decoders' until they replace LibRaw (`OWN_DECODER_FRAME`).

The switch to Adobe's frame comes with the switch to RAWmakase's decoders:

- `OWN_DECODER_FRAME` becomes `Frame::Adobe`, and the frame and visible area
  changes are approved decode deltas (Adobe DNG Converter's DefaultCrop as the
  evidence);
- saved edits keep their numbers. Their positions are relative to the frame,
  so on a camera whose frame moves, an edit's crop, spots, red eye, masks and
  Upright guides land on pixels shifted by the frame's difference: by the
  left or top difference at that side, the right or bottom one at the other,
  in between across the photo (the per-make sizes are below). This is a
  one-time shift, said in the release notes; no catalog format change goes
  with it;
- previews made before the switch are made again, so no thumbnail shows the
  legacy frame;
- the cameras whose visible area moves (Adobe's crop reaches past the legacy
  visible area) are checked for levels measured in the masked borders beside
  it, and lens corrections centred on the decoded image.

Measured with Adobe DNG Converter 18.7 on one file per camera and raw size of
the test dataset (2026-10-17): of 1,245 files Adobe converts (34 more it does
not, CHDK's among them), 1,235 are placed in sensor coordinates. Of the
non-DNG files whose camera and raw size have a row (981 rows; a DNG gives its
own frame), Adobe's frame is RAWmakase's on 701 and differs on 279. The
median difference a side, where the frame differs, is 1 pixel for Canon,
Fujifilm, Kodak and Leica, 2 for Olympus and Panasonic, 3 for Sony and Phase
One, 4 for Minolta and Mamiya, 5 for OM Digital, 7 for Epson, 8 for Nikon and
Hasselblad, 10 for Pentax and 20 for Samsung; the largest is 84 pixels (a
Samsung NX10 side). 22 of Adobe's crops reach past the visible area. With no
row, and so the legacy frame: the 9 Super CCDs (whose image is turned), a
Nikon D2H file (no offset clearly the best), 7 files whose pixels are not
square (Fujifilm's S-series and the Nikon D1X), 2 whose areas fall outside
the raw data (Fujifilm X-T5 and X-M5) and the 4 whose crop is the shot's
aspect ratio (PowerShot G12, E-P2, A580, WB2000); the DC-GH6 high-resolution
square shot keeps its frame by the rule above.

## Colour matrices

A camera's XYZ (D65) to camera matrix is Adobe's, from `data/raw-matrices.toml`
(written by `scripts/cameras/raw-matrices.py`, whose docstring has the
commands): the ColorMatrix Adobe DNG Converter writes for the camera
(`adobe-dng`). Only a camera the converter has no matrix for keeps the one
LibRaw reported (`compat-observed`). No matrix comes from Adobe's camera
profiles. A camera known only from its own DNGs mostly has no row: the
converter keeps each file's matrix (rescaled), and so does the decoder.
Where Adobe's matrix is not LibRaw's, `cam_xyz`, `matrix` and `daylight_wb`
differ from the recorded fingerprints on purpose.

A camera with no row uses the matrix its file carries, where it has one.
Olympus's SP-350, SP-500UZ, SP-510UZ, SP-550UZ and SP-570UZ, which the
converter gives only their own values (unscaled, so no usable matrix), use
the maker note's ColorMatrix (ImageProcessing's or RawInfo's 0x0200, else
the main directory's 0x1011; ExifTool's Olympus tag names): camera to sRGB
(our assumption: the compacts' only output space) of white balanced data,
in 1/256. Its rows are scaled to sum to one, and the matrix is turned into
an XYZ (D65) to camera one through the camera's own D65 white balance
levels (RawInfo's daylight fluorescent, 6500 K, levels 0x0130;
ImageProcessing's 6600 K levels 0x010C), on the as-shot balance's scale,
which become the daylight multipliers.

The Leaf Aptus 75, which the converter does not read, has no matrix: its
MOS ColorMatrix is to Leaf's own working space, before an input profile,
not a colour space RAWmakase can name. A camera without a matrix has
neutral daylight multipliers and the identity camera-to-sRGB matrix, as
LibRaw gives it: the camera's colours pass through unconverted.

Known: a DNG without a D65 matrix is read with its ColorMatrix1, which for
GoPro's GPRs (illuminants Standard A and D50) and Leica's Q and SL DNGs is
the tungsten one, not the D50 one Camera Raw would use at D65.

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
