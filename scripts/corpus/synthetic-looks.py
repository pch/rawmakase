#!/usr/bin/env python3
"""Write the synthetic look profiles in tests/corpus/looks.

Each is a Camera Raw look profile (an XMP with an HSV look table, an RGB table, a tone
curve and profile-internal settings) made up from simple formulas here: no Adobe
profile data. They exercise Lightroom's Profile Amount, which only looks with
`SupportsAmount` have, and the RGB tables of creative looks (Artistic, Vintage, Modern).
Camera Raw reads a look from a sidecar's `crs:Look` and its `Table_` attribute, so
scripts/corpus/camera-raw-charts.py can render them without installing anything.

Run from the repository root: python3 scripts/corpus/synthetic-looks.py
"""
import hashlib
import math
import struct
import zlib
from pathlib import Path

OUT = Path(__file__).resolve().parents[2] / 'tests/corpus/looks'
ALPHABET = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.-:+=^!/*?`'|()[]{}@%$#"


def base85(data):
    """Adobe's table encoding: zlib with the expanded length first, in base 85."""
    packed = struct.pack('<I', len(data)) + zlib.compress(data, 9)
    out = ''
    for i in range(0, len(packed), 4):
        chunk = packed[i:i + 4]
        value = struct.unpack('<I', chunk + b'\0' * (4 - len(chunk)))[0]
        for _ in range(len(chunk) + 1):
            out += chr(ALPHABET[value % 85])
            value //= 85
    return out


def table(dims, f, srgb=True, bounds=(0., 2.)):
    """A DNG SDK HSV look table: values outermost, then hues, then saturations. A
    version 2 table stores the bounds its Profile Amount scales between."""
    hues, sats, vals = dims
    data = struct.pack('<5I', 0, 2 if bounds else 1, hues, sats, vals)
    for v in range(vals):
        for h in range(hues):
            for s in range(sats):
                data += struct.pack('<3f', *f(h / hues, s / (sats - 1), v / max(vals - 1, 1)))
    data += struct.pack('<I', 1 if srgb else 0)
    if bounds:
        data += struct.pack('<2d', *bounds)
    return data


# The color spaces, encodings and gamut modes of a DNG SDK RGB table.
SRGB, ADOBE_RGB, PROPHOTO, DISPLAY_P3, REC2020 = 0, 1, 2, 3, 4
LINEAR, SRGB_GAMMA, GAMMA_1_8, GAMMA_2_2, REC2020_GAMMA = 0, 1, 2, 3, 4
CLIP, EXTEND = 0, 1


def rgb_table(divisions, f, primaries=ADOBE_RGB, gamma=GAMMA_2_2, gamut=CLIP, bounds=(0., 2.), flags=None):
    """A DNG SDK 3D RGB table: red outermost, then green, then blue, each sample three
    16-bit values stored as their difference from the identity, then the table's
    color space, encoding, gamut mode and Profile Amount bounds."""
    data = struct.pack('<4I', 1, 1, 3, divisions)
    grid = [(i * 0xFFFF + (divisions - 1) // 2) // (divisions - 1) for i in range(divisions)]
    for r in range(divisions):
        for g in range(divisions):
            for b in range(divisions):
                out = f(*(i / (divisions - 1) for i in (r, g, b)))
                values = [round(min(max(v, 0.), 1.) * 0xFFFF) for v in out]
                data += struct.pack('<3H', *((v - grid[i]) & 0xFFFF for v, i in zip(values, (r, g, b))))
    return data + rgb_table_tail(primaries, gamma, gamut, bounds, flags)


def rgb_table_1d(divisions, f, primaries=ADOBE_RGB, gamma=GAMMA_2_2, gamut=CLIP, bounds=(0., 2.)):
    """A DNG SDK 1D RGB table: a curve for each channel, stored like a 3D table's
    samples."""
    data = struct.pack('<4I', 1, 1, 1, divisions)
    for i in range(divisions):
        identity = (i * 0xFFFF + (divisions - 1) // 2) // (divisions - 1)
        out = f(i / (divisions - 1))
        data += struct.pack('<3H', *((round(min(max(v, 0.), 1.) * 0xFFFF) - identity) & 0xFFFF for v in out))
    return data + rgb_table_tail(primaries, gamma, gamut, bounds)


def rgb_table_tail(primaries, gamma, gamut, bounds, flags=None):
    data = struct.pack('<3I2d', primaries, gamma, gamut, *bounds)
    if flags is not None:
        data += struct.pack('<I', flags)
    return data


def look(name, data, settings=None, curve=None, monochrome=False, rgb=None, channels=None):
    """`channels` gives the look's Red, Green and Blue point curves by name; a channel
    it leaves out is the identity, and `None` for one leaves its element out."""
    uuid = hashlib.md5(name.encode()).hexdigest().upper()
    tables = {}
    if data:
        tables['LookTable'] = data
    if rgb:
        tables['RGBTable'] = rgb
    ids = {key: hashlib.md5(t).hexdigest().upper() for key, t in tables.items()}
    attributes = {
        'PresetType': 'Look', 'Cluster': '', 'UUID': uuid, 'SupportsAmount': 'True',
        'SupportsColor': 'True', 'SupportsMonochrome': 'True' if monochrome else 'False',
        'SupportsHighDynamicRange': 'True', 'SupportsNormalDynamicRange': 'True',
        'SupportsSceneReferred': 'True', 'SupportsOutputReferred': 'False',
        'CameraModelRestriction': '', 'Copyright': 'Synthetic test look, public domain',
        'ContactInfo': '', 'Version': '18.7', 'ProcessVersion': '15.4',
        'ConvertToGrayscale': 'True' if monochrome else 'False',
        **ids,
        **(settings or {}),
        **{f'Table_{ids[key]}': base85(t) for key, t in tables.items()},
    }
    xml = ('<x:xmpmeta xmlns:x="adobe:ns:meta/">\n <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">\n'
           '  <rdf:Description rdf:about=""\n    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"')
    xml += ''.join(f'\n   crs:{k}="{v}"' for k, v in attributes.items()) + '>\n'
    for element, text in [('Name', name), ('Group', 'Synthetic')]:
        xml += (f'   <crs:{element}>\n    <rdf:Alt>\n     <rdf:li xml:lang="x-default">{text}</rdf:li>\n'
                f'    </rdf:Alt>\n   </crs:{element}>\n')
    for channel in ['', 'Red', 'Green', 'Blue']:
        points = (curve or IDENTITY) if channel == '' else (channels or {}).get(channel, IDENTITY)
        if points is None:
            continue
        xml += (f'   <crs:ToneCurvePV2012{channel}>\n    <rdf:Seq>\n'
                + ''.join(f'     <rdf:li>{p}</rdf:li>\n' for p in points)
                + f'    </rdf:Seq>\n   </crs:ToneCurvePV2012{channel}>\n')
    return xml + '  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n'


def twist(h, s, v):
    """Hue shifts of up to 35°, saturation ±35% and value −12% growing with saturation."""
    return (25 * math.sin(2 * math.pi * h) + 10 * math.cos(4 * math.pi * h) * s,
            1 + 0.35 * math.cos(2 * math.pi * (h - 0.1)) * s,
            1 - 0.12 * s + 0.05 * math.sin(2 * math.pi * h) * s)


def identity(h, s, v):
    return (0., 1., 1.)


def cross(v):
    """Channel curves: red lifted and rolled off, green with an S, blue compressed."""
    return (0.05 + 0.9 * v ** 0.85, v + 0.06 * math.sin(2 * math.pi * v), 0.1 + 0.75 * v)


def fade(r, g, b):
    """A creative grade: lifted blacks, a warm cast growing in the shadows, red bleeding
    into green, blue compressed and a soft roll-off near white."""
    return (0.04 + 0.9 * r ** 1.1 + 0.05 * g * (1 - r),
            0.03 + 0.92 * g + 0.04 * math.sin(math.pi * g) + 0.03 * r * (1 - g),
            0.08 + 0.8 * b + 0.06 * r * (1 - b) - 0.03 * math.sin(math.pi * g))


def half(v):
    """Each channel halved in the table's encoding: on a gray, the encoding's curve."""
    return (0.5 * v,) * 3


IDENTITY = ['0, 0', '255, 255']
S_CURVE = ['0, 0', '64, 44', '128, 132', '192, 214', '255, 255']
# Channel curves a film look might carry: red lifted in the shadows, green with an S,
# blue darkened and ending below white.
CHANNELS = {'Red': ['0, 30', '128, 150', '255, 255'],
            'Green': ['0, 0', '64, 50', '192, 210', '255, 255'],
            'Blue': ['0, 0', '128, 110', '255, 230']}
LOOKS = {
    'synthetic-table.xmp': look('Synthetic Table', table((12, 5, 6), twist)),
    'synthetic-table-v1.xmp': look('Synthetic Table v1', table((12, 5, 6), twist, bounds=None)),
    'synthetic-curve.xmp': look('Synthetic Curve', table((4, 2, 1), identity, srgb=False), curve=S_CURVE),
    'synthetic-tones.xmp': look('Synthetic Tones', table((4, 2, 1), identity, srgb=False),
                                {'Shadows2012': '+40', 'Highlights2012': '-40'}),
    'synthetic-contrast.xmp': look('Synthetic Contrast', table((4, 2, 1), identity, srgb=False),
                                   {'Contrast2012': '+40', 'Blacks2012': '-20'}),
    'synthetic-mono.xmp': look('Synthetic Mono', table((12, 5, 6), twist), {'Contrast2012': '+20'},
                               curve=S_CURVE, monochrome=True),
    'synthetic-look.xmp': look('Synthetic Look', table((12, 5, 6), twist),
                               {'Shadows2012': '+25', 'Clarity2012': '+10'}, curve=S_CURVE),
    'synthetic-color.xmp': look('Synthetic Color', table((4, 2, 1), identity, srgb=False),
                                {'Saturation': '-20', 'SaturationAdjustmentOrange': '-40',
                                 'LuminanceAdjustmentOrange': '-20', 'HueAdjustmentBlue': '+30'}),
    'synthetic-split.xmp': look('Synthetic Split', table((4, 2, 1), identity, srgb=False),
                                {'SplitToningShadowHue': '210', 'SplitToningShadowSaturation': '40',
                                 'SplitToningHighlightHue': '50', 'SplitToningHighlightSaturation': '30',
                                 'SplitToningBalance': '+20'}),
    'synthetic-parametric.xmp': look('Synthetic Parametric', table((4, 2, 1), identity, srgb=False),
                                     {'ParametricShadows': '0', 'ParametricDarks': '-15', 'ParametricLights': '-20',
                                      'ParametricHighlights': '-33', 'ParametricShadowSplit': '25',
                                      'ParametricMidtoneSplit': '50', 'ParametricHighlightSplit': '75'}),
    'synthetic-exposure.xmp': look('Synthetic Exposure', table((4, 2, 1), identity, srgb=False),
                                   {'Exposure2012': '+0.50'}),
    'synthetic-vignette.xmp': look('Synthetic Vignette', table((4, 2, 1), identity, srgb=False),
                                   {'PostCropVignetteAmount': '-30', 'PostCropVignetteMidpoint': '50',
                                    'PostCropVignetteFeather': '50', 'PostCropVignetteRoundness': '0',
                                    'PostCropVignetteStyle': '1', 'PostCropVignetteHighlightContrast': '0'}),
    'synthetic-rgb.xmp': look('Synthetic RGB', None, rgb=rgb_table(17, fade)),
    'synthetic-rgb-half.xmp': look('Synthetic RGB Half', None, {'RGBTableAmount': '0.5'},
                                   rgb=rgb_table(17, fade, bounds=(0., 1.))),
    'synthetic-rgb-bounds.xmp': look('Synthetic RGB Bounds', None, rgb=rgb_table(17, fade, bounds=(0.5, 1.5))),
    'synthetic-rgb-srgb.xmp': look('Synthetic RGB sRGB', None,
                                   rgb=rgb_table(17, fade, primaries=SRGB, gamma=SRGB_GAMMA)),
    'synthetic-rgb-linear.xmp': look('Synthetic RGB Linear', None,
                                     rgb=rgb_table(17, fade, primaries=PROPHOTO, gamma=LINEAR)),
    'synthetic-rgb-extend.xmp': look('Synthetic RGB Extend', None,
                                     rgb=rgb_table(17, fade, primaries=SRGB, gamma=SRGB_GAMMA, gamut=EXTEND)),
    'synthetic-rgb-1d.xmp': look('Synthetic RGB 1D', None, rgb=rgb_table_1d(64, cross)),
    'synthetic-rgb-p3.xmp': look('Synthetic RGB Display P3', None,
                                 rgb=rgb_table(17, fade, primaries=DISPLAY_P3, gamma=SRGB_GAMMA)),
    'synthetic-rgb-rec2020.xmp': look('Synthetic RGB Rec. 2020', None,
                                      rgb=rgb_table(17, fade, primaries=REC2020, gamma=REC2020_GAMMA)),
    'synthetic-rgb-rec2020-half.xmp': look('Synthetic RGB Rec. 2020 Half', None,
                                           rgb=rgb_table_1d(256, half, primaries=REC2020, gamma=REC2020_GAMMA)),
    'synthetic-rgb-code4-half.xmp': look('Synthetic RGB Code 4 Half', None,
                                         rgb=rgb_table_1d(256, half, primaries=SRGB, gamma=REC2020_GAMMA)),
    'synthetic-rgb-flag.xmp': look('Synthetic RGB Flag', None, rgb=rgb_table(17, fade, flags=1)),
    'synthetic-channels.xmp': look('Synthetic Channels', table((4, 2, 1), identity, srgb=False), channels=CHANNELS),
    'synthetic-channels-curve.xmp': look('Synthetic Channels Curve', table((4, 2, 1), identity, srgb=False),
                                         curve=S_CURVE, channels=CHANNELS),
    'synthetic-channels-red.xmp': look('Synthetic Channels Red', table((4, 2, 1), identity, srgb=False),
                                       channels={'Red': CHANNELS['Red'], 'Green': None, 'Blue': None}),
    'synthetic-rgb-look.xmp': look('Synthetic RGB Look', table((12, 5, 6), twist), curve=S_CURVE,
                                   rgb=rgb_table(17, fade)),
}

if __name__ == '__main__':
    OUT.mkdir(parents=True, exist_ok=True)
    for name, text in LOOKS.items():
        (OUT / name).write_text(text)
        print(OUT / name)
