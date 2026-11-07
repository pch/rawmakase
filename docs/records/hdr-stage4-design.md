# HDR stage 4: opening HDR files as HDR edits

Status: Implemented (first increment; gain-map input in a second, [below](#gain-map-input-second-increment)). Agreed after three review rounds. It covers the first increment of stage 4 of the
[HDR implementation plan](hdr-implementation-plan.md): an HDR file (PQ, HLG or
linear light) opens in Develop as an HDR edit instead of being refused
([HDR files](../hdr.md#hdr-files)). Gain-map inputs, HEIC and adjustments other
than Exposure come later. Revision 1's review asked for:

- a precision-qualified decoder;
- a default limit that contains the peak;
- separate identity contracts for the master and for export;
- enforced invariants for the HDR buffer.

Revision 2's review asked for:

- the export domain bounded by each transfer's own clipping;
- explicit clone and reduction operations.

## Goals

- **Master identity.** At neutral settings, an HDR file's master is its light
  decoded by an exact transfer and matrix path, within float rounding. That light
  is linear ProPhoto with 1.0 at graphics white (203 cd/m²), signed values kept. It
  is tested against an independent decoder of the same signal, not against
  RAWmakase's own decoding.
- **Export round trip,** a separate contract. Exporting a master and opening the
  file again gives back the light that was encoded, within the codec's
  quantisation, for light inside the domain export represents.
  - Information is lost by design outside that domain: out-of-gamut channels clip,
    the limit rolls off at its knee, in ProPhoto and again in the output gamut,
    and each transfer clips at its own bounds.
  - PQ clips at 10,000 cd/m².
  - HLG clips each channel at 1 after the inverse OOTF, on 1,000 cd/m², so a
    saturated colour clips below that peak.
  - Round-trip fixtures stay inside all of these bounds, and separate tests show
    the clipping.
- **One SDR rendition.** The Library preview and neutral Develop with HDR off show
  the same SDR rendition: RAWmakase's tone map (`raster::tone_map`, which puts
  graphics white at 0.926 and rolls brighter light off into 1), through the
  rendered-file pseudo-scene. They agree within interpolation and 8-bit
  quantisation. This is RAWmakase's policy: Camera Raw's SDR rendering of an HDR
  file is not measured, and no parity is claimed.
- **No extrapolation.** The inverse ACR3 curve is never extended above white, and
  no raw camera's default tone is applied.
- **Display-independent data.** No decoded or cached pixel depends on the monitor.

## The HDR source

A new type in `rawmakase-model`, `camera_data::HdrLight`, is constructed only by
the decoder:

- `width` and `height`, equal to the paired SDR image's.
- `pixels: Arc<[[f32; 3]]>`: linear ProPhoto, 1.0 at graphics white, finite,
  signed.
- `transfer`: PQ, HLG or linear.
- `peak`: the largest finite channel value of the full-resolution light.

Its fields are private, with read accessors. The constructor checks the
dimensions against the pixel count and replaces non-finite values with 0. It is
immutable once built: clones share the `Arc`.

`CameraImage` gains a private `hdr: Option<Arc<HdrLight>>`, with these operations:

- **Pairing.** `CameraImage::with_hdr(sdr, light)` is the only way to set it. It
  checks both have the same dimensions and sets `Metadata::hdr_source` (the
  serialisable fact: `#[serde(default)]`, transfer and peak) from the light.
- **SDR working copies.** `Clone` is written by hand and drops `hdr`, so every
  copy is SDR-only unless a call asks otherwise. That covers every working image:
  recovery, noise reduction, texture, retouching, AI Denoise, Camera Look
  indicators, the measurement copy (`measurement_source`) and the pyramid (its
  level 0 included).
- **Preview reduction.** The one call that keeps HDR is
  `sampling::preview`, through `CameraImage::reduced_with_hdr`. It reduces both
  buffers with the same footprint. For a small image it shares the light rather
  than cloning: `with_hdr` on the SDR copy and the same `Arc`.
- **Reduced light.** `HdrLight::reduced(&self, width, height, footprint)` is the
  second validated constructor, beside the decoder's. The engine calls it for
  that reduction alone. It checks the new dimensions and keeps the full-resolution
  `peak`.
- **No mismatched pairs.** `CameraImage`'s pixels and dimensions are public and
  mutable, but nothing that changes them goes through a call that keeps HDR.
  `CameraImage::hdr()` returns the light only while its dimensions still match
  the image's; a mismatch is a programming error, refused in release builds and an
  assertion in debug builds.

`render_hdr` doesn't dispatch on whatever image it is given. Its HDR-file path
takes the `HdrLight` and the photo's `Metadata` explicitly
(`render_hdr_light(light, metadata, recipe, max_edge)`). Callers get them from the
document's full image (or its preview reduction), never from a working image.
A deserialised `hdr_source` without a decoded `HdrLight` renders nothing:
`render_hdr` refuses.

## Decoding

`raster::open` decodes an HDR file twice from the same samples:

- **The HDR light, exactly.** This increment supports CICP-signalled files whose
  primaries are BT.709 (1), BT.2020 (9) or Display P3 (12, JXL's 11).
  - The transfer is inverted in f32 by `color::hdr_output`: `pq_decode`, or
    `hlg_inverse_oetf` and then the luminance OOTF, or identity.
  - HLG's OOTF works in the file's primaries, before their conversion, with that
    gamut's native luminance weights (`HdrGamut::luminance`). The ICC colorants are
    D50-adapted, so the preview's weights from them are replaced by these too.
  - The primaries go to ProPhoto with the inverse of the gamut's published matrix
    (`HdrGamut::prophoto_to_rgb`).
  - The light is then scaled so that 1.0 is graphics white.
  - Linear profiles without CICP whose curves are identity (`raster::is_identity`)
    use their ICC colorant matrix, which is already D50-adapted, to XYZ D50 and
    ProPhoto, in f64.
  - Other primaries, and HDR without CICP or identity curves, stay refused with
    the reason.
- **The SDR pseudo-scene:** `to_scene(tone_map(light))`, as previews show it.

Orientation is applied once, to the samples, before both are built; JPEG XL's
decoder has applied it already. Neither path goes through moxcms's bounded tables.

Formats: TIFF, PNG, JPEG XL and AVIF. HDR HEIC, gain maps, animation and the
existing refusals are unchanged.

## The master of an HDR file

`render_hdr_light`:

1. Exposure as a gain of 2^EV.
2. Geometry: the same `Geometry`, frame and output-size rounding the SDR rendition
   uses for that recipe and metadata, so the two line up.
   - Supported: orientation, crop and its aspect constraint, straighten, Rotate
     and Flip.
   - Refused: Transform and Upright, until they are shared.
3. Resizing with `resize_unbounded`.
4. `roll_off` to 2^limit.

`model::hdr::check_recipe` takes the source facts. For an HDR file it compares the
effective recipe (`as_rendered`, so a switched-off panel counts as off) with
`Recipe::for_rendered`'s defaults. It refuses the master, never the edit, for any
difference other than Exposure, the geometry above and the HDR limit. Each refusal
is named:

- white balance, incremental Temperature and Tint included;
- the profile, its Amount and its looks;
- every tone and colour slider;
- sharpening, noise reduction and AI Denoise;
- Heal, Clone and Red Eye;
- lens corrections, local edits, the SDR rendition sliders, Transform and Upright.

The Basic panel's note, Develop's HDR preview, export and `render_hdr` all read
this one check.

## Defaults and intent

`HDREditMode` is editing intent, not the source's encoding. A newly opened,
positively identified HDR file starts with HDR on (`Recipe::for_rendered` reads
`hdr_source`), as the plan's stage 4 asks. Lightroom's opening default for such
files is not measured. `HdrSettings::default()` stays off, so saved recipes without
HDR fields read as before.

A saved edit wins wholesale (`edits::resolve`/`Start`): a catalog edit of a file
refused before keeps every saved setting, HDR off included. An XMP
`HDREditMode="0"` keeps HDR off. Missing mode and missing limit are tested apart.

The default limit is the smallest whole number of stops whose knee (97% of
2^limit) contains the peak: `ceil(log2(peak / 0.97))`, clamped to the model's
range.
- A peak below 1, or a black image, gets the model's minimum.
- A peak beyond the maximum's knee gets the maximum. Light above that knee then
  rolls off: the identity holds up to the knee, and the note says so.

The default is computed once, from the full-resolution `peak`, never from a
reduction.

## Caches and lifecycle

- **The decode cache** stores RAWs only. Rendered files bypass it at every caller
  (loader, reference, Library renders, batch export). The boundary becomes an
  assertion in the cache's store.
- **The stage cache** isn't used by the master. If HDR light ever enters cached
  stages, its key must hold the rendition (HDR or SDR) as well as the source
  allocation.
- **Develop's HDR preview** key already holds the source allocation (held weakly),
  the recipe, the edge and the headroom. A replaced file is a new allocation. Its
  reduction keeps the HDR light, and a late or failed render is handled by the
  existing attempt checks.
- **Memory:** an HDR file holds both buffers (24 bytes per pixel twice). Working
  images never carry the HDR buffer, so it isn't held twice.

## Tests

- **Independent decoding.** PQ, HLG and linear TIFFs, and the PQ and HLG JPEG XL and
  AVIF fixtures, are decoded against BT.2100's formulas written out in the test,
  with f64 matrices. The test pixels cover deep shadows, graphics white, highlights
  near the peak and saturated colours. Tolerance: f32 rounding for float input; half
  a code value of the signal for 16-bit input.
- **Signed and non-finite.** A float TIFF with negative channels keeps them; NaN
  and inf become 0.
- **Default limit.** Peaks exactly at, just below and just above a knee; a black
  image; a peak below 1; a peak beyond the maximum.
- **Master identity.** At neutral settings the master equals the light, below the
  knee, within f32 rounding, at full size and at an exact 2:1 reduction.
- **Export round trip.** Colours inside every bound of the domain open again to
  the encoded light within the codec's quantisation. That means in the output
  gamut, below the knee in ProPhoto and in the output gamut, under 10,000 cd/m² for
  PQ, and with every HLG channel at most 1 after the inverse OOTF. They are
  exported as PQ, HLG and linear TIFF and as JPEG XL.
  - Clipping tests show what does not come back: an out-of-gamut colour, light
    above the knee, PQ above 10,000 cd/m², and Rec. 2020 `[4, 0, 0]` in HLG,
    which reads back near `[3.77, 0, 0]`.
- **SDR rendition.** Neutral Develop equals the Library preview within 1 level, on
  CPU and GPU, with HDR-derived ramps and saturated colours.
- **Geometry.** All eight EXIF orientations, with crop, straighten, Rotate and Flip,
  line up the master and the SDR rendition. Transform and Upright refuse the master.
- **The capability check.** Every refused setting refuses the master with its
  reason and leaves the edit untouched. Exposure scales the master.
- **Intent.**
  - A saved native edit and a Lightroom edit keep their HDR off.
  - Absent HDR fields, an explicit off, a missing mode and a missing limit are
    each checked.
  - Formerly refused files with catalog edits keep them.
- **Derived images.**
  - `Clone` drops the light, and so does every working-image derivation, the
    measurement copy and the pyramid's level 0 among them.
  - Preview reductions carry a reduced light of their own size, with the source's
    peak; that covers odd sizes, and small images, which share it.
  - A light whose dimensions no longer match is refused.
- **Lifecycle.** A replaced or deleted file, a late completion and a failure in the
  HDR preview; opening again decodes a new light.
- **Refusals.** HDR HEIC, animation, unsupported primaries and HDR without CICP stay
  refused. Gain maps were refused in the first increment and open since the second.

## Gain-map input (second increment)

ISO 21496-1 gain-map JPEG and JPEG XL files open as HDR edits
([Gain-map files](../hdr.md#gain-map-files)), on the source contract above. The
decisions this increment made:

1. **Full weight, never the display's.** The light is the alternate rendition at the
   map's full weight, as a display with the alternate's whole headroom shows it.
   The headroom value itself never scales the light, and no display is read. This
   keeps decoded pixels display-independent, as the plan asks.
2. **The base is the SDR rendition.** The base is decoded as the same file without
   its map. That keeps the authored SDR rendition at neutral settings, and keeps
   catalog edits made while these files opened as SDR rendering as they did.
3. **ISO 21496-1 only.** XMP-only (`hdrgm`) maps open as their SDR base, as before; a
   reader for them is a follow-up.
4. **Refuse what isn't applied exactly.** Each of these is refused by reason, not
   approximated:
   - a map in the alternate's colour space, with its own ICC or colour encoding;
   - an HDR base;
   - a grey or table-profile base;
   - a turned JPEG XL;
   - a `brob`-compressed bundle;
   - any damage.
5. **Centre-aligned bilinear sampling** of smaller maps, how libultrahdr's encoder
   averages a block into each sample. libultrahdr's decoder samples corner-aligned,
   which differs on a steep synthetic ramp by a median 0.08 stop with a quarter-size
   map. With a full-size map, the two agree to a median 0.004 stop.
6. **An HDR base's own samples win.** A JPEG XL that is PQ or HLG itself opens by them,
   as in the first increment, whatever map it carries.
7. **Provenance.** `HdrSource::gain_map` records that the light came from a map;
   `transfer` is `Linear` for these files.

## Decisions on revision 1's open questions

1. A typed `HdrLight` with validated construction, held by `CameraImage` and set
   only at decode, rather than an `Opened` variant alone. A variant wouldn't reach
   the reductions and working copies.
2. The SDR rendition is RAWmakase's tone map, documented as its policy, with no
   Camera Raw parity claimed.
3. Exposure, the listed geometry and the limit only, with every other difference
   refusing the master visibly.
