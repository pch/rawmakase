# Merge to HDR

Status: In progress. Merging a bracket of RAWs into one scene-referred linear
floating-point DNG, which opens as an ordinary raw in RAWmakase, Lightroom and
Camera Raw: white balance, profile, lens corrections and HDR stay editable. It
lives in `crates/rawmakase-hdr-merge` and arrives in stages, one pull request each
(the first, #1166). Done so far:

- Floating-point DNGs keep their precision through decoding (`SensorData::F32`,
  [RAW decoding](raw-decoding.md)).
- The DNG writer and its round trip (below).
- The merge of a tripod bracket, without alignment or deghosting, and the
  `rawmakase merge-hdr` command.

Alignment, deghosting, and the app's dialog and job follow.

## The bracket

`rawmakase_hdr_merge::bracket::check` takes 2 to 9 RAWs from one camera (make,
model and, when both record it, body serial number), of one size, sensor kind
and orientation. They must have been taken one after another, with no more than
2 s between one frame's end and the next one's capture, plus a second for EXIF's
rounding. Photos that don't record a capture time are not held to that. Their
exposures, log2(t · ISO / N²), must differ by at least 0.3 stop and leave no gap
over 4 stops. An f-number or ISO speed counts only when every frame records it.
A refusal names the photo and the reason ("IMG_2.CR3 is from another camera").

Frames merge from the darkest to the brightest. The reference, whose
brightness, white balance and crop the DNG takes, is the frame nearest the
median exposure; of two as near, the darker.

## The merge

`rawmakase_hdr_merge::merge` captures each source's identity, then:

1. **Decodes** each frame with `Raw::develop_frame`: the mosaic demosaiced as
   development demosaics it, then its white balance divided out again. Nothing
   is recovered. Each colour's saturation is the highest level its samples pile
   up on, in bins of 1/4000 of white: a sensor that clips stacks a highlight's
   samples on one level, where noise spreads an unclipped area's, and a lone hot
   pixel above makes no pile. It is never above the white level. A pixel is
   marked clipped when a sample that can change it reaches saturation: 3 pixels
   around on Bayer, 9 on X-Trans (beyond that, only X-Trans's pass over fine
   stripes averages colour along rows and columns).
   Small RAWs and linear DNGs are refused. Only two frames are held at a time.
2. **Measures** each frame's exposure against the one before, on pixels both
   saw unclipped (the brighter below 85% of its saturation) and the darker 20
   standard deviations above black: the median of their ratios, refined by
   Tukey's biweight so a moving object can't pull it. Shutter speeds are rounded
   (a camera's 1/60 s is 1/64 s); the EXIF's ratio would leave a seam of that
   size. A ratio more than 0.5 stop from the EXIF's is refused as another scene;
   with fewer than 1000 such pixels, the EXIF's ratio is used.
3. **Measures each frame's noise**, variance = shot · level + read, from its
   flat 16 × 16 tiles: the robust variance of differences two pixels apart,
   the flattest fifth of each half-stop level, a weighted line through them.
   A frame with too few levels takes a rough model by its ISO speed.
4. **Merges**: per pixel and channel, each frame's estimate `s / g` (its
   sample over its exposure relative to the darkest) weighted by
   `g² / (shot · s + read)`, the inverse of its variance. Brighter frames win
   wherever they are unclipped. A ramp takes a frame's weight from 1 to 0
   between 85% and 98% of its saturation, and a clipped pixel weighs 0; the
   ramp and the mark take a pixel's three channels together, so all three come
   from the same frames and a ratio slightly off can't tint the passage. The
   darkest frame is never ramped or dropped: where every brighter frame
   clipped it alone says what the light was, as a single RAW would, and the
   decoder's highlight recovery works on it as on any raw.
5. **Encodes**: the darkest frame's saturation is WhiteLevel 32768, as in
   Lightroom's merges. BaselineExposure is the reference camera's own (Camera
   Raw's, `camera_profiles::reference`) plus log2 of the reference's exposure
   over the darkest's and of the darkest's saturation, so the merge renders at
   the reference's brightness. If that passes ±5 the reference moves toward
   the middle until it doesn't, and the merge says so.

Lightroom's BaselineExposure 1.54 for the Canon T8i bracket of
discuss.pixls.us #39443 (1/20, 1/5 and 0.8 s, IMG_9814 active) is this rule:
the 850D's baseline of −0.46 plus the two stops from 1/20 to the 1/5 reference.

The DNG takes the reference's camera names, D65 colour matrix, unit calibration
(Adobe's diagonal CameraCalibration, signed "com.adobe", as Camera Raw applies
it), as-shot white, orientation and default crop; and the darkest frame's EXIF
and capture time, as Lightroom does. Not yet carried over: the maker note
(each container keeps it its own way), an embedded preview and starting XMP.

`write_dng` stages the file in the destination folder, syncs it, checks that
every source still has the identity the merge captured, and renames it into
place without replacing a file: beside the active photo as `<name>-HDR.dng`,
then `-HDR-2`, `-HDR-3`… as Lightroom names them, or at an exact path. A cancel
or a changed source publishes nothing, and the staged file is removed.

## The command line

```sh
rawmakase merge-hdr IMG_9814.CR3 IMG_9815.CR3 IMG_9816.CR3 [-o out.dng]
```

The first RAW names the merge. Progress and what was measured (each frame's
exposure by EXIF and measured, the reference, the BaselineExposure) go to
standard error, the DNG's path to standard output. `--output` must not exist.
Ctrl-C stops the merge without writing anything; a second Ctrl-C ends the
process at once.

## The merged DNG

`rawmakase_hdr_merge::encode` writes a `LinearImage` of camera RGB and the
`DngFacts` beside it:

| What | How |
| --- | --- |
| Image | Linear raw (PhotometricInterpretation 34892), three interleaved samples a pixel, IEEE half precision, rounded to nearest, in IFD0 |
| Tiles | 256 × 256, Deflate, Adobe's floating-point predictor 34894 (FloatingPointX2: each tile row's samples as byte planes, most significant first, each byte the difference from the same byte two pixels before) |
| Version | DNGVersion and DNGBackwardVersion 1.4.0.0, which defined floating-point samples, the predictor and transparency masks |
| Levels | Black-subtracted (BlackLevel 0), not white balanced, under one WhiteLevel (at most 65504, the largest half) |
| Colour | One or two calibrations: CalibrationIlluminant, ColorMatrix, ForwardMatrix, CameraCalibration; AnalogBalance; AsShotNeutral |
| Exposure | BaselineExposure, the camera's own included |
| Frame | Orientation; DefaultCropOrigin and DefaultCropSize (none: the whole image) |
| Coverage | A transparency mask (NewSubFileType 4, PhotometricInterpretation 4, 8-bit Deflate tiles) in IFD0's SubIFDs, where aligned frames leave the image uncovered |
| Camera | Make, Model and UniqueCameraModel as the camera's own files write them, so Adobe's camera profiles and lens profiles fit |
| EXIF | The camera's EXIF and GPS directories as `interop::exif` reads them, and ImageDescription, Artist and Copyright from its main directory; the maker note is left out of EXIF |
| Maker note | Adobe's DNGPrivateData record `MakN`: the note's byte order, its offset in its original file, the note, as Adobe DNG Converter keeps it, so the built-in lens tables and camera facts read from it survive |

Where it departs from Lightroom's own merges (Lightroom Classic 15.6): those are DNG
1.7 with JPEG XL tiles and the Adobe Standard profile embedded, without
DNGPrivateData. RAWmakase writes lossless Deflate (no float JPEG XL encoder is in the
tree), DNG 1.4, keeps the maker note and embeds no profile: RAWmakase and Camera Raw
find the camera's profiles by its name.

## Validation

- **Round trip** (`crates/rawmakase-hdr-merge/tests/round_trip.rs`): a 600 × 300
  synthetic merge, a 24-stop ramp in three tiles across and two down, opens through
  `rawmakase_decode::photo::open` with its samples to half precision, levels, crop,
  orientation, BaselineExposure, colour matrix, as-shot white, EXIF, maker note
  (through a Fujifilm DynamicRange tag) and transparency mask.
- **Adobe's DNG SDK** (`dng_validate` 1.4, built from the SDK's source): the same
  file validates with no warning. The SDK's public 1.4 source does not flatten
  transparency, so a file with a mask reads it and then stops at "not yet
  implemented". Android's copy of the SDK sizes 16- and 24-bit floating-point tiles
  as 32-bit and refuses every such predicted DNG, HDRMerge's among them, which
  Camera Raw opens; built from it, that one size must be corrected to validate.
