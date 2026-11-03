# Camera table

`data/cameras.toml` holds what RAWmakase knows about camera bodies beyond what the raw
file says, one `[[camera]]` row per model. It is compiled into the app
(`crates/rawmakase-model/src/cameras.rs`) and checked by a unit test, so a malformed row fails `cargo test`.
A row carries baseline exposure. Sony, Canon, Fujifilm, Olympus and OM System rows may
also hold a `unit_calibration`: a white-balance reference that, divided by a preset the
individual RAW stores, gives Adobe's calibration of that camera unit.
Calibration uses exact model/alias matching and identity for unknown bodies; it
does not use the baseline exposure fallback. See
[camera parity](camera-parity.md) for calibration provenance and automated audits.

## Baseline exposure

Camera Raw and Lightroom brighten every unedited raw by a per-camera amount, the
BaselineExposure Adobe writes into a DNG it converts. Without it, RAWmakase renders
most cameras darker than Lightroom: about 0.3 EV for Sony, Canon and Nikon bodies.

RAWmakase applies, in order:

1. A DNG's own BaselineExposure tag, or 0 when the DNG has none (the DNG default).
   The table is never used for DNGs.
2. The camera's row in `data/cameras.toml`.
3. For a camera without a row, the median of the rows of the same make, or the
   median of all rows when the make has none.

Fujifilm rows hold at DR100 and base ISO. Camera Raw's value for a Fujifilm raw is a
per-body constant minus the exposure midpoint shift in its maker notes, which is about
−0.72 EV at DR100 on X-Trans bodies (−0.49 on GFX, 0 on the small Bayer bodies), a
stop lower for each DR step and a stop higher at extended low ISO. RAWmakase adds the
difference from that DR100 shift to the row, so DR200 and DR400 photos work even when
the raw does not record the DR mode itself. Camera Raw keeps one value whatever the
shift for the X-T2 (measured at DR200) and, inferred from their equal values, the other
X-Trans III bodies (X-E3, X-Pro2, X-T20, X100F, X-H1); their rows say
`fujifilm_exposure_shift = "ignored"`. RAWmakase assumes the usual DR100 shift of the
sensor type (−0.72 X-Trans, 0 Bayer); a body that records another one, like the GFX
bodies' −0.49, says so in its row with `fujifilm_dr100_shift`. `rawmakase inspect` on a
DR100 raw at base ISO prints it as `fuji_exposure_shift`.

The baseline is stored per edit, apart from the Exposure slider, so changing the table
changes photos opened afterwards. An existing edit keeps its value; Develop's
"Use camera exposure baseline" button applies the current one.

### How good the fallback is

Predicting each of the 180 bodies in the table on 2026-10-05 from the other rows
(leave-one-out) misses its row by:

| Rule | Mean error | 90th percentile | Worst |
|---|---|---|---|
| No baseline | 0.29 EV | 0.50 EV | 0.85 EV |
| Median of all rows | 0.20 EV | 0.45 EV | 0.90 EV |
| Median of the same make | 0.14 EV | 0.35 EV | 1.00 EV |

The worst cases are bodies unlike the rest of their make: Pentax's 645Z (−0.5 among
+0.5 bodies), Panasonic's GH5 II and G9, and the 1-inch compacts (Sony RX100 and ZV-1
at −0.25, Nikon Coolpix at −0.3, against +0.3 for their makes' larger sensors). No
rule from the raw's own metadata did better: within a make, neither white level nor
black level orders the baselines. The same-make median is the fallback.

The 45 older Canon, Sony and Fujifilm rows added on 2026-10-14 moved the Canon median
from 0.4 to 0.3 EV; the other makes' medians did not change. Canon bodies still without
a row are mostly older ones: on Camera Raw 18.7 default renders of one photo each of 24
such EOS bodies, the median exposure difference went from 0.18 EV too bright to 0.04 EV.

The camera-matching DCPs (Camera Standard and so on) of some Sony bodies carry a
BaselineExposureOffset of −0.35 EV, which RAWmakase applies with those profiles; Adobe
Standard DCPs carry none.

