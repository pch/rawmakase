"""Checks for adobe-frames.py's placement of Adobe's stored image, without Adobe apps."""
import importlib.util
from pathlib import Path
import unittest

import numpy as np

spec = importlib.util.spec_from_file_location('frames', Path(__file__).with_name('adobe-frames.py'))
f = importlib.util.module_from_spec(spec)
spec.loader.exec_module(f)


def photo(height=800, width=1000, seed=1):
    """A sensor image whose every position looks different."""
    rng = np.random.default_rng(seed)
    return rng.integers(200, 12000, (height, width)).astype(np.float32)


def smooth(height, width, sigma, seed=1):
    """Gaussian-smoothed noise, zero mean and unit spread."""
    rng = np.random.default_rng(seed)
    z = rng.standard_normal((height, width))
    ky, kx = np.fft.fftfreq(height)[:, None], np.fft.fftfreq(width)[None, :]
    g = np.exp(-2 * (np.pi * sigma) ** 2 * (kx ** 2 + ky ** 2))
    s = np.real(np.fft.ifft2(np.fft.fft2(z) * g))
    return (s - s.mean()) / s.std()


def stored(ours, dx, dy):
    """Adobe's stored image: `ours` trimmed by `dx`, `dy` on each side."""
    h, w = ours.shape
    return ours[dy:h - dy, dx:w - dx].copy()


class AlignTests(unittest.TestCase):
    def test_a_trimmed_image_is_placed_where_it_is(self):
        ours = photo()
        found = f.align(ours, [0, 0], stored(ours, 52, 34))
        self.assertEqual(found[:2], ([52, 34], 'pixels-exact'))

    def test_a_repeating_centre_does_not_decide(self):
        # A checkerboard over the centre matches at every even shift there.
        ours = photo()
        yy, xx = np.mgrid[100:700, 150:850]
        ours[100:700, 150:850] = np.where((yy + xx) % 2 == 0, 1000., 3000.)
        found = f.align(ours, [0, 0], stored(ours, 52, 34))
        self.assertEqual(found[:2], ([52, 34], 'pixels-exact'))

    def test_a_saturated_centre_does_not_decide(self):
        ours = photo()
        ours[100:700, 150:850] = 16383.
        found = f.align(ours, [0, 0], stored(ours, 52, 34))
        self.assertEqual(found[:2], ([52, 34], 'pixels-exact'))

    def test_an_image_that_repeats_everywhere_gives_no_place(self):
        yy, xx = np.mgrid[0:800, 0:1000]
        ours = np.where((yy + xx) % 2 == 0, 1000., 3000.).astype(np.float32)
        self.assertIsNone(f.align(ours, [0, 0], stored(ours, 52, 34)))

    def test_a_flat_image_gives_no_place(self):
        ours = np.full((800, 1000), 16383., dtype=np.float32)
        self.assertIsNone(f.align(ours, [0, 0], stored(ours, 52, 34)))

    def test_other_values_are_placed_by_correlation(self):
        # Adobe's values through a curve: no sample equal, the same picture.
        ours = photo()
        adobe = np.sqrt(stored(ours, 52, 34)) * 40.
        found = f.align(ours, [4, 2], adobe)
        self.assertEqual(found[:2], ([56, 36], 'pixels-correlated'))


    def test_an_offset_beyond_the_windows_is_not_taken_for_one_a_period_short(self):
        # A smooth Bayer photo trimmed by 15 and 10: the correlation gives the
        # centred offset (10, 10), whose window ends at 13, a filter period
        # short of the truth. Placed right, or not at all.
        height, width = 800, 1000
        ours = 2000 * np.exp(0.8 * smooth(height, width, 4))
        yy, xx = np.mgrid[0:height, 0:width]
        gain = np.where((yy % 2 == 0) & (xx % 2 == 0), 0.5,
                        np.where((yy % 2 == 1) & (xx % 2 == 1), 0.7, 1.0))
        ours = np.round(ours * gain).astype(np.float32)
        found = f.align(ours, [0, 0], ours[10:790, 15:995].copy())
        self.assertTrue(found is None or found[0] == [15, 10], found)

    def test_periodic_and_saturated_images_are_never_placed_wrong(self):
        height, width = 800, 1000
        rng = np.random.default_rng(3)
        yy, xx = np.mgrid[0:height, 0:width]
        cases = []
        for period in (2, 8, 16):
            tile = rng.integers(500, 12000, (period, period)).astype(np.float32)
            periodic = tile[yy % period, xx % period]
            noise = rng.integers(200, 12000, (height, width)).astype(np.float32)
            cases.append((f'P{period}', periodic.copy()))
            bordered = periodic.copy()
            ring = np.ones((height, width), bool)
            ring[60:-60, 60:-60] = False
            bordered[ring] = noise[ring]
            cases.append((f'P{period} random border', bordered))
            cases.append((f'P{period} noisy',
                          np.round(periodic + rng.normal(0, 20, (height, width))).astype(np.float32)))
        for share in (700, 940):
            o = rng.integers(200, 12000, (height, width)).astype(np.float32)
            o[:, :share] = 16383.
            cases.append((f'saturated {share}', o))
        for name, o in cases:
            for x0, y0, h, w in ((15, 10, 780, 980), (20, 20, 780, 980)):
                adobe = o[y0:y0 + h, x0:x0 + w]
                for label, a in (('', adobe), (' curve', np.round(np.sqrt(adobe) * 40))):
                    found = f.align(o, [0, 0], a)
                    self.assertTrue(found is None or found[0] == [x0, y0],
                                    f'{name}{label} {(x0, y0)}: {found}')


class TableTests(unittest.TestCase):
    def record(self, ours, active, crop):
        return {'adobe': {'default_scale': [1.0, 1.0]}, 'raw_size': [4000, 3000],
                'ours': ours, 'adobe_active': active, 'adobe_frame': crop}

    def test_a_crop_of_the_shots_aspect_ratio_gives_no_row(self):
        # A 3:2 shot of a 4:3 sensor, which Adobe crops to 3:2.
        r = self.record([72, 24, 3648, 2736], [52, 12, 3684, 2760], [72, 176, 3648, 2432])
        self.assertEqual(f.usable(r), "the shot's aspect ratio")

    def test_a_ratio_the_raw_size_gives_keeps_its_row(self):
        # A multi-aspect sensor's square raw size: RAWmakase's frame is square too.
        self.assertIsNone(f.usable(self.record([8, 8, 1368, 1368], [0, 0, 1456, 1384], [8, 8, 1368, 1368])))
        # A few pixels in on each side, of the same shape.
        self.assertIsNone(f.usable(self.record([0, 0, 3922, 2636], [0, 0, 3968, 2636], [8, 6, 3936, 2624])))

if __name__ == '__main__':
    unittest.main()
