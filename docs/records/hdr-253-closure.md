# HDR editing (#253): completion

Status: Current, 2026-11-12. How [HDR editing (#253)](https://github.com/pch/rawmakase/issues/253)
meets the [plan's completion criteria](hdr-implementation-plan.md#validation-and-completion-criteria),
what stays refused and why, and what is left for later. The rendering contract is
[HDR](../hdr.md); measurements and decisions are in [HDR results](hdr-stage1-results.md).

## The criteria

The plan: "#253 is complete only when HDR edits persist, render on supported HDR
displays, have a useful SDR fallback and survive reopening. … A macOS-first display
release must state Windows/Linux display coverage explicitly."

- **HDR edits persist.**
  - Settings: Lightroom's `crs:HDREditMode`, `HDRMaxValue` and the seven `SDR*` sliders
    (`model::hdr`).
  - XMP: read and written, through Camera Raw presets and the High Dynamic Range group
    (`hdr_settings_are_kept_while_off_and_applied_on`).
  - Lightroom catalogs: imported (`lightroom_15_controls_at_rest_are_not_reported`).
  - Native presets: `a_preset_keeps_hdr_values_and_saves_hdr_on_in_its_schema`.
  - Copy/Sync: as one group.
  - The catalog: saved in schema 14 and catalog version 6, which earlier releases
    refuse rather than misread (`releases_before_hdr_refuse_its_format`).
- **They survive reopening.** An HDR edit with masks, the SDR rendition's sliders and a
  curve reads back exactly as saved after the catalog is closed and opened again. This
  holds whether it is stored as the photo's edit, in a virtual copy, in a History state
  only or in a snapshot only (`an_hdr_edit_survives_reopening_wherever_it_is_stored`).
  Rendering is a function of the edit and the file, so the master and the SDR rendition
  render again as before.
- **They render on supported HDR displays.** On macOS, Develop shows the HDR master on
  an EDR display:
  - its views: Fit, zoomed out, 100% regions, Before Only, Before/After and Reference
    View;
  - on the GPU during slider drags, checked against the CPU master in float space;
  - with an HDR histogram, readout and beyond-headroom marking.

  Light above white was checked on a MacBook Pro XDR display.
- **Windows and Linux coverage is stated** ([Display](../hdr.md#display)):
  - Windows (Advanced Color, scRGB) and Wayland (the colour-management protocol) have
    display paths. They are tested in code, and Windows also in a virtual machine
    without an HDR display. Neither has been seen on a real HDR display.
  - X11 shows the SDR rendition, and the Basic panel says the display shows no HDR.
- **They have a useful SDR fallback.**
  - Every SDR view and output renders the edit's SDR rendition: SDR displays, the
    Library, thumbnails and SDR exports (`an_sdr_export_of_an_hdr_edit_is_its_sdr_rendition`).
  - Preview for SDR display shows the SDR rendition on an HDR display.
  - Where the master is refused, the edit stays valid with HDR on, and the Basic panel
    says why.

HDR export, the other half of the plan, shipped with it:

- HDR TIFF (PQ, HLG, linear float).
- JPEG gain maps.
- JPEG XL PQ, HLG and gain maps. #894 closed after independent interoperability checks.

Opening HDR files is part of the same work: PQ and HLG TIFF, PNG, JPEG XL, AVIF and
HEIC, and ISO 21496-1, Adobe `hdrgm` and Apple gain maps.

## What the master renders

Every control of the SDR pipeline renders in the HDR master except those listed under
[What stays refused](#what-stays-refused), each measured against Camera Raw 18.7's HDR
output:

- **Tone:** Exposure, Contrast, Shadows, Whites up to +50, Blacks, Clarity −50 to +50,
  Dehaze −50 to +100, Texture.
- **Curves:** the master, channel and parametric curves, and Levels.
- **Colour:** Saturation, Vibrance, the colour mixer (Luminance −100 to +40), Black &
  White, Calibration, Point Color and Color Grading.
- **Profiles and looks:**
  - camera profiles, HDR profiles among them;
  - ordinary profiles' sRGB-encoded tables, the 2,437 camera-matching profiles
    installed with Camera Raw;
  - looks: tables, curves, RGB tables, B&W, Amount and their own sliders, fitted Camera
    Looks included.
- **Effects and local work:**
  - the post-crop vignette and grain;
  - masks with Temp, Tint, Exposure, Contrast, Shadows, Texture, Color, Sharpness and
    Noise;
  - Red Eye;
  - everything that works on the camera's samples: lens corrections, geometry, noise
    reduction, AI Denoise, Heal and Clone.

Since 2026-11-11 a control the master refuses renders only when RAWmakase's HDR error
against Camera Raw is no worse than its SDR error, for the same control on the same
photos or probes ([Decisions](hdr-stage1-results.md#decisions)).

## What stays refused

The master, not the edit, is refused. The edit keeps HDR on and its SDR rendition
renders everything. Each refusal has Camera Raw measurements behind it:

| Control | Why |
| --- | --- |
| Highlights | Camera Raw's HDR Highlights levels off or fades above white; held rules miss by 0.2–0.4 stop ([Highlights and Shadows](hdr-stage1-results.md#highlights-and-shadows)) |
| Whites above +50 | No photo level reaches Camera Raw's render: 0.17–0.52 stop off on 4 of 12 photos at its own best level; on 24 photos the master is further from Camera Raw than the SDR rendition, below white too (+75: 0.039 / 0.146 stop against 0.027 / 0.085) ([Whites](hdr-stage1-results.md#whites)) |
| Clarity beyond ±50, Dehaze below −50 | The master is further from Camera Raw than the SDR rendition (Clarity +75 and +100, Dehaze −70 and −100 measured on 12 photos; [Presence](hdr-stage1-results.md#presence)) |
| The colour mixer's Luminance above +40 | Camera Raw's HDR effect strengthens with brightness where its SDR one rolls off ([Band Luminance](hdr-stage1-results.md#band-luminance)) |
| Defringe with Black & White | The master's Defringe follows the gray mix, where it changes nothing; SDR's precedes it ([Defringe](hdr-stage1-results.md#defringe)). Defringe itself renders (#1048) |
| Refine Saturation other than 100% | Adds colour error in HDR (+1.5 to +2.3 ΔE_ITP) where SDR's removes it ([Tone curves](hdr-stage1-results.md#tone-curves)) |
| A point curve that ends left of white | Camera Raw continues it past its last point, within 0.03 stop up to 16 times white, then holds steep curves back (0.82 stop off above white) ([Tone curves](hdr-stage1-results.md#tone-curves)) |
| A darkening vignette's Highlights | Camera Raw protects only from four times white up, in proportion to Highlights; one synthetic probe, no photos ([Vignette and grain](hdr-stage1-results.md#vignette-and-grain)) |
| A mask's Whites, Blacks, Hue, Saturation, Highlights, Clarity, Dehaze | Camera Raw's HDR mask operators differ from SDR's above white, or RAWmakase's master is further off than its SDR rendition ([Masks](hdr-stage1-results.md#masks)) |
| A look that sets a refused slider (Adobe Landscape) | As the slider it sets; a look's split toning renders since 2026-11-12 ([Looks](hdr-stage1-results.md#looks)) |
| An HDR profile's sRGB-encoded tables (27 installed) | How Camera Raw combines overrange encoding with them is not measured ([sRGB-encoded tables](hdr-stage1-results.md#srgb-encoded-tables)) |
| Two profile curves unlike the family (Fujifilm HS20EXR, HS30EXR) | The fitted extension is 0.013 stop off below 0.9 against SDR's 0.005 ([Tone curves](hdr-stage1-results.md#tone-curves)) |

## Known limitations

- **The CPU renders the master** of an edit with masks, Point Color, the vignette or
  grain.
- **HDR HEIC:** opens on macOS and Linux, not on Windows. HEIC gain maps open on macOS
  only.
- **Camera Raw's own SDR rendition** is a local tone map of the master. RAWmakase's is
  the edit with HDR off, with the SDR rendition sliders rendered as one measured curve
  ([SDR rendition](hdr-stage1-results.md#sdr-rendition)).

## Follow-ups

- Highlights in HDR, from an HDR-trained rule on many photos.
- Whites above +50.
- Camera Raw's SDR rendition tone map.
- The mask operators' own SDR parity (Saturation, Whites, Blacks), after which their
  HDR rendering can be measured again.
- HDR display checks on real Windows and Wayland HDR displays.
