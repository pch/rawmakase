"""AI Denoise scoring on synthetic images, without photos or renders."""
import importlib.util
from pathlib import Path
import unittest

import numpy as np

spec = importlib.util.spec_from_file_location('ai_denoise_score',
                                              Path(__file__).with_name('ai-denoise-score.py'))
score = importlib.util.module_from_spec(spec)
spec.loader.exec_module(score)


def scene(seed=1, size=(384, 512)):
    """A CIELAB image with flat, textured and dark regions."""
    rng = np.random.default_rng(seed)
    h, w = size
    lab = np.zeros((h, w, 3))
    lab[..., 0] = 50.
    lab[:, : w // 4, 0] = 10.
    yy, xx = np.mgrid[:h, :w]
    lab[:, w // 2:, 0] += 20 * np.sin(xx[:, w // 2:] / 3.) * np.cos(yy[:, w // 2:] / 4.)
    lab[..., 1] = 5.
    return lab + rng.normal(0, 0.01, lab.shape)


class Score(unittest.TestCase):
    def test_alignment_finds_the_shift_of_a_cropped_render(self):
        ref = scene()
        rng = np.random.default_rng(2)
        ref[..., 0] += rng.normal(0, 3, ref.shape[:2])
        img = np.roll(ref, (6, -4), axis=(0, 1))
        self.assertEqual(score.align(ref[..., 0], img[..., 0]), (6, -4))

    def test_noise_added_to_a_render_shows_in_its_chroma_and_luma(self):
        cr = scene()
        rng = np.random.default_rng(3)
        noisy = cr + np.stack([rng.normal(0, s, cr.shape[:2]) for s in (1., 2., 2.)], -1)
        m = score.compare(cr, cr, noisy)
        self.assertEqual(m['shift'], [0, 0])
        self.assertGreater(m['rm']['chroma'], 2.)
        self.assertLess(m['cr']['chroma'], 0.1)
        self.assertAlmostEqual(m['rm']['luma'], 1., delta=0.1)

    def test_a_render_framed_a_block_lower_scores_like_camera_raws(self):
        cr = scene()
        for shift in ((20, 0), (0, 22), (-20, 0)):
            m = score.compare(cr, cr, np.roll(cr, shift, axis=(0, 1)))
            self.assertEqual(tuple(m['shift']), shift)
            self.assertAlmostEqual(m['detail'], 1.)
            self.assertAlmostEqual(m['dE'], 0.)

    def test_a_render_equal_to_camera_raws_scores_no_difference(self):
        cr = scene()
        m = score.compare(cr, cr, cr.copy())
        self.assertAlmostEqual(m['detail'], 1.)
        self.assertAlmostEqual(m['dE'], 0.)
        self.assertEqual(m['rm'], m['cr'])


if __name__ == '__main__':
    unittest.main()
