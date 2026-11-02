# Scene tone stage

RAWmakase renders the Basic panel's tone controls where Camera Raw does: on scene
values, before the camera profile's look table and tone curve. This page is the
contract for that stage: how it is compared with Camera Raw, what the survey found
about Camera Raw's stage order, what the stage measures from each photo, and the
gates a change to it must pass. Tracking issue: #374 (from #354).

## Pipeline

| # | Stage | Domain |
|---|---|---|
| S0 | Decode, demosaic, highlight recovery of sensor-clipped channels (none for rendered files) | camera RGB |
| S1 | White balance (a rendered file's: a clip and a matrix), the profile's matrix and HueSatMap, Camera Calibration | linear, sRGB primaries |
| S2 | Exposure and the DNG exposure ramp (black point) | linear Rec.2020 |
| S3 | **Scene tone**: Highlights, Shadows, Clarity, Dehaze, the white point and Whites, Blacks | linear ProPhoto, unclamped |
| S4 | Profile LookTable, enhanced look's HSV table, ProfileToneCurve, look curve | as before |
| S5 | Contrast, Levels, parametric and point curves, colour mixer, Point Color, RGB-table looks, grading, gamut, output | as before |

Each stage boundary has a `PixelOutput` tap, so tests can read a render between
stages through the same sampling, masks and caches as the display.

## Comparison contract

All Camera Raw comparisons go through one boundary: 16-bit ProPhoto RGB (gamma 1.8)
TIFFs from Photoshop 2026's Camera Raw (`scripts/corpus/camera_raw.py`, which opens
copies of the source with ProPhoto/16-bit open options), decoded to linear ProPhoto.

- **Stage probes** are synthetic DNGs (`scripts/corpus/probe_dng.py`): RGGB mosaics
  whose camera RGB is linear ProPhoto, with an embedded profile that maps it to XYZ
  D50 unchanged, As Shot white balance neutral, and either a linear ProfileToneCurve
  or a curved one. With the linear curve and no look, Camera Raw's ProPhoto output is
  its scene tone stage's output.
- **Photos at the scene stage**: `scripts/corpus/linear_profile.py` converts a RAW with
  Adobe DNG Converter and rewrites its embedded profile to a linear tone curve without
  a LookTable (keeping the camera's matrices, HueSatMap and baseline exposure), so a
  real photo can be read at the same boundary.
- **Final renders** (photo gates) compare Camera Raw's ProPhoto export, converted to
  sRGB by clipping (relative colorimetric, Bradford D50→D65), with RAWmakase's sRGB
  render of the settings Camera Raw embedded in its export.

For fitting the scene stage, samples Camera Raw clipped, or within 1% of the encoding
limits, are excluded and counted; final-render scores include them. Camera Raw
references are never re-blessed; RAWmakase snapshots and accepted-distance baselines
are re-blessed only per case with a reason.

## Survey (Camera Raw 18.7, Process Version 2012)

### Execution table

Measured with the probes above: each control rendered with a linear and with a curved
profile tone curve; "before the curve" means applying the curve to the linear render
predicts the curved render to the noise floor (encoded MAE 0.0003–0.003), "after"
that the control's curve measured on the linear probe predicts it when applied after
the profile curve. Look order: the same with a DCP LookTable, an XMP HSV look and an
XMP RGB-table look.

| Control | Stage | vs LookTable / HSV look | vs RGB-table look | Colour handling |
|---|---|---|---|---|
| Exposure | S2 (scale), then S3's white point | before | before | RGBTone (white-point rolloff) |
| White point (default) | S3 | before | before | RGBTone |
| Whites | S3, with the white point | before | before | RGBTone |
| Blacks | S3, after Whites (exact composition) | before | before | per channel (negative) |
| Highlights, Shadows | S3 | before | before | luminance gain |
| Clarity | S3 | before | before | luminance gain |
| Dehaze | S3 | before | before | per channel (positive: from the airlight) |
| Texture | before the profile curve (full-resolution detail) | before | before | per channel |
| Contrast | S5, after the profile curve and looks | after | — | RGBTone |

### What the default render does in S3

Camera Raw's default render is not the identity in S3. It maps a white point W* to
output 1, with a shoulder above about 0.25 (scene-linear), and leaves lower values
alone (identity when W* = 1), with the toe of its black point below about 0.02:

- W* is the sensor's white in scene units after Exposure, limited to twice the photo's
  maximum but not below 1. When the sensor's white is below 1 (negative Exposure), W*
  is that white, but not below 0.5. The sensor's white is the level at which a
  neutral's *last* channel clips, times 2^(baseline exposure + Exposure): with the
  first channel's clip the synthetic chart, whose white balance is not neutral, was
  0.02–0.04 too bright at +2 and +3 EV; with the last channel's, its gray ramp is within
  0.001 at Exposure 0 and −1.
- The photo's maximum is that of its brightest channel on a reduced copy (128 pixels
  on the long edge): on a 920-pixel probe a bright spot counts fully from about 8 pixels
  across and not at 2, and twice as large on a probe twice the size. A red highlight
  counts like a white one.
- The black point's toe does not follow the camera's baseline exposure. Engine 4 scaled
  it with the baseline (0.0015 × 2^(baseline + Exposure)), which crushed the deep
  shadows of files with a large baseline.
- The curve family is measured on neutral ramps at quarter stops of W*; above W* = 4
  it repeats in two-stop steps. Predicted from the table and this rule, 60 probes with
  different sensor whites, image maxima, sizes of bright areas and Exposure settings
  are within 0.00026 mean (0.0015 worst) encoded.

Engine 4 clipped at 1 instead: a probe whose sensor white is at scene 4 rendered
scene 1.0 at 1.0, where Camera Raw renders 0.69.

### Rendered files

