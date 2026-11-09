import unittest

import numpy as np

import blocks
import hdr_gate as gate


def scene(height=64, width=96, seed=0):
    """A linear Display P3 image with a bright area above graphics white."""
    rng = np.random.default_rng(seed)
    image = rng.uniform(0.01, 0.6, (height, width, 3))
    image[:16, :24] *= 12  # a sky up to about 7× white
    return image


class BlocksTest(unittest.TestCase):
    def test_blocks_follow_the_sdr_grid_in_linear_light(self):
        image = scene()
        reduced = gate.blocks(image)
        rows = max(1, int(blocks.COLUMNS * 64 / 96 + 0.5))
        self.assertEqual(reduced.shape, (rows, blocks.COLUMNS, 3))
        cells = list(blocks.grid(96, 64))
        for (x, y, w, h), mean in zip(cells, reduced.reshape(-1, 3)):
            np.testing.assert_allclose(mean, image[y:y + h, x:x + w].reshape(-1, 3).mean(0), rtol=1e-6)

    def test_blocks_keep_highlights_and_negatives(self):
        image = np.full((48, 96, 3), 0.2)
        image[:, :2] = 9.0  # the first block column, above white
        image[:, 2:4, 0] = -0.05  # a P3 negative from a wide-gamut colour
        reduced = gate.blocks(image)
        self.assertAlmostEqual(float(reduced[0, 0, 1]), 9.0, places=5)
        self.assertLess(float(reduced[0, 1, 0]), 0)

    def test_blocks_refuse_what_is_not_finite_rgb(self):
        with self.assertRaises(ValueError):
            gate.blocks(np.full((8, 8, 3), np.nan))
        with self.assertRaises(ValueError):
            gate.blocks(np.ones((8, 8)))

    def test_reduction_keeps_the_factor_8_peak_and_the_share_above_white(self):
        image = np.full((64, 64, 3), 0.5)
        image[:8, :8] = 6.0  # one whole factor-8 block
        image[40, 40] = 20.0  # a single bright pixel: its factor-8 block averages to 0.8
        reduced = gate.reduce(image)
        self.assertAlmostEqual(reduced['peak'], 6.0, places=6)
        self.assertAlmostEqual(reduced['above_white_share'], 1 / 64, places=6)
        self.assertEqual(reduced['blocks'].dtype, np.float32)


class CompareTest(unittest.TestCase):
    def reference(self):
        image = scene()
        return dict(blocks=gate.blocks(image).astype(np.float32), peak=7.0,
                    dimensions=[96, 64])

    def test_identical_renders_are_within_every_tolerance(self):
        ref = self.reference()
        score = gate.compare(ref, dict(ref))
        self.assertTrue(score['valid'])
        self.assertEqual(score['failed'], [])
        self.assertAlmostEqual(score['above_median_stops'], 0.0, places=6)
        self.assertAlmostEqual(score['peak_stops'], 0.0, places=6)

    def test_a_brighter_highlight_fails_the_hdr_tone_tolerance(self):
        ref = self.reference()
        candidate = dict(ref, blocks=np.where(ref['blocks'] > 1, ref['blocks'] * 1.5, ref['blocks']),
                         peak=ref['peak'] * 1.5)
        score = gate.compare(ref, candidate)
        self.assertGreater(score['above_median_stops'], 0.5)
        self.assertIn('above_median_stops', score['failed'])
        self.assertIn('peak_stops', score['failed'])
        self.assertAlmostEqual(score['below_median_stops'], 0.0, places=6)

    def test_the_peak_is_signed_and_judged_by_its_size(self):
        ref = self.reference()
        score = gate.compare(ref, dict(ref, peak=ref['peak'] / 2 ** 0.2))
        self.assertAlmostEqual(score['peak_stops'], -0.2, places=6)
        self.assertIn('peak_stops', score['failed'])

    def test_a_photo_framed_otherwise_is_not_scored(self):
        ref = self.reference()
        score = gate.compare(ref, dict(ref, dimensions=[96, 54]))
        self.assertFalse(score['valid'])
        self.assertIn('framed', score['why'])

    def test_a_render_whose_grid_differs_is_not_scored(self):
        ref = dict(blocks=np.ones((32, 48, 3), np.float32), peak=1.0, dimensions=[960, 640])
        score = gate.compare(ref, dict(blocks=np.ones((33, 48, 3), np.float32), peak=1.0, dimensions=[960, 650]))
        self.assertFalse(score['valid'])
        self.assertIn('grid', score['why'])

    def test_a_render_a_few_pixels_smaller_is_still_compared(self):
        ref = self.reference()
        self.assertTrue(gate.compare(ref, dict(ref, dimensions=[95, 64]))['valid'])


