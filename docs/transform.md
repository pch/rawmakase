# Transform panel

RAWmakase renders Lightroom's Transform panel: the manual sliders (`Recipe::transform`, imported from `crs:Perspective*`) and Upright (`Recipe::upright`, imported from `crs:PerspectiveUpright` and `crs:Upright*`). Areas with no source pixel render white, as in Lightroom.

Both are homographies applied after lens correction and before the crop, in the frame the camera recorded: before the photo is rotated or flipped for display, and after the camera's default crop. Camera Raw 18.6 works in that frame, so on a photo the camera turned to portrait, `PerspectiveVertical` keystones across the screen. Lightroom's panel shows the sliders along the displayed photo instead: its Vertical −70 on a photo turned 90° left is stored as `PerspectiveHorizontal` +70. RAWmakase stores them as Lightroom does and shows them the same way (`Transform::displayed`). Upright applies first, then the sliders. Lightroom's manual lens Distortion applies before both, in the same frame ([lens corrections](lens-corrections.md#manual-distortion)).

## Crop and Straighten

The Crop tool (R) works on the photo as shown, after its rotation and flips. The crop is stored in 0–1 coordinates of that frame, straightened: Straighten turns the photo clockwise for a positive angle and enlarges it to leave no white, so any crop inside 0–1 has the photo behind it everywhere.

- **Rotate and Flip** (`develop::turn`, `develop::mirror`) keep the crop on the same part of the photo: Rotate turns the crop with the photo, and Flip mirrors the crop and turns the straighten angle the other way. Rotate turns the photo as shown clockwise or counter-clockwise even under a single flip, where a turn of the recorded photo shows the other way round. Changing lens corrections leaves the crop's numbers as they are. Rotation and flips are kept in the catalog only; as before, XMP carries the crop and angle (`CropLeft` to `CropBottom`, `CropAngle`) but not the orientation.
- **Straighten ruler**: the panel's Ruler, or a Cmd-drag on the photo as in Lightroom, draws a line; on release the angle is set so the line is level, or plumb when it is nearer upright than level, as one History step ("Straighten"). Lines shorter than 10 points are ignored. The angle stays within ±45°.
- **Auto** (next to the Ruler) measures the photo with Upright's analysis and sets the angle Level would roll it by, as one History step ("Straighten", "Auto"); Upright's mode is left alone. It analyses the photo as shown without its Transform (orientation and lens corrections only), off the UI thread, and measures again if the photo is turned, flipped or its lens corrections change meanwhile. On a synthetic tilted horizon and rolled verticals it levels the edges within 1 px over 200 px. A photo with no long near-horizontal edges or verticals gets no angle.
- **X** swaps the crop between portrait and landscape at the same aspect, about its centre, shrinking it to fit the photo when needed. Aspect presets are long side over short in the photo's own orientation; after X the preset is kept as its reciprocal, so dragging a handle keeps the swapped orientation.
- **Overlays**: Grid, Thirds, Diagonal, Triangle, Golden Ratio and Golden Spiral. O cycles them in that order, Shift+O turns the Triangle (2 ways) and Golden Spiral (4 corners). The panel's Overlay menu shows when: Always, Auto (the default: with the pointer over the photo, as Lightroom's Auto Show, while the crop or ruler is dragged, and for 1.5 s after an overlay is picked, so choosing one in the panel shows it) or Never. The ruler shows a grid while it is drawn. The overlay and when it shows are a view preference saved in the session, not part of the edit. Lightroom's Aspect Ratios overlay is not implemented.

X, O and Shift+O work only while the Crop tool is open and no text field has focus; with the Crop tool open X does not reject the photo.

### In-camera aspect ratio

