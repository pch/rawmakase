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
