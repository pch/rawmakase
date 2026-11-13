# HDR editing and rendering

Status: Current. What an edit with High Dynamic Range on renders, which parts of
the pipeline are measured against Camera Raw in HDR, and what is refused. The plan
and its progress are [HDR implementation plan](records/hdr-implementation-plan.md)
and [HDR results](records/hdr-stage1-results.md), and how #253 meets the plan's
completion criteria is [HDR completion](records/hdr-253-closure.md); the settings themselves are
`model::hdr` (Lightroom's `crs:HDREditMode`, `HDRMaxValue` and `SDR*`).

## Two renderings of one edit

An edit with HDR on has two renderings:

- **The HDR master** (`develop::render_hdr`): linear ProPhoto RGB, 1 = graphics
  white, nothing clipped above white, no channel above 2^HDR limit
  (`rendered::HdrImage`). The CPU renders it, without the stage cache: HDR outputs
  encode it, and conversion to nits and to PQ or HLG belongs to them. The GPU port
  renders a preview of it for the window ([GPU preview](#gpu-preview)), checked
  against the CPU's; the CPU stays the reference.
- **The SDR rendition**: the same edit with HDR off. Every SDR view and output
  renders it: Develop, Reference View, the Library's previews and thumbnails, SDR
  exports, and the GPU's SDR previews (`Recipe::resolved` turns HDR off;
  `Recipe::resolved_hdr` keeps it for the master).

RAWmakase defines the SDR rendition as the HDR-off render, which below 0.5 agrees with
the HDR master as closely as Camera Raw's two renders do. Camera Raw's own is not
that: the SDR base of its gain-map JPEG is a local tone map of the HDR master, which
departs from its HDR-off render by a median 0.2 stop on a photo
([record](records/hdr-stage1-results.md#sdr-rendition)). Its SDR rendition sliders
(`SDRBrightness` and the others) adjust that tone map and never change the master
(Camera Raw 18.7's is bit-identical with each at ±50). RAWmakase keeps them in the edit
and renders the master with them, as without. Its SDR rendition renders the change they
make to Camera Raw's, measured as one curve of luminance (`develop::sdr_rendition`),
within the ranges where that change is the same on every photo measured: Brightness
−50 to +100, Contrast −100 to +50, Highlights 0 to +100, Shadows −100 to 0 and Whites
−100 to +50. With any slider beyond them, or Clarity (a local operator) or Highlight
Saturation (`SDRBlend`) set, none renders, and the Basic panel's note says which
([record](records/hdr-stage1-results.md#sdr-rendition)). Values set while HDR is off
render nothing, as in Camera Raw. Range masks and Auto see the edit without them, so they
change neither what a mask selects nor the master; while a mask's overlay shows, and
for the range pickers, Develop renders the rendition without them, so the overlay
shows that selection. These edits' SDR previews render on the CPU.

Develop shows the HDR master on a display with headroom ([Display](#display)) and
the SDR rendition everywhere else, and says which under the Basic panel's HDR switch,
with the reason when the master can't render.

## Display

**Coverage.** Light above SDR white has been seen on macOS only, on a MacBook Pro XDR
display. Windows (Advanced Color, scRGB) and Linux under Wayland (the color-management
protocol) have HDR display paths, tested in code and, for Windows, in a virtual
machine without an HDR display; neither has been seen on a real HDR display. Linux
under X11 has no HDR display path: an HDR edit shows its SDR rendition there, and the
Basic panel's note says the display shows no HDR. On every platform an HDR edit's
master renders for HDR export and its SDR rendition for SDR views and outputs, with
AI Denoise in both.

On macOS the window's surface is 16-bit float (`egui_wgpu::WgpuConfiguration`'s
`surface_format_selector`, from RAWmakase's egui fork until it is upstream), and its
Core Animation layer is tagged extended sRGB, so values above 1 are light above SDR
white. The interface keeps writing the same sRGB-encoded values it wrote before, so
it stays at SDR white. A display can show HDR when its potential headroom
(`maximumPotentialExtendedDynamicRangeColorComponentValue`) is above 1.05. The
window asks for EDR (`wantsExtendedDynamicRangeContent`) only while it shows an HDR
photo; until it does, macOS reports no current headroom.

On Linux under Wayland, the window's surface is 16-bit float only when an output
already shows HDR as the window opens (an output's image description, from the
color-management protocol, gives it headroom); an SDR display keeps the 8-bit
surface, and a display that shows HDR only later says HDR shows after RAWmakase
restarts. Wayland has no gamma-encoded extended sRGB, so the float surface is
linear extended sRGB (wgpu's `ExtendedSrgbLinear`), which Mesa's Vulkan WSI declares
to the compositor itself, with no luminances. Mesa holds the surface's colour
management object for the swapchain's whole life, so RAWmakase can't declare
anything of its own: a second one is a protocol error that ends the connection.
egui still renders the same sRGB-encoded values, and the egui fork's last pass
decodes them to linear (`egui_wgpu::WgpuConfiguration::linear_float_surface`), so
the HDR preview needs no other encoding.

Where a compositor shows that surface's 1.0 decides the headroom and the black
level (`platform::display::wayland::Anchor`). Hyprland 0.56 (by its source) reads an
undescribed linear surface as Windows' scRGB: 1.0 is 80 cd/m², absolute, on any
output, and 0 is no light. Its sRGB windows' white is the monitor's
`sdr_max_luminance`, 80 cd/m² unless set, so the interface matches them then; the
headroom is the display's target luminance over 80 cd/m², not over the 203 cd/m²
reference its HDR outputs describe; and the last pass lifts black to the output's
minimum over 80 cd/m², or the interface's dark greys sink below its sRGB windows'
(measured on an SDR output, the interface then matches the 8-bit surface to one code
value). Other compositors are assumed to anchor 1.0 to the output's reference
white, as the protocol's default luminances have it. A PQ output that names no
target luminance (nor a maximum of its own) shows no HDR. The headroom is the
window's surface's preferred image description, read on winit's own connection and
read again when the compositor changes it (a move to another output, HDR turned on
or off). A Hyprland `sdr_max_luminance` or `sdrbrightness` other than its default
isn't visible through the protocol and is not followed. Light above SDR white on a
Wayland HDR display has not been seen yet: no HDR display was available.

On Windows the surface is 16-bit float when a display can show HDR as the window
opens, its HDR switch on or not (`DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO_2`'s
`highDynamicRangeSupported`, or before Windows 11 24H2 `advancedColorSupported`); a
PC with no such display keeps the 8-bit surface, and one connected later says HDR
shows after RAWmakase restarts. The float surface is scRGB (wgpu's
`ExtendedSrgbLinear`, DXGI's `RGB_FULL_G10_NONE_P709`), decoded to linear by the same
last pass as on Wayland. With HDR on, scRGB's 1.0 is 80 cd/m², not the user's SDR
white, so the last pass multiplies by the SDR content brightness over 80 cd/m²
(`DISPLAYCONFIG_SDR_WHITE_LEVEL`) and the interface stays at SDR white; with HDR off,
Windows shows 1.0 as the display's white and nothing is scaled. HDR is on when the
display's DXGI output is in BT.2100 PQ (`IDXGIOutput6::GetDesc1`, as Microsoft's
guidance has it), and the headroom is that output's peak luminance over the SDR white.
A display that can show HDR with its switch off says HDR is off in Windows' display
settings. A backend that presents a float surface only as plain sRGB (wgpu's OpenGL,
which RAWmakase falls back to where DirectX 12 and Vulkan don't start, or a Vulkan
driver without extended sRGB) says the graphics backend shows no HDR, on any
platform but macOS. Windows changes the switch and the SDR content brightness without a
message winit passes on, so with a float surface the window reads them again every
two seconds and when it reaches another display (a read costs about a millisecond).
Measured in a virtual machine whose display shows no HDR: the interface on the scRGB
surface matches the 8-bit surface to two code values, and with SDR white and the
headroom forced, the HDR preview's light above white keeps its ratio to the
interface's white. Light above SDR white on a real Windows HDR display has not been
seen yet.

While the display can show HDR, an edit with HDR on whose master renders is shown
as its master in Develop: in Fit, zoomed out, and
zoomed in to 100% or more, and in Before Only and on each side of Before/After and
Reference View, where each side shows its own settings' master or SDR rendition.

Where the GPU renders the master ([GPU preview](#gpu-preview)), the render worker
presents it with each SDR frame of the same view, drafts during slider drags
included: a render job asks for it at the headroom due (`RenderJob::hdr`), and the
viewport draws it over the SDR picture it came with while the master is still due at
that headroom. The status line then says `GPU HDR`. A headroom that moves by a tenth
of a stop, or a master that becomes due or stops being due, renders the view again
(`Editor::follow_hdr_display`; Before's job compares its own).

Elsewhere, and until the GPU's frame is there, the CPU master shows
(`app::develop::hdr_preview`). A
whole photo is rendered on the CPU from a reduced copy of it at the size shown
(`develop::render_hdr_preview`); a
100% region (the part of the photo the SDR rendition renders at 100%, with its
margin) from the full photo, with the margin sharpening reads around it, so it is
that part of the full-size master (`develop::render_hdr_region`). AI Denoise blends
in the result the SDR rendition shows, partial or final, reduced with the photo for
a whole photo; each result renders the master again. The master is
converted to linear sRGB primaries, with negative values kept, as extended sRGB
keeps them. Each channel is rolled off to the display's current headroom with the
HDR limit's roll-off and sRGB-encoded into a half-float texture.

Each place a master can show is a slot: the edit's whole photo and region, and the
other side's. One worker renders them, the latest request of each slot only, the
edit's region first; asking a slot for something else (a pan, an edit, a new
headroom) cancels its render under way, and a result for anything but the slot's
latest request is dropped. A preview is drawn only while it matches the photo, the
settings, the part of the photo and the headroom (to a tenth of a stop) it was
rendered for; until then the SDR rendition stays. Zoomed in, the whole photo's
master shows under the region only with the region's, so a region still rendering
shows the SDR rendition's sharp region rather than a blurred master. A preview of a
view the frame didn't draw (a region after zooming out, a comparison closed, a
reference photo removed) is freed and its render cancelled when the frame ends; only
the edit's whole photo stays, to show again in Fit. While a master is due, the window reads the headroom again every two seconds,
since macOS changes it with the display's brightness without waking the window.
Display changes are view changes, never edits.

**Preview for SDR display**, under the Basic panel's HDR switch while HDR is on, shows
the SDR rendition instead, as Lightroom's does: on every side of the viewport, with the
SDR rendition's histogram and readout, and without asking the display for EDR. It is a
view choice for every photo while RAWmakase runs (`ViewState::sdr_preview`), not part of
the edit: Camera Raw 18.7 writes no setting for it, and turning it on or off adds no
history step. An edit's SDR rendition settings (`crs:SDR*`, from Lightroom or Camera Raw)
are kept; the SDR rendition renders them within their measured ranges
([Two renderings of one edit](#two-renderings-of-one-edit)).

Beside the edit, the Basic panel's note also says what the other side shows, named
(Before or Reference), so a comparison of brightness says when one side is SDR.
Everything else shows the SDR rendition, and the Basic panel's note says why: the
camera's thumbnail, visualizations drawn into the render (the clipping warning,
Remove's spots), a monitor profile chosen in RAWmakase (it converts the SDR rendition
into device RGB, which has no extended range), the Library, other platforms, an SDR
display, and an 8-bit surface (when wgpu offers no float format). A failed render is
that edit's alone: its reason shows until the edit changes. The window asks for EDR
as long as a master is due, including while an edit's next preview renders, so the
display's headroom doesn't ramp down and up with every slider move. Where only the
CPU renders the master, slider drags wait for it, and the SDR rendition shows
meanwhile.

### GPU preview

`PreviewRenderer::render_hdr_to` (and `render_hdr_region_preview_to` for a 100%
drag preview) presents an HDR master's preview on the GPU, from the SDR rendition's
own samples, pyramid and caches, into a half-float texture of the view's HDR slot.
The develop pass's HDR variant (`develop.wgsl`, `V_HDR`) renders the scene tone
stage's HDR Whites and Blacks and the profile's finish (`CameraProfile::finish_hdr`):
an ordinary profile's look table and HDR tone curve; an HDR profile's hue and look
tables on overrange-encoded values and its curve on them, as the DNG SDK applies
them; and a look's table (on overrange-encoded values) and curve (`hdr_look`, on
its own encoding), then Contrast (`basic_tone::HdrContrast`'s table, top and slope)
and the tone curves (`HdrCurves`: Levels, the parametric curve and the master point
curve on `hdr_look`'s encoding, from the SDR port's own samples of the curves, then
each channel's curve, held at 16), and
the colour stage (the colour mixer's grids looked up as `color_mixer::Lookup::Hdr`
looks them up, with Vibrance's fading value factors, band Luminance per channel by the
scene's level and Saturation's fade to gray,
then a look's RGB table on overrange-encoded values (`RgbTable::apply_hdr`), colour
grading in its HDR domain (`grade_hdr`) and black & white's gray, unclipped). It writes linear ProPhoto unclipped; `present_hdr.wgsl` sharpens it on
sRGB-curve-encoded values with ProPhoto luminance, applies the limit, and encodes
extended sRGB held to the display's headroom (`color::hdr_output::extended_srgb`,
which the CPU preview uses too). The HDR tone curve is sampled every 1/32 stop over
log2 scene value from 2^-16 to 2^12, linear below and a power above; Whites' gains
are read as the CPU reads them.

It covers what `pixel_params::supported_hdr` accepts: a master that renders
(Exposure, Whites, Blacks, Shadows, white balance, Contrast, Levels and the tone
curves, channel curves included, the colour stage, Color Grading, Texture, sharpening and what the SDR port samples), through
any profile and look whose HDR rendering is measured (`CameraProfile::hdr_tone`).
An HDR file's master renders from the pseudo-scene of its light, as `render_hdr`
renders it (`PreviewRenderer::render_hdr_file`): Exposure is a gain of the light baked
into the scene, so a new Exposure makes a new scene while every other change renders
from the one kept. Scenes are kept a size each, with stage caches of their own apart
from the SDR rendition's: Fit from the light reduced to twice its edge, as the CPU's
whole-photo preview reduces it, a 100% drag's preview from the light at its scale, and
100% from the full light. The post-crop vignette and grain, Defringe, Point Color, masks, and anything the SDR port leaves to the CPU
(`quality::HdrNotOnGpu`) give no GPU frame, and the CPU master is shown instead. Measured on an Apple M1 Pro
against `render_hdr` and `render_hdr_region` (`develop::gpu::tests`, run by the
"GPU matches CPU" workflow), with ordinary and HDR profiles, 3D tables, looks whose
curves end at white and below it, positive and negative Contrast, Levels, point and
parametric curves, channel curves, Saturation, Vibrance, the colour mixer (band Luminance included), colour grading, black & white, looks' RGB tables of every kind and amount, and Texture (neutral
colours, whose hue is undefined, left out of the colour stage's comparison): the per-pixel stage agrees to 2×10^-4 of the value,
the presented texture to 1.3×10^-3 (half-float steps), at full size and in a
region, at headrooms 4 and 40, sharpened and not. An HDR file's master (a PQ ramp
from 2^-10 to 2^5 times white in four colours, at its own settings, Exposure +1, −0.7
and +0.5, limit 3, Rotate with a crop) agrees to 1.3×10^-3 at full size and in a
region, and a Fit's mean to 2% of the CPU preview's
(`presented_hdr_files_match_the_cpu_master`).

### Histogram and readout

With HDR on, Develop's histogram is the HDR master's (`app::develop::hdr_histogram`),
laid out as Lightroom's: the left 40% is SDR, from black to graphics white by the
sRGB-encoded value as the SDR histogram bins its rendition; a solid line marks
graphics white, and the rest is stops above it up to the edit's HDR limit, with a
dashed line at each whole stop (`rendered::HdrHistogram`, 32 bins a stop). Channels
are the master's in linear sRGB primaries, as the preview shows them; bars of any
width are drawn as densities, so the two sections compare.

The histogram describes the edit, not the display, so it is measured wherever the
master renders, on an SDR display too: from a master of the whole photo 512 pixels
long, on a latest-only worker of its own, keyed by the photo and the settings as
the HDR preview is. A result for anything but the latest request is dropped; while
the next one renders, the last histogram of the same photo stays (another photo's
never shows), as the SDR histogram stays until its next render. Where the master is
refused (HDR off, an unmeasured adjustment) or hasn't rendered yet for
this photo, or failed, the SDR histogram shows, marked SDR, with the reason in its
tooltip.

The display's current headroom is applied only where the histogram is drawn, so a
change shows at once and renders nothing: what lies beyond it (all light above
white on a display without HDR, or while the window shows the SDR rendition) is
dimmed under a red bar, as Lightroom marks what the display can't show. The
highlight triangle is yellow when light above white is all within the headroom and
red when some is beyond it or flattened at the HDR limit; the shadow triangle works
as in SDR.

The RGB readout under the histogram reads the master too: from the HDR preview shown
there (its 100% region, or the whole photo, kept as half floats), otherwise from the
histogram's master of the edit as it is now; where the GPU's master shows, which
reads nothing back, from the histogram's. A channel up to graphics white reads as
its Melissa RGB percentage, one above it as its stops above white (`+1.3`), orange
where the display shows it and red beyond its headroom or at the HDR limit.

### Visualize HDR

**Visualize HDR**, under the Basic panel's HDR switch while HDR is on, colours the
photo Develop shows (the edit, or Before alone; not the side beside it) by the stop
above SDR white its master reaches, as Lightroom's does: cyan up to one stop, blue up
to two, purple up to three, magenta beyond. Light at or below white is left clear,
so the photo shows through. A pixel's stop is that of its brightest channel in linear
sRGB primaries, the channels the histogram and readout read, so a saturated colour
counts by its channel above white. The histogram marks each stop's section with the
same colour along its foot. Like Preview for SDR display it is a view choice
(`ViewState::visualize_hdr`), never saved with the edit.

The overlay describes the edit, not the display, so it shows wherever the master
renders: on an SDR display, under Preview for SDR display, and over the GPU's master
or the SDR rendition alike. It is the master rendered on the CPU by the HDR preview's
worker into slots of its own (`hdr_preview::Slot::OverlayWhole` and `OverlayRegion`),
as the preview renders it (the whole photo at the view's size, a 100% region at full
size), keyed as the master is but without the display's headroom, so a headroom
change renders nothing. While a 100% region's overlay renders, the whole photo's
shows in its place. Where the master is refused nothing is drawn, and the Basic panel's note says
why.

## What the master renders

The pipeline is the SDR one up to the scene tone stage's output (S3), with these
HDR rules, each measured on Camera Raw 18.7's PQ output (synthetic gray ramps and the
HDR reference family):

| Stage | HDR rendering |
| --- | --- |
| S3 default | Identity from scene 0.5 up (`scene_tone::hdr_whites::IDENTITY_FROM`), with the default black point's toe below, whatever the sensor's white or the photo's maximum. SDR's shoulder to the white point is not applied. |
| Exposure | A gain before S3, as in SDR. |
| Whites | A measured gain over scene value (`scene_tone::hdr_whites`), the same at every sensor white and HDR limit. Negative Whites is the same at every photo maximum too. Positive Whites is a gain of the colour's luminance over the default curve (RGBTone), so it keeps colours' saturation as Camera Raw's does; negative Whites is the gained curve RGBTone. Positive Whites follows the photo's level, twice its 99th percentile of luminance with Exposure (`PhotoMeasures::hdr_whites_level`): the darker it is, the further Whites stretches, and from a level of 4 up the gain no longer changes. Rendered up to +50; above it refused, as no level reaches Camera Raw's render: at Camera Raw's own best level per photo, 4 of 12 training photos stay 0.17–0.52 stop off above white at +75 and +100. |
| Blacks, default black | As in SDR, on values below 1; values above white are left alone. |
| Profile look table | An HDR profile's 3D table on overrange-encoded values, as the DNG SDK applies it; an ordinary profile's keeps the values above white it scales, its value looked up in the table's own encoding (an sRGB-encoded table's, as camera-matching profiles have, extended above white). An HDR profile's sRGB-encoded table looks up the sRGB encoding of the overrange-encoded value. |
| Profile tone curve | `camera_profiles::hdr_tone`: an HDR profile's curve by the DNG SDK's overrange rule; an ordinary curve by the measured extension (ACR3's measured HDR curve, and a fitted deviation for other curves); a curve that never brightens as the identity. RGBTone. |
| Look table | A look's HSV table (Adobe Color's and the other Adobe Raw looks', RAWmakase Color's) after the profile's, on overrange-encoded values, as the DNG SDK applies an HDR profile's, unclipped. |
| Look curve | `camera_profiles::hdr_look`, after the profile's curve: the look's curve in an encoding that is sRGB's up to graphics white and logarithmic above. A curve that ends at white follows itself up to its largest lift, then runs parallel to the identity at that lift, at most 16 (so a curve that never lifts renders as the identity); one that ends below white continues along its slope there. A look whose curve is the identity (a `.cube` look's) applies none, so its light above 16 is not held there. RGBTone. |
| Look settings | A look's own sliders and settings (Lightroom's B&W and creative looks carry Contrast, Blacks, Exposure, Saturation, colour mixer and parametric-curve settings) are added to the user's, as in SDR (`Recipe::with_profile_adjustments`), and render as those sliders do; one whose rendering isn't measured refuses the master, naming the look. A B&W look renders the black & white mix. |
| Profile Amount | As in SDR, the look's table, RGB table, curve and settings at that amount (`Enhanced::at_amount`), then rendered by the rules here. |
| Contrast | `develop::basic_tone::HdrContrast`, after the look, from the SDR Contrast curve at the photo's pivot (measured on the SDR rendering), in the look curve's encoding. Positive Contrast follows its curve up to its largest lift, then runs parallel to the identity at that lift; negative Contrast is its curve stretched in stops, its fixed point 1.671 times higher and white at 16. Above 16 positive Contrast adds a constant and negative continues along its slope. RGBTone. |
| Levels | RAWmakase's own (Camera Raw has none), after Contrast: as a point curve of its shape renders, as in SDR up to white and along its slope at white above it (none when the white point is below white). |
| Parametric curve | After Levels, RGBTone on the same encoding, its regions and splits laid over black to four stops above white (16); values above 16 are left alone. |
| Point curve | `color::hdr_curve`, after the parametric curve: the master curve, RGBTone on the look curve's encoding, as in SDR up to white; above it, along the curve's slope at its last point (a natural spline's own derivative there, before clamping), held back above 16 times white toward about 49 times (`hdr_curve::hold`), as Camera Raw holds steep curves. |
| Channel curves | `color::hdr_curve::channel`, after the master curve: each channel's own curve on that channel of the look curve's encoding of ProPhoto RGB, by the master curve's rule, its output held at 16. An identity curve leaves its channel alone. |
| Saturation, Vibrance, the colour mixer | The SDR grids of `develop::color_mixer`, after Contrast, the tone curves and Levels, as in SDR, on unclipped linear values ([The colour stage](#the-colour-stage)): looked up at the colour's value up to 0.2 and at 0.2 above it; Vibrance's value change fades out from 0.5 to white; band Luminance, from −100 to +40, is looked up up to 0.5 and its value change applied per channel, fading out by the channel's scene level from 0.6 to white; negative Luminance is up to 1.2 times stronger as the colour's brightest channel goes from 0.5 to white. |
| Look's RGB table | After the colour mixer, as in SDR (`RgbTable::apply_hdr`): in the table's primaries, each channel is overrange-encoded (0 to 16 times white onto 0 to 1) before the table's own encoding and decoded after it, and what it encodes beyond 1 skips the table and is added back to its output in that encoding. `.cube` looks, Lightroom's creative looks and fitted Camera Looks' tables alike. |
| Point Color | SDR's swatches (`develop::point_color`), after the colour mixer, on each channel taken into Color Grading's HDR domain (16 times white at 1) and back, clipping at 16 ([Point Color](#point-color)). |
| Color Grading | The SDR curves (`develop::color_grade_curves`), after the colour mixer and a look's RGB table, on each channel taken into a domain that is the value itself in the deep shadows and 1 at 16 times white, u = x / (1 + 15x/16), and back ([Color grading](#color-grading)); above white each channel's change strengthened by up to a tenth of its log by three stops above white (`color_grade_curves::hdr_boost`). Above 16 the gain at 16. A look's split toning is a pass of its own, as in SDR. |
| Black & White | The SDR mix's gray (`develop::black_white`) looked up as the colour mixer's, unclipped above white. |
| Defringe | After the colour stage, before sharpening: SDR's operator weighted by the luminance range around each pixel and the colour's chroma, and a band of bright, saturated blues losing chroma above white ([Defringe](#defringe)). |
| Calibration | As in SDR: a matrix on scene colour before Exposure, and Shadows tint, which only reaches values below white. |
| Shadows | The SDR local tone operator (`develop::local_tone`) on scene values, unclipped: its gain at a pixel's base level as in SDR, before S3. |
| Clarity | −50 to +50: the SDR operator (`develop::clarity`, in the local tone map) on scene values, unclipped, its gain 1.5 times SDR's within 1.6% of the long edge of light above scene level −1 (`local_tone::HDR_CLARITY_NEAR_GAIN`), as Camera Raw's HDR Clarity is stronger beside light above white. Beyond ±50 it is refused (+100 is further from Camera Raw below white than in SDR). |
| Dehaze | −50 to +100: the SDR operator, with the photo's haze, unclipped, as in SDR. Below −50 it is refused (at −70 and −100 it is further from Camera Raw below white than in SDR). |
| Post-crop vignette | After sharpening, on linear values, unclipped ([Vignette and grain](#vignette-and-grain)): darkening a gain (of the scene through the profile's HDR curve for Highlight Priority); lightening a blend towards graphics white (Paint Overlay, Color Priority) or the scene towards 14.4 (Highlight Priority). |
| Grain | After the vignette: SDR's grain added to sRGB-curve-encoded values extended above 1, its strength going on above white and fading out by 16 times white. |
| Masks | Temp, Tint, Exposure, Shadows and Texture, through the same stages as their global sliders, weighted by the mask; Contrast below white only, Color fading out by white, and Sharpness and Noise on the encoded master ([Masks](#masks)). |
| HDR limit | Each channel rolled off to 2^limit from 97% of it (`model::hdr::roll_off`), last. |
| Sharpening, resizing | On sRGB-curve-encoded values of the master's primaries, unclipped, with ProPhoto luminance; the limit applies after them. |

Lens corrections, geometry, noise reduction, AI Denoise, Red Eye, Heal and Clone, and Texture
work on camera samples before tone and render as in SDR.

### The colour stage

Camera Raw 18.7's HDR renders of synthetic colour patches (12 hues, saturations 0.3
to 0.9, 0.125 to 8 times white, a linear and the ACR3 profile curve) show:

- Hue and saturation changes don't depend on brightness. Saturation +100 multiplies
  saturation by 1.73–1.74 from 0.125 to 8 times white, and its value change (×1.15 at
  saturation 0.3) is the same at every brightness. Its SDR render gives 1.78 at 0.5,
  because SDR rolls off from there; the colour mixer's grids, fitted on SDR renders,
  carry that roll-off at high values. An HDR master therefore looks them up at the
  colour's value only up to 0.2 (`color_mixer::HDR_VALUE`) and keeps the change found
  there above it.
- Vibrance's value change applies below white and fades out by white (value ×1.06 at
  0.5, ×1.01 from 1 to 8); its saturation change does not.
- The colour mixer's Luminance changes each channel below about white and not the ones
  above it: Luminance +100 of every band gives value ×1.67 at 0.5 and ×1.00 at 1, 2, 4
  and 8, where a colour at white turns almost gray. An HDR master looks its change up at
  the colour's value up to 0.5 (`color_mixer::HDR_LUMINANCE_VALUE`), so below 0.5 it is
  the SDR rendition's, and applies its value part to each ProPhoto channel by that
  channel's scene level (before the profile's curve, as the mixer's other changes change
  it): in full up to 0.6 (`color_mixer::HDR_LUMINANCE_FADE`), fading out to none at white.
  Its hue and saturation parts move to the other changes' lookup between 0.5 and white.
  Negative Luminance darkens bright colours just above white more than that: its change
  grows to 1.2 times as the colour's brightest channel goes from 0.5 to white
  (`color_mixer::HDR_LUMINANCE_DARKEN`), and it renders down to −100. Positive Luminance
  renders up to +40; beyond, Camera Raw's HDR effect strengthens with brightness where
  its SDR one rolls off, which the SDR grids can't give (every band +100 leaves the Nikon
  Z6 III 0.20 stop off above white), and the master is refused.
- Black & White's gray is the mix's, unclipped above white.
- Calibration renders as in SDR: its matrix works on scene colour, and Shadows tint
  fades out below white.

Measurements and validation: [colour stage](records/hdr-stage1-results.md#colour-stage).

### Vignette and grain

Camera Raw 18.7's HDR renders of gray tiles from 0.03 to 8 times white (a linear
profile, limit 8, each style at Amount −100, −50 and +50, and grain at 50 and 100)
show the SDR operators on unclipped light:

- **Darkening** is the same gain at every level from a tenth of white up, in every
  style. RAWmakase applies Paint Overlay's and Color Priority's to the output, and
  Highlight Priority's to the scene, through what the master renders a neutral as at
  the edit's settings (`pipeline::hdr_neutral_response`: the scene tone stage, its toe
  included, then the profile and the rest of the finish), as SDR exposes through
  Camera Raw's measured default response.
- **Lightening** by Paint Overlay or Color Priority blends towards graphics white, so
  it darkens light above white (×0.89 at eight times white at a quarter of the way
  out); Highlight Priority blends the scene towards 14.4 (fitted: 13.5–15.2 at every
  level).
- **Highlights** protects only from about four times white up (−50 with Highlights 50:
  ×0.32 at four times white and ×0.43 at eight, against ×0.29 without), unlike SDR's
  protection near white; it refuses the master.
- **Grain** is SDR's field added to sRGB-curve-encoded values extended above 1. Its
  spread there keeps going above white (0.89 of a quarter of white's at white, 0.79 at
  four times, 0.44 at eight), where SDR's fades by graphics white; it fades out by 16
  times white.

On the tiles RAWmakase renders every style's effect above white to a median 0.010
stop or less (p95 ≤ 0.07), and grain's spread to within 1% from a tenth of white up;
Highlight Priority −100 is further off in the frame's darkest tiles (p95 1.5 stops).
On the twelve validation and holdout photos (Adobe Standard), Highlight Priority −30's
effect is within ±0.06 stop of Camera Raw's in every ring from the centre to the
corners (its corner darkening −1.20 to −1.75 stops); Paint Overlay +30 with grain 40,
within ±0.02 (±0.08 on one Panasonic). The GPU's HDR preview leaves both effects to
the CPU master. Measurements:
[vignette and grain](records/hdr-stage1-results.md#vignette-and-grain).

### Point Color

Camera Raw 18.7 applies Point Color's swatches to an HDR master in the domain it
grades in ([Color grading](#color-grading)): each channel taken into u = x / (1 + 15x/16)
and back, where a swatch's shifts and ranges work as in SDR and channels clip at 1,
16 times white. On synthetic colour patches from 0.125 to 8 times white:

- A Luminance shift is SDR's in u, so it lifts light above white far more than SDR,
  which clips there: Luminance +50 brightens an orange ×1.68 at a quarter of white,
  ×3.5 at white and ×6.1 at twice white, and ×3.8 and ×2.1 at four and eight times as
  the clip at 16 holds it.
- A colour very bright in linear light has little saturation in u, so the swatch's
  saturation range stops selecting it: Hue +50 turns an orange 19° at white, 8° at
  four times white and 2–4° at eight.

`point_colors_hdr` renders that rule. On the patches its effect on luminance is
within a median 0.001 stop of Camera Raw's (p95 ≤ 0.10), and mean ΔE_ITP 0.4–1.3
(2.4 above white, for Luminance +50; RAWmakase's SDR Point Color on the same patches,
0.3–0.5). A master with Point Color renders on the CPU. Measurements:
[record](records/hdr-stage1-results.md#point-color).

### Color grading

Camera Raw 18.7 grades an HDR master with its SDR curves, but spreads the regions over
the HDR range: each channel is graded in u = x / (1 + 15x/16) and mapped back, so
16 times white grades as SDR's white does, whatever the HDR limit (limits 2.3, 4 and
8 render alike below the limit's roll-off). On a gray ramp from 2^−8 to 16 times
white:

- The shadows grade as in SDR, where u is nearly x.
- Highlights' tint and Luminance peak near three times white instead of at half of
  it, and much stronger in linear light: Highlights Luminance +50 brightens 2.8 times
  white ×1.60, where SDR's largest gain is ×1.15.
- Global's Luminance is a gain from the shadows to about 1.4 times white, then fades.
- Every lift and tint fades out by 16 times white; darkening keeps darkening up to it
  (Highlights Luminance −50: ×0.375 at 16).

`ChannelCurves::apply_hdr` renders that rule; on the ramp it gives Camera Raw's effect
to a median 0.000–0.008 stop under 24 settings. Above 16 a channel keeps its gain at 16.
Strong blue tints of saturated colours above white differ: Camera Raw lifts the lowest
channel there, which no per-channel rule does (up to 12 mean ΔE_ITP above white on the
synthetic patches; RAWmakase's SDR grading of the same patches is within 2). On the
twelve validation and holdout photos a split tone, a Highlights tint with Highlights
Luminance +30, and a Global tint with Midtones Luminance −20 score as the photos'
default renders do (median above white within 0.02 stop of the default's, mean
ΔE_ITP within 0.5). Measurements:
[color grading](records/hdr-stage1-results.md#color-grading).

### Defringe

Defringe renders in the master as Camera Raw 18.7's HDR Defringe was measured on
photos, not as SDR's (`develop::quality::hdr_defringe`). On the twelve validation and
holdout photos (Purple Amount 8 and Green Amount 4) Camera Raw's HDR Defringe departs
from its SDR one:

- below white it removes less than its SDR render (chroma kept ×0.67–0.74 where SDR
  keeps ×0.45–0.63 on the blocks it changes), and changes colour along edges, leaving
  smooth and faint colour alone; SDR's operator, by hue and chroma alone, also changes
  smooth areas and deep shadows Camera Raw leaves;
- above white it takes a band of bright, saturated blues its SDR render leaves alone:
  the Z 8 photo's sky with its brightest channel 4.5–6.5 times white (Oklab hue
  0.65–0.67, outside RAWmakase's Purple window) keeps ×0.51–0.56 of its chroma, and a
  Sony ARW's blues in the same band ×0.75–0.80; dimmer or paler blues keep it all.

So the master runs SDR's Oklab operator on each pixel (a colour above white taken with
its brightest channel at white, then scaled back), weighted by the luminance range
within 4 pixels of the full photo (full from one stop) and by the colour's Oklab chroma
(none below 0.015, full from 0.04); then a colour in that band of blues (its brightest
Display P3 channel 4.5–6.5 times white, saturation above 0.55, Oklab hue 0.63–0.675)
loses half its chroma at Purple Amount 8, other Amounts by SDR's Amount curve. It runs
on the CPU after the colour stage and before sharpening, the vignette and grain, as
SDR's Defringe precedes them; a region reads the window around it, so it renders as
that part of the whole. The GPU's HDR preview leaves it to the CPU.

On those photos the master's change scores a median 1.06 below white and 1.09 above
it, against 1.49 for RAWmakase's SDR Defringe against Camera Raw's SDR (the error over
the blocks either render changes most, relative to Camera Raw's change), and no photo
is worse than its SDR score by more than 0.2. On the blocks Camera Raw changes most it
keeps ×0.89 of their chroma where Camera Raw keeps ×0.68. The weights and the band were
fitted on these photos; no other photos have HDR Defringe captures.

Defringe with Black & White is refused: the master's runs after the gray mix, where it
would change nothing, while SDR's precedes it. Measurements:
[record](records/hdr-stage1-results.md#defringe).

### Masks

A mask's Temp and Tint, Exposure, Shadows and Texture are added to the photo's own
values in the stages that render the global sliders (white balance and Exposure on
the scene, the local tone map, Texture on the camera's samples), so in HDR they render
as those do: a mask over the whole frame renders the global slider's master
(`develop::quality::hdr` tests). Range masks (Color Range, Luminance Range)
select from the SDR rendition, as the mask overlay shows them, so the master adjusts
the pixels its SDR rendition adjusts. In Camera Raw 18.7's HDR renders of a
whole-frame mask on synthetic patches, a mask's Exposure, Shadows and Texture render as
their global sliders do (median 0.000–0.039 stop), and Temp is a gain of the scene at
every level. RAWmakase's mask Exposure matches Camera Raw's to 0.001 stop and Texture
to the global slider's own error (0.027); mask Shadows is 0.061 stop off, twice the
global slider's 0.030, and Temp's strength is SDR's, as far from Camera Raw's in SDR.

A mask's own operators render in the master where Camera Raw's HDR render of them
was measured and RAWmakase's comes as close to it as its SDR render comes to Camera
Raw's SDR one ([record](records/hdr-stage1-results.md#masks)):

- **Contrast**: SDR's mask curve on sRGB-encoded values up to white, and nothing above
  it (`masks::local::tone_hdr`), as Camera Raw renders it.
- **Color**: SDR's tint of the scene, in full up to a quarter of white and fading out
  by white on the pixel's brightest channel (`masks::local::color_hdr_strength`), as
  Camera Raw's stops tinting there.
- **Saturation** from 0 to +50, overlapping masks added: Camera Raw's HDR mask
  Saturation, which keeps a colour's value and hue and raises its HSV saturation as a
  power of itself, s^(1 − 0.74 k^1.4) at Saturation k, at every brightness
  (`masks::local::saturate_hdr`). The SDR rendition keeps SDR's Oklab operator.
- **Sharpness and Noise**: SDR's operators on the master's encoded values, as global
  sharpening is; they change detail, not tone.
- **Whites**: Camera Raw's HDR mask Whites, a gain over brightness of one measured
  shape (`scene_tone::global`'s `HDR_MASK_WHITES_SHAPE`) times 0.874 of the largest
  change SDR's mask curve makes at that setting, still rising at 16 times white where
  SDR's curve falls.

Blacks and Hue in an active mask refuse the master by name ("A mask's
Blacks …"), and so does Saturation below 0 or above +50. Camera Raw's HDR mask Blacks
−50 brightens light above white (+0.39 stop at 16 times) and crushes the shadows harder
than its SDR one. SDR's mask Saturation brightens saturated colours (×1.11 in value at
+50, Camera Raw's ×1.004), which white's clip hides in SDR; the master's own operator
misses above white at +100 on the colour patches and at −50. Hue is SDR's Oklab
rotation, further from Camera Raw above white than the SDR rendition is below it. Highlights has no HDR rendering
globally. A mask's Clarity renders where, with the photo's own and a look's, every
pixel stays from 0 to +50, the global slider's range: it adds to the photo's in the
global slider's stage. The check is conservative, taking each mask's Clarity at its
Amount as reaching every pixel. A mask's Dehaze renders from 0 to +30, overlapping
masks added, and up to +100 with the photo's own: it adds to the photo's in the global
slider's stage too. Negative mask Dehaze is refused: at −30 the master is further from
Camera Raw than the SDR rendition (0.198 against 0.184 stop on six photos). A masked
master renders on the CPU.

## What is refused

`model::hdr::check_recipe` refuses the master, not the edit, when the edit uses
something without a measured HDR rendering: Highlights, Whites above
+50, Refine Saturation other than 100%, a point curve (the master's or a
channel's) whose last point is left of white, Clarity beyond ±50, Dehaze below −50, the
colour mixer's Luminance above +40, a darkening vignette's Highlights,
a mask's Blacks, Hue or Highlights, a mask's Saturation outside 0 to +50, a mask's Clarity outside 0 to +50 with the photo's own, a mask's Dehaze outside 0 to +30, Defringe with Black & White ([Defringe](#defringe)), a look that sets any of these (Adobe Landscape sets Highlights −12: "Highlights,
which the look sets, …"; Adobe Vivid's Clarity +10 renders), and ordinary tone curves unlike the measured family. The edit stays valid and keeps HDR on, so nothing is downgraded:
its SDR rendition renders all of it, and the reason is shown where the master is
asked for.

## HDR files

An HDR file opens in Develop as an HDR edit ([the design](records/hdr-stage4-design.md)).
These files qualify:

- a TIFF, PNG, JPEG XL or AVIF whose CICP (in the file or its ICC profile) says PQ,
  HLG or linear light, at full range, in BT.709, BT.2020 or Display P3 (RAWmakase's
  own HDR exports among them);
- a HEIC whose `nclx` says PQ or HLG in BT.709, BT.2020 or Display P3, with a matrix
  of BT.709, BT.601 or BT.2020's non-constant luminance, and no ICC profile, on macOS
  and where libheif decodes it ([HDR HEIC](#hdr-heic));
- on macOS, a HEIC with a gain map over an SDR base: an iPhone's own (Apple's
  auxiliary image) or an ISO 21496-1 `tmap` ([HEIC gain maps](#heic-gain-maps));
- a TIFF of 32-bit float samples, through an ICC profile without CICP whose curves
  are the identity (Photoshop's 32-bit documents), or without a profile, read as
  linear sRGB primaries; 1.0 is graphics white. Integer samples with such a profile
  stop at graphics white, so they open as SDR files;
- a JPEG or JPEG XL with an ISO 21496-1 gain map over an SDR base, or a JPEG with one
  described by Adobe's earlier `hdrgm` XMP, whose light the map reconstructs and whose
  base stays its SDR rendition ([Gain-map files](#gain-map-files)).

A PNG's own cICP chunk counts as its CICP and takes precedence over its ICC profile,
as the PNG specification says; the `png` crate reads it, checking its CRC. A
narrow-range cICP and an APNG are refused for Develop, HDR or not.

Its light (`camera_data::HdrLight`: linear ProPhoto, 1 = graphics white, signed) is
decoded exactly, in f32, by `HdrOutput::decode`, the inverse of the HDR export's
encoding. The image's own pixels are its SDR rendition: graphics white at 0.926 and
brighter light rolled off into white, as the Library shows it, which Develop's SDR
path renders through the rendered-file pseudo-scene. That SDR rendition is
RAWmakase's; Camera Raw's for these files is not measured.

A new HDR file starts with HDR on (`Recipe::for_new_rendered`), at the fewest whole
stops of limit whose roll-off starts at or above its peak
(`model::hdr::limit_containing`). A saved edit keeps its own settings; one that says
nothing of HDR, a Lightroom edit made before, reads HDR off, as it did when HDR
files didn't open.

Its master renders through the same stages as a raw's HDR master
(`develop::render_hdr`), from the HDR pseudo-scene of its light
(`develop::rendered::scene_from_light`), through the Embedded profile with a linear
curve (`CameraProfile::for_hdr_light`), which renders in HDR as the identity. The
pseudo-scene inverts what those stages render a scene as at a new file's settings,
DNG RGBTone fashion: the scene tone stage's HDR white curve at Whites 0, the identity
from 0.5 up and within 0.0015 stop of it below. At the file's own settings the
master is its light again, exactly from 0.5 up and within 0.0001 stop below, and
within 1e-5 of a pixel's brightest channel, the precision of the colour matrices
the stages convert through, as a raw master's do; negative channels render as 0, as
no HDR master renders them. An edit without a profile renders with the Embedded
one. Exposure is a gain of the
light, applied before the pseudo-scene. That is how Camera Raw 18.7 renders a PQ
TIFF it opens as an HDR edit: its light at the file's settings, and Exposure ±1 and
+2 as gains of it at every level ([HDR files](records/hdr-stage1-results.md#hdr-files)). The master takes crop, straighten, Rotate and Flip, sharpening and
resizing as a raw's does, and then the limit, and the SDR rendition's sliders, which
never change it (measured on raws; for HDR files the same is assumed). Anything else
refuses the master by name (`model::hdr::check_hdr_file`); the edit keeps it, and its
SDR rendition renders it.

Still refused, as Develop refused every HDR file before:
- HEIC gain maps on Windows and Linux, which open as their SDR base, and the HDR
  HEICs and HEIC gain maps listed under [HDR HEIC](#hdr-heic) and
  [HEIC gain maps](#heic-gain-maps);
- HDR in other primaries or a curved profile.

The Library shows them with graphics white as white and says why.

### HDR HEIC

RAWmakase has no HEVC decoder ([dependencies](dependencies.md)), so an HDR HEIC's
light decodes exactly only where the system's decoder gives its signal back as coded,
converted to RGB by the file's matrix and range and nothing else:

- **macOS:** ImageIO decodes the file into CoreGraphics' own space of its CICP
  (`kCGColorSpaceITUR_2100_PQ`, `DisplayP3_HLG` and the others), and RAWmakase draws
  it into a bitmap of that same space, which CoreGraphics leaves as it is. On
  `tests/data/formats/hdr-*.heic` (scripts/make-hdr-heic-fixtures.py: flat patches of
  known 10-bit signal, PQ and HLG in BT.2020, PQ in Display P3) it gives back every
  patch to within one code value.
- **Linux and other systems:** libheif converts by the `nclx` itself; the same
  fixtures are checked where it is installed (CI's Arch job).

The signal then decodes as any HDR file's does, by BT.2100, in f32, and its Library
preview is RAWmakase's tone map. Refused for Develop:
- **"HDR"** on Windows, whose decoder (Windows Imaging Component with Microsoft's
  extensions) isn't checked; with an ICC profile beside the `nclx`, which then describes
  the pixels instead; with HDR named only by an ICC profile's `cicp` tag, without an
  `nclx`, which ImageIO draws through that profile; and in other primaries;
- **"HDR matrix N"** for a matrix that isn't one: constant luminance (10, 13) and
  ICtCp (14).

### HEIC gain maps

On macOS, a HEIC with a gain map opens as an HDR edit, its base its SDR rendition and
its light the map at full weight, as for [gain-map files](#gain-map-files)
(`raster::heic`). ImageIO decodes the map with the image (its auxiliary data,
`kCGImageAuxiliaryDataTypeISOGainMap` or `HDRGainMap`); the map is in the image's
stored frame and turns with its `irot` and `imir`. Two forms are read, the first when
a file has both:

- **ISO 21496-1:** a `tmap` item whose `dimg` references are the primary image and
  the map, as iPhones write since iOS 18. Its data is a version byte (0) and the ISO
  21496-1 metadata, read and applied as a JPEG's is.
- **Apple's own:** the primary image's auxiliary image of type
  `urn:com:apple:photo:2020:aux:hdrgainmap`, as iPhones wrote from iOS 14. It applies
  as Apple's [Applying Apple HDR effect to your photos](https://developer.apple.com/documentation/appkit/applying-apple-hdr-effect-to-your-photos)
  says, the formulas written out in `gain_map::Formula::Apple` and
  `gain_map::apple_headroom`:
  - the headroom, from the maker note's tags 33 and 48, is 2 raised to stops on one of
    four lines (−20 × t48 + 1.8 or −0.101 × t48 + 1.601 when t33 is below 1, else
    −70 × t48 + 3 or −0.303 × t48 + 2.303, the first of each where t48 is at most
    0.01), at least 0;
  - the map's sample is Rec. 709-encoded: it is made linear by that transfer;
  - each channel of the base, linear in its own primaries, becomes
    `base × (1 + (headroom − 1) × sample)`.

  An Apple map is applied only as its documentation calls it valid: with
  `HDRGainMap:HDRGainMapVersion` in the XMP describing it and both maker-note
  values, read from the file's own items as it was opened. Without them, or with a
  headroom of 1 (no stops), it isn't applied and the photo opens as its SDR base, as
  it did before; nothing about such a map refuses the photo. ImageIO renders Apple's maps its own
  way (a synthetic map decoded at full headroom comes out 0.3 to 0.4 stop darker
  than the documented formula gives above white), so the documentation, not ImageIO, is the reference.

A map smaller than the base (an iPhone's is half its size each way) is spread over it
between its samples' centres, as for JPEG. Refused for Develop, by reason:
- **"damaged gain-map"**: a `tmap` not over the primary image and one other, a map
  that isn't an HEVC picture or grid, larger than the image or not its shape, whose
  data isn't all in the file or whose pictures, as their SPS code them, take the image
  beyond 150 MP (checked before ImageIO is asked for it), or that ImageIO doesn't
  decode to its header's size in 8-bit grey or RGB;
- **"version N gain-map"**, **"HDR-base gain-map"** and the other refusals of ISO
  21496-1 metadata above, and **"alternate-colour gain-map"** for a `tmap` in the
  alternate's colour space, which its own `colr` describes;
- **"cropped HEIC gain-map"**: an image its `clap` crops, which the map, of the whole
  picture, isn't checked against.

Checked on synthetic files ImageIO writes (scripts/make-heic-gain-map-fixtures.swift):
an Apple map against the documented formulas written out in the test, a turned one
against the same file unturned, and an ISO 21496-1 one against ImageIO's own decode at
full headroom (0.02 stop, 0.032 at the darkest pixel). Windows and Linux don't read
HEIC gain maps: their decoders' maps aren't reached, so the photo opens as its SDR
base, as before.

### Gain-map files

A JPEG or JPEG XL with an ISO 21496-1 gain map opens as an HDR edit too
(`raster::gain_map`). That covers RAWmakase's own gain-map exports, libultrahdr's
Ultra HDR, and Adobe's and Apple's ISO form. The reader mirrors the exports
([Gain maps](#gain-maps)):

- **JPEG:** the base carries the APP2 version marker naming ISO 21496-1 and an MPF
  index. The index's first secondary image whose own APP2 segment holds the metadata
  is the map.
- **JPEG XL:** the container's `jhgm` box holds the metadata and the map's
  codestream.
- **JPEG, XMP only:** Adobe's earlier gain-map format (`hdrgm` 1.0, which Lightroom and
  Camera Raw wrote before ISO 21496-1 and Android's Ultra HDR 1.0 adopted). The base's
  XMP says `hdrgm:Version="1.0"`, and the MPF index's first secondary image whose own
  XMP holds `hdrgm` metadata is the map. A file with both forms is read by its ISO
  metadata, as libultrahdr reads it. Ultra HDR's GContainer directory isn't read:
  its writers must write the MPF index too.

`hdrgm` metadata is read as Android's Ultra HDR 1.0 specification lists it: the
image's own properties (those of each top-level `rdf:Description`, not of a resource
nested in one), from the map's XMP packet and the extended packet it names
(`xmpNote:HasExtendedXMP`), whose chunks must be there and fill it once. Each value is
an attribute or an element of text alone, one for every channel or an ordered array
(`rdf:Seq`) of three such items; an `rdf:Bag`, an `rdf:Alt` or an item holding
anything but text is refused:
`GainMapMin` (0 when absent) and `GainMapMax` in stops, `Gamma` (1), `OffsetSDR` and
`OffsetHDR` (1/64 each), `HDRCapacityMin` (0) and `HDRCapacityMax`, and
`BaseRenditionIsHDR` ("False"). It applies by the same formula as ISO 21496-1's, at
full weight, always in the base's colour space: the map's own profile isn't used. The
capacities only weigh the map for a display, so they are checked, not applied.

**The light** is the authored HDR rendition, reconstructed at the map's full weight:
what a display with the alternate rendition's whole headroom shows. No display's
headroom is read, so the light is the same on every monitor and in every export.
The steps:

1. The base is decoded through its ICC profile to linear light in its own primaries,
   where 1 is its white, taken as graphics white.
2. Each channel becomes (base + base offset) × 2^gain − alternate offset. The gain is
   interpolated in stops between the map's minimum and maximum by the map's sample,
   raised to 1/gamma.
3. The result goes to linear ProPhoto through the profile's colorants.

A single-channel map applies one gain to all three channels, a three-channel map one
each. A map smaller than the base is interpolated bilinearly between its samples'
centres, the edges held, as a GPU samples a texture. This matches how libultrahdr's
encoder makes one: each sample is the average of its block. libultrahdr's own
decoder samples corner-aligned instead.

**The SDR rendition is the base,** decoded exactly as the same JPEG or JPEG XL
without its map: the Library shows it, and Develop's SDR rendition and SDR exports
render it. That makes it the authored SDR rendition at neutral settings, not
RAWmakase's tone map. A catalog edit made before gain maps were read keeps its
settings, HDR off included, and renders as it did. New files start as HDR edits,
with the same defaults, master and refusals as the other HDR files above;
`HdrSource::gain_map` records the provenance.

**What is read.** Everything is read with bounds: segment, MPF, box and metadata
lengths are checked against the file, and the map's size against its header.
Refused for Develop, by reason (the Library shows the base):

- **"damaged gain-map".** Any of these:
  - a short or truncated payload, or a zero denominator;
  - a maximum below its minimum, a gamma not above 0, or an offset outside 0–1;
  - gains or headrooms beyond 16 stops;
  - no MPF index, or one that points outside the file or at no map with metadata;
  - a map larger than its base, or of another shape beyond a map sample's
    rounding;
  - a map that doesn't decode, or decodes to a size other than its header's;
  - for `hdrgm`: no `GainMapMax` or `HDRCapacityMax`, a value that isn't a number, an
    array of two or more than three, an offset below 0, a capacity below 0, or a
    maximum capacity not above the minimum.
- **"version N gain-map":** a minimum version above 0, in the base's marker, the
  metadata or the JPEG XL bundle, or an `hdrgm:Version` other than 1.0, in the base's
  XMP or the map's.
- **"HDR-base gain-map":** the base is the HDR rendition, by its flag (ISO's, or
  `hdrgm:BaseRenditionIsHDR="True"`) or because its headroom is at or above the
  alternate's.
- **"alternate-colour gain-map":** the map applies in the alternate's colour space,
  which an ICC profile or colour encoding of its own describes.
- **"grey gain-map"** and **"table-profile gain-map":** a grey base, or a base whose
  profile converts through tables, which give the map no linear RGB to apply in.
- **"turned JPEG XL gain-map":** a JPEG XL whose base or map codestream is turned.
  Which frame the map is in is not settled for JPEG XL. A JPEG's map is in the base's
  stored frame and turns with its EXIF orientation.
- **"compressed gain-map":** a `jhgm` box compressed in `brob`.

A JPEG XL whose own samples are HDR opens by them as above, whatever map it carries.

**Checks.**

- RAWmakase's own gain-map exports of synthetic ramps reopen with the master's
  luminance: median 0.005 stop for both JPEG and lossless JPEG XL, worst 0.03 and
  0.015. That is the map's 8-bit steps; the single-channel maps carry luminance, and
  colour is the base's.
- libultrahdr 2.0.2's own Ultra HDR file (Display P3 base, three-channel map the
  base's size) opens with the light libultrahdr decodes at full headroom, to 0.019
  stop on the checked pixels: median 0.004 and p95 0.014 over the image.
- With a quarter-size map, libultrahdr's corner-aligned sampling differs from
  RAWmakase's by a median 0.08 stop on that steep ramp.

## HDR export

An export with **HDR** on (the Export dialog's HDR rows for TIFF and JPEG XL, `ExportSettings::hdr`,
`rawmakase render --hdr-export`) writes the edit's HDR master instead of its SDR
rendition. The encoding is captured with the job (`export::job::prepare`) and never
reads the display or the editor. Gamut and transfer are separate choices
(`color::hdr_output`):

| Transfer | File | Graphics white |
| --- | --- | --- |
| PQ (ST 2084) | 16-bit TIFF | 203 cd/m², absolute; signal 0.5806 |
| HLG (BT.2100) | 16-bit TIFF | 203 cd/m² on HLG's 1,000 cd/m² reference display: the inverse OOTF (system gamma 1.2 on the gamut's luminance), then the OETF; signal 0.75 |
| Linear | 32-bit float TIFF | 1.0, as Photoshop's 32-bit documents |

PQ and HLG are also written as 16-bit JPEG XL (`export::jxl`), lossy at the Export
dialog's quality or, with Lossless checked, lossless (`ExportSettings::jxl_lossy`,
`render --jxl-quality`). The header names the primaries (JPEG XL's enumeration: P3 is
11 there, 12 in CICP) and the transfer, with the tone mapping's intensity target
(10,000 cd/m² for PQ, 1,000 for HLG), and the file is a container with `Exif` and
`xml ` boxes. A file beyond codestream Level 5's limits declares Level 10 in a `jxll`
box, as cjxl's do: a lossless HDR export (its 16-bit samples need wider modular
buffers than Level 5 allows), and any export with a side longer than 262,144 pixels
or more than 2^28 pixels. Settings saved before JPEG XL could be lossy read as
lossless; new ones start lossy.

- **Lossy:** `jixel` encodes a VarDCT frame in XYB at libjxl's distance for the quality
  (`cjxl`'s mapping: 90 is distance 1, the default 92 is 0.82), at its fast effort.
  XYB holds absolute display light and the header only says which signal to decode
  it to. jixel reads PQ input correctly but takes HLG input for normalised scene
  light, which decodes about two stops dark, so an HLG export is given to it as the PQ
  of its light on HLG's 1,000 cd/m² reference display, and its header, checked
  against the one RAWmakase writes for that frame, is replaced with one that says
  HLG. The 45 MP Z 8 photo is about 9.1 MB at quality 92; libjxl's decoder gives back
  its light to a median 0.006 stop above white (p95 0.03) for PQ and 0.002 stop for
  HLG ([record](records/hdr-stage1-results.md#codecs)).
- **Lossless:** `zune-jpegxl` encodes the frame and RAWmakase writes the image header in
  place of zune's sRGB one. Lossless files are large (193 MB for the same photo, as
  large as its 16-bit TIFF).

JPEG XL also exports SDR, in 8-bit sRGB only, lossy or lossless as HDR is.

jxl-oxide (RAWmakase's own JPEG XL reader too) converts a lossy file's XYB light to its
primaries after clipping it, in BT.709 primaries, to the intensity target. A wide-gamut
HLG export's saturated highlights near 1,000 cd/m² therefore read back clipped there,
and some of its pixels as NaN, as they do from libjxl's own lossy HLG files; libjxl's
decoder doesn't clip them.

Gamuts: Rec. 2020, Display P3 and Rec. 709, all D65 (Bradford from the master's D50).
Out-of-gamut channels clip at 0, and each channel is held to the HDR limit again in
the output gamut. The ICC profile (`color_management::hdr_profile`) is ICC v4 with
the gamut's colorants and a `cicp` tag naming primaries and transfer; EXIF's
ColorSpace is Uncalibrated. No mastering-display metadata is written.

An HDR export needs an edit with HDR on (otherwise it is refused, not exported as
SDR), refuses watermarks (they are composited in SDR) and is a TIFF or a JPEG XL
(linear light a TIFF only). An SDR export of an HDR edit is its SDR rendition. HDR
exports go through the export queue as SDR ones do: staged to a temporary file beside
the target, which a cancel or a failure removes, and committed whole.

### Gain maps

A JPEG export with HDR on writes a gain map (`export::gain_map`, ISO 21496-1) instead
of HDR pixels: the base is the edit's SDR rendition, exactly the SDR JPEG export's
pixels and metadata, and the gain map turns it into the HDR master on a display with
headroom. The map is one 8-bit channel the base's size, computed in the base's colour
space (sRGB primaries, linear) from luminance as log2 of (HDR + 1/64) / (SDR + 1/64),
between its minimum and maximum over the photo, gamma 1. The base's headroom is 0 and
the alternate's log2 of 2^limit. The SDR rendition's sliders (`crs:SDR*`), which Camera
Raw's base follows, render in the base as in SDR exports, where they render. The file is laid out as libultrahdr writes it: the base
with an ISO 21496-1 version marker and an MPF index, then the map's grayscale JPEG with
the metadata in APP2. No XMP `hdrgm` metadata (Adobe's earlier gain-map format) is
written. A gain map's gamut is the base's, Rec. 709.

A JPEG XL export can carry the same gain map (the Export dialog's "As a gain map over
the SDR rendition", or `--hdr-export gain-map` to a `.jxl`): the base is the 8-bit
sRGB SDR JPEG XL, and a `jhgm` box (libjxl's gain-map bundle) holds version 0, the
ISO 21496-1 metadata, no alternate colour encoding or ICC profile (the map applies in
the base's colour space), and the map's grayscale codestream. Both are lossy, at the
same distance, as a gain-map JPEG's map has its quality, or both lossless. RAWmakase
writes the bundle itself around jixel's or zune's codestreams. For the Z 8 photo at
quality 92 the file is 10.3 MB (74 MB lossless), and the lossy map is within 2 of its
256 codes at p95 (0.03 stop). libjxl 0.12's `jxlinfo` reads the box as a version 0
bundle with its 61-byte metadata and the map's codestream, and `djxl` decodes the base
and the map. No viewer checked applies a JPEG XL gain map: macOS ImageIO and Chrome
show its base, exactly the SDR export's pixels. Its ISO 21496-1 reconstruction is
checked by the interoperability harness instead, at several display headrooms
([JPEG XL interoperability](records/hdr-jxl-interop.md)).

libultrahdr 2.0.2 (`ultrahdr_app`) reads the Z 8 reference photo's gain-map export
(2048 px) as Ultra HDR with this metadata, and its decode at full headroom matches
RAWmakase's own reconstruction to 0.002 stop and the master above white to 0.006 stop
median (p95 0.022). Deep shadows carry the 8-bit JPEG base's quantization (p95 0.4
stop below 0.05 of white).

Checked with an independent reader: the Z 8 reference photo's PQ Display P3 export,
decoded by `tifffile` and the ST 2084 EOTF, scores against Camera Raw's own PQ TIFF
exactly as the master does (median 0.061 stop above white), and the master and the
decoded export agree to 0.003. libjxl 0.12's `jxlinfo` reads the JPEG XL exports' header
as written (P3 or Rec. 2100 primaries, PQ or HLG, intensity target), and `djxl`
decodes the lossless PQ export to exactly the TIFF's 16-bit samples.

JPEG XL PQ, HLG and gain-map exports pass `scripts/jxl-interop.py`, an opt-in harness
that exports synthetic fixtures and CC0 photos through the export queue and decodes
them with libjxl 0.12, jxl-oxide 0.12.6, macOS ImageIO and Chrome 156
([record](records/hdr-jxl-interop.md)):
- headers and boxes are as written above;
- the light comes back as BT.2100 defines it: to 0.0005 stop (p95 above white)
  lossless, and 0.01 stop at quality 92 after a 4× area reduction;
- a gain map's base is exactly the SDR export;
- the map's reconstruction follows ISO 21496-1 at every headroom checked, to 0.043
  stop (p95).

Decoder limits that are not the files':
- ImageIO renders saturated HLG colours its own way, as it does the same signal from
  libjxl's own file;
- ImageIO ignores the intensity target;
- jxl-oxide clips a lossy frame's saturated wide-gamut colours;
- Chrome maps HDR images to SDR in canvases.
