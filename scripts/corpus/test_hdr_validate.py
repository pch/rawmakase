import unittest

import numpy as np

import hdr_validate as hv


class HdrValidateTest(unittest.TestCase):
    def test_pq_and_ictcp_anchors(self):
        # BT.2100: 203 cd/m² is PQ 0.5806; 10,000 is 1.
        self.assertAlmostEqual(float(hv.pq_encode(203)), 0.5806, places=3)
        self.assertAlmostEqual(float(hv.pq_encode(10000)), 1.0, places=6)
        white = np.ones((1, 1, 3))
        i = hv.ictcp(white)[0, 0]
        # A neutral has no chroma; its intensity is PQ of 203 cd/m².
        self.assertAlmostEqual(float(i[1]), 0.0, places=4)
        self.assertAlmostEqual(float(i[2]), 0.0, places=4)
        self.assertAlmostEqual(float(i[0]), 0.5806, places=3)

    def test_metrics_report_highlight_errors_and_the_peak(self):
        reference = np.full((4, 4, 3), 0.2)
        reference[0, 0] = 4.0
        candidate = reference.copy()
        candidate[0, 0] = 8.0
        m = hv.metrics(reference, candidate)
        self.assertAlmostEqual(m['above_median_stops'], 1.0, places=6)
        self.assertAlmostEqual(m['peak_stops'], 1.0, places=6)
        self.assertAlmostEqual(m['below_median_stops'], 0.0, places=6)
        self.assertEqual(float(np.min(hv.delta_e_itp(reference, reference))), 0.0)

    def test_the_prophoto_conversion_keeps_white_neutral(self):
        p3 = np.ones(3) @ hv.PROPHOTO_TO_P3.T
        self.assertTrue(np.allclose(p3, 1.0, atol=2e-3), p3)

    def test_an_xmp_loses_only_its_camera_profile(self):
        xmp = '<x crs:Exposure2012="1" crs:CameraProfile="Adobe Standard" crs:HDREditMode="1"/>'
        self.assertEqual(hv.without_profile(xmp), '<x crs:Exposure2012="1" crs:HDREditMode="1"/>')


if __name__ == '__main__':
    unittest.main()
