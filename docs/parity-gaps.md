# Remaining Lightroom parity gaps

Status: 2026-10-05; tone research updated 2026-10-16 for the [scene tone stage](scene-tone-stage.md); lens, profile and catalog notes updated 2026-10-03. RAWmakase renders close to Lightroom but not identically. Each item below says what differs and, where measured, by how much. Errors are encoded-sRGB mean absolute error (0–1) against Camera Raw 18.6 or Lightroom Classic 15.5 exports, scored with `scripts/lightroom-scorecard.py`; most of the entries below are mean ΔE00 against Camera Raw 18.7 renders of the synthetic charts. For scale: Lightroom X100F references averaged 0.0090, Sony A7 II Camera Raw references 0.0065. The scene tone stage changed the default render and every tone control, so figures measured before it (dated 2026-10-05 and earlier) may have moved. Details: [tone controls](tone-controls.md), [color mixer and grading](color-mixer.md), [lens corrections](lens-corrections.md), [transform](transform.md).

## Measured and matched (for reference)

These controls were fitted to Camera Raw renders and match within the default-render error, or close to it: default look without Adobe files (DNG ColorMatrix + ACR tone curve), Contrast, color mixer, Saturation, color grading at default Blending/Balance, Transform sliders, built-in Fujifilm, Olympus/OM System, Panasonic and DNG lens corrections, imported Adobe lens profiles (on Sony A7 II photos), DNG embedded profile/exposure/crop, and Fujifilm default crop.

## Tone

