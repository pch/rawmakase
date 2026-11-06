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

Camera Raw's SDR rendition has its own sliders (`SDRBrightness` and the others); their
rendering is not measured, so a master is refused while any of them is set. Whether
Camera Raw's default SDR rendition equals its HDR-off render is not measured either
(its TIFF output carries no SDR rendition): RAWmakase defines it as the HDR-off
render, which below 0.5 agrees with the HDR master as closely as Camera Raw's two
renders do.

Until HDR displays are supported, Develop shows the SDR rendition and says so under
the Basic panel's HDR switch, with the reason when the master can't render.

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
sliders, looks (Lightroom's creative and Adobe looks, `.cube` looks and fitted
Camera Looks), DCP tables encoded in sRGB, and ordinary tone curves unlike the
measured family. The edit stays valid and keeps HDR on, so nothing is downgraded:
its SDR rendition renders all of it, and the reason is shown where the master is
asked for.

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

PQ and HLG are also written as lossless 16-bit JPEG XL (`export::jxl`): `zune-jpegxl`
encodes the frame and RAWmakase writes the image header in place of zune's sRGB one,
naming the primaries (JPEG XL's enumeration: P3 is 11 there, 12 in CICP) and the
transfer, with the tone mapping's intensity target (10,000 cd/m² for PQ, 1,000 for
HLG), in a container with `Exif` and `xml ` boxes. Lossless files are large (193 MB for
the 45 MP Z 8 photo, as large as its 16-bit TIFF); lossy JPEG XL waits on #906.
JPEG XL also exports SDR, losslessly in 8-bit sRGB only.

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
written. A gain map's gamut is the base's, Rec. 709; JPEG XL gain maps (`jhgm`) follow.

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
decodes the PQ export to exactly the TIFF's 16-bit samples.
