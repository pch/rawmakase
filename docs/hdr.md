# HDR editing and rendering

Status: Current. What an edit with High Dynamic Range on renders, which parts of
the pipeline are measured against Camera Raw in HDR, and what is refused. The plan
and its progress are [HDR implementation plan](records/hdr-implementation-plan.md)
and [HDR results](records/hdr-stage1-results.md); the settings themselves are
`model::hdr` (Lightroom's `crs:HDREditMode`, `HDRMaxValue` and `SDR*`).

## Two renderings of one edit

An edit with HDR on has two renderings:

- **The HDR master** (`develop::render_hdr`): linear ProPhoto RGB, 1 = graphics
  white, nothing clipped above white, no channel above 2^HDR limit
  (`rendered::HdrImage`). Only the CPU renders it, without the stage cache. HDR
  outputs encode it; conversion to nits and to PQ or HLG belongs to them.
- **The SDR rendition**: the same edit with HDR off. Every SDR view and output
  renders it: Develop, Reference View, the Library's previews and thumbnails, SDR
  exports, and the GPU, which never sees HDR on (`Recipe::resolved` turns it off;
  `Recipe::resolved_hdr` keeps it for the master).

RAWmakase defines the SDR rendition as the HDR-off render, which below 0.5 agrees with
the HDR master as closely as Camera Raw's two renders do. Camera Raw's own is not
that: the SDR base of its gain-map JPEG is a local tone map of the HDR master, which
departs from its HDR-off render by a median 0.2 stop on a photo
([record](records/hdr-stage1-results.md#sdr-rendition)). Its SDR rendition sliders
(`SDRBrightness` and the others) adjust that tone map; RAWmakase doesn't render them,
so a master is refused while any of them is set.

Develop shows the HDR master on a display with headroom ([Display](#display)) and
the SDR rendition everywhere else, and says which under the Basic panel's HDR switch,
with the reason when the master can't render.

## Display

On macOS the window's surface is 16-bit float (`egui_wgpu::WgpuConfiguration`'s
`surface_format_selector`, from RAWmakase's egui fork until it is upstream), and its
Core Animation layer is tagged extended sRGB, so values above 1 are light above SDR
white. The interface keeps writing the same sRGB-encoded values it wrote before, so
it stays at SDR white. A display can show HDR when its potential headroom
(`maximumPotentialExtendedDynamicRangeColorComponentValue`) is above 1.05. The
window asks for EDR (`wantsExtendedDynamicRangeContent`) only while it shows an HDR
photo; until it does, macOS reports no current headroom.

While the display can show HDR, an edit with HDR on whose master renders is shown
as its master in Develop (`app::develop::hdr_preview`): in Fit, zoomed out, and
zoomed in to 100% or more, and in Before Only and on each side of Before/After and
Reference View, where each side shows its own settings' master or SDR rendition. A
whole photo is rendered on the CPU from a reduced copy of it at the size shown; a
100% region (the part of the photo the SDR rendition renders at 100%, with its
margin) from the full photo, with the margin sharpening reads around it, so it is
that part of the full-size master (`develop::render_hdr_region`). The master is
converted to linear sRGB primaries, with negative values kept, as extended sRGB
keeps them. Each channel is rolled off to the display's current headroom with the
HDR limit's roll-off and sRGB-encoded into a half-float texture.

Each place a master can show is a slot: the edit's whole photo and region, and the
other side's. One worker renders them, the latest request of each slot only, the
edit's region first; asking a slot for something else (a pan, an edit, a new
headroom) cancels its render under way, and a result for anything but the slot's
latest request is dropped. A preview is drawn only while it matches the photo, the
settings, the part of the photo and the headroom (to a tenth of a stop) it was
rendered for; until then the SDR rendition stays. Zoomed in, the whole photo's
master shows under the region only with the region's, so a region still rendering
shows the SDR rendition's sharp region rather than a blurred master. A preview of a
view the frame didn't draw (a region after zooming out, a comparison closed, a
reference photo removed) is freed and its render cancelled when the frame ends; only
the edit's whole photo stays, to show again in Fit. While a master is due, the window reads the headroom again every two seconds,
since macOS changes it with the display's brightness without waking the window.
Display changes are view changes, never edits.

Beside the edit, the Basic panel's note also says what the other side shows, named
(Before or Reference), so a comparison of brightness says when one side is SDR.
Everything else shows the SDR rendition, and the Basic panel's note says why: the
camera's thumbnail, visualizations drawn into the render (the clipping warning,
Remove's spots), edits with AI Denoise (its result is the full photo's, made on its
own worker), a monitor profile chosen in RAWmakase (it converts the SDR rendition
into device RGB, which has no extended range), the Library, other platforms, an SDR
display, and an 8-bit surface (when wgpu offers no float format). A failed render is
that edit's alone: its reason shows until the edit changes. The window asks for EDR
as long as a master is due, including while an edit's next preview renders, so the
display's headroom doesn't ramp down and up with every slider move. Slider drags
wait for the CPU master; the SDR rendition shows meanwhile.

## What the master renders

The pipeline is the SDR one up to the scene tone stage's output (S3), with these
HDR rules, each measured on Camera Raw 18.7's PQ output (synthetic gray ramps and the
HDR reference family):

| Stage | HDR rendering |
| --- | --- |
| S3 default | Identity from scene 0.5 up (`scene_tone::hdr_whites::IDENTITY_FROM`), with the default black point's toe below, whatever the sensor's white or the photo's maximum. SDR's shoulder to the white point is not applied. |
| Exposure | A gain before S3, as in SDR. |
| Whites (negative) | A measured gain over scene value (`scene_tone::hdr_whites`), the same at every sensor white, photo maximum and HDR limit. |
| Blacks, default black | As in SDR, on values below 1; values above white are left alone. |
| Profile look table | An HDR profile's 3D table on overrange-encoded values, as the DNG SDK applies it; an ordinary profile's keeps the values above white it scales. |
| Profile tone curve | `camera_profiles::hdr_tone`: an HDR profile's curve by the DNG SDK's overrange rule; an ordinary curve by the measured extension (ACR3's measured HDR curve, and a fitted deviation for other curves); a curve that never brightens as the identity. RGBTone. |
| Look table | A look's HSV table (Adobe Color's and the other Adobe Raw looks', RAWmakase Color's) after the profile's, on overrange-encoded values, as the DNG SDK applies an HDR profile's, unclipped. |
| Look curve | `camera_profiles::hdr_look`, after the profile's curve: the look's curve in an encoding that is sRGB's up to graphics white and logarithmic above. A curve that ends at white follows itself up to its largest lift, then runs parallel to the identity at that lift, at most 16 (so a curve that never lifts renders as the identity); one that ends below white continues along its slope there. RGBTone. |
| HDR limit | Each channel rolled off to 2^limit from 97% of it (`model::hdr::roll_off`), last. |
| Sharpening, resizing | On sRGB-curve-encoded values of the master's primaries, unclipped, with ProPhoto luminance; the limit applies after them. |

Lens corrections, geometry, noise reduction, AI Denoise and Heal and Clone work on
camera samples before tone and render as in SDR.

## What is refused

`model::hdr::check_recipe` refuses the master, not the edit, when the edit uses
something without a measured HDR rendering: Contrast, Highlights, Shadows, positive
Whites, Levels, point and parametric curves, Presence (Clarity, Texture, Dehaze,
Vibrance, Saturation), the colour mixer, Point Color, Color Grading, Calibration,
Black & White, Defringe, grain and vignette, masks, Red Eye, the SDR rendition's
sliders, looks other than a table and a curve at 100% (looks with an RGB table, such
as `.cube` looks and Lightroom's creative looks; looks with their own sliders or
settings, such as Adobe Vivid and Adobe Landscape; Black & White looks; fitted
Camera Looks; any Profile Amount but 100%), DCP tables encoded in sRGB, and
ordinary tone curves unlike the measured family. The edit stays valid and keeps HDR on, so nothing is downgraded:
its SDR rendition renders all of it, and the reason is shown where the master is
asked for.

## HDR files

An HDR file opens in Develop as an HDR edit ([the design](records/hdr-stage4-design.md)).
These files qualify:

- a TIFF, PNG, JPEG XL or AVIF whose CICP (in the file or its ICC profile) says PQ,
  HLG or linear light, at full range, in BT.709, BT.2020 or Display P3 (RAWmakase's
  own HDR exports among them);
- a TIFF of 32-bit float samples, through an ICC profile without CICP whose curves
  are the identity (Photoshop's 32-bit documents), or without a profile, read as
  linear sRGB primaries; 1.0 is graphics white. Integer samples with such a profile
  stop at graphics white, so they open as SDR files;
- a JPEG or JPEG XL with an ISO 21496-1 gain map over an SDR base, whose light the map
  reconstructs and whose base stays its SDR rendition ([Gain-map files](#gain-map-files)).

A PNG's own cICP chunk counts as its CICP and takes precedence over its ICC profile,
as the PNG specification says; the `png` crate reads it, checking its CRC. A
narrow-range cICP and an APNG are refused for Develop, HDR or not.

Its light (`camera_data::HdrLight`: linear ProPhoto, 1 = graphics white, signed) is
decoded exactly, in f32, by `HdrOutput::decode`, the inverse of the HDR export's
encoding. The image's own pixels are its SDR rendition: graphics white at 0.926 and
brighter light rolled off into white, as the Library shows it, which Develop's SDR
path renders through the rendered-file pseudo-scene. That SDR rendition is
RAWmakase's; Camera Raw's for these files is not measured.

A new HDR file starts with HDR on (`Recipe::for_new_rendered`), at the fewest whole
stops of limit whose roll-off starts at or above its peak
(`model::hdr::limit_containing`). A saved edit keeps its own settings; one that says
nothing of HDR, a Lightroom edit made before, reads HDR off, as it did when HDR
files didn't open. Its master is its light, with Exposure (a gain), crop,
straighten, Rotate and Flip, resized in linear light (negative light clamps to 0
there) and then limited. Anything else refuses the master by name
(`model::hdr::check_hdr_file`); the edit keeps it, and its SDR rendition renders
it.

Still refused, as Develop refused every HDR file before:
- HDR HEIC, which ImageIO and libheif draw their own way, and HEIC gain maps
  (Apple's own and ISO 21496-1 ones alike);
- HDR in other primaries or a curved profile.

The Library shows them with graphics white as white and says why.

### Gain-map files

A JPEG or JPEG XL with an ISO 21496-1 gain map opens as an HDR edit too
(`raster::gain_map`). That covers RAWmakase's own gain-map exports, libultrahdr's
Ultra HDR, and Adobe's and Apple's ISO form. The reader mirrors the exports
([Gain maps](#gain-maps)):

- **JPEG:** the base carries the APP2 version marker naming ISO 21496-1 and an MPF
  index. The index's first secondary image whose own APP2 segment holds the metadata
  is the map.
- **JPEG XL:** the container's `jhgm` box holds the metadata and the map's
  codestream.

**The light** is the authored HDR rendition, reconstructed at the map's full weight:
what a display with the alternate rendition's whole headroom shows. No display's
headroom is read, so the light is the same on every monitor and in every export.
The steps:

1. The base is decoded through its ICC profile to linear light in its own primaries,
   where 1 is its white, taken as graphics white.
2. Each channel becomes (base + base offset) × 2^gain − alternate offset. The gain is
   interpolated in stops between the map's minimum and maximum by the map's sample,
   raised to 1/gamma.
3. The result goes to linear ProPhoto through the profile's colorants.

A single-channel map applies one gain to all three channels, a three-channel map one
each. A map smaller than the base is interpolated bilinearly between its samples'
centres, the edges held, as a GPU samples a texture. This matches how libultrahdr's
encoder makes one: each sample is the average of its block. libultrahdr's own
decoder samples corner-aligned instead.

**The SDR rendition is the base,** decoded exactly as the same JPEG or JPEG XL
without its map: the Library shows it, and Develop's SDR rendition and SDR exports
render it. That makes it the authored SDR rendition at neutral settings, not
RAWmakase's tone map. A catalog edit made before gain maps were read keeps its
settings, HDR off included, and renders as it did. New files start as HDR edits,
with the same defaults, master and refusals as the other HDR files above;
`HdrSource::gain_map` records the provenance.

**What is read.** Everything is read with bounds: segment, MPF, box and metadata
lengths are checked against the file, and the map's size against its header.
Refused for Develop, by reason (the Library shows the base):

- **"damaged gain-map".** Any of these:
  - a short or truncated payload, or a zero denominator;
  - a maximum below its minimum, a gamma not above 0, or an offset outside 0–1;
  - gains or headrooms beyond 16 stops;
  - no MPF index, or one that points outside the file or at no map with metadata;
  - a map larger than its base, or of another shape beyond a map sample's
    rounding;
  - a map that doesn't decode, or decodes to a size other than its header's.
- **"version N gain-map":** a minimum version above 0, in the base's marker, the
  metadata or the JPEG XL bundle.
- **"HDR-base gain-map":** the base is the HDR rendition, by its flag or because its
  headroom is at or above the alternate's.
- **"alternate-colour gain-map":** the map applies in the alternate's colour space,
  which an ICC profile or colour encoding of its own describes.
- **"grey gain-map"** and **"table-profile gain-map":** a grey base, or a base whose
  profile converts through tables, which give the map no linear RGB to apply in.
- **"turned JPEG XL gain-map":** a JPEG XL whose base or map codestream is turned.
  Which frame the map is in is not settled for JPEG XL. A JPEG's map is in the base's
  stored frame and turns with its EXIF orientation.
- **"compressed gain-map":** a `jhgm` box compressed in `brob`.

Not read: gain maps described only by XMP (Adobe's and Android's earlier `hdrgm`
metadata, without the ISO marker), which open as their SDR base, as before. A JPEG
XL whose own samples are HDR opens by them as above, whatever map it carries.

**Checks.**

- RAWmakase's own gain-map exports of synthetic ramps reopen with the master's
  luminance: median 0.005 stop for both JPEG and lossless JPEG XL, worst 0.03 and
  0.015. That is the map's 8-bit steps; the single-channel maps carry luminance, and
  colour is the base's.
- libultrahdr 2.0.2's own Ultra HDR file (Display P3 base, three-channel map the
  base's size) opens with the light libultrahdr decodes at full headroom, to 0.019
  stop on the checked pixels: median 0.004 and p95 0.014 over the image.
- With a quarter-size map, libultrahdr's corner-aligned sampling differs from
  RAWmakase's by a median 0.08 stop on that steep ramp.

## HDR export

An export with **HDR** on (the Export dialog's HDR rows for TIFF and JPEG XL, `ExportSettings::hdr`,
`rawmakase render --hdr-export`) writes the edit's HDR master instead of its SDR
rendition. The encoding is captured with the job (`export::job::prepare`) and never
reads the display or the editor. Gamut and transfer are separate choices
(`color::hdr_output`):

| Transfer | File | Graphics white |
| --- | --- | --- |
| PQ (ST 2084) | 16-bit TIFF | 203 cd/m², absolute; signal 0.5806 |
| HLG (BT.2100) | 16-bit TIFF | 203 cd/m² on HLG's 1,000 cd/m² reference display: the inverse OOTF (system gamma 1.2 on the gamut's luminance), then the OETF; signal 0.75 |
| Linear | 32-bit float TIFF | 1.0, as Photoshop's 32-bit documents |

PQ and HLG are also written as 16-bit JPEG XL (`export::jxl`), lossy at the Export
dialog's quality or, with Lossless checked, lossless (`ExportSettings::jxl_lossy`,
`render --jxl-quality`). The header names the primaries (JPEG XL's enumeration: P3 is
11 there, 12 in CICP) and the transfer, with the tone mapping's intensity target
(10,000 cd/m² for PQ, 1,000 for HLG), and the file is a container with `Exif` and
`xml ` boxes. Settings saved before JPEG XL could be lossy read as lossless; new ones
start lossy.

- **Lossy:** `jixel` encodes a VarDCT frame in XYB at libjxl's distance for the quality
  (`cjxl`'s mapping: 90 is distance 1, the default 92 is 0.82), at its fast effort.
  XYB holds absolute display light and the header only says which signal to decode
  it to. jixel reads PQ input correctly but takes HLG input for normalised scene
  light, which decodes about two stops dark, so an HLG export is given to it as the PQ
  of its light on HLG's 1,000 cd/m² reference display, and its header, checked
  against the one RAWmakase writes for that frame, is replaced with one that says
  HLG. The 45 MP Z 8 photo is about 9.1 MB at quality 92; libjxl's decoder gives back
  its light to a median 0.006 stop above white (p95 0.03) for PQ and 0.002 stop for
  HLG ([record](records/hdr-stage1-results.md#codecs)).
- **Lossless:** `zune-jpegxl` encodes the frame and RAWmakase writes the image header in
  place of zune's sRGB one. Lossless files are large (193 MB for the same photo, as
  large as its 16-bit TIFF).

JPEG XL also exports SDR, in 8-bit sRGB only, lossy or lossless as HDR is.

jxl-oxide (RAWmakase's own JPEG XL reader too) converts a lossy file's XYB light to its
primaries after clipping it, in BT.709 primaries, to the intensity target. A wide-gamut
HLG export's saturated highlights near 1,000 cd/m² therefore read back clipped there,
and some of its pixels as NaN, as they do from libjxl's own lossy HLG files; libjxl's
decoder doesn't clip them.

Gamuts: Rec. 2020, Display P3 and Rec. 709, all D65 (Bradford from the master's D50).
Out-of-gamut channels clip at 0, and each channel is held to the HDR limit again in
the output gamut. The ICC profile (`color_management::hdr_profile`) is ICC v4 with
the gamut's colorants and a `cicp` tag naming primaries and transfer; EXIF's
ColorSpace is Uncalibrated. No mastering-display metadata is written.

An HDR export needs an edit with HDR on (otherwise it is refused, not exported as
SDR), refuses watermarks (they are composited in SDR) and is a TIFF or a JPEG XL
(linear light a TIFF only). An SDR export of an HDR edit is its SDR rendition. Gain
maps follow (#894).

### Gain maps

A JPEG export with HDR on writes a gain map (`export::gain_map`, ISO 21496-1) instead
of HDR pixels: the base is the edit's SDR rendition, exactly the SDR JPEG export's
pixels and metadata, and the gain map turns it into the HDR master on a display with
headroom. The map is one 8-bit channel the base's size, computed in the base's colour
space (sRGB primaries, linear) from luminance as log2 of (HDR + 1/64) / (SDR + 1/64),
between its minimum and maximum over the photo, gamma 1. The base's headroom is 0 and
the alternate's log2 of 2^limit. The file is laid out as libultrahdr writes it: the base
with an ISO 21496-1 version marker and an MPF index, then the map's grayscale JPEG with
the metadata in APP2. No XMP `hdrgm` metadata (Adobe's earlier gain-map format) is
written. A gain map's gamut is the base's, Rec. 709.

A JPEG XL export can carry the same gain map (the Export dialog's "As a gain map over
the SDR rendition", or `--hdr-export gain-map` to a `.jxl`): the base is the 8-bit
sRGB SDR JPEG XL, and a `jhgm` box (libjxl's gain-map bundle) holds version 0, the
ISO 21496-1 metadata, no alternate colour encoding or ICC profile (the map applies in
the base's colour space), and the map's grayscale codestream. Both are lossy, at the
same distance, as a gain-map JPEG's map has its quality, or both lossless. RAWmakase
writes the bundle itself around jixel's or zune's codestreams. For the Z 8 photo at
quality 92 the file is 10.3 MB (74 MB lossless), and the lossy map is within 2 of its
256 codes at p95 (0.03 stop). libjxl 0.12's `jxlinfo` reads the box as a version 0
bundle with its 61-byte metadata and the map's codestream, and `djxl` decodes the base
and the map; no local tool applies a JPEG XL gain map, so its reconstruction is checked
only through the JPEG form, which shares the map and metadata.

libultrahdr 2.0.2 (`ultrahdr_app`) reads the Z 8 reference photo's gain-map export
(2048 px) as Ultra HDR with this metadata, and its decode at full headroom matches
RAWmakase's own reconstruction to 0.002 stop and the master above white to 0.006 stop
median (p95 0.022). Deep shadows carry the 8-bit JPEG base's quantization (p95 0.4
stop below 0.05 of white).

Checked with an independent reader: the Z 8 reference photo's PQ Display P3 export,
decoded by `tifffile` and the ST 2084 EOTF, scores against Camera Raw's own PQ TIFF
exactly as the master does (median 0.061 stop above white), and the master and the
decoded export agree to 0.003. libjxl 0.12's `jxlinfo` reads the JPEG XL exports' header
as written (P3 or Rec. 2100 primaries, PQ or HLG, intensity target), and `djxl`
decodes the lossless PQ export to exactly the TIFF's 16-bit samples.