## Adding a camera

Add one row by hand; nothing else is needed:

```toml
[[camera]]
make = "Panasonic"          # the decoders' names: `rawmakase fingerprint <file>`
model = "DC-S5M2"           #   prints them (header.facts.normalized)
aliases = ["S5 II"]         # optional
baseline_exposure = 0.35    # EV
source = "measured"         # adobe-dng | adobe-profile | fitted | measured
how = "Lightroom DNG export of one photo, read with exiftool -BaselineExposure"
checked = "2026-10-05"
sample = "1 photo"
```

`source` says where the number comes from:

- `adobe-dng`: the BaselineExposure tag of a DNG that Adobe DNG Converter, Lightroom or
  Camera Raw wrote for the camera (`exiftool -BaselineExposure file.dng`). Exact.
- `adobe-profile`: the BaselineExposureOffset tag of the camera's Adobe Standard DCP.
  Adobe's current profiles leave it out, so this is rare.
- `fitted`: fitted to Camera Raw renders of unedited photos, the median exposure
  offset over midtone areas; about ±0.05 EV per photo.
- `measured`: anything else; `how` says how.

Values are rounded to 0.05 EV. Only numbers are recorded: never commit raw files,
DNGs or Adobe profiles.

### Optional: filling rows by script

`scripts/cameras/fit-baselines.py` prints rows to paste:

- `--dng <folder>` reads BaselineExposure from DNGs made by Adobe DNG Converter from
  copies of sample raws, kept outside the repository. Check the make and model it
  prints against the decoders' names.
- `--report <report.json>` fits rows from the exposure check in
  `tests/corpus/README.md` (`parity-report.py --photos`): Camera Raw renders of the
  corpus photos against RAWmakase's default renders. Rerun the check after changing
  rows; the second pass lands within ±0.05 EV.

## Adobe's names for a camera

