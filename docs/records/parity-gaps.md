# Remaining Lightroom parity gaps

Status: Record (2026-10-05, last updated 2026-11-20); tone research updated 2026-10-16 for the [scene tone stage](../scene-tone-stage.md); lens, profile and catalog notes updated 2026-10-03. RAWmakase renders close to Lightroom but not identically. Each item below says what differs and, where measured, by how much. Errors are encoded-sRGB mean absolute error (0–1) against Camera Raw 18.6 or Lightroom Classic 15.5 exports, scored with `scripts/lightroom-scorecard.py`; most of the entries below are mean ΔE00 against Camera Raw 18.7 renders of the synthetic charts. For scale: Lightroom X100F references averaged 0.0090, Sony A7 II Camera Raw references 0.0065. The scene tone stage changed the default render and every tone control, so figures measured before it (dated 2026-10-05 and earlier) may have moved. Details: [tone controls](../tone-controls.md), [color mixer and grading](../color-mixer.md), [lens corrections](../lens-corrections.md), [transform](../transform.md).

## Measured and matched (for reference)

These controls were fitted to Camera Raw renders and match within the default-render error, or close to it: default look without Adobe files (DNG ColorMatrix + ACR tone curve), Contrast, color mixer, Saturation, color grading at default Blending/Balance, Transform sliders, built-in Fujifilm, Olympus/OM System, Panasonic and DNG lens corrections, imported Adobe lens profiles (on Sony A7 II photos), DNG embedded profile/exposure/crop, and Fujifilm default crop.

## Tone

- **Scene tone stage**: Exposure, the default white and black point, Whites, Blacks, Shadows, Highlights, Clarity, Texture and Dehaze render on scene values before the profile, fitted on linear-profile Camera Raw 18.7 renders ([scene tone stage](../scene-tone-stage.md), [tone controls](../tone-controls.md#whites-and-blacks)). Shadows and Highlights follow the measured [global rule](../scene-tone-stage.md#the-global-rule): a curve per photo from the histogram of its base levels, which the earlier percentile-keyed tables could not express (positive Shadows and negative Highlights since 2026-10-28, negative Shadows and positive Highlights since 2026-11-17, [below](#negative-shadows-and-positive-highlights-by-the-global-rule-2026-11-17)). Dehaze (a curve per amount, no spatial part) and positive Whites (+100 especially) stay fitted and still differ. Shadows and Highlights on the synthetic chart's flat patches are a degenerate case for Camera Raw (a flat maximum) and are not a measure of either engine.
- **Contrast** follows Camera Raw's curve, after the profile and so after Whites and Blacks, at a pivot predicted from the photo ([tone controls](../tone-controls.md#contrast)); the prediction is within about 0.03 of the pivot that fits each photo best, and its extra error over the default render on photos is 0.003–0.007.
- **Clarity** of both signs follows a model fitted to Camera Raw's scene-value renders of synthetic scenes ([tone controls](../tone-controls.md#clarity)); on photos it leaves most of Camera Raw's field, whose structure is measured [below](#clarity-a-slope-around-a-base-and-no-closed-form-for-either-yet-2026-11-19); most of what is left is a tone curve per photo, and a candidate that predicted it was built and not kept ([2026-11-20](#clarity-on-photos-a-curve-per-photo-that-no-histogram-sets-2026-11-20)). **Texture** follows a model fitted to Camera Raw on synthetic charts ([tone controls](../tone-controls.md#texture)): on the corpus chart its ΔE00 to Camera Raw drops from 2.0–2.2 to 1.5–1.6 at ±100.
- **Parametric tone curve** renders as Camera Raw 18.7 does on the chart ([tone controls](../tone-controls.md#parametric-curve)): the 24 `parametric-*` cases sit at mean ΔE00 0.8–3.0, down from 0.95–7.9, and the looks carrying one at 0.9–1.6, down from 1.7–4.9. With out-of-gamut colors clipped per channel as Camera Raw does ([out-of-gamut colors](../color-pipeline.md#out-of-gamut-colors)), the cases sit at 0.48–1.15. A look's own parametric curve is a second curve after the user's, as in Camera Raw (`look-parametric-darks+30`: 0.93, was 1.41); at Amount 200 Camera Raw's look curve does about 1.6 times its effect at 100, where RAWmakase doubles it. **Point curves** match on ramps, but saturated colors under an S master curve still differ (ramp MAE 0.0024, peak 0.11).
- **Curve Refine Saturation** renders as Camera Raw 18.7 does on the chart ([tone controls](../tone-controls.md#refine-saturation)): the `curve-*-refine-saturation-*` cases sit at mean ΔE00 1.1–1.5, against 1.4 for the same S curve at 100. Values above 100 render as 100, as in Camera Raw. It imports, is written back, and has a Refine › Saturation slider (0–100) under the RGB point curve.
- **Point Curve menu** has Lightroom's Linear, Medium Contrast and Strong Contrast (points from Lightroom Classic's catalog), saved curves and Save…; saved curves live in RAWmakase's data folder, not in Lightroom's Curves folder, and the menu's curve name is not written to XMP ([tone controls](../tone-controls.md#point-curve-menu)). Whether Lightroom's built-in choices also reset the colour curves was not checked; here they set the RGB curve only.
- **Auto** tone is fitted to Lightroom Classic's own Auto values, from 452 Auto Settings steps on 439 catalog photos with every fifth photo held out; each slider is a linear fit of one or two display-encoded percentiles of luminance, of the brightest channel or of chroma, and the held-out error is a median 1 step on Contrast and 0.21 EV on Exposure. Lightroom changed Auto in mid-2019 and the catalog's history shows both versions, so the slopes are fitted on every photo and the levels, Contrast, Vibrance and Saturation on the current Auto alone. The WB menu's Auto white balance is fitted to Lightroom's Auto values (a median of 2.7 mired off on 133 photos); see [tone controls](../tone-controls.md#auto).

### The base level: a local Laplacian filter, and what the curves are once it is right (2026-11-20)

A second thread looked at Shadows and Highlights from the start, on renders already on
disk: Camera Raw 18.7's scene-stage renders of 243 training photos at 1000 px, the
engine's taps of 145 of them, and the stress families' frozen references. No Camera
Raw render was made. Nothing is merged; the engine change is a local commit on
`t3/tone-engine-lightroom-parity`.

**What the base level gets wrong.** For each coefficient of a Laplacian pyramid of
log luminance, the share of the large-scale curve's compression it receives (0: the
detail keeps its contrast; 1: it follows the curve), Camera Raw's own default render as
the input and main as the control:

| Shadows +100, in the shadows | Faint detail, 1 px | Faint, 8 px | Over 2 stops, 1 px |
| --- | --- | --- | --- |
| Camera Raw | 0.11 | 0.35 | 0.72 |
| main | 0.56 | 0.59 | 0.77 |

Camera Raw's share grows with a difference's size at every scale and only mildly with
scale. Main's is 0.54 to 0.77 for everything: its guided filter's weight is set by a
window of 6% of the frame, and a photo's shadows have over half a stop of variance in
nearly every such window, so the base follows fine detail and noise as it follows
edges. At Highlights −100 Camera Raw expands faint detail in the highlights (−0.45 at
1 px, against its own default render) where main compresses it (0.58).

**A local Laplacian filter has Camera Raw's pattern.** With the best curve for each
photo (an oracle, so the curve's own error is out), the error left by the spatial
operator alone, in stops of log luminance, median over photos, in each control's own
tones:

| | Photos | Guided base (main) | Local Laplacian base |
| --- | --- | --- | --- |
| Shadows +100 | 29 | 0.141 | 0.085 |
| Shadows −100 | 29 | 0.137 | 0.086 |
| Highlights −100 | 30 | 0.119 | 0.095 |
| Highlights +100 | 30 | 0.117 | 0.100 |

The filter is Paris, Hasinoff and Kautz's (2011) in Aubry's sampled form (2014), on
log2 luminance: about a reference level `g`, a sample `i` becomes `w·g + (1 − w)·i`
with `w` a Gaussian of `i − g`, 1.5 stops wide. Its response to the gain curve
`G(l) = l` is one base level per pixel, and a curve read at that level does as well as
the full filter, so the engine's structure stays: curves of a base level, on the CPU,
the GPU and in masks. Widths of 1.25 to 1.5 stops are level; 2 and over are clearly
worse. A pixel finer than the 512 px map reads `w(d)·B + (1 − w(d))·l` with `d` its
distance from the map's own level there; a 250 px map on 1000 px renders costs 0.004
stop. No explicit filter tried does as well (guided with ε 2 or 8, a radius of 1%,
bilateral grids of 0.8 to 3.2% and 1 to 3 stops). With the curve fitted on the centre
of the frame alone (18 photos), the centre goes from 0.092 to 0.061 and the corners
from 0.122 to 0.083, and the guided base's over-lift of vignetted corners (+0.034 stop)
goes to +0.002: Camera Raw does not lift a smooth falloff as it lifts a dark object.

On the three training photos with blown areas that were looked at, an oracle curve on
the guided base leaves 0.31, 0.26 and 0.41 stop in the highlights at Highlights −100,
and on the filter's base 0.08, 0.15 and 0.12. One of them holds a large blown shirt and
small bright spots between leaves: any curve of the guided base leaves the shirt too
bright and the spots too dark at once. The size dependence recorded on 2026-10-22 is
this. The white curve stays on every pixel, after the gain: an oracle curve that holds
the shoulder itself is worse (0.115 against 0.096 in the highlights, the median of 17
photos with highlights, both through the full filter).

**In the engine.** `local_tone::laplacian_base` on the map, read by
`LocalToneMap::base` and the shader; a Python copy agrees with it to the last digit.
The rule's two ensembles were refitted through it with the repository's script.

- Training photos, rendered (both builds fitted on them): Highlights −100 in the
  highlights 0.147 → 0.102 (9 photos closer, none further of 12), Shadows +100 in the
  shadows 0.210 → 0.158 (14 and 2 of 19); the error of structure under 32 px 0.085 →
  0.058 and 0.123 → 0.093.
- Held out by photo, the rule's curve error does not move: Shadows +100 0.172 to 0.212
  over four folds against 0.164 to 0.202 through the guided base, Highlights −100
  0.132 to 0.171 against 0.134 to 0.151. The curve's own prediction is the larger
  error, and a better base does not help a wrong curve.
- Stress families, final renders against the frozen references (24 photos, median
  ΔE00 of a photo): Highlights −100 1.19 → 1.12, Highlights +100 1.67 → 1.57, Shadows
  −100 0.93 → 0.96, Shadows +100 2.49 → 2.48 with the mean worse (2.82 → 3.13; 8
  closer, 12 further), Shadows +50 1.42 → 1.71. The park's blown sky goes from 9.3 to
  5.8 L\* too dark and the palms' from 4.8 to 8.1: the level of a blown sky still
  swings with the fit.

**Shadows is one curve, mirrored by the slider's sign.** Through the filter's base, a
photo's own best curve leaves 0.058 stop at Shadows +100 (240 photos; 0.088 through the
guided base). One curve for all photos, read relative to the log of the photo's mean
luminance, leaves 0.125 held out by photo (relative to the median 0.239, to the 90th
percentile 0.161, to the level itself 0.349; power means of 0.75 to 1.5 and the mean of
the scene input do the same as the mean). On 135 of those photos main, whose rule was
fitted on them, leaves 0.144, and the one curve 0.129 (63 photos closer, 56 further).
Its gain at 0 to 5 stops under the mean:

| | 0 | −1 | −2 | −3 | −4 | −5 |
| --- | --- | --- | --- | --- | --- | --- |
| Shadows +100 | +0.14 | +0.63 | +1.27 | +1.88 | +2.42 | +3.23 |
| Shadows −100 | −0.15 | −0.61 | −1.27 | −1.93 | −2.55 | −3.46 |

About 0.62 stop per stop of depth, the same field for both signs. In the engine
(`RAWMAKASE_UNIVERSAL`, a test hook) on the stress families: Shadows +100 2.49 → 2.05
(mean 2.82 → 2.68; 12 photos closer, 9 further), +50 1.42 → 1.40, −100 0.93 → 0.89;
the band of L\* 25 and under is level at the median (3.70 → 3.52) and worse on average
(4.79 → 5.07). On 48 training photos, in the rule's own training set, 0.149 → 0.136 at
the median and 0.157 → 0.173 on average. So a curve with nothing fitted per photo is
level with the network, better at the median and worse in the tail. A shift of the key
per photo takes the 0.125 to 0.086, a scale to 0.083, both to 0.073. Neither follows
the camera's baseline exposure (r 0.10) or the camera (the shift's spread within a
camera is the whole spread); both follow the top's distance over the mean weakly
(r 0.36 to 0.39).

**Highlights is not one curve of one key.** From the engine's scene input (135 photos,
62 with over 4% of the frame in the highlights), a photo's own curve leaves 0.094 in
the highlights (0.119 through the guided base). One curve relative to the median leaves
0.224, to the log of the mean 0.348, to the 90th percentile 0.392, to the top 0.414,
to the level itself 0.351. A knee and a slope per photo leave 0.171, and the knee
predicted from ten statistics, held out, 0.206. The photos' own curves run −0.75 to
−0.95 stop per stop above a knee about 1.5 stops over the median on wide-range photos
(an output slope of 0.05 to 0.25); at the same share of the photo above a level,
denser levels keep more slope (0.20, 0.45 and 0.73 at 1 to 3, 3 to 10 and 10 to 30% of
the photo per stop), but that orders the slopes only weakly (rank correlation 0.2).

**Ruled out.** A local Laplacian filter with one remapping of `i − g` for every photo
and no curve: 0.246 stop held out at Shadows +100, and 0.158 with a remapping per
photo. The curve of a level is needed.

**Correction (2026-11-20, evening): the black-first reading below is withdrawn.** It
was built as a change of the stage and reviewed (a same-provider substitute review,
since no other provider had quota), and a measurement then settled it. On the one
controlled pair there is, the synthetic dark scene with small bright gaps, main reads
0.588, 0.758 and 0.552 EV of lift at Exposure −2, 0 and +2 against Camera Raw's 0.600,
0.785 and 0.572, within 0.03 EV, and the black-first build reads 0.48, 0.51 and 0.51.
Camera Raw's own two renders of that scene (0.52 EV without its default black, 0.79
with it) are a gain of 0.52 EV followed by the same black: that gives 0.78. So Camera
Raw applies the gain and then the black, as the stage does, and on that scene the
rule's output is the true gain. The paragraph's claim that Camera Raw's result is its
default render times the gain is wrong, and so was a reading of this pair as "the
curve reads the level the black leaves" (the arithmetic gives 0.95, not 0.79).

What stands is the measurement: on photos the stage's deep tones are too bright at
Shadows +100, by the amounts in the table below, and a build that multiplies the
default render by the rule's gain removes most of that. The reason is in the rule's
targets, not in the stage's order. Its photo targets are ratios of Camera Raw's
renders (the render at the slider over the default render), and both have the default
black in them, so in deep tones a target is larger than the gain that produced it
(for a tone 2.6 times the black's removal up, 0.79 EV where the gain is 0.52); its
mosaic targets were rendered without the default black and are true gains. The stage
applies a photo's inflated gain to the scene value and then takes the black again. The
fix that follows is the one the second ensemble already uses for the white curve: a
target is the gain that gives Camera Raw's result through the stage's own curves, the
default black included. Not done yet. The numbers below are kept as measured; read
"black first" in them as "the rule's gain taken as a ratio of renders", which is right
for photo-like scenes and wrong by 0.28 EV on the probe.

**The rule refitted with targets through the black (2026-11-20, night).** Both
ensembles were refitted with each photo's renders taken back through the stage's own
curve before the gain is read: the default black at the photo's dark key for the default
render and, for a Shadows render, at the key the stage moves it to (the median scene
gain over the map for positive Shadows, the gain on the darkest 0.1% for negative).
Mosaics, rendered without the default black, are as before. The engine's code is main's;
only the two data files differ (a local branch, fitted by a scratch driver; the
repository's script is not changed yet). A first try with the key left unmoved under
Shadows failed the probe at Exposure −2 (0.90 EV against Camera Raw's 0.60, main 0.59)
and made negative Shadows worse, so the key moves as it does on main.

| | main | Refitted |
| --- | --- | --- |
| Shadows +100, 24 training photos, scene stage: error of the tones under 0.02 at default | 0.332 stop | 0.236 |
| Its bias at scene levels of 0.001 to 0.002, to 0.004, to 0.008 and to 0.016 | +0.59, +0.32, +0.20, +0.11 | +0.03, −0.11, −0.02, −0.02 |
| Shadows −100, the same error | 0.343 | 0.296 |
| Highlights −100, error of all pixels over 0.003 | 0.128 | 0.110 |
| Shadows +100, final renders, the band under L\* 25, mean \|L\* bias\|: stress families (24 photos), main families (29) | 3.11, 3.91 | 2.26, 2.32 |
| The band's mean ΔE00 there | 4.79, 5.48 | 4.40 (9 photos closer by 0.3, 4 further), 4.71 (14, 4) |
| Shadows +100, median ΔE00 of a photo (mean) | 2.49 (2.82), 2.43 (2.52) | 2.34 (2.92), 2.33 (2.67) |
| Shadows +50, the band's bias | 1.13, 1.47 | 0.96, 1.05 |
| Shadows −100, median ΔE00 of a photo | 0.93, 0.77 | 0.92, 0.90 |
| Highlights +100 | 1.67, 1.30 | 1.59, 1.18 |
| Highlights −100 | 1.19, 1.22 | 1.23, 1.25 |
| Shadows +100 with Highlights −100 | 2.68, 2.07 | 2.68, 1.92 |

The photos of the last eight rows are ones nothing was fitted on. The deep tones' bias
at Shadows +100 goes on training photos and falls by a quarter to two fifths on the
others; the mean ΔE00 of a photo at Shadows +100 is a little worse and Shadows −100 on
the main families too. The synthetic targets move, since the photo targets go through
the shared network: the dark scene with gaps reads 0.707, 0.935 and 0.667 EV (0.10 to
0.15 too much, inside the test's 0.25), the chart is further at Highlights +100 (mean
ΔE00 6.99 → 7.48) and +60 (4.91 → 5.22) and Shadows −100 (4.95 → 5.08), and seven scene
probe cases are further than their baselines. Held out by photo, the curve's error is
0.16 to 0.17 at Shadows +100 (0.16 to 0.20 before), 0.17 to 0.19 at −100 (0.20 to 0.24)
and 0.12 to 0.14 at Highlights −100 (0.13 to 0.15), each against its own targets. Not
merged: it needs the script changed and the files regenerated from it, the pair model
refitted, the gate, and a decision on the synthetic cases.

**The default black comes before the local gain (later the same day; withdrawn, see
the correction above).** With every
build tried, Shadows +100 left the darkest tones too bright, and with one sign: on the
stress families the change Shadows +100 makes is +3.1 L\* too large where Camera Raw's
render is under L\* 10 and +2.2 at 10 to 20 (main; the one curve +2.7 and +2.3; the
refitted network +3.6 and +2.9). On 137 training photos at the scene stage the default
renders agree in the deep tones (−0.0001, scene-linear) and at Shadows +100 main is
+0.0023 to +0.0032 above Camera Raw, on nine photos in ten. The error grows with how
far the photo's darkest level is lifted:

| Main's darkest level after the lift | Photos | Deep tones too bright |
| --- | --- | --- |
| under 0.004 | 34 | 0.14 stop |
| 0.004 to 0.008 | 39 | 0.34 |
| 0.008 to 0.016 | 37 | 0.84 |
| over 0.016 | 8 | 1.59 |

Camera Raw's darkest level after the lift is 0.34 to 0.50 of main's in every row. The
stage applies the local gain and then the default black, with the black's key moved by
Shadows' median gain over the map. Three readings were tried:

- *The key moved by the darkest levels' gain* (as negative Shadows has it): little
  (the stress families' dark band, mean |L\* bias| 4.08 → 3.88).
- *The black table continued past its last key* (2^−7), where Shadows +100 puts most
  photos: it removes a quarter of the dark tones' error on training photos (0.351 →
  0.268 stop at its best setting), but it is a patch: at default, on the 15 photos
  whose dark level is over that key, Camera Raw's subtraction does stay near 0.005 and
  the engine matches it within 0.01 stop.
- *The black taken before the gain*: the result is the default render times the gain,
  so the gain scales what the black removes. In the stage's order that is its result
  less `gain·gone(x) − gone(gain·x)`, with `gone(v)` what the photo's own default black
  removes at `v`, and the key left unmoved by Shadows (`RAWMAKASE_BLACK_FIRST`, a test
  switch; nothing fitted; the default render cannot change).

On 24 training photos at Shadows +100, scene stage, with the refitted network's curves:

| | After the gain (as built) | Before the gain |
| --- | --- | --- |
| Deep tones too bright, by the four rows above | +0.15, +0.33, +0.44, +0.99 | −0.03, −0.11, 0.00, +0.14 |
| Error where the default is under 0.02 | 0.382 stop | 0.185 |
| Error of all pixels over 0.003 | 0.114 | 0.101 |
| Darkest level against Camera Raw's | 1.79 times | 1.06 times |

With the one curve the same switch gives 0.351 → 0.258 in the dark tones. It suits the
network's curves because their targets are ratios of Camera Raw's renders, which are
gains of values the black has already been taken from. Final renders, photos nothing
was fitted on (mean |L\* bias| of the band L\* 25 and under, Shadows +100 and +50):

| | main | refitted network | + black first | one curve | + black first |
| --- | --- | --- | --- | --- | --- |
| Stress families, +100 | 3.11 | 4.17 | 2.06 | 4.08 | 2.65 |
| Main families, +100 | 3.91 | 4.30 | 2.46 | 3.00 | 2.14 |
| Stress families, +50 | 1.13 | 1.44 | 0.85 | 1.46 | 0.96 |
| Main families, +50 | 1.47 | 1.66 | 1.04 | 1.06 | 0.78 |

The median ΔE00 of a photo at Shadows +100, main → the one curve with the black first:
2.49 → 1.98 on the stress families (13 photos closer, 9 further) and 2.43 → 2.12 on the
three main families (17 and 10); with the network's curves 2.35 and 2.20, with a worse
mean (2.82 → 2.92 and 2.52 → 2.89). Shadows −100 does not gain from it: 0.77 → 0.92
with the network's curves on the main families and 1.00 with the mirrored one curve,
though its dark band's bias falls (0.65 → 0.52). With an oracle curve per photo the two
orders fit the deep tones alike when the gain-first form gets a free black constant per
photo (0.147 and 0.145 stop on 126 photos, main 0.432), so the renders do not prove the
order; the black-first form needs no constant.

**More on the curves (later the same day).**

- A photo's best shift of the one Shadows curve is a property of the photo: its values
  at +100 and at −100 correlate 0.94 (their difference has a spread of 0.11 stop, each
  of them 0.32). A ridge on the histogram predicts little of it (held out 0.125 →
  0.116 stop); texture at three scales, chroma, the share and layout of the dark, ISO
  (r 0.13) explain 3 to 6% of its variance, and trees on all of those a third.
- The key is the whole frame's mean: relative to a Gaussian mean over 50, 25, 12 and 6%
  of the long edge the one curve leaves 0.127, 0.188, 0.281 and 0.365.
- On the three main validation families the one curve beats the network at positive
  Shadows (+100: 2.43 → 2.18 against 2.31, +50: 1.55 → 1.48 against 1.61) and loses at
  −100 (1.01 against 0.86; main 0.77).
- Highlights as one shape between two keys (median, mean or a quartile to the 90th, 99th
  or 99.9th percentile, the top or the white; the gain times 1 or times the keys'
  distance): 0.25 to 0.42 stop in the highlights, no better than one key.
- Highlights after the default white curve (the default render times a gain of its own
  base) is worse than before it: 0.132 against 0.094 with an oracle curve, further on 43
  of 62 photos. The white curve does not relax under Highlights −100 either: faint fine
  detail in the highlights keeps 0.99 of its scene contrast (quartiles 0.88 and 1.21),
  which is the default curve's slope where the pixels end (0.95; the same method on the
  default render reads 1.06 times the curve's slope); main keeps 0.79.
- With the scene input clamped at the file's white level (the other thread's switch),
  the oracle floor in the highlights of the photos the clamp changes goes from 0.14–0.16
  to 0.09–0.10 stop.
- A full-size crop of a blown building on a stress photo at Highlights −100 (range from
  the 5th to the 95th percentile, L\*): main 34.2, the new base 40.7, Camera Raw 43.3;
  fine detail (standard deviation) 0.49, 0.59, 0.67.

**Open.** What sets a photo's key and scale for Shadows beyond the mean; the form of
the Highlights curve (the network stays the best predictor there); the rule fitted as
a correction of the one Shadows curve, not from nothing, since it does not carry to
photos it has not seen; the rule's photo targets as the gains that give Camera Raw's
result through the stage's own default black, and what the black's key should do under
Shadows once they are; Clarity and the pair model on the new base; the gate, the GPU tests and the
stage probes, none of which ran.

### Negative Highlights in blown skies: an untrained top, and a shoulder on every pixel (2026-11-20)

Camera Raw recovers more of an overexposed sky at Highlights −100 than RAWmakase does.
This round looked at where that comes from. Camera Raw 18.7; training photos for
anything fitted; the stress families have validation and holdout references only, so
their validation photos were looked at and never fitted on, and holdout stayed
untouched. Nothing in the engine changed.

**Where the error is.** The gate's median over a photo hides it: on validation's 56
photos Highlights −100 adds 0.42 ΔE00 at the median, but the reference's band of L\*
90 to 100 is 2.2 L\* too dark (pooled) and 25 photos are over 2 L\* off in the bands 70
to 100.
On the five stress photos with a large blown area (at white in every channel in Camera
Raw's default), the mean L\* of that area at −100, RAWmakase against Camera Raw: a park
under a blown sky 83.3 / 92.6, palms against the sun 90.7 / 95.5, a paved square 94.9 /
96.5, a wall and storm clouds level. The data are not the cause: at Exposure −2 the
same areas carry Camera Raw's contrast within 10%.

**The top of the curve was never trained.** The first ensemble (Shadows +100,
Highlights −100) takes its targets only where Camera Raw's default is under 0.6 of the
white; the second (Shadows −100, Highlights +100) goes to 0.98 with clipped results as
bounds. A pixel at white in the default is in neither. 41 of the 405 training photos
have over 0.5% of their pixels there; Camera Raw's −100 leaves each such area a flat
plateau a little over the brightest unclipped pixels, anywhere from L\* 35 to 97. On
the 14 of them with a DNG, main's plateau is off by 7.4 L\* (RMS over photos, median
−2.3, six photos over 5 L\*), and the unclipped pixels over 0.8 of the white by 4.8.

**Retraining it.** The second ensemble's method (targets with the white curve undone)
for Highlights −100, with the blown pixels kept (their scene level the engine's own,
exact where Camera Raw's result is under the white, a lower bound where it stays
there) and 1000 more samples per photo from over 0.6 of the white. In-sample the
plateaus are off by 3.7 L\* and the near-white pixels by 3.3.

- As the whole Highlights −100 curve it fails validation: the top is better (band 90
  to 100: −2.46 → −1.64 L\*) but the bands 20 to 40 go from −0.03 and −0.31 to −1.22
  and −1.92 L\*, and the median from 0.71 to 0.86. It over-pulls dark and middle tones
  on photos it has not seen.
- Blended in above the first ensemble's range (the first's curve under scene level −2,
  the retrained one over −1) it holds: median 0.71 → 0.69, highlights ΔE00 (pixels of
  L\* 90 and over) 1.90 → 1.75 with seven photos closer by 0.3 and two further, band
  90 to 100 −2.46 → −1.63. The park photo's highlights go from 6.18 to 2.79 and its
  sky from L\* 83.3 to 88.3. Two gate checks fail: a sunset whose sky keeps a channel
  at white where Camera Raw's does not, and one photo's worst region.
- One regression is systematic. With Camera Raw's nearly white results (0.97 to
  0.995) taken as levels and not as bounds, and the soft shoulder below, two fits that
  differ only in the networks' seeds agree on validation: highlights ΔE00 1.90 → 1.69
  and 1.65, seven photos closer by 0.3 and one or none further, the park photo 6.18 →
  2.68 and 2.77, band 90 to 100 −2.46 → −1.68 and −1.87. Both leave one region of the
  palms photo's unclipped sky 5 to 6 L\* darker than main's (main is 2 to 3 under
  Camera Raw there) while its blown part moves by under 1 L\*, and the gate's other
  failures move between photos with the seed (another photo's region; the line for a
  pixel at white crossed on a sunset, or on the paved square at −50). A photo with a fifth of its frame blown under daylight is a
  class the 41 photos do not teach.

**The shoulder acts on every pixel.** Inside the blown areas the slope of the result
on the scene level was measured at three sizes (fine: under 5 px of 2048; cloud: 5 to
41 px; large: beyond), with Camera Raw's render taken back through the engine's own
relation between its scene-tone output and its final render:

| Photo | Camera Raw | main |
| --- | --- | --- |
| Paved square | 0.94 / 0.83 / 0.42 | 0.58 / 0.60 / 0.45 |
| Wall | 0.56 / 0.51 / 0.52 | 0.35 / 0.37 / 0.36 |
| Storm clouds | 0.79 / 0.88 / 0.74 | 0.68 / 0.69 / 0.71 |

Main's three slopes are nearly equal, and in the range of the default white curve's
own log-log slope at that depth (on the training photos 0.50 to 0.72 half a stop under
the white, 0.33 to 0.57 a quarter stop under it). The local gain there is nearly constant, so the engine's operator is
the shoulder alone, applied to each pixel, and it halves all contrast near the white.
Camera Raw's result sits nearer the white and keeps more contrast. Letting the
Highlights curve read a smoother level (half the pixel's own level from the map, or a
bilateral blur of the map at 1 to 8% of the long edge) changes nothing there.

A test build applied the shoulder to the level the pixel is read at and kept the
pixel's distance from it, in proportion to the slider: with `W` the white curve, `l`
the pixel after the local gain and `b` its base level after it, the output is
multiplied by `((W(b)/b) / (W(l)/l))` to the power of the amount. On main's curve the
slopes become 0.66 / 0.68 / 0.44, 0.50 / 0.62 / 0.47 and 0.83 / 0.86 / 0.74. With the
blended curve as well, the share of each blown area still at white at −100 comes out
at 5.9%, 40.7% and 26.6% against Camera Raw's 8.6%, 42.7% and 20.7% (main: 0.1%,
35.8%, 0.0%); nothing was fitted to those. On the training photos' unclipped top tenth
it does nothing (the band gains stay 0.76 0.75 0.73 0.71 against Camera Raw's 0.89
0.83 0.78 0.71): there the base follows the pixel, and that loss is the curve's.

It does not pass as it is:

- The white curve is not the identity under the shoulder, so the same factor darkened
  dark texture (more pixels at black on four photos). Holding the curve's ratio under
  middle grey removes that.
- Camera Raw does not leave pulled whites at white. The 99.9th percentile of the
  brightest channel, linear, on three of the photos: 0.996 to 0.998 at −25, 0.989 to
  0.996 at −50, 0.969 to 0.987 at −100 (the palms keep a sun at white until −100,
  then 0.964). The test build's lifted pixels reach an encoded 0.997 where Camera Raw
  holds 0.992, on 1 to 2% of the frame; the difference is 0.2 L\* but it crosses the
  gate's line for a pixel at white.
- A ceiling at what the photo's top base level renders as removes the contrast again
  (storm clouds 0.59 / 0.66 / 0.62, under main). A soft limit between the base's
  rendered level and the white keeps the clipping at main's and fails no pair of the
  gate's highlights sets, but keeps about half the gain (0.60 / 0.63, 0.42 /
  0.50, 0.71 / 0.75) and moves validation little: highlights ΔE00 1.90 → 1.83, no
  photo closer or further by 0.3.
- With the shoulder on the base, a curve fitted as if each pixel went through the
  shoulder is 1.6 L\* too bright within half a stop of the white on the training
  photos. Retrained with targets written for the shoulder on the base it is +0.4
  (main: −1.7), with the palms region above.

**Voided.** Three test builds of 2026-11-19 (two retrainings and a control) read the
retrained ensemble through the second ensemble's input statistics instead of their
own; their numbers (−8.0, −5.9 and +6.5 L\* on the plateaus) are withdrawn. The
engine's dumped curve against the network's own prediction on one photo showed it
(−4.2 against −3.3 stops).

**More photos do not settle the top.** The 102 train-split photos with over 0.5% of
the raw file clipped that the rule had not seen were rendered (92 usable) and the top
refitted on 467 photos, 197 of them with blown pixels. Validation's highlights ΔE00
goes 1.90 → 1.56 with seven photos closer and one further, but that one is the park
photo: 6.18 → 7.82, its sky at L\* 80.9 where the earlier fits had 88 and main has
83.3. The palms region stays 5 L\* darker than main's. What a photo gets at its top
swings by several L\* with the training set, not only with the fit.

**The top as a slope.** Over base levels above −1, Camera Raw's result on a training
photo is close to a line through its result just under −1. Its slope (1 would be a
gain that stops growing) has a median of 0.83 over 204 photos, quartiles 0.65 and
0.97; 0.97 on photos without blown pixels, 0.70 on those with many. Main's curve
over the same levels has 0.69 on 61 photos, 0.24 lower photo by photo at the median,
correlation 0.47: main keeps pulling harder the brighter it gets. Three test builds
on main's curve, no network at the top:

- The gain held from −1 up with one slope (0.85, the best single slope on the
  training photos) puts the daylight stress skies on Camera Raw (park 93.2 against
  92.6, palms 96.3 / 95.5, paved square 96.8 / 96.5) and overshoots the rest: the
  wall 99.0 / 97.0 with 80% of the area at white against 43%, and the training
  photos' plateaus by 10 L\* at the median (dark scenes with small lights, where
  Camera Raw's slope is far lower).
- The slope is not predicted by the photo's histogram: from the top, the mean, the
  gain just under −1 and the share of the frame above it, leave-one-out, R² is 0.33
  and what is left of the result over the top is 0.257 stop against 0.263 with one
  slope and 0.178 with the photo's own.
- Main's curve tilted by its mean shortfall (0.22 stop less pull per stop above −1)
  takes the park to 88.1, the palms to 94.0 and the paved square to 96.9, with no
  region of the palms worse, and overshoots the storm clouds and the wall (62% and
  59% of the area at white against 21% and 43%); on the training photos the plateaus
  are off by 6.7 L\* (main 7.4) and the unclipped near-white pixels by 6.8 (main
  4.8).

Nor does the pull depend on how far a pixel sits above its surroundings: fitted over
210 photos together, the gain's slope on the base level less a Gaussian mean of the
map (8 to 128 map pixels) and its slope on that mean are the same (−0.38 and −0.42
at 8 px, −0.37 and −0.35 at 128), and what is left does not move (0.27 stop; 0.20
with a slope per photo).

**Probes: a bright area in a photo.** Two training photos' default renders were
written as linear files with a bright area added, and rendered by Camera Raw at
default and at −100 and by main (about 280 scenes; luminance of the area at −100, 1
is the white).

- *Its share of the frame.* A textured band across the top or a centred square, 0.7
  to 3.6 times the white, with two stops of sensor headroom. Up to 20% of the frame
  the area ends at the same low value whatever its level (band of 5%: 0.28 to 0.21;
  square of 1%: 0.30 falling to 0.14 as it gets brighter), just over the brightest
  tones of the rest. At 40% it keeps about half its level in stops (0.26 rising to
  0.61). Main is 0.3 stop too bright over these scenes, the opposite of its error on
  the stress skies.
- *A sky that runs into the sensor's white* (a ramp with cloud texture over 25 or 45%
  of the frame, headroom 0.65 stop). The blown part ends 1.03 to 1.26 times the tones
  just under it, so its level depends on what leads up to it: 0.48 to 0.80 where most
  of the sky is unclipped, 0.06 to 0.30 where nearly all of it is clipped. Main gives
  0.41 to 0.69 in every case (0.93 stop RMS against Camera Raw).
