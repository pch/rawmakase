"""Compare Select Sky masks against each other and against Lightroom's Sky mask.

Usage:
  evaluate.py --run RUN_DIR --lr LR_EXPORT_DIR --masks LABEL=DIR [--masks LABEL=DIR ...]

RUN_DIR/photos.txt lists the photos (one path per line; only the stem is used). Each
--masks DIR holds `<stem>.png` coverage rasters (any size, same aspect as the photo). The
first --masks is the baseline, the last the candidate.

Lightroom's mask is recovered from two exports per photo in LR_EXPORT_DIR:
`<stem>_original.<ext>` and `<stem>_sky-exp_<EV>.<ext>` (EV e.g. -4), where <ext> is tif/tiff
(16-bit preferred: no JPEG noise, 256x finer levels) or jpg. See README.md.

Writes RUN_DIR/metrics.json, RUN_DIR/key.json (which letter is which source, per photo)
and blind review panels in RUN_DIR/review/.
"""
import argparse, glob, json, os, random, re
import numpy as np
from PIL import Image, ImageDraw, ImageFont
from scipy import ndimage

Image.MAX_IMAGE_PIXELS = None
EVAL_EDGE = 2048
LETTERS = "XYZWVU"


def read_rgb(path, size):
    """RGB as float 0..255 at `size`, from 8- or 16-bit files."""
    if path.lower().endswith((".tif", ".tiff")):
        import tifffile
        a = tifffile.imread(path)
        if a.ndim == 3 and a.shape[0] in (3, 4) and a.shape[-1] not in (3, 4):
            a = np.moveaxis(a, 0, -1)
        a = a[..., :3].astype(np.float32)
        scale = 65535.0 if a.max() > 255 else 255.0
        a = a * (255.0 / scale)
        chans = [np.asarray(Image.fromarray(a[..., c]).resize(size, Image.BILINEAR)) for c in range(3)]
        return np.stack(chans, -1).astype(np.float32)
    im = Image.open(path).convert("RGB")
    return np.asarray(im.resize(size, Image.BILINEAR), dtype=np.float32)


def image_size(path):
    """(width, height) without decoding the pixels."""
    if path.lower().endswith((".tif", ".tiff")):
        import tifffile
        shape = tifffile.TiffFile(path).pages[0].shape
        return (shape[2], shape[1]) if len(shape) == 3 and shape[0] in (3, 4) else (shape[1], shape[0])
    with Image.open(path) as im:
        return im.size


def read_gray(path, size):
    return np.asarray(Image.open(path).convert("L").resize(size, Image.BILINEAR), dtype=np.float32) / 255.0


def srgb_lin(a):
    a = a / 255.0
    return np.where(a <= 0.04045, a / 12.92, ((a + 0.055) / 1.055) ** 2.4)


def lr_files(lr_dir, stem):
    def pick(pattern):
        found = sorted(glob.glob(os.path.join(lr_dir, glob.escape(stem) + pattern)))
        tifs = [f for f in found if f.lower().endswith((".tif", ".tiff"))]
        return (tifs or found or [None])[0]
    orig, masked = pick("_original.*"), pick("_sky-exp_*.*")
    if not orig or not masked:
        raise SystemExit(f"no Lightroom exports for {stem} in {lr_dir}")
    ev = float(re.search(r"_sky-exp_(-?[\d.]+)\.", os.path.basename(masked)).group(1))
    return orig, masked, ev


def lightroom(lr_dir, stem, size):
    """Lightroom's coverage, the original as RGB, and where the coverage can be read.

    Lightroom's tone curve squeezes the mask's exposure to a few levels in a dark sky, so
    the full effect is calibrated per photo and per original brightness: the darkening ratio
    divided by the ratio sure sky of that brightness gets (95th percentile in its luma bin).
    """
    po, pm, ev = lr_files(lr_dir, stem)
    u, m = read_rgb(po, size), read_rgb(pm, size)
    luma = lambda p: 0.299 * p[..., 0] + 0.587 * p[..., 1] + 0.114 * p[..., 2]
    lin = lambda p: 0.2126 * srgb_lin(p[..., 0]) + 0.7152 * srgb_lin(p[..., 1]) + 0.0722 * srgb_lin(p[..., 2])
    lu = luma(u)
    with np.errstate(divide="ignore", invalid="ignore"):
        r = np.nan_to_num(1 - lin(m) / np.maximum(lin(u), 1e-6))
    if ev > 0:  # a brightening mask: use the inverse ratio
        with np.errstate(divide="ignore", invalid="ignore"):
            r = np.nan_to_num(1 - lin(u) / np.maximum(lin(m), 1e-6))
    # Black in the unmasked export says nothing about the mask (0/0 would read as full
    # darkening): count it as unmasked; `valid` below leaves it out of the scores anyway.
    r[lin(u) < 1e-4] = 0
    # Smooth with the sign kept: near black the two exports differ by noise in both
    # directions, which averages out; clipping first would keep only the darker half.
    r = np.clip(ndimage.gaussian_filter(np.clip(r, -1, 1), 1.5), 0, 1)
    bins = np.clip((lu / 8).astype(int), 0, 31)
    glob_full = np.percentile(r[r > 0.05], 95) if (r > 0.05).sum() > 1000 else 0.9
    # Calibrate each band only on clearly darkened pixels; a band without enough of them
    # (no sky that dark, e.g. a black forest) borrows from its neighbours instead of
    # scaling its own noise up to full coverage.
    known, values = [], []
    for b in range(32):
        sel = (bins == b) & (r > 0.4 * glob_full)
        if sel.sum() > 500:
            known.append(b)
            values.append(np.percentile(r[sel], 95))
    full = np.interp(np.arange(32), known, values) if known else np.full(32, glob_full)
    rmax = full[bins]
    valid = (lu >= 8) & (lu < 252) & (rmax > 0.08)  # below 8 of 255 the exports are noise
    cov = np.clip(r / np.maximum(rmax, 1e-6), 0, 1)
    return u, cov, valid, {"original": os.path.basename(po), "masked": os.path.basename(pm), "ev": ev}