A JPEG, PNG or TIFF is treated as Lightroom treats it: as Camera Raw's default rendering
of a scene. `develop::rendered` inverts that rendering — the white curve at keys 0 (sensor
white, maximum, Whites level and white point all 1; no black point) followed by the ACR3
default tone curve, both DNG RGBTone — once, when `photo::open_source` opens the file. The
resulting pseudo-scene renders through the raw path with the Embedded profile, whose tone
curve is ACR3. `photo_measures` pins a rendered file's sensor white and maximum to 1, so at
default settings the render is the file (±1/255; finer in 16 bits). S0 highlight recovery
is skipped (`quality::samples::recovered`): a rendered file's bright colours are its
rendering, not clipping.

Measured against Lightroom Classic 15.6 (Camera Raw 18.7) on synthetic charts and 32
photos: Exposure, Whites, Blacks and their combinations within about 1–2/255 on greys;
Contrast with `basic_tone::RENDERED_PIVOT_OFFSET`. White balance is a matrix and a clip
in S1 (`model::white_balance::rendered_white_balance`, see
[colour pipeline](color-pipeline.md)). Known gaps: Highlights/Shadows on very dark or
bright files, Saturation, Vibrance, Clarity, Dehaze, fully saturated colours at large
Exposure, and parts of the cool magenta corner of Temp and Tint.

### Whites and Blacks

- **Whites** is a curve family over scene values, applied RGBTone. Negative Whites
  depends on the white point only; positive Whites stretches toward the photo's own
  maximum: a photo 4 stops below its sensor's white gets most of those stops back at
  +100 (a probe whose maximum is 0.25 maps 0.0625 to white). Two probes with the same
  white point and maximum but different sensor whites differ too, so the table is
  measured over sensor white × the photo's maximum below it × Whites (`white3.bin`).
  On photos the level it stretches toward is not their maximum: a specular highlight
  at the sensor's white does not stop it. For each of 22 training photos the level
  that best reproduces Camera Raw's Whites +100 through the table is, within 0.1 stops
  on average (median 0.06), twice the 99th percentile of the photo's luminance, at most
  its sensor's white; its maximum misses by 1.0 stops on average, the 99.9th
  percentile by 0.6. RAWmakase stretches toward that level; the white point keeps the
  maximum.
- **Blacks** composes after the white point and Whites exactly (encoded error 0.0000:
  Blacks maps their output). Negative Blacks maps the photo's darkest level to black
  when it lies above a fixed black point (about 0.016 scene); even a 4-pixel dark spot
  counts, so the minimum is measured at higher resolution than the maximum. Positive
  Blacks does not depend on it. Negative Blacks works on each channel.
- **The default black** follows the photo's dark scene level, including camera
  baseline exposure and the user's Exposure. Its key is the 0.1th percentile of
  the measurement copy's luminance before red-eye correction and spot removal.
  Positive global Shadows moves this key by its median log-luminance gain on the
  measurement copy. This is a robust approximation to Camera Raw's moving black
  response; using the unchanged key overstated the gain at negative Exposure.
  A mask selects the response at its combined Shadows value locally, interpolating
  prepared curves at steps of 0.1; pixels outside it keep the global response.
  Full-frame masked Shadows matches the global adjustment in Camera Raw probes
  with both Auto and None default-black rendering. Blacks retains its separate
  minimum-based key. On photos that key is almost always below `BLACK_KEYS`'s first
  row (−8 stops), so Blacks renders photos through that row; keying it on the 0.1th
  percentile changes nothing, and a key relative to the median gains under 0.1 ΔE00
  on the train photos ([parity gaps](parity-gaps.md), 2026-10-20).

  Camera Raw 18.7 neutral ramps with the same scene values and different camera
  baselines produce the same dark outputs; raising Exposure moves the response.
  The earlier model subtracted sensor white from the key and used incomplete
  ramps measured at one baseline. Extending each row from its first nonzero
  sample also produced different low-end slopes for rows whose measured portions
  were actually the same curve.

  `default_black.bin` now contains paired DefaultBlackRender None → Auto transfers
  at absolute dark keys −14 to −7, every half stop. Above −7 the response is held
  at the measured saturated response. Samples are monotone; below a ramp's first
  measured input the generator continues its ratio, rather than storing missing
  samples as zero. These remain unmeasured tails, especially relevant to negative
  local adjustments that push pixels below the original scene minimum.

  The white tables already contain the black response of their calibration ramp,
  whose darkest patch is 14 stops below its maximum. The engine composes the
  inverse of that response with the photo's response before uploading the shared
  CPU/GPU correction table. Matching calibration and scene keys gives the identity;
  it does not apply black rendering twice. DefaultBlackRender None retains its
  separate white-curve path.

  Regenerate this family independently with `scene-tone-tables.py
  render-default-black DIR` and `scene-tone-tables.py default-black-table DIR`.
  The regression suite compares complete scene-stage renders across camera
  baselines and Exposure, and checks that correction curves preserve brightness
  order and the calibration identity. This corrects black rendering; it does not
  establish parity for the separate spatial Shadows operator.

### The global rule

Positive Shadows and negative Highlights read a curve computed per photo from the
photo's histogram of base levels (`local_tone_rule.rs`; measured and fitted by
`scripts/corpus/scene-tone-rule.py`, 2026-10-27). The measurement: 1024-pixel
mosaics of flat square patches whose log levels form a chosen histogram, patch
positions permuted so level is uncorrelated with position, rendered with a linear
profile at Shadows +100 and Highlights −100. On them Camera Raw's gain is a function
of the patch's level given the histogram: eight arrangements of the same 16-level
staircase agree per level to 0.06 EV, with no dependence on a patch's neighbours. The
gain scales linearly with the slider (±50 is half of ±100 at every level), and the two
sliders together give about 0.9 of the sum of their gains.