- *Flat or textured.* A band of 20 or 40% at 1.5 times the white in four states, on a
  daylight photo: a clipped plateau 0.085 and 0.045; unclipped and flat at the same
  level 0.085 and 0.045; unclipped with 4% texture 0.22 and 0.44; textured just under
  the clip 0.29 and 0.54. It is flatness, not clipping, that Camera Raw answers to: a
  flat bright area ends just over the rest's brightest tones and takes those down with
  it (the ground's 99th percentile 0.067 against 0.19). Main gives 0.32 to 0.34 and
  0.53 to 0.62 with no difference between the states.
- *How much texture.* The same unclipped band of 40% with smooth texture of 0, 0.25,
  0.5, 1, 2 and 4%: 0.045, 0.084, 0.113, 0.193, 0.322, 0.442, and the ground's 99th
  percentile 0.038 to 0.170 with it. White noise of 1% a pixel: 0.660. A smooth
  gradient of 10 or 40% across the band and nothing else: 0.039 and 0.076, as flat.
  The pull falls steadily as the fine detail grows, noise included, and gradients do
  not count. The rule's texture inputs are measured on the 512 px map, where a pixel's
  noise is averaged away and 4% of smooth texture is under 0.02 stop.

This is the plateau "a little over the brightest unclipped pixels" of the training
photos, and it explains the stress skies: a clipped sky reached by a ramp of unclipped
tones stays bright, one that meets dark ground is taken down to it. A curve per photo
on a base level cannot tell the two apart unless the rule knows which levels are flat.

**The probes as training photos.** 150 such scenes on ten photos (ramps into the
clip, flat and textured bands, three shares and three headrooms) were added to the 467
photos and the top refitted. On scenes over a photo it had not seen the fit follows
the clipped skies (0.26 against Camera Raw's 0.30 where main has 0.68; 0.38 stop RMS
over the set against main's 0.93) but crushes textured bands with the flat ones (0.39
against 0.65): the rule's texture inputs do not separate 4% texture from none. On the
stress photos the palms' blown area lands on Camera Raw (95.4 against 95.5) and the
park's sky gets worse again (78.3; main 83.3; Camera Raw 92.6). Not a candidate.

**Flat is one number for the photo, and the probes' size matters.** More probes on the
same band (40% of a 1000 px daylight scene at 1.57 times the white), Highlights −100:

- *Not spatial.* With the band's left half flat and its right half noisy, both halves
  end at the same value (0.41 and 0.42; all flat 0.045, all noisy 0.66). A quarter
  noisy gives 0.13, three quarters 0.62, a flat half beside 4% texture 0.20. The band
  is uniform inside, and a gradient across it comes out at half its size. The pull is
  one curve for the photo, set by how much of a level is flat.
- *How little detail is enough.* Noise of 0.025, 0.05, 0.1, 0.25 and 1% a pixel: 0.067,
  0.118, 0.385, 0.633, 0.660. On one channel alone it counts the same. A NoiseProfile
  in the file changes nothing, nor does blurring the rest of the scene or adding noise
  to it.
- *Size.* The same noise counts for less in a larger file: 0.1% at 500, 1000, 2000 and
  4000 px gives 0.65, 0.39, 0.14 and 0.09. Smooth texture goes the other way: 0.25%
  of it gives 0.084 at 1000 px whatever its scale (blobs of 2 to 32 px) and 0.67 at
  2000 and 4000 px, and the same 1000 px scene with every pixel doubled gives 0.67.
  The statistic behind this was not found. What follows for training: scenes of
  1000 px with smooth texture only (the rule's mosaics, the 150 scenes above) sit
  between flat and textured for Camera Raw as no photo does. A scene needs noise of
  about a percent a pixel to be read as a photo is.

**Camera Raw keeps nothing above the white level.** A scene whose samples run to 1.64
times the file's WhiteLevel, with a noisy ramp from 0.6 to 1.55 times the white:
Camera Raw is flat from the white on at Exposure −2 (0.155, 0.182, 0.209, 0.239, 0.270,
then 0.275 to the end, with no texture left), at default and at Highlights −100.
RAWmakase's decoders scale by the white level and keep what lies above it: its
Exposure −2 rises to 0.523 with the texture. 17 of 228 training photos hold more than
0.1% of their samples over the white level (up to 2.1 times it; three with 4.5 to
5.8% of the frame), and those are most of the photos with a large clipped area. A
stress photo from a Canon EOS Kiss M shows it in a photo: its samples run to 1.39
times the white level, Camera Raw's flat area at −100 (5.4% of the frame) is where all
three channels reach the white level (6.0%), and only 1.1% of the frame is at the
sensor's saturation. With every channel held at its white before the highlight
reconstruction (a test switch), the probe's Exposure −2 equals Camera Raw's. On the
gate's highlights sets the clamp alone fails no pair (91 pass, 35 inconclusive) and
moves little: three photos closer by 0.2 to 0.3, the band 90 to 100 unchanged.

**What a plateau is in a photo.** On four stress photos the area blown at default and
flat at −100 holds 90 to 97% of the cells whose four raw samples are all at the top,
and 97.5% or more of it has a clipped sample. The park photo's sky has one channel
clipped on 9% of the frame and all three nowhere: it is no plateau, and Camera Raw
leaves it bright. A test build measures this for the rule from the photo at full
size, per cell of the map: the share of pixels with any channel at the white level,
with all three there, and the share that is exactly flat. On the palms photo it
agrees with the raw file's own counts (0.243 against 0.247 and 0.129 against 0.130).

**The top retrained with plateau inputs.** The training set: 468 photos, their maps
made again with the clamp, and 180 scenes built as photos are (noise of 1.5% a pixel
on all but the plateaus; ramps into the clip, plateaus, textured bands, plateau and
texture side by side), without the mosaics. The inputs added: the plateau share and
the clipped share near a level, above it and over the frame; the upper percentiles of
the textured cells (each counted by its share that is not plateau) and their mass
around the level. Errors were taken as rendered L\* per photo (the engine's white
curve on level plus predicted gain, against Camera Raw's result), four folds held out:

- The top is poorly predicted per photo whatever the inputs. Near white (over 0.6 of
  the white at default, not blown) the bias is 5.8 to 6.4 L\* RMS over photos: 7 to 8
  where under 1% of the frame is bright (small lights in dark scenes), 3.3 to 4.9
  between 1 and 15%. Within a photo the samples scatter by 4.7 L\*.
- Main's own rule on the 215 photos it never saw: 4.7 to 6.4 near white, and 11 to 14
  in blown areas where under 15% of the frame is bright (medians to −6.8), against 5
  to 10 for a retrained top. A retrained top is better in blown areas; near white the
  per-photo error stays.
- The held-out error does not follow the bright area's fine detail, plateau share or
  clipped share (correlations up to 0.29).
- Two training photos with a large plateau go opposite ways, as the probes say they
  should: a white dress under a blown sky with bright leaves around it, where Camera
  Raw keeps the sky near white, and a valley under a blank sky over dark rock, where
  it takes the sky to dark grey. Held out, a top without the textured cells'
  percentiles has the first 22 L\* too dark and the second 27 too bright; with them
  the first is within 2 and the second 20 too dark.
- The hand-over from the first ensemble matters as much as the fit. The level that
  renders 0.6 of the white at default, where the first ensemble's training ends, is
  −0.74 to −0.39 stops on the training photos; the hand-over at −2 to −1 gave the
  retrained curve half a stop of the first ensemble's own range on a bright photo.
  That was the palms' regression in every fit above.

With the hand-over at −1 to −0.4 and the clamp, the stress photos' blown areas (mean
L\*, main / test build / Camera Raw): park 83.3 / 89.2 / 92.6, palms 90.7 / 92.7 /
95.5, paved square 94.9 / 96.1 / 96.5, storm clouds 96.6 / 96.4 / 96.5, wall 96.8 /
97.3 / 97.0, and the palms' worst region is 1.1 L\* darker than main's (6.6 before).
Other seeds of the networks give 88.8, 90.6, 96.0, 96.8, 97.5 and 2.2. On the gate's
highlights sets at −100: highlights ΔE00 1.90 → 1.58, nine photos closer by 0.3 and
one further, the band 90 to 100 from −2.46 to −1.56 L\* (80 to 90: +0.43 → +0.82), the
park 6.18 → 2.13, the palms 3.07 → 1.94. One pair fails: a sunset where Camera Raw
keeps the red channel at white on 12.9% of the frame and the test build ends at 0.978
there. At −50 the same curve at half strength is mixed (highlights 1.05 → 0.91, seven
closer and six further). Nothing in the engine changed.

**The slider is close to linear at the top.** Bright bands at Highlights 0, −25, −50,
−75 and −100, gains read through the engine's white curve of the file: a flat band
takes 0.20, 0.50 and 0.75 of −100's gain, a noisy band 0.24, 0.48 and 0.73, bands of
both 0.19 to 0.21, 0.42 to 0.45 and 0.67 to 0.75. That a small blown spot falls much
further between −50 and −100 than before −50 is the shoulder's doing, not the
slider's.

**Mixing the two curves.** On the training photos, the retrained top's held-out
predictions joined to the shipped first ensemble's curve above the hand-over (bias of
the render per photo in L\*, RMS over photos; near white and in blown areas): the
first ensemble 6.8 and 12.0, the retrained top 5.9 and 8.8. Their errors are partly
independent. Taking the top only where it pulls less left 6.6 and 10.3 with a bright
median bias of 1.3 to 2.0, and on the gate failed a photo of small lights twice (a
window at 49.7 against main's 46.1 and Camera Raw's 40.0); capping it where a level's
pixels are partly clipped left 6.4 and 9.9. A share of the way from the first to the
top: 0.4 leaves 6.0 and 9.6, 0.5 5.9 and 9.2, 0.6 5.9 and 9.0, 0.7 5.8 and 8.8, and on
the 24 photos with 15% of the frame and more bright 2.6 and 2.7, 2.6 and 3.2, 2.9 and
3.9, 3.4 and 4.8 against the first's 5.6 and 5.8 and the top's 5.3 and 7.5. Once
mixed, the hand-over level matters little (near white 5.7 to 6.3 from −2..−1 to
0..0.5). A share of 0.6 from −1 to −0.4 on the gate's highlights sets: 90 pass, the
sunset fails again.

**The flags can come from the measurement copy.** A cell of the 512 px copy counted
as plateau or clipped when 90% of its pixels are, with no test for flatness: the mix
at 0.6 leaves 5.8 and 8.7, as the shares measured at full size do (5.9 and 9.0). The
engine therefore reads them off the copy it already has.

**The engine build (not merged).** Per cell of the measurement copy, whether a channel
is at 0.98 of its white and whether all three are; a third ensemble of eight networks
with the 13 inputs above, fitted by the repository's script on 468 photos and 170
synthetic skies; the curve moved a share of the way to it, rising from none at −1
stop to 0.6 at −0.4; a rendered file keeps the first ensemble's curve. The
cross-validation with the engine's own arithmetic (a fold's fit is one network, never
saw the photo nor a sky built on it; the first ensemble is the shipped one): first
6.8 and 11.9, third 5.8 and 8.6, joined at 0.6 5.9 and 8.8; on the 28 photos with 15%
of the frame and more bright 5.5 and 5.6, 3.4 and 5.6, and 2.8 and 3.0. Over all the
photos the join is no better than the third alone; it is on the photos with a large
bright area. Stress photos' blown areas (mean L\*, main / build / Camera Raw): park
83.3 / 86.3 / 92.6, paved square 94.9 / 95.5 / 96.5, wall 96.8 / 97.3 / 97.0, storm
clouds 96.6 / 96.5 / 96.5, palms 90.7 / 90.4 / 95.5; small lights 74.3 / 74.8 / 83.7
and 55.8 / 54.2 / 49.5; the sky through branches 87.1 / 90.1 / 71.1. The gate's
highlights sets: 91 pass, none fails, 35 have too few photos. At −100 the highlights
go 1.90 → 1.62 ΔE00 with seven photos closer by 0.3 and none further (park 6.18 →
4.04, palms 3.07 → 3.32), the band 90 to 100 from −2.46 to −1.95 L\*, 80 to 90 from
+0.43 to +0.74; at −50 1.05 → 0.91 with six closer and three further; at −25 no
change at the median. This fit gains nothing on the palms, where the test builds
before it gained 0.5 to 2 L\*: the networks' seeds move that photo by 2.

**With Shadows: the blend read the wrong extreme.** The gate's pairs sets (33 photos,
84 pairs of a setting and a stratum) with that build, against the same build without
the third ensemble: five fail. At Shadows −100 with Highlights −100 three skies come
out 3 to 4 L\* lighter in a window where Camera Raw is darker, and a dark photo clips
half a point more to black; at one mixed setting the palms keep 0.7 point less at
white, and at Shadows +100 with Highlights −100 the bright photos' median is 0.1
worse. The blend takes each curve toward its gain at the 99th percentile of the
levels, and the third ensemble moves that gain by 0.29 to 0.32 stop on the three sky
photos (0.07 to 0.18 at the failing windows' own levels): the blend's uniform part
carried a share of it to every level, which is all that moved the dark photo, whose
window is four stops down. Refitting the fractions on the new curve does not help. On
213 training photos' renders of the pairs, same pixels, the error of the gain over
0.6 of white (stops, mean of per-photo RMS; Shadows −100 with Highlights −100, then
Shadows +100 with Highlights −100): the old curve 0.226 and 0.191; the new curve
through the shipped fractions 0.217 and 0.173; fractions refitted, photo held out,
0.218 and 0.172; the old blend with the top's change added at the pixel's level 0.211
and 0.171; the same with the change blended as the curve is 0.213 and 0.174. The last
is what the build now does: the blend reads Highlights −100's extreme and reach from
the first ensemble's curve, which its fractions were fitted on, and blends the joined
curve. The failing windows (ΔE00, without the third ensemble / before / now): 3.83 /
6.69 / 5.07, 4.23 / 6.47 / 4.98, 22.29 / 24.43 / 23.67, and the dark photo as without
it. The pairs sets now: 48 pass, one fails (the palms at Shadows +60, Highlights −60,
Whites +30, Blacks −30: 18.4% of the frame at white against 19.2% and Camera Raw's
22.5%), 35 have too few photos. The highlights of five of the 33 photos come closer
at the pairs with negative Highlights (one blown window 16.4 → 10.4 and 19.5 → 13.2
ΔE00).

**What the pairs show of the blend itself.** One of those skies, with Highlights −100
alone: main 74.4, the build 77.5, Camera Raw 77.7. With Shadows −100 added Camera Raw
takes it to 69.7; main renders 74.2 and the build 76.0. The blend leaves a bright sky
4.5 L\* too light there whatever the single curve does, and main's too-dark single
curve hid part of it. On the training photos the shipped blend's bias in the top stop
at Shadows −100 with Highlights −100 is 0.03 to 0.04 stop at the median, and 41% of
the photos are off by over 0.15.

**The decoder change alone on the pairs.** Against main: 47 pass, two fail, 35 have
too few photos. Closer with it: one photo at every pair (median ΔE00 2.77 → 2.10 and
3.47 → 2.77), another at Shadows +100 with Highlights +100 (5.75 → 3.16, clipping
7.1% → 3.8%), two more by 0.1 to 0.4. The two that fail: that second photo at
Shadows +100 with Highlights −100, where a window of foliage with blown specks is 3.7
L\* darker (61.2 against main's 64.9 and Camera Raw's 66.5) while its median, its
shadows and its other windows improve and the specks lose main's magenta; and a night
photo whose deep shadows move 0.2 L\*, which takes 0.9% of the frame across the
gate's black threshold. On the first, holding the samples at the white level lowers
the photo's measured top by 0.47 stop, and the first ensemble's Shadows +100 then
lifts that window 0.14 stop less: the first and second ensembles were fitted on maps
that kept the data above the white level.

**Smooth synthetic scenes go the other way.** The app's own colour tests, which the
build had not been run through until then, failed on two synthetic cases at Highlights
−100. On a smooth scene with nothing clipped (the stage probe `scene1`) the third
ensemble pulled the bright tones, 0.4 to 0.86 of white at default, less where Camera
Raw pulls them more: 23 of 384 blocks moved and four came closer, and the largest
block error went from 0.055 to 0.067, past the probe's limit. On the synthetic chart,
whose brightest patches are clipped and flat, the three brightest greys and the three
brightest steps of the lightness sweep came out 0.8 to 2.1 L\* lighter (the grey at +3
EV 90.8 against 89.0, with Camera Raw at 68.6): Camera Raw pulls the chart's flat
bright patches some 20 L\* further than the stage does either way, and the case moved
from 10.85 to 10.87 ΔE00 at the mean. Both are the regime of the probes above: smooth
content of that size is flat to Camera Raw, the mosaics taught that to the first
ensemble, and the third was fitted on photos and noisy skies alone.

**Only where the photo has clipped cells.** Held out and kept per photo, the join's
gain depends on what the photo has clipped. With no clipped cell (194 photos, 63 with
pixels near white) the first ensemble is off by 5.7 L\* near white (median −2.6), the
third by 5.4 (−0.5) and the join by 5.5 (−2.2). With clipped cells on up to 0.1% of
the map: 8.1, 6.8 and 7.1; 0.1 to 0.5%: 9.0, 7.9 and 8.1 (blown 14.9, 11.2 and 12.0);
0.5 to 2%: 6.6, 5.8 and 5.9 (10.5, 7.1 and 7.9); 2 to 10%: 4.8, 3.7 and 3.6 (11.2,
6.3 and 6.2); over 10%: 5.5, 3.5 and 2.6 (7.9, 6.2 and 4.6). The build now gives a
photo the third ensemble's share by a weight that rises smoothly from none with no
clipped cell to all at 0.02% of the map's cells: over all 468 photos the join is then
off by 5.95 near white against 5.86, and by the same as before in blown areas (8.76)
and on the photos with a large bright area (2.75 and 3.00). The probe scene renders as
it did, the chart's case as above (its snapshot renewed, no Camera Raw reference or
accepted distance changed), and the gate's sets as before: the highlights sets 91
pass and none fails, with five photos closer at −50 where six were and two at −25
where five were; the pairs sets one failing pair against the same build without the
third ensemble.

