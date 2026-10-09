# Imported Lightroom camera and look profiles

RAWmakase renders with profiles from its own library and files the user explicitly selects. Proprietary profile assets are not shipped with the application; RAWmakase ships its own profiles instead (see [RAWmakase profiles](#rawmakase-profiles)). On macOS and Windows, the Develop **Profile** menu looks in Camera Raw's profile folder (`/Library/Application Support/Adobe/CameraRaw/CameraProfiles`, `C:\ProgramData\Adobe\CameraRaw\CameraProfiles`) for the current camera's Adobe Standard and Camera Matching profiles, and offers **Import Adobe profiles for this camera** when it finds some that are not imported yet. To decide whether to offer the button it only lists that folder's file names for the current camera; no profile is opened, parsed or copied until the button is used, and then the files are copied like any other import.

## Import and select

Use **Import profiles…** in the Develop Profile menu or **Import Profiles…** in Preferences. Select one or more `.dcp` / `.xmp` files. For Adobe Color and the other Adobe Raw looks, import both the camera model's **Adobe Standard DCP** and the desired **XMP look profiles**. You can select these together or import the DCP first. Missing dependencies produce an error; another camera's profile is never substituted.

The CLI uses the same importer:

```sh
rawmakase import-profiles '/selected/Fujifilm X100F Adobe Standard.dcp' '/selected/Adobe Color.xmp'
```

Selected files are validated and copied into `camera-profiles` under RAWmakase's application data directory (`~/Library/Application Support/RAWmakase` on macOS, `$XDG_DATA_HOME/rawmakase` on Linux). An identical reimport is harmless. A different file with the same filename is reported instead of overwritten. The entire selection is validated before copying starts. Source files remain unchanged.

Importing does not change the current edit. Choose the imported look in **Profile**. New unedited photos prefer a compatible imported Adobe Color, then imported Adobe Standard, then a DNG's embedded profile, then RAWmakase Color. Saved edits retain their embedded profile and previous rendering flags, so edits made before RAWmakase Color existed keep LibRaw's camera matrix with the DNG default tone curve (engine 4, see [color-pipeline.md](color-pipeline.md)).

## RAWmakase profiles

Two profiles of our own are listed for every camera with a colour matrix, with no files to import. They follow Adobe's two layers: a per-camera base and a camera-independent look.

- **RAWmakase Standard**: the camera's colour matrix (LibRaw's, or the D65 matrix of a DNG's own profile) with the DNG default tone curve. It uses the same colour data as "Default (camera matrix)", which stays in the Profile menu as the entry for no profile and renders through the same matrix and DNG default tone curve.
- **RAWmakase Color**: a look on top of Standard, as Adobe Color is on Adobe Standard. A mild contrast curve and a few smooth hue, saturation and brightness shifts (reds, skin, yellows, foliage, aqua, sky), plus a slight saturation roll-off near white. Neutrals stay neutral. The table is generated from the parameters in `src/camera_profiles/open.rs`, not stored or derived from Adobe data.

Both are embedded in the recipe like any other profile, so later changes to the look don't change existing edits. The look was tuned conservatively and has not yet been compared with Camera Raw renders; measured base profiles (ColorChecker shots per camera) can replace the matrix later without changing any preset.

## Rendering

An enhanced XMP profile is resolved against its matching camera DCP. The profile name remains, for example, **Adobe Color**, but the combined rendering contains that camera's matrices and calibration tables plus the XMP look's table and curve. Camera matching is mandatory.

Creative looks (Lightroom's Artistic, B&W, Modern and Vintage groups) name no base profile. RAWmakase puts them over the camera's Adobe Standard when it is imported, as Lightroom does, else over the DNG's own profile, else over RAWmakase Standard. They import without a DCP.

Supported enhanced-profile features:

- Adobe DNG SDK-format HSV big tables, versions 1/2, decoded from Adobe base85 and zlib, with bounded input/output sizes and validation.
- Linear and sRGB value indexing, interpolated hue/saturation/value corrections.
- Adobe RGB tables, 1D and 3D, in sRGB, Adobe RGB or ProPhoto primaries with linear, sRGB, 1.8 or 2.2 encoding, described in [RGB tables](#rgb-tables).
- A profile-specific master tone curve, applied separately from the user's point curve. Identity per-channel profile curves are accepted; nonidentity profile channel curves are reported as unsupported.
- Profile-internal Highlights, Shadows, Clarity, Contrast and Blacks adjustments, plus monochrome conversion. These use RAWmakase's existing approximate operators without moving the user's sliders.
- Profile-internal Exposure, Saturation, colour mixer, parametric curve, split toning and post-crop vignette, described in [Settings inside looks](#settings-inside-looks).
- The six Adobe Raw looks: Color, Portrait, Neutral, Landscape, Vivid and Monochrome (these have no Amount), the creative looks Artistic 01–08, B&W 01, B&W 03 to 12, Modern 01–10, and Vintage 01–10, and camera-matching XMPs that carry their RGB table (over the camera's imported Camera Standard DCP, or the one they name).
- Profile Amount for looks that have one (`crs:SupportsAmount`), described below.
- XMP sidecars/presets and Lightroom catalog `Look` records resolve imported profiles by name, UUID when supplied, and camera model.

The existing bounded DCP implementation continues to support imported camera-matching and third-party film profiles. Adaptive/AI profiles, camera-matching XMPs whose RGB table is not in the file (Fujifilm's film simulations, whose tables Camera Raw keeps elsewhere), looks that need a DNG's own RGB tables (`RequiresRGBTables`), looks with settings RAWmakase doesn't apply inside a profile (B&W 02's white balance, the B&W filters' mix, colour grading, Dehaze and the other settings not listed above), and unsupported DCP variants fail explicitly. This is not universal Lightroom profile support or pixel-identical Lightroom development.

## Profile Amount

The **Amount** slider under the profile is Lightroom's Profile Amount, 0–200%. It is enabled for looks that support it and shown dimmed at 100% for every other profile, as in Lightroom; choosing a profile sets it back to 100%. It reads and writes `crs:Look`'s `Amount` in XMP sidecars, presets and Lightroom catalogs, travels with the Treatment & Profile group in Copy, Paste, Sync and presets, and is its own History step. Amounts outside 0–200% in imported edits are reported and render at 100%.

The rule was measured with Camera Raw 18.7 on the synthetic chart and seven synthetic looks (`tests/corpus/looks`, written by `scripts/corpus/synthetic-looks.py`), each isolating one part, at 0, 50, 100, 150 and 200%:

| Part | Camera Raw's rule | RAWmakase |
|---|---|---|
| Look table | Hue shifts and saturation/value scales grow in proportion to the Amount, also above 100%, within the amount bounds a version 2 table stores. A version 1 table stays at 100% at any Amount. | Same; the table is scaled when the recipe is resolved, so the CPU and GPU paths read one table. |
| Look curve | Up to 100% a blend of no change and the curve; above, the curve is applied a second time at the excess (200% is the curve applied twice). | Same. |
| Internal settings (Shadows, Highlights, Contrast, Blacks, Clarity) | In proportion up to 100%, half as fast above: +40 Shadows is +60 at 200%, matching Camera Raw's render with user sliders at +60 within ΔE00 0.14. | Same, added to the user's sliders without moving them. |
| Black & white | A B&W look stays black and white at 0%. | Same. |

The look goes in the same place as at 100%: the table after the camera profile's tables, the curve after its tone curve, the internal settings with the user's. On the chart the `amount-*` cases sit at mean ΔE00 0.7–1.4 from Camera Raw for the table, B&W and internal-settings looks at 50%, and 1.3–2.8 at 200% (RAWmakase's default render is 0.9). What remains comes from existing operators rather than the Amount: RAWmakase's Shadows lifts a gray ramp less than Camera Raw's (+25 Shadows raises middle gray by 13 levels in Camera Raw and 2 here), and its Clarity has no global part; Contrast with Blacks (`amount-contrast-*`) is now 1.0 off at 50% and 1.6 at 200%. A look combining a curve with internal Shadows and Clarity is therefore 2.8 off at 100% and 4.7 at 200%.

## RGB tables

Lightroom's Artistic, Vintage and most Modern looks, and Adobe's camera-matching XMPs for Nikon, Panasonic, Sigma and others, carry an RGB table (`crs:RGBTable`, a `Table_<md5>` attribute in the DNG SDK's big-table encoding) beside or instead of an HSV look table. A table is a curve per channel (1D) or a grid over all three (3D, up to 64 divisions per side), stored as 16-bit differences from the identity, with the primaries and encoding it works in, whether colours outside it are clipped or extended, and the amounts it may be applied at. RAWmakase interpolates 3D tables tetrahedrally, as the DNG SDK does, on the CPU and in the GPU shader, which agree within 0.0007 of the output range.

Measured with Camera Raw 18.7 on the synthetic chart and synthetic RGB-table looks (`tests/corpus/looks/synthetic-rgb*.xmp`, from formulas in `scripts/corpus/synthetic-looks.py`):

| Question | Camera Raw's answer | Cases |
|---|---|---|
| Where it applies | After the user's tone controls, point curves, HSL and Saturation, before color grading: `rgb-table-point-curve` is 0.87 ΔE00 from Camera Raw with the table there and 2.80 with it at the end of the profile; with Saturation +50 1.78 against 3.75. | `rgb-table-*` |
| Amount | `r + amount·(table(r) − r)` in the table's encoding, at the look's `RGBTableAmount` (1 when absent) times the Profile Amount, clamped to the bounds the table stores. A 0.5 look at 200% renders as the full table at 100%, identical to the 16-bit level; bounds 0.5–1.5 hold 0% at half and 200% at 1.5. At an amount of 0 RAWmakase leaves the table out, so it clips nothing either. | `rgb-table-50/100/200`, `rgb-half-*`, `rgb-bounds-*` |
| Primaries and encoding | Codes 0, 1, 2 are sRGB, Adobe RGB and ProPhoto; gamma codes 0–3 linear, sRGB, 1.8 and 2.2. | `rgb-srgb-100`, `rgb-linear-100` |
| 1D tables | A curve per channel, stored like the 3D samples. | `rgb-1d-100` |
| The extra word some camera-matching tables end with | No visible effect at 0 or 1. | `rgb-flag-100` |

The table-only cases sit at mean ΔE00 0.68–0.81 from Camera Raw (RAWmakase's default render is 0.91), and the look combining an HSV table, a curve and an RGB table at 1.0–1.14. Gamut extension (used by Modern 02 only) adds back what clipping into the table's space removed, in its encoding; it was not checked closely, because RAWmakase compressed out-of-gamut colours toward gray where Camera Raw clips them; RAWmakase now clips per channel too ([out-of-gamut colors](color-pipeline.md#out-of-gamut-colors)).

A look's HSV table and curve stay where they were, with the camera profile; only the RGB table goes late. Recipes with an RGB-table look save as version 9, which earlier releases refuse as newer instead of dropping the table; the table is stored in Adobe's encoding, about 180 KB for a 32-division table.

## Settings inside looks

Vintage 07 and Modern 03 and 04 carry develop settings besides their tables: Exposure, Saturation, colour mixer bands, the parametric curve, split toning and a post-crop vignette. RAWmakase renders them with the user's settings without moving the user's sliders. Measured with Camera Raw 18.7 on synthetic looks (`synthetic-color`, `-split`, `-parametric`, `-exposure` and `-vignette`) at 50, 100 and 200%:

| Setting | With the user's | Amount | `look-*` cases, mean ΔE00 |
|---|---|---|---|
| Exposure, Saturation, colour mixer, parametric curve | Added to the user's sliders | As the tone adjustments: in proportion up to 100%, half as fast above | colour 1.0/1.2/1.4, exposure 1.2/1.5/2.6, parametric 1.7/2.8/4.9 |
| Split toning | Tones the shadows or highlights the user doesn't tone (`look-split-grading-shadows-h30` 3.5, against 4.9 when the user's toning replaced the look's and 5.3 when the look's replaced the user's) | Saturations scale, hues stay | 1.6/2.6/3.5 |
| Post-crop vignette | Replaces the user's: Camera Raw renders a look vignette with the user's −20 exactly as without it | Amount scales | 1.0/1.2/1.5 |

The half rate above 100% matched best for the colour, exposure and vignette looks (at 200%, colour 1.4 against 2.3 for full rate, vignette 1.5 against 1.8), and was even for split toning; the parametric look was closer at full rate (4.0 against 4.9), but RAWmakase's parametric curve is already 2.8 off at 100%, as it is for the user's own parametric sliders. The remaining error is that of the existing operators: parametric curve (user Lights −50: 3.7), split toning (user shadows 210°: 2.6) and Exposure above +0.5.

Schema/pipeline 5 embeds the resolved camera profile, enhanced color table, sampled curve, identity and copyright in the recipe. Reopening does not require the source XMP or DCP to remain available. Old schema 1–4 recipes migrate and render with the current engine. Older RAWmakase versions reject version 5 instead of silently dropping enhanced-profile data.

## Lightroom comparison — 2026-09-26

Six full-size, 16-bit sRGB TIFF references were exported from Lightroom Classic using a separate comparison catalog and copied X100F RAWs. The XMP look parameters contain the actual selected Adobe profile's table and curve. Exported metadata and catalog records were checked to verify the selected look. No original catalog edits or original RAW changes were made.

Measurements are encoded-sRGB MAE at an 800-pixel long edge, using the existing preview comparison script, with no exposure/color fitting or geometric registration. Active-area/demosaicing differences prevent these from being pixel-aligned sensor comparisons.

| Profile | RAW | Adobe Standard baseline MAE | Imported profile MAE |
|---|---|---:|---:|
| Adobe Color | DSCF7853 | 0.03305 | 0.02643 |
| Adobe Portrait | DSCF7845 | 0.01869 | 0.01819 |
| Adobe Neutral | DSCF7866 | 0.03488 | 0.01667 |
| Adobe Landscape | DSCF7853 | — | 0.02786 |
| Adobe Vivid | DSCF7853 | — | 0.03074 |
| Adobe Monochrome | DSCF7853 | — | 0.03087 |

The baseline renders use the same recipe and camera DCP without the enhanced look layer. Color improves approximately 20%, Neutral 52%, and Portrait 3% on these examples. These results support similar overall looks on the tested X100F images, not exact slider equivalence across cameras. Monochrome mixing, local tones, clarity, saturation, lens corrections and detail processing remain independently implemented. A further RAW was rendered with the imported default Adobe Color as a loading/rendering smoke check.

Private comparison files remain in `target/profile-parity/` and a private local directory. The repository contains no private photos, exported references, or Adobe profile assets. Tests cover import persistence/conflicts, malformed and truncated table payloads, synthetic hue rotation, camera restrictions, default selection, XMP/catalog resolution, embedded-profile round trips, finite output, monochrome neutrality and full-image/region equivalence. The supplied 62 DCPs and all six imported Adobe Raw looks passed private checks.
