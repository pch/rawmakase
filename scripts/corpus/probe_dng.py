"""Write synthetic probe DNGs: linear RGB scenes with an embedded camera profile.

The camera's own RGB is linear ProPhoto (ROMM, D50), with an embedded profile that
maps it to XYZ D50 unchanged: As Shot white balance is neutral, so a renderer's
profile-colour stage passes the scene through, and with a linear ProfileToneCurve
and no look Camera Raw's ProPhoto output reads its scene tone stage directly
(docs/scene-tone-stage.md#comparison-contract).

Images are float arrays (height, width, 3) of scene-linear ProPhoto values with even
dimensions, stored as an RGGB mosaic of 16-bit integers clipped at 1, the sensor's
white level (RAWmakase reads mosaic DNGs only). Flat patches demosaic exactly away
from their edges. Scene values above 1 are written with a positive
`baseline_exposure`: the sensor holds the scene scaled down by it.
"""
import struct

import numpy as np

# ProPhoto RGB (ROMM) to XYZ, D50 white.
PROPHOTO_TO_XYZ = np.array([[0.7976749, 0.1351917, 0.0313534],
                            [0.2880402, 0.7118741, 0.0000857],
                            [0.0000000, 0.0000000, 0.8252100]])
D50 = 23


def _srational(values, scale=1_000_000):
    out = []
    for v in values:
        out += [int(round(v * scale)), scale]
    return out


def tone_curve(kind):
    """ProfileToneCurve points: 'linear', or 'filmic', a smooth monotone curve pinned at
    0 and 1 with the brightening shape of a camera profile's curve. The stage-order
    probes compare Camera Raw with itself through the same curve, so it need not be
    any particular profile's."""
    if kind == 'linear':
        return [0., 0., 1., 1.]
    if kind == 'filmic':
        x = np.linspace(0, 1, 257)
        return [float(v) for pair in zip(x, filmic(x)) for v in pair]
    raise ValueError(kind)


def filmic(x):
    """The 'filmic' profile curve."""
    x = np.asarray(x, dtype=float)
    return np.clip((1 - np.exp(-4.2 * x ** 0.8)) / (1 - np.exp(-4.2)), 0, 1)


def write(path, image, *, curve='linear', look=None, hue_sat=None, baseline_exposure=0.,
          black_render=None, name='RAWmakase Probe'):
    """Write `image` (scene values, divided by 2**baseline_exposure on the sensor) as a
    mosaic DNG.

    `look`/`hue_sat`: optional (dims, data) for ProfileLookTable/ProfileHueSatMap, data
    as (hue, sat, val) triples [hue shift degrees, sat scale, val scale] in DNG order.
    `black_render`: DefaultBlackRender (0 Auto, 1 None); omitted when None.
    """
    image = np.asarray(image, dtype=np.float64)
    h, w, _ = image.shape
    entries = []

    def add(tag, kind, values):
        fmt = {1: 'B', 2: 'B', 3: 'H', 4: 'I', 5: 'I', 10: 'i', 11: 'f'}[kind]
        if isinstance(values, str):
            values = list(values.encode() + b'\0')
        count = len(values) // 2 if kind in (5, 10) else len(values)
        entries.append((tag, kind, count, struct.pack('<' + fmt * len(values), *values)))

    if h % 2 or w % 2:
        raise ValueError('probe images need even dimensions')
    sensor = image / 2. ** baseline_exposure
    mosaic = np.empty((h, w))
    mosaic[0::2, 0::2] = sensor[0::2, 0::2, 0]
    mosaic[0::2, 1::2] = sensor[0::2, 1::2, 1]
    mosaic[1::2, 0::2] = sensor[1::2, 0::2, 1]
    mosaic[1::2, 1::2] = sensor[1::2, 1::2, 2]
    pixels = np.round(np.clip(mosaic, 0, 1) * 65535).astype('<u2').tobytes()
    camera = np.linalg.inv(PROPHOTO_TO_XYZ)
    add(254, 4, [0])
    add(256, 4, [w])
    add(257, 4, [h])
    add(258, 3, [16])
    add(259, 3, [1])
    add(262, 3, [32803])
    add(271, 2, 'RAWmakase')
    add(272, 2, 'Probe')
    add(273, 4, [0])
    add(274, 3, [1])
    add(277, 3, [1])
    add(278, 4, [h])
    add(279, 4, [len(pixels)])
    add(284, 3, [1])
    add(33421, 3, [2, 2])
    add(33422, 1, [0, 1, 1, 2])
    add(50706, 1, [1, 4, 0, 0])
    add(50707, 1, [1, 1, 0, 0])
    add(50708, 2, 'RAWmakase Probe')
    add(50710, 1, [0, 1, 2])
    add(50711, 3, [1])
    add(50714, 4, [0])
    add(50717, 4, [65535])
    add(50721, 10, _srational(camera.flatten()))
    add(50728, 5, [1, 1, 1, 1, 1, 1])
    add(50730, 10, _srational([baseline_exposure]))
    add(50778, 3, [D50])
    add(50936, 2, name)
    add(50940, 11, tone_curve(curve) if isinstance(curve, str) else list(curve))
    add(50941, 4, [3])
    add(50964, 10, _srational(PROPHOTO_TO_XYZ.flatten()))
    if hue_sat is not None:
        dims, data = hue_sat
        add(50937, 4, list(dims))
        add(50938, 11, [float(v) for v in np.asarray(data).flatten()])
    if look is not None:
        dims, data = look
        add(50981, 4, list(dims))
        add(50982, 11, [float(v) for v in np.asarray(data).flatten()])
    if black_render is not None:
        add(51110, 4, [black_render])
    entries.sort(key=lambda e: e[0])
    offset = 8 + 2 + 12 * len(entries) + 4
    body, extra = b'', b''
    for tag, kind, count, payload in entries:
        if len(payload) > 4:
            body += struct.pack('<HHII', tag, kind, count, offset + len(extra))
            extra += payload
            if len(extra) % 2:
                extra += b'\0'
        else:
            body += struct.pack('<HHI', tag, kind, count) + payload + b'\0' * (4 - len(payload))
    while (offset + len(extra)) % 4:
        extra += b'\0'
    data_offset = offset + len(extra)
    strip = [i for i, e in enumerate(entries) if e[0] == 273][0] * 12 + 8
    body = body[:strip] + struct.pack('<I', data_offset) + body[strip + 4:]
    with open(path, 'wb') as f:
        f.write(b'II*\0' + struct.pack('<I', 8) + struct.pack('<H', len(entries)) + body + b'\0' * 4 + extra
                + pixels)
