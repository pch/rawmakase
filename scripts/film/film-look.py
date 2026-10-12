#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy==2.5.3", "scipy==1.18.1", "colour-science==0.4.7", "pdfplumber==0.11.10"]
# ///
"""Make film look profiles from films' and papers' datasheet curves.

Reads a stock's curves with `extract-datasheet.py` (kept in `data/`, read from the
datasheet when missing) and writes a Camera Raw look profile, an XMP with a 3D RGB
table, which RAWmakase and Lightroom both import, with a develop preset choosing it.

    uv run scripts/film/film-look.py --all              # every look RAWmakase ships
    uv run scripts/film/film-look.py portra-400 -o ~/film-looks
    uv run scripts/film/film-look.py portra-400 --print --push 1 -o ~/film-looks

The model follows the light, as a film would see it:

1. The table's input is the photo after the base profile's tone curve. Inverting
   that curve gives the scene light the camera measured (linear ProPhoto, white
   balanced), with a correctly exposed mid gray at `MID_GRAY` after the curve.
2. Each colour becomes a plausible spectrum (Jakob and Hanika's sigmoid model,
   2019), which exposes the film's three layers through their published spectral
   sensitivities, under daylight (D55). Mid gray exposes every layer at the
   datasheet's reference exposure: its "Log H Ref" when printed, else the one its
   speed gives (`REF_H_ISO`, from Portra 400's).
3. The characteristic curves turn each layer's exposure into density, so
   exposure decides where a colour sits on the film's toe and shoulder: the
   Exposure slider moves a photo along the film's response, as over- and
   underexposing film does. `--push N` exposes the film N stops over (or under),
   then scans or prints it balanced on gray as a lab would: the same brightness,
   another part of the curve.
4. The developed film becomes a picture one of three ways:
   - A negative scanned: densities are inverted channel by channel with each
     channel's own gamma (so the straight part of the curve stays neutral, as a
     scanner balanced on mid gray does), the film base set to black, and the three
     channels written out as sRGB, as a lab scanner outputs them: nothing corrects
     the film's colour back toward the camera's. The base tone curve renders it.
   - A slide projected: each layer's density becomes an amount of its dye, the dyes'
     spectra filter the projector's light (the datasheet's viewing illuminant, else
     5000 K), and the eye adapts to the film's clear base.
   - Either printed (`--print`): an enlarger's 3200 K lamp shines through the
     film's dyes onto the paper, whose layers take it through their own spectral
     sensitivities, the printer's filtration set so a correctly exposed gray prints
     as a neutral mid gray. The paper's curves and dyes, seen under D50 with 1%
     viewing flare (`PRINT_FLARE`), make the print. Negatives print on Endura
     Premier, slides on Ektachrome Radiance III (`PAPERS`).

Where a datasheet publishes only the total density of a midscale neutral and of
D-min (as Kodak's negatives do), the three dyes are estimated from it: smooth bands
with their peaks in the yellow, magenta and cyan ranges, fitted to the difference.

Every look gets a develop preset in the RMKS Film group choosing it with the film's
grain: from its published rms granularity, scaled by how much the scan, projection
or print stretches the film's contrast, else estimated from its speed.

What the datasheets don't give is approximated: Status M and Status A densitometry
as narrow bands at each dye's peak, the negatives' individual dyes, the scanner's
colour (sRGB) and the printer's lamp, and there is no halation or interlayer effect.
"""
import argparse
import hashlib
import importlib.util
import struct
import sys
import warnings
from pathlib import Path

import numpy as np

warnings.filterwarnings('ignore', module='colour')
import colour  # noqa: E402
from colour.recovery import find_coefficients_Jakob2019, sd_Jakob2019  # noqa: E402

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]



