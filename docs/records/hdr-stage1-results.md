# HDR results: references, measurements and progress

Status: in progress, 2026-11-09. The record of the
[HDR implementation plan](hdr-implementation-plan.md): what was measured, the
decisions taken from it, validation results and what remains open. Stage 1 (references,
codec and display feasibility) comes first; stage 3's rendering measurements follow
in [HDR rendering](#hdr-rendering). Progress on every stage is in
[Progress](#progress). The rendering contract itself is [HDR](../hdr.md).

## Reference harness

`scripts/corpus/hdr_reference.py` captures a separate HDR reference family through
Photoshop and Camera Raw. It never reads or writes the SDR parity corpus. Each job
hashes the source and the DCP, opens a temporary copy, and keeps the full-resolution
16-bit TIFF, the requested and returned XMP, and the ICC profile's hash. It also
keeps a linear Display P3 array reduced by an area mean in linear light (factor 8
by default, partial edge blocks kept). PQ is decoded with the ST 2084 EOTF and
normalised so that 203 cd/m² is 1.0. The script's unit tests cover the PQ anchors,
linear reduction with highlights and negatives, and metrics that cannot hide
highlight clipping.

The references below are local diagnostics on a CC0 Nikon Z 8 NEF from raw.pixls.us
(8256×5504, Camera Standard and Camera Standard HDR DCPs). They were made with
Camera Raw 18.7 (2743), Process Version 15.4, lens and detail processing off, As
Shot white balance, and output to 16-bit P3 PQ (HDR) or Display P3 (SDR). They are
diagnostics. The frozen references are in the dataset ([Frozen references](#frozen-references)).

### Frozen references

The HDR stages' captures were frozen in the test dataset on 2026-11-09
([HDR references](../../tests/corpus/README.md#hdr-references)); none was re-rendered:

- **Family:** 1,003 distinct jobs. The twelve photos of [Validation](#validation) at
  neutral HDR and every measured setting, with their SDR captures; the six HDRMerge
  floating-point DNGs of a Canon EOS 5D Mark III (CC0, 16, 24 and 32 bit, compressed
  and not) at neutral HDR and SDR; and 565 synthetic ramp, scene and patch jobs. The
  factor-8 linear arrays of photos are stored as float16, which moves every stage 1
  metric by at most 0.0006 stop and 0.006 ΔE_ITP. Read back with `hdr_validate.py`,
  the neutral renders of `main` f7d7ee9a reproduce the [Validation](#validation) table
  within 0.01 stop and 0.1 ΔE_ITP on eleven photos. The Nikon Z5 II now measures ΔE_ITP 4.0 / 8.2 and SDR
  0.030 (the table's 2.2 / 4.5 and 0.010). The Lightroom merge of #525 is not CC0 and
  has no capture.
- **Gate set:** Camera Raw's neutral HDR render (Adobe Standard, limit 4) of 194 CC0
  photos from 176 camera models of 16 makes announced 2016 or later, chosen for
  highlights above white (190 have factor-8 blocks above white; median 3.2% of the
  area), 51 validation, 47 holdout and 96 training, kept as 48-across block means.
  `hdr_gate.py` scores a build.

`main` f7d7ee9a on the gate set, the baseline (blocks; peak from factor 8):

| Split | Scored | Within every tolerance | Median above white, median / p90 (stops) | Peak over 0.1 stop | ΔE_ITP mean over 5 |
| --- | --- | --- | --- | --- | --- |
| Validation | 51 of 51 | 34 | 0.018 / 0.144 | 14 | 9 |
| Holdout | 45 of 47 | 31 | 0.024 / 0.130 | 11 | 9 |
| Training | 89 of 96 | 61 | 0.031 / 0.220 | 19 | 18 |

Nine photos do not render. Eight have no Adobe Standard in the profile library the
gate used: three Samsung phones, the DJI FC9287, the Panasonic DC-G100D and DC-TZ95D
and the Sony ZV-1. The Leica D-Lux 7's profile has sRGB-encoded tables, which HDR
refuses. The peak is the most frequent miss, as for the X100V and Z6 III above.

## Measurements

| Question | Result |
| --- | --- |
| Repeatability | Two captures with identical settings are bit-identical (all metrics 0). The reference noise floor is 0. |
| PQ quantisation | One 16-bit PQ code step is 0.0002 stops at graphics white and at 16× it. It is never the limiting tolerance. |
| Graphics white | Camera Raw's PQ output puts graphics white at 203 cd/m². With `HDRMaxValue` = L, the brightest pixel is 2^L × white: 16.0012 at L=4 and 1.99988 at L=1. |
| `HDRMaxValue` | Stops above graphics white. Camera Raw keeps 1, 2.37, 4, 5, 6 and 8. It replaces 0, 0.99, −1 and 10 with +2.30, and uses +2.30 when the key is absent. It writes `+4.00`. |
| HDR limit shape | A roll-off, not a clip. At L=1 the luminance p90 falls from 2.46 to 1.54 (it would be 2.0 if clipped), and the share of pixels above white barely changes (11.37% to 11.31%). |
| Midtones, HDR against SDR | With the same profile, luminance below 0.5 differs by a median 0.001 stop (p99 0.071). From 0.5 to 1.0, HDR is a median 0.074 stop brighter, and above white it extends to the limit. |
| SDR rendition | `SDRBrightness` +50 leaves the HDR master bit-identical. The sliders are −100 to +100 (`SDRBlend` too); 150 and −150 read as 0. Values set with HDR off are kept in the returned XMP. |
| Exposure +1 (HDR) | Luminance p50 0.075 → 0.204, p99 4.43 → 11.64, peak 5.16 → 13.11. |
| Highlights −100 (HDR) | p99 4.43 → 1.10, peak 16.0 → 8.41 (channel maximum). |
| Whites −100 (HDR) | p99 4.43 → 2.04, peak 16.0 → 11.65 (channel maximum). |
| HDR camera profile | The Camera Standard HDR DCP with HDR on renders different midtones from Camera Standard (p50 0.117 against 0.075) and peaks lower (channel maximum 3.82, luminance 3.46). |

These settle the XMP contract implemented in stage 2: units, ranges, defaults,
Camera Raw's replacement of invalid values, and the preservation of dormant values.

## Tolerances

The reference is deterministic, so every tolerance is a parity target for the
engine, not a measurement allowance. They are provisional until the frozen family
exists and must not be loosened to pass a candidate.

Comparisons use linear P3 normalised to 203 cd/m², after the same linear-area
reduction on both sides, per photo:

- **Exact contracts:** SDR rendition controls never change the HDR master (bit
  equality of RAWmakase's own HDR output). The rendered peak never exceeds
  2^limit × white, with 0.01 stop of quantisation allowance.
- **HDR tone (validation, then holdout):** for pixels whose reference luminance is
  above 1.0, the median |log2 luminance error| is ≤ 0.15 stop and the p95 is
  ≤ 0.35 stop. The peak is within 0.1 stop of the reference.
- **Below white:** the SDR gate (metric policy 2) applies to the SDR rendition.
  RAWmakase's HDR and SDR renders of one edit must agree below 0.5 luminance as
  closely as Camera Raw's do (median ≤ 0.002 stop, p99 ≤ 0.1 stop).
- **Colour:** mean ΔE_ITP ≤ 5 and p95 ≤ 15 (ITU-R BT.2124 units), reported with the
  luminance metrics. Saturated and gamut-edge patches are reported separately.

## Codecs

**JPEG XL encoder.** The plan's Rust-first evaluation found:

- `jxl-encoder` (pure Rust, VarDCT) is AGPL-3.0 or commercial. It is incompatible
  with RAWmakase's MIT licence and was not used or read.
- `zune-jpegxl` (MIT/Apache-2.0/Zlib) is a lossless modular encoder that writes a
  fixed sRGB header. A local spike patched its header to write enumerated colour
  encodings and a `ToneMapping` intensity target. PQ and HLG with P3 and Rec.2100
  primaries, and linear P3, all decode as signalled in libjxl 0.12 (`jxlinfo`,
  `djxl`) and jxl-oxide 0.12, losslessly (exact code values). Pure-Rust PQ/HLG
  signalling is therefore feasible.
- Size decides the export path. The 45 MP P3 PQ reference above is 197 MB as 16-bit
  lossless JXL (1.7 s), against 10.2 MB from libjxl at distance 1, effort 7 (19 s).
  The 16-bit TIFF is 273 MB. Lossless-only output is a valid interchange
  fallback, but not a credible replacement for the lossy JXL exports #894 asks for.

Decision, 2026-11-05: no Rust encoder qualified for lossy HDR JXL, and the libjxl
dependency was deferred to #906. #906 then asked for `jixel` to be evaluated first,
and libjxl used only if it didn't fit.

**jixel evaluation (#906, 2026-11-07).** `jixel` 0.5.7 (BSD-3-Clause OR Apache-2.0,
pure Rust, a port of libjxl-tiny with VarDCT and lossless modes) was measured on the
45 MP Z 8 photo above. It was rendered with Adobe Standard, `HDRMaxValue` 4, as
16-bit PQ and HLG masters in Display P3 and Rec. 2020 and encoded from the same
samples. cjxl 0.12 (distance 1, effort 7) was the size and quality oracle. Decoding
used libjxl 0.12's `djxl` and jxl-oxide 0.12.6. Errors are the stage 1 metrics of the
decoded light against the master's: linear P3, graphics white 1 (203 cd/m²), "above"
meaning luminance above white, "saturated" above white with one channel at least
twice another. They are per pixel at full resolution, and after a 4× linear area
reduction where marked. Encode times are on one Apple-silicon Mac (jixel with 16
threads, cjxl with its default).

| P3 PQ, 8256×5504 | Size | Encode | Above white, median / p95 (stops) | Saturated p95 (stops) | ΔE_ITP mean / p95 | ΔE_ITP mean, 4× |
| --- | --- | --- | --- | --- | --- | --- |
| Lossless (zune) | 197 MB | 1.7 s | exact | exact | 0 | 0 |
| cjxl d=1, e7 | 10.9 MB | 9.1 s | 0.005 / 0.028 | 0.031 | 6.2 / 17.0 | 2.57 |
| jixel d=1 | 8.0 MB | 2.9 s | 0.006 / 0.034 | 0.037 | 7.4 / 19.5 | 3.00 |
| jixel d=0.82 (quality 92, the export) | 9.1 MB | – | 0.006 / 0.032 | 0.035 | 6.9 / 18.1 | 2.65 |
| jixel d=0.6 | 11.6 MB | 2.7 s | 0.006 / 0.027 | 0.031 | 6.0 / 16.0 | 2.14 |

- **Size and speed.** At distance 1 jixel's file is 0.73× cjxl's and its error a
  little larger. At quality 92, the export's default, it is 0.84× the size and between
  cjxl's d=1 and jixel's d=0.6. At d=0.6 it matches cjxl's d=1 at 1.06× the size. It
  encodes 2–3× faster than cjxl e7. Rec. 2020 gives the same numbers to 0.001.
  The full-resolution ΔE_ITP is dominated by the photo's shadow noise for both
  encoders; the 4× reduction shows what remains.
- **Signalling.** `jxlinfo` reads P3 or Rec. 2100 primaries, PQ and 10,000 nits. With
  RAWmakase's header, it reads HLG and 1,000 nits. `djxl` and jxl-oxide decode PQ
  files to the same light (ΔE_ITP mean 6.87 and 6.82).
- **HLG is wrong as jixel 0.5.7 encodes it.** Its HLG input is linearised with the
  inverse OETF alone, as scene light 0–1, where libjxl expects display light at the
  intensity target (the OOTF, gamma 1.2 at 1,000 cd/m²). Both decoders then give back
  light 1.96 stops dark above white (cjxl's HLG file: 0.0016). XYB holds absolute
  light, so RAWmakase encodes an HLG export's display light as PQ, with the HLG
  intensity target, and replaces the header with one that says HLG. jixel's header is
  first checked against the one RAWmakase writes for that PQ frame, and refused if it
  differs. Through the export path at quality 92, HLG in P3 and Rec. 2020 is 9.1 MB, 0.002
  stop above white (median; p95 0.031), saturated p95 0.040–0.041, peak exact (`djxl`).
- **jxl-oxide and lossy HLG.** jxl-oxide clips XYB's light, in BT.709 primaries, to
  the intensity target before converting it to wider primaries. At HLG's 1,000 cd/m²
  a saturated Rec. 2020 highlight near the peak reads back clipped (a red at 855
  cd/m² as 581), where `djxl` gives it back; the codestream is the same at any
  intensity target. It also gives NaN for about 8,500 of the photo's pixels, as it
  does (15,400) for cjxl's own lossy HLG file. Both are decoder limits, not the
  encoder's. RAWmakase's reader is jxl-oxide, so its own lossy HLG exports read back
  that way.
- **Gain maps.** RAWmakase's `jhgm` writer wraps jixel's codestreams: an 8-bit sRGB
  base and a gray map, both lossy at the same distance. At quality 92 the file is
  10.3 MB, against 74 MB lossless (a 67 MB base and a 7 MB map). The map is 0.85 MB,
  within 2 codes at p95 (0.03 stop of its 3.75-stop range) and 3 at p99. `jxlinfo`
  reads the bundle (version 0, 61-byte metadata, the map's codestream), and `djxl`
  decodes the base and the map. jixel's own gain-map writer was not needed.
- **Dependency hygiene.** It has no dependencies with `default-features = false`, no
  build script and no C. Its licence is in cargo deny's allow-list. It needs Rust
  1.94, and the workspace's MSRV is 1.98.1. The app binary grows 3.6 MB stripped
  (51.8 to 55.4 MB, macOS arm64). Its `unsafe` was skimmed, not audited
  ([dependencies](../dependencies.md)).

Verdict: jixel fits, with the HLG workaround. Lossy JPEG XL (HDR PQ and HLG, gain
maps, SDR) is exported through it, at the Export dialog's quality; lossless stays
on zune. libjxl is not added.

**Gain-map bundle.** libjxl's `lib/extras/gain_map.cc` defines the `jhgm` box:
a version byte (0), a big-endian u16 size and the ISO 21496-1 metadata, a u8 size and
a bit-packed `ColorEncoding` for the alternate rendition, a big-endian u32 size and a
Brotli-compressed ICC, then the gain map as a naked codestream (the rest of the box).
The writer and reader need no libjxl. The map's codestream comes from the same
encoder as the base image: zune for a lossless export, jixel for a lossy one.

**Independent oracles available locally:** libjxl 0.12 (`cjxl`, `djxl`,
`jxlinfo`), jxl-oxide, libultrahdr 2.0.2 (`ultrahdr_app`: ISO 21496-1 and Adobe
gain-map JPEGs), libheif 1.23 (`heif-enc`: PQ/HLG HEIC with CICP), and exiftool.
They are used for fixtures and validation only; none is a runtime dependency.

## HDR rendering

Measured with Camera Raw 18.7 on synthetic gray-ramp DNGs (`probe_dng.py`, 192
patches over 14 stops, a linear embedded profile unless stated), captured as P3 PQ
16-bit through `hdr_reference.py`, graphics white 1. Probe sets are local; their
results are below and in the engine's measured tables.

**Scene tone stage.** With a linear profile the default HDR render is the scene
itself from 0.5 up (to 0.004 stop), with the SDR default black point's toe below,
at sensor whites 1, 4, 16 and 64 and limits 1 to 8. The limit rolls off only at the
top: scene 16 renders as 15.84 at limit 4; peaks above 49.26 are PQ's 10,000-nit
ceiling, not the limit. Exposure ±1 and ±2 are gains. Blacks −100 to +100 match the
SDR render below 0.25 (0.001 stop) and leave values above white alone. Negative
Whites is a gain over scene value, the same at sensor whites 4, 16 and 64, photo
maxima 0.5 to 8 and limits 2.3 and 8. Positive Whites follows the photo
([Whites](#whites)). Highlights and Shadows depend on the photo (Highlights −100
holds a ramp at 0.25–0.28 above 0.3); they were measured on photos instead
([Highlights and Shadows](#highlights-and-shadows)). Contrast follows the photo through
its pivot only ([Contrast](#contrast)).

**Ordinary profile curves.** Camera Raw does not extend an ordinary (SDR) profile's
curve by the DNG SDK's slope rule or the overrange rule. Measured on the ramp:

- Curves that never rise above the identity (linear, x^1.1, x^1.25, x², a knee of
  slope 0.5 then 1.5) render as the identity in HDR, ignoring the curve.
- Any brightening curve renders close to one common extension above white: at scene
  1.2 the ACR3 curve and 57 camera-matching curves render between 3.4 and 3.6 (the SDR
  curve gives 1). Curves that render brighter upper midtones render lower highlights.
- Absent and explicit ACR3 curves render identically, and the result does not depend
  on the sensor's white (white +2 against +4 agree).
- Gamma curves without a toe (x^0.6 to x^0.9) and steep filmic curves depart from
  the camera curves' extension: x^0.9, nearly the identity, renders scene 0.43 as 0.89.

RAWmakase renders the ACR3 curve's HDR extension as measured, and other curves as it
times a gain linear in the curve's log2 ratio to ACR3 at twelve inputs and at the
pixel's input, with weights per scene value fitted on the 58 camera-matching curves
(from 9 makers' installed DCPs, captured as probe curves only). Left out one at a
time, 52 curves have a median error above white of 0.05 stop or less; the worst
sample is 0.27 stop (Panasonic Cinelike V2). A curve whose twelve ratios are more
than 4 (Mahalanobis) from the fitted curves' is refused: that excludes the toeless
gammas and steep filmic curves the fit misses, and two of the 58.

**HDR profiles.** The DNG SDK rule (overrange encoding, RGBTone on encoded values,
slope extension above encoded 1, 3D tables on encoded values) renders Camera Standard
HDR within 0.01 stop median above white on six of six photos.

### Tone curves

Measured on gray ramps (sensor white +4 stops, a linear and the ACR3 profile curve,
HDR limits 2, 4 and 8), on the colour patches of [Looks](#looks), and on the twelve
photos of [Validation](#validation), through point curves and parametric settings in
the sidecar (`ToneCurvePV2012`, `Parametric*`).

- **Point curve.** Below white, the master curve renders exactly as in SDR on
  sRGB-encoded values (to 0.001 stop), also between 0.5 and white, where HDR's input
  differs from SDR's. Above white it continues along its slope at the last point, in
  the encoding a look's curve works in, and that slope is the natural spline's own
  derivative there before clamping: 1.33 for a curve through (128, 100), which
  brightens highlights; −0.054 for one through (230, 255), which overshoots white and
  so takes scene 15 down to 0.89. Nine curves (S curves, brightening, darkening,
  lifted and crushed blacks, knees, clips) render within a median 0.004–0.046 stop
  above white up to scene 16, worst 0.05, but for curves that end much steeper than
  the identity: Camera Raw holds their output back far above 16 (a curve through
  (128, 100) renders scene 11.8 as 25.8; the rule gives 31.2, 0.27 stop). Above
  scene 16, Camera Raw drifts toward the input, as for looks.
- **Parametric curve.** Its regions and splits are laid over black to four stops
  above white: the SDR curve over 0–1, scaled to the encoding of 16. Nine settings
  (each region ±50, Highlights +100 with its split at 60) render within a median
  0.016 stop, worst 0.063, below and above white; an axis to 22.6 or 32 is worse
  (median 0.07 and 0.13 above white). Values above 16 are left alone.
- **Order and limit.** The parametric curve applies before the point curve, as in SDR
  (an S curve with Lights +50: median 0.002 stop; the other order 0.20). Neither
  depends on the HDR limit (limits 2 and 4 as 8, within 0.05 stop below the limit's
  roll-off).
- **Colour.** On the colour patches, an S curve's and Highlights +50's effect matches
  Camera Raw's to a median 0.015 stop or less (p95 0.040) at saturations 0.3 and 0.6,
  ΔE_ITP 1.0 or less. At 0.9 the patches differ by ΔE_ITP 24 without a curve too
  ([Open](#open)).
- **Channel curves.** A single channel's curve renders by neither rule above white: a
  red curve through (128, 160), which brightens red in SDR, takes red from 4.1 to 1.2
  at scene 3.9 and to 0 by scene 12, while a blue curve through (128, 100) brightens
  blue and moves red and green too. They stay refused, as do Refine Saturation other
  than 100% and a point curve whose last point is left of white (not measured).
- **Photos.** A soft S curve with Highlights −20, Lights +15, Darks +10 and Shadows −10,
  and a brightening curve through (128, 160), Adobe Standard DCPs, HDR limit 4, score
  as the default render does on the same photos: median above white 0.004–0.076 stop,
  and the same eight of twelve pass every tolerance. The four that fail fail by the
  reasons under [Validation](#validation) (the Panasonics' p95 0.24–1.48 and ΔE_ITP,
  the X100V's rebuilt highlight peak +0.22–0.24, the Pentax's ΔE_ITP); the Z6 III's
  peak, +0.108 by default, is +0.056 and +0.070.

**Levels.** RAWmakase's black point, white point and midtone have no Camera Raw
counterpart to measure. Decision: they render in HDR as Camera Raw renders a point
curve of the same shape, by the rule above: as in SDR up to white, then along the
slope at white, 1 / (midtone (1 − black)); a white point below white leaves a slope of
0, as Camera Raw's curve through (230, 255) holds white (0.89 by scene 15).

### Presence

Measured on five validation and holdout photos (Nikon Z 8, Z f and Z5 II, Sony A7R III,
Canon 850D; Adobe Standard DCPs, HDR limit 4) with Texture +50, Clarity +50 and Dehaze
±30, in HDR and in SDR. The effect is each render's luminance against the same photo's
default render, ours against Camera Raw's, in stops (8× linear blocks).

| Slider | SDR effect error, median | HDR effect error, median | HDR tolerances (above white median / p95, peak) |
| --- | --- | --- | --- |
| Texture +50 | 0.001–0.027 | 0.001–0.029 | all pass but the 850D's peak, +0.108 (its default +0.025) |
| Clarity +50 | 0.07–0.54 | 0.08–0.55 | A7R III 0.47 / 0.53; Z 8 p95 0.85, peak +0.70; Z f peak +0.39; 850D peak −0.30 |
| Dehaze +30 | 0.08–0.19 | 0.09–0.19 | A7R III p95 0.52 |
| Dehaze −30 | 0.04–0.24 | 0.04–0.26 | A7R III 0.70 / 1.07, peak +0.99 |

- **Texture renders in HDR.** It works on the camera's samples before tone, and its
  HDR effect matches Camera Raw's as closely as its SDR effect does; median and p95
  above white are within the tolerances on all five photos, and the 850D's brightest
  block is 0.008 stop past the peak tolerance.
- **Clarity and Dehaze stay refused.** Their SDR operators already depart from Camera
  Raw's effect by up to 0.54 and 0.24 stop median on these photos, and in HDR they
  fail the tolerances on four photos (Clarity) and one (Dehaze, by up to 1.07 stop
  at p95). They need their SDR parity first.

### Looks

Measured on gray ramps (sensor white +4 and +6 stops, a linear and the ACR3 profile
curve, HDR limits 2, 4 and 8) and on colour patches (12 hues, saturations 0.3 to 0.9,
0.25 to 8 times white), through synthetic looks read from the sidecar and Adobe Color.

- **Table.** A look's HSV table applies in HDR as the DNG SDK applies an HDR
  profile's: a table with value divisions looks up overrange-encoded values, linear
  or sRGB-flagged, and a 2.5D table scales linear values. A synthetic table that
  scales value by 1 − 0.3 v over eight divisions renders scene 15.2 as 2.24 (the
  rule gives 2.25) and 0.98 as 0.729 (0.728); a 2.5D scale of 0.8 is a gain of 0.8
  at every value.
- **Curve.** A look's curve applies after the profile's curve, depending only on its
  input (the linear and ACR3 profiles give the same mapping, as do limits 2, 4 and 8).
  It works in an encoding that is sRGB's up to graphics white and
  1 + (a/b) log2(1 + b log2 x) above it (a = 0.19207, b = 0.08544). A curve whose last
  point is white follows itself up to where it lifts its input most, then runs
  parallel to the identity at that lift, at most 16. A curve that never lifts its
  input (one that only darkens) therefore renders as the identity, below white too.
  A curve that ends below white keeps its shape to white and continues along its
  slope. Sixteen synthetic curves (lines, clips, knees, lifted blacks, S curves)
  and Adobe Color's render within 0.063 stop up to scene 15.5, median 0.006. The
  encoding's constants were fitted on four of them; the rest hold within the same
  error. Above 16, Camera Raw holds a curve that ends at white near 16, drifting
  by up to 0.25 stop by scene 32; RAWmakase holds it at 16.
- **Colour.** The curve applies RGBTone fashion on encoded values, as in SDR. On the
  colour patches the effect of an S curve, a hue-and-saturation table and both
  together match Camera Raw's to a median of 0.009 stop (p95 0.095) and a median
  ΔE_ITP of 1.4 or less at every intensity.
- **Photos.** Adobe Color on the twelve photos, and Adobe Neutral and Adobe Portrait
  on the six validation photos, Adobe Standard DCPs, HDR limit 4, score as Adobe
  Standard does without a look (median above white 0.007–0.081, the same photos
  failing for the same reasons listed under [Validation](#validation)). Adobe
  Neutral's curve never lifts: Camera Raw's HDR render departs from its SDR render
  below 0.5 by a median of 0.02–0.24 stop, and RAWmakase's departs by the same
  amount, to 0.004 stop.

Refused until measured: looks with an RGB table (`.cube` looks, Lightroom's
creative looks), looks with their own sliders or settings (Adobe Vivid and Adobe
Landscape carry Clarity, Highlights and Shadows), Black & White looks, fitted Camera
Looks (no Camera Raw counterpart) and Profile Amounts other than 100%.

### HDR files

Measured with Camera Raw 18.7 (Process Version 15.4) on a synthetic 16-bit PQ TIFF in
BT.2020 primaries, with a CICP-tagged ICC profile (9/16/0/1): 25 greys from 0.01 to
40.96 in half stops and four colours, as 64-pixel patches on a 0.18 surround,
captured as P3 PQ. Settings reach Camera Raw only through Open As: an ordinary open
of a TIFF skips Camera Raw, and Camera Raw ignores the file's own and a sidecar's
XMP. Each capture's settings were checked in the XMP Camera Raw wrote. Probe files
and captures are local.

| Setting (HDR on, limit 6 unless stated) | Camera Raw's render, against the file's light |
| --- | --- |
| None | The light, ×1.0028 at every level and on every channel (×1.0002 to ×1.0023 on other files) |
| Limit 2.3 | Each channel clipped at 2^2.3; limit 6 clips only at PQ's 49.26 |
| Exposure ±1, +2 | Gains of ±1 and +2 stops at every level and colour, below the clip |
| Temp +20 (Custom; As Shot ignores it) | A gain per channel at every level: ProPhoto +0.459, +0.379, +0.003 stop |
| Whites −50 | −0.006 stop at 0.01, −0.25 at 0.9, −0.46 at 10, −0.50 at 41; flat fields agree |
| Blacks −50 | Crushes below 0.03 (−0.94 stop at 0.04), nothing above 1 |
| Contrast +50 | −1.05 stop at 0.01, a pivot near 0.11, +0.2 to +0.27 from 0.16 to 15, +0.10 at 41 |
| Highlights −50, Shadows +50, Clarity +50 | Local: no change on flat fields at 2, 8 and 30 |
| Saturation +50 | Greys unchanged, chroma ×1.3 to ×1.5 |
| HDR off | Greys as they are to about 0.23, then rolled off by the content (0.9 → 0.66, 41 → 0.9997) |

A perfectly flat file below white decodes as about twice its light squared in Camera
Raw (0.1 → 0.02); one bright spot restores a linear decode, so the flat-field
controls at 0.02, 0.1 and 0.5 are left out.

This is how the raw HDR master renders a scene through a linear profile
([above](#hdr-rendering)): the scene itself from 0.5 up and Exposure as a gain. The
raw HDR Whites −50, measured on raw ramps through a linear profile, gives the file's
greys within 0.005 stop up to 10 and within 0.021 stop above (the raw ramps end at
scene 12). Temp +20's gains are those of the rendered-file white balance measured on
Lightroom's SDR renders (`rendered_gains`: +0.458, +0.378, +0.003 stop) to 0.002 stop.

### Contrast

Measured on gray ramps (sensor white +4 and +6 stops, ramps topping at scene 1, 4,
16 and 64, a linear and the ACR3 profile curve, HDR limits 2, 4 and 8) at Contrast
−100, −75, −50, −25, +25, +50 and +100, against the SDR render of the same ramp.
Contrast in HDR depends neither on the HDR limit nor on the profile's curve: it maps
the profile's output, as SDR Contrast does, at the pivot of the SDR rendering (the
deep shadows of positive Contrast are the SDR render's to 0.001 stop).

- **Positive Contrast** renders as a look's curve does ([Looks](#looks)), from its
  SDR curve: the curve up to where it lifts its input most, then parallel to the
  identity at that lift, in the look curve's encoding, but with no ceiling. Within
  0.01 stop up to scene 8 and 0.1 stop up to 15.5. Above 16 Camera Raw adds a
  constant in linear light (+3.0 at +50, +6.1 at +100).
- **Negative Contrast** is not the look rule (which would lift highlights it pulls
  down), nor the inverse of positive Contrast in HDR, though it is in SDR. It is its
  SDR curve stretched in stops around the curve's fixed point: the fixed point moves
  up by a factor of 1.671 and graphics white goes to 16, so the HDR range is
  flattened as the SDR range is, and the shadows are lifted more than in SDR (×6.0
  at scene 0.005 at −100, against ×3.6). The factor is the same on every ramp and
  slider value; fitted on eleven cases it renders them within a median of 0.019 stop
  (p95 0.07; the largest error is −100 at scene 1–2, 0.11 stop dark). Above 16 it
  continues along its slope.
- **Photos.** Contrast −100, −50, +50 and +100 on the six validation photos and ±50
  on the six holdout photos (Adobe Standard, HDR limit 4) stay within the tolerances
  where the default render does: median above white 0.007–0.13 stop, p95 0.03–0.26,
  and the same photos fail for the same reasons as the default (the Panasonics'
  and Pentax's SDR gap, the X100V's and Z6 III's rebuilt speculars). Below 0.5 the
  master follows Camera Raw's to a median 0.02–0.13 stop, as far as SDR Contrast
  itself does (the Sony A7R III's SDR Contrast is 0.17–0.27 stop off at ±100, and its
  HDR inherits it). At −100 Camera Raw's HDR lifts the shadows over its SDR render by
  a median 0.34–0.44 stop, RAWmakase's by 0.40–0.47.
- **Colour.** On the colour patches Contrast's effect differs from Camera Raw's by a
  median of 0.24 stop at a quarter of white in SDR and HDR alike: SDR Contrast's
  pivot on this synthetic image, not the HDR rendering (the pivot fit follows photos,
  not synthetic scenes; see `basic_tone::RENDERED_PIVOT_OFFSET`).

### Whites

Positive Whites, measured on 158 renders of gray ramps (linear profile, HDR limit 8
unless stated) and on the twelve photos of [Validation](#validation):

- **One level.** Positive Whites is a gain over the scene value that depends on one
  photo level. Ramps peaking at 0.25 to 32 at sensor whites 16 and 64, and ramps whose
  sensor white is their peak (0.5 and 4), give one family: from a peak of 4 up it no
  longer changes (+100 brightens scene 1 by 1.32 stops and scene 8 by 1.88, as the
  earlier single-ramp measurement had it); below, it stretches further, +3.9 stops at
  scene 0.3 on a ramp peaking at 0.5 and +5.4 at 0.2 on one peaking at 0.25. Fitted
  by that one level, ramps with bright patches added (one to 36 at 0.6 to 8, a flat
  top, a background of 0.01 to 0.4) render as the family does: below the level to a
  median of 0.000–0.009 stop, above it within 0.15 stop (where the PQ output is below
  10,000 cd/m²). The HDR limit only rolls the result off (limits 2.3 and 4 give the
  same gains below them), and Exposure moves the level: a ramp peaking at 0.5 at
  Exposure +1 renders as one peaking at 1.
- **The table.** `scene_tone::hdr_whites` holds the gains over the Whites 0 render
  at +25, +50, +75 and +100, at levels 1/8 to 4 in quarter stops, over scene values
  2^−14 to 2^7. Above the brightest value a level's ramps measured (Camera Raw's PQ
  output reaches 10,000 cd/m² there) the gain stays at its last value; the output
  never decreases.
- **The photo's level.** On the ramps it is rank-based: patches at 1, 2 and 8 that
  cover the same area give the same render, and a background covering half the frame
  does not count. No single percentile of the ramps reproduces it (the best pair of
  percentiles misses by up to 1.3 stops): their level is their peak when it is sparse,
  and at most about 0.75–0.95 stop above the ramp's top when bright patches are added.
  On photos, the level that reproduces Camera Raw's Whites +100 was found by
  rendering each photo at forced levels: twice the 99th percentile of luminance (the
  level SDR's Whites stretches toward) predicts it with a standard deviation of 0.17
  stop and a largest error of 0.35 (Nikon Z5 II); the 98th percentile does as well
  (0.17), the 99.5th worse (0.19), the 99.9th much worse (0.51). RAWmakase uses twice
  the 99th percentile, without SDR's limit at the sensor's white. On sparse synthetic
  ramps it therefore stretches less than Camera Raw (a ramp peaking at 0.5 is given
  level 0.82 where Camera Raw's is 0.5).
- **Validation.** Twelve photos at Whites +25, +50, +75 and +100 (Adobe Standard,
  limit 4): median and p95 above white and the peak, in stops, outside the
  tolerances in bold.

  | Photo (split) | +25 | +50 | +75 | +100 |
  | --- | --- | --- | --- | --- |
  | Nikon Z 8 (validation) | 0.060 / 0.095 / +0.081 | 0.059 / 0.093 / +0.081 | 0.060 / 0.092 / +0.013 | 0.000 / 0.096 / +0.050 |
  | Nikon Z f (validation) | 0.001 / 0.003 / +0.011 | 0.017 / 0.019 / +0.013 | 0.064 / 0.071 / -0.004 | 0.114 / 0.122 / +0.049 |
  | Nikon Z6 III (validation) | 0.011 / 0.085 / +0.064 | 0.030 / 0.107 / +0.000 | 0.113 / 0.204 / **+0.157** | **0.183** / 0.323 / **+0.308** |
  | Panasonic S1M2 (validation) | 0.077 / **0.442** / +0.005 | 0.116 / **0.580** / +0.029 | **0.152** / **0.777** / **+0.230** | **0.168** / **1.027** / **+0.219** |
  | Sony A7R III (validation) | 0.026 / 0.070 / +0.001 | 0.029 / 0.035 / +0.038 | 0.066 / 0.120 / +0.034 | 0.115 / 0.194 / +0.042 |
  | Canon 850D (validation) | 0.037 / 0.041 / +0.032 | 0.048 / 0.053 / +0.045 | 0.062 / 0.070 / +0.061 | 0.079 / 0.091 / +0.079 |
  | Nikon Z50 II (holdout) | 0.034 / 0.053 / **+0.161** | 0.096 / 0.131 / **+0.133** | **0.198** / 0.230 / **+0.245** | **0.301** / 0.337 / **+0.125** |
  | Nikon Z5 II (holdout) | 0.047 / 0.074 / +0.052 | **0.185** / 0.258 / **+0.222** | **0.640** / **0.775** / **+0.104** | **0.751** / **1.220** / **+0.324** |
  | Panasonic S1M2ES (holdout) | 0.089 / **1.707** / -0.028 | 0.120 / **2.044** / -0.033 | **0.172** / **2.955** / +0.009 | **0.200** / **3.952** / +0.023 |
  | Fujifilm X100V (holdout) | 0.036 / 0.099 / **+0.405** | 0.127 / 0.218 / +0.004 | **0.272** / 0.343 / **+0.147** | **0.337** / **0.498** / **+0.312** |
  | Sony A7R IV (holdout) | 0.050 / 0.080 / +0.043 | 0.052 / 0.093 / +0.069 | 0.036 / 0.066 / **+0.141** | 0.025 / 0.078 / +0.020 |
  | Pentax K-1 II (holdout) | 0.029 / 0.037 / +0.037 | 0.067 / 0.172 / +0.037 | **0.203** / **0.492** / -0.024 | 0.124 / **0.871** / +0.096 |

  The level's error is amplified by Whites: at +100 a 0.17-stop error in the level
  is 0.3–0.4 stop above white. At +50, eight of twelve photos pass every luminance
  tolerance, as many as at Whites 0. The two Panasonics fail as they do at Whites 0
  (their SDR gap). Two holdout photos fail because of Whites: the Z50 II's peak is
  0.133 stop high (0.082 at Whites 0), and the Z5 II, with 0.1% of its pixels above
  white, is 0.185 stop off (median) where its level is 0.35 stop from Camera Raw's.
  At +75, five photos fail on Whites.

Decision, 2026-11-09: the HDR master renders Whites up to +50 and refuses higher
values (`model::hdr::check_recipe`) with the reason. Raising the limit needs a better
predictor of Camera Raw's level than twice the 99th percentile.

SDR renders do not change: positive Whites' table and level are read only in the HDR
branch of the scene tone stage (`SceneTone::new`, `GlobalTone::hdr`), which SDR
renders, the SDR rendition among them, never take. As a targeted regression check,
builds of the change and of the main commit it was made on rendered 60 dataset
photos with four sidecars (defaults, SDR Whites +50 and +100, and an HDR edit's SDR
rendition at Whites +50) to identical pixels, 240 of 240. The dataset gate (scored
against Camera Raw) and render snapshots (about 1,800 photos, compared against earlier
RAWmakase renders) were not run for this change.

### Highlights and Shadows

Measured on the six validation photos (Adobe Standard, HDR limit 4) at Highlights −100,
−50 and +50 and Shadows −50, +50 and +100, and Shadows +50 on the six holdout photos,
each against the photo's default HDR and SDR renders. Camera Raw's gain (adjusted
over default, per pixel of the reduced renders) is the same in HDR and SDR below about
half of white on every photo; above white SDR's shoulder hides it.

- **Shadows** renders as RAWmakase's SDR operator does, its scene-value gain unclipped:
  the gain per level agrees with Camera Raw's to about 0.1 stop, and at the top of
  the range Camera Raw adds 0–0.02 stop, RAWmakase 0.02–0.05. All three settings stay
  within the median and p95 tolerances above white where the default render does
  (median 0.005–0.09 stop, p95 ≤ 0.28; the Panasonic fails as by default), and
  RAWmakase's HDR and SDR renders agree below 0.5 as Camera Raw's do. Below white the
  SDR operator's own error carries over (Canon 850D Shadows +100: 0.29 stop in SDR).
  The peak tolerance (0.1 stop) fails on photos whose default peak passes: Sony A7R IV
  +0.28 at Shadows +50 and Nikon Z 8 +0.22 at +100 (+0.03 and +0.08 by default), and
  the Z6 III's +0.19 at +100 is further from it than its default +0.11. Each is a
  small specular in dark surroundings, which positive Shadows lifts and Camera Raw's
  base keeps; the brightest 0.01% of pixels agree to 0.03 stop. Shadows renders in HDR
  with this limitation; the peak failures stay open.
- **Highlights** stays refused. Above white Camera Raw's HDR gain changes shape:
  negative Highlights levels off a stop or two above white (−100: about −2.3 stops on
  the Z 8 and A7R III, −1.2 on the Z6 III and Panasonic, −1.8 on the 850D), and
  positive Highlights peaks just above white and fades (+50: +1.4 stops at 1.4 times
  white on the Z 8, +0.6 at 6 times). RAWmakase's negative-Highlights rule, trained
  on SDR targets, is already 0.3–0.5 stop off at white on some photos (A7R III −0.89
  against −1.37, Z6 III −1.23 against −0.74) and keeps growing above it, and positive
  Highlights' table tail does not fade: median above white 0.26–0.68 stop at −100 and
  −50, 0.09–0.83 at +50. It needs targets above white from more photos than these.

### Validation

`scripts/corpus/hdr_validate.py` renders each HDR job of the family with
`rawmakase render --hdr` and compares as the references were reduced (factor 8,
linear Display P3). Twelve CC0 raw.pixls.us photos, Adobe Standard (the ACR3 curve
and the camera's look table) and Camera Standard HDR, HDR limit 4 unless stated.
`rel` compares how far HDR departs from SDR, ours against Camera Raw's, which
separates the HDR extension from differences both renders share; `SDR` is the SDR
render's own median luminance error against Camera Raw.

| Photo (split) | Default above white: median / p95 | rel p95 | peak | ΔE_ITP mean / p95 | SDR |
| --- | --- | --- | --- | --- | --- |
| Nikon Z 8 (validation) | 0.061 / 0.096 | 0.094 | +0.078 | 2.9 / 7.0 | 0.010 |
| Nikon Z f (validation) | 0.007 / 0.009 | 0.018 | +0.007 | 0.9 / 2.9 | 0.006 |
| Nikon Z6 III (validation) | 0.014 / 0.131 | 0.129 | **+0.108** | 1.4 / 3.7 | 0.011 |
| Panasonic S1M2 (validation) | 0.061 / **0.395** | 0.271 | −0.012 | **27.4 / 110** | **0.116** |
| Sony A7R III (validation) | 0.032 / 0.075 | 0.072 | −0.011 | 0.9 / 1.9 | 0.005 |
| Canon 850D (validation) | 0.032 / 0.035 | 0.034 | +0.025 | 1.6 / 2.4 | 0.028 |
| Nikon Z50 II (holdout) | 0.012 / 0.020 | 0.008 | +0.082 | 1.3 / 2.7 | 0.018 |
| Nikon Z5 II (holdout) | 0.017 / 0.025 | 0.012 | −0.011 | 2.2 / 4.5 | 0.010 |
| Panasonic S1M2ES (holdout) | 0.078 / **1.55** | **0.444** | −0.030 | **24.1 / 91** | **0.220** |
| Fujifilm X100V (holdout) | 0.030 / 0.063 | 0.051 | **+0.360** | **5.6 / 19.4** | 0.041 |
| Sony A7R IV (holdout) | 0.045 / 0.061 | 0.080 | +0.026 | 1.4 / 3.9 | 0.013 |
| Pentax K-1 II (holdout) | none above white | — | +0.025 | **9.6 / 29.7** | **0.121** |

Camera Standard HDR: median above white 0.001–0.008 on the four Nikons and the
Z50 II/Z5 II, 0.05–0.09 on the two Panasonics. Exposure ±1, Whites −100, Blacks −100
and limits 2.3 and 6 stay within the tolerances on the same photos as the default.
Below 0.5, RAWmakase's HDR and SDR renders agree as Camera Raw's do on every photo
(p99 0.005–0.28 against Camera Raw's 0.016–0.289 on the same photos; medians ≤ 0.003).

Re-run after the limit was made to apply once, last: every number above is unchanged to 0.01 stop (limit 2.3 on the Z 8: median 0.051, p95 0.099).

Eight of twelve photos pass every tolerance. The failures are not the HDR extension:

- The two Panasonics and the Pentax have SDR errors of 0.12–0.22 stop median against
  Camera Raw already (a pre-existing SDR gap for these bodies); HDR inherits them.
- The X100V's and Z6 III's peaks are specular highlights rebuilt above the sensor's
  clip. SDR clips them at white, HDR shows the reconstruction's values (known to
  differ from Camera Raw's; decode, not tone).

### SDR rendition

Camera Raw's TIFF output carries no SDR rendition, but its Save Image dialog does: a
JPEG saved with HDR Output on (Maximize Compatibility is then always on) is an 8-bit
sRGB base with an ISO 21496-1 gain map, and the base is the SDR rendition. Measured on
the CC0 Z 8 photo (Adobe Color, HDR limit 6, sliders at 0 unless stated), the bases
compared by luminance in 8× linear blocks:

| SDR base against the HDR-off render | Median | p05 | p95 |
| --- | --- | --- | --- |
| Luminance below 0.05 (41% of the photo) | +0.17 stop | −0.44 | +0.76 |
| 0.05–0.18 (26%) | +0.18 | −0.12 | +0.52 |
| 0.18–0.5 (20%) | −0.02 | −0.39 | +0.25 |
| 0.5–0.9 (3%) | −0.22 | −0.73 | −0.09 |
| 0.9–1 (11%) | −0.19 | −0.44 | −0.13 |

- **The default SDR rendition is not the HDR-off render.** It lifts shadows and holds
  the top below white: median 0.20 stop from the HDR-off render, p95 0.67. After the
  best curve of luminance alone is taken out, a median 0.12 stop (p95 0.49) remains:
  it is a local tone map of the HDR master, not a global curve.
- **The sliders adjust that tone map.** `SDRBrightness` +50 brightens the base by a
  median 0.44 stop (+0.68 below 0.05 luminance, −0.08 at white), and that change is a
  curve of luminance to 0.010 stop (p95 0.051). Camera Raw 18.7 also has an Auto SDR
  that searches these sliders per photo.
- **Decision: the sliders stay refused.** Rendering them on RAWmakase's SDR rendition
  (the HDR-off render) would move it further from Camera Raw's, not closer: the base
  they adjust is the unmatched part. Matching Camera Raw's default SDR rendition needs
  its local tone map measured on many photos first. The SDR rendition stays the
  HDR-off render, a median 0.2 stop from Camera Raw's on this photo; gain-map exports
  carry it as their base.

### Colour stage

Measured with Camera Raw 18.7 on synthetic colour patches through `probe_dng.py`
(12 hues, HSV saturations 0.3, 0.6 and 0.9 and a gray, at 0.125 to 8 times white,
and densely from 0.2 to 3; a linear profile and the ACR3 curve; HDR limit 8 and SDR),
with each control at its extremes: Saturation and Vibrance ±50 and ±100, every
band's Hue, Saturation and Luminance at ±100 (and single bands), Calibration's
primaries and Shadows tint, and Black & White with a zero and a strong mix.
Camera Raw renders Saturation −100 and Black & White in SDR as grayscale
documents, which the harness doesn't read, so those were captured in HDR only.

- **Below 0.25 the HDR and SDR renders agree** for every control (median ΔE_ITP
  0.04–0.3, the default's 0.22). From 0.5 up, SDR rolls off what the controls
  brighten (stage 1's 0.074-stop gap between 0.5 and white), so the two differ:
  Saturation +100 at 0.5, ΔE_ITP 8–27.
- **Hue and saturation changes don't depend on brightness in HDR.** Saturation +100
  multiplies HSV saturation by 1.73 (saturation 0.3) at 0.125 and 1.74 at 8 times
  white, and value by 1.15 at all of them; its SDR render gives 1.78 and 1.13 at 0.5.
  The colour mixer's grids were fitted on SDR renders and carry the roll-off in
  their top values. Looking them up at the colour's value only up to 0.2 and at 0.2
  above it (`color_mixer::HDR_VALUE`; 0.15–0.3 score alike) halves the error above
  white: Saturation +100 32–35 → 16–17 median ΔE_ITP, the colour mixer's Saturation
  +100 31–35 → 13–15, Vibrance +100 25 → 8, Saturation −50 6 → 2, Hue ±100 3.5 → 3.4.
  These match the grids' own error at low values (12, 10, 7.7, 1.7 and 2.6), which
  is SDR's.
- **Vibrance's value change applies below white only:** ×1.06–1.09 at 0.125–0.5,
  ×1.01 from 1 up. It fades from 0.5 to white (`color_mixer::HDR_FADE`; where the
  fade starts and ends changes the error by less than 0.3).
- **The colour mixer's Luminance is not a gain above white.** Luminance +100 of every
  band: value ×1.67 at 0.5 and ×1.00 at 1, 2, 4 and 8, but a colour at white turns
  almost gray (saturation ×0.06) and one at twice white loses a little saturation;
  Camera Raw changes the channels below about white and not the ones above. No
  lookup or fade rule fitted it: through the ACR3 curve the best left 30–60 median
  ΔE_ITP between 0.4 and 1.25 times white (the SDR grids' error at 0.2: 12–28).
  **Decision: band Luminance stays refused in HDR.**
- **Black & White:** the mix's gray with the same lookup, unclipped above white: a
  strong mix 12.7 → 5.5 above white (4.8 at 0.125), a zero mix 3.2 → 0.9.
- **Calibration renders as in SDR** with no change: its matrix works on scene colour
  before Exposure, and Shadows tint fades out below white. Hue +100 of red 1.3,
  saturation −100 0.7, a mix 6.0 above white (SDR 1.0, 0.5, 4.9).

The rendered values match a Python port of the rule to ΔE_ITP 0.8.

**Photos.** Saturation +30 and −40, Vibrance +40, a colour mixer edit (Red Hue +15,
Orange Saturation +30, Green Saturation −40, Blue Hue −20 and Saturation +25),
Calibration (Red Hue +20, Green Saturation −15, Blue Saturation +30) and Black &
White (the gray ramp's Auto mix), Adobe Standard, HDR limit 4, on the twelve photos
of [Validation](#validation):

| Setting (split) | Photos | Above white, median / p95 (worst) | Peak (worst) | ΔE_ITP mean / p95 (worst) |
| --- | --- | --- | --- | --- |
| Default (validation) | 5 | 0.061 / 0.131 | +0.108 | 2.9 / 7.0 |
| Saturation +30 (validation) | 5 | 0.068 / 0.138 | +0.110 | 5.3 / 14.8 |
| Saturation −40 (validation) | 5 | 0.058 / 0.125 | +0.104 | 1.9 / 5.1 |
| Vibrance +40 (validation) | 5 | 0.064 / 0.121 | +0.107 | 3.8 / 9.3 |
| Colour mixer (validation) | 5 | 0.061 / 0.151 | +0.107 | 3.0 / 7.1 |
| Calibration (validation) | 5 | 0.060 / 0.121 | +0.108 | 3.3 / 7.7 |
| Black & White (validation) | 5 | 0.079 / 0.183 | +0.103 | 3.0 / 14.4 |
| Default (holdout) | 3 | 0.045 / 0.061 | +0.082 | 4.0 / 8.2 |
| Saturation +30 (holdout) | 3 | 0.041 / 0.051 | +0.065 | 4.6 / 9.6 |
| Saturation −40 (holdout) | 3 | 0.047 / 0.071 | +0.106 | 2.3 / 4.6 |
| Vibrance +40 (holdout) | 3 | 0.018 / 0.042 | +0.032 | 5.6 / 11.9 |
| Colour mixer (holdout) | 3 | 0.032 / 0.043 | +0.035 | 2.4 / 6.7 |
| Calibration (holdout) | 3 | 0.042 / 0.061 | +0.036 | 4.2 / 9.0 |
| Black & White (holdout) | 3 | 0.073 / 0.094 | +0.125 | 2.5 / 6.3 |

Worst case over the photos whose default render passes; the two Panasonics, the K-1 II
and the X100V, which fail at their defaults for the reasons under
[Validation](#validation), score with each control as at their default (median above
white within 0.03 stop of it).

- Luminance above white stays within its tolerances (median ≤ 0.15, p95 ≤ 0.35 stop)
  with every control on these photos.
- The peak tolerance (0.1 stop) is exceeded on two photos. The Z6 III's peak is
  +0.108 at its default (rebuilt highlights) and +0.103 to +0.110 with the controls.
  The Z50 II's +0.082 becomes +0.106 with Saturation −40 and +0.125 with Black & White,
  while its median above white stays at 0.004–0.006 stop. The peak is the brightest
  reduced pixel.
- Colour stays within mean 5 and p95 15 except Saturation +30 on the Z 8 (mean 5.3,
  default 2.9) and Vibrance +40 on the Z5 II (5.6, default 4.0). On the Z 8, Saturation
  +30's added error is mostly below white: mean ΔE_ITP below 0.18 luminance 2.5 → 4.4,
  from 0.18 to white 2.9 → 7.6, above white 5.1 → 5.4. Looked up as SDR does (the A/B
  below), the same edit scores 4.87, so most of the added error is in the Saturation
  grid itself rather than in the cap; the cap adds 0.4.

**Decision: Saturation, Vibrance, the colour mixer's Hue and Saturation, Calibration
and Black & White render in the HDR master;** the colour mixer's Luminance stays
refused.

On the validation photos the Saturation +30 lookup was also compared with the grids
looked up as SDR does (up to white): that scores slightly closer for Saturation +30
(Z8 ΔE_ITP mean 4.87 against 5.26, above white 0.053 against 0.068 stop) and further
for Vibrance +40 (Z8 above white 0.121 against 0.064). The capped lookup stays, as the
patches above show it at larger slider values.

**SDR is unchanged.** SDR renders and the GPU look the grids up as before
(`Lookup::Sdr`); an HDR lookup exists only while a master renders. Rendered by the parent commit and by
this change, 12 photos of the dataset's render corpus (every 150th, 600 px), each with
a colour edit (Saturation, Vibrance, the colour mixer's Hue, Saturation and Luminance,
Calibration and Shadows tint) and a Black & White edit, with HDR off and with HDR on
(its SDR rendition), give 48 of 48 bit-identical renders. The render snapshots and
the SDR photo gate were not run: they score these same SDR renders.

### Decisions

- **The SDR rendition is the edit with HDR off.** Camera Raw's own is a local tone
  map of the HDR master ([SDR rendition](#sdr-rendition)); the SDR rendition sliders
  refuse the master until it is matched.
- **Unmeasured adjustments refuse the HDR master, not the edit.** An imported or
  edited HDR edit with Clarity, a look or Whites above +50 keeps HDR on and stays
  valid; its SDR rendition renders it, and the master (HDR export) is refused with
  the reason, shown in Develop under the HDR switch. Refusing the edit itself would
  make Develop's ordinary sliders fail to render whenever HDR is on.
- **HDR HEIC (stage 4)** opens as an HDR edit where the system decoder was measured
  to give its signal back as coded: macOS's ImageIO drawn into CoreGraphics' space of
  the file's CICP (every patch of the synthetic fixtures within one 10-bit code value),
  and libheif, which converts by the `nclx` alone. Windows' decoder isn't checked
  here, so an HDR HEIC stays refused there; so does one with an ICC profile beside its
  `nclx`, which ImageIO would draw through, or with a matrix that isn't a matrix
  (constant luminance, ICtCp).
- **`hdrgm` gain maps (stage 4)** are read like ISO 21496-1 ones, with these choices:
  - Located by the MPF index, as for ISO; Ultra HDR's GContainer item lengths alone are
    not read, since its specification requires writers to add the MPF index.
  - ISO 21496-1 metadata wins when a file has both forms, as libultrahdr decides.
  - Offsets below 0 are damaged, as hdrgm forbids; hdrgm sets them no upper bound, so
    one up to 2^16 is accepted (ISO 21496-1 maps keep their 0–1 check).
  - A map whose XMP fails a check refuses the file for Develop by reason, as a damaged
    ISO map does, rather than open its SDR base as Android's specification lets a
    viewer: an edit of the base alone would be lost when the map is read correctly.
- **HEIC gain maps (stage 4)** open as HDR edits on macOS only, where ImageIO decodes
  the map with the image; libheif and Windows' decoder aren't asked for it here, so
  those systems keep opening the SDR base. An ISO 21496-1 `tmap` wins over Apple's own
  map. Apple's map follows Apple's documentation (headroom from maker-note tags 33 and
  48, `sdr × (1 + (headroom − 1) × map)`, the map Rec. 709-encoded), not ImageIO's
  rendering of it, which differs. An Apple map without its version XMP, without both
  maker-note values, or with no headroom, isn't applied, as Apple's documentation
  calls it invalid: the photo opens as its SDR base, as before, rather than being
  refused. A cropped image with a map is refused, as the map's frame isn't checked
  against the crop.
- **The release guard is lifted** (`model::hdr::renders`): every SDR caller receives
  the SDR rendition through `Recipe::resolved`, the GPU refuses HDR recipes, and the
  master is reached only through `render_hdr`.

- **The HDR histogram describes the edit, not the display** (stage 5, 2026-11-09). It
  is measured from the master wherever the master renders, an SDR display included,
  as Lightroom shows the HDR range there and marks what the display can't show.
  Headroom is applied when it is drawn, never in its key, so a headroom change renders
  nothing. It is measured at a fixed 512-pixel long edge rather than the Fit preview's
  size, so window size never renders it again; that costs one small CPU render per
  edit beside the preview's.
- **Lightroom's layout, not its look.** Adobe's help pages could not be fetched; the
  behaviour follows secondary write-ups of Lightroom Classic 13 (Greg Benz, Computer
  Darkroom, 2023): SDR on the left, a section per stop above white up to the HDR
  limit, what the display can't show marked red, a yellow or red highlight triangle,
  and readouts in stops over white (`+2.3`), orange or red. RAWmakase gives SDR a
  fixed 40% of the width and spans exactly the limit, fractional limits included.
- **The last HDR histogram of the same photo stays while the next renders**, so a
  slider drag doesn't flicker between the SDR and HDR layouts. Results for older
  requests are still dropped, and another photo's histogram never shows.
- **The readout reads the master wherever it renders**: the shown HDR preview's own
  pixels (kept as half floats, a few thousandths of a stop), else the histogram's
  512-pixel master, so on an SDR display it is coarser at 100% than the SDR readout.
- **The histogram's drag zones stay fifths of its width** in HDR, as they edit the
  same sliders.

## Open

- Whites above +50 and Highlights in HDR; the colour mixer's Luminance
  ([Colour stage](#colour-stage)), Point Color, Color Grading, Defringe.
- A channel's point curve and Refine Saturation in HDR ([Tone curves](#tone-curves)).
- Clarity and Dehaze in HDR, after their SDR parity ([Presence](#presence)).
- Positive Shadows lifts small speculars in dark surroundings above Camera Raw's peak
  (0.2–0.3 stop; [Highlights and Shadows](#highlights-and-shadows)).
- Camera Raw's default SDR rendition (a local tone map, [SDR rendition](#sdr-rendition))
  and its sliders.
- Looks with an RGB table, their own sliders, B&W, Profile Amount and Camera Looks in
  HDR (looks of a table and a curve render: [Looks](#looks)).
- Saturated colours: with no look, the colour patches at saturation 0.9 differ from
  Camera Raw by a median ΔE_ITP of 24 at every intensity, below white too, while
  those at 0.3 and 0.6 differ by 0.2; not HDR's.
- Fixtures: synthetic PQ, HLG and gain-map files from the tools above (libultrahdr's
  Ultra HDR JPEGs are in `tests/data/formats`), plus a camera HLG/PQ HEIF and an Apple
  gain-map HEIC/JPEG, which need a real capture. Synthetic
  ramps crossing white, saturated highlights and negative matrix results.
- Frozen family: curves, masks and SDR rendition captures (the family and the gate
  set are frozen: [Frozen references](#frozen-references)), and the Lightroom merge,
  which needs a CC0 sample. Camera Raw's TIFF output carries no SDR
  rendition; its Save Image gain-map JPEG does, through the dialog only (no
  scripting route was found).
- Lightroom's opening defaults for unedited PQ, HLG and gain-map sources (RAWmakase
  starts them as HDR edits; Lightroom's choice is not measured), and HDR toggling
  with unavailable paired profiles.
- HDR display beyond macOS Develop's views: a GPU
  HDR path, Windows Advanced Color, Linux. Lightroom's Visualize HDR overlay on the
  photo (colour by stop above white) is not drawn; the histogram marks what lies
  beyond the display's headroom.

## Progress

- [ ] Stage 1: harness, Nikon measurements, codec spike, the frozen family and the
  broad gate set done; fixtures and display prototype open. Lossy JPEG XL uses jixel, not libjxl (#906).
- [x] Stage 2 (#905): HDR settings in the recipe (`model::hdr`), schema 14 and catalog
  version 6 for HDR on, values kept while off without a format change, XMP and
  Lightroom reading and writing, presets, the High Dynamic Range Copy/Sync group,
  and one shared guard refusing HDR on. Profile grouping (52552) and the previous
  reader's profile check are left for stage 3's DCP work.
- [ ] Stage 3: the CPU HDR master (`render_hdr`), HDR profiles rendered and
  importable, the ordinary-curve extension, Exposure, negative Whites, Blacks and the
  limit, measured and validated on the family (above); the SDR rendition everywhere
  else; the release guard lifted with Develop's HDR switch and SDR-rendition status;
  `rawmakase render --hdr`; looks of a table and a curve, Adobe Color among them
  ([Looks](#looks)); Contrast ([Contrast](#contrast)); the master point curve, the
  parametric curve and Levels ([Tone curves](#tone-curves)); Texture ([Presence](#presence)); Saturation, Vibrance, the
  colour mixer's Hue and Saturation, Calibration and Black & White
  ([Colour stage](#colour-stage)); Whites up to +50 ([Whites](#whites)); Shadows
  ([Highlights and Shadows](#highlights-and-shadows)). Open: Whites above
  +50, Highlights, the colour
  mixer's Luminance, the other looks, channel curves, the SDR rendition sliders, 52552 grouping (library only,
  needed when toggling HDR pairs profiles).
- [ ] Stage 6: HDR TIFF export (PQ and HLG 16-bit, linear float; Rec. 2020, Display
  P3, Rec. 709; CICP-tagged ICC; #908), checked against Camera Raw's PQ TIFF through an
  independent decoder; lossless JPEG XL PQ and HLG (and SDR sRGB), checked with
  libjxl's `jxlinfo` and `djxl` and jxl-oxide; gain-map JPEG (ISO 21496-1) checked with
  libultrahdr; JPEG XL gain maps (`jhgm`), the bundle read by libjxl's `jxlinfo`. A
  resized master lost its light above white until #910. Lossy JPEG XL (HDR, gain
  maps and SDR) through jixel, checked with `djxl` and jxl-oxide (#906). Open: AVIF
  and HEIC HDR are not planned for #894.
- [ ] Stage 4: HDR TIFF, PNG, JPEG XL and AVIF files (PQ, HLG or linear in BT.709,
  BT.2020 or P3; identity-curve ICCs; float TIFFs) open as HDR edits
  ([design](hdr-stage4-design.md)): exact light, master identity at their own
  settings, Exposure and geometry. The SDR rendition is the Library's tone map.
  ISO 21496-1 gain-map JPEG and JPEG XL files open as HDR edits too
  ([Gain-map files](../hdr.md#gain-map-files)). Their light is the map at full weight,
  independent of the display, and their base stays the SDR rendition. Damaged maps
  are refused by reason. RAWmakase's own gain-map exports reopen with the master's
  luminance (median 0.005 stop). libultrahdr 2.0.2's own Ultra HDR file opens with
  the light its decoder gives at full headroom, to 0.019 stop
  (`tests/data/formats/ultrahdr.jpg`, from `scripts/make-gain-map-fixtures.py`).
  JPEGs whose map is described only by Adobe's `hdrgm` XMP open the same way, checked
  against the Ultra HDR 1.0 formula on synthetic files. PQ and HLG HEICs open as HDR
  edits on macOS and with libheif ([HDR HEIC](../hdr.md#hdr-heic)), checked against
  BT.2100 on synthetic patches. On macOS, HEIC gain maps (Apple's own and ISO
  21496-1 `tmap` items) open as HDR edits ([HEIC gain maps](../hdr.md#heic-gain-maps)),
  checked against Apple's documented formula and ImageIO's own ISO decode on
  synthetic files. Open: HDR HEIC on Windows, HEIC gain maps on Windows and Linux,
  other adjustments on HDR files, Transform and Upright.
- [ ] Stage 5: macOS HDR display: a float surface through RAWmakase's egui fork
  (`surface_format_selector`, offered upstream), extended sRGB with EDR while an HDR
  photo shows, and the CPU master held to the display's headroom, with the Basic
  panel's note naming it: in Fit (#915; checked on a MacBook Pro XDR display with a
  CC0 Z 8 photo), and zoomed out, in 100% regions, Before Only, Before/After and
  Reference View (#924). The master's preview on the GPU during slider drags (#983),
  for ordinary and HDR profiles and looks (#984), Contrast and the tone curves (#986)
  the colour stage (#987) and Texture (#988), checked against the CPU master in float
  space. HDR files still show the CPU master (their scene carries Exposure as a gain
  of the light, which a GPU path would rebuild on every change); AI Denoise shows no
  HDR preview yet. The HDR histogram in stops above white, the HDR readout,
  and what lies beyond the display's headroom marked in the histogram and its
  highlight triangle. Open: the rest of the stage (above).