**The decoder change alone on every validation set.** 83 sets, 1944 pairs of a setting
and a stratum against main: 1236 pass, 11 fail, 697 have too few photos. No set of
Highlights alone fails. What fails is on photos whose measured top the change lowers,
through the controls that read it:

- A sunset at Shadows +100, in two sets: median ΔE00 6.4 → 8.1 and 6.6 → 8.4, the
  worst window 2.2 worse. The same photo is closer at other settings (highlights at
  one mixed setting 5.6 → 2.4, at Exposure +1 3.0 → 0.5).
- The photo with blown specks in foliage: Shadows +100 in three sets (a window 2.0 to
  2.1 worse while its median goes 1.26 → 1.06 and its shadows 5.4 → 4.0), with
  Highlights −100 as above, and Dehaze −100 and −50, where one window is 7.1 and 2.3
  worse (the regional 90th percentile 13.0 → 17.4 and 5.3 → 6.4).
- Pixels counted at white or black on four photos: the palms at Exposure −1, where
  Camera Raw has 15.7% of the frame at white, main 15.5% and the change 2.8% (the sky
  renders at L\* 99.5 against main's 99.7 and Camera Raw's 100.0, either side of the
  gate's line); the night photo above; and two photos at Vibrance −100 and −50, 0.8
  and 0.9 point.

So the clamp is no change to ship as it stands: the first two ensembles, Dehaze and
the rest read a photo's top from maps that kept the data above the white level, and
lowering it by up to half a stop moves them all on the photos that have such data.
Two ways are left. One is to keep the decoders as they are and give the clamped
reading to the third ensemble alone (its statistics from the measurement copy with
each channel held at its white), which also keeps the highlight data the clamp throws
away. The other is to refit the first two ensembles and the blend on maps read as
Camera Raw reads them. Neither is done.

**Open.** The statistic behind the size effects; whether to keep the data above the
white level for rendering while the rule reads the photo as Camera Raw does, and
refitting the first and second ensembles on maps read as Camera Raw reads them; the
blend's error on bright levels at Shadows −100 with Highlights −100; the palms' sky
and small lights in dark scenes, which no fit so far gets right; the detail inside
blown areas (the shoulder); the engine's other paths (previews, the GPU, masks) and
the gate's other sets; whether the same shoulder matters for Whites and positive
Highlights; and what tells Camera Raw's flat regime from a photo, which the third
ensemble has no input for (it leaves photos with nothing clipped to the first, at the
cost of the bias the first has near white on them).

### Clarity on photos: a curve per photo that no histogram sets (2026-11-20)

A second round on Clarity +100, on photos instead of probes. Camera Raw's Clarity +100
render through a linear profile was added for 222 more training photos (322 in all; 278
line up with the engine's measurement map and are the set below). Scores are the median
over a photo's map cells of |ΔL*| between Camera Raw's default with a predicted log2
change and Camera Raw's Clarity render; fits are cross-validated in four folds by
photo. Nothing was fitted on validation and holdout was not touched. The engine's
Clarity scores 2.13 (median photo; mean 2.36, 90th percentile 3.67) and doing nothing
2.87.

**Most of the gap is a tone curve of the base level that differs by photo.** Adding to
the engine's field the mean of what it leaves at each base level (an oracle, per photo)
gives 1.29, with 237 photos closer by 0.2 and none further. That curve alone, without
the engine's field, gives 1.58. In the darkest levels of daylight photos the engine's
field sits 1 to 2 EV under Camera Raw's, and its brightening peaks one to two stops
above where Camera Raw's does, which is on the bulk of the histogram.

**The curve is not a rule of the histogram.** Mosaics of flat, faintly textured patches
that carry a training photo's histogram of base levels (28 photos, two mosaics each, as
the Shadows/Highlights rule's `photo` family) get a steep, nearly linear change by
level, 0.33 to 0.5 EV per EV, that follows the engine's curve on the photo and not
Camera Raw's. The same levels get far less as coherent regions of a photo than as
scattered patches. Mosaics therefore cannot train a Clarity rule, and the engine's
detail weights, fitted on synthetic scenes, carry the mosaics' behaviour onto photos.
On the size series of 2026-11-19 the change by level of a daylight photo is the same
from full size to an eighth of it, so the reach scales with the frame; only the darkest
levels of noisy photos change with size.

**Three things it is not.** Clarity is a luminance gain: the three channels' gains equal
the luminance gain within 0.012, 0.005 and 0.04 EV RMS (R, G, B; field 0.49 EV), and what
the engine leaves has no tie to colour. The rectification a pixel-level slope would add
per cell, computed from the engine's full-size default (log2 E[Y^p] − p log2 E[Y]),
correlates 0.16 with what the engine leaves and adds nothing (15 photos). A bound on
the engine's brightening makes it worse (2.14 to 2.22).

**Camera Raw's response to detail turns back, but its strength differs by photo.**
Camera Raw's and the engine's change against a cell's detail (level minus a Gaussian
blur of 8 map pixels), relative to cells without detail, lower half of the levels,
median photo:

| Detail (EV) | −3.5 | −2.5 | −1.75 | −1.25 | −0.8 | −0.45 | +0.45 | +0.8 | +1.25 | +1.75 | +2.5 | +3.5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Camera Raw | +0.01 | −0.27 | −0.38 | −0.46 | −0.43 | −0.31 | +0.33 | +0.46 | +0.53 | +0.48 | +0.35 | +0.01 |
| Engine | −1.10 | −1.45 | −1.26 | −1.06 | −0.77 | −0.47 | +0.32 | +0.49 | +0.69 | +0.81 | +0.95 | +0.88 |

The same holds at 2 and 32 map pixels; in the upper half of the levels Camera Raw's
brightening peaks at +0.14 EV. The engine's linear response is what darkens a whole
dark object (a giraffe against the sky, hair, a stand of trees) where Camera Raw keeps
its mean. But the engine's own form refitted on the photos with a response that turns
back (d · exp(−(d/c)²/2), c 1 or 1.4 EV, also with separate weights for positive and
negative detail, least squares and a robust fit) scores 2.13 to 2.25: as many photos
further as closer. Per photo the response's slope for small detail runs from 0.23 to 1.2
(10th to 90th percentile; the engine's 0.37 to 1.39, correlation 0.63), and common
weights do not carry that. The extra slope under the map, read off 59 photos, is 0.75 to
0.98 below and 0.4 to 0.45 above whichever reference of the photo is tried (linear mean,
mean log, median, top, or the absolute level); none of them makes the photos' curves
collapse.

**Two changes that help.** Bounding the engine's darkening (−c · tanh(−gain / c), no
fitted weights) gives 2.01 at c = 1 EV (55 photos closer by 0.2, 5 further, worst +0.41)
and 1.99 at 0.7 EV (69 and 11); in the cells Camera Raw renders under L* 25 the mean
error falls from 2.24 to 2.00. On top of that, boosted regression trees from the
Shadows/Highlights rule's 28 descriptors of a base level to what the bounded field
leaves there (100 trees of depth 4):

| Share of the trees' curve | Median | Mean | 90th percentile | Closer / further by 0.2 | by 0.5 | Worst |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 1.66 | 1.83 | 2.69 | 170 / 36 | 107 / 16 | +2.20 |
| 0.75 | 1.68 | 1.83 | 2.81 | 171 / 18 | 105 / 8 | +0.99 |
| 0.5 | 1.71 | 1.91 | 2.85 | 164 / 12 | 79 / 2 | +0.53 |

(closer and further than the bounded field alone). The whole curve is right in shape and
off by 0.2 to 0.3 EV on the bright bulk of the photos that get worse, which costs 2 to
3 L* there; half of it keeps most of the gain. More photos did not lower the share that
gets worse (108, 223 and 278 photos: 16 to 17% by 0.2 with the whole curve). The
Shadows/Highlights rule's network on the same inputs, without mosaics, extrapolates
(55 L* on a photo of the Moon on black) and trees cannot. With the engine's own curve or
the mean detail by level as further inputs the result is the same. By band of Camera
Raw's L*, bounded field plus half the curve against the engine (mean |ΔL*|, median
photo, and photos closer / further by 0.3): 0 to 25: 2.24 → 1.79 (165 / 10); 25 to 50:
3.28 → 2.96 (158 / 17); 50 to 75: 4.31 → 3.87 (135 / 33); 75 to 100: 3.62 → 3.43
(72 / 45). The photos that get worse look like the engine's render with more lift in
dark areas; the curve is smooth in the base level and adds no local structure.

The bound and half the trees' curve were the candidate; the numbers above are map-level
simulations on training photos.

**Outcome: built, tried on validation, and not kept.** The candidate was implemented in
the engine (the bound in `clarity.rs`, 100 trees of depth 4 evaluated with the global
rule's statistics, an HDR master left as it was) and the result does not justify it.

- On flat synthetic frames the photo-fitted parts are wrong: with them a flat frame
  changed, the dark ground beside bright rails got brighter where Camera Raw makes it
  darker, and the chart's Clarity +100 moved from 3.98 to 4.06–4.09 mean ΔE00 from
  Camera Raw. They had to be switched off by the share of flat cells in the map (under
  0.08 for 99% of 470 training photos, 0.47 for the chart, over 0.9 for flat frames,
  rails and ramps). That is a detector of synthetic frames, not a model of what Camera
  Raw does.
- On the 127 training photos that could be rendered again the build scored 2.15 → 1.75
  (in-sample for the trees).
- On validation, the dataset gate's first six sets (26 photos with Clarity cases, the
  full render's median ΔE00 against Camera Raw; the run was stopped there):

  | | main | candidate | Mean | Closer / further by 0.2 | by 0.5 |
  | --- | --- | --- | --- | --- | --- |
  | Clarity +100 | 2.47 | 2.21 | 2.50 → 2.30 | 11 / 3 | 5 / 2 |
  | Clarity +50 | 1.47 | 1.45 | 1.71 → 1.61 | 7 / 3 | 1 / 0 |
  | Clarity +25 | 0.96 | 1.00 | 1.34 → 1.28 | 2 / 0 | 0 / 0 |
  | Dehaze +40 with Clarity +40 | 2.28 | 2.22 | 2.60 → 2.54 | 4 / 0 | 0 / 0 |

  A tenth off the median at +100, nothing at +50 and +25, and three photos 0.45 to 0.64
  further. On those three Camera Raw darkens a whole area (the sky behind trees, a night
  street) and the candidate lifts the dark levels instead: a curve of the base level
  moves every area at one level together and cannot follow that.
- The cross-validated gain on training photos (2.13 → 1.74 at the map level) was mostly
  the shadow band and the average lift of dark levels; little of it is left in the full
  render, where the median error is set by the mid-tones.

What this leaves: the oracle shows the missing part is a curve per photo, but it is not
predictable from the level's descriptors well enough to matter (half the predicted
curve was the most that did not make a tenth of the photos worse), and what Camera Raw
does on the photos that got worse is regional, not a function of level. A next attempt
should not be another curve of the base level or another refit of the present form. The
open questions are what sets the strength of the response per photo (0.23 to 1.2 for
small detail), and why coherent regions respond so much less than scattered patches of
the same levels.

### Clarity: a slope around a base, and no closed form for either yet (2026-11-19)

Clarity +100 is the third largest gap of the tone controls (1.94 own ΔE00 on validation)
and its field is part of what Dehaze and Highlights leave ([above](#one-field-per-slider-and-dehaze-as-a-veil-2026-11-19)).
This round measured Camera Raw 18.7's Clarity by itself before choosing an operator:
about 420 probe renders (linear probe DNGs, one pattern per frame, rendered at default
and with Clarity, the engine on the same files as the control), three photos at full
size, 59 training photos at the gate's render size, and the 100 training photos'
Clarity ±100 renders through a linear profile on the map grid. Nothing
was fitted on validation and holdout was not touched. No engine change came out of it;
what follows is what a later attempt can start from.

**The step reading of 2026-11-12 is withdrawn.** That probe stacked four steps in one
frame, and the "overshoot of 0.63, 0.45, 0.14 and 0 of a 0.05, 0.2, 1 and 2 EV step" was
the neighbouring bands leaking into each other. On a frame of its own (1536 × 1024, flat
sides) a step of 0.025 to 0.4 EV changes by 1.8% of itself at Clarity +100 and has no
halo; steps of 0.7 to 4 EV get only an overshoot one to four pixels wide (0.46 to 0.71
of the step beside a 0.7 to 2 EV edge, 0.19 at 4 EV). Camera Raw's Clarity is not a
compressive gain on small differences.

**Flat synthetic frames are degenerate for positive Clarity.** The same 0.2 EV step on
a 768 px frame grows to 0.6 EV with the dark side lowered as a whole; on 1536 and 3072 px
frames nothing happens. A dark bar of 8 px drops 0.44 EV and bright bars leave the bar
alone and lower the whole background by 0.02 to 0.035 EV. Whole flat regions move and
the brightest one stays. Probes for positive Clarity need texture.

**Positive Clarity is a slope, and context sets it.** A test texture (one octave of
noise at 4 px, 0.1 EV RMS) gets a gain of 1.02 by itself. Beside a step, on either
side and at every distance from it (768 px, and 1536 px on a 3072 px frame), the same
texture's log contrast is multiplied by:

| Step between the two halves (EV) | 0.1 | 0.25 | 0.5 | 1 | 2 | 4 |
| --- | --- | --- | --- | --- | --- | --- |
| Dark half | 1.01 | 1.02 | 1.17 | 2.12 | 2.59 | 2.74 |
| Bright half | 1.02 | 1.02 | 1.10 | 1.64 | 1.86 | 1.6 |

The halves' means hardly move (−0.15 EV and +0.02 EV at 2 EV). With three and five
levels the gain follows the level and not the place (levels −2, 0 and +2 EV: 2.53,
2.31, 1.38; the same levels in another order: 2.50, 2.37, 1.60), and the brightest
level always has the least. Along a ramp the gain has two plateaus that meet at the
frame's middle level (8 EV across the frame: 2.35 below, 1.83 above, falling to 1.05 at
white). Over coarse noise of 64 px the texture gets 1.24, 1.80, 2.11, 2.03 and 2.07 at
0.1, 0.2, 0.4, 0.8 and 1.6 EV RMS of the coarse part, while the coarse part itself gets
1.12, 1.33, 1.57, 1.68 and 1.48. One octave at 32 px alone gets 1.03 at 0.1 EV RMS,
1.12 at 0.15, 1.47 at 0.2 and 1.5 to 1.6 from 0.3 to 1.2. So at +100 Camera Raw
multiplies the log contrast of all detail at a place by one slope, up to about 2.3 to
2.7, less near the frame's top levels; a frame without about a stop of range gets
nothing; coarser structure gets less than the detail on it (1.3 to 1.7 from 8 to
256 px), and regions of several hundred pixels move apart a little (a band 1 to
2 EV under its neighbours: −0.44 to −0.58 EV). The engine gives the texture 1.02 to 1.07 beside
steps and on ramps and 1.10 to 1.24 over coarse noise, and coarse noise itself 1.2 to
1.6 at every amplitude: it is linear, and it has no slope under its map.

**The law on photos: gain = amount × slope × (level − base).** On five training photos
rendered at 2048 px, the gain of every map cell (4 px) at Clarity +50 and +100 was split
into a slope (the regression of the log gain on the log level inside 12 × 12 px) and a
base (the level that the cell's gain leaves unchanged). The slope at +50 is 0.50 to 0.53
of the slope at +100 with a cell-wise correlation of 0.96 to 1.00, and the base is the
same at both amounts (correlation 0.99; 0.05 to 0.08 EV RMS apart on four photos, 0.41
on a night photo). So Camera Raw's
positive Clarity is linear in the slider, and a photo has one slope field and one base
field. On the raw file at full size the slope is there down to the pixel: 2.19, 1.52
and 1.99 in 7 × 7 px windows of three files (1.94, 1.43 and 1.78 after reducing eight
times). A probe DNG made of the file's own default render answers like the file (field
correlation 0.92 to 0.98 at full size), so probes built from photos are valid, and
the slope hardly depends on the frame's size (2.31, 2.29, 2.23 and 2.11 from full size
to one eighth) while the large-scale part of the field does (correlation with the
file's 0.98 falling to 0.80).

- *The base* is a much stronger edge-preserving smoothing than any tried: regions come
  out nearly flat (a van, a road, a facade each one level), thin structures such as
  branches disappear into their surroundings, and edges between regions stay sharp. With
  Camera Raw's own slope, the best of Gaussian, bilateral (spatial 8 to 64 map px, range
  0.35 to 2 EV), guided (radius 8 to 96, ε 0.05 to 6), iterated guided, median, local
  Laplacian smoothing (threshold 0.7 to 2 EV) and an edge-aware upsampling of a coarse
  copy (cells of 4 to 32 map px) leave 51 to 68% of the field's RMS, and a mix of six of
  them fitted on the other photos 53 to 67%.
- *The slope* is 0.6 to 1.3 at +100 where it can be measured. On daylight photos it is
  1.1 to 1.2 below the photo's middle level and 0.5 to 0.7 above; on a dusk and a night
  photo it is the other way round (0.3 to 0.5 in the dark, noisy areas, 0.9 to 1.0 in
  the lit ones). Halving a photo's log contrast nearly halves it on three of four
  photos; doubling the contrast raises it by 8 to 108%.
- *Dark photos*: what is lit is lifted by 1 to 1.6 EV and the darkest levels stay, with
  the automatic black and without it (DefaultBlackRender None): the base lies far under
  the lit levels there.

**What a gain map can carry.** With Camera Raw's own cell gains on the engine's map grid
(the best any gain map could do), upsampled as the engine upsamples, the median |ΔL\*|
after the gate's reduction to 1024 px is 0.53, 0.03, 0.51, 1.01 and 1.61 on the five
photos (doing nothing: 3.7, 1.2, 2.8, 3.1, 10.8); with the cell's slope as well it is
0.08, 0.03, 0.16, 0.16 and 0.93. The engine's own field in the same place leaves 2.84,
1.27, 2.50, 2.35 and 9.70. So the slope under the map is worth up to 0.85 L\* on
textured photos, and the map-level field is the larger fault: on the 100 training
photos the engine's Clarity +100 leaves 86% of Camera Raw's field (median photo;
correlation 0.72) and a median |ΔL\*| of 2.05 where doing nothing leaves 2.87.

**A slope under the engine's own map: tried in a test build, not kept.** Simulated on
Camera Raw's own 2048 px renders of 59 training photos, giving every cell of the
engine's field a slope of 0.7 (the pixel's gain is the cell's plus 0.7 times its
distance from the cell's level) moves the median photo's |ΔL\*| at 1024 px from 1.79 to
1.66 with 20 photos closer by 0.1 and none further. Rendered by a test build on the 100
training photos it does not hold: the median photo goes from 2.05 to 2.16 (mean 2.45 to
2.39), 33 photos closer by 0.1 and 15 further, ten of them by more than 0.3 and the
worst by 2.2. The slope on full-size pixels lifts the mean of whatever is textured or
noisy when the render is reduced (0.1 to 0.2 EV over whole daylight photos), and Camera
Raw's field has no such lift there: its mean is −0.05 to −0.28 EV on those photos. On
night photos the same lift is what was missing (four of them 1.0 to 2.4 L\* closer).

**In dark, noisy areas the lift is made by the pixels.** On two night photos at full
size, 8 × 8 px blocks whose default level is 6 to 11 stops under white hold mostly
noise, with a fifth to four fifths of their pixels at black in the 16-bit render. At
Clarity +100 the pixels' mean log gain there is +0.1 to +1.9 EV and the gain of the
block's mean is +1.6 to +5.1 EV: a slope on every pixel, followed by a reduction, raises the mean. The reduced
renders show it as a lift of 2 to 4 EV over a third of such a photo. On two daylight
photos the same difference is 0.05 to 0.1 EV. A map cannot carry this part, and an
engine that reproduces it has to match Camera Raw's noise near black as well.

**What the engine's field lacks most is a tone curve per photo.** On top of the
engine's Clarity +100, a curve of the level fitted to each photo's own remainder (an
oracle) takes the median photo's |ΔL\*| from 2.05 to 1.14 (mean 2.45 to 1.35, 90th
percentile 3.83 to 2.17; 85 photos closer by 0.2, none further); one offset per photo
1.84. On daylight photos the engine darkens the darkest levels two to four times as
much as Camera Raw (a photo's levels 3.8, 2.3 and 1.3 stops under its median: −1.83,
−1.01 and −0.35 EV against −0.36, −0.30 and −0.02), and on night photos it lacks the
lift above. The curve is not yet predictable: boosted trees on 30 descriptors of a
level's place in the histogram (those of the Shadows and Highlights rule), four folds
over the 100 photos, reach 1.85 with 50 photos closer by 0.2, 21 further and the worst
6.5 further; a pooled correction of the engine's field by level (a factor and an
offset by the level about the median or under the top) gains nothing (2.02 to 2.12,
16 to 51 photos closer and 20 to 36 further).

**Forms tested against the field on the 100 training photos** (map grid, the input
Camera Raw's own default render, four folds by photo, median photo's share of the field
left and median |ΔL\*|; the engine as rendered: 0.86 and 2.05):

