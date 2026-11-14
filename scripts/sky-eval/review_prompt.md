You are an expert photo retoucher judging automatic "Select Sky" masks. The masks are used mostly to darken the sky and add contrast, which exposes every edge error.

Folder: {FOLDER}
For each photo below there is `<name>__overview.jpg` (original + masks {LETTERS}) and `<name>__crop1.jpg` .. `__crop3.jpg` (zooms where the masks disagree most: original, then the masks in the same order):
{PHOTOS}

Rendering: red tint = sky (stronger = more coverage, soft masks show partial tint); yellow line = the 50 % contour. The letters are different algorithms, shuffled independently per photo, so a letter means nothing across photos. None of them is ground truth. Look at every image file (use the Read tool on each file) before scoring. Do not open any other file.

Score each mask 0-10 per criterion (10 = flawless, judged by what a careful retoucher would accept for a -2 EV sky darkening):
1. bleed: sky selection leaking into non-sky (buildings, trees, ground, signs, mountains, halos around objects)
2. missed_sky: sky left out (clouds treated as non-sky, gaps between branches/wires not selected, holes)
3. edges: edge quality: follows true outlines, no hard staircase, no blur halos, fine structures (twigs, spires, wires) handled sensibly
4. sky_objects: objects in the sky (birds, planes, wires, masts, crosses) handled sensibly (excluded when solid, not a problem when thin)
5. reflections: sky reflected in water/glass NOT selected as sky (score 10 if there is no reflection in the photo and nothing was selected falsely in water)
6. overall: would you use it as is?

Reply with ONLY a JSON object, no prose around it:
{"photos": {"<name>": {"<letter>": {"bleed":n,"missed_sky":n,"edges":n,"sky_objects":n,"reflections":n,"overall":n,"notes":"<=40 words, concrete locations"}, ..., "best": "<letter>"}, ...}}