def _phase_shift(a, b):
    """Shift (dx, dy) and peak strength with a(p) ≈ b(p + d), by phase correlation."""
    win = np.outer(np.hanning(a.shape[0]), np.hanning(a.shape[1]))
    fa, fb = np.fft.fft2((a - a.mean()) * win), np.fft.fft2((b - b.mean()) * win)
    r = np.fft.ifft2(fb * np.conj(fa) / (np.abs(fb * np.conj(fa)) + 1e-9)).real
    y, x = np.unravel_index(np.argmax(r), r.shape)
    h, w = r.shape
    return (x - w if x > w // 2 else x), (y - h if y > h // 2 else y), float(r.max())


def _edges(rgb):
    g = np.sqrt(np.maximum(0.299 * rgb[..., 0] + 0.587 * rgb[..., 1] + 0.114 * rgb[..., 2], 0))
    gy, gx = np.gradient(g)
    return np.hypot(gx, gy)


def _tile_shifts(a, b, tile=192, step=128):
    H, W = a.shape
    pts = []
    for y in range(0, H - tile + 1, step):
        for x in range(0, W - tile + 1, step):
            dx, dy, q = _phase_shift(a[y:y + tile, x:x + tile], b[y:y + tile, x:x + tile])
            if q > 0.08 and abs(dx) < tile // 3 and abs(dy) < tile // 3:
                pts.append((x + tile / 2, y + tile / 2, dx, dy))
    return np.array(pts).reshape(-1, 4)


def _radial(x, y, W, H):
    """Design matrix for a lens-distortion displacement: radial scale (k0 + k1 r² + k2 r⁴)
    about the frame centre plus a shift. As columns for dx and dy stacked."""
    s = max(W, H) / 2
    u, v = (x - W / 2) / s, (y - H / 2) / s
    r2 = u * u + v * v
    one, zero = np.ones_like(u), np.zeros_like(u)
    ax = np.stack([u, u * r2, u * r2 * r2, one, zero], -1)
    ay = np.stack([v, v * r2, v * r2 * r2, zero, one], -1)
    return ax, ay


def registration(input_path, lr_rgb):
    """Where each Lightroom pixel lies in RAWmakase's frame, as (rows, cols) for map_coordinates,
    or None when the two already agree. Lightroom always applies a Nikon Z lens's built-in
    distortion profile; RAWmakase's render here is uncorrected, so the frames differ by a few
    per cent of scale plus distortion. Fitted as a smooth cubic displacement field from
    tile-wise phase correlation of edge images, with outliers dropped."""
    H, W = lr_rgb.shape[:2]
    inp = np.asarray(Image.open(input_path).convert("RGB").resize((W, H), Image.BILINEAR), np.float32)
    pts = _tile_shifts(_edges(lr_rgb), _edges(inp))
    info = {"tiles": int(len(pts))}
    if len(pts) < 6:
        return None, {**info, "applied": False}
    keep = np.ones(len(pts), bool)
    for _ in range(3):
        ax, ay = _radial(pts[keep, 0], pts[keep, 1], W, H)
        k = np.linalg.lstsq(np.r_[ax, ay], np.r_[pts[keep, 2], pts[keep, 3]], rcond=None)[0]
        ax, ay = _radial(pts[:, 0], pts[:, 1], W, H)
        res = np.hypot(ax @ k - pts[:, 2], ay @ k - pts[:, 3])
        keep = res <= max(2.0, 3 * np.median(res[keep]))
    yy, xx = np.mgrid[0:H, 0:W].astype(np.float32)
    ax, ay = _radial(xx.ravel(), yy.ravel(), W, H)
    dx, dy = (ax @ k).reshape(H, W), (ay @ k).reshape(H, W)
    info.update(max_shift_px=float(np.hypot(dx, dy).max()), fit_residual_px=float(np.median(res[keep])))
    if info["max_shift_px"] < 1.0:
        return None, {**info, "applied": False}
    coords = np.stack([yy + dy, xx + dx])
    # Check: the warped render should now line up with Lightroom's.
    warped = np.stack([ndimage.map_coordinates(inp[..., c], coords, order=1, mode="nearest") for c in range(3)], -1)
    after = _tile_shifts(_edges(lr_rgb), _edges(warped))
    info.update(applied=True, residual_after_px=float(np.median(np.hypot(after[:, 2], after[:, 3]))) if len(after) else None)
    return coords, info


def boundary(mask):
    return mask ^ ndimage.binary_erosion(mask, iterations=1, border_value=1)


def boundary_f(pred, gt, valid, tol):
    bp, bg = boundary(pred) & valid, boundary(gt) & valid
    if bp.sum() == 0 or bg.sum() == 0:
        return float("nan"), float("nan"), float("nan")
    dg = ndimage.distance_transform_edt(~bg)
    dp = ndimage.distance_transform_edt(~bp)
    prec = float((dg[bp] <= tol).mean())
    rec = float((dp[bg] <= tol).mean())
    return prec, rec, (2 * prec * rec / (prec + rec) if prec + rec else 0.0)


def compare(a, b, valid, tol):
    """a = candidate, b = reference, both coverage 0..1."""
    A, B, v = a > 0.5, b > 0.5, valid
    inter, union = (A & B & v).sum(), ((A | B) & v).sum()
    near = ndimage.binary_dilation(boundary(B), iterations=int(tol * 3)) & v
    p, r, f = boundary_f(A, B, v, tol)
    n = max(v.sum(), 1)
    return {
        "iou": float(inter / max(union, 1)),
        "soft_iou": float(np.minimum(a, b)[v].sum() / max(np.maximum(a, b)[v].sum(), 1e-6)),
        "boundary_precision": p,
        "boundary_recall": r,
        "boundary_f": f,
        "mae_border_band": float(np.abs(a - b)[near].mean()) if near.any() else float("nan"),
        "missed_sky_pct": float(100 * (B & ~A & v).sum() / n),
        "false_sky_pct": float(100 * (A & ~B & v).sum() / n),
    }


def tint(rgb, cov):
    red = np.array([255, 30, 30], np.float32)
    out = rgb * (1 - 0.55 * cov[..., None]) + red * 0.55 * cov[..., None]
    out[ndimage.binary_dilation(boundary(cov > 0.5), iterations=1)] = [255, 255, 0]
    return out.clip(0, 255).astype(np.uint8)


def label(img, text):
    im = Image.fromarray(img)
    d = ImageDraw.Draw(im)
    try:
        font = ImageFont.truetype("arialbd.ttf", max(18, im.width // 40))
    except OSError:
        font = ImageFont.load_default()
    d.rectangle([0, 0, im.width, font.size + 12], fill=(0, 0, 0))
    d.text((8, 4), text, fill=(255, 255, 255), font=font)
    return im


def grid(tiles, cols):
    w, h = tiles[0].size
    rows = (len(tiles) + cols - 1) // cols
    sheet = Image.new("RGB", (cols * w + (cols - 1) * 8, rows * h + (rows - 1) * 8), (40, 40, 40))
    for i, t in enumerate(tiles):
        sheet.paste(t, ((i % cols) * (w + 8), (i // cols) * (h + 8)))
    return sheet


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--run", required=True)
    ap.add_argument("--lr", required=True)
    ap.add_argument("--masks", action="append", required=True, help="LABEL=DIR, baseline first, candidate last")
    ap.add_argument("--name", action="append", default=[], help="LABEL=display name for the named panels")
    ap.add_argument("--seed", type=int, default=801)
    args = ap.parse_args()
    masks = dict(m.split("=", 1) for m in args.masks)
    display = {"lightroom": "Lightroom", **dict(n.split("=", 1) for n in args.name)}
    named_out = os.path.join(args.run, "panels")
    os.makedirs(named_out, exist_ok=True)
    labels = list(masks)
    baseline, candidate = labels[0], labels[-1]
    stems = [os.path.splitext(os.path.basename(l.strip()))[0]
             for l in open(os.path.join(args.run, "photos.txt"), encoding="utf-8") if l.strip()]
    rng = random.Random(args.seed)
    out = os.path.join(args.run, "review")
    os.makedirs(out, exist_ok=True)
    metrics, key = {}, {}
    for stem in stems:
        W, H = image_size(lr_files(args.lr, stem)[0])
        s = EVAL_EDGE / max(W, H)
        size = (round(W * s), round(H * s))
        tol = 0.0075 * np.hypot(*size)  # DAVIS-style tolerance
        rgb, lr, valid, files = lightroom(args.lr, stem, size)
        for l, d in masks.items():  # a different crop would misalign every comparison
            mw, mh = image_size(os.path.join(d, stem + ".png"))
            if abs(mw / mh - W / H) > 0.01 * W / H:
                print(f"WARNING {stem}: {l} mask is {mw}x{mh}, Lightroom export {W}x{H}: different framing")
        src = {l: read_gray(os.path.join(d, stem + ".png"), size) for l, d in masks.items()}
        # Bring RAWmakase's masks into Lightroom's frame when the renders differ (lens profile).
        inputs = [os.path.join(d, stem + ".input.jpg") for d in masks.values()]
        inputs = [p for p in inputs if os.path.exists(p)]
        reg = {"tiles": 0, "applied": False, "note": "no RAWmakase input saved"}
        if inputs:
            coords, reg = registration(inputs[-1], rgb)
            if coords is not None:
                src = {l: ndimage.map_coordinates(v, coords, order=1, mode="nearest") for l, v in src.items()}
                print(f"  registered {stem}: max shift {reg['max_shift_px']:.1f} px, "
                      f"residual {reg['residual_after_px']:.1f} px")
        src["lightroom"] = lr
        m = {"lightroom_files": files, "valid_pct": float(100 * valid.mean()), "registration": reg,
             "lightroom_sky_pct": float(100 * (lr > 0.5)[valid].mean())}
        for l in labels:
            m[f"{l}_vs_lightroom"] = compare(src[l], lr, valid, tol)
        if candidate != baseline:
            m[f"{candidate}_vs_{baseline}"] = compare(src[candidate], src[baseline], np.ones_like(valid), tol)
        metrics[stem] = m

        order = labels + ["lightroom"]
        named = list(order)  # fixed order for the report: baseline, ..., candidate, Lightroom
        rng.shuffle(order)
        letters = dict(zip(LETTERS, order))
        key[stem] = letters
        # Zooms where the sources disagree most
        stack = np.stack([src[k] for k in order])
        cw = max(size) // 5
        dsum = ndimage.uniform_filter(stack.max(0) - stack.min(0), cw)
        crops = []
        for _ in range(3):
            y, x = np.unravel_index(np.argmax(dsum), dsum.shape)
            y0 = int(np.clip(y - cw // 2, 0, size[1] - cw)); x0 = int(np.clip(x - cw // 2, 0, size[0] - cw))
            dsum[max(0, y0 - cw):y0 + 2 * cw, max(0, x0 - cw):x0 + 2 * cw] = -1
            crops.append((slice(y0, y0 + cw), slice(x0, x0 + cw)))
        # Blind panels for the reviewers (review/), named ones for people (panels/).
        short = stem[:40]
        for folder, entries in ((out, [(f"mask {L}", s_) for L, s_ in letters.items()]),
                                (named_out, [(display.get(s_, s_), s_) for s_ in named])):
            tiles = [label(rgb.astype(np.uint8), f"{short}: original")]
            tiles += [label(tint(rgb, src[s_]), f"{short}: {t}") for t, s_ in entries]
            g = grid(tiles, 2)
            g.thumbnail((2400, 2400))
            g.save(os.path.join(folder, f"{stem}__overview.jpg"), quality=88)
            for ci, sl in enumerate(crops):
                ts = [label(rgb[sl].astype(np.uint8), f"crop {ci+1}: original")]
                ts += [label(tint(rgb[sl], src[s_][sl]), f"crop {ci+1}: {t}") for t, s_ in entries]
                grid([t.resize((700, 700), Image.LANCZOS) for t in ts], len(ts)).save(
                    os.path.join(folder, f"{stem}__crop{ci+1}.jpg"), quality=90)
        print(stem[:40], f"valid {m['valid_pct']:.0f}%", files["original"])
        for k, c in m.items():
            if isinstance(c, dict) and "iou" in c:
                print(f"  {k:28} IoU {c['iou']:.3f} soft {c['soft_iou']:.3f} BF {c['boundary_f']:.3f} "
                      f"missed {c['missed_sky_pct']:.2f}% false {c['false_sky_pct']:.2f}%")
    json.dump({"baseline": baseline, "candidate": candidate,
               "display": {s_: display.get(s_, s_) for s_ in labels + ["lightroom"]}, "photos": metrics},
              open(os.path.join(args.run, "metrics.json"), "w"), indent=1)
    json.dump(key, open(os.path.join(args.run, "key.json"), "w"), indent=1)


if __name__ == "__main__":
    main()
