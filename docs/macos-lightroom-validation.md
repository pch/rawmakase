# macOS and Lightroom validation — 2026-09-25

This is a dated record. The opt-in fields and checkboxes it describes (`reference_curves`, `reference_color`, `reference_calibration`, `wide_gamut_curves`, profile tone rendering) went with the earlier engines: every edit now renders with the one current engine ([rendering engine](rendering-quality.md)), and those fields in saved recipes are ignored.

## Camera calibration and point curves — 2026-09-26

The preceding macOS/color work was committed as `76d00e3`. This follow-up adds reference-calibrated camera primary controls and corrects point-curve interpolation, transfer function and RGB processing order. All photo experiments used copied RAWs and the separate `Reference.lrcat`; generated ramps contain no private photo data. The original catalog was not used for experimental edits, and no Lightroom files were deleted.

### Point curves

- Independent natural cubic splines replace the legacy monotone interpolator for new/reference curves. Natural endpoint conditions and overshoot are retained; output is clipped to 0–1. Input endpoints inside 0–255 clip outside the point domain instead of acquiring invented control points.
- The reference path uses the **sRGB transfer function in ProPhoto primaries**, replacing the previous pure-gamma-2.2 approximation. A hue-preserving master operation precedes the independent RGB curves. All four curves use 4097-entry LUTs shared by preview and export.
- The editor snaps new point placement to 0–255 and exposes selected-point Input/Output fields. Parametric controls are ordered Highlights, Lights, Darks, Shadows.
- Missing `reference_curves` and missing curve `natural` fields preserve old rendering. New edits and point-curve imports opt in; existing edits have an explicit checkbox.

Full RAW preview comparison (sRGB MAE, 800-pixel long edge, no fitted alignment/color correction):

| Curve case | Previous MAE | Updated MAE |
|---|---:|---:|
| Linear | 0.02695 | 0.02695 |
| S master | 0.03108 | 0.03040 |
| Independent red/blue | 0.03337 | 0.03070 |
| Custom master + red/blue | 0.03146 | 0.02617 |

To isolate the curve operation, generated 16-bit sRGB ramps were imported into the copied catalog, metadata explicitly read, and full-size 16-bit sRGB TIFFs exported without sharpening. Lightroom Classic 15.5.1 was the reference. Neutral S-curve MAE is 0.0000092; clipped-endpoint neutral MAE is 0.0000074. Independent RGB curves over seven color/gray ramps have MAE 0.0000211; clipped/nonmonotonic RGB curves have MAE 0.0000644. The stored regression samples and reproducible generator are in `tests/data/` and `scripts/make-curve-fixtures.py`.

**Exact parity is not established.** The S master curve still has whole-ramp MAE 0.00240 (peak 0.112 on saturated colors); the clipped master has MAE 0.00333. Neutral/RGB equivalence does not prove the saturated master operation matches. Parametric curves remain approximate, and non-default Curve Refine Saturation remains unsupported and reported during import. RAW baseline/profile differences also remain. No per-photo fitted correction was added to hide those errors.

### Camera calibration

The compact Calibration panel has Shadows/Tint and Red, Green, Blue Primary Hue/Saturation, all −100–100. A precomputed linear color-difference matrix preserves neutral grays; negative saturation uses the inverse positive adjustment. Shadow tint has the measured asymmetric green/magenta response and highlight falloff. All 128 combinations of slider extrema stay finite. These are independent approximations measured on X100F with Adobe Standard, not Adobe's private algorithms or camera-independent validation. Older saved edits keep the legacy operator unless opted in.

Thirteen complete RAW comparisons improved. The table includes the new point-curve path as well as calibration, so improvements cannot all be attributed to calibration alone.

