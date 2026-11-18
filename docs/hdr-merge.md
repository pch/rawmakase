# Merge to HDR

Status: In progress. Merging a bracket of RAWs into one scene-referred linear
floating-point DNG, which opens as an ordinary raw in RAWmakase, Lightroom and
Camera Raw: white balance, profile, lens corrections and HDR stay editable. It
lives in `crates/rawmakase-hdr-merge` and arrives in stages, one pull request each
(the first, #1166). Done so far:

- Floating-point DNGs keep their precision through decoding (`SensorData::F32`,
  [RAW decoding](raw-decoding.md)).
- The DNG writer and its round trip (below).

Bracket validation, the merge itself, alignment, deghosting, the app's dialog and
job, and the `rawmakase merge-hdr` command follow.

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