| Form | Field left | Median \|ΔL\*\| |
| --- | --- | --- |
| Five Gaussian scales | 0.89 | 2.64 |
| Five bilateral scales (range 1.4 EV) | 0.77 | 2.39 |
| Local Laplacian detail, threshold 1 EV, three groups of levels | 0.78 | 2.34 |
| The same at three thresholds | 0.77 | 2.24 |
| Bilateral scales with weights by level under the top (the engine's form, refitted) | 0.69 | 1.87 |
| The same by level about the mean, with a curve of the level | 0.71 | 1.99 |
| All of the above together (about 120 weights) | 0.62 | 1.68 |

A local Laplacian filter with a free remap (twelve knots from −4 to +4 EV for each of
four groups of pyramid levels) was also fitted where the input is known exactly, on
photo probes: fitted on a probe itself it leaves 20 to 37% of the field, fitted on another
probe 48 to 102%, and on the same photo at half contrast 117 to 168%. Camera Raw's
Clarity is not a local Laplacian filter with one remap, and not a filter of the map's
log luminance of any of these kinds; the best of them would take about a fifth off the
engine's error at the map level, which is what the refit of 2026-10-20 found and did
not see in renders.

**Negative Clarity** is cleaner on flat probes and no simpler on photos. A flat step
loses 0.69 of itself at the edge at −100 (0.41 at −50), 0.51 at 8 px, 0.33 at 16, 0.09
at 32 and nothing at 64 px of a 1536 px frame, the same share from 0.025 to 1 EV and
less above (0.63 at 2 EV, 0.31 at 4), as a blend toward a blur in linear light does (the
dark side +0.40 EV, the bright side −0.28 EV at 1 EV), and a side never gains more than
one stop. Noise of 0.2 EV RMS keeps 0.43 to 0.45 of its contrast up to 32 px, 0.61 at 64
and 0.85 at 128. The field is the same at every frame size (a probe at one eighth of the
size: correlation 0.97 with the file's), so its width is a share of the frame. But that
blend, applied to the training photos, is two to three times Camera Raw's field (in
windows of a few pixels the slope is 0.75 to 1.0, and many regions are not touched): the
strength follows the place there too. The engine's negative Clarity leaves 76% of
Camera Raw's field at −100; its own error on validation is small (0.32).

Open: what sets the slope (the frame's range, the level about the photo's middle, the
top, noise), what the base is, and the curve per photo, which is the largest part and
the one a rule might learn from more photos (100 have Clarity renders; the Shadows and
Highlights rule needed 283). Camera Raw's own cell gains, levels and slopes of 59
training photos at the gate's render size are on disk for that. A refit of the present
form is not worth another try: this round and the one of 2026-10-20 agree on about a
tenth to a fifth at the map level and nothing in renders.

### One field per slider, and Dehaze as a veil (2026-11-19)

After the refit on more photos was rejected, the errors left at full sliders were taken
apart instead of fitted again, the largest first, and controls were compared with each
other. Camera Raw 18.7 through a linear profile, on the measurement map's grid (512 px)
unless said; training photos only, except where a validation gate is named; holdout
untouched.

**The default black's key under every slider: a wash.** A test build keyed the default
black by the whole local gain on the map's darkest 0.1%, for Shadows and Highlights of
either sign with their blend. On 51 training photos Shadows +100 moved from 0.0116 to
0.0114 (RMS of cube-root differences; 22 photos closer, 11 further) and the lift's bias
six to ten stops under the top halved (+0.27 / +0.46 stop to +0.14 / +0.24). On
validation (17 sets) nothing moved: Shadows +100 1.32 → 1.30 own ΔE00 (19 photos closer,
16 further), Shadows +100 with Highlights +100 3.01 → 2.86 (10 / 6), Shadows +100 with
Highlights −100 1.13 → 1.17 (6 / 9); over the changed cases 82 photos closer and 79
further. It removes the dark bands' bias (+2.81 / +1.16 / +0.82 L\* in the bands 0 to 30
at Shadows +100 become +1.26 / +0.26 / −0.01) and the photos' medians stay. Not changed.

**#1153's blend on cases it was not judged on.** The pairs sets added with #1175 hold
six combined cases; #1153 had been judged on one, at half the sliders. The plain sum of
the two curves against the blend, on those sets (33 photos, own ΔE00, median):

| Case | Sum | Blend (main) | Photos closer / further |
| --- | --- | --- | --- |
| Shadows +100, Highlights −100 | 1.52 | 1.13 | 31 / 1 |
| Shadows +60, Highlights −60, Whites +30, Blacks −30 | 1.19 | 0.90 | 30 / 1 |
| Shadows +80, Highlights −30 | 1.08 | 0.91 | 28 / 3 |
| Shadows −100, Highlights +100 | 0.96 | 0.85 | 23 / 6 |
| Shadows −100, Highlights −100 | 2.06 | 1.36 | 21 / 12 |
| Shadows +100, Highlights +100 | 2.85 | 3.01 | 19 / 13 |

The blend holds (152 closer, 36 further). At both sliders up it is better in the mean
(5.69 → 3.72) and the 90th percentile (15.0 → 7.1) and worse at the median: the sum is
2.6 to 4.7 L\* too dark in the bands 20 to 90 and the blend 2.4 to 4.2 L\* too bright
in the bands 0 to 50. That overshoot is open.

**Each slider follows one spatial field per photo.** What a photo's own best curve of
the base level leaves of Camera Raw's gain (its residual, a map) was compared between
settings on 373 photos. Shadows +100's and Shadows −100's residuals correlate −0.94
over a photo's pixels at the median (quartiles −0.96 and −0.88), −0.97 after a 7-pixel
blur; Highlights −100's and +100's −0.95 and −0.98. Shadows' and Highlights' residuals
correlate 0.08 to 0.18 in size. So the residual is not noise. Camera Raw's Shadows moves
a photo by one field, which the slider's sign flips, and Highlights by another, and a
curve of the engine's guided base approximates each; 56 to 62% of the residual's energy
is left after the blur.

The field is not a simple filter of the map's log luminance. On 124 photos the shipped
base (a guided filter, radius 16, ε 0.5) leaves 0.1138 / 0.0700 / 0.1108 / 0.0710 at
Shadows +100, Highlights −100, Shadows −100 and Highlights +100; guided filters of
radius 4 to 32 and ε 0.1 to 2 leave as much or more (0.1148 at radius 8, 0.1162 at 32),
Gaussians of 1 to 8 pixels 0.134 to 0.160, the pixel itself 0.1405, and a guided filter
of a closed or an opened luminance (small dark or bright features removed first) 0.1146
and 0.1154 at 3 pixels and more beyond.

A third of the residual's variance is a function of a pixel's surroundings that carries
over to unseen photos. A base correction that explains both signs' residuals, predicted
from 26 local features of the map (Gaussians less the base, local extremes, textures,
gradients, the guided filter's weight, the level's place in the photo, the distance
from the centre), four folds by photo, 373 photos:

| | Shadows +100 | Shadows −100 | Highlights −100 | Highlights +100 |
| --- | --- | --- | --- | --- |
| The curve alone | 0.1076 | 0.1154 | 0.0751 | 0.0755 |
| With boosted trees | 0.0909 | 0.0948 | 0.0635 | 0.0624 |
| Photos closer / further | 327 / 11 | 333 / 11 | 317 / 11 | 321 / 7 |
| With a ridge regression | 0.1013 | 0.1091 | 0.0703 | 0.0712 |

A six-layer dilated convolution network on the map (65 pixels in view, one fold, 94
held-out photos) does the same and no more: 0.1136 → 0.0982, 0.1215 → 0.1046, 0.0787 →
0.0700, 0.0819 → 0.0700. In the whole held-out error, the curve's and the field's, that
is about 8% for Shadows and 6% for Highlights. A learned local correction is real and
small. Not built.

**Dehaze is one field too, and Camera Raw's Clarity field is part of it.** On 94 photos
with Dehaze −100, −50, +50, +100 and Clarity −100 and +100 beside the four Shadows and
Highlights renders, a curve of the base level leaves 17 to 20% of the gain's RMS for
Shadows and Highlights, 23% at Dehaze −100, 24% at −50, 41% at +50, 46% at +100, 67% at
Clarity +100 and 95% at Clarity −100. Dehaze's residual maps correlate +1.00 between
−100 and −50, +0.98 between +50 and +100, and −0.93 and −0.91 between −100 and the
positive amounts: one field, unrelated to the Shadows and Highlights fields (under 0.1).

Clarity +100's residual correlates +0.52 with positive Dehaze's, −0.53 with negative
Dehaze's, +0.44 and −0.40 with Shadows +100's and −100's, and +0.38 and −0.36 with
Highlights −100's and +100's. Taking Camera Raw's own Clarity +100 field times a weight
by tenth of the photo's levels, with one set of weights for all photos, explains 37% of
Dehaze −100's residual (weights −0.46 to −0.64 at every level; 93% of photos closer),
28% of Dehaze +100's, 28% of Highlights −100's (weights 0 in the dark half, then +0.16
to +0.45 in the top four tenths), 22% of Highlights +100's and 7% of each Shadows
setting's. The engine's own detail (the map less a 4-pixel Gaussian) in its place
explains 1% for Highlights and 7 to 10% for Dehaze. So the field Camera Raw's Clarity
draws is a third of what the Dehaze curve leaves at both signs and a quarter of what
the Highlights curve leaves. Clarity is the third largest gap itself, and a part of the
first, the second and Highlights.

A fast local Laplacian filter on the 512-pixel map, tried as that field on 30 photos,
reaches a correlation of 0.56 with Camera Raw's Clarity +100 field at one scale per
photo (R² 0.32; an unsharp mask 0.17) and R² 0.36 to 0.41 with a weight per pyramid
level. The engine's own Clarity +100, at full resolution, correlates 0.735 with Camera
Raw's (R² 0.17 as rendered, 0.54 with one scale per photo; the field's RMS is 0.567
against Camera Raw's 0.512). The shipped operator is closer in form than the prototype,
which lacks the scales under the map's pixel; the prototype says nothing against the
form.

**Dehaze in luminance is near what a curve per photo can give.** On 94 photos Camera
Raw's Dehaze −100 gain has an RMS of 1.431 stop; the engine's differs from it by 0.388
(mean 0.511, bias +0.07), the photo's own best curve of the base level leaves 0.343 and
one curve for all photos 0.497. At +100: 1.105; 0.528 (bias −0.09); 0.492; 0.642. A
rule for Dehaze's curve would gain little; what is left in luminance is the field.

**Negative Dehaze's error is more colour than lightness.** In Lab, on 25 photos, the
engine's Dehaze −100 is 7.35 ΔE76 from Camera Raw's at the median: 3.42 in lightness and
4.29 in colour. Camera Raw keeps 0.67 of the default render's chroma there and the
engine 0.86 (at −50: 0.87 and 0.95; at +100: 1.20 and 1.27).

Camera Raw's own renders (100 photos) say what it does to colour. With `d = rgb − Y`
the colour part of a pixel and `g` its luminance gain, the colour part's absolute size
after Dehaze over before is 0.98 to 1.00 at +50 in every quartile of the gain (the gain
runs from 0.65 to 0.92), 0.78 to 1.00 at +100, 0.99 to 1.24 at −50 and 0.77 to 0.88 at
−100: Dehaze adds or takes away light of one colour and leaves a pixel's own colour
nearly alone. Clarity +100 (0.97), Shadows +100 (0.99) and Highlights −100 (1.01)
scale the colour part with the luminance: they are gains on RGB, as the engine applies
them. The colour part of ΔE76 left by three models of Camera Raw's Dehaze, each given
Camera Raw's own luminance (median of the photos):

| Model | −100 | −50 | +50 | +100 |
| --- | --- | --- | --- | --- |
| A gain on RGB | 7.41 | 2.92 | 1.72 | 3.30 |
| `t · rgb` plus a neutral veil | 3.27 | 1.71 | 0.97 | 2.36 |
| `t · rgb` plus a veil of one colour per photo | 0.65 | 0.58 | 0.40 | 0.99 |

The veil's colour is the same at every amount (its −25, −50 and −75 fits correlate 0.96
to 0.99 with −100's over 45 photos), and `t`, the share of the pixel's own colour kept,
is 1.0 down to −75 and 0.9 at −100 when one value serves all photos. At the median the
colour is neutral; on half of the photos its blue is between 0.86 and 1.13 and its red
between 0.97 and 1.04 (linear ProPhoto, unit luminance).

**Camera Raw's rule for the veil's colour, by probes.** A neutral daylight training
photo (Camera Raw's default render through a linear profile, about 1000 pixels) written
as a linear DNG whose sensor holds the scene two stops down, so that a source of up to
four times the white is unclipped, with one patch of known colour, level and size
added; rendered at default and Dehaze −100; the veil's colour fitted on the pixels away
from the patch, as the share of the way from the photo's own veil to the patch's colour
(about 110 renders):

| Patch | Share of the way (red / blue) |
| --- | --- |
| Pale orange (1, 0.75, 0.45), 1% of the frame, at 2 × the white | 0.96 / 1.00 |
| The same at 3.5 × the white | 0.84 / 0.83 |
| The same at 8 × (red and green clipped) and 16 × (all clipped) | 0.01 / 0.08, 0.00 / 0.04 |
| The same at 0.9 × on a photo with brighter highlights of its own; on a darker one | 0.00 at every size to 5%; 1.25 / 1.28 |
| Deep orange (1, 0.5, 0.05), 1%, at 0.9 × and 3.5 × | 0.00 |
| Pale orange at 2 ×: a square of 0.015% or 0.1% of the frame (26 px of 1000) | 0.00 to 0.04 |
| A square of 0.2%, 0.3%, 0.5%, 0.7% (36 px and more) | 0.96 / 1.00 each |
| A strip 8 px high of 0.3%; a strip 16 px high of 1% | 0.00; 0.96 / 1.00 |
| The 1% square flush with the right edge, 2% and 5% in, at the top, in the corner | 0.96 / 1.00 each |
| A tint of the whole photo (× 1.15, 1, 0.6), no patch | 0.93 / 0.93 (0.62 / 0.95 on a second photo) |
| White, clipped (16 ×), 1% and 0.3%, on the tinted photo | past neutral (1.36 / 1.32); 0.1%: 0.13 / 0.08 |
| White at 3 ×, 5 × (green clipped), 8 × and 16 × with a camera white balance of (2, 1, 1.54) | neutral each (1.000, 1.000, 0.979) |
| White of 1% in two night photos, at 0.9 ×, 2 × and 16 × | the veil goes white (1.003, 0.999, 0.997) |

So the pixels are ranked by their darkest channel, not their luminance (a saturated
light of any brightness does not count); a window of about 1.3% of the long edge is
taken first (a thin strip does not count); and the colour is all or nothing by whether
what is left of a patch is most of the haziest 0.1% of the frame: a majority, not a
mean, which would answer by degrees. Clipped lights count as white. The veil follows the
photo's own colour cast. The frame's edge is not special. The amount follows the
airlight's level too: the lift away from the patch is +1.47 stops without it, +1.62 with
a pale patch of 0.2 to 1% at 2 ×, +1.73 at 3.5 ×, +1.82 clipped; a patch of 0.1% changes
nothing.

A second round brackets the two sizes (the same patch colour at twice the white, on a
frame of 666 × 1000): squares of 28, 30 and 32 pixels leave the veil alone and one of 34
takes it over; strips 300 pixels wide do so from 12 pixels of height and not at 10;
strips 60 wide do not at 12 or 14; two squares of 20 do not, and three squares of 26 do
where one does not. So the window is 11 pixels per 1000 of the long edge, and what
counts is how many pixels are left after it, wherever they are: more than 0.073% and at
most 0.086% of the frame, the majority of the haziest 0.15 to 0.17%.

That is the dark channel prior's airlight with a median: the engine's window (0.5% of
the long edge each way, 7 pixels of its 512-pixel copy, a little wide: 13.7 per 1000)
and each channel's median over the haziest 0.1% (a little small). Partly clipped lights, on the same files rendered by Camera Raw and
by the engine with that rule (camera white balance (2, 1, 1.54); veil R G B):

| Patch (sensor values) | Camera Raw | Engine |
| --- | --- | --- |
| Red and green clipped, blue 0.2 | 1.38 0.85 0.30 | 1.39 0.84 0.29 |
| Red and green clipped, blue 0.6 | 1.12 0.95 0.75 | 1.14 0.94 0.74 |
| Green alone clipped | 1.21 0.92 0.51 | 1.22 0.91 0.49 |
| The first one's colour, unclipped | 1.39 0.84 0.33 | 1.40 0.84 0.32 |
| All clipped | neutral | neutral |

**The same rule on the training photos.** The earlier table took its statistics from
Camera Raw's render, where a source brighter than white is white, and found nothing
better than the mean of the haziest 1%. On the scene the engine itself measures
(unclamped linear ProPhoto after highlight reconstruction, 512 pixels; dumped by a test
build) the colour fitted to Camera Raw correlates 0.63 in red and 0.58 in blue with the
mean of the haziest 1% and 0.89 and 0.70 with the mean of the haziest 0.1%; the colour
error at −100 is 1.32 / 3.28 / 7.92 with the first, 0.82 / 2.26 / 5.09 with the second
and 0.79 / 1.97 / 4.97 with the median of the 0.1% (0.83 / 1.51 / 3.55 fitted per
photo). The choice is not sharp: shares of 0.05 to 0.3% and radii of 2 to 4 pixels give
0.82 to 0.93 at the median. The median answers the probes' patches as Camera Raw does
(simulated on the probe photo: 0.02 for 0.1% of the frame, 1.00 for 1%; the mean of the
0.1% gives 0.54). A red neon sign shows the difference: the mean of its 1% is the
neon's red (red 1.51, blue 0.49), the median of its 0.1% a white sign's (0.95, 1.15),
and Camera Raw's veil is blue-grey (0.95, 1.19).

Ranking by the darkest channel of the camera's own white-balanced values, where a
saturated colour has more of its weakest channel than in ProPhoto, is level over the
photos (with the haze equation below: 0.50 / 1.43 / 2.03 against 0.49 / 1.46 / 2.77; 8
photos closer by 0.5 and 6 further) and right on two that ProPhoto misses: a vault lit
by lamps (the veil becomes 1.18, 0.42; Camera Raw's fit 1.20, 0.42) and a face on a
bright orange backdrop, where the backdrop is the haze (1.49, 0.08; fit 1.58, −0.06).
The probes cannot tell the two rankings apart, their camera being ProPhoto.

Tried and worse: the colour of the typical pixel (the mean or median of each pixel's
colour per unit luminance, the geometric mean, the darker half: 5.6 to 7.0 at the
median against 1.73 for the haziest 1% of the render); a window minimum of 1 to 8% of
the long edge (the mean error rises to 5.9, with photos 17 to 23 further than neutral);
leaving out pixels near the clip (failures of 43 to 59); a veil coloured by the photo
blurred at 1 to 32% of its long edge, in place of one colour (6.9 to 4.0); the ring
around clipped areas (two of the worst photos have no clipped pixel); the darkest
channel taken in sRGB or Adobe RGB (2.8 against 1.96 in the mean; P3 2.04, Rec.2020
1.91); medians of larger shares (0.3%: 2.80, one photo 23 further); mixes of the
airlight with larger shares or the photo's mean, fitted with one photo left out (1.8 to
2.2). Neither the disagreement between shares nor the share of the haziest pixels that
is sensor-clipped tells where the rule is wrong (rank AUC 0.55 to 0.61; of 20 photos
whose haziest 0.1% is all clipped, mostly white skies, 19 are closer with the veil).

**How much of its own colour a pixel keeps: the haze equation.** Camera Raw rendering
the engine's own scene of five photos, as probe files, gives the engine rule's colour
where the fits on the real files agree (a night square: 1.29, 0.16 against 1.31, 0.16),
and the fits on dusk and blue-sky photos are yellower than both. They were made with
one kept share for all pixels. In `out = t · pixel + (1 − t) · airlight` the airlight
has a level: given the luminance before and after, `t = (Y_A − Y_after) / (Y_A −
Y_before)`, so a pixel far under the airlight keeps its colour and gains the veil, and
one near it, a sky, gives its colour up. With Camera Raw's own luminance and the
engine's airlight (median of the haziest 0.1%, colour and level), the colour error at
−100 over 100 photos:

| Kept share | Median / mean / p90 |
| --- | --- |
| 0.9 for every pixel | 0.80 / 1.96 / 4.97 |
| 1.0 for every pixel | 0.88 / 2.09 / 5.69 |
| The haze equation, the airlight's level as measured | 0.51 / 1.48 / 2.77 |
| The same with the level × 0.5, × 0.7, × 1.4, × 2 | 0.97 / 2.01, 0.71 / 1.66, 0.63 / 1.65, 0.71 / 1.80 |
| The colour fitted per photo, kept 0.9 | 0.83 / 1.51 / 3.55 |

The level is right as measured (47 of the photos are best at × 1), 26 photos are closer
by 0.5 than with one share and 1 further; two blue-sky photos go from 1.8 and 4.1 to 0.4
and 0.5. At −50 it is level with a share of 1.0 (0.85 / 1.32 / 2.55 against 0.85 / 1.31
/ 2.45). At the airlight's own level the equation is unstable; a floor of 1.05 to 1.5
on the ratio of the airlight's luminance to the pixel's costs nothing. So the yellow
veils fitted on blue-sky and dusk photos were the single share's doing.

Haze never takes a pixel past the airlight. Of the pixels under the airlight's level,
the share Camera Raw's −100 leaves more than a tenth above it is nothing at the median
and at the 90th percentile of the photos (0.295 on one, a dusk scene whose lamp is the
engine's airlight and not Camera Raw's); pixels above it lose 0.06 stop.

**The luminance is not the haze equation with one transmission.** With `g = t + (1 −
t) · Y_A / Y` and one `t` for all photos the gain's error at −100 is 0.799 stop (mean
1.021), with `t` per photo 0.578, with the level free as well 0.558; the engine's curve
leaves 0.388 and the photo's own best curve 0.343. Keying the engine's curve to the
airlight's level in place of the luminance's 99th percentile (the two differ by under
half a stop on 69 of 100 photos) is mixed on 34 photos: a night scene improves by 6.4
to 8.8 and the bulk gets worse (7 to 9 closer, 11 to 14 further). What Camera Raw's
amount follows is open: its gain four stops under the airlight is +1.5 to +1.9 on most
photos whatever the airlight's level over the white, and +0.45 and +0.97 on two night
scenes lit by coloured lights, which nothing tried explains (the airlight's level over
the white or over the 99th percentile, its darkest channel, the room under it).

The automatic black is not what keeps the fog off a night scene. Two night photos and a
daylight one as probe files with the default black on Auto and on None render alike at
−100: the first night scene gains 0.00 to 0.20 stop at every depth under its top with
either, the second up to 1.03 four to eight stops down, and the daylight photo 0.5 stop
at its top, 1.9 four stops down and 4.4 at its darkest. A black sky gets no fog and the
darkest corners of a daylight photo get the most, so the amount is not a curve of the
level under the top or under the airlight: it follows what surrounds the pixel at a
large scale, as the structure tests of 2026-11-16 found (the veil follows the photo
blurred at 8% of the frame better than any airlight). Night scenes are that large-scale
layer's clearest case.

**The candidate, and why it is not shipped.** The engine's luminance curve as it is,
with the gain added as a veil instead of applied per channel, in SDR renders (branch
`t3/negative-dehaze-veil`, local). Its forms in turn, rendered on the 100 training
photos (the change on Camera Raw's default against Camera Raw's render, ΔE76 of a
photo: median / mean / p90, photos closer / further than main by 0.3, the worst
worsening) and on validation's six Dehaze sets (53 photos, own ΔE00 against the
baseline of main):

| Form | Training −100 | | | Validation −100 | | −50 |
| --- | --- | --- | --- | --- | --- | --- |
| Main: the curve per channel | 8.00 / 9.42 / 14.73 | | | 6.56 / 7.47 / 13.60 | | 2.92 |
| Neutral veil, kept 1 to 0.9 | 6.25 / 8.04 / 12.42 | 73 / 16 | +11.2 | | | |
| Mean of the haziest 1% | 4.79 / 6.66 / 12.37 | 85 / 10 | +12.5 | 4.00 / 5.02 / 9.94 | 47 / 6 | 1.99 |
| Mean of the haziest 0.1% | 4.54 / 5.98 / 11.43 | 89 / 8 | +3.3 | 3.60 / 4.88 / 8.53 | 47 / 6 | 1.99 |
| Median of the haziest 0.1% | 4.59 / 6.06 / 11.12 | 86 / 9 | +9.0 | 3.48 / 4.87 / 8.56 | 48 / 5 | 1.93 |
| With the haze equation's kept share | 4.20 / 5.77 / 10.58 | 89 / 7 | +11.8 | 3.27 / 4.70 / 8.63 | 50 / 3 | 1.98 |
| With the ceiling and the camera's ranking | 4.09 / 5.87 / 10.53 | 90 / 6 | +20.5 | 3.15 / 4.60 / 8.76 | 50 / 3 | 1.95 |

The colour part of the training error goes from 5.21 to 1.32 with the last form; the
lightness part stays (3.49). On the synthetic chart −100 goes from 7.42 to 4.25 mean ΔE00 with the first
form. Every form fails the gate's regression rule, and the panels say the failures are
real:

- *A moonrise over water* (validation), which Camera Raw's −100 hardly lifts and leaves
  blue, turned pink with the mean of the 1% (3.79 → 5.94) and red-brown with the mean
  of the 0.1% (7.69): the photo the added-haze candidate of 2026-11-13 was rejected on.
  The median leaves it blue (3.87).
- *A portrait on black* and *a desert under a blue sky* (validation) got a skin-coloured
  and a sky-blue veil; with the median and the haze equation they are near Camera Raw's
  cool grey and pale sky.
- *A neon sign at night* (validation) is worse with every form and worst with the last
  ones (14.49 → 29.47 at −100, 4.99 → 14.52 at −50): a blue-grey fog where Camera Raw's
  veil is the street's saturated orange (its blue channel does not move at all). The
  haziest 0.1% is one lit white panel of about 0.8% of the frame, which by every probe
  above should be Camera Raw's airlight too; a white patch of that size added to two
  night photos does turn Camera Raw's veil white. Unexplained.
- *Night scenes lit by coloured lights* (training: Las Vegas, a neon club sign): Camera
  Raw's −100 changes next to nothing. Main lays a brown fog over them; the candidate a
  blue and a magenta one (+11.8 and +6.1), since the haziest pixels of a scene with
  nothing pale in it are saturated ones and the engine's curve lifts such a scene as
  much as any other.
- *A portrait by a grey-green wall* gets a pink cast (+5.2; the haziest 0.1% is skin,
  Camera Raw's veil the wall's grey) and *a dusk scene with one lamp* a grey-blue veil
  (+4.2; Camera Raw's is the lamp's yellow-orange, with a glow around it).
- *A face on a bright orange backdrop* (training): with the camera's ranking the veil is
  the backdrop's orange, as Camera Raw's, and the photo is 20.5 further than main: the
  skin, near the airlight's level, takes the backdrop's colour in full, where Camera Raw
  lays a peach fog over it.
- *Where the engine's curve lifts more than Camera Raw does* (a green park at −50:
  0.519 against 0.474 in display green), the excess is now fog where it was a lighter
  green.

