# Negative conversion

Status: Current. Turning a scanned film negative into a positive in Develop, before
everything else renders. The settings are `model::negative`'s; rendering, the film
base picker and Auto are `develop::negative`'s; the panel is
`app::develop::panels::inspector::panels::negative`.

## What it does

A camera scan of a negative (a RAW of the film on a light source), a film scanner's
DNG or TIFF, or any other scan RAWmakase develops is a negative: inverted, and for
colour film behind an orange mask. The Negative panel, first in Develop, turns it
into a positive at the very start of the per-pixel stage, before white balance.
Everything after it (white balance, Basic, curves, the color mixer, grading, masks,
export) works on the positive, as for any photo, and so does everything that
measures the photo: Auto tone, Auto white balance, the white balance selector, the
scene tone stage's measures and white point, Shadows and Highlights, Contrast's
pivot, Dehaze, Point Color, the Targeted Adjustment Tool and the B&W panel's Auto
mix.

The original file is never changed: the conversion is part of the photo's edit.

## Using it

1. Open a frame and set **Film** to **Color** or **B&W**.
2. Click the **film base picker** (the eyedropper on the Film base row), then click
   the clear film edge beside the frame: the unexposed film, orange mask and all. The
   picker reads the photo as it is shown, so pick the base before cropping the edge
   away. It reads the scan's own values in a 9 × 9 pixel patch; a patch with a
   clipped pixel (the light around the film) or no light is refused, and the panel and
   the status line say why.
3. Crop to the image, leaving out the film holder and the clear edge, then press
   **Auto** on the Levels row: it levels the frame inside the crop.
4. Finish with Basic (Exposure, Contrast, Whites, Blacks) and white balance, as for
   any photo.
5. Sync the conversion to the rest of the roll: Copy Settings, Paste and Sync carry
   it as its own group, **Negative Conversion**. With Auto Sync on, the panel's Auto
   is measured on each selected frame, through that frame's own conversion: a roll's
   frames share their film and base, not their levels.

Each action (the film type, a picked base, Auto, Reset) is one History step, named
"Negative".

## How it renders

On each decoded camera value `v` (for a TIFF or JPEG, the file's own linear value):

```text
d   = log10(base / v)                    density above the film base
d   = mean(d) in every channel           B&W only
R   = mean(dmax - dmin) / 0.6            the scene's range, in decades
out = 10 ^ ((d - dmax) / (dmax - dmin) · R)
```

The frame's densest content (`dmax`) renders at 1.0 before white balance and its
thinnest (`dmin`) at 10<sup>−R</sup>. Reading the range back through a film gamma of
0.6 means a thin, underexposed negative gives a short scene range instead of being
stretched to a fixed one. A colour negative's channels each have their own bounds,
which removes the mask; B&W film has one density, so a toned base or the light's
colour leaves no cast. A gain on the scan before the conversion (the light, the
camera's exposure) cancels, since the base is picked in the same values.

The scene tone stage's default white follows the sensor's clip for a photo; for a
converted negative it follows the conversion's white (1.0), since the sensor's clip
is the film base, the scene's darkest end.

**Auto** samples a reduced copy of the scan inside the crop, leaving out 3% at each
side, and any clipped sample. It sets `dmin` and `dmax` from the 0.5th and 99.5th
percentiles of the densities. A colour negative's white is taken from its densest
near-neutral tones where it has them, so a coloured highlight doesn't read as a cast.
Apart from the 3% inset, Auto measures everything inside the crop: a sliver of the
film holder left in it recorded no light, reads as the densest film there is, and
makes the levels too dark. Auto can't tell it reliably from a scene's own dense areas
(a sky along an edge, or dense film over a dark base), so it never leaves anything
out. When it sees an unlit strip along the crop's edge (samples at most 2<sup>−8</sup>
of the sensor's white, where camera scans record 0.0005 to 0.002 of it inside a
holder, running along at least 40% of an edge), the panel says so under Levels: crop
the holder out and press Auto again. The note goes when the crop changes, when Auto
runs again without finding one, and with the conversion or the photo; it is not part
of the edit. On a dark scan it can show for dense film where there is no holder; the
levels are the same either way. Auto refuses a blank or flat frame.

## Where it applies

- Camera scans (RAWs), scanner DNGs (one-channel linear DNGs among them, which open
  as monochrome) and rendered files (TIFF, JPEG and the other formats Develop opens).
  A rendered file's decoded values are its pseudo-scene (`develop::rendered`), so the
  conversion reads the file's own linear values from it; such a photo renders its
  conversion on the CPU. RAWs render it on the GPU too.
- Native JSON presets carry it with the rest of the recipe. XMP doesn't: Lightroom has
  no such setting, so exports' XMP, XMP presets and New Develop Preset leave it out,
  and applying an XMP preset keeps the photo's conversion as it was.
- Between a RAW and a rendered file, Paste and Sync keep the target's conversion: the
  base is in decoded values, which the two don't share.

## Limits

- **No crosstalk correction.** The dye layers' crosstalk isn't undone, so a colour
  negative's saturated colours can come out less saturated or shifted. A correction
  measured per stock, and roll analysis, are later steps.
- **The film gamma (0.6) is provisional**, chosen to look natural on real camera
  scans, not fitted per stock.
- **What runs before the conversion sees the negative:** highlight recovery, noise
  reduction and AI Denoise, Texture, spot removal, lens corrections. They work on the
  scan as it is, which suits them.
- **What still reads the negative:** red eye detection measures the camera values,
  so it measures the negative; Upright, Auto
  Straighten and the brush's Auto Mask find edges in the camera values, which a
  negative keeps where they are; AI Remove sends the model the negative around the
  area, and turns its answer back the same way.
- **An HDR file's HDR master** (an HDR TIFF, AVIF or gain-map JPEG with HDR on)
  doesn't render a conversion: it applies Exposure to the file's light before
  anything reads it, so a conversion after it would run Exposure backwards. The
  master is refused by name, as other settings it doesn't render are, and the photo
  shows its SDR rendition, which converts; the Negative panel says so.

## Older releases

The conversion is saved in the edit only when it is on, and needs no new saved
format version. A release that predates it keeps it as a setting it doesn't know and
writes it back unchanged, but renders the scan as the negative it is. Its Copy
Settings choice is saved apart in `session.json` (`copy_negative`), so an older
release reading the session loses at most that checkbox.