- **Camera Raw 18.7** opens the same file, with and without the mask, and renders it
  as RAWmakase does: its crop and orientation, luminance to a median 0.0001 stop (95th
  percentile 0.04) over the rendered range, chromaticity within 0.004, the uncovered
  corner white. Only the deep toe, below about 0.2% of white, differs, as the two
  engines' black points do on any raw (2026-11-18, Adobe Standard, the base settings
  of `scripts/corpus/camera_raw.py`).

### The merge (2026-11-18)

- **Synthetic brackets** (`crates/rawmakase-hdr-merge/tests/merge.rs`): a radiance
  map of flat blocks and gradients over 15 stops, exposed at 1/256, 1/64 and
  1/16 s with shot and read noise, clipped and quantised to 14 bits, written as
  RGGB DNGs whose EXIF says 1/250, 1/60 and 1/15. The ratios measure 2 stops
  each to within 0.01 (the EXIF would say 2.06 and 2). The merge read back
  through `photo::open` follows the scene within 0.01 stop over 8-column windows
  of the gradients and 0.002 on average, with no step at the clip ramps; with
  the EXIF's ratios the same test finds a 0.06-stop seam. Every block is within
  its noise, the shadows are as noisy as the brightest frame alone, another
  scene is refused, a cancel at any stage leaves no file, and a taken name is
  never replaced.