What the rule looks like on filled histograms: Shadows lifts nothing within about 3
stops of the top, then about 0.5 EV per stop of depth, and the darkest half stop a
little less; Highlights is a hump over the top 3 to 4 stops, peaking half a stop
below the top, whose amplitude grows with the photo's range (top pull 0.5, 0.7, 0.9,
1.4 and 3.0 EV at ranges of 2, 4, 6, 8 and 10 stops). The shape of the histogram
matters beyond its keys: a gap under the brightest cluster adds about 0.5 EV of lift
per stop of gap to the cluster below it and pulls the bright cluster harder; a
dark-heavy histogram pulls its highlights more and lifts its darks less than a
bright-heavy one. No percentile-keyed table reproduces this: on the same mosaics the
previous tables missed by 0.2 to 0.7 EV root mean square, and by 1 to 3.6 EV on gapped
ones.

Deeper down, the Shadows lift keeps growing: on mosaics spanning 10.5 to 13.5 stops
it is 3.0 EV at 7 to 8 stops under the top, 3.7 at 9 to 10, 4.25 at 11 to 12 and
saturates near 4.3 EV at 12 stops and below (2026-10-28). How much a level is lifted
at a given depth still depends on the rest of the histogram, by 1 to 2 EV: with most
of the frame 9 to 12 stops down and the rest 2 to 5 stops down, Camera Raw lifts the
deep cluster 3 to 4.8 EV and the mid cluster only 0.1 to 0.3, where a filled
histogram lifts the same mid levels 0.9 to 1.4 EV. Highlights −100 pulls a patch
clipped in every channel, one clipped in green only and one just under white all
alike, −0.8 to −1.3 EV depending on the histogram: clipping as such does not hold
whites, so a clipped sky that renders darker than Camera Raw's is a reconstruction
difference ([parity gaps](parity-gaps.md#the-global-rule-2026-10-28)).

A perfectly flat maximum is a degenerate case for Camera Raw. Texture of 0.03 stop on
the brightest patch alone adds about 0.4 EV of Shadows lift to every dark patch and
removes the two-valued Highlights collapse; texture elsewhere, or a brighter spot,
does nothing. Flat charts and two-valued probes therefore never predicted photos; the
training mosaics carry a faint blob texture, and the rule's two texture inputs are
clamped so it cannot extrapolate on amplitude.

The rule is an ensemble of four small networks (24 inputs, two hidden layers of 64,
two outputs; fitted from different seeds and averaged, since one fit's gain at a level
varies by 0.1 to 0.3 EV with the seed on real photos and the average lowers held-out
error in every depth band) from level-relative descriptors of the histogram: the
level's distance below the top (the maximum of the base) and above the bottom (0.1th
percentile), from the mean and five quantiles, the cumulative fraction at the level,
the range, the mass in eight bands of stops around the level, two global texture
measures, the fine and coarse texture of the pixels at the level itself, and the
fraction of the frame within one and within two stops of the top. The texture pair
is what a histogram cannot see: Camera Raw lifts a smooth dark region far less
than a rough one at the same depth (on dark regions under a fixed sky, 1.95 against
3.21 EV seven stops down and 2.71 against 4.11 ten stops down; broad shapes count as
texture too), and a rule without it over-lifted a dusk freeway's smooth sky and road
by 0.4 EV. The measure is the median absolute residual of a 2-pixel and a 12-pixel
box mean over the pixels within half a stop of the level, since an RMS is dominated by
the few edge pixels whose residual spans stops. The last pair tells the rule how
sparse the top is: the top is the maximum, so a clipped spot of a fraction of a
percent of the frame sets it, and on mosaics a square of 0.2 to 12% of the frame one
or two stops over the rest adds only 0.05 to 0.09 EV to Camera Raw's lift of the dark
patches, whether the square is unclipped, clipped in one channel or in all three
(2026-10-30). Without the pair every deep level of such a photo read as farther below
the top than Camera Raw lifts it, by up to 2 EV on a sunlit field under a clipped sky.
It is trained on 496 mosaics (random
histograms at fixed and random texture, histograms sampled from the training photos'
base levels, small bright and small dark minorities above or below a gap, a few tiny
bright spots in a dark field, deep ranges, bimodal deep-and-mid histograms, clipped
top patches, smooth skies of varying texture and extent over textured ground, dark
regions of varying texture under a fixed sky, coloured patches of equal
luminance, and clipped squares and patches under a non-neutral white balance, so that
they reconstruct above the white as a photo's do) and on 283 training photos: the
221 of all three families' training sets and 62 more CC0 raw.pixls.us photos chosen
for small clipped spots, skies over deep ground, large bright areas and heavy clipping,
rendered with the linear profile, with targets counted down to 13 stops under white:
a first fit that stopped at 9 stops had never seen the deepest tones and lifted a
sunlit cabin's floor 2 EV where Camera Raw lifts it 3.7. The inputs come from the engine's own
measurement map of each probe and photo (`RAWMAKASE_RULE_DUMP`, the `taps` step): a
re-implementation of the map differed in its dark tail, since the engine averages
signed camera values before the matrix, and a rule fitted on the re-implementation
over-lifted bright-keyed photos by half a stop. Evaluated once per photo at 48 base
levels, it gives a `Curve` the per-pixel code, masks and the GPU read like the tables;
levels above the top or below the bottom hold the ends, and the output (level plus
gain) is projected onto nondecreasing values as the fitted tables are, so a clipped
plateau keeps no bright rim.

Camera Raw renders of linear-profile DNGs need `CameraProfile` set to the embedded
profile's name, or Camera Raw silently uses Adobe Color; with it, RAWmakase's scene
output equals Camera Raw's on real photos to 0.00 EV at default and at Exposure −3.

Scene gains against Camera Raw on the 221 training photos (per map pixel, EV root
mean square, median over photos): held out by photo the ensemble sits at 0.17 (Shadows
+100) and 0.12 (Highlights −100), where doing nothing is 0.78 and 0.41. Final renders
on the train sets against main before the rule, median ΔE00: Shadows +100 2.12 → 1.53
(7 of 14 photos better, 0 worse), +50 1.19 → 0.97, Highlights −100 1.53 → 1.27 (6
better, 0 worse), −50 1.12 → 1.01. The validation gate's figures are in [parity
gaps](parity-gaps.md#per-level-texture-and-the-ensemble-2026-10-28).

Measured and not in the rule: the pull and the lift are colour-blind (at equal
luminance Camera Raw pulls neutral, blue, orange, green and cyan patches alike at
Highlights −100 and keeps their saturation), a sky's pull grows only mildly with its
cloud texture (0.3 EV from faint to strong, saturating) and strongly with how small
its share of the frame is (−2.4 EV at a quarter of the frame five stops over the
ground, −0.7 at two thirds), which the histogram already carries.

Not in the rule: Camera Raw's spatial regime. Textured bright squares smaller than
about 0.4% of the frame in a dark field are pulled by their size (an 8-pixel square
at 1024 −2 EV, 32 pixels −6 EV, 7 stops over the field), and the same pixels arranged
as scattered gaps or as a strip lift a dark scene differently (0.35 EV on the
scene-probe test). Textured dark squares in a bright field, by contrast, are lifted
by their level alone (depth 5: 1.8 to 2.4 EV from 16 to 512 pixels). The rule reads
such scenes through the histogram alone: the shadow-gap probe lifts 0.4 EV against
Camera Raw's 0.6.

### Local operators

Shadows, Highlights and Clarity are luminance gains from a smooth, edge-aware base
level: on photo-like synthetic scenes their gain is flat within regions whatever their
texture, and stops at edges. Their strength follows the photo: a dark square is lifted
+0.9 EV by Shadows +100 in a mid-gray scene and +1.6 EV in a bright one, and dark areas
of a dark scene are hardly lifted; Highlights −100 pulls the brightest area of a dark
scene down by up to 5.8 EV. With an ideal key per scene, one gain curve of the base
level relative to it explains Shadows +100 to 0.056 EV and Highlights −100 to 0.050 EV
on 40 scenes; keys predicted from the photo's percentiles (Shadows: 0.25·p99.9 +
0.75·p99, Highlights: halfway between p75 and p1 of log luminance) leave 0.17 and 0.11
in cross-validation, against 0.42 and 0.29 for doing nothing. A second dimension (the
pixel against its base) did not help in cross-validation.

On photos the synthetic Shadows tables did not carry over as well as Highlights'. On
22 training photos (cross-validated by photo, luminance gain in EV), Shadows' tables
fitted on the photos themselves against the photo's mean luminance (log2 of the mean
of the map's luminance) leave 0.12 at +100 and −100, against 0.25 and 0.24 for the
synthetic tables and 0.43 and 0.34 for doing nothing; the 90th percentile key does as
well. Highlights' synthetic tables match the photos' luminance gain (0.09–0.10 EV at
±100), but positive Highlights renders closer to Camera Raw with tables fitted on the
photos: rendered with tables fitted on the other half of the photos (two folds, 22
photos), median ΔE00 at +100 is 1.80 against 1.91 for the synthetic tables, at +50 1.57
against 1.69. At −100 the photo tables gain nothing (1.63 against 1.64), so negative
Highlights keeps the synthetic tables. The photo tables are measured through the
engine's own global curves, so they were refitted once the default black (below) was
added: the earlier taps lacked it and the fit had taken Camera Raw's black for Shadows'
gain (median ΔE00 on the training photos, Shadows +100 1.55 → 1.53 and +50 1.16 → 1.11,
Highlights unchanged within 0.02); the cross-validated figures above are from the earlier
taps.

**Brightness ordering of measured tables.** The fits' smoothness penalty does not
prevent a decreasing output curve, especially where the clipped-reference exclusion
leaves few samples. The original Highlights −100 tail had output slopes down to
−2.28 (log luminance out per log luminance in), turning bright centres darker than
edges. Smaller violations occurred at Highlights −50, Shadows −100, positive
Shadows' regional +100 row and Dehaze −100.

`Family::monotone` projects each row's **input + log gain** onto nondecreasing
values with pooled adjacent violators. This is the least-squares correction in
output log luminance; it leaves unaffected bins unchanged and permits flat
compression, rather than inventing a minimum contrast. It runs at compile time on
the shared data, so slider interpolation, masks, CPU rendering and GPU uploads
read the same corrected rows. Generated tables retain this projection. It constrains
each scalar response, not spatial ordering between pixels with different local
bases, nor every composition of controls. Clarity's spatial contrast is not such
a scalar response. The existing scene-probe baselines are not changed.

The engine snapshot for `synthetic-d65 / highlights-100` is updated for this
intentional correction: the offending bright neutral patches become darker
(largest change ΔE00 3.51; chart mean 0.058). Only that engine snapshot case is
updated; Camera Raw references and parity-distance baselines remain unchanged.

The clipped-spot references also show that Camera Raw has a modest spatial edge
lift of its own. Passing the ordering and rim tests does not establish an exact
match for highlight brightness in every scene; isolated highlights in very dark
scenes remain an approximation. Cloud contrast at Exposure 0 can also remain
flatter than Camera Raw after the ordering and partially clipped recovery fixes.
These regressions protect against solarization, not all differences in highlight
recovery or the strength of Shadows in dark scenes.

**Positive Highlights' bright end.** The photo tables have few samples far above the
key: in a dark photo with a bright object, the region 5–7 stops above the key, the
+100 table lifted 1.09 EV at +5, 0.50 at +6 and 1.03 at +7, a fitting artefact. Positive
Highlights now takes each bin from the photo table weighted by its photo samples
(count / (count + 20,000), smoothed over five bins) and from the synthetic scenes' table
for the rest, which carries the lift on smoothly (+100: 1.43, 1.56, 1.94 EV at +5, +6, +7)
and is the photo table below +4. Training photos render as before (median ΔE00 at +100
1.37, at +50 1.12 against 1.13); on validation Highlights +100 now passes on mid photos.

Still open on low-key photos: a dark photo's bright object a couple of stops above the key
(the moon in a night sky, 2.2 stops above it) is lifted about 1.6 EV by Camera Raw at
Highlights +100, as bright spots two stops above a dark surround are on the probes, and
0.35 by the table, which on the training photos only meets such levels near white. A
second table of the base level itself (headroom to white) helps in cross-validation
(Highlights +100 typical error 0.061 → 0.041 EV) but predicts 0.36 for the moon: the
training photos have no such scene. Highlights −100 on bright photos is short of the
rule's material improvement (1.53 against engine 4's 1.67). A median key for Highlights
fitted the photos' scene gain better in cross-validation at every position but rendered
the training photos worse (+100 1.37 → 1.44, −100 1.30 → 1.41), so the key is unchanged.