Main's curve per channel keeps a scene's own saturation, which on these scenes is less
wrong than a fog of the wrong colour. What is established and waits for the amount: the
airlight rule (median of the haziest 0.1%, ranked in the camera's values), the kept
share from the haze equation, and the ceiling at the airlight's level.

Positive Dehaze with the mean of the haziest 0.1% as its airlight is a wash on the
training photos (+100: 3.18 → 3.01 at the median, 23 photos closer and 18 further, one
by 11.7; +50: 1.37 → 1.43): its strengths were fitted with the 1%. With the median of
the 0.1% it is 3.18 → 2.84 (mean 3.30 → 3.23; 24 closer, 14 further) and 1.37 → 1.31 at
+50: 6 closer on the photos whose 1% is a light of another colour (a pier at dusk, a
sunlit city, a neon sign) and 16.5 further on the portrait by the grey-green wall, whose
haziest 0.1% is skin. The same photo fails with either sign, so its airlight is the
thing to get right first: its median turns from the skin's colour to the wall's between
a share of 0.15% and 0.2%.

### What is left of Shadows, Highlights and Whites, and what more training photos buy (2026-11-18)

After #1128 and #1153 the open tone items were gone through again in the order of
their error, with five bounded measurements on the training photos. None of the cheap
causes holds: the errors left at full sliders are scatter between photos, not a bias,
and the one thing that reduces it is more photos to fit on. Camera Raw 18.7 through a
linear profile at 1024 px, 122 to 228 training photos; nothing fitted on validation,
holdout untouched.

**The error by setting**, from the stored validation runs (the builds of #1153 and
#1128 for the sets they change, the 2026-11-12 baseline for the rest, so the Blacks rows
predate #1068). The control's own ΔE00 (the default render's error carried over),
median over photos, with the 90th percentile and the share of photos over 2:

| Setting | Own | p90 | Over 2 |
| --- | --- | --- | --- |
| Dehaze −100 / −50 / +100 | 6.56 / 2.92 / 2.23 | 13.6 / 5.3 / 4.8 | 96% / 72% / 57% |
| Clarity +100 | 1.94 | 3.1 | 48% |
| Exposure −1 with Whites +100 | 1.55 | 3.8 | 43% |
| Shadows +100 | 1.32 | 3.4 | 23% |
| Whites +100 | 1.01 | 3.1 | 25% |
| Shadows +50 | 0.54 | 1.2 | 2% |
| Shadows +50 with Highlights −50 | 0.54 | 1.0 | 2% |
| Shadows −100 | 0.49 | 1.2 | 0% |
| Highlights +100 | 0.46 | 1.1 | 2% |
| Highlights −100 | 0.42 | 0.9 | 2% |
| Whites +50 | 0.15 | 0.6 | 2% |

Among Shadows, Highlights, Whites and Blacks that leaves positive Shadows and Whites
+100. Shadows +100's own error is spread over every tone under L\* 70 (2.3 to 2.5 ΔE00
pooled in each band from 10 to 60); its lightness bias is +1.1 L\* under 20, +0.5 from
20 to 40 and −0.1 to −0.9 from 40 to 70, a small part of it.

**The default black's key under positive Shadows: half of the deep-shadow bias, a
twentieth of the error.** The key follows positive Shadows by the gain's median over
the map and negative Shadows by the gain on the darkest levels. On six photos the first
is 0.16 to 1.2 stop at +100 and the second 2.0 to 2.8. A test build moved the key by a
share of the difference (228 photos, Shadows +100; the RMS of cube-root differences
against Camera Raw, and the lift of the 32-pixel level less Camera Raw's, by stops under
the top):

| Key follows | Median | Mean | p90 | Bias 4–5 / 6–8 / 8–10 stops down | Photos closer / further |
| --- | --- | --- | --- | --- | --- |
| The median gain (main) | 0.0109 | 0.0135 | 0.0236 | +0.12 / +0.33 / +0.51 | |
| A quarter of the way | 0.0106 | 0.0132 | 0.0231 | +0.11 / +0.30 / +0.47 | 72 / 7 |
| Half way | 0.0103 | 0.0130 | 0.0227 | +0.09 / +0.26 / +0.41 | 98 / 14 |
| The darkest levels' gain | 0.0105 | 0.0130 | 0.0223 | +0.05 / +0.17 / +0.24 | 98 / 35 |

The spread between photos at those depths (0.2 to 0.7 stop RMS) hardly moves. Not a
change on its own; it belongs to a refit of the first ensemble, whose deep targets would
have to be taken through the same key.

**The level positive Whites stretches toward: right for most photos.** Whites +100 was
rendered with the level (twice the luminance's 99th percentile, at most the sensor's
white) moved by −1, −0.5, +0.5 and +1 stop (122 photos). The error is steep in it:
0.128, 0.063, 0.0115, 0.055 and 0.103 at the five levels (median). Each photo's best
offset is +0.01 at the median, with quartiles −0.04 and +0.09 and tenths −0.33 and
+0.25, and the best level less each percentile has its smallest spread at the 99th
(0.42 stop; 0.49 at the 98th, 0.81 at the 99.5th, 1.41 at the maximum). The misses are
photos with a steep top:

| 99th less 98th percentile | Photos | Best offset, median | Main, mean | Best of the five, mean |
| --- | --- | --- | --- | --- |
| under 0.6 stop | 97 | −0.02 to +0.05 | 0.0147–0.0188 | 0.0142–0.0178 |
| 0.6 to 1.2 | 15 | 0.00 (lower quartile −0.43) | 0.0229 | 0.0126 |
| over 1.2 | 10 | −0.69 | 0.0496 | 0.0218 |

Of the twelve photos that need the level a third of a stop or more lower, five have
0.3 to 1.6% of their pixels on a clipped plateau at the sensor's white. Taking the
percentile over the pixels under the sensor's white moves four photos by more than a
quarter stop, three closer and one further (mean 0.0209 → 0.0193, 90th percentile
0.0507 → 0.0479, read off the five rendered levels). Lower percentiles with a larger
offset (the 98th, 97th or 95th, alone or as a limit on the 99th) make the bulk worse
(6 to 12 photos closer, 6 to 67 further). The other seven have no plateau in the
engine's luminance. Not changed: the gain is under a tenth and a rule for the tail
needs more photos than 25.

**Whites with Exposure: the level moves with Exposure as Camera Raw's does.** Camera Raw
at Exposure −1 and +1, alone and with Whites +100 (122 photos). The gain of Whites +100
in the mid-tones, two to five stops under the top, is 1.30 stop in Camera Raw and 1.38
in RAWmakase at Exposure 0, 2.11 and 2.18 at −1, 0.59 and 0.60 at +1. Its change with
Exposure is +0.855 stop in Camera Raw and +0.843 in RAWmakase at −1, −0.716 and −0.720
at +1. What grows at Exposure −1 is the scatter between photos (the gain's difference
is 0.31 stop RMS at 0, 0.42 at −1 and 0.18 at +1), which is why Exposure −1 with Whites
+100 is the worst of the Whites cases on validation: the same per-photo error of the
level under a larger stretch.

**The curve's shape near the top: no difference at the median.** The output of Whites
+100 by the default render's luminance, in quarter stops from five stops under white:
RAWmakase less Camera Raw is within 0.03 stop at the median of the photos at every
level, and −0.04 to −0.06 at the mean between 3.5 and 1.75 stops under white, the same
tail of photos. Camera Raw has a channel at white on 19.6% of the pixels at Whites +100
and RAWmakase on 15.4%.

**More photos.** The second ensemble's method, fitted on a share of the training folds'
photos and scored on the held-out fold (four folds of 213 photos; the 436 mosaics
always in; RMS of the gain over a photo's pixels in stops, median and mean over photos):

| Photos | Shadows +100 | Highlights −100 | Shadows −100 | Highlights +100 |
| --- | --- | --- | --- | --- |
| 0 | 0.288 (0.499) | 0.170 (0.840) | 0.342 (0.533) | 0.178 (0.241) |
| 40 | 0.229 (0.310) | 0.161 (0.215) | 0.267 (0.347) | 0.156 (0.189) |
| 80 | 0.208 (0.265) | 0.146 (0.198) | 0.239 (0.306) | 0.148 (0.178) |
| 120 | 0.195 (0.256) | 0.140 (0.183) | 0.223 (0.292) | 0.138 (0.170) |
| 160 | 0.184 (0.244) | 0.144 (0.181) | 0.215 (0.279) | 0.136 (0.168) |

Shadows of both signs still improves by a tenth for each doubling at 160 photos;
Highlights by 1 to 8%. The dataset holds about 790 more in-scope photos of
the train split that the rule has not seen (421 from raw.pixls.us, 342 from Signature
Edits, 26 others). They are being rendered in Camera Raw at the rule's settings and at
Whites +100; a refit of both ensembles on them is the next candidate.

**The newer method for the first ensemble's curves: no difference.** A network of the
second ensemble's method (its inputs, its targets through the engine's white curve)
for Shadows +100 and Highlights −100 too, fitted on the same 213 photos and rendered in a
test build (189 photos): Shadows +100 0.0115 at the median against main's 0.0112 (50
photos closer, 87 further), Highlights −100 0.0094 against 0.0101 (71 and 68). With the
default black's key on the darkest levels' gain as well, Shadows +100 is at 0.0106 (84
and 51). The two ensembles stay; both are to be refitted on the new photos.

**What the rule's error is made of (2026-11-19).** The first 162 of those photos are
in (375 training photos with the 213 before). Held out by photo in four folds, the
second ensemble's method leaves 0.176, 0.125, 0.206 and 0.123 stop at Shadows +100,
Highlights −100, Shadows −100 and Highlights +100 (0.184, 0.142, 0.216 and 0.135 on the
213). Like for like, on the old photos held out and fitted with or without the new
ones: 0.185 → 0.173, 0.132 → 0.129, 0.218 → 0.207 and 0.129 → 0.128 at the median
(84 to 90 photos closer by 0.01, 61 to 68 further), and 1 to 4% at the mean: real and
small, and the worst photos do not move.

Each held-out photo's error was then split (RMS over its pixels, median over photos):
the prediction as it is; scaled by the one factor that suits the photo best; and the
photo's own best curve of the base level, which no rule can beat.

| | Shadows +100 | Highlights −100 | Shadows −100 | Highlights +100 |
| --- | --- | --- | --- | --- |
| Predicted, photo held out | 0.178 | 0.124 | 0.206 | 0.122 |
| With the photo's best factor | 0.148 | 0.110 | 0.168 | 0.101 |
| The photo's own best curve | 0.131 | 0.080 | 0.165 | 0.081 |

For Shadows more than half of the error's variance (54% at +100, 64% at −100) is left
by any curve of the base level, and 42 to 44% for Highlights: the gain is not a
function of the base level alone. Of what a curve could gain, two thirds is one factor
per photo (the whole curve a fifth too strong or too weak on a tenth of the photos
each way). Nothing known about a photo goes with that factor: its best correlations
are 0.23 with the share of black pixels and 0.19 with the top's texture, and the
classes of scene, light, contrast and camera maker explain 1 to 5% of it.

What is left under the photo's own best curve (0.12 stop for Shadows, 0.075 for
Highlights, on the engine's 512-pixel map) was tested against four local causes, each
as a term with one set of coefficients for all photos:

- *A base at another scale* (the curve's slope times a Gaussian of 2 to 64 map pixels
  less the base): 1% of its variance; 14% with coefficients fitted to each photo. With
  the pixel's own detail as a term too, 5 to 7% and 19 to 21%.
- *Another intensity than luminance* (the slope times the log of each channel, of the
  brightest and of the darkest over luminance): 0.5%, no more on the most colourful
  quarter of the photos.
- *Local texture as a multiplier of the lift*: the sign holds for Shadows (3 to 4%
  more lift for each doubling of the texture around a pixel, on three photos in four),
  and it explains 1%.
- *A base that follows fine detail less* (less a quarter to a half of the guided
  filter's weight times the detail finer than two map pixels): 3% less of the residual
  for Shadows, none for Highlights, and the contrast kept in the finest band then moves
  away from Camera Raw's (Shadows +100: Camera Raw 0.80, the base as shipped 0.89, with
  the change 0.93).