def score(**metrics):
    base = dict(valid=True, failed=[], above_white_share=0.1, above_median_stops=0.05, above_p95_stops=0.1,
                peak_stops=0.02,
                delta_e_itp_mean=1.0, delta_e_itp_p95=3.0, below_median_stops=0.02)
    base.update(metrics)
    return base


class AcceptTest(unittest.TestCase):
    def test_an_unchanged_run_is_accepted(self):
        runs = {'a': score(), 'b': score(failed=['delta_e_itp_mean'], delta_e_itp_mean=6.0)}
        verdict = gate.accept(runs, dict(runs), ['a', 'b'])
        self.assertTrue(verdict['accepted'], verdict)

    def test_a_photo_further_from_camera_raw_fails(self):
        verdict = gate.accept({'a': score()}, {'a': score(above_median_stops=0.08)}, ['a'])
        self.assertFalse(verdict['accepted'])
        self.assertEqual(verdict['photos']['a']['worse'], ['above_median_stops'])

    def test_a_larger_peak_error_in_either_direction_fails(self):
        verdict = gate.accept({'a': score(peak_stops=0.02)}, {'a': score(peak_stops=-0.05)}, ['a'])
        self.assertEqual(verdict['photos']['a']['worse'], ['peak_stops'])

    def test_small_changes_and_improvements_pass(self):
        verdict = gate.accept({'a': score()}, {'a': score(above_median_stops=0.06, delta_e_itp_p95=1.0)}, ['a'])
        self.assertTrue(verdict['accepted'], verdict)

    def test_leaving_a_tolerance_fails_even_within_the_margin(self):
        verdict = gate.accept({'a': score(above_median_stops=0.145)},
                              {'a': score(above_median_stops=0.155, failed=['above_median_stops'])}, ['a'])
        self.assertFalse(verdict['accepted'])
        self.assertEqual(verdict['photos']['a']['left'], ['above_median_stops'])

    def test_missing_or_unrendered_photos_fail(self):
        verdict = gate.accept({'a': score(), 'b': score()}, {'a': score(), 'b': dict(error='crashed')},
                              ['a', 'b', 'c'])
        self.assertFalse(verdict['accepted'])
        self.assertIn('b', verdict['unscored'])
        self.assertIn('c', verdict['unscored'])

    def test_a_photo_neither_run_scores_fails_unless_excluded(self):
        runs = {'a': score(), 'b': dict(error='crashed')}
        self.assertEqual(gate.accept(runs, dict(runs), ['a', 'b'])['unscored'], ['b'])
        verdict = gate.accept(runs, dict(runs), ['a', 'b'], exclude={'b'})
        self.assertTrue(verdict['accepted'], verdict)
        self.assertEqual(verdict['excluded'], ['b'])

    def test_a_score_without_its_measurements_is_unscored(self):
        bare = dict(valid=True, failed=[])
        verdict = gate.accept({'a': bare}, {'a': dict(bare)}, ['a'])
        self.assertFalse(verdict['accepted'])
        self.assertEqual(verdict['unscored'], ['a'])
        nan = score(delta_e_itp_mean=float('nan'))
        self.assertEqual(gate.accept({'a': score()}, {'a': nan}, ['a'])['unscored'], ['a'])

    def test_an_incomplete_baseline_score_is_not_a_photo_that_renders_now(self):
        for bad in (dict(valid=True, failed=[]), score(peak_stops=float('nan'))):
            verdict = gate.accept({'a': bad}, {'a': score()}, ['a'])
            self.assertFalse(verdict['accepted'], bad)
            self.assertEqual(verdict['unscored'], ['a'])

    def test_a_photo_framed_otherwise_in_the_baseline_renders_now_when_the_candidate_scores_it(self):
        verdict = gate.accept({'a': dict(valid=False, why='framed otherwise')}, {'a': score()}, ['a'])
        self.assertTrue(verdict['accepted'], verdict)
        self.assertEqual(verdict['photos']['a']['note'], 'renders now')

    def test_highlight_measurements_are_required_when_blocks_are_above_white(self):
        bad = score(above_median_stops=None, above_p95_stops=None)
        self.assertEqual(gate.accept({'a': bad}, {'a': dict(bad)}, ['a'])['unscored'], ['a'])
        none_above = score(above_white_share=0.0, above_median_stops=None, above_p95_stops=None)
        self.assertTrue(gate.accept({'a': none_above}, {'a': dict(none_above)}, ['a'])['accepted'])

    def test_tolerances_are_judged_from_the_measurements_not_the_stored_list(self):
        verdict = gate.accept({'a': score(above_median_stops=0.145)}, {'a': score(above_median_stops=0.155)}, ['a'])
        self.assertFalse(verdict['accepted'])
        self.assertEqual(verdict['photos']['a']['left'], ['above_median_stops'])

    def test_an_empty_selection_is_refused(self):
        with self.assertRaises(ValueError):
            gate.accept({}, {}, [])

    def test_a_photo_that_starts_rendering_is_reported_not_failed(self):
        verdict = gate.accept({'a': dict(error='crashed')}, {'a': score()}, ['a'])
        self.assertTrue(verdict['accepted'], verdict)
        self.assertEqual(verdict['photos']['a']['note'], 'renders now')

    def test_missing_above_white_values_compare_as_absent(self):
        none = dict(above_white_share=0.0, above_median_stops=None, above_p95_stops=None)
        verdict = gate.accept({'a': score(**none)}, {'a': score(**none)}, ['a'])
        self.assertTrue(verdict['accepted'], verdict)