def script(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


# The DNG SDK's RGB table writer, shared with the synthetic test looks, and the
# datasheet reader.
synthetic_looks = script('synthetic_looks', ROOT / 'scripts/corpus/synthetic-looks.py')
extract = script('extract_datasheet', HERE / 'extract-datasheet.py')

SHAPE = colour.SpectralShape(380, 730, 5)
WL = np.arange(SHAPE.start, SHAPE.end + 1, SHAPE.interval, dtype=float)
CMFS = colour.MSDS_CMFS['CIE 1931 2 Degree Standard Observer'].copy().align(SHAPE)
D50 = colour.SDS_ILLUMINANTS['D50'].copy().align(SHAPE)
D55 = colour.SDS_ILLUMINANTS['D55'].copy().align(SHAPE)
PRO = colour.RGB_COLOURSPACES['ProPhoto RGB']
PRO_TO_XYZ = PRO.matrix_RGB_to_XYZ
XYZ_TO_PRO = PRO.matrix_XYZ_to_RGB
# The group the looks and their presets are listed under, and the looks' name prefix.
GROUP = 'RMKS Film'
PREFIX = GROUP + ': '
# The paper `--print` uses for each kind of film: Kodak's papers for printing
# negatives (RA-4) and slides (R-3).
PAPERS = {
    'negative': 'endura-premier',
    'reversal': 'radiance-iii',
}
# Viewing flare on a print: light reflected off its surface, as a fraction of its white.
PRINT_FLARE = 0.01
# The enlarger lamp a print is exposed with, and a slide's viewing light when its
# datasheet names none.
ENLARGER_K = 3200
VIEWING_K = 5000
# Exposure (lux-seconds) times speed for a correctly exposed mid gray on a negative:
# Portra 400's printed Log H Ref, -1.44, at ISO 400. Negatives without one use it.
REF_H_ISO = 10 ** -1.44 * 400
# RGB table divisions per side: 25 renders within ΔE00 0.1 on average (0.5 at the
# 99th percentile) of 33 at under half the size.
DIVISIONS = 25
# The push and pull variants of a negative, in stops.
PUSHES = (1, -1)
# RAWmakase's grain adds 0.93% luminance noise (rms over mean) per unit of Amount at
# mid gray (Size 25, measured on a 970 px render; it scales with the photo's long
# edge, as a frame's grain does). A 48 µm granularity aperture is 1.15 times the
# side of a 970 px frame's pixel on 35 mm film.
GRAIN_NOISE_PER_AMOUNT = 0.0093
GRAIN_APERTURE = 1.15
GRAIN_SIZE, GRAIN_ROUGHNESS = 25, 50
# A Print Grain Index (35 mm at 4×6 in) and the Amount its film's rms gives: Portra
# 400's 37, set to Fujicolor Pro 400H's Amount (rms 4), a film of its class.
PGI_REF = (37, 3)
# Mid gray as the base tone curve shows it: L* 50.
MID_GRAY = 0.1842


def base_tone_curve():
    """The DNG SDK's default tone curve, as RAWmakase applies it for profiles without
    their own (RAWmakase Standard and Adobe Standard): 1025 samples, linear in and
    out. Read from the renderer's source, so the look inverts exactly that curve."""
    src = (ROOT / 'crates/rawmakase-model/src/camera_profiles/dng_tone.rs').read_text()
    body = src[src.index('DEFAULT_TONE'):]
    body = body[body.index('[', body.index('=')) + 1:body.index('];')]
    values = np.array([float(v) for v in body.replace('\n', ' ').split(',') if v.strip()])
    assert len(values) == 1025
    return np.linspace(0, 1, 1025), values


TONE_X, TONE_Y = base_tone_curve()


def tone(v):
    return np.interp(np.clip(v, 0, 1), TONE_X, TONE_Y)


def untone(v):
    return np.interp(np.clip(v, 0, 1), TONE_Y, TONE_X)


def hue_preserving(rgb, f):
    """Apply a curve to the darkest and brightest channels and keep the middle one's
    relative position, as RAWmakase's profile tone curve does (`CameraProfile::finish`)."""
    rgb = np.clip(rgb, 0, None)
    lo = rgb.min(axis=-1, keepdims=True)
    hi = rgb.max(axis=-1, keepdims=True)
    a, b = f(lo), f(hi)
    span = hi - lo
    t = np.where(span > 1e-8, (rgb - lo) / np.where(span > 1e-8, span, 1), 0)
    return a + (b - a) * t


SCENE_GRAY = float(untone(MID_GRAY))


def srgb_encode(v):
    v = np.clip(v, 0, 1)
    return np.where(v <= 0.0031308, 12.92 * v, 1.055 * np.power(v, 1 / 2.4) - 0.055)


def srgb_decode(v):
    return np.where(v <= 0.04045, v / 12.92, np.power((v + 0.055) / 1.055, 2.4))


class Curve:
    """A datasheet curve as a function, extended past its ends: flat or straight."""

    def __init__(self, points, extend='flat'):
        p = np.array(points, dtype=float)
        self.x, self.y = p[:, 0], p[:, 1]
        self.extend = extend

    def __call__(self, x):
        y = np.interp(x, self.x, self.y)
        if self.extend == 'straight':
            n = max(3, len(self.x) // 10)
            lo = np.polyfit(self.x[:n], self.y[:n], 1)[0]
            hi = np.polyfit(self.x[-n:], self.y[-n:], 1)[0]
            y = np.where(x < self.x[0], self.y[0] + (x - self.x[0]) * lo, y)
            y = np.where(x > self.x[-1], self.y[-1] + (x - self.x[-1]) * hi, y)
        return y


def characteristic(points, kind):
    """Density against log exposure. A negative's curve continues straight above its
    plotted end (the datasheet stops before the shoulder) and flat below its toe; a
    slide's flattens at both ends."""
    c = Curve(points, 'flat')
    if kind == 'negative':
        hi_x, hi_y = c.x[-8:], c.y[-8:]
        slope = np.polyfit(hi_x, hi_y, 1)[0]

        def f(x):
            return np.where(x > c.x[-1], c.y[-1] + (x - c.x[-1]) * slope, c(x))
        return f
    return c


def sensitivity(points):
    """Linear spectral sensitivity on WL from a log sensitivity curve; zero outside
    the plotted range, where the datasheet shows the layer is blind."""
    c = np.array(points, dtype=float)
    s = 10 ** np.interp(WL, c[:, 0], c[:, 1])
    return np.where((WL >= c[0, 0]) & (WL <= c[-1, 0]), s, 0.)


def spectral(points):
    """A spectral density curve on WL, its ends held flat."""
    c = np.array(points, dtype=float)
    return np.clip(np.interp(WL, c[:, 0], c[:, 1]), 0, None)


CHANNELS = ('red', 'green', 'blue')
DYES = ('cyan', 'magenta', 'yellow')


class Film:
    """A film's or paper's curves: characteristic curves per channel, the layers'
    spectral sensitivities and its dyes' spectra."""

    def __init__(self, data):
        self.data = data
        self.kind = data['kind']
        curves = data['curves']
        self.curves = [characteristic(curves['characteristic'][c], self.kind) for c in CHANNELS]
        # Linear spectral sensitivity of the red, green and blue layers.
        self.layers = np.stack([sensitivity(curves['sensitivity'][c]) for c in CHANNELS])
        # A flat spectrum under daylight exposes every layer equally, at 1: the
        # characteristic curves were measured on neutrals under daylight.
        lit = self.layers * D55.values
        self.sens = lit / lit.sum(axis=1, keepdims=True)
        self.eps, self.base, self.calibration = dye_spectra(curves['dye_density'])

    def exposure(self, reflectance):
        """Layer exposures of spectra (…, WL), relative to a white reflector."""
        return reflectance @ self.sens.T


def dye_spectra(dd):
    """The dyes' spectra (cyan, magenta, yellow, by wavelength), the base's, and how a
    unit of each relates to a neutral: `('neutral', None)` when the datasheet's dyes
    are normalized to form one, `('solve', None)` when a neutral must be found, or
    `('reference', amounts)` for dyes estimated from a midscale neutral, which those
    amounts form at the film's reference exposure."""
    base = spectral(dd['minimum']) if 'minimum' in dd else np.zeros(len(WL))
    if all(d in dd for d in DYES):
        eps = np.stack([spectral(dd[d]) for d in DYES])
        if 'visual_neutral' in dd:
            neutral = spectral(dd['visual_neutral'])
            inside = (WL >= 420) & (WL <= 700)
            if np.abs(eps.sum(axis=0) - neutral)[inside].max() < 0.1 * neutral[inside].max():
                return eps, base, ('neutral', None)
        return eps, base, ('solve', None)
    eps, amounts = decompose(np.array(dd['midscale_neutral'], dtype=float),
                             np.array(dd['minimum'], dtype=float) if 'minimum' in dd else None)
    return eps, base, ('reference', amounts)


# Where each estimated dye peaks, in nm: cyan, magenta, yellow.
DYE_PEAKS = ((600., 700.), (510., 580.), (420., 480.))


def bump(wl, peak, left, right):
    """A smooth absorption band: Gaussian, with a width of its own on either side."""
    return np.exp(-0.5 * ((wl - peak) / np.where(wl < peak, left, right)) ** 2)


def decompose(midscale, minimum):
    """Three dyes that add up to a midscale neutral's density above D-min, each a
    smooth band peaking in its colour's range. Returns their spectra on WL, each
    peaking at 1, and their amounts in the neutral."""
    from scipy.optimize import least_squares
    wl = midscale[:, 0]
    target = midscale[:, 1] - (np.interp(wl, minimum[:, 0], minimum[:, 1]) if minimum is not None else 0)

    def model(p, x):
        return sum(p[4 * i] * bump(x, *p[4 * i + 1:4 * i + 4]) for i in range(3))

    p0, lo, hi = [], [], []
    for a, b in DYE_PEAKS:
        p0 += [target.max() / 2, (a + b) / 2, 35., 35.]
        lo += [0., a, 15., 15.]
        hi += [5., b, 90., 90.]
    fit = least_squares(lambda p: model(p, wl) - target, p0, bounds=(lo, hi))
    p = fit.x
    eps = np.stack([bump(WL, *p[4 * i + 1:4 * i + 4]) for i in range(3)])
    return eps, np.array([p[4 * i] for i in range(3)])


# The brightness spectra are found at: the ProPhoto value of a dark-ish surface. Bright
# saturated colours have no smooth reflectance spectrum; real ones are darker.
SPECTRUM_VALUE = 0.2
N_HUE, N_SAT = 72, 25


def jakob_shapes():
    """Spectra for a grid of chromaticities (HSV hue and saturation of linear ProPhoto
    at SPECTRUM_VALUE), from Jakob and Hanika's sigmoid model under D50, the white
    ProPhoto is balanced to, and their colours' remaining XYZ error, for colours no
    smooth reflectance reaches (ProPhoto's corners). The same for every film, so kept
    in the user's cache."""
    cache = Path.home() / '.cache' / 'rawmakase-film' / f'spectra-{N_HUE}x{N_SAT}-{SPECTRUM_VALUE}.npz'
    if cache.exists():
        z = np.load(cache)
        return z['spectra'], z['residual']
    spectra = np.zeros((N_HUE, N_SAT, len(WL)))
    residual = np.zeros((N_HUE, N_SAT, 3))
    for i in range(N_HUE):
        coefficients = np.zeros(3)
        for j in range(N_SAT):
            rgb = colour.HSV_to_RGB([i / N_HUE, j / (N_SAT - 1), SPECTRUM_VALUE])
            xyz = PRO_TO_XYZ @ rgb
            if j == 0:
                coefficients = np.zeros(3)
            coefficients, _ = find_coefficients_Jakob2019(xyz, CMFS, D50, coefficients_0=coefficients)
            r = sd_Jakob2019(coefficients, SHAPE).values
            spectra[i, j] = r
            residual[i, j] = xyz - reflectance_xyz(r)
        print(f'\rspectra {i + 1}/{N_HUE}', end='', file=sys.stderr)
    print(file=sys.stderr)
    cache.parent.mkdir(parents=True, exist_ok=True)
    np.savez(cache, spectra=spectra, residual=residual)
    return spectra, residual


def reflectance_xyz(r):
    """XYZ of reflectances under D50, white at Y = 1."""
    w = D50.values[:, None] * CMFS.values
    return (np.asarray(r) @ w) / w[:, 1].sum()


def colorchecker():
    sds = colour.SDS_COLOURCHECKERS['BabelColor Average']
    return np.stack([sd.copy().align(SHAPE).values for sd in sds.values()])


def blackbody(k):
    return colour.sd_blackbody(k, SHAPE).values


class Model:
    def __init__(self, film, paper=None, push=0):
        self.film = film
        spectra, residual = jakob_shapes()
        # Colours a smooth spectrum can't reach keep their difference linearly, through
        # the best linear map from XYZ to layer exposure over the ColorChecker.
        cc = colorchecker()
        k = np.linalg.lstsq(reflectance_xyz(cc), film.exposure(cc), rcond=None)[0].T
        # Exposures per unit of linear ProPhoto value.
        self.unit = np.maximum(film.exposure(spectra) + residual @ k.T, 0) / SPECTRUM_VALUE
        if film.kind == 'negative':
            data = film.data
            self.log_h_ref = data.get('log_h_ref', np.log10(REF_H_ISO / data['iso']))
        else:
            if push:
                raise ValueError('push and pull are for negatives: a slide is its own picture')
            light = blackbody(film.data.get('viewing_illuminant_k', VIEWING_K))
            self.film_dyes = Dyes(film, light)
            self.log_h_ref = self.film_dyes.reference_exposure()
        # Where a correctly exposed gray lands on the film: over or under by `push`.
        self.log_h_gray = self.log_h_ref + push * np.log10(2)
        self.d_gray = density(film.curves, np.full(3, self.log_h_gray))
        x = self.log_h_ref + np.linspace(-0.6, 0.6, 13)
        self.gamma = np.array([np.polyfit(x, c(x), 1)[0] for c in film.curves])
        self.paper = paper
        if paper is not None:
            if film.kind == 'negative':
                self.film_dyes = Dyes(film, blackbody(ENLARGER_K), reference=self.log_h_ref)
            self.printer = Printer(self.film_dyes, paper, self.d_gray)
        elif film.kind == 'negative':
            # Scanned: each channel inverted with its own gamma, balanced on gray, the
            # film base set to black, and the channels written out as sRGB. No
            # calibration pulls colours back toward the camera's.
            self.matrix = colour.matrix_RGB_to_RGB(colour.RGB_COLOURSPACES['sRGB'], PRO,
                                                   chromatic_adaptation_transform='Bradford')
            self.d_min = np.array([c(np.array(-10.)) for c in film.curves])

    def layer_exposure(self, scene):
        """Log exposure of each layer for scene-linear ProPhoto colours, with mid gray
        at `log_h_gray`."""
        scene = np.clip(scene, 0, None)
        hsv = colour.RGB_to_HSV(scene)
        h = hsv[..., 0] * N_HUE
        s = hsv[..., 1] * (N_SAT - 1)
        h0 = np.floor(h).astype(int)
        s0 = np.clip(np.floor(s).astype(int), 0, N_SAT - 2)
        fh, fs = (h - h0)[..., None], np.clip(s - s0, 0, 1)[..., None]
        h0 %= N_HUE
        h1 = (h0 + 1) % N_HUE
        u = self.unit
        unit = ((1 - fh) * (1 - fs) * u[h0, s0] + fh * (1 - fs) * u[h1, s0]
                + (1 - fh) * fs * u[h0, s0 + 1] + fh * fs * u[h1, s0 + 1])
        h = unit * hsv[..., 2:3]
        return self.log_h_gray + np.log10(np.maximum(h, 1e-9) / SCENE_GRAY)

    def display(self, scene):
        """Display-linear ProPhoto of scene-linear ProPhoto colours."""
        d = density(self.film.curves, self.layer_exposure(scene))
        if self.paper is not None:
            return self.printer.display(d)
        if self.film.kind != 'negative':
            return self.film_dyes.view(d)
        positive = SCENE_GRAY * 10 ** ((d - self.d_gray) / self.gamma)
        base = SCENE_GRAY * 10 ** ((self.d_min - self.d_gray) / self.gamma)
        positive = SCENE_GRAY * (positive - base) / (SCENE_GRAY - base)
        return hue_preserving(positive @ self.matrix.T, tone)

    def system_gamma(self):
        """How steeply displayed luminance follows scene luminance at mid gray (log-log)."""
        y = [(self.display(np.full((1, 3), SCENE_GRAY * 2 ** e)) @ PRO_TO_XYZ.T)[0, 1] for e in (-0.5, 0.5)]
        return np.log10(y[1] / y[0]) / np.log10(2)


def density(curves, log_h):
    return np.stack([c(log_h[..., i]) for i, c in enumerate(curves)], axis=-1)


def inverse(curve, d):
    """The log exposure at which a monotonic characteristic curve reaches density d."""
    x, y = curve.x, curve.y
    if y[-1] < y[0]:
        x, y = x[::-1], y[::-1]
    return float(np.interp(d, np.maximum.accumulate(y), x))


class Dyes:
    """A film's or paper's dyes: from its three channels' densities to dye amounts, the
    light they pass, and its colour under the light it is seen in."""

    def __init__(self, film, light, flare=0., reference=None):
        self.eps, self.base, (how, amounts) = film.eps, film.base, film.calibration
        self.curves = film.curves
        self.flare = flare
        self.light = light / (light * CMFS.values[:, 1]).sum()
        # Densitometry as narrow bands at each dye's peak (Status M or A approximated).
        peaks = WL[self.eps.argmax(axis=1)]
        self.bands = np.exp(-0.5 * ((WL[None] - peaks[:, None]) / 15.) ** 2)
        self.bands /= self.bands.sum(axis=1, keepdims=True)
        self.scale = np.ones(3)
        # Densities above the base: with dyes estimated from a midscale neutral, the
        # base (D-min) is its own spectrum; otherwise the dyes carry all the density.
        self.d_base = (np.array([c(np.array(-10.)) for c in self.curves])
                       if how == 'reference' else np.zeros(3))
        if how == 'reference':
            # The estimated dyes form the published midscale neutral at the film's
            # reference exposure: scale each band to read that exposure's densities.
            self.neutral = amounts
            at = density(self.curves, np.full(3, reference)) - self.d_base
        else:
            # The amounts forming a visual neutral of density 1.0: given when the
            # datasheet's dyes are normalized so, else solved for. A material balanced
            # for its light forms a neutral from a gray exposure, so its curves'
            # densities where they average 1.0 are that neutral as the densitometer
            # reads it: scale each band to read so.
            self.neutral = np.ones(3) if how == 'neutral' else self.solve_neutral()
            x = np.linspace(-6, 4, 10001)
            mean = np.mean([c(x) for c in self.curves], axis=0)
            log_h = x[np.argmin(np.abs(mean - 1.0))]
            at = density(self.curves, np.full(3, log_h))
        self.scale = at / self.band_density(self.neutral[None])[0]
        # The eye adapts to the clearest white: a slide's base under the projector, a
        # print's paper white.
        d_min = np.array([c.y.min() if hasattr(c, 'y') else c(np.array(-10.)) for c in self.curves])
        self.white = self.xyz(self.amounts(d_min[None]))[0]

    def solve_neutral(self):
        """Dye amounts whose colour is the light's own at a tenth of its luminance."""
        target = 0.1 * self.xyz(np.zeros(3))
        a = np.ones(3)
        for _ in range(50):
            f = self.xyz(a) - target
            jac = np.stack([(self.xyz(a + e * 1e-5) - self.xyz(a)) / 1e-5 for e in np.eye(3)], axis=-1)
            step = np.linalg.solve(jac, f)
            a = np.maximum(a - step, 0)
            if np.abs(step).max() < 1e-9:
                break
        return a

    def band_density(self, a):
        t = 10 ** -(a @ self.eps)
        return -np.log10(np.maximum(t @ self.bands.T, 1e-12)) * self.scale

    def amounts(self, d):
        """Dye amounts whose band densities are `d` (above the base), by Newton's method."""
        d = np.asarray(d, dtype=float) - self.d_base
        a = np.maximum(d, 0)
        for _ in range(30):
            f = self.band_density(a) - d
            jac = np.stack([(self.band_density(a + e * 1e-4) - self.band_density(a)) / 1e-4
                            for e in np.eye(3)], axis=-1)
            step = np.linalg.solve(jac, f[..., None])[..., 0]
            a = np.maximum(a - step, 0)
            if np.abs(step).max() < 1e-6:
                break
        return a

    def transmittance(self, a):
        return 10 ** -(self.base + a @ self.eps)

    def xyz(self, a):
        return (self.transmittance(a) * self.light) @ CMFS.values

    def view(self, d):
        """Display-linear ProPhoto of densities, white at 1 after adaptation."""
        shape = d.shape
        xyz = self.xyz(self.amounts(d.reshape(-1, 3)))
        xyz = (xyz + self.flare * self.white) / (1 + self.flare)
        xyz = colour.adaptation.chromatic_adaptation_VonKries(
            xyz, self.white, colour.xy_to_XYZ(PRO.whitepoint), 'Bradford')
        return (xyz @ XYZ_TO_PRO.T).reshape(shape)

    def mid_gray_densities(self):
        """The densities of the neutral that shows as mid gray against the white."""
        lo, hi = 0., 5.
        for _ in range(60):
            s = (lo + hi) / 2
            y = (self.xyz(s * self.neutral)[1] / self.white[1] + self.flare) / (1 + self.flare)
            lo, hi = (lo, s) if y < MID_GRAY else (s, hi)
        return self.band_density(((lo + hi) / 2 * self.neutral)[None])[0] + self.d_base

    def reference_exposure(self):
        """The exposure at which a gray shows as mid gray: a correctly exposed slide."""
        lo, hi = -4., 2.
        for _ in range(50):
            mid = (lo + hi) / 2
            d = np.array([[c(np.array(mid)) for c in self.curves]])
            y = (self.view(d) @ PRO_TO_XYZ.T)[0, 1]
            lo, hi = (mid, hi) if y < MID_GRAY else (lo, mid)
        return (lo + hi) / 2


class Printer:
    """An enlarger: its lamp through the film's dyes onto the paper's layers, filtered
    so the film's gray prints as a neutral mid gray, and the print seen under D50."""

    def __init__(self, film_dyes, paper, d_gray):
        self.film_dyes = film_dyes
        self.paper = paper
        self.paper_dyes = Dyes(paper, D50.values, flare=PRINT_FLARE)
        self.lamp = blackbody(ENLARGER_K)
        target = self.paper_dyes.mid_gray_densities()
        log_h = np.array([inverse(c, t) for c, t in zip(paper.curves, target)])
        self.filtration = log_h - np.log10(self.exposure(self.film_dyes.amounts(d_gray[None]))[0])

    def exposure(self, a):
        """Each paper layer's exposure through film of dye amounts `a`."""
        return (self.film_dyes.transmittance(a) * self.lamp) @ self.paper.layers.T

    def display(self, d):
        shape = d.shape
        a = self.film_dyes.amounts(d.reshape(-1, 3))
        log_h = np.log10(np.maximum(self.exposure(a), 1e-30)) + self.filtration
        return self.paper_dyes.view(density(self.paper.curves, log_h)).reshape(shape)


def table(model, divisions):
    """The look's RGB table over display-referred ProPhoto with sRGB encoding: the
    base curve undone, the film model, and the result encoded again."""
    grid = np.linspace(0, 1, divisions)
    r, g, b = np.meshgrid(grid, grid, grid, indexing='ij')
    display = srgb_decode(np.stack([r, g, b], axis=-1))
    scene = hue_preserving(display, untone)
    return srgb_encode(model.display(scene))


def pack(samples):
    """A DNG SDK 3D RGB table: red outermost, then green, then blue, as 16-bit
    differences from the identity (see `synthetic-looks.py`)."""
    n = samples.shape[0]
    grid = (np.arange(n) * 0xFFFF + (n - 1) // 2) // (n - 1)
    ident = np.stack(np.meshgrid(grid, grid, grid, indexing='ij'), axis=-1)
    values = np.round(np.clip(samples, 0, 1) * 0xFFFF).astype(np.int64)
    diff = ((values - ident) & 0xFFFF).astype('<u2')
    return (struct.pack('<4I', 1, 1, 3, n) + diff.tobytes()
            + synthetic_looks.rgb_table_tail(synthetic_looks.PROPHOTO, synthetic_looks.SRGB_GAMMA,
                                             synthetic_looks.CLIP, (0., 2.)))


def look_uuid(name):
    return hashlib.md5(('rawmakase film ' + name).encode()).hexdigest().upper()


def description(attributes, name, group):
    out = ('<x:xmpmeta xmlns:x="adobe:ns:meta/">\n <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">\n'
           '  <rdf:Description rdf:about=""\n    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"')
    out += ''.join(f'\n   crs:{k}="{v}"' for k, v in attributes.items()) + '>\n'
    for element, text in [('Name', name), ('Group', group)]:
        out += (f'   <crs:{element}>\n    <rdf:Alt>\n     <rdf:li xml:lang="x-default">{text}</rdf:li>\n'
                f'    </rdf:Alt>\n   </crs:{element}>\n')
    return out


def xmp(name, sources, rgb):
    uuid = look_uuid(name)
    digest = hashlib.md5(rgb).hexdigest().upper()
    attributes = {
        'PresetType': 'Look', 'Cluster': '', 'UUID': uuid, 'SupportsAmount': 'True',
        'SupportsColor': 'True', 'SupportsMonochrome': 'False',
        'SupportsHighDynamicRange': 'True', 'SupportsNormalDynamicRange': 'True',
        'SupportsSceneReferred': 'True', 'SupportsOutputReferred': 'False',
        'CameraModelRestriction': '',
        'Copyright': 'Generated by RAWmakase scripts/film/film-look.py from ' + '; '.join(sources),
        'ContactInfo': '', 'Version': '18.7', 'ProcessVersion': '15.4',
        'ConvertToGrayscale': 'False', 'RGBTable': digest,
        f'Table_{digest}': synthetic_looks.base85(rgb),
    }
    return description(attributes, name, GROUP) + '  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n'


def preset(name, title, grain):
    """A develop preset named `title` that chooses the look `name` at full Amount and
    adds the film's grain, so the film is also one click in the Presets panel. Other
    settings are left as they are."""
    attributes = {
        'PresetType': 'Normal', 'Cluster': '', 'UUID': hashlib.md5(('rawmakase film preset ' + name).encode()).hexdigest().upper(),
        'SupportsAmount': 'True', 'SupportsColor': 'True', 'SupportsMonochrome': 'False',
        'SupportsHighDynamicRange': 'True', 'SupportsNormalDynamicRange': 'True',
        'SupportsSceneReferred': 'True', 'SupportsOutputReferred': 'False',
        'CameraModelRestriction': '', 'Copyright': 'RAWmakase contributors, MIT licence',
        'ContactInfo': '', 'Version': '18.7', 'ProcessVersion': '15.4',
        'GrainAmount': grain[0], 'GrainSize': grain[1], 'GrainFrequency': grain[2],
        'HasSettings': 'True',
    }
    look = ('   <crs:Look>\n    <rdf:Description\n'
            f'     crs:Name="{name}"\n     crs:Amount="1"\n     crs:UUID="{look_uuid(name)}"\n'
            '     crs:SupportsAmount="true"/>\n   </crs:Look>\n')
    return description(attributes, title, GROUP) + look + '  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n'


def grain(data, model):
    """The preset's Grain (Amount, Size, Roughness) and where its Amount comes from.
    A published rms granularity (σD × 1000 through a 48 µm aperture) is film density
    noise; divided by the film's gamma it is exposure noise, which the scan,
    projection or print stretches by the system's gamma. Kodak's negatives publish a
    Print Grain Index instead (35 mm at 4×6 in; 4 units are one just-noticeable
    step): each step is 1.2 times the grain of PGI_REF, a film whose rms gives its
    Amount. Without either, the Amount is estimated from the film's speed."""
    published = data.get('grain', {})
    pgi = {k: v for k, v in published.get('print_grain_index', {}).items() if '135' in k and '4x6' in k}
    if 'rms' in published:
        film_gamma = np.abs(model.gamma).mean()
        noise = np.log(10) * published['rms'] / 1000 * GRAIN_APERTURE * model.system_gamma() / film_gamma
        amount, how = noise / GRAIN_NOISE_PER_AMOUNT, f'rms granularity {published["rms"]}'
    elif pgi:
        value, = pgi.values()
        index = 21 if str(value).startswith('<') else float(value)
        amount = PGI_REF[1] * 1.2 ** ((index - PGI_REF[0]) / 4)
        how = f'print grain index {value}'
    else:
        amount, how = 2 + 3 * np.log2(data['iso'] / 100), f'estimated from ISO {data["iso"]}'
    return (int(round(min(max(amount, 1), 40))), GRAIN_SIZE, GRAIN_ROUGHNESS), how


def report(name, model):
    """How mid gray, an exposure sweep and a few colours come out."""
    cc = colorchecker()
    scene = reflectance_xyz(cc) @ XYZ_TO_PRO.T * (SCENE_GRAY / 0.18)
    lab = lambda rgb: colour.XYZ_to_Lab(np.clip(rgb, 0, None) @ PRO_TO_XYZ.T, PRO.whitepoint)
    before, after = lab(hue_preserving(scene, tone)), lab(model.display(scene))
    de = colour.delta_E(before, after, method='CIE 2000')
    grays = ' '.join(f'{e:+d}:{srgb_encode(model.display(np.full((1, 3), min(SCENE_GRAY * 2 ** e, 1.))))[0].round(2)}'
                     for e in (-3, 0, 2))
    lch = lambda v: f'{v[0]:.0f}/{np.hypot(*v[1:]):.0f}/{np.degrees(np.arctan2(v[2], v[1])) % 360:.0f}'
    return (f'{name}: log H gray {model.log_h_gray:.2f}, gray {grays}; ColorChecker ΔE00 {de.mean():.1f}; '
            f'green {lch(before[13])} -> {lch(after[13])}, skin {lch(before[1])} -> {lch(after[1])}')


def look_name(data, paper, push):
    return PREFIX + data['look'] + (' Print' if paper else '') + (f' {push:+d}' if push else '')


def safe(name):
    """A file name without characters macOS or Windows refuse."""
    return ''.join(c for c in name if c not in ':/\\?*"<>|')


def build(stock, paper_name=None, push=0, out=Path('.'), presets=None, divisions=None, name=None):
    """Write one look and its preset; returns a line of report."""
    data = extract.datasheet(stock)
    sources = [data['source']]
    paper = None
    if paper_name:
        paper_name = PAPERS[data['kind']] if paper_name == 'auto' else paper_name
        paper_data = extract.datasheet(paper_name)
        paper = Film(paper_data)
        sources.append(f'printed on {paper_data["source"]}')
    model = Model(Film(data), paper, push)
    name = name or look_name(data, paper, push)
    out.mkdir(parents=True, exist_ok=True)
    (out / f'{safe(name)}.xmp').write_text(xmp(name, sources, pack(table(model, divisions or DIVISIONS))))
    settings, how = grain(data, model)
    presets = presets or out / 'presets'
    presets.mkdir(parents=True, exist_ok=True)
    title = name.removeprefix(PREFIX)
    (presets / f'{safe(title)}.xmp').write_text(preset(name, title, [str(g) for g in settings]))
    return report(name, model) + f'; grain {settings[0]} ({how})'


def variants(stock):
    """The looks a stock ships as: scanned or projected, and printed; a negative also
    pushed and pulled, both ways."""
    kind = extract.stock(stock)['kind']
    yield stock, None, 0
    yield stock, 'auto', 0
    if kind == 'negative':
        for push in PUSHES:
            yield stock, None, push
            yield stock, 'auto', push


def build_all(looks, presets, jobs, resume=False):
    """Every stock's looks into `looks` and their presets into `presets`, replacing
    what is there (or, with `resume`, keeping the looks already built), and the lists
    of them the app embeds."""
    from concurrent.futures import ProcessPoolExecutor
    jakob_shapes()
    for d in (looks, presets):
        d.mkdir(parents=True, exist_ok=True)
        for f in d.glob('*.xmp'):
            if not resume:
                f.unlink()
    films = [s for s in extract.stocks() if extract.stock(s)['kind'] != 'paper' and 'look' in extract.stock(s)]
    todo = [v for s in films for v in variants(s)]
    if resume:
        def done(stock, paper, push):
            name = look_name(extract.stock(stock), paper, push)
            return ((looks / f'{safe(name)}.xmp').exists()
                    and (presets / f'{safe(name.removeprefix(PREFIX))}.xmp').exists())
        todo = [v for v in todo if not done(*v)]
    print(f'{len(todo)} looks to build', flush=True)
    with ProcessPoolExecutor(jobs) as pool:
        futures = [pool.submit(build, s, p, push, looks, presets) for s, p, push in todo]
        for f in futures:
            print(f.result(), flush=True)
    write_lists(looks, presets)


def write_lists(looks, presets):
    """The Rust lists of the embedded files, so the app's build includes them."""
    def rust(path, files, what):
        body = ''.join(f'    "{f}",\n' for f in files)
        path.write_text(f'// Generated by scripts/film/film-look.py --all: every {what}.\n'
                        f'// Do not edit; run the script again.\nfiles![\n{body}]\n')
    rust(ROOT / 'crates/rawmakase-model/src/camera_profiles/film_files.in',
         sorted(f.name for f in looks.glob('*.xmp')), 'look in assets/looks')
    rust(ROOT / 'crates/rawmakase-interop/src/presets/film_files.in',
         sorted(f'{GROUP}/{f.name}' for f in presets.glob('*.xmp')), f'preset in assets/presets/{GROUP}')


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('stock', nargs='?', help=', '.join(s for s in extract.stocks() if extract.stock(s)['kind'] != 'paper'))
    ap.add_argument('--all', action='store_true', help="build every stock's looks into assets/")
    ap.add_argument('-j', '--jobs', type=int, default=2, help='looks built at once with --all')
    ap.add_argument('--resume', action='store_true', help='with --all, keep the looks already built')
    ap.add_argument('-o', '--out', type=Path, default=Path.cwd(), help='output directory')
    ap.add_argument('--presets', type=Path, help="the presets' directory (default: OUT/presets)")
    ap.add_argument('--name', help="the profile's name (default: RMKS Film: and the stock's look)")
    ap.add_argument('--divisions', type=int, default=DIVISIONS, help='RGB table divisions per side, 2-64')
    ap.add_argument('--print', dest='paper', nargs='?', const='auto',
                    help="print the film on a paper from stocks/ instead of scanning a negative or "
                         "projecting a slide (default: %s)" % ', '.join(f'{k} for {v}' for v, k in PAPERS.items()))
    ap.add_argument('--push', type=int, default=0, help='stops a negative is exposed over (or under, negative)')
    args = ap.parse_args()
    if args.all:
        build_all(ROOT / 'assets/looks', ROOT / 'assets/presets' / GROUP, args.jobs, args.resume)
        return
    if not args.stock:
        ap.error('name a stock or use --all')
    if not 2 <= args.divisions <= 64:
        ap.error('--divisions must be 2-64')
    print(build(args.stock, args.paper, args.push, args.out, args.presets, args.divisions, args.name))


if __name__ == '__main__':
    main()