On contact sheets the residual shows large smooth dark areas (hair, a car's body)
lifted more than the curve says, small dark details less, smooth haze less than a
textured meadow at the same level, and noise on night scenes, where it is largest.
The stage's form (a per-photo curve of a guided base level) is therefore near its
floor for Shadows and Highlights at full sliders: more photos move the predictable
third slowly, and no single local term explains the rest.

**The refit on the 375 photos: rejected on validation (2026-11-19).** Both ensembles
and the pair model were fitted again on the larger set and rendered through the
validation gate against a new baseline of main (`main-6ef32ab87-metric2`). The control's
own ΔE00, median over photos, main → refit, with the photos closer and further by 0.05:

| Setting | Main | Refit | Closer / further |
| --- | --- | --- | --- |
| Shadows +100 | 1.32 | 1.52 | 17 / 28 |
| Shadows +50 | 0.54 | 0.62 | 10 / 21 |
| Shadows +25 | 0.22 | 0.27 | 4 / 12 |
| Shadows −100 | 0.49 | 0.51 | 15 / 17 |
| Highlights +100 | 0.46 | 0.50 | 17 / 18 |
| Highlights −100 | 0.42 | 0.43 | 16 / 21 |
| Shadows +100, Highlights −100 | 1.13 | 1.28 | 13 / 13 |
| Shadows +100, Highlights +100 | 3.01 | 3.53 | 16 / 15 |
| Shadows +80, Highlights −30 | 0.91 | 1.00 | 8 / 15 |

Over every case it changes, 224 photos are closer and 299 further. At Shadows +100 the
refit is 0.5 to 0.8 L\* brighter than main in every band from 0 to 60, where main sat
within 0.6 of Camera Raw above 20. On the 228 known training photos the two builds are
level, as they must be on photos both have seen (Shadows +100 0.0106 and 0.0105).
What the held-out folds promised did not carry over. Two things bear on that: the
first ensemble's shipped fit was chosen among several with these validation sets in
view (2026-10-28), so a new fit starts behind on them whatever its merit; and the new
photos change the balance of photos to mosaics, which is what set the mid-shadow lift
then. Not shipped. The 162 photos' renders are kept for a fit of another form.

**The baseline of main after #1068, #1128, #1153 and #1162** (`main-6ef32ab87-metric2`,
83 validation sets, 2026-11-19). Blacks +100 now adds 0.95 (1.70 before #1068), +50
0.33 and +25 0.05. The new `pairs` cases, own ΔE00 at the median and the 90th
percentile over 33 photos: Shadows +100 with Highlights +100 3.01 and 7.13 (three
photos in four over 2), −100 with −100 1.36 and 2.96, +100 with −100 1.13 and 2.02,
+80 with −30 0.91 and 1.47, +60 with −60, Whites +30 and Blacks −30 0.90 and 1.22, −100
with +100 0.85 and 1.25. With both sliders at +100 RAWmakase is 2.4 to 4.2 L\* brighter
than Camera Raw in every band from 0 to 50, most on the stress photos: the largest
error outside Dehaze −100, and one the training photos' scene values had not shown.

**The gate (#1175).** Its sets now hold Shadows with Highlights in the four sign combinations at
full sliders, Shadows +80 with Highlights −30, and Shadows +60 with Highlights −60,
Whites +30 and Blacks −30: the `pairs` sets, 33 validation photos of four families,
the same photos as the mixed sets.

### Negative Shadows and positive Highlights by the global rule (2026-11-17)

Shadows, Highlights, Whites and Blacks come before Dehaze and Clarity. This section
records what Camera Raw's renders of the four show on the training photos, the
extension of the [global rule](../scene-tone-stage.md#the-global-rule) to Shadows −100
and Highlights +100 that came out of it, and what was tried on the way and did not
work. Training photos only: the 134 photos of the three families' shadows, highlights,
whites and blacks training sets at each control's ±100, then all 243 training photos at
default, Shadows ±100 and −50 and Highlights ±100 and +50, as scene values through a
linear profile at 1024 pixels. Nothing was fitted on validation; holdout is untouched.
Errors are the RMS of the difference of cube roots of linear values over all channels
(0.01 is about 1.2 L\*), with each engine's own change (its render at the setting over
main's default render) applied to Camera Raw's default render, so the default's own
differences do not count.

**Where main stood** (108 photos whose renders line up with the engine's; median, and
for main also mean and 90th percentile):

| Setting | No change | Main | Main given Camera Raw's 32-pixel level |
| --- | --- | --- | --- |
| Highlights +100 | 0.0626 | 0.0189 / 0.0248 / 0.0510 | 0.0081 |
| Shadows −100 | 0.0378 | 0.0128 / 0.0143 / 0.0228 | 0.0071 |
| Shadows +100 | 0.0484 | 0.0119 / 0.0130 / 0.0196 | 0.0077 |
| Whites +100 | 0.1960 | 0.0118 / 0.0212 / 0.0509 | 0.0067 |
| Highlights −100 | 0.0550 | 0.0103 / 0.0117 / 0.0182 | 0.0053 |
| Blacks +100 | 0.0504 | 0.0051 / 0.0055 / 0.0082 | 0.0051 |
| Blacks −100 | 0.1582 | 0.0042 / 0.0074 / 0.0161 | 0.0041 |
| Whites −100 | 0.0204 | 0.0031 / 0.0041 / 0.0089 | 0.0031 |

Blacks of both signs and Whites −100 are matched. The two largest errors were the two
settings still rendered from fitted tables, and most of each is the large-scale level
(last column: main's render with its 32-pixel level replaced by Camera Raw's).

**What Camera Raw's renders show, with no model of ours between them.**

- *Which level the curve reads does not matter.* With the best curve per photo, main's
  guided base (radius 32 of 1024 pixels, ε 0.5) scores 0.0093 at Shadows +100, as the
  best of the radii and ε tried (0.0093), against 0.0122 for a curve of the pixel
  itself. The base filter's constants are not a lever.
- *Shadows and Highlights are not one detail filter on a large-scale layer*, unlike
  negative Dehaze: given Camera Raw's own 32-pixel level and a median detail gain, 0.0093
  to 0.0112 remains, and 0.0053 to 0.0063 given its 8-pixel level. A perfect curve per
  photo at a guided base leaves 0.0085 (Shadows +100), 0.0068 (−100), 0.0093
  (Highlights −100) and 0.0132 (+100): the curve is worth up to half of main's error at
  Shadows −100 and a third at Highlights +100.
- *Fine detail under positive Shadows.* Camera Raw keeps 0.88, 0.83, 0.79, 0.77, 0.74,
  0.73 and 0.70 of the detail in octave bands from 1 to 64 pixels (0.66 of the low-pass)
  at Shadows +100; main 0.72, 0.72, 0.71, 0.71, 0.70, 0.66 and 0.64 (0.62). At −100
  Camera Raw 1.13 to 1.32 (1.34), main 1.27 to 1.36 (1.40). Highlights, Whites and
  Blacks are within 1 to 3%. Not addressed here.
- *The lift by level*, main less Camera Raw in stops, by stops under the photo's top:
  Shadows +100 0, +0.01, +0.03, +0.06, +0.08, +0.16, +0.31 down to eight stops, +0.60 at
  eight to ten and +1.05 below (deep shadows over-lifted); Shadows −100 +0.02 to +0.14
  in the first four stops, then −0.03 and −0.23; Highlights +100 −0.11 and −0.10 in the
  top two stops, with a tenth of the photos 0.6 short.
- *Highlights +100 follows how dark the photo is.* Its lift of a photo's top stop is
  0.35 to 1.69 EV (10th to 90th percentile) and correlates −0.86 with the photo's median
  level. Main's worst photos are low-key (a window in a dark room, streets at night,
  neon): Camera Raw lifts their bright areas 1.3 to 3.3 EV, main 0.5 to 1.9, and on
  daylight scenes Camera Raw lifts about 0.1 where main lifts 0.5. A rule of the
  histogram's shape alone cannot tell these apart: cross-validated on these photos, it
  was worse than main at Highlights +100 (0.0239 against 0.0184) until it also read the
  level and the photo's top, median and mean themselves (0.0195).
- *Whites +100* follows the 99th percentile (correlation −0.93 to −0.95 with the lift);
  main's error there (0.15 to 0.30 stop RMS) is no worse than such fits, and its misses
  are photos with a clipped top. Open.
- *The slider is linear.* The lift in stops at half the slider over the lift at the full
  slider is 0.49 to 0.50 at every level for Shadows −50 against −100, and 0.50 for
  Highlights +50 against +100 from two stops under the top down, rising to 0.51, 0.52,
  0.54 and 0.56 in the half-stops up to the top (a tenth of the photos reach 0.70
  there).
- *Highlights acts before the white curve.* Predicting Camera Raw's Highlights +50
  render from its default and +100 renders as linear in the slider on the output is off
  by −0.02 to −0.04 stop in the top two stops (RMS 0.03 to 0.115); as linear on the
  input of the engine's white curve of that photo, by −0.00 to −0.02 (0.03 to 0.08). The
  engine's order, the gain first and the white curve after, is the better description.

**The rule's extension.** A second ensemble of the global rule's networks gives the
curves of Shadows −100 and Highlights +100 (28 inputs: the first's 24 and the level and
the photo's top, median and mean), fitted on 436 mosaics and 213 of the 243 training
photos; the fitted tables are no longer read
([scene tone stage](../scene-tone-stage.md#the-global-rule)). The builds on the way, on
228 training photos (median; main is 0.0118 at Shadows −100 and 0.0159 at Highlights
+100; share of the pixels at white at Highlights +100 and +50: Camera Raw 0.89% and
0.57%, main 0.94% and 0.48%):

| Build | What it added | Shadows −100 | Highlights +100 | At white, +100 / +50 | What was still wrong |
| --- | --- | --- | --- | --- | --- |
| 1 | The second ensemble, the default black's key untouched | 0.0106 | 0.0126 | | Shadows −100: tones four to eight stops under the top 0.3 to 0.6 stop too dark |
| 2 | The key moved by the median gain of negative Shadows, as positive Shadows moves it | 0.0096 | | | Still 0.24 to 0.49 stop too dark there |
| 3 | The key moved by the gain of the darkest 0.1% of the map, the levels it reads | 0.0069 | | | |
| 6 | Refitted with 213 photos; first validation gate (252 pass, 62 fail of 432) | 0.0068 | 0.0127 | 0.44% | Positive Highlights short of white |
| 7 | Targets exact up to white | | | 0.30% | Worse: a clipped result understates the gain |
| 8 | Clipped results as bounds | | | 0.41% | |
| 9 | Bounds, and 0.3 stop more where clipped | | | 0.50% | |
| 10 | Targets with the engine's white curve undone, the curve measured on the photo's own pixels; second gate (256 pass, 56 fail) | 0.0069 | 0.0118 | 0.60% / 0.26% | +25 and +50: the L\* ≥ 90 band worse than main on validation (1.55 → 2.08, 2.13 → 2.59) |
| 12 | Half-slider renders where the full slider clipped | 0.0067 | 0.0119 | 0.64% / 0.30% | Only 0.2% of the samples were clipped at all: not the cause |
| 13 | The engine's own white curve, also above the photo's top | 0.0067 | 0.0108 | 0.75% / 0.35% | The fit lands on the bound, half of the pixels just under white |
| 14 | A clipped result asks for white and 0.25 stop more | | 0.0110 | 0.83% / 0.40% | Coloured highlights: Camera Raw clips a channel, luminance is under 0.97 |
| 15 | The brightest channel decides what is clipped; third gate, stopped after 10 of 18 sets | | 0.0106 | 1.03% / 0.49% | A validation photo with a few black holes 2.21 → 6.29 ΔE00 at Shadows −100 |
| 17 | The second ensemble's bottom within 3.5 stops of the 10th percentile | 0.0068 | 0.0106 | 1.00% / 0.48% | |

**What did not work, and why.**

- *The key moved by the median gain for negative Shadows.* The default black's key is
  the 0.1th percentile; what matters is where Shadows leaves those levels, not the
  median level (build 2 against 3).
- *The darkest levels' gain for positive Shadows too.* On the training photos it halved
  the deepest shadows' over-lift at +100 (+0.11, +0.17 and +0.30 stop at four, five and
  six to eight stops under the top to +0.03, +0.06 and +0.14; 0.0106 → 0.0103, 96 photos
  better and 35 worse). On validation it was a wash (Shadows +100 median 1.66 → 1.67,
  mean 1.80 → 1.76; +50 0.93 → 0.96, stress 0.75 → 0.81) and failed the gate's
  median rule in three strata. Positive Shadows keeps the median.
- *The key moved by Highlights' gain.* No measurable effect (Highlights +100 0.0118
  with it, 0.0120 without). Dropped.
- *Targets up to white as they are seen* (build 7): a result at white says only that the
  gain was at least that much, and fitting it as the gain under-lifts.
- *A white curve measured on the photo's own pixels* (builds 10 to 12): it ends at the
  photo's top, which is under 0.9 of white on 73% of the training photos, so every
  Camera Raw result above the top was read as the top. The top stop stayed 0.05 stop
  short and positive Highlights at +25 and +50 lost the highlights main's table had. A
  full-frame mask's Exposure reaches above the top on most photos but not on one with a
  clipped top, so the engine now writes its curve out for the fit
  (`RAWMAKASE_RULE_DUMP`).
- *Luminance deciding what is clipped* (build 14): 57% of the pixels Camera Raw newly
  puts at white at Highlights +50 have a luminance under 0.97; the candidate reached
  white on 88% of the rest and on 37 to 54% of these.
- *The histogram's bottom as the 0.1th percentile* (build 15): a few black pixels set
  it. The four training photos whose bottom sat more than four stops under their 10th
  percentile (two of them the aspect-crop photos, whose map includes a black sensor
  border) were the four worst at Shadows −100 (0.0545, 0.0326, 0.0261 and 0.0273 against
  main's 0.0055, 0.0073, 0.0189 and 0.0051), their mid-tones 0.26 to 0.91 stop too dark;
  the median training photo's bottom is 1.5 stops under its 10th percentile and the
  98th percentile's 3.8. With the bottom held within 3.5 stops they are 0.0221, 0.0208,
  0.0146 and 0.0137, still 0.1 to 0.3 stop too dark in the upper stops on three, and
  better than main at Highlights +100 and +50 on all four. The first ensemble has the
  same weakness and was left as fitted.

**Validation.** The gate's shadows, highlights and mixed validation sets of six
families against the recorded baseline of main: the median ΔE00 of each photo, over the
three main families' 29 Shadows and 32 Highlights photos and over the stress families'
24 (median, mean, 90th percentile).

| Setting | Main families: median | mean | 90th percentile | Stress families: median | mean | 90th percentile |
| --- | --- | --- | --- | --- | --- | --- |
| Shadows −100 | 0.78 → 0.54 | 1.04 → 0.66 | 2.22 → 1.13 | 0.60 → 0.62 | 0.84 → 0.70 | 1.62 → 1.27 |
| Shadows −50 | 0.74 → 0.50 | 0.78 → 0.60 | 1.36 → 1.04 | 0.61 → 0.55 | 0.73 → 0.67 | 1.38 → 1.14 |
| Shadows −25 | 0.57 → 0.50 | 0.65 → 0.57 | 1.15 → 1.03 | 0.59 → 0.52 | 0.67 → 0.65 | 1.25 → 1.10 |
| Highlights +100 | 0.95 → 0.81 | 1.27 → 1.02 | 2.12 → 1.93 | 1.09 → 1.00 | 1.55 → 1.13 | 3.92 → 1.82 |
| Highlights +50 | 0.75 → 0.67 | 0.89 → 0.79 | 1.50 → 1.49 | 0.78 → 0.79 | 1.08 → 0.88 | 2.29 → 1.48 |
| Highlights +25 | 0.61 → 0.50 | 0.72 → 0.68 | 1.32 → 1.33 | 0.71 → 0.65 | 0.85 → 0.78 | 1.54 → 1.34 |

Positive Shadows, negative Highlights and the mixed sets render as on main.

The gate's regression rule fails a stratum when any of a photo's 650 or so windows, or its
band of L\* ≥ 90 or ≤ 10, is 2 ΔE00 further from Camera Raw, when a photo's clipping
disagreement grows by half a point, or when the stratum's median is 0.1 worse. It fails
42 of the 432 setting and stratum pairs (271 pass, 119 have too few photos to judge);
builds 6 and 10 failed 62 and 56. Over all 56 Highlights and 53 Shadows photo cases:

| Setting | Windows 2 ΔE00 worse (photos) | Windows 2 ΔE00 better (photos) | Band (L\* ≥ 90 for Highlights, ≤ 10 for Shadows) | Clipping disagreement, % | Photos' clipping 0.5 point worse / better |
| --- | --- | --- | --- | --- | --- |
| Highlights +100 | 585 (18) | 3,132 (32) | 3.02 → 2.18 | 1.42 → 1.44 | 9 / 10 |
| Highlights +50 | 85 (6) | 176 (12) | 2.13 → 2.05 | 1.04 → 1.14 | 7 / 5 |
| Highlights +25 | 0 | 0 | 1.55 → 1.67 | 0.95 → 1.01 | 2 / 0 |
| Shadows −100 | 32 (4) | 1,306 (19) | 0.93 → 0.67 | 4.52 → 2.55 | 5 / 24 |
| Shadows −50 | 0 | 71 (5) | 0.84 → 0.68 | 1.68 → 1.39 | 9 / 13 |
| Shadows −25 | 0 | 0 | 0.80 → 0.71 | 1.29 → 1.09 | 4 / 9 |

What gets worse. At Highlights +100: a silhouette on a beach at dusk (1.83 → 2.55, and
0.73 → 1.22 at +50), a portrait against a pale wall (0.71 → 1.32), a lioness in grass
(1.72 → 2.21), a taxi at night (0.93 → 1.37) and a snow-capped mountain (0.68 → 1.06).
At negative Shadows: a village in a valley (2.29 → 2.60 at −50) and a man in a dim room
(0.74 → 0.98 at −100, its disagreement with Camera Raw on which pixels are black
growing). Five strata fail on their median, each of four photos: three of one stress
family's bright photos at positive Highlights (0.91 → 1.36 at +100), one of dark
photos at Highlights +100 and one at Shadows −100. Highlights +25 is the one setting
whose highlight band is further from Camera Raw than main's.

**Rendering tests that moved.** The synthetic chart and the synthetic scenes are what
the tables were fitted on, and the rule is not closer on all of them.
`tests/corpus/camera-raw/baseline.json`, the chart's distance from Camera Raw (mean and
95th percentile ΔE00): Highlights +100 6.17 / 10.34 → 6.99 / 12.57 and +60 4.16 / 6.78
→ 4.91 / 8.13 (further), Shadows −100 6.16 / 10.83 → 4.95 / 8.86 and −60 3.66 / 6.42 →
2.99 / 6.06 (closer); flat patches are a degenerate case for both controls (a flat
maximum). `scene-probes-baseline.json`, the error of the setting's effect on three
synthetic scenes (EV): Highlights +50 0.023, 0.036 and 0.012 → 0.043, 0.077 and 0.019
(further), Shadows −50 0.019, 0.067 and 0.066 → 0.013, 0.014 and 0.075 (the first two
closer, the third further). Those entries and the chart's four snapshots were rewritten
with this change; nothing else in the three files was.

**Open.** Whites +100 on photos with a clipped top. The fine detail Camera Raw keeps
under positive Shadows and the deep shadows' over-lift at Shadows +100. Photos shot
with an in-camera aspect crop: the engine measures the whole sensor area, Camera Raw
the crop. The first ensemble's bottom. The tail of daylight photos whose mid-tones
positive Highlights lifts too little.

### Negative Dehaze: what was tried, why it failed, and what Camera Raw's renders say (2026-11-16)

The added-haze candidate of 2026-11-13 (next section) is withdrawn: its medians halved
but its renders are wrong to the eye. This section lists every approach to negative
Dehaze so far with the reason it failed, and then what Camera Raw's own renders show
when no model of ours is put between them. Training photos only (the 52 `dehaze-train`
photos at −10, −25, −50, −75, −100, +50 and +100, scene values through a linear profile);
nothing fitted on validation, holdout untouched. Errors below are the RMS of the
difference of cube roots of linear values (a lightness-like measure; 0.01 is about
1.2 L\*), bright pixels kept, unless a unit is given.

**Why the candidate is withdrawn.** Rendered by the candidate's build and compared in
display values at −100, 9 of the 50 `dehaze-train` photos are more than 9 ΔE00 from
Camera Raw (worst 24.4): six by colour (b\* off by 13 to 24: sunsets and sodium-lit
night scenes, veiled in one airlight colour) and two by lightness (L\* 7 and 14 too
dark); errors of +5 to +10 L\* in one lightness band are common. A forest in fog comes
out as a bright, patchy fog where Camera Raw's is even and darker; a moonrise is veiled
in the moon's orange. The median ΔE00 and the log-exposure error it was fitted with
(bright pixels masked) showed neither. A candidate is now judged on contact sheets and
lightness bands of every training photo before any gate run.

**Approaches so far.**

| When | Approach | Result | Why it failed |
| --- | --- | --- | --- |
| 2026-10-05 | A spatial model fitted to synthetic scenes | −100 worse (block error 0.0523 → 0.0637), luminance only | Synthetic scenes do not carry a photo's structure |
| On main | One curve per amount keyed on the 99th percentile, no spatial part | Gate −100 median 5.88, its own error 6.56; the dark half +3.4 to +4.2 L\* too light | A photo's curve is not where the 99th percentile puts it, and a curve cannot lower local contrast |
| 2026-10-20 | The haze equation J = p·t + A·(1 − t) with a dark-channel t | Far too much haze (shadows +26 L\* at −25) | The strength was never measured against Camera Raw |
| 2026-10-20 | A veil of the median luminance in the airlight's colour, fading toward black | Training median 8.62 → 6.66, but 11 of 26 photos worse (one 3.24 → 12.79) | One global veil; Camera Raw's contrast change differs by photo (0.34 to 1.33) |
| 2026-10-20 | The curves keyed on the median | 8.62 → 8.78 | As the curve on main |
| 2026-11-12 | Flat mosaics in four tints | Main is already within 0.16 to 0.42 EV there | Flat patches have no local contrast, and they were rendered without the automatic black, which the photos have (below) |
| 2026-11-13 | The best curve of each photo (an oracle) | 0.225 EV left of 1.49 | No curve of the level can do better |
| 2026-11-13 | That curve's position from statistics, or from the Shadows/Highlights rule's 24 descriptors and network | No better than the 99th percentile (0.510 against 0.475) | 94 photos are too few to learn a rule of the whole photo |
| 2026-11-13 | A table of the level under the blurred maximum and of the local level | Cross-validated 0.354 EV; gate median stayed 5.88, 9 of 29 worse | As above, and a table of two levels is still a curve |
| 2026-11-13 | The haze model backwards, t = 1 + k·density | Gate 5.88 → 2.90; withdrawn | See above and the scale test below |
| 2026-11-13/14 | Repairs of it: a veil fading toward black (four forms), the density capped by the pixel's darkest channel or clipped, a strength per photo from one or two statistics, an airlight from windows of 2% to 10%, an airlight chosen by the brightest 1% or 0.1% of blurred copies, a free airlight per photo (0.0288 → 0.0225), a transmission (1 − d)^g | None changed the failures | The family is the limit, not its constants |
| 2026-11-16 | The same equation with the density blurred by 1% to 16% of the long edge, from the dark channel, the darkest channel or luminance, with either transmission law | Best 0.029 to 0.031 (the engine's own filter and 1% to 2% blurs); 8% and 16% are worse (0.036 to 0.047) | As above |

What these have in common: each guessed the operator's form and fitted its output
error, in which the working values, the airlight, the density map and the strength law
are mixed; each was judged by one median; and every probe was either a flat mosaic or a
whole photo, never a photo with one thing changed (the photo probes of 2026-11-13 were
the exception, and were what produced a model).

**What Camera Raw's renders say.** Measured on its renders alone, in 32 px cells of the
2048 px renders (out = a + b·in per channel) and in octave bands of a 1024 px copy.

- *One contrast factor for the three channels.* The cell slope of red and of blue over
  green's is 1.000 at the median (10th to 90th percentile 0.94 to 1.05 and 0.9 to
  1.15). Within a cell the change is affine in linear values (r² 0.943 at −100, 0.990 at
  +100; a square-root, cube-root or log encoding is no closer).
- *Positive and negative Dehaze read one map, and the amounts are powers of it.* Across
  cells, log b at −50 over log b at −100 is 0.17 wherever b at −100 is between 0.02 and
  0.6 (0.164 to 0.181 over six bins); −75 about 0.39, −25 0.06, −10 0.02, +50 −0.11, +100
  −0.19. The candidate's 1/(1 + k·d) makes that ratio vary fivefold over the same
  cells. Cells flattened most at −100 are steepened most at +100 (slope 1.95 where the
  slope at −100 is 0.02 to 0.05; 1.05 where it is 0.6 to 0.75).
- *The darkest cells keep a slope above 1.* In the darkest stop of a photo the cell slope
  is 1.07, 1.16, 1.28, 1.31 and 1.21 at −10 to −100, and falls below 1 only about five
  stops above the darkest cells; their log gain is linear in the amount (one photo: 0.20,
  0.51, 1.03, 1.54 and 2.05 stops). Haze added with one transmission per cell cannot
  raise a slope above 1.
- *The veil is not the airlight times the lost transmission.* With Camera Raw's own
  slope, the best single airlight per photo leaves 0.137 (no veil: 0.218), and a
  constant share of the photo itself blurred by 8% of the long edge leaves 0.037 (46 of
  52 photos better); at −50, 0.056 against 0.014.
- *In log luminance it is close to one filter for every photo.* Gain of each octave band
  of log2 luminance (1, 2, 4, 8, 16, 32 and 64 px of 1024, then the 128 px low-pass about
  its mean), median of 52 photos:

  | Amount | 1 | 2 | 4 | 8 | 16 | 32 | 64 | low-pass | r² |
  | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
  | −10 | 0.93 | 0.93 | 0.93 | 0.94 | 0.95 | 0.95 | 0.95 | 0.97 | 1.00 |
  | −25 | 0.81 | 0.82 | 0.83 | 0.85 | 0.86 | 0.86 | 0.88 | 0.93 | 1.00 |
  | −50 | 0.63 | 0.65 | 0.66 | 0.69 | 0.71 | 0.72 | 0.75 | 0.85 | 0.98 to 0.99 |
  | −75 | 0.44 | 0.47 | 0.51 | 0.54 | 0.57 | 0.59 | 0.62 | 0.78 | 0.95 to 0.98 |
  | −100 | 0.27 | 0.30 | 0.34 | 0.40 | 0.44 | 0.46 | 0.50 | 0.71 | 0.86 to 0.96 |
  | +50 | 1.35 | 1.32 | 1.29 | 1.23 | 1.20 | 1.19 | 1.16 | 0.99 | 0.95 to 0.99 |
  | +100 | 1.85 | 1.84 | 1.76 | 1.66 | 1.58 | 1.55 | 1.49 | 0.94 | 0.81 to 0.94 |

  The dark and the bright third of a photo have the same gains in the fine bands (−50:
  0.61 and 0.61 at 1 px), and photos differ little there (−50, 1 px: 0.55 to 0.68 from
  the 10th to the 90th percentile); in linear values the same bands give 1.09 to 1.27 in
  the dark third and 0.85 to 0.94 in the bright one. The finest band's gain is about
  1 + 0.7·s on both signs. Photos differ in the low-pass (−100: 0.25 to 1.01), and at
  +100 the bright third is boosted more (2.26 against 1.79).
- *The candidate against the same test.* Its bands at −100 are 0.27, 0.27, 0.31, 0.41,
  0.58, 0.66, 0.66 and 0.67, and at −50 0.55, 0.56, 0.58, 0.67, 0.78, 0.84, 0.86 and
  0.88: it keeps the contrast of everything larger than its window (1.5% of the long
  edge and up) that Camera Raw goes on lowering, which is the patchy fog, and at −50 it
  lowers fine contrast too far.
- *The detail filter alone is nearly exact.* Luminance only, median of 52 photos at
  −100 (−50): the candidate 0.0267 (0.0137); the median gains of every band with each
  photo's own mean level 0.0319 (0.0153), and with its own low-pass gain as well 0.0240
  (0.0104); the median gains of the bands finer than 32 px on top of Camera Raw's own
  32 px low-pass 0.0073 (0.0036), finer than 128 px on its 128 px low-pass 0.0131
  (0.0062). The best curve of a photo's 32 px low-pass leaves 0.0187 (0.0096): the
  large-scale layer is not a curve of its level either. The same detail filter on the
  candidate's own 32 px low-pass leaves 0.0261 (0.0130): nearly all of the candidate's
  luminance error is in the large-scale layer.
- *Colour follows from the luminance.* With Camera Raw's own luminance, scaling the
  pixel's colour by the luminance's gain leaves 0.0255 at −100. Keeping a share T/gain of
  the pixel's own colour and giving the rest one veil colour per photo leaves 0.0018 (a
  constant share: 0.0039). The share kept falls from 0.56 where the luminance is
  unchanged to 0.03 where it is lifted by more than 3.5 stops. The veil colour is near
  neutral on most photos and orange on sunsets and sodium-lit scenes (red 1.2 to 1.4
  and blue 0.2 to 0.6 of the luminance).

**The automatic black is keyed after Dehaze.** Six training photos rendered with and
without the profile's automatic black (DefaultBlackRender None) at default, −100, −50 and
+100 (24 renders). At each setting the render with the automatic black is a curve of the
one without (scatter 0.005 to 0.26 EV, as at default), but not the default pair's curve
(0.09 to 0.52 EV off at −100, 0.17 to 0.54 at +100). At +100 all six photos share one
curve (−1.3 to −1.9 stops at 2^−13, −0.55 at 2^−10, −0.26 at 2^−8: the darkest key's),
and the deepest shadows end up 0.2 to 1.5 stops lighter than at default where the
render without the automatic black leaves them unchanged; at −100 the curve moves up
with the hazed image's darkest level (one photo: 2^−12.5 to 2^−9.5) and stops growing
near 2^−8. RAWmakase keys its default black on the photo before Dehaze (positive
Shadows already moves the key; Dehaze does not). Every photo
reference has the automatic black and every mosaic probe was rendered without it, a
difference between probes and photos that had not been controlled. In the same renders
the level Camera Raw leaves unchanged at −100 is 1 to 3 stops under white and within a
stop of the photo's brightest large area; above it Camera Raw darkens (−0.25 stop).

**The large-scale layer.** With the detail filter and the colour rule in place, a simulated
operator on the 52 photos at −100 (all channels) leaves 0.0275 at the median on the
candidate's own 32 px low-pass (the candidate: 0.0293; six and four photos above 0.05),
0.0105 on Camera Raw's low-pass with the engine's airlight as the veil colour (two
photos above 0.05) and 0.0092 with the fitted veil colour (none). What is left is that
low-pass, the lift of the 32 px mean of log luminance (1.46 stops RMS at −100). Its
error in stops, RMS and median over photos:

| Large-scale layer | RMS | Median photo | Worst photo |
| --- | --- | --- | --- |
| The candidate's (haze model backwards) | 0.365 | 0.250 | 1.39 |
| Each photo's own best curve of the level (a floor for curves) | 0.197 | 0.157 | 0.55 |
| out = P·(1 − (1 − in/P)^n), P and n free per photo | 0.251 | 0.198 | 0.70 |
| The same with one n and P from a statistic of the photo | 0.47 to 0.59 | 0.32 to 0.54 | 1.5 to 1.8 |
| Scattered light with an airlight, (B + k·M) / (1 + k·M/A), M a mean over 32 px to the frame | 0.47 to 0.85 | 0.32 to 0.64 | 1.8 to 3.0 |
| Gradient boosting on coarse features, cross-validated by photo (98 photos) | 0.27 | 0.17 to 0.18 | 1.15 |
| The same on inputs relative to the airlight, no whole-photo statistics | 0.286 | 0.187 | 0.88 |

- *A learned layer closes part of the gap and stops improving.* Under the same detail
  filter and colour rule it leaves 0.0212 to 0.0230 at −100 (better than the candidate on
  40 photos, worse on 9) and 0.0104 at −50 (the candidate: 0.0146). Trained on 12, 24,
  42, 63 and 84 photos its median error on unseen photos is 0.239, 0.204, 0.196, 0.189
  and 0.180 stops; the dataset has 245 training photos. It fails on saturated scenes (a
  portrait before a yellow wall: Camera Raw's mean lift 0.6 stop, the learned layer's
  1.76), on low-key scenes (a moon on black: 0.03 against 0.45) and under-lifts large pale
  areas. Local dark-channel and density features carry a third of what levels leave
  (0.253 → 0.167); levels relative to the airlight instead of absolute ones change
  nothing.
- *Probes built from photos* (six training photos' scene values as probe DNGs with the
  automatic black, one change each, 96 renders; the lift at the same pixels against
  the unchanged photo). Colour removed at equal luminance: the yellow-wall portrait
  gains 1.10 stops, 1.50 on the wall, which the unchanged photo darkens by 1.28 and
  the grey one does not; half the saturation does the same as none; a night market
  +0.72, a sunset +0.15, fog photos 0.01 to 0.19. Saturation raised by half: the sunset
  −0.51 (−0.97 in its brightest stop), the night market −0.75. A pedestal of 2^−6: −0.08
  to +0.02 on daylight photos though their levels rose by 0.2 to 1.0 stop, +0.76 on the
  night market. The left half darkened by two stops: the other half −0.07 to +0.07
  (−0.16 to −0.31 in its darkest levels), the darkened half +0.2 to +0.9. Detail removed
  (a 16 px blur): +0.02 to +0.10 on four photos, +0.69 on the night market, −0.28 on a
  fog photo whose small white bird was the brightest neutral thing in it. Mid-tones
  darkened: +0.10 to +0.39.
- *What the probes say.* Saturated colours are kept out of the airlight for the whole
  photo, and what is brighter than the airlight is pulled down to it. In the portrait
  and the sunset one transmission fits the three channels at every level once the
  airlight is right: the engine's dim neutral airlight in the portrait, and in the
  sunset the sun's own colour from unclipped values (luminance 0.79 where the engine
  reads 0.31 from the clipped render). A rule trained on the other 92 photos predicts
  the grey versions of its worst photos well (0.17 stop against 0.60 for the portrait,
  0.12 against 0.45 for the night market): what the learned layer lacks is colour, not
  the luminance behaviour.
- *A law read from those scenes does not hold on the rest.* In the three scenes the
  transmission follows the luminance over the airlight's as 1/(1 + 1.45·d^0.6), on to
  five times the airlight. On all 52 photos a luminance density under that law, with
  blurs of 0 to 8%, exponents of 0.4 to 1 and three choices of airlight, is worse than
  the candidate's dark-channel density (best 0.0388 at the median against 0.0293; worse
  on 41 photos).
- *The veil colour.* Against the scene before the default white and black, unclamped
  and at full size: where no light source is clipped at the sensor the fitted veil
  colour is the brightest 1% by luminance within about 0.1 stop per channel (five of
  six such photos); around clipped sources it is two to three times more saturated
  than the brightest pixels, which RAWmakase reconstructs to white. A dusk city (blue
  0.41 of green in the veil, nothing bright that yellow) and two slightly magenta
  veils are not explained.
- *The automatic black does not explain the large-scale error.* Against the six photos
  rendered without it the candidate is no closer (0.0336 against 0.0287, 0.0467 against
  0.0349, and so on): it still lifts levels 6 to 10 stops under the top 1.2 to 1.8
  stops too far.

**Where this leaves negative Dehaze.** It is not the haze equation with one airlight. It
is a contrast change in log luminance that is nearly the same at every scale and for
every photo up to about 3% of the frame, on top of a large-scale layer that differs by
photo and by region, with a veil of one colour per photo. The detail filter and the
colour rule are measured; the large-scale layer has no closed form yet, and neither
they nor the automatic black are worth building for negative Dehaze before it has a
model. Open: how colour enters that layer (probes that change saturation and hue of
whole photos and of single regions), and the airlight Camera Raw reads around clipped
sources. The same band test on Clarity and on positive Dehaze, whose renders are on
disk, and the default black keyed after Dehaze are separate, smaller steps.

**The same band test on Clarity and positive Dehaze.** Camera Raw against main
(`f9b574b2`), each from its own default render, on 49 to 52 training photos; gain of
each octave band of log2 luminance (px of 1024, then the 128 px low-pass):

| Setting | Render | 1 | 2 | 4 | 8 | 16 | 32 | 64 | low-pass |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Clarity +100 | Camera Raw | 1.61 | 1.48 | 1.38 | 1.30 | 1.24 | 1.22 | 1.17 | 1.12 |
| Clarity +100 | main | 1.31 | 1.54 | 1.63 | 1.62 | 1.54 | 1.43 | 1.36 | 1.14 |
| Clarity +50 | Camera Raw | 1.31 | 1.25 | 1.19 | 1.16 | 1.13 | 1.11 | 1.08 | 1.06 |
| Clarity +50 | main | 1.17 | 1.28 | 1.33 | 1.32 | 1.29 | 1.23 | 1.19 | 1.08 |
| Dehaze +100 | Camera Raw | 1.96 | 1.89 | 1.84 | 1.70 | 1.60 | 1.56 | 1.50 | 0.95 |
| Dehaze +100 | main | 1.94 | 1.93 | 1.92 | 1.86 | 1.56 | 1.29 | 1.16 | 1.07 |
| Dehaze +50 | Camera Raw | 1.37 | 1.34 | 1.31 | 1.25 | 1.21 | 1.19 | 1.17 | 0.99 |
| Dehaze +50 | main | 1.43 | 1.43 | 1.43 | 1.38 | 1.27 | 1.15 | 1.10 | 1.07 |

- *Clarity.* Camera Raw's extra gain falls smoothly with size, about 0.61·px^−0.3 at +100
  and half of that at +50. Main peaks at 4 to 8 px, is 19% short at 1 px (its finest
  blur is 4 px of 1024) and 13% to 19% over from 4 to 64 px, and lowers the 32 px level
  of a photo's dark third by 0.31 stop where Camera Raw lowers it by 0.17. Strength is
  not the fault: main's own change at +35 to +100, applied to Camera Raw's default
  render, leaves 0.029 to 0.031 against Camera Raw's +100 at every amount (no change:
  0.036), and 0.015 against +50 (0.018). Nor is Clarity a filter with one gain per band:
  the median gains leave 0.0367, a photo's own gains 0.0338, and the median gains finer
  than 32 px on Camera Raw's own 32 px low-pass 0.0181. Half of Clarity is in its
  large-scale layer. (The step probes of 2026-11-12, cited here before as showing a
  dependence on the detail's size, were withdrawn on 2026-11-19.)
- *Positive Dehaze.* Main matches Camera Raw up to 16 px and is 16% to 21% short at 32
  and 64 px: its density follows structures larger than its window, the fault of the
  negative candidate. Camera Raw lowers the low-pass contrast (0.95; 0.78 in the dark
  third) where main raises it (1.07), and main darkens the 32 px level by 0.08 to 0.16
  stop more (the bright third −1.11 stops against −0.95). Main's own change leaves
  0.0358 at +100 (no change: 0.0941) and 0.0168 at +50 (0.0424); the median band gains
  finer than 32 px on Camera Raw's own low-pass leave as much (0.0353 and 0.0128), so
  unlike negative Dehaze its detail is not one filter for every photo.

### Shadows and Highlights together are not the sum of each alone (2026-11-17)

Every fit and every test of Shadows and Highlights so far used one slider at a time. The
dataset gate's mixed sets hold one combined case of the two, Shadows +50 with Highlights
−50; nothing renders negative Shadows or positive Highlights together with another
control. This experiment renders combined settings in Camera Raw on training photos and
asks two things: whether Camera Raw's combined render follows from its single-slider
renders, and how main and the branch of #1128 (the rule for negative Shadows and positive
Highlights) compare with it. 60 training photos, taken in turn from every family and
stratum, scene values through a linear profile at 1024 px; nothing fitted on validation,
holdout untouched. Lifts are of the 32-pixel level, in stops, by stops under the photo's
top (0–1, 1–2, 2–3, 3–4, 4–5, 5–6, 6–8).