class RunTest(unittest.TestCase):
    def test_a_render_is_deleted_even_when_scoring_it_fails(self):
        import json
        import subprocess
        import tempfile
        import types
        from pathlib import Path
        from unittest import mock
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            refs, out, binary = root / 'refs', root / 'run', root / 'rawmakase'
            (refs / 'blocks').mkdir(parents=True)
            binary.write_text('')
            np.save(refs / 'blocks' / 'aaaaaaaaaaaaaaaa.npy', np.ones((2, 48, 3), np.float32))
            (refs / 'references.json').write_text(json.dumps([dict(
                sha256='a' * 64, path='x.NEF', make='M', model='m', split='validation',
                blocks='blocks/aaaaaaaaaaaaaaaa.npy', peak=1.0, output_dimensions=[96, 4])]))

            def render(command, **_):
                if command[1] == 'render':
                    Path(command[-1]).write_text('render')
                return types.SimpleNamespace(returncode=0, stdout='', stderr='')
            args = types.SimpleNamespace(refs=refs, out=out, binary=binary, data_dir=root, dataset=root,
                                         split=None, work=None)
            with mock.patch.object(subprocess, 'run', render), \
                    mock.patch.object(gate.hdr_validate, 'read_master', side_effect=OSError('unreadable')):
                with self.assertRaises(OSError):
                    gate.run(args)
            self.assertEqual(list(out.glob('*.tif')), [])
            self.assertEqual(list((out / 'scores').glob('*.json')), [])


class ReturnedXmpTest(unittest.TestCase):
    def test_settings_read_as_attributes_or_elements(self):
        self.assertEqual(gate.returned_tag('<x crs:ProcessVersion="15.4"/>', 'ProcessVersion'), '15.4')
        xmp = ('<crs:CameraProfile>Adobe Standard</crs:CameraProfile>'
               '<crs:CameraProfileDigest>541B</crs:CameraProfileDigest>')
        self.assertEqual(gate.returned_tag(xmp, 'CameraProfile'), 'Adobe Standard')
        self.assertEqual(gate.returned_tag(xmp, 'CameraProfileDigest'), '541B')
        self.assertIsNone(gate.returned_tag(xmp, 'ProcessVersion'))


if __name__ == '__main__':
    unittest.main()