A raw shot at an aspect ratio other than the sensor's (1:1, 4:3, 16:9 on a 3:2 Canon; 16:9 on an Olympus or Fujifilm) keeps the whole frame, and the camera records the ratio it showed. Camera Raw opens such a photo cropped to that ratio, and the crop can be widened again: Adobe DNG Converter writes it as DefaultUserCrop inside an unchanged DefaultCrop, and Camera Raw's settings carry it as an ordinary crop (`HasCrop` True, `CropLeft` 0.166667 to `CropRight` 0.833333 on an EOS M6 Mark II at 1:1). RAWmakase does the same: the frame (image space, masks, Upright and Lightroom's crop values) is the camera's default crop, and the ratio is `Metadata::camera_crop`, the crop a photo's settings start from (`Recipe::for_metadata`), so Reset returns to it and the Crop panel's Reset removes it. It comes from LibRaw: for a Canon its AspectInfo crop inside SensorInfo's frame, for other makes the second inset crop LibRaw derives from the recorded ratio, and for a DNG its DefaultUserCrop. LibRaw's rectangles are in raw coordinates and are moved to the decoded image's. Settings with `HasCrop` False are the whole frame, as in Camera Raw; settings without crop values keep the starting crop.

Adobe does not treat every camera this way: DNG Converter 18 hard-crops the PowerShot G12 (2010) to its 3:2 setting as the DefaultCrop, while the S120 (2013) and later get a DefaultUserCrop. RAWmakase uses the starting crop for all of them, so a Lightroom crop of a G12 photo shot at 3:2 is read against the 4:3 frame.

## Constrain Crop

Lightroom's Constrain Crop (`crs:CropConstrainToWarp` 1; the Transform panel's checkbox, `Recipe::constrain_crop`) keeps the white areas that Upright, the Transform sliders and manual Distortion uncover out of the crop. It is not `CropConstrainToUnitSquare`, which only limits Lightroom's crop tool.

Camera Raw 18.7 does not apply the flag when it renders: on the synthetic chart with Vertical +30, Rotate 5, Scale 80 or manual Distortion +50, with no crop, a full crop or a user crop, renders with and without `CropConstrainToWarp="1"` are identical, white areas included. Lightroom constrains the crop in its crop tool and stores the result in `CropLeft` to `CropBottom`. How its tool picks that crop can't be scripted or read from a sidecar, so it was not measured.

RAWmakase applies the constraint while rendering, so it follows every later change to the geometry: the crop as rendered (`Geometry::crop`) is the stored crop when every position in it has a source pixel, and otherwise the largest crop at the stored crop's aspect that has one everywhere and lies inside the stored crop. It shrinks about the stored crop's centre unless moving it keeps more than 0.1% more of its size; for a keystone from Vertical it slides towards the wider edge. A crop lying wholly in the white becomes the largest crop at its aspect anywhere in the photo. Resetting the Transform panel turns Constrain Crop off. Straighten alone never needs it: the photo is already enlarged to leave no white. The stored crop stays as the user drew it, and the Crop tool shows the whole photo around it, white areas included. Lens profiles and built-in lens data never uncover white (their correction is scaled to fill the frame), so the area is set by Upright, the Transform sliders and manual Distortion.

An exported photo's settings carry the crop as rendered with `CropConstrainToWarp="1"`, as Lightroom stores it, so Camera Raw renders the same crop, and reading them back changes nothing.

## Upright

Lightroom stores the correction for every mode, `crs:UprightTransform_0` to `_5`, indexed by the `PerspectiveUpright` code: 0 Off, 1 Auto, 2 Full, 3 Level, 4 Vertical, 5 Guided. Each is a row-major forward (source-to-output) homography in 0–1 coordinates of the recorded frame. Camera Raw renders the stored matrix as it is: replacing it with a translation moves the render by exactly that, and a wrong `UprightDependentDigest` does not make it recompute. RAWmakase keeps all six corrections, so switching modes needs no new analysis, and keeps the other `Upright*` settings to write back.