**Negative Highlights past the table.** The table spans base levels up to 7 stops over
the key, and past it each row used to hold its last column, so Highlights −100 stopped at
about −3.5 EV. On synthetic probes (a flat object 2–10 stops over a flat background,
clipped or just under the clip: clipping makes no difference) Camera Raw keeps pulling:
−6.6, −7.9, −9.1 and −10.2 EV at 7, 8, 9 and 10 stops, about 1.2 EV more per stop, and
at −50 and −25 about 0.5 and 0.25 more per stop. Past 7 stops the −100 row now continues
with the slope of its last 7 columns (`Family::tail`, −0.95 EV per stop over the key
after `monotone`, whose steps are never steeper than −1, so the output stays
nondecreasing); the other rows hold their last column, and positions between −100
and −50 take their share of the tail. On the six
highlights validation sets (56 photos) this left −50 and −25 unchanged and improved −100
in the photos with such areas, most in low-key photos with blown lights (the worst region
of three photos 17–18 ΔE00 better, the worst regional loss 0.8). Extending −50 and −25
with their own slopes, even scaled to Camera Raw's ratios, made regions of two photos up
to 2.4 ΔE00 worse, so they hold. The probes' pull at 2–6 stops is about twice the
table's, which is fitted on photos, where it is right; flat probes are not photos there.

