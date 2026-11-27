import hashlib
import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

import dxc


def archive(files):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as zf:
        for name, content in files.items():
            zf.writestr(name, content)
    return buffer.getvalue()


def release_files(dll=b"dxcompiler", separator="\\"):
    return {
        f"bin{separator}x64{separator}dxcompiler.dll": dll,
        f"bin{separator}x64{separator}dxil.dll": b"unused",
        f"bin{separator}arm64{separator}dxcompiler.dll": b"arm64",
        "LICENSE-LLVM.txt": b"LLVM",
        "LICENCE-MIT.txt": b"MIT",
        "LICENSE-MS.txt": b"MS",
    }


class FetchTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)

    def fetch(self, data, archive_digest, dll_digest, arch="x86_64"):
        member, _ = dxc.LIBRARIES[arch]
        with patch.object(dxc, "ARCHIVE_SHA256", archive_digest), \
                patch.dict(dxc.LIBRARIES, {arch: (member, dll_digest)}), \
                patch("dxc.urllib.request.urlopen", return_value=io.BytesIO(data)):
            return dxc.fetch(arch, self.root / "out", self.root / "notices")

    def test_the_dll_and_notices_are_taken_from_a_checked_archive(self):
        data = archive(release_files())
        dll = self.fetch(data, hashlib.sha256(data).hexdigest(), hashlib.sha256(b"dxcompiler").hexdigest())
        self.assertEqual(dll.name, "dxcompiler.dll")
        self.assertEqual(dll.read_bytes(), b"dxcompiler")
        self.assertEqual(sorted(p.name for p in (self.root / "out").iterdir()), ["dxcompiler.dll"])
        self.assertEqual((self.root / "notices/dxc-LICENSE-LLVM.txt").read_bytes(), b"LLVM")
        self.assertEqual((self.root / "notices/dxc-LICENCE-MIT.txt").read_bytes(), b"MIT")
        self.assertEqual((self.root / "notices/dxc-LICENSE-MS.txt").read_bytes(), b"MS")

    def test_a_different_archive_is_refused_before_anything_is_written(self):
        with self.assertRaisesRegex(SystemExit, "SHA-256"):
            self.fetch(b"not the release", "0" * 64, hashlib.sha256(b"dxcompiler").hexdigest())
        self.assertFalse((self.root / "out").exists())
        self.assertFalse((self.root / "notices").exists())

    def test_a_tampered_dll_inside_the_archive_is_refused_before_writing(self):
        data = archive(release_files(dll=b"tampered"))
        with self.assertRaisesRegex(SystemExit, "SHA-256"):
            self.fetch(data, hashlib.sha256(data).hexdigest(), hashlib.sha256(b"dxcompiler").hexdigest())
        self.assertFalse((self.root / "out").exists())
        self.assertFalse((self.root / "notices").exists())

    def test_a_cached_archive_is_used_and_a_bad_download_is_not_cached(self):
        cache = self.root / "cache"
        data = archive(release_files())
        digest = hashlib.sha256(data).hexdigest()
        dll_digest = hashlib.sha256(b"dxcompiler").hexdigest()
        with patch.dict("os.environ", {"RAWMAKASE_DXC_CACHE": str(cache)}):
            with self.assertRaisesRegex(SystemExit, "SHA-256"):
                self.fetch(b"not the release", "0" * 64, dll_digest)
            self.assertFalse(cache.exists())
            self.fetch(data, digest, dll_digest)
            self.assertEqual((cache / dxc.ARCHIVE).read_bytes(), data)
            with patch.object(dxc, "ARCHIVE_SHA256", digest), \
                    patch.dict(dxc.LIBRARIES, {"x86_64": ("bin/x64/dxcompiler.dll", dll_digest)}), \
                    patch("dxc.urllib.request.urlopen") as urlopen:
                dxc.fetch("x86_64", self.root / "out2", self.root / "notices2")
            urlopen.assert_not_called()

    def test_every_windows_target_pins_a_sha256(self):
        self.assertEqual(set(dxc.LIBRARIES), {"x86_64", "aarch64"})
        self.assertIn(dxc.VERSION, dxc.URL)
        self.assertEqual(len(dxc.ARCHIVE_SHA256), 64)
        for arch, (member, digest) in dxc.LIBRARIES.items():
            self.assertTrue(member.startswith("bin/") and member.endswith("/dxcompiler.dll"), (arch, member))
            self.assertEqual(len(digest), 64, arch)


if __name__ == "__main__":
    unittest.main()