Checked against Camera Raw renders of five photos (Sony A7 II and Fujifilm X100F; landscape, both portrait orientations; Level, Vertical and Full; with and without lens profiles): the stored matrices reproduce Camera Raw's geometry within 0.0002 of the image size, and RAWmakase's renders within 1.3 px at 2000 px, the same as the untransformed renders.

A preset that names only an Upright mode applies, and the app analyses each photo it is applied to. A photo's own settings (sidecar or catalog) without Lightroom's stored corrections, and Guided without a stored correction, are reported as unsupported. Guided edits are made in RAWmakase as [described below](#guided-upright).

## Guided Upright

Guided (Shift+T, or Guided among the Upright modes, which picks up the tool as in Lightroom) corrects the photo along guides drawn on it. The Transform panel's Guides row shows how many there are, Draw opens or closes the tool, and Clear removes them all; Show Loupe and Grid sit below it while the tool is open. Enter or Escape closes the tool.

- **Drawing**: a drag on the photo draws a guide; one shorter than 10 points is not a guide. There can be four, as in Lightroom; a fifth is refused in the status line. Dragging a guide's end moves it, a click selects a guide, and Delete or Backspace removes the selected one.
- **Loupe and grid**: while a guide or an end is being placed, a loupe beside the pointer shows the photo magnified 4× with a cross on the point (Show Loupe, on by default). Grid lays a square grid over the photo to judge the result by.
- **History**: each gesture is one History step: Add Guide, Move Guide, Delete Guide, Clear Guides. The correction is solved again when the gesture ends.

**Solving.** A guide nearer upright than level is a vertical, otherwise a horizontal. Each guide is the plane through the camera that holds its edge, at the photo's focal length (as for the other modes). Two or more verticals meet at a vanishing point, and the camera turns the least that makes that direction plumb, as Vertical does; two or more horizontals likewise make theirs level. With guides of both kinds both directions are fixed: the verticals exactly, then the horizontals as nearly as they allow at right angles to them (with only one vertical, the horizontals come first). One vertical and one horizontal leave a turn free; the smallest turn that makes both right is taken. The correction is framed as the other modes are (enlarged to fill when that takes at most 110%, otherwise fitted to the width) and stored as `UprightTransform_5`. The Transform sliders apply after it, as with every mode.

On synthetic photos of converging edges (camera tilted 1–12°, panned up to 20°, rolled 2–3°; 3000 × 2000), every vertical and horizontal of the scene, not only the guided ones, ends within 1 px per 1000 px of plumb or level for two verticals, two horizontals, three and four guides, and one of each.

**When guides can't be used** nothing crashes, and the status line says why:

- fewer than two guides: nothing is corrected ("draw two or more guides");
- a guide shorter than 2% of the long edge (or broken coordinates from a file) is left out;
- two guides of one kind along one line count as one;
- guides calling for a turn of more than 60°, or one that would put a corner of the photo behind the camera (lines crossing inside the photo, say), correct nothing;
- guides that ask for more than one turn can give (a horizontal drawn off its edge, say) are corrected as nearly as they allow, verticals first, and the status line says so.

**Where guides live.** Guides are kept in Upright's own frame: 0–1 coordinates of the photo as recorded, after lens corrections and before Upright, the Transform sliders, rotation, flips and the crop. So a guide stays on the edge it was drawn along however the correction moves it on screen. The crop leaves guides and correction alone. Rotate and Flip keep the correction, as they do for the other modes; a guide edited after a quarter turn is solved on the turned photo, and the same edges come out straight (now level), giving the same correction within 0.001. Lens corrections apply before Upright, so changing them keeps the guides where they are in the corrected photo; Update in the Upright row, a Paste of new lens settings or a preset analyses the photo again and solves the guides afresh. A change of distortion correction moves the photo's edges a little under guides that stay put; draw them again if one has slipped off its edge.

**XMP.** Camera Raw 18.7 stores the guides as `crs:UprightFourSegmentsCount` (how many) and `crs:UprightFourSegments_0` to `_3`, each `"x1,y1,x2,y2"`: the two ends in 0–1 coordinates, nine decimals, comma separated (captured from the settings Camera Raw kept for a synthetic DNG given two guides; space-separated values were dropped, leaving a count of 0). The correction itself is `UprightTransform_5` with `PerspectiveUpright` 5, and `UprightGuidedDependentDigest` records what Lightroom solved it from. RAWmakase reads the guides into the edit and writes them back the same way, beside its own `UprightTransform_5`; once guides are edited, Lightroom's digest is dropped with the guides it described. Camera Raw renders the stored `UprightTransform_5` and does not solve guides on its own: a render with guides and no stored correction is identical to one without Upright. So the frame of the guide coordinates could not be measured from renders; RAWmakase assumes the frame Lightroom's corrections use (the photo as recorded, after lens corrections), which only a real Lightroom Guided edit can confirm. A Guided sidecar with guides but no stored correction is still reported as unsupported: renders outside the editor (the command line, Library previews) have no analysis to solve them beside.

**Copy, Sync and presets.** Guides belong to the photo they were drawn on. Upright Mode copies the mode only: a photo with guides of its own solves them, and one without is left Off with a note, as before. Upright Transforms (in the Transform section of Copy Settings and Sync, off by default as in Lightroom) copies the correction exactly as solved or analysed on the source, guides and Lightroom's analysis details included, without analysing the target; it is for photos framed alike, such as a tripod series. Presets never carry one photo's correction or guides: they name the mode, and the New Develop Preset dialog leaves Upright Transforms out.

## Upright analysis

For new edits, `develop::upright` finds straight edges in a 1024-pixel luminance copy of the displayed photo (an LSD-style detector), then the vertical vanishing point (edges within 20° of vertical, weighted by squared length, with a prior against strong tilts) and a horizontal one orthogonal to it. Lightroom's corrections are camera rotations K·R·K⁻¹ at focal length f = 35mm-equivalent / 36 in long-edge units (fitted to its stored corrections within 1e-9): Level rolls, Vertical takes the vertical vanishing point to vertical, Full also pans to the facade turned least, and Auto corrects part of the tilt. Lightroom then enlarges the result to fill the frame when that takes at most 110%; otherwise Level keeps its size and the other modes fit its width.

Against Lightroom's own corrections on 160 of the author's photos (median, at 2000 px): Level 13 px, Vertical 58 px, Auto 59 px, Full 186 px. Most of the Vertical and Auto difference is framing; the straightening itself usually agrees within 10 px.

## Sliders

Coordinates are centred, y down, in units of the long edge of the recorded frame. The forward matrix is offset · scale · aspect · perspective · rotate, measured by fitting homographies to Camera Raw renders of slider pairs (each other order was 3–10 times worse):

| Slider (Lightroom value / 100) | Forward matrix |
|---|---|
| Rotate θ | rotation by θ degrees, applied first |
| Vertical v, Horizontal h | with q = (h, v), s = \|q\|: `[[I + e(s) q qᵀ / s², 0], [−qᵀ, 1]]`, e(s) = 0.0391 s² + 0.9251 s³ − 0.2827 s⁴ (fitted to Vertical at ±10 to 100 and five Vertical + Horizontal pairs; within 0.0009) |
| Aspect a | x × 2^(−0.137 a), y × 2^(0.137 a) |
| Scale | uniform scale |
| X / Y Offset | translation by 0.811 × image width / height; positive Y moves up |

Checked against 33 Camera Raw renders of three photos (single sliders, pairs, and with Upright), RAWmakase's geometry is within 1.1 px at 2000 px on photos without a lens profile and 2.7 px with one, the same as the untransformed renders.

Before this, the sliders applied Vertical before Rotate, used e(s) = 0.347 s² + 0.334 s⁴ for each axis separately, and acted on the rotated photo, which put portrait photos off by up to 470 px.