The tail reads a finer level than the table. Camera Raw's pull depends on the object's
size, which the first probes (one 12% object) did not show: on a 7-stop block in a dark
1024-pixel frame, −100 pulls a 4-pixel square −1.1 EV, 8–32 pixels −2.8 and 64 pixels
and up −6.4. In one photo (a window with blinds in a dark kitchen) Camera Raw also left
the bright slats nearly untouched: its mask of reference pixels at L* ≥ 90 averaged
L* 92 where the main branch rendered 60 and the first tail 53, flattening the slats to
one grey (that mask's ΔE00 went from 23.1 to 29.5). A guided base follows contrasty fine
structure pixel by pixel, so a steep tail turns its detail into a plateau. The tail now
reads the base capped at 0.5 stops over the mean of log luminance in a 5 × 5 window of
the map, taken as the largest such mean within the same reach (`TAIL_RADIUS`,
`TAIL_MARGIN`; the `LOCAL_FINE` grid on the GPU): unchanged on a block or a lamp,
lower on stripes and specks. Without the largest-mean step the block's first two map
pixels, whose window includes the dark frame, were pulled up to 3 stops less than its
inside, a bright rim. A larger window (9 × 9 and up), or no
margin, gave back some of the gain of the lamp and ceiling-light photos. This follows
Camera Raw's size dependence only where it mattered in the photos: squares of 4 and
16 pixels are pulled −0.3 and −2.3 EV against Camera Raw's −1.1 and −2.9, and a large
block −3.6 against −6.4.

**Positive Shadows: the two-scale lift.** On the training photos Shadows +100 left the
darkest display tones (L* below 15) 5–7 L* too light, almost all of it from small dark
areas (0.2–3.5% of the frame) inside brighter surroundings, which Camera Raw lifts less
than large dark areas at the same base level. A second level fixes most of it: the
*regional level* K, a bilateral blur of the map's log luminance much wider than the base
(σ 25% of the long edge, range σ 4 stops; `clarity.rs`'s bilateral grid at one scale).
Positive Shadows' gain is a table of the base level less K plus a table of K less the
Shadows key (`SHADOWS_DETAIL`, `SHADOWS_REGION`), fitted together by least squares on the
training photos at the renders' size (1024 pixels), with the region table held at 0 at
the key (a constant could otherwise move between the tables). K is read from the map like
the base level, so region and preview size do not change it. Negative Shadows keeps the
base-level table: an earlier two-scale fit made Shadows −100 worse on validation (bands
20–40 L* about 3.5 too light on bright photos). Masks read the same three tables at their own
slider value.

Chosen on the training photos alone (cross-validated by photo, 2 and 4 folds, EV of scene
gain): at Shadows +100 the error falls from 0.248 to 0.163 and in the deep shadows (base
4 stops below the key) from 1.06 to 0.39, at +50 from 0.123 to 0.082, and Shadows +50
with Highlights −50 from 0.161 to 0.117. A plain Gaussian regional level does most of it
(+100 0.185, +50 0.094); narrower edge-aware ones (σ 8–15%, range 2) less in the deep
shadows. The validation gate was also used during development (seven runs of earlier
fits and settings; of σ 25, 35 and 50% at range 4 it preferred 25%, 37, 38 and 40 failing
setting/stratum pairs against 40 before). With these tables Shadows +100 passes in all
three strata: its darkest band goes from 5.8 to 2.2 L* too light on mid photos and
6.3 → 0.5 (10–20 L*) on bright ones, with Adobe Color 4.8 → 1.7 and 9.1 → 1.8; per photo,
the band 0–10 L* of photos whose darkest tones are under 3% of the frame goes from 2.2–14.1
to −0.2–6.1 L*. Still failing: Shadows +100 with Adobe Color on bright photos
(median 1.56 against the rule's 1.53) and Shadows +50 with Highlights −50 on one mid photo
(Sony A6100: 0.94 with engine 4, 2.38 here, as before this change; each slider alone is
within 1.3), where Camera Raw's Highlights darkens bright areas less when Shadows is raised
than the sum of the two gains does.

**Sparse highlights in dark scenes.** The mean-only Shadows key under-lifts
large dark areas when small bright gaps are scattered through the image. The same
input histogram arranged as a bright strip needs a different correction in Camera
Raw. Positive Shadows now adds a calibrated scene-context offset to its regional
key; negative Shadows and Highlights keep their keys. Ten log-luminance percentiles
and luminance/texture distributions at four spatial scales distinguish these cases.
All statistics are relative to mean luminance, so increasing Exposure does not
activate the correction. The existing measurement copy defines the spatial scale;
CPU, GPU and masks share the corrected key.

The calibration contains 137 anonymous contexts (22 photos, 115 synthetic scenes).
`data/shadow-context.json` keeps their 34 statistics and squared gain-error curves
for offsets −2..2 EV, without pixels, profiles or private paths. Regenerate the
runtime table with `python3 scripts/corpus/shadow-context.py`; `--check` checks it
without rewriting it. The minimum includes a weak 0.002·offset² penalty. Smooth
inverse-distance weighting, with bandwidth set by the third neighbour, interpolates
these bounded offsets without switching abruptly between reference scenes.

The reported tree photo is explicitly a calibration case. Independent checks include
other photos and procedurally generated texture/spatial arrangements. The descriptor and interpolation were selected with grouped cross-validation
on the original 21 usable photo maps and synthetic seeds; the separate texture
probes were withheld. Calibration errors are approximations, not a claim to
reproduce Adobe's proprietary algorithm.
Regression tests compare actual rendered lift with fixed Camera Raw measurements,
including Exposure −2/0/+2, default black Auto/None, and identical pixels arranged
as scattered gaps versus a strip. Camera Raw references are not regenerated to
accommodate the engine.
The engine snapshots for ten positive-Shadows chart cases are refreshed. Against
unchanged Camera Raw chart references, Shadows +100 mean ΔE00 improves 6.64 → 5.21,
+60 improves 4.11 → 3.18, and the look at Amount 100 improves 1.58 → 1.13. One mixed
case is worse: warm white balance, Contrast +25, Highlights −40 and Shadows +30
moves from mean/p95 1.35/3.33 to 1.56/4.15. Only that case's accepted-distance entry
is updated; the general tolerances stay unchanged. Combined-control parity remains
an approximation; this tradeoff must not be described as universal parity.

What did not work, on the same photos:

- *A second table dimension for the pixel against its base* (Camera Raw does lift
  darker-than-their-base pixels less; cross-validation 0.66 → 0.52 EV in the deep shadows,
  display 6.6 → 5.0 L*), pixels below 2^-10 in the fit, smoother bases (guided ε 0.03–8,
  radius 1.6–6%), the default black before the gain, and a key from the brightest levels
  (0.25·p99.9 + 0.75·p99) for the regional table (cross-validated error doubled).
- *A full-resolution fast local Laplacian filter* (Paris, Hasinoff, Kautz 2011; Aubry et
  al. 2014) on log2 luminance with the Shadows curve fitted through it (the output is
  linear in the curve), with the curve's value at the reference level inside a detail
  threshold of 1–3 stops and edges following the curve, with and without a fitted detail
  slope per level. It reproduces the small dark areas (darkest band about 0 L* on those
  photos at a 2-stop threshold) but is blotchy: within the threshold, neighbouring regions
  share one gain where Camera Raw lifts them separately, and the median ΔE00 at Shadows
  +100 rose from 1.53 to 1.98 (1.64 with the detail slope). Its scale behaviour is also
  the reverse of Camera Raw's on synthetic probes (below). It follows the published
  method, not Adobe's code or patents, and is not in the engine.

**Probes.** Single squares 4 stops below a flat 0.18 surround, Shadows +100, rendered at
1024 and 4096 pixels: perfectly flat squares of 8–32 pixels (at 1024) are lifted 2.2 EV,
64 pixels 0.95, 128–256 pixels 0.3–0.5, with no halo and the same gain over the whole
square; the size counts relative to the image (4096-pixel renders match at four times
the pixels), and squares below about 6 pixels are not lifted at any size. The same dark
area scattered as 16-pixel spots is lifted 1.7–1.9 EV, so the dependence is spatial, not
on the histogram. With texture (±0.3 EV noise or ±0.5 EV blobs on everything) a 256-pixel
square is lifted 2.4–2.5 EV: the weak lift of large regions holds only for perfectly flat
areas, and is why the synthetic chart's flat patches stay far from Camera Raw at
Shadows ±60 and ±100 (mean ΔE00 4–7). On bright spots in a dark surround, Highlights ±100
is size-independent from about 8 pixels. Clarity's weights keep the percentile key they
were fitted with.

### Dehaze

On synthetic scenes Dehaze works on each channel by its level relative to the photo's
bright end (the 99th percentile of luminance): no change at the top and about −0.5 EV a
few stops below at +40. A curve per amount explains ±40 to 0.10 (doing nothing:
0.36–0.64) and +100 to 0.46 (1.22) in cross-validation; the rest is spatial. On the 22
training photos the same curves fitted on the photos themselves do better at every
amount (cross-validated by photo, per-channel gain in EV): +100 0.54 against 0.69 for
the synthetic curves and 0.90 for doing nothing, +40 0.16 (0.18, 0.30), −40 0.14 (0.16,
0.57), −100 0.34 (0.39, 1.45). **Negative Dehaze** uses these photo-fitted curves. In Camera
Raw's renders of the train photos it is rather a veil: sized by the photo's median luminance,
coloured like the airlight, fading toward black, with a contrast change per photo (display
slope 0.34–1.33) that follows the airlight's brightness. A veil model without that per-photo
contrast term improved the median but made 11 of 26 photos worse ([parity
gaps](parity-gaps.md), 2026-10-20).

**Positive Dehaze** is a dark-channel haze removal model (`scene_tone/haze.rs`), on the
scene tone stage's input (linear ProPhoto), measured on the photo's measurement copy (512
pixels on the long edge) at Exposure 0:

1. The dark channel: the minimum of the darkest channel over a (2r+1)² window, clamped
   at the borders, with r = max(1, round(0.005 × the copy's long edge)) (3 at 512). The
   airlight A, per channel, is the mean of the pixels at or above the dark channel's 99th
   percentile (sorted index ⌊(n−1)·0.99⌋).
2. The haze's density: the same window minimum of min_c(I_c / A_c), smoothed by a guided
   filter with itself as the guide (box radius 2r, ε = 0.01).
3. A pixel p at Dehaze s > 0 (the slider plus the masks' Dehaze there) becomes
   J = (p − g·A) / t + g·A, with t = max(1 − ω(s)·density, 0.3), the density read
   bilinearly at the pixel's position. The gain g includes 2^Exposure, mask Exposure,
   and the Shadows/Highlights/Clarity gain already applied to p. Transforming the
   airlight consistently prevents negative Highlights from turning the sky black
   when combined with positive Dehaze (#600). The density measurement and
   interpolation stay independent of those tone edits. ω is linear between (0, 0),
   (+20, 0.3), (+40, 0.45) and (+100, 0.9). No channel falls below half of p scaled by
   the luminance's change, so weak channels of bright saturated colours are not driven
   to black (on the training photos: +100 3.06 → 3.04; a floor of 0.7 costs 3.18, 1.0
   costs 4.53). The transmission floor 0.3 costs nothing there (3.06 at 0.1 to 0.3).

The model's few parameters (the window, ε, the transmission floor and ω) were chosen by
grid search on the 22 training photos against Camera Raw 18.7, by display ΔE00. Median
over the photos, against the curves above: +100 5.97 → 3.06, +40 2.73 → 1.68, +20 1.42.
A and the density read exactly what the photo's measures read (white balance, the
profile, calibration, the camera's exposure), so they are kept with them in the stage
cache, measured the first time a render has positive Dehaze somewhere; Exposure scales A
with the scene.

The model is fitted to photos; on synthetic inputs it does less well than the curves.
On the stage probes at +40 the error of the effect grows on all three scenes (0.048 →
0.110, 0.069 → 0.152 and 0.036 → 0.044 EV), and on the synthetic chart it darkens Dehaze
+100 more than Camera Raw (p95 ΔE00 11.3 → 17.1, while the mean improves 6.9 → 5.9):
its wide-gamut cyan patches lose their red, which Camera Raw only reduces, and its dark
gray field darkens strongly.

### Masks

A full-frame mask's Shadows, Highlights, Dehaze, Clarity and Contrast render like the
global sliders in Camera Raw (within 0.006–0.012 encoded, against effects of
0.03–0.12); its Whites and Blacks do not (Blacks +50 differs by 0.07 for an effect of
0.013). Lightroom stores a mask's Exposure normalised: `LocalExposure2012` 1 is +4 EV
(0.8 rendered as +3.2 EV).

## Testing

- **Clipped-highlight regressions (CI)**: `clipped_highlights_do_not_solarize`
  decodes a synthetic clipped plateau with a smooth radial edge on backgrounds
  five, seven and nine stops darker. Highlights 0, −50 and −100 run through both
  global XMP settings and a full-frame mask. Rim/centre luminance ratios must not
  exceed Camera Raw 18.7's measurements by more than 0.05. References are in
  `camera-raw/highlight-spots.json`, reproduced by
  `scripts/corpus/highlight-probes.py`; they are independent references, not
  blessable engine snapshots. The scalar curve test also sweeps every integer
  slider position, including interpolation and the clamped extensions, and checks
  global, per-pixel and GPU-uploaded tables. The hardware test
  `gpu_measured_tails_match_cpu_globally_and_in_masks` checks actual GPU evaluation.
- **Tier 1 (stage probes, CI)**: `scene_probes_do_not_regress` (`tests/color/scene_probes.rs`)
  renders synthetic probes to the S3 output tap (`develop::quality::render_stage`) and
  compares them with Camera Raw 18.7's renders of the same probes
  (`tests/corpus/camera-raw/scene-probes.json`, from `scripts/corpus/scene-probes.py`):
  white ramps at W* = 1, 4 and 16 (default, Whites ±50 and ±100), Exposure −2, −1 and +1
  on ramps at W* = 1 (clipped) and 4 (overrange), black ramps with darkest levels 2^-4
  and 2^-8 (Blacks ±50 and ±100), a full-frame mask's Whites +50 and Blacks −50, and
  three synthetic scenes (`probe_scenes.py` seeds 1, 3, 4, committed reduced to 384×256)
  at Shadows +100/−50, Highlights −100/+50, Dehaze ±40 and Clarity ±50. Ramps are scored
  by patch means, scenes by 24×16 block luminances; each case records its encoded (gamma
  1.8) mean and largest error and, for scenes, the mean error of the setting's effect in
  EV in `scene-probes-baseline.json`. A case fails when it gets further from Camera Raw
  than its baseline (5% plus 0.0005 mean, 0.003 largest, 0.005 EV effect); intended
  changes are blessed per case with `RAWMAKASE_BLESS=1`.
- **Tier 2 (structural, CI)**: stage placement (the full render equals the downstream
  stages replayed from the S3 tap), CPU/GPU equivalence per stage, cached against fresh
  renders, photo measures equal across preview sizes, regions and export.
- **Tier 3 (photo gates, private)**: below.

## Photo gates

`scripts/corpus/gate.py` holds the frozen metric and the acceptance rule. Photos,
references and renders stay outside the repository.

**Sets.** Train (fitting statistics), validation (accept or reject during
development) and holdout (scored once, at the switch decision), fixed before fitting,
from raw.pixls.us CC0 RAWs and the owner's RAWs. Each is stratified by scene
brightness: the 90th percentile of L* of Camera Raw's default render (Adobe
Standard), below 40, 40–75, above 75. One photo per camera model, so a scene never
appears in two sets.

**Settings.** Each percentage slider (Contrast, Highlights, Shadows, Whites, Blacks,
Texture, Clarity, Dehaze) at ±25, ±50, ±100; Exposure at −2, −1, +1, +2 EV; Exposure −1
with Whites +100, Shadows +50 with Highlights −50, Contrast +50 with Blacks −50,
Dehaze +40 with Clarity +40, a gradient mask with Exposure +2 EV and Shadows +50 (0.5 and 0.5 in Lightroom's normalised local units); and,
with Adobe Color, the default render, Shadows +100, Dehaze +40 and Whites +100 (#354).

**Metric.** Both renders reduced by area to 1024 pixels on the long edge. Per-photo
score: median ΔE00 over pixels (secondary: mean). A setting's score over a set: the
median of the per-photo scores (secondary: mean). Signed ΔL*: mean of RAWmakase −
Camera Raw L* over pixels whose Camera Raw L* falls in a 10-wide band, pooled over a
stratum's photos; bands with less than 1% of the pixels are skipped. Clipping
disagreement: the fraction of pixels where exactly one render is at white (any
channel ≥ 0.995) or at black (all ≤ 0.005). Valid pixels: the common frame less a
3-pixel border. Final-render scores include clipped pixels.

Metric policy 2 additionally retains mean ΔE00 for every overlapping spatial
window (edge 1/16 of the valid frame's long edge, half-window steps, including
the far edges). Corresponding windows are compared individually: a bad region
elsewhere cannot hide a new defect. Reference-defined highlights (L* ≥ 90) and
shadows (L* ≤ 10) are also scored separately whenever they contain at least 16
pixels, even below 1% of the frame. These use unsigned colour distance, so
light/dark or colour errors cannot cancel. Any non-finite pixel invalidates a
photo rather than being silently discarded.

All acceptance modes require three scored photos in **every** brightness
stratum for every declared setting. Missing strata, missing settings, unscored
references and inconclusive results fail, including in `dataset-gate.py`.
Every corresponding spatial or tonal region must stay within baseline +2 ΔE00;
every photo's clipping disagreement must stay within baseline +0.5 percentage
points, independently of the stratum average. These checks supplement, not
replace, the clipped-plateau and monotonicity regressions above.

`--rule regression` protects the preceding engine's measured behaviour.
`--rule parity` adds absolute requirements on **each photo**, including defaults:
median ΔE00 ≤ 2, every spatial/tonal region's mean ΔE00 ≤ 5, and clipping
disagreement ≤ 0.5%. These are initial engineering acceptance budgets, not
experimentally established perceptual-equivalence thresholds. They deliberately
do not become looser when the baseline is bad. Review regional crops and retain
full-resolution/detail tests before claiming Lightroom parity. Sharpening, noise
reduction, lens corrections and errors outside sRGB remain outside this gate.

Scores made before metric policy 2 cannot pass acceptance or resume a new metric
run: they do not contain regional errors. Rescore retained renders, or render
the same pinned baseline and candidate binaries into new run folders when the
renders were deleted. Keep Camera Raw references and old scores unchanged.

**Switch rule** (`--rule switch`, fixed before fitting). For every setting, on validation and then
holdout, in every brightness stratum:

1. **Material improvement**: the setting's score ≤ the default render's score + 1.0,
   or ≤ 60% of engine 4's score for that setting.
2. **Bands**: |signed ΔL*| ≤ 3 in every band, or ≤ 50% of engine 4's in that band.
3. **No photo regresses**: per-photo score worse than engine 4's by at most 1.0 for
   settings whose largest slider magnitude is ≤ 50 (Exposure ±1 EV counts as 50,
   ±2 EV as 100) and 2.0 above that.
4. **Endpoints**: clipping disagreement ≤ engine 4's + 0.5 percentage points.
5. **Default render** (all sliders 0): score no worse than engine 4's + 0.1.

Settings where engine 4 is already within the default render's score + 0.5 need only
rules 3–5. A stratum with fewer than 3 photos, or a photo with less than 50% valid
pixels, is inconclusive, which fails the gate until data is added. Any exception
needs the owner's explicit, documented approval per setting. A holdout is scored
once; if it fails and its errors are inspected, it becomes validation data and a
fresh holdout is frozen before further fitting. Adaptive statistics are identified
with controlled synthetic probes before fitting; no offsets are fitted to photos.
Engine 4's photo results are frozen first, as the comparison baseline.

### Default-black calibration validation

The paired black-response calibration corrects the baseline-exposure dependence
and is checked against fixed Camera Raw ramp outputs. Updating the engine
snapshots records this intentional rendering change; the Camera Raw reference
pixels remain unchanged. In scene 3, final-output mean absolute errors improve
for Contrast +50 (0.0074 → 0.0038), Dehaze +40 (0.0118 → 0.0090) and
Dehaze −40 (0.0107 → 0.0064). Their adjustment-relative errors increase from
0.064 to 0.074, 0.223 to 0.264 and 0.069 to 0.092 EV respectively because
the default rendering also changes. These are accepted calibration tradeoffs,
not evidence of complete Camera Raw parity. Shadows +100 on this scene changes
slightly in the other direction (output error 0.0023 → 0.0026). The weak
Shadows response in scenes with scattered bright gaps remains a separate issue.
