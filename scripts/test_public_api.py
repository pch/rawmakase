"""Tests of scripts/public-api.py: the fixture crates in
scripts/public-api-fixture list as scripts/public-api-fixture/expected.txt
says. Needs the nightly toolchain the script names, as the script does."""
import importlib.util
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("public_api", HERE / "public-api.py")
public_api = importlib.util.module_from_spec(spec)
spec.loader.exec_module(public_api)
FIXTURE = HERE / "public-api-fixture"


class Listing(unittest.TestCase):
    def test_the_fixture_lists_as_expected(self):
        have = public_api.listing("rawmakase-fixture-core", FIXTURE)
        want = (FIXTURE / "expected.txt").read_text(encoding="utf-8").splitlines()
        self.assertEqual(have, want)

    def test_a_re_exported_type_is_named_by_the_library_s_path(self):
        have = public_api.listing("rawmakase-fixture-core", FIXTURE)
        self.assertIn("pub fn rawmakase_fixture_core::Shown::kept(&self) -> u8", have)
        self.assertFalse([line for line in have if "hidden" in line or "rawmakase_fixture_sub" in line])


if __name__ == "__main__":
    if len(sys.argv) == 2 and sys.argv[1] == "write":
        lines = public_api.listing("rawmakase-fixture-core", FIXTURE)
        (FIXTURE / "expected.txt").write_text("".join(f"{line}\n" for line in lines), encoding="utf-8")
        sys.exit(0)
    unittest.main()
