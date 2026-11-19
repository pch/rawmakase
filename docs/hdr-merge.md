# Merge to HDR

Status: In progress. Merging a bracket of RAWs into one scene-referred linear
floating-point DNG, which opens as an ordinary raw in RAWmakase, Lightroom and
Camera Raw: white balance, profile, lens corrections and HDR stay editable. It
lives in `crates/rawmakase-hdr-merge` and arrives in stages, one pull request each
(the first, #1166). Done so far:

- Floating-point DNGs keep their precision through decoding (`SensorData::F32`,
  [RAW decoding](raw-decoding.md)).
- The DNG writer and its round trip (below).
- The merge of a bracket and the `rawmakase merge-hdr` command.
- Auto Align: brackets taken by hand.
- Deghost: what moved between the frames.

The app's dialog and job follow.

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
   Small RAWs and linear DNGs are refused. Only two frames are held at a time;
   with Auto Align or Deghost also the reference, decoded first (until its
   turn, or with Deghost to the end), and a frame's warped copy while it is
   warped: up to four.
2. **Aligns** each frame to the reference (Auto Align, on by default; below),
   and warps it into the reference's geometry.
3. **Measures** each frame's exposure against the one before, on pixels both
   saw unclipped (the brighter below 85% of its saturation) and the darker 20
   standard deviations above black: the median of their ratios, refined by
   Tukey's biweight so a moving object can't pull it. Shutter speeds are rounded
   (a camera's 1/60 s is 1/64 s); the EXIF's ratio would leave a seam of that
   size. A ratio more than 0.5 stop from the EXIF's is refused as another scene;
   with fewer than 1000 such pixels, the EXIF's ratio is used.
4. **Deghosts** (Deghost Low, Medium or High; below): masks what moved
   between each frame and the reference, and takes it from the reference.
5. **Measures each frame's noise**, variance = shot · level + read, from its
   flat 16 × 16 tiles: the robust variance of differences two pixels apart,
   the flattest fifth of each half-stop level, a weighted line through them.
   A frame with too few levels takes a rough model by its ISO speed. It is
   measured before the warp, which smooths it.
6. **Merges**: per pixel and channel, each frame's estimate `s / g` (its
   sample over its exposure relative to the darkest) weighted by
   `g² / (shot · s + read)`, the inverse of its variance. Brighter frames win
   wherever they are unclipped. A ramp takes a frame's weight from 1 to 0
   between 85% and 98% of its saturation, and a clipped pixel weighs 0; the
   ramp and the mark take a pixel's three channels together, so all three come
   from the same frames and a ratio slightly off can't tint the passage. The
   darkest frame is never ramped or dropped: where every brighter frame
   clipped it alone says what the light was, as a single RAW would, and the
   decoder's highlight recovery works on it as on any raw. A frame's
   deghosting mask takes its share of the frame's weight away, the darkest
   frame's too.
7. **Encodes**: the darkest frame's saturation is WhiteLevel 32768, as in
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

## Auto Align

`align` fits each frame to the reference with a homography, by direct
photometric alignment (inverse-compositional Lucas–Kanade), with no feature
detector:

- **What is compared:** the log of each frame's demosaiced green. Green is
  sampled on every row and column of a Bayer sensor; where the demosaic puts a
  red or blue edge depends on its phase on the colour filter, which a frame
  moved by an odd number of pixels changes. The exposure between the frames
  is one offset in the log, fitted with the motion from the mean difference
  the coarse search finds.
  A pixel takes no part when it is clipped, above 85% of its saturation or
  less than 10 standard deviations of its noise above black.
- **Weights:** each pixel by the inverse of its log's variance from the
  frame's noise model, times Tukey's biweight of its residual over that
  noise, so a moving object doesn't pull the fit. A residual that half a
  pixel's misalignment explains at the pixel's gradient is not an outlier:
  otherwise smooth areas, which agree under any shift, set so tight a reach
  that the edges which carry the fit are thrown out while it is still a
  little off.
- **Coarse to fine:** a Gaussian pyramid ([1 3 3 1] blur, halved until the long
  edge is 200 pixels at most). On the coarsest level an exhaustive search
  for the shift, within 5% of the long edge, refined to a fraction of a
  pixel; then Gauss–Newton steps on every level down to full resolution, on
  at most 2^18 pixels a level, the steepest of each 16 × 16 cell, with
  Sobel gradients (noise in the gradients shortens every step).
- **Steps:** each is tried at full length, then doubled while the robust cost
  falls, or halved if it rose; one that no length makes cheaper is not taken,
  and the level is done. A level is done when a step moves no corner
  0.01 of its pixels or lowers the cost by less than 0.01%: where parallax
  leaves no one homography exact (a car before a landscape), the weights hand
  pixels from one depth to the other and the fit would creep along a valley
  no lower than where it is.
- **Models:** a shift, then a similarity (shift, turn, scale), then the full
  homography, each through the pyramid from where the simpler one ended (a
  richer one that can't be fitted is passed over), and the identity with only
  its offset fitted. The one kept has the least
  robust cost at full resolution plus 25 per parameter (four times the
  Bayesian information criterion's, as neighbouring pixels' noise is not
  independent), so a richer model must explain more than noise.
- **Failure:** a frame that moved more than 5% of the long edge at its centre,
  turned more than 5°, moved any corner more than 10%, whose fit still moved
  half a pixel in its last step, that most pixels disagree with (fewer than
  half within 14 standard deviations of their noise, or half a pixel's
  misalignment at an edge), or that the
  best fit leaves more than half a stop from the reference (robust standard
  deviation) fails the merge: "Couldn't align IMG_1.CR3: … Try with Auto
  Align off, or a bracket taken on a tripod". A smeared merge is never
  written.
- **Warp:** a fit that moves no corner 0.05 pixel is the identity, and the frame
  is merged as taken. Otherwise every reference pixel takes the Catmull–Rom
  cubic of the 4 × 4 frame pixels around where it lands, clipped if one of
  them is. Lens distortion is the same in every frame, so it stays in the
  data and is corrected at render time as for any raw.
- **Coverage:** where a warped frame doesn't reach adds nothing from it (the
  darkest frame included), and what not every frame covers is the DNG's
  transparency mask, as in Lightroom's merges. The starting crop that hides it
  comes with the app's merge.

Each frame's alignment (its shift at the centre, turn and remaining residual)
is in `Merged::frames`, and the command line prints it.

## Deghost

Something that moved between the frames (a person walking, branches in the
wind, waves, a near object's parallax Auto Align's one homography can't fit)
merges as a half-transparent ghost of every frame's place. `ghost` leaves each
frame out where it shows something else than the reference, so it appears
once, as the reference saw it. None (the default, as in Lightroom), Low,
Medium and High differ only in how readily a difference counts:

- **What is compared:** each frame's luminance (the sum of its three channels)
  against the reference's, the frame's brought to the reference's exposure by
  their ratio, measured between the two directly (`ratio`). A pixel differs
  when they are more than 6, 4 or 3 (Low, Medium, High) standard deviations
  apart, the deviation from both frames' noise, 2% of the reference's level
  for the ratio's error, and 0.3 pixel's misalignment at the reference's
  gradient. Where one of them is clipped (or past 98% of its saturation), its
  luminance is only a lower bound and only the other falling short of it
  differs: a frame that saw a person where the reference saw a clipped sky
  is left out there too.
- **The mask:** the differing pixels are opened (a square of 1/2000 of the long
  edge, so noise's lone outliers leave nothing), dilated by 0.5% of the long
  edge and feathered by a blur of 0.5% (three box blurs), doubled: 1 over the
  dilated mask, falling to 0 over about two standard deviations outside it.
  The frame's weight is multiplied by 1 minus the mask. Frames' weights
  differ by the square of their exposure ratio, so a frame much brighter than
  the rest takes back most of the merge within the feather's first pixels:
  the dilation's margin, not the feather, is what keeps its ghost out. A
  mover narrower than the opening (a twig, a cable) is not masked.
- **Where nothing is left:** where the reference clipped and the frames that
  saw it unclipped were all left out, each connected region takes the
  brightest of them that saw all of it unclipped, or the darkest: all of it
  from one frame, as a single RAW would, so no seam runs through it. Only
  what that can need is kept of the frames left out, the pixels where the
  reference has no weight (none when it is the darkest frame), so it costs
  memory in proportion to what moved where the reference clipped.

Each frame's deghosted share is in `Merged::frames`, and the command line
prints it.

## The command line

```sh
rawmakase merge-hdr IMG_9814.CR3 IMG_9815.CR3 IMG_9816.CR3 [-o out.dng] [--no-align] [--deghost none|low|medium|high]
```

The first RAW names the merge. Progress and what was measured (each frame's
exposure by EXIF and measured, the reference, the BaselineExposure) go to
standard error, the DNG's path to standard output. `--no-align` merges the
frames as they were taken; `--deghost` sets the deghost level (none by
default). `--output` must not exist.
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
  whose measured steps came out smaller than its EXIF's ISO speeds say; Auto
  Align showed why: its frames are up to 30 pixels apart, below).
  A 24-megapixel bracket of three takes about 27 s and 1.9 GB on an Apple M1 Pro.

### Auto Align (2026-11-19)

- **Synthetic frames** (`align.rs`'s tests, 1536 × 1024, a texture of waves and
  soft edges rendered exactly under each motion, two stops apart, with noise
  and the EXIF's exposure a tenth of a stop off): shifts of 60 pixels, turns of
  2°, a perspective and sub-pixel shifts are found within 0.1 pixel everywhere
  in the frame; a frame of the same place is the identity; an eighth of the
  frame moved elsewhere doesn't pull the fit past 0.1 pixel; another scene and
  a frame two thirds covered by one (most pixels disagree), a turn of 8° and a
  frame with nothing well exposed fail. On the four brackets below 91% to 100%
  of the pixels agree with the fit.
- **A synthetic handheld bracket** (`tests/merge.rs`, the bracket above with
  the darkest and brightest frames moved 7.8 and 6.0 pixels and turned 0.5°
  and −0.3°, through mosaic, demosaic and merge): the shifts are found within
  0.2 pixel and the turns within 0.05°, every block of the merge is within its
  noise of the scene, and the moved edges are the transparency mask. Unaligned,
  the exposure ratios are measured across misplaced edges and every block comes
  out about 17% dark.
- **The `play-raw` brackets**, merged with and without Auto Align and rendered
  beside the reference frame alone (CPU time and peak memory on an Apple M1
  Pro under load from other work):

  | Bracket | Frames moved | Residual | CPU | Memory |
  | --- | --- | --- | --- | --- |
  | Fujifilm X-T4 (X-Trans) | 6.6, 4.5 px | 0.04 stop | 32 s (23 s unaligned) | 3.1 GB (2.4) |
  | Panasonic GX80 | 1.7, 3.5 px | 0.08 stop | 13 s (7 s) | 1.6 GB (1.3) |
  | Nikon D750, 5 frames | 14 to 25 px | 0.06 to 0.17 stop | 32 s (15 s) | 2.7 GB (2.0) |
  | Panasonic GH3, 5 frames | 15 to 31 px | 0.10 to 0.12 stop | 18 s (10 s) | 1.8 GB (1.3) |

  Aligned, the X-T4's foliage, the D750's car and the GH3's canyon are as sharp
  as the reference frame alone; unaligned, doubled or smeared. The GH3's
  measured steps now agree with its EXIF (+0.33, +0.66, +0.99, +1.31 stops
  against +0.32, +0.64, +1.00, +1.32; unaligned they measured +0.25 to
  +0.93). The GX80's thin branches still show double edges where the wind moved
  them: what moved within the scene is deghosting's, not alignment's. The
  D750's car stands before a far landscape, which no one homography fits; the
  car comes out sharp, and runs with different stopping rules put the fit
  about a pixel apart.
- **Not checked:** the Canon T8i tripod bracket (refused until its camera
  name is fixed), so whether a real tripod bracket comes out as the identity;
  a bracket of more than 24 megapixels.

### Deghost (2026-11-19)

- **Synthetic frames** (`ghost.rs`'s tests, 600 × 400, shot and read noise):
  frames of one scene, two stops either side of the reference, leave no mask
  at any level; a block in one frame is masked whole with its feather and
  nothing beyond; where the reference clipped, a frame darker than the clip
  there is masked and one that agrees is not; an empty region is filled from
  one frame, the brightest that saw all of it unclipped.
- **A synthetic bracket** (`tests/merge.rs`, through mosaic, demosaic and
  merge, on a tripod): blocks in the darkest and brightest frames that the
  reference doesn't show leave ghosts in eight blocks without deghosting; at
  every level each block follows the scene as the reference saw it, within
  the reference's noise, and a block the darkest frame alone saw where the
  reference clips comes from it, within 5%, not as a hole. The bracket
  without movers deghosts nothing at any level.
- **The `play-raw` brackets**, aligned, the share of each frame deghosted
  (Low / Medium / High) and the cost of Medium over Auto Align alone (CPU
  time and peak memory on an Apple M1 Pro under load from other work):

  | Bracket | Deghosted | CPU | Memory |
  | --- | --- | --- | --- |
  | Panasonic GX80 | 11–15% / 18–21% / 28% | +3 s | +0.3 GB |
  | Fujifilm X-T4 | 0.3–0.5% / 0.5–0.8% / 0.8–1.1% | +5 s | +0.4 GB |
  | Nikon D750, 5 frames | 7–24% / 13–31% / 19–35% | +10 s | +0.3 GB |
  | Panasonic GH3, 5 frames | 0% / 0–0.2% / 0–0.4% | +4 s | +0.3 GB |

  On the GX80 the masks cover the walking person and the birch's branches in
  the wind (and a few patches of grass); with Medium both come out single, as
  in the reference, where without deghosting they are doubled. On the D750
  they cover the waves and the near car and grass, whose parallax no one
  homography fits: the car comes out as sharp as the reference, the sun's
  glitter on the water without most of the patches of darker frames' waves
  (a few small ones remain where only darker frames saw a sparkle unclipped).
  The static GH3 canyon is left nearly whole at every level.
- **Not checked:** a real bracket where a mover crosses an area the reference
  clipped (the synthetic test covers it); the dialog's preview of the masks.

