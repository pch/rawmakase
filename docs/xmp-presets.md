# XMP presets

The left Presets pane groups installed presets, searches names/groups, remembers favorites, and shows unavailable entries dimmed with an explanation on hover. The optional “Compatible only” filter hides entries that cannot be applied to the current camera. Hover for a temporary preview; click to apply as one undo step. Previewing does not save edits. Import accepts individual XMP files; the supplied archive has already been imported with its directory structure preserved.

User library: `xmp-presets` in the data directory: `$RAWMAKASE_DATA_DIR` when set, else `~/Library/Application Support/RAWmakase` on macOS, `%APPDATA%\RAWmakase` on Windows and `$XDG_DATA_HOME/rawmakase` (default `~/.local/share/rawmakase`) elsewhere. Presets in `$XDG_DATA_HOME/rawmakase/xmp-presets` are read on every platform too. Original XMP files remain unchanged. Favorites are stored separately in the application data directory. The supplied archive contains 926 XMP files: 921 presets/curves and five application preference/cache files excluded from the browser. No Adobe presets or profiles are bundled with RAWmakase; its own built-in presets are described below.

Presets are sparse patches: omitted settings retain the current edit; explicit zero/false resets the corresponding control. Matching camera DCP profiles are resolved by name and camera model. Supported enhanced Look records resolve an explicitly imported XMP profile by name/UUID and camera. See [imported profiles](lightroom-profiles.md). The resulting recipe embeds the selected DCP and enhanced rendering data and stores applied settings, so reopening an edited photo does not require reapplying its XMP. Sidecar/preset schema 4 adds these effects; older recipes load with neutral defaults.

## Built-in presets

RAWmakase ships 30 presets of its own, listed before imported ones in Lightroom's group order: Color, Creative, Film, B&W, Curve, Grain and Vignetting. They are ordinary Lightroom XMP presets in `assets/presets`, MIT-licensed like the rest of the code, embedded in the binary, and read-only in the app. Favorites of a built-in preset follow its UUID, so renaming or regrouping one keeps them.

The Film presets choose one of RAWmakase's [film looks](lightroom-profiles.md#film-looks), always installed, and add the film's grain. Only the Creative looks name a camera profile: Adobe Standard, which is what they were made with. Imported presets need the exact profile they name. Built-in presets, and a photo's own Lightroom edit (from a catalog or photo sidecar), fall back instead, so they work with nothing imported:

- Adobe Standard: the imported Adobe Standard for this camera, else the DNG's embedded profile, else RAWmakase Standard.
- Adobe Color: the imported Adobe Color, else RAWmakase Color.

The hover text and status line say when a fallback is used. Another camera's profile is never used, and one Adobe look never stands in for another. See [RAWmakase profiles](lightroom-profiles.md#rawmakase-profiles).

To add one, drop an `.xmp` into a group folder, list it in `src/presets/builtin.rs`, give it a new `crs:UUID` and `crs:Copyright="RAWmakase contributors, MIT licence"`, and run the preset tests; they check that every file is listed, parses, applies without imported profiles and names no other profile.

## Making presets

Settings › New Develop Preset… (Shift+Cmd+N) saves the open photo's settings as a Lightroom XMP preset, with Lightroom's dialog: a name, a group (typed or picked from the groups presets already use) and the setting groups to include, the same groups Copy Settings lists. The file goes to `User Presets/<group>/<name>.xmp` in the user library (see above), laid out as Lightroom writes presets (`PresetType="Normal"`, a fresh `UUID`, `HasSettings`, the name and group as language alternatives), so Lightroom and Camera Raw can read it; this was checked with ExifTool and RAWmakase's own parser, not yet by loading one into Lightroom. Only the chosen groups' settings are written; spots and masks are not.

A preset made here has Update with Current Settings, Rename… and Delete in its menu. Update keeps its name, group, UUID and the groups it holds. Imported and built-in presets cannot be changed from the app. Save Preset File… and Load Preset File… still write and read RAWmakase's own JSON preset files.

## Preset Amount

After a preset that supports it is applied, an **Amount** slider (0–200%, 100% to start) shows at the top of the Presets panel, under the preset's name, as in Lightroom. It scales the preset's changes: 0% is the photo as it was before the preset, 100% the preset as applied, 200% twice its changes. Each drag is one History step ("Preset Amount"). Applying another preset, any other edit, Undo or moving to another photo ends it, and the slider goes.