- **Lightroom's merge of the Canon T8i bracket** (#39443, IMG_9814 active),
  against RAWmakase's, both decoded by RAWmakase and compared with the darkest
  frame alone, in 16-pixel blocks:
  - Where the darkest frame alone carries the light, both match it to 0.001
    stop: the same WhiteLevel 32768 at the darkest frame's saturation.
  - Where the 1/5 s frame carries it, RAWmakase's is within −0.012 to +0.005
    stop of the darkest frame, Lightroom's +0.011 to +0.028. RAWmakase measured
    the bracket's steps at 2.04 and 2.01 stops; Lightroom's merge and its
    BaselineExposure (1.54 = −0.46 + 2) fit the EXIF's 2 and 2. RAWmakase's
    BaselineExposure is 1.59 for the same reason.
  - The two are equally noisy in the shadows.
  - Rendered in Camera Raw 18.7 (Adobe Standard, the base settings of
    `scripts/corpus/camera_raw.py`, 2048 px), RAWmakase's merge is a median
    0.006 stop from Lightroom's (5th to 95th percentile −0.10 to +0.06),
    chromaticity a median 0.007 apart. As Shot renders 4750 K against
    Lightroom's 4650 K from the same AsShotNeutral: Lightroom writes both
    illuminants' matrices, RAWmakase only D65's.
  - Adobe's `dng_validate` 1.4 reads it with no warning but the zero entries
    of the camera's own LensSpecification, copied from its EXIF.
  - RAWmakase names the T8i the EOS 850D, as its colour-matrix and camera
    tables and Adobe's profiles do, so the merge's UniqueCameraModel is
    "Canon EOS 850D". Before it did, the camera had no colour matrix and the
    merge refused the bracket.
- **The other `play-raw` brackets** merge: Fujifilm X-T4 (X-Trans, handheld,
  not aligned yet), Nikon D750 (5 frames), Panasonic GX80 and GH3 (ISO only,
  whose measured steps are smaller than its EXIF's ISO speeds say).
  A 24-megapixel bracket of three takes about 27 s and 1.9 GB on an Apple M1 Pro.

