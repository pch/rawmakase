# Imported Lightroom camera and look profiles

RAWmakase renders with profiles from its own library and files the user explicitly selects. Proprietary profile assets are not shipped with the application; RAWmakase ships its own profiles instead (see [RAWmakase profiles](#rawmakase-profiles)). On macOS and Windows, the Develop **Profile** menu looks in Camera Raw's profile folder (`/Library/Application Support/Adobe/CameraRaw/CameraProfiles`, `C:\ProgramData\Adobe\CameraRaw\CameraProfiles`) for the current camera's Adobe Standard and Camera Matching profiles, and offers **Import Adobe profiles for this camera** when it finds some that are not imported yet. To decide whether to offer the button it only lists that folder's file names for the current camera; no profile is opened, parsed or copied until the button is used, and then the files are copied like any other import, except that a profile RAWmakase can't read (Camera Raw files HDR Camera Matching profiles alongside the others for newer bodies such as Nikon's Z 8) is skipped and named in the status line instead of failing the rest.

## Import and select

Use **Import profiles…** in the Develop Profile menu or **Import Profiles…** in Preferences. Select one or more `.dcp` / `.xmp` files, `.cube` LUTs (see [.cube LUTs](#cube-luts)), or folders holding them. **Import .cube LUTs…** in the Profile menu and **Import LUTs…** in Preferences import LUTs only, asking first which colour space they expect ([below](#cube-luts)); a folder chosen there gives its `.cube` files and leaves the DCPs and look XMPs beside them, which LUT packs often ship, to **Import profiles from folder…**. For Adobe Color and the other Adobe Raw looks, import both the camera model's **Adobe Standard DCP** and the desired **XMP look profiles**. You can select these together or import the DCP first. Missing dependencies produce an error; another camera's profile is never substituted.

The CLI uses the same importer:

```sh
rawmakase import-profiles '/selected/Fujifilm X100F Adobe Standard.dcp' '/selected/Adobe Color.xmp'
rawmakase import-profiles '/selected/Film LUTs'
rawmakase import-profiles --lut-space prophoto-rgb '/selected/ProPhoto LUTs'
```

It prints a line for each `.cube` and for each file it could not import, and fails when any file was not imported or nothing was found. `--lut-space` (`srgb`, the default, `adobe-rgb`, `display-p3`, `prophoto-rgb` or `rec2020`) is the colour space the run's `.cube` LUTs expect ([.cube LUTs](#cube-luts)); DCPs and XMPs import as they are.

Selected files are validated and copied into `camera-profiles` under RAWmakase's application data directory (`~/Library/Application Support/RAWmakase` on macOS, `$XDG_DATA_HOME/rawmakase` on Linux). An identical reimport is harmless. A different file with the same filename is reported instead of overwritten. The entire selection is validated before copying starts. Source files remain unchanged.

Importing does not change the current edit. Choose the imported look in **Profile**. New unedited photos prefer a compatible imported Adobe Color, then imported Adobe Standard, then a DNG's embedded profile, then RAWmakase Color. Saved edits retain their embedded profile and previous rendering flags, so edits made before RAWmakase Color existed keep LibRaw's camera matrix with the DNG default tone curve (engine 4, see [color-pipeline.md](color-pipeline.md)).

## RAWmakase profiles

Two profiles of our own are listed for every camera with a colour matrix, with no files to import. They follow Adobe's two layers: a per-camera base and a camera-independent look.

- **RAWmakase Standard**: the camera's colour matrix (the decoder's, or the D65 matrix of a DNG's own profile) with the DNG default tone curve. It uses the same colour data as "Default (camera matrix)", which stays in the Profile menu as the entry for no profile and renders through the same matrix and DNG default tone curve.
- **RAWmakase Color**: a look on top of Standard, as Adobe Color is on Adobe Standard. A mild contrast curve and a few smooth hue, saturation and brightness shifts (reds, skin, yellows, foliage, aqua, sky), plus a slight saturation roll-off near white. Neutrals stay neutral. The table is generated from the parameters in `src/camera_profiles/open.rs`, not stored or derived from Adobe data.

Both are embedded in the recipe like any other profile, so later changes to the look don't change existing edits. The look was tuned conservatively and has not yet been compared with Camera Raw renders; measured base profiles (ColorChecker shots per camera) can replace the matrix later without changing any preset.

## Camera Look (fitted)

A profile fitted to one photo's embedded JPEG, the maker's in-camera rendering (`rawmakase_decode::camera_look`, built by `camera_profiles::fitted`). **Profile → Fit Camera Look** fits the open RAW's (a few seconds, off the UI thread) and uses it at 100% as one History step; a profile chosen while it runs wins, and a refusal says why in the status line. `rawmakase fit-look [--report] [--drop-matrix] <raws…>` fits and scores it from the command line and saves nothing.

- **The default for photos without colour data.** A photo with no colour matrix (no camera table row, none in the file or its maker notes), no embedded profile and no imported camera profile for it (`fitted::eligibility` is `Absent`) starts from its fitted look rather than camera RGB through the identity matrix. Every other photo is unchanged: invalid or doubtful matrices, unreadable or foreign embedded profiles and photos the user imported a DCP for (any name) keep today's default, and can still fit one from the menu. `camera_look::with_default` adds the fit to the photo's profile list, and the default picks it only when nothing else applies, so Develop, Reset, the Library, preview builds, Reference View, export, Sync and `rawmakase render` start the same. It is fitted once per file and build: the fit cache (`camera-look/` beside the decode cache) keeps the result, or why there is none, by the file's content identity, `FIT_VERSION` and the build; a cancelled or failed fit is neither used nor kept. One caller fits a file at a time: another for the same file (a virtual copy previewed beside it, or Develop opening it meanwhile) waits for that fit and takes its result from the cache, giving up if its own job is cancelled. Unedited previews follow the profile library's fingerprint, so importing a profile rebuilds them.
- **Saved and moved whole.** The recipe embeds the profile like any other. XMP sidecars and presets write `CameraProfile` (the base) and `Look` as for RAWmakase Color, plus `rawmakase:FittedProfile`: the whole profile, versioned (`v1:`), as zlib in Adobe's base 85, about 100 KB; a JPEG export carries it in Extended XMP. Reading it checks the version, the size (1 MB), its UUID against its content's hash and the camera, and uses it before any profile of the name; a missing, damaged or foreign one is refused and reported, never refitted. Presets keep it under Treatment & Profile and refuse other cameras. Sync and Copy copy the source photo's fit, never a same-named one of the target. Lightroom sees unknown profile and look names and falls back.
- **White balance under a fitted matrix base** is relative to the white as shot (As Shot reads about 6500 K). As Shot, Auto, the selector and Temp and Tint work; the WB menu's named presets are disabled for it. XMP gets the Temp and Tint the base computes, which RAWmakase reads back to the same multipliers; they mean nothing in Lightroom for such a camera.

- **Representation.** A complete `CameraProfile`, built as RAWmakase Color is, so no recipe field is new: a base and the look "Camera Look (fitted)" over it, a 72 × 8 × 8 HSV table (value sRGB-encoded) and a tone curve, with Profile Amount (0–200%). With a usable colour matrix (the camera table's or a DNG's own ColorMatrix, never a DNG's embedded profile) the base is RAWmakase Standard. Without one it is "Camera Matrix (fitted)": a fitted forward matrix, a colour matrix derived so the as-shot neutral reads as D65 (white balance is then uncalibrated: As Shot reads about 6500 K whatever the light), and the ACR3 tone curve. A black and white JPEG gets a curve alone and `monochrome`. The look the camera names for its JPEG (a Fujifilm film simulation, Sony Creative Style, Nikon Picture Control or Canon Picture Style) travels with it and is in its menu title, "Camera Look (fitted): Classic Chrome"; a fit is faithful to that look, so a Neutral or low contrast JPEG makes a flat one. The look UUID is the MD5 of the profile's versioned canonical serialization without it, so a fit never changes; fitting again makes a new profile.
- **Preview.** Only the largest embedded JPEG, at least twice the proxy's 192 pixels, whose colour space is stated: an ICC profile, else EXIF ColorSpace 1 (sRGB), else ColorSpace 0xFFFF with interoperability index R03 (Adobe RGB), else the maker notes' (Nikon's ColorSpace, which their EXIF lacks, and Olympus's CameraSettings ColorSpace). JPEG XL previews and DNGs written by Adobe software (their previews are Adobe's renders) are refused. A preview of another aspect ratio than the photo's default crop is cut centrally to it first: cameras that crop to an aspect ratio embed either the whole frame (Canon) or the crop letterboxed (Panasonic).
- **Pairs.** The photo's stages are taken at four times the proxy's size and reduced in boxes, and its renders, which only judge a look, come from a small copy, at its default crop and orientation, white balance as shot, with the camera's built-in lens correction (which its JPEG has) and no lens profile, whatever the user's defaults. The JPEG is aligned on it (scales 0.97–1.03, and, when those are not enough, radial distortion for a lens the camera corrects in its JPEG and the file has no correction for, shifts of 2 proxy pixels; in high-passed log luminance the centre must correlate at 0.8 and each quadrant at 0.45, or all at 0.85 where the matrix is fitted too; once the lens is searched the scale may reach 0.88–1.12) and box-reduced to the same pixels. Not learned from: pixels the sensor clipped (0.99 of white, before highlight reconstruction, through the same geometry), JPEG channels at the encoding's ends, strong edges (a 3 × 3 step of 0.3 in encoded luminance, so that the small steps of shadows still count) and the corners beyond 0.85 of the half-diagonal. A third of the photo, in whole tiles of an 8 × 8 grid, is held out.
- **Fit.** The tone curve starts from 32-knot quantile matching of the scene's and the JPEG's brightest channel; then table and curve are refined in turn through the profile's own operators (Gaussian kernels of 2.5° hue, 0.086 saturation and 0.12 encoded value, shrinkage towards no change of weight 5, the neutral column unchanged, hue shifts within ±30° and saturation and value scales within e^±0.7, and neighbouring nodes within fixed slopes, or a smooth gradient of skin between two photographed colours shows contours; the nodes nearest grey that little was learned for are smoothed too, or a soft edge between two colours shows bands). The curve's 32 equal-weight bins are also at most 0.06 wide in encoded value, so that a few dark pixels still set the blacks. The base's grey is tinted within 5% to match the camera's grey, which a table can't move. A camera without a matrix first gets its forward matrix from the camera input stage (white-balanced camera RGB before highlight reconstruction, the profile and baseline exposure): constrained ridge least squares that asks each colour only for its direction, with camera neutral going to D50 white; coefficients beyond 8 or a condition number of 10⁶ refuse it.
- **Acceptance.** The full render with the look is scored on the held-out tiles against the render it replaces (Standard, or no profile). A colour look needs a squared error below 0.7 of the baseline's, an RMS of at most 0.1, no cell of a 3 × 3 grid more than 2 ΔE00 worse, at least 256 held-out pairs and 5% of them in colour; a curve alone (or a black and white look) a luminance error below 0.7 of the baseline's and a luminance rank correlation of 0.5. Else the photo is refused with the reason.

Opening and decoding a photo never fits or reads a preview for it: the provenance facts the fit needs (`Metadata::matrix_source`, `embedded_profile_status`) are values the decoders already compute, and the preview's EXIF, its colour conversion, the small renders and the solvers (a few seconds a photo) run only when a fit is asked for. A fitted default for photos without colour data must keep it so: fitted once per file and build, kept in the fit cache, and only for those photos.

Everything is deterministic: the solvers run in a fixed order, the renders per pixel. `--report` also scores the fitted look and an unconstrained reference (the look plus a free correction of its remaining error on a 17³ lattice over the scene colour) on the scene stage directly, which measures what the representation loses, and `--drop-matrix` fits a matrix for a camera that has one and compares the two (the no-matrix trial).

## Rendering

An enhanced XMP profile is resolved against its matching camera DCP. The profile name remains, for example, **Adobe Color**, but the combined rendering contains that camera's matrices and calibration tables plus the XMP look's table and curve. Camera matching is mandatory.

Creative looks (Lightroom's Artistic, B&W, Modern and Vintage groups) name no base profile. RAWmakase puts them over the camera's Adobe Standard when it is imported, as Lightroom does, else over the DNG's own profile, else over RAWmakase Standard. They import without a DCP.

Supported enhanced-profile features:

- Adobe DNG SDK-format HSV big tables, versions 1/2, decoded from Adobe base85 and zlib, with bounded input/output sizes and validation.
- Linear and sRGB value indexing, interpolated hue/saturation/value corrections.
- Adobe RGB tables, 1D and 3D, in sRGB, Adobe RGB, ProPhoto, Display P3 or Rec. 2020 primaries with linear, sRGB, 1.8, 2.2 or Rec. 2020 encoding, described in [RGB tables](#rgb-tables).
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

Lightroom's Artistic, Vintage and most Modern looks, and Adobe's camera-matching XMPs for Nikon, Panasonic, Sigma and others, carry an RGB table (`crs:RGBTable`, a `Table_<md5>` attribute in the DNG SDK's big-table encoding) beside or instead of an HSV look table. A table is a curve per channel (1D) or a grid over all three (3D, up to 64 divisions per side: Camera Raw 18.7 renders a table of 65, 66 or 96 divisions as if the look had none), stored as 16-bit differences from the identity, with the primaries and encoding it works in, whether colours outside it are clipped or extended, and the amounts it may be applied at. RAWmakase interpolates 3D tables tetrahedrally, as the DNG SDK does, on the CPU and in the GPU shader, which agree within 0.0007 of the output range.

Measured with Camera Raw 18.7 on the synthetic chart and synthetic RGB-table looks (`tests/corpus/looks/synthetic-rgb*.xmp`, from formulas in `scripts/corpus/synthetic-looks.py`):

| Question | Camera Raw's answer | Cases |
|---|---|---|
| Where it applies | After the user's tone controls, point curves, HSL and Saturation, before color grading: `rgb-table-point-curve` is 0.87 ΔE00 from Camera Raw with the table there and 2.80 with it at the end of the profile; with Saturation +50 1.78 against 3.75. | `rgb-table-*` |
| Amount | `r + amount·(table(r) − r)` in the table's encoding, at the look's `RGBTableAmount` (1 when absent) times the Profile Amount, clamped to the bounds the table stores. A 0.5 look at 200% renders as the full table at 100%, identical to the 16-bit level; bounds 0.5–1.5 hold 0% at half and 200% at 1.5. Over 100% the blend can leave 0–1; a table that clips its gamut clips the blend there too, per channel (a dark gray the table turns teal loses its red and green at 200%, rather than its hue). At an amount of 0 RAWmakase leaves the table out, so it clips nothing either. | `rgb-table-50/100/200`, `rgb-half-*`, `rgb-bounds-*`, `cube-creative-33-200` |
| Primaries and encoding | Codes 0–4 are sRGB, Adobe RGB, ProPhoto, Display P3 (D65 white) and Rec. 2020; gamma codes 0–4 linear, sRGB, 1.8, 2.2 and Rec. 2020's curve (BT.2020's, as BT.709's: 4.5·v below 0.018, 1.0993·v^0.45 − 0.0993 above). Code 4 was identified on a 1D table halving each channel: on the gray ramp it matches Camera Raw to 0.02 8-bit levels on average (0.05 at most), where the sRGB curve or a 2.2 or 2.4 power leaves 6–11 on average and up to 12–19. Halving fixes where the curve turns linear (between 0.016 and 0.027 linear), not the 4.5 slope below it, which follows from the curve being continuous. Code 4 renders the same with sRGB or Rec. 2020 primaries. | `rgb-srgb-100`, `rgb-linear-100`, `rgb-p3-100`, `rgb-rec2020-100`, `rgb-rec2020-half-100`, `rgb-code4-half-*` |
| 1D tables | A curve per channel, stored like the 3D samples. | `rgb-1d-100` |
| The extra word some camera-matching tables end with | No visible effect at 0 or 1. | `rgb-flag-100` |

The table-only cases sit at mean ΔE00 0.68–0.81 from Camera Raw (RAWmakase's default render is 0.91), and the look combining an HSV table, a curve and an RGB table at 1.0–1.14. Gamut extension (used by Modern 02 only) adds back what clipping into the table's space removed, in its encoding; it was not checked closely, because RAWmakase compressed out-of-gamut colours toward gray where Camera Raw clips them; RAWmakase now clips per channel too ([out-of-gamut colors](color-pipeline.md#out-of-gamut-colors)).

A look's HSV table and curve stay where they were, with the camera profile; only the RGB table goes late. Recipes with an RGB-table look save as version 9, which earlier releases refuse as newer instead of dropping the table; the table is stored in Adobe's encoding, about 180 KB for a 32-division table.

## .cube LUTs

Camera Raw and Lightroom have no LUT stage: Photoshop's Camera Raw **Create Profile** turns a `.cube` into a creative look profile whose RGB table is the cube, and the third-party LUT packs that ship Lightroom profiles beside their cubes store each cube node for node as such a table. RAWmakase does the same. Importing a `.cube` (from the Profile menu, Preferences or `rawmakase import-profiles`, files or folders) writes a look XMP into the profile library; the cube itself is not kept. From then on it is an ordinary look: it is listed in **Profile**, renders through the [RGB table](#rgb-tables) path on the CPU and GPU, takes a Profile Amount of 0–200%, and is embedded in the recipe with its table, so presets, snapshots and History carry it and deleting the library file later does not change an edit. There is no Recipe field or format change for it.

- **Where it applies.** Where Camera Raw applies RGB tables: after the tone controls, point curves, HSL and Saturation, before Color Grading, which therefore applies on top of the LUT. Like other creative looks it names no base profile and goes over the photo's own: Adobe Standard when imported, else a DNG's own profile, else RAWmakase Standard. A LUT made for a log or HDR input is not a look and renders wrongly here, as in Lightroom.
- **Switching it off and strength.** Choose another profile, or set Amount to 0%, which leaves the table out entirely. Amount blends in the table's encoding as for any RGB table, up to 200%.
- **Conversion.** The table is in sRGB primaries and sRGB encoding, colours outside sRGB clipped before the lookup, amount bounds 0–2: what Camera Raw's Create Profile gives a cube by default ([below](#camera-raws-create-profile)). A 3D cube of 2–64 divisions keeps its grid, node for node, where Create Profile samples it onto 32: resampling onto another grid is lossy. A larger cube (65 is common, and Create Profile refuses it) is sampled onto 64 divisions, the most a table holds, by tetrahedral lookup in the cube, as Create Profile resamples; on `creative-65` from `scripts/corpus/synthetic-cubes.py` that is at most 4.9 8-bit levels off (mean 0.12), where every other node (33) would be 10.6 (0.26) and 32 divisions 9.3 (0.35). A cube with a 1D shaper in front of its 3D table is sampled the same way, shaper first, onto 64 divisions. A 1D cube becomes a 1D table, resampled to 4096 entries above that. Outputs outside 0–1 are clamped, as the table holds no more.
- **Colour space.** A LUT expects colours in one colour space, and a `.cube` does not say which: most are made for sRGB. As Create Profile asks for it, the import can be told the space for a batch: sRGB, Adobe RGB (1998), Display P3, ProPhoto RGB or Rec. 2020, in the **Import .cube LUTs** window, which opens at sRGB every time so a choice for one pack never applies to the next, or with `--lut-space` on the CLI. **Import profiles…** and the Setup Assistant import cubes at sRGB. The cube's values become the table's samples unchanged, read as values in that space; only the table's primaries and encoding change (Adobe RGB 2.2, Display P3 sRGB's curve, ProPhoto 1.8, Rec. 2020 its own curve), as Create Profile writes them. Colours outside the chosen space are clipped per channel before the lookup, so with the wider spaces fewer colours clip; the table's output goes back from the space's primaries to the colour stage, where out-of-sRGB colours are handled as any others ([out-of-gamut colors](color-pipeline.md#out-of-gamut-colors)). Amount blends in the space's encoding. The wrong space cannot be detected, so keep sRGB unless the LUT's maker says otherwise. The space is part of the table's bytes, so of the look's UUID: the same cube imported again in the same space is already imported, and in another space is another look, named by the rule under *Names and groups* (**Portra (Cool)** beside **Portra**), with the first kept, as edits and presets name it. An sRGB import is the same look, byte for byte, as before the space could be chosen.
- **Fidelity report.** Each file is measured at import against its cube, in table space, on its nodes and cell centres and the table's, so the report is the same in every colour space: *exact* (within 16-bit rounding), *approximated* with its largest sampled error in 8-bit levels and why (resampled, shaper baked in, outputs clamped), or *refused* with the reason. A baked shaper is always reported as approximated; its breakpoints are also probed along the gray axis. The sampled maximum is not a mathematical error bound. The status line counts approximated and renamed files and names a LUT import's space when it is not sRGB ("Imported 12 LUTs in ProPhoto RGB · 2 approximated · 1 renamed"), and its tooltip lists them; the CLI prints every file, naming the space when it is not sRGB (`imported as look "Portra" in Adobe RGB (1998): exact`).
- **Refused.** Cubes whose `DOMAIN_MIN`/`DOMAIN_MAX` (or input range) is not 0–1, which Create Profile refuses too, cubes whose shaper reads another range than 0–1 (`LUT_1D_INPUT_RANGE`, as cubes for log or scene-linear input have), `LUT_3D_OUTPUT_RANGE`, and files that are not cubes (such as the macOS `._` files zips carry beside them, which folder imports skip). Other LUT formats (`.3dl`, Hald, `.csp`) and zips are not read.
- **Names and groups.** The look is named after the file (`Portra.cube` lists as **Portra**); the cube's `TITLE` becomes its description, as packs often repeat one title across files, and its folder its group. LUT packs often keep each format in a folder of its own ("Cube Files", "LUTs", "For Other Programs"), so the group is the nearest folder, up to four up, whose name says more than that: `Moody Pack/For Other Programs/Cube Files/Dusk.cube` is grouped under **Moody Pack**. The search stops at a standard folder such as Downloads or Desktop and at the user's home folder, so a group never names the user: a cube in only such folders takes its own folder's name, and one right in the home folder has no group. A disk's name can be the group, as packs often ship as disk images named after them. Only the path decides, so the same file always gets the same group. Profile names identify looks in the profile list, in recipes and in XMP sidecars, so a name a different profile already has — another folder's Portra, one of the library's DCPs or looks (an Adobe one included), RAWmakase Standard or Color, or Adobe Standard or Color, which Lightroom edits fall back from by name — is never shadowed: the cube is named **Portra (Cool)** after its group, then **Portra (Cool 2)**, and the import reports the rename. The table's MD5 names the table and the look's UUID, so importing the same cube again is recognised by its table under its name or any name it was renamed to, whatever folder or group it comes from or how its text is written, and reported as already imported.
- **Profile menu.** Looks are listed by their group (`crs:Group`), one submenu per group, as in Lightroom's profile browser: imported LUTs under their folder's group, and other looks, such as Adobe's Artistic or Vintage ones, under theirs. Camera profiles, RAWmakase Standard and Color, looks without a group and Adobe's looks in Lightroom's "Profiles" group (Adobe Color, Vivid and the other Adobe Raw and Camera Matching looks) stay at the top level, by name. The group belongs to the library's file, not the profile embedded in an edit, so recipes are unchanged; a look whose library file is gone keeps rendering, but no longer appears in the menu.
- **Lightroom.** The generated XMP is a standard look profile: installed in Lightroom or Camera Raw it renders there, and sidecars that name it resolve back in RAWmakase by name and UUID. The other way does not work: a profile Lightroom creates from the same cube has a UUID of Adobe's own and does not resolve to RAWmakase's import.

### Film looks

RAWmakase ships no film looks: modelled from datasheets alone, without film shot and scanned to check them, their tones were not right ([#404](https://github.com/pch/rawmakase/pull/404), closed). For a spectral film simulation, bake a cube with [spektrafilm](https://github.com/andreavolpato/spektrafilm) (GPL-3.0, its LUTs CC BY-SA 4.0; run separately, nothing of it is in RAWmakase) and import it like any other. The cube must take sRGB and give sRGB, and its exposure should leave the input's linear values as they are, so that mid gray stays mid gray: `stops_above_midgray = 2.47`, which puts sRGB's 1.0 at 0.18 × 2^2.47. Its default for sRGB is 4 stops, which renders sRGB 0.5 at about 0.76. The `--stops-above-gray` flag of spektrafilm 0.3.4 is ignored, so set it in a spec file:

```toml
# portra400.toml
film_profile = "kodak_portra_400"
print_profiles = ["kodak_portra_endura"]
input_color_space = "sRGB"
output_color_space = "sRGB"
stops_above_midgray = 2.47
```

```bash
spektrafilm-lut build --from portra400.toml --name "Portra 400 Endura" --out ./film-luts
rawmakase import-profiles ./film-luts
```

The look is named after the cube's file (`lut_v034_portra400_portraendura`, so rename the `.cube` first for a shorter name) and grouped under the bundle's folder, **Portra 400 Endura**. Checked with spektrafilm 0.3.4 and Kodak Portra 400 on Portra Endura: the 33³ cube imports exact, and mid gray maps to itself within 0.01. The cube is the film printed on paper (`spektrafilm-lut list film` and `list print` name the stocks), and it applies where every look does, after the tone controls, so the print's own contrast adds to the photo's: in dark or contrasty photos the shadows block up. Lower Contrast or the Profile Amount under it, as with any strong look. Grain, halation and the film's exposure latitude are not in a cube.

### Camera Raw's Create Profile

Measured with Camera Raw 18.7's **Create Profile** (Option-click Create Preset) in Photoshop on the synthetic cubes of `scripts/corpus/synthetic-cubes.py`, its profiles read back with `RgbTable::decode` (the space row from the table's bytes, as the decoder refuses primaries 3 and 4). Errors are against the cube's own tetrahedral lookup, in 8-bit levels, over 400,000 random inputs.

| | Create Profile | RAWmakase |
|---|---|---|
| Table size | The cube's, up to 32: its **Samples** field takes 2–32 and starts at the cube's size or 32. A 1D cube keeps its entries (1024 here). | The cube's, up to 64. |
| 33³ (`creative-33`) | 32 divisions: up to 9.4 off, mean 0.43. | 33: exact. |
| 64³ | 32 divisions: up to 9.1, mean 0.37. | 64: exact. |
| 65³ (`creative-65`) | Refused: "Unable to load the color lookup table. There was an error parsing the file." | 64 divisions: up to 4.9, mean 0.12. |
| Resampling | Tetrahedral lookup in the cube at the new grid's nodes (within 0.007 levels of it). | The same. |
| 1D shaper (`shaper-33`) | Accepted, but the shaper is not applied: up to 255 off, mean 62. | Baked in, 64 divisions: up to 4.7, mean 0.12. |
| `DOMAIN_MIN`/`MAX` | 0–1 accepted; −0.1–1.2 (`domain-33`) refused, as the 65³ cube. | The same. |
| Space | sRGB by default; Adobe RGB, Display P3, ProPhoto RGB or Rec. 2020 on request, written as primaries 1, 3, 2, 4 with encodings 2.2, sRGB, 1.8 and code 4 (sRGB is primaries 0, encoding sRGB). The samples are the same in every space, to 16 bits (`creative-32` in all five): the space changes only the codes after them. | The same five, with the same codes and the same samples. Display P3 and Rec. 2020 profiles made in Camera Raw import as any other. |
| Gamut, amounts | Clip; Min 0%, Amount 100%, Max 200% (bounds 0–2), in every space. For ProPhoto RGB the Gamut menu is disabled, and the table still says Clip. | The same. |
| Flags | `SupportsMonochrome` and `SupportsOutputReferred` true, so Lightroom offers the look for B&W and for JPEG and TIFF photos. | The same. |

### Parity with Camera Raw

Camera Raw photo sidecars store a look's Amount and UUID as child elements. Both
these and the attribute form are read, so 50% and 200% survive XMP import and a
UUID mismatch cannot silently select a same-named look.

The chart cases `cube-*` render the synthetic cubes' looks, as RAWmakase imports them, on the synthetic chart, each look embedded in Camera Raw's sidecar as `synthetic-looks.py`'s are (`tests/corpus/camera-raw/synthetic-d65.json`, checked by `camera_raw_parity_does_not_regress`). Every one sits at mean ΔE00 0.21–0.35 from Camera Raw 18.7, the chart's default render being 0.28: identity, channel swap, hue rotation, posterize, black and white, clamped outputs, a 1D cube, 32, 33 and 65 divisions, Amount 0, 50, 100 and 200%, and Color Grading on top (0.30–0.33). A point curve under the look is 0.47, and Saturation +50 1.28, Saturation's own difference (the same table without a cube, `rgb-table-saturation+50`, is 1.27).

Imported for Adobe RGB and ProPhoto RGB (`cube-*-adobe-rgb-*`, `cube-*-prophoto-rgb-*`): an identity cube in ProPhoto renders as no look (0.28; `cube-identity-33-100` is 0.27), hue rotation in Adobe RGB is 0.28 (0.27 in sRGB), and the creative cube in Adobe RGB 0.37, in ProPhoto 0.41 and at 200% in ProPhoto 0.50 (0.31 and 0.33 in sRGB). On the patches that stay inside sRGB the creative cube is as close as in sRGB (0.25 in Adobe RGB, 0.20 in ProPhoto, 0.25 in sRGB): the difference is in the saturated patches the wider space sends to the edge of sRGB (one channel at 0 or full), where every cube case has its largest errors (hue 315, 300 and 135 at the highest chroma), and of which it has more (237 of 344 in ProPhoto, 145 in sRGB). Imported for Display P3 and Rec. 2020 (`cube-*-display-p3-*`, `cube-*-rec2020-*`): identity cubes render as no look (0.28), and the creative cube is 0.31 in Display P3, 0.34 in Rec. 2020 and 0.40 at 200% in Rec. 2020. Of the table codes, ProPhoto's 1.8 encoding is measured here first; the others are measured on the RGB-table cases above.

One observed chart difference remains: Camera Raw renders a photo whose sidecar has both a look and `ConvertToGrayscale` in colour in the tested cases, and writes no `ConvertToGrayscale` back, while RAWmakase renders it in black and white over the look (`cube-creative-33-bw`, 18.1). This was also observed with non-cube looks.

On 12 CC0 raw.pixls.us photos, four synthetic looks at 100% were also rendered over
Adobe Standard and compared at 1024 px (48 pairs, Camera Raw 18.7). The table reports
the median across photos of each photo's median pixel ΔE00, and its full range.
These are descriptive measurements, not a passed photographic regression gate.

| Synthetic look | Median ΔE00 | Per-photo range |
|---|---:|---:|
| Black and white, 33³ | 0.58 | 0.08–16.11 |
| Creative, 33³ | 1.37 | 0.18–23.84 |
| Creative, 65³ sampled onto 64³ | 1.38 | 0.18–23.85 |
| Hue rotation, 33³ | 1.13 | 0.23–16.33 |

The same photos without a look already differ from Camera Raw by 0.25–17.03
median ΔE00. Subtracting that default distance from the distance with a look is
not an isolated measure of LUT error: a look can amplify or suppress an existing
rendering difference. The photo results therefore do not establish the plan's
no-regression acceptance criterion. The chart covers Amount 0/50/100/200 and the
slider interactions above; the photographic synthetic set covers 100% only.

The photo measurements above are sRGB only; the other spaces are measured on the
chart only. Together with the B&W interaction above, these
are limits on claiming complete Camera Raw parity.

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
