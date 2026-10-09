# Spot removal and red eye

Status: experimental and early. It works, but none of it is measured against
Lightroom yet, and details may change.

The Remove tool (Q) works like Lightroom Classic's Remove panel in Heal and Clone
modes. The AI Remove mode is not implemented yet.

## Using it

- **Click** a spot: RAWmakase adds a circle and picks a source nearby automatically.
- **Drag** to paint a brushed area; its source is picked when you release.
- **Cmd-drag** (Ctrl-drag on Linux) places a spot and drags its source by hand.
- Drag a spot's pin or circle to move it; drag its source circle to move the source.
- **/** picks the next best source for the selected spot.
- **[ ]** change the size (of the selected spot, or of new ones); **Shift+[ ]** the
  feather. **Delete** removes the selected spot, **H** hides the pins.
- **A** turns on Visualize Spots, a black-and-white view of fine detail where dust
  stands out; its Threshold slider shows fainter detail.
- Hold **Space** to pan while the tool is open.
- The drawer's Mode, Size, Feather and Opacity apply to the selected spot, or to new
  spots when none is selected. The mouse wheel over the photo sizes the brush and
  the selected spot (Shift: feather), as in Lightroom; one scroll is one History step.

Paste Settings and presets leave a photo's spots alone, as Lightroom's defaults do.

## How it renders

- Spots are stored as parameters (`retouch`) beside the recipe, not in it: in the
  catalog's `local_edits` table, or for photos opened directly in
  `photo.ARW.rawmakase-local.json` next to the sidecar, so the recipe itself holds
  none. Positions are in image space: the photo as the camera oriented it, within its default crop, before lens
  correction, Transform, crop and straightening. Spots stay on their dust when those
  change. Sizes are fractions of the long edge.
- Operations apply in order to the linear, highlight-recovered camera image before
  anything else, so every later edit, and export at full resolution, sees the
  retouched pixels.
- **Clone** blends the source in with the feathered shape.
- **Feather** follows Camera Raw 18.7, measured on Clone spots at Feather 25–100: a
  table of the source's weight in the linear blend against the distance from the
  centre, interpolated between those settings and from a hard edge below 25. On those
  spots the rendered coverage is within a mean 0.003 of Camera Raw's (at most 0.07,
  at Feather 25 on the rim's last pixel); the original smoothstep was 0.06–0.13 off,
  its soft edge reaching much further in. Every spot, including those saved with the
  original feather, renders with the measured one.
- **Heal** copies the source, then adds a membrane: the difference between
  destination and source on a one-pixel ring around the shape, extended inward by
  solving Laplace's equation (multigrid V-cycles, so large brushed areas solve as
  quickly as small spots). This is done on log values: on test data, a texture
  healed from a darker area had an RMS residual of 0.014 in log values, 0.028 with
  ln(1 + x) and 0.035 in linear values. Log values reproduce a smooth gradient
  within 1% (0.01 EV); linear values would be exact there.
- **Automatic source:** candidates on rings at 1.5–6 radii (of the brushed area's
  size for brushes) are scored on a reduced copy of the neighbourhood by how well the
  border matches (SSD of log values on a ring around the shape), how similar the
  texture is (gradient energy), and penalties for overlapping the spot itself, other
  spots and clipped highlights; the best is refined by a local search. On a synthetic
  chart (dust on a gradient and on a lit texture, through a real camera profile) the
  healed spots differ from the clean render by a mean ΔE00 of 0.42; the dust was
  10.2 (`tests/color/retouch.rs`).
- **Previews** keep the retouched image and recompute only the 256-pixel tiles a
  change reaches (and anything that reads them, until nothing more changes), then
  patch those areas of the preview pyramid. Exports build the retouched image at
  once. Tests check that the tiles match a full rebuild and that Fit, 100% regions
  and exports agree.

## Not verified against Lightroom

- Heal's correction was compared with Camera Raw 18.7 on flat and gradient
  destinations healed from a brighter flat, a blue flat and a texture: inside the
  spot RAWmakase is within 0.5 L* of Camera Raw, as close as the rest of the render.
  The membrane itself is our own; Lightroom's algorithm is not public.
- Lightroom's automatic source choice is not measured: Camera Raw renders the source
  a file stores, so its choice can't be scripted.
- Previews keep one full-resolution retouched copy of the photo in memory while it
  has spots.

# Red eye

Status: experimental. Red Eye and Pet Eye are fitted to Camera Raw 18.7 on synthetic
eyes.

## Using it

The **Red Eye** tool sits between Remove and Masking, as in Lightroom Classic. Like
Lightroom's, it has no keyboard shortcut (Lightroom's menu leaves it unassigned, and
Shift+R is its Reference View).

- **Click** the centre of an eye: RAWmakase finds the red pupil inside the circle shown
  around the pointer and corrects it. The mouse wheel over the photo, or **[** and
  **]**, make the circle smaller or larger; dragging doesn't size it, as in Lightroom.
  If nothing red enough is found, the status line says "Unable to find red eye", as
  Lightroom does.
- Click a correction to select it; drag it to move it. **Delete** removes the selected
  one, and Reset removes them all.
- **Type** picks Red Eye or Pet Eye for new corrections, and changes the selected one.
  Pet Eye finds a pupil that glows brighter than the iris around it, whatever its
  colour, and turns it black.
- **Pupil Size** and **Darken** change the selected correction (both 50 by default).
  A pet eye has no Darken (Camera Raw makes it black whatever Darken says) but has
  **Add Catchlight**, on by default at Lightroom's place; drag the small circle to move
  the catchlight. It stays within the pupil.
- Each drag, click or slider change is one History step: "Add Red Eye Correction",
  "Update Pet Eye Correction", "Delete Red Eye Correction" and so on.
- Copy, Paste, Sync and presets leave red eye corrections alone: Lightroom's Copy
  Settings has no group for them. The panel switch (`EnableRedEye`) turns them off.

## How it works

- **Finding the pupil:** redness is the log ratio of red to the larger of green and
  blue in the linear camera image. The pupil is the connected area around the reddest
  point near the circle's centre whose redness is more than half-way from the circle's
  rim (the face) to that point, with any catchlight inside filled in. Its ellipse has
  the area's centre and, from its second moments, semi-axes and a tilt. Red must reach
  3.5 times the larger of green and blue (a brown iris is about 2.5 in linear values,
  a red pupil 10 or more); an area reaching most of the rim is refused. On test eyes
  this finds pupils of 3 to 45 pixels within half a pixel, keeps a red-brown iris out,
  and finds tilted pupils whatever the camera orientation. For Pet Eye the score is
  the log of the largest channel; the pupil's level is where the brightest twentieth
  of the circle's centre begins (so a catchlight isn't taken for the pupil), its edge is
  where the glow falls to 65% of that, and it must be at least 1.6 times brighter than
  the second ring of pixels around it (past its anti-aliased edge), so a bright face
  around a dark iris doesn't hide it.
- **Storage:** corrections are parameters beside the recipe, like spots (`red_eye` in
  the catalog's `local_edits`), in image space, so they stay on the eye through crop,
  straightening, Transform, rotation and flips. Releases that predate them open the
  photo without them. A correction of a kind a later release adds is not shown, but
  is kept and saved back as it was, and never stops the photo's other spots and masks
  from loading.
- **Rendering:** on the linear camera image, before Heal and Clone (so a heal copying
  from an eye copies the corrected pupil), with previews recomputing only the tiles a
  change reaches. Inside a soft ellipse every pixel moves towards a dark neutral, as
  Camera Raw's does: the level is the mean of green and blue in `v^(1/2.4)` encoding
  (red counts −0.11), scaled by a gain of 1.28, 0.90 and 0.39 at Darken 0, 50 and 100
  (quadratic between), keeping 2.2% of the original colour's log ratios, and blended
  with the falloff in the same encoding. The falloff is full to 0.55 and gone at 1.42
  times the half-way distance, which is 0.585 + 0.975 × Pupil Size times the ellipse.
- **Pet Eye** blends towards black with the same falloff at 0.953 times the distance.
  Its catchlight is stored as an offset in units of the semi-axes along x and y
  (Lightroom's `highlightX`/`highlightY` minus 0.5, doubled), times the half-way
  distance; it is full to 0.06 and gone at 0.17 of that distance, blends towards a
  linear 0.5, and fades with the pupil's correction towards the ellipse's edge.

## Measured against Camera Raw 18.7

Synthetic DNGs (the colour charts' invented camera, with red pupils, irises, grey
ramps and colour patches) were opened in Camera Raw with `crs:RedEyeInfo` written by
hand, and rendered to 16-bit sRGB.

- **Format:** XMP holds an `rdf:Seq` with one text item per eye:
  `x = 0.520833, y = 0.341797, width = 0.013021, height = 0.019531, alpha = 0.000000,
  density = 0.750000, strength = 0.080000, redBias = 0.200000, pupilSize = 0.500000,
  pupilDarkenAmount = 0.500000, adaptivePupilColor = 0, gammaEncodeCorrection = 1,
  showPetEyeHighlight = 1, highlightX = 0.591000, highlightY = 0.424000`. Camera Raw
  also reads the first ten fields alone. A Lightroom catalog keeps the same values as a
  table, the ellipse in `pupil.ellipse` (`centerX`, `centerY`, `sizeX`, `sizeY`,
  `alpha`).
- **Coordinates:** the centre is normalised to the unrotated sensor frame, as spots
  are (checked on a photo tagged as portrait). `width` and `height` are semi-axes as
  fractions of that frame's width and height. `alpha` is not an angle but the
  correlation of x and y over the ellipse: 0.5 tilted a 150 × 60 pixel ellipse by 12.6°
  and kept its extent along x and y; Camera Raw refuses `alpha = 1`.
- `density`, `strength` and `redBias` record Lightroom's detection; changing them did
  not change the render. `adaptivePupilColor = 1` is Pet Eye: a black pupil whatever
  Darken says, with a catchlight when `showPetEyeHighlight = 1`. `highlightX` and
  `highlightY` put it at 2 × (h − 0.5) of the falloff's half-way distance along each
  axis (0.5 is the centre); one placed outside the pupil, such as 0.9, 0.1, is not
  drawn. `gammaEncodeCorrection = 0`, an older rendering, is read as 1.
- **Pet Eye against Camera Raw:** the falloff's half-way point is within a pixel or
  two at Pupil Size 0, 50 and 100 on 15–60 pixel pupils. The catchlight's centre is
  within 0.05 of the pupil's radius of Camera Raw's and its area within 7%; its
  brightness is 234–252 against 232–245 (8-bit). Camera Raw renders the pupil at 3
  against RAWmakase's 0, and near the pupil's edge Camera Raw dims the catchlight
  more (152 against 214 at `highlightX = 0.2`).
- **Falloff:** on grey, the correction is half applied at 0.59 times the ellipse at
  Pupil Size 0, growing by 0.97 per unit of Pupil Size, and fades from 0.69 to 1.46
  times that; RAWmakase's falloff matches within an RMS weight of 0.02–0.03.
- **Colour:** on 25 in-gamut reds, browns, skin and greys and on grey ramps at five
  Darken values, RAWmakase's corrected pixels sit a mean ΔE76 of 1.8 from Camera Raw's
  (grey ramps 0.5–1.4, patches 2.6–3.4), after taking out the difference between the
  two renders without the correction. Saturated reds that Camera Raw clips render
  differently without the correction too, and were left out.

## Not like Lightroom

- Camera Raw also desaturates strongly red pixels joined to the pupil beyond the
  ellipse: a red-orange iris around a 20-pixel ellipse changed out to its edge at 78
  pixels, and saturated red patches kept 30–55% of the correction to twice the
  ellipse's size. RAWmakase corrects only within the ellipse, so a reddish iris keeps
  its colour; on brown irises and skin the two agree (under 7% in Camera Raw).
- Lightroom's own detection is not public; RAWmakase's finds a similar ellipse but not
  the same one, and does not try again with a larger area when the first fails.
- Camera Raw's catchlight sits slightly further from the centre vertically than
  horizontally for the same offset (0.196 against 0.186 of the radius for Lightroom's
  default); RAWmakase treats both axes alike.
