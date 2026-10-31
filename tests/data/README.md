# Lightroom curve references

`lightroom-point-curves.json` contains numeric samples from **generated ramps**, not user photographs. The fixtures contain no private image data and no Adobe source code.

Reproduce inputs with `python scripts/make-curve-fixtures.py NEW_DIRECTORY` (numpy, Pillow, tifffile). There are seven 1024-pixel ramps, each repeated for 32 rows: neutral, single-channel ramps with other channels at 0.25, and paired-channel ramps with one channel fixed at 0.5. All inputs are 16-bit sRGB TIFF with embedded XMP settings.

Reference exports were made on 2026-09-26 with Lightroom Classic 15.5.1 in a disposable copied catalog, after **Read Metadata from Files**. Export: full-size 16-bit sRGB TIFF, no sharpening/watermark. Curve Refine Saturation is 100; all other adjustment controls are zero. No RAW demosaicing or camera profile is involved.

The JSON has 224 rows: sample the center of each strip (`y = 16, 48, …, 208`), then `x = 0, 32, …, 992`. Each row has five RGB16 triplets: Lightroom linear baseline, S master, independent RGB, clipped master, clipped/nonmonotonic RGB. Curve points are defined in the generator and the regression test. Using Lightroom's linear export as input isolates curve behavior from its small ICC round-trip error.

The Rust test checks both RGB cases over all strips and master curves on the neutral strip. Mean absolute RGB error is bounded to 0.0001 (0–1 scale); neutral master error to 0.00003. The looser peak bound accommodates dark clipped RGB values and ICC quantization. **This does not establish full parity:** saturated master-curve colors still differ (whole-ramp MAE 0.00240 for the S curve and 0.00333 for the clipped curve). Parametric curves and non-default Refine Saturation are not covered by these goldens. The latter remains reported as unsupported during XMP import.

# Rendered-format fixtures (`formats/`)

Tiny synthetic WebP, AVIF and JPEG XL files for the decoder and metadata tests, all drawn by
`python3 scripts/make-format-fixtures.py tests/data/formats` (Pillow with WebP, `avifenc`,
`cjxl`; the script prints the encoder versions it ran with, which go in the commit
message). No camera data or photos.
`corners.png` is the 16×12 source: red, green, blue and white 4×4 corners
(top-left, top-right, bottom-left, bottom-right) on grey. Metadata fixtures carry Make
"Fixture", Model "Synthetic 1", DateTimeOriginal 2024:05:04 10:21:33 and the XMP title
"Fixture title". Orientation fixtures: AVIF `irot`/`imir`, WebP and JXL EXIF Orientation 6.
`pq.*` use the PQ transfer (HDR), `animated.*` have two frames, `tiled.avif` is a 2×2 grid of 64×64 cells (MIAF forbids smaller tiles). `cropped.avif` has an essential clean aperture (`clap`), which RAWmakase reads (a HEIC's is applied) but does not apply to an AVIF, so it refuses the file. `split-exif.avif` is `metadata.avif` with its Exif item stored in two `iloc` extents.

The HEIC fixtures are drawn by `python3 scripts/make-heic-fixtures.py tests/data/formats` (Pillow and
`pillow-heif`, whose libheif and x265 versions the script prints for the commit message), from the
same `corners.png` drawing. `metadata.heic` (Exif and XMP as above), `ten-bit.heic`, `p3.heic`
(CICP P3), `pq.heic` (10-bit PQ) and `identity.heic` (lossless 4:4:4 through the identity matrix,
for RAWmakase's own decoder) are libheif's own files, each a 64×64 picture it crops (`clap`) to
16×12. All but `identity.heic` are 4:2:0, as phones write; `wide.heic` is a 192×128 picture,
coded larger than one 64×64 block, for the decoder's size limit. The script writes the others around x265's HEVC: `rotated.heic` and
`rotated-mirrored.heic` add `irot` 1 and `imir` 0 as their AVIF namesakes do, and `grid.heic` is a
2×2 grid of 64×64 tiles cut to 120×100, laid out as an iPhone writes one: hidden tiles carrying the
`nclx` (BT.709, limited range), an sRGB ICC profile on the grid, the grid's description in `idat`.
