"""Random photo-like probe scenes for fitting the scene tone stage's local operators.

Each scene is a log-luminance field built from layers a photo has at different
scales: a smooth gradient (sky, light falloff), large and small regions with sharp
or soft edges (objects, windows, shadows), texture at several frequencies, and a
little colour. Its key (overall brightness) and range vary from scene to scene, so
dark and bright photos are both represented. Scenes are deterministic in their
seed and returned as scene-linear ProPhoto RGB for `probe_dng.write`.
"""
import numpy as np


def _blur(a, sigma):
    """Gaussian blur, periodic at the edges (it only shapes random noise)."""
    if sigma < 0.5:
        return a
    fy = np.fft.fftfreq(a.shape[0])[:, None]
    fx = np.fft.rfftfreq(a.shape[1])[None, :]
    gain = np.exp(-2 * (np.pi * sigma) ** 2 * (fx ** 2 + fy ** 2))
    return np.fft.irfft2(np.fft.rfft2(a) * gain, a.shape)


def scene(seed, width=1536, height=1024):
    """Scene `seed`: (image, description)."""
    rng = np.random.default_rng(seed)
    yy, xx = np.mgrid[0:height, 0:width] / max(width, height)
    key = rng.uniform(-6.5, -0.5)          # log2 of the scene's middle level
    spread = rng.uniform(1.0, 4.0)          # how far regions stray from it, in stops
    L = np.full((height, width), key)
    # Gradient.
    angle = rng.uniform(0, 2 * np.pi)
    L += rng.uniform(0, 2.5) * (np.cos(angle) * xx + np.sin(angle) * yy)
    # Regions.
    for _ in range(rng.integers(4, 30)):
        size = np.exp(rng.uniform(np.log(0.01), np.log(0.5)))
        cx, cy = rng.uniform(0, width / max(width, height)), rng.uniform(0, height / max(width, height))
        level = rng.normal(0, spread)
        soft = rng.choice([0., 0.002, 0.02])
        if rng.random() < 0.5:
            d = np.maximum(np.abs(xx - cx) / rng.uniform(0.3, 1), np.abs(yy - cy) / rng.uniform(0.3, 1)) - size / 2
        else:
            d = np.hypot(xx - cx, yy - cy) - size / 2
        mask = 1 / (1 + np.exp(np.clip(d / max(soft, 1e-4), -60, 60)))
        L = L * (1 - mask) + (key + level) * mask
    # Highlights: a few small bright spots (lamps, specular) in some scenes.
    if rng.random() < 0.5:
        for _ in range(rng.integers(1, 6)):
            size = np.exp(rng.uniform(np.log(0.003), np.log(0.05)))
            cx, cy = rng.uniform(0, 1), rng.uniform(0, height / width)
            mask = (np.hypot(xx - cx, yy - cy) < size / 2).astype(float)
            L = np.where(mask > 0, max(key + 4, rng.uniform(-1, 2)), L)
    # Texture at several scales.
    for scale_px, amount in [(1.5, rng.uniform(0, 0.3)), (6, rng.uniform(0, 0.4)), (24, rng.uniform(0, 0.5))]:
        n = _blur(rng.normal(0, 1, (height, width)), scale_px)
        L += amount * n / max(n.std(), 1e-6)
    L = np.clip(L, -14, 3)
    Y = 2.0 ** L
    # Colour: a smooth chroma field.
    chroma = np.stack([_blur(rng.normal(0, 1, (height, width)), 60) for _ in range(3)], -1)
    chroma /= max(chroma.std(), 1e-6)
    rgb = Y[..., None] * np.exp(rng.uniform(0, 0.15) * chroma)
    # Keep the luminance (ProPhoto Y weights).
    y = rgb @ np.array([0.2880402, 0.7118741, 0.0000857])
    rgb *= (Y / np.maximum(y, 1e-12))[..., None]
    return rgb, dict(seed=seed, key=float(key), spread=float(spread))