| Adjustment | Previous MAE | Updated MAE |
|---|---:|---:|
| Red Hue +50 | 0.02387 | 0.02363 |
| Red Saturation +50 | 0.02522 | 0.02359 |
| Green Hue +50 | 0.02561 | 0.02384 |
| Green Saturation +50 | 0.02730 | 0.02366 |
| Blue Hue +50 | 0.02759 | 0.02333 |
| Blue Saturation +50 | 0.03090 | 0.02394 |
| Blue Hue −50 | 0.02975 | 0.02373 |
| Red Saturation −50 | 0.03016 | 0.02426 |
| Shadow Tint +50 | 0.02621 | 0.02385 |
| Shadow Tint −50 | 0.02617 | 0.02434 |
| Mixed calibration | 0.03467 | 0.02463 |
| Second photo: mixed | 0.02525 | 0.01736 |
| Second photo: Blue Hue +50 | 0.02283 | 0.01745 |

Private reference TIFFs and before/after renders remain in a private export directory and `target/calibration-parity/`. Numerical synthetic-ramp samples are the only reference image data checked into the repository. Validation: 80 unit tests passed, all 62 supplied DCP profiles passed parse/render/round-trip checks, and strict all-target Clippy passed. Checks include old-recipe compatibility and full-render/region equality with calibration and master/RGB curves active. The native macOS bundle was rebuilt and exercised on a copied RAW, including selected-point numeric editing and blue-primary hue. Numeric point entry applies on commit so Undo does not stop at intermediate typed digits. Lightroom was returned to the original catalog after testing.

## Color controls follow-up — 2026-09-26

The next pass uses 11 full RAW comparisons across two X100F photos, plus isolated shadow, highlight, midtone and luminance exports to distinguish each control's contribution. Reference TIFFs again came only from the copied catalog and copied RAFs. Lightroom was returned to the original catalog after testing, without applying test edits there or deleting Lightroom files.

Changes:

