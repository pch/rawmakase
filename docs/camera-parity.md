# Checking camera and tone parity

The checks cover camera white-balance calibration (#353) and provide diagnostics
for the tone controls ([tone controls](tone-controls.md),
[scene tone stage](scene-tone-stage.md)). Adobe DNG Converter
supplies white-balance calibration facts. Photoshop with Camera Raw supplies
rendered references for tone controls; DNG Converter alone cannot do that.
Neither is a runtime dependency of RAWmakase.

## White-balance calibration

Install ExifTool and Adobe DNG Converter, then audit RAW samples:

```sh
python3 scripts/cameras/check-calibration.py --converter /path/to/converter \
  --report /tmp/calibration.json sample.ARW another.NEF
```

The script converts temporary copies, reads the calibration DNG Converter
records and the white-balance presets stored in the original RAW, and compares
exact camera models and aliases against `data/cameras.toml`. Already converted
DNGs can be audited without `--converter`, but then the native presets are
missing and a per-unit reference cannot be checked or proposed. A mismatch,
missing row, conflict or unsupported calibration returns a nonzero exit status.
The report proposes `unit_calibration` rows; it does not edit the table.

### Per-unit calibration

Adobe calibrates each camera unit, not each model. DNG Converter records a
diagonal CameraCalibration, equal for both illuminants, that is a model reference
divided by a white-balance preset the unit stored in its own RAW, both normalized
to green and rounded to four decimals. The camera table holds the reference and
which preset it divides:

```toml
unit_calibration = { preset = "daylight", reference = [1893, 1024, 1697] }
```

| Make | Preset | RAW tag (ExifTool) | LibRaw |
|---|---|---|---|
| Sony | `daylight` | `WB_RGBLevelsDaylight` | `WB_Coeffs[Daylight]` |
| Canon (most bodies) | `daylight` | `WB_RGGBLevelsDaylight` | `WB_Coeffs[Daylight]` |
| Canon EOS 200D, 5D Mark IV, 5DS R, 6D Mark II | `shade` | `WB_RGGBLevelsShade` | `WB_Coeffs[Shade]` |
| Fujifilm | `d65` | `WB_GRBLevelsStandard`, the entry for light source 21 | `WB_Coeffs[D65]` |
| Olympus, OM System | `fine-weather` | `WB_RBLevels5300K` | `WB_Coeffs[FineWeather]` |

The native adapter exports these presets for every native RAW, not a
matrix-derived `pre_mul` (LibRaw's); `cameras::neutral_calibration` applies the row. A
missing or invalid preset, or a camera without a row, keeps identity rather than
borrowing another unit's correction, and there is no make-wide fallback.

Evidence (Adobe DNG Converter 18.7, 2026-10-14 to 2026-10-24): 689 samples, 465 public
raw.pixls.us files and 224 anonymous local ones, including bodies of the same
model identified by serial number. For 61 models, bodies with different
calibrations reproduce Adobe's four-decimal values exactly from one integer
reference: 24 Sony and 31 Canon models with the Daylight preset, 3 Canon models
with Shade, 2 Fujifilm models with D65 and the Olympus E-M5 Mark III with its
5300 K preset. Different ISO settings and recording
modes of one unit (RAW and C-RAW, compressed and uncompressed, sRAW and mRAW)
record one calibration. The EOS R8 Mark II stores a Daylight preset that
depends on the lens, and Adobe's calibration follows it: six local files of one
unit (2026-10-24), with two lenses, record two calibrations that its one
reference reproduces. Separately measured models that share a sensor or
processor generation share their reference (Fujifilm X-T3, X-T30, X-T4, X-S10,
X-E4, X-Pro3 and X100V; Canon 250D and 850D, R6 Mark II, R8 and R8 Mark II,
R10 and R50, 100D, 650D and 700D, 600D and 1200D, 1300D and 3000D, 6D and 70D;
Sony ILCE-5000 and SLT-A37), which further supports single-sample rows. A body
sold under several names (the 1100D as the Rebel T3, the 3000D as the 4000D and
Rebel T100) has one reference under each name. The A7CR, A7 II and X100F rows have
been checked against Lightroom or Camera Raw on the owner's bodies. This agrees
with the [RawSpeed calibration investigation](https://github.com/darktable-org/rawspeed/compare/develop...tdonovan4:rawspeed:camera-calibration-matrix)
for Sony.

Rows are installed when the samples identify one preset. Unresolved, so identity:

- Canon EOS-1D X Mark III: its one sample fits both Daylight and Shade.
- Canon EOS R6 Mark III, R50 V and PowerShot V1: no stored preset reproduces the
  sample; the R6 Mark III Daylight preset even varies between files of one body.
- Canon EOS M3: units differ in a way no preset explains. Older PowerShots,
  FinePix bodies without the tag, Olympus C and SP compacts, whose older maker
  notes the audit does not read, and the Olympus E-P1, E-P2, E-P3, E-PL1,
  E-PL3 and XZ-10 samples are likewise unexplained, but have no table rows.
- Olympus TG-4 to TG-6, OM System TG-7, Stylus 1 and SH-2: DNG Converter records
  different calibrations for the two illuminants, which RAWmakase does not apply.
- Nikon, Panasonic, Pentax, the Fujifilm GFX 50R, X-E5 and X-T50, and the Olympus
  PEN-F, E-P7, E-M10 Mark IV and OM System OM-1 Mark II get identity calibration
  from Adobe, which RAWmakase matches.

Olympus and OM System units divide by their 5300 K preset, which LibRaw files as Fine
Weather (older bodies store its green in a tag of its own). Two E-M5 Mark III units, whose calibrations differ by 11% in red and 14% in
blue, differ exactly as their stored 5300 K presets do, to Adobe's four decimals.
Models that share a sensor share a reference: the E-M5, E-M5 Mark II, E-M10 to E-M10
Mark IIIs, E-P5, E-PL5, E-PL6, E-PL9, E-PL10 and E-PM2 hold `[508, 256, 474]`, the E-M1
Mark III, E-M1X, E-M5 Mark III and OM-5 `[482, 256, 472]`, the E-PL7 and E-PL8
`[478, 256, 434]`. Each other model has one body in the samples.

The rows added on 2026-10-14 were checked against Camera Raw 18.7 default renders
of the test dataset. As Shot Temperature and Tint of 521 of 539 photos now land
within Camera Raw's rounding of its displayed values (Temperature to 50 K, Tint to
1), against 427 before, and so do all 27 renders made for ten bodies the dataset had
no render of. With the Olympus and OM System rows, 530 of the 538 dataset photos still
on disk do. The rest are two phones, the TG-6 and TG-7, the EOS M3 and 1D X Mark III
listed above, and two photos within a mired of Camera Raw at 7900 K and 14 000 K.

AnalogBalance is a separate per-file factor (XYZ-to-camera = AnalogBalance x
CameraCalibration x ColorMatrix). DNG Converter writes it for linear DNGs of
in-camera white-balanced modes (Canon sRAW and mRAW, Nikon small RAW, Sony's
non-mosaic modes), and their CameraCalibration matches mosaic RAWs of the same
unit, so the audit records it apart. For linear DNGs, RAWmakase applies
AnalogBalance where a white is set by temperature ([RAW
decoding](raw-decoding.md)); a mosaic DNG's AnalogBalance is not applied.

Off-diagonal CameraCalibration and different matrices for the two illuminants,
which phone DNGs carry, are not applied; such DNGs keep identity. Do not flatten
them into gains or copy another body's values. A DNG's own supported calibration
applies regardless of make.

`data/camera-calibration-facts.json` records the converter version, public source
URLs and SHA-256 hashes, the numerical DNG tags and the native presets. Local
entries contain no filenames or paths. RAWs and Adobe profiles are not committed.
CI checks without Adobe software that every row's reference reproduces each of its
samples, that units of one model share a reference unless listed as unresolved,
and that every reference has committed evidence. The camera rows' `source` and
`checked` fields describe baseline exposure.

```sh
python3 -m unittest discover -s scripts/cameras -p test_calibration.py
```

## Rendered white balance and tone controls

Install Python 3.11+, NumPy, Pillow and ExifTool. Reference capture currently uses
Photoshop's scripting interface on macOS and requires Photoshop to have no open
documents. Install matching Adobe profiles in RAWmakase through its usual profile
workflow. Use licensed local profile files; never commit them.

```sh
python3 scripts/cameras/parity.py reference --out /tmp/camera-parity \
  --raws sample.ARW another.NEF
python3 scripts/cameras/parity.py check --out /tmp/camera-parity \
  --rawmakase target/release/rawmakase
```

The default profile is Adobe Standard. For Adobe Color, also pass
`--look /path/to/Adobe\ Color.xmp`; a profile label alone does not select the actual
look. Use `--cases shadows100 dehaze40 whites100` for a shorter tone-only capture;
the default rendering is always included. The full capture also tests four
Temperature/Tint combinations and intermediate slider values.

Capture uses copies and validates the profile, look and settings actually embedded
in each reference TIFF. It saves resolved XMP and freezes source/reference/XMP
hashes in `manifest.json`. If rendering completes but verification is interrupted,
run `verify --out /tmp/camera-parity`. A frozen manifest is not overwritten.
`check` renders the frozen XMP, rejects profile substitutions and changed inputs,
and writes `report.json`, candidate TIFFs, recipes and diagnostic logs.

Comparison keeps 16-bit precision, reduces to a common 256-pixel long edge, and
uses the default render to select a small alignment offset reused for all edits.
Reports include median CIEDE2000, encoded RGB mean absolute error, and luminance
bands. Default color error must be at most 3; WB error may exceed the default by
at most 1. Tone edits may add at most .020 RGB MAE (.025 for Whites). These are
regression targets, not a promise of Adobe-identical rendering. The checker also
compares each edit’s change from default: its error must stay within 80% of the
reference effect, with a .003 MAE noise floor. This catches ignored small edits
that a baseline-error subtraction alone can miss. A failed gate
must be investigated; do not replace references or loosen limits to hide it.

Use diverse scenes, including dark images, isolated clipped highlights and large
bright areas, across multiple cameras. One sample per camera is not a test of
all scenes. Adobe's tone processing remains proprietary and this implementation
is an approximation; strong Whites and high-key Shadows still need particular
attention. Baseline profile/demosaicing differences can also dominate an edit.

```sh
python3 -m unittest discover -s scripts/cameras
```

## Compatibility and ownership

Camera calibration belongs to the model's camera facts and DNG metadata, and is
applied by the profile's Temperature/Tint conversion only when signatures match.
A DNG's own calibration takes precedence over the native-camera table. A
matrix-only embedded profile retains its signature through the fallback profile;
matching signatures enable the supported correction, and mismatches keep identity.
This includes RAWmakase Standard and Color, which use that same camera matrix.

Calibration changes which gains a Temperature and Tint mean, not a saved edit's
gains, which rendering uses. A changed reference therefore leaves existing edits'
colors as they were, while imported Lightroom and Camera Raw settings, including
their resolved As Shot values, get the gains they have in Lightroom. A saved edit
keeps the Temperature and Tint it shows; its next Temperature or Tint change uses
the current calibration.
