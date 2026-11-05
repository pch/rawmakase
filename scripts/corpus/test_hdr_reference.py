"""HDR reference boundary regressions; no Photoshop or private data required."""
import unittest

import numpy as np

import hdr_reference as hdr


class HdrReferenceTests(unittest.TestCase):
    def test_pq_absolute_luminance_anchors(self):
        encoded = np.array([0, 0.508078421517399, 0.5806888810416109,
                            0.751827096247041, 1])
        np.testing.assert_allclose(hdr.pq_to_nits(encoded), [0, 100, 203, 1000, 10000],
                                   atol=1e-8)

    def test_reduction_is_linear_and_preserves_highlights_and_negatives(self):
        pixels = np.array([[[-0.5, 1, 4], [0.5, 3, 12]],
                           [[0.5, 3, 12], [-0.5, 1, 4]]])
        np.testing.assert_array_equal(hdr.reduce_linear(pixels, 2), [[[0, 2, 8]]])

    def test_reduction_keeps_partial_edge_blocks(self):
        pixels = np.ones((3, 5, 3))
        pixels[-1, -1] = 9
        reduced = hdr.reduce_linear(pixels, 2)
        self.assertEqual(reduced.shape, (2, 3, 3))
        np.testing.assert_array_equal(reduced[-1, -1], [9, 9, 9])

    def test_invalid_pixels_and_factors_are_refused(self):
        for pixels, factor in [(np.full((2, 2, 3), np.nan), 1),
                               (np.ones((2, 2, 3)), 0),
                               (np.ones((2, 2, 3)), 1.5)]:
            with self.assertRaises(ValueError):
                hdr.reduce_linear(pixels, factor)

    def test_metrics_do_not_hide_highlight_clipping(self):
        reference = np.array([[[0.5] * 3, [1.] * 3, [4.] * 3]])
        result = hdr.compare(reference, np.minimum(reference, 1))
        self.assertEqual(result['max_abs_reference_white'], 3)
        self.assertEqual(result['highlight_max_abs_reference_white'], 3)
        self.assertEqual(result['max_luminance_error_stops'], 2)

    def test_comparison_refuses_mismatched_geometry_and_nonfinite(self):
        with self.assertRaises(ValueError):
            hdr.compare(np.ones((2, 2, 3)), np.ones((1, 2, 3)))
        with self.assertRaises(ValueError):
            hdr.compare(np.ones((2, 2, 3)), np.full((2, 2, 3), np.inf))


if __name__ == '__main__':
    unittest.main()
