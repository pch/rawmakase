#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy==2.5.3", "scipy==1.18.1", "colour-science==0.4.7", "pdfplumber==0.11.10"]
# ///
"""Make a film look profile from a film's datasheet curves.

Downloads the film's Kodak datasheet (and its paper's, for a print) and reads its
curves with `extract-datasheet.py`, then writes a Camera Raw look profile, an XMP
with a 3D RGB table, which RAWmakase and Lightroom both import, and a develop preset
choosing it. `build-assets.sh` makes the ones RAWmakase ships.

    uv run scripts/film/film-look.py portra-400 -o ~/film-looks
    uv run scripts/film/film-look.py kodachrome-64 --print -o ~/film-looks

The model follows the light, as a film would see it:

1. The table's input is the photo after the base profile's tone curve. Inverting
   that curve gives the scene light the camera measured (linear ProPhoto, white
   balanced), with a correctly exposed mid gray at `MID_GRAY` after the curve.
2. Each colour becomes a plausible spectrum (Jakob and Hanika's sigmoid model,
   2019), which exposes the film's three layers through their published spectral
   sensitivities, under daylight (D55). Mid gray exposes every layer at the
   datasheet's reference exposure.
3. The characteristic curves turn each layer's exposure into density, so
   exposure decides where a colour sits on the film's toe and shoulder: the
   Exposure slider moves a photo along the film's response, as over- and
   underexposing film does.
4. Negative film is then scanned: densities are inverted channel by channel
   with each channel's own gamma (so the straight part of the curve stays neutral,
   as a scanner balanced on mid gray does), the film base is set to black, and the
   three channels are written out as sRGB, as a lab scanner outputs them: nothing
   corrects the film's colour back toward the camera's. The base tone curve then
   renders it.
   Reversal film is viewed instead: each layer's density becomes an amount of its
   dye, the dyes' spectra filter the projector's light (the datasheet's viewing
   illuminant), and the eye adapts to the film's clear base. The film's own curve
   is the tone curve.
   With `--print` the film is printed instead, on Kodak's paper for its kind
   (PAPERS) or the one named: its densities, brought to a common gamma, expose the
   paper, whose dyes are viewed under D50 with 1% flare. The papers' printing
   densities and a lab's printer settings aren't published, so a print is the
   datasheets' best guess: more contrasty than a scan.

With every look, `presets/<name>.xmp` is a develop preset choosing it at full
Amount with the film's grain (GRAIN), for the Presets panel.

What the datasheets don't give is approximated, and the output says so: Status M
and Status A densitometry are taken as narrow bands at each dye's peak, the
scanner writes its channels as sRGB rather than through a model of a Frontier, and
there is no grain, halation or interlayer effect.
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
# The companion presets' Grain (Amount, Size, Roughness). RAWmakase's grain adds
# 0.93% luminance noise (rms over mean) per unit of Amount at mid gray (Size 25,
# measured on a 970 px render; it scales with the photo's long edge, as a frame's
# grain does). Kodachrome 64's rms granularity of 10 (σD 0.010 through a 48 µm
# aperture) is 2.3% transmittance noise, 2.6% at a 970 px frame's 37 µm pixel: Amount
# 3. Portra 400's datasheet gives only a print grain index, so its Amount is an
# estimate, not a measurement.
GRAIN = {
    'kodachrome-64': (3, 25, 50),
    'portra-400': (8, 25, 50),
}
# The profiles' names: a nod to the film, marked as an emulation.
NAMES = {
    'portra-400': 'RMKS Film: Portra-ish 400',
    'kodachrome-64': 'RMKS Film: Kodachrome-ish 64',
}
# The paper `--print` uses for each kind of film: Kodak's papers for printing
# negatives (RA-4) and slides (R-3).
PAPERS = {
    'negative': 'endura-premier',
    'reversal': 'radiance-iii',
}
# Viewing flare on a print: light reflected off its surface, as a fraction of its white.
PRINT_FLARE = 0.01
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


class Film:
    def __init__(self, data):
        self.data = data
        self.kind = data['type']
        curves = data['curves']
        self.channels = ('red', 'green', 'blue')
        self.curves = [characteristic(curves['characteristic'][c], self.kind) for c in self.channels]
        sens = np.stack([sensitivity(curves['sensitivity'][c]) for c in self.channels])
        # A flat spectrum under daylight exposes every layer equally, at 1: the
        # characteristic curves were measured on neutrals under daylight.
        self.sens = sens * D55.values / (sens * D55.values).sum(axis=1, keepdims=True)

    def exposure(self, reflectance):
        """Layer exposures of spectra (…, WL), relative to a white reflector."""
        return reflectance @ self.sens.T


# The brightness spectra are found at: the ProPhoto value of a dark-ish surface. Bright
# saturated colours have no smooth reflectance spectrum; real ones are darker.
SPECTRUM_VALUE = 0.2


def jakob_shapes(n_hue, n_sat):
    """Spectra for a grid of chromaticities (HSV hue and saturation of linear
    ProPhoto at SPECTRUM_VALUE), from Jakob and Hanika's sigmoid model under D50, the white
    ProPhoto is balanced to. Returns the spectra and their colours' remaining XYZ
    error, for colours no smooth reflectance reaches (ProPhoto's corners)."""
    spectra = np.zeros((n_hue, n_sat, len(WL)))
    residual = np.zeros((n_hue, n_sat, 3))
    for i in range(n_hue):
        coefficients = np.zeros(3)
        for j in range(n_sat):
            rgb = colour.HSV_to_RGB([i / n_hue, j / (n_sat - 1), SPECTRUM_VALUE])
            xyz = PRO_TO_XYZ @ rgb
            if j == 0:
                coefficients = np.zeros(3)
            coefficients, _ = find_coefficients_Jakob2019(xyz, CMFS, D50, coefficients_0=coefficients)
            r = sd_Jakob2019(coefficients, SHAPE).values
            spectra[i, j] = r
            residual[i, j] = xyz - reflectance_xyz(r)
        print(f'\rspectra {i + 1}/{n_hue}', end='', file=sys.stderr)
    print(file=sys.stderr)
    return spectra, residual


def reflectance_xyz(r):
    """XYZ of reflectances under D50, white at Y = 1."""
    w = D50.values[:, None] * CMFS.values
    return (np.asarray(r) @ w) / w[:, 1].sum()


def colorchecker():
    sds = colour.SDS_COLOURCHECKERS['BabelColor Average']
    return np.stack([sd.copy().align(SHAPE).values for sd in sds.values()])


class Model:
    def __init__(self, film, paper=None, n_hue=72, n_sat=25):
        self.film = film
        self.n_hue, self.n_sat = n_hue, n_sat
        spectra, residual = jakob_shapes(n_hue, n_sat)
        # Colours a smooth spectrum can't reach keep their difference linearly, through
        # the best linear map from XYZ to layer exposure over the ColorChecker.
        cc = colorchecker()
        k = np.linalg.lstsq(reflectance_xyz(cc), film.exposure(cc), rcond=None)[0].T
        # Exposures per unit of linear ProPhoto value.
        self.unit = np.maximum(film.exposure(spectra) + residual @ k.T, 0) / SPECTRUM_VALUE
        self.paper = None
        if film.kind == 'negative':
            self.log_h_ref = film.data['log_h_ref']
        else:
            light = colour.sd_blackbody(film.data['viewing_illuminant_k'], SHAPE).values
            # Kodak normalizes a slide's dyes so that one unit of each forms a neutral.
            self.dyes = Dyes(film.data, film.curves, light, neutral=np.ones(3))
            self.log_h_ref = self.dyes.reference_exposure()
        self.d_ref = np.array([c(np.array(self.log_h_ref)) for c in film.curves])
        x = self.log_h_ref + np.linspace(-0.6, 0.6, 13)
        gamma = np.array([np.polyfit(x, c(x), 1)[0] for c in film.curves])
        if paper is not None:
            # Printed: the paper sees the film through its Status M (negative) or Status
            # A (slide) densities. Film and paper are made so a gray prints neutral,
            # but those densities aren't quite the paper's printing densities (blue's
            # gamma reads apart), so each channel is brought to the three's mean gamma
            # at gray. The printer's three lights are then set, as a lab sets them, so
            # a correctly exposed gray prints as a neutral mid gray.
            self.paper = paper
            self.paper_dyes = Dyes(paper.data, paper.curves, D50.values, neutral=None,
                                   flare=PRINT_FLARE)
            self.print_gain = gamma.mean() / gamma
            target = self.paper_dyes.mid_gray_densities()
            self.printer = np.array([inverse(c, t) for c, t in zip(paper.curves, target)])
        elif film.kind == 'negative':
            # Scanned: each channel inverted with its own gamma over two stops either
            # side of mid gray, the film base set to black, and the channels written
            # out as sRGB. No calibration pulls colours back toward the camera's.
            self.matrix = colour.matrix_RGB_to_RGB(colour.RGB_COLOURSPACES['sRGB'], PRO,
                                                   chromatic_adaptation_transform='Bradford')
            self.gamma = gamma
            self.d_min = np.array([c(np.array(-10.)) for c in film.curves])

    def layer_exposure(self, scene):
        """Log exposure of each layer for scene-linear ProPhoto colours, with mid
        gray at the datasheet's reference."""
        scene = np.clip(scene, 0, None)
        hsv = colour.RGB_to_HSV(scene)
        h = hsv[..., 0] * self.n_hue
        s = hsv[..., 1] * (self.n_sat - 1)
        h0 = np.floor(h).astype(int)
        s0 = np.clip(np.floor(s).astype(int), 0, self.n_sat - 2)
        fh, fs = (h - h0)[..., None], np.clip(s - s0, 0, 1)[..., None]
        h0 %= self.n_hue
        h1 = (h0 + 1) % self.n_hue
        u = self.unit
        unit = ((1 - fh) * (1 - fs) * u[h0, s0] + fh * (1 - fs) * u[h1, s0]
                + (1 - fh) * fs * u[h0, s0 + 1] + fh * fs * u[h1, s0 + 1])
        h = unit * hsv[..., 2:3]
        return self.log_h_ref + np.log10(np.maximum(h, 1e-9) / SCENE_GRAY)

    def display(self, scene):
        """Display-linear ProPhoto of scene-linear ProPhoto colours."""
        d = density(self.film.curves, self.layer_exposure(scene))
        if self.paper is not None:
            printing = (d - self.d_ref) * self.print_gain
            return self.paper_dyes.view(density(self.paper.curves, self.printer - printing))
        if self.film.kind != 'negative':
            return self.dyes.view(d)
        positive = SCENE_GRAY * 10 ** ((d - self.d_ref) / self.gamma)
        base = SCENE_GRAY * 10 ** ((self.d_min - self.d_ref) / self.gamma)
        positive = SCENE_GRAY * (positive - base) / (SCENE_GRAY - base)
        return hue_preserving(positive @ self.matrix.T, tone)


def density(curves, log_h):
    return np.stack([c(log_h[..., i]) for i, c in enumerate(curves)], axis=-1)


def inverse(curve, d):
    """The log exposure at which a monotonic characteristic curve reaches density d."""
    x, y = curve.x, curve.y
    if y[-1] < y[0]:
        x, y = x[::-1], y[::-1]
    return float(np.interp(d, np.maximum.accumulate(y), x))


class Dyes:
    """A slide's or a print's dyes: from the three layers' densities to the colour of
    the light they pass, under the light they are viewed in."""

    def __init__(self, data, curves, light, neutral, flare=0.):
        dd = data['curves']['dye_density']
        # Light reflected off the surface or scattered in the room, as a fraction of
        # the white, added to every colour.
        self.flare = flare
        self.eps = np.stack([spectral(dd[d]) for d in ('cyan', 'magenta', 'yellow')])
        self.curves = curves
        self.light = light / (light * CMFS.values[:, 1]).sum()
        # Densitometry as narrow bands at each dye's peak (Status A approximated).
        peaks = WL[self.eps.argmax(axis=1)]
        self.bands = np.exp(-0.5 * ((WL[None] - peaks[:, None]) / 15.) ** 2)
        self.bands /= self.bands.sum(axis=1, keepdims=True)
        self.scale = np.ones(3)
        # The amounts that form a visual neutral of density 1.0: given by the datasheet
        # when its dyes are normalized so, else solved for.
        self.neutral = neutral if neutral is not None else self.solve_neutral()
        # A material balanced for its light forms a neutral from a gray exposure, so its
        # characteristic curves' densities where they average 1.0 are that neutral as
        # Status A reads it (not 1.0 in every channel): scale each band to read so.
        x = np.linspace(-6, 4, 10001)
        mean = np.mean([c(x) for c in curves], axis=0)
        at = x[np.argmin(np.abs(mean - 1.0))]
        self.scale = (np.array([c(np.array(at)) for c in curves])
                      / self.band_density(self.neutral[None])[0])
        # The eye adapts to the material's clearest white: a slide's base under the
        # projector, a print's paper white.
        d_min = np.array([c.y.min() for c in curves])
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
        """Dye amounts whose band densities are `d`, by Newton's method."""
        a = np.array(d, dtype=float)
        for _ in range(30):
            f = self.band_density(a) - d
            jac = np.stack([(self.band_density(a + e * 1e-4) - self.band_density(a)) / 1e-4
                            for e in np.eye(3)], axis=-1)
            step = np.linalg.solve(jac, f[..., None])[..., 0]
            a = np.maximum(a - step, 0)
            if np.abs(step).max() < 1e-6:
                break
        return a

    def xyz(self, a):
        t = 10 ** -(a @ self.eps)
        return (t * self.light) @ CMFS.values

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
        return self.band_density(((lo + hi) / 2 * self.neutral)[None])[0]

    def reference_exposure(self):
        """The exposure at which a gray shows as mid gray: a correctly exposed slide."""
        lo, hi = -4., 2.
        for _ in range(50):
            mid = (lo + hi) / 2
            d = np.array([[c(np.array(mid)) for c in self.curves]])
            y = (self.view(d) @ PRO_TO_XYZ.T)[0, 1]
            lo, hi = (mid, hi) if y < MID_GRAY else (lo, mid)
        return (lo + hi) / 2


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
    return description(attributes, name, 'Film') + '  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n'


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
    return description(attributes, title, 'Film') + look + '  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n'


def report(model):
    """How mid gray and an exposure sweep come out, for a quick sanity check."""
    print(f'scene mid gray {SCENE_GRAY:.4f} (display {MID_GRAY}); reference log H {model.log_h_ref:.3f}')
    for ev in (-4, -2, -1, 0, 1, 2):
        scene = np.full((1, 3), min(SCENE_GRAY * 2 ** ev, 1.))
        out = model.display(scene)[0]
        print(f'  gray {ev:+d} EV -> display {srgb_encode(out).round(3)}')
    cc = colorchecker()
    scene = reflectance_xyz(cc) @ XYZ_TO_PRO.T * (SCENE_GRAY / 0.18)
    before = hue_preserving(scene, tone)
    after = model.display(scene)
    lab = lambda rgb: colour.XYZ_to_Lab(np.clip(rgb, 0, None) @ PRO_TO_XYZ.T, PRO.whitepoint)
    de = colour.delta_E(lab(before), lab(after), method='CIE 2000')
    names = list(colour.SDS_COLOURCHECKERS['BabelColor Average'])
    print(f'  ColorChecker vs the base rendering: mean ΔE00 {de.mean():.1f}, max {de.max():.1f} ({names[de.argmax()]})')
    for i in (1, 2, 3, 13, 14, 15):
        b, a = lab(before)[i], lab(after)[i]
        print(f'    {names[i]:14} L {b[0]:5.1f}->{a[0]:5.1f}  C {np.hypot(*b[1:]):5.1f}->{np.hypot(*a[1:]):5.1f}  '
              f'h {np.degrees(np.arctan2(b[2], b[1])) % 360:5.1f}->{np.degrees(np.arctan2(a[2], a[1])) % 360:5.1f}')


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('film', help=', '.join(k for k, v in extract.FILMS.items() if v['type'] != 'paper'))
    ap.add_argument('-o', '--out', type=Path, default=Path.cwd(), help='output directory')
    ap.add_argument('--presets', type=Path, help="the presets' directory (default: OUT/presets)")
    ap.add_argument('--name', help="the profile's name (default: the film's name)")
    ap.add_argument('--divisions', type=int, default=33, help='RGB table divisions per side, 2-64')
    ap.add_argument('--print', dest='paper', nargs='?', const='auto',
                    help="print the film on a paper in extract-datasheet.py instead of scanning a "
                         "negative or projecting a slide (default paper: %s)" % ', '.join(
                             f'{k} for {v}' for v, k in PAPERS.items()))
    ap.add_argument('--grain', help="the preset's Grain as Amount,Size,Roughness (default: the film's)")
    args = ap.parse_args()
    if not 2 <= args.divisions <= 64:
        sys.exit('--divisions must be 2-64')
    data = extract.datasheet(args.film)
    sources = [data['source']]
    paper = None
    if args.paper:
        paper_name = PAPERS[data['type']] if args.paper == 'auto' else args.paper
        paper_data = extract.datasheet(paper_name)
        paper = Film(paper_data)
        sources.append(f'printed on {paper_data["source"]}')
    model = Model(Film(data), paper)
    report(model)
    rgb = pack(table(model, args.divisions))
    name = args.name or NAMES.get(args.film, data['name']) + (' Print' if paper else '')
    args.out.mkdir(parents=True, exist_ok=True)
    # File names without characters macOS or Windows refuse.
    file = ''.join(c for c in name if c not in ':/\\?*"<>|')
    path = args.out / f'{file}.xmp'
    path.write_text(xmp(name, sources, rgb))
    print(path)
    grain = args.grain.split(',') if args.grain else GRAIN.get(args.film)
    if grain:
        presets = args.presets or args.out / 'presets'
        presets.mkdir(parents=True, exist_ok=True)
        # In the Presets panel's Film group the brand is redundant.
        title = name.removeprefix('RMKS Film: ')
        path = presets / f"{''.join(c for c in title if c not in ':/\\?*\"<>|')}.xmp"
        path.write_text(preset(name, title, [str(g) for g in grain]))
        print(path)


if __name__ == '__main__':
    main()