An imported Adobe profile (a DCP) names the camera it was made for, and RAWmakase
uses it only for photos from that camera (`CameraProfile::ensure_camera`). Adobe
and the decoders (which keep LibRaw's names) name many cameras differently, so a
profile fits when its camera is any of:

- the make and model the decoder reports, or the model alone;
- the decoder's canonical name for a body sold under several names (`normalized_model`):
  the Panasonic DC-G95 is Adobe's DC-G99, the DC-TZ91 its DC-ZS70;
- the names in [data/adobe-camera-names.toml](../data/adobe-camera-names.toml):
  makes Adobe spells otherwise (OM Digital Solutions), phones Adobe names by
  their marketing name (Samsung SM-G965F is the Galaxy S9+) and twins the decoder
  names as themselves (the Panasonic DC-G97 is Adobe's DC-G99), each with a `why`;
- for Google phones, Adobe's older `Model-Google-google`.

Names are compared without case, spaces or underscores (the decoders' Nikon "Z 6_2" is
Adobe's "Z 6 2"). A phone profile named for a camera module (Rear Main, Telephoto…)
fits the module the photo's 35 mm focal length points to. The one-click import
offers Adobe's profiles under the same rule. `rawmakase inspect` lists a photo's
`adobe_names`.

Add a table entry only when neither the decoder's canonical name nor the comparison
finds the profile, and check it against a Camera Raw render of the camera.

## Sources of the current rows

Most rows (`adobe-dng`) come from Adobe DNG Converter 18.x: it converted a copy of one
CC0 [raw.pixls.us](https://raw.pixls.us) sample per camera (the files
`scripts/corpus/pixls.py` downloads), and the row is the DNG's BaselineExposure,
rounded to 0.05 EV. RAWmakase's decoders read Adobe's black and white levels
([RAW decoding](raw-decoding.md)). Canon's white Adobe takes by ISO speed (the camera
table's rows of `iso`, ColorData's levels), and moves its BaselineExposure with it, so
that a photo's brightness stays referenced to one level: the EOS 1100D's is −0.5 at
ISO 160, whose white is 11222, and 0 at ISO 400's 15000: the photo's brightness stays
referenced to the samples' full range. Canon rows therefore give `levels`, the black,
white and samples' maximum of their sample, and a photo whose levels differ (known on
opening, `Metadata::levels`) takes the difference of log2((white − black) / (maximum −
black)) between the two. On the 325 Canon files with a row that Adobe DNG Converter
converted, the exposure is then within 0.1 EV of Adobe's BaselineExposure on 323, as
it was while RAWmakase decoded with LibRaw (234 with the row alone; the white over the
black alone leaves the bodies whose black rises with ISO speed, the 80D's, 200D's and
800D's, 0.2 EV off). The two left are an EOS 6D at extended ISO 50 (Adobe takes a stop
off) and an EOS 1300D frame 0.11 EV off. Other makes' BaselineExposure does not follow their white
(the ILCE-7RM4A's is 0.35 at ISO 160's white of 15000 and at ISO 250's 16383), so
their rows are Adobe's value alone. While RAWmakase decoded with LibRaw, rows added the
difference between Adobe's white level and LibRaw's, log2((LibRaw white − black) /
(Adobe white − black)), up to +1.1 EV (the DC-GH5S at ISO 800), and `sample` said
"plus … for the white level"; when the decoders switched, those terms were taken out
again (the term measured on the dataset's files at each sample's ISO was the one the
row had added, on every row with a file at that ISO), and fitted rows lost the same
term.

Adobe's value also follows some shooting settings, which the rows leave out: Canon
Highlight Tone Priority and Fujifilm DR200 add 1 EV, extended low ISOs (Sony ISO 50,
Fujifilm ISO 100 on ISO 160 bodies, Olympus ISO LOW) take 1 EV off. Each such row says
so in `sample`. RAWmakase adds the stop for Highlight Tone Priority itself, reading it
from the Canon maker notes (both On and Enhanced; no Enhanced sample has been measured)
and, for the bodies from the 40D to the 600D, from their custom functions, where LibRaw
does not read it: their CameraInfo lacks it (60D) or says Off (550D) when it is on, and
Camera Raw follows the custom function. RAWmakase also follows Fujifilm's exposure midpoint shift (above),
and takes the stop off below a row's `lowest_iso`: the Sony A7, A7R, A7R II, A7 III, A7R III,
A7 IV and A7R IV, whose ISO 50, 64 and 80 are extended settings under a native 100 (Adobe DNG
Converter 18.7 on 32 dataset photos of these bodies: always exactly 1 EV under the row; the
A7S III's and HX99's ISO 80 is native and keeps the row). On the 20 such photos with Camera
Raw default renders, median dE00 went from about 10 to 0.4. Extended low ISO on other makes
(Olympus ISO LOW) is not handled yet.

After the table, 151 of the 156 raw.pixls.us samples that Camera Raw 18.7 and
RAWmakase both render come out within ±0.1 EV of Camera Raw (median midtone, LibRaw
master of 2026-10-02). For 15 bodies Adobe's value alone left the render more than
0.1 EV off, so their rows are `fitted` to the render instead and say Adobe's value in
`sample`: the Fujifilm X-H2 (0.40 EV darker in Camera Raw), X-T30 II (0.38 darker), X-S10,
X-S20, X-T5 and X100VI, the Canon PowerShot G5 X Mark II, the Nikon D500,
D5600, D850 and Z 7, the Olympus PEN-F, the Pentax K-70 and the Sony RX100 VII, and since the exposure shift is
followed, the Fujifilm X-H2S (0.14 darker at DR200). The Sony A7 V row is
fitted too: its Adobe DNG is 16-bit, so its white level does not compare with LibRaw's.
Camera Raw renders most of those Fujifilm bodies darker than their baseline explains,
for a reason not found yet.

Still about 1 EV off, for the shooting settings above or a decoding problem: the
Olympus E-M10 Mark III (extended low ISO). LibRaw reads the black level
of the Canon EOS R6 Mark III and PowerShot V1 as 0 plus small per-channel values instead
of 512; RAWmakase takes the black from the raw's masked left border when LibRaw's is
below a quarter of it, which brings both within 0.1 EV. The Leica Q, Ricoh GR III and Sigma fp L
raws are DNGs with their own value and render 0.11 to 0.13 EV brighter than Camera Raw.

Not compared yet: the Hasselblad, OM System, Olympus ORI and Fujifilm GFX100 II,
GFX100RF, GFX100S, GFX100S II and GFX50S II samples, for which RAWmakase does not find the installed Adobe Standard profile by name, and the Pentax KP
and 645Z and Fujifilm X-T200 and X-T50, whose default crop differs from Camera Raw's.

Eight rows are not from raw.pixls.us: the X100F (read from two Lightroom DNGs, see
[macOS and Lightroom validation](records/macos-lightroom-validation.md)), the A7 II and A7CR, fitted to Camera Raw 18.6
renders of private photos (8 and 4), the A7R VI, EOS 700D and A7R, read from
Adobe DNG Converter 18.7's DNG of one private photo each, the EOS 1500D (2000D),
read from its DNG of one Signature Edits photo, and the EOS R8 Mark II, read from
its DNGs of six private photos. LibRaw does not identify
the A7R VI by name yet, but its white level, black level and crop match Adobe's.

The 45 rows added with per-unit calibration on 2026-10-14 (older Canon, Sony and
Fujifilm bodies; see [camera parity](camera-parity.md)) are `adobe-dng` values from
one test-dataset photo each, at the ISO the row says. Against Camera Raw 18.7 default
renders of 110 photos of these bodies (the dataset's, and 26 rendered for bodies it
had none of), the median midtone exposure difference went from 0.15 EV to 0.02 EV, and
103 photos are within 0.1 EV (41 before). Of the other seven, two 80D sRAW frames of one
shoot were already 0.44 EV brighter than Camera Raw and are now 0.58 brighter, the
row's white-level term, measured on a full-size raw, not holding for LibRaw's sRAW
conversion (see below); a 30D photo is 0.30 EV brighter where another body's
at the same ISO matches; a 5DS R and a Rebel T6 photo at other ISO settings than their
rows' samples went from 0.3 EV to 0.16 EV off, Canon's white level being
ISO-dependent; and two RX1R III frames stay 0.14 EV darker, the row's value being the
Sony fallback they had.

### Canon sRAW and mRAW

RAWmakase converts Canon's sRAW and mRAW as Adobe DNG Converter does, on Adobe's
linear scale and white level ([RAW decoding](raw-decoding.md)), so rows give
`sraw_baseline_exposure`: Adobe DNG Converter 18.7's sRAW BaselineExposure. On the
dataset's 37 sRAW and mRAW files (16 bodies) that value is one per body, the same for
sRAW and mRAW and from ISO 100 to 3200. A Canon body without one takes their median,
0.25; the 40D, 50D, 5DS and 1D Mark IV have no full-size row to hold theirs yet.
LibRaw converted them on a 16383 scale with its green 1.303 times Adobe's linear values
(its stored sRAW white balance, 1336/1024); while RAWmakase decoded with LibRaw, rows
added log2(16383 / (1.303 × Adobe's sRAW white level)) to Adobe's value. Against Camera
Raw 18.7 default renders of the 7 such photos the dataset has, the rows of that time
took the median dE00 from 2.6 to 1.1 (the two 80D frames 4.2 and 5.1 to 0.45 and 1.4);
a 5DS R mRAW and a 5D Mark III sRAW at ISO 3200 rendered 0.4 and 0.2 EV darker than
Camera Raw, which renders them brighter than Adobe's own DNG of them says.

Not in the table: cameras whose raw is a DNG (Leica M, Q, SL and CL, Ricoh GR, Sigma
fp), which carry their own value; the Hasselblad X1D and X1D II, which LibRaw names
alike; and the Nikon D810, whose only CC0 sample is an sRAW.