- Color grading interprets hue as an RGB wheel (0 red, 120 green, 240 blue), replacing the incorrect direct use of that number as an Oklab angle. It applies a symmetric, endpoint-preserving tint in gamma-2.2 ProPhoto, with separate shadow/midtone/highlight weights and a finishing global pass. Balance moves the range boundaries; blending adjusts overlap; neither affects the global pass. Luminance uses a bounded brightness-dependent lift before tinting. These are independently measured approximations, not Adobe code or a claim of identical proprietary processing.
- Vibrance protects a broad warm-hue range and limits already saturated colors. HSL luminance response is reduced near black and white. Global saturation retains its previous behavior: the +50/−50 controls were already reasonably close in isolated-response checks.
- Old split-toning XMP records without modern ColorGrade keys set blending to 100 and clear the controls that did not exist in Split Toning. Modern partial grading presets retain omitted values. This follows the [Adobe ACR team's compatibility description](https://blog.adobe.com/en/publish/2020/10/20/introducing-color-grading), which also documents the RGB hue wheel, range overlap and global independence.
- Compact grading rows now display Hue in 0–360 degrees, Saturation in 0–100, Luminance in −100–100, and Blending in 0–100. Hue has a spectrum rail. Global view hides the range-only balance/blending controls.
- `reference_color` is persisted. Missing fields deserialize to false, preserving old RAWmakase edits; **Reference color rendering** in Color grading opts those edits into the new path. New edits and Lightroom imports use it automatically. All-zero color adjustments preserve the previous baseline.

The table compares complete RAW renders against direct Lightroom sRGB TIFFs at an 800-pixel long edge, without image-specific exposure or color fitting. Coefficients were calibrated on isolated controls in the first photo; the second photo was used as a check. This is a narrow two-photo validation, not camera-independent parity.

| Case | Previous MAE | Updated MAE |
|---|---:|---:|
| Saturation +50 | 0.02595 | 0.02595 |
| Saturation −50 | 0.02346 | 0.02346 |
| Vibrance +60 | 0.03329 | 0.02506 |
| Orange/blue HSL | 0.02425 | 0.02336 |
| Blue shadows / warm highlights, blending 50 | 0.02817 | 0.02870 |
| Three-way grading, balance +45 / blending 75 | 0.03085 | 0.02688 |
| Global green, saturation 25 | 0.04256 | 0.02423 |
| Global blue, saturation 50 | 0.06451 | 0.02674 |
| Balance −60 / blending 20, range luminance | 0.03200 | 0.03104 |
| Second photo: vibrance + HSL + split tone | 0.02506 | 0.02510 |
| Second photo: global blue, saturation 50 | 0.06100 | 0.02070 |

Vibrance error falls about 25%; the green/blue global grades improve by approximately 43–66%, and the mixed three-way grade improves by 13%. HSL improves modestly. The basic split-tone case is about 2% worse in full-image MAE, and the second-photo combined edit is effectively unchanged. Remaining baseline color/tone differences limit these comparisons; matching hue labels does not make every slider numerically equivalent to Lightroom. The full before/reference/after contact sheet deliberately includes the difficult cases.

Validation: 74 normal tests pass; 62 supplied private DCPs parse/render/round-trip; strict Clippy passes. Tests cover RGB hue primaries/wrapping, neutral and endpoint behavior, warm-color vibrance protection, balance/blending direction, global independence, legacy recipe fields and split-tone imports, extreme settings, and full-render/region consistency. The final HSL refinement reran the core tests. The release macOS app was rebuilt and exercised with numerical hue/saturation entry, undo and the before shortcut.

Private results: `target/color-parity/final-metrics.json`, `color-parity-comparison.jpg`, `*-final.jpg`, resolved `*-final.json` recipes and all case XMPs. Additional TIFF references remain in the task-created Desktop export folder. Intermediate research trials in the same ignored directory are not final output. RAWs, profiles, reference images and fitted research data are not committed or bundled.

## Parity follow-up — 2026-09-26

Five controlled comparisons now cover three X100F RAWs, custom white balances at 3200 K / −15 and 6500 K / +25, a master point curve, separate red/blue curves, and nonzero exposure, contrast, highlights, shadows and vibrance. All references again came from the isolated catalog and copied photos. The original catalog was restored after testing; no original photo edits or deletions were performed.

Changes retained after comparison:

- DCP ColorMatrix1/2 now drive the DNG Robertson temperature/tint conversion and iterative camera-neutral inversion. Camera white balance can round-trip to the displayed controls; the portrait yields approximately 5056 K / +20.86, consistent with Lightroom's rounded 5050 / +21. Older embedded profiles without these matrices retain the earlier approximation.
- Two Lightroom-generated DNGs (ISO 200 and ISO 400, both DR100) independently report BaselineExposure +0.15 and diagonal CameraCalibration `[0.9883, 1, 1.031]`. These factual camera-reference values are recorded for X100F. The exposure baseline is applied only to the validated DR100 mode and stored separately from the user's Exposure slider in each recipe. No ISO rule or correction for other DR modes is inferred. Camera calibration is used only when the DCP's ProfileCalibrationSignature matches `com.adobe`; other profiles retain identity calibration.
- Imported photo sidecars and catalog records use their resolved temperature/tint values even if the provenance label says As Shot or Auto. An As Shot *preset* still means the camera's native white balance. This fixes the earlier second-photo comparison, whose sidecar retained 5050 / +21 while its native RAW white balance differed.
- New profile-based edits and imported curves use a ProPhoto RGB, gamma-2.2 curve path before output conversion. Its contrast curve preserves black, white and midpoint instead of clipping early with an affine stretch. This is a measured approximation to Lightroom, not a claim that Adobe publishes this exact processing order. The previous sRGB curve path remains available for stored edits.
- Reset, fresh catalog edits, comparisons and normal opens now select the same installed default profile. Loading a saved edit no longer rewrites its stored temperature. The neutral picker updates temperature/tint labels when a calibrated profile is available.

Current measurements use the original sRGB TIFF references directly, converted to 8-bit preview samples and Lanczos-resized to 800 pixels on the long edge. Candidate JPEGs are resized to the same dimensions. There is no exposure/color fitting or geometric registration. The earlier table below used intermediate reference JPEGs, so its last decimals differ. Source active-area differences still limit full-resolution conclusions.

| Case | Previous MAE | Updated MAE | Updated RMSE |
|---|---:|---:|---:|
| Portrait, custom master curve | 0.03313 | 0.01731 | 0.02645 |
| Second photo, nonzero tone sliders | 0.03974 | 0.02248 | 0.03210 |
| Custom WB 3200 K / −15 | 0.03460 | 0.02237 | 0.03450 |
| Custom WB 6500 K / +25 | 0.03321 | 0.02363 | 0.03597 |
| Third photo, independent RGB curves and tone controls | 0.03512 | 0.02930 | 0.04054 |

The portrait improves a further 48% relative to the previous implementation. All five cases improve. The third photo remains the hardest case: bright-window transitions and local shadow/skin contrast still differ. These previews are close in overall color and tone but are not pixel-identical. Lens correction, chromatic aberration, local adjustments and Adobe detail/denoise algorithms remain outside this parity claim. Sony and untested Fuji dynamic-range modes have not been visually validated.

Saved recipes default missing `camera_exposure` to 0 and `wide_gamut_curves` to false, preserving their prior rendering. For an older edit, the profile section offers **Use camera exposure baseline**, and Tone curve offers **Wide-gamut curves**. Re-select an installed profile to acquire its ColorMatrix data for calibrated WB controls. Normal new edits and compatible Lightroom imports use the updated path automatically.

Validation: 67 normal tests pass, all 62 private DCP files parse/render/round-trip, and strict Clippy passes. New checks cover the independent DNG neutral reference, temperature/tint round trips, calibration signature isolation, source-camera/DR restrictions, photo-versus-preset WB semantics, contrast endpoint/monotonicity, legacy fields, and full-image/region consistency with the new curve path. The final native release app was rebuilt and exercised on macOS.

Reproduce a preview measurement (install Pillow and numpy in a virtual environment):

```sh
python scripts/compare-preview.py reference.tif previous.jpg updated.jpg --long-edge 800
```

Private artifacts remain in `target/lightroom-validation/`, including `parity-metrics-2026-09-26.json`, `parity-comparison-2026-09-26.jpg`, resolved recipes and the additional reference exports on Desktop. The temperature code/table is adapted from Adobe DNG SDK `dng_temperature.cpp`, under the same preserved license as the default tone table. [Adobe documents ProPhoto RGB as the Develop preview color space](https://helpx.adobe.com/lightroom-classic/desktop/technical-support/technical-issues/miscellaneous-issues/color-faq.html); precise proprietary slider algorithms remain independently approximated.

## Platform and interaction

Built and ran the native Apple Silicon app with Rust 1.95.0, macOS Homebrew LibRaw 0.22.0, Little CMS 2.17 and libomp 21.1.8. Apple Clang uses the OpenMP preprocessor flag and LLVM runtime; the macOS link path translates LibRaw's advertised C++ runtime to libc++. Linux retains its existing GCC/OpenMP path. Intel macOS and Linux were not executed in this session.

`./packaging/macos/app.sh` creates `target/release/RAWmakase.app`. The local bundle depends on installed native libraries; it is not a signed, standalone installer. App data belongs in `~/Library/Application Support/RAWmakase`, with `RAWMAKASE_DATA_DIR` available as an override.

The inspector now uses compact rows, small slider handles and consistent numeric fields. Basic combines white balance, tone and presence in Lightroom order, followed by Tone curve, Color mixer, Color grading, Detail, Geometry, Effects and Calibration. The photo supports click-to-100% centered on the clicked point, click-to-Fit, and drag-to-pan at 100%. `\` toggles Before, `R` toggles crop, `J` toggles clipping, `F` selects Fit, `Z` toggles Fit/100% (updated for catalog rating shortcuts), and Command+O / Command+Z open / undo on macOS. Double-clicking a slider rail or label resets it. The native app was exercised through computer control; automated coverage also checks inspector width, Before preserving edits/crop, keyboard zoom and point-curve interaction.

## Reference preparation

All Lightroom experiments used a separate SQLite backup catalog, with photo roots redirected to copied RAFs under `target/lightroom-validation/photos`; other roots point to an offline scratch directory. No source photo edits, source sidecar writes or deletion of Lightroom files were performed. The source catalog was not used for experimental changes.

Lightroom exported full-size 16-bit sRGB TIFFs without resizing, output sharpening or watermarking to a private local directory. Private RAWs, catalog, TIFFs, recipes and comparison previews remain outside version control. Supplied archives were read and matching profiles/presets installed in RAWmakase's own data directory: 62 DCPs and 603 Settings XMPs, retaining preset folders. Lens profiles were inspected but no LCP correction engine is implemented.

The two measured Fuji X100F cases were:

- DSCF7845: Adobe Standard, exposure 0, other global tone sliders 0, custom master curve `(0,22), (47,44), (79,83), (133,147), (196,193), (255,223)`; RGB component curves linear. Lightroom's chromatic-aberration correction remained on for this reference; RAWmakase does not implement it.
- DSCF7853: the same profile and custom curve, exposure +0.50, contrast +10, highlights −40, shadows +30, vibrance +10, chromatic-aberration correction off in both comparison recipes. The copied Lightroom sidecar resolved As Shot white balance to 5050 / +21.

RAWmakase recipes were resolved from the exported XMP settings and saved using `render --save-recipe`. Empty modern point-color records and inactive embedded preset provenance can now be read without rejecting otherwise supported photo settings. Actual unsupported active settings still produce errors.

## Tone correction and results

DCP profiles without an explicit tone curve previously received an identity curve. The corrected path uses Adobe's DNG SDK default curve, removes the additional generic shoulder, and applies hue-preserving profile tone interpolation. Newly opened unedited images prefer a matching installed Adobe Standard profile. Existing saved recipes retain their old tone behavior; Profile tone rendering is an explicit opt-in.

Measurements below are mean absolute error and root-mean-square error in encoded sRGB, on a 0–1 scale. Both images were resized to a common preview size (533×800 portrait, 800×533 landscape). There was no fitted exposure or color transform. These are preview measurements, not pixel-registered full-resolution validation: LibRaw and Lightroom expose slightly different active dimensions (3998×5998 versus 4000×6000 for the portrait).

| Reference | Rendering | MAE | RMSE |
|---|---|---:|---:|
| DSCF7845 | Previous profile/shoulder behavior, matching XMP | 0.10249 | 0.13014 |
| DSCF7845 | Corrected default profile tone, matching XMP | 0.03345 | 0.04346 |
| DSCF7853 | Corrected profile tone, nonzero sliders | 0.03968 | 0.05742 |

The portrait's mean absolute difference decreased by approximately 67%. This establishes a substantial improvement on these two references, not general Lightroom parity. Skin warmth and local tonal differences remain visible, especially in the nonzero-slider case. Temperature/tint conversion, Lightroom's local tone operators, demosaicing, detail processing and lens/CA corrections remain independent or unsupported. Sony profiles passed compatibility checks but were not visually matched against Lightroom here. More cameras and lighting conditions need reference testing before claiming numerical slider equivalence.

The normal test suite and strict Clippy checks pass on Apple Silicon. A separate private test parses, renders and round-trips all 62 imported camera profiles. The normal suite includes recipe backward compatibility and modern XMP sentinel handling. Private mixed-camera RAW and long memory-stress tests were not rerun for this change.

## Sources and artifacts

The default tone table comes from Adobe DNG SDK 1.7.1, `dng_render.cpp`, `dng_tone_curve_acr3::Evaluate`. Its copyright and BSD-style license are preserved in `licenses/Adobe-DNG-SDK.txt` and included in the app bundle. See [Adobe's DNG SDK](https://www.adobe.com/go/dng_sdk) and [DNG format documentation](https://helpx.adobe.com/ca/camera-raw/desktop/dng-and-file-formats/digital-negative.html). Private Adobe camera profiles are not redistributed.

Local validation artifacts are under `target/lightroom-validation/`: `rawmakase-legacy-matched.jpg`, `rawmakase-dng-tone.jpg`, `lightroom-reference.jpg`, `7853-rawmakase.jpg`, `7853-lightroom.jpg`, and the corresponding resolved JSON recipes. The `compare` command deliberately rejects unequal source dimensions; the preview metrics above were calculated separately after proportional resizing.
