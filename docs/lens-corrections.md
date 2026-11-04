# Lens corrections

## Built-in (camera-embedded) corrections

Many cameras store per-shot lens corrections in the RAW file. Lightroom applies them as the "built-in lens profile" without any Adobe profile. RAWmakase reads them in `crates/rawmakase-decode/src/raw/lens.rs` when a file is opened (`Metadata::lens`) and applies them while rendering, so no user import is needed.

| Camera | Tables read | Default |
|---|---|---|
| Fujifilm RAF | FujiIFD 0xF00B distortion, 0xF00F lateral CA, 0xF010 vignetting | On |
| Sony ARW | raw SubIFD 0x7032 vignetting, 0x7035 lateral CA, 0x7037 distortion | Lateral CA on (and a compact's distortion); the rest with Enable Profile Corrections ([below](#sony-built-in-corrections)) |
| Olympus and OM System ORF | maker note ImageProcessing 0x150A distortion, 0x150C lateral CA | On ([below](#olympus-om-system-and-panasonic)) |
| Panasonic RW2 (and Leica RWL) | IFD0 0x0119 distortion, 0x011B lateral CA | On ([below](#olympus-om-system-and-panasonic)) |
| Nikon NEF (Z bodies) | raw SubIFD 0xC7D5 (NEFInfo): distortion, vignetting, lateral CA | On for lenses that need it; others follow Enable Profile Corrections ([below](#nikon)) |
| Nikon Coolpix NRW (P950, P1000, P1100) | raw SubIFD 0xC7D5 (NEFInfo): distortion and lateral CA tables | On ([below](#nikon-coolpix)) |
| Samsung SRW (NX200 and later, NX mini, Galaxy NX, EX2F) | maker note 0xA050 distortion, 0xA051 lateral CA | On ([below](#samsung)) |
| Canon CR3 (EOS R5 Mark II, R6 Mark III) | timed metadata (CTMD) record 14: vignetting, lateral CA, distortion | Off; follows Enable Profile Corrections ([below](#canon)) |

Each correction is radial about its optical centre (`LensCorrection::centre`, in fractions of the decoded image's width and height): where the camera's data or Adobe's own output places it, else the decoded image's centre. Radius 1 is the decoded image's half diagonal, so an off-centre correction's tables run out to its farthest corner (`LensCorrection::reach`). Every place that evaluates the correction uses that centre: CPU and GPU sampling, vignetting, GainMaps applied after the warp, the fill scale, Remove Chromatic Aberration's measurement, and the mapping of local adjustments, retouching, Upright and auto analysis through the lens. Corrections saved before the centre existed read as centred. Where each reader takes its centre from:

| Source | Optical centre | Radius unit of its model |
|---|---|---|
| Nikon Z (NEFInfo) | Record 8, in pixels from the decoded image's centre, as Adobe's opcodes and profiles have it | The default crop's corner farthest from it |
| Olympus, OM System, Panasonic, Samsung | The default crop's centre, as Adobe's warps have it (Olympus decodes more unused columns on one side: 8 px on the E-M10 Mark III; Samsung's crop is up to 20 px off its active area's centre) | The default crop's half diagonal (Olympus), the stored unit (Panasonic), pixels (Samsung) |
| DNG opcodes | The opcode's (cx, cy), fractions of the active area | The active area's corner farthest from it, as the DNG SDK normalizes it |
| Adobe LCP profiles | PerspectiveModel's ImageXCenter and ImageYCenter, fractions of the default crop, interpolated in focal length | FocalLengthX × the default crop's long edge |
| Fujifilm, Sony, Coolpix, Canon | The decoded image's centre: no centre is recorded, and Adobe's warps of them are centred (Canon R5 Mark II, R6 Mark III: within 1 px) | The decoded image's half diagonal (Coolpix: the default crop's) |

Rendering order:

1. Highlight reconstruction in camera space.
2. Vignetting gain in linear camera space, evaluated at the source pixel. Shadows, Highlights, Clarity and Texture measure local luminance after this gain.
3. Distortion and lateral CA as a per-channel radial remap while sampling, zoomed about the optical centre only as far as green's samples for the photo's edge need to stay inside the decoded image ([framing](#framing)). Each sample carries its decoded green-channel position to local tone and Texture maps, which are measured before the geometric remap. CPU and GPU sampling use this same coordinate; the pre-warp position would displace local contrast from the edge that produced it.

The recipe field `lens_builtin` controls it. New recipes enable it when the file's correction is marked `default_on` (Fujifilm, Olympus/OM System, Panasonic, Samsung, DNG, Sony's lateral CA and Nikon lenses that need it; not Canon's or Nikon's optional ones). Recipes saved before the field existed lack it and never apply it.

### Framing

Adobe's warps bring their own framing: k₀, green's ratio at the centre, which for Nikon is the camera's own fill scale, keeps the edge of the image inside without any further zoom, and Lightroom adds none. RAWmakase zooms (`LensCorrection::fill_scale`, chosen in `LensMap::fill`) only where green's sample for some point of the photo's edge, the camera's default crop (for Sony's built-in correction, the camera's aspect-ratio crop: [below](#sony-built-in-corrections)), would fall outside the decoded image, and then just enough. The interior cannot leave the image when the edge does not: along each ray from the optical centre the corrected radius grows with the output radius, so a point inside samples nearer the centre than the edge does. A correction that folds back keeps every radius out to the farthest corner inside instead. Only green frames the photo, as in Adobe's warps, whose red and blue planes may reach a little past the image (the P1100's red plane is 1.0009 at the corner); the samplers clamp those few samples at the border. Fujifilm's distortion table carries no fill scale; Adobe's warps of it are scaled so the widest ratio is 1, its corner on the corner (X-Pro2: k₀ 0.99042 against 1 / 1.0098), and the reader scales it the same way, as the Coolpix reader does. Against Adobe's warps that brings the X-Pro2 from 0.88 to 0.25 px, the X-T20 (XC 15-45mm) from 11.8 to 1.7 px, the X-A7 from 20.5 to 3.3 px, the X-T200 from 5.5 to 0.7 px. Newer bodies (X-S10, X-T30 III, X-H2S) get a "Camera Settings" profile instead, not compared here.

Before 2026-10-25 the fill kept the largest ratio of any channel at any radius out to the decoded image's half diagonal at 1. That zoomed in where Adobe does not: 1.5% on a Z5 II photo in DX crop (24-70mm f/4 S at 24mm, ratio 1.015 at the centre and 0.97 at the corner; 33 px off Adobe's warp at its edge, now 0.02 px), and by red's CA on the P1100 (up to 2.6 px at the corner) and on lateral-CA-only corrections (Sony's E-mount warps).

### Reconstructed X-Trans fine detail

Demosaicing can reconstruct nearly constant chromaticity on stripes whose colour
is finer than the CFA resolves ([demosaicing](demosaic.md)). Independently shifting
red and blue then samples different slat brightnesses and creates colour again,
even from a uniformly coloured input.

The CPU lens sampler and its GPU counterpart protect that case on X-Trans images.
They compare source chromaticity at the three channel sample positions with the
chromaticity introduced by assembling the remapped channels. When the source
variation is less than 40% of that introduced colour (full protection below 10%),
and a seven-pixel horizontal or vertical neighbourhood contains at least two
significant luminance slope reversals with coherent chromaticity, the sample moves towards the original
chromaticity at green's position. It keeps the remapped sample's luminance,
distortion and vignetting. A single monotonic edge does not meet the repeated-detail
condition; existing chromaticity variation also rejects protection. Two reversals cover periods through four pixels at every phase. CPU and GPU choose the window centre with ties-to-even rounding, including at half-size sampling positions. Coherence is
full below a 2% chromaticity span and fades to zero at 5%; checking the whole
line prevents chance agreement between two interpolated noise samples from
suppressing high-ISO colour noise. The repeated luminance must also agree on
five parallel lines separated by three pixels: the variance of their average,
after square-root encoding, must exceed 55% of their mean individual variance
(full protection at 70%). This rejects random luminance noise even after colour
noise reduction has made its chromaticity smooth. The check uses
the samples already read for CA plus a small neighbourhood only for candidates. The CPU reuses symmetric line samples and their variances in the one-sided windows, limiting the footprint to nine lines. A full-protection result ends the CPU and GPU search over parallel lines and axes, since further evidence cannot improve it.

A finite strip's boundary needs one-sided support: the symmetric five-line
window may contain two or three flat background lines, lowering agreement even
when every slat line matches. If the symmetric check is not fully coherent,
the CPU and GPU also check five lines extending in either direction along the
slats (offsets 0, 3, 6, 9 and 12 pixels, or their negatives). The same coherence
threshold, local extrema, chromaticity span and source-variation guards apply;
none is relaxed. One-sided windows must fit inside
the image; clamped duplicate border lines cannot establish coherence.

The immediately adjacent border may itself lack the repeated extrema. Only if
its seven-pixel chromaticity span is already below 2%, the sampler may use a
parallel line three pixels away that passes the same periodic/coherence tests.
Its chromaticity must also match the border's (weight fades to zero at a 0.2%
distance), and variation between the three source sample positions must be below
0.1%. This stricter condition avoids protecting real CA on an ordinary edge next
to periodic detail. The original relative source-variation guard still applies.
Tests also keep an ordinary CA edge beside periodic detail unchanged.

Tests cover constant-colour stripes, unchanged isolated CA edges and resolved
coloured stripes, and CPU/GPU presented-preview agreement with fine stripes and lens corrections.
This is a safeguard for reconstructed detail, not a replacement for the lens's
CA model. Ambiguous low-contrast real colour within fine repeated detail remains
a limitation.

### Fujifilm vignetting strength

Applied at full strength, the Fujifilm table leaves corners about 0.08 EV brighter than Lightroom on three X100F photos, while the centre matches. Raising the gain to the power 0.85 brings corners within ±0.03 EV. The LCP path matches at full strength ([below](#adobe-lcp-profiles)); Sony's built-in table has not been compared. The X100F Lightroom reference scorecard goes from 0.0148 to 0.0120 mean MAE.

### Validation — 2026-09-26

The Fujifilm vignetting table for DSCF7853 (X100F) matches the `FixVignetteRadial` opcode Adobe wrote into Lightroom's DNG of the same file. The gain is 1.085 / 1.319 / 1.656 at radius 0.5 / 0.8 / 1.0, against Adobe's 1.087 / 1.326 / 1.670. Distortion is zero for this lens. Adobe's `WarpRectilinear` holds only a red-plane radial term under one pixel, the same order as the Fujifilm CA table.

Full renders against Lightroom's Adobe Standard exports, encoded sRGB at 1200 px, no fitting:

| Photo | Without | With built-in correction |
|---|---:|---:|
| DSCF7853 overall MAE | 0.0304 | 0.0196 |
| DSCF7853 corner MAE | 0.0571 | 0.0167 |
| DSCF7845 overall MAE | 0.0426 | 0.0300 |
| DSCF7845 corner MAE | 0.0546 | 0.0241 |

Sony's embedded vignetting for the FE 55mm F1.8 ZA at f/1.8 restores 1.95× at the corner. Adobe's LCP profile for that lens predicts 1.81×. Lightroom does not apply Sony's vignetting at all ([below](#sony-built-in-corrections)).

### Olympus, OM System and Panasonic

Micro Four Thirds lenses rely on software distortion correction, and Adobe has no LCP profiles for many of them: Lightroom uses the model the camera writes into every RAW (#636). Adobe DNG Converter 18.7 writes it into its DNGs in one of two forms, which RAWmakase compared with the files' own tags on 68 raw.pixls.us photos from 28 bodies: older bodies (E-M10 Mark III, E-M1 Mark II, E-M5 Mark III, E-M1X, TG-6, GX9, GX8, GX85, G9, GH5, GM5, LX100, LX100 II, TZ202) get a WarpRectilinear opcode, newer ones (E-P7, OM-1, OM-1 Mark II, OM-5, G9 II, GH6, GH7, G100, S5) a "Camera Settings" lens profile embedded in the DNG's XMP for distortion, with the lateral CA still in the opcode. Both forms carry the same model, so RAWmakase reads it from the RAW for every body (`crates/rawmakase-decode/src/raw/lens.rs` has the exact formulas):

- **Olympus and OM System**: ImageProcessing 0x150A holds three radial coefficients and a scale, with radius 1 at the half diagonal of the camera's default crop; 0x150C holds red's and blue's scale relative to green, as a polynomial in green's source radius. Distortion and CA match Adobe's opcodes within 0.03 px on 20 of 22 photos (0.4 px on an E-M5 Mark III high-resolution shot; 0.6 px on the TG-6, where Adobe leaves out the CA the file stores), and the newer bodies' distortion matches their embedded profiles within 0.2 px (OM-5: 0.8 px). A lens that needs no correction stores zeros in 0x150A and only the CA in 0x150C; 0x1510 holds a second distortion model that neither Adobe form uses.
- **Panasonic**: 0x0119 (ExifTool's DistortionInfo) holds the camera's own correction, from source radius to output radius, in units of a radius it also stores, and a scale. RAWmakase inverts it numerically. Against Adobe the green warp matches within 0.4 px on all 27 older-body photos (within 0.01 px on 18 of them; the larger differences are on the strongly distorting LX100, LX100 II and TZ202, whose inverse Adobe fits with a polynomial), and the newer bodies' profiles within 0.4 px. 0x011B holds the lateral CA: red's and blue's displacement from green, in thousandths of a pixel, at four radii of the sensor image that it also stores (1/3, 2/3 and 5/6 of about the half diagonal, and the half diagonal itself, on Micro Four Thirds bodies and the LX7; 2/7, 4/7 and 6/7 on the full-frame S bodies and many compacts; other fractions on other compacts). It applies at green's source radius, the sensor radius green samples after the distortion correction; lenses without CA data store an empty table. Adobe does not draw straight lines between those values but writes a smooth polynomial of the warp's form, and RAWmakase fits the same: least squares over the image's radius. Compared with the CA in Adobe's DNGs of 309 distinct photos from 111 bodies (raw.pixls.us; Adobe corrects up to 15 px of CA on them within the default crop, median 1.5 px), RAWmakase's correction, read through the decoder, is within 0.06 px out to the default crop's corner on 90% of them and 0.15 px at worst (median 0.015 px); the straight lines would be up to 0.86 px off, on the 1-inch compacts (LX10, LX15, TZ200/TZ202/ZS200, FZ2500, C-Lux), whose strong distortion correction means the image's corner samples the sensor at only 0.85 of the last knot's radius, so Adobe's fit follows a steep last segment the image never reaches. The CA applies whether or not the distortion correction is on: a Sigma 60mm on a G80 has CA data and no distortion correction, and Adobe corrects the CA there too. Adobe also corrects CA on two LX3 and D-LUX 4 photos that have no 0x011B, from data RAWmakase does not read; Remove Chromatic Aberration can correct those.

Neither Adobe form contains vignetting: Lightroom does not correct Olympus or Panasonic vignetting without an Adobe profile, and neither does RAWmakase.

The correction is on by default, as Lightroom applies it, and is framed as a DNG's opcodes are ([framing](#framing)). Rendering a photo and Adobe's DNG of it with RAWmakase gave the same geometry within 0.4 px at 2400 px on six pairs (E-M10 Mark III, E-M1 Mark II, E-M1X, GX9, LX100 II, G9). The LX100's DNG states an optical centre 8 px left of its active area's centre: that is its default crop's centre, as for every Olympus and Panasonic warp, which the RW2's frame and RAWmakase's DNG reader now both use. Against the dataset's Camera Raw 18.7 renders (2048 px, Enable Profile Corrections off) of an E-M1X photo (M.12-40mm at 12mm) and an OM-1 photo (M.7-14mm at 14mm), the corrected geometry agrees within about 1 px, while the correction itself moves the corner patches by about 13 px on the OM-1 and more on the E-M1X: Camera Raw applies the newer bodies' embedded correction too when profile corrections are off, as the older bodies' warp.

The correction is centred on the default crop, as Adobe's is. Olympus decodes more unused columns on one side than the other, so on the E-M10 Mark III that centre is 8 px (0.3% of the width) from the decoded image's: centring on the decoded image, as RAWmakase did before 2026-10-25, moved its corner geometry by up to 2.9 px from Adobe's warp at full size; it is now within 0.04 px.

### Nikon

Nikon Z bodies store the camera's lens correction in every NEF, in the raw image SubIFD's tag 0xC7D5 (ExifTool's NikonNEFInfo, a small TIFF after a `Nikon` header). Adobe DNG Converter 18.7 turns it into its DNGs in one of two forms, chosen by the distortion record's mode byte, which RAWmakase compared with the records on 101 raw.pixls.us NEFs from 18 bodies (Z 30, Z 5, Z5 II, Z 50, Z50 II, Z fc, Z 6, Z 6II, Z6 III, Z 7, Z 7II, Z 8, Z 9, Z f; D6, D780):

- Mode 3, a lens that needs software correction (NIKKOR Z 24-70mm f/4 S, 24-200mm, 14-30mm f/4 S, DX 16-50mm): FixVignetteRadial and WarpRectilinear opcodes, which always apply.
- Mode 1, a lens whose correction is optional (Z 14-24mm f/2.8 S, 24-70mm f/2.8 S, 50mm f/1.8 S, 40mm f/2, Tamron 35-150mm, an F-mount 18-70mm through the FTZ adapter): a "Camera Settings" lens profile in the DNG's XMP (distortion, piecewise vignetting), with the lateral CA in the warp.
- Mode 0 and 2 (bit 0 clear: the camera's Auto Distortion Control off, as on the D6 and D780 samples): nothing at all, although the records hold values.

Both forms carry the same model (`crates/rawmakase-decode/src/raw/lens.rs` has the exact layout). Each record's polynomial has its coefficients for descending powers of the radius, the last for r¹; r is 1 at the default crop's corner farthest from the optical centre, which record 8 gives in pixels from the image centre:

- Distortion (record 5): green's source radius over the output radius is s·(1 + k₁r⁴ + k₂r³ + k₃r² + k₄r), s = 1/(1 + x) the camera's fill scale (the rational at 0x40), which is Adobe's k₀ and the profile's ScaleFactor to four digits.
- Vignetting (record 6): the gain is 1 + a·(v₀r⁸ + … + v₇r), a the strength at 0x60 (0.40 or 0.53 on these bodies). Adobe's FixVignetteRadial coefficients are a·v₆, a·v₄, a·v₂, a·v₀ exactly (the odd terms are zero on every file), and the profile's piecewise table is its reciprocal within 0.0005.
- Lateral CA (record 7): red's and blue's scale relative to green at the output radius is 1 + e + c₁r³ + c₂r² + c₃r, e the constant after the coefficients.

Against Adobe on the 24 distinct mode-3 corrections, green matches the warp within 0.31 px at full size (median 0.03 px) and red and blue within 0.06 px more; the nine mode-1 profiles match within 0.25 px. The tests check two files' records against Adobe's numbers: the Z 7's warp within 0.18 px on all three planes and its vignetting within 0.003 gain, the Z 8's profile within 0.38 px (CA from the warp composed with it).

Lightroom applies the opcodes whatever Enable Profile Corrections says, so a mode-3 correction is on by default (`default_on`). A mode-1 profile is the one Enable Profile Corrections uses when no Adobe profile matches the lens, so it is off by default and follows the switch, as Sony's built-in data does ([below](#sony-built-in-corrections)); an imported Adobe profile replaces it. Camera Raw 18.7 renders with Enable Profile Corrections off and on confirm both: a Z 6 photo (24-70mm f/4 S, mode 3) renders identically either way, while a Z 8 photo (14-24mm f/2.8 S) and a Z fc photo (18-70mm through the FTZ, both mode 1) are corrected only with it on, the corners moving 10 to 25 px and brightening 1.3 to 1.5×.

The correction is centred on record 8's optical centre, up to 27 px from the image centre on the mode-3 samples (66 px on a Tamron 35-150mm mode-1 one on a Z f), where Adobe's opcodes and Camera Settings profiles centre it too. Centring on the image, as RAWmakase did before 2026-10-25, moved corrected geometry by up to 2.1 px from Adobe's warp on a Z 6 photo (now 0.11 px) and by 6 px on that Tamron photo, whose profile centre (0.489099, 0.502437) record 8 gives within 0.2 px. Adobe's k₀ frames the photo, and RAWmakase adds no zoom when the edge stays inside ([framing](#framing)). Coolpix NRW files keep their correction in other records of the same tag ([below](#nikon-coolpix)).

### Nikon Coolpix

The Coolpix P950, P1000 and P1100 store tables in records 2 and 3 of the same NEFInfo tag, which Adobe DNG Converter 18.7 writes as a WarpRectilinear opcode (distortion and lateral CA, no vignetting), so they apply by default. Each record starts "0100" and four header bytes, then signed rationals:

- Record 2 (header 3 3 17 1): the distortion in percent at 17 radii spread evenly from the centre to the default crop's corner. Through a natural cubic spline, 1 + d/100 is green's source radius over the output radius, scaled like Adobe's warp so that its widest value is 1 (Adobe's k₀ is 1/(1 + d/100) at the corner to five digits).
- Record 3 (header 3 34 1 0): red's and then blue's displacement from green at the same radii, 17 each, in units of 1/3792 of the half diagonal: that unit fits Adobe's red and blue planes on all three bodies within 1% (residual at most 0.02 px).

Against Adobe's warps of one photo per body, the green plane matches within 0.55 px (P1000), 0.8 px (P1100) and 1.2 px (P950), and the test checks the P1100's records against its warp within 0.7 px on all three planes. Record 4 holds a vignetting table, which Adobe does not apply. The A1000 writes record 2 with another header (3 2 17 1) and 15 values, which do not fit Adobe's warp of its single sample photo, so it is not read; older Coolpix NRW files (P7000–P7800, P330/P340) get nothing from Adobe either.

### Samsung

Samsung SRW files store the camera's distortion and lateral CA models in the maker note: Distortion (0xA050, eight signed longs) and ChromaticAberration (0xA051, 22 shorts), in ExifTool's names. Each value is offset by one of the eleven values of the note's EncryptionKey (0xA020): the Distortion values have it added, value i getting key i mod 11, and the CA values after the first have it subtracted, value i getting key (i − 1) mod 11. Adobe DNG Converter 18.7 writes them into its DNGs as a WarpRectilinear opcode (`crates/rawmakase-decode/src/raw/lens.rs` has the exact layout), which RAWmakase compared on all 27 raw.pixls.us SRWs from 22 bodies:

- Distortion: four pairs of a mantissa and a power of ten, c₀ to c₃; green's source radius over the output radius is c₀ + c₁r² + c₂r⁴ + c₃r⁶, r in pixels from the default crop's centre. Adobe's warp is this polynomial exactly, rescaled to its radius unit. A model whose c₀ is zero is none: Adobe then writes the CA alone.
- Lateral CA: a count (7), seven radii in pixels, then red's and blue's scale relative to green at each, in 1/10,000. On every surveyed file each channel stores one value at all radii, apart from blue's seventh, which Adobe ignores, as it does the seventh radius. Adobe applies half of each scale's change on the interchangeable-lens bodies (10,010 is 1.0005), and on the EX2F three quarters of red's and a quarter of blue's.
- No vignetting: the NX200 and later store none (all 1000), and Adobe applies none, nor the first bodies' tables.

The warp's green plane matches Adobe's within 0.07 px at full size on the 11 photos whose distortion Adobe corrects (it moves their corners by up to 260 px), and within 0.15 px on four more whose lens type was relabelled to make Adobe correct them; the CA matches exactly (Adobe corrects up to 1.7 px of it). The tests check three files' tags (NX200, EX2F, and the little-endian NX mini) against Adobe's numbers.

Adobe does not use the camera's data, and nor does RAWmakase, for:

- lenses it has its own profiles for: LensType (0xA003) 1, 2 and 3, the NX 30mm F2, 18-55mm and 50-200mm, on any body (an NX200 file relabelled as the 18-55mm gets no warp, an NX20 file relabelled as the 16mm gets one), and the EX1 and WB2000 compacts. Enable Profile Corrections applies Adobe's profile there, which RAWmakase uses when it is imported ([Adobe LCP profiles](#adobe-lcp-profiles)).
- the first bodies, NX5, NX10, NX100 and NX11, even with a lens Adobe corrects on later ones (an NX10 or NX5 file relabelled as the 16mm gets no warp).

Lightroom applies the warp whatever Enable Profile Corrections says, so the correction is on by default. Camera Raw 18.7 renders of the NX200 photo (NX 16mm) are identical with Enable Profile Corrections off and on, and differ from the same file with its distortion model zeroed by 38 to 73 px at the corners; an NX20 photo with the 18-55mm changes only when the setting is on, as Adobe's profile applies.

Known limits: the CA was verified on constant tables only. Adobe fits a smooth polynomial through the first six values of a table that varies with the radius (a single changed value moves its warp around neighbouring radii too); RAWmakase interpolates between them at green's source radius. On modified files, that is within 0.17 px of Adobe's warp for red rising steadily by 60/10,000 across the radii (7.7 px of CA), but up to 3 px off for a single value 40/10,000 above its neighbours. The correction is centred on the decoded image, while Adobe centres it on the default crop: 1 px apart on the NX200, more on bodies whose decoded image has uneven margins. The EX2F's CA shares are Adobe's for that one compact; other SRW compacts were not available.

### Canon

Of Canon's RAWs, Adobe DNG Converter 18.7 corrects only the EOS R5 Mark II's and R6 Mark III's from the camera's own data: a "Camera Settings" lens profile in the DNG's XMP (distortion and piecewise vignetting) and the lateral CA as a WarpRectilinear opcode whose green plane is the identity. The data is record 14 of the CR3's timed metadata (the `CTMD` track's one sample), found by its vignetting values, which Adobe's piecewise profile repeats exactly (value / 8192). Adobe reads that record and nothing else for it: copies of an R6 Mark III file with the record's tables edited (the copies inside record 9's maker notes left alone) change its profile and warp; flattened tables remove them.

Record 14 holds three counts of knots (17 on these files), then seven arrays of 23 little-endian shorts: the vignetting and its knots, red, blue and their knots, the distortion and its knots (`crates/rawmakase-decode/src/raw/lens.rs` has the layout). The knots are the same on every file and focal length, 0 to 4543, closer together towards the edge, so they are radii on the sensor: Adobe's piecewise radii put them at 5.314 µm each on both bodies (to 0.02%, a 36 mm wide sensor of 6960 or 8192 pixels).

- Vignetting: the share of light kept, in 2^-13; Adobe's profile repeats it with straight lines between the knots.
- Lateral CA: red's and blue's scale relative to green is 1 + (v − 16384)/2^23. Adobe's warp matches straight lines between the knots within 0.01 px.
- Distortion: green's source radius over the output radius, in 2^-14. Adobe does not apply the table as it is but fits its profile's polynomial to it, and that fit is what Lightroom renders. Edited copies of the R6 Mark III file show how: single knots raised or lowered by 1% (the responses add up linearly), smooth tables (1 + 0.01u², u⁴, u), the real table scaled by −1 to 1.5, and spikes up to 20%. The fit is least squares of 1, r², r⁴ and r⁶ to the values past the centre (the centre knot's value is ignored), residuals weighted by the radius; the knots' radii are scaled by the values' mean to the power 0.75, and values above 1.0275 count as 1.0275. The profile is then the fit's change from 1 over its constant term, at 80%, with a ScaleFactor that keeps the default crop's corner in place. This model matches the profiles of the 1% edits, the smooth tables and the scaled real table within 0.15 px at full size, and of knots raised 6 to 20% past the limit within 0.45 px; a knot lowered by 10% is 3.2 px off, so large dips are not modelled exactly (real tables vary smoothly by about 2%). Applied as it is, the table would be up to 9 px from Adobe's profile on the R5 Mark II's 135mm (3.6 px on the R6 Mark III's 50mm), so it would not match Lightroom.

Against Adobe's DNGs of the eight raw.pixls.us files (R6 Mark III with the RF50mm F1.4 L VCM, R5 Mark II with the RF135mm F1.8 L IS USM; full frame and APS-C crop, RAW and C-RAW): green matches the profile within 0.20 px at full size (0.09 px after framing both to the corner: Adobe's ScaleFactor frames about 2·10⁻⁵ tighter than the corner, which is 0.1 px at the corner), red and blue the warp within 0.01 px more, and the light kept within 0.0004 (0.0017 in APS-C crop files, where Adobe places the knots 0.75% further out than in full-frame ones, although the pixels are the same size). The tests check the R6 Mark III's and the R5 Mark II's records against Adobe's numbers, and two edited tables against Adobe's profiles of them. Without the correction, halfway to the corner is 15 px out on the R6 Mark III photo and 37 px on the R5 Mark II one, whose corners keep 0.42 of the light at f/1.8.

Default: off, following Enable Profile Corrections, as for Nikon's optional corrections: Camera Raw 18.7 renders of the R6 Mark III photo with Enable Profile Corrections off and on differ by the profile, so Lightroom applies it only with the switch on. The profile's Distortion and Vignetting amounts scale it, as `Recipe::lens_amounts` does for a correction that is not on by default: renders with the "Camera Settings" profile chosen (`LensProfileSetup` Custom) at Distortion 50 move mid-frame detail half as far as at 100 (7.4 against 14.9 px), at 0 not at all, and Vignetting 0 leaves the corners as dark as with the switch off. Lightroom's default choice for these photos is that profile too, although Adobe ships profiles for both lenses: the default render with the switch on is identical to the one with "Camera Settings" chosen. RAWmakase instead uses an imported Adobe profile when one matches, as for the other makers (a known difference). Whether Lightroom applies the CA warp with the switch off was not measured (it is at most 0.3 px on these files); RAWmakase switches it with the rest.

Not read: an R50 V stores the same record (20 knots, RF28mm F2.8 STM), but Adobe uses it for no body but these two. Its table put into an R6 Mark III file is converted, and renaming the camera or the lens in that file changes nothing, so Adobe evidently decides by the body, not by the record or the lens. Other R bodies (R, R5, R6, R10, R50, R100 among the dataset's) have no record 14. Known limits: RAWmakase centres the correction on the decoded image, which is within 2 px of the default crop's centre, Adobe's, on these files.

PowerShot compacts (G1 X Mark II/III, G3 X, G5 X, G7 X, G9 X, G15/16, S90–S120, SX60/70) get a distortion and CA warp in Adobe's DNGs, but it is Adobe's own data for the camera's lens, not the file's: changing only the EXIF FocalLength of a G7 X Mark III CR3 from 8.8 to 20 mm changes the warp to that of a much weaker distortion, and a G5 X CR2 and a G7 X Mark III CR3 (the same 8.8–36.8 mm lens) at 8.8 mm get bit-identical warps, optical centre included. CTMD records 7 and 9, which hold the maker notes, do not change it when blanked. So RAWmakase reads nothing for them; an imported Adobe profile covers them.

### Sony built-in corrections

Status 2026-10-25. Adobe DNG Converter 18.7 turns Sony's tables into WarpRectilinear opcodes, which Lightroom always applies, on 74 of 108 raw.pixls.us bodies (one photo each; the others, A-mount DSLRs, most SLT and the first NEX bodies, store no tables):

- **Lateral CA on every body with tables**: E-mount (NEX-5N and later, α, FX, ZV-E), the SLT-A77 and A99, the compacts. The warp's red and blue planes match the tables within 0.25 px at full size on 62 of 66 photos, at most 0.7 px (RX100 II, III and V, ZV-1 II).
- **Distortion only on the fixed-lens compacts** (RX100 II to VII, RX10 to RX10 IV, RX0, RX0 II, HX95, HX99, ZV-1, ZV-1 II): on every other body the warp's green plane is the identity, and so is it on the RX1 series, whose stored tables would move the corners 65 to 110 px; Adobe corrects that lens with its own profile only. Through a natural cubic spline over the knots, the green warp matches the table within 0.9 px on 13 of 17 compacts and within 2.6 to 6.2 px on the HX95, the RX0 and RX0 II, the RX100 V A and the ZV-1 II, whose curves Adobe's four-term polynomial cannot follow closely (linear interpolation was 1.4 px off on the RX100 III to V and ZV-1).
- **Vignetting never.**

Camera Raw 18.7 renders of an RX100 IV photo are identical with Enable Profile Corrections off and on. So RAWmakase reads two corrections (`raw::lens::Builtin` in the decode crate): what Lightroom applies, on by default (`Metadata::lens`), and Sony's whole set of tables (`Metadata::lens_stand_in`), which stands in for an Adobe profile when Enable Profile Corrections is on and no imported one matches (`Recipe::builtin_correction`). The Profile row then names "Sony built-in", and the import notice and the panel say "No matching imported Adobe lens profile for …; using Sony built-in" (`Recipe::missing_lens_profile`), so the stand-in is never silent; on the FE 55mm F1.8 ZA it brightens the corners 1.95× where Adobe's profile does 1.81× (above). An imported Adobe profile replaces both, and a profile with PreferMetadataDistort keeps the distortion of the stand-in. Edits saved before 2026-10-25 with built-in corrections off keep them off, also for the CA. The first RX100, whose distortion Adobe also corrects, stores no SubIFD tables and gets none.

Each table leads with its number of values and is padded with zeros to a fixed 16 (CA: 32) slots; a ZV-1 stores 11 distortion and 22 CA values. A table with non-zero data past its count is rejected. On a ZV-1 photo (#403) the corrected geometry matches the camera's own JPEG: 4,000 matched features keep a constant radius ratio from centre to corner (1.0757–1.0773), where the uncorrected render drifts from 1.075 to 0.924. That constant 1.077 was the fill scale, which treated the table's 1.077 at the centre as zoom to take back and made the corrected photo a tighter crop than the camera's (#531). Sony's built-in correction now has the scale that keeps only the photo's edge inside the sensor (`LensCorrection::fill_scale`), the edge of the camera's aspect-ratio crop when there is one, since the camera's 16:9 JPEG is a crop of the 3:2 sensor and samples beyond it. On the ZV-1 that scale is 1, the table's own framing, where the whole-frame fill was 0.928. Every other correction, a Sony DNG's opcodes included, is framed by the default crop's edge ([framing](#framing)). Known limit: widening the crop beyond the camera's can show the sensor's edge smeared in a corner where a table pulls samples past it (the ZV-1's does not); Constrain Crop does not yet know about lens coverage. The crop is the camera crop in `Metadata::camera_crop`, so the framing does not depend on which decoder supplied it.

### Other makers

Which formats Adobe corrects from the camera's own data, from Adobe DNG Converter 18.7's DNGs of one or more raw.pixls.us photos per camera model (2026-10-25; 665 files): it writes what it applies as opcodes, or as a "Camera Settings" profile in the XMP. Where it writes nothing, Lightroom uses only its own lens profiles, and RAWmakase reads nothing either.

| Files | What Adobe applies from the camera's data | RAWmakase |
|---|---|---|
| Sony E-mount bodies (NEX-5N and later, α, FX, ZV-E) | Lateral CA only, as a warp whose green plane is the identity; no distortion or vignetting | Applies the CA; the other tables stand in for a profile ([above](#sony-built-in-corrections)) |
| Sony fixed-lens compacts (RX100, RX10, ZV-1, HX95/99, RX0; RX1: CA only) | Distortion and lateral CA | Applies them (the first RX100 stores no tables) |
| Sony A-mount DSLRs, most SLT, NEX-3, NEX-5, NEX-C3 | Nothing | — |
| Canon R5 Mark II and R6 Mark III (CR3, RF lenses) | A "Camera Settings" profile (distortion, piecewise vignetting) and CA as a warp, from CTMD record 14 | Read, off by default ([above](#canon)) |
| Canon EOS R, RP, R3, R5, R6, R6 Mark II, R7, R8, R10, R50, R50 V, R100 (RF and EF lenses), EOS DSLRs, EOS M | Nothing (the R50 V stores record 14 too) | — |
| Canon PowerShot compacts (G1 X II/III, G3 X, G5 X, G7 X, G9 X, G15/16, S90–S120, SX60/70, V1) | Distortion and lateral CA (warp) from Adobe's own data by focal length, not the file's ([above](#canon)) | — |
| Samsung NX (NX200 and later), NX mini, EX2F, Galaxy NX | Lateral CA, and distortion where the camera stores it (warp); nothing for lenses Adobe has profiles for (NX 30mm, 18-55mm, 50-200mm), EX1 and WB2000 | Read, on by default ([above](#samsung)) |
| Samsung NX5, NX10, NX100, NX11 | Nothing | — |
| Nikon Coolpix P950, P1000, P1100 (A1000) | Distortion and lateral CA (warp) | Applied ([above](#nikon-coolpix)); the A1000's other format is not read |
| Nikon DSLRs, Nikon 1, older Coolpix (P7000–P7800, P330/P340, A) | Nothing | — |
| Leica Q, Q2, CL, SL, SL2, X2 DNGs | The camera's own warp in its DNG (distortion and CA, centred) | The [DNG reader](#dng-files) applies it |
| Leica M8 to M10-R DNGs | Nothing | — |
| Pentax PEF and DNG, Ricoh GR, GR II, GR III, GXR | Nothing | — |
| Hasselblad 3FR and FFF (X1D, X2D, CFV, H series) | Nothing | — |
| Sigma fp DNG | Lens shading as GainMaps | Applied ([DNG files](#dng-files)) |
| Sigma X3F | Not supported by Adobe | — |

## Remove Chromatic Aberration

Lightroom's Remove Chromatic Aberration (`crs:AutoLateralCA`) needs no lens data: it measures lateral CA in the photo. `crates/rawmakase-model/src/lens/auto_ca.rs` does the same on the decoded image, once per decode, when the recipe's `lens_ca` is set:

1. Tiles of about 1/90 of the long edge are scored by green's edge strength across the radius; tiles near the centre or with clipped pixels are skipped, and the 1200 strongest are kept.
2. In each tile, red and blue are matched to green along the radius: the best of whole-pixel shifts (up to 6 px on a 6000 px edge) by least-squares fit, a parabola, then Gauss–Newton steps to a fraction of a pixel. Tiles that fit poorly are dropped.
3. Each channel's shifts are fitted as a radial scale, 1 + k0 + k1 r² + k2 r⁴ (fewer terms with fewer or less spread tiles), with Tukey reweighting. Beyond the radius that 80% of the tiles lie within, the scale is held. Below 0.08 px at the corner (on a 6000 px edge) nothing is corrected.

The measurement replaces the lens data's own lateral CA, keeping its distortion and vignetting, and applies alone when there is no lens data (adapted manual lenses). Radii are measured from the optical centre of the camera's own correction (`Metadata::lens`), else from the image centre, and the measurement is evaluated about that centre also when the edit leaves the camera's correction off (`lens::auto_ca::centre`); with a correction applied, it is evaluated about that correction's centre, so an imported profile whose centre differs (by about 1% at most) shares the camera's measurement. It does not change the framing. Previews and exports measure the same decoded image; pyramid levels and reduced copies share the measurement through `Metadata::lateral_ca`, so it never comes from a reduced copy. It takes about 0.1 s on a 24 MP photo.

### Validation — 2026-10-01

Measured on the uncorrected decode and compared with the camera's own CA table, as displacement in pixels on a 6000 px long edge at radius 0.4 / 0.7 / 1.0:

| Photos | Red, measured | Red, camera table | Blue, measured | Blue, camera table |
|---|---|---|---|---|
| 8 X100F (23mm), range | +0.53…0.78 / +0.66…1.23 / +0.94…1.76 | +0.54 / +0.92 / +1.32 | −0.07…−0.42 / +0.01…−0.50 / +0.02…−0.71 | none |
| DSC05743, A7 II FE 55mm | +0.13 / +0.17 / +0.18 | +0.18 / +0.15 / 0 | −0.15 / −0.21 / −0.26 | −0.18 / −0.23 / −0.22 |
| DSC01768, A7 II FE 55mm | +0.13 / +0.16 / +0.21 | +0.18 / +0.23 / 0 | −0.16 / −0.17 / −0.22 | −0.18 / −0.23 / 0 |

Fujifilm's table corrects red only, but all eight X100F photos show blue displaced inwards at radius 0.4, which Remove Chromatic Aberration corrects too.

The same measurement on rendered sRGB output (Adobe Standard, full size) of 13 photos (8 X100F, 3 A7 II, 2 Leica M10 DNGs), against Camera Raw 18.6 renders of the same files, as mean remaining displacement in pixels of red and blue:

| | Radius 0.4 | Radius 0.7 | Worst at 0.7 |
|---|---:|---:|---:|
| Camera Raw, off | 0.24 | 0.30 | 0.48 |
| Camera Raw, on | 0.18 | 0.17 | 0.32 |
| RAWmakase, off | 0.34 | 0.31 | 0.62 |
| RAWmakase, on | 0.11 | 0.15 | 0.49 |

Output-space figures are only comparable with each other: the colour matrix mixes channels, so they differ from the camera-space ones above.

Camera Raw renders with the setting off also show Sony's lateral CA corrected: Lightroom always applies Sony's stored CA ([above](#sony-built-in-corrections)), as RAWmakase now does.

## Manual distortion

Lightroom's manual Distortion (Lens Corrections > Manual, `crs:LensManualDistortionAmount`, −100 to 100) is `Recipe::lens_manual_distortion`, rendered by `geometry::ManualDistortion`. Measured on Camera Raw 18.7 renders of the synthetic chart at ±10, ±25, ±50, ±75 and ±100, fitting the radial map to whole images (residual 0.004–0.006 mean absolute difference, the resampling floor):

- An output position at radius r, with r = 1 at the frame's corners, samples radius r·(1 + k·(1 − r²)), with k = 0.4 × amount / 100 for positive amounts and 0.5 × amount / 100 for negative ones. Corners stay where they are.
- Positive amounts correct barrel distortion: the middle shrinks and the edges' middles come from outside the photo, which renders white, as Lightroom shows it without Constrain Crop. Negative amounts correct pincushion distortion and enlarge the middle, so nothing white appears.
- Order: Camera Raw applies it in the frame as recorded, centred on the uncropped photo. Crop and Straighten cut its result (a cropped render equals the same crop of the uncropped one exactly), and Upright and the Transform sliders apply after it: Vertical, Scale and Offset renders match only that order. On a DNG whose WarpRectilinear opcode distorts the chart, the lens correction applies after it on the way to the sensor, so `Geometry::source` applies it after the homography and before `LensMap`.

The `lens-manual-distortion*` corpus cases (±50, and +50 with Vertical +30) sit at mean ΔE00 1.3–1.7 from Camera Raw (the default render is 0.9), against 16–21 if it were ignored. The Lens Corrections panel's Distortion › Amount slider sets it. Constrain Crop crops the white out ([transform](transform.md#constrain-crop)).

## Manual vignetting

Lens Corrections > Manual > Vignetting (`crs:VignetteAmount`, `VignetteMidpoint`) follows Camera Raw 18.7, measured on flat synthetic DNGs at three brightnesses, a 3:2 and a square frame, and four crops (`effects::lens_vignette`). Camera Raw multiplies scene-linear light by a radial gain over the whole photo, before the tone curve: the crop neither moves nor resizes it, and the gain is the same at every brightness. Positive amounts lighten the corners. With `r` the distance from the centre over the half diagonal, `ln gain = amount × c·r^p / (1 + k·r^p)`, where Midpoint (0–100) raises `p` from 2.1 to 9.8 and moves `c` and `k` a little. The fit's log error is 0.018 RMS over 32 renders.

RAWmakase applies it while sampling the camera image, in one table with the lens profile's vignetting (on the GPU as well), so it also works without lens data. On the chart cases `lens-vignetting-50`, `+50` and `-50-midpoint20`, mean ΔE00 to Camera Raw went from about 5.1 (7.5 at Midpoint 20) to 0.8–1.0, the default render's own distance. Before, RAWmakase darkened the finished pixels inside the crop with the opposite sign; that operator is no longer rendered, and recipes saved with it render with the measured one.

The radius is measured on the photo frame, the camera's default crop; the gain shares the lens correction's optical centre (the decoded image's without one), so a default crop off that centre is approximated there.

## Defringe

Lightroom's Defringe (`crs:DefringePurpleAmount`, `…GreenAmount` and their Hue ranges) reduces the chroma of hues inside the Purple and Green ranges. `Effects::defringe_color` does it per pixel in Oklab, after the colour controls:

- The Purple and Green windows are centred at Oklab hue 0.875 and 0.46 (0–1). Each Hue slider's 0–100 spans 0.5 of hue around its centre, and a range's ends are softened over ±0.025.
- At full weight the share of chroma removed is (1 − 0.45·e^(−C/0.09)) · (1 − e^(−Amount/2.5)), with C the Oklab chroma and Amount 0–20: a stronger fringe colour loses more of its chroma, and Amount 5 already does most of the work.

The constants were fitted to Camera Raw 18.6 renders of two A7 II photos (FE 55mm F1.8 ZA at f/1.8 and f/5.3) at Purple 5, 10 and 20, Purple 20 with a 45–55 range, and Green 10 and 20, comparing the chroma kept per hue between each render and the same photo without Defringe. The mean difference from Camera Raw fell from 0.14–0.26 to 0.05–0.11 across those cases. Camera Raw also reduces chroma somewhat more next to strong edges, including hues just outside the range; that spatial part is not reproduced.

The Fringe Color Selector (the eyedropper at the Defringe heading) reads the shown colour, picks the nearer window that holds its hue, sets that range to 20 wide around it and Amount to 5 when it was 0. Reds, yellows, blues and near-greys are refused.

## DNG files

A DNG records the corrections Lightroom applies to its raw image. `crates/rawmakase-model/src/dng.rs` reads FixVignetteRadial from OpcodeList2 and WarpRectilinear from OpcodeList3 (radial terms, per plane, about the opcode's optical centre, radius 1 at the active area's corner farthest from it) into the same correction model, enabled by default. It also reads the embedded camera profile (offered as the file's own profile, as in Lightroom), BaselineExposure and DefaultCrop. The Lightroom-made DNG of DSCF7853 renders at 0.020 MAE against Lightroom's export with no imported files, against 0.030 before. GainMap opcodes (used by phone DNGs for lens shading) in OpcodeList1 and 2 are applied while decoding ([RAW decoding](raw-decoding.md)). OpcodeList3's GainMaps, which DJI and Hasselblad drones and the Sigma fp write for their lens shading (gains up to 2.8 in the corners, different per channel), are read into the correction as a gain per channel on a grid (`GainGrid`) and applied with the vignetting while sampling, on the CPU and the GPU, before the warp, in full: like Camera Raw, which renders a file's opcodes whatever the profile amounts, the Vignetting amount does not scale them (`Recipe::lens_amounts`). They need to cover the whole image, every row and column; others are not read. On the dataset's two Hasselblad L1D-20c validation photos the default render's median ΔE00 to Camera Raw 18.7 went from 3.82 and 2.95 to 0.37 and 0.59: without it their corners were up to 11 L* too dark.

## Adobe LCP profiles

Lightroom's Enable Profile Corrections uses an Adobe lens profile. RAWmakase reads the same `.lcp` files when the user imports them (`rawmakase import-lens-profiles FILE…`, or the app's import command); they are copied to `lens-profiles` in the data directory and never read from a Lightroom installation. Each file is imported on its own, so a file without usable entries (Adobe ships a few) is listed as skipped and the rest still import. `crates/rawmakase-model/src/lens/lcp.rs` matches the photo's lens model, preferring raw profiles and profiles made on the photo's camera make, then on a make sharing the lens mount, as Adobe profiles some third-party lenses on one body only, and interpolates the model in focal length and aperture, taking the farthest focus distance. Canon EF lens names match with or without a space between EF and the focal length (`EF 24-105mm` / `EF24-105mm`); mount and version suffixes stay distinct. A file that records no lens model at all (Nikon bodies such as the D600, and Pentax bodies) is matched by the maker's lens ID instead: the maker note's `LensIDNumber` for Nikon, or the two `LensType` bytes for Pentax (`Metadata::lens_id`), against the profile's `stCamera:LensID`, which holds the same number (Pentax's written as the two bytes, "4 252"), and only against profiles of the photo's maker, as each maker numbers its own lenses; Ricoh's profiles count as Pentax's. Such a file takes its lens name from the profile that identifies it: RAWmakase keeps no table of lens IDs of its own. Adobe's files hold the maker's numbering and it is not unique (Nikon's 154 is the AF-S DX VR 18-55mm f/3.5-5.6G, a Tokina 17-35mm F4 and a Yongnuo 50mm f/1.8 alike). Nikon's maker note also records the lens's focal range and maximum apertures (its `Lens` tag, `Metadata::lens_info`), which the profiles hold as `stCamera:LensInfo`; where both say them, a profile whose values differ is not the photo's lens. An ID that still fits more than one lens (Nikon's 139, two 18-200mm f/3.5-5.6 lenses) matches nothing automatically, so those profiles stay a manual choice rather than being applied to the wrong lens. A Nikon file that names no lens and that no profile identifies is named from its focal range and apertures as Camera Raw names it ("18.0-55.0 mm f/3.5-5.6"), so the Library and Develop show it and Lensfun offers the lenses that fit. It converts distortion, vignetting and chromatic models to the correction above. When the profile sets PreferMetadataDistort, the camera's own distortion is kept, as Lightroom does.

Each distortion, vignette and chromatic polynomial keeps its own `FocalLengthX`.
An omitted normalization uses that calibration entry's focal length and sensor
factor, including for nested models; it does not borrow the first zoom entry's
scale or the requested photo focal length. Before focal-length or APEX-aperture
interpolation, coefficients are converted to the common half-diagonal radius
(`k_n × scale^(2n)`). Vignetting interpolates attenuation before taking its
reciprocal. Requests outside a model's calibration range hold its endpoint,
including the endpoint's normalization. This prevents zoom profiles from producing
rings or invalid corrections when their longer-focal-length coefficients are
interpreted at a wide-angle scale (#614).

The profile's optical centre (ImageXCenter, ImageYCenter, up to 5% off the image
centre in Adobe's profiles) is the correction's. A profile with PreferMetadataDistort
keeps the camera's centre with its distortion. Unequal X/Y focal scales, tangential
distortion and the separate chromatic green model are not represented by the radial
correction tables. This fix does not
establish exact Camera Raw interpolation parity. Synthetic tests cover equivalent
polynomials in different coordinates, zoom/aperture interpolation, sensor formats,
frame orientation and CPU/GPU rendering. Optional read-only tests accept a local
profile through `RAWMAKASE_LCP`; no installed profile data is included in fixtures.

`Recipe::lens_profile` enables it, from `crs:LensProfileEnable`. `lens_distortion` and `lens_vignetting` are the profile's Distortion and Vignetting amounts (`crs:LensProfileDistortionScale` / `VignettingScale`, 0–200%). They scale a profile: an imported one, or camera data standing in for it (Sony's tables, Nikon's optional profile, Canon's) while Enable Profile Corrections is on (`Recipe::lens_amounts`). A built-in correction the camera always applies renders in full whatever the amounts and the switch: Camera Raw 18.7 renders a Z 6 NEF and an RX100 IV ARW identically with both amounts at 0, profile corrections off or on, and an X100F RAF identically with them at 0 and profile corrections off: the distortion's radial scale moves from none at 0 to twice the profile's at 200, and the vignetting gain is raised to the amount (0 none, 200 the gain squared). With an imported profile in use, the profile replaces the built-in correction. Without one, the built-in correction applies when `lens_builtin` is set.

### Choosing a profile

The Lens Corrections panel's Profile part has Lightroom's Setup, Make, Model and Profile menus. They list the imported profiles that fit the photo's camera (`lcp::PhotoProfiles`, rebuilt when a photo opens): profiles with an entry whose sensor covers the camera's, raw profiles in place of non-raw ones for the same lens. A profile is one imported `.lcp` file. Make and Model come from its `LensPrettyName` (the first word is the lens maker), the Profile item is its `ProfileName`.

`Recipe::lens_profile_choice` holds the Setup and the profile the edit names (`lens::choice`), from and to `crs:LensProfileSetup` (`LensDefaults`, `Auto`, `Custom`), `LensProfileName`, `LensProfileFilename` and `LensProfileDigest`:

- **Default** and **Auto** use the imported profile of the photo's lens made on the same camera make, else on a make sharing the mount, else on any make. They never choose a [Lensfun](#lensfun-profiles) lens. RAWmakase keeps no saved lens defaults, so Default matches as Auto does, as Lightroom does for a lens without one. A profile the edit names is used instead when it is imported and profiles this lens.
- **Custom** uses the profile the edit names, whatever lens it was made for, as for an adapted or manual lens. Picking a make, model or profile sets Custom; picking Default or Auto forgets the named profile and matches again.
- A named profile is found by its file name, ignoring case, or by profile name when the edit records no file name; another file with the same profile name does not stand in for a recorded one. One that isn't imported is kept in the edit, written back to exports, and reported in the import notice and under the menus: `Lens profile "…" isn't imported; using …`, with the matched profile (Default, Auto), the built-in correction or no correction.
- An edit whose profile is the one the RAW carries (`LensProfileIsEmbedded`, which Adobe names "Camera Settings") renders the built-in correction, even when a matching LCP is imported, and says so when the file has none.
- `LensProfileDigest` is kept as read and written back with the same file; RAWmakase does not compute Adobe's digest, so a profile picked here is written without one.

Constrain Crop and the profile do not interact: the profile's distortion is scaled to stay inside the photo at every amount, and Constrain Crop crops what Upright, the Transform sliders and manual Distortion uncover after it. Switching the panel off renders no profile and keeps the choice.

Against Camera Raw 18.6 renders with profile corrections on (8 A7 II photos, FE 55mm F1.8 ZA at f/1.8), RAWmakase with the imported Adobe profile averages 0.0088 MAE, with centre and corner exposure within ±0.02 EV. Without correction the corners were 1 EV darker.

## Lensfun profiles

[Lensfun](https://lensfun.github.io)'s community database (CC BY-SA 3.0, Lensfun contributors; 1,578 lenses and 1,059 camera entries in the version of 2026-10-28) covers many lenses whose cameras store no corrections (Canon's EF, EF-S, EF-M and most RF bodies, Nikon's DSLRs and Nikon 1, Sony's first NEX and A-mount bodies, Leica M) and adds vignetting where a camera stores only distortion and lateral CA. RAWmakase reads its XML format and evaluates its models with its own code (`lens::lensfun`, `optics::lensfun`); none of Lensfun's code is used, and the database is not bundled ([licensing](dependencies.md)).

### Installing

Nothing is downloaded until the user asks. Where no imported Adobe profile fits the photo and its built-in correction lacks distortion, lateral CA or vignetting, Lens Corrections › Profile shows a **Lensfun** row: before Lensfun is installed it is the same for every photo (**Download**, about 0.5 MB, with the source and license below it), then progress and Cancel, and Retry with the reason after a failure. Preferences › Profiles & Presets › Lensfun profiles shows the version installed and its lens count, Download, **Check for Updates** (which asks Lensfun for its newest version, and only then offers **Update to** its date), **Remove…** (confirmed; edits keep their corrections), Show in Files, the terms, and **Suggest Lensfun profiles in Lens Corrections**. `rawmakase lensfun status|install [--from ARCHIVE]|update|remove` does the same from a terminal.

The source is Lensfun's update channel, `versions.json` (the newest database's timestamp and the formats offered) and `version_2.tar.bz2`, the database in format 2, which is what `lensfun-update-data` installs, so an update needs no new RAWmakase release. Lensfun publishes no checksum: an archive is accepted over HTTPS only, within limits (8 MB packed; 32 MB and 200 files unpacked), holding only regular `.xml` files without folders (and a `timestamp.txt`, should it have one, that agrees), every one parsing as a Lensfun database, with at least one lens, and dated as `versions.json` says (Lensfun dates each file with the database's timestamp). A version is written into a staging folder in the data folder's `lensfun`, synced, renamed to a folder of its own (its timestamp) and never changed after; `installed.json`, with the archive's URL, size, SHA-256 and download time, is then replaced atomically. A failure, cancel or crash leaves the version in use. Older versions stay until the next install, update or removal, for a reader that has just read the old manifest; one install, update or removal runs at a time across RAWmakase's windows and the CLI, under a lock file. Once installed, everything works offline.

### Choosing a Lensfun lens

Lensfun is never applied unasked: Default and Auto never choose it, existing edits never change, and a downloaded database only makes lenses available. Installed, the row says what the photo matches: the lens, the corrections it gives this photo and **Use**; several possible lenses, or none, with a menu of the lenses that fit the camera; a lens of another projection; or a camera Lensfun doesn't know whose crop factor the photo doesn't record. The row appears only when Lensfun would add a correction the photo's built-in data lacks. The Make, Model and Profile menus list Lensfun's lenses after the imported Adobe profiles, the profile labelled **Lensfun** (with the crop factor and aspect ratio of its calibration when Lensfun has the lens calibrated more than once). Choosing one, from the row or the menus, sets Setup to Custom and Enable Profile Corrections on, one History step.

A Lensfun lens then replaces the built-in correction and any Adobe profile, as an imported profile does: its distortion, lateral CA and vignetting, as far as its calibration has them, at the profile Distortion and Vignetting amounts. Components are not combined across sources; the row says what the lens corrects. On the dataset, choosing the matched lens loses a component of the camera's own data on 132 photos (Sony's tables standing in for a profile with vignetting, where Lensfun has none for the lens, among them). Where it can't correct the photo, the built-in correction renders, never an Adobe profile, and the panel says why: a zoom lens and no focal length in the photo, a calibration made on a camera of larger crop factor (by more than 4%, Lensfun's own tolerance), a lens of another projection, no aperture (no vignetting), no vignetting calibrated near this focal length and aperture, or a correction outside what RAWmakase applies (a vignetting falloff reaching zero inside the photo).

**Matching.** The camera is Lensfun's entry for the make and model the file itself writes (EXIF Make and Model, not RAWmakase's names for them), ignoring case and spacing. A lens can be on it when one of its mounts is the camera's or one Lensfun lists as fitting the camera's (`<compat>`, one way), and when its calibration covers the photo. A name of numbers, makers' names aside ("Canon 50mm f/1.8"), is never suggested alone. A name matches when every word of the photo's lens name is among the lens's (words of one kind of character, as Lensfun documents: numbers, with "4.0" read as "4", letters, and `+` and `*`; a lone "f" and makers' names left out), both state the same generation ("II", "Mark III", "Mk2"; Lensfun's "I/II" names both; Tamron's "Di II" and "Di III" are lens lines), focal lengths and widest aperture ("70-200mm f/2.8", "1:2/35"), a third-party maker the photo names (Sigma, Tamron…) is the lens's, and the photo's focal length is within the lens's range (its `<focal>`, else what its name states in millimetres when its calibration lies within that, else its calibration's). Of those, the lens whose name the photo's explains most of is suggested, only when it is the one best and the photo's name says more than numbers ("24-70mm" fits seven lenses); a fixed-lens camera's own lens needs no name. Same-named lenses calibrated on different cameras (the Sigma 19mm f/2.8 DN at crop 1.534, 3:2 and at 2, 4:3) are distinct, and offered as a choice. On the 728 lens names RAWmakase decodes from the dataset's photos (2026-10-29; matched by name alone, with 6,000 × 4,000 frames and no 35 mm equivalent, as the comparison with Lensfun's own matcher was made) the matcher agrees with Lensfun's best match on 528 of its 539 unique matches; on 9 Lensfun ties lenses of different generations (an EF-S 18-55mm IS against its IS II and STM) that RAWmakase tells apart, and 2 are Leica Q bodies' own lenses, which RAWmakase matches by the camera's mount.

### What an edit keeps

`Recipe::lens_profile_choice` names either an Adobe profile, saved as before, or a Lensfun lens (`{"lensfun": …}`), never both. A Lensfun choice holds the lens's identity in the database (maker and model as Lensfun identifies them, its mounts, and the crop factor and aspect ratio it was calibrated at), the camera it was chosen for (EXIF make and model, crop factor and aspect ratio), a copy of the lens's whole calibration (every distortion, CA and vignetting row, its focal range, type and centre; up to about 20 KB, stored once per History) and where it came from: the source URL, the database's version, "Lensfun contributors", "CC BY-SA 3.0", the license's URI and that it was converted by RAWmakase. Rendering evaluates that copy for the photo and never reads the database, so updating or removing it changes no edit. When an installed version has a newer calibration of the lens, the Lensfun row offers **Update**, one History step. Copy Settings, Sync and presets take the lens, its calibration and provenance to another photo with that photo's camera (Lensfun's entry for it in the database installed then, else the crop factor its EXIF gives), so the correction is scaled for its frame.

Recipes naming a Lensfun lens, used or not, are saved as version 12 (presets, legacy sidecars), which releases before Lensfun refuse, keeping the file unchanged; the catalog becomes format 4 with the first write storing one ([catalogs](catalogs.md#sqlite-format-versions-1-to-5)). Other recipes stay 11 and catalogs 3. XMP (sidecars, exports, presets of the Lens Corrections group) carries the lens by identity only, never its calibration, in RAWmakase's namespace (`https://rawmakase.com/ns/xmp/1.0/`: `LensfunMaker`, `LensfunModel`, `LensfunMounts`, `LensfunCrop`, `LensfunAspect`, `LensfunName`, `LensfunDatabase`), with `crs:LensProfileSetup` Auto and no Adobe profile, so Lightroom applies its own profile for the lens if it has one: Lightroom does not see Lensfun's correction. Reading such XMP in RAWmakase (applying a preset, importing a sidecar) resolves the lens against the database installed at that moment (not when the photo opened), which may be newer than the one the XMP was written with; without the database or the lens, the edit keeps it named and **unresolved**: it renders the built-in correction or none, says so, and offers the download or **Resolve**, one History step. Adobe identities in XMP never become a Lensfun choice, and a Lensfun choice is never stood in for by an Adobe profile.

### The model

For the format 2 models of rectilinear lenses, as Lensfun applies them (written from Lensfun's documentation and checked against Lensfun at revision bc5615d6):

- **Frame.** The photo frame is the camera's default crop; its crop factor is Lensfun's camera's (of the entry for the frame's aspect ratio, where Lensfun gives a body's aspect modes as variants with their own crop factors), or, for a readout of another aspect ratio (the G6's 1:1), that readout's: the largest rectangle of its aspect ratio on the sensor, whose aspect ratio is Lensfun's for the camera, or 4:3 for a (Micro) Four Thirds mount. Without either, the frame is taken as the whole sensor, as Lensfun does. A crop mode multiplies it: Canon's APS-C (1.6) and APS-H (1.3) modes as the maker note's AspectInfo records them (an R6 in APS-C mode), and otherwise a 35 mm equivalent focal length more than 10% off both the frame's and the sensor's crop factors (Nikon's DX and Sony's APS-C modes; one that describes the whole sensor, as Panasonic's 1:1 readouts record it, says nothing of a crop mode). A camera Lensfun doesn't know takes the crop factor its 35 mm equivalent focal length gives, and none is guessed beyond that. Margins the decoder keeps outside the default crop are outside the frame, and its centre is the optical axis, moved by a lens's `<center>` (half the frame's short edge per unit).
- **Units.** Distortion and lateral CA are functions of u, the radius in units of half the short edge of the frame the lens was calibrated on (`√(36² + 24²) / (2 · crop · √(aspect² + 1))` mm on the sensor); vignetting of v, in units of half its diagonal.
- **Distortion** maps the corrected radius to the recorded one, rescaled so the centre keeps its magnification: the source radius over the output radius is 1 + k₁u²/(1 − k₁)³ (poly3), 1 + k₁u² + k₂u⁴ (poly5), 1 + a·u³/d⁴ + b·u²/d³ + c·u/d² with d = 1 − a − b − c (ptlens). RAWmakase then frames it by green as every correction ([framing](#framing)).
- **Lateral CA** scales red's and blue's source radius relative to green's as a function of green's source radius (after distortion), k (linear) or v + c·u + b·u² (poly3), so the Distortion amount doesn't move it (`LensCorrection::chromatic_at_source`, on the CPU and the GPU).
- **Vignetting** (pa) darkens by 1 + k₁v² + k₂v⁴ + k₃v⁶; the gain is its reciprocal, at the source pixel.
- **Interpolation.** Distortion and CA in focal length by a cubic Hermite spline between the calibrated focal lengths on either side, tangents from the next ones out (from the pair itself at the ends), held at the nearest beyond them; distortion's coefficients and CA's c and b are multiplied by their focal length before and divided by the photo's after. Vignetting by inverse distance weighting (power 3.5) over (focal length within the vignetting rows' range, 4 / f-number, 0.1 / distance in metres), a row nearer than 10⁻⁴ taken as it is, none nearer than 1 correcting nothing. RAWmakase reads no focus distance yet, so every photo is taken as focused at 1,000 m.
- **Sampling.** Each function is sampled into a table with knots where it bends, halved until the table is within 0.01 px of the model at the photo's size (a table still more than 0.02 px off when the knots run out is not applied); the strongest calibrations (a full-frame E 10-18mm at 10 mm) need about 250 knots, so a table may have up to 256 (`Radial::MAX_KNOTS`), and the GPU's search takes its depth from that.

**Validation.** On 13 synthetic photos of six made-up calibrations (zoom ptlens with Hermite and held ends, poly3 on a smaller sensor, a 4:3 calibration on a portrait sensor, poly5 on 16:9, a decentred lens), RAWmakase's source positions are within 0.0064 px of Lensfun's for distortion, 0.0024 px for CA and 0.0064 px for both, and its vignetting gains within 6·10⁻⁵, over 35 points each; interpolated coefficients equal Lensfun's `Interpolate*` (`optics::lensfun` tests, from `tests/data/lensfun/reference.json`). Every calibration of the real database evaluates within 0.0101 px of its model at 6,000 × 4,000 pixels at both ends of its focal range (an opt-in test, `RAWMAKASE_LENSFUN_DB`). CPU and GPU render the same tables, more than 64 knots and CA at the source radius included, at Distortion 0, 100% and 200% (a hardware test, run on a Radeon 780M). Lensfun's calibrations need not agree with a camera's own data: matched with corners aligned, their distortion differs in shape by 20 px from OM System's at the edge of an OM-5 Mark II photo (OM 12-45mm at 12 mm, 5,184 px wide), and by 13 px from Sony's tables on an α7R V (FE 50mm F1.2 GM) and an α9 III (FE 55mm F1.8 ZA). No photograph has been checked against straight lines or a flat field here; that, and a comparison with Camera Raw's Adobe profiles on the private corpus, are still to do.

**Coverage.** Of the dataset's 1,796 CC0 photos with decode fingerprints (2026-10-29, `scripts/corpus/lensfun-coverage.py`, the decoded lens names as Develop matches them, database of 2026-10-28), 795 match one Lensfun lens (fixed-lens compacts included), 55 several, 105 none, 487 record no lens name and 354 cameras Lensfun doesn't know (Phase One and Hasselblad backs, phones, newer OM System bodies). Of the 795, 791 can be corrected; 425 would gain a correction their camera's data lacks (distortion on 311, lateral CA on 268, vignetting on 251), 247 of them having none at all.

**Not supported.** Fisheye, equisolid, stereographic, orthographic, equirectangular and panoramic lenses (30 in the database: 13 equisolid, 11 fisheye, 5 stereographic, 1 panoramic) are listed but not applied: their projection would have to be converted, which is separate work. Lensfun's acm models (none in the database) are listed as not supported. A lens's `<crop>` (circular fisheye framing) and real focal length are not read, and the focus distance is taken as 1,000 m. Lightroom doesn't see a Lensfun correction in XMP.