- **Scene tone stage**: Exposure, the default white and black point, Whites, Blacks, Shadows, Highlights, Clarity, Texture and Dehaze render on scene values before the profile, fitted on linear-profile Camera Raw 18.7 renders ([scene tone stage](scene-tone-stage.md), [tone controls](tone-controls.md#whites-and-blacks)). Positive Shadows and negative Highlights follow the measured [global rule](scene-tone-stage.md#the-global-rule) (2026-10-28): a curve per photo from the histogram of its base levels, which the earlier percentile-keyed tables could not express. Negative Shadows, positive Highlights, Dehaze (a curve per amount, no spatial part) and positive Whites (+100 especially) stay fitted and still differ. Shadows and Highlights on the synthetic chart's flat patches are a degenerate case for Camera Raw (a flat maximum) and are not a measure of either engine.
- **Contrast** follows Camera Raw's curve, after the profile and so after Whites and Blacks, at a pivot predicted from the photo ([tone controls](tone-controls.md#contrast)); the prediction is within about 0.03 of the pivot that fits each photo best, and its extra error over the default render on photos is 0.003–0.007.
- **Clarity** of both signs follows a model fitted to Camera Raw's scene-value renders of synthetic scenes ([tone controls](tone-controls.md#clarity)); its residual is not modelled. **Texture** follows a model fitted to Camera Raw on synthetic charts ([tone controls](tone-controls.md#texture)): on the corpus chart its ΔE00 to Camera Raw drops from 2.0–2.2 to 1.5–1.6 at ±100.
- **Parametric tone curve** renders as Camera Raw 18.7 does on the chart ([tone controls](tone-controls.md#parametric-curve)): the 24 `parametric-*` cases sit at mean ΔE00 0.8–3.0, down from 0.95–7.9, and the looks carrying one at 0.9–1.6, down from 1.7–4.9. With out-of-gamut colors clipped per channel as Camera Raw does ([out-of-gamut colors](color-pipeline.md#out-of-gamut-colors)), the cases sit at 0.48–1.15. A look's own parametric curve is a second curve after the user's, as in Camera Raw (`look-parametric-darks+30`: 0.93, was 1.41); at Amount 200 Camera Raw's look curve does about 1.6 times its effect at 100, where RAWmakase doubles it. **Point curves** match on ramps, but saturated colors under an S master curve still differ (ramp MAE 0.0024, peak 0.11).
- **Curve Refine Saturation** renders as Camera Raw 18.7 does on the chart ([tone controls](tone-controls.md#refine-saturation)): the `curve-*-refine-saturation-*` cases sit at mean ΔE00 1.1–1.5, against 1.4 for the same S curve at 100. Values above 100 render as 100, as in Camera Raw. It imports, is written back, and has a Refine › Saturation slider (0–100) under the RGB point curve.
- **Point Curve menu** has Lightroom's Linear, Medium Contrast and Strong Contrast (points from Lightroom Classic's catalog), saved curves and Save…; saved curves live in RAWmakase's data folder, not in Lightroom's Curves folder, and the menu's curve name is not written to XMP ([tone controls](tone-controls.md#point-curve-menu)). Whether Lightroom's built-in choices also reset the colour curves was not checked; here they set the RGB curve only.
- **Auto** tone is fitted to Lightroom Classic's own Auto values, from 452 Auto Settings steps on 439 catalog photos with every fifth photo held out; each slider is a linear fit of one or two display-encoded percentiles of luminance, of the brightest channel or of chroma, and the held-out error is a median 1 step on Contrast and 0.21 EV on Exposure. Lightroom changed Auto in mid-2019 and the catalog's history shows both versions, so the slopes are fitted on every photo and the levels, Contrast, Vibrance and Saturation on the current Auto alone. The WB menu's Auto white balance is fitted to Lightroom's Auto values (a median of 2.7 mired off on 133 photos); see [tone controls](tone-controls.md#auto).

### The global rule (2026-10-28)

Measured on flat-patch mosaics with controlled histograms instead of fitting tables on
photos ([scene tone stage](scene-tone-stage.md#the-global-rule)). The earlier models
failed for two reasons the mosaics made visible: Camera Raw's gain is a function of the
level's place in the whole histogram (gaps, mass above and below, range), not of a
percentile key, and perfectly flat maxima are a degenerate case (two-valued probes, the
synthetic charts, flat squares), so synthetic results never transferred to photos.

Median ΔE00 is not enough to judge Shadows: it hid both a deep-tone failure and
texture loss. Each comparison is therefore also reported as the signed L* error in
Camera Raw's own bands (0 to 10, 10 to 25, 25 to 50, 50 to 100) and as the fine
texture kept in the dark bands, with a side-by-side panel (main, rule, Camera Raw).
That report found the first fit lifting deep interiors 1.5 to 2 EV less than Camera
Raw (a sunlit cabin's floor 2.0 against 3.7 EV): its training targets stopped 9 stops
under white. Counting targets to 13 stops and adding mosaic families for deep ranges,
bimodal histograms and clipped tops fixed those photos (cabin 6.3 → 1.1, DC-S1 field
5.2 → 2.1, X-T30 II 6.5 → 3.9 median ΔE00 at Shadows +100) at the price of 0.2 to 0.4
EV less lift in the mid-shadows of ordinary photos. Weighting the photos up, or a
wider and longer-trained network, moved that trade the other way and over-lifted
dark-heavy photos instead: three stress photos with alike histograms (a dusk freeway,
star trails over a lit bridge, a backyard from the ZS200D) need opposite corrections,
which is the spatial and colour regime a histogram rule cannot see. Over the 24
stress photos and all Shadows settings the fits tie at the median (1.20 to 1.29
against main's 1.43) and the shipped one has the lowest mean (1.57 against 1.63 for
the 9-stop fit and 1.79 for main), with the fewest large failures.

Final renders against current main, median ΔE00 per photo (validation sets; better
and worse count photos that moved by more than 0.3):

| Set | Shadows +100 | Highlights −100 | Shadows +50 with Highlights −50 |
| --- | --- | --- | --- |
| pixls | 1.81 → 1.61 (2 better, 3 worse) | 1.43 → 1.13 (4, 1) | 1.26 → 1.04 (3, 1) |
| own | 1.92 → 1.63 (1, 2) | 1.00 → 1.09 (2, 1) | 1.43 → 1.12 (3, 0) |
| signature edits | 2.58 → 1.86 (8, 0) | 1.02 → 0.79 (3, 1) | 0.94 → 0.79 (2, 1) |
| pixls stress | 2.58 → 3.08 (2, 4) | 1.39 → 1.49 (3, 3) | 1.63 → 1.65 (5, 1) |
| own stress | 2.74 → 2.04 (3, 2) | 1.65 → 1.00 (3, 1) | 1.56 → 1.54 (1, 1) |
| signature edits stress | 1.62 → 1.21 (5, 2) | 0.85 → 0.68 (4, 1) | 0.90 → 0.83 (0, 1) |

On the train sets: Shadows +100 2.12 → 1.81, +50 1.19 → 1.08, +25 0.77 → 0.74,
Highlights −100 1.53 → 1.13, −50 1.12 → 0.94, −25 1.01 → 0.89, Shadows +50 with
Highlights −50 1.21 → 1.29; the worst spatial regions at Highlights −100 11.2 → 5.3.
Shadows +100 under a changed white balance (the mixed sets' "colour + Shadows" case)
is worse at the median on four of seven sets (train 1.88 → 3.79, pixls 1.11 → 1.29,
own 1.41 → 1.50, own stress 2.28 → 2.46) and better on three (signature edits 1.30 →
0.98 and its stress set 1.61 → 1.08, pixls stress 2.89 → 2.71): two or three photos
per set (a C-Lux, an L2D-20c) land 3 to 4 ΔE00 off under both the rule and the
earlier fit, which the committed tables did not do; the interaction of white balance
with the histogram rule is open. Other settings are untouched by the rule.

### Per-level texture and the ensemble (2026-10-28)

Dark regions of varying texture under a fixed sky, rendered in Camera Raw, showed the
mechanism behind the dark-heavy conflict: at the same depth a smooth region is lifted
1.95 EV and a rough one 3.21 (seven stops down; 2.71 against 4.11 at ten), and the
histogram rule, which had no way to tell them apart, over-lifted smooth deep regions
by up to 3 EV on those probes. Two inputs for the texture of the pixels at the level
itself bring the probes to 0.11 EV rms and hold the dusk freeway's smooth sky and road
at Camera Raw's level while keeping the cabin's floor. A single fit, however, moved
other photos by ±0.3 EV in bands where texture plays no part: four fits of the same
configuration from different seeds disagree by 0.1 to 0.3 EV per level on real photos.
Averaging four seeds lowers held-out error in every depth band (median 0.178 → 0.166
EV) and is what ships.

Final renders against current main (which ships the first rule), median ΔE00 per
photo on the validation sets (better and worse count photos that moved by more than
0.3):

| Set | Shadows +100 | Highlights −100 | Shadows +50 with Highlights −50 |
| --- | --- | --- | --- |
| pixls | 1.61 → 1.41 (2 better, 1 worse) | 1.13 → 1.06 (1, 2) | 1.04 → 1.15 (1, 1) |
| own | 1.63 → 1.67 (0, 2) | 1.09 → 1.11 (0, 0) | 1.12 → 1.12 (0, 0) |
| signature edits | 1.86 → 2.06 (0, 6) | 0.79 → 0.87 (1, 1) | 0.79 → 0.76 (0, 1) |
| pixls stress | 3.08 → 2.20 (4, 0) | 1.49 → 1.26 (3, 1) | 1.65 → 1.94 (1, 1) |
| own stress | 2.04 → 1.93 (1, 3) | 1.00 → 1.06 (0, 1) | 1.54 → 1.59 (1, 0) |
| signature edits stress | 1.21 → 1.21 (1, 2) | 0.68 → 0.66 (2, 0) | 0.83 → 0.76 (1, 0) |

On the train sets against main: Shadows +100 2.12 → 1.53, +50 1.19 → 0.97, Highlights
−100 1.53 → 1.27, −50 1.12 → 1.01. The gain is concentrated where the first rule
failed, dark-heavy and noisy photos (pixls stress Shadows +100 with no photo worse,
the 80D tree 2.41 → 1.09, the ZS200D backyard 3.40 → 2.20, the dusk freeway 2.10 →
1.54); the signature-edits Shadows set loses 0.2 at the median, with the DC-S1 field
(2.08 → 3.59) the largest single regression, where the deep trees come up 0.3 to 0.5
EV more than Camera Raw's. Over all stress photos and Shadows settings the median
moves 1.27 → 1.24 and the mean 1.65 → 1.57.

Issue #464's sky at Highlights −100 renders at L* 87 against Camera Raw's 90 with its
cloud structure kept (2.4 L* against 3.1; before 1.3). Clipped skies at Highlights
−100 can still render darker and warmer than Camera Raw's (an RX100 VII sunrise 5 L*
darker over a quarter of the frame): the mosaics show Highlights pulling fully clipped
patches like any other, and at Exposure −3 the same sky reconstructs to 3.3× white in
RAWmakase against 3.8× in Camera Raw (an X-T30 II sky 2.2× against 2.8×), so this is
highlight reconstruction in the decode stage, not the tone rule. Still open besides
that: the spatial regime (tiny bright lights in dark scenes, scattered gaps against a
strip), the dark-heavy conflict above, and the darkest band of some cameras whose
default render already sits 0.3 to 0.4 EV too light (#471).

### Noise clipped before demosaicing (2026-10-29)

The worst Shadows +100 photos were not the rule's. Feeding Camera Raw's own
per-level lift to the operator (an oracle curve fitted by iteration until the
per-level difference was under 0.02 EV) left their median ΔE00 almost where it
was: an OM-5 foliage frame 7.2 → 7.0, an iPhone XS 6.1 → 6.1, an a7 II night scene
10.1 → 10.1, two X100F frames 4.8 → 3.3 and 4.9 → 3.0, a D7000 interior 4.2 → 3.6.
Their error is detail (the OM-5 already differs by 4.9 at default) and colour in deep
shadows: the night scene's near-black background rendered magenta where Camera Raw
renders it warm neutral, with the sidecar's colour noise reduction at 0 and the
reference's luminance matched within 0.1 EV at Exposure +4.

The raw explains it: at ISO 6400 a third of that background's green samples lie
below the black level, and clipping them at zero before demosaicing raises every
channel by about 0.4 of its noise, scaled by its white-balance gain, so red and blue
rise most. Camera Raw's bias on synthetic noisy patches is 0.5 to 0.7 of that, as
if it clipped only after demosaicing ([demosaicing](demosaic.md)). Development now
keeps samples negative through the Bayer passes and clips once after them. Dataset
gate against the shipped engine (median ΔE00, no photo worse by more than 0.3 in any
set):

| Set | Default | Shadows +100 | Shadows +50 | Highlights +100 |
| --- | --- | --- | --- | --- |
| own | 0.77 → 0.77 | 1.67 → 1.56 | 1.14 → 0.91 | 1.16 → 0.93 |
| own stress | 0.43 → 0.36 | 1.93 → 1.77 | 0.89 → 0.77 | 2.01 → 1.88 |
| pixls, signature edits, their stress sets | unchanged | unchanged | unchanged | unchanged |

The night scene itself goes 10.1 → 8.7 at Shadows +100 (3.9 → 3.1 at default), its
cast gone; what remains there is luminance (our lift of its deepest two stops is 0.3
to 0.7 EV short of Camera Raw's, which the oracle cannot close either) and the
demosaic's own noise. Not changed: X-Trans, whose demosaic still clips each sample
first; the D7000 interior and the OM-5, which are not noise cases.

### Highlight reconstruction colour (2026-10-29)

Clipped skies at Highlights −100 rendered duller and pinker than Camera Raw's,
with a hard edge where the second channel clipped (an RX100 VII sunrise: median
ΔE00 2.79, chroma-only 1.33). Highlight recovery rebuilt a clipped channel from the
ratios of unclipped neighbours, or from a colour map of the same bright area, so a
warm patch with green clipped stayed warm. Flat coloured patches rendered through a
linear profile with an As Shot neutral, so that the channels clip at different scene
levels, give Camera Raw's rule: in white-balanced camera space every channel is pulled
up towards the mean of the other two, never down, by a weight
((s − 0.25) / 0.75)^1.8 of its own sensor fraction s, solved as a fixed point anchored
at the recorded values. A clipped channel becomes the mean of the other two, past two
clips the pixel is neutral, and unclipped colours near the white are desaturated the
same way; over 136 patches the fit is within 0.005 RMS of the level per channel. The
rule reads no neighbours, as the flat patches show, and replaces the neighbourhood
estimate and the colour map ([rendering quality](rendering-quality.md)).

The sunrise goes 2.79 → 2.55 (chroma 1.33 → 1.05). Dataset gate against the engine
with the noise-clipping change above (median ΔE00 of the validation sets; Default
and Highlights from the highlights sets, Shadows from the shadows sets):

| Set | Default | Highlights −100 | Highlights −50 | Shadows +100 | Shadows −50 |
| --- | --- | --- | --- | --- | --- |
| pixls | 0.46 → 0.46 | 1.06 → 1.28 | 0.81 → 0.89 | 1.41 → 1.40 | 0.99 → 0.93 |
| own | 0.58 → 0.58 | 1.11 → 1.11 | 0.86 → 0.86 | 1.56 → 1.61 | 0.74 → 0.74 |
| signature edits | 0.30 → 0.30 | 0.87 → 0.88 | 0.62 → 0.60 | 2.05 → 2.11 | 0.60 → 0.51 |
| pixls stress | 0.90 → 0.79 | 1.26 → 1.18 | 1.14 → 1.02 | 2.20 → 2.14 | 1.16 → 1.16 |
| own stress | 0.53 → 0.53 | 1.05 → 0.87 | 0.84 → 0.72 | 1.77 → 1.77 | 0.39 → 0.39 |
| signature edits stress | 0.34 → 0.33 | 0.66 → 0.56 | 0.49 → 0.45 | 1.19 → 1.14 | 0.57 → 0.43 |

Across all 18 sets 21 photo/setting rows improve by more than 0.3 and 12 worsen, the
mixed sets' Dehaze + Clarity and colour Shadows cases most (up to 5.25 → 3.38). The
rows that worsen are luminance, not colour, and come from the Shadows/Highlights
rule rather than from the reconstruction: fully clipped pixels now reconstruct to the
highest white-balance gain instead of the colour map's ceiling, so the histogram top
the rule reads rises by 0.44 stops on clipped photos (a DC-S1 field 0.36 → 0.80, a
Rebel T7i 0.06 → 0.50 stops above white) and the rule, fitted to maps from the old
decode, lifts the field 4 L* too far at Shadows +100 (3.59 → 5.48) and darkens the
T7i 2 L* too far at Highlights −100 (1.06 → 1.61), while both default renders
improve. Next: refit the rule on measurement maps from this decode. Still open: the
X-T30 II plateau that is darker than Camera Raw's already at default (white level or
X-Trans clipping), and the demosaic's red/blue noise above Camera Raw's.

### Moving the rule's histogram top: what did not work (2026-10-30)

The two luminance regressions above (a DC-S1 field at Shadows +100, a Rebel T7i at
Highlights −100) come from the Shadows/Highlights rule reading the reconstructed
overshoot: a clipped spot of 0.05% of the frame raises the top the rule reads by 0.44
stops. A probe settles what Camera Raw does with such a top (mosaics under an As Shot
neutral, with a square of 0.05, 0.5, 3 or 12% of the frame either clipped in every
channel, so that it reconstructs a stop above the white, or just below the white):
the clipped square adds only 0.05 to 0.09 EV to Camera Raw's Shadows +100 lift of the
dark patches beside it, against the unclipped square of the same size, so Camera Raw
counts clipped pixels close to the white. Every way of giving the rule that behaviour
failed on the photos, each retrained from the engine's own maps with four seeds:

| Variant | Small-clip photos | Heavy-overshoot photos | Decision |
| --- | --- | --- | --- |
| Map clamped at the white, retrained | DC-S1 5.48 → 1.39, T7i 1.61 → 0.63, signature Shadows +100 2.11 → 1.38 | RX100 VII at 6400 K + Shadows +100 2.67 → 9.54; a beach with a quarter of the frame above the white 1.37 → 7.75 at Highlights −100 (the curve pulls the mid-tones, the whole frame 8 L* dark) | Rejected: 34 rows better, 33 worse, two catastrophic |
| Overshoot compressed to a quarter or a half instead of clamped | Same wins | RX100 5.37 / 3.74, beach 8.44 / 6.97 | Rejected |
| Clamp plus 34 training mosaics with 10 to 35% of the frame clipped under white-balance gains | Smaller wins | RX100 6.87, beach 4.57 | Rejected: the shape is not learnt from mosaics |
| Shipped weights, top at the 99.9th or 99.95th percentile instead of the maximum | T7i 2.70 → 2.28, DC-S1 unchanged (its spot is 0.16%) | beach 1.50 → 2.21, a FZ1000 1.64 → 3.01 | Rejected |
| Eight seeds instead of four | Identical to four | Identical | The losses are not seed variance |

A retrain of the shipped configuration on the new decode's maps without any clamp
reproduces the shipped rule on these photos (RX100 2.54, beach 1.55) and leaves the
DC-S1 and T7i as they are. The rule as fitted therefore depends on the overshoot in a
way that the flat-patch probe does not show, and the photos with a large reconstructed
area are matched by that dependence; the small-spot photos are not. What is left to
try is a feature that tells the rule how much of the frame is clipped, so that it can
learn both regimes, and training photos chosen for small clipped spots.

A second gap surfaced by the same probe: with a bright area of 12% of the frame, clipped
or not, Camera Raw lifts the mid-tones at Shadows +100 by 0.4 EV and the rule by 0.7 to
1.2 EV. The sky and ground mosaics do not cover it.

### How sparse the top is (2026-10-30)

A further probe settles whether Camera Raw's rule reads a clipped pixel differently
from a bright one: mosaics under an As Shot neutral with a square of 0.2, 3 or 12% of
the frame at four levels, just under white, clipped in green (reconstructing 0.26 stop
over the white), in green and blue (0.77 over) and in all three (a stop over). Camera
Raw's Shadows +100 lift of the dark patches beside the square is the same to 0.05 EV
at every level, and Highlights −100 pulls the square −0.84 to −1.00 EV whether it is
clipped or not: the rule does not count clipping, and the engine's reconstruction is
not what sets the DC-S1 field apart. What sets it apart is the histogram's top: a
0.16% spot sets the maximum, and the rule has no input that says how little sits
there. Reading the clipped pixels before reconstruction (their recorded luminance
with each clipped channel held at its white) was tried on the shipped weights and
rejected: it helps the DC-S1 field and the RX100 but hurts a DC-S1RM2, an a7 II and a
beach by as much.

Two inputs, the fraction of the map within one and within two stops of the top, were
added to the rule, and it was retrained on the three families of mosaics with clipped
areas under a white-balance gain (clipped squares of 0.05 to 12% of the frame, 10 to
35% of the patches clipped, and the partially clipped squares above) and on 62 more
CC0 raw.pixls.us photos chosen for small clipped spots, skies over deep ground, large
bright areas and heavy clipping. Variants that failed on the way, each retrained with
four seeds from the engine's maps:

| Variant | Outcome | Decision |
| --- | --- | --- |
| The sensor's white, the clipped fractions and the clipped fraction at the level as inputs | The curves jump where the clipped fraction changes; no better on the photos | Rejected |
| Those plus the 99th and 99.9th percentiles' distance from the level | DC-S1 field 6.22 → 3.12, but the mid-tones of filled histograms (a ZS200D, an X100F) over-lifted by 0.3 to 0.5 EV; gate 36 better, 42 worse | Rejected: the percentile inputs cause the over-lift |
| The two top-mass inputs alone (shipped) | DC-S1 field 6.22 → 3.34 with the mid-tones held; gate 34 better, 20 worse | Shipped |
| The same without the 18 mosaics with 10 to 35% clipped, or without all three clipped families | The noisy 80D sample below improves to 4.6 or stays at 7.7, three other photos worsen by 0.3 to 1.0 | Rejected |

The retrained rule against the shipped one on the validation sets (median ΔE00 over
photos; photos moving more than 0.3 in brackets):

| Set | Shadows +100 | Highlights −100 | Colour Shadows +100 | Shadows +50 with Highlights −50 |
| --- | --- | --- | --- | --- |
| pixls | 1.40 → 1.68 (1 better, 3 worse) | 1.28 → 1.22 (2, 2) | 1.34 → 1.18 (4, 2) | 1.13 → 1.03 (0, 1) |
| own | 1.61 → 1.51 (2, 0) | 1.11 → 1.06 (0, 0) | 1.76 → 1.37 (1, 0) | 1.09 → 1.17 (0, 0) |
| signature edits | 2.11 → 1.94 (5, 0) | 0.88 → 0.79 (0, 1) | 1.12 → 0.99 (1, 0) | 0.76 → 0.71 (0, 0) |
| pixls stress | 2.14 → 2.01 (1, 2) | 1.18 → 1.25 (0, 1) | 2.31 → 1.94 (2, 1) | 1.82 → 1.23 (1, 0) |
| own stress | 1.77 → 1.68 (2, 0) | 0.87 → 0.64 (0, 0) | 1.63 → 1.45 (2, 0) | 1.27 → 1.27 (0, 0) |
| signature edits stress | 1.14 → 1.09 (1, 2) | 0.56 → 0.57 (0, 0) | 1.14 → 0.86 (2, 2) | 0.73 → 0.75 (0, 1) |

Over every non-default row the median is 0.87 → 0.86 and the mean 1.22 → 1.21; on
the train sets Highlights −100 is 1.27 → 1.17, Colour Shadows +100 3.33 → 1.69 and
Shadows +100 1.56 → 1.67. The two regressions above are fixed: the Rebel T7i at
Highlights −100 1.61 → 0.89 and the DC-S1 field at Shadows +100 5.48 → 2.13; a star
trail (DC-S1RM2) at Shadows +100 2.51 → 2.29 and the RX100 VII at 6400 K with Colour
Shadows 2.67 → 1.62 come closer too.

What the retrain costs. On filled histograms the rule now lifts the mid-tones 0.1 EV
more than Camera Raw (a FZ1000 at Shadows +100 0.80 → 1.54, an a6600 0.82 → 1.29, the
ZS200D 2.14 → 2.51), a GFX 50R at Highlights −100 pulls 0.1 EV more (1.71 → 2.29), and
the X-T30 III plateau is lifted 0.5 EV more seven stops down (4.78 → 5.57). One photo
is lost outright: a noisy Canon 80D sample with 14% of the frame within a stop of the
top, whose mid-shadows Camera Raw lifts 1.0 EV at three stops down, is lifted 0.58
(2.98 → 6.38 at Shadows +100, 3.12 → 6.62 with Colour Shadows). Its mid levels are
very smooth (texture 0.015 stop) and no training photo or mosaic sits near it: the
nearest in input space are old training photos whose lift there is 0.0 to 0.16 EV,
and the rule without the new inputs, retrained on the same data, lifts it 0.07 EV.
The shipped rule matched it by luck of the fit rather than by anything it had learnt.

Measured on the way and not in the rule: the texture input over the top two stops
(`tex_top`) is saturated at its clamp on every training mosaic and photo, so it is a
constant; and on the clipped mosaics Camera Raw lifts the mid-tones only 0.2 to 0.4
EV where photos with the same top mass are lifted 1.0 to 1.4, which is the
large-bright-area gap above seen from the training side.

### Shadows/Highlights research: what did not work (2026-10-16)

**No candidate from this investigation passed the requirements for shipping.**
The learned operator on `t3code/learned-local-tone` remains unmerged. Its paired
final-render validation median against main was **1.21 → 1.21 ΔE00** across
5,222 setting/photo rows; p90 was 5.00 → 5.04. Better Highlights medians did not
establish a safe Highlights-only release: individual photos and scene probes
still regressed.

The experiments below used Camera Raw 18.7. Photo decisions used the dataset
**validation** split; dataset holdout was not rendered or scored. Scene-gain RMS
in EV, synthetic-probe errors, and final-render ΔE00 are different measurements
and must not be compared as one score. These results describe particular tested
implementations, not proofs that every variant of an algorithm must fail.

| Approach | Evidence | Decision |
| --- | --- | --- |
| Add Local-Laplacian features to the learned operator | An earlier rendered candidate improved median ΔE00 versus main: Shadows +100, 2.432 → 1.920 (44 photos); Highlights −100, 1.781 → 1.310 (51 photos). But 21 Shadows and 13 Highlights photos had regional regressions greater than 2 ΔE00; clipping and a clipped-spot scene probe also regressed. | Rejected despite better medians. Not evidence for shipping Highlights alone. |
| Retrain Laplacian features with corrected intrinsic targets and `DefaultBlackRender=None` | On 47 validation photos, direct gain RMS improved from 0.260/0.140 to 0.246/0.130 EV (Shadows/Highlights), but p90 worsened for both. Three of six scene-probe tests failed; the dark-gap Shadows lift was 1.188 EV versus Camera Raw's 0.600 EV. | Rejected before another final-render photo gate. Direct network accuracy did not establish deployed parity. |
| Median prefilter before the existing Highlights/Shadows base, with frozen network weights | Small-square diagnostics worsened: Shadows RMS 0.274 → 0.291/0.299 EV for the two tested radii; Highlights 0.945 → 1.019/1.137 EV. | Rejected as a drop-in substitution. No native photo gate was run for this variant. |
| Remove the four per-location coarse features; predict one globally conditioned curve on the guided base | Matched 248 training photos + 64 synthetic scenes, three seeds, same architecture and training schedule. On 47 validation photos, ensemble RMS worsened: Shadows 0.260 → 0.272 EV, Highlights 0.140 → 0.155 EV. Medians and p90 also worsened. At a 0.02 EV per-photo threshold, Shadows had 4 improvements/23 regressions; Highlights 1/14. | Rejected. Retain the local features in this architecture; no deployment or rendered gate for this candidate. |
| Describe scene context using only minimum/maximum of a 64- or 128-pixel reduced image | Identical source **and decoded-default** extrema at 128 pixels still gave a 0.0225 EV difference in Shadows +100 at the same distant dark luminance. | Insufficient context; no curves fitted to this representation. |
| Predict gains from reduced-image quantiles with a fixed kernel regressor | Excluding each entire synthetic family during fitting, the 64-pixel model had 0.092/0.529 EV RMS; the 128-pixel model had 0.141/0.441 EV RMS. Unseen brightness depths were particularly poor. | Not accurate enough to advance. This does not rule out every filtered-distribution model. |
| Add independently measured Shadows and Highlights log gains | Clean LinearRaw probes had interaction residuals of −0.190 EV at (+50, −50) and −0.380 EV at (+100, −100), for the same dark patch. | Addition is not Camera Raw's combined response at the measured boundary. |
| Compose the two single-slider curves | Re-rendering one control's output through the other missed the original joint result by 0.300–0.670 EV at the tested centers, with default roundtrip error below 0.00051 EV. A separate frozen-context scalar Shadows→Highlights test put 15/72 joint measurements outside conservative monotone endpoint bounds, by up to 0.266 EV. | Neither tested scalar composition explains the joint response. Arbitrary spatial processing order remains unidentified. |
| Replace addition at (+50, −50) with a shared weighted sum of the two measured single-slider gains | Using saved renders only, coefficients constrained to [0, 2] and excluding each whole synthetic family, 41 eligible scenes improved from 0.162 to 0.049 EV RMS, but maximum error remained 0.102 EV. Every fit put the Shadows coefficient at its zero bound; incomplete coverage and this restricted setting do not establish a general control law. | Insufficient evidence for an engine change. No photo validation or new rendering was performed for this diagnostic. |

**Unconfirmed, rather than disproved:** the LibRaw/Adobe white-level audit found
camera differences, but did not establish that changing the scene white point
would improve the observed residuals. The within-Canon comparison did not
support the proposed causal explanation. No white-level correction was made.

**Useful findings that are not a parity fix:** identical full-resolution RGB
histograms can produce different broad Shadows responses when texture frequency
changes. Some spatially processed scene context is therefore missing from a
plain histogram model. On three clean scenes, common-amplitude scaling plus
quadratic interpolation over slider directions predicted 152 additional dark
ROI measurements with 0.00048 EV RMS and 0.00253 EV maximum error. This describes
slider geometry, **not how to predict the image-dependent curves**; bright
outputs violated the same simple scaling law. Small spatial variation in the
joint interaction suggests a shared joint curve plus spatial residuals, but
that decomposition has not produced a validated photo-quality improvement.

Target construction also needed two correctness repairs: robust monotone
default-transfer calibration instead of a cumulative maximum over individual
pixels, and rejection of Adobe Standard references paired with linear-profile
taps. These trainer repairs are on the research branch (`b26079fd` and
`6f6e2701`); this findings update does not merge them. Historical
mismatched-profile gain scores are not evidence. See the stage
[comparison contract](scene-tone-stage.md#comparison-contract) for the
measurement boundaries.

The large synthetic render queue was **stopped at the user's request**; its
combined-control coverage is incomplete. Existing measurements were retained.
Do not restart that grid or generate more large probe batches as the default
next step. A further candidate needs a specific hypothesis, a bounded test on
existing evidence where possible, and a validation/rendering decision before
expanding data collection. No matched photo-quality comparison against
darktable, RawTherapee or RapidRAW was performed; studying their mechanisms did
not establish that RAWmakase renders better or worse than those applications.

**Final stress-data attempt, then stopped.** At the user's request, one final
bounded test used only existing stress-validation references: 17 distinct
photos with default, Shadows +50, Highlights −50 and combined renders. TIFF
hashes and embedded metadata were checked; all used Adobe Standard and differed
only in those two controls. No new images were rendered. The test compared five
fixed rules on the same supported pixels, with median coverage of 78.2%:

| Rule for the combined response | Aggregate RMS (EV) | Median photo RMS (EV) | p90 photo RMS (EV) |
| --- | ---: | ---: | ---: |
| Add log gains | 0.123 | 0.117 | 0.177 |
| Add linear-luminance changes | 0.142 | 0.092 | 0.228 |
| Frozen scalar Shadows → Highlights | 0.147 | 0.132 | 0.215 |
| Frozen scalar Highlights → Shadows | 0.155 | 0.145 | 0.208 |
| Probe-derived weighting, 0.85 × Highlights log gain | 0.498 | 0.376 | 0.748 |

The sequential models fitted monotone curves to each photo's **single-control**
references on one spatial half, then predicted the combined render on the other
half; combined outputs were never fitted. All rules received Camera Raw's own
single-control results, not RAWmakase approximations. Predictions were limited
to measured interpolation support, with all four luminances between 0.003 and
0.85; excluded dark/clipped pixels are not covered by these scores.

None met the predeclared criterion: median photo RMS at most 0.03 EV, p90 at most
0.05 EV, median paired error reduction of at least 50% against log addition, and
at least 50% median pixel coverage. Every alternative also worsened aggregate
RMS; the linear-change rule improved only 6 of 17 photos despite its lower
median. The synthetic weighting did not transfer to these photos.

These are **final-output luminance diagnostics**, not intrinsic scene-gain or
rendered-engine parity scores. Adobe Standard's downstream mapping, color and
spatial effects prevent treating them as proof of Adobe's internal algorithm.
The attempt did not identify a defensible predictive mechanism or a new engine
candidate. Research was stopped as instructed; no further experiment or render
queue was left running, and dataset holdout remained untouched.

### Detail kept and range-aware Highlights (2026-10-19)

Started from two reported photos: Highlights −100 flattens an overcast sky
that Camera Raw keeps cloudy, and Shadows +100 leaves dark foliage flat and
dark. None of the candidates below passed the validation gate against
`main-a8c5d6d7-metric2`; none was merged.

- **What the photos showed.** In Camera Raw the Shadows lift is nearly constant
  within a dark area: dark areas keep 101% of their local contrast on the
  dataset's 80 train photos, against 77% in RAWmakase. Positive Shadows' detail
  table gives pixels darker than their region extra lift, and in foliage every
  darker leaf counts. In the sky, the negative-Highlights row above 5 stops over
  the key had been flattened by `monotone` (its decreasing, unsampled tail), so
  1 EV of sky became about 0.1 EV.
- **Not the cause:** the default black. Camera Raw and RAWmakase subtract the
  same black, by output level, under Shadows +100 and at default.
- **Train-split refits.** Linear-profile Camera Raw renders of the train split
  (80 usable photos; DNG-native files cannot take the linear profile) were
  fitted with `scene-tone-local.py`'s target construction and cross-validated
  by photo.
  - Positive Shadows' detail table read from a guided base at ε 2 instead of
    0.5, both tables refitted: held-out scene-gain RMS at +100 0.244 EV against
    0.275 for main's tables, and detail kept 85% against 77%. The train photos
    have no regional level below about 4 stops under the key, so the darkest
    regions are extrapolation in either direction.
  - Negative Highlights interpolated between rows fitted at a photo range
    (p99.9 less the key) of 4 and 9 stops: Camera Raw pulls the brightest areas
    of a wide-range photo much harder than a narrow-range one. Held-out
    bright-area RMS at −100 fell by about 11% against main's table. A fixed
    hold above 5 stops helped narrow-range photos and broke wide-range ones.
    A term keyed to the photo's top, or to distance below white, added nothing.
- **Validation gate** (19 sets: Highlights, Shadows, mixed, A7R VI tone; photos
  counted once). Range-aware Highlights alone: −100 median −0.12 ΔE00, 11 photos
  better and 4 worse by more than 0.3; −50 4 better, none worse. It still fails:
  about 10 photos regress regionally by more than 2 ΔE00 (low-key scenes with a
  mid-bright area, which it pulls harder than Camera Raw), clipping rises on two
  stress photos, and two strata's medians worsen. The Shadows refit was neutral
  on validation (+100: 15 better, 10 worse, mean +0.06), with and without the
  scene-context offset, which was calibrated for main's tables; its train
  improvement did not carry over to rendered validation photos.

- **Follow-up (2026-10-20): how Camera Raw's pull varies.** On the 80 train photos
  Camera Raw compresses bright areas by about 20% at every scale from 1 to 32 px
  of the 512-px map, as RAWmakase does: Highlights is not an exposure-like
  change of whole regions. A per-photo table on RAWmakase's guided base halves the
  held-out error at −100 (0.158 → 0.080 RMS), so the base is right and the curve
  varies by photo; the variation is consistent across −25, −50 and −100. Keys
  from other percentiles, absolute level, histogram rank, a range-normalised level
  and a two-scale (regional) term all fit worse than the base less the key.
  A table conditioned on the base histogram's shape (spread, skew, kurtosis,
  entropy, mean, interquartile range, centroids weighted to the top) cut held-out
  train error by 16% at −100 (49 photos better, 5 worse) but failed validation
  worse than the range-aware version: −100 median −0.04 ΔE00, 9 photos better and
  11 worse by more than 0.3, 17 regional regressions. Its per-photo changes
  correlate 0.63 with the range-aware version's.

- **Why it does not carry over (2026-10-20).** The engine applies the fitted gain
  exactly (on a D7100 photo its scene gain matches the Python model within 0.03 EV),
  so the gap is the model, not the port. The range-aware version improved the
  −100 mean in nearly every validation family (plain −0.22, stress −0.16, own-stress
  −0.49, signature edits −0.05 ΔE00); it failed on the regional rule, where in about
  12 photos a cluster of bright regions moved several ΔE00 the wrong way (one from
  1.8 to 11.8) while the photo's median improved. One curve per photo, chosen from
  global statistics, is wrong in all bright areas of a photo at once when the
  choice is wrong. At half strength (rows halfway between main's and the
  range-aware ones) −100 improved by −0.07 median, 6 photos better and none worse
  by more than 0.3, but 6 photos still regress regionally (702250c1, a2d7222a,
  2d9c40f6, ae83bb3a, 1860202e) and two stress photos clip more at −25 and −50;
  it fails too. The train-split regional proxy rated the histogram model best and
  validation rated it worst, so the train split does not predict validation for
  per-photo choices.

Next step, if resumed: none of the train-split fits (Shadows refit, range-aware,
histogram-shape) carried over to validation, though each held up in
cross-validation by photo. The train split (80 linear-profile photos) is too
small or too unlike the validation sets to fit a per-photo Highlights pull; more
linear-profile Camera Raw renders of varied photos, including low-key scenes and
the signature-edit sets' kind of photo, should come before another model.

### Highlights far above the key (2026-10-21)

Synthetic probes showed Highlights −100 holding at about −3.5 EV for areas more than 7
stops over the key, where Camera Raw keeps pulling (−10.2 EV at 10 stops), clipped or
not. The −100 row now continues past its table ([negative Highlights past the
table](scene-tone-stage.md#local-operators)); −50 and −25 still hold,
since extending them made photo regions worse. Camera Raw's pull at those positions past
7 stops (about 0.5 and 0.25 EV per stop) is not matched.

### Default render, Blacks, negative Dehaze and Clarity (2026-10-20)

Started from the open issues #467, #468, #470, #471 and #474, judged against
`main-a8c5d6d7-metric2` with its pinned build. No tone candidate from this round
passed its train-split check; none was gated or merged. Train fits are on
`*-train`, judged on rendered train photos (final-image ΔE00 and band ΔL\*) before
any validation run; holdout was not touched.

- **Default render against each control (validation).** Each control's own error
  is measured as ΔE00 between our render and Camera Raw's with the default
  render's per-pixel Lab error carried over (not meaningful where a control clips
  both renders, as negative Blacks does).
  - Negative Dehaze is the control's own error: pixls mid −100 is 12.17, of which
    the default 0.54 and the control 10.65; owner's low −100 9.14, control 8.75.
  - Positive Exposure (#470) is mostly the default render: pixls mid +1 2.41 with
    the default at 2.04; its 20–30 L\* band is +7.0 against +6.2 at default. Most
    of the pixls mid default was the Panasonic DMC-ZS60 (10.65, +0.84 EV uniform),
    which main's DMC-ZS60 row (#540, −0.5) has fixed since a8c5d6d7. The DMC-G85
    (7.78) is a contrast difference, not exposure, still open.
  - Positive Blacks (#474) is the control's own on low-key photos: +100 about
    2.1–2.4 of its own, about +2 L\* too light in the darkest band.
  - Clarity +100 (#468) is mixed: pixls high 5.02 with 3.44 from the default;
    own and Signature Edits 1.4–3.4 of its own. Clarity −100 is the default's.
  - The Hasselblad L1D-20c (3.82) is its unapplied OpcodeList3 GainMap (lens
    shading), not tone: 0 L\* at the centre to −11 L\* in the corners.
- **Blacks keyed on the 0.1th percentile (#471).** Exactly neutral on
  `blacks-train`: `BLACK_KEYS` starts at −8 stops and every photo's minimum and
  0.1th percentile lie below it (−8.7 to −12.7), so both clamp to the same row. A
  key sweep (−8 to −3.5) found low-key photos preferring −5 to −6.5; the best rule
  (0.1th percentile − median − 4) moves Blacks −25 from 1.21 to 1.14 (mean 1.873 →
  1.859) and the rest not at all. Per side, at default Camera Raw's black pixels
  are at our L\* below 1 (0.56% black in Camera Raw only, 0.11% in ours only): it
  clips the last half L\* where ours approaches; the clipping rule counts it, ΔE00
  barely does. On low-key photos ours is lighter than Camera Raw's Blacks −50
  between default L\* 3 and 12 (at 8: 1.7–1.8 against 0.9–1.0).
- **Negative Dehaze as added haze (#467).** The dark-channel model with the
  airlight, J = p·t + A·(1−t), adds far too much: the airlight is near sky white,
  and even ω 0.12 at −25 lifts shadows 26 L\*. What Camera Raw does, in its own
  renders: an additive veil whose size follows the photo's median luminance (log
  correlation 0.83; its 99th percentile, today's key, 0.23), at −50 and −25 about
  0.3 and 0.12 of −100's, coloured like the airlight (b\* correlation 0.64, slope
  1.0; the photo's mean colour 0.02), fading toward black, with a contrast change
  per photo from 0.34 to 1.33 (display-linear slope) that follows the airlight's
  brightness (correlation 0.73) and the haze density (−0.48). A veil of the
  median in the airlight's colour, fading as L / (L + 0.3·median), with a fixed
  gain, takes the median of half the train photos at −100 from 8.62 to 6.66
  (−50 4.06 → 3.46, −25 2.29 → 2.25) with every band within 3 L\*, but 11 photos
  get worse by more than 0.5 (one 3.24 → 12.79), so it would fail the
  no-photo-regresses rule. Keying today's curves on the median instead of the
  99th percentile was no better (−100 8.62 → 8.78 at median + 2).
- **Clarity on photos (#468).** The four-scale weights refitted on `clarity-train`
  (53 photos, map grid registered to the renders, L\*-weighted least squares,
  cross-validated by photo) cut per-pixel L\* RMS at +100 from 5.40 to 4.45, but
  rendered train photos did not follow: +100 median 2.54 → 2.63 (9 better, 13
  worse), +50 1.92 → 1.83, −100 1.63 → 1.68. At 1024 px Camera Raw's +100 adds
  1.6–4.1 L\* of change finer than 2 px that ours (on the 512 px map) does not; a
  full-resolution term like Texture made it worse (+30: 2.54 → 2.59, 1 better, 16
  worse). Our mid-scale strength against Camera Raw's ranges from 0.03× to 1.3×
  by photo.

Next steps, if resumed: negative Dehaze with a contrast factor predicted from the
airlight and haze density (fit on `dehaze-train`, check per photo before any
validation run); Camera Raw probes for positive Blacks and Exposure against
DefaultBlackRender None (#474, #470), which this round could not render.

## Color

- **Saturation** −100 renders Camera Raw's exact luminance gray ([saturation](color-mixer.md#saturation)): 0.65 mean ΔE00 on the chart, down from 3.97. Between −50 and +100 it is the photo-measured tables (chart: −50 1.21, +50 1.39, +100 2.93); Camera Raw's own blend toward luminance fits the chart better but was worse on real photos. **Vibrance** follows Camera Raw's chart measurements beyond ±50 ([vibrance](color-mixer.md#vibrance)): −100 at 0.83 and +100 at 1.00 on the chart, down from 3.90 and 1.54; up to ±50 it is the photo-measured tables.
- **Color mixer** bands are fitted to Camera Raw 18.7 on a dense synthetic chart ([chart tables](color-mixer.md#chart-tables)): the chart's single-band cases sit at mean ΔE00 0.64–1.31 (default render 0.66). Only Blue and Purple were rendered at ±50; the other bands use the same Luminance slider curve, measured on those two. Colors beyond sRGB keep the earlier photo-measured tables, and several bands combine by adding their changes (`mixer-alternate-luminance`: 1.42).
- **Color grading** follows Camera Raw 18.7 at every hue, saturation, Luminance, Blending and Balance ([color grading](color-mixer.md#color-grading)): the chart's `grading-*` cases sit at mean ΔE00 0.96–2.02 (default render 0.91), down from 1.88–4.15, and looks with split toning at 1.05–2.17, down from 1.62–3.46. Strong tints at Blending near 100 are the furthest off: Shadows and Highlights at Saturation 60 and Blending 100 leave 2.5 ΔE00 over Camera Raw's own render, where Camera Raw stops a channel's gain from falling further than the fitted curves do. RAWmakase clips colors the tint pushes out of sRGB per channel, as Camera Raw does, which brings the `grading-*` cases to 0.56–1.43.
- **Targeted Adjustment Tool** works on the Tone Curve's parametric regions, the Color Mixer's Hue, Saturation and Luminance, and the B&W mix, sampling the color where each control sees it ([color mixer](color-mixer.md#targeted-adjustment)). How a drag is shared among bands follows RAWmakase's render, not a measurement of Lightroom's tool; its drag rate and History names are RAWmakase's own, and in point-curve mode it moves the parametric regions where Lightroom drags a point on the curve.
- **Black & white mix** follows Camera Raw 18.7's renders of the synthetic chart ([chart gray](color-mixer.md#chart-gray)): the chart's single-band `bw-*` cases sit at mean ΔE00 0.51–0.82 (default render 0.66), down from 1.1–2.5, and the Auto mix at 1.75, down from 2.30. A monochrome look at Amount 200 is a little further off (2.50, was 2.41). No real photos with Camera Raw black & white renders were available to check it on.
- **Auto black & white mix** is fitted to Camera Raw 18.7's Auto on synthetic scenes and is within 2 slider steps of it on the chart it was not fitted to; on 30 held-out scenes it is off by 1.3 steps on average (worst band 10). Scenes of two flat colors are predicted worst (up to 30 steps off on a band). It has not been compared with Lightroom on the same real photos.
- **Point Color** imports, renders, is written back and has Lightroom's Point Color tab with the dropper, range handles and Visualize Range ([color mixer](color-mixer.md#point-color)). Fitted to Camera Raw 18.7 on synthetic charts, the 16 `pc-*` cases on the chart sit at mean ΔE00 0.83–1.26 (the default render is 0.91), down from 1.2–4.4 with Point Color left out. RAWmakase clips colors brightened past white per channel, as Camera Raw does, turning oranges towards yellow (`pc-orange-lum+50`: 1.82 → 0.76; all `pc-*` cases 0.69–1.11). The range shapes and Variance's effect on saturation and lightness are fitted, not exact. The sliders show the stored values × 100 (Range 0–100), and the dropper's neutral and dark limits are RAWmakase's own: neither was compared with Lightroom's panel.
- **Camera Calibration** primary sliders follow Camera Raw 18.7 ([camera calibration](rendering-quality.md#camera-calibration)): the chart's `calibration-*` cases sit at mean ΔE00 0.62–0.68 (default render 0.66), down from 1.11–2.02. Measured on one synthetic camera; combinations of several sliders were checked on one pair only. Shadow Tint is still the earlier approximation.
- **Camera exposure offsets** come from a table with a row per body, 180 bodies across 10 makes, so RAWmakase no longer needs a measured offset per camera to be close: a camera without a row falls back to the median of its own make and then of the whole table, which leaves a leave-one-out error of 0.14 EV on average against 0.29 EV for no baseline at all. A DNG's own BaselineExposure is used instead and the table is never used for one. Fujifilm rows hold at DR100 and base ISO, with the maker-notes DR shift added so DR200 and DR400 work. Without a baseline RAWmakase renders most cameras about 0.3 EV darker than Lightroom, for Sony, Canon and Nikon alike. X100F renders are still about 0.04 EV brighter in midtones; see [cameras](cameras.md).

## RAW processing and profiles

- **Demosaic, highlight reconstruction, noise reduction and sharpening** are not Adobe's algorithms; sharpening and colour noise reduction are fitted to Camera Raw's ([rendering quality](rendering-quality.md)). Colour noise reduction measures the photo's noise from its smoothest areas, where Camera Raw uses the camera's noise profile; a photo with no smooth areas reads as noisier than it is. At 100% the detail error is about 0.007 on X100F. X-Trans uses 1-pass Markesteijn, which measures the same as 3-pass.
- **DCP support** is a bounded subset. Triple-illuminant, HDR and other unsupported profile structures are rejected. Enhanced XMP looks (Adobe Color etc.), creative HSV looks (B&W 01, 03–12, Modern 01), RGB-table looks (Artistic, Vintage, Modern, camera-matching XMPs that carry their table), and looks carrying Exposure, Saturation, colour mixer, parametric curve, split toning or vignette settings ([settings inside looks](lightroom-profiles.md#settings-inside-looks)) are supported; adaptive/AI profiles, camera-matching XMPs whose table Camera Raw keeps elsewhere (Fujifilm film simulations) and looks with settings RAWmakase doesn't apply inside a profile (B&W 02's white balance, the B&W filters' mix, the Premium film looks' colour grading) are not. RGB tables match Camera Raw 18.7 on synthetic looks within RAWmakase's default-render error ([RGB tables](lightroom-profiles.md#rgb-tables)). `.cube` LUTs import as RGB-table looks, as Camera Raw's Create Profile makes them ([.cube LUTs](lightroom-profiles.md#cube-luts)); how Create Profile converts 65-division cubes, domains other than 0–1 and shapers, and render parity on photos, are not measured yet. Profile Amount follows Camera Raw 18.7's rule, measured on synthetic looks ([Profile Amount](lightroom-profiles.md#profile-amount)); the remaining error is that of Shadows, Clarity and Contrast with Blacks, which a look's internal settings use.
- **RAWmakase Color** is our own look and is not meant to match Adobe Color exactly. It hasn't yet been compared with Camera Raw renders; built-in presets made with Adobe Standard render with RAWmakase Standard (the camera matrix) when Adobe Standard isn't imported.
- **White balance** at extreme values, and the exact order of profile, WB and calibration, are not verified.
- **Other cameras** (Canon, Nikon, Panasonic, …) render through the same generic path but have not been compared with Lightroom, for lack of sample files.

## Lens corrections

- **Fujifilm built-in vignetting** is applied at 85% log strength to match Lightroom (fitted on three X100F photos).
- **Sony built-in corrections** are available but not measured against Lightroom, and whether Lightroom uses Sony's stored data at all has not been checked. They are off by default and follow Enable Profile Corrections in the Lens Corrections panel. A Lightroom edit or preset with profile corrections on turns them on as the checkbox does: an imported Adobe profile when one matches, else Sony's built-in correction, and the import notice and the panel say the Adobe profile isn't imported. With an imported Adobe profile, Sony matches Camera Raw within ±0.02 EV in the corners ([lens corrections](lens-corrections.md#sony-built-in-corrections)).
- **Lens profile Setup** Default matches as Auto does: RAWmakase keeps no saved per-lens defaults (Lightroom's "Save New Lens Profile Defaults"). The Make menu takes the lens maker from the profile's display name, which may differ from Lightroom's for some third-party lenses. A profile picked in RAWmakase is written without Adobe's `LensProfileDigest` ([lens corrections](lens-corrections.md#choosing-a-profile)).
- **LCP** interpolation uses the farthest focus distance, since focus distance is not read from the files. Tangential distortion terms are ignored; optical centres are read.
- **DNG GainMap opcodes** are applied: OpcodeList1 and 2's (phone lens shading) while decoding ([RAW decoding](raw-decoding.md)), OpcodeList3's (DJI, Hasselblad and Sigma fp DNGs) as a per-channel lens correction ([lens corrections](lens-corrections.md)). GainMaps that cover only part of the image are not read.
- **Remove Chromatic Aberration** measures lateral CA radially from the image centre; Camera Raw's own estimate is not reproduced exactly, and off-centre (decentred) CA is not corrected ([lens corrections](lens-corrections.md#remove-chromatic-aberration)).
- **Defringe** matches Camera Raw's hue ranges and strength, but not its extra reduction next to strong edges ([lens corrections](lens-corrections.md#defringe)).
- **Manual lens vignetting** is not measured against Camera Raw.
- **Manual Distortion** matches Camera Raw's radial map and order on the chart ([lens corrections](lens-corrections.md#manual-distortion)); Lens Corrections has a Distortion amount for it, and Constrain Crop crops out the white it uncovers.

## Geometry

- **Crop & Straighten** has Lightroom's aspect presets with X to swap orientation, the Straighten ruler (Cmd-drag), Auto straighten from Upright's Level analysis, and the Grid, Thirds, Diagonal, Triangle, Golden Ratio and Golden Spiral overlays (O, Shift+O). The Aspect Ratios overlay is not implemented, and Auto straighten measures the photo without its Transform; pasted or synchronized, an Auto angle is measured on each photo, where Lightroom copies it ([transform](transform.md#crop-and-straighten)). Rotation and flips are not written to XMP.
- **Upright** renders imported edits from the corrections Lightroom stores, exactly. For new edits and mode-only presets RAWmakase analyses the photo itself; on 160 photos its Level is within a median 13 px of Lightroom's at 2000 px, Vertical and Auto about 60 px (mostly framing), Full about 180 px ([transform](transform.md)). Guided draws two to four guides and solves them on the photo ([transform](transform.md#guided-upright)); how close its framing comes to Lightroom's is not measured. Constrain Crop keeps the white out of the crop; its rule could not be measured in Camera Raw, which renders the stored crop as it is ([transform](transform.md#constrain-crop)).

## Local and finishing adjustments

- **Spot removal and masks are experimental and early** ([retouching](retouching.md), [masking](masking.md)). Heal and Clone, and brush, linear, radial, color range and luminance range masks with local sliders render; little of it is measured against Camera Raw yet:
  - Heal and Clone's feather is measured in Camera Raw 18.7 (mean coverage error 0.003 on Clone spots), and Heal's tone matching is within 0.5 L* of Camera Raw on flat, gradient and textured tests ([retouching](retouching.md)); the heal algorithm and automatic source choice are our own.
  - Local Exposure, Highlights, Shadows, Clarity and Dehaze add to the global sliders where those render, as a full-frame mask does in Camera Raw, and local Contrast is the global Contrast curve at the mask's amount. Local Whites and Blacks are Camera Raw's separate curves after the global ones, and local Texture scales the global Texture's detail. Local Temp, Tint, Hue, Saturation, Color, Sharpness and Noise are approximations; Noise only reduces noise, and local Moiré, Defringe, Grain and tone curves are not implemented.
  - Gradient transition and brush feather profiles, Flow build-up, Auto Mask edges and range-mask Refine/Smoothness are approximations.
  - The Subject, Sky and Background selections are experimental and our own, not measured against Camera Raw ([masking](masking.md)). Lightroom's Objects, People and Depth selections, the AI Remove mode, AI Denoise, Enhance and Lens Blur are not implemented.
- **Red Eye** is experimental ([retouching](retouching.md#red-eye)): its correction is fitted to Camera Raw 18.7 (mean ΔE76 1.8 on in-gamut colours and grey ramps), and Lightroom's `RedEyeInfo` imports with its measured coordinates. Camera Raw also desaturates strongly red pixels joined to the pupil, such as a red-orange iris; RAWmakase keeps to the ellipse. The pupil detection is our own. Pet Eye with Add Catchlight renders and imports too, fitted to Camera Raw's falloff and catchlight.
- **Lightroom import of spots and masks:** positions (default crop, unrotated sensor frame), long-edge sizes and spot sources were checked exactly against Camera Raw renders. Gradient Full/Zero points, the radial `Flipped` flag (read as inside the ellipse; not flipped applies outside, as the old Radial Filter default), the radial angle's sign, mask blend modes (0 add, 1 subtract, 2 intersect), brush feather from `CenterWeight`, local Hue's scale and legacy range-mask feathering are assumptions: Camera Raw ignored the test files for those. Color range masks and AI masks are reported and left out.
- **Post-crop vignetting** reads all three Lightroom styles (codes 1 Highlight Priority, 2 Color Priority, 3 Paint Overlay; Camera Raw 18.7 renders 0 and an omitted style as Highlight Priority). Highlights applies only to Highlight and Color Priority with a negative Amount, as in Lightroom. Each style has its own operator fitted to Camera Raw 18.7 (Paint Overlay blends linear output, Highlight Priority changes exposure before the tone curve, Color Priority sits between them), with Camera Raw's mask shape for Midpoint, Feather and Roundness. The `vignette-*` cases in `tests/corpus` sit at mean ΔE00 0.8–1.6 from Camera Raw (RAWmakase's default render is 0.9); Amount −100 Highlight Priority (2.0) and Highlight or Color Priority with a low Midpoint (2.8 at Midpoint 20) are further off. Highlight and Color Priority take pixels back through Camera Raw's default tone curve, so with strong tone settings their highlight response is approximate.
- **Grain** is fitted to Camera Raw 18.7's statistics on flat synthetic DNGs 1500–6000 px wide: its strength is within a mean 4% (at most 10%) of Camera Raw's over 21 Amount, Size and Roughness settings, and, as in Camera Raw, its size scales with the photo's long edge. The pattern itself is our own, not Camera Raw's pixels; its slightly "blue" fine grain is not modelled, and whether Lightroom regenerates grain for a crop or an export size is not measured (RAWmakase sizes it to the cropped photo at full resolution).
- Glow and Reshape are not measured and are rejected when non-zero.
- HDR editing and output are not implemented.

## Catalog and interaction

- The RAWmakase catalog is separate from Lightroom's, with no write-back or sync. Custom color-label text is kept, but custom labels display white.
- Rating, flag and label changes, adding to or taking out of the Quick Collection, and title, caption, creator, copyright, location and keyword edits apply to every photo selected in the Grid. Each is one step that Cmd+Z undoes and Cmd+Shift+Z redoes, in one undo sequence shared with Develop, as in Lightroom. That sequence is kept in memory only (up to 100 steps) and is cleared when another catalog opens.
- Copy Settings, Paste Settings, Paste from Previous and Sync Settings transfer the groups chosen in Lightroom's Copy Settings dialog, worked out for each photo's camera. Sync applies to the photos selected with the open one (Cmd or Shift in the filmstrip) in one transaction and one Undo. Settings › Match Total Exposures (Cmd+Option+Shift+M) sets the other selected photos' Exposure so their aperture, shutter speed and ISO end up as bright as the open photo, the same way. The dialog (also used by Sync and New Develop Preset) confirms with Return and cancels with Escape. Auto Sync is missing.
- In the Library, Quick Develop's **Saved Preset** and **Auto Settings** apply to every selected photo (Auto Settings also with Cmd/Ctrl+Shift+U there; in Develop that key stays the open photo's), each measured on its own photo, in one transaction and one Undo, with a progress window whose Cancel discards the whole batch until it starts saving. Presets apply at 100%. As in Lightroom there is no Presets panel in the Library. Lightroom's Cmd+U for Auto Settings is not mapped (U unflags), nor Quick Develop's other buttons (white balance, the tone and presence steps).
- Before/After views follow Lightroom Classic: Before alone (\\), left/right (Y), top/bottom (Option+Y) and split (Shift+Y, or split top/bottom from the toolbar's Before / After menu), with zoom and pan shared by both sides. Before is the photo's starting settings until History's or a snapshot's Copy … Settings to Before sets it, and the menu's Copy Before's Settings to After (Cmd+Option+Shift+→), Copy After's Settings to Before (←) and Swap (↑) move settings between them; changes to the edit are History steps. Before is kept for the open photo only, not saved. Before is always shown with the edit's crop and orientation so the two sides line up; whether Lightroom frames Before with its own crop instead is not checked. Opening a tool returns to the edit alone.
- Reference View (Shift+R, or the toolbar's Reference) follows Lightroom Classic: drag a filmstrip photo onto the Reference side, or right-click it › Set as Reference Photo; Left / Right or Top / Bottom; the reference stays while moving between photos and is cleared on leaving Develop unless locked. It shows its saved edit (or the raw defaults), with its own crop, and is never edited. Its zoom is its own, Fit or 100% (click it; drag to pan), as in Lightroom; Lightroom's Cmd-drag to a custom zoom on the reference is missing. Until it is developed the Library's preview stands in; it then renders like the edit, from the half-size decode at Fit and in full at 100%. JPEG, TIFF and PNG photos can't be references yet, and the reference is not kept between launches.
- Unsupported develop settings are kept and reported, but not rendered.
- The Color Grading wheels work as Lightroom's (3-Way and single views, Shift to constrain, Cmd/Ctrl for fine, double-click to reset), but how Shift picks hue or saturation (the way the drag first moves), the fine ratio (a tenth) and the History name for a drag that changes both ("Shadow Hue & Saturation") are RAWmakase's own, not checked in Lightroom. See [color grading](color-mixer.md#color-grading).
- Panel switches sit in the headers of Tone Curve, Color Mixer or B&W, Color Grading, Detail, Lens Corrections, Transform, Effects and Calibration; each toggle is one History step ("Enable Tone Curve", as Lightroom names it) and changing a setting in a panel that is off turns it back on. Spot Removal, Red Eye and Masking have no switch in their tool drawers yet: they keep imported switches and Red Eye turns its own on. Solo Mode (right-click a panel header) is per side of the window and kept across launches. The Histogram is always shown at the top of Develop's right side, with no header to fold or pin it, and nothing else pins. There is no Navigator panel: the photo views zoom with a slider in their toolbar and show where with a movable minimap over the photo, and a magnifier (Shift+Z) enlarges the photo under the pointer at Fit. Up and Down over a hovered slider move it by its smallest step (Shift: ten); Lightroom's own step sizes, and its + and − keys, are not checked.
- While the pointer is over the photo being edited, the line under the histogram shows the R, G and B percentages of the pixel under it, as Lightroom's, in Melissa RGB (ProPhoto primaries, sRGB tone curve). They are read from the rendered photo before clipping warnings and the monitor profile, not from the screen: at Fit one pixel of the preview, which averages the photo beneath it, at 100% and above one pixel of the photo. The renderer's output is bounded to sRGB, so colours Lightroom keeps outside sRGB read lower here. The White Balance loupe shows the same values. Over Before or the reference photo there is no readout. I cycles the photo info overlay (Info 1, Info 2, off) over the photo in Develop, the same one the Library's Loupe shows; its fields are the Library's, not Lightroom's configurable View Options.
- The histogram's clipping triangles work as Lightroom's (click for shadows or highlights alone, hover to preview, J for both), but its thresholds are RAWmakase's own (encoded sRGB 0.001 and 0.999, [color pipeline](color-pipeline.md#display)), not measured against Lightroom. Dragging in the histogram moves Blacks, Shadows, Exposure, Highlights or Whites, in equal fifths of its width, which Lightroom's regions only approximate.
- Preset Amount works as Lightroom's slider (0–200% after applying a preset that supports one, ended by any other change), but how each setting scales is RAWmakase's own rule, not yet measured: Camera Raw offers the slider only in its own window, so measuring means driving that window, which hasn't been done. See [preset Amount](xmp-presets.md#preset-amount).
- Raw defaults (Adobe Default, Camera Settings, RAWmakase Default or a preset, with per-camera overrides) follow Lightroom Classic, except that photos without an edit follow the current defaults instead of having them written in at import, and Auto Tone, Auto white balance or Upright in a default preset is not applied. Camera Settings starts from the camera's standard camera-matching profile (Camera Standard), not the picture style set in the camera, and needs that profile imported. See [raw defaults](xmp-presets.md#raw-defaults).

## Validation still needed

- More cameras, profiles (Adobe Color and other looks), illuminants and clipped highlights.
- Combinations of sliders: Shadows/Highlights probes reject simple gain addition, but a replacement joint operator has not passed photo validation. Other combinations and their processing order still need verification.
- A repeatable test set that isn't private photos; Piotr plans to design the testing pipeline.
