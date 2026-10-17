"""Synthetic tests of the decode fingerprint comparison; no RAWs needed."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('fingerprints', Path(__file__).with_name('dataset-fingerprints.py'))
fingerprints = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fingerprints)


class Compare(unittest.TestCase):
    old = {'version': 2, 'decoder': 'a', 'header': {'metadata': {'wb': [2, 1, 1.5], 'make': 'X'}},
           'sensor': {'white': 4095}}

    def test_nested_fields_compare_by_dotted_name(self):
        new = {'version': 2, 'decoder': 'b', 'header': {'metadata': {'wb': [2, 1, 1.25], 'make': 'X'}},
               'sensor': {'white': 4095, 'cfa': 'RG/gB'}}
        self.assertEqual(fingerprints.compare(self.old, new), [
            ('header.metadata.wb', [2, 1, 1.5], [2, 1, 1.25]),
            ('sensor.cfa', None, 'RG/gB'),
        ])

    def test_an_approved_delta_is_no_change_but_another_value_is(self):
        new = {**self.old, 'sensor': {'white': 4000}}
        deltas = {('h', 'sensor.white'): (4095, 4000)}
        self.assertEqual(fingerprints.compare(self.old, new, 'h', deltas), [])
        self.assertEqual(len(fingerprints.compare(self.old, new, 'other', deltas)), 1)
        moved = {**self.old, 'sensor': {'white': 3900}}
        self.assertEqual(len(fingerprints.compare(self.old, moved, 'h', deltas)), 1)

    def test_deltas_need_evidence_and_approval(self):
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / 'deltas.toml'
            path.write_text('[[delta]]\nsha256 = "h"\nfield = "sensor.white"\nbefore = 1\nafter = 2\n')
            with self.assertRaises(fingerprints.Setup):
                fingerprints.load_deltas(path)
            path.write_text(path.read_text() + 'evidence = "e"\napproval = "a"\n')
            self.assertEqual(fingerprints.load_deltas(path), {('h', 'sensor.white'): (1, 2)})
            path.write_text(path.read_text().replace('approval = "a"', 'approval = "pending"'))
            self.assertEqual(fingerprints.load_deltas(path), {})
        # The repository's own deltas load: every entry complete, one per field.
        import tomllib
        entries = tomllib.loads(fingerprints.DELTAS.read_text()).get('delta', [])
        approved = [d for d in entries if d['approval'] != 'pending']
        self.assertEqual(len(fingerprints.load_deltas()), len(approved))


spec = importlib.util.spec_from_file_location('develop', Path(__file__).with_name('dataset-develop.py'))
develop = importlib.util.module_from_spec(spec)
spec.loader.exec_module(develop)


class DevelopCheck(unittest.TestCase):
    def test_a_baseline_file_missing_from_the_dataset_is_a_problem_unless_a_manifest_selects(self):
        import types
        with tempfile.TemporaryDirectory() as d:
            dataset = Path(d)
            sample = {'sha256': 'a' * 64, 'source': 's', 'make': 'X', 'model': 'Y', 'ext': 'raw', 'path': 'p.raw',
                      'license': 'x'}
            fingerprints.write_baseline(dataset / 'baselines/develop/s.json', {},
                                        {h: {'sha256': h, 'full': {}} for h in ('a' * 64, 'b' * 64)})
            for manifest, status in ((None, 1), (Path('m.json'), 0)):
                args = types.SimpleNamespace(dataset=dataset, manifest=manifest, public=False)
                result = develop.check(args, {'a' * 64: sample}, {'a' * 64: {'sha256': 'a' * 64, 'full': {}}}, [])
                self.assertEqual(result, status)


if __name__ == '__main__':
    unittest.main()
