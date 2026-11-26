"""Tests of scripts/ambient.py's scanner: what counts, and what is test-only."""
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import ambient  # noqa: E402


def kinds(code):
    """Each kind found in `code` outside test-only items, in order."""
    code = ambient.blank_strings_keep_features(code)
    excluded = ambient.test_only_spans(code)
    found = []
    for kind, pattern in ambient.KINDS.items():
        for m in pattern.finditer(code):
            if not any(start <= m.start() < end for start, end in excluded):
                found.append((m.start(), kind))
    return [kind for _, kind in sorted(found)]


class Kinds(unittest.TestCase):
    def test_each_kind(self):
        self.assertEqual(kinds("fn f() { std::env::var(\"X\"); }"), ["env"])
        self.assertEqual(kinds("use std::env;\nfn f() { env::current_exe(); }"), ["env"])
        self.assertEqual(kinds("fn f() { directories::UserDirs::new(); }"), ["directories"])
        self.assertEqual(kinds("fn f() { wgpu::Backends::from_env(); f.with_env(); }"), ["wgpu-env", "wgpu-env"])
        self.assertEqual(kinds("fn f() { wgpu::BackendOptions::from_env_or_default(); }"), ["wgpu-env"])
        self.assertEqual(kinds("use std::env::{var, var_os};"), ["env"])
        self.assertEqual(kinds("fn f() { Command::new(\"ps\"); }"), ["command"])
        self.assertEqual(kinds("fn f() { data_dir(); storage::asset_dirs(); }"), ["folder", "folder"])
        self.assertEqual(kinds("pub use rawmakase_protocol::paths::data_dir;"), ["folder"])
        self.assertEqual(kinds("fn f() { tempfile::tempdir(); NamedTempFile::new(); }"), ["temp", "temp"])

    def test_what_is_not_counted(self):
        # Compile-time reads, a folder given, a method of the same name, text.
        self.assertEqual(kinds("const V: &str = env!(\"CARGO_PKG_VERSION\");"), [])
        self.assertEqual(kinds("fn f() { option_env!(\"X\"); }"), [])
        self.assertEqual(kinds("fn f() { tempfile::tempdir_in(dir); NamedTempFile::new_in(dir); }"), [])
        self.assertEqual(kinds("fn f() { ctx.data_dir(); data_dir(&root); }"), [])
        self.assertEqual(kinds("// std::env::var\nfn f() { \"Command::new\"; }"), [])
        self.assertEqual(kinds("fn f() { crate::env::thing(); }"), [])


class TestOnly(unittest.TestCase):
    def test_test_items_are_left_out(self):
        self.assertEqual(kinds("#[cfg(test)]\nmod tests { fn f() { std::env::var(\"X\"); } }"), [])
        self.assertEqual(kinds("#[cfg(feature = \"test-support\")]\nfn f() { data_dir(); }"), [])
        self.assertEqual(kinds("#[cfg(any(test, feature = \"test-support\"))]\nfn f() { data_dir(); }"), [])
        self.assertEqual(kinds("#[cfg(all(test, unix))]\nfn f() { data_dir(); }"), [])

    def test_production_arms_still_count(self):
        self.assertEqual(kinds("#[cfg(any(test, unix))]\nfn f() { data_dir(); }"), ["folder"])
        self.assertEqual(kinds("#[cfg(unix)]\nfn f() { data_dir(); }"), ["folder"])
        self.assertEqual(kinds("#[cfg(not(test))]\nfn f() { data_dir(); }"), ["folder"])

    def test_a_signature_s_commas_keep_the_body_test_only(self):
        code = (
            "#[cfg(test)]\nfn dir() -> Result<tempfile::TempDir, std::io::Error> {\n"
            "    std::env::var(\"X\");\n    tempfile::tempdir()\n}\nfn f() { data_dir(); }"
        )
        self.assertEqual(kinds(code), ["folder"])
        code = "#[cfg(test)]\nimpl<A, B> T for (A, B) where A: X, B: Y { fn f() { data_dir(); } }"
        self.assertEqual(kinds(code), [])

    def test_a_test_only_field_or_arm_ends_at_its_comma(self):
        code = "match x {\n    #[cfg(test)]\n    A => data_dir(),\n    B => data_dir(),\n}"
        self.assertEqual(kinds(code), ["folder"])

    def test_only_the_item_is_left_out(self):
        code = "#[cfg(test)]\nfn t() { data_dir(); }\nfn f() { data_dir(); }"
        self.assertEqual(kinds(code), ["folder"])


if __name__ == "__main__":
    unittest.main()
