import unittest
from pathlib import Path

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

    def test_a_frozen_family_reads_its_stored_arrays_and_dataset_sources(self):
        import tempfile
        with tempfile.TemporaryDirectory() as folder:
            family = Path(folder)
            (family / 'arrays').mkdir()
            (family / '000').mkdir()
            linear = np.array([[[0.25, 2.0, 15.5]]])
            np.savez_compressed(family / 'arrays' / 'a.npz', linear=linear.astype(np.float16))
            np.save(family / '000' / 'linear-p3.npy', linear)
            frozen = dict(stored_array='arrays/a.npz')
            read = hv.load_reference(family, 0, frozen)
            self.assertEqual(read.dtype, np.float64)
            np.testing.assert_array_equal(read, linear)  # these values are exact in float16
            np.testing.assert_array_equal(hv.load_reference(family, 0, {}), linear)
            self.assertEqual(hv.source({'source': 'raws/x.NEF'}, Path('/data')), '/data/raws/x.NEF')
            self.assertEqual(hv.source({'source': '/abs/x.NEF'}, Path('/data')), '/abs/x.NEF')
            self.assertEqual(hv.source({'source': '/abs/x.NEF'}, None), '/abs/x.NEF')


if __name__ == '__main__':
    unittest.main()