**Camera Raw's own renders.** The lift at a combined setting less the sum of the two
single-slider lifts, mean over photos (at the two ends the RMS over photos is within 0.1
of the mean's size, so most photos show the same):

| Setting | 0–1 | 1–2 | 2–3 | 3–4 | 4–5 | 5–6 | 6–8 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Shadows +100, Highlights +100 | +0.25 | +0.36 | +0.43 | +0.45 | +0.47 | +0.48 | +0.47 |
| Shadows −100, Highlights −100 | −0.44 | −0.47 | −0.48 | −0.49 | −0.53 | −0.52 | −0.45 |
| Shadows +100, Highlights −100 | +0.23 | +0.13 | +0.04 | −0.03 | −0.10 | −0.16 | −0.21 |
| Shadows −100, Highlights +100 | −0.13 | −0.09 | −0.02 | +0.05 | +0.13 | +0.19 | +0.22 |
| Shadows −50, Highlights +50 | −0.09 | −0.05 | −0.01 | +0.02 | +0.06 | +0.09 | +0.14 |

- Moved the same way, the two sliders brighten or darken every level by nearly half a
  stop more than their sum. Moved opposite ways, each does less at its own end: with
  Shadows +100 and Highlights −100, the pair most edits use, Camera Raw pulls the top
  stop 0.23 less and lifts the deepest shadows 0.21 less than the two sliders do alone.
- At half of both sliders the difference is half to 0.7 of the full one, not a quarter:
  it scales with the sliders, like the single-slider lifts, and not with their product.
- Exposure does not enter: Camera Raw's Exposure −1 with Highlights +100 and Exposure +1
  with Shadows −100 are the sums of the single renders to 0.01–0.05 stop below the top
  stop (22 photos; in the top stop, where the white curve acts, the first differs by
  0.17). Highlights +50 with Whites +50 is the sum within 0.08. Blacks reads what Shadows
  leaves (Shadows −50 with Blacks −50: 0.02–0.43 RMS down to six stops under the top
  read that way, against 0.03–0.96 for the sum), as the engine already applies it.

**A form that fits it.** With both sliders set, each one's lift is a blend of its own
curve and a uniform lift at that curve's extreme:
`combined(L) = (1 − λs)·S(L) + λs·S_ext + (1 − λh)·H(L) + λh·H_ext`, where `S(L)` and
`H(L)` are the photo's single-slider lifts by level. Each slider protects the other end
of the tonal range less than it does alone. With the two fractions fitted per photo, the
RMS over a photo's levels of what the sum of the singles leaves falls (median of 59
photos) from 0.41 to 0.03 at Shadows +100 with Highlights +100, 0.44 to 0.05 at −100 with
−100, 0.11 to 0.02 at +100 with −100, 0.10 to 0.02 at −100 with +100 and 0.06 to 0.01 at
−50 with +50. The fractions are the same at half the sliders and a photo keeps its
fractions across the four sign combinations (correlation 0.5 to 0.99), but they differ
between photos: λs is 0.09 to 0.12 at the median of a setting (quartiles about 0.07 and
0.17), λh 0.19 to 0.33 (0.15 and 0.5). With the median fractions for every photo the
residuals are 0.17, 0.17, 0.10, 0.09 and 0.06: most of the same-sign difference goes,
little of the opposite-sign one. Of the descriptors tried, a photo's λh follows the mean
of its Highlights mask best (rank correlation 0.71); none explains λs.

**Over the two sliders' grid** (16 settings at ±50 and ±100, 16 photos): the same-sign
difference is 0.47 stop below the top stop at +100 with +100, 0.20–0.28 at +50 with +50,
0.12–0.23 at +100 with +50 and 0.14–0.18 at +50 with +100 (the negative quadrant mirrors
it: −0.5, −0.22…−0.30, −0.14…−0.22, −0.18…−0.22), so it is largest when the two sliders
are equal. In the opposite-sign quadrants each slider's own part follows its own amount:
the top stop moves 0.16–0.22 at Shadows ±100 and 0.05–0.11 at ±50 whatever the
Highlights amount, the deep shadows 0.22–0.28 at Highlights ±100 and 0.08–0.17 at ±50.

**How it sets in** (one slider at +100 and the other at a tenth, a quarter and a half of
it, same 16 photos; the form's two fractions fitted per photo, which leaves 0.003–0.035
stop RMS at every setting). A slider's fraction grows with the other slider up to where
the two are equal and stays there beyond; there is no jump at zero. Median fraction by
the ratio of the other slider to this one, 0.1 | 0.25 | 0.5 | 1 | 2 | 4 | 10:

| Fraction | 0.1 | 0.25 | 0.5 | 1 | 2 | 4 | 10 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| λs, same sign | 0.00 | 0.01 | 0.04 | 0.13 | 0.13 | 0.13 | 0.11 |
| λs, opposite signs | 0.02 | 0.05 | 0.09 | 0.12 | 0.11 | 0.10 | 0.11 |
| λh, same sign | 0.01 | 0.04 | 0.11 | 0.36 | 0.32 | 0.33 | 0.47 |
| λh, opposite signs | 0.06 | 0.15 | 0.25 | 0.30 | 0.25 | 0.22 | 0.21 |

With the same sign the fraction rises as the square of the ratio; with opposite signs
it rises faster, half of it by a quarter of the ratio. (The last λh of the same sign is
read from a 0.05 stop effect.) The test build below ramps both linearly to half the
ratio, which is neither.

**What did not explain it.**