The settings before the preset and the preset's result are kept once, when it is applied, and every Amount is worked out from those two again, so dragging back and forth never drifts. Nothing about the Amount is saved: the photo keeps the resulting settings, as Lightroom does. Likewise a Lightroom edit or sidecar is read from its settings alone; the preset it came from and its Amount are never applied again.

A preset offers an Amount when its file says so (`crs:SupportsAmount="True"`, as most of Adobe's do) and it changes nothing that doesn't scale: lens profile corrections, chromatic aberration, crop and straighten, Transform, Upright, spots, masks, and Point Color swatches added or taken away (a preset with Color Adjustments clears a photo's swatches, so it has no Amount on a photo that has some). Adobe's own presets with lens or chromatic aberration corrections don't offer one either. Built-in presets offer one, except Curve › Linear, which resets the curve as Adobe's “None” presets do. A preset made with New Develop Preset… offers one when none of those groups is among its settings.

Adobe doesn't document how each setting scales, and Camera Raw offers the slider only in its own window rather than through the settings a script can hand it, so RAWmakase's rule is not yet measured against Lightroom:

- Sliders move in a straight line from their value before the preset to the preset's, continuing past it above 100%, within the slider's range.
- Point Color swatches the photo already has scale their Hue, Saturation and Luminance shifts.
- Temperature moves evenly in mireds, as its slider does; Tint in a straight line.
- Point curves blend their outputs at the points of both curves. When the curve before is straight, as it usually is, that is exactly the blended curve.
- Color Grading hues take the shorter way round the wheel, or the preset's hue straight away where there was no saturation before.
- Settings that aren't numbers (profile, Color or Black & White, panel switches, vignette style, grain seed) take the preset's choice at any Amount above 0, so every Amount keeps the preset's character. A creative look with a [Profile Amount](lightroom-profiles.md#profile-amount) the photo didn't have fades in from 0 instead; the same look already chosen moves from its Amount before.
- The process version follows the preset at every Amount, as applying it does.

## Raw defaults

Preferences › Profiles & Presets › Raw Defaults works like Lightroom Classic's Preferences › Presets › Raw Defaults. The Master choice is one of:

- **Adobe Default** (out of the box): Adobe Color, else Adobe Standard, else a DNG's embedded profile, else RAWmakase Color, with the camera's own white balance and lens defaults.
- **Camera Settings**: the same, with the camera-matching profile for the camera's standard look (Camera Standard; Camera PROVIA/Standard on Fujifilm) from the profiles imported into RAWmakase. Lightroom also follows the picture style set in the camera; RAWmakase does not read it, so it always starts from the standard one. Nothing is read from Lightroom or Camera Raw; without that profile imported for the camera, the photo gets Adobe Default and the status line says so.
- **RAWmakase Default**: the same, with RAWmakase Color wherever it fits the camera, even when Adobe profiles are imported.
- **Any Develop preset**, applied over Adobe Default as a click in the Presets panel would be.

“Override global setting for specific camera” gives a camera its own choice: pick the camera (those in the open catalog, and the open photo's), its default, then Create Default (or Update Default). A camera's choice beats the Master; turning the checkbox off keeps the cameras' choices without using them. Cameras are named as Lightroom names them and match by model, so a choice made for “ILCE-7CR” applies to a Sony ILCE-7CR whichever way the catalog names it. As in current Lightroom Classic there are no ISO- or serial-number-specific defaults.

They apply to a photo with no RAWmakase edit and no Lightroom edit when it opens (and when the Library renders it for Compare or Survey, or Sync starts from it), and to Reset. A Lightroom edit, its history steps and Snapshots always convert from Adobe Default, because that is what Lightroom stores them relative to. A saved edit is never rewritten; Reset brings it to the current defaults. If the chosen preset is gone or does not apply to the photo (another camera's profile, say), or Camera Settings has no camera-matching profile for it, the photo gets Adobe Default and the status line says why; the photo still opens. Auto Tone or Auto white balance in a default preset is not measured on the photo, so a photo looks the same opened, reset or previewed; those stages keep Adobe Default's values. For the same reason a default preset's Upright mode is left off, since it would need analysing on each photo.

One difference from Lightroom: Lightroom writes the defaults into every photo at import, so changing them later affects only new imports and Reset. RAWmakase saves nothing for a photo until it is edited, so photos without an edit follow the current defaults, and changing them updates the open photo and the Library's previews of unedited photos. The choices are kept with the session, by preset id (a built-in preset's UUID or an installed file's path) and name.

## Implemented settings

- White balance, exposure, contrast, highlights, shadows, whites, blacks, saturation and vibrance.
- Master and RGB point curves with Refine Saturation, parametric curves and region boundaries.
- Eight-band HSL, black-and-white mixing, split toning and modern color grading.
- Primary calibration and shadow tint, clarity, texture and dehaze.
- Sharpening, luminance/chroma noise controls and hue-based defringing.
- Grain, post-crop vignette and manual lens vignette.
- Lens profile corrections (Enable Profile Corrections with its Distortion and Vignetting amounts), manual Distortion, Remove Chromatic Aberration, Transform sliders and Upright. See [lens corrections](lens-corrections.md) and [transform](transform.md).
- Declared crop/straighten and automatic tone/white-balance estimates.

These use RAWmakase's rendering algorithms. Adobe's proprietary operators are not reproduced exactly; the same numerical preset values are not evidence of identical Lightroom pixels. Auto adjustments, calibration, local contrast, noise reduction, grain, vignette and defringe are independent implementations. Post-crop vignette styles follow Lightroom's codes (1 Highlight Priority, 2 Color Priority, 3 Paint Overlay; 0 or none reads as Highlight Priority). Highlight Priority renders as an exposure change; Color Priority and Paint Overlay share one blend toward black or white until Color Priority has its own operator. Validate critical appearance against Lightroom exports when available.

## Compatibility checks

Spot removal (`RetouchAreas`, `RetouchInfo`) and brush, gradient, radial and luminance-range masks convert to RAWmakase's experimental spots and masks ([masking](masking.md)); AI and color-range masks are reported. Point Color swatches (`PointColors` with `ColorVariance`) import from presets, sidecars and catalogs, and presets with the Color group carry them ([color mixer](color-mixer.md#point-color)). Unknown active settings, missing DCPs or imported enhanced-profile dependencies, Glow and Reshape mark a preset unavailable, with the reason on hover. A look's Profile Amount applies when the look supports one ([Profile Amount](lightroom-profiles.md#profile-amount)). Lens-profile correction, Transform and Upright apply (as of 2026-10-03): an Upright mode without Lightroom's stored correction is analysed on each photo, except Guided, which needs the stored one. They render only at process version 4: on an edit at an older version (Calibration) they are stored but have no effect, and an active Remove Chromatic Aberration skips the whole geometry group. Unavailable entries stay visible by default and can still be clicked: as in Lightroom, each group of settings (profile, basic tone, geometry, white balance and so on) that applies does, the others keep the photo's current values, and the status line lists what was skipped. Applying any preset, even in part, also moves an edit older than process version 3 to version 3 with profile tone and reference color on, so an old edit can change appearance beyond the settings applied. Hover previews do the same. A camera-specific Sony preset is not substituted with an unrelated Fuji profile.

All 921 supplied presets parse. When counted on 2026-09-26, 785 were applicable to Sony ILCE-7M2 and 534 to Fujifilm X100F; these sets overlap. Presets with lens-profile correction or perspective settings were rejected then and apply now, so these counts are out of date until the audit is rerun on the same archive. Most unavailable entries reference profiles for the other camera or dependencies absent from the archive. Complete Adobe XMP/rendering parity is not implemented.

The parser uses XML namespaces, supports RDF names/groups and both attribute and scalar-element settings. It narrowly repairs a repeated closing Group tag found in some supplied VSCO files in memory and reports that repair. Curve endpoints missing from older preset curves are extended constantly. Malformed numbers, invalid curves and out-of-range values are rejected.

## CLI and verification

```sh
rawmakase render photo.ARW output.jpg --xmp /path/to/preset.xmp --max-edge 2400
cargo test --test xmp_presets -- --ignored --nocapture
```

The private audit requires the installed library. Unit tests cover sparse application, explicit resets, namespace aliases, RGB curves, missing-dependency refusal, malformed input, serialization and full-frame/tile agreement with spatial effects.
