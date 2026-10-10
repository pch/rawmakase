# Rendering pipeline versions

The description below documents the preserved engine 2 pipeline. Engine 3 is described in [rendering-quality.md](rendering-quality.md), with its stages, camera profiles, preview behavior and version policy.

New photos use engine 4 (schema/pipeline 6). It differs from engine 3 when no camera profile is imported or bundled for the camera: instead of LibRaw's sRGB matrix and a generic scene shoulder, the photo renders like a DNG that carries only a ColorMatrix. LibRaw's XYZ-to-camera matrix (the same Adobe-derived coefficients) becomes a forward matrix adapted to D50, the ACR 3 default tone curve is applied with hue-preserving RGB tone, and white balance temperature/tint are solved through that matrix. The camera exposure baseline applies whether or not a profile is present. On DSCF7853 (X100F) against Lightroom's Adobe Standard export this changes the out-of-the-box result from −1.1 EV / MAE 0.140 to −0.01 EV / MAE 0.030, the same as with the imported Adobe Standard DCP. Saved recipes from earlier pipelines keep engine 3 or lower.

# Color and rendering contract — pipeline version 1

## Native development boundary

The system LibRaw 0.22.2 implementation was inspected against its [versioned source](https://github.com/LibRaw/LibRaw/tree/0.22.2/src/postprocessing). A small C++ subclass overrides only `scale_colors_loop`; LibRaw still performs decoder-specific black subtraction, scaling bookkeeping and demosaicing. `highlight=1`, camera WB, `output_color=0`, linear gamma and disabled automatic brightness/max adjustment prevent ordinary output RGB conversion and exposure normalization from consuming the editing range. Orientation is handled by Rust exactly once.

Before integer scaling, the shim measures an upper bound across all four camera planes and the proposed scale multipliers. It multiplies all scales by `min(1, 60000 / bound)` before delegating to LibRaw. This retains approximately 0.13 stop below the integer ceiling. The result is converted to camera RGB floats using the inverse common factor and the normalized green WB scale. Values above one are retained. Black subtraction is delegated once; the shim does not repeat it.

The bound intentionally uses values before any residual subtraction, making it conservative for nonnegative black levels. Camera-specific decode artifacts, negative black offsets, unavailable camera matrices, or unsupported sensor layouts still require additional validation. Only three-color Bayer/X-Trans development is accepted, plus Canon and Nikon sRAW/mRAW (YCbCr, which LibRaw converts to camera RGB and develops without demosaicing). Source dimensions are checked before copying the native image into a Rust-owned buffer.

Tests feed the actual override synthetic 14-bit ramps at WB ratios 1, 2.5, 8 and 16. No scale-stage pixels reach 65535, and reconstruction error remains below WB / 59000. The two real fixtures also produce zero scale-stage saturation and camera values above one. This proves the integer bridge's tested bounds; it does not prove that clipped sensor information or every demosaicing artifact can be recovered.

Full development uses LibRaw quality 3 (AHD for conventional Bayer and the three-pass X-Trans branch). The optional CLI `--fast` path uses half-size/quality 0. The application shows the embedded JPEG during full development instead of developing twice. LibRaw owns one job context and is limited to four OpenMP threads; Rust processing uses at most eight Rayon threads in the application. Loading and rendering never call LibRaw from the Rayon pool.

## White balance

Development is cached at as-shot WB. Relative camera-space multipliers are applied before the camera matrix in Rust, without rerunning demosaicing. Metadata records both as-shot and daylight multipliers. A Planckian-locus CIE xy approximation, transformed through the inverse camera matrix and daylight normalization, estimates the as-shot temperature. Temperature/tint changes are anchored to this reference so touching a slider does not jump from camera WB to a different model. Stored relative multipliers are authoritative; Kelvin is approximate.

The neutral picker samples a 5 × 5 region in camera space via the same inverse geometry mapping used for rendering. It derives relative gains from that patch. Extreme WB changes can differ from WB-aware re-demosaicing; there is no claim of exact equivalence.

## Rust pipeline

1. Cache demosaiced as-shot camera RGB (`CameraImage`, f32). Keep metadata, source orientation, default inset and development diagnostics with it.
2. Apply relative WB. Use the LibRaw camera-to-linear-sRGB matrix, then a fixed linear-sRGB-to-Rec.2020 matrix. Exposure multiplies by `2^EV`. Negative matrix results and values above one survive this boundary.
3. Basic edge-aware luma/chroma averaging uses a 3 × 3 camera-space neighborhood at original-image scale. The implementation performs this before WB/color conversion. Fit previews omit it; full exports and 100% regions include it.
4. Shadows/highlights/whites/blacks modify luminance with smooth range weights. Version 1 uses global mappings, not spatial/local contrast operators.
5. A luminance-preserving `Y (2.2Y + 0.05) / (Y (2.2Y + 0.6) + 0.1)` tone mapper brings scene luminance into display range. Tone-mapped linear sRGB is transformed to Oklab for saturation, vibrance, smoothly overlapping hue bands and grading. Near the normalized sensor ceiling, chroma is reduced to suppress false clipped-channel colors. This is neutralization, not highlight reconstruction.
6. Bring out-of-sRGB colors into sRGB ([out-of-gamut colors](#out-of-gamut-colors)). Apply the sRGB transfer function, then levels, contrast and an editable shape-preserving cubic master curve in encoded output space. Output is bounded to [0,1].
7. Geometry uses one inverse map: camera default inset, metadata orientation plus quarter-turn, flips, straighten with an inscribed auto-fit, and normalized crop in the oriented/straightened canvas. Rendering samples through this map; global color operators remain shared between full frames and tiles.
8. Resized exports first render the full edited image, then area-average output samples and apply output sharpening. Full-size exports and 100% region rendering apply identical 3 × 3 sharpening with matching boundary halos. The region/full-frame equivalence test includes denoise, sharpening and rotated geometry.
9. Quantize to 8-bit JPEG or 16-bit TIFF at export. Embed an sRGB profile generated by Little CMS, plus camera make/model, orientation 1, ISO, exposure, aperture and focal length. Old thumbnails and maker-note offsets are not copied.

The fit preview area-averages camera data to a maximum 1600-pixel edge before the shared color operations, trading exact spatial/nonlinear equivalence for responsive interaction. It omits detail processing. The histogram describes the whole photo, as that preview, also at 100%; clipping overlays describe what is shown: that preview or the visible 100% region. Fit and export can differ at fine edges; use 100% for detail decisions.

## Out-of-gamut colors

Colors the edit pushes outside sRGB, such as a saturated orange brightened past white or a strong white-balance shift, reach sRGB in one of two ways (`GamutModel` in `develop/pipeline/pixel.rs`, `P_GAMUT_CLIP` in `develop.wgsl`):

- **Clip** (new edits): each linear channel is clipped to 0–1 on its own, as Camera Raw's conversion to sRGB does. A color keeps its in-gamut channels, so a too-bright orange turns toward yellow rather than toward gray.
- **Compress** (edits saved before this model): chroma moves toward the neutral of the same Oklab lightness until every channel fits, which keeps the hue but desaturates.

On the corpus chart (Camera Raw 18.7, 369 cases), clipping lowers the mean ΔE00 from 1.62 to 1.29: the default render 0.91 → 0.66, white balance at 8000 K under illuminant A 4.22 → 0.89 and at 5000 K 2.45 → 0.69, the parametric curve cases 0.76–2.95 → 0.48–1.15, Color Grading 0.75–2.02 → 0.56–1.43, Point Color 0.83–1.82 → 0.69–1.11 and Camera Calibration 0.97–2.21 → 0.71–2.02. Positive Highlights moves further off (+60: 5.27 → 5.89, +100: 7.98 → 8.66), and Dehaze and Shadows +100 by up to 0.15. The recipe keeps the model (`gamut_model`), so edits saved before keep compression and look as they did.

## Display

The renderer returns encoded sRGB. `egui::ColorImage::from_rgb` supplies those bytes to egui's own texture/compositing path; there is no extra application gamma pass. The viewport measures logical-point dimensions times `pixels_per_point` when asking for a 100% region, then paints the result at one texel per physical pixel.

Clipping is judged on the rendered encoded-sRGB values (0–1), before any monitor profile, so the display never changes it. A channel clips in the highlights at 0.999 or above and in the shadows at 0.001 or below (`HIGHLIGHT_CLIP`, `SHADOW_CLIP` in `develop/rendered.rs`; the GPU's `present.wgsl` uses the same values and counts clipped pixels per channel while it counts the histogram). The highlight warning paints red where any channel clips, the shadow warning blue where all three do. Each histogram triangle lights when more than 0.1% of the pixels clip in a channel, in the colours of the clipping channels (white when all three).

Develop's RGB readout under the histogram (and the White Balance loupe) reads these rendered values, never the screen, and shows them as Lightroom does in Melissa RGB: linear sRGB to ProPhoto primaries (the D65-to-D50 matrix the camera profiles use), then the sRGB tone curve.

On macOS, the default display mode explicitly tags the Metal surface as sRGB so
Core Animation matches it to the current monitor, including wide-gamut displays.
`app::display` schedules this declaration in the paint callback, after wgpu can
reconfigure the surface; `platform::display` finds the current Metal layer and
sets its color space. This works around wgpu-hal 30.0.1 leaving sRGB surfaces
untagged ([upstream fix](https://github.com/gfx-rs/wgpu/pull/10286)). No additional
gamma or pixel conversion is applied by this workaround.

Optional monitor ICC conversion maps encoded sRGB bytes to device RGB through
Little CMS with relative-colorimetric intent and black-point compensation.
Choosing this manual override on macOS leaves the Metal surface unmanaged to
avoid applying a second conversion; choosing Use sRGB restores system color
management. Failure of the manual transform still shows the sRGB bytes with a
visible error, but on macOS the surface stays unmanaged while a custom profile
is selected, so that fallback is not matched to the display and can look
oversaturated on wide-gamut monitors. Automatic monitor-profile discovery for
manual conversion and HDR output are outside this release. Native tests check
the actual Metal layer tag, surface reset/replacement and switching manual
conversion on and off; the Little CMS test proves an sRGB-profile round trip
within one byte. Visual matching with other applications still requires
validation on the chosen display.

## Invalidation and ownership

WB/tone/color/crop changes reuse the as-shot development. A single replaceable pending render request coalesces slider activity; results carry generation IDs and obsolete results are discarded. Full-resolution loading is serialized and cancelable through LibRaw's progress callback. Thumbnails are limited to 32 neighbors. Export holds an immutable recipe and the active development; navigation waits for it to finish. No texture contains a full 24 MP image just to display a fit preview.

Sidecars record schema 2 and pipeline 2. Version 1 curves load with their original linear interpolation; editing a curve switches it to smooth interpolation. Unknown versions and changed source identities are preserved and disable autosave for that photo. Legacy sidecars and presets migrate on save; their curve shape remains unchanged until edited.
