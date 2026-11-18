#!/usr/bin/env python3
"""Score RAWmakase's AI Denoise against Camera Raw 18.7's Denoise (#802).

  python3 scripts/corpus/ai-denoise-score.py DIR VARIANT [VARIANT ...]

DIR holds, per photo and Amount (0 = off, 50, 100):

  cr/<photo>-<amount>.tif           Camera Raw's render: 16-bit ProPhoto, full size
  xmp/<photo>-<amount>.xmp          the sidecar Camera Raw wrote for it
  rm/<variant>/<photo>-<amount>.tif RAWmakase's render of that sidecar
                                    (`rawmakase render-batch`, sRGB 16-bit, max_edge 0)

Camera Raw runs Denoise only from its dialog: a sidecar naming the Enhance filter is
not enough, as the denoised data lives in an `.acr` file the dialog writes beside the
RAW. Make the references by opening a copy of the RAW per Amount in Camera Raw,
switching Denoise on at that Amount and clicking Done, then opening each copy by script
(scripts/corpus/camera_raw.py's open options) and saving the TIFF. Not with
`camera_raw.render`: it opens a fresh copy with a sidecar of its own, without the `.acr`,
and Camera Raw then renders without Denoise and without a warning. The sidecar's
`photoshop:SidecarForExtension` is what lets `render-batch` import Denoise as AI Denoise.

Both renders are compared as encoded sRGB in CIELAB at full size, after aligning by an
integer shift (default crops differ by a few pixels). 32-pixel blocks are classed on
Camera Raw's Amount-100 render: flat (the lowest quarter of gradient energy, L* 8–92),
textured (the top quarter) and dark (mean L* below 25). Per photo and Amount:

  chroma   median over flat blocks of the spread of a*, b* within the block
  luma     the same for L*
  blotch   dark blocks: spread of the blocks' mean a*, b* after removing a 256-pixel
           trend, the low-frequency colour mottling of #802
  detail   textured blocks: mean |∇L*| of RAWmakase's render over Camera Raw's
  dE       median ΔE00 of the 32-pixel block means against Camera Raw (colour and
           tone without the noise); dEdark the same over dark blocks

Camera Raw's own chroma, luma and blotch are printed beside RAWmakase's. Requires
numpy, Pillow and tifffile. Never commit photos or renders.
"""
import json
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
import gate  # noqa: E402
import parity_metrics as pm  # noqa: E402

B = 32
AMOUNTS = (0, 50, 100)


def to_lab(srgb):
    return pm.lab(srgb.astype(np.float32))


def align(ref_l, img_l, r=24):
    """The (dy, dx) shift that best maps `img_l` onto `ref_l`, on a central crop."""
    h, w = ref_l.shape
    cy, cx, s = h // 2, w // 2, min(h, w) // 4
    a = ref_l[cy - s:cy + s, cx - s:cx + s]

    def error(dy, dx, step):
        b = img_l[cy - s + dy:cy + s + dy, cx - s + dx:cx + s + dx]
        return np.mean(np.abs(a[::step, ::step] - b[::step, ::step])) if b.shape == a.shape else np.inf

    best = min((error(dy, dx, 4), dy, dx) for dy in range(-r, r + 1, 2) for dx in range(-r, r + 1, 2))
    _, by, bx = best
    best = min((error(dy, dx, 2), dy, dx) for dy in range(by - 1, by + 2) for dx in range(bx - 1, bx + 2))
    return best[1], best[2]


def common(ref, img, dy, dx):
    """`ref` and `img` cropped to their overlap, `img` shifted by (dy, dx)."""
    h, w = ref.shape[:2]
    H, W = img.shape[:2]
    y0, x0 = max(0, -dy), max(0, -dx)
    y1, x1 = min(h, H - dy), min(w, W - dx)
    return ref[y0:y1, x0:x1], img[y0 + dy:y1 + dy, x0 + dx:x1 + dx]


