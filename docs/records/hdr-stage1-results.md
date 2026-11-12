# HDR results: references, measurements and progress

Status: in progress, 2026-11-12. The record of the
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

- **Family:** 1,003 distinct jobs (1,091 since 2026-11-12, with the band Luminance,
  channel-curve and twelve training photos' Whites +75 / +100 captures of 2026-11-11). The twelve photos of [Validation](#validation) at
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
refused until 2026-11-12 ([sRGB-encoded tables](#srgb-encoded-tables)). The peak is the most frequent miss, as for the X100V and Z6 III above.

`main` b012705d (2026-11-12: Clarity 0 to +50, Dehaze ±30 and the mask operators of
#1031 among the stages added since f7d7ee9a) is accepted against that baseline on
validation (51 of 51) and holdout (47 scored; the two excluded). The gate scores neutral
renders, so it checks that the stages added since leave the default master alone; the
controls themselves are scored on the family's captures (re-scored on the same build:
Clarity, Dehaze and the masks' Contrast and Color reproduce their sections' tables).

## Measurements

| Question | Result |
| --- | --- |
| Repeatability | Two captures with identical settings are bit-identical (all metrics 0). The reference noise floor is 0. |
| PQ quantisation | One 16-bit PQ code step is 0.0002 stops at graphics white and at 16× it. It is never the limiting tolerance. |
| Graphics white | Camera Raw's PQ output puts graphics white at 203 cd/m². With `HDRMaxValue` = L, the brightest pixel is 2^L × white: 16.0012 at L=4 and 1.99988 at L=1. |
| `HDRMaxValue` | Stops above graphics white. Camera Raw keeps 1, 2.37, 4, 5, 6 and 8. It replaces 0, 0.99, −1 and 10 with +2.30, and uses +2.30 when the key is absent. It writes `+4.00`. |
| HDR limit shape | A roll-off, not a clip. At L=1 the luminance p90 falls from 2.46 to 1.54 (it would be 2.0 if clipped), and the share of pixels above white barely changes (11.37% to 11.31%). |
| Midtones, HDR against SDR | With the same profile, luminance below 0.5 differs by a median 0.001 stop (p99 0.071). From 0.5 to 1.0, HDR is a median 0.074 stop brighter, and above white it extends to the limit. |
| SDR rendition | Each of the seven sliders (`SDRBrightness`, `SDRContrast`, `SDRClarity`, `SDRHighlights`, `SDRShadows`, `SDRWhites`, `SDRBlend`) at +50 or −50 leaves the HDR master bit-identical (on the Z 8 photo for `SDRBrightness`, on a synthetic gray ramp for all seven). The sliders are −100 to +100 (`SDRBlend` too); 150 and −150 read as 0. Values set with HDR off are kept in the returned XMP. |
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
- **Channel curves.** A channel's own curve renders by the master curve's rule, after
  it, on that channel of encoded ProPhoto RGB, as in SDR: up to white as in SDR, then
  along its end slope. Measured on the gray ramp (linear profile, HDR limit 8) through
  a red curve through (128, 160) and a blue curve through (128, 100), read in ProPhoto
  RGB: within 0.03 stop from white to scene 8 (median 0.004–0.025), and the other
  channels stay the scene's. Camera Raw holds a channel's output at 16: the blue
  curve renders scene 7.9 and 11.8 as 16.003. Above scene 8 Camera Raw's red drifts
  toward the input, as its master curves do (0.09 stop at 11.8, 0.26 at 15). An
  earlier reading of the same captures in Display P3 took red's P3 clipping at 0 for
  the curve's own output (red at scene 11.8 is 4.59 in ProPhoto). An identity channel
  curve leaves its channel alone (not measured: in SDR it is a no-op). Refine
  Saturation other than 100% and a curve whose last point is left of white stay
  refused (not measured).
  On the twelve photos of [Validation](#validation) (Adobe Standard, limit 4) a red
  curve through (128, 150) and a blue curve through (128, 112) keep the median and p95
  above white within the tolerances wherever the default render does (median
  0.007–0.074 stop), and ΔE_ITP as by default; the peak does not: the Z50 II's brightest
  block is +0.192 stop (default +0.082) and the Z 8's +0.109 (+0.078).
  SDR renders do not change: builds of the change and of the main commit it was made on
  rendered 46 raw files (40 local copies and 6 dataset photos) with five sidecars (none,
  channel curves, band Luminance with Saturation and Vibrance, and two HDR edits'
  SDR renditions) to identical pixels, 229 of 229.
- **Photos.** A soft S curve with Highlights −20, Lights +15, Darks +10 and Shadows −10,
  and a brightening curve through (128, 160), Adobe Standard DCPs, HDR limit 4, score
  as the default render does on the same photos: median above white 0.004–0.076 stop,
  and the same eight of twelve pass every tolerance. The four that fail fail by the
  reasons under [Validation](#validation) (the Panasonics' p95 0.24–1.48 and ΔE_ITP,
  the X100V's rebuilt highlight peak +0.22–0.24, the Pentax's ΔE_ITP); the Z6 III's
  peak, +0.108 by default, is +0.056 and +0.070.

**Refine Saturation and curves that end left of white (2026-11-12).** Camera Raw 18.7
rendered gray and colour patches (saturations 0.3, 0.6 and 0.9, 0.125 to 8 times white,
a linear profile, HDR limit 8 and SDR) through an S curve and a brightening curve at
Refine Saturation 0 and 50, and four master curves and a red curve whose last point is
left of white. Scores are the edit's effect against Camera Raw's (luminance, median /
p95 stops) and the colour error it adds to the base render's (mean ΔE_ITP), HDR and SDR
on the patches below 0.9, and HDR above white ([Decisions](#decisions)).

- **Refine Saturation.** SDR's rule on the master's encoded values, without its bound at
  white, scored against the S curve alone: at 0, 0.003 / 0.156 stop and +1.5 ΔE_ITP
  below white and +2.3 above, where the SDR rendition is 0.002 / 0.120 and −0.9; at 50,
  0.001 / 0.072 and +0.6 (+1.3 above) against 0.001 / 0.054 and −0.4. With SDR's bound
  kept, light above white loses its colour (+20.8 and +53.3). Refused.
- **Curves that end left of white.** Camera Raw's HDR render doesn't hold such a curve's
  value, as SDR does: it continues from the last point along its slope there, below
  white too (a curve through (0, 0) and (200, 200) is the identity), as the master
  curve continues past white. That rule renders Camera Raw's ramp within 0.03 stop up to
  16 times white; above, Camera Raw holds steep curves back (scene 16 through (90, 110)
  and (180, 255) renders 49, the rule 357). On the patches: through (128, 140) and
  (220, 255), 0.008 / 0.166 stop and +2.8 ΔE_ITP below white against SDR's 0.002 /
  0.116 and +1.7, and 0.034 / 0.238 (+7.8) above; through (90, 110) and (180, 255),
  0.016 / 0.893 and 0.822 / 2.419 above white. Refused.

**Levels.** RAWmakase's black point, white point and midtone have no Camera Raw
counterpart to measure. Decision: they render in HDR as Camera Raw renders a point
curve of the same shape, by the rule above: as in SDR up to white, then along the
slope at white, 1 / (midtone (1 − black)); a white point below white leaves a slope of
0, as Camera Raw's curve through (230, 255) holds white (0.89 by scene 15).

**Curves unlike the family (2026-11-12).** Of the DCPs installed with Camera Raw 18.7,
two have a curve [`hdr_tone`]'s family refuses: the Fujifilm FinePix HS20EXR's and
HS30EXR's Adobe Standard (Mahalanobis distance 4.22, the limit 4). Camera Raw rendered
the HS20EXR's curve on gray and colour patches (HDR limits 8 and 4, and SDR). The fitted
extension renders them 0.013 / 0.095 stop and 2.49 mean ΔE_ITP from Camera Raw below
0.9, and 0.067 / 0.160 above white (scene 16: 63 against Camera Raw's 49), where the
SDR render is 0.005 / 0.043 and 1.00. They stay refused.

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

Re-examined on 2026-11-11 under the [acceptance rule](#decisions), with RAWmakase's
SDR operators unclipped in HDR and the same captures (CC0 copies of the five photos
from raw.pixls.us, checked against the family's hashes). The effect error in stops,
median / p95 over the blocks, then the median over the photos, and the mean ΔE_ITP of
the edited renders; below white is where both of Camera Raw's renders are below white,
above white where its HDR base is:

| Setting | HDR below white | SDR below white | HDR above white | ΔE_ITP HDR / SDR (added to the default's) |
| --- | --- | --- | --- | --- |
| Clarity +50 | 0.177 / 0.619 | 0.176 / 0.616 | 0.064 / 0.233 | 8.8 / 8.5 (+7.9 / +7.6) |
| Dehaze +30 | 0.097 / 0.305 | 0.096 / 0.305 | 0.072 / 0.183 | 6.7 / 5.7 (+4.7 / +4.1) |
| Dehaze −30 | 0.116 / 0.225 | 0.115 / 0.225 | 0.037 / 0.066 | 6.0 / 5.9 (+5.1 / +5.0) |
| Texture +50 (renders) | 0.005 / 0.074 | 0.005 / 0.074 | 0.002 / 0.058 | 2.9 / 1.9 (+1.0 / +1.0) |

"No worse" is taken as within 0.01 stop of SDR's error below white (the same operator
renders both there; the 0.001–0.003 stop between them is the HDR and SDR default
renders' own difference) and within 1 ΔE_ITP, about one just-noticeable difference, of
the colour error SDR's render of the control adds to its default render's. Clarity and
Dehaze ±30 are within both, and their HDR error above white is below SDR's below it.
The SDR operators' own gap stays (Clarity's median error is above that of leaving it
out on these photos, 0.146 over the whole frame, in SDR as in HDR).

**Decision, 2026-11-11: Clarity from 0 to +50 renders in the HDR master** by the SDR
operator, unclipped; negative Clarity and Clarity above +50 stay refused until photos
are captured there. Dehaze from −30 to +30 renders too, by the same measurements (since 2026-11-12). SDR
renders don't change: the change is in the HDR master's checks
only (`model::hdr::check_recipe`, and the GPU's HDR eligibility, which checks the
recipe as edited and the resolved one without adding a look's settings again), which
SDR renders never call. The GPU's HDR preview renders Clarity as the CPU
master does (`gpu_develop_matches_cpu_hdr_master`).

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

**RGB tables, Profile Amount, B&W and a look's own settings** (2026-11-10). Gray ramps
(sensor white +4, a linear and the ACR3 profile curve, HDR limit 8, to 48 times white)
and colour patches through the synthetic looks of `tests/corpus/looks`: RGB tables in
Adobe RGB (gamma 2.2, 3D and 1D), linear ProPhoto, sRGB (Clip and Extend), at amounts
0.5 to 2, and `.cube` looks (identity, creative, a 1D curve); Profile Amount 50%, 150%
and 200%; a B&W look; and looks that carry Contrast and Blacks, Exposure or a
parametric curve beside the same values set as sliders.

- **RGB table.** Camera Raw applies it on overrange-encoded values (DNG 1.7's
  encoding, 16 times white at 1): each channel in the table's primaries is encoded,
  then put through the table's own encoding, looked up, and decoded back. An identity
  table renders every value unchanged up to 48 times white. What a channel encodes
  beyond 1 is added back to the table's output in the overrange encoding: on the 1D
  table that rule gives 0.0009 stop median above 16 (adding it in the table's own
  encoding gives 0.036, adding nothing 1.1). Extend tables render as Clip ones there.
  RAWmakase's renders of all ten RGB looks score 0.0009–0.0023 stop median above
  white (p95 at most 0.046, the Adobe RGB 3D table on the ACR3 curve; the 1D table's
  0.22 p95 is in the shadows, where its SDR render misses as much). On the colour
  patches the Adobe RGB and sRGB tables score as the patches do without a look
  (median ΔE_ITP 0.3, the same saturation-0.9 outliers).
- **A look's curve that is the identity** (every `.cube` look's) applies no curve:
  Camera Raw does not hold such a look at 16, as the curve rule above has it. That was
  measured with `.cube` looks; an HSV-table look with an identity curve follows the
  same rule, unmeasured above 16 times white.
- **Profile Amount** renders as in SDR, the table, curve and settings at that amount
  then rendered by the HDR rules: 0.004–0.011 stop median above white at 50%, 150% and
  200% (p95 at most 0.042).
- **B&W look** (an HSV table, an S curve and Contrast +20, black and white): 0.012
  stop median above white, p95 0.093.
- **A look's own settings** render exactly as the same sliders: the look and the
  sliders score the same against Camera Raw (Contrast +40 with Blacks −20: 0.005 median;
  Exposure +0.5: 0.002; the parametric curve: 0.011), so they are added to the user's
  and checked as those sliders are.

Still refused: a look that sets a slider whose HDR rendering isn't measured (Adobe
Landscape sets Highlights; Adobe Vivid's Clarity +10 renders since 2026-11-11), and a
look's split toning.

**A look's split toning (2026-11-12).** Camera Raw 18.7 rendered the synthetic split
look (`tests/corpus/looks/synthetic-split.xmp`: shadows 210° at 40, highlights 50° at
30, balance +20) on gray and colour patches (saturations 0.3, 0.6 and 0.9, 0.125 to 8
times white, a linear profile, HDR limit 8 and SDR). RAWmakase's SDR grading pass for
it, in colour grading's HDR domain ([Color grading](#color-grading)), renders the
effect on luminance as closely in HDR as in SDR (median 0.003 against 0.002 stop, p95
0.060 against 0.036; above white 0.002), but not its colour: on the patches below
saturation 0.9 it moves the master's mean ΔE_ITP from Camera Raw's from 0.24 to 1.79
below white and to 2.94 above it, where the SDR rendition's goes from 0.23 to 1.05.
Camera Raw's HDR split tone is stronger than its SDR one there (its mean change 20.6
ΔE_ITP below white and 25.7 above, against 12.6 in SDR). By the acceptance criterion
([Decisions](#decisions)) a look's split toning stays refused.

**On photos** (2026-11-11). The six validation photos of [Validation](#validation)
(Adobe Standard, HDR limit 4) through a `.cube` look (creative), an HSV-table look at
50%, an Adobe RGB RGB-table look at 150% and the B&W look, against Camera Raw 18.7's
renders of the same: every photo but the Panasonic S1M2 meets the HDR tolerances, as
it does without a look.

| Photo | Median / p95 above white (stop), worst look | Peak, worst look | ΔE_ITP mean, worst look |
| --- | --- | --- | --- |
| Nikon Z 8 | 0.056 / 0.087 (B&W) | +0.082 | 3.5 (`.cube`) |
| Nikon Z f | 0.023 / 0.032 (B&W) | +0.133 (`.cube`) | 2.4 (B&W) |
| Nikon Z6 III | 0.020 / 0.149 (table at 50%) | +0.125 (B&W) | 2.3 (B&W) |
| Sony A7R III | 0.040 / 0.075 (B&W) | −0.012 | 1.5 (`.cube`) |
| Canon 850D | 0.037 / 0.057 (B&W) | +0.034 | 2.1 (B&W) |
| Panasonic S1M2 | 0.095 / 0.762 (`.cube`) | −0.056 | 39.9 (`.cube`) |

The S1M2 fails as its neutral render does (an SDR gap of that body). The Z6 III's peak
misses as its neutral render's does (+0.108). The Z f's `.cube` peak is one saturated
channel: its luminance peak is within 0.012 stop (2.565 against 2.543), the channel
3.92 against 3.58.

The remaining colour patches: the `.cube` creative and hue-rotation looks, the tables at
50% and 200% and the 1D table score as the patches do without a look (median ΔE_ITP
0.2–0.7). Two looks miss on saturation-0.9 patches only, which the tolerances report
apart: the B&W look (median 0.038 stop, p95 0.49; worst at 0.25 to 0.5 times white,
below white, where the saturation-0.9 patches miss in SDR too) and a B&W `.cube`, whose
saturation-0.9 red and magenta patches, far outside its sRGB table's gamut, Camera Raw
renders almost black at and above white (up to 16 stops darker; 3 of 216 patches at 8
times white), where RAWmakase renders their gray.

### sRGB-encoded tables

Of the 4,327 DCPs installed with Camera Raw 18.7 that RAWmakase reads, 2,439 have a table
whose value divisions are sRGB-encoded (`ProfileLookTableEncoding` 1): 2,437
camera-matching profiles (Camera Standard, Vivid, Flat and the others, for Sony, Nikon,
Canon, Panasonic, Olympus and OM, Ricoh, Leica, Pentax and Sigma) and two Adobe
Standard profiles (the Leica D-Lux 7's and a Samsung phone's front camera's). 2,430
of them have only a 3D look table (90 × 16 × 16 in 2,394); 27 are named as HDR
profiles (such as the Nikon Z 5 II's Camera Standard HDR).

The only Camera Raw HDR capture of such a profile on a CC0 photo is the gate set's
Leica D-Lux 7 (Adobe Standard, limit 4; 4.9% of its area above white), with its SDR
default render in the SDR corpus. Three renderings of the table above white were
scored on the gate's blocks (luminance error median / p95 in stops, mean ΔE_ITP):

| Rendering | HDR, below 0.9 | HDR, above white | Peak | SDR, below 0.9 |
| --- | --- | --- | --- | --- |
| The value sRGB-encoded past 1, as in SDR | 0.008 / 0.040, 0.39 | 0.005 / 0.007, 0.28 | −0.004 | 0.010 / 0.037, 0.59 |
| The table's change at white as a gain above it | 0.008 / 0.040, 0.39 | 0.005 / 0.007, 0.28 | −0.004 | |
| The overrange rule of HDR profiles | 0.007 / 0.041, 0.35 | 0.005 / 0.007, 0.23 | −0.001 | |

Each is closer to Camera Raw's HDR render than RAWmakase's SDR render is to its SDR
one, and the first two are identical on this photo (its table sees few scene values
above white, before the profile's curve).

Decision, 2026-11-12: an ordinary profile's sRGB-encoded tables render in HDR as in
SDR, the value encoded past 1 by the sRGB curve: the master matches the SDR rendition
below white, and the GPU's HDR preview already applies them so. An HDR profile's
stay refused: how Camera Raw combines its overrange encoding with an sRGB-encoded
table is not measured, and no capture of one exists.

### Red Eye

Camera Raw 18.7 rendered a synthetic eye probe (red pupils and glowing pet pupils on
skin and iris, at a quarter of, once and four times their base level, a linear
profile) with Red Eye and Pet Eye at Darken 0, 50 and 100, in HDR at limit 8 and in
SDR. Its HDR correction is its SDR one: each corrected pupil's luminance matches to
about 1% (the brightest red pupil, 0.89 of white in HDR and clipped to 0.28 in SDR,
corrects to 0.320 and 0.319), so the fix works on the scene before tone, as
RAWmakase's does on the camera's samples.

Scored on the pupils' cores (the correction's luminance effect against Camera Raw's,
and mean ΔE_ITP of the corrected pupils), RAWmakase's master is 0.205 stop off at the
median (p95 0.87) against its SDR rendition's 0.221 (p95 1.35); on the red pupils the
mean ΔE_ITP is 32.7 in both. Pet Eye's pupils are black in RAWmakase and 0.0001–0.0005
of white in Camera Raw (0.00052 in HDR, 0.00045 in SDR at the brightest), which is all
of the HDR score's 0.56 higher mean ΔE_ITP over every pupil (33.4 against 32.8). The
SDR operator's own error (its Darken and colour fit) dominates both.

Decision, 2026-11-12: Red Eye and Pet Eye render in the HDR master, as in SDR. HDR
files still refuse them, as every retouch.

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

Above +50, re-examined on 2026-11-11 with Camera Raw 18.7 captures of twelve more CC0
photos (the gate set's training split, Adobe Standard, limit 4, spread over p99 from
−7.4 to +0.1 EV) at Whites +75 and +100. For each, RAWmakase's master was rendered at
forced levels to find the one whose median error above white is zero:

- **Twice the 99th percentile is as good a level as any found.** Camera Raw's best
  level is within a standard deviation of 0.23 stop of it at +75 and 0.20 at +100 (mean
  −0.04 and −0.06; worst 0.49), as on the validation photos, with one exception: the
  Leica V-Lux 5, a dark photo whose 99th percentile is about 7 stops under white,
  renders at a level 6.4 stops above twice it.
- **The level is not enough.** At Camera Raw's own best level per photo, four of the
  twelve are still outside the median tolerance above white at both +75 and +100
  (Panasonic DC-S9 0.22 / 0.38 stop, Fujifilm X-T5 0.52 / 0.47, Canon PowerShot SX70
  HS 0.17 / 0.26, Sony ILCE-1 0.21 / 0.21). At twice the 99th percentile, seven are
  (worst 2.7 stops, the V-Lux 5).

**Decision, 2026-11-11: Whites above +50 stays refused.** Part of Camera Raw's positive
Whites above +50 is not a gain of one photo level; a better level predictor alone
cannot reach the tolerances.

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

Re-examined on 2026-11-10 with the same captures (six validation photos at −100, −50
and +50, RAWmakase's renders of the same jobs), by reshaping RAWmakase's gain above a
level and scoring against Camera Raw (reduced blocks):

- **A level-off does not reach parity.** Holding negative Highlights' pull from 1.5
  stops above white, the best shared level, takes the Z 8's median above white at −100
  from 0.68 to 0.08 stop, but leaves the Z6 III at 0.58, the A7R III at 0.99 and the
  Panasonic at 0.23 (other levels from 0 to 2 stops are worse). The rest is the rule's
  gain at white itself, which SDR's shoulder hides. Positive Highlights faded from 1.5
  stops above white over 2 stops gives medians of 0.03–0.27 and p95 up to 0.80.
- **Not a curve of level either.** Even Camera Raw's own gain per level for each photo,
  applied above white, leaves p95 at 0.36–0.49 (A7R III, Z6 III) and the Z 8's peak
  0.94 stop high at −100: part of the HDR operator is spatial.

**Decision, 2026-11-10: Highlights stays refused in HDR.** It needs a rule trained on
HDR targets of many photos, not a reshaping of the SDR one.

Re-examined on 2026-11-11 under the [acceptance rule](#decisions), with RAWmakase's HDR
and SDR renders of the same six photos (CC0 copies from raw.pixls.us, checked against
the family's hashes). The effect error in stops, median / p95 over the blocks, then the
median over the photos; below white is where both of Camera Raw's renders are below
white, above white where its HDR base is:

| Highlights | HDR below white | SDR below white | HDR above white | HDR above white, pull held from −0.25 |
| --- | --- | --- | --- | --- |
| −100 | 0.077 / 0.197 | 0.077 / 0.183 | 0.369 / 0.737 | 0.217 / 0.442 |
| −50 | 0.038 / 0.099 | 0.038 / 0.086 | 0.186 / 0.415 | 0.117 / 0.267 |
| +50 | 0.059 / 0.143 | 0.052 / 0.112 | 0.218 / 0.526 | — |

- **Holding negative Highlights' pull** at its value from a base level (log2 scene
  luminance) brings the error above white down, from −0.25 most at no cost below half
  of white; lower levels fit above white a little better but reach pixels below half of
  white through the base (Canon 850D at −100: the HDR and SDR renders' p99 below 0.5 is
  0.019 unheld, 0.069 held from −0.25, 0.139 from −0.5, past the 0.1 stop they must
  agree to). Held, it is still two to three times SDR's error below white.
- **Positive Highlights** is stronger in Camera Raw's HDR render near white than the SDR
  rule (Canon 850D: +1.49 stops against +0.69 just above white) and fades from about 1.5
  stops above it, by different amounts on each photo. A gain per level fitted to Camera
  Raw's HDR renders, scored with each photo left out of its fit, does not generalise
  (and fitted to negative Highlights scores worse than no fit).

Highlights stays refused in HDR. Scored over the whole frame instead (a measure first
tried here), the held negative Highlights would pass, as the light above white is a
few percent of the frame; that measure was dropped for this reason.

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
- **The master renders with them.** They never change the master in Camera Raw
  (bit-identical with each at ±50), so the master, HDR exports and Develop's HDR
  preview render as without them (2026-11-10; until then they refused the master).

**Each slider on three photos** (2026-11-11). Camera Raw's SDR rendition of the CC0
Nikon Z 8, Nikon Z f and Sony A7R III photos (Adobe Standard, HDR limit 4), saved
through Save Image as gain-map JPEGs: the default and each slider at −100, −50, +50 and
+100 (87 renders), compared by luminance in 16-pixel linear blocks. RAWmakase's default
SDR rendition (the HDR-off render) is a median 0.22–0.38 stop from Camera Raw's on these
photos. Per slider, how the rendition changes against the default, how much of that
change one curve of luminance leaves (p95), and how far the three photos' curves are
apart (median / p90):

| Slider | Change, median (stop) | Left by one curve, p95 | Curves apart |
| --- | --- | --- | --- |
| Brightness ±50, ±100 | −1.02 to +0.95 | 0.01–0.11 | 0.011–0.021 / 0.045–0.110 |
| Contrast −100 to +50 | −0.14 to +0.13 | 0.01–0.09 | 0.014–0.032 / 0.044–0.090 |
| Contrast +100 | −0.11 to −0.43 | 0.08–0.37 | 0.154 / 0.363 |
| Highlights ±50, ±100 | −1.25 to +0.18 | 0.01–0.11 | 0.148–0.849 / 0.175–1.046 |
| Shadows −50, −100 | −0.12 to −0.34 | 0.06–0.17 | 0.114–0.228 / 0.170–0.295 |
| Shadows +50, +100 | +0.23 to +1.18 | 0.13–0.33 | 0.235–0.889 / 0.456–1.283 |
| Whites ±50, ±100 | −0.01 to +0.42 | 0.00–0.14 | 0.001–0.027 / 0.004–0.142 |
| Clarity ±50, ±100 | −0.22 to +0.95 | 0.09–1.14 | 0.232–2.546 / 0.291–4.178 |
| Blend ±50, ±100 | 0.00 | 0.00–0.01 | — |

Blend is Camera Raw's "Highlight Saturation": it changes no luminance, and saturation
only above about half of white (±0.1 at ±100 on the A7R III). Clarity is local.

The test of rendering them is how well the change, applied to RAWmakase's own SDR
rendition, gives Camera Raw's change, with the curve fitted on two photos and checked
on the third, against the HDR tone tolerances (median 0.15, p95 0.35 stop on every
photo). These pass:

| Slider | Renders | Worst held-out photo, median / p95 (stop) | Fails beyond (worst) |
| --- | --- | --- | --- |
| Brightness | −50 to +100 | 0.124 / 0.254 | −100: 0.038 / 0.387 |
| Contrast | −100 to +50 | 0.084 / 0.191 | +100: 0.274 / 0.807 |
| Highlights | 0 to +100 | 0.098 / 0.204 | −50: 0.226 / 0.261; −100: 0.820 / 0.913 |
| Shadows | −100 to 0 | 0.061 / 0.267 | +50: 0.085 / 0.393; +100: 0.353 / 1.062 |
| Whites | −100 to +50 | 0.025 / 0.164 | +100: 0.142 / 0.465 |

- **Decision: the SDR rendition renders them within those ranges** (2026-11-11), the
  three photos' average curve per measured value, interpolated linearly in the value
  (none at 0), each slider in the panel's order on the luminance the others leave. With
  any slider beyond them, or Clarity or Blend set, none renders: an edit is never half
  rendered, and the Basic panel's note names the slider. Several at once were not
  measured together. The rendition stays the HDR-off render otherwise, its default a
  local tone map short of Camera Raw's; gain-map exports carry it as their base.

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
  Camera Raw changes the channels below about white and not the ones above. No lookup
  or fade by the colour's value fitted it: through the ACR3 curve the best left 30–60
  median ΔE_ITP between 0.4 and 1.25 times white (the SDR grids' error at 0.2: 12–28).
  A fade by each channel's own level does ([below](#band-luminance)).
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
and Black & White render in the HDR master;** the colour mixer's Luminance stayed
refused until 2026-11-11 ([Band Luminance](#band-luminance)).

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

#### Band Luminance

Fitted on 2026-11-10 to the colour patches above at every band's Luminance ±50 and ±100
(through a linear profile, ±100 through the ACR3 curve), starting from Camera Raw's own
HDR render without the slider so that only the slider's effect is compared, and checked
on 2026-11-11 on the twelve photos of [Validation](#validation). Camera Raw's HDR
Luminance is the SDR grids' change with its value part applied to each ProPhoto channel
by that channel's level:

- **Per channel.** Each channel takes the value change in full up to 0.6 of white and
  none from white up, linearly between (`color_mixer::HDR_LUMINANCE_FADE`; fades from 0.5
  to 0.7 score alike). A fade by the colour's value, as Vibrance's, fits no better than
  none.
- **By the scene's level.** Through the ACR3 curve the level that fades a channel is its
  scene value before the curve (as the mixer's other changes change it), not the
  rendered value: a colour the curve brightens from a scene of 0.4 to 0.8 takes the full
  change. Fading by the rendered value left the ACR3 patches 31 (+100) and 64 (−100)
  median ΔE_ITP below white, by the scene's 20 and 39.
- **Its lookup.** The change is looked up at the colour's value up to 0.5
  (`color_mixer::HDR_LUMINANCE_VALUE`; 0.4–0.6 score alike), where the grids still hold
  Camera Raw's SDR response, rather than up to 0.2 as the other changes are; below 0.5
  an HDR master's Luminance is then its SDR rendition's exactly. Above 0.5 the tables'
  hue and saturation changes move back to the 0.2 lookup by white: looked up at 0.5 up
  to white, they desaturate colours above white more than Camera Raw does (+5 to +30
  ΔE_ITP above white on the patches).

The rule's Python port on the probe DNGs, median ΔE_ITP against Camera Raw (RAWmakase's
renders of the same jobs matched the port to within 1 before the hue and saturation
blend was added; its Rust form is tested against SDR below 0.5):

| Luminance, every band | Above white | Below 0.5 | SDR render, below 0.5 | Before: as the other changes, above white |
| --- | --- | --- | --- | --- |
| +100, linear | 7.0 | 12.3 | 11.9 | 47 |
| −100, linear | 28.2 | 33.1 | 34.7 | 66 |
| +50, linear | 4.4 | 6.8 | 6.7 | 27 |
| −50, linear | 10.8 | 16.3 | 17.9 | 24 |
| +100, ACR3 | 12.4 | 19.6 | 3.3 | 53 |
| −100, ACR3 | 33.9 | 38.5 | 26.1 | 73 |

Through the ACR3 curve "below 0.5" is the scene's value, which the curve renders up to
0.9, where Camera Raw's own HDR render departs from its SDR render (Luminance +100: 14.7
ΔE_ITP at a scene of 0.2, which the curve renders at 0.4).

**Photos.** Adobe Standard, HDR limit 4: a mixed edit (Red −20, Orange +40, Yellow +25,
Green −30, Aqua +20, Blue −40, Purple +15, Magenta −15) on all twelve photos, and every
band at +100 on the six validation photos. Median / p95 above white and the peak in
stops, ΔE_ITP mean / p95; outside the tolerances in bold, the default render beside:

| Photo (split) | Default | Mixed edit | Every band +100 |
| --- | --- | --- | --- |
| Nikon Z 8 (validation) | 0.061 / 0.096, +0.078, 2.9 / 7.0 | 0.050 / 0.131, +0.077, **5.2** / 11.6 | 0.112 / 0.169, +0.077, 3.8 / 9.4 |
| Nikon Z f (validation) | 0.007 / 0.009, +0.007, 0.9 / 2.9 | 0.012 / 0.025, +0.009, 3.3 / 9.0 | 0.021 / 0.054, −0.012, 2.1 / 4.5 |
| Nikon Z6 III (validation) | 0.014 / 0.131, **+0.108**, 1.4 / 3.7 | 0.076 / 0.180, **+0.107**, 3.7 / 10.1 | **0.202** / 0.291, **+0.107**, 2.9 / 12.1 |
| Panasonic S1M2 (validation) | 0.061 / **0.395**, −0.012, **27.4 / 110** | 0.140 / **1.33**, −0.021, **30.8 / 113** | **0.251 / 0.579**, −0.035, **34.9 / 126** |
| Sony A7R III (validation) | 0.032 / 0.075, −0.011, 0.9 / 1.9 | 0.026 / 0.173, −0.011, 1.8 / 3.2 | 0.065 / 0.317, −0.013, 3.1 / 5.9 |
| Canon 850D (validation) | 0.032 / 0.035, +0.025, 1.6 / 2.4 | 0.047 / 0.071, +0.025, 3.2 / 6.4 | 0.015 / 0.044, +0.025, 2.2 / 4.5 |
| Nikon Z50 II (holdout) | 0.012 / 0.020, +0.082, 1.3 / 2.7 | 0.025 / 0.052, +0.015, 2.2 / 4.2 | – |
| Nikon Z5 II (holdout) | 0.017 / 0.025, −0.011, 4.0 / 8.2 | 0.027 / 0.081, −0.071, 4.9 / 8.3 | – |
| Panasonic S1M2ES (holdout) | 0.078 / **1.55**, −0.030, **24.1 / 91** | **0.204 / 1.60**, −0.030, **25.4 / 87** | – |
| Fujifilm X100V (holdout) | 0.030 / 0.063, **+0.360**, **5.6 / 19.4** | 0.081 / 0.229, **+0.360**, **10.5 / 20.3** | – |
| Sony A7R IV (holdout) | 0.045 / 0.061, +0.026, 1.4 / 3.9 | 0.145 / 0.177, +0.006, 3.6 / 10.1 | – |
| Pentax K-1 II (holdout) | none above white, +0.025, **9.6 / 29.7** | 0.145 / 0.202, **−0.115**, **11.6 / 30.3** | – |

The default column is the [Validation](#validation) table's (the Z5 II's ΔE_ITP as the
frozen family reads it).

- **The mixed edit** stays within the luminance tolerances wherever the default render
  does; the Z 8's ΔE_ITP mean is 5.2 (default 2.9), as Saturation +30's 5.3 is. Below
  0.5 RAWmakase's HDR and SDR renders agree to a median of 0.004 stop or less on every
  photo, as Camera Raw's (≤ 0.006).
- **Every band at +100** leaves the Z6 III 0.20 stop median above white (default 0.014).
  There Camera Raw's HDR Luminance strengthens with brightness up to white (+0.70 stop
  at a ProPhoto value of 0.16, +1.0 from 0.6 to 0.9) where its SDR effect stays flat and
  rolls off (+0.68 to +0.40), so the SDR-fitted grids can't give it.
- An earlier form that looked the tables' hue and saturation up at 0.2 below white too
  left the Z5 II's HDR render 0.12 stop brighter than its SDR rendition below 0.5 with
  the mixed edit (Camera Raw's: 0.006); the lookup above fixed it (0.004).

**Decision, 2026-11-11: band Luminance renders in the HDR master up to ±40**, the range
the photos measured, and refuses it beyond (`model::hdr::check_recipe`), as Whites is
limited to +50.

Beyond ±40, re-examined on 2026-11-11 under the [acceptance rule](#decisions) with the
same rule at every amount (effect error, median / p95, median over the photos or patch
frames; below white as for Highlights above):

| Edit | Captures | HDR below white | SDR below white | HDR above white |
| --- | --- | --- | --- | --- |
| Every band +100 | 6 photos | 0.031 / 0.120 | 0.035 / 0.120 | 0.055 / 0.200 |
| The mixed edit (±40, renders) | 12 photos | 0.042 / 0.183 | 0.039 / 0.124 | 0.058 / 0.141 |

At +100 the error above white is past SDR's below it (median 0.055 against 0.035, p95
0.200 against 0.120); beyond ±40 stays refused. On the synthetic colour patches of
[Colour stage](#colour-stage), over the whole frame, every band at −100 and −50 scores
better in HDR than in SDR (median 0.061 / 1.140 against 0.111 / 1.783 at −100); no photo
is captured there. The ±40 edit that renders is past the rule too; it was rendered
before the rule and isn't re-scored by it.

### Vignette and grain

Measured with Camera Raw 18.7 on a synthetic DNG through `probe_dng.py` (a linear
profile, sensor white +4 stops): a checker of 32-pixel gray tiles at 0.03, 0.1, 0.25,
0.5, 1, 2, 4 and 8 times white over a 1536×1024 frame, so every level sits at every
distance from the centre. Highlight Priority, Color Priority and Paint Overlay at
Amount −100, −50 and +50 (Midpoint 50, Feather 50, Roundness 0), Highlight Priority
−50 with Highlights 50, and grain at Amount 50 and 100 (Size 25, Roughness 50), in HDR
at limit 8 and in SDR. Tile means and deviations are of each tile's inner 20 pixels.

- **Darkening** is a gain independent of level in HDR: at a quarter of the way out to
  the corners, −50 gives ×0.29 (Highlight and Color Priority) and ×0.25 (Paint
  Overlay) at every level from 0.1 to 8 times white; through the probe's linear
  profile the scene and the output are the same above the toe. In SDR, Highlight
  Priority's gain rises toward white (×0.41 at 0.69, ×0.91 at 0.99) as the default
  curve's shoulder holds it; Color Priority's and Paint Overlay's don't.
- **Lightening** by Paint Overlay and Color Priority, identical to each other in both
  renders, is SDR's paint rule v + (1 − v)·opacity on unclipped light: median 0.002
  stop, p95 0.037. Above white it darkens (+50: ×0.89 at eight times white). Highlight
  Priority's +50 blends the scene towards a white W with SDR's opacity: W is 14.4
  (p10–p90 13.5–15.2) at every level up to twice white and 14.9–15.2 at four and eight
  times.
- **Highlights 50** under −50 Highlight Priority changes nothing up to twice white and
  lifts four and eight times white to ×0.32 and ×0.43 (from ×0.29); SDR's smoothstep
  near white doesn't fit, and two levels don't settle another rule.
- **Grain**: its deviation in sRGB-curve-encoded values, relative to a quarter of
  white's, is 1.27, 1.10, 1.00, 0.94, 0.89, 0.84, 0.79 and 0.44 at 0.03 to 8 times
  white for Amount 50, within 0.02 for Amount 100, and 0.98 of SDR's at a quarter of
  white. SDR's fades by white (0.27 at 0.98).
- **Engine.** Rendered by the build with the rule (`PostCropVignette::apply_hdr`,
  `GrainField::noise_hdr`), each style's effect above white is within a median 0.000–
  0.010 stop of Camera Raw's (p95 0.003–0.069); over all tiles 0.001–0.022 (p95 up to
  0.29, and 2.0 for Highlight Priority −100, whose darkest corner tiles are in the
  scene tone stage's toe). Grain's spread is 0.98–1.01 of Camera Raw's from 0.1 to 8
  times white and 0.89–0.90 at 0.03.

Decision, 2026-11-10: the post-crop vignette and grain render in the HDR master on the
CPU (the GPU's HDR preview leaves them to it); a darkening vignette's Highlights
refuses the master.

**Photos (2026-11-11).** Highlight Priority −30 and Paint Overlay +30 with grain 40
(Size 25, Roughness 50), Midpoint and Feather 50, Adobe Standard DCPs, HDR limit 4, on
the twelve photos of [Validation](#validation); each edit's effect is its render
against the photo's default render, ours against Camera Raw's frozen one, in 8× blocks
by squared distance from the centre (quarters to the corners).

- **Highlight Priority through the profile's curve alone** (the first rule) darkened
  too little away from the centre: in the outer quarter +0.07 to +0.24 stop (Camera
  Raw's corners −1.23 to −1.77 stops; the 850D's −1.63 rendered −1.08), most in dark
  areas (+0.04 to +0.19), not above white (±0.01).
- **Through the master's own neutral response** (`pipeline::hdr_neutral_response`:
  the scene tone stage, toe included, and the finish at the edit's settings) the
  effect is within ±0.06 stop in every quarter on every photo, ±0.015 in dark areas,
  and the corners render −1.20 to −1.75 stops. On the synthetic tiles this also
  brings Highlight Priority −50's p95 over all tiles from 0.22 to 0.03 stop and −100's
  from 2.0 to 1.5.
- **Paint Overlay +30 with grain** is within ±0.02 stop in every quarter (the
  Panasonic S1M2ES −0.08 in the outer one). Grain's pattern is not Camera Raw's, so
  its blocks differ by noise: mean ΔE_ITP 2.7–6.1 against the default render's
  0.9–5.6 on the photos that pass by default.

Decision, 2026-11-11: Highlight Priority exposes through the master's neutral
response.

**The vignette's Highlights (2026-11-12).** On the tile checker (a linear profile),
Camera Raw 18.7's HDR Highlight Priority −50 keeps the gain it has without Highlights
up to twice white at every Highlights, and protects only four and eight times white,
almost in proportion to Highlights: the outer ring's gain at eight times white is 0.398,
0.457, 0.516, 0.575 and 0.634 at Highlights 0, 25, 50, 75 and 100 (0.377 to 0.431 at
four times), the same at HDR limits 4 and 8. Its SDR protection rises from about half of
white (0.456 at half, 0.558 at white). Color Priority −100 with Highlights 100 leaves
light from white up untouched in HDR (gain 1.000), where SDR's gain is 0.696 at white.
A rule fitted to these tiles would be checked on nothing else (the photo captures carry
no Highlights); the vignette's Highlights stays refused.

### Point Color

Measured with Camera Raw 18.7 on the Color Grading probe's colour patches (12 hues at
HSV saturations 0.3 and 0.6, 0.125 to 8 times white, a linear profile), HDR limit 8 and
SDR, with one swatch sampled at an orange (hue 30°, saturation 0.6, value 0.5):
Hue, Saturation and Luminance +50 and Luminance −50 at Lightroom's default ranges,
Hue and Luminance +50 with the whole luminance range, a blue swatch's Luminance −50,
and a swatch sampled at value 2 (which Camera Raw drops, as RAWmakase does).

- **The HDR domain is Color Grading's.** Luminance +50 brightens the orange ×1.32,
  ×1.68, ×2.18, ×3.51, ×6.06, ×3.54 and ×1.42 at 0.125 to 8 times white (SDR: ×1.32,
  ×1.54, ×1.58, then clipped). SDR's value gain of ×1.55 applied in u = x / (1 + 15x/16),
  clipped at u = 1, gives ×1.6, ×1.72, ×2.24, ×3.2, ×8 (held to ×6 by the selection),
  ×4.0 and ×2.0.
- **Selection.** Hue +50 turns the orange by 9.8°–19.9° up to white, 16.4° at twice
  white, 8.2° at four times and 1.8° at eight; with the whole luminance range 17.9°–
  19.9°, 16.4°, 8.6° and 3.8°. In u these colours lose saturation (an orange at eight
  times white has 0.12), so the swatch's saturation range selects less of them.
- **Engine.** `point_colors_hdr` (SDR's swatches on each channel in u, clipped at 1
  and mapped back; a pixel brighter than 16 is scaled to 16 and back) renders the
  effect on luminance within a median 0.001 stop of Camera Raw's for every swatch (p95
  0.005–0.096), and mean ΔE_ITP 0.39–1.31 over the colour patches, 0.42–2.42 above
  white (Luminance +50 the worst; the default render 0.28). RAWmakase's SDR Point
  Color on the same patches is within 0.27–0.51.

Decision, 2026-11-11: Point Color renders in the HDR master by this rule, on the CPU
(the GPU's HDR preview leaves it to it).

### Color grading

Measured with Camera Raw 18.7 on a synthetic probe through `probe_dng.py` (a linear
profile, sensor white +4 stops): 49 grays from 2^−8 to 16 times white in quarter stops,
and 12 hues at HSV saturations 0.3 and 0.6 from 0.125 to 8 times white. Each region's
tint at hues 30 and 210 (saturation 50), Highlights at hue 40 and saturation 100, every
region's Luminance at ±50 (Highlights at ±100 too), a blue Highlights tint at Blending
100 and Balance ±50, a blue Shadows tint at Balance −50 and a split tone, in HDR at
limit 8 and in SDR; six of them also at limits 2.3 and 4. Gains are each render's
ProPhoto channels against the same probe's default render.

- **Where the regions sit.** Below about a quarter of white the HDR gains are the SDR
  ones (the default renders agree there). Above, they are not SDR's gain held at white:
  Highlights' blue tint at saturation 50 is ×1.97 on blue at 2.8 times white (SDR's
  largest, at half of white, ×1.21) and ×1.00 at 16; Global's blue tint and Luminance
  +50 are a constant gain (×1.33, ×1.27) up to about 1.4 times white, fading to ×1.00
  at 16; Highlights Luminance −50 keeps darkening, ×0.375 at 16.
- **One domain explains them.** The SDR transfer of each channel, measured on the SDR
  ramp, applied in u = x / (1 + b x) and mapped back, renders every setting's HDR ramp
  to a mean 0.012 stop with b = 15/16 (16 times white at u = 1), and 0.010 with a
  fitted b of 0.945; a fitted power n in x / (1 + (b x)^n)^(1/n) gives 0.009 (n = 0.96).
  RAWmakase uses 15/16. The same rule applied to a shared scale of the pixel (its
  largest channel or its luminance) leaves 7–10 times the colour error, so the domain
  is per channel, as SDR's grading is.
- **The limit doesn't move it.** At limits 4 and 8 every gain is the same to 0.01; at
  2.3 they agree up to where the limit rolls off.
- **Engine.** `ChannelCurves::apply_hdr` renders the probe's effect on luminance to a
  median 0.000–0.008 stop on the grays (worst sample 0.285, Highlights Luminance −50 at
  16 times white, where Camera Raw's ×0.375 is RAWmakase's ×0.31), and on the colour
  patches above white to a median 0.000–0.106 stop (p95 ≤ 0.195). Colour above white:
  mean ΔE_ITP 0.3–3.7 for Shadows, Midtones, Global and Luminance, 3.4–12.2 for the
  Highlights tints (blue under Balance +50 or Blending 100 the worst, 12.1–12.2;
  hue 40 at saturation 100, 8.8); the default render is 0.2. Camera Raw lifts the
  lowest channel of a saturated colour under a strong tint above white (a cyan at
  four times white: red 1.60 → 2.10 under a blue Highlights tint, which the
  per-channel rule takes to 1.22). RAWmakase's SDR grading of the same patches is
  within mean ΔE_ITP 0.2–1.8 of Camera Raw's SDR.

Decision, 2026-11-10: Color Grading renders in the HDR master by this rule, after the
colour mixer as in SDR, and on the GPU's HDR preview. The strong-tint colour error
above white stays open; Camera Raw captures of photos with grading were paused when
this was decided, so the photo validation followed.

**Photos (2026-11-11).** A split tone (Shadows hue 210 saturation 25, Highlights 40/30),
a Highlights tint 40/40 with Highlights Luminance +30, and a Global tint 210/20 with
Midtones Luminance −20, Blending 50, Adobe Standard DCPs, HDR limit 4, on the twelve
photos of [Validation](#validation): median above white 0.002–0.086 stop, p95
0.004–0.16 but on the two Panasonics (0.40–1.64, as at their defaults), mean ΔE_ITP
within 0.5 of each photo's default render (the Z 8 3.2–3.3 against 2.9), peaks within
the default's but on the Z50 II with the Global tint (+0.169 against +0.082, past the
0.1-stop tolerance its default meets; its median above white is 0.021). The other
photos that fail a tolerance fail it at their default render for the reasons under
[Validation](#validation).

SDR renders do not change: `ChannelCurves::apply` and the GPU's SDR `grade` are the
same code as before, and `apply_hdr` is reached only from the HDR master's finish.

### Defringe

Measured with Camera Raw 18.7 on the Color Grading probe's colour patches (12 hues at
HSV saturations 0.3 and 0.6, 0.125 to 8 times white, a linear profile), Purple Amount
20 (hues 30–70) and Green Amount 20 (hues 40–60), HDR limit 8 and SDR; saturation kept
is the patch centre's HSV saturation in linear ProPhoto against the default render's.

- **HDR removes the same share at every brightness.** Purple: the 270°–330° patches at
  saturation 0.6 keep ×0.83, ×0.04, ×0.05, ×0.01 at 0.125 to four times white, the same
  to 0.01–0.04 at every level. Green: the 120°–150° patches keep ×0.10–0.17 and
  ×0.03–0.09. SDR keeps more towards its top (Purple ×0.70 and ×0.62, Green ×0.65 and
  ×0.81 at its brightest patches), where the rendition rolls colours off to white. At
  eight times white Camera Raw's renders of these patches change hue (the probe's
  highlight handling) and the selected patches are others; that level is left out.
- **Engine.** `defringe_hdr` takes a colour with its brightest channel at white when
  brighter, applies SDR's operator there and scales it back. Rendered by the build, the
  share kept is the same at every level from white up (Purple ×0.11, ×0.07, ×0.65;
  Green ×0.09, ×0.07, ×0.92) and the hues selected are SDR's. The error against Camera
  Raw is SDR's selection, which RAWmakase's SDR render of the same patches shows too
  (Purple ×0.20, ×0.12, ×0.66 kept at a quarter of white where Camera Raw keeps ×0.04,
  ×0.05, ×0.01; mean ΔE_ITP 2.3 in SDR). Mean ΔE_ITP against Camera Raw's HDR render,
  default 0.26: Purple 5.7 (6.8 above white), Green 3.2 (5.5).

Decision, 2026-11-10: Defringe renders in the HDR master by this rule, on the CPU (the
GPU's HDR preview leaves it to it). Its hue selection is SDR's, with SDR's error.

**Photos (2026-11-11).** Camera Raw 18.7 rendered the twelve photos of
[Validation](#validation) with Purple Amount 8 (hues 30–70) and Green Amount 4 (hues
40–60), in HDR (limit 4) and in SDR; each edit's effect is its chroma (ICtCp Ct, Cp) in
8× blocks against the photo's default render, RAWmakase's against Camera Raw's, on the
blocks either changes most (the top 1% of each).

| | Camera Raw HDR | Camera Raw SDR | RAWmakase SDR | RAWmakase HDR |
| --- | --- | --- | --- | --- |
| Chroma kept below white, on the blocks Camera Raw's HDR changes (four photos) | ×0.67–0.74 | ×0.45–0.63 | ×0.41–1.00 | ×0.39–1.00 |
| Z 8 sky, 3–99 times white (8,643 blocks) | ×0.47 | ×1.00 | ×1.00 | ×1.00 |
| Z 8 sky, 1–3 times white (2,875 blocks) | ×0.73 | ×1.00 | ×1.00 | ×1.00 |

- **Below white** Camera Raw's HDR Defringe removes less than its SDR one on the same
  blocks; RAWmakase's master removes what its SDR render removes.
- **Above white** Camera Raw takes a bright, saturated part of the Z 8's blue sky
  (Oklab hue 0.653–0.677, chroma 0.08–0.10, at 2.7–3.7 times white), a smooth patch,
  not edges, that neither SDR render touches; the paler, brighter rest of the sky
  (×5 white, chroma 0.06) it keeps. Over all twelve photos, bright blues (hue 0.60–0.70)
  keep a median ×0.81–1.00 at every level, so the selection depends on more than hue
  and brightness.
- **Relative error** (mean |RAWmakase's change − Camera Raw's| over Camera Raw's mean
  change, on those blocks): median 0.94 in SDR and 1.49 in HDR (means 0.99 and 1.58);
  HDR is worse than SDR by more than 0.2 on 8 photos, from 0.94 to 1.29 on the Z 8 and
  from 1.17 to 3.29 on a Sony ARW (DSC03793). Above 1, leaving Defringe out would be
  closer to Camera Raw's master.
- **Color Grading's HDR domain** (each channel through x/(1 + 15x/16), the operator, and
  back) leaves RAWmakase's change on the photos within 4% of what it was and worsens the patches: Purple
  mean ΔE_ITP 5.8 (8.6 above white), Green 6.3 (12.1), against 6.0 (7.6) and 3.0 (5.3).

Decision, 2026-11-11: Defringe refuses the master again ("Defringe is not supported in
HDR yet"); the edit and its SDR rendition keep it.

Re-examined on 2026-11-12 under the [acceptance rule](#decisions), with RAWmakase's HDR
renders of the twelve photos (CC0 copies) through #1002's operator, its change scaled to
a fraction of SDR's. The relative error here is taken over the 2% of blocks either render
changes most, below white (both of Camera Raw's renders below it) and above white (its
HDR base above it), the median over the photos; SDR's own below white is 1.49 (above 1:
in SDR too, leaving Defringe out is closer to Camera Raw).

| HDR change, of SDR's | Below white | Above white |
| --- | --- | --- |
| ×1 (#1002) | 3.78 | 1.29 |
| ×0.6 (Camera Raw's HDR removal over its SDR one below white, 0.30 / 0.46 kept-chroma loss) | 1.86 | 1.00 |
| ×0.4 | 1.20 | 1.00 |

Only a change weaker than Camera Raw's own HDR one passes, and only because it comes
closer to leaving Defringe out, which SDR's operator is already further from Camera
Raw than; the bright blues Camera Raw takes above white are not selected at all (above
white ≈ 1.00, no change). **Defringe stays refused in HDR**; it needs SDR parity of the
hue selection first.

### Masks

A mask's adjustment is added to the photo's own in RAWmakase's stages: its Temp and
Tint scale the white balance, its Exposure the scene, its Shadows the local tone map's
gain and its Texture the texture image, each weighted by the mask, and the HDR master
renders those stages as it renders the global sliders. Rendered by RAWmakase, a mask
over the whole frame gives the global slider's master to 0.2% (Exposure +1, Shadows
+50, Texture +40, on gray and coloured patches from 0.02 to 3 times white). Range masks
select from the SDR rendition (the edit developed with HDR off), so a Luminance Range
mask brightens the same patches of a gray ramp in the master as in the SDR rendition.

Decision, 2026-11-10: masks render in the HDR master with Temp, Tint, Exposure,
Shadows and Texture; any other slider in an active mask refuses it by name.
Decision, 2026-11-11: Contrast, Color, Sharpness and Noise render too (below). Contrast,
Whites, Blacks, Hue, Saturation and Color are operators of their own, fitted on SDR
renders, and Sharpness and Noise work on the SDR rendition's detail; Highlights has no
HDR rendering globally, and a mask's Clarity and Dehaze aren't measured. A masked master renders on the CPU;
the GPU's HDR preview doesn't port masks yet.

**Camera Raw (2026-11-11).** Camera Raw 18.7 rendered the Color Grading probe's patches
(grays from 2^−8 to 16 times white and colour patches, a linear profile, HDR limit 8
and SDR) with a radial mask covering the whole frame, each slider alone, and with the
global slider of the same value. Effects are luminance in stops, medians over the
patches (and ΔE_ITP means):

| Slider | Camera Raw's mask against its global slider | RAWmakase's mask against Camera Raw's | RAWmakase's global slider against Camera Raw's |
| --- | --- | --- | --- |
| Exposure +1 | 0.000 (0.39) | 0.001 (0.24) | — |
| Exposure +1 at Amount 50 | 0.001 (0.36) | 0.002 (0.23) | — |
| Shadows +50 | 0.039 (3.8) | 0.061 (3.6) | 0.030 (2.8) |
| Texture +50 | 0.000 (0.0) | 0.027 (8.1) | 0.027 (8.1) |
| Temp +30 | — | 0.106 (11.6) | — |
| Contrast +50 | 0.165 (9.6) | refused | |
| Whites +40 / −50 | 0.140 / 0.159 (6.3 / 7.0) | refused | |
| Blacks −50 | 0.287 (27.9) | refused | |
| Saturation +50 | 0.059 (10.6) | refused | |
| Highlights −50 | 0.115 (8.1) | refused | |

- A mask's Exposure, Shadows and Texture are their global sliders in Camera Raw's
  HDR render too. RAWmakase's mask renders Exposure to 0.001 stop and Texture to the
  global operator's own error (Texture's flat patches carry the global Texture's
  error); its Shadows is 0.061 stop off, twice the global slider's 0.030 (Camera Raw's
  own mask and global Shadows differ by 0.039).
- A mask's Temp is a gain of the scene in Camera Raw's HDR render, the same from 0.01
  to 8 times white (red ×1.12, blue ×0.84 at +30), as RAWmakase renders it; RAWmakase's
  local Temp is weaker (red ×1.015, blue ×0.92), as in SDR, where its mask is 0.064
  stop and ΔE_ITP 6.5 from Camera Raw's.
- Contrast, Whites, Blacks, Saturation and Highlights in a mask are not their global
  sliders in Camera Raw either (0.06–0.29 stop).

**A mask's own operators (2026-11-11).** Scored by the
[acceptance criterion](#decisions) on the same captures: each slider's effect (the
masked render's luminance over the base render's, in stops) against Camera Raw's, its
median and p95 over the patches, and the mean ΔE_ITP of the masked renders; HDR and SDR
on the 155 patches below 0.9 in Camera Raw's SDR base render, and HDR alone on the 87
above white.

| Mask slider | HDR below 0.9 | SDR below 0.9 | HDR above white | Result |
| --- | --- | --- | --- | --- |
| Contrast +50 (below white only) | 0.047 / 0.228, 4.6 | 0.058 / 0.229, 4.8 | 0.000 / 0.012, 0.4 | renders |
| Color, hue 40 (fading out by white) | 0.037 / 0.089, 9.4 | 0.056 / 0.101, 14.2 | 0.000 / 0.004, 0.4 | renders |
| Color, hue 40, not fading | 0.047 / 0.097, 22.7 | 0.056 / 0.101, 14.2 | 0.020 / 0.065, 34.1 | |
| Saturation +50 | 0.081 / 0.304, 10.4 | 0.027 / 0.197, 8.9 | 0.078 / 0.269, 10.5 | refused |
| Whites +40, SDR's curve | 0.069 / 1.26, 22.9 | 0.003 / 0.034, 1.1 | 2.05 / 3.32, 122 | refused |
| Whites −50, SDR's curve | 0.080 / 1.02, 16.7 | 0.001 / 0.002, 0.2 | 1.80 / 3.06, 108 | refused |
| Blacks −50, SDR's curve | 0.097 / 1.36, 23.8 | 0.003 / 0.057, 1.4 | 1.97 / 3.34, 119 | refused |
| Shadows +50 (rendered since #1003) | 0.071 / 0.178, 4.1 | 0.033 / 0.173, 3.1 | 0.045 / 0.077, 2.5 | |
| Temp +30 (rendered since #1003) | 0.107 / 0.117, 11.5 | 0.086 / 0.115, 8.2 | 0.106 / 0.113, 12.2 | |
| Texture +50 (rendered since #1003) | 0.018 / 0.078, 9.0 | 0.025 / 0.070, 6.6 | 0.029 / 0.036, 7.6 | |

- **Contrast.** Camera Raw's HDR mask Contrast is its SDR mask's below white, as a
  function of the linear value (+0.112 stop at 0.71 times white, SDR's +0.11 there),
  and changes nothing from 1.19 times white up. RAWmakase renders SDR's mask curve
  (the global Contrast curve at the mask's value, which is stronger than Camera
  Raw's in the shadows, −1.48 stop at 0.001 against −0.50, in SDR too) up to white
  and the identity above.
- **Color.** Camera Raw's HDR mask Color tints as its SDR one up to a quarter of
  white (saturation ×1.08), less at half of white (×1.06 against SDR's ×1.08) and not
  at all from white up, where SDR's tint fades only by clipping. RAWmakase applies
  its SDR tint of the scene in full up to a quarter of white and fades it out by
  white on the pixel's brightest channel (smoothstep).
- **Saturation.** Camera Raw's HDR mask Saturation +50 multiplies HSV saturation by
  1.20–1.21 at every level from an eighth of white to eight times white, value
  ×1.004. RAWmakase's SDR mask Saturation (Oklab chroma ×1.5) gives ×1.35 and value
  ×1.11 in the master at every level, 0.098 stop off; in SDR, white's clip takes
  most of that back above half of white, so SDR scores better. Hue is the same Oklab
  operator and is not measured; both stay refused.
- **Whites and Blacks.** Camera Raw's HDR mask Whites +40 and −50 have one shape:
  +0.17 / −0.21 stop at 0.001 times white rising to +0.33 / −0.40 at 16 times, 0.87
  of the SDR mask's largest effect (+0.375 / −0.461); below white they are not its SDR
  mask's (+0.230 at 0.015 against SDR's +0.282). Its HDR mask Blacks −50 crushes
  further than SDR's (−4.15 stops at 0.049, SDR −2.28) and brightens above white
  (+0.05 at 1.19, +0.39 at 16). RAWmakase's SDR mask curves are within 0.003 stop of
  Camera Raw's SDR renders and fall steeply above white. With one setting each, a fit
  to these shapes could not be checked on another; they stay refused.
- **Sharpness and Noise** render as SDR's operators on the master's encoded values,
  as global sharpening does: they change detail, not tone (a flat patch keeps its
  light, above white too). Not measured against Camera Raw.
- **Already rendered.** Mask Shadows and Temp, rendered since the decision of
  2026-11-10, score worse in HDR than in SDR on these patches (0.071 against 0.033
  stop, and 0.107 against 0.086). They stay rendered: the criterion is applied to
  controls the master refuses, and refusing them now would take the master from
  edits that have it.

**Clarity and Dehaze in a mask (2026-11-12).** After global Clarity (0 to +50) and
Dehaze (±30) came to render in the master, Camera Raw 18.7 rendered a whole-frame mask
with Clarity +25 and +50 and Dehaze +30 and −30 on three validation photos (Nikon Z 8
and Z f, Sony A7R III; Adobe Standard, HDR limit 4 and SDR). The mask's value adds to
the photo's own in the global sliders' stages, so RAWmakase's whole-frame mask renders
the global slider's master (a test). Its effect against Camera Raw's (median stops on
factor-8 blocks, mean ΔE_ITP):

| Photo, mask slider | HDR below 0.9 | SDR below 0.9 | HDR above white |
| --- | --- | --- | --- |
| Z 8, Clarity +25 / +50 | 0.323, 18.7 / 0.604, 30.6 | 0.284, 17.5 / 0.492, 26.7 | 0.032 / 0.065 |
| Z 8, Dehaze +30 / −30 | 0.061, 11.4 / 0.235, 17.4 | 0.060, 11.2 / 0.209, 16.4 | 0.110 / 0.043 |
| Z f, Clarity +25 / +50 | 0.083, 9.1 / 0.165, 12.6 | 0.081, 8.7 / 0.163, 12.1 | 0.007 / 0.014 |
| Z f, Dehaze +30 / −30 | 0.097, 10.5 / 0.056, 11.4 | 0.080, 9.4 / 0.044, 11.1 | 0.071 / 0.019 |
| A7R III, Clarity +25 / +50 | 0.050, 3.7 / 0.093, 6.4 | 0.050, 3.5 / 0.093, 6.0 | 0.130 / 0.235 |
| A7R III, Dehaze +30 / −30 | 0.104, 6.0 / 0.235, 12.1 | 0.104, 5.9 / 0.234, 12.0 | 0.096 / 0.733 |

The master is further from Camera Raw than the SDR rendition on every photo and
setting, by colour where not by luminance, and on the A7R III's small highlights above
white by up to 0.73 stop. Mask Clarity and Dehaze stay refused.

### Decisions

- **When a refused control renders** (2026-11-11). A control the master refuses
  renders in it when RAWmakase's HDR error against Camera Raw is no worse than its
  SDR error against Camera Raw, for the same control on the same photos or probes: a
  refusal costs the whole edit its HDR master, so the master need only be as faithful
  as the SDR rendition is. The error is the control's effect (the edit's render over
  the same photo's base render, luminance in stops) against Camera Raw's, its median
  and p95, with the mean ΔE_ITP of the edited renders; HDR and SDR are compared over
  the pixels or patches both render below white, and the HDR error above white must
  be no worse than SDR's below it. Where the same operator renders HDR and SDR below
  white, "no worse" there allows 0.01 stop (the HDR and SDR default renders' own
  difference), and the mean ΔE_ITP compares what the control adds to the default
  render's, within 1 ([Presence](#presence)). Controls rendered before this decision are not
  re-scored by it.
- **The SDR rendition is the edit with HDR off.** Camera Raw's own is a local tone
  map of the HDR master ([SDR rendition](#sdr-rendition)); the SDR rendition sliders
  never change the master, which renders with them; the SDR rendition doesn't render
  them until that tone map is matched.
- **Unmeasured adjustments refuse the HDR master, not the edit.** An imported or
  edited HDR edit with Dehaze beyond ±30, a look or Whites above +50 keeps HDR on and stays
  valid; its SDR rendition renders it, and the master (HDR export) is refused with
  the reason, shown in Develop under the HDR switch. Refusing the edit itself would
  make Develop's ordinary sliders fail to render whenever HDR is on.
- **JPEG XL interoperability (stage 6, #894).** These decisions are recorded in
  [JPEG XL interoperability](hdr-jxl-interop.md#decisions):
  - The HLG workaround stays: jixel 0.5.7, the newest release, still reads HLG input
    as scene light, and libjxl reads RAWmakase's HLG header correctly.
  - PQ files keep cjxl's intensity target of 10,000 cd/m², not the content's peak.
    ImageIO ignores it, and libjxl uses it only when asked to tone map.
  - HLG exports are compared with the light HLG can represent (BT.2100's per-channel
    clip after the inverse OOTF).
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

- **Fitted Camera Looks render in HDR by the rules of their parts** (2026-11-10). A
  fitted look is an HSV table and a curve over RAWmakase Standard or a fitted matrix
  with the ACR3 curve, the family measured above; it has no Camera Raw counterpart to
  measure, and the camera's own JPEG it is fitted to has no HDR rendering, so the
  measured rules for each part are the reference.
- **A look's settings are checked as the user's sliders.** The master's recipe adds
  them to the user's (`Recipe::with_profile_adjustments`), so whatever the same slider
  renders, the look's renders, and a refusal names the look ("Highlights, which the
  look sets").
- **Preview for SDR display is a view choice** (stage 5, 2026-11-10), as the plan
  has it: Camera Raw 18.7's returned settings carry no key for it in any of the HDR
  captures (they carry `HDREditMode`, `HDRMaxValue` and the seven `SDR*` sliders), so
  it is never saved, copied or synced. While it is on, Develop shows the SDR rendition
  on an HDR display, with its histogram, as Lightroom does; it applies to every photo
  and every side until turned off.

## Open

- Whites above +50 (not a gain of one photo level: [Whites](#whites)) and Highlights
  in HDR (a rule trained on HDR targets:
  [Highlights and Shadows](#highlights-and-shadows)); the colour mixer's Luminance beyond
  ±40, where Camera Raw's HDR effect strengthens with brightness and the SDR grids don't
  ([Band Luminance](#band-luminance)); Point Color on photos and on the GPU's HDR preview
  ([Point Color](#point-color)).
- Defringe in HDR: Camera Raw's HDR Defringe removes less than its SDR one below
  white and takes some bright blues above it; RAWmakase's hue selection misses Camera
  Raw's in SDR too ([Defringe](#defringe)).
- Channel curves' peak on photos: the Z50 II's brightest block is 0.19 stop high, the
  Z 8's 0.11, where their median and p95 pass ([Tone curves](#tone-curves)).
- Strong Highlights tints of saturated colours above white, and the Z50 II's peak with
  a Global tint ([Color grading](#color-grading)).
- The vignette's Highlights in HDR, and Highlight Priority −100 in a frame's darkest
  tiles (p95 1.5 stops on the synthetic tiles) ([Vignette and grain](#vignette-and-grain)).
- Refine Saturation in HDR ([Tone curves](#tone-curves)).
- Masks: a mask's Whites, Blacks, Hue and Saturation (one Camera Raw setting each,
  unlike SDR's operators above white), Temp's strength (SDR's), Shadows' HDR error
  (twice SDR's on the patches), and masks on the GPU's HDR preview ([Masks](#masks)).
- Negative Clarity, Clarity above +50, and Dehaze beyond ±30 in HDR ([Presence](#presence)).
- Positive Shadows lifts small speculars in dark surroundings above Camera Raw's peak
  (0.2–0.3 stop; [Highlights and Shadows](#highlights-and-shadows)).
- Camera Raw's default SDR rendition (a local tone map, [SDR rendition](#sdr-rendition))
  and its sliders.
- Looks that set Clarity above +50 or Highlights, and split toning, stay refused (Adobe Vivid sets Clarity +10). A B&W `.cube`
  renders saturation-0.9 red and magenta out of its gamut almost black above white in
  Camera Raw ([Looks](#looks)); RAWmakase renders their gray.
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
  parametric curve, the channels' curves and Levels ([Tone curves](#tone-curves)); Texture ([Presence](#presence)); Saturation, Vibrance, the
  colour mixer's Hue and Saturation, Calibration and Black & White
  ([Colour stage](#colour-stage)); band Luminance up to ±40
  ([Band Luminance](#band-luminance)); Whites up to +50 ([Whites](#whites)); Shadows
  ([Highlights and Shadows](#highlights-and-shadows)); looks with RGB tables, B&W,
  their own settings, any Profile Amount, and fitted Camera Looks ([Looks](#looks));
  Color Grading ([Color grading](#color-grading)); the post-crop vignette and grain
  ([Vignette and grain](#vignette-and-grain)); masks with Temp,
  Tint, Exposure, Contrast, Shadows, Texture, Color, Sharpness and Noise ([Masks](#masks)); Point Color ([Point Color](#point-color)); Clarity from 0 to +50 and Dehaze up to ±30 ([Presence](#presence)).
  Open: Whites above +50, Highlights, the colour
  mixer's Luminance beyond ±40, Defringe ([Defringe](#defringe), refused again 2026-11-11), looks that set Clarity above +50 or Highlights, the SDR rendition sliders in the SDR rendition (the master renders
  with them), 52552 grouping (library only,
  needed when toggling HDR pairs profiles).
- [x] Stage 6: HDR TIFF export (PQ and HLG 16-bit, linear float; Rec. 2020, Display
  P3, Rec. 709; CICP-tagged ICC; #908), checked against Camera Raw's PQ TIFF through an
  independent decoder; lossless JPEG XL PQ and HLG (and SDR sRGB), checked with
  libjxl's `jxlinfo` and `djxl` and jxl-oxide; gain-map JPEG (ISO 21496-1) checked with
  libultrahdr; JPEG XL gain maps (`jhgm`), the bundle read by libjxl's `jxlinfo`. A
  resized master lost its light above white until #910. Lossy JPEG XL (HDR, gain
  maps and SDR) through jixel, checked with `djxl` and jxl-oxide (#906). JPEG XL PQ, HLG
  and gain maps pass the interoperability harness: libjxl, jxl-oxide, ImageIO and
  Chrome; the gain map's reconstruction is computed independently from ISO 21496-1.
  Lossless 16-bit files now declare codestream Level 10 (#1030). The queue's
  cancellation, failure and publication are tested for HDR JPEG XL
  ([JPEG XL interoperability](hdr-jxl-interop.md)). AVIF and HEIC HDR export are not
  planned for #894.
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
  highlight triangle. Preview for SDR display, a view choice showing the SDR rendition
  on an HDR display. Open: the rest of the stage (above).
