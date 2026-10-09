# Lens corrections

## Built-in (camera-embedded) corrections

Many cameras store per-shot lens corrections in the RAW file. Lightroom applies them as the "built-in lens profile" without any Adobe profile. RAWmakase reads them in `src/lens/embedded.rs` when a file is opened (`Metadata::lens`) and applies them while rendering, so no user import is needed.

| Camera | Tables read | Default |
|---|---|---|
| Fujifilm RAF | FujiIFD 0xF00B distortion, 0xF00F lateral CA, 0xF010 vignetting | On |
| Sony ARW | raw SubIFD 0x7032 vignetting, 0x7035 lateral CA, 0x7037 distortion | Off; follows Enable Profile Corrections ([below](#sony-built-in-corrections)) |

Radius is normalized to the half diagonal of the decoded image. Rendering order:

1. Highlight reconstruction in camera space.
2. Vignetting gain in linear camera space, evaluated at the source pixel. Shadows, Highlights, Clarity and Texture measure local luminance after this gain.
3. Distortion and lateral CA as a per-channel radial remap while sampling, scaled so corrected corners stay inside the sensor.

The recipe field `lens_builtin` controls it. New recipes enable it when the file's correction is marked `default_on` (Fujifilm and DNG, not Sony). Recipes saved before the field existed lack it and never apply it.

### Fujifilm vignetting strength

Applied at full strength, the Fujifilm table leaves corners about 0.08 EV brighter than Lightroom on three X100F photos, while the centre matches. Raising the gain to the power 0.85 brings corners within ±0.03 EV. The LCP path matches at full strength ([below](#adobe-lcp-profiles)); Sony's built-in table has not been compared. The X100F Lightroom reference scorecard goes from 0.0148 to 0.0120 mean MAE.

### Validation — 2026-09-26

The Fujifilm vignetting table for DSCF7853 (X100F) matches the `FixVignetteRadial` opcode Adobe wrote into Lightroom's DNG of the same file. The gain is 1.085 / 1.319 / 1.656 at radius 0.5 / 0.8 / 1.0, against Adobe's 1.087 / 1.326 / 1.670. Distortion is zero for this lens. Adobe's `WarpRectilinear` holds only a red-plane radial term under one pixel, the same order as the Fujifilm CA table.

Full renders against Lightroom's Adobe Standard exports, encoded sRGB at 1200 px, no fitting:

| Photo | Without | With built-in correction |
|---|---:|---:|
| DSCF7853 overall MAE | 0.0304 | 0.0196 |
| DSCF7853 corner MAE | 0.0571 | 0.0167 |
| DSCF7845 overall MAE | 0.0426 | 0.0300 |
| DSCF7845 corner MAE | 0.0546 | 0.0241 |

Sony's embedded vignetting for the FE 55mm F1.8 ZA at f/1.8 restores 1.95× at the corner. Adobe's LCP profile for that lens predicts 1.81×. Lightroom does not enable profile corrections by default, and it has not been checked whether it applies Sony's embedded data, so Sony corrections start disabled until a Lightroom reference is available.

### Sony built-in corrections

Status 2026-10-03. Available, not measured. Sony's tables are read and render, but no Lightroom or Camera Raw render has been compared with them alone, and it is still unknown whether Lightroom applies Sony's stored data or only Adobe's own profiles. What is measured for Sony is the [imported Adobe profile](#adobe-lcp-profiles) path, and one unexplained observation under [Remove Chromatic Aberration](#validation--2026-10-01).

So the correction is off by default (`default_on` is false), and in the Lens Corrections panel it follows Enable Profile Corrections: ticking the box turns it on, unticking turns it off, and the Profile row names "Sony built-in" when no imported Adobe profile matches. An imported Adobe profile replaces it whenever one matches. A Lightroom edit or preset with `crs:LensProfileEnable` set does what ticking the box does (`Recipe::set_profile_corrections`): it also turns Sony's built-in correction on or off. Without a matching imported Adobe profile the photo renders with Sony's data, which on the FE 55mm F1.8 ZA brightens the corners 1.95× where Adobe's profile does 1.81× (above). The import notice and the panel then say "Adobe lens profile for … isn't imported; using Sony built-in" (`Recipe::missing_lens_profile`), so the stand-in is never silent.

## Remove Chromatic Aberration

Lightroom's Remove Chromatic Aberration (`crs:AutoLateralCA`) needs no lens data: it measures lateral CA in the photo. `src/lens/auto_ca.rs` does the same on the decoded image, once per decode, when the recipe's `lens_ca` is set:

1. Tiles of about 1/90 of the long edge are scored by green's edge strength across the radius; tiles near the centre or with clipped pixels are skipped, and the 1200 strongest are kept.
2. In each tile, red and blue are matched to green along the radius: the best of whole-pixel shifts (up to 6 px on a 6000 px edge) by least-squares fit, a parabola, then Gauss–Newton steps to a fraction of a pixel. Tiles that fit poorly are dropped.
3. Each channel's shifts are fitted as a radial scale, 1 + k0 + k1 r² + k2 r⁴ (fewer terms with fewer or less spread tiles), with Tukey reweighting. Beyond the radius that 80% of the tiles lie within, the scale is held. Below 0.08 px at the corner (on a 6000 px edge) nothing is corrected.

The measurement replaces the lens data's own lateral CA, keeping its distortion and vignetting, and applies alone when there is no lens data (adapted manual lenses). It does not change the framing. Previews and exports measure the same decoded image; pyramid levels and reduced copies share the measurement through `Metadata::lateral_ca`, so it never comes from a reduced copy. It takes about 0.1 s on a 24 MP photo.

### Validation — 2026-10-01

Measured on the uncorrected decode and compared with the camera's own CA table, as displacement in pixels on a 6000 px long edge at radius 0.4 / 0.7 / 1.0:

| Photos | Red, measured | Red, camera table | Blue, measured | Blue, camera table |
|---|---|---|---|---|
| 8 X100F (23mm), range | +0.53…0.78 / +0.66…1.23 / +0.94…1.76 | +0.54 / +0.92 / +1.32 | −0.07…−0.42 / +0.01…−0.50 / +0.02…−0.71 | none |
| DSC05743, A7 II FE 55mm | +0.13 / +0.17 / +0.18 | +0.18 / +0.15 / 0 | −0.15 / −0.21 / −0.26 | −0.18 / −0.23 / −0.22 |
| DSC01768, A7 II FE 55mm | +0.13 / +0.16 / +0.21 | +0.18 / +0.23 / 0 | −0.16 / −0.17 / −0.22 | −0.18 / −0.23 / 0 |

Fujifilm's table corrects red only, but all eight X100F photos show blue displaced inwards at radius 0.4, which Remove Chromatic Aberration corrects too.

The same measurement on rendered sRGB output (Adobe Standard, full size) of 13 photos (8 X100F, 3 A7 II, 2 Leica M10 DNGs), against Camera Raw 18.6 renders of the same files, as mean remaining displacement in pixels of red and blue:

| | Radius 0.4 | Radius 0.7 | Worst at 0.7 |
|---|---:|---:|---:|
| Camera Raw, off | 0.24 | 0.30 | 0.48 |
| Camera Raw, on | 0.18 | 0.17 | 0.32 |
| RAWmakase, off | 0.34 | 0.31 | 0.62 |
| RAWmakase, on | 0.11 | 0.15 | 0.49 |

Output-space figures are only comparable with each other: the colour matrix mixes channels, so they differ from the camera-space ones above.

Camera Raw renders with the setting off also show Sony's lateral CA corrected, as RAWmakase does only with built-in corrections on; this was seen on two A7 II photos and has not been investigated further.

## Manual distortion

Lightroom's manual Distortion (Lens Corrections > Manual, `crs:LensManualDistortionAmount`, −100 to 100) is `Recipe::lens_manual_distortion`, rendered by `geometry::ManualDistortion`. Measured on Camera Raw 18.7 renders of the synthetic chart at ±10, ±25, ±50, ±75 and ±100, fitting the radial map to whole images (residual 0.004–0.006 mean absolute difference, the resampling floor):

- An output position at radius r, with r = 1 at the frame's corners, samples radius r·(1 + k·(1 − r²)), with k = 0.4 × amount / 100 for positive amounts and 0.5 × amount / 100 for negative ones. Corners stay where they are.
- Positive amounts correct barrel distortion: the middle shrinks and the edges' middles come from outside the photo, which renders white, as Lightroom shows it without Constrain Crop. Negative amounts correct pincushion distortion and enlarge the middle, so nothing white appears.
- Order: Camera Raw applies it in the frame as recorded, centred on the uncropped photo. Crop and Straighten cut its result (a cropped render equals the same crop of the uncropped one exactly), and Upright and the Transform sliders apply after it: Vertical, Scale and Offset renders match only that order. On a DNG whose WarpRectilinear opcode distorts the chart, the lens correction applies after it on the way to the sensor, so `Geometry::source` applies it after the homography and before `LensMap`.

The `lens-manual-distortion*` corpus cases (±50, and +50 with Vertical +30) sit at mean ΔE00 1.3–1.7 from Camera Raw (the default render is 0.9), against 16–21 if it were ignored. The Lens Corrections panel's Distortion › Amount slider sets it. Constrain Crop crops the white out ([transform](transform.md#constrain-crop)).

## Manual vignetting

Lens Corrections > Manual > Vignetting (`crs:VignetteAmount`, `VignetteMidpoint`) follows Camera Raw 18.7, measured on flat synthetic DNGs at three brightnesses, a 3:2 and a square frame, and four crops (`effects::lens_vignette`). Camera Raw multiplies scene-linear light by a radial gain over the whole photo, before the tone curve: the crop neither moves nor resizes it, and the gain is the same at every brightness. Positive amounts lighten the corners. With `r` the distance from the centre over the half diagonal, `ln gain = amount × c·r^p / (1 + k·r^p)`, where Midpoint (0–100) raises `p` from 2.1 to 9.8 and moves `c` and `k` a little. The fit's log error is 0.018 RMS over 32 renders.

RAWmakase applies it while sampling the camera image, in one table with the lens profile's vignetting (on the GPU as well), so it also works without lens data. On the chart cases `lens-vignetting-50`, `+50` and `-50-midpoint20`, mean ΔE00 to Camera Raw went from about 5.1 (7.5 at Midpoint 20) to 0.8–1.0, the default render's own distance. Before, RAWmakase darkened the finished pixels inside the crop with the opposite sign; that operator is no longer rendered, and recipes saved with it render with the measured one.

The radius is measured on the photo frame, the camera's default crop; the gain shares the lens table's centre, the decoded image's, so a default crop off the sensor's centre is approximated there.

## Defringe

Lightroom's Defringe (`crs:DefringePurpleAmount`, `…GreenAmount` and their Hue ranges) reduces the chroma of hues inside the Purple and Green ranges. `Effects::defringe_color` does it per pixel in Oklab, after the colour controls:

- The Purple and Green windows are centred at Oklab hue 0.875 and 0.46 (0–1). Each Hue slider's 0–100 spans 0.5 of hue around its centre, and a range's ends are softened over ±0.025.
- At full weight the share of chroma removed is (1 − 0.45·e^(−C/0.09)) · (1 − e^(−Amount/2.5)), with C the Oklab chroma and Amount 0–20: a stronger fringe colour loses more of its chroma, and Amount 5 already does most of the work.

The constants were fitted to Camera Raw 18.6 renders of two A7 II photos (FE 55mm F1.8 ZA at f/1.8 and f/5.3) at Purple 5, 10 and 20, Purple 20 with a 45–55 range, and Green 10 and 20, comparing the chroma kept per hue between each render and the same photo without Defringe. The mean difference from Camera Raw fell from 0.14–0.26 to 0.05–0.11 across those cases. Camera Raw also reduces chroma somewhat more next to strong edges, including hues just outside the range; that spatial part is not reproduced.

The Fringe Color Selector (the eyedropper at the Defringe heading) reads the shown colour, picks the nearer window that holds its hue, sets that range to 20 wide around it and Amount to 5 when it was 0. Reds, yellows, blues and near-greys are refused.

## DNG files

A DNG records the corrections Lightroom applies to its raw image. `src/dng.rs` reads FixVignetteRadial from OpcodeList2 and WarpRectilinear from OpcodeList3 (radial terms, per plane, when centred) into the same correction model, enabled by default. It also reads the embedded camera profile (offered as the file's own profile, as in Lightroom), BaselineExposure and DefaultCrop. The Lightroom-made DNG of DSCF7853 renders at 0.020 MAE against Lightroom's export with no imported files, against 0.030 before. GainMap opcodes (used by phone DNGs for lens shading) are not applied yet.

## Adobe LCP profiles

Lightroom's Enable Profile Corrections uses an Adobe lens profile. RAWmakase reads the same `.lcp` files when the user imports them (`rawmakase import-lens-profiles FILE…`, or the app's import command); they are copied to `lens-profiles` in the data directory and never read from a Lightroom installation. Each file is imported on its own, so a file without usable entries (Adobe ships a few) is listed as skipped and the rest still import. `src/lens/lcp.rs` matches the photo's lens model, preferring raw profiles and profiles made on the photo's camera make, then on a make sharing the lens mount, as Adobe profiles some third-party lenses on one body only, and interpolates the model in focal length and aperture, taking the farthest focus distance. It converts distortion, vignetting and chromatic models to the correction above. When the profile sets PreferMetadataDistort, the camera's own distortion is kept, as Lightroom does.

`Recipe::lens_profile` enables it, from `crs:LensProfileEnable`. `lens_distortion` and `lens_vignetting` are the profile's Distortion and Vignetting amounts (`crs:LensProfileDistortionScale` / `VignettingScale`, 0–200%): the distortion's radial scale moves from none at 0 to twice the profile's at 200, and the vignetting gain is raised to the amount (0 none, 200 the gain squared). With an imported profile in use, the profile replaces the built-in correction. Without one, the built-in correction applies when `lens_builtin` is set.

### Choosing a profile

The Lens Corrections panel's Profile part has Lightroom's Setup, Make, Model and Profile menus. They list the imported profiles that fit the photo's camera (`lcp::PhotoProfiles`, rebuilt when a photo opens): profiles with an entry whose sensor covers the camera's, raw profiles in place of non-raw ones for the same lens. A profile is one imported `.lcp` file. Make and Model come from its `LensPrettyName` (the first word is the lens maker), the Profile item is its `ProfileName`.

`Recipe::lens_profile_choice` holds the Setup and the profile the edit names (`lens::choice`), from and to `crs:LensProfileSetup` (`LensDefaults`, `Auto`, `Custom`), `LensProfileName`, `LensProfileFilename` and `LensProfileDigest`:

- **Default** and **Auto** use the imported profile of the photo's lens made on the same camera make, else on a make sharing the mount, else on any make. RAWmakase keeps no saved lens defaults, so Default matches as Auto does, as Lightroom does for a lens without one. A profile the edit names is used instead when it is imported and profiles this lens.
- **Custom** uses the profile the edit names, whatever lens it was made for, as for an adapted or manual lens. Picking a make, model or profile sets Custom; picking Default or Auto forgets the named profile and matches again.
- A named profile is found by its file name, ignoring case, or by profile name when the edit records no file name; another file with the same profile name does not stand in for a recorded one. One that isn't imported is kept in the edit, written back to exports, and reported in the import notice and under the menus: `Lens profile "…" isn't imported; using …`, with the matched profile (Default, Auto), the built-in correction or no correction.
- An edit whose profile is the one the RAW carries (`LensProfileIsEmbedded`, which Adobe names "Camera Settings") renders the built-in correction, even when a matching LCP is imported, and says so when the file has none.
- `LensProfileDigest` is kept as read and written back with the same file; RAWmakase does not compute Adobe's digest, so a profile picked here is written without one.

Constrain Crop and the profile do not interact: the profile's distortion is scaled to stay inside the photo at every amount, and Constrain Crop crops what Upright, the Transform sliders and manual Distortion uncover after it. Switching the panel off renders no profile and keeps the choice.

Against Camera Raw 18.6 renders with profile corrections on (8 A7 II photos, FE 55mm F1.8 ZA at f/1.8), RAWmakase with the imported Adobe profile averages 0.0088 MAE, with centre and corner exposure within ±0.02 EV. Without correction the corners were 1 EV darker.