def blocks(a):
    h, w = a.shape[0] // B * B, a.shape[1] // B * B
    a = a[:h, :w]
    return a.reshape(h // B, B, w // B, B, *a.shape[2:]).swapaxes(1, 2)


def grad(l):
    gy, gx = np.gradient(l)
    return np.hypot(gy, gx)


def trend(means, k=8):
    """Box blur of block means over k × k blocks (256 pixels), edges repeated."""
    lo, hi = k // 2, k - 1 - k // 2
    padded = np.pad(means, ((lo, hi), (lo, hi), (0, 0)), mode='edge')
    c = np.pad(padded.cumsum(0).cumsum(1), ((1, 0), (1, 0), (0, 0)))
    h, w = means.shape[:2]
    return (c[k:k + h, k:k + w] - c[:h, k:k + w] - c[k:k + h, :w] + c[:h, :w]) / (k * k)


def classes(lab100):
    g = blocks(grad(lab100[..., 0])).mean((2, 3))
    L = blocks(lab100[..., 0]).mean((2, 3))
    ok = (L > 8) & (L < 92)
    flat = ok & (g <= np.quantile(g[ok], 0.25))
    textured = ok & (g >= np.quantile(g[ok], 0.75))
    return flat, textured, L < 25


def stats(lab, flat, textured, dark):
    bl = blocks(lab)
    spread = bl.std((2, 3))
    means = bl.mean((2, 3))
    residual = means - trend(means)
    g = blocks(grad(lab[..., 0])).mean((2, 3))
    blotch = None
    if dark.sum() > 20:
        blotch = float(np.hypot(residual[..., 1][dark].std(), residual[..., 2][dark].std()))
    return dict(chroma=float(np.median(np.hypot(spread[..., 1], spread[..., 2])[flat])),
                luma=float(np.median(spread[..., 0][flat])), blotch=blotch,
                gradient=float(g[textured].mean()), means=means)


def compare(cr, cr100, img):
    """Metrics of RAWmakase's render `img` (CIELAB) against Camera Raw's `cr` at the
    same Amount, blocks classed on Camera Raw's Amount-100 render `cr100`."""
    dy, dx = align(cr100[..., 0], img[..., 0])
    ref100, framed = common(cr100, img, dy, dx)
    ref, _ = common(cr, img, dy, dx)
    flat, textured, dark = classes(ref100)
    a, b = stats(ref, flat, textured, dark), stats(framed, flat, textured, dark)
    de = pm.de00(a.pop('means'), b.pop('means'))
    return dict(shift=[dy, dx], cr={k: a[k] for k in ('chroma', 'luma', 'blotch')},
                rm={k: b[k] for k in ('chroma', 'luma', 'blotch')},
                detail=b['gradient'] / a['gradient'], dE=float(np.median(de)),
                dEdark=float(np.median(de[dark])) if dark.sum() > 20 else None)


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    root, variants = Path(sys.argv[1]), sys.argv[2:]
    photos = sorted({p.name.rsplit('-', 1)[0] for p in (root / 'cr').glob('*-100.tif')})
    rows = []
    for photo in photos:
        cr = {a: to_lab(gate.reference_srgb(root / 'cr' / f'{photo}-{a}.tif'))
              for a in AMOUNTS if (root / 'cr' / f'{photo}-{a}.tif').exists()}
        for variant in variants:
            for amount, ref in cr.items():
                path = root / 'rm' / variant / f'{photo}-{amount}.tif'
                if path.exists():
                    m = compare(ref, cr[100], to_lab(gate.render_srgb(path)))
                    rows.append(dict(photo=photo, variant=variant, amount=amount, **m))
    out = root / 'scores' / ('-'.join(variants) + '.json')
    out.parent.mkdir(exist_ok=True)
    out.write_text(json.dumps(rows, indent=1))
    f = lambda v: '   -  ' if v is None else f'{v:6.2f}'
    print(f'{"photo":8} {"variant":14} amt | chroma rm/cr | luma rm/cr | blotch rm/cr | detail |   dE  dEdark')
    for r in rows:
        rm, cr = r['rm'], r['cr']
        print(f'{r["photo"]:8} {r["variant"]:14} {r["amount"]:3} | {f(rm["chroma"])} {f(cr["chroma"])} |'
              f' {f(rm["luma"])} {f(cr["luma"])} | {f(rm["blotch"])} {f(cr["blotch"])} |'
              f' {r["detail"]:6.2f} | {f(r["dE"])} {f(r["dEdark"])}')
    print(f'Written to {out}')


if __name__ == '__main__':
    main()