- *One slider read at the level the other leaves* (either order, Camera Raw's own curves):
  leaves as much as the plain sum in every case.
- *One slider's rule reading the photo as the other left it* (the engine's measurement map
  after the first gain, the rule's statistics taken again; a test build, 24 photos):
  Highlights after Shadows improves −100 with −100 (0.040 → 0.020 against Camera Raw) and
  worsens the other three (−100 with +100 0.024 → 0.047); Shadows after Highlights improves
  +100 with +100 (0.069 → 0.049) and worsens +100 with −100 (0.021 → 0.026). Camera Raw's
  Highlights −100 alone does depend on the photo (it pulls the top stop 0.42 on the
  high-key third of 81 photos and 1.68 on the low-key third), but the combined render is
  not that dependence applied in sequence.

**The engines at combined settings** (RMS of cube-root differences of each engine's change
against Camera Raw's; median of 60 photos, or of 22 where marked; no change | main | the
branch of #1128):

| Setting | No change | Main | Branch | Photos better / worse |
| --- | --- | --- | --- | --- |
| Shadows −100, Highlights +100 | 0.0476 | 0.0228 | 0.0182 | 44 / 14 |
| Shadows −50, Highlights +50 | 0.0248 | 0.0124 | 0.0089 | 47 / 8 |
| Shadows +100, Highlights +100 | 0.1376 | 0.0666 | 0.0430 | 44 / 12 |
| Shadows −100, Highlights −100 | 0.0992 | 0.0326 | 0.0285 | 47 / 7 |
| Shadows +100, Highlights −100 (the first ensemble only) | 0.0514 | 0.0198 | 0.0198 | same render |
| Highlights +40, Shadows −40, Whites +20, Blacks −20 | 0.0465 | 0.0158 | 0.0140 | 36 / 18 |
| Highlights +50, Whites +50 (22) | 0.0784 | 0.0179 | 0.0188 | 12 / 8 |
| Shadows −50, Blacks −50 (22) | 0.0908 | 0.0119 | 0.0075 | 20 / 2 |
| Exposure −1, Highlights +100 (22) | 0.0626 | 0.0165 | 0.0127 | 19 / 2 |
| Exposure +1, Shadows −100 (22) | 0.0776 | 0.0170 | 0.0114 | 15 / 6 |

The branch is closer than main wherever its curves are used, and level with it beside
Whites. Both add the two sliders' gains, so both carry the difference above: at Shadows
+100 with Highlights +100 the branch is 0.28 to 0.38 stop too dark at every level (main
0.41 to 0.54), at −100 with −100 0.28 to 0.52 too bright, and at +100 with −100 the
render is 0.32 too dark in the top stop and 0.48 too bright six to eight stops down. The
errors at combined settings are two to six times the single-slider ones (0.007 to 0.011
on the branch).

**A test build of the form** (not a candidate; a switch on the branch of #1128, no
retraining): with both sliders set, each scaled rule curve is blended toward its value at
the photo's 1st (Shadows) or 99th (Highlights) percentile base level, with λs 0.11 and
λh either 0.25 or read from the photo's mean Highlights mask (1.56 × mean − 0.23, held
to 0.1–0.9, the line through the 59 photos' fitted fractions), both ramped in over the
other slider's first half of this one's amount. The constants come from these same 60
photos. Median error against Camera Raw, main | the branch | λh fixed | λh from the mask:

| Setting | Main | Branch | λh 0.25 | λh from the mask | Better / worse than the branch (mask) |
| --- | --- | --- | --- | --- | --- |
| Shadows +100, Highlights −100 | 0.0198 | 0.0198 | 0.0182 | 0.0147 | 47 / 11 |
| Shadows −100, Highlights +100 | 0.0228 | 0.0182 | 0.0192 | 0.0163 | 31 / 23 |
| Shadows +100, Highlights +100 | 0.0666 | 0.0430 | 0.0291 | 0.0283 | 42 / 18 |
| Shadows −100, Highlights −100 | 0.0326 | 0.0285 | 0.0188 | 0.0189 | 41 / 16 |
| Shadows −50, Highlights +50 | 0.0124 | 0.0089 | 0.0102 | 0.0085 | 27 / 24 |
| Highlights +40, Shadows −40, Whites +20, Blacks −20 | 0.0158 | 0.0140 | 0.0131 | 0.0128 | 34 / 19 |

With the mask's fraction the level's bias at Shadows +100 with Highlights +100 falls from
−0.28…−0.38 stop to −0.06…+0.09, at −100 with −100 from +0.28…+0.52 to −0.07…+0.07, and
at +100 with −100 from −0.32 in the top stop and +0.48 six to eight stops down to −0.13
and +0.18. The fixed fraction is worse than the branch on 36 of 60 photos at Shadows −100
with Highlights +100: the fraction has to follow the photo. A first build that blended
toward the extreme of the curve's table instead of the level the photo reaches overshot
by half a stop, because the Shadows −100 table runs far below the photo's pixels.

On validation the gate's one combined case of the two, Shadows +50 with Highlights −50,
is the only case the switch changes (the other 504 of the mixed sets render the same).
With λh from the mask, against the recorded baseline of main (56 photos of six families,
median ΔE00 of a photo): median 1.04 → 0.94, mean 1.26 → 1.20, 90th percentile 2.13 →
2.26; 26 photos closer by more than 0.05 and 14 further. In the three main families
0.96 → 0.87 (90th percentile 2.27 → 2.09), in the stress families 1.20 → 0.95 (1.90 →
2.25). Four photos have windows more than 2 ΔE00 worse (355 windows; 81 on three photos
are that much better), the largest a stress photo at 1.14 → 2.46; clipping does not
move. The form carries over to photos it was not read from, at half the sliders; the
fraction per photo does not yet, and the build would not pass the gate's regression
rule.

**The candidate (#1153, stacked on #1128; both merged 2026-11-18).** The blend with the fractions predicted
per photo and sign combination from the photo's own two curves: how much of the photo
each reaches (the mean over the map of its gain as a fraction of the extreme's), the
extremes' sizes and their reciprocals, the extremes read at the 1st and 99th percentile
of the base levels. A linear map, fitted on the four ±100 pairs of 212 training photos
on what the blend leaves of Camera Raw's combined gain in the engine's own terms (its
curves from its dump, its white curve undone). Histogram descriptors added to those six
numbers gained nothing on held-out photos. A slider's fraction ramps in as the ratio's
power 1.7 for sliders of one sign and as 1 − e^(−2.4 × ratio), scaled to 1 at equal
sliders, for opposite signs. Held out by photo in five folds, the gain's error at each
pair (RMS over a photo's pixels in stops, median; the sum of the two curves, then the
blend): Shadows +100 with Highlights +100 0.318 → 0.205 (166 photos closer by 0.02, 20
further), −100 with −100 0.359 → 0.231 (173 / 19), +100 with −100 0.197 → 0.174
(104 / 8), −100 with +100 0.220 → 0.192 (129 / 7); the fractions fitted to each photo
would leave 0.154, 0.184, 0.157 and 0.173, about the two single curves' own error (0.15
to 0.16 for Shadows, 0.09 to 0.10 for Highlights).

Rendered (median error against Camera Raw on the 60 photos; #1128, then the candidate):

| Setting | #1128 | Candidate | Photos closer / further |
| --- | --- | --- | --- |
| Shadows +100, Highlights −100 | 0.0198 | 0.0148 | 50 / 5 |
| Shadows −100, Highlights +100 | 0.0182 | 0.0145 | 41 / 16 |
| Shadows +100, Highlights +100 | 0.0430 | 0.0262 | 45 / 14 |
| Shadows −100, Highlights −100 | 0.0285 | 0.0151 | 47 / 10 |
| Shadows −50, Highlights +50 | 0.0089 | 0.0078 | 36 / 21 |
| Highlights +40, Shadows −40, Whites +20, Blacks −20 | 0.0140 | 0.0122 | 33 / 22 |

The last one's 90th percentile grows (0.0246 → 0.0275): with Blacks lowered too the deep
shadows end 0.3 to 0.4 stop too bright, 0.1 more than before. Over the grid on 16 photos
the candidate is closer at all twelve other settings of ±50 and ±100 (Shadows +50 with
Highlights −50 0.0101 → 0.0075, 14 closer and 1 further; +50 with +50 0.0287 → 0.0114)
and level where one slider is a tenth or a quarter of the other (0.0095–0.0151 →
0.0098–0.0132). One slider alone renders as before.

On validation, Shadows +50 with Highlights −50 against the recorded baseline of main (56
photos of six families, median ΔE00 of a photo): median 1.04 → 0.85, mean 1.26 → 1.12,
90th percentile 2.13 → 2.03; 39 photos closer by more than 0.05 and 3 further (1.14 →
1.59, a backlit hut whose shadows the blend leaves a little dark; 1.86 → 2.19; 0.68 →
0.79). 210 windows on seven photos are more than 2 ΔE00 closer and 4 windows on one
photo further; that photo, itself closer (0.39 → 0.31), fails the gate's regression rule
for its stratum: 129 of the mixed sets' 180 setting and stratum pairs pass, 1 fails, 50
have too few photos. The other 504 cases render as before. The synthetic chart's three
cases with both sliders get closer (mean ΔE00 0.85 → 0.68, 2.58 → 2.00, 2.51 → 2.45).

**Found in its review, and older than it:** the sum of the two curves is not always
monotone in the base level. Each curve is made monotone on its own; their sum never was.
On a synthetic scene of a deep-shadow block in a textured surround, Shadows +80 with
Highlights −60 renders five pairs of neighbouring table levels in reversed order, by up
to 0.051 stop, and +100 with −100 nine, by up to 0.128; +50 with −50 none. The blend
halves it (one pair and 0.015; six and 0.070) and cannot add to it. Not fixed: a
projection of the summed table cannot be applied per pixel in a mask, and the two paths
have to stay identical.

**Open.** Combined cases with both signs of each slider in the gate's mixed sets:
validation rests on one case at half the sliders. The reversal above. HDR masters, which
keep the sum. The deep shadows under the pair with Blacks lowered.

### Negative Dehaze as added haze, and the dark key (2026-11-13)

Follow-ups of the round below, on Camera Raw's scene values of the training photos (98
photos of the three families' `dehaze-train` and `clarity-train` sets, read through a linear
profile and reduced to 2048 px; nothing fitted on validation).

**Negative Dehaze: the haze model run backwards (withdrawn 2026-11-14; see the section above).**

- *What does not predict it.* Per photo, Camera Raw's gain at −100 is close to one curve of
  the level (0.225 EV RMS; with a blur of 6% of the long edge 0.145) whose position moves by
  about a stop from photo to photo. Percentiles, the mean, the airlight and blurred maxima
  do not predict that position. The Shadows/Highlights rule's 24 histogram descriptors and
  network, cross-validated by photo, do no better than the level under the 99th percentile
  (0.510 against 0.475 on 94 photos). A table of the channel's level under the maximum of
  blurred luminance and of the local level cross-validates at 0.354 (per channel; main
  0.42), but in the gate its median at −100 stayed at 5.88 with 9 of 29 photos worse.
- *Probes built from photos.* Three training photos' scene values written as probe DNGs in
  13 variants each (78 renders). The gain curve at −100 keeps its place relative to the
  photo when the data is scaled by −1 or −2 stops, and when the frame is flipped or doubled
  in size. A bright patch of 1% of the frame moves it by +0.53 stop on a photo with nothing
  near white (5%: +0.64, 20%: +0.75); on photos that already reach white a patch adds little
  at 1% and up to +0.47 at 20%, far more in the darks (+1.6 at −9 stops). A dark patch does
  nothing. Removing colour moves it by +0.07 to +0.26, more in the darks. Blurring by 8 px
  moves it by up to +0.83, shuffling 64 px blocks by −0.13 to +0.38.
- *The model.* That is the dark-channel haze model with a transmission above 1:
  J = (p − A) / t + A, t = 1 + k·density, with the airlight and density positive Dehaze
  already measures. One k for all photos fits −100 to 0.31 EV RMS (median of 98, per
  channel; k free per photo 0.28, all three constants free per photo 0.21). The fit wants
  no exponent on the density (0.98) and no scaling of the airlight (0.98). The engine's
  window (0.5% of the long edge), guided filter and airlight percentile fit best: no guided
  filter 0.40, double the window 0.34, four times 0.40, the haziest 0.1% or 5% 0.33 and
  0.34. k at −10, −25, −50, −75 and −100 is 0.22, 0.67, 1.79, 3.64 and 6.71:
  e^(2.05·|s|) − 1 within 1% (`scripts/corpus/scene-dehaze.py`).
- *Gate* (Dehaze validation sets against `main-0a711ba5-metric2`, median display ΔE00): main
  families, 29 photos: −100 5.88 → 2.90 (mean 7.36 → 3.45, 90th percentile 13.51 → 5.58; 27
  better and 1 worse by more than 0.3), −50 2.93 → 1.71 (26 and 1), −25 1.50 → 1.18 (14 and
  2). Stress families, 24 photos: −100 7.51 → 3.56 (19 and 3), −50 3.46 → 2.20 (17 and 4),
  −25 1.81 → 1.27 (13 and 2). Default and positive Dehaze are unchanged. HDR family,
  validation, mean ΔE ITP: Dehaze −30 11.73 → 6.68, 8.91 → 6.80, 5.11 → 5.11, 5.98 → 7.10;
  −50 better on all six photos (19.44 → 8.91 the largest).
- *What fails.* `accept` fails on the photo rule for three photos: a moonrise over a harbour
  (−100 3.79 → 11.64, −50 2.28 → 6.10), a lit sign at night (stress, −50 4.99 → 7.36) and a
  portrait (stress, −100 5.74 → 9.20); on single regions of photos that improve overall;
  and on clipping (+0.5 point) for three stress photos. On the moonrise the airlight is the
  moon's orange and the whole frame is veiled in it, where Camera Raw's veil is weak and
  the sky's blue. Two night frames and a dark DNG among the training photos fail the same
  way (Camera Raw lifts the bulk 0.3 to 0.5 stop, the model 1.4 to 2). Tried on the training
  photos without effect: a veil fading toward black (four forms; the fit sets the fade to
  nothing), the density capped by or blended with the pixel's own darkest channel, the
  density clipped at 1 and no darkening, a strength predicted from one or two statistics of
  the photo (leave-one-out mean 0.357 against 0.384, worst photo still above 1.1), and an
  airlight from a larger window (2% to 10% of the long edge: median 0.40 to 0.53). The
  synthetic scene probes at Dehaze −40 move both ways against their recorded errors (mean
  absolute error 0.0105 → 0.0144, 0.0065 → 0.0052, 0.0089 → 0.0117).

**The dark key (#471): identified on probes, a wash on photos.** 58 probe renders: Camera
Raw's default black follows the 0.01th percentile (index ⌊n·10⁻⁴⌋) of the mean luminance of
the cells of a copy 128 px on the long edge. In a 1024² frame one dark 8 px cell does
nothing and two, adjacent or far apart, move the black fully; in 512 × 1024 one is enough;
in 2048² the cell is 16 px; partly dark cells count by their linear mean; a spot dark in
red only does nothing. With that key the probes below −4 stops go from 0.30 to 0.02 EV RMS.
On 61 training photos at default it changes little: −11 to −9 stops 0.251 → 0.242 EV, −9
to −7 0.088 → 0.080, black share against Camera Raw's 0.16 → 0.24 point (about 8 photos
better, 5 worse), and several photos stay 0.8 EV off with either key. Not taken further.

### Tone experiments on current main (2026-11-12)

Seven bounded Camera Raw 18.7 probes against main `0a711ba5`, planned from the open
tone issues (#464, #467, #468, #470, #471, #474, #600). Probes are synthetic DNGs read at
the scene stage; photo numbers are the validation sets; holdout was not touched. One
engine change came out of it (positive Blacks, #1068); the rest are measurements and refuted
hypotheses, recorded so that they are not tried again. The scripts and reduced
renders are local; no Camera Raw render of a photo is in the repository.

**1. Error budget on current main.** The validation gate over all families (79 sets,
145 photos; `main-0a711ba5-metric2`, which replaces `main-a8c5d6d7-metric2` as the
baseline) with each case split into the default render's error and the control's own
(ΔE00 against Camera Raw with the default render's per-pixel Lab error carried over).
Median over photos, all families:

| Setting | Total | Default | Control's own | Own p90 | Own ΔL\* by band of Camera Raw's L\* |
| --- | --- | --- | --- | --- | --- |
| Dehaze −100 | 6.84 | 0.31 | 6.56 | 13.6 | 0–50: +3.4 to +4.2 |
| Dehaze −50 | 3.39 | 0.31 | 2.92 | 5.3 | 0–40: about +1 |
| Dehaze +100 | 2.38 | 0.31 | 2.23 | 4.8 | 10–70: −1.3 to −2.7 |
| Clarity +100 | 2.26 | 0.47 | 1.94 | 3.1 | 10–60: about −1.5 |
| Blacks +100 | 1.95 | 0.51 | 1.70 | 2.2 | 0–20: +1.5 to +1.9 |
| Dehaze +40 with Clarity +40 | 1.93 | 0.45 | 1.66 | 2.3 | 70–90: +1.2 to +1.4 |
| Exposure −1 with Whites +100 | 1.66 | 0.45 | 1.55 | 3.8 | 30–90: +2.0 to +3.6 |
| Shadows +100 | 1.82 | 0.53 | 1.32 | 3.4 | 0–20: +1.1 |
| Whites +100 | 1.24 | 0.54 | 1.01 | 3.1 | 50–90: −1.2 to −2.2 |
| Shadows −100 | 0.73 | 0.53 | 0.59 | 1.9 | 10–40: +1.5 to +2.3 |
| Highlights +100 | 0.99 | 0.36 | 0.67 | 2.1 | 70–90: −1.4 to −2.3 |
| Highlights −100 | 0.72 | 0.36 | 0.42 | 0.9 | within 1 |
| Exposure ±1, ±2 | 0.25–0.56 | 0.44 | 0.20–0.38 | 0.8–1.4 | within 0.5 |

Negative Dehaze is the largest gap by far (2.6 to 14.0 by family and brightness at
−100), then positive Dehaze and Clarity +100 (both worst on bright photos). The
default render sits at a median of 0.34 to 0.47; Camera Raw has more pixels at black
than RAWmakase on low-key photos (3.62% against 2.61%).

**2. Blacks, Exposure and the default black (#474, #470, #471).** Neutral ramps with
dark keys −14 to −8 and DefaultBlackRender Auto and None; mosaics with a dark tail;
dark spots of 2 to 32 pixels.

- *Positive Blacks: confirmed, proposed in #1068.* Camera Raw's Blacks +25 with the default black
  equals the render without one, and +100 equals +100 without one, at every key:
  positive Blacks works on the scene value. RAWmakase read `blacks.bin` after the
  photo's own default black though it was measured after a key −8 ramp's, so photos
  with darker keys were 0.3 to 1.0 EV too light two to four stops above their key. From +25 up the
  change reads curves measured without a default black on the value before it; below
  +25, where the default black fades out unevenly and by key, it goes from the photo's
  default black to the value without it by the share the key −8 row has gone (0.03 to
  0.09 EV RMS on a key −12 ramp, where main is 0.03 to 0.34). On the
  ramps the error falls from 0.33–0.52 to 0.008–0.010 EV RMS from +25 up, and on the
  50 photos of the six Blacks validation sets the median ΔE00 goes 1.95 → 1.23 at
  +100, 1.17 → 0.71 at +50 and 0.95 → 0.50 at +25 (23 to 25 photos better by more
  than 0.3, none worse; the darkest band's own error +1.9 → +0.2 L\*). One low-key photo
  (a 5DS R) fails the gate's clipping rule at +50 and +100 while its ΔE00 falls from 2.80
  to 0.92: Camera Raw keeps 3.3% of it at black at +100 and RAWmakase 0.05%, the same
  disagreement it has at default (12.5% against 7.9%), which the old over-crushed
  path had partly hidden; the gate's accept therefore does not pass, and the change waits
  on that decision. Profiles without a default black are not changed: on None
  ramps `blacks.bin` is 0.9 EV off at +50 and +100, but rendered files use that path
  and were matched to Lightroom with it.
- *Positive Exposure: refuted.* On the same ramps RAWmakase is within 1 to 3% of Camera
  Raw at +1 and +2 at every key. What is left of #470 is not Exposure's response to the
  default black.
- *A black clip threshold: refuted; the key differs instead.* Camera Raw's default
  render reaches 0 only at the frame's darkest level, and RAWmakase reaches 0 earlier,
  not later. But Camera Raw's dark key is not a pixel percentile: one dark spot of 16
  pixels in a 1024-pixel frame (0.02% of it) moves the default black fully, a spot of 8
  pixels or less does not move it at all, and a spot at 2^-10 or 2^-9 moves it part of
  the way. That is a minimum over cells of about 16 pixels (a copy of about 64 pixels
  on the long edge). RAWmakase's 0.1th percentile of the 512-pixel map ignores such a
  spot and renders the level above it 1.5 EV darker; on textured photos it reads darker
  than a cell minimum would, which fits Camera Raw's larger share of black pixels at
  default. Not changed: it moves every photo's default render, and the cell size needs
  spots of 10 to 14 pixels and an offset spot first.

**3. Contiguity of the Shadows/Highlights rule: refuted.** One histogram arranged as
scattered patches of 4 to 128 pixels, as a block and as a strip, for a filled
histogram, a sky over ground and a dark frame with a small bright area (72 renders).
Contiguous and scattered arrangements of large patches differ by at most 0.35 EV in
Camera Raw (the mid levels beside the bright set), and main's engine is within 0.09 to
0.16 EV RMS of Camera Raw on patches of 32 and 64 pixels, the block and the strip. What
differs is the amplitude of small structures: at Shadows +100 the darkest stop of the
filled histogram is lifted 0.81 EV in 4-pixel patches, 1.14 at 8, 1.56 at 16, 1.81 at 32
and 1.95 at 128; at Highlights −100 the bright area is pulled −1.6, −2.9, −4.5 and −4.7 EV
at 4, 8, 16 and 32 pixels. Main is 0.2 to 0.5 EV RMS off there. Feeding the shipped rule
a low-passed map (box radius 0 to 64 map pixels, Gaussian 0.5 to 4, in linear or log
values, as statistics and level or level only, and guided filters of ε 1 to 8) never
brings the small sizes under 0.2 EV without losing the large ones (best overall
0.17 / 0.28 EV for Shadows / Highlights against 0.23 / 0.26 unblurred). Camera Raw still
tells 4-pixel bright and dark patches apart, so it is not a blur of the base. No
retrain.

**4. Mosaics for the controls on fitted tables.** Forty mosaics of the rule's families
at Whites +100, Shadows −100 and Highlights +100 (DefaultBlackRender None, scene
values), gain by stops below the frame's top, RAWmakase against Camera Raw:

- *Shadows −100* is far too weak in deep tones: 6 to 7 stops down Camera Raw −2.26 EV
  and RAWmakase −1.76; 8 to 9 stops down −4.38 against −1.44; overall 1.83 EV RMS
  where doing nothing is 2.45. Camera Raw's own spread across histograms at one depth
  is 0.5 to 1.4 EV, so it is a per-histogram curve like the positive side.
- *Whites +100* stretches the mid-tones 0.1 to 0.2 EV too far (0.34 EV RMS; 0.73 on the
  sky scenes), which is the sign of the photos' +2 to +3.6 L\* at Exposure −1 with
  Whites +100.
- *Highlights +100* is the closest: 0.25 EV RMS, 0.1 to 0.2 EV short within three stops
  of the top; the small-bright-minority mosaics are worst (0.44).

Not fitted in this round: each needs the full mosaic families and the training photos
at the new setting, as the positive Shadows rule did. Negative Shadows comes first.

**5. Negative Dehaze on tinted mosaics (#467): answered, not fitted.** Five histograms
in four tints at −50 and −100, plus texture variants. On mosaics Camera Raw's response
is a function of the histogram and the haze colour: neutral, blue and warm casts give
the same luminance response, the veil takes the colour of the brightest patches
(correlation 0.99, also when only the top stop and a half is tinted), and its size
follows the frame's mean or median luminance (log correlation 0.97 and 0.95; the 99th
percentile 0.92). Blob texture of 0.05 to 1.0 stop changes nothing (mean gain 1.63 to
1.68 EV). But RAWmakase's curves are already within 0.16 to 0.42 EV RMS of Camera Raw on
these mosaics, and a little darker, while on photos they are 6.6 ΔE00 off and too
light. Flat-patch mosaics therefore do not reproduce what goes wrong on photos, and a
fit to them would not help. An affine veil per channel does not describe the mosaics
either (0.3 to 0.4 EV RMS residual): the bright end is held and everything below it is
lifted toward it. Next: mosaics built from the failing photos' own histograms, to see
whether the histogram or the spatial structure carries the photos' response.

**6. Clarity +100 on sinusoids and steps (#468): gain depends on amplitude.** (The step
reading here is withdrawn: the four steps shared one frame and leaked into each other; see
[Clarity, 2026-11-19](#clarity-a-slope-around-a-base-and-no-closed-form-for-either-yet-2026-11-19).) Step
edges of 0.05, 0.2, 1 and 2 EV in one frame: Camera Raw's overshoot beside the edge is
0.63, 0.45, 0.14 and 0.00 of the step at +100 (half that at +50), with a halo wider
than 128 pixels; RAWmakase's is 0.21 to 0.27 at every step size, 26 to 85 pixels wide.
Sinusoids of 0.5 to 2 EV: Camera Raw leaves periods of 32 pixels and more almost alone
(gain 0.94 to 1.17) and boosts fine ones (1.4 to 1.8 at 8 pixels, 1.4 to 2.1 at 4),
where RAWmakase applies 1.2 to 1.45 at every period up to 256. Camera Raw's Clarity is
therefore a compressive detail gain (small differences boosted most, two-stop edges
not at all), which the linear four-scale model cannot express; that is the 0.03× to
1.3× spread by photo. Smooth sinusoids of 0.05 and 0.2 EV are degenerate (their flat
crests are pushed to white, as flat maxima are for Shadows), so the small-amplitude
gain needs textured gratings. No change yet: it needs a new operator.

**7. Positive Dehaze halos with lens corrections (#600): refuted.** On the reporter's
ARW at Dehaze +100, the bright rim beside strong edges (the gain 2 to 8 pixels from the
edge less the gain 24 to 48 pixels away, on the bright side, at 1024 pixels) is +0.25 EV
in RAWmakase with lens corrections off and +0.22 with them on, against +0.07 and +0.08
in Camera Raw; on the dark side Camera Raw darkens the band by 1.4 to 1.5 EV more than
its surroundings and RAWmakase by 0.7 to 0.8. Lens corrections are not the cause; the
density filter is.

### The global rule (2026-10-28)

Measured on flat-patch mosaics with controlled histograms instead of fitting tables on
photos ([scene tone stage](../scene-tone-stage.md#the-global-rule)). The earlier models
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
if it clipped only after demosaicing ([demosaicing](../demosaic.md)). Development now
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
estimate and the colour map ([rendering quality](../rendering-quality.md)).

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
[comparison contract](../scene-tone-stage.md#comparison-contract) for the
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
table](../scene-tone-stage.md#local-operators)); −50 and −25 still hold,
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

- **Saturation** −100 renders Camera Raw's exact luminance gray ([saturation](../color-mixer.md#saturation)): 0.65 mean ΔE00 on the chart, down from 3.97. Between −50 and +100 it is the photo-measured tables (chart: −50 1.21, +50 1.39, +100 2.93); Camera Raw's own blend toward luminance fits the chart better but was worse on real photos. **Vibrance** follows Camera Raw's chart measurements beyond ±50 ([vibrance](../color-mixer.md#vibrance)): −100 at 0.83 and +100 at 1.00 on the chart, down from 3.90 and 1.54; up to ±50 it is the photo-measured tables.
- **Color mixer** bands are fitted to Camera Raw 18.7 on a dense synthetic chart ([chart tables](../color-mixer.md#chart-tables)): the chart's single-band cases sit at mean ΔE00 0.64–1.31 (default render 0.66). Only Blue and Purple were rendered at ±50; the other bands use the same Luminance slider curve, measured on those two. Colors beyond sRGB keep the earlier photo-measured tables, and several bands combine by adding their changes (`mixer-alternate-luminance`: 1.42).
- **Color grading** follows Camera Raw 18.7 at every hue, saturation, Luminance, Blending and Balance ([color grading](../color-mixer.md#color-grading)): the chart's `grading-*` cases sit at mean ΔE00 0.96–2.02 (default render 0.91), down from 1.88–4.15, and looks with split toning at 1.05–2.17, down from 1.62–3.46. Strong tints at Blending near 100 are the furthest off: Shadows and Highlights at Saturation 60 and Blending 100 leave 2.5 ΔE00 over Camera Raw's own render, where Camera Raw stops a channel's gain from falling further than the fitted curves do. RAWmakase clips colors the tint pushes out of sRGB per channel, as Camera Raw does, which brings the `grading-*` cases to 0.56–1.43.
- **Targeted Adjustment Tool** works on the Tone Curve's parametric regions, the Color Mixer's Hue, Saturation and Luminance, and the B&W mix, sampling the color where each control sees it ([color mixer](../color-mixer.md#targeted-adjustment)). How a drag is shared among bands follows RAWmakase's render, not a measurement of Lightroom's tool; its drag rate and History names are RAWmakase's own, and in point-curve mode it moves the parametric regions where Lightroom drags a point on the curve.
- **Black & white mix** follows Camera Raw 18.7's renders of the synthetic chart ([chart gray](../color-mixer.md#chart-gray)): the chart's single-band `bw-*` cases sit at mean ΔE00 0.51–0.82 (default render 0.66), down from 1.1–2.5, and the Auto mix at 1.75, down from 2.30. A monochrome look at Amount 200 is a little further off (2.50, was 2.41). No real photos with Camera Raw black & white renders were available to check it on.
- **Auto black & white mix** is fitted to Camera Raw 18.7's Auto on synthetic scenes and is within 2 slider steps of it on the chart it was not fitted to; on 30 held-out scenes it is off by 1.3 steps on average (worst band 10). Scenes of two flat colors are predicted worst (up to 30 steps off on a band). It has not been compared with Lightroom on the same real photos.
- **Point Color** imports, renders, is written back and has Lightroom's Point Color tab with the dropper, range handles and Visualize Range ([color mixer](../color-mixer.md#point-color)). Fitted to Camera Raw 18.7 on synthetic charts, the 16 `pc-*` cases on the chart sit at mean ΔE00 0.83–1.26 (the default render is 0.91), down from 1.2–4.4 with Point Color left out. RAWmakase clips colors brightened past white per channel, as Camera Raw does, turning oranges towards yellow (`pc-orange-lum+50`: 1.82 → 0.76; all `pc-*` cases 0.69–1.11). The range shapes and Variance's effect on saturation and lightness are fitted, not exact. The sliders show the stored values × 100 (Range 0–100), and the dropper's neutral and dark limits are RAWmakase's own: neither was compared with Lightroom's panel.
- **Camera Calibration** primary sliders follow Camera Raw 18.7 ([camera calibration](../rendering-quality.md#camera-calibration)): the chart's `calibration-*` cases sit at mean ΔE00 0.62–0.68 (default render 0.66), down from 1.11–2.02. Measured on one synthetic camera; combinations of several sliders were checked on one pair only. Shadow Tint is still the earlier approximation.
- **Temp and Tint far from the as-shot white** follow Camera Raw more closely since #1148 (2026-11-17), which scales the white balance gains so that no raw channel darkens, as the DNG SDK does, and adapts the chosen white to D50 by Bradford for profiles without forward matrices. Scored at the commit before it and at its merge (2026-11-18): on the synthetic chart against Camera Raw 18.7, mean ΔE00 at Temp 2500 K 2.83 → 1.57 (the tungsten chart 1.69 → 0.52), at 12000 K 1.77 → 0.44 (2.54 → 1.41) and at Tint −80 1.27 → 0.42; Temp 3200, 5000 and 8000 K, Tint ±30 and Tint +80 render the same, as do the other 399 chart cases (the matrix-only default 0.245 → 0.244). On the dataset gate's white-balance sets (32 validation photos at Daylight, Tungsten, Fluorescent, 3200, 5000 and 8000 K and Tint ±30, profiles with forward matrices) it changes almost nothing: seven of the nine settings render identically, two photos get closer at 8000 K (one by 0.52 median ΔE00) and two at Tint −30 (by 0.06 at most), none further. For most whites green is the channel that limits the scale, and there the gains are what they were. The gate holds no photo below 3200 K, above 8000 K or past Tint ±30, where the change acts; the chart's baseline entries for the three improved cases were not tightened.
- **Camera exposure offsets** come from a table with a row per body, 180 bodies across 10 makes, so RAWmakase no longer needs a measured offset per camera to be close: a camera without a row falls back to the median of its own make and then of the whole table, which leaves a leave-one-out error of 0.14 EV on average against 0.29 EV for no baseline at all. A DNG's own BaselineExposure is used instead and the table is never used for one. Fujifilm rows hold at DR100 and base ISO, with the maker-notes DR shift added so DR200 and DR400 work. Without a baseline RAWmakase renders most cameras about 0.3 EV darker than Lightroom, for Sony, Canon and Nikon alike. X100F renders are still about 0.04 EV brighter in midtones; see [cameras](../cameras.md).

## RAW processing and profiles

- **Demosaic, highlight reconstruction, noise reduction and sharpening** are not Adobe's algorithms; sharpening and colour noise reduction are fitted to Camera Raw's ([rendering quality](../rendering-quality.md)). Colour noise reduction measures the photo's noise from its smoothest areas, where Camera Raw uses the camera's noise profile; a photo with no smooth areas reads as noisier than it is. At 100% the detail error is about 0.007 on X100F. X-Trans uses 1-pass Markesteijn, which measures the same as 3-pass.
- **AI Denoise** is not Lightroom's Denoise: it runs a different model (RawNIND UtNet2, [AI Denoise](../rendering-quality.md#ai-denoise)). Its Amount blends as Camera Raw 18.7's does and its switch sets manual noise reduction as Camera Raw's, but the result differs: close at ISO 12,800–20,000, behind at ISO 32,000 and above (colour blotches in dark areas, less detail) and on X-Trans (softer). A Lightroom edit with Denoise imports from its XMP sidecar with AI Denoise at its Amount ([XMP presets](../xmp-presets.md#compatibility-checks)), not yet from a Lightroom catalog; Raw Details, Super Resolution and flattened AI settings do not.
- **DCP support** is a bounded subset. Triple-illuminant, HDR and other unsupported profile structures are rejected. Enhanced XMP looks (Adobe Color etc.), creative HSV looks (B&W 01, 03–12, Modern 01), RGB-table looks (Artistic, Vintage, Modern, camera-matching XMPs that carry their table), and looks carrying Exposure, Saturation, colour mixer, parametric curve, split toning or vignette settings ([settings inside looks](../lightroom-profiles.md#settings-inside-looks)) are supported; adaptive/AI profiles, camera-matching XMPs whose table Camera Raw keeps elsewhere (Fujifilm film simulations) and looks with settings RAWmakase doesn't apply inside a profile (B&W 02's white balance, the B&W filters' mix, the Premium film looks' colour grading) are not. RGB tables match Camera Raw 18.7 on synthetic looks within RAWmakase's default-render error ([RGB tables](../lightroom-profiles.md#rgb-tables)). `.cube` LUTs import as RGB-table looks, as Camera Raw's Create Profile makes them ([.cube LUTs](../lightroom-profiles.md#cube-luts)); how Create Profile converts 65-division cubes, domains other than 0–1 and shapers, and render parity on photos, are not measured yet. Profile Amount follows Camera Raw 18.7's rule, measured on synthetic looks ([Profile Amount](../lightroom-profiles.md#profile-amount)); the remaining error is that of Shadows, Clarity and Contrast with Blacks, which a look's internal settings use.
- **RAWmakase Color** is our own look and is not meant to match Adobe Color exactly. It hasn't yet been compared with Camera Raw renders; built-in presets made with Adobe Standard render with RAWmakase Standard (the camera matrix) when Adobe Standard isn't imported.
- **White balance** at extreme values, and the exact order of profile, WB and calibration, are not verified.
- **Other cameras** (Canon, Nikon, Panasonic, …) render through the same generic path but have not been compared with Lightroom, for lack of sample files.

## Lens corrections

- **Fujifilm built-in vignetting** is applied at 85% log strength to match Lightroom (fitted on three X100F photos).
- **Sony built-in corrections** are available but not measured against Lightroom, and whether Lightroom uses Sony's stored data at all has not been checked. They are off by default and follow Enable Profile Corrections in the Lens Corrections panel. A Lightroom edit or preset with profile corrections on turns them on as the checkbox does: an imported Adobe profile when one matches, else Sony's built-in correction, and the import notice and the panel say the Adobe profile isn't imported. With an imported Adobe profile, Sony matches Camera Raw within ±0.02 EV in the corners ([lens corrections](../lens-corrections.md#sony-built-in-corrections)).
- **Lens profile Setup** Default matches as Auto does: RAWmakase keeps no saved per-lens defaults (Lightroom's "Save New Lens Profile Defaults"). The Make menu takes the lens maker from the profile's display name, which may differ from Lightroom's for some third-party lenses. A profile picked in RAWmakase is written without Adobe's `LensProfileDigest` ([lens corrections](../lens-corrections.md#choosing-a-profile)).
- **LCP** interpolation uses the farthest focus distance, since focus distance is not read from the files. Tangential distortion terms are ignored; optical centres are read.
- **DNG GainMap opcodes** are applied: OpcodeList1 and 2's (phone lens shading) while decoding ([RAW decoding](../raw-decoding.md)), OpcodeList3's (DJI, Hasselblad and Sigma fp DNGs) as a per-channel lens correction ([lens corrections](../lens-corrections.md)). GainMaps that cover only part of the image are not read.
- **DNG vignetting opcodes** (FixVignetteRadial) are read from OpcodeList2 and from OpcodeList3, where iPhones and DJI drones put theirs, with Camera Raw's limit on the older iPhones' ([lens corrections](../lens-corrections.md#vignetting-opcodes)). One that follows the list's warp is not read (no file seen). The copy the photo is measured on carries OpcodeList3's only: Camera Raw renders the same with the opcode in OpcodeList2, so that list's, and probably the camera makers' own vignetting tables, belong there too, which is not measured on enough photos yet.
- **Remove Chromatic Aberration** measures lateral CA radially from the image centre; Camera Raw's own estimate is not reproduced exactly, and off-centre (decentred) CA is not corrected ([lens corrections](../lens-corrections.md#remove-chromatic-aberration)).
- **Defringe** matches Camera Raw's hue ranges and strength, but not its extra reduction next to strong edges ([lens corrections](../lens-corrections.md#defringe)).
- **Manual lens vignetting** is not measured against Camera Raw.
- **Manual Distortion** matches Camera Raw's radial map and order on the chart ([lens corrections](../lens-corrections.md#manual-distortion)); Lens Corrections has a Distortion amount for it, and Constrain Crop crops out the white it uncovers.

## Geometry

- **Crop & Straighten** has Lightroom's aspect presets with X to swap orientation, the Straighten ruler (Cmd-drag), Auto straighten from Upright's Level analysis, and the Grid, Thirds, Diagonal, Triangle, Golden Ratio and Golden Spiral overlays (O, Shift+O). The Aspect Ratios overlay is not implemented, and Auto straighten measures the photo without its Transform; pasted or synchronized, an Auto angle is measured on each photo, where Lightroom copies it ([transform](../transform.md#crop-and-straighten)). Rotation and flips are not written to XMP.
- **Upright** renders imported edits from the corrections Lightroom stores, exactly. For new edits and mode-only presets RAWmakase analyses the photo itself; on 160 photos its Level is within a median 13 px of Lightroom's at 2000 px, Vertical and Auto about 60 px (mostly framing), Full about 180 px ([transform](../transform.md)). Guided draws two to four guides and solves them on the photo ([transform](../transform.md#guided-upright)); how close its framing comes to Lightroom's is not measured. Constrain Crop keeps the white out of the crop; its rule could not be measured in Camera Raw, which renders the stored crop as it is ([transform](../transform.md#constrain-crop)).

## Local and finishing adjustments

- **Spot removal and masks are experimental and early** ([retouching](../retouching.md), [masking](../masking.md)). Heal and Clone, and brush, linear, radial, color range and luminance range masks with local sliders render; little of it is measured against Camera Raw yet:
  - Heal and Clone's feather is measured in Camera Raw 18.7 (mean coverage error 0.003 on Clone spots), and Heal's tone matching is within 0.5 L* of Camera Raw on flat, gradient and textured tests ([retouching](../retouching.md)); the heal algorithm and automatic source choice are our own.
  - Local Exposure, Highlights, Shadows, Clarity and Dehaze add to the global sliders where those render, as a full-frame mask does in Camera Raw, and local Contrast is the global Contrast curve at the mask's amount. Local Whites and Blacks are Camera Raw's separate curves after the global ones, and local Texture scales the global Texture's detail. Local Temp, Tint, Hue, Saturation, Color, Sharpness and Noise are approximations; Noise only reduces noise, and local Moiré, Defringe, Grain and tone curves are not implemented.
  - Gradient transition and brush feather profiles, Flow build-up, Auto Mask edges and range-mask Refine/Smoothness are approximations.
  - The Subject, Sky and Background selections are experimental and our own, not measured against Camera Raw ([masking](../masking.md)). Lightroom's Objects, People and Depth selections, the AI Remove mode, Raw Details, Super Resolution and Lens Blur are not implemented; AI Denoise is our own (above).
- **Red Eye** is experimental ([retouching](../retouching.md#red-eye)): its correction is fitted to Camera Raw 18.7 (mean ΔE76 1.8 on in-gamut colours and grey ramps), and Lightroom's `RedEyeInfo` imports with its measured coordinates. Camera Raw also desaturates strongly red pixels joined to the pupil, such as a red-orange iris; RAWmakase keeps to the ellipse. The pupil detection is our own. Pet Eye with Add Catchlight renders and imports too, fitted to Camera Raw's falloff and catchlight.
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
- The Color Grading wheels work as Lightroom's (3-Way and single views, Shift to constrain, Cmd/Ctrl for fine, double-click to reset), but how Shift picks hue or saturation (the way the drag first moves), the fine ratio (a tenth) and the History name for a drag that changes both ("Shadow Hue & Saturation") are RAWmakase's own, not checked in Lightroom. See [color grading](../color-mixer.md#color-grading).
- Panel switches sit in the headers of Tone Curve, Color Mixer or B&W, Color Grading, Detail, Lens Corrections, Transform, Effects and Calibration; each toggle is one History step ("Enable Tone Curve", as Lightroom names it) and changing a setting in a panel that is off turns it back on. Spot Removal, Red Eye and Masking have no switch in their tool drawers yet: they keep imported switches and Red Eye turns its own on. Solo Mode (right-click a panel header) is per side of the window and kept across launches. The Histogram is always shown at the top of Develop's right side, with no header to fold or pin it, and nothing else pins. There is no Navigator panel: the photo views zoom with a slider in their toolbar and show where with a movable minimap over the photo, and a magnifier (Shift+Z) enlarges the photo under the pointer at Fit. Up and Down over a hovered slider move it by its smallest step (Shift: ten); Lightroom's own step sizes, and its + and − keys, are not checked.
- While the pointer is over the photo being edited, the line under the histogram shows the R, G and B percentages of the pixel under it, as Lightroom's, in Melissa RGB (ProPhoto primaries, sRGB tone curve). They are read from the rendered photo before clipping warnings and the monitor profile, not from the screen: at Fit one pixel of the preview, which averages the photo beneath it, at 100% and above one pixel of the photo. The renderer's output is bounded to sRGB, so colours Lightroom keeps outside sRGB read lower here. The White Balance loupe shows the same values. Over Before or the reference photo there is no readout. I cycles the photo info overlay (Info 1, Info 2, off) over the photo in Develop, the same one the Library's Loupe shows; its fields are the Library's, not Lightroom's configurable View Options.
- The histogram's clipping triangles work as Lightroom's (click for shadows or highlights alone, hover to preview, J for both), but its thresholds are RAWmakase's own (encoded sRGB 0.001 and 0.999, [color pipeline](../color-pipeline.md#display)), not measured against Lightroom. Dragging in the histogram moves Blacks, Shadows, Exposure, Highlights or Whites, in equal fifths of its width, which Lightroom's regions only approximate.
- Preset Amount works as Lightroom's slider (0–200% after applying a preset that supports one, ended by any other change), but how each setting scales is RAWmakase's own rule, not yet measured: Camera Raw offers the slider only in its own window, so measuring means driving that window, which hasn't been done. See [preset Amount](../xmp-presets.md#preset-amount).
- Raw defaults (Adobe Default, Camera Settings, RAWmakase Default or a preset, with per-camera overrides) follow Lightroom Classic, except that photos without an edit follow the current defaults instead of having them written in at import, and Auto Tone, Auto white balance or Upright in a default preset is not applied. Camera Settings starts from the camera's standard camera-matching profile (Camera Standard), not the picture style set in the camera, and needs that profile imported. See [raw defaults](../xmp-presets.md#raw-defaults).

## Validation still needed

- More cameras, profiles (Adobe Color and other looks), illuminants and clipped highlights.
- Combinations of sliders: Shadows/Highlights probes reject simple gain addition, but a replacement joint operator has not passed photo validation. Other combinations and their processing order still need verification.
- A repeatable test set that isn't private photos; Piotr plans to design the testing pipeline.
